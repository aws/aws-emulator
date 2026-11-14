// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the transit gateway prefix list reference family:
//! `CreateTransitGatewayPrefixListReference` and the
//! `GetTransitGatewayPrefixListReferences` that reads it back, driven over `ec2Query`
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a prefix list reference is a ROUTE of one transit gateway route
//! table, and a route table has at most one route per prefix list — so a create stores one
//! record per (route table, prefix list) pair, answers it in the non-terminal `pending`
//! state with the clock owing it `available`, fills its owner, state and attachment
//! information from the STORE rather than from the request, and a second create for the same
//! pair is refused and writes nothing. Every id a request names is resolved first, so one of
//! the wrong form is refused as malformed and one that resolves to nothing as not found, each
//! with that resource's own code; a request that says neither where matching traffic goes nor
//! that it is dropped, or says both, is refused as well.
//!
//! Each table row is a distinct input along what that rule generalises over: whether the
//! route forwards or blackholes, which kind of attachment it forwards to (and so which
//! `ResourceType` and `ResourceId` the reference reports), which published id form the prefix
//! list carries, which id of the request is the unresolvable one, and which documented filter
//! the reader is narrowed by.

use std::time::Duration;

use ec2::ops::types::{
    PeeringTgwInfo, TransitGatewayClientVpnAttachment, TransitGatewayPeeringAttachment,
    TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
    PREFIX_LIST_REFERENCE_STATE_PENDING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters, unescaped};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// `MissingParameter`, the code EC2 answers for a request that left out a parameter the
/// operation needs (`crate::scaffolded::Scaffolded::missing_parameter`).
const MISSING_PARAMETER: &str = "MissingParameter";

/// `InvalidParameterCombination`, the code EC2 answers for members that cannot hold together
/// (`crate::scaffolded::Scaffolded::invalid_parameter_combination`).
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// `InvalidParameterValue`, the code EC2 answers for a filter name no operation documents.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// Long enough past [`TRANSITION_DELAY`] that a move the clock owes is certainly due.
/// Long enough past the span a clock-owed move takes that it is certainly overdue: the
/// deadline the polls below give up at, generous because what is under test is the state a
/// reference reaches rather than how fast it reaches it.
fn move_deadline() -> Duration {
    Duration::from_secs(60)
}

/// How long a poll below waits between two reads of a reference's state.
fn poll_interval() -> Duration {
    Duration::from_millis(50)
}

/// Wait until every reference of `route_table_id` is reported in `state`.
///
/// Polls for that STATE rather than sleeping for the span the clock owes the move, so a
/// loaded machine cannot read a reference before the move has been applied.
async fn wait_until_every_reference_is(
    router: &MultiServiceRouter,
    route_table_id: &str,
    state: &str,
) {
    let deadline = tokio::time::Instant::now() + move_deadline();
    loop {
        let body = reported(router, route_table_id, &[]).await;
        let states = elements(&body, "state");
        if !states.is_empty() && states.iter().all(|reported| reported == state) {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "every reference of {route_table_id} must reach {state:?}, last read {states:?}"
        );
        tokio::time::sleep(poll_interval()).await;
    }
}

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as the id of the default ASSOCIATION route
/// table the create owes it ("we create a default transit gateway route table and use it as
/// the default association route table" — `CreateTransitGateway`).
async fn route_table(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has an association route table")
}

/// One kind of attachment a reference can route to, with the `ResourceType` and `ResourceId`
/// the reference then reports and the stored attachment it is seeded as.
struct Kind {
    /// The kind, as part of the case name.
    name: &'static str,
    /// The attachment's `TransitGatewayAttachmentResourceType`.
    resource_type: &'static str,
    /// The id of the resource the attachment attaches.
    resource_id: &'static str,
    /// An available attachment of this kind, as it is stored.
    view: fn() -> TransitGatewayAttachmentView,
}

/// A VPC attachment in service.
fn available_vpc_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Vpc(TransitGatewayVpcAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        vpc_id: Some("vpc-0123456789abcdef0".to_string()),
        vpc_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        subnet_ids: Some(vec!["subnet-0123456789abcdef0".to_string()]),
        ..Default::default()
    })
}

