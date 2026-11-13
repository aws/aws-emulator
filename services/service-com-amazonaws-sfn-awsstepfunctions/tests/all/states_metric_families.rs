// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the `AWS/States` metric FAMILIES this service publishes beside
//! the per-state-machine execution counters, driven over each service's own wire
//! protocol through the production `MultiServiceRouter` with Step Functions and
//! CloudWatch mounted on ONE engine — the shape `core/router/src/lib.rs` mounts them in
//! — and read back through CloudWatch's own `GetMetricStatistics`, so what is asserted
//! is what a client charting or alarming on these series sees.
//!
//! The rules under test, each series dimensioned on WHAT it is about:
//!
//! 1. the work a `Task` state hands to an ACTIVITY's workers is counted per activity
//!    ARN: scheduled when the state queues it, started when a poll takes it, and closed
//!    under the series of the outcome the worker — or the state's own bound — reported,
//!    beside the spans it stayed queued and ran for;
//! 2. every call this service SERVES consumes one unit of the capacity of the API it
//!    named, so the sum over a window of that API's series is the number of calls made
//!    in it, whatever each call then answered;
//! 3. how many versions and how many aliases a state machine HOLDS is reported per state
//!    machine on every accepted mutation of them, so a publish the service refused — and
//!    one it answered idempotently — leaves the count where the last accepted mutation
//!    put it;
//! 4. an execution started through a qualified ARN is counted under the alias that
//!    routed it and the version it ran as well as under its state machine, and a redrive
//!    is counted once per accepted redrive, with the re-run's own outcome counted in the
//!    redriven series of that outcome.
//!
//! The rows differ along what each rule generalises over: how a worker's task closes and
//! how many executions one activity takes (1), the operation called, how many times, and
//! whether it was served a success or a refusal (2), which mutation moved a count and
//! which left it (3), and which reference an execution was started through (4).

use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_cloudwatch_graniteserviceversion20100801::state as cloudwatch;
use service_com_amazonaws_sfn_awsstepfunctions::state::observability;
use service_com_amazonaws_sfn_awsstepfunctions::EmulatedService;

use crate::state_machine_lifecycle::{string, ACCOUNT, REGION, ROLE_ARN};

/// How long one poll of a status the clock owes sleeps between reads.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// How long a row waits for an execution to leave `RUNNING`: generous, so a loaded host
/// cannot fail a row whose own declared bound is the only tight thing in it.
const PATIENCE: Duration = Duration::from_secs(60);

/// The window every series below is read over, in seconds either side of now.
const WINDOW_SECS: f64 = 600.0;

/// The period the datapoints of that window are folded into: 20 periods, well inside the
/// 1440 datapoints one `GetMetricStatistics` request serves.
const PERIOD_SECS: i32 = 60;

/// A definition that succeeds at once, for the executions whose own work is not what a
/// row is about.
const HELLO: &str = r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"hi"},"End":true}}}"#;

/// A second such definition, so an update really changes what the state machine holds
/// and the version published from it is a new one.
const HELLO_AGAIN: &str = r#"{"StartAt":"Hello","States":{"Hello":{"Type":"Pass","Result":{"msg":"again"},"End":true}}}"#;

/// A definition that fails at once, so a row has an execution AWS reports as redrivable.
const FAILING: &str =
    r#"{"StartAt":"Bang","States":{"Bang":{"Type":"Fail","Error":"Boom","Cause":"on purpose"}}}"#;

/// The name the rows' workers poll under.
const WORKER: &str = "metered-worker";

/// Step Functions and CloudWatch over ONE engine.
///
/// `pub(crate)`, not private: `tests/all.rs` is this binary's crate root, so a suite that
/// reads the same `AWS/States` series — the activity-timeout hand-off
/// (`crate::activity_timeout_late_answer`) — drives them through this harness rather
/// than carrying a second copy of it.
pub(crate) struct Metered {
    router: MultiServiceRouter,
    engine: Arc<engine::InMemoryEngine>,
}

