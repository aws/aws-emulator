// SPDX-License-Identifier: CC0-1.0

//! The POLLER behind an event source mapping whose event source is an Amazon MSK cluster's
//! topic.
//!
//! `CreateEventSourceMapping` "creates a mapping between an event source and an Lambda Event
//! Function. Lambda reads items from the event source and invokes the function", and
//! `Enabled` is "when true, the event source mapping is active. When false, Lambda pauses
//! polling and invocation" — so a mapping that reports `Enabled` on a cluster is one that
//! CONSUMES the topics it names with no client action at all, exactly as the reader of a
//! queue source ([`super::sqs_poller`]), of a broker's queue ([`super::mq_poller`]) and of a
//! shard source ([`super::event_source_poller`]) do for theirs. A Kafka topic needs a reader
//! of its own because what it hands over, and how a consumer keeps its place in it, are
//! neither of those: a topic's records sit in PARTITIONS and are handed out from an OFFSET
//! the consumer carries, and nothing is removed by being read. ONE pass of it is:
//!
//! * CONNECT. The source is "Amazon Managed Streaming for Apache Kafka – The ARN of the
//!   cluster" (`EventSourceArn`), so the cluster is resolved in the store of the service that
//!   OWNS clusters, over the shared engine, and the topics read are that cluster's own — the
//!   same log every Apache Kafka client of that cluster produces to and fetches from, read
//!   through the same data-plane reading the cluster's own wire protocol serves a `Fetch`
//!   from ([`kafka::ComAmazonawsKafkaKafkaState::data_plane_fetch`]), with the records the
//!   topic's retention has aged out already gone. A cluster that is still being provisioned
//!   answers no client on its brokers yet, so nothing is consumed from it until it reports
//!   `ACTIVE`; a mapping whose cluster has since been deleted, or that names a topic the
//!   cluster does not hold, consumes nothing and records that it could not reach its source
//!   ([`LAST_PROCESSING_RESULT_CONNECTION_ERROR`]) — the reading a pass over a topic that IS
//!   there replaces by itself, which is what makes the mapping recover when the topic is
//!   created later.
//! * POLL. `Topics` is "the name of the Kafka topic", and a topic's records are divided over
//!   its partitions, so each partition of each topic the mapping names is read from the
//!   position that mapping carries for it, at most `BatchSize` records at a time: "the
//!   maximum number of records in each batch that Lambda pulls from your stream or queue".
//!   Each partition is handed over as a batch of its own, so the records of one partition
//!   reach the function in the order the log holds them.
//! * POSITION. `StartingPosition` is "the position in a stream from which to start reading",
//!   a property of the mapping fixed when the mapping is created, so pausing does not move it
//!   ([`super::ShardCursor`]): a mapping enabled again reads on from the offset it had got
//!   to, and one that has read nothing yet reads from where its creation fixed it — the
//!   oldest record the log still holds for `TRIM_HORIZON`, and the first record at or after
//!   the stated instant for `AT_TIMESTAMP`, which for `LATEST` is the instant the mapping was
//!   created. That is what makes a backlog built up while the mapping was paused OBSERVABLE
//!   rather than skipped. Nothing is deleted from the log by being read — its records stay in
//!   it for the topic's retention period — so the position is the whole of what one consumer
//!   has handled, and it moves past a record the moment the pass has handled it.
//! * GATHER. "For Amazon MSK, Self-managed Apache Kafka, Amazon MQ, and Amazon DocumentDB
//!   event sources, the default batching window is 500 ms"
//!   (`CreateEventSourceMappingInput$MaximumBatchingWindowInSeconds`), and a mapping stating
//!   a window of its own gathers over that instead: a batch that is neither full nor out of
//!   time is held until [`super::batch_is_due`] says it may go, measured from the pass that
//!   first saw a record of it. The gathering costs no record and no position — the log keeps
//!   the records and the position moves only past a batch that was handled — so a batch still
//!   gathering is simply read again, with the records that arrived meanwhile.
//! * FILTER. `FilterCriteria` is "an object that defines the filter criteria that determine
//!   whether Lambda should process an event", so of the records a pass read exactly those at
//!   least one of the mapping's patterns matches are handed to the function. The decision is
//!   the shared one ([`super::event_source_filter`]); what is this source's own is the
//!   document a record is matched AS ([`filter_event`]). A record no pattern matches is not
//!   handed to any invocation, and the PASS accounts for it: the position moves past it
//!   exactly as it moves past a record an invocation processed, which is what lets the
//!   records behind it be delivered, and a batch whose every record the criteria reject is a
//!   pass that makes no invocation at all.
//! * INVOKE. The records the filter admits are handed to the mapping's function as ONE event,
//!   over the in-process callback into the `Invoke` surface of this same emulator
//!   ([`smithy_server_runtime::EmbeddedEmulator`]), in the form Lambda hands a Kafka batch
//!   over in: `records`, keyed by the topic and partition they came from, holding one record
//!   per message ([`kafka_event`]). What the attempt came to is recorded on the mapping as
//!   "the result of the event source mapping's last processing attempt"
//!   (`LastProcessingResult`).
//! * CHECKPOINT. A partition has ONE position, so it moves past the run of records at the
//!   HEAD of the batch the pass HANDLED: the ones an invocation processed, and the ones the
//!   criteria rejected. A batch the function did not process moves the position not at all,
//!   so its records are read again — which is what makes a function that cannot process them
//!   recoverable rather than a reason for the records to be skipped. Which records an
//!   invocation processed is the whole batch unless the mapping asks for its function's
//!   report of the ones that failed, which is what `FunctionResponseTypes` says (see
//!   [`super::batch_item_failures`]); a Kafka record is named in such a report by its OFFSET,
//!   which is how a report of this source identifies one record.
//! * METER. What the pass did with the records it read is published per MAPPING while its
//!   `MetricsConfig` asks for it — "the metrics you want your event source mapping to
//!   produce" — exactly as the other pollers publish it
//!   ([`super::metrics::FunctionMetrics::publish_event_counts`]): how many records the pass
//!   read, how many its criteria rejected and how many an invocation accepted, with a
//!   0-valued read count for a minute in which the pass read nothing. Nothing is DELETED from
//!   a topic by being read, so that one count of the family is not a quantity this source has
//!   at all — the same difference a shard source has from a queue.
//!
//! At most ONE poller runs per mapping ([`drive_kafka_pollers`], armed by the calls that make
//! or change a mapping), through the same per-mapping claim every other reader takes
//! ([`PollerClaim`]), and it stands down as soon as its mapping stops wanting to be polled.
//!
//! The ONE Kafka source this reader consumes is a cluster of THIS account, which is the one
//! an `EventSourceArn` of the `cluster` resource type names: the other form the member
//! documents is "the ARN of the VPC connection (for cross-account event source mappings)",
//! whose cluster belongs to another account and so is held in no store this emulator has, and
//! a self-managed Apache Kafka cluster is named by bootstrap servers rather than by a
//! resource of this emulator at all. A mapping on either is left untouched here rather than
//! consumed from a log that is not the one it names.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ::serde_json::{json, Map, Value};
use ::service_com_amazonaws_kafka_kafka::state as kafka;
use ::service_com_amazonaws_kafka_kafka::state::broker_plane::{
    AccessPath, RecordHeaders, StoredRecord,
};

