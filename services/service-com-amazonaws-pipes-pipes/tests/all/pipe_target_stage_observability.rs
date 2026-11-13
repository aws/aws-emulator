// SPDX-License-Identifier: CC0-1.0

//! Whether a client can TELL a pipe whose target refused it from a pipe that never ran,
//! end to end over the wire with pipes, Amazon SQS, Lambda, IAM and CloudWatch mounted on
//! ONE engine — the reads an operator dashboard or an alarm is built on, asked exactly as
//! such a client asks them.
//!
//! The RULE asserted over the table below: a RUNNING pipe's target stage makes the call
//! the TARGET's own API takes its events by, as the pipe's execution role, so a role the
//! policies in force on it do not let make that call is the TARGET stage failing — and the
//! pipe publishes that outcome as its own `TargetStageFailed` counter in
//! `AWS/EventBridge/Pipes`, dimensioned on the pipe's name ALONE and carrying the unit
//! `None`, reading at least 1 for a pipe whose target call was refused and EXISTING at
//! exactly 0 for a pipe whose was not. Which target the pipe names changes only the call
//! the role is asked about, never the counter the outcome is published on.
//!
//! The rows differ along what the rule generalises over: WHICH KIND of target the pipe
//! delivers to — an Amazon SQS queue, whose events travel by `SendMessage`, and a Lambda
//! function, whose travel by `Invoke` — and HOW the role does or does not bar that
//! target's own call (a grant naming it on that resource, policies stating nothing about
//! it, and an explicit deny of it on that resource while it is allowed everywhere else).
//! So each target kind appears both delivering and refused, and the counter is read the
//! same way for all of them.
//!
//! `tests/all/pipe_metrics.rs` asserts WHAT a single running pipe publishes and
//! `tests/all/pipe_metric_attribution.rs` WHICH pipe each datapoint belongs to, both over
//! queue targets read with `ListMetrics` narrowed by namespace alone. This suite is the
//! client-facing half neither covers: the other target kind's own call, and the NARROWED
//! catalogue read — `ListMetrics` filtered by namespace, metric name and the
//! `PipeName` dimension's value, which is how a dashboard asks for one pipe's series and
//! what has to report the one-dimension set.

use std::time::{Duration, Instant};

use serde_json::{json, Value};
use service_com_amazonaws_pipes_pipes::state::execution_role::SERVICE_PRINCIPAL;
use service_com_amazonaws_pipes_pipes::state::metrics::{
    COUNTER_UNIT, EVENT_COUNT_METRIC, METRIC_NAMESPACE, PIPE_NAME_DIMENSION,
    TARGET_STAGE_FAILED_METRIC,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::pipe_data_plane::{
    collect_at_least, collect_during, given_queue, rpc, send, wait_for_counted, LOOK_AGAIN, QUIET,
};
use crate::pipe_lambda_target::given_function;
use crate::pipe_lifecycle::{call, REGION};
use crate::pipe_metrics::{datapoints, given_role_stating, series};

/// How long a wait for something a pipe has to do — the pipe settling, a delivery, an
/// interpreted run of the function target, a counter moving — is given before the row is a
/// failure. Well above the poll interval, the state-settle span and one run of the
/// handler, so a row fails only when the pipe did not do the work at all.
const BUDGET: Duration = Duration::from_secs(60);

/// The queue attributes that between them say how many records a source still HOLDS: the
/// ones waiting on it and the ones a consumer has taken and not deleted. A record a pipe
/// could not deliver is still one of them, which is what tells a withheld record from a
/// consumed one.
const HELD_ATTRIBUTES: &[&str] = &[
    "ApproximateNumberOfMessages",
    "ApproximateNumberOfMessagesNotVisible",
];

/// The function target's handler: it reports how many events the invocation was handed and
/// nothing else, so a pipe whose target call is allowed delivers its batch with no second
/// service in the way. It says nothing about the records, so the batch it took counts as
/// wholly processed and leaves the source.
const HANDLER_SOURCE: &str = r#"
def lambda_handler(event, context):
    return {"taken": len(event)}
"#;

/// Pipes, plus every service this rule is observed through, on ONE engine — the shape
/// `core/router/src/lib.rs` mounts them in: the two kinds of target a pipe delivers to
/// (Amazon SQS and Lambda), the IAM store its execution role lives in, and the CloudWatch
/// store its own counters are read back from.
fn emulator() -> MultiServiceRouter {
    let engine: std::sync::Arc<engine::InMemoryEngine> =
        std::sync::Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(
        std::sync::Arc::clone(&engine),
    ));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(
        std::sync::Arc::clone(&engine),
    ));
    router.register_service(
        service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(std::sync::Arc::clone(
            &engine,
        )),
    );
    router.register_service(
        service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router.register_service(
        service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(
            std::sync::Arc::clone(&engine),
        ),
    );
    router
}

