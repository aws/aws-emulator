// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the REGION of the `ResourceArn` the three database-activity-stream
//! operations (`ModifyActivityStream`, `StartActivityStream`, `StopActivityStream`) address
//! their resource by.
//!
//! The rule under test: each of them takes a `ResourceArn` — "the Amazon Resource Name (ARN)"
//! of the DB instance or DB cluster the call acts on — and Amazon RDS serves one Region per
//! endpoint, so a well-formed ARN whose Region field is not the Region the endpoint serves
//! names nothing that endpoint could ever act on; the value is refused ON THE MEMBER, before
//! any lookup, with HTTP 400, the Query API's `InvalidParameterValue` and the sentence that
//! states the member and both Regions. None of the faults the three operations model
//! (`smithy_query(query="operation", operation="ModifyActivityStream")`:
//! `DBInstanceNotFoundFault` 404, `InvalidDBInstanceStateFault` 400, `ResourceNotFoundFault`
//! 404) covers such a value, and reporting it as a missing resource would tell a client
//! branching on not-found that the resource it holds in another Region is gone.
//!
//! The table spans what the rule generalises over: both resource kinds a stream belongs to,
//! all three operations, both directions of the Region pair (so the Region the refusal
//! reports as expected is read from the endpoint and not from a constant), four Regions in
//! the ARN, and whether a resource of that identifier is held by the endpoint's Region, by
//! the ARN's Region, or by neither.
//!
//! Every call goes over the wire through the harness in `db_instances.rs`, so the assertions
//! read the XML the emulator itself answered with, and every wait is the poll loop a client
//! writes (`lifecycle_poll_loops::poll_until`) rather than a sleep.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{call_in, emulator, expect_error, expect_ok, optional_text, text};
use crate::lifecycle_poll_loops::poll_until;

/// The canonical account id every generated emulator presents.
const ACCOUNT: &str = "000000000000";

/// The Region whose endpoint serves most of the calls below.
const HOME: &str = "us-west-2";

/// A second Region, whose endpoint serves the rows that drive the pair the other way round.
const AWAY: &str = "us-east-1";

/// The `db` resource type of a DB instance ARN, and the `cluster` one of a DB cluster ARN
/// ("Constructing an ARN for Amazon RDS").
const DB: &str = "db";
const CLUSTER: &str = "cluster";

/// An instance the engine families `ModifyActivityStream` is published for — "This operation
/// is supported for RDS for Oracle and Microsoft SQL Server" — runs, held by [`HOME`].
const HOME_INSTANCE: &str = "emu-asrr-home";

/// An instance of the same engine, held by [`AWAY`] only.
const AWAY_INSTANCE: &str = "emu-asrr-away";

/// An Aurora cluster held by [`HOME`], so the cluster rows name a resource that exists.
const HOME_CLUSTER: &str = "emu-asrr-cluster";

/// An identifier no Region holds a resource of.
const ABSENT_INSTANCE: &str = "emu-asrr-absent";

/// The `unlocked` member of `AuditPolicyState`, which the refused and the served
/// modifications below ask for.
const UNLOCKED: &str = "unlocked";

/// The `unlocking-policy` member of `ActivityStreamPolicyStatus`: the state a modification
/// that asked for [`UNLOCKED`] reports until this service's clock makes the change due.
const UNLOCKING: &str = "unlocking-policy";

/// The `async` member of `ActivityStreamMode`, which the starts below ask for.
const ASYNC: &str = "async";

/// The `starting` member of `ActivityStreamStatus`, which an accepted start reports.
const STARTING: &str = "starting";

/// The `stopped` member of `ActivityStreamStatus`: the status of the activity stream of a
/// resource no start recorded one on.
const STOPPED: &str = "stopped";

/// The `available` status a created DB instance or DB cluster settles in.
const AVAILABLE: &str = "available";

/// The `arn:<partition>:rds:<region>:<account>:<type>:<identifier>` form Amazon RDS gives a
/// DB instance and a DB cluster ("Constructing an ARN for Amazon RDS"), written out here
/// rather than taken from the code under test.
fn resource_arn(region: &str, resource_type: &str, identifier: &str) -> String {
    format!("arn:aws:rds:{region}:{ACCOUNT}:{resource_type}:{identifier}")
}

