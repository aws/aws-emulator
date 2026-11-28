// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-kinesis-kinesis-20131202` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## This service's edited seam: how a broken input constraint is stated
//!
//! Kinesis words a constraint refusal FROM THE REQUEST — the member path, the value as
//! submitted, and the requirement as the service itself states it ([`validation`]). Two parts
//! of that sentence the framework cannot produce on its own, both of them the value half of a
//! clause. A SCALAR value it stores SHORTENED (a byte-bounded prefix plus a marker naming the
//! byte count, text AWS never emits), because a clause interpolates whatever is stored and a
//! caller controls the length. A COLLECTION it stores as a count summary (`<n> items`), which
//! is no value any caller sent, because a `@length` on a list is checked on the member before
//! the walk descends into its elements. Both are only recoverable from the built input, which
//! [`Service::override_refusal`] is never handed — it is given the violations alone.
//!
//! [`StatedPerOperation`] is where they come from: each operation this crate serves is
//! mounted wrapped in it, the wrapper asks `service_common::validation` for the framework's
//! OWN violations against that operation's input schema together with the echo each clause
//! can state its value from, the two shared restoring rules in
//! `service_common::validation_message` put those values back ([`stated_refusal`]), and
//! [`validation`] turns them into the sentence. `override_refusal` states a refusal that
//! reaches the centralized walk anyway through that same module, so the wording lives in ONE
//! place.
//!
//! The refusals a HANDLER words — a stream that is not there, a shard that does not exist, a
//! dry run — are not here: they are built by the shared helpers in [`crate::state`] so every
//! operation states them one way.
//!
//! ## This service's second edited seam: who may perform the action at all
//!
//! Every request to an AWS service is decided at the IAM layer before the service acts on
//! the stream it names, so every operation mounted below is served behind
//! [`authorization::Authorized`]: the principal the request's credential speaks for is
//! resolved, the documents in force on it are evaluated against
//! `kinesis:<OperationName>` on the resource the request addresses, and a call they do not
//! allow is refused with `AccessDeniedException` instead of reaching the handler. The gate
//! is one wrapper rather than a call in each of the 57 handlers because the rule is the
//! SERVICE's and not one operation's.
//!
//! ## This service's third edited seam: the operation whose output is an event stream
//!
//! `SubscribeToShard`'s output member `EventStream` targets a `@streaming` union, so the
//! generated `ErasedOperation` shim of it declines the request (it cannot drive
//! `serialize_as_event_stream<P>` through an object-safe `run`) and the op-stub generator
//! emitted no handler for it at all (`src/ops/SKIPPED_STUBS.md`). It is served by the
//! hand-authored [`crate::ops::subscribe_to_shard`] behind a
//! [`EventStreamRoute`], which [`EmulatedService::override_operation`] — the seam the router
//! consults FIRST — hands back for it. That operation reads its own request, so it is not
//! behind [`StatedPerOperation`]; it words a refused request through
//! [`Service::override_refusal`] below, which is the same module and therefore the same
//! sentence — and it asks the authorization gate itself, through
//! [`authorization_refusal`], for the same reason.
//!
//! ## This service's fourth edited seam: the ENCODING a request asked for
//!
//! `awsJson1_1` is a body-only RPC protocol, and which ENCODING the body is in is the
//! caller's choice, stated in `Content-Type`: `application/x-amz-json-1.1` carries the
//! input's members as JSON and `application/x-amz-cbor-1.1` carries the same members as
//! CBOR. The framework selects a codec per request from the protocols the MODEL declares,
//! and a model declares a protocol rather than an encoding of one, so every request was
//! handed to the JSON codec whatever it asked for. [`cbor`] serves the second encoding —
//! the framework's own CBOR codec, through the framework's own RPC protocol and its own
//! per-request sequence, so an operation behaves identically in the two — and every answer,
//! a success, a modelled error and a refusal alike, is encoded as the request was
//! ([`answered_before_the_handler`], [`StatedPerOperation::run`]).
//!
//! ## This service's fifth edited seam: a value no member could be read from
//!
//! A body value the modelled member it is bound to cannot be read from never becomes a
//! value, so no constraint on it is ever reached: it is a SERIALIZATION fault and a
//! different error class from a constraint refusal, which is what a client branches on.
//! [`unreadable_values`] answers that class — in the encoding the request arrived in — ahead
//! of the constraint walk above, because that is the order the two layers run in.

mod authorization;
mod cbor;
mod submitted_batch;
mod unreadable_values;
mod validation;

