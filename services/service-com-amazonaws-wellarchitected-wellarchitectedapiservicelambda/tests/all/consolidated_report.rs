// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `GetConsolidatedReport`: the CONSOLIDATED report over the
//! workloads the calling account can see.
//!
//! The rules these pin, over TABLES of inputs rather than one request:
//!
//! * every reported metric is DERIVED from the workload it reports — its identity, the
//!   instant it last changed and its risk roll-up are the ones `GetWorkload` reports, the
//!   lenses are the ones it applies (one `LensMetric` each, counted by
//!   `LensesAppliedCount`), and each lens's and each pillar's roll-up is the one
//!   `GetLensReview` reports for that review, so a metric and a read of the same review
//!   cannot disagree;
//! * `Questions` is "the questions that have been identified as risks in the pillar"
//!   (`smithy_query(query="shape", shape="PillarMetric")`), so a question is reported
//!   exactly when its stored answer leaves a risk (`HIGH` or `MEDIUM`) — the same selection
//!   `ListLensReviewImprovements` makes — and its `BestPractices` are the choices that
//!   answer leaves out;
//! * the report is PAGED by its own `MaxResults` ceiling (`@range(min: 1, max: 15)`), and
//!   following the `NextToken` it issues reports every workload exactly once; a token this
//!   service did not issue is refused;
//! * "for PDF, Base64String is returned. For JSON, Metrics is returned"
//!   (`GetConsolidatedReportInput$Format` doc): the two formats report the same page, one
//!   as metrics and one as a PDF document that says what those metrics say;
//! * a workload another account owns is reported only when the request asks for shared
//!   resources and this account accepted the share of it;
//! * a report is a READ: two reports of unchanged state are identical, and nothing a report
//!   is taken of moves.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. The HTTP client and the request builders are the shared
//! `super::workload_harness`.

use serde_json::{json, Value};

use super::workload_harness::{
    created_id, encode_path_segment, framework_answer, framework_answer_that_does_not_apply,
    framework_choice_ids, minimal_create, official_lens_arn, Answer, Client, InPlace,
    SIGNED_REGION,
};

/// The alias of the AWS Well-Architected Framework lens, and a second AWS-official lens
/// this emulator's catalog publishes content for, so a row can apply more than one lens.
const FRAMEWORK_LENS: &str = "wellarchitected";
const SERVERLESS_LENS: &str = "serverless";

/// The `Risk` members the enum publishes (`smithy_query(query="shape", shape="Risk")`),
/// which is what every roll-up in the report carries a bucket for.
const RISKS: [&str; 5] = ["HIGH", "MEDIUM", "NONE", "NOT_APPLICABLE", "UNANSWERED"];

/// The one `MetricType` the enum publishes (`smithy_query(query="shape",
/// shape="MetricType")`), which every metric of this report carries.
const METRIC_TYPE: &str = "WORKLOAD";

/// The ceiling this operation's `MaxResults` member publishes: `@range(min: 1, max: 15)`
/// (`smithy_query(query="operation", operation="GetConsolidatedReport")`).
const MAX_RESULTS_CEILING: usize = 15;

/// An account id that is not the one this emulator attributes every request to, so a
/// workload handed to it is a workload the caller does not own.
const FOREIGN_ACCOUNT: &str = "210987654321";

/// `GetConsolidatedReport`: `GET /consolidatedReport`, with every member of the request
/// bound to the query string.
async fn get_report(
    client: &Client,
    format: &str,
    max_results: Option<usize>,
    next_token: Option<&str>,
    include_shared_resources: Option<bool>,
) -> Answer {
    let mut query = vec![format!("Format={}", encode_path_segment(format))];
    if let Some(max_results) = max_results {
        query.push(format!("MaxResults={max_results}"));
    }
    if let Some(token) = next_token {
        query.push(format!("NextToken={}", encode_path_segment(token)));
    }
    if let Some(include) = include_shared_resources {
        query.push(format!("IncludeSharedResources={include}"));
    }
    client
        .send("GET", &format!("/consolidatedReport?{}", query.join("&")), None)
        .await
}

/// The `Metrics` of a successful JSON report, with the status checked.
fn metrics_of(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected 200, got {answer:?}"
    );
    answer
        .body
        .get("Metrics")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Metrics must be present, got {answer:?}"))
}

