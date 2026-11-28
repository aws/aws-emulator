// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for `PutAgentRecommendationFeedback`: the write that "submits
//! user feedback on a recommendation to help improve future optimization suggestions and
//! track implementation outcomes".
//!
//! The rules pinned here, each over a TABLE of inputs rather than one request:
//!
//! 1. A submission is recorded as the feedback sub-resource of the recommendation its
//!    `recommendationArn` names — the type it judges the finding with, the category it
//!    classifies the judgement under and the comments it adds, attributed to the caller and
//!    to the instant it was accepted — and answers an empty 200.
//! 2. The whole sub-resource is PUT: an optional member a submission omits is absent in
//!    what is held rather than inherited from an earlier submission, and a later
//!    submission replaces the one that stood.
//! 3. `@idempotent`: resubmitting the feedback a recommendation already holds leaves it
//!    exactly as it stood, the instant it was accepted at included.
//! 4. Feedback judges the finding rather than modifying it: every operation that reports
//!    the recommendation answers after a submission exactly as it did before.
//! 5. A request the service cannot serve changes nothing: a `recommendationArn` that names
//!    no recommendation is `404 ResourceNotFoundException` carrying the ARN as sent and the
//!    missing resource's type, and a member outside its modelled enum, length bounds or
//!    pattern — or a missing `@required` member — is `400 ValidationException`.
//!
//! Requests go through the real `MultiServiceRouter`, so routing, restJson1
//! (de)serialization, boundary constraint validation and the handler all run as they do in
//! production. Nothing in this API reports feedback back
//! (`PutAgentRecommendationFeedbackResponse` is an empty structure), so what was recorded
//! is read off the state the handlers share, through the same accessor they use; everything
//! about behaviour crosses the HTTP boundary.
//!
//! The HTTP client lives in `super::workload_harness`; the profile and the generation whose
//! completion produces the recommendations come from the suites beside this one.

use std::time::SystemTime;

use serde_json::{json, Value};

use service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::state;

use super::agent_goals::{member, profile_arn_of, PILLARS};
use super::agent_recommendations::{
    recommendations_of, start_generation, text, until_complete, RECOMMENDATION_TYPES,
};
use super::workload_harness::{encode_path_segment, Answer, Client};

/// Every value the `RecommendationFeedbackType` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="RecommendationFeedbackType")`). Spelled out here so
/// the suite pins the catalog rather than reading it back out of the code under test.
const FEEDBACK_TYPES: [&str; 2] = ["USEFUL", "NOT_USEFUL"];

/// Every value the `FeedbackCategory` enum publishes, in the model's order
/// (`smithy_query(query="shape", shape="FeedbackCategory")`). Spelled out here for the
/// reason [`FEEDBACK_TYPES`] gives.
const FEEDBACK_CATEGORIES: [&str; 5] = [
    "OTHER",
    "RECOMMENDATION_NOT_RELEVANT",
    "RESOURCE_NOT_IMPORTANT",
    "RESOURCE_TYPE_NOT_IMPORTANT",
    "RECOMMENDATION_INCORRECT",
];

/// The modelled bounds on the comments a submission may carry
/// (`smithy_query(query="operation", operation="PutAgentRecommendationFeedback")`:
/// `comments` `@length(1, 4096)`).
const COMMENTS_BOUNDS: (usize, usize) = (1, 4096);

/// `PUT /api/v1/agent-recommendations/{recommendationArn}/feedback`. The ARN is a
/// non-greedy `@httpLabel`, so it travels percent-encoded in one path segment, exactly as
/// an SDK sends it.
async fn put_feedback(client: &Client, recommendation_arn: &str, body: Value) -> Answer {
    client
        .send(
            "PUT",
            &format!(
                "/api/v1/agent-recommendations/{}/feedback",
                encode_path_segment(recommendation_arn)
            ),
            Some(body),
        )
        .await
}

/// `GET /api/v1/agent-recommendations/{recommendationArn}`: what a read of the
/// recommendation reports, for the rule that a submission changes none of it.
async fn get_recommendation(client: &Client, recommendation_arn: &str) -> Answer {
    client
        .send(
            "GET",
            &format!(
                "/api/v1/agent-recommendations/{}",
                encode_path_segment(recommendation_arn)
            ),
            None,
        )
        .await
}

