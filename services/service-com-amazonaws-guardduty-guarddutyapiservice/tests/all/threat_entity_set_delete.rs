// SPDX-License-Identifier: CC0-1.0

//! `DeleteThreatEntitySet`, end to end over the wire, read back through
//! `GetThreatEntitySet` and `ListThreatEntitySets`.
//!
//! `threat_entity_set_lifecycle.rs` owns the create-and-read half and
//! `threat_entity_set_listing.rs` the list; this suite owns the delete, which
//! answered HTTP 501 `NotImplemented` and so left the
//! `create -> DELETE_PENDING -> DELETED` half of the state machine unverifiable.
//! Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `DeleteThreatEntitySet` takes the set off the detector the instant it
//!    answers, and the removal it starts is OBSERVABLE on the id it named:
//!    `GetThreatEntitySet` reports `DELETE_PENDING` from the very next read and
//!    then `DELETED` once the clock owes it, whatever status the set was in when
//!    the delete arrived and whatever the create stored, so a client can poll the
//!    removal to its terminal status. A SECOND delete has nothing left to accept,
//!    so it is refused with the entity-set families' absence refusal — HTTP 400
//!    under the `InvalidInputException` class.
//! 2. The delete is PERSISTED and is observable on the detector: its own `UpdatedAt`
//!    moves on the first delete and not on a refused repeat.
//! 3. A deleted set is no longer one GuardDuty keeps, so it occupies none of the
//!    detector's slots and `ListThreatEntitySets` stops naming it. A replayed
//!    `ClientToken` whose set was deleted mints a new set rather than handing back
//!    the deleted one.
//! 4. A delete touches only the set it names: the detector's other threat entity
//!    sets, its trusted entity set and another detector's sets are untouched.
//! 5. An id the store does not resolve is refused the same way as a deleted one, and
//!    a threat entity set is a sub-resource of a REGIONAL detector, so only the
//!    Region that created it may delete it — where the DETECTOR is what the request
//!    fails to resolve, so the refusal is the detector family's.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_ACTIVE, ENTITY_SET_DELETED,
    ENTITY_SET_DELETE_PENDING, ENTITY_SET_INACTIVE, THREAT_ENTITY_SETS_PER_DETECTOR,
    THREAT_ENTITY_SET_ID_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed entity-set id (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A valid Amazon S3 location, the one member most rows here keep fixed.
const LOCATION: &str = "s3://guardduty-lists/threat.txt";

/// How long a poll may wait for a status the clock owes before the test gives up.
/// Generous on purpose: every wait below ends on the CONDITION it waits for, and this
/// is only the deadline past which the emulator is considered broken.
const SETTLE_DEADLINE: std::time::Duration = std::time::Duration::from_secs(30);

/// The gap between two reads of a poll. Only the pause between attempts; no loop
/// below ever ends on it.
const POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

/// How many times a read of a deleted id is repeated to show the refusal is not a
/// phase: the answer is the same on every one of them, so no wait is involved.
const REPEATED_READS: usize = 3;

// ---------------------------------------------------------------------------
// The four routes this suite drives, under `/detector/{DetectorId}/threatentityset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatentityset"),
        Some(body),
    )
    .await
}

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await
}

async fn get_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await
}

/// Create a threat entity set and return its id, failing the test if the create did
/// not succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, got) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected create to answer 200, got {status} {got:?}"
    );
    got["threatEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a threatEntitySetId, got {got:?}"))
        .to_string()
}

/// Read a set back, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
    let (status, got) = get_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected read to answer 200, got {status} {got:?}"
    );
    got
}