/// This service's authorization gate, as the one hand-authored operation asks it: every other
/// operation is served behind [`authorization::Authorized`], which asks it for them.
pub(crate) use authorization::refusal as authorization_refusal;

/// This service's wording for a broken input constraint, as a HANDLER that collected a
/// violation itself asks it: the clause rules — the value set of an enumerated type included —
/// stay in [`validation`], so the two layers cannot word one refusal differently.
pub(crate) use validation::stated_message as stated_constraint_message;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::EventStreamRoute;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

use unreadable_values::Wire;

/// The `com-amazonaws-kinesis-kinesis-20131202` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The one generated definition of this service, held behind an [`Arc`] because the view
    /// of it that serves the CBOR representation ([`cbor::ServedInCbor`]) is the SAME
    /// definition — one identity, one routing table, one set of model-derived facts.
    entry: Arc<crate::ServiceDefinition<Eng>>,
    /// The same operations the generated table holds, each wrapped in
    /// [`authorization::Authorized`] (IAM decides the call) inside [`StatedPerOperation`] (a
    /// refused request is worded the way this service words it). Keyed by the routing key the
    /// protocol resolves — the key the generated table uses — and built from the SAME shared
    /// engine, so a wrapped operation reads and writes the one store.
    stated: OperationRoutingTable,
    /// This service's one event-stream operation, `SubscribeToShard`. It is NOT in the
    /// generated table in a servable form — the shim of an operation whose output is a
    /// `@streaming` union declines the request — so it is held here behind an
    /// [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`], built from
    /// the same shared engine as everything else.
    subscribe_to_shard: EventStreamRoute<crate::ops::subscribe_to_shard::SubscribeToShard<Eng>>,
}

/// The operation this service serves as an event stream, by the key the protocol resolves.
/// Routed by [`EmulatedService::override_operation`] rather than through the generated table.
const SUBSCRIBE_TO_SHARD_OPERATION: &str = "SubscribeToShard";

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`authorization::Authorized`] (IAM decides the call) inside [`StatedPerOperation`] (a
/// refused request is worded the way this service words it) and keyed by its operation shape
/// name — the routing key `ServerHttpProtocol::routing_key` resolves and the generated table
/// is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $in_cbor:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    authorization::Authorized::new(
                        crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                        ::std::sync::Arc::clone(&$engine),
                    ),
                    ::std::sync::Arc::clone(&$in_cbor),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let entry = Arc::new(crate::ServiceDefinition::new(Arc::clone(&engine)));
        // The same service, as the framework's per-request sequence reads it for a request
        // in the CBOR representation of this service's protocol: type-erased, so an
        // operation wrapper names no engine.
        let in_cbor: Arc<dyn Service> = Arc::new(cbor::ServedInCbor::new(Arc::clone(&entry)));
        let mut stated = OperationRoutingTable::new();
        // EVERY operation this crate serves (one per file in `src/ops/`), because the rule is
        // the service's and not one operation's: an operation whose input declares no
        // constraint simply never has a violation to state, which the wrapper reads off that
        // operation's own input schema. An operation added later is added here as well, or it
        // answers a refusal in the framework's wording rather than this service's; the tests
        // below fail when one is missing.
        wrap_operations!(
            stated,
            engine,
            in_cbor,
            acknowledge_dead_letter_queue_records::AcknowledgeDeadLetterQueueRecords,
            acknowledge_queue_records::AcknowledgeQueueRecords,
            add_tags_to_stream::AddTagsToStream,
            create_channel::CreateChannel,
            create_queue::CreateQueue,
            create_stream::CreateStream,
            decrease_stream_retention_period::DecreaseStreamRetentionPeriod,
            delete_channel::DeleteChannel,
            delete_queue::DeleteQueue,
            delete_resource_policy::DeleteResourcePolicy,
            delete_stream::DeleteStream,
            deregister_stream_consumer::DeregisterStreamConsumer,
            describe_account_settings::DescribeAccountSettings,
            describe_channel::DescribeChannel,
            describe_limits::DescribeLimits,
            describe_queue::DescribeQueue,
            describe_stream::DescribeStream,
            describe_stream_consumer::DescribeStreamConsumer,
            describe_stream_summary::DescribeStreamSummary,
            disable_enhanced_monitoring::DisableEnhancedMonitoring,
            enable_enhanced_monitoring::EnableEnhancedMonitoring,
            get_records::GetRecords,
            get_resource_policy::GetResourcePolicy,
            get_shard_iterator::GetShardIterator,
            increase_stream_retention_period::IncreaseStreamRetentionPeriod,
            list_channels::ListChannels,
            list_queues::ListQueues,
            list_shards::ListShards,
            list_stream_consumers::ListStreamConsumers,
            list_streams::ListStreams,
            list_tags_for_resource::ListTagsForResource,
            list_tags_for_stream::ListTagsForStream,
            merge_shards::MergeShards,
            put_record::PutRecord,
            put_records::PutRecords,
            put_resource_policy::PutResourcePolicy,
            receive_dead_letter_queue_records::ReceiveDeadLetterQueueRecords,
            receive_queue_records::ReceiveQueueRecords,
            register_stream_consumer::RegisterStreamConsumer,
            remove_tags_from_stream::RemoveTagsFromStream,
            restore_dead_letter_queue_records::RestoreDeadLetterQueueRecords,
            split_shard::SplitShard,
            start_stream_encryption::StartStreamEncryption,
            stop_stream_encryption::StopStreamEncryption,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_account_settings::UpdateAccountSettings,
            update_channel::UpdateChannel,
            update_dead_letter_queue_records_visibility::UpdateDeadLetterQueueRecordsVisibility,
            update_max_record_size::UpdateMaxRecordSize,
            update_queue::UpdateQueue,
            update_queue_records_visibility::UpdateQueueRecordsVisibility,
            update_shard_count::UpdateShardCount,
            update_stream_mode::UpdateStreamMode,
            update_stream_record_distribution_strategy::UpdateStreamRecordDistributionStrategy,
            update_stream_warm_throughput::UpdateStreamWarmThroughput,
        );
        Self {
            entry,
            stated,
            subscribe_to_shard: EventStreamRoute(
                crate::ops::subscribe_to_shard::SubscribeToShard::new(Arc::clone(&engine)),
            ),
        }
    }
}

