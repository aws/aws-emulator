// SPDX-License-Identifier: CC0-1.0

//! `DescribeScalingActivities`, end to end over the wire.
//!
//! The suite drives the production router through the shared harness in
//! `tests/all/scalable_target_lifecycle.rs`, so what is asserted is what a signing AWS
//! client would see.
//!
//! What it pins, over a TABLE rather than one call:
//!
//! * every capacity movement this service performs is reported as one scaling activity,
//!   for any namespace/dimension pair and in either direction, and a registration that
//!   moves nothing reports none;
//! * an activity is non-terminal when it starts and `Successful` once the clock reaches
//!   its fulfilment instant, without anything having to tick;
//! * the log reads newest first and pages at `MaxResults` with an opaque `NextToken`;
//! * `ResourceId` / `ScalableDimension` filter it, and a request outside the operation's
//!   documented contract is refused with the modelled error.

use std::collections::BTreeSet;
use std::thread::sleep;
use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::state::ACTIVITY_FULFILMENT;

use crate::emulator::{error_code, Emulator};

/// How long a case waits before expecting an activity to have reached its terminal
/// status: the span the service gives a capacity movement, taken from the service's own
/// constant so the wait cannot drift from it, plus a margin. Nothing in the emulator sleeps
/// for it.
const FULFILMENT_WAIT: Duration = ACTIVITY_FULFILMENT.saturating_add(Duration::from_millis(300));

/// Call `RegisterScalableTarget` and assert it succeeded, creating the resource the target
/// names first where this emulator owns the service that resource lives in.
///
/// The resource is created ONCE per identity, at the floor of the window the registration
/// that first names it declares — a capacity inside that window, so the registration that
/// creates it corrects nothing and every movement a case then asserts is one a later
/// registration made.
async fn register(emulator: &Emulator, request: Value, case: &str) {
    emulator
        .given_resource(
            request["ServiceNamespace"].as_str().unwrap_or_default(),
            request["ResourceId"].as_str().unwrap_or_default(),
            request["MinCapacity"].as_i64().unwrap_or(1),
        )
        .await;
    let (status, body) = emulator.call("RegisterScalableTarget", request).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected RegisterScalableTarget to answer 200, got {status} {body:?}"
    );
}

/// Call `DescribeScalingActivities` and answer `(ScalingActivities, NextToken)`.
async fn activities(emulator: &Emulator, request: Value, case: &str) -> (Vec<Value>, Option<String>) {
    let (status, body) = emulator
        .call("DescribeScalingActivities", request.clone())
        .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected DescribeScalingActivities{request} to answer 200, \
         got {status} {body:?}"
    );
    let listed = body["ScalingActivities"]
        .as_array()
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a ScalingActivities list, got {body:?}")
        })
        .clone();
    let token = body["NextToken"].as_str().map(ToString::to_string);
    (listed, token)
}

/// The `Description` of each listed activity, in the order they were listed.
fn descriptions(listed: &[Value]) -> Vec<String> {
    listed
        .iter()
        .map(|activity| activity["Description"].as_str().unwrap_or("").to_string())
        .collect()
}

/// A capacity movement: the window a target is registered with, the update that moves
/// its capacity, and the activity that movement must be reported as.
///
/// The namespace/dimension pairs are entries of the model's own `ScalableDimension`
/// value set (`SCALABLE_DIMENSION_VALUES` in `src/ops/types/shared_types.rs`); the
/// expected `Description`/`StatusMessage` nouns are that dimension's scaling property
/// read as words, the wording the model's own `DescribeScalingActivities` example
/// carries ("Setting desired count to 1."). The rows differ in namespace, in the
/// direction the capacity moves, in the magnitude it moves by, and in whether the
/// property is one word or three.
struct MovementRow {
    namespace: &'static str,
    resource_id: &'static str,
    dimension: &'static str,
    min: i64,
    max: i64,
    /// The members the second registration names, as a non-capturing closure.
    patch: fn() -> Value,
    /// Where the capacity lands once the window is enforced.
    capacity: i64,
    /// The capacity the activity's text names, in words: the dimension's scaling property,
    /// or the capacity the namespace's own contract carries where the dimension names none.
    noun: &'static str,
    /// What caused the movement, built from the request that caused it.
    cause: &'static str,
}

