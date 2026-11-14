// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the Glue blueprint registration slice: `CreateBlueprint` and the
//! `GetBlueprint` that reads back what it stored.
//!
//! RULE: a blueprint create registers ONE record per account under the name it was sent,
//! holding the whole registration the request carried (`Name`, `BlueprintLocation`,
//! `Description`, `Tags`) plus the instants and the registration status the service sets,
//! and answers with the name it stored; every later read of that name reports that record,
//! a name already taken is refused instead of overwritten, and a name nothing registered
//! is a miss.
//!
//! Every call goes over the wire through the shared [`crate::wire`] harness, so what is
//! asserted is HTTP behaviour and not a direct call into a handler.
//!
//! The round-trip rows differ along what the rule generalises over: which optional members
//! the registration carries, the form and length of the name, and the form and length of
//! the S3 location — each published length bound, and one past it, included.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::ops::types::CreateBlueprintInput;
use service_com_amazonaws_glue_awsglue::state::{
    messages, BlueprintArchive, BlueprintEntry, ComAmazonawsGlueAwsglueState,
    BLUEPRINT_SETTLES_AFTER, MAX_BLUEPRINTS,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::far_future;
use crate::wire::{assert_echoed, call, emulator, emulator_on, error_code, error_message, ok};

/// The longest `Name` the model accepts: `OrchestrationNameString`, "len 1..128"
/// (`smithy_query operation=CreateBlueprint`).
const MAX_NAME_LENGTH: usize = 128;

/// The longest `BlueprintLocation` the model accepts: `OrchestrationS3Location`,
/// "len 1..8192" (`smithy_query operation=CreateBlueprint`).
const MAX_LOCATION_LENGTH: usize = 8192;

/// The longest `Description` the model accepts: `Generic512CharString`, "len 1..512"
/// (`smithy_query operation=CreateBlueprint`).
const MAX_DESCRIPTION_LENGTH: usize = 512;

/// The `BlueprintStatus` a registration is in until the clock owes it the next one, and the
/// one it owes: the modelled values "Creating — The blueprint registration is in progress"
/// and "Active — The blueprint has been successfully registered" (`smithy_query
/// shape=Blueprint`, member `Status`).
const STATUS_CREATING: &str = "CREATING";
const STATUS_ACTIVE: &str = "ACTIVE";

/// An S3 location of exactly `length` characters, built as the modelled pattern
/// `^s3://([^/]+)/([^/]+/)*([^/]+)$` requires: a bucket and one key segment.
fn location_of_length(length: usize) -> String {
    let prefix = "s3://blueprint-bucket/";
    format!("{prefix}{}", "k".repeat(length - prefix.len()))
}

/// Register a blueprint and return the response body.
async fn create(router: &MultiServiceRouter, body: &Value) -> Value {
    ok(router, "CreateBlueprint", body).await
}

/// The `Blueprint` the `GetBlueprint` answer for `name` reports.
async fn get(router: &MultiServiceRouter, name: &str) -> Value {
    let read = ok(router, "GetBlueprint", &json!({ "Name": name })).await;
    read.get("Blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("GetBlueprint must report a Blueprint, got {read}"))
}

/// Every blueprint name `ListBlueprints` reports.
async fn listed(router: &MultiServiceRouter) -> Vec<String> {
    let page = ok(router, "ListBlueprints", &json!({})).await;
    page["Blueprints"]
        .as_array()
        .expect("Blueprints is a list")
        .iter()
        .map(|name| name.as_str().expect("a name is a string").to_string())
        .collect()
}

/// Pull a stored registration's instants back by `elapsed`, so a suite can observe the
/// status the clock owes it without sleeping for the span.
fn rewind(engine: &Arc<engine::InMemoryEngine>, name: &str, elapsed: Duration) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(name)
        .unwrap_or_else(|| panic!("{name} was registered"));
    let back = |at: std::time::SystemTime| at.checked_sub(elapsed).expect("an instant in range");
    entry.created_on = back(entry.created_on);
    entry.last_modified_on = back(entry.last_modified_on);
    entry.settles_at = entry.settles_at.map(back);
}

/// Put the instant the clock owes a stored registration its next status at `at`.
///
/// A row that means "the clock has not reached the due instant yet" holds that instant far
/// enough ahead that the wall clock cannot cross it while the row's calls are made: the
/// status turns on the stored due instant, so positioning the instant is what fixes which
/// side of it a call lands on, and a margin measured against the clock is not.
fn set_due_instant(engine: &Arc<engine::InMemoryEngine>, name: &str, at: SystemTime) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(name)
        .unwrap_or_else(|| panic!("{name} was registered"));
    if entry.settles_at.is_some() {
        entry.settles_at = Some(at);
    }
}

