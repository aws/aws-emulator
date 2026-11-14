// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the static routes of a transit gateway route table:
//! `CreateTransitGatewayRoute` and the `SearchTransitGatewayRoutes` that reads them back,
//! driven over `ec2Query` (README.md / OPERATIONS.md "Wire protocol").
//!
//! The rule under test: a static route is named by the CANONICAL address prefix it matches
//! inside ONE transit gateway route table, so a create stores one record per (route table,
//! destination) and answers it as stored — type `static`, state `blackhole` when the request
//! asks for the matching traffic to be dropped and `active` with the STORED attachment's own
//! resource and resource type when it forwards — and a second create for that destination,
//! however it is spelled, is refused and writes nothing. The search of a table reports exactly
//! the routes that table holds, in destination order, narrowed by the filter names the
//! operation documents and by nothing else, and the routes go when the table they are in does.
//!
//! Each table row is a distinct input along what that rule generalises over: the address family
//! and prefix length of the destination, whether the destination is written in its canonical
//! form, whether the route forwards or blackholes, which kind of attachment it forwards to (and
//! so which `ResourceType` and `ResourceId` the route reports), which documented filter the
//! reader is narrowed by and what that filter's values relate to, and which member of the
//! request is the one at fault.

use ec2::ops::types::{
    PeeringTgwInfo, TransitGatewayClientVpnAttachment, TransitGatewayPeeringAttachment,
    TransitGatewayVpcAttachment,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_CLIENT_VPN,
    TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_PEERING, TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC,
    TRANSIT_GATEWAY_ATTACHMENT_STATE_AVAILABLE, TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE,
    TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE, TRANSIT_GATEWAY_ROUTE_TYPE_STATIC,
    TransitGatewayAttachmentView,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, filters};
use super::wire::param;

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The HTTP status EC2 answers `DryRunOperation` with.
const DRY_RUN_STATUS: u16 = 412;

/// `MissingParameter`, the code EC2 answers a request that left out a parameter the operation
/// needs.
const MISSING_PARAMETER: &str = "MissingParameter";

/// `InvalidParameterValue`, the code EC2 answers a parameter value it cannot accept — a filter
/// name no operation documents among them.
const INVALID_PARAMETER_VALUE: &str = "InvalidParameterValue";

/// `InvalidParameterCombination`, the code EC2 answers members that cannot hold together.
const INVALID_PARAMETER_COMBINATION: &str = "InvalidParameterCombination";

/// `MalformedInput`, the code the request boundary answers a `@required` member the request did
/// not carry at all — `Filters` for a search of the routes, which is a list, so a request
/// naming no filter carries no field of it and the member arrives null.
const MALFORMED_INPUT: &str = "MalformedInput";

/// `RouteAlreadyExists`, the code EC2 answers a destination the route table already holds a
/// route for.
const ROUTE_ALREADY_EXISTS: &str = "RouteAlreadyExists";

/// `InvalidRouteTableId.Malformed`, the code EC2 answers something that is not a transit
/// gateway route table id at all.
const INVALID_ROUTE_TABLE_ID_MALFORMED: &str = "InvalidRouteTableId.Malformed";

/// `InvalidRouteTableID.NotFound`, the code EC2 answers a route table id the account holds no
/// transit gateway route table for.
const INVALID_ROUTE_TABLE_ID_NOT_FOUND: &str = "InvalidRouteTableID.NotFound";

/// `InvalidTransitGatewayAttachmentID.NotFound`, the code EC2 answers an attachment id the
/// account holds no attachment for.
const INVALID_ATTACHMENT_ID_NOT_FOUND: &str = "InvalidTransitGatewayAttachmentID.NotFound";

