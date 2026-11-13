// SPDX-License-Identifier: CC0-1.0

//! The INVOCATION half of EventBridge Scheduler: the clock that notices which
//! schedules the expressions have made due, and the hand-over that reaches each
//! one's target.
//!
//! EventBridge Scheduler is a data-plane service. A schedule in `State=ENABLED`
//! "invokes the target" at each occurrence its `ScheduleExpression` names
//! ([`super::occurrence`]), inside the window `StartDate`/`EndDate` bound, and at
//! each occurrence it acts as the `Target$RoleArn` — "the Amazon Resource Name
//! (ARN) of the IAM role that EventBridge Scheduler will use for this target when
//! the schedule is invoked" — and hands `Target$Input`, "the text, or well-formed
//! JSON, passed to the target", to the API that target's service is invoked by.
//! Nothing a client calls makes that happen: a client writes a schedule and then
//! watches the TARGET, so this clock is what makes a schedule observable at all.
//!
//! It is the same shape as the schedule clock EventBridge's own rules are fired by:
//! at most one task per store, holding a claim it records a tick against, started
//! by whichever request last touched a schedule, and nothing sleeps for a due
//! instant — the instant is stored and the clock performs what it has already
//! made due.
//!
//! ## Which schedule is on a clock
//!
//! One whose `State` is `ENABLED` — "specifies whether the schedule is enabled or
//! disabled", and a disabled schedule invokes nothing — in a group whose deletion
//! is not under way, and whose `ScheduleExpression` names occurrences this emulator
//! can compute. A schedule that stops satisfying any of those (deleted, disabled,
//! re-pointed, its group deleted) keeps no due instant, so re-enabling one starts
//! its series afresh rather than owing the invocations it missed.
//!
//! ## When this emulator performs an occurrence
//!
//! AT the occurrence's own instant — the one the expression names, in the schedule's
//! zone, and the one the target is told as its scheduled time — and never before it.
//! A time-based service's whole contract is that instant: `FlexibleTimeWindow$Mode`
//! "determines whether the schedule is invoked within a flexible time window", and
//! `MaximumWindowInMinutes` is "the maximum time window during which a schedule can
//! be invoked", so the window extends FORWARD from the occurrence and never back.
//! With `Mode=OFF` the invocation is made at the occurrence itself; with
//! `Mode=FLEXIBLE` at an instant inside `[occurrence, occurrence + window]`
//! ([`window_offset`]). The wait is served at its own length — nothing is compressed
//! and nothing sleeps for it: the instant is stored and the clock performs what it
//! has already made due.
//!
//! ## What reaches the target
//!
//! `Target$Input` with the schedule's context attributes substituted
//! ([`rendered_input`]), handed to the API the target names. A TEMPLATED target names
//! a RESOURCE, and the API is the one that kind of resource is invoked by:
//! `sqs:SendMessage` for a queue, `sns:Publish` for a topic,
//! `lambda:InvokeFunction` for a function and `states:StartExecution` for a state
//! machine. A UNIVERSAL target names the CALL itself
//! (`arn:aws:scheduler:::aws-sdk:<service>:<operation>`), and the invocation is that
//! operation with the input as its own request ([`super::universal_target`]). The call
//! is made over the in-process callback either way, so what the target holds afterwards
//! is what that service's own handlers wrote.
//!
//! ## When a schedule is done
//!
//! A schedule COMPLETES when it has no occurrence left: a one-time `at(...)`
//! schedule at the occurrence it just ran, a recurring one at the end of the window
//! `EndDate` closes (its last occurrence being the one closest to that instant).
//! `ActionAfterCompletion` "specifies the action that EventBridge Scheduler applies
//! to the schedule after the schedule completes invoking the target", so a schedule
//! stating `DELETE` is removed from the store then ([`apply_action_after_completion`])
//! and one stating `NONE` is left where it is.
//!
//! ## When an invocation does not reach its target
//!
//! The invocation is made UNDER the schedule's execution role, so the call it makes is
//! one that role's permissions allow or it is not made at all; and a call that was
//! made and failed is made AGAIN, inside both bounds `Target$RetryPolicy` states,
//! until one of them is exhausted. The invocation that could not be delivered is then
//! handed to the queue `Target$DeadLetterConfig` names, carrying the request the
//! attempt made of the target's own API ([`failed_event`]) and the detail of the
//! failure that ended it. That half of
//! the question — which failures are worth another attempt, how many attempts the
//! policy admits, and what the dead-letter message carries — is [`super::dead_letter`];
//! the attempts themselves wait on this clock, so nothing sleeps between them either.
//!
//! The remaining templated target kinds (the ECS task, the EventBridge bus, the Kinesis
//! stream and the SageMaker pipeline) are NOT invoked here: an occurrence whose target
//! kind this does not invoke is recorded as not attempted — never as delivered, and
//! never as a failure of a call nothing made. The same holds for a universal target
//! naming an operation this emulator makes no call for.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{FlexibleTimeWindow, Target};

use super::dead_letter::{
    dead_letter_attributes, failure_of, withheld, Attempt, Exhaustion, Failure, Report,
    RetryBounds, ACCESS_DENIED_CODE, AUTHORIZATION_ERROR_CODE, QUERY_ACCESS_DENIED_CODE,
    RETRY_INTERVAL,
};
use super::universal_target::{self, UniversalCall};
use super::{
    occurrence::Occurrences, service_state, ComAmazonawsSchedulerAwschronosserviceState,
    ScheduleRecord, FALLBACK_REGION,
};

/// Seconds in the minute `MaximumWindowInMinutes` states its window in.
const SECONDS_PER_MINUTE: u64 = 60;

/// How often the clock wakes up to ask what is due.
///
/// Emulator-chosen: fine enough that an invocation is performed close to the
/// instant it fell due on this clock, and coarse enough that an idle emulator
/// spends nothing noticeable on it.
const CLOCK_TICK: Duration = Duration::from_millis(250);

/// How many ticks one clock may take before it stands down, so a task started by
/// one request cannot outlive the process it was started in by much: at
/// [`CLOCK_TICK`] a tick, two hours. A later call that writes a schedule starts a
/// fresh one ([`drive_invocation_clock`]).
const MAX_CLOCK_TICKS: usize = 28_800;

/// After how long with no tick recorded another caller may take the clock over.
///
/// The claim below is held by whichever clock last recorded a tick; a task that
/// ENDED without releasing it (a panic, a runtime shut down under it) would
/// otherwise keep every schedule from ever being invoked again, so the claim lapses
/// after this much silence and the next call that writes a schedule starts a clock
/// that takes it. It is longer than one invocation can take, so a clock busy
/// delivering is never taken over mid-invocation.
const CLAIM_LAPSES_AFTER: Duration = Duration::from_secs(120);

/// The `ScheduleState` that invokes nothing: "DISABLED […] specifies whether the
/// schedule is enabled or disabled".
const SCHEDULE_STATE_DISABLED: &str = "DISABLED";

/// The `ActionAfterCompletion` that REMOVES a schedule once it has completed:
/// modelled `NONE | DELETE`, and only this one acts on the store.
const ACTION_AFTER_COMPLETION_DELETE: &str = "DELETE";

/// The format the scheduled-time context attribute carries, as EventBridge
/// Scheduler publishes it (`2022-03-10T13:00:00Z`): the occurrence's own instant in
/// UTC, to the second.
const SCHEDULED_TIME_FORMAT: &str = "%Y-%m-%dT%H:%M:%SZ";

/// The context attributes a target's `Input` may carry, which EventBridge Scheduler
/// substitutes on every invocation ("schedule context attributes": the schedule's
/// ARN, the occurrence's scheduled time, the invocation's execution id and the
/// attempt this delivery is).
const SCHEDULE_ARN_ATTRIBUTE: &str = "<aws.scheduler.schedule-arn>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const SCHEDULED_TIME_ATTRIBUTE: &str = "<aws.scheduler.scheduled-time>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const EXECUTION_ID_ATTRIBUTE: &str = "<aws.scheduler.execution-id>";
/// See [`SCHEDULE_ARN_ATTRIBUTE`].
const ATTEMPT_NUMBER_ATTRIBUTE: &str = "<aws.scheduler.attempt-number>";

/// The most invocations this store keeps unfinished attempts for at once.
///
/// An EMULATOR-CHOSEN ceiling: a failure the retry policy admits another attempt after
/// keeps its invocation — and its payload — on the clock until the policy is
/// exhausted, and a client can write schedules faster than their targets refuse them.
/// Past this point a further failed invocation is not retried again but reported at
/// once, so the bound costs attempts rather than losing the failure.
const MAX_PENDING_INVOCATIONS: usize = 1_024;

/// What a target with NO `Input` of its own is handed.
///
/// `Target$Input` is optional and the service documents only that it "delivers a
/// default notification to the target" when a schedule states none, without
/// publishing that notification's content — so the empty JSON document is what this
/// emulator hands such a target, and it is CHOSEN rather than read off the service.
const DEFAULT_INPUT: &str = "{}";

/// The ARN namespaces of the target kinds this module invokes, each the
/// `arnNamespace` of the service whose API carries the invocation.
const SQS_ARN_NAMESPACE: &str = "sqs";
/// See [`SQS_ARN_NAMESPACE`].
const SNS_ARN_NAMESPACE: &str = "sns";
/// See [`SQS_ARN_NAMESPACE`].
const LAMBDA_ARN_NAMESPACE: &str = "lambda";
/// See [`SQS_ARN_NAMESPACE`].
const STATES_ARN_NAMESPACE: &str = "states";

