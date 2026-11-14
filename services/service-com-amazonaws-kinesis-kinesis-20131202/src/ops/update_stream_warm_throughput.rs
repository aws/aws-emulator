// SPDX-License-Identifier: CC0-1.0

//! @op UpdateStreamWarmThroughput
//!
//! Scales the WARM THROUGHPUT of the stream the request addresses, on the stored stream
//! record, so every later read of that stream reports the target the call named.
//!
//! RULE: an accepted call stores the requested `WarmThroughputMiBps` on the stream and leaves
//! it `UPDATING` until the transition back to `ACTIVE` falls due, so the warm throughput every
//! later read reports is the one the call named — upwards to scale ahead of traffic, and
//! downwards to release capacity again. The call is served only for a stream whose capacity
//! mode is the one warm throughput is configurable on, only while that stream is `ACTIVE`, and
//! only for a target inside the published ceiling; every other request is refused with a
//! modelled error and leaves the stored target exactly as it was.
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request. `ctx`
//! ([`smithy_server_runtime::OperationContext`]) carries request metadata only.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `UpdateStreamWarmThroughput`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json")
//! ```
//!
//! Updates the warm throughput configuration for the specified Amazon Kinesis Data Streams
//! on-demand data stream. Updates the warm throughput configuration for the specified
//! on-demand data stream. Use this operation to scale your stream to a specified throughput
//! level before anticipated traffic spikes, or to release excess capacity after traffic has
//! decreased. When invoking this API, you must use either the StreamARN or the StreamName
//! parameter, or both. It is recommended that you use the StreamARN input parameter when
//! you invoke this API. Updating the warm throughput is an asynchronous operation. Upon
//! receiving the request, Kinesis Data Streams returns immediately and sets the status of
//! the stream to UPDATING. After the update is complete, Kinesis Data Streams sets the
//! status of the stream back to ACTIVE. Depending on the size of the stream, the scaling
//! action could take a few minutes to complete. You can continue to read and write data to
//! your stream while its status is UPDATING. This operation is only supported for data
//! streams with the on-demand capacity mode in accounts that have
//! MinimumThroughputBillingCommitment enabled. Provisioned capacity mode streams do not
//! support warm throughput configuration. To release excess capacity, call the API again
//! and set the warm throughput to the same or a lower value. This operation has the
//! following default limits. By default, you cannot do the following: Scale to more than 10
//! GiBps for an on-demand stream. This API has a call limit of 5 transactions per second
//! (TPS) for each Amazon Web Services account. TPS over 5 will initiate the
//! LimitExceededException. For the default limits for an Amazon Web Services account, see
//! Streams Limits in the Amazon Kinesis Data Streams Developer Guide. To request an
//! increase in the call rate limit, the shard limit for this API, or your overall shard
//! limit, use the limits form.
//!
//! ## Input
//!
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream to be updated.
//! * `StreamId` (StreamId, len 1..24, pattern ^[a-z0-9]{20}-[a-z0-9]{3}$) — Not Implemented.
//!   Reserved for future use.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream to be updated.
//! * `WarmThroughputMiBps` (NaturalIntegerObject, range 0..) **required** — The target warm
//!   throughput in MB/s that the stream should be scaled to handle. This represents the
//!   throughput capacity that will be immediately available for write operations.
//!
//! ## Output
//!
//! * `StreamARN` (StreamARN, len 1..2048, pattern ^arn:aws.*:kinesis:.*:\d{12}:stream/\S+$) —
//!   The ARN of the stream that was updated.
//! * `StreamName` (StreamName, len 1..128, pattern ^[a-zA-Z0-9_.-]+$) — The name of the
//!   stream that was updated.
//! * `WarmThroughput` (WarmThroughputObject) — Specifies the updated warm throughput
//!   configuration for your data stream.
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
// `UpdateStreamWarmThroughputError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `UpdateStreamWarmThroughputError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#UpdateStreamWarmThroughput",
    // `InternalFailureException` is widened in with `extra_errors`, exactly as `CreateStream`
    // and `UpdateStreamMode` widen it: this SERVICE models it ("The processing of the request
    // failed because of an unknown error, exception, or failure"), the operation shape does
    // not list it, and it is the only honest answer for a service mounted on an engine that
    // holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum UpdateStreamWarmThroughputErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    ValidationException(ValidationException),
    InternalFailureException(InternalFailureException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::update_stream_warm_throughput::UpdateStreamWarmThroughputError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type UpdateStreamWarmThroughputError = crate::scaffolded::Scaffolded<UpdateStreamWarmThroughputErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<UpdateStreamWarmThroughputErrorModeled> for UpdateStreamWarmThroughputError {
    fn from(modeled: UpdateStreamWarmThroughputErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `UpdateStreamWarmThroughput` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct UpdateStreamWarmThroughput<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

/// Record this state-changing call for attribution: the access key id the caller presented
/// (a public identifier, never a secret), the region the stream lives in, and how the call
/// ended. No stream name and no ARN: a log sink needs neither to attribute the change.
fn audit(akid: &str, region: &str, outcome: &str) {
    ::tracing::info!(
        target: "emulator::audit",
        operation = "UpdateStreamWarmThroughput",
        akid = %akid,
        region = %region,
        outcome = %outcome,
        "state-changing operation"
    );
}

/// Refuse the request with the error this operation models for a parameter that "exceeds
/// its restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> UpdateStreamWarmThroughputError {
    UpdateStreamWarmThroughputErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
    .into()
}

/// The modelled refusal for a request whose `StreamName` / `StreamARN` pair names no stream of
/// this account and region, worded exactly as the reads word it: one rule, one phrasing, for
/// every operation that addresses a stream.
fn lookup_refusal(
    failure: crate::state::StreamLookupFailure,
    identifier: &str,
) -> UpdateStreamWarmThroughputError {
    match failure {
        crate::state::StreamLookupFailure::Unspecified => {
            invalid_argument(crate::state::STREAM_IDENTIFIER_REQUIRED_MESSAGE.to_string())
        }
        crate::state::StreamLookupFailure::Mismatch { name, arn } => {
            invalid_argument(crate::state::stream_identifier_mismatch_message(&name, &arn))
        }
        crate::state::StreamLookupFailure::NotFound => {
            UpdateStreamWarmThroughputErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message: Some(crate::state::stream_not_found_message(identifier)),
                },
            )
            .into()
        }
    }
}

