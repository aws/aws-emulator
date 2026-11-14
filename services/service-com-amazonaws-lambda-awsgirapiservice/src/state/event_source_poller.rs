// SPDX-License-Identifier: CC0-1.0

//! The DATA PLANE of an event source mapping on a SHARD source — an Amazon Kinesis data
//! stream or an Amazon DynamoDB table's stream: what such a mapping does with no client
//! action at all.
//!
//! A mapping's control plane (the record, its UUID and ARN, its lifecycle states) lives in
//! [`crate::state`]. This module is the other half for the stream sources whose records sit
//! in SHARDS, as [`crate::state::sqs_poller`] is for a mapping on a queue: once a mapping on
//! one of those streams is `Enabled`, Lambda itself reads that stream's shards and invokes
//! the mapped function with the batch it read — no caller asks it to. A queue needs a reader
//! of its own because what it hands over, and how a reader keeps its place in it, differ: a
//! queue hides a message it hands over for its own visibility timeout, while a shard hands
//! out the records after a POSITION the reader carries. The two shard sources share this
//! reader for the same reason — both are read per shard from a position the mapping carries
//! — and differ only in how their shards are listed and what one record is handed over as,
//! which is what [`kinesis_stream`] and [`dynamodb_stream`] each state for its own source.
//! The rules this reader follows:
//!
//! * WHILE PAUSED, NOTHING IS READ. "When true, the event source mapping is active. When
//!   false, Lambda pauses polling and invocation" (`Enabled`), so a `Disabled` mapping
//!   reads no record and invokes nothing, and the records put to the stream meanwhile stay
//!   in the shard for its retention period.
//! * THE POSITION IS THE MAPPING'S. `StartingPosition` is "The position in a stream from
//!   which to start reading", a property of the mapping fixed when the mapping is created,
//!   so pausing does not move it ([`crate::state::ShardCursor`]): a mapping enabled again
//!   reads on from where it had got to, which for one that read nothing yet is where its
//!   creation fixed it. That is what makes a backlog built up while it was paused
//!   OBSERVABLE rather than skipped.
//! * EACH BATCH READ IS METERED. The age of the newest record of the batch, at the instant
//!   the batch was read off the shard, is published as this function's
//!   `IteratorAge` ([`crate::state::metrics`]) — so a batch that sat in the
//!   shard while the mapping was paused meters that whole wait, and a batch read as its
//!   records arrive meters almost nothing.
//! * A BATCH IS CHECKPOINTED PAST ONLY ONCE DELIVERED. An invocation that did not succeed
//!   leaves the cursor where it was, so the same batch is read again — "The default value
//!   is infinite (-1). When set to infinite (-1), failed records are retried until the
//!   record expires" (`MaximumRetryAttempts`) — and what the attempt came to is reported as
//!   the mapping's own `LastProcessingResult`. A mapping whose `FunctionResponseTypes` asks
//!   for its function's report of which records of the batch failed is checkpointed past
//!   only the records at the head of the batch that report leaves out, so the first record
//!   it names is where the next read starts (see [`super::batch_item_failures`]).
//! * THE RETRIES A MAPPING ALLOWS ARE SPENT ON ONE BATCH. "Discard records after the
//!   specified number of retries" (`MaximumRetryAttempts`): a mapping that states a finite
//!   count gets the first attempt plus that many at one batch, and once they are spent the
//!   shard is read on from PAST those records — so the records behind a batch its function
//!   cannot process are still delivered. The count is a fact about the batch, so a shard
//!   that has moved on starts it again. Between two attempts at one batch the wait GROWS
//!   ([`undelivered_backoff`]), so a function that keeps failing is not invoked at the rate
//!   a healthy mapping polls.
//!
//! * NOTHING IS READ FOR A FUNCTION THAT CANNOT BE INVOKED. "Lambda reads items from the
//!   event source and invokes the function" (`CreateEventSourceMapping`), so a mapping
//!   whose function this store does not hold `Active` takes nothing off a shard — the same
//!   rule [`crate::state::sqs_poller`] holds a queue mapping to.
//! * THE SHARDS ARE READ AS THE FUNCTION'S EXECUTION ROLE. Lambda reads the source acting as
//!   that role, so while the policies in force on it withhold one of the reads consuming the
//!   stream takes, a pass reads nothing and no cursor moves
//!   ([`super::event_source_authorization`]) — the records stay in the shard until the grant
//!   is in place, which is the same question the call that WIRED the pair together was
//!   refused for, and again the rule [`crate::state::sqs_poller`] holds a queue mapping to.
//!
//! Each pass reads what the shard holds and hands it over at once, so a mapping that
//! configures a `MaximumBatchingWindowInSeconds` above the stream default of 0 is not
//! waited out here: that yields SMALLER batches than such a mapping would see on AWS,
//! never a record that is not delivered.
//!
//! Every call this module makes goes to the emulator it is running inside, over the
//! in-process callback ([`smithy_server_runtime::EmbeddedEmulator`]): the stream is read
//! through the data-plane surface the source publishes in this same emulator and the
//! function is invoked through its own (`Invoke`), so nothing is read or delivered that this
//! emulator does not serve itself, and a record is consumed only through the data plane a
//! client would consume it through.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::serde_json::{json, Value};

use super::{
    metrics::FunctionMetrics, ComAmazonawsLambdaAwsgirapiserviceState, ShardCursor, ShardSource,
};

/// How a DynamoDB table's stream is listed, read and handed over.
pub mod dynamodb_stream;
/// How a Kinesis data stream is listed, read and handed over.
pub mod kinesis_stream;

/// How long the poller waits before looking at an idle shard again. Short enough that a
/// record put on the stream is delivered promptly, long enough not to spin.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// How long it waits before looking at a mapping that is NOT reading again — one that
/// rests `Disabled`, or is still settling into the state its `Enabled` setting asks for.
/// Longer than [`POLL_INTERVAL`], because such a mapping has no shard to look at: the one
/// thing a pass does is read the mapping's own state, and nothing but a call can change
/// it.
const PAUSED_POLL_INTERVAL: Duration = Duration::from_millis(500);