/// A transit gateway peering attachment in service. Its `ResourceId` is the accepter gateway,
/// which is what a report of the attachment names.
fn available_peering_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::Peering(TransitGatewayPeeringAttachment {
        requester_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some(REGION.to_string()),
            ..Default::default()
        }),
        accepter_tgw_info: Some(PeeringTgwInfo {
            transit_gateway_id: Some("tgw-00000000000000aaa".to_string()),
            owner_id: Some("210987654321".to_string()),
            region: Some("us-west-2".to_string()),
            ..Default::default()
        }),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// A Client VPN attachment in service.
fn available_client_vpn_attachment() -> TransitGatewayAttachmentView {
    TransitGatewayAttachmentView::ClientVpn(TransitGatewayClientVpnAttachment {
        transit_gateway_id: Some("tgw-0123456789abcdef0".to_string()),
        client_vpn_endpoint_id: Some("cvpn-endpoint-0123456789abcdef0".to_string()),
        client_vpn_owner_id: Some("210987654321".to_string()),
        state: Some(TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE.to_string()),
        ..Default::default()
    })
}

/// The attachment kinds a reference can route to, one row per `ResourceType` this crate
/// stores.
fn kinds() -> Vec<Kind> {
    vec![
        Kind {
            name: "a VPC attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
            resource_id: "vpc-0123456789abcdef0",
            view: available_vpc_attachment,
        },
        Kind {
            name: "a peering attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING,
            resource_id: "tgw-00000000000000aaa",
            view: available_peering_attachment,
        },
        Kind {
            name: "a Client VPN attachment",
            resource_type: TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
            resource_id: "cvpn-endpoint-0123456789abcdef0",
            view: available_client_vpn_attachment,
        },
    ]
}

/// Store an available attachment of `kind` and answer its `TransitGatewayAttachmentId`.
fn seed_attachment(engine: &engine::InMemoryEngine, kind: &Kind) -> String {
    engine
        .state::<Ec2State>()
        .lock()
        .insert_transit_gateway_attachment((kind.view)())
        .expect("the store holds the seeded attachment")
}

/// Store a managed prefix list under `prefix_list_id`, so a reference created FOR it names a
/// prefix list the account holds.
///
/// A reference routes exactly the ranges its prefix list holds, so the create resolves the
/// `PrefixListId` against the account's own lists and the Region's published ones. Which list
/// it is has no part in any rule here — the rows are about the ROUTE — so one is seeded at
/// each id the table names rather than every row being rewritten around the ids a create
/// mints.
fn seed_prefix_list(engine: &engine::InMemoryEngine, prefix_list_id: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard.managed_prefix_lists.insert(
        prefix_list_id.to_string(),
        ec2::state::ManagedPrefixListRecord {
            view: ec2::ops::types::ManagedPrefixList {
                prefix_list_id: Some(prefix_list_id.to_string()),
                address_family: Some("IPv4".to_string()),
                state: Some("create-complete".to_string()),
                prefix_list_name: Some(format!("seeded-{prefix_list_id}")),
                max_entries: Some(5),
                version: Some(1),
                owner_id: Some(engine::emulator_account_id().to_string()),
                ..Default::default()
            },
            entries: Vec::new(),
            pending: None,
        },
    );
}

/// Every prefix list id the rows below name, so each create resolves the list it routes.
const SEEDED_PREFIX_LISTS: [&str; 11] = [
    "pl-00000000000000aa1",
    "pl-0000aa12",
    "pl-00000000000000bb2",
    "pl-00000000000000cc3",
    "pl-0000dd45",
    "pl-0000000000000000a",
    "pl-0000000000000000b",
    "pl-0000000000000000c",
    "pl-0000000000000000d",
    "pl-0000000000000000e",
    "pl-0000000000000000f",
];

/// Seed [`SEEDED_PREFIX_LISTS`] into a fresh store.
fn seed_prefix_lists(engine: &engine::InMemoryEngine) {
    for prefix_list_id in SEEDED_PREFIX_LISTS {
        seed_prefix_list(engine, prefix_list_id);
    }
}

/// The state the store holds for one (route table, prefix list) pair, or `None` when it holds
/// no reference for it at all.
fn stored_state(
    engine: &engine::InMemoryEngine,
    route_table_id: &str,
    prefix_list_id: &str,
) -> Option<String> {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_prefix_list_references
        .get(&(route_table_id.to_string(), prefix_list_id.to_string()))
        .and_then(|record| record.view.state.clone())
}

