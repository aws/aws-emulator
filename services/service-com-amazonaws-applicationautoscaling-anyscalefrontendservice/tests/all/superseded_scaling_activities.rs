// SPDX-License-Identifier: CC0-1.0

//! A capacity movement that a LATER movement of the same scalable target displaces, end to
//! end over the wire.
//!
//! The suite drives the production router through the shared harness in `tests/all/emulator.rs`,
//! so what is asserted is what a signing AWS client would see.
//!
//! What it pins, over a TABLE rather than one call:
//!
//! * only one capacity is in force per scalable target, so a movement recorded while an
//!   earlier one of the same target has not reached fulfilment yet takes that earlier
//!   movement's capacity over: the displaced activity ends where the displacing one starts,
//!   reports the status the model keeps for an overtaken activity, and its `StatusMessage`
//!   names both the capacity it had applied and the one that took effect instead;
//! * a movement that DID reach fulfilment before the next one started keeps the status it
//!   reported — a terminal status is terminal, so a later movement never rewrites it;
//! * the newest movement of a target is never an overtaken one, and neither status changes
//!   when the log is read again.
//!
//! Each case reads the activity log back and decides which half of the rule applies from the
//! timestamps the service itself reports, so nothing here depends on how fast the machine
//! running it performs two requests.

use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::ACTIVITY_FULFILMENT;

use crate::emulator::{resource_id_for, Emulator};

/// How long a case waits before expecting a movement to have reached its terminal status: the
/// span the service gives a capacity movement, plus room for the request that reads it.
/// Nothing in the emulator sleeps for it — the fulfilment instant is stored when the movement
/// is recorded.
fn fulfilment_wait() -> Duration {
    ACTIVITY_FULFILMENT + Duration::from_millis(300)
}

/// One capacity movement: the window a registration declares, and where the capacity lands
/// once that window is enforced ("If it's below the minimum capacity or above the maximum
/// capacity, Application Auto Scaling scales" the resource into the window).
struct Movement {
    min: i64,
    max: i64,
    capacity: i64,
}

/// One case: a scalable target, the window it is first registered with (which moves nothing,
/// so it records no activity), and the movements performed on it back to back.
///
/// The namespace/dimension pairs are entries of the model's own `ScalableDimension` value set,
/// and each `noun` is that dimension's scaling property read as words — the wording the
/// model's own `DescribeScalingActivities` example carries ("Setting desired count to 1.").
/// The rows differ in namespace, in how many movements are performed, in the direction each
/// one moves the capacity, and in the magnitude it moves by.
struct SupersessionRow {
    case: &'static str,
    dimension: &'static str,
    /// The `MinCapacity`/`MaxCapacity` of the first registration.
    registered: (i64, i64),
    /// The movements, in the order they are performed.
    movements: &'static [Movement],
    noun: &'static str,
}

const SUPERSESSION_CATALOG: &[SupersessionRow] = &[
    // Scaled out, then displaced by a movement in the other direction.
    SupersessionRow {
        case: "a scale-out displaced by a scale-in",
        dimension: "dynamodb:table:ReadCapacityUnits",
        registered: (1, 20),
        movements: &[
            Movement {
                min: 8,
                max: 20,
                capacity: 8,
            },
            Movement {
                min: 1,
                max: 4,
                capacity: 4,
            },
        ],
        noun: "read capacity units",
    },
    // Three movements: the middle one displaces the first and is displaced in turn.
    SupersessionRow {
        case: "three movements, the middle one displaced in turn",
        dimension: "ecs:service:DesiredCount",
        registered: (0, 10),
        movements: &[
            Movement {
                min: 3,
                max: 10,
                capacity: 3,
            },
            Movement {
                min: 7,
                max: 10,
                capacity: 7,
            },
            Movement {
                min: 1,
                max: 2,
                capacity: 2,
            },
        ],
        noun: "desired count",
    },
    // A namespace whose owning service this emulator holds no store for.
    SupersessionRow {
        case: "a namespace with no resource of its own",
        dimension: "lambda:function:ProvisionedConcurrency",
        registered: (2, 8),
        movements: &[
            Movement {
                min: 5,
                max: 8,
                capacity: 5,
            },
            Movement {
                min: 1,
                max: 3,
                capacity: 3,
            },
        ],
        noun: "provisioned concurrency",
    },
    // Both movements scale in, the second all the way to one.
    SupersessionRow {
        case: "two scale-ins in a row",
        dimension: "elasticache:replication-group:Replicas",
        registered: (3, 6),
        movements: &[
            Movement {
                min: 1,
                max: 2,
                capacity: 2,
            },
            Movement {
                min: 1,
                max: 1,
                capacity: 1,
            },
        ],
        noun: "replicas",
    },
    // A large capacity, moved by 150 and then by 130.
    SupersessionRow {
        case: "a large capacity moved twice",
        dimension: "kafka:broker-storage:VolumeSize",
        registered: (100, 1000),
        movements: &[
            Movement {
                min: 250,
                max: 1000,
                capacity: 250,
            },
            Movement {
                min: 100,
                max: 120,
                capacity: 120,
            },
        ],
        noun: "volume size",
    },
];