// ---------------------------------------------------------------------------
// Register, then read back
// ---------------------------------------------------------------------------

/// What `CreateBlueprint` persists is what `GetBlueprint` reports: the create answers with
/// the name it stored, and every member of the registration the request carried comes back
/// from the store unchanged.
#[tokio::test]
async fn registered_blueprints_are_read_back_from_the_store() {
    let router = emulator();

    let long_name = "n".repeat(MAX_NAME_LENGTH);
    let long_location = location_of_length(MAX_LOCATION_LENGTH);
    let long_description = "d".repeat(MAX_DESCRIPTION_LENGTH);

    let cases: Vec<(&str, Value)> = vec![
        (
            // The shape a lifecycle client sends: the two required members alone.
            "required members only",
            json!({
                "Name": "etl-lifecycle",
                "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
            }),
        ),
        (
            "a description",
            json!({
                "Name": "described",
                "BlueprintLocation": "s3://blueprint-bucket/described.zip",
                "Description": "Partitioned ingest of one table.",
            }),
        ),
        (
            "tags",
            json!({
                "Name": "tagged",
                "BlueprintLocation": "s3://blueprint-bucket/tagged.zip",
                "Tags": { "team": "data", "env": "prod" },
            }),
        ),
        (
            "a location nested under several prefixes",
            json!({
                "Name": "deeply.nested-location_1",
                "BlueprintLocation": "s3://blueprint-bucket/team/data/v2/nested.zip",
            }),
        ),
        (
            // The pattern admits both cases and every one of `.`, `-`, `_`.
            "a name carrying dots, dashes, underscores and mixed case",
            json!({
                "Name": "Mixed.Case-blueprint_42",
                "BlueprintLocation": "s3://blueprint-bucket/mixed.zip",
                "Description": "Mixed-case name.",
            }),
        ),
        (
            "a name at the modelled length bound",
            json!({
                "Name": long_name,
                "BlueprintLocation": "s3://blueprint-bucket/long-name.zip",
            }),
        ),
        (
            "a location at the modelled length bound",
            json!({
                "Name": "long-location",
                "BlueprintLocation": long_location,
            }),
        ),
        (
            "a description at the modelled length bound",
            json!({
                "Name": "long-description",
                "BlueprintLocation": "s3://blueprint-bucket/long-description.zip",
                "Description": long_description,
            }),
        ),
    ];

    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");

        // "Returns the name of the blueprint that was registered": the create reports the
        // name it stored, which is what a later read addresses the blueprint by.
        let created = create(&router, request).await;
        assert_eq!(
            created.get("Name").and_then(Value::as_str),
            Some(name),
            "case {case}: CreateBlueprint must report the stored name, got {created}"
        );

        let read = get(&router, name).await;
        // Every member of the registration the request carried reappears unchanged.
        let expected = json!({
            "Name": name,
            "BlueprintLocation": request["BlueprintLocation"],
            "Description": request.get("Description").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);

        // "The date and time the blueprint was registered", and "the date and time the
        // blueprint was last modified" — which for a registration nothing has updated is
        // the same instant.
        let created_on = read["CreatedOn"].as_f64();
        assert!(
            created_on.is_some_and(|at| at > 0.0),
            "case {case}: expected a CreatedOn instant, got {read}"
        );
        assert_eq!(
            read["LastModifiedOn"].as_f64(),
            created_on,
            "case {case}: a registration nothing has updated reports one instant, got {read}"
        );

        // "Creating — The blueprint registration is in progress": the registration is
        // still in flight the instant the create returns.
        assert_eq!(
            read["Status"].as_str(),
            Some(STATUS_CREATING),
            "case {case}: expected {STATUS_CREATING} straight after the create, got {read}"
        );

        // The modelled `Blueprint` shape carries no `Tags` member, so the tags a create
        // carried are not part of the blueprint a read reports.
        assert!(
            read.get("Tags").is_none(),
            "case {case}: GetBlueprint must report no Tags, got {read}"
        );
    }

    // Each registration is its own record: after every create has landed, each name still
    // reports its OWN location, so one create never overwrote another — and every name is
    // listed.
    let names = listed(&router).await;
    for (case, request) in &cases {
        let name = request["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        assert_eq!(
            read.get("BlueprintLocation"),
            request.get("BlueprintLocation"),
            "case {case}: each blueprint keeps its own BlueprintLocation, got {read}"
        );
        assert!(
            names.iter().any(|listed| listed == name),
            "case {case}: {name:?} must be listed, got {names:?}"
        );
    }
}

