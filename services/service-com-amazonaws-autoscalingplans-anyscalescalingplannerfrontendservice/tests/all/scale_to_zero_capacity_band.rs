// SPDX-License-Identifier: CC0-1.0

//! The per-member bound on a scaling instruction's capacity band, pinned where that band
//! configures PREDICTIVE scaling.
//!
//! `ScalingInstruction$MinCapacity` and `$MaxCapacity` both target `ResourceCapacity`, an
//! integer the model gives no `@range`
//! (`smithy_query(query="shape",shape="ResourceCapacity,ScalingInstruction")`), so the
//! service states the bound itself: a capacity counts units of a resource, so the per-member
//! bound on either end of the band is "not negative" — a minimum of ZERO is a capacity like
//! any other and is applied as given, and only a minimum BELOW zero is refused. The bound
//! does not depend on what else the instruction configures, so it is driven here over the
//! predictive-scaling configuration of a resource that scales to nothing: a plan whose
//! `PredictiveScalingMode` only forecasts scales nothing at all, which is exactly where a
//! zero minimum is the ordinary setting.
//!
//! Two things are asserted over tables rather than over one request:
//!
//! 1. the boundary of the bound, through BOTH operations that carry
//!    `ScalingInstructions` — a minimum below zero is refused with the modelled
//!    `ValidationException` (HTTP 400) and the fixed sentence for that error class, a
//!    minimum at zero and above is accepted;
//! 2. a zero-minimum predictive plan is reachable end to end — created at version 1,
//!    settling `Active`, reading its instruction back as written over either
//!    `ApplicationSource` form, and answering every modelled `ForecastDataType` for the
//!    resource it instructs.
//!
//! The harness (`Emulator`, `instruction`, `stack_source`, `points`) is the sibling
//! suites'; nothing here builds a second one.

use serde_json::{json, Value};

use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::FORECAST_DATA_TYPE_VALUES;

use crate::forecast_datapoints::points;
use crate::scaling_plan_lifecycle::{
    error_code, error_message, instruction, now_secs, stack_source, Emulator,
};

/// The least capacity either end of the band may be: a capacity counts units of a resource,
/// and a resource that scales to nothing is kept at zero of them.
const CAPACITY_FLOOR: i64 = 0;

/// AWS's fixed text for a minimum capacity below that floor. The sentence interpolates
/// nothing — the 1 it names is part of the wording, not the value the refusal is drawn at.
const MINIMUM_CAPACITY_BELOW_FLOOR: &str = "Minimum capacity cannot be less than 1";

/// AWS's fixed text for the OTHER relation the pair has to satisfy, so the rows below can
/// assert the two refusals are not interchangeable.
const MAXIMUM_CAPACITY_BELOW_MINIMUM: &str = "Maximum capacity cannot be less than minimum capacity";

/// The only scaling-plan version the model documents as valid ("This value is always 1").
const ONLY_VERSION: i64 = 1;

/// The Auto Scaling group every plan here scales. `ResourceId` "consists of a prefix
/// (autoScalingGroup) followed by the name of a specified Auto Scaling group".
const RESOURCE: &str = "autoScalingGroup/my-asg";

/// The `PredictiveScalingMode` under which no scheduled scaling action is created, so the
/// plan scales nothing at all (`smithy_query(query="shape",shape="PredictiveScalingMode")`).
const MODE_FORECAST_ONLY: &str = "ForecastOnly";

/// The two `ForecastDataType` values that describe "each scheduled scaling action", from
/// `smithy_query(query="shape",shape="ForecastDataType")` — the ones a plan that only
/// forecasts has none of.
const SCHEDULED_ACTION_DATA_TYPES: [&str; 2] =
    ["ScheduledActionMinCapacity", "ScheduledActionMaxCapacity"];

/// A `LoadMetricType` value, from
/// `smithy_query(query="shape",shape="LoadMetricType")` — what makes an instruction
/// configure predictive scaling at all.
const LOAD_METRIC_TYPE: &str = "ASGTotalCPUUtilization";

