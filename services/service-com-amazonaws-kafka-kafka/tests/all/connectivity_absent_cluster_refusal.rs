// SPDX-License-Identifier: CC0-1.0

//! WHICH refusal a mutation of a cluster's CLIENT-CONNECTIVITY surface takes for a
//! cluster ARN this endpoint holds no cluster for, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from (the shared `kafka_harness`).
//!
//! RULE under test: a cluster-scoped call handed a well-formed cluster ARN this
//! endpoint's Region holds no cluster under reports that absence as the absence it
//! is — HTTP 404 `NotFoundException`, the service's one fixed not-found sentence,
//! and the member that carried the reference named in `invalidParameter` by its WIRE
//! name (`clusterArn`) — and the call that CONFIGURES the connectivity surface
//! (`UpdateConnectivity`) is no exception: its grant rests on the caller's own
//! identity-based documents, which are in force whether or not the cluster exists.
//!
//! The rows differ along what the rule generalises over: the way this endpoint comes
//! to hold no cluster under the submitted ARN (a name never created, an ARN of the
//! shape this service mints with another name and id, a cluster id with a different
//! revision suffix, a cluster that was created and then deleted, and a live
//! cluster's own ARN seen from another region's endpoint), and the BODY the request
//! carries (each connectivity member the operation can change, a version token the
//! cluster would refuse, and a body naming nothing to change at all), since the
//! target is resolved before every member of the body.
//!
//! The boundaries are pinned too: a value that is no cluster ARN at all is left to
//! the operation's own parameter refusal (HTTP 400), a mutation of any OTHER part of
//! a cluster reports the same ARNs as absent, the REPORTING side of those ARNs
//! reports the absence as well, and the cluster this endpoint DOES hold has its
//! connectivity update accepted and applied — so the rows above are answers about
//! the target rather than about the caller or the request.

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{CLUSTER_ARN_MEMBER, RESOURCE_NOT_FOUND_MESSAGE};

use crate::kafka_harness::{
    pct, poll_until_active, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The status and wire code the service reports a resource it does not hold with,
/// spelled out here rather than read from the service so a drift in its own
/// constants fails this suite.
const NOT_FOUND_STATUS: u16 = 404;
const NOT_FOUND_CODE: &str = "NotFoundException";

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

/// Assert `reply` reported the absence of `target` as the absence it is, naming the
/// member that carried the reference by its wire name and nothing of the request in
/// its sentence.
fn assert_reported_absent(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code(), reply.error_message()),
        (NOT_FOUND_STATUS, NOT_FOUND_CODE, RESOURCE_NOT_FOUND_MESSAGE),
        "case {case:?}: expected {NOT_FOUND_STATUS} {NOT_FOUND_CODE} \
         {RESOURCE_NOT_FOUND_MESSAGE:?}, got {} {}",
        reply.status,
        reply.body
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_MEMBER),
        "case {case:?}: expected the refusal to name {CLUSTER_ARN_MEMBER} at fault, got {}",
        reply.body
    );
}

/// A second region's endpoint, for the row that addresses a live cluster from an
/// endpoint that does not hold it.
const OTHER_REGION: &str = "eu-west-1";

/// Every body `UpdateConnectivity` can carry while the TARGET is resolved before any
/// of it is read: each member the operation can change, a version token the cluster
/// would refuse, and a body that names nothing to change at all — which is a defect
/// of the REQUEST, answered only for a target this endpoint holds.
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

/// Mutations of any OTHER part of a cluster, which rest on the same identity-based
/// documents: `(case, method, URI suffix, body members)`, each one legal so the row
/// exercises the target rather than the request.
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
/// holds no cluster for reports the absence — whatever the body carries and however
/// the endpoint came to hold no cluster under it.
#[tokio::test]
async fn an_absent_cluster_is_reported_absent_by_a_connectivity_mutation() {
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
            assert_reported_absent(&reply, &format!("{case}/{body_case}"));
        }
    }

    // CONTROL (the boundary below the rule): a value that is no cluster reference at
    // all is a request this service cannot parse, which is the operation's own
    // parameter refusal rather than a resource it cannot find.
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
    // same clusters reports the same absence, so the rule is the service's rather
    // than this one operation's.
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
            assert_reported_absent(&reply, &format!("{case}/{operation}"));
        }
    }

    // CONTROL (the same boundary, on the reporting side): the cluster's own reader
    // answers the same ARNs as absent.
    for (case, target, region) in &targets {
        let reply = emulator
            .call_in(region, "GET", &format!("/v1/clusters/{}", pct(target)), None)
            .await;
        assert_reported_absent(&reply, &format!("{case}/DescribeCluster"));
    }

    // CONTROL (the boundary above the rule): the cluster this endpoint DOES hold has
    // its connectivity update reached — accepted at the version the rows quoted, and
    // refused on the REQUEST alone when the body names nothing to change.
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
