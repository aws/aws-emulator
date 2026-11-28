// SPDX-License-Identifier: CC0-1.0

//! How many brokers a cluster has AVAILABLE bounds what Apache Kafka can place,
//! driven end to end over the same `MultiServiceRouter` the emulator serves from.
//!
//! RULE under test: a partition's replicas go on brokers that are UP, so a call
//! that asks this service to PLACE partitions — `UpdateTopic` growing a topic's
//! partition count, and `CreateTopic` minting one — is admitted only while the
//! addressed cluster has at least as many brokers available as the replication
//! factor those replicas need: its own broker nodes, less the ones a reboot
//! operation has down. A call the cluster is short of brokers for is refused with
//! HTTP 400 `BadRequestException` carrying Apache Kafka's shortfall sentence, with
//! the factor and the available count taken from state, and changes nothing; the
//! same call is accepted once the reboot has given the node back.
//!
//! The rows differ along what the rule generalises over: how many brokers the
//! cluster holds (two, three and four, over two and three Availability Zones),
//! how the topic's replication factor stands against the count left available
//! (equal to the whole inventory, one below it, and far below it), and which of
//! the two placing operations the call is.
//!
//! Every call that must meet the shortfall is driven while the cluster REPORTS the
//! reboot in flight, and an attempt whose call was judged after that window closed
//! is driven again on a fresh name rather than asserted on: the suite waits for
//! the CONDITION it needs and never for a span, so nothing here depends on how
//! fast the host runs.
//!
//! The shortfall sentence is written out as Apache Kafka words it rather than
//! built from the service's own helper, so this suite pins the wording instead of
//! agreeing with whatever the code says.

use std::time::{Duration, Instant};

use serde_json::json;

use crate::kafka_harness::{pct, poll_until_active, ClusterRequest, Emulator, Reply};

/// How long a loop here waits for a value the clock owes a resource before the
/// suite calls it a failure: far longer than any span this service owes a
/// transition, so the deadline says nothing about how fast the host runs.
const SETTLED_DEADLINE: Duration = Duration::from_secs(60);

/// The cadence an unsettled record is read again at — a probing interval, not a
/// bound on the transition.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How many times a case is driven again when the reboot it needs finished before
/// the call under test was judged. The window is the service's own span and no
/// attempt depends on its length; this only bounds the retrying.
const WINDOW_ATTEMPTS: usize = 6;

/// The `ClusterState` a cluster whose broker is restarting reports.
const REBOOTING: &str = "REBOOTING_BROKER";

/// The member a refusal of this call names as the parameter at fault, as the wire
/// spells it on the `UpdateTopic` request (`PartitionCount`'s `jsonName`).
const PARTITION_COUNT_PARAMETER: &str = "partitionCount";

/// The member `CreateTopic`'s own refusal names, spelled the same way.
const REPLICATION_FACTOR_PARAMETER: &str = "replicationFactor";

/// Apache Kafka's sentence for a replica assignment it cannot place: fixed text
/// around the factor asked for and the brokers that were available for it.
fn shortfall_sentence(replication_factor: i64, available_brokers: i64) -> String {
    format!(
        "Replication factor: {replication_factor} larger than available brokers: \
         {available_brokers}."
    )
}

/// `CreateTopic`: `POST /v1/clusters/{ClusterArn}/topics`.
async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    partitions: i64,
    replication_factor: i64,
) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(json!({
                "topicName": topic,
                "partitionCount": partitions,
                "replicationFactor": replication_factor,
            })),
        )
        .await
}

/// `UpdateTopic`: `PUT /v1/clusters/{ClusterArn}/topics/{TopicName}`, naming the
/// new total number of partitions.
async fn grow_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    partitions: i64,
) -> Reply {
    emulator
        .call(
            "PUT",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(cluster_arn),
                pct(topic)
            ),
            Some(json!({ "partitionCount": partitions })),
        )
        .await
}

