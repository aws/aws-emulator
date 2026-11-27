// SPDX-License-Identifier: CC0-1.0

//! The TOPIC REASSIGNMENT family of one cluster — `StartTopicReassignment`,
//! `DescribeTopicReassignment`, `ListTopicReassignments` and
//! `StopTopicReassignment` — together with the `BrokerTypeUpgradeInfo` an
//! `UpdateBrokerType` reports, driven end to end over the same
//! `MultiServiceRouter` the emulator serves from.
//!
//! RULE 1: a reassignment is a stored operation of one cluster whose TOPICS are the
//! cluster's own, selected by the include patterns minus the exclude patterns, and
//! whose counts are those topics' — so `TotalTopics` and `TotalPartitions` are what
//! the cluster really holds under those patterns. Its status walks `PENDING` ->
//! `IN_PROGRESS` -> `COMPLETE` on the clock, its progress never exceeds its totals
//! and reaches them exactly when it completes, and an accepted stop freezes that
//! progress and walks `STOPPING` -> `STOPPED`.
//!
//! RULE 2: `BrokerTypeUpgradeInfo` is reported by the one `UpdateBrokerType`
//! operation that upgrades a cluster FROM Standard brokers TO Express brokers, and
//! by no other operation: it carries the `TopicReassignment` mode that call asked
//! for, and the ARN of the reassignment the service started for it once one has
//! started — which `AUTO` makes the service do for itself.
//!
//! The rows differ along what the rules generalise over: the patterns a
//! reassignment selects with (all topics, a prefix, a prefix minus one name, a
//! pattern that matches nothing), the topics and partition counts the cluster
//! holds, the reassignment mode the upgrade asks for, and whether the broker-type
//! change crosses broker categories at all.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared harness.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::unsupported_instance_type_message;

use crate::kafka_harness::{pct, poll_until_active, ClusterRequest, Emulator, Reply};

/// How long a suite waits for a status the clock owes a reassignment before it
/// calls the emulator broken: far longer than the spans the service owes those
/// transitions, so the deadline says nothing about how fast the host runs.
const REASSIGNMENT_DEADLINE: Duration = Duration::from_secs(60);

/// The cadence an unsettled reassignment is re-read at — a probing interval, not a
/// bound on any transition.
const REASSIGNMENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// The topics every cluster in this suite holds, as (name, partitions): the set the
/// pattern rows select from.
const TOPICS: &[(&str, i64)] = &[
    ("orders", 3),
    ("orders-archive", 2),
    ("payments", 1),
    ("telemetry", 4),
];

async fn start_reassignment(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/topic-reassignments", pct(cluster_arn)),
            Some(body),
        )
        .await
}

async fn describe_reassignment(emulator: &Emulator, cluster_arn: &str, operation_arn: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topic-reassignments/{}",
                pct(cluster_arn),
                pct(operation_arn)
            ),
            None,
        )
        .await
}

async fn stop_reassignment(emulator: &Emulator, cluster_arn: &str, operation_arn: &str) -> Reply {
    emulator
        .call(
            "POST",
            &format!(
                "/v1/clusters/{}/topic-reassignments/{}/stop",
                pct(cluster_arn),
                pct(operation_arn)
            ),
            None,
        )
        .await
}

async fn list_reassignments(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/topic-reassignments{query}",
                pct(cluster_arn)
            ),
            None,
        )
        .await
}

async fn describe_operation(emulator: &Emulator, operation_arn: &str) -> Reply {
    emulator
        .call("GET", &format!("/v1/operations/{}", pct(operation_arn)), None)
        .await
}

