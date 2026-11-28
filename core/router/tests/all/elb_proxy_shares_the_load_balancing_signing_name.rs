// SPDX-License-Identifier: CC0-1.0

//! What ELB Proxy claims of the signing name it SHARES with the Elastic Load Balancing
//! endpoint.
//!
//! Classic Elastic Load Balancing, Elastic Load Balancing v2 and ELB Proxy all publish
//! `elasticloadbalancing` as their `aws.auth#sigv4` name — ELB Proxy's own
//! `smithy_query(query="service_info")` reports `sigv4Name: elasticloadbalancing` beside
//! its `endpointPrefix: elb-proxy` — so the routing key every AWS SDK for any of the three
//! sends resolves to that one name, and a claim that reads the key alone makes each of them
//! claim the others' traffic.
//!
//! RULE under test: **ELB Proxy claims exactly the requests its OWN wire form names it in
//! and declines every other request signed for that shared name.** The wire form is what
//! names the service: ELB Proxy is `rpcv2Cbor`, whose request names the service and the
//! operation in its URI (`POST /service/ELBProxy/operation/<Operation>`), while the Elastic
//! Load Balancing endpoint is `awsQuery`, which posts to `/` and names its action in the
//! body. Declining that awsQuery traffic is what leaves the endpoint's owner as its single
//! claimant — which matters because the owner serves actions its own routing table does not
//! hold out of the other Elastic Load Balancing model, and the router keeps "only the
//! claimants whose model names an operation for it" (`MultiServiceRouter`), so a second
//! claimant there can leave NONE and the request is refused `500 AmbiguousService`.
//!
//! The rows differ along what the rule generalises over: the wire form (its own and the
//! endpoint owner's), which service that form names, and which of the two routing keys of
//! the family the request is signed for. Each row asks THIS service's
//! [`smithy_server_runtime::Service::can_handle`] directly — what the other front doors of
//! the shared name claim, and which model finally serves each request, are their own suites'
//! rules (`elb_classic_and_v2_signing_name_routing.rs`,
//! `elbproxy_shared_signing_name_routing.rs` and their siblings).

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// The signing name all three services publish.
const SHARED_NAME: &str = "elasticloadbalancing";

/// The ELBv2 model's OWN routing key, which only that service claims.
const V2_NAME: &str = "elasticloadbalancingv2";

/// The URI an ELB Proxy call arrives on: the `rpcv2Cbor` wire form
/// `POST /service/{Service}/operation/{Operation}`, whose `{Service}` label is this
/// model's service shape name (`smithy_query(query="service_info")`: `ELBProxy`).
const PROXY_CBOR_URI: &str = "/service/ELBProxy/operation/ListProxies";

/// The same wire form naming ANOTHER service, for the row that pins that ELB Proxy claims
/// a cbor request only when the request names IT.
const OTHER_CBOR_URI: &str = "/service/ElasticLoadBalancing_v10/operation/DescribeLoadBalancers";

/// The media type and marker header a `rpcv2Cbor` request carries, and the value that
/// protocol's specification requires in the marker ("the `smithy-protocol` header … MUST
/// be set to `rpc-v2-cbor`").
const CBOR_CONTENT_TYPE: &str = "application/cbor";
const SMITHY_PROTOCOL_HEADER: &str = "smithy-protocol";
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every AWS
/// SDK sends it. The signature is never checked here: the claim predicate reads only the
/// credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// Which wire form a row's request is addressed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wire {
    /// The `rpcv2Cbor` form: the URI names the service and the operation, and the marker
    /// header names the protocol.
    Cbor(&'static str),
    /// The `awsQuery` form: `POST /` with a form-encoded body naming the action.
    Query,
}

/// One row of the claim table: how the request is addressed, what it is signed for, and
/// whether ELB Proxy claims it.
struct Row {
    case: &'static str,
    wire: Wire,
    signed_for: &'static str,
    claimed: bool,
}

/// The rows: each wire form, under the shared name and under the key only ELBv2 claims.
const ROWS: &[Row] = &[
    Row {
        // The one request that is this service's: its own wire form, naming it, signed for
        // the name its endpoint publishes.
        case: "a cbor request naming ELB Proxy, signed for the shared name",
        wire: Wire::Cbor(PROXY_CBOR_URI),
        signed_for: SHARED_NAME,
        claimed: true,
    },
    Row {
        // The Elastic Load Balancing endpoint's own traffic on that same name: not this
        // service's, and claiming it is what left the router with no single claimant.
        case: "an awsQuery request signed for the shared name",
        wire: Wire::Query,
        signed_for: SHARED_NAME,
        claimed: false,
    },
    Row {
        case: "an awsQuery request signed for the ELBv2 model's own key",
        wire: Wire::Query,
        signed_for: V2_NAME,
        claimed: false,
    },
    Row {
        // The cbor wire form naming a service that is not ELB Proxy.
        case: "a cbor request naming another service, signed for the shared name",
        wire: Wire::Cbor(OTHER_CBOR_URI),
        signed_for: SHARED_NAME,
        claimed: false,
    },
    Row {
        // A cbor request naming ELB Proxy but signed for the key only ELBv2 claims: the
        // scope decides which name is being addressed, so this service declines it too.
        case: "a cbor request naming ELB Proxy, signed for the ELBv2 key",
        wire: Wire::Cbor(PROXY_CBOR_URI),
        signed_for: V2_NAME,
        claimed: false,
    },
];

/// RULE: ELB Proxy claims only the requests its own `rpcv2Cbor` wire form names it in, so
/// every other request signed for the shared load-balancing name is left to the service
/// that owns it.
#[test]
fn elb_proxy_claims_only_the_cbor_requests_that_name_it() {
    use smithy_server_runtime::Service;

    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    for row in ROWS {
        let case = row.case;
        let mut builder = http::Request::builder()
            .method("POST")
            .header("host", "localhost:9297")
            .header("authorization", authorization(row.signed_for));
        builder = match row.wire {
            Wire::Cbor(uri) => builder
                .uri(uri)
                .header("content-type", CBOR_CONTENT_TYPE)
                .header(SMITHY_PROTOCOL_HEADER, CBOR_PROTOCOL),
            Wire::Query => builder
                .uri("/")
                .header("content-type", "application/x-www-form-urlencoded"),
        };
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimed = proxy.can_handle(&parts);
        assert_eq!(
            claimed, row.claimed,
            "case {case:?}: expected ELB Proxy to {} this request, it {}",
            if row.claimed { "claim" } else { "decline" },
            if claimed { "claimed it" } else { "declined it" }
        );
    }
}
