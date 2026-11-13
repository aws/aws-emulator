// SPDX-License-Identifier: CC0-1.0

//! Lambda's ASYNCHRONOUS INVOCATION queue, and what an entry on it DOES.
//!
//! An asynchronous invocation's accepting call promises the QUEUEING and nothing about the
//! run: "A success response (202 Accepted) indicates that the request is queued for
//! invocation" (`InvokeAsyncResponse`), and `Invoke`'s `Event` type is "Invoke the function
//! asynchronously […] The API response only includes a status code". So the call records one
//! [`AsyncInvocationRecord`] against the version it resolved and answers the accepted status.
//!
//! What follows the accepting call is the DELIVERY: Lambda hands the queued event to the
//! function, which runs it, so whatever the handler itself performs becomes observable a
//! short while after the call that queued it rather than never. That is [`deliver`]: the
//! event's delivery attempt comes due [`ASYNC_DELIVERY_DELAY`] after it was accepted, and the
//! attempt runs the function's own deployment package through the in-process engine every
//! synchronous invocation runs it through ([`super::execution::run_invocation`]) — the same
//! handler, the same package, the same bounds. An attempt whose handler ERRORED is followed
//! by another, as many as the event has ([`super::async_delivery`]), because "if you invoke
//! a function asynchronously and it returns an error, Lambda executes the function up to two
//! more times"; so is one that found no capacity to run the function at all, because "if your
//! function is throttled, Lambda returns the event to the queue and attempts to run the
//! function again" — how busy this host is decides WHEN the function runs the event, never
//! whether it ever does. Nothing about the run reaches the accepting call's response, because
//! the response was given before the run happened; the entry keeps what the last attempt came
//! to ([`AsyncInvocationOutcome`]).
//!
//! Nothing sleeps inside a request: the attempt is driven on a task of its own, exactly as
//! the account's other in-process pumps are, and with no async runtime to spawn onto there
//! is no attempt — the entry then settles on the clock, carrying
//! [`ASYNC_NOT_DELIVERED_REASON`], because nothing delivered it. An event an attempt IS
//! pending for ([`AsyncInvocationRecord::attempt_pending`]) is left to that attempt however
//! long the host takes to get to it, so what the entry comes to is the attempt's and never a
//! race between the attempt and the clock.

use super::*;

/// What has become of one queued asynchronous invocation.
///
/// An entry starts [`Self::Queued`]. Once a delivery attempt claims it, it is
/// [`Self::Delivering`] for as long as that attempt lasts, and then [`Self::Delivered`]
/// carrying what the run came to, or [`Self::Failed`] carrying the reason the function never
/// ran at all. Nothing here reports a result, a log tail or a duration for a function that
/// did not execute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AsyncInvocationOutcome {
    /// Accepted and waiting for its delivery attempt, which is due at
    /// [`AsyncInvocationRecord::attempt_due`].
    Queued,
    /// A delivery attempt has claimed the event and the function is being invoked with it.
    Delivering,
    /// The event was delivered to the function and the handler RAN.
    Delivered {
        /// The `FunctionError` the run reported — "Unhandled" for an exception the handler
        /// raised — and `None` for a handler that returned normally. An asynchronous
        /// invocation's error is not reported to the caller, which was answered before the
        /// run, so this is where the run's own outcome is kept.
        function_error: Option<String>,
    },
    /// The delivery attempt was made and the function never ran; the string is why.
    Failed(String),
}

