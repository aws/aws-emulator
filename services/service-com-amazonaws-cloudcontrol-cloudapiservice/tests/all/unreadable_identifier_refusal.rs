// SPDX-License-Identifier: CC0-1.0

//! What a resource-level operation answers for a reference the resource type's READ HANDLER
//! cannot even submit to the owning service.
//!
//! The rule every case here holds the service to: **a read of a resource is the type's read
//! handler calling the owning service's own API with the reference the request carried, so a
//! reference that API will not accept at the member the handler submits it on is refused with
//! the verdict that handler fails with — `InvalidRequestException`, "the resource handler has
//! returned that invalid input from the user has generated a generic exception"
//! (`smithy_query(query="doc",shape="InvalidRequestException")`), carrying
//! `(HandlerErrorCode: InvalidRequest, …)` and the owning service's own refusal of the call —
//! while `ResourceNotFoundException` and `(HandlerErrorCode: NotFound, …)` are kept for a
//! reference that call ACCEPTED and reported nothing for.**
//!
//! The two classes have to differ because a client branches on them: a reference no resource
//! of the type could ever answer to is the caller's to fix, where an absent resource is one to
//! create or to stop polling for.
//!
//! The rows differ along what the rule generalises over: which part of the owning service's
//! domain the reference breaks (a character the class excludes, a length past the published
//! bound), which character it is, which resource type is read — so which member of which
//! downstream call carries the reference — and which of the two operations that resolve a
//! reference through that read answers. The controls are the references that call DOES take:
//! a legal name nothing created, a name of exactly the longest the shape admits, and a
//! resource that is really there.

use serde_json::{json, Value};

use super::emulator::{code, message, progress, Emulator};

/// A type whose read handler submits the reference as the prefix of `logs:DescribeLogGroups`.
const LOG_GROUP: &str = "AWS::Logs::LogGroup";

/// A type whose read handler submits the GROUP segment of its compound reference as the group
/// of `logs:DescribeLogStreams`.
const LOG_STREAM: &str = "AWS::Logs::LogStream";

/// The error code a reference the read handler could not submit is refused under.
const INVALID_REQUEST: &str = "InvalidRequestException";

/// The error code a reference that read ACCEPTED and reported nothing for is refused under.
const NOT_FOUND: &str = "ResourceNotFoundException";

/// The status this awsJson1_0 service answers both refusals with.
const REFUSAL_STATUS: u16 = 400;

/// The fixed text a handler verdict is bracketed by, around the diagnostic it carries.
const WRAPPER_OPENING: &str = " Handler returned status FAILED: ";

/// The most characters CloudWatch Logs holds a log group name of (`LogGroupName`,
/// `@length(min: 1, max: 512)`), which is the bound a reference one past is refused by.
const MAX_LOG_GROUP_NAME_CHARS: usize = 512;

/// The clause CloudWatch Logs refuses a name carrying a character its class excludes with, at
/// the member the call carried it on.
///
/// Source: the expression AWS publishes for `LogGroupName`, which the model's own
/// documentation of a log group name states ("Pattern: [\.\-_/#A-Za-z0-9]+", in the
/// `logGroupIdentifier` docs of `PutLogGroupDeletionProtection`).
fn class_clause(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {}",
        r"[\.\-_/#A-Za-z0-9]+"
    )
}

/// The same clause for the OTHER log-name shape, whose domain is wider: the only characters
/// `LogStreamName` excludes are the two CloudWatch Logs keeps for addressing and matching
/// (`smithy_query(query="shape",shape="LogStreamName")`, pattern `^[^:*]*$`, published
/// unanchored).
fn stream_class_clause(member: &str) -> String {
    format!(
        "1 validation error detected: Value at '{member}' failed to satisfy constraint: Member \
         must satisfy regular expression pattern: {}",
        r"[^:*]*"
    )
}

/// The sentence CloudWatch Logs refuses a name past the bound it holds both log names to
/// with, which names the pair and the bound rather than one member.
fn length_sentence() -> String {
    format!(
        "Invalid LogGroup or LogStream, both must have length less than or equal to \
         {MAX_LOG_GROUP_NAME_CHARS}."
    )
}

