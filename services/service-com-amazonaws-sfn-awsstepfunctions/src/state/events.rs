// SPDX-License-Identifier: CC0-1.0

//! The EventBridge events this service publishes as an execution's status moves: one
//! event per status an execution enters.
//!
//! # The rule this module implements
//!
//! Step Functions does not only record an execution's progress for a reader to poll:
//! every status an execution MOVES INTO is published onto the account's default
//! EventBridge bus, under the `source` this service owns ([`EVENT_SOURCE`]) and the
//! `detail-type` of an execution's status change ([`EXECUTION_STATUS_CHANGE`]), carrying
//! the execution's identity, its instants, its data and — when it failed or was stopped —
//! its `error` and `cause` in the event's `detail`. A rule on that bus whose
//! `EventPattern` matches then delivers it to that rule's targets, so a client that
//! reacts to a workflow through EventBridge observes the run without calling Step
//! Functions again.
//!
//! Two bounds belong to the event rather than to the execution:
//!
//! * an EXPRESS workflow publishes no execution status-change event at all, so only an
//!   execution of a STANDARD one is reported here;
//! * an event is bounded in SIZE ([`MAX_EVENT_BYTES`]), while an execution's `input` and
//!   `output` are each bounded at the modelled 256 KiB, so a datum larger than the room
//!   one event leaves for it ([`MAX_EVENT_DATA_BYTES`]) is OMITTED and its
//!   `inputDetails`/`outputDetails` reports `"included": false` — the member's own
//!   documentation says `included` "indicates whether input or output was included in the
//!   response", and is "always true for API calls", which this is not.
//!
//! # How a change becomes an event
//!
//! Nothing here decides WHEN anything moves: the walk and the clock do
//! ([`super::ComAmazonawsSfnAwsstepfunctionsState::settle`]), and a status they write is
//! the fact an event reports. So this module is a DIFF rather than a second state
//! machine: [`record_status_changes`] compares every stored execution against the status
//! last published for it and queues one change per execution whose status differs —
//! whichever call, read or clock tick materialised it, and without any transition site
//! having to remember to announce itself. A redrive therefore publishes of its own
//! accord: the re-run moves the execution back to `RUNNING` and then to its next terminal
//! status, which are two statuses that differ from the last published one.
//!
//! A queued change is published by [`publish`], which runs with no lock held: EventBridge
//! is reached the way a `Task` state's own integration call reaches another service
//! ([`super::integration`]), so this crate names no other service's crate. Publication is
//! best-effort — an emulator with no EventBridge mounted has nowhere to publish — so a
//! failure is logged and the operation's own result stands.
//!
//! An execution's terminal status falls due after the call that started it has answered,
//! and a client watching only its queue makes no second Step Functions call, so the
//! transitions nobody asks for are materialised by [`drive`]: one task per store that
//! waits for the next due instant, settles, and publishes what the settling produced.
//!
//! "What the settling produced" is EVERYTHING those transitions owe another service, not
//! the bus alone: the same settle also queues what the execution publishes about itself
//! ([`super::observability`]) and the trace a tracing state machine's run records at X-Ray
//! ([`super::xray_tracing`]), so one hand-over ([`hand_over`]) writes all of them. Nothing
//! an execution owes may therefore wait for a client to call this service again — which is
//! the only record there is of a workflow whose own reporting APIs do not serve it.
//!
//! Nor does it wait for the clock: a status the clock made due is written by whichever
//! call settles the store first, and that is as often a REPORT of the execution (a
//! describe, a list, a history read) as a call that moves it, so a report hands over what
//! its own settle produced too — it settles through [`publish`] before it reads, so the
//! facts it reports have already gone out. A client that polls until a describe tells it
//! the execution has ended is therefore told so by the same call that sent the event
//! saying it has.
//!
//! The queue is BOUNDED ([`MAX_PENDING_EVENTS`], oldest dropped first) because every
//! transition appends to it, and the record of what has been published is pruned to the
//! executions the store still holds, so neither grows with the number of calls served.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::serde_json::{Map, Value};

use super::ComAmazonawsSfnAwsstepfunctionsState;

