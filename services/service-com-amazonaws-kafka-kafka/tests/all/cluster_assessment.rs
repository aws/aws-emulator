// SPDX-License-Identifier: CC0-1.0

//! The ASSESSMENT family of one cluster — `CreateAssessment`, `DescribeAssessment`,
//! `ListAssessments`, `ListAssessmentFindings` and `DeleteAssessment` — driven end
//! to end over the same `MultiServiceRouter` the emulator serves from.
//!
//! RULE: an assessment of a cluster is a stored resource whose FINDINGS are
//! computed from that cluster's own state when the scan runs, each classified with
//! one of the model's `FindingCategory` and `FindingSeverity` values and identified
//! by a content-based id ("generated from hash(category + resource + check_type)"),
//! and every reader answers from that one record: the scan reports `IN_PROGRESS`
//! until the clock owes it `COMPLETE`, its summary counts exactly the findings the
//! listing pages, and its deletion moves it to `DELETING` and then out of the
//! registry.
//!
//! The rows differ along what the rule generalises over: the broker type the
//! cluster runs, whether the request names a target instance type (and whether that
//! type is one the service offers), whether the cluster holds topics below the
//! replication factor Amazon MSK recommends, and the state the cluster is in when
//! the scan runs.
//!
//! The router, the `Reply` wrapper, the percent-encoder and the `CreateCluster`
//! request builder come from [`crate::kafka_harness`], the crate's shared harness.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_kafka_kafka::state::{
    unsupported_instance_type_message, MAX_ASSESSMENTS_PER_CLUSTER, RESOURCE_NOT_FOUND_MESSAGE,
};

use crate::kafka_harness::{pct, poll_until_active, ClusterRequest, Emulator, Reply};

/// How long a suite waits for the status the clock owes an assessment before it
/// calls the emulator broken: far longer than the span the service owes that
/// transition, so the deadline says nothing about how fast the host runs.
const ASSESSMENT_DEADLINE: Duration = Duration::from_secs(60);

/// The cadence an unsettled assessment is re-read at — a probing interval, not a
/// bound on the transition.
const ASSESSMENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

async fn create_assessment(emulator: &Emulator, cluster_arn: &str, body: Value) -> Reply {
    emulator
        .call(
            "POST",
            &format!("/v1/clusters/{}/assessments", pct(cluster_arn)),
            Some(body),
        )
        .await
}

async fn describe_assessment(
    emulator: &Emulator,
    cluster_arn: &str,
    assessment_arn: &str,
) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/assessments/{}",
                pct(cluster_arn),
                pct(assessment_arn)
            ),
            None,
        )
        .await
}

async fn list_assessments(emulator: &Emulator, cluster_arn: &str, query: &str) -> Reply {
    emulator
        .call(
            "GET",
            &format!("/v1/clusters/{}/assessments{query}", pct(cluster_arn)),
            None,
        )
        .await
}

async fn list_findings(
    emulator: &Emulator,
    cluster_arn: &str,
    assessment_arn: &str,
    query: &str,
) -> Reply {
    emulator
        .call(
            "GET",
            &format!(
                "/v1/clusters/{}/assessments/{}/findings{query}",
                pct(cluster_arn),
                pct(assessment_arn)
            ),
            None,
        )
        .await
}

async fn delete_assessment(emulator: &Emulator, cluster_arn: &str, assessment_arn: &str) -> Reply {
    emulator
        .call(
            "DELETE",
            &format!(
                "/v1/clusters/{}/assessments/{}",
                pct(cluster_arn),
                pct(assessment_arn)
            ),
            None,
        )
        .await
}

/// Poll `DescribeAssessment` until the scan reports `COMPLETE`, and hand that read
/// back. Waits for the CONDITION rather than for a span: the transition is one the
/// clock owes the assessment.
async fn poll_until_complete(
    emulator: &Emulator,
    cluster_arn: &str,
    assessment_arn: &str,
    case: &str,
) -> Reply {
    let deadline = Instant::now() + ASSESSMENT_DEADLINE;
    loop {
        let described = describe_assessment(emulator, cluster_arn, assessment_arn).await;
        if described.at("/status").as_str() == Some("COMPLETE") {
            return described;
        }
        assert!(
            Instant::now() < deadline,
            "case {case}: the assessment never reported COMPLETE, last read {} {}",
            described.status,
            described.body
        );
        std::thread::sleep(ASSESSMENT_POLL_INTERVAL);
    }
}

