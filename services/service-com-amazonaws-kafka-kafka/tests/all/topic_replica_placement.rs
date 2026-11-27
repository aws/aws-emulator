// SPDX-License-Identifier: CC0-1.0

//! The topic REPLICA PLACEMENT the control plane reports, and the BROKER TYPE each
//! node reports, driven end to end over the same `MultiServiceRouter` the emulator
//! serves from.
//!
//! RULE 1: a topic's `ReplicaIds` are the broker ids Apache Kafka placed its
//! partition replicas on — partition `p` is placed on `min(ReplicationFactor,
//! brokers)` brokers starting at the broker its own index falls on — reported once
//! each, ascending, and the same by every reader of that topic (`DescribeTopic` and
//! `ListTopics`).
//!
//! RULE 2: a broker node reports which broker type it runs: `EXPRESS` when its
//! instance type belongs to the Express broker category (`express.*`) and
//! `STANDARD` otherwise, for every node of the cluster.
//!
//! Each rule is asserted over a TABLE rather than one request: the rows differ
//! along what the rule generalises over — the cluster's broker count, the topic's
//! partition count and replication factor (one, two, every broker, and a factor
//! above the broker count), and the broker category and size the cluster runs.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared harness.

use serde_json::{json, Value};

use crate::kafka_harness::{pct, poll_until_active, ClusterRequest, Emulator, Reply};

/// Stand a provisioned cluster of `brokers` nodes on `instance_type` up, wait for
/// it to settle, and hand back its ARN: a topic is created on a cluster that is
/// serving.
async fn active_cluster(
    emulator: &Emulator,
    name: &str,
    brokers: i64,
    instance_type: &'static str,
) -> String {
    let mut request = ClusterRequest::named(name);
    request.brokers = brokers;
    request.instance_type = instance_type;
    let arn = emulator.create_ok(&request).await;
    poll_until_active(emulator, &arn, name).await;
    arn
}

async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(json!({
                "topicName": name,
                "partitionCount": partitions,
                "replicationFactor": replication,
            })),
        )
        .await
}

async fn describe_topic(emulator: &Emulator, cluster_arn: &str, topic: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/topics/{}", pct(cluster_arn), pct(topic)),
            None,
        )
        .await
}

/// The broker ids a reader reported, as a plain `Vec<i64>`.
fn ids(value: &Value, case: &str, what: &str) -> Vec<i64> {
    value
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected {what} to be a list, got {value}"))
        .iter()
        .map(|id| {
            id.as_i64()
                .unwrap_or_else(|| panic!("case {case}: {what} holds a non-integer id: {id}"))
        })
        .collect()
}

/// The brokers Apache Kafka's default assignment places `partitions` partitions of
/// a topic at `replication` on, over a cluster of `brokers` nodes: the union, over
/// the partitions, of the `min(replication, brokers)` broker ids starting at the
/// partition's own index — the placement the cluster's own `Metadata` publishes.
fn expected_replica_ids(brokers: i64, partitions: i64, replication: i64) -> Vec<i64> {
    let replicas = replication.clamp(1, brokers);
    let mut hosting: Vec<i64> = Vec::new();
    for partition in 0..partitions {
        for replica in 0..replicas {
            // Broker ids are 1-based, and the assignment walks them round-robin.
            let id = (partition + replica) % brokers + 1;
            if !hosting.contains(&id) {
                hosting.push(id);
            }
        }
    }
    hosting.sort_unstable();
    hosting
}

/// One row of the placement table.
#[derive(Debug, Clone, Copy)]
struct PlacementRow {
    case: &'static str,
    topic: &'static str,
    brokers: i64,
    partitions: i64,
    replication: i64,
}

/// The rows the placement rule is checked over: they differ along the broker
/// count, the partition count (one, fewer than the brokers, as many, more) and the
/// replication factor (one, two, every broker, and a factor above the broker count
/// — which Apache Kafka cannot place more replicas than brokers for).
fn placement_rows() -> Vec<PlacementRow> {
    vec![
        PlacementRow {
            case: "one-partition-one-replica",
            topic: "single",
            brokers: 3,
            partitions: 1,
            replication: 1,
        },
        PlacementRow {
            case: "two-partitions-two-replicas",
            topic: "paired",
            brokers: 3,
            partitions: 2,
            replication: 2,
        },
        PlacementRow {
            case: "every-broker",
            topic: "spread",
            brokers: 3,
            partitions: 3,
            replication: 3,
        },
        PlacementRow {
            case: "more-partitions-than-brokers",
            topic: "wide",
            brokers: 3,
            partitions: 7,
            replication: 2,
        },
        PlacementRow {
            case: "six-brokers-one-replica",
            topic: "narrow",
            brokers: 6,
            partitions: 2,
            replication: 1,
        },
        PlacementRow {
            case: "factor-above-broker-count",
            topic: "capped",
            brokers: 6,
            partitions: 6,
            replication: 6,
        },
    ]
}