/// The `source` of every event Step Functions publishes.
///
/// EventBridge names the publishing service in an event's `source` as `aws.<service>`,
/// and this service's namespace is the one its ARNs carry ([`super::ARN_SERVICE`]).
pub const EVENT_SOURCE: &str = "aws.states";

/// The `detail-type` of the event published when an EXECUTION changes status.
pub const EXECUTION_STATUS_CHANGE: &str = "Step Functions Execution Status Change";

/// The event bus this service publishes onto.
///
/// The events an AWS service generates go to the account's default event bus, which is
/// the bus a rule carrying no `EventBusName` is created on.
pub const DEFAULT_EVENT_BUS: &str = "default";

/// The ARN namespace of the service this publishes through, and the operation that
/// publishes ([`super::integration::own_call`]).
const EVENTS_NAMESPACE: &str = "events";
/// See [`EVENTS_NAMESPACE`].
const PUT_EVENTS: &str = "PutEvents";

/// How large ONE event may be, in bytes.
///
/// The published bound on an EventBridge event, which is what decides whether an
/// execution's `input` and `output` are carried: each is bounded by the modelled
/// `SensitiveData` at 256 KiB on its own, so a datum at that bound cannot also fit beside
/// the execution's identity, its instants and its failure in one event.
pub const MAX_EVENT_BYTES: usize = 256 * 1024;

/// How many bytes of ONE event the facts beside its data may take.
///
/// The detail carries the execution's identity and its failure beside the two data
/// members, and those are bounded by the MODEL rather than by this emulator: `cause` is
/// `SensitiveCause` (len 0..32768), `error` is `SensitiveError` (0..256), a Map Run's
/// `mapRunArn` is `LongArn` (1..2000), each other ARN is `Arn` (1..256) and `name` is
/// `Name` (1..80) — so this is the room one event keeps for everything that is not the
/// input or the output, the envelope EventBridge adds included.
pub const MAX_EVENT_METADATA_BYTES: usize = 36 * 1024;

/// How large one DATUM — the execution's `input`, or its `output` — may be and still be
/// carried by the event: the room the event leaves beside everything else it holds.
///
/// One event carries BOTH data, so this is the room they SHARE rather than a bound on
/// each: the `input` is charged first and the `output` against what is left
/// ([`status_change_detail`]). A datum there is no room for is omitted and reported as
/// not included rather than truncated into the event, because a truncated document is not
/// the execution's data.
pub const MAX_EVENT_DATA_BYTES: usize = MAX_EVENT_BYTES - MAX_EVENT_METADATA_BYTES;

/// How many changes the queue holds at once.
///
/// This module's OWN ceiling, not an AWS quota: the queue lives in memory and every
/// transition appends to it, so without a bound a client looping on starts could grow it
/// without end. The oldest change is dropped first, so the transitions a live execution
/// just made are the ones that survive. It is far above what a realistic burst produces
/// (an execution contributes one change per status it enters).
pub const MAX_PENDING_EVENTS: usize = 2_000;

/// How many entries one `PutEvents` request carries.
///
/// The bound the service this publishes to declares: its `PutEventsRequestEntryList` is
/// `@length(min: 1, max: 10)`, so a batch holding more changes than that is published in
/// several requests rather than one the service would refuse.
const MAX_PUT_EVENTS_ENTRIES: usize = 10;

/// How many changes ONE batch of a drain publishes.
///
/// The queue itself is bounded ([`MAX_PENDING_EVENTS`]), but one request dispatching all
/// of it would fan out to a rule's targets that many times: this caps one batch, and
/// [`publish`] repeats a batch until the queue is empty, so nothing waits for a later
/// call.
const MAX_PUBLISHED_PER_DRAIN: usize = 50;

/// The shortest a wake-up of the clock waits, so a transition already past its due
/// instant costs a yield rather than a CPU-bound spin.
const MIN_TICK: Duration = Duration::from_millis(25);

/// The longest a wake-up waits, so the clock re-reads the store — which a request may
/// have changed since — at least this often.
const MAX_TICK: Duration = Duration::from_secs(2);