const MOVEMENT_CATALOG: &[MovementRow] = &[
    // Raising the floor above the capacity moves it up.
    MovementRow {
        namespace: "dynamodb",
        resource_id: "table/my-table",
        dimension: "dynamodb:table:ReadCapacityUnits",
        min: 1,
        max: 10,
        patch: || json!({"MinCapacity": 5}),
        capacity: 5,
        noun: "read capacity units",
        cause: "minimum capacity was set to 5",
    },
    // Lowering the ceiling below the capacity moves it down.
    MovementRow {
        namespace: "dynamodb",
        resource_id: "table/other-table",
        dimension: "dynamodb:table:WriteCapacityUnits",
        min: 4,
        max: 20,
        patch: || json!({"MinCapacity": 1, "MaxCapacity": 2}),
        capacity: 2,
        noun: "write capacity units",
        cause: "maximum capacity was set to 2",
    },
    // A window whose floor is 0, moved up by 3.
    MovementRow {
        namespace: "ecs",
        resource_id: "service/my-cluster/my-service",
        dimension: "ecs:service:DesiredCount",
        min: 0,
        max: 10,
        patch: || json!({"MinCapacity": 3}),
        capacity: 3,
        noun: "desired count",
        cause: "minimum capacity was set to 3",
    },
    // Moved all the way to the ceiling.
    MovementRow {
        namespace: "lambda",
        resource_id: "function:my-function:prod",
        dimension: "lambda:function:ProvisionedConcurrency",
        min: 2,
        max: 8,
        patch: || json!({"MinCapacity": 8}),
        capacity: 8,
        noun: "provisioned concurrency",
        cause: "minimum capacity was set to 8",
    },
    // A single-word property, moved down.
    MovementRow {
        namespace: "elasticache",
        resource_id: "replication-group/my-group",
        dimension: "elasticache:replication-group:Replicas",
        min: 3,
        max: 6,
        patch: || json!({"MinCapacity": 1, "MaxCapacity": 2}),
        capacity: 2,
        noun: "replicas",
        cause: "maximum capacity was set to 2",
    },
    // A large capacity, moved by 150.
    MovementRow {
        namespace: "kafka",
        resource_id: "arn:aws:kafka:us-east-1:000000000000:cluster/my-cluster/abcd",
        dimension: "kafka:broker-storage:VolumeSize",
        min: 100,
        max: 1000,
        patch: || json!({"MinCapacity": 250}),
        capacity: 250,
        noun: "volume size",
        cause: "minimum capacity was set to 250",
    },
    // The namespace a caller brings its own resource to, whose dimension names no capacity of
    // its own: the activity names the capacity the custom-resource contract carries instead.
    MovementRow {
        namespace: "custom-resource",
        resource_id: "https://example.test/scalableResource/1",
        dimension: "custom-resource:ResourceType:Property",
        min: 1,
        max: 5,
        patch: || json!({"MinCapacity": 4}),
        capacity: 4,
        noun: "desired capacity",
        cause: "minimum capacity was set to 4",
    },
    // A three-word property.
    MovementRow {
        namespace: "sagemaker",
        resource_id: "inference-component/my-component",
        dimension: "sagemaker:inference-component:DesiredCopyCount",
        min: 1,
        max: 4,
        patch: || json!({"MinCapacity": 2}),
        capacity: 2,
        noun: "desired copy count",
        cause: "minimum capacity was set to 2",
    },
];

/// The identity members every request for a row carries.
fn identity(row: &MovementRow) -> Value {
    json!({
        "ServiceNamespace": row.namespace,
        "ResourceId": row.resource_id,
        "ScalableDimension": row.dimension,
    })
}

/// `base` with every member of `patch` merged in.
fn merged(base: Value, patch: Value) -> Value {
    let mut merged = base;
    for (member, value) in patch.as_object().expect("an object patch") {
        merged[member] = value.clone();
    }
    merged
}