/// A cluster on `instance_type` holding [`TOPICS`], waited into `ACTIVE`, as
/// (ARN, `CurrentVersion`).
async fn cluster_with_topics(
    emulator: &Emulator,
    name: &str,
    instance_type: &'static str,
) -> (String, String) {
    let request = ClusterRequest {
        instance_type,
        ..ClusterRequest::named(name)
    };
    let arn = emulator.create_ok(&request).await;
    let version = poll_until_active(emulator, &arn, name).await;
    for (topic, partitions) in TOPICS {
        let created = emulator
            .call(
                "POST",
                &format!("/v1/clusters/{}/topics", pct(&arn)),
                Some(json!({
                    "topicName": topic,
                    "partitionCount": partitions,
                    "replicationFactor": 3,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {name}: expected CreateTopic to answer 200 for {topic}, got {} {}",
            created.status, created.body
        );
    }
    (arn, version)
}

/// Poll `DescribeTopicReassignment` until it reports `status`, and hand that read
/// back. Waits for the CONDITION rather than for a span.
async fn poll_until_status(
    emulator: &Emulator,
    cluster_arn: &str,
    operation_arn: &str,
    status: &str,
    case: &str,
) -> Reply {
    let deadline = Instant::now() + REASSIGNMENT_DEADLINE;
    loop {
        let described = describe_reassignment(emulator, cluster_arn, operation_arn).await;
        if described.at("/status").as_str() == Some(status) {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the reassignment never reported {status}, last read {} {}",
            described.status,
            described.body
        );
        std::thread::sleep(REASSIGNMENT_POLL_INTERVAL);
    }
}

/// One row of the pattern table.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    include: Vec<&'static str>,
    exclude: Vec<&'static str>,
    /// The topics of [`TOPICS`] those patterns select.
    selected: Vec<&'static str>,
}

/// The rows the selection rule is checked over: every topic, one exact name, a
/// prefix, a prefix minus one name, and a pattern no topic of the cluster matches.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "all-topics",
            include: vec![".*"],
            exclude: Vec::new(),
            selected: vec!["orders", "orders-archive", "payments", "telemetry"],
        },
        Row {
            case: "one-name",
            include: vec!["payments"],
            exclude: Vec::new(),
            selected: vec!["payments"],
        },
        Row {
            case: "prefix",
            include: vec!["orders.*"],
            exclude: Vec::new(),
            selected: vec!["orders", "orders-archive"],
        },
        Row {
            case: "prefix-minus-one",
            include: vec!["orders.*"],
            exclude: vec!["orders-archive"],
            selected: vec!["orders"],
        },
        Row {
            case: "two-patterns",
            include: vec!["payments", "telemetry"],
            exclude: Vec::new(),
            selected: vec!["payments", "telemetry"],
        },
        Row {
            case: "matches-nothing",
            include: vec!["no-such-topic.*"],
            exclude: Vec::new(),
            selected: Vec::new(),
        },
    ]
}

/// The partitions the named topics hold between them, from [`TOPICS`].
fn partitions_of(selected: &[&str]) -> i64 {
    TOPICS
        .iter()
        .filter(|(topic, _)| selected.contains(topic))
        .map(|(_, partitions)| *partitions)
        .sum()
}

