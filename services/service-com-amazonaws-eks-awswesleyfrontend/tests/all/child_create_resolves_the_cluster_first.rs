// SPDX-License-Identifier: CC0-1.0

//! WHEN the cluster a create hangs off is resolved, relative to everything else the
//! request says.
//!
//! RULE: an operation that creates a resource INSIDE a cluster resolves the cluster its
//! `@httpLabel` names against this service's state — and answers one it cannot resolve
//! with the 404 `ResourceNotFoundException` that names only the cluster
//! ([`service_com_amazonaws_eks_awswesleyfrontend::state::cluster_not_found`]) — BEFORE
//! it judges any other member of the request. Nothing the body says can make a create
//! against a cluster that does not exist answerable, so the answer reports the missing
//! parent and points the caller at its only recovery (create the cluster, retry) instead
//! of complaining about a sibling member the caller did supply.
//!
//! Only a decision that names no resource of this service at all may precede the
//! resolution: the pass-role authorization a caller-supplied role earns (403, pinned by
//! `pass_role_authorization`), and a member outside its own documented syntactic form
//! (which names nothing in any cluster — pinned where each operation enforces it). Every
//! row below therefore carries a role of this account and well-formed identifiers, and is
//! invalid only against a rule that needs the cluster, the body's semantics or another
//! service to decide.
//!
//! The tables span what the rule generalises over:
//!
//! * the OPERATIONS that create inside a cluster and have a parameter rule of their own
//!   (`CreateNodegroup`, `CreateCapability`, `CreateAddon`, `CreateAccessEntry`);
//! * the MEMBER each row gets wrong — a list's length, a numeric bound, a mutually
//!   exclusive pair, a value that must match the cluster's, an enum-like string, a
//!   charset, a configuration that belongs to another type;
//! * the CLUSTER NAME form (both documented length bounds of `clusterName`, `length
//!   1..100`) and the region the request is signed for.
//!
//! The control half of each row is what shows the refusals were not simply deleted: the
//! SAME body against a real, `ACTIVE` cluster is still refused 400, and a valid body
//! against it is created and reads back.
//!
//! This suite is the ONE that spans the OPERATIONS the rule holds for; `error_class_selection`
//! pins the same rule's error CLASS in depth on `CreateNodegroup`'s body rules, and
//! `not_found_payload` pins the shape of the 404 body across every operation that can raise
//! one. The half that needs peers — the `roleArn` IAM resolution and the `subnets` EC2
//! resolution, which a single-service router cannot reach — is
//! `core/router/tests/all/eks_cluster_inputs_resolve.rs`.

use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, describe, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// The sub-resource identity members a not-found payload never carries: the request's own
/// child name is not echoed into it (`ResourceNotFoundException`'s optional members, from
/// `smithy_query query="shape" shape="ResourceNotFoundException"`).
const CHILD_IDENTITY_MEMBERS: &[&str] = &[
    "nodegroupName",
    "fargateProfileName",
    "addonName",
    "subscriptionId",
];

/// A role of the account this emulator serves, so the pass-role authorization (403) is
/// never what answers a row.
fn role_arn() -> String {
    format!(
        "arn:aws:iam::{}:role/eksOrderingProbeRole",
        engine::emulator_account_id()
    )
}

/// One row: a child-create request whose body breaks a rule of its own operation. The
/// case names the operation, so the collection segment and the body are all that is
/// needed to send it.
struct Row {
    case: &'static str,
    /// The collection segment under `/clusters/{clusterName}/`.
    collection: &'static str,
    body: serde_json::Value,
}

