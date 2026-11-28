// SPDX-License-Identifier: CC0-1.0

//! ELB Proxy and Classic Elastic Load Balancing are TWO emulated services under ONE SigV4
//! signing name.
//!
//! The ELB Proxy model declares `sigv4Name` / `arnNamespace` `elasticloadbalancing`
//! (`smithy_query(query="service_info")`) while serving its own `elb-proxy` endpoint over
//! `rpcv2Cbor`, and the Classic Elastic Load Balancing model declares that same name while
//! serving `awsQuery`. The router resolves a request's routing key from the SigV4 credential
//! scope first, so both services see the SAME key: with the generated signing-name claim on
//! both, the router found TWO claimants for every Elastic Load Balancing request and
//! answered `500 AmbiguousService` — a code that exists in no AWS model — so NEITHER API was
//! reachable.
//!
//! The rule this suite pins: a signing name shared by several emulated services resolves to
//! exactly one of them per request, decided by the WIRE FORM each model declares — an
//! `rpcv2Cbor` request (`POST /service/{Service}/operation/{Operation}`, whose `{Service}`
//! label names the service) is the proxy's, and an `awsQuery` request (`POST /`, naming its
//! operation in the `Action` form field) is the Classic front door's — so no request has two
//! claimants and none loses the one it had.
//!
//! The rows span what the rule generalises over: each model's own wire form, every routing
//! signal that resolves the shared key (a credential scope, a host label, a presigned query
//! parameter, none at all), each service's own key beside the shared one, several operations
//! of the proxy's model rather than one, and the forms that must be claimed by NEITHER (an
//! RPC path naming a service neither model declares).

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::Service;

/// Keep this at or above the sibling router suites' stack: assembling the whole router is
/// what costs it.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The signing name both models declare.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// The `{Service}` label an `rpcv2Cbor` request names the proxy's service shape by
/// (`smithy_query(query="service_info")`: the `ELBProxy` service shape).
const PROXY_SERVICE_LABEL: &str = "ELBProxy";

/// Operations the proxy's model binds, named in the `rpcv2Cbor` path
/// (`smithy_query(query="digest")`: `ListProxies`, `CreateProxy`, `GetProxy`,
/// `ListProxyVersions`, `TagResource`).
const PROXY_OPERATIONS: &[&str] = &[
    "ListProxies",
    "CreateProxy",
    "GetProxy",
    "ListProxyVersions",
    "TagResource",
];

/// How a row addresses the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 credential scope naming this routing key — what every AWS SDK sends, and the
    /// signal the router reads first.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming the shared signing name.
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// Which emulated service a row must be claimed by, named the way this suite reports one.
const CLASSIC: &str = "elasticloadbalancing (Classic)";
const PROXY: &str = "elbproxy";

/// One row of the claim table: how the request is addressed, its `Host`, its method and
/// URI, and the services that must claim it.
struct Row {
    case: String,
    addressed: Addressed,
    host: &'static str,
    path: String,
    claimants: Vec<&'static str>,
}

/// The `Authorization` header of a request signed for `signing_name`. The signature is never
/// checked: a claim predicate reads only the credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// A presigned credential scope naming the shared signing name, with its `/` separators
/// encoded as a query string carries them.
const PRESIGNED: &str =
    "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Felasticloadbalancing%2Faws4_request";

/// The `rpcv2Cbor` path one operation of `service` arrives on.
fn rpc_path(service: &str, operation: &str) -> String {
    format!("/service/{service}/operation/{operation}")
}

/// The rows, built so every signal and both wire forms are covered.
fn rows() -> Vec<Row> {
    let mut rows = vec![
        Row {
            // The Classic model's own wire form under the shared key: every one of its
            // operations is this one request shape.
            case: "an awsQuery request signed for the shared name".to_string(),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            path: "/".to_string(),
            claimants: vec![CLASSIC],
        },
        Row {
            case: "an awsQuery request on a bare emulator endpoint".to_string(),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "localhost:9297",
            path: "/".to_string(),
            claimants: vec![CLASSIC],
        },
        Row {
            case: "a presigned awsQuery request on the shared name".to_string(),
            addressed: Addressed::Presigned,
            host: "elasticloadbalancing.eu-west-2.amazonaws.com",
            path: "/".to_string(),
            claimants: vec![CLASSIC],
        },
        Row {
            case: "an anonymous awsQuery request on the Elastic Load Balancing host"
                .to_string(),
            addressed: Addressed::Anonymous,
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            path: "/".to_string(),
            claimants: vec![CLASSIC],
        },
        Row {
            // The proxy's endpoint resolves the key for a request that carries no
            // credential scope at all.
            case: "an anonymous rpcv2Cbor request on the ELB Proxy host".to_string(),
            addressed: Addressed::Anonymous,
            host: "elb-proxy.us-east-1.amazonaws.com",
            path: rpc_path(PROXY_SERVICE_LABEL, "ListProxies"),
            claimants: vec![PROXY],
        },
        Row {
            // An RPC path naming a service NEITHER model declares, ADDRESSED TO the Elastic
            // Load Balancing endpoint: the proxy declines it (the path names another
            // service), and the query API published at that endpoint answers it in its own
            // envelope as the unresolvable request it is — the claim
            // `services/service-com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7/src/service.rs`
            // states for a request that addresses this endpoint and names no form of its RPC
            // peer. One claimant, which is what this table is about; the row below, addressed
            // at a bare emulator endpoint instead, is the side that reaches nobody.
            case: "an rpcv2Cbor path naming another service".to_string(),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            path: rpc_path("SomeOtherService", "ListProxies"),
            claimants: vec![CLASSIC],
        },
        Row {
            // The same RPC path on a bare emulator endpoint: nothing about the request
            // addresses either endpoint of this family and no emulated model publishes that
            // path, so it is claimed by nobody — the router's honest `404 UnknownService`
            // rather than an ambiguity.
            case: "an rpcv2Cbor path naming another service on a bare emulator endpoint"
                .to_string(),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "localhost:9297",
            path: rpc_path("SomeOtherService", "ListProxies"),
            claimants: vec![],
        },
        Row {
            // The proxy's wire form addressed to the Classic host: the wire form decides,
            // not the host.
            case: "an rpcv2Cbor request on the Elastic Load Balancing host".to_string(),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            path: rpc_path(PROXY_SERVICE_LABEL, "ListProxies"),
            claimants: vec![PROXY],
        },
    ];

    // One row per proxy operation, so the claim is on the wire FORM and not on one
    // operation name.
    for operation in PROXY_OPERATIONS {
        rows.push(Row {
            case: format!("the proxy's {operation} over rpcv2Cbor, signed for the shared name"),
            addressed: Addressed::Signed(SHARED_SIGNING_NAME),
            host: "elb-proxy.us-east-1.amazonaws.com",
            path: rpc_path(PROXY_SERVICE_LABEL, operation),
            claimants: vec![PROXY],
        });
    }

    rows
}

