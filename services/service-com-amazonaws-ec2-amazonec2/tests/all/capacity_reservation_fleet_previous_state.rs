// SPDX-License-Identifier: CC0-1.0

//! The state word pair `CancelCapacityReservationFleets` reports for a Capacity Reservation
//! Fleet — driven over the wire the service actually speaks, `ec2Query`, through the crate's
//! shared [`super::ec2_query`] harness.
//!
//! The rule under test: a cancellation result reports the FLEET'S OWN previous state —
//! "The previous state of the Capacity Reservation Fleet"
//! (`CapacityReservationFleetCancellationState$PreviousFleetState`,
//! `smithy_query(query="shape",shape="CapacityReservationFleetCancellationState")`), which is
//! the state word the Fleet had already moved out of when the call arrived, NOT the state the
//! cancellation itself interrupts — beside `currentFleetState`, the state the cancellation
//! puts the Fleet in. So the pair a client reads records the move the Fleet made before this
//! call and the move this call made, and a Fleet that has not yet left the state it was
//! created in reports that state as its previous one.
//!
//! The rows differ along the Fleet's own state HISTORY — how many moves it has made and
//! which ones — because that is what the rule generalises over: a Fleet that has made none,
//! one that has made the activation the clock owes it, one modified after it activated, and
//! one modified while it was still being prepared. Every wait here polls
//! `DescribeCapacityReservationFleets` for the state word it waits on rather than sleeping a
//! span, so a slow machine only polls more often.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;
use tokio::time::Instant;

use ec2::state::{
    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE, CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
    CAPACITY_RESERVATION_FLEET_STATE_CANCELLING, CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
    CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
};

use super::ec2_query::{call, element, indexed};
use super::wire::{child_text, document, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// The instance type and platform every Fleet below reserves capacity for: members of the
/// modelled `InstanceType` and `CapacityReservationInstancePlatform` catalogs. Which type a
/// row names is not what this rule generalises over, so every row names the same one.
const INSTANCE_TYPE: &str = "m5.large";
const INSTANCE_PLATFORM: &str = "Linux/UNIX";

/// How often a wait looks again for the state word it waits on.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// How long a wait keeps looking before it gives up: generous, because the only thing it
/// bounds is a hang — the spans this service gives its clock-owed moves are far shorter, and
/// a loaded machine is meant to poll more often rather than to fail.
const POLL_DEADLINE: Duration = Duration::from_secs(60);

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
    element(&body, "zoneName").unwrap_or_else(|| {
        panic!("case \"zones\": the Region must report Availability Zones, got {body}")
    })
}

/// Create one Fleet reserving `target` capacity units in `zone`, as the Fleet id the create
/// reported.
async fn create_fleet(
    case: &str,
    router: &MultiServiceRouter,
    zone: &str,
    target: i64,
) -> String {
    let body = ok(
        case,
        router,
        "CreateCapacityReservationFleet",
        &[
            param("TotalTargetCapacity", &target.to_string()),
            param("InstanceTypeSpecification.1.InstanceType", INSTANCE_TYPE),
            param(
                "InstanceTypeSpecification.1.InstancePlatform",
                INSTANCE_PLATFORM,
            ),
            param("InstanceTypeSpecification.1.AvailabilityZone", zone),
        ],
    )
    .await;
    assert_eq!(
        element(&body, "state").as_deref(),
        Some(CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED),
        "case {case:?}: a Fleet is created in {CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED:?}, \
         got {body}"
    );
    element(&body, "capacityReservationFleetId")
        .unwrap_or_else(|| panic!("case {case:?}: the create must report a Fleet id: {body}"))
}

/// The state `DescribeCapacityReservationFleets` reports for the one Fleet `fleet_id`.
async fn described_state(case: &str, router: &MultiServiceRouter, fleet_id: &str) -> String {
    let body = ok(
        case,
        router,
        "DescribeCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[fleet_id]),
    )
    .await;
    element(&body, "state")
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report a state: {body}"))
}

