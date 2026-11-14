// SPDX-License-Identifier: CC0-1.0

//! The POLLER behind an event source mapping whose event source is an Amazon SQS queue.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda
//! Event Function. Lambda reads items from the event source and invokes the function", and
//! `Enabled` is "when true, the event source mapping is active. When false, Lambda pauses
//! polling and invocation" — so a mapping that reports `Enabled` is one that READS from its
//! source. ONE pass of this poller is the whole of what such a mapping does with no client
//! action at all:
//!
//! * POLL. A batch of at most `BatchSize` messages is taken out of the queue the mapping
//!   names, through the QUEUE's own delivery bookkeeping
//!   (`service_com_amazonaws_sqs_amazonsqs::state::Queue::hand_over`), the one path every
//!   consumer of a queue goes through, so a delivery the poller makes counts exactly as a
//!   client's `ReceiveMessage` does: the message's `ApproximateReceiveCount` rises, it is
//!   hidden while a consumer holds it and it becomes visible again when that hold lapses.
//!   The hold covers the whole span this poller holds the batch — the window it gathers it
//!   over plus the attempt it hands it to, which is the queue's own `VisibilityTimeout` or
//!   the function's `Timeout`, whichever is longer, since one attempt cannot outlast the
//!   function's — and it is RENEWED while an attempt is still running ([`renew_hold`]),
//!   because what an attempt costs this emulator is not bounded by those spans alone: an
//!   invocation also waits for its share of the run capacity. So no message is handed to a
//!   second consumer while the first attempt at it may still be running (see
//!   [`invisible_for_seconds`]). A mapping carrying a
//!   `MaximumBatchingWindowInSeconds` — "the maximum amount of time, in seconds, that Lambda
//!   spends gathering records before invoking the function" — holds a batch it has not
//!   filled for up to that long, taking up what arrives meanwhile, so its function is
//!   invoked with the records that accumulated over the window rather than with each as it
//!   arrives ([`gathered`]). A poll also
//!   applies what the queue's own `RedrivePolicy` owes first, so a
//!   message that has had every delivery that policy allows is moved to the dead-letter
//!   queue the policy names instead of being handed over again. The reads of that poll are
//!   made AS the function's execution role, so a pass whose role the policies in force on it
//!   do not let receive from the queue — the role's own documents, or the QUEUE's own
//!   `Policy` attribute, which fences the queue off from every consumer it denies — takes
//!   nothing out of it at all
//!   ([`super::event_source_authorization`]): the messages stay where they are until the
//!   grant is in place, which is the same question the call that WIRED the pair together was
//!   refused for.
//! * FILTER. `FilterCriteria` is "an object that defines the filter criteria that determine
//!   whether Lambda should process an event", so a polled message that matches none of the
//!   mapping's patterns is not handed to the function. On a QUEUE source such a message is
//!   also removed from the queue, because nothing else would: leaving it would hand it back
//!   to the same filter at every poll until the queue gave up on it.
//! * INVOKE. The messages the filter admits are handed to the mapping's function as ONE
//!   event, over the in-process callback into the `Invoke` surface of this same emulator
//!   ([`smithy_server_runtime::EmbeddedEmulator`]), so nothing is delivered that this
//!   emulator does not serve itself. A pass hands over as many batches AT ONCE as the
//!   mapping may have invocations in flight ([`in_flight_batches`]) — "Limits the number of
//!   concurrent instances that the Amazon SQS event source can invoke"
//!   (`ScalingConfig$MaximumConcurrency`) — so a mapping draining a backlog really does run
//!   batches concurrently up to what it configures, and the drain rate a client sized that
//!   setting for is the one it gets.//! * DELETE. A message leaves the queue only once the invocation it was handed to has
//!   succeeded. A batch whose invocation did not succeed is left exactly where it was: its
//!   messages come back when their visibility timeout lapses, which is precisely the case a
//!   queue's `RedrivePolicy` exists for, and the mapping reports what the attempt came to as
//!   its `LastProcessingResult`. A mapping whose `FunctionResponseTypes` asks for its
//!   function's report of which messages of the batch failed deletes only the messages that
//!   report leaves out, and leaves the rest where they were — so one message of a batch can
//!   be retried without the ones beside it being delivered again (see
//!   [`super::batch_item_failures`]). The report names MESSAGES, so the set deleted is its
//!   complement whatever `MessageGroupId` each message carries: a message of the same group
//!   as a reported failure is deleted when the report leaves it out, which is why a function
//!   on a FIFO source that wants its group's order kept reports the messages after a failure
//!   as failed too.
//! * METER. While the mapping's `MetricsConfig` selects `EventCount`, what the pass did with
//!   its events — how many it polled, how many the filter rejected, how many an invocation
//!   accepted and how many it deleted — is published for that mapping to CloudWatch (see
//!   [`super::metrics`]), including a 0-valued poll count for a minute in which the mapping
//!   polled nothing.
//!
//! A function whose deployment package this emulator can run no engine for is one no
//! invocation of can succeed: such an invocation is refused, the batch is not deleted, and
//! the counts the pass publishes say exactly that — the events were polled, none was
//! invoked and none was deleted. Nothing here reports a success, or counts an event as
//! processed, that no invocation earned.
//!
//! # How a poll is driven
//!
//! Nothing sleeps inside a request. The poll loop is a background task claimed once per
//! store ([`drive_sqs_pollers`], armed by the calls that make or change a mapping) which
//! polls, hands the runtime back for [`POLL_INTERVAL`] and polls again, and stands down as
//! soon as the store holds no enabled queue mapping — so its lifetime is bounded by the
//! resource it serves rather than by a timer that would leave a mapping reporting `Enabled`
//! while nothing polls it. One wake-up's work is bounded too: at most one batch per mapping,
//! and the store holds at most [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings. With no async
//! runtime to spawn onto there is no task, and [`poll_due`] — which is the whole of one
//! wake-up's work and takes the instant it is working at — can be awaited directly instead.
//! A caller that wants the passes to be ITS OWN, made when it says so rather than on the
//! loop's interval, takes the store's poll claim ([`claim_poll_passes`]) and then makes each
//! pass with [`poll_passes_once`]: the same work, awaited rather than raced.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::serde_json::{json, Value};
use ::service_com_amazonaws_sqs_amazonsqs as sqs;

use super::batch_item_failures::{
    processed_records, read_batch_item_failures, reports_batch_item_failures, BatchReport,
};
use super::event_source_authorization::IamState;
use super::event_source_poller::Clients;
use super::metrics::{EventCounts, FunctionMetrics};
use super::{
    service_state, ComAmazonawsLambdaAwsgirapiserviceState, EVENT_SOURCE_MAPPING_STATE_ENABLED,
    EVENT_SOURCE_VENDOR_QUEUE, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
    LAST_PROCESSING_RESULT_OK, STATE_ACTIVE,
};

/// How long the poll loop waits between two polls of a mapping's queue.
///
/// An EMULATOR-CHOSEN span, like the other spans this crate owns: Lambda publishes no poll
/// frequency for an event source mapping, only that the mapping polls while it is enabled.
/// It is deliberately non-zero, so the loop hands the runtime back between wake-ups instead
/// of spinning, and short enough that a caller watching its queue sees the mapping consume.
pub const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How often the hold on a batch an attempt is still running on is renewed, at the fastest.
///
/// An EMULATOR-CHOSEN floor, for the same reason as [`POLL_INTERVAL`]: a queue whose
/// `VisibilityTimeout` is a second or two leaves no room to renew the hold at half its span,
/// so the renewal is made on this floor instead of being skipped. It is well inside the
/// smallest hold a delivery takes, so a renewed hold never has a gap in it.
const MIN_HOLD_RENEWAL: Duration = Duration::from_millis(250);

/// The most messages this emulator takes out of a queue in ONE batch, whatever `BatchSize`
/// the mapping configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` reaches 10,000 for a
/// standard queue): the whole batch is held in memory and becomes one invocation payload, so
/// the count a caller chose is clamped before anything is polled, exactly as the reader of a
/// shard source clamps it. A mapping configured above it polls this many messages per batch
/// instead, which is a smaller batch and never a message that is not polled.
const MAX_MESSAGES_PER_BATCH: usize = 1_000;

/// The longest batching window a mapping gathers a batch over.
///
/// Source: `MaximumBatchingWindowInSeconds`' own `@range(min: 0, max: 300)`. The boundary
/// validator already refuses a request past it; this is that same bound applied where the
/// POLLER relies on it, since the value is read back out of stored state.
const MAX_BATCHING_WINDOW_SECONDS: i32 = 300;

/// The most concurrent invocations an `@range`-bounded `ScalingConfig` may ask a queue
/// mapping to scale to, and the fewest: `MaximumConcurrency`'s own
/// `@range(min: 2, max: 1000)`
/// (`smithy_query(query="shape", shape="MaximumConcurrency")`). The boundary validator
/// already refuses a request outside it; this is that same bound applied where the POLLER
/// relies on it, since the value is read back out of stored state.
const MAX_SOURCE_CONCURRENCY: i32 = 1_000;
/// The other end of that range; see [`MAX_SOURCE_CONCURRENCY`].
const MIN_SOURCE_CONCURRENCY: i32 = 2;

/// How many batches one pass of a mapping that configures NO concurrency hands over: the one
/// this poller has always handed over per wake-up, which is its own cadence rather than
/// anything AWS publishes.
const ONE_BATCH_PER_PASS: usize = 1;

