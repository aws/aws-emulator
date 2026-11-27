// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the scaling-plan lifecycle, driven over the real wire path.
//!
//! Every call goes through [`MultiServiceRouter::dispatch`] — the same entry the
//! production router serves — as an `awsJson1_1` `POST /` with
//! `X-Amz-Target: AnyScaleScalingPlannerFrontendService.<Operation>`, so these tests
//! exercise routing, deserialization, the handler and serialization together.
//!
//! The lifecycle transitions are due at a STORED instant, never at a slept-for one, so
//! the tests drive them by moving that instant rather than by racing a wall clock:
//! `hold_pending` pushes it out (the transient window), `make_due` pulls it into the
//! past (the settled window). One test does sleep past the real span, to prove the
//! default span actually elapses on its own.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use serde_json::{json, Value};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::get_scaling_plan_resource_forecast_data::FORECAST_INTERVAL_SECONDS;
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::state::{
    self as plan_state, ComAmazonawsAutoscalingplansAnyscalescalingplannerfrontendserviceState as PlanState,
    PendingOutcome, MAX_RESULTS_MAX, MAX_RESULTS_MIN, MAX_SCALING_PLANS, SETTLE_AFTER,
};
use service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The `X-Amz-Target` prefix from the crate's OPERATIONS.md wire-protocol section.
const TARGET_PREFIX: &str = "AnyScaleScalingPlannerFrontendService.";

/// The error code of an `awsJson1_1` error body: the shape name out of `__type`.
///
/// `pub(crate)` so every suite reads the code the same way.
pub(crate) fn error_code(body: &Value) -> &str {
    body.get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', '/'])
        .next()
        .unwrap_or_default()
}

/// The `$.Message` member of an `awsJson1_1` error body.
pub(crate) fn error_message(body: &Value) -> &str {
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(Value::as_str)
        .unwrap_or_default()
}

/// Seconds in a day, for the windows and spans the suites in this target probe.
///
/// `pub(crate)` so every suite measures a day the same way.
pub(crate) const DAY_SECONDS: i64 = 86_400;

/// Hours in a day, for the published load history's daily cycle.
const HOURS_PER_DAY: i64 = 24;

/// The hour of day, in UTC, the published load history peaks at; it troughs twelve hours
/// later. The TEST's shape for a resource's load, not the service's.
const PEAK_HOUR_UTC: f64 = 12.0;

/// How far the published load of one hour sits above and below the day's mean, as a fraction
/// of it — again the test's own shape, which the forecast then reports back.
const DAILY_SWING: f64 = 0.5;

/// The hour of day, in UTC, an instant given as whole epoch seconds falls in — the index a
/// published day of load history is read by.
///
/// `pub(crate)` so a suite reading a forecast back indexes its own published day the same way.
pub(crate) fn hour_of_day(at_secs: i64) -> i64 {
    at_secs
        .div_euclid(FORECAST_INTERVAL_SECONDS)
        .rem_euclid(HOURS_PER_DAY)
}

/// Now, as the epoch seconds an `awsJson1_1` timestamp is carried as.
///
/// `pub(crate)` for the same reason: one owner for the epoch-seconds conversion.
pub(crate) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a host clock at or after the epoch")
        .as_secs() as i64
}

/// A SigV4 `Authorization` header whose credential scope names this service's signing
/// name (`autoscaling-plans`) — the routing signal the router reads. Request
/// authentication is off in tests, so the signature itself is never verified.
const SIGV4_HEADER: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEXAMPLE/20260101/us-east-1/autoscaling-plans/aws4_request, \
     SignedHeaders=host;x-amz-target, Signature=00";

/// One emulator: a single shared engine with this one service mounted on it.
///
/// `pub(crate)` so every suite in this test target drives the emulator the same way
/// rather than building a second harness beside this one.
pub(crate) struct Emulator {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Emulator {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        Self { router, engine }
    }

