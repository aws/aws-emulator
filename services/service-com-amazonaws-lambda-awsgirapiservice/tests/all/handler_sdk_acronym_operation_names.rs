// SPDX-License-Identifier: CC0-1.0

//! WHICH operation a client method a running handler calls reaches, for a service whose
//! operation names carry an ACRONYM — Amazon CloudWatch mounted beside Lambda on ONE engine, a
//! `python3.12` function whose handler drives the AWS SDK the runtime ships, and the same
//! operations read by an external caller around the invoke.
//!
//! The RULE asserted over the table below: the wire operation a client method names is the one
//! the CALLED SERVICE'S OWN MODEL declares under that name, matched without regard to case,
//! because an SDK's method name is the operation's name under that SDK's naming rule and no
//! word-by-word recasing of it reproduces a name that carries an acronym (`GetOTelEnrichment`,
//! `DescribeDBInstances`). So a handler inside a function is answered exactly what an external
//! caller of that operation is answered — the modelled output for a served call, the service's
//! own modelled refusal for a refused one — and a method naming no operation the service
//! declares is the SDK's own missing attribute rather than a service refusal.
//!
//! The rows differ along what the rule generalises over: whether the operation's name survives
//! a word-by-word recasing of the method name (`list_metrics` does, the four
//! `*_otel_enrichment` methods do not), whether the call reports or mutates, whether the
//! service serves it or refuses it with a modelled error, and whether the method names an
//! operation of that service at all.
//!
//! The suite RUNS a handler, so it takes this target's one invocation stream
//! ([`crate::invoke_request_id::one_invocation_stream_at_a_time`]) and makes exactly ONE
//! invocation: every row is one call of that single run.

use std::sync::Arc;

use base64::Engine as _;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

// The Lambda wire, the role a function runs as and the activation window every suite of this
// target drives, owned by the suite that first needed them rather than copied.
use crate::function_lifecycle::{make_activation_due, router_on, send, Answer, ROLE};
// The CloudWatch call an external caller of this emulator makes.
use crate::kinesis_event_source_delivery::cloudwatch;
// The one .zip writer this target packages a running handler with.
use crate::synchronous_invoke_run::package;

/// Where `CreateFunction` and `Invoke` are bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// The function this suite invokes, and the module and entry point its package holds.
const FUNCTION: &str = "sdk-acronym-operation-names";
const HANDLER: &str = "lambda_function.lambda_handler";

/// The function's own `Timeout`, in seconds: generous room for a run that makes a handful of
/// in-process calls, so nothing here is bounded by it however loaded the host is.
const FUNCTION_TIMEOUT_SECONDS: i64 = 30;

/// The two statuses `GetOTelEnrichmentOutput$Status` carries (`OTelEnrichmentStatus`, an enum
/// whose values are `Running` and `Stopped`).
const RUNNING: &str = "Running";
/// See [`RUNNING`].
const STOPPED: &str = "Stopped";

/// The error code and HTTP status `UpdateOTelEnrichment` refuses a call made while enrichment
/// is not running with (`ResourceNotFoundException`, `httpError` 404).
const NOT_FOUND_CODE: &str = "ResourceNotFoundException";
/// See [`NOT_FOUND_CODE`].
const NOT_FOUND_STATUS: u16 = 404;

/// The namespace the filters this suite stores select, and the metric inside it: a custom
/// namespace, so nothing here names a reserved `AWS/` one.
const FILTER_NAMESPACE: &str = "Emulator/HandlerSdkAcronymOperationNames";
/// See [`FILTER_NAMESPACE`].
const FILTER_METRIC: &str = "EnrichedSeries";

/// The error code this emulator refuses an operation it does not serve with, which is NOT an
/// answer AWS ever gives a client whose method names an operation its service has: a row that
/// reports it names the defect this suite was written for.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The handler every row runs in: it asks the client for the method the row names and reports
/// what the SDK handed back — the answer's own members, the service's refusal, or the fact that
/// the client has no such attribute at all.
const HANDLER_SOURCE: &str = r#"
import boto3
import botocore.exceptions


