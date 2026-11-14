// SPDX-License-Identifier: CC0-1.0

//! The X-RAY TRACE an execution publishes about itself, and the one record that decides
//! whether it publishes one at all.
//!
//! # The rule this module implements
//!
//! A state machine's `tracingConfiguration` "selects whether or not the state machine's
//! X-Ray tracing is enabled"
//! (`smithy_query(query="shape",shape="TracingConfiguration")`), and that is the whole of
//! what decides it: an execution of a state machine whose configuration is ENABLED is
//! recorded at X-Ray — it belongs to the chain the caller's trace header named, or to one
//! started for it where the caller named none, and the work it did is submitted as segment
//! documents, so the execution's trace id is listed by `GetTraceSummaries` over the window
//! it ran in and its segments are returned by `BatchGetTraces` and `GetTraceGraph`. An
//! execution of a state machine whose configuration is DISABLED is recorded nowhere: no
//! trace of it is retrievable, whatever header the start carried.
//!
//! Two things beside the configuration belong to the trace rather than to the execution:
//!
//! * the SAMPLING DECISION is the caller's where the header took one — `Sampled=0` is a
//!   run that happens and whose trace is not recorded — and the state machine's own where
//!   it did not, a state machine that is tracing recording the runs it is asked about;
//! * the CHAIN is the caller's `Root` where the header named one in X-Ray's own form, and
//!   one minted for the run where it named none or named text that is not a trace id, so a
//!   malformed header never changes what the run does.
//!
//! Both readings are `service_common`'s one reading of a tracing header
//! ([`::service_common::xray_trace`]) rather than this service's own, because every hop of
//! a request chain takes them the same way, and the submission is the one in-process X-Ray
//! client an emulated service reports its own work through
//! ([`::service_common::xray_segments`]).
//!
//! What the configuration asks for is PINNED to the execution: an update to the state
//! machine "will not affect currently running executions", so the configuration this reads
//! is the snapshot the start recorded ([`super::RanConfiguration::tracing_configuration`])
//! and not what the state machine holds by the time the run ends.
//!
//! # When it is published
//!
//! A segment reports work that HAPPENED, so a run's own segment is recorded once that run
//! has reached a terminal status and the instants it began and ended are both facts. Like
//! the status changes beside it ([`super::events`]), the recording is a DIFF rather than a
//! second state machine: [`record_execution_traces`] queues a segment for every stored
//! execution whose trace has not gone out for the attempt it has now finished, whichever
//! call or clock tick materialised that outcome — so a redriven run, which "append[s]
//! their event history to the existing event history" and reaches a further terminal
//! status, records the attempt that redrive ran too, and an execution nobody polls has its
//! trace published by the clock that settles it ([`super::events::publish`]).
//!
//! The queue is drained with no lock held ([`publish`]) and is BOUNDED
//! ([`MAX_PENDING_SEGMENTS`], oldest dropped first), and the record of what has gone out is
//! pruned to the executions the store still holds, so neither grows with the number of
//! calls served. Publication is best-effort: an emulator with no X-Ray mounted has nowhere
//! to publish, so a failure is logged and the operation's own result stands.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::SystemTime;

use ::serde_json::{json, Value};

use super::ComAmazonawsSfnAwsstepfunctionsState;

/// The `origin` the segment of an execution carries: the resource type of the state
/// machine that ran it, as X-Ray names a Step Functions state machine.
pub const STATE_MACHINE_ORIGIN: &str = "AWS::StepFunctions::StateMachine";

/// The most segment documents waiting to be submitted at once, oldest dropped first.
///
/// Emulator resource guard, NOT an AWS quota: every terminal execution appends one, so the
/// queue is capped for the same reason the status-change queue is
/// ([`super::events::MAX_PENDING_EVENTS`]).
pub const MAX_PENDING_SEGMENTS: usize = 1_024;

/// The most documents submitted in one `PutTraceSegments` call.
///
/// X-Ray's own handler bounds a batch, so a drain is split into calls it accepts rather
/// than into one request the service would refuse whole.
const MAX_SEGMENTS_PER_CALL: usize = 50;

/// The most of a run's states one segment reports a subsegment for.
///
/// Emulator resource guard, NOT an AWS quota: a definition's loop can enter states without
/// end, and the subsegments of one document are bounded by X-Ray's own ingestion, so the
/// states reported are the first this many the run entered.
pub const MAX_REPORTED_STATES: usize = 100;