/// One asynchronous invocation this emulator accepted for a function.
///
/// Held per function on [`FunctionRecord::async_invocations`], because Lambda queues an
/// asynchronous event against the function version it was addressed to.
#[derive(Debug, Clone)]
pub struct AsyncInvocationRecord {
    /// The version label the event was addressed to (`$LATEST` or a published version).
    pub version: String,
    /// This invocation's own request id, which is `context.aws_request_id` of the run the
    /// event is delivered to and the key a delivery attempt finds its entry by.
    pub request_id: String,
    /// The event payload, exactly as the caller supplied it.
    pub payload: Vec<u8>,
    /// ISO-8601 instant the event was accepted for invocation.
    pub received: String,
    /// When the first delivery attempt becomes due; see [`ASYNC_DELIVERY_DELAY`].
    pub attempt_due: SystemTime,
    /// How many delivery attempts have been MADE for this event — every attempt, including
    /// one that found no capacity to run the function; what
    /// [`super::async_delivery::MAX_ASYNC_DELIVERY_ATTEMPTS`] bounds is the attempts that RAN
    /// the handler ([`super::async_delivery::next_attempt_in`]).
    pub attempts: u32,
    /// Whether a delivery attempt is PENDING for this event: set by [`deliver`] before the
    /// call that accepted the event is answered, and the reason the clock leaves the entry
    /// alone ([`FunctionRecord::materialize`]) — an event an attempt is coming for is one
    /// that IS being delivered, whether or not the host has got to the attempt yet, so
    /// [`ASYNC_NOT_DELIVERED_REASON`] is owed only where no attempt was ever made.
    pub attempt_pending: bool,
    /// What has become of the event.
    pub outcome: AsyncInvocationOutcome,
}

/// How long after an asynchronous invocation is accepted its first delivery attempt
/// becomes due.
///
/// An EMULATOR-CHOSEN span, like [`ACTIVATION_DELAY`]: Lambda publishes no latency for
/// the asynchronous event queue. It is deliberately non-zero, so a caller that looks
/// straight after the accepting call still sees the event [`AsyncInvocationOutcome::Queued`].
pub const ASYNC_DELIVERY_DELAY: Duration = Duration::from_millis(250);

/// Why a queued asynchronous invocation never ran: no delivery attempt was ever made for
/// it, because this emulator was handed no async runtime to make one on.
///
/// Recorded on the entry when its attempt instant comes due with nothing having claimed it;
/// an entry a delivery attempt did claim records what that attempt came to instead.
pub const ASYNC_NOT_DELIVERED_REASON: &str =
    "the event was accepted and queued, but this emulator had no asynchronous runtime to \
     deliver it to the function on, so it was never executed";

/// Why an accepted asynchronous invocation never ran when Lambda's recursive-loop
/// guardrail stopped it: the function's recursive loop detection configuration is
/// [`recursion::RECURSIVE_LOOP_TERMINATE`] and the chain this invocation belongs to had
/// already come back round into the function, so Lambda "stops your function being
/// invoked" instead of delivering the event. Recorded on the entry at the moment the
/// invocation is stopped, because there is no delivery attempt still to come.
pub const ASYNC_RECURSION_STOPPED_REASON: &str =
    "the invocation was stopped by Lambda's recursive loop detection: the request chain it \
     belongs to had already invoked this function, and the function's recursive loop \
     detection configuration is Terminate, so the event was never delivered";

/// The most queued asynchronous invocations this emulator keeps for one function.
///
/// An EMULATOR-CHOSEN ceiling for the same reason as [`MAX_FUNCTIONS`]: the queue is in
/// memory and every accepted event adds its payload to it, so past this point the
/// oldest entry is dropped rather than the store growing without bound. Dropping the
/// oldest does not change what any call answers — the accepting call reports only that
/// the event was queued, and no operation reads the queue back out.
pub const MAX_QUEUED_ASYNC_INVOCATIONS: usize = 100;

/// How many delivery attempts this emulator has in flight at once.
///
/// An EMULATOR-CHOSEN ceiling: every accepted event is delivered on a task of its own, and a
/// caller can accept events as fast as it can call, so this is what keeps the number of
/// tasks — and the payloads they hold — bounded by the emulator rather than by the request
/// rate. It is above [`MAX_QUEUED_ASYNC_INVOCATIONS`], so a function's whole queue can be in
/// flight, and well above the handlers that can RUN at once
/// ([`super::execution::MAX_CONCURRENT_RUNS`]), so a burst waits on the run memory rather than
/// being dropped here.
pub const MAX_IN_FLIGHT_DELIVERIES: usize = 256;

/// How many delivery attempts are in flight right now; see [`MAX_IN_FLIGHT_DELIVERIES`].
static IN_FLIGHT_DELIVERIES: ::std::sync::atomic::AtomicUsize =
    ::std::sync::atomic::AtomicUsize::new(0);

