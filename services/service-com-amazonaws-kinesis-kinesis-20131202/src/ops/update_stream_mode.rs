// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStreamMode
//! Changes the capacity mode of the stream the request names, reading and writing the
//! `streams` store this service's [`crate::state`] owns.
//!
//! RULE: an accepted call stores the requested `StreamModeDetails` on the stream and leaves
//! it `UPDATING` until the transition back to `ACTIVE` falls due, so the mode every later
//! read reports is the one the call named. The call is served only while the stream is
//! `ACTIVE`, so a second change that overlaps one still in flight is refused with the
//! modelled `ResourceInUseException` and leaves the stored mode exactly as it was: of two
//! overlapping calls on one stream, exactly one is accepted.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStreamMode`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Updates the capacity mode of the data stream. Currently, in Kinesis Data Streams, you
//! can choose between an on-demand capacity mode and a provisioned capacity mode for your
//! data stream. If you'd still like to proactively scale your on-demand data stream’s
//! capacity, you can unlock the warm throughput feature for on-demand data streams by
//! enabling MinimumThroughputBillingCommitment for your account. Once your account has
//! MinimumThroughputBillingCommitment enabled, you can specify the warm throughput in MiB
//! per second that your stream can support in writes.
//!
//! ## Input
//!
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$)
//!   **required** — Specifies the ARN of the data stream whose capacity mode you want to
//!   update.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamModeDetails` (StreamModeDetails) **required** — Specifies the capacity mode to
//!   which you want to set your data stream. Currently, in Kinesis Data Streams, you can
//!   choose between an on-demand capacity mode and a provisioned capacity mode for your data
//!   streams.
//! * `WarmThroughputMiBps` (NaturalIntegerObject, range 0..) — The target warm throughput in
//!   MB/s that the stream should be scaled to handle. This represents the throughput capacity
//!   that will be immediately available for write operations. This field is only valid when
//!   the stream mode is being updated to on-demand.
//!
//! ## Errors
//!
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
// `UpdateStreamModeError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

#[derive(Debug, Clone, Default)]
pub struct UpdateStreamModeOutput;

impl UpdateStreamModeOutput {
    // `with_http` carries this operation's modelled response status. Remove it and
    // the status falls back to the protocol default, so a modelled 204 answers 200.
    pub const SCHEMA: ::aws_smithy_schema::Schema = ::aws_smithy_schema::Schema::new_struct(
        ::aws_smithy_schema::ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
        ::aws_smithy_schema::ShapeType::Structure,
        &[],
    );
}

pub static UPDATE_STREAM_MODE_OUTPUT_SCHEMA: &::aws_smithy_schema::Schema = &UpdateStreamModeOutput::SCHEMA;

