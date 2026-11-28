// SPDX-License-Identifier: CC0-1.0

//! THREE emulated services declare the `elasticloadbalancing` SigV4 signing name —
//! a request addressed to that key must still be claimed by exactly ONE of them.
//!
//! Classic Elastic Load Balancing (`sdkId: Elastic Load Balancing`), Elastic Load
//! Balancing v2 (`sdkId: Elastic Load Balancing v2`) and ELB Proxy (`sdkId: ELB
//! Proxy`, `endpointPrefix: elb-proxy`, `rpcv2Cbor`) all declare `aws.auth#sigv4`
//! name `elasticloadbalancing` (`smithy_query(query="service_info")` on each of the
//! three models), and the router resolves a request's routing key from the SigV4
//! credential scope FIRST — so a service that claims on its signing name alone
//! claims its peers' traffic too, the router finds several claimants and answers
//! `500 AmbiguousService` instead of serving any of them.
//!
//! The rule this table pins, for the awsQuery front door that key addresses: the
//! shared name resolves to the ONE service whose own identity IS that name (the
//! Classic model), and every peer is claimed by the key that names IT. The sibling
//! suite `elb_classic_and_v2_signing_name_routing.rs` pins the same rule over the
//! two Elastic Load Balancing models and drives their traffic end to end; this one
//! asks every service that declares the shared name, so a service added to the
//! family later cannot quietly make that front door ambiguous again. How a request
//! in ELB Proxy's own `rpcv2Cbor` wire form (`POST
//! /service/<Service>/operation/<Operation>`) is claimed is that crate's own rule
//! and its own suites'; no row here is about it.
//!
//! It asks each service's [`smithy_server_runtime::Service::can_handle`] directly,
//! so a row pins both that the right service claims a request and that NO other
//! does — the condition the router turns into `500 AmbiguousService` when it fails.

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;

/// The region every signed request below is signed for.
const REGION: &str = "us-east-1";

/// How a row names the service its request is for.
#[derive(Debug, Clone, Copy)]
enum Claiming {
    /// A SigV4 `Authorization` scope naming this routing key — what every AWS SDK
    /// sends, and the signal the router reads first.
    Signed(&'static str),
    /// A presigned URL's `X-Amz-Credential` query parameter naming the shared name.
    Presigned,
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// The SigV4 `Authorization` header of a request signed for `signing_name`, as every
/// AWS SDK sends it. The signature is never checked here: a claim predicate reads
/// only the credential scope.
fn authorization(signing_name: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/aws4_request, \
         SignedHeaders=host, Signature=not-verified"
    )
}

/// One row: how the request is addressed, its `Host`, and the services that must
/// claim it, named by routing key.
struct ClaimRow {
    case: &'static str,
    claiming: Claiming,
    host: &'static str,
    expected: &'static [&'static str],
}

#[test]
fn exactly_one_service_of_the_elastic_load_balancing_family_claims_each_request() {
    use smithy_server_runtime::Service;

    /// A presigned credential scope naming the shared signing name, with its `/`
    /// separators encoded.
    const PRESIGNED: &str =
        "X-Amz-Credential=AKIDEXAMPLE%2F20260101%2Fus-east-1%2Felasticloadbalancing%2Faws4_request";

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

    // Every awsQuery request of this family is `POST /`: none of these models binds a
    // `smithy.api#http` trait, so the path carries no discriminator and the routing key
    // decides alone. The rows span the three signals that resolve it (a credential
    // scope, a presigned query scope, the `Host` label of an unsigned request), the two
    // keys this front door is addressed by, each endpoint host of the family, and a host
    // belonging to none of them.
    let rows: &[ClaimRow] = &[
        ClaimRow {
            case: "a signed request on the Elastic Load Balancing host",
            claiming: Claiming::Signed("elasticloadbalancing"),
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            case: "a signed request on a bare emulator endpoint",
            claiming: Claiming::Signed("elasticloadbalancing"),
            host: "localhost:9297",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            case: "a presigned request in the query string",
            claiming: Claiming::Presigned,
            host: "elasticloadbalancing.eu-west-2.amazonaws.com",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            case: "an anonymous request addressed to the Elastic Load Balancing host",
            claiming: Claiming::Anonymous,
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            // The scope decides, not the host: a request signed for the shared name
            // while addressed to another endpoint of the family is still the Classic
            // front door's.
            case: "a request signed for the shared name on the ELB Proxy host",
            claiming: Claiming::Signed("elasticloadbalancing"),
            host: "elb-proxy.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            case: "a request signed for the shared name on the ELBv2 host",
            claiming: Claiming::Signed("elasticloadbalancing"),
            host: "elasticloadbalancingv2.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancing"],
        },
        ClaimRow {
            case: "a request signed for the ELBv2 model's own key",
            claiming: Claiming::Signed("elasticloadbalancingv2"),
            host: "elasticloadbalancing.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancingv2"],
        },
        ClaimRow {
            case: "an anonymous request addressed to the ELBv2 host",
            claiming: Claiming::Anonymous,
            host: "elasticloadbalancingv2.us-east-1.amazonaws.com",
            expected: &["elasticloadbalancingv2"],
        },
        ClaimRow {
            // Claimed by none of the three, which is the router's `404 UnknownService`
            // and NOT an ambiguity.
            case: "an anonymous request addressed to an unrelated host",
            claiming: Claiming::Anonymous,
            host: "sqs.us-east-1.amazonaws.com",
            expected: &[],
        },
    ];

    for row in rows {
        let uri = match row.claiming {
            Claiming::Presigned => format!("/?{PRESIGNED}"),
            _ => "/".to_string(),
        };
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("host", row.host);
        if let Claiming::Signed(signing_name) = row.claiming {
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
            ("elb-proxy", proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        let (case, host, claiming) = (row.case, row.host, row.claiming);
        assert_eq!(
            claimants,
            row.expected.to_vec(),
            "case {case:?} (POST / to host {host}, {claiming:?}): expected it to be claimed \
             by exactly {:?}, got {claimants:?}",
            row.expected
        );
    }
}