/// One string member of a JSON object, `None` when it carries none.
fn text<'a>(owner: &'a Value, name: &str) -> Option<&'a str> {
    owner.get(name).and_then(Value::as_str)
}

/// One `RiskCounts` map as the bucket table the enum publishes: every `Risk`, so a bucket
/// the response omits is read as missing rather than skipped.
fn risk_counts(owner: &Value, case: &str) -> Vec<(String, i64)> {
    let counts = owner.get("RiskCounts").and_then(Value::as_object);
    RISKS
        .iter()
        .map(|risk| {
            let count = counts
                .and_then(|counts| counts.get(*risk))
                .and_then(Value::as_i64)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: RiskCounts must carry {risk}, got {owner:?}")
                });
            ((*risk).to_string(), count)
        })
        .collect()
}

/// A workload applying `lenses`, and its `WorkloadId`.
async fn workload_with_lenses(client: &Client, name: &str, lenses: &[&str]) -> String {
    let mut body = minimal_create(name, &format!("tok-{name}"));
    body["Lenses"] = json!(lenses);
    created_id(&client.create(body).await, name)
}

/// `GetWorkload`, which reports the identity and the roll-up a metric of that workload
/// has to agree with.
async fn workload_of(client: &Client, workload_id: &str, case: &str) -> Value {
    let answer = client.get(workload_id).await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetWorkload 200, got {answer:?}"
    );
    answer
        .body
        .get("Workload")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: Workload must be present, got {answer:?}"))
}

/// `GetLensReview`, the read of the review a `LensMetric` is a metric of.
async fn lens_review_of(client: &Client, workload_id: &str, lens: &str, case: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}",
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected GetLensReview 200, got {answer:?}"
    );
    answer
        .body
        .get("LensReview")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: LensReview must be present, got {answer:?}"))
}

/// `GetAnswer`, the read that says which pillar a question belongs to and what its choices
/// are titled — so neither has to be restated here.
async fn answer_of(client: &Client, workload_id: &str, lens: &str, question_id: &str) -> Value {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/answers/{}",
                encode_path_segment(lens),
                encode_path_segment(question_id)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "GetAnswer for {question_id} must be 200, got {answer:?}"
    );
    answer
        .body
        .get("Answer")
        .cloned()
        .unwrap_or_else(|| panic!("GetAnswer for {question_id} must report an Answer"))
}

/// The improvement plan of one lens review, which selects the questions a review
/// identified a risk in — the same selection the report's `Questions` makes.
async fn improvement_question_ids(client: &Client, workload_id: &str, lens: &str) -> Vec<String> {
    let answer = client
        .send(
            "GET",
            &format!(
                "/workloads/{workload_id}/lensReviews/{}/improvements?MaxResults=100",
                encode_path_segment(lens)
            ),
            None,
        )
        .await;
    assert_eq!(
        answer.status, 200,
        "ListLensReviewImprovements must be 200, got {answer:?}"
    );
    answer
        .body
        .get("ImprovementSummaries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|summary| text(summary, "QuestionId").map(ToString::to_string))
        .collect()
}

/// The metric of one workload in a page, by `WorkloadId`.
fn metric_of<'page>(metrics: &'page [Value], workload_id: &str, case: &str) -> &'page Value {
    metrics
        .iter()
        .find(|metric| text(metric, "WorkloadId") == Some(workload_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: the report must carry a metric for {workload_id}")
        })
}

/// The `LensMetric` of one lens of a metric, by `LensArn`.
fn lens_metric_of<'metric>(metric: &'metric Value, lens_arn: &str, case: &str) -> &'metric Value {
    metric
        .get("Lenses")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a metric must carry Lenses, got {metric:?}"))
        .iter()
        .find(|lens| text(lens, "LensArn") == Some(lens_arn))
        .unwrap_or_else(|| panic!("case {case:?}: the metric must carry a lens metric for {lens_arn}"))
}

/// The `PillarMetric`s of a lens metric, in the order the report reports them.
fn pillar_metrics(lens: &Value, case: &str) -> Vec<Value> {
    lens.get("Pillars")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: a lens metric must carry Pillars, got {lens:?}"))
}

