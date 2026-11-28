// SPDX-License-Identifier: CC0-1.0

//! EVERY operation this model declares is reachable through the claim seam.
//!
//! The claim rule itself — that a service under a signing name it shares claims only its own
//! wire surface — is pinned by the suite beside this one (`signing_name_claim`). What this one
//! pins is its COVERAGE: the rule is stated over the protocol's path form rather than over any
//! operation, so every operation the model declares is claimed on its own path, and none of the
//! eighteen is reachable only because it happens to be the one a form row names.
//!
//! The operation list is this model's own (`smithy_query(query="list_operations")` on
//! `services/service-com-amazonaws-elbproxy-elbproxy/smithy-model/com-amazonaws-elbproxy-elbproxy.json`),
//! and each row is addressed the way its `rpcv2Cbor` protocol addresses one:
//! `POST /service/<Service>/operation/<Operation>`, signed under the shared signing name.

use std::sync::Arc;

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// The signing name this service shares with both Elastic Load Balancing models
/// (`smithy_query(query="service_info")`: `sigv4Name: elasticloadbalancing`).
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// Every operation this model declares. Source:
/// `smithy_query(query="list_operations",model_path="services/service-com-amazonaws-elbproxy-elbproxy/smithy-model/com-amazonaws-elbproxy-elbproxy.json")`.
const OPERATIONS: [&str; 18] = [
    "AssociateCertificate",
    "CreateProxy",
    "DeleteProxy",
    "DisassociateCertificate",
    "GetProxy",
    "GetProxyAttributes",
    "GetProxyConfigurationContents",
    "GetProxyUpdate",
    "ListCertificateAssociations",
    "ListProxies",
    "ListProxyUpdates",
    "ListProxyVersions",
    "ListTagsForResource",
    "TagResource",
    "UntagResource",
    "UpdateProxyAttributes",
    "UpdateProxyConfiguration",
    "UpdateProxyNetworkConfiguration",
];

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. No signature is
/// verified here: the claim seam reads what the request is addressed to.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/\
         aws4_request, SignedHeaders=host, Signature=0000"
    )
}

/// Whether this service claims a request addressed to `path` and signed under the shared name.
fn claims(service: &EmulatedService<engine::InMemoryEngine>, path: &str) -> bool {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("https://elb-proxy.us-east-1.amazonaws.com{path}"))
        .header("host", "elb-proxy.us-east-1.amazonaws.com")
        .header("authorization", authorization(SHARED_SIGNING_NAME))
        .header("content-type", "application/cbor")
        .body(bytes::Bytes::new())
        .expect("build the request");
    let (parts, _body) = request.into_parts();
    service.can_handle(&parts)
}

/// RULE: the claim is stated over this model's own protocol path, so EVERY operation it declares
/// is claimed when a client addresses that operation's own path.
#[test]
fn every_operation_this_model_declares_is_claimed_on_its_own_path() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let service = EmulatedService::new(engine);

    for operation in OPERATIONS {
        let path = format!("/service/{SERVICE_SHAPE}/operation/{operation}");
        let claimed = claims(&service, &path);
        assert!(
            claimed,
            "case {operation:?}: expected this service to claim {path:?}, got {claimed}"
        );
    }
}
