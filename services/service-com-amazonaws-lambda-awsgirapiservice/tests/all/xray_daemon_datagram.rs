// SPDX-License-Identifier: CC0-1.0

//! The `TracingConfig` MODE a function carries — the one input to the trace context an
//! invocation of it runs with — and where the RUN-based half of that rule is asserted.
//!
//! The rule pinned here: a function's `TracingConfig` mode is the one the caller named, over
//! the values `TracingMode` declares ("Set Mode to Active to sample and trace a subset of
//! incoming requests with X-Ray", and the `PassThrough` a function gets when the caller named
//! none); it is reported by every read of the function's configuration; and `PublishVersion`
//! FREEZES it on the version it publishes — so an invocation that resolves a published version
//! is traced as that version was configured rather than as the unpublished one now is. That
//! mode is what `crate::state::execution::trace_context` reads to decide the context: `Active`
//! has Lambda record the invocation and stamp its own, `PassThrough` has it record nothing and
//! pass the caller's header on.
//!
//! The rows differ along what the rule generalises over: which value the create named, which
//! value an update moved it to, and the absence of the member altogether.
//!
//! The RUN-based half — what the environment of an invocation really reports in
//! `_X_AMZN_TRACE_ID`, `AWS_XRAY_DAEMON_ADDRESS` and `AWS_XRAY_CONTEXT_MISSING`, what the
//! daemon at that address does with a datagram a handler writes to it, and the two segments
//! X-Ray records for the invocation — is asserted in three other places, and never from the
//! test FUNCTION below: the suites of this target run in parallel and the runs of all of them
//! share one process-wide resident ceiling (`crate::state::execution::python`), which the
//! target reaches as it goes, so a suite that keeps several interpreters in flight gets
//! whichever run is in flight then killed whatever it is doing. (Reproduced both ways: a
//! handler-running suite that drives one run per row is killed, and with none added
//! `invoke_request_id` fails on its own.) The three places are:
//!
//! * beside the rule itself, over the whole table each half generalises over: every form a
//!   caller's tracing header can take and both modes
//!   (`crate::state::execution::trace_context`);
//! * END TO END for a DIRECT `Invoke`, by `crate::xray_daemon_trace_store` — which mounts THIS
//!   service and AWS X-Ray on one engine, reads those three variables out of the environment of
//!   a handler it really runs, and drives the whole datagram FRAMING table through ONE run on
//!   ONE socket while holding `crate::invoke_request_id::one_invocation_stream_at_a_time`,
//!   which is what keeps it inside the ceiling above (measured: one interpreter for the whole
//!   table leaves this target's own suites passing, one per row does not);
//! * END TO END through a deployed stage, by
//!   `services/service-com-amazonaws-apigateway-backplanecontrolservice/tests/all/\
//!   stage_trace_context.rs` — which mounts API Gateway, THIS service and AWS X-Ray on one
//!   engine, drives the datagram FRAMING table from inside the handler it runs, and reads the
//!   `AWS::Lambda` and `AWS::Lambda::Function` segments back through X-Ray's own
//!   `BatchGetTraces`.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_lambda_awsgirapiservice::state::ACTIVATION_DELAY;
use service_com_amazonaws_lambda_awsgirapiservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for.
const REGION: &str = "us-east-1";

/// Where `CreateFunction` is bound.
const FUNCTIONS_URI: &str = "/2015-03-31/functions";

/// A well-formed execution role ARN, satisfying the model's `RoleArn` pattern.
const ROLE: &str = "arn:aws:iam::000000000000:role/service-role/lambda-exec";

/// The mode a function carries when the caller names no `TracingConfig` at all, from the
/// model's `TracingMode` enum (`Active | PassThrough`).
const DEFAULT_MODE: &str = "PassThrough";

/// How long past a due instant this suite waits before calling. The provisioning window a
/// create documents is owed by the CLOCK, so nothing here sleeps on the emulator's behalf.
const SLACK: Duration = Duration::from_millis(150);

/// This service alone, on its own engine: the rule is a record of this service's.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// A SigV4 `Authorization` whose credential scope names this service's signing name. The
/// signature is never checked here.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=emulator-test-key/20260101/{REGION}/lambda/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One Lambda `restJson1` call, as its status and parsed body.
async fn lambda(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body.map(|value| value.to_string()).unwrap_or_default();
    let request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("lambda.{REGION}.amazonaws.com"))
        .header("authorization", authorization())
        .header("content-type", "application/json")
        .body(bytes::Bytes::from(payload))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    let parsed: Value = if body.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&body).to_string()))
    };
    (parts.status.as_u16(), parsed)
}