/// One instruction over the band `min..max` that configures PREDICTIVE scaling: a load
/// metric specification is what "is required when configuring predictive scaling", and the
/// mode and the dynamic-scaling switch are the members that make such a plan forecast
/// without scaling.
fn forecast_only_instruction(min: i64, max: i64) -> Value {
    let mut built = instruction(
        "autoscaling",
        RESOURCE,
        "autoscaling:autoScalingGroup:DesiredCapacity",
        min,
        max,
        "ASGAverageCPUUtilization",
        50.0,
    );
    let object = built.as_object_mut().expect("an instruction is an object");
    object.insert(
        "PredefinedLoadMetricSpecification".to_string(),
        json!({ "PredefinedLoadMetricType": LOAD_METRIC_TYPE }),
    );
    object.insert(
        "PredictiveScalingMode".to_string(),
        json!(MODE_FORECAST_ONLY),
    );
    object.insert("DisableDynamicScaling".to_string(), json!(true));
    built
}

/// An `ApplicationSource` whose `TagFilters` names one tag — the other of the two forms the
/// shape models, beside a CloudFormation stack ARN.
fn tag_source(n: usize) -> Value {
    json!({ "TagFilters": [{ "Key": "scale-to-zero", "Values": [format!("v{n}")] }] })
}

/// An emulator holding one plan over the band `min..max`, with its creation transition
/// already owed on the clock so the next call settles it.
async fn with_plan(name: &str, source: Value, min: i64, max: i64) -> Emulator {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": name,
                "ApplicationSource": source,
                "ScalingInstructions": [forecast_only_instruction(min, max)],
            }),
        )
        .await;
    emulator.make_all_due();
    emulator
}

