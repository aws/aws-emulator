// SPDX-License-Identifier: CC0-1.0

//! The WORDING of the route server surface's `IncorrectState` refusal, driven over the wire
//! the service actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST
//! carrying `Action=<operation>`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol").
//!
//! The rule under test: a request aimed at a resource of this surface whose own state does
//! not admit it is refused HTTP 400 `IncorrectState` with the sentence EC2 words for that
//! class — the kind of resource in EC2's own words, then the id the request carried, then
//! that its state is not a valid one — and nothing else: the template has no slot for the
//! state the resource is in, so the SAME sentence answers the refusal whichever state of its
//! delete the resource had reached. An id that resolves to no resource of the account is a
//! different class, `Invalid<Resource>Id.NotFound`, so the split between "there is no such
//! resource" and "that resource cannot take this call" survives.
//!
//! Every row is a distinct input along what the rule generalises over: which kind of resource
//! the request named (a route server, an endpoint of one, a peer of an endpoint — each
//! modelled with its own delete and its own state enum), and which of the two delete states
//! that resource had reached when the second delete arrived.
//!
//! The sentences are spelled out here as literals rather than read from `ec2::state`: a suite
//! that imported the service's own constant would pass by agreeing with itself.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for. The default VPC's subnets — the subnets an
/// endpoint of a route server is placed into below — are seeded for the emulator's default
/// Region.
const REGION: &str = "us-east-1";

/// EC2's code for a request whose resource exists but is not in a state that admits it.
const INCORRECT_STATE: &str = "IncorrectState";

/// How long a poll for a clock-owed state may look before it gives up: generous, so a loaded
/// host cannot turn "not yet" into a failure, and never a bound on how fast the move has to
/// happen.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

/// How long a poll waits between two looks at the same resource.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn call_ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One element of a response body, or a panic naming the case.
fn reported(case: &str, body: &str, name: &str) -> String {
    element(body, name).unwrap_or_else(|| panic!("case {case:?}: no <{name}> in {body}"))
}

/// One kind of resource of this surface: how EC2 names it in a refusal, the operation that
/// takes one down, the member that names one, the describe that reports it, and the terminal
/// state of its delete.
struct Family {
    /// How EC2 names this kind of resource in the sentence of the refusal.
    resource: &'static str,
    /// The operation that takes one down — the one a second call of is refused.
    delete: &'static str,
    /// The request member that names one, which the delete and the describe share.
    id_member: &'static str,
    /// The describe that reports the state the resource has reached.
    describe: &'static str,
    /// The terminal state the clock owes a resource this delete took down.
    deleted_state: &'static str,
    /// The code an id resolving to no resource of this kind is refused with.
    not_found_code: &'static str,
}