/// How long the poller waits after a failed attempt before trying the batch again. It is
/// multiplied by the number of consecutive failures, up to [`MAX_RETRY_BACKOFF`].
const RETRY_BACKOFF: Duration = Duration::from_millis(250);

/// The longest the poller waits between two attempts at a batch it keeps failing to
/// deliver.
///
/// A mapping whose stream or function cannot be reached keeps trying — that is what lets
/// it recover when the resource comes back — but it does so at this interval rather than
/// at the rate a healthy mapping polls.
const MAX_RETRY_BACKOFF: Duration = Duration::from_secs(5);

/// The longest the poller waits between two attempts at a batch its FUNCTION did not
/// process, which the wait grows towards by doubling ([`undelivered_backoff`]).
///
/// An EMULATOR bound, not an AWS number: what AWS states is that such a batch is retried
/// until it is processed, it expires, or the mapping's `MaximumRetryAttempts` is spent, with
/// the wait between attempts growing. The ceiling is chosen so a function that keeps failing
/// is attempted at a minute scale rather than at the rate a healthy mapping polls, and so a
/// function that starts succeeding again is still tried within a minute.
const MAX_UNDELIVERED_BACKOFF: Duration = Duration::from_secs(60);

/// How often a mapping that keeps failing says so: the first failure, and every this many
/// after it, so one broken mapping cannot fill the log.
const FAILURE_LOG_INTERVAL: u32 = 20;

/// The most records this emulator reads in ONE batch, whatever `BatchSize` the mapping
/// configures.
///
/// This is the EMULATOR's bound, not an AWS number (`BatchSize` itself reaches 10000): a
/// batch is held in memory and the whole of it becomes one invocation payload, so the
/// count a caller chose is clamped before anything is read. A mapping configured above it
/// reads this many records per batch instead, which is a smaller batch and never a dropped
/// record.
const MAX_RECORDS_PER_BATCH: i32 = 1_000;

/// The most pages of a stream's shard listing one pass walks, so a stream cannot keep the
/// poller in its shard listing.
const MAX_SHARD_PAGES: usize = 10;

/// Start the in-process poller for the event source mapping `uuid` names, if one is not
/// already running for it.
///
/// Called by an operation that has created or updated the mapping, AFTER it has released
/// the store's lock. It changes nothing a caller is told: the poller observes the
/// mapping's own `State` and reads nothing until the clock has moved it to `Enabled`. With
/// no async runtime to spawn onto there is no poller and nothing is polled.
///
/// At most ONE task runs per mapping, and the store holds at most
/// [`super::MAX_EVENT_SOURCE_MAPPINGS`] mappings, so the tasks this can ever spawn are
/// bounded by that count. A task ends as soon as its mapping is gone or being deleted, so
/// none of them outlives the mapping it polls, and its claim is released however it ends
/// (see [`PollerClaim`]).
pub fn drive_mapping<Eng: Send + Sync + 'static>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    engine: &::std::sync::Arc<Eng>,
    uuid: &str,
) {
    let Some(state) = super::service_state(&**engine) else {
        return;
    };
    let Ok(runtime) = ::tokio::runtime::Handle::try_current() else {
        return;
    };
    {
        let mut store = state.lock();
        // The sources this module reads are the two whose records sit in shards; a mapping
        // on any other source has no reader here, so no task is started for it at all.
        let reads_shards = store
            .event_source_mappings
            .get(uuid)
            .and_then(|mapping| mapping.event_source_arn.clone())
            .as_deref()
            .and_then(super::event_source_vendor)
            .and_then(ShardSource::of_vendor)
            .is_some();
        if !reads_shards || !store.claim_event_source_poller(uuid) {
            return;
        }
    }
    let callback = ctx.callback().clone();
    // The CloudWatch store this mapping's work is published to, resolved once per poller
    // because the handle does not change.
    let metrics = FunctionMetrics::new(super::metrics::cloudwatch_state(&**engine));
    // The stores the decision about each pass's reads is made from, resolved once per poller
    // because the handles do not change.
    let policies = PolicyStores {
        iam: super::event_source_authorization::iam_state(&**engine),
        sources: super::event_source_resource_policy::SourcePolicies::of(&**engine),
    };
    let claim = PollerClaim {
        state: ::engine::ServiceState::clone(&state),
        uuid: uuid.to_string(),
        held: true,
    };
    runtime.spawn(async move {
        poll_mapping(&callback, claim, metrics, policies).await;
    });
}

/// The stores the decision about a pass's reads is made from: the IAM store the invoked
/// function's execution role is resolved against — an engine carrying none shows nothing
/// about the role — and the stores the EVENT SOURCE's own policy lives in, since a stream is
/// fenced by the policy attached to it whoever reads it, this poller included.
#[derive(Clone)]
struct PolicyStores {
    iam: Option<::engine::ServiceState<super::event_source_authorization::IamState>>,
    sources: super::event_source_resource_policy::SourcePolicies,
}

/// The claim of the one poller of a mapping, held for as long as that poller runs.
///
/// It is released by [`Self::release`] inside the critical section that decided the poller
/// is done, so a call that arrives afterwards starts a new poller. The `Drop` is the
/// backstop for every OTHER way the task can end — the runtime shutting down, or a panic
/// inside the poller — because a claim left behind would stop this mapping from ever being
/// polled again.
struct PollerClaim {
    state: ::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: String,
    /// Whether this claim is still the live one. Set to `false` by [`Self::release`], so
    /// the `Drop` cannot release a claim a LATER poller has since taken.
    held: bool,
}

impl PollerClaim {
    /// Release the claim through a store lock the caller already holds.
    fn release(&mut self, store: &mut ComAmazonawsLambdaAwsgirapiserviceState) {
        if self.held {
            store.release_event_source_poller(&self.uuid);
            self.held = false;
        }
    }
}

