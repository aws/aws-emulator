// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a blueprint REGISTRATION comes to: the terminal
//! `BlueprintStatus` the archive published at `BlueprintLocation` decides, the
//! `Blueprint$ErrorMessage` a registration that could not read that archive carries, and the
//! `Blueprint$Version` every registration reports.
//!
//! The rule under test: `CreateBlueprint` is asynchronous, so the create records the
//! registration in `CREATING` and the registration then settles on what the OBJECT STORE
//! answers about the archive at `BlueprintLocation` — `ACTIVE` ("the blueprint has been
//! successfully registered") when the store serves that object, and `FAILED` ("the blueprint
//! registration failed") carrying `Access denied on object at location: <BlueprintLocation>.`
//! when it answers that it holds nothing there or will not serve it. Every registration a
//! create wrote reports `Version` 1, the first version of the blueprint's definition, and a
//! registration that has not settled yet carries no `ErrorMessage` at all.
//!
//! Amazon S3 is mounted beside Glue on one engine, so the archive a row stages with
//! `PutObject` is the object the registration reads — what is asserted is HTTP behaviour over
//! state the object store really holds.
//!
//! The rows differ along what the location names and whether the store serves it: an object
//! staged under one key segment, one staged under several, a key never put in a bucket that
//! exists, and a bucket that does not exist at all.
//!
//! The due instant the registration settles at is moved THROUGH THE STORE rather than slept
//! for, so every transition here is deterministic and no outcome depends on how fast the host
//! runs.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_glue_awsglue::state::{
    messages, ComAmazonawsGlueAwsglueState, BLUEPRINT_STATUS_ACTIVE, BLUEPRINT_STATUS_CREATING,
    BLUEPRINT_STATUS_FAILED, FIRST_BLUEPRINT_VERSION,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::job_runs::already_past;
use crate::job_script_execution::{create_bucket, emulator_with_s3, BUCKET};
use crate::wire::{call, error_code, error_message, ok, s3_client};

/// The role a blueprint run would be started as, for the one row that proves a registration
/// that failed backs no run.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/sim2real-glue-blueprint-role";

/// Stage `key` in the suite's bucket as a published blueprint archive, and answer the
/// `s3://` location that names it.
async fn stage_archive(router: &MultiServiceRouter, key: &str) -> String {
    s3_client(router)
        .put_object()
        .bucket(BUCKET)
        .key(key)
        .body(aws_sdk_s3::primitives::ByteStream::from_static(
            b"PK\x03\x04 a blueprint archive",
        ))
        .send()
        .await
        .unwrap_or_else(|error| panic!("the archive {key} must be staged, got {error:?}"));
    format!("s3://{BUCKET}/{key}")
}

/// The `Blueprint` the `GetBlueprint` answer for `name` reports.
async fn read(router: &MultiServiceRouter, name: &str) -> Value {
    let body = ok(router, "GetBlueprint", &json!({ "Name": name })).await;
    body.get("Blueprint")
        .cloned()
        .unwrap_or_else(|| panic!("GetBlueprint must report a Blueprint, got {body}"))
}

/// Put the instant the clock owes `name`'s registration its terminal status in the past, so
/// the next read reports the status the clock had already reached.
fn make_settlement_due(engine: &Arc<engine::InMemoryEngine>, name: &str) {
    let state = engine.state::<ComAmazonawsGlueAwsglueState>();
    let mut store = state.lock();
    let entry = store
        .blueprint_mut(name)
        .unwrap_or_else(|| panic!("{name} was registered"));
    entry.settles_at = Some(already_past());
}

/// One row of the registration table: the location the create publishes the blueprint at and
/// the terminal status that location's archive decides.
struct ArchiveCase {
    /// What this row varies, named in every assertion it drives.
    case: &'static str,
    /// The key to stage before the create, and `None` for a row whose location names an
    /// object the store never holds.
    staged: Option<&'static str>,
    /// The location the create sends, when the row does not stage one.
    unstaged_location: Option<&'static str>,
    /// The `BlueprintStatus` the registration must come to rest in.
    settles_to: &'static str,
}

/// The rows: two locations the store serves and two it does not.
fn archive_cases() -> Vec<ArchiveCase> {
    vec![
        ArchiveCase {
            case: "an archive staged under one key segment",
            staged: Some("served.zip"),
            unstaged_location: None,
            settles_to: BLUEPRINT_STATUS_ACTIVE,
        },
        ArchiveCase {
            case: "an archive staged under several key segments",
            staged: Some("blueprints/nested/served.zip"),
            unstaged_location: None,
            settles_to: BLUEPRINT_STATUS_ACTIVE,
        },
        ArchiveCase {
            case: "a key the bucket does not hold",
            staged: None,
            unstaged_location: Some("never-put.zip"),
            settles_to: BLUEPRINT_STATUS_FAILED,
        },
        ArchiveCase {
            case: "a bucket the object store does not hold",
            staged: None,
            unstaged_location: Some("@other-bucket/archive.zip"),
            settles_to: BLUEPRINT_STATUS_FAILED,
        },
    ]
}

/// The location a row registers at: the staged object's, or the one the row names in the
/// suite's bucket — unless it names another bucket, which the `@` prefix marks.
async fn location_of(router: &MultiServiceRouter, row: &ArchiveCase) -> String {
    match (row.staged, row.unstaged_location) {
        (Some(key), _) => stage_archive(router, key).await,
        (None, Some(location)) => match location.strip_prefix('@') {
            Some(elsewhere) => format!("s3://{elsewhere}"),
            None => format!("s3://{BUCKET}/{location}"),
        },
        (None, None) => panic!("case {}: a row names a location", row.case),
    }
}