/// Every `QuestionMetric` of a lens metric, in the order the report reports them, each
/// beside the `PillarId` it was reported under.
fn question_metrics(lens: &Value, case: &str) -> Vec<(String, Value)> {
    pillar_metrics(lens, case)
        .iter()
        .flat_map(|pillar| {
            let pillar_id = text(pillar, "PillarId").unwrap_or_default().to_string();
            pillar
                .get("Questions")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(move |question| (pillar_id.clone(), question))
        })
        .collect()
}

/// One row of the derivation table: a workload to create, the lenses it applies and the
/// questions of its framework review that carry a stored answer.
struct Row {
    case: &'static str,
    name: &'static str,
    lenses: &'static [&'static str],
    answers: &'static [(&'static str, InPlace)],
}

/// Rule: every metric of a reported workload is DERIVED from that workload's stored state,
/// so it says exactly what the reads of that workload and of its lens reviews say.
///
/// The rows differ along what the rule generalises over: how many lenses the workload
/// applies (one and two), and whether its reviews carry answers (none, one, and several at
/// different risks).
#[tokio::test]
async fn every_metric_is_derived_from_the_workload_it_reports() {
    let client = Client::new();

    let rows: [Row; 3] = [
        Row {
            case: "one-lens-unanswered",
            name: "report-one-lens",
            lenses: &[FRAMEWORK_LENS],
            answers: &[],
        },
        Row {
            case: "two-lenses-answered",
            name: "report-two-lenses",
            lenses: &[FRAMEWORK_LENS, SERVERLESS_LENS],
            answers: &[
                ("permissions", InPlace::AllButLast),
                (
                    "organization-structure",
                    InPlace::NothingAndTheLastDoesNotApply,
                ),
                ("prevent-interaction-failure", InPlace::Every),
            ],
        },
        Row {
            case: "one-lens-fully-answered-question",
            name: "report-answered",
            lenses: &[FRAMEWORK_LENS],
            answers: &[("permissions", InPlace::Every)],
        },
    ];

    let mut workload_ids = Vec::new();
    for row in &rows {
        let workload_id = workload_with_lenses(&client, row.name, row.lenses).await;
        for (question_id, in_place) in row.answers {
            client.record_answer(
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                framework_answer(question_id, *in_place),
            );
        }
        workload_ids.push((row.case, workload_id, row.lenses));
    }

    let answer = get_report(&client, "JSON", None, None, None).await;
    let metrics = metrics_of(&answer, "derivation");
    assert_eq!(
        metrics.len(),
        workload_ids.len(),
        "case \"derivation\": the report must cover every workload of the account, got {metrics:?}"
    );

    for (case, workload_id, lenses) in &workload_ids {
        let metric = metric_of(&metrics, workload_id, case);
        let workload = workload_of(&client, workload_id, case).await;

        assert_eq!(
            text(metric, "MetricType"),
            Some(METRIC_TYPE),
            "case {case:?}: a metric of a workload carries the WORKLOAD metric type, got {metric:?}"
        );
        for member in ["WorkloadName", "WorkloadArn"] {
            assert_eq!(
                text(metric, member),
                text(&workload, member),
                "case {case:?}: {member} must be the workload's own, got {metric:?}"
            );
        }
        assert_eq!(
            metric.get("UpdatedAt"),
            workload.get("UpdatedAt"),
            "case {case:?}: UpdatedAt must be the instant the workload last changed, got {metric:?}"
        );
        assert_eq!(
            metric.get("LensesAppliedCount").and_then(Value::as_i64),
            Some(lenses.len() as i64),
            "case {case:?}: LensesAppliedCount must count the lenses applied, got {metric:?}"
        );
        assert_eq!(
            metric
                .get("Lenses")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(lenses.len()),
            "case {case:?}: the metric must report one lens metric per applied lens, got {metric:?}"
        );
        assert_eq!(
            risk_counts(metric, case),
            risk_counts(&workload, case),
            "case {case:?}: the workload roll-up must be the one the workload reports"
        );

        for lens in *lenses {
            let lens_arn = official_lens_arn(SIGNED_REGION, lens);
            let lens_metric = lens_metric_of(metric, &lens_arn, case);
            let review = lens_review_of(&client, workload_id, lens, case).await;
            assert_eq!(
                risk_counts(lens_metric, case),
                risk_counts(&review, case),
                "case {case:?}: the {lens} roll-up must be the one its review reports"
            );

            let summaries = review
                .get("PillarReviewSummaries")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            let reported: Vec<String> = pillar_metrics(lens_metric, case)
                .iter()
                .map(|pillar| text(pillar, "PillarId").unwrap_or_default().to_string())
                .collect();
            let expected: Vec<String> = summaries
                .iter()
                .map(|summary| text(summary, "PillarId").unwrap_or_default().to_string())
                .collect();
            assert_eq!(
                reported, expected,
                "case {case:?}: the {lens} metric must report the pillars its review reports"
            );
            for summary in &summaries {
                let pillar_id = text(summary, "PillarId").unwrap_or_default();
                let pillar = pillar_metrics(lens_metric, case)
                    .into_iter()
                    .find(|pillar| text(pillar, "PillarId") == Some(pillar_id))
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: the {lens} metric must carry pillar {pillar_id}")
                    });
                assert_eq!(
                    risk_counts(&pillar, case),
                    risk_counts(summary, case),
                    "case {case:?}: the {pillar_id} roll-up must be the one its review reports"
                );
            }
        }
    }

    // A report is a READ: taken twice over unchanged state it is the same report, and the
    // workload it is a report of has not moved.
    let again = get_report(&client, "JSON", None, None, None).await;
    assert_eq!(
        again.body, answer.body,
        "case \"read-twice\": two reports of unchanged state must be identical"
    );
    for (case, workload_id, _) in &workload_ids {
        let workload = workload_of(&client, workload_id, case).await;
        let metric = metric_of(&metrics, workload_id, case);
        assert_eq!(
            metric.get("UpdatedAt"),
            workload.get("UpdatedAt"),
            "case {case:?}: reporting a workload must not move it"
        );
    }
}

