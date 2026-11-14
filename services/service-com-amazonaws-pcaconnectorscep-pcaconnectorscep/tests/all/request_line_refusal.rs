// SPDX-License-Identifier: CC0-1.0

//! How this service refuses a value its request carried on the REQUEST LINE.
//!
//! The rule under test: a value bound OUTSIDE the request body — an `@httpLabel` path segment
//! or an `@httpQuery` parameter — is in no JSON body, so the body-schema validator that
//! reports a refused body (`model_pattern_refusal`) never sees it. Such a value is refused
//! HTTP 400 `ValidationException` in the standard validation envelope: `N validation
//! error(s) detected:` over one clause per violated member, each naming the member in
//! lowerCamel, the value the request sent and the requirement it broke — and a `@pattern`
//! requirement is quoted WITHOUT the anchors the model wraps the whole-value expression in.
//!
//! The same member name is bound both ways by different operations of this model
//! (`ConnectorArn` is `CreateChallenge`'s body member, `GetConnector`'s and
//! `DeleteConnector`'s path segment and `ListChallengeMetadata`'s query parameter —
//! smithy_query operation=CreateChallenge,GetConnector,DeleteConnector,ListChallengeMetadata),
//! so the LAST test here sends one malformed value both ways and pins that the two halves of
//! a request are worded differently.
//!
//! The rows differ along what the rule generalises over: which member carries the value
//! (`ConnectorArn`, `ChallengeArn`, `NextToken`), where on the request line it travels (a
//! path segment, a query parameter), which operation reads it, how the value fails (not an
//! ARN at all, a well-formed ARN of the wrong resource kind, a well-formed connector ARN with
//! a challenge segment hung off it, text that is not a page token), and — in the tests after
//! the table — which constraint class it broke (a `@pattern`, a `@length` maximum, a
//! `@range` minimum) and how many clauses one refusal carries. The `DeleteConnector` path
//! segment the issue was filed about is ONE row.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{self, label, new_router, Answer};

/// The Region these cases are signed for.
const REGION: &str = "eu-west-2";

/// The canonical emulator account id every ARN of this service carries.
const ACCOUNT: &str = "000000000000";

/// `ConnectorArn`'s published pattern, as the model declares it (smithy_query
/// shape=ConnectorArn: `@length` 5..200, this `@pattern`).
const CONNECTOR_ARN_PATTERN: &str = r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// The same expression as a refusal of a request-line value quotes it: the model's with its
/// leading `^` and trailing `$` removed. [`the_quoted_expression_is_the_modelled_one_unanchored`]
/// pins that relationship, so neither constant can drift from the other.
const STATED_CONNECTOR_ARN_PATTERN: &str = r"arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}";

/// `ChallengeArn`'s published pattern (smithy_query shape=ChallengeArn: `@length` 5..200).
const CHALLENGE_ARN_PATTERN: &str = r"^arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}$";

/// The same expression as a refusal quotes it.
const STATED_CHALLENGE_ARN_PATTERN: &str = r"arn:aws(-[a-z]+)*:pca-connector-scep:[a-z]+(-[a-z]+)+-[1-9]\d*:\d{12}:connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}";

/// `NextToken`'s published pattern (smithy_query shape=NextToken: `@length` 1..1000).
const NEXT_TOKEN_PATTERN: &str =
    r"^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$";

/// The same expression as a refusal quotes it.
const STATED_NEXT_TOKEN_PATTERN: &str =
    r"(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?";

/// The published upper bound of both ARN members' `@length(5, 200)`.
const MAX_ARN_LENGTH: usize = 200;

/// The published lower bound of `ListChallengeMetadata`'s and `ListConnectors`'s
/// `MaxResults` `@range(1, 1000)` (smithy_query operation=ListChallengeMetadata).
const MIN_MAX_RESULTS: i64 = 1;

/// A well-formed connector ARN, for the rows whose refusal is about another member.
fn connector_arn(connector_id: &str) -> String {
    format!("arn:aws:pca-connector-scep:{REGION}:{ACCOUNT}:connector/{connector_id}")
}

/// AWS's envelope over the clauses of one refusal.
fn envelope(clauses: &[String]) -> String {
    match clauses.len() {
        1 => format!("1 validation error detected: {}", clauses[0]),
        count => format!(
            "{count} validation errors detected: {}",
            clauses.join("; ")
        ),
    }
}

