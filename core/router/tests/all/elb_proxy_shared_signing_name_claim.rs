// SPDX-License-Identifier: CC0-1.0

//! THREE emulated services declare the SigV4 signing name `elasticloadbalancing`.
//!
//! Classic Elastic Load Balancing, Elastic Load Balancing v2 and ELB Proxy all declare
//! `aws.auth#sigv4` name `elasticloadbalancing` (`smithy_query(query="service_info")` on
//! the three models) while each serves its own AWS endpoint, and the router resolves a
//! request's routing key from the SigV4 credential scope first, so all three see that one
//! key. A service that claims the shared name ALONE claims its siblings' traffic too: the
//! router then finds several claimants and answers `500 AmbiguousService`, a code no AWS
//! model declares, in a body an awsQuery client cannot parse — so none of the APIs is
//! reachable at all.
//!
//! The rule this suite pins, over the awsQuery traffic BOTH Elastic Load Balancing APIs
//! arrive in: such a request reaches EXACTLY ONE service — the front door whose own
//! identity is the shared name for the shared key, and each sibling under the key that
//! names it — and a service whose modelled wire surface the request does not address (ELB
//! Proxy, whose `rpcv2Cbor` operations arrive on their own RPC path) claims none of it.
//!
//! The claim is read through `Service::can_handle`, the one decision the router makes
//! before it dispatches, so the rows need no stored state and no handler: a row with two
//! claimants is the `AmbiguousService` refusal and a row with none is `UnknownService`.
//!
//! The sibling table for the two Elastic Load Balancing API versions behind the shared
//! front door is `elb_classic_and_v2_signing_name_routing.rs`.

use std::sync::Arc;

use bytes::Bytes;
use smithy_server_runtime::Service;

/// The signing name all three models declare.
const SHARED_NAME: &str = "elasticloadbalancing";

/// The Smithy shape ids of the three services, which is how a claimant is named below.
const CLASSIC: &str = "com.amazonaws.elasticloadbalancing#ElasticLoadBalancing_v7";
const V2: &str = "com.amazonaws.elasticloadbalancingv2#ElasticLoadBalancing_v10";
const PROXY: &str = "com.amazonaws.elbproxy#ELBProxy";

/// How a request names the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 credential scope naming this routing key — what every AWS SDK sends, and
    /// the signal the router reads first.
    Signed(&'static str),
    /// A host-style endpoint and no credentials: the last signal, which an anonymous
    /// request still carries.
    Host(&'static str),
}

/// One row: an awsQuery request of the Elastic Load Balancing APIs, and the service that
/// must be its ONE claimant.
struct Row {
    case: &'static str,
    addressed: Addressed,
    method: &'static str,
    path: &'static str,
    claimant: &'static str,
}

/// A SigV4 `Authorization` header whose credential scope names `signing_name`. The
/// signature is never checked here: the claim is read off the request's parts.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0000000000000000000000000000000000000000000000000000000000000000"
    )
}

/// The three services that declare the shared signing name, over ONE engine, as
/// `core/router/src/lib.rs` mounts them.
fn elastic_load_balancing_family(engine: &Arc<engine::InMemoryEngine>) -> Vec<Box<dyn Service>> {
    vec![
        Box::new(
            service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(
            service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
                Arc::clone(engine),
            ),
        ),
        Box::new(service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(
            Arc::clone(engine),
        )),
    ]
}

/// Every service of the family that claims this row's request, by shape id.
fn claimants(family: &[Box<dyn Service>], row: &Row) -> Vec<String> {
    let host = match row.addressed {
        Addressed::Host(host) => host,
        Addressed::Signed(_) => "emulator.local",
    };
    let mut builder = http::Request::builder()
        .method(row.method)
        .uri(format!("https://{host}{}", row.path))
        .header("host", host);
    if let Addressed::Signed(signing_name) = row.addressed {
        builder = builder.header("authorization", authorization(signing_name));
    }
    let request = builder.body(Bytes::new()).expect("build the request");
    let (parts, _body) = request.into_parts();
    family
        .iter()
        .filter(|service| service.can_handle(&parts))
        .map(|service| service.definition().identity().shape_id().to_string())
        .collect()
}

#[test]
fn every_elastic_load_balancing_request_has_exactly_one_claimant() {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let family = elastic_load_balancing_family(&engine);

    let rows = [
        Row {
            // The awsQuery form both Elastic Load Balancing APIs arrive in, signed for the
            // shared name: the front door whose own identity is that name owns it, and
            // serves either API version out of the one envelope's `Version`.
            case: "an awsQuery request signed for the shared name",
            addressed: Addressed::Signed(SHARED_NAME),
            method: "POST",
            path: "/",
            claimant: CLASSIC,
        },
        Row {
            case: "an awsQuery request addressed to the shared endpoint by host",
            addressed: Addressed::Host("elasticloadbalancing.us-east-1.amazonaws.com"),
            method: "POST",
            path: "/",
            claimant: CLASSIC,
        },
        Row {
            // Elastic Load Balancing v2 stays reachable under the key that names IT.
            case: "a request signed for the Elastic Load Balancing v2 key",
            addressed: Addressed::Signed("elasticloadbalancingv2"),
            method: "POST",
            path: "/",
            claimant: V2,
        },
        Row {
            case: "a request addressed to the Elastic Load Balancing v2 key by host",
            addressed: Addressed::Host("elasticloadbalancingv2.us-east-1.amazonaws.com"),
            method: "POST",
            path: "/",
            claimant: V2,
        },
    ];

    for row in &rows {
        let claimed = claimants(&family, row);
        assert_eq!(
            claimed.len(),
            1,
            "case {:?}: the request must reach exactly one service, got {claimed:?}",
            row.case
        );
        assert_eq!(
            claimed[0], row.claimant,
            "case {:?}: expected {} to claim it, got {claimed:?}",
            row.case, row.claimant
        );
        assert!(
            claimed[0] != PROXY,
            "case {:?}: ELB Proxy must claim no awsQuery traffic of the shared name, got \
             {claimed:?}",
            row.case
        );
    }
}