/// The IAM action a queue's invocation — and the delivery of a dead-letter message,
/// which reaches its queue the same way — is authorized by.
///
/// One per target kind below: each is the action of the single API that kind of target
/// is invoked by, which is the permission EventBridge Scheduler's execution-role
/// reference documents for a schedule with that target.
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";
/// See [`SEND_MESSAGE_ACTION`].
const PUBLISH_ACTION: &str = "sns:Publish";
/// See [`SEND_MESSAGE_ACTION`].
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";
/// See [`SEND_MESSAGE_ACTION`].
const START_EXECUTION_ACTION: &str = "states:StartExecution";

/// What the clock has done and which clock is running it: the whole of this
/// module's state, held in the store so it survives the task that reads it.
#[derive(Debug, Default)]
pub struct Invocations {
    /// The next occurrence of each schedule on a clock, by the schedule's own ARN.
    armed: BTreeMap<String, Armed>,
    /// The generation of the clock currently driving these schedules, with the
    /// instant it last recorded a tick: see [`CLAIM_LAPSES_AFTER`].
    clock: Option<(u64, SystemTime)>,
    /// The generations handed out so far, so no two clocks of one run share one.
    generation: u64,
    /// The task the clock of that generation runs as, so it stays ABORTABLE: a
    /// clock whose claim is taken over is stopped rather than left running beside
    /// the one that took it.
    running: Option<::tokio::task::JoinHandle<()>>,
    /// How many invocations this store has performed, which is what makes each
    /// execution id its own ([`Self::next_execution_id`]).
    performed: u64,
    /// The invocations this clock has attempted and not finished with, each holding
    /// the instant its next attempt falls due ([`Pending`]).
    pending: Vec<Pending>,
}

/// One schedule's next occurrence, as the clock holds it.
#[derive(Debug, Clone)]
struct Armed {
    /// The definition the entry was computed from, so a schedule an
    /// `UpdateSchedule` re-pointed is re-armed rather than invoked on the series it
    /// no longer states ([`definition_of`]).
    definition: String,
    /// The occurrence's OWN instant, the one the expression names and the target is
    /// told: it is what the next occurrence is computed from, so the series keeps the
    /// cadence the expression states rather than drifting by however long a delivery
    /// took.
    nominal: SystemTime,
    /// The instant the invocation for that occurrence is made at: the occurrence
    /// itself, or an instant inside the flexible window that opens AT it — never
    /// earlier. `None` for a series with no occurrence left: a one-time schedule that
    /// has been invoked, or a recurring one whose `EndDate` holds no further
    /// occurrence.
    due: Option<SystemTime>,
    /// When the schedule COMPLETED, for a series with no occurrence left, or `None`
    /// while it still has one (or has run nothing and states no end): what
    /// [`apply_action_after_completion`] acts on once that instant has passed.
    completed: Option<SystemTime>,
}

/// One schedule the clock may invoke: how it is addressed, the series it runs on,
/// and the window that series is bounded by.
#[derive(Debug, Clone)]
struct Schedulable {
    group: String,
    name: String,
    arn: String,
    definition: String,
    occurrences: Occurrences,
    /// "The date, after which the schedule can begin invoking its target"
    /// (`StartDate`), when the schedule states one.
    start: Option<SystemTime>,
    /// "The date, before which the schedule can invoke its target" (`EndDate`),
    /// when the schedule states one.
    end: Option<SystemTime>,
    /// The FLEXIBLE window an occurrence of this schedule may be invoked inside —
    /// "the maximum time window during which a schedule can be invoked"
    /// (`FlexibleTimeWindow$MaximumWindowInMinutes`) — or `None` for a schedule whose
    /// `Mode` asks for none, which is invoked at the occurrence itself.
    window: Option<Duration>,
}

impl Schedulable {
    /// The first occurrence after `after` that falls inside this schedule's window,
    /// or `None` when the series has none left.
    ///
    /// The window is the schedule's: an occurrence before `StartDate` is not one it
    /// "can begin invoking its target" at, and one at or past `EndDate` is not one
    /// it "can invoke its target" before. `EndDate` bounds a recurring series only
    /// — "EventBridge Scheduler ignores StartDate and EndDate for one-time
    /// schedules" — so an `at(...)` instant is kept whatever those members state.
    fn next_after(&self, after: SystemTime) -> Option<SystemTime> {
        let one_time = self.occurrences.is_one_time();
        // `StartDate` is a FLOOR on the series, not an offset into it: the first
        // occurrence is the first one the expression names at or after that instant
        // ("invocations might occur on, or after, the StartDate you specify"), so a
        // schedule armed ahead of time invokes nothing while the window is shut.
        let next = match self.start {
            Some(start) if !one_time && start > after => {
                self.occurrences.next_at_or_after(start)
            }
            _ => self.occurrences.next_after(after),
        }?;
        match self.end {
            Some(end) if !one_time && next >= end => None,
            _ => Some(next),
        }
    }

    /// Whether this schedule names at most one occurrence, which is what decides
    /// whether `StartDate`/`EndDate` bound it at all ("EventBridge Scheduler ignores
    /// StartDate and EndDate for one-time schedules").
    fn is_one_time(&self) -> bool {
        self.occurrences.is_one_time()
    }
}

/// One occurrence the clock has made due: which schedule, and the occurrence's own
/// instant.
#[derive(Debug, Clone)]
struct Firing {
    schedule: Schedulable,
    nominal: SystemTime,
}

/// One invocation this clock has attempted and not finished with.
///
/// It carries everything a FURTHER attempt of that invocation needs, read when the
/// first attempt was made: an invocation is of the schedule as it stood when the
/// occurrence fell due, so a schedule re-pointed (or deleted) while one of its
/// invocations is still being attempted neither re-points nor withdraws it.
#[derive(Debug, Clone)]
struct Pending {
    /// The schedule the invocation belongs to.
    schedule_arn: String,
    /// The Region the schedule itself is in, which a target ARN labelled with none is
    /// reached in.
    region: String,
    /// The target as the schedule stated it when the occurrence fell due.
    target: Target,
    /// The occurrence's own instant, which is the invocation's scheduled time.
    nominal: SystemTime,
    /// The invocation's own execution id, which every attempt of it shares: the
    /// attempts are one invocation.
    execution_id: String,
    /// The instant the FIRST attempt was made at, which the event's age — "the
    /// maximum amount of time, in seconds, to continue to make retry attempts" — is
    /// measured from.
    began: SystemTime,
    /// How many attempts of this invocation have been MADE.
    attempts: u32,
    /// When the next attempt of it falls due.
    due: SystemTime,
}

impl Invocations {
    /// Claim the single clock for this store, answering the generation the caller's
    /// clock runs under — `None` when a clock is really running, which is what
    /// keeps one clock per store however many calls ask.
    fn claim(&mut self, now: SystemTime) -> Option<u64> {
        if let Some((_, ticked)) = self.clock {
            let lapsed = now
                .duration_since(ticked)
                .is_ok_and(|silent| silent >= CLAIM_LAPSES_AFTER);
            if !lapsed {
                return None;
            }
        }
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.clock = Some((generation, now));
        Some(generation)
    }

    /// Record that the clock of `generation` is still running, answering whether it
    /// still holds the claim: one that has been taken over stands down rather than
    /// invoking beside the clock that took it.
    fn ticked(&mut self, generation: u64, now: SystemTime) -> bool {
        match self.clock {
            Some((held, _)) if held == generation => {
                self.clock = Some((generation, now));
                true
            }
            _ => false,
        }
    }

    /// Release the claim held by `generation`, leaving a claim taken over since
    /// alone.
    fn release(&mut self, generation: u64) {
        if matches!(self.clock, Some((held, _)) if held == generation) {
            self.clock = None;
            self.running = None;
        }
    }

    /// Hold the task the clock just started, stopping whichever task the claim it
    /// took was held by: at most one clock task is alive per store, and it can
    /// always be stopped.
    fn set_running(&mut self, running: ::tokio::task::JoinHandle<()>) {
        if let Some(previous) = self.running.replace(running) {
            previous.abort();
        }
    }

    /// The execution id of the next invocation this store performs.
    ///
    /// EventBridge Scheduler names every invocation ("the execution id" a target's
    /// `Input` may carry), and the form it publishes is a UUID, rendered here
    /// through this tree's one rendering of 16 opaque bytes as one. The bytes are
    /// this store's own invocation counter and its complement, so no two
    /// invocations of one run share an id.
    fn next_execution_id(&mut self) -> String {
        self.performed = self.performed.wrapping_add(1);
        let mut bytes = [0u8; 16];
        bytes[..8].copy_from_slice(&(!self.performed).to_be_bytes());
        bytes[8..].copy_from_slice(&self.performed.to_be_bytes());
        ::service_common::uuid_from_random_bytes(bytes)
    }

