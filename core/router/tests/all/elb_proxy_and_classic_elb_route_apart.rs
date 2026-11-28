// SPDX-License-Identifier: CC0-1.0

//! An ANONYMOUS request on ELB Proxy's own `rpcv2Cbor` surface still has exactly one
//! claimant.
//!
//! Three emulated services publish `aws.auth#sigv4` name `elasticloadbalancing`
//! (`smithy_query(query="service_info")` on each), so the routing key every request
//! resolves to names all of them and a service claiming that key alone claims its peers'
//! traffic — which the router answers `500 AmbiguousService`, a code no AWS model carries.
//! What separates them is the PATH each wire protocol publishes: `rpcv2Cbor` arrives at
//! `POST /service/{Service}/operation/{Operation}` and names the service there, while the
//! two Elastic Load Balancing query APIs post every action to `/`.
//!
//! The sibling suites — `elb_signing_name_family_claims.rs`,
//! `elb_proxy_claims_only_its_rpc_surface.rs`, `elb_proxy_shared_signing_name_claim.rs`,
//! `elbproxy_shared_signing_name_routing.rs` and
//! `elb_classic_and_v2_signing_name_routing.rs` — table that rule for requests carrying a
//! SigV4 credential scope, which is the tier the router reads first. This one covers the
//! case none of them does: a request with NO credentials at all, whose key comes from the
//! host label instead, on each of the two endpoint hosts of that family. The claim must
//! still rest on the path, so an unsigned registry-style client reaches ELB Proxy and an
//! unsigned query client reaches the front door that owns the shared name.

use std::sync::Arc;

use aws_emulator_router::InMemoryEngine;
use smithy_server_runtime::Service;

/// The `smithy-protocol` header value the `rpcv2Cbor` specification requires of every
/// request of that protocol, and that protocol's media type.
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";
const CBOR_MEDIA_TYPE: &str = "application/cbor";

/// A CBOR empty map (major type 5, length 0) — the body of a `ListProxies` call, whose
/// members are all optional.
const CBOR_EMPTY_MAP: &[u8] = &[0xA0];

/// ELB Proxy's own `rpcv2Cbor` path, naming the service as that protocol's wire form does.
const PROXY_PATH: &str = "/service/ELBProxy/operation/ListProxies";

/// The endpoint hosts of this family: the Elastic Load Balancing one, whose first label is
/// the shared signing name, and ELB Proxy's own `endpointPrefix`.
const ELB_HOST: &str = "elasticloadbalancing.us-east-1.amazonaws.com";
const PROXY_HOST: &str = "elb-proxy.us-east-1.amazonaws.com";

/// One unsigned request: the host that resolves its routing key, the path and headers that
/// name its wire, and the services that must claim it.
struct Row {
    case: &'static str,
    host: &'static str,
    uri: &'static str,
    headers: &'static [(&'static str, &'static str)],
    body: &'static [u8],
    claimed_by: &'static [&'static str],
}

/// The awsQuery body of a Classic Elastic Load Balancing read, which names its API version
/// in the envelope as that protocol requires.
const QUERY_BODY: &[u8] = b"Action=DescribeLoadBalancers&Version=2012-06-01";

const ROWS: &[Row] = &[
    Row {
        // The rule under test: unsigned, so the host label resolves the shared key, and the
        // cbor path is what says the request is ELB Proxy's.
        case: "an unsigned cbor request on the Elastic Load Balancing host",
        host: ELB_HOST,
        uri: PROXY_PATH,
        headers: &[
            ("content-type", CBOR_MEDIA_TYPE),
            ("smithy-protocol", CBOR_PROTOCOL),
        ],
        body: CBOR_EMPTY_MAP,
        claimed_by: &["elbproxy"],
    },
    Row {
        // The same wire on ELB Proxy's own endpoint host, whose first label is its
        // `endpointPrefix` rather than the shared signing name: no service of this family
        // publishes that key, so the request reaches nobody — the router's
        // `404 UnknownService`, not an ambiguity.
        case: "an unsigned cbor request on the ELB Proxy endpoint host",
        host: PROXY_HOST,
        uri: PROXY_PATH,
        headers: &[
            ("content-type", CBOR_MEDIA_TYPE),
            ("smithy-protocol", CBOR_PROTOCOL),
        ],
        body: CBOR_EMPTY_MAP,
        claimed_by: &[],
    },
    Row {
        // The other wire, unsigned on the same host: the query front door that owns the
        // shared name, and not the RPC peer.
        case: "an unsigned awsQuery envelope on the Elastic Load Balancing host",
        host: ELB_HOST,
        uri: "/",
        headers: &[("content-type", "application/x-www-form-urlencoded")],
        body: QUERY_BODY,
        claimed_by: &["elasticloadbalancing"],
    },
];

/// It asks each service's [`smithy_server_runtime::Service::can_handle`] directly, one
/// layer below dispatch: that is where a second claimant is visible as such rather than as
/// the one refusal the router renders for it.
#[test]
fn an_unsigned_request_of_the_shared_elb_signing_name_has_one_claimant() {
    let engine: Arc<InMemoryEngine> = Arc::new(InMemoryEngine::new());
    let classic =
        service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(
            Arc::clone(&engine),
        );
    let proxy = service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine);

    for row in ROWS {
        let case = row.case;
        let mut builder = http::Request::builder()
            .method("POST")
            .uri(row.uri)
            .header("host", row.host);
        for (name, value) in row.headers {
            builder = builder.header(*name, *value);
        }
        let parts = builder
            .body(bytes::Bytes::from_static(row.body))
            .expect("a well-formed request")
            .into_parts()
            .0;
        let claimants: Vec<&str> = [
            ("elasticloadbalancing", classic.can_handle(&parts)),
            ("elbproxy", proxy.can_handle(&parts)),
        ]
        .into_iter()
        .filter_map(|(key, claimed)| claimed.then_some(key))
        .collect();
        assert_eq!(
            claimants,
            row.claimed_by.to_vec(),
            "case {case:?} (POST {} to host {}): expected it to be claimed by exactly {:?}, \
             got {claimants:?}",
            row.uri,
            row.host,
            row.claimed_by
        );
    }
}