/// `DescribeTopic`: the reader that says what the service really holds.
async fn describe_topic(emulator: &Emulator, cluster_arn: &str, topic: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(cluster_arn),
                pct(topic)
            ),
            None,
        )
        .await
}

/// The `ClusterState` `DescribeCluster` reports for the cluster.
async fn cluster_state(emulator: &Emulator, cluster_arn: &str) -> String {
    emulator
        .describe(cluster_arn)
        .await
        .at("/clusterInfo/state")
        .as_str()
        .unwrap_or_default()
        .to_string()
}

/// Reboot one node of the cluster, answering whether the cluster REPORTS the reboot
/// in flight — the premise every case below is driven under. A reboot whose window
/// had already closed when the cluster was read is answered `false`, so the caller
/// drives its case again rather than asserting on a cluster with every broker up.
async fn start_reboot(emulator: &Emulator, cluster_arn: &str, case: &str) -> bool {
    let accepted = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/reboot-broker", pct(cluster_arn)),
            Some(json!({ "brokerIds": ["1"] })),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case {case}: expected RebootBroker to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    still_rebooting(emulator, cluster_arn).await
}

/// Whether the reboot that was started is STILL in flight — what says the call just
/// made was judged inside the window, so an attempt that fell outside it can be
/// driven again instead of asserted on.
async fn still_rebooting(emulator: &Emulator, cluster_arn: &str) -> bool {
    cluster_state(emulator, cluster_arn).await == REBOOTING
}

/// Poll `DescribeTopic` until the topic reports `ACTIVE`, and hand that read back.
///
/// Waits for the CONDITION rather than for a span: a topic leaves `CREATING` (and
/// the `UPDATING` an accepted change parks it in) on a clock, so the loop looks
/// again until the record reports it or [`SETTLED_DEADLINE`] passes.
async fn poll_until_topic_active(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    case: &str,
) -> Reply {
    let deadline = Instant::now() + SETTLED_DEADLINE;
    loop {
        let described = describe_topic(emulator, cluster_arn, topic).await;
        assert_eq!(
            described.status, 200,
            "case {case}: expected DescribeTopic to answer, got {} {}",
            described.status, described.body
        );
        if described.at("/status").as_str() == Some("ACTIVE") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the topic never reported ACTIVE, last read {}",
            described.body
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

/// An `ACTIVE` topic of one partition at `replication_factor`, created while every
/// broker of the cluster is up.
async fn active_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    topic: &str,
    replication_factor: i64,
    case: &str,
) {
    poll_until_active(emulator, cluster_arn, case).await;
    let created = create_topic(emulator, cluster_arn, topic, 1, replication_factor).await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected CreateTopic to be accepted while every broker is up, got {} {}",
        created.status, created.body
    );
    poll_until_topic_active(emulator, cluster_arn, topic, case).await;
}

/// One row of the table: a topic of a cluster of `brokers` nodes, created with
/// `replication_factor`, whose growth is driven while ONE of those nodes is
/// rebooting.
#[derive(Debug)]
struct Row {
    case: &'static str,
    /// How many broker nodes the cluster holds, and the client subnets they are
    /// spread over — a count has to be a multiple of the Availability Zones those
    /// subnets span, which is why the rows carry both.
    brokers: i64,
    subnets: &'static [&'static str],
    topic: &'static str,
    replication_factor: i64,
    /// What the growth asks for: the rows differ in how far the count moves, since
    /// the rule is about the placement rather than about the size of the change.
    wanted_partitions: i64,
}

impl Row {
    /// The brokers left to place a replica on while one node of this cluster is
    /// rebooting.
    fn available(&self) -> i64 {
        self.brokers - 1
    }

    /// Whether the topic's replicas can be placed on them.
    fn placeable(&self) -> bool {
        self.replication_factor <= self.available()
    }

    /// The cluster the row's topic lives on — one per broker layout, so a layout is
    /// provisioned once however many rows read it.
    fn cluster_name(&self) -> String {
        format!("avail-{}-{}", self.brokers, self.subnets.len())
    }
}

