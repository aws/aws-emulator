// SPDX-License-Identifier: CC0-1.0

//! The EventBridge event this service publishes when a JOB RUN comes to rest.
//!
//! # The rule this module implements
//!
//! Glue is an event SOURCE, not only a control plane a client polls: a job run that reaches
//! a terminal [`JobRunState`](super::TERMINAL_JOB_RUN_STATES) is published onto the
//! account's default event bus ([`DEFAULT_EVENT_BUS`]) under the `source` this service owns
//! ([`EVENT_SOURCE`]) and the `detail-type` of a job run's state change
//! ([`JOB_STATE_CHANGE_DETAIL_TYPE`]), carrying — in the event's `detail` — the job the run
//! belongs to, the run's own id, the state it came to rest in, how severe that state is for
//! the job ([`SEVERITY_INFO`]/[`SEVERITY_ERROR`]) and the reason the run carries where it
//! carries one. A rule on that bus whose `EventPattern` matches then delivers it to that
//! rule's targets, so a client that reacts to a job run through EventBridge — a post-job
//! notification, a chained job, an audit archive — observes the run without calling Glue
//! again.
//!
//! WHAT counts as the event: the state a run COMES TO REST in. A run is published once, when
//! it reaches a terminal state, whichever state that is (`SUCCEEDED` for a script that
//! exited cleanly, the failure state for one that raised, the timeout state for one stopped
//! at its wall clock, `STOPPED` for one a client stopped) and whatever moved it there. A run
//! still in flight has come to rest at nothing, so it publishes nothing, and a run already
//! published is not published again.
//!
//! # How a transition becomes an event
//!
//! Nothing here decides WHEN a run moves: the stored due instant does, through
//! [`super::JobRunEntry::settle_if_due`]. This module only COMPARES what the store now holds
//! against what has been published ([`super::JobRunEntry::state_change_published`]), so a
//! run is published exactly once however it was materialised and no writer has to remember
//! to announce itself. The queue it feeds is bounded ([`MAX_PENDING_EVENTS`], oldest dropped
//! first) because every run that comes to rest appends to it.
//!
//! A queued change is published with NO store guard held
//! ([`publish_pending_job_run_events`]): EventBridge is reached the way every emulated
//! service reaches another, through the framework's in-process callback, so no socket is
//! opened and nothing outside this process is reachable from it. Publication is best effort —
//! an emulator with no EventBridge mounted has nowhere to publish — so a failure is logged
//! and the operation's own answer stands.
//!
//! WHICH call publishes: the CLOCK PUMP ([`drive_job_run_clock`]) does, because a client that
//! reacts to a run through EventBridge makes no further Glue call — an event published only
//! by the next reader would never be published at all. The ONE path that performs the script
//! work a job's runs are owed ([`super::perform_owed_job_runs`]) — the path `StartJobRun`,
//! the job-run readers, the trigger starts and the workflow reads all go through — records
//! what it materialised and kicks that pump, which then waits for each state the clock still
//! owes, materialises it and publishes it with no request waiting on any of it.

use ::std::collections::VecDeque;
use ::std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::*;

/// The `source` of every event Glue publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`, and
/// this service's namespace is the one its own ARNs carry.
pub const EVENT_SOURCE: &str = "aws.glue";

/// The `detail-type` of the event published when a JOB RUN comes to rest.
pub const JOB_STATE_CHANGE_DETAIL_TYPE: &str = "Glue Job State Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is the
/// bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// How severe the state a run came to rest in is for the job: the value the event's
/// `detail.severity` carries for a run that came to rest having failed at nothing.
pub const SEVERITY_INFO: &str = "INFO";

/// The same, for a run that came to rest on a failure — the states whose runs carry a
/// `JobRun$ErrorMessage` ([`super::JOB_RUN_STATE_FAILED`],
/// [`super::JOB_RUN_STATE_TIMEOUT`], [`super::JOB_RUN_STATE_ERROR`]).
pub const SEVERITY_ERROR: &str = "ERROR";