    /// Dispatch one operation and return `(http status, parsed body)`.
    pub(crate) async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = ::http::Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/x-amz-json-1.1")
            .header("x-amz-target", format!("{TARGET_PREFIX}{operation}"))
            .header("authorization", SIGV4_HEADER)
            .body(::bytes::Bytes::from(body.to_string()))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
        (status, parsed)
    }

    /// Dispatch one operation that must succeed, and return its body.
    pub(crate) async fn ok(&self, operation: &str, body: Value) -> Value {
        let (status, parsed) = self.call(operation, &body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected HTTP 200, got {status} with {parsed}"
        );
        parsed
    }

    /// Dispatch one operation that must fail, and return `(status, error code)`.
    pub(crate) async fn err(&self, operation: &str, body: Value) -> (u16, String) {
        let (status, parsed) = self.call(operation, &body).await;
        assert_ne!(
            status, 200,
            "case {operation} {body}: expected a failure, got HTTP 200 with {parsed}"
        );
        (status, error_code(&parsed).to_string())
    }

    /// Read or write the stored state directly, WITHOUT going through a handler — so
    /// nothing settles behind the test's back.
    fn with_state<R>(&self, body: impl FnOnce(&mut PlanState) -> R) -> R {
        let state = plan_state::service_state(&*self.engine).expect("this service's state");
        let mut guard = state.lock();
        body(&mut guard)
    }

    /// The stored `(status code, pending outcome, how long after the status started the
    /// transition is due)` of one plan.
    fn stored(&self, name: &str) -> (String, Option<PendingOutcome>, Option<Duration>) {
        self.with_state(|state| {
            let plan = state
                .scaling_plans
                .get(name)
                .unwrap_or_else(|| panic!("no stored plan named {name:?}"));
            let span = plan
                .pending
                .and_then(|pending| pending.due_at.duration_since(plan.status_start_time).ok());
            (
                plan.status_code.clone(),
                plan.pending.map(|pending| pending.outcome),
                span,
            )
        })
    }

    /// Hold a plan's pending transition open, so the transient window is deterministic.
    ///
    /// `pub(crate)` so a suite that reads a plan mid-transition holds it open the same
    /// way rather than racing the real span.
    pub(crate) fn hold_pending(&self, name: &str) {
        self.with_state(|state| {
            let plan = state
                .scaling_plans
                .get_mut(name)
                .unwrap_or_else(|| panic!("no stored plan named {name:?}"));
            let pending = plan
                .pending
                .as_mut()
                .unwrap_or_else(|| panic!("plan {name:?} has no pending transition"));
            pending.due_at = SystemTime::now() + Duration::from_secs(3600);
        });
    }

    /// Hold a plan's staged update open, so the window in which it is asked for but not
    /// applied is deterministic.
    ///
    /// `pub(crate)` so a suite that reads a plan between an update and its application
    /// holds it open the same way rather than racing the real span.
    pub(crate) fn hold_staged(&self, name: &str) {
        self.with_state(|state| {
            let plan = state
                .scaling_plans
                .get_mut(name)
                .unwrap_or_else(|| panic!("no stored plan named {name:?}"));
            let staged = plan
                .staged
                .as_mut()
                .unwrap_or_else(|| panic!("plan {name:?} has no staged update"));
            staged.due_at = SystemTime::now() + Duration::from_secs(3600);
        });
    }

    /// What was published at each hour of the day, by hour of day in UTC.
    ///
    /// `pub(crate)` so a suite that chooses its own history reads it back by the same index.
    pub(crate) const HOURS_OF_DAY: usize = HOURS_PER_DAY as usize;

    /// Publish the LOAD METRIC HISTORY a plan's forecast is computed from, one observation
    /// per hour of the day before now, carrying `by_hour_of_day[hour]` for the hour of day it
    /// falls in.
    ///
    /// `GetScalingPlanResourceForecastData` answers "predicted values, or data points, that
    /// are calculated using historical data points from a specified CloudWatch load metric",
    /// so a forecast exists only for a resource whose load metric this emulator holds history
    /// for — in production, the metric data its publisher gave CloudWatch. A suite that reads
    /// a forecast therefore publishes that history first, standing in for the publisher, into
    /// the directory the two services share, for every load metric the stored plan's
    /// instructions name. These are the TEST's numbers: the service invents none of them.
    ///
    /// `pub(crate)` so every suite that needs a forecast publishes its history the same way.
    pub(crate) fn publish_hourly_load(&self, name: &str, by_hour_of_day: &[f64]) {
        for instruction in self.stored_instructions(name) {
            self.publish_day(&instruction, by_hour_of_day);
        }
    }

    /// Publish a day of load history for `name` shaped as a daily cycle around the middle of
    /// the capacity band each instruction registers — the shape a suite that only needs SOME
    /// history to forecast from uses, so every hour of the day is covered, the series varies
    /// over the day, and two resources whose bands differ carry different load.
    pub(crate) fn publish_load_history(&self, name: &str) {
        for instruction in self.stored_instructions(name) {
            let middle_of_band =
                (f64::from(instruction.min_capacity) + f64::from(instruction.max_capacity)) / 2.0;
            let per_unit = instruction
                .target_tracking_configurations
                .iter()
                .map(|configuration| configuration.target_value)
                .filter(|target| *target > 0.0)
                .fold(f64::INFINITY, f64::min);
            let per_unit = if per_unit.is_finite() { per_unit } else { 1.0 };
            let day: Vec<f64> = (0..HOURS_PER_DAY)
                .map(|hour| {
                    let phase = (hour as f64 - PEAK_HOUR_UTC) / HOURS_PER_DAY as f64
                        * std::f64::consts::TAU;
                    middle_of_band * per_unit * (1.0 + DAILY_SWING * phase.cos())
                })
                .collect();
            self.publish_day(&instruction, &day);
        }
    }

    /// The scaling instructions the plan `name` holds.
    fn stored_instructions(
        &self,
        name: &str,
    ) -> Vec<service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::ScalingInstruction>
    {
        self.with_state(|state| {
            state
                .scaling_plans
                .get(name)
                .map(|plan| plan.scaling_instructions.clone())
        })
        .unwrap_or_else(|| panic!("no stored plan named {name:?}"))
    }

    /// Publish one day of history for the load metric ONE instruction names, carrying
    /// `by_hour_of_day[hour]` at each hour of the day before now. An instruction that names
    /// no load metric — one configuring no predictive scaling — has nothing to publish.
    fn publish_day(
        &self,
        instruction: &service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::ops::types::ScalingInstruction,
        by_hour_of_day: &[f64],
    ) {
        assert_eq!(
            by_hour_of_day.len(),
            Self::HOURS_OF_DAY,
            "case publishing load history: one value per hour of the day is needed, got {}",
            by_hour_of_day.len()
        );
        let Some(series) = plan_state::load_metric_history::load_metric_series(instruction) else {
            return;
        };
        let directory = service_common::published_metric_data::published_metric_data(&*self.engine)
            .expect("the shared published-metric directory");
        let current_interval =
            now_secs().div_euclid(FORECAST_INTERVAL_SECONDS) * FORECAST_INTERVAL_SECONDS;
        for hours_ago in 0..HOURS_PER_DAY {
            let at = current_interval - hours_ago * FORECAST_INTERVAL_SECONDS;
            let observed = by_hour_of_day[hour_of_day(at) as usize];
            directory.lock().publish(series.clone(), at, observed);
        }
    }

    /// Make every plan's pending transition due, so the next call settles it.
    ///
    /// Everything the clock owes a plan is moved into the past: its status transition AND
    /// the configuration a staged update asked for, so a test that drives a mutation to
    /// its settled state sees the whole of it rather than the half the status carries.
    pub(crate) fn make_all_due(&self) {
        self.with_state(|state| {
            let past = SystemTime::now() - Duration::from_secs(1);
            for plan in state.scaling_plans.values_mut() {
                if let Some(pending) = plan.pending.as_mut() {
                    pending.due_at = past;
                }
                if let Some(staged) = plan.staged.as_mut() {
                    staged.due_at = past;
                }
            }
        });
    }
}

