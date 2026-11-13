// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHICH class of answer a refusal belongs to, decided by what makes
//! the call impossible.
//!
//! The rule under test, asserted over tables that span the class rather than over one
//! request:
//!
//! 1. A call refused for what the REQUEST carries — a member value outside the domain the
//!    parameter accepts, or members (or a member and the stored configuration of the
//!    resource the call addresses) that cannot hold together — is answered with the Query
//!    API's own parameter codes at HTTP 400: `InvalidParameterValue` for the value and
//!    `InvalidParameterCombination` for the combination. A client branches on the code to
//!    decide whether to retry, so a request error must never arrive as a resource-state
//!    fault (which reads as "wait and try again") nor as the framework's synthetic
//!    `ValidationException`, which is no Amazon RDS error code at all.
//! 2. A call refused because the resource its required identifier NAMES does not exist is
//!    answered with the not-found fault that operation's own model declares, with that
//!    model's status.
//! 3. A call refused for the addressed resource's own lifecycle state keeps the modelled
//!    state fault, and its sentence is worded from the record: a status the resource will
//!    LEAVE states the state the call needs, and one it cannot leave names that status and
//!    that it is terminal.
//! 4. None of this refuses what the service does serve: the same calls succeed once the
//!    conflict is gone, and a refused modification stores nothing.
//!
//! Every call goes over the wire: the service is mounted on a `MultiServiceRouter` exactly
//! as the production router mounts it (`crate::db_instances::emulator`), each request is a
//! real awsQuery form POST carrying the SigV4 credential scope the router keys on, and
//! every assertion reads the XML the emulator answered with.

use smithy_server_runtime::MultiServiceRouter;

use crate::db_instances::{
    call, emulator, expect_error, expect_ok, text, texts, wait_for_settle, REGION,
};

/// The canonical account id every generated emulator presents, so every ARN below is one
/// this service could have minted.
const ACCOUNT: &str = "000000000000";

// The sentences this service's refusals must carry, spelled out HERE rather than taken
// from the code under test, so the table pins the wording instead of re-stating whatever
// the implementation happens to say. Each is the text the recorded answers of this front
// door carry, including which parts Amazon RDS does NOT interpolate: the deletion-protection
// sentences name the resource KIND and not the identifier the request sent, and the
// final-snapshot, force-failover, subnet-list and modification-state sentences have no
// variable part at all.

/// `DeleteDBCluster` on a cluster whose deletion protection is on.
const PROTECTED_CLUSTER: &str =
    "Cannot delete protected Cluster, please disable deletion protection and try again.";

/// `DeleteDBInstance` on an instance whose deletion protection is on.
const PROTECTED_DB_INSTANCE: &str = "Cannot delete protected DB Instance, please disable \
                                    deletion protection and try again.";

/// `DeleteGlobalCluster` on a global cluster whose deletion protection is on.
const PROTECTED_GLOBAL_CLUSTER: &str = "Cannot delete protected Global Cluster, please \
                                        disable deletion protection and try again.";

/// A delete that names BOTH of its final-snapshot members.
const ONLY_ONE_SNAPSHOT_MEMBER: &str =
    "Only one of SkipFinalSnapshot and FinalDBSnapshotIdentifier can be specified";

/// A delete that names NEITHER of them.
const A_SNAPSHOT_MEMBER_IS_REQUIRED: &str =
    "FinalDBSnapshotIdentifier is required unless SkipFinalSnapshot is specified.";

/// `RebootDBInstance` forcing a failover on an instance that is not configured for it.
const FORCE_FAILOVER_NEEDS_MULTI_AZ: &str = "ForceFailover cannot be specified since the \
                                             instance is not configured for either MultiAZ \
                                             or High Availability";

/// `CreateDBCluster` asking for an engine mode the resolved version does not run. The mode
/// the request named is the one variable part.
const UNAVAILABLE_SERVERLESS_MODE: &str =
    "The engine mode serverless you requested is currently unavailable.";

/// A `SubnetIds` list that names no subnet.
const SUBNET_IDS_REQUIRED: &str = "Subnet IDs are required.";