/// RULE 1: the operation counts the cluster's own topics under its patterns, and
/// its status and progress walk the clock up to exactly those counts.
#[tokio::test(flavor = "current_thread")]
async fn a_reassignment_counts_the_topics_its_patterns_select_and_completes_on_the_clock() {
    let emulator = Emulator::new();
    for row in rows() {
        let case = row.case;
        let (cluster_arn, _) = cluster_with_topics(
            &emulator,
            &format!("reassign-{case}"),
            "kafka.m5.large",
        )
        .await;

        let started = start_reassignment(
            &emulator,
            &cluster_arn,
            json!({
                "targetInstanceType": "kafka.m5.large",
                "topicsToInclude": row.include,
                "topicsToExclude": row.exclude,
            }),
        )
        .await;
        assert_eq!(
            started.status, 200,
            "case {case}: expected StartTopicReassignment to answer 200, got {} {}",
            started.status, started.body
        );
        let operation_arn = started
            .at("/topicReassignmentOperationArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: the start returned no operation ARN: {}", started.body)
            })
            .to_string();
        assert!(
            operation_arn.contains(":topic-reassignment/"),
            "case {case}: the ARN names the topic-reassignment resource type, got {operation_arn}"
        );
        assert_eq!(
            started.at("/clusterArn").as_str(),
            Some(cluster_arn.as_str()),
            "case {case}: the start must report the cluster it acts on, got {}",
            started.body
        );
        assert_eq!(
            started.at("/status"),
            &json!("PENDING"),
            "case {case}: an operation the service has just queued is PENDING, got {}",
            started.body
        );

        let expected_topics = i64::try_from(row.selected.len()).unwrap_or(i64::MAX);
        let expected_partitions = partitions_of(&row.selected);
        let reading = describe_reassignment(&emulator, &cluster_arn, &operation_arn).await;
        assert_eq!(
            reading.status, 200,
            "case {case}: expected DescribeTopicReassignment to answer 200, got {} {}",
            reading.status, reading.body
        );
        assert_eq!(
            (
                reading.at("/totalTopics").as_i64(),
                reading.at("/totalPartitions").as_i64()
            ),
            (Some(expected_topics), Some(expected_partitions)),
            "case {case}: the operation must count the cluster's topics under its patterns, got {}",
            reading.body
        );
        assert_eq!(
            reading.at("/topicsToInclude"),
            &json!(row.include),
            "case {case}: the reader must report the patterns it was filed with, got {}",
            reading.body
        );
        assert_eq!(
            reading.at("/topicsToExclude"),
            &json!(row.exclude),
            "case {case}: the reader must report the exclusions it was filed with, got {}",
            reading.body
        );
        assert_eq!(
            reading.at("/targetInstanceType"),
            &json!("kafka.m5.large"),
            "case {case}: the reader must report the broker type it moves onto, got {}",
            reading.body
        );
        assert!(
            reading.at("/creationTime").is_string() && reading.at("/lastUpdateTime").is_string(),
            "case {case}: both instants are ISO-8601 date-time strings, got {}",
            reading.body
        );
        let completed = reading.at("/completedTopics").as_i64().unwrap_or(-1);
        let migrated = reading.at("/migratedPartitions").as_i64().unwrap_or(-1);
        assert!(
            (0..=expected_topics).contains(&completed)
                && (0..=expected_partitions).contains(&migrated),
            "case {case}: the progress may never exceed the totals, got {}",
            reading.body
        );

        let done = poll_until_status(
            &emulator,
            &cluster_arn,
            &operation_arn,
            "COMPLETE",
            case,
        )
        .await;
        assert_eq!(
            (
                done.at("/completedTopics").as_i64(),
                done.at("/migratedPartitions").as_i64()
            ),
            (Some(expected_topics), Some(expected_partitions)),
            "case {case}: a completed reassignment has moved every topic it selected, got {}",
            done.body
        );

        // The listing reports the same record, at the same counts.
        let listed = list_reassignments(&emulator, &cluster_arn, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListTopicReassignments to answer 200, got {} {}",
            listed.status, listed.body
        );
        let entries = listed
            .at("/reassignments")
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {case}: the cluster holds exactly the one operation, got {}",
            listed.body
        );
        let entry = &entries[0];
        assert_eq!(
            (
                entry.get("topicReassignmentOperationArn"),
                entry.get("status"),
                entry.get("totalTopics"),
                entry.get("completedTopics"),
            ),
            (
                Some(&json!(operation_arn)),
                Some(&json!("COMPLETE")),
                Some(&json!(expected_topics)),
                Some(&json!(expected_topics)),
            ),
            "case {case}: the listing and the reader are two views of one record, got {entry}"
        );
        assert_eq!(
            entry.get("creationTime"),
            done.body.get("creationTime"),
            "case {case}: both readers render one stored instant, got {entry}"
        );

        // A reassignment that has finished has nothing left to stop.
        let stopped = stop_reassignment(&emulator, &cluster_arn, &operation_arn).await;
        assert_eq!(
            (stopped.status, stopped.error_code()),
            (400, "BadRequestException"),
            "case {case}: stopping a finished reassignment is a 400 BadRequestException, got {} {}",
            stopped.status,
            stopped.body
        );
    }
}

