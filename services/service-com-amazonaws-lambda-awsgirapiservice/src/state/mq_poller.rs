// SPDX-License-Identifier: CC0-1.0

//! The POLLER behind an event source mapping whose event source is an Amazon MQ for
//! RabbitMQ broker.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda
//! Event Function. Lambda reads items from the event source and invokes the function", and
//! `Enabled` is "when true, the event source mapping is active. When false, Lambda pauses
//! polling and invocation" — so a mapping that reports `Enabled` on a broker is one that
//! CONSUMES from that broker with no client action at all, exactly as the reader of a queue
//! source ([`super::sqs_poller`]) and the reader of a shard source
//! ([`super::event_source_poller`]) do for theirs. A broker needs a reader of its own
//! because what it hands over, and how a consumer keeps its place in it, are neither of
//! those: a broker's queue hands the message at its head to ONE consumer and keeps it out of
//! the queue until that consumer acknowledges it, which is what this reader does through the
//! broker's own delivery bookkeeping. ONE pass of it is:
//!
//! * CONNECT. The source is "Amazon MQ – The ARN of the broker" (`EventSourceArn`), which
//!   Lambda consumes by connecting to the endpoints of the named broker — so the broker is
//!   resolved in the store of the service that OWNS brokers, over the shared engine, exactly
//!   as the call that WIRED the pair together resolved it
//!   ([`super::event_source_request::absent_event_source_message`]), and the queues read are
//!   that broker's own ([`mq::state::BrokerDataPlane`]), the same ones every AMQP
//!   client of that broker publishes to and consumes from. A broker that is still being
//!   provisioned answers no client on its endpoints yet, so nothing is consumed from it
//!   until it reports `RUNNING`; a mapping whose broker has since been deleted consumes
//!   nothing at all. What a BATCH is handed over as is the broker's ENGINE's own, so this
//!   reader consumes a RABBITMQ broker and no other: a mapping on a broker of another engine
//!   is left untouched and its poller stands down ([`serves_another_engine`]) rather than
//!   taking messages out of a queue it would deliver in the wrong form.
//! * POLL. `Queues` is "(Amazon MQ) The name of the Amazon MQ broker destination queue to
//!   consume" — exactly one — so the messages of that one queue are what the mapping takes,
//!   at most `BatchSize` of them per batch, oldest first, through the broker's own
//!   `take_next`: the message LEAVES the queue while this consumer holds it, as it does for
//!   any other consumer, and comes back marked redelivered when the consumer does not
//!   acknowledge it. A queue the broker does not hold is nothing this consumer can consume
//!   from, so a mapping naming one is `Enabled` and simply delivers nothing, recording that
//!   it could not reach its source
//!   ([`LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED`]) — the reading a pass over a queue that IS
//!   there replaces by itself, which is what makes the mapping recover when the queue is
//!   declared later.
//! * GATHER. "For ... Amazon MQ ... event sources, the default batching window is 500 ms"
//!   (`MaximumBatchingWindowInSeconds`), and a mapping stating a window of its own gathers
//!   over that instead: a batch that is neither full nor out of time is held until
//!   [`super::batch_is_due`] says it may go, measured from the first message gathered, so
//!   the function is invoked with the messages that accumulated over the window rather than
//!   with each as it arrives.
//! * FILTER. `FilterCriteria` is "an object that defines the filter criteria that determine
//!   whether Lambda should process an event", so a polled message that matches none of the
//!   mapping's patterns is not handed to the function. The decision is the shared one
//!   ([`super::event_source_filter`]); what is this source's own is the document a message is
//!   matched AS ([`filter_event`]). Such a message is not handed back to the queue either,
//!   because nothing else would: leaving it would hand it to the same filter at every pass.
//! * INVOKE. The messages the filter admits are handed to the mapping's function as ONE
//!   event, over the in-process callback into the `Invoke` surface of this same emulator
//!   ([`smithy_server_runtime::EmbeddedEmulator`]), in the form Lambda hands a RabbitMQ
//!   batch over in: `rmqMessagesByQueue`, keyed by the queue and the broker's virtual host,
//!   holding one record per message ([`rabbitmq_record`]). What the attempt came to is
//!   recorded on the mapping as its "result of the event source mapping's last processing
//!   attempt" (`LastProcessingResult`).
//! * ACKNOWLEDGE. A batch an invocation accepted is acknowledged by being gone: the poll
//!   already took it out of the queue. A batch no invocation accepted is handed BACK to the
//!   queue instead, in the order it was taken, so its messages are redelivered — which is
//!   what makes a function that cannot process them recoverable rather than a reason for the
//!   messages to be lost.
//!
//! At most ONE poller runs per mapping ([`drive_mq_pollers`], armed by the calls that make or
//! change a mapping), through the same per-mapping claim both other readers take
//! ([`PollerClaim`]), and it stands down as soon as its mapping stops wanting to be polled.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::amq_protocol::protocol::basic::AMQPProperties;
use ::amq_protocol::types::AMQPValue;
use ::serde_json::{json, Map, Value};
use ::service_com_amazonaws_mq_mq as mq;
use ::service_com_amazonaws_mq_mq::state::message_broker::Message;
use ::service_com_amazonaws_mq_mq::state::SharedMessageBroker;