    /// Which schedules are DUE now, and the occurrence each one's clock moves to.
    ///
    /// Both halves happen here, under one lock: the invocation is performed outside
    /// it, so the due instant has to move BEFORE the delivery rather than after it —
    /// otherwise a slow delivery would make the same occurrence due twice.
    fn take_due(&mut self, schedulable: &[Schedulable], now: SystemTime) -> Vec<Firing> {
        let mut firing = Vec::new();
        let mut known: BTreeMap<String, Armed> = BTreeMap::new();
        for schedule in schedulable {
            let held = self
                .armed
                .get(&schedule.arn)
                .filter(|armed| armed.definition == schedule.definition);
            let Some(armed) = held else {
                // A schedule this clock has not seen yet, or one whose definition a
                // write has changed: its next occurrence is computed from now, so
                // the series a client just asked for starts where the client asked
                // for it.
                known.insert(
                    schedule.arn.clone(),
                    match schedule.next_after(now) {
                        Some(nominal) => armed_for(schedule, nominal),
                        None => exhausted(schedule, now),
                    },
                );
                continue;
            };
            let Some(due) = armed.due else {
                // A series with nothing left is remembered, and asked no more.
                known.insert(schedule.arn.clone(), armed.clone());
                continue;
            };
            if due > now {
                known.insert(schedule.arn.clone(), armed.clone());
                continue;
            }
            let nominal = armed.nominal;
            // The occurrence AFTER the one being performed is computed from the
            // occurrence's own instant, so the series keeps the cadence the
            // expression states rather than drifting by however long a delivery
            // took.
            known.insert(
                schedule.arn.clone(),
                match schedule.next_after(nominal) {
                    Some(next) => armed_for(schedule, next),
                    None => completed(schedule, nominal),
                },
            );
            firing.push(Firing {
                schedule: schedule.clone(),
                nominal,
            });
        }
        self.armed = known;
        firing
    }

    /// Every schedule whose series this clock has run out and whose COMPLETION the
    /// clock has already made due, by the schedule's own ARN: what
    /// [`apply_action_after_completion`] applies the stored action to.
    fn completed_by(&self, now: SystemTime) -> ::std::collections::BTreeSet<String> {
        self.armed
            .iter()
            .filter(|(_, armed)| {
                armed.due.is_none() && armed.completed.is_some_and(|at| at <= now)
            })
            .map(|(arn, _)| arn.clone())
            .collect()
    }

    /// The further attempts this clock has already made due, taken OFF it.
    ///
    /// An unfinished invocation is held only while it waits, so one whose attempt is
    /// being made is not handed out again, the way a due occurrence's instant moves
    /// before its delivery ([`Self::take_due`]).
    fn take_due_attempts(&mut self, now: SystemTime) -> Vec<Pending> {
        let (due, waiting): (Vec<Pending>, Vec<Pending>) = ::std::mem::take(&mut self.pending)
            .into_iter()
            .partition(|pending| pending.due <= now);
        self.pending = waiting;
        due
    }

    /// Hold `pending` until the attempt it is due is performed, answering whether the
    /// clock took it: one past [`MAX_PENDING_INVOCATIONS`] is not held, and its
    /// caller reports the failure instead of retrying it.
    fn hold(&mut self, pending: Pending) -> bool {
        if self.pending.len() >= MAX_PENDING_INVOCATIONS {
            return false;
        }
        self.pending.push(pending);
        true
    }
}

/// The clock entry for a schedule whose series names no occurrence at all — a
/// one-time instant already behind the clock, or a window that holds none.
///
/// Such a schedule has invoked nothing, so it has not "completed invoking the
/// target" at any instant of its own: the only one it completes at is the end of the
/// window `EndDate` closes, which may still be ahead of the clock.
fn exhausted(schedule: &Schedulable, now: SystemTime) -> Armed {
    Armed {
        definition: schedule.definition.clone(),
        nominal: now,
        due: None,
        completed: schedule.end.filter(|_| !schedule.is_one_time()),
    }
}

/// The clock entry for a schedule that has just run its LAST occurrence.
///
/// A one-time schedule completes at the occurrence it ran ("at expressions […]
/// invoke a target once"); a recurring one at the end of its own window, its last
/// occurrence being the one closest to that instant, so a schedule whose series ends
/// before `EndDate` is not complete until that date passes.
fn completed(schedule: &Schedulable, nominal: SystemTime) -> Armed {
    let completed = match schedule.end {
        Some(end) if !schedule.is_one_time() => end,
        _ => nominal,
    };
    Armed {
        definition: schedule.definition.clone(),
        nominal,
        due: None,
        completed: Some(completed),
    }
}

/// The clock entry for `nominal`: the occurrence's own instant, and the instant the
/// invocation for it is made at ([`invocation_instant`]).
fn armed_for(schedule: &Schedulable, nominal: SystemTime) -> Armed {
    Armed {
        definition: schedule.definition.clone(),
        nominal,
        due: Some(invocation_instant(schedule, nominal)),
        completed: None,
    }
}

/// WHEN the invocation for the occurrence at `nominal` is made.
///
/// At the occurrence itself for a schedule whose `FlexibleTimeWindow` asks for no
/// window, and inside `[nominal, nominal + MaximumWindowInMinutes]` for one that
/// does — "the maximum time window during which a schedule can be invoked". The
/// window opens AT the occurrence and extends forward only, so neither mode is ever
/// invoked before the instant its expression names.
fn invocation_instant(schedule: &Schedulable, nominal: SystemTime) -> SystemTime {
    match schedule.window {
        Some(window) => after(nominal, window_offset(&schedule.arn, nominal, window)),
        None => nominal,
    }
}

/// How far INTO its flexible window one occurrence of `arn` is invoked.
///
/// AWS picks an instant inside the window rather than its edge, and which instant is
/// the service's to choose; this spreads the occurrences of one store the same way,
/// from the schedule's own ARN and the occurrence's own instant, so a schedule's
/// invocation instant is reproducible and two schedules sharing a window do not
/// crowd onto one instant. The offset never exceeds the window.
fn window_offset(arn: &str, nominal: SystemTime, window: Duration) -> Duration {
    use ::std::hash::{Hash, Hasher};

    let span = u64::try_from(window.as_millis()).unwrap_or(u64::MAX);
    if span == 0 {
        return Duration::ZERO;
    }
    let mut hasher = ::std::collections::hash_map::DefaultHasher::new();
    arn.hash(&mut hasher);
    nominal
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .hash(&mut hasher);
    // The window is inclusive of both edges, so the offset may be the whole span.
    Duration::from_millis(hasher.finish() % (span + 1))
}

/// `at` plus `span`, saturating rather than panicking on a clock near the end of
/// its range.
fn after(at: SystemTime, span: Duration) -> SystemTime {
    at.checked_add(span).unwrap_or(at)
}

/// The DEFINITION a schedule's series is computed from: every member of it that
/// decides which instants it names, which target they reach and under which role.
///
/// A write that changes any of them re-arms the schedule, so a client that
/// re-points or re-expresses one is not served the series the schedule no longer
/// states.
fn definition_of(record: &ScheduleRecord) -> String {
    format!(
        "{}|{}|{:?}|{:?}|{}|{}|{}|{}|{:?}|{:?}",
        record.schedule_expression,
        record
            .schedule_expression_timezone
            .as_deref()
            .unwrap_or_default(),
        record.start_date.map(|at| (at.secs(), at.subsec_nanos())),
        record.end_date.map(|at| (at.secs(), at.subsec_nanos())),
        record.state,
        record.flexible_time_window.mode,
        record
            .flexible_time_window
            .maximum_window_in_minutes
            .unwrap_or_default(),
        record.target.arn,
        record.target.role_arn,
        record.target.input,
    )
}

/// Every schedule of this store that is on a clock, in the order the store holds
/// them.
fn schedulable(store: &ComAmazonawsSchedulerAwschronosserviceState) -> Vec<Schedulable> {
    store
        .schedules
        .values()
        .filter(|record| !store.group_is_deleting(&record.group_name))
        .filter_map(schedulable_record)
        .collect()
}

/// The schedulable form of one record, or `None` when it invokes nothing: a
/// `DISABLED` schedule, or an expression this emulator computes no occurrence from.
fn schedulable_record(record: &ScheduleRecord) -> Option<Schedulable> {
    if record.state == SCHEDULE_STATE_DISABLED {
        return None;
    }
    let occurrences = Occurrences::parse(
        &record.schedule_expression,
        record.schedule_expression_timezone.as_deref(),
    )?;
    Some(Schedulable {
        group: record.group_name.clone(),
        name: record.name.clone(),
        arn: record.arn.clone(),
        definition: definition_of(record),
        occurrences,
        start: record.start_date.map(wire_instant),
        end: record.end_date.map(wire_instant),
        window: flexible_window(&record.flexible_time_window),
    })
}

/// The window an occurrence of this schedule may be invoked inside, or `None` for a
/// schedule invoked at the occurrence itself.
///
/// A window is asked for by `Mode=FLEXIBLE` alone — "determines whether the schedule
/// is invoked within a flexible time window" — so `MaximumWindowInMinutes` under any
/// other mode names no window (a pair the write path refuses outright), and the
/// modelled range (1..1440) keeps the figure positive.
fn flexible_window(window: &FlexibleTimeWindow) -> Option<Duration> {
    if window.mode != super::FLEXIBLE_TIME_WINDOW_MODE_FLEXIBLE {
        return None;
    }
    let minutes = u64::try_from(window.maximum_window_in_minutes?).ok()?;
    Some(Duration::from_secs(minutes.saturating_mul(SECONDS_PER_MINUTE)))
}