/// RULE 1, over the stop: an accepted stop freezes the progress and walks
/// `STOPPING` -> `STOPPED`.
#[tokio::test(flavor = "current_thread")]
async fn an_accepted_stop_freezes_the_progress_it_had_reached() {
    let emulator = Emulator::new();
    let case = "stop";
    let (cluster_arn, _) =
        cluster_with_topics(&emulator, "reassign-stop", "kafka.m5.large").await;
    let started = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": [".*"],
        }),
    )
    .await;
    let operation_arn = started
        .at("/topicReassignmentOperationArn")
        .as_str()
        .unwrap_or_default()
        .to_string();

    let stopped = stop_reassignment(&emulator, &cluster_arn, &operation_arn).await;
    assert_eq!(
        stopped.status, 200,
        "case {case}: expected StopTopicReassignment to answer 200, got {} {}",
        stopped.status, stopped.body
    );
    assert_eq!(
        (
            stopped.at("/topicReassignmentOperationArn"),
            stopped.at("/status")
        ),
        (&json!(operation_arn), &json!("STOPPING")),
        "case {case}: an accepted stop reports the operation STOPPING, got {}",
        stopped.body
    );
    // Repeating the stop cannot postpone it or re-report another status.
    let again = stop_reassignment(&emulator, &cluster_arn, &operation_arn).await;
    assert_eq!(
        again.status, 200,
        "case {case}: expected a repeated stop to answer 200, got {} {}",
        again.status, again.body
    );
    assert_eq!(
        again.at("/status"),
        &json!("STOPPING"),
        "case {case}: a repeated stop reports the status the first one left, got {}",
        again.body
    );

    let frozen = describe_reassignment(&emulator, &cluster_arn, &operation_arn).await;
    let completed_at_stop = frozen.at("/completedTopics").as_i64().unwrap_or(-1);
    let total = frozen.at("/totalTopics").as_i64().unwrap_or(-1);
    assert_eq!(
        total,
        i64::try_from(TOPICS.len()).unwrap_or(i64::MAX),
        "case {case}: the operation counted every topic of the cluster, got {}",
        frozen.body
    );
    assert!(
        (0..=total).contains(&completed_at_stop),
        "case {case}: the frozen progress may never exceed the total, got {}",
        frozen.body
    );

    let done = poll_until_status(&emulator, &cluster_arn, &operation_arn, "STOPPED", case).await;
    assert_eq!(
        done.at("/completedTopics").as_i64(),
        Some(completed_at_stop),
        "case {case}: nothing moved after the stop was accepted, got {}",
        done.body
    );
    // The status filter selects by the status the reader reports.
    for (query, expected) in [("?statusFilter=STOPPED", 1usize), ("?statusFilter=COMPLETE", 0)] {
        let listed = list_reassignments(&emulator, &cluster_arn, query).await;
        assert_eq!(
            listed.at("/reassignments").as_array().map(Vec::len),
            Some(expected),
            "case {case} {query}: expected {expected} operations, got {}",
            listed.body
        );
    }
    let bad_filter = list_reassignments(&emulator, &cluster_arn, "?statusFilter=HALTED").await;
    assert_eq!(
        (bad_filter.status, bad_filter.error_code()),
        (400, "BadRequestException"),
        "case {case}: a status outside the model's enum is a 400, got {} {}",
        bad_filter.status,
        bad_filter.body
    );
}