/// How many job-run state changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every run that
/// comes to rest appends to it, so without a bound a client starting runs without end could
/// grow it without end. The oldest change is dropped first, so the runs that have just come
/// to rest are the ones that survive. It is far above what a realistic burst produces — a
/// run contributes one change, and the runs one job holds are themselves bounded
/// ([`super::MAX_RUNS_PER_JOB`]).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a drain holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one call draining all of it
/// would fan out to a rule's targets that many times: this caps one drain, and
/// [`publish_pending_job_run_events`] repeats a drain until nothing of that Region is left
/// queued, so nothing waits for a later call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// The shortest one wake-up of the clock pump waits.
///
/// A due instant already in the past is materialised on the NEXT wake-up, not on this one's
/// instant, so the pump always hands the runtime back and a wake-up that materialises nothing
/// cannot turn the loop into a spin. It is far below [`JOB_RUN_SETTLES_AFTER`], so it never
/// delays a transition noticeably.
const MIN_CLOCK_WAIT: Duration = Duration::from_millis(5);

/// The longest one wake-up of the clock pump waits before it looks at the store again.
///
/// Every state a run owes falls due within a span or two of the one before it, so a wait is
/// never near this value; it is a backstop, so a due instant far in the future cannot park
/// the pump indefinitely.
const MAX_CLOCK_WAIT: Duration = Duration::from_secs(5);

/// How many wake-ups one pump makes before it stands down.
///
/// A backstop, not a budget: the states the store owes are already finite (a run owes a
/// handful, and both the jobs and the runs one job keeps are bounded), and every wake-up
/// either materialises a state or clears the due instant that woke it. The cap guarantees
/// termination even if a later change left a due instant that materialising does not clear,
/// and the next call starts a fresh pump.
const MAX_CLOCK_WAKE_UPS: usize = 10_000;

/// One job-run state change waiting to be published, rendered against the run as it stood
/// when the change was observed.
#[derive(Debug, Clone)]
pub struct JobRunStateChange {
    /// The Region the run belongs to, which is the Region whose default bus the event goes
    /// onto: a job exists only in the Region it was created in, so its runs are published
    /// there and nowhere else.
    pub region: String,
    /// WHEN the run came to rest — the instant the record itself stamps on its completion,
    /// not the instant some later call materialised or published it.
    pub at: SystemTime,
    /// The rendered `detail`.
    pub detail: ::serde_json::Value,
}

impl ComAmazonawsGlueAwsglueState {
    /// Queue a state-change event for every run of `job_names` in `region` that has come to
    /// rest and has not been published yet.
    ///
    /// This is the ONE place a terminal state becomes an event, so no writer can forget to
    /// announce one and none can be published twice: the comparison is against what was
    /// published, not against who wrote it. The changes are queued oldest completion first,
    /// so events about several runs leave in the order the runs finished. The walk is
    /// bounded by the jobs named and the runs one job holds
    /// ([`super::MAX_RUNS_PER_JOB`]).
    pub fn record_job_run_state_changes(&mut self, region: &str, job_names: &[String]) {
        let mut observed: Vec<JobRunStateChange> = Vec::new();
        for job_name in job_names {
            let Some(job) = self.job_mut(region, job_name) else {
                continue;
            };
            for run in &mut job.runs {
                if !run.is_terminal() || run.state_change_published {
                    continue;
                }
                run.state_change_published = true;
                observed.push(JobRunStateChange {
                    region: region.to_string(),
                    at: run.completed_on.unwrap_or(run.last_modified_on),
                    detail: job_run_state_change_detail(run),
                });
            }
        }
        observed.sort_by_key(|change| change.at);
        for change in observed {
            while self.pending_job_run_events.len() >= MAX_PENDING_EVENTS {
                self.pending_job_run_events.pop_front();
                ::tracing::warn!(
                    "the queue of Glue job run state changes waiting to be published to \
                     EventBridge is full at {MAX_PENDING_EVENTS}; the oldest change was dropped"
                );
            }
            self.pending_job_run_events.push_back(change);
        }
    }

    /// Take at most `limit` of this Region's changes that are not published yet, oldest
    /// first, leaving every other Region's where they are.
    ///
    /// Taking the changes out under the guard is what stops two concurrent calls from
    /// publishing one change twice.
    fn take_job_run_state_changes(
        &mut self,
        region: &str,
        limit: usize,
    ) -> Vec<JobRunStateChange> {
        let mut taken: Vec<JobRunStateChange> = Vec::new();
        let mut kept: VecDeque<JobRunStateChange> = VecDeque::new();
        for change in ::std::mem::take(&mut self.pending_job_run_events) {
            if taken.len() < limit && change.region == region {
                taken.push(change);
            } else {
                kept.push_back(change);
            }
        }
        self.pending_job_run_events = kept;
        taken
    }