/// `GetTransitGatewayPrefixListReferences` for `route_table_id`, narrowed by `narrow_by`, as
/// the response body of a call that must succeed.
async fn reported(
    router: &MultiServiceRouter,
    route_table_id: &str,
    narrow_by: &[(&str, &[&str])],
) -> String {
    let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
    params.extend(filters(narrow_by));
    let (status, body) =
        invoke(router, "GetTransitGatewayPrefixListReferences", &params).await;
    assert_eq!(
        status, 200,
        "GetTransitGatewayPrefixListReferences must succeed, got {status} with body {body}"
    );
    body
}

/// Where the traffic a reference matches goes, which is what one row varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `Blackhole`: the traffic is dropped, and the reference names no attachment.
    Blackhole,
    /// `TransitGatewayAttachmentId`: the traffic is routed to the attachment at this index
    /// of [`kinds`].
    Attachment(usize),
}

/// One create, as a row of the table: the prefix list the route is for and what the route
/// does with the traffic it matches.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The `PrefixListId` the reference is created for.
    prefix_list_id: &'static str,
    /// Where matching traffic goes.
    target: Target,
}

/// Creating a reference stores one route of the named route table and answers it as stored:
/// `pending`, owned by the account this emulator serves, carrying the stored attachment's own
/// resource when it forwards and no attachment at all when it blackholes. The reader reports
/// exactly that, the clock then owes the reference `available`, and a second create for the
/// same (route table, prefix list) pair is refused and writes nothing.
#[tokio::test]
async fn creating_a_prefix_list_reference_stores_one_route_and_reads_it_back() {
    // The two published id forms of an EC2 resource id (8 or 17 lowercase hexadecimal
    // digits, `crate::state::prefixed_id_is_well_formed`) are both exercised, and neither
    // prefix list id is the one the issue's recording named.
    let cases = [
        Case {
            name: "a blackhole route for a long-form prefix list",
            prefix_list_id: "pl-00000000000000aa1",
            target: Target::Blackhole,
        },
        Case {
            name: "a blackhole route for a short-form prefix list",
            prefix_list_id: "pl-0000aa12",
            target: Target::Blackhole,
        },
        Case {
            name: "a route to a VPC attachment",
            prefix_list_id: "pl-00000000000000bb2",
            target: Target::Attachment(0),
        },
        Case {
            name: "a route to a peering attachment",
            prefix_list_id: "pl-00000000000000cc3",
            target: Target::Attachment(1),
        },
        Case {
            name: "a route to a Client VPN attachment",
            prefix_list_id: "pl-0000dd45",
            target: Target::Attachment(2),
        },
    ];

    for case in cases {
        let Case {
            name,
            prefix_list_id,
            target,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        seed_prefix_lists(&engine);
        let route_table_id = route_table(&router).await;
        let kinds = kinds();
        let attachment = match target {
            Target::Blackhole => None,
            Target::Attachment(index) => {
                let kind = &kinds[index];
                Some((kind, seed_attachment(&engine, kind)))
            }
        };
        let case = match attachment.as_ref() {
            None => name.to_string(),
            Some((kind, _)) => format!("{name} ({})", kind.name),
        };

        let mut request = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PrefixListId", prefix_list_id),
        ];
        match attachment.as_ref() {
            None => request.push(param("Blackhole", "true")),
            Some((_, attachment_id)) => {
                request.push(param("TransitGatewayAttachmentId", attachment_id));
            }
        }

        let (status, body) =
            invoke(&router, "CreateTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "prefixListId").as_deref(),
            Some(prefix_list_id),
            "case {case:?}: expected the prefix list the request named, got {body}"
        );
        assert_eq!(
            element(&body, "transitGatewayRouteTableId").as_deref(),
            Some(route_table_id.as_str()),
            "case {case:?}: expected the route table the request named, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_PENDING),
            "case {case:?}: expected the non-terminal state of a create, got {body}"
        );
        assert_eq!(
            element(&body, "prefixListOwnerId").as_deref(),
            Some(engine::emulator_account_id()),
            "case {case:?}: expected the account this emulator serves, got {body}"
        );
        assert_eq!(
            element(&body, "blackhole").as_deref(),
            Some(if target == Target::Blackhole {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: expected the route's own blackhole flag, got {body}"
        );
        match attachment.as_ref() {
            None => assert_eq!(
                elements(&body, "transitGatewayAttachment"),
                Vec::<String>::new(),
                "case {case:?}: a blackhole route routes to no attachment, got {body}"
            ),
            Some((kind, attachment_id)) => {
                assert_eq!(
                    element(&body, "transitGatewayAttachmentId").as_deref(),
                    Some(attachment_id.as_str()),
                    "case {case:?}: expected the attachment traffic is routed to, got {body}"
                );
                assert_eq!(
                    element(&body, "resourceType").as_deref(),
                    Some(kind.resource_type),
                    "case {case:?}: expected the stored attachment's resource type, got \
                     {body}"
                );
                assert_eq!(
                    element(&body, "resourceId").as_deref(),
                    Some(kind.resource_id),
                    "case {case:?}: expected the stored attachment's resource id, got {body}"
                );
            }
        }
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id).as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_PENDING),
            "case {case:?}: the store must hold what the response reported"
        );

        // The round trip: the reader answers the one stored record, not a second copy of it.
        let body = reported(
            &router,
            &route_table_id,
            &[("prefix-list-id", &[prefix_list_id])],
        )
        .await;
        assert_eq!(
            elements(&body, "prefixListId"),
            vec![prefix_list_id.to_string()],
            "case {case:?}: the reference must be reported exactly once, got {body}"
        );
        assert_eq!(
            elements(&body, "state"),
            vec![PREFIX_LIST_REFERENCE_STATE_PENDING.to_string()],
            "case {case:?}: the reader must report the state the create answered with"
        );

        // A second create for the same pair has nothing to create: a route table carries at
        // most one route per prefix list.
        let (status, body) =
            invoke(&router, "CreateTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second create must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ec2::state::INCORRECT_STATE,
            "case {case:?}: expected IncorrectState, got {body}"
        );

        // The move the clock owed falls due on its own, without anybody asking for it — read
        // until it has rather than after a span of this test's choosing.
        wait_until_every_reference_is(
            &router,
            &route_table_id,
            PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
        )
        .await;
        let body = reported(
            &router,
            &route_table_id,
            &[("prefix-list-id", &[prefix_list_id])],
        )
        .await;
        assert_eq!(
            elements(&body, "state"),
            vec![PREFIX_LIST_REFERENCE_STATE_AVAILABLE.to_string()],
            "case {case:?}: the clock owed this reference its available state"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id).as_deref(),
            Some(PREFIX_LIST_REFERENCE_STATE_AVAILABLE),
            "case {case:?}: the due move must be applied to the stored record too"
        );
    }
}

