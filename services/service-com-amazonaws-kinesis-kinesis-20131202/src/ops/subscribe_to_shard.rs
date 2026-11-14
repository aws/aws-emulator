// SPDX-License-Identifier: CC0-1.0

//! @op SubscribeToShard
//!
//! `POST / -> 200` (`X-Amz-Target: Kinesis_20131202.SubscribeToShard`). Subscribes one
//! registered enhanced fan-out consumer to one shard of the stream its `ConsumerARN` extends
//! and answers the modelled `SubscribeToShardEventStream` as a `vnd.amazon.eventstream` body
//! of `SubscribeToShardEvent` frames. This is the ONLY file to touch for this operation.
//!
//! This operation is hand-authored rather than generated: its output member `EventStream`
//! targets a `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim declines the
//! request. The framing and the routing seam are `service_common::event_stream` and
//! `src/service.rs`; this file holds the behaviour, and the shapes are in
//! `src/ops/types/subscribe_to_shard_types.rs`.
//!
//! ## What a subscription answers
//!
//! "This operation establishes an HTTP/2 connection between the consumer you specify in the
//! ConsumerARN parameter and the shard you specify in the ShardId parameter. After the
//! connection is successfully established, Kinesis Data Streams pushes records from the shard
//! to the consumer over this connection." The subscription this emulator serves is ONE
//! response rather than a connection held open, so it carries the frame a client reads the
//! output shape out of (`service_common::event_stream::INITIAL_RESPONSE_EVENT`, carrying no
//! member, because the stream is this output's only one) and then ONE
//! `SubscribeToShardEvent` over the records the shard holds from the position the request
//! named — read out of the stored shard, never synthesised, by the same shared read of a
//! shard `GetRecords` serves its page from ([`crate::state::read_shard_from`]), so the push
//! reader and the pull reader deliver the same records in the same order.
//!
//! The records one event carries are bounded by the ceilings that shared read is given
//! ([`MAX_EVENT_RECORDS`] / [`MAX_EVENT_BYTES`]), and the event reports where the reader now
//! stands: `ContinuationSequenceNumber` — "Use this as SequenceNumber in the next call to
//! SubscribeToShard, with StartingPosition set to AT_SEQUENCE_NUMBER or
//! AFTER_SEQUENCE_NUMBER. Use ContinuationSequenceNumber for checkpointing because it
//! captures your shard progress even when no data is written to the shard" — and
//! `MillisBehindLatest`, how far behind the tip of the shard the subscription is.
//! `ChildShards` is carried only "at the end of the current shard", which is a shard a
//! reshard closed and whose retained records the subscription has delivered.
//!
//! A successful subscription is RECORDED on the consumer it was established for
//! ([`crate::state::ConsumerRecord::subscribed_at`]), because the next one is judged against
//! it: "If you call SubscribeToShard again with the same ConsumerARN and ShardId within 5
//! seconds of a successful call, you'll get a ResourceInUseException. If you call
//! SubscribeToShard 5 seconds or more after a successful call, the second call takes over
//! the subscription."
//!
//! ## Input
//!
//! * `ConsumerARN` (ConsumerARN, len 1..2048, pattern
//!   `^(arn):aws.*:kinesis:.*:\d{12}:.*stream\/[a-zA-Z0-9_.-]+\/consumer\/[a-zA-Z0-9_.-]+:[0-9]+$`)
//!   **required** — the consumer to subscribe. "Before you call this operation, call
//!   RegisterStreamConsumer to register the consumer with Kinesis Data Streams." A consumer's
//!   ARN is its stream's ARN extended, so it names the stream too
//!   ([`crate::state::stream_arn_of_consumer`]).
//! * `ShardId` (ShardId, len 1..128, pattern `^[a-zA-Z0-9_.-]+$`) **required** — the shard of
//!   that stream to read.
//! * `StartingPosition` (StartingPosition) **required** — "The starting position in the data
//!   stream from which to start streaming", one of the five `ShardIteratorType` positions
//!   with the member each of them reads ([`crate::state::requested_shard_position`]).
//! * `StreamId` (StreamId, len 1..24, pattern `^[a-z0-9]{20}-[a-z0-9]{3}$`) — "Not
//!   Implemented. Reserved for future use", so no stream carries one.
//! * `DryRun` (BooleanObject) — "Checks if your request will succeed."
//!
//! ## Output
//!
//! * `EventStream` (SubscribeToShardEventStream, `@streaming`) **required** — the body. Its
//!   `SubscribeToShardEvent` member is the data event; its `KMSDisabledException` member and
//!   the rest of the KMS family are in-band failures of reading a stream encrypted under a
//!   CMK the data plane may no longer use, which is why no KMS error is among this
//!   operation's own modelled errors.
//!
//! ## Errors
//!
//! * `ResourceNotFoundException` (HTTP 400) — the `ConsumerARN` names a stream this account
//!   and region do not hold, a consumer that stream does not carry, or the `ShardId` names no
//!   shard of it.
//! * `ResourceInUseException` (HTTP 400) — the stream is not `ACTIVE`, the consumer is not
//!   `ACTIVE` yet ("a consumer can't read data while in the CREATING or DELETING states"), or
//!   this consumer already holds a subscription to this shard.
//! * `InvalidArgumentException` (HTTP 400) — the `StartingPosition` members name no position
//!   in the shard.
//! * `DryRunOperationException` (HTTP 400) — "The request was rejected because the DryRun
//!   parameter was specified."
//! * `AccessDeniedException`, `LimitExceededException` (HTTP 400) — this operation's
//!   remaining modelled errors; no condition of this store raises them.
//! * `InternalFailureException` (HTTP 500) — widened in from the service's own error set: the
//!   only honest answer for a service mounted on an engine that holds no state for it.

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_only_response, frame_empty_initial_response, frame_event,
    read_request, RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, Service};