/// One segment document waiting to be submitted, and the Region it is submitted in.
#[derive(Debug, Clone)]
pub struct Segment {
    /// The Region the execution ran in, which is where its trace is held.
    pub region: String,
    /// The document itself, as `PutTraceSegments` takes it.
    pub document: String,
}

/// Queue the segment of every stored execution whose trace has not gone out for the attempt
/// it has now finished, and take that attempt to be the recorded one.
///
/// This is the whole of the "which executions owe a trace" rule: a run is recorded when the
/// configuration it ran under enables tracing and the sampling decision says the chain is
/// recorded, and it is recorded ONCE per attempt — the attempt being counted by the
/// redrives that have re-run it, so each re-run records its own work and a run that has not
/// moved records nothing however many times this runs.
///
/// Called by [`ComAmazonawsSfnAwsstepfunctionsState::settle`], which every operation runs,
/// so no transition site has to announce itself.
pub(super) fn record_execution_traces(store: &mut ComAmazonawsSfnAwsstepfunctionsState) {
    let mut queued: Vec<(String, i32, Segment)> = Vec::new();
    for (arn, execution) in &store.executions {
        if !execution.is_terminal() {
            continue;
        }
        if store
            .recorded_execution_traces
            .get(arn)
            .is_some_and(|recorded| *recorded == execution.redrive_count)
        {
            continue;
        }
        let Some(segment) = execution_segment(execution) else {
            continue;
        };
        queued.push((arn.clone(), execution.redrive_count, segment));
    }
    for (arn, attempt, segment) in queued {
        store.recorded_execution_traces.insert(arn, attempt);
        enqueue(&mut store.pending_trace_segments, segment);
    }
    // What has gone out for an execution the store no longer holds can never be owed again.
    let executions = &store.executions;
    store
        .recorded_execution_traces
        .retain(|arn, _| executions.contains_key(arn));
}

/// Append `segment` to the queue, dropping the oldest once the queue is full.
fn enqueue(pending: &mut VecDeque<Segment>, segment: Segment) {
    while pending.len() >= MAX_PENDING_SEGMENTS {
        pending.pop_front();
        ::tracing::warn!(
            "the queue of execution trace segments waiting to be submitted to X-Ray is full \
             at {MAX_PENDING_SEGMENTS}; the oldest segment was dropped"
        );
    }
    pending.push_back(segment);
}

/// Whether the configuration `execution` RAN records it at X-Ray, and the chain it belongs
/// to — `None` for a run that is recorded nowhere.
///
/// Both halves of the decision live here, because neither is worth anything without the
/// other: tracing is enabled by the state machine's own record, and the sampling decision
/// the caller's header took says whether this particular chain is being recorded.
fn recorded_chain(execution: &super::Execution) -> Option<String> {
    let enabled =
        super::reported_tracing_configuration(execution.ran.tracing_configuration.as_ref())
            .enabled;
    if !enabled {
        return None;
    }
    let header = execution.trace_header.as_deref();
    let sampled = header
        .and_then(::service_common::xray_trace::sampling_decision)
        .unwrap_or(true);
    if !sampled {
        return None;
    }
    Some(
        header
            .and_then(::service_common::xray_trace::continued_root)
            .map(str::to_string)
            // A run whose caller named no chain is the beginning of one, dated at the
            // instant the run started — which is the instant a trace id carries.
            .unwrap_or_else(|| ::service_common::xray_trace::mint_trace_id(execution.start_date)),
    )
}

