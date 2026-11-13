// SPDX-License-Identifier: CC0-1.0

//! The cluster operation a `RebootBroker` leaves behind: what state it is reported
//! in, and how long it is observably in flight. Driven end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! The rules under test, each asserted over a TABLE rather than one request:
//!
//! 1. a cluster operation's `OperationState` names the WORK it does, taken from its
//!    own `OperationType`: a node reboot walks `REBOOT_IN_PROGRESS` ->
//!    `REBOOT_COMPLETE` while a change to the cluster's configuration walks
//!    `UPDATE_IN_PROGRESS` -> `UPDATE_COMPLETE`, and every reader of one record —
//!    `DescribeClusterOperation`, `DescribeClusterOperationV2` and
//!    `ListClusterOperations` — reports the same one;
//! 2. a reboot is a ROLLING restart: the operation stays non-terminal for a window a
//!    client polling at a few seconds' cadence samples several times over, and
//!    throughout it the cluster keeps serving — its broker inventory never shrinks
//!    and its topic listing keeps answering — after which the operation is complete
//!    and the cluster is `ACTIVE` again at the version it already had.
//!
//! The rows differ along what the rules generalise over: the kind of work the
//! operation does (a reboot and a configuration change), how many brokers the
//! cluster holds and which of them is rebooted.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    OPERATION_STATE_COMPLETE, OPERATION_STATE_IN_PROGRESS, OPERATION_STATE_REBOOT_COMPLETE,
    OPERATION_STATE_REBOOT_IN_PROGRESS, OPERATION_TYPE_REBOOT_NODE,
    OPERATION_TYPE_UPDATE_BROKER_COUNT,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, REGION,
};

/// How often the loops below look again, and how many times at most. The cadence
/// is a client's, not the emulator's span; the ceiling is far more than any
/// transition of this service needs, so a loop ends on the CONDITION rather than on
/// a budget that has to be tuned.
const SAMPLE_INTERVAL: Duration = Duration::from_millis(1000);
const MAX_SAMPLES: usize = 120;

/// `PUT /v1/clusters/{ClusterArn}/reboot-broker`.
async fn reboot(emulator: &Emulator, cluster_arn: &str, broker_id: &str) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/reboot-broker", pct(cluster_arn)),
            Some(json!({ "brokerIds": [broker_id] })),
        )
        .await
}

/// `PUT /v1/clusters/{ClusterArn}/nodes/count` — a change to the cluster's
/// configuration, which is the other kind of work a cluster operation carries out.
async fn update_broker_count(
    emulator: &Emulator,
    cluster_arn: &str,
    current_version: &str,
    target: i64,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/nodes/count", pct(cluster_arn)),
            Some(json!({
                "currentVersion": current_version,
                "targetNumberOfBrokerNodes": target,
            })),
        )
        .await
}

/// `DescribeClusterOperation`: `GET /v1/operations/{ClusterOperationArn}`.
async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation_arn)), None)
        .await
}

/// `DescribeClusterOperationV2`: `GET /api/v2/operations/{ClusterOperationArn}`.
async fn describe_operation_v2(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/api/v2/operations/{}", pct(operation_arn)),
            None,
        )
        .await
}

/// The `OperationState` `ListClusterOperations` reports for `operation_arn` in its
/// cluster's own history — the third reader of the one record.
async fn listed_operation_state(
    emulator: &Emulator,
    cluster_arn: &str,
    operation_arn: &str,
    case: &str,
) -> String {
    let reply = emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/operations", pct(cluster_arn)),
            None,
        )
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListClusterOperations to answer 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/clusterOperationInfoList")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .find(|operation| operation["operationArn"].as_str() == Some(operation_arn))
        .and_then(|operation| operation["operationState"].as_str())
        .unwrap_or_else(|| {
            panic!("case {case}: the operation is not in its cluster's history: {}", reply.body)
        })
        .to_string()
}