/// The table. The replication factors span the three ways a factor can stand
/// against the count a reboot leaves available: the whole inventory (so the
/// shortfall is one broker), one below it (which exactly fits), and far below it.
/// The rows are ordered so consecutive ones address different clusters, which is
/// what lets each reboot wind down while another layout is driven.
fn rows() -> Vec<Row> {
    let three: &'static [&'static str] = &["subnet-a", "subnet-b", "subnet-c"];
    let two: &'static [&'static str] = &["subnet-a", "subnet-b"];
    vec![
        Row {
            case: "three brokers, a factor that needs every one of them",
            brokers: 3,
            subnets: three,
            topic: "orders",
            replication_factor: 3,
            wanted_partitions: 2,
        },
        Row {
            case: "two brokers, a factor that needs both",
            brokers: 2,
            subnets: two,
            topic: "payments",
            replication_factor: 2,
            wanted_partitions: 4,
        },
        Row {
            case: "four brokers over two zones, a factor that needs every one",
            brokers: 4,
            subnets: two,
            topic: "shipments",
            replication_factor: 4,
            wanted_partitions: 8,
        },
        Row {
            case: "three brokers, a factor the remaining two place",
            brokers: 3,
            subnets: three,
            topic: "orders.v2",
            replication_factor: 2,
            wanted_partitions: 12,
        },
        Row {
            case: "two brokers, a factor of one",
            brokers: 2,
            subnets: two,
            topic: "payments.dlq",
            replication_factor: 1,
            wanted_partitions: 2,
        },
        Row {
            case: "four brokers over two zones, a factor the remaining three place",
            brokers: 4,
            subnets: two,
            topic: "shipments.eu",
            replication_factor: 3,
            wanted_partitions: 5,
        },
        Row {
            case: "three brokers, a factor of one",
            brokers: 3,
            subnets: three,
            topic: "orders_audit-2025",
            replication_factor: 1,
            wanted_partitions: 3,
        },
    ]
}

/// The ARN of the cluster `name` names among the ones a suite created.
fn cluster_arn<'a>(clusters: &'a [(String, String)], name: &str) -> &'a str {
    clusters
        .iter()
        .find(|(held, _)| held == name)
        .map(|(_, arn)| arn.as_str())
        .unwrap_or_else(|| panic!("the suite created no cluster named {name:?}"))
}

