// SPDX-License-Identifier: CC0-1.0

//! What a plan reports between an `UpdateScalingPlan` and the moment the plan has
//! APPLIED it.
//!
//! The rule: a scaling plan reports the configuration it has applied to the resources it
//! covers, so the members an in-place update asks for — its `ScalingInstructions`, its
//! `ApplicationSource` and the narrative of its applied status alike — become what the
//! plan reports only once it has applied them, which the lifecycle clock owes after the
//! request. `UpdateScalingPlan` "Updates the specified scaling plan" and answers an EMPTY
//! body, and `UpdateScalingPlanRequest$ScalingInstructions` is "The scaling instructions"
//! (`smithy_query(query="operation",operation="UpdateScalingPlan")`), so this read-back is
//! the ONLY place a caller learns what an update took: a read that echoed the request
//! instead could never tell an update that has been applied from one that has not.
//!
//! What the update then puts in force, entry for entry and member for member, is spanned
//! by [`crate::update_replaces_the_instruction_set`]; this suite spans the window BEFORE
//! that, over a table whose rows differ along what the rule generalises over — which
//! member of the request moves, the number of tracked configurations, the number of
//! resources, the form of the resource, a boolean member, and a request that asks for
//! what is already in force.
//!
//! The clock is driven by moving the stored due instants, never by sleeping: the harness
//! (router, dispatch, lifecycle clock) is [`crate::scaling_plan_lifecycle`]'s and the two
//! describe readers are [`crate::applied_policies_and_status_narratives`]'s; nothing here
//! builds a second one.

use serde_json::{json, Value};

use crate::applied_policies_and_status_narratives::{applied_plan, described, resources};
use crate::scaling_plan_lifecycle::{instruction, stack_source, Emulator};

/// The narrative a plan reports once its creation has been applied.
const CREATED_MESSAGE: &str = "Scaling plan has been created and applied to all resources.";

/// The narrative a plan reports once an in-place update of it has been applied.
const UPDATED_MESSAGE: &str = "Scaling plan has been updated successfully.";

/// The plan every case here writes.
const PLAN: &str = "my-plan";

/// A DynamoDB table instruction tracking the read-capacity metric at `target`.
fn table_instruction(target: f64) -> Value {
    instruction(
        "dynamodb",
        "table/my-table",
        "dynamodb:table:ReadCapacityUnits",
        1,
        100,
        "DynamoDBReadCapacityUtilization",
        target,
    )
}

/// An ECS service instruction, so a case can move the plan onto another resource.
fn service_instruction(target: f64) -> Value {
    instruction(
        "ecs",
        "service/default/sample-webapp",
        "ecs:service:DesiredCount",
        2,
        20,
        "ECSServiceAverageCPUUtilization",
        target,
    )
}

/// A target tracking configuration over a CUSTOMIZED metric — the other form a tracked
/// metric takes. `Statistic` is a modelled `MetricStatistic` value
/// (`smithy_query(query="shape",shape="MetricStatistic")`).
fn customized(metric_name: &str, target: f64) -> Value {
    json!({
        "CustomizedScalingMetricSpecification": {
            "Namespace": "MyApp",
            "MetricName": metric_name,
            "Statistic": "Sum"
        },
        "TargetValue": target
    })
}

/// `instruction` with the members `extras` carries merged in.
fn with(mut instruction: Value, extras: Value) -> Value {
    for (member, value) in extras.as_object().cloned().unwrap_or_default() {
        instruction[member] = value;
    }
    instruction
}

/// One row: what the plan has in force, what the update asks for, and whether that
/// request changes what the plan APPLIES to its resources (which is what the narrative of
/// the applied status reports).
struct Case {
    label: &'static str,
    in_force: Vec<Value>,
    requested: Option<Vec<Value>>,
    source: Option<Value>,
    changes_what_is_applied: bool,
}