impl ::aws_smithy_schema::serde::SerializableStruct for UpdateStreamModeOutput {
    fn serialize_members(&self, _s: &mut dyn ::aws_smithy_schema::serde::ShapeSerializer) -> Result<(), ::aws_smithy_schema::serde::SerdeError> { Ok(()) }
}

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStreamModeError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#UpdateStreamMode",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer
    // for a service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum UpdateStreamModeErrorModeled {
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stream_mode::UpdateStreamModeError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStreamModeError = crate::scaffolded::Scaffolded<UpdateStreamModeErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStreamModeErrorModeled> for UpdateStreamModeError {
    fn from(modeled: UpdateStreamModeErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStreamMode` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStreamMode<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "UpdateStreamMode",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> UpdateStreamModeError {
    UpdateStreamModeErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The modelled refusal for a request whose `StreamARN` names no stream of this account and
/// region, worded exactly as the reads word it: one rule, one phrasing, for every operation
/// that addresses a stream.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> UpdateStreamModeError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified
        | crate::state::StreamLookupFailure::Mismatch { .. } => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::NotFound => {
            UpdateStreamModeErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
                message: Some(crate::state::stream_not_found_message(identifier)),
            })
            .into()
        }
    }
}

/// Change the capacity mode of the stream the request names, or answer the modelled refusal
/// that stands in the way. The whole write lives here so the caller holds the store's lock
/// for exactly this long — which is also what admits exactly ONE of two overlapping calls:
/// the one that gets the lock first leaves the stream `UPDATING`, and the next one reads
/// that status.
fn update_stream_mode(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &UpdateStreamModeInput,
) -> Result<(), UpdateStreamModeError> {
    // `StreamMode` is a modelled enum, so a value outside `STREAM_MODE_VALUES` is a client
    // error rather than a stored mode. The model's own constraint walk refuses one before
    // this handler runs; this line states the same refusal for a caller that reaches the
    // handler another way.
    let stream_mode = input.stream_mode_details.stream_mode.as_str();
    if !STREAM_MODE_VALUES.contains(&stream_mode) {
        return Err(invalid_argument(format!(
            "StreamModeDetails.StreamMode {stream_mode} is not supported; the supported \
             values are {}",
            STREAM_MODE_VALUES.join(" | ")
        )));
    }

    // "This field is only valid when the stream mode is being updated to on-demand"
    // (`UpdateStreamModeInput$WarmThroughputMiBps`).
    if input.warm_throughput_mi_bps.is_some()
        && stream_mode != crate::state::STREAM_MODE_ON_DEMAND
    {
        return Err(invalid_argument(format!(
            "WarmThroughputMiBps is only valid when the stream mode is being updated to {}",
            crate::state::STREAM_MODE_ON_DEMAND
        )));
    }

    // How many streams of the on-demand capacity mode this region already holds, read
    // BEFORE the stored stream is borrowed to be written: the quota the change below is
    // counted against. The queues held on the stream are read here for the same reason — no
    // borrow of one stream reaches the records beside it.
    let on_demand_in_region = store.on_demand_streams_held_in(now, region);
    let queues_on_stream = store.queues_on(now, &input.stream_arn);

    // `StreamARN` is the only identifier this operation takes (`@required`), and `StreamId`
    // is documented "Not Implemented. Reserved for future use".
    let identifier = input.stream_arn.clone();
    let stream = store
        .resolve_stream_mut(now, region, None, Some(&identifier))
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose
    // create, whose removal or whose PREVIOUS update is still due has a capacity mode
    // nothing else may change, so of two overlapping calls the second is refused and the
    // stored mode is left exactly as the first one set it.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(UpdateStreamModeErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(
                    &stream.name,
                    status,
                )),
            },
        )
        .into());
    }

    // A stream of the on-demand capacity mode is counted against the on-demand stream quota
    // of this account and region, which is the quota `DescribeLimits` reports as
    // `OnDemandStreamCountLimit`: a change that would take the region past it is refused,
    // while a stream that already carries that mode is counted already and so takes it past
    // nothing.
    if stream_mode == crate::state::STREAM_MODE_ON_DEMAND
        && stream.stream_mode != crate::state::STREAM_MODE_ON_DEMAND
        && on_demand_in_region >= crate::state::MAX_ON_DEMAND_STREAMS_PER_REGION
    {
        return Err(UpdateStreamModeErrorModeled::LimitExceededException(
            LimitExceededException {
                message: Some(crate::state::on_demand_stream_ceiling_exceeded_message(
                    on_demand_in_region,
                )),
            },
        )
        .into());
    }

    // "You must delete every queue on a data stream before you can... switch it from
    // on-demand to provisioned capacity mode" (`DeleteQueue`), because queues "are supported
    // only on data streams in on-demand capacity mode" (`CreateQueue`): a queue on the stream
    // DEPENDS on the mode it was created under, so the stream is not available for that change
    // while the queue is there — the same `ResourceInUseException` rule, decided BEFORE
    // anything is mutated, so the stored mode survives the refusal and the identical call is
    // served once the queues are deleted.
    if stream_mode == crate::state::STREAM_MODE_PROVISIONED
        && stream.stream_mode != crate::state::STREAM_MODE_PROVISIONED
        && queues_on_stream > 0
    {
        return Err(UpdateStreamModeErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_has_queues_for_mode_change_message(
                    &stream.name,
                )),
            },
        )
        .into());
    }

    // The capacity mode is what changes: an on-demand stream's shard count is the service's
    // to manage from here on, so the shard map the stream already holds is carried over
    // rather than re-tiled, and the records in it stay readable. UPDATING is not terminal:
    // the stream is owed the `ACTIVE` it goes back to, and the instant that is due is stored
    // rather than slept on.
    stream.stream_mode = stream_mode.to_string();
    if let Some(warm_throughput_mi_bps) = input.warm_throughput_mi_bps {
        stream.warm_throughput_mi_bps = Some(warm_throughput_mi_bps);
    }
    // "The record distribution strategy is only supported for streams that use the on-demand
    // capacity mode" (`CreateStreamInput$RecordDistributionStrategy`), so a stream taken to
    // the provisioned mode carries none and its records are placed by their partition key;
    // and a warm-throughput target is on-demand-only too, so a provisioned stream reports
    // the throughput its own open shards provide instead.
    if stream_mode == crate::state::STREAM_MODE_PROVISIONED {
        stream.record_distribution_strategy = None;
        stream.warm_throughput_mi_bps = None;
    }
    stream.updating_until = Some(now + crate::state::MODE_UPDATE_DELAY);
    Ok(())
}

impl<Eng> UpdateStreamMode<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStreamMode<Eng>
{
    type Input = UpdateStreamModeInput;
    type Output = UpdateStreamModeOutput;
    type Error = UpdateStreamModeError;

    const NAME: &'static str = "UpdateStreamMode";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STREAM_MODE_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = UPDATE_STREAM_MODE_OUTPUT_SCHEMA;
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
            UpdateStreamModeError::from(UpdateStreamModeErrorModeled::InternalFailureException(
                InternalFailureException {
                    message: Some("the stream store is unavailable".to_string()),
                },
            ))
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            update_stream_mode(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "updated" } else { "refused" },
        );
        outcome?;

        // `UpdateStreamMode` models no output members: the mode it stored is what
        // `DescribeStreamSummary` reports for the stream.
        Ok(UpdateStreamModeOutput)
    }
}
