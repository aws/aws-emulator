// SPDX-License-Identifier: CC0-1.0

//! `UpdateThreatEntitySet`, end to end over the wire, read back through
//! `GetThreatEntitySet`.
//!
//! `threat_entity_set_lifecycle.rs` owns the create-and-read half of the
//! threat-entity-set lifecycle, `threat_entity_set_listing.rs` the list and
//! `threat_entity_set_delete.rs` the delete; this suite owns the UPDATE, which
//! answered HTTP 501 `NotImplemented` and so left the `create -> update -> read`
//! half of that state machine unverifiable. Every request goes through
//! [`smithy_server_runtime::MultiServiceRouter::dispatch`] (the harness lives in the
//! `detector_lifecycle` suite), so routing, restJson1 (de)serialization,
//! modelled-constraint validation and the handlers are exercised together.
//!
//! The rules under test, each asserted over a SET of inputs rather than one call:
//!
//! 1. `UpdateThreatEntitySet` is a PARTIAL update: every member but the two ids is
//!    optional (`smithy_query(operation="UpdateThreatEntitySet")`), so a member the
//!    request carries replaces the stored one and a member it omits leaves the
//!    stored one exactly where it was. `Format` and `Tags` are not members of this
//!    request, so neither ever changes. The update is PERSISTED: successive updates
//!    accumulate and the last write is what a read reports.
//! 2. `Activate` on an update starts the same transition `CreateThreatEntitySet`
//!    starts: the status becomes the transitional one and only the CLOCK settles it
//!    (`state::ENTITY_SET_TRANSITION_MILLIS`); asking for the status the set already
//!    has moves nothing, and an update that omits `Activate` leaves the status
//!    alone.
//! 3. The documented value rules of an entity set's members hold whichever call
//!    writes them, so an update carrying a `Name` outside the documented alphabet or
//!    a `Location` that is not a valid Amazon S3 URL is refused with HTTP 400 under
//!    the `InvalidInputException` class, naming the MEMBER, and writes nothing.
//! 4. An id the store does not resolve is refused with the entity-set families'
//!    absence refusal — HTTP 404 under the `ResourceNotFoundException` class, with
//!    one fixed sentence that names neither the member nor the caller's value — and
//!    updates nothing. That covers the id of a set a delete has taken out of the
//!    detector's holdings, and, a threat entity set being a sub-resource of a
//!    REGIONAL detector, an id only the Region that created it may update (where the
//!    detector is what the request fails to resolve, so the refusal is the detector
//!    family's).
//! 5. An update touches only the set its id names: a SIBLING threat entity set under
//!    the same detector, the detector's trusted entity set and another detector's
//!    sets are untouched, and the detector's own `UpdatedAt` does not move, because
//!    the set of sub-resources it holds is unchanged.

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    call, call_in, enabled_detector, error_code, new_router, Router, ABSENT_DETECTOR_ID,
};
use crate::entity_set_id_refusal_classes::assert_entity_set_id_constraint_violation;
use crate::set_delete_terminality::assert_entity_set_absent;
use service_com_amazonaws_guardduty_guarddutyapiservice::state::{
    DETECTOR_ID_MEMBER, ENTITY_SET_ACTIVATING, ENTITY_SET_ACTIVE, ENTITY_SET_DEACTIVATING,
    ENTITY_SET_INACTIVE, ENTITY_SET_TRANSITION_MILLIS, THREAT_ENTITY_SET_ID_MEMBER,
};

/// The error class every client rejection on this service carries in the body's
/// `__type` discriminator.
const INVALID_INPUT: &str = "InvalidInputException";

/// A well-formed entity-set id (32 lowercase hex characters, the form this service
/// mints) that no create ever minted.
const ABSENT_SET_ID: &str = "0123456789abcdef0123456789abcdef";

/// A valid Amazon S3 location, the one the creates below start from.
const LOCATION: &str = "s3://guardduty-lists/threat.txt";

/// The wire names of the two value members a refusal may name
/// (`smithy_query(operation="UpdateThreatEntitySet")` renders the `jsonName`s
/// `name` and `location`).
const NAME_MEMBER: &str = "name";
const LOCATION_MEMBER: &str = "location";

/// Long enough for the clock to owe the next status, plus slack.
fn past_the_transition() -> std::time::Duration {
    let span = u64::try_from(ENTITY_SET_TRANSITION_MILLIS).unwrap_or_default();
    std::time::Duration::from_millis(span + 250)
}

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