/// One `ScalingInstruction` as JSON.
///
/// `namespace` / `dimension` / `metric` are drawn from the model's own value lists
/// (`smithy_query(query="shape",shape="ServiceNamespace,ScalableDimension,ScalingMetricType")`,
/// mirrored in `src/ops/types/shared_types.rs` as `SERVICE_NAMESPACE_VALUES`,
/// `SCALABLE_DIMENSION_VALUES` and `SCALING_METRIC_TYPE_VALUES`).
pub(crate) fn instruction(
    namespace: &str,
    resource_id: &str,
    dimension: &str,
    min: i64,
    max: i64,
    metric: &str,
    target: f64,
) -> Value {
    json!({
        "ServiceNamespace": namespace,
        "ResourceId": resource_id,
        "ScalableDimension": dimension,
        "MinCapacity": min,
        "MaxCapacity": max,
        "TargetTrackingConfigurations": [{
            "PredefinedScalingMetricSpecification": { "PredefinedScalingMetricType": metric },
            "TargetValue": target
        }]
    })
}

/// The one-instruction plan body most tests start from.
pub(crate) fn simple_instructions() -> Vec<Value> {
    vec![instruction(
        "autoscaling",
        "autoScalingGroup/my-asg",
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    )]
}

/// The `(ServiceNamespace, ResourceId, ScalableDimension)` triple one scalable resource
/// is addressed by — "Each scaling instruction applies to one resource"
/// (`smithy_query(query="shape",shape="ScalingInstruction")`).
///
/// `pub(crate)` so every suite reads that identity off a `ScalingInstruction`, or off
/// the `ScalingPlanResource` derived from it, the same way.
pub(crate) type ResourceIdentity = (String, String, String);

/// The [`ResourceIdentity`] a scaling instruction — or the scaling plan resource derived
/// from it — names, read off the three members both shapes carry.
pub(crate) fn resource_identity(value: &Value) -> ResourceIdentity {
    let member = |name: &str| value[name].as_str().unwrap_or_default().to_string();
    (
        member("ServiceNamespace"),
        member("ResourceId"),
        member("ScalableDimension"),
    )
}

/// A CloudFormation-stack application source, distinct per `n`.
pub(crate) fn stack_source(n: usize) -> Value {
    json!({
        "CloudFormationStackARN":
            format!("arn:aws:cloudformation:us-east-1:000000000000:stack/s{n}/abc")
    })
}

/// A plan the tests create: name, application source, instruction list.
struct PlanCase {
    label: &'static str,
    name: &'static str,
    source: Value,
    instructions: Vec<Value>,
}

