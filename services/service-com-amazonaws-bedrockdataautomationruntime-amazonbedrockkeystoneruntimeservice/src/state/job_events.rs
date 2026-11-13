// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events a data-automation job publishes as it moves.
//!
//! # The rule this module implements
//!
//! A submission may ask to be NOTIFIED of its job instead of polling for it
//! (`notificationConfiguration.eventBridgeConfiguration.eventBridgeEnabled`). For such a
//! job every lifecycle transition is published onto the account's default event bus
//! ([`DEFAULT_EVENT_BUS`]) under the `source` the publishing service owns
//! ([`EVENT_SOURCE`]) and the `detail-type` of that transition
//! ([`JobEventKind::detail_type`]), carrying the job's own id in the event's `detail`. A
//! rule on that bus whose `EventPattern` matches then delivers it to the rule's targets,
//! so a client driven by the event observes the job without calling this service again.
//! Accepting the flag and publishing nothing is the one thing that must not happen: it
//! leaves such a client waiting forever on an event that was configured and never sent.
//!
//! A submission that asked for nothing publishes nothing, so the flag is read off the
//! stored record and nowhere else ([`crate::state::Invocation::event_bridge_enabled`]).
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a job moves: the clock does, and the ONE place that
//! materialises a transition queues the event for it
//! ([`crate::state::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::settle_invocation`]),
//! so a transition is queued exactly once however it was materialised and the queue is
//! bounded ([`MAX_PENDING_EVENTS`], oldest dropped first).
//!
//! A queued event is published with NO store lock held ([`publish_pending`]): EventBridge
//! is reached the way every emulated service reaches another, through the framework's
//! in-process callback, so this crate names no other service's crate. Publication is BEST
//! EFFORT — an emulator with no EventBridge mounted has nowhere to publish, and AWS
//! documents this delivery as best effort too — so a failure is logged and the operation's
//! own answer stands.
//!
//! WHICH call publishes: the submission itself publishes the job's creation, and then
//! whatever settles the terminal transition publishes its outcome — the status read that
//! finds it due, or, for a job whose submission asked to be notified,
//! [`settle_when_due`], the settle that submission started so a client that never polls
//! still gets its event.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::{AutomationJobStatus, Invocation};

/// The `source` of every event this service publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// data automation publishes under the Amazon Bedrock source, which is the namespace this
/// service's own ARNs carry (`crate::state`'s `ARN_NAMESPACE`).
pub const EVENT_SOURCE: &str = "aws.bedrock";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How many job events the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition of every notified job appends to it, so without a bound a client submitting
/// without end could grow it without end. The oldest event is dropped first, so the
/// transitions the live jobs just made are the ones that survive. It is far above what a
/// realistic burst produces (a job contributes two events).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more events than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many events ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one call draining all of it
/// would fan out to a rule's targets that many times: this caps one drain, and
/// [`publish_pending`] repeats a drain until the queue is empty, so nothing waits for a
/// later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// How long after the terminal transition fell due [`settle_when_due`] settles the job.
///
/// The settle must not run BEFORE the transition is due — a job is not finished until the
/// clock says so — so it waits for the due instant and this much more, which absorbs the
/// coarseness of a timer without making a notified job's outcome arrive later than a
/// polling caller's.
const SETTLE_SLACK: Duration = Duration::from_millis(50);

/// How many settles started by a submission may be in flight at once.
///
/// This module's OWN ceiling: each notified submission starts one, so without a bound a
/// stream of submissions could have unbounded work outstanding. Each settle is one timer,
/// one object-store read and one publication, and it releases its slot however it ends, so
/// this is a cap on concurrency rather than on how many jobs may be notified. A submission
/// that finds every slot taken starts nothing and loses nothing: its job's outcome is
/// settled and published by the next read that asks for it.
pub const MAX_SETTLES_IN_FLIGHT: usize = 128;

/// Which transition of a job's lifecycle an event reports.
///
/// FOUR kinds, because four are what this service's events are published under: the
/// behavioural corpus's own rule for this channel matches a job's lifecycle on exactly
/// this set of `detail-type` values, so a job's creation is published as well as its
/// outcome. None of them is modelled — EventBridge events are not in any Smithy model —
/// so the set is evidence-led and adding a fifth would need its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobEventKind {
    /// The submission was accepted and the job exists.
    Created,
    /// The job finished its work.
    Succeeded,
    /// The job failed for a reason attributable to the caller's request.
    FailedWithClientError,
    /// The job failed inside the service.
    FailedWithServiceError,
}

