// SPDX-License-Identifier: CC0-1.0

//! `UpdateTopic`, driven end to end over the same `MultiServiceRouter` the
//! emulator serves from, through the shared harness in
//! `tests/all/kafka_harness.rs`.
//!
//! RULE under test: a change to a topic of a provisioned cluster is applied to
//! the record every later reader answers from — the partition count the request
//! names and the topic-level `Configs` it submits — and it is accepted only for a
//! topic that cluster HOLDS and that is in a state a change is accepted from, with
//! a count that GROWS: Apache Kafka adds partitions to a topic and never takes
//! them away, so a count below the topic's own, or the one it already has, is
//! refused and changes nothing. An accepted change makes the topic `UPDATING`
//! until the clock owes it `ACTIVE` again, and the new partitions are the topic's
//! from the instant the call is accepted. Two concurrent increases of one topic
//! admit exactly one winner.
//!
//! The rows differ along what the rule generalises over: which members the request
//! carries (count alone, configs alone, both), the size and direction of the
//! count change, whether the topic already carried a configuration, the topic
//! name's form, and the state the topic is in when the call arrives.

use std::time::Duration;

use serde_json::{json, Value};
// The service's own spans, wordings and accepted-state sets, so the suite and the
// handlers cannot drift apart on what the service does or says.
use service_com_amazonaws_kafka_kafka::state::state_gate::{
    not_in_accepted_state_message, TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM,
};
use service_com_amazonaws_kafka_kafka::state::{
    BAD_REQUEST_MESSAGE, CLUSTER_ARN_MEMBER, RESOURCE_NOT_FOUND_MESSAGE,
    SERVERLESS_TOPIC_APIS_CODE, SERVERLESS_TOPIC_APIS_MESSAGE, TOPIC_ACTIVATION_DELAY,
    TOPIC_NAME_MEMBER, TOPIC_STATE_ACTIVE, TOPIC_STATE_UPDATING,
};

use crate::kafka_harness::{
    pct, sleep_past_activation, ClusterRequest, Emulator, Reply, ACCOUNT, REGION,
};

/// The cluster every row's topics live on: a provisioned cluster with enough
/// brokers to place the replicas of a topic created with a replication factor of
/// two, settled before any topic call is made.
async fn active_cluster(emulator: &Emulator, name: &str) -> String {
    let mut request = ClusterRequest::named(name);
    request.brokers = 3;
    let arn = emulator.create_ok(&request).await;
    sleep_past_activation();
    arn
}

/// `CreateTopic`: `POST /v1/clusters/{ClusterArn}/topics`.
async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    configs: Option<&str>,
) -> Reply {
    let mut body = json!({
        "topicName": name,
        "partitionCount": partitions,
        "replicationFactor": 2,
    });
    if let Some(configs) = configs {
        body["configs"] = json!(configs);
    }
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(body),
        )
        .await
}

/// A topic of `cluster_arn`, created and waited past its `CREATING` span, so the
/// calls below meet the `ACTIVE` topic the rule is about.
async fn active_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    configs: Option<&str>,
) {
    let reply = create_topic(emulator, cluster_arn, name, partitions, configs).await;
    assert_eq!(
        reply.status, 200,
        "case {name:?}: expected CreateTopic to return 200, got {} {}",
        reply.status, reply.body
    );
    sleep_past_topic_activation();
}

/// `UpdateTopic`, which the model binds to
/// `PUT /v1/clusters/{ClusterArn}/topics/{TopicName}`, as an endpoint for `region`
/// serves it.
async fn update_in(
    emulator: &Emulator,
    region: &str,
    cluster_arn: &str,
    topic: &str,
    body: Value,
) -> Reply {
    emulator
        .call_in(
            region,
            "PUT",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(cluster_arn),
                pct(topic)
            ),
            Some(body),
        )
        .await
}

/// The same call, signed for the region the clusters below live in.
async fn update(emulator: &Emulator, cluster_arn: &str, topic: &str, body: Value) -> Reply {
    update_in(emulator, REGION, cluster_arn, topic, body).await
}