/// The table the round-trip test spans. Rows differ along the things the rule
/// generalises over: the FORM of the application source (a CloudFormation stack vs a
/// tag-filter set), the NUMBER of tag filters, the NUMBER of scaling instructions, the
/// service namespace / scalable dimension each instruction names, the capacity values,
/// and which optional instruction members are present.
fn plan_cases() -> Vec<PlanCase> {
    vec![
        PlanCase {
            label: "cloudformation stack, one autoscaling instruction",
            name: "cfn-single",
            source: stack_source(1),
            instructions: simple_instructions(),
        },
        PlanCase {
            label: "one tag filter, two instructions across two namespaces",
            name: "tags-one-filter",
            source: json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] }),
            instructions: vec![
                instruction(
                    "ecs",
                    "service/default/sample-webapp",
                    "ecs:service:DesiredCount",
                    2,
                    20,
                    "ECSServiceAverageCPUUtilization",
                    55.5,
                ),
                instruction(
                    "dynamodb",
                    "table/my-table",
                    "dynamodb:table:WriteCapacityUnits",
                    5,
                    500,
                    "DynamoDBWriteCapacityUtilization",
                    70.0,
                ),
            ],
        },
        PlanCase {
            label: "three tag filters, three instructions",
            name: "tags-three-filters",
            source: json!({
                "TagFilters": [
                    { "Key": "env", "Values": ["prod", "staging"] },
                    { "Key": "team", "Values": ["platform"] },
                    { "Key": "tier", "Values": ["web", "api", "worker"] }
                ]
            }),
            instructions: vec![
                instruction(
                    "dynamodb",
                    "table/my-table/index/my-index",
                    "dynamodb:index:ReadCapacityUnits",
                    1,
                    100,
                    "DynamoDBReadCapacityUtilization",
                    65.0,
                ),
                instruction(
                    "ec2",
                    "spot-fleet-request/sfr-73fbd2ce-aa30-494c-8788-1cee4EXAMPLE",
                    "ec2:spot-fleet-request:TargetCapacity",
                    3,
                    30,
                    "EC2SpotFleetRequestAverageCPUUtilization",
                    45.0,
                ),
                instruction(
                    "rds",
                    "cluster:my-db-cluster",
                    "rds:cluster:ReadReplicaCount",
                    1,
                    15,
                    "RDSReaderAverageCPUUtilization",
                    50.0,
                ),
            ],
        },
        PlanCase {
            label: "stack source with every optional instruction member set",
            name: "cfn-with-optionals",
            source: stack_source(2),
            instructions: vec![json!({
                "ServiceNamespace": "autoscaling",
                "ResourceId": "autoScalingGroup/predictive-asg",
                "ScalableDimension": "autoscaling:autoScalingGroup:DesiredCapacity",
                "MinCapacity": 4,
                "MaxCapacity": 40,
                "TargetTrackingConfigurations": [{
                    "PredefinedScalingMetricSpecification": {
                        "PredefinedScalingMetricType": "ASGAverageNetworkIn"
                    },
                    "TargetValue": 1000.0,
                    "DisableScaleIn": true,
                    "ScaleOutCooldown": 60,
                    "ScaleInCooldown": 300,
                    "EstimatedInstanceWarmup": 120
                }],
                "PredefinedLoadMetricSpecification": {
                    "PredefinedLoadMetricType": "ASGTotalNetworkIn"
                },
                "ScheduledActionBufferTime": 600,
                "PredictiveScalingMaxCapacityBehavior": "SetMaxCapacityToForecastCapacity",
                "PredictiveScalingMode": "ForecastOnly",
                "ScalingPolicyUpdateBehavior": "ReplaceExternalPolicies",
                "DisableDynamicScaling": false
            })],
        },
    ]
}

#[tokio::test]
async fn a_created_plan_is_read_back_exactly_as_created() {
    for case in plan_cases() {
        let emulator = Emulator::new();
        let label = case.label;

        let created = emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": case.name,
                    "ApplicationSource": case.source,
                    "ScalingInstructions": case.instructions,
                }),
            )
            .await;
        assert_eq!(
            created.get("ScalingPlanVersion"),
            Some(&json!(1)),
            "case {label:?}: CreateScalingPlan must answer version 1, got {created:?}"
        );

        emulator.make_all_due();

        let described = emulator
            .ok(
                "DescribeScalingPlans",
                json!({ "ScalingPlanNames": [case.name] }),
            )
            .await;
        let plans = described
            .get("ScalingPlans")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no ScalingPlans in {described:?}"));
        assert_eq!(
            plans.len(),
            1,
            "case {label:?}: expected exactly one plan, got {plans:?}"
        );
        let plan = &plans[0];
        assert_eq!(
            plan.get("ScalingPlanName"),
            Some(&json!(case.name)),
            "case {label:?}: wrong ScalingPlanName in {plan:?}"
        );
        assert_eq!(
            plan.get("ScalingPlanVersion"),
            Some(&json!(1)),
            "case {label:?}: wrong ScalingPlanVersion in {plan:?}"
        );
        assert_eq!(
            plan.get("ApplicationSource"),
            Some(&case.source),
            "case {label:?}: ApplicationSource did not round-trip, got {plan:?}"
        );
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&Value::Array(case.instructions.clone())),
            "case {label:?}: ScalingInstructions did not round-trip, got {plan:?}"
        );
        assert_eq!(
            plan.get("StatusCode"),
            Some(&json!("Active")),
            "case {label:?}: expected StatusCode Active once the plan settled, got {plan:?}"
        );
        for member in ["CreationTime", "StatusStartTime"] {
            let seconds = plan.get(member).and_then(Value::as_f64);
            assert!(
                seconds.is_some_and(|seconds| seconds > 0.0),
                "case {label:?}: {member} must be a populated epoch timestamp, got {plan:?}"
            );
        }

        // One ScalingPlanResource per stored instruction, addressed by the identity
        // triple that instruction names its one scalable resource by.
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": case.name, "ScalingPlanVersion": 1 }),
            )
            .await;
        let resources = resources
            .get("ScalingPlanResources")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {label:?}: no ScalingPlanResources in {resources:?}"))
            .clone();
        assert_eq!(
            resources.len(),
            case.instructions.len(),
            "case {label:?}: expected one resource per instruction, got {resources:?}"
        );
        // Every instruction's resource is reported, ordered by the identity that
        // addresses it — the order a paged walk resumes in, and not the order the
        // instructions happen to be written in.
        let reported: Vec<ResourceIdentity> = resources.iter().map(resource_identity).collect();
        let mut expected: Vec<ResourceIdentity> =
            case.instructions.iter().map(resource_identity).collect();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {label:?}: each instruction's resource must be reported once, in the \
             order of the identity that addresses it, got {resources:?}"
        );
        for (index, resource) in resources.iter().enumerate() {
            assert_eq!(
                resource.get("ScalingPlanName"),
                Some(&json!(case.name)),
                "case {label:?} resource {index}: wrong ScalingPlanName in {resource:?}"
            );
            assert_eq!(
                resource.get("ScalingPlanVersion"),
                Some(&json!(1)),
                "case {label:?} resource {index}: wrong ScalingPlanVersion in {resource:?}"
            );
            assert_eq!(
                resource.get("ScalingStatusCode"),
                Some(&json!("Active")),
                "case {label:?} resource {index}: expected ScalingStatusCode Active, \
                 got {resource:?}"
            );
        }
    }
}