/// Look again until the Fleet reports `wanted`, exactly as a client's readiness waiter does,
/// and answer the state it finally reported.
async fn poll_until_state(
    case: &str,
    router: &MultiServiceRouter,
    fleet_id: &str,
    wanted: &str,
) -> String {
    let deadline = Instant::now() + POLL_DEADLINE;
    let mut state = described_state(case, router, fleet_id).await;
    while state != wanted && Instant::now() < deadline {
        tokio::time::sleep(POLL_INTERVAL).await;
        state = described_state(case, router, fleet_id).await;
    }
    assert_eq!(
        state, wanted,
        "case {case:?}: waited for the Fleet to report {wanted:?}, it reports {state:?}"
    );
    state
}

/// Modify one Fleet's total target capacity and wait for the modification to settle, so the
/// Fleet's history carries the `modifying` word the clock then moves it out of.
async fn modify_and_settle(
    case: &str,
    router: &MultiServiceRouter,
    fleet_id: &str,
    new_target: i64,
) {
    let body = ok(
        case,
        router,
        "ModifyCapacityReservationFleet",
        &[
            param("CapacityReservationFleetId", fleet_id),
            param("TotalTargetCapacity", &new_target.to_string()),
        ],
    )
    .await;
    assert_eq!(
        element(&body, "return").as_deref(),
        Some("true"),
        "case {case:?}: the modification must be accepted, got {body}"
    );
    poll_until_state(
        case,
        router,
        fleet_id,
        CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
    )
    .await;
}

/// The one entry of `successfulFleetCancellationSet`, as the id and the state pair it
/// carries.
fn cancelled_entry(case: &str, body: &str) -> (Option<String>, Option<String>, Option<String>) {
    let parsed = document(case, body);
    let successful = set_items(parsed.root_element(), "successfulFleetCancellationSet");
    assert_eq!(
        successful.len(),
        1,
        "case {case:?}: one id cancels one Fleet, got body {body}"
    );
    assert!(
        set_items(parsed.root_element(), "failedFleetCancellationSet").is_empty(),
        "case {case:?}: nothing failed, got body {body}"
    );
    (
        child_text(successful[0], "capacityReservationFleetId"),
        child_text(successful[0], "previousFleetState"),
        child_text(successful[0], "currentFleetState"),
    )
}

/// What a row does to the Fleet before it is cancelled.
#[derive(Debug, Clone, Copy, PartialEq)]
enum History {
    /// Nothing: the Fleet is cancelled as it was created, so it has left no state word.
    AsCreated,
    /// The activation the clock owes a `submitted` Fleet, waited out through the describe.
    Activated,
    /// The activation, then a modification waited out through the describe.
    ActivatedThenModified,
    /// A modification asked for while the Fleet was still `submitted`, waited out through
    /// the describe.
    ModifiedWhileSubmitted,
}

/// One row: a Fleet's state history, and the `previousFleetState` the cancellation of such a
/// Fleet reports.
struct Row {
    case: &'static str,
    history: History,
    /// The capacity units the Fleet is created with, and the ones a modification moves it
    /// to: a row's capacity is not what the rule generalises over, so each row simply
    /// differs from its neighbours.
    target: i64,
    new_target: i64,
    expected_previous: &'static str,
}

/// The rows of the table: one per shape of state history a cancellable Fleet can have in
/// this service, with the previous state word each history leaves behind.
const ROWS: &[Row] = &[
    Row {
        case: "a Fleet cancelled as it was created",
        history: History::AsCreated,
        target: 1,
        new_target: 1,
        // The Fleet has left no state word, so the state it was created in is also the one
        // before its current one — the same answer whether or not the activation the clock
        // owes it has fallen due by the time the cancel arrives.
        expected_previous: CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
    },
    Row {
        case: "a Fleet cancelled once the clock activated it",
        history: History::Activated,
        target: 2,
        new_target: 2,
        expected_previous: CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED,
    },
    Row {
        case: "a Fleet cancelled after a modification it had activated before",
        history: History::ActivatedThenModified,
        target: 3,
        new_target: 7,
        expected_previous: CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
    },
    Row {
        case: "a Fleet cancelled after a modification asked for while it was submitted",
        history: History::ModifiedWhileSubmitted,
        target: 4,
        new_target: 2,
        expected_previous: CAPACITY_RESERVATION_FLEET_STATE_MODIFYING,
    },
];