use super::event_source_poller::{Clients, PollerClaim};
use super::{
    batch_is_due, service_state, ComAmazonawsLambdaAwsgirapiserviceState,
    EVENT_SOURCE_MAPPING_STATE_ENABLED, LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED,
    LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_NO_RECORDS,
    LAST_PROCESSING_RESULT_OK, POLL_INTERVAL, STATE_ACTIVE,
};

/// The `eventSource` the event a RabbitMQ mapping hands its function carries.
const RABBITMQ_EVENT_SOURCE: &str = "aws:rmq";

/// The virtual host a broker of this emulator serves, which is the one every AMQP session on
/// it opens: a broker holds one virtual host ([`mq::state::message_broker`]), and `/` is the
/// name RabbitMQ gives it.
const DEFAULT_VIRTUAL_HOST: &str = "/";

/// `SourceAccessConfiguration$Type` naming the virtual host a mapping consumes in:
/// "VIRTUAL_HOST – (RabbitMQ) The name of the virtual host in your RabbitMQ broker"
/// (`smithy_query(query="shape",shape="SourceAccessType")`).
const SOURCE_ACCESS_VIRTUAL_HOST: &str = "VIRTUAL_HOST";

/// The batching window a mapping on a broker that states none gathers over: "For ... Amazon
/// MQ ... event sources, the default batching window is 500 ms"
/// (`CreateEventSourceMapping$MaximumBatchingWindowInSeconds`).
const DEFAULT_BATCHING_WINDOW: Duration = Duration::from_millis(500);

/// The longest batching window a mapping gathers a batch over: `MaximumBatchingWindowInSeconds`'
/// own `@range(min: 0, max: 300)`. The boundary validator already refuses a request past it;
/// this is that same bound applied where the POLLER relies on it, since the value is read back
/// out of stored state.
const MAX_BATCHING_WINDOW_SECONDS: i32 = 300;

/// How deeply a record's `basicProperties.headers` is rendered.
///
/// How deeply a header table NESTS is whatever the publishing client chose, so the rendering
/// is bounded rather than recursing on a depth a caller picked: a table or an array below this
/// many levels is reported as no value at all. It is the same bound the broker itself holds one
/// message's own fan-out to
/// ([`mq::state::message_broker::MAX_PLACEMENT_STEPS`]), and far deeper than any header a
/// client sends.
const MAX_HEADER_DEPTH: usize = 32;

/// One mapping's broker queue, as a pass reads it out of the store before it touches the
/// broker: the mapping to record the attempt against, the broker and queue to consume, the
/// function the batch is handed to, how many messages one batch may hold, how long a batch is
/// gathered for, and what the mapping asked be filtered.
#[derive(Debug)]
struct BrokerTarget {
    /// `UUID`, the key the mapping is stored under.
    uuid: String,
    /// `EventSourceArn`, the broker to consume, as the event reports it.
    broker_arn: String,
    /// The broker's own NAME, which is how a broker ARN addresses one broker of an account.
    broker_name: String,
    /// The Region that broker lives in, which is the Region this pass reaches this emulator's
    /// services in.
    region: String,
    /// The one `Queues` entry: "the name of the Amazon MQ broker destination queue to
    /// consume".
    queue: String,
    /// The virtual host the event's `rmqMessagesByQueue` key names the queue inside.
    virtual_host: String,
    /// `FunctionArn`: "The ARN of the Lambda function" the batch is handed to.
    function_arn: String,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from your
    /// stream or queue".
    batch_size: usize,
    /// The window one batch is gathered over ([`DEFAULT_BATCHING_WINDOW`]).
    batching_window: Duration,
    /// The `Pattern` of each of the mapping's `FilterCriteria` filters, as stored.
    filters: Vec<String>,
}

