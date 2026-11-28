// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `StartTableMigration`, `SyncTableMigration` and
//! `CompleteTableMigration`.
//!
//! The rule under test: starting a migration creates the destination table in the
//! namespace the request names and records a migration whose every reported
//! member is read back out of that record — the source, the role, the review
//! flag, the report configuration, and the maintenance settings and migration
//! policies with every member the request left out settled at the model's own
//! default. The migration is accepted in the state the model documents for an
//! accepted request and reaches its later states on the CLOCK, never on a read.
//! Once it has ended the migration can be completed, after which it can neither
//! be completed nor synced again.
//!
//! The table spans what the rule generalises over: the destination coordinates,
//! whether the request asks for a plan review, which maintenance settings and
//! migration policies it names, and which report prefixes it names.

use serde_json::{json, Value};

use super::harness::{
    await_migration_state, migration_role_arn, served, start_migration_request,
    sync_migration_request, Emulator, Reply, MIGRATION_REPORT_BUCKET,
};

/// The fixed sentences this service states the classes below with.
const NO_MIGRATION: &str = "The specified migration does not exist.";
const NO_NAMESPACE: &str = "The specified namespace does not exist.";
const NO_BUCKET: &str = "The specified bucket does not exist.";
const TABLE_EXISTS: &str = "A table with an identical name already exists in the namespace.";
const ALREADY_COMPLETE: &str = "The migration of the specified table is already complete.";
const NOT_ENDED: &str = "The most recent migration of the specified table has not ended.";
const IN_PROGRESS: &str = "A migration of the specified table is already in progress.";
const ADDRESSING: &str = "Invalid input: Must provide either migrationId alone OR tableARN alone";

/// The maintenance settings and migration policies a migration runs under when
/// the request names none of them — the model's own defaults for each member
/// (`smithy_query(query="shape",
/// shape="MigrationCompactionSettings,MigrationSnapshotManagementSettings,MigrationPolicies")`).
fn default_data_migration() -> Value {
    json!({
        "maintenanceSettings": {
            "icebergCompaction": { "targetFileSizeMB": 512, "strategy": "binpack" },
            "icebergSnapshotManagement": { "minSnapshotsToKeep": 3, "maxSnapshotAgeHours": 120 }
        },
        "migrationPolicies": {
            "unmatchedSourceFieldsPolicy": "fail",
            "incompatibleValuesPolicy": "fail"
        }
    })
}

/// Assert `reply` is the modelled 202 of a call that ACCEPTED a migration, and
/// hand it back for the case to read.
fn accepted(reply: Reply, operation: &str, case: &str) -> Reply {
    assert_eq!(
        reply.status, 202,
        "case {case:?}: {operation} answers its modelled 202, got {status} {body}",
        status = reply.status,
        body = reply.body
    );
    reply
}

/// Create a table bucket holding `namespaces`, and answer its ARN.
async fn create_bucket(
    emulator: &Emulator,
    region: &str,
    bucket: &str,
    namespaces: &[&str],
) -> String {
    let created = served(
        emulator
            .create_table_bucket(region, &json!({ "name": bucket }))
            .await,
        "CreateTableBucket",
        bucket,
    );
    let bucket_arn = created.text("arn");
    for namespace in namespaces {
        served(
            emulator
                .create_namespace(region, &bucket_arn, &json!({ "namespace": [namespace] }))
                .await,
            "CreateNamespace",
            namespace,
        );
    }
    bucket_arn
}