/// The status the model keeps for an activity whose capacity a later one took over.
const OVERRIDDEN: &str = "Overridden";
/// The status of a movement whose capacity took effect.
const SUCCESSFUL: &str = "Successful";

/// The identity members of a row's scalable target.
fn identity(row: &SupersessionRow) -> Value {
    json!({
        "ServiceNamespace": namespace_of(row),
        "ResourceId": resource_id_for(row.dimension),
        "ScalableDimension": row.dimension,
    })
}

/// The namespace a dimension belongs to: "the service namespace, resource type, and scaling
/// property", so it is the first segment.
fn namespace_of(row: &SupersessionRow) -> &'static str {
    row.dimension.split(':').next().unwrap_or_default()
}

/// `base` with every member of `patch` merged in.
fn merged(base: Value, patch: Value) -> Value {
    let mut merged = base;
    for (member, value) in patch.as_object().expect("an object patch") {
        merged[member] = value.clone();
    }
    merged
}

/// Register a row's target with `min`/`max`, creating the resource it names first where this
/// emulator owns the service that resource lives in.
async fn register(emulator: &Emulator, row: &SupersessionRow, min: i64, max: i64) {
    emulator
        .given_resource(namespace_of(row), &resource_id_for(row.dimension), min)
        .await;
    let request = merged(
        identity(row),
        json!({"MinCapacity": min, "MaxCapacity": max}),
    );
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {:?}: expected RegisterScalableTarget({min}, {max}) to answer 200, \
         got {status} {body:?}",
        row.case
    );
}

/// The activity log of a row's target, as `DescribeScalingActivities` reports it (newest
/// first).
async fn activities(emulator: &Emulator, row: &SupersessionRow) -> Vec<Value> {
    let (status, body) = emulator
        .call("DescribeScalingActivities", identity(row))
        .await;
    assert_eq!(
        status, 200,
        "case {:?}: expected DescribeScalingActivities to answer 200, got {status} {body:?}",
        row.case
    );
    body["ScalingActivities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {:?}: expected a ScalingActivities list, got {body:?}",
                row.case
            )
        })
        .clone()
}

/// The `StatusMessage` of an activity whose capacity TOOK EFFECT: the capacity the target was
/// set to and the service that owns the resource it was set on, the wording the model's own
/// `DescribeScalingActivities` example carries.
fn fulfilled_message(row: &SupersessionRow, capacity: i64) -> String {
    format!(
        "Successfully set {} to {capacity}. Change successfully fulfilled by {}.",
        row.noun,
        namespace_of(row)
    )
}

/// The `StatusMessage` of an activity a later movement took over: BOTH halves of the
/// override — the capacity this activity itself applied, and the capacity that was put in its
/// place before this one was confirmed.
fn overridden_message(
    row: &SupersessionRow,
    own_capacity: i64,
    displacing_capacity: i64,
) -> String {
    format!(
        "Successfully set {} to {own_capacity}. Found it was later changed to \
         {displacing_capacity}.",
        row.noun
    )
}