/// `DescribeTopic`: the reader that proves what the service persisted.
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

/// `DescribeTopicPartitions`, page size at the model's maximum so one page
/// carries every partition the rows below produce.
async fn describe_partitions(emulator: &Emulator, cluster_arn: &str, topic: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topics/{}/partitions?maxResults=100",
                pct(cluster_arn),
                pct(topic)
            ),
            None,
        )
        .await
}

/// The partition ids `DescribeTopicPartitions` reports for `topic`, in the order
/// it reports them.
async fn reported_partition_ids(
    emulator: &Emulator,
    case: &str,
    cluster_arn: &str,
    topic: &str,
) -> Vec<i64> {
    let reply = describe_partitions(emulator, cluster_arn, topic).await;
    assert_eq!(
        reply.status, 200,
        "case {case}: expected DescribeTopicPartitions to return 200, got {} {}",
        reply.status, reply.body
    );
    reply
        .at("/partitions")
        .as_array()
        .map(|partitions| {
            partitions
                .iter()
                .map(|partition| partition["partition"].as_i64().unwrap_or(-1))
                .collect()
        })
        .unwrap_or_default()
}

/// Sleep past the emulator's own span for a topic's transition to `ACTIVE` —
/// after its creation, and after a change accepted on it.
/// `std::thread::sleep` because the workspace's tokio carries no timer feature;
/// nothing else has to make progress here.
fn sleep_past_topic_activation() {
    std::thread::sleep(TOPIC_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Assert `reply` is the refusal a change to a topic that is not in a state the
/// change is accepted from answers: HTTP 400 `BadRequestException`, naming the
/// topic's own member as the parameter at fault, carrying the one sentence that
/// names the set of states a change IS accepted from — built from the service's
/// own set, so the suite cannot assert a set the handler does not gate on.
fn assert_topic_state_refusal(reply: &Reply, case: &str) {
    assert_eq!(
        (reply.status, reply.error_code()),
        (400, "BadRequestException"),
        "case {case}: expected 400 BadRequestException, got {} {}",
        reply.status,
        reply.body
    );
    let expected = not_in_accepted_state_message(&TOPIC_STATES_A_CHANGE_IS_ACCEPTED_FROM);
    assert_eq!(
        reply.error_message(),
        expected,
        "case {case}: expected the state-machine wording {expected:?}, got {:?}",
        reply.error_message()
    );
    assert_eq!(
        reply.at("/invalidParameter").as_str(),
        Some(TOPIC_NAME_MEMBER),
        "case {case}: the refusal must name {TOPIC_NAME_MEMBER} at fault, got {}",
        reply.body
    );
}

/// One row of the `UpdateTopic` table.
#[derive(Debug)]
struct Case {
    case: &'static str,
    topic: &'static str,
    /// The partition count and `Configs` the topic is CREATED with — what the
    /// change is applied to.
    created_partitions: i64,
    created_configs: Option<&'static str>,
    /// The members the update submits; `None` means the request omits it.
    partitions: Option<i64>,
    configs: Option<&'static str>,
    /// What every reader must report once the change has been applied.
    expected_partitions: i64,
    expected_configs: Option<&'static str>,
}

/// The table: every row changes something the rule covers, and the rows differ
/// along which members the request carries, how far the count grows, and whether
/// the topic already carried a configuration. The `Configs` values are the Base64
/// strings the model types that member as (`UpdateTopicRequest$Configs`: "The new
/// topic configurations encoded as a Base64 string").
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "one more partition",
            topic: "orders",
            created_partitions: 1,
            created_configs: None,
            partitions: Some(2),
            configs: None,
            expected_partitions: 2,
            expected_configs: None,
        },
        Case {
            case: "many more partitions",
            topic: "orders.v2",
            created_partitions: 2,
            created_configs: None,
            partitions: Some(12),
            configs: None,
            expected_partitions: 12,
            expected_configs: None,
        },
        Case {
            case: "configs alone leaves the count",
            topic: "orders_v2-2025",
            created_partitions: 3,
            created_configs: None,
            partitions: None,
            configs: Some("cmV0ZW50aW9uLm1zPTYwMDAwMA=="),
            expected_partitions: 3,
            expected_configs: Some("cmV0ZW50aW9uLm1zPTYwMDAwMA=="),
        },
        Case {
            case: "configs replacing the ones the topic was created with",
            topic: "__audit.trail",
            created_partitions: 4,
            created_configs: Some("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
            partitions: None,
            configs: Some("Y2xlYW51cC5wb2xpY3k9Y29tcGFjdA=="),
            expected_partitions: 4,
            expected_configs: Some("Y2xlYW51cC5wb2xpY3k9Y29tcGFjdA=="),
        },
        Case {
            case: "both members at once",
            topic: "payments",
            created_partitions: 1,
            created_configs: Some("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
            partitions: Some(32),
            configs: Some("bWluLmluc3luYy5yZXBsaWNhcz0y"),
            expected_partitions: 32,
            expected_configs: Some("bWluLmluc3luYy5yZXBsaWNhcz0y"),
        },
        Case {
            case: "a count change leaves the stored configs alone",
            topic: "shipments-2025.eu",
            created_partitions: 6,
            created_configs: Some("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
            partitions: Some(7),
            configs: None,
            expected_partitions: 7,
            expected_configs: Some("cmV0ZW50aW9uLm1zPTg2NDAwMDAw"),
        },
    ]
}

impl Case {
    /// The update body: only the members this row submits.
    fn body(&self) -> Value {
        let mut body = json!({});
        if let Some(partitions) = self.partitions {
            body["partitionCount"] = json!(partitions);
        }
        if let Some(configs) = self.configs {
            body["configs"] = json!(configs);
        }
        body
    }
}

/// RULE: an accepted `UpdateTopic` writes what the request named into the one
/// topic record every reader answers from — the partition count at once, with the
/// partitions `DescribeTopicPartitions` reports growing to match, and the
/// `Configs` string verbatim — reports the topic `UPDATING`, and leaves it
/// `ACTIVE` again once the clock owes it that transition, with the change still
/// in place.
#[tokio::test]
async fn an_accepted_update_changes_the_topic_every_reader_reports() {
    let emulator = Emulator::new();
    let cluster = active_cluster(&emulator, "topic-updates").await;

    for case in cases() {
        active_topic(
            &emulator,
            &cluster,
            case.topic,
            case.created_partitions,
            case.created_configs,
        )
        .await;

        let reply = update(&emulator, &cluster, case.topic, case.body()).await;
        assert_eq!(
            reply.status, 200,
            "case {}: expected UpdateTopic to return 200, got {} {}",
            case.case, reply.status, reply.body
        );
        assert_eq!(
            reply.at("/topicName").as_str(),
            Some(case.topic),
            "case {}: expected the stored topic name back, got {}",
            case.case,
            reply.body
        );
        assert_eq!(
            reply.at("/status").as_str(),
            Some(TOPIC_STATE_UPDATING),
            "case {}: expected the answer to report {TOPIC_STATE_UPDATING}, got {}",
            case.case,
            reply.body
        );
        let expected_arn = describe_topic(&emulator, &cluster, case.topic)
            .await
            .at("/topicArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        assert_eq!(
            reply.at("/topicArn").as_str(),
            Some(expected_arn.as_str()),
            "case {}: expected the topic's own ARN back, got {}",
            case.case,
            reply.body
        );

        // The change is the record's at once: the new partitions exist, and the
        // topic reports the state the change left it in.
        let described = describe_topic(&emulator, &cluster, case.topic).await;
        assert_eq!(
            (
                described.at("/partitionCount").as_i64(),
                described.at("/configs").as_str(),
                described.at("/status").as_str(),
            ),
            (
                Some(case.expected_partitions),
                case.expected_configs,
                Some(TOPIC_STATE_UPDATING),
            ),
            "case {}: expected the applied change and {TOPIC_STATE_UPDATING}, got {}",
            case.case,
            described.body
        );
        let ids = reported_partition_ids(&emulator, case.case, &cluster, case.topic).await;
        assert_eq!(
            ids,
            (0..case.expected_partitions).collect::<Vec<i64>>(),
            "case {}: expected the partitions 0..{}, got {ids:?}",
            case.case,
            case.expected_partitions
        );
        // The replication factor is the topic's own and no update touches it: the
        // request has no member for it.
        assert_eq!(
            described.at("/replicationFactor").as_i64(),
            Some(2),
            "case {}: an update must not change the replication factor, got {}",
            case.case,
            described.body
        );

        // Once the clock owes the topic its transition it is ACTIVE again, with
        // the change still in place.
        sleep_past_topic_activation();
        let settled = describe_topic(&emulator, &cluster, case.topic).await;
        assert_eq!(
            (
                settled.at("/status").as_str(),
                settled.at("/partitionCount").as_i64(),
                settled.at("/configs").as_str(),
            ),
            (
                Some(TOPIC_STATE_ACTIVE),
                Some(case.expected_partitions),
                case.expected_configs,
            ),
            "case {}: expected a settled topic carrying the change, got {}",
            case.case,
            settled.body
        );
    }
}

/// RULE: a partition count that does not GROW the topic is refused with a
/// `BadRequestException` naming the count at fault, and so is a request that names
/// no change at all; neither changes the topic. The row values span the directions
/// a count can fail in — one below, far below, exactly the stored count, and the
/// values the model's plain `__integer` lets through where `CreateTopic`'s
/// `__integerMin1` would not (zero and a negative).
#[tokio::test]
async fn a_count_that_does_not_grow_the_topic_is_refused() {
    let emulator = Emulator::new();
    let cluster = active_cluster(&emulator, "topic-update-counts").await;
    let topic = "orders";
    let held = 8;
    active_topic(&emulator, &cluster, topic, held, None).await;

    for wanted in [7, 1, 0, -4] {
        let reply = update(
            &emulator,
            &cluster,
            topic,
            json!({ "partitionCount": wanted }),
        )
        .await;
        let expected = format!(
            "Topic currently has {held} partitions, which is higher than the requested {wanted}."
        );
        assert_eq!(
            (reply.status, reply.error_code(), reply.error_message()),
            (400, "BadRequestException", expected.as_str()),
            "case partitionCount={wanted}: expected the shrink refusal {expected:?}, got {} {}",
            reply.status,
            reply.body
        );
        assert_eq!(
            reply.at("/invalidParameter").as_str(),
            Some("partitionCount"),
            "case partitionCount={wanted}: expected partitionCount named at fault, got {}",
            reply.body
        );
    }

    // The count the topic already has is no change either, and the service says
    // so in its own words for that case.
    let same = update(&emulator, &cluster, topic, json!({ "partitionCount": held })).await;
    let expected = format!("Topic already has {held} partitions.");
    assert_eq!(
        (same.status, same.error_code(), same.error_message()),
        (400, "BadRequestException", expected.as_str()),
        "case partitionCount=held: expected {expected:?}, got {} {}",
        same.status,
        same.body
    );

    // A request naming neither member asks for nothing.
    let empty = update(&emulator, &cluster, topic, json!({})).await;
    assert_eq!(
        (empty.status, empty.error_code()),
        (400, "BadRequestException"),
        "case no member: expected a 400 BadRequestException, got {} {}",
        empty.status,
        empty.body
    );
    assert_eq!(
        empty.at("/invalidParameter").as_str(),
        Some("partitionCount"),
        "case no member: expected partitionCount named at fault, got {}",
        empty.body
    );

    // None of the refusals touched the topic: it is the ACTIVE topic of `held`
    // partitions it was, and it reports exactly those partitions.
    let described = describe_topic(&emulator, &cluster, topic).await;
    assert_eq!(
        (
            described.at("/partitionCount").as_i64(),
            described.at("/status").as_str(),
            described.at("/configs").as_str(),
        ),
        (Some(held), Some(TOPIC_STATE_ACTIVE), None),
        "a refused update must leave the topic exactly as it was, got {}",
        described.body
    );
    let ids = reported_partition_ids(&emulator, "after the refusals", &cluster, topic).await;
    assert_eq!(
        ids,
        (0..held).collect::<Vec<i64>>(),
        "a refused update must add no partition, got {ids:?}"
    );
}

/// RULE: a change is accepted only for a topic the addressed cluster HOLDS and
/// that is in a state a change is accepted from — a topic whose own last change is
/// still in flight, and one whose deletion the service has accepted, are refused
/// with the state-machine refusal; a name no topic of the cluster has, a
/// well-formed cluster ARN this endpoint holds no cluster for, a value that is no
/// cluster ARN at all, a name Apache Kafka would not accept and a serverless
/// cluster each get the refusal that condition has.
#[tokio::test]
async fn a_change_is_accepted_only_for_a_held_topic_in_an_accepting_state() {
    let emulator = Emulator::new();
    let cluster = active_cluster(&emulator, "topic-update-gates").await;
    let grow = json!({ "partitionCount": 4 });

    // A topic whose own change is still in flight is UPDATING, so a second change
    // meets the state the first left behind — which is what admits exactly one of
    // two changes to one topic.
    active_topic(&emulator, &cluster, "in-flight", 1, None).await;
    let accepted = update(&emulator, &cluster, "in-flight", json!({ "partitionCount": 2 })).await;
    assert_eq!(
        accepted.status, 200,
        "case in-flight: expected the first change to be accepted, got {} {}",
        accepted.status, accepted.body
    );
    let refused = update(&emulator, &cluster, "in-flight", grow.clone()).await;
    assert_topic_state_refusal(&refused, "in-flight");

    // A topic being deleted takes no change either.
    active_topic(&emulator, &cluster, "going-away", 1, None).await;
    let deleted = emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}/topics/{}",
                pct(&cluster),
                pct("going-away")
            ),
            None,
        )
        .await;
    assert_eq!(
        deleted.status, 200,
        "case deleting: expected DeleteTopic to return 200, got {} {}",
        deleted.status, deleted.body
    );
    let refused = update(&emulator, &cluster, "going-away", grow.clone()).await;
    assert_topic_state_refusal(&refused, "deleting");

    // A name this cluster holds no topic of is the missing resource.
    let absent = update(&emulator, &cluster, "never-created", grow.clone()).await;
    assert_eq!(
        (absent.status, absent.error_code(), absent.error_message()),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case absent topic: expected the service's not-found refusal, got {} {}",
        absent.status,
        absent.body
    );
    assert_eq!(
        absent.at("/invalidParameter").as_str(),
        Some(TOPIC_NAME_MEMBER),
        "case absent topic: expected TopicName named at fault, got {}",
        absent.body
    );

    // A well-formed cluster ARN this endpoint's region holds no cluster for is the
    // missing resource too — the cluster, not the topic.
    let elsewhere = format!(
        "arn:aws:kafka:{REGION}:{ACCOUNT}:cluster/absent/11111111-2222-4333-8444-555555555555-1"
    );
    let no_cluster = update(&emulator, &elsewhere, "orders", grow.clone()).await;
    assert_eq!(
        (
            no_cluster.status,
            no_cluster.error_code(),
            no_cluster.error_message()
        ),
        (404, "NotFoundException", RESOURCE_NOT_FOUND_MESSAGE),
        "case absent cluster: expected the service's not-found refusal, got {} {}",
        no_cluster.status,
        no_cluster.body
    );
    assert_eq!(
        no_cluster.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_MEMBER),
        "case absent cluster: expected {CLUSTER_ARN_MEMBER} named at fault, got {}",
        no_cluster.body
    );

    // A value that is no cluster ARN at all is a request the service cannot parse,
    // reported in the service's own envelope for an unusable parameter.
    let malformed = update(&emulator, "not-an-arn", "orders", grow.clone()).await;
    assert_eq!(
        (
            malformed.status,
            malformed.error_code(),
            malformed.error_message()
        ),
        (400, "BadRequestException", BAD_REQUEST_MESSAGE),
        "case malformed ARN: expected the ARN refusal, got {} {}",
        malformed.status,
        malformed.body
    );
    assert_eq!(
        malformed.at("/invalidParameter").as_str(),
        Some(CLUSTER_ARN_MEMBER),
        "case malformed ARN: expected {CLUSTER_ARN_MEMBER} named at fault, got {}",
        malformed.body
    );

    // A name Apache Kafka itself would refuse is refused before anything is
    // resolved, in the service's wording for that name.
    let illegal = update(&emulator, &cluster, "bad topic", grow.clone()).await;
    assert_eq!(
        (illegal.status, illegal.error_code()),
        (400, "BadRequestException"),
        "case illegal name: expected a 400 BadRequestException, got {} {}",
        illegal.status,
        illegal.body
    );
    assert_eq!(
        illegal.at("/invalidParameter").as_str(),
        Some(TOPIC_NAME_MEMBER),
        "case illegal name: expected TopicName named at fault, got {}",
        illegal.body
    );

    // The Topic Management surface is not offered for a serverless cluster.
    let serverless = emulator
        .create_serverless_ok("topic-update-serverless", &[vec!["subnet-a", "subnet-b"]], Some(true))
        .await;
    sleep_past_activation();
    let refused = update(&emulator, &serverless, "orders", grow).await;
    assert!(
        refused.error_code().ends_with(SERVERLESS_TOPIC_APIS_CODE),
        "case serverless: expected {SERVERLESS_TOPIC_APIS_CODE}, got {:?}",
        refused.error_code()
    );
    assert_eq!(
        refused.error_message(),
        SERVERLESS_TOPIC_APIS_MESSAGE,
        "case serverless: expected the fixed serverless refusal, got {}",
        refused.body
    );
}