/// One row of the start table.
struct StartCase {
    case: &'static str,
    namespace: &'static str,
    name: &'static str,
    /// What the request carries on top of the members every start needs.
    extra: Value,
    /// The whole `dataMigration` overview the read must report.
    reported_data_migration: Value,
    /// The prefix each report must be reported under, in the order
    /// `MigrationOutputOverview` names them.
    reported_prefixes: [Option<&'static str>; 3],
    review: bool,
}

/// The rows: they differ along the destination coordinates, the review flag, the
/// maintenance settings and policies the request names (none, some, all), and the
/// report prefixes it names (none, one, all three).
fn start_cases() -> Vec<StartCase> {
    vec![
        StartCase {
            case: "nothing but the members every start needs",
            namespace: "sales",
            name: "orders",
            extra: json!({}),
            reported_data_migration: default_data_migration(),
            reported_prefixes: [None, None, None],
            review: false,
        },
        StartCase {
            case: "a plan review, one report prefix and the compaction settings",
            namespace: "sales",
            name: "orders_reviewed",
            extra: json!({
                "reviewMigrationPlan": true,
                "dataMigration": {
                    "maintenanceSettings": {
                        "icebergCompaction": { "targetFileSizeMB": 64, "strategy": "binpack" }
                    }
                },
                "migrationOutput": {
                    "bucket": MIGRATION_REPORT_BUCKET,
                    "prefixes": { "tableCompatibilityReport": "compat/" }
                }
            }),
            reported_data_migration: json!({
                "maintenanceSettings": {
                    "icebergCompaction": { "targetFileSizeMB": 64, "strategy": "binpack" },
                    "icebergSnapshotManagement": {
                        "minSnapshotsToKeep": 3,
                        "maxSnapshotAgeHours": 120
                    }
                },
                "migrationPolicies": {
                    "unmatchedSourceFieldsPolicy": "fail",
                    "incompatibleValuesPolicy": "fail"
                }
            }),
            reported_prefixes: [Some("compat/"), None, None],
            review: true,
        },
        StartCase {
            case: "every setting, every policy and every report prefix",
            namespace: "inventory",
            name: "stock",
            extra: json!({
                "reviewMigrationPlan": false,
                "dataMigration": {
                    "maintenanceSettings": {
                        "icebergCompaction": { "targetFileSizeMB": 256, "strategy": "binpack" },
                        "icebergSnapshotManagement": {
                            "minSnapshotsToKeep": 5,
                            "maxSnapshotAgeHours": 1
                        }
                    },
                    "migrationPolicies": {
                        "unmatchedSourceFieldsPolicy": "drop",
                        "incompatibleValuesPolicy": "setNull"
                    }
                },
                "migrationOutput": {
                    "bucket": MIGRATION_REPORT_BUCKET,
                    "prefixes": {
                        "tableCompatibilityReport": "a/",
                        "objectCompatibilityReport": "b/",
                        "objectMigrationReport": "c/"
                    }
                }
            }),
            reported_data_migration: json!({
                "maintenanceSettings": {
                    "icebergCompaction": { "targetFileSizeMB": 256, "strategy": "binpack" },
                    "icebergSnapshotManagement": {
                        "minSnapshotsToKeep": 5,
                        "maxSnapshotAgeHours": 1
                    }
                },
                "migrationPolicies": {
                    "unmatchedSourceFieldsPolicy": "drop",
                    "incompatibleValuesPolicy": "setNull"
                }
            }),
            reported_prefixes: [Some("a/"), Some("b/"), Some("c/")],
            review: false,
        },
    ]
}

/// `base` with every member of `extra` applied over it, so a row states only the
/// members it cares about.
fn request_with(base: Value, extra: &Value) -> Value {
    let (Value::Object(mut members), Value::Object(extra)) = (base, extra.clone()) else {
        panic!("a request and its overrides are both objects");
    };
    for (name, value) in extra {
        members.insert(name, value);
    }
    Value::Object(members)
}

/// RULE: the start creates the destination table and records a migration whose
/// every reported member is read back out of that record.
#[tokio::test]
async fn a_started_migration_creates_the_destination_table_and_is_read_back_whole() {
    let region = "us-east-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(
        &emulator,
        region,
        "migration-start",
        &["sales", "inventory"],
    )
    .await;