/// RULE 1: every reader of a topic reports the brokers its replicas sit on, and
/// reports the same ones.
#[tokio::test(flavor = "current_thread")]
async fn every_topic_reader_reports_the_brokers_its_replicas_sit_on() {
    let emulator = Emulator::new();
    for row in placement_rows() {
        let cluster_arn = active_cluster(
            &emulator,
            &format!("replicas-{}", row.case),
            row.brokers,
            "kafka.m5.large",
        )
        .await;
        let created = create_topic(
            &emulator,
            &cluster_arn,
            row.topic,
            row.partitions,
            row.replication,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {}: expected CreateTopic to answer 200, got {} {}",
            row.case, created.status, created.body
        );

        let expected = expected_replica_ids(row.brokers, row.partitions, row.replication);
        let described = describe_topic(&emulator, &cluster_arn, row.topic).await;
        assert_eq!(
            described.status, 200,
            "case {}: expected DescribeTopic to answer 200, got {} {}",
            row.case, described.status, described.body
        );
        let reported = ids(described.at("/replicaIds"), row.case, "replicaIds");
        assert_eq!(
            reported, expected,
            "case {}: DescribeTopic must report the brokers the topic's replicas sit on, got {}",
            row.case, described.body
        );

        let listed = emulator
            .call(
                "GET",
                &format!("/v1/clusters/{}/topics", pct(&cluster_arn)),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {}: expected ListTopics to answer 200, got {} {}",
            row.case, listed.status, listed.body
        );
        let entry = listed
            .at("/topics")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .find(|entry| entry.get("topicName") == Some(&json!(row.topic)))
            .unwrap_or_else(|| {
                panic!(
                    "case {}: {} is missing from the listing: {}",
                    row.case, row.topic, listed.body
                )
            });
        assert_eq!(
            ids(
                entry.get("replicaIds").unwrap_or(&Value::Null),
                row.case,
                "replicaIds"
            ),
            expected,
            "case {}: the listing must report the same brokers the reader does, got {entry}",
            row.case
        );
    }
}

/// One row of the broker-type table: the instance type a cluster's brokers run and
/// the `BrokerNodeType` every one of its nodes must report.
#[derive(Debug, Clone, Copy)]
struct NodeTypeRow {
    case: &'static str,
    instance_type: &'static str,
    node_type: &'static str,
}

/// The rows that rule is checked over: both broker categories, and more than one
/// size and family within them, since the category — not the size — is what the
/// node type follows. Source: the broker types the service offers
/// (`crate::kafka_harness` builds its clusters from them, and the service's own
/// catalog names them).
fn node_type_rows() -> Vec<NodeTypeRow> {
    vec![
        NodeTypeRow {
            case: "standard-m5",
            instance_type: "kafka.m5.large",
            node_type: "STANDARD",
        },
        NodeTypeRow {
            case: "standard-m7g",
            instance_type: "kafka.m7g.xlarge",
            node_type: "STANDARD",
        },
        NodeTypeRow {
            case: "standard-t3",
            instance_type: "kafka.t3.small",
            node_type: "STANDARD",
        },
        NodeTypeRow {
            case: "express-large",
            instance_type: "express.m7g.large",
            node_type: "EXPRESS",
        },
        NodeTypeRow {
            case: "express-4xlarge",
            instance_type: "express.m7g.4xlarge",
            node_type: "EXPRESS",
        },
    ]
}

/// RULE 2: `ListNodes` reports the broker type every node of the cluster runs.
#[tokio::test(flavor = "current_thread")]
async fn every_broker_node_reports_the_broker_type_it_runs() {
    let emulator = Emulator::new();
    for row in node_type_rows() {
        let cluster_arn = active_cluster(
            &emulator,
            &format!("node-type-{}", row.case),
            3,
            row.instance_type,
        )
        .await;
        let listed = emulator
            .call(
                "GET",
                &format!("/v1/clusters/{}/nodes", pct(&cluster_arn)),
                None,
            )
            .await;
        assert_eq!(
            listed.status, 200,
            "case {}: expected ListNodes to answer 200, got {} {}",
            row.case, listed.status, listed.body
        );
        let nodes = listed
            .at("/nodeInfoList")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            nodes.len(),
            3,
            "case {}: expected the cluster's three nodes, got {}",
            row.case,
            listed.body
        );
        for node in &nodes {
            assert_eq!(
                node.pointer("/brokerNodeInfo/brokerNodeType"),
                Some(&json!(row.node_type)),
                "case {}: a node on {} must report brokerNodeType {}, got {node}",
                row.case,
                row.instance_type,
                row.node_type
            );
            assert_eq!(
                node.get("instanceType"),
                Some(&json!(row.instance_type)),
                "case {}: the node must report the instance type it runs, got {node}",
                row.case
            );
        }
    }
}