/// The segment document the finished `execution` records, or `None` where it records none.
///
/// Every value is read off the execution's own record: the chain its header named, the
/// state machine that ran it, the instants it began and ended, the outcome it reached and
/// the states its history says it entered. The run's work is a CHILD of the caller's span
/// where the caller named one, and the root of the trace where it did not.
fn execution_segment(execution: &super::Execution) -> Option<Segment> {
    let root = recorded_chain(execution)?;
    let started = ::service_common::xray_segments::epoch_seconds(execution.start_date);
    let ended = ::service_common::xray_segments::epoch_seconds(
        execution.stop_date.unwrap_or(execution.start_date),
    );
    let outcome = outcome_flags(&execution.status);
    let mut document = json!({
        "trace_id": root,
        "id": ::service_common::xray_trace::mint_span_id(),
        // "the name of the service that handled the request" (`PutTraceSegments`), which
        // for a workflow is the state machine that ran it.
        "name": state_machine_name(&execution.state_machine_arn),
        "start_time": started,
        "end_time": ended,
        "origin": STATE_MACHINE_ORIGIN,
        "resource_arn": execution.state_machine_arn,
        "error": outcome.error,
        "throttle": outcome.throttle,
        "fault": outcome.fault,
        // What the trace is about, as the annotations a reader filters a trace by: the
        // execution this segment reports and the status it reached.
        "annotations": {
            "execution_arn": execution.arn,
            "status": execution.status,
        },
    });
    if let Some(parent) = execution
        .trace_header
        .as_deref()
        .and_then(::service_common::xray_trace::parent_span)
    {
        insert(&mut document, "parent_id", json!(parent));
    }
    let subsegments = state_subsegments(execution);
    if !subsegments.is_empty() {
        insert(&mut document, "subsegments", Value::Array(subsegments));
    }
    Some(Segment {
        region: super::arn_region(&execution.arn),
        document: document.to_string(),
    })
}

/// One subsegment per STATE the run entered, in the order its history entered them: the
/// span each state's own work occupied, which is what tells a reader which states
/// contributed to the trace.
///
/// A state's span is the instant its `<Type>StateEntered` event was written to the instant
/// its `<Type>StateExited` one was; a state the run never left — the state a failed walk
/// stopped at — ran until the execution itself ended.
fn state_subsegments(execution: &super::Execution) -> Vec<Value> {
    let ended = execution.stop_date.unwrap_or(execution.start_date);
    let mut entered: Vec<(String, SystemTime, Option<SystemTime>)> = Vec::new();
    for event in &execution.history {
        let Some(name) = event.state_name.as_deref() else {
            continue;
        };
        if event.event_type.ends_with(super::STATE_ENTERED_SUFFIX) {
            // Past the cap the entry is not reported, and the walk CONTINUES: the states
            // that are reported keep the spans their own exits give them rather than
            // falling back to the instant the run ended.
            if entered.len() < MAX_REPORTED_STATES {
                entered.push((name.to_string(), event.timestamp, None));
            }
            continue;
        }
        if event.event_type.ends_with(super::STATE_EXITED_SUFFIX) {
            // The exit belongs to the LAST entry of that state still open, so a state a
            // loop re-entered reports one span per pass rather than one span widened.
            if let Some(open) = entered
                .iter_mut()
                .rev()
                .find(|(state, _, exited)| state == name && exited.is_none())
            {
                open.2 = Some(event.timestamp);
            }
        }
    }
    entered
        .into_iter()
        .map(|(name, from, to)| {
            json!({
                "id": ::service_common::xray_trace::mint_span_id(),
                "name": name,
                "start_time": ::service_common::xray_segments::epoch_seconds(from),
                "end_time": ::service_common::xray_segments::epoch_seconds(to.unwrap_or(ended)),
            })
        })
        .collect()
}

/// The three OUTCOME flags a segment document carries, as X-Ray documents each: "`error` —
/// boolean indicating that a client error occurred", "`throttle` — boolean indicating that
/// a request was throttled" and "`fault` — boolean indicating that a server error
/// occurred".
///
/// A workflow's outcome is a status rather than an HTTP status, and the two kinds of
/// unsuccessful run differ in WHOSE fault they are: a run the service ended — one that
/// failed or ran out of time — is a fault of the work the segment covers, while a run the
/// CLIENT stopped is the client's own doing and so the error of the request rather than a
/// fault of the service. A run that succeeded carries none of the three.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Outcome {
    error: bool,
    throttle: bool,
    fault: bool,
}

/// The flags `status` gives, per the documentation on [`Outcome`].
fn outcome_flags(status: &str) -> Outcome {
    Outcome {
        error: status == super::EXECUTION_STATUS_ABORTED,
        // Nothing a workflow reaches is a throttle: the statuses are the terminal outcomes
        // of the run, and a request this service refused for capacity never started one.
        throttle: false,
        fault: status == super::EXECUTION_STATUS_FAILED
            || status == super::EXECUTION_STATUS_TIMED_OUT,
    }
}

/// The state machine's NAME, out of its ARN: the resource field of
/// `arn:<partition>:states:<region>:<account>:stateMachine:<name>`.
///
/// The name is what the segment reports as the service that handled the run, and an ARN
/// this store minted always carries one; text that carries no field at all is reported as
/// written, because a segment names the service as well as it can rather than refusing to
/// report work that happened.
fn state_machine_name(state_machine_arn: &str) -> &str {
    state_machine_arn
        .rsplit(':')
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(state_machine_arn)
}