/// Rule: `Questions` reports the questions the review IDENTIFIED a risk in, and each one's
/// `BestPractices` are the choices its answer leaves out.
///
/// The rows differ along what the rule generalises over: the `Risk` the stored answer
/// leaves — every bucket the enum publishes, reached through a different shape of answer —
/// and how many of the question's own choices are left out.
#[tokio::test]
async fn only_questions_identified_as_risks_are_reported_with_the_best_practices_left_out() {
    let client = Client::new();
    let workload_id = workload_with_lenses(&client, "report-risks", &[FRAMEWORK_LENS]).await;

    // (case, question, the answer recorded, the risk the report must show, how many of the
    // question's choices the answer leaves out)
    enum Recorded {
        /// A seeded answer over the question's own choices.
        Answered(InPlace),
        /// A seeded answer saying the question does not apply.
        DoesNotApply,
        /// No stored answer at all.
        Nothing,
    }
    let rows: [(&str, &str, Recorded, Option<&str>); 6] = [
        (
            "no-best-practice-in-place",
            "organization-structure",
            Recorded::Answered(InPlace::NothingAndTheLastDoesNotApply),
            Some("HIGH"),
        ),
        (
            "one-best-practice-left-out",
            "permissions",
            Recorded::Answered(InPlace::AllButLast),
            Some("MEDIUM"),
        ),
        (
            "every-best-practice-in-place",
            "prevent-interaction-failure",
            Recorded::Answered(InPlace::Every),
            None,
        ),
        (
            "nothing-answered",
            "test-reliability",
            Recorded::Answered(InPlace::Nothing),
            None,
        ),
        (
            "question-does-not-apply",
            "resource-type-size-number",
            Recorded::DoesNotApply,
            None,
        ),
        ("no-stored-answer", "software-architecture", Recorded::Nothing, None),
    ];

    for (_, question_id, recorded, _) in &rows {
        match recorded {
            Recorded::Answered(in_place) => client.record_answer(
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                framework_answer(question_id, *in_place),
            ),
            Recorded::DoesNotApply => client.record_answer(
                &workload_id,
                FRAMEWORK_LENS,
                question_id,
                framework_answer_that_does_not_apply(question_id, "OUT_OF_SCOPE"),
            ),
            Recorded::Nothing => {}
        }
    }

    let answer = get_report(&client, "JSON", None, None, None).await;
    let metrics = metrics_of(&answer, "risks");
    let metric = metric_of(&metrics, &workload_id, "risks");
    let lens_metric = lens_metric_of(
        metric,
        &official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS),
        "risks",
    );
    let reported = question_metrics(lens_metric, "risks");

    for (case, question_id, _, risk) in &rows {
        let found = reported
            .iter()
            .find(|(_, question)| text(question, "QuestionId") == Some(*question_id));
        match risk {
            None => {
                assert!(
                    found.is_none(),
                    "case {case:?}: {question_id} leaves no identified risk, so no question \
                     metric reports it, got {found:?}"
                );
                continue;
            }
            Some(risk) => {
                let (pillar_id, question) = found.unwrap_or_else(|| {
                    panic!("case {case:?}: {question_id} must be reported at {risk}")
                });
                assert_eq!(
                    text(question, "Risk"),
                    Some(*risk),
                    "case {case:?}: expected {risk}, got {question:?}"
                );

                // The pillar it is reported under is the question's own, as the read of that
                // answer reports it.
                let stored = answer_of(&client, &workload_id, FRAMEWORK_LENS, question_id).await;
                assert_eq!(
                    Some(pillar_id.as_str()),
                    text(&stored, "PillarId"),
                    "case {case:?}: {question_id} must be reported under its own pillar"
                );

                // The best practices contributing to the risk: the choices the answer does
                // NOT record as selected, with the titles the question's choice catalog
                // gives them.
                let choice_ids = framework_choice_ids(question_id);
                let selected: Vec<String> = stored
                    .get("SelectedChoices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|choice| choice.as_str().map(ToString::to_string))
                    .collect();
                let applicable: Vec<String> = stored
                    .get("ChoiceAnswers")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter(|choice| text(choice, "Status") == Some("UNSELECTED"))
                    .filter_map(|choice| text(choice, "ChoiceId").map(ToString::to_string))
                    .collect();
                let expected: Vec<String> = choice_ids
                    .iter()
                    .filter(|choice_id| applicable.contains(choice_id))
                    .cloned()
                    .collect();
                let actual: Vec<String> = question
                    .get("BestPractices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|practice| {
                        text(practice, "ChoiceId").map(ToString::to_string)
                    })
                    .collect();
                assert_eq!(
                    actual, expected,
                    "case {case:?}: BestPractices must be the choices the answer leaves out, \
                     selected {selected:?}"
                );
                let titles: Vec<(String, String)> = stored
                    .get("Choices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                    .iter()
                    .filter_map(|choice| {
                        Some((
                            text(choice, "ChoiceId")?.to_string(),
                            text(choice, "Title")?.to_string(),
                        ))
                    })
                    .collect();
                for practice in question
                    .get("BestPractices")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default()
                {
                    let choice_id = text(&practice, "ChoiceId").unwrap_or_default().to_string();
                    let title = titles
                        .iter()
                        .find(|(id, _)| *id == choice_id)
                        .map(|(_, title)| title.clone());
                    assert_eq!(
                        text(&practice, "ChoiceTitle").map(ToString::to_string),
                        title,
                        "case {case:?}: a best practice must carry the title its choice \
                         catalog gives it, got {practice:?}"
                    );
                }
            }
        }
    }

    // The selection is the one the improvement plan of the same review makes: both report
    // the questions the review identified a risk in.
    let mut planned = improvement_question_ids(&client, &workload_id, FRAMEWORK_LENS).await;
    let mut from_report: Vec<String> = reported
        .iter()
        .filter_map(|(_, question)| text(question, "QuestionId").map(ToString::to_string))
        .collect();
    planned.sort();
    from_report.sort();
    assert_eq!(
        from_report, planned,
        "case \"plan-agreement\": the report and the improvement plan must select the same \
         questions"
    );
}

