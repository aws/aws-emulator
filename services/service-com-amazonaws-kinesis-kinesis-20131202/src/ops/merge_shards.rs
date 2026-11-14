// SPDX-License-Identifier: CC0-1.0

//! @op MergeShards
//! Merges two adjacent open shards of a PROVISIONED stream into one, reading and writing
//! the `streams` store this service's [`crate::state`] owns.
//!
//! RULE: an accepted call closes both named shards and lays down ONE new open shard
//! covering the hash key range they jointly covered, naming both of them as the shards it
//! was produced from, and leaves the stream `UPDATING` until the transition back to
//! `ACTIVE` falls due. The call is served only for a stream in the provisioned capacity
//! mode, only while that stream is `ACTIVE`, and only for two shards the stream holds open
//! whose ranges meet; every refusal leaves the shard map exactly as it was.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `MergeShards`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Merges two adjacent shards in a Kinesis data stream and combines them into a single
//! shard to reduce the stream's capacity to ingest and transport data. This API is only
//! supported for the data streams with the provisioned capacity mode. Two shards are
//! considered adjacent if the union of the hash key ranges for the two shards form a
//! contiguous set with no gaps. For example, if you have two shards, one with a hash key
//! range of 276...381 and the other with a hash key range of 382...454, then you could
//! merge these two shards into a single shard that would have a hash key range of
//! 276...454. After the merge, the single child shard receives data for all hash key values
//! covered by the two parent shards. When invoking this API, you must use either the
//! StreamARN or the StreamName parameter, or both. It is recommended that you use the
//! StreamARN input parameter when you invoke this API. MergeShards is called when there is
//! a need to reduce the overall capacity of a stream because of excess capacity that is not
//! being used. You must specify the shard to be merged and the adjacent shard for a stream.
//! For more information about merging shards, see Merge Two Shards in the Amazon Kinesis
//! Data Streams Developer Guide. If the stream is in the ACTIVE state, you can call
//! MergeShards. If a stream is in the CREATING, UPDATING, or DELETING state, MergeShards
//! returns a ResourceInUseException. If the specified stream does not exist, MergeShards
//! returns a ResourceNotFoundException. You can use DescribeStreamSummary to check the
//! state of the stream, which is returned in StreamStatus. MergeShards is an asynchronous
//! operation. Upon receiving a MergeShards request, Amazon Kinesis Data Streams immediately
//! returns a response and sets the StreamStatus to UPDATING. After the operation is
//! completed, Kinesis Data Streams sets the StreamStatus to ACTIVE. Read and write
//! operations continue to work while the stream is in the UPDATING state. You use
//! DescribeStreamSummary and the ListShards APIs to determine the shard IDs that are
//! specified in the MergeShards request. If you try to operate on too many streams in
//! parallel using CreateStream, DeleteStream, MergeShards, or SplitShard, you receive a
//! LimitExceededException. MergeShards has a limit of five transactions per second per
//! account.
//!
//! ## Input
//!
//! * `AdjacentShardToMerge` (ShardId, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) **required** —
//!   The shard ID of the adjacent shard for the merge.
//! * `ShardToMerge` (ShardId, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) **required** — The shard
//!   ID of the shard to combine with the adjacent shard for the merge.
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream for the merge.
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
// `MergeShardsError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct MergeShardsOutput;