#[tokio::test]
async fn every_mutating_call_leaves_a_transition_the_clock_owes() {
    // The due instant is read out of state, not raced against: a mutating call must
    // store a NON-ZERO span, so a caller acting straight afterwards still sees the
    // transient status.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;

    for (label, status, outcome) in [
        ("after CreateScalingPlan", "CreationInProgress", PendingOutcome::Active),
        ("after UpdateScalingPlan", "UpdateInProgress", PendingOutcome::Active),
        ("after DeleteScalingPlan", "DeletionInProgress", PendingOutcome::Removed),
    ] {
        let (stored_status, stored_outcome, span) = emulator.stored("my-plan");
        assert_eq!(
            stored_status, status,
            "case {label}: wrong stored StatusCode"
        );
        assert_eq!(
            stored_outcome,
            Some(outcome),
            "case {label}: wrong pending outcome"
        );
        let span = span.unwrap_or_else(|| panic!("case {label}: no pending transition stored"));
        assert!(
            (Duration::from_millis(250)..=Duration::from_millis(750)).contains(&span),
            "case {label}: the owed transition must be due 250-750 ms out, got {span:?}"
        );

        // Move on to the next mutating call: settle this transition first, because a
        // plan with one pending cannot be updated or deleted.
        emulator.make_all_due();
        match status {
            "CreationInProgress" => {
                emulator
                    .ok(
                        "UpdateScalingPlan",
                        json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
                    )
                    .await;
            }
            "UpdateInProgress" => {
                emulator
                    .ok(
                        "DeleteScalingPlan",
                        json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
                    )
                    .await;
            }
            _ => {}
        }
    }
}

#[tokio::test]
async fn a_plan_with_a_pending_transition_is_reported_but_not_mutated() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;

    // Each row: the transient status, the ScalingStatusCode its resources report, and
    // the call that puts the plan into it.
    for (label, status, scaling_status) in [
        ("being created", "CreationInProgress", "Inactive"),
        ("being updated", "UpdateInProgress", "PartiallyActive"),
        ("being deleted", "DeletionInProgress", "PartiallyActive"),
    ] {
        emulator.hold_pending("my-plan");

        let described = emulator.ok("DescribeScalingPlans", json!({})).await;
        assert_eq!(
            described["ScalingPlans"][0]["StatusCode"],
            json!(status),
            "case {label}: wrong reported StatusCode in {described:?}"
        );
        let resources = emulator
            .ok(
                "DescribeScalingPlanResources",
                json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
            )
            .await;
        assert_eq!(
            resources["ScalingPlanResources"][0]["ScalingStatusCode"],
            json!(scaling_status),
            "case {label}: wrong reported ScalingStatusCode in {resources:?}"
        );

        // "You cannot update a scaling plan if it is in the process of being created,
        // updated, or deleted." — refused as the client error it is (the status lives in
        // `state::wire_refusals`; `crate::concurrent_update_refusal` tables the rule).
        for operation in ["UpdateScalingPlan", "DeleteScalingPlan"] {
            let (code_status, code) = emulator
                .err(
                    operation,
                    json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
                )
                .await;
            assert_eq!(
                (code_status, code.as_str()),
                (
                    plan_state::wire_refusals::CONCURRENT_UPDATE_STATUS,
                    "ConcurrentUpdateException"
                ),
                "case {label}/{operation}: a pending transition must refuse the mutation"
            );
        }

        emulator.make_all_due();
        match status {
            "CreationInProgress" => {
                emulator
                    .ok(
                        "UpdateScalingPlan",
                        json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
                    )
                    .await;
            }
            "UpdateInProgress" => {
                emulator
                    .ok(
                        "DeleteScalingPlan",
                        json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
                    )
                    .await;
            }
            _ => {}
        }
    }

    // The deletion was made due above: the plan is gone, and stays gone.
    let gone = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        gone.get("ScalingPlans"),
        Some(&json!([])),
        "a deleted plan must no longer be listed, got {gone:?}"
    );
    for operation in ["DeleteScalingPlan", "UpdateScalingPlan"] {
        let (status, code) = emulator
            .err(
                operation,
                json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ObjectNotFoundException"),
            "case {operation}: a plan that is gone must be ObjectNotFoundException"
        );
    }
}