/// Every id a prefix list reference request names is resolved against the store, so an id of
/// the wrong form is refused as malformed and an id that resolves to nothing as not found —
/// each with that resource's own code, and never as a 501 or an empty success. A request that
/// has not said what the route does with matching traffic, or has said two different things,
/// is refused as well.
///
/// The rows are one per id of the request and per way it can fail to resolve, plus one per
/// contradictory target; the route table rows are asserted for the READER too, because it
/// resolves that id the same way.
#[tokio::test]
async fn a_request_whose_ids_or_target_do_not_resolve_is_refused_with_its_own_code() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;
    let attachment_id = seed_attachment(&engine, &kinds()[0]);

    // (case, the request's fields, the code expected)
    type UnresolvableCase = (&'static str, Vec<(String, String)>, &'static str);
    let cases: Vec<UnresolvableCase> = vec![
        (
            "a route table id of the right form that names no route table",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0000000000000000f"),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a short-form route table id that names no route table",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0000000f"),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table id that is not an id of that kind",
            vec![
                param("TransitGatewayRouteTableId", "tgw-rtb-0123456789abcde"),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "an attachment id where a route table id belongs",
            vec![
                param("TransitGatewayRouteTableId", &attachment_id),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
        (
            "a prefix list id that is not an id of that kind",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-not-an-id"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_PREFIX_LIST_ID_MALFORMED,
        ),
        (
            "a prefix list id carrying a digit outside the id alphabet",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-0000000g"),
                param("Blackhole", "true"),
            ],
            ec2::state::INVALID_PREFIX_LIST_ID_MALFORMED,
        ),
        (
            "an attachment id that is not an id of that kind",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("TransitGatewayAttachmentId", "tgw-attach-nope"),
            ],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_MALFORMED,
        ),
        (
            "an attachment id of the right form that names no attachment",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("TransitGatewayAttachmentId", "tgw-attach-0000000000000000f"),
            ],
            ec2::state::INVALID_TRANSIT_GATEWAY_ATTACHMENT_ID_NOT_FOUND,
        ),
        (
            "neither an attachment nor a blackhole",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-00000000000000aa1"),
            ],
            MISSING_PARAMETER,
        ),
        (
            "a blackhole that also names an attachment",
            vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", "pl-00000000000000aa1"),
                param("TransitGatewayAttachmentId", &attachment_id),
                param("Blackhole", "true"),
            ],
            INVALID_PARAMETER_COMBINATION,
        ),
    ];

    for (name, request, expected) in &cases {
        let case = format!("CreateTransitGatewayPrefixListReference with {name}");
        let (status, body) = invoke(&router, "CreateTransitGatewayPrefixListReference", request)
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            &error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, "pl-00000000000000aa1"),
            None,
            "case {case:?}: a refused create must write nothing"
        );
    }

    // The reader resolves the route table the same way: a route table the account does not
    // hold is an error, never an empty page.
    for (name, id, expected) in [
        (
            "a route table id that names no route table",
            "tgw-rtb-0000000000000000f",
            ec2::state::INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        ),
        (
            "a route table id that is not an id of that kind",
            "tgw-rtb-0123456789abcde",
            ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
        ),
    ] {
        let case = format!("GetTransitGatewayPrefixListReferences with {name}");
        let (status, body) = invoke(
            &router,
            "GetTransitGatewayPrefixListReferences",
            &[param("TransitGatewayRouteTableId", id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected,
            "case {case:?}: expected {expected}, got {body}"
        );
    }
}

/// A malformed transit gateway route table id is refused with a message that NAMES the
/// resource and the id the request gave — the rule a refusal built from the request follows,
/// shared by every operation that resolves a `TransitGatewayRouteTableId` rather than written
/// at one call site.
///
/// The rows are one per way an id can be the wrong form (a hex suffix of neither published
/// length, a suffix that is not hex at all, an id of another resource) crossed with the
/// operations that resolve one.
#[tokio::test]
async fn a_malformed_route_table_id_is_named_in_the_refusal() {
    let (router, engine) = super::ec2_query::emulator();
    let attachment_id = seed_attachment(&engine, &kinds()[0]);

    for id in [
        "tgw-rtb-0123456789abcde",
        "tgw-rtb-not-an-id",
        "tgw-attach-0123456789abcdef0",
    ] {
        // (the action, the fields it needs beside the route table id)
        let requests: Vec<(&str, Vec<(String, String)>)> = vec![
            (
                "CreateTransitGatewayPrefixListReference",
                vec![
                    param("PrefixListId", "pl-00000000000000aa1"),
                    param("Blackhole", "true"),
                ],
            ),
            ("GetTransitGatewayPrefixListReferences", vec![]),
            (
                "EnableTransitGatewayRouteTablePropagation",
                vec![param("TransitGatewayAttachmentId", &attachment_id)],
            ),
            ("GetTransitGatewayRouteTablePropagations", vec![]),
        ];
        for (action, extra) in requests {
            let case = format!("{action} with the route table id {id:?}");
            let mut params = vec![param("TransitGatewayRouteTableId", id)];
            params.extend(extra);
            let (status, body) = invoke(&router, action, &params).await;
            assert_eq!(
                status, 400,
                "case {case:?}: expected HTTP 400, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED,
                "case {case:?}: expected {}, got {body}",
                ec2::state::INVALID_ROUTE_TABLE_ID_MALFORMED
            );
            assert_eq!(
                element(&body, "Message").map(|message| unescaped(&message)),
                Some(format!("Invalid Transit Gateway Route Table id {id}.")),
                "case {case:?}: the refusal must name the resource and the id, got {body}"
            );
        }
    }
}

/// A dry run of a request that WOULD have succeeded performs nothing: no reference is stored,
/// and the call that follows is the one that creates it.
#[tokio::test]
async fn a_dry_run_creates_no_reference() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;
    let attachment_id = seed_attachment(&engine, &kinds()[0]);

    // (case, the fields that say what the route does)
    for (case, target) in [
        ("a blackhole route", param("Blackhole", "true")),
        (
            "a route to an attachment",
            param("TransitGatewayAttachmentId", &attachment_id),
        ),
    ] {
        let prefix_list_id = match target.0.as_str() {
            "Blackhole" => "pl-00000000000000aa1",
            _ => "pl-00000000000000bb2",
        };
        let request = vec![
            param("TransitGatewayRouteTableId", &route_table_id),
            param("PrefixListId", prefix_list_id),
            target,
        ];
        let dry_run = {
            let mut dry_run = request.clone();
            dry_run.push(param("DryRun", "true"));
            dry_run
        };

        let (status, body) =
            invoke(&router, "CreateTransitGatewayPrefixListReference", &dry_run).await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS}, got {status} with body {body}"
        );
        assert_eq!(
            stored_state(&engine, &route_table_id, prefix_list_id),
            None,
            "case {case:?}: a dry run must create no reference"
        );
        let (status, body) =
            invoke(&router, "CreateTransitGatewayPrefixListReference", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 after a dry run, got {status} with body {body}"
        );
        assert!(
            stored_state(&engine, &route_table_id, prefix_list_id).is_some(),
            "case {case:?}: the call after the dry run is the one that creates the reference"
        );
    }
}