/// One clause: the value, the member it was sent for, and the requirement it broke.
fn clause(member: &str, value: &str, requirement: &str) -> String {
    format!("Value '{value}' at '{member}' failed to satisfy constraint: {requirement}")
}

/// The requirement a `@pattern` states, quoting the expression the way a request-line refusal
/// quotes it.
fn pattern_requirement(stated: &str) -> String {
    format!("Member must satisfy regular expression pattern: {stated}")
}

/// The whole message a request-line refusal of one patterned member carries.
fn pattern_message(member: &str, value: &str, stated_pattern: &str) -> String {
    envelope(&[clause(member, value, &pattern_requirement(stated_pattern))])
}

/// One row: which request-line value is malformed, the request that carries it, and the
/// member, value and expression its refusal states.
struct Row {
    name: &'static str,
    method: &'static str,
    path: String,
    member: &'static str,
    value: String,
    stated_pattern: &'static str,
}

fn rows() -> Vec<Row> {
    let not_an_arn = "not-an-arn";
    let good_connector = connector_arn("11111111-2222-3333-4444-555555555555");
    // A connector ARN with a challenge segment hung off it: well formed as a CHALLENGE ARN
    // and outside `ConnectorArn`'s own pattern, which is the shape the issue reports.
    let connector_plus_challenge = format!(
        "{}/challenge/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee",
        connector_arn("0e59d6d3-7bf8-490d-84dd-b86bb71c83b0")
    );
    // And the other way round: a connector ARN sent where a challenge ARN is required.
    let challenge_without_segment = connector_arn("ffffffff-eeee-dddd-cccc-bbbbbbbbbbbb");
    vec![
        Row {
            name: "DeleteConnector's ConnectorArn path segment, not an ARN",
            method: "DELETE",
            path: format!("/connectors/{}", label(not_an_arn)),
            member: "connectorArn",
            value: not_an_arn.to_string(),
            stated_pattern: STATED_CONNECTOR_ARN_PATTERN,
        },
        Row {
            name: "DeleteConnector's ConnectorArn path segment, with a challenge segment",
            method: "DELETE",
            path: format!("/connectors/{}", label(&connector_plus_challenge)),
            member: "connectorArn",
            value: connector_plus_challenge.clone(),
            stated_pattern: STATED_CONNECTOR_ARN_PATTERN,
        },
        Row {
            name: "GetConnector's ConnectorArn path segment",
            method: "GET",
            path: format!("/connectors/{}", label(&connector_plus_challenge)),
            member: "connectorArn",
            value: connector_plus_challenge,
            stated_pattern: STATED_CONNECTOR_ARN_PATTERN,
        },
        Row {
            name: "DeleteChallenge's ChallengeArn path segment",
            method: "DELETE",
            path: format!("/challenges/{}", label(&challenge_without_segment)),
            member: "challengeArn",
            value: challenge_without_segment.clone(),
            stated_pattern: STATED_CHALLENGE_ARN_PATTERN,
        },
        Row {
            name: "GetChallengeMetadata's ChallengeArn path segment",
            method: "GET",
            path: format!("/challengeMetadata/{}", label(not_an_arn)),
            member: "challengeArn",
            value: not_an_arn.to_string(),
            stated_pattern: STATED_CHALLENGE_ARN_PATTERN,
        },
        Row {
            name: "GetChallengePassword's ChallengeArn path segment",
            method: "GET",
            path: format!("/challengePasswords/{}", label(&challenge_without_segment)),
            member: "challengeArn",
            value: challenge_without_segment,
            stated_pattern: STATED_CHALLENGE_ARN_PATTERN,
        },
        Row {
            name: "ListConnectors's NextToken query parameter",
            method: "GET",
            path: format!("/connectors?NextToken={}", label("not a page token")),
            member: "nextToken",
            value: "not a page token".to_string(),
            stated_pattern: STATED_NEXT_TOKEN_PATTERN,
        },
        Row {
            name: "ListChallengeMetadata's NextToken query parameter",
            method: "GET",
            path: format!(
                "/challengeMetadata?ConnectorArn={}&NextToken={}",
                label(&good_connector),
                label("=====")
            ),
            member: "nextToken",
            value: "=====".to_string(),
            stated_pattern: STATED_NEXT_TOKEN_PATTERN,
        },
        Row {
            name: "ListChallengeMetadata's ConnectorArn query parameter",
            method: "GET",
            path: format!("/challengeMetadata?ConnectorArn={}", label(not_an_arn)),
            member: "connectorArn",
            value: not_an_arn.to_string(),
            stated_pattern: STATED_CONNECTOR_ARN_PATTERN,
        },
    ]
}