/// Apply each completed schedule's `ActionAfterCompletion` — "the action that
/// EventBridge Scheduler applies to the schedule after the schedule completes
/// invoking the target".
///
/// `DELETE` removes the record, so the schedule stops resolving exactly as a deleted
/// one does; `NONE` (and an absent member) leaves it. Only a schedule the clock has
/// run out of occurrences for, at or past the instant it completed, is acted on
/// ([`Invocations::completed_by`]).
fn apply_action_after_completion(
    store: &mut ComAmazonawsSchedulerAwschronosserviceState,
    now: SystemTime,
) {
    let completed = store.invocations.completed_by(now);
    if completed.is_empty() {
        return;
    }
    let removed: Vec<(String, String)> = store
        .schedules
        .iter()
        .filter(|(_, record)| {
            record.action_after_completion.as_deref() == Some(ACTION_AFTER_COMPLETION_DELETE)
                && completed.contains(&record.arn)
        })
        .map(|(key, _)| key.clone())
        .collect();
    for key in removed {
        store.schedules.remove(&key);
    }
}

/// One stored wire timestamp as a clock reading.
fn wire_instant(at: ::aws_smithy_types::DateTime) -> SystemTime {
    SystemTime::try_from(at).unwrap_or(SystemTime::UNIX_EPOCH)
}

/// Start the one in-process task that invokes this store's schedules as the clock
/// makes them due, if one is not already running.
///
/// Called by every operation that WRITES a schedule, so a schedule is on a clock
/// from the moment a client asks for it, whether the client called `CreateSchedule`
/// itself or a stack deployed an `AWS::Scheduler::Schedule` that did. With no async
/// runtime to spawn onto there is no clock — which is the case in a unit test that
/// drives a handler directly, where nothing is watching a target either.
///
/// `callback` is the in-process emulator every invocation is made through, cloned
/// from the request that started the clock: the calls this clock makes reach the
/// same handlers a client's own calls reach.
pub fn drive_invocation_clock<Eng: Send + Sync + 'static>(
    engine: &::std::sync::Arc<Eng>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let now = SystemTime::now();
    let Some(generation) = state.lock().invocations.claim(now) else {
        return;
    };
    let callback = callback.clone();
    let claimed = ::engine::ServiceState::clone(&state);
    let engine = ::std::sync::Arc::clone(engine);
    let running = runtime.spawn(async move {
        for _ in 0..MAX_CLOCK_TICKS {
            ::tokio::time::sleep(CLOCK_TICK).await;
            let Some(state) = service_state(&*engine) else {
                return;
            };
            let now = SystemTime::now();
            let (firing, retrying) = {
                let mut store = state.lock();
                if !store.invocations.ticked(generation, now) {
                    return;
                }
                // Every group removal the clock already owed is materialised
                // first, so a schedule whose group has gone is not invoked.
                store.settle(now);
                let schedulable = schedulable(&store);
                let firing = store.invocations.take_due(&schedulable, now);
                (firing, store.invocations.take_due_attempts(now))
            };
            for occurrence in firing {
                invoke(&state, &callback, &occurrence).await;
            }
            // A further attempt of an invocation is made after this tick's own
            // occurrences, so a target that keeps refusing never holds up the
            // schedules whose occurrences are due beside it.
            for pending in retrying {
                attempt(&state, &callback, pending).await;
            }
            // A schedule's action is applied only AFTER the occurrences of this
            // tick have been handed to their targets: a schedule completes "after
            // the schedule completes invoking the target", so its last invocation
            // is made and only then is the action acted on.
            apply_action_after_completion(&mut state.lock(), SystemTime::now());
        }
        state.lock().invocations.release(generation);
    });
    claimed.lock().invocations.set_running(running);
}

/// Perform ONE occurrence: hand the schedule's target the rendered input, acting as
/// the schedule's execution role.
///
/// The record is read HERE, with the store locked only for as long as reading it
/// takes: an invocation reaches another service's handler, which takes that
/// service's own lock. What is read is what the invocation is OF — the target, and
/// the execution id that names this invocation — so every attempt of it afterwards
/// is of the same invocation ([`attempt`]).
async fn invoke(
    state: &::engine::ServiceState<ComAmazonawsSchedulerAwschronosserviceState>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    occurrence: &Firing,
) {
    let schedule = &occurrence.schedule;
    let (target, execution_id) = {
        let mut store = state.lock();
        let Some(record) = store
            .schedules
            .get(&(schedule.group.clone(), schedule.name.clone()))
            .filter(|record| record.state != SCHEDULE_STATE_DISABLED)
            .cloned()
        else {
            return;
        };
        let execution_id = store.invocations.next_execution_id();
        (record.target, execution_id)
    };
    let region = ::service_common::arn::region_of(&schedule.arn, "scheduler")
        .unwrap_or(FALLBACK_REGION)
        .to_string();
    let began = SystemTime::now();
    attempt(
        state,
        callback,
        Pending {
            schedule_arn: schedule.arn.clone(),
            region,
            target,
            nominal: occurrence.nominal,
            execution_id,
            began,
            attempts: 0,
            due: began,
        },
    )
    .await;
}

/// Make ONE attempt of an invocation, and settle what becomes of it.
///
/// The input is rendered for THIS attempt, because the attempt-number context
/// attribute is the attempt being made. A failure the schedule's retry policy still
/// admits an attempt after is put back on the clock, due one [`RETRY_INTERVAL`] on; a
/// failure it does not is reported to the schedule's dead-letter queue
/// ([`dead_letter`]). Nothing sleeps for the wait between two attempts: the due
/// instant is stored and the clock performs the attempt it has already made due.
async fn attempt(
    state: &::engine::ServiceState<ComAmazonawsSchedulerAwschronosserviceState>,
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    mut pending: Pending,
) {
    pending.attempts = pending.attempts.saturating_add(1);
    let body = rendered_input(
        &pending.target.input,
        &pending.schedule_arn,
        pending.nominal,
        &pending.execution_id,
        pending.attempts,
    );
    let failure = match perform(callback, &pending, &body).await {
        Attempt::Delivered => return,
        Attempt::NotAttempted(why) => {
            ::tracing::warn!(
                schedule = %pending.schedule_arn,
                target = %pending.target.arn,
                %why,
                "an EventBridge Scheduler invocation was not attempted"
            );
            return;
        }
        Attempt::Failed(failure) => failure,
    };
    // The attempts already made BEYOND the first are the retries the policy counts.
    let retries_made = pending.attempts.saturating_sub(1);
    let next = after(SystemTime::now(), RETRY_INTERVAL);
    let exhausted = if failure.retryable {
        RetryBounds::of(pending.target.retry_policy.as_ref()).exhausted(
            retries_made,
            next.duration_since(pending.began).unwrap_or_default(),
        )
    } else {
        // A refusal of the request itself spends neither bound: it is the
        // invocation's last attempt however many the policy would have admitted.
        None
    };
    if failure.retryable && exhausted.is_none() {
        let mut held = pending.clone();
        held.due = next;
        if state.lock().invocations.hold(held) {
            return;
        }
    }
    // What is reported is the failed EVENT — the request the attempt carried to the
    // target's own API ([`failed_event`]) — rather than the bare payload the target
    // was to be handed.
    let event = failed_event(&pending.target, &pending.region, &body);
    dead_letter(callback, &pending, &event, &failure, exhausted, retries_made).await;
}

/// WHAT one occurrence calls, which is what the target's own ARN names.
#[derive(Debug)]
enum Called {
    /// A TEMPLATED target: the ARN names a resource, and the call is the one API that
    /// kind of resource is invoked by ([`invoking_action`]).
    Templated(&'static str),
    /// A UNIVERSAL target: the ARN names the operation itself, and the call is that
    /// operation with the invocation's payload as its request. Held behind an
    /// indirection, because a resolved call carries the invoked operation's whole
    /// request while the other kind carries one action name.
    Universal(Box<UniversalCall>),
}

/// Make one attempt's calls: the permissions the invocation is made under, and then
/// the API the target names.
async fn perform(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    pending: &Pending,
    body: &str,
) -> Attempt {
    let arn = pending.target.arn.as_str();
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return Attempt::NotAttempted(format!("the target ARN {arn} names no resource"));
    };
    // The target is reached in ITS OWN Region — the one its ARN names — falling back
    // to the schedule's when the ARN carries no Region label, which is every universal
    // target: such an ARN names a call rather than a resource in a Region.
    let region = ::service_common::arn::region_of(arn, parsed.vendor).unwrap_or(&pending.region);
    let called = if universal_target::names_universal_target(&parsed) {
        match universal_target::universal_call(&parsed, body) {
            Ok(call) => Called::Universal(Box::new(call)),
            Err(why) => return Attempt::NotAttempted(why),
        }
    } else {
        match invoking_action(parsed.vendor) {
            Some(action) => Called::Templated(action),
            None => {
                return Attempt::NotAttempted(format!(
                    "this emulator does not invoke a target of {}",
                    parsed.vendor
                ))
            }
        }
    };
    // The role is assumed BEFORE anything is invoked: "EventBridge Scheduler will
    // use [this role] for this target when the schedule is invoked", so a role this
    // account no longer holds, or one whose trust policy no longer admits the
    // scheduler, invokes nothing.
    if let Some(refusal) =
        super::execution_role_violation(callback, &pending.region, &pending.target.role_arn).await
    {
        return Attempt::Failed(withheld(ACCESS_DENIED_CODE, refusal));
    }
    // The access request the call is subject to: a templated target names the resource
    // the call acts on ITSELF, while a universal target's REQUEST names it, and a
    // request naming none leaves the decision to the invoked operation.
    let access = match &called {
        Called::Templated(action) => Some(((*action).to_string(), arn.to_string())),
        Called::Universal(call) => call.access_request(region),
    };
    if let Some((action, resource)) = access {
        if let Some(failure) = withheld_action(callback, pending, &action, &resource).await {
            return Attempt::Failed(failure);
        }
    }
    match called {
        Called::Templated(_) => deliver(callback, region, &parsed, &pending.target, body).await,
        Called::Universal(call) => call.invoke(callback, region).await,
    }
}