async fn update_set(router: &Router, detector_id: &str, set_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
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

async fn delete_set(router: &Router, detector_id: &str, set_id: &str) -> (u16, Value) {
    call(
        router,
        "DELETE",
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

/// Update a set, failing the test if the call did not succeed.
async fn updated(router: &Router, detector_id: &str, set_id: &str, case: &str, body: Value) {
    let (status, got) = update_set(router, detector_id, set_id, body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected update to answer the modelled 200, got {status} {got:?}"
    );
    assert!(
        got.get("status").is_none(),
        "case {case:?}: the update's output is Unit, so it reports no entity set, got {got:?}"
    );
}

/// The `Status` a read reports.
async fn status_of(router: &Router, detector_id: &str, set_id: &str, case: &str) -> String {
    read(router, detector_id, set_id, case).await["status"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// The epoch-seconds timestamp `member` carries.
fn timestamp(got: &Value, member: &str, case: &str) -> f64 {
    got[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} timestamp, got {got:?}"))
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
// Rule 1: an update replaces what it carries and leaves the rest alone.
// ---------------------------------------------------------------------------

/// The set every row of the partial-update table starts from. `activate` is false so
/// the status is settled and nothing in rule 1 depends on the clock. `format` varies
/// by row, to show that whichever of the modelled formats a set was created with, the
/// update neither reads nor changes it.
fn base_create(format: &str, with_owner: bool) -> Value {
    let mut body = json!({
        "name": "base_set-1",
        "format": format,
        "location": LOCATION,
        "activate": false,
        "tags": {"Env": "test", "Team": "security"},
    });
    if with_owner {
        body["expectedBucketOwner"] = json!("210987654321");
    }
    body
}

/// One partial-update case: the format the create stored, whether it recorded an
/// `ExpectedBucketOwner`, and the update body.
struct UpdateRow {
    case: &'static str,
    /// A member of `ThreatEntitySetFormat`
    /// (`smithy_query(query="shape",shape="ThreatEntitySetFormat")` lists TXT, STIX,
    /// OTX_CSV, ALIEN_VAULT, PROOF_POINT and FIRE_EYE).
    format: &'static str,
    with_owner: bool,
    update: Value,
}

/// The rows differ along what the rule generalises over: WHICH optional members the
/// request carries (each alone, several together, none at all), the SIZE of the
/// value (both ends of `Name`'s modelled `len 1..300`), the character classes the
/// documented `Name` alphabet admits, the S3 URL FORM of the location, whether the
/// member already had a stored value, and the FORMAT the set was created with.
fn partial_update_rows() -> Vec<UpdateRow> {
    // `Name` is `len 1..300` (`smithy_query(operation="UpdateThreatEntitySet")`),
    // so this is the longest value the model accepts.
    let longest_name = "n".repeat(300);
    vec![
        UpdateRow {
            case: "name alone",
            format: "TXT",
            with_owner: false,
            update: json!({"name": "renamed_set"}),
        },
        UpdateRow {
            case: "the shortest name the model accepts",
            format: "STIX",
            with_owner: false,
            update: json!({"name": "a"}),
        },
        UpdateRow {
            case: "the longest name the model accepts",
            format: "OTX_CSV",
            with_owner: true,
            update: json!({"name": longest_name}),
        },
        UpdateRow {
            case: "every character class the documented alphabet admits",
            // A format the issue did not cite, to show the update is indifferent to it.
            format: "ALIEN_VAULT",
            with_owner: false,
            update: json!({"name": "Upper-lower_0123456789"}),
        },
        UpdateRow {
            case: "location alone, moved to a path-style S3 URL",
            format: "PROOF_POINT",
            with_owner: false,
            update: json!({"location": "https://s3.eu-west-1.amazonaws.com/other-lists/moved.txt"}),
        },
        UpdateRow {
            case: "location alone, moved to a virtual-hosted-style S3 URL",
            format: "FIRE_EYE",
            with_owner: true,
            update: json!({"location": "https://other-lists.s3.amazonaws.com/deep/moved.txt"}),
        },
        UpdateRow {
            case: "expectedBucketOwner recorded by the update, not by the create",
            format: "TXT",
            with_owner: false,
            update: json!({"expectedBucketOwner": "123456789012"}),
        },
        UpdateRow {
            case: "expectedBucketOwner replaced, having been set by the create",
            format: "STIX",
            with_owner: true,
            update: json!({"expectedBucketOwner": "123456789012"}),
        },
        UpdateRow {
            case: "every updatable value member at once",
            format: "TXT",
            with_owner: true,
            update: json!({
                "name": "all-three",
                "location": "s3://other-lists/all-three.txt",
                "expectedBucketOwner": "000000000000",
            }),
        },
        UpdateRow {
            case: "an update that carries no updatable member at all",
            format: "FIRE_EYE",
            with_owner: true,
            update: json!({}),
        },
    ]
}

#[tokio::test]
async fn an_update_replaces_only_the_members_it_carries() {
    for row in partial_update_rows() {
        let case = row.case;
        let (router, detector_id) = detector().await;
        let create = base_create(row.format, row.with_owner);
        let set_id = created(&router, &detector_id, case, create.clone()).await;
        let before = read(&router, &detector_id, &set_id, case).await;

        updated(&router, &detector_id, &set_id, case, row.update.clone()).await;
        let got = read(&router, &detector_id, &set_id, case).await;

        // A member the update carried reports the updated value; one it omitted
        // reports what the create stored.
        for member in ["name", "location", "expectedBucketOwner"] {
            let expected = row.update.get(member).or_else(|| create.get(member)).cloned();
            match expected {
                Some(expected) => assert_eq!(
                    got[member], expected,
                    "case {case:?}: expected {member} {expected:?}, got {:?}",
                    got[member]
                ),
                // An `ExpectedBucketOwner` neither the create nor the update carried is
                // still reported: the set knows which account owns the bucket its
                // location names, and with no request naming one that is the account
                // this emulator serves.
                None => assert_eq!(
                    got[member],
                    json!(engine::emulator_account_id()),
                    "case {case:?}: expected {member} to report the owning account when \
                     neither the create nor the update carried it, got {:?}",
                    got[member]
                ),
            }
        }

        // `Format` is not a member of the update request at all, and neither is
        // `Tags`, so neither can have moved.
        assert_eq!(
            got["format"], create["format"],
            "case {case:?}: an update must not change the format, got {:?}",
            got["format"]
        );
        assert_eq!(
            got["tags"], create["tags"],
            "case {case:?}: an update must not change the tags, got {:?}",
            got["tags"]
        );
        // The update carried no `activate`, so the settled status stays put.
        let status = got["status"].as_str().unwrap_or_default();
        assert_eq!(
            status, ENTITY_SET_INACTIVE,
            "case {case:?}: an update without activate must leave the status alone, got \
             {status:?}"
        );
        // `CreatedAt` records the create and nothing else moves it; `UpdatedAt`
        // moves exactly when this call changed the set.
        assert_eq!(
            timestamp(&got, "createdAt", case),
            timestamp(&before, "createdAt", case),
            "case {case:?}: an update must not move createdAt, got {got:?}"
        );
        let changed_something = ["name", "location", "expectedBucketOwner"]
            .iter()
            .any(|member| {
                row.update
                    .get(member)
                    .is_some_and(|value| before.get(member) != Some(value))
            });
        let updated_at = timestamp(&got, "updatedAt", case);
        let was = timestamp(&before, "updatedAt", case);
        if changed_something {
            assert!(
                updated_at > was,
                "case {case:?}: the update changed the set, so updatedAt must move, got \
                 {updated_at} after {was}"
            );
        } else {
            assert_eq!(
                updated_at, was,
                "case {case:?}: the update changed nothing, so updatedAt must stay put, got \
                 {updated_at} after {was}"
            );
        }

        // A read never changes what a later read sees.
        let again = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            again, got,
            "case {case:?}: a read must not change what a later read sees, got {again:?} after \
             {got:?}"
        );
    }
}

/// An update is PERSISTED, not merely answered: a second update sees what the first
/// one wrote, and the last write is what a read reports.
#[tokio::test]
async fn successive_updates_accumulate_on_the_stored_set() {
    let (router, detector_id) = detector().await;
    let set_id = created(
        &router,
        &detector_id,
        "the accumulating set",
        base_create("TXT", false),
    )
    .await;

    // Each step carries ONE member, so what a read reports afterwards can only come
    // from the steps before it.
    let steps = [
        (
            "first: name",
            json!({"name": "step_one"}),
            "step_one",
            LOCATION,
        ),
        (
            "second: location, name untouched",
            json!({"location": "s3://guardduty-lists/step-two.txt"}),
            "step_one",
            "s3://guardduty-lists/step-two.txt",
        ),
        (
            "third: name again, location untouched",
            json!({"name": "step_three"}),
            "step_three",
            "s3://guardduty-lists/step-two.txt",
        ),
    ];

    for (case, update, expected_name, expected_location) in steps {
        updated(&router, &detector_id, &set_id, case, update).await;
        let got = read(&router, &detector_id, &set_id, case).await;
        let name = got["name"].as_str().unwrap_or_default();
        let location = got["location"].as_str().unwrap_or_default();
        assert_eq!(
            name, expected_name,
            "case {case:?}: expected the name {expected_name:?}, got {name:?}"
        );
        assert_eq!(
            location, expected_location,
            "case {case:?}: expected the location {expected_location:?}, got {location:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: Activate starts a transition only the clock settles.
// ---------------------------------------------------------------------------

/// One activation case: the status the set holds when the update arrives, what the
/// update asks for, and the transitional and settled statuses that follow.
struct ActivationRow {
    case: &'static str,
    /// Whether the create asked to activate the set.
    created_active: bool,
    /// Wait past the create's transition, so the update meets a SETTLED status.
    settle_first: bool,
    activate: bool,
    transitional: &'static str,
    settled: &'static str,
}

fn activation_rows() -> Vec<ActivationRow> {
    vec![
        ActivationRow {
            case: "an INACTIVE set asked to activate",
            created_active: false,
            settle_first: false,
            activate: true,
            transitional: ENTITY_SET_ACTIVATING,
            settled: ENTITY_SET_ACTIVE,
        },
        ActivationRow {
            case: "an ACTIVE set asked to deactivate",
            created_active: true,
            settle_first: true,
            activate: false,
            transitional: ENTITY_SET_DEACTIVATING,
            settled: ENTITY_SET_INACTIVE,
        },
        ActivationRow {
            case: "an ACTIVATING set asked to deactivate mid-transition",
            created_active: true,
            settle_first: false,
            activate: false,
            transitional: ENTITY_SET_DEACTIVATING,
            settled: ENTITY_SET_INACTIVE,
        },
        ActivationRow {
            case: "an INACTIVE set asked to activate once the create settled",
            created_active: false,
            settle_first: true,
            activate: true,
            transitional: ENTITY_SET_ACTIVATING,
            settled: ENTITY_SET_ACTIVE,
        },
    ]
}

#[tokio::test]
async fn activate_on_an_update_starts_a_transition_only_the_clock_settles() {
    for row in activation_rows() {
        let case = row.case;
        let (router, detector_id) = detector().await;
        let set_id = created(
            &router,
            &detector_id,
            case,
            json!({
                "name": "activation_set",
                "format": "TXT",
                "location": LOCATION,
                "activate": row.created_active,
            }),
        )
        .await;
        if row.settle_first {
            tokio::time::sleep(past_the_transition()).await;
        }

        updated(
            &router,
            &detector_id,
            &set_id,
            case,
            json!({"activate": row.activate}),
        )
        .await;

        // The span is never zero, so a caller reading straight after the update
        // still observes the transitional status.
        let transitional = status_of(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            transitional, row.transitional,
            "case {case:?}: expected {} straight after the update, got {transitional:?}",
            row.transitional
        );

        tokio::time::sleep(past_the_transition()).await;

        let settled = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            settled["status"], row.settled,
            "case {case:?}: expected {} once the clock owed it, got {:?}",
            row.settled, settled["status"]
        );

        // Asking for the status the set has already settled on owes nothing, so a
        // repeated update must not re-open the transition.
        updated(
            &router,
            &detector_id,
            &set_id,
            case,
            json!({"activate": row.activate}),
        )
        .await;
        let again = read(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            again, settled,
            "case {case:?}: asking for the status the set already has must move nothing, got \
             {again:?} after {settled:?}"
        );

        // An update that omits `activate` leaves the status alone, whatever else it
        // carries.
        updated(
            &router,
            &detector_id,
            &set_id,
            case,
            json!({"name": "renamed_after_activation"}),
        )
        .await;
        let kept = status_of(&router, &detector_id, &set_id, case).await;
        assert_eq!(
            kept, row.settled,
            "case {case:?}: an update without activate must leave the status alone, got {kept:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: the documented value rules hold on an update too.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_value_outside_the_documented_rules_is_refused_and_writes_nothing() {
    // "The name of your list can include lowercase letters, uppercase letters,
    // numbers, dash (-), and underscore (_)"
    // (`smithy_query(query="doc",shape="UpdateThreatEntitySetRequest",member="Name")`),
    // so each of these carries a character outside that alphabet.
    let bad_names = ["has space", "dots.in.name", "slash/name", "plus+name", "naïve"];
    // "The format of the Location URL must be a valid Amazon S3 URL format": a
    // scheme no S3 object is addressed by, a URL naming a bucket and no object key,
    // a URL whose key is empty and a bare path with no scheme.
    let bad_locations = [
        "ftp://guardduty-lists/threat.txt",
        "s3://guardduty-lists",
        "s3://guardduty-lists/",
        "https://s3.amazonaws.com/guardduty-lists",
        "https://127.0.0.1:9000/guardduty-lists",
        "/guardduty-lists/threat.txt",
    ];

    let mut rows: Vec<(String, Value, &str)> = Vec::new();
    for name in bad_names {
        rows.push((format!("name {name:?}"), json!({"name": name}), NAME_MEMBER));
    }
    for location in bad_locations {
        rows.push((
            format!("location {location:?}"),
            json!({"location": location}),
            LOCATION_MEMBER,
        ));
    }
    // The rule does not depend on `Activate`, and it refuses the whole request: a
    // well-formed member travelling with a refused one is not written either.
    rows.push((
        "a bad location beside a good name, activate=true".to_string(),
        json!({"name": "would_be_fine", "location": "ftp://guardduty-lists/x.txt", "activate": true}),
        LOCATION_MEMBER,
    ));
    rows.push((
        "a bad name beside a good location, activate=false".to_string(),
        json!({"name": "not ok", "location": "s3://other-lists/fine.txt", "activate": false}),
        NAME_MEMBER,
    ));

    for (case, update, member) in rows {
        let (router, detector_id) = detector().await;
        let set_id = created(&router, &detector_id, &case, base_create("TXT", false)).await;
        let before = read(&router, &detector_id, &set_id, &case).await;

        let (status, body) = update_set(&router, &detector_id, &set_id, update).await;
        assert_classified_refusal(&case, status, &body, member, &[]);

        // The refusal wrote nothing: not the member at fault, not the one beside it,
        // and not the status either.
        let after = read(&router, &detector_id, &set_id, &case).await;
        assert_eq!(
            after, before,
            "case {case:?}: a refused update must leave the stored set exactly as it was, got \
             {after:?} after {before:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: an id the store does not resolve.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_unresolvable_id_is_refused_rather_than_answered() {
    let case = "rejections";
    let (router, detector_id) = detector().await;
    let set_id = created(&router, &detector_id, case, base_create("TXT", false)).await;

    // Each row is an id no store resolves: a well-formed detector id and a value
    // that cannot be one, then a well-formed set id and a value that cannot be one.
    // The set rows are refused with the entity-set families' absence refusal and the
    // detector rows with the detector family's, because they name different
    // resources.
    let detector_rows: Vec<(&str, String)> = vec![
        ("absent detector", ABSENT_DETECTOR_ID.to_string()),
        ("malformed detector", "not-a-detector".to_string()),
    ];
    for (row_case, detector_arg) in detector_rows {
        let (status, body) = update_set(
            &router,
            &detector_arg,
            &set_id,
            json!({"name": "unreachable"}),
        )
        .await;
        assert_classified_refusal(
            row_case,
            status,
            &body,
            DETECTOR_ID_MEMBER,
            &[detector_arg.as_str(), set_id.as_str()],
        );
        // The refusal updated nothing: the set that does exist still reports what
        // its create stored.
        let got = read(&router, &detector_id, &set_id, row_case).await;
        assert_eq!(
            got["name"], "base_set-1",
            "case {row_case:?}: a refused update must not touch the stored set, got {got:?}"
        );
    }
    // A well-formed set id the detector does not hold is the entity-set families'
    // absence refusal; one outside the alphabet the member is constrained to is the
    // constraint violation, decided from the request before anything is resolved.
    for (row_case, set_arg, well_formed) in [
        ("absent set", ABSENT_SET_ID, true),
        ("malformed set", "not-a-set", false),
    ] {
        let (status, body) = update_set(
            &router,
            &detector_id,
            set_arg,
            json!({"name": "unreachable"}),
        )
        .await;
        if well_formed {
            assert_entity_set_absent(row_case, status, &body, set_arg);
        } else {
            assert_entity_set_id_constraint_violation(
                row_case,
                status,
                &body,
                THREAT_ENTITY_SET_ID_MEMBER,
                set_arg,
            );
        }
        let got = read(&router, &detector_id, &set_id, row_case).await;
        assert_eq!(
            got["name"], "base_set-1",
            "case {row_case:?}: a refused update must not touch the stored set, got {got:?}"
        );
    }

    // A threat entity set is a sub-resource of a REGIONAL detector, so a request
    // signed for another Region resolves neither the detector nor the set.
    let (status, body) = call_in(
        &router,
        "ap-southeast-2",
        "POST",
        &format!("/detector/{detector_id}/threatentityset/{set_id}"),
        Some(json!({"name": "from_another_region"})),
    )
    .await;
    assert_classified_refusal(
        "another Region",
        status,
        &body,
        DETECTOR_ID_MEMBER,
        &[detector_id.as_str(), set_id.as_str()],
    );
    let survived = read(&router, &detector_id, &set_id, "another Region").await;
    assert_eq!(
        survived["name"], "base_set-1",
        "case \"another Region\": the set must be untouched, got {survived:?}"
    );
}

/// A set a delete has taken out of the detector's holdings is no longer there, so
/// its id is refused exactly like one the detector never held — an update must not
/// walk a deleted set back into use. Nothing addresses the id afterwards, so the
/// refusal is checked against the LISTING and a re-read rather than against a record.
#[tokio::test]
async fn a_set_whose_delete_was_accepted_cannot_be_updated() {
    for (case, wait) in [
        ("straight after the delete", false),
        ("once every transition would have come due", true),
    ] {
        let (router, detector_id) = detector().await;
        let set_id = created(&router, &detector_id, case, base_create("TXT", false)).await;
        let (status, body) = delete_set(&router, &detector_id, &set_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the delete to answer 200, got {status} {body:?}"
        );
        if wait {
            tokio::time::sleep(past_the_transition()).await;
        }

        let (status, body) = update_set(
            &router,
            &detector_id,
            &set_id,
            json!({"name": "revived", "activate": true}),
        )
        .await;
        assert_entity_set_absent(case, status, &body, &set_id);

        // The refused update revived nothing: the id still resolves to no set at
        // all.
        let (status, body) = get_set(&router, &detector_id, &set_id).await;
        assert_entity_set_absent(case, status, &body, &set_id);
    }
}

// ---------------------------------------------------------------------------
// Rule 5: an update touches only the set it names.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_update_touches_only_the_set_it_names() {
    let case = "sibling resources";
    let router = new_router();
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
    // A detector may hold SEVERAL threat entity sets, so a sibling in the very same
    // collection is the sharpest witness that the update reaches only the id it was
    // given.
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
    // threat update must not reach into.
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

    // The detector's own `UpdatedAt` before the update: the set of sub-resources it
    // holds does not change, so the update must not move it.
    let (status, detector_before) =
        call(&router, "GET", &format!("/detector/{first_detector}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {detector_before:?}"
    );

    updated(
        &router,
        &first_detector,
        &mine,
        case,
        json!({"name": "mine_renamed", "activate": true}),
    )
    .await;

    let got = read(&router, &first_detector, &mine, case).await;
    assert_eq!(
        got["name"], "mine_renamed",
        "case {case:?}: expected the named set to be updated, got {got:?}"
    );

    let kept = read(&router, &first_detector, &sibling, case).await;
    assert_eq!(
        kept["name"], "sibling",
        "case {case:?}: a sibling threat entity set must be untouched, got {kept:?}"
    );
    assert_eq!(
        kept["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a sibling threat entity set must keep its status, got {kept:?}"
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
        other["name"], "theirs",
        "case {case:?}: another detector's set must be untouched, got {other:?}"
    );
    assert_eq!(
        other["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: another detector's set must keep its status, got {other:?}"
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
        trusted_set["name"], "trusted-set",
        "case {case:?}: a threat update must not touch a trusted entity set, got {trusted_set:?}"
    );
    assert_eq!(
        trusted_set["status"], ENTITY_SET_INACTIVE,
        "case {case:?}: a threat update must not move a trusted entity set's status, got \
         {trusted_set:?}"
    );

    let (status, detector_after) =
        call(&router, "GET", &format!("/detector/{first_detector}"), None).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected get-detector to answer 200, got {status} {detector_after:?}"
    );
    assert_eq!(
        detector_after["updatedAt"], detector_before["updatedAt"],
        "case {case:?}: the detector holds the same sub-resources, so its updatedAt must stay \
         put, got {:?} after {:?}",
        detector_after["updatedAt"], detector_before["updatedAt"]
    );
}