/// The `eventSource` each record of the event a queue mapping hands its function carries.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// The member a record reports the digest of its MESSAGE ATTRIBUTES under, beside
/// `md5OfBody` — the counterpart of `Message$MD5OfMessageAttributes`, which Amazon SQS
/// publishes for a message carrying message attributes and for no other.
const RECORD_MD5_OF_MESSAGE_ATTRIBUTES: &str = "md5OfMessageAttributes";

/// How much work ONE pass may spend deciding which of its messages its mapping's filter
/// criteria admit, in the steps the shared matcher charges.
///
/// The pass is background work on a batch bounded by `BatchSize` against at most the
/// patterns a mapping may carry, so this is the ONE explicit cap on the product of the two.
const MAX_FILTER_STEPS_PER_BATCH: u64 = ::service_common::event_pattern::MAX_MATCH_STEPS_PER_REQUEST;

/// How often a mapping whose batch keeps failing says so: the first failure, and every this
/// many after it, so one mapping nothing can process cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// How long one minute is, in seconds: the window an idle mapping publishes at most one
/// 0-valued poll count in. A series published at the regular storage resolution is
/// aggregated per minute ([`super::metrics::STANDARD_STORAGE_RESOLUTION_SECONDS`]), so one
/// zero per minute is what a reader of such a series can tell apart.
const SECONDS_PER_MINUTE: f64 = 60.0;

/// One mapping's queue, as the poll loop reads it out of the store before it touches a
/// queue: the mapping to record the attempt against, the queue to poll, the function the
/// batch is handed to and how long it may run for, how many messages one batch may hold,
/// and what the mapping asked be filtered and metered.
struct PollTarget {
    /// `UUID`, which is the key the mapping is stored under and the dimension its own
    /// series are reported under.
    uuid: String,
    /// `EventSourceArn`, the queue to take the batch out of.
    queue_arn: String,
    /// The Region that queue lives in, which is the Region this pass reaches this
    /// emulator's services in: a resource belongs to one Region's namespace.
    region: String,
    /// `FunctionArn`: "The ARN of the Lambda function" the batch is handed to.
    function_arn: String,
    /// That function's `Timeout`, in seconds: "the amount of time that Lambda allows a
    /// function to run before stopping it" — so the LONGEST one attempt at this batch can
    /// last, which is the span the batch has to stay in flight for.
    function_timeout_seconds: i64,
    /// The EXECUTION ROLE of the function version this mapping invokes, which is who the
    /// poller reads the queue as; `None` when this store holds no configuration to read it
    /// from.
    role_arn: Option<String>,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from
    /// your stream or queue".
    batch_size: usize,
    /// How many batches this mapping may have in flight at ONE instant, which is how many
    /// invocations of its function one pass hands over concurrently ([`in_flight_batches`]).
    in_flight_batches: usize,
    /// `MaximumBatchingWindowInSeconds`: "the maximum amount of time, in seconds, that
    /// Lambda spends gathering records before invoking the function" — so a mapping that
    /// configures one holds a batch it has not filled for up to that long, taking up what
    /// arrives meanwhile, instead of invoking its function with the first messages it finds.
    /// Zero (which is the default for a queue source) hands over what one poll found.
    batching_window: Duration,
    /// The `Pattern` of each of the mapping's `FilterCriteria` filters, as stored.
    filters: Vec<String>,
    /// Whether the mapping's `FunctionResponseTypes` asks for the function's report of which
    /// messages of the batch failed to be read (see [`super::batch_item_failures`]).
    reports_batch_item_failures: bool,
    /// Whether the mapping's `MetricsConfig` selects the event-count family.
    meters_event_counts: bool,
}

/// One message this pass polled: the message itself, and the record the event handed to the
/// function carries it as.
#[derive(Debug)]
struct PolledMessage {
    /// The id the queue holds the message under, which is what a delete names it by.
    message_id: String,
    /// The handle the delivery of this message minted, which is what the hold on it is
    /// renewed under while the poller is still working on the batch (see [`renew_hold`]).
    receipt_handle: String,
    /// The record envelope, as the function receives it.
    record: Value,
}

/// What ONE poll loop carries between its wake-ups.
///
/// Nothing in it is state a client can read: it is the loop's own bookkeeping — the clients
/// it reaches this emulator through, and per mapping how recently it published an idle poll
/// count and how many attempts in a row have failed, which is what keeps one zero per minute
/// and one log line per twenty failures rather than per wake-up.
#[derive(Default)]
pub struct PollSession {
    /// The clients this pass invokes a function through, built on first use.
    clients: Clients,
    /// For each mapping that is GATHERING a batch, what it has gathered so far — a mapping
    /// with a `MaximumBatchingWindowInSeconds` of its own, whose batch is neither full nor
    /// out of time yet.
    gathering: BTreeMap<String, Gathering>,
    /// For each mapping, the minute (epoch seconds divided by sixty) its last 0-valued poll
    /// count was published in.
    idle_minute: BTreeMap<String, i64>,
    /// For each mapping, how many attempts in a row have failed.
    failures: BTreeMap<String, u32>,
}

/// The batch a mapping with a batching window is still gathering.
///
/// Its messages are already OUT of the queue — the poll that took them hid them from every
/// other consumer for the whole span this hold covers (see [`invisible_for_seconds`]) — so
/// they are held here until the batch is full or the window has run, and then handed to the
/// function as one event.
#[derive(Debug)]
struct Gathering {
    /// When the first message of this batch was gathered, which is what the window is
    /// measured from.
    opened_at: SystemTime,
    /// The messages gathered so far, in the order the queue handed them over.
    messages: Vec<PolledMessage>,
}

impl ::std::fmt::Debug for PollSession {
    /// The bookkeeping, without the clients: an SDK client has no rendering of its own.
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PollSession")
            .field("gathering", &self.gathering)
            .field("idle_minute", &self.idle_minute)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

/// Start the background poll loop for this engine's mappings, if one is not already running.
///
/// Called by the operations that make or change a mapping. At most one loop runs per store
/// (the store carries the claim), it polls for as long as the store holds a mapping that
/// wants polling and stands down when it does not, and with no async runtime to spawn onto
/// there is no loop — a caller can then drive the same work through [`poll_due`].
pub fn drive_sqs_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let Some(generation) = claim_poller(&mut state.lock()) else {
        return;
    };
    let callback = ctx.callback().clone();
    // The CloudWatch store this loop's work is published to, resolved once because the
    // handle does not change.
    let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&**engine));
    let engine = Arc::clone(engine);
    let claimed = ::engine::ServiceState::clone(&state);
    let handle = runtime.spawn(async move {
        let mut session = PollSession::default();
        // The loop ends when nothing polls any more, which it observes — and releases its
        // claim in — inside one critical section.
        while polling_wanted_or_release(&mut state.lock(), generation) {
            // Every wake-up waits, which is what hands the runtime back between polls. No
            // lock is held across it.
            ::tokio::time::sleep(POLL_INTERVAL).await;
            poll_due(
                &callback,
                &mut session,
                &*engine,
                &metrics,
                SystemTime::now(),
            )
            .await;
        }
    });
    // The handle is kept so the loop it replaces is aborted rather than merely forgotten.
    set_poller(&mut claimed.lock(), generation, handle);
}

/// Take the one poll claim of `engine`'s store, so the passes of its queue mappings are the
/// CALLER's to make: `true` when this call took it, `false` when a loop already holds it.
///
/// [`drive_sqs_pollers`] starts a background loop only for a store whose claim is free, so
/// while this claim is held nothing but the caller polls that store: each pass is one
/// [`poll_passes_once`] call, made at the instant the caller chooses rather than on a wake-up
/// interval. That is what lets a host advance a mapping's data plane IN STEP with the calls
/// it makes — the same work, every rule this module states, awaited instead of raced — which
/// is otherwise impossible to do, since nothing a caller can read says whether a pass has
/// happened yet. The claim is kept for as long as the store lives: a caller that has taken
/// over the passes keeps making them, and no loop takes over behind it.
pub fn claim_poll_passes<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> bool {
    service_state(&**engine).is_some_and(|state| claim_poller(&mut state.lock()).is_some())
}

/// ONE pass over every queue mapping of `engine`'s store, at `now`, for a caller that makes
/// the passes itself ([`claim_poll_passes`]).
///
/// The whole of one wake-up's work ([`poll_due`]), with the CloudWatch store the pass
/// publishes to resolved for the caller, so a caller driving the passes names only what it
/// already holds. It ANSWERS once the pass is done — every batch this pass took has been
/// handed to its function and what the attempt came to is recorded — so a caller needs no
/// span of its own to know the pass has happened.
///
/// `session` is the bookkeeping one pass carries to the next ([`PollSession`]), so a caller
/// making a series of passes keeps one across them, exactly as the loop does.
pub async fn poll_passes_once<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Arc<Eng>,
    now: SystemTime,
) {
    let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&**engine));
    poll_due(callback, session, &**engine, &metrics, now).await;
}