impl JobEventKind {
    /// The `detail-type` this transition is published under — the value a rule's
    /// `EventPattern` matches on.
    pub fn detail_type(self) -> &'static str {
        match self {
            Self::Created => "Bedrock Data Automation Job Created",
            Self::Succeeded => "Bedrock Data Automation Job Succeeded",
            Self::FailedWithClientError => "Bedrock Data Automation Job Failed With Client Error",
            Self::FailedWithServiceError => "Bedrock Data Automation Job Failed With Service Error",
        }
    }

    /// The transition a job reaching `status` made, or `None` for a status that is not
    /// one a job MOVES into (`Created` is where a job starts, and the submission is what
    /// reports it).
    pub fn of_terminal_status(status: AutomationJobStatus) -> Option<Self> {
        match status {
            AutomationJobStatus::Success => Some(Self::Succeeded),
            AutomationJobStatus::ClientError => Some(Self::FailedWithClientError),
            AutomationJobStatus::ServiceError => Some(Self::FailedWithServiceError),
            AutomationJobStatus::Created | AutomationJobStatus::InProgress => None,
        }
    }
}

/// One job event waiting to be published: WHICH job, WHICH transition, WHEN it happened,
/// and the Region it happened in.
///
/// Self-contained on purpose — everything the event carries is taken when the transition
/// is queued, so a drain does not depend on the record still being retained.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobEvent {
    /// The job's own id: the `invocationArn`'s resource-id segment, which is what the
    /// event's `detail.job_id` carries.
    pub job_id: String,
    /// The ARN of the job the event is about, which the envelope carries in `resources`.
    pub invocation_arn: String,
    /// Which transition this reports.
    pub kind: JobEventKind,
    /// WHEN the transition happened — the instant the clock made it due, not the instant
    /// it was materialised or published.
    pub at: SystemTime,
    /// The Region the job was submitted in, read off its own ARN: an event is published
    /// in the Region the resource lives in.
    pub region: String,
}

impl JobEvent {
    /// The event reporting the transition `invocation` has just made into its terminal
    /// status, taken off the record itself so the event and the job cannot disagree.
    ///
    /// Falls back to the creation event only if called for a job that is not terminal,
    /// which [`super::ComAmazonawsBedrockdataautomationruntimeAmazonbedrockkeystoneruntimeserviceState::settle_invocation`]
    /// never does.
    pub fn of(invocation: &Invocation) -> Self {
        let kind = JobEventKind::of_terminal_status(invocation.status).unwrap_or(JobEventKind::Created);
        let at = invocation
            .job_completion_time
            .unwrap_or(invocation.job_submission_time);
        Self::new(invocation, kind, at)
    }

    /// The event reporting that the job `invocation` exists, as the submission that
    /// accepted it saw it.
    pub fn created(invocation: &Invocation) -> Self {
        Self::new(
            invocation,
            JobEventKind::Created,
            invocation.job_submission_time,
        )
    }

    /// One event about `invocation`.
    fn new(invocation: &Invocation, kind: JobEventKind, at: SystemTime) -> Self {
        Self {
            job_id: invocation.invocation_id.clone(),
            invocation_arn: invocation.invocation_arn.clone(),
            kind,
            at,
            region: ::service_common::arn::parse(&invocation.invocation_arn)
                .map(|arn| arn.region.to_string())
                .unwrap_or_else(|| super::DEFAULT_REGION.to_string()),
        }
    }

    /// The event's `detail`: the id of the job it is about.
    ///
    /// ONE member, `job_id`, which is the id segment of the job's `invocationArn` — what a
    /// consumer correlates the event with the submission it made by. Nothing else is
    /// stated: a member this emulator cannot ground in the record would be an invented
    /// claim about the job.
    fn detail(&self) -> ::serde_json::Value {
        ::serde_json::json!({ "job_id": self.job_id })
    }
}

/// `at` as the epoch milliseconds the event's `time` carries.
fn epoch_millis(at: SystemTime) -> i64 {
    i64::try_from(
        at.duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis(),
    )
    .unwrap_or_default()
}