impl Drop for PollerClaim {
    fn drop(&mut self) {
        if self.held {
            let mut store = self.state.lock();
            store.release_event_source_poller(&self.uuid);
            self.held = false;
        }
    }
}

/// One record of a batch as this reader carries it, whichever source it was read from: how
/// the shard identifies it, when the source stamped it, and the envelope the function is
/// handed for it.
///
/// The envelope is the SOURCE's own form — a Kinesis record and a DynamoDB stream record
/// are handed to a function as different documents — built by the module that read it, so
/// everything the poller itself does (metering the batch, delivering it, checkpointing past
/// what was processed) is the one rule for both sources.
struct ShardRecord {
    /// "the sequence number of the data record in the shard": how a report of this source
    /// names one record, and what the mapping's cursor reads on from.
    sequence_number: String,
    /// When the source stamped the record, which is what the age of a batch is measured
    /// from; `None` for a record the source reports no instant for.
    arrived_at: Option<::aws_smithy_types::DateTime>,
    /// The envelope the function is handed for this record.
    envelope: Value,
}

/// One mapping's configuration as ONE pass reads it: everything the pass needs, taken from
/// the record under the store's lock and never held across a call.
#[derive(Debug)]
struct ShardSourcedMapping {
    /// Which source the shards below are read from.
    source: ShardSource,
    /// The stream the shards are read from.
    stream_arn: String,
    /// The Region that stream lives in, which is the Region it is reached in: a resource
    /// belongs to one Region's namespace.
    region: String,
    /// The function the batch is handed to, as the mapping resolved it.
    function_arn: String,
    /// That function's bare name, which is the dimension its metrics are published per.
    function_name: String,
    /// The execution role the invocation is attributed to, reported to the function as
    /// `invokeIdentityArn`; `None` when this store holds no configuration to read it from.
    invoke_identity_arn: Option<String>,
    /// How many records one batch holds at most, from `BatchSize`, clamped to what this
    /// emulator reads in one go ([`MAX_RECORDS_PER_BATCH`]).
    batch_size: i32,
    /// Whether the mapping's `FunctionResponseTypes` asks for the function's report of which
    /// records of the batch failed to be read (see [`super::batch_item_failures`]).
    reports_batch_item_failures: bool,
    /// How many attempts one batch of this mapping gets before its records are discarded,
    /// from `MaximumRetryAttempts`; `None` for the infinite default
    /// ([`attempts_allowed`]).
    attempts_allowed: Option<u32>,
    /// Where the mapping reads next in each shard it has already read from.
    cursors: BTreeMap<String, ShardCursor>,
    /// Where it reads a shard it has not read from yet.
    initial_cursor: ShardCursor,
}

/// What the poller may do on one pass, decided under the store's lock.
enum Pass {
    /// The mapping is `Enabled`: read what its shards hold under this configuration. Boxed
    /// because it is far larger than the other answers, which most passes are.
    Run(Box<ShardSourcedMapping>),
    /// The mapping is not reading: it is still settling into the state its `Enabled`
    /// setting asks for, it rests `Disabled`, which "pauses polling and invocation", or the
    /// function it invokes is not `Active`, so there is no invocation to read a batch for.
    /// Nothing is read and the poller stays, so the mapping resumes where it left off.
    Paused,
    /// The mapping is `Enabled` but names an event source this reader cannot read, and
    /// why. Nothing is read and the poller ends.
    Unserved(String),
    /// The mapping is gone or being deleted. The claim is already released.
    Stop,
}

/// What ONE pass over the mapping's shards came to.
enum Outcome {
    /// Every batch read was delivered, so there may be more waiting.
    Delivered,
    /// Every shard held nothing to read.
    Idle,
    /// A call failed, or an invocation did not succeed, for at least one shard. That
    /// shard was not checkpointed past, so its records are read again.
    Failed {
        /// What went wrong, for the one log line a failing mapping writes.
        detail: String,
        /// Whether what failed was the FUNCTION's processing of a batch that was read and
        /// handed over, rather than a read or a call this pass could not make. The two are
        /// retried on different ladders: a batch whose function keeps failing is a property
        /// of that batch, so the wait between attempts at it grows by doubling
        /// ([`undelivered_backoff`]), while a pass that could not act at all keeps looking
        /// at the interval a mapping waiting on a resource looks at ([`retry_backoff`]).
        undelivered: bool,
    },
}