/// The tags a create carried are stored: a `ListBlueprints` that "filters the list by an
/// Amazon Web Services resource tag" reports a blueprint only for the pairs it was
/// registered with.
#[tokio::test]
async fn the_tags_a_registration_carried_narrow_a_filtered_listing() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "tag-filtered",
            "BlueprintLocation": "s3://blueprint-bucket/filtered.zip",
            "Tags": { "team": "data", "env": "prod" },
        }),
    )
    .await;
    create(
        &router,
        &json!({
            "Name": "untagged-too",
            "BlueprintLocation": "s3://blueprint-bucket/untagged.zip",
        }),
    )
    .await;

    // (case, the Tags filter, the names it must report)
    let rows: &[(&str, Value, &[&str])] = &[
        (
            "one pair the create carried",
            json!({ "team": "data" }),
            &["tag-filtered"],
        ),
        (
            "both pairs the create carried",
            json!({ "team": "data", "env": "prod" }),
            &["tag-filtered"],
        ),
        (
            "a pair the create did not carry",
            json!({ "team": "data", "env": "dev" }),
            &[],
        ),
        ("a key nothing carries", json!({ "owner": "data" }), &[]),
    ];

    for (case, filter, expected) in rows {
        let page = ok(&router, "ListBlueprints", &json!({ "Tags": filter })).await;
        let names: Vec<&str> = page["Blueprints"]
            .as_array()
            .expect("Blueprints is a list")
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert_eq!(
            names, *expected,
            "case {case}: expected the blueprints carrying every pair of {filter}, got {page}"
        );
    }
}

// ---------------------------------------------------------------------------
// The status the clock owes a registration
// ---------------------------------------------------------------------------

/// A registration leaves `CREATING` for `ACTIVE` when the instant the clock owed it has
/// passed, and not before — and the change is materialised by whichever read happens next,
/// so nothing sleeps and two reads of one registration agree.
///
/// The status turns on the stored DUE instant, so each row positions that instant: a row
/// that reads a registration still in flight holds it ahead of the clock, and a row that
/// reads a settled one leaves it at or behind the clock, where the clock only moves further.
#[tokio::test]
async fn a_registration_reaches_active_once_the_clock_owes_it() {
    // (case, how long ago the registration was made, whether its due instant is held ahead
    //  of the clock, the status a read must report)
    let rows: &[(&str, Duration, bool, &str)] = &[
        ("the instant it was registered", Duration::ZERO, false, STATUS_CREATING),
        (
            "halfway through the span, the due instant still ahead",
            BLUEPRINT_SETTLES_AFTER / 2,
            true,
            STATUS_CREATING,
        ),
        (
            // A registration as old as the span still reports the status its DUE instant
            // says, not one computed from how long ago it was made.
            "a hair short of the span, the due instant still ahead",
            BLUEPRINT_SETTLES_AFTER - Duration::from_millis(1),
            true,
            STATUS_CREATING,
        ),
        ("exactly at the due instant", BLUEPRINT_SETTLES_AFTER, false, STATUS_ACTIVE),
        ("well past the due instant", BLUEPRINT_SETTLES_AFTER * 4, false, STATUS_ACTIVE),
    ];

    for (case, elapsed, due_held_ahead, expected) in rows {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let router = emulator_on(engine.clone());
        create(
            &router,
            &json!({
                "Name": "settling",
                "BlueprintLocation": "s3://blueprint-bucket/settling.zip",
                "Description": "Registration in flight.",
            }),
        )
        .await;
        rewind(&engine, "settling", *elapsed);
        if *due_held_ahead {
            set_due_instant(&engine, "settling", far_future());
        }

        let read = get(&router, "settling").await;
        assert_eq!(
            read["Status"].as_str(),
            Some(*expected),
            "case {case}: expected {expected} {elapsed:?} after the create, got {read}"
        );

        // Reading again reports the same record: the status comes from the DUE instant the
        // create recorded, not from the instant somebody read it, and the registration's
        // own members are untouched by the read.
        let again = get(&router, "settling").await;
        assert_eq!(
            again, read,
            "case {case}: two reads of one registration must agree, got {again} then {read}"
        );
        assert_eq!(
            read["Description"].as_str(),
            Some("Registration in flight."),
            "case {case}: the stored registration must survive the status change, got {read}"
        );
    }
}

