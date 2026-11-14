// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateSubnetCidrReservation`, `GetSubnetCidrReservations` and
//! `DeleteSubnetCidrReservation`,
//! driven over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries
//! `Action=CreateSubnetCidrReservation`, answered with XML (README.md / OPERATIONS.md "Wire
//! protocol"). ec2Query is selected by the `ec2` signing name in the request's SigV4
//! credential scope, so every call here carries one.
//!
//! The rule under test is that `CreateSubnetCidrReservation` RESERVES PART OF A STORED
//! SUBNET'S CIDR RANGE: the reserved range is the request's own `Cidr`, read in its address
//! family and stored with its host bits cleared, it has to sit inside that subnet's range
//! of the same family and must not overlap a reservation the subnet already has, the
//! service mints the `scr-` id and the owning account, and the reservation reads back
//! unchanged through `GetSubnetCidrReservations` — which reports the reservations of ONE
//! subnet, narrowed by the filters it documents and cut into `MaxResults`-sized pages — until
//! `DeleteSubnetCidrReservation` takes the reservation its id names back out of the store,
//! answering the record as it was stored, leaving every other reservation where it was and
//! the range reservable again.
//!
//! Every table row is a distinct input along what that generalises over — the reserved
//! block's size and position, whether the request names it canonically, the reservation type
//! (one row per member of the model's own `SubnetCidrReservationType` catalog), the
//! description, the tag count, which documented filter, how many reservations and which page
//! size — so the one case a recording happens to contain is one row among many rather than
//! the specification.

use std::collections::BTreeMap;
use std::sync::Arc;

use ec2::ops::types::{
    SUBNET_CIDR_RESERVATION_TYPE_VALUES, SubnetCidrBlockState, SubnetIpv6CidrBlockAssociation,
};
use ec2::state::ComAmazonawsEc2Amazonec2State as Ec2State;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{error_code, error_message, filters};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The `ResourceType` a `CreateSubnetCidrReservation` tag specification names, from the
/// model's `ResourceType` catalog (`crate::ops::types::RESOURCE_TYPE_VALUES` carries
/// `subnet-cidr-reservation`).
const RESERVATION_RESOURCE_TYPE: &str = "subnet-cidr-reservation";

/// The id prefix EC2 mints a `SubnetCidrReservationId` with.
const RESERVATION_ID_PREFIX: &str = "scr";

/// The emulator the tests drive, as [`super::ec2_query::emulator`] builds it: the router and
/// the engine whose store the handlers read.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// The account every reservation this emulator stores is owned by.
fn account_id() -> &'static str {
    engine::emulator_account_id()
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

/// A VPC with `cidr_block`, as its id — the container every subnet below is created in.
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

/// One `CreateSubnetCidrReservation` call, as its status and body.
async fn reserve(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    call(router, "CreateSubnetCidrReservation", params).await
}

/// The form fields of a reservation request: the subnet, the range, the type, and whatever
/// else the row carries.
fn reserve_params(
    subnet_id: &str,
    cidr: &str,
    reservation_type: &str,
    extra: &[Param],
) -> Vec<Param> {
    let mut params = vec![
        param("SubnetId", subnet_id),
        param("Cidr", cidr),
        param("ReservationType", reservation_type),
    ];
    params.extend_from_slice(extra);
    params
}

/// A reservation that has to be created, as the record the service answered with.
async fn reserve_ok(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    cidr: &str,
    reservation_type: &str,
) -> Reservation {
    let params = reserve_params(subnet_id, cidr, reservation_type, &[]);
    let body = ok(case, router, "CreateSubnetCidrReservation", &params).await;
    one_reservation(case, &body)
}

/// One `GetSubnetCidrReservations` call for `subnet_id`, carrying `params`.
async fn get(router: &MultiServiceRouter, subnet_id: &str, params: &[Param]) -> (u16, String) {
    let mut fields = vec![param("SubnetId", subnet_id)];
    fields.extend_from_slice(params);
    call(router, "GetSubnetCidrReservations", &fields).await
}

/// `GetSubnetCidrReservations` requiring success, and the body it answered.
async fn get_ok(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
    params: &[Param],
) -> String {
    let (status, body) = get(router, subnet_id, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from GetSubnetCidrReservations, got {status} \
         with body {body}"
    );
    body
}

/// The form fields for a `TagSpecification.1` naming the reservation resource type and
/// `tags`.
fn tag_fields(tags: &[(&str, &str)]) -> Vec<Param> {
    let mut fields = Vec::new();
    if tags.is_empty() {
        return fields;
    }
    fields.push(param(
        "TagSpecification.1.ResourceType",
        RESERVATION_RESOURCE_TYPE,
    ));
    for (index, (key, value)) in tags.iter().enumerate() {
        let position = index + 1;
        fields.push(param(&format!("TagSpecification.1.Tag.{position}.Key"), key));
        fields.push(param(
            &format!("TagSpecification.1.Tag.{position}.Value"),
            value,
        ));
    }
    fields
}

/// One subnet CIDR reservation as a response reports it: its scalar members by their
/// ec2Query element name, and its tags in the order they were reported.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Reservation {
    members: BTreeMap<String, String>,
    tags: Vec<(String, String)>,
}

impl Reservation {
    /// The text of member `name`, or `None` when the reservation reports none.
    fn member(&self, name: &str) -> Option<&str> {
        self.members.get(name).map(String::as_str)
    }

    /// The text of member `name`, or a panic naming the case.
    fn required(&self, case: &str, name: &str) -> &str {
        self.member(name)
            .unwrap_or_else(|| panic!("case {case:?}: no <{name}> on the reservation {self:?}"))
    }

    /// The reservation's own id.
    fn id(&self, case: &str) -> String {
        self.required(case, "subnetCidrReservationId").to_string()
    }
}

/// The element names of the sets nested inside a reservation: a scalar member is never read
/// from inside one of these.
const NESTED_SETS: [&str; 1] = ["tagSet"];

