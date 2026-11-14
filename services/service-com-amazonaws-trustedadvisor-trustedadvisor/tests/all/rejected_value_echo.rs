// SPDX-License-Identifier: CC0-1.0

//! The value half of a constraint-refusal clause, over values LARGER than the framework's
//! own 256-byte echo cap.
//!
//! A `ValidationException` message is built from the request, and the value between its
//! quotes is the only place the server reports the offending input back: a caller that
//! compares the echo against what it sent must see its own bytes. So the rule this suite
//! pins is one rule over every non-`@sensitive` constrained member of this service, at any
//! size the model lets a request carry:
//!
//! > a clause echoes the value the member HELD, as submitted, whatever its length.
//!
//! The rows span the members the rule reaches, where the value rides (a URI path label, a
//! query parameter, a structure member inside a body list), the constraint class broken
//! (`@pattern` and both ends of `@length`), and sizes from just past the echo cap up to —
//! and one past — each member's own modelled maximum. Constraints come from
//! `smithy_query(query="operation", …)` over the 12 operations this service binds and
//! `smithy_query(query="shape", shape="RecommendationResourceExclusion")`:
//! `ListRecommendationsForResource$awsResourceArn`, `$checkArn` and
//! `RecommendationResourceExclusion$arn` each declare `@length(20, 2048)` with a
//! `@pattern`, and `nextToken` declares `@length(4, 10000)`, so a value well past 256
//! bytes is INSIDE what the model admits and a clause about one still has to quote it.
//!
//! The ONE member excluded is `updateReason`, whose target shape the model marks
//! `@sensitive`: its value is never echoed at any size, which
//! `tests/all/validation_message_rendering.rs` pins (including over the cap).

use serde_json::{json, Value};
use service_com_amazonaws_trustedadvisor_trustedadvisor::state::SupportPlan;
use smithy_server_runtime::MultiServiceRouter;

use crate::recommendation_loop::{dispatch, emulator_on_plan, error_code, label};
use crate::validation_message_rendering::{
    assert_refused, message_of, one_error, AWS_RESOURCE_ARN_PATTERN, CHECK_PATTERN,
};

/// `RecommendationResourceArn`'s pattern as a clause reports it — the expression without
/// its anchors. Source
/// (`smithy_query(query="shape", shape="RecommendationResourceExclusion")`):
/// `^arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+$`,
/// `@length(20, 2048)`.
const RESOURCE_EXCLUSION_ARN_PATTERN: &str =
    r"arn:[\w-]+:trustedadvisor::\d{12}:recommendation-resource\/[\w-]+\/[\w-]+";

/// An `AwsResourceArn` the modelled pattern admits, so a row about `checkArn` is refused
/// for that member alone.
const VALID_RESOURCE_ARN: &str = "arn:aws:ec2:us-east-1:000000000000:instance/i-0abc1de23f4567890";

/// One row: what it is about, the request to make, and the clause it must be refused with.
type EchoRow = (String, &'static str, String, Option<Value>, String);

/// An emulator whose account's support plan includes this API, so a row is refused for the
/// reason it is about rather than by the support-level gate.
fn emulator() -> MultiServiceRouter {
    emulator_on_plan(SupportPlan::Business).0
}

/// `len` bytes of ASCII that no ARN pattern of this service admits: a repeating run with
/// its own position written into it, so the echo asserted is a value that could not be
/// matched by a prefix alone.
fn padded(len: usize) -> String {
    let mut value = String::with_capacity(len);
    while value.len() < len {
        value.push_str(&format!("{:02x}-not-an-arn-", value.len() % 251));
    }
    value.truncate(len);
    value
}

/// The clause a `@pattern` violation on `member` is refused with.
fn pattern_clause(member: &str, value: &str, pattern: &str) -> String {
    one_error(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
         regular expression pattern: {pattern}"
    ))
}

/// The clause a `@length` maximum violation on `member` is refused with.
fn max_length_clause(member: &str, value: &str, max: usize) -> String {
    one_error(&format!(
        "Value '{value}' at '{member}' failed to satisfy constraint: Member must have \
         length less than or equal to {max}"
    ))
}

/// A `checkArn` or `awsResourceArn` past the echo cap but INSIDE `@length(20, 2048)`: the
/// pattern is the only rule broken, and the clause quotes every byte that was sent.
///
/// The sizes span the cap's neighbourhood (257, the first size that is past it), a mid
/// value, and the member's own modelled maximum (2048) — the published bound, which a
/// request may still carry.
#[tokio::test]
async fn an_over_cap_value_inside_its_maximum_is_echoed_as_submitted() {
    let router = emulator();
    let mut rows: Vec<EchoRow> = Vec::new();
    for len in [257_usize, 300, 1024, 2048] {
        let value = padded(len);
        rows.push((
            format!("a {len}-byte checkArn, inside @length(20, 2048) and outside the pattern"),
            "GET",
            format!(
                "/v1/recommendations-for-resource/{}?checkArn={}",
                label(VALID_RESOURCE_ARN),
                label(&value),
            ),
            None,
            pattern_clause("checkArn", &value, CHECK_PATTERN),
        ));
        rows.push((
            format!("a {len}-byte awsResourceArn path label, inside @length(20, 2048)"),
            "GET",
            format!("/v1/recommendations-for-resource/{}", label(&value)),
            None,
            pattern_clause("awsResourceArn", &value, AWS_RESOURCE_ARN_PATTERN),
        ));
    }

    for (case, method, path, body, expected) in rows {
        assert_refused(&router, &case, method, &path, body, &expected).await;
    }
}