async fn send(router: &MultiServiceRouter, method: &str, path: &str, body: Option<Value>) -> Answer {
    harness::call(router, REGION, method, path, body).await
}

/// The `message` member a refusal of this service carries.
fn stated(answer: &Answer) -> &str {
    answer.body["message"].as_str().unwrap_or_default()
}

/// The two constants each expression is spelled as are the SAME expression: the model's, and
/// the model's with its two anchors removed. Pins the relationship the rule states, so a
/// quoted expression cannot be a third thing.
#[test]
fn the_quoted_expression_is_the_modelled_one_unanchored() {
    for (member, modelled, stated) in [
        ("ConnectorArn", CONNECTOR_ARN_PATTERN, STATED_CONNECTOR_ARN_PATTERN),
        ("ChallengeArn", CHALLENGE_ARN_PATTERN, STATED_CHALLENGE_ARN_PATTERN),
        ("NextToken", NEXT_TOKEN_PATTERN, STATED_NEXT_TOKEN_PATTERN),
    ] {
        assert_eq!(
            format!("^{stated}$"),
            modelled,
            "case {member:?}: the stated expression must be the modelled one without its \
             anchors, got {stated:?}"
        );
    }
}

/// Every member this model binds to a request line, in both positions one appears in, is
/// refused with the envelope the rule requires.
#[tokio::test]
async fn a_request_line_value_outside_its_pattern_is_refused_in_the_validation_envelope() {
    let (router, _engine) = new_router();
    for row in rows() {
        let case = row.name;
        let answer = send(&router, row.method, &row.path, None).await;
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} / {:?}",
            answer.status,
            answer.body
        );
        let message = stated(&answer);
        assert_eq!(
            message,
            pattern_message(row.member, &row.value, row.stated_pattern),
            "case {case:?}: expected the validation envelope naming {:?}, got {message:?}",
            row.member
        );
        // The two things this wording is NOT: the body-schema validator's findings, and the
        // anchors the model wraps the whole-value expression in.
        assert!(
            !message.contains("ECMA 262"),
            "case {case:?}: a request-line value is not reported by the body validator, got \
             {message:?}"
        );
        assert!(
            !message.contains("pattern: ^") && !message.ends_with('$'),
            "case {case:?}: the quoted expression carries no anchors, got {message:?}"
        );
    }
}

/// Every CONSTRAINT CLASS a request-line value can break is stated in that same envelope,
/// with the framework's own requirement for the class — and a value that breaks two carries
/// two clauses under the plural envelope.
#[tokio::test]
async fn every_constraint_class_a_request_line_value_breaks_is_stated_in_the_envelope() {
    let (router, _engine) = new_router();

    // A path segment one character past the published `@length` maximum, which is also
    // outside the pattern: two clauses, in the order the boundary walks them.
    let too_long = format!("arn:{}", "x".repeat(MAX_ARN_LENGTH - 3));
    assert_eq!(
        too_long.len(),
        MAX_ARN_LENGTH + 1,
        "the over-long case must be exactly one character past the published maximum"
    );
    let answer = send(
        &router,
        "DELETE",
        &format!("/connectors/{}", label(&too_long)),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "case \"a path segment past the published maximum length\": got {} / {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        stated(&answer),
        envelope(&[
            clause(
                "connectorArn",
                &too_long,
                &format!("Member must have length less than or equal to {MAX_ARN_LENGTH}"),
            ),
            clause(
                "connectorArn",
                &too_long,
                &pattern_requirement(STATED_CONNECTOR_ARN_PATTERN),
            ),
        ]),
        "case \"a path segment past the published maximum length\": got {:?}",
        stated(&answer)
    );

    // A value AT the published maximum, and outside the pattern: the one pattern clause.
    let at_max = format!("arn:{}", "x".repeat(MAX_ARN_LENGTH - 4));
    assert_eq!(at_max.len(), MAX_ARN_LENGTH, "the at-maximum case is at the bound");
    let answer = send(
        &router,
        "DELETE",
        &format!("/connectors/{}", label(&at_max)),
        None,
    )
    .await;
    assert_eq!(
        stated(&answer),
        pattern_message("connectorArn", &at_max, STATED_CONNECTOR_ARN_PATTERN),
        "case \"a path segment at the published maximum length\": got {:?}",
        stated(&answer)
    );

    // A query parameter below its published `@range` minimum: the range requirement, in the
    // same envelope.
    let below_minimum = MIN_MAX_RESULTS - 1;
    let answer = send(
        &router,
        "GET",
        &format!("/connectors?MaxResults={below_minimum}"),
        None,
    )
    .await;
    assert_eq!(
        (answer.status, answer.error_type.as_str()),
        (400, "ValidationException"),
        "case \"a page size below the published minimum\": got {} / {:?}",
        answer.status,
        answer.body
    );
    assert_eq!(
        stated(&answer),
        envelope(&[clause(
            "maxResults",
            &below_minimum.to_string(),
            &format!("Member must have value greater than or equal to {MIN_MAX_RESULTS}"),
        )]),
        "case \"a page size below the published minimum\": got {:?}",
        stated(&answer)
    );
}