use super::event_source_poller::{Clients, PollerClaim};
use super::metrics::{EventCounts, FunctionMetrics};
use super::{
    batch_is_due, service_state, ComAmazonawsLambdaAwsgirapiserviceState, ShardCursor,
    EVENT_SOURCE_MAPPING_STATE_ENABLED, EVENT_SOURCE_RESOURCE_CLUSTER,
    EVENT_SOURCE_VENDOR_KAFKA, LAST_PROCESSING_RESULT_CONNECTION_ERROR,
    LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED, LAST_PROCESSING_RESULT_NO_RECORDS,
    LAST_PROCESSING_RESULT_OK, POLL_INTERVAL, STATE_ACTIVE,
};

/// The `eventSource` the event a Kafka mapping hands its function carries.
const KAFKA_EVENT_SOURCE: &str = "aws:kafka";

/// The batching window a mapping on a Kafka source that states none gathers over: "For Amazon
/// MSK, Self-managed Apache Kafka, Amazon MQ, and Amazon DocumentDB event sources, the default
/// batching window is 500 ms"
/// (`CreateEventSourceMappingInput$MaximumBatchingWindowInSeconds`) — the same sentence the
/// reader of a broker's queue takes its own default from ([`super::mq_poller`]).
const DEFAULT_BATCHING_WINDOW: Duration = Duration::from_millis(500);

/// The `timestampType` a record is reported with: whose clock its timestamp is — the
/// producer's, or the broker's for a topic whose `message.timestamp.type` is `LogAppendTime`
/// ("Topic-Level Configs", Apache Kafka documentation). They are the two values Apache Kafka's
/// own `TimestampType` has.
const TIMESTAMP_TYPE_CREATE: &str = "CREATE_TIME";
/// See [`TIMESTAMP_TYPE_CREATE`].
const TIMESTAMP_TYPE_LOG_APPEND: &str = "LOG_APPEND_TIME";

/// `SourceAccessConfiguration$Type` naming the SASL/SCRAM credential a Kafka mapping
/// authenticates with: "SASL_SCRAM_512_AUTH – The Secrets Manager ARN of your secret key used
/// for SASL SCRAM-512 authentication of your Apache Kafka brokers"
/// (`smithy_query(query="shape",shape="SourceAccessType")`).
const SOURCE_ACCESS_SASL_SCRAM: &str = "SASL_SCRAM_512_AUTH";

/// `SourceAccessConfiguration$Type` naming the client certificate one: "CLIENT_CERTIFICATE_TLS_AUTH
/// – (Amazon MSK, self-managed Apache Kafka) The Secrets Manager ARN of your secret key
/// containing the certificate chain" (same shape).
const SOURCE_ACCESS_CLIENT_CERTIFICATE: &str = "CLIENT_CERTIFICATE_TLS_AUTH";

/// What the client a mapping's consumer goes by in its group opens with: the service consuming
/// for it, before the identifier of the mapping it consumes for ([`consumer_client_of`]).
const CONSUMER_CLIENT_PREFIX: &str = "lambda-event-source-mapping-";

/// The most records one pass walks when it is still resolving WHERE an `AT_TIMESTAMP` position
/// starts — the records at the head of the log that the mapping's starting position excludes.
///
/// An EMULATOR bound, like the other bounds these readers carry: resolving that position is a
/// walk of the records before the instant, so it is bounded whatever a producer wrote, and the
/// records it walks past are checkpointed past at once — so the walk happens until the
/// mapping's position is an offset and never again.
const POSITION_SCAN_RECORDS: usize = 1_000;

/// One mapping's cluster as ONE pass reads it out of the store before it touches the log:
/// everything the pass needs, taken from the record under the store's lock and never held
/// across a call.
#[derive(Debug)]
struct ClusterTarget {
    /// `UUID`, the key the mapping is stored under.
    uuid: String,
    /// `EventSourceArn`, the cluster to consume, as the event reports it.
    cluster_arn: String,
    /// The Region that cluster lives in, which is the Region it is reached in: a resource
    /// belongs to one Region's namespace.
    region: String,
    /// `Topics`: "the name of the Kafka topic", each of whose partitions this mapping reads.
    topics: Vec<String>,
    /// `FunctionArn`: "The ARN of the Lambda function" the batch is handed to.
    function_arn: String,
    /// `BatchSize`: "the maximum number of records in each batch that Lambda pulls from your
    /// stream or queue", clamped to what this emulator reads in one go.
    batch_size: usize,
    /// The window one batch is gathered over ([`batching_window`]).
    batching_window: Duration,
    /// The `Pattern` of each of the mapping's `FilterCriteria` filters, as stored.
    filters: Vec<String>,
    /// Whether the mapping's `FunctionResponseTypes` asks for the function's report of which
    /// records of the batch failed to be read (see [`super::batch_item_failures`]).
    reports_batch_item_failures: bool,
    /// Whether the mapping's `MetricsConfig` asks for the per-mapping event counts
    /// ([`super::metrics::publishes_event_counts`]).
    meters_event_counts: bool,
    /// Which published access method this mapping's consumer reaches the cluster's brokers
    /// over, which is what the event's `bootstrapServers` names the addresses of
    /// ([`consumed_paths`]).
    access: ConsumedAccess,
    /// The consumer group this mapping consumes the cluster as ([`consumer_group_of`]).
    consumer_group: String,
    /// The client this mapping's consumer goes by in that group, which is what tells one
    /// consumer of a cluster from another ([`consumer_client_of`]).
    consumer_client: String,
    /// Where the mapping reads next in each partition it has already read from, keyed as
    /// [`partition_key`] names one.
    cursors: BTreeMap<String, ShardCursor>,
    /// Where it reads a partition it has not read from yet.
    initial_cursor: ShardCursor,
}