    /// How many changes are queued for publication — reporting only, for a reader that has
    /// to tell "nothing came to rest" from "nothing was published".
    #[must_use]
    pub fn pending_job_run_event_count(&self) -> usize {
        self.pending_job_run_events.len()
    }

    /// The Regions this store has changes queued for, each once, in the order they were
    /// queued — the buses one publication pass has to reach.
    fn pending_job_run_event_regions(&self) -> Vec<String> {
        let mut regions: Vec<String> = Vec::new();
        for change in &self.pending_job_run_events {
            if !regions.iter().any(|region| region == &change.region) {
                regions.push(change.region.clone());
            }
        }
        regions
    }

    /// Give every run of every job the state the CLOCK already owed it at `now`, and record
    /// what that materialised.
    ///
    /// What the clock pump applies on a wake-up: the same [`JobRunEntry::settle_if_due`]
    /// every reading door applies, over the whole store rather than over one job, because the
    /// pump is not serving a request about one of them. It moves nothing that was not already
    /// due, so a caller reading before the due instant still sees the run in flight. The walk
    /// is bounded by the jobs this store holds and the runs one job keeps
    /// ([`MAX_RUNS_PER_JOB`]).
    pub fn settle_due_job_runs(&mut self, now: SystemTime) {
        let scope: Vec<(String, Vec<String>)> = self
            .jobs
            .iter()
            .map(|(region, jobs)| (region.clone(), jobs.keys().cloned().collect()))
            .collect();
        for (region, job_names) in scope {
            if let Some(jobs) = self.jobs.get_mut(&region) {
                for name in &job_names {
                    if let Some(job) = jobs.get_mut(name) {
                        job.settle_due_runs(now);
                    }
                }
            }
            self.record_job_run_state_changes(&region, &job_names);
        }
    }

    /// The earliest instant at which this store owes a job run its next state, if any — the
    /// instant the clock pump waits for.
    fn next_job_run_due_at(&self) -> Option<SystemTime> {
        self.jobs
            .values()
            .flat_map(HashMap::values)
            .flat_map(|job| job.runs.iter())
            .filter_map(|run| run.settles_at)
            .min()
    }

    /// What the clock pump waits for next, or `None` — having RELEASED its claim — when this
    /// store owes nothing more.
    ///
    /// Observing "nothing is owed" and standing down happen in ONE critical section, which is
    /// what makes the hand-over race-free: a door records its change (and a start stores its
    /// run, and so its due instant) under this same guard BEFORE it tries to claim the pump,
    /// so either this call sees that instant and the running pump keeps it, or the door's
    /// claim finds the flag already cleared and starts a pump of its own. A change recorded
    /// but not yet published keeps the pump awake too, so nothing queued waits for the next
    /// call.
    fn next_job_run_due_or_release_pump(
        &mut self,
        now: SystemTime,
        generation: u64,
    ) -> Option<SystemTime> {
        if !self.pending_job_run_events.is_empty() {
            return Some(now);
        }
        match self.next_job_run_due_at() {
            Some(due) => Some(due),
            None => {
                self.release_job_run_clock_pump(generation);
                None
            }
        }
    }

    /// Claim the single clock pump for this store: the claim's GENERATION when the caller may
    /// start one, `None` when one is already running.
    ///
    /// The generation is what makes a release safe: a pump releases its own claim and never a
    /// later pump's (see [`Self::release_job_run_clock_pump`]).
    fn start_job_run_clock_pump(&mut self) -> Option<u64> {
        if self.job_run_clock_pump_running {
            return None;
        }
        self.job_run_clock_pump_running = true;
        self.job_run_clock_pump_generation = self.job_run_clock_pump_generation.wrapping_add(1);
        Some(self.job_run_clock_pump_generation)
    }

    /// Release the claim `generation` took — and ONLY that one.
    ///
    /// A pump that ends on the wake-up cap must not clear a claim a pump started since is
    /// holding, or two pumps would run against one store.
    fn release_job_run_clock_pump(&mut self, generation: u64) {
        if self.job_run_clock_pump_generation == generation {
            self.job_run_clock_pump_running = false;
        }
    }

