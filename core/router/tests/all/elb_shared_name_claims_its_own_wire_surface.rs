// SPDX-License-Identifier: CC0-1.0

//! A signing name THREE emulated services publish decides none of them on its own: each
//! claims the requests its own wire surface names, and nothing else.
//!
//! Classic Elastic Load Balancing, Elastic Load Balancing v2 and ELB Proxy all declare
//! `aws.auth#sigv4` name `elasticloadbalancing` (`smithy_query(query="service_info")` on
//! each model; the two query models are `awsQuery` at API versions `2012-06-01` and
//! `2015-12-01`, and ELB Proxy is `rpcv2Cbor` with `endpointPrefix` `elb-proxy`). The
//! router resolves a request's routing key from the SigV4 credential scope first, so all
//! three see the SAME key and a claim resting on that key alone claims the others' traffic
//! — which the router answers `500 AmbiguousService`, a code no AWS model carries, in a
//! body neither an awsQuery nor an rpcv2Cbor client can parse.
//!
//! RULE: a service whose signing name is shared claims a request only when the request
//! addresses that service's own wire surface — the `POST /` an `awsQuery` envelope posts
//! every action to, naming the API in its `Version` field, for the query front door that
//! owns the name, and the `POST /service/<Service>/operation/<Operation>` form that NAMES
//! the service for the `rpcv2Cbor` peer — plus, for the front door whose identity IS that
//! name and whose endpoint prefix it therefore is, a request ADDRESSED TO that endpoint
//! host naming no form of its RPC peer, which that API answers in its own envelope as the
//! unresolvable request it is rather than leaving a request that reached its endpoint
//! unanswered. A request that addresses NEITHER a wire form nor an endpoint of the family
//! — the same path on a bare emulator socket, where only the credential scope names the key
//! — is claimed by nobody, which is the router's honest `404 UnknownService` rather than an
//! answer from a service that does not serve it. Either way exactly one service answers.
//!
//! The rows differ along what the rule generalises over: the wire the request arrives on
//! (the query root post, the RPC form, a REST-shaped path no model binds), the routing tier
//! the key comes from (a credential scope, a presigned query scope, the host label of an
//! unsigned request), the endpoint host it is addressed to (the shared one, the ELBv2 one,
//! ELB Proxy's own, a bare emulator socket that names no service), the API version the
//! envelope names, the operation the RPC path names, and the service label it names — the
//! peer's, one that merely extends it, one no model of this name is, and the peer's own form
//! under a key outside the family.
//!
//! Two layers are asserted, as the sibling suites of this family do: the claim predicate of
//! each of the three services, where a second claimant is visible as such, and the WHOLE
//! assembled router, so what claims a request here is what claims it in the emulator a
//! client talks to. Assembling the router costs stack, so that half runs on an explicitly
//! sized thread for the same reason `boot_serve.rs` does.

use std::sync::Arc;

use aws_emulator_router::{build_router, InMemoryEngine};
use smithy_server_runtime::Service;

/// Keep this at or above the size `boot_serve.rs` uses.
const BOOT_STACK_SIZE: usize = 64 * 1024 * 1024;

/// The region every row is signed for or addressed in.
const REGION: &str = "us-east-1";

/// The signing name all three models declare, and the key every AWS SDK client of any of
/// them resolves.
const SHARED_SIGNING_NAME: &str = "elasticloadbalancing";

/// The routing key that names the ELBv2 model directly, beside the shared one.
const V2_ROUTE_KEY: &str = "elasticloadbalancingv2";

/// The endpoint hosts of this family, and one bare emulator socket whose label names no
/// service at all.
const SHARED_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";
const V2_HOST: &str = "elasticloadbalancingv2.us-east-1.amazonaws.com";
const PROXY_HOST: &str = "elb-proxy.us-east-1.amazonaws.com";
const BARE_HOST: &str = "localhost:9297";

