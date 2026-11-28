// SPDX-License-Identifier: CC0-1.0

//! The SENTENCES Amazon MSK answers two refusals of this service with, driven end
//! to end over the same `MultiServiceRouter` the emulator serves from (the shared
//! harness in `tests/all/kafka_harness.rs`).
//!
//! RULE under test: a refusal's `Message` is the service's own, not a sentence
//! composed from the request — a value the caller submitted travels back only
//! where MSK's wording has a place for it. The two refusals here are the two ends
//! of that rule:
//!
//! 1. `CreateTopic` for a name the cluster already holds is refused with
//!    `TopicExistsException` and a sentence that is FIXED end to end: neither the
//!    cluster the request addressed nor the name it submitted appears in it, since
//!    both are the caller's own and the error CODE is what says which condition it
//!    is.
//! 2. A replication flow's topic-selection pattern that is not a regular
//!    expression is refused with `BadRequestException` and a sentence whose one
//!    variable part IS the submitted pattern, quoted in it — so a caller sending
//!    several patterns is told which one cannot be compiled.
//!
//! The rows differ along what each rule generalises over: for the duplicate, the
//! topic name's form and length, what else the repeat request changes, which state
//! the held topic is in when the name is asked for again, and which cluster holds
//! it; for the pattern, HOW the pattern fails to be a regular expression, which
//! `TopicReplication` member carries it, and whether it reaches the service
//! through `CreateReplicator` or `UpdateReplicationInfo`.
//!
//! Both sentences are spelled out here as Amazon MSK words them rather than built
//! from the service's own constants, so this suite pins the wording instead of
//! agreeing with whatever the code says.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    MAX_TOPIC_NAME_LENGTH, REPLICATOR_ACTIVATION_DELAY, TOPIC_ACTIVATION_DELAY,
};

use crate::kafka_harness::{pct, poll_until_active, ClusterRequest, Emulator, Reply};

/// What MSK answers a `CreateTopic` for a name the cluster already holds: fixed
/// text end to end.
const TOPIC_EXISTS_MESSAGE: &str = "Topic already exists.";

/// What it answers a topic-selection pattern it cannot compile, as a template over
/// the pattern the request submitted — the one variable part of the sentence.
fn invalid_regex_message(pattern: &str) -> String {
    format!("The regex \"{pattern}\" is invalid.")
}

/// The IAM role the replicators below are created with. `ServiceExecutionRoleArn`
/// is a plain `__string` in the model, so the service stores and reports it
/// verbatim.
const ROLE_ARN: &str = "arn:aws:iam::000000000000:role/service-role/MskReplicatorRole";

