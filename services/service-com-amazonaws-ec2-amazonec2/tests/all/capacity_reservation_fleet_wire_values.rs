// SPDX-License-Identifier: CC0-1.0

//! How this service writes a FLOATING-POINT member on the ec2Query wire, and the two
//! members a Capacity Reservation Fleet derives that depend on it — driven over the wire
//! the service actually speaks through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. a Smithy `double` (or `float`) member is written as a DECIMAL carrying a fractional
//!    part — `1.0`, `4.0`, `99.999`, never `1`, `4` — which is the XML form AWS answers
//!    with for a floating-point member, while an INTEGER member keeps its own form;
//! 2. the rule belongs to this service's response envelope rather than to any one shape,
//!    so it holds for a Spot Fleet request's weighted and fulfilled capacity exactly as it
//!    does for a Capacity Reservation Fleet's;
//! 3. every Capacity Reservation of a Fleet reports a `priority`: the one its instance
//!    type specification assigned, or the Fleet's default where it assigned none.
//!
//! The rows of each table differ along what the rule generalises over: whether the value
//! is whole or fractional, how many fractional digits it carries, its magnitude, each
//! published bound of the member's modelled `@range` and one past it, and — for the
//! priority — whether the request assigned one at all and where it sits in the ordering.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{DEFAULT_FLEET_INSTANCE_PRIORITY, TRANSITION_DELAY};

use super::ec2_query::{call, element, elements, indexed};
use super::wire::{child, child_text, document, param, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The instance type and platform the fleet rows reserve capacity for: members of the
/// modelled `InstanceType` and `CapacityReservationInstancePlatform` catalogs rather than
/// invented values. Which type a row names is not what any rule here generalises over, so
/// every row names the same one.
const INSTANCE_TYPE: &str = "m5.large";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";

/// The ARN of the IAM role a Spot Fleet request grants the fleet; its account is the
/// caller's rather than this emulator's.
const FLEET_ROLE: &str = "arn:aws:iam::111122223333:role/aws-ec2-spot-fleet-tagging-role";

/// One `action` call that has to succeed, and the body it answered.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The first Availability Zone of [`REGION`], read off `DescribeAvailabilityZones` rather
/// than written down here.
async fn first_zone(router: &MultiServiceRouter) -> String {
    let body = ok("zones", router, "DescribeAvailabilityZones", &[]).await;
    let names = elements(&body, "zoneName");
    assert!(
        !names.is_empty(),
        "the Region must report Availability Zones, got {body}"
    );
    names[0].clone()
}

/// A `CreateCapacityReservationFleet` request for one instance type specification of
/// `weight` (left unset when `None`) and `priority` (likewise), reserving `target`
/// capacity units in `zone`.
fn create_params(
    zone: &str,
    target: i64,
    weight: Option<&str>,
    priority: Option<i32>,
) -> Vec<Param> {
    let mut params = vec![
        param("TotalTargetCapacity", &target.to_string()),
        param("InstanceTypeSpecification.1.InstanceType", INSTANCE_TYPE),
        param(
            "InstanceTypeSpecification.1.InstancePlatform",
            INSTANCE_PLATFORM,
        ),
        param("InstanceTypeSpecification.1.AvailabilityZone", zone),
    ];
    if let Some(weight) = weight {
        params.push(param("InstanceTypeSpecification.1.Weight", weight));
    }
    if let Some(priority) = priority {
        params.push(param(
            "InstanceTypeSpecification.1.Priority",
            &priority.to_string(),
        ));
    }
    params
}

/// Create one Fleet and read it back through `DescribeCapacityReservationFleets`, as the
/// response body the describe answered.
async fn create_and_describe(
    case: &str,
    router: &MultiServiceRouter,
    params: &[Param],
) -> String {
    let created = ok(case, router, "CreateCapacityReservationFleet", params).await;
    let fleet_id = element(&created, "capacityReservationFleetId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a Fleet id: {created}"));
    ok(
        case,
        router,
        "DescribeCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[&fleet_id]),
    )
    .await
}

/// The text of the one `<name>` element a body carries, as the service WROTE it — the
/// lexical form, not a parsed number.
fn one_text(case: &str, body: &str, name: &str) -> String {
    let found = elements(body, name);
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one <{name}>, got {found:?} in {body}"
    );
    found[0].clone()
}

// ---------------------------------------------------------------------------
// Rule 1: a floating-point member is written as a decimal
// ---------------------------------------------------------------------------

