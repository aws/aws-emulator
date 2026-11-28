// SPDX-License-Identifier: CC0-1.0

//! Which requests this service CLAIMS — the probe of the one seam that decides whether a
//! Global Inference Gateway request reaches this service at all.
//!
//! Every SageMaker-family model declares the sigv4 name `sagemaker` while serving its own
//! endpoint, and the router resolves a request's routing key from the SigV4 credential
//! scope first, so each of them sees the key `sagemaker`. A service that claimed on the
//! signing name alone would claim its siblings' traffic too, and the router — which refuses
//! rather than guesses — would answer `500 AmbiguousService` for all of it. So this
//! service claims the key AND its own modelled path space, and the rows below probe that
//! rule along what it generalises over: the routing key the request is signed for, which
//! path space it addresses, and how deep into this service's own space it goes.
//!
//! The path space comes from the model's `smithy.api#http` URIs, which the build emits as
//! `ROUTE_PATH_SEGMENTS`; the siblings' spaces (`/endpoints/…`, `/inference-routers/…`,
//! `/human-loops…`) are the ones this service must NOT claim. Which service claims each of
//! those is pinned on the assembled router, in
//! `core/router/tests/all/sagemaker_gateway_runtime_signing_name_routing.rs`.

use std::sync::Arc;

use bytes::Bytes;
use service_com_amazonaws_sagemakergatewayruntime_globalinferencegatewayruntime as gateway_runtime;
use smithy_server_runtime::Service;

/// A SigV4 `Authorization` header scoped to the signing name the SageMaker family shares.
const SAGEMAKER_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTSAGEMAKER0000/20260101/us-east-1/sagemaker/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The same header scoped to ANOTHER service's signing name: a request this service must
/// not claim however its path reads.
const OTHER_SERVICE_SCOPE: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDTESTOTHERSERVICE0/20260101/us-east-1/bedrock/aws4_request, \
     SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The request parts the router hands to `Service::can_handle`.
fn parts(method: &str, uri: &str, authorization: &str) -> http::request::Parts {
    http::Request::builder()
        .method(method)
        .uri(uri)
        .header("host", "localhost:9297")
        .header("content-type", "application/json")
        .header("authorization", authorization)
        .body(Bytes::new())
        .expect("the test request is well formed")
        .into_parts()
        .0
}

#[test]
fn this_service_claims_its_own_path_space_under_its_own_signing_name() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = gateway_runtime::EmulatedService::new(engine);

    // (case, method, uri, the credential scope, whether this service claims it)
    let rows: &[(&str, &str, &str, &str, bool)] = &[
        (
            "the buffered route of the model",
            "POST",
            "/global-inference-gateway/default/invocations",
            SAGEMAKER_SCOPE,
            true,
        ),
        (
            "the streaming route of the model",
            "POST",
            "/global-inference-gateway/default/invocations-response-stream",
            SAGEMAKER_SCOPE,
            true,
        ),
        (
            "this service's path space with no route under it",
            "POST",
            "/global-inference-gateway",
            SAGEMAKER_SCOPE,
            true,
        ),
        (
            "a method the model binds no route for, in this service's space",
            "GET",
            "/global-inference-gateway/default/invocations",
            SAGEMAKER_SCOPE,
            true,
        ),
        (
            "the endpoint runtime's path space",
            "POST",
            "/endpoints/endpoint1/invocations",
            SAGEMAKER_SCOPE,
            false,
        ),
        (
            "the inference router runtime's path space",
            "POST",
            "/inference-routers/router1/invocations",
            SAGEMAKER_SCOPE,
            false,
        ),
        (
            "the A2I runtime's path space",
            "POST",
            "/human-loops",
            SAGEMAKER_SCOPE,
            false,
        ),
        (
            "a path space no SageMaker model publishes",
            "POST",
            "/nothing-here",
            SAGEMAKER_SCOPE,
            false,
        ),
        (
            "this service's own route, signed for another service",
            "POST",
            "/global-inference-gateway/default/invocations",
            OTHER_SERVICE_SCOPE,
            false,
        ),
    ];

    for (case, method, uri, authorization, claimed) in rows {
        let actual = service.can_handle(&parts(method, uri, authorization));
        assert_eq!(
            actual, *claimed,
            "case {case:?} ({method} {uri}): expected this service to claim it = {claimed}, \
             got {actual}"
        );
    }
}
