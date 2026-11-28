// SPDX-License-Identifier: CC0-1.0

//! `UpdateRebalancing`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: an update of a provisioned cluster's intelligent-rebalancing
//! status is accepted only for a cluster this endpoint's region holds that is in a
//! state a change is accepted from and whose stored version is the one the request
//! names, and it is applied asynchronously — the call hands back a
//! cluster-operation ARN and the cluster enters `UPDATING` with that operation as
//! its active one, keeping its old status and version; once the clock makes the
//! operation due the submitted status is the cluster's own, every reader reports
//! it, and the cluster gets a new version and returns to `ACTIVE`. A refused call
//! changes nothing.
//!
//! The rows differ along what the rule generalises over: which status the request
//! names, the status the cluster starts from (including a cluster created without
//! the setting at all), and the broker category the cluster runs — the setting is
//! the cluster's whichever of them it was created with.

use serde_json::{json, Value};

// The service's own operation type, spans and refusal wordings, so the suite and
// the handlers cannot drift apart on what the service does or says.
use service_com_amazonaws_kafka_kafka::ops::types::REBALANCING_STATUS_VALUES;
use service_com_amazonaws_kafka_kafka::state::{
    CLUSTER_VERSION_NOT_CURRENT_MESSAGE, CURRENT_VERSION_MEMBER, OPERATION_TYPE_UPDATE_REBALANCING,
    PROVISIONED_ONLY_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{
    assert_cluster_state_refusal, pct, sleep_past_activation, sleep_past_update, ClusterRequest,
    Emulator, Reply, ACCOUNT, REGION,
};

/// `UpdateRebalancing`, which the model binds to
/// `PUT /v1/clusters/{ClusterArn}/rebalancing`, as an endpoint for `region` serves
/// it.
async fn update_in(emulator: &Emulator, region: &str, arn: &str, body: Value) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!("/v1/clusters/{}/rebalancing", pct(arn)),
            Some(body),
        )
        .await
}

/// The same call, signed for the region the clusters below live in.
async fn update(emulator: &Emulator, arn: &str, body: Value) -> Reply {
    update_in(emulator, REGION, arn, body).await
}

/// The body an update naming `status` submits.
fn body(version: &str, status: &str) -> Value {
    json!({ "currentVersion": version, "rebalancing": { "status": status } })
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn operation(emulator: &Emulator, arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(arn)), None)
        .await
}

/// `DescribeCluster`'s `clusterInfo` for `arn` — the reader that proves what the
/// service persisted.
async fn described(emulator: &Emulator, case: &str, arn: &str) -> Value {
    let reply = emulator.describe(arn).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeCluster to return 200, got {} {}",
        reply.status, reply.body
    );
    reply.at("/clusterInfo").clone()
}

/// A member of a described cluster, or `Value::Null` when it is absent.
fn member(info: &Value, pointer: &str) -> Value {
    info.pointer(pointer).cloned().unwrap_or(Value::Null)
}

/// One row of the rebalancing table.
#[derive(Debug)]
struct Case {
    /// The cluster the update starts from — its broker type and the rebalancing
    /// status it was created with are part of the row.
    created: ClusterRequest,
    /// The `rebalancing` the CREATE submits, if the starting cluster carries the
    /// setting already.
    created_status: Option<&'static str>,
    /// The status the update submits.
    status: &'static str,
}

impl Case {
    /// The create body: the row's `CreateCluster` request plus the rebalancing
    /// member the harness's builder does not carry.
    fn create_body(&self) -> Value {
        let mut body = self.created.to_json();
        if let Some(status) = self.created_status {
            body["rebalancing"] = json!({ "status": status });
        }
        body
    }
}