/// One call of `action` signed for the Region above.
async fn invoke(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// A transit gateway created through the wire, as the id of the default ASSOCIATION route table
/// the create owes it ("we create a default transit gateway route table and use it as the
/// default association route table" — `CreateTransitGateway`).
async fn route_table(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    element(&body, "associationDefaultRouteTableId")
        .expect("a gateway created with the documented defaults has an association route table")
}

/// An ADDITIONAL route table of a freshly created transit gateway ("You can use
/// CreateTransitGatewayRouteTable to create additional transit gateway route tables" —
/// `CreateTransitGateway`), as its `TransitGatewayRouteTableId`.
///
/// Neither of the gateway's DEFAULT tables, so it is one a delete may take away.
async fn additional_route_table(router: &MultiServiceRouter) -> String {
    let (status, body) = invoke(router, "CreateTransitGateway", &[]).await;
    assert_eq!(
        status, 200,
        "CreateTransitGateway must succeed, got {status} with body {body}"
    );
    let transit_gateway_id =
        element(&body, "transitGatewayId").expect("a created gateway reports its own id");
    let (status, body) = invoke(
        router,
        "CreateTransitGatewayRouteTable",
        &[param("TransitGatewayId", &transit_gateway_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "CreateTransitGatewayRouteTable must succeed, got {status} with body {body}"
    );
    element(&body, "transitGatewayRouteTableId").expect("a created route table reports its own id")
}

/// One kind of attachment a route can forward to, with the `ResourceType` and `ResourceId` the
/// route then reports and the stored attachment it is seeded as.
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

/// The attachment kinds a route can forward to, one row per `ResourceType` this crate stores.
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

/// How many routes the store holds for `route_table_id`.
fn stored_routes(engine: &engine::InMemoryEngine, route_table_id: &str) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .transit_gateway_route_count(route_table_id)
}

/// `CreateTransitGatewayRoute` for `destination` in `route_table_id`, forwarding to
/// `attachment_id` or dropping the traffic when it is `None`.
fn create_request(
    route_table_id: &str,
    destination: &str,
    attachment_id: Option<&str>,
) -> Vec<(String, String)> {
    let mut request = vec![
        param("TransitGatewayRouteTableId", route_table_id),
        param("DestinationCidrBlock", destination),
    ];
    match attachment_id {
        None => request.push(param("Blackhole", "true")),
        Some(attachment_id) => request.push(param("TransitGatewayAttachmentId", attachment_id)),
    }
    request
}

/// `SearchTransitGatewayRoutes` for `route_table_id` narrowed by `narrow_by`, as the body of a
/// call that must succeed.
async fn searched(
    router: &MultiServiceRouter,
    route_table_id: &str,
    narrow_by: &[(&str, &[&str])],
) -> String {
    let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
    params.extend(filters(narrow_by));
    let (status, body) = invoke(router, "SearchTransitGatewayRoutes", &params).await;
    assert_eq!(
        status, 200,
        "SearchTransitGatewayRoutes must succeed, got {status} with body {body}"
    );
    body
}

/// Every route a search body reports, as its `destinationCidrBlock`, in the order reported.
fn reported_destinations(body: &str) -> Vec<String> {
    elements(body, "destinationCidrBlock")
}

/// Where the traffic a route matches goes, which is what one row varies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Target {
    /// `Blackhole`: the traffic is dropped, and the route names no attachment.
    Blackhole,
    /// `TransitGatewayAttachmentId`: the traffic goes to the attachment at this index of
    /// [`kinds`].
    Attachment(usize),
}

/// One create, as a row of the table: the destination the request names, the canonical form of
/// the prefix that destination is, and what the route does with the traffic it matches.
struct Case {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The `DestinationCidrBlock` the request carries.
    destination: &'static str,
    /// The canonical form of that prefix, which is what names the route in its table.
    canonical: &'static str,
    /// Where matching traffic goes.
    target: Target,
}

/// Creating a route stores one route of the named route table and answers it as stored: the
/// canonical destination, the `static` type a create fixes, the terminal state its target
/// decides, and the stored attachment's own resource when it forwards. The search of the table
/// reports exactly that route back.
#[tokio::test]
async fn creating_a_route_stores_it_and_searches_it_back() {
    // Rows differ along the address family, the prefix length, whether the request spelled the
    // destination canonically, and where the matching traffic goes.
    let cases = [
        Case {
            name: "an IPv4 /16 to a VPC attachment",
            destination: "10.0.0.0/16",
            canonical: "10.0.0.0/16",
            target: Target::Attachment(0),
        },
        Case {
            name: "an IPv4 /24 to a peering attachment",
            destination: "192.168.7.0/24",
            canonical: "192.168.7.0/24",
            target: Target::Attachment(1),
        },
        Case {
            name: "an IPv4 /32 to a Client VPN attachment",
            destination: "203.0.113.9/32",
            canonical: "203.0.113.9/32",
            target: Target::Attachment(2),
        },
        Case {
            name: "an IPv4 prefix written with its host bits set",
            destination: "10.1.2.5/24",
            canonical: "10.1.2.0/24",
            target: Target::Attachment(0),
        },
        Case {
            name: "an IPv4 /8 dropped",
            destination: "172.0.0.0/8",
            canonical: "172.0.0.0/8",
            target: Target::Blackhole,
        },
        Case {
            name: "an IPv6 /64 dropped",
            destination: "2001:db8:1:2::/64",
            canonical: "2001:db8:1:2::/64",
            target: Target::Blackhole,
        },
        Case {
            name: "an IPv6 /56 to a VPC attachment",
            destination: "2001:db8:3::/56",
            canonical: "2001:db8:3::/56",
            target: Target::Attachment(0),
        },
    ];

    for case in cases {
        let Case {
            name,
            destination,
            canonical,
            target,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        let route_table_id = route_table(&router).await;
        let kinds = kinds();
        let forwards_to = match target {
            Target::Blackhole => None,
            Target::Attachment(index) => {
                let kind = &kinds[index];
                Some((kind, seed_attachment(&engine, kind)))
            }
        };
        let case = match forwards_to.as_ref() {
            None => name.to_string(),
            Some((kind, _)) => format!("{name} ({})", kind.name),
        };

        let request = create_request(
            &route_table_id,
            destination,
            forwards_to.as_ref().map(|(_, id)| id.as_str()),
        );
        let (status, body) = invoke(&router, "CreateTransitGatewayRoute", &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the create, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "destinationCidrBlock").as_deref(),
            Some(canonical),
            "case {case:?}: expected the canonical form of the destination, got {body}"
        );
        assert_eq!(
            element(&body, "type").as_deref(),
            Some(TRANSIT_GATEWAY_ROUTE_TYPE_STATIC),
            "case {case:?}: expected the type a create fixes, got {body}"
        );
        let expected_state = match target {
            Target::Blackhole => TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE,
            Target::Attachment(_) => TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE,
        };
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(expected_state),
            "case {case:?}: expected the state the route's target decides, got {body}"
        );
        match forwards_to.as_ref() {
            None => assert!(
                element(&body, "transitGatewayAttachmentId").is_none(),
                "case {case:?}: a dropped route reaches no attachment, got {body}"
            ),
            Some((kind, attachment_id)) => {
                assert_eq!(
                    element(&body, "transitGatewayAttachmentId").as_deref(),
                    Some(attachment_id.as_str()),
                    "case {case:?}: expected the attachment the request named, got {body}"
                );
                assert_eq!(
                    element(&body, "resourceId").as_deref(),
                    Some(kind.resource_id),
                    "case {case:?}: expected the stored attachment's own resource, got {body}"
                );
                assert_eq!(
                    element(&body, "resourceType").as_deref(),
                    Some(kind.resource_type),
                    "case {case:?}: expected the stored attachment's resource type, got {body}"
                );
            }
        }
        assert_eq!(
            stored_routes(&engine, &route_table_id),
            1,
            "case {case:?}: expected exactly one stored route"
        );

        // Read back through the search of the table it was created in: the route is reported
        // with the same members the create answered, and an exact-match search for the
        // canonical prefix reaches it however the request spelled it. (`Filters` is
        // `@required`, so every search below carries one; the type filter holds for every
        // route a create writes.)
        let body = searched(
            &router,
            &route_table_id,
            &[("type", &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC])],
        )
        .await;
        assert_eq!(
            reported_destinations(&body),
            vec![canonical.to_string()],
            "case {case:?}: expected the search to report the stored route, got {body}"
        );
        assert_eq!(
            element(&body, "state").as_deref(),
            Some(expected_state),
            "case {case:?}: expected the search to report the stored state, got {body}"
        );
        let body = searched(
            &router,
            &route_table_id,
            &[("route-search.exact-match", &[canonical])],
        )
        .await;
        assert_eq!(
            reported_destinations(&body),
            vec![canonical.to_string()],
            "case {case:?}: expected an exact-match search to reach the route, got {body}"
        );
    }
}

