// SPDX-License-Identifier: CC0-1.0

//! The two rules that decide a customized metric's `Statistic`, and which of them
//! answers for a given value.
//!
//! ONE rule is pinned here, over a TABLE spanning both members that carry the modelled
//! `MetricStatistic` enum — a scaling instruction's `CustomizedLoadMetricSpecification`
//! and the `CustomizedScalingMetricSpecification` of one of its target tracking
//! configurations (`smithy_query(query="shape",shape="CustomizedLoadMetricSpecification,
//! CustomizedScalingMetricSpecification")`: both `Statistic` members target
//! `MetricStatistic`): a value that enum does not list is refused with the ONE off-enum
//! sentence, naming the member, the rejected value and the values the model lists,
//! whichever of the two members carried it; a member's own narrower rule — "The statistic
//! of the metric. The only valid value is `Sum`" on the LOAD member — speaks about a
//! statistic the enum DOES list, so it answers only for such a value and never for one
//! outside the enum.
//!
//! The rows differ along what the rule generalises over: the MEMBER that carries the
//! statistic, the FORM of the miss (nothing at all, an unmodelled name, a listed value
//! with a character added, with a trailing space, in the wrong case), the on-enum value
//! itself (every value the model lists), and the OPERATION that receives the instruction.
//!
//! The wire tests assert PARITY: the sentence the two sibling members earn for one and
//! the same off-enum value is the same sentence, differing only in the member it names.
//! The validation-layer tests hold this crate's own refusal layer
//! ([`plan_state::validate_scaling_instructions`], which both write paths call) to the
//! same rule, so the answer does not depend on which layer refuses first.
//!
//! The harness (router, dispatch, error-body readers) is reused from
//! [`crate::scaling_plan_lifecycle`]; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::{
    CustomizedLoadMetricSpecification, CustomizedScalingMetricSpecification,
    PredefinedScalingMetricSpecification, ScalingInstruction, TargetTrackingConfiguration,
    METRIC_STATISTIC_VALUES,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state as plan_state;

use crate::scaling_plan_lifecycle::{
    error_code, error_message, simple_instructions, stack_source, Emulator,
};

/// The statistic the LOAD member's own documentation admits: "The only valid value is
/// `Sum`."
const LOAD_STATISTIC: &str = "Sum";

/// The plan the update rows mutate and the create rows must not collide with.
const STORED_PLAN: &str = "statistic probe plan";

/// The plan name the create rows use.
const NEW_PLAN: &str = "statistic create plan";

/// The two operations that take a `ScalingInstructions` list, so every row is driven
/// through both.
const WRITE_OPERATIONS: [&str; 2] = ["CreateScalingPlan", "UpdateScalingPlan"];

/// What the member a sentence names is replaced with before two sentences about the same
/// value on two different members are compared.
const ANY_MEMBER: &str = "<member>";

/// How a value can miss the `MetricStatistic` value set, by form of the miss. Each probe
/// is built from a value the model DOES list, so no row depends on a particular name.
fn off_enum_statistics() -> Vec<(&'static str, String)> {
    let listed = METRIC_STATISTIC_VALUES[0];
    let mut probes = vec![
        ("nothing at all", String::new()),
        ("an unmodelled name", "Bogus".to_string()),
        ("a listed value with a trailing character", format!("{listed}X")),
        ("a listed value with a trailing space", format!("{listed} ")),
        ("a listed value in lower case", listed.to_lowercase()),
        ("a listed value in upper case", listed.to_uppercase()),
    ];
    probes.retain(|(_, value)| !METRIC_STATISTIC_VALUES.contains(&value.as_str()));
    probes
}

// ---------------------------------------------------------------------------
// Over the wire
// ---------------------------------------------------------------------------

/// The valid one-instruction list both write paths accept, carrying a customized LOAD
/// metric whose statistic is `statistic`.
fn with_load_statistic(statistic: &str) -> Value {
    let mut instruction = simple_instructions().remove(0);
    instruction["CustomizedLoadMetricSpecification"] = json!({
        "MetricName": "my-load",
        "Namespace": "MyApp",
        "Statistic": statistic,
    });
    json!([instruction])
}