/// The same rule on a member reached INSIDE a body list, where the clause names the
/// element's position: the exclusion entry's `arn`, at the same sizes.
///
/// One row's value is multi-byte UTF-8, so the echo is quoted whole on a value whose BYTE
/// length is past the cap by more than its character count.
#[tokio::test]
async fn an_over_cap_value_inside_a_list_element_is_echoed_as_submitted() {
    let router = emulator();
    let member = "recommendationResourceExclusions.1.member.arn";

    let mut values: Vec<(String, String)> = Vec::new();
    for len in [257_usize, 300, 2048] {
        values.push((format!("a {len}-byte exclusion arn"), padded(len)));
    }
    // 200 two-byte characters: 400 bytes over the cap, 200 characters under it.
    values.push((
        "a 200-character exclusion arn of two-byte characters".to_string(),
        "é".repeat(200),
    ));

    for (case, value) in values {
        let body = json!({
            "recommendationResourceExclusions": [
                { "arn": value, "isExcluded": true },
            ],
        });
        let expected = pattern_clause(member, &value, RESOURCE_EXCLUSION_ARN_PATTERN);
        assert_refused(
            &router,
            &case,
            "PUT",
            "/v1/batch-update-recommendation-resource-exclusion",
            Some(body),
            &expected,
        )
        .await;
    }
}

/// A member whose modelled maximum is BELOW the echo cap reaches the same clause: the
/// value is past the maximum AND past the cap, and the clause still quotes it whole.
///
/// `awsService` declares `@length(2, 30)` with no pattern, so the maximum is the only rule
/// broken, and the rows run from just past the cap to far past it.
#[tokio::test]
async fn an_over_cap_value_past_a_small_maximum_is_echoed_as_submitted() {
    let router = emulator();
    let mut rows: Vec<EchoRow> = Vec::new();
    for len in [257_usize, 300, 1_000] {
        let value = padded(len);
        for (operation, path) in [
            ("ListChecks", "/v1/checks"),
            ("ListRecommendations", "/v1/recommendations"),
        ] {
            rows.push((
                format!("{operation} with a {len}-byte awsService, past @length(2, 30)"),
                "GET",
                format!("{path}?awsService={}", label(&value)),
                None,
                max_length_clause("awsService", &value, 30),
            ));
        }
    }
    // The other over-cap member: `nextToken`, `@length(4, 10000)`, one byte past its
    // published maximum.
    let token = padded(10_001);
    rows.push((
        "a nextToken one byte past @length(4, 10000)".to_string(),
        "GET",
        format!("/v1/checks?nextToken={}", label(&token)),
        None,
        max_length_clause("nextToken", &token, 10_000),
    ));

    for (case, method, path, body, expected) in rows {
        assert_refused(&router, &case, method, &path, body, &expected).await;
    }
}

/// A value past BOTH its `@length` maximum and its `@pattern` is one request with two
/// violations, and each of the two clauses quotes the whole value: the echo is a property
/// of the clause, not of which rule fired first.
#[tokio::test]
async fn every_clause_of_one_refusal_quotes_the_whole_value() {
    let router = emulator();
    // One byte past `checkArn`'s modelled maximum, so the maximum and the pattern are
    // both broken.
    let value = padded(2_049);
    let case = "a 2049-byte checkArn, past both @length(20, 2048) and the pattern";
    let (status, _errortype, response) = dispatch(
        &router,
        "GET",
        &format!(
            "/v1/recommendations-for-resource/{}?checkArn={}",
            label(VALID_RESOURCE_ARN),
            label(&value),
        ),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected __type ValidationException, got {response}"
    );
    let message = message_of(&response);
    assert!(
        message.starts_with("2 validation errors detected: "),
        "case {case:?}: expected the plural envelope over two clauses, got a message of \
         {} bytes starting {:?}",
        message.len(),
        &message[..message.len().min(80)]
    );
    let quoted = format!("Value '{value}' at 'checkArn' failed to satisfy constraint: ");
    let occurrences = message.matches(quoted.as_str()).count();
    assert_eq!(
        occurrences, 2,
        "case {case:?}: expected both clauses to quote the whole value, got {occurrences} \
         of them in a message of {} bytes",
        message.len()
    );
}

/// The bound on the echo: a value past the ceiling one clause may quote keeps the
/// framework's own shortened rendering, so a refusal body is never sized by the request.
///
/// 20 KiB is past every maximum this service's model declares (the largest is
/// `nextToken`'s 10000) and past the shared restoring walk's 16 KiB ceiling, so the clause
/// must be far shorter than the value it is about while still refusing the request.
#[tokio::test]
async fn a_value_past_the_stated_ceiling_is_refused_with_a_bounded_message() {
    let router = emulator();
    let value = padded(20 * 1024);
    let case = "a 20 KiB nextToken, past the ceiling one clause quotes";
    let (status, _errortype, response) = dispatch(
        &router,
        "GET",
        &format!("/v1/checks?nextToken={}", label(&value)),
        None,
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400, got {status}"
    );
    assert_eq!(
        error_code(&response),
        "ValidationException",
        "case {case:?}: expected __type ValidationException, got {error_code:?}",
        error_code = error_code(&response)
    );
    let message = message_of(&response);
    assert!(
        !message.contains(value.as_str()),
        "case {case:?}: expected the whole value NOT to be quoted, got a message of {} \
         bytes",
        message.len()
    );
    assert!(
        message.len() < value.len(),
        "case {case:?}: expected a message shorter than the {}-byte value, got {} bytes",
        value.len(),
        message.len()
    );
    assert!(
        message.contains("at 'nextToken' failed to satisfy constraint: Member must have \
                          length less than or equal to 10000"),
        "case {case:?}: expected the clause to name the member and the bound it broke, got \
         {message:?}"
    );
}
