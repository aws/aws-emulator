// SPDX-License-Identifier: CC0-1.0

//! The other routing SIGNAL a request under the shared signing name can carry: its `Host`.
//!
//! The sibling suite `super::signing_name_claim` drives the claim rule over the request's
//! PATH and `X-Amz-Target` with a loopback host, where the SigV4 credential scope is what
//! the routing key comes from. A request can also resolve its key from the host label it was
//! addressed to — an SDK pointed at an AWS endpoint host, and a request carrying no
//! credential at all — so these rows vary the host while the path stays this model's own
//! `rpcv2Cbor` surface, and the one in the middle carries no `Authorization` header.
//!
//! The rule is the sibling's: the routing key has to name the signing name this service
//! shares with both Elastic Load Balancing models, and the request has to address THIS
//! service's own wire surface — which its `rpcv2Cbor` path names
//! (`/service/<Service>/operation/<Operation>`).

use service_com_amazonaws_elbproxy_elbproxy::EmulatedService;
use smithy_server_runtime::Service;
use std::sync::Arc;

/// The local name of this model's service shape, which an `rpcv2Cbor` request names in its
/// path (`smithy_query(query="service_info")` reports the service as `ELBProxy`).
const SERVICE_SHAPE: &str = "ELBProxy";

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. Nothing
/// verifies the signature here: the claim seam reads the scope's service field only.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261120/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000"
    )
}

/// RULE: under the shared signing name, the claim follows the routing key the request
/// resolves — from its credential scope, or from the host label when it carries none — and
/// this service's own `rpcv2Cbor` path.
#[tokio::test]
async fn the_host_a_request_was_addressed_to_resolves_the_claim_the_same_way() {
    let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
    let own_path = format!("/service/{SERVICE_SHAPE}/operation/ListProxies");

    // (case, the host the request was addressed to, the signing name its scope names or
    // `None` for a request carrying no credential, whether this service claims it)
    let rows: [(&str, &str, Option<&str>, bool); 5] = [
        (
            "the Elastic Load Balancing endpoint host, signed for the shared name",
            "elasticloadbalancing.us-east-1.amazonaws.com",
            Some("elasticloadbalancing"),
            true,
        ),
        (
            "the shared endpoint host, carrying no credential at all",
            "elasticloadbalancing.us-east-1.amazonaws.com",
            None,
            true,
        ),
        (
            // The scope decides, not the host: a request signed for the ELBv2 model's own
            // key names that model wherever it was addressed.
            "the shared endpoint host, signed for the ELBv2 model's own key",
            "elasticloadbalancing.us-east-1.amazonaws.com",
            Some("elasticloadbalancingv2"),
            false,
        ),
        (
            "an unrelated service's endpoint host, carrying no credential",
            "sqs.us-east-1.amazonaws.com",
            None,
            false,
        ),
        (
            "a loopback host, signed for the shared name",
            "localhost:9297",
            Some("elasticloadbalancing"),
            true,
        ),
    ];

    for (case, host, signing_name, expected) in rows {
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(own_path.as_str())
            .header("host", host)
            .header("content-type", "application/cbor");
        if let Some(signing_name) = signing_name {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let (parts, ()) = builder.body(()).expect("build the request").into_parts();
        let claimed = service.can_handle(&parts);
        assert_eq!(
            claimed, expected,
            "case {case:?} (POST {own_path} to host {host}, signed for {signing_name:?}): \
             expected can_handle to be {expected}, got {claimed}"
        );
    }
}
