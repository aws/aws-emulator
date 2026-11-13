// SPDX-License-Identifier: CC0-1.0

//! The pipes DATA PLANE: what a pipe does with no client action at all.
//!
//! A pipe's control plane (the record, its ARN, its lifecycle states) lives in
//! [`crate::state`]. This module is the other half: once a pipe's `CurrentState` is
//! `RUNNING`, EventBridge Pipes itself polls the source, hands what it polled to the
//! enrichment, and invokes the target — no caller asks it to. The three stages and the
//! rules they follow:
//!
//! * SOURCE. The source is polled for up to `BatchSize` records, and a poll that has not
//!   filled a batch keeps collecting for up to `MaximumBatchingWindowInSeconds` —
//!   "the maximum number of records to include in each batch" and "the maximum length of
//!   a time to wait for events" (every `PipeSource*Parameters` shape that batches carries
//!   both). Those two bound the POLL; they say nothing about how many calls the target
//!   stage makes. WHICH source is polled is the kind of resource the pipe's `Source`
//!   names: an Amazon SQS queue is received from, and a DynamoDB or Kinesis stream is read
//!   shard by shard from the position `StartingPosition` names, so the changes made to one
//!   partition key reach the target in the order they were applied — the records of a
//!   shard are ordered, and the batches of one shard are moved one after another.
//! * FILTER. A pipe carrying `FilterCriteria` admits only the records that match one of
//!   its patterns — "the collection of event patterns used to filter events" — and it
//!   does so BEFORE the enrichment, so a record the filter drops reaches neither the
//!   enrichment nor the target. The patterns are EventBridge event patterns, read and
//!   matched by the one reading of that language in this tree
//!   (`service_common::event_pattern`). A dropped record is still consumed: it entered
//!   the pipe and the pipe decided about it. A record's message body travels as the text
//!   the producer sent, so the filter reads it in the format that text is in — a body
//!   spelling a JSON document is matched as that document, one spelling none as its text
//!   — and a pattern whose `body` clause is written for the other format admits nothing
//!   ([`FilterView`]). What the later stages are handed is the record envelope itself,
//!   with the body as sent.
//! * ENRICHMENT. The records polled together are handed to the enrichment as ONE JSON
//!   array of record envelopes, and what the enrichment RETURNS — not the source batch —
//!   is what the target stage delivers. So an enrichment that collapses a batch into one
//!   aggregate produces fewer target deliveries than there were source records, one that
//!   returns several events produces more, and one that returns nothing produces none.
//! * TARGET. How many calls the array handed to the target stage comes to is the TARGET's
//!   own, never the pipe's: an Amazon SQS queue's `SendMessage` takes ONE payload per
//!   call, and `PipeTargetSqsQueueParameters` carries only `MessageGroupId` and
//!   `MessageDeduplicationId` — no batch member — so a batch of N events becomes N
//!   messages of one event each, never one message holding an array; a Lambda function's
//!   `Invoke` takes one payload which can be the array itself, so the same batch becomes
//!   ONE invocation carrying every event, which is what makes the source's `BatchSize` the
//!   count of invocations a backlog comes to. A function target is invoked synchronously or
//!   asynchronously as its `InvocationType` says ([`deliver_batch`]); an EventBridge event
//!   bus takes up to ten events per `PutEvents`, each as one entry under the members its
//!   `PipeTargetEventBridgeEventBusParameters` configure ([`event_bus`]). A target
//!   parameter's value is used as configured unless it is a JSON path into the event
//!   (`$.…`), which is resolved against each event delivered, so the value can be the
//!   record's own.
//!
//! Either of those last two stages may be handed something OTHER than the event: an
//! `InputTemplate` is what that stage receives instead, rendered per event from the
//! values its `<$.…>` placeholders select in the event, so a target configured with one
//! receives the payload the template spells out rather than the record envelope
//! ([`render_template`]).
//!
//! A JSON path reads a record the way the filter's patterns do — the message body in the
//! format its producer sent it in ([`read_body`]) — so a path INTO a body spelling a JSON
//! document selects the member it names in that document, whether the path is a
//! placeholder of an `InputTemplate` or the value of a target parameter. Only the readers
//! change: what a stage handed the event itself receives is the envelope with the body as
//! sent.
//!
//! A batch is finished with its source only once every event derived from it has been
//! delivered: a queue's records are deleted then and not before, so a target the pipe
//! could not reach leaves them on the queue to reappear when their visibility timeout
//! lapses, and a stream's read position moves on then and not before, so the same
//! records are read again. Either way delivery is at-least-once, which is what the
//! service promises for a stream source.
//!
//! WHICH records of a batch that is depends on what the target said about them. A target
//! whose call answers a document of its own — a function invoked synchronously — is read as
//! reporting the records of the batch it did NOT process
//! ([`::service_common::batch_item_failures`]), each named by the identifier its source names
//! it by: a queue record by its message id, a stream record by its sequence number. Exactly
//! the records such a report leaves processed are finished with the source, so the ones it
//! names stay there and are handed over again — a queue's when their visibility timeout
//! lapses, a shard's because it is read on only from the first record the target did not
//! process. A report that cannot be read as a report about the records of THAT batch, an
//! identifier the batch does not hold among them, leaves nothing of it processed: the whole
//! batch is handed over again. A target that answers nothing about the records — an Amazon
//! SQS queue, a function invoked asynchronously — has taken the whole batch.
//!
//! A polled queue batch is a CHECKOUT, and the pipe is one consumer of that queue among
//! others: the records a receive handed over are hidden from every other consumer for the
//! queue's own `VisibilityTimeout`, and when that span lapses they are visible again,
//! their delivery count raised, for whoever receives them next. So a pipe that could not
//! move a batch waits that checkout out before it reads the source again, and leaves the
//! records it failed visible for a grace of their own ([`source_free_again`]): a pipe
//! retrying a delivery it keeps failing paces its reads by that rather than by the
//! interval a healthy pipe polls at, so the release of the records it is holding is
//! something the other consumers of its source can see and act on.
//!
//! The poller is started by the operations that make a pipe RUNNING (`CreatePipe`,
//! `StartPipe`, `UpdatePipe`) and ends itself as soon as the pipe's `CurrentState` is no
//! longer one a running pipe rests in — so a `STOPPED` pipe consumes nothing, its
//! backlog stays visible and undeleted, and the next call that makes the pipe RUNNING
//! again resumes consumption of everything that accumulated while it was stopped. That
//! gate is the pipe's own record and it is re-read before EVERY read of the source
//! ([`LiveRecord::runs_now`]), not once per batch: a poll that is still collecting —
//! which it does for as long as `MaximumBatchingWindowInSeconds` allows — takes nothing
//! more from the source the moment the record says the pipe no longer runs, so a record
//! that arrives after the pipe was stopped is left on the source, visible and counted,
//! rather than consumed by a poll that began while it was still running. The records
//! that poll had already taken are finished as usual, because they entered the pipe
//! while it ran and delivery is at-least-once. At most one poller runs per pipe
//! ([`ComAmazonawsPipesPipesState::claim_poller`]). While the pipe is still reaching
//! RUNNING the poller consumes nothing, but it does take up its position in a stream
//! source ([`establish_cursors`]), so a change made to the table between the call that
//! started the pipe and the moment it is running is not one the pipe silently skipped.
//!
//! Every call this module makes goes to the emulator it is running inside, over the
//! in-process callback ([`smithy_server_runtime::EmbeddedEmulator`]): the pipe reaches
//! the sqs, dynamodbstreams, kinesis, lambda and states surfaces of this same emulator and
//! nothing else.
//!
//! Every one of those calls is made AS THE PIPE'S EXECUTION ROLE, so each stage asks
//! first whether the policies in force on that role let it make the call it is about to
//! make ([`super::execution_role`]): a call the role may not make is that stage FAILING —
//! the source records stay where they are and the pipe tries again — and never a call
//! that happens anyway. WHICH role that is is read from the pipe's own record at the
//! moment of each call ([`LiveRecord`]), never captured for the length of a batch, so a
//! role switched while a batch is in flight authorizes every call the pipe has not made
//! yet: nothing is delivered or deleted as an identity the record no longer carries. The
//! rest of the configuration is read the same way, as what the record says NOW: the filter
//! patterns a record is admitted by, the enrichment and target a batch is handed to and the
//! templates they are handed through are the ones the record carries when the batch is
//! DECIDED, not the ones it carried when the poll that collected it began — so a filter an
//! `UpdatePipe` replaced on a running pipe governs the records already polled, and the
//! patterns it replaced govern none.
//!
//! What each pass came to is published as the pipe's OWN CloudWatch metrics
//! ([`super::metrics`]), for every minute the pipe is running: that series is the only
//! thing that tells a pipe whose stage was refused from a pipe that never ran, because
//! both leave the target holding nothing.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant, SystemTime};

use ::serde_json::{json, Value};

use super::{pipe_state, ComAmazonawsPipesPipesState, PipeRecord};
use crate::ops::types::PipeTargetParameters;

mod dead_letter;
mod event_bus;
mod kinesis_source;

/// How long the poller waits before looking at an idle source again. Short enough that
/// a record sent to the source is moved promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long the poller waits after a stage failed before trying the batch again. It is
/// multiplied by the number of consecutive failures, up to [`MAX_RETRY_BACKOFF`].
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The longest the poller waits between two attempts at a batch it keeps failing to
/// move.
///
/// A pipe whose source or target cannot be reached keeps trying — that is what the
/// service does, and it is what lets a pipe recover when the resource comes back — but
/// it does so at this interval rather than at the rate a healthy pipe polls.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// How long the records a failed batch left on an Amazon SQS source are left VISIBLE,
/// after the checkout that hid them has lapsed, before this poller reads that source
/// again.
///
/// A receive is an atomic checkout shared with every other consumer of the queue, so the
/// instant a failed batch's checkout lapses its records are the queue's to hand to
/// whoever receives next — and a poller that re-claimed them inside its own poll interval
/// would make that release unobservable to any other consumer. This span is the
/// EMULATOR's choice of "paced like a service rather than like a loop": long enough that a
/// consumer polling the queue sees the records come back and reads the delivery count they
/// came back with, short enough that a pipe whose target recovers moves them promptly.
const RELEASED_RECORD_GRACE: Duration = Duration::from_secs(5);

/// The longest a pipe leaves its source unread after a batch it could not move.
///
/// The checkout a failed batch left behind is as long as the queue says
/// (`VisibilityTimeout` reaches 12 hours), and the records that arrive while the pipe waits
/// are not the ones it failed. This is the EMULATOR's bound on that wait, so a pipe whose
/// target keeps refusing it still takes up what arrives afterwards instead of standing
/// still for the whole of a long checkout.
const MAX_SOURCE_HOLD_OFF: Duration = Duration::from_secs(60);

/// How often a pipe that keeps failing says so: the first failure, and every this many
/// after it, so one broken pipe cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// "The maximum number of messages to return. ... Valid values: 1 to 10."
/// (`ReceiveMessageRequest$MaxNumberOfMessages`), so a batch larger than this is
/// collected over several receives.
const MAX_RECEIVE_PER_CALL: i32 = 10;

/// The most records this emulator holds in ONE batch, whatever `BatchSize` the pipe
/// configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` itself reaches 10000):
/// a batch is held in memory, and the enrichment payload is built from the whole of it,
/// so the count a caller chooses is clamped before anything is collected. A pipe
/// configured above it polls this many records per batch instead, which is a smaller
/// batch and never a dropped record.
const MAX_RECORDS_PER_BATCH: usize = 1_000;

/// The `BatchSize` a pipe whose source parameters name none polls with.
///
/// The model declares no `@default` on the member (only its `@range`), so this is the
/// default EventBridge Pipes documents for an Amazon SQS source — ten records per batch,
/// the same default the Amazon SQS event-source polling of a Lambda function has. A pipe
/// that wants one record per batch says `BatchSize: 1`.
const DEFAULT_BATCH_SIZE: usize = 10;

/// The `BatchSize` a DynamoDB-stream source polls with when its parameters name none.
///
/// The model declares no `@default` here either, so this is the default EventBridge Pipes
/// documents for a stream source — 100 records per batch, the same default the DynamoDB
/// Streams event-source polling of a Lambda function has.
const DEFAULT_STREAM_BATCH_SIZE: usize = 100;

/// `BatchSize`'s own `@range`, from `PipeSourceSqsQueueParameters` in
/// `smithy-model/com-amazonaws-pipes-pipes.json`.
const BATCH_SIZE_RANGE: std::ops::RangeInclusive<usize> = 1..=10_000;

/// `MaximumBatchingWindowInSeconds`' own `@range`, from the same shape.
const BATCHING_WINDOW_RANGE: std::ops::RangeInclusive<i32> = 0..=300;

/// How much work matching ONE polled batch against the pipe's filters may spend, in
/// steps.
///
/// `FilterCriteria` holds at most five patterns and each is `@length(max: 4096)`, but a
/// batch holds up to [`MAX_RECORDS_PER_BATCH`] records whose own values are the other side
/// of every compare, so the PRODUCT is not bounded by those lengths alone. This is the one
/// cap on the total for a batch, spent down across its records and patterns by the
/// matcher's own budget: the matcher's per-request figure, which is orders of magnitude
/// above what a real pattern set costs.
const MAX_FILTER_STEPS_PER_BATCH: u64 =
    ::service_common::event_pattern::MAX_MATCH_STEPS_PER_REQUEST;

/// The `eventSource` an Amazon SQS record envelope reports.
const SQS_EVENT_SOURCE: &str = "aws:sqs";

/// The ARN service namespaces the stages below address: an SQS queue, a DynamoDB table's
/// stream, a Kinesis data stream, an EventBridge event bus, a Lambda function and a Step
/// Functions state machine (`states`, the namespace Step Functions signs and publishes its
/// endpoint under).
const SQS_NAMESPACE: &str = "sqs";
const DYNAMODB_NAMESPACE: &str = "dynamodb";
const KINESIS_NAMESPACE: &str = "kinesis";
const EVENTS_NAMESPACE: &str = "events";
const LAMBDA_NAMESPACE: &str = "lambda";
const STATES_NAMESPACE: &str = "states";

/// The resource types the enrichment ARNs name within those namespaces.
const FUNCTION_RESOURCE: &str = "function";
const STATE_MACHINE_RESOURCE: &str = "stateMachine";

/// The resource type a DynamoDB stream ARN names, and the segment that makes it a
/// STREAM of that table rather than the table itself:
/// `arn:…:dynamodb:…:table/<name>/stream/<label>`.
const TABLE_RESOURCE: &str = "table";
const STREAM_SEGMENT: &str = "/stream/";

/// The resource type a Kinesis data stream ARN names:
/// `arn:…:kinesis:…:stream/<name>`.
const KINESIS_STREAM_RESOURCE: &str = "stream";

/// The `StartingPosition` values a DynamoDB stream source may name, as the shard
/// iterator type each one reads from. Source: `DynamoDBStreamStartPosition` in
/// `smithy-model/com-amazonaws-pipes-pipes.json` (`TRIM_HORIZON`, `LATEST`), which are
/// also two of DynamoDB Streams' own `ShardIteratorType` values.
const TRIM_HORIZON_POSITION: &str = "TRIM_HORIZON";
const LATEST_POSITION: &str = "LATEST";

/// The most shards of one stream a pass reads, and the most `DescribeStream` pages it
/// walks to list them.
///
/// Both are the EMULATOR's bounds: a stream's shard count is the table's, which this
/// emulator does not cap, so a pass is bounded by work of its own rather than by what the
/// store happens to hold. A pass lists the shards AFTER the one the previous pass served
/// ([`StreamCursors::last_served`]) and wraps round at the end of the stream, so every
/// shard is reached in turn — a stream with more shards than one pass lists is read pass
/// by pass, and a continuously written shard cannot starve the ones behind it.
const MAX_SHARDS_PER_PASS: usize = 100;
const MAX_DESCRIBE_STREAM_PAGES: usize = 10;

/// The IAM action each stage's call is authorized under — the action name of the API call
/// the stage makes, in the namespace of the service it makes it to, which is what the
/// policies in force on the pipe's execution role are evaluated against
/// ([`super::execution_role`]).
const RECEIVE_MESSAGE_ACTION: &str = "sqs:ReceiveMessage";
const DELETE_MESSAGE_ACTION: &str = "sqs:DeleteMessage";
const GET_QUEUE_ATTRIBUTES_ACTION: &str = "sqs:GetQueueAttributes";
const SEND_MESSAGE_ACTION: &str = "sqs:SendMessage";
const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";
const START_SYNC_EXECUTION_ACTION: &str = "states:StartSyncExecution";
const DESCRIBE_STREAM_ACTION: &str = "dynamodb:DescribeStream";
const GET_SHARD_ITERATOR_ACTION: &str = "dynamodb:GetShardIterator";
const GET_RECORDS_ACTION: &str = "dynamodb:GetRecords";

/// `ReceiveMessage`'s selector for every attribute: "All".
const ALL_ATTRIBUTES: &str = "All";

/// What a target parameter's value starts with when it is a JSON path INTO THE EVENT
/// rather than a literal value: EventBridge Pipes resolves such a value against each
/// event it delivers, so one pipe can take a per-record value from the record itself.
///
/// `PipeTargetParameters`' own documentation names this form — "how to use dynamic path
/// parameters" — as part of what a pipe's target parameters are.
const DYNAMIC_PATH_PREFIX: &str = "$.";

/// The `MessageGroupId` target parameter, named as the model names it — what a
/// resolution failure of that parameter is reported by.
const MESSAGE_GROUP_ID: &str = "MessageGroupId";

/// The `MessageDeduplicationId` target parameter, likewise.
const MESSAGE_DEDUPLICATION_ID: &str = "MessageDeduplicationId";

/// The `InvocationType` of a Lambda function target that is invoked ASYNCHRONOUSLY.
/// Source: `PipeTargetInvocationType` in `smithy-model/com-amazonaws-pipes-pipes.json`,
/// whose other value `REQUEST_RESPONSE` is the one the member documents as the default.
const FIRE_AND_FORGET_INVOCATION: &str = "FIRE_AND_FORGET";

/// What an `InputTemplate` OPENS a placeholder with, before the JSON path of the value
/// the placeholder stands for.
const PLACEHOLDER_OPEN: char = '<';

/// What it CLOSES one with.
const PLACEHOLDER_CLOSE: char = '>';

/// The path that selects the WHOLE event, which a template uses to pass the event on
/// unchanged.
const WHOLE_EVENT_PATH: &str = "$";

/// The character a JSON string opens and closes with, as the text a rendering trims it
/// from.
const JSON_STRING_QUOTE: &str = "\"";

/// That same character, for the walk that counts the quotes before a placeholder.
const JSON_STRING_QUOTE_CHARACTER: char = '"';

/// The character that escapes the next one inside a JSON string.
const JSON_STRING_ESCAPE: char = '\\';

/// The most bytes of template one render walks.
///
/// The model states this bound on the member itself (`InputTemplate` is
/// `@length(max: 8192)`) and the request boundary enforces it, so this is that same
/// bound checked where the walk RELIES on it: the template is read from stored state, and
/// a render is refused rather than walked when the text it was handed is longer than a
/// template may be.
const MAX_TEMPLATE_BYTES: usize = 8192;

/// The most one event's render may come to.
///
/// This is the EMULATOR's bound, not an AWS number: a template is bounded (8192 bytes)
/// but it may name the same value of the event many times, so what it renders is not
/// bounded by its own length. 256 KiB is what an Amazon SQS message carries at most
/// ("The minimum size is 1 byte. The maximum is 262,144 bytes (256 KiB)",
/// `SendMessage`), so a render past it is a payload no delivery could take: the render
/// fails and the record stays on the source rather than the batch growing the heap.
const MAX_RENDERED_PAYLOAD: usize = 256 * 1024;

/// Start the in-process poller for the pipe named `pipe_name`, if one is not already
/// running for it.
///
/// Called by an operation that has made the pipe RUNNING, AFTER it has released the
/// store's lock. It changes nothing a caller is told: the poller observes the pipe's own
/// `CurrentState` and does nothing until the clock has moved the pipe into `RUNNING`.
/// With no async runtime to spawn onto there is no poller and nothing is polled.
///
/// At most ONE task runs per pipe, and the store holds at most [`super::MAX_PIPES`]
/// pipes, so the tasks this can ever spawn are bounded by that count. A task is ended by
/// the lifecycle call that stops or deletes its pipe — it reads the pipe's state every
/// pass and returns as soon as the pipe is not RUNNING — so none of them outlives the
/// pipe it polls, and its claim is released however it ends (see [`PollerClaim`]).
pub fn drive_pipe<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    pipe_name: &str,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    if !state.lock().claim_poller(pipe_name) {
        return;
    }
    let callback = ctx.callback().clone();
    // The two OTHER stores of this same engine the stages read: the IAM records the
    // pipe's execution role lives in, and the CloudWatch store the pipe publishes its own
    // metrics to. Resolved once per poller, because neither handle changes.
    let iam = super::execution_role::iam_state(&**engine);
    let metrics = super::metrics::PipeMetrics::new(
        pipe_name,
        super::metrics::cloudwatch_state(&**engine),
    );
    let claim = PollerClaim {
        state: ::engine::ServiceState::clone(&state),
        pipe: pipe_name.to_string(),
        held: true,
    };
    runtime.spawn(async move {
        poll_pipe(&callback, claim, iam, metrics).await;
    });
}