/// A reported timestamp, in seconds since the epoch.
fn seconds(activity: &Value, member: &str, case: &str) -> f64 {
    activity[member].as_f64().unwrap_or_else(|| {
        panic!("case {case:?}: expected a numeric {member}, got {activity:?}")
    })
}

#[tokio::test]
async fn a_movement_a_later_one_displaces_before_fulfilment_is_reported_overridden() {
    // How many movements of the catalogue were really displaced in flight. Two requests one
    // after another are far inside the fulfilment span, so this is every movement but the
    // newest of each row; it is counted rather than assumed, because a case whose movements
    // all turned out fulfilled would have proved nothing about an overtaken one.
    let mut displaced = 0_usize;
    for row in SUPERSESSION_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        let (min, max) = row.registered;
        register(&emulator, row, min, max).await;

        // A first registration adopts the window it declares, so there is nothing to report
        // yet and every activity below is one of the movements this case performs.
        let none_yet = activities(&emulator, row).await;
        assert!(
            none_yet.is_empty(),
            "case {case:?}: expected a first registration to report no activity, got {none_yet:?}"
        );

        // The movements, back to back: nothing waits between them, so each one is recorded
        // while the one before it is still on its way to fulfilment.
        for movement in row.movements {
            register(&emulator, row, movement.min, movement.max).await;
        }

        let listed = activities(&emulator, row).await;
        assert_eq!(
            listed.len(),
            row.movements.len(),
            "case {case:?}: expected one activity per movement, got {listed:?}"
        );

        // The log reads newest first; the movements were performed oldest first.
        let oldest_first: Vec<&Value> = listed.iter().rev().collect();
        for (position, (activity, movement)) in
            oldest_first.iter().zip(row.movements).enumerate()
        {
            let expected_description = format!("Setting {} to {}.", row.noun, movement.capacity);
            assert_eq!(
                activity["Description"],
                json!(expected_description),
                "case {case:?}: expected movement {position} to describe itself as \
                 {expected_description:?}, got {activity:?}"
            );

            let Some(displacing) = oldest_first.get(position + 1) else {
                // The newest movement: nothing has taken its capacity over, so it is either
                // still on its way to fulfilment or fulfilled — never an overtaken activity.
                let status = activity["StatusCode"].as_str().unwrap_or_default();
                assert!(
                    status == "InProgress" || status == SUCCESSFUL,
                    "case {case:?}: expected the newest movement to be InProgress or \
                     {SUCCESSFUL}, got {status:?} in {activity:?}"
                );
                continue;
            };

            let started = seconds(activity, "StartTime", case);
            let displaced_at = seconds(displacing, "StartTime", case);
            let displacing_capacity = row.movements[position + 1].capacity;
            let in_flight = displaced_at - started < ACTIVITY_FULFILMENT.as_secs_f64();
            if in_flight {
                displaced += 1;
                // The capacity this movement asked for was replaced before it was reached.
                assert_eq!(
                    activity["StatusCode"],
                    json!(OVERRIDDEN),
                    "case {case:?}: expected movement {position}, displaced {} s after it \
                     started, to report {OVERRIDDEN}, got {activity:?}",
                    displaced_at - started
                );
                let expected_message =
                    overridden_message(row, movement.capacity, displacing_capacity);
                assert_eq!(
                    activity["StatusMessage"],
                    json!(expected_message),
                    "case {case:?}: expected movement {position} to report \
                     {expected_message:?}, got {activity:?}"
                );
                let ended = seconds(activity, "EndTime", case);
                assert_eq!(
                    ended, displaced_at,
                    "case {case:?}: expected movement {position} to end where the movement \
                     that displaced it starts ({displaced_at}), got {ended}"
                );
            } else {
                // The machine was slow enough that this movement was fulfilled before the
                // next one started, which is the other half of the same rule: its capacity
                // took effect, so it stays the status a fulfilled movement reports.
                assert_eq!(
                    activity["StatusCode"],
                    json!(SUCCESSFUL),
                    "case {case:?}: expected movement {position}, fulfilled {} s before the \
                     next one started, to report {SUCCESSFUL}, got {activity:?}",
                    displaced_at - started
                );
                let expected_message = fulfilled_message(row, movement.capacity);
                assert_eq!(
                    activity["StatusMessage"],
                    json!(expected_message),
                    "case {case:?}: expected movement {position} to report \
                     {expected_message:?}, got {activity:?}"
                );
            }
        }

        // Reading the log again answers the same thing, and the clock reaching the newest
        // movement's fulfilment instant leaves every earlier status where it was.
        let again = activities(&emulator, row).await;
        assert_eq!(
            again, listed,
            "case {case:?}: expected a second read to answer identically, got {again:?}"
        );
        sleep(fulfilment_wait());
        let settled = activities(&emulator, row).await;
        assert_eq!(
            settled[0]["StatusCode"],
            json!(SUCCESSFUL),
            "case {case:?}: expected the newest movement to be {SUCCESSFUL} once its \
             fulfilment instant has passed, got {:?}",
            settled[0]
        );
        let newest_capacity = row.movements[row.movements.len() - 1].capacity;
        let expected_message = fulfilled_message(row, newest_capacity);
        assert_eq!(
            settled[0]["StatusMessage"],
            json!(expected_message),
            "case {case:?}: expected the newest movement to report {expected_message:?}, \
             got {:?}",
            settled[0]
        );
        assert_eq!(
            settled[1..],
            listed[1..],
            "case {case:?}: expected the displaced movements to keep the status they \
             reported, got {settled:?}"
        );
    }

    let movements: usize = SUPERSESSION_CATALOG
        .iter()
        .map(|row| row.movements.len() - 1)
        .sum();
    assert!(
        displaced > 0,
        "expected at least one of the {movements} movements each row performs before its \
         newest to be displaced in flight, got {displaced}"
    );
}

