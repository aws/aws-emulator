// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a mutation of a cluster's CLIENT-CONNECTIVITY surface takes for a
//! cluster ARN this endpoint holds no cluster for, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! RULE under test: the surface clients REACH a cluster over — its public access and
//! its multi-VPC private connectivity — is the surface a cluster's own
//! resource-based policy is attached for, so the only document that can grant a call
//! on it lives ON the target cluster; an ARN this endpoint's Region holds no cluster
//! under carries no such document, so the call reaches no grant and is refused as
//! UNAUTHORIZED (HTTP 403 `AccessDeniedException`) in IAM's sentence for a request no
//! resource-based policy allows, naming the calling principal, this operation's IAM
//! action and the ARN exactly as submitted — and never telling the caller whether
//! this registry holds anything under it. `UpdateConnectivity` is the call that
//! CONFIGURES that surface; `RejectClientVpcConnection`, which manages a connection
//! made over it, is pinned in `client_vpc_connection_rejection.rs`.
//!
//! The rows differ along what the rule generalises over: the way this endpoint comes
//! to hold no cluster under the submitted ARN (a name never created, an ARN of the
//! shape this service mints with another name and id, a cluster id with a different
//! revision suffix, a cluster that was created and then deleted, and a live
//! cluster's own ARN seen from another region's endpoint), and the BODY the request
//! carries (each connectivity member the operation can change, a version token the
//! cluster would refuse, and a body naming nothing to change at all), since the
//! decision over the target precedes every member of the body.
//!
//! The boundaries are pinned too: a value that is no cluster ARN at all is left to
//! the operation's own parameter refusal (HTTP 400), a mutation of any OTHER part of
//! a cluster rests on the caller's own identity documents and so reports the same
//! ARNs as merely absent (HTTP 404), the REPORTING side of those ARNs reports the
//! absence as well, and the cluster this endpoint DOES hold has its connectivity
//! update accepted and applied — so the rows above are refusals about the target
//! rather than about the caller or the request.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::authorization::caller_principal;

use crate::kafka_harness::{
    pct, poll_until_active, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The status and wire code an authorization refusal carries, spelled out here
/// rather than read from the service so a drift in its own constants fails this
/// suite.
const ACCESS_DENIED_STATUS: u16 = 403;
const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// The IAM action a call to this operation asks for: `kafka:` plus the API name,
/// which is how every control-plane action of this service is published.
const ACTION: &str = "kafka:UpdateConnectivity";

/// The status and wire code the service reports a resource it does not hold with,
/// which the boundary controls below pin for the surfaces the rule does not cover.
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND_CODE: &str = "NotFoundException";

/// The whole sentence that refusal carries: fixed text, naming nothing of the
/// request.
const NOT_FOUND_MESSAGE: &str = "The requested resource doesn\u{2019}t exist.";

/// The status and wire code of the parameter refusal `UpdateConnectivity` answers a
/// value that is no cluster ARN at all with, which the control below pins.
const BAD_REQUEST_STATUS: u16 = 400;
const BAD_REQUEST_CODE: &str = "BadRequestException";

/// `UpdateConnectivity` against `target`, as the endpoint of `region` serves it.
async fn update_connectivity_in(
    emulator: &Emulator,
    region: &str,
    target: &str,
    body: &Value,
) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!("/v1/clusters/{}/connectivity", pct(target)),
            Some(body.clone()),
        )
        .await
}

/// The sentence IAM refuses a request no RESOURCE-based policy allows with: fixed
/// text around the principal that signed, the action asked for and the resource the
/// decision was taken over.
fn denial_message(resource: &str) -> String {
    format!(
        "User: {} is not authorized to perform: {ACTION} on resource: {resource} \
         because no resource-based policy allows the {ACTION} action",
        caller_principal()
    )
}

/// Assert `reply` refused the call as unauthorized over `target` as submitted, and
/// disclosed nothing about what this registry holds.
fn assert_refused_as_unauthorized(reply: &Reply, case: &str, target: &str) {
    let expected = denial_message(target);
    assert_eq!(
        (reply.status, reply.error_code(), reply.error_message()),
        (
            ACCESS_DENIED_STATUS,
            ACCESS_DENIED_CODE,
            expected.as_str()
        ),
        "case {case:?}: expected {ACCESS_DENIED_STATUS} {ACCESS_DENIED_CODE} {expected:?}, \
         got {} {}",
        reply.status,
        reply.body
    );
    // An authorization refusal is decided from the request alone, so it complains
    // about no parameter — and it says nothing about the resource either way.
    assert_eq!(
        reply.at("/invalidParameter"),
        &Value::Null,
        "case {case:?}: an authorization refusal names no parameter at fault, got {}",
        reply.body
    );
}