/// Publish every job event this store has queued onto the account's default EventBridge
/// bus, and leave the queue empty.
///
/// Called with NO store lock held: each drain takes its events out in one critical
/// section, so nothing is published twice, and the in-process `PutEvents` runs outside it.
/// The queue is drained until it is EMPTY — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single call is unbounded, and the loop is what
/// guarantees the tail of a burst is published by the call that produced it. Nothing about
/// the answer the operation gives changes either way.
pub async fn publish_pending<Eng: Send + Sync + 'static>(
    callback: &EmbeddedEmulator,
    engine: &Eng,
) {
    let Some(state) = super::service_state(engine) else {
        return;
    };
    loop {
        let events = state.lock().take_job_events(MAX_PUBLISHED_PER_DRAIN);
        if events.is_empty() {
            return;
        }
        publish_batch(callback, &events).await;
    }
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`]
/// entries, one request per Region the drained events belong to.
async fn publish_batch(callback: &EmbeddedEmulator, events: &[JobEvent]) {
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        for region in regions_of(window) {
            let client = ::service_common::eventbridge_client::eventbridge_client(callback, &region);
            let mut request = client.put_events();
            for event in window.iter().filter(|event| event.region == region) {
                let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                    .event_bus_name(DEFAULT_EVENT_BUS)
                    .source(EVENT_SOURCE)
                    .detail_type(event.kind.detail_type())
                    // The event's `time` is WHEN the job moved, not when the queue was
                    // drained: `PutEventsRequestEntry$Time` is "the time stamp of the
                    // event, per RFC3339. If no time stamp is provided, the time stamp of
                    // the PutEvents call is used."
                    .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                        epoch_millis(event.at),
                    ))
                    .resources(&event.invocation_arn)
                    .detail(event.detail().to_string())
                    .build();
                request = request.entries(entry);
            }
            if let Err(error) = request.send().await {
                ::tracing::warn!(
                    region = %region,
                    error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                    "a data automation job event could not be published to EventBridge"
                );
            }
        }
    }
}

/// The Regions the events of one window belong to, each once and in the order they first
/// appear: an event is published in the Region of the job it is about, so a window holding
/// two Regions' jobs is published to both buses.
fn regions_of(window: &[JobEvent]) -> Vec<String> {
    let mut regions: Vec<String> = Vec::new();
    for event in window {
        if !regions.iter().any(|region| region == &event.region) {
            regions.push(event.region.clone());
        }
    }
    regions
}

/// Settle the job `invocation_arn` names once the clock owes it its terminal transition,
/// and publish what that settled.
///
/// Returns at once: the settle runs in a task of its own, because the submission that
/// started it has to answer while the job is still running. It exists for the job whose
/// submission asked to be NOTIFIED: such a client never calls this service again, so
/// without it the outcome event would wait for a status read that never comes. A job
/// nobody asked to be notified about needs none of this and starts no task — its outcome
/// is settled by the read that asks for it.
///
/// It materialises exactly the transition the clock already made due, through the same one
/// settle path a status read uses, so what this writes is what a read of the same instant
/// would have written; a job already settled by a reader is left alone, and the event is
/// queued exactly once whichever got there first.
///
/// The work it starts is BOUNDED: it takes one of
/// [`MAX_SETTLES_IN_FLIGHT`] slots and gives it back however it ends, so a stream of
/// notified submissions cannot leave unbounded work outstanding, and a submission that
/// finds no slot free starts nothing.
pub fn settle_when_due<Eng: Send + Sync + 'static>(
    engine: Arc<Eng>,
    callback: EmbeddedEmulator,
    invocation_arn: String,
    region: String,
    due: SystemTime,
) {
    let Some(state) = super::service_state(&*engine) else {
        return;
    };
    if !state.lock().try_begin_settle() {
        ::tracing::warn!(
            "{MAX_SETTLES_IN_FLIGHT} data automation job settles are already in flight; this \
             job's outcome will be settled by the next read of it"
        );
        return;
    }
    ::tokio::spawn(async move {
        let wait = due
            .duration_since(SystemTime::now())
            .unwrap_or_default()
            .saturating_add(SETTLE_SLACK);
        ::tokio::time::sleep(wait).await;
        settle_now(&*engine, &callback, &invocation_arn, &region).await;
        publish_pending(&callback, &*engine).await;
        if let Some(state) = super::service_state(&*engine) {
            state.lock().finish_settle();
        }
    });
}

/// Materialise the transitions the clock owes the job `invocation_arn` names — through the
/// ONE settle path a status read uses ([`super::job_output::settle_due_transitions`]), so the
/// job's output is delivered and its outcome settled exactly as a read of the same instant
/// would have done it.
async fn settle_now<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: &EmbeddedEmulator,
    invocation_arn: &str,
    region: &str,
) {
    super::job_output::settle_due_transitions(
        engine,
        callback,
        region,
        invocation_arn,
        SystemTime::now(),
    )
    .await;
}