/// The batch a mapping is still gathering: its messages are already OUT of the broker's
/// queue — the poll that took them is this consumer's hold on them — so they are held here
/// until the batch is full or the window has run, and then handed to the function as one
/// event.
///
/// It carries the queue those messages came OUT of, so a hold that ends without a delivery —
/// the source going away under the gathering, or the poller standing down because its mapping
/// stopped wanting to be polled — hands them BACK to that queue instead of dropping them,
/// which is what RabbitMQ does with a consumer's unacknowledged messages when its channel
/// closes ([`Gathering::hand_back`]).
#[derive(Debug, Default)]
struct Gathering {
    /// When the first message of this batch was gathered, which is what the window is
    /// measured from; `None` while nothing has been gathered.
    opened_at: Option<SystemTime>,
    /// The messages gathered so far, in the order the queue handed them over.
    messages: Vec<Message>,
    /// The broker's queues and the queue inside them these messages were taken out of, held
    /// from the pass that took the first of them.
    source: Option<(SharedMessageBroker, String)>,
}

impl Gathering {
    /// Hand every message still held back to the queue it came out of, in the order it was
    /// taken, leaving this gathering empty.
    ///
    /// A queue the broker no longer holds takes none of them back, which is the same answer
    /// RabbitMQ gives a requeue onto a queue that has been deleted.
    fn hand_back(&mut self) {
        let messages = ::std::mem::take(&mut self.messages);
        self.opened_at = None;
        let Some((queues, queue)) = self.source.take() else {
            return;
        };
        hand_back_to(&queues, &queue, messages);
    }
}

/// What ONE poll loop carries between its wake-ups: the clients it invokes through, and the
/// batch each of its mappings is still gathering. Nothing in it is state a client can read.
#[derive(Default)]
struct PollSession {
    clients: Clients,
    gathering: BTreeMap<String, Gathering>,
}

/// Start the in-process poller of every Amazon MQ mapping of this engine's store that wants
/// one, for the mappings that have none running yet.
///
/// Called by the operations that make or change a mapping, AFTER they have released the
/// store's lock. At most ONE poller runs per MAPPING (the store carries that claim,
/// [`ComAmazonawsLambdaAwsgirapiserviceState::claim_event_source_poller`]) and each of them
/// consumes its own mapping's queue alone, so what one mapping's function takes to answer is
/// no part of what another mapping's messages wait for. A poller stands down as soon as its
/// mapping stops wanting to be polled, or as soon as its broker turns out to run an engine
/// this reader does not serve, so none of them outlives the mapping it polls and none of them
/// consumes a broker whose batches are another engine's event; the
/// store holds at most [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings, so the tasks this can
/// ever spawn are bounded by that count. With no async runtime to spawn onto there is no
/// poller and nothing is consumed.
pub fn drive_mq_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    let Some(state) = service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    let claimed: Vec<PollerClaim> = {
        let mut store = state.lock();
        wanted_broker_mappings(&store)
            .into_iter()
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
        runtime.spawn(async move {
            poll_one_mapping(&callback, claim, &*engine).await;
        });
    }
}

/// The poll loop of ONE mapping: a pass of its own, [`POLL_INTERVAL`] later another, until it
/// stops wanting to be polled.
async fn poll_one_mapping<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    engine: &Eng,
) {
    let mut session = PollSession::default();
    let state = ::engine::ServiceState::clone(&claim.state);
    loop {
        // Every wake-up waits, which is what hands the runtime back between passes. No lock
        // is held across it.
        ::tokio::time::sleep(POLL_INTERVAL).await;
        // Whether this mapping still wants polling, and the claim's release, happen in ONE
        // critical section, so either this poller keeps the mapping or the call that re-arms
        // the pollers finds the claim free and starts one of its own.
        let named_broker = {
            let mut store = state.lock();
            if !mapping_wants_polling(&store, &claim.uuid) {
                claim.release(&mut store);
                drop(store);
                // The hold this poller had on a batch it was still gathering ends with it, so
                // those messages go back to the queue they came out of rather than going with
                // the task ([`Gathering::hand_back`]).
                if let Some(mut gathering) = session.gathering.remove(&claim.uuid) {
                    gathering.hand_back();
                }
                return;
            }
            broker_named_by(&store, &claim.uuid)
        };
        // A broker running an engine this reader does not serve is none of its business: a
        // batch of such a broker is handed over as that engine's own event, not as the
        // RabbitMQ one this reader builds, so the poller gives the mapping back untouched
        // rather than consuming a queue it would deliver in the wrong form. The two stores
        // are never locked at once: the broker's name was read above and the broker's engine
        // is read here.
        if named_broker.is_some_and(|name| serves_another_engine(engine, &name)) {
            let mut store = state.lock();
            claim.release(&mut store);
            drop(store);
            if let Some(mut gathering) = session.gathering.remove(&claim.uuid) {
                gathering.hand_back();
            }
            return;
        }
        poll_pass(callback, &mut session, engine, &claim.uuid, SystemTime::now()).await;
    }
}