/// How a mapping's consumer AUTHENTICATES to the cluster's brokers, which is what selects the
/// access method it reaches them over.
///
/// "Lambda ... uses IAM access control to authenticate with your Amazon MSK cluster" unless
/// the mapping names a credential of its own: `SourceAccessConfigurations` is "an array of the
/// authentication protocol, VPC components, or virtual host to secure and define your event
/// source", and the two Kafka credentials it may carry each name the scheme the consumer then
/// uses ([`SOURCE_ACCESS_SASL_SCRAM`], [`SOURCE_ACCESS_CLIENT_CERTIFICATE`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConsumedAccess {
    /// Amazon MSK's IAM access control, which is what a mapping naming no credential uses.
    Iam,
    /// SASL/SCRAM with the secret the mapping names.
    SaslScram,
    /// TLS with the client certificate the mapping names.
    Certificate,
}

/// The cluster's data plane as ONE pass sees it: where the brokers are published, and which
/// partitions of the mapping's topics there are to read.
#[derive(Debug)]
struct ClusterPlane {
    /// "A string containing one or more hostname:port pairs": the addresses this emulator
    /// serves the brokers the consumer reaches them at, which is what the event reports as
    /// `bootstrapServers`; `None` while the cluster publishes no address for that method.
    bootstrap_servers: Option<String>,
    /// Every partition of every topic the mapping names that the cluster holds, as
    /// `(topic, partition)`.
    partitions: Vec<(String, i32)>,
}

/// What ONE poll loop carries between its wake-ups: the clients it invokes through, when the
/// batch each partition is gathering was first seen, and the last minute it published a
/// 0-valued read count in. Nothing in it is state a client can read.
#[derive(Default)]
struct PollSession {
    clients: Clients,
    /// For each partition ([`partition_key`]), the instant a pass first saw a record of the
    /// batch it is gathering — what the batching window is measured from.
    opened: BTreeMap<String, SystemTime>,
    /// The member this consumer holds in its mapping's consumer group, once it has joined one
    /// ([`joined`]); empty while it holds none, which is what a commit quoting no member is.
    member: String,
    /// The minute (epoch seconds divided by sixty) this poller last published a 0-valued read
    /// count in, so such a datapoint is published once per minute at most.
    idle_minute: Option<i64>,
}

impl ::std::fmt::Debug for PollSession {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter
            .debug_struct("PollSession")
            .field("opened", &self.opened)
            .field("member", &self.member)
            .field("idle_minute", &self.idle_minute)
            .finish_non_exhaustive()
    }
}

/// What one partition's pass came to.
#[derive(Debug)]
enum PartitionPass {
    /// Nothing was handed over: the partition held no record past the mapping's position, or
    /// the batch it holds is still gathering.
    Idle,
    /// Records were HANDLED — delivered to the function, or rejected by the mapping's criteria
    /// — and the mapping's position moved past them.
    Handled(EventCounts),
    /// The pass could not act: the partition could not be read, or the function did not
    /// process the batch it was handed, so the position did not move and the records are read
    /// again.
    Failed {
        /// What went wrong, for the one log line a failing mapping writes.
        detail: String,
        /// How much of the batch was read before it could not be handed over, which the pass
        /// still METERS: those records were taken out of the log by this pass.
        counts: EventCounts,
        /// Whether what failed was the FUNCTION's processing of a batch that was read and
        /// handed over, rather than a read the pass could not make — the two are reported as
        /// different processing results.
        undelivered: bool,
    },
}

/// Start the in-process poller of every Amazon MSK mapping of this engine's store that wants
/// one, for the mappings that have none running yet.
///
/// Called by the operations that make or change a mapping, AFTER they have released the
/// store's lock. At most ONE poller runs per MAPPING (the store carries that claim,
/// [`ComAmazonawsLambdaAwsgirapiserviceState::claim_event_source_poller`]) and each of them
/// consumes its own mapping's topics alone, so what one mapping's function takes to answer is
/// no part of what another mapping's records wait for. A poller stands down as soon as its
/// mapping stops wanting to be polled, so none of them outlives the mapping it polls; the
/// store holds at most [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings, so the tasks this can
/// ever spawn are bounded by that count. With no async runtime to spawn onto there is no
/// poller and nothing is consumed.
pub fn drive_kafka_pollers<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &Arc<Eng>,
) {
    drive_kafka_pollers_of(ctx, engine, None);
}

/// The same, over the mapping `only` names alone when it names one — the selection the caller
/// that arms the pollers of the mappings it REPORTED hands down
/// ([`super::arm_reported_mapping_pollers`]) — and over every cluster mapping of the store when
/// it names none.
pub fn drive_kafka_pollers_of<Eng: Send + Sync + 'static>(
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
    let claimed: Vec<PollerClaim> = {
        let mut store = state.lock();
        wanted_cluster_mappings(&store)
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
        // because the handle does not change while it runs.
        let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&*engine));
        runtime.spawn(async move {
            poll_one_mapping(&callback, claim, &*engine, metrics).await;
        });
    }
}

/// The poll loop of ONE mapping: a pass of its own, [`POLL_INTERVAL`] later another, until it
/// stops wanting to be polled.
async fn poll_one_mapping<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    engine: &Eng,
    metrics: FunctionMetrics,
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
        let wanted = {
            let mut store = state.lock();
            if mapping_wants_polling(&store, &claim.uuid) {
                true
            } else {
                claim.release(&mut store);
                false
            }
        };
        if !wanted {
            // A consumer that stands down leaves the group it was reading as, so the group it
            // leaves empty reports itself empty — what a mapping whose polling was paused, or
            // that is gone, has done to its consumer ([`stand_down`]).
            stand_down(&mut session, engine, &state, &claim.uuid);
            return;
        }
        poll_pass(
            callback,
            &mut session,
            engine,
            &metrics,
            &claim.uuid,
            SystemTime::now(),
        )
        .await;
    }
}

/// Take this poller's consumer out of the group it was reading as, if it joined one.
///
/// A consumer that stops reading is no member of its group any more: the mapping it reads for
/// has paused polling or is gone, so the group is told what a `LeaveGroup` tells it rather than
/// left holding a member nothing is behind — and the offsets that member committed stay the
/// group's, which is what the mapping resumes from.
fn stand_down<Eng: Send + Sync + 'static>(
    session: &mut PollSession,
    engine: &Eng,
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: &str,
) {
    if session.member.is_empty() {
        return;
    }
    let member = ::std::mem::take(&mut session.member);
    // The group and the cluster this consumer read as, as the mapping still states them; a
    // mapping that is gone has no group left to leave.
    let read_as = {
        let store = state.lock();
        store.event_source_mappings.get(uuid).and_then(|mapping| {
            mapping
                .event_source_arn
                .clone()
                .map(|cluster| (cluster, consumer_group_of(mapping)))
        })
    };
    let Some((cluster_arn, group)) = read_as else {
        return;
    };
    if let Some(clusters) = kafka::service_state(engine) {
        clusters
            .lock()
            .data_plane_leave_group(&cluster_arn, &group, &member, SystemTime::now());
    }
}

