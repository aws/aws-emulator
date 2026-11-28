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
//!   function's — and it is RENEWED while the mapping is still GATHERING the batch, when the
//!   attempt BEGINS and while that attempt is still running ([`renew_hold`]), because the
//!   mapping is the consumer holding those records for every one of those spans and what each
//!   of them costs this emulator is not bounded by the configured spans alone: an invocation
//!   also waits for its share of the run capacity. So no message is
//!   handed to a second consumer while the first attempt at it may still be running (see
//!   [`invisible_for_seconds`]). A mapping carrying a
//!   `MaximumBatchingWindowInSeconds` — "the maximum amount of time, in seconds, that Lambda
//!   spends gathering records before invoking the function" — holds a batch it has not
//!   filled for up to that long, taking up what arrives meanwhile, so its function is
//!   invoked with the records that accumulated over the window rather than with each as it
//!   arrives ([`gathered`]). That window is the ONLY span over which records that became
//!   available at different instants are merged into one batch: one read of a queue answers
//!   with the records that are available when it is made, so it takes the records that were
//!   available TOGETHER ([`records_of_one_read`]) — every record already waiting when the
//!   mapping's reads opened, up to `BatchSize`, and otherwise the arrival that ended the wait —
//!   and the records that arrive while the batch it found is being handed to the function
//!   belong to the next read. A mapping that spends no time gathering therefore invokes its
//!   function with one arrival's records, however much its `BatchSize` would admit, and a
//!   mapping that states a window is the one that merges arrivals. A poll also
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
//! * OPEN. A message of a queue encrypted with a CUSTOMER MANAGED KMS key is sealed under a
//!   data key only KMS can open, and the pass is the consumer asking for it — as the
//!   function's execution role — so a key that does not let that role perform the
//!   `kms:Decrypt` a hand-over performs stops the hand-over of the batch it took
//!   ([`undecryptable_queue`]): nothing is handed to the function and nothing is deleted, so
//!   the messages are the queue's own, hidden for the hold their delivery took and handed
//!   over again when it lapses — the delivery attempt counting against the queue's
//!   `RedrivePolicy` like every other — until the decrypt is granted, which the pass after it
//!   decides afresh.
//! * FILTER. `FilterCriteria` is "an object that defines the filter criteria that determine
//!   whether Lambda should process an event", so a polled message that matches none of the
//!   mapping's patterns is not handed to the function. On a QUEUE source such a message is
//!   also removed from the queue, because nothing else would: leaving it would hand it back
//!   to the same filter at every poll until the queue gave up on it.
//! * INVOKE. The messages the filter admits are handed to the mapping's function as ONE
//!   event, over the in-process callback into the `Invoke` surface of this same emulator
//!   ([`smithy_server_runtime::EmbeddedEmulator`]), so nothing is delivered that this
//!   emulator does not serve itself. A pass hands over AS MANY BATCHES AT ONCE as the
//!   backlog holds, bounded by the most invocations the mapping may have in flight
//!   ([`in_flight_batches`]) — "Limits the number of concurrent instances that the Amazon SQS
//!   event source can invoke" (`ScalingConfig$MaximumConcurrency`) when the mapping configures
//!   one, and otherwise the most concurrent instances a queue source can be asked to invoke at
//!   all — so a mapping draining a backlog scales its batch consumers out with the depth of
//!   that backlog instead of consuming it one batch at a time, and the drain rate a client
//!   sized its concurrency for is the one it gets. Each mapping's batches also go over while
//!   every OTHER mapping's do: "Lambda reads items from the event source and invokes the
//!   function" for each mapping, so a mapping is a consumer of its own — one whose function is
//!   slow, or whose invocations fail, delays no other mapping's records, however many of them
//!   this store holds.
//! * DELETE. A message leaves the queue only once the invocation it was handed to has
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
//! Nothing sleeps inside a request. Each mapping is polled by a background task of ITS OWN,
//! claimed once per mapping ([`drive_sqs_pollers`], armed by the calls that make or change a
//! mapping), which polls, hands the runtime back for [`POLL_INTERVAL`] and polls again, and
//! stands down as soon as its mapping stops wanting to be polled — so a poller's lifetime is
//! bounded by the resource it serves rather than by a timer that would leave a mapping
//! reporting `Enabled` while nothing polls it, and one mapping's cadence is no other
//! mapping's business. One pass is bounded too: at most one batch per invocation the mapping
//! may have in flight, and the store holds at most [`super::MAX_EVENT_SOURCE_MAPPINGS`]
//! mappings, which bounds the tasks there can ever be. With no async runtime to spawn onto
//! there is no task, and [`poll_due`] — which is one pass over every mapping of the store and
//! takes the instant it is working at — can be awaited directly instead. A caller that wants
//! the passes to be ITS OWN, made when it says so rather than on a poller's interval, takes
//! the store's poll claim ([`claim_poll_passes`], which keeps those pollers from starting at
//! all) and then makes each pass with [`poll_passes_once`]: the same work, awaited rather
//! than raced.
//!
//! That cadence is also the GRANULARITY of a read: this emulator looks at a queue on
//! [`POLL_INTERVAL`], so two arrivals closer than that are arrivals no two of its reads could
//! have fallen between, and one read takes both however short the window the mapping gathers
//! over ([`ONE_READ_SPREAD`]). The interval and that floor are this emulator's own spans, like
//! the other spans this module owns, and no number Lambda publishes.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::serde_json::{json, Value};
use ::service_com_amazonaws_sqs_amazonsqs as sqs;

use super::batch_item_failures::{
    processed_records, read_batch_item_failures, reports_batch_item_failures, BatchReport,
};
use super::event_source_authorization::IamState;
use super::event_source_poller::{Clients, PollerClaim};
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

/// How far apart two records may have become available on a queue and still be taken by ONE
/// read of it, whatever batching window the mapping gathers over.
///
/// An EMULATOR-CHOSEN floor, for the same reason as [`POLL_INTERVAL`] — this is the span that
/// interval makes a read of a queue able to tell apart. A read AWS makes returns as soon as
/// the queue has records ("if a message is available, the call returns sooner than
/// `WaitTimeSeconds`"), so it takes the records that were available TOGETHER; this emulator's
/// reads are made on that interval, so two records that became available closer than one
/// interval apart are records one read could not have been made between.
const ONE_READ_SPREAD: Duration = POLL_INTERVAL;

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
///
/// `pub(crate)` because it is this emulator's bound on one batch of MESSAGES, whichever
/// message source a mapping takes them from: the reader of a broker's queue
/// ([`super::mq_poller`]) holds the count a caller chose to the same ceiling.
pub(crate) const MAX_MESSAGES_PER_BATCH: usize = 1_000;

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