/// One operation, served so that a refused request is stated the way this service states it:
/// the value as submitted rather than the framework's shortened rendering.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery ([`stated_refusal`]), so this cannot refuse a request the framework's own
/// walk would accept, nor accept one it would refuse. The cost is that a request this wrapper
/// does not refuse is read twice, once here and once in the operation's own sequence, because
/// the framework hands no already-built input to an operation. The structurally right place
/// for the collecting half is the frozen crate — handing a handler, or `override_refusal`,
/// the input it already built — which `services/common/src/validation.rs` records as the core
/// seam this re-read exists only until.
struct StatedPerOperation<O> {
    operation: O,
    /// This service as the framework's per-request sequence reads it for a request in the
    /// CBOR representation of its protocol ([`cbor::ServedInCbor`]), type-erased so this
    /// wrapper names no engine. Held here because the framework's codec seam is per SERVICE
    /// rather than per request, so the encoding a request asked for is served by running it
    /// against this view of the same service.
    in_cbor: Arc<dyn Service>,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O, in_cbor: Arc<dyn Service>) -> Self {
        Self { operation, in_cbor }
    }
}

/// This service's answer to every modelled constraint `req` breaks, for the operation `O`:
/// the violations the framework's own walk finds, and the wording [`validation`] gives them.
/// `None` when the request breaks none — and for a failure that is NOT a refusal (a body the
/// codec cannot read at all, a member of the wrong type), which the framework's own walk
/// answers in its own envelope.
///
/// The collecting half is the tree's shared one,
/// `service_common::validation::refusing_violations_with_declared_echoes_rendering_up_to`:
/// one read of the request yields the framework's own violations (the protocol's
/// deserializer for an absent `@required` member, `constraints::validate` over the built
/// input) together with the echo each clause can state its value from. Nothing is re-checked
/// here, no second read of the request lives in this crate, and the two restorations below
/// are the shared rules for the two halves this service's clauses state:
///
/// * the whole value the caller SUBMITTED where the framework stored its own elision of one
///   (`validation_message::quoting_the_whole_submitted_value`);
/// * the COLLECTION the caller submitted where the framework stored its own `<n> items`
///   summary of one (`validation_message::quoting_the_submitted_collection`) — the clause a
///   `@length` on a list earns, which is checked on the member before the walk descends into
///   the elements.
///
/// The two CEILINGS passed are MODEL constants, from the largest list this service's model
/// declares ([`submitted_batch`]), so no response is sized by the request: past them the
/// framework's own summary stands.
///
/// A violation found on a list ELEMENT is stated at the LIST MEMBER, quoting the whole list
/// the request carried ([`validation`] rule 6), which no violation carries: that reading is
/// the tree's own (`service_common::validation::refusing_violations_with_lists`) and is asked
/// for ONLY when such a violation is among them, so every other request — and every request
/// that satisfies the model — pays nothing for it. It is the one place this crate reads a
/// request twice, for the same reason the re-read above exists: the framework hands no
/// already-built input over, and the echo mode this service's own clauses need is not the one
/// that collects the lists.
fn stated_refusal<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<Violation>, Refused)> {
    use ::service_common::validation_message::{
        quoting_the_submitted_collection, quoting_the_whole_submitted_value,
    };
    let violations: Vec<Violation> =
        ::service_common::validation::refusing_violations_with_declared_echoes_rendering_up_to::<O>(
            protocol,
            req,
            submitted_batch::QUOTED_BATCH_POSITIONS,
            submitted_batch::QUOTED_BATCH_RENDERING_BYTES,
        )
        .into_iter()
        .map(|(violation, echo)| {
            let violation = quoting_the_whole_submitted_value(violation, &echo);
            quoting_the_submitted_collection(violation, echo)
        })
        .collect();
    let stated = validation::refusal(&violations, || {
        ::service_common::validation::refusing_violations_with_lists::<O>(protocol, req).1
    })?;
    Some((violations, stated))
}