/// A route IS the destination it matches inside one table: a second create for that
/// destination, in any spelling of the prefix, is refused and leaves the stored route alone.
#[tokio::test]
async fn a_second_route_for_one_destination_is_refused() {
    // Rows differ along how the SECOND request spells the one prefix the first created.
    let cases = [
        ("the same spelling", "10.4.0.0/16"),
        ("a host address inside the prefix", "10.4.9.9/16"),
        ("the prefix with its host bits set", "10.4.255.255/16"),
    ];

    for (name, second_destination) in cases {
        let (router, engine) = super::ec2_query::emulator();
        let route_table_id = route_table(&router).await;
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayRoute",
            &create_request(&route_table_id, "10.4.0.0/16", None),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {name:?}: the first create must succeed, got {status} with body {body}"
        );

        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayRoute",
            &create_request(&route_table_id, second_destination, None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {name:?}: expected HTTP 400 for a destination already routed, got {status} \
             with body {body}"
        );
        assert_eq!(
            error_code(&body),
            ROUTE_ALREADY_EXISTS,
            "case {name:?}: expected the code for a route that is already there, got {body}"
        );
        assert_eq!(
            stored_routes(&engine, &route_table_id),
            1,
            "case {name:?}: expected the refused create to have written nothing"
        );
    }
}

