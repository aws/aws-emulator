// SPDX-License-Identifier: CC0-1.0

//! `DeleteTrustedEntitySet`, end to end over the wire, read back through
//! `GetTrustedEntitySet`.
//!
//! `trusted_entity_set_lifecycle.rs` owns the create-and-read half; this suite owns
//! the delete, which answered HTTP 501 `NotImplemented` and so left the
//! create-and-delete half of the state machine unverifiable.
//! Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `DeleteTrustedEntitySet` takes the set off the detector the instant it
//!    answers, and the removal it starts is OBSERVABLE on the id it named:
//!    `GetTrustedEntitySet` reports `DELETE_PENDING` from the very next read and
//!    then `DELETED` once the clock owes it, whatever status the set was in when
//!    the delete arrived and whatever the create stored, so a client can poll the
//!    removal to its terminal status. A SECOND delete has nothing left to accept,
//!    so it is refused with the entity-set families' absence refusal — HTTP 400
//!    under the `InvalidInputException` class.
//! 2. The delete is PERSISTED and is observable on the detector: its own `UpdatedAt`
//!    moves on the first delete and not on a refused repeat.
//! 3. A deleted set is no longer one GuardDuty keeps, so it occupies none of the
//!    detector's slots: "at any given time, you can have only one trusted entity
//!    set", and a delete frees that one. A replayed `ClientToken` whose set was
//!    deleted mints a new set rather than handing back the deleted one.
//! 4. A delete touches only the set it names: another detector's trusted set and the
//!    same detector's threat entity sets are untouched.
//! 5. An id the store does not resolve is refused the same way as a deleted one, and
//!    a trusted entity set is a sub-resource of a REGIONAL detector, so only the
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
    ENTITY_SET_DELETE_PENDING, ENTITY_SET_INACTIVE, TRUSTED_ENTITY_SET_ID_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed entity-set id (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A valid Amazon S3 location, the one member every row here keeps fixed.
const LOCATION: &str = "s3://guardduty-lists/trusted.txt";

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
// The three routes this suite drives, under `/detector/{DetectorId}/trustedentityset`.
// ---------------------------------------------------------------------------

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/trustedentityset"),
        Some(body),
    )
    .await
}

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await
}

async fn get_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
        None,
    )
    .await
}

/// Create a trusted entity set and return its id, failing the test if the create did
/// not succeed.
async fn created(router: &Router, detector_id: &str, case: &str, body: Value) -> String {
    let (status, got) = create_set(router, detector_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected create to answer 200, got {status} {got:?}"
    );
    got["trustedEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected a trustedEntitySetId, got {got:?}"))
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
/// transitional one included), the FORMAT, whether the caller activated the set,
/// whether it carried an expected bucket owner, the number of TAGS and the SIZE of
/// the name (both ends of the modelled `len 1..300`).
struct DeleteRow {
    case: &'static str,
    create: Value,
    /// Wait past the transition before deleting, so the set is settled first.
    settle_first: bool,
    status_when_deleted: &'static str,
}

