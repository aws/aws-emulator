// SPDX-License-Identifier: CC0-1.0

//! ELB Proxy and the two Elastic Load Balancing query APIs share one SigV4 signing name —
//! each must still be claimed by exactly one service in the assembled router.
//!
//! RULE: a signing name several services publish cannot decide which of them serves a
//! request, so each service claims only the requests its OWN wire form names: the service
//! an RPC-addressed request names (`rpcv2Cbor` carries it in the path,
//! `POST /service/<Service>/operation/<Operation>`) is the one that serves it, and a request
//! addressed on the query wire every Elastic Load Balancing client posts to (`POST /`, the
//! operation in the `Action` form field) belongs to the query front door that owns the shared
//! name. Nothing may be claimed twice (`500 AmbiguousService`), and a request that addresses
//! none of their wire surfaces — an RPC form naming a service none of them is — is claimed by
//! nobody, whichever endpoint it was sent to, which is the router's honest
//! `404 UnknownService` rather than an answer from a service that does not serve it (the claim
//! rule `service-common` states for every service under a shared signing name, pinned at both
//! layers by the sibling suite `elb_signing_name_family_claims`).
//!
//! All three publish `aws.auth#sigv4` name `elasticloadbalancing`
//! (`smithy_query(query="service_info")` on each model: ELB Proxy's `endpointPrefix` is
//! `elb-proxy` and its protocol `rpcv2Cbor`, while the Classic model is `2012-06-01` and the
//! v2 model `2015-12-01`, both `awsQuery`), and the router resolves a request's routing key
//! from the SigV4 credential scope FIRST — so all three see the same key and a claim resting
//! on it alone claims the others' traffic too. That is what made every Elastic Load
//! Balancing request answer `500 AmbiguousService`, a code no AWS model carries, in a JSON
//! body an awsQuery client cannot parse.
//!
//! The rows differ along what the rule generalises over: the wire the request arrives on
//! (`rpcv2Cbor` and `awsQuery`), the routing tier its key comes from (a credential scope on
//! the shared name, or the endpoint host of an unsigned request), which API version an
//! envelope names, and an RPC-addressed request naming a service that is NOT ELB Proxy —
//! which ELB Proxy must decline even though it arrives on the path space its own protocol
//! uses. Such a request is then answered by the API published at the ENDPOINT it was addressed
//! to — the query front door of the shared name, in its own `awsQuery` envelope, as the
//! unresolvable action it is — while the same form on an endpoint no model here is published at
//! reaches nobody and is answered the router's own `404 UnknownService`, which the family
//! suites `elasticloadbalancing_signing_name_claims` and `elb_signing_name_family_claims` pin.
//!
//! Every row is driven through the assembled router, so what claims a request here is what
//! claims it in the emulator a client talks to.
//!
//! The `elb_classic_and_v2_signing_name_routing` suite is the Classic/v2 half of the same
//! table, in far more depth; this one is about the third claimant on that name.
//!
//! Assembling the router costs stack, so it runs on an explicitly sized thread for the same
//! reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The signing name all three services publish, and the one every AWS SDK client of any of
/// them signs.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The API versions the two query models declare, which is what an awsQuery envelope names.
const CLASSIC_VERSION: &str = "2012-06-01";
const V2_VERSION: &str = "2015-12-01";

/// The `rpcv2Cbor` media type and protocol header every request of that wire carries.
const CBOR_MEDIA_TYPE: &str = "application/cbor";
const SMITHY_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");

/// An empty CBOR map: a `ListProxies` input, whose two members (`maxResults`, `nextToken`)
/// are both optional (`smithy_query(query="operation",operation="ListProxies")`).
const EMPTY_CBOR_MAP: &[u8] = &[0xa0];

/// Which wire one row's request arrives on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Wire {
    /// `rpcv2Cbor`: the service and the operation in the path, the body CBOR, and no
    /// `X-Amz-Target` at all.
    RpcV2Cbor { service: &'static str, operation: &'static str },
    /// `awsQuery`: `POST /`, the operation and the API version in the form body.
    Query { action: &'static str, version: &'static str },
}

