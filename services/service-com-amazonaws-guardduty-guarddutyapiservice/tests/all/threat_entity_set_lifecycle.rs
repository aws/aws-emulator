// SPDX-License-Identifier: CC0-1.0

//! GuardDuty threat entity sets, end to end over the wire.
//!
//! Every request goes through [`smithy_server_runtime::MultiServiceRouter::dispatch`]
//! (the harness lives in the `detector_lifecycle` suite), so routing, restJson1
//! (de)serialization, modelled-constraint validation and the handlers are exercised
//! together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one recorded
//! call:
//!
//! 1. `CreateThreatEntitySet` PERSISTS the set under the detector and mints its id;
//!    `GetThreatEntitySet` reports back every member the create stored — the name, the
//!    format, the location, the expected bucket owner and the tags.
//! 2. A set the caller asked to ACTIVATE starts `ACTIVATING` and reaches `ACTIVE` only
//!    when the CLOCK owes it (`state::ENTITY_SET_TRANSITION_MILLIS`); one the caller
//!    did not ask to activate is `INACTIVE` and stays there. A read never moves a
//!    status early and never changes what a later read sees.
//! 3. `ClientToken` is `@idempotencyToken`: the same token returns the set the first
//!    call minted. AWS documents no one-at-a-time rule for a THREAT entity set, so a
//!    create under a different token mints a SECOND set, up to
//!    `state::THREAT_ENTITY_SETS_PER_DETECTOR`; the create past that is refused and
//!    leaves the stored sets untouched.
//! 4. `Location` must be a valid Amazon S3 URL — "invalid URL formats will result in
//!    an error, regardless of whether you activate the entity set or not" — so the S3
//!    URL forms are accepted, anything else is refused with HTTP 400, and a refused
//!    create persists nothing.
//! 5. A `Name` may hold only letters, numbers, dash and underscore; anything else is
//!    refused with HTTP 400.
//! 6. An entity-set id the detector does not hold is refused with the entity-set
//!    families' absence refusal — HTTP 404 under the `ResourceNotFoundException`
//!    class — never answered with a hollow 200; and the threat and trusted entity
//!    sets are separate resources: neither id resolves through the other's path.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    ENTITY_SET_TRANSITION_MILLIS, THREAT_ENTITY_SETS_PER_DETECTOR, THREAT_ENTITY_SET_ID_MEMBER,
};

/// `ThreatEntitySetStatus` members (`smithy_query(shape="ThreatEntitySetStatus")`).
const INACTIVE: &str = "INACTIVE";
const ACTIVATING: &str = "ACTIVATING";
const ACTIVE: &str = "ACTIVE";

/// The error class every client rejection on this service carries in `__type`.
const INVALID_INPUT: &str = "InvalidInputException";

/// Every member of `ThreatEntitySetFormat`
/// (`smithy_query(shape="ThreatEntitySetFormat")`) — the whole catalogue, so a format
/// no issue cited is exercised too.
const FORMATS: [&str; 6] = [
    "TXT",
    "STIX",
    "OTX_CSV",
    "ALIEN_VAULT",
    "PROOF_POINT",
    "FIRE_EYE",
];

/// A well-formed entity-set id (32 lowercase hex characters) no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// Long enough for the clock to owe the activated status, plus slack.
fn past_the_transition() -> std::time::Duration {
    std::time::Duration::from_millis(ENTITY_SET_TRANSITION_MILLIS as u64 + 250)
}

async fn create_set(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatentityset"),
        Some(body),
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

/// Create a set and return its id, failing the test if the create did not succeed.
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

/// The set a read reports, failing the test if the read did not succeed.
async fn read(router: &Router, detector_id: &str, set_id: &str, case: &str) -> Value {
    let (status, got) = get_set(router, detector_id, set_id).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected read to answer 200, got {status} {got:?}"
    );
    got
}

/// A router holding one enabled detector.
async fn detector() -> (Router, String) {
    let router = new_router();
    let detector_id = enabled_detector(&router).await;
    (router, detector_id)
}

// ---------------------------------------------------------------------------
// Rule 1: what a create stored is what a read reports.
// ---------------------------------------------------------------------------

