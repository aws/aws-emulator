// SPDX-License-Identifier: CC0-1.0

//! The Elastic Load Balancing front door stays unambiguous with a THIRD service mounted
//! under its signing name.
//!
//! `ELBProxy` signs as `elasticloadbalancing`
//! (`smithy_query(query="service_info")`: sigv4Name `elasticloadbalancing`, endpointPrefix
//! `elb-proxy`, protocols `["rpcv2Cbor"]`) — the name both Elastic Load Balancing models
//! are addressed by, and the name every AWS SDK for any of the three signs. A service that
//! claims on that signing name alone claims the others' traffic too, and the router answers
//! `500 AmbiguousService` — a code that exists in no AWS model — for every Elastic Load
//! Balancing request.
//!
//! RULE this suite pins, from the router's side: **however many services a signing name is
//! shared by, the awsQuery front door that name addresses has EXACTLY ONE claimant.** The
//! rows differ along the signal that resolves the routing key (a SigV4 credential scope, a
//! `Host` label with no credentials at all) and the key it names (the shared name, the key
//! that names the ELBv2 model alone).
//!
//! Only the front door is asked here, because that is the surface the three share. What
//! each service claims of its OWN wire surface is its own suite's: the `rpcv2Cbor` path
//! `/service/ELBProxy/operation/<Operation>` is
//! `services/service-com-amazonaws-elbproxy-elbproxy/tests/all/signing_name_claim.rs`, and
//! the two Elastic Load Balancing APIs' dispatch is the sibling
//! `elb_classic_and_v2_signing_name_routing`.

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;
use smithy_server_runtime::Service;

/// The region every signed row below is signed for.
const REGION: &str = "us-east-1";

/// The signing name all three models declare.
const SHARED_NAME: &str = "elasticloadbalancing";

/// The routing key that names the ELBv2 model alone.
const V2_KEY: &str = "elasticloadbalancingv2";

/// The routing key that names the ELB Proxy model alone: its own endpoint prefix.
const PROXY_KEY: &str = "elb-proxy";

/// The host an Elastic Load Balancing SDK addresses the shared front door by.
const ELB_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";

/// How a row names the service it is for.
#[derive(Debug, Clone, Copy)]
enum Addressed {
    /// A SigV4 credential scope naming this routing key — the signal the router reads
    /// first, and what every AWS SDK sends.
    Signed(&'static str),
    /// No credentials at all, so the `Host` label resolves the key.
    Anonymous,
}

/// One row: how the awsQuery request is addressed, and which service must claim it, named
/// by its own routing key.
struct Row {
    case: &'static str,
    addressed: Addressed,
    host: &'static str,
    expected: &'static str,
}

/// Exactly one service claims each awsQuery request for the shared signing name.
#[test]
fn the_load_balancing_front_door_has_one_claimant_with_the_proxy_mounted() {
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

    let rows: &[Row] = &[
        Row {
            case: "the awsQuery front door, signed for the shared name",
            addressed: Addressed::Signed(SHARED_NAME),
            host: ELB_HOST,
            expected: SHARED_NAME,
        },
        Row {
            case: "the awsQuery front door, anonymous on the Elastic Load Balancing host",
            addressed: Addressed::Anonymous,
            host: ELB_HOST,
            expected: SHARED_NAME,
        },
        Row {
            case: "the awsQuery front door, signed for the ELBv2 key",
            addressed: Addressed::Signed(V2_KEY),
            host: ELB_HOST,
            expected: V2_KEY,
        },
    ];

    for row in rows {
        let mut builder = ::http::Request::builder()
            .method("POST")
            // Every awsQuery operation of either model posts to the root path and names
            // its action in the form body, which `can_handle` is not handed.
            .uri(format!("http://{}/", row.host))
            .header("host", row.host)
            .header("content-type", "application/x-www-form-urlencoded");
        if let Addressed::Signed(signing_name) = row.addressed {
            builder = builder.header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{signing_name}/\
                     aws4_request, SignedHeaders=host, Signature=not-verified"
                ),
            );
        }
        let request = builder
            .body(bytes::Bytes::new())
            .expect("a well-formed request");
        let (parts, _) = request.into_parts();

        // Which of the three claimed it, each named by its own routing key.
        let mut claimed: Vec<&str> = Vec::new();
        if classic.can_handle(&parts) {
            claimed.push(SHARED_NAME);
        }
        if v2.can_handle(&parts) {
            claimed.push(V2_KEY);
        }
        if proxy.can_handle(&parts) {
            claimed.push(PROXY_KEY);
        }
        assert_eq!(
            claimed,
            vec![row.expected],
            "case {:?}: exactly {:?} must claim this request, got {claimed:?}",
            row.case,
            row.expected
        );
    }
}
