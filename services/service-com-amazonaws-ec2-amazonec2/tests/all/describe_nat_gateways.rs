// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeNatGateways`, driven over the wire the service actually
//! speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body carries
//! `Action=DescribeNatGateways`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rules under test:
//!
//! 1. the call REPORTS THE NAT GATEWAYS THE ACCOUNT'S STORE HOLDS, each as the modelled
//!    `NatGateway` shape it is stored as — every member included, down to the addresses on
//!    it — in `NatGatewayId` order, and writes nothing a later caller would not have seen;
//! 2. the report is NARROWED by the ids the request names and by every filter the operation
//!    documents, filters joined with an AND and a filter's values with an OR, where a
//!    filter name outside that catalog selects no gateway rather than refusing the call;
//! 3. the report is PAGED: at most `MaxResults` gateways with a `NextToken` naming where the
//!    next page starts, and "this value is null when there are no more items to return"
//!    (`DescribeNatGatewaysResult$NextToken`);
//! 4. a request the contract does not admit is refused with the code that names what was
//!    wrong — EC2's malformed and not-found codes for a NAT gateway id, its
//!    `InvalidPaginationToken` for a token this service did not mint, and `DryRunOperation`
//!    for a permission check, which reads nothing;
//! 5. a read reports the status the CLOCK owes a gateway's addresses, so an association
//!    answered `associating` is reported `succeeded` once its span has passed.
//!
//! Each table row is a distinct input along what those rules generalise over — which
//! documented filter, how many values, how many filters at once, one and several ids, every
//! member of the modelled `NatGatewayState` catalog rather than only the states one
//! recording happens to carry, and several page sizes against several gateway counts — so no
//! row is the specification.
//!
//! The NAT gateways themselves are seeded straight into the store, the way
//! `nat_gateway_addresses` seeds its gateways: `CreateNatGateway` is still a scaffold (its
//! own stub issue, `20261023T102725Z-ec2-createnatgateway-stub`), so no API path brings a NAT
//! gateway into the account yet. Everything else is real: the VPCs and subnets the gateways
//! sit in come from `CreateVpc`/`CreateSubnet`, and the Elastic IP addresses from
//! `AllocateAddress` and `AssociateNatGatewayAddress`.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ec2::ops::types::{NAT_GATEWAY_STATE_VALUES, NatGateway, Tag};
use ec2::state::{
    AVAILABILITY_MODE_ZONAL, CONNECTIVITY_TYPE_PRIVATE, CONNECTIVITY_TYPE_PUBLIC,
    ComAmazonawsEc2Amazonec2State as Ec2State, NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING,
    NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED, NAT_GATEWAY_STATE_AVAILABLE, TRANSITION_DELAY,
    system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{as_timestamp, error_code, error_message, filters, indexed};
use super::wire::{Param, child_text, document, param, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// EC2's error code for a NAT gateway id the account holds no NAT gateway for.
const NAT_GATEWAY_NOT_FOUND: &str = "NatGatewayNotFound";

/// EC2's error code for something that is not a NAT gateway id at all.
const NAT_GATEWAY_MALFORMED: &str = "NatGatewayMalformed";

/// EC2's error code for a `NextToken` this service did not mint.
const INVALID_PAGINATION_TOKEN: &str = "InvalidPaginationToken";

/// EC2's answer to a permission check by a caller that holds the permission.
const DRY_RUN_OPERATION: &str = "DryRunOperation";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine whose store the handlers read.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

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

/// One `DescribeNatGateways` call carrying `params`.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, "DescribeNatGateways", params).await
}

/// One `DescribeNatGateways` call that has to succeed.
async fn describe_ok(case: &str, router: &MultiServiceRouter, params: &[Param]) -> String {
    ok(case, router, "DescribeNatGateways", params).await
}

/// A VPC with `cidr_block`, as its id.
async fn create_vpc(case: &str, router: &MultiServiceRouter, cidr_block: &str) -> String {
    let body = ok(case, router, "CreateVpc", &[param("CidrBlock", cidr_block)]).await;
    super::ec2_query::element(&body, "vpcId")
        .unwrap_or_else(|| panic!("case {case:?}: no <vpcId> in {body}"))
}

/// A subnet of `vpc_id` with `cidr_block`, as its id.
async fn create_subnet(
    case: &str,
    router: &MultiServiceRouter,
    vpc_id: &str,
    cidr_block: &str,
) -> String {
    let body = ok(
        case,
        router,
        "CreateSubnet",
        &[param("VpcId", vpc_id), param("CidrBlock", cidr_block)],
    )
    .await;
    super::ec2_query::element(&body, "subnetId")
        .unwrap_or_else(|| panic!("case {case:?}: no <subnetId> in {body}"))
}

/// One Elastic IP address out of Amazon's pool, as `(allocation id, public address)`.
async fn allocate_address(case: &str, router: &MultiServiceRouter) -> (String, String) {
    let body = ok(case, router, "AllocateAddress", &[]).await;
    (
        super::ec2_query::element(&body, "allocationId")
            .unwrap_or_else(|| panic!("case {case:?}: no <allocationId> in {body}")),
        super::ec2_query::element(&body, "publicIp")
            .unwrap_or_else(|| panic!("case {case:?}: no <publicIp> in {body}")),
    )
}

/// What one seeded NAT gateway is written with — the facts of a stored gateway the describe
/// reports and the documented filters select on.
#[derive(Debug, Clone)]
struct GatewaySpec {
    /// Which network of [`World`] the gateway sits in, by index.
    network: usize,
    /// Which member of the modelled `NatGatewayState` catalog it is in.
    state: &'static str,
    /// Its `ConnectivityType`.
    connectivity: &'static str,
    /// The tags it carries.
    tags: &'static [(&'static str, &'static str)],
}

/// One seeded NAT gateway: the id the store minted for it beside the facts a row asserts
/// against.
#[derive(Debug, Clone)]
struct Seeded {
    nat_gateway_id: String,
    vpc_id: String,
    subnet_id: String,
    state: &'static str,
    connectivity: &'static str,
    tags: &'static [(&'static str, &'static str)],
    created_at: f64,
}

impl Seeded {
    /// The value this gateway carries for the tag `key`, if any.
    fn tag(&self, key: &str) -> Option<&str> {
        self.tags
            .iter()
            .find(|(tag_key, _)| *tag_key == key)
            .map(|(_, value)| *value)
    }
}

/// The networks the seeded gateways sit in, and the gateways themselves.
#[derive(Debug, Clone)]
struct World {
    /// `(vpc id, subnet id)` per network, indexed by [`GatewaySpec::network`].
    networks: Vec<(String, String)>,
    gateways: Vec<Seeded>,
}

impl World {
    /// The ids of the seeded gateways `keep` accepts, in the `NatGatewayId` order a report is
    /// in.
    fn ids_where(&self, keep: impl Fn(&Seeded) -> bool) -> Vec<String> {
        let mut ids: Vec<String> = self
            .gateways
            .iter()
            .filter(|gateway| keep(gateway))
            .map(|gateway| gateway.nat_gateway_id.clone())
            .collect();
        ids.sort();
        ids
    }

    /// Every seeded gateway's id, in report order.
    fn all_ids(&self) -> Vec<String> {
        self.ids_where(|_| true)
    }

    /// The seeded gateway in `state`, which the table below gives exactly one of per state.
    fn in_state(&self, state: &str) -> &Seeded {
        self.gateways
            .iter()
            .find(|gateway| gateway.state == state)
            .unwrap_or_else(|| panic!("the seeded world holds a gateway in state {state:?}"))
    }
}

/// Write one NAT gateway built to `spec` into `networks`, and answer what a row asserts
/// against it.
fn seed_gateway(
    engine: &engine::InMemoryEngine,
    networks: &[(String, String)],
    spec: &GatewaySpec,
) -> Seeded {
    let (vpc_id, subnet_id) = networks[spec.network].clone();
    let created_at = SystemTime::now();
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let nat_gateway_id = guard.next_id("nat");
    guard.insert_nat_gateway(NatGateway {
        nat_gateway_id: Some(nat_gateway_id.clone()),
        state: Some(spec.state.to_string()),
        connectivity_type: Some(spec.connectivity.to_string()),
        availability_mode: Some(AVAILABILITY_MODE_ZONAL.to_string()),
        subnet_id: Some(subnet_id.clone()),
        vpc_id: Some(vpc_id.clone()),
        create_time: Some(system_time_to_date_time(created_at)),
        nat_gateway_addresses: Some(Vec::new()),
        tags: Some(
            spec.tags
                .iter()
                .map(|(key, value)| Tag {
                    key: Some((*key).to_string()),
                    value: Some((*value).to_string()),
                })
                .collect(),
        ),
        ..NatGateway::default()
    });
    Seeded {
        nat_gateway_id,
        vpc_id,
        subnet_id,
        state: spec.state,
        connectivity: spec.connectivity,
        tags: spec.tags,
        created_at: system_time_to_date_time(created_at).as_secs_f64(),
    }
}

/// The world every narrowing test reads: two VPCs, three subnets, and one gateway per row of
/// the table below — a row per member of the modelled `NatGatewayState` catalog, spread over
/// the networks and carrying different tag sets, so no single attribute separates them.
async fn seed_world(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> World {
    let first_vpc = create_vpc("the first network", router, "10.0.0.0/16").await;
    let second_vpc = create_vpc("the second network", router, "10.1.0.0/16").await;
    let networks = vec![
        (
            first_vpc.clone(),
            create_subnet("the first network", router, &first_vpc, "10.0.1.0/24").await,
        ),
        (
            first_vpc.clone(),
            create_subnet("the first network's second subnet", router, &first_vpc, "10.0.2.0/24")
                .await,
        ),
        (
            second_vpc.clone(),
            create_subnet("the second network", router, &second_vpc, "10.1.1.0/24").await,
        ),
    ];

    // One gateway per member of `NatGatewayState`, so the `state` filter is driven off the
    // modelled catalog rather than off the states one recording carries.
    let specs = [
        GatewaySpec {
            network: 0,
            state: "available",
            connectivity: CONNECTIVITY_TYPE_PUBLIC,
            tags: &[("Owner", "TeamA"), ("Tier", "edge")],
        },
        GatewaySpec {
            network: 1,
            state: "pending",
            connectivity: CONNECTIVITY_TYPE_PUBLIC,
            tags: &[("Owner", "TeamB")],
        },
        GatewaySpec {
            network: 1,
            state: "failed",
            connectivity: CONNECTIVITY_TYPE_PRIVATE,
            tags: &[],
        },
        GatewaySpec {
            network: 2,
            state: "deleting",
            connectivity: CONNECTIVITY_TYPE_PRIVATE,
            tags: &[("Owner", "TeamA")],
        },
        GatewaySpec {
            network: 2,
            state: "deleted",
            connectivity: CONNECTIVITY_TYPE_PUBLIC,
            tags: &[("Tier", "edge")],
        },
    ];
    let gateways = specs
        .iter()
        .map(|spec| seed_gateway(engine, &networks, spec))
        .collect();
    World { networks, gateways }
}

/// The `<item>` elements of a report's `natGatewaySet`, in the order it reported them.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    set_items(parsed.root_element(), "natGatewaySet")
        .into_iter()
        .map(|item| {
            child_text(item, "natGatewayId")
                .unwrap_or_else(|| panic!("case {case:?}: a reported gateway has no id in {body}"))
        })
        .collect()
}

/// The `nextToken` of a report, or `None` when it carries none.
fn next_token(case: &str, body: &str) -> Option<String> {
    let parsed = document(case, body);
    let token = parsed
        .root_element()
        .children()
        .find(|node| node.is_element() && node.tag_name().name() == "nextToken")
        .map(|node| node.text().unwrap_or_default().to_string());
    if let Some(token) = token.as_deref() {
        assert!(
            !token.is_empty(),
            "case {case:?}: a reported nextToken is never empty, got {body}"
        );
    }
    token
}

/// One `DescribeNatGateways` narrowed by `params`, as the ids it reported.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = describe_ok(case, router, params).await;
    reported_ids(case, &body)
}

