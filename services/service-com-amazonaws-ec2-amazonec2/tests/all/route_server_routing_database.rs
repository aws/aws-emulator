// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetRouteServerRoutingDatabase`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. the report is OF ONE ROUTE SERVER'S learned routes: an entry of the database is a
//!    prefix one of that route server's peers advertised over an ESTABLISHED BGP session, so
//!    a peer whose session is not up contributes none — and no session of this emulator is
//!    established, which the peers' own `bgpStatus` is asserted for in the same rows, so
//!    every route server reports a database with no entry and no `NextToken`, however many
//!    peers it holds, whichever liveness protocol they use, and whether or not the clock has
//!    settled them;
//! 2. `AreRoutesPersisted` is DERIVED from the stored route server's persistence
//!    configuration as of the read — true exactly while `persistRoutesState` is the settled
//!    `enabled`, which `DescribeRouteServers` reports for the same route server — so the two
//!    reads cannot disagree, and the clock, not a caller, moves a persistence state on;
//! 3. a request this operation cannot serve is REFUSED rather than answered with an empty
//!    database: a `RouteServerId` that is absent, malformed or names no route server of the
//!    account, a filter name the operation does not answer, and a `NextToken` this service
//!    did not mint for THIS route server — each with the code and the wording EC2 answers it
//!    with, and all of them ahead of a `DryRun`, which is answered only for a request that
//!    would otherwise have succeeded;
//! 4. the report WRITES NOTHING: two reads answer the same database, and neither the route
//!    server nor its peers move because they were read.
//!
//! Each table row is a distinct input along what those rules generalise over — the number of
//! peers a route server holds, each published liveness protocol, each persistence action and
//! the states before and after the clock settles them, the FORM of an id that names nothing
//! (absent, each malformed shape, a well-formed id at both published lengths), the page size
//! at both published bounds, and the form of a token the service did not mint for the route
//! server being read — so no one row is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters};

/// The Region every call here is signed for. The default VPC's subnets — the subnets a route
/// server endpoint is placed into below — are seeded for the emulator's default Region.
const REGION: &str = "us-east-1";

/// A `(key, value)` request parameter, as the ec2Query harness takes them.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters.
type Params = Vec<(String, String)>;

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(ec2::state::TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One call of `action` carrying `params`, as its HTTP status and XML body.
async fn action(router: &MultiServiceRouter, name: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, name, params).await
}

/// One call that must succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, name: &str, params: &[(String, String)]) -> String {
    let (status, body) = action(router, name, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {name}, got {status} with body {body}"
    );
    body
}

/// A route server with `extra` beside its ASN, and the id it was minted with.
async fn create_route_server(case: &str, router: &MultiServiceRouter, asn: i64, extra: Params) -> String {
    let mut params = vec![p("AmazonSideAsn", asn.to_string())];
    params.extend(extra);
    let body = ok(case, router, "CreateRouteServer", &params).await;
    element(&body, "routeServerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"))
}

/// The subnet ids the account holds, read over the wire rather than out of the store.
async fn subnet_ids(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(case, router, "DescribeSubnets", &[]).await;
    elements(&body, "subnetId")
}

/// An endpoint of `route_server_id` in `subnet_id`, and its id.
async fn create_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
    subnet_id: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateRouteServerEndpoint",
        &[
            p("RouteServerId", route_server_id),
            p("SubnetId", subnet_id),
        ],
    )
    .await;
    element(&body, "routeServerEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerEndpointId in {body}"))
}

/// A peer of `endpoint_id` at `peer_address`, with `liveness` when the row names one, and
/// the id it was minted with.
async fn create_peer(
    case: &str,
    router: &MultiServiceRouter,
    endpoint_id: &str,
    peer_address: &str,
    peer_asn: i64,
    liveness: Option<&str>,
) -> String {
    let mut params = vec![
        p("RouteServerEndpointId", endpoint_id),
        p("PeerAddress", peer_address),
        p("BgpOptions.PeerAsn", peer_asn.to_string()),
    ];
    if let Some(liveness) = liveness {
        params.push(p("BgpOptions.PeerLivenessDetection", liveness));
    }
    let body = ok(case, router, "CreateRouteServerPeer", &params).await;
    element(&body, "routeServerPeerId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerPeerId in {body}"))
}