/// `IncludeBlueprint` and `IncludeParameterSpec` ask for the members Glue reads out of the
/// blueprint ARCHIVE published at the location. Nothing here reads that archive, so no
/// parameter specification and no service location is invented for it, and the
/// registration a read reports is the same whichever way the two flags are set.
#[tokio::test]
async fn the_include_flags_report_the_same_registration() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "flagged",
            "BlueprintLocation": "s3://blueprint-bucket/flagged.zip",
            "Description": "Flag-independent.",
        }),
    )
    .await;

    let baseline = get(&router, "flagged").await;
    for (case, request) in [
        ("both flags set", json!({ "IncludeBlueprint": true, "IncludeParameterSpec": true })),
        ("both flags clear", json!({ "IncludeBlueprint": false, "IncludeParameterSpec": false })),
        ("the blueprint alone", json!({ "IncludeBlueprint": true })),
        ("the parameter spec alone", json!({ "IncludeParameterSpec": true })),
    ] {
        let mut body = request.clone();
        body["Name"] = json!("flagged");
        let read = ok(&router, "GetBlueprint", &body).await;
        let blueprint = read
            .get("Blueprint")
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a Blueprint, got {read}"));
        assert_eq!(
            blueprint, baseline,
            "case {case}: expected the stored registration, got {blueprint}"
        );
        for member in ["ParameterSpec", "BlueprintServiceLocation"] {
            assert!(
                blueprint.get(member).is_none_or(Value::is_null),
                "case {case}: {member} comes from the published archive and must not be \
                 invented, got {blueprint}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Refusals
// ---------------------------------------------------------------------------

/// A blueprint name is unique per account: the second create under a name already taken is
/// refused with the modelled `AlreadyExistsException`, and the stored registration is left
/// as the first create wrote it.
#[tokio::test]
async fn a_duplicate_blueprint_name_is_refused_and_changes_nothing() {
    let router = emulator();

    let cases: Vec<(&str, Value, Value)> = vec![
        (
            "same name, a different location",
            json!({ "Name": "dup-one", "BlueprintLocation": "s3://blueprint-bucket/first.zip" }),
            json!({ "Name": "dup-one", "BlueprintLocation": "s3://other-bucket/second.zip" }),
        ),
        (
            "same name, a description added",
            json!({ "Name": "dup-two", "BlueprintLocation": "s3://blueprint-bucket/keep.zip" }),
            json!({
                "Name": "dup-two",
                "BlueprintLocation": "s3://blueprint-bucket/keep.zip",
                "Description": "Added by the second create.",
            }),
        ),
        (
            "same name and the identical registration",
            json!({
                "Name": "dup-three",
                "BlueprintLocation": "s3://blueprint-bucket/same.zip",
                "Description": "Same.",
            }),
            json!({
                "Name": "dup-three",
                "BlueprintLocation": "s3://blueprint-bucket/same.zip",
                "Description": "Same.",
            }),
        ),
    ];

    for (case, first, second) in &cases {
        create(&router, first).await;

        let (status, body) = call(&router, "CreateBlueprint", second).await;
        assert_eq!(
            status, 400,
            "case {case}: a duplicate name must be refused with 400, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("AlreadyExistsException"),
            "case {case}: expected AlreadyExistsException, got {body}"
        );

        // The stored registration is still the FIRST create's, member for member.
        let name = first["Name"].as_str().expect("the case carries a Name");
        let read = get(&router, name).await;
        let expected = json!({
            "Name": name,
            "BlueprintLocation": first["BlueprintLocation"],
            "Description": first.get("Description").cloned().unwrap_or(Value::Null),
        });
        assert_echoed(case, &expected, &read);
    }
}

/// A read for a name this account holds no blueprint under is refused with the modelled
/// `EntityNotFoundException` naming the blueprint that was asked for. The rows differ along
/// how the requested name misses: never registered, a prefix of a stored name, a suffix of
/// one, and a stored name in another case — the model documents no case fold for a
/// blueprint name, so a differently-cased name is a different blueprint.
#[tokio::test]
async fn reading_an_unknown_blueprint_is_refused() {
    let router = emulator();
    create(
        &router,
        &json!({
            "Name": "StoredBlueprint",
            "BlueprintLocation": "s3://blueprint-bucket/stored.zip",
        }),
    )
    .await;

    for (case, name) in [
        ("never registered", "no-such-blueprint"),
        ("a prefix of a stored name", "Stored"),
        ("a suffix of a stored name", "Blueprint"),
        ("a stored name lowercased", "storedblueprint"),
        ("a stored name uppercased", "STOREDBLUEPRINT"),
    ] {
        let (status, body) = call(&router, "GetBlueprint", &json!({ "Name": name })).await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for {name:?}, got {status} with {body}"
        );
        assert!(
            error_code(&body).contains("EntityNotFoundException"),
            "case {case}: expected EntityNotFoundException for {name:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            messages::BLUEPRINT_NOT_FOUND,
            "case {case}: the refusal is the recorded blueprint miss, got {body}"
        );
    }

    // The name as stored still resolves, so the misses above are misses and not a broken
    // lookup.
    let read = get(&router, "StoredBlueprint").await;
    assert_eq!(
        read["Name"].as_str(),
        Some("StoredBlueprint"),
        "case the stored name: expected the stored registration, got {read}"
    );
}

/// A registration outside the members' modelled constraints is refused at the boundary and
/// nothing is stored for it: each published length bound is served, one past it is refused,
/// and a location that is not an S3 path of the modelled form
/// (`^s3://([^/]+)/([^/]+/)*([^/]+)$`) is refused whatever else it looks like.
#[tokio::test]
async fn a_registration_outside_the_modelled_constraints_is_refused() {
    let router = emulator();

    // (case, the request, whether it is served)
    let rows: Vec<(&str, Value, bool)> = vec![
        (
            "a location with no scheme",
            json!({ "Name": "bad-1", "BlueprintLocation": "blueprint-bucket/etl.zip" }),
            false,
        ),
        (
            "a location with another scheme",
            json!({ "Name": "bad-2", "BlueprintLocation": "https://blueprint-bucket/etl.zip" }),
            false,
        ),
        (
            "a bucket with no key",
            json!({ "Name": "bad-3", "BlueprintLocation": "s3://blueprint-bucket" }),
            false,
        ),
        (
            "a key that is empty",
            json!({ "Name": "bad-4", "BlueprintLocation": "s3://blueprint-bucket/" }),
            false,
        ),
        (
            "an empty location",
            json!({ "Name": "bad-5", "BlueprintLocation": "" }),
            false,
        ),
        (
            "a location one character past the length bound",
            json!({
                "Name": "bad-6",
                "BlueprintLocation": location_of_length(MAX_LOCATION_LENGTH + 1),
            }),
            false,
        ),
        (
            "a name carrying a character the pattern excludes",
            json!({ "Name": "bad name", "BlueprintLocation": "s3://blueprint-bucket/etl.zip" }),
            false,
        ),
        (
            "a name one character past the length bound",
            json!({
                "Name": "n".repeat(MAX_NAME_LENGTH + 1),
                "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
            }),
            false,
        ),
        (
            "an empty name",
            json!({ "Name": "", "BlueprintLocation": "s3://blueprint-bucket/etl.zip" }),
            false,
        ),
        (
            "a description one character past the length bound",
            json!({
                "Name": "bad-7",
                "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
                "Description": "d".repeat(MAX_DESCRIPTION_LENGTH + 1),
            }),
            false,
        ),
        (
            "no location at all",
            json!({ "Name": "bad-8" }),
            false,
        ),
        (
            "the shortest location the pattern admits",
            json!({ "Name": "good-1", "BlueprintLocation": "s3://b/k" }),
            true,
        ),
        (
            "a location at the length bound",
            json!({
                "Name": "good-2",
                "BlueprintLocation": location_of_length(MAX_LOCATION_LENGTH),
            }),
            true,
        ),
        (
            "a name at the length bound",
            json!({
                "Name": "g".repeat(MAX_NAME_LENGTH),
                "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
            }),
            true,
        ),
        (
            "a description at the length bound",
            json!({
                "Name": "good-3",
                "BlueprintLocation": "s3://blueprint-bucket/etl.zip",
                "Description": "d".repeat(MAX_DESCRIPTION_LENGTH),
            }),
            true,
        ),
    ];

    for (case, request, served) in &rows {
        let (status, body) = call(&router, "CreateBlueprint", request).await;
        if *served {
            assert_eq!(
                status, 200,
                "case {case}: the registration must be served, got {status} with {body}"
            );
            continue;
        }
        assert_eq!(
            status, 400,
            "case {case}: the registration must be refused, got {status} with {body}"
        );
        // Refused means nothing was stored: the name the refused create asked for is not
        // registered.
        if let Some(name) = request["Name"].as_str().filter(|name| !name.is_empty()) {
            let (read_status, read_body) =
                call(&router, "GetBlueprint", &json!({ "Name": name })).await;
            assert_eq!(
                read_status, 400,
                "case {case}: a refused create must store nothing, got {read_status} with \
                 {read_body}"
            );
        }
    }

    // Only the served rows are in the account's listing, so a refusal left no record.
    let names = listed(&router).await;
    let expected: Vec<String> = vec![
        "g".repeat(MAX_NAME_LENGTH),
        "good-1".to_string(),
        "good-2".to_string(),
        "good-3".to_string(),
    ];
    assert_eq!(
        names, expected,
        "case the whole table: only the served registrations are held, got {names:?}"
    );
}

/// The store holds a bounded number of blueprints: the create at the ceiling succeeds, the
/// one past it is refused with the modelled limit error, and freeing a slot lets a create
/// succeed again — so the ceiling bounds what is stored rather than latching on how many
/// creates were ever made.
#[tokio::test]
async fn a_create_past_the_blueprint_ceiling_is_refused() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let router = emulator_on(engine.clone());

    // Reaching the ceiling over the wire would take a thousand requests and would exercise
    // the transport rather than the bound.
    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        let mut store = state.lock();
        for index in 0..MAX_BLUEPRINTS - 1 {
            let name = format!("seeded-{index}");
            let input = CreateBlueprintInput {
                name: name.clone(),
                blueprint_location: format!("s3://blueprint-bucket/{name}.zip"),
                ..CreateBlueprintInput::default()
            };
            store.insert_blueprint(BlueprintEntry::from_create_input(
                &input,
                std::time::SystemTime::now(),
                // This row is about the ceiling on what is stored, so each seeded record is
                // a registration whose published archive was read.
                BlueprintArchive::Published,
            ));
        }
    }

    let last = json!({ "Name": "last-blueprint", "BlueprintLocation": "s3://b/last.zip" });
    let past = json!({ "Name": "one-too-many", "BlueprintLocation": "s3://b/past.zip" });

    create(&router, &last).await;
    let (status, body) = call(&router, "CreateBlueprint", &past).await;
    assert_eq!(
        status, 400,
        "case blueprint {} of {MAX_BLUEPRINTS}: expected 400, got {status} with {body}",
        MAX_BLUEPRINTS + 1
    );
    assert!(
        error_code(&body).contains("ResourceNumberLimitExceededException"),
        "case one blueprint too many: expected ResourceNumberLimitExceededException, got {body}"
    );

    {
        let state = engine.state::<ComAmazonawsGlueAwsglueState>();
        state.lock().blueprints.remove("seeded-0");
    }
    create(&router, &past).await;
}