/// How many times the wake-up floor doubles while nothing moves.
///
/// An execution can be waiting on something only a REQUEST can release (an activity
/// task's worker, a callback task's `SendTaskSuccess`). Nothing is due for it until that
/// request arrives — and that request publishes what it materialises itself — so a clock
/// that finds nothing to publish backs off to `MIN_TICK << MAX_IDLE_SHIFT` between
/// wake-ups instead of polling a parked store at full rate. The floor is reset by the
/// first wake-up that publishes again.
const MAX_IDLE_SHIFT: u32 = 5;

/// How many wake-ups that publish NOTHING one clock takes before it stands down.
///
/// A backstop, not a schedule: it bounds how long one request's clock keeps polling a
/// store whose executions are parked on something only another request can release. A
/// clock that is still publishing is not spent — every wake-up that publishes resets the
/// count — so an execution in flight is carried to its terminal status rather than
/// abandoned part way through. Whatever a spent clock has not reached is published by the
/// next clock (every call that starts or continues an execution starts one if none is
/// running) or by the next call itself.
const MAX_IDLE_WAKE_UPS: usize = 1_024;

/// One status change waiting to be published.
///
/// The `detail` is rendered when the change is RECORDED, so the event reports the
/// execution as it was at that instant rather than as a later drain finds it.
#[derive(Debug, Clone)]
pub struct StatusChange {
    /// The execution's ARN, which the event envelope carries in `resources`: the event
    /// names the resource it is about, which is what a rule filtering on `resources`
    /// reads.
    pub execution_arn: String,
    /// The Region the execution runs in, which is the Region whose event bus it is
    /// published onto — read off the execution's own ARN, so an execution reports in its
    /// own Region however the call that moved it was signed.
    pub region: String,
    /// WHEN the execution entered this status, in epoch milliseconds: the instant the
    /// record itself carries for the transition, not the instant a drain reached it.
    pub at: i64,
    /// The status the execution moved INTO, which is what this change records as
    /// published for it.
    pub status: String,
    /// The rendered `detail` of the event.
    pub detail: Value,
}

/// Queue one event for every stored execution whose status differs from the status last
/// published for it, and take that status to be the published one.
///
/// This is the whole of the "which transitions owe an event" rule: a status the walk or
/// the clock wrote is compared against the last one published, so every move — a start, a
/// stop, a terminal outcome coming due, a redrive and the re-run's own outcome — owes
/// exactly one event, and a status that has not changed owes none however many times this
/// runs.
///
/// Called by [`ComAmazonawsSfnAwsstepfunctionsState::settle`], which every operation
/// runs, so no transition site has to announce itself.
pub(super) fn record_status_changes(
    store: &mut ComAmazonawsSfnAwsstepfunctionsState,
    now: SystemTime,
) {
    // The whole store is READ first and written afterwards, because an event's detail
    // reports whether `RedriveExecution` would redrive the execution, which is the
    // store's own predicate over the execution and its state machine.
    let mut queued: Vec<(String, StatusChange)> = Vec::new();
    for (arn, execution) in &store.executions {
        // An EXPRESS workflow publishes no execution status-change event, so an
        // execution of one owes nothing however its status moves. The type is the state
        // machine's own, and an execution whose state machine the store no longer holds
        // has no type to read — and no execution record either, since removing a state
        // machine removes its executions.
        let express = store
            .state_machines
            .get(&execution.state_machine_arn)
            .is_none_or(|machine| machine.sm_type == super::STATE_MACHINE_TYPE_EXPRESS);
        if express {
            continue;
        }
        if store
            .published_execution_statuses
            .get(arn)
            .is_some_and(|last| last == &execution.status)
        {
            continue;
        }
        // The instant of the transition this event reports: the stop that ended the
        // execution, the redrive that re-ran it, or the start that began it.
        let at = match execution.is_terminal() {
            true => execution.stop_date.unwrap_or(execution.start_date),
            false => execution.redrive_date.unwrap_or(execution.start_date),
        };
        queued.push((
            arn.clone(),
            StatusChange {
                execution_arn: execution.arn.clone(),
                region: super::arn_region(&execution.arn),
                at: super::epoch_millis(at),
                status: execution.status.clone(),
                detail: status_change_detail(
                    execution,
                    store.not_redrivable_reason(execution, now),
                ),
            },
        ));
    }
    for (arn, change) in queued {
        store
            .published_execution_statuses
            .insert(arn, change.status.clone());
        enqueue(&mut store.pending_execution_events, change);
    }
    // What was published for an execution the store no longer holds can never differ
    // again.
    let executions = &store.executions;
    store
        .published_execution_statuses
        .retain(|arn, _| executions.contains_key(arn));
}

