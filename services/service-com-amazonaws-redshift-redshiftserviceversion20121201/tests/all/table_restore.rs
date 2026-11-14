// SPDX-License-Identifier: CC0-1.0

//! Restoring ONE TABLE of a cluster snapshot back into the cluster, and reading
//! the restore requests that tracks it — end to end over the awsQuery wire with
//! the Redshift Data API mounted beside the control plane on one engine.
//!
//! The RULE these cases assert: `RestoreTableFromClusterSnapshot` "creates a new
//! table from a table in an Amazon Redshift cluster snapshot", so the new table
//! really holds the ROWS the snapshot's table held — not the rows the source
//! cluster has now — and the request that asked for it is readable through
//! `DescribeTableRestoreStatus` until it reports `SUCCEEDED`. A name the snapshot
//! or the cluster does not hold, and a new table name the cluster already holds,
//! are refused with the modelled `InvalidTableRestoreArgumentFault` and write
//! nothing.
//!
//! The rows differ along what the rule generalises over: which table, how many
//! rows, which schema the copy is created in, and which of the names resolves to
//! nothing. The control-plane harness and the Data API harness are shared with
//! [`crate::cluster_lifecycle`] and [`crate::snapshot_data`], which own them.

use serde_json::json;
use service_com_amazonaws_redshift_redshiftserviceversion20121201::state;

use crate::cluster_lifecycle::{all_text, Answer, Emulator};
use crate::snapshot_data::{create_cluster, create_snapshot, query, run_sql, submit_sql};

/// The database every cluster in this suite serves, and the one the tables live
/// in.
const DATABASE: &str = "devdb";

/// The members one row of a table sends beside the cluster and the snapshot.
type Params = Vec<(&'static str, &'static str)>;

/// One row of the restore table: the case, the restore's members, the read-back
/// and the rows it must answer.
type RestoreRow = (&'static str, Params, &'static str, serde_json::Value);

/// One row of the refusal table: the case, the restore's members, and the status
/// and code the request is refused with.
type RefusalRow = (&'static str, Params, u16, &'static str);

/// Assert one refusal: the status and the `<Code>` the model gives the fault.
fn assert_refused(case: &str, answer: &Answer, status: u16, code: &str) {
    assert_eq!(
        answer.status, status,
        "case {case}: expected status {status}, got {} with body {}",
        answer.status, answer.body
    );
    assert_eq!(
        answer.error_code().as_deref(),
        Some(code),
        "case {case}: expected error code {code:?}, got {:?} with body {}",
        answer.error_code(),
        answer.body
    );
}