/// Rule: the report is paged by its own `MaxResults` ceiling, and following the token it
/// issues reports every covered workload exactly once.
///
/// The rows differ along the page size asked for: one, two, the published ceiling, and a
/// request that omits the member (which asks for a full page of that ceiling).
#[tokio::test]
async fn a_page_holds_the_requested_max_results_or_the_published_ceiling() {
    let client = Client::new();
    // More workloads than the ceiling, so the row that omits `MaxResults` tells the
    // published ceiling from any other default. Seeded through the shared state, as the
    // other paging suites of this crate do; every request under test still crosses HTTP.
    let total = MAX_RESULTS_CEILING + 3;
    client.seed_workloads(total);

    for (case, requested) in [
        ("one-at-a-time", Some(1)),
        ("two-at-a-time", Some(2)),
        ("at-the-ceiling", Some(MAX_RESULTS_CEILING)),
        ("no-max-results", None),
    ] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let answer = get_report(&client, "JSON", requested, token.as_deref(), None).await;
            let metrics = metrics_of(&answer, case);
            let expected = requested
                .unwrap_or(MAX_RESULTS_CEILING)
                .min(total - seen.len());
            assert_eq!(
                metrics.len(),
                expected,
                "case {case:?}: a page holds the requested size or the remainder, got \
                 {metrics:?}"
            );
            seen.extend(
                metrics
                    .iter()
                    .filter_map(|metric| text(metric, "WorkloadId").map(ToString::to_string)),
            );
            pages += 1;
            assert!(
                pages <= total + 1,
                "case {case:?}: the enumeration must terminate, got {pages} pages"
            );
            match text(&answer.body, "NextToken") {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        let mut unique = seen.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(
            unique.len(),
            total,
            "case {case:?}: every workload must be reported exactly once, got {} ids",
            seen.len()
        );
        assert_eq!(
            seen.len(),
            total,
            "case {case:?}: no workload may be reported twice, got {seen:?}"
        );
    }

    // A `NextToken` this service did not issue is a bad request naming the member, not an
    // empty page: the token is the service's own opaque cursor.
    for (case, token) in [
        ("not-a-token", "not-a-token"),
        ("hex-of-another-cursor", "7b2278223a2261227d"),
    ] {
        let answer = get_report(&client, "JSON", Some(1), Some(token), None).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_code, "ValidationException",
            "case {case:?}: expected ValidationException, got {answer:?}"
        );
        let fields = answer
            .body
            .get("Fields")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            fields
                .iter()
                .filter_map(|field| text(field, "Name"))
                .collect::<Vec<&str>>(),
            vec!["NextToken"],
            "case {case:?}: the finding must name the member that could not be decoded, got \
             {answer:?}"
        );
    }
}