/// The boundary of the per-member capacity bound, through both write paths.
///
/// Rows differ along what the bound generalises over: how far the minimum sits from zero
/// (far below, one below, at it, one above, far above) and the width of the band above that
/// minimum (none, one unit, wide) — and each row is driven through `CreateScalingPlan` and
/// `UpdateScalingPlan`, the two operations that carry `ScalingInstructions`, on an
/// instruction that configures predictive scaling, since the bound is about the member and
/// not about what the rest of the instruction asks for.
#[tokio::test]
async fn the_capacity_bound_admits_a_zero_minimum_and_refuses_a_negative_one() {
    // `ResourceCapacity` is a Smithy `integer`, so `i32::MIN` is the smallest value the
    // member can carry at all.
    let rows: [(&str, i64, i64); 9] = [
        ("the smallest minimum the member carries", i64::from(i32::MIN), 10),
        ("a far negative minimum", -1_000, 1_000),
        ("one unit below the floor", CAPACITY_FLOOR - 1, 1),
        ("a band at the floor", CAPACITY_FLOOR, CAPACITY_FLOOR),
        ("a band one unit wide at the floor", CAPACITY_FLOOR, 1),
        ("a wide band from the floor", CAPACITY_FLOOR, 10_000),
        ("a band one unit above the floor", 1, 1),
        ("a minimum several units up", 7, 25),
        ("a minimum far above the floor", 10_000, 20_000),
    ];

    for (label, min, max) in rows {
        let admitted = min >= CAPACITY_FLOOR;
        let instructions = json!([forecast_only_instruction(min, max)]);

        // The create path.
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": "created",
                    "ApplicationSource": tag_source(1),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        if admitted {
            assert_eq!(
                (status, body.get("ScalingPlanVersion")),
                (200, Some(&json!(ONLY_VERSION))),
                "case create/{label} ({min}..{max}): expected HTTP 200 at version \
                 {ONLY_VERSION}, got {status} with {body}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case create/{label} ({min}..{max}): expected a 400 ValidationException, \
                 got {status} with {body}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, MINIMUM_CAPACITY_BELOW_FLOOR,
                "case create/{label} ({min}..{max}): expected \
                 {MINIMUM_CAPACITY_BELOW_FLOOR:?}, got {actual:?}"
            );
            // A refused create records nothing.
            let described = emulator
                .ok(
                    "DescribeScalingPlans",
                    json!({ "ScalingPlanNames": ["created"] }),
                )
                .await;
            assert_eq!(
                described["ScalingPlans"],
                json!([]),
                "case create/{label} ({min}..{max}): a refused create stores no plan, got \
                 {described}"
            );
        }

        // The update path, against a plan already holding a band well inside the bound.
        let emulator = with_plan("updated", tag_source(2), 1, 10).await;
        let (status, body) = emulator
            .call(
                "UpdateScalingPlan",
                &json!({
                    "ScalingPlanName": "updated",
                    "ScalingPlanVersion": ONLY_VERSION,
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
        if admitted {
            assert_eq!(
                status, 200,
                "case update/{label} ({min}..{max}): expected HTTP 200, got {status} with \
                 {body}"
            );
            // The accepted band is what the plan applies, read back through the reporting
            // operation once the update is owed.
            emulator.make_all_due();
            let described = emulator
                .ok(
                    "DescribeScalingPlans",
                    json!({ "ScalingPlanNames": ["updated"] }),
                )
                .await;
            let applied = &described["ScalingPlans"][0]["ScalingInstructions"][0];
            assert_eq!(
                (applied.get("MinCapacity"), applied.get("MaxCapacity")),
                (Some(&json!(min)), Some(&json!(max))),
                "case update/{label} ({min}..{max}): the applied band is the one the \
                 update asked for, got {described}"
            );
        } else {
            assert_eq!(
                (status, error_code(&body)),
                (400, "ValidationException"),
                "case update/{label} ({min}..{max}): expected a 400 ValidationException, \
                 got {status} with {body}"
            );
            let actual = error_message(&body);
            assert_eq!(
                actual, MINIMUM_CAPACITY_BELOW_FLOOR,
                "case update/{label} ({min}..{max}): expected \
                 {MINIMUM_CAPACITY_BELOW_FLOOR:?}, got {actual:?}"
            );
            // A refused update leaves the band in force where it was.
            let described = emulator
                .ok(
                    "DescribeScalingPlans",
                    json!({ "ScalingPlanNames": ["updated"] }),
                )
                .await;
            let applied = &described["ScalingPlans"][0]["ScalingInstructions"][0];
            assert_eq!(
                (applied.get("MinCapacity"), applied.get("MaxCapacity")),
                (Some(&json!(1)), Some(&json!(10))),
                "case update/{label} ({min}..{max}): a refused update keeps the stored \
                 band, got {described}"
            );
        }
    }

    // The bound is per-member: the CROSS-member relation keeps its own sentence, including
    // where the minimum itself sits at the floor, so admitting a zero minimum does not
    // admit a band that is inverted.
    for (label, min, max) in [
        ("inverted by one unit at the floor", CAPACITY_FLOOR, CAPACITY_FLOOR - 1),
        ("inverted by one unit above the floor", 9, 8),
        ("inverted far above the floor", 9, 2),
    ] {
        let emulator = Emulator::new();
        let (status, body) = emulator
            .call(
                "CreateScalingPlan",
                &json!({
                    "ScalingPlanName": "inverted",
                    "ApplicationSource": tag_source(3),
                    "ScalingInstructions": [forecast_only_instruction(min, max)],
                }),
            )
            .await;
        assert_eq!(
            (status, error_code(&body)),
            (400, "ValidationException"),
            "case {label} ({min}..{max}): expected a 400 ValidationException, got {status} \
             with {body}"
        );
        let actual = error_message(&body);
        assert_eq!(
            actual, MAXIMUM_CAPACITY_BELOW_MINIMUM,
            "case {label} ({min}..{max}): expected {MAXIMUM_CAPACITY_BELOW_MINIMUM:?}, got \
             {actual:?}"
        );
    }
}

#[tokio::test]
async fn a_zero_minimum_predictive_plan_settles_and_reads_back_as_written() {
    // Rows differ along the width of the band above the zero minimum and along the form of
    // the `ApplicationSource` the plan is created from — the two forms the shape models
    // ("You can create one scaling plan per application source").
    let rows: [(&str, i64, i64, Value); 4] = [
        ("a zero-width band, tag filters", 0, 0, tag_source(10)),
        ("a one-unit band, tag filters", 0, 1, tag_source(11)),
        ("a wide band, tag filters", 0, 50, tag_source(12)),
        ("a one-unit band, a stack", 0, 1, stack_source(13)),
    ];

    for (label, min, max, source) in rows {
        let written = json!([forecast_only_instruction(min, max)]);
        let emulator = Emulator::new();
        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": "scale-to-zero",
                    "ApplicationSource": source,
                    "ScalingInstructions": written,
                }),
            )
            .await;
        assert_eq!(
            created.get("ScalingPlanVersion"),
            Some(&json!(ONLY_VERSION)),
            "case {label} ({min}..{max}): expected the plan at version {ONLY_VERSION}, got \
             {created}"
        );

        emulator.make_all_due();
        let described = emulator
            .ok(
                "DescribeScalingPlans",
                json!({ "ScalingPlanNames": ["scale-to-zero"] }),
            )
            .await;
        let plan = &described["ScalingPlans"][0];
        assert_eq!(
            plan.get("StatusCode"),
            Some(&json!("Active")),
            "case {label} ({min}..{max}): a created plan settles Active, got {plan}"
        );
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&written),
            "case {label} ({min}..{max}): the instruction did not round-trip, got {plan}"
        );

        // The resource the zero-minimum band was registered for is reported, so the
        // instruction is applied and not merely stored.
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({
                    "ScalingPlanName": "scale-to-zero",
                    "ScalingPlanVersion": ONLY_VERSION,
                }),
            )
            .await;
        assert_eq!(
            resources["ScalingPlanResources"]
                .as_array()
                .map(Vec::len)
                .zip(resources["ScalingPlanResources"][0]["ResourceId"].as_str()),
            Some((1, RESOURCE)),
            "case {label} ({min}..{max}): one instruction covers the one resource it \
             names, got {resources}"
        );
    }
}