/// Wait until the cluster is serving and the snapshot is available: a restore
/// needs both, and both are transitions the clock owes.
async fn settled(emulator: &Emulator, case: &str, cluster: &str, snapshot: &str) {
    emulator
        .poll_until(
            case,
            "DescribeClusters",
            &[("ClusterIdentifier", cluster)],
            |answer| answer.text("ClusterStatus").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
    emulator
        .poll_until(
            case,
            "DescribeClusterSnapshots",
            &[("SnapshotIdentifier", snapshot)],
            |answer| answer.text("Status").as_deref() == Some(state::STATUS_AVAILABLE),
        )
        .await;
}

/// `RestoreTableFromClusterSnapshot` with these members beside the cluster and
/// the snapshot every row names.
async fn restore_table(
    emulator: &Emulator,
    cluster: &str,
    snapshot: &str,
    extra: &[(&str, &str)],
) -> Answer {
    let mut params: Vec<(&str, &str)> = vec![
        ("ClusterIdentifier", cluster),
        ("SnapshotIdentifier", snapshot),
    ];
    params.extend_from_slice(extra);
    emulator.call("RestoreTableFromClusterSnapshot", &params).await
}

/// A restored table holds the rows the snapshot's table held, and the request is
/// readable until it reports success — for a table spanning the table's shape, the
/// number of rows and the schema the copy is created in.
#[tokio::test]
async fn a_restored_table_holds_the_rows_the_snapshot_captured() {
    let emulator = Emulator::with_data_api();
    let cluster = "table-restore-cluster";
    let snapshot = "table-restore-snapshot";
    create_cluster(&emulator, "a restored table", cluster, DATABASE).await;

    // Three tables, so the rows below differ along the data's shape and size and
    // not only its name: two columns and three rows, one column and one row, and
    // a table of a SCHEMA rather than of the default one.
    for sql in [
        "CREATE TABLE people (id INTEGER, name VARCHAR(16))",
        "INSERT INTO people (id, name) VALUES (1, 'ada'), (2, 'alan'), (3, 'grace')",
        "CREATE TABLE totals (total INTEGER)",
        "INSERT INTO totals (total) VALUES (42)",
        "CREATE SCHEMA analytics",
        "CREATE TABLE analytics.visits (day VARCHAR(10), hits INTEGER)",
        "INSERT INTO analytics.visits (day, hits) VALUES ('monday', 7), ('tuesday', 9)",
    ] {
        run_sql(&emulator, "seed the tables", cluster, DATABASE, sql).await;
    }

    create_snapshot(&emulator, "a restored table", cluster, snapshot).await;
    settled(&emulator, "a restored table", cluster, snapshot).await;

    // Written AFTER the snapshot: a restored table must NOT carry it, which is
    // what makes the copy a copy of the snapshot rather than of the cluster.
    run_sql(
        &emulator,
        "a write after the snapshot",
        cluster,
        DATABASE,
        "INSERT INTO people (id, name) VALUES (4, 'edsger')",
    )
    .await;

    // (case, the restore's members, the read-back, the rows it must answer)
    let rows: Vec<RestoreRow> = vec![
        (
            "a table of two columns and three rows",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("NewTableName", "people_restored"),
            ],
            "SELECT id, name FROM people_restored ORDER BY id",
            json!([
                [{"longValue": 1}, {"stringValue": "ada"}],
                [{"longValue": 2}, {"stringValue": "alan"}],
                [{"longValue": 3}, {"stringValue": "grace"}]
            ]),
        ),
        (
            "a table of one column and one row",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "totals"),
                ("NewTableName", "totals_restored"),
            ],
            "SELECT total FROM totals_restored",
            json!([[{"longValue": 42}]]),
        ),
        (
            "a table of a schema, restored into that schema",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceSchemaName", "analytics"),
                ("SourceTableName", "visits"),
                ("TargetSchemaName", "analytics"),
                ("NewTableName", "visits_restored"),
            ],
            "SELECT day, hits FROM analytics.visits_restored ORDER BY day",
            json!([
                [{"stringValue": "monday"}, {"longValue": 7}],
                [{"stringValue": "tuesday"}, {"longValue": 9}]
            ]),
        ),
    ];

    let mut request_ids: Vec<String> = Vec::new();
    for (case, members, read, expected) in rows {
        let restored = restore_table(&emulator, cluster, snapshot, &members).await;
        assert_eq!(
            restored.status, 200,
            "case {case}: expected 200, got {} with body {}",
            restored.status, restored.body
        );
        // The request is reported with the identifier the service minted and the
        // status the restore is in.
        let request_id = restored.text("TableRestoreRequestId").unwrap_or_default();
        assert!(
            !request_id.is_empty(),
            "case {case}: expected a minted TableRestoreRequestId, got body {}",
            restored.body
        );
        assert_eq!(
            restored.text("Status").as_deref(),
            Some(state::TABLE_RESTORE_STATUS_IN_PROGRESS),
            "case {case}: expected Status={}, got {:?}",
            state::TABLE_RESTORE_STATUS_IN_PROGRESS,
            restored.text("Status")
        );
        assert_eq!(
            restored.text("ClusterIdentifier").as_deref(),
            Some(cluster),
            "case {case}: expected the cluster the table was restored to, got {:?}",
            restored.text("ClusterIdentifier")
        );
        request_ids.push(request_id);

        // The TABLE is really there, with the rows the snapshot held: this is the
        // read a client makes after the restore.
        let records = query(&emulator, case, cluster, DATABASE, read).await;
        assert_eq!(
            records, expected,
            "case {case}: expected the snapshot's rows in the restored table, got {records}"
        );
    }

    // Every request is readable, in request order, and each reaches `SUCCEEDED` at
    // the due instant — whether or not anybody called.
    let (listed, _) = emulator
        .poll_until(
            "every restore succeeds",
            "DescribeTableRestoreStatus",
            &[("ClusterIdentifier", cluster)],
            |answer| {
                all_text(&answer.body, "Status")
                    .iter()
                    .filter(|status| status.as_str() == state::TABLE_RESTORE_STATUS_SUCCEEDED)
                    .count()
                    == 3
            },
        )
        .await;
    assert_eq!(
        all_text(&listed.body, "TableRestoreRequestId"),
        request_ids,
        "case every restore succeeds: expected the requests in request order, got body {}",
        listed.body
    );

    // One request by its own identifier is that request alone.
    let single = emulator
        .call(
            "DescribeTableRestoreStatus",
            &[("TableRestoreRequestId", &request_ids[1])],
        )
        .await;
    assert_eq!(
        all_text(&single.body, "TableRestoreRequestId"),
        vec![request_ids[1].clone()],
        "case one request by identifier: expected exactly that request, got body {}",
        single.body
    );
    assert_eq!(
        single.text("NewTableName").as_deref(),
        Some("totals_restored"),
        "case one request by identifier: expected the table that request created, got {:?}",
        single.text("NewTableName")
    );

    // An identifier this account made no restore under names nothing.
    let unknown = emulator
        .call(
            "DescribeTableRestoreStatus",
            &[("TableRestoreRequestId", "nowhere-request")],
        )
        .await;
    assert_refused(
        "a request identifier nothing minted",
        &unknown,
        400,
        "TableRestoreNotFoundFault",
    );
}

