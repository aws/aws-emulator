// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `StartActivityStream` and `StopActivityStream`: the database activity
//! stream of the DB cluster or DB instance a `ResourceArn` names, and the
//! `ActivityStream*` members that resource's own describe reports it through.
//!
//! The rule under test: `StartActivityStream` records the mode, the KMS key and the
//! engine-native-audit-fields setting of a database activity stream on the DB cluster or DB
//! instance its `ResourceArn` ARN resolves to in the request's Region, delivers it to a
//! Kinesis data stream named after that resource's own immutable resource id, and reports the
//! stream `starting` until this service's clock makes the start due — at once where
//! `ApplyImmediately` asked for it, else in the resource's next maintenance window — after
//! which the resource reports `started` and the stream's configuration; `StopActivityStream`
//! ends that same stream, reporting it `stopping` with the delivery target and key the start
//! recorded until the stop is due, after which the resource reports `stopped` and no stream
//! configuration at all. A `ResourceArn` that names no DB cluster of this account in the
//! request's Region is `DBClusterNotFoundFault` (HTTP 404) and one that names no DB instance
//! is `DBInstanceNotFoundFault` (HTTP 404); a resource whose engine has no database activity
//! stream, one that is not in a state the call can be taken in, a second start of a running
//! stream and a stop of a stream that is not running are the matching invalid-state fault
//! (HTTP 400); and a value that is no Amazon RDS DB cluster or DB instance ARN of the
//! request's Region at all is refused on the member that carried it —
//! `smithy_query(query="operation", operation="StartActivityStream")` for the faults and their
//! statuses, and `smithy_query(query="shape", shape="ActivityStreamStatus")` for the four
//! states a stream is reported in.
//!
//! The tables span what the rules generalise over: both resource kinds a stream belongs to,
//! both Aurora engines at the cluster level and both instance-level engine families, both
//! modes a stream can be started in, each form `KmsKeyId` accepts (key ARN, key id, alias ARN
//! and alias name), the engine-native-audit-fields setting present and absent, both ways
//! `ApplyImmediately` decides when the start comes due, the engines that have no stream at
//! all, and the forms a `ResourceArn` can take that resolve to nothing (another account,
//! another resource type and no ARN at all), and a `ResourceArn` naming another REGION, which
//! is refused on the member rather than reported absent — the Region pairs and resource kinds
//! that rule covers are the table in `activity_stream_resource_arn_region`.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with, and every wait is the poll loop a client
//! writes (`lifecycle_poll_loops::poll_until`) rather than a sleep.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_rds_amazonrdsv19::state::{
    ACTIVITY_STREAM_ALREADY_STARTED_MESSAGE, ACTIVITY_STREAM_CLUSTER_ENGINE_MESSAGE,
    ACTIVITY_STREAM_ENGINE_MESSAGE, ACTIVITY_STREAM_NOT_STARTED_MESSAGE,
    DB_CLUSTER_STATUS_AVAILABLE, DB_INSTANCE_STATUS_AVAILABLE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::db_clusters::describe_one;
use crate::db_instances::{
    call, call_in, emulator, expect_error, expect_ok, optional_text, subtrees, text, REGION,
};
use crate::lifecycle_poll_loops::poll_until;
use crate::pending_modified_values::{window_days_ahead, window_that_started};

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// A second Region, so the REGIONAL scope of the resource a `ResourceArn` resolves to is
/// observable.
const OTHER_REGION: &str = "eu-west-1";

// The members of `ActivityStreamStatus` (`smithy_query(query="shape",
// shape="ActivityStreamStatus")`: stopped | starting | started | stopping), written out here
// rather than taken from the code under test.
/// The stream is on its way to delivering.
const STARTING: &str = "starting";
/// The stream is delivering.
const STARTED: &str = "started";
/// The stream is on its way out of delivering.
const STOPPING: &str = "stopping";
/// The resource has no stream delivering.
const STOPPED: &str = "stopped";

// The members of `ActivityStreamMode` (`smithy_query(query="shape",
// shape="ActivityStreamMode")`: async | sync).
/// The database session handles an activity stream event asynchronously.
const ASYNC: &str = "async";
/// And synchronously.
const SYNC: &str = "sync";

/// The name Amazon RDS gives the Kinesis data stream a database activity stream is delivered
/// to: `aws-rds-das-` followed by the resource's own immutable resource id (Amazon Aurora User
/// Guide / Amazon RDS User Guide, "Monitoring … with Database Activity Streams"). Written out
/// here rather than taken from the code under test.
fn kinesis_stream_name(resource_id: &str) -> String {
    format!("aws-rds-das-{resource_id}")
}