/// Every filter the reader documents selects the references whose own attribute it names, and
/// a filter name it does not document is refused rather than read as "no results".
///
/// The filter catalog comes from `GetTransitGatewayPrefixListReferencesRequest$Filters`:
/// `attachment.resource-id`, `attachment.resource-type`,
/// `attachment.transit-gateway-attachment-id`, `is-blackhole`, `prefix-list-id`,
/// `prefix-list-owner-id` and `state` —
/// `smithy_query(query="doc",shape="GetTransitGatewayPrefixListReferencesRequest",member="Filters",model_path="services/service-com-amazonaws-ec2-amazonec2/smithy-model/com-amazonaws-ec2-amazonec2.json")`.
#[tokio::test]
async fn every_documented_filter_selects_the_references_it_names() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;
    let kind = &kinds()[0];
    let attachment_id = seed_attachment(&engine, kind);

    // One blackhole route and one that forwards, so every filter has a reference it selects
    // and one it does not.
    let blackholed = "pl-00000000000000aa1";
    let forwarded = "pl-00000000000000bb2";
    for (prefix_list_id, target) in [
        (blackholed, param("Blackhole", "true")),
        (
            forwarded,
            param("TransitGatewayAttachmentId", &attachment_id),
        ),
    ] {
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayPrefixListReference",
            &[
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", prefix_list_id),
                target,
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the seeding create for {prefix_list_id} must succeed, got {status} with body \
             {body}"
        );
    }

    // Past the move the clock owed, so the `state` rows below name a settled state rather
    // than racing the transition — read until every reference has reached it.
    wait_until_every_reference_is(
        &router,
        &route_table_id,
        PREFIX_LIST_REFERENCE_STATE_AVAILABLE,
    )
    .await;

    let account = engine::emulator_account_id().to_string();
    // (case, the filters, the prefix list ids expected in report order)
    type FilterCase<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<&'a str>);
    let cases: Vec<FilterCase<'_>> = vec![
        ("no filter at all", vec![], vec![blackholed, forwarded]),
        (
            "prefix-list-id naming the forwarding route",
            vec![("prefix-list-id", vec![forwarded])],
            vec![forwarded],
        ),
        (
            "prefix-list-id naming both routes",
            vec![("prefix-list-id", vec![blackholed, forwarded])],
            vec![blackholed, forwarded],
        ),
        (
            "prefix-list-id naming a prefix list no route is for",
            vec![("prefix-list-id", vec!["pl-00000000000000cc3"])],
            vec![],
        ),
        (
            "prefix-list-owner-id naming this account",
            vec![("prefix-list-owner-id", vec![account.as_str()])],
            vec![blackholed, forwarded],
        ),
        (
            "prefix-list-owner-id naming another account",
            vec![("prefix-list-owner-id", vec!["210987654321"])],
            vec![],
        ),
        (
            "is-blackhole true",
            vec![("is-blackhole", vec!["true"])],
            vec![blackholed],
        ),
        (
            "is-blackhole false",
            vec![("is-blackhole", vec!["false"])],
            vec![forwarded],
        ),
        (
            "state available",
            vec![("state", vec![PREFIX_LIST_REFERENCE_STATE_AVAILABLE])],
            vec![blackholed, forwarded],
        ),
        (
            "state pending",
            vec![("state", vec![PREFIX_LIST_REFERENCE_STATE_PENDING])],
            vec![],
        ),
        (
            "attachment.transit-gateway-attachment-id",
            vec![(
                "attachment.transit-gateway-attachment-id",
                vec![attachment_id.as_str()],
            )],
            vec![forwarded],
        ),
        (
            "attachment.resource-type",
            vec![("attachment.resource-type", vec![kind.resource_type])],
            vec![forwarded],
        ),
        (
            "attachment.resource-id",
            vec![("attachment.resource-id", vec![kind.resource_id])],
            vec![forwarded],
        ),
        (
            "two filters, which are joined with an AND",
            vec![
                ("is-blackhole", vec!["true"]),
                ("prefix-list-id", vec![forwarded]),
            ],
            vec![],
        ),
    ];

    for (case, narrow_by, expected) in cases {
        let narrow_by: Vec<(&str, &[&str])> = narrow_by
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let body = reported(&router, &route_table_id, &narrow_by).await;
        let reported_ids = elements(&body, "prefixListId");
        let expected: Vec<String> = expected.iter().map(|id| (*id).to_string()).collect();
        assert_eq!(
            reported_ids, expected,
            "case {case:?}: expected {expected:?}, got {reported_ids:?} from {body}"
        );
    }

    // A filter name the operation does not document is refused, whether or not a reference
    // would have matched it.
    let (status, body) = invoke(
        &router,
        "GetTransitGatewayPrefixListReferences",
        &{
            let mut params = vec![param("TransitGatewayRouteTableId", &route_table_id)];
            params.extend(filters(&[("prefix-list-name", &["anything"])]));
            params
        },
    )
    .await;
    assert_eq!(
        status, 400,
        "an undocumented filter must be refused, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_PARAMETER_VALUE,
        "expected {INVALID_PARAMETER_VALUE}, got {body}"
    );
}