/// One of the [`MAX_IN_FLIGHT_DELIVERIES`] delivery slots, held for exactly as long as the
/// task holding it runs and released however that task ends.
struct DeliveryPermit;

impl DeliveryPermit {
    /// Take a slot, or `None` when every one of them is already taken.
    fn claim() -> Option<Self> {
        IN_FLIGHT_DELIVERIES
            .try_update(
                ::std::sync::atomic::Ordering::SeqCst,
                ::std::sync::atomic::Ordering::SeqCst,
                |in_flight| (in_flight < MAX_IN_FLIGHT_DELIVERIES).then_some(in_flight + 1),
            )
            .ok()
            .map(|_| Self)
    }
}

impl Drop for DeliveryPermit {
    fn drop(&mut self) {
        IN_FLIGHT_DELIVERIES.fetch_sub(1, ::std::sync::atomic::Ordering::SeqCst);
    }
}

/// Deliver one accepted asynchronous event to its function, on the runtime's own time.
///
/// The entry point the accepting operation calls with the store's guard released, once the
/// event it queued is held: the attempt is made on a task of its own, so a caller's request
/// is answered — "the API response only includes a status code" — without waiting for the
/// handler, and the entry `invocation.request_id` names keeps what the attempt came to.
///
/// What bounds the task: at most [`MAX_IN_FLIGHT_DELIVERIES`] of them run at once, each is
/// one wait for the event's delivery delay and then at most
/// [`super::async_delivery::MAX_ASYNC_DELIVERY_ATTEMPTS`] runs of the handler through
/// [`super::execution::run_invocation`] — which holds a run slot of its own, waits at most
/// [`super::execution::PERMIT_WAIT`] for one of them and is cut off by the function's `Timeout`
/// inside [`super::execution::HOST_WAIT_CEILING`] — with only an errored handler taking
/// another ([`super::async_delivery::next_attempt_in`]). An attempt that found no capacity to
/// run the function at all RAN nothing, so it is made again instead, for as long as the event
/// is retained ([`super::async_delivery::next_throttled_attempt_in`]) and never past it.
///
/// With no async runtime to spawn onto nothing is delivered, and nothing a caller was told
/// depends on it: the entry settles on the clock instead, carrying
/// [`ASYNC_NOT_DELIVERED_REASON`].
///
/// `durable_execution` names the durable execution this invocation IS, when the accepting
/// call started one: the run is that execution's, so what the run comes to is the status the
/// execution ends in ([`super::durable_run`]).
pub fn deliver(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    invocation: Invocation,
    durable_execution: Option<String>,
) {
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        ::tracing::warn!(
            function = %invocation.function_name,
            "no async runtime is available to deliver this function's asynchronous event on"
        );
        return;
    };
    let Some(permit) = DeliveryPermit::claim() else {
        ::tracing::warn!(
            function = %invocation.function_name,
            in_flight = MAX_IN_FLIGHT_DELIVERIES,
            "this emulator is already delivering as many asynchronous invocations as it \
             runs at once; this event is not delivered"
        );
        return;
    };
    // The attempt is this event's from here on: the entry is marked as one a delivery
    // attempt is PENDING for before the accepting call is answered, so the clock leaves it
    // to that attempt rather than settling it as an event nothing ever delivered. Which of
    // the two reaches the entry first is the host's to decide; what the entry comes to is
    // not.
    mark_attempt_pending(state, &invocation.function_name, &invocation.request_id);
    let state = ::engine::ServiceState::clone(state);
    let function_name = invocation.function_name.clone();
    let request_id = invocation.request_id.clone();
    // When this event was accepted, which is what its age is measured from: the window a
    // throttled event is attempted again inside runs from the acceptance
    // ([`super::async_delivery::next_throttled_attempt_in`]).
    let accepted = ::tokio::time::Instant::now();
    runtime.spawn(async move {
        // Moved into the task, so the slot is held for exactly as long as the task runs and
        // is released however it ends.
        let _permit = permit;
        // The delay the accepting call recorded on the entry, waited for here rather than
        // inside the request that queued the event, so a caller that looks straight after
        // that call still sees the event queued. No store guard is held across it.
        ::tokio::time::sleep(ASYNC_DELIVERY_DELAY).await;
        // How many attempts RAN the handler, and how many found no capacity to run it at
        // all: the two are bounded by different published rules, so the attempts are counted
        // apart (`super::async_delivery`).
        let mut runs: u32 = 0;
        let mut throttles: u32 = 0;
        // WHAT ends the event, which is the condition the record a destination reads reports:
        // the attempts it was allowed, until the retention window below is what runs out
        // first (`super::async_completion_delivery::Ending`).
        let mut ending = super::async_completion_delivery::Ending::AttemptsSpent;
        // Each turn is ONE delivery attempt: the attempt is the entry's from here on, so the
        // clock settles it no further, and what follows an attempt is the retry policy's
        // (`super::async_delivery`) — an errored handler is invoked again, an attempt that
        // found no capacity is made again, and a handler that returned or a run nothing
        // could start are the end of the event.
        loop {
            let Some(claimed) = claim(&state, &function_name, &request_id) else {
                return;
            };
            let run = super::execution::run_invocation(invocation.clone()).await;
            // A THROTTLED attempt is not what became of the event: nothing about the event
            // stopped it, so "Lambda returns the event to the queue and attempts to run the
            // function again" for as long as the event is retained. The entry goes back to
            // being queued, with the instant the next attempt is due on it, rather than
            // recording a capacity this host happened not to have as the event's outcome.
            if matches!(&run, Err(super::execution::Unrunnable::Overloaded(_))) {
                throttles = throttles.saturating_add(1);
                let retention = event_retention_of(&state, &function_name, &request_id);
                if let Some(again) = super::async_delivery::next_throttled_attempt_in(
                    throttles,
                    accepted.elapsed(),
                    retention,
                ) {
                    ::tracing::warn!(
                        function = %function_name,
                        request_id = %request_id,
                        next_attempt_seconds = again.as_secs(),
                        "no capacity was free to run this function for an accepted \
                         asynchronous invocation, so the event goes back on the queue"
                    );
                    requeue(&state, &function_name, &request_id, again);
                    ::tokio::time::sleep(again).await;
                    continue;
                }
                // The window the event is retained for is spent, so this is what ENDED it:
                // its age rather than the attempts it was allowed, which is what the record
                // a destination reads reports
                // (`super::async_completion_delivery::Ending`).
                ending = super::async_completion_delivery::Ending::AgeExceeded;
            }
            if run.is_ok() {
                runs = runs.saturating_add(1);
                // The AGE of the event at the moment the function was invoked with it, which
                // is what `AsyncEventAge` measures: one observation per attempt that RAN the
                // handler, so a retried event meters the longer wait its retry really had. A
                // retry is also one more invocation of the function code, which the accepting
                // call could not have metered (`super::metrics`).
                if let Some(metrics) = invocation.metrics.as_ref() {
                    metrics.publish_async_event_age(accepted.elapsed());
                    if runs > 1 {
                        metrics.publish_retry_invocation();
                    }
                }
            }
            let (outcome, ran, function_error) = match &run {
                Ok(invoked) => (
                    AsyncInvocationOutcome::Delivered {
                        function_error: invoked.function_error.clone(),
                    },
                    true,
                    invoked.function_error.clone(),
                ),
                Err(why) => (
                    AsyncInvocationOutcome::Failed(unrunnable_reason(why, THE_EVENT)),
                    false,
                    None,
                ),
            };
            if let AsyncInvocationOutcome::Failed(ref why) = outcome {
                // A function an accepted event could not be run for is worth seeing: the
                // caller was answered before the attempt, so this is the only place it is
                // reported.
                ::tracing::warn!(
                    function = %function_name,
                    request_id = %request_id,
                    detail = %why,
                    "an accepted asynchronous invocation could not be delivered to its \
                     function"
                );
            }
            let Some(again) = super::async_delivery::next_attempt_in(
                ran,
                function_error.as_deref(),
                runs,
                claimed.allowed,
            ) else {
                settle(&state, &function_name, &request_id, outcome);
                // The event's deliveries to the FUNCTION are over, so what the configuration
                // of this qualifier and of this version asks for next is owed: the invocation
                // record to the destination, and the event itself to the dead-letter target
                // (`super::async_completion_delivery`). It happens on the LAST attempt only,
                // because an event the retry policy will deliver again has not completed.
                super::async_completion_delivery::deliver_completion(
                    &state,
                    &super::async_completion_delivery::Completion {
                        invocation: &invocation,
                        qualifier: &claimed.qualifier,
                        attempts: runs,
                        run: &run,
                        ending,
                    },
                )
                .await;
                // The run was the durable execution's, so the execution ends in the status
                // the invocation came to — which is what makes a caller polling it see a
                // terminal status rather than `RUNNING` for ever. It is settled on the LAST
                // attempt only: an attempt the retry policy will make again has not ended
                // the invocation.
                if let Some(name) = &durable_execution {
                    super::durable_run::settle(
                        &state,
                        &function_name,
                        name,
                        &super::durable_run::run_outcome(&run),
                    );
                }
                return;
            };
            // The event has another attempt, so the entry is left as the one it is being
            // delivered under rather than carrying an outcome a later attempt may replace.
            ::tokio::time::sleep(again).await;
        }
    });
}

