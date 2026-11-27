// SPDX-License-Identifier: CC0-1.0

//! What a description of an application's snapshot configuration reports, and what the mode
//! it reports decides.
//!
//! The rule under test: a described snapshot configuration reports the snapshot MODE the
//! application's snapshots are taken under beside whether they are taken at all, so every
//! response carrying an `ApplicationSnapshotConfigurationDescription` carries both members —
//! the mode being the one the configuration SELECTED, else the one an application that
//! selected none runs under. The mode is held on the stored description, so every reader of
//! one application agrees on it, an update that names only `SnapshotsEnabled` carries it
//! through, and a version the application has LEFT keeps reporting the mode that version ran
//! under.
//!
//! The mode then decides what the snapshot operations serve: an ENHANCED snapshot is a
//! resource of the ACCOUNT, taken only of a Managed Service for Apache Flink application that
//! runs Apache Flink 1.20 or later and selected `ENHANCED_SNAPSHOTS`, and its whole lifecycle
//! — `CREATING` to `READY`, exported, deleted — is computed from stored state, which the
//! second half of this suite asserts through every reader of it.
//!
//! The rows span what the rules generalise over: the runtime family the application runs on
//! (the Flink family and the Zeppelin notebooks, read from the generated
//! `RUNTIME_ENVIRONMENT_VALUES`), both sides of the published Apache Flink 1.20 bound, how its
//! mode was decided (named by the request, implied by its runtime, moved by an update),
//! whether its create configured snapshots at all, which way it configured them, how a
//! snapshot came into being, the status an operation is attempted in, each `ListSnapshots`
//! filter the model declares, and the page sizes a caller asks for.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::RUNTIME_ENVIRONMENT_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::ops::types::SNAPSHOT_MODE_VALUES;
use service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::state::DEFAULT_SNAPSHOT_MODE;

use super::harness::{
    await_status, call, create_body, create_body_carrying, creates_applications_on,
    current_notebook_runtime, describe, error_code, router, runtime_mode, ACCOUNT, CUSTOMER_KEY,
    POLL_INTERVAL, SETTLE_DEADLINE,
};

/// The `RuntimeEnvironment` family a Studio notebook runs on
/// (`smithy_query(query="shape",shape="RuntimeEnvironment")`: `ZEPPELIN-FLINK-<major>_<minor>`).
/// A notebook has no version history to read — `DescribeApplicationVersion` is "supported
/// only for Managed Service for Apache Flink" (its own doc) — so a row on one asserts the
/// describe family it IS served.
const NOTEBOOK_FAMILY: &str = "ZEPPELIN-";

/// The described snapshot configuration of one `ApplicationDetail`.
fn snapshot_description(detail: &Value) -> &Value {
    &detail["ApplicationConfigurationDescription"]["ApplicationSnapshotConfigurationDescription"]
}

/// The `(SnapshotsEnabled, SnapshotMode)` pair a detail describes, as the two wire values.
fn described(detail: &Value) -> (Option<bool>, Option<&str>) {
    let snapshots = snapshot_description(detail);
    (
        snapshots["SnapshotsEnabled"].as_bool(),
        snapshots["SnapshotMode"].as_str(),
    )
}

/// Assert that `detail` describes its snapshot configuration with both members: snapshots
/// switched the way the case expects, and the mode in force.
fn assert_describes_mode(case: &str, reader: &str, detail: &Value, snapshots_enabled: bool) {
    assert_eq!(
        described(detail),
        (Some(snapshots_enabled), Some(DEFAULT_SNAPSHOT_MODE)),
        "case {case:?} read through {reader}: expected SnapshotsEnabled {snapshots_enabled} \
         and SnapshotMode {DEFAULT_SNAPSHOT_MODE:?}, got {:?}",
        snapshot_description(detail)
    );
}

/// Create an application, asserting the call was accepted, and answer its detail.
async fn create_with(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    mode: Option<&str>,
    configuration: Option<Value>,
) -> Value {
    let mut body = create_body(name, runtime);
    if let Some(mode) = mode {
        body["ApplicationMode"] = json!(mode);
    }
    if let Some(configuration) = configuration {
        body["ApplicationConfiguration"] = configuration;
    }
    let (status, created) = call(router, "CreateApplication", &body).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected CreateApplication to be accepted, got {status} {created}"
    );
    created["ApplicationDetail"].clone()
}

/// `DescribeApplicationVersion` on one version of `name`, asserting the call succeeded.
async fn describe_version(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    version_id: i64,
) -> Value {
    let (status, answered) = call(
        router,
        "DescribeApplicationVersion",
        &json!({"ApplicationName": name, "ApplicationVersionId": version_id}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} v{version_id}: DescribeApplicationVersion got {status} {answered}"
    );
    answered["ApplicationVersionDetail"].clone()
}

/// Apply one `UpdateApplication` to `name` against the version it is on now, and answer the
/// detail the update reported.
async fn update(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    current: i64,
    configuration_update: Value,
) -> Value {
    let (status, updated) = call(
        router,
        "UpdateApplication",
        &json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": current,
            "ApplicationConfigurationUpdate": configuration_update,
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?} v{current}: UpdateApplication got {status} {updated}"
    );
    updated["ApplicationDetail"].clone()
}

/// The mode a described snapshot configuration reports is the legacy marker, pinned as the
/// WIRE STRING rather than read off the service's own constant: the recorded service reports
/// that marker for an application whose mode was never selected, so a change to the constant
/// is a failure here even though the table above would follow it.
#[tokio::test]
async fn the_reported_mode_is_the_legacy_marker_on_the_wire() {
    /// The snapshot mode of an application that never selected one, as the service spells it.
    const LEGACY_MODE: &str = "APPLICATION_SNAPSHOTS_LEGACY";

    let router = router();
    let name = "legacy-marker";
    let created = create_with(&router, LEGACY_MODE, name, "FLINK-1_20", None, None).await;
    assert_eq!(
        snapshot_description(&created)["SnapshotMode"].as_str(),
        Some(LEGACY_MODE),
        "expected the create to report the legacy snapshot mode, got {:?}",
        snapshot_description(&created)
    );
    let described = describe(&router, name).await;
    assert_eq!(
        snapshot_description(&described)["SnapshotMode"].as_str(),
        Some(LEGACY_MODE),
        "expected the describe to report the legacy snapshot mode, got {:?}",
        snapshot_description(&described)
    );
    assert_eq!(
        DEFAULT_SNAPSHOT_MODE, LEGACY_MODE,
        "expected the mode the rest of this suite reads to be the legacy marker, got \
         {DEFAULT_SNAPSHOT_MODE:?}"
    );
}

