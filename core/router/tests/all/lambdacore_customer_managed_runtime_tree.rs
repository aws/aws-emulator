// SPDX-License-Identifier: CC0-1.0

//! A Lambda Core customer-managed RUNTIME call read back out of CloudTrail event history.
//!
//! RULE: every control-plane call an account's APIs serve is a management event of that
//! account, captured in event history whether or not a trail exists, and `readOnly` carries
//! the operation's own semantics — true exactly for the operations the model declares
//! `@readonly`. So after a successful call of a service whose model declares
//! `awsService.cloudTrailEventSource: lambda.amazonaws.com`, a lookup filtered on the
//! `eventName` the serving service publishes the operation's calls under answers an event
//! whose `CloudTrailEvent` carries that event source, the published `eventName`, that
//! `readOnly`, the Region the request was made in and the caller's account.
//!
//! Only a cross-service suite can pin this: the calls are served by Lambda Core
//! (`restJson1`), the records are read back through `cloudtrail:LookupEvents`
//! (`awsJson1_1`), and the two meet in the one shared management-event log of the engine
//! `core/router/src/lib.rs` mounts them both on. The Lambda-Core-side half of the rule —
//! which class each operation's calls are recorded with — is
//! `services/service-com-amazonaws-lambdacore-lambdacoreapiservice/src/service/management_events.rs`,
//! and the runtime tree's own behaviour is that crate's own suite.
//!
//! The rows span what the rule generalises over rather than the one create that motivated
//! it: over the runtime resource's whole surface, the four writes (a create, an update, a
//! delete and a restore) and the two `@readonly` reads (one runtime, the account's
//! runtimes). Every page of each lookup is read, paging `NextToken` to exhaustion, because a
//! record a client has to find is a record it finds however the service pages.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// The Region every request below is signed for, which is the Region the records are filed
/// in and the one the lookups are made in.
const REGION: &str = "eu-west-1";

/// The account every emulated resource and every record belongs to.
const ACCOUNT: &str = "000000000000";

/// The event source of every record below: the `awsService.cloudTrailEventSource` the Lambda
/// Core model declares.
const LAMBDA_EVENT_SOURCE: &str = "lambda.amazonaws.com";

/// The URI prefix every customer-managed runtime operation is bound under.
const PREFIX: &str = "/2026-10-01/runtimes";

/// The runtime every call below is made on.
const RUNTIME: &str = "audited.runtime";

/// How many events one lookup page carries, and how many pages are read at most: bounds on
/// this test, not on the API — a service that paged without end fails here rather than loops.
const PAGE: usize = 50;
const MAX_PAGES: usize = 50;

/// What the emulator answered: the HTTP status, the raw text and the parsed JSON body.
struct Answered {
    status: u16,
    text: String,
    body: Value,
}

/// Lambda Core and CloudTrail over one shared engine, exactly as `core/router/src/lib.rs`
/// mounts them. No trail is created: event history holds an account's management calls
/// "whether or not a trail exists", which is the fact this suite pins.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    // Signing is not the subject here, so requests are unauthenticated; the credential
    // scope is still carried, because it is what names the Region and the caller.
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService::new(Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine),
    );
    router
}

/// A SigV4 `Authorization` whose credential scope names `signing_name`, which is what routes
/// the request. The signature is never checked here.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261001/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=abc"
    )
}

/// Dispatch one request and read back what the emulator answered.
async fn dispatch(
    router: &MultiServiceRouter,
    method: &str,
    signing_name: &str,
    uri: &str,
    content_type: &str,
    body: Vec<u8>,
    headers: &[(&str, String)],
) -> Answered {
    let mut builder = ::http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
        .header("authorization", authorization(signing_name));
    if !body.is_empty() {
        builder = builder.header("content-type", content_type);
    }
    for (name, value) in headers {
        builder = builder.header(*name, value.as_str());
    }
    let request = builder
        .body(Bytes::from(body))
        .expect("the test request is well formed");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let text = String::from_utf8_lossy(&bytes).to_string();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Answered { status, text, body }
}

/// One Lambda Core call that must be served with `expected`.
async fn served(
    router: &MultiServiceRouter,
    case: &str,
    method: &str,
    uri: &str,
    body: Option<Value>,
    expected: u16,
) {
    let payload = body
        .map(|value| value.to_string().into_bytes())
        .unwrap_or_default();
    let answered = dispatch(router, method, "lambda", uri, "application/json", payload, &[]).await;
    assert_eq!(
        answered.status, expected,
        "setup {case:?}: expected HTTP {expected}, got {} with {}",
        answered.status, answered.text
    );
}