/// One wake-up's work: poll every enabled queue mapping this engine holds, at `now`.
///
/// For each such mapping, in turn: whatever the clock already owed on the queues is applied,
/// the queue's `RedrivePolicy` is applied to the messages that have had every delivery it
/// allows, a batch of at most the mapping's `BatchSize` messages is handed over, the
/// mapping's filter criteria decide which of them the function is invoked with, and what the
/// pass did is metered. See this module's documentation for what each step is, and why a
/// batch no invocation accepted is left on the queue.
///
/// The two stores are never locked at the same time, and no lock is held across an
/// invocation: each step takes one lock, finishes with it and releases it before the next
/// step takes the other.
pub async fn poll_due<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Eng,
    metrics: &FunctionMetrics,
    now: SystemTime,
) {
    let Some(mappings) = service_state(engine) else {
        return;
    };
    let Some(queues) = sqs::state::service_state(engine) else {
        return;
    };
    // The IAM store the execution role of each mapping's function is judged against,
    // resolved once per pass because the handle does not change. An engine carrying none
    // shows nothing about any role.
    let iam = super::event_source_authorization::iam_state(engine);
    // The stores an event source's OWN policy lives in, resolved for the same reason: a
    // queue is fenced by its `Policy` attribute whoever reads it, this poller included.
    let sources = super::event_source_resource_policy::SourcePolicies::of(engine);
    let targets = {
        let mut store = mappings.lock();
        store.materialize_all(now);
        polling_targets(&store)
    };
    let at_secs = epoch_secs(now);
    // A mapping that no longer polls — disabled, deleted, or its function no longer
    // invocable — is gathering nothing: anything it held stays on its queue and becomes
    // visible again when the hold on it lapses.
    let polling: ::std::collections::BTreeSet<String> =
        targets.iter().map(|target| target.uuid.clone()).collect();
    session.gathering.retain(|uuid, _| polling.contains(uuid));
    for target in targets {
        // Lambda reads the queue ACTING AS the function's execution role, so the reads this
        // pass would make are subject to the policies in force on that role AT THIS INSTANT
        // rather than when the pair was wired: while one of them is withheld, the pass takes
        // nothing out of the queue — nothing is hidden, invoked or deleted — and the mapping
        // consumes again as soon as the grant is back (see
        // [`super::event_source_authorization`], including the cases that show nothing about
        // a role and withhold nothing).
        if let Some(detail) = withheld_queue_read(iam.as_ref(), &sources, &target, now) {
            note_unpollable(session, &target, &detail);
            // A mapping that may take nothing out of its queue is not gathering either: the
            // batch it was holding is let go, so the records in it are the queue's to hand
            // over again once their hold lapses — rather than being handed over later, under
            // receipt handles that lapsed with it, by a pass whose window has long run.
            session.gathering.remove(&target.uuid);
            continue;
        }
        // How long one delivery of this queue's messages hides them for, which is also the
        // span the hold on the batch is renewed by while the attempt runs.
        let hold_seconds = batch_hold(&queues, &target);
        // What this mapping has to hand over NOW: as many batches as it may have invocations
        // in flight, each of at most its `BatchSize` messages.
        let batches = match due_batches(&queues, session, &target, now, hold_seconds) {
            Due::Batches(batches) => batches,
            // Still gathering: the messages are held, so nothing is invoked, deleted or
            // metered for them yet. The pass that hands them over reports all of it.
            Due::Gathering => continue,
            Due::Nothing => {
                // Nothing was waiting, so no processing attempt was made and there is no
                // result of one to report. The poll itself is still a measurement: a mapping
                // that polled an empty queue says so, at most once a minute.
                if target.meters_event_counts && claim_idle_minute(session, &target.uuid, at_secs)
                {
                    metrics.publish_idle_poll(&target.uuid, at_secs);
                }
                continue;
            }
        };
        // What the filter admits of each batch, decided before any of them is handed over: a
        // message the filter rejected is removed from the queue, because nothing else would
        // and the mapping would otherwise be handed it again at every poll.
        let mut counts = EventCounts {
            polled: 0,
            filtered_out: 0,
            invoked: 0,
            deleted: 0,
        };
        let admitted: Vec<AdmittedBatch> = batches
            .into_iter()
            .map(|polled| {
                let (invoking, filtered_out) = partition_by_filters(&target, &polled);
                delete_messages(&queues, &target, &filtered_out);
                counts.polled = counts.polled.saturating_add(count_of(polled.len()));
                counts.filtered_out = counts
                    .filtered_out
                    .saturating_add(count_of(filtered_out.len()));
                counts.deleted = counts.deleted.saturating_add(count_of(filtered_out.len()));
                AdmittedBatch {
                    payload: queue_event(&invoking, &polled),
                    polled,
                    invoking,
                }
            })
            .collect();
        // The client every attempt of this pass is made through, resolved ONCE: the
        // invocations below are in flight together, so none of them may hold the session the
        // client is built on.
        let client = session.clients.lambda(callback, &target.region).clone();
        // CONCURRENTLY: each batch the mapping may have in flight is handed to the function
        // while the others are, so a mapping configured for several concurrent instances
        // really runs several at once (see this module's documentation).
        let attempts = ::futures_util::future::join_all(
            admitted
                .iter()
                .map(|batch| batch_attempt(&client, &target, batch, &queues, hold_seconds)),
        )
        .await;
        // What each attempt came to, recorded one at a time: the queue writes and this
        // session's own bookkeeping are the pass's, not an attempt's.
        for (batch, attempt) in admitted.iter().zip(attempts) {
            match attempt {
                Ok(processed) => {
                    // The messages the attempt PROCESSED leave the queue — that is what a
                    // consumer of a queue does with a message it has handled — and the ones
                    // its function reported it did not process stay exactly where they were,
                    // so each comes back when its visibility timeout lapses and is handed
                    // over again.
                    counts.invoked = counts
                        .invoked
                        .saturating_add(count_of(batch.invoking.len()));
                    delete_messages(&queues, &target, &processed);
                    counts.deleted = counts.deleted.saturating_add(count_of(processed.len()));
                    session.failures.remove(&target.uuid);
                    record_attempt(&mappings, &target, LAST_PROCESSING_RESULT_OK);
                }
                Err(detail) => {
                    // Not processed: nothing is deleted, so every message comes back when
                    // its visibility timeout lapses.
                    let failures = session
                        .failures
                        .entry(target.uuid.clone())
                        .and_modify(|failures| *failures = failures.saturating_add(1))
                        .or_insert(1);
                    if *failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                        ::tracing::warn!(
                            mapping = %target.uuid,
                            attempt = *failures,
                            detail = %detail,
                            "an event source mapping could not hand a batch from its queue \
                             to its function; the messages stay on the queue and it tries \
                             again"
                        );
                    }
                    record_attempt(
                        &mappings,
                        &target,
                        LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                    );
                }
            }
        }
        if target.meters_event_counts {
            metrics.publish_event_counts(&target.uuid, &counts, at_secs);
            // This pass polled, so the next idle minute is one to publish a zero for.
            session.idle_minute.remove(&target.uuid);
        }
    }
}

/// One batch of a pass, with what its mapping's filter criteria admitted of it: the messages
/// the poll took out of the queue, the ids of the ones the function is invoked with, and the
/// event they are handed over as.
struct AdmittedBatch {
    /// Every message this batch took out of the queue, in the order the queue handed them
    /// over — the ones the hold covers.
    polled: Vec<PolledMessage>,
    /// The ids of the messages the filter admitted, which is what the attempt reports on.
    invoking: Vec<String>,
    /// The event the function is invoked with: the records of the admitted messages.
    payload: Value,
}

/// What a mapping has to hand its function at `now`.
enum Due {
    /// These batches: hand each of them over.
    Batches(Vec<Vec<PolledMessage>>),
    /// The mapping is still gathering its first batch — it holds messages, and neither its
    /// `BatchSize` nor its batching window is up yet.
    Gathering,
    /// Nothing is held and the queue had nothing to hand over.
    Nothing,
}

/// Take the batches `target` may have in flight at once out of its queue at `now`, each
/// hidden from every other consumer for `hold_seconds` ([`batch_hold`]).
///
/// One poll per batch, up to [`PollTarget::in_flight_batches`] of them, stopping as soon as
/// the queue has nothing more to hand over — so a mapping with no backlog takes exactly the
/// one batch a serial pass would, and one draining a backlog takes as many as it may invoke
/// its function with concurrently. A mapping GATHERING a batch over a batching window holds
/// it as it did ([`gathered`]): the window is what decides when that batch goes, and the
/// batches beside it are the ones a later poll of this same pass found room for.
fn due_batches(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    session: &mut PollSession,
    target: &PollTarget,
    now: SystemTime,
    hold_seconds: i64,
) -> Due {
    let mut batches: Vec<Vec<PolledMessage>> = Vec::new();
    while batches.len() < target.in_flight_batches {
        let polled = take_batch(
            queues,
            target,
            now,
            gathering_room(session, target),
            hold_seconds,
        );
        match gathered(session, target, polled, now) {
            Gathered::Due(polled) => batches.push(polled),
            Gathered::Gathering => break,
            Gathered::Nothing => break,
        }
    }
    if batches.is_empty() {
        // Which of the two it is matters: a mapping holding a batch it has not finished
        // gathering polled something, and one whose queue was empty polled nothing and is the
        // idle poll a mapping meters.
        return if session.gathering.contains_key(&target.uuid) {
            Due::Gathering
        } else {
            Due::Nothing
        };
    }
    Due::Batches(batches)
}

/// Why the reads this pass would make on `target`'s queue are WITHHELD — from the execution
/// role of the function the mapping invokes, or by the queue's own policy — or `None` when
/// they are not, which includes every case that shows nothing about either
/// (see [`super::event_source_authorization`]).
///
/// Only the IAM store and the queue's own are locked here, one at a time, and this service's
/// is not: the decision is taken between two steps of the pass rather than inside either of
/// them.
fn withheld_queue_read(
    iam: Option<&::engine::ServiceState<IamState>>,
    sources: &super::event_source_resource_policy::SourcePolicies,
    target: &PollTarget,
    now: SystemTime,
) -> Option<String> {
    let role_arn = target.role_arn.as_deref()?;
    super::event_source_authorization::unreadable_event_source_held(
        iam,
        sources,
        role_arn,
        &target.queue_arn,
        now,
    )
}