/// Every runtime this API creates applications for describes its snapshot configuration with
/// the mode in force, whatever its create said about snapshots — on the create's own answer
/// and on every later read of the same application.
#[tokio::test]
async fn every_described_snapshot_configuration_reports_the_mode_in_force() {
    // (what the create says about snapshots, the ApplicationConfiguration it carries,
    // whether snapshots then run for a Flink application, whether they run for a notebook)
    let configurations: [(&str, Option<Value>, bool, bool); 3] = [
        ("no configuration at all", None, true, false),
        (
            "snapshots switched on",
            Some(json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": true}})),
            true,
            true,
        ),
        (
            "snapshots switched off",
            Some(json!({"ApplicationSnapshotConfiguration": {"SnapshotsEnabled": false}})),
            false,
            false,
        ),
    ];

    let router = router();
    for runtime in RUNTIME_ENVIRONMENT_VALUES
        .iter()
        .filter(|runtime| creates_applications_on(runtime))
    {
        let notebook = runtime.starts_with(NOTEBOOK_FAMILY);
        for (index, (configured, configuration, flink_snapshots, notebook_snapshots)) in
            configurations.iter().enumerate()
        {
            let case = format!("{runtime} created with {configured}");
            let name = format!("snapshot-mode-{}-{index}", runtime.to_lowercase());
            let snapshots_enabled = if notebook {
                *notebook_snapshots
            } else {
                *flink_snapshots
            };

            let created =
                create_with(&router, &case, &name, runtime, None, configuration.clone()).await;
            assert_describes_mode(&case, "CreateApplication", &created, snapshots_enabled);

            let described = describe(&router, &name).await;
            assert_describes_mode(&case, "DescribeApplication", &described, snapshots_enabled);

            // A per-version read is served for the Flink family only, so the notebook rows
            // stop at the two describe-shaped answers above.
            if !notebook {
                let version = describe_version(&router, &case, &name, 1).await;
                assert_describes_mode(
                    &case,
                    "DescribeApplicationVersion",
                    &version,
                    snapshots_enabled,
                );
            }
        }
    }
}

/// The mode an application runs under is decided for the application, not for the request:
/// a create that NAMES its mode and one that leaves it to the runtime describe the same
/// snapshot mode.
#[tokio::test]
async fn the_mode_is_reported_however_the_application_mode_was_decided() {
    // (case, runtime, the ApplicationMode the create names, whether snapshots then run)
    let rows: [(&str, &str, Option<&str>, bool); 4] = [
        (
            "a streaming runtime naming its mode",
            "FLINK-1_20",
            Some("STREAMING"),
            true,
        ),
        (
            "a streaming runtime naming no mode",
            "FLINK-1_8",
            None,
            true,
        ),
        (
            "a notebook runtime naming its mode",
            "ZEPPELIN-FLINK-3_0",
            Some("INTERACTIVE"),
            false,
        ),
        (
            "a notebook runtime naming no mode",
            "ZEPPELIN-FLINK-3_0",
            None,
            false,
        ),
    ];

    let router = router();
    for (case, runtime, named_mode, snapshots_enabled) in rows {
        let name = format!("mode-{}", case.replace(' ', "-"));
        let created = create_with(&router, case, &name, runtime, named_mode, None).await;
        assert_describes_mode(case, "CreateApplication", &created, snapshots_enabled);
        let described = describe(&router, &name).await;
        assert_describes_mode(case, "DescribeApplication", &described, snapshots_enabled);
    }
}

/// An update carries the stored mode through, whatever it names: one that switches snapshots
/// the other way reports the same mode with the new switch, one that touches another
/// sub-configuration leaves both members as they stood — and the version the application has
/// LEFT keeps reporting the configuration it ran under.
#[tokio::test]
async fn an_update_carries_the_stored_mode_through() {
    // (case, the ApplicationConfigurationUpdate, whether snapshots run after it)
    let rows: [(&str, Value, bool); 3] = [
        (
            "an update that switches snapshots off",
            json!({"ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": false}}),
            false,
        ),
        (
            "an update that switches snapshots on",
            json!({"ApplicationSnapshotConfigurationUpdate": {"SnapshotsEnabledUpdate": true}}),
            true,
        ),
        (
            "an update that names another sub-configuration",
            json!({
                "EnvironmentPropertyUpdates": {
                    "PropertyGroups": [
                        {"PropertyGroupId": "producer", "PropertyMap": {"topic": "orders"}}
                    ]
                }
            }),
            true,
        ),
    ];

    let router = router();
    for (index, (case, configuration_update, snapshots_enabled)) in rows.into_iter().enumerate() {
        let name = format!("snapshot-mode-update-{index}");
        let created = create_with(&router, case, &name, "FLINK-1_19", None, None).await;
        assert_describes_mode(case, "CreateApplication", &created, true);

        let updated = update(&router, case, &name, 1, configuration_update).await;
        assert_describes_mode(case, "UpdateApplication", &updated, snapshots_enabled);

        let described = describe(&router, &name).await;
        assert_describes_mode(case, "DescribeApplication", &described, snapshots_enabled);

        let in_force = updated["ApplicationVersionId"]
            .as_i64()
            .unwrap_or_else(|| panic!("case {case:?}: expected a version id, got {updated}"));
        let current_version = describe_version(&router, case, &name, in_force).await;
        assert_describes_mode(
            case,
            "DescribeApplicationVersion on the version in force",
            &current_version,
            snapshots_enabled,
        );

        // The version the update left behind is reported as it RAN: snapshots on, since
        // every one of these applications was created with the streaming defaults.
        let superseded = describe_version(&router, case, &name, 1).await;
        assert_describes_mode(
            case,
            "DescribeApplicationVersion on the superseded version",
            &superseded,
            true,
        );
    }
}