/// Every `CloudTrailEvent` document of the events whose `EventName` is `event_name`, paging
/// the lookup's `NextToken` to exhaustion.
async fn looked_up(router: &MultiServiceRouter, case: &str, event_name: &str) -> Vec<Value> {
    let mut found: Vec<Value> = Vec::new();
    let mut next: Option<String> = None;
    for _ in 0..MAX_PAGES {
        let mut request = json!({
            "LookupAttributes": [{ "AttributeKey": "EventName", "AttributeValue": event_name }],
            "MaxResults": PAGE,
        });
        if let Some(token) = &next {
            request["NextToken"] = json!(token);
        }
        let answered = dispatch(
            router,
            "POST",
            "cloudtrail",
            "/",
            "application/x-amz-json-1.1",
            request.to_string().into_bytes(),
            &[(
                "x-amz-target",
                "CloudTrail_20131101.LookupEvents".to_string(),
            )],
        )
        .await;
        assert_eq!(
            answered.status, 200,
            "case {case:?}: expected HTTP 200 from CloudTrail LookupEvents, got {} with {}",
            answered.status, answered.text
        );
        for event in answered.body["Events"].as_array().unwrap_or(&Vec::new()) {
            let document = event["CloudTrailEvent"].as_str().unwrap_or_else(|| {
                panic!("case {case:?}: every event carries its CloudTrailEvent, got {event}")
            });
            found.push(serde_json::from_str(document).unwrap_or_else(|error| {
                panic!("case {case:?}: a CloudTrailEvent is JSON ({error}): {document}")
            }));
        }
        next = answered.body["NextToken"].as_str().map(ToString::to_string);
        if next.is_none() {
            return found;
        }
    }
    panic!("case {case:?}: the lookup must page to exhaustion")
}

/// RULE, both halves: every control-plane call of this service's runtime surface is recorded
/// in the account's event history, with `readOnly` true exactly for the `@readonly`
/// operations.
#[tokio::test]
async fn a_customer_managed_runtime_call_is_recorded_as_a_management_event() {
    let router = router();

    served(
        &router,
        "the create",
        "POST",
        PREFIX,
        Some(json!({ "Name": RUNTIME, "Description": "audited" })),
        202,
    )
    .await;
    served(
        &router,
        "the update",
        "PUT",
        &format!("{PREFIX}/{RUNTIME}"),
        Some(json!({ "Description": "audited twice" })),
        200,
    )
    .await;
    served(
        &router,
        "the read of one runtime",
        "GET",
        &format!("{PREFIX}/{RUNTIME}"),
        None,
        200,
    )
    .await;
    served(&router, "the list over the account", "GET", PREFIX, None, 200).await;
    served(
        &router,
        "the delete",
        "DELETE",
        &format!("{PREFIX}/{RUNTIME}"),
        None,
        202,
    )
    .await;
    served(
        &router,
        "the restore",
        "POST",
        &format!("{PREFIX}/{RUNTIME}/restore"),
        None,
        200,
    )
    .await;

    let rows = [
        ("the create", "CreateRuntime", false),
        ("the update", "UpdateRuntime", false),
        ("the delete", "DeleteRuntime", false),
        ("the restore", "RestoreRuntime", false),
        ("the read of one runtime", "GetRuntime", true),
        ("the list over the account", "ListRuntimes", true),
    ];
    for (case, event_name, read_only) in rows {
        let found = looked_up(&router, case, event_name).await;
        let document = found
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: event history must hold a {event_name}"));
        assert_eq!(
            document.get("eventSource"),
            Some(&json!(LAMBDA_EVENT_SOURCE)),
            "case {case:?}: the record names the service's modelled event source, got {document}"
        );
        assert_eq!(
            document.get("eventName"),
            Some(&json!(event_name)),
            "case {case:?}: the record names the operation, got {document}"
        );
        assert_eq!(
            document.get("readOnly"),
            Some(&json!(read_only)),
            "case {case:?}: expected readOnly {read_only}, got {document}"
        );
        assert_eq!(
            document.get("errorCode"),
            None,
            "case {case:?}: a served call carries no errorCode, got {document}"
        );
        assert_eq!(
            document.get("awsRegion"),
            Some(&json!(REGION)),
            "case {case:?}: the record is filed in the Region the call was made in, got \
             {document}"
        );
        assert_eq!(
            document.get("recipientAccountId"),
            Some(&json!(ACCOUNT)),
            "case {case:?}: the record names the caller's account, got {document}"
        );
        assert!(
            document
                .get("eventID")
                .and_then(Value::as_str)
                .is_some_and(|id| !id.is_empty()),
            "case {case:?}: every record carries an eventID, got {document}"
        );
    }

    // A name the model does not bind is recorded nowhere, so the lookup that asks for one
    // answers no event rather than any event.
    let none = looked_up(&router, "a name this service does not serve", "PublishLayerVersion").await;
    assert!(
        none.is_empty(),
        "a lookup for an operation nothing served answers no event, got {none:?}"
    );
}
