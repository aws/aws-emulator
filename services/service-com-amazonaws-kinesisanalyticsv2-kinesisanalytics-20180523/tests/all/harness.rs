// SPDX-License-Identifier: CC0-1.0

//! The wire harness every suite in this crate drives the service through: `awsJson1_1`,
//! `POST /` with `X-Amz-Target: KinesisAnalytics_20180523.<Operation>`, dispatched by the
//! same `MultiServiceRouter` the emulator boots.
//!
//! It holds ONLY the plumbing — build a router, call an operation, read a member of the
//! answer, create an application a case needs. Every assertion about behaviour lives in
//! the suite that makes it, and both suites read one response envelope here, so the two
//! cannot drift apart on what `__type` or a message member is.

use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The emulator account id every ARN this service mints carries, and the account its
/// refusals name.
pub(crate) const ACCOUNT: &str = "000000000000";

/// Longer than the span the service gives a clock-owed status transition, so a read after
/// this wait must see the settled status.
pub(crate) const SETTLE_WAIT: Duration = Duration::from_millis(1_200);

/// A router with this one service mounted on `engine`, so a test can inspect the same
/// store the handlers write to.
pub(crate) fn router_on(engine: Arc<engine::InMemoryEngine>) -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    // In-process tests sign with a placeholder key; signature verification is a front-door
    // setting, not part of what these suites are about.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::EmulatedService::new(
            engine,
        ),
    );
    router
}

/// A router with this one service mounted on a fresh shared engine.
pub(crate) fn router() -> MultiServiceRouter {
    router_on(Arc::new(engine::InMemoryEngine::new()))
}

/// Call one operation and return `(http status, parsed body)`.
pub(crate) async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    op: &str,
    body: &Value,
) -> (u16, Value) {
    let request = http::Request::builder()
        .method("POST")
        .uri("/")
        .header("x-amz-target", format!("KinesisAnalytics_20180523.{op}"))
        .header("content-type", "application/x-amz-json-1.1")
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20240101/{region}/kinesisanalytics/\
                 aws4_request, SignedHeaders=host;x-amz-date, Signature=deadbeef"
            ),
        )
        .body(bytes::Bytes::from(
            serde_json::to_vec(body).expect("the request body serializes"),
        ))
        .expect("a valid request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let parsed = if response.body().is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(response.body()).unwrap_or_else(|e| {
            panic!(
                "{op} answered a body that is not JSON ({e}): {}",
                String::from_utf8_lossy(response.body())
            )
        })
    };
    (status, parsed)
}

/// Call one operation in the default test region.
pub(crate) async fn call(router: &MultiServiceRouter, op: &str, body: &Value) -> (u16, Value) {
    call_in(router, "us-east-1", op, body).await
}

/// The wire error code of an error body (`__type` on awsJson1_1), as a bare shape name.
pub(crate) fn error_code(body: &Value) -> String {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or("<no __type>")
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// The message of an error body, wherever this service's envelope carries it: a MODELLED
/// exception declares `Message`, while the boundary validator's own refusal — an error no
/// operation models — carries `message`.
pub(crate) fn error_message(body: &Value) -> String {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A minimal valid `CreateApplication` body.
pub(crate) fn create_body(name: &str, runtime: &str) -> Value {
    json!({
        "ApplicationName": name,
        "RuntimeEnvironment": runtime,
        "ServiceExecutionRole": "arn:aws:iam::000000000000:role/kda-service-role",
    })
}

/// `body` with the inline code content a streaming application needs before
/// `StartApplication` accepts it (an application stored with none has no valid
/// configuration to start, which `modeled_client_errors.rs` asserts).
pub(crate) fn with_code_content(mut body: Value) -> Value {
    body["ApplicationConfiguration"] = json!({
        "ApplicationCodeConfiguration": {
            "CodeContentType": "PLAINTEXT",
            "CodeContent": {"TextContent": "-- the application's code"}
        }
    });
    body
}

/// Create an application and return the `ApplicationDetail` the service answered with.
pub(crate) async fn create(router: &MultiServiceRouter, name: &str, runtime: &str) -> Value {
    let (status, body) = call(router, "CreateApplication", &create_body(name, runtime)).await;
    assert_eq!(status, 200, "case {name:?}: CreateApplication got {body}");
    body["ApplicationDetail"].clone()
}

/// Create an application that can be STARTED: [`create`] with code content stored.
pub(crate) async fn create_startable(
    router: &MultiServiceRouter,
    name: &str,
    runtime: &str,
) -> Value {
    let (status, body) = call(
        router,
        "CreateApplication",
        &with_code_content(create_body(name, runtime)),
    )
    .await;
    assert_eq!(status, 200, "case {name:?}: CreateApplication got {body}");
    body["ApplicationDetail"].clone()
}

/// The ARN of an application this service stored in the default test region, the value a
/// caller reads off `ApplicationDetail$ApplicationARN` and hands to a tagging call.
pub(crate) fn application_arn(name: &str) -> String {
    format!("arn:aws:kinesisanalytics:us-east-1:{ACCOUNT}:application/{name}")
}

/// `ListTagsForResource` for one ARN, as `(http status, body)`.
pub(crate) async fn list_tags(router: &MultiServiceRouter, resource_arn: &str) -> (u16, Value) {
    call(
        router,
        "ListTagsForResource",
        &json!({"ResourceARN": resource_arn}),
    )
    .await
}

/// The tags an application carries, as a `(key, value)` table read through
/// `ListTagsForResource` in the order the service reports them. Shared plumbing, so no two
/// suites that read one application's tags back can drift on where a tag carries its key.
pub(crate) async fn tags_of(router: &MultiServiceRouter, name: &str) -> Vec<(String, String)> {
    let arn = application_arn(name);
    let (status, body) = list_tags(router, &arn).await;
    assert_eq!(status, 200, "case {name:?}: ListTagsForResource got {body}");
    body["Tags"]
        .as_array()
        .unwrap_or_else(|| panic!("case {name:?}: expected a Tags member, got {body}"))
        .iter()
        .map(|tag| {
            (
                tag["Key"].as_str().unwrap_or_default().to_string(),
                tag["Value"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// Every `SnapshotName` `ListApplicationSnapshots` reports for one application, in the
/// order it reports them, asserting the call succeeded. Shared plumbing, so no two suites
/// that read one application's snapshots back can drift on where a summary carries its
/// name.
pub(crate) async fn listed_snapshots(
    router: &MultiServiceRouter,
    case: &str,
    application: &str,
) -> Vec<String> {
    let (status, page) = call(
        router,
        "ListApplicationSnapshots",
        &json!({"ApplicationName": application}),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case}: ListApplicationSnapshots got {status} {page}"
    );
    page["SnapshotSummaries"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected the summaries member, got {page}"))
        .iter()
        .map(|summary| {
            summary["SnapshotName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// Describe an application, asserting the call succeeded.
pub(crate) async fn describe(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(
        router,
        "DescribeApplication",
        &json!({"ApplicationName": name}),
    )
    .await;
    assert_eq!(status, 200, "case {name:?}: DescribeApplication got {body}");
    body["ApplicationDetail"].clone()
}