use std::sync::Arc;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative `smithy!`
// macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-kinesis-kinesis-20131202.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it, and every one of them is a client error at HTTP 400.
#[smithy_macros::operation_error(
    "com.amazonaws.kinesis#SubscribeToShard",
    // `InternalFailureException` is widened in with `extra_errors`: this SERVICE models it
    // ("The processing of the request failed because of an unknown error, exception, or
    // failure"), the operation shape does not list it, and it is the only honest answer for a
    // service mounted on an engine that holds no state for it.
    extra_errors = ["InternalFailureException"]
)]
#[derive(Debug)]
pub enum SubscribeToShardErrorModeled {
    AccessDeniedException(AccessDeniedException),
    DryRunOperationException(DryRunOperationException),
    InvalidArgumentException(InvalidArgumentException),
    LimitExceededException(LimitExceededException),
    ResourceInUseException(ResourceInUseException),
    ResourceNotFoundException(ResourceNotFoundException),
    InternalFailureException(InternalFailureException),
}

/// The `SubscribeToShardEventStream` member a data event is framed as, which the frame
/// carries in its `:event-type` header (source: `smithy_query(query="shape",
/// shape="SubscribeToShardEventStream")`).
const SUBSCRIBE_TO_SHARD_EVENT: &str = "SubscribeToShardEvent";

/// The `SubscribeToShardEventStream` member an in-band failure to read the stream's records
/// under its CMK is framed as (same source).
const KMS_DISABLED_EVENT: &str = "KMSDisabledException";

/// The members a `SubscribeToShard` request spells a starting position with, as its refusals
/// name them: this operation carries all three inside its `StartingPosition` structure.
const POSITION_MEMBERS: crate::state::PositionMembers = crate::state::PositionMembers {
    position_type: "StartingPosition.Type",
    sequence_number: "StartingPosition.SequenceNumber",
    timestamp: "StartingPosition.Timestamp",
};

/// The most bytes of record DATA one `SubscribeToShardEvent` carries: the ceiling a single
/// pull read of a shard answers with ("The maximum size of data that GetRecords can return is
/// 10 MiB", `GetRecords`), because what bounds one delivery of a shard's records is a
/// property of the shard rather than of which reader asked for them. An event that reached it
/// still carries its first record, and its `ContinuationSequenceNumber` is what the consumer
/// reads on from.
const MAX_EVENT_BYTES: usize = crate::state::MAX_GET_RECORDS_BYTES;

/// The most records one such event carries, for the same reason ("The maximum number of
/// records that can be returned per call is 10,000", `GetRecords`).
fn max_event_records() -> usize {
    usize::try_from(crate::state::MAX_GET_RECORDS_LIMIT).unwrap_or(1)
}