/// The routing database `route_server_id` reports, with `extra` request members.
async fn database(
    router: &MultiServiceRouter,
    route_server_id: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("RouteServerId", route_server_id)];
    params.extend(extra.iter().cloned());
    action(router, "GetRouteServerRoutingDatabase", &params).await
}

/// The database `route_server_id` reports, requiring the read to succeed.
async fn database_ok(case: &str, router: &MultiServiceRouter, route_server_id: &str) -> String {
    let (status, body) = database(router, route_server_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetRouteServerRoutingDatabase, got {status} \
         with body {body}"
    );
    body
}

/// The prefixes a reported database carries, in document order — one per entry.
fn reported_prefixes(body: &str) -> Vec<String> {
    elements(body, "prefix")
}

/// The peers a reported database attributes its entries to, in document order.
fn reported_peers(body: &str) -> Vec<String> {
    elements(body, "routeServerPeerId")
}

/// The BGP session status `DescribeRouteServerPeers` reports for every peer of the account,
/// in document order.
async fn reported_session_statuses(case: &str, router: &MultiServiceRouter) -> Vec<String> {
    let body = ok(case, router, "DescribeRouteServerPeers", &[]).await;
    elements(&body, "status")
}

/// The `persistRoutesState` `DescribeRouteServers` reports for `route_server_id`.
async fn described_persistence(case: &str, router: &MultiServiceRouter, route_server_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeRouteServers",
        &[p("RouteServerId.1", route_server_id)],
    )
    .await;
    element(&body, "persistRoutesState")
        .unwrap_or_else(|| panic!("case {case:?}: no persistRoutesState in {body}"))
}

// ---------------------------------------------------------------------------
// Rule 1: the database is what the route server's sessions carried
// ---------------------------------------------------------------------------

/// One row of the peers table: the case, how many peers the route server holds and the
/// liveness protocol each was created with (`None` for a request that named none, which the
/// create leaves at its published default).
type PeersRow = (&'static str, Vec<Option<&'static str>>);