/// Poll one mapping until it is gone.
///
/// The store's lock is taken for the decision of each pass and released before any call
/// leaves this process, so a request never waits on a delivery.
async fn poll_mapping(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    mut claim: PollerClaim,
    metrics: FunctionMetrics,
    policies: PolicyStores,
) {
    let uuid = claim.uuid.clone();
    // A handle of its own, so the store can be locked while the claim is borrowed.
    let state = ::engine::ServiceState::clone(&claim.state);
    let mut work = PollerWork::default();
    let mut failures = 0u32;
    loop {
        let pass = {
            let mut store = state.lock();
            // The state moves the clock has ALREADY made due, so a mapping whose
            // `Creating` span has lapsed is seen as the `Enabled` mapping it became.
            store.materialize_all(SystemTime::now());
            next_pass(&mut store, &mut claim)
        };
        match pass {
            Pass::Stop => return,
            Pass::Paused => ::tokio::time::sleep(PAUSED_POLL_INTERVAL).await,
            Pass::Unserved(detail) => {
                ::tracing::warn!(
                    mapping = %uuid,
                    detail = %detail,
                    "this emulator reads no event source of that kind, so the mapping's \
                     source is left untouched"
                );
                let mut store = state.lock();
                claim.release(&mut store);
                return;
            }
            Pass::Run(run) => {
                // Lambda reads the shards ACTING AS the function's execution role, so this
                // pass's reads are subject to the policies in force on that role AT THIS
                // INSTANT rather than when the pair was wired: while one of them is
                // withheld, nothing is read off a shard — no record is delivered and no
                // cursor moves — and the mapping resumes where it left off once the grant is
                // back (see [`super::event_source_authorization`], including the cases that
                // show nothing about a role and withhold nothing).
                if let Some(detail) = withheld_shard_read(&policies, &run, SystemTime::now()) {
                    failures = failures.saturating_add(1);
                    if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                        ::tracing::warn!(
                            mapping = %uuid,
                            attempt = failures,
                            detail = %detail,
                            "an event source mapping may not read its stream as its \
                             function's execution role; the records stay in the shard until \
                             that role is granted the reads"
                        );
                    }
                    ::tokio::time::sleep(retry_backoff(failures)).await;
                    continue;
                }
                match move_batches(
                    callback,
                    &mut work,
                    &state,
                    &metrics,
                    &policies,
                    &uuid,
                    &run,
                )
                .await
                {
                    Outcome::Delivered => failures = 0,
                    Outcome::Idle => {
                        failures = 0;
                        ::tokio::time::sleep(POLL_INTERVAL).await;
                    }
                    Outcome::Failed {
                        detail,
                        undelivered,
                    } => {
                        failures = failures.saturating_add(1);
                        if failures == 1 || failures.is_multiple_of(FAILURE_LOG_INTERVAL) {
                            ::tracing::warn!(
                                mapping = %uuid,
                                attempt = failures,
                                detail = %detail,
                                "an event source mapping could not move a batch from its \
                                 stream to its function; the records stay in the shard and \
                                 it tries again"
                            );
                        }
                        let waiting = if undelivered {
                            undelivered_backoff(failures)
                        } else {
                            retry_backoff(failures)
                        };
                        ::tokio::time::sleep(waiting).await;
                    }
                }
            }
        }
    }
}

/// Why the reads this pass would make on `run`'s stream are WITHHELD — from the execution
/// role of the function the mapping invokes, or by the policy attached to the stream itself
/// — or `None` when they are not, which includes every case that shows nothing about either
/// (see [`super::event_source_authorization`]).
///
/// Only the IAM store and the source's own are locked here, one at a time, and this
/// service's is not: the decision is taken between two passes rather than inside the
/// critical section either of them is decided in.
fn withheld_shard_read(
    policies: &PolicyStores,
    run: &ShardSourcedMapping,
    now: SystemTime,
) -> Option<String> {
    let role_arn = run.invoke_identity_arn.as_deref()?;
    super::event_source_authorization::unreadable_event_source_held(
        policies.iam.as_ref(),
        &policies.sources,
        role_arn,
        &run.stream_arn,
        now,
    )
}

/// How long to wait after `failures` consecutive failed attempts at one batch: it grows
/// with them, capped at [`MAX_RETRY_BACKOFF`].
fn retry_backoff(failures: u32) -> Duration {
    RETRY_BACKOFF
        .saturating_mul(failures.max(1))
        .min(MAX_RETRY_BACKOFF)
}

/// How long to wait after `failures` consecutive attempts at a batch the FUNCTION did not
/// process: it DOUBLES with each of them, capped at [`MAX_UNDELIVERED_BACKOFF`].
///
/// "When set to infinite (-1), failed records are retried until the record expires"
/// (`MaximumRetryAttempts`), so such a batch is attempted again however many times it has
/// already failed — but the wait between two attempts grows with them, so a function that
/// keeps failing one batch is not invoked at the rate a healthy mapping polls and the
/// failures it reports do not grow without bound over the span a caller watches them.
fn undelivered_backoff(failures: u32) -> Duration {
    let doubling = 1u32.checked_shl(failures.saturating_sub(1).min(u32::BITS - 1));
    match doubling {
        Some(factor) => RETRY_BACKOFF
            .saturating_mul(factor)
            .min(MAX_UNDELIVERED_BACKOFF),
        // Past what a doubling fits in: the wait has long since reached its ceiling.
        None => MAX_UNDELIVERED_BACKOFF,
    }
}

/// The WORKING state one poller task carries from pass to pass: how it reaches the rest of
/// this emulator, and what it has already tried.
///
/// It belongs to the task rather than to the store because every part of it is a fact about
/// the attempts THIS poller has made, not one a caller reads off the mapping.
#[derive(Default)]
struct PollerWork {
    /// The clients this data plane reaches the other services of this emulator through, built
    /// on first use and then reused.
    clients: Clients,
    /// How many attempts the batch at the head of each shard has had, keyed by shard id, so
    /// the mapping's `MaximumRetryAttempts` is spent on ONE batch. Bounded by the shards of
    /// the one stream this poller reads, and an entry is given up as soon as its shard moves
    /// on.
    attempts: BTreeMap<String, BatchAttempts>,
}

/// How many consecutive attempts the batch at the head of ONE shard has had, and which batch
/// those attempts were at.
///
/// `MaximumRetryAttempts` is spent on a BATCH — "Discard records after the specified number
/// of retries" — so the count belongs to the records the attempts were made with and starts
/// again once the shard has moved on to others.
#[derive(Debug)]
struct BatchAttempts {
    /// The sequence number of the FIRST record of the batch, which is the position the shard
    /// is still at: a batch read from a moved position is a different batch.
    head: String,
    /// How many attempts at that batch have been made, the first one included.
    made: u32,
}

/// How many attempts one batch of a mapping carrying `maximum_retry_attempts` gets: the
/// first attempt plus the retries that member allows.
///
/// `None` is "as many as it takes": "Discard records after the specified number of retries.
/// The default value is infinite (-1). When set to infinite (-1), failed records are retried
/// until the record expires" (`MaximumRetryAttempts`), which is what a mapping carrying no
/// value and one carrying the infinite default both ask for.
fn attempts_allowed(maximum_retry_attempts: Option<i32>) -> Option<u32> {
    // A negative value is the infinite default; every other value the member's own range
    // admits is a count of retries.
    let retries = u32::try_from(maximum_retry_attempts?).ok()?;
    Some(retries.saturating_add(1))
}