/// The broker ids `ListNodes` reports, which a reboot must never take away.
async fn listed_broker_ids(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<i64> {
    let reply = emulator
        .call("GET", &format!("/v1/clusters/{}/nodes", pct(cluster_arn)), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListNodes to answer 200 throughout the reboot, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/nodeInfoList")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|node| {
            node.pointer("/brokerNodeInfo/brokerId")
                .and_then(Value::as_f64)
                .map(|id| id as i64)
        })
        .collect()
}

/// The topic names `ListTopics` reports — a read a client keeps making across the
/// reboot to show the cluster never went away.
async fn listed_topics(emulator: &Emulator, cluster_arn: &str, case: &str) -> Vec<String> {
    let reply = emulator
        .call("GET", &format!("/v1/clusters/{}/topics", pct(cluster_arn)), None)
        .await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected ListTopics to answer 200 throughout the reboot, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/topics")
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|topic| topic["topicName"].as_str().map(str::to_string))
        .collect()
}

/// A provisioned cluster of `brokers` nodes, waited into `ACTIVE`, holding one
/// topic — as (ARN, `CurrentVersion`, topic name).
async fn serving_cluster(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
) -> (String, String, String) {
    let request = ClusterRequest {
        brokers,
        subnets: vec!["subnet-a", "subnet-b", "subnet-c"],
        ..ClusterRequest::named(name)
    };
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    let topic = format!("{name}-orders");
    let created = emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(&arn)),
            Some(json!({
                "topicName": topic,
                "partitionCount": 1,
                "replicationFactor": if brokers > 1 { 2 } else { 1 },
            })),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "case {name}: expected CreateTopic to answer 200, got {} {}",
        created.status, created.body
    );
    let described = emulator.describe(&arn).await;
    assert_eq!(
        described.at("/clusterInfo/state").as_str(),
        Some("ACTIVE"),
        "case {name}: expected the new cluster to settle ACTIVE, got {}",
        described.body
    );
    let version = described
        .at("/clusterInfo/currentVersion")
        .as_str()
        .unwrap_or_else(|| panic!("case {name}: DescribeCluster reported no currentVersion"))
        .to_string();
    (arn, version, topic)
}