    for row in start_cases() {
        let case = row.case;
        let request = request_with(
            start_migration_request(&bucket_arn, row.namespace, row.name),
            &row.extra,
        );
        let started = emulator.start_table_migration(region, &request).await;
        assert_eq!(
            started.status, 202,
            "case {case:?}: StartTableMigration answers its modelled 202, got {status} {body}",
            status = started.status,
            body = started.body
        );
        let migration_id = started.text("migrationId");
        // `MigrationId` is `@length(1, 64)`, `@pattern(^[a-z0-9-]+$)`.
        assert!(
            (1..=64).contains(&migration_id.len())
                && migration_id
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
            "case {case:?}: the minted migrationId must be of the modelled form, got \
             {migration_id:?}"
        );
        // "new - S3 Tables accepted the request": the state the request is
        // accepted in, before the clock has moved the migration anywhere.
        assert_eq!(
            started.text("state"),
            "new",
            "case {case:?}: the start reports the state the request was accepted in, got {body}",
            body = started.body
        );

        // The destination table really stands in the bucket: a read of this
        // service answers for it, under the coordinates the migration named.
        let table = served(
            emulator
                .get_table(region, &bucket_arn, row.namespace, row.name)
                .await,
            "GetTable",
            case,
        );
        let table_arn = table.text("tableARN");
        assert_eq!(
            (table.text("name"), table.text("type")),
            (row.name.to_string(), "customer".to_string()),
            "case {case:?}: the migrated table is the account's own table, got {body}",
            body = table.body
        );
        assert_eq!(
            table.optional_text("managedByService"),
            None,
            "case {case:?}: no service manages the table a migration creates, got {body}",
            body = table.body
        );

        // The migration, read back by its id and by its destination table: the
        // two addressing forms resolve to the same record.
        for (form, query) in [
            ("by id", vec![("migrationId", migration_id.clone())]),
            ("by destination table", vec![("tableARN", table_arn.clone())]),
        ] {
            let read = served(
                emulator.get_table_migration(region, &query).await,
                "GetTableMigration",
                case,
            );
            assert_eq!(
                read.text("migrationId"),
                migration_id,
                "case {case:?} ({form}): the read answers the migration addressed, got {body}",
                body = read.body
            );
            assert_eq!(
                (
                    read.text("tableBucketARN"),
                    read.text("namespace"),
                    read.text("name"),
                    read.text("tableARN"),
                ),
                (
                    bucket_arn.clone(),
                    row.namespace.to_string(),
                    row.name.to_string(),
                    table_arn.clone(),
                ),
                "case {case:?} ({form}): the read names the destination the start named, got \
                 {body}",
                body = read.body
            );
            assert_eq!(
                (
                    read.text("type"),
                    read.text("sourceType"),
                    read.text("roleARN"),
                ),
                (
                    "start".to_string(),
                    "glueTable".to_string(),
                    migration_role_arn(),
                ),
                "case {case:?} ({form}): the read reports what started the migration, got {body}",
                body = read.body
            );
            assert_eq!(
                read.body.get("reviewMigrationPlan"),
                Some(&Value::Bool(row.review)),
                "case {case:?} ({form}): the read reports the review the start asked for, got \
                 {body}",
                body = read.body
            );
            assert_eq!(
                read.body.get("source"),
                request.get("source"),
                "case {case:?} ({form}): the read reports the source the start named, got {body}",
                body = read.body
            );
            assert_eq!(
                read.body.get("dataMigration"),
                Some(&row.reported_data_migration),
                "case {case:?} ({form}): the read reports one whole configuration, with the \
                 model's default for every member the start left out, got {body}",
                body = read.body
            );
            let output = read
                .body
                .get("migrationOutput")
                .unwrap_or_else(|| panic!("case {case:?}: a read reports the report locations"));
            assert_eq!(
                output.get("bucket").and_then(Value::as_str),
                Some(MIGRATION_REPORT_BUCKET),
                "case {case:?} ({form}): the read names the bucket the reports go to, got \
                 {output}"
            );
            for (report, expected) in [
                "tableCompatibilityReport",
                "objectCompatibilityReport",
                "objectMigrationReport",
            ]
            .iter()
            .zip(row.reported_prefixes)
            {
                let overview = output
                    .get(report)
                    .unwrap_or_else(|| panic!("case {case:?}: {report} has an overview"));
                assert_eq!(
                    overview.get("prefix").and_then(Value::as_str),
                    expected,
                    "case {case:?} ({form}): {report} is reported under the prefix the start \
                     named, got {overview}"
                );
                // No report object exists until a migration writes one, so the
                // members that name the written object are absent.
                assert_eq!(
                    (overview.get("key"), overview.get("checksumCRC64NVME")),
                    (None, None),
                    "case {case:?} ({form}): a report nothing wrote names no object, got \
                     {overview}"
                );
            }
            // Planning has not finished, so the member that reports its outcome
            // is absent.
            assert_eq!(
                read.body.get("hasValidPlan"),
                None,
                "case {case:?} ({form}): a migration still before planning reports no \
                 hasValidPlan, got {body}",
                body = read.body
            );
        }

        // A second migration into the same destination has nowhere to create the
        // table, which is the conflict the start models.
        let again = emulator.start_table_migration(region, &request).await;
        assert_eq!(
            (again.status, again.message()),
            (409, TABLE_EXISTS.to_string()),
            "case {case:?}: a name a table already stands under is the modelled conflict, got \
             {status} {body}",
            status = again.status,
            body = again.body
        );
    }
}