/// What the poller does next for the mapping it claims, releasing its claim when the
/// answer is [`Pass::Stop`] — the release and the state it was decided from are read in ONE
/// critical section, so a call that has already re-created a poller's work either keeps
/// this poller or starts a new one, never neither.
fn next_pass(
    store: &mut ComAmazonawsLambdaAwsgirapiserviceState,
    claim: &mut PollerClaim,
) -> Pass {
    let resolved = match store.event_source_mappings.get(&claim.uuid) {
        // "When you delete an event source mapping, it enters a Deleting state": nothing
        // more is read for it, and its removal is owed by the clock.
        None => None,
        Some(mapping) if mapping.is_deleting() => None,
        // Still moving into the state its settings ask for, or resting `Disabled`.
        Some(mapping)
            if mapping.is_settling()
                || mapping.state != super::EVENT_SOURCE_MAPPING_STATE_ENABLED =>
        {
            Some(Pass::Paused)
        }
        // "Lambda reads items from the event source and invokes the function": a mapping
        // whose function this store does not hold `Active` takes nothing off a shard, so
        // no batch is read for an invocation that could not be made.
        Some(mapping)
            if !store
                .functions
                .get(&mapping.function_name)
                .is_some_and(|function| function.state == super::STATE_ACTIVE) =>
        {
            Some(Pass::Paused)
        }
        Some(mapping) => Some(match run_of(store, mapping) {
            Ok(run) => Pass::Run(Box::new(run)),
            // A source this reader cannot read is reported by the pass itself, so the one
            // log line is written outside the lock.
            Err(detail) => Pass::Unserved(detail),
        }),
    };
    match resolved {
        Some(pass) => pass,
        None => {
            claim.release(store);
            Pass::Stop
        }
    }
}

/// The configuration one pass reads under, or why this mapping's source cannot be read
/// here.
fn run_of(
    store: &ComAmazonawsLambdaAwsgirapiserviceState,
    mapping: &super::EventSourceMappingRecord,
) -> Result<ShardSourcedMapping, String> {
    let Some(stream_arn) = mapping.event_source_arn.clone() else {
        return Err("the mapping names no event source ARN to read".to_string());
    };
    let Some(parsed) = ::service_common::arn::parse(&stream_arn) else {
        return Err("the mapping's event source is not a resource ARN".to_string());
    };
    let Some(source) = ShardSource::of_vendor(parsed.vendor) else {
        return Err(format!(
            "an event source of the {} namespace has no reader in this emulator",
            parsed.vendor
        ));
    };
    let region = parsed.region.to_string();
    // The role the invocation is attributed to, read from the VERSION this mapping
    // invokes: "The ARN of the Lambda function" the mapping carries may be qualified, and
    // an alias names the version it invokes.
    let invoke_identity_arn = store.functions.get(&mapping.function_name).and_then(|function| {
        let version = mapping
            .function_qualifier
            .as_deref()
            .and_then(|qualifier| function.resolved_version(qualifier))
            .unwrap_or_else(|| super::VERSION_LATEST.to_string());
        function.config_of(&version).map(|config| config.role.clone())
    });
    Ok(ShardSourcedMapping {
        source,
        stream_arn,
        region,
        function_arn: mapping.function_arn.clone(),
        function_name: mapping.function_name.clone(),
        invoke_identity_arn,
        batch_size: mapping.batch_size.clamp(1, MAX_RECORDS_PER_BATCH),
        reports_batch_item_failures: super::reports_batch_item_failures(
            &mapping.function_response_types,
        ),
        attempts_allowed: attempts_allowed(mapping.maximum_retry_attempts),
        cursors: mapping.shard_cursors.clone(),
        initial_cursor: mapping.initial_shard_cursor(),
    })
}