/// The table. The statuses come from the model's own enum
/// (`REBALANCING_STATUS_VALUES`, generated from `RebalancingStatus`), and the rows
/// cover each of them submitted onto a cluster that carried no status, the one it
/// already has, and the other one — over both broker categories Amazon MSK
/// offers, since the setting belongs to the cluster rather than to a broker type.
fn cases() -> Vec<Case> {
    let mut express = ClusterRequest::named("rebalancing-express");
    express.instance_type = "express.m7g.large";
    let mut express_paused = ClusterRequest::named("rebalancing-express-paused");
    express_paused.instance_type = "express.m7g.large";
    vec![
        Case {
            created: ClusterRequest::named("rebalancing-unset-to-paused"),
            created_status: None,
            status: "PAUSED",
        },
        Case {
            created: ClusterRequest::named("rebalancing-unset-to-active"),
            created_status: None,
            status: "ACTIVE",
        },
        Case {
            created: express,
            created_status: Some("ACTIVE"),
            status: "PAUSED",
        },
        Case {
            created: express_paused,
            created_status: Some("PAUSED"),
            status: "ACTIVE",
        },
        Case {
            created: ClusterRequest::named("rebalancing-active-to-active"),
            created_status: Some("ACTIVE"),
            status: "ACTIVE",
        },
    ]
}

/// RULE: an accepted `UpdateRebalancing` is applied asynchronously — the cluster
/// keeps its status and version and reports `UPDATING` with the returned operation
/// as its active one, and once the clock owes the operation its completion the
/// submitted status is the cluster's own, at a new version and `ACTIVE` again. The
/// operation it mints resolves for the cluster-operation reader and reports what
/// it changed.
#[tokio::test]
async fn an_accepted_update_applies_the_submitted_status_when_it_falls_due() {
    for case in cases() {
        // Each row stands up its own emulator: the rule is about one cluster's
        // own settings, and a fresh registry keeps the rows independent.
        let emulator = Emulator::new();
        let reply = emulator
            .call("POST", "/v1/clusters", Some(case.create_body()))
            .await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected CreateCluster to return 200, got {} {}",
            case.created.name, reply.status, reply.body
        );
        let arn = reply.body["clusterArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {}: no clusterArn", case.created.name))
            .to_string();
        sleep_past_activation();

        let before = described(&emulator, &case.created.name, &arn).await;
        let version = member(&before, "/currentVersion")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            member(&before, "/rebalancing/status"),
            case.created_status.map_or(Value::Null, |status| json!(status)),
            "case {}: expected the created cluster to carry the status it was created with",
            case.created.name
        );

        let accepted = update(&emulator, &arn, body(&version, case.status)).await;
        assert_eq!(
            accepted.status, 200,
            "case {}: expected UpdateRebalancing to return 200, got {} {}",
            case.created.name, accepted.status, accepted.body
        );
        assert_eq!(
            accepted.at("/clusterArn").as_str(),
            Some(arn.as_str()),
            "case {}: expected the stored cluster ARN back, got {}",
            case.created.name,
            accepted.body
        );
        let operation_arn = accepted
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {}: UpdateRebalancing returned no clusterOperationArn: {}",
                    case.created.name, accepted.body
                )
            })
            .to_string();

        // While the operation is in flight the cluster is UPDATING at its OLD
        // status, with this operation as its active one — and at the NEW version
        // the accepted update generated, which is what a further change must
        // quote.
        let during = described(&emulator, &case.created.name, &arn).await;
        assert_eq!(
            (
                member(&during, "/state"),
                member(&during, "/activeOperationArn"),
                member(&during, "/rebalancing/status"),
            ),
            (
                json!("UPDATING"),
                json!(operation_arn),
                case.created_status.map_or(Value::Null, |status| json!(status)),
            ),
            "case {}: expected an UPDATING cluster at its old status, got {during}",
            case.created.name
        );
        assert_ne!(
            member(&during, "/currentVersion"),
            json!(version),
            "case {}: an accepted update generates the cluster's next version, got {during}",
            case.created.name
        );

        // The operation the call minted is the service's own record of the change.
        let in_flight = operation(&emulator, &operation_arn).await;
        assert_eq!(
            (
                in_flight.status,
                in_flight.at("/clusterOperationInfo/operationType").as_str(),
                in_flight.at("/clusterOperationInfo/clusterArn").as_str(),
            ),
            (200, Some(OPERATION_TYPE_UPDATE_REBALANCING), Some(arn.as_str())),
            "case {}: expected the cluster operation to report this update, got {} {}",
            case.created.name,
            in_flight.status,
            in_flight.body
        );
        assert_eq!(
            in_flight
                .at("/clusterOperationInfo/targetClusterInfo/rebalancing/status")
                .as_str(),
            Some(case.status),
            "case {}: expected the operation's target to carry the submitted status, got {}",
            case.created.name,
            in_flight.body
        );
        assert_eq!(
            in_flight
                .at("/clusterOperationInfo/sourceClusterInfo/rebalancing/status")
                .as_str(),
            case.created_status,
            "case {}: expected the operation's source to carry the status the cluster had, got {}",
            case.created.name,
            in_flight.body
        );

        // Once the clock owes the operation its completion the status is the
        // cluster's own, at a new version and ACTIVE again.
        sleep_past_update();
        let after = described(&emulator, &case.created.name, &arn).await;
        assert_eq!(
            (
                member(&after, "/state"),
                member(&after, "/rebalancing/status"),
                member(&after, "/activeOperationArn"),
            ),
            (json!("ACTIVE"), json!(case.status), Value::Null),
            "case {}: expected the applied status on a settled cluster, got {after}",
            case.created.name
        );
        assert_ne!(
            member(&after, "/currentVersion"),
            json!(version),
            "case {}: a landed update must generate a new cluster version",
            case.created.name
        );
        let completed = operation(&emulator, &operation_arn).await;
        assert_eq!(
            completed
                .at("/clusterOperationInfo/operationState")
                .as_str(),
            Some("UPDATE_COMPLETE"),
            "case {}: expected the operation to report its completion, got {}",
            case.created.name,
            completed.body
        );
    }
}