/// ONE pass over the mapping `uuid` names, at `now`: the whole of what such a mapping does
/// with no client action at all, as this module's documentation states it.
///
/// No lock is held across an invocation: each step takes one store's lock, finishes with it
/// and releases it before the next step takes another.
async fn poll_pass<Eng: Send + Sync + 'static>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    engine: &Eng,
    metrics: &FunctionMetrics,
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
    // The cluster's own log, as the service that owns clusters holds it. A cluster this
    // mapping cannot reach, or one holding none of the topics it names, is a connect failure
    // and no attempt at a batch: nothing is consumed, and the pass after it decides afresh.
    let plane = kafka::service_state(engine)
        .and_then(|clusters| cluster_plane(&clusters, &target, now).map(|plane| (clusters, plane)));
    let Some((clusters, plane)) = plane else {
        record_attempt(&state, &target, LAST_PROCESSING_RESULT_CONNECTION_ERROR);
        return;
    };
    let mut counts = EventCounts::default();
    let mut failure: Option<(String, bool)> = None;
    let mut handled = false;
    // The consumer this pass reads as is a MEMBER of the mapping's own consumer group, holding
    // the partitions it reads: the group is joined on the first pass and its session renewed on
    // every one after it ([`joined`]).
    joined(session, &clusters, &target, &plane, now);
    for (topic, partition) in &plane.partitions {
        let pass = poll_partition(
            callback, session, &state, &clusters, &target, &plane, topic, *partition, now,
        )
        .await;
        match pass {
            PartitionPass::Idle => {}
            PartitionPass::Handled(read) => {
                handled = true;
                counts = merged(counts, read);
            }
            PartitionPass::Failed {
                detail,
                counts: read,
                undelivered,
            } => {
                counts = merged(counts, read);
                if failure.is_none() {
                    failure = Some((detail, undelivered));
                }
            }
        }
    }
    meter(metrics, session, &target, &counts, now);
    match failure {
        Some((detail, undelivered)) => {
            ::tracing::warn!(
                mapping = %target.uuid,
                cluster = %target.cluster_arn,
                "{detail}, so the records it holds are read again"
            );
            let result = match undelivered {
                true => LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
                false => LAST_PROCESSING_RESULT_CONNECTION_ERROR,
            };
            record_attempt(&state, &target, result);
        }
        None if handled => record_attempt(&state, &target, LAST_PROCESSING_RESULT_OK),
        // Every partition was there and held nothing past the mapping's position: the pass
        // processed no record, which is what the mapping reports until one of them attempts a
        // batch — and what CLEARS a connect failure recorded while the topic was not there
        // yet.
        None => record_attempt(&state, &target, LAST_PROCESSING_RESULT_NO_RECORDS),
    }
}

/// Join the mapping's consumer group as the member this poller's consumer is, or renew the
/// session of the member it already holds — which is what each pass does, as a consumer that
/// keeps reading keeps heartbeating.
///
/// The subscription it joins with is the topics the mapping names, and the assignment it holds
/// is the partitions this pass reads: a mapping is the consumer of its own group, so every
/// partition of its topics is its own. A group another member already holds admits it as no
/// member ([`kafka::ComAmazonawsKafkaKafkaState::data_plane_join_group`]), and this poller then
/// commits nothing into it: the position such a group holds is the one ITS members committed.
fn joined(
    session: &mut PollSession,
    clusters: &::engine::ServiceState<kafka::ComAmazonawsKafkaKafkaState>,
    target: &ClusterTarget,
    plane: &ClusterPlane,
    now: SystemTime,
) {
    let mut assigned: BTreeMap<String, Vec<i32>> = BTreeMap::new();
    for (topic, partition) in &plane.partitions {
        assigned.entry(topic.clone()).or_default().push(*partition);
    }
    let assigned: Vec<(String, Vec<i32>)> = assigned.into_iter().collect();
    let member = clusters.lock().data_plane_join_group(
        &target.cluster_arn,
        &target.consumer_group,
        &target.consumer_client,
        &target.topics,
        &assigned,
        now,
    );
    session.member = member.unwrap_or_default();
}

/// ONE pass over one partition: read it from the mapping's position, hand over what is due,
/// and move the position past what the pass handled.
#[allow(clippy::too_many_arguments)]
async fn poll_partition(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    session: &mut PollSession,
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    clusters: &::engine::ServiceState<kafka::ComAmazonawsKafkaKafkaState>,
    target: &ClusterTarget,
    plane: &ClusterPlane,
    topic: &str,
    partition: i32,
    now: SystemTime,
) -> PartitionPass {
    let key = partition_key(topic, partition);
    let read = match read_partition(clusters, target, topic, partition, &key, now) {
        Ok(read) => read,
        Err(detail) => {
            return PartitionPass::Failed {
                detail,
                counts: EventCounts::default(),
                undelivered: false,
            }
        }
    };
    let member = session.member.clone();
    // The records the mapping's STARTING POSITION excludes are handled by being excluded: no
    // invocation will ever be handed them, so the position moves past them at once — which is
    // also what keeps the walk that found them from being made again.
    if let Some(offset) = read.excluded_through {
        checkpoint(
            mappings, clusters, target, &member, topic, partition, &key, offset,
        );
    }
    if read.records.is_empty() {
        session.opened.remove(&key);
        return PartitionPass::Idle;
    }
    // A batch that is neither full nor out of time is held: the log keeps its records and the
    // position is not moved, so the next pass reads them again with whatever arrived
    // meanwhile.
    let opened = *session.opened.entry(key.clone()).or_insert(now);
    let waited = now.duration_since(opened).unwrap_or_default();
    if !batch_is_due(
        read.records.len(),
        target.batch_size,
        waited,
        target.batching_window,
    ) {
        return PartitionPass::Idle;
    }
    session.opened.remove(&key);
    let polled = count_of(read.records.len());
    let criteria = super::event_source_filter::Filters::of(&target.uuid, &target.filters);
    let mut budget = super::event_source_filter::budget();
    let mut invoking: Vec<StoredRecord> = Vec::new();
    let mut rejected: BTreeSet<i64> = BTreeSet::new();
    for record in &read.records {
        if criteria.admits(&filter_event(topic, partition, record), &mut budget) {
            invoking.push(record.clone());
        } else {
            rejected.insert(record.offset);
        }
    }
    let filtered_out = count_of(rejected.len());
    if invoking.is_empty() {
        // The criteria rejected every record of the batch: the pass handled the whole batch
        // and there is no invocation to make, so the position moves past it.
        if let Some(last) = read.records.last() {
            checkpoint(
                mappings,
                clusters,
                target,
                &member,
                topic,
                partition,
                &key,
                last.offset,
            );
        }
        return PartitionPass::Handled(EventCounts {
            polled,
            filtered_out,
            invoked: 0,
            deleted: None,
        });
    }
    let identifiers: Vec<String> = invoking
        .iter()
        .map(|record| record.offset.to_string())
        .collect();
    let payload = kafka_event(target, plane, topic, partition, &invoking);
    let client = session.clients.lambda(callback, &target.region);
    let attempt = hand_over(client, target, &payload, &identifiers).await;
    let mut handled: BTreeSet<i64> = rejected;
    handled.extend(attempt.processed.iter().copied());
    let through = read
        .records
        .iter()
        .map(|record| record.offset)
        .take_while(|offset| handled.contains(offset))
        .last();
    if let Some(offset) = through {
        checkpoint(
            mappings, clusters, target, &member, topic, partition, &key, offset,
        );
    }
    let counts = EventCounts {
        polled,
        filtered_out,
        invoked: count_of(attempt.processed.len()),
        deleted: None,
    };
    match attempt.failure {
        Some(detail) => PartitionPass::Failed {
            detail,
            counts,
            undelivered: true,
        },
        None => PartitionPass::Handled(counts),
    }
}