/// A patch that changes nothing: every case here is about the reference, not the patch.
fn inert_patch() -> Value {
    json!([{ "op": "add", "path": "/Tags", "value": [] }])
}

/// Check that `body` carries the handler verdict `expected_code`/`handler_code` with
/// `detail` as the diagnostic, under the status this service answers a refusal with.
fn assert_handler_verdict(
    status: u16,
    body: &Value,
    case: &str,
    type_name: &str,
    expected_code: &str,
    handler_code: &str,
    detail: &str,
) {
    assert_eq!(
        status, REFUSAL_STATUS,
        "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body:?}"
    );
    assert!(
        code(body).contains(expected_code),
        "case {case:?}: expected {expected_code}, got {:?}",
        code(body)
    );
    assert!(
        progress(body).is_null(),
        "case {case:?}: a refused request mints no resource operation, got {body:?}"
    );
    let stated = message(body);
    let opening = format!(
        "{type_name}{WRAPPER_OPENING}{detail} (HandlerErrorCode: {handler_code}, RequestToken: "
    );
    assert!(
        stated.starts_with(&opening) && stated.ends_with(')') && stated.len() > opening.len() + 1,
        "case {case:?}: expected a message opening {opening:?} and closing on the failing \
         invocation's token, got {stated:?}"
    );
}

#[tokio::test]
async fn a_reference_the_read_handler_cannot_submit_is_refused_as_invalid_input() {
    let emulator = Emulator::with_cloudwatch_logs();
    // (case, type, the reference, the owning service's own refusal of the call the handler
    // made)
    let rows: Vec<(&str, &str, String, String)> = vec![
        (
            "a group name carrying the colon the class excludes",
            LOG_GROUP,
            "orders-v1:colon-not-allowed".to_string(),
            class_clause("logGroupNamePrefix"),
        ),
        (
            "a group name carrying a space",
            LOG_GROUP,
            "orders v1".to_string(),
            class_clause("logGroupNamePrefix"),
        ),
        (
            "a group name carrying the wildcard the class excludes",
            LOG_GROUP,
            "orders*".to_string(),
            class_clause("logGroupNamePrefix"),
        ),
        (
            "a group name carrying a character outside ASCII",
            LOG_GROUP,
            "ordrés".to_string(),
            class_clause("logGroupNamePrefix"),
        ),
        (
            "a group name one character past the bound the shape publishes",
            LOG_GROUP,
            "g".repeat(MAX_LOG_GROUP_NAME_CHARS + 1),
            length_sentence(),
        ),
        (
            "a stream whose group segment carries a colon",
            LOG_STREAM,
            "orders-v1:colon-not-allowed|host-1".to_string(),
            class_clause("logGroupName"),
        ),
        (
            "a stream whose group segment is past that same bound",
            LOG_STREAM,
            format!("{}|host-1", "g".repeat(MAX_LOG_GROUP_NAME_CHARS + 1)),
            length_sentence(),
        ),
        (
            "a stream whose own name carries the colon its class excludes",
            LOG_STREAM,
            "absent-orders-v1|host:1".to_string(),
            stream_class_clause("logStreamNamePrefix"),
        ),
        (
            "a stream whose own name carries the wildcard its class excludes",
            LOG_STREAM,
            "absent-orders-v1|host*".to_string(),
            stream_class_clause("logStreamNamePrefix"),
        ),
        (
            "a stream whose own name is past the bound both log names share",
            LOG_STREAM,
            format!("absent-orders-v1|{}", "s".repeat(MAX_LOG_GROUP_NAME_CHARS + 1)),
            length_sentence(),
        ),
    ];

    for (row, type_name, identifier, detail) in rows {
        // Both operations that resolve a reference through the type's read handler answer
        // the refusal inline.
        let answers = [
            ("GetResource", emulator.get(type_name, &identifier).await),
            (
                "UpdateResource",
                emulator
                    .update(type_name, &identifier, &inert_patch(), None)
                    .await,
            ),
        ];
        for (operation, (status, body)) in answers {
            let case = format!("{row} ({operation})");
            assert_handler_verdict(
                status,
                &body,
                &case,
                type_name,
                INVALID_REQUEST,
                "InvalidRequest",
                &detail,
            );
        }
    }
}