/// This service's whole answer to `req` BEFORE the handler runs, for the operation `O` it
/// was routed to, in the encoding `wire` names — or `None` when nothing refuses it and the
/// operation is to serve it.
///
/// The two refusals are different error CLASSES and happen in this order, which is the order
/// the two layers run in: a value the body could not be read from never becomes a value, so
/// no constraint on it is ever reached ([`unreadable_values`]); only a body that WAS read has
/// constraints to break ([`stated_refusal`]). Both are answered in the encoding the request
/// arrived in, so a client reads its own refusal.
///
/// It takes the definition rather than the service because `&dyn ErasedServiceDefinition` is
/// not `Sync`: the whole answer is computed in one statement, so nothing non-`Sync` is held
/// across the await in [`StatedPerOperation::run`].
fn answered_before_the_handler<O: Operation>(
    definition: &dyn ErasedServiceDefinition,
    wire: Wire,
    req: &RtRequest,
) -> Option<RtResponse> {
    let protocol: &dyn ServerHttpProtocol = match wire {
        Wire::Cbor => cbor::protocol(),
        Wire::Json => definition.protocol(),
    };
    if let Some(answer) = unreadable_values::refusal_for::<O>(protocol, wire, req) {
        return Some(answer);
    }
    let (violations, stated) = stated_refusal::<O>(protocol, req)?;
    Some(match wire {
        Wire::Cbor => cbor::refusal_response(violations, stated),
        Wire::Json => {
            definition.refusal_response(ProtocolError::Invalid(violations), Some(stated), req)
        }
    })
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The encoding of this service's protocol the caller asked for. Every step of
            // the request — what refuses it, what reads its body, what its answer is encoded
            // as — is that one encoding.
            let wire = Wire::of(req);
            // The definition is read, used and dropped inside this one statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            if let Some(answer) = answered_before_the_handler::<O>(svc.definition(), wire, req) {
                return answer;
            }
            match wire {
                // The framework's own per-request sequence, read in the encoding the caller
                // asked for: the operation, its state and its modelled errors are the same.
                Wire::Cbor => {
                    cbor::answer::<O>(&self.operation, self.in_cbor.as_ref(), req, ctx).await
                }
                Wire::Json => self.operation.run(svc, req, ctx).await,
            }
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        self.entry.as_ref()
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped in `StatedPerOperation` so
        // a refused request is worded the way this service words it. A key the model does
        // not name, or one no handler in `src/ops/` serves, resolves to `None` here exactly
        // as it does in the generated table, so the router's routing-error path is
        // unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        // The exception is the event-stream operation, whose generated shim in that table
        // declines the request: it is served by its hand-authored operation, which owns the
        // whole request-to-response step and therefore reads its own request.
        if key == SUBSCRIBE_TO_SHARD_OPERATION {
            return Some(&self.subscribe_to_shard as &dyn ErasedOperation);
        }
        self.stated.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it. It is
        // handed the violations ALONE — no request — so it cannot echo a long value the
        // caller submitted (the walk stores a shortened rendering of one);
        // `StatedPerOperation` answers a served operation's refusals before this walk is
        // reached, with the request in hand. A refusal that gets here anyway is still stated
        // by this service, through the SAME module, so the wording cannot differ between the
        // two paths — it just has only the violations to state from.
        //
        // An authentication refusal (HTTP 403) is not an input-constraint failure and keeps
        // the framework's wording.
        match refusal {
            Refusal::Invalid(violations) => {
                validation::refusal(violations, ::std::collections::HashMap::new)
            }
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}