/// Append `change` to the queue, dropping the oldest once the queue is full.
fn enqueue(pending: &mut VecDeque<StatusChange>, change: StatusChange) {
    while pending.len() >= MAX_PENDING_EVENTS {
        pending.pop_front();
        ::tracing::warn!(
            "the queue of execution status changes waiting to be published to EventBridge \
             is full at {MAX_PENDING_EVENTS}; the oldest change was dropped"
        );
    }
    pending.push_back(change);
}

/// The `detail` of an execution status change: the execution's identity, the status it
/// moved into, its instants, its data, its failure and its redrive accounting — the same
/// facts `DescribeExecution` reports, read off the one record.
///
/// `not_redrivable` is why `RedriveExecution` would refuse this execution, as the store's
/// own predicate answers it, so the `redriveStatus` the event carries cannot disagree
/// with the one a describe of the same execution reports.
///
/// The instants are whole epoch milliseconds, as an event's own numbers are, and a fact
/// the execution does not carry is reported as null rather than left out, so a rule's
/// target reads one shape of detail whatever the status is. The two SensitiveData members
/// are the exception: a datum the event has no room for is omitted altogether and its
/// details member says it was not included.
fn status_change_detail(
    execution: &super::Execution,
    not_redrivable: Option<&'static str>,
) -> Value {
    let mut detail = Map::new();
    detail.insert(
        "executionArn".to_string(),
        Value::String(execution.arn.clone()),
    );
    detail.insert(
        "stateMachineArn".to_string(),
        Value::String(execution.state_machine_arn.clone()),
    );
    detail.insert("name".to_string(), Value::String(execution.name.clone()));
    detail.insert("status".to_string(), Value::String(execution.status.clone()));
    detail.insert(
        "startDate".to_string(),
        Value::from(super::epoch_millis(execution.start_date)),
    );
    detail.insert("stopDate".to_string(), instant(execution.stop_date));
    detail.insert("error".to_string(), text(execution.error.as_deref()));
    detail.insert("cause".to_string(), text(execution.cause.as_deref()));
    // The version and the alias the start was made through, which the record carries
    // ("Step Functions associates executions that you start with an alias ARN with that
    // alias and the state machine version used for that execution"), and the Map Run that
    // dispatched a child workflow execution.
    detail.insert(
        "stateMachineVersionArn".to_string(),
        text(execution.state_machine_version_arn.as_deref()),
    );
    detail.insert(
        "stateMachineAliasArn".to_string(),
        text(execution.state_machine_alias_arn.as_deref()),
    );
    detail.insert(
        "mapRunArn".to_string(),
        text(execution.map_run_arn.as_deref()),
    );
    // "The number of times you've redriven an execution", and the instant the last
    // redrive happened — null until one has, exactly as `DescribeExecution` reports it.
    detail.insert(
        "redriveCount".to_string(),
        Value::from(execution.redrive_count),
    );
    detail.insert("redriveDate".to_string(), instant(execution.redrive_date));
    // Whether `RedriveExecution` would redrive this execution as it stands, and — when
    // it would not — the reason why: the SAME projection `DescribeExecution` reports,
    // from the same predicate, so a client reading the event decides what a describe
    // would have told it. A CHILD workflow execution of a Distributed Map is redriven by
    // redriving its Map Run rather than by its own ARN.
    let (redrive_status, redrive_status_reason) = match not_redrivable {
        Some(reason) => (
            super::REDRIVE_STATUS_NOT_REDRIVABLE,
            Value::String(reason.to_string()),
        ),
        None if execution.is_map_run_child() => {
            (super::REDRIVE_STATUS_REDRIVABLE_BY_MAP_RUN, Value::Null)
        }
        None => (super::REDRIVE_STATUS_REDRIVABLE, Value::Null),
    };
    detail.insert(
        "redriveStatus".to_string(),
        Value::String(redrive_status.to_string()),
    );
    detail.insert("redriveStatusReason".to_string(), redrive_status_reason);

    // The data, each carried only while the event has room for it.
    // The data, each carried only while the event still has room for it: the room is
    // SHARED, because one event carries both, so the `input` is charged first — it is the
    // datum the execution has had since it started — and the `output` is charged against
    // what is left of it.
    let mut room = MAX_EVENT_DATA_BYTES;
    let input_carried = execution.input.len() <= room;
    if input_carried {
        room = room.saturating_sub(execution.input.len());
        detail.insert("input".to_string(), Value::String(execution.input.clone()));
    }
    detail.insert("inputDetails".to_string(), included(input_carried));
    match execution.output.as_deref() {
        // An execution that has produced no output reports none, and no details for
        // one: there is nothing whose inclusion could be reported.
        None => {
            detail.insert("output".to_string(), Value::Null);
            detail.insert("outputDetails".to_string(), Value::Null);
        }
        Some(output) => {
            let carried = output.len() <= room;
            match carried {
                true => {
                    detail.insert("output".to_string(), Value::String(output.to_string()));
                }
                false => {
                    detail.insert("output".to_string(), Value::Null);
                }
            }
            detail.insert("outputDetails".to_string(), included(carried));
        }
    }
    Value::Object(detail)
}

