// SPDX-License-Identifier: CC0-1.0

//! `GetLogObject` over the large log objects this store holds.
//!
//! RULE: a `logObjectPointer` resolves only when THIS SERVICE issued it for a large log
//! object it holds — the value of an `@ptr.$[path.to.field]` reference field a log event
//! carries — so every other pointer a caller can present is refused as the operation's own
//! modelled client error, HTTP 400 `InvalidParameterException`: one the client built
//! itself, one that has expired, one this service minted for another purpose. A value
//! outside a modelled constraint is refused the same way, worded as the service words a
//! constraint clause.
//!
//! No operation bound to this service makes a large log object: the ingestion path that
//! splits an oversized event's fields into one (`PutOpenTelemetryLogs`) is not among the
//! operations this model binds, and the ingestion path it does bind refuses an event past
//! the 1 MB ceiling rather than splitting it. So no pointer this service issued exists, and
//! every request below is refused rather than answered with object bytes — which is the
//! answer AWS gives the same requests, and NOT the `NotImplemented` an unwired operation
//! used to answer.
//!
//! The rows below differ along what the rule generalises over: the FORM of the pointer (a
//! reference-field spelling, an opaque token of this service's own encoding, a pointer this
//! service really minted for a query row, a URL-safe token, a bare word, the shortest and
//! the longest the model admits), whether `unmask` is asked for, absent, or `false`, and
//! which modelled constraint a refused value broke.

use serde_json::{json, Value};
use std::sync::Arc;

use crate::harness::{base_ms, batch, messages, now_ms, Logs};

/// The operation under test.
const GET_LOG_OBJECT: &str = "GetLogObject";

/// The modelled error every refusal below carries: "A parameter is specified incorrectly"
/// (`GetLogObject`'s `InvalidParameterException`, HTTP 400).
const INVALID_PARAMETER: &str = "InvalidParameterException";

/// The sentence a pointer this service did not issue is refused with.
const POINTER_NOT_ISSUED: &str = "Invalid log object pointer format.";

/// The `@length` maximum the model declares on `LogObjectPointer` (1..512). Source:
/// `smithy_query(query="operation",operation="GetLogObject")` —
/// `logObjectPointer` (LogObjectPointer, len 1..512, required).
const MAX_POINTER_CHARS: usize = 512;

/// The log group, stream and events the suite reads back, so a refused request is shown to
/// have changed nothing a later reader sees.
const GROUP: &str = "/log-objects/group";
const STREAM: &str = "stream-one";
const MESSAGES: [&str; 2] = [
    r#"{"level":"ERROR","payload":"the large field"}"#,
    "plain text line",
];

/// The group above, holding [`MESSAGES`], and the whole-second base instant they are
/// stamped from (a query's window is stated in whole seconds).
async fn given_stored_events(logs: &Logs) -> i64 {
    logs.ok("CreateLogGroup", &json!({"logGroupName": GROUP}))
        .await;
    logs.ok(
        "CreateLogStream",
        &json!({"logGroupName": GROUP, "logStreamName": STREAM}),
    )
    .await;
    let base = base_ms() / 1_000 * 1_000;
    let events: Vec<(i64, &str)> = MESSAGES
        .iter()
        .enumerate()
        .map(|(index, message)| (base + 1_000 * (index as i64 + 1), *message))
        .collect();
    logs.ok(
        "PutLogEvents",
        &json!({
            "logGroupName": GROUP,
            "logStreamName": STREAM,
            "logEvents": batch(&events),
        }),
    )
    .await;
    base
}

/// The messages a reader still finds in the group, so a refusal is shown to have persisted
/// nothing and consumed nothing.
async fn stored_messages(logs: &Logs) -> Vec<String> {
    let page = logs
        .ok(
            "GetLogEvents",
            &json!({"logGroupName": GROUP, "logStreamName": STREAM, "startFromHead": true}),
        )
        .await;
    messages(&page)
}

