// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the route server ASSOCIATION slice: `AssociateRouteServer`, which
//! establishes the connection between a route server and a VPC, and
//! `GetRouteServerAssociations`, which reports the connections one route server holds.
//! Driven over the wire the service actually speaks — `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rules under test:
//!
//! 1. `GetRouteServerAssociations` REPORTS THE STORE for the route server the request
//!    names: exactly the (route server, VPC) pairs associated with THAT route server, in
//!    `VpcId` order, and the empty set for a route server that holds none;
//! 2. an association is answered in the non-terminal `associating` state and the CLOCK, not
//!    a caller, moves it to `associated`;
//! 3. a `RouteServerId` that resolves to no route server of the account is REFUSED rather
//!    than answered with an empty set — absent before malformed before unknown — with the
//!    code and the wording EC2 answers each with, and the same resolution order a
//!    `DryRun` request is held to;
//! 4. reading associations WRITES NOTHING: two reads of one route server report the same
//!    associations, and a read never creates one;
//! 5. a route server holds at most ONE association: it writes what it learns into the route
//!    tables of the one VPC it is associated with, so a second `AssociateRouteServer` naming
//!    it — for another VPC or for the one it already serves — is refused with
//!    `InvalidRouteServerId.AlreadyAssociated` and writes nothing, while a VPC already served
//!    by one route server can be served by another; and deleting a VPC takes the association
//!    it held with it rather than leaving one that names a VPC which no longer exists;
//! 6. every way `AssociateRouteServer` can be refused is rendered in THIS SERVICE'S
//!    ec2Query error envelope: the status that refusal class carries, its code, the message
//!    AWS words for it with the request's own values in it, the envelope's media type, and
//!    one `<RequestID>` of its own naming the call — which is what a client of this
//!    operation reads a refusal out of, and what an unimplemented operation's 501 never
//!    gave it.
//!
//! Each table row is a distinct input along what those rules generalise over — which route
//! server, which VPC, how many associations it holds, the form of an id that names nothing
//! (absent, each malformed shape, a well-formed id at both published id lengths), which
//! parameter is missing, and whether the second associate repeats the pair or names a
//! different route server — so no one row is the specification.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, call_with_content_type, element, error_code, error_message};
use super::ec2_query_error_envelope::{RESPONSE_CONTENT_TYPE, refusal_request_id};
use super::wire::{Param, child, child_text, document, param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The state an association the service has just written is in.
const ASSOCIATING: &str = "associating";

/// The state the clock then owes it.
const ASSOCIATED: &str = "associated";

/// One row of the report table: the case, the route server read, and the `(VPC, state)`
/// pairs it must report.
type ReportRow<'a> = (&'a str, &'a str, Vec<(String, String)>);

/// One row of an associate-refusal table: the case, the route server and VPC the request
/// names, the extra parameters it carries, and the error code and message it must be
/// refused with.
type RefusalRow<'a> = (&'a str, &'a str, &'a str, Vec<Param>, &'a str, String);

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn action(router: &MultiServiceRouter, name: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, name, params).await
}

/// One call that must succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, name: &str, params: &[Param]) -> String {
    let (status, body) = action(router, name, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {name}, got {status} with body {body}"
    );
    body
}

/// A route server with `amazon_side_asn`, and the id it was minted with.
async fn create_route_server(case: &str, router: &MultiServiceRouter, amazon_side_asn: &str) -> String {
    let body = ok(
        case,
        router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", amazon_side_asn)],
    )
    .await;
    element(&body, "routeServerId").unwrap_or_else(|| panic!("case {case:?}: no routeServerId in {body}"))
}

/// A VPC with `cidr_block`, and the id it was minted with.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    element(&body, "vpcId").unwrap_or_else(|| panic!("case {case:?}: no vpcId in {body}"))
}

/// Every association a response carries, as `(routeServerId, vpcId, state)` triples in
/// document order.
///
/// An association is the element that carries BOTH ids, which is the one `<item>` shape of
/// `GetRouteServerAssociations`'s set and of `AssociateRouteServer`'s single
/// `<routeServerAssociation>` alike — so both responses are read the one way, and the
/// association's own `<state>` is never confused with a member of something else.
fn associations(case: &str, body: &str) -> Vec<(String, String, String)> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && child(*node, "routeServerId").is_some()
                && child(*node, "vpcId").is_some()
        })
        .map(|node| {
            (
                child_text(node, "routeServerId").unwrap_or_default(),
                child_text(node, "vpcId").unwrap_or_default(),
                child_text(node, "state").unwrap_or_default(),
            )
        })
        .collect()
}