/// How one row's request is addressed, which is the routing tier its key comes from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Addressed {
    /// Signed for the shared signing name, the tier the router reads first.
    SharedName,
    /// Unsigned and addressed at an endpoint host, whose first label is then the key.
    Host(&'static str),
}

/// One row: a request all three services could see the same routing key for, the status the
/// one service that owns it answers, and a marker only that service's answer carries.
struct Row {
    case: &'static str,
    wire: Wire,
    addressed: Addressed,
    status: u16,
    /// Text the owning service's own answer carries.
    carries: &'static [&'static str],
    /// Text the answer must NOT carry, because it would mean another service served it.
    withholds: &'static [&'static str],
    /// Whether NO service may claim this request — a path no emulated model publishes, which
    /// the router answers itself. Every other row requires exactly one claimant; this one
    /// requires none, which is the same rule read from the other side: a service claims only
    /// what its own wire form names.
    unclaimed: bool,
}

/// The marker of an answer ELB PROXY served: its operations are scaffolded, so the service
/// that claims one answers `501` naming the operation (`crate::scaffolded`'s wording). A CBOR
/// map writes its member values as text, so the sentence is as readable in these bytes as in
/// a JSON body.
const PROXY_ANSWER: &str = "ListProxies is scaffolded but not implemented";

/// The markers of an answer one of the QUERY front doors served: the awsQuery envelope of the
/// model whose API version the request named.
const CLASSIC_ANSWER: &str = "http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/";
const V2_ANSWER: &str = "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/";

/// The table.
const ROWS: &[Row] = &[
    Row {
        // ELB Proxy's own wire on the SHARED name — what an AWS SDK ELB Proxy client sends.
        case: "an ELB Proxy operation on the shared signing name",
        wire: Wire::RpcV2Cbor {
            service: "ELBProxy",
            operation: "ListProxies",
        },
        addressed: Addressed::SharedName,
        status: 501,
        carries: &[PROXY_ANSWER],
        withholds: &[CLASSIC_ANSWER, V2_ANSWER],
        unclaimed: false,
    },
    Row {
        // An UNSIGNED query request, addressed at the Elastic Load Balancing endpoint host:
        // the key comes from the host label instead of a credential scope, and must still
        // resolve to the one front door that owns that name rather than to this model.
        case: "the Classic API version addressed by the Elastic Load Balancing endpoint host",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        addressed: Addressed::Host("elasticloadbalancing.us-east-1.amazonaws.com"),
        status: 200,
        carries: &["<DescribeLoadBalancersResponse", CLASSIC_ANSWER],
        withholds: &[PROXY_ANSWER],
        unclaimed: false,
    },
    Row {
        // The Classic query API on the shared name: the front door that OWNS the name
        // serves it out of its own `2012-06-01` shapes.
        case: "the Classic API version on the shared signing name",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        addressed: Addressed::SharedName,
        status: 200,
        carries: &["<DescribeLoadBalancersResponse", CLASSIC_ANSWER],
        withholds: &[PROXY_ANSWER],
        unclaimed: false,
    },
    Row {
        // The ELBv2 query API on the same shared name: served out of THAT model's shapes,
        // which is the half the shared front door resolves by API version.
        case: "the ELBv2 API version on the shared signing name",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: V2_VERSION,
        },
        addressed: Addressed::SharedName,
        status: 200,
        carries: &["<DescribeLoadBalancersResponse", V2_ANSWER],
        withholds: &[PROXY_ANSWER],
        unclaimed: false,
    },
    Row {
        // An RPC-addressed request naming a service ELB Proxy is NOT: it arrives on the path
        // space this protocol uses, so a claim that read the path alone would take it. The
        // service the form NAMES is what decides, so ELB Proxy declines it. This row is
        // ADDRESSED TO the Elastic Load Balancing endpoint host, and the API published at an
        // endpoint is what answers a request that reached it naming no form of it: the query
        // front door that owns the shared name answers it in its own `awsQuery` envelope, as
        // the unresolvable action it is, rather than the request reaching no service at all
        // (`…-elasticloadbalancing-v7/src/service.rs`, `can_handle`). The same form on an
        // endpoint this API is NOT published at — a bare emulator socket, where only the
        // credential scope names the key — is claimed by nobody and answered the router's own
        // `404 UnknownService`, which the family suites
        // `elasticloadbalancing_signing_name_claims` and `elb_signing_name_family_claims` pin.
        case: "an RPC form naming another service on the shared signing name",
        wire: Wire::RpcV2Cbor {
            service: "NotELBProxy",
            operation: "ListProxies",
        },
        addressed: Addressed::SharedName,
        status: 400,
        carries: &["<ErrorResponse"],
        withholds: &[PROXY_ANSWER],
        unclaimed: false,
    },
];