/// The IAM action a target of this kind's invocation is authorized by, or `None` for
/// a kind this emulator does not invoke.
fn invoking_action(vendor: &str) -> Option<&'static str> {
    match vendor {
        SQS_ARN_NAMESPACE => Some(SEND_MESSAGE_ACTION),
        SNS_ARN_NAMESPACE => Some(PUBLISH_ACTION),
        LAMBDA_ARN_NAMESPACE => Some(INVOKE_FUNCTION_ACTION),
        STATES_ARN_NAMESPACE => Some(START_EXECUTION_ACTION),
        _ => None,
    }
}

/// What separates an IAM action's service from its operation: an action is
/// `<service>:<Operation>`.
const ACTION_SEPARATOR: char = ':';

/// Amazon SNS's IAM action prefix, which is the case that service names an action in:
/// "an SNS action is `SNS:<OperationName>`", the spelling its own refusals use.
const SNS_ACTION_PREFIX: &str = "SNS";

/// WHICH service answered a refusal of one invocation's call, for the two whose answer
/// has a code or a wording of its own.
///
/// Read off the IAM action that call is authorized by — an action's prefix is the
/// service's own namespace ([`ACTION_SEPARATOR`]) — so one reading serves a templated
/// target and a universal one alike. The comparison ignores case, because the case an
/// action is spelled in is each service's own wording and never part of the decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Refusing {
    /// Amazon SQS.
    Sqs,
    /// Amazon SNS.
    Sns,
    /// A service whose refusal is AWS's authorization layer's own, in both its code and
    /// its wording.
    Layer,
}

/// The service `action` is of ([`Refusing`]).
fn refusing_service(action: &str) -> Refusing {
    let service = action.split(ACTION_SEPARATOR).next().unwrap_or_default();
    if service.eq_ignore_ascii_case(SQS_ARN_NAMESPACE) {
        return Refusing::Sqs;
    }
    if service.eq_ignore_ascii_case(SNS_ARN_NAMESPACE) {
        return Refusing::Sns;
    }
    Refusing::Layer
}

/// The error code the service an invocation's call is made to answers a call its caller
/// is not authorized to make with.
///
/// AWS's authorization layer answers in the refused service's own terms, so the code is
/// the one that service publishes for such an answer
/// ([`QUERY_ACCESS_DENIED_CODE`], [`AUTHORIZATION_ERROR_CODE`], [`ACCESS_DENIED_CODE`]).
fn authorization_refusal_code(action: &str) -> &'static str {
    match refusing_service(action) {
        Refusing::Sqs => QUERY_ACCESS_DENIED_CODE,
        Refusing::Sns => AUTHORIZATION_ERROR_CODE,
        Refusing::Layer => ACCESS_DENIED_CODE,
    }
}

/// The action as a refusal of the service that ANSWERED it names it, which is that
/// service's own wording rather than the spelling the decision was taken on: IAM compares
/// an action name without regard to its case, so the case a refusal reports is a wording
/// choice of the refusing service.
///
/// Each arm is the rule the service that owns the refusal states for its own: Amazon SQS
/// names the action case-folded throughout (`sqs:sendmessage`), Amazon SNS names it
/// `SNS:<OperationName>` ([`SNS_ACTION_PREFIX`]), and a refusal AWS's authorization layer
/// answers for a service names the action as that service documents it.
fn reported_action(action: &str) -> String {
    match refusing_service(action) {
        Refusing::Sqs => action.to_lowercase(),
        Refusing::Sns => action.split_once(ACTION_SEPARATOR).map_or_else(
            || action.to_string(),
            |(_, operation)| format!("{SNS_ACTION_PREFIX}{ACTION_SEPARATOR}{operation}"),
        ),
        Refusing::Layer => action.to_string(),
    }
}

/// How the refusal of a call the invocation's own permissions withhold is WORDED: AWS's
/// authorization refusal, naming the principal the call was made by — the session this
/// invocation assumed the execution role under — the action as the refusing service names
/// it ([`reported_action`]) and the resource it was on.
fn authorization_refusal_message(pending: &Pending, action: &str, resource: &str) -> String {
    let named = reported_action(action);
    ::service_common::iam_authorization::not_authorized_by_identity_policy(
        &invocation_principal(&pending.target.role_arn, &pending.execution_id),
        &named,
        resource,
        &::service_common::iam_authorization::no_identity_policy_allows(&named),
    )
}

/// The failure a call the invocation's own permissions withhold comes to, or `None`
/// when nothing withholds it.
///
/// `Target$RoleArn` is "the IAM role that EventBridge Scheduler will use for this
/// target when the schedule is invoked", so the call is made UNDER that role and is
/// authorized against ITS permissions rather than against those of whoever wrote the
/// schedule. The question goes to the one reading of it this tree has
/// ([`::service_common::iam_role::role_withholds_action_through`]), which withholds
/// nothing unless IAM answered that the role's policies do — so an emulator with no
/// IAM to ask invokes the target, nothing having been established about the role.
async fn withheld_action(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    pending: &Pending,
    action: &str,
    resource: &str,
) -> Option<Failure> {
    let role_arn = pending.target.role_arn.as_str();
    if !::service_common::iam_role::role_withholds_action_through(
        callback,
        &pending.region,
        role_arn,
        action,
        resource,
    )
    .await
    {
        return None;
    }
    Some(withheld(
        authorization_refusal_code(action),
        authorization_refusal_message(pending, action, resource),
    ))
}

/// The separator the form an execution id is REPORTED in carries, and which a session
/// name does not: a session is named by an opaque token rather than by that form.
const EXECUTION_ID_SEPARATOR: char = '-';

/// The principal one invocation's calls are made by: the session the schedule's
/// execution role is assumed under, which is what an authorization refusal of such a
/// call names. The session is the invocation's own, so two invocations of one
/// schedule are not one session.
///
/// The session is named by a token of that invocation — the characters of its own
/// execution id, without the separators of the grouped form that id is REPORTED in
/// ([`dead_letter_attributes`]) — so the name is this invocation's and no other's, and
/// is the opaque run of characters a session name is.
fn invocation_principal(role_arn: &str, execution_id: &str) -> String {
    let execution_id = execution_id.replace(EXECUTION_ID_SEPARATOR, "");
    let parsed = ::service_common::arn::parse_of(role_arn, "iam");
    let name = ::service_common::iam_role::role_name(role_arn);
    match (parsed, name) {
        (Some(parsed), Some(name)) => format!(
            "arn:{}:sts::{}:assumed-role/{name}/{execution_id}",
            parsed.partition, parsed.account
        ),
        // A value that names no role of this account is named as the schedule
        // stated it: there is no session to name instead.
        _ => role_arn.to_string(),
    }
}

/// The members of a templated target's own API request: the one that ADDRESSES the
/// target, and the one that carries the payload the schedule hands it.
///
/// Each pair is the one that API publishes for its request — Amazon SQS's `SendMessage`
/// takes `QueueUrl`, `MessageBody` and `MessageGroupId`, Amazon SNS's `Publish` takes
/// `TopicArn` and `Message`, AWS Lambda's `Invoke` takes `FunctionName` and `Payload`,
/// and Step Functions' `StartExecution` takes `StateMachineArn` and `Input` — spelled
/// the way a UNIVERSAL target's own input spells the same call
/// ([`super::universal_target`]), so one call is written one way whichever kind of
/// target names it.
const QUEUE_URL_MEMBER: &str = "QueueUrl";
/// See [`QUEUE_URL_MEMBER`].
const MESSAGE_BODY_MEMBER: &str = "MessageBody";
/// See [`QUEUE_URL_MEMBER`].
const MESSAGE_GROUP_ID_MEMBER: &str = "MessageGroupId";
/// See [`QUEUE_URL_MEMBER`].
const TOPIC_ARN_MEMBER: &str = "TopicArn";
/// See [`QUEUE_URL_MEMBER`].
const MESSAGE_MEMBER: &str = "Message";
/// See [`QUEUE_URL_MEMBER`].
const FUNCTION_NAME_MEMBER: &str = "FunctionName";
/// See [`QUEUE_URL_MEMBER`].
const PAYLOAD_MEMBER: &str = "Payload";
/// See [`QUEUE_URL_MEMBER`].
const STATE_MACHINE_ARN_MEMBER: &str = "StateMachineArn";
/// See [`QUEUE_URL_MEMBER`].
const EXECUTION_INPUT_MEMBER: &str = "Input";