/// A row of the filter table: the filter the search is narrowed by and the destinations it
/// selects out of the seeded table.
struct FilterCase {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The filter name, as `SearchTransitGatewayRoutesRequest$Filters` documents it.
    filter: &'static str,
    /// The filter's values.
    values: &'static [&'static str],
    /// The destinations the filter selects, in the destination order a search reports.
    expected: &'static [&'static str],
}

/// The search of a table reports the routes it holds in destination order, and each documented
/// filter narrows that set to the routes it names — the `route-search.*` ones by relating CIDR
/// blocks rather than by comparing text.
#[tokio::test]
async fn documented_filters_narrow_the_search() {
    // The seeded table, chosen so each relation has a distinct answer: the doc's own
    // supernet-of worked example ("if you have 10.0.1.0/29 and 10.0.1.0/31 routes in your route
    // table and you specify supernet-of-match as 10.0.1.0/30, then the result returns
    // 10.0.1.0/29") is two of the rows, and none of the destinations is the one the issue's
    // recording named.
    let cases = [
        FilterCase {
            name: "the state of a route that forwards",
            filter: "state",
            values: &[TRANSIT_GATEWAY_ROUTE_STATE_ACTIVE],
            expected: &["10.0.1.0/29", "10.0.1.0/31"],
        },
        FilterCase {
            name: "the state of a route that drops",
            filter: "state",
            values: &[TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE],
            expected: &["10.0.0.0/16", "2001:db8::/32"],
        },
        FilterCase {
            name: "the type every created route carries",
            filter: "type",
            values: &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC],
            expected: &[
                "10.0.0.0/16",
                "10.0.1.0/29",
                "10.0.1.0/31",
                "2001:db8::/32",
            ],
        },
        FilterCase {
            name: "a type no route of this table has",
            filter: "type",
            values: &["propagated"],
            expected: &[],
        },
        FilterCase {
            name: "the resource type of the attachment the routes forward to",
            filter: "attachment.resource-type",
            values: &[TRANSIT_GATEWAY_ATTACHMENT_RESOURCE_TYPE_VPC],
            expected: &["10.0.1.0/29", "10.0.1.0/31"],
        },
        FilterCase {
            name: "the resource the attachment attaches",
            filter: "attachment.resource-id",
            values: &["vpc-0123456789abcdef0"],
            expected: &["10.0.1.0/29", "10.0.1.0/31"],
        },
        FilterCase {
            name: "a resource no attachment of these routes attaches",
            filter: "attachment.resource-id",
            values: &["vpc-00000000000000fff"],
            expected: &[],
        },
        FilterCase {
            name: "the prefix list of a route created for a CIDR range",
            filter: "prefix-list-id",
            values: &["pl-0123456789abcdef0"],
            expected: &[],
        },
        FilterCase {
            name: "an exact match",
            filter: "route-search.exact-match",
            values: &["10.0.1.0/29"],
            expected: &["10.0.1.0/29"],
        },
        FilterCase {
            name: "an exact match written with its host bits set",
            filter: "route-search.exact-match",
            values: &["10.0.1.7/29"],
            expected: &["10.0.1.0/29"],
        },
        FilterCase {
            name: "two exact matches, joined with an OR",
            filter: "route-search.exact-match",
            values: &["10.0.1.0/29", "2001:db8::/32"],
            expected: &["10.0.1.0/29", "2001:db8::/32"],
        },
        FilterCase {
            name: "the routes that are a subnet of the value",
            filter: "route-search.subnet-of-match",
            values: &["10.0.0.0/16"],
            expected: &["10.0.0.0/16", "10.0.1.0/29", "10.0.1.0/31"],
        },
        FilterCase {
            name: "the doc's supernet-of worked example",
            filter: "route-search.supernet-of-match",
            values: &["10.0.1.0/30"],
            expected: &["10.0.0.0/16", "10.0.1.0/29"],
        },
        FilterCase {
            name: "the longest prefix matching an address of the table",
            filter: "route-search.longest-prefix-match",
            values: &["10.0.1.0/32"],
            expected: &["10.0.1.0/31"],
        },
        FilterCase {
            name: "the longest prefix matching an address only the widest route holds",
            filter: "route-search.longest-prefix-match",
            values: &["10.0.9.0/32"],
            expected: &["10.0.0.0/16"],
        },
    ];

    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = route_table(&router).await;
    let kinds = kinds();
    let attachment_id = seed_attachment(&engine, &kinds[0]);
    for (destination, forwards) in [
        ("10.0.0.0/16", false),
        ("10.0.1.0/29", true),
        ("10.0.1.0/31", true),
        ("2001:db8::/32", false),
    ] {
        let target = forwards.then_some(attachment_id.as_str());
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayRoute",
            &create_request(&route_table_id, destination, target),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding the route {destination} must succeed, got {status} with body {body}"
        );
    }

    // Narrowed only by the type every created route carries, the search reports every route of
    // the table in destination order.
    let body = searched(
        &router,
        &route_table_id,
        &[("type", &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC])],
    )
    .await;
    assert_eq!(
        reported_destinations(&body),
        vec![
            "10.0.0.0/16".to_string(),
            "10.0.1.0/29".to_string(),
            "10.0.1.0/31".to_string(),
            "2001:db8::/32".to_string(),
        ],
        "expected every seeded route in destination order, got {body}"
    );
    assert_eq!(
        element(&body, "additionalRoutesAvailable").as_deref(),
        Some("false"),
        "expected no further routes after a page that holds them all, got {body}"
    );

    for case in cases {
        let FilterCase {
            name,
            filter,
            values,
            expected,
        } = case;
        let body = searched(&router, &route_table_id, &[(filter, values)]).await;
        let reported = reported_destinations(&body);
        let expected: Vec<String> = expected.iter().map(|entry| (*entry).to_string()).collect();
        assert_eq!(
            reported, expected,
            "case {name:?}: expected the filter {filter:?} to select {expected:?}, got \
             {reported:?} from {body}"
        );
    }

    // "If you specify multiple filters, the filters are joined with an AND" (`Filter`).
    let body = searched(
        &router,
        &route_table_id,
        &[
            ("route-search.subnet-of-match", &["10.0.0.0/16"]),
            ("state", &[TRANSIT_GATEWAY_ROUTE_STATE_BLACKHOLE]),
        ],
    )
    .await;
    assert_eq!(
        reported_destinations(&body),
        vec!["10.0.0.0/16".to_string()],
        "expected two filters to be joined with an AND, got {body}"
    );
}