/// Look at `id` until its describe reports `wanted`, answering as soon as it does. The clock,
/// not a caller, moves a resource whose deletion is underway to its terminal state, so a test
/// that needs that state waits for the CONDITION rather than for a span.
async fn poll_until_state(case: &str, router: &MultiServiceRouter, family: &Family, id: &str, wanted: &str) {
    let deadline = tokio::time::Instant::now() + POLL_DEADLINE;
    loop {
        let body = call_ok(case, router, family.describe, &indexed(family.id_member, &[id])).await;
        let state = reported(case, &body, "state");
        if state == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected {id} to reach {wanted:?}, last read {state:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Pin that `family`'s delete naming `id` is refused HTTP 400 `IncorrectState` carrying
/// exactly the sentence the rule words for that kind of resource.
async fn refused_for_its_state(case: &str, router: &MultiServiceRouter, family: &Family, id: &str) {
    let (status, body) = call(router, family.delete, &[param(family.id_member, id)]).await;
    assert_eq!(
        status, 400,
        "case {case:?}: expected HTTP 400 from {delete}, got {status} with body {body}",
        delete = family.delete
    );
    let code = error_code(&body);
    assert_eq!(
        code, INCORRECT_STATE,
        "case {case:?}: expected the code {INCORRECT_STATE:?}, got {code:?} in {body}"
    );
    let message = error_message(&body);
    let expected = format!(
        "{resource} {id} is in an invalid state.",
        resource = family.resource
    );
    assert_eq!(
        message, expected,
        "case {case:?}: expected the message {expected:?}, got {message:?}"
    );
}

/// An id of the same form as `id` that names no resource of the account: the last character
/// of a minted id, changed.
fn unknown_like(id: &str) -> String {
    let (head, last) = id.split_at(id.len() - 1);
    let replacement = if last == "f" { "0" } else { "f" };
    format!("{head}{replacement}")
}

/// The subnet ids the account holds, read over the wire rather than out of the store.
async fn subnet_ids(router: &MultiServiceRouter) -> Vec<String> {
    let body = call_ok("the account's subnets", router, "DescribeSubnets", &[]).await;
    elements(&body, "subnetId")
}

/// RULE: one sentence per kind of resource, with the id the request carried and no state
/// token — so a second delete of a resource whose deletion is underway and a second delete of
/// one whose deletion has finished are refused with the same message, and an id that names
/// nothing is refused with a different CLASS.
///
/// The rows are the three kinds of resource this surface models, each driven in both of its
/// delete states.
#[tokio::test]
async fn the_route_server_surface_words_one_incorrect_state_sentence_per_resource() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;

    let route_server = Family {
        resource: "Route Server",
        delete: "DeleteRouteServer",
        id_member: "RouteServerId",
        describe: "DescribeRouteServers",
        deleted_state: ec2::state::ROUTE_SERVER_STATE_DELETED,
        not_found_code: ec2::state::INVALID_ROUTE_SERVER_ID_NOT_FOUND,
    };
    let endpoint = Family {
        resource: "Route Server Endpoint",
        delete: "DeleteRouteServerEndpoint",
        id_member: "RouteServerEndpointId",
        describe: "DescribeRouteServerEndpoints",
        deleted_state: ec2::state::ROUTE_SERVER_ENDPOINT_STATE_DELETED,
        not_found_code: ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
    };
    let peer = Family {
        resource: "Route Server Peer",
        delete: "DeleteRouteServerPeer",
        id_member: "RouteServerPeerId",
        describe: "DescribeRouteServerPeers",
        deleted_state: ec2::state::ROUTE_SERVER_PEER_STATE_DELETED,
        not_found_code: ec2::state::INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
    };

    // A route server of its own per row, so a delete that cascades to the resources OF a
    // route server cannot take another row's resource down with it.
    let doomed_server = create_route_server("a route server to delete", &router, "64512").await;
    let endpoint_server = create_route_server("the endpoint's route server", &router, "64513").await;
    let peer_server = create_route_server("the peer's route server", &router, "64514").await;

    let doomed_endpoint = create_endpoint(
        "the endpoint to delete",
        &router,
        &endpoint_server,
        &subnets[0],
    )
    .await;
    let peer_endpoint = create_endpoint(
        "the peer's endpoint",
        &router,
        &peer_server,
        &subnets[subnets.len() - 1],
    )
    .await;
    let doomed_peer = create_peer("the peer to delete", &router, &peer_endpoint, "10.0.1.9").await;

    // (the kind of resource, the id of the one this row deletes)
    let rows: Vec<(&Family, String)> = vec![
        (&route_server, doomed_server),
        (&endpoint, doomed_endpoint),
        (&peer, doomed_peer),
    ];

    for (family, id) in rows {
        let resource = family.resource;

        // An id of the right form that names no resource of the account is the OTHER class,
        // whatever state the account's own resources are in.
        let unknown = unknown_like(&id);
        let (status, body) = call(&router, family.delete, &[param(family.id_member, &unknown)]).await;
        let case = format!("an id no {resource} of the account has");
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, family.not_found_code,
            "case {case:?}: expected the code {expected:?}, got {code:?} in {body}",
            expected = family.not_found_code
        );

        // The first delete succeeds and leaves the resource in a non-terminal delete state.
        let case = format!("the first delete of a {resource}");
        call_ok(&case, &router, family.delete, &[param(family.id_member, &id)]).await;

        let case = format!("a second delete of a {resource} whose deletion is underway");
        refused_for_its_state(&case, &router, family, &id).await;

        // The clock, not a caller, carries the resource to its terminal state — and the
        // refusal there is worded exactly as the one above, because the template names no
        // state.
        poll_until_state(&case, &router, family, &id, family.deleted_state).await;
        let case = format!("a second delete of a {resource} whose deletion has finished");
        refused_for_its_state(&case, &router, family, &id).await;
    }
}

/// A route server with `amazon_side_asn`, and its id.
async fn create_route_server(case: &str, router: &MultiServiceRouter, amazon_side_asn: &str) -> String {
    let body = call_ok(
        case,
        router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", amazon_side_asn)],
    )
    .await;
    reported(case, &body, "routeServerId")
}

/// An endpoint of `route_server_id` in `subnet_id`, and its id.
async fn create_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
    subnet_id: &str,
) -> String {
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
    reported(case, &body, "routeServerEndpointId")
}

/// A peer of `endpoint_id` at `peer_address`, and its id.
async fn create_peer(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
    peer_address: &str,
) -> String {
    let body = call_ok(
        case,
        router,
        "CreateRouteServerPeer",
        &[
            param("RouteServerEndpointId", endpoint_id),
            param("PeerAddress", peer_address),
            param("BgpOptions.PeerAsn", "65001"),
        ],
    )
    .await;
    reported(case, &body, "routeServerPeerId")
}