/// The rows differ along everything the rule generalises over: the FORMAT (every
/// member of the enum), whether the caller asked to ACTIVATE, the NAME (each
/// character class the member admits, at both ends of the modelled `len 1..300`),
/// the S3 URL FORM of the location, whether an expected bucket owner is given, and
/// the number of TAGS.
fn create_rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "txt-activated-minimal",
            json!({
                "name": "T",
                "format": FORMATS[0],
                "location": "s3://guardduty-lists/threat.txt",
                "activate": true,
            }),
        ),
        (
            "stix-inactive-path-style-url",
            json!({
                "name": "stix_threat-set_2",
                "format": FORMATS[1],
                "location": "https://s3.amazonaws.com/guardduty-lists/threat.stix",
                "activate": false,
                "expectedBucketOwner": "123456789012",
            }),
        ),
        (
            "otx-csv-regional-path-style-url-and-tags",
            json!({
                "name": "OTX-CSV-threats",
                "format": FORMATS[2],
                "location": "https://s3.eu-west-1.amazonaws.com/guardduty-lists/otx.csv",
                "activate": true,
                "tags": {"team": "secops", "env": "test"},
            }),
        ),
        (
            "alien-vault-virtual-hosted-url",
            json!({
                "name": "alienvault",
                "format": FORMATS[3],
                "location": "https://guardduty-lists.s3.us-west-2.amazonaws.com/nested/path/av.txt",
                "activate": false,
                "tags": {"only": "one"},
            }),
        ),
        (
            "proof-point-dashed-regional-url",
            json!({
                "name": "proof_point_0",
                "format": FORMATS[4],
                "location": "https://s3-ap-southeast-2.amazonaws.com/guardduty-lists/pp.txt",
                "activate": true,
                "expectedBucketOwner": "000000000000",
            }),
        ),
        (
            "fire-eye-longest-name",
            json!({
                "name": "f".repeat(300),
                "format": FORMATS[5],
                "location": "s3://guardduty-lists/deep/fire-eye.txt",
                "activate": false,
            }),
        ),
    ]
}

#[tokio::test]
async fn a_created_threat_entity_set_is_read_back_from_stored_state() {
    // Several threat entity sets may live under ONE detector, so every row goes into
    // the same store and each read must still report its own row.
    let (router, detector_id) = detector().await;
    let rows = create_rows();
    assert!(
        rows.len() <= THREAT_ENTITY_SETS_PER_DETECTOR,
        "the table must fit under the per-detector limit of {THREAT_ENTITY_SETS_PER_DETECTOR}, got {} rows",
        rows.len()
    );

    let mut minted: Vec<(&str, Value, String)> = Vec::new();
    for (case, body) in rows {
        let set_id = created(&router, &detector_id, case, body.clone()).await;
        assert_eq!(
            set_id.len(),
            32,
            "case {case:?}: expected a 32-character minted id, got {set_id:?}"
        );
        assert!(
            minted.iter().all(|(_, _, other)| other != &set_id),
            "case {case:?}: every create mints a fresh id, got the already-minted {set_id:?}"
        );
        minted.push((case, body, set_id));
    }

    for (case, body, set_id) in &minted {
        let got = read(&router, &detector_id, set_id, case).await;

        for member in ["name", "format", "location", "expectedBucketOwner"] {
            if let Some(expected) = body.get(member) {
                assert_eq!(
                    &got[member], expected,
                    "case {case:?}: expected {member} {expected:?}, got {:?}",
                    got[member]
                );
            }
        }
        let expected_tags = body.get("tags").cloned().unwrap_or_else(|| json!({}));
        assert_eq!(
            got["tags"], expected_tags,
            "case {case:?}: expected tags {expected_tags:?}, got {:?}",
            got["tags"]
        );
        // A create that named no `expectedBucketOwner` still reports one: the member is
        // "the Amazon Web Services account ID that owns the Amazon S3 bucket specified
        // in the location parameter", and a request that names none is uploading from a
        // bucket of its own account — the account this emulator serves.
        if body.get("expectedBucketOwner").is_none() {
            assert_eq!(
                got["expectedBucketOwner"],
                json!(engine::emulator_account_id()),
                "case {case:?}: expected the owning account to be reported, got {:?}",
                got["expectedBucketOwner"]
            );
        }
        assert!(
            got["errorDetails"].is_null(),
            "case {case:?}: nothing failed, so errorDetails must be absent, got {:?}",
            got["errorDetails"]
        );

        let created_at = timestamp(&got, "createdAt", case);
        let updated_at = timestamp(&got, "updatedAt", case);
        assert!(
            created_at > 0.0 && updated_at >= created_at,
            "case {case:?}: expected createdAt > 0 and updatedAt >= createdAt, got {created_at} and {updated_at}"
        );

        // A second read reports the same set: the first one changed nothing.
        let again = read(&router, &detector_id, set_id, case).await;
        assert_eq!(
            again, got,
            "case {case:?}: a read must not change what a later read sees, got {again:?} after {got:?}"
        );
    }
}