impl Metered {
    pub(crate) fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(Arc::clone(&engine)));
        router.register_service(
            service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
                Arc::clone(&engine),
            ),
        );
        Self { router, engine }
    }

    /// A SigV4 `Authorization` header whose credential scope names `service`, which is
    /// what routes the request. The signature is never checked: this router runs with
    /// request authentication off.
    fn authorization(service: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/{service}/aws4_request, \
             SignedHeaders=host;x-amz-target, Signature=0123456789abcdef"
        )
    }

    /// One `awsJson` call to the service `signing_name` routes to, as `(status, body)`.
    async fn rpc(
        &self,
        signing_name: &str,
        target_prefix: &str,
        operation: &str,
        body: &Value,
    ) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", format!("{signing_name}.{REGION}.amazonaws.com"))
            .header("content-type", "application/x-amz-json-1.0")
            .header("x-amz-target", format!("{target_prefix}.{operation}"))
            .header("authorization", Self::authorization(signing_name))
            .body(Bytes::from(body.to_string()))
            .expect("a well-formed request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let bytes = response.into_body();
        let parsed = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or_else(|error| {
                panic!(
                    "case {operation}: expected a JSON body, got {error} for {:?}",
                    String::from_utf8_lossy(&bytes)
                )
            })
        };
        (status, parsed)
    }

    /// One Step Functions call, as `(status, body)`.
    pub(crate) async fn sfn(&self, operation: &str, body: &Value) -> (u16, Value) {
        self.rpc("states", "AWSStepFunctions", operation, body).await
    }

    /// One Step Functions call, requiring a 200.
    pub(crate) async fn ok(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.sfn(operation, body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200, got {status} with {answered}"
        );
        answered
    }

    /// One Step Functions call, requiring a refusal.
    async fn refused(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self.sfn(operation, body).await;
        assert_ne!(
            status, 200,
            "case {case:?}: {operation} must be refused, got {answered}"
        );
        answered
    }

    /// One CloudWatch call, requiring a 200.
    async fn cloudwatch(&self, case: &str, operation: &str, body: &Value) -> Value {
        let (status, answered) = self
            .rpc(
                "monitoring",
                "GraniteServiceVersion20100801",
                operation,
                body,
            )
            .await;
        assert_eq!(
            status, 200,
            "case {case:?}: {operation} must answer 200 from CloudWatch, got {status} with \
             {answered}"
        );
        answered
    }

    /// The datapoints the series `metric` of `AWS/States` holds for `dimensions`, as
    /// CloudWatch's own reader answers them.
    async fn datapoints(&self, case: &str, metric: &str, dimensions: &Value) -> Vec<Value> {
        let now = now_secs();
        let stats = self
            .cloudwatch(
                case,
                "GetMetricStatistics",
                &json!({
                    "Namespace": observability::METRIC_NAMESPACE,
                    "MetricName": metric,
                    "Dimensions": dimensions,
                    "StartTime": now - WINDOW_SECS,
                    "EndTime": now + WINDOW_SECS,
                    "Period": PERIOD_SECS,
                    "Statistics": ["Sum", "Maximum", "SampleCount"],
                }),
            )
            .await;
        stats["Datapoints"].as_array().cloned().unwrap_or_default()
    }

    /// The SUM of the series `metric` for `dimensions` over the window — the magnitude a
    /// client charts. A series with no datapoint sums to zero, which is what "the series
    /// carries nothing" reads as.
    pub(crate) async fn summed(&self, case: &str, metric: &str, dimensions: &Value) -> f64 {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["Sum"].as_f64())
            .sum()
    }

    /// How many SAMPLES the series holds over the window, which is how many measurements
    /// were published into it.
    async fn samples(&self, case: &str, metric: &str, dimensions: &Value) -> f64 {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["SampleCount"].as_f64())
            .sum()
    }

    /// The SUM each of `queries` — a `(label, metric, dimension set)` per series — reports
    /// over the window, read in ONE batched `GetMetricData` call, which is how a client
    /// reading a LEDGER of several series at one instant reads them.
    async fn batched_sums(
        &self,
        case: &str,
        queries: &[(String, &str, Value)],
    ) -> Vec<(String, f64)> {
        let now = now_secs();
        let requested: Vec<Value> = queries
            .iter()
            .enumerate()
            .map(|(index, (_, metric, dimensions))| {
                json!({
                    "Id": format!("q{index}"),
                    "MetricStat": {
                        "Metric": {
                            "Namespace": observability::METRIC_NAMESPACE,
                            "MetricName": metric,
                            "Dimensions": dimensions,
                        },
                        "Period": PERIOD_SECS,
                        "Stat": "Sum",
                    },
                    "ReturnData": true,
                })
            })
            .collect();
        let answered = self
            .cloudwatch(
                case,
                "GetMetricData",
                &json!({
                    "MetricDataQueries": requested,
                    "StartTime": now - WINDOW_SECS,
                    "EndTime": now + WINDOW_SECS,
                }),
            )
            .await;
        let results = answered["MetricDataResults"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        queries
            .iter()
            .enumerate()
            .map(|(index, (label, _, _))| {
                let id = format!("q{index}");
                let summed = results
                    .iter()
                    .filter(|result| result["Id"].as_str() == Some(id.as_str()))
                    .flat_map(|result| {
                        result["Values"].as_array().cloned().unwrap_or_default()
                    })
                    .filter_map(|value| value.as_f64())
                    .sum();
                (label.clone(), summed)
            })
            .collect()
    }

    /// The greatest value the series reached over the window, and `None` for a series
    /// that holds nothing: what a client reads off a series reporting a COUNT of what a
    /// resource holds rather than a count of events.
    async fn highest(&self, case: &str, metric: &str, dimensions: &Value) -> Option<f64> {
        self.datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["Maximum"].as_f64())
            .fold(None, |highest: Option<f64>, value| {
                Some(highest.map_or(value, |held: f64| held.max(value)))
            })
    }

    /// The distinct `Unit` the datapoints of the series carry.
    async fn units(&self, case: &str, metric: &str, dimensions: &Value) -> Vec<String> {
        let mut seen: Vec<String> = self
            .datapoints(case, metric, dimensions)
            .await
            .iter()
            .filter_map(|point| point["Unit"].as_str().map(str::to_string))
            .collect();
        seen.sort();
        seen.dedup();
        seen
    }

    /// The values the series `metric` holds for `dimensions`, in the order they were
    /// published.
    ///
    /// Read out of the CloudWatch store through the shared engine — the very store the
    /// reader above answers from — because a series reporting what a resource HOLDS is
    /// about the ORDER of its measurements: several of them fall inside one period, which
    /// a reader folds into one datapoint, so the sequence is what says which mutation
    /// moved the count and which left it.
    fn published(&self, metric: &str, state_machine_arn: &str) -> Vec<f64> {
        let store = cloudwatch::service_state(&*self.engine).expect("the CloudWatch store");
        let store = store.lock();
        store
            .metrics
            .get(&cloudwatch::MetricKey {
                namespace: observability::METRIC_NAMESPACE.to_string(),
                metric_name: metric.to_string(),
                dimensions: observability::resource_dimensions(state_machine_arn),
            })
            .map(|series| series.iter().map(|datum| datum.sum).collect())
            .unwrap_or_default()
    }

    /// Create a state machine, answering its ARN.
    pub(crate) async fn machine(&self, case: &str, name: &str, definition: &str) -> String {
        let created = self
            .ok(
                case,
                "CreateStateMachine",
                &json!({ "name": name, "definition": definition, "roleArn": ROLE_ARN }),
            )
            .await;
        string(&created, "stateMachineArn")
    }

    /// Publish the state machine's current configuration as its next version, answering
    /// that version's ARN.
    async fn publish(&self, case: &str, machine: &str) -> String {
        let published = self
            .ok(
                case,
                "PublishStateMachineVersion",
                &json!({ "stateMachineArn": machine }),
            )
            .await;
        string(&published, "stateMachineVersionArn")
    }

    /// Start one execution of the reference `arn` names, answering its ARN.
    async fn start(&self, case: &str, arn: &str, name: &str) -> String {
        let started = self
            .ok(
                case,
                "StartExecution",
                &json!({ "stateMachineArn": arn, "name": name }),
            )
            .await;
        string(&started, "executionArn")
    }

    /// The execution once it has left `RUNNING`, looked at again until it has or
    /// [`PATIENCE`] has passed.
    pub(crate) async fn settled(&self, case: &str, execution: &str) -> Value {
        let began = Instant::now();
        loop {
            let described = self
                .ok(
                    case,
                    "DescribeExecution",
                    &json!({ "executionArn": execution }),
                )
                .await;
            if described.get("status").and_then(Value::as_str) != Some("RUNNING") {
                return described;
            }
            assert!(
                began.elapsed() < PATIENCE,
                "case {case:?}: expected a terminal status within {PATIENCE:?}, last saw \
                 {described}"
            );
            std::thread::sleep(POLL_INTERVAL);
        }
    }
}