/// Rule: the two formats report the SAME page — JSON as the metrics, PDF as a document
/// those metrics are set as — and neither fills the member the other format reports.
#[tokio::test]
async fn the_two_formats_report_the_same_page() {
    let client = Client::new();
    let workload_id = workload_with_lenses(&client, "report-formats", &[FRAMEWORK_LENS]).await;
    client.record_answer(
        &workload_id,
        FRAMEWORK_LENS,
        "permissions",
        framework_answer("permissions", InPlace::AllButLast),
    );

    let json = get_report(&client, "JSON", None, None, None).await;
    let metrics = metrics_of(&json, "json");
    assert!(
        json.body.get("Base64String").is_none(),
        "case \"json\": the JSON format returns Metrics, not a document, got {json:?}"
    );

    let pdf = get_report(&client, "PDF", None, None, None).await;
    assert_eq!(
        pdf.status, 200,
        "case \"pdf\": expected 200, got {pdf:?}"
    );
    assert!(
        pdf.body.get("Metrics").is_none(),
        "case \"pdf\": the PDF format returns Base64String, not Metrics, got {pdf:?}"
    );
    let encoded = text(&pdf.body, "Base64String")
        .unwrap_or_else(|| panic!("case \"pdf\": Base64String must be present, got {pdf:?}"));
    let bytes = aws_smithy_types::base64::decode(encoded)
        .unwrap_or_else(|error| panic!("case \"pdf\": Base64String must be base64 ({error})"));
    assert!(
        bytes.starts_with(b"%PDF-"),
        "case \"pdf\": the decoded report must be a PDF document, got {:?}",
        String::from_utf8_lossy(&bytes[..bytes.len().min(32)])
    );
    // The page's content stream is written uncompressed, so each shown line is in it as a
    // PDF literal string; the `\(` and `\)` escapes are undone so a line carrying
    // parentheses reads as it is shown.
    let shown = String::from_utf8_lossy(&bytes)
        .replace("\\(", "(")
        .replace("\\)", ")");
    assert!(
        shown.contains("%%EOF"),
        "case \"pdf\": the decoded report must carry the PDF end-of-file marker"
    );

    // The document SAYS what the metrics say: the workload it reports, and the roll-up of
    // that workload and of its lens review.
    let metric = metric_of(&metrics, &workload_id, "pdf");
    let workload_name = text(metric, "WorkloadName").unwrap_or_default();
    for line in [
        format!("Workload: {workload_name} ({workload_id})"),
        format!(
            "Workload ARN: {}",
            text(metric, "WorkloadArn").unwrap_or_default()
        ),
        format!("Metric type: {METRIC_TYPE}"),
        "Lenses applied: 1".to_string(),
    ] {
        assert!(
            shown.contains(&line),
            "case \"pdf\": the document must show {line:?}"
        );
    }
    for (risk, count) in risk_counts(metric, "pdf") {
        assert!(
            shown.contains(&format!("{risk}: {count}")),
            "case \"pdf\": the document must count {risk} as {count}"
        );
    }
    let lens_metric = lens_metric_of(
        metric,
        &official_lens_arn(SIGNED_REGION, FRAMEWORK_LENS),
        "pdf",
    );
    assert!(
        shown.contains(&format!(
            "Lens ARN: {}",
            text(lens_metric, "LensArn").unwrap_or_default()
        )),
        "case \"pdf\": the document must name the lens the metric reports"
    );
    for (_, question) in question_metrics(lens_metric, "pdf") {
        let question_id = text(&question, "QuestionId").unwrap_or_default();
        assert!(
            shown.contains(&format!("Question {question_id} at risk")),
            "case \"pdf\": the document must report the at-risk question {question_id}"
        );
    }

    // A report is a READ: the same request twice over unchanged state is byte for byte the
    // same document.
    let again = get_report(&client, "PDF", None, None, None).await;
    assert_eq!(
        text(&again.body, "Base64String"),
        Some(encoded),
        "case \"pdf-twice\": two documents of unchanged state must be identical"
    );
}