/// What one partition's read came to: the records to hand over, and the position the mapping's
/// own starting position excludes everything up to.
#[derive(Debug)]
struct PartitionRead {
    /// The records of this batch, in the order the log holds them: at most `BatchSize` of
    /// them, from the mapping's position on.
    records: Vec<StoredRecord>,
    /// The offset of the last record the mapping's STARTING POSITION excludes, when it
    /// excluded any — the records at the head of the log that lie before the instant an
    /// `AT_TIMESTAMP` position names.
    excluded_through: Option<i64>,
}

/// Read one partition from the position the mapping carries for it, through the SAME data-plane
/// reading the cluster's own wire protocol serves a `Fetch` from — so what this consumer takes
/// is what any other consumer of that partition reads, with the records the topic's retention
/// has aged out already gone.
fn read_partition(
    clusters: &::engine::ServiceState<kafka::ComAmazonawsKafkaKafkaState>,
    target: &ClusterTarget,
    topic: &str,
    partition: i32,
    key: &str,
    now: SystemTime,
) -> Result<PartitionRead, String> {
    let cursor = target
        .cursors
        .get(key)
        .unwrap_or(&target.initial_cursor)
        .clone();
    // Where the read starts, and the instant the mapping's own position excludes the records
    // before: an offset position reads on from past the record it names, and both of the
    // positions stated as a place in TIME read the log from its start and then skip the
    // records that lie before them.
    let (from, before) = match &cursor {
        ShardCursor::AfterSequenceNumber(text) => (
            text.parse::<i64>()
                .map(|offset| offset.saturating_add(1))
                .unwrap_or_default(),
            None,
        ),
        ShardCursor::TrimHorizon => (0, None),
        ShardCursor::AtTimestamp(instant) => (0, Some(kafka::broker_plane::epoch_millis(*instant))),
    };
    // A position stated in time is resolved by WALKING the records before it, so that read
    // takes the bounded window ([`POSITION_SCAN_RECORDS`]) rather than one batch.
    let wanted = match before {
        Some(_) => POSITION_SCAN_RECORDS.max(target.batch_size),
        None => target.batch_size,
    };
    let fetched = {
        let mut store = clusters.lock();
        store
            .data_plane_fetch(&target.cluster_arn, topic, partition, from, wanted, now)
            .map_err(|refusal| {
                format!("{topic}-{partition} of {} could not be read ({refusal:?})", target.cluster_arn)
            })?
    };
    let mut records = fetched.records;
    // The records this read surfaced NONE of are handled by being passed over: a
    // transaction's control batch and a record an abort discarded are not records any
    // consumer is handed, so the position moves past them exactly as a client library
    // moves its own past them — otherwise a window made only of them would be read again
    // on every pass and the partition would never move.
    let mut excluded_through = fetched.undelivered_through;
    if let Some(instant) = before {
        let excluded = records
            .iter()
            .take_while(|record| record.timestamp_ms < instant)
            .count();
        if excluded > 0 {
            excluded_through = records.get(excluded - 1).map(|record| record.offset);
            records.drain(..excluded);
        }
    }
    records.truncate(target.batch_size);
    Ok(PartitionRead {
        records,
        excluded_through,
    })
}

/// What ONE attempt at one batch came to.
#[derive(Debug)]
struct Attempt {
    /// The offsets of the records the attempt PROCESSED.
    processed: Vec<i64>,
    /// Why the attempt processed no record of the batch, when it processed none.
    failure: Option<String>,
}