/// Read one batch from each shard of the mapped stream and hand each to the function.
///
/// Each shard is read on its own — "Process multiple batches from each shard"
/// (`ParallelizationFactor`) is per shard, and a shard's position is its own — so a batch
/// one shard could not deliver does not stop the others from being read on this pass.
///
/// Every batch read is metered before it is handed over, because the age it meters is a
/// property of the READ: it is how long the newest record of the batch had been in the
/// shard at the instant Lambda took it off, whatever the invocation then comes to.
///
/// `work` carries this poller's own working state: the clients this data plane reaches the
/// rest of the emulator through, and how many attempts the batch at the head of each shard
/// has already had — so a batch the function keeps failing is discarded once the mapping's
/// `MaximumRetryAttempts` is spent on it ([`failed_attempt`]).
async fn move_batches(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    work: &mut PollerWork,
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    metrics: &FunctionMetrics,
    policies: &PolicyStores,
    uuid: &str,
    run: &ShardSourcedMapping,
) -> Outcome {
    let shards = match shard_ids(callback, &mut work.clients, run).await {
        Ok(shards) => shards,
        Err(detail) => {
            return Outcome::Failed {
                detail,
                undelivered: false,
            }
        }
    };
    let mut delivered = false;
    let mut failure: Option<String> = None;
    // Whether what failed was the FUNCTION's processing of a batch this pass handed over,
    // rather than something the pass could not do at all (see [`Outcome::Failed`]).
    let mut undelivered = false;
    for shard in shards {
        let cursor = run
            .cursors
            .get(&shard)
            .cloned()
            .unwrap_or_else(|| run.initial_cursor.clone());
        let records = match read_batch(callback, &mut work.clients, run, &shard, &cursor).await {
            // "If the shard has been closed, the shard iterator can't return more data":
            // such a shard has no position left to read from.
            Ok(None) => continue,
            Ok(Some(records)) => records,
            Err(detail) => {
                failure = Some(detail);
                continue;
            }
        };
        let Some(last) = records.last() else {
            continue;
        };
        // The grant is asked about once more now that the batch is in hand, because the
        // policies in force on the role decide this pass at the instant it acts rather than
        // at the instant it started: a grant taken away while this pass was reading means
        // nothing is handed over, nothing is metered and the shard's position does not move,
        // so the records stay for the pass that follows the grant coming back.
        if let Some(detail) = withheld_shard_read(policies, run, SystemTime::now()) {
            failure = Some(detail);
            continue;
        }
        // Every record of the batch, by the identifier a report of this source names one by:
        // "the sequence number of the data record in the shard".
        let sequence_numbers: Vec<String> = records
            .iter()
            .map(|record| record.sequence_number.clone())
            .collect();
        metrics.publish_iterator_age(
            &run.function_name,
            iterator_age_millis(last.arrived_at.as_ref(), SystemTime::now()),
        );
        let payload = batch_event(&records);
        match hand_over_batch(callback, &mut work.clients, run, &payload, &sequence_numbers).await {
            Ok(processed) => {
                // The mapping reads on from past the records the attempt processed, and the
                // attempt is reported as what the mapping last came to. A shard has ONE
                // position, so what it reads on from is the run of records at the head of
                // the batch the attempt processed: the first record its function reported it
                // did not process is where the next read starts, whatever the report says
                // about the records behind that one.
                match checkpointed_through(&sequence_numbers, &processed) {
                    Some(sequence_number) => {
                        checkpoint(
                            state,
                            uuid,
                            run,
                            &shard,
                            Some(sequence_number),
                            super::LAST_PROCESSING_RESULT_OK,
                        );
                        // The shard has moved on, so the attempts the batch it was at had are
                        // spent on nothing further: the next batch starts its own count.
                        work.attempts.remove(&shard);
                        delivered = true;
                    }
                    None => {
                        // The report named the batch's own first record, so the cursor stays
                        // where it was and the whole batch is read again: an attempt that
                        // moved the shard's position not at all is one that did not process
                        // the batch, which is what it reports and what the poller backs off
                        // after.
                        failure = Some(failed_attempt(
                            state,
                            uuid,
                            run,
                            &shard,
                            &sequence_numbers,
                            &mut work.attempts,
                            format!(
                                "{} reported the first record of the batch it was handed as \
                                 one it did not process",
                                run.function_arn
                            ),
                        ));
                        undelivered = true;
                    }
                }
            }
            Err(detail) => {
                // Not delivered: the cursor stays where it was, so this batch is read
                // again — until the mapping's `MaximumRetryAttempts` is spent on it — and
                // the failed attempt is what the mapping last came to. Each shard is read on
                // its own, so the other shards of the stream are still read on this pass.
                failure = Some(failed_attempt(
                    state,
                    uuid,
                    run,
                    &shard,
                    &sequence_numbers,
                    &mut work.attempts,
                    detail,
                ));
                undelivered = true;
            }
        }
    }
    match failure {
        Some(detail) => Outcome::Failed {
            detail,
            undelivered,
        },
        None if delivered => Outcome::Delivered,
        None => Outcome::Idle,
    }
}

/// Record one FAILED attempt at the batch `sequence_numbers` names on `shard`, and answer
/// what the attempt came to for the one log line a failing mapping writes.
///
/// What the attempt came to is reported as the mapping's `LastProcessingResult` either way.
/// The cursor stays where it was — so the same batch is read again — until the mapping has
/// spent on this batch every attempt it allows: "Discard records after the specified number
/// of retries" (`MaximumRetryAttempts`), at which point the shard is read on from PAST the
/// batch, so the records behind the ones that were discarded are delivered. A mapping
/// carrying the infinite default discards nothing and keeps trying
/// ([`attempts_allowed`]).
fn failed_attempt(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: &str,
    run: &ShardSourcedMapping,
    shard: &str,
    sequence_numbers: &[String],
    attempts: &mut BTreeMap<String, BatchAttempts>,
    detail: String,
) -> String {
    let (Some(head), Some(tail)) = (sequence_numbers.first(), sequence_numbers.last()) else {
        return detail;
    };
    let made = match attempts.get_mut(shard) {
        // The shard is still at the position this batch was read from, so this is one more
        // attempt at the SAME batch.
        Some(held) if &held.head == head => {
            held.made = held.made.saturating_add(1);
            held.made
        }
        // A batch read from a position the shard has since moved to is a different batch,
        // which gets the mapping's whole allowance of its own.
        _ => {
            attempts.insert(
                shard.to_string(),
                BatchAttempts {
                    head: head.clone(),
                    made: 1,
                },
            );
            1
        }
    };
    let discarded = run.attempts_allowed.is_some_and(|allowed| made >= allowed);
    if discarded {
        attempts.remove(shard);
    }
    checkpoint(
        state,
        uuid,
        run,
        shard,
        discarded.then(|| tail.clone()),
        super::LAST_PROCESSING_RESULT_FUNCTION_CALL_FAILED,
    );
    if discarded {
        return format!(
            "{detail}, and the mapping allows {made} attempt(s) at a batch, so the {} \
             record(s) of this one were discarded and {shard} reads on past them",
            sequence_numbers.len()
        );
    }
    format!("{detail}, so {shard} reads the same batch again")
}

/// The event ONE batch is handed over as: "Lambda reads items from the event source and
/// invokes the function", and what it invokes the function with carries one envelope per
/// record read, under `Records`, in the order the shard holds them.
///
/// Each envelope is the one the record's own source built for it ([`ShardRecord::envelope`]),
/// so this composition is the one part of the hand-over that is the same for every source.
fn batch_event(records: &[ShardRecord]) -> Value {
    json!({
        "Records": records
            .iter()
            .map(|record| record.envelope.clone())
            .collect::<Vec<Value>>(),
    })
}