impl MergeShardsOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static MERGE_SHARDS_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &MergeShardsOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for MergeShardsOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `MergeShardsError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#MergeShards",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum MergeShardsErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::merge_shards::MergeShardsError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type MergeShardsError = crate::scaffolded::Scaffolded<MergeShardsErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<MergeShardsErrorModeled> for MergeShardsError {
    fn from(modeled: MergeShardsErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `MergeShards` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct MergeShards<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "MergeShards",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> MergeShardsError {
    MergeShardsErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// How many shards a merge lays down: "Merges two adjacent shards in a Kinesis data stream
/// and combines them into a single shard" (`MergeShards`), so an accepted call adds one
/// shard record to the store and is counted against the account's shard quota by that
/// number.
const MERGE_CHILDREN: i32 = 1;

/// Refuse the request because serving it would take the account past its shard quota: "the
/// requested resource exceeds the maximum number allowed" (`LimitExceededException`).
fn limit_exceeded(message: String) -> MergeShardsError {
    MergeShardsErrorModeled::LimitExceededException(LimitExceededException {
        message: Some(message),
    })
    .into()
}

/// The identifier the request addressed the stream by, which is what a refusal names.
/// `StreamId` is documented "Not Implemented. Reserved for future use", so no stream
/// carries one and a request naming only that member names no stream.
fn stream_identifier(input: &MergeShardsInput) -> &str {
    input
        .stream_name
        .as_deref()
        .or(input.stream_arn.as_deref())
        .unwrap_or_default()
}

/// The modelled refusal for a request whose `StreamName` / `StreamARN` pair names no stream
/// of this account and region, worded exactly as the reads word it: one rule, one phrasing,
/// for every operation that addresses a stream.
fn lookup_refusal(failure: crate::state::StreamLookupFailure, identifier: &str) -> MergeShardsError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            MergeShardsErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::stream_not_found_message(identifier)),
            })
            .into()
        }
    }
}

/// Merge the two shards the request names, or answer the modelled refusal that stands in
/// the way. The whole write lives here so the caller holds the store's lock for exactly
/// this long.
fn merge_shards(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &MergeShardsInput,
) -> Result<(), MergeShardsError> {
    // The account and region's shard quota is counted across the whole store, so it is read
    // BEFORE the stream is borrowed for writing: the child this call lays down is one more
    // shard record beside the ones the store already holds, since the shards it closes keep
    // the records written to them.
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
        return Err(MergeShardsErrorModeled::ValidationException(ValidationException {
            message: Some(crate::state::stream_on_demand_message(&stream.name)),
        })
        .into());
    }

    // "If the stream is in the ACTIVE state, you can call MergeShards. If a stream is in
    // the CREATING, UPDATING, or DELETING state, MergeShards returns a
    // ResourceInUseException."
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(MergeShardsErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&stream.name, status)),
            },
        )
        .into());
    }

    // "You must specify the shard to be merged and the adjacent shard for a stream": each
    // has to be a shard the stream holds, and an open one — a closed shard has already
    // given its hash key range up to the children a split or a merge minted.
    let mut resolved = [0usize; 2];
    for (slot, shard_id) in resolved.iter_mut().zip([
        input.shard_to_merge.as_str(),
        input.adjacent_shard_to_merge.as_str(),
    ]) {
        *slot = match stream.open_shard_index(shard_id) {
            Some(position) => position,
            None if stream.holds_shard(shard_id) => {
                return Err(invalid_argument(crate::state::shard_not_open_message(
                    shard_id,
                    &identifier,
                )));
            }
            None => {
                return Err(MergeShardsErrorModeled::ResourceNotFoundException(
                    ResourceNotFoundException {
                        message: Some(crate::state::shard_not_in_stream_message(
                            shard_id,
                            &identifier,
                        )),
                    },
                )
                .into());
            }
        };
    }
    let (first, second) = (resolved[0], resolved[1]);

    // "Two shards are considered adjacent if the union of the hash key ranges for the two
    // shards form a contiguous set with no gaps" — which also refuses a request that names
    // one shard twice, since no range meets itself.
    let bounds = |position: usize| {
        stream
            .shards
            .get(position)
            .and_then(crate::state::ShardRecord::hash_key_bounds)
    };
    let adjacent = match (bounds(first), bounds(second)) {
        (Some((first_starting, first_ending)), Some((second_starting, second_ending))) => {
            first_ending.checked_add(1) == Some(second_starting)
                || second_ending.checked_add(1) == Some(first_starting)
        }
        _ => false,
    };
    if !adjacent {
        return Err(invalid_argument(crate::state::shards_not_adjacent_message(
            &input.shard_to_merge,
            &input.adjacent_shard_to_merge,
            &identifier,
        )));
    }

    // A merge lays down a child shard beside the two it closes, and a shard record it adds
    // is capacity the account takes up, so it is counted against the room left under the
    // quota `DescribeLimits` publishes for this account and region exactly as every other
    // call that lays down a shard is: one quota, one refusal.
    if open_shard_count.saturating_add(MERGE_CHILDREN) > shard_limit {
        return Err(limit_exceeded(crate::state::shard_limit_exceeded_message(
            region,
            open_shard_count,
            shard_limit,
            MERGE_CHILDREN,
        )));
    }

    // "MergeShards is an asynchronous operation. Upon receiving a MergeShards request,
    // Amazon Kinesis Data Streams immediately returns a response and sets the StreamStatus
    // to UPDATING" — which the store records as an instant rather than sleeping on it.
    stream.merge_shards(first, second, now).ok_or_else(|| {
        MergeShardsError::from(MergeShardsErrorModeled::InternalFailureException(
            InternalFailureException {
                message: Some(
                    "the shards named by this request no longer cover a hash key range"
                        .to_string(),
                ),
            },
        ))
    })?;
    Ok(())
}

impl<Eng> MergeShards<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for MergeShards<Eng>
{
    type Input = MergeShardsInput;
    type Output = MergeShardsOutput;
    type Error = MergeShardsError;

    const NAME: &'static str = "MergeShards";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::MERGE_SHARDS_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = MERGE_SHARDS_OUTPUT_SCHEMA;
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
            MergeShardsError::from(MergeShardsErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the stream store is unavailable".to_string()),
                },
            ))
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            merge_shards(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "merged" } else { "refused" },
        );
        outcome?;

        // `MergeShards` models no output members: the call "is an asynchronous operation"
        // whose answer is the stream's shard map, which `ListShards` reports.
        Ok(MergeShardsOutput)
    }
}