/// A second region's endpoint, for the row that addresses a live cluster from an
/// endpoint that does not hold it.
const OTHER_REGION: &str = "eu-west-1";

/// Every body `UpdateConnectivity` can carry while the TARGET is decided before any
/// of it is read: each member the operation can change, a version token the cluster
/// would refuse, and a body that names nothing to change at all — which is a defect
/// of the REQUEST, answered only for a target the call reaches a grant on.
fn connectivity_bodies(version: &str) -> Vec<(&'static str, Value)> {
    vec![
        (
            "public-access-on",
            json!({
                "currentVersion": version,
                "connectivityInfo": { "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" } },
            }),
        ),
        (
            "public-access-off",
            json!({
                "currentVersion": version,
                "connectivityInfo": { "publicAccess": { "type": "DISABLED" } },
            }),
        ),
        (
            "vpc-connectivity-sasl-iam",
            json!({
                "currentVersion": version,
                "connectivityInfo": {
                    "vpcConnectivity": {
                        "clientAuthentication": { "sasl": { "iam": { "enabled": true } } },
                    },
                },
            }),
        ),
        (
            "network-type",
            json!({
                "currentVersion": version,
                "connectivityInfo": { "networkType": "DUAL" },
            }),
        ),
        (
            "zookeeper-access-only",
            json!({
                "currentVersion": version,
                "zookeeperAccess": { "enabled": true },
            }),
        ),
        (
            "a-version-the-cluster-would-refuse",
            json!({
                "currentVersion": "not-a-version-token",
                "connectivityInfo": { "publicAccess": { "type": "DISABLED" } },
            }),
        ),
        ("nothing-to-change", json!({ "currentVersion": version })),
    ]
}

/// Mutations of any OTHER part of a cluster, which rest on the caller's own
/// identity-based documents rather than on a document attached to the cluster:
/// `(case, method, URI suffix, body members)`, each one legal so the row exercises
/// the target rather than the request.
fn other_cluster_mutations(version: &str) -> Vec<(&'static str, &'static str, &'static str, Value)> {
    vec![
        (
            "UpdateStorage",
            "PUT",
            "/storage",
            json!({ "currentVersion": version, "volumeSizeGB": 200 }),
        ),
        (
            "UpdateMonitoring",
            "PUT",
            "/monitoring",
            json!({ "currentVersion": version, "enhancedMonitoring": "PER_BROKER" }),
        ),
        (
            "UpdateBrokerCount",
            "PUT",
            "/nodes/count",
            json!({ "currentVersion": version, "targetNumberOfBrokerNodes": 6 }),
        ),
    ]
}