/// The `arn:<partition>:rds:<region>:<account>:cluster:<identifier>` form Amazon RDS gives a
/// DB cluster ("Constructing an ARN for Amazon RDS").
fn cluster_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:cluster:{identifier}")
}

/// And the `:db:` form it gives a DB instance.
fn instance_arn(region: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:db:{identifier}")
}

/// Which resource kind a row drives the stream of: a stream belongs either to a DB cluster or
/// to a DB instance, and each is addressed, created and read its own way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Cluster,
    Instance,
}

/// Which of the forms `KmsKeyId` accepts a row names its key in: "the key ARN, key ID, alias
/// ARN, or alias name for the KMS key" (`StartActivityStreamRequest$KmsKeyId`).
#[derive(Clone, Copy, Debug)]
enum KeyForm {
    KeyArn,
    KeyId,
    AliasArn,
    AliasName,
}

/// One resource a start and a stop are driven against.
#[derive(Clone, Copy, Debug)]
struct StreamCase {
    case: &'static str,
    kind: Kind,
    identifier: &'static str,
    engine: &'static str,
    /// The `DBInstanceClass` a DB instance row is created with; unread for a cluster row.
    class: &'static str,
    /// `Mode`: the mode the stream is started in.
    mode: &'static str,
    /// `KmsKeyId`: which form the row names its key in, and the key's own name.
    key_form: KeyForm,
    key: &'static str,
    /// `EngineNativeAuditFieldsIncluded` as the row names it, and `None` for a row that names
    /// none (the documented default, "no engine-native audit fields are included").
    audit_fields: Option<bool>,
}

/// Both resource kinds a database activity stream belongs to: the Aurora engines at the
/// cluster level, and the two instance-level engine families Amazon RDS serves streams for,
/// across both modes, every form `KmsKeyId` accepts, and the audit-fields setting present and
/// absent.
const STREAM_CASES: &[StreamCase] = &[
    StreamCase {
        case: "an aurora-mysql cluster, async, key ARN",
        kind: Kind::Cluster,
        identifier: "emu-das-aurora-mysql",
        engine: "aurora-mysql",
        class: "",
        mode: ASYNC,
        key_form: KeyForm::KeyArn,
        key: "aa11bb22-cc33-dd44-ee55-ff6677889900",
        audit_fields: None,
    },
    StreamCase {
        case: "an aurora-postgresql cluster, sync, alias name",
        kind: Kind::Cluster,
        identifier: "emu-das-aurora-postgres",
        engine: "aurora-postgresql",
        class: "",
        mode: SYNC,
        key_form: KeyForm::AliasName,
        key: "emu-das-cluster-key",
        // The option "applies to an Oracle or Microsoft SQL Server DB instance", so a cluster
        // row names it to show it is not reported back for a cluster.
        audit_fields: Some(true),
    },
    StreamCase {
        case: "an oracle-se2 instance, async, alias ARN",
        kind: Kind::Instance,
        identifier: "emu-das-oracle",
        engine: "oracle-se2",
        class: "db.m5.large",
        mode: ASYNC,
        key_form: KeyForm::AliasArn,
        key: "emu-das-instance-key",
        audit_fields: Some(true),
    },
    StreamCase {
        case: "a sqlserver-ex instance, sync, key id",
        kind: Kind::Instance,
        identifier: "emu-das-sqlserver",
        engine: "sqlserver-ex",
        class: "db.t3.medium",
        mode: SYNC,
        key_form: KeyForm::KeyId,
        key: "11223344-5566-7788-99aa-bbccddeeff00",
        audit_fields: None,
    },
];

impl StreamCase {
    /// The ARN the stored resource carries, which is the handle both calls address it by.
    fn arn(&self) -> String {
        match self.kind {
            Kind::Cluster => cluster_arn(REGION, self.identifier),
            Kind::Instance => instance_arn(REGION, self.identifier),
        }
    }

    /// `KmsKeyId` as this row sends it.
    fn kms_key_id(&self) -> String {
        match self.key_form {
            KeyForm::KeyArn => format!("arn:aws:kms:{REGION}:{ACCOUNT}:key/{}", self.key),
            KeyForm::KeyId => self.key.to_string(),
            KeyForm::AliasArn => format!("arn:aws:kms:{REGION}:{ACCOUNT}:alias/{}", self.key),
            KeyForm::AliasName => format!("alias/{}", self.key),
        }
    }

    /// Whether `EngineNativeAuditFieldsIncluded` is a setting this resource kind carries: the
    /// member "applies to an Oracle or Microsoft SQL Server DB instance", and `DBCluster`
    /// carries no member for it at all.
    fn reports_audit_fields(&self) -> bool {
        self.kind == Kind::Instance
    }
}