/// `{"included": <included>}`, the `CloudWatchEventsExecutionDataDetails` an event
/// carries for one datum.
fn included(included: bool) -> Value {
    let mut details = Map::new();
    details.insert("included".to_string(), Value::Bool(included));
    Value::Object(details)
}

/// `time` as the whole epoch milliseconds an event's detail reports an instant in, or
/// null for an instant the execution does not carry.
fn instant(time: Option<SystemTime>) -> Value {
    time.map_or(Value::Null, |time| Value::from(super::epoch_millis(time)))
}

/// `value` as the detail reports it, or null for a member the execution does not carry.
fn text(value: Option<&str>) -> Value {
    value.map_or(Value::Null, |value| Value::String(value.to_string()))
}

/// Publish everything the transitions this store has materialised owe: every status
/// change queued onto the account's default EventBridge bus, every observation queued
/// for CloudWatch and CloudWatch Logs, and the clock that keeps doing both for the
/// transitions nobody will ask for.
///
/// Called with NO lock held, by every operation whose own settle can materialise an
/// execution's transition — a call that MOVES a status, and a REPORT that finds one the
/// clock had already made due, alike: each batch of changes is taken out in one critical
/// section, so nothing is published
/// twice, and the in-process `PutEvents` runs outside it. The queue is drained until it
/// is EMPTY — a batch is capped ([`MAX_PUBLISHED_PER_DRAIN`]), and the loop is what
/// guarantees the tail of a burst is published by the call that produced it rather than
/// waiting for the next one. Nothing about the answer changes what the caller of the
/// operation is told — an emulator with no EventBridge mounted has nowhere to publish —
/// so a failure is logged and the operation's own result stands.
///
/// It SETTLES as it drains, so a report calls it BEFORE it reads: what the report then
/// tells its caller has already been handed over, rather than the caller being told an
/// execution has ended by a call that left the event saying so for the clock.
pub async fn publish<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    hand_over(engine, &state, callback).await;
    drive(engine, &state, callback);
}

/// Hand over everything the transitions this store has materialised owe another
/// service, and answer how much went out — which is what tells the clock whether this
/// store is still moving.
///
/// The two hand-overs belong together because ONE settle produces both: the status
/// changes queued for the account's event bus, and the observations queued for the
/// stores CloudWatch and CloudWatch Logs answer from
/// ([`super::observability::publish`]). Called with no lock held, by a request and by
/// the clock alike, so a transition owes the same writes whoever materialised it.
async fn hand_over<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    state: &::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> usize {
    let changes = drain(engine, state, callback).await;
    // The trace of a run that has ended is owed by the same settle, and for the same
    // reason: an execution of a state machine whose tracing is enabled is recorded at
    // X-Ray whether or not anybody calls this service again
    // ([`super::xray_tracing`]).
    let segments = super::xray_tracing::publish(engine, callback).await;
    changes + segments + super::observability::publish(&**engine)
}