/// Now, in whole epoch seconds, which is how an `awsJson` request carries an instant.
fn now_secs() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("now is after the epoch")
        .as_secs_f64()
        .trunc()
}

/// The dimension set of a series reported per ACTIVITY, as `AWS/States` names it.
pub(crate) fn activity_dimensions(activity_arn: &str) -> Value {
    json!([{ "Name": observability::ACTIVITY_ARN_DIMENSION, "Value": activity_arn }])
}

/// The dimension set of a series reported per STATE MACHINE.
pub(crate) fn machine_dimensions(state_machine_arn: &str) -> Value {
    json!([{ "Name": observability::STATE_MACHINE_ARN_DIMENSION, "Value": state_machine_arn }])
}

/// The dimension set of a series reported per the ALIAS that routed the executions: the
/// state machine's own dimension and the alias' name, as the publisher writes it.
fn alias_dimensions(state_machine_arn: &str, alias_name: &str) -> Value {
    wire_dimensions(&observability::state_machine_alias_dimensions(
        state_machine_arn,
        alias_name,
    ))
}

/// The dimension set of a series reported per the VERSION that served them: the state
/// machine's own dimension and the version's number.
fn version_dimensions(state_machine_arn: &str, version: &str) -> Value {
    wire_dimensions(&observability::state_machine_version_dimensions(
        state_machine_arn,
        version,
    ))
}

/// The dimension set of a series reporting what one state machine HOLDS.
fn resource_dimensions(state_machine_arn: &str) -> Value {
    wire_dimensions(&observability::resource_dimensions(state_machine_arn))
}

/// A dimension set as a CloudWatch request carries it, built from the very construction
/// the publisher writes the series under, so a reader cannot ask for a set the publisher
/// never wrote.
fn wire_dimensions(dimensions: &std::collections::BTreeMap<String, String>) -> Value {
    Value::Array(
        dimensions
            .iter()
            .map(|(name, value)| json!({ "Name": name, "Value": value }))
            .collect(),
    )
}

/// The dimension set of a series reported per API of this service.
fn api_dimensions(api_name: &str) -> Value {
    json!([{ "Name": observability::API_NAME_DIMENSION, "Value": api_name }])
}

/// What a QUALIFIED state machine ARN names inside its state machine: the alias name or
/// the version number its qualifier carries, which is how the series of that reference
/// name it.
fn qualifier_of<'a>(state_machine_arn: &str, qualified_arn: &'a str) -> &'a str {
    qualified_arn
        .strip_prefix(state_machine_arn)
        .and_then(|rest| rest.strip_prefix(':'))
        .unwrap_or_else(|| {
            panic!("expected {qualified_arn:?} to qualify {state_machine_arn:?}")
        })
}

/// A one-state definition whose `Task` state hands its work to `activity_arn`, plus
/// whatever bound the row declares on that state.
fn activity_definition(activity_arn: &str, timeout_seconds: Option<u32>) -> String {
    let mut state = json!({ "Type": "Task", "Resource": activity_arn, "End": true });
    if let Some(seconds) = timeout_seconds {
        state
            .as_object_mut()
            .expect("the state is an object")
            .insert("TimeoutSeconds".to_string(), json!(seconds));
    }
    json!({ "StartAt": "Work", "States": { "Work": state } }).to_string()
}

