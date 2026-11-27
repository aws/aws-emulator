// SPDX-License-Identifier: CC0-1.0

//! Which Region's task definitions a `taskDefinition` reference may name.
//!
//! The rule under test, over a TABLE of inputs rather than one request:
//!
//! > Amazon ECS is a REGIONAL service, so a task definition belongs to the one Region's
//! > endpoint it was registered against, and a member documented as taking "the full Amazon
//! > Resource Name (ARN) of the task definition" names a revision of the Region the request
//! > was addressed to: an ARN whose Region segment is some other Region names nothing this
//! > endpoint can resolve and is refused HTTP 400 `InvalidParameterException` BEFORE the
//! > reference is resolved, in a sentence naming the MEMBER the foreign identifier was
//! > supplied in and then the segment of the ARN that is not this endpoint's — "Invalid task
//! > definition: Invalid Region in ARN", beside the bare segment sentence the cluster, task,
//! > service and container-instance reference members earn (`crate::arn_region_scope`).
//!
//! What the rows span: every operation on this surface that resolves a task-definition
//! reference (a read, a deregister, a batch delete, the two task launches, the two service
//! calls and the task-set create), the member kind the reference reaches it through (a
//! required member, an optional one and an entry of a list), the Region the ARN carries (five
//! Region codes across three partitions, plus two values that are no Region code at all), and
//! the ACCOUNT segment beside the Region — since an ARN is one whole identifier.
//!
//! Every case goes through the shared wire harness in [`crate::ecs_wire`], the same front door
//! a client uses, so nothing reaches into the state struct.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::ecs_wire::{
    answered, cluster_with_capacity, ok, refusal, register_definition, service, ACCOUNT,
    SIGNED_REGION,
};

/// The sentence a `taskDefinition` reference that is an ARN of another Region earns: the
/// member the foreign identifier was supplied in, then the segment that is foreign.
const INVALID_REGION_IN_ARN: &str = "Invalid task definition: Invalid Region in ARN";

/// The same sentence for an ARN of another ACCOUNT.
const ACCOUNT_IDS_MISMATCH: &str = "Invalid task definition: AccountIDs mismatch";

/// The error code a reference naming another Region's or another account's resource earns.
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The cluster the launch and service rows act in.
const CLUSTER: &str = "tdregionscope";

/// The task-definition family every row references, registered in the fixture so a row
/// carrying THIS Region's spelling names a revision that exists.
const FAMILY: &str = "tdregionscopefamily";

/// The revision of [`FAMILY`] the fixture registers.
const REVISION: i64 = 1;

/// The service the service-shaped rows name.
const SERVICE: &str = "tdregionscopeservice";

/// A container-instance id of the form a registration mints, for the `StartTask` row's
/// required `containerInstances` member.
const INSTANCE_ID: &str = "fedcba9876543210fedcba9876543210";

/// Region segments that are NOT the Region these requests are addressed to
/// ([`SIGNED_REGION`]), each with the partition it belongs to. The first five are Region codes
/// of three partitions, from the Regions table of the AWS General Reference (`us-east-2`,
/// `eu-west-1`, `ap-southeast-4`, `us-gov-west-1`, `cn-north-1`); the last two are values that
/// are no Region code at all. What makes an ARN another Region's is only that its Region
/// segment is not this endpoint's.
const OTHER_REGIONS: [(&str, &str); 7] = [
    ("aws", "us-east-2"),
    ("aws", "eu-west-1"),
    ("aws", "ap-southeast-4"),
    ("aws-us-gov", "us-gov-west-1"),
    ("aws-cn", "cn-north-1"),
    ("aws", "not-a-region"),
    ("aws", "US-WEST-2"),
];

/// Account ids that are not the one this emulator serves ([`ACCOUNT`]).
const OTHER_ACCOUNTS: [&str; 2] = ["111122223333", "999999999999"];

/// One row of the table: the operation, and the request built around the task-definition
/// reference the row is run with.
struct Row {
    case: &'static str,
    operation: &'static str,
    request: fn(&str) -> Value,
}

/// A task-definition ARN in `partition`, `region` and `account` for [`FAMILY`]'s revision.
fn task_definition_arn(partition: &str, region: &str, account: &str) -> String {
    format!("arn:{partition}:ecs:{region}:{account}:task-definition/{FAMILY}:{REVISION}")
}