/// The table this suite spans.
fn cases() -> Vec<Case> {
    let tracked = table_instruction(50.0);
    vec![
        Case {
            label: "the target value of the tracked metric rises",
            in_force: vec![tracked.clone()],
            requested: Some(vec![table_instruction(75.0)]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "the target value of the tracked metric falls",
            in_force: vec![tracked.clone()],
            requested: Some(vec![table_instruction(12.5)]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "the capacity band moves with the target value",
            in_force: vec![tracked.clone()],
            requested: Some(vec![with(
                table_instruction(65.0),
                json!({ "MinCapacity": 4, "MaxCapacity": 400 }),
            )]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "a second configuration is added to the resource",
            in_force: vec![tracked.clone()],
            requested: Some(vec![with(
                table_instruction(50.0),
                json!({
                    "TargetTrackingConfigurations": [
                        {
                            "PredefinedScalingMetricSpecification": {
                                "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                            },
                            "TargetValue": 50.0
                        },
                        customized("queue-depth", 30.0)
                    ]
                }),
            )]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "a second resource joins the plan",
            in_force: vec![tracked.clone()],
            requested: Some(vec![table_instruction(50.0), service_instruction(40.0)]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "the one resource in force is swapped for another",
            in_force: vec![tracked.clone()],
            requested: Some(vec![service_instruction(55.5)]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "dynamic scaling is turned off on the resource",
            in_force: vec![tracked.clone()],
            requested: Some(vec![with(
                table_instruction(50.0),
                json!({ "DisableDynamicScaling": true }),
            )]),
            source: None,
            changes_what_is_applied: true,
        },
        Case {
            label: "the application source moves and the instructions stay",
            in_force: vec![tracked.clone()],
            requested: None,
            source: Some(stack_source(2)),
            changes_what_is_applied: true,
        },
        Case {
            label: "the update re-sends the configuration in force",
            in_force: vec![tracked.clone()],
            requested: Some(vec![tracked.clone()]),
            source: None,
            changes_what_is_applied: false,
        },
    ]
}

/// Update [`PLAN`] with what `case` asks for.
async fn update(emulator: &Emulator, case: &Case) {
    let mut request = json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 });
    if let Some(instructions) = &case.requested {
        request["ScalingInstructions"] = Value::Array(instructions.clone());
    }
    if let Some(source) = &case.source {
        request["ApplicationSource"] = source.clone();
    }
    let body = emulator.ok("UpdateScalingPlan", request).await;
    assert_eq!(
        body,
        json!({}),
        "case {:?}: UpdateScalingPlan answers an empty body, got {body:?}",
        case.label
    );
}

/// Settle the plan's own status transition while the application of a staged update is
/// still owed — the window this suite is about, held open at the stored instant rather
/// than raced against the real span.
fn settled_but_not_applied(emulator: &Emulator) {
    emulator.make_all_due();
    emulator.hold_staged(PLAN);
}

#[tokio::test]
async fn a_read_reports_what_is_in_force_until_the_plan_has_applied_the_update() {
    for case in cases() {
        let label = case.label;
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, case.in_force.clone()).await;
        let created = described(&emulator, PLAN).await;

        update(&emulator, &case).await;
        settled_but_not_applied(&emulator);

        // The plan is back in its applied status, and what it reports there is the
        // configuration — and the narrative — of the application in force.
        let reading = described(&emulator, PLAN).await;
        assert_eq!(
            (
                reading.get("StatusCode"),
                reading.get("StatusMessage"),
                reading.get("ScalingInstructions"),
                reading.get("ApplicationSource"),
            ),
            (
                Some(&json!("Active")),
                Some(&json!(CREATED_MESSAGE)),
                created.get("ScalingInstructions"),
                created.get("ApplicationSource"),
            ),
            "case {label:?}: an update the plan has not applied must not change what it \
             reports, got {reading:?}"
        );
        // The identity and the timestamps of the plan are the created plan's throughout:
        // an update moves neither, which is why the configuration and the narrative are
        // what a caller reads it back for.
        for member in ["ScalingPlanName", "ScalingPlanVersion", "CreationTime"] {
            assert_eq!(
                reading.get(member),
                created.get(member),
                "case {label:?}: {member} must not move on an update, got {reading:?}"
            );
        }

        // Once the plan HAS applied it, the read reports what the request asked for — and
        // the narrative reports an update exactly when the request changed what the plan
        // applies to its resources.
        emulator.make_all_due();
        let applied = described(&emulator, PLAN).await;
        let expected_instructions =
            Value::Array(case.requested.clone().unwrap_or(case.in_force.clone()));
        let expected_source = case
            .source
            .clone()
            .unwrap_or_else(|| created["ApplicationSource"].clone());
        let expected_message = if case.changes_what_is_applied {
            UPDATED_MESSAGE
        } else {
            CREATED_MESSAGE
        };
        assert_eq!(
            (
                applied.get("StatusCode"),
                applied.get("StatusMessage"),
                applied.get("ScalingInstructions"),
                applied.get("ApplicationSource"),
            ),
            (
                Some(&json!("Active")),
                Some(&json!(expected_message)),
                Some(&expected_instructions),
                Some(&expected_source),
            ),
            "case {label:?}: an applied update must be what the plan reports, got \
             {applied:?}"
        );
    }
}

/// One row of the resource-envelope table: the configuration in force, the one the update
/// asks for, and the target values the plan's resources report before and after the plan
/// has applied it.
struct PolicyCase {
    label: &'static str,
    in_force: Vec<Value>,
    requested: Vec<Value>,
    before: Vec<f64>,
    after: Vec<f64>,
}

#[tokio::test]
async fn a_plans_resources_report_the_policies_of_the_configuration_in_force() {
    // The same rule read through the other describe envelope: the policies a resource
    // carries are derived from the instruction in force on it, so they move when the plan
    // has applied the update and not before. Rows: a re-pointed metric, and one of two
    // configurations dropped.
    let both = with(
        table_instruction(50.0),
        json!({
            "TargetTrackingConfigurations": [
                {
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "DynamoDBReadCapacityUtilization"
                    },
                    "TargetValue": 50.0
                },
                customized("queue-depth", 30.0)
            ]
        }),
    );
    let rows = vec![
        PolicyCase {
            label: "the tracked metric is re-pointed",
            in_force: vec![table_instruction(50.0)],
            requested: vec![table_instruction(75.0)],
            before: vec![50.0],
            after: vec![75.0],
        },
        PolicyCase {
            label: "one of two configurations is dropped and the survivor re-pointed",
            in_force: vec![both],
            requested: vec![with(
                table_instruction(50.0),
                json!({ "TargetTrackingConfigurations": [customized("queue-depth", 60.0)] }),
            )],
            before: vec![50.0, 30.0],
            after: vec![60.0],
        },
    ];

    for row in rows {
        let label = row.label;
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, row.in_force.clone()).await;
        let case = Case {
            label,
            in_force: row.in_force,
            requested: Some(row.requested),
            source: None,
            changes_what_is_applied: true,
        };
        update(&emulator, &case).await;
        settled_but_not_applied(&emulator);

        let tracked = tracked_target_values(&emulator).await;
        assert_eq!(
            tracked, row.before,
            "case {label:?}: the resources must report the policies of the configuration \
             in force, got {tracked:?}"
        );

        emulator.make_all_due();
        let tracked = tracked_target_values(&emulator).await;
        assert_eq!(
            tracked, row.after,
            "case {label:?}: once applied, the resources must report the policies of the \
             configuration the update put in force, got {tracked:?}"
        );
    }
}

/// The target values the policies of [`PLAN`]'s resources track, in the order the
/// resources and their policies are reported.
async fn tracked_target_values(emulator: &Emulator) -> Vec<f64> {
    resources(emulator, PLAN)
        .await
        .iter()
        .flat_map(|resource| {
            resource
                .get("ScalingPolicies")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default()
        })
        .filter_map(|policy| policy["TargetTrackingConfiguration"]["TargetValue"].as_f64())
        .collect()
}

/// One row of the superseding-update table: the two updates made before either is
/// applied, and the configuration the plan converges on.
struct SupersedingCase {
    label: &'static str,
    first: Value,
    second: Value,
    instructions: Vec<Value>,
    source: Value,
}

#[tokio::test]
async fn the_plan_converges_on_what_the_latest_request_to_carry_each_member_asked_for() {
    // Two updates, the second made before the plan has applied the first: what the plan
    // converges on is the member the latest request to carry it asked for, and a member
    // only the earlier request carried still applies — the same "replace only what the
    // request carries" rule, read over a request that has not been applied yet.
    let rows = vec![
        SupersedingCase {
            label: "the instructions then another member",
            first: json!({ "ScalingInstructions": [table_instruction(75.0)] }),
            second: json!({ "ApplicationSource": stack_source(2) }),
            instructions: vec![table_instruction(75.0)],
            source: stack_source(2),
        },
        SupersedingCase {
            label: "another member then the instructions",
            first: json!({ "ApplicationSource": stack_source(2) }),
            second: json!({ "ScalingInstructions": [table_instruction(75.0)] }),
            instructions: vec![table_instruction(75.0)],
            source: stack_source(2),
        },
        SupersedingCase {
            label: "the same member twice: the later request wins",
            first: json!({ "ScalingInstructions": [table_instruction(75.0)] }),
            second: json!({ "ScalingInstructions": [table_instruction(60.0)] }),
            instructions: vec![table_instruction(60.0)],
            source: stack_source(1),
        },
    ];

    for row in rows {
        let label = row.label;
        let emulator = Emulator::new();
        applied_plan(&emulator, PLAN, vec![table_instruction(50.0)]).await;

        for extras in [&row.first, &row.second] {
            let mut request = json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 });
            for (member, value) in extras.as_object().cloned().unwrap_or_default() {
                request[member] = value;
            }
            emulator.ok("UpdateScalingPlan", request).await;
            // The plan is back in its applied status before the next request, so that one
            // is admitted rather than refused as a concurrent update; neither application
            // has come due.
            settled_but_not_applied(&emulator);
        }

        emulator.make_all_due();
        let applied = described(&emulator, PLAN).await;
        assert_eq!(
            (
                applied.get("ScalingInstructions"),
                applied.get("ApplicationSource"),
            ),
            (
                Some(&Value::Array(row.instructions.clone())),
                Some(&row.source),
            ),
            "case {label:?}: the plan must converge on what the latest request to carry \
             each member asked for, got {applied:?}"
        );
    }
}

#[tokio::test]
async fn a_delete_before_the_update_is_applied_takes_the_plan_with_it() {
    // A plan being withdrawn applies nothing further: the update it never applied does
    // not bring it back, and the clock has nothing left to put in force.
    let emulator = Emulator::new();
    applied_plan(&emulator, PLAN, vec![table_instruction(50.0)]).await;
    let case = Case {
        label: "a delete before the update is applied",
        in_force: vec![table_instruction(50.0)],
        requested: Some(vec![table_instruction(75.0)]),
        source: None,
        changes_what_is_applied: true,
    };
    update(&emulator, &case).await;
    settled_but_not_applied(&emulator);

    emulator
        .ok(
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": PLAN, "ScalingPlanVersion": 1 }),
        )
        .await;
    emulator.make_all_due();

    let body = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        body["ScalingPlans"],
        json!([]),
        "case a delete before the update was applied: the plan must be gone and stay \
         gone, got {body:?}"
    );
}