/// How the row's worker answers the task it was handed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Worker {
    /// It reports the task succeeded.
    Succeeds,
    /// It reports the task failed.
    Fails,
    /// It takes the task and never answers, so the state's own bound closes it.
    GoesQuiet,
    /// Nothing polls at all, so the task stays queued for one.
    NeverPolls,
}

/// One row of the activity table: what the workers of one activity did, and what the
/// per-activity series must then hold.
struct ActivityRow {
    case: &'static str,
    /// How many executions the row starts, each handing one task to the one activity.
    executions: usize,
    worker: Worker,
    /// The bound the scheduling state declares, in seconds; `None` for a state that
    /// declares none.
    timeout_seconds: Option<u32>,
    /// What the five counting series must hold, summed over the row's executions:
    /// scheduled, started, succeeded, failed, timed out.
    counted: [f64; 5],
    /// The terminal status each execution reaches, and `None` for a row whose executions
    /// are still running when it is read.
    status: Option<&'static str>,
}

fn activity_rows() -> Vec<ActivityRow> {
    vec![
        ActivityRow {
            case: "a worker that reports the task succeeded",
            executions: 1,
            worker: Worker::Succeeds,
            timeout_seconds: None,
            counted: [1.0, 1.0, 1.0, 0.0, 0.0],
            status: Some("SUCCEEDED"),
        },
        ActivityRow {
            case: "two executions handing work to one activity",
            executions: 2,
            worker: Worker::Succeeds,
            timeout_seconds: None,
            counted: [2.0, 2.0, 2.0, 0.0, 0.0],
            status: Some("SUCCEEDED"),
        },
        ActivityRow {
            case: "a worker that reports the task failed",
            executions: 1,
            worker: Worker::Fails,
            timeout_seconds: None,
            counted: [1.0, 1.0, 0.0, 1.0, 0.0],
            status: Some("FAILED"),
        },
        ActivityRow {
            case: "a worker that goes quiet, so the state's own bound closes the task",
            executions: 1,
            worker: Worker::GoesQuiet,
            timeout_seconds: Some(1),
            counted: [1.0, 1.0, 0.0, 0.0, 1.0],
            status: Some("FAILED"),
        },
        ActivityRow {
            case: "a task no worker polls for, which is backlog and nothing else",
            executions: 1,
            worker: Worker::NeverPolls,
            timeout_seconds: None,
            counted: [1.0, 0.0, 0.0, 0.0, 0.0],
            status: None,
        },
    ]
}