/// Which kind of resource a row's `Target` names, and therefore which call its target stage
/// makes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum TargetKind {
    /// An Amazon SQS queue: the events travel by `sqs:SendMessage`.
    Queue,
    /// A Lambda function: they travel by `lambda:InvokeFunction`.
    Function,
}

/// One row: the kind of target, the policy its role carries against that target, and
/// whether the target ends up with the record.
struct Row {
    /// The pipe's name, which is also the value of the one dimension its series carries.
    case: &'static str,
    kind: TargetKind,
    /// The statements of the pipe's role, written against this pipe's OWN source and
    /// target.
    policy: fn(&str, &str) -> Value,
    /// Whether this pipe's target ends up with the record.
    delivered: bool,
}

/// Every Amazon SQS call a pipe's SOURCE stage makes on its own queue: the receive, the
/// delete that finishes a batch with it, and the attribute read that says when records a
/// failed batch checked out come back. Every row grants these, so no row fails before its
/// target stage.
fn source_grant(source: &str) -> Value {
    json!({
        "Effect": "Allow",
        "Action": ["sqs:ReceiveMessage", "sqs:DeleteMessage", "sqs:GetQueueAttributes"],
        "Resource": source,
    })
}

/// The `CreatePipe` body of one row: one record per batch collected with no batching
/// window, so a record sent to the source is polled as its own execution.
fn pipe_body(source_arn: &str, target_arn: &str, role_arn: &str) -> Value {
    json!({
        "Source": source_arn,
        "Target": target_arn,
        "RoleArn": role_arn,
        "DesiredState": "RUNNING",
        "SourceParameters": {
            "SqsQueueParameters": { "BatchSize": 1, "MaximumBatchingWindowInSeconds": 0 },
        },
    })
}

/// What one row's pipe was given, kept so the assertions can read where its record was
/// meant to end up.
struct Prepared<'row> {
    row: &'row Row,
    source_url: String,
    /// The target queue's url, for a row whose target is a queue.
    target_url: Option<String>,
    marker: String,
}