/// One reservation element, read through `roxmltree` so nesting is resolved by the document
/// rather than by string surgery.
fn read_reservation(node: roxmltree::Node<'_, '_>) -> Reservation {
    let members = node
        .children()
        .filter(|inner| inner.is_element() && !NESTED_SETS.contains(&inner.tag_name().name()))
        .map(|inner| {
            (
                inner.tag_name().name().to_string(),
                inner.text().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let tags = set_items(node, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect();
    Reservation { members, tags }
}

/// The one reservation a create answered with: the element that has a
/// `subnetCidrReservationId` child.
fn one_reservation(case: &str, body: &str) -> Reservation {
    let parsed = document(case, body);
    let found: Vec<Reservation> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "subnetCidrReservationId").is_some())
        .map(read_reservation)
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one reservation, got {found:?} from {body}"
    );
    found.into_iter().next().expect("one reservation")
}

/// The reservations of one output set of a get, in the order it reported them. A set with no
/// reservation of its family is reported as an EMPTY set, which reads as no entries here.
fn reservations_in(case: &str, body: &str, set_name: &str) -> Vec<Reservation> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == set_name)
    else {
        panic!("case {case:?}: no <{set_name}> in {body}");
    };
    set.children()
        .filter(|item| item.is_element() && item.tag_name().name() == "item")
        .map(read_reservation)
        .collect()
}

/// The IPv4 reservations a get reported.
fn ipv4_reservations(case: &str, body: &str) -> Vec<Reservation> {
    reservations_in(case, body, "subnetIpv4CidrReservationSet")
}

/// The IPv6 reservations a get reported.
fn ipv6_reservations(case: &str, body: &str) -> Vec<Reservation> {
    reservations_in(case, body, "subnetIpv6CidrReservationSet")
}

/// The reservation ids a get reported out of its IPv4 set, in order.
fn reported_ids(case: &str, body: &str) -> Vec<String> {
    ipv4_reservations(case, body)
        .into_iter()
        .map(|reservation| reservation.id(case))
        .collect()
}

/// How many reservations the store holds right now — what a refusal must leave unchanged.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine.state::<Ec2State>().lock().subnet_cidr_reservations.len()
}

/// Associate `ipv6_cidr_block` with the subnet `subnet_id` names, directly in the store.
///
/// No implemented operation associates an IPv6 block with a subnet yet
/// (`AssociateSubnetCidrBlock` and IPv6 `CreateSubnet` are still scaffolds with their own
/// issues), so the IPv6 side of a subnet is seeded the way the sibling suites seed resources
/// their own create does not reach — through the store a handler reads, as the modelled
/// shape, so the handler sees exactly what that association will look like.
fn associate_ipv6_block(engine: &engine::InMemoryEngine, subnet_id: &str, ipv6_cidr_block: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let subnet = guard
        .subnets
        .get_mut(subnet_id)
        .unwrap_or_else(|| panic!("the seeded subnet {subnet_id} is in the store"));
    subnet
        .ipv6_cidr_block_association_set
        .get_or_insert_with(Vec::new)
        .push(SubnetIpv6CidrBlockAssociation {
            association_id: Some(format!("subnet-cidr-assoc-{}", &subnet_id[7..])),
            ipv6_cidr_block: Some(ipv6_cidr_block.to_string()),
            ipv6_cidr_block_state: Some(SubnetCidrBlockState {
                state: Some("associated".to_string()),
                status_message: None,
            }),
            ipv6_address_attribute: None,
            ip_source: None,
        });
}

// ---------------------------------------------------------------------------
// Create, then read the reservation back
// ---------------------------------------------------------------------------

/// One row of the create table: the case, the subnet's own CIDR block, the range the request
/// asks to reserve, the range the reservation must be STORED with, the reservation type, the
/// description the request carries, and the tags it assigns.
type CreateCase = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    Option<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// Every reservation the table describes is created from the request, stored, and read back
/// unchanged through `GetSubnetCidrReservations`.
#[tokio::test]
async fn every_reservation_is_created_from_the_request_and_reads_back_through_get() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;

    // Rows: the smallest and largest blocks of a subnet, a block in the middle, a single
    // address, a non-canonical range (host bits set) the service canonicalises, a subnet
    // whose own block is the smallest a subnet may have, and a larger subnet with a block
    // bigger than a /24 — plus the description and tag counts crossed over them.
    let cases: Vec<CreateCase> = vec![
        (
            "a /28 of a /24, no description, no tags",
            "10.0.0.0/24",
            "10.0.0.0/28",
            "10.0.0.0/28",
            "prefix",
            None,
            Vec::new(),
        ),
        (
            "a /27 in the middle, described and tagged twice",
            "10.0.1.0/24",
            "10.0.1.32/27",
            "10.0.1.32/27",
            "explicit",
            Some("probe reservation"),
            vec![("Name", "probe"), ("Owner", "TeamA")],
        ),
        (
            "host bits set are cleared",
            "10.0.2.0/24",
            "10.0.2.21/28",
            "10.0.2.16/28",
            "prefix",
            Some("one tag only"),
            vec![("Name", "canonical")],
        ),
        (
            "the subnet's whole range",
            "10.0.3.0/28",
            "10.0.3.0/28",
            "10.0.3.0/28",
            "explicit",
            None,
            Vec::new(),
        ),
        (
            "a single address",
            "10.0.4.0/24",
            "10.0.4.7/32",
            "10.0.4.7/32",
            "prefix",
            Some("a /32"),
            Vec::new(),
        ),
        (
            "a /22 of a /20 subnet",
            "10.0.16.0/20",
            "10.0.20.0/22",
            "10.0.20.0/22",
            "explicit",
            None,
            vec![("Stage", "test")],
        ),
    ];

    for (case, subnet_cidr, requested, stored, reservation_type, description, tags) in cases {
        let subnet = create_subnet(case, &router, &vpc, subnet_cidr).await;
        let mut extra = tag_fields(&tags);
        if let Some(description) = description {
            extra.push(param("Description", description));
        }
        let params = reserve_params(&subnet, requested, reservation_type, &extra);
        let body = ok(case, &router, "CreateSubnetCidrReservation", &params).await;
        let created = one_reservation(case, &body);

        let id = created.id(case);
        assert!(
            is_long_id(RESERVATION_ID_PREFIX, &id),
            "case {case:?}: expected a minted {RESERVATION_ID_PREFIX}- id, got {id:?}"
        );
        assert_eq!(
            created.required(case, "cidr"),
            stored,
            "case {case:?}: expected the reserved range {stored:?}, got {created:?}"
        );
        assert_eq!(
            created.required(case, "subnetId"),
            subnet,
            "case {case:?}: expected the reservation in subnet {subnet:?}, got {created:?}"
        );
        assert_eq!(
            created.required(case, "reservationType"),
            reservation_type,
            "case {case:?}: expected the requested type, got {created:?}"
        );
        assert_eq!(
            created.required(case, "ownerId"),
            account_id(),
            "case {case:?}: expected the account that owns the store, got {created:?}"
        );
        assert_eq!(
            created.member("description"),
            description,
            "case {case:?}: expected the requested description, got {created:?}"
        );
        let expected_tags: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {case:?}: expected the assigned tags, got {created:?}"
        );

        // The round trip: the reservation the create answered with is the one a later
        // caller reads, and it is reported in the set of its own address family.
        let get_body = get_ok(case, &router, &subnet, &[]).await;
        let reported = ipv4_reservations(case, &get_body);
        assert_eq!(
            reported.len(),
            1,
            "case {case:?}: expected the one reservation of this subnet, got {reported:?}"
        );
        let read_back = &reported[0];
        assert_eq!(
            read_back.id(case),
            id,
            "case {case:?}: expected the created reservation, got {read_back:?}"
        );
        for member in ["cidr", "subnetId", "reservationType", "ownerId"] {
            assert_eq!(
                read_back.member(member),
                created.member(member),
                "case {case:?}: expected <{member}> to read back as created, got {read_back:?}"
            );
        }
        assert_eq!(
            read_back.member("description"),
            description,
            "case {case:?}: expected the stored description, got {read_back:?}"
        );
        assert_eq!(
            read_back.tags, expected_tags,
            "case {case:?}: expected the stored tags, got {read_back:?}"
        );
        assert!(
            ipv6_reservations(case, &get_body).is_empty(),
            "case {case:?}: an IPv4 reservation belongs to no IPv6 set, got {get_body}"
        );
    }

    assert_eq!(
        stored_count(&engine),
        6,
        "every created reservation must be stored once"
    );
}

