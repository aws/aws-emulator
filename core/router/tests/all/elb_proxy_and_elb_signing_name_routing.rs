// SPDX-License-Identifier: CC0-1.0

//! The three services that sign the one `elasticloadbalancing` name — the Classic Elastic
//! Load Balancing model, the ELBv2 model and ELB Proxy (`sigv4Name: elasticloadbalancing` on
//! each, `smithy_query(query="service_info")`) — asked which of them claims a request that
//! carries NO CREDENTIALS at all.
//!
//! The rule the sibling claim tables pin over SIGNED requests
//! (`elb_signing_name_family_claims`, `elb_classic_and_v2_signing_name_routing`) is that a
//! request under the shared name belongs to the one model whose own wire form binds it: the
//! two query models post every action to `/`, where the Classic front door owns the name,
//! while ELB Proxy is `rpcv2Cbor` and names itself in its path
//! (`POST /service/ELBProxy/operation/{Operation}`). What this suite adds is that the rule is
//! the same when the routing key comes from the `Host` label instead of a SigV4 credential
//! scope — the last signal the router reads, and the only one an anonymous request carries —
//! so an unsigned container-client-style call reaches the same model a signed one does, and no
//! request has two claimants or none where one was due.

use std::sync::Arc;

use engine::InMemoryEngine;
use smithy_server_runtime::Service;

/// The host-style endpoint an anonymous Elastic Load Balancing request addresses, whose first
/// label is the routing key all three models share.
const SHARED_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";

/// One `rpcv2Cbor` request path of the ELB Proxy model: `ListProxies` is bound by that model
/// (`smithy_query(query="digest")`), and the wire form of an `rpcv2Cbor` operation is
/// `POST /service/{Service}/operation/{Operation}`.
const PROXY_ROUTE: &str = "/service/ELBProxy/operation/ListProxies";

#[test]
fn an_unsigned_request_on_the_shared_endpoint_host_keeps_one_claimant() {
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

    // (case, the host the request is addressed to, its path, the models that must claim it)
    let rows: [(&str, &str, &str, &[&str]); 3] = [
        (
            "an unsigned awsQuery post on the shared endpoint host",
            SHARED_HOST,
            "/",
            &["elasticloadbalancing"],
        ),
        (
            "an unsigned ELB Proxy route on the shared endpoint host",
            SHARED_HOST,
            PROXY_ROUTE,
            &["elb-proxy"],
        ),
        (
            "an unsigned request on a host that names no Elastic Load Balancing endpoint",
            "sqs.us-east-1.amazonaws.com",
            "/",
            &[],
        ),
    ];

    for (case, host, path, expected) in rows {
        let parts = http::Request::builder()
            .method("POST")
            .uri(path)
            .header("host", host)
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
        assert_eq!(
            claimants,
            expected.to_vec(),
            "case {case:?} (unsigned POST {path} to host {host}): expected it to be claimed by \
             exactly {expected:?}, got {claimants:?}"
        );
    }
}