/// Put `value` on `document`, which is the object `json!` built above.
fn insert(document: &mut Value, member: &str, value: Value) {
    if let Some(object) = document.as_object_mut() {
        object.insert(member.to_string(), value);
    }
}

/// Submit every segment this store's executions have queued, and answer how many went out.
///
/// Called with NO store of this service locked, by every operation whose own settle can
/// materialise an execution's outcome and by the clock alike ([`super::events::publish`]):
/// the queue is taken out in one critical section, so nothing is submitted twice, and the
/// in-process `PutTraceSegments` runs outside it.
pub async fn publish<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> usize {
    let Some(state) = super::service_state(&**engine) else {
        return 0;
    };
    let queued: Vec<Segment> = {
        let mut store = state.lock();
        store.pending_trace_segments.drain(..).collect()
    };
    if queued.is_empty() {
        return 0;
    }
    let submitted = queued.len();
    // One submission per Region: a trace belongs to the Region its segments were recorded
    // in, which is the Region the execution ran in.
    let mut by_region: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for segment in queued {
        by_region
            .entry(segment.region)
            .or_default()
            .push(segment.document);
    }
    for (region, documents) in by_region {
        for batch in documents.chunks(MAX_SEGMENTS_PER_CALL) {
            ::service_common::xray_segments::report_segments_through(
                callback,
                &region,
                batch.to_vec(),
            )
            .await;
        }
    }
    submitted
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, UNIX_EPOCH};

    use super::{outcome_flags, state_machine_name, Outcome};

    /// The outcome flags a terminal status gives — over the whole set of statuses an
    /// execution reaches, because the rule is about which party an unsuccessful run is
    /// down to and not about one of them.
    #[test]
    fn the_outcome_flags_follow_whose_doing_the_status_is() {
        for (case, status, expected) in [
            (
                "a run that succeeded",
                super::super::EXECUTION_STATUS_SUCCEEDED,
                Outcome {
                    error: false,
                    throttle: false,
                    fault: false,
                },
            ),
            (
                "a run the service failed",
                super::super::EXECUTION_STATUS_FAILED,
                Outcome {
                    error: false,
                    throttle: false,
                    fault: true,
                },
            ),
            (
                "a run that ran out of time",
                super::super::EXECUTION_STATUS_TIMED_OUT,
                Outcome {
                    error: false,
                    throttle: false,
                    fault: true,
                },
            ),
            (
                "a run the client stopped",
                super::super::EXECUTION_STATUS_ABORTED,
                Outcome {
                    error: true,
                    throttle: false,
                    fault: false,
                },
            ),
            (
                "a run that is still going",
                super::super::EXECUTION_STATUS_RUNNING,
                Outcome {
                    error: false,
                    throttle: false,
                    fault: false,
                },
            ),
        ] {
            let got = outcome_flags(status);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?} for {status:?}, got {got:?}"
            );
        }
    }

    /// The service a segment names, out of however the state machine is addressed.
    #[test]
    fn the_segment_names_the_state_machine_its_arn_names() {
        for (case, arn, expected) in [
            (
                "a state machine ARN this store mints",
                "arn:aws:states:us-east-1:000000000000:stateMachine:traced",
                "traced",
            ),
            ("text that is no ARN at all", "traced", "traced"),
            ("text ending in the separator", "stateMachine:", "stateMachine:"),
        ] {
            let got = state_machine_name(arn);
            assert_eq!(
                got, expected,
                "case {case:?}: expected {expected:?} from {arn:?}, got {got:?}"
            );
        }
    }

    /// A trace id minted for a run carries the instant that run started, which is the
    /// field a reader of a trace id reads its time off.
    #[test]
    fn a_minted_chain_is_dated_at_the_instant_the_run_started() {
        let started = UNIX_EPOCH + Duration::from_secs(0x5759_e988);
        let minted = ::service_common::xray_trace::mint_trace_id(started);
        assert!(
            ::service_common::xray_trace::is_trace_id(&minted),
            "expected a trace id, got {minted:?}"
        );
        assert!(
            minted.starts_with("1-5759e988-"),
            "expected the run's own start instant in {minted:?}"
        );
    }
}