/// The associations `GetRouteServerAssociations` reports for `route_server_id`.
async fn reported(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
) -> Vec<(String, String, String)> {
    let body = ok(
        case,
        router,
        "GetRouteServerAssociations",
        &[param("RouteServerId", route_server_id)],
    )
    .await;
    associations(case, &body)
}

/// The `(vpcId, state)` pairs reported for `route_server_id`, each triple's route server
/// having first been asserted to be the one asked about.
async fn reported_pairs(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
) -> Vec<(String, String)> {
    reported(case, router, route_server_id)
        .await
        .into_iter()
        .map(|(reported_server, vpc_id, state)| {
            assert_eq!(
                reported_server, route_server_id,
                "case {case:?}: an association of {route_server_id} must report that route \
                 server, got {reported_server:?}"
            );
            (vpc_id, state)
        })
        .collect()
}

/// The form fields one `AssociateRouteServer` call carries: the pair it names, plus
/// whatever extra parameters the case needs. Built here so the suite's own calls and the
/// envelope table below name the operation's parameters the one way.
fn associate_params(route_server_id: &str, vpc_id: &str, extra: &[Param]) -> Vec<Param> {
    let mut params = vec![
        param("RouteServerId", route_server_id),
        param("VpcId", vpc_id),
    ];
    params.extend_from_slice(extra);
    params
}

/// `AssociateRouteServer` for one pair, with whatever extra parameters the case needs.
async fn associate(
    router: &MultiServiceRouter,
    route_server_id: &str,
    vpc_id: &str,
    extra: &[Param],
) -> (u16, String) {
    action(
        router,
        "AssociateRouteServer",
        &associate_params(route_server_id, vpc_id, extra),
    )
    .await
}

// ---------------------------------------------------------------------------
// Rules 1, 2 and 4: the read reports the store, and only observes the clock
// ---------------------------------------------------------------------------