/// RULE 1, over the preconditions: the target type must be one the cluster has, a
/// second start while one is in flight is a conflict, and a repeat carrying the
/// same client token answers the operation the first call filed.
#[tokio::test(flavor = "current_thread")]
async fn a_reassignment_is_admitted_on_its_target_type_and_is_idempotent_per_client_token() {
    let emulator = Emulator::new();
    let case = "admission";
    let (cluster_arn, _) =
        cluster_with_topics(&emulator, "reassign-admission", "kafka.m5.large").await;

    // A type the service does not offer at all, in the service's own words.
    let unoffered = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.96xlarge",
            "topicsToInclude": [".*"],
        }),
    )
    .await;
    assert_eq!(
        (unoffered.status, unoffered.error_code(), unoffered.error_message()),
        (
            400,
            "BadRequestException",
            unsupported_instance_type_message().as_str()
        ),
        "case {case}: an unoffered broker type is refused in the service's own words, got {} {}",
        unoffered.status,
        unoffered.body
    );

    // A type the service offers but this cluster has no brokers of.
    let elsewhere = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "express.m7g.large",
            "topicsToInclude": [".*"],
        }),
    )
    .await;
    assert_eq!(
        (elsewhere.status, elsewhere.error_code()),
        (400, "BadRequestException"),
        "case {case}: a type the cluster has no brokers of is a 400, got {} {}",
        elsewhere.status,
        elsewhere.body
    );
    assert!(
        elsewhere
            .error_message()
            .contains("express.m7g.large"),
        "case {case}: the refusal names the type the request asked for, got {}",
        elsewhere.body
    );

    // A pattern that is not a regular expression at all.
    let unusable = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": ["orders("],
        }),
    )
    .await;
    assert_eq!(
        (unusable.status, unusable.error_code()),
        (400, "BadRequestException"),
        "case {case}: a pattern that cannot select topics is a 400, got {} {}",
        unusable.status,
        unusable.body
    );

    // Accepted, with a client token.
    let first = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": [".*"],
            "clientToken": "token-one",
        }),
    )
    .await;
    assert_eq!(
        first.status, 200,
        "case {case}: expected StartTopicReassignment to answer 200, got {} {}",
        first.status, first.body
    );
    let operation_arn = first
        .at("/topicReassignmentOperationArn")
        .as_str()
        .unwrap_or_default()
        .to_string();

    // The same token answers the same operation rather than filing a second one.
    let repeat = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": [".*"],
            "clientToken": "token-one",
        }),
    )
    .await;
    assert_eq!(
        repeat.status, 200,
        "case {case}: expected a retry carrying the same token to answer 200, got {} {}",
        repeat.status, repeat.body
    );
    assert_eq!(
        repeat.at("/topicReassignmentOperationArn"),
        &json!(operation_arn),
        "case {case}: a retry with the same client token answers the original operation, got {}",
        repeat.body
    );

    // A different token, while the first operation is still in flight, is a
    // conflict: a cluster reassigns one set of partitions at a time.
    let second = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": ["payments"],
            "clientToken": "token-two",
        }),
    )
    .await;
    if second.status != 200 {
        assert_eq!(
            (second.status, second.error_code()),
            (409, "ConflictException"),
            "case {case}: a second reassignment while one runs is a 409 ConflictException, got {} {}",
            second.status,
            second.body
        );
    }

    // The SAME token asking for DIFFERENT work names no operation this service
    // filed, so it is never answered from the first one: it is refused while that
    // one is in flight, or filed as its own operation once it is not.
    let changed = start_reassignment(
        &emulator,
        &cluster_arn,
        json!({
            "targetInstanceType": "kafka.m5.large",
            "topicsToInclude": ["payments"],
            "clientToken": "token-one",
        }),
    )
    .await;
    assert_ne!(
        changed.at("/topicReassignmentOperationArn"),
        &json!(operation_arn),
        "case {case}: a token reused for different work must not answer the original operation, \
         got {} {}",
        changed.status,
        changed.body
    );

    // Only the operations the service actually accepted were ever filed: the retry
    // carrying the same token for the same work added nothing.
    poll_until_status(&emulator, &cluster_arn, &operation_arn, "COMPLETE", case).await;
    let mut expected: Vec<String> = vec![operation_arn.clone()];
    for extra in [&second, &changed] {
        if extra.status == 200 {
            if let Some(arn) = extra.at("/topicReassignmentOperationArn").as_str() {
                expected.push(arn.to_string());
            }
        }
    }
    expected.sort();
    expected.dedup();
    let listed = list_reassignments(&emulator, &cluster_arn, "").await;
    let mut arns: Vec<String> = listed
        .at("/reassignments")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| {
            entry
                .get("topicReassignmentOperationArn")
                .and_then(Value::as_str)
        })
        .map(str::to_string)
        .collect();
    arns.sort();
    assert_eq!(
        arns, expected,
        "case {case}: the registry holds exactly the operations the service accepted, got {}",
        listed.body
    );

    // A reference the service cannot read is a bad request; a well-formed one it
    // holds nothing for is a missing resource.
    let malformed = describe_reassignment(&emulator, &cluster_arn, "not-an-arn").await;
    assert_eq!(
        (malformed.status, malformed.error_code()),
        (400, "BadRequestException"),
        "case {case}: a malformed operation reference is a 400, got {} {}",
        malformed.status,
        malformed.body
    );
    let absent = describe_reassignment(
        &emulator,
        &cluster_arn,
        &operation_arn.replace(
            operation_arn.rsplit('/').next().unwrap_or_default(),
            "11111111-2222-4333-8444-555555555555",
        ),
    )
    .await;
    assert_eq!(
        (absent.status, absent.error_code()),
        (404, "NotFoundException"),
        "case {case}: a well-formed reference to no stored operation is a 404, got {} {}",
        absent.status,
        absent.body
    );
}