/// A restore whose names resolve to nothing is refused with the fault that names
/// the resource, and creates no table — for a table spanning each name the
/// request carries.
#[tokio::test]
async fn a_restore_naming_something_absent_is_refused_and_creates_nothing() {
    let emulator = Emulator::with_data_api();
    let cluster = "refused-restore-cluster";
    let snapshot = "refused-restore-snapshot";
    create_cluster(&emulator, "a refused restore", cluster, DATABASE).await;
    for sql in [
        "CREATE TABLE people (id INTEGER, name VARCHAR(16))",
        "INSERT INTO people (id, name) VALUES (1, 'ada')",
    ] {
        run_sql(&emulator, "seed the table", cluster, DATABASE, sql).await;
    }
    create_snapshot(&emulator, "a refused restore", cluster, snapshot).await;
    settled(&emulator, "a refused restore", cluster, snapshot).await;

    // (case, the restore's members, the status and code expected)
    let rows: Vec<RefusalRow> = vec![
        (
            "a snapshot this account does not hold",
            vec![
                ("SnapshotIdentifier", "nowhere-snapshot"),
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("NewTableName", "people_copy"),
            ],
            404,
            "ClusterSnapshotNotFound",
        ),
        (
            "a source database the snapshot does not hold",
            vec![
                ("SourceDatabaseName", "nowheredb"),
                ("SourceTableName", "people"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidTableRestoreArgument",
        ),
        (
            "a source table the snapshot's database does not hold",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "nowhere_table"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidTableRestoreArgument",
        ),
        (
            "a source schema the snapshot's database does not hold",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceSchemaName", "nowhere_schema"),
                ("SourceTableName", "people"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidTableRestoreArgument",
        ),
        (
            "a target schema the cluster's database does not hold",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("TargetSchemaName", "nowhere_schema"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidTableRestoreArgument",
        ),
        (
            "a new table name the cluster already holds",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("NewTableName", "people"),
            ],
            400,
            "InvalidTableRestoreArgument",
        ),
        (
            // Every name the request carries addresses a database object, so each
            // is held to the documented Redshift database-name syntax before it
            // reaches the engine: a value outside it names no object at all.
            "a source table name outside the documented name syntax",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people; drop table people"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a new table name outside the documented name syntax",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("NewTableName", "people\"_copy"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a source database name starting with a digit",
            vec![
                ("SourceDatabaseName", "9devdb"),
                ("SourceTableName", "people"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a target schema name outside the documented name syntax",
            vec![
                ("SourceDatabaseName", DATABASE),
                ("SourceTableName", "people"),
                ("TargetSchemaName", "analytics/public"),
                ("NewTableName", "people_copy"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (case, members, status, code) in rows {
        // Every row carries the snapshot this suite took unless it names its own.
        let named_snapshot = members
            .iter()
            .any(|(member, _)| *member == "SnapshotIdentifier");
        let answer = if named_snapshot {
            let mut params: Vec<(&str, &str)> = vec![("ClusterIdentifier", cluster)];
            params.extend(members.iter().copied());
            emulator
                .call("RestoreTableFromClusterSnapshot", &params)
                .await
        } else {
            restore_table(&emulator, cluster, snapshot, &members).await
        };
        assert_refused(case, &answer, status, code);
        // Nothing was recorded: a refused restore leaves no request behind.
        let listed = emulator
            .call(
                "DescribeTableRestoreStatus",
                &[("ClusterIdentifier", cluster)],
            )
            .await;
        assert!(
            all_text(&listed.body, "TableRestoreRequestId").is_empty(),
            "case {case}: expected the refused restore to record nothing, got body {}",
            listed.body
        );
        // … and the table the refused row named is not in the cluster, while the
        // source table is untouched.
        let copy = submit_sql(
            &emulator,
            case,
            cluster,
            DATABASE,
            "SELECT id FROM people_copy",
        )
        .await;
        assert_ne!(
            copy["Status"],
            json!("FINISHED"),
            "case {case}: expected no table created by the refused restore, got {copy}"
        );
        let source = query(
            &emulator,
            case,
            cluster,
            DATABASE,
            "SELECT id FROM people ORDER BY id",
        )
        .await;
        assert_eq!(
            source,
            json!([[{"longValue": 1}]]),
            "case {case}: expected the source table untouched, got {source}"
        );
    }
}