/// Create the resource a row drives, with a maintenance window `window` where the row needs a
/// particular one, and wait until the clock has made it available.
async fn create_available(router: &MultiServiceRouter, row: &StreamCase, window: Option<&str>) {
    let mut params: Vec<(&str, String)> = match row.kind {
        Kind::Cluster => vec![
            ("DBClusterIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
            ("MasterUsername", "emuadmin".to_string()),
            ("MasterUserPassword", "a-strong-password-1".to_string()),
        ],
        Kind::Instance => vec![
            ("DBInstanceIdentifier", row.identifier.to_string()),
            ("Engine", row.engine.to_string()),
            ("DBInstanceClass", row.class.to_string()),
        ],
    };
    if let Some(window) = window {
        params.push(("PreferredMaintenanceWindow", window.to_string()));
    }
    let action = match row.kind {
        Kind::Cluster => "CreateDBCluster",
        Kind::Instance => "CreateDBInstance",
    };
    expect_ok(row.case, call(router, action, &params).await);
    poll_until_available(router, row).await;
}

/// Poll the resource's own describe until it reports the available status, the way a client
/// waits for a state this service's clock owes.
async fn poll_until_available(router: &MultiServiceRouter, row: &StreamCase) {
    let (action, member, identifier_member, expected) = match row.kind {
        Kind::Cluster => (
            "DescribeDBClusters",
            "Status",
            "DBClusterIdentifier",
            DB_CLUSTER_STATUS_AVAILABLE,
        ),
        Kind::Instance => (
            "DescribeDBInstances",
            "DBInstanceStatus",
            "DBInstanceIdentifier",
            DB_INSTANCE_STATUS_AVAILABLE,
        ),
    };
    poll_until(
        row.case,
        &format!("{} reaching {expected}", row.identifier),
        || async { call(router, action, &[(identifier_member, row.identifier)]).await },
        |status, body| status == 200 && text(body, member) == expected,
    )
    .await;
}

/// The one `<DBCluster>` or `<DBInstance>` subtree the resource's own describe reports.
async fn envelope(router: &MultiServiceRouter, row: &StreamCase) -> String {
    match row.kind {
        Kind::Cluster => describe_one(router, row.case, row.identifier).await,
        Kind::Instance => {
            let body = expect_ok(
                row.case,
                call(
                    router,
                    "DescribeDBInstances",
                    &[("DBInstanceIdentifier", row.identifier)],
                )
                .await,
            );
            let found = subtrees(&body, "DBInstance");
            assert_eq!(
                found.len(),
                1,
                "case {:?}: expected exactly one DBInstance, got {} in {body}",
                row.case,
                found.len()
            );
            found.into_iter().next().expect("checked just above")
        }
    }
}

/// Poll the resource's own describe until its `ActivityStreamStatus` is `expected`.
async fn poll_until_stream(router: &MultiServiceRouter, row: &StreamCase, expected: &str) {
    let (action, identifier_member) = match row.kind {
        Kind::Cluster => ("DescribeDBClusters", "DBClusterIdentifier"),
        Kind::Instance => ("DescribeDBInstances", "DBInstanceIdentifier"),
    };
    poll_until(
        row.case,
        &format!("the activity stream of {} reaching {expected}", row.identifier),
        || async { call(router, action, &[(identifier_member, row.identifier)]).await },
        |status, body| status == 200 && text(body, "ActivityStreamStatus") == expected,
    )
    .await;
}

/// `StartActivityStream` for `arn`, with the members a row carries.
async fn start(
    router: &MultiServiceRouter,
    arn: &str,
    mode: &str,
    kms_key_id: &str,
    audit_fields: Option<bool>,
    apply_immediately: Option<bool>,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![
        ("ResourceArn", arn.to_string()),
        ("Mode", mode.to_string()),
        ("KmsKeyId", kms_key_id.to_string()),
    ];
    if let Some(included) = audit_fields {
        params.push(("EngineNativeAuditFieldsIncluded", included.to_string()));
    }
    if let Some(immediately) = apply_immediately {
        params.push(("ApplyImmediately", immediately.to_string()));
    }
    call(router, "StartActivityStream", &params).await
}

/// `StartActivityStream` for a row, asked to apply as soon as possible.
async fn start_row(router: &MultiServiceRouter, row: &StreamCase) -> (u16, String) {
    start(
        router,
        &row.arn(),
        row.mode,
        &row.kms_key_id(),
        row.audit_fields,
        Some(true),
    )
    .await
}

/// `StopActivityStream` for `arn`.
async fn stop(
    router: &MultiServiceRouter,
    arn: &str,
    apply_immediately: Option<bool>,
) -> (u16, String) {
    let mut params: Vec<(&str, String)> = vec![("ResourceArn", arn.to_string())];
    if let Some(immediately) = apply_immediately {
        params.push(("ApplyImmediately", immediately.to_string()));
    }
    call(router, "StopActivityStream", &params).await
}

/// Assert one member of a response or envelope, naming the case and the value read.
fn assert_member(case: &str, xml: &str, member: &str, expected: &str) {
    let actual = text(xml, member);
    assert_eq!(
        actual, expected,
        "case {case:?}: expected {member} {expected:?}, got {actual:?} in {xml}"
    );
}

/// RULE: a start records the stream on the resource its ARN names and reports it `starting`;
/// the resource's own describe reports the same stream and reaches `started` on this service's
/// clock; a stop reports the same delivery target `stopping`; and once the stop is due the
/// resource reports `stopped` with no stream configuration at all.
#[tokio::test]
async fn each_resource_kind_starts_its_stream_reads_it_back_and_stops_it() {
    let router = emulator();

    for row in STREAM_CASES {
        create_available(&router, row, None).await;
    }

    for row in STREAM_CASES {
        let case = row.case;
        // The resource's own immutable id, which is what the delivery target is named after:
        // read from the resource rather than assumed, so the assertion is about the NAME the
        // service derives rather than about an id the test chose.
        let before = envelope(&router, row).await;
        let resource_id = match row.kind {
            Kind::Cluster => text(&before, "DbClusterResourceId"),
            Kind::Instance => text(&before, "DbiResourceId"),
        };
        let expected_kinesis = kinesis_stream_name(&resource_id);
        // A resource no stream was started on reports the `stopped` member and none of a
        // stream's configuration.
        assert_member(case, &before, "ActivityStreamStatus", STOPPED);
        for member in [
            "ActivityStreamMode",
            "ActivityStreamKmsKeyId",
            "ActivityStreamKinesisStreamName",
        ] {
            let reported = optional_text(&before, member);
            assert_eq!(
                reported, None,
                "case {case:?}: expected no {member} before the start, got {reported:?}"
            );
        }

        let started = expect_ok(case, start_row(&router, row).await);
        assert_member(case, &started, "Status", STARTING);
        assert_member(case, &started, "Mode", row.mode);
        assert_member(case, &started, "KmsKeyId", &row.kms_key_id());
        assert_member(case, &started, "KinesisStreamName", &expected_kinesis);
        assert_member(case, &started, "ApplyImmediately", "true");
        let audit = optional_text(&started, "EngineNativeAuditFieldsIncluded");
        let expected_audit = row
            .reports_audit_fields()
            .then(|| row.audit_fields.unwrap_or(false).to_string());
        assert_eq!(
            audit, expected_audit,
            "case {case:?}: expected EngineNativeAuditFieldsIncluded {expected_audit:?}, got \
             {audit:?} in {started}"
        );

        // The resource reports the stream the start recorded, and reaches `started` on the
        // clock without any further call of this test's own.
        poll_until_stream(&router, row, STARTED).await;
        let running = envelope(&router, row).await;
        assert_member(case, &running, "ActivityStreamMode", row.mode);
        assert_member(case, &running, "ActivityStreamKmsKeyId", &row.kms_key_id());
        assert_member(
            case,
            &running,
            "ActivityStreamKinesisStreamName",
            &expected_kinesis,
        );
        if row.kind == Kind::Cluster {
            // The cluster envelope of this front door carries the status under a member name
            // of its own as well, and the two always agree.
            assert_member(case, &running, "DBActivityStreamStatus", STARTED);
        }
        let reported_audit = optional_text(
            &running,
            "ActivityStreamEngineNativeAuditFieldsIncluded",
        );
        assert_eq!(
            reported_audit, expected_audit,
            "case {case:?}: expected the resource to report \
             ActivityStreamEngineNativeAuditFieldsIncluded {expected_audit:?}, got \
             {reported_audit:?} in {running}"
        );

        // A second start of a running stream is refused: its mode and key are the
        // configuration of the stream that is running.
        refused_as_invalid_state(
            row,
            start_row(&router, row).await,
            ACTIVITY_STREAM_ALREADY_STARTED_MESSAGE,
        );

        // The stop reports the delivery target and the key the START recorded, `stopping`.
        let stopped = expect_ok(case, stop(&router, &row.arn(), Some(true)).await);
        assert_member(case, &stopped, "Status", STOPPING);
        assert_member(case, &stopped, "KinesisStreamName", &expected_kinesis);
        assert_member(case, &stopped, "KmsKeyId", &row.kms_key_id());

        poll_until_stream(&router, row, STOPPED).await;
        let after = envelope(&router, row).await;
        for member in [
            "ActivityStreamMode",
            "ActivityStreamKmsKeyId",
            "ActivityStreamKinesisStreamName",
            "ActivityStreamEngineNativeAuditFieldsIncluded",
        ] {
            let reported = optional_text(&after, member);
            assert_eq!(
                reported, None,
                "case {case:?}: expected no {member} once the stream is stopped, got \
                 {reported:?} in {after}"
            );
        }

        // And a stop of a stream that is not running is refused, as is a stop of one that
        // never ran.
        refused_as_invalid_state(
            row,
            stop(&router, &row.arn(), Some(true)).await,
            ACTIVITY_STREAM_NOT_STARTED_MESSAGE,
        );

        // The stream can be started again once it is stopped, which is how its configuration
        // is changed, and the resource reads the NEW mode back.
        let restart_mode = if row.mode == ASYNC { SYNC } else { ASYNC };
        let restarted = expect_ok(
            case,
            start(
                &router,
                &row.arn(),
                restart_mode,
                &row.kms_key_id(),
                row.audit_fields,
                Some(true),
            )
            .await,
        );
        assert_member(case, &restarted, "Mode", restart_mode);
        poll_until_stream(&router, row, STARTED).await;
        assert_member(
            case,
            &envelope(&router, row).await,
            "ActivityStreamMode",
            restart_mode,
        );
    }
}

/// Assert a refusal is the invalid-state fault of the row's own resource kind, with `message`.
fn refused_as_invalid_state(row: &StreamCase, response: (u16, String), message: &str) {
    let code = match row.kind {
        Kind::Cluster => "InvalidDBClusterStateFault",
        Kind::Instance => "InvalidDBInstanceState",
    };
    let refused = expect_error(row.case, response, 400, code);
    assert_member(row.case, &refused, "Message", message);
}

/// RULE: `ApplyImmediately` decides WHEN the start comes due — "as soon as possible,
/// regardless of the maintenance window for the database" where it is enabled, and in the
/// resource's next maintenance window where it is not.
#[tokio::test]
async fn a_start_left_for_the_window_waits_for_it() {
    let router = emulator();

    // One cluster whose window is days away, so a start left for the window is not applied
    // while this test runs, and one whose window is open right now.
    let deferred = StreamCase {
        case: "a start left for a window days away",
        kind: Kind::Cluster,
        identifier: "emu-das-deferred",
        engine: "aurora-postgresql",
        class: "",
        mode: ASYNC,
        key_form: KeyForm::KeyArn,
        key: "aa11bb22-cc33-dd44-ee55-ff6677889900",
        audit_fields: None,
    };
    let in_window = StreamCase {
        case: "a start left for a window that is open",
        identifier: "emu-das-in-window",
        ..deferred
    };
    let deferred_off = StreamCase {
        case: "a start that disabled ApplyImmediately",
        identifier: "emu-das-deferred-off",
        ..deferred
    };
    let immediate = StreamCase {
        case: "a start asked for immediately",
        identifier: "emu-das-immediate",
        ..deferred
    };
    create_available(&router, &deferred, Some(&window_days_ahead())).await;
    create_available(&router, &deferred_off, Some(&window_days_ahead())).await;
    create_available(&router, &in_window, Some(&window_that_started())).await;
    create_available(&router, &immediate, Some(&window_days_ahead())).await;

    // A start that names no `ApplyImmediately` at all, and one that disables it, both wait
    // for the window — each on a cluster of its own, since a stream is started once.
    for row in [&deferred, &deferred_off] {
        let asked = (row.identifier == deferred_off.identifier).then_some(false);
        let started = expect_ok(
            row.case,
            start(
                &router,
                &row.arn(),
                row.mode,
                &row.kms_key_id(),
                None,
                asked,
            )
            .await,
        );
        assert_member(row.case, &started, "Status", STARTING);
        assert_member(row.case, &started, "ApplyImmediately", "false");
    }

    // The clock has moved on: the start that asked to apply at once has settled, which is
    // what this loop waits for rather than for any span of time.
    expect_ok(immediate.case, start_row(&router, &immediate).await);
    poll_until_stream(&router, &immediate, STARTED).await;

    // The deferred starts are still on their way, because their window has not come round.
    for row in [&deferred, &deferred_off] {
        assert_member(
            row.case,
            &envelope(&router, row).await,
            "ActivityStreamStatus",
            STARTING,
        );
    }

    // And a start left for a window that IS open is due at once, which is what makes the
    // window — rather than the parameter alone — what the deferred start waits on.
    expect_ok(
        in_window.case,
        start(
            &router,
            &in_window.arn(),
            in_window.mode,
            &in_window.kms_key_id(),
            None,
            Some(false),
        )
        .await,
    );
    poll_until_stream(&router, &in_window, STARTED).await;
}

/// One resource whose engine has no database activity stream at all.
struct EngineCase {
    case: &'static str,
    kind: Kind,
    identifier: &'static str,
    engine: &'static str,
    class: &'static str,
}

/// The engines with no stream to start: the cluster engines of the sibling control planes
/// this front door also serves, which are not Aurora, and the instance engine families
/// Amazon RDS serves no database activity stream for.
const ENGINES_WITHOUT_A_STREAM: &[EngineCase] = &[
    EngineCase {
        case: "a docdb cluster",
        kind: Kind::Cluster,
        identifier: "emu-das-docdb",
        engine: "docdb",
        class: "",
    },
    EngineCase {
        case: "a mysql instance",
        kind: Kind::Instance,
        identifier: "emu-das-mysql",
        engine: "mysql",
        class: "db.t3.medium",
    },
    EngineCase {
        case: "a postgres instance",
        kind: Kind::Instance,
        identifier: "emu-das-postgres",
        engine: "postgres",
        class: "db.t3.medium",
    },
    EngineCase {
        // RDS Custom supports no database activity stream, so an edition of a served family
        // has none either.
        case: "a custom-sqlserver-ee instance",
        kind: Kind::Instance,
        identifier: "emu-das-custom-sqlserver",
        engine: "custom-sqlserver-ee",
        class: "db.m5.large",
    },
];

/// RULE: a resource whose engine has no database activity stream is refused with the
/// invalid-state fault of its own resource kind, and nothing is written for it.
#[tokio::test]
async fn an_engine_with_no_activity_stream_is_refused_on_its_engine() {
    let router = emulator();

    for row in ENGINES_WITHOUT_A_STREAM {
        let resource = StreamCase {
            case: row.case,
            kind: row.kind,
            identifier: row.identifier,
            engine: row.engine,
            class: row.class,
            mode: ASYNC,
            key_form: KeyForm::KeyArn,
            key: "aa11bb22-cc33-dd44-ee55-ff6677889900",
            audit_fields: None,
        };
        create_available(&router, &resource, None).await;

        let expected = match row.kind {
            Kind::Cluster => ACTIVITY_STREAM_CLUSTER_ENGINE_MESSAGE,
            Kind::Instance => ACTIVITY_STREAM_ENGINE_MESSAGE,
        };
        refused_as_invalid_state(
            &resource,
            start_row(&router, &resource).await,
            expected,
        );
        // The refused start wrote nothing: the resource still reports no stream.
        assert_member(
            row.case,
            &envelope(&router, &resource).await,
            "ActivityStreamStatus",
            STOPPED,
        );
    }
}

/// RULE: a `ResourceArn` that names no resource of this account in the request's Region is the
/// not-found fault of the resource kind the ARN names, whichever of the two calls raised it —
/// while an ARN whose REGION is not the endpoint's is refused on the member instead, since no
/// resource of another Region is one this endpoint could act on.
#[tokio::test]
async fn a_resource_arn_that_names_nothing_in_the_region_is_not_found() {
    let router = emulator();

    // One cluster and one instance in each Region, so a miss is a miss because of what the
    // ARN names rather than because the store is empty.
    let here = StreamCase {
        case: "setup",
        kind: Kind::Cluster,
        identifier: "emu-das-here",
        engine: "aurora-postgresql",
        class: "",
        mode: ASYNC,
        key_form: KeyForm::KeyArn,
        key: "aa11bb22-cc33-dd44-ee55-ff6677889900",
        audit_fields: None,
    };
    create_available(&router, &here, None).await;
    expect_ok(
        "setup",
        call_in(
            &router,
            OTHER_REGION,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", "emu-das-elsewhere"),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", "emuadmin"),
                ("MasterUserPassword", "a-strong-password-1"),
            ],
        )
        .await,
    );

    // (case, the ARN as sent, the fault's HTTP status and wire code, the wording)
    //
    // Every row names the Region the request is signed for: an ARN naming ANOTHER Region is
    // not a missing resource but a value this endpoint could not act on, which the loop after
    // this one asserts.
    let misses = [
        (
            "a cluster ARN of an identifier nothing created",
            cluster_arn(REGION, "emu-das-absent"),
            "DBClusterNotFoundFault",
        ),
        (
            "a cluster ARN of another account",
            cluster_arn(REGION, "emu-das-here").replace(ACCOUNT, "123456789012"),
            "DBClusterNotFoundFault",
        ),
        (
            "an instance ARN of an identifier nothing created",
            instance_arn(REGION, "emu-das-absent"),
            "DBInstanceNotFound",
        ),
        (
            "an instance ARN of another account",
            instance_arn(REGION, "emu-das-here").replace(ACCOUNT, "123456789012"),
            "DBInstanceNotFound",
        ),
    ];

    for (case, arn, code) in &misses {
        for refused in [
            expect_error(
                case,
                start(&router, arn, ASYNC, "alias/emu-das-key", None, Some(true)).await,
                404,
                code,
            ),
            expect_error(case, stop(&router, arn, Some(true)).await, 404, code),
        ] {
            // The cluster's fault names the ARN the request carried and the instance's the
            // handle that ARN names, which is how each of this front door's two resource
            // families words a reference it could not resolve.
            let expected = if *code == "DBClusterNotFoundFault" {
                format!("DBCluster not found: {arn}")
            } else {
                let named = arn.rsplit(':').next().expect("an ARN carries a resource");
                format!("DBInstance not found: {named}")
            };
            assert_member(case, &refused, "Message", &expected);
        }
    }

    // An ARN of ANOTHER Region is no miss at all: Amazon RDS serves one Region per endpoint,
    // so such a value names nothing either call could act on and is refused on the member,
    // before the lookup — the Region pairs and resource kinds that rule covers are the table
    // in `activity_stream_resource_arn_region`.
    for (case, arn) in [
        (
            "a cluster ARN in another Region",
            cluster_arn(OTHER_REGION, "emu-das-elsewhere"),
        ),
        (
            "an instance ARN in another Region",
            instance_arn(OTHER_REGION, "emu-das-elsewhere"),
        ),
    ] {
        for refused in [
            expect_error(
                case,
                start(&router, &arn, ASYNC, "alias/emu-das-key", None, Some(true)).await,
                400,
                "InvalidParameterValue",
            ),
            expect_error(
                case,
                stop(&router, &arn, Some(true)).await,
                400,
                "InvalidParameterValue",
            ),
        ] {
            // The sentence Amazon RDS refuses an ARN parameter of another Region with: its
            // fixed text, with the ARN as submitted, the member, the Region the endpoint
            // serves and the Region in the ARN as the variable parts.
            let expected = format!(
                "The provided ARN ({arn}) is invalid for this parameter (ResourceArn). \
                 Expected region = {REGION}, actual region = {OTHER_REGION}"
            );
            assert_member(case, &refused, "Message", &expected);
        }
    }
}