/// RULE: a connectivity mutation handed a well-formed cluster ARN this endpoint
/// holds no cluster for reaches no grant, so it is refused as unauthorized over that
/// ARN — whatever the body carries and however the endpoint came to hold no cluster
/// under it.
#[tokio::test]
async fn an_absent_cluster_refuses_a_connectivity_mutation_as_unauthorized() {
    let emulator = Emulator::new();
    // A cluster this endpoint really holds, so the version the rows quote is one a
    // request could otherwise be accepted at, and the controls at the end have a
    // cluster to be served for.
    let live = emulator
        .create_ok(&ClusterRequest::named("connectivity-refusal"))
        .await;
    let version = poll_until_active(&emulator, &live, "the created cluster").await;

    // A cluster created and then DELETED: the registry holds no cluster under its
    // ARN any more, which the rule covers exactly as a name never created.
    let deleted = emulator
        .create_ok(&ClusterRequest::named("connectivity-refusal-deleted"))
        .await;
    let deleted_version = poll_until_active(&emulator, &deleted, "the cluster to delete").await;
    let removed = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}?currentVersion={}",
                pct(&deleted),
                deleted_version
            ),
            None,
        )
        .await;
    assert_eq!(
        removed.status, 200,
        "case \"deleted\": expected DeleteCluster to be accepted, got {} {}",
        removed.status, removed.body
    );

    // Every way this endpoint holds no cluster under a well-formed cluster ARN of
    // the caller's own account, as `(case, ARN, the endpoint's region)`.
    let targets: Vec<(&str, String, &str)> = vec![
        (
            "never-created",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/never-created/\
                 11111111-2222-4333-8444-555555555555-1"
            ),
            REGION,
        ),
        (
            "another-name-and-id",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/other-name/\
                 99999999-8888-4777-8666-555555555555-1"
            ),
            REGION,
        ),
        (
            "another-revision-suffix",
            format!(
                "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/never-created/\
                 11111111-2222-4333-8444-555555555555-7"
            ),
            REGION,
        ),
        ("deleted", deleted.clone(), REGION),
        ("another-region", live.clone(), OTHER_REGION),
    ];

    for (case, target, region) in &targets {
        for (body_case, body) in connectivity_bodies(&version) {
            let reply = update_connectivity_in(&emulator, region, target, &body).await;
            assert_refused_as_unauthorized(&reply, &format!("{case}/{body_case}"), target);
        }
    }

    // CONTROL (the boundary below the rule): a value that is no cluster reference at
    // all names no cluster whose document could have granted the call, so it is left
    // to the operation's own parameter refusal.
    for (case, value) in [
        ("not-an-arn", "kafka-cluster".to_string()),
        (
            "another-resource-type",
            format!("arn:aws:kafka:{REGION}:{ACCOUNT}:configuration/cfg/abc-1"),
        ),
    ] {
        let reply = update_connectivity_in(
            &emulator,
            REGION,
            &value,
            &json!({
                "currentVersion": version,
                "connectivityInfo": { "publicAccess": { "type": "DISABLED" } },
            }),
        )
        .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (BAD_REQUEST_STATUS, BAD_REQUEST_CODE),
            "case {case:?}: expected the parameter refusal, got {} {}",
            reply.status,
            reply.body
        );
    }

    // CONTROL (the boundary beside the rule): a mutation of any other part of the
    // same clusters rests on the caller's own identity documents, which are in force
    // whether or not the cluster exists, so those report the absence instead.
    for (case, target, region) in &targets {
        for (operation, method, suffix, body) in other_cluster_mutations(&version) {
            let reply = emulator
                .call_in(
                    region,
                    method,
                    &format!("/v1/clusters/{}{suffix}", pct(target)),
                    Some(body),
                )
                .await;
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (NOT_FOUND_STATUS, NOT_FOUND_CODE, NOT_FOUND_MESSAGE),
                "case {case:?}/{operation}: expected the absence to be reported, got {} {}",
                reply.status,
                reply.body
            );
        }
    }

    // CONTROL (the same boundary, on the reporting side): the cluster's own reader
    // answers the same ARNs as absent.
    for (case, target, region) in &targets {
        let reply = emulator
            .call_in(region, "GET", &format!("/v1/clusters/{}", pct(target)), None)
            .await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (NOT_FOUND_STATUS, NOT_FOUND_CODE),
            "case {case:?}/DescribeCluster: expected the absence to be reported, got {} {}",
            reply.status,
            reply.body
        );
    }

    // CONTROL (the boundary above the rule): the cluster this endpoint DOES hold
    // carries the document the decision could rest on, so its connectivity update is
    // reached — accepted at the version the rows quoted, and refused on the REQUEST
    // alone when the body names nothing to change.
    let empty = update_connectivity_in(
        &emulator,
        REGION,
        &live,
        &json!({ "currentVersion": version }),
    )
    .await;
    assert_eq!(
        (empty.status, empty.error_code()),
        (BAD_REQUEST_STATUS, BAD_REQUEST_CODE),
        "case \"a cluster this endpoint holds\"/nothing-to-change: expected the request's own \
         refusal, got {} {}",
        empty.status,
        empty.body
    );
    let accepted = update_connectivity_in(
        &emulator,
        REGION,
        &live,
        &json!({
            "currentVersion": version,
            "connectivityInfo": { "publicAccess": { "type": "SERVICE_PROVIDED_EIPS" } },
        }),
    )
    .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a cluster this endpoint holds\": expected the update to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let operation = accepted.at("/clusterOperationArn").as_str().unwrap_or("");
    assert!(
        !operation.is_empty(),
        "case \"a cluster this endpoint holds\": an accepted update must answer with the \
         cluster operation it filed, got {}",
        accepted.body
    );

    // And the refusals persisted nothing: the update just accepted is the FIRST one
    // this cluster carries, read back through the cluster's own reader once the clock
    // has made it due.
    let settled = poll_until_active(&emulator, &live, "the accepted update").await;
    assert_ne!(
        settled, version,
        "case \"a cluster this endpoint holds\": an accepted update must move the cluster's \
         version, got {settled:?}"
    );
    let described = emulator.describe(&live).await;
    assert_eq!(
        described.at("/clusterInfo/brokerNodeGroupInfo/connectivityInfo/publicAccess/type"),
        &json!("SERVICE_PROVIDED_EIPS"),
        "case \"a cluster this endpoint holds\": expected the applied connectivity to read \
         back, got {}",
        described.body
    );
}