def made(client, call):
    """One call of the method the row names, as the SDK answered it."""
    method = call["method"]
    parameters = call.get("parameters") or {}
    try:
        bound = getattr(client, method)
    except AttributeError as error:
        return {"ok": False, "no_such_method": str(error)}
    try:
        answered = bound(**parameters)
    except AttributeError as error:
        return {"ok": False, "no_such_method": str(error)}
    except botocore.exceptions.ClientError as error:
        return {
            "ok": False,
            "class": type(error).__name__,
            "code": error.response["Error"]["Code"],
            "message": error.response["Error"].get("Message"),
            "status": error.response["ResponseMetadata"]["HTTPStatusCode"],
        }
    except Exception as error:
        return {"ok": False, "raised": "%s: %s" % (type(error).__name__, error)}
    return {
        "ok": True,
        "status": answered["ResponseMetadata"]["HTTPStatusCode"],
        "keys": sorted(key for key in answered if key != "ResponseMetadata"),
        "enrichment_status": answered.get("Status"),
        "enrichment_status_type": type(answered.get("Status")).__name__,
        "include_filters": answered.get("IncludeFilters"),
        "exclude_filters": answered.get("ExcludeFilters"),
    }


def lambda_handler(event, context):
    client = boto3.client("cloudwatch")
    return {"results": [made(client, call) for call in event["calls"]]}
"#;

/// What one row expects the SDK to have handed the handler.
enum Expected {
    /// The service served it: HTTP 200, with these members on the answer, and — for a row whose
    /// answer carries `Status` — that status as a string.
    Answered {
        members: &'static [&'static str],
        status: Option<&'static str>,
    },
    /// The service refused it with this modelled error: its code, its HTTP status, and the
    /// exception class an SDK raises a refusal of that modelled shape as.
    Refused {
        code: &'static str,
        status: u16,
        class: &'static str,
    },
    /// The service declares no operation of that name, so the client has no such method — which
    /// is what an SDK says, rather than any service refusal.
    NoSuchMethod,
}

/// One row: the client method the handler calls, what it passes, and what the SDK must hand
/// back.
struct Row {
    case: &'static str,
    /// The method name, as a handler writes it.
    method: &'static str,
    /// The keyword arguments it passes, as the operation's own input members.
    parameters: Value,
    expected: Expected,
}

/// One selector of the metrics in one namespace (`OTelEnrichmentMetricSelector`: `Namespace`,
/// required, and `MetricNames`).
fn selector() -> Value {
    json!([{ "Namespace": FILTER_NAMESPACE, "MetricNames": [FILTER_METRIC] }])
}

/// The rows, in the order the one invocation makes them: the enrichment configuration they
/// drive is the ACCOUNT's, so each row reads what the rows before it stored.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "a method whose operation name survives a word-by-word recasing",
            method: "list_metrics",
            parameters: json!({ "Namespace": FILTER_NAMESPACE }),
            expected: Expected::Answered {
                members: &["Metrics"],
                status: None,
            },
        },
        Row {
            case: "an acronym-named mutation refused by its own modelled error",
            method: "update_otel_enrichment",
            parameters: json!({ "IncludeFilters": [] }),
            expected: Expected::Refused {
                code: NOT_FOUND_CODE,
                status: NOT_FOUND_STATUS,
                class: NOT_FOUND_CODE,
            },
        },
        Row {
            case: "an acronym-named read of an account that has stored nothing",
            method: "get_otel_enrichment",
            parameters: json!({}),
            expected: Expected::Answered {
                members: &["Status"],
                status: Some(STOPPED),
            },
        },
        Row {
            case: "an acronym-named mutation that starts enrichment with a filter",
            method: "start_otel_enrichment",
            parameters: json!({ "IncludeFilters": selector() }),
            expected: Expected::Answered {
                // A start reports the tag-enrichment setting it stored as well as the filters:
                // "Specifies whether enriched metrics include resource tag labels for the
                // account." (`StartOTelEnrichmentOutput$EnableTagEnrichment`)
                members: &["CreatedAt", "EnableTagEnrichment", "IncludeFilters", "UpdatedAt"],
                status: None,
            },
        },
        Row {
            case: "the same acronym-named read, now that enrichment is running",
            method: "get_otel_enrichment",
            parameters: json!({}),
            expected: Expected::Answered {
                members: &[
                    "CreatedAt",
                    "EnableTagEnrichment",
                    "IncludeFilters",
                    "Status",
                    "UpdatedAt",
                ],
                status: Some(RUNNING),
            },
        },
        Row {
            case: "an acronym-named mutation that replaces the stored filters",
            method: "update_otel_enrichment",
            parameters: json!({ "ExcludeFilters": selector() }),
            expected: Expected::Answered {
                members: &["CreatedAt", "EnableTagEnrichment", "ExcludeFilters", "UpdatedAt"],
                status: None,
            },
        },
        Row {
            case: "an acronym-named mutation whose output carries no member at all",
            method: "stop_otel_enrichment",
            parameters: json!({}),
            expected: Expected::Answered {
                members: &[],
                status: None,
            },
        },
        Row {
            case: "the acronym-named read after the stop",
            method: "get_otel_enrichment",
            parameters: json!({}),
            expected: Expected::Answered {
                // The tag-enrichment setting goes with the enrichment the stop ended: "This
                // parameter is omitted when enrichment is stopped."
                // (`GetOTelEnrichmentOutput$EnableTagEnrichment`)
                members: &["Status", "UpdatedAt"],
                status: Some(STOPPED),
            },
        },
        Row {
            case: "a method naming no operation this service declares",
            method: "get_otel_enrichment_configuration",
            parameters: json!({}),
            expected: Expected::NoSuchMethod,
        },
    ]
}