/// Every finding the listing reports for one assessment, following `nextToken` to
/// the end.
async fn all_findings(
    emulator: &Emulator,
    cluster_arn: &str,
    assessment_arn: &str,
    case: &str,
) -> Vec<Value> {
    let mut reported: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let query = match &token {
            Some(resume) => format!("?nextToken={}", pct(resume)),
            None => String::new(),
        };
        let reply = list_findings(emulator, cluster_arn, assessment_arn, &query).await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected ListAssessmentFindings to answer 200, got {} {}",
            reply.status, reply.body
        );
        reported.extend(reply.at("/findings").as_array().cloned().unwrap_or_default());
        token = reply.at("/nextToken").as_str().map(str::to_string);
        if token.is_none() {
            break;
        }
        pages += 1;
        assert!(
            pages < 100,
            "case {case}: the paginated read did not terminate after {pages} pages"
        );
    }
    reported
}

/// One topic to put on the cluster a row assesses.
#[derive(Debug, Clone, Copy)]
struct TopicRow {
    name: &'static str,
    partitions: i64,
    replication: i64,
}

/// One row of the table: a cluster, the topics it holds, and the parameters the
/// scan is asked for.
#[derive(Debug, Clone)]
struct Row {
    case: &'static str,
    instance_type: &'static str,
    brokers: i64,
    topics: Vec<TopicRow>,
    target_instance_type: Option<&'static str>,
    /// The findings the scan must detect, as (category, severity, resource) —
    /// every one of them a fact of the cluster this row stands up.
    expected: Vec<(&'static str, &'static str, Option<&'static str>)>,
    /// Whether any of them blocks the migration (an `ERROR` finding).
    blocking: bool,
}

/// The rows the rule is checked over.
fn rows() -> Vec<Row> {
    vec![
        // No target type named, every topic at the recommended factor: the scan
        // has only the recommendation to report, which the model documents as a
        // CAPACITY finding.
        Row {
            case: "recommendation-only",
            instance_type: "kafka.m5.large",
            brokers: 3,
            topics: vec![TopicRow {
                name: "orders",
                partitions: 3,
                replication: 3,
            }],
            target_instance_type: None,
            expected: vec![("CAPACITY", "INFO", None)],
            blocking: false,
        },
        // Two topics below the recommended replication factor: one finding each,
        // naming the topic as the resource affected.
        Row {
            case: "topics-below-recommended-factor",
            instance_type: "kafka.m7g.xlarge",
            brokers: 3,
            topics: vec![
                TopicRow {
                    name: "events",
                    partitions: 2,
                    replication: 1,
                },
                TopicRow {
                    name: "audit",
                    partitions: 1,
                    replication: 2,
                },
                TopicRow {
                    name: "ledger",
                    partitions: 3,
                    replication: 3,
                },
            ],
            target_instance_type: Some("express.m7g.xlarge"),
            expected: vec![
                ("TOPIC_CONFIGURATION", "WARNING", Some("audit")),
                ("TOPIC_CONFIGURATION", "WARNING", Some("events")),
            ],
            blocking: false,
        },
        // A target type the service does not offer: an INFRASTRUCTURE finding that
        // blocks the migration, in the service's own words.
        Row {
            case: "unoffered-target-type",
            instance_type: "kafka.m5.large",
            brokers: 3,
            topics: Vec::new(),
            target_instance_type: Some("express.m7g.32xlarge"),
            expected: vec![("INFRASTRUCTURE", "ERROR", None)],
            blocking: true,
        },
        // A cluster already running Express brokers, assessed against a Standard
        // type: nothing is wrong with it, so the scan reports nothing at all.
        Row {
            case: "no-findings",
            instance_type: "express.m7g.large",
            brokers: 3,
            topics: Vec::new(),
            target_instance_type: Some("kafka.m5.large"),
            expected: Vec::new(),
            blocking: false,
        },
    ]
}

/// The findings a reply reports, as a sorted (category, severity, resource) table —
/// the shape a row states its expectation in.
fn classified(findings: &[Value]) -> Vec<(String, String, Option<String>)> {
    let mut rows: Vec<(String, String, Option<String>)> = findings
        .iter()
        .map(|finding| {
            (
                finding
                    .get("category")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                finding
                    .get("severity")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                finding
                    .get("resource")
                    .and_then(Value::as_str)
                    .map(str::to_string),
            )
        })
        .collect();
    rows.sort();
    rows
}

/// RULE: the scan's findings are computed from the cluster's state, the summary
/// counts exactly the findings the listing pages, and both are read off one record.
#[tokio::test(flavor = "current_thread")]
async fn an_assessment_reports_the_findings_its_cluster_state_implies() {
    let emulator = Emulator::new();
    for row in rows() {
        let case = row.case;
        let mut request = ClusterRequest::named(&format!("assess-{case}"));
        request.brokers = row.brokers;
        request.instance_type = row.instance_type;
        let cluster_arn = emulator.create_ok(&request).await;
        poll_until_active(&emulator, &cluster_arn, case).await;
        for topic in &row.topics {
            let created = emulator
                .call(
                    "POST",
                    &format!("/v1/clusters/{}/topics", pct(&cluster_arn)),
                    Some(json!({
                        "topicName": topic.name,
                        "partitionCount": topic.partitions,
                        "replicationFactor": topic.replication,
                    })),
                )
                .await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected CreateTopic to answer 200 for {}, got {} {}",
                topic.name, created.status, created.body
            );
        }

        let mut body = json!({ "assessmentType": "UPDATE_BROKER_TYPE" });
        if let Some(target) = row.target_instance_type {
            body["assessmentParameters"] = json!({ "targetInstanceType": target });
        }
        let created = create_assessment(&emulator, &cluster_arn, body).await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateAssessment to answer 200, got {} {}",
            created.status, created.body
        );
        let assessment_arn = created
            .at("/assessmentArn")
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: CreateAssessment returned no ARN: {}", created.body))
            .to_string();
        assert_eq!(
            created.at("/status"),
            &json!("IN_PROGRESS"),
            "case {case}: a scan the service has just started is IN_PROGRESS, got {}",
            created.body
        );
        assert_eq!(
            created.at("/clusterArn").as_str(),
            Some(cluster_arn.as_str()),
            "case {case}: the create must report the cluster it assesses, got {}",
            created.body
        );
        assert_eq!(
            created.at("/assessmentType"),
            &json!("UPDATE_BROKER_TYPE"),
            "case {case}: the create must report the type it was asked for, got {}",
            created.body
        );
        assert!(
            created.at("/creationTime").is_string(),
            "case {case}: creationTime must be an ISO-8601 date-time string, got {}",
            created.body
        );

