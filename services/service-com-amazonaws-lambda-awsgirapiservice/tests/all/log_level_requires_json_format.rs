// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for which LOG FORMAT a function's level filters may be configured on.
//!
//! The rule under test: `LoggingConfig$ApplicationLogLevel` and `$SystemLogLevel` are the
//! levels Lambda filters a function's logs at, and that filtering belongs to the structured
//! format alone ("To use this feature, set the log format to JSON"), so a configuration
//! naming a level while the format it will carry is the plain-text one — named, or left to
//! its documented default — is refused before anything is stored, rather than stored as
//! filtering the function cannot perform. Both operations that accept a `LoggingConfig`
//! apply it.
//!
//! Every accepted configuration is read back through `GetFunctionConfiguration`, and after
//! every refusal the function's stored configuration is read back too, so no assertion
//! trusts the write's own answer.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::function_lifecycle::{
    create_function, json_call, make_update_due, package_bytes, router_on, zip_file, Answer, ROLE,
};

/// The modelled 400 a configuration Lambda will not store is refused with.
const INVALID_PARAMETER: &str = "InvalidParameterValueException";

/// The sentence the refusal carries: which format does not support a log level, what to do
/// about it, and the format that does.
const LEVELS_NEED_JSON: &str = "LogLevel is not supported when LogFormat is set to \"Text\". \
     Remove LogLevel from your request or change the LogFormat to \"JSON\" and try again.";

/// The body of an answer, for a failure message.
fn body_text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The `message` of a wire error, however the protocol spells the member.
fn message_of(answer: &Answer) -> String {
    answer
        .string("message")
        .or_else(|| answer.string("Message"))
        .unwrap_or_default()
}

/// Create one function with the request members `extras` adds.
async fn create_named(router: &MultiServiceRouter, name: &str, extras: &Value) -> Answer {
    let mut body = json!({
        "FunctionName": name,
        "Role": ROLE,
        "Runtime": "python3.12",
        "Handler": "index.handler",
        "Code": { "ZipFile": zip_file(&package_bytes(64)) },
    });
    if let Some(members) = extras.as_object() {
        for (member, value) in members {
            body[member] = value.clone();
        }
    }
    create_function(router, &body).await
}

/// `UpdateFunctionConfiguration` with the members `body` names.
async fn update_configuration(router: &MultiServiceRouter, name: &str, body: &Value) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/configuration");
    json_call(router, "PUT", &uri, body).await
}

/// `GetFunctionConfiguration` on one function name.
async fn get_configuration(router: &MultiServiceRouter, name: &str) -> Answer {
    let uri = format!("/2015-03-31/functions/{name}/configuration");
    json_call(router, "GET", &uri, &Value::Null).await
}

/// One logging configuration the rule is asserted over.
struct Case {
    /// The case name, which is also the function name a create row uses.
    case: &'static str,
    /// The `LoggingConfig` the request carries.
    config: Value,
    /// Whether a write naming it is accepted.
    accepted: bool,
}

/// The configurations the rule is asserted over: which level member is named (either, both,
/// neither), whether the format is named or left to its documented default, and each of the
/// two formats the model declares (`smithy_query(query="shape", shape="LogFormat")`:
/// `JSON | Text`).
fn cases() -> Vec<Case> {
    vec![
        Case {
            case: "the-structured-format-alone",
            config: json!({ "LogFormat": "JSON" }),
            accepted: true,
        },
        Case {
            case: "the-structured-format-with-both-levels",
            config: json!({
                "LogFormat": "JSON",
                "ApplicationLogLevel": "WARN",
                "SystemLogLevel": "DEBUG",
            }),
            accepted: true,
        },
        Case {
            case: "the-structured-format-with-the-application-level",
            config: json!({ "LogFormat": "JSON", "ApplicationLogLevel": "TRACE" }),
            accepted: true,
        },
        Case {
            case: "the-plain-text-format-alone",
            config: json!({ "LogFormat": "Text" }),
            accepted: true,
        },
        Case {
            case: "a-log-group-alone",
            config: json!({ "LogGroup": "/aws/lambda/declared-group" }),
            accepted: true,
        },
        Case {
            case: "the-plain-text-format-with-the-application-level",
            config: json!({ "LogFormat": "Text", "ApplicationLogLevel": "WARN" }),
            accepted: false,
        },
        Case {
            case: "the-plain-text-format-with-the-system-level",
            config: json!({ "LogFormat": "Text", "SystemLogLevel": "DEBUG" }),
            accepted: false,
        },
        Case {
            case: "the-plain-text-format-with-both-levels",
            config: json!({
                "LogFormat": "Text",
                "ApplicationLogLevel": "FATAL",
                "SystemLogLevel": "WARN",
            }),
            accepted: false,
        },
        Case {
            case: "a-level-with-no-format-named",
            config: json!({ "ApplicationLogLevel": "INFO" }),
            accepted: false,
        },
        Case {
            case: "a-system-level-with-no-format-named",
            config: json!({ "SystemLogLevel": "WARN" }),
            accepted: false,
        },
    ]
}