/// One row of the decimal table: a Fleet whose weight and target capacity make the
/// floating-point members take a different value, and the text each must be written as.
struct DecimalCase {
    case: &'static str,
    /// `Weight`, as the request spells it, or `None` to leave it unset.
    weight: Option<&'static str>,
    target: i64,
    /// The `<weight>` text the reservation must report.
    expected_weight: &'static str,
    /// The `<totalInstanceCount>` text — an INTEGER member, which carries no fractional
    /// part.
    expected_instance_count: &'static str,
    /// The `<fulfilledCapacity>` and `<totalFulfilledCapacity>` text.
    expected_capacity: &'static str,
}

/// The rows of the decimal table.
///
/// The weights span the published bounds of the modelled `@range` on
/// `ReservationFleetInstanceSpecification$Weight` (min 0.001, max 99.999 —
/// `smithy_query(query="shape",shape="FleetCapacityReservation")`), a whole weight, a
/// defaulted one, and fractional weights with one and two digits; the targets make the
/// capacity whole in some rows and fractional in others.
const DECIMAL_CASES: &[DecimalCase] = &[
    DecimalCase {
        case: "a defaulted weight, one capacity unit",
        weight: None,
        target: 1,
        expected_weight: "1.0",
        expected_instance_count: "1",
        expected_capacity: "1.0",
    },
    DecimalCase {
        case: "a whole weight, four capacity units",
        weight: Some("1"),
        target: 4,
        expected_weight: "1.0",
        expected_instance_count: "4",
        expected_capacity: "4.0",
    },
    DecimalCase {
        case: "a weight of one fractional digit, a whole capacity",
        weight: Some("0.5"),
        target: 3,
        expected_weight: "0.5",
        expected_instance_count: "6",
        expected_capacity: "3.0",
    },
    DecimalCase {
        case: "a weight of one fractional digit, a fractional capacity",
        weight: Some("1.5"),
        target: 4,
        expected_weight: "1.5",
        expected_instance_count: "3",
        expected_capacity: "4.5",
    },
    DecimalCase {
        case: "a weight of two fractional digits",
        weight: Some("2.25"),
        target: 9,
        expected_weight: "2.25",
        expected_instance_count: "4",
        expected_capacity: "9.0",
    },
    DecimalCase {
        case: "the smallest weight the member publishes",
        weight: Some("0.001"),
        target: 1,
        expected_weight: "0.001",
        expected_instance_count: "1000",
        expected_capacity: "1.0",
    },
    DecimalCase {
        case: "the largest weight the member publishes",
        weight: Some("99.999"),
        target: 1,
        expected_weight: "99.999",
        expected_instance_count: "1",
        expected_capacity: "99.999",
    },
];

/// RULE 1: every floating-point member of a response is written as a decimal carrying a
/// fractional part, and an integer member beside it is not.
#[tokio::test]
async fn a_floating_point_member_is_written_as_a_decimal() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    for row in DECIMAL_CASES {
        let case = row.case;
        let body = create_and_describe(
            case,
            &router,
            &create_params(&zone, row.target, row.weight, None),
        )
        .await;
        let weight = one_text(case, &body, "weight");
        let fulfilled = one_text(case, &body, "fulfilledCapacity");
        let total = one_text(case, &body, "totalFulfilledCapacity");
        let count = one_text(case, &body, "totalInstanceCount");
        assert_eq!(
            (
                weight.as_str(),
                fulfilled.as_str(),
                total.as_str(),
                count.as_str()
            ),
            (
                row.expected_weight,
                row.expected_capacity,
                row.expected_capacity,
                row.expected_instance_count
            ),
            "case {case:?}: expected the decimal form of each double and the integer form \
             of the count, got body {body}"
        );
        for (name, text) in [
            ("weight", &weight),
            ("fulfilledCapacity", &fulfilled),
            ("totalFulfilledCapacity", &total),
        ] {
            assert!(
                text.contains('.'),
                "case {case:?}: <{name}> must carry a fractional part, got {text:?}"
            );
        }
    }
}