/// The form fields naming `ids` as the request's `NatGatewayIds`.
fn id_params(ids: &[&str]) -> Vec<Param> {
    indexed("NatGatewayId", ids)
}

/// The form fields for the filters `entries` names.
fn filter_params(entries: &[(&str, &[&str])]) -> Vec<Param> {
    filters(entries)
}

/// The rule: every NAT gateway the store holds is reported, as the modelled shape it is
/// stored as, in `NatGatewayId` order — and reading them twice reports the same thing, a read
/// changing nothing a later caller sees.
#[tokio::test]
async fn every_stored_nat_gateway_is_reported_as_it_is_stored() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;

    let case = "the whole store";
    let body = describe_ok(case, &router, &[]).await;
    assert_eq!(
        reported_ids(case, &body),
        world.all_ids(),
        "case {case:?}: expected every stored gateway in NatGatewayId order, got {body}"
    );

    // Every member the store holds for one gateway reaches the wire, per row of the table.
    let parsed = document(case, &body);
    let items = set_items(parsed.root_element(), "natGatewaySet");
    for gateway in &world.gateways {
        let item = items
            .iter()
            .find(|item| child_text(**item, "natGatewayId").as_deref() == Some(gateway.nat_gateway_id.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: {} is not in the report {body}",
                    gateway.nat_gateway_id
                )
            });
        let reported = |name: &str| child_text(*item, name);
        for (member, expected) in [
            ("state", gateway.state.to_string()),
            ("subnetId", gateway.subnet_id.clone()),
            ("vpcId", gateway.vpc_id.clone()),
            ("connectivityType", gateway.connectivity.to_string()),
            ("availabilityMode", AVAILABILITY_MODE_ZONAL.to_string()),
        ] {
            assert_eq!(
                reported(member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected <{member}> {expected:?} on {}, got {:?}",
                gateway.nat_gateway_id,
                reported(member)
            );
        }
        let create_time = reported("createTime")
            .and_then(|text| as_timestamp(&text))
            .unwrap_or_else(|| {
                panic!(
                    "case {case:?}: expected an ISO 8601 createTime on {}, got {:?}",
                    gateway.nat_gateway_id,
                    reported("createTime")
                )
            });
        assert!(
            (create_time - gateway.created_at).abs() < 1.0,
            "case {case:?}: expected the stored createTime {} on {}, got {create_time}",
            gateway.created_at,
            gateway.nat_gateway_id
        );
        let reported_tags: BTreeSet<(String, String)> = set_items(*item, "tagSet")
            .into_iter()
            .map(|tag| {
                (
                    child_text(tag, "key").unwrap_or_default(),
                    child_text(tag, "value").unwrap_or_default(),
                )
            })
            .collect();
        let expected_tags: BTreeSet<(String, String)> = gateway
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            reported_tags, expected_tags,
            "case {case:?}: expected the stored tags on {}, got {reported_tags:?}",
            gateway.nat_gateway_id
        );
    }

    // A report is a READ: the second one answers exactly what the first did.
    let again = describe_ok("the whole store, read again", &router, &[]).await;
    assert_eq!(
        reported_ids("the whole store, read again", &again),
        world.all_ids(),
        "case {case:?}: a read changes nothing a later caller sees, got {again}"
    );
    assert_eq!(
        next_token(case, &body),
        None,
        "case {case:?}: no token once there are no more items to return, got {body}"
    );
}

