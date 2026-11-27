// SPDX-License-Identifier: CC0-1.0

//! The Kubernetes Gateway API configuration of a cluster's load balancing
//! capability, end to end over the wire.
//!
//! RULE under test: every member of a capability's configuration other than its
//! `enabled` toggle is reported as it was CONFIGURED — the value the request
//! names, else the value the cluster already runs with — and a cluster nothing
//! configured it on reports none of them, while `enabled` is always reported
//! because a capability is always either on or off.
//!
//! The member names and the shape of the objects come from the model:
//! `smithy_query(query="shape", shape="ElasticLoadBalancing")` gives `enabled`
//! and `gatewayApi` -> `GatewayApiConfiguration`, whose `crds` is a
//! `CrdManagementConfiguration` carrying `managed`.
//!
//! Both suites run over a TABLE whose rows differ along what the rule
//! generalises over: whether the request names the capability at all, how deep
//! into it the request reaches (the capability, the Gateway API object, its CRD
//! management, the flag inside it) and which value the flag carries.

use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{create, describe, describe_when_active, minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, read_until, Answer};

/// Where a cluster reports the load balancing capability of its Kubernetes
/// network configuration.
const LOAD_BALANCING: &str = "/kubernetesNetworkConfig/elasticLoadBalancing";

/// A `CreateCluster` body for `name` carrying `network` as its
/// `kubernetesNetworkConfig`, plus whatever else a row needs.
fn create_body(name: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut body = minimal_request(name);
    for (key, value) in extra.as_object().cloned().unwrap_or_default() {
        body[key] = value;
    }
    body
}

async fn update_config(
    router: &MultiServiceRouter,
    name: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/update-config", encode_label(name)),
        &body.to_string(),
    )
    .await
}

/// The load balancing capability an answer reports, or `Null` when it reports
/// none.
fn reported(answer: &Answer) -> serde_json::Value {
    answer
        .cluster()
        .pointer(LOAD_BALANCING)
        .cloned()
        .unwrap_or(serde_json::Value::Null)
}

/// The three EKS Auto Mode capabilities a request has to carry TOGETHER to turn
/// the load balancing one on, with `load_balancing` as the load balancing member
/// of them (the all-or-nothing rule of that mode, which the suites below are not
/// about).
fn auto_mode_on(load_balancing: serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "computeConfig": {
            "enabled": true,
            "nodePools": ["general-purpose"],
            "nodeRoleArn": "arn:aws:iam::000000000000:role/eksNodeRole",
        },
        "storageConfig": { "blockStorage": { "enabled": true } },
        "kubernetesNetworkConfig": { "elasticLoadBalancing": load_balancing },
    })
}

/// One row of the create table: what the request carries, and what the cluster
/// reports for its load balancing capability.
struct Row {
    case: &'static str,
    cluster: &'static str,
    /// The members merged into the smallest accepted `CreateCluster` body.
    request: serde_json::Value,
    expected: serde_json::Value,
}

fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a request that names no network configuration at all",
            cluster: "gw-none",
            request: serde_json::json!({}),
            expected: serde_json::json!({ "enabled": false }),
        },
        Row {
            case: "the capability alone, left off",
            cluster: "gw-capability-only",
            request: serde_json::json!({
                "kubernetesNetworkConfig": { "elasticLoadBalancing": { "enabled": false } },
            }),
            expected: serde_json::json!({ "enabled": false }),
        },
        Row {
            case: "a Gateway API object that names no CRD management",
            cluster: "gw-empty",
            request: serde_json::json!({
                "kubernetesNetworkConfig": {
                    "elasticLoadBalancing": { "gatewayApi": {} },
                },
            }),
            expected: serde_json::json!({ "enabled": false, "gatewayApi": {} }),
        },
        Row {
            case: "CRD management named but left empty",
            cluster: "gw-crds-empty",
            request: serde_json::json!({
                "kubernetesNetworkConfig": {
                    "elasticLoadBalancing": { "gatewayApi": { "crds": {} } },
                },
            }),
            expected: serde_json::json!({
                "enabled": false,
                "gatewayApi": { "crds": {} },
            }),
        },
        Row {
            case: "CRD management asked for on a capability left off",
            cluster: "gw-crds-managed",
            request: serde_json::json!({
                "kubernetesNetworkConfig": {
                    "elasticLoadBalancing": { "gatewayApi": { "crds": { "managed": true } } },
                },
            }),
            expected: serde_json::json!({
                "enabled": false,
                "gatewayApi": { "crds": { "managed": true } },
            }),
        },
        Row {
            case: "CRD management declined",
            cluster: "gw-crds-unmanaged",
            request: serde_json::json!({
                "kubernetesNetworkConfig": {
                    "elasticLoadBalancing": { "gatewayApi": { "crds": { "managed": false } } },
                },
            }),
            expected: serde_json::json!({
                "enabled": false,
                "gatewayApi": { "crds": { "managed": false } },
            }),
        },
        Row {
            case: "the whole capability on, with CRD management",
            cluster: "gw-auto-mode",
            request: auto_mode_on(
                serde_json::json!({ "enabled": true, "gatewayApi": { "crds": { "managed": true } } }),
            ),
            expected: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": true } },
            }),
        },
    ]
}