/// The search pages over the routes of one table in destination order: a page holds at most the
/// routes the request asked for, says that more follow, and the token it hands back starts the
/// next page at the next destination.
#[tokio::test]
async fn the_search_pages_over_the_routes_in_destination_order() {
    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = route_table(&router).await;
    let kinds = kinds();
    let attachment_id = seed_attachment(&engine, &kinds[0]);
    // Six destinations, so a page of five (the smallest `MaxResults` the model admits) leaves
    // exactly one route for the next page.
    let destinations = [
        "10.0.0.0/24",
        "10.0.1.0/24",
        "10.0.2.0/24",
        "10.0.3.0/24",
        "10.0.4.0/24",
        "10.0.5.0/24",
    ];
    for destination in destinations {
        let (status, body) = invoke(
            &router,
            "CreateTransitGatewayRoute",
            &create_request(&route_table_id, destination, Some(&attachment_id)),
        )
        .await;
        assert_eq!(
            status, 200,
            "seeding the route {destination} must succeed, got {status} with body {body}"
        );
    }

    let mut params = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("MaxResults", "5"),
    ];
    params.extend(filters(&[("type", &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC])]));
    let (status, body) = invoke(&router, "SearchTransitGatewayRoutes", &params).await;
    assert_eq!(
        status, 200,
        "the first page must succeed, got {status} with body {body}"
    );
    assert_eq!(
        reported_destinations(&body),
        destinations[..5]
            .iter()
            .map(|entry| (*entry).to_string())
            .collect::<Vec<String>>(),
        "expected the first five destinations, got {body}"
    );
    assert_eq!(
        element(&body, "additionalRoutesAvailable").as_deref(),
        Some("true"),
        "expected the page to say a route follows it, got {body}"
    );
    let next_token = element(&body, "nextToken").expect("a page with a route after it has a token");

    let mut params = vec![
        param("TransitGatewayRouteTableId", &route_table_id),
        param("MaxResults", "5"),
        param("NextToken", &next_token),
    ];
    params.extend(filters(&[("type", &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC])]));
    let (status, body) = invoke(&router, "SearchTransitGatewayRoutes", &params).await;
    assert_eq!(
        status, 200,
        "the second page must succeed, got {status} with body {body}"
    );
    assert_eq!(
        reported_destinations(&body),
        vec![destinations[5].to_string()],
        "expected the destination the token named, got {body}"
    );
    assert_eq!(
        element(&body, "additionalRoutesAvailable").as_deref(),
        Some("false"),
        "expected the last page to say nothing follows it, got {body}"
    );
    assert!(
        element(&body, "nextToken").is_none(),
        "expected no token on the last page, got {body}"
    );
}