/// Sleep past the emulator's own span into `ACTIVE` for a TOPIC, so a row that
/// needs the held topic settled reads it settled.
fn sleep_past_topic_activation() {
    std::thread::sleep(TOPIC_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// Sleep past the emulator's own span into `RUNNING` for a REPLICATOR, so the
/// update row drives a replicator that has settled.
fn sleep_past_replicator_activation() {
    std::thread::sleep(REPLICATOR_ACTIVATION_DELAY + Duration::from_millis(250));
}

/// `CreateTopic` on `cluster_arn`, with the members `extra` adds to the required
/// three.
async fn create_topic(
    emulator: &Emulator,
    cluster_arn: &str,
    name: &str,
    partitions: i64,
    replication: i64,
    extra: Value,
) -> Reply {
    let mut body = json!({
        "topicName": name,
        "partitionCount": partitions,
        "replicationFactor": replication,
    });
    if let Value::Object(members) = extra {
        for (member, value) in members {
            body[member] = value;
        }
    }
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topics", pct(cluster_arn)),
            Some(body),
        )
        .await
}

/// One row of the duplicate-topic table: the name the cluster is made to hold,
/// what the repeat request changes, and whether the row waits for the held topic
/// to settle before asking for the name again.
struct Duplicate {
    case: &'static str,
    name: String,
    /// The repeat's partition count and replication factor, so a row can differ
    /// from the create in every member but the name.
    repeat: (i64, i64),
    /// Extra members the repeat carries.
    extra: Value,
    /// Whether to let the held topic reach `ACTIVE` first — a name is taken in
    /// either state, so the row's answer must not depend on it.
    settled: bool,
}

/// The rows: the name's form and length (including the longest name the model's
/// `TopicName` admits, `MAX_TOPIC_NAME_LENGTH` characters), what else the repeat
/// changes, and whether the held topic has settled.
fn duplicates() -> Vec<Duplicate> {
    vec![
        Duplicate {
            case: "the same request twice, the held topic still creating",
            name: "orders".to_string(),
            repeat: (3, 2),
            extra: Value::Null,
            settled: false,
        },
        Duplicate {
            case: "a repeat that changes every other member",
            name: "payments".to_string(),
            repeat: (1, 1),
            extra: json!({ "configs": "cmV0ZW50aW9uLm1zPTE=" }),
            settled: true,
        },
        Duplicate {
            case: "a name carrying the character set the model admits",
            name: "orders.events_v2-1".to_string(),
            repeat: (5, 3),
            extra: Value::Null,
            settled: true,
        },
        Duplicate {
            case: "a one-character name",
            name: "t".to_string(),
            repeat: (2, 2),
            extra: Value::Null,
            settled: false,
        },
        Duplicate {
            // The longest name the model's member admits, so the rule is asserted
            // at the published bound as well as inside it.
            case: "a name at the model's length bound",
            name: "n".repeat(MAX_TOPIC_NAME_LENGTH),
            repeat: (1, 2),
            extra: Value::Null,
            settled: true,
        },
    ]
}

/// RULE: a `CreateTopic` for a name its cluster already holds is refused with HTTP
/// 409 `TopicExistsException` and MSK's fixed sentence, which carries neither the
/// cluster ARN nor the topic name — whatever the name is, whatever else the repeat
/// changes, and whichever state the held topic is in — and the held topic is left
/// exactly as it was created.
#[tokio::test]
async fn a_repeated_topic_name_is_refused_in_the_service_s_fixed_sentence() {
    let emulator = Emulator::new();
    let mut request = ClusterRequest::named("duplicate-topic");
    request.brokers = 3;
    let cluster = emulator.create_ok(&request).await;
    // A second cluster, so one row can show the refusal is about the cluster the
    // request addressed rather than about the name being in use anywhere.
    let other = emulator
        .create_ok(&ClusterRequest::named("duplicate-topic-other"))
        .await;
    poll_until_active(&emulator, &cluster, "duplicate topic").await;
    poll_until_active(&emulator, &other, "duplicate topic other").await;

    for row in duplicates() {
        let case = row.case;
        let created = create_topic(&emulator, &cluster, &row.name, 3, 2, Value::Null).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected the first CreateTopic to be accepted, got {} {}",
            created.status, created.body
        );
        if row.settled {
            sleep_past_topic_activation();
        }

        let (partitions, replication) = row.repeat;
        let refused = create_topic(
            &emulator,
            &cluster,
            &row.name,
            partitions,
            replication,
            row.extra.clone(),
        )
        .await;
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.error_message(),
                refused.at("/invalidParameter").as_str(),
            ),
            (409, "TopicExistsException", TOPIC_EXISTS_MESSAGE, Some("topicName")),
            "case {case}: expected MSK's fixed duplicate-topic refusal, got {} {}",
            refused.status,
            refused.body
        );
        // The two per-run values a composed sentence would have leaked. The name
        // cannot appear either — the sentence asserted above is a CONSTANT, so no
        // part of any request reaches it — and the cluster ARN is checked
        // explicitly because it is the value the composed sentence carried.
        assert!(
            !refused.error_message().contains(cluster.as_str()),
            "case {case}: the refusal must not carry the cluster ARN, got {:?}",
            refused.error_message()
        );

        // The refused create changed nothing: the held topic still reports the
        // configuration it was created with.
        let described = emulator
            .call(
                "GET",
                &format!(
                    "/v1/clusters/{}/topics/{}",
                    pct(&cluster),
                    pct(&row.name)
                ),
                None,
            )
            .await;
        assert_eq!(
            (
                described.status,
                described.at("/partitionCount").as_i64(),
                described.at("/replicationFactor").as_i64(),
                described.at("/configs").clone(),
            ),
            (200, Some(3), Some(2), Value::Null),
            "case {case}: a refused create must leave the held topic alone, got {}",
            described.body
        );

        // And the SAME name on the other cluster is a topic of its own: the name
        // identifies a topic within its cluster, which is what makes the refusal
        // above about this cluster.
        let elsewhere = create_topic(&emulator, &other, &row.name, 1, 1, Value::Null).await;
        assert_eq!(
            elsewhere.status, 200,
            "case {case}: expected the same name on another cluster to be accepted, got {} {}",
            elsewhere.status, elsewhere.body
        );
    }
}