/// The NAME of the broker the mapping `uuid` names, and `None` for a mapping of this store on
/// no broker at all.
fn broker_named_by(store: &ComAmazonawsLambdaAwsgirapiserviceState, uuid: &str) -> Option<String> {
    store
        .event_source_mappings
        .get(uuid)?
        .event_source_arn
        .as_deref()
        .and_then(super::event_source_request::named_broker)
        .map(str::to_string)
}

/// ONE pass over the mapping `uuid` names, at `now`: the whole of what such a mapping does
/// with no client action at all, as this module's documentation states it.
///
/// No lock is held across the invocation: each step takes one store's lock, finishes with it
/// and releases it before the next step takes another.
async fn poll_pass<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Eng,
    uuid: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    let Some(target) = ({
        let mut store = state.lock();
        // A mapping the clock has already moved into `Enabled` is polled, and one it has
        // removed is not.
        store.materialize_all(now);
        polling_target(&store, uuid)
    }) else {
        return;
    };
    // The broker's own queues, as the service that owns brokers holds them. A broker this
    // mapping cannot reach is a connect failure and no attempt at a batch: whatever this
    // poller was still gathering goes back to the queue it came out of, nothing is consumed,
    // and the pass after it decides afresh.
    let Some(queues) = broker_queues(engine, &target, now) else {
        if let Some(gathering) = session.gathering.get_mut(&target.uuid) {
            gathering.hand_back();
        }
        record_attempt(&state, &target, LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED);
        return;
    };
    let gathering = session.gathering.entry(target.uuid.clone()).or_default();
    let reachable = {
        let mut broker = queues.lock();
        // A message that has outlived its time to live is the queue's own business and is
        // owed by the clock, so it is applied before this pass sees what there is to take.
        broker.expire_due(now);
        // A queue the broker does not hold is nothing this consumer can consume from, so the
        // pass takes nothing and reports the connect failure below.
        let reachable = broker.has_queue(&target.queue);
        while reachable && gathering.messages.len() < target.batch_size {
            match broker.take_next(&target.queue, now) {
                Some(message) => {
                    if gathering.messages.is_empty() {
                        // The window is measured from the first message gathered, not from
                        // the pass that found the queue empty.
                        gathering.opened_at = Some(now);
                    }
                    gathering.messages.push(message);
                    // Where these messages go BACK to if this hold ends without a delivery.
                    gathering.source = Some((queues.clone(), target.queue.clone()));
                }
                None => break,
            }
        }
        reachable
    };
    if !reachable {
        // The queue went away under a batch this poller was still gathering: the messages go
        // back to it, which takes none of them while it is gone and all of them once it is
        // declared again.
        gathering.hand_back();
        record_attempt(&state, &target, LAST_PROCESSING_RESULT_BROKER_CONNECT_FAILED);
        return;
    }
    let waited = gathering
        .opened_at
        .map(|opened| now.duration_since(opened).unwrap_or_default())
        .unwrap_or_default();
    if gathering.messages.is_empty() {
        // The queue was there and held nothing: the pass processed no record, which is what
        // the mapping reports until one of them attempts a batch — and what CLEARS a connect
        // failure recorded while the queue was not there yet.
        record_attempt(&state, &target, LAST_PROCESSING_RESULT_NO_RECORDS);
        return;
    }
    if !batch_is_due(
        gathering.messages.len(),
        target.batch_size,
        waited,
        target.batching_window,
    ) {
        return;
    }
    let batch = ::std::mem::take(&mut gathering.messages);
    session.gathering.remove(&target.uuid);
    hand_over(callback, session, &state, &queues, &target, batch).await;
}