/// A SigV4 `Authorization` whose credential scope names the shared signing name.
fn authorization() -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/us-east-1/{SHARED_SIGNING_NAME}/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

#[test]
fn elb_proxy_and_the_query_apis_are_each_claimed_by_exactly_one_service() {
    std::thread::Builder::new()
        .name("elb-proxy-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check: these
                // rows are about which service claims a request.
                router.set_request_authentication(false);

                for row in ROWS {
                    let host = match row.addressed {
                        Addressed::Host(host) => host,
                        Addressed::SharedName => "elasticloadbalancing.us-east-1.amazonaws.com",
                    };
                    let mut builder = ::http::Request::builder()
                        .method("POST")
                        .header("host", host);
                    let body = match row.wire {
                        Wire::RpcV2Cbor { service, operation } => {
                            builder = builder
                                .uri(format!("/service/{service}/operation/{operation}"))
                                .header("content-type", CBOR_MEDIA_TYPE)
                                .header("accept", CBOR_MEDIA_TYPE)
                                .header(SMITHY_PROTOCOL.0, SMITHY_PROTOCOL.1);
                            ::bytes::Bytes::from_static(EMPTY_CBOR_MAP)
                        }
                        Wire::Query { action, version } => {
                            builder = builder
                                .uri("/")
                                .header("content-type", "application/x-www-form-urlencoded");
                            ::bytes::Bytes::from(format!("Action={action}&Version={version}"))
                        }
                    };
                    if row.addressed == Addressed::SharedName {
                        builder = builder.header("authorization", authorization());
                    }
                    let request = builder.body(body).expect("the request is well formed");
                    let response = router.dispatch(request).await;
                    let status = response.status().as_u16();
                    let answered = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !answered.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request, expected exactly one \
                         claimant, got {status} with {answered}",
                        row.case
                    );
                    assert!(
                        !answered.contains("UnknownService") || row.unclaimed,
                        "case {:?}: no service claimed the request, expected exactly one \
                         claimant, got {status} with {answered}",
                        row.case
                    );
                    assert!(
                        answered.contains("UnknownService") || !row.unclaimed,
                        "case {:?}: expected no service to claim a path no model publishes, \
                         got {status} with {answered}",
                        row.case
                    );
                    assert_eq!(
                        status, row.status,
                        "case {:?}: expected HTTP {} from the service that owns the request, \
                         got {status} with {answered}",
                        row.case, row.status
                    );
                    for expected in row.carries {
                        assert!(
                            answered.contains(expected),
                            "case {:?}: expected the owning service's answer to carry \
                             {expected:?}, got {status} with {answered}",
                            row.case
                        );
                    }
                    for forbidden in row.withholds {
                        assert!(
                            !answered.contains(forbidden),
                            "case {:?}: another service served the request — its answer \
                             carries {forbidden:?}: {status} with {answered}",
                            row.case
                        );
                    }
                }
            });
        })
        .expect("spawn the routing thread")
        .join()
        .expect("ELB Proxy and the Elastic Load Balancing query APIs must route apart");
}