/// One row of the invalid-pattern table: why the pattern is not a regular
/// expression, the pattern itself, and which `TopicReplication` member carries it.
struct Pattern {
    case: &'static str,
    pattern: &'static str,
    /// `true` when the row submits the pattern as a topic to EXCLUDE rather than
    /// as one to replicate — both members are "a list of regular expression
    /// patterns", so both are admitted by the one rule.
    exclude: bool,
}

/// The rows: every way a short pattern can fail to parse as a regular expression
/// — an unterminated character class, an unterminated group, a repetition with
/// nothing to repeat, a counted repetition whose bounds are inverted, a trailing
/// escape and an unterminated group name — across both pattern members.
fn patterns() -> Vec<Pattern> {
    vec![
        Pattern {
            case: "an unterminated character class",
            pattern: "[",
            exclude: false,
        },
        Pattern {
            case: "an unterminated character range",
            pattern: "orders-[a-",
            exclude: false,
        },
        Pattern {
            case: "an unterminated group",
            pattern: "(orders",
            exclude: false,
        },
        Pattern {
            case: "a repetition with nothing to repeat",
            pattern: "*",
            exclude: false,
        },
        Pattern {
            case: "a counted repetition with inverted bounds",
            pattern: "a{2,1}",
            exclude: false,
        },
        Pattern {
            case: "a trailing escape",
            pattern: "orders\\",
            exclude: false,
        },
        Pattern {
            case: "an unterminated group name in the exclusion list",
            pattern: "(?P<name",
            exclude: true,
        },
        Pattern {
            case: "an unterminated character class in the exclusion list",
            pattern: "__internal[",
            exclude: true,
        },
    ]
}

impl Pattern {
    /// The `TopicReplication` settings this row submits: the offending pattern in
    /// its own member, with the other member carrying a pattern that compiles, so
    /// the only fault in the request is this row's.
    fn topic_replication(&self) -> Value {
        if self.exclude {
            json!({ "topicsToReplicate": [".*"], "topicsToExclude": [self.pattern] })
        } else {
            json!({ "topicsToReplicate": [self.pattern, "orders-[0-9]+"], "topicsToExclude": [] })
        }
    }

    /// The `TopicReplication` member the refusal names, by its wire name.
    fn member(&self) -> &'static str {
        if self.exclude {
            "topicsToExclude"
        } else {
            "topicsToReplicate"
        }
    }
}

/// A `CreateReplicator` body replicating between `source` and `target` under
/// `topic_replication`.
fn replicator_body(name: &str, source: &str, target: &str, topic_replication: Value) -> Value {
    let clusters: Vec<Value> = [source, target]
        .iter()
        .map(|arn| {
            json!({
                "amazonMskCluster": { "mskClusterArn": arn },
                "vpcConfig": { "subnetIds": ["subnet-a", "subnet-b"] },
            })
        })
        .collect();
    json!({
        "replicatorName": name,
        "serviceExecutionRoleArn": ROLE_ARN,
        "kafkaClusters": clusters,
        "replicationInfoList": [{
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "targetCompressionType": "NONE",
            "consumerGroupReplication": { "consumerGroupsToReplicate": [".*"] },
            "topicReplication": topic_replication,
        }],
    })
}