/// The epoch-seconds timestamp `member` carries.
fn timestamp(got: &Value, member: &str, case: &str) -> f64 {
    got[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {got:?}"))
}

// ---------------------------------------------------------------------------
// Rule 2: the status moves only when the clock owes it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_activated_set_reaches_active_only_when_the_clock_owes_it() {
    // Activated and not activated, over two formats, so the rule is not read off one
    // row.
    for (case, activate, format, settled) in [
        ("activated-txt", true, FORMATS[0], ACTIVE),
        ("activated-stix", true, FORMATS[1], ACTIVE),
        ("not-activated-txt", false, FORMATS[0], INACTIVE),
        ("not-activated-fire-eye", false, FORMATS[5], INACTIVE),
    ] {
        let (router, detector_id) = detector().await;
        let set_id = created(
            &router,
            &detector_id,
            case,
            json!({
                "name": "lifecycle-set",
                "format": format,
                "location": "s3://guardduty-lists/threat.txt",
                "activate": activate,
            }),
        )
        .await;

        // Straight after the create the transient status is still observable.
        let fresh = read(&router, &detector_id, &set_id, case).await;
        let expected_fresh = if activate { ACTIVATING } else { INACTIVE };
        assert_eq!(
            fresh["status"], expected_fresh,
            "case {case:?}: expected status {expected_fresh} straight after the create, got {:?}",
            fresh["status"]
        );

        tokio::time::sleep(past_the_transition()).await;

        let later = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            later["status"], settled,
            "case {case:?}: expected status {settled} once the clock owed it, got {:?}",
            later["status"]
        );
        // Settled once and for all: the next read reports the same status AND the
        // same `UpdatedAt`, so the instant is the one the change was owed at rather
        // than the instant somebody looked.
        let again = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            again, later,
            "case {case:?}: a settled set must not move again, got {again:?} after {later:?}"
        );
        if activate {
            let created_at = timestamp(&later, "createdAt", case);
            let updated_at = timestamp(&later, "updatedAt", case);
            assert!(
                updated_at > created_at,
                "case {case:?}: the activation changed the set, so updatedAt must be later than createdAt, got {updated_at} and {created_at}"
            );
        } else {
            assert_eq!(
                timestamp(&later, "updatedAt", case),
                timestamp(&later, "createdAt", case),
                "case {case:?}: nothing changed an inactive set, so updatedAt must still equal createdAt in {later:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 3: idempotency, and the per-detector ceiling.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_replayed_client_token_returns_the_set_the_first_create_minted() {
    for (case, token) in [
        ("short-token", "t"),
        ("uuid-shaped-token", "6f1b4c7a-9d2e-4f31-8a55-0b1c2d3e4f50"),
        ("longest-token", &"9".repeat(64)),
    ] {
        let (router, detector_id) = detector().await;
        let body = json!({
            "name": "idempotent-set",
            "format": FORMATS[0],
            "location": "s3://guardduty-lists/threat.txt",
            "activate": true,
            "clientToken": token,
        });
        let first = created(&router, &detector_id, case, body.clone()).await;
        let replayed = created(&router, &detector_id, case, body).await;
        assert_eq!(
            first, replayed,
            "case {case:?}: a replayed clientToken must return the first set, got {replayed} after {first}"
        );

        // A DIFFERENT token is a second create, and a detector may hold several
        // threat entity sets: it mints a set of its own and leaves the first alone.
        let second = created(
            &router,
            &detector_id,
            case,
            json!({
                "name": "second-set",
                "format": FORMATS[1],
                "location": "s3://guardduty-lists/other.txt",
                "activate": false,
                "clientToken": "a-second-token",
            }),
        )
        .await;
        assert_ne!(
            second, first,
            "case {case:?}: a create under a fresh token must mint a new set, got {second} again"
        );
        let still = read(&router, &detector_id, &first, case).await;
        assert_eq!(
            still["name"], "idempotent-set",
            "case {case:?}: the second create must not touch the first set, got {still:?}"
        );
        let other = read(&router, &detector_id, &second, case).await;
        assert_eq!(
            other["name"], "second-set",
            "case {case:?}: the second set reports its own members, got {other:?}"
        );
    }
}

#[tokio::test]
async fn a_create_past_the_per_detector_limit_is_refused() {
    let (router, detector_id) = detector().await;
    let mut ids = Vec::new();
    for index in 0..THREAT_ENTITY_SETS_PER_DETECTOR {
        let case = format!("set-{index}");
        ids.push(
            created(
                &router,
                &detector_id,
                &case,
                json!({
                    "name": format!("limit_set_{index}"),
                    // Each row varies the format, so filling the quota is not one
                    // input repeated.
                    "format": FORMATS[index % FORMATS.len()],
                    "location": format!("s3://guardduty-lists/threat-{index}.txt"),
                    "activate": index % 2 == 0,
                }),
            )
            .await,
        );
    }

    let (status, got) = create_set(
        &router,
        &detector_id,
        json!({
            "name": "one_too_many",
            "format": FORMATS[0],
            "location": "s3://guardduty-lists/threat-extra.txt",
            "activate": false,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"one past the limit\": expected 400, got {status} {got:?}"
    );
    assert_eq!(
        error_code(&got),
        INVALID_INPUT,
        "case \"one past the limit\": expected {INVALID_INPUT}, got {got:?}"
    );

    // The refusal persisted nothing: every set the quota holds is still readable and
    // still reports its own name.
    for (index, set_id) in ids.iter().enumerate() {
        let case = format!("set-{index}");
        let got = read(&router, &detector_id, set_id, &case).await;
        assert_eq!(
            got["name"],
            format!("limit_set_{index}"),
            "case {case:?}: the refused create must not touch a stored set, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the location must be a valid Amazon S3 URL.
// ---------------------------------------------------------------------------

/// The S3 object URL forms a location may take: the `s3://` scheme, the path-style
/// endpoint (Region-less, dotted and dashed regional spellings), the
/// virtual-hosted-style one, and an object URL on a non-public S3 endpoint — the host
/// and port of the endpoint are environment-specific and no part of the rule, and so
/// is the scheme the endpoint is reached over. Each names a bucket and an object key.
const VALID_LOCATIONS: [&str; 9] = [
    "s3://guardduty-lists/threat.txt",
    "s3://guardduty.lists/deep/nested/threat.txt",
    "https://s3.amazonaws.com/guardduty-lists/threat.txt",
    "https://s3.us-east-2.amazonaws.com/guardduty-lists/threat.txt",
    "https://s3-eu-central-1.amazonaws.com/guardduty-lists/threat.txt",
    "https://guardduty-lists.s3.amazonaws.com/threat.txt",
    "https://s3.internal.example:4566/guardduty-lists/threat.txt",
    "http://127.0.0.1:9000/guardduty-lists/threat.txt",
    "http://s3.amazonaws.com/guardduty-lists/threat.txt",
];

/// Locations that are no S3 object URL at all: a scheme no S3 object is addressed by,
/// a URL naming a bucket and no object key, and a bare path with no scheme.
const INVALID_LOCATIONS: [&str; 5] = [
    "ftp://guardduty-lists/threat.txt",
    "s3://guardduty-lists",
    "https://s3.amazonaws.com/guardduty-lists",
    "https://127.0.0.1:9000/guardduty-lists",
    "/guardduty-lists/threat.txt",
];

#[tokio::test]
async fn a_location_is_accepted_exactly_when_it_is_a_valid_s3_url() {
    for location in VALID_LOCATIONS {
        let (router, detector_id) = detector().await;
        let set_id = created(
            &router,
            &detector_id,
            location,
            json!({
                "name": "location-set",
                "format": FORMATS[0],
                "location": location,
                "activate": true,
            }),
        )
        .await;
        let got = read(&router, &detector_id, &set_id, location).await;
        assert_eq!(
            got["location"], location,
            "case {location:?}: expected the location stored verbatim, got {:?}",
            got["location"]
        );
    }

    // "Invalid URL formats will result in an error, regardless of whether you
    // activate the entity set or not", so both values of `Activate` are refused.
    for location in INVALID_LOCATIONS {
        for activate in [true, false] {
            let (router, detector_id) = detector().await;
            let case = format!("{location} activate={activate}");
            let (status, got) = create_set(
                &router,
                &detector_id,
                json!({
                    "name": "location-set",
                    "format": FORMATS[0],
                    "location": location,
                    "activate": activate,
                }),
            )
            .await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected 400 for an invalid S3 URL, got {status} {got:?}"
            );
            assert_eq!(
                error_code(&got),
                INVALID_INPUT,
                "case {case:?}: expected {INVALID_INPUT}, got {got:?}"
            );
            // Nothing was persisted: a create with a valid location still succeeds,
            // and the refused one left no set behind to read.
            let set_id = created(
                &router,
                &detector_id,
                &case,
                json!({
                    "name": "location-set",
                    "format": FORMATS[0],
                    "location": VALID_LOCATIONS[0],
                    "activate": activate,
                }),
            )
            .await;
            let got = read(&router, &detector_id, &set_id, &case).await;
            assert_eq!(
                got["location"], VALID_LOCATIONS[0],
                "case {case:?}: the refused create must have persisted nothing, got {got:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the name's character classes.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_name_is_accepted_exactly_when_it_holds_the_documented_characters() {
    // "The name of your list can include lowercase letters, uppercase letters,
    // numbers, dash (-), and underscore (_)".
    for name in ["lower", "UPPER", "0123456789", "a-b_C-9", "_", "-"] {
        let (router, detector_id) = detector().await;
        let set_id = created(
            &router,
            &detector_id,
            name,
            json!({
                "name": name,
                "format": FORMATS[0],
                "location": VALID_LOCATIONS[0],
                "activate": false,
            }),
        )
        .await;
        let got = read(&router, &detector_id, &set_id, name).await;
        assert_eq!(
            got["name"], name,
            "case {name:?}: expected the name stored verbatim, got {:?}",
            got["name"]
        );
    }

    for name in ["has space", "dots.in.name", "slash/name", "plus+name", "naïve"] {
        let (router, detector_id) = detector().await;
        let (status, got) = create_set(
            &router,
            &detector_id,
            json!({
                "name": name,
                "format": FORMATS[0],
                "location": VALID_LOCATIONS[0],
                "activate": false,
            }),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected 400 for a name outside the documented characters, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            INVALID_INPUT,
            "case {name:?}: expected {INVALID_INPUT}, got {got:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 6: an id the store does not hold, and the two entity-set namespaces.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_id_the_store_does_not_hold_is_refused_rather_than_answered() {
    let (router, detector_id) = detector().await;
    let set_id = created(
        &router,
        &detector_id,
        "rejections",
        json!({
            "name": "rejection-set",
            "format": FORMATS[0],
            "location": VALID_LOCATIONS[0],
            "activate": true,
        }),
    )
    .await;

    // Each row is an id no store holds: a well-formed detector id, a value that
    // cannot be one, a well-formed set id and a value that cannot be one. The
    // detector rows are refused with the detector family's own refusal and the set
    // rows with the entity-set families' absence refusal, because they name
    // different resources.
    for (case, detector) in [
        ("absent detector", ABSENT_DETECTOR_ID.to_string()),
        ("malformed detector", "not-a-detector".to_string()),
    ] {
        let (status, got) = get_set(&router, &detector, &set_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected 400, got {status} {got:?}"
        );
        assert_eq!(
            error_code(&got),
            INVALID_INPUT,
            "case {case:?}: expected {INVALID_INPUT}, got {got:?}"
        );
        assert!(
            got.get("status").is_none(),
            "case {case:?}: a refusal must report no entity set, got {got:?}"
        );
    }
    // A well-formed set id the detector does not hold is the entity-set families'
    // absence refusal; one outside the alphabet the member is constrained to is the
    // constraint violation, decided from the request before anything is resolved.
    for (case, set, well_formed) in [
        ("absent set", ABSENT_SET_ID, true),
        ("malformed set", "not-a-set", false),
    ] {
        let (status, got) = get_set(&router, &detector_id, set).await;
        if well_formed {
            assert_entity_set_absent(case, status, &got, set);
        } else {
            assert_entity_set_id_constraint_violation(
                case,
                status,
                &got,
                THREAT_ENTITY_SET_ID_MEMBER,
                set,
            );
        }
        assert!(
            got.get("status").is_none(),
            "case {case:?}: a refusal must report no entity set, got {got:?}"
        );
    }

    // A create under a detector the store does not hold is refused too, and the
    // detector that does exist still holds exactly its own set.
    let (status, got) = create_set(
        &router,
        ABSENT_DETECTOR_ID,
        json!({
            "name": "other-set",
            "format": FORMATS[0],
            "location": VALID_LOCATIONS[0],
            "activate": true,
        }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"create under an absent detector\": expected 400, got {status} {got:?}"
    );
    let still = read(&router, &detector_id, &set_id, "rejections").await;
    assert_eq!(
        still["name"], "rejection-set",
        "the refused create must not touch another detector's set, got {still:?}"
    );
}

#[tokio::test]
async fn a_threat_entity_set_and_a_trusted_entity_set_are_separate_resources() {
    let (router, detector_id) = detector().await;
    // One token, replayed across the two different creates: each resolves to the
    // resource its own create made.
    let token = "shared-idempotency-token";
    let threat_id = created(
        &router,
        &detector_id,
        "threat",
        json!({
            "name": "threat_set",
            "format": FORMATS[0],
            "location": VALID_LOCATIONS[0],
            "activate": true,
            "clientToken": token,
        }),
    )
    .await;
    let (status, got) = call(
        &router,
        "POST",
        &format!("/detector/{detector_id}/trustedentityset"),
        Some(json!({
            "name": "trusted_set",
            "format": FORMATS[0],
            "location": VALID_LOCATIONS[0],
            "activate": true,
            "clientToken": token,
        })),
    )
    .await;
    assert_eq!(
        status, 200,
        "case \"trusted create\": expected 200, got {status} {got:?}"
    );
    let trusted_id = got["trustedEntitySetId"]
        .as_str()
        .unwrap_or_else(|| panic!("case \"trusted create\": expected an id, got {got:?}"))
        .to_string();
    assert_ne!(
        trusted_id, threat_id,
        "a token replayed across the two creates must not hand back the other resource, got {trusted_id}"
    );

    // Neither id resolves through the other's path: each family holds its own ids,
    // so the other's is an id that family does not hold — its absence refusal.
    let (status, got) = get_set(&router, &detector_id, &trusted_id).await;
    assert_entity_set_absent(
        "trusted id on the threat path",
        status,
        &got,
        &trusted_id,
    );
    let (status, got) = call(
        &router,
        "GET",
        &format!("/detector/{detector_id}/trustedentityset/{threat_id}"),
        None,
    )
    .await;
    assert_entity_set_absent("threat id on the trusted path", status, &got, &threat_id);

    // Each read on its own path still reports its own set.
    let threat = read(&router, &detector_id, &threat_id, "threat").await;
    assert_eq!(
        threat["name"], "threat_set",
        "the threat set reports its own name, got {threat:?}"
    );
}