/// The API versions the two query models declare, which is what an awsQuery envelope names.
const CLASSIC_VERSION: &str = "2012-06-01";
const V2_VERSION: &str = "2015-12-01";

/// The marker of an answer served out of each query model: the awsQuery document namespace
/// of the model whose API version the envelope named.
const CLASSIC_ANSWER: &str = "http://elasticloadbalancing.amazonaws.com/doc/2012-06-01/";
const V2_ANSWER: &str = "http://elasticloadbalancing.amazonaws.com/doc/2015-12-01/";

/// The marker of an answer ELB PROXY served: its operations are scaffolded, so the service
/// that claims one answers `501` with the `NotImplemented` code naming the operation.
const PROXY_ANSWER: &str = "NotImplemented";

/// The markers of the query front door's own refusal of a request that reached its endpoint
/// naming no action it can resolve: its awsQuery error envelope and that refusal's code.
const FRONT_DOOR_REFUSAL: &[&str] = &["<ErrorResponse", "InvalidAction"];

/// The router's own answer when NO service claims a request.
const NO_CLAIMANT: &str = "UnknownService";

/// The `rpcv2Cbor` media type, protocol header and empty-map body every RPC row carries.
/// `ListProxies`' two members are both optional
/// (`smithy_query(query="operation",operation="ListProxies")`), so that row's body is a
/// complete input; a row naming an operation with a required member is answered by that
/// operation's own refusal, which says just as clearly which service served it.
const CBOR_MEDIA_TYPE: &str = "application/cbor";
const CBOR_PROTOCOL: (&str, &str) = ("smithy-protocol", "rpc-v2-cbor");
const CBOR_EMPTY_MAP: &[u8] = &[0xa0];

/// The local name of ELB Proxy's service shape, which its `rpcv2Cbor` path names
/// (`smithy_query(query="service_info")`: the `ELBProxy` service shape).
const PROXY_SERVICE_SHAPE: &str = "ELBProxy";

/// The three services, named the way this suite reports a claimant.
const CLASSIC: &str = "classic";
const V2: &str = "elbv2";
const PROXY: &str = "elbproxy";

/// Which wire one row's request arrives on.
#[derive(Clone, Copy, Debug)]
enum Wire {
    /// `awsQuery`: `POST /`, the action and the API version in the form body.
    Query {
        action: &'static str,
        version: &'static str,
    },
    /// `rpcv2Cbor`: `POST /service/<Service>/operation/<Operation>`, the body CBOR.
    Rpc {
        service: &'static str,
        operation: &'static str,
    },
    /// A REST-shaped path no model of this signing name binds.
    Path(&'static str),
}

/// The routing tier one row's key comes from.
#[derive(Clone, Copy, Debug)]
enum Addressed {
    /// A SigV4 credential scope naming this key — what every AWS SDK sends.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming the shared name.
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// What the ONE service that owns a row answers, end to end through the router.
struct Answer {
    status: u16,
    /// Text that says WHICH service answered.
    carries: &'static [&'static str],
    /// Text the answer must NOT carry, because it would mean another service served it.
    withholds: &'static [&'static str],
}

/// One row: a request all three services see one routing key for, the services that must
/// claim it, and what the router answers it.
struct Row {
    case: &'static str,
    wire: Wire,
    host: &'static str,
    addressed: Addressed,
    /// The services that must claim it — empty for a request that addresses no wire form
    /// and no endpoint of this family.
    claimants: &'static [&'static str],
    /// What the whole router answers, or `None` for a row whose key names a service outside
    /// this family: which service answers THAT is not this family's rule, so only the claim
    /// half applies.
    answer: Option<Answer>,
}