/// Every pointer a caller can present is refused as this operation's modelled
/// `InvalidParameterException`, whatever its form and whatever `unmask` asks for — and the
/// refusal carries the one sentence for the class, no object bytes, and leaves the stored
/// events exactly where they were.
#[tokio::test]
async fn a_pointer_this_service_did_not_issue_is_refused_as_an_invalid_parameter() {
    let logs = Logs::new();
    given_stored_events(&logs).await;

    let longest = "p".repeat(MAX_POINTER_CHARS);
    // (case, the pointer the request carries)
    let pointers: Vec<(&str, String)> = vec![
        (
            "a reference-field spelling a client wrote itself",
            "@ptr.$['payload']".to_string(),
        ),
        (
            "a base64 token of this service's own opaque-token encoding",
            "eyJraW5kIjoiTG9nT2JqZWN0UG9pbnRlciJ9".to_string(),
        ),
        (
            "a URL-safe base64 token with its padding stripped",
            "aGVsbG8td29ybGQ_YQ".to_string(),
        ),
        ("a bare word", "pointer".to_string()),
        (
            "a UUID, the form several of this service's own ids take",
            "00000000-0000-4000-8000-000000000001".to_string(),
        ),
        ("the shortest pointer the model admits", "p".to_string()),
        ("the longest pointer the model admits", longest),
    ];
    // How the request spells `unmask`: the member has a default, so an absent one is a
    // third form of the same request rather than a different request.
    let unmask_forms: [(&str, Option<bool>); 3] = [
        ("absent", None),
        ("false", Some(false)),
        ("true", Some(true)),
    ];

    for (case, pointer) in &pointers {
        for (unmask_case, unmask) in unmask_forms {
            let mut request = json!({"logObjectPointer": pointer});
            if let Some(unmask) = unmask {
                request["unmask"] = Value::Bool(unmask);
            }
            let (status, body) = logs.call(GET_LOG_OBJECT, &request).await;
            assert_eq!(
                status, 400,
                "case {case} with unmask {unmask_case}: a pointer the service did not issue \
                 is a client error, got {status} {body}"
            );
            let code = crate::harness::error_code(&body);
            assert_eq!(
                code, INVALID_PARAMETER,
                "case {case} with unmask {unmask_case}: expected {INVALID_PARAMETER}, got \
                 {code} {body}"
            );
            let message = body.get("message").and_then(Value::as_str).unwrap_or_default();
            assert_eq!(
                message, POINTER_NOT_ISSUED,
                "case {case} with unmask {unmask_case}: expected {POINTER_NOT_ISSUED:?}, got \
                 {message:?}"
            );
            assert_eq!(
                body.get("fieldStream"),
                None,
                "case {case} with unmask {unmask_case}: a refusal carries no object, got {body}"
            );
        }
    }

    assert_eq!(
        stored_messages(&logs).await,
        MESSAGES.to_vec(),
        "case after every refusal: a refused read changes nothing a later reader sees"
    );
}

/// A `logObjectPointer` outside the model's own `@length` bound, and an absent one, are
/// refused with the clause this service words a constraint failure with — on each published
/// bound and one past it.
#[tokio::test]
async fn a_pointer_outside_the_modelled_constraints_is_refused_with_the_constraint_clause() {
    let logs = Logs::new();

    let past_max = "p".repeat(MAX_POINTER_CHARS + 1);
    // (case, the request body, the message it is refused with)
    let cases: Vec<(&str, Value, String)> = vec![
        (
            "no logObjectPointer at all",
            json!({}),
            "1 validation error detected: Value at 'logObjectPointer' failed to satisfy \
             constraint: Member must not be null"
                .to_string(),
        ),
        (
            "a pointer shorter than the minimum",
            json!({"logObjectPointer": ""}),
            "1 validation error detected: Value at 'logObjectPointer' failed to satisfy \
             constraint: Member must have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a pointer one character past the maximum",
            json!({"logObjectPointer": past_max}),
            format!(
                "1 validation error detected: Value at 'logObjectPointer' failed to satisfy \
                 constraint: Member must have length less than or equal to {MAX_POINTER_CHARS}"
            ),
        ),
        (
            "a pointer exactly at the maximum, which the model admits",
            json!({"logObjectPointer": "p".repeat(MAX_POINTER_CHARS)}),
            POINTER_NOT_ISSUED.to_string(),
        ),
        (
            "a pointer exactly at the minimum, which the model admits",
            json!({"logObjectPointer": "p"}),
            POINTER_NOT_ISSUED.to_string(),
        ),
    ];

    for (case, request, expected) in cases {
        let (code, message) = logs.refusal(GET_LOG_OBJECT, &request).await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case {case}: this operation refuses a bad parameter as {INVALID_PARAMETER}, got \
             {code} {message:?}"
        );
        assert_eq!(
            message, expected,
            "case {case}: expected {expected:?}, got {message:?}"
        );
    }
}