// ---------------------------------------------------------------------------------------
// The ENHANCED snapshot mode, and the account-scoped snapshots it unlocks.
// ---------------------------------------------------------------------------------------

/// The mode an application takes ENHANCED snapshots under, as the model's `SnapshotMode` enum
/// spells it.
const ENHANCED: &str = "ENHANCED_SNAPSHOTS";

/// The earliest `RuntimeEnvironment` generation that takes enhanced snapshots, as
/// `ApplicationSnapshotConfiguration$SnapshotMode` publishes the bound ("the application must
/// use Apache Flink version 1.20 or later"), the generation one step BELOW it, one above it,
/// and the Studio-notebook family, which runs no Apache Flink job of its own.
const AT_THE_BOUND: &str = "FLINK-1_20";
const BELOW_THE_BOUND: &str = "FLINK-1_19";
const ABOVE_THE_BOUND: &str = "FLINK-2_3";

/// A bucket ARN and key prefix for an export destination or an import source, matching the
/// modelled `^arn:` pattern of `S3PrefixLocation$BucketARN`.
const SNAPSHOT_BUCKET: &str = "arn:aws:s3:::kda-enhanced-snapshots";
const SNAPSHOT_PREFIX: &str = "savepoints/one";

/// A role ARN for an export or an import to assume, matching the modelled `RoleARN` pattern.
const SNAPSHOT_ROLE: &str = "arn:aws:iam::000000000000:role/kda-snapshot-role";

/// A `CreateApplication` body selecting `mode`, and carrying the inline code a startable
/// application needs when the case starts it.
fn create_selecting_mode(
    name: &str,
    runtime: &str,
    mode: Option<&str>,
    with_code: bool,
) -> Value {
    let mut snapshots = json!({"SnapshotsEnabled": true});
    if let Some(mode) = mode {
        snapshots["SnapshotMode"] = json!(mode);
    }
    let mut configuration = json!({"ApplicationSnapshotConfiguration": snapshots});
    if with_code {
        configuration["ApplicationCodeConfiguration"] = json!({
            "CodeContentType": "PLAINTEXT",
            "CodeContent": {"TextContent": "-- the application's code"},
        });
    }
    create_body_carrying(
        name,
        runtime,
        &json!({
            "ApplicationMode": runtime_mode(runtime),
            "ApplicationConfiguration": configuration,
        }),
    )
}

/// Create an application selecting `mode` and start it, so a snapshot of its state can be
/// taken: a snapshot is taken only of a RUNNING application.
async fn running_application_selecting(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    runtime: &str,
    mode: Option<&str>,
) -> Value {
    let (status, created) = call(
        router,
        "CreateApplication",
        &create_selecting_mode(name, runtime, mode, true),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateApplication({name:?}) got {status} {created}"
    );
    let (status, started) = call(router, "StartApplication", &json!({"ApplicationName": name})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: StartApplication({name:?}) got {status} {started}"
    );
    await_status(router, case, name, "RUNNING").await;
    created["ApplicationDetail"].clone()
}

/// `DescribeSnapshot` for one name, as `(http status, body)`.
async fn describe_snapshot(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "DescribeSnapshot", &json!({"SnapshotName": name})).await
}