/// RULE: a value that is no Amazon RDS DB cluster or DB instance ARN at all names no resource
/// a stream belongs to, so it is refused on the member that carried it rather than reported as
/// a resource that might yet appear.
#[tokio::test]
async fn a_value_that_names_no_cluster_or_instance_is_refused_on_the_member() {
    let router = emulator();

    // Every form that carries no DB cluster or DB instance ARN: a bare identifier, an ARN of
    // another of this service's resource types, an ARN of another service, and a value that
    // is no ARN at all.
    let unusable = [
        "emu-das-member".to_string(),
        cluster_arn(REGION, "emu-das-member").replace(":cluster:", ":cluster-snapshot:"),
        cluster_arn(REGION, "emu-das-member").replace(":cluster:", ":og:"),
        format!("arn:aws:kinesis:{REGION}:{ACCOUNT}:stream/aws-rds-das-cluster-ABC"),
        "not-an-arn-at-all".to_string(),
    ];
    for value in &unusable {
        for refused in [
            expect_error(
                value,
                start(&router, value, ASYNC, "alias/emu-das-key", None, Some(true)).await,
                400,
                "InvalidParameterValue",
            ),
            expect_error(
                value,
                stop(&router, value, Some(true)).await,
                400,
                "InvalidParameterValue",
            ),
        ] {
            assert_member(value, &refused, "Message", &format!("Invalid resource name: {value}"));
        }
    }

    // And a call that names no resource at all. A member the model requires and the request
    // omits is refused before the handler runs, by the parameter sentence this front door
    // answers an absent required member with; a member that is present but EMPTY carries no
    // reference either, and is refused by the operation itself.
    let absent = "Please specify a value for parameter 'resourceArn'.";
    let blank = "The parameter ResourceArn must be provided and must not be blank.";
    for (case, params, expected) in [
        (
            "ResourceArn absent",
            vec![("Mode", ASYNC), ("KmsKeyId", "alias/emu-das-key")],
            absent,
        ),
        (
            "ResourceArn empty",
            vec![
                ("ResourceArn", ""),
                ("Mode", ASYNC),
                ("KmsKeyId", "alias/emu-das-key"),
            ],
            blank,
        ),
    ] {
        let refused = expect_error(
            case,
            call(&router, "StartActivityStream", &params).await,
            400,
            "InvalidParameterValue",
        );
        assert_member(case, &refused, "Message", expected);
    }
    for (case, params, expected) in [
        ("ResourceArn absent", Vec::new(), absent),
        ("ResourceArn empty", vec![("ResourceArn", "")], blank),
    ] {
        let refused = expect_error(
            case,
            call(&router, "StopActivityStream", &params).await,
            400,
            "InvalidParameterValue",
        );
        assert_member(case, &refused, "Message", expected);
    }
}