/// Publish what is queued onto the account's event bus, and answer how many changes went
/// out.
async fn drain<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    state: &::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> usize {
    let mut published = 0;
    loop {
        // What an execution's own KEY owes the outcome the clock has made due, decided
        // BEFORE that outcome is materialised and with no store of this service locked
        // ([`super::encryption::fail_due_runs_the_key_refuses`]). The decision belongs to
        // the instant the outcome was owed and not to whoever asked first, so it is taken
        // wherever a due outcome is settled — here as much as in front of a request —
        // rather than only where a caller happens to reach it.
        super::encryption::fail_due_runs_the_key_refuses(&**engine, super::now());
        // The guard is taken and dropped inside this statement: settling is what
        // records the transitions the clock has made due, and the batch is taken in the
        // same critical section so no change is published twice.
        let changes = {
            let mut store = state.lock();
            store.settle(super::now());
            take_pending(&mut store)
        };
        if changes.is_empty() {
            return published;
        }
        published += changes.len();
        publish_batch(callback, &changes).await;
    }
}

/// Take at most [`MAX_PUBLISHED_PER_DRAIN`] queued changes out of the store, oldest
/// first — one batch of the drain [`drain`] repeats until the queue is empty.
fn take_pending(store: &mut ComAmazonawsSfnAwsstepfunctionsState) -> Vec<StatusChange> {
    let taking = MAX_PUBLISHED_PER_DRAIN.min(store.pending_execution_events.len());
    store.pending_execution_events.drain(..taking).collect()
}

/// Publish one batch of changes, in `PutEvents` requests of at most
/// [`MAX_PUT_EVENTS_ENTRIES`] entries, each made in the Region the executions it reports
/// run in.
async fn publish_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    changes: &[StatusChange],
) {
    let Some(call) = super::integration::own_call(EVENTS_NAMESPACE, PUT_EVENTS) else {
        ::tracing::warn!(
            "no wire facts are stated for the '{EVENTS_NAMESPACE}' service, so an execution \
             status change cannot be published"
        );
        return;
    };
    // One request per Region: a bus belongs to a Region, and an execution is published
    // onto the bus of the Region it runs in.
    let mut by_region: BTreeMap<&str, Vec<Value>> = BTreeMap::new();
    for change in changes {
        by_region
            .entry(change.region.as_str())
            .or_default()
            .push(entry(change));
    }
    for (region, entries) in by_region {
        for window in entries.chunks(MAX_PUT_EVENTS_ENTRIES) {
            let mut request = Map::new();
            request.insert("Entries".to_string(), Value::Array(window.to_vec()));
            let answered = super::integration::call_api(
                callback,
                region,
                &call,
                &Value::Object(request),
            )
            .await;
            match answered {
                // The publication answered: a `PutEvents` reports an entry it did NOT
                // publish on that entry's own row rather than by refusing the request,
                // so the rows are read and an entry the service rejected is logged.
                Ok(reported) => {
                    for refused in reported
                        .get("Entries")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|row| row.get("ErrorCode").and_then(Value::as_str))
                    {
                        ::tracing::warn!(
                            error_code = %refused,
                            "EventBridge did not publish a Step Functions execution status \
                             change"
                        );
                    }
                }
                Err(failed) => {
                    ::tracing::warn!(
                        error = %failed.error,
                        cause = %failed.cause,
                        "a Step Functions execution status change could not be published to \
                         EventBridge"
                    );
                }
            }
        }
    }
}