#[tokio::test]
async fn the_owed_transition_settles_on_its_own_wall_clock() {
    // The one test that waits for the real span rather than moving the due instant:
    // nothing but the clock turns a created plan Active.
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "my-plan",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;

    tokio::time::sleep(SETTLE_AFTER + Duration::from_millis(400)).await;

    let described = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        described["ScalingPlans"][0]["StatusCode"],
        json!("Active"),
        "waiting past the settle span must leave the plan Active, got {described:?}"
    );
}

#[tokio::test]
async fn an_update_replaces_only_what_the_request_carries() {
    let emulator = Emulator::new();
    let source = stack_source(1);
    let other_source = json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] });
    let original = instruction(
        "autoscaling",
        "autoScalingGroup/my-asg",
        "autoscaling:autoScalingGroup:DesiredCapacity",
        1,
        10,
        "ASGAverageCPUUtilization",
        40.0,
    );
    let updated = instruction(
        "autoscaling",
        "autoScalingGroup/my-asg",
        "autoscaling:autoScalingGroup:DesiredCapacity",
        3,
        30,
        "ASGAverageCPUUtilization",
        75.0,
    );
    // What the update above leaves in force: an update puts the instruction set it
    // carries in force, entry for entry and member for member, so both the new capacity
    // bounds and the new target value apply; the whole rule is tabulated in
    // [`crate::update_replaces_the_instruction_set`] and
    // [`crate::update_keeps_an_applied_policys_target_value`].
    let in_force = updated.clone();

    // Rows: which members the update carries, and what each one leaves behind.
    for (label, request_extras, expect_source, expect_instructions) in [
        (
            "instructions only",
            json!({ "ScalingInstructions": [updated.clone()] }),
            source.clone(),
            json!([in_force.clone()]),
        ),
        (
            "application source only",
            json!({ "ApplicationSource": other_source }),
            other_source.clone(),
            json!([in_force.clone()]),
        ),
        (
            "neither member",
            json!({}),
            other_source.clone(),
            json!([in_force.clone()]),
        ),
        (
            "both members",
            json!({ "ApplicationSource": source, "ScalingInstructions": [original] }),
            source.clone(),
            json!([original]),
        ),
    ] {
        if label == "instructions only" {
            emulator
                .ok(
                    "CreateScalingPlan",
                    json!({
                        "ScalingPlanName": "my-plan",
                        "ApplicationSource": source,
                        "ScalingInstructions": [original],
                    }),
                )
                .await;
            emulator.make_all_due();
        }

        let mut request = json!({ "ScalingPlanName": "my-plan", "ScalingPlanVersion": 1 });
        for (member, value) in request_extras.as_object().expect("an object") {
            request[member] = value.clone();
        }
        emulator.ok("UpdateScalingPlan", request).await;
        emulator.make_all_due();

        let described = emulator
            .ok(
                "DescribeScalingPlans",
                json!({ "ScalingPlanNames": ["my-plan"] }),
            )
            .await;
        let plan = &described["ScalingPlans"][0];
        assert_eq!(
            plan.get("ScalingInstructions"),
            Some(&expect_instructions),
            "case {label}: wrong ScalingInstructions after the update, got {plan:?}"
        );
        assert_eq!(
            plan.get("ApplicationSource"),
            Some(&expect_source),
            "case {label}: wrong ApplicationSource after the update, got {plan:?}"
        );
        assert_eq!(
            plan.get("StatusCode"),
            Some(&json!("Active")),
            "case {label}: the plan must be Active again once the update settled, got {plan:?}"
        );
    }
}

#[tokio::test]
async fn an_unknown_plan_or_version_answers_each_operations_own_envelope() {
    let emulator = Emulator::new();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "known",
                "ApplicationSource": stack_source(1),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    emulator.make_all_due();

    // `DescribeScalingPlanResources` identifies one plan by its two required members but
    // models no not-found shape, so a coordinate it holds no plan at — an unknown NAME, or
    // a version no stored plan carries — reports the empty set of scalable resources; see
    // `tests/all/absent_plan_reads.rs` and
    // `tests/all/plan_version_selects_a_plan.rs` for the full tables.
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": "missing", "ScalingPlanVersion": 1 }),
        )
        .await;
    assert_eq!(
        body["ScalingPlanResources"],
        json!([]),
        "case unknown name: expected an empty ScalingPlanResources list for a plan that is \
         not held, got {body:?}"
    );
    let body = emulator
        .ok(
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": "known", "ScalingPlanVersion": 2 }),
        )
        .await;
    assert_eq!(
        body["ScalingPlanResources"],
        json!([]),
        "case known name, a version it does not carry: expected an empty \
         ScalingPlanResources list, got {body:?}"
    );

    // The same two inputs on the mutating operations, which DO model a not-found error.
    for operation in ["UpdateScalingPlan", "DeleteScalingPlan"] {
        for (label, name, version) in [
            ("unknown name", "missing", 1),
            ("known name, unmodelled version", "known", 2),
        ] {
            let (status, code) = emulator
                .err(
                    operation,
                    json!({ "ScalingPlanName": name, "ScalingPlanVersion": version }),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ObjectNotFoundException"),
                "case {operation}/{label}: expected a 400 ObjectNotFoundException"
            );
        }
    }
}