#[tokio::test]
async fn a_capacity_movement_is_reported_as_one_activity_that_reaches_its_terminal_status() {
    // First pass: move every row's capacity and assert what a reader sees while the
    // movement is in flight. Each row keeps its own emulator (so the one activity below
    // is its own) and they are all held until the second pass, which lets the whole
    // catalogue wait on ONE fulfilment span instead of one per row.
    let mut staged: Vec<(Emulator, Value)> = Vec::new();
    for row in MOVEMENT_CATALOG {
        let case = format!("{}/{}", row.namespace, row.dimension);
        let emulator = Emulator::empty();

        register(
            &emulator,
            merged(
                identity(row),
                json!({"MinCapacity": row.min, "MaxCapacity": row.max}),
            ),
            &case,
        )
        .await;

        // A first registration adopts the window it declares; it moves nothing, so there
        // is nothing to report.
        let (none_yet, _) = activities(
            &emulator,
            json!({"ServiceNamespace": row.namespace}),
            &case,
        )
        .await;
        assert!(
            none_yet.is_empty(),
            "case {case:?}: expected a first registration to report no activity, \
             got {none_yet:?}"
        );

        register(&emulator, merged(identity(row), (row.patch)()), &case).await;

        // Read straight back. On an unloaded machine the movement is still in flight;
        // the invariant every reader must see either way is that a movement is
        // non-terminal with no end, or terminal with one — never half of each. The
        // non-terminal state itself is pinned by
        // `a_movement_is_observable_before_the_clock_fulfils_it` below.
        let (in_flight, token) = activities(
            &emulator,
            json!({"ServiceNamespace": row.namespace}),
            &case,
        )
        .await;
        assert_eq!(
            in_flight.len(),
            1,
            "case {case:?}: expected exactly one scaling activity, got {in_flight:?}"
        );
        assert_eq!(
            token, None,
            "case {case:?}: expected no NextToken on a single-page answer, got {token:?}"
        );
        let started = &in_flight[0];
        match started["StatusCode"].as_str() {
            Some("InProgress") => {
                assert_eq!(
                    started["EndTime"],
                    Value::Null,
                    "case {case:?}: expected no EndTime before fulfilment, got {started:?}"
                );
                // A movement that has started reports how far its fulfilment got, naming the
                // capacity it set; the fulfilment itself is what it is still waiting on.
                let awaited = format!(
                    "Successfully set {} to {}. Waiting for change to be fulfilled by {}.",
                    row.noun, row.capacity, row.namespace
                );
                assert_eq!(
                    started["StatusMessage"],
                    json!(awaited),
                    "case {case:?}: expected StatusMessage {awaited:?} before fulfilment, \
                     got {started:?}"
                );
            }
            Some("Successful") => assert!(
                started["EndTime"].is_f64(),
                "case {case:?}: expected a fulfilled movement to carry an EndTime, \
                 got {started:?}"
            ),
            other => panic!(
                "case {case:?}: expected a movement to be InProgress or Successful, \
                 got {other:?} in {started:?}"
            ),
        }
        let expected_description = format!("Setting {} to {}.", row.noun, row.capacity);
        assert_eq!(
            started["Description"],
            json!(expected_description),
            "case {case:?}: expected Description {expected_description:?}, got {started:?}"
        );
        assert_eq!(
            started["Cause"],
            json!(row.cause),
            "case {case:?}: expected Cause {:?}, got {started:?}",
            row.cause
        );
        assert_eq!(
            started["ServiceNamespace"],
            json!(row.namespace),
            "case {case:?}: expected the activity's namespace, got {started:?}"
        );
        assert_eq!(
            started["ResourceId"],
            json!(row.resource_id),
            "case {case:?}: expected the activity's ResourceId, got {started:?}"
        );
        assert_eq!(
            started["ScalableDimension"],
            json!(row.dimension),
            "case {case:?}: expected the activity's ScalableDimension, got {started:?}"
        );
        let start_time = started["StartTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric StartTime, got {started:?}")
        });
        assert!(
            start_time > 0.0,
            "case {case:?}: expected a StartTime after the epoch, got {start_time}"
        );
        let activity_id = started["ActivityId"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case:?}: expected an ActivityId, got {started:?}"))
            .to_string();
        let groups: Vec<usize> = activity_id.split('-').map(str::len).collect();
        assert_eq!(
            groups,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected an opaque 8-4-4-4-12 ActivityId, got {activity_id:?}"
        );

        staged.push((emulator, started.clone()));
    }

    // The clock, not a later call, is what fulfils them — and one wait covers every row,
    // because each movement's fulfilment instant was set when it was recorded.
    sleep(FULFILMENT_WAIT);

    for (row, (emulator, started)) in MOVEMENT_CATALOG.iter().zip(&staged) {
        let case = format!("{}/{}", row.namespace, row.dimension);
        let start_time = started["StartTime"]
            .as_f64()
            .expect("the first pass asserted a numeric StartTime");
        let (fulfilled, _) = activities(
            emulator,
            json!({"ServiceNamespace": row.namespace}),
            &case,
        )
        .await;
        assert_eq!(
            fulfilled.len(),
            1,
            "case {case:?}: expected the log to still hold one activity, got {fulfilled:?}"
        );
        let done = &fulfilled[0];
        assert_eq!(
            done["ActivityId"], started["ActivityId"],
            "case {case:?}: expected the same activity, got {done:?}"
        );
        assert_eq!(
            done["StatusCode"],
            json!("Successful"),
            "case {case:?}: expected the movement to be Successful by now, got {done:?}"
        );
        let expected_message = format!(
            "Successfully set {} to {}. Change successfully fulfilled by {}.",
            row.noun, row.capacity, row.namespace
        );
        assert_eq!(
            done["StatusMessage"],
            json!(expected_message),
            "case {case:?}: expected StatusMessage {expected_message:?}, got {done:?}"
        );
        let end_time = done["EndTime"].as_f64().unwrap_or_else(|| {
            panic!("case {case:?}: expected a numeric EndTime once fulfilled, got {done:?}")
        });
        assert!(
            end_time >= start_time,
            "case {case:?}: expected EndTime {end_time} at or after StartTime {start_time}"
        );
        assert_eq!(
            done["StartTime"], started["StartTime"],
            "case {case:?}: expected StartTime not to move, got {done:?}"
        );

        // A read must not change what the next reader sees.
        let (again, _) = activities(
            emulator,
            json!({"ServiceNamespace": row.namespace}),
            &case,
        )
        .await;
        assert_eq!(
            again, fulfilled,
            "case {case:?}: expected a second read to answer identically, got {again:?}"
        );

        // Not-scaled activities are the ones that were never started; a movement that
        // was performed is reported either way and carries no reasons.
        let (with_not_scaled, _) = activities(
            emulator,
            json!({
                "ServiceNamespace": row.namespace,
                "IncludeNotScaledActivities": true,
            }),
            &case,
        )
        .await;
        assert_eq!(
            with_not_scaled, fulfilled,
            "case {case:?}: expected IncludeNotScaledActivities to add nothing to a log of \
             performed movements, got {with_not_scaled:?}"
        );
        assert_eq!(
            fulfilled[0]["NotScaledReasons"],
            Value::Null,
            "case {case:?}: expected a performed movement to carry no NotScaledReasons, \
             got {:?}",
            fulfilled[0]
        );
    }
}