/// One `PutEventsRequestEntry`, as that operation's own model spells its members.
fn entry(change: &StatusChange) -> Value {
    let mut entry = Map::new();
    entry.insert(
        "EventBusName".to_string(),
        Value::String(DEFAULT_EVENT_BUS.to_string()),
    );
    entry.insert("Source".to_string(), Value::String(EVENT_SOURCE.to_string()));
    entry.insert(
        "DetailType".to_string(),
        Value::String(EXECUTION_STATUS_CHANGE.to_string()),
    );
    entry.insert(
        "Resources".to_string(),
        Value::Array(vec![Value::String(change.execution_arn.clone())]),
    );
    // The event's `Time` is WHEN the execution moved, not when the queue was drained:
    // `PutEventsRequestEntry$Time` is "the time stamp of the event, per RFC3339. If no
    // time stamp is provided, the time stamp of the PutEvents call is used." The wire
    // form of a timestamp in this service's protocol is epoch SECONDS, so the stored
    // milliseconds go out as a fraction of a second rather than as a whole number of
    // them.
    #[allow(clippy::cast_precision_loss)]
    let seconds = change.at as f64 / 1_000.0;
    entry.insert("Time".to_string(), Value::from(seconds));
    entry.insert(
        "Detail".to_string(),
        Value::String(change.detail.to_string()),
    );
    Value::Object(entry)
}

/// Start the one in-process task that waits for this store's next due execution
/// transition, materialises what is then due and publishes it, and stands down once no
/// execution is still in flight.
///
/// Called by [`publish`], so every operation that can move an execution keeps the clock
/// running; a second call while one runs starts nothing. With no async runtime to spawn
/// onto there is no clock: the transitions are then materialised and published by the
/// next operation instead, as they always were.
fn drive<Eng: Send + Sync + 'static>(
    engine: &Arc<Eng>,
    state: &::engine::ServiceState<ComAmazonawsSfnAwsstepfunctionsState>,
    callback: &smithy_server_runtime::EmbeddedEmulator,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim(&mut state.lock()) else {
        return;
    };
    let driven = ::engine::ServiceState::clone(state);
    // The same engine the request itself would have published through, so the clock
    // reaches the stores the owning services' own readers answer from.
    let engine = Arc::clone(engine);
    // The same handle into this same emulator the request itself would have published
    // through, owned by the task rather than borrowed from the request that started it.
    let callback = callback.clone();
    let handle = runtime.spawn(async move {
        // `true` only if the idle cap is what ended the loop; a clock that ran out of
        // work has already released its claim inside the one critical section that
        // observed it.
        let mut capped = true;
        let mut idle_shift = 0;
        let mut idle_wake_ups = 0;
        while idle_wake_ups < MAX_IDLE_WAKE_UPS {
            let Some(next_due) = in_flight_or_release(&mut driven.lock(), generation) else {
                capped = false;
                break;
            };
            // Every wake-up waits, even for an instant already past: that is what keeps
            // a due transition from costing a CPU-bound spin, and it hands the runtime
            // back between wake-ups.
            ::tokio::time::sleep(wait(next_due, idle_shift)).await;
            let published = hand_over(&engine, &driven, &callback).await;
            idle_shift = match published {
                0 => (idle_shift + 1).min(MAX_IDLE_SHIFT),
                _ => 0,
            };
            // A wake-up that published leaves the clock as fresh as a new one, so only
            // the ones that found nothing count against the cap.
            idle_wake_ups = match published {
                0 => idle_wake_ups + 1,
                _ => 0,
            };
        }
        if capped {
            ::tracing::warn!(
                "the execution event clock stood down after {MAX_IDLE_WAKE_UPS} wake-ups \
                 that published nothing while an execution was still in flight; the next \
                 call to this service publishes what it owes"
            );
            // Only THIS clock's claim, so a clock started since is left holding its own.
            release(&mut driven.lock(), generation);
        }
    });
    set_clock(&mut state.lock(), generation, handle);
}

/// How long one wake-up waits before it settles and publishes: until the next due
/// instant, never less than the idle floor and never more than [`MAX_TICK`].
fn wait(next_due: Option<SystemTime>, idle_shift: u32) -> Duration {
    let floor = (MIN_TICK * (1 << idle_shift)).min(MAX_TICK);
    next_due
        .and_then(|due| due.duration_since(SystemTime::now()).ok())
        .unwrap_or_default()
        .clamp(floor, MAX_TICK)
}