        // A scan still running has no findings to report yet, which is what its
        // ConflictException says.
        let early = list_findings(&emulator, &cluster_arn, &assessment_arn, "").await;
        if early.status != 200 {
            assert_eq!(
                (early.status, early.error_code()),
                (409, "ConflictException"),
                "case {case}: findings of a scan still running are a 409 ConflictException, got {} {}",
                early.status,
                early.body
            );
        }

        let described = poll_until_complete(&emulator, &cluster_arn, &assessment_arn, case).await;
        assert_eq!(
            described.at("/assessmentArn").as_str(),
            Some(assessment_arn.as_str()),
            "case {case}: the reader must report the assessment it was asked for, got {}",
            described.body
        );
        assert!(
            described.at("/completionTime").is_string(),
            "case {case}: a COMPLETE scan reports when it completed, got {}",
            described.body
        );
        assert_eq!(
            described.at("/assessmentParameters/targetInstanceType").as_str(),
            row.target_instance_type,
            "case {case}: the reader must report the parameters the scan ran with, got {}",
            described.body
        );

        let findings = all_findings(&emulator, &cluster_arn, &assessment_arn, case).await;
        let expected: Vec<(String, String, Option<String>)> = {
            let mut expected: Vec<(String, String, Option<String>)> = row
                .expected
                .iter()
                .map(|(category, severity, resource)| {
                    (
                        (*category).to_string(),
                        (*severity).to_string(),
                        resource.map(str::to_string),
                    )
                })
                .collect();
            expected.sort();
            expected
        };
        assert_eq!(
            classified(&findings),
            expected,
            "case {case}: the scan must report the findings the cluster's state implies, got \
             {findings:?}"
        );