/// One row of the upgrade table: the broker type the cluster runs, the type the
/// update moves it to, the mode it asks for, and what `BrokerTypeUpgradeInfo` must
/// then report.
#[derive(Debug, Clone, Copy)]
struct UpgradeRow {
    case: &'static str,
    from: &'static str,
    to: &'static str,
    mode: Option<&'static str>,
    /// The mode the operation must report, or `None` when it reports no upgrade
    /// info at all.
    reported_mode: Option<&'static str>,
    /// Whether the service must have started the reassignment itself.
    starts_reassignment: bool,
}

/// The rows RULE 2 is checked over: both modes, the documented default (no mode
/// named), and the two broker-type changes that are NOT an upgrade from Standard to
/// Express — a change within the Standard families, and one within Express.
fn upgrade_rows() -> Vec<UpgradeRow> {
    vec![
        UpgradeRow {
            case: "manual-upgrade",
            from: "kafka.m5.large",
            to: "express.m7g.large",
            mode: Some("MANUAL"),
            reported_mode: Some("MANUAL"),
            starts_reassignment: false,
        },
        UpgradeRow {
            case: "auto-upgrade",
            from: "kafka.m5.large",
            to: "express.m7g.large",
            mode: Some("AUTO"),
            reported_mode: Some("AUTO"),
            starts_reassignment: true,
        },
        UpgradeRow {
            case: "default-mode",
            from: "kafka.m5.large",
            to: "express.m7g.large",
            mode: None,
            reported_mode: Some("MANUAL"),
            starts_reassignment: false,
        },
        UpgradeRow {
            case: "within-standard",
            from: "kafka.m5.large",
            to: "kafka.m7g.large",
            mode: None,
            reported_mode: None,
            starts_reassignment: false,
        },
        UpgradeRow {
            case: "within-express",
            from: "express.m7g.large",
            to: "express.m7g.xlarge",
            mode: None,
            reported_mode: None,
            starts_reassignment: false,
        },
    ]
}

