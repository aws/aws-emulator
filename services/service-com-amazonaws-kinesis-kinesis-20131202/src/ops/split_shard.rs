// SPDX-License-Identifier: CC0-1.0

//! @op SplitShard
//! Splits one open shard of a PROVISIONED stream into two, reading and writing the
//! `streams` store this service's [`crate::state`] owns.
//!
//! RULE: an accepted call closes the named shard and lays down TWO new open shards that
//! together cover exactly the hash key range it covered — the lower one every key below
//! `NewStartingHashKey`, the upper one that key and every key above it — each naming the
//! shard it was produced from, and leaves the stream `UPDATING` until the transition back
//! to `ACTIVE` falls due. The call is served only for a stream in the provisioned capacity
//! mode, only while that stream is `ACTIVE`, only for a shard the stream holds open, and
//! only for a hash key strictly inside that shard's range; every refusal leaves the shard
//! map exactly as it was.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `SplitShard`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Splits a shard into two new shards in the Kinesis data stream, to increase the stream's
//! capacity to ingest and transport data. SplitShard is called when there is a need to
//! increase the overall capacity of a stream because of an expected increase in the volume
//! of data records being ingested. This API is only supported for the data streams with the
//! provisioned capacity mode. When invoking this API, you must use either the StreamARN or
//! the StreamName parameter, or both. It is recommended that you use the StreamARN input
//! parameter when you invoke this API. You can also use SplitShard when a shard appears to
//! be approaching its maximum utilization; for example, the producers sending data into the
//! specific shard are suddenly sending more than previously anticipated. You can also call
//! SplitShard to increase stream capacity, so that more Kinesis Data Streams applications
//! can simultaneously read data from the stream for real-time processing. You must specify
//! the shard to be split and the new hash key, which is the position in the shard where the
//! shard gets split in two. In many cases, the new hash key might be the average of the
//! beginning and ending hash key, but it can be any hash key value in the range being
//! mapped into the shard. For more information, see Split a Shard in the Amazon Kinesis
//! Data Streams Developer Guide. You can use DescribeStreamSummary and the ListShards APIs
//! to determine the shard ID and hash key values for the ShardToSplit and
//! NewStartingHashKey parameters that are specified in the SplitShard request. SplitShard
//! is an asynchronous operation. Upon receiving a SplitShard request, Kinesis Data Streams
//! immediately returns a response and sets the stream status to UPDATING. After the
//! operation is completed, Kinesis Data Streams sets the stream status to ACTIVE. Read and
//! write operations continue to work while the stream is in the UPDATING state. You can use
//! DescribeStreamSummary to check the status of the stream, which is returned in
//! StreamStatus. If the stream is in the ACTIVE state, you can call SplitShard. If the
//! specified stream does not exist, DescribeStreamSummary returns a
//! ResourceNotFoundException. If you try to create more shards than are authorized for your
//! account, you receive a LimitExceededException. For the default shard limit for an Amazon
//! Web Services account, see Kinesis Data Streams Limits in the Amazon Kinesis Data Streams
//! Developer Guide. To increase this limit, contact Amazon Web Services Support. If you try
//! to operate on too many streams simultaneously using CreateStream, DeleteStream,
//! MergeShards, and/or SplitShard, you receive a LimitExceededException. SplitShard has a
//! limit of five transactions per second per account.
//!
//! ## Input
//!
//! * `NewStartingHashKey` (HashKey, pattern ^(0|([1-9]\d{0,38}))$) **required** — A hash key
//!   value for the starting hash key of one of the child shards created by the split. The
//!   hash key range for a given shard constitutes a set of ordered contiguous positive
//!   integers. The value for NewStartingHashKey must be in the range of hash keys being
//!   mapped into the shard. The NewStartingHashKey hash key value and all higher hash key
//!   values in hash key range are distributed to one of the child shards. All the lower hash
//!   key values in the range are distributed to the other child shard.
//! * `ShardToSplit` (ShardId, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) **required** — The shard
//!   ID of the shard to split.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream for the shard split.
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 400) — Specifies that you do not have the permissions
//!   required to perform this operation.
//! * `InvalidArgumentException` (HTTP 400) — A specified parameter exceeds its restrictions,
//!   is not supported, or can't be used. For more information, see the returned message.
//! * `LimitExceededException` (HTTP 400) — The requested resource exceeds the maximum number
//!   allowed, or the number of concurrent stream requests exceeds the maximum number allowed.
//! * `ResourceInUseException` (HTTP 400) — The resource is not available for this operation.
//!   For successful operation, the resource must be in the ACTIVE state.
//! * `ResourceNotFoundException` (HTTP 400) — The requested resource could not be found. The
//!   stream might not be specified correctly.
//! * `ValidationException` (HTTP 400) — Specifies that you tried to invoke this API for a
//!   data stream with the on-demand capacity mode. This API is only supported for data
//!   streams with the provisioned capacity mode.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `SplitShardError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct SplitShardOutput;