/// The reader pages in prefix-list-id order, minting a `NextToken` only while a reference
/// follows the page it answered — so a client walking the token sees every reference exactly
/// once, whatever page size it asked for.
#[tokio::test]
async fn the_reader_pages_the_references_in_prefix_list_order() {
    let (router, engine) = super::ec2_query::emulator();
    seed_prefix_lists(&engine);
    let route_table_id = route_table(&router).await;

    // More references than the smallest page the model's `TransitGatewayMaxResults`
    // (`range 5..1000`) admits, so a token is minted at that boundary.
    let prefix_lists = [
        "pl-0000000000000000a",
        "pl-0000000000000000b",
        "pl-0000000000000000c",
        "pl-0000000000000000d",
        "pl-0000000000000000e",
        "pl-0000000000000000f",
    ];
    for prefix_list_id in prefix_lists {
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayPrefixListReference",
            &[
                param("TransitGatewayRouteTableId", &route_table_id),
                param("PrefixListId", prefix_list_id),
                param("Blackhole", "true"),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "the seeding create for {prefix_list_id} must succeed, got {status} with body \
             {body}"
        );
        assert!(
            stored_state(&engine, &route_table_id, prefix_list_id).is_some(),
            "the store must hold the reference created for {prefix_list_id}"
        );
    }

    // (case, the page size asked for, the pages expected)
    let cases: Vec<(&str, usize, Vec<Vec<&str>>)> = vec![
        (
            "the smallest page the model admits",
            5,
            vec![prefix_lists[..5].to_vec(), prefix_lists[5..].to_vec()],
        ),
        (
            "a page as large as the reference count",
            6,
            vec![prefix_lists.to_vec()],
        ),
        (
            "a page larger than the reference count",
            1000,
            vec![prefix_lists.to_vec()],
        ),
    ];

    for (case, max_results, expected_pages) in cases {
        let mut next_token: Option<String> = None;
        let mut pages: Vec<Vec<String>> = Vec::new();
        loop {
            let mut params = vec![
                param("TransitGatewayRouteTableId", &route_table_id),
                param("MaxResults", &max_results.to_string()),
            ];
            if let Some(token) = next_token.as_deref() {
                params.push(param("NextToken", token));
            }
            let (status, body) =
                invoke(&router, "GetTransitGatewayPrefixListReferences", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            pages.push(elements(&body, "prefixListId"));
            next_token = element(&body, "nextToken");
            if next_token.is_none() {
                break;
            }
            assert!(
                pages.len() <= expected_pages.len(),
                "case {case:?}: the token must stop advancing, got pages {pages:?}"
            );
        }
        let expected_pages: Vec<Vec<String>> = expected_pages
            .iter()
            .map(|page| page.iter().map(|id| (*id).to_string()).collect())
            .collect();
        assert_eq!(
            pages, expected_pages,
            "case {case:?}: expected {expected_pages:?}, got {pages:?}"
        );
    }
}