/// Hand one batch to the function, synchronously, as a Kafka event source's invocation is, and
/// read what became of the records it held: the ones the attempt processed, or why none of them
/// was.
///
/// Every record of a batch an invocation accepted was processed unless the mapping asks for its
/// function's report of which of them failed, which is what `FunctionResponseTypes` says (see
/// [`super::batch_item_failures`]).
async fn hand_over(
    client: &::aws_sdk_lambda::Client,
    target: &ClusterTarget,
    payload: &Value,
    identifiers: &[String],
) -> Attempt {
    let answer = client
        .invoke()
        .function_name(&target.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await;
    let answer = match answer {
        Ok(answer) => answer,
        Err(error) => {
            return Attempt {
                processed: Vec::new(),
                failure: Some(format!(
                    "{} could not be invoked ({})",
                    target.function_arn,
                    ::aws_smithy_types::error::display::DisplayErrorContext(&error)
                )),
            }
        }
    };
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed, so the position does
    // not move past it.
    if let Some(failure) = answer.function_error.as_deref() {
        return Attempt {
            processed: Vec::new(),
            failure: Some(format!(
                "{} answered a {failure} error for the batch",
                target.function_arn
            )),
        };
    }
    if !target.reports_batch_item_failures {
        return Attempt {
            processed: offsets_of(identifiers),
            failure: None,
        };
    }
    let payload = answer.payload.map(::aws_smithy_types::Blob::into_inner);
    match super::read_batch_item_failures(payload.as_deref(), identifiers) {
        super::BatchReport::WholeBatchFailed(detail) => Attempt {
            processed: Vec::new(),
            failure: Some(format!(
                "{} reported no record of the batch as processed: {detail}",
                target.function_arn
            )),
        },
        report => Attempt {
            processed: offsets_of(
                &super::processed_records(&report, identifiers).unwrap_or_default(),
            ),
            failure: None,
        },
    }
}

/// The offsets `identifiers` name: a Kafka record is named in a batch report by its OFFSET, so
/// each identifier is read back as the offset it was written from.
fn offsets_of(identifiers: &[String]) -> Vec<i64> {
    identifiers
        .iter()
        .filter_map(|identifier| identifier.parse::<i64>().ok())
        .collect()
}

/// Two passes' counts added together, so what the pass as a whole did with the records it read
/// is one measurement however many partitions it read.
fn merged(held: EventCounts, read: EventCounts) -> EventCounts {
    EventCounts {
        polled: held.polled.saturating_add(read.polled),
        filtered_out: held.filtered_out.saturating_add(read.filtered_out),
        invoked: held.invoked.saturating_add(read.invoked),
        // Nothing is removed from a topic by being read, so this source has no deleted count
        // at all rather than a zero.
        deleted: None,
    }
}

/// A count of records as a metric carries it.
fn count_of(records: usize) -> u32 {
    u32::try_from(records).unwrap_or(u32::MAX)
}

/// Publish what this pass did with the records it read, while the mapping's `MetricsConfig`
/// asks for it — including a 0-valued read count for a minute in which the pass read nothing,
/// published once per minute, which is the resolution such a series is aggregated at.
fn meter(
    metrics: &FunctionMetrics,
    session: &mut PollSession,
    target: &ClusterTarget,
    counts: &EventCounts,
    now: SystemTime,
) {
    if !target.meters_event_counts {
        return;
    }
    let at_secs = now
        .duration_since(::std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    if counts.polled > 0 {
        metrics.publish_event_counts(&target.uuid, counts, at_secs);
        return;
    }
    const SECONDS_PER_MINUTE: f64 = 60.0;
    let minute = (at_secs / SECONDS_PER_MINUTE).trunc() as i64;
    if session.idle_minute == Some(minute) {
        return;
    }
    session.idle_minute = Some(minute);
    metrics.publish_event_counts(&target.uuid, counts, at_secs);
}

/// Record what one processing attempt came to on the mapping itself.
///
/// The record is re-read under the lock and only written when it still names the cluster and
/// the function this batch was polled for, so a mapping re-targeted while the batch was in
/// flight does not report an attempt made under a configuration it no longer carries.
///
/// And only while that mapping still WANTS polling: `LastProcessingResult` is "the result of
/// the event source mapping's last processing attempt", a reading of the poller making those
/// attempts, and "when false, Lambda pauses polling and invocation" — so a mapping a caller
/// paused while this pass was in flight reports no attempt at all
/// ([`super::EventSourceMappingRecord::begin_update`] is what clears it), rather than the
/// result of the pass that was already running when the pause landed.
fn record_attempt(
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    target: &ClusterTarget,
    result: &str,
) {
    let mut store = mappings.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(&target.uuid) else {
        return;
    };
    if !mapping_still_polls(mapping, target) || !wants_polling(mapping) {
        return;
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// Move the mapping's position for one partition past the record `offset` names, which is what
/// makes the records behind it the ones the next pass reads — and COMMIT that position into the
/// consumer group this mapping consumes the cluster as, which is where every other reader of
/// the cluster sees how far it has got ([`kafka::ComAmazonawsKafkaKafkaState::data_plane_commit_offset`]).
///
/// A committed offset is the one a consumer reads FROM next, so what is committed is the offset
/// past the record the mapping has handled.
///
/// Written under the same reading as [`record_attempt`]: a mapping re-targeted while the batch
/// was in flight keeps the position it carries for the source it now names.
#[allow(clippy::too_many_arguments)]
fn checkpoint(
    mappings: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    clusters: &::engine::ServiceState<kafka::ComAmazonawsKafkaKafkaState>,
    target: &ClusterTarget,
    member: &str,
    topic: &str,
    partition: i32,
    key: &str,
    offset: i64,
) {
    {
        let mut store = mappings.lock();
        let Some(mapping) = store.event_source_mappings.get_mut(target.uuid.as_str()) else {
            return;
        };
        if !mapping_still_polls(mapping, target) {
            return;
        }
        mapping.shard_cursors.insert(
            key.to_string(),
            ShardCursor::AfterSequenceNumber(offset.to_string()),
        );
    }
    let mut clusters = clusters.lock();
    clusters.data_plane_commit_offset(
        &target.cluster_arn,
        &kafka::broker_plane::CommittedPosition {
            group_id: &target.consumer_group,
            member_id: member,
            topic,
            partition,
            // A committed offset is the one a consumer reads FROM next, so what is committed
            // is the offset past the record this pass handled.
            next_offset: offset.saturating_add(1),
        },
        SystemTime::now(),
    );
}

/// Whether `mapping` still names the cluster and the function the pass read for.
fn mapping_still_polls(mapping: &super::EventSourceMappingRecord, target: &ClusterTarget) -> bool {
    mapping.event_source_arn.as_deref() == Some(target.cluster_arn.as_str())
        && mapping.function_arn == target.function_arn
}

/// The cluster `target` names, as the service that owns clusters holds it at `now` — `None`
/// when this engine holds no cluster store, when the cluster is gone, when it is not `ACTIVE`
/// yet, or when it holds none of the topics the mapping names.
///
/// A cluster that is still being provisioned answers no client on its brokers, so there is
/// nothing for this consumer to read yet; the pass after the move the clock owes finds it
/// active. A topic the cluster does not hold is nothing this consumer can consume either, and
/// a pass that finds the topic created later reads it.
fn cluster_plane(
    clusters: &::engine::ServiceState<kafka::ComAmazonawsKafkaKafkaState>,
    target: &ClusterTarget,
    now: SystemTime,
) -> Option<ClusterPlane> {
    let mut store = clusters.lock();
    let state = store
        .settled_cluster_in(&target.region, &target.cluster_arn, now)
        .map(|cluster| cluster.state.clone())?;
    if state != kafka::CLUSTER_STATE_ACTIVE {
        return None;
    }
    let bootstrap_servers = published_brokers(&store, target);
    let held = store.data_plane_topics(&target.cluster_arn, now);
    let mut partitions: Vec<(String, i32)> = Vec::new();
    for topic in &target.topics {
        let Some(found) = held.iter().find(|candidate| candidate.name == *topic) else {
            // A topic the cluster does not hold makes the pass a connect failure, so the
            // mapping reports that it could not reach its source rather than that it read an
            // empty log.
            return None;
        };
        partitions.extend((0..found.partitions).map(|partition| (topic.clone(), partition)));
    }
    Some(ClusterPlane {
        bootstrap_servers,
        partitions,
    })
}

/// The addresses this emulator publishes the brokers at that `target`'s consumer reaches them
/// over: every `host:port` pair of the first access method the cluster publishes of the ones
/// that consumer uses ([`consumed_paths`]), comma-separated as a bootstrap string is.
///
/// `None` while the cluster publishes no address for any of them — an address nothing serves is
/// not reported.
fn published_brokers(
    store: &kafka::ComAmazonawsKafkaKafkaState,
    target: &ClusterTarget,
) -> Option<String> {
    for path in consumed_paths(target.access) {
        let published = store
            .broker_authorities(&target.region, &target.cluster_arn, *path)
            .unwrap_or_default();
        if !published.is_empty() {
            let addresses: Vec<String> = published
                .into_iter()
                .map(|(_node, authority)| authority)
                .collect();
            return Some(addresses.join(","));
        }
    }
    None
}

/// The published access methods a consumer authenticating as `access` reaches the cluster's
/// brokers over, in the order it would try them: the scheme its credential names, and then the
/// schemes a cluster that does not offer that one does — a cluster is reached on what it
/// publishes.
fn consumed_paths(access: ConsumedAccess) -> &'static [AccessPath] {
    match access {
        ConsumedAccess::Iam => &[
            AccessPath::SaslIam,
            AccessPath::Tls,
            AccessPath::Plaintext,
        ],
        ConsumedAccess::SaslScram => &[
            AccessPath::SaslScram,
            AccessPath::Tls,
            AccessPath::Plaintext,
        ],
        ConsumedAccess::Certificate => &[AccessPath::Tls, AccessPath::Plaintext],
    }
}

/// The event one batch is handed over as: `records`, keyed by the topic and partition the
/// records came from, holding one record per message in the order the log holds them, beside
/// the source the mapping names and the addresses its consumer reached the brokers at.
fn kafka_event(
    target: &ClusterTarget,
    plane: &ClusterPlane,
    topic: &str,
    partition: i32,
    records: &[StoredRecord],
) -> Value {
    let envelopes: Vec<Value> = records
        .iter()
        .map(|record| record_envelope(topic, partition, record))
        .collect();
    let mut by_partition = Map::new();
    by_partition.insert(partition_key(topic, partition), Value::Array(envelopes));
    let mut event = json!({
        "eventSource": KAFKA_EVENT_SOURCE,
        "eventSourceArn": target.cluster_arn.clone(),
        "records": Value::Object(by_partition),
    });
    if let Some(servers) = plane.bootstrap_servers.as_deref() {
        event["bootstrapServers"] = json!(servers);
    }
    event
}

/// How the event names one partition of one topic, which is also the key the mapping's own
/// position for that partition is held under: the topic and the partition index, as Lambda
/// keys a Kafka batch by.
fn partition_key(topic: &str, partition: i32) -> String {
    format!("{topic}-{partition}")
}

/// One stored record as the record of the event Lambda hands a function for a Kafka source:
/// where it sits in the log, when and by whose clock it was stamped, and its own bytes.
///
/// A record's key and value are bytes a producer wrote, which travel base64-encoded as the event
/// carries them. BOTH members are reported for every record — a record produced without a key,
/// and a tombstone, carry NO VALUE for the member rather than leaving it out — because a Kafka
/// record HAS a key and a value, either of which may be absent, and a handler reading the member
/// of such a record reads the absence rather than a record that does not carry it. Its headers
/// are reported in the order the producer wrote them, each as the bytes of its value under its
/// own name ([`headers_of`]).
fn record_envelope(topic: &str, partition: i32, record: &StoredRecord) -> Value {
    json!({
        "topic": topic,
        "partition": partition,
        "offset": record.offset,
        "timestamp": record.timestamp_ms,
        "timestampType": match record.log_append_time {
            true => TIMESTAMP_TYPE_LOG_APPEND,
            false => TIMESTAMP_TYPE_CREATE,
        },
        "key": bytes_of(record.key.as_deref()),
        "value": bytes_of(record.value.as_deref()),
        "headers": headers_of(&record.headers),
    })
}

/// The bytes a record's key or value carries, as the event carries them: base64-encoded text,
/// and no value at all for a member the record carries none for.
fn bytes_of(bytes: Option<&[u8]>) -> Value {
    bytes.map_or(Value::Null, |bytes| {
        json!(::aws_smithy_types::base64::encode(bytes))
    })
}

/// One record's headers as the event reports them: one entry per header, in the order the
/// producer wrote them, each naming the header and carrying its value's own BYTES — and no
/// value at all for a header written without one, which is a header a Kafka record may carry.
fn headers_of(headers: &RecordHeaders) -> Value {
    let entries: Vec<Value> = headers
        .iter()
        .map(|(name, value)| {
            let mut entry = Map::new();
            entry.insert(
                name.clone(),
                match value {
                    Some(bytes) => Value::Array(bytes.iter().map(|byte| json!(byte)).collect()),
                    None => Value::Null,
                },
            );
            Value::Object(entry)
        })
        .collect();
    Value::Array(entries)
}

/// One record as the FILTER sees it: the record the function receives, with the payload bytes it
/// carries base64-encoded read as the document those bytes spell where they spell one.
///
/// The criteria decide one record of the SOURCE, so a clause names the members one Kafka record
/// HAS — `topic`, `partition`, `offset`, `timestamp`, `timestampType`, `headers`, and the
/// record's own `key` and `value`. Those last two are bytes a producer put, and a clause for
/// either is written for the document those bytes spell: so each is read as that document where
/// they spell one — an object or a list, which is what a clause names fields inside — and left
/// as the text it travels as where they do not, a bare scalar and bytes that are no text at all
/// included, so a clause naming fields in such a payload admits the record no more than one
/// whose values disagreed. It is the rule every other source follows for a structured payload
/// ([`super::sqs_poller`], [`super::event_source_poller`]).
fn filter_event(topic: &str, partition: i32, record: &StoredRecord) -> Value {
    let mut event = record_envelope(topic, partition, record);
    if let Some(document) = document_of(record.key.as_deref()) {
        event["key"] = document;
    }
    if let Some(document) = document_of(record.value.as_deref()) {
        event["value"] = document;
    }
    event
}

/// The document `bytes` spell, and `None` when they spell no object or list.
fn document_of(bytes: Option<&[u8]>) -> Option<Value> {
    ::serde_json::from_slice::<Value>(bytes?)
        .ok()
        .filter(|document| document.is_object() || document.is_array())
}

/// The batching window a mapping on a Kafka source gathers ONE batch over, read inside the
/// range `MaximumBatchingWindowInSeconds` admits: the span the mapping states, and otherwise
/// this source's own default ([`DEFAULT_BATCHING_WINDOW`]).
fn batching_window(configured: Option<i32>) -> Duration {
    match configured {
        Some(_) => super::sqs_poller::batching_window(configured),
        None => DEFAULT_BATCHING_WINDOW,
    }
}

/// The cluster of the mapping `uuid` names, when that mapping is polling at this instant.
///
/// A mapping polls when it reports `Enabled` — "when true, the event source mapping is active.
/// When false, Lambda pauses polling and invocation" — its event source is an Amazon MSK
/// cluster of this account, it names at least one topic to consume, and the function it invokes
/// is one this store holds in [`STATE_ACTIVE`]: "Lambda reads items from the event source and
/// invokes the function", so a mapping whose function cannot be invoked yet reads nothing.
///
/// Read it only after [`ComAmazonawsLambdaAwsgirapiserviceState::materialize_all`].
fn polling_target(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    uuid: &str,
) -> Option<ClusterTarget> {
    let mapping = store.event_source_mappings.get(uuid)?;
    if mapping.state != EVENT_SOURCE_MAPPING_STATE_ENABLED || mapping.is_deleting() {
        return None;
    }
    let function = store.functions.get(&mapping.function_name)?;
    if function.state != STATE_ACTIVE {
        return None;
    }
    let cluster_arn = mapping.event_source_arn.clone()?;
    let region = named_cluster(&cluster_arn)?.to_string();
    // "The name of the Kafka topic": a mapping that names none has no topic to consume.
    let topics: Vec<String> = mapping
        .topics
        .clone()
        .unwrap_or_default()
        .into_iter()
        .filter(|topic| !topic.is_empty())
        .collect();
    if topics.is_empty() {
        return None;
    }
    Some(ClusterTarget {
        uuid: mapping.uuid.clone(),
        cluster_arn,
        region,
        topics,
        function_arn: mapping.function_arn.clone(),
        // "The maximum number of records in each batch that Lambda pulls from your stream or
        // queue": the size is the batch's ceiling, and a mapping always pulls at least one
        // record.
        batch_size: usize::try_from(mapping.batch_size)
            .unwrap_or(1)
            .clamp(1, super::sqs_poller::MAX_MESSAGES_PER_BATCH),
        batching_window: batching_window(mapping.maximum_batching_window_in_seconds),
        filters: super::event_source_filter::patterns_of(mapping.filter_criteria.as_ref()),
        reports_batch_item_failures: super::reports_batch_item_failures(
            &mapping.function_response_types,
        ),
        meters_event_counts: super::metrics::publishes_event_counts(
            mapping.metrics_config.as_ref(),
        ),
        access: consumed_access(mapping),
        consumer_group: consumer_group_of(mapping),
        consumer_client: consumer_client_of(mapping),
        cursors: mapping.shard_cursors.clone(),
        initial_cursor: mapping.initial_shard_cursor(),
    })
}

/// The CONSUMER GROUP this mapping consumes its cluster as.
///
/// `AmazonManagedKafkaEventSourceConfig$ConsumerGroupId` is "the identifier for the Kafka
/// consumer group to join. The consumer group ID must be unique among all your Kafka event
/// sources", so a mapping that states one consumes the cluster as THAT group; a mapping that
/// states none is consumed under an identifier of its own, which is what keeps one mapping's
/// committed position from being another's — and the identifier a mapping HAS is its `UUID`,
/// "the identifier of the event source mapping".
fn consumer_group_of(mapping: &super::EventSourceMappingRecord) -> String {
    mapping
        .amazon_managed_kafka_event_source_config
        .as_ref()
        .and_then(|configuration| configuration.consumer_group_id.clone())
        .filter(|group| !group.is_empty())
        .unwrap_or_else(|| mapping.uuid.clone())
}

/// The CLIENT this mapping's consumer goes by in its group: the identifier of the mapping doing
/// the consuming, under the name of the service consuming for it.
///
/// A reader of a group tells one member from another by the client each goes by, and what
/// distinguishes one consumer of a cluster from another here is the MAPPING it reads for:
/// `UUID` is "the identifier of the event source mapping", unique among them.
fn consumer_client_of(mapping: &super::EventSourceMappingRecord) -> String {
    format!("{CONSUMER_CLIENT_PREFIX}{}", mapping.uuid)
}

/// How this mapping's consumer authenticates to the cluster's brokers
/// ([`ConsumedAccess`]): the scheme the credential its `SourceAccessConfigurations` names uses,
/// and otherwise Amazon MSK's IAM access control.
fn consumed_access(mapping: &super::EventSourceMappingRecord) -> ConsumedAccess {
    for configuration in mapping
        .source_access_configurations
        .as_ref()
        .into_iter()
        .flatten()
    {
        match configuration.r#type.as_deref() {
            Some(SOURCE_ACCESS_SASL_SCRAM) => return ConsumedAccess::SaslScram,
            Some(SOURCE_ACCESS_CLIENT_CERTIFICATE) => return ConsumedAccess::Certificate,
            _ => {}
        }
    }
    ConsumedAccess::Iam
}

/// The Region of the Amazon MSK CLUSTER `arn` names, and `None` for a value naming no cluster of
/// this emulator at all.
///
/// The one Kafka source this reader consumes is a cluster of this account
/// ([`EVENT_SOURCE_RESOURCE_CLUSTER`]); the cross-account VPC-connection form belongs to
/// another account's cluster, which is held in no store here.
fn named_cluster(arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(arn)?;
    if parsed.vendor != EVENT_SOURCE_VENDOR_KAFKA
        || parsed.resource_type() != EVENT_SOURCE_RESOURCE_CLUSTER
    {
        return None;
    }
    Some(parsed.region)
}

/// Whether the mapping `uuid` names wants a poller of its own at this instant: it is a mapping
/// of this store, on an Amazon MSK cluster, that is `Enabled` and not being deleted.
///
/// A mapping still SETTLING into `Enabled` counts: the state it settles into is its `Enabled`
/// setting, and the clock owes that move. Whether its FUNCTION can be invoked yet, and whether
/// its cluster is active, are questions of each PASS and not of the poller's life.
fn mapping_wants_polling(store: &ComAmazonawsLambdaAwsgirapiserviceState, uuid: &str) -> bool {
    store
        .event_source_mappings
        .get(uuid)
        .is_some_and(wants_polling)
}

/// Every mapping of this store that wants a poller of its own ([`mapping_wants_polling`]).
fn wanted_cluster_mappings(store: &ComAmazonawsLambdaAwsgirapiserviceState) -> Vec<String> {
    store
        .event_source_mappings
        .values()
        .filter(|mapping| wants_polling(mapping))
        .map(|mapping| mapping.uuid.clone())
        .collect()
}

/// The ONE predicate behind both: the mapping consumes an Amazon MSK cluster's topics, is
/// `Enabled` and is not being deleted.
fn wants_polling(mapping: &super::EventSourceMappingRecord) -> bool {
    mapping.enabled
        && !mapping.is_deleting()
        && mapping
            .event_source_arn
            .as_deref()
            .and_then(named_cluster)
            .is_some()
}

#[cfg(test)]
mod tests;