#[tokio::test]
async fn a_movement_already_fulfilled_when_the_next_one_starts_stays_successful() {
    // One row is enough to pin the other half of the rule deterministically: this case WAITS
    // for the first movement's fulfilment instant before performing the second, so the first
    // movement's capacity took effect and nothing displaced it.
    let row = &SUPERSESSION_CATALOG[0];
    let case = "a fulfilled movement is not displaced by the next one";
    let emulator = Emulator::empty();
    let (min, max) = row.registered;
    register(&emulator, row, min, max).await;

    let first = &row.movements[0];
    register(&emulator, row, first.min, first.max).await;
    sleep(fulfilment_wait());

    let fulfilled = activities(&emulator, row).await;
    assert_eq!(
        fulfilled.len(),
        1,
        "case {case:?}: expected one activity for one movement, got {fulfilled:?}"
    );
    assert_eq!(
        fulfilled[0]["StatusCode"],
        json!(SUCCESSFUL),
        "case {case:?}: expected the movement to be {SUCCESSFUL} by now, got {:?}",
        fulfilled[0]
    );
    let ended = seconds(&fulfilled[0], "EndTime", case);

    let second = &row.movements[1];
    register(&emulator, row, second.min, second.max).await;

    let listed = activities(&emulator, row).await;
    assert_eq!(
        listed.len(),
        2,
        "case {case:?}: expected the second movement to be reported too, got {listed:?}"
    );
    let earlier = &listed[1];
    assert_eq!(
        earlier["StatusCode"],
        json!(SUCCESSFUL),
        "case {case:?}: expected a movement that reached fulfilment to keep reporting \
         {SUCCESSFUL}, got {earlier:?}"
    );
    let expected_message = fulfilled_message(row, first.capacity);
    assert_eq!(
        earlier["StatusMessage"],
        json!(expected_message),
        "case {case:?}: expected {expected_message:?}, got {earlier:?}"
    );
    assert_eq!(
        seconds(earlier, "EndTime", case),
        ended,
        "case {case:?}: expected the fulfilled movement's EndTime not to move, got {earlier:?}"
    );
}