/// The claim of the one poller of a pipe, held for as long as that poller runs.
///
/// It is released by [`Self::release`] inside the critical section that decided the
/// poller is done, so a lifecycle call that arrives afterwards starts a new poller. The
/// `Drop` is the backstop for every OTHER way the task can end — the runtime shutting
/// down, or a panic inside the poller — because a claim left behind would stop this pipe
/// from ever being polled again.
struct PollerClaim {
    state: ::engine::ServiceState<ComAmazonawsPipesPipesState>,
    pipe: String,
    /// Whether this claim is still the live one. Set to `false` by [`Self::release`], so
    /// the `Drop` cannot release a claim a LATER poller has since taken.
    held: bool,
}

impl PollerClaim {
    /// Release the claim through a store lock the caller already holds.
    fn release(&mut self, store: &mut ComAmazonawsPipesPipesState) {
        if self.held {
            store.release_poller(&self.pipe);
            self.held = false;
        }
    }
}

impl Drop for PollerClaim {
    fn drop(&mut self) {
        if self.held {
            let mut store = self.state.lock();
            store.release_poller(&self.pipe);
            self.held = false;
        }
    }
}

/// What the poller may do on one pass, decided under the store's lock.
enum Pass {
    /// The pipe is RUNNING: move one batch under this configuration. Boxed because it is
    /// far larger than the other answers, which are what most passes are.
    Run(Box<RunningPipe>),
    /// The pipe is mid-transition (`CREATING`, `STARTING`, `UPDATING`): nothing is
    /// consumed until it rests in `RUNNING`, but the position a stream source is read
    /// from is taken up now, so a change made while the pipe was settling is not skipped.
    Settling(Box<RunningPipe>),
    /// The pipe is mid-transition and this data plane cannot resolve its configuration
    /// yet: nothing is consumed and nothing is taken up.
    Wait,
    /// The pipe is RUNNING but its configuration is not one this data plane serves, and
    /// why. Nothing is polled, so no record is consumed by a stage that would deliver it
    /// wrongly, and the poller ends — unless the record has since been given a
    /// configuration this data plane does serve, which it then keeps polling under.
    Unserved(String),
    /// The pipe is no longer one this poller runs: it rests `STOPPED`, is stopping or
    /// being deleted, or the record is gone. The claim is already released.
    Stop,
}

/// Poll one pipe until it is no longer RUNNING.
///
/// The store's lock is taken for the decision of each pass and released before any call
/// leaves this process, so a request never waits on a delivery.
async fn poll_pipe(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    iam: Option<::engine::ServiceState<super::execution_role::IamState>>,
    mut metrics: super::metrics::PipeMetrics,
) {
    let pipe_name = claim.pipe.clone();
    // A handle of its own, so the store can be locked while the claim is borrowed.
    let state = ::engine::ServiceState::clone(&claim.state);
    // The questions each stage asks before it calls, and the one its polled records are
    // checked against, are all asked of the record itself. This outlives any one pass,
    // because what a call is made as — and what it is made under — is what the pipe
    // carries when it is made, not what it carried when the batch began.
    let record = LiveRecord {
        iam: iam.as_ref(),
        pipes: &state,
        pipe: &pipe_name,
    };
    let mut clients = Clients::default();
    // Where the stream sources this poller may read have been read up to, for the length
    // of this poller.
    let mut positions = SourcePositions::default();
    // What the batch this pipe's source stands at has cost in failed attempts, which is
    // what the retries its source parameters allow are spent against.
    let mut attempts = BatchAttempts::default();
    let mut failures = 0u32;
    // When this poller may read its source again. A batch it could not move left its
    // records checked out of that source, and they are the source's to hand to its other
    // consumers once that checkout lapses ([`source_free_again`]), so the source is left
    // alone until then.
    let mut hold_source_until: Option<Instant> = None;
    loop {
        let pass = {
            let mut store = state.lock();
            // The state moves the clock has ALREADY made due, so a pipe whose
            // `STARTING` span has lapsed is seen as the `RUNNING` pipe it became.
            store.settle_due_transitions();
            next_pass(&mut store, &mut claim)
        };
        match pass {
            Pass::Stop => return,
            Pass::Wait => ::tokio::time::sleep(POLL_INTERVAL).await,
            Pass::Settling(run) => {
                // Not a poll: the pipe is still reaching RUNNING, so nothing is read and
                // nothing is delivered. All this takes up is the position the source will
                // be read FROM, which is what keeps a change made while the pipe was
                // settling from falling outside a `LATEST` read.
                if let Err(detail) =
                    establish_cursors(callback, &mut clients, &record, &run, &mut positions).await
                {
                    ::tracing::debug!(
                        pipe = %pipe_name,
                        detail = %detail,
                        "a pipe that is still settling could not take up its source \
                         position yet; it tries again"
                    );
                }
                ::tokio::time::sleep(POLL_INTERVAL).await;
            }
            Pass::Unserved(detail) => {
                // The claim is given up only if the pipe's record STILL asks for a
                // configuration no stage here serves, re-read inside the critical section
                // the claim is released through: a call that reconfigured the pipe while
                // this pass was being decided starts no poller of its own — this one still
                // held the claim when it looked — so letting the claim go on the strength
                // of the older reading would leave a RUNNING pipe with no poller at all.
                let unservable = {
                    let mut store = state.lock();
                    store.settle_due_transitions();
                    let unservable = store
                        .pipes
                        .get(&pipe_name)
                        .is_none_or(|pipe| resolve_run(pipe).is_err());
                    if unservable {
                        claim.release(&mut store);
                    }
                    unservable
                };
                if !unservable {
                    ::tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
                ::tracing::warn!(
                    pipe = %pipe_name,
                    detail = %detail,
                    "this emulator's pipes data plane does not serve this pipe's \
                     configuration, so its source is left untouched"
                );
                return;
            }
            Pass::Run(run) => {
                // A RUNNING pipe publishes for every minute it runs, so its counters
                // exist and read 0 for a minute in which nothing of their kind happened.
                metrics.open_window();
                if hold_source_until.is_some_and(|until| Instant::now() < until) {
                    // Waiting out the checkout of a batch this pipe could not move: it is
                    // running — it keeps publishing, and it keeps re-reading its own
                    // record — but it takes nothing from its source, so the records it
                    // failed are the queue's to hand over when they come back.
                    ::tokio::time::sleep(POLL_INTERVAL).await;
                    continue;
                }
                hold_source_until = None;
                let moved = move_one_batch(
                    callback,
                    &mut clients,
                    &mut positions,
                    &record,
                    &run,
                    &mut attempts,
                )
                .await;
                match moved {
                    Batch::Moved { events } => {
                        metrics.count(super::metrics::EVENT_COUNT_METRIC, events);
                        failures = 0;
                    }
                    Batch::Idle => {
                        failures = 0;
                        ::tokio::time::sleep(POLL_INTERVAL).await;
                    }
                    Batch::NotProcessed { events, detail } => {
                        // The events reached the target and it reported it processed none of
                        // them: the execution took them from its source and did not move
                        // them, so it is counted the way a target that did not take the
                        // batch is. The records are on the source already, so nothing is
                        // held off: the pipe reads whatever that source offers while the
                        // ones it did not consume wait their checkout out.
                        metrics.count(super::metrics::EVENT_COUNT_METRIC, events);
                        metrics.count(super::metrics::EXECUTION_FAILED_METRIC, 1);
                        metrics.count(super::metrics::TARGET_STAGE_FAILED_METRIC, 1);
                        failures = failures.saturating_add(1);
                        if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                            ::tracing::warn!(
                                pipe = %pipe_name,
                                attempt = failures,
                                detail = %detail,
                                "a pipe's target reported no record of the batch it was \
                                 handed as processed; the records stay on the source and it \
                                 hands them over again"
                            );
                        }
                        ::tokio::time::sleep(retry_backoff(failures)).await;
                    }
                    Batch::Discarded {
                        stage,
                        events,
                        detail,
                        dead_lettered,
                    } => {
                        // The attempts the pipe's source parameters allow are spent. The
                        // events still entered the pipe and the execution still failed at
                        // the stage that failed it, which is what those counters say — and
                        // the source has moved past the batch, so the pass made progress:
                        // the next one reads what is behind it at the pipe's own rate rather
                        // than backing off, and nothing is held off, because the discarded
                        // records are not read again.
                        metrics.count(super::metrics::EVENT_COUNT_METRIC, events);
                        metrics.count(super::metrics::EXECUTION_FAILED_METRIC, 1);
                        if let Some(metric) = stage.failure_metric() {
                            metrics.count(metric, 1);
                        }
                        failures = 0;
                        hold_source_until = None;
                        ::tracing::warn!(
                            pipe = %pipe_name,
                            stage = %stage.name(),
                            detail = %detail,
                            dead_letter = dead_lettered.as_deref().unwrap_or("none"),
                            "a pipe discarded a batch it could not move after the retries its \
                             source parameters allow; the records are not read again"
                        );
                    }
                    Batch::Superseded => {
                        // Nothing was handed to a stage and nothing was asked of one, so
                        // this is neither work done nor work that failed: the next pass
                        // reads the configuration the pipe now carries.
                        failures = 0;
                        ::tracing::debug!(
                            pipe = %pipe_name,
                            "a pipe's record asked for no configuration this data plane \
                             serves while a batch was in flight; the records stay on the \
                             source for a later pass to move"
                        );
                        ::tokio::time::sleep(POLL_INTERVAL).await;
                    }
                    Batch::Failed {
                        stage,
                        events,
                        detail,
                        checked_out,
                    } => {
                        // The events the failed execution took from its source still
                        // entered the pipe, and the execution itself failed — at the stage
                        // that failed it, which is the counter that tells a refused
                        // delivery from a pipe that never ran.
                        metrics.count(super::metrics::EVENT_COUNT_METRIC, events);
                        metrics.count(super::metrics::EXECUTION_FAILED_METRIC, 1);
                        if let Some(metric) = stage.failure_metric() {
                            metrics.count(metric, 1);
                        }
                        failures = failures.saturating_add(1);
                        if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                            ::tracing::warn!(
                                pipe = %pipe_name,
                                attempt = failures,
                                stage = %stage.name(),
                                detail = %detail,
                                "a pipe could not move a batch from its source to its \
                                 target; the records stay on the source and it tries again"
                            );
                        }
                        // The records it failed are still checked out of the source: it
                        // reads that source again once they have come back and been left
                        // for its other consumers to take.
                        hold_source_until = source_free_again(
                            callback,
                            &mut clients,
                            &record,
                            &run,
                            checked_out,
                        )
                        .await;
                        ::tokio::time::sleep(retry_backoff(failures)).await;
                    }
                }
            }
        }
    }
}

/// How long to wait after `failures` consecutive failed attempts at one batch: it grows
/// with them, capped at [`MAX_RETRY_BACKOFF`].
fn retry_backoff(failures: u32) -> Duration {
    RETRY_BACKOFF
        .saturating_mul(failures.max(1))
        .min(MAX_RETRY_BACKOFF)
}

/// When this poller may read its source again, after a batch it could not move left
/// records checked out of that source.
///
/// A polled Amazon SQS batch is a CHECKOUT: the records a receive handed over are hidden
/// from every other consumer of the queue for the queue's own `VisibilityTimeout` — "the
/// length of time, in seconds, that a message received from a queue will be invisible to
/// other receiving components" (`SetQueueAttributes$Attributes`) — and when that span
/// lapses they are visible again, their delivery count raised, for whoever receives them
/// next. A pipe that could not move them has not finished with them, so it waits that
/// checkout out and then leaves them to the queue for [`RELEASED_RECORD_GRACE`] rather than
/// re-claiming them inside its own poll interval, which no other consumer could observe.
/// The wait is capped at [`MAX_SOURCE_HOLD_OFF`], so a long checkout does not stop the pipe
/// from taking up what arrives while it waits.
///
/// The queue's own timeout is read with `GetQueueAttributes`, one of the calls an Amazon SQS
/// source's own stage makes and the role is asked about ([`LiveRecord::refusal`]). `None` —
/// the plain [`retry_backoff`] is what paces the retry — for a failure that checked nothing
/// out, for a source whose records are no checkout (a stream read is not), and for a queue
/// whose timeout this pipe may not read or that did not answer: nothing is then known about
/// when the records come back.
async fn source_free_again(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    checked_out: Option<Instant>,
) -> Option<Instant> {
    let checked_out = checked_out?;
    let Source::Queue { url } = &run.source else {
        return None;
    };
    if let Some(detail) = record.refusal(GET_QUEUE_ATTRIBUTES_ACTION, &run.source_arn) {
        ::tracing::debug!(
            queue = %url,
            detail = %detail,
            "a pipe could not read how long its source holds the records it failed to \
             move; it retries at its own interval"
        );
        return None;
    }
    let answer = clients
        .sqs(callback, &run.source_region)
        .get_queue_attributes()
        .queue_url(url)
        .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::VisibilityTimeout)
        .send()
        .await
        .map_err(|error| {
            ::tracing::debug!(
                queue = %url,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a pipe could not read its source queue's visibility timeout; it retries \
                 at its own interval"
            );
        })
        .ok()?;
    let seconds: u64 = answer
        .attributes
        .as_ref()?
        .get(&::aws_sdk_sqs::types::QueueAttributeName::VisibilityTimeout)?
        .parse()
        .ok()?;
    let held = Duration::from_secs(seconds).saturating_add(RELEASED_RECORD_GRACE);
    Some(checked_out + held.min(MAX_SOURCE_HOLD_OFF))
}

/// What the poller does next for the pipe it claims, releasing its claim when the answer
/// is [`Pass::Stop`] — the release and the state it was decided from are read in ONE
/// critical section, so a `StartPipe` that has already moved the pipe back towards
/// `RUNNING` either keeps this poller or starts a new one, never neither.
fn next_pass(store: &mut ComAmazonawsPipesPipesState, claim: &mut PollerClaim) -> Pass {
    let pipe_name = claim.pipe.clone();
    let Some(pipe) = store.pipes.get(&pipe_name) else {
        claim.release(store);
        return Pass::Stop;
    };
    if pipe.rests_in_running() {
        return match resolve_run(pipe) {
            Ok(run) => Pass::Run(Box::new(run)),
            // A configuration this data plane cannot serve is reported by the pass
            // itself, so the one log line is written outside the lock.
            Err(detail) => Pass::Unserved(detail),
        };
    }
    // The states a pipe is in while a move it is already making lands. A pipe that is
    // on its way to RUNNING keeps its poller; everything else ends it.
    let towards_running = matches!(
        pipe.current_state.as_str(),
        pipe_state::CREATING | pipe_state::STARTING | pipe_state::UPDATING
    ) && pipe.desired_state == super::requested_state::RUNNING;
    if towards_running {
        return match resolve_run(pipe) {
            Ok(run) => Pass::Settling(Box::new(run)),
            // A configuration that cannot be resolved has no source position to take up
            // either; the pass that finds the pipe RUNNING is the one that reports it.
            Err(_) => Pass::Wait,
        };
    }
    claim.release(store);
    Pass::Stop
}

/// The stage of a pipe an execution reached: which of the three made the call that
/// failed it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// The source poll.
    Source,
    /// The enrichment the polled batch is handed to.
    Enrichment,
    /// The delivery of one event to the target.
    Target,
}

impl Stage {
    /// The metric a failure AT THIS STAGE is counted on, or `None` for a stage that has
    /// no counter of its own — a failed poll is an execution that failed, and nothing
    /// narrower is published about it.
    fn failure_metric(self) -> Option<&'static str> {
        match self {
            Self::Source => None,
            Self::Enrichment => Some(super::metrics::ENRICHMENT_STAGE_FAILED_METRIC),
            Self::Target => Some(super::metrics::TARGET_STAGE_FAILED_METRIC),
        }
    }

    /// This stage's own name, for the one log line a repeatedly failing pipe writes.
    fn name(self) -> &'static str {
        match self {
            Self::Source => "source",
            Self::Enrichment => "enrichment",
            Self::Target => "target",
        }
    }

    /// The stage as the RECORD of a failure names it: the stage of the pipe EventBridge
    /// Pipes reports a failed execution against, so a client reading the record of a
    /// discarded batch is told which of them gave up on it.
    fn reported_stage(self) -> &'static str {
        match self {
            Self::Source => "SOURCE",
            Self::Enrichment => "ENRICHMENT",
            Self::Target => "TARGET",
        }
    }
}

/// What one pass of the poller came to.
enum Batch {
    /// A batch was polled and every event it produced was delivered, carrying how many
    /// events the execution took from its source.
    Moved { events: usize },
    /// The source held nothing.
    Idle,
    /// The pipe's record asks for NO configuration this data plane can act under — the
    /// store no longer holds the pipe, or what it now holds is a configuration no stage
    /// here serves — so nothing was handed to a stage and nothing was counted. The records
    /// stay on the source and reappear when their visibility timeout lapses, for a later
    /// pass to move under the record as it then reads.
    Superseded,
    /// A stage failed; the batch is not finished with its source, so a later pass reads
    /// the same records again (a queue's reappear when their visibility timeout lapses,
    /// and a stream's position has not moved past them). `events` is how many events the
    /// failed execution had taken from its source, which entered the pipe whether or not
    /// the execution finished.
    Failed {
        stage: Stage,
        events: usize,
        detail: String,
        /// When the records this batch failed were checked out of their source, for a
        /// source whose records are a checkout, so the poller knows when they are the
        /// source's to hand over again ([`source_free_again`]). `None` when the failure
        /// checked nothing out.
        checked_out: Option<Instant>,
    },
    /// The batch was delivered and the TARGET reported it processed no record of it — a
    /// `batchItemFailures` naming an identifier the batch does not hold, or every record of
    /// it — so nothing of the batch is finished with its source and a later pass hands the
    /// same records over again.
    ///
    /// Unlike [`Self::Failed`] this leaves the source unheld: the records were not withheld
    /// by a stage this pipe could not reach, they are on the source already, invisible until
    /// their own checkout lapses, and the pipe is free to move whatever that source offers
    /// while it waits.
    NotProcessed { events: usize, detail: String },
    /// The attempts the pipe's source parameters allow at a batch are SPENT: it was
    /// attempted `MaximumRetryAttempts` times beyond the first and failed every time, so it
    /// is discarded — the source moves past it and the records are not read again — and the
    /// record of it went to the dead-letter destination the same parameters configure, named
    /// here by `dead_lettered` when there was one to write to.
    Discarded {
        stage: Stage,
        events: usize,
        detail: String,
        /// The destination the batch's dead letter was written to, or `None` for a pipe
        /// that configures none.
        dead_lettered: Option<String>,
    },
}

/// Move one batch: poll the source, drop the records the pipe's filters do not admit,
/// hand the rest to the enrichment, deliver what came back to the target, and finish the
/// batch with its source.
///
/// `run` is the configuration the POLL is taken under — which source is read, how many
/// records at most and for how long. What the records are then DECIDED by is the
/// configuration the pipe's record asks for once that poll is in ([`LiveRecord::resolved_now`]):
/// the filter stage matches the patterns the record holds then, and the enrichment, the
/// templates and the target are the ones it names then, so a call that replaced any of them
/// while the batch was being collected governs the records it was collecting.
///
/// Every one of those calls is made AS THE PIPE'S EXECUTION ROLE, so each is preceded by
/// the one question [`LiveRecord::refusal`] answers: may the role the pipe carries AT THAT
/// MOMENT make this call on this resource? A call it may not make is the stage FAILING
/// rather than the call happening.
/// A batch a pass could not get through the pipe's stages, and what the poller may do about
/// it: attempt it again, or — once the retries the pipe allows are spent — discard it.
///
/// Both halves of that decision are counted against the batch itself ([`BatchAttempts`]),
/// never against the pipe: a pipe that keeps failing one poison batch while moving everything
/// behind it has failed that batch as many times as it has attempted it.
struct Attempt<'a> {
    /// The configuration the batch was decided under, which is what says how many attempts
    /// it gets and where its dead letter goes.
    decided: &'a RunningPipe,
    /// The batch itself, for the pointer its dead letter carries and the finish that moves
    /// its source past it.
    polled: &'a Polled,
    /// What the batch the source stands at has cost in failed attempts so far.
    attempts: &'a mut BatchAttempts,
    /// When the records were checked out of their source, for a source they are a checkout
    /// of ([`Polled::checked_out`]).
    checked_out: Option<Instant>,
}

