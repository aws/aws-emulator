// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeRouteServerEndpoints`, the two creates an endpoint needs
//! under it (`CreateRouteServer` and `CreateRouteServerEndpoint`) and the
//! `DeleteRouteServerEndpoint` that takes one down, driven over the
//! wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeRouteServerEndpoints`, answered with XML (README.md / OPERATIONS.md
//! "Wire protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! * a describe REPORTS THE STORE — every stored route server endpoint
//!   unless the request narrows it by `RouteServerEndpointIds` or by a filter, in
//!   `RouteServerEndpointId` order, cut into `MaxResults`-sized pages followed by a token
//!   exactly while a further endpoint remains; an id that names no endpoint of the account is
//!   an error rather than an absent row; a filter name the operation does not answer is
//!   refused rather than read as "no results"; a token this service did not mint is refused;
//!   a dry run reports nothing; and reading changes nothing a later caller sees, beside the
//!   `pending` -> `available` move the clock already owed;
//! * a DELETE takes the endpoint the request names DOWN — it answers that stored endpoint,
//!   member for member, in the non-terminal `deleting` state the clock then owes `deleted`,
//!   whatever state the endpoint was in before the call; an id resolving to no endpoint of
//!   the account is refused with `InvalidRouteServerEndpointId.NotFound`; and an endpoint
//!   whose deletion is already underway is refused with `IncorrectState` rather than deleted
//!   twice.
//!
//! Every table row is a distinct input along what the rules generalise over — how many ids
//! the request names and in what order, which attribute a filter selects on (each name the
//! operation answers, including a tag by key and by key/value), the page size at the
//! smallest bound the model publishes and past the whole report, the tag count, which
//! route server and subnet an endpoint belongs to, whether a delete arrives while the
//! endpoint is coming up or once it is up, and the form of an id that names nothing — so the
//! one case a recording happens to contain is one row among many rather than the
//! specification.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{element, elements, error_code, error_message, filters, indexed};
use super::wire::{Param, child, child_text, document, is_long_id, param};

/// The Region every call here is signed for. The default VPC's subnets — the subnets a
/// route server endpoint is placed into below — are seeded for the emulator's default
/// Region, so calls are signed for that Region.
const REGION: &str = "us-east-1";

/// The scalar members of `RouteServerEndpoint` this suite compares, in the ec2Query
/// spelling the wire uses (`smithy_query(query="shape",shape="RouteServerEndpoint")`).
/// `failureReason` is not among them: nothing in this emulator fails an endpoint, so it is
/// reported by no stored endpoint at all.
const ENDPOINT_MEMBERS: [&str; 6] = [
    "routeServerEndpointId",
    "routeServerId",
    "vpcId",
    "subnetId",
    "eniId",
    "eniAddress",
];

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

/// The form fields of one `CreateRouteServerEndpoint` request.
fn endpoint_params(route_server_id: &str, subnet_id: &str, tags: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![
        param("RouteServerId", route_server_id),
        param("SubnetId", subnet_id),
    ];
    if !tags.is_empty() {
        params.push(param(
            "TagSpecification.1.ResourceType",
            ec2::state::RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT,
        ));
        for (index, (key, value)) in tags.iter().enumerate() {
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Key", index + 1),
                key,
            ));
            params.push(param(
                &format!("TagSpecification.1.Tag.{}.Value", index + 1),
                value,
            ));
        }
    }
    params
}

/// One endpoint of `route_server_id` in `subnet_id`, as the `routeServerEndpointId` it was
/// minted with and the whole body its create answered.
async fn create_endpoint(
    case: &str,
    router: &MultiServiceRouter,
    route_server_id: &str,
    subnet_id: &str,
    tags: &[(&str, &str)],
) -> (String, String) {
    let body = call_ok(
        case,
        router,
        "CreateRouteServerEndpoint",
        &endpoint_params(route_server_id, subnet_id, tags),
    )
    .await;
    let id = element(&body, "routeServerEndpointId")
        .unwrap_or_else(|| panic!("case {case:?}: no routeServerEndpointId in {body}"));
    (id, body)
}

/// The scalar members [`ENDPOINT_MEMBERS`] names of the endpoint `id` a response carries,
/// read through the XML tree so a member of the endpoint is never confused with one of the
/// same name elsewhere in the envelope.
fn endpoint_members(case: &str, body: &str, id: &str) -> BTreeMap<String, String> {
    let parsed = document(case, body);
    let endpoint = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "routeServerEndpointId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route server endpoint {id} in {body}"));
    ENDPOINT_MEMBERS
        .iter()
        .filter_map(|name| {
            child_text(endpoint, name).map(|value| ((*name).to_string(), value))
        })
        .collect()
}