/// Every open and closed shard of the mapped stream, in the order the stream reports them
/// — through the listing the source's own data plane publishes.
async fn shard_ids(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
) -> Result<Vec<String>, String> {
    match run.source {
        ShardSource::Kinesis => kinesis_stream::shard_ids(callback, clients, run).await,
        ShardSource::DynamoDbStream => dynamodb_stream::shard_ids(callback, clients, run).await,
    }
}

/// The records one batch holds: what `shard` has at `cursor`, up to the mapping's batch
/// size, each already rendered as the envelope its own source hands a record over as.
///
/// `None` for a shard that has no position left to read from.
async fn read_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    shard: &str,
    cursor: &ShardCursor,
) -> Result<Option<Vec<ShardRecord>>, String> {
    match run.source {
        ShardSource::Kinesis => {
            kinesis_stream::read_batch(callback, clients, run, shard, cursor).await
        }
        ShardSource::DynamoDbStream => {
            dynamodb_stream::read_batch(callback, clients, run, shard, cursor).await
        }
    }
}

/// Record what one processing attempt came to on the mapping itself, and — for an attempt
/// that delivered its batch — move the shard's cursor past that batch.
///
/// The record is re-read under the lock and only written when it still names the stream and
/// the function this batch was read for, so a mapping re-targeted while the batch was in
/// flight is not checkpointed under a configuration it no longer carries.
fn checkpoint(
    state: &::engine::ServiceState<ComAmazonawsLambdaAwsgirapiserviceState>,
    uuid: &str,
    run: &ShardSourcedMapping,
    shard: &str,
    delivered_through: Option<String>,
    result: &str,
) {
    let mut store = state.lock();
    let Some(mapping) = store.event_source_mappings.get_mut(uuid) else {
        return;
    };
    if mapping.event_source_arn.as_deref() != Some(run.stream_arn.as_str())
        || mapping.function_arn != run.function_arn
    {
        return;
    }
    if let Some(sequence_number) = delivered_through {
        mapping
            .shard_cursors
            .insert(shard.to_string(), ShardCursor::AfterSequenceNumber(sequence_number));
    }
    mapping.last_processing_result = Some(result.to_string());
}

/// How long the newest record of a batch had been in the shard when the batch was read,
/// in milliseconds: never negative, and 0 for a record this emulator holds no arrival
/// instant for.
fn iterator_age_millis(
    arrived_at: Option<&::aws_smithy_types::DateTime>,
    read_at: SystemTime,
) -> f64 {
    let Some(arrived_at) = arrived_at else {
        return 0.0;
    };
    let read_at = ::aws_smithy_types::DateTime::from(read_at);
    ((read_at.as_secs_f64() - arrived_at.as_secs_f64()) * 1_000.0).max(0.0)
}

/// Hand one batch to the function and read what became of the records it held: the ones the
/// attempt PROCESSED, or why none of them was.
///
/// Every record of a batch an invocation accepted was processed unless the mapping asks for
/// its function's report of which of them failed to be read, which is what
/// `FunctionResponseTypes` says (see [`super::batch_item_failures`]).
async fn hand_over_batch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    payload: &Value,
    sequence_numbers: &[String],
) -> Result<Vec<String>, String> {
    let answered = invoke_function(callback, clients, run, payload).await?;
    if !run.reports_batch_item_failures {
        return Ok(sequence_numbers.to_vec());
    }
    match super::read_batch_item_failures(answered.as_deref(), sequence_numbers) {
        super::BatchReport::WholeBatchFailed(detail) => Err(format!(
            "{} reported no record of the batch as processed: {detail}",
            run.function_arn
        )),
        report => Ok(super::processed_records(&report, sequence_numbers).unwrap_or_default()),
    }
}

/// The sequence number a shard reads on from after an attempt that processed `processed` of
/// the batch `sequence_numbers`, or `None` when the attempt did not process the batch's own
/// first record and so moved the shard's position not at all.
///
/// A shard has ONE position, so the records it can be read on from past are the run at the
/// HEAD of the batch that were processed: the first record the attempt did not process is
/// the one the next read must start at.
fn checkpointed_through(sequence_numbers: &[String], processed: &[String]) -> Option<String> {
    let consumed: ::std::collections::BTreeSet<&str> =
        processed.iter().map(String::as_str).collect();
    sequence_numbers
        .iter()
        .take_while(|sequence_number| consumed.contains(sequence_number.as_str()))
        .last()
        .cloned()
}