/// The work a `Task` state hands to an activity's workers is counted in the per-activity
/// series, dimensioned on that activity's ARN alone, whatever closed each task — and the
/// executions themselves stay counted under their state machine.
#[tokio::test]
async fn activity_work_is_counted_per_activity() {
    let metered = Metered::new();

    for (index, row) in activity_rows().into_iter().enumerate() {
        let ActivityRow {
            case,
            executions,
            worker,
            timeout_seconds,
            counted,
            status,
        } = row;
        // One activity and one state machine per row, so each row's series are its own.
        let created = metered
            .ok(
                case,
                "CreateActivity",
                &json!({ "name": format!("metered-{index}") }),
            )
            .await;
        let activity = string(&created, "activityArn");
        assert_eq!(
            activity,
            format!("arn:aws:states:{REGION}:{ACCOUNT}:activity:metered-{index}"),
            "case {case:?}: the activity ARN is the one its name mints"
        );
        let machine = metered
            .machine(
                case,
                &format!("metered-{index}"),
                &activity_definition(&activity, timeout_seconds),
            )
            .await;

        let mut started: Vec<String> = Vec::new();
        for run in 0..executions {
            started.push(metered.start(case, &machine, &format!("run-{run}")).await);
        }
        // The worker takes each task and answers it as the row says.
        if worker != Worker::NeverPolls {
            for _ in 0..executions {
                let polled = metered
                    .ok(
                        case,
                        "GetActivityTask",
                        &json!({ "activityArn": activity, "workerName": WORKER }),
                    )
                    .await;
                let token = string(&polled, "taskToken");
                match worker {
                    Worker::Succeeds => {
                        metered
                            .ok(
                                case,
                                "SendTaskSuccess",
                                &json!({ "taskToken": token, "output": r#"{"done":true}"# }),
                            )
                            .await;
                    }
                    Worker::Fails => {
                        metered
                            .ok(
                                case,
                                "SendTaskFailure",
                                &json!({ "taskToken": token, "error": "WorkerFailed" }),
                            )
                            .await;
                    }
                    // It holds the token and reports nothing: the state's own bound is
                    // what closes the task.
                    Worker::GoesQuiet | Worker::NeverPolls => {}
                }
            }
        }
        if let Some(expected) = status {
            for execution in &started {
                let settled = metered.settled(case, execution).await;
                assert_eq!(
                    settled.get("status").and_then(Value::as_str),
                    Some(expected),
                    "case {case:?}: expected the execution to end {expected}, got {settled}"
                );
            }
        }

        let dimensions = activity_dimensions(&activity);
        for (metric, expected) in [
            (observability::ACTIVITIES_SCHEDULED_METRIC, counted[0]),
            (observability::ACTIVITIES_STARTED_METRIC, counted[1]),
            (observability::ACTIVITIES_SUCCEEDED_METRIC, counted[2]),
            (observability::ACTIVITIES_FAILED_METRIC, counted[3]),
            (observability::ACTIVITIES_TIMED_OUT_METRIC, counted[4]),
        ] {
            let summed = metered.summed(case, metric, &dimensions).await;
            assert_eq!(
                summed, expected,
                "case {case:?}: expected {metric} to count {expected}, got {summed}"
            );
            if expected > 0.0 {
                let units = metered.units(case, metric, &dimensions).await;
                assert_eq!(
                    units,
                    vec![observability::COUNT_UNIT.to_string()],
                    "case {case:?}: {metric} counts things, got {units:?}"
                );
            }
        }
        // The spans: one sample per task that STARTED for the span it stayed queued, and
        // one per task that CLOSED for the span it ran and for the whole span it took —
        // each a span of milliseconds.
        let closed = counted[2] + counted[3] + counted[4];
        for (metric, expected) in [
            (observability::ACTIVITY_SCHEDULE_TIME_METRIC, counted[1]),
            (observability::ACTIVITY_RUN_TIME_METRIC, closed),
            (observability::ACTIVITY_TIME_METRIC, closed),
        ] {
            let samples = metered.samples(case, metric, &dimensions).await;
            assert_eq!(
                samples, expected,
                "case {case:?}: expected {expected} {metric} samples, got {samples}"
            );
            if expected > 0.0 {
                let units = metered.units(case, metric, &dimensions).await;
                assert_eq!(
                    units,
                    vec!["Milliseconds".to_string()],
                    "case {case:?}: {metric} is a span of milliseconds, got {units:?}"
                );
            }
        }
        // The two families never cross: no per-activity series is written under the
        // state machine's dimension, and no execution series under the activity's.
        for metric in [
            observability::ACTIVITIES_SCHEDULED_METRIC,
            observability::ACTIVITIES_SUCCEEDED_METRIC,
            observability::ACTIVITIES_FAILED_METRIC,
        ] {
            let crossed = metered
                .summed(case, metric, &machine_dimensions(&machine))
                .await;
            assert_eq!(
                crossed, 0.0,
                "case {case:?}: {metric} belongs to the activity's dimension alone, got \
                 {crossed}"
            );
        }
        let crossed = metered
            .summed(case, observability::EXECUTIONS_STARTED_METRIC, &dimensions)
            .await;
        assert_eq!(
            crossed, 0.0,
            "case {case:?}: ExecutionsStarted belongs to the state machine's dimension alone, \
             got {crossed}"
        );
        // And the executions themselves are still counted under their state machine, so
        // the absences above are about the DIMENSION rather than about publishing at all.
        #[allow(clippy::cast_precision_loss)]
        let expected_starts = executions as f64;
        let starts = metered
            .summed(
                case,
                observability::EXECUTIONS_STARTED_METRIC,
                &machine_dimensions(&machine),
            )
            .await;
        assert_eq!(
            starts, expected_starts,
            "case {case:?}: expected {expected_starts} executions counted, got {starts}"
        );
    }
}

/// One row of the capacity table: the API, and how many calls of it the test made.
struct CapacityRow {
    case: &'static str,
    api_name: &'static str,
    calls: f64,
}

/// How many starts the burst below makes.
const BURST: usize = 12;

/// Every call this service serves consumes one unit of the capacity of the API it named,
/// so the sum over the window of that API's series is the number of calls made against
/// it — the refused ones included, because the capacity is consumed by the request.
#[tokio::test]
async fn every_served_call_consumes_the_capacity_of_its_api() {
    let case = "a burst of calls across several APIs";
    let metered = Metered::new();

    // One create, a burst of starts, one start the service refuses, and two kinds of
    // read, so the rows span a create, a mutation made in bulk, a refusal, a read of one
    // resource and a listing of them.
    let machine = metered.machine(case, "capacity", HELLO).await;
    for run in 0..BURST {
        metered.start(case, &machine, &format!("run-{run}")).await;
    }
    // A start the service refuses: the ARN names another region, which is a defect in
    // the request rather than a state machine that is absent.
    metered
        .refused(
            case,
            "StartExecution",
            &json!({
                "stateMachineArn":
                    format!("arn:aws:states:eu-west-1:{ACCOUNT}:stateMachine:elsewhere"),
                "name": "refused",
            }),
        )
        .await;
    for _ in 0..3 {
        metered
            .ok(
                case,
                "DescribeStateMachine",
                &json!({ "stateMachineArn": machine }),
            )
            .await;
    }
    for _ in 0..2 {
        metered.ok(case, "ListStateMachines", &json!({})).await;
    }
    // A request whose target names NO operation of this service reaches no API of it, so
    // it is metered in none: the name a call is counted under comes from this service's
    // own catalog of its operations, not from the request's own text, so a caller cannot
    // make the series of this namespace grow by naming something else.
    for name in ["NoSuchOperation", "DescribeTable"] {
        let (status, answered) = metered.sfn(name, &json!({})).await;
        assert_ne!(
            status, 200,
            "case {case:?}: {name:?} is no operation of this service, got {answered}"
        );
        let consumed = metered
            .summed(
                case,
                observability::CONSUMED_CAPACITY_METRIC,
                &api_dimensions(name),
            )
            .await;
        assert_eq!(
            consumed, 0.0,
            "case {case:?}: expected {name:?} to be metered in nothing, got {consumed}"
        );
    }
    // A target naming nothing at all is refused the same way; a series of an empty name
    // is one CloudWatch itself will not read, which is the point: no such series exists.
    let (status, answered) = metered.sfn("", &json!({})).await;
    assert_ne!(
        status, 200,
        "case {case:?}: an empty target names no operation of this service, got {answered}"
    );

    #[allow(clippy::cast_precision_loss)]
    let rows = vec![
        CapacityRow {
            case: "the API a burst of mutations was made against, one of them refused",
            api_name: "StartExecution",
            calls: BURST as f64 + 1.0,
        },
        CapacityRow {
            case: "the API one create was made against",
            api_name: "CreateStateMachine",
            calls: 1.0,
        },
        CapacityRow {
            case: "the API a resource was read through",
            api_name: "DescribeStateMachine",
            calls: 3.0,
        },
        CapacityRow {
            case: "the API the resources were listed through",
            api_name: "ListStateMachines",
            calls: 2.0,
        },
        CapacityRow {
            case: "an API no call was made against",
            api_name: "DeleteActivity",
            calls: 0.0,
        },
    ];
    for row in rows {
        let CapacityRow {
            case,
            api_name,
            calls,
        } = row;
        let dimensions = api_dimensions(api_name);
        let consumed = metered
            .summed(case, observability::CONSUMED_CAPACITY_METRIC, &dimensions)
            .await;
        assert_eq!(
            consumed, calls,
            "case {case:?}: expected {api_name} to have consumed {calls}, got {consumed}"
        );
        // One unit per call, so the samples are the calls too.
        let samples = metered
            .samples(case, observability::CONSUMED_CAPACITY_METRIC, &dimensions)
            .await;
        assert_eq!(
            samples, calls,
            "case {case:?}: expected {calls} {api_name} samples, got {samples}"
        );
        if calls > 0.0 {
            let units = metered
                .units(case, observability::CONSUMED_CAPACITY_METRIC, &dimensions)
                .await;
            assert_eq!(
                units,
                vec![observability::COUNT_UNIT.to_string()],
                "case {case:?}: consumed capacity is a count, got {units:?}"
            );
        }
    }
}

/// What the two counting series hold for `machine`, in the order they were published.
fn counts(metered: &Metered, machine: &str) -> (Vec<f64>, Vec<f64>) {
    (
        metered.published(observability::VERSION_COUNT_METRIC, machine),
        metered.published(observability::ALIAS_COUNT_METRIC, machine),
    )
}

/// How many versions and how many aliases a state machine holds is reported per state
/// machine as of every ACCEPTED mutation of them, so a refused publish and an idempotent
/// repeat leave the counts where the last accepted mutation put them.
#[tokio::test]
async fn version_and_alias_counts_follow_the_accepted_mutations() {
    let metered = Metered::new();
    let machine = metered
        .machine("the state machine the counts are about", "counted", HELLO)
        .await;

    // A create that published nothing has moved neither count, so neither series carries
    // anything about this state machine yet.
    let case = "a create that published no version";
    assert_eq!(
        counts(&metered, &machine),
        (vec![], vec![]),
        "case {case:?}: neither count has been reported yet"
    );
    for metric in [
        observability::VERSION_COUNT_METRIC,
        observability::ALIAS_COUNT_METRIC,
    ] {
        let held = metered
            .highest(case, metric, &resource_dimensions(&machine))
            .await;
        assert_eq!(
            held, None,
            "case {case:?}: a client reads no {metric} at all, got {held:?}"
        );
    }

    let case = "the first accepted publish";
    let first = metered.publish(case, &machine).await;
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0], vec![]),
        "case {case:?}: one version is held, and no alias"
    );

    // "It doesn't create a duplicate state machine version if it already exists for the
    // current revision": an idempotent repeat holds the count still, because it created
    // nothing.
    let case = "an idempotent repeat of that publish";
    let repeated = metered.publish(case, &machine).await;
    assert_eq!(
        repeated, first,
        "case {case:?}: the repeat answers the version the first publish created"
    );
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0], vec![]),
        "case {case:?}: a publish that created no version reports no count"
    );

    // A publish the service REFUSES — its `revisionId` is not the state machine's —
    // leaves the count where the accepted one put it.
    let case = "a publish refused for a stale revision";
    metered
        .refused(
            case,
            "PublishStateMachineVersion",
            &json!({ "stateMachineArn": machine, "revisionId": "not-the-current-revision" }),
        )
        .await;
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0], vec![]),
        "case {case:?}: a refused publish moves nothing"
    );

    let case = "an update and a second publish";
    metered
        .ok(
            case,
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine, "definition": HELLO_AGAIN }),
        )
        .await;
    let second = metered.publish(case, &machine).await;
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0, 2.0], vec![]),
        "case {case:?}: the second accepted publish reports two versions"
    );

    // The aliases move their own count the same way.
    let case = "two alias creates and one delete";
    for (name, version) in [("PROD", &first), ("BETA", &second)] {
        metered
            .ok(
                case,
                "CreateStateMachineAlias",
                &json!({
                    "name": name,
                    "routingConfiguration": [
                        { "stateMachineVersionArn": version, "weight": 100 },
                    ],
                }),
            )
            .await;
    }
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0, 2.0], vec![1.0, 2.0]),
        "case {case:?}: each accepted create reports the count it left"
    );
    metered
        .ok(
            case,
            "DeleteStateMachineAlias",
            &json!({ "stateMachineAliasArn": format!("{machine}:BETA") }),
        )
        .await;
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0, 2.0], vec![1.0, 2.0, 1.0]),
        "case {case:?}: the delete reports the one alias that is left"
    );

    // A version no alias points at is deletable, and that is another accepted mutation.
    let case = "a version delete";
    metered
        .ok(
            case,
            "DeleteStateMachineVersion",
            &json!({ "stateMachineVersionArn": second }),
        )
        .await;
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0, 2.0, 1.0], vec![1.0, 2.0, 1.0]),
        "case {case:?}: the delete reports the one version that is left"
    );

    // What a CLIENT reads off those series: the greatest count the state machine ever
    // held, which is the two versions and the two aliases it carried.
    let case = "the counts a client reads back";
    for (metric, expected) in [
        (observability::VERSION_COUNT_METRIC, 2.0),
        (observability::ALIAS_COUNT_METRIC, 2.0),
    ] {
        let highest = metered
            .highest(case, metric, &resource_dimensions(&machine))
            .await;
        assert_eq!(
            highest,
            Some(expected),
            "case {case:?}: expected {metric} to have reached {expected}, got {highest:?}"
        );
        let units = metered
            .units(case, metric, &resource_dimensions(&machine))
            .await;
        assert_eq!(
            units,
            vec![observability::COUNT_UNIT.to_string()],
            "case {case:?}: {metric} counts things, got {units:?}"
        );
    }

    // The counts are per STATE MACHINE: another one's publish is reported under its own
    // ARN and says nothing about this one.
    let case = "another state machine's own publish";
    let other = metered.machine(case, "counted-other", HELLO).await;
    metered.publish(case, &other).await;
    assert_eq!(
        counts(&metered, &other),
        (vec![1.0], vec![]),
        "case {case:?}: the other state machine reports its own count"
    );
    assert_eq!(
        counts(&metered, &machine),
        (vec![1.0, 2.0, 1.0], vec![1.0, 2.0, 1.0]),
        "case {case:?}: and nothing of this one's moved"
    );
}