/// The `state` the endpoint `id` is reported in.
fn endpoint_state(case: &str, body: &str, id: &str) -> String {
    let parsed = document(case, body);
    let endpoint = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "routeServerEndpointId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route server endpoint {id} in {body}"));
    child_text(endpoint, "state")
        .unwrap_or_else(|| panic!("case {case:?}: no <state> on the endpoint {id} in {body}"))
}

/// The tags of the endpoint `id`, as key/value pairs in document order.
fn endpoint_tags(case: &str, body: &str, id: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let endpoint = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "routeServerEndpointId").as_deref() == Some(id)
        })
        .unwrap_or_else(|| panic!("case {case:?}: no route server endpoint {id} in {body}"));
    let Some(set) = child(endpoint, "tagSet") else {
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

/// The endpoint ids a `DescribeRouteServerEndpoints` body reports, in document order.
fn reported_ids(body: &str) -> Vec<String> {
    elements(body, "routeServerEndpointId")
}

/// One `DescribeRouteServerEndpoints` call carrying `params`, requiring success.
async fn describe(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    call_ok(case, router, "DescribeRouteServerEndpoints", params).await
}

/// One row of the round-trip table: a distinct input along what the rule generalises over —
/// which route server the endpoint hangs off, which subnet it sits in (and so which VPC and
/// which address), and how many tags it carries.
struct EndpointCase<'a> {
    case: &'a str,
    route_server_id: &'a str,
    subnet_id: &'a str,
    tags: Vec<(&'a str, &'a str)>,
}

// ---------------------------------------------------------------------------
// The report is the store
// ---------------------------------------------------------------------------

/// The rule: `DescribeRouteServerEndpoints` reports the STORED endpoint — every member the
/// create answered reads back unchanged through the describe, including the network
/// interface and address the service minted and the tags the request asked for — and the
/// only member that moves is the `state` the clock owed `available`.
///
/// The rows differ in which route server the endpoint belongs to, which subnet it sits in
/// (and so which VPC and which address), and how many tags it carries (none, one, several).
#[tokio::test]
async fn describe_route_server_endpoints_reports_the_endpoint_the_create_stored() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    assert!(
        subnets.len() >= 2,
        "the seeded account must hold at least two subnets, got {subnets:?}"
    );
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;

    let rows: Vec<EndpointCase<'_>> = vec![
        EndpointCase {
            case: "no tags at all",
            route_server_id: &first_server,
            subnet_id: &subnets[0],
            tags: Vec::new(),
        },
        EndpointCase {
            case: "one tag",
            route_server_id: &first_server,
            subnet_id: &subnets[1],
            tags: vec![("Name", "the edge endpoint")],
        },
        EndpointCase {
            case: "several tags, on another route server",
            route_server_id: &second_server,
            subnet_id: &subnets[0],
            tags: vec![("Name", "paired"), ("tier", "edge"), ("owner", "network")],
        },
    ];

    for EndpointCase {
        case,
        route_server_id,
        subnet_id,
        tags,
    } in rows
    {
        let (id, created) = create_endpoint(case, &router, route_server_id, subnet_id, &tags).await;
        assert!(
            is_long_id("rse", &id),
            "case {case:?}: expected a long route server endpoint id, got {id:?}"
        );

        // A create answers the non-terminal state the clock then owes `available`, and the
        // describe run before that instant must report the same.
        let created_state = endpoint_state(case, &created, &id);
        assert_eq!(
            created_state,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING,
            "case {case:?}: expected a created endpoint in {}, got {created_state:?}",
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING
        );

        let body = describe(case, &router, &indexed("RouteServerEndpointId", &[&id])).await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported,
            vec![id.clone()],
            "case {case:?}: expected only {id}, got {reported:?}"
        );

        let expected = endpoint_members(case, &created, &id);
        assert_eq!(
            expected.len(),
            ENDPOINT_MEMBERS.len(),
            "case {case:?}: the create must report every member of {ENDPOINT_MEMBERS:?}, \
             got {expected:?}"
        );
        let actual = endpoint_members(case, &body, &id);
        assert_eq!(
            actual, expected,
            "case {case:?}: expected the stored members {expected:?}, got {actual:?}"
        );
        let state = endpoint_state(case, &body, &id);
        assert_eq!(
            state,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING,
            "case {case:?}: expected the endpoint still in {}, got {state:?}",
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING
        );
        let reported_tags = endpoint_tags(case, &body, &id);
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?}, got {reported_tags:?}"
        );
        assert_eq!(
            reported_tags,
            endpoint_tags(case, &created, &id),
            "case {case:?}: the create and the describe must report the same tags"
        );

        // The endpoint's VPC is the VPC OF ITS SUBNET, read off the stored subnet rather
        // than asked of the caller, so the describe reports the subnet the request named.
        assert_eq!(
            actual.get("subnetId").map(String::as_str),
            Some(subnet_id),
            "case {case:?}: expected the endpoint in the subnet {subnet_id}, got {actual:?}"
        );
        assert_eq!(
            actual.get("routeServerId").map(String::as_str),
            Some(route_server_id),
            "case {case:?}: expected the endpoint on {route_server_id}, got {actual:?}"
        );
    }

    // The clock owes every created endpoint `available`, and a read is what observes that
    // move rather than what causes it: two reads after the due instant agree.
    tokio::time::sleep(Duration::from_millis(900)).await;
    let body = describe("every endpoint, after the due instant", &router, &[]).await;
    let settled = reported_ids(&body);
    for id in &settled {
        let state = endpoint_state("after the due instant", &body, id);
        assert_eq!(
            state,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
            "case \"after the due instant\": expected {id} in {}, got {state:?}",
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE
        );
    }
    let again = describe("a second read", &router, &[]).await;
    assert_eq!(
        reported_ids(&again),
        settled,
        "a second read must report the same endpoints, got {:?}",
        reported_ids(&again)
    );
    for id in &settled {
        let state = endpoint_state("a second read", &again, id);
        assert_eq!(
            state,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
            "case \"a second read\": expected {id} in {}, got {state:?}",
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE
        );
    }
}