/// Report that a mapping's execution role may not read its queue, at most once every
/// [`FAILURE_LOG_INTERVAL`] passes so one such mapping cannot fill the log.
///
/// Nothing is recorded on the mapping: no batch was polled, so there is no processing
/// attempt whose result to report — exactly as for a pass that found the queue empty.
fn note_unpollable(session: &mut PollSession, target: &PollTarget, detail: &str) {
    let passes = session
        .failures
        .entry(target.uuid.clone())
        .and_modify(|passes| *passes = passes.saturating_add(1))
        .or_insert(1);
    if *passes == 1 || passes.is_multiple_of(FAILURE_LOG_INTERVAL) {
        ::tracing::warn!(
            mapping = %target.uuid,
            attempt = *passes,
            detail = %detail,
            "an event source mapping may not read its queue as its function's execution \
             role; the messages stay on the queue until that role is granted the read"
        );
    }
}

/// How long the batch this pass hands to a function stays invisible to every other
/// consumer of `queue_visibility_timeout_seconds`'s queue, in seconds.
///
/// A delivery hides a message for the queue's own `VisibilityTimeout`, which is the span a
/// message is handed to ONE consumer for. A poller holding a batch for a function is that
/// one consumer for as long as it holds it: the window it gathers the batch over plus the
/// attempt it then hands it to, and it keeps the batch's visibility extended for that whole
/// span rather than letting it lapse part way through — a message that came back mid-attempt
/// would be handed to a second consumer while the first was still working on it, which is a
/// delivery nothing asked for. One attempt cannot outlast the function's own `Timeout` —
/// "the amount of time that Lambda allows a function to run before stopping it" — so
/// holding the batch for the gathering window plus the longer of the two spans covers the
/// whole hold whatever it comes to, and leaves the queue's own timeout in charge whenever it
/// is already the longer one. That is also what the comparison a mapping is ACCEPTED against
/// assumes (`crate::state::queue_visibility_refusal`); the hold is what keeps it true after
/// a client lowers the queue's timeout or raises the function's, neither of which the other
/// service knows about.
///
/// Bounded by the range the attribute itself takes, since the result is how long one
/// delivery hides a message for.
fn invisible_for_seconds(
    queue_visibility_timeout_seconds: i64,
    function_timeout_seconds: i64,
    gathering_window_seconds: i64,
) -> i64 {
    let (floor, ceiling) = sqs::state::integer_bounds(sqs::state::VISIBILITY_TIMEOUT);
    queue_visibility_timeout_seconds
        .max(function_timeout_seconds)
        .saturating_add(gathering_window_seconds)
        .clamp(floor, ceiling)
}

/// How many batches a mapping may have in flight at ONE instant, which is how many
/// invocations of its function one pass hands over concurrently.
///
/// "Limits the number of concurrent instances that the Amazon SQS event source can invoke"
/// (`ScalingConfig$MaximumConcurrency`) is what a mapping configuring one scales to: that
/// many batches are handed over together, read inside the range the member admits
/// ([`MAX_SOURCE_CONCURRENCY`]) however the value got into the store. A mapping configuring
/// none keeps this poller's own cadence of [`ONE_BATCH_PER_PASS`], which is what it has
/// always handed over per wake-up.
///
/// Two further bounds hold whatever a mapping configures, because neither is the mapping's to
/// raise:
///
/// * "The number of simultaneous executions to reserve for the function"
///   (`PutFunctionConcurrency`) is all the concurrency the function has, so a mapping cannot
///   have more invocations of it in flight than that reservation admits;
/// * the runs this process can hold at once ([`super::execution::MAX_CONCURRENT_RUNS`]),
///   since a batch taken for an invocation that would wait for run memory is a batch held
///   invisible to every other consumer for no gain.
///
/// Never zero: a pass always hands over the one batch it would have handed over anyway, and
/// an invocation the function's reservation has no room for is refused by the admission that
/// counts it ([`super::execution_slot`]) rather than by the poll that polled for it.
#[must_use]
pub fn in_flight_batches(
    maximum_concurrency: Option<i32>,
    reserved_concurrent_executions: Option<i32>,
) -> usize {
    let Some(configured) = maximum_concurrency else {
        return ONE_BATCH_PER_PASS;
    };
    let configured = configured.clamp(MIN_SOURCE_CONCURRENCY, MAX_SOURCE_CONCURRENCY);
    let reserved = reserved_concurrent_executions.unwrap_or(i32::MAX);
    let host = i32::try_from(super::execution::MAX_CONCURRENT_RUNS).unwrap_or(i32::MAX);
    usize::try_from(configured.min(reserved).min(host))
        .unwrap_or(ONE_BATCH_PER_PASS)
        .max(ONE_BATCH_PER_PASS)
}

/// How many more messages `target` may take out of its queue on this pass: its `BatchSize`,
/// less whatever it has already gathered towards the batch it is holding.
fn gathering_room(session: &PollSession, target: &PollTarget) -> usize {
    let held = session
        .gathering
        .get(&target.uuid)
        .map_or(0, |gathering| gathering.messages.len());
    target.batch_size.saturating_sub(held)
}

/// What a mapping has to hand its function at `now`.
enum Gathered {
    /// This batch: hand it over.
    Due(Vec<PolledMessage>),
    /// The mapping is still gathering — it holds messages, and neither its `BatchSize` nor
    /// its batching window is up yet.
    Gathering,
    /// Nothing is held and the queue had nothing to hand over.
    Nothing,
}

/// Whether the batch a mapping has gathered is DUE — its function is handed it now — or
/// whether the mapping goes on gathering.
///
/// The two reasons a batch is due, and nothing else: it holds "the maximum number of records
/// in each batch that Lambda pulls from your stream or queue" (`BatchSize`), or it has been
/// gathering for the whole of "the maximum amount of time, in seconds, that Lambda spends
/// gathering records before invoking the function" (`MaximumBatchingWindowInSeconds`) — both
/// MAXIMA, so a batch that fills goes without waiting the rest of the window out, and a
/// window that has run hands over whatever was gathered however small it is. A batch holding
/// nothing is never due: there is nothing to hand over.
///
/// `pub` because it is the whole of the decision this poller takes about a gathered batch,
/// stated in one place.
#[must_use]
pub fn batch_is_due(
    gathered: usize,
    batch_size: usize,
    waited: Duration,
    window: Duration,
) -> bool {
    gathered > 0 && (gathered >= batch_size || waited >= window)
}

/// Fold what this pass polled into what `target` is gathering, and decide whether the batch
/// is due.
///
/// A mapping with no batching window gathers nothing: what one poll found is the batch, as
/// `MaximumBatchingWindowInSeconds` being zero says. A mapping WITH one holds its batch until
/// [`batch_is_due`] says it may go, measured from the first message of that batch — so the
/// function is invoked with the records that accumulated over the window rather than with
/// each as it arrives.
fn gathered(
    session: &mut PollSession,
    target: &PollTarget,
    polled: Vec<PolledMessage>,
    now: SystemTime,
) -> Gathered {
    if target.batching_window.is_zero() {
        return if polled.is_empty() {
            Gathered::Nothing
        } else {
            Gathered::Due(polled)
        };
    }
    let gathering = session
        .gathering
        .entry(target.uuid.clone())
        .or_insert_with(|| Gathering {
            opened_at: now,
            messages: Vec::new(),
        });
    if gathering.messages.is_empty() {
        // The window is measured from the first message gathered, not from the pass that
        // found the queue empty.
        gathering.opened_at = now;
    }
    gathering.messages.extend(polled);
    if gathering.messages.is_empty() {
        session.gathering.remove(&target.uuid);
        return Gathered::Nothing;
    }
    let waited = now
        .duration_since(gathering.opened_at)
        .unwrap_or_default();
    if !batch_is_due(
        gathering.messages.len(),
        target.batch_size,
        waited,
        target.batching_window,
    ) {
        return Gathered::Gathering;
    }
    let batch = ::std::mem::take(&mut gathering.messages);
    session.gathering.remove(&target.uuid);
    Gathered::Due(batch)
}

/// Take the next batch out of `target`'s queue at `now`, as the records the function would
/// be handed them as — at most `room` messages, which is what is left of its `BatchSize`,
/// each hidden from every other consumer for `hold_seconds` ([`batch_hold`]).
///
/// Empty when the mapping names a queue this engine no longer holds, when the queue had
/// nothing it could hand over, and when the batch being gathered is already full.
fn take_batch(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    target: &PollTarget,
    now: SystemTime,
    room: usize,
    hold_seconds: i64,
) -> Vec<PolledMessage> {
    if room == 0 {
        return Vec::new();
    }
    let mut store = queues.lock();
    // Retention lapses, visibility timeouts and message movement tasks are owed by the
    // clock, so what the delivery below sees is what the queue may hand out.
    store.materialise_due(now);
    let Some((region, name)) = store
        .queue_by_arn(&target.queue_arn)
        .map(|queue| (queue.region.clone(), queue.name.clone()))
    else {
        return Vec::new();
    };
    // A message this queue has already handed over as many times as its `RedrivePolicy`
    // allows is not handed over again: it goes to the dead-letter queue that policy names,
    // BEFORE this poll decides what there is to deliver.
    store.dead_letter_due(&region, &name, now);
    let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
        return Vec::new();
    };
    // The batch is hidden for as long as this poller holds it: the window it is gathered
    // over and the attempt it is then handed to; see `invisible_for_seconds`, and
    // [`renew_hold`] for the attempt that outlasts one span of it.
    let handed = queue.hand_over(now, room, sqs::state::after(now, hold_seconds));
    handed
        .iter()
        .filter_map(|(index, delivered)| {
            let message = queue.messages.get(*index)?;
            Some(PolledMessage {
                message_id: delivered.message_id.clone(),
                receipt_handle: delivered.receipt_handle.clone(),
                record: queue_record(message, &delivered.receipt_handle, &target.queue_arn, &region),
            })
        })
        .collect()
}

