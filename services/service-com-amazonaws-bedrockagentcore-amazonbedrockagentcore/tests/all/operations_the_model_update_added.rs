// SPDX-License-Identifier: CC0-1.0

//! The operations the model UPDATE added, and what the emulator answers for them today.
//!
//! THE RULE: an operation this service's model binds is ROUTED — the request reaches the
//! operation rather than the framework's unroutable path — and one no handler implements
//! yet answers HTTP 501 `NotImplemented`, never an empty success and never a refusal that
//! claims the route does not exist. That is the scaffold's contract
//! (`crate::scaffolded`): a caller, and any check that only asserts a call succeeded,
//! must be able to tell "nobody has written this yet" from a stored answer.
//!
//! The rows are every operation the update added, which is what makes this suite the
//! census of that contract for them: the four of the dataset-generation family (a start, a
//! read, a listing and a delete, so a write route and a read route each), the three agent
//! insight operations (a read, an update and a listing) and the web search invocation.
//! Each row carries a request the model admits — the boundary validator runs BEFORE the
//! handler, so a request it refuses would never reach the stub and the row would assert
//! nothing about it.
//!
//! When one of these operations is implemented, its row moves to that operation's own
//! suite; this one holds only what is still a stub.

use std::sync::Arc;

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore as agentcore;

/// The wire error code the scaffold's 501 carries.
const NOT_IMPLEMENTED: &str = "NotImplemented";

/// The emulator, with request authentication off so a test signs nothing.
fn emulator() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(agentcore::EmulatedService::new(engine));
    router
}

/// Send one request and return its status, the `x-amzn-errortype` header and the parsed
/// body.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, String, Value) {
    let payload = body.map_or_else(bytes::Bytes::new, |v| bytes::Bytes::from(v.to_string()));
    let request = http::Request::builder()
        .method(method)
        .uri(format!(
            "https://bedrock-agentcore.us-east-1.amazonaws.com{path}"
        ))
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-east-1/bedrock-agentcore/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .header("content-type", "application/json")
        .body(payload)
        .expect("build the request");
    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, error_type, parsed)
}

/// The wire error code: the `x-amzn-errortype` header `restJson1` sets, else the body's
/// `__type`.
fn error_code(error_type: &str, body: &Value) -> String {
    if !error_type.is_empty() {
        return error_type.to_string();
    }
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit('#')
        .next()
        .unwrap_or_default()
        .to_string()
}

/// A dataset the generation routes address. `DatasetId` is `^[a-zA-Z0-9_-]{1,110}$`.
const DATASET_ID: &str = "demo_dataset";

/// A generation the read and the delete address. `DatasetGenerationId` is
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`.
const GENERATION_ID: &str = "nightly-1a2b3c4d5e";

/// An insight the read and the update address. `AgentInsightId` is `^[a-z0-9]+$`.
const AGENT_INSIGHT_ID: &str = "insight01";

/// A web search the invocation addresses. `WebSearchId` is
/// `^[a-z0-9]+([.-][a-z0-9]+)*$`.
const WEB_SEARCH_ID: &str = "aws-docs";

/// Every operation the model update added, with a request the model admits. Source for
/// each route: this crate's `OPERATIONS.md` (the `Route:` line of the operation).
fn added_operations() -> Vec<(&'static str, &'static str, String, Option<Value>)> {
    vec![
        (
            "StartDatasetGeneration",
            "POST",
            format!("/datasets/{DATASET_ID}/generations"),
            Some(json!({
                "name": "nightly_run",
                "scenarioType": "FIXED",
                "source": {"traceGeneration": {"sourceConfig": {"cloudWatchLogs": {
                    "serviceNames": ["checkout-agent"],
                    "logGroupNames": ["/aws/bedrock-agentcore/checkout"],
                }}}},
            })),
        ),
        (
            "GetDatasetGeneration",
            "GET",
            format!("/datasets/{DATASET_ID}/generations/{GENERATION_ID}"),
            None,
        ),
        (
            "ListDatasetGenerations",
            "GET",
            format!("/datasets/{DATASET_ID}/generations"),
            None,
        ),
        (
            "DeleteDatasetGeneration",
            "DELETE",
            format!("/datasets/{DATASET_ID}/generations/{GENERATION_ID}"),
            None,
        ),
        (
            "GetAgentInsight",
            "GET",
            format!("/agentinsights/{AGENT_INSIGHT_ID}?applicationName=checkout-agent"),
            None,
        ),
        (
            "UpdateAgentInsight",
            "PUT",
            format!("/agentinsights/{AGENT_INSIGHT_ID}?applicationName=checkout-agent"),
            Some(json!({"archiveStatus": "ARCHIVED"})),
        ),
        (
            "ListAgentInsights",
            "GET",
            "/agentinsights?applicationName=checkout-agent".to_string(),
            None,
        ),
        (
            "InvokeWebSearch",
            "POST",
            format!("/web-search/{WEB_SEARCH_ID}/invoke"),
            Some(json!({"searchQuery": "what is an agent runtime", "maxResults": 5})),
        ),
    ]
}

/// Each operation the update added is routed, and answers the scaffold's 501 rather than
/// an empty success or a claim that the route does not exist.
#[tokio::test]
async fn every_operation_the_model_update_added_is_routed_and_answers_not_implemented() {
    let router = emulator();

    for (case, method, path, body) in added_operations() {
        let (status, error_type, answered) = send(&router, method, &path, body).await;
        assert_eq!(
            status, 501,
            "case {case:?}: expected HTTP 501 for an operation nothing implements yet, got \
             {status} with {answered}"
        );
        assert_eq!(
            error_code(&error_type, &answered),
            NOT_IMPLEMENTED,
            "case {case:?}: expected the error code {NOT_IMPLEMENTED}, got {answered}"
        );
        let message = answered
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            message.contains(case),
            "case {case:?}: the 501 must name the operation it is scaffolded for, got \
             {message:?}"
        );
    }
}