/// The next instant anything is due at — `None` for a store with work in flight but no
/// stored due instant — or `None` from the whole call, having RELEASED the claim, when
/// nothing is in flight at all.
///
/// Observing "nothing is in flight" and standing down happen in ONE critical section,
/// which is what makes the hand-over race-free: a call that starts an execution holds
/// this same lock to store it BEFORE it tries to claim the clock, so either this call
/// sees that execution and the running clock keeps it, or the starter's own claim finds
/// the flag cleared and starts a clock of its own.
fn in_flight_or_release(
    store: &mut ComAmazonawsSfnAwsstepfunctionsState,
    generation: u64,
) -> Option<Option<SystemTime>> {
    // A queued change is work in flight of its own: it is published before this clock
    // may stand down.
    let mut in_flight = !store.pending_execution_events.is_empty();
    let mut next_due: Option<SystemTime> = None;
    for execution in store.executions.values() {
        if execution.is_terminal() {
            continue;
        }
        in_flight = true;
        let due = execution.pending.as_ref().map(|pending| pending.due_at);
        next_due = next_due.into_iter().chain(due).min();
    }
    if !in_flight {
        release(store, generation);
        return None;
    }
    Some(next_due)
}

/// Claim the single clock for this store: the claim's GENERATION when the caller may
/// start one, `None` when one is really running.
///
/// The claim is taken over from a clock whose TASK HAS ALREADY ENDED while still holding
/// it. A clock releases its claim itself on the two ends it can see coming — nothing left
/// in flight, and the idle cap — and [`release`] drops the handle with the claim, so a
/// claim still held while the task is over belongs to a clock that ended on neither: one
/// wake-up's work panicked, or the task was cancelled. Without the take-over that claim
/// is held for as long as this store lives, and because the claim is what stops a second
/// clock from starting, no later call could ever start one again.
///
/// A granted claim takes the OLD handle out with it, so the window between this call and
/// the new handle being installed ([`set_clock`]) holds no handle at all: a call that
/// reaches this one inside that window sees a held claim and no ended task, and starts
/// nothing. One clock runs at a time.
fn claim(store: &mut ComAmazonawsSfnAwsstepfunctionsState) -> Option<u64> {
    let ended = store
        .event_clock
        .as_ref()
        .is_some_and(::tokio::task::JoinHandle::is_finished);
    if store.event_clock_running && !ended {
        return None;
    }
    if ended && store.event_clock_running {
        ::tracing::warn!(
            "the execution event clock this store holds the claim for has ended without \
             releasing it; the claim is taken over so the transitions it owes are still \
             published"
        );
    }
    // The task this handle names has ended, or has released its claim and is ending, so
    // nothing is lost by dropping it; the abort is what makes that true in the second
    // case as well.
    if let Some(previous) = store.event_clock.take() {
        previous.abort();
    }
    store.event_clock_running = true;
    store.event_clock_generation = store.event_clock_generation.wrapping_add(1);
    Some(store.event_clock_generation)
}

/// Release the claim `generation` took, and the handle with it — and ONLY that one, so a
/// clock that ended on the idle cap cannot clear a claim a clock started since is
/// holding.
fn release(store: &mut ComAmazonawsSfnAwsstepfunctionsState, generation: u64) {
    if store.event_clock_generation == generation {
        store.event_clock_running = false;
        store.event_clock = None;
    }
}

/// Keep the running clock's task handle, aborting the one it replaces — and only while
/// `generation` is still the CURRENT claim, so a starter whose own task has already stood
/// down installs nothing but its own abort.
///
/// An abort leaves the store exactly as it was: the clock only settles and publishes, and
/// both happen in one critical section each, so a clock that is cut off mid-flight has
/// either published a batch or not taken it out of the queue at all.
fn set_clock(
    store: &mut ComAmazonawsSfnAwsstepfunctionsState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    // A claim that is no longer the current one, or no longer held at all (the task has
    // already run out of work and released it), installs nothing: the handle and the
    // claim are written together, so the store never holds one without the other.
    if store.event_clock_generation != generation || !store.event_clock_running {
        handle.abort();
        return;
    }
    if let Some(previous) = store.event_clock.replace(handle) {
        previous.abort();
    }
}