/// How long one delivery of `target`'s queue hides the messages it hands over for, in
/// seconds: the span [`invisible_for_seconds`] states, read off the queue's OWN current
/// `VisibilityTimeout`.
///
/// Zero for a queue this engine no longer holds — there is no batch of it to hold.
fn batch_hold(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    target: &PollTarget,
) -> i64 {
    let store = queues.lock();
    let Some(queue) = store.queue_by_arn(&target.queue_arn) else {
        return 0;
    };
    invisible_for_seconds(
        queue.integer(sqs::state::VISIBILITY_TIMEOUT),
        target.function_timeout_seconds,
        i64::try_from(target.batching_window.as_secs()).unwrap_or(i64::MAX),
    )
}

/// How often the hold on a batch an attempt is running on is renewed: twice per hold span,
/// so the messages stay hidden at least half a span ahead of the attempt at every instant,
/// and never less often than [`MIN_HOLD_RENEWAL`] — a hold of a second or two is renewed on
/// that floor rather than not at all.
fn hold_renewal(hold_seconds: i64) -> Duration {
    let span = Duration::from_secs(u64::try_from(hold_seconds).unwrap_or(0));
    (span / 2).max(MIN_HOLD_RENEWAL)
}

/// The batch a pass is HOLDING while it hands it over: the queue store the hold is renewed
/// in, the messages it covers and the span it is renewed by ([`batch_hold`]).
///
/// One value rather than three parameters, because the three only ever travel together: a
/// renewal is always of THIS pass's batch, in the store it came out of, for the span that
/// store's queue hides one delivery for.
struct HeldBatch<'batch> {
    /// The Amazon SQS store the batch's queue lives in.
    queues: &'batch ::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    /// The messages this pass took out of that queue.
    messages: &'batch [PolledMessage],
    /// How long each renewal hides them for, in seconds.
    hold_seconds: i64,
}

/// Renew the hold on `held`'s messages in `target`'s queue, hiding each of them for the hold
/// span again, counted from `now`.
///
/// One delivery hides a message for one hold span, counted from that delivery; an ATTEMPT
/// still running when that span is up belongs to a consumer that is still working on the
/// batch, so the hold is extended through the queue's own visibility bookkeeping — the path a
/// client's `ChangeMessageVisibility` takes — instead of being let lapse under a running
/// attempt. The span an attempt really takes is not bounded by the function's `Timeout`
/// alone: an invocation also waits for its share of this emulator's run capacity, so a hold
/// computed from the configured spans can be up before the attempt it covers is decided.
///
/// Best effort per message: one the attempt has already had deleted, and one whose hold
/// cannot be extended any further, keep whatever the queue says about them.
fn renew_hold(target: &PollTarget, held: &HeldBatch<'_>, now: SystemTime) {
    let mut store = held.queues.lock();
    let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
        return;
    };
    for message in held.messages {
        let _ = queue.change_visibility(&message.receipt_handle, held.hold_seconds, now);
    }
}

/// Split `polled` into the messages `target`'s filter criteria admit and the ones they
/// reject, by message id.
///
/// A mapping carrying no criteria admits every message — there is nothing to determine
/// otherwise. A mapping whose criteria this emulator cannot read also admits every message:
/// a message rejected on a pattern nothing could evaluate would be deleted from the queue on
/// the strength of a filter that was never applied, so the reading failure withholds the
/// filter rather than the data.
fn partition_by_filters(
    target: &PollTarget,
    polled: &[PolledMessage],
) -> (Vec<String>, Vec<String>) {
    let admit_all = || {
        (
            polled
                .iter()
                .map(|message| message.message_id.clone())
                .collect(),
            Vec::new(),
        )
    };
    if target.filters.is_empty() {
        return admit_all();
    }
    let Some(patterns) = parsed_filters(&target.filters) else {
        ::tracing::warn!(
            mapping = %target.uuid,
            "an event source mapping carries filter criteria this emulator cannot read, so \
             every message it polls is handed to its function"
        );
        return admit_all();
    };
    let mut budget = ::service_common::event_pattern::MatchBudget::new(MAX_FILTER_STEPS_PER_BATCH);
    let mut admitted: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for message in polled {
        let event = filter_event(&message.record);
        // "The collection of event patterns used to filter events": a record matching ANY
        // of them is one the function is invoked with.
        let matched = patterns.iter().any(|pattern| {
            ::service_common::event_pattern::matches_within(pattern, &event, &mut budget)
        });
        if matched {
            admitted.push(message.message_id.clone());
        } else {
            rejected.push(message.message_id.clone());
        }
    }
    (admitted, rejected)
}

/// The mapping's filter patterns, parsed by the ONE reading of the event-pattern language in
/// this tree — `None` when any of them is not a pattern this emulator can match, because a
/// filter is a conjunction of all of them and a partly applied one would admit or reject on
/// criteria the mapping does not carry.
fn parsed_filters(filters: &[String]) -> Option<Vec<Value>> {
    let mut parsed: Vec<Value> = Vec::with_capacity(filters.len());
    for pattern in filters {
        parsed.push(::service_common::event_pattern::parse(pattern).ok()?);
    }
    Some(parsed)
}

/// One record as the FILTER sees it: the record the function receives, with a `body` that
/// holds a JSON object or array read as that JSON.
///
/// The function always receives `body` as the string the queue holds — a message body "can
/// be any string" — but a filter names the fields INSIDE a structured body ("if your filter
/// criteria expects body to be in a valid JSON format"), so for matching a body that is a
/// JSON object or array is read as that document. A body that is anything else, a bare
/// scalar included, stays the string it is.
fn filter_event(record: &Value) -> Value {
    let mut event = record.clone();
    let parsed = record
        .get("body")
        .and_then(Value::as_str)
        .and_then(|body| ::serde_json::from_str::<Value>(body).ok())
        .filter(|body| body.is_object() || body.is_array());
    if let Some(body) = parsed {
        event["body"] = body;
    }
    event
}

/// The event a batch is handed over as: `Records`, holding the record of each message the
/// filter admitted, in the order the queue handed them over.
fn queue_event(invoking: &[String], polled: &[PolledMessage]) -> Value {
    let records: Vec<Value> = polled
        .iter()
        .filter(|message| invoking.contains(&message.message_id))
        .map(|message| message.record.clone())
        .collect();
    json!({ "Records": records })
}

/// One stored message as the record of the event Lambda hands a function for a queue
/// source: the message's own id, receipt handle, body and MD5 digests, the system attributes
/// the queue records for it, its message attributes, and which source it came from.
fn queue_record(
    message: &sqs::state::StoredMessage,
    receipt_handle: &str,
    queue_arn: &str,
    region: &str,
) -> Value {
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    if !message.sender_id.is_empty() {
        attributes.insert("SenderId".to_string(), message.sender_id.clone());
    }
    attributes.insert(
        "SentTimestamp".to_string(),
        sqs::state::epoch_millis(message.sent_at),
    );
    attributes.insert(
        "ApproximateReceiveCount".to_string(),
        message.receive_count.to_string(),
    );
    if let Some(first) = message.first_received_at {
        attributes.insert(
            "ApproximateFirstReceiveTimestamp".to_string(),
            sqs::state::epoch_millis(first),
        );
    }
    if let Some(group_id) = &message.message_group_id {
        attributes.insert("MessageGroupId".to_string(), group_id.clone());
    }
    if let Some(deduplication_id) = &message.message_deduplication_id {
        attributes.insert(
            "MessageDeduplicationId".to_string(),
            deduplication_id.clone(),
        );
    }
    if let Some(sequence_number) = &message.sequence_number {
        attributes.insert("SequenceNumber".to_string(), sequence_number.clone());
    }
    let mut message_attributes: BTreeMap<String, Value> = BTreeMap::new();
    for (name, attribute) in &message.message_attributes {
        let mut rendered = json!({ "dataType": attribute.data_type.clone() });
        if let Some(text) = attribute.string_value.clone() {
            rendered["stringValue"] = Value::String(text);
        }
        if let Some(bytes) = attribute.binary_value.as_ref() {
            // A binary attribute travels as base64 in a JSON event, the way the record
            // Lambda hands a function reports one.
            rendered["binaryValue"] = Value::String(::base64::Engine::encode(
                &::base64::engine::general_purpose::STANDARD,
                bytes,
            ));
        }
        message_attributes.insert(name.clone(), rendered);
    }
    let mut record = json!({
        "messageId": message.message_id.clone(),
        "receiptHandle": receipt_handle,
        "body": message.body.clone(),
        "attributes": attributes,
        "messageAttributes": message_attributes,
        "md5OfBody": message.md5_of_body.clone(),
        "eventSource": SQS_EVENT_SOURCE,
        "eventSourceARN": queue_arn,
        "awsRegion": region,
    });
    // The digest of the message's ATTRIBUTES, as Amazon SQS computes one: a message
    // carrying attributes has one and a message carrying none has none, so the record
    // reports it for exactly the messages the queue publishes it for.
    if let Some(digest) = sqs::state::md5_of_message_attributes(&message.message_attributes) {
        record[RECORD_MD5_OF_MESSAGE_ATTRIBUTES] = Value::String(digest);
    }
    record
}

