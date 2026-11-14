// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateRouteServerPeer`, the two creates a peer needs under it
//! (`CreateRouteServer` and `CreateRouteServerEndpoint`) and the `DescribeRouteServerPeers`
//! it reads back through, driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateRouteServerPeer`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * a create STORES THE RESOURCE THE REQUEST DESCRIBES — the peer carries the requested
//!   peer address and BGP options, with the liveness protocol the request omitted at the
//!   default its own documentation publishes, and every id it reports (its route server,
//!   VPC, subnet and the endpoint's network interface) is read off the ENDPOINT it was
//!   created on rather than from the request; it is answered in the non-terminal `pending`
//!   state the clock then owes `available`; and every member of it reads back through
//!   `DescribeRouteServerPeers`;
//! * a create whose `RouteServerEndpointId` resolves to no endpoint of the account is
//!   refused with `InvalidRouteServerEndpointId.NotFound`, and that refusal comes BEFORE
//!   the BGP options are read, so it is the answer even when those options are themselves
//!   invalid;
//! * a DELETE takes the peer the request names DOWN — it answers that stored peer, member
//!   for member, in the non-terminal `deleting` state the clock then owes `deleted`,
//!   whatever state the peer was in before the call; an id resolving to no peer of the
//!   account is refused with `InvalidRouteServerPeerId.NotFound`; and a peer whose deletion
//!   is already underway is refused with `IncorrectState` rather than deleted twice.
//!
//! Every table row is a distinct input along what those rules generalise over — the ASN at
//! both published bounds and inside both recommended private ranges, each published
//! liveness protocol and its absence, the peer address, the tag count, which endpoint (and
//! so which route server, VPC and subnet) the peer belongs to, and the form of an id that
//! names nothing — so the one case a recording happens to contain is one row among many
//! rather than the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for. The default VPC's subnets — the subnets a
/// route server endpoint is placed into below — are seeded for the emulator's default
/// Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it. The store is
/// reached only through the wire here, so the engine handle is not needed.
fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One call of `action` carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One call of `action` requiring success, and the body it answered.
async fn call_ok(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The subnet ids the account holds, read over the wire rather than out of the store.
async fn subnet_ids(router: &MultiServiceRouter) -> Vec<String> {
    let body = call_ok("the account's subnets", router, "DescribeSubnets", &[]).await;
    elements(&body, "subnetId")
}

/// The facts of one created route server endpoint a peer of it must inherit.
#[derive(Debug, Clone)]
struct Endpoint {
    id: String,
    route_server_id: String,
    vpc_id: String,
    subnet_id: String,
    eni_id: String,
    eni_address: String,
}

/// One row of the create table: a distinct input along what the rule generalises over,
/// beside the liveness protocol the peer must then report (the requested one, or the
/// published default when the request named none).
struct PeerCase<'a> {
    case: &'a str,
    endpoint: &'a Endpoint,
    peer_address: &'a str,
    peer_asn: &'a str,
    liveness: Option<&'a str>,
    expected_liveness: &'a str,
    tags: Vec<(&'a str, &'a str)>,
}

/// A route server with `amazon_side_asn`, and its id.
async fn create_route_server(
    case: &str,
    router: &MultiServiceRouter,
    amazon_side_asn: &str,
) -> String {
    let body = call_ok(
        case,
        router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", amazon_side_asn)],
    )
    .await;
    element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"))
}

/// An endpoint of `route_server_id` in `subnet_id`, and the facts of it.
async fn create_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
    subnet_id: &str,
) -> Endpoint {
    let body = call_ok(
        case,
        router,
        "CreateRouteServerEndpoint",
        &[
            param("RouteServerId", route_server_id),
            param("SubnetId", subnet_id),
        ],
    )
    .await;
    let read = |name: &str| {
        element(&body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
    };
    Endpoint {
        id: read("routeServerEndpointId"),
        route_server_id: read("routeServerId"),
        vpc_id: read("vpcId"),
        subnet_id: read("subnetId"),
        eni_id: read("eniId"),
        eni_address: read("eniAddress"),
    }
}

/// The form fields of one `CreateRouteServerPeer` request.
fn peer_params(
    endpoint_id: &str,
    peer_address: &str,
    peer_asn: &str,
    liveness: Option<&str>,
    tags: &[(&str, &str)],
) -> Vec<Param> {
    let mut params = vec![
        param("RouteServerEndpointId", endpoint_id),
        param("PeerAddress", peer_address),
        param("BgpOptions.PeerAsn", peer_asn),
    ];
    if let Some(liveness) = liveness {
        params.push(param("BgpOptions.PeerLivenessDetection", liveness));
    }
    if !tags.is_empty() {
        params.push(param(
            "TagSpecification.1.ResourceType",
            ec2::state::RESOURCE_TYPE_ROUTE_SERVER_PEER,
        ));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(param(&format!("TagSpecification.1.Tag.{}.Key", index + 1), key));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    params
}

/// A scalar member of the one route server peer a response carries.
fn peer_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let peer = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "routeServerPeerId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no route server peer in {body}"));
    child_text(peer, name)
}

/// A member of a sub-structure (`bgpOptions`, `bgpStatus`, `bfdStatus`) of the one peer a
/// response carries, or `None` when the peer carries no such sub-structure at all.
fn peer_nested(case: &str, body: &str, structure: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let peer = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "routeServerPeerId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no route server peer in {body}"));
    child(peer, structure).and_then(|node| child_text(node, name))
}

/// The tags of the one peer a response carries, as key/value pairs in document order.
fn peer_tags(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let peer = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "routeServerPeerId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no route server peer in {body}"));
    let Some(set) = child(peer, "tagSet") else {
        return Vec::new();
    };
    set.children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// The peer ids a `DescribeRouteServerPeers` body reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "routeServerPeerId")
}

/// Every stored peer's id, read over the wire.
async fn all_peer_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = call_ok(case, router, "DescribeRouteServerPeers", &[]).await;
    reported_ids(&body)
}

// ---------------------------------------------------------------------------
// A create stores the resource the request describes
// ---------------------------------------------------------------------------