/// RULE: a request that addresses a migration by neither whole form, and one that
/// names a resource the store does not hold, is refused — and refuses without
/// creating anything.
#[tokio::test]
async fn a_start_or_a_read_that_addresses_nothing_is_refused() {
    let region = "eu-central-1";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-refusals", &["sales"]).await;

    // The read's two addressing forms are whole and exclusive.
    for (case, query) in [
        ("neither member", vec![]),
        (
            "both members",
            vec![
                ("migrationId", "deadbeef".to_string()),
                (
                    "tableARN",
                    format!("{bucket_arn}/table/00000000-0000-4000-8000-000000000000"),
                ),
            ],
        ),
    ] {
        let read = emulator.get_table_migration(region, &query).await;
        assert_eq!(
            (read.status, read.message()),
            (400, ADDRESSING.to_string()),
            "case {case:?}: a read addressing no migration is refused, got {status} {body}",
            status = read.status,
            body = read.body
        );
    }

    // An id nothing is stored under, and a destination table no migration
    // migrated into, are each the one not-found this service states for a
    // migration.
    let plain = served(
        emulator
            .create_table(
                region,
                &bucket_arn,
                "sales",
                &json!({ "name": "hand_made", "format": "ICEBERG" }),
            )
            .await,
        "CreateTable",
        "hand-made table",
    );
    let plain_arn = plain.text("tableARN");
    for (case, query) in [
        ("an unknown id", vec![("migrationId", "absent".to_string())]),
        (
            "a table no migration created",
            vec![("tableARN", plain_arn.clone())],
        ),
    ] {
        let read = emulator.get_table_migration(region, &query).await;
        assert_eq!(
            (read.status, read.message()),
            (404, NO_MIGRATION.to_string()),
            "case {case:?}: a migration the store does not hold is the modelled not-found, got \
             {status} {body}",
            status = read.status,
            body = read.body
        );
    }

    // The namespace must already exist, and the bucket too.
    let into_absent_namespace = emulator
        .start_table_migration(
            region,
            &start_migration_request(&bucket_arn, "absent_namespace", "orders"),
        )
        .await;
    assert_eq!(
        (
            into_absent_namespace.status,
            into_absent_namespace.message()
        ),
        (404, NO_NAMESPACE.to_string()),
        "case \"an absent namespace\": the migration creates the table, not the namespace, got \
         {status} {body}",
        status = into_absent_namespace.status,
        body = into_absent_namespace.body
    );
    let absent_bucket = format!("arn:aws:s3tables:{region}:000000000000:bucket/migration-absent");
    let into_absent_bucket = emulator
        .start_table_migration(
            region,
            &start_migration_request(&absent_bucket, "sales", "orders"),
        )
        .await;
    assert_eq!(
        (into_absent_bucket.status, into_absent_bucket.message()),
        (404, NO_BUCKET.to_string()),
        "case \"an absent bucket\": a bucket the store does not hold is the modelled not-found, \
         got {status} {body}",
        status = into_absent_bucket.status,
        body = into_absent_bucket.body
    );

    // A refused start creates no table: the namespace still holds only the table
    // the create above put in it.
    let listed = served(
        emulator.list_tables(region, &bucket_arn, &[]).await,
        "ListTables",
        "after refusals",
    );
    let names: Vec<String> = listed
        .list("tables")
        .iter()
        .filter_map(|row| {
            row.get("name")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        names,
        vec!["hand_made".to_string()],
        "case \"after refusals\": a refused start must create no table, got {names:?}"
    );
}

/// RULE: a migration reaches its later states on the clock; once it has ended the
/// table's migration can be completed, and after that neither completed nor
/// synced again.
#[tokio::test]
async fn a_migration_runs_to_succeeded_and_is_then_completed_once() {
    let region = "us-west-2";
    let case = "run to succeeded, then complete";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-complete", &["sales"]).await;

    let started = accepted(
        emulator
            .start_table_migration(
                region,
                &start_migration_request(&bucket_arn, "sales", "orders"),
            )
            .await,
        "StartTableMigration",
        case,
    );
    let migration_id = started.text("migrationId");
    let table_arn = served(
        emulator.get_table(region, &bucket_arn, "sales", "orders").await,
        "GetTable",
        case,
    )
    .text("tableARN");

    // A migration that has not ended cannot be completed.
    let too_early = emulator.complete_table_migration(region, &table_arn).await;
    assert_eq!(
        (too_early.status, too_early.message()),
        (409, NOT_ENDED.to_string()),
        "case {case:?}: a migration still running cannot be completed, got {status} {body}",
        status = too_early.status,
        body = too_early.body
    );

    // Wait for the CONDITION, not for a span.
    let ended = await_migration_state(&emulator, region, &migration_id, "succeeded", case).await;
    assert_eq!(
        ended.body.get("hasValidPlan"),
        Some(&Value::Bool(true)),
        "case {case:?}: a migration that ran its plan reports that planning produced one, got \
         {body}",
        body = ended.body
    );
    // The instant the state was entered does not move because somebody asked
    // again.
    let again = served(
        emulator
            .get_table_migration(region, &[("migrationId", migration_id.clone())])
            .await,
        "GetTableMigration",
        case,
    );
    assert_eq!(
        again.body.get("stateUpdatedAt"),
        ended.body.get("stateUpdatedAt"),
        "case {case:?}: a read must not move the instant a state was entered, got {body}",
        body = again.body
    );

    let completed = emulator.complete_table_migration(region, &table_arn).await;
    assert_eq!(
        completed.status, 204,
        "case {case:?}: CompleteTableMigration answers its modelled 204, got {status} {body}",
        status = completed.status,
        body = completed.body
    );
    let repeated = emulator.complete_table_migration(region, &table_arn).await;
    assert_eq!(
        (repeated.status, repeated.message()),
        (409, ALREADY_COMPLETE.to_string()),
        "case {case:?}: completing a table twice is the modelled conflict, got {status} {body}",
        status = repeated.status,
        body = repeated.body
    );
    let synced = emulator
        .sync_table_migration(region, &sync_migration_request(&table_arn))
        .await;
    assert_eq!(
        (synced.status, synced.message()),
        (409, ALREADY_COMPLETE.to_string()),
        "case {case:?}: a completed table cannot be synced again, got {status} {body}",
        status = synced.status,
        body = synced.body
    );

    // Completing the migration leaves the table itself exactly where it is.
    let table = served(
        emulator.get_table(region, &bucket_arn, "sales", "orders").await,
        "GetTable",
        case,
    );
    assert_eq!(
        table.text("tableARN"),
        table_arn,
        "case {case:?}: completing a migration must not move the table, got {body}",
        body = table.body
    );
}

/// RULE: a sync records a further migration into the same table, under the source
/// and the settings of the table's migration, and only when no migration of that
/// table is still running.
#[tokio::test]
async fn a_sync_carries_on_from_the_tables_migration() {
    let region = "ap-northeast-1";
    let case = "sync after a migration that ended";
    let emulator = Emulator::new();
    let bucket_arn = create_bucket(&emulator, region, "migration-sync", &["sales"]).await;

    let started = accepted(
        emulator
            .start_table_migration(
                region,
                &start_migration_request(&bucket_arn, "sales", "orders"),
            )
            .await,
        "StartTableMigration",
        case,
    );
    let first_id = started.text("migrationId");
    let table_arn = served(
        emulator.get_table(region, &bucket_arn, "sales", "orders").await,
        "GetTable",
        case,
    )
    .text("tableARN");

    // "You can't sync a table while a start or sync is in progress for it."
    let too_early = emulator
        .sync_table_migration(region, &sync_migration_request(&table_arn))
        .await;
    assert_eq!(
        (too_early.status, too_early.message()),
        (409, IN_PROGRESS.to_string()),
        "case {case:?}: a sync while a migration runs is the modelled conflict, got {status} \
         {body}",
        status = too_early.status,
        body = too_early.body
    );
    // A table no migration migrated into has nothing to sync.
    let plain = served(
        emulator
            .create_table(
                region,
                &bucket_arn,
                "sales",
                &json!({ "name": "hand_made", "format": "ICEBERG" }),
            )
            .await,
        "CreateTable",
        case,
    );
    let plain_sync = emulator
        .sync_table_migration(region, &sync_migration_request(&plain.text("tableARN")))
        .await;
    assert_eq!(
        (plain_sync.status, plain_sync.message()),
        (404, NO_MIGRATION.to_string()),
        "case {case:?}: a table with no migration is the modelled not-found, got {status} {body}",
        status = plain_sync.status,
        body = plain_sync.body
    );

    let first = await_migration_state(&emulator, region, &first_id, "succeeded", case).await;
    let synced = emulator
        .sync_table_migration(region, &sync_migration_request(&table_arn))
        .await;
    assert_eq!(
        synced.status, 202,
        "case {case:?}: SyncTableMigration answers its modelled 202, got {status} {body}",
        status = synced.status,
        body = synced.body
    );
    let sync_id = synced.text("migrationId");
    assert_ne!(
        sync_id, first_id,
        "case {case:?}: a sync is a migration of its own, got {sync_id:?}"
    );
    assert_eq!(
        synced.text("state"),
        "new",
        "case {case:?}: the sync reports the state the request was accepted in, got {body}",
        body = synced.body
    );

    // Addressed by the destination table, the read answers the table's MOST
    // RECENT migration, which is the sync.
    let read = served(
        emulator
            .get_table_migration(region, &[("tableARN", table_arn.clone())])
            .await,
        "GetTableMigration",
        case,
    );
    assert_eq!(
        (read.text("migrationId"), read.text("type")),
        (sync_id.clone(), "sync".to_string()),
        "case {case:?}: the destination table's most recent migration is the sync, got {body}",
        body = read.body
    );
    // "A sync uses the source, destination and data migration settings of the
    // table's migration", which the sync request carries none of.
    assert_eq!(
        (
            read.body.get("source"),
            read.body.get("dataMigration"),
            read.body.get("tableARN"),
        ),
        (
            first.body.get("source"),
            first.body.get("dataMigration"),
            first.body.get("tableARN"),
        ),
        "case {case:?}: the sync carries the source and the settings of the table's migration, \
         got {body}",
        body = read.body
    );
}