/// The clock, not a later call, is what fulfils a movement: one registration, two
/// reads bracketing the fulfilment instant.
#[tokio::test]
async fn a_movement_is_observable_before_the_clock_fulfils_it() {
    let emulator = Emulator::empty();
    let case = "a movement in flight";
    let identity = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/in-flight",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });
    register(
        &emulator,
        merged(identity.clone(), json!({"MinCapacity": 1, "MaxCapacity": 10})),
        case,
    )
    .await;
    register(
        &emulator,
        merged(identity, json!({"MinCapacity": 7})),
        case,
    )
    .await;

    // Nothing ticks: the movement is non-terminal because its fulfilment instant has
    // not arrived, so a caller acting straight after the call still sees it in flight.
    let (in_flight, _) = activities(
        &emulator,
        json!({"ServiceNamespace": "dynamodb"}),
        case,
    )
    .await;
    assert_eq!(
        in_flight.len(),
        1,
        "case {case:?}: expected one activity, got {in_flight:?}"
    );
    assert_eq!(
        in_flight[0]["StatusCode"],
        json!("InProgress"),
        "case {case:?}: expected the movement to still be InProgress, got {:?}",
        in_flight[0]
    );

    sleep(FULFILMENT_WAIT);
    let (fulfilled, _) = activities(
        &emulator,
        json!({"ServiceNamespace": "dynamodb"}),
        case,
    )
    .await;
    assert_eq!(
        fulfilled[0]["StatusCode"],
        json!("Successful"),
        "case {case:?}: expected the clock alone to fulfil the movement, got {:?}",
        fulfilled[0]
    );
    assert_eq!(
        fulfilled[0]["ActivityId"], in_flight[0]["ActivityId"],
        "case {case:?}: expected the same activity, not a second one, got {:?}",
        fulfilled[0]
    );
}

