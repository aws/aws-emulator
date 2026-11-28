// SPDX-License-Identifier: CC0-1.0

//! The three APIs that sign as `elasticloadbalancing` each claim only their OWN traffic.
//!
//! RULE: a request reaches exactly one emulated service. Where several models share one
//! SigV4 signing name — here Classic Elastic Load Balancing, ELBv2 and ELB Proxy, all three
//! of which publish `sigv4Name: elasticloadbalancing`
//! (`smithy_query(query="service_info")` on each model) — the credential scope every AWS SDK
//! signs resolves to that one key for all of them, so the key alone cannot say which service
//! a request is for. The service a request NAMES claims it: an `rpcv2Cbor` request names its
//! service in its own URI (`POST /service/<Service>/operation/<Operation>`), which is how ELB
//! Proxy's traffic is told from the `awsQuery` envelopes of the two Elastic Load Balancing
//! APIs, and those two are told apart by the `Version` their envelope carries (the sibling
//! suite `elb_classic_and_v2_signing_name_routing.rs` is that half of the rule, and this one
//! is the half the third model added).
//!
//! A request claimed by two services is answered `500 AmbiguousService` — a code no AWS model
//! carries, in a JSON body neither an awsQuery nor a CBOR client can parse — so the condition
//! under test is that no row below has more than one claimant.
//!
//! The rows span the forms a request can name one of the three by: the `awsQuery` root post
//! each Elastic Load Balancing client sends (signed for the shared name and for the ELBv2
//! model's own key), the `rpcv2Cbor` path form, that same path naming a service none of the
//! three is, and a request that names no service at all.

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;
use smithy_server_runtime::Service;

/// The Region every signed row names, which no claim rule here reads.
const REGION: &str = "us-east-1";

/// The `rpcv2Cbor` service shape ELB Proxy's wire names it by, as its model declares it
/// (`smithy_query(query="service_info")`: one protocol, `rpcv2Cbor`, on service `ELBProxy`).
const ELB_PROXY_SHAPE: &str = "ELBProxy";

/// The media type an `rpcv2Cbor` request carries its body under.
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// How a row names the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 `Authorization` scope naming this signing name.
    Signed(&'static str),
    /// No credentials at all, so nothing but the `Host` can name a service.
    Anonymous,
}

/// The SigV4 `Authorization` header a request signed for `signing_name` carries, as every AWS
/// SDK sends it. The signature is never checked: a claim rule reads only the credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

#[tokio::test]
async fn each_elasticloadbalancing_signer_claims_only_the_requests_that_name_it() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let v2 =
        service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(
            Arc::clone(&engine),
        );
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(Arc::clone(&engine));

    let cbor_path = format!("/service/{ELB_PROXY_SHAPE}/operation/ListProxies");
    // (case, path, how the request is addressed, its Host, the services that must claim it)
    let rows: [(&str, &str, Addressed, &str, &[&str]); 7] = [
        (
            "the awsQuery root post every Elastic Load Balancing client sends",
            "/",
            Addressed::Signed("elasticloadbalancing"),
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            "an awsQuery post signed for the ELBv2 model's own key",
            "/",
            Addressed::Signed("elasticloadbalancingv2"),
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancingv2"],
        ),
        (
            // The form under test: the shared signing name, with ELB Proxy named in band by
            // the URI its one protocol addresses an operation through.
            "an rpcv2Cbor request naming ELB Proxy, signed for the shared name",
            &cbor_path,
            Addressed::Signed("elasticloadbalancing"),
            "elb-proxy.us-east-1.amazonaws.com",
            &["elbproxy"],
        ),
        (
            // The same form on the emulator's own socket: the path, not the host, names the
            // service.
            "an rpcv2Cbor request naming ELB Proxy on a bare emulator endpoint",
            &cbor_path,
            Addressed::Signed("elasticloadbalancing"),
            "localhost:9297",
            &["elbproxy"],
        ),
        (
            // The ELB Proxy path form naming a service none of the three is: claimed by
            // nobody, which is the router's `404 UnknownService` and not an ambiguity.
            "an rpcv2Cbor request naming another service altogether",
            "/service/ElsewhereService/operation/ListThings",
            Addressed::Signed("elasticloadbalancing"),
            "localhost:9297",
            &[],
        ),
        (
            "an anonymous awsQuery post addressed to the Elastic Load Balancing host",
            "/",
            Addressed::Anonymous,
            "elasticloadbalancing.us-east-1.amazonaws.com",
            &["elasticloadbalancing"],
        ),
        (
            "an anonymous request addressed to an unrelated host",
            "/",
            Addressed::Anonymous,
            "sqs.us-east-1.amazonaws.com",
            &[],
        ),
    ];

    for (case, path, addressed, host, expected) in rows {
        let mut builder = ::http::Request::builder()
            .method("POST")
            .uri(path)
            .header("host", host);
        builder = match path == "/" {
            true => builder.header("content-type", "application/x-www-form-urlencoded"),
            false => builder.header("content-type", CBOR_MEDIA_TYPE),
        };
        if let Addressed::Signed(signing_name) = addressed {
            builder = builder.header("authorization", authorization(signing_name));
        }
        let parts = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            ("elasticloadbalancing", classic.can_handle(&parts)),
            ("elasticloadbalancingv2", v2.can_handle(&parts)),
            ("elbproxy", proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        // The condition the router turns into `500 AmbiguousService` when it fails, stated
        // first so a second claimant is reported as that rather than as an unexpected list.
        assert!(
            claimants.len() <= 1,
            "case {case:?} (POST {path} to host {host}, {addressed:?}): a request must reach \
             at most one service, got {claimants:?}"
        );
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} (POST {path} to host {host}, {addressed:?}): expected it to be \
             claimed by exactly {expected:?}, got {claimants:?}"
        );
    }
}
