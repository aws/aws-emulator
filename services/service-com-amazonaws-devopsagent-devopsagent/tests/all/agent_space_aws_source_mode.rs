// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for an AgentSpace's `awsSourceMode`: whether its Amazon
//! Web Services source configuration targets one account or an Organization.
//!
//! The rule under test: the mode a create configures is persisted and reported
//! unchanged by every later read, and a space created without one carries none —
//! `AgentSpace$awsSourceMode` has no default.
//!
//! Every case drives the service through the same router the emulator serves, so
//! the routes, status codes, protocol envelope and state are all exercised.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

/// A SigV4 credential scope naming this service's signing name (`aidevops`), which
/// is how the router resolves which service claims a request. Signature
/// verification is switched off for these tests, so the signature is a placeholder.
const AUTHORIZATION: &str = concat!(
    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/us-west-2/aidevops/aws4_request, ",
    "SignedHeaders=host, Signature=0123456789abcdef"
);

/// The `AwsSourceMode` value set, from the model's enum
/// (`smithy_query(query="shape", shape="AwsSourceMode")`).
const AWS_SOURCE_MODES: &[&str] = &["standalone", "organization"];

/// One emulator: a single shared engine behind the production router.
fn new_router() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(
        service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(Arc::new(
            engine::InMemoryEngine::new(),
        )),
    );
    router
}

/// Send one request and return its status and parsed body (`Value::Null` for an
/// empty body).
async fn call(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let payload = body
        .as_ref()
        .map(|b| Bytes::from(serde_json::to_vec(b).expect("serialize request body")))
        .unwrap_or_default();
    let mut request = http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "aidevops.us-west-2.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    let request = request.body(payload).expect("build request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let bytes = response.into_body();
    let parsed = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or_else(|e| {
            panic!(
                "response body is not JSON ({e}): {}",
                String::from_utf8_lossy(&bytes)
            )
        })
    };
    (status, parsed)
}

/// The mode a create configures is what every read of the space reports; a create
/// that configures none leaves the member absent.
#[tokio::test]
async fn aws_source_mode_round_trips_through_create_and_get() {
    let router = new_router();

    // One row per value the model declares, plus the row that names none.
    let mut rows: Vec<(String, Option<&str>)> = vec![("absent".to_string(), None)];
    rows.extend(
        AWS_SOURCE_MODES
            .iter()
            .map(|mode| ((*mode).to_string(), Some(*mode))),
    );

    for (case, mode) in rows {
        let mut body = json!({"name": format!("source-mode-{case}")});
        if let Some(mode) = mode {
            body["awsSourceMode"] = json!(mode);
        }
        let (status, created) = call(&router, "POST", "/v1/agentspaces", Some(body)).await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected 201 from CreateAgentSpace, got {status}: {created}"
        );
        let expected = mode.map_or(Value::Null, |mode| json!(mode));
        assert_eq!(
            created["agentSpace"]["awsSourceMode"], expected,
            "case {case:?}: the create reports awsSourceMode {:?}, not {expected:?}",
            created["agentSpace"]["awsSourceMode"]
        );

        let space_id = created["agentSpace"]["agentSpaceId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: no agentSpaceId in {created}"));
        let (status, read) =
            call(&router, "GET", &format!("/v1/agentspaces/{space_id}"), None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200 from GetAgentSpace, got {status}: {read}"
        );
        assert_eq!(
            read["agentSpace"]["awsSourceMode"], expected,
            "case {case:?}: the read reports awsSourceMode {:?}, not {expected:?}",
            read["agentSpace"]["awsSourceMode"]
        );
    }
}