/// The sentence Amazon RDS refuses an ARN parameter naming another Region with: the fixed
/// text "The provided ARN (", ") is invalid for this parameter (", "). Expected region = "
/// and ", actual region = ", with the ARN exactly as the request submitted it, the input
/// member the value arrived in, the Region the endpoint serves and the Region read out of the
/// ARN as its variable parts. Written out here rather than taken from the code under test.
fn cross_region_message(arn: &str, expected_region: &str, arn_region: &str) -> String {
    format!(
        "The provided ARN ({arn}) is invalid for this parameter (ResourceArn). Expected \
         region = {expected_region}, actual region = {arn_region}"
    )
}

/// The three operations that address a DB instance's activity stream by `ResourceArn`.
const INSTANCE_OPERATIONS: &[&str] = &[
    "ModifyActivityStream",
    "StartActivityStream",
    "StopActivityStream",
];

/// The two of them a DB cluster ARN can name a resource for: the audit policy
/// `ModifyActivityStream` writes belongs to a DB INSTANCE ("the ARN of the RDS for Oracle or
/// Microsoft SQL Server DB instance").
const CLUSTER_OPERATIONS: &[&str] = &["StartActivityStream", "StopActivityStream"];

/// One of the three calls on the activity stream of `arn`, sent to the endpoint of `region`
/// with the members that operation requires.
async fn call_on(
    router: &MultiServiceRouter,
    region: &str,
    operation: &str,
    arn: &str,
) -> (u16, String) {
    let params: Vec<(&str, String)> = match operation {
        "ModifyActivityStream" => vec![
            ("ResourceArn", arn.to_string()),
            ("AuditPolicyState", UNLOCKED.to_string()),
        ],
        "StartActivityStream" => vec![
            ("ResourceArn", arn.to_string()),
            ("Mode", ASYNC.to_string()),
            ("KmsKeyId", "alias/emu-asrr-key".to_string()),
            ("ApplyImmediately", "true".to_string()),
        ],
        "StopActivityStream" => vec![
            ("ResourceArn", arn.to_string()),
            ("ApplyImmediately", "true".to_string()),
        ],
        other => panic!("{other} does not address its resource by ResourceArn"),
    };
    call_in(router, region, operation, &params).await
}

/// `CreateDBInstance` in `region`, with the engine family the audit policy is published for.
async fn create_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "CreateDBInstance",
        &[
            ("DBInstanceIdentifier", identifier),
            ("Engine", "oracle-se2"),
            ("DBInstanceClass", "db.m5.large"),
        ],
    )
    .await
}

/// `DescribeDBInstances` for one instance, from the endpoint of `region`.
async fn describe_instance(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBInstances",
        &[("DBInstanceIdentifier", identifier)],
    )
    .await
}

/// `DescribeDBClusters` for one cluster, from the endpoint of `region`.
async fn describe_cluster(
    router: &MultiServiceRouter,
    region: &str,
    identifier: &str,
) -> (u16, String) {
    call_in(
        router,
        region,
        "DescribeDBClusters",
        &[("DBClusterIdentifier", identifier)],
    )
    .await
}