/// The same list, carrying a customized SCALING metric whose statistic is `statistic` on
/// its single target tracking configuration.
fn with_scaling_statistic(statistic: &str) -> Value {
    let mut instruction = simple_instructions().remove(0);
    instruction["TargetTrackingConfigurations"] = json!([{
        "CustomizedScalingMetricSpecification": {
            "MetricName": "my-metric",
            "Namespace": "MyApp",
            "Statistic": statistic,
        },
        "TargetValue": 40.0,
    }]);
    json!([instruction])
}

/// One write request carrying `instructions`, in the envelope `operation` requires.
fn write_request(operation: &str, instructions: Value) -> Value {
    if operation == "CreateScalingPlan" {
        return json!({
            "ScalingPlanName": NEW_PLAN,
            "ApplicationSource": stack_source(2),
            "ScalingInstructions": instructions,
        });
    }
    json!({
        "ScalingPlanName": STORED_PLAN,
        "ScalingPlanVersion": 1,
        "ScalingInstructions": instructions,
    })
}

/// An emulator holding one settled plan, so an update row names a plan that exists and a
/// create row still writes a plan of its own (under a different application source).
async fn with_one_stored_plan() -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": STORED_PLAN,
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The plan `operation` writes to.
fn written_plan(operation: &str) -> &'static str {
    if operation == "CreateScalingPlan" {
        NEW_PLAN
    } else {
        STORED_PLAN
    }
}

/// The statistics the customized SCALING metric specifications of the one instruction
/// `operation` wrote carry, read back through `DescribeScalingPlans`.
async fn stored_scaling_statistics(emulator: &Emulator, operation: &str) -> Vec<String> {
    let read = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ScalingPlanNames": [written_plan(operation)] }),
        )
        .await;
    read["ScalingPlans"][0]["ScalingInstructions"][0]["TargetTrackingConfigurations"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|configuration| {
            configuration["CustomizedScalingMetricSpecification"]["Statistic"]
                .as_str()
                .map(str::to_string)
        })
        .collect()
}

/// Dispatch one write that must be refused, and answer the message it was refused with.
async fn refusal(emulator: &Emulator, case: &str, operation: &str, request: &Value) -> String {
    let (status, body) = emulator.call(operation, request).await;
    assert_eq!(
        (status, error_code(&body)),
        (400, "ValidationException"),
        "case {case:?}: expected HTTP 400 ValidationException, got {status} with {body}"
    );
    error_message(&body).to_string()
}

#[tokio::test]
async fn an_off_enum_statistic_earns_the_one_sentence_on_either_customized_metric() {
    let emulator = with_one_stored_plan().await;

    for (miss, statistic) in off_enum_statistics() {
        for operation in WRITE_OPERATIONS {
            let case = format!("{operation} / an off-enum statistic: {miss}");
            // Both rows are refused, so neither write leaves anything behind and one
            // emulator serves every row.
            let load = refusal(
                &emulator,
                &format!("{case} / the load metric"),
                operation,
                &write_request(operation, with_load_statistic(&statistic)),
            )
            .await;
            let scaling = refusal(
                &emulator,
                &format!("{case} / the scaling metric"),
                operation,
                &write_request(operation, with_scaling_statistic(&statistic)),
            )
            .await;

            // The sentence has to state the facts only it can state: that THIS value was
            // rejected, and what the model accepts instead.
            for (member, actual) in [("load", &load), ("scaling", &scaling)] {
                assert!(
                    actual.contains(&format!("'{statistic}'")),
                    "case {case:?} / the {member} metric: the refusal must name the \
                     rejected value {statistic:?}, got {actual:?}"
                );
                for accepted in METRIC_STATISTIC_VALUES {
                    assert!(
                        actual.contains(accepted),
                        "case {case:?} / the {member} metric: the refusal must list the \
                         accepted value {accepted:?}, got {actual:?}"
                    );
                }
                assert!(
                    !actual.contains("the only valid value"),
                    "case {case:?} / the {member} metric: a value the enum does not list \
                     must not be answered by a rule about a value it lists, got {actual:?}"
                );
            }

            // PARITY: one off-enum value, one sentence — the member it names is the only
            // thing that may differ between the two siblings.
            let load_template = load.replace("customizedLoadMetricSpecification", ANY_MEMBER);
            let scaling_template = scaling
                .replace("targetTrackingConfigurations.1.member.", "")
                .replace("customizedScalingMetricSpecification", ANY_MEMBER);
            assert_eq!(
                load_template, scaling_template,
                "case {case:?}: the two sibling members must be refused with the same \
                 sentence, got {load:?} for the load metric and {scaling:?} for the \
                 scaling metric"
            );
        }
    }
}