/// RULE 2: the one operation that initiates a Standard-to-Express upgrade reports
/// how its topics reach the new brokers, and no other operation reports any.
#[tokio::test(flavor = "current_thread")]
async fn only_the_operation_that_initiates_an_express_upgrade_reports_its_topic_reassignment() {
    let emulator = Emulator::new();
    for row in upgrade_rows() {
        let case = row.case;
        let (cluster_arn, version) =
            cluster_with_topics(&emulator, &format!("upgrade-{case}"), row.from).await;

        let mut body = json!({
            "currentVersion": version,
            "targetInstanceType": row.to,
        });
        if let Some(mode) = row.mode {
            body["topicReassignment"] = json!(mode);
        }
        let accepted = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/nodes/type", pct(&cluster_arn)),
                Some(body),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {case}: expected UpdateBrokerType to answer 200, got {} {}",
            accepted.status, accepted.body
        );
        let operation_arn = accepted
            .at("/clusterOperationArn")
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {case}: the update filed no operation: {}", accepted.body)
            })
            .to_string();

        let operation = describe_operation(&emulator, &operation_arn).await;
        assert_eq!(
            operation.status, 200,
            "case {case}: expected DescribeClusterOperation to answer 200, got {} {}",
            operation.status, operation.body
        );
        let upgrade = operation.at("/clusterOperationInfo/brokerTypeUpgradeInfo");
        match row.reported_mode {
            None => assert!(
                upgrade.is_null(),
                "case {case}: an operation that initiates no Standard-to-Express upgrade reports \
                 no brokerTypeUpgradeInfo, got {}",
                operation.body
            ),
            Some(mode) => {
                assert_eq!(
                    upgrade.get("topicReassignmentMode"),
                    Some(&json!(mode)),
                    "case {case}: the operation must report the mode the call asked for, got {}",
                    operation.body
                );
                let reassignment_arn = upgrade
                    .get("topicReassignmentOperationArn")
                    .and_then(Value::as_str);
                assert_eq!(
                    reassignment_arn.is_some(),
                    row.starts_reassignment,
                    "case {case}: the reassignment ARN is reported once the service has started \
                     one, got {}",
                    operation.body
                );
                if let Some(reassignment_arn) = reassignment_arn {
                    // AUTO reassigns every topic to the target broker type, so the
                    // operation it started counts the cluster's whole topic set.
                    let described =
                        describe_reassignment(&emulator, &cluster_arn, reassignment_arn).await;
                    assert_eq!(
                        described.status, 200,
                        "case {case}: the reassignment the upgrade started must resolve, got {} {}",
                        described.status, described.body
                    );
                    assert_eq!(
                        (
                            described.at("/targetInstanceType"),
                            described.at("/topicsToInclude"),
                            described.at("/totalTopics").as_i64(),
                        ),
                        (
                            &json!(row.to),
                            &json!([".*"]),
                            Some(i64::try_from(TOPICS.len()).unwrap_or(i64::MAX)),
                        ),
                        "case {case}: AUTO reassigns every topic onto the target type, got {}",
                        described.body
                    );
                }
            }
        }

        // The V2 reader of the same operation reports the same upgrade info, under
        // its provisioned arm.
        let v2 = emulator
            .call(
                "GET",
                &format!("/api/v2/operations/{}", pct(&operation_arn)),
                None,
            )
            .await;
        assert_eq!(
            v2.status, 200,
            "case {case}: expected DescribeClusterOperationV2 to answer 200, got {} {}",
            v2.status, v2.body
        );
        assert_eq!(
            v2.at("/clusterOperationInfo/provisioned/brokerTypeUpgradeInfo"),
            upgrade,
            "case {case}: the V1 and V2 views of one operation report one upgrade, got {}",
            v2.body
        );
    }
}

/// RULE 2, over the member's own value set: a `TopicReassignment` outside the
/// model's enum names no mode the service could record, so the update is refused
/// rather than accepted and reported back as a mode AWS does not define.
#[tokio::test(flavor = "current_thread")]
async fn a_topic_reassignment_mode_outside_the_models_enum_is_refused() {
    let emulator = Emulator::new();
    let case = "mode";
    let (cluster_arn, version) =
        cluster_with_topics(&emulator, "upgrade-mode", "kafka.m5.large").await;
    for mode in ["auto", "AUTOMATIC", "NONE", ""] {
        let refused = emulator
            .call(
                "PUT",
                &format!("/v1/clusters/{}/nodes/type", pct(&cluster_arn)),
                Some(json!({
                    "currentVersion": version,
                    "targetInstanceType": "express.m7g.large",
                    "topicReassignment": mode,
                })),
            )
            .await;
        assert_eq!(
            (refused.status, refused.error_code()),
            (400, "BadRequestException"),
            "case {case} {mode:?}: a mode outside the model's enum is a 400 BadRequestException, \
             got {} {}",
            refused.status,
            refused.body
        );
    }
    // The cluster took no change from any of them: its version is the one it
    // settled at, so a refused update left nothing behind.
    let described = emulator.describe(&cluster_arn).await;
    assert_eq!(
        described.at("/clusterInfo/currentVersion"),
        &json!(version),
        "case {case}: a refused update must not move the cluster's version, got {}",
        described.body
    );
    assert_eq!(
        described.at("/clusterInfo/state"),
        &json!("ACTIVE"),
        "case {case}: a refused update must leave the cluster serving, got {}",
        described.body
    );
}