/// A registration settles on what the object store answers about its published archive: the
/// served locations reach `ACTIVE` with no error, and the ones nothing serves reach `FAILED`
/// carrying the location they could not read.
#[tokio::test]
async fn a_registration_settles_on_whether_its_archive_can_be_read() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, row) in archive_cases().iter().enumerate() {
        let case = row.case;
        let name = format!("archive-blueprint-{index}");
        let location = location_of(&router, row).await;
        ok(
            &router,
            "CreateBlueprint",
            &json!({ "Name": name, "BlueprintLocation": location }),
        )
        .await;

        // Whatever the archive comes to, the registration is reported as being made until
        // the clock owes it its terminal status, and it carries no error yet.
        let registering = read(&router, &name).await;
        assert_eq!(
            registering.get("Status").and_then(Value::as_str),
            Some(BLUEPRINT_STATUS_CREATING),
            "case {case}: a registration in progress reports {BLUEPRINT_STATUS_CREATING}, got \
             {registering}"
        );
        assert!(
            registering.get("ErrorMessage").is_none_or(Value::is_null),
            "case {case}: a registration that has not settled carries no ErrorMessage, got \
             {registering}"
        );

        make_settlement_due(&engine, &name);
        let settled = read(&router, &name).await;
        assert_eq!(
            settled.get("Status").and_then(Value::as_str),
            Some(row.settles_to),
            "case {case}: expected the registration to settle {}, got {settled}",
            row.settles_to
        );
        let expected_error = (row.settles_to == BLUEPRINT_STATUS_FAILED)
            .then(|| messages::blueprint_location_not_accessible(&location));
        assert_eq!(
            settled
                .get("ErrorMessage")
                .and_then(Value::as_str)
                .map(str::to_string),
            expected_error,
            "case {case}: expected the error message of a {} registration, got {settled}",
            row.settles_to
        );
        assert_eq!(
            settled.get("BlueprintLocation").and_then(Value::as_str),
            Some(location.as_str()),
            "case {case}: the registration reports where the blueprint is published, got \
             {settled}"
        );

        // A settled registration is settled: reading it again reports the same record, so
        // the read materialises what the clock owed rather than moving the record because it
        // was read.
        let again = read(&router, &name).await;
        assert_eq!(
            again, settled,
            "case {case}: a settled registration must read the same twice; first {settled}, \
             then {again}"
        );
    }
}

/// Every registration a create wrote reports the first version of the blueprint's
/// definition, in every status it passes through.
#[tokio::test]
async fn every_registration_reports_the_version_of_its_definition() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    for (index, row) in archive_cases().iter().enumerate() {
        let case = row.case;
        let name = format!("version-blueprint-{index}");
        let location = location_of(&router, row).await;
        ok(
            &router,
            "CreateBlueprint",
            &json!({ "Name": name, "BlueprintLocation": location }),
        )
        .await;

        for stage in ["while it is being registered", "once it has settled"] {
            if stage == "once it has settled" {
                make_settlement_due(&engine, &name);
            }
            let reported = read(&router, &name).await;
            assert_eq!(
                reported.get("Version").and_then(Value::as_i64),
                Some(i64::from(FIRST_BLUEPRINT_VERSION)),
                "case {case} {stage}: the registration a create wrote is version \
                 {FIRST_BLUEPRINT_VERSION}, got {reported}"
            );
        }
    }
}

/// A registration that failed has no definition to run, so a run started of it is refused
/// with the modelled state error and the blueprint holds no run.
#[tokio::test]
async fn a_failed_registration_backs_no_run() {
    let (router, engine) = emulator_with_s3();
    create_bucket(&router).await;

    let case = "a blueprint whose archive the object store does not hold";
    let name = "unreadable-blueprint";
    ok(
        &router,
        "CreateBlueprint",
        &json!({
            "Name": name,
            "BlueprintLocation": format!("s3://{BUCKET}/never-published.zip"),
        }),
    )
    .await;
    make_settlement_due(&engine, name);
    let settled = read(&router, name).await;
    assert_eq!(
        settled.get("Status").and_then(Value::as_str),
        Some(BLUEPRINT_STATUS_FAILED),
        "case {case}: the registration must settle {BLUEPRINT_STATUS_FAILED}, got {settled}"
    );

    let (status, body) = call(
        &router,
        "StartBlueprintRun",
        &json!({ "BlueprintName": name, "RoleArn": ROLE_ARN }),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case}: a run of a registration that failed is refused, got {status} with {body}"
    );
    assert!(
        error_code(&body).contains("IllegalBlueprintStateException"),
        "case {case}: expected IllegalBlueprintStateException, got {body}"
    );
    assert_eq!(
        error_message(&body),
        messages::blueprint_has_no_active_definition(name),
        "case {case}: the refusal names the blueprint that has no active definition, got {body}"
    );

    let runs = ok(
        &router,
        "GetBlueprintRuns",
        &json!({ "BlueprintName": name }),
    )
    .await;
    assert_eq!(
        runs.get("BlueprintRuns")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(0),
        "case {case}: a refused start creates no run, got {runs}"
    );
}