    /// Keep the running pump's task handle, aborting the one it replaces.
    ///
    /// The replaced handle is a pump that has already ended — by standing down, or on its
    /// wake-up cap — so the abort is normally a no-op; it is what guarantees the invariant
    /// anyway, that no earlier pump task is still running beside the one that now holds the
    /// claim.
    fn set_job_run_clock_pump(&mut self, handle: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.job_run_clock_pump.replace(handle) {
            previous.abort();
        }
    }
}

/// The `detail` of a Glue job run state change: WHICH job, WHICH run, the state it came to
/// rest in, how severe that state is, and the reason the run carries where it carries one.
///
/// Every member is read off the stored run, so the event reports the run a `GetJobRun` of
/// the same instant reports and never a second, invented copy of it. A run that came to rest
/// having failed at nothing carries no `JobRun$ErrorMessage` ("the error message associated
/// with this job run"), so the event states no message either rather than an empty one.
fn job_run_state_change_detail(run: &JobRunEntry) -> ::serde_json::Value {
    let mut detail = ::serde_json::Map::new();
    detail.insert(
        "jobName".to_string(),
        ::serde_json::Value::String(run.job_name.clone()),
    );
    detail.insert(
        "severity".to_string(),
        ::serde_json::Value::String(state_severity(&run.state).to_string()),
    );
    detail.insert(
        "state".to_string(),
        ::serde_json::Value::String(run.state.clone()),
    );
    detail.insert(
        "jobRunId".to_string(),
        ::serde_json::Value::String(run.id.clone()),
    );
    if let Some(message) = run.error_message.as_ref() {
        detail.insert(
            "message".to_string(),
            ::serde_json::Value::String(message.clone()),
        );
    }
    ::serde_json::Value::Object(detail)
}

/// How severe the terminal `state` is for the job: a run that FAILED at something is the
/// error half ([`SEVERITY_ERROR`]), and one that came to rest having failed at nothing — it
/// succeeded, or a client stopped it — is the informational half ([`SEVERITY_INFO`]).
#[must_use]
pub fn state_severity(state: &str) -> &'static str {
    match state {
        JOB_RUN_STATE_FAILED | JOB_RUN_STATE_TIMEOUT | JOB_RUN_STATE_ERROR => SEVERITY_ERROR,
        _ => SEVERITY_INFO,
    }
}

/// Record what the clock has materialised for the runs of `job_names` in `region`, and hand
/// the publication — and the rest of the lifecycle — to the clock pump.
///
/// Called with NO store guard held: this takes the guard itself to compare those jobs' runs
/// against what has been published, so a door still holding it must drop it first. A call
/// that materialised nothing records nothing, so it costs one walk of those jobs' runs.
///
/// The publication itself is the pump's ([`drive_job_run_clock`]), which is what lets a door
/// answer its caller without waiting on another service: the event leaves one pump tick
/// later, and each run the clock still owes a terminal state is published as it falls due.
pub fn record_and_publish_job_run_state_changes(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
    region: &str,
    job_names: &[String],
) {
    state
        .lock()
        .record_job_run_state_changes(region, job_names);
    drive_job_run_clock(callback, state);
}