#[tokio::test]
async fn a_zero_minimum_predictive_plans_forecast_surface_is_reachable() {
    let emulator = with_plan("scale-to-zero", tag_source(20), 0, 1).await;
    let issued_at = now_secs();
    let window = (
        issued_at - FORECAST_INTERVAL_SECONDS,
        issued_at + 4 * FORECAST_INTERVAL_SECONDS,
    );

    // A forecast is "calculated using historical data points from a specified CloudWatch
    // load metric", so with no history published for this resource there is nothing to
    // predict from and every data type answers 200 with no data points — the plan being
    // reachable is what makes that answer observable at all.
    for data_type in FORECAST_DATA_TYPE_VALUES {
        let got = points(&emulator, "scale-to-zero", RESOURCE, data_type, window).await;
        assert!(
            got.is_empty(),
            "case no history / {data_type}: nothing has been published for this resource, \
             so there is no forecast, got {got:?}"
        );
    }

    // With that history published, the two quantities a plan that only forecasts has are
    // reported over the requested window, while the two that describe "each scheduled
    // scaling action" stay empty: its `PredictiveScalingMode` creates none.
    emulator.publish_load_history("scale-to-zero");
    for data_type in FORECAST_DATA_TYPE_VALUES {
        let got = points(&emulator, "scale-to-zero", RESOURCE, data_type, window).await;
        let scheduled_action = SCHEDULED_ACTION_DATA_TYPES.contains(data_type);
        assert_eq!(
            got.is_empty(),
            scheduled_action,
            "case published history / {data_type}: a forecast-only plan reports the \
             forecast quantities and no scheduled-action capacity, got {got:?}"
        );
        for (at, value) in &got {
            assert!(
                *at >= window.0 && *at < window.1,
                "case published history / {data_type}: a data point sits inside the \
                 requested window {window:?}, got {at}"
            );
            assert!(
                *value >= 0.0,
                "case published history / {data_type}: a forecast quantity of a resource \
                 is not negative, got {value} at {at}"
            );
        }
    }
}