/// The rule: `GetRouteServerAssociations` reports exactly the VPCs associated with the
/// route server it names, in `VpcId` order, each in the state the clock has reached — an
/// association being `associating` until the service's own transition span has passed and
/// `associated` after it — and the read itself writes nothing.
///
/// The rows differ in which route server holds the association, which VPC it is with, and
/// whether a route server holds one association or none — a route server holds at most one,
/// which `one_route_server_holds_at_most_one_association` is the rule for.
#[tokio::test]
async fn get_route_server_associations_reports_the_associations_of_the_route_server_named() {
    let router = emulator();

    // Two route servers that hold associations and a third that holds none, so "the
    // associations of THIS route server" is a property of the store rather than of the one
    // route server a recording used.
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;
    let sharing_server = create_route_server("a route server sharing a VPC", &router, "64513").await;
    let unassociated_server = create_route_server("a route server nobody associates", &router, "1").await;

    // Two VPCs of the account, so the VPC side of a pair varies too — and one of them is
    // served by two route servers, which a VPC may be.
    let first_vpc = create_vpc("the first VPC", &router, "10.11.0.0/16").await;
    let third_vpc = create_vpc("the third VPC", &router, "10.13.0.0/16").await;

    // One row per pair the table associates.
    let pairs: [(&str, &str, &str); 3] = [
        ("the first route server with the first VPC", &first_server, &first_vpc),
        ("the second route server with the third VPC", &second_server, &third_vpc),
        ("a second route server for the first VPC", &sharing_server, &first_vpc),
    ];

    for (case, route_server_id, vpc_id) in pairs {
        let (status, body) = associate(&router, route_server_id, vpc_id, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from AssociateRouteServer, got {status} \
             with body {body}"
        );
        // The association AWS hands back is not yet established, and the pair it reports is
        // the pair the request named.
        assert_eq!(
            associations(case, &body),
            vec![(
                route_server_id.to_string(),
                vpc_id.to_string(),
                ASSOCIATING.to_string()
            )],
            "case {case:?}: expected the requested pair in {ASSOCIATING}, got body {body}"
        );
    }

    // Still inside the transition span the associate started: the read OBSERVES the transient
    // state rather than a state nobody waited for.
    let transient = reported_pairs("the first route server, read at once", &router, &first_server).await;
    assert!(
        transient
            .iter()
            .all(|(_, state)| state == ASSOCIATING),
        "case {case:?}: every association must still be {ASSOCIATING:?}, got {transient:?}",
        case = "the first route server, read at once"
    );

    past_the_transition().await;

    // Rule 1: each route server reports exactly its own pairs, in VpcId order, and a route
    // server that holds none reports the empty set.
    let expectations: [ReportRow<'_>; 4] = [
        (
            "the route server holding the first VPC",
            &first_server,
            vec![(first_vpc.clone(), ASSOCIATED.to_string())],
        ),
        (
            "the route server holding the third VPC",
            &second_server,
            vec![(third_vpc.clone(), ASSOCIATED.to_string())],
        ),
        (
            "the route server sharing the first VPC",
            &sharing_server,
            vec![(first_vpc.clone(), ASSOCIATED.to_string())],
        ),
        (
            "the route server holding none",
            &unassociated_server,
            Vec::new(),
        ),
    ];

    for (case, route_server_id, expected) in expectations {
        let reported = reported_pairs(case, &router, route_server_id).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} for {route_server_id}, got {reported:?}"
        );
        // Rule 4: the read changed nothing a later caller sees — not for the route server
        // read, and not for a route server that has no association to create by being
        // asked about.
        let again = reported_pairs(case, &router, route_server_id).await;
        assert_eq!(
            again, expected,
            "case {case:?}: a second read of {route_server_id} must report {expected:?}, \
             got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an id that resolves to nothing is refused, not answered
// ---------------------------------------------------------------------------

/// The rule: a `RouteServerId` the request left out is `MissingParameter`, one that is not
/// a route server id at all is `InvalidRouteServerId.Malformed`, and a well-formed one the
/// account holds no route server for is `InvalidRouteServerId.NotFound` naming the resource
/// the way EC2 names this resource type — and both reads and the associate answer the same
/// refusal for the same id, before any `DryRun` is honoured.
///
/// The rows differ in the FORM of the id: absent, no suffix, a suffix of the wrong length,
/// a non-hexadecimal digit, the wrong case, another resource's id, a missing separator, and
/// a well-formed id at each of the two published EC2 id lengths.
#[tokio::test]
async fn an_unresolvable_route_server_id_is_refused_rather_than_answered() {
    let router = emulator();
    let vpc_id = create_vpc("a VPC to associate", &router, "10.21.0.0/16").await;

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
        // The read refuses it...
        let (status, body) = action(
            &router,
            "GetRouteServerAssociations",
            &[param("RouteServerId", route_server_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from GetRouteServerAssociations, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code} from GetRouteServerAssociations, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got body {body}"
        );

        // ...and so does a DryRun of it: the flag asks whether the request WOULD have
        // worked, and this one would not.
        let (dry_status, dry_body) = action(
            &router,
            "GetRouteServerAssociations",
            &[
                param("RouteServerId", route_server_id),
                param("DryRun", "true"),
            ],
        )
        .await;
        assert_eq!(
            error_code(&dry_body),
            code,
            "case {case:?}: a dry run must be refused with {code} too, got {dry_status} \
             with body {dry_body}"
        );

        // ...and the associate resolves the same id the same way, before it looks at the
        // VPC the request named.
        let (associate_status, associate_body) = associate(&router, route_server_id, &vpc_id, &[]).await;
        assert_eq!(
            associate_status, 400,
            "case {case:?}: expected HTTP 400 from AssociateRouteServer, got \
             {associate_status} with body {associate_body}"
        );
        assert_eq!(
            error_code(&associate_body),
            code,
            "case {case:?}: expected {code} from AssociateRouteServer, got body \
             {associate_body}"
        );
    }
}

/// A `DryRun` of a request that WOULD have succeeded answers `DryRunOperation` and writes
/// nothing — asserted on both operations of this slice, the read and the write.
#[tokio::test]
async fn a_dry_run_of_a_request_that_would_succeed_writes_nothing() {
    let router = emulator();
    let route_server_id = create_route_server("the route server", &router, "65001").await;
    let vpc_id = create_vpc("the VPC", &router, "10.22.0.0/16").await;

    for (case, name, params) in [
        (
            "a dry run of the read",
            "GetRouteServerAssociations",
            vec![
                param("RouteServerId", &route_server_id),
                param("DryRun", "true"),
            ],
        ),
        (
            "a dry run of the associate",
            "AssociateRouteServer",
            vec![
                param("RouteServerId", &route_server_id),
                param("VpcId", &vpc_id),
                param("DryRun", "true"),
            ],
        ),
    ] {
        let (status, body) = action(&router, name, &params).await;
        assert_eq!(
            status, 412,
            "case {case:?}: expected HTTP 412 from {name}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "DryRunOperation",
            "case {case:?}: expected DryRunOperation from {name}, got body {body}"
        );
    }

    // Neither dry run wrote an association.
    let after = reported_pairs("after the dry runs", &router, &route_server_id).await;
    assert!(
        after.is_empty(),
        "case {case:?}: a dry run must write no association, got {after:?}",
        case = "after the dry runs"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: one route server holds at most one association
// ---------------------------------------------------------------------------

/// The rule: an association is the connection between one route server and one VPC, and the
/// route server is what holds it — it writes the routes it learns into the route tables of
/// the one VPC it is associated with — so a route server that already holds an association
/// takes no second one, whether the request names another VPC or repeats the association it
/// already has: that is `InvalidRouteServerId.AlreadyAssociated`, with nothing written. The
/// VPC is the association's SUBJECT rather than its holder, so a VPC already served by one
/// route server can be served by another. An unknown VPC id is refused with the code every
/// operation of this service refuses one with, and deleting a VPC takes the association it
/// held with it.
///
/// The rows differ in which route server the second associate names, which VPC it names,
/// which parameter the request leaves out, and whether the VPC named exists at all.
#[tokio::test]
async fn one_route_server_holds_at_most_one_association() {
    let router = emulator();
    let first_server = create_route_server("the associated route server", &router, "65010").await;
    let second_server = create_route_server("another route server", &router, "65011").await;
    let third_server = create_route_server("a third route server", &router, "65012").await;
    let associated_vpc = create_vpc("the VPC that gets associated", &router, "10.31.0.0/16").await;
    let free_vpc = create_vpc("a VPC nobody associates", &router, "10.32.0.0/16").await;

    let (status, body) = associate(&router, &first_server, &associated_vpc, &[]).await;
    assert_eq!(
        status, 200,
        "the first associate must succeed, got {status} with body {body}"
    );

    // A VPC is the subject of an association, not its holder, so a SECOND route server may be
    // associated with the VPC the first one already serves.
    let (status, second_body) = associate(&router, &second_server, &associated_vpc, &[]).await;
    assert_eq!(
        status, 200,
        "a second route server must take the same VPC, got {status} with body {second_body}"
    );
    assert_eq!(
        (
            element(&second_body, "routeServerId"),
            element(&second_body, "vpcId")
        ),
        (Some(second_server.clone()), Some(associated_vpc.clone())),
        "the second association must name the pair it was asked for, got {second_body}"
    );

    // (case, the route server, the VPC, the extra parameters, the code, the message)
    let rows: [RefusalRow<'_>; 5] = [
        (
            "the same pair again",
            &first_server,
            &associated_vpc,
            Vec::new(),
            "InvalidRouteServerId.AlreadyAssociated",
            format!("The route server '{first_server}' is already associated with vpc '{associated_vpc}'"),
        ),
        (
            "the associated route server for another VPC",
            &first_server,
            &free_vpc,
            Vec::new(),
            "InvalidRouteServerId.AlreadyAssociated",
            format!("The route server '{first_server}' is already associated with vpc '{associated_vpc}'"),
        ),
        (
            "a dry run of a request that would be refused",
            &first_server,
            &free_vpc,
            vec![param("DryRun", "true")],
            "InvalidRouteServerId.AlreadyAssociated",
            format!("The route server '{first_server}' is already associated with vpc '{associated_vpc}'"),
        ),
        (
            "a VPC the account does not hold",
            &third_server,
            "vpc-0a1b2c3d4e5f60719",
            Vec::new(),
            "InvalidVpcID.NotFound",
            "The vpc ID 'vpc-0a1b2c3d4e5f60719' does not exist".to_string(),
        ),
        (
            "no VpcId at all",
            &third_server,
            "",
            Vec::new(),
            "MissingParameter",
            "The request must contain the parameter vpcId".to_string(),
        ),
    ];

    for (case, route_server_id, vpc_id, extra, code, message) in rows {
        let (status, body) = associate(&router, route_server_id, vpc_id, &extra).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from AssociateRouteServer, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected {code}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got body {body}"
        );
        // Nothing was written: the third route server holds no association, and the first
        // still holds exactly the one it was given.
        let third = reported_pairs(case, &router, &third_server).await;
        assert!(
            third.is_empty(),
            "case {case:?}: a refused associate must write nothing, got {third:?}"
        );
        let first = reported_pairs(case, &router, &first_server).await;
        assert_eq!(
            first.len(),
            1,
            "case {case:?}: the first route server must still hold its one association, \
             got {first:?}"
        );
        assert_eq!(
            first[0].0, associated_vpc,
            "case {case:?}: that association must still be with {associated_vpc}, got {first:?}"
        );
    }

    // A route server association is the connection between a route server and THIS VPC, so
    // deleting the VPC takes it away rather than leaving an association naming a VPC that is
    // gone — and the delete itself is not refused over it, which is what a recorded
    // `DeleteVpc` of a VPC a route server had been associated with shows.
    let (status, body) = action(&router, "DeleteVpc", &[param("VpcId", &associated_vpc)]).await;
    assert_eq!(
        status, 200,
        "deleting an associated VPC must succeed, got {status} with body {body}"
    );
    let after_delete = reported_pairs("after the VPC was deleted", &router, &first_server).await;
    assert!(
        after_delete.is_empty(),
        "the association must go with the VPC, got {after_delete:?}"
    );
    // The VPC the association was taken from can then be associated again, by either route
    // server, because the pair it held is gone rather than merely hidden.
    let (free_status, free_body) = associate(&router, &second_server, &free_vpc, &[]).await;
    assert_eq!(
        free_status, 200,
        "an unassociated VPC must still associate, got {free_status} with body {free_body}"
    );
    let second_pairs = reported_pairs("the second route server after its associate", &router, &second_server).await;
    assert_eq!(
        second_pairs.len(),
        1,
        "the second route server must now hold one association, got {second_pairs:?}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: every refusal of the associate is rendered in the service's envelope
// ---------------------------------------------------------------------------

/// The status EC2 answers a `DryRun` the caller asked for with: the request was refused
/// because a precondition the CALLER set holds, not because it was malformed.
const DRY_RUN_STATUS: u16 = 412;

/// One row of the envelope table: the case, the form fields the request carries, and the
/// status, code and message the service must refuse it with.
struct EnvelopeRow {
    case: String,
    params: Vec<Param>,
    status: u16,
    code: &'static str,
    message: String,
}

/// The rule: whichever way `AssociateRouteServer` is refused, the refusal reaches the client
/// as this service's ec2Query error envelope — the status that refusal class carries, its
/// `<Code>`, a `<Message>` worded for that class with the request's own values quoted into
/// it, the envelope's media type, and one `<RequestID>` of its own, in the 8-4-4-4-12 form,
/// closing it.
///
/// The rows differ in WHICH refusal it is (a parameter sent empty, an id of neither route
/// server form, a well-formed route server id the account holds nothing for at
/// each published id length, a VPC the account does not hold, a VPC that already holds an
/// association, and a `DryRun` of a request that would otherwise have succeeded), in WHICH
/// member carries the offending value, and in WHICH status it carries — so no one row is the
/// specification, and none of them is the 501 an operation nothing serves answers.
#[tokio::test]
async fn every_refusal_of_the_associate_is_rendered_in_this_services_error_envelope() {
    let router = emulator();
    let route_server_id = create_route_server("the route server", &router, "65020").await;
    let free_server = create_route_server("a route server with no association", &router, "65021")
        .await;
    let associated_vpc =
        create_vpc("the VPC that holds an association", &router, "10.41.0.0/16").await;
    let free_vpc = create_vpc("a VPC that holds none", &router, "10.42.0.0/16").await;

    // The one association every "already associated" row below is refused over.
    let (status, body) = associate(&router, &route_server_id, &associated_vpc, &[]).await;
    assert_eq!(
        status, 200,
        "the association under test must be written, got {status} with body {body}"
    );

    // A well-formed route server id at each of the two published EC2 id lengths that this
    // store minted nothing for.
    let unknown_short = "rs-1a2b3c4d";
    let unknown_long = "rs-1a2b3c4d5e6f70819";
    // A value that is not a route server id of either form.
    let malformed = "rs-not-hex";
    // A well-formed VPC id the account holds no VPC for.
    let unknown_vpc = "vpc-1a2b3c4d5e6f70819";

    let rows = vec![
        // Both ids are modelled `clientOptional`, so a field sent EMPTY reaches the handler
        // as an empty value and is the parameter missing. (A field left out of the body
        // altogether never reaches the handler at all: the boundary validator in front of
        // every operation of this service refuses it first, which is that validator's
        // behaviour rather than this operation's.)
        EnvelopeRow {
            case: "an empty RouteServerId".to_string(),
            params: associate_params("", &free_vpc, &[]),
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter routeServerId".to_string(),
        },
        EnvelopeRow {
            case: "an empty VpcId".to_string(),
            params: associate_params(&route_server_id, "", &[]),
            status: 400,
            code: "MissingParameter",
            message: "The request must contain the parameter vpcId".to_string(),
        },
        EnvelopeRow {
            case: "a value that is not a route server id".to_string(),
            params: associate_params(malformed, &free_vpc, &[]),
            status: 400,
            code: "InvalidRouteServerId.Malformed",
            message: format!("Invalid id: \"{malformed}\""),
        },
        EnvelopeRow {
            case: "a well-formed short route server id the account holds nothing for".to_string(),
            params: associate_params(unknown_short, &free_vpc, &[]),
            status: 400,
            code: "InvalidRouteServerId.NotFound",
            message: format!("The route-server ID '{unknown_short}' does not exist"),
        },
        EnvelopeRow {
            case: "a well-formed long route server id the account holds nothing for".to_string(),
            params: associate_params(unknown_long, &free_vpc, &[]),
            status: 400,
            code: "InvalidRouteServerId.NotFound",
            message: format!("The route-server ID '{unknown_long}' does not exist"),
        },
        EnvelopeRow {
            case: "a VPC the account does not hold".to_string(),
            params: associate_params(&route_server_id, unknown_vpc, &[]),
            status: 400,
            code: "InvalidVpcID.NotFound",
            message: format!("The vpc ID '{unknown_vpc}' does not exist"),
        },
        EnvelopeRow {
            case: "a route server that already holds an association".to_string(),
            params: associate_params(&route_server_id, &free_vpc, &[]),
            status: 400,
            code: "InvalidRouteServerId.AlreadyAssociated",
            message: format!(
                "The route server '{route_server_id}' is already associated with vpc \
                 '{associated_vpc}'"
            ),
        },
        EnvelopeRow {
            case: "a DryRun of a request that would have succeeded".to_string(),
            params: associate_params(&free_server, &free_vpc, &[param("DryRun", "true")]),
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
    ];

    // Every row runs against ONE emulator, so the request ids are compared across refusals
    // of this operation and not only within one row.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in rows {
        let EnvelopeRow {
            case,
            params,
            status,
            code,
            message,
        } = row;
        let (actual_status, content_type, body) =
            call_with_content_type(&router, REGION, "AssociateRouteServer", &params).await;
        assert_eq!(
            actual_status, status,
            "case {case:?}: expected HTTP {status}, got {actual_status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {case:?}: expected the code {code:?}, got body {body}"
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {case:?}: expected the message {message:?}, got body {body}"
        );
        assert_eq!(
            content_type.as_deref(),
            Some(RESPONSE_CONTENT_TYPE),
            "case {case:?}: expected the media type {RESPONSE_CONTENT_TYPE:?}, got \
             {content_type:?} with body {body}"
        );
        let request_id = refusal_request_id(&case, &body, code);
        assert!(
            seen.insert(request_id.clone()),
            "case {case:?}: the request id {request_id:?} was already reported for another \
             response"
        );
    }

    // None of those refusals wrote anything: the one association made above is still the
    // only one this route server holds, and the VPC the rows kept naming is still free.
    let after = reported_pairs("after every refusal", &router, &route_server_id).await;
    assert_eq!(
        after.len(),
        1,
        "case {case:?}: a refused associate must write nothing, got {after:?}",
        case = "after every refusal"
    );
    assert_eq!(
        after[0].0, associated_vpc,
        "case {case:?}: the one association must still be with {associated_vpc}, got {after:?}",
        case = "after every refusal"
    );
    let (free_status, free_body) = associate(&router, &free_server, &free_vpc, &[]).await;
    assert_eq!(
        free_status, 200,
        "the VPC only ever named in refused requests must still associate, got {free_status} \
         with body {free_body}"
    );
}