/// Publish the terminal states the CLOCK owes this store's job runs, as they fall due, with
/// no further call from anybody.
///
/// Glue notifies EventBridge as a job run comes to rest, which is the whole point of the
/// event path: a client that reacts to a run through EventBridge makes no further Glue call,
/// so an event published only by the next reader would never be published at all. This starts
/// ONE in-process task that waits for the earliest due instant the store holds, materialises
/// what is then due — the same [`JobRunEntry::settle_if_due`] every reading door applies — and
/// publishes what that produced, until nothing is owed and nothing is queued.
///
/// It does not change WHEN a run moves: only the stored due instant does, so a caller reading
/// before that instant still sees the run in flight and two reads at the same instant still
/// answer identically. The task waits for the clock so that no request ever has to. It
/// performs no script: a run still owed one is moved no further than the state a run whose
/// script is running is in, and the door that performs the script records what it came to.
///
/// At most one pump runs per store (the store carries the claim), it is bounded by
/// [`MAX_CLOCK_WAKE_UPS`] wake-ups, and it exits as soon as the store owes nothing. With no
/// async runtime to spawn onto there is no pump, and the changes are published by the next
/// call that reaches this instead.
pub fn drive_job_run_clock(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = state.lock().start_job_run_clock_pump() else {
        return;
    };
    let callback = callback.clone();
    let pumped = ::engine::ServiceState::clone(state);
    let state = ::engine::ServiceState::clone(state);
    let handle = runtime.spawn(async move {
        // `true` only if the wake-up cap below is what ended the loop; a pump that ran out of
        // work has already released its claim inside the one critical section that observed
        // it ([`ComAmazonawsGlueAwsglueState::next_job_run_due_or_release_pump`]).
        let mut capped = true;
        for _ in 0..MAX_CLOCK_WAKE_UPS {
            let now = SystemTime::now();
            let Some(due) = state.lock().next_job_run_due_or_release_pump(now, generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps the
            // backstop above from ever costing a CPU-bound spin, and it hands the runtime
            // back between wake-ups.
            let wait = due
                .duration_since(now)
                .unwrap_or_default()
                .clamp(MIN_CLOCK_WAIT, MAX_CLOCK_WAIT);
            ::tokio::time::sleep(wait).await;
            state
                .lock()
                .settle_due_job_runs(SystemTime::now());
            publish_pending_job_run_events(&callback, &state).await;
        }
        if capped {
            // Only THIS pump's claim, so a pump started since is left holding its own.
            state.lock().release_job_run_clock_pump(generation);
        }
    });
    // The handle is kept so the pump it replaces is aborted rather than merely forgotten:
    // that is what guarantees no earlier pump task runs beside the one holding the claim.
    pumped.lock().set_job_run_clock_pump(handle);
}

/// Publish every queued job-run state change onto the default EventBridge bus of the Region
/// it belongs to, and leave the queue empty.
///
/// Called with NO store guard held: each drain takes its changes out in one critical section,
/// so nothing is published twice, and the in-process `PutEvents` runs outside it. A Region's
/// changes are drained until it has none left — one drain is capped
/// ([`MAX_PUBLISHED_PER_DRAIN`]) so no single request is unbounded. Nothing about the answer
/// any operation gives changes either way.
pub async fn publish_pending_job_run_events(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    state: &::engine::ServiceState<ComAmazonawsGlueAwsglueState>,
) {
    let regions = state.lock().pending_job_run_event_regions();
    for region in regions {
        let client =
            ::service_common::eventbridge_client::eventbridge_client(callback, &region);
        let mut events = {
            let mut store = state.lock();
            store.take_job_run_state_changes(&region, MAX_PUBLISHED_PER_DRAIN)
        };
        while !events.is_empty() {
            publish_batch(&client, &events).await;
            events = {
                let mut store = state.lock();
                store.take_job_run_state_changes(&region, MAX_PUBLISHED_PER_DRAIN)
            };
        }
    }
}

/// Publish one drain, in `PutEvents` requests of at most [`MAX_PUT_EVENTS_ENTRIES`] entries.
///
/// The event names no resource in its envelope: a job run is addressed by its job's name and
/// its own id, which the `detail` carries, and the model gives a run no ARN for `resources`
/// to name it by.
async fn publish_batch(
    client: &::aws_sdk_eventbridge::Client,
    events: &[JobRunStateChange],
) {
    for window in events.chunks(MAX_PUT_EVENTS_ENTRIES) {
        let mut request = client.put_events();
        for event in window {
            let entry = ::aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(DEFAULT_EVENT_BUS)
                .source(EVENT_SOURCE)
                .detail_type(JOB_STATE_CHANGE_DETAIL_TYPE)
                // The event's `time` is WHEN the run came to rest, not when the queue was
                // drained: `PutEventsRequestEntry$Time` is "the time stamp of the event,
                // per RFC3339. If no time stamp is provided, the time stamp of the
                // PutEvents call is used."
                .time(::aws_sdk_eventbridge::primitives::DateTime::from_millis(
                    epoch_millis(event.at),
                ))
                .detail(event.detail.to_string())
                .build();
            request = request.entries(entry);
        }
        if let Err(error) = request.send().await {
            ::tracing::warn!(
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a Glue job run state change could not be published to EventBridge"
            );
        }
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