/// The request one row describes, as the parts a claim predicate reads.
fn parts_of(row: &Row) -> http::request::Parts {
    let uri = match row.addressed {
        Addressed::Presigned => format!("{}?{PRESIGNED}", row.path),
        _ => row.path.clone(),
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", row.host);
    if let Addressed::Signed(signing_name) = row.addressed {
        builder = builder.header("authorization", authorization(signing_name));
    }
    builder
        .body(bytes::Bytes::new())
        .expect("a well-formed request")
        .into_parts()
        .0
}

#[test]
fn exactly_one_service_claims_each_request_under_the_shared_signing_name() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    for row in rows() {
        let parts = parts_of(&row);
        let claimants: Vec<&str> = [
            (CLASSIC, classic.can_handle(&parts)),
            (PROXY, proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(name, claimed)| claimed.then_some(name))
        .collect();
        assert_eq!(
            claimants,
            row.claimants,
            "case {:?} (POST {} to host {}, {:?}): expected it to be claimed by exactly \
             {:?}, got {claimants:?}",
            row.case,
            row.path,
            row.host,
            row.addressed,
            row.claimants
        );
    }
}

#[test]
fn the_whole_router_answers_each_wire_form_out_of_its_own_service() {
    std::thread::Builder::new()
        .name("elb-proxy-routing".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
            let mut router = build_router(engine);
            // These rows are about ROUTING, not about SigV4.
            router.set_request_authentication(false);
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("a current-thread runtime");

            // Each row is answered by the ONE service whose wire form it carries: the
            // Classic front door answers an awsQuery envelope in its own XML, and the proxy
            // answers its own `rpcv2Cbor` requests — never the router's `AmbiguousService`
            // (two claimants) or `UnknownService` (none), which are what a shared signing
            // name with no discriminator produces.
            let rows: Vec<(String, String, String, &str)> = PROXY_OPERATIONS
                .iter()
                .map(|operation| {
                    (
                        format!("the proxy's {operation}"),
                        rpc_path(PROXY_SERVICE_LABEL, operation),
                        String::new(),
                        "application/cbor",
                    )
                })
                .chain([(
                    "the Classic front door's DescribeLoadBalancers".to_string(),
                    "/".to_string(),
                    "Action=DescribeLoadBalancers&Version=2012-06-01".to_string(),
                    "application/x-www-form-urlencoded",
                )])
                .collect();

            for (case, path, body, content_type) in rows {
                let request = http::Request::builder()
                    .method("POST")
                    .uri(format!("http://localhost{path}"))
                    .header("host", "localhost")
                    .header("content-type", content_type)
                    .header("authorization", authorization(SHARED_SIGNING_NAME))
                    .body(bytes::Bytes::from(body))
                    .expect("a well-formed request");
                let response = runtime.block_on(router.dispatch(request));
                let status = response.status().as_u16();
                let rendered = String::from_utf8_lossy(response.body()).to_string();
                assert!(
                    !rendered.contains("AmbiguousService") && !rendered.contains("UnknownService"),
                    "case {case:?}: the request must reach exactly one service, got {status} \
                     with {rendered}"
                );
                assert_ne!(
                    status, 500,
                    "case {case:?}: a routing failure must not be answered as a server \
                     fault, got {rendered}"
                );
            }

            // And the Classic front door still answers its own envelope as XML, which is
            // what an awsQuery client parses.
            let request = http::Request::builder()
                .method("POST")
                .uri("http://localhost/")
                .header("host", "localhost")
                .header("content-type", "application/x-www-form-urlencoded")
                .header("authorization", authorization(SHARED_SIGNING_NAME))
                .body(bytes::Bytes::from(
                    "Action=DescribeLoadBalancers&Version=2012-06-01".to_string(),
                ))
                .expect("a well-formed request");
            let response = runtime.block_on(router.dispatch(request));
            let status = response.status().as_u16();
            let rendered = String::from_utf8_lossy(response.body()).to_string();
            assert!(
                status == 200 && rendered.trim_start().starts_with('<'),
                "case \"the Classic front door's own envelope\": expected an awsQuery XML \
                 answer, got {status} with {rendered}"
            );
        })
        .expect("spawn the routing thread")
        .join()
        .expect("Classic Elastic Load Balancing and ELB Proxy must route apart");
}