/// The `Status` a read reports.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    read(router, detector_id, set_id, case).await["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Read a set until it reports `want`, or until [`SETTLE_DEADLINE`] passes — the
/// CONDITION ends the wait, never a fixed pause. Answers the read that saw it, or
/// `None` once the service has stopped holding the record: that read is the family's
/// absence refusal, so a wait cannot outlive the record it waits on.
async fn read_until_status(
    router: &Router,
    detector_id: &str,
    set_id: &str,
    want: &str,
    case: &str,
) -> Option<Value> {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, got) = get_set(router, detector_id, set_id).await;
        if status != 200 {
            assert_entity_set_absent(case, status, &got, set_id);
            return None;
        }
        if got["status"].as_str() == Some(want) {
            return Some(got);
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: {set_id} never reached {want}, last read {got:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Assert every one of a handful of consecutive reads of `id` reports a set on its
/// way out: the delete this service served left the record readable, so each read
/// answers 200 with `DELETE_PENDING` or the terminal `DELETED`, and nothing the
/// clock does walks it back to a live status.
async fn assert_stays_deleting(router: &Router, detector_id: &str, id: &str, case: &str) {
    let mut settled = false;
    for attempt in 0..REPEATED_READS {
        let (status, body) = get_set(router, detector_id, id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: read {attempt} of a retained deleted record must answer 200, got \
             {status} {body:?}"
        );
        let reported = body["status"].as_str().unwrap_or_default();
        assert!(
            reported == ENTITY_SET_DELETE_PENDING || reported == ENTITY_SET_DELETED,
            "case {case:?}: read {attempt} must report {ENTITY_SET_DELETE_PENDING} or \
             {ENTITY_SET_DELETED}, got {reported:?} {body:?}"
        );
        assert!(
            !(settled && reported == ENTITY_SET_DELETE_PENDING),
            "case {case:?}: read {attempt} walked a settled {ENTITY_SET_DELETED} back to \
             {reported:?}, got {body:?}"
        );
        settled |= reported == ENTITY_SET_DELETED;
    }
}

/// Delete a set, failing the test if the call did not succeed.
async fn deleted(router: &Router, detector_id: &str, set_id: &str, case: &str) {
    let (status, got) = delete_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected delete to answer 200, got {status} {got:?}"
    );
    assert!(
        got.get("status").is_none(),
        "case {case:?}: the delete's output is Unit, so it reports no entity set, got {got:?}"
    );
}

/// The ids `ListThreatEntitySets` names for a detector. The whole collection is
/// bounded well under `ThreatEntitySetIds`'s own `len 0..50`, so it arrives in one
/// page and this helper needs no token walk (`threat_entity_set_listing.rs` owns
/// pagination).
async fn listed(router: &Router, detector_id: &str, case: &str) -> Vec<String> {
    let (status, got) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/threatentityset"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected list to answer 200, got {status} {got:?}"
    );
    assert!(
        got["nextToken"].is_null(),
        "case {case:?}: the whole collection fits one page, got {got:?}"
    );
    got["threatEntitySetIds"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected threatEntitySetIds, got {got:?}"))
        .iter()
        .map(|id| id.as_str().unwrap_or_default().to_string())
        .collect()
}

/// `GetDetector`, so an assertion can see the detector's own `UpdatedAt`.
async fn read_detector(router: &Router, detector_id: &str, case: &str) -> Value {
    let (status, got) = call(router, "GET", &format!("/detector/{detector_id}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {got:?}"
    );
    got
}

/// A detector's own `UpdatedAt`, which `GetDetector` reports as an ISO-8601 string
/// rather than as the epoch seconds an entity set's timestamps carry
/// (`smithy_query(operation="GetDetector")`). The format is fixed and UTC, so the
/// strings order the same way the instants do.
fn detector_updated_at(got: &Value, case: &str) -> String {
    got["updatedAt"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected the detector's updatedAt, got {got:?}"))
        .to_string()
}

/// A router holding one enabled detector.
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

/// Assert a refusal is this service's classified HTTP 400, names `member` and echoes
/// none of `never_echoed`.
fn assert_classified_refusal(
    case: &str,
    status: u16,
    body: &Value,
    member: &str,
    never_echoed: &[&str],
) {
    assert_eq!(
        status, 400,
        "case {case:?}: expected 400, got {status} {body:?}"
    );
    let class = error_code(body);
    assert_eq!(
        class, INVALID_INPUT,
        "case {case:?}: expected the {INVALID_INPUT} class, got {class:?} {body:?}"
    );
    let message = body["message"].as_str().unwrap_or_default();
    assert!(
        message.contains(member),
        "case {case:?}: the refusal must name {member}, got {message:?}"
    );
    for value in never_echoed {
        assert!(
            !message.contains(value),
            "case {case:?}: the refusal must not echo {value:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 1 + 2: the set is gone, and the detector records that it is.
// ---------------------------------------------------------------------------

/// One delete case: the create body, and the status the set is in when the delete
/// arrives.
///
/// The rows differ along everything the rule generalises over: the status the set
/// held when the delete arrived (each of the three a created set can be in, the
/// transitional one included), the FORMAT (drawn from
/// `ThreatEntitySetFormat`'s modelled values, `smithy_query(shape=
/// "ThreatEntitySetFormat")`), whether the caller activated the set, whether it
/// carried an expected bucket owner, the number of TAGS and the SIZE of the name
/// (both ends of the modelled `len 1..300`).
struct DeleteRow {
    case: &'static str,
    create: Value,
    /// Wait for the create's transition to settle before deleting.
    settle_first: bool,
    status_when_deleted: &'static str,
}

fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "an INACTIVE set, created without tags",
            create: json!({
                "name": "a",
                "format": "TXT",
                "location": LOCATION,
                "activate": false,
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_INACTIVE,
        },
        DeleteRow {
            case: "an ACTIVATING set, deleted mid-transition",
            create: json!({
                "name": "activating-threat-set",
                "format": "PROOF_POINT",
                "location": "https://s3.amazonaws.com/guardduty-lists/threat.pp",
                "activate": true,
                "tags": {"team": "secops"},
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_ACTIVATING,
        },
        DeleteRow {
            case: "an ACTIVE set, settled before the delete",
            create: json!({
                "name": "ALIEN_vault-1",
                "format": "ALIEN_VAULT",
                "location": "https://guardduty-lists.s3.us-west-2.amazonaws.com/otx.csv",
                "activate": true,
                "expectedBucketOwner": "123456789012",
                "tags": {"team": "secops", "env": "test"},
            }),
            settle_first: true,
            status_when_deleted: ENTITY_SET_ACTIVE,
        },
        DeleteRow {
            case: "an INACTIVE set with the longest name the model accepts",
            create: json!({
                "name": "f".repeat(300),
                "format": "FIRE_EYE",
                "location": "s3://guardduty-lists/deep/nested/fire-eye.txt",
                "activate": false,
                "expectedBucketOwner": "000000000000",
            }),
            settle_first: true,
            status_when_deleted: ENTITY_SET_INACTIVE,
        },
    ]
}

#[tokio::test]
async fn a_delete_takes_the_set_off_the_detector_at_once() {
    for row in delete_rows() {
        let case = row.case;
        let (router, detector_id) = detector().await;
        let set_id = created(&router, &detector_id, case, row.create.clone()).await;
        if row.settle_first {
            let settled = read_until_status(
                &router,
                &detector_id,
                &set_id,
                row.status_when_deleted,
                case,
            )
            .await;
            assert!(
                settled.is_some(),
                "case {case:?}: a set nothing deleted must stay readable until it settles"
            );
        }

        let before = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            before["status"], row.status_when_deleted,
            "case {case:?}: expected status {} before the delete, got {:?}",
            row.status_when_deleted, before["status"]
        );
        let detector_before = read_detector(&router, &detector_id, case).await;

        deleted(&router, &detector_id, &set_id, case).await;

        // Rule 1: the delete took the set off the detector and left the removal
        // OBSERVABLE on its id: the very next read reports the transitional status,
        // and the clock settles it on the terminal one.
        let after = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            after["status"], ENTITY_SET_DELETE_PENDING,
            "case {case:?}: expected {ENTITY_SET_DELETE_PENDING} straight after the delete, got \
             {after:?}"
        );
        // Everything the create stored is still reported: the delete moved the
        // status and nothing else.
        assert_eq!(
            after["name"], before["name"],
            "case {case:?}: a deleted record must still report the stored name, got {after:?}"
        );
        assert_eq!(
            after["format"], before["format"],
            "case {case:?}: a deleted record must still report the stored format, got {after:?}"
        );
        assert_eq!(
            after["location"], before["location"],
            "case {case:?}: a deleted record must still report the stored location, got {after:?}"
        );
        let settled = read_until_status(
            &router,
            &detector_id,
            &set_id,
            ENTITY_SET_DELETED,
            case,
        )
        .await;
        let settled = settled.unwrap_or_else(|| {
            panic!("case {case:?}: the retained record must settle on {ENTITY_SET_DELETED}")
        });
        assert_eq!(
            settled["name"], before["name"],
            "case {case:?}: a settled deletion still reports the stored name, got {settled:?}"
        );

        // Rule 2: the detector holds one fewer sub-resource, so its own UpdatedAt
        // moved.
        let detector_after = read_detector(&router, &detector_id, case).await;
        assert!(
            detector_updated_at(&detector_after, case) > detector_updated_at(&detector_before, case),
            "case {case:?}: the first delete changed the detector, so its updatedAt must move, \
             got {:?} after {:?}",
            detector_after["updatedAt"],
            detector_before["updatedAt"]
        );

        // A second delete is the same absence refusal, and moves neither the store
        // nor the detector.
        let (status, body) = delete_set(&router, &detector_id, &set_id).await;
        assert_entity_set_absent(case, status, &body, &set_id);
        let detector_again = read_detector(&router, &detector_id, case).await;
        assert_eq!(
            detector_updated_at(&detector_again, case),
            detector_updated_at(&detector_after, case),
            "case {case:?}: a refused second delete changed nothing, so the detector's updatedAt \
             must stay put, got {detector_again:?}"
        );

        // And the removal is terminal: every later read reports a set on its way
        // out, so nothing the clock does puts it back into use.
        assert_stays_deleting(&router, &detector_id, &set_id, case).await;
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a deleted set leaves the collection and frees its slot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_deleted_set_leaves_the_collection_and_frees_its_slot() {
    // The rows differ along WHICH of the detector's sets is deleted — the first, one
    // in the middle and the last — so the rule is not about a position in the
    // collection.
    for (case, position) in [
        ("the first set deleted", 0),
        ("a middle set deleted", THREAT_ENTITY_SETS_PER_DETECTOR / 2),
        ("the last set deleted", THREAT_ENTITY_SETS_PER_DETECTOR - 1),
    ] {
        let (router, detector_id) = detector().await;

        // Fill the detector to its ceiling, so the next create has nowhere to go.
        let mut ids = Vec::new();
        for index in 0..THREAT_ENTITY_SETS_PER_DETECTOR {
            ids.push(
                created(
                    &router,
                    &detector_id,
                    case,
                    json!({
                        "name": format!("threat-set-{index}"),
                        "format": "TXT",
                        "location": format!("s3://guardduty-lists/threat-{index}.txt"),
                        "activate": index % 2 == 0,
                    }),
                )
                .await,
            );
        }
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(
            listed(&router, &detector_id, case).await,
            expected,
            "case {case:?}: the list must name every created set"
        );

        let (status, refused) = create_set(
            &router,
            &detector_id,
            json!({
                "name": "one-too-many",
                "format": "STIX",
                "location": LOCATION,
                "activate": false,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a create past the ceiling to be refused, got {status} \
             {refused:?}"
        );

        // The set this row deletes, taken from the ids in the order they were
        // created rather than from the listing's order.
        let victim = ids[position].clone();
        deleted(&router, &detector_id, &victim, case).await;

        let remaining = listed(&router, &detector_id, case).await;
        assert!(
            !remaining.contains(&victim),
            "case {case:?}: a deleted set is no longer part of the collection, got {remaining:?}"
        );
        let mut still_there: Vec<String> =
            ids.iter().filter(|id| **id != victim).cloned().collect();
        still_there.sort();
        assert_eq!(
            remaining, still_there,
            "case {case:?}: the delete must leave every other set in the list"
        );

        // It has left the collection but not the reader: the detector no longer
        // holds it, while its retained record still reports the removal.
        assert_stays_deleting(&router, &detector_id, &victim, case).await;

        // The slot is free again, so the create that was just refused now succeeds.
        let replacement = created(
            &router,
            &detector_id,
            case,
            json!({
                "name": "one-too-many",
                "format": "STIX",
                "location": LOCATION,
                "activate": false,
            }),
        )
        .await;
        assert!(
            !ids.contains(&replacement),
            "case {case:?}: the create after the delete must mint a new id, got {replacement}"
        );
        let after = listed(&router, &detector_id, case).await;
        assert_eq!(
            after.len(),
            THREAT_ENTITY_SETS_PER_DETECTOR,
            "case {case:?}: the detector is back at its ceiling, got {after:?}"
        );
    }
}

#[tokio::test]
async fn a_replayed_client_token_whose_set_was_deleted_mints_a_new_set() {
    // The rows differ along the token the create carries and whether the set it
    // minted was activated, so the rule is not about one token spelling.
    for (case, token, activate, settled_status) in [
        ("a plain token on an activated set", "replayed-token", true, ENTITY_SET_ACTIVATING),
        (
            "a uuid-shaped token on a set left unused",
            "6d5f3b2e-0a1c-4d7e-9b8a-2c3d4e5f6a7b",
            false,
            ENTITY_SET_INACTIVE,
        ),
    ] {
        let (router, detector_id) = detector().await;
        let body = json!({
            "name": "token-set",
            "format": "OTX_CSV",
            "location": LOCATION,
            "activate": activate,
            "clientToken": token,
        });

        let first = created(&router, &detector_id, case, body.clone()).await;
        // Before the delete the token is a pure replay key: it hands back that set.
        let replay = created(&router, &detector_id, case, body.clone()).await;
        assert_eq!(
            replay, first,
            "case {case:?}: a replayed token must hand back the set it minted"
        );

        deleted(&router, &detector_id, &first, case).await;
        // The deleted set is no longer one the detector holds, so the token resolves
        // to nothing even though the record is still readable.
        assert_stays_deleting(&router, &detector_id, &first, case).await;

        let second = created(&router, &detector_id, case, body).await;
        assert_ne!(
            second, first,
            "case {case:?}: a replayed token whose set was deleted must mint a new set, got \
             {second} again"
        );
        let got = read(&router, &detector_id, &second, case).await;
        assert_eq!(
            got["status"], settled_status,
            "case {case:?}: expected the new set to start on {settled_status}, got {:?}",
            got["status"]
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a delete touches only the set it names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_touches_only_the_set_it_names() {
    let router = new_router();
    let case = "sibling resources";
    let first_detector = enabled_detector(&router).await;
    // A second detector in another Region, so it is a store of its own.
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        "/detector",
        Some(json!({"enable": true})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the second detector to be created, got {status} {body:?}"
    );
    let other_detector = body["detectorId"]
        .as_str()
        .expect("a detectorId")
        .to_string();

    let mine = created(
        &router,
        &first_detector,
        case,
        json!({
            "name": "mine",
            "format": "TXT",
            "location": LOCATION,
            "activate": false,
        }),
    )
    .await;
    // A SECOND threat entity set under the same detector: a detector may hold
    // several, so a delete has siblings in its own collection to leave alone.
    let sibling = created(
        &router,
        &first_detector,
        case,
        json!({
            "name": "sibling",
            "format": "STIX",
            "location": "s3://guardduty-lists/sibling.txt",
            "activate": false,
        }),
    )
    .await;
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{other_detector}/threatentityset"),
        Some(json!({
            "name": "theirs",
            "format": "TXT",
            "location": LOCATION,
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other detector's create to answer 200, got {status} {body:?}"
    );
    let theirs = body["threatEntitySetId"]
        .as_str()
        .expect("a threatEntitySetId")
        .to_string();

    // A TRUSTED entity set under the same detector: a different collection, which a
    // threat delete must not reach into.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{first_detector}/trustedentityset"),
        Some(json!({
            "name": "trusted-set",
            "format": "TXT",
            "location": "s3://guardduty-lists/trusted.txt",
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the trusted create to answer 200, got {status} {body:?}"
    );
    let trusted = body["trustedEntitySetId"]
        .as_str()
        .expect("a trustedEntitySetId")
        .to_string();

    deleted(&router, &first_detector, &mine, case).await;

    let sibling_status = status_of(&router, &first_detector, &sibling, case).await;
    assert_eq!(
        sibling_status, ENTITY_SET_INACTIVE,
        "case {case:?}: a sibling threat entity set must be untouched, got {sibling_status:?}"
    );

    let (status, other) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{other_detector}/threatentityset/{theirs}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the other detector's set to still read, got {status} {other:?}"
    );
    assert_eq!(
        other["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: another detector's set must be untouched, got {other:?}"
    );

    let (status, trusted_set) = call(
        &router,
        "GET",
        &format!("/detector/{first_detector}/trustedentityset/{trusted}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the trusted set to still read, got {status} {trusted_set:?}"
    );
    assert_eq!(
        trusted_set["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a threat delete must not touch a trusted entity set, got {trusted_set:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: an id the store does not resolve, and the Region that owns the set.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_rather_than_answered() {
    let (router, detector_id) = detector().await;
    let set_id = created(
        &router,
        &detector_id,
        "rejections",
        json!({
            "name": "rejection-set",
            "format": "TXT",
            "location": LOCATION,
            "activate": true,
        }),
    )
    .await;

    // Each row is an id no store resolves: a well-formed detector id and a value
    // that cannot be one, then a well-formed set id and a value that cannot be one.
    // The detector rows are refused with the detector family's own refusal and the
    // set rows with the entity-set families' absence refusal, because they name
    // different resources.
    for (case, detector_arg) in [
        ("absent detector", ABSENT_DETECTOR_ID.to_string()),
        ("malformed detector", "not-a-detector".to_string()),
    ] {
        let (status, body) = delete_set(&router, &detector_arg, &set_id).await;
        assert_classified_refusal(
            case,
            status,
            &body,
            DETECTOR_ID_MEMBER,
            &[detector_arg.as_str(), set_id.as_str()],
        );
        // The refusal deleted nothing: the set that does exist is still live.
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not touch the stored set, got {status} {body:?}"
        );
    }
    // A well-formed set id the detector does not hold is the entity-set families'
    // absence refusal; one outside the alphabet the member is constrained to is the
    // constraint violation, decided from the request before anything is resolved.
    for (case, set_arg, well_formed) in [
        ("absent set", ABSENT_SET_ID, true),
        ("malformed set", "not-a-set", false),
    ] {
        let (status, body) = delete_set(&router, &detector_id, set_arg).await;
        if well_formed {
            assert_entity_set_absent(case, status, &body, set_arg);
        } else {
            assert_entity_set_id_constraint_violation(
                case,
                status,
                &body,
                THREAT_ENTITY_SET_ID_MEMBER,
                set_arg,
            );
        }
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not touch the stored set, got {status} {body:?}"
        );
    }

    // A threat entity set is a sub-resource of a REGIONAL detector, so a request
    // signed for another Region resolves neither the detector nor the set.
    let (status, body) = call_in(
        &router,
        "ap-southeast-2",
        "DELETE",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        None,
    )
    .await;
    assert_classified_refusal(
        "another Region",
        status,
        &body,
        DETECTOR_ID_MEMBER,
        &[detector_id.as_str(), set_id.as_str()],
    );
    let survived = status_of(&router, &detector_id, &set_id, "another Region").await;
    // The refusal moved nothing, so the set is still one of the detector's LIVE
    // sets: it is in whichever status its own transition has reached, never one the
    // delete would have put it in.
    assert!(
        survived == ENTITY_SET_ACTIVATING || survived == ENTITY_SET_ACTIVE,
        "case \"another Region\": the set must be untouched, so it is still \
         {ENTITY_SET_ACTIVATING} or {ENTITY_SET_ACTIVE}, got {survived:?}"
    );
}