/// The FAILED EVENT one undelivered invocation IS: the request the attempt carried to
/// the target's own API, with `payload` where that request carries it.
///
/// `Target$DeadLetterConfig` is the queue "EventBridge Scheduler delivers failed events
/// that could not be successfully delivered to a target" to, and what an invocation
/// delivers is a CALL: every target names one, a UNIVERSAL target naming it outright
/// and a templated target naming one API operation of the target's service —
/// `SqsParameters` is "the templated target type for the Amazon SQS `SendMessage` API
/// operation" — of whose request the target it addresses, `Target$Input` ("the text, or
/// well-formed JSON, passed to the target") and that target's own parameters are the
/// members. So the failed event is that request's own document, written the way a
/// universal target's input writes the same call: a templated target's is built here
/// from the members the schedule states, and a universal target's input already IS the
/// request, so it is dead-lettered as it stands.
///
/// `region` is the schedule's, for an ARN carrying no Region label of its own — the same
/// fallback the invocation itself is made in ([`perform`]).
fn failed_event(target: &Target, region: &str, payload: &str) -> String {
    let text = |value: &str| ::serde_json::Value::String(value.to_string());
    let arn = target.arn.as_str();
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return payload.to_string();
    };
    if universal_target::names_universal_target(&parsed) {
        return payload.to_string();
    }
    let region = ::service_common::arn::region_of(arn, parsed.vendor).unwrap_or(region);
    let mut request = ::serde_json::Map::new();
    match parsed.vendor {
        SQS_ARN_NAMESPACE => {
            // A queue is addressed by its url rather than by its ARN, which is the one
            // address this request can carry ([`queue_url`]).
            let Some(url) = queue_url(region, parsed.account, parsed.resource) else {
                return payload.to_string();
            };
            request.insert(QUEUE_URL_MEMBER.to_string(), text(&url));
            request.insert(MESSAGE_BODY_MEMBER.to_string(), text(payload));
            // "The FIFO message group ID to use as the target" travels with the
            // invocation ([`deliver`]), so it is part of the request that failed.
            if let Some(group) = target
                .sqs_parameters
                .as_ref()
                .and_then(|parameters| parameters.message_group_id.as_deref())
            {
                request.insert(MESSAGE_GROUP_ID_MEMBER.to_string(), text(group));
            }
        }
        SNS_ARN_NAMESPACE => {
            request.insert(TOPIC_ARN_MEMBER.to_string(), text(arn));
            request.insert(MESSAGE_MEMBER.to_string(), text(payload));
        }
        LAMBDA_ARN_NAMESPACE => {
            request.insert(FUNCTION_NAME_MEMBER.to_string(), text(arn));
            request.insert(PAYLOAD_MEMBER.to_string(), text(payload));
        }
        STATES_ARN_NAMESPACE => {
            request.insert(STATE_MACHINE_ARN_MEMBER.to_string(), text(arn));
            request.insert(EXECUTION_INPUT_MEMBER.to_string(), text(payload));
        }
        // A kind this emulator does not invoke, no call of which ever fails: nothing
        // is attempted for it, so nothing of it is dead-lettered either.
        _ => return payload.to_string(),
    }
    ::serde_json::Value::Object(request).to_string()
}

/// Report an invocation that could not be delivered to the queue the schedule names
/// for one: "EventBridge Scheduler delivers failed events that could not be
/// successfully delivered to a target to the queue".
///
/// The message body is `event`, the failed event the invocation could not deliver
/// ([`failed_event`]), and its attributes are the failure's own detail
/// ([`dead_letter_attributes`]). The send is made under the schedule's execution role
/// as well — a dead-letter queue is reached by the same `sqs:SendMessage` a queue
/// target is, which is the grant AWS documents for one — so a role that may not write
/// to that queue leaves the failure in this emulator's log alone, which is also all a
/// schedule naming no such queue leaves behind.
async fn dead_letter(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    pending: &Pending,
    event: &str,
    failure: &Failure,
    exhausted: Option<Exhaustion>,
    retries: u32,
) {
    let code = failure.code.as_str();
    let detail = failure.message.as_str();
    let Some(queue_arn) = pending
        .target
        .dead_letter_config
        .as_ref()
        .and_then(|config| config.arn.as_deref())
        .filter(|arn| !arn.is_empty())
    else {
        ::tracing::warn!(
            schedule = %pending.schedule_arn,
            target = %pending.target.arn,
            %code,
            %detail,
            "an EventBridge Scheduler invocation failed and its schedule names no dead-letter \
             queue"
        );
        return;
    };
    let region =
        ::service_common::arn::region_of(queue_arn, SQS_ARN_NAMESPACE).unwrap_or(&pending.region);
    let url = ::service_common::arn::parse_of(queue_arn, SQS_ARN_NAMESPACE)
        .and_then(|parsed| queue_url(region, parsed.account, parsed.resource));
    let Some(url) = url else {
        ::tracing::warn!(
            schedule = %pending.schedule_arn,
            dead_letter_queue = %queue_arn,
            "an EventBridge Scheduler invocation was not dead-lettered: the dead-letter ARN \
             names no queue"
        );
        return;
    };
    if let Some(withheld) = withheld_action(callback, pending, SEND_MESSAGE_ACTION, queue_arn).await
    {
        ::tracing::warn!(
            schedule = %pending.schedule_arn,
            dead_letter_queue = %queue_arn,
            refusal = %withheld.message,
            "an EventBridge Scheduler invocation was not dead-lettered: its execution role may \
             not write to the dead-letter queue"
        );
        return;
    }
    let scheduled_time = scheduled_time_text(pending.nominal);
    let attributes = dead_letter_attributes(&Report {
        schedule_arn: &pending.schedule_arn,
        target_arn: &pending.target.arn,
        execution_id: &pending.execution_id,
        scheduled_time: &scheduled_time,
        retries,
        exhausted,
        failure,
    });
    let mut request = sqs_client(callback, region)
        .send_message()
        .queue_url(url)
        .message_body(event);
    for attribute in attributes {
        let value = ::aws_sdk_sqs::types::MessageAttributeValue::builder()
            .data_type(attribute.data_type)
            .string_value(attribute.value)
            .build();
        match value {
            Ok(value) => request = request.message_attributes(attribute.name, value),
            Err(error) => ::tracing::warn!(
                attribute = %attribute.name,
                %error,
                "a dead-letter message attribute could not be built"
            ),
        }
    }
    if let Err(error) = request.send().await {
        ::tracing::warn!(
            schedule = %pending.schedule_arn,
            dead_letter_queue = %queue_arn,
            error = ?error,
            "an EventBridge Scheduler invocation was not dead-lettered: the dead-letter queue \
             did not accept it"
        );
    }
}

/// Hand `body` to the target `target.arn` names, through the one API that target's
/// service is invoked by, and answer what the call came to.
async fn deliver(
    callback: &::smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    parsed: &::service_common::arn::Arn<'_>,
    target: &Target,
    body: &str,
) -> Attempt {
    let arn = target.arn.as_str();
    match parsed.vendor {
        SQS_ARN_NAMESPACE => {
            let Some(queue_url) = queue_url(region, parsed.account, parsed.resource) else {
                return Attempt::NotAttempted(format!("the target ARN {arn} names no queue"));
            };
            let mut request = sqs_client(callback, region)
                .send_message()
                .queue_url(queue_url)
                .message_body(body);
            // "The FIFO message group ID to use as the target" (`SqsParameters`):
            // such a queue takes no message without one, so the group the target
            // names travels with the invocation.
            if let Some(group) = target
                .sqs_parameters
                .as_ref()
                .and_then(|parameters| parameters.message_group_id.as_deref())
            {
                request = request.message_group_id(group);
            }
            answered(request.send().await)
        }
        SNS_ARN_NAMESPACE => answered(
            sns_client(callback, region)
                .publish()
                .topic_arn(arn)
                .message(body)
                .send()
                .await,
        ),
        LAMBDA_ARN_NAMESPACE => answered(
            lambda_client(callback, region)
                .invoke()
                .function_name(arn)
                // A schedule does not wait for its function target's answer.
                .invocation_type(::aws_sdk_lambda::types::InvocationType::Event)
                .payload(::aws_sdk_lambda::primitives::Blob::new(body.as_bytes()))
                .send()
                .await,
        ),
        STATES_ARN_NAMESPACE => answered(
            sfn_client(callback, region)
                .start_execution()
                .state_machine_arn(arn)
                .input(body)
                .send()
                .await,
        ),
        vendor => Attempt::NotAttempted(format!(
            "this emulator does not invoke a target of {vendor}"
        )),
    }
}

/// What one target API's answer came to: a delivery, or the failure it reported
/// ([`failure_of`]).
fn answered<T, E>(
    answer: Result<
        T,
        ::aws_smithy_runtime_api::client::result::SdkError<
            E,
            ::aws_smithy_runtime_api::client::orchestrator::HttpResponse,
        >,
    >,
) -> Attempt
where
    E: ::aws_smithy_types::error::metadata::ProvideErrorMetadata + ::std::error::Error + 'static,
{
    match answer {
        Ok(_) => Attempt::Delivered,
        Err(error) => Attempt::Failed(failure_of(&error)),
    }
}