/// A registration that leaves the capacity inside the window, so nothing is moved and
/// nothing may be reported. The rows differ in which members the update names.
struct QuietRow {
    case: &'static str,
    patch: fn() -> Value,
}

const QUIET_CATALOG: &[QuietRow] = &[
    QuietRow {
        case: "the same window again",
        patch: || json!({"MinCapacity": 2, "MaxCapacity": 10}),
    },
    QuietRow {
        case: "a wider window",
        patch: || json!({"MinCapacity": 1, "MaxCapacity": 40}),
    },
    QuietRow {
        case: "a higher ceiling only",
        patch: || json!({"MaxCapacity": 12}),
    },
    QuietRow {
        case: "a lower floor only",
        // The floor of a DynamoDB capacity dimension is one provisioned unit, so this is
        // as low as the window goes; the stored capacity of 2 stays inside it.
        patch: || json!({"MinCapacity": 1}),
    },
    QuietRow {
        case: "a ceiling still above the capacity",
        patch: || json!({"MaxCapacity": 3}),
    },
    QuietRow {
        case: "no capacity member at all",
        patch: || json!({"RoleARN": "arn:aws:iam::000000000000:role/aas-other"}),
    },
    QuietRow {
        case: "a suspended-state update",
        patch: || json!({"SuspendedState": {"DynamicScalingInSuspended": true}}),
    },
];

#[tokio::test]
async fn a_registration_that_moves_no_capacity_reports_no_activity() {
    let target = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/quiet-table",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });

    for row in QUIET_CATALOG {
        let case = row.case;
        let emulator = Emulator::empty();
        register(
            &emulator,
            merged(target.clone(), json!({"MinCapacity": 2, "MaxCapacity": 10})),
            case,
        )
        .await;
        register(&emulator, merged(target.clone(), (row.patch)()), case).await;

        let (listed, _) = activities(
            &emulator,
            json!({"ServiceNamespace": "dynamodb", "IncludeNotScaledActivities": true}),
            case,
        )
        .await;
        assert!(
            listed.is_empty(),
            "case {case:?}: expected no activity when no capacity moved, got {listed:?}"
        );
    }
}

/// The floors a run of registrations raises the capacity to, one movement each.
const RAISED_FLOORS: &[i64] = &[2, 3, 4, 5, 6];

/// Every page size the paging case walks the log at: below, at and above the number of
/// activities, plus the documented bounds of `MaxResults` (1 and 50, "this value can be
/// between 1 and 50").
const PAGE_SIZES: &[i64] = &[1, 2, 3, 5, 6, 50];

#[tokio::test]
async fn the_log_reads_newest_first_and_pages_at_max_results() {
    let emulator = Emulator::empty();
    let target = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/paged-table",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });
    register(
        &emulator,
        merged(target.clone(), json!({"MinCapacity": 1, "MaxCapacity": 50})),
        "setup",
    )
    .await;
    for floor in RAISED_FLOORS {
        register(
            &emulator,
            merged(target.clone(), json!({"MinCapacity": floor})),
            "setup",
        )
        .await;
    }

    // Newest first: the last floor raised is the first activity reported.
    let expected: Vec<String> = RAISED_FLOORS
        .iter()
        .rev()
        .map(|floor| format!("Setting read capacity units to {floor}."))
        .collect();
    let (all, token) = activities(
        &emulator,
        json!({"ServiceNamespace": "dynamodb"}),
        "newest first",
    )
    .await;
    assert_eq!(
        descriptions(&all),
        expected,
        "case \"newest first\": expected the log newest first, got {:?}",
        descriptions(&all)
    );
    assert_eq!(
        token, None,
        "case \"newest first\": expected no NextToken when every activity fits, got {token:?}"
    );

    for page_size in PAGE_SIZES {
        let case = format!("MaxResults {page_size}");
        let mut walked: Vec<Value> = Vec::new();
        let mut next: Option<String> = None;
        let mut pages = 0;
        loop {
            let mut request = json!({"ServiceNamespace": "dynamodb", "MaxResults": page_size});
            if let Some(token) = &next {
                request["NextToken"] = json!(token);
            }
            let (page, token) = activities(&emulator, request, &case).await;
            assert!(
                page.len() as i64 <= *page_size,
                "case {case:?}: expected at most {page_size} activities in a page, got {page:?}"
            );
            assert!(
                !page.is_empty(),
                "case {case:?}: expected a non-empty page while a NextToken was offered"
            );
            walked.extend(page);
            pages += 1;
            assert!(
                pages <= RAISED_FLOORS.len() + 1,
                "case {case:?}: expected the walk to terminate, it took {pages} pages"
            );
            match token {
                None => break,
                Some(token) => next = Some(token),
            }
        }
        assert_eq!(
            descriptions(&walked),
            expected,
            "case {case:?}: expected the walk to visit the whole log newest first, got {:?}",
            descriptions(&walked)
        );
        let ids: BTreeSet<&str> = walked
            .iter()
            .filter_map(|activity| activity["ActivityId"].as_str())
            .collect();
        assert_eq!(
            ids.len(),
            RAISED_FLOORS.len(),
            "case {case:?}: expected {} distinct ActivityIds, got {ids:?}",
            RAISED_FLOORS.len()
        );
    }
}

