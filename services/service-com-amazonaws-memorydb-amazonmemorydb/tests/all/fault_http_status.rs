// SPDX-License-Identifier: CC0-1.0

//! What HTTP STATUS a modelled MemoryDB fault answers with, end to end over the wire.
//!
//! Every case drives the SAME path a client does: an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AmazonMemoryDB.<Operation>` through the multi-service router (the
//! shared harness in `cluster_lifecycle`), so what is asserted is the emulator's HTTP
//! behaviour and not a handler called directly.
//!
//! The rule under test, over a SET of inputs rather than one recorded request: both wire
//! protocols this service's model declares are RPC protocols whose request URI names the
//! ENDPOINT and the OPERATION and never a resource, so a fault about the request's
//! CONTENT is a client fault and answers **HTTP 400** with the fault's own `__type` in
//! the body — NO modelled fault answers 404, whatever `@httpError` its shape declares.
//! The table spans that class along what it generalises over:
//!
//! * the kind of resource the reference misses (cluster, ACL, user, parameter group,
//!   tag key) and so the `__type` the fault carries;
//! * the operation kind that resolves it (create, read, update, delete, tag);
//! * the form the reference takes (a name member, a name inside a LIST member, a name
//!   inside an ARN, a tag key on a resource that does resolve);
//! * and the control: a fault whose shape declares a status that is NOT 404 keeps that
//!   status, so the mapping moves one status and flattens nothing else.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{call, error_code, ok, router};

/// The region the suite-wide harness signs its requests for
/// (`crate::cluster_lifecycle::REGION`), which every ARN the emulator mints carries.
const REGION: &str = "us-west-2";

/// The account id every generated emulator presents (`engine::EMULATOR_ACCOUNT_ID`).
const ACCOUNT: &str = engine::EMULATOR_ACCOUNT_ID;

/// A node type MemoryDB publishes, for the clusters this suite seeds.
const NODE_TYPE: &str = "db.t4g.small";

/// An access string that is valid for every user this suite seeds (MemoryDB's "full
/// access" string: enabled, all keys, all channels, all commands).
const FULL_ACCESS: &str = "on ~* &* +@all";

/// A password that breaks none of the documented user password rules (16 characters,
/// none of `,` `"` `/` `@`).
const PASSWORD: &str = "Aa1!Aa1!Aa1!Aa1!";

/// A parameter group family MemoryDB publishes for its Redis OSS engine.
const FAMILY: &str = "memorydb_redis7";

/// A name NO seeded resource of any kind carries, in every member and ARN below.
const ABSENT: &str = "nothing-here";

/// The names the suite SEEDS, one resource of each kind this emulator can create, so
/// every row's fault is about the name the request carries rather than about an empty
/// store.
const PRESENT_CLUSTER: &str = "present-cluster";
const PRESENT_ACL: &str = "present-acl";
const PRESENT_USER: &str = "present-user";
const PRESENT_PARAMETER_GROUP: &str = "present-pg";

/// One `arn:aws:memorydb:<region>:<account>:<type>/<name>` in this suite's region — the
/// form the tagging operations resolve a resource through.
fn arn(resource_type: &str, name: &str) -> String {
    format!("arn:aws:memorydb:{REGION}:{ACCOUNT}:{resource_type}/{name}")
}