/// What claiming one delivery attempt reports about the event it claimed.
struct Claimed {
    /// How many delivery attempts the event is allowed in all, as the qualifier's own
    /// asynchronous-invocation configuration says
    /// ([`super::async_delivery::attempts_allowed`]).
    allowed: u32,
    /// The version label the event was addressed to, which is the qualifier that
    /// configuration belongs to.
    qualifier: String,
}

/// Mark the entry `request_id` names on `function` as having a delivery attempt PENDING, so
/// the clock leaves the outcome to that attempt however long the host takes to make it.
///
/// Called by [`deliver`] with no store guard held, before the accepting call is answered: an
/// event an attempt is coming for is one that IS being delivered, and the entry is left
/// [`AsyncInvocationOutcome::Queued`] until the attempt claims it so a caller that looks
/// straight after the accepting call still sees the event queued.
///
/// Does nothing when the store holds no such entry, which is nothing to deliver to.
fn mark_attempt_pending(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    request_id: &str,
) {
    let mut store = state.lock();
    if let Some(entry) = entry_of(&mut store, function, request_id) {
        entry.attempt_pending = true;
    }
}

/// How long the event `request_id` names on `function` is RETAINED while this emulator
/// tries to get the function run for it.
///
/// The window is the `MaximumEventAgeInSeconds` the asynchronous-invocation configuration of
/// the QUALIFIER the event was addressed to states — that resource is "for asynchronous
/// invocation on a function, version, or alias", so a configuration put on an alias is the
/// alias's own and never the one the version that alias invokes holds — and the documented
/// default ([`super::async_delivery::DEFAULT_ASYNC_EVENT_RETENTION`]) for a qualifier nobody
/// has configured, which is also the window for an event the store no longer holds.
///
/// A READ of both: it decides how long a throttled event keeps its place in the queue and
/// changes nothing a later caller sees.
#[must_use]
pub fn event_retention_of(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    request_id: &str,
) -> Duration {
    let mut store = state.lock();
    let configured = entry_of(&mut store, function, request_id)
        .map(|entry| entry.version.clone())
        .and_then(|version| {
            store
                .functions
                .get(function)?
                .event_invoke_configs
                .get(&version)?
                .maximum_event_age_in_seconds
        });
    super::async_delivery::event_retention(configured)
}