/// How many consecutive attempts the batch a source stands at has FAILED, for the length of
/// one poller.
///
/// A source whose position has not moved hands the same batch over again, so the attempts at
/// it are counted under the position it begins at ([`Polled::attempt_key`]) rather than
/// under the pipe: a pipe that moves a batch and then fails the next one starts that one's
/// count at its own first failure. It holds ONE key, because a pipe moves one batch at a
/// time.
#[derive(Debug, Default)]
struct BatchAttempts {
    /// The batch the count below belongs to, or `None` when nothing is counted.
    batch: Option<String>,
    /// How many attempts at that batch have failed, counting the first.
    failed: u32,
}

impl BatchAttempts {
    /// Count a failed attempt at the batch `key` names, and answer how many of its attempts
    /// have now failed — this one included.
    fn failed(&mut self, key: &str) -> u32 {
        if self.batch.as_deref() != Some(key) {
            self.batch = Some(key.to_string());
            self.failed = 0;
        }
        self.failed = self.failed.saturating_add(1);
        self.failed
    }

    /// Forget what was counted: the batch is finished with, either moved or discarded, so
    /// the next one is counted from its own first failure.
    fn settled(&mut self) {
        self.batch = None;
        self.failed = 0;
    }
}

/// One FAILED attempt at a polled batch: the stage it failed at, why, and what that stage's
/// own resource said about it.
struct FailedAttempt {
    /// Which of the pipe's stages failed the batch.
    stage: Stage,
    /// Why, as the one log line a repeatedly failing pipe writes reports it.
    detail: String,
    /// The error the invoked resource NAMED, for a call that answered one of its own — a
    /// function's `FunctionError`. This is the invoked resource's own word for what went
    /// wrong, so it is what a dead letter reports; `None` for a failure nothing named.
    function_error: Option<String>,
    /// Whether the records were WITHHELD from the source by a stage the pipe could not get
    /// through, rather than left on it by a target that answered and reported it processed
    /// no record of the batch.
    withheld: bool,
}

impl FailedAttempt {
    /// A stage the pipe could not get through: the records never reached the target, so they
    /// are still checked out of their source.
    fn withheld(stage: Stage, detail: String) -> Self {
        Self {
            stage,
            detail,
            function_error: None,
            withheld: true,
        }
    }

    /// A stage whose own call failed, carrying the error the invoked resource named when it
    /// named one.
    fn from_call(stage: Stage, failure: CallFailure) -> Self {
        Self {
            stage,
            detail: failure.detail,
            function_error: failure.function_error,
            withheld: true,
        }
    }

    /// A batch the target ANSWERED about, reporting it processed no record of it: the records
    /// are on the source already.
    fn unprocessed(stage: Stage, detail: String) -> Self {
        Self {
            stage,
            detail,
            function_error: None,
            withheld: false,
        }
    }

    /// This attempt as the pass's outcome, for a batch whose attempts are not spent yet: the
    /// records stay on the source and a later pass hands them over again.
    fn into_batch(self, events: usize, checked_out: Option<Instant>) -> Batch {
        if self.withheld {
            Batch::Failed {
                stage: self.stage,
                events,
                detail: self.detail,
                checked_out,
            }
        } else {
            Batch::NotProcessed {
                events,
                detail: self.detail,
            }
        }
    }
}

/// Move one batch: poll the source, drop the records the pipe's filters do not admit,
/// hand the rest to the enrichment, deliver what came back to the target, and finish the
/// batch with its source.
///
/// `run` is the configuration the POLL is taken under — which source is read, how many
/// records at most and for how long. What the records are then DECIDED by is the
/// configuration the pipe's record asks for once that poll is in ([`LiveRecord::resolved_now`]):
/// the filter stage matches the patterns the record holds then, and the enrichment, the
/// templates and the target are the ones it names then, so a call that replaced any of them
/// while the batch was being collected governs the records it was collecting.
///
/// A batch no attempt gets through is attempted again — and only so many times: the retries
/// the pipe's own source parameters allow are counted against it here, and a batch that has
/// spent them is DISCARDED rather than attempted for ever ([`discarded_or_failed`]).
async fn move_one_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    positions: &mut SourcePositions,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    attempts: &mut BatchAttempts,
) -> Batch {
    let polled = match collect_batch(callback, clients, record, run, positions).await {
        Ok(polled) => polled,
        Err(detail) => {
            return Batch::Failed {
                stage: Stage::Source,
                events: 0,
                detail,
                checked_out: None,
            }
        }
    };
    if polled.is_empty() {
        return Batch::Idle;
    }
    // When the source handed these records over, for a source they are checked OUT of:
    // every failure below leaves them there, so this is when they come back.
    let checked_out = polled.checked_out();
    // The configuration the stages below act under is the one the pipe's record asks for
    // NOW, re-read once the poll is in: a call that reconfigured the pipe while this batch
    // was being collected decides every stage the batch has not reached yet, so a filter
    // pattern, an enrichment, a template or a target that has just been replaced governs the
    // records already polled — the records are decided by what the pipe carries rather than
    // held back until their checkout of the source lapses. The one configuration a batch
    // cannot be decided under is none at all: a pipe the store no longer holds, or whose
    // record asks for something this data plane does not serve, leaves them on the source.
    let Some(decided) = record.resolved_now() else {
        return Batch::Superseded;
    };
    let decided = &decided;
    match run_stages(callback, clients, positions, record, decided, &polled, checked_out).await {
        Ok(moved) => {
            if matches!(moved, Batch::Moved { .. }) {
                // The batch is finished with its source: whatever earlier attempts at it
                // cost is spent, and the next batch is counted from its own first failure.
                attempts.settled();
            }
            moved
        }
        Err(failure) => {
            let attempt = Attempt {
                decided,
                polled: &polled,
                attempts,
                checked_out,
            };
            discarded_or_failed(callback, clients, positions, record, attempt, failure).await
        }
    }
}

/// What the poller does with a batch no attempt could move: attempt it again, or discard it
/// because the retries the pipe allows are spent.
///
/// "Discard records after the specified number of retries. The default value is -1, which
/// sets the maximum number of retries to infinite" (`MaximumRetryAttempts` of either stream
/// source's parameters): a pipe that names no limit — and a source whose parameters carry no
/// such member at all — attempts the batch for as long as its source holds it, which is what
/// every pass before this one did. A pipe that names one gets that many attempts BEYOND the
/// first, and the attempt after them discards the batch: its source moves past it, so the
/// records are not read again, and the dead-letter destination the same parameters configure
/// is sent the one record a client has of them ([`dead_letter`]).
///
/// A dead letter that could not be written is a batch that is NOT discarded: the records stay
/// where they are and the pipe attempts them again, because a pipe that dropped them with
/// nothing holding the record of them would be exactly the silent loss the destination exists
/// to prevent.
async fn discarded_or_failed(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    positions: &mut SourcePositions,
    record: &LiveRecord<'_>,
    attempt: Attempt<'_>,
    failure: FailedAttempt,
) -> Batch {
    let Attempt {
        decided,
        polled,
        attempts,
        checked_out,
    } = attempt;
    let taken = polled.len();
    let failed = attempts.failed(&polled.attempt_key());
    let Some(allowed) = decided.maximum_retry_attempts else {
        return failure.into_batch(taken, checked_out);
    };
    // The first attempt is not a retry: a pipe allowing one retry attempts the batch twice.
    if failed <= allowed {
        return failure.into_batch(taken, checked_out);
    }
    let dead_lettered = match (&decided.dead_letter, polled.discarded_batch()) {
        (Some(destination), Some(batch)) => {
            let body = dead_letter::payload(decided, &batch, &failure, failed);
            match dead_letter::send(callback, clients, record, destination, &body).await {
                Ok(()) => Some(destination.arn().to_string()),
                Err(detail) => {
                    ::tracing::warn!(
                        pipe = %record.pipe,
                        destination = %destination.arn(),
                        detail = %detail,
                        "a pipe could not record the batch it gave up on at its dead-letter \
                         destination; it keeps the batch and attempts it again"
                    );
                    return failure.into_batch(taken, checked_out);
                }
            }
        }
        // A pipe that configures no destination still discards the batch — the retries it
        // allowed are spent either way — and so does one whose source is not a kind a dead
        // letter can point at.
        _ => None,
    };
    // Discarded: the source moves past the whole batch, so the records it held are not read
    // again. Nothing here can leave a stream's position where it stood, because the finish
    // is for the whole batch.
    if !matches!(
        finish_batch(
            callback,
            clients,
            positions,
            record,
            decided,
            polled,
            &Finished::WholeBatch,
        )
        .await,
        SourceAdvance::MovedOn
    ) {
        ::tracing::warn!(
            pipe = %record.pipe,
            "a pipe could not move its source past the batch it discarded; it reads it again"
        );
    }
    attempts.settled();
    Batch::Discarded {
        stage: failure.stage,
        events: taken,
        detail: failure.detail,
        dead_lettered,
    }
}

/// Run the pipe's stages over a batch that has been polled: the filter, the enrichment, the
/// target, and the finish that moves the source past what the target processed.
///
/// Every one of those calls is made AS THE PIPE'S EXECUTION ROLE, so each is preceded by
/// the one question [`LiveRecord::refusal`] answers: may the role the pipe carries AT THAT
/// MOMENT make this call on this resource? A call it may not make is the stage FAILING
/// rather than the call happening — and a stage that fails answers the FAILED ATTEMPT it
/// was, which is what the caller counts against the batch's retries.
async fn run_stages(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    positions: &mut SourcePositions,
    record: &LiveRecord<'_>,
    decided: &RunningPipe,
    polled: &Polled,
    checked_out: Option<Instant>,
) -> Result<Batch, FailedAttempt> {
    let taken = polled.len();
    // The identifier its source names each polled record by: what a target's report of the
    // records it did not process names them by, and what the finish below leaves on the
    // source or moves past.
    let identifiers = polled.identifiers();

    // The WHOLE polled batch, as one JSON array of record envelopes.
    let batch = match polled.envelopes(decided) {
        Ok(batch) => batch,
        Err(detail) => return Err(FailedAttempt::withheld(Stage::Source, detail)),
    };

    // The filter decides which of the polled records enter the pipe, BEFORE the
    // enrichment: one the pipe's patterns do not admit reaches neither stage. It is still
    // consumed, so the source moves on from it rather than offering it again.
    let batch = admitted(&decided.filters, batch);
    if batch.is_empty() {
        let advance = finish_batch(
            callback,
            clients,
            positions,
            record,
            decided,
            polled,
            &Finished::WholeBatch,
        )
        .await;
        return moved_or_standing_still(advance, decided, taken, checked_out);
    }

    // What the TARGET stage delivers: the enrichment's answer when the pipe has one,
    // the polled batch itself when it has none. An enrichment is handed the records
    // rendered through ITS input template, when the pipe carries one, because that
    // template is what it receives instead of the record.
    let events = match &decided.enrichment {
        None => batch,
        Some(enrichment) => {
            let handed = match transformed(decided.enrichment_template.as_deref(), batch) {
                Ok(handed) => handed,
                Err(detail) => return Err(FailedAttempt::withheld(Stage::Enrichment, detail)),
            };
            if let Some(detail) =
                record.refusal(enrichment.invocation_action(), enrichment.arn())
            {
                return Err(FailedAttempt::withheld(Stage::Enrichment, detail));
            }
            match invoke_enrichment(callback, clients, enrichment, &handed).await {
                Ok(events) => events,
                Err(failure) => return Err(FailedAttempt::from_call(Stage::Enrichment, failure)),
            }
        }
    };

    // The target stage makes the same call on the same resource for every event of the
    // batch, so the role is asked about it once for the batch — and only when there is an
    // event to deliver, because a batch the enrichment emptied makes no target call at all.
    if !events.is_empty() {
        if let Some(detail) = record.refusal(decided.target.delivery_action(), &decided.target_arn)
        {
            return Err(FailedAttempt::withheld(Stage::Target, detail));
        }
    }

    // What the target RECEIVES per event: the event rendered through the TARGET's input
    // template, when the pipe carries one; the target parameters are read from the event
    // itself either way. Both of those read the event through its PATHS, so both see the
    // message body in the format its producer sent it in — while the event a target with
    // no template is handed is the envelope itself, body as sent.
    let payloads = match target_payloads(decided, &events) {
        Ok(payloads) => payloads,
        Err(detail) => return Err(FailedAttempt::withheld(Stage::Target, detail)),
    };

    // How the target TAKES them is the target's own: one call per event for a target whose
    // API carries one payload per call, one call holding every event for a target whose API
    // takes the batch ([`deliver_batch`]). What it ANSWERS is which records of the batch it
    // did not process, for a target whose call answers the caller at all.
    let report =
        match deliver_batch(callback, clients, &decided.target, &payloads, &identifiers).await {
            Ok(report) => report,
            Err(failure) => return Err(FailedAttempt::from_call(Stage::Target, failure)),
        };
    // A report that cannot be read as a report of the records of THIS batch — an identifier
    // the batch does not hold among them, or every record of it — leaves nothing of it
    // processed, so the records stay on the source in full and a later pass hands them over
    // again.
    if let ::service_common::batch_item_failures::BatchReport::WholeBatchFailed(detail) = &report {
        return Err(FailedAttempt::unprocessed(
            Stage::Target,
            format!(
                "the target {} reported no record of the batch as processed: {detail}",
                decided.target_arn
            ),
        ));
    }
    let processed =
        ::service_common::batch_item_failures::processed_records(&report, &identifiers)
            .unwrap_or_default();

    // Delivered: the records the target processed may be finished with their source, and the
    // ones it reported it did not stay there to be handed over again.
    let finished = if processed.len() == identifiers.len() {
        Finished::WholeBatch
    } else {
        // A SET, so finishing a batch costs its own length and never the product of the
        // batch's length and the report's.
        Finished::Records(processed.iter().map(String::as_str).collect())
    };
    let advance =
        finish_batch(callback, clients, positions, record, decided, polled, &finished).await;
    moved_or_standing_still(advance, decided, taken, checked_out)
}

/// The pass's outcome for a batch whose events were all delivered, from what finishing it
/// left its source at.
///
/// A finish that moved the source on is the moved batch it looks like. One that left a
/// shard's position where it stood is NOT: the same records are the next thing that shard
/// hands over, so the pass made no progress and is reported as the kind of failure it was —
/// which is what makes the poller pace its next pass instead of re-reading, re-enriching and
/// re-delivering the same records at full rate.
fn moved_or_standing_still(
    advance: SourceAdvance,
    run: &RunningPipe,
    taken: usize,
    checked_out: Option<Instant>,
) -> Result<Batch, FailedAttempt> {
    match advance {
        SourceAdvance::MovedOn => Ok(Batch::Moved { events: taken }),
        SourceAdvance::HeadUnprocessed => Err(FailedAttempt::unprocessed(
            Stage::Target,
            format!(
                "the target {} processed no record at the head of the batch, so the position \
                 of its source stands where it stood",
                run.target_arn
            ),
        )),
        SourceAdvance::PositionUnavailable(detail) => Ok(Batch::Failed {
            stage: Stage::Source,
            events: taken,
            detail,
            checked_out,
        }),
    }
}

/// What ONE event contributes to the target stage: the payload the target receives for it,
/// beside that event as a path reads it ([`path_view`]), which the target parameters are
/// resolved against.
type Delivery<'a> = (Cow<'a, Value>, Cow<'a, Value>);

/// What each event of `events` contributes to the target stage: the payload the target
/// receives beside the event the target parameters are resolved against.
///
/// A render that fails is a FAILED delivery for the whole batch — no event of it is
/// handed over — so the records stay on the source rather than part of a batch being
/// delivered.
fn target_payloads<'a>(
    run: &RunningPipe,
    events: &'a [Value],
) -> Result<Vec<Delivery<'a>>, String> {
    let mut payloads = Vec::with_capacity(events.len());
    for event in events {
        let read = path_view(event);
        let payload = match run.target_template.as_deref() {
            None => Cow::Borrowed(event),
            Some(template) => Cow::Owned(render_template(template, read.as_ref())?),
        };
        payloads.push((payload, read));
    }
    Ok(payloads)
}

/// Which records of a polled batch the pipe is FINISHED with: the ones its source may move
/// on from.
///
/// Everything else the poll took stays where it is, so it is handed over again — a queue's
/// records when their visibility timeout lapses, a stream's because the shard is not read on
/// from past them.
enum Finished<'a> {
    /// Every record the poll took.
    WholeBatch,
    /// Only the records named here, by the identifier their source names a record by
    /// ([`Polled::identifiers`]).
    Records(BTreeSet<&'a str>),
}

impl Finished<'_> {
    /// Whether the record `identifier` names is one of them.
    fn holds(&self, identifier: &str) -> bool {
        match self {
            Self::WholeBatch => true,
            Self::Records(finished) => finished.contains(identifier),
        }
    }
}

/// What finishing a batch left its SOURCE at, so the pass that moved it knows whether it
/// made PROGRESS.
///
/// Only a stream source can be left standing still: a shard has ONE position, so a batch
/// whose head record the target did not process — or whose next position could not be taken
/// up — moves that position not at all, and the very same records are the next thing the
/// shard hands over. A queue's records are a CHECKOUT instead: the ones a finish did not
/// delete are invisible until their visibility timeout lapses, so a queue is never left
/// offering them again straight away.
///
/// The distinction is what paces the poller: a pass that moved no position must not be
/// reported as a moved batch, or the poller re-reads, re-enriches and re-delivers the same
/// records with no backoff at all.
enum SourceAdvance {
    /// The source moved on: a queue's finished records were deleted, or the shard is read on
    /// from after them.
    MovedOn,
    /// A shard's position stands where it stood because the target processed no record at
    /// the HEAD of the batch.
    HeadUnprocessed,
    /// A shard's position stands where it stood because the position after the records the
    /// target processed could not be taken up, and why.
    PositionUnavailable(String),
}

/// Finish a batch with its SOURCE, now that the events derived from it have been delivered
/// and the target has reported which records of it it processed: a queue's records are
/// deleted, and a stream's read position moves past them.
///
/// `finished` is which of the polled records that is. A target that reported records it did
/// not process leaves exactly those on the source — a queue's reappear when their visibility
/// timeout lapses, and a shard is read on only from the first record the target did not
/// process, because a shard has ONE position and the records it can be read past are the run
/// at the HEAD of the batch that were processed.
///
/// Nothing here fails the batch. A delete the role may not make, or that fails, only means
/// the record reappears and is delivered again, which is the at-least-once the service
/// promises; and a position this poller cannot take up is one it keeps, so the same records
/// are read again. What it DOES answer is whether the source moved on at all
/// ([`SourceAdvance`]), which is what the caller paces the next pass from.
async fn finish_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    positions: &mut SourcePositions,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    polled: &Polled,
    finished: &Finished<'_>,
) -> SourceAdvance {
    match polled {
        Polled::Messages { messages, .. } => {
            let Source::Queue { url } = &run.source else {
                return SourceAdvance::MovedOn;
            };
            if let Some(detail) = record.refusal(DELETE_MESSAGE_ACTION, &run.source_arn) {
                ::tracing::warn!(
                    queue = %url,
                    detail = %detail,
                    "a delivered pipe source record could not be deleted"
                );
                return SourceAdvance::MovedOn;
            }
            for message in messages {
                // A record the target reported it did not process is left on the queue,
                // invisible until its visibility timeout lapses and then the queue's to hand
                // over again.
                if !finished.holds(message.message_id.as_deref().unwrap_or_default()) {
                    continue;
                }
                let Some(handle) = message.receipt_handle.as_deref() else {
                    continue;
                };
                if let Err(error) = clients
                    .sqs(callback, &run.source_region)
                    .delete_message()
                    .queue_url(url)
                    .receipt_handle(handle)
                    .send()
                    .await
                {
                    ::tracing::warn!(
                        queue = %url,
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "a delivered pipe source record could not be deleted"
                    );
                }
            }
            // The records this finish did not delete are checked out of the queue, so the
            // queue is not offering them again until that checkout lapses.
            SourceAdvance::MovedOn
        }
        Polled::Records(batch) => {
            let processed = match finished {
                // The shard is read from the position AFTER the delivered records from now
                // on. A shard whose answer carried no next position is closed and drained,
                // so it is taken off the walk rather than read from its start again.
                Finished::WholeBatch => {
                    positions
                        .stream
                        .advance(&batch.shard, batch.next_iterator.clone());
                    return SourceAdvance::MovedOn;
                }
                Finished::Records(processed) => processed,
            };
            // The last record of the run at the HEAD of the batch that was processed: the
            // first record the target did not process is where the shard must be read on
            // from. A batch whose very first record was not processed moves the position not
            // at all, so the whole of it is read again — and the pass made no progress.
            let batch_identifiers = stream_identifiers(&batch.records);
            let Some(through) = checkpointed_through(&batch_identifiers, processed) else {
                return SourceAdvance::HeadUnprocessed;
            };
            match shard_iterator_after(callback, clients, record, run, &batch.shard, through).await
            {
                Ok(iterator) => {
                    positions.stream.advance(&batch.shard, iterator);
                    SourceAdvance::MovedOn
                }
                // The position this poller holds is kept, so the batch is read again in
                // full: at-least-once, which is what a stream source promises.
                Err(detail) => {
                    ::tracing::warn!(
                        stream = %run.source_arn,
                        shard = %batch.shard,
                        detail = %detail,
                        "a pipe could not take up the position after the records its target \
                         processed; it reads the batch again"
                    );
                    SourceAdvance::PositionUnavailable(detail)
                }
            }
        }
        // A Kinesis shard is read on from after the records the target processed, which is
        // the run at the HEAD of the batch: the position is the pipe's own and needs no
        // call to take up, so nothing here can fail.
        Polled::KinesisRecords(batch) => {
            let batch_identifiers = kinesis_source::identifiers(&batch.records);
            let through = match finished {
                Finished::WholeBatch => batch_identifiers.last().map(String::as_str),
                Finished::Records(processed) => {
                    checkpointed_through(&batch_identifiers, processed)
                }
            };
            if kinesis_source::finish(&mut positions.kinesis, batch, through) {
                SourceAdvance::MovedOn
            } else {
                SourceAdvance::HeadUnprocessed
            }
        }
    }
}

