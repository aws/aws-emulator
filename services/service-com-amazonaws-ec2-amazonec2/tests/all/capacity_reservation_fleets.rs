// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the Capacity Reservation Fleet slice —
//! `CreateCapacityReservationFleet`, `DescribeCapacityReservationFleets` and
//! `CancelCapacityReservationFleets` — driven over the wire the service actually speaks:
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateCapacityReservationFleet` STORES ONE FLEET and `DescribeCapacityReservationFleets`
//!    REPORTS THAT STORE: what the request sets (target capacity, instance types, zone,
//!    weights, priorities, tenancy, matching criteria, end date, tags) round-trips, and the
//!    members the service owns (the Fleet id, its ARN, its state, its create time, the
//!    Capacity Reservations it reserves through and the capacity they fulfil) are DERIVED
//!    rather than taken — the create's own result answering the Fleet as SUBMITTED, which
//!    holds no Capacity Reservation yet and fulfils none of its target capacity;
//! 2. the CLOCK, not a caller, moves a Fleet `submitted` -> `active`: the capacity is
//!    reserved by the create, so activation moves the Fleet's state word and nothing else;
//! 3. `CancelCapacityReservationFleets` moves a Fleet into `cancelling`, reporting the
//!    Fleet's own previous state beside it, and the clock moves it on to `cancelled`, where
//!    it still reports the composition it reserved;
//! 4. an id that is not a Capacity Reservation Fleet id at all is refused for the WHOLE
//!    request with `InvalidCapacityReservationFleetId.Malformed`, before any Fleet is looked
//!    up or moved, while a well-formed id that names no Fleet is reported PER FLEET by the
//!    cancel and refused by the describe;
//! 5. a request that cannot be served is refused with EC2's own error code and stores
//!    nothing, `DryRun` performs nothing, and one `ClientToken` creates one Fleet;
//! 6. `CapacityReservationFleetIds`, `Filters` and `MaxResults` narrow and page the report,
//!    in `CapacityReservationFleetId` order.
//!
//! Each table row is a distinct input along what the rule generalises over — which member
//! names the zone, which instance type and platform, how many capacity units an instance
//! provides, which priority orders the specifications, how many tags, which malformed id
//! form, which filter, which page size at each published bound and one past it — so no row
//! is the specification.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::ops::types::{
    CAPACITY_RESERVATION_INSTANCE_PLATFORM_VALUES, FLEET_CAPACITY_RESERVATION_TENANCY_VALUES,
    FLEET_INSTANCE_MATCH_CRITERIA_VALUES,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY,
    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE, CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
    CAPACITY_RESERVATION_FLEET_STATE_CANCELLING, CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
    DEFAULT_FLEET_INSTANCE_PRIORITY, FLEET_ALLOCATION_STRATEGY_PRIORITIZED,
    RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET, TRANSITION_DELAY,
};

use super::ec2_query::{call, elements, error_code, filters, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for. A Fleet's ARN names the Region the request was
/// signed for, so it is read back out of that ARN below.
const REGION: &str = "us-west-2";

/// A well-formed Capacity Reservation Fleet id no Fleet of the account is stored under: the
/// mint counts from one, so a high counter value names nothing.
const UNKNOWN_FLEET_ID: &str = "crf-0eeeeeeeeeeeeeeee";

/// How long every clock-owed move of a Fleet takes to fall due: the longest span the
/// service gives one — the Fleet's own activation, which waits on the Capacity Reservations
/// it reserves through ([`CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY`]) — plus a margin so
/// a slow machine cannot read the state before it is due.
async fn past_the_transition() {
    tokio::time::sleep(
        CAPACITY_RESERVATION_FLEET_ACTIVATION_DELAY.max(TRANSITION_DELAY)
            + Duration::from_millis(400),
    )
    .await;
}

/// One `action` call carrying `params`, as its HTTP status and XML body.
async fn api(router: &MultiServiceRouter, action: &str, params: &[Param]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = api(router, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One `DescribeCapacityReservationFleets` call.
async fn describe(router: &MultiServiceRouter, params: &[Param]) -> (u16, String) {
    api(router, "DescribeCapacityReservationFleets", params).await
}

/// One `CancelCapacityReservationFleets` call naming `fleet_ids`.
async fn cancel(router: &MultiServiceRouter, fleet_ids: &[&str]) -> (u16, String) {
    let params = indexed("CapacityReservationFleetId", fleet_ids);
    api(router, "CancelCapacityReservationFleets", &params).await
}

/// How many Fleets the store holds.
fn stored_count(engine: &engine::InMemoryEngine) -> usize {
    engine
        .state::<Ec2State>()
        .lock()
        .capacity_reservation_fleets
        .len()
}

/// One Capacity Reservation of a Fleet, as a response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Reservation {
    capacity_reservation_id: Option<String>,
    instance_type: String,
    instance_platform: String,
    availability_zone: String,
    availability_zone_id: String,
    total_instance_count: i64,
    fulfilled_capacity: f64,
    weight: f64,
    priority: Option<i64>,
    ebs_optimized: String,
    create_date: Option<String>,
}

/// One Capacity Reservation Fleet, as a response reports it.
#[derive(Debug, Clone, PartialEq)]
struct Fleet {
    id: String,
    arn: Option<String>,
    state: String,
    total_target_capacity: i64,
    total_fulfilled_capacity: f64,
    tenancy: String,
    instance_match_criteria: String,
    allocation_strategy: String,
    create_time: String,
    end_date: Option<String>,
    reservations: Vec<Reservation>,
    tags: Vec<(String, String)>,
}

/// The text of a required child element, or a panic naming the case.
fn required(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> String {
    child_text(node, name)
        .unwrap_or_else(|| panic!("case {case:?}: no <{name}> in <{}>", node.tag_name().name()))
}

/// A numeric child element as an `i64`.
fn integer(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> i64 {
    let text = required(case, node, name);
    text.parse()
        .unwrap_or_else(|error| panic!("case {case:?}: <{name}> must be an integer: {error}"))
}

/// A numeric child element as an `f64`.
fn double(case: &str, node: roxmltree::Node<'_, '_>, name: &str) -> f64 {
    let text = required(case, node, name);
    text.parse()
        .unwrap_or_else(|error| panic!("case {case:?}: <{name}> must be a number: {error}"))
}

/// The `<item>` children of `node`'s `set_name` child.
fn set_items<'a, 'i>(
    node: roxmltree::Node<'a, 'i>,
    set_name: &str,
) -> Vec<roxmltree::Node<'a, 'i>> {
    child(node, set_name)
        .map(|set| {
            set.children()
                .filter(|inner| inner.is_element() && inner.tag_name().name() == "item")
                .collect()
        })
        .unwrap_or_default()
}

/// The `<tagSet>` of `node`, as key/value pairs.
fn tags_of(node: roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    set_items(node, "tagSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "key").unwrap_or_default(),
                child_text(item, "value").unwrap_or_default(),
            )
        })
        .collect()
}

/// One Fleet read off a response element, whose Capacity Reservations are under
/// `reservation_set` — `fleetCapacityReservationSet` in a create result,
/// `instanceTypeSpecificationSet` in a describe's Fleet.
fn fleet_from(case: &str, node: roxmltree::Node<'_, '_>, reservation_set: &str) -> Fleet {
    let reservations = set_items(node, reservation_set)
        .into_iter()
        .map(|item| Reservation {
            capacity_reservation_id: child_text(item, "capacityReservationId"),
            instance_type: required(case, item, "instanceType"),
            instance_platform: required(case, item, "instancePlatform"),
            availability_zone: required(case, item, "availabilityZone"),
            availability_zone_id: required(case, item, "availabilityZoneId"),
            total_instance_count: integer(case, item, "totalInstanceCount"),
            fulfilled_capacity: double(case, item, "fulfilledCapacity"),
            weight: double(case, item, "weight"),
            priority: child_text(item, "priority").map(|value| {
                value
                    .parse()
                    .unwrap_or_else(|error| panic!("case {case:?}: <priority>: {error}"))
            }),
            ebs_optimized: required(case, item, "ebsOptimized"),
            create_date: child_text(item, "createDate"),
        })
        .collect();
    Fleet {
        id: required(case, node, "capacityReservationFleetId"),
        arn: child_text(node, "capacityReservationFleetArn"),
        state: required(case, node, "state"),
        total_target_capacity: integer(case, node, "totalTargetCapacity"),
        total_fulfilled_capacity: double(case, node, "totalFulfilledCapacity"),
        tenancy: required(case, node, "tenancy"),
        instance_match_criteria: required(case, node, "instanceMatchCriteria"),
        allocation_strategy: required(case, node, "allocationStrategy"),
        create_time: required(case, node, "createTime"),
        end_date: child_text(node, "endDate"),
        reservations,
        tags: tags_of(node),
    }
}

/// The Fleet a `CreateCapacityReservationFleet` result carries: the response element itself.
fn created_fleet(case: &str, body: &str) -> Fleet {
    let doc = document(case, body);
    fleet_from(case, doc.root_element(), "fleetCapacityReservationSet")
}

/// Every Fleet a `DescribeCapacityReservationFleets` response carries, in document order.
fn described_fleets(case: &str, body: &str) -> Vec<Fleet> {
    let doc = document(case, body);
    let root = doc.root_element();
    set_items(root, "capacityReservationFleetSet")
        .into_iter()
        .map(|item| fleet_from(case, item, "instanceTypeSpecificationSet"))
        .collect()
}

/// The ids a `DescribeCapacityReservationFleets` call reports, in the order it reports them.
async fn described_ids(case: &str, router: &MultiServiceRouter, params: &[Param]) -> Vec<String> {
    let body = ok(case, router, "DescribeCapacityReservationFleets", params).await;
    described_fleets(case, &body)
        .into_iter()
        .map(|fleet| fleet.id)
        .collect()
}

/// The one Fleet a describe narrowed to `fleet_id` reports.
async fn described_fleet(case: &str, router: &MultiServiceRouter, fleet_id: &str) -> Fleet {
    let body = ok(
        case,
        router,
        "DescribeCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[fleet_id]),
    )
    .await;
    let mut fleets = described_fleets(case, &body);
    assert_eq!(
        fleets.len(),
        1,
        "case {case:?}: a describe narrowed to one id reports one Fleet, got {fleets:?}"
    );
    fleets.remove(0)
}

/// Look again until the Fleet `fleet_id` reports `wanted`, exactly as a client's readiness
/// waiter does, and answer the Fleet it then reports.
///
/// A wait on the CONDITION rather than on a span: a loaded machine polls more often instead
/// of reading the state before the move the clock owes is applied. The deadline is generous,
/// because all it bounds is a hang.
async fn polled_fleet(
    case: &str,
    router: &MultiServiceRouter,
    fleet_id: &str,
    wanted: &str,
) -> Fleet {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    let mut fleet = described_fleet(case, router, fleet_id).await;
    while fleet.state != wanted && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(25)).await;
        fleet = described_fleet(case, router, fleet_id).await;
    }
    assert_eq!(
        fleet.state, wanted,
        "case {case:?}: waited for the Fleet to report {wanted:?}, it reports {:?}",
        fleet.state
    );
    fleet
}

/// One `instanceTypeSpecification` of a create request, at `index` (counting from 1).
fn specification(index: usize, fields: &[(&str, String)]) -> Vec<Param> {
    fields
        .iter()
        .map(|(name, value)| {
            (
                format!("InstanceTypeSpecification.{index}.{name}"),
                value.clone(),
            )
        })
        .collect()
}

/// The form fields of one tag of a `capacity-reservation-fleet` tag specification.
fn tag_fields(index: usize, key: &str, value: &str) -> Vec<Param> {
    vec![
        param(
            "TagSpecification.1.ResourceType",
            RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET,
        ),
        param(&format!("TagSpecification.1.Tag.{index}.Key"), key),
        param(&format!("TagSpecification.1.Tag.{index}.Value"), value),
    ]
}

/// Whether `id` is a long-form EC2 id of `prefix` that EC2 could have ISSUED: 17 lowercase
/// hex digits, leading digit `0`, not all `0` — the form `crf-…` ids the mint produces take,
/// and the form a malformed-id refusal is defined against.
fn is_issued_long_id(prefix: &str, id: &str) -> bool {
    let Some(suffix) = id
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('-'))
    else {
        return false;
    };
    suffix.len() == 17
        && suffix.bytes().all(|byte| byte.is_ascii_hexdigit())
        && suffix.bytes().all(|byte| !byte.is_ascii_uppercase())
        && suffix.starts_with('0')
        && suffix.bytes().any(|byte| byte != b'0')
}

/// How many instances it takes to cover `target` capacity units at `weight` units each: the
/// quotient rounded up, since an instance is not reserved in fractions.
fn expected_instance_count(target: i64, weight: f64) -> i64 {
    let count = (target as f64 / weight).ceil();
    count as i64
}

/// One row of the create/describe table: an input that differs from the others along what
/// rule 1 generalises over.
struct CreateCase {
    case: &'static str,
    /// Which member names the zone: `AvailabilityZone` or `AvailabilityZoneId`.
    by_zone_id: bool,
    instance_types: &'static [&'static str],
    instance_platform: &'static str,
    /// The weight of each specification, in the same order as `instance_types`.
    weights: &'static [f64],
    /// The priority of each specification, in the same order as `instance_types`.
    priorities: &'static [Option<i32>],
    total_target_capacity: i64,
    tags: &'static [(&'static str, &'static str)],
    end_date: Option<&'static str>,
    ebs_optimized: bool,
}

/// The rows of the create/describe table.
///
/// The instance types and platforms are read off the modelled catalogs
/// (`crate::ops::types::INSTANCE_TYPE_VALUES` /
/// `CAPACITY_RESERVATION_INSTANCE_PLATFORM_VALUES`) rather than invented, and no row repeats
/// another's combination of zone member, weight, priority order, capacity, tags or end date.
const CREATE_CASES: &[CreateCase] = &[
    CreateCase {
        case: "one instance type, by zone name, unit weight",
        by_zone_id: false,
        instance_types: &["m5.large"],
        instance_platform: "Linux/UNIX",
        weights: &[1.0],
        priorities: &[None],
        total_target_capacity: 3,
        tags: &[],
        end_date: None,
        ebs_optimized: false,
    },
    CreateCase {
        case: "by zone id, fractional weight, an end date and two tags",
        by_zone_id: true,
        instance_types: &["c5.xlarge"],
        instance_platform: "Windows",
        weights: &[2.5],
        priorities: &[Some(0)],
        total_target_capacity: 10,
        tags: &[("Name", "fleet-two"), ("Team", "capacity")],
        end_date: Some("2030-01-02T03:04:05Z"),
        ebs_optimized: true,
    },
    CreateCase {
        case: "three instance types, the LAST one highest priority",
        by_zone_id: false,
        instance_types: &["m5.large", "r5.2xlarge", "c5.large"],
        instance_platform: "Red Hat Enterprise Linux",
        weights: &[1.0, 4.0, 2.0],
        priorities: &[Some(7), Some(3), Some(1)],
        total_target_capacity: 7,
        tags: &[("Name", "fleet-three")],
        end_date: None,
        ebs_optimized: false,
    },
    CreateCase {
        case: "a weight above the target capacity, so one instance covers it",
        by_zone_id: false,
        instance_types: &["m5.24xlarge"],
        instance_platform: "Ubuntu Pro",
        weights: &[99.0],
        priorities: &[Some(2)],
        total_target_capacity: 1,
        tags: &[],
        end_date: None,
        ebs_optimized: false,
    },
];

impl CreateCase {
    /// This row as `CreateCapacityReservationFleet` form fields.
    fn params(&self, zones: &[(String, String)]) -> Vec<Param> {
        let mut params = vec![param(
            "TotalTargetCapacity",
            &self.total_target_capacity.to_string(),
        )];
        for (index, instance_type) in self.instance_types.iter().enumerate() {
            let (zone_name, zone_id) = &zones[0];
            let mut fields: Vec<(&str, String)> = vec![
                ("InstanceType", (*instance_type).to_string()),
                ("InstancePlatform", self.instance_platform.to_string()),
                ("Weight", self.weights[index].to_string()),
                ("EbsOptimized", self.ebs_optimized.to_string()),
            ];
            if self.by_zone_id {
                fields.push(("AvailabilityZoneId", zone_id.clone()));
            } else {
                fields.push(("AvailabilityZone", zone_name.clone()));
            }
            if let Some(priority) = self.priorities[index] {
                fields.push(("Priority", priority.to_string()));
            }
            params.extend(specification(index + 1, &fields));
        }
        for (index, (key, value)) in self.tags.iter().enumerate() {
            params.extend(tag_fields(index + 1, key, value));
        }
        if let Some(end_date) = self.end_date {
            params.push(param("EndDate", end_date));
        }
        params
    }

    /// The specification this row's `prioritized` Fleet reserves through: the one with the
    /// lowest `Priority` value, a specification that assigned none coming last.
    fn chosen(&self) -> (&'static str, f64, Option<i32>) {
        let mut best = 0;
        for index in 1..self.instance_types.len() {
            let current = self.priorities[index].unwrap_or(i32::MAX);
            let incumbent = self.priorities[best].unwrap_or(i32::MAX);
            if current < incumbent {
                best = index;
            }
        }
        (
            self.instance_types[best],
            self.weights[best],
            self.priorities[best],
        )
    }
}

/// The first Availability Zone of [`REGION`], as its name and id, read off
/// `DescribeAvailabilityZones` rather than written down here.
async fn zones(router: &MultiServiceRouter) -> Vec<(String, String)> {
    let body = ok("zones", router, "DescribeAvailabilityZones", &[]).await;
    let names = elements(&body, "zoneName");
    let ids = elements(&body, "zoneId");
    assert!(
        !names.is_empty() && names.len() == ids.len(),
        "the Region must report zones with ids, got {body}"
    );
    names.into_iter().zip(ids).collect()
}

// ---------------------------------------------------------------------------
// Rule 1 and 2: the store round-trips, and the clock activates the Fleet
// ---------------------------------------------------------------------------

/// RULES 1 and 2: every member the request sets round-trips through the create result and
/// the describe, the members the service owns are derived — including the Capacity
/// Reservations the create reserved the target capacity through — and the CLOCK moves the
/// Fleet `submitted` -> `active` without changing the capacity it holds.
#[tokio::test]
async fn create_round_trips_and_the_clock_activates_the_fleet() {
    let (router, engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;
    let (zone_name, zone_id) = zones[0].clone();

    for (index, row) in CREATE_CASES.iter().enumerate() {
        let case = row.case;
        let body = ok(
            case,
            &router,
            "CreateCapacityReservationFleet",
            &row.params(&zones),
        )
        .await;
        let created = created_fleet(case, &body);

        // The create answers the Fleet as SUBMITTED: "Amazon Elastic Compute Cloud is
        // preparing to create the Capacity Reservations", so the result holds none of them
        // and reports none of the target capacity as fulfilled. What the Fleet went on to
        // reserve is what the describe below reports.
        assert!(
            created.reservations.is_empty(),
            "case {case:?}: the create result of a submitted Fleet holds no Capacity \
             Reservation, got {:?}",
            created.reservations
        );
        assert_eq!(
            created.total_fulfilled_capacity, 0.0,
            "case {case:?}: a submitted Fleet has fulfilled no capacity yet, got {}",
            created.total_fulfilled_capacity
        );

        // The describe reports the STORE the create wrote: the Fleet, and the Capacity
        // Reservations it reserves its target capacity through.
        let stored = described_fleet(case, &router, &created.id).await;

        // What the `prioritized` strategy reserves through: the specification with the
        // lowest `Priority` value, covering the whole target capacity at its own weight.
        let (chosen_type, chosen_weight, chosen_priority) = row.chosen();
        let expected_count = expected_instance_count(row.total_target_capacity, chosen_weight);
        let expected_capacity = expected_count as f64 * chosen_weight;

        // The members the SERVICE owns are derived, not taken: the id is one EC2 could have
        // issued, and the Fleet starts in the non-terminal state the clock owes `active`.
        assert!(
            is_issued_long_id("crf", &created.id),
            "case {case:?}: expected an issued crf- id, got {:?}",
            created.id
        );
        assert_eq!(
            created.state, CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
            "case {case:?}: a new Fleet starts submitted, got {:?}",
            created.state
        );
        assert_eq!(
            stored.reservations.len(),
            row.instance_types.len(),
            "case {case:?}: every instance type specification is reported, got {:?}",
            stored.reservations
        );
        // The specification the `prioritized` strategy reserved on carries the Capacity
        // Reservation; the others carry none, and reserve nothing.
        for (index, reservation) in stored.reservations.iter().enumerate() {
            let is_chosen = row.instance_types[index] == chosen_type
                && row.weights[index] == chosen_weight
                && row.priorities[index] == chosen_priority;
            let expected_here = if is_chosen { expected_capacity } else { 0.0 };
            let expected_count_here = if is_chosen { expected_count } else { 0 };
            assert_eq!(
                (
                    reservation.instance_type.as_str(),
                    reservation.instance_platform.as_str(),
                    reservation.availability_zone.as_str(),
                    reservation.availability_zone_id.as_str(),
                    reservation.total_instance_count,
                    reservation.priority.map(|priority| priority as i32),
                    reservation.ebs_optimized.as_str(),
                ),
                (
                    row.instance_types[index],
                    row.instance_platform,
                    zone_name.as_str(),
                    zone_id.as_str(),
                    expected_count_here,
                    // Every reservation reports a priority: the one the specification
                    // assigned, or the Fleet's default where it assigned none.
                    Some(
                        row.priorities[index].unwrap_or(DEFAULT_FLEET_INSTANCE_PRIORITY),
                    ),
                    if row.ebs_optimized { "true" } else { "false" },
                ),
                "case {case:?}: specification {index} must report its own facts, \
                 got {reservation:?}"
            );
            assert!(
                (reservation.weight - row.weights[index]).abs() < 1e-9,
                "case {case:?}: specification {index} must report the weight the request \
                 named ({}), got {}",
                row.weights[index],
                reservation.weight
            );
            assert!(
                (reservation.fulfilled_capacity - expected_here).abs() < 1e-9,
                "case {case:?}: specification {index} must fulfil {expected_here} capacity \
                 units, got {}",
                reservation.fulfilled_capacity
            );
            assert_eq!(
                reservation.capacity_reservation_id.is_some(),
                is_chosen,
                "case {case:?}: specification {index} holds a Capacity Reservation only \
                 where capacity was reserved on it, got {reservation:?}"
            );
            if let Some(capacity_reservation_id) = reservation.capacity_reservation_id.as_deref() {
                assert!(
                    is_issued_long_id("cr", capacity_reservation_id),
                    "case {case:?}: expected an issued cr- id, got {capacity_reservation_id:?}"
                );
            }
        }
        assert!(
            (stored.total_fulfilled_capacity - expected_capacity).abs() < 1e-9,
            "case {case:?}: the Fleet's fulfilled capacity is its reservations', \
             expected {expected_capacity}, got {}",
            stored.total_fulfilled_capacity
        );
        assert!(
            stored.total_fulfilled_capacity >= row.total_target_capacity as f64,
            "case {case:?}: the create reserves at least the target capacity, got {}",
            stored.total_fulfilled_capacity
        );
        // What the REQUEST set round-trips.
        assert_eq!(
            created.total_target_capacity, row.total_target_capacity,
            "case {case:?}: expected the target capacity the request named, got {}",
            created.total_target_capacity
        );
        assert_eq!(
            created.allocation_strategy, FLEET_ALLOCATION_STRATEGY_PRIORITIZED,
            "case {case:?}: expected the only supported allocation strategy, got {:?}",
            created.allocation_strategy
        );
        assert_eq!(
            created.instance_match_criteria, FLEET_INSTANCE_MATCH_CRITERIA_VALUES[0],
            "case {case:?}: expected the modelled matching criteria, got {:?}",
            created.instance_match_criteria
        );
        assert_eq!(
            created.tenancy, FLEET_CAPACITY_RESERVATION_TENANCY_VALUES[0],
            "case {case:?}: expected the modelled tenancy, got {:?}",
            created.tenancy
        );
        let expected_tags: Vec<(String, String)> = row
            .tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {case:?}: expected the tags the request assigned, got {:?}",
            created.tags
        );
        match row.end_date {
            None => assert!(
                created.end_date.is_none(),
                "case {case:?}: a Fleet with no end date reports none, got {:?}",
                created.end_date
            ),
            Some(end_date) => {
                let reported = created.end_date.clone().unwrap_or_default();
                let instant = end_date.trim_end_matches('Z');
                assert!(
                    reported.starts_with(instant),
                    "case {case:?}: expected the end date {instant:?}, got {reported:?}"
                );
            }
        }

        // The describe reports THAT store: the members the request set are reported the same
        // by the create result and by the describe.
        assert_eq!(
            stored.arn.as_deref(),
            Some(
                format!(
                    "arn:aws:ec2:{REGION}:000000000000:{RESOURCE_TYPE_CAPACITY_RESERVATION_FLEET}/{}",
                    created.id
                )
                .as_str()
            ),
            "case {case:?}: expected an EC2 ARN in the signed Region, got {:?}",
            stored.arn
        );
        assert_eq!(
            (
                stored.state.clone(),
                stored.total_target_capacity,
                stored.tenancy.clone(),
                stored.instance_match_criteria.clone(),
                stored.allocation_strategy.clone(),
                stored.create_time.clone(),
                stored.end_date.clone(),
                stored.tags.clone(),
            ),
            (
                created.state.clone(),
                created.total_target_capacity,
                created.tenancy.clone(),
                created.instance_match_criteria.clone(),
                created.allocation_strategy.clone(),
                created.create_time.clone(),
                created.end_date.clone(),
                created.tags.clone(),
            ),
            "case {case:?}: the describe must report the Fleet the create stored, \
             got {stored:?} against {created:?}"
        );

        // RULE 2: the clock, not a caller, activates the Fleet — and activation is a state
        // word, the capacity having been reserved by the create.
        past_the_transition().await;
        let active = described_fleet(case, &router, &created.id).await;
        assert_eq!(
            active.state, CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
            "case {case:?}: the clock owes a submitted Fleet active, got {:?}",
            active.state
        );
        assert_eq!(
            (
                active.reservations.clone(),
                active.total_target_capacity,
                active.create_time.clone(),
            ),
            (
                stored.reservations.clone(),
                stored.total_target_capacity,
                stored.create_time.clone(),
            ),
            "case {case:?}: activation moves the state word and nothing else, got {active:?}"
        );
        assert!(
            (active.total_fulfilled_capacity - expected_capacity).abs() < 1e-9,
            "case {case:?}: an active Fleet still holds {expected_capacity} capacity units, \
             got {}",
            active.total_fulfilled_capacity
        );
        assert_eq!(
            stored_count(&engine),
            index + 1,
            "case {case:?}: each create stores exactly one Fleet"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: cancelling moves the Fleet through `cancelling` to `cancelled`
// ---------------------------------------------------------------------------

/// RULE 3: a cancel reports the Fleet's OWN previous state — the word it had moved out of
/// before the state the cancel interrupts — and leaves it `cancelling`, and the CLOCK moves
/// it on to `cancelled`, where it still reports the composition it reserved.
///
/// The rows are the two states a Fleet of this emulator can be cancelled from: the
/// `submitted` it is created in, and the `active` the clock moves it to. Both report
/// `submitted` as the previous state — a Fleet that has not left the state it was created in
/// has no earlier word, and one the clock activated moved out of exactly that word. The
/// histories that leave a DIFFERENT word behind are tabled in
/// `super::capacity_reservation_fleet_previous_state`.
#[tokio::test]
async fn cancelling_a_fleet_moves_it_through_cancelling_to_cancelled() {
    let (router, _engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;

    for (case, wait_for_active) in [
        ("cancelled while submitted", false),
        ("cancelled while active", true),
    ] {
        let body = ok(
            case,
            &router,
            "CreateCapacityReservationFleet",
            &CREATE_CASES[0].params(&zones),
        )
        .await;
        let fleet_id = created_fleet(case, &body).id;
        if wait_for_active {
            polled_fleet(
                case,
                &router,
                &fleet_id,
                CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
            )
            .await;
        }
        // Either way the word the Fleet had moved out of by the time the cancel arrives is
        // the one it was created in.
        let expected_previous = CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED;

        let (status, body) = cancel(&router, &[&fleet_id]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the cancel, got {status} with body {body}"
        );
        let doc = document(case, &body);
        let successful = set_items(doc.root_element(), "successfulFleetCancellationSet");
        assert_eq!(
            successful.len(),
            1,
            "case {case:?}: one id cancelled one Fleet, got body {body}"
        );
        assert_eq!(
            (
                child_text(successful[0], "capacityReservationFleetId"),
                child_text(successful[0], "previousFleetState"),
                child_text(successful[0], "currentFleetState"),
            ),
            (
                Some(fleet_id.clone()),
                Some(expected_previous.to_string()),
                Some(CAPACITY_RESERVATION_FLEET_STATE_CANCELLING.to_string()),
            ),
            "case {case:?}: expected the Fleet's previous state {expected_previous:?} beside \
             the {CAPACITY_RESERVATION_FLEET_STATE_CANCELLING:?} the cancel moves it to, got \
             body {body}"
        );
        assert!(
            set_items(doc.root_element(), "failedFleetCancellationSet").is_empty(),
            "case {case:?}: nothing failed, got body {body}"
        );

        // The transient state is observable, which is what a caller polling right after the
        // call sees.
        let cancelling = described_fleet(case, &router, &fleet_id).await;
        assert_eq!(
            cancelling.state, CAPACITY_RESERVATION_FLEET_STATE_CANCELLING,
            "case {case:?}: a cancelled Fleet is first cancelling, got {:?}",
            cancelling.state
        );

        let cancelled = polled_fleet(
            case,
            &router,
            &fleet_id,
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
        )
        .await;
        // Cancelling is a state WORD: the Fleet's composition — the specifications it
        // reserved on, the Capacity Reservations they held and the capacity they fulfilled —
        // is still reported, so a teardown flow can read what was reserved.
        assert_eq!(
            (
                cancelled.reservations.clone(),
                cancelled.total_fulfilled_capacity,
                cancelled.total_target_capacity,
            ),
            (
                cancelling.reservations.clone(),
                cancelling.total_fulfilled_capacity,
                cancelling.total_target_capacity,
            ),
            "case {case:?}: a cancelled Fleet still reports what it reserved, \
             got {cancelled:?}"
        );
        assert!(
            !cancelled.reservations.is_empty() && cancelled.total_fulfilled_capacity > 0.0,
            "case {case:?}: the Fleet under test reserved capacity, so the cancelled Fleet \
             has something to report, got {cancelled:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 4: malformed ids are refused for the whole request; unknown ids are not
// ---------------------------------------------------------------------------

/// The malformed Fleet id forms, each a distinct way of being outside the form EC2 issues.
///
/// The first row is the id one recorded unit probes
/// (`issues/.../20261023T102725Z-ec2-cancelcapacityreservationfleets-stub.md`) and the second
/// the id another does
/// (`issues/.../20261023T102725Z-ec2-modifycapacityreservationfleet-stub.md`); the rest span
/// the other ways an id can fail the form.
const MALFORMED_FLEET_IDS: [(&str, &str); 7] = [
    ("a leading digit above zero", "crf-fffffffffffffffff"),
    ("every digit zero", "crf-00000000000000000"),
    ("the retired short form", "crf-0abcdef1"),
    ("a non-hexadecimal digit", "crf-0zzzzzzzzzzzzzzzz"),
    ("eighteen digits", "crf-0123456789abcdef01"),
    ("another resource's prefix", "cr-0123456789abcdef0"),
    ("no prefix at all", "0123456789abcdef0"),
];

/// RULE 4: an id that is not a Capacity Reservation Fleet id is refused for the WHOLE request
/// with `InvalidCapacityReservationFleetId.Malformed`, by the cancel and by the describe
/// alike, and the cancel moves nothing when it answers that.
#[tokio::test]
async fn a_malformed_fleet_id_is_refused_before_any_fleet_is_touched() {
    let (router, _engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;
    let body = ok(
        "a Fleet to leave alone",
        &router,
        "CreateCapacityReservationFleet",
        &CREATE_CASES[0].params(&zones),
    )
    .await;
    let fleet_id = created_fleet("a Fleet to leave alone", &body).id;
    past_the_transition().await;

    for (case, malformed) in MALFORMED_FLEET_IDS {
        // Named ALONE, and named BESIDE a Fleet that could have been cancelled: the refusal
        // is the request's, so the good id is not cancelled either.
        for (label, ids) in [
            (format!("{case} alone"), vec![malformed]),
            (
                format!("{case} beside a known Fleet"),
                vec![fleet_id.as_str(), malformed],
            ),
        ] {
            let (status, body) = cancel(&router, &ids).await;
            assert_eq!(
                status, 400,
                "case {label:?}: expected HTTP 400 for {malformed:?}, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidCapacityReservationFleetId.Malformed",
                "case {label:?}: expected the malformed-id code, got body {body}"
            );
            let described = described_fleet(&label, &router, &fleet_id).await;
            assert_eq!(
                described.state, CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
                "case {label:?}: a refused request cancels nothing, got {:?}",
                described.state
            );
        }

        let (status, body) = describe(
            &router,
            &indexed("CapacityReservationFleetId", &[malformed]),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: the describe refuses {malformed:?} too, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidCapacityReservationFleetId.Malformed",
            "case {case:?}: expected the malformed-id code from the describe, got body {body}"
        );
    }
}

/// RULE 4: a well-formed id that names no Fleet is ONE Fleet that could not be cancelled —
/// the others named in the same call still are — while the describe refuses the request; and
/// a Fleet whose cancellation has already happened is reported as `IncorrectState` rather
/// than cancelled twice.
#[tokio::test]
async fn an_unknown_or_already_cancelled_fleet_is_reported_per_fleet() {
    let (router, _engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;
    let body = ok(
        "a Fleet to cancel",
        &router,
        "CreateCapacityReservationFleet",
        &CREATE_CASES[0].params(&zones),
    )
    .await;
    let fleet_id = created_fleet("a Fleet to cancel", &body).id;
    past_the_transition().await;

    // One call naming a known Fleet and an unknown one: the known Fleet is cancelled and the
    // unknown id is reported beside it.
    let case = "an unknown id beside a known Fleet";
    let (status, body) = cancel(&router, &[fleet_id.as_str(), UNKNOWN_FLEET_ID]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a per-Fleet failure is not a request failure, got {status} with \
         body {body}"
    );
    let doc = document(case, &body);
    let successful = set_items(doc.root_element(), "successfulFleetCancellationSet");
    assert_eq!(
        successful
            .iter()
            .filter_map(|item| child_text(*item, "capacityReservationFleetId"))
            .collect::<Vec<String>>(),
        vec![fleet_id.clone()],
        "case {case:?}: the known Fleet is still cancelled, got body {body}"
    );
    let failed = set_items(doc.root_element(), "failedFleetCancellationSet");
    assert_eq!(
        failed.len(),
        1,
        "case {case:?}: one id named no Fleet, got body {body}"
    );
    let error = child(failed[0], "cancelCapacityReservationFleetError")
        .unwrap_or_else(|| panic!("case {case:?}: no error on the failed entry: {body}"));
    assert_eq!(
        (
            child_text(failed[0], "capacityReservationFleetId"),
            child_text(error, "code"),
        ),
        (
            Some(UNKNOWN_FLEET_ID.to_string()),
            Some("InvalidCapacityReservationFleetId.NotFound".to_string()),
        ),
        "case {case:?}: expected the unknown id and its code, got body {body}"
    );
    assert!(
        child_text(error, "message").is_some_and(|message| message.contains(UNKNOWN_FLEET_ID)),
        "case {case:?}: the message must name the id, got body {body}"
    );

    // The same Fleet again, while it is `cancelling`, and once more after the clock has made
    // it `cancelled`: neither is a state a cancellation runs from.
    for case in ["cancelling again", "cancelled again"] {
        if case == "cancelled again" {
            past_the_transition().await;
        }
        let (status, body) = cancel(&router, &[fleet_id.as_str()]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let doc = document(case, &body);
        assert!(
            set_items(doc.root_element(), "successfulFleetCancellationSet").is_empty(),
            "case {case:?}: nothing was cancelled a second time, got body {body}"
        );
        let failed = set_items(doc.root_element(), "failedFleetCancellationSet");
        assert_eq!(
            failed.len(),
            1,
            "case {case:?}: the Fleet is reported as one that could not be cancelled, \
             got body {body}"
        );
        let error = child(failed[0], "cancelCapacityReservationFleetError")
            .unwrap_or_else(|| panic!("case {case:?}: no error on the failed entry: {body}"));
        assert_eq!(
            child_text(error, "code"),
            Some("IncorrectState".to_string()),
            "case {case:?}: expected the wrong-state code, got body {body}"
        );
    }

    // The describe resolves ids rather than reporting per Fleet, so an unknown one is the
    // request's refusal.
    let (status, body) = describe(
        &router,
        &indexed("CapacityReservationFleetId", &[UNKNOWN_FLEET_ID]),
    )
    .await;
    assert_eq!(
        status, 400,
        "case \"an unknown id in a describe\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidCapacityReservationFleetId.NotFound",
        "case \"an unknown id in a describe\": expected the not-found code, got body {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 5: refusals, DryRun and idempotency
// ---------------------------------------------------------------------------

/// RULE 5: a create that cannot be served is refused with EC2's own code and stores nothing.
///
/// The rows differ in WHICH part of the request cannot be served — a capacity, a catalog
/// word, a zone, two members that cannot hold together, a tag specification, an end date —
/// not in the value of one of them.
#[tokio::test]
async fn a_create_that_cannot_be_served_is_refused_and_stores_nothing() {
    let (router, engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;
    let (zone_name, zone_id) = zones[0].clone();
    let other_zone = zones[1].0.clone();
    let platform = CAPACITY_RESERVATION_INSTANCE_PLATFORM_VALUES[0];

    let one_spec = |fields: Vec<(&str, String)>| specification(1, &fields);
    let rows: Vec<(&str, Vec<Param>, &str)> = vec![
        (
            "no capacity units at all",
            [
                vec![param("TotalTargetCapacity", "0")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "no instance type specification",
            vec![param("TotalTargetCapacity", "4")],
            "MissingParameter",
        ),
        (
            // `ReservationFleetInstanceSpecification$InstanceType` targets the modelled
            // `InstanceType` enum, so a value outside that catalog is refused BEFORE this
            // handler runs — with the code and sentence EC2 words for that parameter type,
            // which `crate::service`'s refusal override states for the whole service.
            "an instance type EC2 does not offer",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.enormous".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            // `$InstancePlatform` targets the modelled
            // `CapacityReservationInstancePlatform` enum, refused at the same boundary.
            "an operating system outside the modelled catalog",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", "Plan 9".to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "ValidationError",
        ),
        (
            "both the zone and its id",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                    ("AvailabilityZoneId", zone_id.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterCombination",
        ),
        (
            "neither the zone nor its id",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                ]),
            ]
            .concat(),
            "MissingParameter",
        ),
        (
            "a zone of another Region",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", "eu-west-3c".to_string()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "two specifications in two zones",
            [
                vec![param("TotalTargetCapacity", "4")],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
                specification(
                    2,
                    &[
                        ("InstanceType", "c5.large".to_string()),
                        ("InstancePlatform", platform.to_string()),
                        ("AvailabilityZone", other_zone.clone()),
                    ],
                ),
            ]
            .concat(),
            "InvalidParameterCombination",
        ),
        (
            "an allocation strategy that is not prioritized",
            [
                vec![
                    param("TotalTargetCapacity", "4"),
                    param("AllocationStrategy", "lowest-price"),
                ],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "an end date already behind us",
            [
                vec![
                    param("TotalTargetCapacity", "4"),
                    param("EndDate", "2001-02-03T04:05:06Z"),
                ],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
        (
            "tags for another resource type",
            [
                vec![
                    param("TotalTargetCapacity", "4"),
                    param("TagSpecification.1.ResourceType", "instance"),
                    param("TagSpecification.1.Tag.1.Key", "Name"),
                    param("TagSpecification.1.Tag.1.Value", "nope"),
                ],
                one_spec(vec![
                    ("InstanceType", "m5.large".to_string()),
                    ("InstancePlatform", platform.to_string()),
                    ("AvailabilityZone", zone_name.clone()),
                ]),
            ]
            .concat(),
            "InvalidParameterValue",
        ),
    ];

    for (case, params, expected_code) in rows {
        let (status, body) = api(&router, "CreateCapacityReservationFleet", &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got body {body}"
        );
        assert_eq!(
            stored_count(&engine),
            0,
            "case {case:?}: a refused create stores nothing"
        );
    }
}

/// RULE 5: `DryRun` performs nothing — it answers `DryRunOperation` and neither creates nor
/// cancels — and one `ClientToken` creates one Fleet however often the request is replayed.
#[tokio::test]
async fn dry_run_performs_nothing_and_a_client_token_creates_one_fleet() {
    let (router, engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;

    // A create under DryRun stores nothing.
    let mut params = CREATE_CASES[0].params(&zones);
    params.push(param("DryRun", "true"));
    let (status, body) = api(&router, "CreateCapacityReservationFleet", &params).await;
    assert_eq!(
        status, 412,
        "case \"create under DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"create under DryRun\": expected DryRunOperation, got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        0,
        "case \"create under DryRun\": nothing is stored"
    );

    // One token, two identical creates: one Fleet.
    let mut tokened = CREATE_CASES[0].params(&zones);
    tokened.push(param("ClientToken", "one-token"));
    let first = created_fleet(
        "first create",
        &ok(
            "first create",
            &router,
            "CreateCapacityReservationFleet",
            &tokened,
        )
        .await,
    );
    let replayed = created_fleet(
        "replayed create",
        &ok(
            "replayed create",
            &router,
            "CreateCapacityReservationFleet",
            &tokened,
        )
        .await,
    );
    assert_eq!(
        replayed.id, first.id,
        "case \"replayed create\": one ClientToken names one Fleet"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case \"replayed create\": a replay creates nothing"
    );

    // A dry run is never answered with a success body — not even the one the token above
    // would replay: the flag asks whether the request WOULD succeed, and nothing is done.
    let mut dry_replay = tokened.clone();
    dry_replay.push(param("DryRun", "true"));
    let (status, body) = api(&router, "CreateCapacityReservationFleet", &dry_replay).await;
    assert_eq!(
        status, 412,
        "case \"DryRun with a used ClientToken\": expected HTTP 412, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "DryRunOperation",
        "case \"DryRun with a used ClientToken\": expected DryRunOperation, got body {body}"
    );
    assert_eq!(
        stored_count(&engine),
        1,
        "case \"DryRun with a used ClientToken\": nothing further is stored"
    );

    // A cancel and a describe under DryRun change nothing either.
    let mut cancel_params = indexed("CapacityReservationFleetId", &[first.id.as_str()]);
    cancel_params.push(param("DryRun", "true"));
    let (status, body) = api(&router, "CancelCapacityReservationFleets", &cancel_params).await;
    assert_eq!(
        status, 412,
        "case \"cancel under DryRun\": expected HTTP 412, got {status} with body {body}"
    );
    let still = described_fleet("cancel under DryRun", &router, &first.id).await;
    assert_ne!(
        still.state, CAPACITY_RESERVATION_FLEET_STATE_CANCELLING,
        "case \"cancel under DryRun\": a dry run cancels nothing"
    );
    let (status, body) = describe(&router, &[param("DryRun", "true")]).await;
    assert_eq!(
        status, 412,
        "case \"describe under DryRun\": expected HTTP 412, got {status} with body {body}"
    );
}

// ---------------------------------------------------------------------------
// Rule 6: ids, filters and pages narrow the report
// ---------------------------------------------------------------------------

/// RULE 6: `CapacityReservationFleetIds` and `Filters` narrow the report and `MaxResults`
/// pages it, in `CapacityReservationFleetId` order.
///
/// The filter rows are every name the operation documents, each asked with a value that
/// matches and a value that does not, so a filter that selected nothing could not pass for a
/// filter that was ignored. The page sizes are the published bounds of the modelled
/// `MaxResults` range and one past each.
#[tokio::test]
async fn ids_filters_and_pages_narrow_the_report() {
    let (router, _engine) = super::ec2_query::emulator();
    let zones = zones(&router).await;

    let mut fleet_ids: Vec<String> = Vec::new();
    for row in CREATE_CASES {
        let body = ok(
            row.case,
            &router,
            "CreateCapacityReservationFleet",
            &row.params(&zones),
        )
        .await;
        fleet_ids.push(created_fleet(row.case, &body).id);
    }
    fleet_ids.sort();

    // Unnarrowed: every Fleet, in id order.
    let reported = described_ids("every Fleet", &router, &[]).await;
    assert_eq!(
        reported, fleet_ids,
        "case \"every Fleet\": expected every Fleet in id order"
    );

    // Narrowed by ids: one, two, and all of them.
    for (case, wanted) in [
        ("one id", vec![fleet_ids[1].clone()]),
        (
            "two ids",
            vec![fleet_ids[0].clone(), fleet_ids[2].clone()],
        ),
        ("every id", fleet_ids.clone()),
    ] {
        let params = indexed(
            "CapacityReservationFleetId",
            &wanted.iter().map(String::as_str).collect::<Vec<&str>>(),
        );
        let reported = described_ids(case, &router, &params).await;
        let mut expected = wanted.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {case:?}: expected exactly the Fleets the request named"
        );
    }

    // Every documented filter, with a value that matches every Fleet and one that matches
    // none. The states differ once one Fleet is cancelled, which is what the `state` rows
    // read.
    let cancelled = fleet_ids[0].clone();
    let (status, body) = cancel(&router, &[cancelled.as_str()]).await;
    assert_eq!(
        status, 200,
        "cancelling one Fleet must succeed, got {status} with body {body}"
    );
    // Past the transition the store owes NOTHING: the cancelled Fleet has reached the
    // terminal `cancelled` and the others the stable `active`, so the `state` rows below
    // compare against states no clock can move out from under them.
    past_the_transition().await;
    let still_active: Vec<String> = fleet_ids
        .iter()
        .filter(|id| **id != cancelled)
        .cloned()
        .collect();
    let rows: Vec<(&str, &str, &str, Vec<String>)> = vec![
        (
            "allocation-strategy matches",
            "allocation-strategy",
            FLEET_ALLOCATION_STRATEGY_PRIORITIZED,
            fleet_ids.clone(),
        ),
        (
            "allocation-strategy does not match",
            "allocation-strategy",
            "lowest-price",
            Vec::new(),
        ),
        (
            "instance-match-criteria matches",
            "instance-match-criteria",
            FLEET_INSTANCE_MATCH_CRITERIA_VALUES[0],
            fleet_ids.clone(),
        ),
        (
            "instance-match-criteria does not match",
            "instance-match-criteria",
            "targeted",
            Vec::new(),
        ),
        (
            "tenancy matches",
            "tenancy",
            FLEET_CAPACITY_RESERVATION_TENANCY_VALUES[0],
            fleet_ids.clone(),
        ),
        (
            "tenancy does not match",
            "tenancy",
            "dedicated",
            Vec::new(),
        ),
        (
            "state selects the cancelled Fleet",
            "state",
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
            vec![cancelled.clone()],
        ),
        (
            "state selects the Fleets that are not cancelled",
            "state",
            CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
            still_active.clone(),
        ),
        (
            "state selects no Fleet that has left submitted",
            "state",
            CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
            Vec::new(),
        ),
    ];
    for (case, name, value, expected) in rows {
        let params = filters(&[(name, &[value])]);
        let reported = described_ids(case, &router, &params).await;
        assert_eq!(
            reported, expected,
            "case {case:?}: the filter {name}={value} must select exactly those Fleets"
        );
    }

    // A filter name that is neither one this operation documents nor one of the tag filter
    // family every describe of a taggable resource answers is refused rather than read as
    // "no results".
    let (status, body) = describe(&router, &filters(&[("fleet-name", &["fleet-two"])])).await;
    assert_eq!(
        status, 400,
        "case \"an undocumented filter\": expected HTTP 400, got {status} with body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidParameterValue",
        "case \"an undocumented filter\": expected InvalidParameterValue, got body {body}"
    );

    // Paging: every page holds at most `MaxResults` Fleets, and the walk reports each
    // exactly once in id order.
    for page_size in [1_usize, 2, 100] {
        let case = format!("MaxResults={page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![param("MaxResults", &page_size.to_string())];
            if let Some(token) = token.as_deref() {
                params.push(param("NextToken", token));
            }
            let body = ok(&case, &router, "DescribeCapacityReservationFleets", &params).await;
            let page = described_fleets(&case, &body);
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page holds at most MaxResults Fleets, got {}",
                page.len()
            );
            walked.extend(page.into_iter().map(|fleet| fleet.id));
            token = elements(&body, "nextToken").into_iter().next();
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, fleet_ids,
            "case {case:?}: the walk reports every Fleet exactly once in id order"
        );
    }

    // The modelled `@range` on `MaxResults` (1..100) is refused before the handler runs, so
    // only the status is this operation's to promise; each published bound is accepted.
    for (max_results, expected_status) in [("1", 200), ("100", 200), ("0", 400), ("101", 400)] {
        let (status, body) = describe(&router, &[param("MaxResults", max_results)]).await;
        assert_eq!(
            status, expected_status,
            "case MaxResults={max_results}: expected HTTP {expected_status}, got {status} \
             with body {body}"
        );
    }

    // A token this service did not mint is refused rather than read as an id.
    let (status, body) = describe(&router, &[param("NextToken", "not-a-token")]).await;
    assert_eq!(
        status, 400,
        "case \"a token this service did not mint\": expected HTTP 400, got {status} with \
         body {body}"
    );
    assert_eq!(
        error_code(&body),
        "InvalidPaginationToken",
        "case \"a token this service did not mint\": expected InvalidPaginationToken, \
         got body {body}"
    );
}