impl SplitShardOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static SPLIT_SHARD_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &SplitShardOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for SplitShardOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `SplitShardError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#SplitShard",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum SplitShardErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::split_shard::SplitShardError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type SplitShardError = crate::scaffolded::Scaffolded<SplitShardErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<SplitShardErrorModeled> for SplitShardError {
    fn from(modeled: SplitShardErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `SplitShard` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct SplitShard<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "SplitShard",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> SplitShardError {
    SplitShardErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// How many shards a split lays down: "Splits a shard into two new shards in the Kinesis
/// data stream" (`SplitShard`), so an accepted call adds two shard records to the store and
/// is counted against the account's shard quota by that number.
const SPLIT_CHILDREN: i32 = 2;

/// Refuse the request because serving it would take the stream past one of this operation's
/// documented limits, or the account past its shard quota: "the requested resource exceeds
/// the maximum number allowed" (`LimitExceededException`).
fn limit_exceeded(message: String) -> SplitShardError {
    SplitShardErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message),
    })
    .into()
}

/// The identifier the request addressed the stream by, which is what a refusal names.
/// `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
/// carries one and a request naming only that member names no stream.
fn stream_identifier(input: &SplitShardInput) -> &str {
    input
        .stream_name
        .as_deref()
        .or(input.stream_arn.as_deref())
        .unwrap_or_default()
}

/// The modelled refusal for a request whose `StreamName` / `StreamARN` pair names no stream
/// of this account and region, worded exactly as the reads word it: one rule, one phrasing,
/// for every operation that addresses a stream.
fn lookup_refusal(failure: crate::state::StreamLookupFailure, identifier: &str) -> SplitShardError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            SplitShardErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::stream_not_found_message(identifier)),
            })
            .into()
        }
    }
}