/// The identifier its source names each of `records` by: a stream record's own sequence
/// number, "the sequence number of the stream record" (`StreamRecord$SequenceNumber`), which
/// is both what names its position in the shard and what a target's report names it by.
fn stream_identifiers(records: &[::aws_sdk_dynamodbstreams::types::Record]) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            record
                .dynamodb
                .as_ref()
                .and_then(|change| change.sequence_number.clone())
                .unwrap_or_default()
        })
        .collect()
}

/// The identifier a shard may be read on from past, after an attempt that processed
/// `processed` of the batch `identifiers`, or `None` when the attempt did not process the
/// batch's own first record and so moved the shard's position not at all.
///
/// A shard has ONE position, so the records it can be read on from past are the run at the
/// HEAD of the batch that were processed: the first record the attempt did not process is the
/// one the next read must start at.
fn checkpointed_through<'a>(
    identifiers: &'a [String],
    processed: &BTreeSet<&str>,
) -> Option<&'a str> {
    identifiers
        .iter()
        .map(String::as_str)
        .take_while(|identifier| processed.contains(identifier))
        .last()
}

/// The one question each stage asks before it makes its call: may this pipe's execution
/// role perform it?
///
/// The pipe's own record, read at the moment a decision needs it rather than kept for the
/// length of a pass.
///
/// Two decisions are taken from it, and both are about what the record says NOW: may the
/// execution role it carries make the call a stage is about to make
/// ([`Self::refusal`], decided by [`super::execution_role::call_refusal`]), and what does the
/// record ask the stages of a batch already polled to do
/// ([`Self::resolved_now`]). This holds only where those are read FROM — the pipes
/// store this poller runs in and the IAM store of the same engine — and takes each guard
/// for the length of one decision, so no call leaves this process while either is held.
struct LiveRecord<'a> {
    iam: Option<&'a ::engine::ServiceState<super::execution_role::IamState>>,
    pipes: &'a ::engine::ServiceState<ComAmazonawsPipesPipesState>,
    pipe: &'a str,
}

impl LiveRecord<'_> {
    /// The execution role the pipe carries NOW, or `None` when the store holds no such
    /// pipe any more.
    fn role_arn(&self) -> Option<String> {
        self.pipes
            .lock()
            .pipes
            .get(self.pipe)
            .map(|pipe| pipe.role_arn.clone())
    }

    /// Whether the pipe's record says it RUNS at this instant: the one gate every read of
    /// its source is taken under.
    ///
    /// Read from the record when the question is asked, never captured for the length of
    /// a poll, so a call that moved the pipe out of `RUNNING` while a batch was being
    /// collected is seen by the very next read — the records that arrive after it are left
    /// on the source for whichever later poll runs under a record that says the pipe runs
    /// again. The moves the clock has already made due are materialised first, exactly as
    /// every other reader of a `CurrentState` does, so a pipe whose settle span has lapsed
    /// is seen as the state it became and not as the one it was leaving.
    fn runs_now(&self) -> bool {
        let mut store = self.pipes.lock();
        store.settle_due_transitions();
        store
            .pipes
            .get(self.pipe)
            .is_some_and(PipeRecord::rests_in_running)
    }

    /// What the pipe's record asks for NOW, or `None` for a pipe the store no longer holds
    /// and one whose record asks for a configuration this data plane does not serve.
    ///
    /// This is what the stages of a batch already polled act under: the configuration is
    /// read when the batch is decided, so a call that reconfigured the pipe while the batch
    /// was being collected decides every stage that batch has not reached yet.
    fn resolved_now(&self) -> Option<RunningPipe> {
        let store = self.pipes.lock();
        resolve_run(store.pipes.get(self.pipe)?).ok()
    }

    /// Why the role may not perform `action` on `resource`, or `None` when nothing of the
    /// kind can be shown and the call goes ahead.
    fn refusal(&self, action: &str, resource: &str) -> Option<String> {
        let Some(role_arn) = self.role_arn() else {
            return Some(format!(
                "no record of the pipe {} is stored, so no execution role is in force \
                 for {action} on {resource}",
                self.pipe
            ));
        };
        let iam = self.iam?;
        super::execution_role::call_refusal(&role_arn, action, resource, &iam.lock())
    }
}

/// What one poll of a source came back with, by the kind of source it was polled from.
///
/// It carries what the SOURCE needs to be finished with ([`finish_batch`]) as well as the
/// records themselves: an Amazon SQS message holds the receipt handle its delete needs,
/// and a stream batch holds the shard it came from and the position after it.
enum Polled {
    /// Amazon SQS messages, in the order they were received.
    Messages {
        messages: Vec<::aws_sdk_sqs::types::Message>,
        /// When the LAST receive of this poll handed records over: a received record is
        /// hidden from every other consumer of the queue from that instant, for the
        /// queue's own visibility timeout, so this is where the checkout of the batch as a
        /// whole is counted from.
        checked_out: Instant,
    },
    /// One shard's stream records, in the order the changes were applied.
    Records(StreamBatch),
    /// One shard's Kinesis records, in the order they were put.
    KinesisRecords(kinesis_source::Batch),
}

impl Polled {
    /// How many source records this poll took.
    fn len(&self) -> usize {
        match self {
            Self::Messages { messages, .. } => messages.len(),
            Self::Records(batch) => batch.records.len(),
            Self::KinesisRecords(batch) => batch.records.len(),
        }
    }

    /// Whether the source held nothing.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// When these records were checked OUT of their source, for a source a poll takes a
    /// checkout on.
    ///
    /// An Amazon SQS receive is one: what it hands over is invisible to every other
    /// consumer of the queue for the queue's visibility timeout, and comes back when that
    /// lapses. A stream read is not: a shard iterator names a position and takes nothing
    /// away from anyone, so there is nothing to come back and this answers `None`.
    fn checked_out(&self) -> Option<Instant> {
        match self {
            Self::Messages { checked_out, .. } => Some(*checked_out),
            Self::Records(_) | Self::KinesisRecords(_) => None,
        }
    }

    /// The identifier its SOURCE names each polled record by, in the order they were
    /// polled.
    ///
    /// This is what a target's report of the records it did not process names them by
    /// ([`::service_common::batch_item_failures`]): an Amazon SQS message is named by its
    /// message id and a stream record by its sequence number.
    fn identifiers(&self) -> Vec<String> {
        match self {
            Self::Messages { messages, .. } => messages
                .iter()
                .map(|message| message.message_id.clone().unwrap_or_default())
                .collect(),
            Self::Records(batch) => stream_identifiers(&batch.records),
            Self::KinesisRecords(batch) => kinesis_source::identifiers(&batch.records),
        }
    }

    /// What the batch this poll took is COUNTED under when an attempt at it fails: the
    /// position its source hands it over from, which is the shard it came from and the
    /// identifier of its first record.
    ///
    /// A source whose position has not moved hands the same first record over again, so two
    /// attempts at a batch beginning at that record are two attempts at the same work —
    /// whether or not the later poll happened to collect more records behind it.
    fn attempt_key(&self) -> String {
        match self {
            Self::Messages { messages, .. } => messages
                .first()
                .and_then(|message| message.message_id.clone())
                .unwrap_or_default(),
            Self::Records(batch) => format!(
                "{}:{}",
                batch.shard,
                stream_identifiers(&batch.records)
                    .first()
                    .cloned()
                    .unwrap_or_default()
            ),
            Self::KinesisRecords(batch) => format!(
                "{}:{}",
                batch.shard,
                kinesis_source::identifiers(&batch.records)
                    .first()
                    .cloned()
                    .unwrap_or_default()
            ),
        }
    }

    /// This poll as a DEAD LETTER points at the batch it took: the shard and the run of
    /// sequence numbers it covered.
    ///
    /// `None` for a poll a dead letter cannot point at — an Amazon SQS receive, whose source
    /// parameters carry no `DeadLetterConfig` to begin with, because a queue's records are
    /// addressed by a receipt that is gone by then rather than by a position in a shard.
    fn discarded_batch(&self) -> Option<dead_letter::DiscardedBatch> {
        match self {
            Self::Messages { .. } => None,
            Self::Records(batch) => {
                let identifiers = stream_identifiers(&batch.records);
                let arrival = |record: Option<&::aws_sdk_dynamodbstreams::types::Record>| {
                    record
                        .and_then(|record| record.dynamodb.as_ref())
                        .and_then(|change| change.approximate_creation_date_time.as_ref())
                        .and_then(dead_letter::instant_text)
                };
                Some(dead_letter::DiscardedBatch {
                    member: dead_letter::DYNAMODB_BATCH_INFO,
                    shard: batch.shard.clone(),
                    start_sequence_number: identifiers.first().cloned().unwrap_or_default(),
                    end_sequence_number: identifiers.last().cloned().unwrap_or_default(),
                    first_arrival: arrival(batch.records.first()),
                    last_arrival: arrival(batch.records.last()),
                    size: batch.records.len(),
                })
            }
            Self::KinesisRecords(batch) => {
                let identifiers = kinesis_source::identifiers(&batch.records);
                let arrival = |record: Option<&::aws_sdk_kinesis::types::Record>| {
                    record
                        .and_then(::aws_sdk_kinesis::types::Record::approximate_arrival_timestamp)
                        .and_then(dead_letter::instant_text)
                };
                Some(dead_letter::DiscardedBatch {
                    member: dead_letter::KINESIS_BATCH_INFO,
                    shard: batch.shard.clone(),
                    start_sequence_number: identifiers.first().cloned().unwrap_or_default(),
                    end_sequence_number: identifiers.last().cloned().unwrap_or_default(),
                    first_arrival: arrival(batch.records.first()),
                    last_arrival: arrival(batch.records.last()),
                    size: batch.records.len(),
                })
            }
        }
    }

    /// The WHOLE poll as the record envelopes a pipe hands to its stages, in the order
    /// the records were polled in.
    fn envelopes(&self, run: &RunningPipe) -> Result<Vec<Value>, String> {
        match self {
            Self::Messages { messages, .. } => Ok(messages
                .iter()
                .map(|message| queue_record_envelope(message, &run.source_arn, &run.source_region))
                .collect()),
            Self::Records(batch) => batch
                .records
                .iter()
                .map(|record| {
                    ::service_common::dynamodb_stream_event::record_envelope(
                        record,
                        &run.source_arn,
                        &run.source_region,
                    )
                })
                .collect(),
            Self::KinesisRecords(batch) => Ok(batch
                .records
                .iter()
                .map(|record| kinesis_source::record_envelope(record, &batch.shard, run))
                .collect()),
        }
    }
}

/// One shard's worth of stream records, with where they were read from and where the
/// shard is read from NEXT once they have been delivered.
struct StreamBatch {
    /// The shard they came from: one partition key's changes are all in one shard, which
    /// is why reading a shard in order preserves the order they were applied in.
    shard: String,
    /// The position after the last record read, or `None` for a shard that is closed and
    /// drained. Taken up only once the batch has been delivered.
    next_iterator: Option<String>,
    records: Vec<::aws_sdk_dynamodbstreams::types::Record>,
}

/// Where a DynamoDB-stream source has been read up to, for the length of one poller.
///
/// A shard iterator names an immutable position and is not consumed by the read it
/// serves, so the position a shard is read FROM is kept here until the batch read from it
/// has been delivered: a failed delivery re-reads the same records rather than skipping
/// them, which is the at-least-once a stream source promises.
///
/// Both maps are bounded by the stream's own shard count: a shard the stream no longer
/// lists is forgotten, and a drained closed shard is remembered only by its id.
#[derive(Default)]
struct StreamCursors {
    /// The position each live shard is read from next, by shard id.
    shards: BTreeMap<String, String>,
    /// The shards that are closed and read to their end, which are not read again.
    drained: BTreeSet<String>,
    /// The shard the last pass moved a batch from. The next pass lists the shards AFTER
    /// it and wraps round at the end of the stream, so each shard is served in turn
    /// rather than one busy shard starving the rest.
    last_served: Option<String>,
    /// Whether this poller has taken up its position in the stream at all. The
    /// configured `StartingPosition` applies to the shards the stream had THEN; a shard
    /// the stream gains afterwards is read from its own beginning, so no change made to
    /// it is skipped.
    started: bool,
}

impl StreamCursors {
    /// Take up the position after a delivered batch, or retire a shard that is closed and
    /// drained.
    fn advance(&mut self, shard: &str, next_iterator: Option<String>) {
        match next_iterator {
            Some(iterator) => {
                self.shards.insert(shard.to_string(), iterator);
            }
            None => {
                self.shards.remove(shard);
                self.drained.insert(shard.to_string());
            }
        }
    }

    /// Forget every shard the stream no longer lists, so what this holds is bounded by
    /// the stream as it is now.
    fn retain_listed(&mut self, listed: &BTreeSet<String>) {
        self.shards.retain(|shard, _| listed.contains(shard));
        self.drained.retain(|shard| listed.contains(shard));
    }
}

/// Where the STREAM sources this poller may read have been read up to, for the length of
/// one poller.
///
/// A pipe reads from exactly one source, so only the member for the kind its `Source`
/// names is ever filled; they are held together so one poller carries one position for
/// whatever its record names, and a reconfiguration that changes the source kind cannot
/// hand the new source the old one's place.
#[derive(Default)]
struct SourcePositions {
    /// Where a DynamoDB-stream source has been read up to.
    stream: StreamCursors,
    /// Where a Kinesis-stream source has been read up to.
    kinesis: kinesis_source::Cursors,
}

/// Poll the source for up to `BatchSize` records, collecting for up to the batching
/// window once the first record has arrived.
///
/// The window is what lets a batch span several reads: "the maximum length of a time
/// to wait for events". It starts when the poll finds its first record, so an idle
/// source is not waited on — this pass simply reports nothing and the next one looks
/// again.
///
/// WHICH source is polled is the pipe's own ([`Source`]), and the one question every
/// stage asks is asked here too: may the pipe's execution role make the call this poll is
/// about to make?
async fn collect_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    positions: &mut SourcePositions,
) -> Result<Polled, String> {
    match &run.source {
        Source::Queue { url } => {
            if let Some(detail) = record.refusal(RECEIVE_MESSAGE_ACTION, &run.source_arn) {
                return Err(detail);
            }
            let mut collected = receive(callback, clients, run, url, run.batch_size).await?;
            // When the queue handed these records over, and therefore when it hid them
            // from its other consumers: the LAST receive that added to the batch, because
            // each receive hides what it hands over from the moment it is made.
            let mut checked_out = Instant::now();
            if collected.is_empty() {
                return Ok(Polled::Messages {
                    messages: collected,
                    checked_out,
                });
            }
            let deadline = Instant::now() + run.batching_window;
            // The run gate, re-read before every FURTHER read of this source (the first
            // one is the decision this pass was taken under, made moments ago while the
            // store was locked): a pipe moved out of RUNNING while the batch was being
            // collected receives nothing more, so the records sent to the queue after
            // that move stay on it, visible and counted.
            while collected.len() < run.batch_size
                && Instant::now() < deadline
                && record.runs_now()
            {
                let wanted = run.batch_size - collected.len();
                let more = receive(callback, clients, run, url, wanted).await?;
                if more.is_empty() {
                    ::tokio::time::sleep(POLL_INTERVAL).await;
                } else {
                    checked_out = Instant::now();
                    collected.extend(more);
                }
            }
            Ok(Polled::Messages {
                messages: collected,
                checked_out,
            })
        }
        Source::Stream { starting_position } => {
            let batch = collect_stream_batch(
                callback,
                clients,
                record,
                run,
                starting_position,
                &mut positions.stream,
            )
            .await?;
            Ok(Polled::Records(batch))
        }
        Source::KinesisStream { .. } => {
            let batch =
                kinesis_source::collect(callback, clients, record, run, &mut positions.kinesis)
                    .await?;
            Ok(Polled::KinesisRecords(batch))
        }
    }
}

/// One `ReceiveMessage` for up to `wanted` records, with every attribute a record
/// envelope reports.
async fn receive(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &RunningPipe,
    url: &str,
    wanted: usize,
) -> Result<Vec<::aws_sdk_sqs::types::Message>, String> {
    let asked = i32::try_from(wanted)
        .unwrap_or(MAX_RECEIVE_PER_CALL)
        .clamp(1, MAX_RECEIVE_PER_CALL);
    let answer = clients
        .sqs(callback, &run.source_region)
        .receive_message()
        .queue_url(url)
        .max_number_of_messages(asked)
        .message_system_attribute_names(::aws_sdk_sqs::types::MessageSystemAttributeName::All)
        .message_attribute_names(ALL_ATTRIBUTES)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the source queue {url} could not be received from ({})",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.messages.unwrap_or_default())
}

/// Take up the position a stream source is read FROM, without reading anything.
///
/// This is what a pipe that is still reaching RUNNING does: it mints the shard iterators
/// its configured `StartingPosition` names, so a change applied between the call that
/// started the pipe and the moment it runs is inside the span the pipe then reads. For a
/// queue source there is no position to take up and this does nothing.
async fn establish_cursors(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    positions: &mut SourcePositions,
) -> Result<(), String> {
    match &run.source {
        Source::Queue { .. } => Ok(()),
        Source::Stream { starting_position } => {
            open_shards(
                callback,
                clients,
                record,
                run,
                starting_position,
                &mut positions.stream,
            )
            .await?;
            Ok(())
        }
        Source::KinesisStream { .. } => {
            kinesis_source::establish(callback, clients, record, run, &mut positions.kinesis).await
        }
    }
}

/// Take up the position every shard the stream lists NOW is read from — minting the
/// iterators this poller does not hold yet — and answer those shards with the position
/// each is read from, `None` for one this poller is finished with.
///
/// Every listed shard is opened before any of them is read, so the configured
/// `StartingPosition` is applied to the stream as it is at that moment and not shard by
/// shard as the reads reach them: a `LATEST` pipe would otherwise read a shard it had not
/// opened yet from a tail that had moved on, which is a skipped change.
async fn open_shards(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    starting_position: &str,
    cursors: &mut StreamCursors,
) -> Result<Vec<(String, Option<String>)>, String> {
    let shards = list_shards(callback, clients, record, run, cursors).await?;
    let mut opened = Vec::with_capacity(shards.len());
    for shard in shards {
        let iterator =
            shard_iterator(callback, clients, record, run, &shard, starting_position, cursors)
                .await?;
        opened.push((shard, iterator));
    }
    // Every shard of the stream as it is now holds a position of its own, so the
    // configured starting position has been applied; a shard the stream gains after this
    // is read from its own beginning.
    cursors.started = true;
    Ok(opened)
}

