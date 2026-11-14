// SPDX-License-Identifier: CC0-1.0

//! What `StartExportTask` answers for a source whose stored state the export cannot read,
//! and what it answers once that source is settled.
//!
//! The rule: `StartExportTask` is SERVED from this service's persisted state — a request
//! every resource it names admits is answered with the modelled `ExportTask`, and a source
//! whose stored status is not the one its data can be exported from is answered with the
//! modelled `InvalidExportSourceState` (HTTP 400) carrying the sentence the model states for
//! that error class. The scaffold's 501 `NotImplemented` is no outcome of this operation:
//! the model declares it nowhere.
//!
//! The rows span the kinds of source `SourceArn` may name — a DB snapshot, a DB cluster
//! snapshot and a DB cluster — and, for each, a status of that resource's own published
//! status values that the export cannot read, with the resource's available status as the
//! control, and a destination prefix given and omitted. What decides the refusal is the
//! state the source is IN, not the kind of source and not the request.
//!
//! Each source's status is written into the store with the transition the clock owes it
//! dropped, so every row's outcome is decided by the state it names and by nothing else: no
//! assertion here waits for a span, and none of them can be changed by how fast the host
//! runs.
//!
//! Amazon S3, IAM and KMS are mounted beside this service on ONE engine, exactly as the
//! assembled router mounts them, because the export resolves its bucket, role and key
//! against them.

use std::sync::Arc;

use service_com_amazonaws_rds_amazonrdsv19::state as st;
use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call, expect_error, expect_ok, optional_text, subtrees, text, REGION};
use crate::rds_s3_integration::{create_bucket, emulator, kms, put_object_policy, role};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The master user the sources below are created with, and its password — test credentials,
/// and the only place in this suite they appear.
const MASTER_USER: &str = "emuadmin";
const MASTER_PASSWORD: &str = "emu-Secret-99";

/// The DB instance the DB snapshot is taken of, the snapshot itself, and the DB cluster and
/// its snapshot: lower case throughout, as the identifiers of this API are stored.
const INSTANCE: &str = "emu-source-state-instance";
const SNAPSHOT: &str = "emu-source-state-snapshot";
const CLUSTER: &str = "emu-source-state-cluster";
const CLUSTER_SNAPSHOT: &str = "emu-source-state-cluster-snapshot";

/// The sentence `InvalidExportSourceStateFault` carries, as this service's model states that
/// error class: one fact, with no variable part. Spelled out here rather than read from the
/// code under test.
const INVALID_SOURCE_STATE: &str =
    "The state of the export snapshot is invalid for exporting to an Amazon S3 bucket.";

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        [$(($name, $value.to_string())),*]
    };
}

/// The destination an export writes to, and the engine the sources are stored in.
struct Fixture {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
    bucket: String,
    role_arn: String,
    key_id: String,
}

/// The kind of resource one row's `SourceArn` names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceKind {
    /// `arn:aws:rds:…:snapshot:<id>` — a DB snapshot of a DB instance.
    DbSnapshot,
    /// `arn:aws:rds:…:cluster-snapshot:<id>` — a DB cluster snapshot.
    ClusterSnapshot,
    /// `arn:aws:rds:…:cluster:<id>` — the DB cluster itself.
    Cluster,
}

impl SourceKind {
    /// The ARN a request names this source by.
    fn source_arn(self) -> String {
        match self {
            Self::DbSnapshot => format!("arn:aws:rds:{REGION}:{ACCOUNT}:snapshot:{SNAPSHOT}"),
            Self::ClusterSnapshot => {
                format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster-snapshot:{CLUSTER_SNAPSHOT}")
            }
            Self::Cluster => format!("arn:aws:rds:{REGION}:{ACCOUNT}:cluster:{CLUSTER}"),
        }
    }

    /// The status the export reads this kind of source in — `DBSnapshot$Status` and
    /// `DBCluster$Status` each publish their own.
    fn available(self) -> &'static str {
        match self {
            Self::DbSnapshot | Self::ClusterSnapshot => st::SNAPSHOT_STATUS_AVAILABLE,
            Self::Cluster => st::DB_CLUSTER_STATUS_AVAILABLE,
        }
    }

    /// `ExportTask$SourceType` for an export of this kind of source.
    fn source_type(self) -> &'static str {
        match self {
            Self::DbSnapshot | Self::ClusterSnapshot => st::EXPORT_SOURCE_TYPE_SNAPSHOT,
            Self::Cluster => st::EXPORT_SOURCE_TYPE_CLUSTER,
        }
    }
}