/// The `SubscribeToShard` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds one of
/// these and `service.rs` holds another wrapped in an `EventStreamRoute`; both reach the same
/// state, because the state lives in the shared engine, not on the operation.
pub struct SubscribeToShard<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> SubscribeToShard<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any request
    /// runs.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Refuse the request with the error this operation models for a parameter that "exceeds its
/// restrictions, is not supported, or can't be used".
fn invalid_argument(message: String) -> SubscribeToShardErrorModeled {
    SubscribeToShardErrorModeled::InvalidArgumentException(InvalidArgumentException {
        message: Some(message),
    })
}

/// The modelled refusal for a resource this account and region do not hold: "The requested
/// resource could not be found. The stream might not be specified correctly."
fn not_found(message: String) -> SubscribeToShardErrorModeled {
    SubscribeToShardErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        message: Some(message),
    })
}

/// The modelled refusal for a resource that is not available for this operation: "For
/// successful operation, the resource must be in the ACTIVE state" (`ResourceInUseException`).
fn in_use(message: String) -> SubscribeToShardErrorModeled {
    SubscribeToShardErrorModeled::ResourceInUseException(ResourceInUseException {
        message: Some(message),
    })
}

/// A failure of this emulator rather than of the request.
fn internal(message: String) -> SubscribeToShardErrorModeled {
    SubscribeToShardErrorModeled::InternalFailureException(InternalFailureException {
        message: Some(message),
    })
}