/// Rule: a workload another account owns is reported only when the request asks for shared
/// resources AND this account has accepted the share of it; the account's own workloads are
/// always reported.
///
/// The rows differ along who owns the workload and what state the share of it is in.
#[tokio::test]
async fn a_shared_workload_is_reported_only_when_the_request_asks_for_it() {
    let client = Client::new();
    let caller = engine::emulator_account_id();

    let own = workload_with_lenses(&client, "report-own", &[FRAMEWORK_LENS]).await;
    let accepted = workload_with_lenses(&client, "report-shared-in", &[FRAMEWORK_LENS]).await;
    let pending = workload_with_lenses(&client, "report-offered", &[FRAMEWORK_LENS]).await;
    // Hand the two shared rows to another account and record that account's share of each
    // with the caller. Neither has an HTTP path: `CreateWorkloadShare` refuses a
    // `SharedWith` naming the sharer, and this emulator attributes every request to its one
    // account, so the account that would share them in cannot sign a request. Only the SEED
    // reaches state; every request under test crosses the HTTP boundary.
    for workload_id in [&accepted, &pending] {
        let state = client.store();
        let mut store = state.lock();
        store
            .workloads
            .get_mut(workload_id)
            .expect("the workload under test must exist")
            .owner = FOREIGN_ACCOUNT.to_string();
    }
    client.record_workload_share(&accepted, caller, "READONLY", "ACCEPTED");
    client.record_workload_share(&pending, caller, "READONLY", "PENDING");

    // (case, the request's IncludeSharedResources, the workloads the report must cover)
    let rows: [(&str, Option<bool>, Vec<&String>); 3] = [
        ("omitted", None, vec![&own]),
        ("not-asked-for", Some(false), vec![&own]),
        ("asked-for", Some(true), vec![&own, &accepted]),
    ];

    for (case, include_shared_resources, expected) in rows {
        let answer = get_report(&client, "JSON", None, None, include_shared_resources).await;
        let metrics = metrics_of(&answer, case);
        let mut reported: Vec<String> = metrics
            .iter()
            .filter_map(|metric| text(metric, "WorkloadId").map(ToString::to_string))
            .collect();
        let mut wanted: Vec<String> = expected.iter().map(|id| (*id).clone()).collect();
        reported.sort();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: the report must cover exactly the workloads this account can see"
        );
        assert!(
            !reported.contains(&pending),
            "case {case:?}: a share still waiting on its invitation reports nothing, got \
             {reported:?}"
        );
    }
}