/// A filter, and the (resource id, dimension) pairs whose activities it must select.
struct FilterRow {
    case: &'static str,
    request: fn() -> Value,
    expected: &'static [(&'static str, &'static str)],
}

const FILTER_CATALOG: &[FilterRow] = &[
    FilterRow {
        case: "a namespace selects every resource in it",
        request: || json!({"ServiceNamespace": "dynamodb"}),
        expected: &[
            ("table/one", "dynamodb:table:ReadCapacityUnits"),
            ("table/one", "dynamodb:table:WriteCapacityUnits"),
            ("table/two", "dynamodb:table:ReadCapacityUnits"),
        ],
    },
    FilterRow {
        case: "another namespace selects only its own",
        request: || json!({"ServiceNamespace": "ecs"}),
        expected: &[("service/my-cluster/my-service", "ecs:service:DesiredCount")],
    },
    FilterRow {
        case: "a resource id narrows to one resource",
        request: || json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/one"}),
        expected: &[
            ("table/one", "dynamodb:table:ReadCapacityUnits"),
            ("table/one", "dynamodb:table:WriteCapacityUnits"),
        ],
    },
    FilterRow {
        case: "a resource id and a dimension narrow to one target",
        request: || {
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/one",
                "ScalableDimension": "dynamodb:table:WriteCapacityUnits",
            })
        },
        expected: &[("table/one", "dynamodb:table:WriteCapacityUnits")],
    },
    FilterRow {
        case: "a resource with no movement selects nothing",
        request: || json!({"ServiceNamespace": "dynamodb", "ResourceId": "table/three"}),
        expected: &[],
    },
];

#[tokio::test]
async fn resource_id_and_scalable_dimension_filter_the_log() {
    let emulator = Emulator::empty();
    // Four targets that moved, and one that did not.
    let moved = [
        ("dynamodb", "table/one", "dynamodb:table:ReadCapacityUnits"),
        ("dynamodb", "table/one", "dynamodb:table:WriteCapacityUnits"),
        ("dynamodb", "table/two", "dynamodb:table:ReadCapacityUnits"),
        (
            "ecs",
            "service/my-cluster/my-service",
            "ecs:service:DesiredCount",
        ),
    ];
    for (namespace, resource_id, dimension) in moved {
        let identity = json!({
            "ServiceNamespace": namespace,
            "ResourceId": resource_id,
            "ScalableDimension": dimension,
        });
        register(
            &emulator,
            merged(identity.clone(), json!({"MinCapacity": 1, "MaxCapacity": 9})),
            "setup",
        )
        .await;
        register(
            &emulator,
            merged(identity, json!({"MinCapacity": 4})),
            "setup",
        )
        .await;
    }
    let quiet = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/three",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
        "MinCapacity": 1,
        "MaxCapacity": 9,
    });
    register(&emulator, quiet, "setup").await;

    for row in FILTER_CATALOG {
        let case = row.case;
        let (listed, _) = activities(&emulator, (row.request)(), case).await;
        let selected: BTreeSet<(String, String)> = listed
            .iter()
            .map(|activity| {
                (
                    activity["ResourceId"].as_str().unwrap_or("").to_string(),
                    activity["ScalableDimension"]
                        .as_str()
                        .unwrap_or("")
                        .to_string(),
                )
            })
            .collect();
        let expected: BTreeSet<(String, String)> = row
            .expected
            .iter()
            .map(|(resource_id, dimension)| (resource_id.to_string(), dimension.to_string()))
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: expected the filter to select {expected:?}, got {selected:?}"
        );
        assert_eq!(
            listed.len(),
            row.expected.len(),
            "case {case:?}: expected {} activities, got {listed:?}",
            row.expected.len()
        );
    }
}