/// Read up to `BatchSize` records from ONE shard of the stream, collecting for up to the
/// batching window once the first record has been read.
///
/// The shards are walked in the order the pass listed them — which starts after the shard
/// the previous pass served — and the FIRST one holding records is the batch: a pipe moves
/// one batch at a time, so the records of one shard, and therefore the changes to one
/// partition key, which all land in one shard, reach the target in the order they were
/// applied.
///
/// Each read asks only for the records the batch still has room for, so the batch never
/// holds more than `BatchSize` and no record this pass read is left out of it: the
/// position the batch is finished at is the position after the records it carries.
async fn collect_stream_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    starting_position: &str,
    cursors: &mut StreamCursors,
) -> Result<StreamBatch, String> {
    let opened = open_shards(callback, clients, record, run, starting_position, cursors).await?;
    for (shard, iterator) in opened {
        let Some(iterator) = iterator else {
            continue;
        };
        let (mut records, mut next) =
            get_records(callback, clients, record, run, &iterator, run.batch_size).await?;
        if records.is_empty() {
            // A closed shard that answered nothing more is drained: it is retired here
            // rather than read again, because its position cannot move any further.
            if next.is_none() {
                cursors.advance(&shard, None);
            }
            continue;
        }
        let deadline = Instant::now() + run.batching_window;
        // The same run gate the queue source's collection is held to: a stream that is
        // still being read when the pipe leaves RUNNING is read no further, so the
        // records written after that move are left for a later poll.
        while records.len() < run.batch_size && Instant::now() < deadline && record.runs_now() {
            let Some(iterator) = next.clone() else {
                break;
            };
            let wanted = run.batch_size - records.len();
            let (more, after) =
                get_records(callback, clients, record, run, &iterator, wanted).await?;
            if more.is_empty() {
                ::tokio::time::sleep(POLL_INTERVAL).await;
            } else {
                records.extend(more);
                next = after;
            }
        }
        cursors.last_served = Some(shard.clone());
        return Ok(StreamBatch {
            shard,
            next_iterator: next,
            records,
        });
    }
    Ok(StreamBatch {
        shard: String::new(),
        next_iterator: None,
        records: Vec::new(),
    })
}