/// What ONE batch of a pass came to: the ids of the messages the attempt PROCESSED, or why
/// none of them was.
///
/// A batch whose filter criteria admitted NOTHING is an attempt that succeeded having
/// processed no message of the queue: the pass handled the whole batch by removing what the
/// filter rejected, so there is no invocation to make for it — which is the one case this
/// answers without handing anything to the function.
async fn batch_attempt(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    batch: &AdmittedBatch,
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    hold_seconds: i64,
) -> Result<Vec<String>, String> {
    if batch.invoking.is_empty() {
        return Ok(Vec::new());
    }
    let held = HeldBatch {
        queues,
        messages: &batch.polled,
        hold_seconds,
    };
    // The report reading below answers a non-empty set for every report it could read, so the
    // guard on an empty one is belt and braces: it keeps this pass from recording a successful
    // attempt that consumed nothing, whatever a future reading answers.
    attempt_batch(client, target, &batch.payload, &batch.invoking, &held)
        .await
        .and_then(|processed| {
            if processed.is_empty() {
                return Err(format!(
                    "{} processed no message of the batch it was handed",
                    target.function_arn
                ));
            }
            Ok(processed)
        })
}

/// Hand one batch to the function and read what became of the messages it held: the ids of
/// the ones the attempt PROCESSED, or why none of them was.
///
/// Every message of a batch an invocation accepted was processed unless the mapping asks for
/// its function's report of which of them failed to be read, which is what
/// `FunctionResponseTypes` says (see [`super::batch_item_failures`]): a report that names
/// messages leaves exactly those behind, and one that cannot be read leaves the whole batch.
async fn hand_over_batch(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    payload: &Value,
    invoking: &[String],
) -> Result<Vec<String>, String> {
    let answered = invoke_function(client, target, payload).await?;
    if !target.reports_batch_item_failures {
        return Ok(invoking.to_vec());
    }
    match read_batch_item_failures(answered.as_deref(), invoking) {
        BatchReport::WholeBatchFailed(detail) => Err(format!(
            "{} reported no message of the batch as processed: {detail}",
            target.function_arn
        )),
        report => Ok(processed_records(&report, invoking).unwrap_or_default()),
    }
}

/// Hand one batch over ([`hand_over_batch`]) while KEEPING the hold on the messages `held`
/// covers alive for as long as the attempt takes.
///
/// The attempt is raced against the renewal cadence [`hold_renewal`] states, so every time
/// one hold span is about to run out under a running attempt the batch is hidden for another
/// ([`renew_hold`]) — which is what makes the poller's promise true: no message is handed to
/// a second consumer, this loop's own next pass included, while the first attempt at it is
/// still being decided. Nothing is renewed once the attempt answers: the hold the last
/// renewal left runs out on the queue's own bookkeeping, so a message the attempt did not
/// process comes back for another delivery one hold span later.
async fn attempt_batch(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    payload: &Value,
    invoking: &[String],
    held: &HeldBatch<'_>,
) -> Result<Vec<String>, String> {
    let mut attempt = ::std::pin::pin!(hand_over_batch(client, target, payload, invoking));
    let renew_every = hold_renewal(held.hold_seconds);
    loop {
        // No store is locked across the attempt: the renewal takes the queue store, finishes
        // with it and releases it between two polls of the attempt.
        match ::tokio::time::timeout(renew_every, &mut attempt).await {
            Ok(outcome) => return outcome,
            Err(_) => renew_hold(target, held, SystemTime::now()),
        }
    }
}