/// The rule: the ids the request names narrow the report to those gateways, whatever their
/// number or the order they are named in.
#[tokio::test]
async fn the_ids_the_request_names_narrow_the_report() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    let all = world.all_ids();

    // `(case, the ids the request names, the ids expected back)`.
    let rows: Vec<(&str, Vec<&str>, Vec<String>)> = vec![
        ("one id", vec![all[0].as_str()], vec![all[0].clone()]),
        (
            "two ids",
            vec![all[1].as_str(), all[3].as_str()],
            vec![all[1].clone(), all[3].clone()],
        ),
        (
            "the same two ids named in the other order",
            vec![all[3].as_str(), all[1].as_str()],
            vec![all[1].clone(), all[3].clone()],
        ),
        (
            "one id named twice",
            vec![all[2].as_str(), all[2].as_str()],
            vec![all[2].clone()],
        ),
        (
            "every id of the account",
            all.iter().map(String::as_str).collect(),
            all.clone(),
        ),
    ];

    for (case, named, expected) in rows {
        let reported = described_ids(case, &router, &id_params(&named)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} for the ids {named:?}, got {reported:?}"
        );
    }
}

/// One row of the filter table: the case, the filters the request carries as
/// `(name, values)`, and the ids expected back in report order.
type FilterRow<'a> = (&'a str, Vec<(&'a str, Vec<&'a str>)>, Vec<String>);