/// Split the shard the request names, or answer the modelled refusal that stands in the
/// way. The whole write lives here so the caller holds the store's lock for exactly this
/// long.
fn split_shard(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &SplitShardInput,
) -> Result<(), SplitShardError> {
    // The account and region's shard quota is counted across the whole store, so it is read
    // BEFORE the stream is borrowed for writing: the two children this call lays down are
    // two more shard records beside the ones the store already holds, since the shard it
    // closes keeps the records written to it.
    let (shard_limit, open_shard_count) = store.shard_quota_in(now, region);

    let identifier = stream_identifier(input).to_string();
    let stream = store
        .resolve_stream_mut(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;
    // A refusal about the stream this request RESOLVED names it by its `StreamName`, which
    // is the variable part the service's messages carry: whichever identifier addressed the
    // stream — its name, its ARN or both — the STORED name is what the message reports.
    let identifier = stream.name.clone();

    // "This API is only supported for the data streams with the provisioned capacity mode":
    // an on-demand stream's shard map is the service's to re-tile, so the request is refused
    // rather than served and the stored shard map is left exactly as it was.
    if stream.stream_mode != crate::state::STREAM_MODE_PROVISIONED {
        return Err(SplitShardErrorModeled::ValidationException(ValidationException {
            message: Some(crate::state::stream_on_demand_message(&stream.name)),
        })
        .into());
    }

    // "If the specified stream is in the CREATING, DELETING, or UPDATING state, SplitShard
    // returns a ResourceInUseException" — only an `ACTIVE` stream's shard map is re-tiled.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(SplitShardErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&stream.name, status)),
            },
        )
        .into());
    }

    // `ShardToSplit` has to name a shard the stream holds, and an open one: a closed shard
    // has already given its hash key range up to the children a split or a merge minted.
    let shard_to_split = input.shard_to_split.as_str();
    let Some(position) = stream.open_shard_index(shard_to_split) else {
        if stream.holds_shard(shard_to_split) {
            return Err(invalid_argument(crate::state::shard_not_open_message(
                shard_to_split,
                &identifier,
            )));
        }
        return Err(SplitShardErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message: Some(crate::state::shard_not_in_stream_message(
                    shard_to_split,
                    &identifier,
                )),
            },
        )
        .into());
    };

    // "The value for NewStartingHashKey must be in the range of hash keys being mapped into
    // the shard", and both children have to cover a key: the key names the lowest key of
    // the upper child, so it has to lie strictly inside the parent's range. `HashKey` is
    // modelled `^(0|([1-9]\d{0,38}))$`, which admits values past the 128-bit key space, and
    // one of those lies inside no shard's range — the same refusal as a key below it.
    let (starting, ending) = stream
        .shards
        .get(position)
        .and_then(crate::state::ShardRecord::hash_key_bounds)
        .ok_or_else(|| {
            SplitShardError::from(SplitShardErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some(
                        "the shard named by this request covers no hash key range".to_string(),
                    ),
                },
            ))
        })?;
    let new_starting_hash_key = input.new_starting_hash_key.parse::<u128>().ok();
    let inside = new_starting_hash_key.is_some_and(|key| {
        starting.checked_add(1).is_some_and(|lowest| key > lowest) && key < ending
    });
    if !inside {
        return Err(invalid_argument(
            crate::state::split_hash_key_outside_the_parent_message(
                &input.new_starting_hash_key,
                shard_to_split,
                &identifier,
                &starting.to_string(),
                &ending.to_string(),
            ),
        ));
    }

    // "If you try to create more shards than are authorized for your account, you receive a
    // LimitExceededException" (`SplitShard`): the two children this split lays down are
    // counted against the room left under the quota `DescribeLimits` publishes for this
    // account and region, and a split past it is refused rather than grown into.
    if open_shard_count.saturating_add(SPLIT_CHILDREN) > shard_limit {
        return Err(limit_exceeded(crate::state::shard_limit_exceeded_message(
            region,
            open_shard_count,
            shard_limit,
            SPLIT_CHILDREN,
        )));
    }

    // "SplitShard is an asynchronous operation. Upon receiving a SplitShard request, Kinesis
    // Data Streams immediately returns a response and sets the stream status to UPDATING" —
    // which the store records as an instant rather than sleeping on it.
    let split = new_starting_hash_key.and_then(|key| stream.split_shard(position, key, now));
    split.ok_or_else(|| {
        SplitShardError::from(SplitShardErrorModeled::InternalFailureException(
            InternalFailureException {
                message: Some(
                    "the shard named by this request covers no hash key range".to_string(),
                ),
            },
        ))
    })?;
    Ok(())
}

impl<Eng> SplitShard<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for SplitShard<Eng>
{
    type Input = SplitShardInput;
    type Output = SplitShardOutput;
    type Error = SplitShardError;

    const NAME: &'static str = "SplitShard";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::SPLIT_SHARD_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = SPLIT_SHARD_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The credential scope's access key id is read only to ATTRIBUTE the change
        // ([`audit`]); the region it names is the region whose streams this request
        // addresses, since a stream name "is also scoped by Amazon Web Services Region".
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            audit(&akid, &region, "unavailable");
            SplitShardError::from(SplitShardErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the stream store is unavailable".to_string()),
                },
            ))
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            split_shard(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "split" } else { "refused" },
        );
        outcome?;

        // `SplitShard` models no output members: the call "is an asynchronous operation"
        // whose answer is the stream's shard map, which `ListShards` reports.
        Ok(SplitShardOutput)
    }
}
