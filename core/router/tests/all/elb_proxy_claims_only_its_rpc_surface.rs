// SPDX-License-Identifier: CC0-1.0

//! ELB Proxy signs as `elasticloadbalancing`, the signing name Classic Elastic Load
//! Balancing's front door owns.
//!
//! `smithy_query(query="service_info")` on `com-amazonaws-elbproxy-elbproxy` reports
//! `sigv4Name` / `arnNamespace` `elasticloadbalancing` and the `rpcv2Cbor` protocol, while
//! Classic Elastic Load Balancing declares the same signing name over `awsQuery`. The
//! router resolves a request's routing key from the SigV4 credential scope, so the
//! generated signing-name claim left BOTH services claiming every `elasticloadbalancing`
//! request and the router answered `500 AmbiguousService` — a code no AWS model carries, in
//! a JSON body an awsQuery client cannot parse — for the Classic front door's own traffic.
//!
//! The rule this suite pins: a service that declares an RPC protocol claims only requests
//! that NAME it on a path that protocol uses — for `rpcv2Cbor`, the URI
//! `/service/<Service>/operation/<Operation>` — so the awsQuery front door's `POST /` space
//! is left to the service that owns it, and exactly one service claims each request there.
//!
//! It asks each service's [`Service::can_handle`] directly, one layer below dispatch, which
//! is where the ambiguity is decided; the sibling suite
//! `elb_classic_and_v2_signing_name_routing` drives the whole front door.

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;
use smithy_server_runtime::Service;

/// The region the signed rows scope their credentials to.
const REGION: &str = "us-east-1";

/// How a row addresses the request it describes.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 `Authorization` scope naming this routing key.
    Signed(&'static str),
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// One row: a request, and which of the two services must claim it.
struct Row {
    case: &'static str,
    /// The URI the request posts to: `/` for the awsQuery front door, a
    /// `/service/<Service>/operation/<Operation>` for the `rpcv2Cbor` form.
    uri: &'static str,
    host: &'static str,
    addressed: Addressed,
    /// Whether ELB Proxy must claim it. This is the subject of the suite.
    elb_proxy_claims: bool,
    /// Whether the Classic front door must claim it, where that is part of the rule under
    /// test — the `POST /` space it owns, which exactly one service may claim. `None` on a
    /// row addressed to another protocol's RPC path, since which service owns the shared
    /// key THERE is the ELB Proxy bootstrap's to settle and not this rule.
    classic_claims: Option<bool>,
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

#[test]
fn elb_proxy_claims_only_the_requests_that_name_it_on_its_own_protocols_path() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let elb_proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    let rows = [
        Row {
            case: "an awsQuery request signed for the shared name",
            uri: "/",
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            addressed: Addressed::Signed("elasticloadbalancing"),
            elb_proxy_claims: false,
            classic_claims: Some(true),
        },
        Row {
            case: "an awsQuery request signed for the shared name on a bare emulator endpoint",
            uri: "/",
            host: "localhost:9297",
            addressed: Addressed::Signed("elasticloadbalancing"),
            elb_proxy_claims: false,
            classic_claims: Some(true),
        },
        Row {
            case: "an anonymous request addressed to the Elastic Load Balancing host",
            uri: "/",
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            addressed: Addressed::Anonymous,
            elb_proxy_claims: false,
            classic_claims: Some(true),
        },
        Row {
            case: "an rpcv2Cbor request naming ELB Proxy",
            uri: "/service/ELBProxy/operation/ListProxies",
            host: "localhost:9297",
            addressed: Addressed::Signed("elasticloadbalancing"),
            elb_proxy_claims: true,
            classic_claims: None,
        },
        Row {
            case: "another of ELB Proxy's own operations",
            uri: "/service/ELBProxy/operation/GetProxy",
            host: "elb-proxy.us-east-1.amazonaws.com",
            addressed: Addressed::Signed("elasticloadbalancing"),
            elb_proxy_claims: true,
            classic_claims: None,
        },
        Row {
            case: "an rpcv2Cbor request naming another service",
            uri: "/service/ELBProxyShadow/operation/ListProxies",
            host: "localhost:9297",
            addressed: Addressed::Signed("elasticloadbalancing"),
            elb_proxy_claims: false,
            classic_claims: None,
        },
        Row {
            case: "an rpcv2Cbor request naming ELB Proxy under another signing name",
            uri: "/service/ELBProxy/operation/ListProxies",
            host: "localhost:9297",
            addressed: Addressed::Signed("sqs"),
            elb_proxy_claims: false,
            classic_claims: Some(false),
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(row.uri)
            .header("host", row.host);
        if let Addressed::Signed(signing_name) = row.addressed {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;

        let claimed = elb_proxy.can_handle(&parts);
        assert_eq!(
            claimed, row.elb_proxy_claims,
            "case {case:?} (POST {} to host {}): expected ELB Proxy to claim it = {}, got {claimed}",
            row.uri, row.host, row.elb_proxy_claims
        );
        if let Some(expected) = row.classic_claims {
            let claimed = classic.can_handle(&parts);
            assert_eq!(
                claimed, expected,
                "case {case:?} (POST {} to host {}): expected Classic Elastic Load Balancing \
                 to claim it = {expected}, got {claimed}",
                row.uri, row.host
            );
        }
    }
}