/// The rule: each filter the operation documents selects on the attribute it names, filters
/// are joined with an AND and one filter's values with an OR, and a name outside that catalog
/// selects no gateway rather than refusing the call.
#[tokio::test]
async fn each_documented_filter_selects_on_the_attribute_it_names() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    let all = world.all_ids();

    // Every member of the modelled `NatGatewayState` catalog
    // (`ec2::ops::types::NAT_GATEWAY_STATE_VALUES`), including the ones no row is in, so the
    // filter is tested past the states this world happens to hold.
    for state in NAT_GATEWAY_STATE_VALUES.iter().copied() {
        let case = format!("state {state}");
        let expected = world.ids_where(|gateway| gateway.state == state);
        let reported = described_ids(&case, &router, &filter_params(&[("state", &[state])])).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?} for state {state}, got {reported:?}"
        );
    }

    let first_gateway = world.gateways[0].clone();
    let (first_vpc, first_subnet) = world.networks[0].clone();
    let (_, second_subnet) = world.networks[1].clone();
    let (second_vpc, _) = world.networks[2].clone();

    // `(case, the filters the request carries, the ids expected back)`. The catalog is
    // `DescribeNatGatewaysRequest$Filter`: `nat-gateway-id`, `state`, `subnet-id`, `vpc-id`,
    // `tag:<key>` and `tag-key`.
    let rows: Vec<FilterRow> = vec![
        (
            "nat-gateway-id names one gateway",
            vec![("nat-gateway-id", vec![all[2].as_str()])],
            vec![all[2].clone()],
        ),
        (
            "nat-gateway-id names two, the values joined with an OR",
            vec![(
                "nat-gateway-id",
                vec![all[0].as_str(), all[4].as_str()],
            )],
            vec![all[0].clone(), all[4].clone()],
        ),
        (
            "nat-gateway-id names an id nothing holds",
            vec![("nat-gateway-id", vec!["nat-0123456789abcdef0"])],
            Vec::new(),
        ),
        (
            "subnet-id selects the gateways of one subnet",
            vec![("subnet-id", vec![first_subnet.as_str()])],
            world.ids_where(|gateway| gateway.subnet_id == first_subnet),
        ),
        (
            "subnet-id selects the gateways of two subnets",
            vec![(
                "subnet-id",
                vec![first_subnet.as_str(), second_subnet.as_str()],
            )],
            world.ids_where(|gateway| {
                gateway.subnet_id == first_subnet || gateway.subnet_id == second_subnet
            }),
        ),
        (
            "vpc-id selects the gateways of one VPC",
            vec![("vpc-id", vec![second_vpc.as_str()])],
            world.ids_where(|gateway| gateway.vpc_id == second_vpc),
        ),
        (
            "tag:Owner selects by one tag's value",
            vec![("tag:Owner", vec!["TeamA"])],
            world.ids_where(|gateway| gateway.tag("Owner") == Some("TeamA")),
        ),
        (
            "tag:Owner selects by either of two values",
            vec![("tag:Owner", vec!["TeamA", "TeamB"])],
            world.ids_where(|gateway| gateway.tag("Owner").is_some()),
        ),
        (
            "tag:Tier selects by another key entirely",
            vec![("tag:Tier", vec!["edge"])],
            world.ids_where(|gateway| gateway.tag("Tier") == Some("edge")),
        ),
        (
            "tag-key selects whatever carries the key, whatever its value",
            vec![("tag-key", vec!["Owner"])],
            world.ids_where(|gateway| gateway.tag("Owner").is_some()),
        ),
        (
            "a tag key nothing carries selects nothing",
            vec![("tag-key", vec!["Nobody"])],
            Vec::new(),
        ),
        (
            "two filters are joined with an AND",
            vec![
                ("vpc-id", vec![first_vpc.as_str()]),
                ("state", vec![first_gateway.state]),
            ],
            world.ids_where(|gateway| {
                gateway.vpc_id == first_vpc && gateway.state == first_gateway.state
            }),
        ),
        (
            "an AND no gateway satisfies selects nothing",
            vec![
                ("vpc-id", vec![second_vpc.as_str()]),
                ("tag:Owner", vec!["TeamB"]),
            ],
            Vec::new(),
        ),
        (
            "a filter value is matched with EC2's wildcards",
            vec![("nat-gateway-id", vec!["nat-*"])],
            all.clone(),
        ),
        (
            "a filter carrying no value at all selects nothing",
            vec![("state", Vec::new())],
            Vec::new(),
        ),
        (
            "a filter name outside the catalog selects nothing",
            vec![("not-a-nat-gateway-attribute", vec!["anything"])],
            Vec::new(),
        ),
        (
            "an undocumented filter narrows a documented one to nothing",
            vec![
                ("state", vec![first_gateway.state]),
                ("sim-bogus-filter", vec!["anything"]),
            ],
            Vec::new(),
        ),
    ];

    for (case, entries, expected) in rows {
        let fields: Vec<(&str, &[&str])> = entries
            .iter()
            .map(|(name, values)| (*name, values.as_slice()))
            .collect();
        let reported = described_ids(case, &router, &filter_params(&fields)).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: expected {expected:?}, got {reported:?}"
        );
    }
}