/// The series `ListMetrics` catalogues for ONE pipe's `metric`, asked the way a dashboard
/// asks for one pipe: narrowed by namespace, by metric name and by the value of the
/// `PipeName` dimension.
async fn catalogued(router: &MultiServiceRouter, pipe: &str, metric: &str) -> Vec<Value> {
    let (status, listed) = rpc(
        router,
        "monitoring",
        "GraniteServiceVersion20100801",
        "ListMetrics",
        &json!({
            "Namespace": METRIC_NAMESPACE,
            "MetricName": metric,
            "Dimensions": [{ "Name": PIPE_NAME_DIMENSION, "Value": pipe }],
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {pipe:?}: ListMetrics for {metric} answered {status} {listed}"
    );
    listed["Metrics"].as_array().cloned().unwrap_or_default()
}

/// Wait until one pipe's `metric` sums to at least `want`, answering what it summed to
/// last.
async fn wait_for_sum(router: &MultiServiceRouter, pipe: &str, metric: &str, want: f64) -> f64 {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (_, sum) = series(router, pipe, metric).await;
        if sum >= want || Instant::now() >= deadline {
            return sum;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// Wait until one pipe's `metric` has a datapoint at all, answering how many it had last:
/// a counter of a running pipe EXISTS even for a kind of execution that pipe never had.
async fn wait_for_series(router: &MultiServiceRouter, pipe: &str, metric: &str) -> usize {
    let deadline = Instant::now() + BUDGET;
    loop {
        let (points, _) = series(router, pipe, metric).await;
        if points > 0 || Instant::now() >= deadline {
            return points;
        }
        tokio::time::sleep(LOOK_AGAIN).await;
    }
}

/// RULE: a RUNNING pipe's target stage makes the call its target's own API takes events by,
/// as the pipe's execution role — so a role that may not make it fails the pipe at the
/// TARGET stage — and the pipe publishes that outcome as `TargetStageFailed` in
/// `AWS/EventBridge/Pipes`, dimensioned on `PipeName` alone with the unit `None`: at least
/// 1 for a pipe whose target call was refused, and an existing 0 for one whose was not,
/// whichever kind of target the pipe names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_refused_target_call_is_published_as_target_stage_failed_for_either_target_kind() {
    let rows = [
        Row {
            case: "queue-target-send-allowed",
            kind: TargetKind::Queue,
            policy: |source, target| {
                json!([
                    source_grant(source),
                    { "Effect": "Allow", "Action": "sqs:SendMessage", "Resource": target },
                ])
            },
            delivered: true,
        },
        Row {
            // The twin of the row above on identical wiring, whose role simply does not
            // state the queue's own call.
            case: "queue-target-send-not-stated",
            kind: TargetKind::Queue,
            policy: |source, _target| json!([source_grant(source)]),
            delivered: false,
        },
        Row {
            case: "function-target-invoke-allowed",
            kind: TargetKind::Function,
            policy: |source, target| {
                json!([
                    source_grant(source),
                    { "Effect": "Allow", "Action": "lambda:InvokeFunction", "Resource": target },
                ])
            },
            delivered: true,
        },
        Row {
            // The same function wiring with the invocation left unstated: the other
            // target kind's own call, refused the same way.
            case: "function-target-invoke-not-stated",
            kind: TargetKind::Function,
            policy: |source, _target| json!([source_grant(source)]),
            delivered: false,
        },
        Row {
            // Allowed everywhere and denied on this one function: a deny bars the call
            // the grant would have let through.
            case: "function-target-invoke-denied",
            kind: TargetKind::Function,
            policy: |source, target| {
                json!([
                    source_grant(source),
                    { "Effect": "Allow", "Action": "lambda:*", "Resource": "*" },
                    { "Effect": "Deny", "Action": "lambda:InvokeFunction", "Resource": target },
                ])
            },
            delivered: false,
        },
    ];

    // Every row's pipe runs on ONE emulator at the same time, so "this counter reads 0" is
    // a statement about a pipe that is running beside pipes whose same counter is rising,
    // rather than about an idle account.
    let router = emulator();
    let mut prepared: Vec<Prepared<'_>> = Vec::new();
    for row in &rows {
        let (source_url, source_arn) = given_queue(&router, &format!("{}-source", row.case)).await;
        let (target_url, target_arn) = match row.kind {
            TargetKind::Queue => {
                let (url, arn) = given_queue(&router, &format!("{}-target", row.case)).await;
                (Some(url), arn)
            }
            TargetKind::Function => {
                let arn = given_function(
                    &router,
                    row.case,
                    &format!("{}-target", row.case),
                    HANDLER_SOURCE,
                    json!({}),
                )
                .await;
                (None, arn)
            }
        };
        let role_arn = given_role_stating(
            &router,
            &format!("{}-role", row.case),
            SERVICE_PRINCIPAL,
            (row.policy)(&source_arn, &target_arn),
        )
        .await;
        let created = call(
            &router,
            "POST",
            &format!("/v1/pipes/{}", row.case),
            Some(pipe_body(&source_arn, &target_arn, &role_arn)),
            REGION,
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {:?}: CreatePipe answered {} {}",
            row.case, created.status, created.body
        );
        let marker = format!("{}-record", row.case);
        send(&router, &source_url, std::slice::from_ref(&marker)).await;
        prepared.push(Prepared {
            row,
            source_url,
            target_url,
            marker,
        });
    }

    // Both pipes of a pair are RUNNING on the control plane: what tells them apart is the
    // counter and not their reported state.
    for entry in &prepared {
        let reported = crate::pipe_execution_role::wait_for_state(
            &router,
            entry.row.case,
            service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING,
            BUDGET,
        )
        .await;
        assert_eq!(
            reported,
            service_com_amazonaws_pipes_pipes::state::pipe_state::RUNNING,
            "case {:?}: expected the pipe RUNNING, it reports {reported}",
            entry.row.case
        );
    }

    // What a client sees where the record was meant to end up: a queue target holding it,
    // a function target having consumed it off the source — or, for a refused delivery,
    // the record still held by the source and the target queue empty.
    for entry in &prepared {
        let case = entry.row.case;
        match (entry.row.delivered, &entry.target_url) {
            (true, Some(url)) => {
                let delivered = collect_at_least(&router, url, 1, BUDGET).await;
                assert_eq!(
                    delivered.len(),
                    1,
                    "case {case:?}: expected the target queue to hold the one record this pipe \
                     moved, got {delivered:?}"
                );
                // What a queue target is handed is the record envelope, so the row's own
                // marker is the body INSIDE it.
                assert!(
                    delivered[0].contains(&entry.marker),
                    "case {case:?}: expected the delivered record to carry this pipe's own \
                     marker {:?}, got {:?}",
                    entry.marker,
                    delivered[0]
                );
            }
            (true, None) => {
                let held =
                    wait_for_counted(&router, &entry.source_url, HELD_ATTRIBUTES, 0, BUDGET).await;
                assert_eq!(
                    held, 0,
                    "case {case:?}: expected the source drained once the invocation took its \
                     record, it still holds {held}"
                );
            }
            (false, target) => {
                let failed =
                    wait_for_sum(&router, case, TARGET_STAGE_FAILED_METRIC, 1.0).await;
                assert!(
                    failed >= 1.0,
                    "case {case:?}: expected the refused target stage counted, got {failed}"
                );
                let held =
                    wait_for_counted(&router, &entry.source_url, HELD_ATTRIBUTES, 1, BUDGET).await;
                assert_eq!(
                    held, 1,
                    "case {case:?}: expected the record a refused delivery left on the source \
                     still held there, the source holds {held}"
                );
                if let Some(url) = target {
                    let delivered = collect_during(&router, url, QUIET).await;
                    assert!(
                        delivered.is_empty(),
                        "case {case:?}: expected the target of a refused delivery to hold \
                         nothing, got {delivered:?}"
                    );
                }
            }
        }
    }

    // Every pipe that had a refused delivery first, so each "reads 0" below is asserted at
    // an instant when other pipes of the same account have already accumulated that very
    // counter.
    for entry in &prepared {
        if entry.row.delivered {
            continue;
        }
        let sum = wait_for_sum(&router, entry.row.case, TARGET_STAGE_FAILED_METRIC, 1.0).await;
        assert!(
            sum >= 1.0,
            "case {:?}: expected {TARGET_STAGE_FAILED_METRIC} to read at least 1 for a pipe \
             whose target call its role bars, got {sum}",
            entry.row.case
        );
    }

    // And the pipes that delivered: the SAME counter exists and reads exactly 0 — the
    // documented zero that tells a healthy pipe from one that never ran — while their own
    // execution is counted.
    for entry in &prepared {
        if !entry.row.delivered {
            continue;
        }
        let case = entry.row.case;
        let events = wait_for_sum(&router, case, EVENT_COUNT_METRIC, 1.0).await;
        assert!(
            events >= 1.0,
            "case {case:?}: expected the events this pipe moved counted, got {events}"
        );
        let points = wait_for_series(&router, case, TARGET_STAGE_FAILED_METRIC).await;
        assert!(
            points > 0,
            "case {case:?}: expected {TARGET_STAGE_FAILED_METRIC} of a healthy pipe to EXIST \
             rather than be absent, it has no datapoint"
        );
        // And it HOLDS at 0 for the whole window, while the pipes whose target calls are
        // refused go on accumulating theirs.
        let deadline = Instant::now() + QUIET;
        loop {
            let (points, sum) = series(&router, case, TARGET_STAGE_FAILED_METRIC).await;
            assert!(
                points > 0 && sum == 0.0,
                "case {case:?}: expected {TARGET_STAGE_FAILED_METRIC} of a healthy pipe to \
                 hold at 0, got {sum} over {points} datapoint(s)"
            );
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(LOOK_AGAIN).await;
        }
    }

    // Every datapoint of the counter, for every pipe of the table, carries the unit the
    // namespace publishes in and the one dimension it is attributed by.
    for entry in &prepared {
        let case = entry.row.case;
        let points = datapoints(&router, case, TARGET_STAGE_FAILED_METRIC).await;
        assert!(
            !points.is_empty(),
            "case {case:?}: expected {TARGET_STAGE_FAILED_METRIC} of a running pipe to have a \
             datapoint, it had none"
        );
        for point in &points {
            assert_eq!(
                point["Unit"],
                json!(COUNTER_UNIT),
                "case {case:?}: expected {TARGET_STAGE_FAILED_METRIC} published in \
                 {COUNTER_UNIT}, got {point}"
            );
        }

        // The narrowed catalogue read a dashboard asks for one pipe with: exactly this
        // pipe's series, carrying the one `PipeName` dimension and no other.
        let catalogue = catalogued(&router, case, TARGET_STAGE_FAILED_METRIC).await;
        assert_eq!(
            catalogue,
            vec![json!({
                "Namespace": METRIC_NAMESPACE,
                "MetricName": TARGET_STAGE_FAILED_METRIC,
                "Dimensions": [{ "Name": PIPE_NAME_DIMENSION, "Value": case }],
            })],
            "case {case:?}: expected ListMetrics narrowed to this pipe to catalogue its one \
             {TARGET_STAGE_FAILED_METRIC} series under the {PIPE_NAME_DIMENSION} dimension \
             alone"
        );
    }
}