/// RULE: while a reboot operation has one broker of the cluster down, a growth of
/// a topic whose replication factor needs more brokers than are left is refused
/// with Apache Kafka's shortfall sentence over the two counts state holds, and
/// leaves the topic exactly as it was; a growth whose factor the remaining brokers
/// can place is accepted and applied.
#[tokio::test]
async fn a_growth_is_placed_only_on_the_brokers_the_cluster_has_available() {
    let emulator = Emulator::new();
    let rows = rows();

    // Every cluster the rows need, created first and then waited into ACTIVE: the
    // creates are all in flight together, so the suite waits one activation span
    // rather than one per layout.
    let mut clusters: Vec<(String, String)> = Vec::new();
    for row in &rows {
        let name = row.cluster_name();
        if clusters.iter().any(|(held, _)| held == &name) {
            continue;
        }
        let arn = emulator
            .create_ok(&ClusterRequest {
                brokers: row.brokers,
                subnets: row.subnets.to_vec(),
                ..ClusterRequest::named(&name)
            })
            .await;
        clusters.push((name, arn));
    }
    for (name, arn) in &clusters {
        poll_until_active(&emulator, arn, name).await;
    }

    for row in &rows {
        let arn = cluster_arn(&clusters, &row.cluster_name());
        let mut judged = false;
        for attempt in 1..=WINDOW_ATTEMPTS {
            // A fresh topic per attempt, so an attempt whose window closed can be
            // driven again against a topic nothing has changed.
            let topic = format!("{}-{attempt}", row.topic);
            active_topic(&emulator, arn, &topic, row.replication_factor, row.case).await;
            if !start_reboot(&emulator, arn, row.case).await {
                continue;
            }
            let reply = grow_topic(&emulator, arn, &topic, row.wanted_partitions).await;
            // Whether the node was STILL down after the call: an ACCEPTANCE only says
            // the bound admits the factor if a broker was down throughout, and a
            // refusal that did not arrive may mean the window closed before the call
            // was judged. Either way the attempt is driven again rather than
            // asserted on.
            let window_held = still_rebooting(&emulator, arn).await;
            let expected_status = if row.placeable() { 200 } else { 400 };
            if !window_held && (row.placeable() || reply.status != expected_status) {
                continue;
            }
            judged = true;
            if row.placeable() {
                assert_eq!(
                    reply.status, 200,
                    "case {}: a factor of {} is placeable on the {} brokers left, so the growth \
                     must be accepted, got {} {}",
                    row.case,
                    row.replication_factor,
                    row.available(),
                    reply.status,
                    reply.body
                );
                assert_eq!(
                    reply.at("/status").as_str(),
                    Some("UPDATING"),
                    "case {}: an accepted growth reports UPDATING, got {}",
                    row.case,
                    reply.body
                );
                let described = describe_topic(&emulator, arn, &topic).await;
                assert_eq!(
                    described.at("/partitionCount").as_i64(),
                    Some(row.wanted_partitions),
                    "case {}: expected the accepted growth to hold {} partitions, got {}",
                    row.case,
                    row.wanted_partitions,
                    described.body
                );
                break;
            }
            let expected = shortfall_sentence(row.replication_factor, row.available());
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (400, "BadRequestException", expected.as_str()),
                "case {}: expected the shortfall refusal {expected:?}, got {} {}",
                row.case,
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(PARTITION_COUNT_PARAMETER),
                "case {}: expected {PARTITION_COUNT_PARAMETER} named at fault, got {}",
                row.case,
                reply.body
            );
            // A refused growth changes nothing: the topic is the ACTIVE topic of
            // one partition it was before the call.
            let described = describe_topic(&emulator, arn, &topic).await;
            assert_eq!(
                (
                    described.at("/partitionCount").as_i64(),
                    described.at("/status").as_str(),
                ),
                (Some(1), Some("ACTIVE")),
                "case {}: a refused growth must leave the topic exactly as it was, got {}",
                row.case,
                described.body
            );
            break;
        }
        assert!(
            judged,
            "case {}: no attempt's growth was judged while the reboot was in flight",
            row.case
        );
    }
}

/// RULE: the shortfall is TRANSIENT — once the reboot has given the node back the
/// cluster places the same replicas again, so the very call it refused is accepted
/// and every reader reports the grown topic.
#[tokio::test]
async fn the_same_growth_is_accepted_once_the_reboot_gives_the_broker_back() {
    let case = "an RF-3 topic grown across a broker reboot";
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("avail-window"))
        .await;

    for attempt in 1..=WINDOW_ATTEMPTS {
        let topic = format!("orders-{attempt}");
        active_topic(&emulator, &cluster, &topic, 3, case).await;
        if !start_reboot(&emulator, &cluster, case).await {
            continue;
        }
        let refused = grow_topic(&emulator, &cluster, &topic, 2).await;
        if refused.status != 400 && !still_rebooting(&emulator, &cluster).await {
            continue;
        }
        let expected = shortfall_sentence(3, 2);
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.error_message()
            ),
            (400, "BadRequestException", expected.as_str()),
            "case {case}: expected the shortfall refusal {expected:?}, got {} {}",
            refused.status,
            refused.body
        );

        // The node is back once the reboot operation has finished, which is what
        // the cluster reporting ACTIVE again says.
        poll_until_active(&emulator, &cluster, case).await;
        let accepted = grow_topic(&emulator, &cluster, &topic, 2).await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected the same growth to be accepted once the brokers are back, got \
             {} {}",
            accepted.status, accepted.body
        );
        let settled = poll_until_topic_active(&emulator, &cluster, &topic, case).await;
        assert_eq!(
            (
                settled.at("/partitionCount").as_i64(),
                settled.at("/replicationFactor").as_i64(),
            ),
            (Some(2), Some(3)),
            "case {case}: expected the grown topic to report two partitions at its own factor, \
             got {}",
            settled.body
        );
        return;
    }
    panic!("case {case}: no attempt's growth was judged while the reboot was in flight");
}