        // The summary counts exactly those findings, grouped the two ways the model
        // asks for.
        let mut by_category: BTreeMap<String, i64> = BTreeMap::new();
        let mut by_severity: BTreeMap<String, i64> = BTreeMap::new();
        for (category, severity, _) in classified(&findings) {
            *by_category.entry(category).or_default() += 1;
            *by_severity.entry(severity).or_default() += 1;
        }
        assert_eq!(
            described.at("/summary/totalFindings").as_i64(),
            Some(i64::try_from(findings.len()).unwrap_or(i64::MAX)),
            "case {case}: totalFindings must count the findings the scan detected, got {}",
            described.body
        );
        assert_eq!(
            described.at("/summary/findingsCapped"),
            &json!(false),
            "case {case}: a scan whose findings are all stored reports findingsCapped false, \
             got {}",
            described.body
        );
        assert_eq!(
            described.at("/summary/hasBlockingFindings"),
            &json!(row.blocking),
            "case {case}: hasBlockingFindings must say whether an ERROR finding was detected, \
             got {}",
            described.body
        );
        for (member, counts) in [
            ("byCategory", by_category),
            ("bySeverity", by_severity),
        ] {
            let reported = described
                .at(&format!("/summary/{member}"))
                .as_object()
                .cloned()
                .unwrap_or_default();
            let reported: BTreeMap<String, i64> = reported
                .into_iter()
                .map(|(key, value)| (key, value.as_i64().unwrap_or_default()))
                .collect();
            assert_eq!(
                reported, counts,
                "case {case}: {member} must group the reported findings, got {}",
                described.body
            );
        }

        // Every finding carries the three members the model marks required, and its
        // id is the content-based one: two findings of one assessment never share
        // an id.
        let mut seen_ids: Vec<&str> = Vec::new();
        for finding in &findings {
            for member in ["findingId", "category", "message", "severity"] {
                assert!(
                    finding.get(member).and_then(Value::as_str).is_some(),
                    "case {case}: a finding must carry {member}, got {finding}"
                );
            }
            let id = finding
                .get("findingId")
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert!(
                !seen_ids.contains(&id),
                "case {case}: two findings share the id {id}: {findings:?}"
            );
            seen_ids.push(id);
        }

        // The row that names a type the service does not offer is told so in the
        // service's own words.
        if row.target_instance_type == Some("express.m7g.32xlarge") {
            let message = findings
                .first()
                .and_then(|finding| finding.get("message"))
                .and_then(Value::as_str)
                .unwrap_or_default();
            assert_eq!(
                message,
                unsupported_instance_type_message(),
                "case {case}: an unoffered target type is refused in the service's own words, \
                 got {findings:?}"
            );
        }

        // The listing reports the same record the reader does.
        let listed = list_assessments(&emulator, &cluster_arn, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case}: expected ListAssessments to answer 200, got {} {}",
            listed.status, listed.body
        );
        let entries = listed.at("/assessments").as_array().cloned().unwrap_or_default();
        assert_eq!(
            entries.len(),
            1,
            "case {case}: the cluster holds exactly the one assessment, got {}",
            listed.body
        );
        let entry = &entries[0];
        assert_eq!(
            entry.get("assessmentArn"),
            Some(&json!(assessment_arn)),
            "case {case}: the listing must report the stored assessment, got {entry}"
        );
        assert_eq!(
            entry.get("status"),
            Some(&json!("COMPLETE")),
            "case {case}: the listing must report the status the reader does, got {entry}"
        );
        assert_eq!(
            entry.get("creationTime"),
            described.body.get("creationTime"),
            "case {case}: both readers render one stored instant, got {entry}"
        );
    }
}