/// Hand one gathered batch to the mapping's function, and do with its messages what the
/// attempt came to says: a batch the invocation accepted is acknowledged by being gone, and
/// one it did not is handed back to the queue for redelivery.
///
/// A batch whose filter criteria admitted NOTHING is an attempt that succeeded having handed
/// no message to the function: the pass handled the whole batch by acknowledging what the
/// criteria rejected, so nothing is invoked and nothing comes back.
async fn hand_over(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    queues: &SharedMessageBroker,
    target: &BrokerTarget,
    batch: Vec<Message>,
) {
    let admitted = admitted_of(target, batch);
    if admitted.is_empty() {
        record_attempt(mappings, target, LAST_PROCESSING_RESULT_OK);
        return;
    }
    let payload = rabbitmq_event(target, &admitted);
    let client = session.clients.lambda(callback, &target.region);
    match invoke_function(client, &target.function_arn, &payload).await {
        Ok(()) => record_attempt(mappings, target, LAST_PROCESSING_RESULT_OK),
        Err(failure) => {
            ::tracing::warn!(
                mapping = %target.uuid,
                queue = %target.queue,
                "{failure}, so the batch is handed back to the broker's queue"
            );
            hand_back_to(queues, &target.queue, admitted);
            record_attempt(mappings, target, LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED);
        }
    }
}

/// Exactly the messages of `batch` the mapping's `FilterCriteria` admit, in the order the
/// queue handed them over.
fn admitted_of(
    target: &BrokerTarget,
    batch: Vec<Message>,
) -> Vec<Message> {
    let criteria = super::event_source_filter::Filters::of(&target.uuid, &target.filters);
    if criteria.admits_everything() {
        return batch;
    }
    let mut budget = super::event_source_filter::budget();
    batch
        .into_iter()
        .filter(|message| criteria.admits(&filter_event(message), &mut budget))
        .collect()
}

/// Hand `messages` back to the queue `queue` names, in the order they were taken out of it,
/// so the next pass reads them in that same order marked redelivered.
///
/// The ONE way a hold this poller took ends without a delivery: a batch no invocation accepted
/// ([`hand_over`]) and a batch whose hold ended before it was due ([`Gathering::hand_back`])
/// both go back the same way.
fn hand_back_to(queues: &SharedMessageBroker, queue: &str, messages: Vec<Message>) {
    if messages.is_empty() {
        return;
    }
    let mut broker = queues.lock();
    // A requeue puts the message at the HEAD of the queue, so the batch goes back newest
    // first for the order it was taken in to be the order it comes back in.
    for message in messages.into_iter().rev() {
        broker.requeue(queue, message);
    }
}

/// The broker queues `target`'s mapping consumes, as the service that owns brokers holds them
/// at `now` — `None` when this engine holds no broker store, when the broker the mapping
/// names is gone, when it runs an ENGINE this reader does not serve, when it is not `RUNNING`
/// yet, or when this emulator opened no data plane for it.
///
/// A broker that is still being provisioned answers no client on its endpoints, so there is
/// nothing for this consumer to connect to yet; the pass after the move the clock owes finds
/// it running.
///
/// The ENGINE is part of the question because a broker of either engine holds its
/// destinations in the same store ([`mq::state::message_broker`]) while what a BATCH of it is
/// handed over as is the engine's: this reader builds the RabbitMQ event
/// ([`rabbitmq_event`]), so it consumes a RabbitMQ broker's queue and nothing else — a
/// mapping on a broker of another engine is not consumed here at all, and its poller stands
/// down ([`serves_another_engine`]).
fn broker_queues<Eng: Send + Sync + 'static>(
    engine: &Eng,
    target: &BrokerTarget,
    now: SystemTime,
) -> Option<SharedMessageBroker> {
    let brokers = mq::state::service_state(engine)?;
    let store = brokers.lock();
    let broker = store.broker_by_name(&target.broker_name)?;
    if broker.engine_type != mq::state::ENGINE_TYPE_RABBITMQ
        || broker.broker_state(now) != mq::state::BROKER_STATE_RUNNING
    {
        return None;
    }
    let broker_id = broker.broker_id.clone();
    Some(store.broker_data_planes.get(&broker_id)?.broker().clone())
}

/// Whether the broker `broker_name` names runs an engine this reader does NOT serve, which is
/// what makes a mapping on it none of this poller's business.
///
/// `false` while nothing is known about the broker — this engine holds no broker store, or no
/// broker of that name exists yet — because a mapping may be made before its broker is: such
/// a poller keeps its mapping and finds the broker on a later pass.
fn serves_another_engine<Eng: Send + Sync + 'static>(engine: &Eng, broker_name: &str) -> bool {
    mq::state::service_state(engine).is_some_and(|brokers| {
        let store = brokers.lock();
        store
            .broker_by_name(broker_name)
            .is_some_and(|broker| broker.engine_type != mq::state::ENGINE_TYPE_RABBITMQ)
    })
}