/// RULE 1: `GetRouteServerRoutingDatabase` reports the routes the route server's peers
/// advertised over an ESTABLISHED BGP session. No session of this emulator is established —
/// asserted here through each peer's own `bgpStatus`, which is the state the database is
/// derived from — so the report carries no entry and no token, for every peer count and
/// liveness protocol, both while the peers are still coming up and after the clock has
/// settled them.
#[tokio::test]
async fn the_database_reports_the_routes_the_route_servers_sessions_carried() {
    let rows: Vec<PeersRow> = vec![
        ("a route server with no peer at all", Vec::new()),
        (
            "one peer whose create named no liveness protocol",
            vec![None],
        ),
        ("one peer detecting liveness with BGP keepalives", vec![Some("bgp-keepalive")]),
        ("one peer detecting liveness with BFD", vec![Some("bfd")]),
        (
            "two peers, one per published liveness protocol",
            vec![Some("bgp-keepalive"), Some("bfd")],
        ),
        (
            "three peers on one endpoint",
            vec![None, Some("bfd"), Some("bgp-keepalive")],
        ),
    ];

    for (case, peers) in rows {
        let router = emulator();
        let subnets = subnet_ids(case, &router).await;
        let subnet = subnets
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the account holds no subnet to place an endpoint into"));
        let route_server_id = create_route_server(case, &router, 64_512, Vec::new()).await;
        let mut peer_ids = Vec::new();
        if !peers.is_empty() {
            let endpoint_id = create_endpoint(case, &router, &route_server_id, subnet).await;
            for (index, liveness) in peers.iter().enumerate() {
                let address = format!("192.0.2.{}", index + 10);
                peer_ids.push(
                    create_peer(
                        case,
                        &router,
                        &endpoint_id,
                        &address,
                        64_600 + index as i64,
                        *liveness,
                    )
                    .await,
                );
            }
        }

        // The PREMISE the database is derived from: nothing in this emulator speaks BGP to a
        // peer address, so no peer reports a session that is up. A session reported `up`
        // would have to carry the routes it learned, and the assertions below would then be
        // wrong rather than merely unmet.
        let statuses = reported_session_statuses(case, &router).await;
        assert!(
            statuses.len() >= peer_ids.len(),
            "case {case:?}: expected at least one session status per created peer \
             ({} peers), got {statuses:?}",
            peer_ids.len()
        );
        assert!(
            statuses.iter().all(|status| status != "up"),
            "case {case:?}: no session of this emulator is established, got {statuses:?}"
        );

        // RULE 1 while the peers are still coming up...
        let body = database_ok(case, &router, &route_server_id).await;
        let prefixes = reported_prefixes(&body);
        assert!(
            prefixes.is_empty(),
            "case {case:?}: expected a database with no entry for peers whose sessions \
             carried nothing, got prefixes {prefixes:?} in {body}"
        );
        let attributed = reported_peers(&body);
        assert!(
            attributed.is_empty(),
            "case {case:?}: expected no entry attributed to any peer, got {attributed:?} \
             in {body}"
        );
        let token = element(&body, "nextToken");
        assert_eq!(
            token, None,
            "case {case:?}: a database with no further entry carries no NextToken, got \
             {token:?} in {body}"
        );

        // ...and after the clock has settled every peer: becoming `available` establishes no
        // session, so it advertises no route either.
        past_the_transition().await;
        let settled = database_ok(case, &router, &route_server_id).await;
        assert!(
            reported_prefixes(&settled).is_empty() && reported_peers(&settled).is_empty(),
            "case {case:?}: expected a settled route server's database to hold no entry \
             either, got {settled}"
        );
        let statuses = reported_session_statuses(case, &router).await;
        assert!(
            statuses.iter().all(|status| status != "up"),
            "case {case:?}: the clock establishes no BGP session, got {statuses:?}"
        );
    }
}