/// An execution started through a qualified ARN is counted under the alias that routed
/// it and the version it ran as well as under its state machine, so a client can see
/// which reference served which traffic.
#[tokio::test]
async fn an_execution_is_counted_under_every_reference_it_was_started_through() {
    let metered = Metered::new();
    let case = "executions started through two aliases and through the state machine";
    let machine = metered.machine(case, "split", HELLO).await;
    let first = metered.publish(case, &machine).await;
    metered
        .ok(
            case,
            "UpdateStateMachine",
            &json!({ "stateMachineArn": machine, "definition": HELLO_AGAIN }),
        )
        .await;
    let second = metered.publish(case, &machine).await;

    // One alias per version, each routing all of its traffic to that version, so which
    // version served a run is decided rather than drawn.
    let mut aliases: Vec<(String, String, usize)> = Vec::new();
    for (name, version, runs) in [("LIVE", &first, 2usize), ("NEXT", &second, 1usize)] {
        let created = metered
            .ok(
                case,
                "CreateStateMachineAlias",
                &json!({
                    "name": name,
                    "routingConfiguration": [
                        { "stateMachineVersionArn": version, "weight": 100 },
                    ],
                }),
            )
            .await;
        aliases.push((
            string(&created, "stateMachineAliasArn"),
            version.clone(),
            runs,
        ));
    }

    let mut executions: Vec<String> = Vec::new();
    for (alias, _, runs) in &aliases {
        for run in 0..*runs {
            let name = format!("through-{}-{run}", executions.len());
            let started = metered.start(case, alias, &name).await;
            executions.push(started);
        }
    }
    // And one run started with the UNQUALIFIED ARN, which "Step Functions doesn't
    // associate with a version": it is counted under the state machine alone.
    executions.push(metered.start(case, &machine, "unqualified").await);
    for execution in &executions {
        let settled = metered.settled(case, execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("SUCCEEDED"),
            "case {case:?}: expected every run to succeed, got {settled}"
        );
    }

    // (case, the dimension set of the reference, the runs it must have counted)
    let mut rows: Vec<(String, Value, f64)> = vec![(
        "the state machine, which every run belongs to".to_string(),
        machine_dimensions(&machine),
        4.0,
    )];
    for (alias, version, runs) in &aliases {
        #[allow(clippy::cast_precision_loss)]
        let counted = *runs as f64;
        rows.push((
            format!("the alias {alias} that routed its own runs"),
            alias_dimensions(&machine, qualifier_of(&machine, alias)),
            counted,
        ));
        rows.push((
            format!("the version {version} that served them"),
            version_dimensions(&machine, qualifier_of(&machine, version)),
            counted,
        ));
    }
    let mut through_references = 0.0;
    let references = rows.len();
    for (case, dimensions, expected) in rows {
        for metric in [
            observability::EXECUTIONS_STARTED_METRIC,
            observability::EXECUTIONS_SUCCEEDED_METRIC,
        ] {
            let counted = metered.summed(&case, metric, &dimensions).await;
            assert_eq!(
                counted, expected,
                "case {case:?}: expected {metric} to count {expected}, got {counted}"
            );
        }
        // The span every run is measured for is published under each reference too, one
        // sample per run.
        let samples = metered
            .samples(&case, observability::EXECUTION_TIME_METRIC, &dimensions)
            .await;
        assert_eq!(
            samples, expected,
            "case {case:?}: expected {expected} ExecutionTime samples, got {samples}"
        );
        if dimensions != machine_dimensions(&machine) {
            through_references += expected;
        }
    }
    assert_eq!(
        references, 5,
        "case {case:?}: the state machine, two aliases and two versions are five references"
    );
    // The alias and version series are a SPLIT of the state machine's: the runs started
    // through a reference, counted once per alias and once per version, are the three
    // qualified ones and not the unqualified fourth.
    assert_eq!(
        through_references, 6.0,
        "case {case:?}: three qualified runs are counted under one alias and one version \
         each, got {through_references}"
    );
    // And the two dimensions never carry the OTHER kind of reference: an alias ARN is
    // not a state machine ARN, so nothing is written under it there.
    for (alias, version, _) in &aliases {
        for reference in [alias, version] {
            let crossed = metered
                .summed(
                    case,
                    observability::EXECUTIONS_STARTED_METRIC,
                    &machine_dimensions(reference),
                )
                .await;
            assert_eq!(
                crossed, 0.0,
                "case {case:?}: {reference} is a reference of its own, not a state machine \
                 ARN, got {crossed}"
            );
        }
    }
    // A client watching a rollout reads the whole LEDGER at once — the state machine's
    // total beside each reference's share — so every one of those series is served in ONE
    // batched read as well as one at a time.
    let mut ledger: Vec<(String, &str, Value)> = vec![(
        "Started[sm]".to_string(),
        observability::EXECUTIONS_STARTED_METRIC,
        machine_dimensions(&machine),
    )];
    for (alias, version, _) in &aliases {
        ledger.push((
            format!("Started[{alias}]"),
            observability::EXECUTIONS_STARTED_METRIC,
            alias_dimensions(&machine, qualifier_of(&machine, alias)),
        ));
        ledger.push((
            format!("Succeeded[{alias}]"),
            observability::EXECUTIONS_SUCCEEDED_METRIC,
            alias_dimensions(&machine, qualifier_of(&machine, alias)),
        ));
        ledger.push((
            format!("Started[{version}]"),
            observability::EXECUTIONS_STARTED_METRIC,
            version_dimensions(&machine, qualifier_of(&machine, version)),
        ));
    }
    ledger.push((
        "VersionCount".to_string(),
        observability::VERSION_COUNT_METRIC,
        resource_dimensions(&machine),
    ));
    ledger.push((
        "AliasCount".to_string(),
        observability::ALIAS_COUNT_METRIC,
        resource_dimensions(&machine),
    ));
    let read = metered.batched_sums(case, &ledger).await;
    for (label, summed) in &read {
        assert!(
            *summed > 0.0,
            "case {case:?}: every series of the ledger carries data in a batched read, but \
             {label} read {summed} of {read:?}"
        );
    }
}