fn delete_rows() -> Vec<DeleteRow> {
    vec![
        DeleteRow {
            case: "an INACTIVE set, created without tags",
            create: json!({
                "name": "inactive-set",
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
                "name": "activating-set",
                "format": "STIX",
                "location": "https://s3.amazonaws.com/guardduty-lists/trusted.stix",
                "activate": true,
                "tags": {"team": "secops"},
            }),
            settle_first: false,
            status_when_deleted: ENTITY_SET_ACTIVATING,
        },
        DeleteRow {
            case: "an ACTIVE set, settled before the delete",
            create: json!({
                "name": "ACTIVE_set-1",
                "format": "OTX_CSV",
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
        // One set per detector: "at any given time, you can have only one trusted
        // entity set", so each row gets its own store.
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
        let settled =
            read_until_status(&router, &detector_id, &set_id, ENTITY_SET_DELETED, case).await;
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
// Rule 3: a delete frees the detector's single slot.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_delete_frees_the_detectors_single_trusted_entity_set_slot() {
    // The rows differ along whether the creates carry an idempotency token at all,
    // and along the token's spelling: a replayed token whose set was deleted must
    // mint a new set rather than hand the deleted one back.
    for (case, token) in [
        ("creates without a clientToken", None),
        (
            "a replayed clientToken whose set was deleted",
            Some("replayed-token"),
        ),
        (
            "a replayed uuid-shaped clientToken whose set was deleted",
            Some("6d5f3b2e-0a1c-4d7e-9b8a-2c3d4e5f6a7b"),
        ),
    ] {
        let (router, detector_id) = detector().await;
        let mut body = json!({
            "name": "first-set",
            "format": "TXT",
            "location": LOCATION,
            "activate": true,
        });
        if let Some(token) = token {
            body["clientToken"] = json!(token);
        }
        let first = created(&router, &detector_id, case, body.clone()).await;

        // Before the delete the single slot is taken, so a second create is refused.
        let (status, refused) = create_set(
            &router,
            &detector_id,
            json!({
                "name": "second-set",
                "format": "STIX",
                "location": "s3://guardduty-lists/other.txt",
                "activate": false,
                "clientToken": "a-different-token",
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a second create to be refused while the slot is taken, \
             got {status} {refused:?}"
        );

        deleted(&router, &detector_id, &first, case).await;
        // The deleted set left the detector's single slot, while its retained record
        // still reports the removal on that read and on every later one.
        assert_stays_deleting(&router, &detector_id, &first, case).await;

        // The slot is free again, so the create that was just refused now succeeds —
        // under the SAME token as the deleted set, too: a replayed token must not
        // hand back a set the service is no longer keeping.
        let second = created(&router, &detector_id, case, body).await;
        assert_ne!(
            second, first,
            "case {case:?}: the create after the delete must mint a new set, got {second} again"
        );
        let got = read(&router, &detector_id, &second, case).await;
        assert_eq!(
            got["status"], ENTITY_SET_ACTIVATING,
            "case {case:?}: the new set starts its own transition, got {:?}",
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
    let (status, body) = call_in(
        &router,
        "eu-west-1",
        "POST",
        &format!("/detector/{other_detector}/trustedentityset"),
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
    let theirs = body["trustedEntitySetId"]
        .as_str()
        .expect("a trustedEntitySetId")
        .to_string();

    // A THREAT entity set under the same detector: a different collection, which a
    // trusted delete must not reach into.
    let (status, body) = call(
        &router,
        "POST",
        &format!("/detector/{first_detector}/threatentityset"),
        Some(json!({
            "name": "threat-set",
            "format": "TXT",
            "location": "s3://guardduty-lists/threat.txt",
            "activate": false,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the threat create to answer 200, got {status} {body:?}"
    );
    let threat = body["threatEntitySetId"]
        .as_str()
        .expect("a threatEntitySetId")
        .to_string();

    deleted(&router, &first_detector, &mine, case).await;

    let (status, other) = call_in(
        &router,
        "eu-west-1",
        "GET",
        &format!("/detector/{other_detector}/trustedentityset/{theirs}"),
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

    let (status, threat_set) = call(
        &router,
        "GET",
        &format!("/detector/{first_detector}/threatentityset/{threat}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected the threat set to still read, got {status} {threat_set:?}"
    );
    assert_eq!(
        threat_set["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a trusted delete must not touch a threat entity set, got {threat_set:?}"
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
                TRUSTED_ENTITY_SET_ID_MEMBER,
                set_arg,
            );
        }
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a refused delete must not touch the stored set, got {status} {body:?}"
        );
    }

    // A trusted entity set is a sub-resource of a REGIONAL detector, so a request
    // signed for another Region resolves neither the detector nor the set.
    let (status, body) = call_in(
        &router,
        "ap-southeast-2",
        "DELETE",
        &format!("/detector/{detector_id}/trustedentityset/{set_id}"),
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
    assert_eq!(
        survived, ENTITY_SET_ACTIVATING,
        "case \"another Region\": the set must be untouched, got {survived:?}"
    );
}