/// One row: the source kind, the stored status the export cannot read it in, and the
/// destination prefix the request names.
struct Row {
    case: &'static str,
    kind: SourceKind,
    /// A value of that resource's own published status values, outside the one the export
    /// reads: a snapshot still being taken or copied (`DBSnapshot$Status`), and a cluster
    /// still being created or stopped (`DBCluster$Status`).
    blocked: &'static str,
    prefix: Option<&'static str>,
}

const ROWS: &[Row] = &[
    Row {
        case: "a DB snapshot still being taken",
        kind: SourceKind::DbSnapshot,
        blocked: st::SNAPSHOT_STATUS_CREATING,
        prefix: None,
    },
    Row {
        case: "a DB snapshot still being copied",
        kind: SourceKind::DbSnapshot,
        blocked: st::SNAPSHOT_STATUS_COPYING,
        prefix: Some("nightly/exports"),
    },
    Row {
        case: "a DB cluster snapshot still being taken",
        kind: SourceKind::ClusterSnapshot,
        blocked: st::SNAPSHOT_STATUS_CREATING,
        prefix: Some("cluster-snapshots"),
    },
    Row {
        case: "a DB cluster snapshot still being copied",
        kind: SourceKind::ClusterSnapshot,
        blocked: st::SNAPSHOT_STATUS_COPYING,
        prefix: None,
    },
    Row {
        case: "a DB cluster still being created",
        kind: SourceKind::Cluster,
        blocked: st::DB_CLUSTER_STATUS_CREATING,
        prefix: Some("live"),
    },
    Row {
        case: "a DB cluster that is stopped",
        kind: SourceKind::Cluster,
        blocked: st::DB_CLUSTER_STATUS_STOPPED,
        prefix: None,
    },
];

/// Stand up the destination — a bucket of this Region, a KMS key and a role the export
/// principal may assume that is granted the object write — and the four sources the rows
/// name.
async fn fixture() -> Fixture {
    let (router, engine) = emulator();
    let bucket = "emu-source-state-bucket".to_string();
    create_bucket(&router, &bucket, REGION).await;
    let key_id = kms(&router)
        .create_key()
        .description("the export's encryption key")
        .send()
        .await
        .expect("the emulated KMS creates the export key")
        .key_metadata()
        .map(|metadata| metadata.key_id().to_string())
        .expect("the created key reports its id");
    let role_arn = role(
        &router,
        "emu-source-state-role",
        st::EXPORT_SERVICE_PRINCIPAL,
        Some(put_object_policy(&bucket)),
    )
    .await;
    let fixture = Fixture {
        router,
        engine,
        bucket,
        role_arn,
        key_id,
    };

    // The DB instance the snapshot is taken of, settled so the snapshot may be taken at all.
    expect_ok(
        "the rows' DB instance",
        call(
            &fixture.router,
            "CreateDBInstance",
            &params![
                "DBInstanceIdentifier" => INSTANCE,
                "Engine" => "mysql",
                "DBInstanceClass" => "db.t3.micro",
                "AllocatedStorage" => 20,
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
            ],
        )
        .await,
    );
    settle_instance(&fixture);
    expect_ok(
        "the rows' DB snapshot",
        call(
            &fixture.router,
            "CreateDBSnapshot",
            &params![
                "DBInstanceIdentifier" => INSTANCE,
                "DBSnapshotIdentifier" => SNAPSHOT,
            ],
        )
        .await,
    );

    // And the DB cluster and the cluster snapshot taken of it.
    expect_ok(
        "the rows' DB cluster",
        call(
            &fixture.router,
            "CreateDBCluster",
            &params![
                "DBClusterIdentifier" => CLUSTER,
                "Engine" => "aurora-mysql",
                "MasterUsername" => MASTER_USER,
                "MasterUserPassword" => MASTER_PASSWORD,
            ],
        )
        .await,
    );
    put_status(
        &fixture,
        "the rows' DB cluster",
        SourceKind::Cluster,
        st::DB_CLUSTER_STATUS_AVAILABLE,
    );
    expect_ok(
        "the rows' DB cluster snapshot",
        call(
            &fixture.router,
            "CreateDBClusterSnapshot",
            &params![
                "DBClusterIdentifier" => CLUSTER,
                "DBClusterSnapshotIdentifier" => CLUSTER_SNAPSHOT,
            ],
        )
        .await,
    );
    fixture
}