/// The rows: one per (operation, member) pair the rule generalises over. Every body is
/// well-formed and carries a passable role; each is invalid only against a rule its
/// operation decides AFTER the cluster is resolved, so a validation that ran first would
/// answer 400 here instead of the 404 the rule requires.
fn invalid_body_rows() -> Vec<Row> {
    let role = role_arn();
    let subnets = serde_json::json!(["subnet-aaaa1111", "subnet-bbbb2222"]);
    vec![
        Row {
            case: "CreateNodegroup: subnets names no subnet at all",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-empty-subnets",
                "nodeRole": role,
                "subnets": [],
            }),
        },
        Row {
            case: "CreateNodegroup: diskSize below the documented minimum",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-disk",
                "nodeRole": role,
                "subnets": subnets,
                "diskSize": 0,
            }),
        },
        Row {
            case: "CreateNodegroup: launchTemplate naming both id and name",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-template",
                "nodeRole": role,
                "subnets": subnets,
                "launchTemplate": { "id": "lt-00000000000000000", "name": "probe" },
            }),
        },
        Row {
            case: "CreateNodegroup: scalingConfig with only one of the three sizes",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-scaling",
                "nodeRole": role,
                "subnets": subnets,
                "scalingConfig": { "minSize": 1 },
            }),
        },
        Row {
            case: "CreateNodegroup: updateConfig naming both maxUnavailable forms",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-update",
                "nodeRole": role,
                "subnets": subnets,
                "updateConfig": { "maxUnavailable": 1, "maxUnavailablePercentage": 10 },
            }),
        },
        Row {
            case: "CreateNodegroup: a version that is not the cluster's",
            collection: "node-groups",
            body: serde_json::json!({
                "nodegroupName": "order-ng-version",
                "nodeRole": role,
                "subnets": subnets,
                "version": "1.1",
            }),
        },
        Row {
            case: "CreateCapability: capabilityName outside the documented charset",
            collection: "capabilities",
            body: serde_json::json!({
                "capabilityName": "order cap!",
                "type": "ACK",
                "roleArn": role,
                "deletePropagationPolicy": "RETAIN",
            }),
        },
        Row {
            case: "CreateCapability: a configuration belonging to another capability type",
            collection: "capabilities",
            body: serde_json::json!({
                "capabilityName": "order-cap-config",
                "type": "KRO",
                "roleArn": role,
                "deletePropagationPolicy": "RETAIN",
                "configuration": { "ack": { "enableCrossNamespace": true } },
            }),
        },
        Row {
            case: "CreateAddon: addonName outside the add-on name charset",
            collection: "addons",
            body: serde_json::json!({ "addonName": "vpc cni!" }),
        },
        Row {
            case: "CreateAccessEntry: a type outside the documented set",
            collection: "access-entries",
            body: serde_json::json!({
                "principalArn": role_arn(),
                "type": "NOT_AN_ACCESS_ENTRY_TYPE",
            }),
        },
    ]
}

async fn send(router: &MultiServiceRouter, region: &str, cluster: &str, row: &Row) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/{}",
            encode_label(cluster),
            row.collection
        ),
        &row.body.to_string(),
    )
    .await
}