/// The fewest batches a mapping may have in flight: one. A pass always hands over the one
/// batch it found, whatever the bounds a mapping's concurrency is read inside
/// ([`batches_in_flight`]) leave it.
const MIN_BATCHES_IN_FLIGHT: usize = 1;

/// The `eventSource` each record of the event a queue mapping hands its function carries.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// The member a record reports the digest of its MESSAGE ATTRIBUTES under, beside
/// `md5OfBody` — the counterpart of `Message$MD5OfMessageAttributes`, which Amazon SQS
/// publishes for a message carrying message attributes and for no other.
const RECORD_MD5_OF_MESSAGE_ATTRIBUTES: &str = "md5OfMessageAttributes";

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
    /// For each mapping, the instant its reads of its source BEGAN — the first pass that read
    /// it, and afresh when a pass could take nothing out of it at all, because the reads a
    /// refusal withheld are reads the mapping did not make.
    ///
    /// What one read of a source finds WAITING is decided against it ([`records_of_one_read`]):
    /// a backlog is what was already there when the reading began, and everything after it is
    /// an arrival the reading was open for.
    reading_since: BTreeMap<String, SystemTime>,
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
            .field("reading_since", &self.reading_since)
            .field("idle_minute", &self.idle_minute)
            .field("failures", &self.failures)
            .finish_non_exhaustive()
    }
}

/// Start the in-process poller of every queue mapping of this engine's store that wants one,
/// for the mappings that have none running yet.
///
/// Called by the operations that make or change a mapping. At most ONE poller runs per
/// MAPPING (the store carries that claim,
/// [`ComAmazonawsLambdaAwsgirapiserviceState::claim_event_source_poller`]), and each of them
/// polls its own mapping alone: a mapping is a consumer of its own event source, so what one
/// mapping's function takes to answer is no part of what another mapping's records wait for.
/// A poller stands down as soon as its mapping stops wanting to be polled, so none of them
/// outlives the mapping it polls, and the store holds at most
/// [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings, so the tasks this can ever spawn are
/// bounded by that count. Nothing keeps a task handle: a poller ends ITSELF at its next
/// wake-up, in the same critical section that gives its claim back ([`PollerClaim`], the
/// claim type the reader of the other kind of event source uses too), which is what bounds
/// its life by the resource it serves rather than by a handle somebody has to remember to
/// abort.
///
/// With no async runtime to spawn onto there is no poller — a caller can then drive the same
/// work through [`poll_due`] — and while a caller holds the store's poll claim
/// ([`claim_poll_passes`]) this starts nothing, because the passes are that caller's to make.
pub fn drive_sqs_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    drive_sqs_pollers_of(ctx, engine, None);
}

/// The same, over the mapping `only` names alone when it names one — the selection the caller
/// that arms the pollers of the mappings it REPORTED hands down
/// ([`super::arm_reported_mapping_pollers`]) — and over every queue mapping of the store when
/// it names none.
pub fn drive_sqs_pollers_of<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
    only: Option<&str>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    // Which mappings want a poller this call has to start: the ones that want polling and
    // whose claim is free, each claimed here so two tasks never poll one mapping.
    let claimed: Vec<PollerClaim> = {
        let mut store = state.lock();
        // A caller that drives the passes itself holds the store's claim: nothing else polls
        // that store while it does.
        if store.sqs_poller_running {
            return;
        }
        let wanted = wanted_queue_mappings(&store);
        wanted
            .into_iter()
            .filter(|uuid| only.is_none_or(|named| named == uuid))
            .filter(|uuid| store.claim_event_source_poller(uuid))
            .map(|uuid| PollerClaim {
                state: ::engine::ServiceState::clone(&state),
                uuid,
                held: true,
            })
            .collect()
    };
    let callback = ctx.callback().clone();
    for claim in claimed {
        let callback = callback.clone();
        let engine = Arc::clone(engine);
        // The CloudWatch store this poller's work is published to, resolved once per poller
        // because the handle does not change.
        let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&*engine));
        runtime.spawn(async move {
            poll_one_mapping(&callback, claim, &metrics, &*engine).await;
        });
    }
}

/// The poll loop of ONE mapping: a pass of its own, [`POLL_INTERVAL`] later another, until it
/// stops wanting to be polled.
///
/// Its bookkeeping ([`PollSession`]) is its own, carried from each of its passes to the next,
/// and so is its cadence: a pass of this mapping waits for nothing another mapping is doing.
async fn poll_one_mapping<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    metrics: &FunctionMetrics,
    engine: &Eng,
) {
    let mut session = PollSession::default();
    // The store this poller's mapping lives in, held beside the claim so the claim itself can
    // be released through a lock taken here.
    let state = ::engine::ServiceState::clone(&claim.state);
    // What the queues tell a waiting consumer: the signal every send to this engine's Amazon
    // SQS store raises ([`sqs::state::message_arrivals`]), resolved once because the handle
    // does not change. An engine holding no queue store has none, and this poller then keeps
    // its own cadence alone.
    let arrivals = sqs::state::message_arrivals(engine);
    loop {
        // Every wake-up waits, which is what hands the runtime back between passes. No lock
        // is held across it.
        wait_for_the_next_pass(arrivals.as_ref()).await;
        // Whether this mapping still wants polling, and the claim's release, happen in ONE
        // critical section, which is what makes the hand-over race-free: either this poller
        // keeps the mapping, or the call that re-arms the pollers finds the claim free and
        // starts one of its own.
        {
            let mut store = state.lock();
            if !mapping_wants_polling(&store, &claim.uuid) {
                claim.release(&mut store);
                return;
            }
        }
        poll_mapping_due(
            callback,
            &mut session,
            engine,
            metrics,
            SystemTime::now(),
            Some(&claim.uuid),
        )
        .await;
    }
}