/// The feedback the service holds for one recommendation, read through the accessor the
/// handlers themselves resolve a `RecommendationArn` by. The operation's response carries
/// no members, so this is the only place what was recorded can be observed.
fn held_feedback(
    client: &Client,
    recommendation_arn: &str,
    case: &str,
) -> Option<state::AgentRecommendationFeedback> {
    let store = client.store();
    let guard = store.lock();
    guard
        .agent_recommendation(recommendation_arn)
        .unwrap_or_else(|| {
            panic!("case {case:?}: the recommendation under test must still be stored")
        })
        .feedback
        .clone()
}

/// A profile whose one generation over every published pillar has completed, answered as
/// the `RecommendationArn` of each recommendation it produced, in the listing's order.
pub(crate) async fn generated_recommendations(client: &Client, name: &str) -> Vec<String> {
    let profile_arn = profile_arn_of(client, name, 1).await;
    let started = start_generation(client, &profile_arn, &PILLARS, RECOMMENDATION_TYPES[0]).await;
    assert_eq!(
        started.status, 200,
        "case {name:?}: expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;

    let listed = recommendations_of(client, &profile_arn, name).await;
    assert_eq!(
        listed.len(),
        PILLARS.len(),
        "case {name:?}: a generation scoping every pillar produces one recommendation per \
         pillar, got {listed:?}"
    );
    listed
        .iter()
        .map(|item| text(item, "recommendationArn"))
        .collect()
}

/// A well-formed `AgentRecommendationArn` (the modelled
/// `…:agent-recommendation/[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$`)
/// that no stored recommendation carries.
pub(crate) fn absent_recommendation_arn() -> String {
    format!(
        "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/\
         ffffffff-0000-4a00-8b00-0000000000fe",
        engine::emulator_account_id()
    )
}

/// One row of the table: the feedback one submission carries.
#[derive(Debug, Clone)]
struct Row {
    case: String,
    feedback_type: &'static str,
    category: Option<&'static str>,
    comments: Option<String>,
}

/// The rows: EVERY published `RecommendationFeedbackType` crossed with every published
/// `FeedbackCategory` and with none at all, so the table spans the whole value space of
/// both enum-typed members; the comments vary by SIZE across the rows — none, exactly the
/// modelled minimum, a middling value, exactly the modelled maximum — and one row carries
/// the line breaks the member's `@pattern` admits (`[\P{C}\r\n]`), so the class the
/// pattern spans is exercised and not only its length.
fn rows() -> Vec<Row> {
    let comments: [Option<String>; 5] = [
        None,
        Some("!".repeat(COMMENTS_BOUNDS.0)),
        Some("a middling comment about this finding".to_string()),
        Some("c".repeat(COMMENTS_BOUNDS.1)),
        Some("a comment\r\nover two lines".to_string()),
    ];
    let mut rows = Vec::new();
    for (type_index, feedback_type) in FEEDBACK_TYPES.iter().enumerate() {
        let categories = FEEDBACK_CATEGORIES
            .iter()
            .map(|category| Some(*category))
            .chain(std::iter::once(None));
        for (category_index, category) in categories.enumerate() {
            let comments = comments[(type_index + category_index) % comments.len()].clone();
            rows.push(Row {
                case: format!(
                    "{feedback_type}/{}/comments-{}",
                    category.unwrap_or("no-category"),
                    comments.as_ref().map_or(0, |value| value.chars().count())
                ),
                feedback_type,
                category,
                comments,
            });
        }
    }
    rows
}

/// The request body one row sends: an optional member the row does not carry is ABSENT from
/// the body rather than sent as null, exactly as an SDK omits it.
fn feedback_body(row: &Row) -> Value {
    let mut body = json!({ "type": row.feedback_type });
    if let Some(category) = row.category {
        body["feedbackCategory"] = json!(category);
    }
    if let Some(comments) = &row.comments {
        body["comments"] = json!(comments);
    }
    body
}

/// Assert a successful submission: the empty 200 the model declares.
fn assert_accepted(answer: &Answer, case: &str) {
    assert_eq!(
        answer.status, 200,
        "case {case:?}: expected PutAgentRecommendationFeedback 200, got {answer:?}"
    );
    assert!(
        answer
            .body
            .as_object()
            .is_none_or(|members| members.is_empty()),
        "case {case:?}: the response is modelled with no members, got {answer:?}"
    );
}

/// Assert the feedback held for `arn` is the one `row` submitted.
fn assert_holds(client: &Client, arn: &str, row: &Row) -> state::AgentRecommendationFeedback {
    let case = &row.case;
    let held = held_feedback(client, arn, case)
        .unwrap_or_else(|| panic!("case {case:?}: an accepted submission is held"));
    assert_eq!(
        (
            held.feedback_type.as_str(),
            held.category.as_deref(),
            held.comments.as_deref()
        ),
        (
            row.feedback_type,
            row.category,
            row.comments.as_deref()
        ),
        "case {case:?}: the feedback held is the feedback last submitted, got {held:?}"
    );
    held
}

/// Rule 1: a submission is recorded against the recommendation it names, with the identity
/// and instant the service attributes it to, and answers an empty 200. Rule 2's second
/// half: a later submission on the same recommendation replaces the one that stood — the
/// rows wrap around the recommendations, so every row past the first lap is a replacement.
#[tokio::test]
async fn a_submission_is_recorded_against_the_recommendation_it_names() {
    let client = Client::new();
    let arns = generated_recommendations(&client, "feedback-rows").await;

    for (index, row) in rows().iter().enumerate() {
        let arn = &arns[index % arns.len()];
        let case = &row.case;

        // Rule 1's other half: nothing has judged this finding until a submission does.
        if index < arns.len() {
            assert!(
                held_feedback(&client, arn, case).is_none(),
                "case {case:?}: a recommendation nobody has judged holds no feedback"
            );
        }

        let before = SystemTime::now();
        let answer = put_feedback(&client, arn, feedback_body(row)).await;
        let after = SystemTime::now();
        assert_accepted(&answer, case);

        let held = assert_holds(&client, arn, row);
        assert_eq!(
            held.submitted_by,
            engine::emulator_account_id(),
            "case {case:?}: a submission is attributed to the caller's account, got {held:?}"
        );
        assert!(
            held.submitted_at >= before && held.submitted_at <= after,
            "case {case:?}: a submission is attributed to the instant it was accepted \
             ({before:?}..={after:?}), got {:?}",
            held.submitted_at
        );
    }

    // Each recommendation holds what its LAST row submitted, and no row leaked onto
    // another recommendation.
    let rows = rows();
    for (index, arn) in arns.iter().enumerate() {
        let last = rows
            .iter()
            .enumerate()
            .rfind(|(row_index, _)| row_index % arns.len() == index)
            .map(|(_, row)| row)
            .unwrap_or_else(|| panic!("every recommendation took at least one row"));
        assert_holds(&client, arn, last);
    }
}

/// Rule 2's first half: the whole sub-resource is PUT, so an optional member a submission
/// omits is absent in what is held rather than inherited from the submission before it.
#[tokio::test]
async fn a_submission_omitting_an_optional_member_leaves_none_of_it_behind() {
    let client = Client::new();
    let arns = generated_recommendations(&client, "feedback-replacement").await;

    // Each pair is a FULL submission followed by one that drops some of it; the pairs
    // differ in which members are dropped, and in the type the replacement judges with.
    let pairs: Vec<(Row, Row)> = vec![
        (
            Row {
                case: "both-optional-members-then-neither".to_string(),
                feedback_type: FEEDBACK_TYPES[0],
                category: Some(FEEDBACK_CATEGORIES[0]),
                comments: Some("the finding helped".to_string()),
            },
            Row {
                case: "both-optional-members-then-neither (replacement)".to_string(),
                feedback_type: FEEDBACK_TYPES[1],
                category: None,
                comments: None,
            },
        ),
        (
            Row {
                case: "comments-dropped".to_string(),
                feedback_type: FEEDBACK_TYPES[1],
                category: Some(FEEDBACK_CATEGORIES[2]),
                comments: Some("c".repeat(COMMENTS_BOUNDS.1)),
            },
            Row {
                case: "comments-dropped (replacement)".to_string(),
                feedback_type: FEEDBACK_TYPES[1],
                category: Some(FEEDBACK_CATEGORIES[3]),
                comments: None,
            },
        ),
        (
            Row {
                case: "category-dropped".to_string(),
                feedback_type: FEEDBACK_TYPES[0],
                category: Some(FEEDBACK_CATEGORIES[4]),
                comments: Some("!".repeat(COMMENTS_BOUNDS.0)),
            },
            Row {
                case: "category-dropped (replacement)".to_string(),
                feedback_type: FEEDBACK_TYPES[0],
                category: None,
                comments: Some("a different comment entirely".to_string()),
            },
        ),
    ];

    for (index, (first, second)) in pairs.iter().enumerate() {
        let arn = &arns[index % arns.len()];

        let answer = put_feedback(&client, arn, feedback_body(first)).await;
        assert_accepted(&answer, &first.case);
        assert_holds(&client, arn, first);

        let answer = put_feedback(&client, arn, feedback_body(second)).await;
        assert_accepted(&answer, &second.case);
        assert_holds(&client, arn, second);
    }
}

/// Rule 3: `@idempotent` — resubmitting the feedback a recommendation already holds leaves
/// it exactly as it stood, the instant it was accepted at included, while feedback that
/// differs in ANY member replaces it.
#[tokio::test]
async fn resubmitting_the_feedback_already_held_leaves_it_as_it_stood() {
    let client = Client::new();
    let arns = generated_recommendations(&client, "feedback-idempotence").await;

    for (index, row) in rows().iter().take(arns.len()).enumerate() {
        let arn = &arns[index];
        let case = &row.case;

        let answer = put_feedback(&client, arn, feedback_body(row)).await;
        assert_accepted(&answer, case);
        let first = assert_holds(&client, arn, row);

        for repeat in 0..2 {
            let answer = put_feedback(&client, arn, feedback_body(row)).await;
            assert_accepted(&answer, case);
            let again = held_feedback(&client, arn, case)
                .unwrap_or_else(|| panic!("case {case:?}: the submission is still held"));
            assert_eq!(
                again, first,
                "case {case:?}: repeat {repeat} of the same submission must leave the \
                 feedback exactly as it stood"
            );
        }

        // One member different is a different submission, so it replaces what stood.
        let replacement = Row {
            case: format!("{case} (one member different)"),
            feedback_type: row.feedback_type,
            category: row.category,
            comments: Some(match &row.comments {
                None => "a comment where there was none".to_string(),
                Some(_) => "a comment that is not the one held".to_string(),
            }),
        };
        let answer = put_feedback(&client, arn, feedback_body(&replacement)).await;
        assert_accepted(&answer, &replacement.case);
        let replaced = assert_holds(&client, arn, &replacement);
        assert!(
            replaced.submitted_at >= first.submitted_at,
            "case {:?}: a replacement is attributed to its own instant, got {:?} before \
             {:?}",
            replacement.case,
            replaced.submitted_at,
            first.submitted_at
        );
    }
}

/// Rule 4: a submission judges the finding rather than modifying it, so every operation
/// that reports the recommendation answers exactly as it did before.
#[tokio::test]
async fn a_submission_changes_nothing_any_report_of_the_recommendation_carries() {
    let client = Client::new();
    let profile_arn = profile_arn_of(&client, "feedback-reports", 1).await;
    let started = start_generation(&client, &profile_arn, &PILLARS, RECOMMENDATION_TYPES[1]).await;
    assert_eq!(
        started.status, 200,
        "expected StartAgentRecommendationGeneration 200, got {started:?}"
    );
    tokio::time::sleep(until_complete()).await;

    let listed = recommendations_of(&client, &profile_arn, "feedback-reports").await;
    let arns: Vec<String> = listed
        .iter()
        .map(|item| text(item, "recommendationArn"))
        .collect();

    let mut reads: Vec<Value> = Vec::new();
    for arn in &arns {
        let answer = get_recommendation(&client, arn).await;
        assert_eq!(
            answer.status, 200,
            "case {arn:?}: expected GetAgentRecommendation 200, got {answer:?}"
        );
        reads.push(answer.body);
    }

    for (index, arn) in arns.iter().enumerate() {
        let row = &rows()[index % rows().len()];
        let answer = put_feedback(&client, arn, feedback_body(row)).await;
        assert_accepted(&answer, &row.case);

        let after = get_recommendation(&client, arn).await;
        assert_eq!(
            after.status, 200,
            "case {:?}: expected GetAgentRecommendation 200, got {after:?}",
            row.case
        );
        assert_eq!(
            after.body, reads[index],
            "case {:?}: a submission must not change what a read of the recommendation \
             reports",
            row.case
        );
    }

    let after = recommendations_of(&client, &profile_arn, "feedback-reports-after").await;
    assert_eq!(
        after, listed,
        "case \"listing\": a submission must not change what a listing of the \
         recommendations reports"
    );
}

/// Rule 5: a request the service cannot serve is refused, the refusal names what was looked
/// for, and nothing it named is changed.
#[tokio::test]
async fn a_request_the_service_cannot_serve_is_refused() {
    let client = Client::new();
    let arns = generated_recommendations(&client, "feedback-refusals").await;
    let arn = &arns[0];

    // A submission that stands, so each refusal below can be shown to leave it alone.
    let standing = &rows()[0];
    let answer = put_feedback(&client, arn, feedback_body(standing)).await;
    assert_accepted(&answer, &standing.case);
    let held = assert_holds(&client, arn, standing);

    // A well-formed `AgentRecommendationArn` no recommendation carries.
    let absent = absent_recommendation_arn();
    let answer = put_feedback(&client, &absent, json!({ "type": FEEDBACK_TYPES[0] })).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"absent recommendation\": expected a 404 ResourceNotFoundException, got \
         {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceId"),
        absent,
        "case \"absent recommendation\": the 404 names the ARN as sent, got {answer:?}"
    );
    assert_eq!(
        member(&answer, "ResourceType"),
        state::AGENT_RECOMMENDATION_RESOURCE_TYPE,
        "case \"absent recommendation\": the 404 names the missing resource's type, got \
         {answer:?}"
    );

    // Values no submission may carry: a member outside its published enum, comments
    // outside the modelled length bounds or carrying a character the pattern excludes, an
    // ARN the pattern refuses, and a missing `@required` member.
    let refusals: Vec<(&str, Answer)> = vec![
        (
            "a feedback type outside the published enum",
            put_feedback(&client, arn, json!({ "type": "SOMEWHAT_USEFUL" })).await,
        ),
        (
            "a feedback category outside the published enum",
            put_feedback(
                &client,
                arn,
                json!({ "type": FEEDBACK_TYPES[0], "feedbackCategory": "RESOURCE_TOO_SMALL" }),
            )
            .await,
        ),
        (
            "no type at all",
            put_feedback(&client, arn, json!({ "feedbackCategory": FEEDBACK_CATEGORIES[0] })).await,
        ),
        (
            "comments one short of the modelled minimum",
            put_feedback(
                &client,
                arn,
                json!({
                    "type": FEEDBACK_TYPES[0],
                    "comments": "!".repeat(COMMENTS_BOUNDS.0 - 1),
                }),
            )
            .await,
        ),
        (
            "comments one past the modelled maximum",
            put_feedback(
                &client,
                arn,
                json!({
                    "type": FEEDBACK_TYPES[0],
                    "comments": "c".repeat(COMMENTS_BOUNDS.1 + 1),
                }),
            )
            .await,
        ),
        (
            "comments carrying a character the pattern excludes",
            put_feedback(
                &client,
                arn,
                json!({ "type": FEEDBACK_TYPES[0], "comments": "a bell\u{7} in the comment" }),
            )
            .await,
        ),
        (
            "comments opening a variable reference the pattern forbids",
            put_feedback(
                &client,
                arn,
                json!({ "type": FEEDBACK_TYPES[0], "comments": "${aws:region} in the comment" }),
            )
            .await,
        ),
        (
            "a recommendation ARN the pattern refuses",
            put_feedback(
                &client,
                &format!(
                    "arn:aws:wellarchitected:us-west-2:{}:agent-recommendation/not-a-uuid",
                    engine::emulator_account_id()
                ),
                json!({ "type": FEEDBACK_TYPES[0] }),
            )
            .await,
        ),
    ];
    for (case, answer) in refusals {
        assert_eq!(
            (answer.status, answer.error_code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {answer:?}"
        );
    }

    // None of those refusals touched the feedback that stood.
    let after = held_feedback(&client, arn, "refusals-after")
        .unwrap_or_else(|| panic!("case \"refusals\": the accepted submission is still held"));
    assert_eq!(
        after, held,
        "case \"refusals\": a refused request must leave the feedback as it was"
    );

    // Nor did the 404 bring a recommendation into being for the ARN it named.
    let store = client.store();
    let guard = store.lock();
    assert!(
        guard.agent_recommendation(&absent).is_none(),
        "case \"absent recommendation\": a refused submission must create nothing"
    );
}