#[tokio::test]
async fn a_created_cluster_reports_the_gateway_api_configuration_it_was_given() {
    let router = emulator();
    for Row {
        case,
        cluster,
        request,
        expected,
    } in rows()
    {
        let created = create(&router, &create_body(cluster, request)).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {created:?}"
        );
        assert_eq!(
            reported(&created),
            expected,
            "case {case:?}: expected the create to report {expected}, got {:?}",
            reported(&created)
        );

        // The same configuration reads back through `DescribeCluster`: it is the
        // stored record that is reported, not the request that was echoed.
        let described = describe(&router, cluster).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: expected HTTP 200 from the read, got {described:?}"
        );
        assert_eq!(
            reported(&described),
            expected,
            "case {case:?}: expected the read to report {expected}, got {:?}",
            reported(&described)
        );
    }
}

/// One row of the update table: what the cluster was created with, what the
/// update asks for, and what it must report afterwards.
struct UpdateRow {
    case: &'static str,
    cluster: &'static str,
    created_with: serde_json::Value,
    update: serde_json::Value,
    expected: serde_json::Value,
}

fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "an update that says nothing about the Gateway API keeps it",
            cluster: "gwup-kept",
            created_with: serde_json::json!({ "gatewayApi": { "crds": { "managed": true } } }),
            update: serde_json::json!({ "enabled": true }),
            expected: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": true } },
            }),
        },
        UpdateRow {
            case: "an update that names the Gateway API replaces it",
            cluster: "gwup-replaced",
            created_with: serde_json::json!({ "gatewayApi": { "crds": { "managed": true } } }),
            update: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": false } },
            }),
            expected: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": false } },
            }),
        },
        UpdateRow {
            case: "an update that configures the Gateway API for the first time",
            cluster: "gwup-added",
            created_with: serde_json::json!({ "enabled": false }),
            update: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": true } },
            }),
            expected: serde_json::json!({
                "enabled": true,
                "gatewayApi": { "crds": { "managed": true } },
            }),
        },
    ]
}

#[tokio::test]
async fn an_update_of_the_capability_keeps_the_gateway_api_it_does_not_name() {
    let router = emulator();
    for UpdateRow {
        case,
        cluster,
        created_with,
        update,
        expected,
    } in update_rows()
    {
        let created = create(
            &router,
            &create_body(
                cluster,
                serde_json::json!({
                    "kubernetesNetworkConfig": { "elasticLoadBalancing": created_with },
                }),
            ),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {created:?}"
        );
        // A configuration change is served on a cluster the clock has finished
        // creating, so the suite waits for that state by looking again.
        describe_when_active(&router, cluster).await;

        let requested = update_config(&router, cluster, &auto_mode_on(update)).await;
        assert_eq!(
            requested.status, 200,
            "case {case:?}: expected HTTP 200 from the update, got {requested:?}"
        );

        // The update is completed by the clock: the suite waits for the state the
        // cluster reaches rather than for a span to pass.
        let settled = read_until(
            case,
            "the cluster reports the load balancing capability the update asked for",
            || describe(&router, cluster),
            |answer| answer.cluster().pointer(LOAD_BALANCING) == Some(&expected),
        )
        .await;
        assert_eq!(
            reported(&settled),
            expected,
            "case {case:?}: expected the cluster to report {expected}, got {:?}",
            reported(&settled)
        );
    }
}