/// Every ACCEPTED redrive is counted once, and the re-run's own outcome is counted in
/// the redriven series of that outcome as well as in the execution's own; a redrive the
/// service refuses is counted in neither.
#[tokio::test]
async fn a_redrive_is_counted_once_and_its_outcome_beside_it() {
    let metered = Metered::new();
    let case = "an execution redriven twice, with a refused redrive between";
    let machine = metered.machine(case, "redriven", FAILING).await;
    let dimensions = machine_dimensions(&machine);
    let execution = metered.start(case, &machine, "run").await;
    metered.settled(case, &execution).await;

    // Nothing has been redriven yet, so the redrive series carry nothing while the
    // execution's own failure is already counted.
    for metric in [
        observability::EXECUTIONS_REDRIVEN_METRIC,
        observability::REDRIVEN_EXECUTIONS_FAILED_METRIC,
        observability::REDRIVEN_EXECUTIONS_SUCCEEDED_METRIC,
    ] {
        let counted = metered.summed(case, metric, &dimensions).await;
        assert_eq!(
            counted, 0.0,
            "case {case:?}: expected {metric} to count nothing before a redrive, got {counted}"
        );
    }

    // Each redrive is one row: the counts the series must hold once its re-run has ended.
    for redrive in 1..=2 {
        metered
            .ok(
                case,
                "RedriveExecution",
                &json!({ "executionArn": execution, "clientToken": format!("token-{redrive}") }),
            )
            .await;
        // The re-run is in flight, so a second redrive is refused — and a refusal is
        // counted in nothing.
        metered
            .refused(
                case,
                "RedriveExecution",
                &json!({ "executionArn": execution }),
            )
            .await;
        let settled = metered.settled(case, &execution).await;
        assert_eq!(
            settled.get("status").and_then(Value::as_str),
            Some("FAILED"),
            "case {case:?}: the re-run reaches the definition's own outcome, got {settled}"
        );

        #[allow(clippy::cast_precision_loss)]
        let redriven = redrive as f64;
        for (metric, expected) in [
            (observability::EXECUTIONS_REDRIVEN_METRIC, redriven),
            (observability::REDRIVEN_EXECUTIONS_FAILED_METRIC, redriven),
            (observability::REDRIVEN_EXECUTIONS_SUCCEEDED_METRIC, 0.0),
            // The first attempt plus each re-run, all counted in the execution's own
            // completion series.
            (observability::EXECUTIONS_FAILED_METRIC, redriven + 1.0),
            // A redrive re-runs an execution that was started once, so the started
            // series does not move.
            (observability::EXECUTIONS_STARTED_METRIC, 1.0),
        ] {
            let counted = metered.summed(case, metric, &dimensions).await;
            assert_eq!(
                counted, expected,
                "case {case:?} after redrive {redrive}: expected {metric} to count {expected}, \
                 got {counted}"
            );
        }
    }
}