/// Look again until the enhanced snapshot reports `expected`, and answer its details.
///
/// A status the clock owes becomes due without any further call, so what this waits for is the
/// CONDITION: a slow host waits longer rather than failing.
async fn await_snapshot_status(
    router: &MultiServiceRouter,
    case: &str,
    name: &str,
    expected: &str,
) -> Value {
    let give_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = describe_snapshot(router, name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSnapshot({name:?}) got {status} {described}"
        );
        let details = described["SnapshotInfoDetails"].clone();
        if details["SnapshotStatus"].as_str() == Some(expected) {
            return details;
        }
        assert!(
            tokio::time::Instant::now() < give_up_at,
            "case {case:?}: expected snapshot {name:?} to settle at {expected}, got {details}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until one operation of the snapshot reports `expected`, and answer its details.
async fn await_operation_status(
    router: &MultiServiceRouter,
    case: &str,
    snapshot: &str,
    operation_id: &str,
    expected: &str,
) -> Value {
    let give_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, described) = call(
            router,
            "DescribeSnapshotOperation",
            &json!({"SnapshotName": snapshot, "SnapshotOperationId": operation_id}),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: DescribeSnapshotOperation({snapshot:?}, {operation_id:?}) got \
             {status} {described}"
        );
        let details = described["SnapshotOperationInfoDetails"].clone();
        if details["SnapshotOperationStatus"].as_str() == Some(expected) {
            return details;
        }
        assert!(
            tokio::time::Instant::now() < give_up_at,
            "case {case:?}: expected operation {operation_id:?} to settle at {expected}, got \
             {details}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look again until the snapshot no longer exists, which is what a completed delete leaves.
async fn await_snapshot_absent(router: &MultiServiceRouter, case: &str, name: &str) {
    let give_up_at = tokio::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, answered) = describe_snapshot(router, name).await;
        if (status, answer_code(&answered).as_str()) == (400, "ResourceNotFoundException") {
            return;
        }
        assert!(
            tokio::time::Instant::now() < give_up_at,
            "case {case:?}: expected snapshot {name:?} to be removed, got {status} {answered}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// An `ImportSnapshot` body reading a savepoint from Amazon S3 under `name`.
fn import_body(name: &str) -> Value {
    json!({
        "SnapshotName": name,
        "ImportSourceConfiguration": {
            "SourceType": "S3",
            "ImportRoleArn": SNAPSHOT_ROLE,
            "SourceS3PrefixLocation": {
                "BucketARN": SNAPSHOT_BUCKET,
                "KeyPrefix": SNAPSHOT_PREFIX,
            },
        },
    })
}

/// The modelled error code an answer carries, or the EMPTY string when the call succeeded, so
/// one table row states what it expects whether the case is served or refused.
fn answer_code(body: &Value) -> String {
    if body.get("__type").is_some() {
        error_code(body)
    } else {
        String::new()
    }
}

/// Every `SnapshotName` one `ListSnapshots` page reports, in the order it reports them.
fn listed_names(page: &Value) -> Vec<String> {
    page["SnapshotInfoList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|snapshot| {
            snapshot["SnapshotName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// The table below judges every `SnapshotMode` the model declares, so a mode the enum gains
/// later is left unjudged by neither half of this suite.
#[tokio::test]
async fn the_modelled_snapshot_modes_are_the_two_this_suite_drives() {
    let mut declared: Vec<&str> = SNAPSHOT_MODE_VALUES.to_vec();
    declared.sort_unstable();
    assert_eq!(
        declared,
        vec![DEFAULT_SNAPSHOT_MODE, ENHANCED],
        "expected the modelled snapshot modes to be the two this suite drives, got \
         {SNAPSHOT_MODE_VALUES:?}"
    );
}

/// A created enhanced snapshot is reported by every reader of it from the record the create
/// wrote, its operations settle on the clock, and the snapshot stops existing once its delete
/// completes.
#[tokio::test]
async fn an_enhanced_snapshot_round_trips_through_every_reader() {
    let case = "a snapshot created from a running application";
    let router = router();
    let application = "enhanced-round-trip";
    let detail =
        running_application_selecting(&router, case, application, AT_THE_BOUND, Some(ENHANCED))
            .await;
    assert_eq!(
        snapshot_description(&detail)["SnapshotMode"].as_str(),
        Some(ENHANCED),
        "case {case:?}: expected the selected mode to be described, got {detail}"
    );

    let snapshot = "enhanced-round-trip-snapshot";
    let (status, taken) = call(
        &router,
        "CreateSnapshot",
        &json!({
            "ApplicationName": application,
            "SnapshotName": snapshot,
            "SnapshotDescription": "the state of a running job",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: CreateSnapshot got {status} {taken}"
    );
    let expected_arn = format!("arn:aws:kinesisanalytics:us-east-1:{ACCOUNT}:snapshot/{snapshot}");
    assert_eq!(
        (
            taken["SnapshotArn"].as_str(),
            taken["SnapshotStatus"].as_str(),
            taken["SnapshotOperationStatus"].as_str(),
        ),
        (
            Some(expected_arn.as_str()),
            Some("CREATING"),
            Some("IN_PROGRESS"),
        ),
        "case {case:?}: expected the snapshot's ARN with the transient statuses, got {taken}"
    );
    let create_operation = taken["SnapshotOperationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !create_operation.is_empty(),
        "case {case:?}: expected a snapshot operation id, got {taken}"
    );

    // Every member a reader sees is the one that was written: the application the snapshot was
    // taken from, the version it was on, who initiated it and in which format.
    let details = await_snapshot_status(&router, case, snapshot, "READY").await;
    let source = details["SnapshotSourceDescription"]["SourceApplicationDescription"].clone();
    assert_eq!(
        (
            details["SnapshotName"].as_str(),
            details["SnapshotArn"].as_str(),
            details["SnapshotDescription"].as_str(),
            details["RuntimeEnvironment"].as_str(),
            details["SnapshotInitiator"].as_str(),
            details["SnapshotInitiatorOperation"].as_str(),
            details["SnapshotFormat"].as_str(),
            source["ApplicationName"].as_str(),
            source["ApplicationVersionId"].as_i64(),
            source["ApplicationCreationTimestamp"].as_f64(),
        ),
        (
            Some(snapshot),
            Some(expected_arn.as_str()),
            Some("the state of a running job"),
            Some(AT_THE_BOUND),
            Some("USER"),
            Some("CreateSnapshot"),
            Some("CANONICAL"),
            Some(application),
            detail["ApplicationVersionId"].as_i64(),
            detail["CreateTimestamp"].as_f64(),
        ),
        "case {case:?}: expected the stored snapshot to be reported as written, got {details}"
    );
    assert!(
        details["SnapshotSourceDescription"]
            .get("S3PrefixLocation")
            .is_none(),
        "case {case:?}: a snapshot taken of an application names no import location, got \
         {details}"
    );

    // Its create operation settled, and it is the operation the snapshot's own log reports.
    let operation =
        await_operation_status(&router, case, snapshot, &create_operation, "SUCCESSFUL").await;
    assert_eq!(
        (
            operation["SnapshotOperation"].as_str(),
            operation["SnapshotArn"].as_str(),
            operation["EndTime"].is_null(),
        ),
        (Some("CreateSnapshot"), Some(expected_arn.as_str()), false),
        "case {case:?}: expected the completed create operation, got {operation}"
    );
    let (status, operations) = call(
        &router,
        "ListSnapshotOperations",
        &json!({"SnapshotName": snapshot}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: ListSnapshotOperations got {status} {operations}"
    );
    assert_eq!(
        operations["SnapshotOperationInfoList"]
            .as_array()
            .map(|list| list
                .iter()
                .map(|entry| (
                    entry["SnapshotOperationId"].as_str().unwrap_or_default(),
                    entry["SnapshotOperation"].as_str().unwrap_or_default(),
                    entry["SnapshotOperationStatus"].as_str().unwrap_or_default(),
                ))
                .collect::<Vec<_>>()),
        Some(vec![(
            create_operation.as_str(),
            "CreateSnapshot",
            "SUCCESSFUL"
        )]),
        "case {case:?}: expected the snapshot's own operation log, got {operations}"
    );

    // An export of the READY snapshot runs as an operation of its own, and the prefix the data
    // was written under is reported once it completes.
    let (status, exported) = call(
        &router,
        "ExportSnapshot",
        &json!({
            "SnapshotName": snapshot,
            "ExportDestinationConfiguration": {
                "DestinationType": "S3",
                "ExportRoleArn": SNAPSHOT_ROLE,
                "DestinationS3PrefixLocation": {
                    "BucketARN": SNAPSHOT_BUCKET,
                    "KeyPrefix": "exports/one",
                },
            },
        }),
    )
    .await;
    assert_eq!(
        (status, exported["SnapshotOperationStatus"].as_str()),
        (200, Some("IN_PROGRESS")),
        "case {case:?}: ExportSnapshot got {status} {exported}"
    );
    let export_operation = exported["SnapshotOperationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let export =
        await_operation_status(&router, case, snapshot, &export_operation, "SUCCESSFUL").await;
    let location = export["SnapshotDestinationLocationDescription"].clone();
    assert_eq!(
        (
            location["LocationType"].as_str(),
            location["S3PrefixLocation"]["BucketARN"].as_str(),
            location["S3PrefixLocation"]["KeyPrefix"].as_str(),
            export["DestinationEncryptionConfigurationDescription"]["KeyType"].as_str(),
        ),
        (
            Some("S3"),
            Some(SNAPSHOT_BUCKET),
            Some("exports/one"),
            Some("AWS_OWNED_KEY"),
        ),
        "case {case:?}: expected the export's recorded destination, got {export}"
    );
    assert!(
        location["S3PrefixLocation"]["ExportedPrefix"]
            .as_str()
            .is_some_and(|prefix| prefix.starts_with("exports/one")
                && prefix.len() > "exports/one".len()),
        "case {case:?}: a completed export reports the full prefix it wrote under, got {export}"
    );

    // The delete is addressed with the creation timestamp the reader reported; a value naming
    // another instant deletes nothing.
    let creation_timestamp = details["SnapshotCreationTimestamp"].clone();
    let stamp = creation_timestamp
        .as_f64()
        .unwrap_or_else(|| panic!("case {case:?}: expected a creation timestamp, got {details}"));
    let (status, stale) = call(
        &router,
        "DeleteSnapshot",
        &json!({"SnapshotName": snapshot, "SnapshotCreationTimestamp": stamp - 60.0}),
    )
    .await;
    assert_eq!(
        (status, answer_code(&stale).as_str()),
        (400, "InvalidArgumentException"),
        "case {case:?}: a delete carrying another instant is refused, got {status} {stale}"
    );
    let (status, untouched) = describe_snapshot(&router, snapshot).await;
    assert_eq!(
        (
            status,
            untouched["SnapshotInfoDetails"]["SnapshotStatus"].as_str()
        ),
        (200, Some("READY")),
        "case {case:?}: the refused delete left the snapshot alone, got {status} {untouched}"
    );

    let (status, deleted) = call(
        &router,
        "DeleteSnapshot",
        &json!({
            "SnapshotName": snapshot,
            "SnapshotCreationTimestamp": creation_timestamp,
        }),
    )
    .await;
    assert_eq!(
        (
            status,
            deleted["SnapshotArn"].as_str(),
            deleted["SnapshotStatus"].as_str(),
            deleted["SnapshotOperationStatus"].as_str(),
        ),
        (
            200,
            Some(expected_arn.as_str()),
            Some("DELETING"),
            Some("IN_PROGRESS"),
        ),
        "case {case:?}: DeleteSnapshot answers with the transient statuses, got {status} \
         {deleted}"
    );
    await_snapshot_absent(&router, case, snapshot).await;
    let (status, listed) = call(&router, "ListSnapshots", &json!({})).await;
    assert_eq!(status, 200, "case {case:?}: ListSnapshots got {status} {listed}");
    assert_eq!(
        listed_names(&listed),
        Vec::<String>::new(),
        "case {case:?}: a deleted snapshot is listed nowhere, got {listed}"
    );
}

/// One application, and what `CreateSnapshot` on it earns.
struct ModeRow {
    case: &'static str,
    runtime: &'static str,
    /// The `SnapshotMode` its `ApplicationSnapshotConfiguration` selects, if any.
    mode: Option<&'static str>,
    /// Whether the create carries the inline code a startable application needs.
    code: bool,
    /// The `(status, error code)` the create earns; an empty code means it was accepted.
    create: (u16, &'static str),
    /// Whether the application is started before the snapshot is attempted.
    running: bool,
    /// The `(status, error code)` `CreateSnapshot` then earns.
    snapshot: (u16, &'static str),
}

/// Which applications take an enhanced snapshot follows the runtime they run and the mode their
/// configuration selected — on both sides of the published Apache Flink 1.20 bound, and for
/// each way a configuration can decide the mode.
#[tokio::test]
async fn the_enhanced_snapshot_mode_gate_decides_which_applications_take_one() {
    let rows: [ModeRow; 7] = [
        ModeRow {
            case: "the earliest runtime that takes enhanced snapshots",
            runtime: AT_THE_BOUND,
            mode: Some(ENHANCED),
            code: true,
            create: (200, ""),
            running: true,
            snapshot: (200, ""),
        },
        ModeRow {
            case: "a runtime generation above the bound",
            runtime: ABOVE_THE_BOUND,
            mode: Some(ENHANCED),
            code: true,
            create: (200, ""),
            running: true,
            snapshot: (200, ""),
        },
        ModeRow {
            case: "an eligible application that is not running",
            runtime: AT_THE_BOUND,
            mode: Some(ENHANCED),
            code: true,
            create: (200, ""),
            running: false,
            snapshot: (400, "ResourceInUseException"),
        },
        ModeRow {
            case: "one generation below the bound selecting the enhanced mode",
            runtime: BELOW_THE_BOUND,
            mode: Some(ENHANCED),
            code: true,
            create: (400, "UnsupportedOperationException"),
            running: false,
            snapshot: (0, ""),
        },
        ModeRow {
            case: "one generation below the bound on the mode it can run",
            runtime: BELOW_THE_BOUND,
            mode: Some(DEFAULT_SNAPSHOT_MODE),
            code: true,
            create: (200, ""),
            running: true,
            snapshot: (400, "UnsupportedOperationException"),
        },
        ModeRow {
            case: "an eligible runtime left on the mode of an application that selected none",
            runtime: AT_THE_BOUND,
            mode: None,
            code: true,
            create: (200, ""),
            running: true,
            snapshot: (400, "InvalidApplicationConfigurationException"),
        },
        ModeRow {
            case: "the Studio-notebook family, which runs no Apache Flink job of its own",
            runtime: current_notebook_runtime(),
            mode: Some(ENHANCED),
            code: false,
            create: (400, "UnsupportedOperationException"),
            running: false,
            snapshot: (0, ""),
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let application = format!("mode-gate-{index}");
        let (status, created) = call(
            &router,
            "CreateApplication",
            &create_selecting_mode(&application, row.runtime, row.mode, row.code),
        )
        .await;
        assert_eq!(
            (status, answer_code(&created).as_str()),
            row.create,
            "case {case:?}: expected CreateApplication to answer {:?}, got {status} {created}",
            row.create
        );
        if row.create.0 != 200 {
            continue;
        }

        // An accepted create describes the mode the application now takes its snapshots under.
        assert_eq!(
            snapshot_description(&created["ApplicationDetail"])["SnapshotMode"].as_str(),
            Some(row.mode.unwrap_or(DEFAULT_SNAPSHOT_MODE)),
            "case {case:?}: expected the described mode to be the one in force, got {created}"
        );

        if row.running {
            let (status, started) = call(
                &router,
                "StartApplication",
                &json!({"ApplicationName": application}),
            )
            .await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartApplication got {status} {started}"
            );
            await_status(&router, case, &application, "RUNNING").await;
        }

        let snapshot = format!("mode-gate-snapshot-{index}");
        let (status, taken) = call(
            &router,
            "CreateSnapshot",
            &json!({"ApplicationName": application, "SnapshotName": snapshot}),
        )
        .await;
        assert_eq!(
            (status, answer_code(&taken).as_str()),
            row.snapshot,
            "case {case:?}: expected CreateSnapshot to answer {:?}, got {status} {taken}",
            row.snapshot
        );

        // A refused snapshot wrote nothing: the name it asked for names no snapshot.
        if row.snapshot.0 == 200 {
            await_snapshot_status(&router, case, &snapshot, "READY").await;
        } else {
            let (status, absent) = describe_snapshot(&router, &snapshot).await;
            assert_eq!(
                (status, answer_code(&absent).as_str()),
                (400, "ResourceNotFoundException"),
                "case {case:?}: a refused CreateSnapshot stores nothing, got {status} {absent}"
            );
        }
    }
}

/// One update of an application's snapshot mode.
struct UpdateRow {
    case: &'static str,
    runtime: &'static str,
    /// The mode the create selected, if any.
    created_with: Option<&'static str>,
    /// The `SnapshotModeUpdate` the update names.
    update_to: &'static str,
    /// The `RuntimeEnvironmentUpdate` the update names, if any.
    runtime_update: Option<&'static str>,
    /// The `(status, error code)` the update earns.
    answered: (u16, &'static str),
    /// The mode in force afterwards.
    in_force: &'static str,
}

/// The snapshot mode an update moves an application to goes in ONE direction, and only onto a
/// runtime that runs it: the runtime judged is the one the update moves the application to.
#[tokio::test]
async fn an_update_moves_the_snapshot_mode_forward_only() {
    let rows: [UpdateRow; 5] = [
        UpdateRow {
            case: "an eligible runtime moving onto the enhanced mode",
            runtime: AT_THE_BOUND,
            created_with: None,
            update_to: ENHANCED,
            runtime_update: None,
            answered: (200, ""),
            in_force: ENHANCED,
        },
        UpdateRow {
            case: "a runtime below the bound moving onto the enhanced mode",
            runtime: BELOW_THE_BOUND,
            created_with: None,
            update_to: ENHANCED,
            runtime_update: None,
            answered: (400, "UnsupportedOperationException"),
            in_force: DEFAULT_SNAPSHOT_MODE,
        },
        UpdateRow {
            case: "an update that moves the runtime onto an eligible one at the same time",
            runtime: BELOW_THE_BOUND,
            created_with: None,
            update_to: ENHANCED,
            runtime_update: Some(AT_THE_BOUND),
            answered: (200, ""),
            in_force: ENHANCED,
        },
        UpdateRow {
            case: "an application on the enhanced mode asked to go back",
            runtime: AT_THE_BOUND,
            created_with: Some(ENHANCED),
            update_to: DEFAULT_SNAPSHOT_MODE,
            runtime_update: None,
            answered: (400, "UnsupportedOperationException"),
            in_force: ENHANCED,
        },
        UpdateRow {
            case: "an application on the enhanced mode asked for the mode it is already on",
            runtime: AT_THE_BOUND,
            created_with: Some(ENHANCED),
            update_to: ENHANCED,
            runtime_update: None,
            answered: (200, ""),
            in_force: ENHANCED,
        },
    ];

    let router = router();
    for (index, row) in rows.iter().enumerate() {
        let case = row.case;
        let name = format!("mode-update-{index}");
        let (status, created) = call(
            &router,
            "CreateApplication",
            &create_selecting_mode(&name, row.runtime, row.created_with, true),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );

        let mut update = json!({
            "ApplicationName": name,
            "CurrentApplicationVersionId": 1,
            "ApplicationConfigurationUpdate": {
                "ApplicationSnapshotConfigurationUpdate": {
                    "SnapshotsEnabledUpdate": true,
                    "SnapshotModeUpdate": row.update_to,
                },
            },
        });
        if let Some(runtime) = row.runtime_update {
            update["RuntimeEnvironmentUpdate"] = json!(runtime);
        }
        let (status, answered) = call(&router, "UpdateApplication", &update).await;
        assert_eq!(
            (status, answer_code(&answered).as_str()),
            row.answered,
            "case {case:?}: expected UpdateApplication to answer {:?}, got {status} {answered}",
            row.answered
        );

        // Whatever the update earned, the mode a reader reports is the one in force — a refused
        // update left the application as it stood.
        let described = describe(&router, &name).await;
        assert_eq!(
            snapshot_description(&described)["SnapshotMode"].as_str(),
            Some(row.in_force),
            "case {case:?}: expected the mode in force to be {:?}, got {described}",
            row.in_force
        );
    }
}

/// Restoring from an enhanced snapshot is a capability of an application that TAKES enhanced
/// snapshots: a start naming a `SnapshotArn` for one on the legacy mode names a parameter that
/// application does not support, and the same start is served for one on the enhanced mode.
#[tokio::test]
async fn a_start_restoring_from_a_snapshot_arn_needs_the_enhanced_mode() {
    // (case, the mode the application selected, the (status, error code) the start earns)
    let rows: [(&str, Option<&str>, (u16, &str)); 3] = [
        (
            "an application that selected no mode",
            None,
            (400, "UnsupportedOperationException"),
        ),
        (
            "an application on the legacy mode",
            Some(DEFAULT_SNAPSHOT_MODE),
            (400, "UnsupportedOperationException"),
        ),
        ("an application on the enhanced mode", Some(ENHANCED), (200, "")),
    ];

    let router = router();
    for (index, (case, mode, expected)) in rows.into_iter().enumerate() {
        let name = format!("restore-arn-{index}");
        let (status, created) = call(
            &router,
            "CreateApplication",
            &create_selecting_mode(&name, AT_THE_BOUND, mode, true),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateApplication got {status} {created}"
        );

        let (status, started) = call(
            &router,
            "StartApplication",
            &json!({
                "ApplicationName": name,
                "RunConfiguration": {
                    "ApplicationRestoreConfiguration": {
                        "ApplicationRestoreType": "RESTORE_FROM_CUSTOM_SNAPSHOT",
                        "SnapshotArn": format!(
                            "arn:aws:kinesisanalytics:us-east-1:{ACCOUNT}:snapshot/some-snapshot"
                        ),
                    },
                },
            }),
        )
        .await;
        assert_eq!(
            (status, answer_code(&started).as_str()),
            expected,
            "case {case:?}: expected StartApplication to answer {expected:?}, got {status} \
             {started}"
        );
    }
}

/// `ListSnapshots` reports the account's snapshots newest first, admits one filter at a time,
/// and pages a sequence without losing or repeating an entry.
#[tokio::test]
async fn list_snapshots_orders_filters_and_pages_the_accounts_snapshots() {
    let case = "the account's snapshots";
    let router = router();

    // One snapshot taken of an application and four imported ones, so every filter the model
    // declares has both a matching and a non-matching entry to choose between.
    let application = "list-source-app";
    running_application_selecting(&router, case, application, AT_THE_BOUND, Some(ENHANCED)).await;
    let created = "list-created-snapshot";
    let (status, taken) = call(
        &router,
        "CreateSnapshot",
        &json!({"ApplicationName": application, "SnapshotName": created}),
    )
    .await;
    assert_eq!(status, 200, "case {case:?}: CreateSnapshot got {status} {taken}");
    let imported = [
        "list-imported-a",
        "list-imported-b",
        "list-imported-c",
        "list-imported-d",
    ];
    for name in imported {
        let (status, answered) = call(&router, "ImportSnapshot", &import_body(name)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: ImportSnapshot({name:?}) got {status} {answered}"
        );
    }
    for name in imported.iter().chain(std::iter::once(&created)) {
        await_snapshot_status(&router, case, name, "READY").await;
    }

    // The whole sequence every assertion below is taken against: descending on creation
    // timestamp, and the same on every call.
    let (status, all) = call(&router, "ListSnapshots", &json!({})).await;
    assert_eq!(status, 200, "case {case:?}: ListSnapshots got {status} {all}");
    let order = listed_names(&all);
    assert_eq!(
        order.len(),
        5,
        "case {case:?}: expected every snapshot of the account, got {all}"
    );
    let stamps: Vec<i64> = all["SnapshotInfoList"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|snapshot| snapshot["SnapshotCreationTimestamp"].as_f64().unwrap_or(0.0) as i64)
        .collect();
    assert!(
        stamps.windows(2).all(|pair| pair[0] >= pair[1]),
        "case {case:?}: expected creation timestamps in descending order, got {stamps:?}"
    );
    let (_, again) = call(&router, "ListSnapshots", &json!({})).await;
    assert_eq!(
        listed_names(&again),
        order,
        "case {case:?}: expected the same sequence on a second call, got {again}"
    );

    // (case, the filter, the names it admits)
    let filters: [(&str, Value, Vec<&str>); 6] = [
        (
            "the application a snapshot was taken of",
            json!({"SourceApplicationName": application}),
            vec![created],
        ),
        (
            "an application that is the source of none",
            json!({"SourceApplicationName": "list-other-app"}),
            Vec::new(),
        ),
        (
            "the operation that created each snapshot",
            json!({"SnapshotInitiatorOperation": "ImportSnapshot"}),
            imported.to_vec(),
        ),
        (
            "the initiator every snapshot a caller asked for carries",
            json!({"SnapshotInitiator": "USER"}),
            order.iter().map(String::as_str).collect(),
        ),
        (
            "the status every settled snapshot is in",
            json!({"SnapshotStatus": "READY"}),
            order.iter().map(String::as_str).collect(),
        ),
        (
            "a status none of them is in",
            json!({"SnapshotStatus": "DELETING"}),
            Vec::new(),
        ),
    ];
    for (filtered, filter, expected) in filters {
        let (status, page) = call(&router, "ListSnapshots", &filter).await;
        assert_eq!(
            status, 200,
            "case {filtered:?}: ListSnapshots({filter}) got {status} {page}"
        );
        let mut answered = listed_names(&page);
        answered.sort();
        let mut wanted: Vec<String> = expected.iter().map(|name| name.to_string()).collect();
        wanted.sort();
        assert_eq!(
            answered, wanted,
            "case {filtered:?}: expected {wanted:?}, got {answered:?}"
        );
    }

    // Two filters at once is refused, whichever two they are.
    let (status, both) = call(
        &router,
        "ListSnapshots",
        &json!({"SnapshotInitiator": "USER", "SnapshotStatus": "READY"}),
    )
    .await;
    assert_eq!(
        (status, answer_code(&both).as_str()),
        (400, "InvalidArgumentException"),
        "case {case:?}: one filter at a time, got {status} {both}"
    );

    // Paging the same sequence at each size yields it back entry for entry.
    for limit in [1_usize, 2, 5] {
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut body = json!({"Limit": limit});
            if let Some(token) = token.as_deref() {
                body["NextToken"] = json!(token);
            }
            let (status, page) = call(&router, "ListSnapshots", &body).await;
            assert_eq!(
                status, 200,
                "case \"paging at {limit}\": ListSnapshots({body}) got {status} {page}"
            );
            let names = listed_names(&page);
            assert!(
                names.len() <= limit,
                "case \"paging at {limit}\": a page holds at most the limit, got {page}"
            );
            walked.extend(names);
            token = page["NextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
            assert!(
                walked.len() <= order.len(),
                "case \"paging at {limit}\": the walk must terminate, got {walked:?}"
            );
        }
        assert_eq!(
            walked, order,
            "case \"paging at {limit}\": expected the whole sequence, in order and once each"
        );
    }

    // A token naming no position in the sequence resumes nothing.
    let (status, bogus) = call(
        &router,
        "ListSnapshots",
        &json!({"NextToken": "not-a-token-this-service-issued"}),
    )
    .await;
    assert_eq!(
        (status, answer_code(&bogus).as_str()),
        (400, "InvalidArgumentException"),
        "case {case:?}: an unissued token is refused, got {status} {bogus}"
    );
}

/// An imported snapshot records where its data came from and is held to the same name rules a
/// created one is: an enhanced snapshot's name is unique across the account, and the names the
/// service reserves for its own snapshots are not a caller's to take.
#[tokio::test]
async fn an_imported_snapshot_records_its_source_and_is_held_to_the_name_rules() {
    let case = "a snapshot imported from Amazon S3";
    let router = router();

    let snapshot = "imported-snapshot";
    let mut body = import_body(snapshot);
    body["SnapshotDescription"] = json!("a savepoint written by Apache Flink");
    body["DestinationEncryptionConfiguration"] = json!({
        "KeyType": "CUSTOMER_MANAGED_KEY",
        "KeyId": CUSTOMER_KEY,
    });
    let (status, imported) = call(&router, "ImportSnapshot", &body).await;
    assert_eq!(
        (
            status,
            imported["SnapshotStatus"].as_str(),
            imported["SnapshotOperationStatus"].as_str(),
        ),
        (200, Some("CREATING"), Some("IN_PROGRESS")),
        "case {case:?}: ImportSnapshot answers with the transient statuses, got {status} \
         {imported}"
    );
    let operation_id = imported["SnapshotOperationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();

    let details = await_snapshot_status(&router, case, snapshot, "READY").await;
    assert_eq!(
        (
            details["SnapshotInitiator"].as_str(),
            details["SnapshotInitiatorOperation"].as_str(),
            details["SnapshotDescription"].as_str(),
            details["SnapshotFormat"].as_str(),
            details["SnapshotSourceDescription"]["S3PrefixLocation"]["BucketARN"].as_str(),
            details["SnapshotSourceDescription"]["S3PrefixLocation"]["KeyPrefix"].as_str(),
            details["SnapshotEncryptionConfigurationDescription"]["KeyType"].as_str(),
            details["SnapshotEncryptionConfigurationDescription"]["KeyId"].as_str(),
        ),
        (
            Some("USER"),
            Some("ImportSnapshot"),
            Some("a savepoint written by Apache Flink"),
            Some("CANONICAL"),
            Some(SNAPSHOT_BUCKET),
            Some(SNAPSHOT_PREFIX),
            Some("CUSTOMER_MANAGED_KEY"),
            Some(CUSTOMER_KEY),
        ),
        "case {case:?}: expected the imported snapshot to be reported as written, got {details}"
    );
    assert!(
        details.get("RuntimeEnvironment").is_none(),
        "case {case:?}: an imported snapshot carries no application runtime, got {details}"
    );
    assert!(
        details["SnapshotSourceDescription"]
            .get("SourceApplicationDescription")
            .is_none(),
        "case {case:?}: an imported snapshot names no source application, got {details}"
    );

    // Its import operation records where the data was read from and under which keys.
    let operation =
        await_operation_status(&router, case, snapshot, &operation_id, "SUCCESSFUL").await;
    assert_eq!(
        (
            operation["SnapshotOperation"].as_str(),
            operation["SnapshotSourceLocationDescription"]["LocationType"].as_str(),
            operation["SnapshotSourceLocationDescription"]["S3PrefixLocation"]["KeyPrefix"]
                .as_str(),
            operation["SourceEncryptionConfigurationDescription"]["KeyType"].as_str(),
            operation["DestinationEncryptionConfigurationDescription"]["KeyType"].as_str(),
        ),
        (
            Some("ImportSnapshot"),
            Some("S3"),
            Some(SNAPSHOT_PREFIX),
            Some("AWS_OWNED_KEY"),
            Some("CUSTOMER_MANAGED_KEY"),
        ),
        "case {case:?}: expected the import's recorded locations and keys, got {operation}"
    );

    // (case, the name the import asks for, the status and code it earns)
    let names: [(&str, &str, (u16, &str)); 4] = [
        (
            "a name this account already holds a snapshot under",
            snapshot,
            (400, "ResourceInUseException"),
        ),
        (
            "a name in the space the service reserves for its own snapshots",
            "SYSTEMSNAPSHOT-mine",
            (400, "InvalidArgumentException"),
        ),
        (
            "a name whose prefix only resembles the reserved one",
            "SYSTEM-mine",
            (200, ""),
        ),
        ("a name of its own", "imported-second", (200, "")),
    ];
    for (named, name, expected) in names {
        let (status, answered) = call(&router, "ImportSnapshot", &import_body(name)).await;
        assert_eq!(
            (status, answer_code(&answered).as_str()),
            expected,
            "case {named:?}: expected {expected:?}, got {status} {answered}"
        );
    }
}
