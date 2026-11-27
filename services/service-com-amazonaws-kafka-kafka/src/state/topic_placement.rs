// SPDX-License-Identifier: CC0-1.0

//! What Apache Kafka can place a topic's partition replicas ON.
//!
//! A partition is replicated onto as many brokers as its replication factor
//! names, and each of those replicas has to sit on a broker that is UP: Apache
//! Kafka's controller assigns the replicas of every partition it is asked to
//! place over the brokers available to it at that instant, and refuses an
//! assignment it cannot make. That is one rule over two counts — the factor
//! asked for and the brokers available — so it lives here, beside the state
//! module rather than in it, and every caller that places partitions applies it:
//! the control plane's `CreateTopic` and `UpdateTopic`, and the `CreateTopics`
//! and `CreatePartitions` the cluster's own brokers serve.
//!
//! How many brokers a cluster has available is the cluster record's own answer
//! ([`super::Cluster::available_broker_count`]), since a node a reboot operation
//! has in flight is not one the controller can place a replica on.

/// The sentence Apache Kafka refuses a replica assignment it cannot place with:
/// fixed text around the two counts the controller compared — the factor asked
/// for, and the brokers it had to place those replicas on.
///
/// Both numbers come from state (the factor the topic carries or the request
/// named, and the cluster's available broker count); nothing else of the request
/// is echoed.
pub fn replication_factor_shortfall_message(
    replication_factor: i32,
    available_brokers: i32,
) -> String {
    format!(
        "Replication factor: {replication_factor} larger than available brokers: \
         {available_brokers}."
    )
}

/// The broker ids ONE partition's replicas sit on: as many brokers as the
/// replication factor names, starting at the broker the partition's own index
/// falls on and taken in order from there, which is how Apache Kafka lays a
/// topic's partitions out over the brokers available to it.
///
/// `broker_ids` is the cluster's own broker node list, in id order, and a
/// partition cannot be placed on more brokers than the cluster has, so the
/// factor is read within that count. The leader is the first of them. Empty for
/// a cluster that publishes no broker ids at all: there is nothing to place a
/// replica on.
///
/// The ONE placement of this service, so the brokers the cluster's own metadata
/// names for a partition and the brokers its control plane reports as that
/// topic's replicas ([`topic_replica_broker_ids`]) are the same brokers.
pub fn partition_replica_broker_ids(
    partition_index: i32,
    replication_factor: i32,
    broker_ids: &[i32],
) -> Vec<i32> {
    if broker_ids.is_empty() {
        return Vec::new();
    }
    let replicas = usize::try_from(replication_factor)
        .unwrap_or(1)
        .clamp(1, broker_ids.len());
    let first = usize::try_from(partition_index).unwrap_or(0);
    (0..replicas)
        .map(|replica| broker_ids[first.saturating_add(replica) % broker_ids.len()])
        .collect()
}

/// The broker ids a WHOLE topic's partition replicas sit on, each reported once
/// and in id order: the union of [`partition_replica_broker_ids`] over the
/// topic's partitions.
///
/// This is what `TopicInfo$ReplicaIds` and `DescribeTopicResponse$ReplicaIds`
/// report ("The broker IDs hosting replicas of this topic's partitions"), so it
/// is derived from the one placement rather than stored a second time: a topic
/// of a cluster whose brokers change reports the brokers it is placed on now.
///
/// The walk is bounded by the cluster's BROKER COUNT rather than by the topic's
/// partition count, which a request chooses: the placement moves one broker per
/// partition, so after as many partitions as the cluster has brokers the union is
/// every broker and no further partition can add to it. A topic of a million
/// partitions therefore costs the same walk as one of ten.
pub fn topic_replica_broker_ids(
    partition_count: i32,
    replication_factor: i32,
    broker_ids: &[i32],
) -> Vec<i32> {
    let covering = i32::try_from(broker_ids.len()).unwrap_or(i32::MAX);
    let mut hosting: std::collections::BTreeSet<i32> = std::collections::BTreeSet::new();
    for partition in 0..partition_count.max(0).min(covering) {
        hosting.extend(partition_replica_broker_ids(
            partition,
            replication_factor,
            broker_ids,
        ));
        // Every broker already holds a replica, so no later partition can add one.
        if hosting.len() == broker_ids.len() {
            break;
        }
    }
    hosting.into_iter().collect()
}

/// Why `replication_factor` replicas of a partition cannot be placed on the
/// brokers a cluster has available, or `None` when they can.
///
/// One replica per broker, so a factor above the number of brokers available
/// describes a placement Apache Kafka cannot make. `available_brokers` is `None`
/// for a cluster that publishes no broker nodes of its own — an Amazon MSK
/// Serverless cluster's brokers are the service's — and such a cluster bounds
/// nothing here: there is no count to compare against, so the KIND of cluster is
/// what decides that call rather than a count it does not report.
pub fn replication_factor_shortfall(
    replication_factor: i32,
    available_brokers: Option<i32>,
) -> Option<String> {
    available_brokers
        .filter(|available| replication_factor > *available)
        .map(|available| replication_factor_shortfall_message(replication_factor, available))
}
