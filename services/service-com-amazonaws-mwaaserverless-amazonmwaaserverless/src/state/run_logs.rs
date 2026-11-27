// SPDX-License-Identifier: CC0-1.0

//! Where a run's own execution output GOES: the CloudWatch Logs log group the workflow's
//! `LoggingConfiguration` names.
//!
//! A workflow that configures logging asks this service to export what its runs produce —
//! "Logs are automatically exported to your specified CloudWatch log group using remote
//! logging functionality" over output that "capture[s] workflow execution details,
//! task-level information, and system events" (`smithy_query(query="doc",
//! shape="LoggingConfiguration")`), into the group `LogGroupName` names ("The name of the
//! CloudWatch log group where workflow execution logs are stored"). So a client that
//! configures a group and then asks CloudWatch Logs for it (`DescribeLogGroups`,
//! `DescribeLogStreams`, `GetLogEvents`) has to FIND the run's output there, and the stream
//! each task instance wrote to is the one `GetTaskInstance` reports as `LogStream` ("The
//! CloudWatch log stream name for this task instance execution") — a reported stream name
//! that names nothing would be a resource no reader can reach.
//!
//! What is exported is what this emulator's own orchestration PRODUCED and the store holds:
//! the run's admission and each status the clock moved it to, and, per task instance, the
//! attempt starting and the outcome it ended with ([`crate::state::WorkflowRun::settle`]).
//! Nothing else is invented: what a task's operator does to the resources it names is the
//! operator's own work against another service, which this emulator does not perform, so no
//! output is written for it.
//!
//! ## How it travels
//!
//! A line is QUEUED on the run it belongs to at the instant the transition that produced it
//! is materialised ([`crate::state::WorkflowRun::pending_log_lines`]) and DELIVERED by the
//! call that materialised it, once that call has released this service's store
//! ([`publish`]) — so the two stores are never locked at once and the work that produces a
//! line stays a pure store transition. A line a store write queued with no handler of its
//! own to deliver it — the admission of a run a workflow's trigger fired — stays on that run
//! until the next call that settles it takes it, so it is delivered rather than lost. The
//! queue is bounded ([`MAX_PENDING_LOG_LINES`]), which is what holds the wait between the
//! write that produces a line and the call that takes it to a fixed size per run.
//!
//! An engine with no CloudWatch Logs store mounted has nowhere for a line to go, so the
//! call that takes it drops it ([`publish`]) rather than keeping it for a reader that cannot
//! exist: nothing accumulates there either.
//!
//! The events are written into the CloudWatch Logs service's OWN store in the SAME shared
//! engine, through that service's own record constructors
//! ([`logs::LogGroupRecord::new`], [`logs::LogStreamRecord::new`]) and under its own
//! ceilings, so what this service exported is what CloudWatch Logs' own readers report.
//! They do not travel over `logs:PutLogEvents`: that API refuses a group or a stream the
//! account does not hold yet, while the remote-logging export this service documents is
//! what CREATES the group and the streams it writes to.

use ::std::collections::BTreeMap;
use ::std::time::{SystemTime, UNIX_EPOCH};

use ::service_com_amazonaws_cloudwatchlogs_logs_20140328::state as logs;

use super::workflow_definition::MAX_DECLARED_TASKS;

/// The CloudWatch Logs store a run's exported output lives in.
///
/// It is the CloudWatch Logs service's OWN state struct, reached through the SAME shared
/// engine this service's store lives in, so a group, a stream or an event written here is
/// one that service's own readers report.
pub type LogsState =
    ::service_com_amazonaws_cloudwatchlogs_logs_20140328::ComAmazonawsCloudwatchlogsLogs20140328State;

/// How many lines ONE run keeps queued for delivery.
///
/// A run's whole execution produces at most two lines per task instance its definition
/// declared — the attempt starting and the outcome it ended with — plus the handful its own
/// lifecycle produces, so this holds every line a run can owe and no line of a run is ever
/// dropped. It is still a ceiling, because a line queued by a store write waits on the run
/// until a call takes it (the admission of a trigger-fired run waits for the next call that
/// settles that run), and this is what bounds that wait to a fixed size per run.
pub const MAX_PENDING_LOG_LINES: usize = 2 * MAX_DECLARED_TASKS + 8;