/// Put the entry `request_id` names back on the QUEUE, with its next attempt due `next_in`
/// from now: what a throttled attempt leaves behind, because "Lambda returns the event to
/// the queue and attempts to run the function again".
///
/// The entry keeps its pending mark, so the instant the attempt falls due is the attempt's
/// to act on and not an outcome the clock writes.
fn requeue(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    request_id: &str,
    next_in: Duration,
) {
    let mut store = state.lock();
    if let Some(entry) = entry_of(&mut store, function, request_id) {
        entry.outcome = AsyncInvocationOutcome::Queued;
        entry.attempt_due = SystemTime::now() + next_in;
    }
}

/// Mark the entry `request_id` names on `function` as being delivered, counting the attempt
/// that is about to be made, so the clock does not settle an event an attempt is already
/// making — and answer what that leaves known about the event ([`Claimed`]).
///
/// The claim is unconditional: an entry the clock settled in the instant between its attempt
/// falling due and this attempt taking it is one that IS being delivered, so the attempt's
/// own outcome is the one that stands.
///
/// `None` when the store holds no such entry any more — the queue is bounded, so a newer
/// event may already have made room with it — in which case there is nothing to deliver to.
fn claim(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    request_id: &str,
) -> Option<Claimed> {
    let mut store = state.lock();
    let record = store.functions.get_mut(function)?;
    let entry = record
        .async_invocations
        .iter_mut()
        .find(|entry| entry.request_id == request_id)?;
    entry.outcome = AsyncInvocationOutcome::Delivering;
    entry.attempts = entry.attempts.saturating_add(1);
    let qualifier = entry.version.clone();
    // How many attempts the event is allowed is the ADDRESSED qualifier's own, read on each
    // attempt rather than fixed when the event was accepted: the resource is the
    // configuration, which a caller may replace while the event is still being delivered.
    let allowed = super::async_delivery::attempts_allowed(
        record
            .event_invoke_configs
            .get(&qualifier)
            .and_then(|config| config.maximum_retry_attempts),
    );
    Some(Claimed { allowed, qualifier })
}