/// Scale the warm throughput of the stream the request names, or answer the modelled refusal
/// that stands in the way. The whole write lives here so the caller holds the store's lock for
/// exactly this long — which is also what admits exactly ONE of two overlapping calls: the one
/// that gets the lock first leaves the stream `UPDATING`, and the next one reads that status.
fn apply_update(
    store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
    region: &str,
    now: ::std::time::SystemTime,
    input: &UpdateStreamWarmThroughputInput,
) -> Result<UpdateStreamWarmThroughputOutput, UpdateStreamWarmThroughputError> {
    // "This operation is only supported for data streams with the on-demand capacity mode in
    // accounts that have MinimumThroughputBillingCommitment enabled": the commitment is the
    // ACCOUNT's, so the precondition is judged BEFORE the request's stream is resolved — an
    // account that holds none is served no warm-throughput change at all, whether or not the
    // stream it named exists, and a client cannot act on a refusal about one stream when the
    // thing in the way is its account's billing commitment.
    if !store.commitment_is_enabled_at(now) {
        return Err(UpdateStreamWarmThroughputErrorModeled::ValidationException(
            ValidationException {
                message: Some(crate::state::account_not_enabled_for_commitment_message()),
            },
        )
        .into());
    }

    // "When invoking this API, you must use either the StreamARN or the StreamName parameter,
    // or both", and `StreamId` is documented "Not Implemented. Reserved for future use", so a
    // request naming only that member names no stream.
    let identifier = input
        .stream_name
        .as_deref()
        .or(input.stream_arn.as_deref())
        .unwrap_or_default()
        .to_string();
    let stream = store
        .resolve_stream_mut(
            now,
            region,
            input.stream_name.as_deref(),
            input.stream_arn.as_deref(),
        )
        .map_err(|failure| lookup_refusal(failure, &identifier))?;

    // "This operation is only supported for data streams with the on-demand capacity mode...
    // Provisioned capacity mode streams do not support warm throughput configuration": a
    // provisioned stream's write capacity is the shard map its owner sized, so the request is
    // refused rather than served and the stored target is left exactly as it was.
    if stream.stream_mode != crate::state::STREAM_MODE_ON_DEMAND {
        return Err(UpdateStreamWarmThroughputErrorModeled::ValidationException(
            ValidationException {
                message: Some(crate::state::stream_provisioned_message(&stream.name)),
            },
        )
        .into());
    }

    // "By default, you cannot do the following: Scale to more than 10 GiBps for an on-demand
    // stream... To request an increase in the call rate limit, the shard limit for this API,
    // or your overall shard limit, use the limits form" — a target past that published
    // ceiling asks for throughput beyond what the account is allowed, which is the modelled
    // "requested resource exceeds the maximum number allowed" refusal, so it is refused and
    // nothing is stored. The modelled `range 0..` of `WarmThroughputMiBps` is refused BEFORE
    // this handler runs, so the floor needs no second statement here. A target no stream may
    // carry is refused whatever state the stream is in, so this stands ahead of the
    // lifecycle-status refusal below.
    if input.warm_throughput_mi_bps > crate::state::MAX_WARM_THROUGHPUT_MI_BPS {
        return Err(
            UpdateStreamWarmThroughputErrorModeled::LimitExceededException(
                LimitExceededException {
                    message: Some(crate::state::warm_throughput_over_ceiling_message(
                        input.warm_throughput_mi_bps,
                    )),
                },
            )
            .into(),
        );
    }

    // "The resource is not available for this operation. For successful operation, the
    // resource must be in the ACTIVE state" (`ResourceInUseException`): a stream whose create,
    // whose removal or whose PREVIOUS update is still due has a configuration nothing else may
    // change, so of two overlapping calls the second is refused.
    let status = stream.status(now);
    if status != crate::state::STREAM_ACTIVE {
        return Err(UpdateStreamWarmThroughputErrorModeled::ResourceInUseException(
            ResourceInUseException {
                message: Some(crate::state::stream_not_active_message(&stream.name, status)),
            },
        )
        .into());
    }

    // The target the request names becomes the stream's own warm throughput, replacing
    // whatever it carried — the value a create or an earlier update named, or the throughput
    // its open shards provide — so a later call naming the same or a lower value releases the
    // excess capacity again. "Updating the warm throughput is an asynchronous operation. Upon
    // receiving the request, Kinesis Data Streams returns immediately and sets the status of
    // the stream to UPDATING. After the update is complete, Kinesis Data Streams sets the
    // status of the stream back to ACTIVE": UPDATING is not terminal, and the instant the
    // stream is owed its `ACTIVE` back at is stored rather than slept on.
    stream.warm_throughput_mi_bps = Some(input.warm_throughput_mi_bps);
    stream.updating_until = Some(now + crate::state::WARM_THROUGHPUT_UPDATE_DELAY);

    Ok(UpdateStreamWarmThroughputOutput {
        stream_name: Some(stream.name.clone()),
        stream_arn: Some(stream.arn.clone()),
        // "Specifies the updated warm throughput configuration for your data stream": the
        // configuration the stream now carries, read back off the stored record, which is the
        // same one every later read of it reports.
        warm_throughput: stream.warm_throughput(),
    })
}