/// `ModifyDBInstance` on an instance in a status it will LEAVE.
const MODIFICATION_NEEDS_AVAILABLE: &str = "Database instance is not in available state.";

/// `ModifyDBInstance` on an instance in the one status it never leaves, which the sentence
/// names together with the fact that it is terminal.
const MODIFICATION_IN_A_TERMINAL_STATE: &str = "The specified database instance is currently \
                                                in deleting state, which is a terminal \
                                                state.";

/// The Query API's own code for a member value the service will not serve.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// The Query API's own code for members that cannot be served together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// The `awsQueryError` code of `InvalidDBInstanceStateFault`.
const INVALID_DB_INSTANCE_STATE: &str = "InvalidDBInstanceState";

/// Two EC2 subnet ids in the two forms the id takes (8 and 17 hexadecimal characters),
/// which between them cover the two Availability Zones a DB subnet group needs.
const TWO_SUBNETS: [&str; 2] = ["subnet-1a2b3c4d", "subnet-0a1b2c3d4e5f60711"];

/// How a row says its refusal must be worded.
#[derive(Debug, Clone)]
enum Wording {
    /// The whole sentence, for a refusal whose text Amazon RDS fixes.
    Exact(String),
    /// A fragment the sentence must carry, so the row cannot pass on a refusal of
    /// something else.
    Says(&'static str),
}

/// One row of the refusal table: a call, and the class of answer it must get.
struct Row {
    case: &'static str,
    action: &'static str,
    params: Vec<(&'static str, String)>,
    status: u16,
    code: &'static str,
    wording: Wording,
}

/// `[(name, value.to_string()), ..]` — the query parameters of one call.
///
/// `Vec::from` rather than `vec!`, so one spelling serves both a row's owned parameter
/// list and a call that only borrows it.
macro_rules! params {
    ($($name:expr => $value:expr),* $(,)?) => {
        Vec::from([$(($name, $value.to_string())),*])
    };
}

/// `CreateDBInstance` with its three required members plus `extra`.
async fn create_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = params![
        "DBInstanceIdentifier" => identifier,
        "Engine" => "postgres",
        "DBInstanceClass" => "db.t3.micro",
    ];
    request.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBInstance", &request).await
}

/// Create a DB instance and wait until the clock has made it `available`.
async fn create_available_instance(
    router: &MultiServiceRouter,
    identifier: &str,
    extra: &[(&str, &str)],
) {
    expect_ok(
        &format!("create {identifier}"),
        create_instance(router, identifier, extra).await,
    );
    wait_for_settle();
}

/// `CreateDBCluster` with its two required members plus `extra`.
async fn create_cluster(
    router: &MultiServiceRouter,
    identifier: &str,
    engine: &str,
    extra: &[(&str, &str)],
) -> (u16, String) {
    let mut request: Vec<(&str, String)> = params![
        "DBClusterIdentifier" => identifier,
        "Engine" => engine,
    ];
    request.extend(
        extra
            .iter()
            .map(|(name, value)| (*name, (*value).to_string())),
    );
    call(router, "CreateDBCluster", &request).await
}

/// `CreateDBSubnetGroup` over `subnets`, which may be empty.
///
/// An EMPTY list is sent the way the AWS SDKs serialise one on this query protocol — the
/// member name with no value — so the request carries `SubnetIds` and names no subnet,
/// rather than omitting the required member altogether (which the boundary validator
/// refuses before any handler runs).
async fn create_subnet_group(
    router: &MultiServiceRouter,
    name: &str,
    subnets: &[&str],
) -> (u16, String) {
    let mut request: Vec<(String, String)> = vec![
        ("DBSubnetGroupName".to_string(), name.to_string()),
        ("DBSubnetGroupDescription".to_string(), "as created".to_string()),
    ];
    if subnets.is_empty() {
        request.push(("SubnetIds".to_string(), String::new()));
    }
    for (index, subnet) in subnets.iter().enumerate() {
        request.push((
            format!("SubnetIds.SubnetIdentifier.{}", index + 1),
            (*subnet).to_string(),
        ));
    }
    call(router, "CreateDBSubnetGroup", &request).await
}

/// The ARN a stored DB instance of [`REGION`] reports for itself.
fn instance_arn(identifier: &str) -> String {
    format!("arn:aws:rds:{REGION}:{ACCOUNT}:db:{identifier}")
}

/// The `DBInstanceStatus` `DescribeDBInstances` reports for `identifier`.
async fn instance_status(router: &MultiServiceRouter, case: &str, identifier: &str) -> String {
    let body = expect_ok(
        case,
        call(
            router,
            "DescribeDBInstances",
            &params!["DBInstanceIdentifier" => identifier],
        )
        .await,
    );
    text(&body, "DBInstanceStatus")
}

/// Assert `row`'s refusal: its status, its wire code and the wording of its sentence.
async fn expect_refused(router: &MultiServiceRouter, row: &Row) {
    let body = expect_error(
        row.case,
        call(router, row.action, &row.params).await,
        row.status,
        row.code,
    );
    let message = text(&body, "Message");
    match &row.wording {
        Wording::Exact(expected) => assert_eq!(
            &message, expected,
            "case {:?}: expected the refusal to read {expected:?}, got {message:?}",
            row.case
        ),
        Wording::Says(fragment) => assert!(
            message.contains(fragment),
            "case {:?}: expected the refusal to carry {fragment:?}, got {message:?}",
            row.case
        ),
    }
    // The awsQuery envelope types every client error `Sender`; a client that branches on
    // that must not read `Receiver` for a request it can correct itself.
    let fault_type = text(&body, "Type");
    assert_eq!(
        fault_type, "Sender",
        "case {:?}: expected a Sender fault, got {fault_type:?} in {body}",
        row.case
    );
}

// ---------------------------------------------------------------------------
// 1 and 2: the request's own content, and the resource it names
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_refusal_of_what_the_request_carries_is_a_query_api_parameter_error() {
    let router = emulator();

    // The resources the rows are aimed at, so each is refused for what its request carries
    // and not because this service holds nothing at all.
    expect_ok(
        "the protected cluster",
        create_cluster(
            &router,
            "emu-rcls-protected",
            "aurora-postgresql",
            &[("DeletionProtection", "true")],
        )
        .await,
    );
    expect_ok(
        "the cluster the rescales are aimed at",
        create_cluster(&router, "emu-rcls-live", "aurora-mysql", &[]).await,
    );
    expect_ok(
        "the protected global cluster",
        call(
            &router,
            "CreateGlobalCluster",
            &params![
                "GlobalClusterIdentifier" => "emu-rcls-global",
                "Engine" => "aurora-postgresql",
                "DeletionProtection" => "true",
            ],
        )
        .await,
    );
    expect_ok(
        "the subnet group the modification is aimed at",
        create_subnet_group(&router, "emu-rcls-subnets", &TWO_SUBNETS).await,
    );
    create_available_instance(&router, "emu-rcls-protected-inst", &[("DeletionProtection", "true")])
        .await;
    create_available_instance(&router, "emu-rcls-single", &[]).await;
    create_available_instance(&router, "emu-rcls-live-inst", &[]).await;

    let rows: Vec<Row> = vec![
        // Deletion protection: a setting of the addressed resource the request conflicts
        // with, on each of the three resource kinds that carry it.
        Row {
            case: "DeleteDBCluster on a deletion-protected cluster",
            action: "DeleteDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-protected",
                "SkipFinalSnapshot" => "true",
            ],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(PROTECTED_CLUSTER.to_string()),
        },
        Row {
            case: "DeleteDBInstance on a deletion-protected instance",
            action: "DeleteDBInstance",
            params: params![
                "DBInstanceIdentifier" => "emu-rcls-protected-inst",
                "SkipFinalSnapshot" => "true",
            ],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(PROTECTED_DB_INSTANCE.to_string()),
        },
        Row {
            case: "DeleteGlobalCluster on a deletion-protected global cluster",
            action: "DeleteGlobalCluster",
            params: params!["GlobalClusterIdentifier" => "emu-rcls-global"],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(PROTECTED_GLOBAL_CLUSTER.to_string()),
        },
        // The final-snapshot pair, in both directions, on the two deletes that carry it.
        Row {
            case: "DeleteDBInstance naming both final-snapshot members",
            action: "DeleteDBInstance",
            params: params![
                "DBInstanceIdentifier" => "emu-rcls-live-inst",
                "SkipFinalSnapshot" => "true",
                "FinalDBSnapshotIdentifier" => "emu-rcls-final",
            ],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(ONLY_ONE_SNAPSHOT_MEMBER.to_string()),
        },
        Row {
            case: "DeleteDBInstance naming neither final-snapshot member",
            action: "DeleteDBInstance",
            params: params!["DBInstanceIdentifier" => "emu-rcls-live-inst"],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(A_SNAPSHOT_MEMBER_IS_REQUIRED.to_string()),
        },
        Row {
            case: "DeleteDBCluster naming both final-snapshot members",
            action: "DeleteDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-live",
                "SkipFinalSnapshot" => "true",
                "FinalDBSnapshotIdentifier" => "emu-rcls-final",
            ],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(ONLY_ONE_SNAPSHOT_MEMBER.to_string()),
        },
        Row {
            case: "DeleteDBCluster naming neither final-snapshot member",
            action: "DeleteDBCluster",
            params: params!["DBClusterIdentifier" => "emu-rcls-live"],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Exact(A_SNAPSHOT_MEMBER_IS_REQUIRED.to_string()),
        },
        // A member that cannot hold together with another member of the same request.
        Row {
            case: "CreateDBInstance naming an AvailabilityZone with Multi-AZ enabled",
            action: "CreateDBInstance",
            params: params![
                "DBInstanceIdentifier" => "emu-rcls-both-az",
                "Engine" => "postgres",
                "DBInstanceClass" => "db.t3.micro",
                "MultiAZ" => "true",
                "AvailabilityZone" => "us-west-2a",
            ],
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
            wording: Wording::Says("AvailabilityZone"),
        },
        // A member whose value the addressed resource's own configuration rules out.
        Row {
            case: "RebootDBInstance forcing a failover on a single-AZ instance",
            action: "RebootDBInstance",
            params: params![
                "DBInstanceIdentifier" => "emu-rcls-single",
                "ForceFailover" => "true",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact(FORCE_FAILOVER_NEEDS_MULTI_AZ.to_string()),
        },
        // A value outside the domain the member accepts for the resolved engine version:
        // the same refusal on both Aurora engines, neither of which publishes the
        // withdrawn Aurora Serverless v1 mode.
        Row {
            case: "CreateDBCluster asking for the serverless engine mode on aurora-mysql",
            action: "CreateDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-mode-mysql",
                "Engine" => "aurora-mysql",
                "EngineMode" => "serverless",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact(UNAVAILABLE_SERVERLESS_MODE.to_string()),
        },
        Row {
            case: "CreateDBCluster asking for the serverless engine mode on aurora-postgresql",
            action: "CreateDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-mode-pg",
                "Engine" => "aurora-postgresql",
                "EngineMode" => "serverless",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact(UNAVAILABLE_SERVERLESS_MODE.to_string()),
        },
        // The Serverless v2 capacity a rescale asks for, over the axes the rule spans: past
        // the published ceiling, off the half-step grid, below the published auto-pause
        // bound, and a floor above the ceiling (each a VALUE the member does not accept).
        Row {
            case: "ModifyDBCluster past the engine version's capacity ceiling",
            action: "ModifyDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-live",
                "ServerlessV2ScalingConfiguration.MaxCapacity" => "256.5",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            // The service states the value it would not take and the range it will.
            wording: Wording::Says("Serverless v2 capacity value 256.5 is not valid."),
        },
        Row {
            case: "ModifyDBCluster with a capacity off the half-step grid",
            action: "ModifyDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-live",
                "ServerlessV2ScalingConfiguration.MaxCapacity" => "1.2",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Says("It must be a multiple of 0.5."),
        },
        Row {
            case: "ModifyDBCluster one second below the auto-pause bound",
            action: "ModifyDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-live",
                "ServerlessV2ScalingConfiguration.SecondsUntilAutoPause" => "299",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Says("SecondsUntilAutoPause"),
        },
        Row {
            case: "ModifyDBCluster with a capacity floor above its ceiling",
            action: "ModifyDBCluster",
            params: params![
                "DBClusterIdentifier" => "emu-rcls-live",
                "ServerlessV2ScalingConfiguration.MinCapacity" => "8",
                "ServerlessV2ScalingConfiguration.MaxCapacity" => "4",
            ],
            status: 400,
            // A floor above the ceiling is a capacity VALUE the member does not accept, and
            // Amazon RDS answers it with one fixed sentence that names neither value \u2014 the
            // recorded answer this front door is held to.
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact(
                service_com_amazonaws_rds_amazonrdsv19::state::SERVERLESS_V2_FLOOR_ABOVE_CEILING_MESSAGE
                    .to_string(),
            ),
        },
        // A list member that names nothing is refused as the list it is — the modelled
        // `InvalidSubnet` — on both operations that carry such a list, and before the
        // Availability Zone coverage its entries would resolve to.
        Row {
            case: "CreateDBSubnetGroup with an empty SubnetIds list",
            action: "CreateDBSubnetGroup",
            params: params![
                "DBSubnetGroupName" => "emu-rcls-no-subnets",
                "DBSubnetGroupDescription" => "names no subnet",
                "SubnetIds" => "",
            ],
            status: 400,
            code: "InvalidSubnet",
            wording: Wording::Exact(SUBNET_IDS_REQUIRED.to_string()),
        },
        Row {
            case: "ModifyDBSubnetGroup with an empty SubnetIds list",
            action: "ModifyDBSubnetGroup",
            params: params![
                "DBSubnetGroupName" => "emu-rcls-subnets",
                "SubnetIds" => "",
            ],
            status: 400,
            code: "InvalidSubnet",
            wording: Wording::Exact(SUBNET_IDS_REQUIRED.to_string()),
        },
        // A value outside the list a member publishes, on each of the two members that
        // say what to DO with a resolved resource.
        Row {
            case: "ApplyPendingMaintenanceAction with an action AWS does not publish",
            action: "ApplyPendingMaintenanceAction",
            params: params![
                "ResourceIdentifier" => instance_arn("emu-rcls-live-inst"),
                "ApplyAction" => "reboot-now",
                "OptInType" => "immediate",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Says("ApplyAction"),
        },
        Row {
            case: "ApplyPendingMaintenanceAction with an opt-in type AWS does not publish",
            action: "ApplyPendingMaintenanceAction",
            params: params![
                "ResourceIdentifier" => instance_arn("emu-rcls-live-inst"),
                "ApplyAction" => "system-update",
                "OptInType" => "later",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Says("OptInType"),
        },
        // Rule 2: the required identifier resolves to no resource, and every such value is
        // refused on the MEMBER that carried it, whichever resource kind it names.
        Row {
            case: "ApplyPendingMaintenanceAction over an ARN of a resource never created",
            action: "ApplyPendingMaintenanceAction",
            params: params![
                "ResourceIdentifier" => instance_arn("emu-rcls-absent"),
                "ApplyAction" => "system-update",
                "OptInType" => "immediate",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact(format!(
                "Unknown resource: {}",
                instance_arn("emu-rcls-absent")
            )),
        },
        Row {
            case: "ApplyPendingMaintenanceAction over a value that is no ARN at all",
            action: "ApplyPendingMaintenanceAction",
            params: params![
                "ResourceIdentifier" => "emu-rcls-live-inst",
                "ApplyAction" => "system-update",
                "OptInType" => "immediate",
            ],
            status: 400,
            code: INVALID_PARAMETER_VALUE,
            wording: Wording::Exact("Unknown resource: emu-rcls-live-inst".to_string()),
        },
    ];

    for row in &rows {
        expect_refused(&router, row).await;
    }

    // Rule 4, first half: every refusal above left the resources exactly as they were — the
    // protected ones still protected, the rescaled cluster still carrying no Serverless v2
    // capacity, and the group still holding the subnets the create stored.
    let protected = expect_ok(
        "the protected cluster after the refusals",
        call(
            &router,
            "DescribeDBClusters",
            &params!["DBClusterIdentifier" => "emu-rcls-protected"],
        )
        .await,
    );
    assert_eq!(
        text(&protected, "DeletionProtection"),
        "true",
        "a refused delete must leave the cluster exactly as it was, got {protected}"
    );
    let live = expect_ok(
        "the rescaled cluster after the refusals",
        call(
            &router,
            "DescribeDBClusters",
            &params!["DBClusterIdentifier" => "emu-rcls-live"],
        )
        .await,
    );
    assert!(
        texts(&live, "MaxCapacity").is_empty(),
        "a refused rescale must store no capacity, got {live}"
    );
    let group = expect_ok(
        "the subnet group after the refusals",
        call(
            &router,
            "DescribeDBSubnetGroups",
            &params!["DBSubnetGroupName" => "emu-rcls-subnets"],
        )
        .await,
    );
    let mut stored = texts(&group, "SubnetIdentifier");
    stored.sort();
    let mut expected: Vec<String> = TWO_SUBNETS.iter().map(|id| (*id).to_string()).collect();
    expected.sort();
    assert_eq!(
        stored, expected,
        "a refused modification must leave the group's subnets as they were, got {group}"
    );
}

// ---------------------------------------------------------------------------
// 3: the state refusal is worded from the record
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_state_refusal_says_whether_the_status_can_be_waited_out() {
    let router = emulator();

    // A status the instance will LEAVE: the one a create starts it in, and the one a stop
    // leaves it in. The refusal states the state a modification needs, which is what a
    // caller polls on.
    expect_ok(
        "an instance that is still coming up",
        create_instance(&router, "emu-rcls-creating", &[]).await,
    );
    let body = expect_error(
        "ModifyDBInstance while the create is still running",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-creating",
                "BackupRetentionPeriod" => "7",
            ],
        )
        .await,
        400,
        INVALID_DB_INSTANCE_STATE,
    );
    assert_eq!(
        text(&body, "Message"),
        MODIFICATION_NEEDS_AVAILABLE,
        "a status the instance will leave states the state a modification needs, got {body}"
    );

    create_available_instance(&router, "emu-rcls-stopped", &[]).await;
    expect_ok(
        "stop the instance",
        call(
            &router,
            "StopDBInstance",
            &params!["DBInstanceIdentifier" => "emu-rcls-stopped"],
        )
        .await,
    );
    wait_for_settle();
    let status = instance_status(&router, "read the stopped instance", "emu-rcls-stopped").await;
    assert_eq!(
        status, "stopped",
        "expected the stop to settle the instance, got {status:?}"
    );
    let body = expect_error(
        "ModifyDBInstance on a stopped instance",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-stopped",
                "BackupRetentionPeriod" => "7",
            ],
        )
        .await,
        400,
        INVALID_DB_INSTANCE_STATE,
    );
    assert_eq!(
        text(&body, "Message"),
        MODIFICATION_NEEDS_AVAILABLE,
        "a stopped instance is one a start brings back, so the sentence is the same, got \
         {body}"
    );

    // And the one status an instance never comes back from: the refusal names that status,
    // taken from the instance's own record, and says it is terminal — so a caller does not
    // poll for a change that will never come.
    create_available_instance(&router, "emu-rcls-deleting", &[]).await;
    expect_ok(
        "delete the instance",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-deleting",
                "SkipFinalSnapshot" => "true",
            ],
        )
        .await,
    );
    let body = expect_error(
        "ModifyDBInstance inside the deleting window",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-deleting",
                "BackupRetentionPeriod" => "7",
            ],
        )
        .await,
        400,
        INVALID_DB_INSTANCE_STATE,
    );
    let message = text(&body, "Message");
    assert_eq!(
        message, MODIFICATION_IN_A_TERMINAL_STATE,
        "a terminal status must be named in the refusal, got {message:?}"
    );
    assert!(
        message.contains("deleting") && message.contains("terminal"),
        "the refusal must name the status and that it is terminal, got {message:?}"
    );
}