/// RULE, for the other placing operation: a `CreateTopic` names the factor itself,
/// and it is bounded by the same available count — refused with the same sentence
/// while a node is down, registering nothing, and accepted once it is back.
#[tokio::test]
async fn a_create_is_bounded_by_the_same_available_brokers() {
    let case = "topics created while a broker is rebooting";
    let emulator = Emulator::new();
    let cluster = emulator
        .create_ok(&ClusterRequest::named("avail-create"))
        .await;

    // The factors a cluster of three brokers with one of them down places (one and
    // two) and the ones it cannot (three, and one past the inventory itself).
    for factor in [3, 4, 1, 2] {
        let placeable = factor <= 2;
        let mut judged = false;
        for attempt in 1..=WINDOW_ATTEMPTS {
            poll_until_active(&emulator, &cluster, case).await;
            if !start_reboot(&emulator, &cluster, case).await {
                continue;
            }
            let topic = format!("rf-{factor}-{attempt}");
            let reply = create_topic(&emulator, &cluster, &topic, 1, factor).await;
            // The same premise the growth rows are judged under: an acceptance
            // counts only while the node was still down.
            let window_held = still_rebooting(&emulator, &cluster).await;
            let expected_status = if placeable { 200 } else { 400 };
            if !window_held && (placeable || reply.status != expected_status) {
                continue;
            }
            judged = true;
            if placeable {
                assert_eq!(
                    reply.status, 200,
                    "case {case}, replicationFactor {factor}: the two brokers left place this \
                     factor, so the create must be accepted, got {} {}",
                    reply.status, reply.body
                );
                break;
            }
            let expected = shortfall_sentence(factor, 2);
            assert_eq!(
                (reply.status, reply.error_code(), reply.error_message()),
                (400, "BadRequestException", expected.as_str()),
                "case {case}, replicationFactor {factor}: expected {expected:?}, got {} {}",
                reply.status,
                reply.body
            );
            assert_eq!(
                reply.at("/invalidParameter").as_str(),
                Some(REPLICATION_FACTOR_PARAMETER),
                "case {case}, replicationFactor {factor}: expected \
                 {REPLICATION_FACTOR_PARAMETER} named at fault, got {}",
                reply.body
            );
            // Nothing was registered under that name.
            let described = describe_topic(&emulator, &cluster, &topic).await;
            assert_eq!(
                described.status, 404,
                "case {case}, replicationFactor {factor}: a refused create registers no topic, \
                 got {} {}",
                described.status, described.body
            );
            break;
        }
        assert!(
            judged,
            "case {case}, replicationFactor {factor}: no attempt's create was judged while the \
             reboot was in flight"
        );
    }

    // And the factor that needed every broker is admitted once the node is back.
    poll_until_active(&emulator, &cluster, case).await;
    let accepted = create_topic(&emulator, &cluster, "rf-3-after", 1, 3).await;
    assert_eq!(
        accepted.status, 200,
        "case {case}: expected the RF-3 create to be accepted once the brokers are back, got {} \
         {}",
        accepted.status, accepted.body
    );
    let settled = poll_until_topic_active(&emulator, &cluster, "rf-3-after", case).await;
    assert_eq!(
        settled.at("/replicationFactor").as_i64(),
        Some(3),
        "case {case}: expected the created topic to carry the factor it named, got {}",
        settled.body
    );
}