#[tokio::test]
async fn a_name_or_an_application_source_is_claimed_by_exactly_one_plan() {
    let emulator = Emulator::new();
    let stack = stack_source(1);
    let tagged = json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] });
    for (name, source) in [("first", &stack), ("tagged", &tagged)] {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();

    // Rows: the same name again; a new name over an already-covered stack; a new name
    // over an already-covered tag set; and the three application-source shapes that
    // name no single application.
    for (label, name, source) in [
        ("duplicate name", "first", stack.clone()),
        ("duplicate stack source", "second", stack.clone()),
        ("duplicate tag source", "third", tagged.clone()),
        ("empty application source", "fourth", json!({})),
        ("empty tag filter list", "fifth", json!({ "TagFilters": [] })),
        (
            "both a stack and tag filters",
            "sixth",
            json!({
                "CloudFormationStackARN":
                    "arn:aws:cloudformation:us-east-1:000000000000:stack/other/abc",
                "TagFilters": [{ "Key": "env", "Values": ["dev"] }]
            }),
        ),
    ] {
        let (status, code) = emulator
            .err(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label}: expected a 400 ValidationException"
        );
    }

    // An update may not name a contradictory application source either.
    let (status, code) = emulator
        .err(
            "UpdateScalingPlan",
            json!({
                "ScalingPlanName": "first",
                "ScalingPlanVersion": 1,
                "ApplicationSource": { "TagFilters": [] }
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "an update naming no application must be a 400 ValidationException"
    );
}

#[tokio::test]
async fn the_plan_store_is_bounded_by_the_per_account_quota() {
    // The quota is the published bound of this store: exactly `MAX_SCALING_PLANS`
    // creates succeed and the one past it is refused, so a caller cannot grow the
    // store without limit.
    let emulator = Emulator::new();
    for n in 0..MAX_SCALING_PLANS {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": format!("plan-{n}"),
                    "ApplicationSource": stack_source(n),
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    let (status, code) = emulator
        .err(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "one-too-many",
                "ApplicationSource": stack_source(MAX_SCALING_PLANS),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "LimitExceededException"),
        "the create past the quota must be a 400 LimitExceededException"
    );

    // Deleting one frees exactly one slot.
    emulator.make_all_due();
    emulator
        .ok(
            "DeleteScalingPlan",
            json!({ "ScalingPlanName": "plan-0", "ScalingPlanVersion": 1 }),
        )
        .await;
    emulator.make_all_due();
    emulator
        .ok(
            "CreateScalingPlan",
            json!({
                "ScalingPlanName": "one-too-many",
                "ApplicationSource": stack_source(MAX_SCALING_PLANS),
                "ScalingInstructions": simple_instructions(),
            }),
        )
        .await;
}

#[tokio::test]
async fn both_describe_operations_page_and_refuse_a_token_they_did_not_mint() {
    let emulator = Emulator::new();
    // Five plans, each with as many instructions as its index, so one fixture pages
    // over plans AND over the instructions of a single plan.
    let names: Vec<String> = (1..=5).map(|n| format!("plan-{n}")).collect();
    for (index, name) in names.iter().enumerate() {
        let instructions: Vec<Value> = (0..=index)
            .map(|i| {
                instruction(
                    "autoscaling",
                    &format!("autoScalingGroup/asg-{index}-{i}"),
                    "autoscaling:autoScalingGroup:DesiredCapacity",
                    1,
                    10,
                    "ASGAverageCPUUtilization",
                    40.0,
                )
            })
            .collect();
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": stack_source(index),
                    "ScalingInstructions": instructions,
                }),
            )
            .await;
    }
    emulator.make_all_due();

    // Page over the plans two at a time and check the pages reassemble the whole set
    // in the service's stable ordering.
    let mut seen: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..10 {
        let mut request = json!({ "MaxResults": 2 });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator.ok("DescribeScalingPlans", request).await;
        let plans = body["ScalingPlans"].as_array().cloned().unwrap_or_default();
        assert!(
            plans.len() <= 2,
            "page {page}: MaxResults=2 must cap the page, got {plans:?}"
        );
        seen.extend(plans.iter().map(|plan| {
            plan["ScalingPlanName"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        }));
        token = body["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        seen, names,
        "paging must reassemble every plan exactly once, in order"
    );

    // Page over the five instructions of `plan-5` one at a time.
    let mut resource_ids: Vec<String> = Vec::new();
    let mut token: Option<String> = None;
    for page in 0..10 {
        let mut request = json!({
            "ScalingPlanName": "plan-5",
            "ScalingPlanVersion": 1,
            "MaxResults": MAX_RESULTS_MIN,
        });
        if let Some(token) = token.as_deref() {
            request["NextToken"] = json!(token);
        }
        let body = emulator.ok("DescribeScalingPlanResources", request).await;
        let resources = body["ScalingPlanResources"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(
            resources.len() <= 1,
            "page {page}: MaxResults=1 must cap the page, got {resources:?}"
        );
        resource_ids.extend(
            resources
                .iter()
                .map(|r| r["ResourceId"].as_str().unwrap_or_default().to_string()),
        );
        token = body["NextToken"].as_str().map(ToString::to_string);
        if token.is_none() {
            break;
        }
    }
    assert_eq!(
        resource_ids,
        (0..5)
            .map(|i| format!("autoScalingGroup/asg-4-{i}"))
            .collect::<Vec<_>>(),
        "paging must reassemble every instruction of the plan exactly once, in order"
    );

    // The documented MaxResults range is 1..=50: each published bound is accepted and
    // one past each is refused.
    for (operation, base) in [
        ("DescribeScalingPlans", json!({})),
        (
            "DescribeScalingPlanResources",
            json!({ "ScalingPlanName": "plan-1", "ScalingPlanVersion": 1 }),
        ),
    ] {
        for accepted in [MAX_RESULTS_MIN, MAX_RESULTS_MAX] {
            let mut request = base.clone();
            request["MaxResults"] = json!(accepted);
            emulator.ok(operation, request).await;
        }
        for refused in [MAX_RESULTS_MIN - 1, MAX_RESULTS_MAX + 1] {
            let mut request = base.clone();
            request["MaxResults"] = json!(refused);
            let (status, code) = emulator.err(operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {operation} MaxResults={refused}: expected a 400 ValidationException"
            );
        }

        // A token neither operation minted does not decode, whatever its shape.
        for token in ["garbage", "", "0", "7a7a7a", "plans:plan-1"] {
            let mut request = base.clone();
            request["NextToken"] = json!(token);
            let (status, code) = emulator.err(operation, request).await;
            assert_eq!(
                (status, code.as_str()),
                (400, "InvalidNextTokenException"),
                "case {operation} NextToken={token:?}: expected a 400 InvalidNextTokenException"
            );
        }
    }
}

#[tokio::test]
async fn describe_scaling_plans_honours_its_documented_filter_rules() {
    let emulator = Emulator::new();
    let stack = stack_source(1);
    let tagged = json!({ "TagFilters": [{ "Key": "env", "Values": ["prod"] }] });
    for (name, source) in [("by-stack", &stack), ("by-tags", &tagged)] {
        emulator
            .ok(
                "CreateScalingPlan",
                json!({
                    "ScalingPlanName": name,
                    "ApplicationSource": source,
                    "ScalingInstructions": simple_instructions(),
                }),
            )
            .await;
    }
    emulator.make_all_due();

    // Filtering by application source selects exactly the plan created over it.
    let by_source = emulator
        .ok(
            "DescribeScalingPlans",
            json!({ "ApplicationSources": [tagged.clone()] }),
        )
        .await;
    assert_eq!(
        by_source["ScalingPlans"].as_array().map(|plans| plans
            .iter()
            .map(|plan| plan["ScalingPlanName"].clone())
            .collect::<Vec<_>>()),
        Some(vec![json!("by-tags")]),
        "ApplicationSources must select only the plan over that source, got {by_source:?}"
    );

    // With no filter at all, every plan is returned.
    let all = emulator.ok("DescribeScalingPlans", json!({})).await;
    assert_eq!(
        all["ScalingPlans"].as_array().map(Vec::len),
        Some(2),
        "an unfiltered DescribeScalingPlans returns every plan, got {all:?}"
    );

    // The documented combination rules, each refused with ValidationException.
    let eleven: Vec<String> = (0..11).map(|n| format!("name-{n}")).collect();
    let eleven_sources: Vec<Value> = (0..11).map(stack_source).collect();
    for (label, request) in [
        (
            "names and sources together",
            json!({ "ScalingPlanNames": ["by-stack"], "ApplicationSources": [stack] }),
        ),
        ("version without a name", json!({ "ScalingPlanVersion": 1 })),
        (
            "sources together with a version",
            json!({ "ApplicationSources": [tagged.clone()], "ScalingPlanVersion": 1 }),
        ),
        ("more than ten names", json!({ "ScalingPlanNames": eleven })),
        // Both selector lists are documented "(up to 10)", so the source list is bounded
        // exactly as the name list is (the whole rule lives in
        // `crate::describe_scaling_plans_selector_bounds`).
        (
            "more than ten sources",
            json!({ "ApplicationSources": eleven_sources }),
        ),
        // An ApplicationSources entry that identifies no application must be refused,
        // never dropped: dropping it would widen the filter to "every plan" and
        // answer a narrow question with an unfiltered 200.
        (
            "an application source naming nothing",
            json!({ "ApplicationSources": [{}] }),
        ),
        (
            "an application source with an empty tag filter list",
            json!({ "ApplicationSources": [{ "TagFilters": [] }] }),
        ),
        (
            "an application source naming both a stack and tag filters",
            json!({ "ApplicationSources": [{
                "CloudFormationStackARN":
                    "arn:aws:cloudformation:us-east-1:000000000000:stack/s/1",
                "TagFilters": [{ "Key": "env", "Values": ["prod"] }]
            }] }),
        ),
        (
            "one usable source beside one that names nothing",
            json!({ "ApplicationSources": [tagged, {}] }),
        ),
    ] {
        let (status, code) = emulator.err("DescribeScalingPlans", request).await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label}: expected a 400 ValidationException"
        );
    }
}