/// This service's store, as the handlers reach it.
fn store(fixture: &Fixture) -> engine::ServiceState<st::ComAmazonawsRdsAmazonrdsv19State> {
    st::service_state(&*fixture.engine).expect("this service's store")
}

/// Settle the DB instance the snapshot is taken of: `CreateDBSnapshot` reads a DB instance
/// only in its available state, and this suite asserts nothing about that span.
fn settle_instance(fixture: &Fixture) {
    let state = store(fixture);
    let mut store = state.lock();
    let record = store
        .db_instances
        .get_mut(&(REGION.to_string(), INSTANCE.to_string()))
        .expect("the store holds the created DB instance");
    record.status = st::DB_INSTANCE_STATUS_AVAILABLE.to_string();
    record.pending_status = None;
}

/// Put `status` on the stored source and drop the transition the clock owes it, so the state
/// the export is judged against is exactly the row's.
fn put_status(fixture: &Fixture, case: &str, kind: SourceKind, status: &str) {
    let state = store(fixture);
    let mut store = state.lock();
    match kind {
        SourceKind::DbSnapshot => {
            let record = store
                .db_snapshots
                .get_mut(&(REGION.to_string(), SNAPSHOT.to_string()))
                .unwrap_or_else(|| panic!("case {case:?}: the store holds the DB snapshot"));
            record.status = status.to_string();
            record.pending_status = None;
        }
        SourceKind::ClusterSnapshot => {
            let record = store
                .db_cluster_snapshots
                .get_mut(&(REGION.to_string(), CLUSTER_SNAPSHOT.to_string()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store holds the DB cluster snapshot")
                });
            record.status = status.to_string();
            record.pending_status = None;
        }
        SourceKind::Cluster => {
            let record = store
                .db_clusters
                .get_mut(&(REGION.to_string(), CLUSTER.to_string()))
                .unwrap_or_else(|| panic!("case {case:?}: the store holds the DB cluster"));
            record.status = status.to_string();
            record.pending_status = None;
        }
    }
}

/// Start one export, as a client starts it.
async fn start_export(fixture: &Fixture, identifier: &str, row: &Row) -> (u16, String) {
    let mut request: Vec<(&str, String)> = params![
        "ExportTaskIdentifier" => identifier,
        "SourceArn" => row.kind.source_arn(),
        "S3BucketName" => fixture.bucket,
        "IamRoleArn" => fixture.role_arn,
        "KmsKeyId" => fixture.key_id,
    ]
    .to_vec();
    if let Some(prefix) = row.prefix {
        request.push(("S3Prefix", prefix.to_string()));
    }
    call(&fixture.router, "StartExportTask", &request).await
}

/// What `DescribeExportTasks` answers for one identifier.
async fn describe_export(fixture: &Fixture, identifier: &str) -> (u16, String) {
    call(
        &fixture.router,
        "DescribeExportTasks",
        &params!["ExportTaskIdentifier" => identifier],
    )
    .await
}