/// The shards of the source stream this pass may read, in the order `DescribeStream`
/// lists them.
///
/// The listing WINDOW starts after the shard the previous pass served
/// ([`StreamCursors::last_served`]) and wraps round when it runs past the last shard, so
/// each shard of the stream is served in turn — an earlier shard that is written
/// continuously cannot starve the ones behind it, and a stream with more shards than one
/// pass lists is covered pass by pass.
///
/// `DescribeStream` pages its shards, so the pages are followed — bounded by
/// [`MAX_DESCRIBE_STREAM_PAGES`] and [`MAX_SHARDS_PER_PASS`], because the count is the
/// table's and not this pass's. What this poller remembers about OTHER shards is pruned
/// only when the window covered the whole stream, so a shard outside this window keeps
/// the position it was read up to.
async fn list_shards(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    cursors: &mut StreamCursors,
) -> Result<Vec<String>, String> {
    if let Some(detail) = record.refusal(DESCRIBE_STREAM_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let from = cursors.last_served.clone();
    let mut listed = describe_shards(callback, clients, run, from.clone()).await?;
    if listed.shards.is_empty() && from.is_some() {
        // The window ran past the stream's last shard: the next one is its beginning.
        cursors.last_served = None;
        listed = describe_shards(callback, clients, run, None).await?;
    }
    if listed.complete {
        cursors.retain_listed(&listed.shards.iter().cloned().collect());
    }
    Ok(listed.shards)
}

/// The shards one run of `DescribeStream` pages lists, starting after `after`.
struct ListedShards {
    shards: Vec<String>,
    /// Whether the run covered the WHOLE stream: it started at the beginning and the
    /// service reported no shard beyond the ones it listed. Only then is what this poller
    /// remembers about shards outside the run safe to prune.
    complete: bool,
}

/// One run of `DescribeStream` pages, from the shard after `after` (or from the stream's
/// first shard when there is none).
async fn describe_shards(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &RunningPipe,
    after: Option<String>,
) -> Result<ListedShards, String> {
    let from_the_beginning = after.is_none();
    let mut shards: Vec<String> = Vec::new();
    let mut after = after;
    let mut whole_stream = false;
    for _ in 0..MAX_DESCRIBE_STREAM_PAGES {
        let mut request = clients
            .dynamodbstreams(callback, &run.source_region)
            .describe_stream()
            .stream_arn(&run.source_arn);
        if let Some(after) = after.as_deref() {
            request = request.exclusive_start_shard_id(after);
        }
        let answer = request.send().await.map_err(|error| {
            format!(
                "the source stream {} could not be described ({})",
                run.source_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
        let Some(description) = answer.stream_description else {
            break;
        };
        for shard in description.shards.unwrap_or_default() {
            if let Some(id) = shard.shard_id {
                shards.push(id);
            }
        }
        after = description.last_evaluated_shard_id;
        if after.is_none() {
            whole_stream = true;
            break;
        }
        if shards.len() >= MAX_SHARDS_PER_PASS {
            break;
        }
    }
    let within_the_bound = shards.len() <= MAX_SHARDS_PER_PASS;
    shards.truncate(MAX_SHARDS_PER_PASS);
    Ok(ListedShards {
        shards,
        complete: from_the_beginning && whole_stream && within_the_bound,
    })
}

/// The position a shard is read from on this pass, minting one when this poller holds
/// none, or `None` for a shard it is finished with.
///
/// The configured `StartingPosition` is where the shards the stream had when the poller
/// took up its position are read from; a shard it gains afterwards is read from its own
/// beginning, so nothing appended to a new shard is skipped.
async fn shard_iterator(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    shard: &str,
    starting_position: &str,
    cursors: &mut StreamCursors,
) -> Result<Option<String>, String> {
    if cursors.drained.contains(shard) {
        return Ok(None);
    }
    if let Some(held) = cursors.shards.get(shard) {
        return Ok(Some(held.clone()));
    }
    if let Some(detail) = record.refusal(GET_SHARD_ITERATOR_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let from = if cursors.started {
        ::aws_sdk_dynamodbstreams::types::ShardIteratorType::TrimHorizon
    } else {
        iterator_type(starting_position)
    };
    let answer = clients
        .dynamodbstreams(callback, &run.source_region)
        .get_shard_iterator()
        .stream_arn(&run.source_arn)
        .shard_id(shard)
        .shard_iterator_type(from)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the shard {shard} of the source stream {} could not be opened ({})",
                run.source_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // A shard that is closed and already past its end has no iterator to read from.
    let Some(iterator) = answer.shard_iterator else {
        cursors.drained.insert(shard.to_string());
        return Ok(None);
    };
    cursors.shards.insert(shard.to_string(), iterator.clone());
    Ok(Some(iterator))
}

/// The position a shard is read on from AFTER the record `sequence_number` names, or `None`
/// when the stream answered none.
///
/// This is what a partially processed batch leaves the shard at: `AFTER_SEQUENCE_NUMBER` is
/// "start reading right after the position denoted by a specific sequence number"
/// (`GetShardIterator$ShardIteratorType`), so the next read begins at the first record the
/// target did not process.
async fn shard_iterator_after(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    shard: &str,
    sequence_number: &str,
) -> Result<Option<String>, String> {
    if let Some(detail) = record.refusal(GET_SHARD_ITERATOR_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let answer = clients
        .dynamodbstreams(callback, &run.source_region)
        .get_shard_iterator()
        .stream_arn(&run.source_arn)
        .shard_id(shard)
        .shard_iterator_type(::aws_sdk_dynamodbstreams::types::ShardIteratorType::AfterSequenceNumber)
        .sequence_number(sequence_number)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the position after the record {sequence_number} of the shard {shard} could \
                 not be taken up ({})",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok(answer.shard_iterator)
}

/// The shard iterator type the configured `StartingPosition` reads from. Both values
/// `DynamoDBStreamStartPosition` admits are DynamoDB Streams' own iterator types.
fn iterator_type(starting_position: &str) -> ::aws_sdk_dynamodbstreams::types::ShardIteratorType {
    match starting_position {
        LATEST_POSITION => ::aws_sdk_dynamodbstreams::types::ShardIteratorType::Latest,
        TRIM_HORIZON_POSITION => ::aws_sdk_dynamodbstreams::types::ShardIteratorType::TrimHorizon,
        // A value outside the enum never reaches here — the boundary validator refuses one
        // — and is read as the stream's oldest retained record rather than as a position
        // that would silently skip records.
        _ => ::aws_sdk_dynamodbstreams::types::ShardIteratorType::TrimHorizon,
    }
}

/// One `GetRecords` from `iterator` for at most `wanted` records: the records it answered
/// and the position after them.
///
/// `wanted` is what the batch still has room for, so a read never answers records the
/// batch cannot carry — the position this answers is the position after the records the
/// batch takes.
async fn get_records(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    record: &LiveRecord<'_>,
    run: &RunningPipe,
    iterator: &str,
    wanted: usize,
) -> Result<
    (
        Vec<::aws_sdk_dynamodbstreams::types::Record>,
        Option<String>,
    ),
    String,
> {
    if let Some(detail) = record.refusal(GET_RECORDS_ACTION, &run.source_arn) {
        return Err(detail);
    }
    let limit = i32::try_from(wanted).unwrap_or(i32::MAX).max(1);
    let answer = clients
        .dynamodbstreams(callback, &run.source_region)
        .get_records()
        .shard_iterator(iterator)
        .limit(limit)
        .send()
        .await
        .map_err(|error| {
            format!(
                "the source stream {} could not be read ({})",
                run.source_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    Ok((
        answer.records.unwrap_or_default(),
        answer.next_shard_iterator,
    ))
}

/// The events of `batch` the pipe's filters admit, in the order they were polled.
///
/// "The collection of event patterns used to filter events": a record that matches ANY of
/// the patterns enters the pipe, and a pipe carrying no filter admits every record. The
/// patterns are matched by the one reading of the event-pattern language in this tree,
/// against the record as the filter READS it ([`FilterView`]), under one work budget for
/// the whole batch, so a batch can never buy more matching work than the budget sold it.
fn admitted(filters: &[Value], batch: Vec<Value>) -> Vec<Value> {
    if filters.is_empty() {
        return batch;
    }
    let mut budget =
        ::service_common::event_pattern::MatchBudget::new(MAX_FILTER_STEPS_PER_BATCH);
    batch
        .into_iter()
        .filter(|event| {
            // The record is matched as the FILTER reads it, which is not always as the
            // envelope carries it; what the later stages are handed is the envelope
            // itself, untouched.
            let view = FilterView::of(event);
            filters
                .iter()
                .any(|pattern| view.admits(pattern, &mut budget))
        })
        .collect()
}

/// The member of a source record envelope that carries the message AS SENT, whose format
/// is the producer's and not the envelope's.
const RECORD_BODY: &str = "body";

/// The member a Kinesis record envelope carries its payload under, which travels
/// base64-encoded because a record's data is bytes the producer put — so the payload a
/// reader looks inside is what that encoding undoes.
const RECORD_DATA: &str = "data";

/// The member a record envelope names the source it came from under, which is what says
/// how its payload member travels.
const RECORD_EVENT_SOURCE: &str = "eventSource";

/// The format the message a record carries is in.
///
/// A record envelope reports its payload as the text it was sent as — under the encoding
/// its source carries one with — whatever that text spells; the filter stage is the one
/// reader that cares which of the two formats a producer used, because a pattern is written
/// for one of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyFormat {
    /// The body is in "a valid JSON format" — it spells a JSON document — so it is read
    /// as the document it parses to.
    Document,
    /// The body spells no document, so it is read as the text it carries.
    Text,
}

/// How the FILTER stage reads one polled record.
///
/// A record's payload travels as text — an Amazon SQS body as the text its producer sent,
/// a Kinesis record's data as the base64 of the bytes it put — and a pattern's clause for
/// that member is written either for the document the payload spells or for the text it
/// carries. So the filter decides the payload's FORMAT once per record and reads the record
/// accordingly: a payload in "a valid JSON format" is matched as the document it parses to,
/// a payload that spells none as its text, and a pattern whose clause is written for the
/// other format admits the record no more than it would admit one whose values disagreed. A
/// record envelope carrying no payload member at all — a DynamoDB stream record, whose
/// change is already a document — is read exactly as it was built.
///
/// This view lives only for the length of the match: the payload the enrichment and the
/// target are handed is the record envelope with its payload member as sent.
struct FilterView<'a> {
    /// Which member carries the record's payload and the format it is in, or `None` for an
    /// envelope that carries none.
    reading: Option<PayloadReading>,
    /// The record as the patterns see it.
    event: Cow<'a, Value>,
}

impl<'a> FilterView<'a> {
    /// How the filter stage reads `event`.
    fn of(event: &'a Value) -> Self {
        let (reading, event) = read_payload(event);
        Self { reading, event }
    }

    /// Whether `pattern` admits this record, spending `budget`.
    ///
    /// The format of the payload is decided BEFORE any value is compared, so a pattern
    /// whose clause for the payload member is written for the other format drops the record
    /// whatever it holds.
    fn admits(
        &self,
        pattern: &Value,
        budget: &mut ::service_common::event_pattern::MatchBudget,
    ) -> bool {
        self.format_fits(pattern)
            && ::service_common::event_pattern::matches_within(pattern, &self.event, budget)
    }

    /// Whether `pattern`'s clause for this record's PAYLOAD member is written for the
    /// format that payload carries. A pattern with no such clause constrains neither format
    /// and fits both.
    fn format_fits(&self, pattern: &Value) -> bool {
        let Some(reading) = self.reading else {
            return true;
        };
        let form = match reading.format {
            BodyFormat::Document => ::service_common::event_pattern::ClauseForm::Document,
            BodyFormat::Text => ::service_common::event_pattern::ClauseForm::Values,
        };
        ::service_common::event_pattern::reads_as(pattern, reading.member, form)
    }
}

/// How one record envelope carries its payload: the member that holds it, and the format
/// the payload that member spells is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PayloadReading {
    /// The member the payload travels under ([`RECORD_BODY`], [`RECORD_DATA`]).
    member: &'static str,
    format: BodyFormat,
}

/// The record `event` as every reader that looks INSIDE it reads it: how its payload member
/// is read, and the record with that member replaced by the payload it spells.
///
/// A record's payload travels as the text its producer sent, under whatever encoding its
/// source carries it with — an Amazon SQS body as that text itself, a Kinesis record's data
/// as the base64 of the bytes that were put — and a reader that looks inside the record
/// (the filter's patterns [`FilterView`], and the JSON paths a template placeholder or a
/// target parameter carries [`path_view`]) reads the payload that encoding undoes, in the
/// format that payload is in: a payload in "a valid JSON format" is the document it parses
/// to, so a clause or a path can name what is inside it, and a payload spelling no document
/// is the text it carries. An envelope with no payload member at all — a DynamoDB stream
/// record, whose change is already a document — is read exactly as it was built, and the
/// reading is then `None`.
///
/// The payload is decoded and parsed at most once per reader per record, and what it may be
/// is already bounded by the largest record its source accepts, so the work is bounded by
/// the poll that collected the record. The view lives only for the length of the reading:
/// the payload a stage handed the event itself receives is the envelope with its payload
/// member as sent.
fn read_payload(event: &Value) -> (Option<PayloadReading>, Cow<'_, Value>) {
    let Some((member, payload)) = payload_as_read(event) else {
        return (None, Cow::Borrowed(event));
    };
    // A payload is in a JSON format when it spells a JSON DOCUMENT — an object or a
    // list, which is what a clause names fields inside and what a path descends into. A
    // payload spelling a bare scalar names nothing to descend into and is read as the text
    // it is, the same reading the event-source poller of a Lambda function takes.
    let document = ::serde_json::from_str::<Value>(&payload)
        .ok()
        .filter(|payload| payload.is_object() || payload.is_array());
    let reading = PayloadReading {
        member,
        format: if document.is_some() {
            BodyFormat::Document
        } else {
            BodyFormat::Text
        },
    };
    // What the member is REPLACED by, and nothing at all when the envelope already carries
    // exactly what a reader reads — a borrowed payload is the envelope's own text, an owned
    // one is what undoing the encoding it travelled under produced.
    let read_as = match document {
        Some(document) => Some(document),
        None => match payload {
            Cow::Borrowed(_) => None,
            Cow::Owned(text) => Some(Value::String(text)),
        },
    };
    let Some(read_as) = read_as else {
        return (Some(reading), Cow::Borrowed(event));
    };
    let mut view = event.clone();
    if let Value::Object(members) = &mut view {
        members.insert(member.to_string(), read_as);
    }
    (Some(reading), Cow::Owned(view))
}

/// Which member of `event` carries its payload and the text that payload spells, or `None`
/// for an envelope that carries no payload member and for one whose payload is no text at
/// all.
///
/// The text is BORROWED where the envelope carries it as it is, and OWNED where undoing the
/// encoding it travels under produced it — which is what tells a reading that replaces the
/// member from one that leaves the envelope alone.
///
/// A Kinesis record's data that is not base64 of UTF-8 text is bytes rather than text, so
/// there is no payload for a clause or a path to look inside and the envelope is read as it
/// was built.
fn payload_as_read(event: &Value) -> Option<(&'static str, Cow<'_, str>)> {
    if let Some(text) = event.get(RECORD_BODY).and_then(Value::as_str) {
        return Some((RECORD_BODY, Cow::Borrowed(text)));
    }
    // The `data` member is read as the base64 it travels as only for a record whose own
    // `eventSource` says it came off a Kinesis stream: a member of that name carries an
    // encoded payload nowhere else, and an event an enrichment answered with one of its own
    // is read as the value it holds.
    if event.get(RECORD_EVENT_SOURCE).and_then(Value::as_str)
        != Some(kinesis_source::KINESIS_EVENT_SOURCE)
    {
        return None;
    }
    let encoded = event.get(RECORD_DATA).and_then(Value::as_str)?;
    let decoded = ::aws_smithy_types::base64::decode(encoded).ok()?;
    Some((RECORD_DATA, Cow::Owned(String::from_utf8(decoded).ok()?)))
}

/// The record `event` as a JSON PATH reads it — the reading [`read_payload`] states,
/// without the format the filter also decides by.
///
/// A path is resolved against this: `$.body` selects the document a valid-JSON body spells
/// and `$.data` the document a Kinesis record's decoded data spells, so
/// `$.<member>.<field>` selects that field of it, and a payload spelling no document is the
/// text a path selects. It is what both readers of a path are given, the placeholders of an
/// `InputTemplate` ([`render_template`]) and a target parameter's own value
/// ([`resolved_parameter`]), so one pipe cannot read a record two ways.
fn path_view(event: &Value) -> Cow<'_, Value> {
    read_payload(event).1
}

/// The member a record envelope reports the digest of its MESSAGE ATTRIBUTES under —
/// "an MD5 digest of the non-URL-encoded message attribute string"
/// (`ReceiveMessage`'s `Message$MD5OfMessageAttributes`), which is what lets a consumer
/// of a forwarded record check the attributes reached it intact.
///
/// Amazon SQS reports that digest for a message that CARRIES message attributes and
/// reports none for a message with none, so the envelope carries the member exactly when
/// the record it was built from has one.
const RECORD_MD5_OF_MESSAGE_ATTRIBUTES: &str = "md5OfMessageAttributes";

/// One polled Amazon SQS message as the record envelope a pipe hands to its enrichment
/// and its target: the message's own id, receipt handle, body and MD5s, its system and
/// message attributes, and which source it came from.
fn queue_record_envelope(
    message: &::aws_sdk_sqs::types::Message,
    source_arn: &str,
    region: &str,
) -> Value {
    let mut attributes: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in message.attributes.iter().flatten() {
        attributes.insert(name.as_str().to_string(), value.clone());
    }
    let mut message_attributes: BTreeMap<String, Value> = BTreeMap::new();
    for (name, attribute) in message.message_attributes.iter().flatten() {
        let mut rendered = json!({ "dataType": attribute.data_type.clone() });
        if let Some(text) = attribute.string_value.clone() {
            rendered["stringValue"] = Value::String(text);
        }
        if let Some(bytes) = attribute.binary_value.as_ref() {
            // A binary attribute travels as base64 in a JSON envelope, the way the
            // record envelope reports one.
            rendered["binaryValue"] = Value::String(
                ::base64::Engine::encode(&::base64::engine::general_purpose::STANDARD, bytes.as_ref()),
            );
        }
        message_attributes.insert(name.clone(), rendered);
    }
    let mut envelope = json!({
        "messageId": message.message_id.clone().unwrap_or_default(),
        "receiptHandle": message.receipt_handle.clone().unwrap_or_default(),
        "body": message.body.clone().unwrap_or_default(),
        "attributes": attributes,
        "messageAttributes": message_attributes,
        "md5OfBody": message.md5_of_body.clone().unwrap_or_default(),
        "eventSource": SQS_EVENT_SOURCE,
        "eventSourceARN": source_arn,
        "awsRegion": region,
    });
    // The attribute digest is the one the RECEIVE that handed this message over reported,
    // never one computed here: the digest is Amazon SQS's own and a record it reports none
    // for (one carrying no message attributes) carries none on the envelope either.
    if let Some(digest) = message.md5_of_message_attributes.clone() {
        envelope[RECORD_MD5_OF_MESSAGE_ATTRIBUTES] = Value::String(digest);
    }
    envelope
}

/// Why a stage's own CALL did not come back with what the pipe needed, and — for a call the
/// invoked resource answered an error of its own on — the error it named.
///
/// The error is the invoked resource's own word for what went wrong, which is what a record
/// of a discarded batch reports ([`dead_letter::payload`]); everything else about the failure
/// is this emulator's account of it and travels only in the log line.
#[derive(Debug)]
struct CallFailure {
    detail: String,
    /// "If present, indicates that an error occurred during function execution"
    /// (`InvocationResponse$FunctionError`) — `None` for a call that failed without the
    /// resource naming an error of its own.
    function_error: Option<String>,
}

impl From<String> for CallFailure {
    fn from(detail: String) -> Self {
        Self {
            detail,
            function_error: None,
        }
    }
}

/// Hand the whole polled batch to the enrichment as ONE array, and read the events it
/// returned.
///
/// What comes back drives the target stage: an array is that many events, a single JSON
/// value is one event, and nothing at all is NO event — which suppresses the target
/// invocation for this batch.
async fn invoke_enrichment(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    enrichment: &Enrichment,
    batch: &[Value],
) -> Result<Vec<Value>, CallFailure> {
    let payload = Value::Array(batch.to_vec()).to_string();
    match enrichment {
        Enrichment::LambdaFunction { arn, region } => {
            let answer = clients
                .lambda(callback, region)
                .invoke()
                .function_name(arn)
                .payload(::aws_sdk_lambda::primitives::Blob::new(payload.into_bytes()))
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "the enrichment {arn} could not be invoked ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            // "If present, indicates that an error occurred during function execution"
            // (`InvocationResponse$FunctionError`): the batch is not enriched, so
            // nothing is delivered and the records stay on the source.
            if let Some(failure) = answer.function_error.as_deref() {
                return Err(CallFailure {
                    detail: format!("the enrichment {arn} answered a {failure} error"),
                    function_error: Some(failure.to_string()),
                });
            }
            let returned = answer
                .payload
                .map(::aws_sdk_lambda::primitives::Blob::into_inner)
                .unwrap_or_default();
            enrichment_events(&returned, arn)
        }
        Enrichment::ExpressWorkflow { arn, region } => {
            let answer = clients
                .sfn(callback, region)
                .start_sync_execution()
                .state_machine_arn(arn)
                .input(payload)
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "the enrichment {arn} could not be run ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            if answer.status != ::aws_sdk_sfn::types::SyncExecutionStatus::Succeeded {
                return Err(CallFailure {
                    detail: format!("the enrichment {arn} ended {}", answer.status.as_str()),
                    function_error: Some(answer.status.as_str().to_string()),
                });
            }
            let returned = answer.output.unwrap_or_default();
            enrichment_events(returned.as_bytes(), arn)
        }
    }
}

/// The events an enrichment's answer carries: the elements of a returned array, the one
/// value a returned object or scalar is, or none when it returned nothing.
fn enrichment_events(returned: &[u8], arn: &str) -> Result<Vec<Value>, CallFailure> {
    let trimmed: &[u8] = {
        let text = returned;
        let start = text.iter().position(|byte| !byte.is_ascii_whitespace());
        match start {
            None => &[],
            Some(start) => &text[start..],
        }
    };
    if trimmed.is_empty() {
        return Ok(Vec::new());
    }
    let value: Value = ::serde_json::from_slice(trimmed).map_err(|error| CallFailure {
        detail: format!("the enrichment {arn} answered a body that is not JSON ({error})"),
        function_error: None,
    })?;
    Ok(match value {
        Value::Null => Vec::new(),
        Value::Array(events) => events,
        single => vec![single],
    })
}

/// Deliver the batch to the target, and answer what the target reported about the records
/// of the batch it was handed.
///
/// Each entry of `payloads` is what the target RECEIVES for one event — the event, or the
/// event rendered through the target's input template — beside the event its target
/// parameters are read from, as a path reads it ([`path_view`]). `records` are the source
/// records the batch was polled from, by the identifier their source names each of them by:
/// what a report of the records the target did not process names them by.
///
/// How many calls that comes to is the TARGET's own: `SendMessage` takes one message body
/// per call and `PipeTargetSqsQueueParameters` carries no batch member, so a queue target
/// receives one call per event and never one message holding the array; `Invoke` takes one
/// payload, which is the batch itself, so a function target receives ONE call carrying
/// every event — which is what makes the source's `BatchSize` the count of invocations a
/// backlog comes to.
///
/// A target whose call ANSWERS the caller a document of its own is the one that can report:
/// a function invoked synchronously answers what it returned, which is read as the report of
/// the records it did not process ([`::service_common::batch_item_failures`]). Every other
/// delivery says nothing about the records, so its batch counts as wholly processed — an
/// Amazon SQS `SendMessage` answers nothing about the event it took, and an asynchronous
/// invocation has only been accepted at the point it answers.
async fn deliver_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    target: &Target,
    payloads: &[Delivery<'_>],
    records: &[String],
) -> Result<::service_common::batch_item_failures::BatchReport, CallFailure> {
    match target {
        Target::Queue {
            url,
            region,
            message_group_id,
            message_deduplication_id,
        } => {
            for (payload, event) in payloads {
                // Each parameter is read for THIS event, so one that is a path into the
                // event contributes the record's own value rather than one constant for
                // every record — which is what lets a FIFO target keep a group or a
                // deduplication token per record.
                let group = resolved_parameter(
                    MESSAGE_GROUP_ID,
                    message_group_id.as_deref(),
                    event.as_ref(),
                )?;
                let deduplication = resolved_parameter(
                    MESSAGE_DEDUPLICATION_ID,
                    message_deduplication_id.as_deref(),
                    event.as_ref(),
                )?;
                let mut request = clients
                    .sqs(callback, region)
                    .send_message()
                    .queue_url(url)
                    .message_body(target_body(payload.as_ref()));
                if let Some(group) = group {
                    request = request.message_group_id(group);
                }
                if let Some(deduplication) = deduplication {
                    request = request.message_deduplication_id(deduplication);
                }
                request.send().await.map_err(|error| {
                    format!(
                        "the target queue {url} did not take the event ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            }
            Ok(::service_common::batch_item_failures::BatchReport::WholeBatchProcessed)
        }
        Target::EventBus(bus) => {
            // `PutEvents` answers one result per ENTRY — the event it published, not the
            // source record it was derived from — so it reports nothing about the records of
            // the batch: a call every entry of which was taken is the whole batch processed,
            // and one entry the bus refused fails the attempt ([`event_bus::publish`]).
            event_bus::publish(callback, clients, bus, payloads).await?;
            Ok(::service_common::batch_item_failures::BatchReport::WholeBatchProcessed)
        }
        Target::Function {
            arn,
            region,
            synchronous,
        } => {
            // The events as ONE array, serialized from the payloads themselves rather than
            // from a second copy of them.
            let events: Vec<&Value> = payloads.iter().map(|(payload, _)| payload.as_ref()).collect();
            let batch = ::serde_json::to_string(&events)
                .map_err(|error| format!("the batch for the target {arn} is not JSON ({error})"))?;
            // How big a payload ONE invocation carries is the INVOKED service's own quota,
            // enforced where it is published: a batch past it is refused by the `Invoke`
            // below, which fails the target stage and leaves the records on the source,
            // exactly as any other refusal of that call does.
            let answer = clients
                .lambda(callback, region)
                .invoke()
                .function_name(arn)
                .invocation_type(if *synchronous {
                    ::aws_sdk_lambda::types::InvocationType::RequestResponse
                } else {
                    ::aws_sdk_lambda::types::InvocationType::Event
                })
                .payload(::aws_sdk_lambda::primitives::Blob::new(batch.into_bytes()))
                .send()
                .await
                .map_err(|error| {
                    format!(
                        "the target {arn} could not be invoked ({})",
                        ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                    )
                })?;
            // "If present, indicates that an error occurred during function execution"
            // (`InvocationResponse$FunctionError`): the batch was not taken, so nothing of
            // it is deleted from the source and the pipe hands it over again. An
            // asynchronous invocation reports none — Lambda has only accepted the event at
            // that point — so what such a target did with the batch is its own business.
            if let Some(failure) = answer.function_error.as_deref() {
                return Err(CallFailure {
                    detail: format!("the target {arn} answered a {failure} error"),
                    function_error: Some(failure.to_string()),
                });
            }
            if !*synchronous {
                return Ok(
                    ::service_common::batch_item_failures::BatchReport::WholeBatchProcessed,
                );
            }
            // What the function RETURNED, read as its report of the records of the batch it
            // did not process: a response naming none is the whole batch processed, which is
            // what a handler that knows nothing of the report answers.
            Ok(::service_common::batch_item_failures::read_batch_item_failures(
                answer.payload.as_ref().map(::aws_sdk_lambda::primitives::Blob::as_ref),
                records,
            ))
        }
    }
}

/// The value the target parameter `member` contributes to ONE delivery, or `None` when
/// the pipe configures none.
///
/// A configured value is used AS GIVEN unless it is a JSON path into the event
/// ([`DYNAMIC_PATH_PREFIX`]), which is resolved against the event being delivered through
/// the one bounded reference-path evaluation of this tree
/// (`service_common::json_reference_path`): the one value it names, as the text a message
/// member carries. A path that names no one value of the event, or names something that
/// is not a single value, is a FAILED delivery rather than a message carrying the path
/// itself — the record stays on the source and the pipe tries again, exactly as for a
/// target that refused the event.
fn resolved_parameter(
    member: &str,
    configured: Option<&str>,
    event: &Value,
) -> Result<Option<String>, String> {
    let Some(configured) = configured else {
        return Ok(None);
    };
    if !configured.starts_with(DYNAMIC_PATH_PREFIX) {
        return Ok(Some(configured.to_string()));
    }
    let selected = ::service_common::json_reference_path::select(configured, event).ok_or_else(
        || format!("the target parameter {member} {configured} names no one value of the event"),
    )?;
    match selected {
        Value::String(text) => Ok(Some(text.clone())),
        Value::Number(number) => Ok(Some(number.to_string())),
        Value::Bool(flag) => Ok(Some(flag.to_string())),
        other => Err(format!(
            "the target parameter {member} {configured} names a {}, which is not a value a \
             message carries",
            kind_of(other)
        )),
    }
}

/// What KIND of value a path named, for a failure that must not copy the event into its
/// own text.
fn kind_of(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

/// One event as a target message body: the text of a string event, the JSON of any
/// other.
fn target_body(event: &Value) -> String {
    match event {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// Every event of `batch` rendered through `template`, or the batch itself when the
/// stage carries no template.
///
/// Each render reads the event through its PATHS ([`path_view`]), so a placeholder naming
/// a member of a valid-JSON message body renders that member; a stage handed the batch
/// itself is handed the envelopes with their bodies as sent.
fn transformed(template: Option<&str>, batch: Vec<Value>) -> Result<Vec<Value>, String> {
    let Some(template) = template else {
        return Ok(batch);
    };
    batch
        .iter()
        .map(|event| render_template(template, path_view(event).as_ref()))
        .collect()
}

/// One event rendered through an `InputTemplate`: what the stage the template belongs to
/// is handed INSTEAD of the event.
///
/// "Valid JSON text passed to the target. In this case, nothing from the event itself is
/// passed to the target" (`PipeTargetParameters$InputTemplate`, and the same sentence for
/// the enrichment's) — so the template, not the record, is the payload, and the only parts
/// of the event that reach the stage are the ones the template reads.
///
/// A template is text carrying placeholders — a JSON path between
/// [`PLACEHOLDER_OPEN`] and [`PLACEHOLDER_CLOSE`] — each standing for the value that path
/// selects in the event. A template with no placeholder is a constant, which is what a
/// stage handed the same payload for every event is configured with.
///
/// The rendering is the one a template is WRITTEN for: a placeholder contributes the JSON
/// of the value it selected, so a template that is JSON text renders to JSON (a
/// placeholder inside a quoted string contributes the string's own text, the quotes of
/// the template doing the quoting), and a template that is not JSON text renders to the
/// text it spells out, each placeholder contributing its value's own text. A path that
/// selects no one value of the event is a FAILED render — the record stays on the source
/// — rather than a payload carrying the path itself.
///
/// The text is walked ONCE, left to right, and its length is bounded
/// ([`MAX_TEMPLATE_BYTES`]), so neither the walk nor what it builds is unbounded; what
/// the placeholders ADD is bounded by [`MAX_RENDERED_PAYLOAD`], because a template may
/// name the same value many times. The JSON it renders to is read by `serde_json` and
/// each path by `service_common::json_reference_path`, not by anything spelled out here.
fn render_template(template: &str, event: &Value) -> Result<Value, String> {
    if template.len() > MAX_TEMPLATE_BYTES {
        return Err(format!(
            "the input template is {} bytes, past the {MAX_TEMPLATE_BYTES} a template carries",
            template.len()
        ));
    }
    let placeholders = placeholders_of(template);
    if placeholders.is_empty() {
        // A constant template: the JSON it is, or the text it is.
        return Ok(as_json_or_text(template));
    }
    let mut as_json = String::with_capacity(template.len());
    let mut as_text = String::with_capacity(template.len());
    let mut taken = 0usize;
    for (span, path) in &placeholders {
        let selected = selected_value(path, event)?;
        // Inside a quoted string the template supplies the quotes, so the placeholder
        // contributes the value's text escaped as a JSON string body; anywhere else it
        // contributes the whole JSON of the value.
        let text = value_text(selected);
        let json = if inside_a_json_string(template, span.start) {
            json_string_body(&text)
        } else {
            selected.to_string()
        };
        let before = &template[taken..span.start];
        // Checked BEFORE either buffer grows, so what the render holds never passes the
        // bound rather than passing it and being refused afterwards.
        within_the_payload_bound(as_json.len() + before.len() + json.len())?;
        within_the_payload_bound(as_text.len() + before.len() + text.len())?;
        as_json.push_str(before);
        as_json.push_str(&json);
        as_text.push_str(before);
        as_text.push_str(&text);
        taken = span.end;
    }
    let rest = &template[taken..];
    within_the_payload_bound(as_json.len().max(as_text.len()) + rest.len())?;
    as_json.push_str(rest);
    as_text.push_str(rest);
    Ok(match ::serde_json::from_str::<Value>(&as_json) {
        Ok(value) => value,
        // Not JSON text: the template spells out text, and the text it spells out is what
        // the stage is handed.
        Err(_) => Value::String(as_text),
    })
}

/// Whether a render of `bytes` is one this emulator delivers, or why it is not.
fn within_the_payload_bound(bytes: usize) -> Result<(), String> {
    if bytes > MAX_RENDERED_PAYLOAD {
        return Err(format!(
            "the input template renders more than {MAX_RENDERED_PAYLOAD} bytes for one event, \
             which is more than a payload this emulator delivers"
        ));
    }
    Ok(())
}

/// A template with no placeholder, as the payload it is: the JSON value it is the text
/// of, or the text itself.
fn as_json_or_text(template: &str) -> Value {
    ::serde_json::from_str::<Value>(template).unwrap_or_else(|_| Value::String(template.to_string()))
}

/// The placeholders of `template`, in the order they appear: where each one sits and the
/// JSON path it carries.
///
/// Only a `<…>` span whose content is a JSON path ([`DYNAMIC_PATH_PREFIX`], or the event
/// itself) is a placeholder, so a template carrying `<` for any other reason is left
/// alone — and the scan then resumes just PAST that `<`, so a stray one cannot swallow
/// the placeholder behind it.
fn placeholders_of(template: &str) -> Vec<(std::ops::Range<usize>, String)> {
    let mut found: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    let mut rest = template;
    let mut base = 0usize;
    while let Some(open) = rest.find(PLACEHOLDER_OPEN) {
        let after_open = open + PLACEHOLDER_OPEN.len_utf8();
        let Some(close) = rest[after_open..].find(PLACEHOLDER_CLOSE) else {
            break;
        };
        let end = after_open + close;
        let content = rest[after_open..end].trim();
        let consumed = if content == WHOLE_EVENT_PATH || content.starts_with(DYNAMIC_PATH_PREFIX) {
            found.push((
                base + open..base + end + PLACEHOLDER_CLOSE.len_utf8(),
                content.to_string(),
            ));
            end + PLACEHOLDER_CLOSE.len_utf8()
        } else {
            // Not a placeholder: only this `<` is consumed, because the span it opened
            // may hold the real one.
            after_open
        };
        base += consumed;
        rest = &rest[consumed..];
    }
    found
}

/// Whether the placeholder that starts at `upto` sits INSIDE a quoted string of the
/// template, so the template's own quotes are what quote the value.
///
/// Decided by walking the text before it and counting the quotes that open and close a
/// JSON string (a backslash escapes the next character inside one), so a placeholder with
/// text around it inside the same string is read as quoted too.
fn inside_a_json_string(template: &str, upto: usize) -> bool {
    let mut quoted = false;
    let mut escaped = false;
    for character in template[..upto].chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            JSON_STRING_ESCAPE if quoted => escaped = true,
            JSON_STRING_QUOTE_CHARACTER => quoted = !quoted,
            _ => {}
        }
    }
    quoted
}

/// The ONE value `path` names in `event`, or why the render fails.
///
/// Read through `service_common::json_reference_path`, the one bounded evaluation of a
/// caller-supplied path in this tree, so a template cannot ask for a walk of the whole
/// event.
fn selected_value<'a>(path: &str, event: &'a Value) -> Result<&'a Value, String> {
    ::service_common::json_reference_path::select(path, event)
        .ok_or_else(|| format!("the input template's {path} names no one value of the event"))
}

/// A selected value as its own TEXT: the contents of a string, the JSON of anything else
/// (a number, a boolean, an object or an array has no text but its JSON).
fn value_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// `text` as the BODY of a JSON string — its JSON encoding without the surrounding
/// quotes — so a template's own quotes quote it, whatever characters it carries.
fn json_string_body(text: &str) -> String {
    let encoded = Value::String(text.to_string()).to_string();
    encoded
        .strip_prefix(JSON_STRING_QUOTE)
        .and_then(|rest| rest.strip_suffix(JSON_STRING_QUOTE))
        .unwrap_or(&encoded)
        .to_string()
}

/// A pipe this data plane serves, as the stages above read it: what the poller moves for
/// one pipe, resolved from the stored record.
///
/// It carries no `RoleArn`: the identity each stage's call is made as is read from the
/// record when that call is made ([`LiveRecord::refusal`]), not resolved with the rest of
/// the configuration.
///
/// One of these is resolved for the POLL of a pass ([`next_pass`]) and one for the DECISION
/// of the batch that poll came back with ([`LiveRecord::resolved_now`]), so what the stages
/// act under is never older than the batch they are handed.
struct RunningPipe {
    /// The pipe's own ARN: what a record of a batch this pipe gave up on names it by.
    pipe_arn: String,
    /// The source the records are polled from, by the kind of resource the `Source`
    /// names.
    source: Source,
    /// The `Source` itself, which every record envelope reports as its event source.
    source_arn: String,
    /// The Region the source ARN names — the one its records are polled in and report.
    source_region: String,
    /// `BatchSize`: the most records one batch holds.
    batch_size: usize,
    /// `MaximumBatchingWindowInSeconds`, as a span.
    batching_window: Duration,
    /// `FilterCriteria`'s patterns, parsed: a record matching any of them enters the
    /// pipe, and an empty list admits every record.
    filters: Vec<Value>,
    /// The enrichment the batch is handed to, when the pipe has one.
    enrichment: Option<Enrichment>,
    /// `EnrichmentParameters.InputTemplate`: what the ENRICHMENT is handed instead of the
    /// record, rendered per record. Absent when the pipe configures none.
    enrichment_template: Option<String>,
    /// `TargetParameters.InputTemplate`: what the TARGET is handed instead of the event,
    /// rendered per event. Absent when the pipe configures none.
    target_template: Option<String>,
    target: Target,
    /// The `Target` itself: the resource the delivery is authorized on.
    target_arn: String,
    /// `MaximumRetryAttempts`: how many attempts BEYOND the first a batch the pipe cannot
    /// move gets before it is discarded. `None` is infinite — "the default value is -1,
    /// which sets the maximum number of retries to infinite" — which is also what a source
    /// whose parameters carry no such member at all comes to.
    maximum_retry_attempts: Option<u32>,
    /// `DeadLetterConfig`: where the record of a batch this pipe discarded goes, when the
    /// pipe configures a destination for it.
    dead_letter: Option<dead_letter::Destination>,
}

/// The source a pipe's records are polled from, by the resource its `Source` names.
#[derive(PartialEq, Eq)]
enum Source {
    /// An Amazon SQS queue, received from at the url its ARN addresses.
    Queue { url: String },
    /// A DynamoDB stream, read shard by shard from the position `StartingPosition`
    /// names.
    Stream { starting_position: String },
    /// A Kinesis data stream, read shard by shard from the position `StartingPosition`
    /// names — and, where that position is a stated instant, from the one
    /// `StartingPositionTimestamp` carries ([`kinesis_source`]).
    KinesisStream {
        starting_position: String,
        starting_position_timestamp: Option<SystemTime>,
    },
}

/// The enrichment a batch is handed to, by the resource its ARN names.
#[derive(PartialEq, Eq)]
enum Enrichment {
    /// A Lambda function, invoked with the batch as its payload.
    LambdaFunction { arn: String, region: String },
    /// A Step Functions EXPRESS workflow, run synchronously with the batch as its
    /// input: "You can only use Express workflows as an enrichment", and a synchronous
    /// run is what answers the enriched events.
    ExpressWorkflow { arn: String, region: String },
}

impl Enrichment {
    /// The resource the enrichment stage's call is made on.
    fn arn(&self) -> &str {
        match self {
            Self::LambdaFunction { arn, .. } | Self::ExpressWorkflow { arn, .. } => arn,
        }
    }

    /// The IAM action the enrichment stage's call is authorized under: the API call this
    /// kind of enrichment is invoked by.
    fn invocation_action(&self) -> &'static str {
        match self {
            Self::LambdaFunction { .. } => INVOKE_FUNCTION_ACTION,
            Self::ExpressWorkflow { .. } => START_SYNC_EXECUTION_ACTION,
        }
    }
}

/// The target an event is delivered to, by the resource its ARN names.
#[derive(PartialEq, Eq)]
enum Target {
    /// An Amazon SQS queue: one `SendMessage` per event.
    Queue {
        url: String,
        region: String,
        /// `MessageGroupId` and `MessageDeduplicationId` AS THE PIPE CONFIGURES THEM —
        /// each a literal value or a JSON path into the event, read for every delivery by
        /// [`resolved_parameter`] rather than fixed when the pipe is resolved.
        message_group_id: Option<String>,
        message_deduplication_id: Option<String>,
    },
    /// An EventBridge event bus: the events are PUT on it, up to ten per call
    /// ([`event_bus`]).
    EventBus(event_bus::Bus),
    /// A Lambda function: ONE `Invoke` carrying the whole batch.
    Function {
        arn: String,
        region: String,
        /// Whether that invocation is synchronous. `InvocationType` is "whether to invoke
        /// the function synchronously or asynchronously", `REQUEST_RESPONSE` (the default)
        /// corresponding to the `RequestResponse` of Lambda's own `Invoke` and
        /// `FIRE_AND_FORGET` to its `Event`.
        synchronous: bool,
    },
}

impl Target {
    /// The IAM action the target stage's call is authorized under: the API call this kind
    /// of target takes its events by.
    fn delivery_action(&self) -> &'static str {
        match self {
            Self::Queue { .. } => SEND_MESSAGE_ACTION,
            Self::EventBus(_) => event_bus::PUT_EVENTS_ACTION,
            Self::Function { .. } => INVOKE_FUNCTION_ACTION,
        }
    }
}

/// What the SOURCE half of a pipe's configuration comes to: which source is polled and the
/// members that source's own parameter shape carries.
///
/// One shape per source kind carries them (`PipeSourceSqsQueueParameters`,
/// `PipeSourceDynamoDBStreamParameters`, `PipeSourceKinesisStreamParameters`) and they do not
/// carry the same members — only a stream's parameters say how often a batch is retried and
/// where it goes when it is given up on — so this is what each arm of the resolution answers,
/// read the same way whichever arm filled it.
struct SourceConfiguration {
    source: Source,
    /// `BatchSize` as the pipe configured it, and the default for a source that named none.
    batch_size: Option<i32>,
    /// See [`Self::batch_size`].
    default_batch_size: usize,
    /// `MaximumBatchingWindowInSeconds` as the pipe configured it.
    window_seconds: Option<i32>,
    /// `MaximumRetryAttempts`, for a source whose parameters carry one.
    retry_attempts: Option<i32>,
    /// `DeadLetterConfig.Arn`, for a source whose parameters carry one.
    dead_letter_arn: Option<String>,
}

/// What this record asks the stages to do, or why this data plane does not serve it.
///
/// A configuration it cannot serve FAITHFULLY leaves the source untouched rather than
/// delivering something the pipe did not ask for: a source or target of a kind no stage
/// here addresses, or a filter pattern the event-pattern language does not admit.
fn resolve_run(pipe: &PipeRecord) -> Result<RunningPipe, String> {
    let parsed = ::service_common::arn::parse(&pipe.source)
        .ok_or_else(|| format!("its Source {} is not a resource ARN", pipe.source))?;
    let source_region = parsed.region.to_string();
    let parameters = pipe.source_parameters.as_ref();

    // WHICH source is polled, and the parameters that source's own shape carries: a pipe
    // reads from exactly one source, which is the kind of resource its `Source` names.
    let SourceConfiguration {
        source,
        batch_size,
        window_seconds,
        default_batch_size,
        retry_attempts,
        dead_letter_arn,
    } = match parsed.vendor {
        SQS_NAMESPACE => {
            let queue_name = parsed.resource;
            if queue_name.contains('/') || queue_name.contains(':') {
                return Err(format!("its Source {} names no queue", pipe.source));
            }
            let queue = parameters.and_then(|p| p.sqs_queue_parameters.as_ref());
            SourceConfiguration {
                source: Source::Queue {
                    url: queue_url(&source_region, parsed.account, queue_name),
                },
                batch_size: queue.and_then(|p| p.batch_size),
                window_seconds: queue.and_then(|p| p.maximum_batching_window_in_seconds),
                default_batch_size: DEFAULT_BATCH_SIZE,
                // `PipeSourceSqsQueueParameters` carries neither member: a queue's records
                // are retried for as long as the queue holds them, and there is no
                // dead-letter destination of the PIPE's — the queue has one of its own.
                retry_attempts: None,
                dead_letter_arn: None,
            }
        }
        DYNAMODB_NAMESPACE => {
            // A DynamoDB source is the table's STREAM, which is the resource the ARN has
            // to name: `table/<name>/stream/<label>`.
            if parsed.resource_type() != TABLE_RESOURCE
                || !parsed.resource.contains(STREAM_SEGMENT)
            {
                return Err(format!(
                    "its Source {} names no DynamoDB stream",
                    pipe.source
                ));
            }
            let stream = parameters
                .and_then(|p| p.dynamo_db_stream_parameters.as_ref())
                .ok_or_else(|| {
                    "it carries no DynamoDBStreamParameters, so there is no position to \
                     read its stream from"
                        .to_string()
                })?;
            SourceConfiguration {
                source: Source::Stream {
                    starting_position: stream.starting_position.clone(),
                },
                batch_size: stream.batch_size,
                window_seconds: stream.maximum_batching_window_in_seconds,
                default_batch_size: DEFAULT_STREAM_BATCH_SIZE,
                retry_attempts: stream.maximum_retry_attempts,
                dead_letter_arn: stream
                    .dead_letter_config
                    .as_ref()
                    .and_then(|configured| configured.arn.clone()),
            }
        }
        KINESIS_NAMESPACE => {
            // A Kinesis source is the data stream itself, which is the resource the ARN
            // has to name: `stream/<name>`.
            if parsed.resource_type() != KINESIS_STREAM_RESOURCE {
                return Err(format!(
                    "its Source {} names no Kinesis stream",
                    pipe.source
                ));
            }
            let stream = parameters
                .and_then(|p| p.kinesis_stream_parameters.as_ref())
                .ok_or_else(|| {
                    "it carries no KinesisStreamParameters, so there is no position to \
                     read its stream from"
                        .to_string()
                })?;
            SourceConfiguration {
                source: Source::KinesisStream {
                    starting_position: stream.starting_position.clone(),
                    // "The time from which to start reading, in Unix time seconds"
                    // (`StartingPositionTimestamp`): the instant itself, read through the
                    // one conversion the SDK's timestamp type offers.
                    starting_position_timestamp: stream
                        .starting_position_timestamp
                        .and_then(|at| SystemTime::try_from(at).ok()),
                },
                batch_size: stream.batch_size,
                window_seconds: stream.maximum_batching_window_in_seconds,
                default_batch_size: DEFAULT_STREAM_BATCH_SIZE,
                retry_attempts: stream.maximum_retry_attempts,
                dead_letter_arn: stream
                    .dead_letter_config
                    .as_ref()
                    .and_then(|configured| configured.arn.clone()),
            }
        }
        other => {
            return Err(format!(
                "its Source names a {other} resource, and only an Amazon SQS queue, a \
                 DynamoDB stream or a Kinesis stream source is polled by this emulator"
            ))
        }
    };
    let batch_size = batch_size
        .and_then(|size| usize::try_from(size).ok())
        .unwrap_or(default_batch_size)
        .clamp(*BATCH_SIZE_RANGE.start(), *BATCH_SIZE_RANGE.end())
        // Clamped BEFORE anything is collected, so the records one batch holds in
        // memory are bounded by this emulator rather than by the request.
        .min(MAX_RECORDS_PER_BATCH);
    let window_seconds = window_seconds
        .unwrap_or(0)
        .clamp(*BATCHING_WINDOW_RANGE.start(), *BATCHING_WINDOW_RANGE.end());
    let batching_window = Duration::from_secs(u64::try_from(window_seconds).unwrap_or_default());

    // "Discard records after the specified number of retries. The default value is -1, which
    // sets the maximum number of retries to infinite" (`MaximumRetryAttempts` of either
    // stream source's parameters): the negative value, and a source whose parameters name
    // none, are a batch attempted for as long as its source holds it.
    let maximum_retry_attempts = retry_attempts.and_then(|allowed| u32::try_from(allowed).ok());
    // Where the record of a batch this pipe gives up on goes. A destination this data plane
    // cannot write to is a configuration it does not serve: the pipe polls nothing rather
    // than discarding batches whose record would reach nobody.
    let dead_letter = match dead_letter_arn
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        None => None,
        Some(arn) => Some(dead_letter::resolve(arn)?),
    };

    // The filters the pipe admits records by, read through the one reading of the
    // event-pattern language in this tree. An absent or empty pattern carries no pattern
    // at all, and `FilterCriteria` with an empty `Filters` array is how a caller REMOVES
    // filtering.
    let mut filters = Vec::new();
    for filter in parameters
        .and_then(|p| p.filter_criteria.as_ref())
        .and_then(|criteria| criteria.filters.as_ref())
        .into_iter()
        .flatten()
    {
        let pattern = match filter.pattern.as_deref() {
            None | Some("") => continue,
            Some(pattern) => pattern,
        };
        filters.push(
            ::service_common::event_pattern::parse(pattern)
                .map_err(|reason| format!("it carries a filter pattern this emulator cannot match ({reason})"))?,
        );
    }

    let enrichment = match pipe
        .enrichment
        .as_deref()
        .filter(|arn| !arn.is_empty())
    {
        None => None,
        Some(arn) => Some(resolve_enrichment(arn)?),
    };
    // A template is what the stage it belongs to is handed instead of the event; an empty
    // one configures nothing.
    let enrichment_template = pipe
        .enrichment_parameters
        .as_ref()
        .and_then(|parameters| parameters.input_template.clone())
        .filter(|template| !template.is_empty());

    let target_parameters = pipe.target_parameters.as_ref();
    let target_template = target_parameters
        .and_then(|parameters| parameters.input_template.clone())
        .filter(|template| !template.is_empty());
    let target = resolve_target(&pipe.target, target_parameters)?;

    Ok(RunningPipe {
        pipe_arn: pipe.arn.clone(),
        source,
        source_arn: pipe.source.clone(),
        source_region,
        batch_size,
        batching_window,
        filters,
        enrichment,
        enrichment_template,
        target_template,
        target,
        target_arn: pipe.target.clone(),
        maximum_retry_attempts,
        dead_letter,
    })
}

/// The enrichment the `Enrichment` ARN names, or why this data plane does not invoke it.
fn resolve_enrichment(arn: &str) -> Result<Enrichment, String> {
    let parsed = ::service_common::arn::parse(arn)
        .ok_or_else(|| format!("its Enrichment {arn} is not a resource ARN"))?;
    let region = parsed.region.to_string();
    match (parsed.vendor, parsed.resource_type()) {
        (LAMBDA_NAMESPACE, FUNCTION_RESOURCE) => Ok(Enrichment::LambdaFunction {
            arn: arn.to_string(),
            region,
        }),
        (STATES_NAMESPACE, STATE_MACHINE_RESOURCE) => Ok(Enrichment::ExpressWorkflow {
            arn: arn.to_string(),
            region,
        }),
        _ => Err(format!(
            "its Enrichment {arn} names a resource this emulator's enrichment stage does not \
             invoke"
        )),
    }
}

/// The target the `Target` ARN names, or why this data plane does not deliver to it.
fn resolve_target(
    arn: &str,
    parameters: Option<&PipeTargetParameters>,
) -> Result<Target, String> {
    let parsed = ::service_common::arn::parse(arn)
        .ok_or_else(|| format!("its Target {arn} is not a resource ARN"))?;
    match (parsed.vendor, parsed.resource_type()) {
        (SQS_NAMESPACE, _) => {
            let name = parsed.resource;
            if name.contains('/') || name.contains(':') {
                return Err(format!("its Target {arn} names no queue"));
            }
            let queue_parameters = parameters.and_then(|p| p.sqs_queue_parameters.as_ref());
            Ok(Target::Queue {
                url: queue_url(parsed.region, parsed.account, name),
                region: parsed.region.to_string(),
                message_group_id: queue_parameters
                    .and_then(|parameters| parameters.message_group_id.clone())
                    .filter(|group| !group.is_empty()),
                message_deduplication_id: queue_parameters
                    .and_then(|parameters| parameters.message_deduplication_id.clone())
                    .filter(|token| !token.is_empty()),
            })
        }
        (EVENTS_NAMESPACE, event_bus::EVENT_BUS_RESOURCE) => Ok(Target::EventBus(
            event_bus::resolve(
                arn,
                parsed.region,
                parameters.and_then(|p| p.event_bridge_event_bus_parameters.as_ref()),
            ),
        )),
        (LAMBDA_NAMESPACE, FUNCTION_RESOURCE) => Ok(Target::Function {
            arn: arn.to_string(),
            region: parsed.region.to_string(),
            // `REQUEST_RESPONSE` is the default the member documents, so a pipe that
            // configures no invocation type is invoked synchronously.
            synchronous: parameters
                .and_then(|p| p.lambda_function_parameters.as_ref())
                .and_then(|parameters| parameters.invocation_type.as_deref())
                .is_none_or(|configured| configured != FIRE_AND_FORGET_INVOCATION),
        }),
        (vendor, _) => Err(format!(
            "its Target names a {vendor} resource, and only an Amazon SQS queue, an \
             EventBridge event bus or a Lambda function target is delivered to by this \
             emulator"
        )),
    }
}

/// The IAM action of the call that TAKES THE RECORDS from a source of the kind `source`
/// names — the one call the source stage cannot poll that source without — or `None` for a
/// source this data plane takes no records from at all.
///
/// The source stage makes other calls beside it (it reads a queue's attributes, it lists a
/// stream's shards), and each is authorized where it is made ([`LiveRecord::refusal`]); this
/// is the single call whose refusal means the pipe could never poll, which is what the
/// lifecycle asks about before it reports a pipe up ([`super::bring_up`]).
pub(super) fn source_record_action(source: &str) -> Option<&'static str> {
    let parsed = ::service_common::arn::parse(source)?;
    match parsed.vendor {
        SQS_NAMESPACE => Some(RECEIVE_MESSAGE_ACTION),
        DYNAMODB_NAMESPACE if parsed.resource.contains(STREAM_SEGMENT) => Some(GET_RECORDS_ACTION),
        KINESIS_NAMESPACE if parsed.resource_type() == KINESIS_STREAM_RESOURCE => {
            Some(kinesis_source::GET_RECORDS_ACTION)
        }
        _ => None,
    }
}

/// Whether the service that OWNS the source `source` names answers that it holds no such
/// resource.
///
/// The resource is asked for through that service's own read — the queue's attributes, the
/// stream's description — so a source is reported absent only where the component that owns
/// it says so. `false` for everything this emulator cannot show: a source whose kind no
/// owner here is asked (only an Amazon SQS queue, a DynamoDB stream and a Kinesis stream
/// are), a source of another account, a service that answered something else, and one that
/// did not answer.
pub(super) async fn source_is_absent(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    source: &str,
) -> bool {
    let Some(parsed) = ::service_common::arn::parse(source) else {
        return false;
    };
    // A resource of another account is not this emulator's to hold, so its absence here
    // shows nothing about it.
    if !parsed.account.is_empty() && parsed.account != ::engine::emulator_account_id() {
        return false;
    }
    let mut clients = Clients::default();
    match parsed.vendor {
        SQS_NAMESPACE => {
            let name = parsed.resource;
            if name.contains('/') || name.contains(':') {
                return false;
            }
            let url = queue_url(parsed.region, parsed.account, name);
            let answered = clients
                .sqs(callback, parsed.region)
                .get_queue_attributes()
                .queue_url(url)
                .attribute_names(::aws_sdk_sqs::types::QueueAttributeName::QueueArn)
                .send()
                .await;
            answered.err().is_some_and(|error| {
                error
                    .as_service_error()
                    .is_some_and(::aws_sdk_sqs::operation::get_queue_attributes::GetQueueAttributesError::is_queue_does_not_exist)
            })
        }
        DYNAMODB_NAMESPACE if parsed.resource.contains(STREAM_SEGMENT) => {
            let answered = clients
                .dynamodbstreams(callback, parsed.region)
                .describe_stream()
                .stream_arn(source)
                .send()
                .await;
            answered.err().is_some_and(|error| {
                error
                    .as_service_error()
                    .is_some_and(::aws_sdk_dynamodbstreams::operation::describe_stream::DescribeStreamError::is_resource_not_found_exception)
            })
        }
        KINESIS_NAMESPACE if parsed.resource_type() == KINESIS_STREAM_RESOURCE => {
            let answered = clients
                .kinesis(callback, parsed.region)
                .describe_stream_summary()
                .stream_arn(source)
                .send()
                .await;
            answered.err().is_some_and(|error| {
                error
                    .as_service_error()
                    .is_some_and(::aws_sdk_kinesis::operation::describe_stream_summary::DescribeStreamSummaryError::is_resource_not_found_exception)
            })
        }
        _ => false,
    }
}

/// The url Amazon SQS addresses the queue named `name` by:
/// `<endpoint>/<accountId>/<QueueName>`, the form every SQS operation takes as its
/// `QueueUrl`. An ARN with no account field is this emulator's own account.
fn queue_url(region: &str, account: &str, name: &str) -> String {
    let account = if account.is_empty() {
        ::engine::emulator_account_id().to_string()
    } else {
        account.to_string()
    };
    format!("https://sqs.{region}.amazonaws.com/{account}/{name}")
}

/// The clients the stages reach the other services of this emulator through, built on
/// first use and then reused.
///
/// Each slot remembers the Region its client was built for, because a resource is
/// reached in the Region ITS OWN ARN names: a pipe whose source and target are in two
/// Regions rebuilds the client rather than sending the delivery to the source's Region.
#[derive(Default)]
struct Clients {
    sqs: Option<(String, ::aws_sdk_sqs::Client)>,
    eventbridge: Option<(String, ::aws_sdk_eventbridge::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
    sfn: Option<(String, ::aws_sdk_sfn::Client)>,
    dynamodbstreams: Option<(String, ::aws_sdk_dynamodbstreams::Client)>,
    kinesis: Option<(String, ::aws_sdk_kinesis::Client)>,
}

/// Build each client the same way: over the in-process callback into this same
/// emulator, against the called service's endpoint in `region`.
///
/// The endpoint url names the called service's public host so the request is signed and
/// routed as that service's, but the TRANSPORT is the emulator's own in-process
/// connector ([`smithy_server_runtime::EmbeddedEmulator::http_client`]): no socket is
/// opened and no name is resolved, so a queue, function or workflow a caller named is
/// reached only if this emulator serves it itself.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity
/// caching each need a sleep implementation the callback transport is not given.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        fn $field(
            &mut self,
            callback: &smithy_server_runtime::EmbeddedEmulator,
            region: &str,
        ) -> &::$sdk::Client {
            if self
                .$field
                .as_ref()
                .is_none_or(|(held, _)| held != region)
            {
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
                self.$field = Some((region.to_string(), ::$sdk::Client::from_conf(config)));
            }
            &self
                .$field
                .as_ref()
                .expect("the client for this region was just built")
                .1
        }
    };
}

impl Clients {
    callback_client!(sqs, aws_sdk_sqs, "sqs");
    // EventBridge publishes its endpoint and signs under `events`.
    callback_client!(eventbridge, aws_sdk_eventbridge, "events");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
    // Step Functions publishes its endpoint and signs under `states`.
    callback_client!(sfn, aws_sdk_sfn, "states");
    // DynamoDB Streams publishes its own endpoint under the DynamoDB one
    // (`streams.dynamodb.<region>.amazonaws.com`) and signs as `dynamodb`.
    callback_client!(dynamodbstreams, aws_sdk_dynamodbstreams, "streams.dynamodb");
    callback_client!(kinesis, aws_sdk_kinesis, "kinesis");
}

#[cfg(test)]
mod test_support {
    use super::{Duration, RunningPipe, Source, Target};

    /// A resolved pipe whose source is a Kinesis stream read from `starting_position` and
    /// whose target is an Amazon SQS queue: what the unit tests of the Kinesis source stage
    /// read their configuration off.
    pub(super) fn kinesis_run(starting_position: &str) -> RunningPipe {
        RunningPipe {
            pipe_arn: "arn:aws:pipes:us-east-1:000000000000:pipe/orders-to-results".to_string(),
            source: Source::KinesisStream {
                starting_position: starting_position.to_string(),
                starting_position_timestamp: None,
            },
            source_arn: "arn:aws:kinesis:us-east-1:000000000000:stream/orders".to_string(),
            source_region: "us-east-1".to_string(),
            batch_size: 10,
            batching_window: Duration::ZERO,
            filters: Vec::new(),
            enrichment: None,
            enrichment_template: None,
            target_template: None,
            target: Target::Queue {
                url: "https://sqs.us-east-1.amazonaws.com/000000000000/results".to_string(),
                region: "us-east-1".to_string(),
                message_group_id: None,
                message_deduplication_id: None,
            },
            target_arn: "arn:aws:sqs:us-east-1:000000000000:results".to_string(),
            maximum_retry_attempts: None,
            dead_letter: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        checkpointed_through, enrichment_events, path_view, render_template, resolved_parameter,
        target_body, MAX_TEMPLATE_BYTES, MESSAGE_GROUP_ID,
    };
    use ::serde_json::{json, Value};
    use ::std::collections::BTreeSet;

    /// RULE: a shard has ONE position, so an attempt that processed part of the batch read
    /// from it leaves that shard read on from the FIRST record it did not process: the
    /// position moves through the run of processed records at the HEAD of the batch and no
    /// further, and not at all when the batch's own first record was not processed.
    ///
    /// The rows differ along which records of the batch the attempt processed, which is what
    /// the rule generalises over: all of them, none, a prefix, a suffix, and a prefix with a
    /// gap after it.
    #[test]
    fn a_shard_is_read_on_from_the_first_record_the_attempt_did_not_process() {
        let batch: Vec<String> = ["s-1", "s-2", "s-3", "s-4"]
            .iter()
            .map(|id| (*id).to_string())
            .collect();
        // (case, the records the attempt processed, the position the shard reads on from)
        let rows: [(&str, &[&str], Option<&str>); 6] = [
            ("every record", &["s-1", "s-2", "s-3", "s-4"], Some("s-4")),
            ("no record", &[], None),
            ("the first two", &["s-1", "s-2"], Some("s-2")),
            ("the last two", &["s-3", "s-4"], None),
            ("the first and the third", &["s-1", "s-3"], Some("s-1")),
            ("all but the first", &["s-2", "s-3", "s-4"], None),
        ];
        for (case, processed, expected) in rows {
            let processed: BTreeSet<&str> = processed.iter().copied().collect();
            let through = checkpointed_through(&batch, &processed);
            assert_eq!(
                through, expected,
                "case {case:?}: expected the shard read on from {expected:?}, got {through:?}"
            );
        }
    }

    /// The record envelope the template rows below are rendered against: an Amazon SQS
    /// record as the source stage builds one.
    fn record() -> Value {
        json!({
            "messageId": "4fd2-1",
            "body": "order-7",
            "attributes": { "MessageGroupId": "orders", "SequenceNumber": 7 },
            "eventSource": "aws:sqs",
        })
    }

    /// RULE: an `InputTemplate` is what the stage it belongs to is handed INSTEAD of the
    /// event: each `<$.…>` placeholder contributes the value that path selects in the
    /// event, a template that renders JSON text is handed over as that JSON, and one that
    /// does not is handed over as the text it spells out.
    ///
    /// The rows differ along the form of the template, which is what the rule generalises
    /// over: one bare placeholder, a quoted one, placeholders inside a JSON object, a
    /// template with no placeholder at all, and text around a placeholder.
    #[test]
    fn an_input_template_is_what_the_stage_is_handed() {
        let event = record();
        // (case, the template, what the stage is handed)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("the-body-alone", "<$.body>", json!("order-7")),
            ("the-body-quoted", "\"<$.body>\"", json!("order-7")),
            ("the-whole-event", "<$>", record()),
            (
                "an-object-of-two-values",
                r#"{"order": "<$.body>", "from": "<$.attributes.MessageGroupId>"}"#,
                json!({"order": "order-7", "from": "orders"}),
            ),
            (
                "an-unquoted-number",
                r#"{"seq": <$.attributes.SequenceNumber>}"#,
                json!({"seq": 7}),
            ),
            (
                "a-nested-value",
                r#"{"group": <$.attributes.MessageGroupId>}"#,
                json!({"group": "orders"}),
            ),
            ("a-constant-object", r#"{"fixed": true}"#, json!({"fixed": true})),
            ("a-constant-text", "nothing to read", json!("nothing to read")),
            (
                "text-around-a-placeholder",
                "order <$.body> received",
                json!("order order-7 received"),
            ),
            // Inside a JSON string with text around it: the template's own quotes quote
            // the value, so the render is that string.
            (
                "a-quoted-placeholder-with-text",
                "\"order <$.body> received\"",
                json!("order order-7 received"),
            ),
            (
                "a-quoted-placeholder-with-text-in-an-object",
                r#"{"text": "order <$.body> from <$.attributes.MessageGroupId>"}"#,
                json!({"text": "order order-7 from orders"}),
            ),
        ];
        for (case, template, expected) in rows {
            let rendered = render_template(template, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }

    /// RULE: a template whose placeholder selects no one value of the event renders
    /// nothing — the record stays on the source — rather than a payload carrying the path
    /// itself.
    #[test]
    fn a_template_placeholder_that_selects_nothing_fails_the_render() {
        let event = record();
        // (case, the template)
        let rows: Vec<(&str, &str)> = vec![
            ("absent", "<$.detail>"),
            ("absent-inside-an-object", r#"{"x": <$.attributes.Absent>}"#),
        ];
        for (case, template) in rows {
            let outcome = render_template(template, &event);
            assert!(
                outcome.is_err(),
                "case {case:?}: expected the render to fail, got {outcome:?}"
            );
        }
    }

    /// RULE: what a template renders for ONE event is bounded, however many times it
    /// names a value of that event, and a template longer than one may carry is refused
    /// rather than walked.
    #[test]
    fn a_template_that_renders_too_much_fails_the_render() {
        // A big event, named by as many placeholders as a bounded template holds: each
        // one is three bytes of template and a whole event of payload.
        let event = json!({ "body": "x".repeat(8 * 1024) });
        let template = "<$>".repeat(2 * 1024);
        let outcome = render_template(&template, &event);
        assert!(
            outcome.is_err(),
            "case amplified: expected the render to be refused, got {} byte(s)",
            outcome.map(|value| value.to_string().len()).unwrap_or(0)
        );
        // And the template's own length is a bound of its own.
        let too_long = "a".repeat(MAX_TEMPLATE_BYTES + 1);
        let outcome = render_template(&too_long, &event);
        assert!(
            outcome.is_err(),
            "case past-the-template-bound: expected the render to be refused, got {outcome:?}"
        );
        let at_the_bound = "a".repeat(MAX_TEMPLATE_BYTES);
        let outcome = render_template(&at_the_bound, &event);
        assert!(
            outcome.is_ok(),
            "case at-the-template-bound: expected a template of {MAX_TEMPLATE_BYTES} bytes to \
             render, got {outcome:?}"
        );
    }

    /// RULE: a `<…>` span that is not a JSON path is not a placeholder, so a template may
    /// carry the characters for any other reason and is handed over as written — and a
    /// stray `<` does not swallow the placeholder behind it.
    #[test]
    fn a_span_that_is_not_a_path_is_left_as_written() {
        let event = record();
        // (case, the template, what the stage is handed)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("not-a-path", "a < b and c > d", json!("a < b and c > d")),
            (
                "a-stray-open-before-a-placeholder",
                "a < b <$.body>",
                json!("a < b order-7"),
            ),
            (
                "a-stray-open-inside-an-object",
                r#"{"text": "a < b <$.body>"}"#,
                json!({"text": "a < b order-7"}),
            ),
            // A placeholder nothing closes is not one either: the template is text.
            (
                "an-unterminated-placeholder",
                "<$.body",
                json!("<$.body"),
            ),
        ];
        for (case, template, expected) in rows {
            let rendered = render_template(template, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
    }

    /// RULE: a target parameter's value is used AS CONFIGURED unless it is a JSON path
    /// into the event, in which case the value the path selects in the event is what the
    /// delivery carries.
    ///
    /// The rows differ along the form of the configured value and the type of the node a
    /// path selects, which is what the rule generalises over.
    #[test]
    fn a_target_parameter_is_the_literal_or_what_its_path_selects() {
        let event = json!({
            "messageId": "4fd2-1",
            "body": "{\"n\":1}",
            "attributes": { "MessageGroupId": "orders", "SequenceNumber": 7 },
            "deep": { "flag": true },
        });
        // (case, configured value, the value one delivery carries)
        let rows: Vec<(&str, Option<&str>, Option<&str>)> = vec![
            ("configures-none", None, None),
            ("a-literal", Some("orders"), Some("orders")),
            // A literal is never read as a path, even when it looks like one in part.
            ("a-literal-with-a-dollar", Some("group$1"), Some("group$1")),
            ("a-path-to-a-string", Some("$.messageId"), Some("4fd2-1")),
            (
                "a-path-into-an-object",
                Some("$.attributes.MessageGroupId"),
                Some("orders"),
            ),
            (
                "a-path-to-a-number",
                Some("$.attributes.SequenceNumber"),
                Some("7"),
            ),
            ("a-path-to-a-boolean", Some("$.deep.flag"), Some("true")),
        ];
        for (case, configured, expected) in rows {
            let resolved = resolved_parameter(MESSAGE_GROUP_ID, configured, &event)
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                resolved.as_deref(),
                expected,
                "case {case:?}: expected {expected:?}, got {resolved:?}"
            );
        }
    }

    /// RULE: a path that does not select ONE value of the event is a failed delivery —
    /// the record stays on the source — and never a message carrying the path itself.
    #[test]
    fn a_path_that_selects_no_one_value_fails_the_delivery() {
        let event = json!({ "a": [1, 2], "b": { "c": "x" } });
        // (case, the configured path)
        let rows: Vec<(&str, &str)> = vec![
            ("absent", "$.missing"),
            ("several", "$.a[*]"),
            ("an-array", "$.a"),
            ("an-object", "$.b"),
        ];
        for (case, configured) in rows {
            let outcome = resolved_parameter(MESSAGE_GROUP_ID, Some(configured), &event);
            assert!(
                outcome.is_err(),
                "case {case:?}: expected the delivery to fail, got {outcome:?}"
            );
        }
    }

    /// RULE: what an enrichment RETURNS is what the target stage delivers — the
    /// elements of a returned array (however many), the one value a returned object or
    /// scalar is, and nothing at all for an answer that carries no value.
    ///
    /// The rows differ along the shape of the answer, which is what the rule
    /// generalises over: the empty answer, an empty array, one object, several objects,
    /// a scalar, and `null`.
    #[test]
    fn an_enrichment_answer_is_read_as_the_events_it_returned() {
        let rows: Vec<(&str, &str, usize)> = vec![
            ("no-answer", "", 0),
            ("blank-answer", "   ", 0),
            ("null", "null", 0),
            ("empty-array", "[]", 0),
            ("one-aggregate", r#"[{"batch_len":3}]"#, 1),
            ("three-events", r#"[{"a":1},{"a":2},{"a":3}]"#, 3),
            ("bare-object", r#"{"a":1}"#, 1),
            ("bare-string", r#""text""#, 1),
        ];
        for (case, answer, expected) in rows {
            let events = enrichment_events(answer.as_bytes(), "arn:aws:lambda:us-west-2::function:e")
                .unwrap_or_else(|error| panic!("case {case:?}: {}", error.detail));
            assert_eq!(
                events.len(),
                expected,
                "case {case:?}: expected {expected} event(s), got {events:?}"
            );
        }
    }

    /// RULE: an answer that is not JSON at all is a failed enrichment, not zero events —
    /// the batch must not be reported as delivered.
    #[test]
    fn an_enrichment_answer_that_is_not_json_is_a_failure() {
        let outcome = enrichment_events(b"not json", "arn:aws:lambda:us-west-2::function:e");
        assert!(
            outcome.is_err(),
            "case not-json: expected a failure, got {outcome:?}"
        );
    }

    /// RULE: a target message carries ONE event: the text of a string event, the JSON of
    /// any other — never a wrapper this emulator invented.
    #[test]
    fn a_target_body_is_the_one_event_it_delivers() {
        let rows: Vec<(&str, Value, &str)> = vec![
            ("string", json!("hello"), "hello"),
            ("object", json!({"a":1}), r#"{"a":1}"#),
            ("number", json!(7), "7"),
        ];
        for (case, event, expected) in rows {
            let body = target_body(&event);
            assert_eq!(
                body, expected,
                "case {case:?}: expected {expected:?}, got {body:?}"
            );
        }
    }

    /// RULE: a JSON path reads a record's message body in the format its producer sent it
    /// in — a body spelling a JSON document is the document it parses to, so a path names
    /// what is inside it, and a body spelling none is the text it carries — while every
    /// other member of the envelope is read exactly as it was built.
    ///
    /// The rows differ along what the rule generalises over: the format of the body
    /// (object, list, bare scalar, quoted string, plain text, not-quite-JSON), how deep
    /// the path reaches into it, the type of the value it selects, an envelope with no
    /// body member at all, and a path that reaches outside the body.
    #[test]
    fn a_path_reads_a_record_body_in_the_format_it_was_sent_in() {
        // (case, the envelope, the path, what it selects — `None` for a path that selects
        // no one value)
        let rows: Vec<(&str, Value, &str, Option<Value>)> = vec![
            (
                "a-member-of-a-json-object-body",
                json!({ "body": r#"{"id": "a-7", "type": "order"}"# }),
                "$.body.id",
                Some(json!("a-7")),
            ),
            (
                "a-number-member-of-a-json-object-body",
                json!({ "body": r#"{"count": 3}"# }),
                "$.body.count",
                Some(json!(3)),
            ),
            (
                "a-nested-member-of-a-json-object-body",
                json!({ "body": r#"{"order": {"id": "deep-1"}}"# }),
                "$.body.order.id",
                Some(json!("deep-1")),
            ),
            (
                "the-whole-json-body",
                json!({ "body": r#"{"id": "a-7"}"# }),
                "$.body",
                Some(json!({ "id": "a-7" })),
            ),
            (
                "an-element-of-a-json-list-body",
                json!({ "body": r#"[{"id": "first"}, {"id": "second"}]"# }),
                "$.body[1].id",
                Some(json!("second")),
            ),
            (
                // A body spelling no document is the text it is, so a path INTO it selects
                // nothing and the body itself is that text.
                "a-plain-text-body",
                json!({ "body": "not json at all" }),
                "$.body",
                Some(json!("not json at all")),
            ),
            (
                "a-member-of-a-plain-text-body",
                json!({ "body": "not json at all" }),
                "$.body.id",
                None,
            ),
            (
                // A bare scalar names nothing to descend into: the body is its text.
                "a-bare-number-body",
                json!({ "body": "17" }),
                "$.body",
                Some(json!("17")),
            ),
            (
                "a-quoted-json-string-body",
                json!({ "body": "\"quoted\"" }),
                "$.body",
                Some(json!("\"quoted\"")),
            ),
            (
                "a-body-that-is-almost-json",
                json!({ "body": "{not-valid-json" }),
                "$.body",
                Some(json!("{not-valid-json")),
            ),
            (
                // Every other member is read as it was built, body or no body.
                "a-member-beside-the-body",
                json!({ "body": r#"{"id": "a-7"}"#, "messageId": "m-1" }),
                "$.messageId",
                Some(json!("m-1")),
            ),
            (
                "an-envelope-with-no-body-member",
                json!({ "dynamodb": { "Keys": { "pk": { "S": "k" } } } }),
                "$.dynamodb.Keys.pk.S",
                Some(json!("k")),
            ),
        ];
        for (case, event, path, expected) in rows {
            let view = path_view(&event);
            let selected = ::service_common::json_reference_path::select(path, view.as_ref());
            assert_eq!(
                selected, expected.as_ref(),
                "case {case:?}: expected {path} to select {expected:?}, got {selected:?}"
            );
        }
    }

    /// RULE: reading a record for its paths changes nothing a stage handed the event
    /// itself receives — the envelope keeps the message body as the producer sent it.
    #[test]
    fn reading_a_record_for_its_paths_leaves_the_envelope_as_sent() {
        // (case, the body as sent)
        let rows: Vec<(&str, &str)> = vec![
            ("a-json-object", r#"{"id": "a-7"}"#),
            ("a-json-list", "[1, 2, 3]"),
            ("plain-text", "a plain body"),
        ];
        for (case, body) in rows {
            let event = json!({ "body": body, "eventSource": "aws:sqs" });
            let view = path_view(&event);
            assert_eq!(
                event["body"],
                json!(body),
                "case {case:?}: expected the envelope to keep the body as sent, got {event}"
            );
            assert_eq!(
                view["eventSource"],
                json!("aws:sqs"),
                "case {case:?}: expected every other member read as built, got {}",
                view.as_ref()
            );
        }
    }

    /// RULE: a template placeholder and a target parameter read the SAME record, so a
    /// path into a valid-JSON message body renders the member it names through either —
    /// and the render is the value itself, unquoted where the template does not quote it.
    #[test]
    fn a_path_into_a_json_body_renders_the_member_it_names() {
        let event = json!({
            "messageId": "4fd2-1",
            "body": r#"{"id": "a-7", "count": 3, "group": "orders"}"#,
            "eventSource": "aws:sqs",
        });
        let view = path_view(&event);
        // (case, the template, what the stage is handed)
        let rows: Vec<(&str, &str, Value)> = vec![
            ("a-scalar-member", "<$.body.id>", json!("a-7")),
            ("a-number-member", "<$.body.count>", json!(3)),
            (
                "a-member-inside-an-object",
                r#"{"order": "<$.body.id>", "n": <$.body.count>}"#,
                json!({"order": "a-7", "n": 3}),
            ),
            (
                "the-parsed-body-itself",
                "<$.body>",
                json!({"id": "a-7", "count": 3, "group": "orders"}),
            ),
        ];
        for (case, template, expected) in rows {
            let rendered = render_template(template, view.as_ref())
                .unwrap_or_else(|error| panic!("case {case:?}: {error}"));
            assert_eq!(
                rendered, expected,
                "case {case:?}: expected {expected}, got {rendered}"
            );
        }
        let resolved = resolved_parameter(MESSAGE_GROUP_ID, Some("$.body.group"), view.as_ref())
            .unwrap_or_else(|error| panic!("case a-parameter-into-the-body: {error}"));
        assert_eq!(
            resolved.as_deref(),
            Some("orders"),
            "case a-parameter-into-the-body: expected the body's group, got {resolved:?}"
        );
    }
}