/// The rule: `CreateRouteServerPeer` stores the peer the request describes — its peer
/// address and its BGP options, with the liveness protocol an omitting request leaves at
/// its published default — inherits its route server, VPC, subnet and endpoint network
/// interface from the ENDPOINT it names, answers it with a server-minted id in `pending`
/// with a BGP session that is not up, and every member of it reads back through
/// `DescribeRouteServerPeers`.
///
/// The rows differ in the ASN (both published bounds and a value inside each recommended
/// private range), the liveness protocol (absent, each published member), the peer address,
/// the tag count, and which endpoint — and so which route server, VPC and subnet — the peer
/// belongs to.
#[tokio::test]
async fn create_route_server_peer_stores_the_request_and_round_trips_through_describe() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    assert!(
        subnets.len() >= 2,
        "the account must hold at least two subnets to place two endpoints into, got {subnets:?}"
    );

    // Two endpoints, of two route servers, in two subnets: what a peer inherits is then a
    // property of its endpoint rather than of the one endpoint a recording used.
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;
    let first = create_endpoint("the first endpoint", &router, &first_server, &subnets[0]).await;
    let second = create_endpoint("the second endpoint", &router, &second_server, &subnets[1]).await;
    assert_ne!(
        first.subnet_id, second.subnet_id,
        "the two endpoints must sit in different subnets, got {:?} twice",
        first.subnet_id
    );
    assert_ne!(
        first.eni_address, second.eni_address,
        "each endpoint must get its own network interface address, got {:?} twice",
        first.eni_address
    );

    // One row per distinct input along what the rule generalises over.
    let cases: Vec<PeerCase<'_>> = vec![
        PeerCase {
            case: "the published ASN floor, no liveness asked for",
            endpoint: &first,
            peer_address: "10.0.1.5",
            peer_asn: "1",
            liveness: None,
            expected_liveness: ec2::state::PEER_LIVENESS_BGP_KEEPALIVE,
            tags: Vec::new(),
        },
        PeerCase {
            case: "the published ASN ceiling, BGP keepalive asked for",
            endpoint: &first,
            peer_address: "192.168.44.7",
            peer_asn: "4294967295",
            liveness: Some(ec2::state::PEER_LIVENESS_BGP_KEEPALIVE),
            expected_liveness: ec2::state::PEER_LIVENESS_BGP_KEEPALIVE,
            tags: vec![("role", "firewall")],
        },
        PeerCase {
            case: "a 16-bit private ASN with BFD liveness",
            endpoint: &first,
            peer_address: "172.16.0.200",
            peer_asn: "65000",
            liveness: Some(ec2::state::PEER_LIVENESS_BFD),
            expected_liveness: ec2::state::PEER_LIVENESS_BFD,
            tags: Vec::new(),
        },
        PeerCase {
            case: "a 32-bit private ASN on the second endpoint",
            endpoint: &second,
            peer_address: "198.51.100.42",
            peer_asn: "4200000000",
            liveness: None,
            expected_liveness: ec2::state::PEER_LIVENESS_BGP_KEEPALIVE,
            tags: vec![("role", "router"), ("tier", "edge")],
        },
        PeerCase {
            case: "the top of the recommended 32-bit range, BFD, on the second endpoint",
            endpoint: &second,
            peer_address: "203.0.113.9",
            peer_asn: "4294967294",
            liveness: Some(ec2::state::PEER_LIVENESS_BFD),
            expected_liveness: ec2::state::PEER_LIVENESS_BFD,
            tags: vec![("role", "appliance")],
        },
    ];

    for PeerCase {
        case,
        endpoint,
        peer_address,
        peer_asn,
        liveness,
        expected_liveness,
        tags,
    } in cases
    {
        let created = call_ok(
            case,
            &router,
            "CreateRouteServerPeer",
            &peer_params(&endpoint.id, peer_address, peer_asn, liveness, &tags),
        )
        .await;

        let peer_id = peer_member(case, &created, "routeServerPeerId")
            .unwrap_or_else(|| panic!("case {case:?}: no routeServerPeerId in {created}"));
        assert!(
            is_long_id("rsp", &peer_id),
            "case {case:?}: expected a server-minted route server peer id, got {peer_id:?}"
        );

        // Everything the peer inherits from its endpoint, which the request never named.
        for (name, expected) in [
            ("routeServerEndpointId", &endpoint.id),
            ("routeServerId", &endpoint.route_server_id),
            ("vpcId", &endpoint.vpc_id),
            ("subnetId", &endpoint.subnet_id),
            ("endpointEniId", &endpoint.eni_id),
            ("endpointEniAddress", &endpoint.eni_address),
        ] {
            let actual = peer_member(case, &created, name);
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{name}> {expected:?} off the endpoint, got {actual:?}"
            );
        }

        // What the request asked for.
        let reported_address = peer_member(case, &created, "peerAddress");
        assert_eq!(
            reported_address.as_deref(),
            Some(peer_address),
            "case {case:?}: expected peerAddress {peer_address:?}, got {reported_address:?}"
        );
        let reported_asn = peer_nested(case, &created, "bgpOptions", "peerAsn");
        assert_eq!(
            reported_asn.as_deref(),
            Some(peer_asn),
            "case {case:?}: expected bgpOptions/peerAsn {peer_asn:?}, got {reported_asn:?}"
        );
        let reported_liveness = peer_nested(case, &created, "bgpOptions", "peerLivenessDetection");
        assert_eq!(
            reported_liveness.as_deref(),
            Some(expected_liveness),
            "case {case:?}: expected bgpOptions/peerLivenessDetection \
             {expected_liveness:?}, got {reported_liveness:?}"
        );

        // A create answers the non-terminal state, and no session with the appliance is up:
        // nothing in this emulator speaks BGP to the peer address.
        let state = peer_member(case, &created, "state");
        assert_eq!(
            state.as_deref(),
            Some(ec2::state::ROUTE_SERVER_PEER_STATE_PENDING),
            "case {case:?}: expected a create to answer the pending state, got {state:?}"
        );
        let bgp_status = peer_nested(case, &created, "bgpStatus", "status");
        assert_eq!(
            bgp_status.as_deref(),
            Some(ec2::state::ROUTE_SERVER_SESSION_DOWN),
            "case {case:?}: expected a BGP session that is not up, got {bgp_status:?}"
        );
        // BFD is a session only a peer that asked for that liveness mode has.
        let bfd_status = peer_nested(case, &created, "bfdStatus", "status");
        let expected_bfd = (expected_liveness == ec2::state::PEER_LIVENESS_BFD)
            .then_some(ec2::state::ROUTE_SERVER_SESSION_DOWN);
        assert_eq!(
            bfd_status.as_deref(),
            expected_bfd,
            "case {case:?}: expected a BFD status of {expected_bfd:?}, got {bfd_status:?}"
        );

        let created_tags = peer_tags(case, &created);
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created_tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {created_tags:?}"
        );

        // The round trip: the same peer, member for member, out of the reader.
        let read = call_ok(
            case,
            &router,
            "DescribeRouteServerPeers",
            &indexed("RouteServerPeerId", &[&peer_id]),
        )
        .await;
        assert_eq!(
            reported_ids(&read),
            vec![peer_id.clone()],
            "case {case:?}: expected the describe to report exactly the created peer, \
             got {read}"
        );
        for name in [
            "routeServerEndpointId",
            "routeServerId",
            "vpcId",
            "subnetId",
            "endpointEniId",
            "endpointEniAddress",
            "peerAddress",
        ] {
            let from_create = peer_member(case, &created, name);
            let from_describe = peer_member(case, &read, name);
            assert_eq!(
                from_describe, from_create,
                "case {case:?}: expected <{name}> {from_create:?} from the describe, \
                 got {from_describe:?}"
            );
        }
        let read_asn = peer_nested(case, &read, "bgpOptions", "peerAsn");
        assert_eq!(
            read_asn.as_deref(),
            Some(peer_asn),
            "case {case:?}: expected bgpOptions/peerAsn {peer_asn:?} from the describe, \
             got {read_asn:?}"
        );
        let read_liveness = peer_nested(case, &read, "bgpOptions", "peerLivenessDetection");
        assert_eq!(
            read_liveness.as_deref(),
            Some(expected_liveness),
            "case {case:?}: expected bgpOptions/peerLivenessDetection \
             {expected_liveness:?} from the describe, got {read_liveness:?}"
        );
        let read_tags = peer_tags(case, &read);
        assert_eq!(
            read_tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?} from the describe, \
             got {read_tags:?}"
        );
        let read_state = peer_member(case, &read, "state").unwrap_or_default();
        assert!(
            read_state == ec2::state::ROUTE_SERVER_PEER_STATE_PENDING
                || read_state == ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE,
            "case {case:?}: expected pending or available from the describe, got {read_state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// An endpoint id that names nothing
// ---------------------------------------------------------------------------

/// The rule: a create whose `RouteServerEndpointId` resolves to no endpoint of the account
/// is refused with HTTP 400 `InvalidRouteServerEndpointId.NotFound` and stores nothing —
/// and that refusal is decided BEFORE the BGP options are read, so it is the answer even
/// when those options could not have been accepted either.
///
/// The rows differ in the FORM of the id that names nothing — a well-formed long id, a
/// short one, an id of another resource kind, an id of a route server that does exist, a
/// deleted-looking id — and the last two rows pair such an id with options that are
/// themselves invalid, which is the ordering the recorded unit this suite covers asserts.
#[tokio::test]
async fn create_route_server_peer_refuses_an_endpoint_id_that_names_nothing() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;
    let before = all_peer_ids("before", &router).await;

    // (case, the endpoint id, the ASN the request carries, the liveness it carries)
    let cases: Vec<(&str, String, &str, Option<&str>)> = vec![
        (
            "a well-formed long id nothing was created under",
            "rse-00000000000000099".to_string(),
            "65000",
            None,
        ),
        (
            "a short-form id nothing was created under",
            "rse-abcd1234".to_string(),
            "65000",
            None,
        ),
        (
            "a string that is not an id at all",
            "not-an-endpoint".to_string(),
            "65000",
            None,
        ),
        (
            "the id of the route server rather than of its endpoint",
            route_server.clone(),
            "65000",
            None,
        ),
        (
            "the endpoint id with one character changed",
            format!("{}0", &endpoint.id[..endpoint.id.len() - 1]),
            "65000",
            None,
        ),
        (
            "an unknown endpoint and an ASN below the published floor",
            "rse-00000000000000099".to_string(),
            "0",
            None,
        ),
        (
            "an unknown endpoint and an ASN above the published ceiling",
            "rse-00000000000000099".to_string(),
            "4294967296",
            Some(ec2::state::PEER_LIVENESS_BFD),
        ),
    ];

    for (case, endpoint_id, peer_asn, liveness) in cases {
        let (status, body) = call(
            &router,
            "CreateRouteServerPeer",
            &peer_params(&endpoint_id, "10.0.1.5", peer_asn, liveness, &[]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
            "case {case:?}: expected {expected}, got {code:?} with body {body}",
            expected = ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND
        );
    }

    let after = all_peer_ids("after", &router).await;
    assert_eq!(
        after, before,
        "a refused create must store nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// Values outside a published range or form
// ---------------------------------------------------------------------------

/// The rule: on an endpoint that DOES exist, a value outside what its member publishes is
/// refused with `InvalidParameterValue` and stores nothing — the ASN outside "from 1 to
/// 4294967295" (`RouteServerBgpOptionsRequest$PeerAsn`), and a peer address that is not
/// "The IPv4 address of the peer device" (`CreateRouteServerPeerRequest$PeerAddress`).
///
/// The rows walk one step past each published ASN bound and cover four forms of a value
/// that is not an IPv4 address, plus a tag specification naming the wrong resource type.
#[tokio::test]
async fn create_route_server_peer_refuses_a_value_outside_a_published_range_or_form() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;
    let before = all_peer_ids("before", &router).await;

    // (case, the request's form fields, the code it must be refused with)
    let cases: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "one below the published ASN floor",
            peer_params(&endpoint.id, "10.0.1.5", "0", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a negative ASN",
            peer_params(&endpoint.id, "10.0.1.5", "-1", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "one above the published ASN ceiling",
            peer_params(&endpoint.id, "10.0.1.5", "4294967296", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a peer address that is not an address",
            peer_params(&endpoint.id, "the-appliance", "65000", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a peer address with an octet out of range",
            peer_params(&endpoint.id, "10.0.0.256", "65000", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a peer address with too few octets",
            peer_params(&endpoint.id, "10.0.1", "65000", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "an IPv6 peer address",
            peer_params(&endpoint.id, "2001:db8::1", "65000", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a peer address that is a CIDR block rather than an address",
            peer_params(&endpoint.id, "10.0.1.0/24", "65000", None, &[]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag specification naming a resource type this action does not tag",
            {
                let mut params =
                    peer_params(&endpoint.id, "10.0.1.5", "65000", None, &[("role", "edge")]);
                params.retain(|(name, _)| name != "TagSpecification.1.ResourceType");
                params.push(param("TagSpecification.1.ResourceType", "route-server"));
                params
            },
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, params, expected) in cases {
        let (status, body) = call(&router, "CreateRouteServerPeer", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
    }

    let after = all_peer_ids("after", &router).await;
    assert_eq!(
        after, before,
        "a refused create must store nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// The rule: "If you have the required permissions, the error response is DryRunOperation"
/// (`DryRun`) — a dry run of a request that would have succeeded reports that outcome with
/// HTTP 412 and stores nothing; a dry run of one that would have been refused is refused
/// with its own reason instead.
///
/// The rows differ in how much of the request the dry run carries, so the answer cannot
/// depend on one request's shape.
#[tokio::test]
async fn a_dry_run_create_reports_the_outcome_and_stores_nothing() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;
    let before = all_peer_ids("before", &router).await;

    // (case, the request's form fields, the status it must answer, the code it must carry)
    let cases: Vec<(&str, Vec<Param>, u16, &str)> = vec![
        (
            "a dry run of the smallest request that would have succeeded",
            peer_params(&endpoint.id, "10.0.1.5", "65000", None, &[]),
            412,
            "DryRunOperation",
        ),
        (
            "a dry run of a request carrying every member",
            peer_params(
                &endpoint.id,
                "192.168.44.7",
                "4200000000",
                Some(ec2::state::PEER_LIVENESS_BFD),
                &[("role", "firewall")],
            ),
            412,
            "DryRunOperation",
        ),
        (
            "a dry run of a request naming an endpoint that does not exist",
            peer_params("rse-00000000000000099", "10.0.1.5", "65000", None, &[]),
            400,
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
        ),
        (
            "a dry run of a request whose ASN is outside the published range",
            peer_params(&endpoint.id, "10.0.1.5", "0", None, &[]),
            400,
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
    ];

    for (case, mut params, expected_status, expected_code) in cases {
        params.push(param("DryRun", "true"));
        let (status, body) = call(&router, "CreateRouteServerPeer", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
    }

    let after = all_peer_ids("after", &router).await;
    assert_eq!(
        after, before,
        "a dry run must store nothing: expected {before:?}, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// The clock-owed transition
// ---------------------------------------------------------------------------

/// The rule: a create answers the peer in the non-terminal `pending` state, and the clock —
/// not a later call — owes it `available`; a read before the move is due must not bring it
/// forward.
///
/// The rows differ in the BGP options the peer was created with, so the transition is a
/// property of the resource rather than of one request's shape.
#[tokio::test]
async fn a_created_route_server_peer_becomes_available_on_the_clock() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;

    // (case, the request's form fields)
    let cases: Vec<(&str, Vec<Param>)> = vec![
        (
            "the published liveness default",
            peer_params(&endpoint.id, "10.0.1.5", "65000", None, &[]),
        ),
        (
            "BFD liveness",
            peer_params(
                &endpoint.id,
                "10.0.1.6",
                "4200000000",
                Some(ec2::state::PEER_LIVENESS_BFD),
                &[],
            ),
        ),
    ];

    let mut created: Vec<(&str, String)> = Vec::new();
    for (case, params) in cases {
        let body = call_ok(case, &router, "CreateRouteServerPeer", &params).await;
        let peer_id = peer_member(case, &body, "routeServerPeerId")
            .unwrap_or_else(|| panic!("case {case:?}: no routeServerPeerId in {body}"));
        // Immediately after the create the move is not yet due, so a read still reports the
        // transient state a caller acting straight away has to cope with.
        let read = call_ok(
            case,
            &router,
            "DescribeRouteServerPeers",
            &indexed("RouteServerPeerId", &[&peer_id]),
        )
        .await;
        let state = peer_member(case, &read, "state");
        assert_eq!(
            state.as_deref(),
            Some(ec2::state::ROUTE_SERVER_PEER_STATE_PENDING),
            "case {case:?}: expected pending before the transition fell due, got {state:?}"
        );
        created.push((case, peer_id));
    }

    tokio::time::sleep(Duration::from_millis(900)).await;

    for (case, peer_id) in created {
        let read = call_ok(
            case,
            &router,
            "DescribeRouteServerPeers",
            &indexed("RouteServerPeerId", &[&peer_id]),
        )
        .await;
        let state = peer_member(case, &read, "state");
        assert_eq!(
            state.as_deref(),
            Some(ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE),
            "case {case:?}: expected available once the transition fell due, got {state:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The reader
// ---------------------------------------------------------------------------

/// The rule: `DescribeRouteServerPeers` REPORTS THE STORE — every stored peer unless the
/// request narrows it by `RouteServerPeerIds` or by a filter, in `RouteServerPeerId` order,
/// cut into `MaxResults`-sized pages; an id that names no peer of the account is an error
/// rather than an absent row, and an unanswered filter name is refused rather than read as
/// "no results".
///
/// The filter rows cover each name the operation answers — on an id, on an inherited id, on
/// the state, on the peer address, on a BGP option and on a tag by key and by key/value —
/// so the set is the one the operation defines rather than the one a recording happened to
/// use.
#[tokio::test]
async fn describe_route_server_peers_selects_by_id_filter_and_page() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "64513").await;
    let first = create_endpoint("the first endpoint", &router, &first_server, &subnets[0]).await;
    let second = create_endpoint("the second endpoint", &router, &second_server, &subnets[1]).await;

    // Three peers differing in every attribute a filter can name.
    let first_body = call_ok(
        "the first peer",
        &router,
        "CreateRouteServerPeer",
        &peer_params(&first.id, "10.0.1.5", "65000", None, &[("role", "firewall")]),
    )
    .await;
    let second_body = call_ok(
        "the second peer",
        &router,
        "CreateRouteServerPeer",
        &peer_params(
            &first.id,
            "10.0.1.6",
            "65001",
            Some(ec2::state::PEER_LIVENESS_BFD),
            &[("role", "router")],
        ),
    )
    .await;
    let third_body = call_ok(
        "the third peer",
        &router,
        "CreateRouteServerPeer",
        &peer_params(&second.id, "192.168.44.7", "4200000000", None, &[("tier", "edge")]),
    )
    .await;
    let first_peer = peer_member("the first peer", &first_body, "routeServerPeerId")
        .expect("a route server peer id");
    let second_peer = peer_member("the second peer", &second_body, "routeServerPeerId")
        .expect("a route server peer id");
    let third_peer = peer_member("the third peer", &third_body, "routeServerPeerId")
        .expect("a route server peer id");

    // Every peer is `available` once the clock has passed their due instant, so the `state`
    // row below is about a settled store rather than a race.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let all = vec![first_peer.clone(), second_peer.clone(), third_peer.clone()];
    // (case, the request's form fields, the ids it must select, in order)
    let cases: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        ("no selector at all", Vec::new(), all.clone()),
        (
            "one id",
            indexed("RouteServerPeerId", &[&second_peer]),
            vec![second_peer.clone()],
        ),
        (
            "two ids, out of order",
            indexed("RouteServerPeerId", &[&third_peer, &first_peer]),
            vec![first_peer.clone(), third_peer.clone()],
        ),
        (
            "the same id twice",
            indexed("RouteServerPeerId", &[&first_peer, &first_peer]),
            vec![first_peer.clone()],
        ),
        (
            "by peer id filter",
            filters(&[("route-server-peer-id", &[third_peer.as_str()])]),
            vec![third_peer.clone()],
        ),
        (
            "by the endpoint the peer belongs to",
            filters(&[("route-server-endpoint-id", &[first.id.as_str()])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "by the route server the peer belongs to",
            filters(&[("route-server-id", &[second_server.as_str()])]),
            vec![third_peer.clone()],
        ),
        (
            "by subnet",
            filters(&[("subnet-id", &[second.subnet_id.as_str()])]),
            vec![third_peer.clone()],
        ),
        (
            "by VPC",
            filters(&[("vpc-id", &[first.vpc_id.as_str()])]),
            all.clone(),
        ),
        (
            "by state",
            filters(&[(
                "state",
                &[ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE],
            )]),
            all.clone(),
        ),
        (
            "by peer address",
            filters(&[("peer-address", &["10.0.1.6"])]),
            vec![second_peer.clone()],
        ),
        (
            "by the peer's ASN",
            filters(&[("bgp-options.peer-asn", &["4200000000"])]),
            vec![third_peer.clone()],
        ),
        (
            "by the peer's liveness protocol",
            filters(&[(
                "bgp-options.peer-liveness-detection",
                &[ec2::state::PEER_LIVENESS_BFD],
            )]),
            vec![second_peer.clone()],
        ),
        (
            "by a tag key",
            filters(&[("tag-key", &["tier"])]),
            vec![third_peer.clone()],
        ),
        (
            "by a tag key and value",
            filters(&[("tag:role", &["router"])]),
            vec![second_peer.clone()],
        ),
        (
            "two filters, which are joined with an AND",
            filters(&[
                ("route-server-endpoint-id", &[first.id.as_str()]),
                ("tag:role", &["firewall"]),
            ]),
            vec![first_peer.clone()],
        ),
        (
            "a filter nothing matches",
            filters(&[("peer-address", &["10.255.255.254"])]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in cases {
        let body = call_ok(case, &router, "DescribeRouteServerPeers", &params).await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    // An id that names no peer is an error, and an unanswered filter name is refused.
    // (case, the request's form fields, the code it must be refused with)
    let refusals: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "an id nothing was created under",
            indexed("RouteServerPeerId", &["rsp-00000000000000099"]),
            ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
        ),
        (
            "a known id beside an unknown one",
            indexed("RouteServerPeerId", &[&first_peer, "rsp-00000000000000099"]),
            ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
        ),
        (
            "a filter name the operation does not answer",
            filters(&[("peer-asn", &["65000"])]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["router"])]),
            ec2::state::INVALID_PARAMETER_VALUE,
        ),
        (
            "a next-token this service did not mint",
            vec![param("NextToken", "not-a-token")],
            ec2::state::INVALID_PAGINATION_TOKEN,
        ),
    ];
    for (case, params, expected) in refusals {
        let (status, body) = call(&router, "DescribeRouteServerPeers", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected,
            "case {case:?}: expected {expected}, got {code:?} with body {body}"
        );
    }
}

/// The rule: the pages of a `DescribeRouteServerPeers` report partition the selection in
/// `RouteServerPeerId` order, each page carries at most `MaxResults` peers, a token is
/// minted exactly while a peer follows the page, and following the tokens reports every
/// peer exactly once.
///
/// The rows differ in the page size, including the smallest the model's `MaxResults` range
/// publishes and one larger than the whole report.
#[tokio::test]
async fn describe_route_server_peers_pages_in_id_order() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..7u8 {
        let case = "one of seven peers";
        let body = call_ok(
            case,
            &router,
            "CreateRouteServerPeer",
            &peer_params(
                &endpoint.id,
                &format!("10.0.1.{}", index + 10),
                "65000",
                None,
                &[],
            ),
        )
        .await;
        stored.push(
            peer_member(case, &body, "routeServerPeerId").expect("a route server peer id"),
        );
    }
    // The store is an ordered map keyed by the minted id, which is monotonic, so the
    // creation order IS the id order the pages must come in.
    let mut expected = stored.clone();
    expected.sort();
    assert_eq!(
        stored, expected,
        "the minted ids must already be in order, got {stored:?}"
    );

    // (case, the page size, the page sizes the report must come in)
    let cases: [(&str, usize, &[usize]); 3] = [
        ("the smallest published page size", 5, &[5, 2]),
        ("a page size that splits the report evenly", 7, &[7]),
        ("a page size larger than the whole report", 1000, &[7]),
    ];

    for (case, page_size, page_shape) in cases {
        let mut seen: Vec<String> = Vec::new();
        let mut sizes: Vec<usize> = Vec::new();
        let mut next: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = next.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = call_ok(case, &router, "DescribeRouteServerPeers", &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} peers in a page, got {}",
                page.len()
            );
            sizes.push(page.len());
            seen.extend(page);
            next = element(&body, "nextToken");
            if next.is_none() {
                break;
            }
        }
        assert_eq!(
            sizes, page_shape,
            "case {case:?}: expected the pages {page_shape:?}, got {sizes:?}"
        );
        assert_eq!(
            seen, stored,
            "case {case:?}: expected every peer exactly once in id order, got {seen:?}"
        );
    }
}

/// The rule: ONE filter's values are joined with an OR, and each value is matched
/// case-sensitively against the WHOLE attribute with EC2's two wildcards — `*` for zero or
/// more characters, `?` for exactly one, either made literal by a backslash — so a value
/// that is not a wildcard selects only an attribute equal to it.
///
/// The rows differ along what that rule generalises over: how many values a filter carries
/// (one, two, one of them matching nothing), which wildcard and where in the value it sits
/// (leading, trailing, inner, escaped), and which KIND of attribute is named — a stored
/// member, a member inherited from the peer's endpoint network interface, a number, the
/// state, a tag value and the tag keys. The two inherited network-interface filters are
/// reached nowhere else.
#[tokio::test]
async fn describe_route_server_peers_joins_values_with_or_and_matches_wildcards() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let first = create_endpoint("the first endpoint", &router, &route_server, &subnets[0]).await;
    let second = create_endpoint("the second endpoint", &router, &route_server, &subnets[1]).await;
    assert_ne!(
        first.eni_id, second.eni_id,
        "the two endpoints must hold different network interfaces, got {:?} twice",
        first.eni_id
    );

    // Three peers: two sharing the first endpoint's network interface, one on the second.
    let bodies = [
        (
            "the first peer",
            peer_params(&first.id, "10.0.1.5", "65000", None, &[("role", "firewall")]),
        ),
        (
            "the second peer",
            peer_params(
                &first.id,
                "10.0.1.6",
                "65001",
                Some(ec2::state::PEER_LIVENESS_BFD),
                &[("role", "router")],
            ),
        ),
        (
            "the third peer",
            peer_params(&second.id, "192.168.44.7", "4200000000", None, &[("tier", "edge")]),
        ),
    ];
    let mut created: Vec<String> = Vec::new();
    for (case, params) in bodies {
        let body = call_ok(case, &router, "CreateRouteServerPeer", &params).await;
        created.push(peer_member(case, &body, "routeServerPeerId").expect("a peer id"));
    }
    let (first_peer, second_peer, third_peer) =
        (created[0].clone(), created[1].clone(), created[2].clone());

    // The `state` row below is about a settled store rather than a race: every peer is
    // `available` once the clock has passed the due instant its create started.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let all = vec![first_peer.clone(), second_peer.clone(), third_peer.clone()];

    // (case, the request's form fields, the ids it must select, in id order)
    let cases: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        (
            "by the network interface the peer's endpoint holds",
            filters(&[("endpoint-eni-id", &[first.eni_id.as_str()])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "by the address of that network interface",
            filters(&[("endpoint-eni-address", &[first.eni_address.as_str()])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "two values of one filter, which are joined with an OR",
            filters(&[("peer-address", &["10.0.1.5", "192.168.44.7"])]),
            vec![first_peer.clone(), third_peer.clone()],
        ),
        (
            "a value that selects nothing beside one that does",
            filters(&[("peer-address", &["10.255.255.254", "10.0.1.6"])]),
            vec![second_peer.clone()],
        ),
        (
            "both network interfaces as the values of one filter",
            filters(&[(
                "endpoint-eni-id",
                &[first.eni_id.as_str(), second.eni_id.as_str()],
            )]),
            all.clone(),
        ),
        (
            "a trailing multi-character wildcard",
            filters(&[("peer-address", &["10.0.1.*"])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "a single-character wildcard",
            filters(&[("peer-address", &["10.0.1.?"])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "a leading multi-character wildcard",
            filters(&[("peer-address", &["*.44.7"])]),
            vec![third_peer.clone()],
        ),
        (
            "an inner multi-character wildcard",
            filters(&[("peer-address", &["10.*.6"])]),
            vec![second_peer.clone()],
        ),
        (
            "a value that is a prefix of an address but carries no wildcard",
            filters(&[("peer-address", &["10.0.1"])]),
            Vec::new(),
        ),
        (
            "a single-character wildcard where two characters stand",
            filters(&[("peer-address", &["10.0.1?"])]),
            Vec::new(),
        ),
        (
            "an escaped wildcard, which is the literal character",
            filters(&[("peer-address", &[r"10.0.1.\*"])]),
            Vec::new(),
        ),
        (
            "a wildcard on a numeric attribute",
            filters(&[("bgp-options.peer-asn", &["6500?"])]),
            vec![first_peer.clone(), second_peer.clone()],
        ),
        (
            "a wildcard on the state",
            filters(&[("state", &["avail*"])]),
            all.clone(),
        ),
        (
            "a wildcard on every minted peer id",
            filters(&[("route-server-peer-id", &["rsp-*"])]),
            all.clone(),
        ),
        (
            "a wildcard on a tag's value",
            filters(&[("tag:role", &["r*"])]),
            vec![second_peer.clone()],
        ),
        (
            "a wildcard on the tag keys a peer carries",
            filters(&[("tag-key", &["t*"])]),
            vec![third_peer.clone()],
        ),
        (
            "a wildcard matched case-sensitively",
            filters(&[("tag:role", &["R*"])]),
            Vec::new(),
        ),
        (
            "a wildcard nothing matches",
            filters(&[("peer-address", &["172.*"])]),
            Vec::new(),
        ),
    ];

    for (case, params, expected) in cases {
        let body = call_ok(case, &router, "DescribeRouteServerPeers", &params).await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The rule: a `NextToken` this service did not mint is refused with
/// `InvalidPaginationToken` and EC2's own sentence for it, while a token it DID mint
/// resumes the report; and a `DryRun` probe from a caller holding the action's permissions
/// is answered `DryRunOperation` (HTTP 412) and reports nothing, whatever else the request
/// selects, where `DryRun=false` is an ordinary call.
///
/// The token rows differ in HOW the token fails to be one of this service's — not hex at
/// all, hex carrying nothing it minted, and one of its own with bytes appended — and the
/// dry-run rows in what the probe asks for beside the flag.
#[tokio::test]
async fn describe_route_server_peers_refuses_a_foreign_token_and_performs_no_dry_run() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;

    // Six peers: one more than the smallest page the model's `MaxResults` range publishes,
    // so the first page of that size is followed by a token this service really minted.
    let mut stored: Vec<String> = Vec::new();
    for index in 0..6u8 {
        let case = "one of six peers";
        let body = call_ok(
            case,
            &router,
            "CreateRouteServerPeer",
            &peer_params(
                &endpoint.id,
                &format!("10.0.2.{}", index + 10),
                "65000",
                None,
                &[],
            ),
        )
        .await;
        stored.push(peer_member(case, &body, "routeServerPeerId").expect("a peer id"));
    }

    let body = call_ok(
        "the first page",
        &router,
        "DescribeRouteServerPeers",
        &[param("MaxResults", "5")],
    )
    .await;
    let minted = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("a token while a peer follows the page, got {body}"));

    // A token this service minted resumes the report: the refusals below are about a
    // token's provenance, not about paging.
    let body = call_ok(
        "the token this service minted",
        &router,
        "DescribeRouteServerPeers",
        &[param("MaxResults", "5"), param("NextToken", &minted)],
    )
    .await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        stored[5..].to_vec(),
        "case \"the token this service minted\": expected the peers after the first page, \
         got {reported:?}"
    );

    let tampered = format!("{minted}00");
    // (case, the token the request sends)
    let refusals: [(&str, &str); 3] = [
        ("a token that is not hex at all", "not-a-token"),
        ("hex carrying nothing this service minted", "deadbeef"),
        ("a minted token with bytes appended", tampered.as_str()),
    ];
    for (case, token) in refusals {
        let (status, body) = call(
            &router,
            "DescribeRouteServerPeers",
            &[param("NextToken", token)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_PAGINATION_TOKEN,
            "case {case:?}: expected {}, got {code:?} with body {body}",
            ec2::state::INVALID_PAGINATION_TOKEN
        );
        let message = error_message(&body);
        let expected = ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE;
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // (case, the form fields of the probe)
    let mut filtered = vec![param("DryRun", "true")];
    filtered.extend(filters(&[("peer-address", &["10.0.2.10"])]));
    let mut by_id = vec![param("DryRun", "true")];
    by_id.extend(indexed("RouteServerPeerId", &[stored[0].as_str()]));
    let probes: Vec<(&str, Vec<Param>)> = vec![
        ("a dry run with nothing else", vec![param("DryRun", "true")]),
        (
            "a dry run beside a page size",
            vec![param("DryRun", "true"), param("MaxResults", "5")],
        ),
        ("a dry run beside a filter", filtered),
        ("a dry run beside an id that names a peer", by_id),
    ];
    for (case, params) in probes {
        let (status, body) = call(&router, "DescribeRouteServerPeers", &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::DRY_RUN_OPERATION,
            "case {case:?}: expected {}, got {code:?} with body {body}",
            ec2::state::DRY_RUN_OPERATION
        );
        let reported = reported_ids(&body);
        assert!(
            reported.is_empty(),
            "case {case:?}: a dry run reports no peer, got {reported:?}"
        );
    }

    // `DryRun=false` is not a probe: the report is served.
    let body = call_ok(
        "DryRun=false",
        &router,
        "DescribeRouteServerPeers",
        &[param("DryRun", "false")],
    )
    .await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported, stored,
        "case \"DryRun=false\": expected every peer, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// The delete
// ---------------------------------------------------------------------------

/// One row of the delete table: a peer to create and the state it must be in when the
/// delete arrives.
struct DeleteCase<'a> {
    case: &'a str,
    endpoint: &'a Endpoint,
    peer_address: &'a str,
    peer_asn: &'a str,
    liveness: Option<&'a str>,
    tags: Vec<(&'a str, &'a str)>,
    /// Whether to wait for the create's own clock-owed move before deleting, so the delete
    /// is exercised both on a peer still coming up and on one already up.
    wait_for_available: bool,
}

/// `DeleteRouteServerPeer` on `peer_id`, as its HTTP status and XML body.
async fn delete_peer(router: &MultiServiceRouter, peer_id: &str) -> (u16, String) {
    call(
        router,
        "DeleteRouteServerPeer",
        &[param("RouteServerPeerId", peer_id)],
    )
    .await
}

/// The state `DescribeRouteServerPeers` reports for `peer_id`.
async fn described_state(case: &str, router: &MultiServiceRouter, peer_id: &str) -> String {
    let body = call_ok(
        case,
        router,
        "DescribeRouteServerPeers",
        &indexed("RouteServerPeerId", &[peer_id]),
    )
    .await;
    peer_member(case, &body, "state")
        .unwrap_or_else(|| panic!("case {case:?}: no <state> in {body}"))
}

/// The rule: `DeleteRouteServerPeer` takes the stored peer the request names down — it
/// answers that peer as the store then holds it, in the non-terminal `deleting` state, with
/// every other member the one the create stored, and the CLOCK (not a later call) owes it
/// the terminal `deleted` one transition span later.
///
/// The rows differ along what the rule generalises over: the state the peer is in when the
/// delete arrives (still `pending`, so the `available` move it was owed must not happen
/// after the delete, and `available`), the BGP options and tag count it carries, and which
/// endpoint — and so which route server, VPC and subnet — it belongs to.
#[tokio::test]
async fn delete_route_server_peer_puts_the_peer_into_deleting_and_the_clock_owes_it_deleted() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    assert!(
        subnets.len() >= 2,
        "the account must hold at least two subnets to place two endpoints into, got {subnets:?}"
    );
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;
    let first = create_endpoint("the first endpoint", &router, &first_server, &subnets[0]).await;
    let second = create_endpoint("the second endpoint", &router, &second_server, &subnets[1]).await;

    let cases: Vec<DeleteCase<'_>> = vec![
        DeleteCase {
            case: "a peer deleted while it is still coming up",
            endpoint: &first,
            peer_address: "10.0.3.11",
            peer_asn: "1",
            liveness: None,
            tags: Vec::new(),
            wait_for_available: false,
        },
        DeleteCase {
            case: "a tagged BFD peer deleted once it is available",
            endpoint: &first,
            peer_address: "172.16.0.201",
            peer_asn: "65000",
            liveness: Some(ec2::state::PEER_LIVENESS_BFD),
            tags: vec![("role", "firewall"), ("tier", "edge")],
            wait_for_available: true,
        },
        DeleteCase {
            case: "a peer of the second endpoint deleted once it is available",
            endpoint: &second,
            peer_address: "203.0.113.19",
            peer_asn: "4294967295",
            liveness: Some(ec2::state::PEER_LIVENESS_BGP_KEEPALIVE),
            tags: vec![("role", "appliance")],
            wait_for_available: true,
        },
    ];

    let mut deleted: Vec<(&str, String)> = Vec::new();
    for DeleteCase {
        case,
        endpoint,
        peer_address,
        peer_asn,
        liveness,
        tags,
        wait_for_available,
    } in cases
    {
        let created = call_ok(
            case,
            &router,
            "CreateRouteServerPeer",
            &peer_params(&endpoint.id, peer_address, peer_asn, liveness, &tags),
        )
        .await;
        let peer_id = peer_member(case, &created, "routeServerPeerId")
            .unwrap_or_else(|| panic!("case {case:?}: no routeServerPeerId in {created}"));

        if wait_for_available {
            tokio::time::sleep(Duration::from_millis(900)).await;
        }
        let before = described_state(case, &router, &peer_id).await;
        if wait_for_available {
            assert_eq!(
                before,
                ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE,
                "case {case:?}: expected the peer to be available before the delete, \
                 got {before:?}"
            );
        } else {
            // The create's own move may already have fallen due on a loaded machine, so
            // what this row fixes is that the peer is still COMING UP OR UP when the
            // delete arrives — the delete's own answer is asserted below either way.
            assert!(
                before == ec2::state::ROUTE_SERVER_PEER_STATE_PENDING
                    || before == ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE,
                "case {case:?}: expected the peer to be coming up or up before the delete, \
                 got {before:?}"
            );
        }

        let (status, body) = delete_peer(&router, &peer_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
        );
        let answered = peer_member(case, &body, "state");
        assert_eq!(
            answered.as_deref(),
            Some(ec2::state::ROUTE_SERVER_PEER_STATE_DELETING),
            "case {case:?}: expected the delete to answer the deleting state, got {answered:?}"
        );
        // The answer is the STORED peer: every member but the state is the one the create
        // stored, whatever the delete did to it.
        for name in [
            "routeServerPeerId",
            "routeServerEndpointId",
            "routeServerId",
            "vpcId",
            "subnetId",
            "endpointEniId",
            "endpointEniAddress",
            "peerAddress",
        ] {
            let from_create = peer_member(case, &created, name);
            let from_delete = peer_member(case, &body, name);
            assert_eq!(
                from_delete, from_create,
                "case {case:?}: expected <{name}> {from_create:?} from the delete, \
                 got {from_delete:?}"
            );
        }
        for member in ["peerAsn", "peerLivenessDetection"] {
            let from_create = peer_nested(case, &created, "bgpOptions", member);
            let from_delete = peer_nested(case, &body, "bgpOptions", member);
            assert_eq!(
                from_delete, from_create,
                "case {case:?}: expected bgpOptions/{member} {from_create:?} from the \
                 delete, got {from_delete:?}"
            );
        }
        let created_tags = peer_tags(case, &created);
        let delete_tags = peer_tags(case, &body);
        assert_eq!(
            delete_tags, created_tags,
            "case {case:?}: expected the tags {created_tags:?} from the delete, \
             got {delete_tags:?}"
        );

        // Straight after the call the terminal move is not yet due, so the reader still
        // reports the transient state a caller acting immediately has to cope with.
        let reading = described_state(case, &router, &peer_id).await;
        assert_eq!(
            reading, ec2::state::ROUTE_SERVER_PEER_STATE_DELETING,
            "case {case:?}: expected deleting before the terminal move fell due, \
             got {reading:?}"
        );
        deleted.push((case, peer_id));
    }

    // The clock owes every deleted peer its terminal state, including the one deleted while
    // it was still coming up: that peer's `available` move is off, not merely late.
    tokio::time::sleep(Duration::from_millis(900)).await;
    for (case, peer_id) in &deleted {
        let state = described_state(case, &router, peer_id).await;
        assert_eq!(
            state,
            ec2::state::ROUTE_SERVER_PEER_STATE_DELETED,
            "case {case:?}: expected deleted once the terminal move fell due, got {state:?}"
        );
    }

    // Every peer is still reported, so the delete moved the records rather than losing them.
    let reported = all_peer_ids("after the deletes", &router).await;
    let mut expected: Vec<String> = deleted.iter().map(|(_, id)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected every deleted peer to still be reported, got {reported:?}"
    );
}

/// The rule: a `DeleteRouteServerPeer` whose `RouteServerPeerId` resolves to no peer of the
/// account is refused with HTTP 400 `InvalidRouteServerPeerId.NotFound`, naming the id the
/// request sent, and deletes nothing.
///
/// The rows differ in the FORM of the id that names nothing — a well-formed long id, a
/// short one, a string that is no id at all, the id of another kind of resource, and a
/// stored peer's id with one character changed.
#[tokio::test]
async fn delete_route_server_peer_refuses_an_id_that_names_nothing() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;
    let created = call_ok(
        "a peer that must survive every refusal",
        &router,
        "CreateRouteServerPeer",
        &peer_params(&endpoint.id, "10.0.4.5", "65000", None, &[]),
    )
    .await;
    let peer_id = peer_member("a peer that must survive every refusal", &created, "routeServerPeerId")
        .expect("a route server peer id");

    // (case, the id the request names)
    let cases: Vec<(&str, String)> = vec![
        (
            "a well-formed long id nothing was created under",
            "rsp-00000000000000000".to_string(),
        ),
        (
            "another well-formed long id nothing was created under",
            "rsp-00000000000000099".to_string(),
        ),
        (
            "a short-form id nothing was created under",
            "rsp-abcd1234".to_string(),
        ),
        ("a string that is not an id at all", "not-a-peer".to_string()),
        (
            "the id of the endpoint rather than of its peer",
            endpoint.id.clone(),
        ),
        (
            "the id of the route server rather than of its peer",
            route_server.clone(),
        ),
        (
            "the stored peer's id with one character changed",
            format!("{}0", &peer_id[..peer_id.len() - 1]),
        ),
    ];

    for (case, id) in cases {
        let (status, body) = delete_peer(&router, &id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
            "case {case:?}: expected {expected}, got {code:?} with body {body}",
            expected = ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND
        );
        let message = error_message(&body);
        let expected = format!(
            "The {kind} ID '{id}' does not exist",
            kind = ec2::state::RESOURCE_TYPE_ROUTE_SERVER_PEER,
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // Nothing was deleted: the one stored peer is still in the state its create left it in.
    let state = described_state("after the refusals", &router, &peer_id).await;
    assert!(
        state == ec2::state::ROUTE_SERVER_PEER_STATE_PENDING
            || state == ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE,
        "expected the stored peer to be untouched by a refused delete, got {state:?}"
    );
}

/// The rule: a peer whose deletion has already been asked for has nothing left to delete,
/// so a second `DeleteRouteServerPeer` is refused with HTTP 400 `IncorrectState` naming the
/// state the peer is in — in either delete state — and "If you have the required
/// permissions, the error response is DryRunOperation" (`DryRun`): a dry run of a delete
/// that would have succeeded reports that outcome with HTTP 412 and deletes nothing, while a
/// dry run of one that would have been refused is refused with its own reason instead.
#[tokio::test]
async fn delete_route_server_peer_refuses_a_second_delete_and_performs_no_dry_run() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let endpoint = create_endpoint("an endpoint", &router, &route_server, &subnets[0]).await;

    // Two peers: one the dry runs probe and then delete, one deleted outright so the second
    // delete is exercised in both delete states.
    let mut peers: Vec<String> = Vec::new();
    for (index, case) in ["the probed peer", "the deleted peer"].iter().enumerate() {
        let body = call_ok(
            case,
            &router,
            "CreateRouteServerPeer",
            &peer_params(
                &endpoint.id,
                &format!("10.0.5.{}", index + 10),
                "65000",
                None,
                &[],
            ),
        )
        .await;
        peers.push(peer_member(case, &body, "routeServerPeerId").expect("a peer id"));
    }
    let (probed, doomed) = (peers[0].clone(), peers[1].clone());

    // A dry run of a delete that would have succeeded: the outcome, and no deletion.
    // (case, the id the probe names, the status it must answer, the code it must carry)
    let probes: Vec<(&str, String, u16, &str)> = vec![
        (
            "a dry run of a delete that would have succeeded",
            probed.clone(),
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run naming an id that resolves to no peer",
            "rsp-00000000000000000".to_string(),
            400,
            ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
        ),
    ];
    for (case, id, expected_status, expected_code) in probes {
        let (status, body) = call(
            &router,
            "DeleteRouteServerPeer",
            &[param("RouteServerPeerId", &id), param("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
    }
    let probed_state = described_state("the probed peer", &router, &probed).await;
    assert!(
        probed_state == ec2::state::ROUTE_SERVER_PEER_STATE_PENDING
            || probed_state == ec2::state::ROUTE_SERVER_PEER_STATE_AVAILABLE,
        "a dry run must delete nothing: expected the probed peer to be coming up or up, \
         got {probed_state:?}"
    );

    let (status, body) = delete_peer(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the first delete, got {status} with body {body}"
    );

    // A second delete, while the first is still underway and once it has finished, and a dry
    // run of one: each is refused for the state the peer is in.
    // (case, whether to wait for the terminal state first, whether the probe is a dry run)
    let refusals: [(&str, bool, bool); 3] = [
        ("a second delete while the first is still underway", false, false),
        ("a dry run of a second delete, which is refused first", false, true),
        ("a second delete once the first has finished", true, false),
    ];
    for (case, wait, dry_run) in refusals {
        if wait {
            // Wait for the CONDITION the clock owes — the peer reported `deleted` — by
            // looking again until it holds or a deadline far past the service's own
            // transition span passes, so nothing here depends on how fast the host runs.
            let deadline = std::time::Instant::now() + ec2::state::TRANSITION_DELAY * 40;
            loop {
                let state = described_state(case, &router, &doomed).await;
                if state == ec2::state::ROUTE_SERVER_PEER_STATE_DELETED {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "case {case:?}: expected the clock to report the peer \
                     {deleted:?}, it was still {state:?}",
                    deleted = ec2::state::ROUTE_SERVER_PEER_STATE_DELETED
                );
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        }
        let mut params = vec![param("RouteServerPeerId", &doomed)];
        if dry_run {
            params.push(param("DryRun", "true"));
        }
        let (status, body) = call(&router, "DeleteRouteServerPeer", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code,
            ec2::state::INCORRECT_STATE,
            "case {case:?}: expected {expected}, got {code:?} with body {body}",
            expected = ec2::state::INCORRECT_STATE
        );
        // The refusal is AWS's wording for this resource: the peer named, and that its state
        // does not admit the request — the same sentence whichever state it had reached, so
        // the row that waited for `deleted` and the rows that did not expect one message.
        let message = error_message(&body);
        let expected = format!("Route Server Peer {doomed} is in an invalid state.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}