// ---------------------------------------------------------------------------
// Narrowing: by id and by filter
// ---------------------------------------------------------------------------

/// The rule: the report is narrowed by `RouteServerEndpointIds` and by filters — one
/// filter's values are joined with an OR and matched against the WHOLE attribute with EC2's
/// wildcards, several filters with an AND — while an id that names no endpoint of the
/// account is an error and a filter name the operation does not answer is refused.
///
/// The filter rows cover every name the operation answers — the endpoint's own id, the route
/// server it belongs to, its VPC, its subnet, its network interface and that interface's
/// address, its state, and a tag by key and by key/value — plus how many values a filter
/// carries and which wildcard it uses, so the set under test is the one the operation
/// defines rather than the one a recording happened to use.
#[tokio::test]
async fn describe_route_server_endpoints_selects_by_id_and_by_filter() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "64513").await;

    // Three endpoints differing in every attribute a filter can name.
    let (first, first_body) = create_endpoint(
        "the first endpoint",
        &router,
        &first_server,
        &subnets[0],
        &[("role", "firewall")],
    )
    .await;
    let (second, second_body) = create_endpoint(
        "the second endpoint",
        &router,
        &first_server,
        &subnets[1],
        &[("role", "router")],
    )
    .await;
    let (third, third_body) = create_endpoint(
        "the third endpoint",
        &router,
        &second_server,
        &subnets[0],
        &[("tier", "edge")],
    )
    .await;

    let member = |case: &str, body: &str, id: &str, name: &str| -> String {
        endpoint_members(case, body, id)
            .remove(name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on {id}"))
    };
    let first_vpc = member("the first endpoint", &first_body, &first, "vpcId");
    let second_vpc = member("the second endpoint", &second_body, &second, "vpcId");
    let first_eni = member("the first endpoint", &first_body, &first, "eniId");
    let third_eni = member("the third endpoint", &third_body, &third, "eniId");
    let third_address = member("the third endpoint", &third_body, &third, "eniAddress");
    assert_ne!(
        first_eni, third_eni,
        "two endpoints must hold different network interfaces, got {first_eni:?} twice"
    );

    // The `state` rows below are about a settled store rather than a race: every endpoint is
    // `available` once the clock has passed the due instant its create started.
    tokio::time::sleep(Duration::from_millis(900)).await;

    let all = vec![first.clone(), second.clone(), third.clone()];
    // (case, the request's form fields, the ids it must select, in id order)
    let cases: Vec<(&str, Vec<Param>, Vec<String>)> = vec![
        ("no selector at all", Vec::new(), all.clone()),
        (
            "one id",
            indexed("RouteServerEndpointId", &[&second]),
            vec![second.clone()],
        ),
        (
            "two ids, out of order",
            indexed("RouteServerEndpointId", &[&third, &first]),
            vec![first.clone(), third.clone()],
        ),
        (
            "the same id twice",
            indexed("RouteServerEndpointId", &[&first, &first]),
            vec![first.clone()],
        ),
        (
            "by the endpoint's own id",
            filters(&[("route-server-endpoint-id", &[third.as_str()])]),
            vec![third.clone()],
        ),
        (
            "by the route server the endpoint belongs to",
            filters(&[("route-server-id", &[first_server.as_str()])]),
            vec![first.clone(), second.clone()],
        ),
        (
            "by VPC",
            filters(&[("vpc-id", &[first_vpc.as_str()])]),
            all.clone(),
        ),
        (
            "by subnet",
            filters(&[("subnet-id", &[subnets[1].as_str()])]),
            vec![second.clone()],
        ),
        (
            "by the endpoint's network interface",
            filters(&[("eni-id", &[first_eni.as_str()])]),
            vec![first.clone()],
        ),
        (
            "by the address of that network interface",
            filters(&[("eni-address", &[third_address.as_str()])]),
            vec![third.clone()],
        ),
        (
            "by state",
            filters(&[(
                "state",
                &[ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE],
            )]),
            all.clone(),
        ),
        (
            "by a tag key",
            filters(&[("tag-key", &["tier"])]),
            vec![third.clone()],
        ),
        (
            "by a tag key and value",
            filters(&[("tag:role", &["router"])]),
            vec![second.clone()],
        ),
        (
            "two values of one filter, which are joined with an OR",
            filters(&[(
                "route-server-endpoint-id",
                &[third.as_str(), first.as_str()],
            )]),
            vec![first.clone(), third.clone()],
        ),
        (
            "a value that selects nothing beside one that does",
            filters(&[("eni-id", &["eni-00000000000000099", first_eni.as_str()])]),
            vec![first.clone()],
        ),
        (
            "two filters, which are joined with an AND",
            filters(&[
                ("route-server-id", &[first_server.as_str()]),
                ("tag:role", &["firewall"]),
            ]),
            vec![first.clone()],
        ),
        (
            "a trailing multi-character wildcard on every minted id",
            filters(&[("route-server-endpoint-id", &["rse-*"])]),
            all.clone(),
        ),
        (
            "a wildcard on the state",
            filters(&[("state", &["avail*"])]),
            all.clone(),
        ),
        (
            "a wildcard on a tag's value",
            filters(&[("tag:role", &["r*"])]),
            vec![second.clone()],
        ),
        (
            "a wildcard matched case-sensitively",
            filters(&[("tag:role", &["R*"])]),
            Vec::new(),
        ),
        (
            "a filter on a member no stored endpoint carries",
            filters(&[("failure-reason", &["*"])]),
            Vec::new(),
        ),
        (
            "a filter nothing matches",
            filters(&[("subnet-id", &["subnet-00000000000000099"])]),
            Vec::new(),
        ),
        (
            "an id and a filter that disagree",
            {
                let mut params = indexed("RouteServerEndpointId", &[&first]);
                params.extend(filters(&[("route-server-id", &[second_server.as_str()])]));
                params
            },
            Vec::new(),
        ),
    ];

    for (case, params, expected) in cases {
        let body = describe(case, &router, &params).await;
        let reported = reported_ids(&body);
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }

    assert_ne!(
        second_vpc, "",
        "the second endpoint must report the VPC of its subnet, got an empty one"
    );

    // An id that names no endpoint is an error, and an unanswered filter name is refused.
    // (case, the request's form fields, the code it must be refused with, the message)
    let refusals: Vec<(&str, Vec<Param>, &str, String)> = vec![
        (
            "an id nothing was created under",
            indexed("RouteServerEndpointId", &["rse-00000000000000099"]),
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
            format!(
                "The {} ID 'rse-00000000000000099' does not exist",
                ec2::state::RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT
            ),
        ),
        (
            "a known id beside an unknown one",
            indexed("RouteServerEndpointId", &[&first, "rse-00000000000000099"]),
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
            format!(
                "The {} ID 'rse-00000000000000099' does not exist",
                ec2::state::RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT
            ),
        ),
        (
            "an id that is not one of this service's at all",
            indexed("RouteServerEndpointId", &["not-an-endpoint"]),
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
            "The route-server-endpoint ID 'not-an-endpoint' does not exist".to_string(),
        ),
        (
            "a filter name the operation does not answer",
            filters(&[("endpoint-id", &[first.as_str()])]),
            ec2::state::INVALID_PARAMETER_VALUE,
            "The filter 'endpoint-id' is invalid".to_string(),
        ),
        (
            "a filter name in the wrong case",
            filters(&[("Subnet-Id", &[subnets[0].as_str()])]),
            ec2::state::INVALID_PARAMETER_VALUE,
            "The filter 'Subnet-Id' is invalid".to_string(),
        ),
        (
            "a tag filter with no key",
            filters(&[("tag:", &["router"])]),
            ec2::state::INVALID_PARAMETER_VALUE,
            "The filter 'tag:' is invalid".to_string(),
        ),
    ];
    for (case, params, expected_code, expected_message) in refusals {
        let (status, body) = call(&router, "DescribeRouteServerEndpoints", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code:?} with body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case:?}: expected the message {expected_message:?}, got {message:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Paging
// ---------------------------------------------------------------------------

/// The rule: the pages of a report partition the selection in `RouteServerEndpointId` order,
/// each page carries at most `MaxResults` endpoints, a token is minted exactly while an
/// endpoint follows the page, and following the tokens reports every endpoint exactly once.
///
/// The rows differ in the page size, including the smallest the model's `MaxResults` range
/// publishes, one that divides the report evenly, and one larger than the whole report.
#[tokio::test]
async fn describe_route_server_endpoints_pages_in_id_order() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;

    let mut stored: Vec<String> = Vec::new();
    for index in 0..7usize {
        let (id, _) = create_endpoint(
            "one of seven endpoints",
            &router,
            &route_server,
            &subnets[index % subnets.len()],
            &[],
        )
        .await;
        stored.push(id);
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
            let body = describe(case, &router, &params).await;
            let page = reported_ids(&body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: expected at most {page_size} endpoints in a page, got {}",
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
            "case {case:?}: expected every endpoint exactly once in id order, got {seen:?}"
        );
    }

    // A page size outside the range the model publishes is refused by the boundary
    // validator, whichever side of it the request sits on.
    // (case, the page size)
    let outside: [(&str, &str); 3] = [
        ("one below the published floor", "4"),
        ("one above the published ceiling", "1001"),
        ("no results at all", "0"),
    ];
    for (case, max_results) in outside {
        let (status, body) = call(
            &router,
            "DescribeRouteServerEndpoints",
            &[param("MaxResults", max_results)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// A foreign token, and the dry run
// ---------------------------------------------------------------------------

/// The rule: a `NextToken` this service did not mint is refused with `InvalidPaginationToken`
/// and EC2's own sentence for it, while a token it DID mint resumes the report; and a
/// `DryRun` probe from a caller holding the action's permissions is answered
/// `DryRunOperation` (HTTP 412) and reports nothing, whatever else the request selects,
/// where `DryRun=false` is an ordinary call.
///
/// The token rows differ in HOW the token fails to be one of this service's — not hex at
/// all, hex carrying nothing it minted, and one of its own with bytes appended — and the
/// dry-run rows in what the probe asks for beside the flag.
#[tokio::test]
async fn describe_route_server_endpoints_refuses_a_foreign_token_and_performs_no_dry_run() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;

    // Six endpoints: one more than the smallest page the model's `MaxResults` range
    // publishes, so the first page of that size is followed by a token this service really
    // minted.
    let mut stored: Vec<String> = Vec::new();
    for index in 0..6usize {
        let (id, _) = create_endpoint(
            "one of six endpoints",
            &router,
            &route_server,
            &subnets[index % subnets.len()],
            &[],
        )
        .await;
        stored.push(id);
    }

    let body = describe("the first page", &router, &[param("MaxResults", "5")]).await;
    let minted = element(&body, "nextToken")
        .unwrap_or_else(|| panic!("a token while an endpoint follows the page, got {body}"));

    // A token this service minted resumes the report: the refusals below are about a
    // token's provenance, not about paging.
    let body = describe(
        "the token this service minted",
        &router,
        &[param("MaxResults", "5"), param("NextToken", &minted)],
    )
    .await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported,
        stored[5..].to_vec(),
        "case \"the token this service minted\": expected the endpoints after the first \
         page, got {reported:?}"
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
            "DescribeRouteServerEndpoints",
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
    filtered.extend(filters(&[("route-server-id", &[route_server.as_str()])]));
    let mut by_id = vec![param("DryRun", "true")];
    by_id.extend(indexed("RouteServerEndpointId", &[stored[0].as_str()]));
    let probes: Vec<(&str, Vec<Param>)> = vec![
        ("a dry run with nothing else", vec![param("DryRun", "true")]),
        (
            "a dry run beside a page size",
            vec![param("DryRun", "true"), param("MaxResults", "5")],
        ),
        ("a dry run beside a filter", filtered),
        ("a dry run beside an id that names an endpoint", by_id),
    ];
    for (case, params) in probes {
        let (status, body) = call(&router, "DescribeRouteServerEndpoints", &params).await;
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
            "case {case:?}: a dry run reports no endpoint, got {reported:?}"
        );
    }

    // `DryRun=false` is not a probe: the report is served.
    let body = describe("DryRun=false", &router, &[param("DryRun", "false")]).await;
    let reported = reported_ids(&body);
    assert_eq!(
        reported, stored,
        "case \"DryRun=false\": expected every endpoint, got {reported:?}"
    );
}

// ---------------------------------------------------------------------------
// A delete takes the endpoint down
// ---------------------------------------------------------------------------

/// One `DeleteRouteServerEndpoint` call naming `id`, as its HTTP status and XML body.
async fn delete_endpoint(router: &MultiServiceRouter, id: &str) -> (u16, String) {
    call(
        router,
        "DeleteRouteServerEndpoint",
        &[param("RouteServerEndpointId", id)],
    )
    .await
}

/// The state `DescribeRouteServerEndpoints` reports the endpoint `id` in, read over the wire
/// so a delete's own answer is never what a later read is checked against.
async fn described_state(case: &str, router: &MultiServiceRouter, id: &str) -> String {
    let body = describe(case, router, &indexed("RouteServerEndpointId", &[id])).await;
    endpoint_state(case, &body, id)
}

/// Look at the endpoint `id` until its describe reports `wanted`, answering as soon as it
/// does. The clock, not a caller, moves a `deleting` endpoint to `deleted`, so a test that
/// needs the terminal state waits for the CONDITION — with a deadline generous enough that a
/// loaded host cannot turn "not yet" into a failure.
async fn poll_until_endpoint_state(
    case: &str,
    router: &MultiServiceRouter,
    id: &str,
    wanted: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let state = described_state(case, router, id).await;
        if state == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: expected the endpoint {id} to reach {wanted:?}, last read \
             {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// One row of the delete table: a distinct input along what the rule generalises over — which
/// route server and subnet the endpoint belongs to, how many tags it carries, and whether the
/// delete arrives while the endpoint is still coming up or once it is up.
struct DeleteEndpointCase<'a> {
    case: &'a str,
    route_server_id: &'a str,
    subnet_id: &'a str,
    tags: Vec<(&'a str, &'a str)>,
    wait_for_available: bool,
}

/// The rule: `DeleteRouteServerEndpoint` takes the endpoint the request names DOWN — it
/// answers that STORED endpoint, member for member, in the non-terminal `deleting` state the
/// clock then owes the terminal `deleted` one transition later, whatever state the endpoint
/// was in before the call, and the record stays reported in both delete states.
///
/// The rows differ in which route server the endpoint hangs off, which subnet it sits in (and
/// so which VPC and which network interface address), how many tags it carries, and whether
/// the delete arrives while the endpoint is still coming up or after it is up.
#[tokio::test]
async fn delete_route_server_endpoint_puts_the_endpoint_into_deleting_and_owes_it_deleted() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    assert!(
        subnets.len() >= 2,
        "the account must hold at least two subnets to place two endpoints into, got {subnets:?}"
    );
    let first_server = create_route_server("the first route server", &router, "64512").await;
    let second_server = create_route_server("the second route server", &router, "4200000000").await;

    let cases: Vec<DeleteEndpointCase<'_>> = vec![
        DeleteEndpointCase {
            case: "an untagged endpoint deleted while it is still coming up",
            route_server_id: &first_server,
            subnet_id: &subnets[0],
            tags: Vec::new(),
            wait_for_available: false,
        },
        DeleteEndpointCase {
            case: "a tagged endpoint of the same route server deleted once it is available",
            route_server_id: &first_server,
            subnet_id: &subnets[1],
            tags: vec![("role", "bgp"), ("tier", "edge")],
            wait_for_available: true,
        },
        DeleteEndpointCase {
            case: "an endpoint of the second route server deleted once it is available",
            route_server_id: &second_server,
            subnet_id: &subnets[0],
            tags: vec![("owner", "network")],
            wait_for_available: true,
        },
    ];

    let mut deleted: Vec<(&str, String)> = Vec::new();
    for DeleteEndpointCase {
        case,
        route_server_id,
        subnet_id,
        tags,
        wait_for_available,
    } in cases
    {
        let (id, created) = create_endpoint(case, &router, route_server_id, subnet_id, &tags).await;

        if wait_for_available {
            tokio::time::sleep(Duration::from_millis(900)).await;
        }
        let before = described_state(case, &router, &id).await;
        if wait_for_available {
            assert_eq!(
                before,
                ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
                "case {case:?}: expected the endpoint to be available before the delete, \
                 got {before:?}"
            );
        } else {
            // The create's own move may already have fallen due on a loaded machine, so what
            // this row fixes is that the endpoint is still COMING UP OR UP when the delete
            // arrives — the delete's own answer is asserted below either way.
            assert!(
                before == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING
                    || before == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
                "case {case:?}: expected the endpoint to be coming up or up before the \
                 delete, got {before:?}"
            );
        }

        let (status, body) = delete_endpoint(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the delete, got {status} with body {body}"
        );
        let answered = endpoint_state(case, &body, &id);
        assert_eq!(
            answered,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_DELETING,
            "case {case:?}: expected the delete to answer the deleting state, got {answered:?}"
        );
        // The answer is the STORED endpoint: every member but the state is the one the create
        // stored, whatever the delete did to it.
        let expected_members = endpoint_members(case, &created, &id);
        let delete_members = endpoint_members(case, &body, &id);
        assert_eq!(
            delete_members, expected_members,
            "case {case:?}: expected the stored members {expected_members:?} from the \
             delete, got {delete_members:?}"
        );
        let expected_tags = endpoint_tags(case, &created, &id);
        let delete_tags = endpoint_tags(case, &body, &id);
        assert_eq!(
            delete_tags, expected_tags,
            "case {case:?}: expected the tags {expected_tags:?} from the delete, \
             got {delete_tags:?}"
        );

        // Straight after the call the terminal move is not yet due, so the reader still
        // reports the transient state a caller acting immediately has to cope with.
        let reading = described_state(case, &router, &id).await;
        assert_eq!(
            reading,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_DELETING,
            "case {case:?}: expected deleting before the terminal move fell due, \
             got {reading:?}"
        );
        deleted.push((case, id));
    }

    // The clock owes every deleted endpoint its terminal state, including the one deleted
    // while it was still coming up: that endpoint's `available` move is off, not merely late.
    tokio::time::sleep(Duration::from_millis(900)).await;
    for (case, id) in &deleted {
        let state = described_state(case, &router, id).await;
        assert_eq!(
            state,
            ec2::state::ROUTE_SERVER_ENDPOINT_STATE_DELETED,
            "case {case:?}: expected deleted once the terminal move fell due, got {state:?}"
        );
    }

    // Every endpoint is still reported, so the delete moved the records rather than losing
    // them.
    let body = describe("after the deletes", &router, &[]).await;
    let reported = reported_ids(&body);
    let mut expected: Vec<String> = deleted.iter().map(|(_, id)| id.clone()).collect();
    expected.sort();
    assert_eq!(
        reported, expected,
        "expected every deleted endpoint to still be reported, got {reported:?}"
    );
}

/// The rule: a `DeleteRouteServerEndpoint` whose `RouteServerEndpointId` resolves to no
/// endpoint of the account is refused with HTTP 400 `InvalidRouteServerEndpointId.NotFound`,
/// naming the id the request sent, and deletes nothing.
///
/// The rows differ in the FORM of the id that names nothing — a well-formed long id, a short
/// one, a string that is no id at all, the id of another kind of resource (the route server
/// the endpoint hangs off, and the subnet it sits in), and a stored endpoint's id with one
/// character changed.
#[tokio::test]
async fn delete_route_server_endpoint_refuses_an_id_that_names_nothing() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;
    let (stored, _) = create_endpoint(
        "an endpoint that must survive every refusal",
        &router,
        &route_server,
        &subnets[0],
        &[],
    )
    .await;

    // (case, the id the request names)
    let cases: Vec<(&str, String)> = vec![
        (
            "a well-formed long id nothing was created under",
            "rse-00000000000000000".to_string(),
        ),
        (
            "another well-formed long id nothing was created under",
            "rse-00000000000000099".to_string(),
        ),
        (
            "a short-form id nothing was created under",
            "rse-abcd1234".to_string(),
        ),
        (
            "a string that is not an id at all",
            "not-an-endpoint".to_string(),
        ),
        (
            "the id of the route server rather than of its endpoint",
            route_server.clone(),
        ),
        (
            "the id of the subnet rather than of the endpoint in it",
            subnets[0].clone(),
        ),
        (
            "the stored endpoint's id with one character changed",
            format!("{}0", &stored[..stored.len() - 1]),
        ),
    ];

    for (case, id) in cases {
        let (status, body) = delete_endpoint(&router, &id).await;
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
        let message = error_message(&body);
        let expected = format!(
            "The {kind} ID '{id}' does not exist",
            kind = ec2::state::RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT,
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }

    // Nothing was deleted: the one stored endpoint is still in the state its create left it
    // in.
    let state = described_state("after the refusals", &router, &stored).await;
    assert!(
        state == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING
            || state == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
        "expected the stored endpoint to be untouched by a refused delete, got {state:?}"
    );
}

/// The rule: an endpoint whose deletion has already been asked for has nothing left to
/// delete, so a second `DeleteRouteServerEndpoint` is refused with HTTP 400 `IncorrectState`
/// and the sentence EC2 words for that class — the resource and the id the request carried,
/// with no state token — in either delete state, and "If you have the
/// required permissions, the error response is DryRunOperation" (`DryRun`): a dry run of a
/// delete that would have succeeded reports that outcome with HTTP 412 and deletes nothing,
/// while a dry run of one that would have been refused is refused with its own reason
/// instead.
#[tokio::test]
async fn delete_route_server_endpoint_refuses_a_second_delete_and_performs_no_dry_run() {
    let router = emulator();
    let subnets = subnet_ids(&router).await;
    let route_server = create_route_server("a route server", &router, "64512").await;

    // Two endpoints: one the dry runs probe, one deleted outright so the second delete is
    // exercised in both delete states.
    let (probed, _) = create_endpoint(
        "the probed endpoint",
        &router,
        &route_server,
        &subnets[0],
        &[],
    )
    .await;
    let (doomed, _) = create_endpoint(
        "the deleted endpoint",
        &router,
        &route_server,
        &subnets[subnets.len() - 1],
        &[],
    )
    .await;

    // (case, the id the probe names, the status it must answer, the code it must carry)
    let probes: Vec<(&str, String, u16, &str)> = vec![
        (
            "a dry run of a delete that would have succeeded",
            probed.clone(),
            412,
            ec2::state::DRY_RUN_OPERATION,
        ),
        (
            "a dry run naming an id that resolves to no endpoint",
            "rse-00000000000000000".to_string(),
            400,
            ec2::state::INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
        ),
    ];
    for (case, id, expected_status, expected_code) in probes {
        let (status, body) = call(
            &router,
            "DeleteRouteServerEndpoint",
            &[
                param("RouteServerEndpointId", &id),
                param("DryRun", "true"),
            ],
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
    let probed_state = described_state("the probed endpoint", &router, &probed).await;
    assert!(
        probed_state == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_PENDING
            || probed_state == ec2::state::ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE,
        "a dry run must delete nothing: expected the probed endpoint to be coming up or up, \
         got {probed_state:?}"
    );

    let (status, body) = delete_endpoint(&router, &doomed).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from the first delete, got {status} with body {body}"
    );

    // A second delete, while the first is still underway and once it has finished, and a dry
    // run of one: each is refused for the state the endpoint is in.
    // (case, whether to wait for the terminal state first, whether the probe is a dry run)
    let refusals: [(&str, bool, bool); 3] = [
        (
            "a second delete while the first is still underway",
            false,
            false,
        ),
        (
            "a dry run of a second delete, which is refused first",
            false,
            true,
        ),
        ("a second delete once the first has finished", true, false),
    ];
    for (case, wait, dry_run) in refusals {
        if wait {
            poll_until_endpoint_state(
                case,
                &router,
                &doomed,
                ec2::state::ROUTE_SERVER_ENDPOINT_STATE_DELETED,
            )
            .await;
        }
        let mut params = vec![param("RouteServerEndpointId", &doomed)];
        if dry_run {
            params.push(param("DryRun", "true"));
        }
        let (status, body) = call(&router, "DeleteRouteServerEndpoint", &params).await;
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
        // The sentence EC2 words for this class names the resource and the id the request
        // carried, and says the state does not admit the call without naming which state
        // that is — so the SAME sentence answers the refusal whichever of the two delete
        // states the endpoint had reached.
        let message = error_message(&body);
        let expected = format!("Route Server Endpoint {doomed} is in an invalid state.");
        assert_eq!(
            message, expected,
            "case {case:?}: expected the message {expected:?}, got {message:?}"
        );
    }
}