/// The `QueueUrl` Amazon SQS addresses the queue of `name` in `account` by, or
/// `None` when the ARN's fields name no queue.
///
/// A queue's ARN names the queue DIRECTLY
/// (`arn:<partition>:sqs:<region>:<account>:<QueueName>`), and the url every SQS
/// operation takes is `<endpoint>/<account>/<QueueName>`. A resource field carrying
/// a type separator addresses no queue, and an account field that is not an account
/// id names none either — so neither reaches the url, which is what keeps a
/// caller-written ARN from reshaping the address this invocation is made to. The
/// Region in the host is the one the shared ARN reader already held to the Region
/// grammar.
fn queue_url(region: &str, account: &str, name: &str) -> Option<String> {
    /// How many digits an account id carries.
    const ACCOUNT_ID_DIGITS: usize = 12;

    let account_names_an_account =
        account.len() == ACCOUNT_ID_DIGITS && account.bytes().all(|byte| byte.is_ascii_digit());
    if !account_names_an_account || name.contains('/') || name.contains(':') {
        return None;
    }
    Some(format!("https://sqs.{region}.amazonaws.com/{account}/{name}"))
}

/// Build one client over the IN-PROCESS callback into this same emulator, against
/// the target service's endpoint in `region`.
///
/// The transport is in-process, so each client disables what a network client needs
/// and this one cannot have: retries, timeouts, stalled-stream protection and
/// identity caching each need a sleep implementation the callback transport is not
/// given, and there is nothing to retry behind it anyway.
macro_rules! callback_client {
    ($name:ident, $sdk:ident, $endpoint:literal) => {
        fn $name(
            callback: &::smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> ::$sdk::Client {
            let config = ::$sdk::Config::builder()
                .behavior_version(::$sdk::config::BehaviorVersion::latest())
                .http_client(callback.http_client())
                .credentials_provider(callback.aws_credentials_provider())
                .region(::$sdk::config::Region::new(region.to_string()))
                .endpoint_url(format!("https://{}.{region}.amazonaws.com", $endpoint))
                .retry_config(::$sdk::config::retry::RetryConfig::disabled())
                .timeout_config(::$sdk::config::timeout::TimeoutConfig::disabled())
                .identity_cache(::$sdk::config::IdentityCache::no_cache())
                .stalled_stream_protection(
                    ::$sdk::config::StalledStreamProtectionConfig::disabled(),
                )
                .build();
            ::$sdk::Client::from_conf(config)
        }
    };
}

callback_client!(sqs_client, aws_sdk_sqs, "sqs");
callback_client!(sns_client, aws_sdk_sns, "sns");
callback_client!(lambda_client, aws_sdk_lambda, "lambda");
// Step Functions publishes its endpoint and signs under `states`.
callback_client!(sfn_client, aws_sdk_sfn, "states");

/// The text one occurrence hands its target: the target's own `Input` with the
/// schedule's context attributes substituted, or [`DEFAULT_INPUT`] for a target that
/// states none.
///
/// "The text, or well-formed JSON, passed to the target" travels VERBATIM —
/// nothing of the schedule is folded into it — except for the context attributes
/// EventBridge Scheduler substitutes, which are the schedule's ARN, the
/// occurrence's own scheduled time, this invocation's execution id and the attempt
/// it is. `attempt` is the attempt being MADE, counted from one, so what a retried
/// invocation hands its target says which attempt reached it.
#[must_use]
pub fn rendered_input(
    input: &Option<String>,
    schedule_arn: &str,
    scheduled_time: SystemTime,
    execution_id: &str,
    attempt: u32,
) -> String {
    let Some(input) = input.as_deref().filter(|input| !input.is_empty()) else {
        return DEFAULT_INPUT.to_string();
    };
    input
        .replace(SCHEDULE_ARN_ATTRIBUTE, schedule_arn)
        .replace(SCHEDULED_TIME_ATTRIBUTE, &scheduled_time_text(scheduled_time))
        .replace(EXECUTION_ID_ATTRIBUTE, execution_id)
        .replace(ATTEMPT_NUMBER_ATTRIBUTE, &attempt.to_string())
}

/// One occurrence's instant as the scheduled-time attribute carries it.
#[must_use]
pub fn scheduled_time_text(at: SystemTime) -> String {
    ::chrono::DateTime::<::chrono::Utc>::from(at)
        .format(SCHEDULED_TIME_FORMAT)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One row of the window table: the case, the expression, the `StartDate` and
    /// `EndDate` the schedule states as seconds from the epoch, and the occurrences
    /// the pair leaves it.
    type WindowRow = (
        &'static str,
        &'static str,
        Option<u64>,
        Option<u64>,
        &'static [u64],
    );

    /// A clock reading `seconds` after the epoch.
    fn epoch(seconds: u64) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)
    }

    /// One schedule as the clock reads it, from the members that decide its series:
    /// the expression, the zone it is read in, the window its instants are bounded
    /// by, and the flexible window one occurrence may be invoked inside.
    fn record(
        expression: &str,
        timezone: Option<&str>,
        start: Option<u64>,
        end: Option<u64>,
        window_minutes: Option<i32>,
    ) -> ScheduleRecord {
        let at = |seconds: u64| ::aws_smithy_types::DateTime::from_secs(seconds as i64);
        ScheduleRecord {
            name: "nightly".to_string(),
            group_name: "default".to_string(),
            arn: "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly".to_string(),
            schedule_expression: expression.to_string(),
            schedule_expression_timezone: timezone.map(ToString::to_string),
            start_date: start.map(at),
            end_date: end.map(at),
            description: None,
            state: "ENABLED".to_string(),
            kms_key_arn: None,
            action_after_completion: None,
            target: Target::default(),
            flexible_time_window: FlexibleTimeWindow {
                mode: match window_minutes {
                    Some(_) => "FLEXIBLE".to_string(),
                    None => "OFF".to_string(),
                },
                maximum_window_in_minutes: window_minutes,
            },
            creation_date: SystemTime::UNIX_EPOCH,
            last_modification_date: SystemTime::UNIX_EPOCH,
        }
    }

    /// The first `count` invocations the clock makes for `record` from `now`: each
    /// occurrence's own instant paired with the instant the invocation for it is
    /// made at, stepped exactly as [`Invocations::take_due`] steps the series.
    fn planned(
        record: &ScheduleRecord,
        now: SystemTime,
        count: usize,
    ) -> Vec<(SystemTime, SystemTime)> {
        let Some(schedule) = schedulable_record(record) else {
            return Vec::new();
        };
        let mut planned = Vec::new();
        let mut from = now;
        for _ in 0..count {
            let Some(nominal) = schedule.next_after(from) else {
                break;
            };
            planned.push((nominal, invocation_instant(&schedule, nominal)));
            from = nominal;
        }
        planned
    }

    /// A series runs on the cadence its own expression states, against the server
    /// clock: the rows differ along the FORM of the expression, the rate value and
    /// unit, and the zone a cron expression is read in.
    #[test]
    fn a_series_runs_on_the_cadence_its_expression_states() {
        // 2030-01-01T00:00:30Z: half a minute past a minute boundary, so a cron
        // expression's own instants are not the reading itself.
        let now = epoch(1_893_456_030);
        // (case, expression, zone, the occurrences as seconds from the epoch)
        let rows: [(&str, &str, Option<&str>, &[u64]); 8] = [
            (
                "a rate of one minute",
                "rate(1 minute)",
                None,
                &[1_893_456_090, 1_893_456_150, 1_893_456_210],
            ),
            (
                "a rate of two minutes",
                "rate(2 minutes)",
                None,
                &[1_893_456_150, 1_893_456_270, 1_893_456_390],
            ),
            (
                "a rate of fifteen minutes",
                "rate(15 minutes)",
                None,
                &[1_893_456_930, 1_893_457_830],
            ),
            (
                "a rate of hours",
                "rate(2 hours)",
                None,
                &[1_893_463_230, 1_893_470_430],
            ),
            (
                "a rate of days",
                "rate(1 day)",
                None,
                &[1_893_542_430, 1_893_628_830],
            ),
            (
                "a cron naming every minute",
                "cron(* * * * ? *)",
                None,
                &[1_893_456_060, 1_893_456_120],
            ),
            (
                // 08:00 in UTC+05:30 is 02:30 UTC on that date.
                "a cron read in an eastern zone",
                "cron(0 8 * * ? *)",
                Some("Asia/Kolkata"),
                &[1_893_456_000 + 2 * 3_600 + 1_800],
            ),
            (
                "a one-time instant names one occurrence",
                "at(2030-01-01T01:00:00)",
                None,
                &[1_893_456_000 + 3_600],
            ),
        ];
        for (case, expression, zone, expected) in rows {
            let planned = planned(&record(expression, zone, None, None, None), now, expected.len());
            let nominals: Vec<u64> = planned
                .iter()
                .map(|(nominal, _)| {
                    nominal
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                })
                .collect();
            assert_eq!(
                nominals,
                expected.to_vec(),
                "case {case:?}: expected the occurrences at {expected:?}, got {nominals:?}"
            );
        }
    }

    /// No occurrence is invoked BEFORE the instant its expression names, and one of a
    /// schedule asking for a flexible window is invoked inside that window: the rows
    /// differ along the mode and along the window's own length, including each
    /// published bound of `MaximumWindowInMinutes` (1..1440).
    #[test]
    fn an_occurrence_is_invoked_inside_the_window_that_opens_at_it() {
        let now = epoch(1_893_456_030);
        // (case, the window the schedule asks for in minutes)
        let rows: [(&str, Option<i32>); 5] = [
            ("no window at all", None),
            ("the shortest window", Some(1)),
            ("a window of five minutes", Some(5)),
            ("a window of an hour", Some(60)),
            ("the longest window", Some(1_440)),
        ];
        for (case, window_minutes) in rows {
            let window = Duration::from_secs(
                u64::try_from(window_minutes.unwrap_or_default()).unwrap_or_default() * 60,
            );
            // Every form of expression is held to the same rule, so each row runs
            // over all three.
            for expression in [
                "rate(1 minute)",
                "cron(0 8 * * ? *)",
                "at(2030-01-01T01:00:00)",
            ] {
                let record = record(expression, None, None, None, window_minutes);
                for (nominal, invoked) in planned(&record, now, 3) {
                    assert!(
                        invoked >= nominal,
                        "case {case:?} on {expression:?}: an occurrence at {nominal:?} was \
                         invoked at {invoked:?}, before the instant it names"
                    );
                    let spread = invoked.duration_since(nominal).unwrap_or_default();
                    assert!(
                        spread <= window,
                        "case {case:?} on {expression:?}: an occurrence at {nominal:?} was \
                         invoked {spread:?} into a window of {window:?}"
                    );
                }
            }
        }
    }

    /// `StartDate` is a floor on the series and `EndDate` its ceiling, for a
    /// recurring schedule; a one-time instant is bounded by neither. The rows differ
    /// along which boundary the schedule states and along the form of the expression.
    #[test]
    fn the_schedules_own_window_bounds_its_occurrences() {
        let now = epoch(1_893_456_030);
        // (case, expression, start, end, the occurrences as seconds from the epoch)
        let rows: [WindowRow; 7] = [
            (
                "a rate series opens at its StartDate",
                "rate(1 minute)",
                Some(1_893_459_630),
                None,
                &[1_893_459_630, 1_893_459_690],
            ),
            (
                "a cron series opens at the first instant at or after its StartDate",
                "cron(0 8 * * ? *)",
                // 09:00 UTC, an hour after that day's own occurrence.
                Some(1_893_488_400),
                None,
                // 08:00 UTC the next day, the first the floor admits.
                &[1_893_571_200],
            ),
            (
                "a StartDate behind the clock bounds nothing",
                "rate(1 minute)",
                Some(1_893_452_430),
                None,
                &[1_893_456_090, 1_893_456_150],
            ),
            (
                "a recurring series stops before its EndDate",
                "rate(1 minute)",
                None,
                Some(1_893_456_180),
                &[1_893_456_090, 1_893_456_150],
            ),
            (
                "an EndDate that admits no occurrence at all",
                "rate(1 minute)",
                None,
                Some(1_893_456_040),
                &[],
            ),
            (
                // "EventBridge Scheduler ignores StartDate and EndDate for one-time
                // schedules."
                "a one-time instant ignores a StartDate after it",
                "at(2030-01-01T01:00:00)",
                Some(1_893_492_000),
                None,
                &[1_893_459_600],
            ),
            (
                "a one-time instant ignores an EndDate before it",
                "at(2030-01-01T01:00:00)",
                None,
                Some(1_893_456_090),
                &[1_893_459_600],
            ),
        ];
        for (case, expression, start, end, expected) in rows {
            let record = record(expression, None, start, end, None);
            let planned = planned(&record, now, expected.len().max(1));
            let nominals: Vec<u64> = planned
                .iter()
                .map(|(nominal, _)| {
                    nominal
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                })
                .collect();
            assert_eq!(
                nominals,
                expected.to_vec(),
                "case {case:?}: expected the occurrences at {expected:?}, got {nominals:?}"
            );
        }
    }

    /// The substituted input is the target's own text with the schedule's context
    /// attributes replaced, and a target stating no input is handed the default. The
    /// attempt the invocation is carries into it, so the rows differ along the
    /// attribute substituted and along the attempt it is substituted for.
    #[test]
    fn an_input_carries_the_schedules_context_attributes() {
        let arn = "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly";
        let at = epoch(1_893_456_000);
        // (case, the target's Input, the attempt, what the target is handed)
        let rows: [(&str, Option<&str>, u32, &str); 8] = [
            ("no input at all", None, 1, "{}"),
            ("an empty input", Some(""), 1, "{}"),
            ("text carrying no attribute", Some("a marker"), 1, "a marker"),
            (
                "the scheduled time",
                Some(r#"{"at":"<aws.scheduler.scheduled-time>"}"#),
                1,
                r#"{"at":"2030-01-01T00:00:00Z"}"#,
            ),
            (
                "the execution id and the attempt",
                Some(r#"{"id":"<aws.scheduler.execution-id>","try":<aws.scheduler.attempt-number>}"#),
                1,
                r#"{"id":"EXEC","try":1}"#,
            ),
            (
                "a retried invocation's attempt",
                Some(r#"{"try":<aws.scheduler.attempt-number>}"#),
                2,
                r#"{"try":2}"#,
            ),
            (
                "the last attempt a default policy admits",
                Some(r#"{"try":<aws.scheduler.attempt-number>}"#),
                186,
                r#"{"try":186}"#,
            ),
            (
                "the schedule's own ARN",
                Some("<aws.scheduler.schedule-arn>"),
                1,
                arn,
            ),
        ];
        for (case, input, attempt, expected) in rows {
            let rendered =
                rendered_input(&input.map(ToString::to_string), arn, at, "EXEC", attempt);
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected:?}, got {rendered:?}"
            );
        }
    }

    /// A queue url is built only from fields that really name a queue of an
    /// account: the rows differ along the resource field and the account field.
    #[test]
    fn a_queue_url_is_built_only_from_fields_that_name_a_queue() {
        // (case, account, resource, the url the pair addresses)
        let rows: [(&str, &str, &str, Option<&str>); 6] = [
            (
                "a queue name",
                "000000000000",
                "orders",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders"),
            ),
            (
                "a FIFO queue name",
                "000000000000",
                "orders.fifo",
                Some("https://sqs.eu-west-2.amazonaws.com/000000000000/orders.fifo"),
            ),
            ("a resource carrying a path", "000000000000", "../other", None),
            ("a resource carrying a type", "000000000000", "queue:orders", None),
            ("an account that is no account id", "my-account", "orders", None),
            ("an account of the wrong length", "0000", "orders", None),
        ];
        for (case, account, resource, expected) in rows {
            let built = queue_url("eu-west-2", account, resource);
            assert_eq!(
                built.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {built:?}"
            );
        }
    }

    /// Every execution id a store hands out is its own.
    #[test]
    fn each_invocation_is_named_once() {
        let mut invocations = Invocations::default();
        let mut seen = ::std::collections::BTreeSet::new();
        for index in 0..64 {
            let id = invocations.next_execution_id();
            assert!(
                seen.insert(id.clone()),
                "case invocation {index}: the id {id:?} was handed out twice"
            );
        }
    }

    /// An unfinished invocation waits on the clock until the attempt it is due falls
    /// due, is then handed out ONCE, and is not held at all past the ceiling on how
    /// many of them this store keeps: the rows differ along how far ahead the next
    /// attempt is.
    #[test]
    fn a_further_attempt_waits_on_the_clock_until_it_is_due() {
        let now = epoch(1_893_456_030);
        let waiting = |due: SystemTime| Pending {
            schedule_arn: "arn:aws:scheduler:eu-west-2:000000000000:schedule/default/nightly"
                .to_string(),
            region: "eu-west-2".to_string(),
            target: Target::default(),
            nominal: now,
            execution_id: "EXEC".to_string(),
            began: now,
            attempts: 1,
            due,
        };
        // (case, when the next attempt is due, whether this tick performs it)
        let rows: [(&str, SystemTime, bool); 4] = [
            ("an attempt due in the past", epoch(1_893_456_029), true),
            ("an attempt due at this very tick", now, true),
            ("an attempt a second ahead", epoch(1_893_456_031), false),
            ("an attempt an hour ahead", epoch(1_893_459_630), false),
        ];
        for (case, due, performed) in rows {
            let mut invocations = Invocations::default();
            assert!(
                invocations.hold(waiting(due)),
                "case {case:?}: the clock did not hold an unfinished invocation"
            );
            let taken = invocations.take_due_attempts(now);
            assert_eq!(
                taken.len(),
                usize::from(performed),
                "case {case:?}: expected performed={performed}, got {} attempt(s)",
                taken.len()
            );
            // Whatever this tick took is off the clock, so the next tick does not
            // perform the same attempt again.
            let again = invocations.take_due_attempts(now);
            assert!(
                again.is_empty(),
                "case {case:?}: the same attempt was handed out twice: {again:?}"
            );
        }

        let mut invocations = Invocations::default();
        for index in 0..MAX_PENDING_INVOCATIONS {
            assert!(
                invocations.hold(waiting(epoch(1_893_459_630))),
                "case the ceiling: invocation {index} was not held"
            );
        }
        assert!(
            !invocations.hold(waiting(epoch(1_893_459_630))),
            "case the ceiling: an invocation past {MAX_PENDING_INVOCATIONS} was held"
        );
    }
}