/// The `TracingConfig.Mode` a document reports, or `None` where it reports no mode at all.
fn reported_mode(reported: &Value) -> Option<&str> {
    reported
        .get("TracingConfig")
        .and_then(|tracing| tracing.get("Mode"))
        .and_then(Value::as_str)
}

/// An opaque deployment package: nothing here RUNS a handler, so what the archive holds does
/// not matter — only that the create carries one.
const ZIP_FILE: &str = "UEsFBgAAAAAAAAAAAAAAAAAAAAAAAA==";

/// One row: the mode the create names, and the mode an update then moves it to.
struct Row {
    case: &'static str,
    /// The `TracingConfig` the create carries, or `None` for a create that names none.
    created: Option<&'static str>,
    /// The mode every read of the created function must report.
    reported: &'static str,
    /// The mode an update then names, and which every later read must report.
    updated: &'static str,
}

const ROWS: &[Row] = &[
    Row {
        case: "a create naming Active, updated to PassThrough",
        created: Some("Active"),
        reported: "Active",
        updated: "PassThrough",
    },
    Row {
        case: "a create naming PassThrough, updated to Active",
        created: Some("PassThrough"),
        reported: "PassThrough",
        updated: "Active",
    },
    Row {
        case: "a create naming no tracing configuration at all, updated to Active",
        created: None,
        reported: DEFAULT_MODE,
        updated: "Active",
    },
];

#[tokio::test]
async fn the_tracing_mode_a_function_carries_is_the_one_its_caller_named() {
    let router = router();
    for (index, row) in ROWS.iter().enumerate() {
        let case = row.case;
        let name = format!("tracing-mode-{index}");
        let mut create = json!({
            "FunctionName": name,
            "Role": ROLE,
            "Runtime": "python3.12",
            "Handler": "lambda_function.lambda_handler",
            "Code": { "ZipFile": ZIP_FILE },
        });
        if let Some(mode) = row.created {
            create["TracingConfig"] = json!({ "Mode": mode });
        }
        let (status, created) = lambda(&router, "POST", FUNCTIONS_URI, Some(create)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from CreateFunction, got {status} ({created})"
        );
        assert_eq!(
            reported_mode(&created),
            Some(row.reported),
            "case {case:?}: expected the create to report the mode {:?}, got {created}",
            row.reported
        );
        // A created function is not modifiable inside the provisioning window the create
        // documents ("During this time, you can't invoke or modify the function"), which the
        // CLOCK closes.
        tokio::time::sleep(ACTIVATION_DELAY + SLACK).await;
        // Every READ of the function reports the same mode, which is what an invocation's own
        // trace context is decided from.
        let (status, read) = lambda(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from GetFunctionConfiguration, got {status} \
             ({read})"
        );
        assert_eq!(
            reported_mode(&read),
            Some(row.reported),
            "case {case:?}: expected a read to report the mode {:?}, got {read}",
            row.reported
        );
        // A PUBLISHED version FREEZES the mode it was published with, so an invocation that
        // resolves that version is traced as it was configured rather than as the unpublished
        // version is after the update below.
        let (status, published) = lambda(
            &router,
            "POST",
            &format!("{FUNCTIONS_URI}/{name}/versions"),
            Some(json!({})),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected HTTP 201 from PublishVersion, got {status} ({published})"
        );
        let version = published
            .get("Version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let (status, updated) = lambda(
            &router,
            "PUT",
            &format!("{FUNCTIONS_URI}/{name}/configuration"),
            Some(json!({ "TracingConfig": { "Mode": row.updated } })),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from UpdateFunctionConfiguration, got {status} \
             ({updated})"
        );
        assert_eq!(
            reported_mode(&updated),
            Some(row.updated),
            "case {case:?}: expected the update to report the mode {:?}, got {updated}",
            row.updated
        );
        let (status, frozen) = lambda(
            &router,
            "GET",
            &format!("{FUNCTIONS_URI}/{name}/configuration?Qualifier={version}"),
            None,
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 reading version {version}, got {status} \
             ({frozen})"
        );
        assert_eq!(
            reported_mode(&frozen),
            Some(row.reported),
            "case {case:?}: expected version {version} to keep the mode {:?} it was published \
             with, got {frozen}",
            row.reported
        );
    }
}