/// The rule: the ids the request names and the filters it carries both narrow the one report,
/// so a filter that excludes a named id leaves it out.
#[tokio::test]
async fn ids_and_filters_narrow_the_same_report() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    let all = world.all_ids();
    let available = world.in_state(NAT_GATEWAY_STATE_AVAILABLE).clone();

    let case = "an id the filter keeps";
    let mut params = id_params(&[available.nat_gateway_id.as_str()]);
    params.extend(filter_params(&[("state", &[NAT_GATEWAY_STATE_AVAILABLE])]));
    let reported = described_ids(case, &router, &params).await;
    assert_eq!(
        reported,
        vec![available.nat_gateway_id.clone()],
        "case {case:?}: expected the named gateway, got {reported:?}"
    );

    let case = "an id the filter excludes";
    let mut params = id_params(&all.iter().map(String::as_str).collect::<Vec<_>>());
    params.extend(filter_params(&[("tag-key", &["Owner"])]));
    let reported = described_ids(case, &router, &params).await;
    assert_eq!(
        reported,
        world.ids_where(|gateway| gateway.tag("Owner").is_some()),
        "case {case:?}: expected only the tagged gateways, got {reported:?}"
    );
}

/// The rule: at most `MaxResults` gateways come back with a `NextToken` naming where the next
/// page starts, that token continues from the end of the page before it, and no token is
/// answered once there are no more items to return.
#[tokio::test]
async fn the_report_is_paged_in_a_stable_id_order() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    // The model's `DescribeNatGatewaysMaxResults` has a minimum of 5, so a second page needs
    // more than five gateways to exist at all: seed beyond the five the world already holds.
    let mut expected: BTreeSet<String> = world.all_ids().into_iter().collect();
    for nth in 0..8 {
        let spec = GatewaySpec {
            network: nth % world.networks.len(),
            state: NAT_GATEWAY_STATE_AVAILABLE,
            connectivity: CONNECTIVITY_TYPE_PUBLIC,
            tags: &[],
        };
        expected.insert(seed_gateway(&engine, &world.networks, &spec).nat_gateway_id);
    }

    for page_size in [5, 6, 7, 13] {
        let case = format!("pages of {page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = describe_ok(&case, &router, &params).await;
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page carries at most {page_size} gateways, got {body}"
            );
            seen.extend(page);
            token = next_token(&case, &body);
            pages += 1;
            assert!(
                pages <= 6,
                "case {case:?}: the paging must terminate, got {seen:?}"
            );
            if token.is_none() {
                break;
            }
        }
        let mut sorted = seen.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            seen.len(),
            "case {case:?}: no gateway is reported twice across the pages, got {seen:?}"
        );
        assert_eq!(
            seen.iter().cloned().collect::<BTreeSet<_>>(),
            expected,
            "case {case:?}: the pages together report every stored gateway, got {seen:?}"
        );
        assert_eq!(
            seen, sorted,
            "case {case:?}: the pages are reported in a stable id order, got {seen:?}"
        );
    }

    // A page size the whole store fits in carries no token at all.
    let case = "one page for the whole store";
    let body = describe_ok(case, &router, &[param("MaxResults", "1000")]).await;
    assert_eq!(
        next_token(case, &body),
        None,
        "case {case:?}: no token once there are no more items to return, got {body}"
    );

    // A page is cut out of the SELECTED set, so a filtered walk pages the gateways the filter
    // kept and nothing else.
    let case = "a filtered page";
    let mut params = vec![param("MaxResults", "5")];
    params.extend(filter_params(&[("tag-key", &["Owner"])]));
    let body = describe_ok(case, &router, &params).await;
    assert_eq!(
        reported_ids(case, &body),
        world.ids_where(|gateway| gateway.tag("Owner").is_some()),
        "case {case:?}: expected the tagged gateways, got {body}"
    );
    assert_eq!(
        next_token(case, &body),
        None,
        "case {case:?}: the filtered set fits one page, so it carries no token, got {body}"
    );
}