#[tokio::test]
async fn a_listed_statistic_is_judged_by_the_members_own_rule() {
    for statistic in METRIC_STATISTIC_VALUES {
        for operation in WRITE_OPERATIONS {
            let case = format!("{operation} / the listed statistic {statistic:?}");

            // The LOAD member documents one valid value, so every other listed statistic
            // is refused — with the sentence that rule owns, which never claims the value
            // is outside the enum. A row that writes gets its own emulator, so no row
            // depends on what another row stored.
            let emulator = with_one_stored_plan().await;
            let load_request = write_request(operation, with_load_statistic(statistic));
            if *statistic == LOAD_STATISTIC {
                emulator.ok(operation, load_request).await;
                // A write is read back once the plan has APPLIED it: a create settles and
                // an update's configuration comes into force on the lifecycle clock.
                emulator.make_all_due();
                let read = emulator
                    .ok(
                        "DescribeScalingPlans",
                        json!({ "ScalingPlanNames": [written_plan(operation)] }),
                    )
                    .await;
                let stored = &read["ScalingPlans"][0]["ScalingInstructions"][0]
                    ["CustomizedLoadMetricSpecification"]["Statistic"];
                assert_eq!(
                    stored,
                    &json!(statistic),
                    "case {case:?} / the load metric: the documented valid value must be \
                     stored as written, got {read}"
                );
            } else {
                let actual = refusal(
                    &emulator,
                    &format!("{case} / the load metric"),
                    operation,
                    &load_request,
                )
                .await;
                assert!(
                    !actual.contains("enum value set") && actual.contains(LOAD_STATISTIC),
                    "case {case:?} / the load metric: a value the enum LISTS must earn the \
                     member's own sentence naming the valid value, got {actual:?}"
                );
            }

            // The SCALING member documents no such narrowing, so every listed statistic
            // is accepted there — and the round trip: an accepted statistic is what a
            // later read reports.
            let emulator = with_one_stored_plan().await;
            let scaling_request = write_request(operation, with_scaling_statistic(statistic));
            let (status, body) = emulator.call(operation, &scaling_request).await;
            assert_eq!(
                status, 200,
                "case {case:?} / the scaling metric: expected HTTP 200, got {status} with \
                 {body}"
            );
            emulator.make_all_due();
            let stored = stored_scaling_statistics(&emulator, operation).await;
            assert!(
                stored.iter().any(|value| value == statistic),
                "case {case:?} / the scaling metric: the stored instruction must carry the \
                 statistic the write named, got {stored:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// This crate's own refusal layer
// ---------------------------------------------------------------------------

/// The member path this crate's validation names the LOAD member's statistic by.
const LOAD_MEMBER: &str = "ScalingInstructions[0].CustomizedLoadMetricSpecification.Statistic";

/// The same for the SCALING member's statistic, inside the first target tracking
/// configuration.
const SCALING_MEMBER: &str = "ScalingInstructions[0].TargetTrackingConfigurations[0]\
     .CustomizedScalingMetricSpecification.Statistic";

/// One applicable scaling instruction: the resource, capacities and target tracking
/// configuration every row below starts from.
fn instruction() -> ScalingInstruction {
    ScalingInstruction {
        service_namespace: "autoscaling".to_string(),
        resource_id: "autoScalingGroup/my-asg".to_string(),
        scalable_dimension: "autoscaling:autoScalingGroup:DesiredCapacity".to_string(),
        min_capacity: 1,
        max_capacity: 10,
        target_tracking_configurations: vec![TargetTrackingConfiguration {
            predefined_scaling_metric_specification: Some(PredefinedScalingMetricSpecification {
                predefined_scaling_metric_type: "ASGAverageCPUUtilization".to_string(),
                resource_label: None,
            }),
            target_value: 40.0,
            ..Default::default()
        }],
        ..Default::default()
    }
}

/// That instruction with a customized LOAD metric carrying `statistic`.
fn load_instruction(statistic: &str) -> Vec<ScalingInstruction> {
    let mut instruction = instruction();
    instruction.customized_load_metric_specification = Some(CustomizedLoadMetricSpecification {
        metric_name: "my-load".to_string(),
        namespace: "MyApp".to_string(),
        statistic: statistic.to_string(),
        ..Default::default()
    });
    vec![instruction]
}

/// That instruction with a customized SCALING metric carrying `statistic`.
fn scaling_instruction(statistic: &str) -> Vec<ScalingInstruction> {
    let mut instruction = instruction();
    instruction.target_tracking_configurations = vec![TargetTrackingConfiguration {
        customized_scaling_metric_specification: Some(CustomizedScalingMetricSpecification {
            metric_name: "my-metric".to_string(),
            namespace: "MyApp".to_string(),
            statistic: statistic.to_string(),
            ..Default::default()
        }),
        target_value: 40.0,
        ..Default::default()
    }];
    vec![instruction]
}

#[tokio::test]
async fn the_validation_layer_refuses_an_off_enum_statistic_the_same_way_for_both_members() {
    for (miss, statistic) in off_enum_statistics() {
        let case = format!("an off-enum statistic: {miss}");
        let load = plan_state::validate_scaling_instructions(&load_instruction(&statistic))
            .expect_err(&format!(
                "case {case:?}: an off-enum load statistic must be refused"
            ));
        let scaling = plan_state::validate_scaling_instructions(&scaling_instruction(&statistic))
            .expect_err(&format!(
                "case {case:?}: an off-enum scaling statistic must be refused"
            ));

        for (member, actual) in [("load", &load), ("scaling", &scaling)] {
            assert!(
                actual.contains(&format!("'{statistic}'")),
                "case {case:?} / the {member} metric: the refusal must name the rejected \
                 value {statistic:?}, got {actual:?}"
            );
            for accepted in METRIC_STATISTIC_VALUES {
                assert!(
                    actual.contains(accepted),
                    "case {case:?} / the {member} metric: the refusal must list the \
                     accepted value {accepted:?}, got {actual:?}"
                );
            }
        }

        // PARITY again, one layer down: the member named is all that may differ.
        assert_eq!(
            load.replace(LOAD_MEMBER, ANY_MEMBER),
            scaling.replace(SCALING_MEMBER, ANY_MEMBER),
            "case {case:?}: the two sibling members must be refused with the same \
             sentence, got {load:?} for the load metric and {scaling:?} for the scaling \
             metric"
        );
    }
}

#[tokio::test]
async fn the_validation_layer_keeps_the_narrower_rule_for_a_listed_statistic() {
    for statistic in METRIC_STATISTIC_VALUES {
        let case = format!("the listed statistic {statistic:?}");

        let load = plan_state::validate_scaling_instructions(&load_instruction(statistic));
        if *statistic == LOAD_STATISTIC {
            assert!(
                load.is_ok(),
                "case {case:?} / the load metric: the documented valid value must be \
                 accepted, got {load:?}"
            );
        } else {
            let actual = load.expect_err(&format!(
                "case {case:?} / the load metric: a statistic other than the documented \
                 one must be refused"
            ));
            assert_eq!(
                actual,
                format!("{LOAD_MEMBER}: the only valid value is {LOAD_STATISTIC}"),
                "case {case:?} / the load metric: expected the member's own sentence, got \
                 {actual:?}"
            );
        }

        let scaling = plan_state::validate_scaling_instructions(&scaling_instruction(statistic));
        assert!(
            scaling.is_ok(),
            "case {case:?} / the scaling metric: every statistic the enum lists must be \
             accepted, got {scaling:?}"
        );
    }
}