/// The other side of the rule: a request line every one of whose values the model admits is
/// not refused at all — it is served, and answers out of this service's own state.
#[tokio::test]
async fn a_request_line_the_model_admits_is_served() {
    let (router, _engine) = new_router();
    let arn = connector_arn("99999999-8888-7777-6666-555544443333");
    // (case, method, path, the status the served operation answers)
    let rows: Vec<(&str, &str, String, u16)> = vec![
        (
            "GetConnector on a well-formed ARN holding no record",
            "GET",
            format!("/connectors/{}", label(&arn)),
            404,
        ),
        (
            "DeleteConnector on a well-formed ARN holding no record",
            "DELETE",
            format!("/connectors/{}", label(&arn)),
            202,
        ),
        (
            "DeleteChallenge on a well-formed ARN holding no record",
            "DELETE",
            format!(
                "/challenges/{}",
                label(&format!(
                    "{arn}/challenge/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
                ))
            ),
            202,
        ),
        (
            "ListConnectors inside its published page size",
            "GET",
            format!("/connectors?MaxResults={MIN_MAX_RESULTS}"),
            200,
        ),
    ];
    for (case, method, path, expected) in rows {
        let answer = send(&router, method, &path, None).await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected {expected}, got {} / {:?}",
            answer.status, answer.body
        );
        assert!(
            !stated(&answer).contains("validation error"),
            "case {case:?}: a value the model admits is no validation failure, got {:?}",
            stated(&answer)
        );
    }
}

/// The discriminating case: ONE malformed value, sent for ONE member name, is worded by the
/// layer that READ it — the validation envelope when it travelled on the request line, the
/// body validator's bracketed finding when it travelled in the body.
#[tokio::test]
async fn one_member_name_is_worded_by_the_half_of_the_request_it_travelled_in() {
    let (router, _engine) = new_router();
    let malformed = "not-an-arn";

    let on_the_request_line = send(
        &router,
        "DELETE",
        &format!("/connectors/{}", label(malformed)),
        None,
    )
    .await;
    assert_eq!(
        stated(&on_the_request_line),
        pattern_message("connectorArn", malformed, STATED_CONNECTOR_ARN_PATTERN),
        "case \"connectorArn as a path segment\": got {:?}",
        stated(&on_the_request_line)
    );

    let in_the_body = send(
        &router,
        "POST",
        "/challenges",
        Some(json!({ "ConnectorArn": malformed })),
    )
    .await;
    assert_eq!(
        stated(&in_the_body),
        format!(
            "[ECMA 262 regex \"{CONNECTOR_ARN_PATTERN}\" does not match input string \
             \"{malformed}\"]"
        ),
        "case \"connectorArn as a body member\": got {:?}",
        stated(&in_the_body)
    );

    // Both are the same status and the same error code: only the words differ.
    for (case, answer) in [
        ("a path segment", &on_the_request_line),
        ("a body member", &in_the_body),
    ] {
        assert_eq!(
            (answer.status, answer.error_type.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {} / {:?}",
            answer.status,
            answer.body
        );
    }
}