/// Rule: a source whose stored status is not the one the export reads is refused with the
/// modelled `InvalidExportSourceState`, for every kind of source `SourceArn` may name — and
/// once the source is settled the SAME request is served with the modelled `ExportTask`.
#[tokio::test]
async fn an_export_reads_a_settled_source_and_refuses_one_that_is_not() {
    let fixture = fixture().await;

    for (index, row) in ROWS.iter().enumerate() {
        let identifier = format!("emu-source-state-export-{index}");

        // The source in the state this row names: the export is refused, with the status,
        // the code and the sentence the model states for that error class.
        put_status(&fixture, row.case, row.kind, row.blocked);
        let body = expect_error(
            row.case,
            start_export(&fixture, &identifier, row).await,
            400,
            "InvalidExportSourceState",
        );
        let message = text(&body, "Message");
        assert_eq!(
            message, INVALID_SOURCE_STATE,
            "case {:?}: expected the refusal sentence {INVALID_SOURCE_STATE:?}, got {message:?}",
            row.case
        );
        // A refused export records no task, so a report narrowed to the identifier it named
        // carries none.
        let after_refusal = expect_ok(row.case, describe_export(&fixture, &identifier).await);
        let recorded = subtrees(&after_refusal, "ExportTask");
        assert!(
            recorded.is_empty(),
            "case {:?}: a refused export records no task, got {after_refusal}",
            row.case
        );

        // The one thing that changes is the source's own status, and the same request is
        // then served: the modelled output, with the request's values on it.
        put_status(&fixture, row.case, row.kind, row.kind.available());
        let started = expect_ok(row.case, start_export(&fixture, &identifier, row).await);
        let status = text(&started, "Status");
        assert_eq!(
            status,
            st::EXPORT_TASK_STATUS_STARTING,
            "case {:?}: an accepted export starts in {}, got {status:?}",
            row.case,
            st::EXPORT_TASK_STATUS_STARTING
        );
        for (member, expected) in [
            ("ExportTaskIdentifier", identifier.clone()),
            ("SourceArn", row.kind.source_arn()),
            ("SourceType", row.kind.source_type().to_string()),
            ("S3Bucket", fixture.bucket.clone()),
            ("IamRoleArn", fixture.role_arn.clone()),
            ("KmsKeyId", fixture.key_id.clone()),
            ("PercentProgress", st::INITIAL_PERCENT_PROGRESS.to_string()),
        ] {
            let reported = text(&started, member);
            assert_eq!(
                reported, expected,
                "case {:?}: expected {member} {expected:?}, got {reported:?}",
                row.case
            );
        }
        let reported_prefix = optional_text(&started, "S3Prefix");
        assert_eq!(
            reported_prefix.as_deref(),
            row.prefix,
            "case {:?}: expected S3Prefix {:?}, got {reported_prefix:?}",
            row.case,
            row.prefix
        );

        // And the task the export recorded is what `DescribeExportTasks` reports: one task,
        // under this identifier, carrying the same destination and source and a status of
        // the ones that member publishes — whichever the clock has moved it to by now.
        let described = expect_ok(row.case, describe_export(&fixture, &identifier).await);
        let tasks = subtrees(&described, "ExportTask");
        assert_eq!(
            tasks.len(),
            1,
            "case {:?}: expected the one recorded task, got {described}",
            row.case
        );
        let task = &tasks[0];
        for (member, expected) in [
            ("ExportTaskIdentifier", identifier.clone()),
            ("SourceArn", row.kind.source_arn()),
            ("SourceType", row.kind.source_type().to_string()),
            ("S3Bucket", fixture.bucket.clone()),
            ("IamRoleArn", fixture.role_arn.clone()),
            ("KmsKeyId", fixture.key_id.clone()),
        ] {
            let reported = text(task, member);
            assert_eq!(
                reported, expected,
                "case {:?}: expected the described {member} {expected:?}, got {reported:?}",
                row.case
            );
        }
        // The status is the one the clock has moved the accepted export to by the time of
        // this read: it has started, so it is in flight or already finished — never a status
        // the export did not reach (`CANCELED`, `CANCELING`, `FAILED`).
        let described_status = text(task, "Status");
        let in_flight_or_done = [
            st::EXPORT_TASK_STATUS_STARTING,
            st::EXPORT_TASK_STATUS_IN_PROGRESS,
            st::EXPORT_TASK_STATUS_COMPLETE,
        ];
        assert!(
            in_flight_or_done.contains(&described_status.as_str()),
            "case {:?}: expected one of {in_flight_or_done:?}, got {described_status:?}",
            row.case
        );
        let failure_cause = optional_text(task, "FailureCause");
        assert!(
            failure_cause.is_none(),
            "case {:?}: an export of a settled source carries no FailureCause, got {failure_cause:?}",
            row.case
        );
    }
}