/// A router with one resource of each creatable kind in it.
async fn seeded() -> MultiServiceRouter {
    let router = router();
    ok(
        &router,
        "CreateACL",
        &json!({ "ACLName": PRESENT_ACL }),
        "seed acl",
    )
    .await;
    ok(
        &router,
        "CreateUser",
        &json!({
            "UserName": PRESENT_USER,
            "AccessString": FULL_ACCESS,
            "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
        }),
        "seed user",
    )
    .await;
    ok(
        &router,
        "CreateParameterGroup",
        &json!({ "ParameterGroupName": PRESENT_PARAMETER_GROUP, "Family": FAMILY }),
        "seed parameter group",
    )
    .await;
    ok(
        &router,
        "CreateCluster",
        &json!({
            "ClusterName": PRESENT_CLUSTER,
            "NodeType": NODE_TYPE,
            "ACLName": PRESENT_ACL,
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
        "seed cluster",
    )
    .await;
    router
}

/// One row: the operation, the request, and the `__type` its fault must carry.
struct Row {
    case: &'static str,
    operation: &'static str,
    body: fn() -> Value,
    code: &'static str,
}

/// Every reference a request can make to a MemoryDB resource that is not in state, over
/// the operations this emulator implements: by name member, by a name inside a list
/// member, and by a name inside an ARN.
const MISSING: &[Row] = &[
    // A cluster named in `ClusterName`, on each of the operations that resolve one.
    Row {
        case: "DescribeClusters names a cluster that is not in state",
        operation: "DescribeClusters",
        body: || json!({ "ClusterName": ABSENT }),
        code: "ClusterNotFoundFault",
    },
    Row {
        case: "UpdateCluster names a cluster that is not in state",
        operation: "UpdateCluster",
        body: || json!({ "ClusterName": ABSENT, "Description": "moved" }),
        code: "ClusterNotFoundFault",
    },
    Row {
        case: "DeleteCluster names a cluster that is not in state",
        operation: "DeleteCluster",
        body: || json!({ "ClusterName": ABSENT }),
        code: "ClusterNotFoundFault",
    },
    // An ACL named by a cluster write, and by the ACL operations themselves.
    Row {
        case: "CreateCluster names an ACL that is not in state",
        operation: "CreateCluster",
        body: || json!({ "ClusterName": "fresh-cluster", "NodeType": NODE_TYPE, "ACLName": ABSENT }),
        code: "ACLNotFoundFault",
    },
    Row {
        case: "UpdateCluster repoints a stored cluster at an ACL that is not in state",
        operation: "UpdateCluster",
        body: || json!({ "ClusterName": PRESENT_CLUSTER, "ACLName": ABSENT }),
        code: "ACLNotFoundFault",
    },
    Row {
        case: "DescribeACLs names an ACL that is not in state",
        operation: "DescribeACLs",
        body: || json!({ "ACLName": ABSENT }),
        code: "ACLNotFoundFault",
    },
    Row {
        case: "DeleteACL names an ACL that is not in state",
        operation: "DeleteACL",
        body: || json!({ "ACLName": ABSENT }),
        code: "ACLNotFoundFault",
    },
    Row {
        case: "UpdateACL names an ACL that is not in state",
        operation: "UpdateACL",
        body: || json!({ "ACLName": ABSENT, "UserNamesToAdd": [PRESENT_USER] }),
        code: "ACLNotFoundFault",
    },
    // A user, named directly and inside a LIST member of a stored ACL's update.
    Row {
        case: "DescribeUsers names a user that is not in state",
        operation: "DescribeUsers",
        body: || json!({ "UserName": ABSENT }),
        code: "UserNotFoundFault",
    },
    Row {
        case: "DeleteUser names a user that is not in state",
        operation: "DeleteUser",
        body: || json!({ "UserName": ABSENT }),
        code: "UserNotFoundFault",
    },
    Row {
        case: "UpdateUser names a user that is not in state",
        operation: "UpdateUser",
        body: || json!({ "UserName": ABSENT, "AccessString": FULL_ACCESS }),
        code: "UserNotFoundFault",
    },
    Row {
        case: "UpdateACL removes a user the stored ACL does not list",
        operation: "UpdateACL",
        body: || json!({ "ACLName": PRESENT_ACL, "UserNamesToRemove": [ABSENT] }),
        code: "UserNotFoundFault",
    },
    // A parameter group, on each operation that resolves one.
    Row {
        case: "DescribeParameterGroups names a group that is not in state",
        operation: "DescribeParameterGroups",
        body: || json!({ "ParameterGroupName": ABSENT }),
        code: "ParameterGroupNotFoundFault",
    },
    Row {
        case: "DescribeParameters names a group that is not in state",
        operation: "DescribeParameters",
        body: || json!({ "ParameterGroupName": ABSENT }),
        code: "ParameterGroupNotFoundFault",
    },
    Row {
        case: "UpdateParameterGroup names a group that is not in state",
        operation: "UpdateParameterGroup",
        body: || json!({
            "ParameterGroupName": ABSENT,
            "ParameterNameValues": [{ "ParameterName": "maxmemory-policy", "ParameterValue": "allkeys-lru" }],
        }),
        code: "ParameterGroupNotFoundFault",
    },
    // A name carried inside an ARN, one tagging operation per resource kind.
    Row {
        case: "ListTags resolves an ARN naming no cluster",
        operation: "ListTags",
        body: || json!({ "ResourceArn": arn("cluster", ABSENT) }),
        code: "ClusterNotFoundFault",
    },
    Row {
        case: "TagResource resolves an ARN naming no user",
        operation: "TagResource",
        body: || json!({
            "ResourceArn": arn("user", ABSENT),
            "Tags": [{ "Key": "env", "Value": "test" }],
        }),
        code: "UserNotFoundFault",
    },
    Row {
        case: "UntagResource resolves an ARN naming no ACL",
        operation: "UntagResource",
        body: || json!({ "ResourceArn": arn("acl", ABSENT), "TagKeys": ["env"] }),
        code: "ACLNotFoundFault",
    },
    Row {
        case: "ListTags resolves an ARN naming no parameter group",
        operation: "ListTags",
        body: || json!({ "ResourceArn": arn("parametergroup", ABSENT) }),
        code: "ParameterGroupNotFoundFault",
    },
    Row {
        case: "ListTags resolves an ARN naming no snapshot",
        operation: "ListTags",
        body: || json!({ "ResourceArn": arn("snapshot", ABSENT) }),
        code: "SnapshotNotFoundFault",
    },
    Row {
        case: "ListTags resolves an ARN naming no subnet group",
        operation: "ListTags",
        body: || json!({ "ResourceArn": arn("subnetgroup", ABSENT) }),
        code: "SubnetGroupNotFoundFault",
    },
    Row {
        case: "ListTags resolves an ARN naming no multi-Region cluster",
        operation: "ListTags",
        body: || json!({ "ResourceArn": arn("multiregioncluster", ABSENT) }),
        code: "MultiRegionClusterNotFoundFault",
    },
    // A tag KEY the resource does not carry: the same family, on a resource that DOES
    // resolve, so the row is about the missing key and not about the ARN.
    Row {
        case: "UntagResource removes a key the stored cluster does not carry",
        operation: "UntagResource",
        body: || json!({ "ResourceArn": arn("cluster", PRESENT_CLUSTER), "TagKeys": ["absent"] }),
        code: "TagNotFoundFault",
    },
];

/// A fault whose shape declares a status OTHER than 404 keeps the status the model
/// gives it — the mapping moves one status and leaves every other alone.
const DECLARED_400: &[Row] = &[
    Row {
        case: "an ACL that is already in state",
        operation: "CreateACL",
        body: || json!({ "ACLName": PRESENT_ACL }),
        code: "ACLAlreadyExistsFault",
    },
    Row {
        case: "a user that is already in state",
        operation: "CreateUser",
        body: || json!({
            "UserName": PRESENT_USER,
            "AccessString": FULL_ACCESS,
            "AuthenticationMode": { "Type": "password", "Passwords": [PASSWORD] },
        }),
        code: "UserAlreadyExistsFault",
    },
    Row {
        case: "an update that asks for no change",
        operation: "UpdateCluster",
        body: || json!({ "ClusterName": PRESENT_CLUSTER }),
        code: "NoOperationFault",
    },
    Row {
        case: "an ARN that is not a MemoryDB resource ARN",
        operation: "ListTags",
        body: || json!({ "ResourceArn": "not-an-arn" }),
        code: "InvalidARNFault",
    },
];

/// Call one row and assert it answers `expected_status` with the row's `__type` and a
/// message to read it by.
async fn assert_row(router: &MultiServiceRouter, row: &Row, expected_status: u16) {
    let body = (row.body)();
    let case = row.case;
    let (status, answered) = call(router, row.operation, &body).await;
    assert_eq!(
        (status, error_code(&answered).as_str()),
        (expected_status, row.code),
        "case {case:?}: expected {} {expected_status} {}, got {status} with {answered}",
        row.operation,
        row.code
    );
    let message = answered
        .get("message")
        .or_else(|| answered.get("Message"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    assert!(
        !message.is_empty(),
        "case {case:?}: expected the fault to carry a message, got {answered}"
    );
}

/// Every modelled fault about a resource that is not in state answers HTTP 400 with its
/// own `__type` — never the 404 its shape declares.
#[tokio::test]
async fn a_missing_resource_is_a_four_hundred_whatever_fault_it_is() {
    let router = seeded().await;
    for row in MISSING {
        assert_row(&router, row, 400).await;
    }
}

/// The same rows, read through the status alone: no modelled fault of this service
/// answers 404, which is the status a client reads as "this endpoint did not serve the
/// request" rather than as a fault about the request's content.
#[tokio::test]
async fn no_modelled_fault_answers_four_oh_four() {
    let router = seeded().await;
    for row in MISSING.iter().chain(DECLARED_400) {
        let case = row.case;
        let (status, answered) = call(&router, row.operation, &(row.body)()).await;
        assert_ne!(
            status, 404,
            "case {case:?}: {} must not answer 404, got {status} with {answered}",
            row.operation
        );
        assert!(
            (400..500).contains(&status),
            "case {case:?}: expected a client fault from {}, got {status} with {answered}",
            row.operation
        );
    }
}

/// A fault whose shape declares a status that is not 404 keeps it, so the mapping
/// cannot be read as "every fault is a 400 because every fault is rewritten".
#[tokio::test]
async fn a_fault_declaring_another_status_keeps_it() {
    let router = seeded().await;
    for row in DECLARED_400 {
        assert_row(&router, row, 400).await;
    }
}

/// A refused read changes nothing a later caller sees: the seeded resources are still
/// exactly where they were after the whole table has run.
#[tokio::test]
async fn the_refusals_leave_every_seeded_resource_in_place() {
    let router = seeded().await;
    for row in MISSING.iter().chain(DECLARED_400) {
        let (_, _) = call(&router, row.operation, &(row.body)()).await;
    }

    let case = "after the whole table";
    let described = ok(
        &router,
        "DescribeClusters",
        &json!({ "ClusterName": PRESENT_CLUSTER }),
        case,
    )
    .await;
    assert_eq!(
        described["Clusters"][0]["Name"],
        json!(PRESENT_CLUSTER),
        "case {case:?}: expected the seeded cluster, got {described}"
    );
    assert_eq!(
        described["Clusters"][0]["ACLName"],
        json!(PRESENT_ACL),
        "case {case:?}: a refused repoint must leave the ACL where it was, got {described}"
    );
    let tags = ok(
        &router,
        "ListTags",
        &json!({ "ResourceArn": arn("cluster", PRESENT_CLUSTER) }),
        case,
    )
    .await;
    assert_eq!(
        tags["TagList"],
        json!([{ "Key": "env", "Value": "test" }]),
        "case {case:?}: a refused tag mutation must leave the tag set alone, got {tags}"
    );
    for (operation, body) in [
        ("DescribeACLs", json!({ "ACLName": PRESENT_ACL })),
        ("DescribeUsers", json!({ "UserName": PRESENT_USER })),
        (
            "DescribeParameterGroups",
            json!({ "ParameterGroupName": PRESENT_PARAMETER_GROUP }),
        ),
    ] {
        ok(&router, operation, &body, case).await;
    }
}