#[cfg(test)]
mod tests {
    //! The one thing about the wiring above that a wire test cannot see: that the
    //! hand-maintained `wrap_operations!` list is the WHOLE surface this crate serves.

    use super::*;

    /// This service's Smithy model, as it ships beside this crate.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-kinesis-kinesis-20131202.json"
    );

    /// Every operation name the MODEL declares, read from the model this crate was generated
    /// from rather than from a hand-kept list \u2014 so a rule stated over "every operation of
    /// this service" is checked over the surface the model publishes.
    pub(super) fn modelled_operations() -> Vec<String> {
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes")
            .clone();
        let operations: Vec<String> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next().map(str::to_string))
            .collect();
        assert!(
            operations.len() >= 57,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );
        operations
    }

    /// RULE applied to the whole surface: every operation the generated routing table serves
    /// is also served behind [`StatedPerOperation`], under the same key that table is keyed
    /// by — except the one whose output is an event stream, which reads its own request and
    /// is routed by [`EmulatedService::override_operation`] instead.
    ///
    /// The names come from the MODEL rather than from a second hand list, so an operation
    /// that gains a handler and not an entry in `wrap_operations!` fails here instead of
    /// silently answering a refusal in the framework's wording — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`. An operation the model declares but no file in
    /// `src/ops/` serves is in NEITHER table, which is what the generated routing table
    /// does with it.
    #[test]
    fn every_operation_this_crate_serves_is_served_with_this_services_refusal_wording() {
        let operations = modelled_operations();

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        let mut served = 0usize;
        for operation in operations {
            let operation = operation.as_str();
            if service.entry.routing_table().get(operation).is_none() {
                continue;
            }
            served += 1;
            // The event-stream operation is served by the route `override_operation` hands
            // back, not out of the `stated` table: it reads its own request, and words a
            // refused one through `override_refusal` — the same module, the same sentence.
            if operation == SUBSCRIBE_TO_SHARD_OPERATION {
                continue;
            }
            assert!(
                service.stated.get(operation).is_some(),
                "case {operation}: every operation this crate serves is served behind \
                 StatedPerOperation; this one is missing from wrap_operations!, so it answers \
                 a refusal in the framework's wording instead of this service's"
            );
        }
        assert_eq!(
            served, 57,
            "case the served surface: expected the 57 operations this crate has a handler \
             for, got {served}"
        );
    }

    /// RULE applied to the whole QUEUE surface this crate serves: a queue's delivery is a PULL
    /// out of this service's own in-memory store, so no file of it dials a socket, reads the
    /// host's environment or files, or spawns a process.
    ///
    /// Checked over the SOURCE of the state this service holds rather than asserted in prose,
    /// because the rule is about what the code may name at all: a delivery path that grew a
    /// destination of its own would have to name one of these primitives, and this test fails
    /// the moment it does. The queue's data plane has no destination to dial — `CreateQueue`
    /// takes a queue name, one stream ARN with a starting position, attributes and tags, and no
    /// endpoint, URL or callback member — and what a receive answers comes from the records the
    /// stream already holds in the same store.
    #[test]
    fn the_queue_module_names_no_socket_no_host_read_and_no_process() {
        // The primitives an outbound dial, a host read or a process spawn would have to name.
        const PROHIBITED: &[&str] = &[
            "TcpStream",
            "UdpSocket",
            "TcpListener",
            "reqwest",
            "ureq",
            "to_socket_addrs",
            "std::fs",
            "File::open",
            "File::create",
            "env::var",
            "env::vars",
            "std::env",
            "Command",
            "std::process",
        ];
        // Every file of the queue state module and the module root that declares it, named here
        // so a file added to it without being listed is a failure rather than an unchecked gap.
        let files = [
            "src/state.rs",
            "src/state/queues.rs",
            "src/state/queues/delivery.rs",
            "src/state/queues/refusals.rs",
            "src/state/queues/store.rs",
            "src/state/queues/tests.rs",
        ];
        for file in files {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("case {file}: expected to read it, got {error}"));
            for prohibited in PROHIBITED {
                assert!(
                    !source.contains(prohibited),
                    "case {file}: a queue's delivery is a pull out of this service's own store, \
                     so this file must not name {prohibited}"
                );
            }
        }
    }
}