/// RULE, over the filters and the identity rule: a category or severity filter
/// reports exactly the findings carrying it, an unknown filter value is refused,
/// and one cluster's problem keeps one finding id across two scans of it.
#[tokio::test(flavor = "current_thread")]
async fn findings_are_filtered_and_identified_by_their_content() {
    let emulator = Emulator::new();
    let case = "filters";
    let cluster_arn = emulator
        .create_ok(&ClusterRequest::named("assess-filters"))
        .await;
    poll_until_active(&emulator, &cluster_arn, case).await;
    for (topic, replication) in [("alpha", 1), ("beta", 2)] {
        let created = emulator
            .call(
                "POST",
                &format!("/v1/clusters/{}/topics", pct(&cluster_arn)),
                Some(json!({
                    "topicName": topic,
                    "partitionCount": 1,
                    "replicationFactor": replication,
                })),
            )
            .await;
        assert_eq!(
            created.status, 200,
            "case {case}: expected CreateTopic to answer 200 for {topic}, got {} {}",
            created.status, created.body
        );
    }

    let first = create_assessment(
        &emulator,
        &cluster_arn,
        json!({ "assessmentType": "UPDATE_BROKER_TYPE" }),
    )
    .await;
    let first_arn = first
        .at("/assessmentArn")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: CreateAssessment returned no ARN: {}", first.body))
        .to_string();
    poll_until_complete(&emulator, &cluster_arn, &first_arn, case).await;
    let all = all_findings(&emulator, &cluster_arn, &first_arn, case).await;
    assert_eq!(
        classified(&all),
        vec![
            ("CAPACITY".to_string(), "INFO".to_string(), None),
            (
                "TOPIC_CONFIGURATION".to_string(),
                "WARNING".to_string(),
                Some("alpha".to_string())
            ),
            (
                "TOPIC_CONFIGURATION".to_string(),
                "WARNING".to_string(),
                Some("beta".to_string())
            ),
        ],
        "case {case}: the scan must report one finding per topic below the recommended factor \
         plus the recommendation, got {all:?}"
    );

    // Each filter narrows to exactly the findings carrying that value.
    for (query, expected) in [
        ("?category=TOPIC_CONFIGURATION", 2usize),
        ("?category=CAPACITY", 1),
        ("?severity=WARNING", 2),
        ("?severity=INFO", 1),
        ("?severity=ERROR", 0),
        ("?category=CAPACITY&severity=WARNING", 0),
    ] {
        let reply = list_findings(&emulator, &cluster_arn, &first_arn, query).await;
        assert_eq!(
            reply.status, 200,
            "case {case} {query}: expected 200, got {} {}",
            reply.status, reply.body
        );
        let findings = reply.at("/findings").as_array().cloned().unwrap_or_default();
        assert_eq!(
            findings.len(),
            expected,
            "case {case} {query}: expected {expected} findings, got {}",
            reply.body
        );
    }

    // A value outside the model's enums selects nothing, so it is refused rather
    // than answered with an empty page.
    for query in ["?category=NOT_A_CATEGORY", "?severity=CRITICAL"] {
        let reply = list_findings(&emulator, &cluster_arn, &first_arn, query).await;
        assert_eq!(
            (reply.status, reply.error_code()),
            (400, "BadRequestException"),
            "case {case} {query}: an unknown filter value is a 400 BadRequestException, got {} {}",
            reply.status,
            reply.body
        );
    }

    // A page smaller than the finding set carries a token, and following it reports
    // the rest exactly once.
    let page = list_findings(&emulator, &cluster_arn, &first_arn, "?maxResults=2").await;
    assert_eq!(
        page.at("/findings").as_array().map(Vec::len),
        Some(2),
        "case {case}: a page of two reports two findings, got {}",
        page.body
    );
    let token = page
        .at("/nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a full page must carry a token: {}", page.body))
        .to_string();
    let rest = list_findings(
        &emulator,
        &cluster_arn,
        &first_arn,
        &format!("?maxResults=2&nextToken={}", pct(&token)),
    )
    .await;
    assert_eq!(
        rest.at("/findings").as_array().map(Vec::len),
        Some(1),
        "case {case}: the second page reports the remaining finding, got {}",
        rest.body
    );

    // "The same resource with the same problem always produces the same findingId
    // across assessment runs": a second scan of the unchanged cluster reports the
    // same ids.
    let second = create_assessment(
        &emulator,
        &cluster_arn,
        json!({ "assessmentType": "UPDATE_BROKER_TYPE" }),
    )
    .await;
    assert_eq!(
        second.status, 200,
        "case {case}: expected a second CreateAssessment to answer 200, got {} {}",
        second.status, second.body
    );
    let second_arn = second
        .at("/assessmentArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert_ne!(
        second_arn, first_arn,
        "case {case}: a second scan is its own resource, got {}",
        second.body
    );
    poll_until_complete(&emulator, &cluster_arn, &second_arn, case).await;
    let again = all_findings(&emulator, &cluster_arn, &second_arn, case).await;
    let ids = |findings: &[Value]| -> Vec<String> {
        let mut ids: Vec<String> = findings
            .iter()
            .map(|finding| {
                finding
                    .get("findingId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        ids.sort();
        ids
    };
    assert_eq!(
        ids(&again),
        ids(&all),
        "case {case}: one problem of one resource keeps one finding id across runs, got \
         {again:?} after {all:?}"
    );
}

/// RULE, over the lifecycle and the references: a scan still running cannot be
/// deleted, an accepted delete reports `DELETING` and then takes the assessment out
/// of the registry, and a reference the service cannot read or resolve is refused
/// on which of the two it is.
#[tokio::test(flavor = "current_thread")]
async fn an_assessment_is_deleted_and_its_references_are_refused_on_their_kind() {
    let emulator = Emulator::new();
    let case = "lifecycle";
    let cluster_arn = emulator
        .create_ok(&ClusterRequest::named("assess-lifecycle"))
        .await;
    poll_until_active(&emulator, &cluster_arn, case).await;

    let created = create_assessment(
        &emulator,
        &cluster_arn,
        json!({
            "assessmentType": "UPDATE_BROKER_TYPE",
            "tags": { "owner": "platform" },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected CreateAssessment to answer 200, got {} {}",
        created.status, created.body
    );
    let assessment_arn = created
        .at("/assessmentArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        assessment_arn.contains(":assessment/"),
        "case {case}: an assessment ARN names the assessment resource type, got {assessment_arn}"
    );

    // A second scan of the same cluster while the first is running is a conflict.
    let second = create_assessment(
        &emulator,
        &cluster_arn,
        json!({ "assessmentType": "UPDATE_BROKER_TYPE" }),
    )
    .await;
    if second.status != 200 {
        assert_eq!(
            (second.status, second.error_code()),
            (409, "ConflictException"),
            "case {case}: a second scan while one runs is a 409 ConflictException, got {} {}",
            second.status,
            second.body
        );
    }

    let described = poll_until_complete(&emulator, &cluster_arn, &assessment_arn, case).await;
    assert_eq!(
        described.at("/tags/owner"),
        &json!("platform"),
        "case {case}: the reader must report the tags the create carried, got {}",
        described.body
    );

    let deleted = delete_assessment(&emulator, &cluster_arn, &assessment_arn).await;
    assert_eq!(
        deleted.status, 200,
        "case {case}: expected DeleteAssessment to answer 200, got {} {}",
        deleted.status, deleted.body
    );
    assert_eq!(
        (deleted.at("/assessmentArn"), deleted.at("/status")),
        (&json!(assessment_arn), &json!("DELETING")),
        "case {case}: an accepted delete reports the assessment DELETING, got {}",
        deleted.body
    );

    // The record stays resolvable in DELETING until the clock owes its removal, and
    // is gone after it.
    let deadline = Instant::now() + ASSESSMENT_DEADLINE;
    loop {
        let read = describe_assessment(&emulator, &cluster_arn, &assessment_arn).await;
        if read.status == 404 {
            assert_eq!(
                read.error_code(),
                "NotFoundException",
                "case {case}: a removed assessment is a NotFoundException, got {}",
                read.body
            );
            assert_eq!(
                read.error_message(),
                RESOURCE_NOT_FOUND_MESSAGE,
                "case {case}: the refusal carries the service's own sentence, got {}",
                read.body
            );
            break;
        }
        assert_eq!(
            read.at("/status"),
            &json!("DELETING"),
            "case {case}: an assessment on its way out reports DELETING, got {}",
            read.body
        );
        assert!(
            Instant::now() < deadline,
            "case {case}: the assessment was never removed, last read {}",
            read.body
        );
        std::thread::sleep(ASSESSMENT_POLL_INTERVAL);
    }

    // The slot it held is free again, so the cluster can be scanned to its quota.
    let mut held: Vec<String> = Vec::new();
    for ordinal in 0..MAX_ASSESSMENTS_PER_CLUSTER {
        let reply = create_assessment(
            &emulator,
            &cluster_arn,
            json!({ "assessmentType": "UPDATE_BROKER_TYPE" }),
        )
        .await;
        assert_eq!(
            reply.status, 200,
            "case {case}: expected scan {ordinal} to be accepted, got {} {}",
            reply.status, reply.body
        );
        let arn = reply
            .at("/assessmentArn")
            .as_str()
            .unwrap_or_default()
            .to_string();
        poll_until_complete(&emulator, &cluster_arn, &arn, case).await;
        held.push(arn);
    }
    let over = create_assessment(
        &emulator,
        &cluster_arn,
        json!({ "assessmentType": "UPDATE_BROKER_TYPE" }),
    )
    .await;
    assert_eq!(
        (over.status, over.error_code()),
        (429, "TooManyRequestsException"),
        "case {case}: a scan past the cluster's quota of {MAX_ASSESSMENTS_PER_CLUSTER} is a 429 \
         TooManyRequestsException, got {} {}",
        over.status,
        over.body
    );

    // A value that is not an assessment ARN is a bad request; a well-formed one the
    // registry holds nothing for is a missing resource.
    let malformed = describe_assessment(&emulator, &cluster_arn, "not-an-arn").await;
    assert_eq!(
        (malformed.status, malformed.error_code()),
        (400, "BadRequestException"),
        "case {case}: a malformed assessment reference is a 400 BadRequestException, got {} {}",
        malformed.status,
        malformed.body
    );
    let absent = describe_assessment(&emulator, &cluster_arn, &assessment_arn).await;
    assert_eq!(
        (absent.status, absent.error_code()),
        (404, "NotFoundException"),
        "case {case}: a well-formed reference to a removed assessment is a 404, got {} {}",
        absent.status,
        absent.body
    );

    // An unknown assessment TYPE is refused, as is a listing filter outside the
    // model's enum.
    let wrong_type = create_assessment(
        &emulator,
        &cluster_arn,
        json!({ "assessmentType": "UPDATE_STORAGE" }),
    )
    .await;
    assert_eq!(
        (wrong_type.status, wrong_type.error_code()),
        (400, "BadRequestException"),
        "case {case}: an assessment type outside the model's enum is a 400, got {} {}",
        wrong_type.status,
        wrong_type.body
    );
    let wrong_filter = list_assessments(&emulator, &cluster_arn, "?assessmentType=UPDATE_STORAGE").await;
    assert_eq!(
        (wrong_filter.status, wrong_filter.error_code()),
        (400, "BadRequestException"),
        "case {case}: a filter outside the model's enum is a 400, got {} {}",
        wrong_filter.status,
        wrong_filter.body
    );

    // The listing pages its own order, and a page smaller than the set carries a
    // token that reports the rest.
    let page = list_assessments(&emulator, &cluster_arn, "?maxResults=1").await;
    assert_eq!(
        page.at("/assessments").as_array().map(Vec::len),
        Some(1),
        "case {case}: a page of one reports one assessment, got {}",
        page.body
    );
    let token = page
        .at("/nextToken")
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: a full page must carry a token: {}", page.body))
        .to_string();
    let mut seen: Vec<String> = page
        .at("/assessments")
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|entry| entry.get("assessmentArn").and_then(Value::as_str))
        .map(str::to_string)
        .collect();
    let rest = list_assessments(
        &emulator,
        &cluster_arn,
        &format!("?nextToken={}", pct(&token)),
    )
    .await;
    seen.extend(
        rest.at("/assessments")
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .filter_map(|entry| entry.get("assessmentArn").and_then(Value::as_str))
            .map(str::to_string),
    );
    let mut expected = held.clone();
    expected.sort();
    let mut reported = seen.clone();
    reported.sort();
    assert_eq!(
        reported, expected,
        "case {case}: the pages together must report every stored assessment exactly once, got \
         {seen:?}"
    );

    // A bad resume token is refused rather than resuming nothing.
    let bad_token = list_assessments(&emulator, &cluster_arn, "?nextToken=not-a-token").await;
    assert_eq!(
        (bad_token.status, bad_token.error_code()),
        (400, "BadRequestException"),
        "case {case}: an unreadable token is a 400 BadRequestException, got {} {}",
        bad_token.status,
        bad_token.body
    );
}

/// RULE, over the cluster's own state: a cluster that cannot take a broker-type
/// change right now is reported as a HEALTH finding that blocks the migration.
///
/// The cluster is held in a non-`ACTIVE` state by a broker REBOOT, whose span is
/// the longest this service owes any transition — the one limit this case tests —
/// and the read after the scan is what witnesses that the cluster really was in
/// that state while the scan ran.
#[tokio::test(flavor = "current_thread")]
async fn a_cluster_that_cannot_take_the_change_is_a_blocking_health_finding() {
    let emulator = Emulator::new();
    let case = "cluster-state";
    let cluster_arn = emulator
        .create_ok(&ClusterRequest::named("assess-rebooting"))
        .await;
    poll_until_active(&emulator, &cluster_arn, case).await;
    let rebooted = emulator
        .call(
            "PUT",
            &format!("/v1/clusters/{}/reboot-broker", pct(&cluster_arn)),
            Some(json!({ "brokerIds": ["1"] })),
        )
        .await;
    assert_eq!(
        rebooted.status, 200,
        "case {case}: expected RebootBroker to answer 200, got {} {}",
        rebooted.status, rebooted.body
    );

    let created = create_assessment(
        &emulator,
        &cluster_arn,
        json!({
            "assessmentType": "UPDATE_BROKER_TYPE",
            "assessmentParameters": { "targetInstanceType": "express.m7g.large" },
        }),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "case {case}: expected CreateAssessment to answer 200, got {} {}",
        created.status, created.body
    );
    // Read right after the scan: the cluster was in this state while the scan ran,
    // so the finding below is about a state the service really was in.
    let during = emulator.describe(&cluster_arn).await;
    assert_eq!(
        during.at("/clusterInfo/state"),
        &json!("REBOOTING_BROKER"),
        "case {case}: the reboot must still be in flight when the scan ran, got {}",
        during.body
    );

    let assessment_arn = created
        .at("/assessmentArn")
        .as_str()
        .unwrap_or_default()
        .to_string();
    let described = poll_until_complete(&emulator, &cluster_arn, &assessment_arn, case).await;
    let findings = all_findings(&emulator, &cluster_arn, &assessment_arn, case).await;
    assert!(
        classified(&findings)
            .iter()
            .any(|(category, severity, _)| category == "HEALTH" && severity == "ERROR"),
        "case {case}: a cluster that cannot take the change is a blocking HEALTH finding, got \
         {findings:?}"
    );
    assert_eq!(
        described.at("/summary/hasBlockingFindings"),
        &json!(true),
        "case {case}: an ERROR finding makes hasBlockingFindings true, got {}",
        described.body
    );
}