/// The rule: a request `DescribeNatGateways`'s contract does not admit is refused with the
/// code that names what was wrong, and nothing is reported alongside the refusal.
#[tokio::test]
async fn a_request_the_contract_does_not_admit_is_refused() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    let known = world.all_ids();

    // `(case, the request's fields, expected status, expected code, what the message names)`.
    let rows: Vec<(&str, Vec<Param>, u16, &str, String)> = vec![
        (
            "an id that is not a NAT gateway id at all",
            id_params(&["not-an-id"]),
            400,
            NAT_GATEWAY_MALFORMED,
            "not-an-id".to_string(),
        ),
        (
            "an id of another resource kind",
            id_params(&["vpc-0123456789abcdef0"]),
            400,
            NAT_GATEWAY_MALFORMED,
            "vpc-0123456789abcdef0".to_string(),
        ),
        (
            "a NAT gateway id prefix with nothing behind it",
            id_params(&["nat-"]),
            400,
            NAT_GATEWAY_MALFORMED,
            "nat-".to_string(),
        ),
        (
            "a well-formed id the account holds no gateway for",
            id_params(&["nat-0123456789abcdef0"]),
            400,
            NAT_GATEWAY_NOT_FOUND,
            "nat-0123456789abcdef0".to_string(),
        ),
        (
            "an unknown id beside a known one",
            id_params(&[known[0].as_str(), "nat-00000000000000000"]),
            400,
            NAT_GATEWAY_NOT_FOUND,
            "nat-00000000000000000".to_string(),
        ),
        (
            "a page token this service did not mint",
            vec![param("NextToken", "not-a-token-this-service-minted")],
            400,
            INVALID_PAGINATION_TOKEN,
            // This refusal's sentence is fixed: it names no token.
            ec2::state::INVALID_PAGINATION_TOKEN_MESSAGE.to_string(),
        ),
        (
            "a permission check",
            vec![param("DryRun", "true")],
            412,
            DRY_RUN_OPERATION,
            String::new(),
        ),
    ];

    for (case, params, expected_status, expected_code, named) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&named),
            "case {case:?}: expected the message to name {named:?}, got {message:?}"
        );
        assert!(
            reported_ids(case, &body).is_empty(),
            "case {case:?}: a refusal reports no gateway, got {body}"
        );
    }

    // The refusals read nothing: the store still reports every gateway it held.
    let case = "the store after the refusals";
    let reported = described_ids(case, &router, &[]).await;
    assert_eq!(
        reported, known,
        "case {case:?}: expected the store untouched, got {reported:?}"
    );
}