/// RULE: the call is refused — changing nothing — for a cluster that is not in a
/// state a change is accepted from, for a `CurrentVersion` that is not the one the
/// cluster is at, for a cluster this endpoint's region does not hold, for a
/// serverless cluster, and for a `Rebalancing` that names no usable status.
#[tokio::test]
async fn an_update_this_cluster_cannot_take_is_refused_and_changes_nothing() {
    let emulator = Emulator::new();

    // A cluster still being provisioned has no settled configuration to change,
    // and the refusal leaves its version and its status exactly as they were.
    let creating = emulator
        .create_ok(&ClusterRequest::named("rebalancing-creating"))
        .await;
    let creating_version = member(
        &described(&emulator, "creating", &creating).await,
        "/currentVersion",
    )
    .as_str()
    .unwrap_or_default()
    .to_string();
    let refused = update(&emulator, &creating, body(&creating_version, "PAUSED")).await;
    assert_cluster_state_refusal(&refused, "creating");
    sleep_past_activation();
    let settled = described(&emulator, "creating", &creating).await;
    assert_eq!(
        (
            member(&settled, "/currentVersion"),
            member(&settled, "/rebalancing/status"),
        ),
        (json!(creating_version), Value::Null),
        "case creating: a refused update must change neither the version nor the status, got \
         {settled}"
    );

    // A stale version means the caller has not seen the cluster's latest state.
    // The quote is a token of the shape the service mints, which is what makes
    // it stale rather than unusable (`current_version_parameter`'s rule).
    let stale = update(&emulator, &creating, body("K0000000000000", "PAUSED")).await;
    assert_eq!(
        (stale.status, stale.error_code(), stale.error_message()),
        (
            400,
            "BadRequestException",
            CLUSTER_VERSION_NOT_CURRENT_MESSAGE
        ),
        "case stale version: expected the service's stale-version refusal, got {} {}",
        stale.status,
        stale.body
    );
    assert_eq!(
        stale.at("/invalidParameter").as_str(),
        Some(CURRENT_VERSION_MEMBER),
        "case stale version: expected {CURRENT_VERSION_MEMBER} named at fault, got {}",
        stale.body
    );

    // A well-formed cluster ARN this endpoint's region holds no cluster for names
    // a resource that is not there.
    let absent = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let missing = update(&emulator, &absent, body("K0000000000000", "PAUSED")).await;
    assert_eq!(
        (
            missing.status,
            missing.error_code(),
            missing.error_message()
        ),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case absent cluster: expected the service's not-found refusal, got {} {}",
        missing.status,
        missing.body
    );

    // The same cluster addressed through ANOTHER region's endpoint is a cluster
    // that endpoint does not hold.
    let elsewhere = update_in(
        &emulator,
        "us-east-1",
        &creating,
        body(&creating_version, "PAUSED"),
    )
    .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (404, "NotFoundException"),
        "case another region: expected a 404 NotFoundException, got {} {}",
        elsewhere.status,
        elsewhere.body
    );

    // A serverless cluster has no brokers of its own to balance partitions over.
    let serverless = emulator
        .create_serverless_ok(
            "rebalancing-serverless",
            &[vec!["subnet-a", "subnet-b"]],
            Some(true),
        )
        .await;
    sleep_past_activation();
    let refused = update(&emulator, &serverless, body("K0000000000000", "PAUSED")).await;
    assert_eq!(
        (
            refused.status,
            refused.error_code(),
            refused.error_message()
        ),
        (400, "BadRequestException", PROVISIONED_ONLY_MESSAGE),
        "case serverless: expected the service's provisioned-only refusal, got {} {}",
        refused.status,
        refused.body
    );

    // A `Rebalancing` naming no status asks for no change, and one naming a value
    // the model's enum does not carry names nothing the cluster could run — that
    // one is refused at the request boundary, before the handler sees it. Neither
    // reaches the cluster.
    let active = emulator
        .create_ok(&ClusterRequest::named("rebalancing-input-gates"))
        .await;
    sleep_past_activation();
    let version = member(&described(&emulator, "input gates", &active).await, "/currentVersion")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let no_status = update(
        &emulator,
        &active,
        json!({ "currentVersion": version, "rebalancing": {} }),
    )
    .await;
    assert_eq!(
        (no_status.status, no_status.error_code()),
        (400, "BadRequestException"),
        "case no status: expected a 400 BadRequestException, got {} {}",
        no_status.status,
        no_status.body
    );
    assert_eq!(
        no_status.at("/invalidParameter").as_str(),
        Some("rebalancing.status"),
        "case no status: expected the status member named at fault, got {}",
        no_status.body
    );
    // The suite's statuses are the model's own, so a value outside them is not one
    // this service may store — and the request carrying it is refused outright.
    assert!(
        !REBALANCING_STATUS_VALUES.contains(&"SUSPENDED"),
        "the model's RebalancingStatus must not carry the value this suite refuses, got {:?}",
        REBALANCING_STATUS_VALUES
    );
    let unknown = update(&emulator, &active, body(&version, "SUSPENDED")).await;
    assert_eq!(
        (unknown.status, unknown.error_code()),
        (400, "BadRequestException"),
        "case an unknown status: expected a 400 BadRequestException, got {} {}",
        unknown.status,
        unknown.body
    );

    let after = described(&emulator, "input gates", &active).await;
    assert_eq!(
        (
            member(&after, "/state"),
            member(&after, "/currentVersion"),
            member(&after, "/rebalancing/status"),
        ),
        (json!("ACTIVE"), json!(version), Value::Null),
        "a refused update must leave the cluster exactly as it was, got {after}"
    );
}