/// ONE line a run owes to the log group its workflow configured: the stream it belongs in,
/// the instant it describes and the text of it.
#[derive(Debug, Clone)]
pub struct LogLine {
    /// The log group the workflow's `LoggingConfiguration` named.
    pub log_group: String,
    /// The stream inside that group ([`run_log_stream`], [`task_log_stream`]).
    pub log_stream: String,
    /// The instant the line describes: the DUE instant of the transition that produced it,
    /// never the instant it was delivered, so two readers of one transition deliver the
    /// same event.
    pub at: SystemTime,
    /// The text of the line.
    pub message: String,
}

/// The stream a RUN's own lifecycle output goes to: the run's identifier, so every stream
/// of one run shares that prefix and a reader asking CloudWatch Logs for the run's output
/// (`DescribeLogStreams`'s `logStreamNamePrefix`) finds the run's own lines beside its
/// tasks'.
#[must_use]
pub fn run_log_stream(run_id: &str) -> String {
    run_id.to_string()
}

/// The stream ONE task instance's output goes to: the run it ran in, the instance's own
/// identifier and the attempt, so two attempts of one task and two runs of one task never
/// interleave in a stream and the stream name alone says which execution it holds.
///
/// The composed name is built from values that already satisfy the modelled `IdString`
/// character class (the run id this service minted and the identifier the run minted for the
/// instance), so only its LENGTH can take it outside what `LogStream` reports — that member
/// targets the same modelled `IdString` (`smithy_query(query="shape",
/// shape="GetTaskInstanceResponse")`). A name past it is therefore CUT to fit, through the
/// one helper that holds a composed identifier of this service to the modelled length
/// ([`crate::state::id_within_modelled_length`]): the reported name stays inside the
/// modelled length and still names one task instance of one run uniquely.
#[must_use]
pub fn task_log_stream(run_id: &str, task_instance_id: &str, attempt: i32) -> String {
    super::id_within_modelled_length(format!("{run_id}.{task_instance_id}.attempt-{attempt}"))
}