/// A request outside the operation's contract, and the error code it must be refused
/// with.
struct RefusalRow {
    case: &'static str,
    request: fn() -> Value,
    code: &'static str,
}

const REFUSAL_CATALOG: &[RefusalRow] = &[
    RefusalRow {
        // "If you specify a scalable dimension, you must also specify a resource ID."
        case: "a scalable dimension without a resource id",
        request: || {
            json!({
                "ServiceNamespace": "dynamodb",
                "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
            })
        },
        code: "ValidationException",
    },
    RefusalRow {
        case: "a dimension of another namespace",
        request: || {
            json!({
                "ServiceNamespace": "dynamodb",
                "ResourceId": "table/one",
                "ScalableDimension": "ecs:service:DesiredCount",
            })
        },
        code: "ValidationException",
    },
    RefusalRow {
        // "This value can be between 1 and 50" — one past each bound.
        case: "MaxResults below the documented range",
        request: || json!({"ServiceNamespace": "dynamodb", "MaxResults": 0}),
        code: "ValidationException",
    },
    RefusalRow {
        case: "MaxResults above the documented range",
        request: || json!({"ServiceNamespace": "dynamodb", "MaxResults": 51}),
        code: "ValidationException",
    },
    RefusalRow {
        case: "a NextToken this service never issued",
        request: || json!({"ServiceNamespace": "dynamodb", "NextToken": "not-a-token"}),
        code: "InvalidNextTokenException",
    },
];

#[tokio::test]
async fn a_request_outside_the_contract_is_refused_with_the_modelled_error() {
    let emulator = Emulator::empty();
    let identity = json!({
        "ServiceNamespace": "dynamodb",
        "ResourceId": "table/one",
        "ScalableDimension": "dynamodb:table:ReadCapacityUnits",
    });
    register(
        &emulator,
        merged(identity.clone(), json!({"MinCapacity": 1, "MaxCapacity": 9})),
        "setup",
    )
    .await;
    register(
        &emulator,
        merged(identity.clone(), json!({"MinCapacity": 4})),
        "setup",
    )
    .await;
    register(
        &emulator,
        merged(identity, json!({"MinCapacity": 5})),
        "setup",
    )
    .await;

    for row in REFUSAL_CATALOG {
        let case = row.case;
        let (status, body) = emulator
            .call("DescribeScalingActivities", (row.request)())
            .await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected the request to be refused with 400, got {status} {body:?}"
        );
        assert!(
            error_code(&body).ends_with(row.code),
            "case {case:?}: expected error code {:?}, got {:?}",
            row.code,
            error_code(&body)
        );
    }

    // Each documented bound of `MaxResults` is accepted.
    for page_size in [1, 50] {
        let case = format!("MaxResults {page_size}");
        let (listed, _) = activities(
            &emulator,
            json!({"ServiceNamespace": "dynamodb", "MaxResults": page_size}),
            &case,
        )
        .await;
        assert!(
            !listed.is_empty(),
            "case {case:?}: expected the bound to be accepted and answer activities"
        );
    }

    // A token this service DID issue, replayed against a filter whose log does not hold
    // the activity it names, is just as invalid as a forged one.
    let (_, token) = activities(
        &emulator,
        json!({"ServiceNamespace": "dynamodb", "MaxResults": 1}),
        "a token from another filter",
    )
    .await;
    let token = token.expect("a two-activity log paged at 1 offers a NextToken");
    let (status, body) = emulator
        .call(
            "DescribeScalingActivities",
            json!({
                "ServiceNamespace": "ecs",
                "NextToken": token,
            }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"a token from another filter\": expected 400, got {status} {body:?}"
    );
    assert!(
        error_code(&body).ends_with("InvalidNextTokenException"),
        "case \"a token from another filter\": expected InvalidNextTokenException, got {:?}",
        error_code(&body)
    );
}