/// The rule: a read reports the status the CLOCK owes a gateway's addresses — an association
/// answered `associating` is reported `succeeded` once its span has passed, whether or not
/// anybody called in between — and the addresses themselves round-trip through the report.
#[tokio::test]
async fn the_report_carries_the_addresses_and_the_status_the_clock_owes_them() {
    let (router, engine) = emulator();
    let world = seed_world(&router, &engine).await;
    let serving = world.in_state(NAT_GATEWAY_STATE_AVAILABLE).clone();

    let case = "the address the association added";
    let (allocation_id, public_ip) = allocate_address(case, &router).await;
    let body = ok(
        case,
        &router,
        "AssociateNatGatewayAddress",
        &[
            param("NatGatewayId", &serving.nat_gateway_id),
            param("AllocationId.1", &allocation_id),
        ],
    )
    .await;
    assert!(
        body.contains(&allocation_id),
        "case {case:?}: the association answers the address it added, got {body}"
    );

    // Read straight after the call: the address is still in the transient status the
    // association left it in.
    let reported = described_address(case, &router, &serving.nat_gateway_id, &allocation_id).await;
    assert_eq!(
        reported.get("status").map(String::as_str),
        Some(NAT_GATEWAY_ADDRESS_STATUS_ASSOCIATING),
        "case {case:?}: expected the transient status, got {reported:?}"
    );
    assert_eq!(
        reported.get("publicIp").map(String::as_str),
        Some(public_ip.as_str()),
        "case {case:?}: expected the Elastic IP address it was given, got {reported:?}"
    );
    assert!(
        reported
            .get("privateIp")
            .is_some_and(|private_ip| !private_ip.is_empty()),
        "case {case:?}: expected a private address on the reported address, got {reported:?}"
    );

    // One span later the move is due, and a read materialises it.
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
    let case = "the address one span later";
    let reported = described_address(case, &router, &serving.nat_gateway_id, &allocation_id).await;
    assert_eq!(
        reported.get("status").map(String::as_str),
        Some(NAT_GATEWAY_ADDRESS_STATUS_SUCCEEDED),
        "case {case:?}: expected the terminal status, got {reported:?}"
    );
}

/// The members of the address of `nat_gateway_id` carrying `allocation_id`, as
/// `DescribeNatGateways` reports it.
async fn described_address(
    case: &str,
    router: &MultiServiceRouter,
    nat_gateway_id: &str,
    allocation_id: &str,
) -> std::collections::BTreeMap<String, String> {
    let body = describe_ok(case, router, &id_params(&[nat_gateway_id])).await;
    let parsed = document(case, &body);
    let gateway = set_items(parsed.root_element(), "natGatewaySet")
        .into_iter()
        .find(|item| child_text(*item, "natGatewayId").as_deref() == Some(nat_gateway_id))
        .unwrap_or_else(|| panic!("case {case:?}: {nat_gateway_id} is not in {body}"));
    let address = set_items(gateway, "natGatewayAddressSet")
        .into_iter()
        .find(|item| child_text(*item, "allocationId").as_deref() == Some(allocation_id))
        .unwrap_or_else(|| {
            panic!("case {case:?}: no address for {allocation_id} in {body}")
        });
    address
        .children()
        .filter(|node| node.is_element())
        .map(|node| {
            (
                node.tag_name().name().to_string(),
                node.text().unwrap_or_default().to_string(),
            )
        })
        .collect()
}