/// RULE 1, at the other end of the range: a `Weight` outside the bounds the member
/// publishes is refused rather than reserved on, so no response has to render it.
#[tokio::test]
async fn a_weight_outside_the_published_range_is_refused() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    // One value past each published bound of the modelled `@range` (0.001 … 99.999).
    for (case, weight) in [
        ("below the smallest weight", "0.0009"),
        ("above the largest weight", "100.0"),
    ] {
        let (status, body) = call(
            &router,
            REGION,
            "CreateCapacityReservationFleet",
            &create_params(&zone, 1, Some(weight), None),
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400 for Weight={weight}, got {status} with \
             body {body}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: the rule is the envelope's, not one shape's
// ---------------------------------------------------------------------------

/// RULE 2: a Spot Fleet request's floating-point members are written the same way — the
/// weighted capacity of a launch specification and the capacity the fleet has fulfilled,
/// including the zero it reports before anything has launched.
#[tokio::test]
async fn a_spot_fleet_writes_its_floating_point_members_as_decimals() {
    let (router, _engine) = super::ec2_query::emulator();

    // An AMI of the published catalog, read out of the service's own state: a launch
    // specification must name an image the account can resolve.
    let image_id = ec2::state::ComAmazonawsEc2Amazonec2State::default()
        .images
        .keys()
        .next()
        .cloned()
        .expect("a fresh account must publish at least one image");

    // The rows are the two forms a weighted capacity takes: a whole number of units and a
    // fractional one.
    for (case, weight, expected_weight, expected_fulfilled) in [
        ("a whole weighted capacity", "2", "2.0", "2.0"),
        ("a fractional weighted capacity", "1.5", "1.5", "1.5"),
    ] {
        let params = vec![
            param("SpotFleetRequestConfig.IamFleetRole", FLEET_ROLE),
            param("SpotFleetRequestConfig.TargetCapacity", "1"),
            param(
                "SpotFleetRequestConfig.LaunchSpecifications.1.ImageId",
                &image_id,
            ),
            param(
                "SpotFleetRequestConfig.LaunchSpecifications.1.InstanceType",
                INSTANCE_TYPE,
            ),
            param(
                "SpotFleetRequestConfig.LaunchSpecifications.1.WeightedCapacity",
                weight,
            ),
        ];
        let created = ok(case, &router, "RequestSpotFleet", &params).await;
        let fleet_id = element(&created, "spotFleetRequestId")
            .unwrap_or_else(|| panic!("case {case:?}: no spotFleetRequestId in {created}"));

        // Before the clock has launched anything: nothing is fulfilled, which is a double
        // too and so is written `0.0`.
        let pending = ok(
            case,
            &router,
            "DescribeSpotFleetRequests",
            &indexed("SpotFleetRequestId", &[&fleet_id]),
        )
        .await;
        assert_eq!(
            (
                one_text(case, &pending, "weightedCapacity").as_str(),
                one_text(case, &pending, "fulfilledCapacity").as_str(),
            ),
            (expected_weight, "0.0"),
            "case {case:?}: expected the decimal form of each double, got body {pending}"
        );

        // Once the clock has fulfilled the fleet's Spot Instance request, the capacity it
        // provides is reported in the same form.
        tokio::time::sleep(TRANSITION_DELAY * 3 + Duration::from_millis(400)).await;
        let fulfilled = ok(
            case,
            &router,
            "DescribeSpotFleetRequests",
            &indexed("SpotFleetRequestId", &[&fleet_id]),
        )
        .await;
        assert_eq!(
            one_text(case, &fulfilled, "fulfilledCapacity"),
            expected_fulfilled,
            "case {case:?}: expected the fulfilled capacity as a decimal, got body {fulfilled}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: every Capacity Reservation of a Fleet reports a priority
// ---------------------------------------------------------------------------

/// RULE 3: the priority reported for a Capacity Reservation is the one its specification
/// assigned, and the Fleet's default where it assigned none.
///
/// The rows span the ends of the modelled `@range` (`min: 0`), the default itself asked for
/// explicitly, a value in between, and a request that assigns none at all.
#[tokio::test]
async fn a_reservation_reports_the_priority_its_specification_assigned() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    let default_priority = DEFAULT_FLEET_INSTANCE_PRIORITY.to_string();
    let rows: [(&str, Option<i32>, String); 5] = [
        ("no priority at all", None, default_priority.clone()),
        ("the highest priority", Some(0), "0".to_string()),
        ("a priority beside it", Some(1), "1".to_string()),
        ("a priority in between", Some(500), "500".to_string()),
        (
            "the default asked for explicitly",
            Some(DEFAULT_FLEET_INSTANCE_PRIORITY),
            default_priority.clone(),
        ),
    ];
    for (case, priority, expected) in rows {
        let body = create_and_describe(
            case,
            &router,
            &create_params(&zone, 1, None, priority),
        )
        .await;
        let reported = one_text(case, &body, "priority");
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the priority {expected:?}, got {reported:?} in {body}"
        );
    }
}

/// RULE 3, where it meets the allocation strategy: a specification that assigns no priority
/// is ordered BEHIND one that does, so the prioritized Fleet reserves on the prioritized
/// specification — and both report a priority.
#[tokio::test]
async fn a_defaulted_priority_is_ordered_behind_an_assigned_one() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    let case = "one specification prioritized, one not";
    let mut params = vec![param("TotalTargetCapacity", "2")];
    for (index, priority) in [None, Some(7)].into_iter().enumerate() {
        let position = index + 1;
        params.push(param(
            &format!("InstanceTypeSpecification.{position}.InstanceType"),
            INSTANCE_TYPE,
        ));
        params.push(param(
            &format!("InstanceTypeSpecification.{position}.InstancePlatform"),
            INSTANCE_PLATFORM,
        ));
        params.push(param(
            &format!("InstanceTypeSpecification.{position}.AvailabilityZone"),
            &zone,
        ));
        if let Some(priority) = priority {
            params.push(param(
                &format!("InstanceTypeSpecification.{position}.Priority"),
                &priority.to_string(),
            ));
        }
    }
    let body = create_and_describe(case, &router, &params).await;

    let parsed = document(case, &body);
    let fleet = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "instanceTypeSpecificationSet").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no Fleet in {body}"));
    let set = child(fleet, "instanceTypeSpecificationSet")
        .unwrap_or_else(|| panic!("case {case:?}: no specification set in {body}"));
    let items: Vec<roxmltree::Node<'_, '_>> = set
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() == "item")
        .collect();
    assert_eq!(
        items.len(),
        2,
        "case {case:?}: both specifications are reported, got {body}"
    );
    let reported: Vec<(Option<String>, Option<String>)> = items
        .iter()
        .map(|item| {
            (
                child_text(*item, "priority"),
                child_text(*item, "capacityReservationId"),
            )
        })
        .collect();
    assert_eq!(
        reported[0].0.as_deref(),
        Some(DEFAULT_FLEET_INSTANCE_PRIORITY.to_string().as_str()),
        "case {case:?}: the specification that assigned no priority reports the default, \
         got {reported:?}"
    );
    assert_eq!(
        reported[1].0.as_deref(),
        Some("7"),
        "case {case:?}: the specification that assigned a priority reports it, \
         got {reported:?}"
    );
    assert!(
        reported[0].1.is_none() && reported[1].1.is_some(),
        "case {case:?}: the prioritized specification is the one capacity is reserved on, \
         got {reported:?}"
    );
}