/// Wait for the next pass of this mapping's poller: until a message ARRIVES on one of this
/// engine's queues, and at most [`POLL_INTERVAL`].
///
/// "Lambda reads items from the event source and invokes the function", and the read AWS makes
/// is a LONG POLL — "if a message is available, the call returns sooner than
/// `WaitTimeSeconds`" (`ReceiveMessageRequest$WaitTimeSeconds`) — so a message a client sends
/// is handed to the mapping's function as soon as it is on the queue rather than whenever the
/// consumer next happens to look. This poller's consumer reads the queue through the store
/// instead of over the wire, so the store TELLS it ([`sqs::state::message_arrivals`]): a send
/// wakes every poller waiting on that store, and the batch the pass that follows takes is the
/// one that was just sent. A poller whose own queue got nothing finds nothing and waits again,
/// and nothing is lost when a send lands mid-pass, because the signal keeps a permit for the
/// one that waits next.
///
/// The interval is the CEILING on the wait and not the cadence of it, because a pass also
/// does the work the CLOCK owes — a visibility timeout lapsing, a redrive becoming due, a
/// batching window running out — which no send signals. An engine with no queue store to
/// signal (nothing to poll either) keeps the interval alone.
async fn wait_for_the_next_pass(arrivals: Option<&sqs::state::MessageArrivals>) -> NextPass {
    match arrivals {
        Some(arrivals) => {
            // The wait is bounded whether or not anything arrives, so the clock-driven work
            // of a pass is never held up by a quiet queue.
            match ::tokio::time::timeout(POLL_INTERVAL, arrivals.notified()).await {
                Ok(()) => NextPass::Arrival,
                Err(_) => NextPass::Interval,
            }
        }
        None => {
            ::tokio::time::sleep(POLL_INTERVAL).await;
            NextPass::Interval
        }
    }
}

/// What ended a wait for the next pass.
#[derive(Debug, PartialEq, Eq)]
enum NextPass {
    /// A message arrived on one of this engine's queues, so the pass that follows takes it.
    Arrival,
    /// Nothing arrived inside [`POLL_INTERVAL`]: the pass that follows is the one the cadence
    /// owes, which is what does the work the clock owes on a quiet queue.
    Interval,
}

/// Take the one poll claim of `engine`'s store, so the passes of its queue mappings are the
/// CALLER's to make: `true` when this call took it, `false` when it is already held.
///
/// [`drive_sqs_pollers`] starts a poller only for a store whose claim is free, so while this
/// claim is held nothing but the caller polls that store: each pass is one
/// [`poll_passes_once`] call, made at the instant the caller chooses rather than on a wake-up
/// interval. That is what lets a host advance a mapping's data plane IN STEP with the calls
/// it makes — the same work, every rule this module states, awaited instead of raced — which
/// is otherwise impossible to do, since nothing a caller can read says whether a pass has
/// happened yet. The claim is kept for as long as the store lives: a caller that has taken
/// over the passes keeps making them, and no poller takes over behind it.
pub fn claim_poll_passes<Eng: Send + Sync + 'static>(engine: &Arc<Eng>) -> bool {
    service_state(&**engine).is_some_and(|state| {
        let mut store = state.lock();
        if store.sqs_poller_running {
            return false;
        }
        store.sqs_poller_running = true;
        true
    })
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
/// For each such mapping: whatever the clock already owed on the queues is applied, the
/// queue's `RedrivePolicy` is applied to the messages that have had every delivery it allows,
/// a batch of at most the mapping's `BatchSize` messages is handed over, the mapping's filter
/// criteria decide which of them the function is invoked with, and what the pass did is
/// metered. See this module's documentation for what each step is, and why a batch no
/// invocation accepted is left on the queue.
///
/// The pass is in three parts, and that is what makes one mapping's deliveries independent of
/// every other's: what each mapping has to hand over is taken out of its queue mapping by
/// mapping (the locks and this session's bookkeeping, nothing awaited), every mapping's
/// batches are then handed to their functions AT ONCE, and what each attempt came to is
/// recorded mapping by mapping once they are all in. So a mapping whose function takes a long
/// time, or whose invocations fail, holds up no other mapping of the store.
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
    poll_mapping_due(callback, session, engine, metrics, now, None).await;
}

/// The whole of [`poll_due`], over ONE mapping when `only` names one and over every polling
/// mapping of the store when it names none.
///
/// A mapping's own poller ([`poll_one_mapping`]) names its mapping, so its pass takes nothing
/// out of another mapping's queue and hands nothing of another mapping over; a caller driving
/// the passes of a whole store ([`poll_passes_once`]) names none, and gets one pass over all
/// of them.
async fn poll_mapping_due<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Eng,
    metrics: &FunctionMetrics,
    now: SystemTime,
    only: Option<&str>,
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
        // A pass of ONE mapping's own poller is about that mapping alone, and costs nothing
        // per mapping beside it.
        polling_targets(&store, only)
    };
    let at_secs = epoch_secs(now);
    // A mapping that no longer polls — disabled, deleted, or its function no longer
    // invocable — is gathering nothing: anything it held stays on its queue and becomes
    // visible again when the hold on it lapses.
    let polling: ::std::collections::BTreeSet<String> =
        targets.iter().map(|target| target.uuid.clone()).collect();
    session.gathering.retain(|uuid, _| polling.contains(uuid));
    // A mapping this pass polls is one whose reads are open from this pass on, unless they were
    // already open: what is waiting on its source when they open is the backlog one read takes
    // ([`records_of_one_read`]), and a mapping that stopped polling begins afresh when it polls
    // again.
    session.reading_since.retain(|uuid, _| polling.contains(uuid));
    for uuid in &polling {
        session.reading_since.entry(uuid.clone()).or_insert(now);
    }
    // And it is bookkeeping of nobody's: a mapping this pass does not poll has no idle minute
    // to claim and no run of failures to count, so what this session remembers about it is let
    // go with it rather than kept for the life of the store.
    session.idle_minute.retain(|uuid, _| polling.contains(uuid));
    session.failures.retain(|uuid, _| polling.contains(uuid));
    // What each mapping has to hand over, taken out of its queue here: this half of the pass
    // takes the stores' locks and this session's bookkeeping, so it is done mapping by mapping
    // and awaits nothing. The hand-over itself comes after, every mapping's at once.
    let mut handovers: Vec<Handover> = Vec::new();
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
            // And it is not READING it either, so its reads begin again when the grant is
            // back: what accumulated meanwhile is a backlog, exactly as it is for a mapping
            // that has just begun polling.
            session.reading_since.insert(target.uuid.clone(), now);
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
            // metered for them yet. The pass that hands them over reports all of it. The
            // mapping IS still holding them, though, so their reservation is renewed on this
            // pass ([`renew_gathering_hold`]) rather than left to run out under the window.
            Due::Gathering => {
                renew_gathering_hold(&queues, session, &target, hold_seconds, now);
                continue;
            }
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
        // The bodies of a batch out of a queue encrypted with a CUSTOMER MANAGED KMS key are
        // sealed under a data key only KMS can open, and this pass is the consumer asking for
        // it — as the function's execution role. So a key that does not let that role decrypt
        // stops the hand-over HERE, with the batch already taken: nothing is handed to the
        // function and nothing is deleted, and the messages the pass took are the queue's own,
        // hidden for the hold their delivery took and handed over again when it lapses — until
        // the decrypt is granted, which the pass after it decides afresh
        // ([`undecryptable_queue`]).
        if let Some(detail) = undecryptable_queue(engine, &target) {
            note_undecryptable(session, &target, &detail);
            // The batch this pass took is let go rather than kept for a later pass, for the
            // reason a withheld read lets one go: its receipt handles lapse with the hold.
            session.gathering.remove(&target.uuid);
            // And the reads begin again when the key can be used, for the same reason as a
            // withheld read above.
            session.reading_since.insert(target.uuid.clone(), now);
            continue;
        }
        // What the filter admits of each batch, decided before any of them is handed over: a
        // message the filter rejected is removed from the queue, because nothing else would
        // and the mapping would otherwise be handed it again at every poll.
        let mut counts = EventCounts::of_a_source_events_leave();
        let admitted: Vec<AdmittedBatch> = batches
            .into_iter()
            .map(|polled| {
                let (invoking, filtered_out) = partition_by_filters(&target, &polled);
                delete_messages(&queues, &target, &filtered_out);
                counts.polled = counts.polled.saturating_add(count_of(polled.len()));
                counts.filtered_out = counts
                    .filtered_out
                    .saturating_add(count_of(filtered_out.len()));
                counts.add_deleted(count_of(filtered_out.len()));
                AdmittedBatch {
                    payload: queue_event(&invoking, &polled),
                    links: upstream_traces(&invoking, &polled),
                    polled,
                    invoking,
                }
            })
            .collect();
        // The client every attempt of this mapping is made through, resolved ONCE: the
        // invocations below are in flight together, so none of them may hold the session the
        // client is built on.
        let client = session.clients.lambda(callback, &target.region).clone();
        handovers.push(Handover {
            target,
            admitted,
            counts,
            client,
            hold_seconds,
        });
    }

    // CONCURRENTLY, across the mappings as well as within one: each mapping's batches are
    // handed to its function while every other mapping's are, so one mapping's slow or
    // failing function delays no other mapping's deliveries — a mapping is polled by its own
    // poller, and what one function takes to answer is no part of what another mapping's
    // records wait for.
    let attempted = ::futures_util::future::join_all(
        handovers
            .iter()
            .map(|handover| handover.hand_over(&queues)),
    )
    .await;

    // What each attempt came to, recorded one at a time: the queue writes and this session's
    // own bookkeeping are the pass's, not an attempt's.
    for (handover, attempts) in handovers.iter_mut().zip(attempted) {
        let (target, counts) = (&handover.target, &mut handover.counts);
        for (batch, attempt) in handover.admitted.iter().zip(attempts) {
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
                    delete_messages(&queues, target, &processed);
                    counts.add_deleted(count_of(processed.len()));
                    session.failures.remove(&target.uuid);
                    record_attempt(&mappings, target, LAST_PROCESSING_RESULT_OK);
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
                        target,
                        LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                    );
                }
            }
        }
        if target.meters_event_counts {
            metrics.publish_event_counts(&target.uuid, counts, at_secs);
            // This pass polled, so the next idle minute is one to publish a zero for.
            session.idle_minute.remove(&target.uuid);
        }
    }
}

