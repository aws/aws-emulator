// SPDX-License-Identifier: CC0-1.0

//! The request/response harness every suite in this crate drives the service with.
//!
//! One copy, so two suites cannot drift apart over how a request is signed or how a
//! response is read: a suite adds only the per-operation helpers and the cases that
//! are its own.

use std::sync::Arc;

use bytes::Bytes;
use serde_json::Value;
use service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// A router carrying only this service, over one shared engine.
pub fn router() -> MultiServiceRouter {
    let engine = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.register_service(EmulatedService::new(engine));
    router
}

/// A request routed to this service by its SigV4 credential scope, which is also
/// where a handler reads the region from.
pub fn request(
    region: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> http::Request<Bytes> {
    let builder = http::Request::builder()
        .method(method)
        .uri(format!("http://localhost{path}"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDTESTONLY/20260101/{region}/es/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        );
    match body {
        Some(body) => builder
            .header("content-type", "application/json")
            .body(Bytes::from(serde_json::to_vec(body).expect("serialize body")))
            .expect("build request"),
        None => builder.body(Bytes::new()).expect("build request"),
    }
}

/// One call's HTTP status, `x-amzn-errortype` (restJson1 names the error shape there)
/// and parsed JSON body.
pub struct Answer {
    pub status: u16,
    pub error_type: String,
    pub body: Value,
}

impl Answer {
    /// The member at `pointer`, or a failure naming the case and the body.
    pub fn at(&self, case: &str, pointer: &str) -> &Value {
        self.body.pointer(pointer).unwrap_or_else(|| {
            panic!(
                "case {case:?}: response has no {pointer}, got {body}",
                body = self.body
            )
        })
    }

    /// The member at `pointer` as a string, or a failure naming the case.
    pub fn text(&self, case: &str, pointer: &str) -> String {
        self.at(case, pointer)
            .as_str()
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {pointer} is not a string, got {body}",
                    body = self.body
                )
            })
            .to_string()
    }

    /// Whether `pointer` is absent (or JSON null).
    pub fn missing(&self, pointer: &str) -> bool {
        self.body.pointer(pointer).is_none_or(Value::is_null)
    }
}

/// The endpoint THIS emulator answers on, as the framework states it for a router that
/// is not served on a port — which is what an in-process suite's router is, and so the
/// authority a published data-plane address carries here. Read from the framework rather
/// than spelled out, so the expectation is "this emulator's own endpoint" and not a
/// literal.
pub fn emulator_authority() -> &'static str {
    ::smithy_server_runtime::UNSERVED_URL_BASE
        .split_once("://")
        .map_or(::smithy_server_runtime::UNSERVED_URL_BASE, |(_, authority)| authority)
}

/// One call, signed for `region`.
pub async fn call_in(
    router: &MultiServiceRouter,
    region: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Answer {
    let response = router.dispatch(request(region, method, path, body)).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_string();
    let bytes = response.into_body();
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).to_string()))
    };
    Answer {
        status,
        error_type,
        body,
    }
}