/// Every member of the model's own `SubnetCidrReservationType` catalog is accepted, stored
/// as asked, and selected by the `reservationType` filter — the whole published catalog,
/// not the one type a recording happens to carry.
#[tokio::test]
async fn every_modelled_reservation_type_is_stored_and_selected_by_its_filter() {
    let (router, _engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;

    // One reservation per catalog member, in blocks that do not overlap, so each type has
    // exactly one reservation to select.
    let mut by_type: Vec<(&str, String)> = Vec::new();
    for (index, reservation_type) in SUBNET_CIDR_RESERVATION_TYPE_VALUES.iter().enumerate() {
        let case = format!("type {reservation_type}");
        let cidr = format!("10.0.0.{}/28", index * 16);
        let created = reserve_ok(&case, &router, &subnet, &cidr, reservation_type).await;
        assert_eq!(
            created.required(&case, "reservationType"),
            *reservation_type,
            "case {case:?}: expected the requested type, got {created:?}"
        );
        by_type.push((*reservation_type, created.id(&case)));
    }

    for (reservation_type, id) in &by_type {
        let case = format!("filter reservationType={reservation_type}");
        let body = get_ok(
            &case,
            &router,
            &subnet,
            &filters(&[("reservationType", &[*reservation_type][..])]),
        )
        .await;
        assert_eq!(
            reported_ids(&case, &body),
            vec![id.clone()],
            "case {case:?}: expected only the reservation of that type, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// What a reservation's range has to be
// ---------------------------------------------------------------------------

/// An IPv6 range inside a subnet's associated IPv6 block is reserved and reported in the
/// IPv6 set, while one outside it — and one of the other family's set — is not: a reservation
/// belongs to the set of its own address family.
#[tokio::test]
async fn an_ipv6_range_of_the_subnets_ipv6_block_reads_back_in_the_ipv6_set() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    // The /64 AWS gives a subnet out of its VPC's /56.
    associate_ipv6_block(&engine, &subnet, "2600:1f00:1234:5600::/64");

    // Rows inside that block, each DISJOINT from the others (a reservation may not overlap
    // one the subnet already holds): two /80s — the prefix length AWS assigns to an IPv6
    // network-interface prefix — a /96 further in, and a single address.
    let inside = [
        ("a /80 of it", "2600:1f00:1234:5600:1::/80"),
        ("another /80 of it", "2600:1f00:1234:5600:2::/80"),
        ("a /96 of it", "2600:1f00:1234:5600:3:4::/96"),
        ("one IPv6 address", "2600:1f00:1234:5600:5::7/128"),
    ];
    let mut expected_ids = Vec::new();
    for (case, cidr) in inside {
        let created = reserve_ok(case, &router, &subnet, cidr, "prefix").await;
        assert_eq!(
            created.required(case, "cidr"),
            cidr,
            "case {case:?}: expected the reserved IPv6 range, got {created:?}"
        );
        expected_ids.push(created.id(case));
    }

    let body = get_ok("the subnet's IPv6 reservations", &router, &subnet, &[]).await;
    let reported: Vec<String> = ipv6_reservations("the subnet's IPv6 reservations", &body)
        .into_iter()
        .map(|reservation| reservation.id("the subnet's IPv6 reservations"))
        .collect();
    assert_eq!(
        reported, expected_ids,
        "expected every IPv6 reservation in the IPv6 set, got {body}"
    );
    assert!(
        reported_ids("the subnet's IPv4 reservations", &body).is_empty(),
        "an IPv6 reservation belongs to no IPv4 set, got {body}"
    );

    // Rows outside that block, each refused: the neighbouring /64, another /56 entirely, a
    // block containing the subnet's own, and the IPv4 range of the same subnet asked for as
    // an IPv6 one.
    let outside = [
        ("the neighbouring /64", "2600:1f00:1234:5601::/64"),
        ("another /56", "2600:1f00:9999:9900::/56"),
        ("a block containing it", "2600:1f00:1234:5600::/56"),
        ("the loopback address", "::1/128"),
    ];
    let held = stored_count(&engine);
    for (case, cidr) in outside {
        let (status, body) = reserve(&router, &reserve_params(&subnet, cidr, "explicit", &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            held,
            "case {case:?}: a refused reservation must store nothing"
        );
    }

    // An IPv6 range of one subnet does not overlap the same range reserved in another
    // subnet, and inside ONE subnet it does: the overlap rule is per subnet, whichever
    // family the range belongs to.
    let (status, body) = reserve(
        &router,
        &reserve_params(&subnet, "2600:1f00:1234:5600:1::/80", "prefix", &[]),
    )
    .await;
    assert_eq!(
        status, 400,
        "the held /80 again: expected HTTP 400, got {status} with body {body}"
    );
    let other = create_subnet("seed", &router, &vpc, "10.0.1.0/24").await;
    associate_ipv6_block(&engine, &other, "2600:1f00:1234:5600::/64");
    let elsewhere = reserve_ok(
        "the same /80 in another subnet",
        &router,
        &other,
        "2600:1f00:1234:5600:1::/80",
        "prefix",
    )
    .await;
    assert_eq!(
        elsewhere.required("the same /80 in another subnet", "subnetId"),
        other,
        "the reservation belongs to the subnet it was asked for, got {elsewhere:?}"
    );

    // A subnet's WHOLE IPv6 block is a range of it, so it can be reserved in one go — in a
    // subnet holding no reservation yet, since nothing else of it would be free afterwards.
    let whole = create_subnet("seed", &router, &vpc, "10.0.2.0/24").await;
    associate_ipv6_block(&engine, &whole, "2600:1f00:1234:5700::/64");
    let created = reserve_ok(
        "the whole IPv6 block",
        &router,
        &whole,
        "2600:1f00:1234:5700::/64",
        "explicit",
    )
    .await;
    assert_eq!(
        created.required("the whole IPv6 block", "cidr"),
        "2600:1f00:1234:5700::/64",
        "expected the subnet's whole IPv6 block reserved, got {created:?}"
    );
}

/// A range that is not a CIDR block, or not inside the subnet's own range of its family, is
/// refused with EC2's `InvalidParameterValue` and NOTHING is stored.
#[tokio::test]
async fn a_range_outside_the_subnet_or_not_a_cidr_block_is_refused() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;

    // Rows: a value that is not a range at all, one with no prefix length, prefix lengths
    // past each family's address size, an address that does not parse, a block of the VPC
    // but not of the subnet, a block of another network entirely, a block that STRADDLES the
    // subnet's range, and an IPv6 range for a subnet that has no IPv6 block.
    let cases = [
        "not-a-cidr",
        "10.0.0.0",
        "10.0.0.0/33",
        "10.0.0.0/abc",
        "10.0.0.256/28",
        "10.0.1.0/28",
        "192.168.0.0/24",
        "10.0.0.0/16",
        "2600:1f00::/56",
        "2600:1f00::/129",
    ];
    for case in cases {
        let (status, body) = reserve(&router, &reserve_params(&subnet, case, "prefix", &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused reservation must store nothing"
    );
}

/// Two reservations of ONE subnet may not share an address: an overlapping range is refused
/// and nothing is stored, while a disjoint one — and the same range in another subnet — is
/// accepted.
#[tokio::test]
async fn an_overlapping_range_is_refused_and_a_disjoint_one_is_accepted() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let first_subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let second_subnet = create_subnet("seed", &router, &vpc, "10.0.1.0/24").await;
    let held = reserve_ok("held", &router, &first_subnet, "10.0.0.0/28", "prefix").await;
    let held_id = held.id("held");

    // Rows: the held range itself, a block inside it, a block containing it, a non-canonical
    // spelling of it, and the first address of it — each overlapping; then the adjacent
    // block, the last block of the subnet, and a block of the subnet's lower half that the
    // held range does not reach — each disjoint. The third column is the range as the
    // service reads it (host bits cleared), which is what its refusal names.
    let cases = [
        ("the held range", "10.0.0.0/28", "10.0.0.0/28", 400),
        ("a block inside it", "10.0.0.4/30", "10.0.0.4/30", 400),
        ("a block containing it", "10.0.0.0/24", "10.0.0.0/24", 400),
        ("a non-canonical spelling of it", "10.0.0.9/28", "10.0.0.0/28", 400),
        ("one address of it", "10.0.0.3/32", "10.0.0.3/32", 400),
        ("the adjacent block", "10.0.0.16/28", "10.0.0.16/28", 200),
        ("the last block", "10.0.0.240/28", "10.0.0.240/28", 200),
        ("a /26 above it", "10.0.0.64/26", "10.0.0.64/26", 200),
    ];
    let mut accepted = 1;
    for (case, cidr, canonical, expected_status) in cases {
        let (status, body) =
            reserve(&router, &reserve_params(&first_subnet, cidr, "explicit", &[])).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "InvalidParameterValue",
                "case {case:?}: expected InvalidParameterValue, got {body}"
            );
            // The refusal names the range the request asked to reserve and what it collided
            // with — not the id of the reservation already holding those addresses.
            let message = error_message(&body);
            assert_eq!(
                message,
                format!(
                    "Reservation CIDR {canonical} has overlap with existing reserved CIDR block"
                ),
                "case {case:?}: unexpected overlap message, got {message:?}"
            );
            assert!(
                !body.contains(&held_id),
                "case {case:?}: the refusal must not name the reservation in the way \
                 ({held_id}), got {body}"
            );
        } else {
            accepted += 1;
        }
        assert_eq!(
            stored_count(&engine),
            accepted,
            "case {case:?}: expected {accepted} stored reservations, got {}",
            stored_count(&engine)
        );
    }

    // The conflict is per SUBNET: the held range is free in another subnet.
    let elsewhere = reserve_ok(
        "the held range in another subnet",
        &router,
        &second_subnet,
        "10.0.1.0/28",
        "prefix",
    )
    .await;
    assert_ne!(
        elsewhere.id("elsewhere"),
        held_id,
        "a reservation of another subnet is its own record, got {elsewhere:?}"
    );
}

/// A subnet id this account holds no subnet for is refused by BOTH operations with
/// `InvalidSubnetID.NotFound`, rather than reserving part of nothing or reporting an empty
/// set.
#[tokio::test]
async fn an_unknown_subnet_id_is_refused_by_both_operations() {
    let (router, engine) = emulator();

    // Rows: a well-formed long id nothing has, the short id form, and a value that is not a
    // subnet id at all.
    let cases = [
        "subnet-0ffffffffffffffff",
        "subnet-0fffffff",
        "not-a-subnet-id",
    ];
    for case in cases {
        let (status, body) = reserve(&router, &reserve_params(case, "10.0.0.0/28", "prefix", &[])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the create, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound from the create, got {body}"
        );

        let (status, body) = get(&router, case, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 from the get, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound from the get, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused reservation must store nothing"
    );
}

/// A tag specification naming any resource type other than the reservation being created is
/// refused rather than silently dropped.
#[tokio::test]
async fn a_tag_specification_for_another_resource_type_is_refused() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;

    // Rows: the subnet the reservation is part of, and an unrelated type — both refused by
    // the handler; then the reservation's own type with the wrong case, which the model's own
    // `ResourceType` enum refuses at the boundary before the handler runs.
    for (case, expected_code) in [
        ("subnet", "InvalidParameterValue"),
        ("instance", "InvalidParameterValue"),
        ("Subnet-Cidr-Reservation", "ValidationError"),
    ] {
        let extra = vec![
            param("TagSpecification.1.ResourceType", case),
            param("TagSpecification.1.Tag.1.Key", "Name"),
            param("TagSpecification.1.Tag.1.Value", "probe"),
        ];
        let (status, body) = reserve(
            &router,
            &reserve_params(&subnet, "10.0.0.0/28", "prefix", &extra),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }
    assert_eq!(
        stored_count(&engine),
        0,
        "a refused reservation must store nothing"
    );
}

// ---------------------------------------------------------------------------
// What the get reports
// ---------------------------------------------------------------------------

/// The get reports the reservations of the subnet it names and no other's, and it is a READ:
/// two calls answer the same and the store is unchanged.
#[tokio::test]
async fn the_get_reports_one_subnet_and_reading_changes_nothing() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let first = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let second = create_subnet("seed", &router, &vpc, "10.0.1.0/24").await;
    let third = create_subnet("seed", &router, &vpc, "10.0.2.0/24").await;

    let first_ids = vec![
        reserve_ok("first/a", &router, &first, "10.0.0.0/28", "prefix")
            .await
            .id("first/a"),
        reserve_ok("first/b", &router, &first, "10.0.0.16/28", "explicit")
            .await
            .id("first/b"),
    ];
    let second_ids = vec![
        reserve_ok("second/a", &router, &second, "10.0.1.0/26", "explicit")
            .await
            .id("second/a"),
    ];

    for (case, subnet, expected) in [
        ("first subnet", &first, first_ids.clone()),
        ("second subnet", &second, second_ids.clone()),
        ("a subnet with no reservation", &third, Vec::new()),
    ] {
        let body = get_ok(case, &router, subnet, &[]).await;
        assert_eq!(
            reported_ids(case, &body),
            expected,
            "case {case:?}: expected that subnet's own reservations, got {body}"
        );
        assert!(
            ipv6_reservations(case, &body).is_empty(),
            "case {case:?}: no IPv6 range was reserved, got {body}"
        );
    }

    let held = stored_count(&engine);
    let repeat = get_ok("second read", &router, &first, &[]).await;
    assert_eq!(
        reported_ids("second read", &repeat),
        first_ids,
        "a read must not change what a later caller sees, got {repeat}"
    );
    assert_eq!(
        stored_count(&engine),
        held,
        "reporting reservations must not write to the store"
    );
}

/// One seeded reservation of the filter table: the case, the range it reserves, its type,
/// and the tags it carries.
type FilterFixture = (&'static str, &'static str, &'static str, Vec<(&'static str, &'static str)>);

/// One row of the filter table: the case, the `Filters` to send, and the indices of the
/// seeded reservations that must come back.
type FilterCase<'a> = (&'a str, Vec<(&'a str, &'a [&'a str])>, Vec<usize>);

/// Each filter the operation documents selects the reservations whose own attribute matches a
/// value; values within one filter are OR'd, several filters are AND'd, and the comparison is
/// case-sensitive.
#[tokio::test]
async fn each_documented_filter_selects_the_reservations_whose_attribute_matches() {
    let (router, _engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let other = create_subnet("seed", &router, &vpc, "10.0.1.0/24").await;

    // Three reservations of the subnet under test, differing in type and in tags, plus one of
    // another subnet that no filter here may reach.
    let mut ids = Vec::new();
    let rows: [FilterFixture; 3] = [
        ("a", "10.0.0.0/28", "prefix", vec![("Name", "web")]),
        (
            "b",
            "10.0.0.16/28",
            "explicit",
            vec![("Name", "db"), ("Owner", "TeamA")],
        ),
        ("c", "10.0.0.32/28", "prefix", Vec::new()),
    ];
    for (case, cidr, reservation_type, tags) in rows {
        let params = reserve_params(&subnet, cidr, reservation_type, &tag_fields(&tags));
        let body = ok(case, &router, "CreateSubnetCidrReservation", &params).await;
        ids.push(one_reservation(case, &body).id(case));
    }
    let elsewhere = reserve_ok("elsewhere", &router, &other, "10.0.1.0/28", "prefix")
        .await
        .id("elsewhere");

    let subnet_values = [subnet.as_str()];
    let both_subnets = [subnet.as_str(), other.as_str()];
    // Rows: one per documented filter name, with extra rows for value counts, the wildcard
    // the EC2 matcher supports, case sensitivity, a value nothing has, and the empty value
    // list.
    let cases: Vec<FilterCase<'_>> = vec![
        ("reservationType=prefix", vec![("reservationType", &["prefix"][..])], vec![0, 2]),
        ("reservationType=explicit", vec![("reservationType", &["explicit"][..])], vec![1]),
        (
            "reservationType, both values",
            vec![("reservationType", &["prefix", "explicit"][..])],
            vec![0, 1, 2],
        ),
        (
            "reservationType, wrong case",
            vec![("reservationType", &["Prefix"][..])],
            Vec::new(),
        ),
        ("subnet-id of this subnet", vec![("subnet-id", &subnet_values[..])], vec![0, 1, 2]),
        (
            "subnet-id naming both subnets",
            vec![("subnet-id", &both_subnets[..])],
            vec![0, 1, 2],
        ),
        (
            "subnet-id of the other subnet only",
            vec![("subnet-id", &["subnet-0ffffffffffffffff"][..])],
            Vec::new(),
        ),
        ("tag:Name=web", vec![("tag:Name", &["web"][..])], vec![0]),
        (
            "tag:Name with two values",
            vec![("tag:Name", &["web", "db"][..])],
            vec![0, 1],
        ),
        ("tag:Name wildcard", vec![("tag:Name", &["*b"][..])], vec![0, 1]),
        ("tag:Owner=TeamA", vec![("tag:Owner", &["TeamA"][..])], vec![1]),
        ("tag-key=Owner", vec![("tag-key", &["Owner"][..])], vec![1]),
        ("tag-key=Name", vec![("tag-key", &["Name"][..])], vec![0, 1]),
        ("tag-key nothing has", vec![("tag-key", &["Stage"][..])], Vec::new()),
        ("an empty value list", vec![("tag-key", &[][..])], Vec::new()),
        (
            "two filters are AND'd",
            vec![("reservationType", &["prefix"][..]), ("tag-key", &["Name"][..])],
            vec![0],
        ),
    ];
    for (case, entries, expected) in cases {
        let body = get_ok(case, &router, &subnet, &filters(&entries)).await;
        let want: Vec<String> = expected.iter().map(|index| ids[*index].clone()).collect();
        assert_eq!(
            reported_ids(case, &body),
            want,
            "case {case:?}: expected the matching reservations, got {body}"
        );
        assert!(
            !body.contains(&elsewhere),
            "case {case:?}: another subnet's reservation must never be reported, got {body}"
        );
    }
}

/// A filter name this operation does not document is refused with `InvalidParameterValue`,
/// rather than silently reported as an empty set.
#[tokio::test]
async fn an_undocumented_filter_name_is_refused() {
    let (router, _engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    reserve_ok("held", &router, &subnet, "10.0.0.0/28", "prefix").await;

    // Rows: the hyphenated spelling of the documented camelCase name, a documented name with
    // the wrong case, a member of the reservation that is no filter, filters of other
    // describes, the tag filter without a key, and an empty name.
    let cases = [
        "reservation-type",
        "reservationtype",
        "Subnet-Id",
        "cidr",
        "owner-id",
        "subnet-cidr-reservation-id",
        "tag",
        "tag:",
        "",
    ];
    for case in cases {
        let (status, body) = get(&router, &subnet, &filters(&[(case, &["prefix"])])).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidParameterValue",
            "case {case:?}: expected InvalidParameterValue, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Pagination
// ---------------------------------------------------------------------------

/// Whatever the page size, walking the pages covers every reservation of the subnet exactly
/// once, in id order, and the last page carries no token.
#[tokio::test]
async fn paging_covers_every_reservation_exactly_once_for_each_page_size() {
    let (router, _engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;

    // Eight reservations, each a /28 of the subnet, so several page sizes have something to
    // cut.
    let mut all = Vec::new();
    for index in 0..8u32 {
        let case = format!("seed /28 #{index}");
        let cidr = format!("10.0.0.{}/28", index * 16);
        all.push(
            reserve_ok(&case, &router, &subnet, &cidr, "prefix")
                .await
                .id(&case),
        );
    }

    // `GetSubnetCidrReservationsMaxResults` is modelled `range 5..1000`, so the rows are its
    // smallest published size, one that does not divide the set, one that holds it exactly,
    // and the largest published size.
    for max_results in [5, 6, 8, 1000] {
        let case = format!("MaxResults={max_results}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..8 {
            let mut params = vec![param("MaxResults", &max_results.to_string())];
            if let Some(token) = &token {
                params.push(param("NextToken", token));
            }
            let body = get_ok(&case, &router, &subnet, &params).await;
            let page = reported_ids(&case, &body);
            assert!(
                page.len() <= max_results,
                "case {case:?}: a page may not exceed the page size, got {page:?}"
            );
            walked.extend(page);
            token = super::ec2_query::element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the last page must carry no token, got {token:?}"
        );
        assert_eq!(
            walked, all,
            "case {case:?}: expected every reservation exactly once in id order, got {walked:?}"
        );
    }
}

/// `MaxResults` outside the range the model publishes is refused at the boundary; each
/// published bound itself is accepted. A `NextToken` this service did not issue is refused
/// with `InvalidPaginationToken` rather than read as a cursor.
#[tokio::test]
async fn max_results_bounds_and_an_unissued_token_are_refused() {
    let (router, _engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let held = reserve_ok("held", &router, &subnet, "10.0.0.0/28", "prefix")
        .await
        .id("held");

    // The published bounds (`range 5..1000`) and one past each.
    for (max_results, expected_status) in [(4, 400), (5, 200), (1000, 200), (1001, 400)] {
        let case = format!("MaxResults={max_results}");
        let (status, body) = get(
            &router,
            &subnet,
            &[param("MaxResults", &max_results.to_string())],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
    }

    // Rows: free text, punctuation, the bare resource id a caller might guess the token is,
    // and that id hex-encoded (the shape a token takes, without the marker and checksum a
    // minted one carries).
    let cases = [
        "not-a-token".to_string(),
        "###".to_string(),
        held.clone(),
        held.bytes().map(|byte| format!("{byte:02x}")).collect(),
    ];
    for case in cases {
        let (status, body) = get(&router, &subnet, &[param("NextToken", &case)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidPaginationToken",
            "case {case:?}: expected InvalidPaginationToken, got {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// DryRun
// ---------------------------------------------------------------------------

/// `DryRun` on a request that would have succeeded answers `DryRunOperation` and writes
/// nothing; a request that would have been REFUSED — for its own parameters or for the
/// subnet it names — is refused with that reason instead, and `DryRun=false` is an ordinary
/// call. Both operations answer the flag in that same order.
#[tokio::test]
async fn dry_run_reports_whether_the_request_would_have_succeeded() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;

    let dry_run = vec![param("DryRun", "true")];
    let (status, body) = reserve(
        &router,
        &reserve_params(&subnet, "10.0.0.0/28", "prefix", &dry_run),
    )
    .await;
    assert_eq!(
        status, 412,
        "a create that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a create that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "a dry run must write nothing"
    );

    // A request that would have been refused anyway keeps its own reason.
    let (status, body) = reserve(
        &router,
        &reserve_params(&subnet, "10.0.1.0/28", "prefix", &dry_run),
    )
    .await;
    assert_eq!(
        status, 400,
        "a create that would have been refused: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "a create that would have been refused: expected its own reason, got {body}"
    );

    // The get's dry run reports the same outcome and reads nothing.
    let (status, body) = get(&router, &subnet, &dry_run).await;
    assert_eq!(
        status, 412,
        "a get dry run: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a get dry run: expected DryRunOperation, got {body}"
    );

    // Both operations answer the flag in the same order: a request that would have been
    // refused for the subnet it names keeps that reason instead of reporting the dry run.
    let unknown_subnet = "subnet-0ffffffffffffffff";
    for (case, status, body) in [
        (
            "create, unknown subnet, dry run",
            reserve(
                &router,
                &reserve_params(unknown_subnet, "10.0.0.0/28", "prefix", &dry_run),
            )
            .await,
        ),
        (
            "get, unknown subnet, dry run",
            get(&router, unknown_subnet, &dry_run).await,
        ),
    ]
    .map(|(case, (status, body))| (case, status, body))
    {
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetID.NotFound",
            "case {case:?}: expected InvalidSubnetID.NotFound, got {body}"
        );
    }

    // `DryRun=false` is an ordinary call, which stores the reservation.
    let created = ok(
        "DryRun=false",
        &router,
        "CreateSubnetCidrReservation",
        &reserve_params(
            &subnet,
            "10.0.0.0/28",
            "prefix",
            &[param("DryRun", "false")],
        ),
    )
    .await;
    let created = one_reservation("DryRun=false", &created);
    assert!(
        is_long_id(RESERVATION_ID_PREFIX, &created.id("DryRun=false")),
        "DryRun=false: expected a stored reservation, got {created:?}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "DryRun=false: the reservation must be stored"
    );
}

// ---------------------------------------------------------------------------
// Delete, and what the get reports afterwards
// ---------------------------------------------------------------------------

/// One `DeleteSubnetCidrReservation` call for `reservation_id`, carrying `params`.
async fn delete(
    router: &MultiServiceRouter,
    reservation_id: &str,
    params: &[Param],
) -> (u16, String) {
    let mut fields = vec![param("SubnetCidrReservationId", reservation_id)];
    fields.extend_from_slice(params);
    call(router, "DeleteSubnetCidrReservation", &fields).await
}

/// `DeleteSubnetCidrReservation` requiring success, and the reservation it answered with.
async fn delete_ok(case: &str, router: &MultiServiceRouter, reservation_id: &str) -> Reservation {
    let (status, body) = delete(router, reservation_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DeleteSubnetCidrReservation, got {status} \
         with body {body}"
    );
    one_reservation(case, &body)
}

/// Every reservation the get reports for `subnet_id`, both families together.
async fn reported_reservations(
    case: &str,
    router: &MultiServiceRouter,
    subnet_id: &str,
) -> Vec<Reservation> {
    let body = get_ok(case, router, subnet_id, &[]).await;
    let mut reported = ipv4_reservations(case, &body);
    reported.extend(ipv6_reservations(case, &body));
    reported
}

/// One row of the delete table: the case, the subnet's own CIDR block, an IPv6 block to
/// associate with that subnet (for a row whose range is an IPv6 one), the range to reserve,
/// its reservation type, the description the create carries, and the tags it assigns.
type DeleteCase = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static str,
    &'static str,
    Option<&'static str>,
    Vec<(&'static str, &'static str)>,
);

/// Deleting a reservation answers the record as it was stored, takes it out of the store, and
/// leaves the get reporting the subnet's remaining reservations and nothing else.
#[tokio::test]
async fn every_deleted_reservation_is_answered_as_stored_and_leaves_the_store() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;

    // Rows along what the rule generalises over: the reserved block's size and position, the
    // reservation type, whether the reservation carries a description, how many tags it
    // carries, and the address family of its range.
    let cases: Vec<DeleteCase> = vec![
        (
            "a /28 of a /24, no description, no tags",
            "10.0.0.0/24",
            None,
            "10.0.0.0/28",
            "prefix",
            None,
            Vec::new(),
        ),
        (
            "a single address, described",
            "10.0.1.0/24",
            None,
            "10.0.1.7/32",
            "explicit",
            Some("one address"),
            Vec::new(),
        ),
        (
            "a /26 in the middle, described and tagged twice",
            "10.0.2.0/24",
            None,
            "10.0.2.64/26",
            "prefix",
            Some("probe reservation"),
            vec![("Name", "probe"), ("Owner", "TeamA")],
        ),
        (
            "the subnet's whole range, tagged once",
            "10.0.3.0/28",
            None,
            "10.0.3.0/28",
            "explicit",
            None,
            vec![("Stage", "test")],
        ),
        (
            "an IPv6 /80",
            "10.0.4.0/24",
            Some("2600:1f00:1234:5600::/64"),
            "2600:1f00:1234:5600:1::/80",
            "prefix",
            Some("an IPv6 prefix"),
            Vec::new(),
        ),
    ];

    for (index, (case, subnet_cidr, ipv6_block, cidr, reservation_type, description, tags)) in
        cases.into_iter().enumerate()
    {
        let subnet = create_subnet(case, &router, &vpc, subnet_cidr).await;
        if let Some(block) = ipv6_block {
            associate_ipv6_block(&engine, &subnet, block);
        }
        // A reservation of ANOTHER subnet, which this row must leave alone: a delete names
        // ONE reservation. Each row gets its own subnet for it, so no two rows ask this VPC
        // for the same range.
        let neighbour_subnet_cidr = format!("10.0.{}.0/24", 100 + index);
        let neighbour_cidr = if ipv6_block.is_some() {
            "2600:1f00:1234:5600:9::/80".to_string()
        } else {
            format!("10.0.{}.0/28", 100 + index)
        };
        let neighbour_subnet = create_subnet(case, &router, &vpc, &neighbour_subnet_cidr).await;
        if let Some(block) = ipv6_block {
            associate_ipv6_block(&engine, &neighbour_subnet, block);
        }
        let neighbour =
            reserve_ok(case, &router, &neighbour_subnet, &neighbour_cidr, "prefix").await;

        let mut extra = tag_fields(&tags);
        if let Some(text) = description {
            extra.push(param("Description", text));
        }
        let body = ok(
            case,
            &router,
            "CreateSubnetCidrReservation",
            &reserve_params(&subnet, cidr, reservation_type, &extra),
        )
        .await;
        let created = one_reservation(case, &body);
        let id = created.id(case);
        let held = stored_count(&engine);

        let deleted = delete_ok(case, &router, &id).await;
        assert_eq!(
            deleted, created,
            "case {case:?}: expected the deleted reservation reported as it was stored, \
             got {deleted:?}"
        );
        assert_eq!(
            stored_count(&engine),
            held - 1,
            "case {case:?}: exactly the deleted reservation must leave the store"
        );

        let remaining = reported_reservations(case, &router, &subnet).await;
        assert!(
            remaining.is_empty(),
            "case {case:?}: the deleted reservation must not be reported any more, \
             got {remaining:?}"
        );
        let untouched = reported_reservations(case, &router, &neighbour_subnet).await;
        assert_eq!(
            untouched,
            vec![neighbour.clone()],
            "case {case:?}: a delete must leave every other reservation where it was, \
             got {untouched:?}"
        );

        // The range is free again: what the delete gave back can be reserved anew.
        let again = reserve_ok(case, &router, &subnet, cidr, reservation_type).await;
        assert_eq!(
            again.required(case, "cidr"),
            created.required(case, "cidr"),
            "case {case:?}: expected the freed range reservable again, got {again:?}"
        );
        assert_ne!(
            again.id(case),
            id,
            "case {case:?}: a new reservation gets a new id, got {again:?}"
        );
    }
}

/// A `SubnetCidrReservationId` this account holds no reservation for is refused with EC2's
/// own not-found code, whatever form the value takes — and a second delete of a reservation
/// already deleted is that same refusal rather than a silent success.
#[tokio::test]
async fn an_unknown_reservation_id_is_refused_and_a_second_delete_too() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let created = reserve_ok("seed", &router, &subnet, "10.0.0.0/28", "prefix").await;
    let deleted_id = created.id("seed");
    delete_ok("seed", &router, &deleted_id).await;

    // Rows: a well-formed long id nothing has, the short id form, a value that is not a
    // reservation id at all, another resource's id, and the id of a reservation that was
    // deleted a moment ago.
    let cases = [
        "scr-0ffffffffffffffff".to_string(),
        "scr-0fffffff".to_string(),
        "not-a-reservation-id".to_string(),
        subnet.clone(),
        deleted_id.clone(),
    ];
    for case in cases {
        let (status, body) = delete(&router, &case, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidSubnetCidrReservationID.NotFound",
            "case {case:?}: expected the not-found code, got {body}"
        );
        // The sentence EC2 answers a resource named by one id with, naming the reservation's
        // own `ResourceType` and the id the REQUEST carried.
        assert_eq!(
            error_message(&body),
            format!("The subnet-cidr-reservation ID '{case}' does not exist"),
            "case {case:?}: expected the refusal to name the id the request carried, got {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused delete must change nothing"
        );
    }
}

/// `DryRun` on a delete reports whether the request would have succeeded and removes nothing;
/// a request that would have been refused anyway keeps its own reason.
#[tokio::test]
async fn a_dry_run_delete_removes_nothing() {
    let (router, engine) = emulator();
    let vpc = create_vpc("seed", &router, "10.0.0.0/16").await;
    let subnet = create_subnet("seed", &router, &vpc, "10.0.0.0/24").await;
    let created = reserve_ok("seed", &router, &subnet, "10.0.0.0/28", "prefix").await;
    let id = created.id("seed");

    let (status, body) = delete(&router, &id, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "a delete that would have succeeded: expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "a delete that would have succeeded: expected DryRunOperation, got {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "a dry run must remove nothing"
    );
    let still_there = reported_reservations("after the dry run", &router, &subnet).await;
    assert_eq!(
        still_there,
        vec![created.clone()],
        "a dry run must leave the reservation readable, got {still_there:?}"
    );

    // A delete that would have been refused keeps that reason instead of reporting the dry
    // run, the same order the create and the get answer the flag in.
    let (status, body) = delete(
        &router,
        "scr-0ffffffffffffffff",
        &[param("DryRun", "true")],
    )
    .await;
    assert_eq!(
        status, 400,
        "a delete that would have been refused: expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidSubnetCidrReservationID.NotFound",
        "a delete that would have been refused: expected its own reason, got {body}"
    );

    // `DryRun=false` is an ordinary call, which removes the reservation.
    let (status, body) = delete(&router, &id, &[param("DryRun", "false")]).await;
    assert_eq!(
        status, 200,
        "DryRun=false: expected HTTP 200, got {status} with body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "DryRun=false: the reservation must be removed"
    );
}