/// What ONE mapping has to hand its function in this pass: the batches already taken out of
/// its queue, what the pass has counted for them so far, and what the hand-over needs.
///
/// A pass builds one of these per mapping while it holds the stores' locks, and then hands
/// every one of them over AT ONCE ([`poll_due`]).
struct Handover {
    /// The mapping this is the hand-over of.
    target: PollTarget,
    /// Its batches, with what each one's filter criteria admitted.
    admitted: Vec<AdmittedBatch>,
    /// What the pass has counted for this mapping so far: what it polled, and what the filter
    /// rejected and the pass therefore deleted. The invoked and deleted counts of the attempts
    /// are added once they are in.
    counts: EventCounts,
    /// The client each of its attempts is made through.
    client: ::aws_sdk_lambda::Client,
    /// How long the hold on each of its batches is renewed by while its attempt runs.
    hold_seconds: i64,
}

impl Handover {
    /// Hand every batch of this mapping to its function, each of them while the others are —
    /// as many at once as the mapping may have invocations in flight ([`PollTarget`]'s
    /// `in_flight_batches`) — and answer what each attempt came to, in the batches' own order.
    async fn hand_over(
        &self,
        queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    ) -> Vec<Result<Vec<String>, String>> {
        ::futures_util::future::join_all(self.admitted.iter().map(|batch| {
            batch_attempt(
                &self.client,
                &self.target,
                batch,
                queues,
                self.hold_seconds,
            )
        }))
        .await
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
    /// The chains the admitted messages belong to ([`upstream_traces`]), which the invocation
    /// this batch is handed to links back to.
    links: Vec<String>,
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
    // When this mapping's reads of its source opened, which is what decides what ONE of them
    // finds waiting ([`records_of_one_read`]); this pass's instant for a mapping whose reads
    // open here.
    let reading_since = session
        .reading_since
        .get(&target.uuid)
        .copied()
        .unwrap_or(now);
    let mut batches: Vec<Vec<PolledMessage>> = Vec::new();
    while batches.len() < target.in_flight_batches {
        let polled = take_batch(
            queues,
            target,
            now,
            gathering_room(session, target),
            hold_seconds,
            reading_since,
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

/// Why the batch this pass took out of `target`'s queue cannot be DECRYPTED for the function's
/// execution role — the key the queue names is gone, unusable, or does not let that role
/// perform the `kms:Decrypt` a hand-over of an encrypted message performs — or `None` when it
/// can be, which includes a queue on the Amazon SQS owned key, a mapping whose function this
/// store holds no execution role for, and every case that shows nothing about the key.
///
/// The decision is Amazon SQS's own
/// ([`sqs::state::encryption::authorize_queue_key_as_role`]), the one this emulator makes for
/// every consumer of an encrypted queue, so a pass may open a queue's messages exactly when a
/// client's receive under the same role may: the key's state, the key's own policy, the
/// documents in force on the role and the grants issued to it — and nothing of this service's
/// own.
///
/// A READ: it decides, and takes nothing out of the queue and puts nothing back. The batch
/// the pass already took stays where every undelivered batch stays — in the queue, hidden for
/// the hold its delivery took — which is what a refused hand-over leaves behind.
fn undecryptable_queue<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &PollTarget,
) -> Option<String> {
    let role_arn = target.role_arn.as_deref()?;
    let refusal = sqs::state::encryption::authorize_queue_key_as_role(
        engine,
        &target.region,
        &target.queue_arn,
        role_arn,
        sqs::state::encryption::DECRYPT_ACTION,
    )
    .err()?;
    Some(format!(
        "the messages of {} are encrypted with the queue's KMS key, which cannot be used to \
         decrypt them for {role_arn}: {}",
        target.queue_arn,
        refusal.message()
    ))
}

/// Report that a mapping could not have the key of its queue opened for its function's
/// execution role, at most once every [`FAILURE_LOG_INTERVAL`] passes so one such mapping
/// cannot fill the log.
///
/// Nothing is recorded on the mapping: no batch reached the function, so there is no
/// processing attempt whose result to report — exactly as for a pass whose reads are withheld
/// ([`note_unpollable`]).
fn note_undecryptable(session: &mut PollSession, target: &PollTarget, detail: &str) {
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
            "an event source mapping could not have the key of its queue opened for its \
             function's execution role; the batch it took is handed to nobody and deleted by \
             nobody, and the queue hands it over again when the hold on it lapses"
        );
    }
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

/// The CEILING on how many batches a mapping may have in flight at ONE instant, which is how
/// many invocations of its function one pass may hand over concurrently.
///
/// "Limits the number of concurrent instances that the Amazon SQS event source can invoke"
/// (`ScalingConfig$MaximumConcurrency`) is what a mapping configuring one scales to: that
/// many batches are handed over together, read inside the range the member admits
/// ([`MAX_SOURCE_CONCURRENCY`]) however the value got into the store. A mapping configuring
/// NONE is a mapping that has asked for no limit, not one that consumes a batch at a time: a
/// queue source scales its batch consumers out with the backlog on its own, so what bounds it
/// then is the most concurrent instances such a source can be asked to invoke at all, which is
/// the top of that same range ([`MAX_SOURCE_CONCURRENCY`]).
///
/// This is a CEILING and not a count: how many batches a pass really hands over is how many
/// the backlog holds, which is what [`due_batches`] polls for — one batch per consumer while
/// the queue has messages to hand over, stopping as soon as it has none — so the consumers of
/// a mapping grow with the depth of its queue and a mapping with nothing waiting still polls
/// once.
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
    batches_in_flight(
        // An absent `MaximumConcurrency` is a mapping that limited itself to nothing, so the
        // ceiling is the top of the range that member admits rather than a value to read
        // inside it.
        maximum_concurrency
            .map_or(MAX_SOURCE_CONCURRENCY, |configured| {
                configured.clamp(MIN_SOURCE_CONCURRENCY, MAX_SOURCE_CONCURRENCY)
            }),
        reserved_concurrent_executions,
    )
}

/// How many batches a mapping that asks for `configured` of them really has in flight: that
/// many, less whatever the two bounds above leave it — the function's own reservation and the
/// runs this process can hold at once — and never fewer than the one batch a pass hands over
/// anyway.
///
/// `configured` is already inside the range of the member that asked for it, because WHICH
/// member that is differs per source: a queue mapping asks through
/// `ScalingConfig$MaximumConcurrency` ([`in_flight_batches`]) and a shard mapping through
/// `ParallelizationFactor`
/// ([`super::event_source_poller::in_flight_batches_per_shard`]). Everything past that range
/// is the same question for both, so both sources read it here.
#[must_use]
pub fn batches_in_flight(configured: i32, reserved_concurrent_executions: Option<i32>) -> usize {
    let reserved = reserved_concurrent_executions.unwrap_or(i32::MAX);
    let host = i32::try_from(super::execution::MAX_CONCURRENT_RUNS).unwrap_or(i32::MAX);
    usize::try_from(configured.min(reserved).min(host))
        .unwrap_or(MIN_BATCHES_IN_FLIGHT)
        .max(MIN_BATCHES_IN_FLIGHT)
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

/// How many of the records a queue would hand over ONE read of it takes, given the instant
/// each of those records became AVAILABLE in the order the queue hands them over, the instant
/// the mapping's reads of that queue BEGAN, and the batching window it gathers a batch over.
///
/// A read of a queue returns the records that are available when it is made — "if a message is
/// available, the call returns sooner than `WaitTimeSeconds`"
/// (`ReceiveMessageRequest$WaitTimeSeconds`) — so it takes the records that were available
/// together and not the ones that become available after it has answered. The only span over
/// which records that became available at DIFFERENT instants are merged into one batch is
/// therefore "the maximum amount of time, in seconds, that Lambda spends gathering records
/// before invoking the function" (`MaximumBatchingWindowInSeconds`): a mapping that spends no
/// time gathering hands over the records of one read, so its batch holds only records that
/// were available together — and never records that arrived a window apart, however much its
/// `BatchSize` would admit.
///
/// The read OPENS at `reading_since`, the instant the mapping began reading this source
/// ("Lambda reads items from the event source and invokes the function", which it does for as
/// long as the mapping is enabled), and answers as soon as a record is available. So the two
/// cases a backlog and a trickle are:
///
/// * a record that was ALREADY waiting when the read opened is one the read is answered with
///   at that instant, so every record waiting then goes in one batch, up to `BatchSize` —
///   which is the backlog a mapping made, re-enabled or granted its reads again finds;
/// * a record that becomes available after the read opened ends the wait itself, so the read
///   is answered then and the records that become available later belong to the next read.
///
/// Hence the span is counted from the later of `reading_since` and the first record of the
/// read, and is never shorter than [`ONE_READ_SPREAD`], the span this emulator's reads can
/// tell apart at all.
///
/// `pub` because it is the whole of what ONE read of a queue takes, stated in one place, and
/// the counterpart of [`batch_is_due`], which decides when what was gathered goes.
#[must_use]
pub fn records_of_one_read(
    available_since: &[SystemTime],
    reading_since: SystemTime,
    window: Duration,
) -> usize {
    let spread = window.max(ONE_READ_SPREAD);
    let Some(first) = available_since.first().copied() else {
        return 0;
    };
    let opened = first.max(reading_since);
    available_since
        .iter()
        .take_while(|since| since.duration_since(opened).unwrap_or_default() <= spread)
        .count()
}

/// The batching window a mapping gathers ONE batch over: "the maximum amount of time, in
/// seconds, that Lambda spends gathering records before invoking the function"
/// (`MaximumBatchingWindowInSeconds`).
///
/// Zero for a mapping that states none — "for Kinesis, DynamoDB, and Amazon SQS event
/// sources, the default batching window is 0 seconds" — so such a mapping hands over what
/// one read found. The value is read inside the member's own range
/// ([`MAX_BATCHING_WINDOW_SECONDS`]) however it got into the store.
///
/// `pub` because the window is ONE rule whichever source a mapping polls, so the reader of a
/// shard source gathers over it from here too ([`super::event_source_poller`]).
#[must_use]
pub fn batching_window(configured: Option<i32>) -> Duration {
    Duration::from_secs(
        configured
            .unwrap_or(0)
            .clamp(0, MAX_BATCHING_WINDOW_SECONDS)
            .unsigned_abs()
            .into(),
    )
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
    reading_since: SystemTime,
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
    // ONE read takes the records that were available TOGETHER, inside the span the mapping
    // gathers a batch over ([`records_of_one_read`]): the records that become available while
    // this pass is handing the last batch to the function are the next read's, not this one's,
    // so a mapping that spends no time gathering never merges arrivals a window apart into one
    // invocation.
    let room = one_read_room(queue, now, room, target.batching_window, reading_since);
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

/// How many messages of `queue` ONE read of it takes at `now`: the ones available together
/// inside the span `window` admits, counted from the later of `reading_since` and the first of
/// them ([`records_of_one_read`]), of the at most `room` messages the queue would hand over.
///
/// Each candidate's availability instant is the QUEUE's own reading of it
/// ([`sqs::state::StoredMessage::available_since`]), so this reader compares the instants the
/// service that owns the messages keeps rather than deriving them here.
fn one_read_room(
    queue: &sqs::state::Queue,
    now: SystemTime,
    room: usize,
    window: Duration,
    reading_since: SystemTime,
) -> usize {
    let instants: Vec<SystemTime> = queue
        // The candidates are the messages the queue may hand over at `now`, so each one's
        // availability instant is one that has passed; a message still in flight is not among
        // them.
        .delivery_order(now, room)
        .into_iter()
        .filter_map(|index| queue.messages.get(index))
        .map(sqs::state::StoredMessage::available_since)
        .collect();
    records_of_one_read(&instants, reading_since, window)
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
/// One delivery hides a message for one hold span, counted from that delivery; a poller that
/// is STILL HOLDING the batch when part of that span has been spent is a consumer that is
/// still working on it, so the hold is extended through the queue's own visibility
/// bookkeeping — the path a client's `ChangeMessageVisibility` takes — instead of being let
/// lapse under it. Both spans a pass holds a batch over are renewed this way: the ATTEMPT
/// reserves it afresh when it begins, because the delivery's hold has already paid for the
/// batching window the batch was gathered over, and again while the attempt is still running,
/// because the span an attempt really takes is not bounded by the function's `Timeout` alone
/// — an invocation also waits for its share of this emulator's run capacity, so a hold
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

/// Renew the hold on the messages `target` is still GATHERING into a batch, for one hold span
/// counted from `now`.
///
/// A mapping that has taken records and not handed them over yet is a consumer still holding
/// them: it holds them for the whole of "the maximum amount of time, in seconds, that Lambda
/// spends gathering records before invoking the function"
/// (`MaximumBatchingWindowInSeconds`), and a record that came back part way through that
/// window would be handed to a second consumer while this mapping still had it — a delivery
/// nothing asked for. So the reservation is renewed on every pass that finds the mapping still
/// gathering, exactly as it is renewed while an attempt runs ([`renew_hold`]), rather than
/// resting on the one span the delivery that took them hid them for.
///
/// Nothing to renew for a mapping that is gathering no messages.
fn renew_gathering_hold(
    queues: &::engine::ServiceState<sqs::state::ComAmazonawsSqsAmazonsqsState>,
    session: &PollSession,
    target: &PollTarget,
    hold_seconds: i64,
    now: SystemTime,
) {
    let Some(gathering) = session.gathering.get(&target.uuid) else {
        return;
    };
    if gathering.messages.is_empty() {
        return;
    }
    renew_hold(
        target,
        &HeldBatch {
            queues,
            messages: &gathering.messages,
            hold_seconds,
        },
        now,
    );
}

/// Split `polled` into the messages `target`'s filter criteria admit and the ones they
/// reject, by message id.
///
/// The decision about one record is the shared one
/// ([`super::event_source_filter::Filters`]), including which criteria admit everything;
/// what is this source's own is the record each message is matched AS
/// ([`filter_event`]).
fn partition_by_filters(
    target: &PollTarget,
    polled: &[PolledMessage],
) -> (Vec<String>, Vec<String>) {
    let criteria = super::event_source_filter::Filters::of(&target.uuid, &target.filters);
    let mut budget = super::event_source_filter::budget();
    let mut admitted: Vec<String> = Vec::new();
    let mut rejected: Vec<String> = Vec::new();
    for message in polled {
        let event = filter_event(&message.record);
        if criteria.admits(&event, &mut budget) {
            admitted.push(message.message_id.clone());
        } else {
            rejected.push(message.message_id.clone());
        }
    }
    (admitted, rejected)
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

/// The system attribute a message carries its own X-Ray tracing context in: "currently, the
/// only supported message system attribute is AWSTraceHeader. Its type must be String and its
/// value must be a correctly formatted X-Ray trace header string"
/// (`SendMessageRequest$MessageSystemAttributes`), which the record reports under that name.
const RECORD_TRACE_HEADER_ATTRIBUTE: &str = "AWSTraceHeader";

/// The CHAINS the messages this batch is handing over belong to: the trace of each admitted
/// message's own tracing context, each named once, in the order the queue handed them over.
///
/// The invocation a batch is handed to runs under a chain of its OWN — one batch may carry
/// messages of many chains, so there is no one chain for it to continue — and REFERS back to
/// each of these ([`crate::state::execution::trace_context`]). A message whose send carried no
/// tracing context, or one naming nothing X-Ray reads as a chain, belongs to none and is
/// referred to nowhere.
fn upstream_traces(invoking: &[String], polled: &[PolledMessage]) -> Vec<String> {
    let mut traces: Vec<String> = Vec::new();
    for message in polled
        .iter()
        .filter(|message| invoking.contains(&message.message_id))
    {
        let named = message.record["attributes"][RECORD_TRACE_HEADER_ATTRIBUTE]
            .as_str()
            .and_then(::service_common::xray_trace::trace_root);
        if let Some(trace) = named {
            if !traces.iter().any(|held| held == trace) {
                traces.push(trace.to_string());
            }
        }
    }
    traces
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
    // What the SEND stored in `MessageSystemAttributes` — "currently, the only supported
    // message system attribute is AWSTraceHeader" — reported under the name the queue holds it
    // under, exactly as the queue's own `ReceiveMessage` reports it to any other consumer. It
    // is the record's own copy of the chain the message belongs to, which is the chain the
    // invocation this record is handed to links back to ([`upstream_traces`]).
    for (name, attribute) in &message.message_system_attributes {
        if let Some(string_value) = attribute.string_value.clone() {
            attributes.insert(name.clone(), string_value);
        }
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
    attempt_batch(client, target, batch, &held)
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
    batch: &AdmittedBatch,
) -> Result<Vec<String>, String> {
    let invoking = batch.invoking.as_slice();
    let answered = invoke_function(client, target, batch).await?;
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
/// The attempt RESERVES the batch for a whole hold span counted from the instant it begins,
/// before it is even started ([`renew_hold`]): the reservation it would otherwise inherit is
/// only what is LEFT of the hold the delivery took, and a mapping that gathered its batch
/// over a batching window has already spent that window of it — so a batch gathered over a
/// window would come back mid-attempt, a delivery to a second consumer that nothing asked
/// for. From there the attempt is raced against the renewal cadence [`hold_renewal`] states,
/// so every time one hold span is about to run out under a running attempt the batch is
/// hidden for another — which is what makes the poller's promise true: no message is handed
/// to a second consumer, this loop's own next pass included, while the first attempt at it is
/// still being decided. Nothing is renewed once the attempt answers: the hold the last
/// renewal left runs out on the queue's own bookkeeping, so a message the attempt did not
/// process comes back for another delivery one hold span later.
async fn attempt_batch(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    batch: &AdmittedBatch,
    held: &HeldBatch<'_>,
) -> Result<Vec<String>, String> {
    renew_hold(target, held, SystemTime::now());
    let mut attempt = ::std::pin::pin!(hand_over_batch(client, target, batch));
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
///
/// The call carries the chains of the records it is handing over
/// ([`crate::state::EventSourceTraceLinks`]): the invocation runs under a chain of its own and
/// refers back to each of theirs, which is what links a producer's trace to the work its message
/// was handed to. A batch whose messages belong to no chain carries none.
async fn invoke_function(
    client: &::aws_sdk_lambda::Client,
    target: &PollTarget,
    batch: &AdmittedBatch,
) -> Result<Option<Vec<u8>>, String> {
    let links = crate::state::EventSourceTraceLinks(batch.links.clone());
    let answer = client
        .invoke()
        .function_name(&target.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            batch.payload.to_string().into_bytes(),
        ))
        .customize()
        .mutate_request(move |request| {
            if links.0.is_empty() {
                return;
            }
            request.add_extension(links.clone());
        })
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

/// The queue of every mapping that is polling at this instant, or of the ONE mapping `only`
/// names when it names one.
///
/// A mapping polls when it reports `Enabled` — "when true, the event source mapping is
/// active. When false, Lambda pauses polling and invocation" — its event source is an Amazon
/// SQS queue, and the function it invokes is one this store holds in [`STATE_ACTIVE`]:
/// "Lambda reads items from the event source and invokes the function", so a mapping whose
/// function cannot be invoked yet takes nothing out of the queue.
///
/// `only` is what makes a pass of ONE mapping's own poller cost nothing per mapping beside
/// it: that mapping is selected before any target is built, so the store's other mappings are
/// never read, parsed or cloned.
///
/// Read it only after [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`], so a
/// mapping the clock has already moved into `Enabled` is polled and one it has removed is
/// not.
fn polling_targets(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    only: Option<&str>,
) -> Vec<PollTarget> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| only.is_none_or(|uuid| mapping.uuid == uuid))
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
                batching_window: batching_window(mapping.maximum_batching_window_in_seconds),
                filters: super::event_source_filter::patterns_of(
                    mapping.filter_criteria.as_ref(),
                ),
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

/// Whether the mapping `uuid` names wants a poller of its own at this instant: it is a
/// mapping of this store, on an Amazon SQS queue, that is `Enabled` and not being deleted.
///
/// A mapping still SETTLING into `Enabled` counts: the state it settles into is its `Enabled`
/// setting, and the clock owes that move, so its poller waits for it rather than standing
/// down between the create and the move it already knows about. Whether the mapping's
/// FUNCTION can be invoked yet is a question of each PASS ([`polling_targets`]) and not of
/// the poller's life: a mapping whose function is not `Active` yet polls nothing and keeps
/// its poller, which reads the queue as soon as the function is invocable.
fn mapping_wants_polling(store: &ComAmazonawsLambdaAwsgirapiserviceState, uuid: &str) -> bool {
    store
        .event_source_mappings
        .get(uuid)
        .is_some_and(wants_polling)
}

/// Every mapping of this store that wants a poller of its own ([`mapping_wants_polling`]).
fn wanted_queue_mappings(store: &ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<String> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| wants_polling(mapping))
        .map(|mapping| mapping.uuid.clone())
        .collect()
}

/// The ONE predicate behind both: the mapping polls an Amazon SQS queue, is `Enabled` and is
/// not being deleted.
fn wants_polling(mapping: &super::EventSourceMappingRecord) -> bool {
    mapping.enabled
        && !mapping.is_deleting()
        && queue_event_source(mapping.event_source_arn.as_deref()).is_some()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use ::serde_json::json;
    use ::service_com_amazonaws_sqs_amazonsqs as sqs;
    use ::service_com_amazonaws_sqs_amazonsqs::ops::types::shared_types::{
        MessageAttributeValue, MessageSystemAttributeValue,
    };

    use super::{
        filter_event, invisible_for_seconds, minute_of, queue_event, queue_record,
        upstream_traces, wait_for_the_next_pass, NextPass, PolledMessage,
        RECORD_MD5_OF_MESSAGE_ATTRIBUTES, RECORD_TRACE_HEADER_ATTRIBUTE,
    };

    /// RULE: the wait between two passes ends the instant a message ARRIVES on one of the
    /// engine's queues, and otherwise when the loop's own interval is up — so a message a
    /// client sends is taken by the pass that follows it rather than by whichever pass the
    /// cadence happens to make next, while the work the clock owes on a quiet queue is still
    /// done every interval.
    ///
    /// The cases differ along what the wait has to go on: an arrival already signalled (the
    /// send that landed while the previous pass was running), a store with nothing signalled,
    /// and an engine that holds no queue store to signal at all.
    ///
    /// The clock is PAUSED, so the two cases that run the wait out advance virtual time
    /// instead of sleeping: what each assertion reads is why the wait ended, never how long it
    /// took.
    #[tokio::test(start_paused = true)]
    async fn a_message_arrival_ends_the_wait_before_the_interval_does() {
        let arrivals: sqs::state::MessageArrivals =
            ::std::sync::Arc::new(::tokio::sync::Notify::new());
        arrivals.notify_one();
        assert_eq!(
            wait_for_the_next_pass(Some(&arrivals)).await,
            NextPass::Arrival,
            "case \"an arrival was signalled\": the wait must end on the arrival"
        );
        assert_eq!(
            wait_for_the_next_pass(Some(&arrivals)).await,
            NextPass::Interval,
            "case \"nothing has arrived\": the wait must end on the loop's own interval"
        );
        assert_eq!(
            wait_for_the_next_pass(None).await,
            NextPass::Interval,
            "case \"no queue store to signal\": the wait must end on the loop's own interval"
        );
    }

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
            visible_again_at: None,
            receive_count: 1,
            queue_receive_count: 1,
            first_received_at: Some(at_millis(FIRST_RECEIVED_AT_MILLIS)),
            last_received_at: Some(at_millis(FIRST_RECEIVED_AT_MILLIS)),
            dead_letter_source_arn: None,
            // A message of a queue on the Amazon SQS owned key is sealed under no data key
            // of the account's.
            data_key: None,
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

    /// The `String` message SYSTEM attribute `name` carrying `string_value`, as a send stores
    /// one.
    fn system_attribute(string_value: &str) -> MessageSystemAttributeValue {
        MessageSystemAttributeValue {
            data_type: "String".to_string(),
            string_value: Some(string_value.to_string()),
            ..Default::default()
        }
    }

    /// One polled message whose send carried `trace_header` as its tracing context, under the
    /// id `id` — the record built by [`queue_record`] itself, so what this feeds the links is
    /// the very record the function is handed.
    fn polled_tracing(id: &str, trace_header: Option<&str>) -> PolledMessage {
        let mut message = stored_message();
        message.message_id = id.to_string();
        if let Some(trace_header) = trace_header {
            message.message_system_attributes.insert(
                RECORD_TRACE_HEADER_ATTRIBUTE.to_string(),
                system_attribute(trace_header),
            );
        }
        PolledMessage {
            message_id: id.to_string(),
            receipt_handle: RECEIPT_HANDLE.to_string(),
            record: queue_record(&message, RECEIPT_HANDLE, QUEUE_ARN, REGION),
        }
    }

    /// RULE: a record reports the tracing context its send carried, as the queue holds it \u2014
    /// "AWSTraceHeader \u2013 Returns the X-Ray trace header string" \u2014 and a message whose send
    /// carried none reports none.
    ///
    /// The rows differ along what the send stored: a context naming a chain, one naming a chain
    /// with no decision, one whose `Root` is not a chain X-Ray reads (reported as written: what
    /// a tracer put there is the tracer's business), and no context at all.
    #[test]
    fn a_record_reports_the_tracing_context_its_send_carried() {
        let root = "1-5759e988-bd862e3fe1be46a994272793";
        let rows: Vec<(&str, Option<String>)> = vec![
            (
                "a sampled context",
                Some(format!("Root={root};Parent=ae60e65a7062dd18;Sampled=1")),
            ),
            ("a context with no decision", Some(format!("Root={root}"))),
            (
                "a context naming no chain X-Ray reads",
                Some("Root=not-a-trace-id;Sampled=1".to_string()),
            ),
            ("no context at all", None),
        ];
        for (case, stored) in rows {
            let mut message = stored_message();
            if let Some(stored) = stored.as_deref() {
                message.message_system_attributes.insert(
                    RECORD_TRACE_HEADER_ATTRIBUTE.to_string(),
                    system_attribute(stored),
                );
            }
            let record = queue_record(&message, RECEIPT_HANDLE, QUEUE_ARN, REGION);
            let reported = record["attributes"][RECORD_TRACE_HEADER_ATTRIBUTE]
                .as_str()
                .map(str::to_string);
            assert_eq!(
                reported, stored,
                "case {case:?}: expected the record to report {stored:?} under \
                 {RECORD_TRACE_HEADER_ATTRIBUTE}, got {reported:?} in {record}"
            );
        }
    }

    /// RULE: the batch a pass hands over names the CHAIN of each admitted record once —
    /// which is what the invocation that batch is handed to links back to.
    ///
    /// The rows differ along what the rule generalises over: how many records the batch holds,
    /// how many distinct chains they belong to, whether two of them repeat one chain, whether a
    /// record carries a tracing context at all, whether the one it carries names a chain X-Ray
    /// reads, and whether the record the chain would come from was admitted by the filter.
    #[test]
    fn a_batch_names_the_chain_of_each_record_the_filter_admitted() {
        let first = "1-5759e988-bd862e3fe1be46a994272793";
        let second = "1-00000000-000000000000000000000000";
        // (case, the records of the batch, the ids the filter admitted, the chains the batch
        // names)
        type Row = (
            &'static str,
            Vec<PolledMessage>,
            Vec<String>,
            Vec<&'static str>,
        );
        let rows: Vec<Row> = vec![
            (
                "one record of a sampled chain",
                vec![polled_tracing(
                    "m-1",
                    Some(&format!("Root={first};Parent=ae60e65a7062dd18;Sampled=1")),
                )],
                vec!["m-1".to_string()],
                vec![first],
            ),
            (
                "one record of an unsampled chain",
                vec![polled_tracing("m-1", Some(&format!("Root={first};Sampled=0")))],
                vec!["m-1".to_string()],
                vec![first],
            ),
            (
                "two records of two chains",
                vec![
                    polled_tracing("m-1", Some(&format!("Root={first};Sampled=1"))),
                    polled_tracing("m-2", Some(&format!("Root={second};Sampled=1"))),
                ],
                vec!["m-1".to_string(), "m-2".to_string()],
                vec![first, second],
            ),
            (
                "two records of ONE chain",
                vec![
                    polled_tracing("m-1", Some(&format!("Root={first};Sampled=1"))),
                    polled_tracing("m-2", Some(&format!("Root={first};Sampled=0"))),
                ],
                vec!["m-1".to_string(), "m-2".to_string()],
                vec![first],
            ),
            (
                "a record carrying no tracing context",
                vec![polled_tracing("m-1", None)],
                vec!["m-1".to_string()],
                Vec::new(),
            ),
            (
                "a context naming no chain X-Ray reads",
                vec![polled_tracing("m-1", Some("Root=not-a-trace-id;Sampled=1"))],
                vec!["m-1".to_string()],
                Vec::new(),
            ),
            (
                "a context that is not a tracing header at all",
                vec![polled_tracing("m-1", Some("whatever"))],
                vec!["m-1".to_string()],
                Vec::new(),
            ),
            (
                "the record of the second chain was not admitted",
                vec![
                    polled_tracing("m-1", Some(&format!("Root={first};Sampled=1"))),
                    polled_tracing("m-2", Some(&format!("Root={second};Sampled=1"))),
                ],
                vec!["m-1".to_string()],
                vec![first],
            ),
            (
                "no record admitted at all",
                vec![polled_tracing(
                    "m-1",
                    Some(&format!("Root={first};Sampled=1")),
                )],
                Vec::new(),
                Vec::new(),
            ),
        ];
        for (case, polled_messages, invoking, expected) in rows {
            let named = upstream_traces(&invoking, &polled_messages);
            assert_eq!(
                named, expected,
                "case {case:?}: expected the batch to name the chains {expected:?}, got \
                 {named:?}"
            );
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