/// Create `name` and move it past the span the clock owes it, so it is `ACTIVE`.
///
/// The body is the one whose `authenticationMode` serves the access-entry API, since
/// one of the rows below is a `CreateAccessEntry`: a cluster that serves no access
/// entries refuses that call for its mode instead of judging the body.
async fn active_cluster(router: &MultiServiceRouter, name: &str) {
    let created = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &access_entry_cluster_request(name).to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {name:?} must succeed, got {created:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
    let described = describe(router, name).await;
    assert_eq!(
        described.cluster()["status"],
        serde_json::json!("ACTIVE"),
        "setup: cluster {name:?} must be ACTIVE, got {described:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the cluster is resolved first, so a create against one that does not exist is
// the 404 that names the cluster — whatever else the body gets wrong.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_child_create_on_an_unresolvable_cluster_reports_the_cluster_not_the_body() {
    let router = emulator();
    // One real, ACTIVE cluster, so a row's 404 is about the name that row carries rather
    // than about an empty store — and so the control half below has a cluster to resolve.
    active_cluster(&router, "order-real").await;

    // (case, region, cluster) — the rows differ along WHY the cluster cannot be resolved
    // and along the form of the name (`clusterName` is `length 1..100`, so both bounds
    // are rows of their own).
    let longest = "o".repeat(100);
    let clusters: Vec<(&str, &str, &str)> = vec![
        ("a cluster that was never created", REGION, "order-ghost"),
        ("a cluster name differing only in case", REGION, "ORDER-REAL"),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            longest.as_str(),
        ),
        ("a one-character cluster name", REGION, "o"),
        (
            "the real cluster name, signed for another region",
            "eu-west-1",
            "order-real",
        ),
    ];

    for (why, region, cluster) in clusters {
        for row in invalid_body_rows() {
            let case = format!("{why} / {}", row.case);
            let answer = send(&router, region, cluster, &row).await;
            assert_eq!(
                answer.status, 404,
                "case {case:?}: the unresolvable cluster must be reported before the body, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {case:?}: expected the not-found error class, got {answer:?}"
            );
            assert_eq!(
                answer.body["clusterName"].as_str(),
                Some(cluster),
                "case {case:?}: the payload must name the cluster that was not found, got {:?}",
                answer.body["clusterName"]
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(cluster_not_found_message(cluster).as_str()),
                "case {case:?}: expected the message built from the request, got {:?}",
                answer.body["message"]
            );
            for member in CHILD_IDENTITY_MEMBERS {
                assert_eq!(
                    answer.body[member],
                    serde_json::Value::Null,
                    "case {case:?}: the payload must not echo the request's {member}, got {:?}",
                    answer.body[member]
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: the rule is about ORDER, not about dropping the rules — once the cluster
// resolves, every row's body is still refused, and with the parameter error class.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn once_the_cluster_resolves_the_same_body_is_still_refused_as_a_parameter() {
    let router = emulator();
    active_cluster(&router, "order-control").await;

    for row in invalid_body_rows() {
        let answer = send(&router, REGION, "order-control", &row).await;
        assert_eq!(
            answer.status, 400,
            "case {:?}: on a cluster that resolves, the body is still judged, got {answer:?}",
            row.case
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {:?}: expected the parameter error class, got {answer:?}",
            row.case
        );
        assert!(
            answer.body["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty()),
            "case {:?}: the refusal must say which member is wrong, got {answer:?}",
            row.case
        );
    }
}

// ---------------------------------------------------------------------------
// ROUND TRIP: a create whose cluster resolves and whose body is valid is written and
// reads back, so the resolution-first order did not cost the create itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_valid_child_create_on_a_resolvable_cluster_is_stored_and_read_back() {
    let router = emulator();
    let cluster = "order-roundtrip";
    active_cluster(&router, cluster).await;

    // The two operations the ordering rule was broken on, each created and then read
    // back through its own Describe.
    let subnets = serde_json::json!(["subnet-aaaa1111", "subnet-bbbb2222"]);
    let created = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/node-groups", encode_label(cluster)),
        &serde_json::json!({
            "nodegroupName": "order-nodes",
            "nodeRole": role_arn(),
            "subnets": subnets,
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "a node group on a resolvable cluster must be created, got {created:?}"
    );
    let read = call(
        &router,
        REGION,
        "GET",
        &format!("/clusters/{}/node-groups/order-nodes", encode_label(cluster)),
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "the stored node group must read back, got {read:?}"
    );
    assert_eq!(
        read.body["nodegroup"]["subnets"],
        subnets,
        "the node group read back must carry the subnets it was given, got {:?}",
        read.body["nodegroup"]
    );

    let created = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{}/capabilities", encode_label(cluster)),
        &serde_json::json!({
            "capabilityName": "order-cap",
            "type": "ACK",
            "roleArn": role_arn(),
            "deletePropagationPolicy": "RETAIN",
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "a capability on a resolvable ACTIVE cluster must be created, got {created:?}"
    );
    let read = call(
        &router,
        REGION,
        "GET",
        &format!("/clusters/{}/capabilities/order-cap", encode_label(cluster)),
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "the stored capability must read back, got {read:?}"
    );
    assert_eq!(
        read.body["capability"]["roleArn"],
        serde_json::json!(role_arn()),
        "the capability read back must carry the role it was given, got {:?}",
        read.body["capability"]
    );
}