impl<Eng> UpdateStreamWarmThroughput<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for UpdateStreamWarmThroughput<Eng>
{
    type Input = UpdateStreamWarmThroughputInput;
    type Output = UpdateStreamWarmThroughputOutput;
    type Error = UpdateStreamWarmThroughputError;

    const NAME: &'static str = "UpdateStreamWarmThroughput";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STREAM_WARM_THROUGHPUT_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::UPDATE_STREAM_WARM_THROUGHPUT_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The credential scope's access key id is read only to ATTRIBUTE the change
        // ([`audit`]); the region it names is the region whose streams this request addresses,
        // since a stream name "is also scoped by Amazon Web Services Region".
        let akid = ::emulator_server::extract_akid(&ctx.parts().headers);
        let region = crate::state::request_region(ctx);

        let state = crate::state::service_state(&*self.engine).ok_or_else(|| {
            audit(&akid, &region, "unavailable");
            UpdateStreamWarmThroughputError::from(
                UpdateStreamWarmThroughputErrorModeled::InternalFailureException(
                    InternalFailureException {
                        message: Some("the stream store is unavailable".to_string()),
                    },
                ),
            )
        })?;

        let now = ::std::time::SystemTime::now();
        let outcome = {
            let mut store = state.lock();
            apply_update(&mut store, &region, now, &input)
        };
        audit(
            &akid,
            &region,
            if outcome.is_ok() { "updated" } else { "refused" },
        );
        outcome
    }
}