#[tokio::test]
async fn a_reference_the_read_handler_submits_is_still_reported_absent() {
    let emulator = Emulator::with_cloudwatch_logs();
    // The control the rule turns on: every reference here is one the owning service's read
    // TAKES, so the verdict is the absence of the resource rather than a refusal of the
    // reference — including a name of exactly the longest the shape admits, one character
    // under the row refused above.
    let rows: Vec<(&str, &str, String)> = vec![
        (
            "a legal group name nothing created",
            LOG_GROUP,
            "absent-orders-v1".to_string(),
        ),
        (
            "a path shaped group name of the punctuation the class admits",
            LOG_GROUP,
            "/aws/absent.orders-v2_a/#b".to_string(),
        ),
        (
            "a group name of exactly the most the shape admits",
            LOG_GROUP,
            "g".repeat(MAX_LOG_GROUP_NAME_CHARS),
        ),
        (
            "a legal stream reference nothing created",
            LOG_STREAM,
            "absent-orders-v1|host-1".to_string(),
        ),
        (
            // The stream shape's domain is WIDER than the group shape's: a space is a
            // character a stream name may carry, so the same text that makes a group name
            // unreadable makes a stream name merely absent.
            "a stream name carrying a character only its own shape admits",
            LOG_STREAM,
            "absent-orders-v1|host 1 (eu)".to_string(),
        ),
    ];

    for (row, type_name, identifier) in rows {
        let answers = [
            ("GetResource", emulator.get(type_name, &identifier).await),
            (
                "UpdateResource",
                emulator
                    .update(type_name, &identifier, &inert_patch(), None)
                    .await,
            ),
        ];
        for (operation, (status, body)) in answers {
            let case = format!("{row} ({operation})");
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS}, got {status} with body {body:?}"
            );
            assert!(
                code(&body).contains(NOT_FOUND),
                "case {case:?}: expected {NOT_FOUND}, got {:?}",
                code(&body)
            );
            let stated = message(&body);
            assert!(
                stated.contains("(HandlerErrorCode: NotFound, RequestToken: ")
                    && !stated.contains("HandlerErrorCode: InvalidRequest"),
                "case {case:?}: expected the handler's not-found verdict, got {stated:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_resource_the_owning_service_holds_still_reads_and_patches() {
    let emulator = Emulator::with_cloudwatch_logs();
    // Nothing the rule changes may turn a served call into a refusal: a group of a name the
    // owning service takes reads back through the same path, and the patch that was inert in
    // the refusal rows above is applied to it.
    let group = "served.orders-v1/#a";
    let identifier = emulator
        .provision(LOG_GROUP, &json!({ "LogGroupName": group }))
        .await;
    assert_eq!(
        identifier, group,
        "a provisioned group must answer to the name it was created under, got {identifier:?}"
    );
    let reported = emulator.properties(LOG_GROUP, &identifier).await;
    assert_eq!(
        reported["LogGroupName"].as_str(),
        Some(group),
        "the read must report the group the owning service holds, got {reported:?}"
    );
    let settled = emulator
        .update_and_settle(LOG_GROUP, &identifier, &inert_patch())
        .await;
    assert_eq!(
        settled["Identifier"].as_str(),
        Some(group),
        "the patch must be applied to that group, got {settled:?}"
    );

    // And a stream in it: the compound reference the stream type is addressed by is one the
    // read of THAT type takes, so it resolves too.
    let stream = "host-1";
    let stream_identifier = emulator
        .provision(
            LOG_STREAM,
            &json!({ "LogGroupName": group, "LogStreamName": stream }),
        )
        .await;
    let reported = emulator.properties(LOG_STREAM, &stream_identifier).await;
    assert_eq!(
        (
            reported["LogGroupName"].as_str(),
            reported["LogStreamName"].as_str(),
        ),
        (Some(group), Some(stream)),
        "the read must report the stream the owning service holds, got {reported:?}"
    );
}