/// RULE 1: the report is of ONE route server. A peer of another route server is not an entry
/// of this one's database, and each route server is read through its own id.
#[tokio::test]
async fn each_route_server_is_read_through_its_own_id() {
    let case = "two route servers, each with its own endpoint and peer";
    let router = emulator();
    let subnets = subnet_ids(case, &router).await;
    assert!(
        subnets.len() >= 2,
        "case {case:?}: the account must hold two subnets to place two endpoints into, \
         got {subnets:?}"
    );

    let mut ids = Vec::new();
    for (index, subnet) in subnets.iter().take(2).enumerate() {
        let route_server_id = create_route_server(case, &router, 64_512 + index as i64, Vec::new()).await;
        let endpoint_id = create_endpoint(case, &router, &route_server_id, subnet).await;
        create_peer(
            case,
            &router,
            &endpoint_id,
            &format!("198.51.100.{}", index + 20),
            64_700 + index as i64,
            None,
        )
        .await;
        ids.push(route_server_id);
    }

    for route_server_id in &ids {
        let body = database_ok(case, &router, route_server_id).await;
        assert!(
            reported_prefixes(&body).is_empty() && reported_peers(&body).is_empty(),
            "case {case:?}: expected {route_server_id} to report a database with no entry, \
             got {body}"
        );
        // The report is the one this route server's id names: a route server that exists is
        // answered rather than refused, and the refusal table covers the ids that do not.
        let persisted = element(&body, "areRoutesPersisted");
        assert_eq!(
            persisted.as_deref(),
            Some("false"),
            "case {case:?}: expected {route_server_id} to persist nothing, got {persisted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: whether routes are persisted is derived from the stored route server
// ---------------------------------------------------------------------------

/// One row of the persistence table: the case, the create's extra members, the modify's
/// members (empty for a route server nothing modifies), the `areRoutesPersisted` the read
/// answers before the clock settles the move, and the one it answers after.
type PersistRow = (&'static str, Params, Params, bool, bool);

/// RULE 2: `AreRoutesPersisted` follows the STORED route server's persistence configuration
/// as of the read — true exactly while the settled `enabled` state holds — and it agrees with
/// the `persistRoutesState` `DescribeRouteServers` reports for the same route server, before
/// and after the clock settles each move.
///
/// The rows differ in the persistence action (each published member, and none at all), where
/// it is asked for (the create, a later modify), and the configuration it is asked of.
#[tokio::test]
async fn whether_routes_are_persisted_follows_the_stored_route_server() {
    let (floor, ceiling) = ec2::state::PERSIST_ROUTES_DURATION_RANGE;
    let enabled_at = |duration: i64| -> Params {
        vec![
            p("PersistRoutes", "enable"),
            p("PersistRoutesDuration", duration.to_string()),
        ]
    };

    let rows: Vec<PersistRow> = vec![
        (
            "a route server that was never asked to persist anything",
            Vec::new(),
            Vec::new(),
            false,
            false,
        ),
        (
            "persistence asked for at the create, at the published floor",
            enabled_at(floor),
            Vec::new(),
            false,
            true,
        ),
        (
            "persistence asked for at the create, at the published ceiling",
            enabled_at(ceiling),
            Vec::new(),
            false,
            true,
        ),
        (
            "persistence turned on by a later modify",
            Vec::new(),
            enabled_at(floor),
            false,
            true,
        ),
        (
            "persistence turned off by a later modify",
            enabled_at(ceiling),
            vec![p("PersistRoutes", "disable")],
            false,
            false,
        ),
        (
            "the persisted routes of a persisting route server withdrawn",
            enabled_at(floor),
            vec![p("PersistRoutes", "reset")],
            false,
            true,
        ),
        (
            "a reset asked of a route server that persists nothing",
            Vec::new(),
            vec![p("PersistRoutes", "reset")],
            false,
            false,
        ),
    ];

    for (case, created_with, modified_with, before, after) in rows {
        let router = emulator();
        let route_server_id = create_route_server(case, &router, 64_512, created_with).await;
        if !modified_with.is_empty() {
            // The create's own `pending` span is waited out first, so the modify is the one
            // move in flight when the read below happens.
            past_the_transition().await;
            let mut params = vec![p("RouteServerId", route_server_id.clone())];
            params.extend(modified_with);
            ok(case, &router, "ModifyRouteServer", &params).await;
        }

        let body = database_ok(case, &router, &route_server_id).await;
        let persisted = element(&body, "areRoutesPersisted");
        assert_eq!(
            persisted.as_deref(),
            Some(if before { "true" } else { "false" }),
            "case {case:?}: expected areRoutesPersisted {before} while the move is in \
             flight, got {persisted:?} in {body}"
        );
        // The two reads of one route server cannot disagree: persistence is reported here
        // exactly while the state the describe reports is the settled `enabled`.
        let described = described_persistence(case, &router, &route_server_id).await;
        assert_eq!(
            persisted.as_deref() == Some("true"),
            described == ec2::state::PERSIST_ROUTES_STATE_ENABLED,
            "case {case:?}: expected areRoutesPersisted {persisted:?} to agree with the \
             reported persistRoutesState {described:?}"
        );

        // The CLOCK, not a caller, settles the move.
        past_the_transition().await;
        let settled = database_ok(case, &router, &route_server_id).await;
        let persisted = element(&settled, "areRoutesPersisted");
        assert_eq!(
            persisted.as_deref(),
            Some(if after { "true" } else { "false" }),
            "case {case:?}: expected areRoutesPersisted {after} once the clock settled the \
             move, got {persisted:?} in {settled}"
        );
        let described = described_persistence(case, &router, &route_server_id).await;
        assert_eq!(
            persisted.as_deref() == Some("true"),
            described == ec2::state::PERSIST_ROUTES_STATE_ENABLED,
            "case {case:?}: expected the settled areRoutesPersisted {persisted:?} to agree \
             with the reported persistRoutesState {described:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: a request this read cannot serve is refused, not answered
// ---------------------------------------------------------------------------

/// RULE 3 over the `RouteServerId`: absent is `MissingParameter`, a value that is not a route
/// server id is `InvalidRouteServerId.Malformed`, and a well-formed id the account holds no
/// route server for is `InvalidRouteServerId.NotFound` — naming the resource the way EC2
/// names this resource type — rather than a database with no entry, which is what a route
/// server that has learned nothing reports.
///
/// The rows differ in the FORM of the id: absent, no suffix, a suffix of neither published
/// length, a non-hexadecimal digit, the wrong case, another resource's id, a missing
/// separator, and a well-formed id at each published length.
#[tokio::test]
async fn a_route_server_id_that_resolves_to_nothing_is_refused() {
    let router = emulator();
    // One route server the store really holds, so a refusal below is about the id the row
    // names rather than about an empty store.
    let held = create_route_server("the route server the store holds", &router, 64_512, Vec::new()).await;

    // (case, the id, the code, the message)
    let rows: [(&str, &str, &str, String); 10] = [
        (
            "no RouteServerId at all",
            "",
            "MissingParameter",
            "The request must contain the parameter routeServerId".to_string(),
        ),
        (
            "the prefix with no digits",
            "rs-",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-\"".to_string(),
        ),
        (
            "a suffix of neither published length",
            "rs-0a1b2c",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0a1b2c\"".to_string(),
        ),
        (
            "a non-hexadecimal digit in the suffix",
            "rs-0a1b2c3d4e5f6071z",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0a1b2c3d4e5f6071z\"".to_string(),
        ),
        (
            "an upper-case suffix",
            "rs-0A1B2C3D",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs-0A1B2C3D\"".to_string(),
        ),
        (
            "another resource's id",
            "vpc-0a1b2c3d4e5f60718",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"vpc-0a1b2c3d4e5f60718\"".to_string(),
        ),
        (
            "the prefix without its separator",
            "rs0a1b2c3d",
            "InvalidRouteServerId.Malformed",
            "Invalid id: \"rs0a1b2c3d\"".to_string(),
        ),
        (
            "a well-formed short id no route server has",
            "rs-9f8e7d6c",
            "InvalidRouteServerId.NotFound",
            "The route-server ID 'rs-9f8e7d6c' does not exist".to_string(),
        ),
        (
            "a well-formed long id no route server has",
            "rs-0a1b2c3d4e5f60718",
            "InvalidRouteServerId.NotFound",
            "The route-server ID 'rs-0a1b2c3d4e5f60718' does not exist".to_string(),
        ),
        (
            "a well-formed long id of all one digit",
            "rs-11111111111111111",
            "InvalidRouteServerId.NotFound",
            "The route-server ID 'rs-11111111111111111' does not exist".to_string(),
        ),
    ];

    for (case, route_server_id, code, message) in rows {
        let (status, body) = database(&router, route_server_id, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got {body}"
        );

        // The same id is refused the same way with `DryRun` set: the flag asks whether the
        // request WOULD have worked, so one that would have been refused keeps its reason.
        let (status, body) = database(&router, route_server_id, &[p("DryRun", "true")]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected a dry run to keep the refusal's own status, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected a dry run to keep the error code {code:?}, got {body}"
        );
    }

    // The route server the store DOES hold is answered, and a dry run of that read reports
    // the outcome rather than the database.
    let body = database_ok("the route server the store holds", &router, &held).await;
    assert!(
        reported_prefixes(&body).is_empty(),
        "the held route server has learned no route, got {body}"
    );
    let (status, body) = database(&router, &held, &[p("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a dry run of a read that would have succeeded reports that outcome, got {status} \
         with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a dry run of a read that would have succeeded is DryRunOperation, got {body}"
    );
}

/// RULE 3 over the request's `Filters` and `NextToken`: a filter name this operation does not
/// answer is refused rather than ignored, a token this service did not mint is refused rather
/// than read as a position, and so is one minted for a DIFFERENT route server — while a page
/// size at either published bound, and a token minted for the route server being read, are
/// served.
#[tokio::test]
async fn a_filter_or_token_this_read_cannot_serve_is_refused() {
    let router = emulator();
    let first = create_route_server("the first route server", &router, 64_512, Vec::new()).await;
    let second = create_route_server("the second route server", &router, 64_513, Vec::new()).await;

    // (case, the request's extra members, the code, the message)
    let rows: Vec<(&str, Params, &str, String)> = vec![
        (
            "a filter name this operation does not answer",
            filters(&[("prefix-list-id", &["pl-0a1b2c3d"])]),
            "InvalidParameterValue",
            "The filter 'prefix-list-id' is invalid".to_string(),
        ),
        (
            "a filter name differing only in case",
            filters(&[("Prefix", &["10.0.0.0/16"])]),
            "InvalidParameterValue",
            "The filter 'Prefix' is invalid".to_string(),
        ),
        (
            "a filter name of another reported shape",
            filters(&[("route-server-id", &[])]),
            "InvalidParameterValue",
            "The filter 'route-server-id' is invalid".to_string(),
        ),
        (
            "a filter entry carrying no name at all",
            vec![p("Filter.1.Value.1", "10.0.0.0/16")],
            "InvalidParameterValue",
            "The filter '' is invalid".to_string(),
        ),
        (
            "a token that is not hexadecimal",
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a token that is hexadecimal but carries no cursor",
            vec![p("NextToken", "6162636465")],
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a token minted for another route server's database",
            vec![p(
                "NextToken",
                ec2::state::encode_scoped_page_token(&second, "10.0.0.0/16"),
            )],
            "InvalidPaginationToken",
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
    ];

    for (case, extra, code, message) in rows {
        let (status, body) = database(&router, &first, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the error code {code:?}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got {body}"
        );
    }

    // Served: every filter name the operation answers, a page size at each published bound,
    // and a token minted for THIS route server — each answering the database this route
    // server has learned, which holds no entry for any of them to select from.
    let served: Vec<(&str, Params)> = vec![
        ("the smallest published page", vec![p("MaxResults", "5")]),
        ("the largest published page", vec![p("MaxResults", "1000")]),
        (
            "a filter on the prefix",
            filters(&[("prefix", &["10.0.0.0/16"])]),
        ),
        (
            "a filter on the peer an entry came from",
            filters(&[("route-server-peer-id", &["rsp-0a1b2c3d4e5f60718"])]),
        ),
        (
            "a filter on the endpoint an entry arrived on",
            filters(&[("route-server-endpoint-id", &["rse-0a1b2c3d4e5f60718"])]),
        ),
        (
            "a filter on an entry's status in the database",
            filters(&[("route-status", &["in-rib", "in-fib"])]),
        ),
        (
            "a filter on the AS path an entry carries",
            filters(&[("as-paths", &["64512"])]),
        ),
        ("a filter on the metric", filters(&[("med", &["100"])])),
        (
            "a filter on the next hop",
            filters(&[("next-hop-ip", &["10.0.1.5"])]),
        ),
        (
            "a filter on the route table an entry was installed into",
            filters(&[("route-installation-details.route-table-id", &["rtb-0a1b2c3d"])]),
        ),
        (
            "a filter on an entry's installation status",
            filters(&[(
                "route-installation-details.route-installation-status",
                &["installed"],
            )]),
        ),
        (
            "a filter on why an entry's installation ended that way",
            filters(&[(
                "route-installation-details.route-installation-status-reason",
                &["*conflict*"],
            )]),
        ),
        (
            "a token minted for this route server's own database",
            vec![p(
                "NextToken",
                ec2::state::encode_scoped_page_token(&first, "10.0.0.0/16"),
            )],
        ),
    ];

    for (case, extra) in served {
        let (status, body) = database(&router, &first, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert!(
            reported_prefixes(&body).is_empty(),
            "case {case:?}: expected no entry to be selected from a database that holds \
             none, got {body}"
        );
        let token = element(&body, "nextToken");
        assert_eq!(
            token, None,
            "case {case:?}: expected no NextToken on a page nothing follows, got {token:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: the report writes nothing
// ---------------------------------------------------------------------------

/// RULE 4: reading a routing database changes nothing a later caller sees — not the database,
/// not the route server's own states, and not the peers it was derived from.
#[tokio::test]
async fn reading_the_routing_database_writes_nothing() {
    let case = "a persisting route server with a peer, read twice";
    let router = emulator();
    let subnets = subnet_ids(case, &router).await;
    let subnet = subnets
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the account holds no subnet"));
    let (floor, _) = ec2::state::PERSIST_ROUTES_DURATION_RANGE;
    let route_server_id = create_route_server(
        case,
        &router,
        64_512,
        vec![
            p("PersistRoutes", "enable"),
            p("PersistRoutesDuration", floor.to_string()),
        ],
    )
    .await;
    let endpoint_id = create_endpoint(case, &router, &route_server_id, subnet).await;
    create_peer(case, &router, &endpoint_id, "203.0.113.9", 64_601, Some("bfd")).await;
    past_the_transition().await;

    let describe_before = ok(
        case,
        &router,
        "DescribeRouteServers",
        &[p("RouteServerId.1", route_server_id.clone())],
    )
    .await;
    let peers_before = ok(case, &router, "DescribeRouteServerPeers", &[]).await;

    let first = database_ok(case, &router, &route_server_id).await;
    let second = database_ok(case, &router, &route_server_id).await;
    assert_eq!(
        (
            element(&first, "areRoutesPersisted"),
            reported_prefixes(&first),
            element(&first, "nextToken")
        ),
        (
            element(&second, "areRoutesPersisted"),
            reported_prefixes(&second),
            element(&second, "nextToken")
        ),
        "case {case:?}: two reads of one database must answer the same, got {first} \
         then {second}"
    );
    assert_eq!(
        element(&first, "areRoutesPersisted").as_deref(),
        Some("true"),
        "case {case:?}: the stored route server persists its routes, got {first}"
    );

    let describe_after = ok(
        case,
        &router,
        "DescribeRouteServers",
        &[p("RouteServerId.1", route_server_id.clone())],
    )
    .await;
    assert_eq!(
        (
            element(&describe_after, "state"),
            element(&describe_after, "persistRoutesState"),
            element(&describe_after, "persistRoutesDuration")
        ),
        (
            element(&describe_before, "state"),
            element(&describe_before, "persistRoutesState"),
            element(&describe_before, "persistRoutesDuration")
        ),
        "case {case:?}: the read moved the route server: {describe_before} then \
         {describe_after}"
    );

    let peers_after = ok(case, &router, "DescribeRouteServerPeers", &[]).await;
    assert_eq!(
        (
            elements(&peers_after, "routeServerPeerId"),
            elements(&peers_after, "state"),
            elements(&peers_after, "status")
        ),
        (
            elements(&peers_before, "routeServerPeerId"),
            elements(&peers_before, "state"),
            elements(&peers_before, "status")
        ),
        "case {case:?}: the read moved a peer: {peers_before} then {peers_after}"
    );
}
