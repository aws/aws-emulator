// SPDX-License-Identifier: CC0-1.0

//! What a SUCCESSFUL delete makes observable on the Network Access Scope, Traffic Mirror
//! filter and route server surfaces, driven over the ec2Query wire the service speaks
//! (`Action=<Operation>` in an `application/x-www-form-urlencoded` POST, answered with XML —
//! README.md / OPERATIONS.md "Wire protocol").
//!
//! ONE rule, in the two shapes the model gives it: a delete that answered 200 is observable
//! in the state every later read uses.
//!
//! * A resource whose modelled shape carries NO state member has no terminal state to be
//!   reported in, so its id stops resolving: every read of it answers HTTP 400 with that
//!   resource type's `Invalid<Resource>Id.NotFound` and the id the request carried, and the
//!   unfiltered describe stops listing it — `NetworkInsightsAccessScope` and
//!   `TrafficMirrorFilter` are both of that kind
//!   (`smithy_query(query="shape",shape="NetworkInsightsAccessScope")`,
//!   `smithy_query(query="shape",shape="TrafficMirrorFilter")`).
//! * A resource whose state enum HAS terminal delete members keeps being described, its
//!   state advancing through them: `RouteServerState` is `pending`, `available`,
//!   `modifying`, `deleting` and `deleted`
//!   (`smithy_query(query="shape",shape="RouteServerState")`), so a deleted route server is
//!   reported `deleting` and then, one transition span later, `deleted` — and no operation
//!   that ACTS on a route server acts on one in either of those states.
//!
//! Each table row is a distinct input along what the rule generalises over — which members
//! the deleted resource carries, how many of its kind the store holds beside it, which of
//! them is deleted, and which read is made afterwards — so no single row is the
//! specification. The sibling suite `deleted_resources_and_dependencies` holds the same rule
//! for the resource kinds whose delete was already implemented.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use ec2::ops::types::{Tag, TrafficMirrorSession};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, AMAZON_SIDE_ASN_RANGE, DEPENDENCY_VIOLATION,
    INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
    INVALID_TRAFFIC_MIRROR_FILTER_ID_NOT_FOUND, ROUTE_SERVER_STATE_AVAILABLE,
    ROUTE_SERVER_STATE_DELETED, ROUTE_SERVER_STATE_DELETING, ROUTE_SERVER_TAGGABLE_RESOURCE,
    TRAFFIC_MIRROR_FILTER_TAGGABLE_RESOURCE, TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, indexed};
use super::wire::{param, Param};

/// The Region every call here is signed for. None of the three resources' modelled shapes
/// names a Region, so the Region only selects the envelope and the ARNs a create derives.
const REGION: &str = "us-west-2";