/// A router holding the resources the rows name: an instance and an Aurora cluster in
/// [`HOME`], and an instance of the same engine in [`AWAY`] — each polled until it reports
/// `available`, so a refusal below is never a refusal about a resource still coming up.
async fn fixture() -> MultiServiceRouter {
    let router = emulator();
    expect_ok("setup", create_instance(&router, HOME, HOME_INSTANCE).await);
    expect_ok("setup", create_instance(&router, AWAY, AWAY_INSTANCE).await);
    expect_ok(
        "setup",
        call_in(
            &router,
            HOME,
            "CreateDBCluster",
            &[
                ("DBClusterIdentifier", HOME_CLUSTER),
                ("Engine", "aurora-postgresql"),
                ("MasterUsername", "emuadmin"),
                ("MasterUserPassword", "a-strong-password-1"),
            ],
        )
        .await,
    );
    for (region, identifier) in [(HOME, HOME_INSTANCE), (AWAY, AWAY_INSTANCE)] {
        poll_until(
            "setup",
            &format!("{identifier} reaching {AVAILABLE}"),
            || async { describe_instance(&router, region, identifier).await },
            |status, body| status == 200 && text(body, "DBInstanceStatus") == AVAILABLE,
        )
        .await;
    }
    poll_until(
        "setup",
        &format!("{HOME_CLUSTER} reaching {AVAILABLE}"),
        || async { describe_cluster(&router, HOME, HOME_CLUSTER).await },
        |status, body| status == 200 && text(body, "Status") == AVAILABLE,
    )
    .await;
    router
}

/// One `ResourceArn` whose Region is not the one the endpoint serves.
struct Row {
    case: &'static str,
    /// The Region the refused call is sent to.
    endpoint_region: &'static str,
    /// The Region written in the ARN it carries.
    arn_region: &'static str,
    /// The resource type the ARN names, which decides which operations can take it.
    resource_type: &'static str,
    /// The identifier the ARN names.
    identifier: &'static str,
}

/// RULE: a `ResourceArn` naming another Region than the endpoint's is refused on the member
/// with HTTP 400 `InvalidParameterValue`, whichever of the three calls carried it and whether
/// or not a resource of that identifier exists anywhere.
#[tokio::test]
async fn a_resource_arn_of_another_region_is_refused_on_the_member() {
    let router = fixture().await;

    let rows = [
        Row {
            case: "an instance this Region holds, named under another Region",
            endpoint_region: HOME,
            arn_region: AWAY,
            resource_type: DB,
            identifier: HOME_INSTANCE,
        },
        Row {
            case: "an instance only the ARN's Region holds",
            endpoint_region: HOME,
            arn_region: AWAY,
            resource_type: DB,
            identifier: AWAY_INSTANCE,
        },
        Row {
            case: "an instance no Region holds, under another Region",
            endpoint_region: HOME,
            arn_region: "eu-central-1",
            resource_type: DB,
            identifier: ABSENT_INSTANCE,
        },
        Row {
            case: "a cluster this Region holds, named under another Region",
            endpoint_region: HOME,
            arn_region: "ap-southeast-2",
            resource_type: CLUSTER,
            identifier: HOME_CLUSTER,
        },
        Row {
            case: "a cluster no Region holds, under another Region",
            endpoint_region: HOME,
            arn_region: AWAY,
            resource_type: CLUSTER,
            identifier: "emu-asrr-absent-cluster",
        },
        Row {
            case: "the pair the other way round: this Region's instance named from the other",
            endpoint_region: AWAY,
            arn_region: HOME,
            resource_type: DB,
            identifier: HOME_INSTANCE,
        },
        Row {
            case: "the pair the other way round: the local instance under the other Region",
            endpoint_region: AWAY,
            arn_region: HOME,
            resource_type: DB,
            identifier: AWAY_INSTANCE,
        },
    ];

    for row in &rows {
        let arn = resource_arn(row.arn_region, row.resource_type, row.identifier);
        let operations = match row.resource_type {
            DB => INSTANCE_OPERATIONS,
            _ => CLUSTER_OPERATIONS,
        };
        for operation in operations {
            let case = format!("{} through {operation}", row.case);
            let refused = expect_error(
                &case,
                call_on(&router, row.endpoint_region, operation, &arn).await,
                400,
                "InvalidParameterValue",
            );
            let message = text(&refused, "Message");
            let expected = cross_region_message(&arn, row.endpoint_region, row.arn_region);
            assert_eq!(
                message, expected,
                "case {case:?}: expected the refusal to read {expected:?}, got {message:?}"
            );
            // A value the member does not accept is the CALLER's to correct, which is what
            // the envelope's fault type tells a client that branches on it.
            let fault_type = text(&refused, "Type");
            assert_eq!(
                fault_type, "Sender",
                "case {case:?}: expected a Sender fault, got {fault_type:?} in {refused}"
            );
        }
    }

    // None of the refused calls touched a resource: the refusal is answered before the
    // lookup, so the instance and the cluster of the endpoint's own Region still report the
    // activity stream and audit policy of a resource nothing was asked of.
    let instance = expect_ok(
        "the home instance after the refusals",
        describe_instance(&router, HOME, HOME_INSTANCE).await,
    );
    let stream = text(&instance, "ActivityStreamStatus");
    assert_eq!(
        stream, STOPPED,
        "expected the refused calls to leave {HOME_INSTANCE} reporting \
         ActivityStreamStatus {STOPPED:?}, got {stream:?} in {instance}"
    );
    let policy = optional_text(&instance, "ActivityStreamPolicyStatus");
    assert!(
        policy.is_none(),
        "expected the refused calls to leave {HOME_INSTANCE} without an \
         ActivityStreamPolicyStatus, got {policy:?}"
    );
    let cluster = expect_ok(
        "the home cluster after the refusals",
        describe_cluster(&router, HOME, HOME_CLUSTER).await,
    );
    let cluster_stream = text(&cluster, "ActivityStreamStatus");
    assert_eq!(
        cluster_stream, STOPPED,
        "expected the refused calls to leave {HOME_CLUSTER} reporting \
         ActivityStreamStatus {STOPPED:?}, got {cluster_stream:?} in {cluster}"
    );
}