/// Record what the delivery attempt for `request_id` came to on the entry that holds it, if
/// the store still holds it.
fn settle(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    function: &str,
    request_id: &str,
    outcome: AsyncInvocationOutcome,
) {
    let mut store = state.lock();
    if let Some(entry) = entry_of(&mut store, function, request_id) {
        entry.outcome = outcome;
    }
}

/// The queue entry `request_id` names on `function`, or `None` when the store holds neither.
fn entry_of<'store>(
    store: &'store mut ComAmazonawsLambdaAwsgirapiserviceState,
    function: &str,
    request_id: &str,
) -> Option<&'store mut AsyncInvocationRecord> {
    store
        .functions
        .get_mut(function)?
        .async_invocations
        .iter_mut()
        .find(|entry| entry.request_id == request_id)
}

/// Why the function the work `subject` names was handed to never ran, one reason per
/// condition a run can fail to start on.
///
/// A synchronous invocation answers each of these as one of `Invoke`'s own modelled errors;
/// an asynchronous one has no caller left to answer, so the reason is recorded on the entry
/// instead — and on the durable execution that invocation started, which reads it from here
/// ([`super::durable_run`]), as does the function URL plane for a request it could not run
/// (`crate::service::function_url`). `subject` is what the sentence calls that work: the
/// queued event here, the request there — so the ONE set of sentences serves every caller.
pub(crate) fn unrunnable_reason(why: &super::execution::Unrunnable, subject: &str) -> String {
    match why {
        super::execution::Unrunnable::NoEngine { runtime } => format!(
            "the function's runtime {runtime} is not one this emulator embeds an engine for, \
             so {subject} was never executed"
        ),
        super::execution::Unrunnable::NoHandler => format!(
            "the function's configuration names no Handler to run, so {subject} was never \
             executed"
        ),
        super::execution::Unrunnable::PackageNotHeld => format!(
            "this emulator holds no deployment package for the version {subject} was \
             addressed to, so there was nothing to run it on"
        ),
        super::execution::Unrunnable::PackageUnreadable(detail) => format!(
            "the deployment package of the version {subject} was addressed to could not be \
             read: {detail}"
        ),
        super::execution::Unrunnable::Overloaded(detail) => format!(
            "no capacity was found to run the function for {subject}: {detail}"
        ),
        super::execution::Unrunnable::Internal(detail) => {
            format!("the function could not be run for {subject}: {detail}")
        }
    }
}

/// What the sentences above call a QUEUED EVENT, which is the work this module's own
/// delivery makes.
pub(crate) const THE_EVENT: &str = "the event";