/// RULE: two concurrent increases of ONE topic admit exactly one winner — the
/// change is judged against the state the registry is in when it is applied, not
/// against the state both callers started from — and afterwards the topic holds
/// the count the winner asked for.
#[tokio::test]
async fn two_concurrent_increases_admit_exactly_one_winner() {
    let emulator = Emulator::new();
    let cluster = active_cluster(&emulator, "topic-update-race").await;
    let topic = "orders";
    active_topic(&emulator, &cluster, topic, 1, None).await;

    let wanted = 3;
    let body = json!({ "partitionCount": wanted });
    let (first, second) = tokio::join!(
        update(&emulator, &cluster, topic, body.clone()),
        update(&emulator, &cluster, topic, body.clone()),
    );
    let outcomes = [&first, &second];
    let accepted = outcomes
        .iter()
        .filter(|reply| reply.status == 200)
        .count();
    assert_eq!(
        accepted,
        1,
        "expected exactly one racer to be accepted, got {} and {}",
        first.body,
        second.body
    );
    for reply in outcomes {
        if reply.status == 200 {
            continue;
        }
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "expected the losing racer to be refused with a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
    }

    // The winner's change landed exactly once: the topic holds the count it asked
    // for, and reports exactly those partitions.
    let ids = reported_partition_ids(&emulator, "after the race", &cluster, topic).await;
    assert_eq!(
        ids,
        (0..wanted).collect::<Vec<i64>>(),
        "expected the topic to hold {wanted} partitions after the race, got {ids:?}"
    );
    sleep_past_topic_activation();
    let settled = describe_topic(&emulator, &cluster, topic).await;
    assert_eq!(
        (
            settled.at("/status").as_str(),
            settled.at("/partitionCount").as_i64(),
        ),
        (Some(TOPIC_STATE_ACTIVE), Some(wanted)),
        "expected a settled topic of {wanted} partitions, got {}",
        settled.body
    );
}