/// Look again until the cluster reports `state`, answering the state it ended on.
async fn poll_cluster_state(emulator: &Emulator, cluster_arn: &str, state: &str, case: &str) {
    for _ in 0..MAX_SAMPLES {
        let described = emulator.describe(cluster_arn).await;
        if described.at("/clusterInfo/state").as_str() == Some(state) {
            return;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
    }
    let described = emulator.describe(cluster_arn).await;
    panic!(
        "case {case}: expected the cluster to reach {state}, it reports {}",
        described.body
    );
}

/// RULE 1: a cluster operation is reported in the pair of states its own
/// `OperationType` names — a node reboot in the `REBOOT_*` pair and a change to the
/// cluster's configuration in the `UPDATE_*` pair — and all three readers of that
/// one record agree, from the moment it is filed to the moment it completes.
#[tokio::test]
async fn an_operations_state_pair_is_named_by_the_work_it_does() {
    let emulator = Emulator::new();

    // Rows: the two kinds of work this service files a cluster operation for, each
    // with the cluster it runs on, the operation type it records and the state pair
    // that type is reported in.
    let rows: [(&str, &str, &str, (&str, &str)); 2] = [
        (
            "a node reboot",
            "pair-reboot",
            OPERATION_TYPE_REBOOT_NODE,
            (
                OPERATION_STATE_REBOOT_IN_PROGRESS,
                OPERATION_STATE_REBOOT_COMPLETE,
            ),
        ),
        (
            "a broker-count change",
            "pair-broker-count",
            OPERATION_TYPE_UPDATE_BROKER_COUNT,
            (OPERATION_STATE_IN_PROGRESS, OPERATION_STATE_COMPLETE),
        ),
    ];

    for (case, cluster_name, operation_type, (in_progress, complete)) in rows {
        let (arn, version, _) = serving_cluster(&emulator, cluster_name, 2).await;
        let reply = if operation_type == OPERATION_TYPE_REBOOT_NODE {
            reboot(&emulator, &arn, "1").await
        } else {
            update_broker_count(&emulator, &arn, &version, 3).await
        };
        assert_eq!(
            reply.status, 200,
            "case {case}: expected the call to be accepted, got {} {}",
            reply.status, reply.body
        );
        let operation_arn = reply.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no clusterOperationArn in {}", reply.body))
            .to_string();

        // Filed: whatever the clock has done by now, the state is one of THIS
        // operation's pair and never one of the other kind's.
        let filed = describe_operation(&emulator, &operation_arn).await;
        let filed_state = filed
            .at("/clusterOperationInfo/operationState")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            filed.at("/clusterOperationInfo/operationType").as_str(),
            Some(operation_type),
            "case {case}: expected operationType {operation_type}, got {}",
            filed.body
        );
        assert!(
            filed_state == in_progress || filed_state == complete,
            "case {case}: expected one of ({in_progress}, {complete}), got {filed_state:?}"
        );

        // Complete: the terminal half of the same pair, reported identically by the
        // V1 reader, the V2 reader and the cluster's own operation history.
        let mut terminal = None;
        for _ in 0..MAX_SAMPLES {
            let described = describe_operation(&emulator, &operation_arn).await;
            if described
                .at("/clusterOperationInfo/operationState")
                .as_str()
                == Some(complete)
            {
                terminal = Some(described);
                break;
            }
            std::thread::sleep(SAMPLE_INTERVAL);
        }
        let terminal = terminal.unwrap_or_else(|| {
            panic!("case {case}: the operation never reported {complete}");
        });
        assert!(
            terminal.at("/clusterOperationInfo/endTime").is_string(),
            "case {case}: a finished operation reports when it ended, got {}",
            terminal.body
        );
        let v2 = describe_operation_v2(&emulator, &operation_arn).await;
        assert_eq!(
            (
                v2.at("/clusterOperationInfo/operationState").as_str(),
                v2.at("/clusterOperationInfo/operationType").as_str(),
            ),
            (Some(complete), Some(operation_type)),
            "case {case}: the V2 reader must report the same record, got {}",
            v2.body
        );
        let listed = listed_operation_state(&emulator, &arn, &operation_arn, case).await;
        assert_eq!(
            listed, complete,
            "case {case}: the cluster's operation history must report the same state, got \
             {listed:?}"
        );
        poll_cluster_state(&emulator, &arn, "ACTIVE", case).await;
    }
}