/// Hand one batch to the function, synchronously, as a queue event source's invocation is,
/// and answer with the response document it returned.
async fn invoke_function(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    payload: &Value,
) -> Result<Option<Vec<u8>>, String> {
    let answer = client
        .invoke()
        .function_name(&target.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "{} could not be invoked ({})",
                target.function_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed, so nothing of it is
    // deleted.
    if let Some(failure) = answer.function_error.as_deref() {
        return Err(format!(
            "{} answered a {failure} error for the batch",
            target.function_arn
        ));
    }
    // "The response from the function, or an error object" (`InvocationResponse$Payload`):
    // the document a mapping that reads a batch item failure report reads it out of.
    Ok(answer.payload.map(::aws_smithy_types::Blob::into_inner))
}

/// Delete the messages `message_ids` names from `target`'s queue.
fn delete_messages(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    target: &PollTarget,
    message_ids: &[String],
) {
    if message_ids.is_empty() {
        return;
    }
    let mut store = queues.lock();
    let Some(queue) = store.queue_by_arn_mut(&target.queue_arn) else {
        return;
    };
    for message_id in message_ids {
        queue.remove_message(message_id);
    }
}

/// Record what one processing attempt came to on the mapping itself.
///
/// The record is re-read under the lock and only written when it still names the queue and
/// the function this batch was polled for, so a mapping re-targeted while the batch was in
/// flight does not report an attempt made under a configuration it no longer carries.
fn record_attempt(
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    target: &PollTarget,
    result: &str,
) {
    let mut store = mappings.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(&target.uuid) else {
        return;
    };
    if mapping.event_source_arn.as_deref() != Some(target.queue_arn.as_str())
        || mapping.function_arn != target.function_arn
    {
        return;
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// Whether the mapping `uuid` names may publish a 0-valued poll count at `at_secs` — true
/// once per minute, which is the resolution such a series is aggregated at — and claim that
/// minute when it may.
fn claim_idle_minute(session: &mut PollSession, uuid: &str, at_secs: f64) -> bool {
    let minute = minute_of(at_secs);
    if session.idle_minute.get(uuid) == Some(&minute) {
        return false;
    }
    session.idle_minute.insert(uuid.to_string(), minute);
    true
}

/// The minute `at_secs` (epoch seconds) falls in.
fn minute_of(at_secs: f64) -> i64 {
    (at_secs / SECONDS_PER_MINUTE).trunc() as i64
}

/// `at` as epoch seconds, which is how a CloudWatch datapoint carries its instant.
fn epoch_secs(at: SystemTime) -> f64 {
    at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs_f64()
}

/// A count of messages as a metric carries it.
fn count_of(messages: usize) -> u32 {
    u32::try_from(messages).unwrap_or(u32::MAX)
}

/// The queue of every mapping that is polling at this instant.
///
/// A mapping polls when it reports `Enabled` — "when true, the event source mapping is
/// active. When false, Lambda pauses polling and invocation" — its event source is an Amazon
/// SQS queue, and the function it invokes is one this store holds in [`STATE_ACTIVE`]:
/// "Lambda reads items from the event source and invokes the function", so a mapping whose
/// function cannot be invoked yet takes nothing out of the queue.
///
/// Read it only after [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`], so a
/// mapping the clock has already moved into `Enabled` is polled and one it has removed is
/// not.
fn polling_targets(store: &ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<PollTarget> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| {
            mapping.state == EVENT_SOURCE_MAPPING_STATE_ENABLED
                && !mapping.is_deleting()
                && store
                    .functions
                    .get(&mapping.function_name)
                    .is_some_and(|function| function.state == STATE_ACTIVE)
        })
        .filter_map(|mapping| {
            let queue_arn = queue_event_source(mapping.event_source_arn.as_deref())?;
            let region = ::service_common::arn::parse(queue_arn)?.region.to_string();
            let function = store.functions.get(&mapping.function_name)?;
            Some(PollTarget {
                uuid: mapping.uuid.clone(),
                queue_arn: queue_arn.to_string(),
                region,
                function_arn: mapping.function_arn.clone(),
                // Who the poller reads the queue as: the execution role of the VERSION this
                // mapping invokes, since "The ARN of the Lambda function" it carries may be
                // qualified and an alias names the version it invokes.
                role_arn: {
                    let version = mapping
                        .function_qualifier
                        .as_deref()
                        .and_then(|qualifier| function.resolved_version(qualifier))
                        .unwrap_or_else(|| super::VERSION_LATEST.to_string());
                    function.config_of(&version).map(|config| config.role.clone())
                },
                // How long the batch this mapping polls has to stay in flight for: one
                // attempt at it cannot outlast the function's own `Timeout`.
                function_timeout_seconds: i64::from(function.config.timeout),
                // "Amazon Simple Queue Service – Default 10. For standard queues the max is
                // 10,000": the size is the batch's ceiling, and a mapping always pulls at
                // least one record.
                batch_size: usize::try_from(mapping.batch_size)
                    .unwrap_or(1)
                    .clamp(1, MAX_MESSAGES_PER_BATCH),
                in_flight_batches: in_flight_batches(
                    mapping
                        .scaling_config
                        .as_ref()
                        .and_then(|scaling| scaling.maximum_concurrency),
                    function.reserved_concurrent_executions,
                ),
                // "The maximum amount of time, in seconds, that Lambda spends gathering
                // records before invoking the function": a mapping that states none gathers
                // for no time at all, which is the default for a queue source.
                batching_window: Duration::from_secs(
                    mapping
                        .maximum_batching_window_in_seconds
                        .unwrap_or(0)
                        .clamp(0, MAX_BATCHING_WINDOW_SECONDS)
                        .unsigned_abs()
                        .into(),
                ),
                filters: mapping
                    .filter_criteria
                    .as_ref()
                    .and_then(|criteria| criteria.filters.as_ref())
                    .map(|filters| {
                        filters
                            .iter()
                            .filter_map(|filter| filter.pattern.clone())
                            .collect()
                    })
                    .unwrap_or_default(),
                reports_batch_item_failures: reports_batch_item_failures(
                    &mapping.function_response_types,
                ),
                meters_event_counts: super::metrics::publishes_event_counts(
                    mapping.metrics_config.as_ref(),
                ),
            })
        })
        .collect()
}

/// `event_source_arn` when it names an Amazon SQS queue — "Amazon Simple Queue Service –
/// The ARN of the queue" — and `None` for every other event source, which no poller here
/// reads.
///
/// The value is stored request text, so it goes through the ONE shared ARN parser rather
/// than a second ad-hoc reader.
fn queue_event_source(event_source_arn: Option<&str>) -> Option<&str> {
    let arn = event_source_arn?;
    let parsed = ::service_common::arn::parse(arn)?;
    (parsed.vendor == EVENT_SOURCE_VENDOR_QUEUE && super::names_event_source(&parsed))
        .then_some(arn)
}

/// Claim the single poll loop for this store: the claim's GENERATION when the caller may
/// start one, `None` when one already runs.
fn claim_poller(store: &mut ComAmazonawsLambdaAwsgirapiserviceState) -> Option<u64> {
    if store.sqs_poller_running {
        return None;
    }
    store.sqs_poller_running = true;
    store.sqs_poller_generation = store.sqs_poller_generation.wrapping_add(1);
    Some(store.sqs_poller_generation)
}

/// Release the claim `generation` took — and ONLY that one, so a loop standing down cannot
/// clear a claim a loop started since is holding.
fn release_poller(store: &mut ComAmazonawsLambdaAwsgirapiserviceState, generation: u64) {
    if store.sqs_poller_generation == generation {
        store.sqs_poller_running = false;
    }
}

/// Keep the running loop's task handle, aborting the one it replaces.
///
/// Only when `generation` is still the CURRENT claim. A loop can end before its starter gets
/// here (it releases its own claim as soon as no mapping polls), and a call that claimed the
/// loop since then has stored a handle of its own: replacing that one would abort a live
/// loop and leave its claim held for ever. So a stale starter installs nothing and aborts
/// nothing — its own task has already released its claim, so it stands down on its own.
fn set_poller(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
    handle: ::tokio::task::JoinHandle<()>,
) {
    if store.sqs_poller_generation != generation {
        return;
    }
    if let Some(previous) = store.sqs_poller.replace(handle) {
        previous.abort();
    }
}

/// Whether any mapping of this store still wants polling — and, when none does, RELEASE the
/// claim `generation` holds.
///
/// Observing "nothing polls" and standing down happen in ONE critical section, which is what
/// makes the hand-over race-free: a create holds this same lock to store its mapping BEFORE
/// it tries to claim the loop, so either this call sees that mapping and the running loop
/// keeps it, or the create's claim finds the flag cleared and starts a loop of its own.
///
/// A mapping still SETTLING into `Enabled` counts: the state it settles into is its
/// `Enabled` setting, and the clock owes that move, so the loop waits for it rather than
/// standing down between the create and the move it already knows about.
fn polling_wanted_or_release(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    generation: u64,
) -> bool {
    let wanted = store.event_source_mappings.values().any(|mapping| {
        mapping.enabled
            && !mapping.is_deleting()
            && queue_event_source(mapping.event_source_arn.as_deref()).is_some()
    });
    if !wanted {
        release_poller(store, generation);
    }
    wanted
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use ::serde_json::json;
    use ::service_com_amazonaws_sqs_amazonsqs as sqs;
    use ::service_com_amazonaws_sqs_amazonsqs::ops::types::shared_types::MessageAttributeValue;

    use super::{
        filter_event, invisible_for_seconds, minute_of, queue_event, queue_record, PolledMessage,
        RECORD_MD5_OF_MESSAGE_ATTRIBUTES,
    };

    /// The queue every envelope row below is built for, and the Region it lives in.
    const QUEUE_ARN: &str = "arn:aws:sqs:us-east-1:000000000000:orders";
    /// See [`QUEUE_ARN`].
    const REGION: &str = "us-east-1";
    /// The receipt handle the delivery that built the envelope minted.
    const RECEIPT_HANDLE: &str = "AQEBh-handle";

    /// When every row's message was sent, in epoch milliseconds — the units a record's
    /// `SentTimestamp` and `ApproximateFirstReceiveTimestamp` carry.
    const SENT_AT_MILLIS: u64 = 1_761_000_000_123;
    /// When it was first handed to a consumer, which a REDELIVERY of it keeps.
    const FIRST_RECEIVED_AT_MILLIS: u64 = 1_761_000_000_456;

    /// `millis` epoch milliseconds as an instant.
    fn at_millis(millis: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_millis(millis)
    }

    /// One message a standard queue holds, delivered once: the base each envelope row below
    /// changes one thing of.
    fn stored_message() -> sqs::state::StoredMessage {
        sqs::state::StoredMessage {
            message_id: "11111111-2222-4333-8444-555555555555".to_string(),
            body: "hello".to_string(),
            md5_of_body: "5d41402abc4b2a76b9719d911017c592".to_string(),
            message_attributes: BTreeMap::new(),
            message_system_attributes: BTreeMap::new(),
            message_group_id: None,
            message_deduplication_id: None,
            sequence_number: None,
            sent_at: at_millis(SENT_AT_MILLIS),
            retention_from: at_millis(SENT_AT_MILLIS),
            sender_id: "AIDACKCEVSQ6C2EXAMPLE".to_string(),
            available_at: at_millis(SENT_AT_MILLIS),
            invisible_until: None,
            receive_count: 1,
            queue_receive_count: 1,
            first_received_at: Some(at_millis(FIRST_RECEIVED_AT_MILLIS)),
            last_received_at: Some(at_millis(FIRST_RECEIVED_AT_MILLIS)),
            dead_letter_source_arn: None,
        }
    }

    /// One message attribute of `data_type` carrying `string_value`.
    fn string_attribute(data_type: &str, string_value: &str) -> MessageAttributeValue {
        MessageAttributeValue {
            data_type: data_type.to_string(),
            string_value: Some(string_value.to_string()),
            ..Default::default()
        }
    }

    /// One `Binary` message attribute carrying `bytes`.
    fn binary_attribute(bytes: &[u8]) -> MessageAttributeValue {
        MessageAttributeValue {
            data_type: "Binary".to_string(),
            binary_value: Some(bytes.to_vec()),
            ..Default::default()
        }
    }

    #[test]
    fn a_record_carries_the_message_the_queue_holds() {
        // The rows differ along what the message IS: what its body holds, whether it carries
        // message attributes and of which data types, whether its queue is FIFO (and so
        // whether the record carries the group, deduplication id and sequence number the
        // queue assigned), how many times it has been delivered, and whether the queue
        // recorded a first delivery of it at all.
        type Row = (
            &'static str,
            fn(&mut sqs::state::StoredMessage),
            ::serde_json::Value,
        );
        let rows: [Row; 6] = [
            (
                "a plain body, delivered once, no message attributes",
                |_message| {},
                json!({
                    "SenderId": "AIDACKCEVSQ6C2EXAMPLE",
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "1",
                    "ApproximateFirstReceiveTimestamp": "1761000000456",
                }),
            ),
            (
                "a redelivered message keeps the instant of its FIRST delivery",
                |message| {
                    message.receive_count = 4;
                    message.queue_receive_count = 2;
                    message.last_received_at = Some(at_millis(1_761_000_009_999));
                },
                json!({
                    "SenderId": "AIDACKCEVSQ6C2EXAMPLE",
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "4",
                    "ApproximateFirstReceiveTimestamp": "1761000000456",
                }),
            ),
            (
                "a message its queue recorded no first delivery of",
                |message| {
                    message.first_received_at = None;
                    message.last_received_at = None;
                    message.receive_count = 0;
                },
                json!({
                    "SenderId": "AIDACKCEVSQ6C2EXAMPLE",
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "0",
                }),
            ),
            (
                "a message of a FIFO queue",
                |message| {
                    message.message_group_id = Some("group-a".to_string());
                    message.message_deduplication_id = Some("dedup-a".to_string());
                    message.sequence_number = Some("18849496460467696128".to_string());
                },
                json!({
                    "SenderId": "AIDACKCEVSQ6C2EXAMPLE",
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "1",
                    "ApproximateFirstReceiveTimestamp": "1761000000456",
                    "MessageGroupId": "group-a",
                    "MessageDeduplicationId": "dedup-a",
                    "SequenceNumber": "18849496460467696128",
                }),
            ),
            (
                "a message sent by an anonymous caller",
                |message| {
                    message.sender_id = String::new();
                },
                json!({
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "1",
                    "ApproximateFirstReceiveTimestamp": "1761000000456",
                }),
            ),
            (
                "a non-ascii body",
                |message| {
                    message.body = "ünïcödé ⚑ body".to_string();
                    message.md5_of_body = "0a3f0c1e5bdbd4fbb4b1a6c9f7a06b7e".to_string();
                },
                json!({
                    "SenderId": "AIDACKCEVSQ6C2EXAMPLE",
                    "SentTimestamp": "1761000000123",
                    "ApproximateReceiveCount": "1",
                    "ApproximateFirstReceiveTimestamp": "1761000000456",
                }),
            ),
        ];
        for (case, change, attributes) in rows {
            let mut message = stored_message();
            change(&mut message);
            let record = queue_record(&message, RECEIPT_HANDLE, QUEUE_ARN, REGION);
            let expected = json!({
                "messageId": message.message_id.clone(),
                "receiptHandle": RECEIPT_HANDLE,
                "body": message.body.clone(),
                "attributes": attributes,
                "messageAttributes": {},
                "md5OfBody": message.md5_of_body.clone(),
                "eventSource": "aws:sqs",
                "eventSourceARN": QUEUE_ARN,
                "awsRegion": REGION,
            });
            assert_eq!(
                record, expected,
                "case {case:?}: expected the record {expected}, got {record}"
            );
        }
    }

    #[test]
    fn a_record_reports_the_attribute_digest_for_the_messages_that_have_one() {
        // The rows differ along the attributes the digest is computed over: none at all,
        // one, the same one again (the digest is of the attributes, so it is the same), a
        // different value for that name, and several attributes of several data types.
        type Row = (&'static str, Vec<(&'static str, MessageAttributeValue)>);
        let rows: [Row; 5] = [
            ("no message attributes", vec![]),
            ("one String attribute", vec![("kind", string_attribute("String", "order"))]),
            (
                "that same attribute again",
                vec![("kind", string_attribute("String", "order"))],
            ),
            (
                "that attribute carrying another value",
                vec![("kind", string_attribute("String", "refund"))],
            ),
            (
                "three attributes of three types",
                vec![
                    ("kind", string_attribute("String", "order")),
                    ("count", string_attribute("Number", "17")),
                    ("payload", binary_attribute(&[0x00, 0x01])),
                ],
            ),
        ];
        let mut digests: BTreeMap<&str, String> = BTreeMap::new();
        for (case, attributes) in rows {
            let mut message = stored_message();
            let carried = !attributes.is_empty();
            for (name, attribute) in attributes {
                message.message_attributes.insert(name.to_string(), attribute);
            }
            let record = queue_record(&message, RECEIPT_HANDLE, QUEUE_ARN, REGION);
            let reported = record[RECORD_MD5_OF_MESSAGE_ATTRIBUTES].as_str();
            if !carried {
                assert!(
                    reported.is_none(),
                    "case {case:?}: a message carrying no message attributes has no \
                     attribute digest, got {record}"
                );
                continue;
            }
            let digest = reported.unwrap_or_else(|| {
                panic!(
                    "case {case:?}: a message carrying message attributes reports their \
                     digest, got {record}"
                )
            });
            assert!(
                digest.len() == 32 && digest.chars().all(|character| character.is_ascii_hexdigit()),
                "case {case:?}: expected an MD5 hex digest, got {digest:?}"
            );
            digests.insert(case, digest.to_string());
        }
        // The digest is OF the attributes: the same attributes digest alike and different
        // attributes do not.
        assert_eq!(
            digests.get("one String attribute"),
            digests.get("that same attribute again"),
            "the same message attributes must digest alike, got {digests:?}"
        );
        assert_ne!(
            digests.get("one String attribute"),
            digests.get("that attribute carrying another value"),
            "different message attributes must digest differently, got {digests:?}"
        );
        assert_ne!(
            digests.get("one String attribute"),
            digests.get("three attributes of three types"),
            "different message attributes must digest differently, got {digests:?}"
        );
    }

    #[test]
    fn a_record_keeps_each_message_attribute_s_data_type_and_value() {
        // The rows differ along the data type of the attribute and the form its value takes:
        // the three types `SendMessage` documents ("String, Number, and Binary"), a custom
        // type ("You can also append custom labels"), and the bytes of a binary one, which
        // travel as base64 in a JSON event.
        let rows: [(&str, &str, MessageAttributeValue, ::serde_json::Value); 5] = [
            (
                "a String attribute",
                "kind",
                string_attribute("String", "order"),
                json!({ "dataType": "String", "stringValue": "order" }),
            ),
            (
                "a Number attribute",
                "count",
                string_attribute("Number", "17"),
                json!({ "dataType": "Number", "stringValue": "17" }),
            ),
            (
                "a custom data type",
                "shade",
                string_attribute("String.colour", "crimson"),
                json!({ "dataType": "String.colour", "stringValue": "crimson" }),
            ),
            (
                "a Binary attribute",
                "payload",
                binary_attribute(&[0x00, 0x01, 0xfe, 0xff]),
                json!({ "dataType": "Binary", "binaryValue": "AAH+/w==" }),
            ),
            (
                "a Binary attribute carrying no bytes",
                "payload",
                binary_attribute(&[]),
                json!({ "dataType": "Binary", "binaryValue": "" }),
            ),
        ];
        for (case, name, attribute, expected) in rows {
            let mut message = stored_message();
            message
                .message_attributes
                .insert(name.to_string(), attribute);
            let record = queue_record(&message, RECEIPT_HANDLE, QUEUE_ARN, REGION);
            let reported = record["messageAttributes"][name].clone();
            assert_eq!(
                reported, expected,
                "case {case:?}: expected the attribute {name:?} to be reported as {expected}, \
                 got {reported} in {record}"
            );
        }
    }

    /// One polled message carrying `body`, under the id `id`.
    fn polled(id: &str, body: &str) -> PolledMessage {
        PolledMessage {
            message_id: id.to_string(),
            receipt_handle: RECEIPT_HANDLE.to_string(),
            record: json!({ "messageId": id, "body": body }),
        }
    }

    #[test]
    fn a_json_body_is_read_as_json_for_the_filter_and_left_a_string_for_the_function() {
        // The rows differ along what the body IS: an object, an array, a bare scalar that
        // is still valid JSON, and text that is not JSON at all. Only the first two are the
        // "valid JSON format" a filter names fields inside.
        let rows = [
            ("an object", r#"{"kind":"keep"}"#, json!({ "kind": "keep" })),
            ("an array", r#"[1,2]"#, json!([1, 2])),
            ("a bare number", "17", json!("17")),
            ("plain text", "keep", json!("keep")),
        ];
        for (case, body, expected) in rows {
            let message = polled("m-1", body);
            let event = filter_event(&message.record);
            assert_eq!(
                event["body"], expected,
                "case {case:?}: the filter must see {expected:?}, got {:?}",
                event["body"]
            );
            assert_eq!(
                message.record["body"],
                json!(body),
                "case {case:?}: the function must still see the body as the string it is, \
                 got {:?}",
                message.record["body"]
            );
        }
    }

    #[test]
    fn the_event_carries_only_the_records_the_filter_admitted() {
        let polled_messages = vec![polled("m-1", "a"), polled("m-2", "b"), polled("m-3", "c")];
        let rows: [(&str, Vec<String>, Vec<&str>); 3] = [
            ("none admitted", Vec::new(), Vec::new()),
            (
                "the middle one admitted",
                vec!["m-2".to_string()],
                vec!["m-2"],
            ),
            (
                "all admitted",
                vec!["m-1".to_string(), "m-2".to_string(), "m-3".to_string()],
                vec!["m-1", "m-2", "m-3"],
            ),
        ];
        for (case, invoking, expected) in rows {
            let event = queue_event(&invoking, &polled_messages);
            let ids: Vec<String> = event["Records"]
                .as_array()
                .map(|records| {
                    records
                        .iter()
                        .map(|record| record["messageId"].as_str().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            assert_eq!(
                ids, expected,
                "case {case:?}: expected the records {expected:?}, got {ids:?}"
            );
        }
    }

    #[test]
    fn a_minute_is_the_window_an_idle_poll_count_is_published_once_in() {
        let rows = [
            ("the epoch", 0.0, 0),
            ("the first minute", 59.9, 0),
            ("the next minute", 60.0, 1),
            ("a later instant", 1_761_000_000.0, 29_350_000),
        ];
        for (case, at_secs, expected) in rows {
            let minute = minute_of(at_secs);
            assert_eq!(
                minute, expected,
                "case {case:?}: expected minute {expected}, got {minute}"
            );
        }
    }

    #[test]
    fn a_batch_is_held_for_the_longer_of_the_queue_and_the_function_spans() {
        // The rows differ along which of the two spans is the longer one, and are taken at
        // each published bound of both members: `VisibilityTimeout` is "an integer from 0 to
        // 43,200 (12 hours)" and a function's `Timeout` runs from one second to AWS's
        // published Lambda quota of 900. The last column is the window the batch is
        // GATHERED over before any of that, which the hold has to cover too
        // (`MaximumBatchingWindowInSeconds`, `@range(min: 0, max: 300)`).
        let rows = [
            ("the queue hides nothing", 0, 5, 0, 5),
            ("a one second queue, an eight second function", 1, 8, 0, 8),
            ("a queue above the function", 30, 3, 0, 30),
            ("the two spans equal", 10, 10, 0, 10),
            ("the shortest function a queue can outlast", 1, 1, 0, 1),
            ("the longest function", 30, 900, 0, 900),
            ("the queue at its ceiling", 43_200, 900, 0, 43_200),
            ("a batch gathered over ten seconds", 30, 5, 10, 40),
            ("the longest window", 30, 5, 300, 330),
            ("a window on top of the ceiling", 43_200, 900, 300, 43_200),
        ];
        for (case, visibility_timeout, function_timeout, window, expected) in rows {
            let held = invisible_for_seconds(visibility_timeout, function_timeout, window);
            assert_eq!(
                held, expected,
                "case {case:?}: a batch gathered over {window}s and handed to a function \
                 whose timeout is {function_timeout}s, out of a queue hiding a delivery for \
                 {visibility_timeout}s, must stay in flight for {expected}s, got {held}s"
            );
        }
    }
}