/// RULE: a topic-selection pattern that is not a regular expression is refused
/// with HTTP 400 `BadRequestException` and MSK's sentence with that pattern quoted
/// in it — by `CreateReplicator` and by `UpdateReplicationInfo`, for either
/// pattern member, and whichever way the pattern fails to parse — while a list of
/// patterns that do compile is admitted.
#[tokio::test]
async fn an_invalid_topic_pattern_is_refused_with_the_pattern_quoted_in_the_sentence() {
    let emulator = Emulator::new();
    let source = emulator
        .create_ok(&ClusterRequest::named("pattern-source"))
        .await;
    let target = emulator
        .create_ok(&ClusterRequest::named("pattern-target"))
        .await;

    for (index, row) in patterns().iter().enumerate() {
        let case = row.case;
        let refused = emulator
            .call(
                "POST",
                "/replication/v1/replicators",
                Some(replicator_body(
                    &format!("pattern-{index}"),
                    &source,
                    &target,
                    row.topic_replication(),
                )),
            )
            .await;
        let expected = invalid_regex_message(row.pattern);
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.error_message(),
                refused.at("/invalidParameter").as_str(),
            ),
            (
                400,
                "BadRequestException",
                expected.as_str(),
                Some(row.member()),
            ),
            "case CreateReplicator/{case}: expected {expected:?}, got {} {}",
            refused.status,
            refused.body
        );
    }

    // The same rule admits a list whose patterns compile, which is what the rows
    // above are a contrast to — and gives this suite a replicator to update.
    let created = emulator
        .call(
            "POST",
            "/replication/v1/replicators",
            Some(replicator_body(
                "pattern-usable",
                &source,
                &target,
                json!({ "topicsToReplicate": ["orders-[0-9]+", ".*\\.events"] }),
            )),
        )
        .await;
    assert_eq!(
        created.status, 200,
        "expected a list of patterns that compile to be admitted, got {} {}",
        created.status, created.body
    );
    let replicator = created
        .at("/replicatorArn")
        .as_str()
        .expect("CreateReplicator reported no replicatorArn")
        .to_string();
    sleep_past_replicator_activation();
    let described = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        described.at("/replicatorState").as_str(),
        Some("RUNNING"),
        "expected the replicator to settle RUNNING, got {}",
        described.body
    );
    let version = described
        .at("/currentVersion")
        .as_str()
        .expect("DescribeReplicator reported no currentVersion")
        .to_string();

    for row in patterns() {
        let case = row.case;
        let mut update = json!({
            "currentVersion": version,
            "sourceKafkaClusterArn": source,
            "targetKafkaClusterArn": target,
            "topicReplication": row.topic_replication(),
        });
        // `TopicReplicationUpdate` requires the three switches the shape models,
        // so the only fault in this request is the row's pattern.
        for (switch, value) in [
            ("detectAndCopyNewTopics", true),
            ("copyTopicConfigurations", true),
            ("copyAccessControlListsForTopics", false),
        ] {
            update["topicReplication"][switch] = json!(value);
        }
        let refused = emulator
            .call(
                "PUT",
                &format!(
                    "/replication/v1/replicators/{}/replication-info",
                    pct(&replicator)
                ),
                Some(update),
            )
            .await;
        let expected = invalid_regex_message(row.pattern);
        assert_eq!(
            (
                refused.status,
                refused.error_code(),
                refused.error_message(),
                refused.at("/invalidParameter").as_str(),
            ),
            (
                400,
                "BadRequestException",
                expected.as_str(),
                Some(row.member()),
            ),
            "case UpdateReplicationInfo/{case}: expected {expected:?}, got {} {}",
            refused.status,
            refused.body
        );
    }

    // None of the refused updates changed the stored flow: it still replicates by
    // the patterns the accepted create carried.
    let after = emulator
        .call(
            "GET",
            &format!("/replication/v1/replicators/{}", pct(&replicator)),
            None,
        )
        .await;
    assert_eq!(
        after.at("/replicationInfoList/0/topicReplication/topicsToReplicate"),
        &json!(["orders-[0-9]+", ".*\\.events"]),
        "a refused update must leave the stored pattern list alone, got {}",
        after.body
    );
}