/// Look again at the Fleet `narrowed` names until it reports `wanted`, as a client's
/// readiness waiter does. The deadline is generous: all it bounds is a hang.
async fn poll_until_state(
    case: &str,
    router: &MultiServiceRouter,
    narrowed: &[Param],
    wanted: &str,
) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let body = ok(case, router, "DescribeCapacityReservationFleets", narrowed).await;
        let state = element(&body, "state").unwrap_or_default();
        if state == wanted {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "case {case:?}: waited for the Fleet to report {wanted:?}, it reports {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// RULE 3 past the Fleet's own lifecycle: the clock's activation does not change what a
/// reservation reports, and neither does a cancellation — the Fleet keeps reporting the
/// composition it reserved.
#[tokio::test]
async fn what_a_fleet_reserved_is_still_reported_once_it_is_cancelled() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    let case = "a cancelled Fleet";
    let created = ok(
        case,
        &router,
        "CreateCapacityReservationFleet",
        &create_params(&zone, 4, Some("2"), None),
    )
    .await;
    let fleet_id = element(&created, "capacityReservationFleetId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a Fleet id: {created}"));
    let narrowed = indexed("CapacityReservationFleetId", &[&fleet_id]);

    let reserved = ok(
        case,
        &router,
        "DescribeCapacityReservationFleets",
        &narrowed,
    )
    .await;
    let reservation_id = element(&reserved, "capacityReservationId").unwrap_or_else(|| {
        panic!("case {case:?}: the Fleet must hold a Capacity Reservation: {reserved}")
    });

    // Wait for the state word the clock owes the Fleet, by looking again rather than by
    // sleeping a span: a loaded machine polls more often instead of reading too early.
    poll_until_state(case, &router, &narrowed, "active").await;
    let cancelled = ok(
        case,
        &router,
        "CancelCapacityReservationFleets",
        &narrowed,
    )
    .await;
    assert_eq!(
        element(&cancelled, "previousFleetState").as_deref(),
        Some("submitted"),
        "case {case:?}: the cancel reports the state the Fleet had moved out of, \
         got {cancelled}"
    );
    poll_until_state(case, &router, &narrowed, "cancelled").await;

    let after = ok(
        case,
        &router,
        "DescribeCapacityReservationFleets",
        &narrowed,
    )
    .await;
    assert_eq!(
        (
            element(&after, "state").as_deref(),
            element(&after, "capacityReservationId").as_deref(),
            one_text(case, &after, "totalFulfilledCapacity").as_str(),
            one_text(case, &after, "fulfilledCapacity").as_str(),
            one_text(case, &after, "weight").as_str(),
            one_text(case, &after, "priority").as_str(),
        ),
        (
            Some("cancelled"),
            Some(reservation_id.as_str()),
            "4.0",
            "4.0",
            "2.0",
            DEFAULT_FLEET_INSTANCE_PRIORITY.to_string().as_str(),
        ),
        "case {case:?}: a cancelled Fleet still reports what it reserved, got body {after}"
    );
}