/// The pointer this service DOES mint — the `@ptr` of a query row, "the identifier for the
/// log record" — addresses a whole log record rather than a large log object, so presenting
/// one to `GetLogObject` is refused like any other pointer the service did not issue for an
/// object.
#[tokio::test]
async fn a_query_row_pointer_is_not_a_log_object_pointer() {
    let logs = Logs::new();
    let base = given_stored_events(&logs).await;

    let started = logs
        .ok(
            "StartQuery",
            &json!({
                "logGroupName": GROUP,
                "startTime": base / 1_000 - 60,
                "endTime": now_ms() / 1_000 + 60,
                "queryString": "fields @message",
            }),
        )
        .await;
    let query_id = started["queryId"]
        .as_str()
        .unwrap_or_else(|| panic!("StartQuery answers a queryId, got {started}"))
        .to_string();
    // Past both spans the start armed, so the run has produced its rows.
    tokio::time::sleep(std::time::Duration::from_millis(900)).await;
    let results = logs
        .ok("GetQueryResults", &json!({"queryId": query_id}))
        .await;

    let pointers: Vec<String> = results["results"]
        .as_array()
        .unwrap_or_else(|| panic!("GetQueryResults answers a results list, got {results}"))
        .iter()
        .filter_map(|row| {
            row.as_array()?.iter().find_map(|field| {
                (field["field"].as_str()? == "@ptr")
                    .then(|| field["value"].as_str().map(str::to_string))?
            })
        })
        .collect();
    assert_eq!(
        pointers.len(),
        MESSAGES.len(),
        "every row of a non-aggregating query carries a @ptr, got {results}"
    );

    for pointer in &pointers {
        let (code, message) = logs
            .refusal(GET_LOG_OBJECT, &json!({"logObjectPointer": pointer}))
            .await;
        assert_eq!(
            code, INVALID_PARAMETER,
            "case a @ptr of a query row: expected {INVALID_PARAMETER}, got {code} {message:?}"
        );
        assert_eq!(
            message, POINTER_NOT_ISSUED,
            "case a @ptr of a query row: expected {POINTER_NOT_ISSUED:?}, got {message:?}"
        );
    }

    // The query's own rows are still readable: refusing a pointer consumed nothing.
    let again = logs
        .ok("GetQueryResults", &json!({"queryId": query_id}))
        .await;
    assert_eq!(
        again["results"], results["results"],
        "case after the refusals: the query's rows are unchanged"
    );
}

/// The endpoint this service answers on EXISTS for a caller it will not authorize: a
/// request that addresses a modelled operation of this service with a credential the
/// emulator cannot verify — a bearer token rather than a SigV4 signature — is refused as
/// unauthorized, HTTP 403, and never as an unknown endpoint (404).
///
/// The refusal is the shared authenticator's, which is reached only because this service
/// CLAIMS the request and names an operation for it; a request that names no operation of
/// this model is answered by the routing-error path instead, which is why the status a
/// caller sees turns on whether the endpoint it addressed is one this service serves.
#[tokio::test]
async fn an_unverifiable_credential_is_refused_as_unauthorized_not_as_an_unknown_endpoint() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = smithy_server_runtime::MultiServiceRouter::new();
    // Authentication ON, unlike the shared harness: the subject here is which status an
    // unauthorized caller is refused with.
    router.set_request_authentication(true);
    router.register_service(
        service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine),
    );

    // (case, the Authorization header the caller presents)
    let credentials = [
        ("a bearer token", "Bearer not-a-sigv4-credential"),
        ("a basic credential", "Basic dXNlcjpwYXNzd29yZA=="),
    ];
    // The operations the requests address: one the generated routing table serves, and the
    // two the table holds only a declining shim for, routed by `override_operation`. Each
    // is an endpoint this service names an operation for, so the credential is what decides
    // the answer — where, before `GetLogObject` was wired, a request to it named no
    // operation at all and was answered as an unknown endpoint instead.
    let targets = [
        "Logs_20140328.DescribeLogGroups",
        "Logs_20140328.GetLogObject",
        "Logs_20140328.StartLiveTail",
    ];
    for target in targets {
        for (case, authorization) in credentials {
            let request = http::Request::builder()
                .method("POST")
                .uri("/")
                .header("content-type", "application/x-amz-json-1.1")
                .header("x-amz-target", target)
                .header("authorization", authorization)
                .body(bytes::Bytes::from("{}"))
                .expect("a valid request");
            let status = router.dispatch(request).await.status().as_u16();
            assert_eq!(
                status, 403,
                "case {case} addressing {target}: an endpoint that exists refuses an \
                 unauthorized caller, got {status}"
            );
        }
    }
}