/// Hand one batch to the function, synchronously, as a stream event source's invocation is,
/// and answer with the response document it returned.
async fn invoke_function(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    clients: &mut Clients,
    run: &ShardSourcedMapping,
    payload: &Value,
) -> Result<Option<Vec<u8>>, String> {
    let answer = clients
        .lambda(callback, &run.region)
        .invoke()
        .function_name(&run.function_arn)
        .payload(::aws_sdk_lambda::primitives::Blob::new(
            payload.to_string().into_bytes(),
        ))
        .send()
        .await
        .map_err(|error| {
            format!(
                "{} could not be invoked ({})",
                run.function_arn,
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    // "If present, indicates that an error occurred during function execution"
    // (`InvocationResponse$FunctionError`): the batch was not processed, so it is not
    // checkpointed past.
    if let Some(failure) = answer.function_error.as_deref() {
        return Err(format!(
            "{} answered a {failure} error for the batch",
            run.function_arn
        ));
    }
    // "The response from the function, or an error object" (`InvocationResponse$Payload`):
    // the document a mapping that reads a batch item failure report reads it out of.
    Ok(answer.payload.map(::aws_smithy_types::Blob::into_inner))
}

/// The clients this data plane reaches the other services of this emulator through, built
/// on first use and then reused.
///
/// Each slot remembers the Region its client was built for, because a resource is reached
/// in the Region ITS OWN ARN names.
///
/// Shared with [`super::sqs_poller`], the reader of the other kind of event source, so both
/// pollers reach this emulator the one way.
#[derive(Default)]
pub(crate) struct Clients {
    kinesis: Option<(String, ::aws_sdk_kinesis::Client)>,
    dynamodbstreams: Option<(String, ::aws_sdk_dynamodbstreams::Client)>,
    lambda: Option<(String, ::aws_sdk_lambda::Client)>,
}

/// Build each client the same way: over the in-process callback into this same emulator,
/// against the called service's endpoint in `region`.
///
/// The endpoint url names the called service's public host so the request is signed and
/// routed as that service's, but the TRANSPORT is the emulator's own in-process connector
/// ([`smithy_server_runtime::EmbeddedEmulator::http_client`]): no socket is opened and no
/// name is resolved, so a stream or function a mapping named is reached only if this
/// emulator serves it itself.
///
/// The transport is in-process, so each client disables what a network client needs and
/// this one cannot have: retries, timeouts, stalled-stream protection and identity caching
/// each need a sleep implementation the callback transport is not given.
macro_rules! callback_client {
    ($field:ident, $sdk:ident, $endpoint:literal) => {
        pub(crate) fn $field(
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
    callback_client!(kinesis, aws_sdk_kinesis, "kinesis");
    callback_client!(dynamodbstreams, aws_sdk_dynamodbstreams, "streams.dynamodb");
    callback_client!(lambda, aws_sdk_lambda, "lambda");
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{batch_event, checkpointed_through, iterator_age_millis, ShardRecord};

    #[test]
    fn a_shard_reads_on_from_the_processed_run_at_the_head_of_the_batch() {
        // A shard has ONE position, so what it reads on from is the run of records at the
        // head of the batch the attempt processed. The rows differ along where in the batch
        // the first unprocessed record is: nowhere (every record processed), the head, the
        // middle, the tail, and a batch none of which was processed.
        let batch: Vec<String> = ["49-1", "49-2", "49-3"]
            .iter()
            .map(|sequence_number| (*sequence_number).to_string())
            .collect();
        let rows: [(&str, Vec<&str>, Option<&str>); 5] = [
            (
                "every record processed",
                vec!["49-1", "49-2", "49-3"],
                Some("49-3"),
            ),
            ("the first record failed", vec!["49-2", "49-3"], None),
            ("the middle record failed", vec!["49-1", "49-3"], Some("49-1")),
            ("the last record failed", vec!["49-1", "49-2"], Some("49-2")),
            ("no record processed", Vec::new(), None),
        ];
        for (case, processed, expected) in rows {
            let processed: Vec<String> = processed
                .iter()
                .map(|sequence_number| (*sequence_number).to_string())
                .collect();
            let through = checkpointed_through(&batch, &processed);
            assert_eq!(
                through.as_deref(),
                expected,
                "case {case:?}: a shard whose attempt processed {processed:?} of {batch:?} \
                 must read on from {expected:?}, got {through:?}"
            );
        }
    }

    /// RULE: the age one batch meters is how long the NEWEST record of that batch had been
    /// in the shard when the batch was read — never negative, and 0 for a record with no
    /// arrival instant to measure from.
    ///
    /// The rows differ along what the rule generalises over: how long ago the record
    /// arrived (now, a second ago, a minute and a half ago — above one CloudWatch period),
    /// a record whose arrival is ahead of the read, and one carrying no arrival instant at
    /// all.
    #[test]
    fn the_age_a_batch_meters_is_the_wait_of_its_newest_record() {
        let read_at = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000);
        // (case, when the newest record arrived, the age in milliseconds)
        let rows: Vec<(&str, Option<SystemTime>, f64)> = vec![
            ("read-as-it-arrives", Some(read_at), 0.0),
            (
                "a-second-old",
                Some(read_at - Duration::from_secs(1)),
                1_000.0,
            ),
            (
                "held-past-one-period",
                Some(read_at - Duration::from_secs(90)),
                90_000.0,
            ),
            (
                "arrived-after-the-read",
                Some(read_at + Duration::from_secs(5)),
                0.0,
            ),
            ("no-arrival-instant", None, 0.0),
        ];
        for (case, arrived_at, expected) in rows {
            let arrived_at = arrived_at.map(::aws_smithy_types::DateTime::from);
            let actual = iterator_age_millis(arrived_at.as_ref(), read_at);
            assert!(
                (actual - expected).abs() < 1.0,
                "case {case:?}: expected an age of {expected} ms, got {actual:?} ms"
            );
        }
    }

    /// RULE: the event a batch is handed over as carries ONE envelope per record read,
    /// under `Records`, in the order the shard holds them — each the envelope the record's
    /// own source built for it.
    ///
    /// The rows differ along what the rule generalises over: how many records the batch
    /// holds (one, several, and — for completeness of the composition itself — none).
    #[test]
    fn a_batch_is_handed_over_as_one_envelope_per_record_in_shard_order() {
        // (case, the envelopes of the batch's records, in the order they were read)
        let rows: Vec<(&str, Vec<::serde_json::Value>)> = vec![
            ("no-record", Vec::new()),
            (
                "one-record",
                vec![::serde_json::json!({ "eventID": "only" })],
            ),
            (
                "three-records",
                vec![
                    ::serde_json::json!({ "eventID": "first" }),
                    ::serde_json::json!({ "eventID": "second" }),
                    ::serde_json::json!({ "eventID": "third" }),
                ],
            ),
        ];
        for (case, envelopes) in rows {
            let records: Vec<ShardRecord> = envelopes
                .iter()
                .enumerate()
                .map(|(index, envelope)| ShardRecord {
                    sequence_number: format!("1000000000000000000{index}"),
                    arrived_at: None,
                    envelope: envelope.clone(),
                })
                .collect();
            let event = batch_event(&records);
            assert_eq!(
                event["Records"],
                ::serde_json::json!(envelopes),
                "case {case:?}: expected one envelope per record in shard order, got {event}"
            );
        }
    }
}