/// RULE 2: a reboot is a rolling restart — its operation stays non-terminal for a
/// window a client polling at a few seconds' cadence observes several times over,
/// the cluster keeps serving throughout (the broker inventory never shrinks and the
/// topic listing keeps answering), and when it ends the cluster is `ACTIVE` again at
/// the version it already had with the same nodes.
#[tokio::test]
async fn a_reboot_is_observably_in_flight_while_the_cluster_keeps_serving() {
    /// The fewest consecutive samples of the in-flight operation a client polling
    /// at [`SAMPLE_INTERVAL`] must be able to take: below three there is no window
    /// in which to observe that the cluster stayed reachable.
    const MIN_IN_FLIGHT_SAMPLES: usize = 3;

    let emulator = Emulator::new();

    // Rows: how many brokers the cluster holds and which of them is rebooted —
    // the first of two, and a middle one of three.
    let rows: [(&str, i64, &str); 2] = [
        ("the first of two brokers", 2, "1"),
        ("the middle of three brokers", 3, "2"),
    ];

    for (case, brokers, broker_id) in rows {
        let (arn, version, topic) =
            serving_cluster(&emulator, &format!("roll-{brokers}"), brokers).await;
        let nodes_before = listed_broker_ids(&emulator, &arn, case).await;
        assert_eq!(
            nodes_before.len(),
            brokers as usize,
            "case {case}: expected {brokers} nodes before the reboot, got {nodes_before:?}"
        );

        let accepted = reboot(&emulator, &arn, broker_id).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected RebootBroker to answer 200, got {} {}",
            accepted.status, accepted.body
        );
        let operation_arn = accepted.body["clusterOperationArn"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: no clusterOperationArn in {}", accepted.body))
            .to_string();

        // Sample the operation the way a client that drains around a rolling
        // restart does: every interval, and alongside it the two reads that show
        // the cluster still serving. The loop ends on the CONDITION — the operation
        // reporting its terminal state — not on a fixed number of rounds.
        let mut in_flight_samples = 0_usize;
        // The topics the LAST sample read — the one that saw the reboot finish, so
        // this is what a client checking "the cluster is still serving as the
        // restart lands" sees.
        let mut topics_on_the_landing_sample: Option<Vec<String>> = None;
        for _ in 0..MAX_SAMPLES {
            let described = describe_operation(&emulator, &operation_arn).await;
            assert_eq!(
                described.status, 200,
                "case {case}: the operation must stay readable, got {} {}",
                described.status, described.body
            );
            let state = described
                .at("/clusterOperationInfo/operationState")
                .as_str()
                .unwrap_or_default()
                .to_string();
            let nodes = listed_broker_ids(&emulator, &arn, case).await;
            assert_eq!(
                nodes, nodes_before,
                "case {case}: the broker inventory must never shrink during a reboot, got \
                 {nodes:?}"
            );
            let topics = listed_topics(&emulator, &arn, case).await;
            assert!(
                topics.contains(&topic),
                "case {case}: the topic listing must keep reporting {topic:?}, got {topics:?}"
            );
            if state == OPERATION_STATE_REBOOT_COMPLETE {
                topics_on_the_landing_sample = Some(topics);
                break;
            }
            assert_eq!(
                state, OPERATION_STATE_REBOOT_IN_PROGRESS,
                "case {case}: a reboot that has not finished reports \
                 {OPERATION_STATE_REBOOT_IN_PROGRESS}, got {state:?}"
            );
            in_flight_samples += 1;
            std::thread::sleep(SAMPLE_INTERVAL);
        }
        // The landing sample is the witness that the reboot finished at all, and
        // the per-sample assertions above have already held for it: the inventory
        // was whole and the topic listing answered on the very sample that saw the
        // operation complete.
        assert!(
            topics_on_the_landing_sample.is_some(),
            "case {case}: the reboot never reported {OPERATION_STATE_REBOOT_COMPLETE}"
        );
        assert!(
            in_flight_samples >= MIN_IN_FLIGHT_SAMPLES,
            "case {case}: the reboot was observably in flight on only {in_flight_samples} \
             sample(s), too few to observe a rolling restart"
        );

        // Landed: the cluster is serving again, at the same version, with the same
        // nodes — a reboot changes no cluster attribute.
        poll_cluster_state(&emulator, &arn, "ACTIVE", case).await;
        let after = emulator.describe(&arn).await;
        assert_eq!(
            after.at("/clusterInfo/currentVersion").as_str(),
            Some(version.as_str()),
            "case {case}: a reboot generates no new cluster version, got {}",
            after.body
        );
        assert!(
            after.at("/clusterInfo/activeOperationArn").is_null(),
            "case {case}: a finished reboot leaves no active operation, got {}",
            after.body
        );
        let nodes_after = listed_broker_ids(&emulator, &arn, case).await;
        assert_eq!(
            nodes_after, nodes_before,
            "case {case}: the rebooted cluster must hold the same nodes, got {nodes_after:?}"
        );
        // The endpoints a client reaches the cluster on are still published.
        let brokers_reply = emulator.bootstrap_brokers(&arn).await;
        assert_eq!(
            brokers_reply.status, 200,
            "case {case}: expected the bootstrap endpoints to be served after the reboot, got \
             {} {}",
            brokers_reply.status, brokers_reply.body
        );
        assert!(
            brokers_reply.body.as_object().is_some_and(|body| !body.is_empty()),
            "case {case}: expected bootstrap broker strings in {REGION}, got {}",
            brokers_reply.body
        );
    }
}