/// The code EC2 answers a request whose resource exists but is not in a state that admits
/// it, as this crate's state module spells it. Written out rather than imported, because the
/// state module re-exports that one code from two of its files.
const INCORRECT_STATE: &str = "IncorrectState";

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn call(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
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

/// Pin that `action` refused `params` with HTTP `status`, `code` and `message`.
async fn refused(
    case: &str,
    router: &MultiServiceRouter,
    action: &str,
    params: &[Param],
    status: u16,
    code: &str,
    message: &str,
) {
    let (answered, body) = call(router, action, params).await;
    assert_eq!(
        answered, status,
        "case {case:?}: expected HTTP {status} from {action}, got {answered} with body {body}"
    );
    let answered_code = error_code(&body);
    assert_eq!(
        answered_code, code,
        "case {case:?}: expected {code} from {action}, got {answered_code:?} in {body}"
    );
    let answered_message = error_message(&body);
    assert_eq!(
        answered_message, message,
        "case {case:?}: expected the message {message:?} from {action}, got \
         {answered_message:?}"
    );
}

/// Wait until every clock-owed transition started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

// ---------------------------------------------------------------------------
// Network Access Scopes
// ---------------------------------------------------------------------------

/// The form fields of one `MatchPath`/`ExcludePath` whose source names `resource_type`, as
/// ec2Query flattens the nested request shapes.
fn path_params(list_member: &str, resource_type: &str) -> Vec<Param> {
    vec![param(
        &format!("{list_member}.1.Source.ResourceStatement.ResourceType.1"),
        resource_type,
    )]
}

/// The form fields of one tag on the scope a create mints.
fn scope_tag_params(key: &str, value: &str) -> Vec<Param> {
    vec![
        param(
            "TagSpecification.1.ResourceType",
            ec2::state::RESOURCE_TYPE_NETWORK_INSIGHTS_ACCESS_SCOPE,
        ),
        param("TagSpecification.1.Tag.1.Key", key),
        param("TagSpecification.1.Tag.1.Value", value),
    ]
}

/// RULE, first shape: a deleted Network Access Scope stops resolving for EVERY read of it —
/// the describe narrowed to its id, the content get, and the unfiltered describe that listed
/// it — while the scopes the account still holds are reported exactly as before.
///
/// The rows differ along what the rule generalises over: which content the deleted scope
/// carried (none, a match path, an exclude path, both) and whether it carried tags; and each
/// row is driven at all three positions of a three-scope store, so the first, a middle and
/// the last scope of the store are each the one deleted.
#[tokio::test]
async fn a_deleted_access_scope_stops_resolving_for_every_read() {
    let rows: Vec<(&str, Vec<Param>)> = vec![
        ("a scope with neither paths nor tags", Vec::new()),
        (
            "a scope with one match path",
            path_params("MatchPath", "AWS::EC2::VPC"),
        ),
        (
            "a scope with one exclude path and a tag",
            [
                path_params("ExcludePath", "AWS::EC2::NetworkInterface"),
                scope_tag_params("Team", "platform"),
            ]
            .concat(),
        ),
        (
            "a scope with paths in both directions",
            [
                path_params("MatchPath", "AWS::EC2::InternetGateway"),
                path_params("ExcludePath", "AWS::EC2::Subnet"),
            ]
            .concat(),
        ),
    ];

    for (case, content) in rows {
        for position in 0..3usize {
            let case = format!("{case}, deleted at position {position} of three");
            let case = case.as_str();
            let (router, _engine) = super::ec2_query::emulator();

            // Three scopes, so a scope the request did not name has to be left alone; the
            // row's own content sits at `position`, which moves with the loop.
            let mut scopes = Vec::new();
            for index in 0..3usize {
                let mut params = vec![param("ClientToken", &format!("token-{index}"))];
                if index == position {
                    params.extend(content.clone());
                }
                let body = ok(case, &router, "CreateNetworkInsightsAccessScope", &params).await;
                scopes.push(reported(case, &body, "networkInsightsAccessScopeId"));
            }
            let target = scopes[position].clone();
            let by_id = indexed("NetworkInsightsAccessScopeId", &[target.as_str()]);
            let named = vec![param("NetworkInsightsAccessScopeId", &target)];

            // Both reads report the scope BEFORE the delete, so the refusals below are the
            // delete's doing rather than a read that never reported it.
            for (read, params) in [
                ("DescribeNetworkInsightsAccessScopes", by_id.clone()),
                ("GetNetworkInsightsAccessScopeContent", named.clone()),
            ] {
                let body = ok(case, &router, read, &params).await;
                assert!(
                    body.contains(&target),
                    "case {case:?}: {read} must report {target} before the delete, got {body}"
                );
            }

            // The delete answers the id of the scope it took away.
            let body = ok(
                case,
                &router,
                "DeleteNetworkInsightsAccessScope",
                &named,
            )
            .await;
            let answered = reported(case, &body, "networkInsightsAccessScopeId");
            assert_eq!(
                answered, target,
                "case {case:?}: expected the delete to answer {target}, got {answered:?}"
            );

            // Every read of that id now refuses, with this resource's own not-found code and
            // the id the REQUEST carried.
            let not_found_message = format!("{target} does not exist");
            for (read, params) in [
                ("DescribeNetworkInsightsAccessScopes", by_id.clone()),
                ("GetNetworkInsightsAccessScopeContent", named.clone()),
            ] {
                refused(
                    case,
                    &router,
                    read,
                    &params,
                    400,
                    INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
                    &not_found_message,
                )
                .await;
            }

            // The unfiltered describe reports the store: the deleted scope is gone from it
            // and the two the account still holds are both still there.
            let body = ok(
                case,
                &router,
                "DescribeNetworkInsightsAccessScopes",
                &[],
            )
            .await;
            let listed = elements(&body, "networkInsightsAccessScopeId");
            assert!(
                !listed.contains(&target),
                "case {case:?}: the unfiltered describe must not list the deleted {target}, \
                 got {listed:?}"
            );
            for (index, scope) in scopes.iter().enumerate() {
                if index == position {
                    continue;
                }
                assert!(
                    listed.contains(scope),
                    "case {case:?}: the unfiltered describe must still list {scope}, got \
                     {listed:?}"
                );
            }

            // A second delete is refused rather than answered as a no-op: a caller that
            // deletes twice has to be able to tell the deletion it performed from one it did
            // not. The id is one this service ISSUED whose scope has gone, so the refusal is
            // this resource's own not-found code — the value refusal the delete words its own
            // way is for a value the mint could never have handed out.
            refused(
                case,
                &router,
                "DeleteNetworkInsightsAccessScope",
                &named,
                400,
                INVALID_NETWORK_INSIGHTS_ACCESS_SCOPE_ID_NOT_FOUND,
                &not_found_message,
            )
            .await;

            // The client token that named the deleted scope names it no longer, so a replay
            // of the create that made it mints a NEW scope instead of reporting one nothing
            // holds.
            let mut params = vec![param("ClientToken", &format!("token-{position}"))];
            params.extend(content.clone());
            let body = ok(case, &router, "CreateNetworkInsightsAccessScope", &params).await;
            let minted = reported(case, &body, "networkInsightsAccessScopeId");
            assert_ne!(
                minted, target,
                "case {case:?}: a create replayed after the delete must mint a new scope, got \
                 {minted:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Traffic Mirror filters
// ---------------------------------------------------------------------------

/// The form fields of one tag on the filter a create mints.
fn filter_tag_params(key: &str, value: &str) -> Vec<Param> {
    vec![
        param(
            "TagSpecification.1.ResourceType",
            TRAFFIC_MIRROR_FILTER_TAGGABLE_RESOURCE,
        ),
        param("TagSpecification.1.Tag.1.Key", key),
        param("TagSpecification.1.Tag.1.Value", value),
    ]
}

/// RULE, first shape, for the other resource kind of that shape: a deleted Traffic Mirror
/// filter stops resolving for the describe, both narrowed to its id and unfiltered, and the
/// filters the account still holds are reported exactly as before.
///
/// The rows differ along what the rule generalises over: which members the deleted filter
/// carried (none, a description, a description and a tag), and each row is driven at all
/// three positions of a three-filter store.
#[tokio::test]
async fn a_deleted_traffic_mirror_filter_stops_resolving_for_the_describe() {
    let rows: Vec<(&str, Vec<Param>)> = vec![
        ("a filter carrying nothing of its own", Vec::new()),
        (
            "a filter with a description",
            vec![param("Description", "web tier mirror")],
        ),
        (
            "a filter with a description and a tag",
            [
                vec![param("Description", "mirror: ingress/egress — grün")],
                filter_tag_params("Env", "staging"),
            ]
            .concat(),
        ),
    ];

    for (case, extra) in rows {
        for position in 0..3usize {
            let case = format!("{case}, deleted at position {position} of three");
            let case = case.as_str();
            let (router, _engine) = super::ec2_query::emulator();

            let mut filters = Vec::new();
            for index in 0..3usize {
                let params = if index == position {
                    extra.clone()
                } else {
                    Vec::new()
                };
                let body = ok(case, &router, "CreateTrafficMirrorFilter", &params).await;
                filters.push(reported(case, &body, "trafficMirrorFilterId"));
            }
            let target = filters[position].clone();
            let by_id = indexed("TrafficMirrorFilterId", &[target.as_str()]);

            let body = ok(case, &router, "DescribeTrafficMirrorFilters", &by_id).await;
            assert!(
                body.contains(&target),
                "case {case:?}: the describe must report {target} before the delete, got {body}"
            );

            let body = ok(
                case,
                &router,
                "DeleteTrafficMirrorFilter",
                &[param("TrafficMirrorFilterId", &target)],
            )
            .await;
            let answered = reported(case, &body, "trafficMirrorFilterId");
            assert_eq!(
                answered, target,
                "case {case:?}: expected the delete to answer {target}, got {answered:?}"
            );

            // The id stops resolving, for the describe and for a second delete alike.
            let not_found_message = format!("{target} does not exist");
            refused(
                case,
                &router,
                "DescribeTrafficMirrorFilters",
                &by_id,
                400,
                INVALID_TRAFFIC_MIRROR_FILTER_ID_NOT_FOUND,
                &not_found_message,
            )
            .await;
            refused(
                case,
                &router,
                "DeleteTrafficMirrorFilter",
                &[param("TrafficMirrorFilterId", &target)],
                400,
                INVALID_TRAFFIC_MIRROR_FILTER_ID_NOT_FOUND,
                &not_found_message,
            )
            .await;

            let body = ok(case, &router, "DescribeTrafficMirrorFilters", &[]).await;
            let listed = elements(&body, "trafficMirrorFilterId");
            assert!(
                !listed.contains(&target),
                "case {case:?}: the unfiltered describe must not list the deleted {target}, \
                 got {listed:?}"
            );
            for (index, filter) in filters.iter().enumerate() {
                if index == position {
                    continue;
                }
                assert!(
                    listed.contains(filter),
                    "case {case:?}: the unfiltered describe must still list {filter}, got \
                     {listed:?}"
                );
            }
        }
    }
}

/// "You cannot delete a Traffic Mirror filter that is in use by a Traffic Mirror session"
/// (`DeleteTrafficMirrorFilter`): while a stored session mirrors by the filter the delete is
/// refused with `DependencyViolation` — worded for the resource THE REQUEST named, as this
/// error class is worded for every other resource of this service — and the filter is left
/// stored, while the very same delete succeeds once the session is gone, so the refusal
/// follows from the store rather than from the shape of the request.
///
/// `CreateTrafficMirrorSession` is still a scaffold stub with an issue of its own, so the
/// session is seeded through the store's own write path, which is where that create will
/// write it.
#[tokio::test]
async fn a_traffic_mirror_filter_a_session_mirrors_by_is_not_deleted() {
    // The rows differ along how many sessions hold the filter, which is what the message
    // names: one session, and two.
    for sessions_held in 1..=2usize {
        let case = format!("a filter held by {sessions_held} session(s)");
        let case = case.as_str();
        let (router, engine) = super::ec2_query::emulator();

        // Two filters: the one the sessions mirror by, and one nothing holds, so the refusal
        // is about the filter the sessions name.
        let body = ok(case, &router, "CreateTrafficMirrorFilter", &[]).await;
        let held = reported(case, &body, "trafficMirrorFilterId");
        let body = ok(case, &router, "CreateTrafficMirrorFilter", &[]).await;
        let free = reported(case, &body, "trafficMirrorFilterId");

        let mut session_ids = Vec::new();
        for index in 0..sessions_held {
            let session = TrafficMirrorSession {
                traffic_mirror_session_id: None,
                traffic_mirror_target_id: Some(format!("tmt-{index:017x}")),
                traffic_mirror_filter_id: Some(held.clone()),
                network_interface_id: Some(format!("eni-{index:017x}")),
                owner_id: Some(::engine::emulator_account_id().to_string()),
                packet_length: None,
                session_number: Some(1 + i32::try_from(index).unwrap_or_default()),
                virtual_network_id: None,
                description: None,
                tags: Some(vec![Tag {
                    key: Some("Suite".to_string()),
                    value: Some("deleted-resources".to_string()),
                }]),
            };
            let seeded = engine
                .state::<Ec2State>()
                .lock()
                .insert_traffic_mirror_session(session)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: the store must have room for a seeded session")
                });
            session_ids.push(seeded);
        }

        refused(
            case,
            &router,
            "DeleteTrafficMirrorFilter",
            &[param("TrafficMirrorFilterId", &held)],
            400,
            DEPENDENCY_VIOLATION,
            &format!("The trafficMirrorFilter '{held}' has dependencies and cannot be deleted."),
        )
        .await;

        // The refused delete performed nothing: the filter is still described.
        let body = ok(
            case,
            &router,
            "DescribeTrafficMirrorFilters",
            &indexed("TrafficMirrorFilterId", &[held.as_str()]),
        )
        .await;
        assert!(
            body.contains(&held),
            "case {case:?}: a refused delete must leave {held} stored, got {body}"
        );

        // The filter nothing holds deletes while the held one does not, so the guard is read
        // off the sessions rather than applied to every filter.
        ok(
            case,
            &router,
            "DeleteTrafficMirrorFilter",
            &[param("TrafficMirrorFilterId", &free)],
        )
        .await;

        // Once no session mirrors by it, the very same delete succeeds and the id stops
        // resolving.
        for session_id in &session_ids {
            ok(
                case,
                &router,
                "DeleteTrafficMirrorSession",
                &[param("TrafficMirrorSessionId", session_id)],
            )
            .await;
        }
        ok(
            case,
            &router,
            "DeleteTrafficMirrorFilter",
            &[param("TrafficMirrorFilterId", &held)],
        )
        .await;
        refused(
            case,
            &router,
            "DescribeTrafficMirrorFilters",
            &indexed("TrafficMirrorFilterId", &[held.as_str()]),
            400,
            INVALID_TRAFFIC_MIRROR_FILTER_ID_NOT_FOUND,
            &format!("{held} does not exist"),
        )
        .await;
    }
}

// ---------------------------------------------------------------------------
// Route servers
// ---------------------------------------------------------------------------

/// The `state` `DescribeRouteServers` reports for the one route server `route_server_id`.
async fn described_state(case: &str, router: &MultiServiceRouter, route_server_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeRouteServers",
        &indexed("RouteServerId", &[route_server_id]),
    )
    .await;
    assert!(
        body.contains(route_server_id),
        "case {case:?}: the describe must report {route_server_id}, got {body}"
    );
    reported(case, &body, "state")
}

/// RULE, second shape: a deleted route server is still DESCRIBED, in the delete states its
/// modelled enum publishes — `deleting` while the move is owed, then the terminal `deleted` —
/// so a caller polling for the delete to finish has something to stop on, while the route
/// servers the account still holds stay `available`.
///
/// The rows differ along what the rule generalises over: the ASN the route server was created
/// with (both published bounds of `AmazonSideAsn` and a value between them), whether it
/// persists routes and whether its notifications are on.
#[tokio::test]
async fn a_deleted_route_server_is_described_deleting_then_deleted() {
    let (smallest_asn, largest_asn) = AMAZON_SIDE_ASN_RANGE;
    let rows: Vec<(&str, Vec<Param>)> = vec![
        (
            "the smallest documented ASN",
            vec![param("AmazonSideAsn", &smallest_asn.to_string())],
        ),
        (
            "a 16-bit private ASN that persists routes",
            vec![
                param("AmazonSideAsn", "64512"),
                param("PersistRoutes", "enable"),
                param("PersistRoutesDuration", "1"),
            ],
        ),
        (
            "the largest documented ASN with notifications on",
            vec![
                param("AmazonSideAsn", &largest_asn.to_string()),
                param("SnsNotificationsEnabled", "true"),
            ],
        ),
    ];

    for (case, create_params) in rows {
        let (router, _engine) = super::ec2_query::emulator();

        // A second route server nothing deletes, so a delete that reached the wrong record
        // would show up here.
        let body = ok(
            case,
            &router,
            "CreateRouteServer",
            &[param("AmazonSideAsn", "64513")],
        )
        .await;
        let untouched = reported(case, &body, "routeServerId");

        let body = ok(case, &router, "CreateRouteServer", &create_params).await;
        let target = reported(case, &body, "routeServerId");

        // The delete answers the route server it took down, in the non-terminal `deleting`
        // state — the span is stored rather than slept for, so a caller reading straight
        // after the call still sees it going down.
        let body = ok(
            case,
            &router,
            "DeleteRouteServer",
            &[param("RouteServerId", &target)],
        )
        .await;
        let answered = reported(case, &body, "state");
        assert_eq!(
            answered, ROUTE_SERVER_STATE_DELETING,
            "case {case:?}: expected the delete of {target} to answer \
             {ROUTE_SERVER_STATE_DELETING:?}, got {answered:?}"
        );
        let described = described_state(case, &router, &target).await;
        assert_eq!(
            described, ROUTE_SERVER_STATE_DELETING,
            "case {case:?}: expected the describe of {target} to report \
             {ROUTE_SERVER_STATE_DELETING:?} straight after the delete, got {described:?}"
        );

        // The terminal state is owed to the clock, and nothing else moves with it.
        past_the_transition().await;
        let described = described_state(case, &router, &target).await;
        assert_eq!(
            described, ROUTE_SERVER_STATE_DELETED,
            "case {case:?}: expected the describe of {target} to report \
             {ROUTE_SERVER_STATE_DELETED:?} once the move fell due, got {described:?}"
        );
        let untouched_state = described_state(case, &router, &untouched).await;
        assert_eq!(
            untouched_state, ROUTE_SERVER_STATE_AVAILABLE,
            "case {case:?}: expected the route server nothing deleted to stay \
             {ROUTE_SERVER_STATE_AVAILABLE:?}, got {untouched_state:?}"
        );

        // The unfiltered describe reports it too, so a caller that lists route servers sees
        // the delete rather than a row that vanished.
        let body = ok(case, &router, "DescribeRouteServers", &[]).await;
        let listed = elements(&body, "routeServerId");
        assert!(
            listed.contains(&target) && listed.contains(&untouched),
            "case {case:?}: expected the unfiltered describe to list {target} and {untouched}, \
             got {listed:?}"
        );
    }
}

/// Nothing ACTS on a route server a delete took down: the state it would have to change is
/// the route server's own, which is what EC2's `IncorrectState` names — one answer for the
/// second delete, the modify, the association and the endpoint create alike. The route
/// server the account still holds admits each of those calls, so the refusal follows from the
/// state of the record rather than from the operation.
#[tokio::test]
async fn no_operation_acts_on_a_route_server_a_delete_took_down() {
    let case = "a deleted route server";
    let (router, _engine) = super::ec2_query::emulator();

    // A VPC and a subnet, for the two operations that name one beside the route server.
    let body = ok(case, &router, "CreateVpc", &[param("CidrBlock", "10.9.0.0/16")]).await;
    let vpc = reported(case, &body, "vpcId");
    let body = ok(
        case,
        &router,
        "CreateSubnet",
        &[param("VpcId", &vpc), param("CidrBlock", "10.9.1.0/24")],
    )
    .await;
    let subnet = reported(case, &body, "subnetId");

    let body = ok(
        case,
        &router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", "64512")],
    )
    .await;
    let target = reported(case, &body, "routeServerId");
    ok(
        case,
        &router,
        "DeleteRouteServer",
        &[param("RouteServerId", &target)],
    )
    .await;
    past_the_transition().await;

    // (the operation, the parameters it names beside the route server)
    let acting: Vec<(&str, Vec<Param>)> = vec![
        ("DeleteRouteServer", Vec::new()),
        (
            "ModifyRouteServer",
            vec![param("PersistRoutes", "enable"), param("PersistRoutesDuration", "1")],
        ),
        ("AssociateRouteServer", vec![param("VpcId", &vpc)]),
        ("CreateRouteServerEndpoint", vec![param("SubnetId", &subnet)]),
    ];
    let message = format!(
        "{ROUTE_SERVER_TAGGABLE_RESOURCE} '{target}' is in the '{ROUTE_SERVER_STATE_DELETED}' \
         state."
    );
    for (action, extra) in acting {
        let mut params = vec![param("RouteServerId", &target)];
        params.extend(extra);
        refused(
            case,
            &router,
            action,
            &params,
            400,
            INCORRECT_STATE,
            &message,
        )
        .await;
    }

    // The route server the account still holds admits every one of those calls, so none of
    // the refusals above was the operation refusing whatever it was given.
    let body = ok(
        case,
        &router,
        "CreateRouteServer",
        &[param("AmazonSideAsn", "64514")],
    )
    .await;
    let live = reported(case, &body, "routeServerId");
    past_the_transition().await;
    for (action, extra) in [
        (
            "ModifyRouteServer",
            vec![param("PersistRoutes", "enable"), param("PersistRoutesDuration", "1")],
        ),
        ("AssociateRouteServer", vec![param("VpcId", &vpc)]),
        ("CreateRouteServerEndpoint", vec![param("SubnetId", &subnet)]),
    ] {
        let mut params = vec![param("RouteServerId", &live)];
        params.extend(extra);
        ok(case, &router, action, &params).await;
    }
}