/// A router carrying this service beside the one that OWNS data streams — Amazon Kinesis —
/// over ONE shared engine, exactly as the assembled router mounts them.
fn emulator_with_kinesis() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Verifying signatures is the emulator's concern, not this service's; these tests sign
    // nothing, so the check is off and the scope header is read for routing only.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(Arc::clone(&engine)),
    );
    router.register_service(
        service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(Arc::clone(&engine)),
    );
    router
}

/// One Amazon Kinesis call (`awsJson1_1`, `POST /` carrying `X-Amz-Target`), as the HTTP
/// status and the JSON body it answered with.
async fn kinesis_call(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
) -> (u16, String) {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://kinesis.{REGION}.amazonaws.com/"))
        .header("content-type", "application/x-amz-json-1.1")
        .header("x-amz-target", format!("Kinesis_20131202.{operation}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEMULATOR/20260101/{REGION}/kinesis/\
                 aws4_request, SignedHeaders=host, Signature=unverified"
            ),
        )
        .body(bytes::Bytes::from(body.to_string()))
        .expect("a well-formed test request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let answered = String::from_utf8(response.body().to_vec()).expect("a UTF-8 response body");
    (status, answered)
}

/// RULE: the Kinesis data stream a start reports as the delivery target is a REAL data stream
/// of this emulator's Kinesis — created by the start, read back through Kinesis's own
/// operations — and the stop takes it away again.
#[tokio::test]
async fn the_delivery_target_a_start_reports_is_a_real_kinesis_data_stream() {
    let router = emulator_with_kinesis();

    // Both resource kinds a stream belongs to, since each derives its delivery target from
    // its own resource id.
    let rows = [
        StreamCase {
            case: "an aurora-postgresql cluster's delivery target",
            kind: Kind::Cluster,
            identifier: "emu-das-target-cluster",
            engine: "aurora-postgresql",
            class: "",
            mode: ASYNC,
            key_form: KeyForm::KeyArn,
            key: "aa11bb22-cc33-dd44-ee55-ff6677889900",
            audit_fields: None,
        },
        StreamCase {
            case: "an oracle-se2 instance's delivery target",
            kind: Kind::Instance,
            identifier: "emu-das-target-instance",
            engine: "oracle-se2",
            class: "db.m5.large",
            mode: SYNC,
            key_form: KeyForm::AliasName,
            key: "emu-das-key",
            audit_fields: Some(true),
        },
    ];

    for row in &rows {
        let case = row.case;
        create_available(&router, row, None).await;
        let started = expect_ok(case, start_row(&router, row).await);
        let reported = text(&started, "KinesisStreamName");

        // Kinesis itself answers for the stream the start named, under that very name: the
        // delivery target exists because its OWNER holds it, not because this service said so.
        let summary = json!({ "StreamName": reported.clone() });
        let (status, body) = kinesis_call(&router, "DescribeStreamSummary", &summary).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected Kinesis to hold the data stream {reported:?}, got HTTP \
             {status} with {body}"
        );
        let described: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
        let held = described["StreamDescriptionSummary"]["StreamName"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            held, reported,
            "case {case:?}: expected Kinesis to report the stream {reported:?}, got {held:?} \
             in {body}"
        );
        // Its capacity is Amazon RDS's to manage, so the stream it created is an on-demand one.
        let mode = described["StreamDescriptionSummary"]["StreamModeDetails"]["StreamMode"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            mode, "ON_DEMAND",
            "case {case:?}: expected the data stream to be ON_DEMAND, got {mode:?} in {body}"
        );

        // And the stop takes the delivery target away again: Kinesis stops answering for it
        // once the deletion it accepted has run its course.
        expect_ok(case, stop(&router, &row.arn(), Some(true)).await);
        poll_until(
            case,
            &format!("Kinesis no longer holding the data stream {reported}"),
            || async { kinesis_call(&router, "DescribeStreamSummary", &summary).await },
            |status, body| status != 200 && body.contains("ResourceNotFoundException"),
        )
        .await;
    }
}