/// Lambda and Amazon CloudWatch over ONE engine, as `core/router/src/lib.rs` mounts them: the
/// handler's SDK calls are dispatched back into this same router, so the configuration it
/// drives is the one this suite reads from outside.
fn router(engine: &Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = router_on(engine);
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            Arc::clone(engine),
        ),
    );
    router
}

/// The `Status` an EXTERNAL caller of `GetOTelEnrichment` reads, over CloudWatch's own wire.
async fn status_outside(router: &MultiServiceRouter, case: &str) -> String {
    let (status, answered) = cloudwatch(router, "GetOTelEnrichment", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetOTelEnrichment, got {status} ({answered})"
    );
    answered["Status"]
        .as_str()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a Status on the answer, got {answered}")
        })
        .to_string()
}

/// Create the function and bring it to `Active`.
async fn given_function(router: &MultiServiceRouter, engine: &Arc<engine::InMemoryEngine>) {
    let created = send(
        router,
        "POST",
        FUNCTIONS_URI,
        "application/json",
        serde_json::to_vec(&json!({
            "FunctionName": FUNCTION,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": HANDLER,
            "Timeout": FUNCTION_TIMEOUT_SECONDS,
            "Code": {
                "ZipFile": base64::engine::general_purpose::STANDARD
                    .encode(package(HANDLER_SOURCE)),
            },
        }))
        .expect("the create body serializes"),
        &[],
    )
    .await;
    assert_eq!(
        created.status,
        201,
        "setup: expected HTTP 201 from CreateFunction, got {} ({})",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
    make_activation_due(engine);
}

/// Make every row's call in ONE invocation and answer the result the SDK gave the guest for
/// each.
async fn results(router: &MultiServiceRouter, rows: &[Row]) -> Vec<Value> {
    let calls: Vec<Value> = rows
        .iter()
        .map(|row| json!({ "method": row.method, "parameters": row.parameters }))
        .collect();
    let answered: Answer = send(
        router,
        "POST",
        &format!("{FUNCTIONS_URI}/{FUNCTION}/invocations"),
        "application/json",
        serde_json::to_vec(&json!({ "calls": calls })).expect("the event serializes"),
        &[],
    )
    .await;
    assert_eq!(
        answered.status,
        200,
        "setup: expected HTTP 200 from Invoke, got {} ({})",
        answered.status,
        String::from_utf8_lossy(&answered.body)
    );
    let signal = answered.header("x-amz-function-error");
    assert_eq!(
        signal,
        None,
        "setup: expected the handler to run to completion, got the function error {signal:?} \
         with {}",
        String::from_utf8_lossy(&answered.body)
    );
    let payload = answered.json();
    let results = payload["results"].as_array().cloned().unwrap_or_default();
    assert_eq!(
        results.len(),
        rows.len(),
        "setup: expected one result per call, got {payload}"
    );
    results
}

/// The members a served answer carries, as the handler reported them.
fn members(result: &Value) -> Vec<String> {
    result["keys"]
        .as_array()
        .map(|listed| {
            listed
                .iter()
                .filter_map(|key| key.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// RULE: a client method a running handler calls names the operation the called service's own
/// model declares under that name, whatever case the SDK derived it in — so an acronym-named
/// operation is reached from inside a function exactly as it is from outside, answered with its
/// modelled output or its own modelled refusal, and a method naming no operation of that
/// service is the SDK's own missing attribute and never a service refusal.
#[tokio::test]
async fn a_handlers_client_method_reaches_the_operation_the_model_names() {
    let _stream = crate::invoke_request_id::one_invocation_stream_at_a_time().await;
    let engine = Arc::new(engine::InMemoryEngine::new());
    let router = router(&engine);
    let rows = rows();

    given_function(&router, &engine).await;

    // What the operation answers an EXTERNAL caller before the invoke: the account has stored
    // nothing, so enrichment is stopped.
    let before = status_outside(&router, "the external read before the invoke").await;
    assert_eq!(
        before, STOPPED,
        "case {:?}: expected {STOPPED:?} before anything started enrichment, got {before:?}",
        "the external read before the invoke"
    );

    let results = results(&router, &rows).await;

    for (row, result) in rows.iter().zip(&results) {
        let case = row.case;
        assert!(
            result["raised"].is_null(),
            "case {case:?}: expected the SDK to dispatch the call, got {result}"
        );
        assert_ne!(
            result["code"],
            json!(NOT_IMPLEMENTED_CODE),
            "case {case:?}: expected the call to reach the operation the model names, got \
             {result}"
        );
        match row.expected {
            Expected::Answered {
                members: expected,
                status,
            } => {
                assert_eq!(
                    result["ok"],
                    json!(true),
                    "case {case:?}: expected the call to be served, got {result}"
                );
                assert_eq!(
                    result["status"],
                    json!(200),
                    "case {case:?}: expected the SDK to report HTTP 200, got {result}"
                );
                let held = members(result);
                let expected: Vec<String> =
                    expected.iter().map(|name| (*name).to_string()).collect();
                assert_eq!(
                    held, expected,
                    "case {case:?}: expected the answer to carry {expected:?}, got {held:?} in \
                     {result}"
                );
                if let Some(status) = status {
                    assert_eq!(
                        result["enrichment_status"],
                        json!(status),
                        "case {case:?}: expected Status {status:?}, got {result}"
                    );
                    assert_eq!(
                        result["enrichment_status_type"],
                        json!("str"),
                        "case {case:?}: expected Status to be a string, got {result}"
                    );
                }
            }
            Expected::Refused {
                code,
                status,
                class,
            } => {
                assert_eq!(
                    result["ok"],
                    json!(false),
                    "case {case:?}: expected the service's own refusal, got {result}"
                );
                assert_eq!(
                    result["code"],
                    json!(code),
                    "case {case:?}: expected the error code {code:?}, got {result}"
                );
                assert_eq!(
                    result["status"],
                    json!(status),
                    "case {case:?}: expected HTTP {status} on the refusal, got {result}"
                );
                assert_eq!(
                    result["class"],
                    json!(class),
                    "case {case:?}: expected the refusal to be raised as the modelled class \
                     {class:?}, got {result}"
                );
                assert!(
                    result["message"].as_str().is_some_and(|text| !text.is_empty()),
                    "case {case:?}: expected the service's own message on the refusal, got \
                     {result}"
                );
            }
            Expected::NoSuchMethod => {
                assert_eq!(
                    result["ok"],
                    json!(false),
                    "case {case:?}: expected no such method on the client, got {result}"
                );
                let reported = result["no_such_method"].as_str();
                let expected = format!("has no attribute '{}'", row.method);
                assert!(
                    reported.is_some_and(|text| text.contains(&expected)),
                    "case {case:?}: expected the SDK's own missing attribute naming \
                     {expected:?}, got {result}"
                );
                assert!(
                    result["code"].is_null(),
                    "case {case:?}: expected no service refusal for a method the service has \
                     no operation for, got {result}"
                );
            }
        }
    }

    // What the operation answers the EXTERNAL caller after the invoke: the configuration the
    // handler's own calls left — stopped again, after the stop the last mutation row made.
    let after = status_outside(&router, "the external read after the invoke").await;
    assert_eq!(
        after, STOPPED,
        "case {:?}: expected the handler's own stop to be what an external caller reads, got \
         {after:?}",
        "the external read after the invoke"
    );

    // And the filters that stop cleared are gone for the external reader too, which is the
    // other half of the mutations the handler made through those acronym-named methods.
    let (status, answered) = cloudwatch(&router, "GetOTelEnrichment", &json!({})).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected HTTP 200 from GetOTelEnrichment, got {status} ({answered})",
        "the external read after the invoke"
    );
    assert!(
        answered["IncludeFilters"].is_null() && answered["ExcludeFilters"].is_null(),
        "case {:?}: expected no filters while enrichment is stopped, got {answered}",
        "the external read after the invoke"
    );
    assert!(
        !answered["UpdatedAt"].is_null(),
        "case {:?}: expected the stop the handler made to be the configuration last stored, \
         got {answered}",
        "the external read after the invoke"
    );
}