/// A route is a resource OF its table, so the routes go when the table they are in does and the
/// search of that table is refused rather than reporting them.
#[tokio::test]
async fn deleting_the_route_table_takes_its_routes_with_it() {
    let (router, engine) = super::ec2_query::emulator();
    let route_table_id = additional_route_table(&router).await;
    let (status, body) = invoke(
        &router,
        "CreateTransitGatewayRoute",
        &create_request(&route_table_id, "10.7.0.0/16", None),
    )
    .await;
    assert_eq!(
        status, 200,
        "the create must succeed, got {status} with body {body}"
    );
    assert_eq!(
        stored_routes(&engine, &route_table_id),
        1,
        "expected the created route to be stored"
    );

    let (status, body) = invoke(
        &router,
        "DeleteTransitGatewayRouteTable",
        &[param("TransitGatewayRouteTableId", &route_table_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "the delete of the route table must succeed, got {status} with body {body}"
    );
    assert_eq!(
        stored_routes(&engine, &route_table_id),
        0,
        "expected the routes of a deleted route table to be gone"
    );

    let (status, body) = invoke(
        &router,
        "SearchTransitGatewayRoutes",
        &{
            let mut params = vec![param("TransitGatewayRouteTableId", &route_table_id)];
            params.extend(filters(&[("type", &[TRANSIT_GATEWAY_ROUTE_TYPE_STATIC])]));
            params
        },
    )
    .await;
    assert_eq!(
        status, 400,
        "expected HTTP 400 for a route table that is gone, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        "expected the code for an unresolvable route table, got {body}"
    );
}

/// A row of the refusal table: the request at fault and the code and status EC2 answers it with.
struct RefusalCase {
    /// The case name every assertion of the row quotes.
    name: &'static str,
    /// The operation the request is made of.
    action: &'static str,
    /// The request, built from the route table id and the stored attachment id of the fixture.
    request: fn(&str, &str) -> Vec<(String, String)>,
    /// The HTTP status expected.
    status: u16,
    /// The `<Code>` expected.
    code: &'static str,
}

/// Every member of the request is read before anything is written: a parameter the request did
/// not carry, a value that is no CIDR block or no id of its resource, two members that cannot
/// hold together, an id that resolves to nothing, a filter name the operation does not
/// document, and a dry run are each answered with their own code, and nothing is stored.
#[tokio::test]
async fn a_request_at_fault_is_refused_with_its_own_code() {
    let cases = [
        RefusalCase {
            name: "a create whose destination is empty",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, _| {
                vec![
                    param("TransitGatewayRouteTableId", route_table_id),
                    param("DestinationCidrBlock", ""),
                ]
            },
            status: 400,
            code: MISSING_PARAMETER,
        },
        RefusalCase {
            name: "a create whose route table id is empty",
            action: "CreateTransitGatewayRoute",
            request: |_, attachment_id| create_request("", "10.8.0.0/16", Some(attachment_id)),
            status: 400,
            code: MISSING_PARAMETER,
        },
        RefusalCase {
            name: "a create whose destination is no CIDR block",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, attachment_id| {
                create_request(route_table_id, "10.8.0.0", Some(attachment_id))
            },
            status: 400,
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            name: "a create that both drops and forwards",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, attachment_id| {
                vec![
                    param("TransitGatewayRouteTableId", route_table_id),
                    param("DestinationCidrBlock", "10.8.0.0/16"),
                    param("TransitGatewayAttachmentId", attachment_id),
                    param("Blackhole", "true"),
                ]
            },
            status: 400,
            code: INVALID_PARAMETER_COMBINATION,
        },
        RefusalCase {
            name: "a create that neither drops nor names an attachment",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, _| {
                vec![
                    param("TransitGatewayRouteTableId", route_table_id),
                    param("DestinationCidrBlock", "10.8.0.0/16"),
                ]
            },
            status: 400,
            code: MISSING_PARAMETER,
        },
        RefusalCase {
            name: "a create whose route table id is no id of that kind",
            action: "CreateTransitGatewayRoute",
            request: |_, attachment_id| {
                create_request("not-a-route-table", "10.8.0.0/16", Some(attachment_id))
            },
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_MALFORMED,
        },
        RefusalCase {
            name: "a create whose route table id resolves to nothing",
            action: "CreateTransitGatewayRoute",
            request: |_, attachment_id| {
                create_request("tgw-rtb-00000000000000fff", "10.8.0.0/16", Some(attachment_id))
            },
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        },
        RefusalCase {
            name: "a create whose attachment resolves to nothing",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, _| {
                create_request(
                    route_table_id,
                    "10.8.0.0/16",
                    Some("tgw-attach-00000000000000fff"),
                )
            },
            status: 400,
            code: INVALID_ATTACHMENT_ID_NOT_FOUND,
        },
        RefusalCase {
            name: "a create asked as a dry run",
            action: "CreateTransitGatewayRoute",
            request: |route_table_id, attachment_id| {
                let mut request =
                    create_request(route_table_id, "10.8.0.0/16", Some(attachment_id));
                request.push(param("DryRun", "true"));
                request
            },
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
        },
        RefusalCase {
            name: "a search narrowed by a filter the operation does not document",
            action: "SearchTransitGatewayRoutes",
            request: |route_table_id, _| {
                let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
                params.extend(filters(&[("route-search.exact-matches", &["10.8.0.0/16"])]));
                params
            },
            status: 400,
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            name: "a search whose route-search value is no CIDR block",
            action: "SearchTransitGatewayRoutes",
            request: |route_table_id, _| {
                let mut params = vec![param("TransitGatewayRouteTableId", route_table_id)];
                params.extend(filters(&[("route-search.exact-match", &["10.8.0.0"])]));
                params
            },
            status: 400,
            code: INVALID_PARAMETER_VALUE,
        },
        RefusalCase {
            name: "a search that carries no filter at all",
            action: "SearchTransitGatewayRoutes",
            request: |route_table_id, _| vec![param("TransitGatewayRouteTableId", route_table_id)],
            status: 400,
            code: MALFORMED_INPUT,
        },
        RefusalCase {
            name: "a search whose route table id is empty",
            action: "SearchTransitGatewayRoutes",
            request: |_, _| {
                let mut params = vec![param("TransitGatewayRouteTableId", "")];
                params.extend(filters(&[("type", &["static"])]));
                params
            },
            status: 400,
            code: MISSING_PARAMETER,
        },
        RefusalCase {
            name: "a search whose route table id resolves to nothing",
            action: "SearchTransitGatewayRoutes",
            request: |_, _| {
                let mut params = vec![param(
                    "TransitGatewayRouteTableId",
                    "tgw-rtb-00000000000000fff",
                )];
                params.extend(filters(&[("type", &["static"])]));
                params
            },
            status: 400,
            code: INVALID_ROUTE_TABLE_ID_NOT_FOUND,
        },
        RefusalCase {
            name: "a search asked as a dry run",
            action: "SearchTransitGatewayRoutes",
            request: |route_table_id, _| {
                let mut params = vec![
                    param("TransitGatewayRouteTableId", route_table_id),
                    param("DryRun", "true"),
                ];
                params.extend(filters(&[("type", &["static"])]));
                params
            },
            status: DRY_RUN_STATUS,
            code: "DryRunOperation",
        },
    ];

    for case in cases {
        let RefusalCase {
            name,
            action,
            request,
            status: expected_status,
            code,
        } = case;
        let (router, engine) = super::ec2_query::emulator();
        let route_table_id = route_table(&router).await;
        let kinds = kinds();
        let attachment_id = seed_attachment(&engine, &kinds[0]);

        let (status, body) = invoke(&router, action, &request(&route_table_id, &attachment_id)).await;
        assert_eq!(
            status, expected_status,
            "case {name:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {name:?}: expected the code {code}, got {body}"
        );
        assert_eq!(
            stored_routes(&engine, &route_table_id),
            0,
            "case {name:?}: expected a refused request to have written nothing"
        );
    }
}