/// `at` in milliseconds since the Unix epoch, which is what every CloudWatch Logs timestamp
/// is expressed in (`OutputLogEvent$timestamp`). An instant before the epoch, which no
/// clock reports, reads as the epoch itself.
fn epoch_millis(at: SystemTime) -> i64 {
    at.duration_since(UNIX_EPOCH)
        .map(|since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or_default()
}

/// Deliver `lines` into the CloudWatch Logs store of the SAME engine — the store
/// `DescribeLogStreams`, `GetLogEvents` and `FilterLogEvents` answer from — stamping the
/// groups and streams it has to open for them in `region`.
///
/// Call it with NO store of this service locked: it takes the destination's guard only.
/// An engine carrying no CloudWatch Logs store has nowhere to export to, so the lines are
/// dropped rather than kept for a reader that cannot exist.
///
/// The group a configuration names is CREATED when the account does not hold it yet,
/// because the configuration says where the run's logs go. A group the account ALREADY
/// holds is left exactly as it is — its retention, its tags and the events already in it
/// belong to the account — and so is one this emulator has no room for, which takes no
/// delivery rather than displacing another. Every factor a caller can grow is admitted
/// against the OWNING service's own ceiling before anything is written: how many groups
/// that store holds ([`logs::MAX_LOG_GROUPS`]), how many streams one group holds
/// ([`logs::MAX_STREAMS_PER_GROUP`], the ceiling its own `CreateLogStream` admits one
/// against) and how much one stream retains (its own `enforce_storage_ceiling`).
pub fn publish<Eng: Send + Sync + 'static>(engine: &Eng, region: &str, lines: Vec<LogLine>) {
    if lines.is_empty() {
        return;
    }
    let Some(state) = logs::service_state(engine) else {
        return;
    };
    // One batch per destination stream, each keeping the order the lines were produced in,
    // so the destination's guard is taken once per stream rather than once per line.
    let mut batches: BTreeMap<(String, String), Vec<(i64, String)>> = BTreeMap::new();
    for line in lines {
        batches
            .entry((line.log_group, line.log_stream))
            .or_default()
            .push((epoch_millis(line.at), line.message));
    }

    let mut store = state.lock();
    let ingested_at = logs::now_ms();
    for ((log_group, log_stream), events) in batches {
        if !store.log_groups.contains_key(&log_group)
            && store.log_groups.len() >= logs::MAX_LOG_GROUPS
        {
            ::tracing::warn!(
                %log_group,
                "the CloudWatch Logs store is at capacity; a workflow run's log export was \
                 not delivered"
            );
            continue;
        }
        // The data identifiers the data protection policies in force for the group name: such
        // a policy masks what is ingested into the group, by whichever path the events
        // arrive.
        let data_identifiers = store
            .log_groups
            .get(&log_group)
            .map(|group| store.data_protection_identifiers_for(group))
            .unwrap_or_default();
        // The customer managed key the destination group is associated with, under which the
        // data it takes in is encrypted — by whichever path the events arrive.
        let ingest_kms_key_id = store
            .log_groups
            .get(&log_group)
            .and_then(|group| group.kms_key_id.clone());
        // The transformer in force for the destination group transforms what that group takes
        // in — by whichever path the events arrive — so an exported event is stored with the
        // fields that pipeline produced from it, like any other ingested event.
        let transformer = store
            .log_groups
            .get(&log_group)
            .map(|group| logs::active_transformer::pipeline(&store, group))
            .unwrap_or_default();
        let events: Vec<logs::StoredEvent> = events
            .into_iter()
            .map(|(timestamp, message)| logs::StoredEvent {
                timestamp,
                masked_message: logs::data_protection::masked(&message, &data_identifiers),
                transformed_message: transformer.transform(&message),
                message,
                ingestion_time: ingested_at,
                ingestion_seq: store.take_ingestion_seq(),
                ingest_kms_key_id: ingest_kms_key_id.clone(),
            })
            .collect();
        let group = store
            .log_groups
            .entry(log_group.clone())
            .or_insert_with(|| logs::LogGroupRecord::new(region, &log_group, ingested_at));
        // A stream is opened per run and per task instance, so how many of them one group
        // holds grows with the runs a caller starts: it is admitted against the same
        // per-group ceiling the owning service's own `CreateLogStream` admits one against,
        // and a group already holding its full complement takes no further delivery rather
        // than displacing a stream the account holds.
        if !group.streams.contains_key(&log_stream)
            && group.streams.len() >= logs::MAX_STREAMS_PER_GROUP
        {
            ::tracing::warn!(
                %log_group,
                %log_stream,
                "the log group already holds the maximum number of streams; a workflow run's \
                 log export was not delivered"
            );
            continue;
        }
        let stream = group
            .streams
            .entry(log_stream.clone())
            .or_insert_with(|| {
                logs::LogStreamRecord::new(region, &log_group, &log_stream, ingested_at)
            });
        stream.events.extend(events);
        // The stream is kept in `(timestamp, ingestion_seq)` order, which is the order its
        // reader pages it in; a run's lines are produced in that order already, so only an
        // export that predates events already there needs the sort.
        stream.events.sort_by_key(logs::StoredEvent::key);
        stream.enforce_storage_ceiling();
        stream.refresh_event_times();
        stream.last_ingestion_time = Some(ingested_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: a task instance's stream name is composed of the run, the instance and the
    /// attempt, and a composed name past what the modelled `LogStream` can carry is cut to
    /// fit while still naming one execution uniquely.
    #[test]
    fn a_task_stream_name_fits_the_modelled_member() {
        // (case, task instance id, attempt)
        let rows = [
            ("short", "extract", 1),
            ("punctuated", "extract.data_task-01", 3),
            (
                "longest the member admits",
                &"t".repeat(crate::state::MAX_ID_STRING_CHARS)[..],
                1,
            ),
            ("every attempt digit", "load", i32::MAX),
        ];
        let run_id = "run-0123456789abcdef";
        let mut seen: Vec<String> = Vec::new();
        for (case, task_instance_id, attempt) in rows {
            let name = task_log_stream(run_id, task_instance_id, attempt);
            let length = name.chars().count();
            assert!(
                (1..=crate::state::MAX_ID_STRING_CHARS).contains(&length),
                "case {case:?}: expected a name of 1..={} characters, got {length} in {name:?}",
                crate::state::MAX_ID_STRING_CHARS
            );
            assert!(
                name.starts_with(run_id),
                "case {case:?}: expected the name to name the run {run_id:?}, got {name:?}"
            );
            assert!(
                !seen.contains(&name),
                "case {case:?}: expected a name no other row produced, got {name:?}"
            );
            seen.push(name);
        }
    }
}