/// Every operation that resolves a caller-supplied `taskDefinition` reference, one row per
/// member kind the reference reaches it through.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "DescribeTaskDefinition, the required reference",
            operation: "DescribeTaskDefinition",
            request: |reference| json!({ "taskDefinition": reference }),
        },
        Row {
            case: "DeregisterTaskDefinition, the required reference",
            operation: "DeregisterTaskDefinition",
            request: |reference| json!({ "taskDefinition": reference }),
        },
        Row {
            case: "DeleteTaskDefinitions, an entry of the list",
            operation: "DeleteTaskDefinitions",
            request: |reference| json!({ "taskDefinitions": [reference] }),
        },
        Row {
            case: "RunTask, the definition the task runs",
            operation: "RunTask",
            request: |reference| {
                json!({ "cluster": CLUSTER, "taskDefinition": reference, "count": 1 })
            },
        },
        Row {
            case: "StartTask, the definition the task runs",
            operation: "StartTask",
            request: |reference| {
                json!({
                    "cluster": CLUSTER,
                    "taskDefinition": reference,
                    "containerInstances": [INSTANCE_ID],
                })
            },
        },
        Row {
            case: "CreateService, the optional definition the service runs",
            operation: "CreateService",
            request: |reference| {
                json!({
                    "cluster": CLUSTER,
                    "serviceName": "tdregionscopecreated",
                    "taskDefinition": reference,
                    "desiredCount": 0,
                    "launchType": "EC2",
                })
            },
        },
        Row {
            case: "UpdateService, the optional definition the service is moved to",
            operation: "UpdateService",
            request: |reference| {
                json!({ "cluster": CLUSTER, "service": SERVICE, "taskDefinition": reference })
            },
        },
        Row {
            case: "CreateTaskSet, the definition the task set runs",
            operation: "CreateTaskSet",
            request: |reference| {
                json!({
                    "cluster": CLUSTER,
                    "service": SERVICE,
                    "taskDefinition": reference,
                })
            },
        },
    ]
}

/// A router holding the cluster the rows act in, with capacity, one revision of [`FAMILY`] and
/// one service of [`SERVICE`] — so a row carrying THIS Region's spelling names resources that
/// exist and the refusal under test is the only thing a row can be refused for.
async fn fixture() -> MultiServiceRouter {
    let router = service();
    cluster_with_capacity(&router, CLUSTER, "fixture").await;
    register_definition(&router, FAMILY, "fixture").await;
    ok(
        &router,
        "CreateService",
        json!({
            "cluster": CLUSTER,
            "serviceName": SERVICE,
            "taskDefinition": FAMILY,
            "desiredCount": 0,
            "launchType": "EC2",
        }),
        "fixture",
    )
    .await;
    router
}

#[tokio::test]
async fn a_task_definition_arn_of_another_region_is_refused_before_it_is_resolved() {
    let router = fixture().await;

    for row in rows() {
        for (partition, region) in OTHER_REGIONS {
            let reference = task_definition_arn(partition, region, ACCOUNT);
            let case = format!("{}: {reference}", row.case);
            let (code, message) = refusal(&router, row.operation, (row.request)(&reference), &case)
                .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_PARAMETER, INVALID_REGION_IN_ARN),
                "case {case:?}: expected the ARN-Region refusal, got {code} with {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_task_definition_arn_of_another_account_is_refused_for_its_account() {
    let router = fixture().await;

    // The Region segment is this endpoint's, so what is left foreign about the identifier is
    // the account that owns the revision it names.
    for row in rows() {
        for account in OTHER_ACCOUNTS {
            let reference = task_definition_arn("aws", SIGNED_REGION, account);
            let case = format!("{}: {reference}", row.case);
            let (code, message) = refusal(&router, row.operation, (row.request)(&reference), &case)
                .await;
            assert_eq!(
                (code.as_str(), message.as_str()),
                (INVALID_PARAMETER, ACCOUNT_IDS_MISMATCH),
                "case {case:?}: expected the ARN-account refusal, got {code} with {message:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_same_reference_in_this_region_is_resolved_as_the_revision_it_names() {
    let router = fixture().await;

    // The refusal is about the ARN's REGION and not about ARNs: the same request carrying the
    // same revision in THIS Region's spelling — and the family name the fixture registered —
    // is answered as the reference it is rather than refused for its Region.
    let references = [
        task_definition_arn("aws", SIGNED_REGION, ACCOUNT),
        format!("{FAMILY}:{REVISION}"),
        FAMILY.to_string(),
    ];
    for row in rows() {
        for reference in &references {
            let case = format!("{}: {reference}", row.case);
            let (status, body) = answered(&router, row.operation, (row.request)(reference)).await;
            let message = body["message"].as_str().or(body["Message"].as_str());
            assert!(
                message != Some(INVALID_REGION_IN_ARN)
                    && message != Some(ACCOUNT_IDS_MISMATCH),
                "case {case:?}: this Region's own reference must not be refused for its ARN \
                 segments, got {status} with {body}"
            );
        }
    }

    // And the read of that revision is SERVED: the ARN this endpoint reports for it carries
    // the Region the request was addressed to.
    let case = "DescribeTaskDefinition of this Region's own ARN";
    let reference = task_definition_arn("aws", SIGNED_REGION, ACCOUNT);
    let body = ok(
        &router,
        "DescribeTaskDefinition",
        json!({ "taskDefinition": reference.clone() }),
        case,
    )
    .await;
    assert_eq!(
        body["taskDefinition"]["taskDefinitionArn"].as_str(),
        Some(reference.as_str()),
        "case {case:?}: expected the revision this Region holds, got {body}"
    );
}