/// RULE: the gate is about the ARN's REGION and nothing else — an ARN naming the endpoint's
/// own Region is served, and one naming that Region but no stored resource is still the
/// operation's not-found fault.
#[tokio::test]
async fn an_arn_of_the_endpoints_own_region_is_resolved_as_before() {
    let router = fixture().await;

    // The served call, through the operation the issue's ARN was carried by: the same
    // identifier and account, with the endpoint's Region in the ARN.
    let modified = expect_ok(
        "the instance of the endpoint's Region",
        call_on(
            &router,
            HOME,
            "ModifyActivityStream",
            &resource_arn(HOME, DB, HOME_INSTANCE),
        )
        .await,
    );
    let policy = text(&modified, "PolicyStatus");
    assert_eq!(
        policy, UNLOCKING,
        "expected the accepted modification to report PolicyStatus {UNLOCKING:?}, got \
         {policy:?} in {modified}"
    );

    // And through the operation a cluster ARN is taken by.
    let started = expect_ok(
        "the cluster of the endpoint's Region",
        call_on(
            &router,
            HOME,
            "StartActivityStream",
            &resource_arn(HOME, CLUSTER, HOME_CLUSTER),
        )
        .await,
    );
    let status = text(&started, "Status");
    assert_eq!(
        status, STARTING,
        "expected the accepted start to report Status {STARTING:?}, got {status:?} in \
         {started}"
    );

    // Each Region's endpoint serves its OWN instance of the pair, so the Region the gate
    // compares against is the request's and not a constant.
    let away = expect_ok(
        "the instance of the other Region, from its own endpoint",
        call_on(
            &router,
            AWAY,
            "ModifyActivityStream",
            &resource_arn(AWAY, DB, AWAY_INSTANCE),
        )
        .await,
    );
    let away_policy = text(&away, "PolicyStatus");
    assert_eq!(
        away_policy, UNLOCKING,
        "expected the accepted modification to report PolicyStatus {UNLOCKING:?}, got \
         {away_policy:?} in {away}"
    );

    // A well-formed ARN of the endpoint's own Region naming no stored instance is still the
    // modelled not-found fault, named by the handle the ARN carries.
    let absent = resource_arn(HOME, DB, ABSENT_INSTANCE);
    let refused = expect_error(
        "an instance of this Region that was never created",
        call_on(&router, HOME, "ModifyActivityStream", &absent).await,
        404,
        "DBInstanceNotFound",
    );
    let message = text(&refused, "Message");
    let expected = format!("DBInstance not found: {ABSENT_INSTANCE}");
    assert_eq!(
        message, expected,
        "expected the not-found fault to read {expected:?}, got {message:?}"
    );
}