// ---------------------------------------------------------------------------
// 4: the same calls are served once the conflict is gone
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_same_calls_are_served_once_the_conflict_is_gone() {
    let router = emulator();
    expect_ok(
        "a cluster to rescale",
        create_cluster(&router, "emu-rcls-ok-cluster", "aurora-mysql", &[]).await,
    );
    wait_for_settle();
    create_available_instance(&router, "emu-rcls-ok-multi", &[("MultiAZ", "true")]).await;
    create_available_instance(&router, "emu-rcls-ok-protected", &[("DeletionProtection", "true")])
        .await;

    // A capacity inside the published range, on the half-step grid: stored, and read back
    // through `DescribeDBClusters`.
    expect_ok(
        "rescale the cluster inside its range",
        call(
            &router,
            "ModifyDBCluster",
            &params![
                "DBClusterIdentifier" => "emu-rcls-ok-cluster",
                "ServerlessV2ScalingConfiguration.MinCapacity" => "0.5",
                "ServerlessV2ScalingConfiguration.MaxCapacity" => "8.5",
            ],
        )
        .await,
    );
    let read_back = expect_ok(
        "read the rescaled cluster back",
        call(
            &router,
            "DescribeDBClusters",
            &params!["DBClusterIdentifier" => "emu-rcls-ok-cluster"],
        )
        .await,
    );
    assert_eq!(
        text(&read_back, "MaxCapacity"),
        "8.5",
        "the accepted capacity must be the one reported back, got {read_back}"
    );

    // `ForceFailover` on an instance that IS configured for Multi-AZ.
    expect_ok(
        "force a failover on a Multi-AZ instance",
        call(
            &router,
            "RebootDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-ok-multi",
                "ForceFailover" => "true",
            ],
        )
        .await,
    );

    // A `SubnetIds` list that names subnets covering two Availability Zones.
    let created = expect_ok(
        "a group whose list names subnets",
        create_subnet_group(&router, "emu-rcls-ok-subnets", &TWO_SUBNETS).await,
    );
    assert_eq!(
        texts(&created, "SubnetIdentifier").len(),
        TWO_SUBNETS.len(),
        "the accepted list must be the one reported back, got {created}"
    );

    // A maintenance action over an identifier that DOES resolve: its own (empty) pending
    // action list, because nothing here schedules one.
    let applied = expect_ok(
        "a maintenance action over a stored instance",
        call(
            &router,
            "ApplyPendingMaintenanceAction",
            &params![
                "ResourceIdentifier" => instance_arn("emu-rcls-ok-multi"),
                "ApplyAction" => "system-update",
                "OptInType" => "immediate",
            ],
        )
        .await,
    );
    assert_eq!(
        text(&applied, "ResourceIdentifier"),
        instance_arn("emu-rcls-ok-multi"),
        "the resolved resource must be the one reported, got {applied}"
    );

    // And the delete deletion protection refused is served once the setting is cleared,
    // which is exactly what the refusal told the caller to do.
    expect_ok(
        "clear the instance's deletion protection",
        call(
            &router,
            "ModifyDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-ok-protected",
                "DeletionProtection" => "false",
            ],
        )
        .await,
    );
    wait_for_settle();
    let deleted = expect_ok(
        "delete the instance that is no longer protected",
        call(
            &router,
            "DeleteDBInstance",
            &params![
                "DBInstanceIdentifier" => "emu-rcls-ok-protected",
                "SkipFinalSnapshot" => "true",
            ],
        )
        .await,
    );
    assert_eq!(
        text(&deleted, "DBInstanceStatus"),
        "deleting",
        "the accepted delete must report the instance on its way out, got {deleted}"
    );
}