impl<Eng: Send + Sync + 'static> SubscribeToShard<Eng> {
    /// Resolve the subscription the request asks for, refuse whatever stands in the way, and
    /// RECORD the subscription it established.
    ///
    /// The whole resolution lives here so the store's lock is held for exactly this long,
    /// which is also what admits exactly ONE of two overlapping subscriptions of the same
    /// consumer to the same shard: the one that gets the lock first records it, and the next
    /// one reads that record and is refused.
    fn establish(
        store: &mut crate::state::ComAmazonawsKinesisKinesis20131202State,
        region: &str,
        now: std::time::SystemTime,
        input: &SubscribeToShardInput,
    ) -> Result<SubscribeToShardEvent, SubscribeToShardErrorModeled> {
        // "Before you call this operation, call RegisterStreamConsumer to register the
        // consumer with Kinesis Data Streams": the consumer's ARN names the stream it was
        // registered with, so the stream is resolved first and the consumer inside it — a
        // consumer of a stream this account and region do not hold is not found under the
        // stream, which is what the refusal states.
        let stream_arn = crate::state::stream_arn_of_consumer(&input.consumer_arn);
        let stream = store
            .stream_by_arn(now, region, stream_arn)
            .ok_or_else(|| not_found(crate::state::stream_not_found_message(stream_arn)))?;
        let stream_key = crate::state::RegionalName::new(&stream.region, &stream.name);

        // A stream whose create, removal or re-tiling is still due serves no shard to read,
        // which is the modelled "the resource must be in the ACTIVE state" refusal. A stream
        // whose shard count is being changed DOES serve the read: "you can continue to read
        // and write data to your stream while its status is UPDATING" (`UpdateShardCount`).
        let status = stream.status(now);
        if !stream.serves_shards(now) {
            return Err(in_use(crate::state::stream_not_active_message(
                &stream.name,
                status,
            )));
        }

        let consumer = stream
            .consumer_by_arn(&input.consumer_arn)
            .ok_or_else(|| {
                not_found(crate::state::consumer_not_found_message(
                    &input.consumer_arn,
                    &stream.arn,
                ))
            })?;

        // "A consumer can't read data while in the CREATING or DELETING states"
        // (`Consumer$ConsumerStatus`), so a consumer whose registration has not settled yet
        // is not available for this operation.
        let consumer_status = consumer.status(now);
        if consumer_status != crate::state::CONSUMER_ACTIVE {
            return Err(in_use(crate::state::consumer_not_active_message(
                &consumer.name,
                consumer_status,
            )));
        }

        // "If you call SubscribeToShard again with the same ConsumerARN and ShardId within 5
        // seconds of a successful call, you'll get a ResourceInUseException."
        if consumer.holds_subscription(&input.shard_id, now) {
            return Err(in_use(crate::state::consumer_already_subscribed_message(
                &input.consumer_arn,
                &input.shard_id,
            )));
        }

        let shard = stream
            .shards
            .iter()
            .find(|shard| shard.shard_id == input.shard_id)
            .ok_or_else(|| {
                // A shard of another stream, and a shard id nothing laid down, are the same
                // thing to this stream: a shard it does not hold.
                not_found(crate::state::shard_not_in_stream_message(
                    &input.shard_id,
                    &stream.name,
                ))
            })?;

        // Where in the shard the subscription starts, by this service's one rule for naming a
        // position in a shard — the rule `GetShardIterator` resolves its iterator with.
        let trim_horizon = stream.trim_horizon(now);
        let position = crate::state::requested_shard_position(
            POSITION_MEMBERS,
            input.starting_position.r#type.as_str(),
            input.starting_position.sequence_number.as_deref(),
            input.starting_position.timestamp,
            shard,
            trim_horizon,
        )
        .map_err(invalid_argument)?;

        // "Checks if your request will succeed. DryRun is an optional parameter"
        // (`SubscribeToShardInput$DryRun`): the request is judged in full above and then
        // refused with the error the model declares for it, stating that it WOULD have
        // succeeded, so a dry run establishes no subscription and reads no record.
        if input.dry_run.unwrap_or(false) {
            return Err(SubscribeToShardErrorModeled::DryRunOperationException(
                DryRunOperationException {
                    message: Some(crate::state::dry_run_succeeded_message(
                        "SubscribeToShard",
                    )),
                },
            ));
        }

        // A record outside the stream's retention window is trimmed, so it is not among the
        // records any position of this shard reads.
        let retained: Vec<&crate::state::DataRecord> = shard.retained(trim_horizon).collect();
        let read = crate::state::read_shard_from(
            &position,
            &retained,
            max_event_records(),
            MAX_EVENT_BYTES,
            now,
        );
        let event = SubscribeToShardEvent {
            records: read.records.iter().map(|record| record.to_shape()).collect(),
            // "Use ContinuationSequenceNumber for checkpointing because it captures your
            // shard progress even when no data is written to the shard": the newest record
            // this consumer has now seen, and the shard's own first sequence number while it
            // has seen none — every record the shard can hold is numbered past that one, so
            // reading on AFTER it delivers all of them.
            continuation_sequence_number: read.last_seen.map_or_else(
                || shard.starting_sequence_number.clone(),
                |record| record.sequence_number.clone(),
            ),
            millis_behind_latest: read.millis_behind_latest,
            // "The list of the child shards of the current shard, returned only at the end of
            // the current shard": a shard is closed by a reshard, and a consumer that has
            // been delivered everything a closed one retains reads on from the shards that
            // reshard produced from it.
            child_shards: (!shard.is_open() && read.drained)
                .then(|| stream.child_shards_of(&shard.shard_id))
                .filter(|children| !children.is_empty()),
        };

        // The subscription is established, so it is recorded on the consumer it was
        // established for: that is what the next call to the same consumer and shard is
        // judged against.
        if let Some(consumer) = store
            .streams
            .get_mut(&stream_key)
            .and_then(|stream| stream.consumer_by_arn_mut(&input.consumer_arn))
        {
            consumer.subscribed_at(&input.shard_id, now);
        }

        Ok(event)
    }

    /// Read the request into the modelled input shape and check it against the model.
    ///
    /// Driven through the service's own protocol, so the body codec is the protocol's, and a
    /// request the model refuses is answered through this service's OWN refusal seam
    /// ([`smithy_server_runtime::Service::override_refusal`], which `src/service.rs` words
    /// through one module) rather than the framework's default envelope — so a bad request to
    /// this hand-authored operation reads exactly as a bad request to any other operation of
    /// this service.
    ///
    /// The refusal is BOXED because a `Response` is large and this is the error half of a
    /// result (clippy's `result_large_err`).
    fn read_input(
        &self,
        svc: &dyn Service,
        req: &Request,
    ) -> Result<SubscribeToShardInput, Box<Response>> {
        // The definition is read, used and dropped inside this statement: `&dyn
        // ErasedServiceDefinition` is not `Sync`, so holding it across an await would make
        // the serving future non-`Send`.
        let read = read_request::<SubscribeToShardInput>(
            svc.definition().protocol(),
            req,
            &SubscribeToShardInput::SCHEMA,
        );
        match read {
            Ok(input) => Ok(input),
            Err(RequestRefusal::Invalid(violations)) => {
                let stated = svc.override_refusal(&Refusal::Invalid(violations.clone()));
                Err(Box::new(svc.definition().refusal_response(
                    ProtocolError::Invalid(violations),
                    stated,
                    req,
                )))
            }
            // A body the protocol's codec could not read at all is not a constraint refusal
            // and has no violation form: it keeps the envelope the framework's own boundary
            // answers one with.
            Err(RequestRefusal::Unreadable(message)) => Err(Box::new(
                svc.definition()
                    .framework_error_response(ProtocolError::Malformed(message), req),
            )),
        }
    }

    /// The whole subscription: the stream's own CMK, then the event the shard's records make
    /// up, framed as this service's event-stream body.
    async fn respond(
        &self,
        svc: &dyn Service,
        ctx: &OperationContext<'_>,
        region: &str,
        now: std::time::SystemTime,
        input: &SubscribeToShardInput,
    ) -> Response {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return error_response(
                svc.definition().protocol(),
                &internal("the stream store is unavailable".to_string()),
            );
        };

        // The stream's own CMK decides whether its data plane serves this read at all, so the
        // key is resolved against KMS first — exactly as the pull reader resolves it
        // (`crate::ops::get_records`). The store guard is taken only to read which key the
        // subscribed stream carries, because no guard is held across an await; every refusal
        // about the subscription itself is answered by the body below, which reads the store
        // again.
        let encryption_key = {
            let store = state.lock();
            store
                .stream_by_arn(
                    now,
                    region,
                    crate::state::stream_arn_of_consumer(&input.consumer_arn),
                )
                .and_then(|stream| stream.encryption_key().map(str::to_string))
        };
        let mut disabled_key: Option<String> = None;
        if let Some(key_id) = encryption_key {
            if let crate::state::KeyState::Disabled(key_arn) =
                crate::state::key_state(ctx, region, &key_id).await
            {
                disabled_key = Some(key_arn);
            }
        }

        // A stream whose CMK the data plane may no longer use delivers no record. The
        // `SubscribeToShardEventStream` union is where this operation models that failure
        // (`KMSDisabledException` is a member of the union and not one of the operation's own
        // errors), so it travels as an in-band exception frame of the established stream
        // rather than as a refusal of the call.
        if let Some(key_arn) = disabled_key {
            let protocol = svc.definition().protocol();
            let event = KMSDisabledException {
                message: Some(crate::state::key_disabled_message(&key_arn)),
            };
            return match frame_empty_initial_response(protocol).and_then(|mut frames| {
                frames.extend(frame_event(
                    protocol,
                    KMS_DISABLED_EVENT,
                    true,
                    &event,
                    &SCHEMA_KMS_DISABLED_EXCEPTION,
                )?);
                Ok(frames)
            }) {
                Ok(frames) => event_stream_only_response(frames),
                Err(error) => error_response(
                    protocol,
                    &internal(format!("the event stream could not be framed: {error}")),
                ),
            };
        }

        let established = {
            let mut store = state.lock();
            Self::establish(&mut store, region, now, input)
        };
        let protocol = svc.definition().protocol();
        let event = match established {
            Ok(event) => event,
            Err(error) => return error_response(protocol, &error),
        };

        // The body opens with the frame a client reads the OUTPUT shape out of, before any
        // event of the subscription: `SubscribeToShardOutput`'s only member is the stream
        // itself, so that frame carries no member.
        match frame_empty_initial_response(protocol).and_then(|mut frames| {
            frames.extend(frame_event(
                protocol,
                SUBSCRIBE_TO_SHARD_EVENT,
                false,
                &event,
                &SubscribeToShardEvent::SCHEMA,
            )?);
            Ok(frames)
        }) {
            Ok(frames) => event_stream_only_response(frames),
            Err(error) => error_response(
                protocol,
                &internal(format!("the event stream could not be framed: {error}")),
            ),
        }
    }
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for SubscribeToShard<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let input = match self.read_input(svc, req) {
                Ok(input) => input,
                Err(refused) => return *refused,
            };
            let region = crate::state::request_region(ctx);
            let now = std::time::SystemTime::now();

            let response = self.respond(svc, ctx, &region, now, &input).await;
            // One access record per request, so a refused subscription is diagnosable. No
            // request payload and no credential reaches it.
            tracing::debug!(
                operation = "SubscribeToShard",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