/// Assert `answered` is the refusal a level on the plain-text format gets.
fn assert_refused(case: &str, operation: &str, answered: &Answer) {
    assert_eq!(
        (answered.status, answered.error_code()),
        (400, INVALID_PARAMETER.to_string()),
        "case {case:?} ({operation}): expected 400 {INVALID_PARAMETER}, got {} {}",
        answered.status,
        body_text(answered)
    );
    let reported = message_of(answered);
    assert_eq!(
        reported, LEVELS_NEED_JSON,
        "case {case:?} ({operation}): expected the message {LEVELS_NEED_JSON:?}, got {reported:?}"
    );
}

/// RULE: `CreateFunction` refuses a logging configuration that asks for level filtering on
/// the plain-text format, and stores no function; every other combination is created and
/// reports its format back.
#[tokio::test]
async fn create_function_refuses_a_log_level_the_plain_text_format_cannot_filter_at() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);

    for case in cases() {
        let answered = create_named(&router, case.case, &json!({ "LoggingConfig": case.config }))
            .await;
        let read = get_configuration(&router, case.case).await;
        if case.accepted {
            assert_eq!(
                answered.status,
                201,
                "case {:?}: expected 201 from CreateFunction, got {} {}",
                case.case,
                answered.status,
                body_text(&answered)
            );
            // The stored configuration is read back: the format it carries is the one the
            // request named, or the documented default where it named none.
            let expected = case
                .config
                .get("LogFormat")
                .cloned()
                .unwrap_or_else(|| json!("Text"));
            assert_eq!(
                read.json()["LoggingConfig"]["LogFormat"],
                expected,
                "case {:?}: expected LogFormat {expected}, got {}",
                case.case,
                body_text(&read)
            );
        } else {
            assert_refused(case.case, "CreateFunction", &answered);
            assert_eq!(
                read.status,
                404,
                "case {:?}: expected the refused create to store no function, got {} {}",
                case.case,
                read.status,
                body_text(&read)
            );
        }
    }
}

/// RULE (the same one, on the other operation that accepts a `LoggingConfig`):
/// `UpdateFunctionConfiguration` refuses the same configurations and leaves the one the
/// function already had.
#[tokio::test]
async fn update_function_configuration_refuses_the_same_configurations() {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router_on(&engine);
    let created = create_named(
        &router,
        "logging-fn",
        &json!({ "LoggingConfig": { "LogFormat": "Text" } }),
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected 201 from CreateFunction, got {} {}",
        created.status,
        body_text(&created)
    );

    for case in cases() {
        // Each row starts from the same stored configuration — the plain-text format with
        // no level — so what a refused row leaves behind is known. "During this time, you
        // can't invoke or modify the function": the settle each write owes is brought
        // forward rather than waited out.
        make_update_due(&engine);
        let baseline = update_configuration(
            &router,
            "logging-fn",
            &json!({ "LoggingConfig": { "LogFormat": "Text" } }),
        )
        .await;
        assert_eq!(
            baseline.status,
            200,
            "case {:?}: expected the baseline update to be served, got {} {}",
            case.case,
            baseline.status,
            body_text(&baseline)
        );

        make_update_due(&engine);
        let answered = update_configuration(
            &router,
            "logging-fn",
            &json!({ "LoggingConfig": case.config }),
        )
        .await;
        let read = get_configuration(&router, "logging-fn").await;
        if case.accepted {
            assert_eq!(
                answered.status,
                200,
                "case {:?}: expected 200 from UpdateFunctionConfiguration, got {} {}",
                case.case,
                answered.status,
                body_text(&answered)
            );
            let expected = case
                .config
                .get("LogFormat")
                .cloned()
                .unwrap_or_else(|| json!("Text"));
            assert_eq!(
                read.json()["LoggingConfig"]["LogFormat"],
                expected,
                "case {:?}: expected LogFormat {expected}, got {}",
                case.case,
                body_text(&read)
            );
        } else {
            assert_refused(case.case, "UpdateFunctionConfiguration", &answered);
            // The configuration the function already had is the one it still has: a
            // refused update changes nothing a later read sees.
            let body = read.json();
            assert_eq!(
                (
                    body["LoggingConfig"]["LogFormat"].clone(),
                    body["LoggingConfig"]["ApplicationLogLevel"].clone(),
                    body["LoggingConfig"]["SystemLogLevel"].clone()
                ),
                (json!("Text"), Value::Null, Value::Null),
                "case {:?}: expected the refused update to leave the plain-text format with \
                 no level, got {}",
                case.case,
                body_text(&read)
            );
        }
    }
}