/// Record what one processing attempt came to on the mapping itself.
///
/// The record is re-read under the lock and only written when it still names the broker and
/// the function this batch was polled for, so a mapping re-targeted while the batch was in
/// flight does not report an attempt made under a configuration it no longer carries.
fn record_attempt(
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    target: &BrokerTarget,
    result: &str,
) {
    let mut store = mappings.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(&target.uuid) else {
        return;
    };
    if mapping.event_source_arn.as_deref() != Some(target.broker_arn.as_str())
        || mapping.function_arn != target.function_arn
    {
        return;
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// Hand one batch to the function, synchronously, as an Amazon MQ event source's invocation
/// is, and answer whether the invocation ACCEPTED it.
async fn invoke_function(
    client: &::aws_sdk_lambda::Client,
    function_arn: &str,
    payload: &Value,
) -> Result<(), String> {
    let answer = client
        .invoke()
        .function_name(function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "{function_arn} could not be invoked ({})",
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed.
    if let Some(failure) = answer.function_error.as_deref() {
        return Err(format!(
            "{function_arn} answered a {failure} error for the batch"
        ));
    }
    Ok(())
}

/// The event a batch is handed over as: `rmqMessagesByQueue`, keyed by the queue and the
/// virtual host it lives in, holding one record per message in the order the queue handed
/// them over.
fn rabbitmq_event(
    target: &BrokerTarget,
    messages: &[Message],
) -> Value {
    let records: Vec<Value> = messages.iter().map(rabbitmq_record).collect();
    let mut by_queue = Map::new();
    by_queue.insert(
        format!("{}::{}", target.queue, target.virtual_host),
        Value::Array(records),
    );
    json!({
        "eventSource": RABBITMQ_EVENT_SOURCE,
        "eventSourceArn": target.broker_arn.clone(),
        "rmqMessagesByQueue": Value::Object(by_queue),
    })
}

/// One message as the FILTER sees it: the record the function receives, with a `data` that
/// holds a JSON object or array read as that JSON.
///
/// The function always receives `data` as the message's bytes base64-encoded, but a filter
/// names the fields INSIDE a structured message, so for matching a body that is a JSON object
/// or array is read as that document — the same reading the queue source's filter takes of a
/// structured body ([`super::sqs_poller`]). A body that is anything else stays what it is.
fn filter_event(message: &Message) -> Value {
    let mut event = rabbitmq_record(message);
    let parsed = ::std::str::from_utf8(&message.body)
        .ok()
        .and_then(|text| ::serde_json::from_str::<Value>(text).ok())
        .filter(|body| body.is_object() || body.is_array());
    if let Some(body) = parsed {
        event["data"] = body;
    }
    event
}

/// One stored message as the record of the event Lambda hands a function for a RabbitMQ
/// source: the AMQP basic properties the publish carried, whether the broker has already
/// delivered the message once, and the message's own bytes.
fn rabbitmq_record(message: &Message) -> Value {
    json!({
        "basicProperties": basic_properties(&message.properties, message.body.len()),
        "redelivered": message.redelivered,
        "data": ::base64::Engine::encode(
            &::base64::engine::general_purpose::STANDARD,
            &message.body,
        ),
    })
}

/// The `basicProperties` of one record: every AMQP 0-9-1 basic property, reported as the
/// publish set it and `null` for one the publish left out, plus the size of the message's own
/// body.
fn basic_properties(properties: &AMQPProperties, body_size: usize) -> Value {
    json!({
        "contentType": text(properties.content_type().as_ref().map(ToString::to_string)),
        "contentEncoding": text(
            properties.content_encoding().as_ref().map(ToString::to_string),
        ),
        "headers": properties
            .headers()
            .as_ref()
            .map_or(Value::Null, field_table),
        "deliveryMode": number(properties.delivery_mode().map(u64::from)),
        "priority": number(properties.priority().map(u64::from)),
        "correlationId": text(properties.correlation_id().as_ref().map(ToString::to_string)),
        "replyTo": text(properties.reply_to().as_ref().map(ToString::to_string)),
        "expiration": text(properties.expiration().as_ref().map(ToString::to_string)),
        "messageId": text(properties.message_id().as_ref().map(ToString::to_string)),
        "timestamp": properties
            .timestamp()
            .and_then(rendered_timestamp)
            .map_or(Value::Null, Value::String),
        "type": text(properties.kind().as_ref().map(ToString::to_string)),
        "userId": text(properties.user_id().as_ref().map(ToString::to_string)),
        "appId": text(properties.app_id().as_ref().map(ToString::to_string)),
        "clusterId": text(properties.cluster_id().as_ref().map(ToString::to_string)),
        "bodySize": body_size,
    })
}

/// A property the publish set, as the record carries it, and `null` for one it left out.
fn text(value: Option<String>) -> Value {
    value.map_or(Value::Null, Value::String)
}

/// See [`text`], for a numeric property.
fn number(value: Option<u64>) -> Value {
    value.map_or(Value::Null, |value| Value::Number(value.into()))
}

/// An AMQP field table as the record reports one: a member per entry, each value rendered by
/// [`field_value`].
fn field_table(table: &::amq_protocol::types::FieldTable) -> Value {
    // This table is the first of the levels the bound admits, so the levels left below it are
    // one fewer.
    field_table_within(table, MAX_HEADER_DEPTH.saturating_sub(1))
}

/// [`field_table`] with `depth` levels of nesting left to render.
fn field_table_within(table: &::amq_protocol::types::FieldTable, depth: usize) -> Value {
    let mut rendered = Map::new();
    for (name, value) in table.inner() {
        rendered.insert(name.to_string(), field_value(value, depth));
    }
    Value::Object(rendered)
}

/// One AMQP field value as the record reports it, with `depth` levels of nesting left.
///
/// A header whose value is a STRING is reported as the bytes it is — AMQP long strings are
/// byte strings, and that is how the record carries one — and the other field types as the
/// JSON values they already are. A table or an array nested deeper than [`MAX_HEADER_DEPTH`]
/// is reported as no value at all rather than walked, because how deeply a header nests is the
/// PUBLISHER's choice and nothing here may recurse on a depth a caller picked.
fn field_value(value: &AMQPValue, depth: usize) -> Value {
    match value {
        AMQPValue::Boolean(flag) => Value::Bool(*flag),
        AMQPValue::ShortShortInt(number) => json!(number),
        AMQPValue::ShortShortUInt(number) => json!(number),
        AMQPValue::ShortInt(number) => json!(number),
        AMQPValue::ShortUInt(number) => json!(number),
        AMQPValue::LongInt(number) => json!(number),
        AMQPValue::LongUInt(number) => json!(number),
        AMQPValue::LongLongInt(number) => json!(number),
        AMQPValue::Float(number) => json!(number),
        AMQPValue::Double(number) => json!(number),
        AMQPValue::DecimalValue(decimal) => json!({
            "scale": decimal.scale,
            "value": decimal.value,
        }),
        AMQPValue::Timestamp(stamp) => json!(stamp),
        AMQPValue::ShortString(string) => bytes_of(string.as_str().as_bytes()),
        AMQPValue::LongString(string) => bytes_of(string.as_bytes()),
        AMQPValue::FieldArray(array) => match depth.checked_sub(1) {
            None => Value::Null,
            Some(left) => Value::Array(
                array
                    .as_slice()
                    .iter()
                    .map(|value| field_value(value, left))
                    .collect(),
            ),
        },
        AMQPValue::FieldTable(table) => match depth.checked_sub(1) {
            None => Value::Null,
            Some(left) => field_table_within(table, left),
        },
        AMQPValue::ByteArray(array) => bytes_of(array.as_slice()),
        AMQPValue::Void => Value::Null,
    }
}

/// A byte string as the record carries one: its bytes, under the member a reader reads them
/// back out of.
fn bytes_of(bytes: &[u8]) -> Value {
    json!({ "bytes": bytes })
}

/// The instant an AMQP `timestamp` property carries, as the record renders it: the month, the
/// day and the year, then the time of day on a twelve-hour clock, written by this crate's one
/// renderer of such text ([`super::calendar_fields`]).
///
/// `None` for a value no instant can be computed for at all.
fn rendered_timestamp(seconds: u64) -> Option<String> {
    super::calendar_fields::twelve_hour_clock(seconds)
}

/// The broker queue of the mapping `uuid` names, when that mapping is polling at this
/// instant.
///
/// A mapping polls when it reports `Enabled` — "when true, the event source mapping is
/// active. When false, Lambda pauses polling and invocation" — its event source is an Amazon
/// MQ broker, it names the queue to consume, and the function it invokes is one this store
/// holds in [`STATE_ACTIVE`]: "Lambda reads items from the event source and invokes the
/// function", so a mapping whose function cannot be invoked yet takes nothing out of the
/// queue.
///
/// Read it only after [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`].
fn polling_target(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    uuid: &str,
) -> Option<BrokerTarget> {
    let mapping = store.event_source_mappings.get(uuid)?;
    if mapping.state != EVENT_SOURCE_MAPPING_STATE_ENABLED || mapping.is_deleting() {
        return None;
    }
    let function = store.functions.get(&mapping.function_name)?;
    if function.state != STATE_ACTIVE {
        return None;
    }
    let broker_arn = mapping.event_source_arn.clone()?;
    let broker_name = super::event_source_request::named_broker(&broker_arn)?.to_string();
    let region = ::service_common::arn::parse(&broker_arn)?.region.to_string();
    // "(Amazon MQ) The name of the Amazon MQ broker destination queue to consume": one
    // entry, so a mapping that names none has no queue to consume.
    let queue = mapping
        .queues
        .as_ref()
        .and_then(|queues| queues.first())
        .filter(|queue| !queue.is_empty())?
        .clone();
    Some(BrokerTarget {
        uuid: mapping.uuid.clone(),
        broker_arn,
        broker_name,
        region,
        queue,
        virtual_host: virtual_host_of(mapping),
        function_arn: mapping.function_arn.clone(),
        // "The maximum number of records in each batch that Lambda pulls from your stream
        // or queue": the size is the batch's ceiling, and a mapping always pulls at least
        // one message.
        batch_size: usize::try_from(mapping.batch_size)
            .unwrap_or(1)
            .clamp(1, super::sqs_poller::MAX_MESSAGES_PER_BATCH),
        batching_window: batching_window(mapping.maximum_batching_window_in_seconds),
        filters: super::event_source_filter::patterns_of(mapping.filter_criteria.as_ref()),
    })
}

/// The virtual host a mapping consumes in: the one its `SourceAccessConfigurations` name
/// under [`SOURCE_ACCESS_VIRTUAL_HOST`], and otherwise the broker's own default
/// ([`DEFAULT_VIRTUAL_HOST`]).
fn virtual_host_of(mapping: &super::EventSourceMappingRecord) -> String {
    mapping
        .source_access_configurations
        .as_ref()
        .into_iter()
        .flatten()
        .find(|configuration| configuration.r#type.as_deref() == Some(SOURCE_ACCESS_VIRTUAL_HOST))
        .and_then(|configuration| configuration.uri.clone())
        .filter(|host| !host.is_empty())
        .unwrap_or_else(|| DEFAULT_VIRTUAL_HOST.to_string())
}

/// The batching window a mapping on a broker gathers ONE batch over, read inside the range
/// `MaximumBatchingWindowInSeconds` admits: the span the mapping states, and otherwise this
/// source's own default ([`DEFAULT_BATCHING_WINDOW`]).
fn batching_window(configured: Option<i32>) -> Duration {
    match configured {
        Some(seconds) => Duration::from_secs(u64::from(
            u32::try_from(seconds.clamp(0, MAX_BATCHING_WINDOW_SECONDS)).unwrap_or_default(),
        )),
        None => DEFAULT_BATCHING_WINDOW,
    }
}

/// Whether the mapping `uuid` names wants a poller of its own at this instant: it is a
/// mapping of this store, on an Amazon MQ broker, that is `Enabled` and not being deleted.
///
/// A mapping still SETTLING into `Enabled` counts: the state it settles into is its `Enabled`
/// setting, and the clock owes that move. Whether its FUNCTION can be invoked yet, and
/// whether its broker is running, are questions of each PASS and not of the poller's life.
fn mapping_wants_polling(store: &ComAmazonawsLambdaAwsgirapiserviceState, uuid: &str) -> bool {
    store
        .event_source_mappings
        .get(uuid)
        .is_some_and(wants_polling)
}

/// Every mapping of this store that wants a poller of its own ([`mapping_wants_polling`]).
fn wanted_broker_mappings(store: &ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<String> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| wants_polling(mapping))
        .map(|mapping| mapping.uuid.clone())
        .collect()
}

/// The ONE predicate behind both: the mapping consumes an Amazon MQ broker's queue, is
/// `Enabled` and is not being deleted.
fn wants_polling(mapping: &super::EventSourceMappingRecord) -> bool {
    mapping.enabled
        && !mapping.is_deleting()
        && mapping
            .event_source_arn
            .as_deref()
            .and_then(super::event_source_request::named_broker)
            .is_some()
}

#[cfg(test)]
mod tests;