/// The rule: whatever the Fleet's history, the cancellation reports the word the Fleet had
/// moved OUT of before the state the cancel interrupts, and `cancelling` as the state it
/// moves the Fleet to.
#[tokio::test]
async fn a_cancellation_reports_the_fleets_own_previous_state() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;

    for row in ROWS {
        let case = row.case;
        let fleet_id = create_fleet(case, &router, &zone, row.target).await;
        match row.history {
            History::AsCreated => {}
            History::Activated => {
                poll_until_state(
                    case,
                    &router,
                    &fleet_id,
                    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
                )
                .await;
            }
            History::ActivatedThenModified => {
                poll_until_state(
                    case,
                    &router,
                    &fleet_id,
                    CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
                )
                .await;
                modify_and_settle(case, &router, &fleet_id, row.new_target).await;
            }
            History::ModifiedWhileSubmitted => {
                modify_and_settle(case, &router, &fleet_id, row.new_target).await;
            }
        }

        let body = ok(
            case,
            &router,
            "CancelCapacityReservationFleets",
            &indexed("CapacityReservationFleetId", &[&fleet_id]),
        )
        .await;
        let reported = cancelled_entry(case, &body);
        assert_eq!(
            reported,
            (
                Some(fleet_id.clone()),
                Some(row.expected_previous.to_string()),
                Some(CAPACITY_RESERVATION_FLEET_STATE_CANCELLING.to_string()),
            ),
            "case {case:?}: expected the Fleet's previous state \
             {:?} beside {CAPACITY_RESERVATION_FLEET_STATE_CANCELLING:?}, got {reported:?} in \
             body {body}",
            row.expected_previous
        );

        // The state the pair reports as current is the one the store now holds, and the
        // clock carries the Fleet on to the terminal word from there.
        assert_eq!(
            described_state(case, &router, &fleet_id).await,
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLING,
            "case {case:?}: a cancelled Fleet is first {CAPACITY_RESERVATION_FLEET_STATE_CANCELLING:?}"
        );
        poll_until_state(
            case,
            &router,
            &fleet_id,
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLED,
        )
        .await;
    }
}

/// The same rule where the cancellation names SEVERAL Fleets at once: each entry carries its
/// own Fleet's previous state, so one call cannot report one Fleet's history for another.
#[tokio::test]
async fn each_cancelled_fleet_carries_its_own_previous_state() {
    let (router, _engine) = super::ec2_query::emulator();
    let zone = first_zone(&router).await;
    let case = "two Fleets with different histories cancelled together";

    // One Fleet that has only ever been prepared, and one the clock activated and a
    // modification then carried through `modifying`.
    let untouched = create_fleet(case, &router, &zone, 1).await;
    let modified = create_fleet(case, &router, &zone, 5).await;
    poll_until_state(
        case,
        &router,
        &modified,
        CAPACITY_RESERVATION_FLEET_STATE_ACTIVE,
    )
    .await;
    modify_and_settle(case, &router, &modified, 9).await;

    let body = ok(
        case,
        &router,
        "CancelCapacityReservationFleets",
        &indexed("CapacityReservationFleetId", &[&untouched, &modified]),
    )
    .await;
    let parsed = document(case, &body);
    let reported: Vec<(String, String, String)> =
        set_items(parsed.root_element(), "successfulFleetCancellationSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "capacityReservationFleetId").unwrap_or_default(),
                    child_text(item, "previousFleetState").unwrap_or_default(),
                    child_text(item, "currentFleetState").unwrap_or_default(),
                )
            })
            .collect();
    let mut expected = vec![
        (
            untouched.clone(),
            CAPACITY_RESERVATION_FLEET_STATE_SUBMITTED.to_string(),
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLING.to_string(),
        ),
        (
            modified.clone(),
            CAPACITY_RESERVATION_FLEET_STATE_MODIFYING.to_string(),
            CAPACITY_RESERVATION_FLEET_STATE_CANCELLING.to_string(),
        ),
    ];
    let mut actual = reported.clone();
    actual.sort();
    expected.sort();
    assert_eq!(
        actual, expected,
        "case {case:?}: each entry carries its own Fleet's previous state, got {reported:?} in \
         body {body}"
    );
}