/// The table.
const ROWS: &[Row] = &[
    Row {
        // The query front door's own wire form on the shared name.
        case: "the query wire's root post signed for the shared name",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        host: SHARED_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 200,
            carries: &["<DescribeLoadBalancersResponse", CLASSIC_ANSWER],
            withholds: &[V2_ANSWER, PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The same wire naming the OTHER API version: the one front door resolves it out
        // of that model's shapes, which is the version half of the shared endpoint.
        case: "the same root post naming the ELBv2 API version",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: V2_VERSION,
        },
        host: SHARED_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 200,
            carries: &["<DescribeLoadBalancersResponse", V2_ANSWER],
            withholds: &[CLASSIC_ANSWER, PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The key from the host label instead of a credential scope.
        case: "an unsigned root post on the shared endpoint host",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        host: SHARED_HOST,
        addressed: Addressed::Anonymous,
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 200,
            carries: &["<DescribeLoadBalancersResponse", CLASSIC_ANSWER],
            withholds: &[V2_ANSWER, PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // And from a presigned credential scope in the query string.
        case: "a presigned root post on the shared endpoint host",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: CLASSIC_VERSION,
        },
        host: SHARED_HOST,
        addressed: Addressed::Presigned,
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 200,
            carries: &["<DescribeLoadBalancersResponse", CLASSIC_ANSWER],
            withholds: &[V2_ANSWER, PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The ELBv2 model addressed by the key that names IT, beside the shared one.
        case: "a root post signed for the ELBv2 model's own key",
        wire: Wire::Query {
            action: "DescribeLoadBalancers",
            version: V2_VERSION,
        },
        host: V2_HOST,
        addressed: Addressed::Signed(V2_ROUTE_KEY),
        claimants: &[V2],
        answer: Some(Answer {
            status: 200,
            carries: &["<DescribeLoadBalancersResponse", V2_ANSWER],
            withholds: &[CLASSIC_ANSWER, PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The RPC peer's own wire form on the shared name, addressed at the query front
        // door's endpoint host: the form the request takes decides, not the host.
        case: "the RPC wire naming the peer, signed for the shared name",
        wire: Wire::Rpc {
            service: PROXY_SERVICE_SHAPE,
            operation: "ListProxies",
        },
        host: SHARED_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[PROXY],
        answer: Some(Answer {
            status: 501,
            carries: &[PROXY_ANSWER, "ListProxies"],
            withholds: &[CLASSIC_ANSWER, V2_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The same form on the OTHER query API's endpoint host: neither host decides it.
        case: "the RPC wire naming the peer on the ELBv2 endpoint host",
        wire: Wire::Rpc {
            service: PROXY_SERVICE_SHAPE,
            operation: "ListProxies",
        },
        host: V2_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[PROXY],
        answer: Some(Answer {
            status: 501,
            carries: &[PROXY_ANSWER, "ListProxies"],
            withholds: &[CLASSIC_ANSWER, V2_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // Another operation of the same peer, on a bare emulator socket: the claim is on
        // the service the path names, not on one operation of it. This one's input has a
        // required member (`smithy_query(query="operation",operation="GetProxy")`:
        // `proxyIdentifier` is required), and the empty body names none of it — so what
        // proves the peer served it is its own refusal over its own input shape, in its own
        // CBOR envelope, rather than the scaffold's 501.
        case: "another of the peer's operations on a bare emulator socket",
        wire: Wire::Rpc {
            service: PROXY_SERVICE_SHAPE,
            operation: "GetProxy",
        },
        host: BARE_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[PROXY],
        answer: Some(Answer {
            status: 400,
            carries: &["ValidationException", "proxyIdentifier"],
            withholds: &[CLASSIC_ANSWER, V2_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // Unsigned on the peer's OWN endpoint host, whose label is its `endpointPrefix`:
        // the API published at an endpoint answers a client that addresses it.
        case: "the RPC wire naming the peer, unsigned on the peer's endpoint host",
        wire: Wire::Rpc {
            service: PROXY_SERVICE_SHAPE,
            operation: "ListProxies",
        },
        host: PROXY_HOST,
        addressed: Addressed::Anonymous,
        claimants: &[PROXY],
        answer: Some(Answer {
            status: 501,
            carries: &[PROXY_ANSWER, "ListProxies"],
            withholds: &[CLASSIC_ANSWER, V2_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // An RPC form naming a service no model of this name is, ADDRESSED TO the shared
        // endpoint host: the peer declines it (the path names another service), and the API
        // published at the endpoint the request reached answers it in its own awsQuery
        // envelope as the unresolvable request it is.
        case: "an RPC form naming a service no model of this name is, on the shared host",
        wire: Wire::Rpc {
            service: "ELBShadow",
            operation: "ListProxies",
        },
        host: SHARED_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 400,
            carries: FRONT_DOOR_REFUSAL,
            withholds: &[PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // A label that merely EXTENDS the peer's, with the key resolved from the host label
        // rather than a credential scope: the service a path names is matched whole, so the
        // peer declines it and the endpoint it was addressed to answers it.
        case: "an RPC form whose service label merely extends the peer's, unsigned",
        wire: Wire::Rpc {
            service: "ELBProxyMirror",
            operation: "ListProxies",
        },
        host: SHARED_HOST,
        addressed: Addressed::Anonymous,
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 400,
            carries: FRONT_DOOR_REFUSAL,
            withholds: &[PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // The OTHER side of that rule: the same form on a bare emulator socket, where only
        // the credential scope names the key and nothing addresses an endpoint of this
        // family. No wire surface publishes that path, so it is claimed by nobody and the
        // router answers it itself.
        case: "an RPC form naming a service no model of this name is, on a bare socket",
        wire: Wire::Rpc {
            service: "ELBShadow",
            operation: "ListProxies",
        },
        host: BARE_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[],
        answer: Some(Answer {
            status: 404,
            carries: &[NO_CLAIMANT],
            withholds: &["<ErrorResponse", PROXY_ANSWER, CLASSIC_ANSWER, V2_ANSWER],
        }),
    },
    Row {
        // Neither query model binds a URI at all, so a REST-shaped path is no model's wire
        // form — on the shared endpoint host it is still the API published there to answer.
        case: "a REST path no Elastic Load Balancing model binds, on the shared host",
        wire: Wire::Path("/2012-06-01/load-balancers"),
        host: SHARED_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[CLASSIC],
        answer: Some(Answer {
            status: 400,
            carries: FRONT_DOOR_REFUSAL,
            withholds: &[PROXY_ANSWER, NO_CLAIMANT],
        }),
    },
    Row {
        // And the same shape, spelled with the other API version, on a bare socket: nobody.
        case: "a REST path spelled with the ELBv2 API version, on a bare socket",
        wire: Wire::Path("/2015-12-01/load-balancers/describe"),
        host: BARE_HOST,
        addressed: Addressed::Signed(SHARED_SIGNING_NAME),
        claimants: &[],
        answer: Some(Answer {
            status: 404,
            carries: &[NO_CLAIMANT],
            withholds: &["<ErrorResponse", PROXY_ANSWER, CLASSIC_ANSWER, V2_ANSWER],
        }),
    },
    Row {
        // The peer's own form under a key that is not this family's: the key decides first,
        // so none of the three claims it. What answers a request for another service's key
        // is that service's business, so only the claim half of this row applies.
        case: "the peer's RPC form signed for a key outside this family",
        wire: Wire::Rpc {
            service: PROXY_SERVICE_SHAPE,
            operation: "ListProxies",
        },
        host: BARE_HOST,
        addressed: Addressed::Signed("sqs"),
        claimants: &[],
        answer: None,
    },
];

/// A SigV4 `Authorization` whose credential scope names `signing_name`. No signature is
/// verified here: a claim seam reads only the scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 \
         Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=0123456789abcdef"
    )
}

/// A presigned credential scope naming the shared signing name, with its `/` separators
/// encoded as a query string carries them.
const PRESIGNED: &str =
    "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Felasticloadbalancing%2Faws4_request";

/// The request one row describes.
fn request_of(row: &Row) -> http::Request<bytes::Bytes> {
    let path = match row.wire {
        Wire::Query { .. } => "/".to_string(),
        Wire::Rpc { service, operation } => format!("/service/{service}/operation/{operation}"),
        Wire::Path(path) => path.to_string(),
    };
    let uri = match row.addressed {
        Addressed::Presigned => format!("{path}?{PRESIGNED}"),
        _ => path,
    };
    let mut builder = http::Request::builder()
        .method("POST")
        .uri(uri)
        .header("host", row.host);
    let body = match row.wire {
        Wire::Query { action, version } => {
            builder = builder.header("content-type", "application/x-www-form-urlencoded");
            bytes::Bytes::from(format!("Action={action}&Version={version}"))
        }
        Wire::Rpc { .. } => {
            builder = builder
                .header("content-type", CBOR_MEDIA_TYPE)
                .header("accept", CBOR_MEDIA_TYPE)
                .header(CBOR_PROTOCOL.0, CBOR_PROTOCOL.1);
            bytes::Bytes::from_static(CBOR_EMPTY_MAP)
        }
        Wire::Path(_) => bytes::Bytes::new(),
    };
    if let Addressed::Signed(signing_name) = row.addressed {
        builder = builder.header("authorization", authorization(signing_name));
    }
    builder.body(body).expect("the request is well formed")
}

/// The claim half: each of the three services is asked directly, which is where a second
/// claimant — the condition the router renders as `500 AmbiguousService` — is visible as
/// such, and where a request no surface of the family publishes is visibly nobody's.
#[test]
fn every_request_of_the_shared_name_is_claimed_by_the_service_whose_surface_it_addresses() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let v2 =
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        );
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    for row in ROWS {
        let parts = request_of(row).into_parts().0;
        let claimants: Vec<&str> = [
            (CLASSIC, classic.can_handle(&parts)),
            (V2, v2.can_handle(&parts)),
            (PROXY, proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(name, claimed)| claimed.then_some(name))
        .collect();
        assert_eq!(
            claimants,
            row.claimants.to_vec(),
            "case {:?} (POST {} to host {}, {:?}): expected it to be claimed by exactly \
             {:?}, got {claimants:?}",
            row.case,
            parts.uri,
            row.host,
            row.addressed,
            row.claimants
        );
    }
}

/// The dispatch half: the same rows through the WHOLE assembled router, so each one proves
/// the request reached the one service that owns it — or nobody, which is the router's own
/// answer rather than a service answering for a request that addressed neither its wire
/// form nor its endpoint.
#[test]
fn the_assembled_router_answers_each_row_out_of_the_service_that_owns_it() {
    std::thread::Builder::new()
        .name("elb-shared-name-claims".to_string())
        .stack_size(BOOT_STACK_SIZE)
        .spawn(|| {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build a current-thread runtime");
            runtime.block_on(async {
                let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
                let mut router = build_router(engine);
                // Signature verification belongs to the emulator, not to this check: these
                // rows are about which service claims a request.
                router.set_request_authentication(false);

                for row in ROWS {
                    let Some(answer) = &row.answer else {
                        continue;
                    };
                    let response = router.dispatch(request_of(row)).await;
                    let status = response.status().as_u16();
                    let answered = String::from_utf8_lossy(response.body().as_ref()).to_string();

                    assert!(
                        !answered.contains("AmbiguousService"),
                        "case {:?}: two services claimed one request, expected exactly one \
                         claimant, got {status} with {answered}",
                        row.case
                    );
                    assert_eq!(
                        status, answer.status,
                        "case {:?}: expected HTTP {} from the service that owns the request, \
                         got {status} with {answered}",
                        row.case, answer.status
                    );
                    for expected in answer.carries {
                        assert!(
                            answered.contains(expected),
                            "case {:?}: expected the owning service's answer to carry \
                             {expected:?}, got {status} with {answered}",
                            row.case
                        );
                    }
                    for forbidden in answer.withholds {
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
        .expect("every service of the shared signing name claims only the requests it owns");
}
