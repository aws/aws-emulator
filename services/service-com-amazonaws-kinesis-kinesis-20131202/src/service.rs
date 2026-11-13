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
//! ## This service's other edited seam: the operation whose output is an event stream
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
//! sentence.

mod submitted_batch;
mod validation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::EventStreamRoute;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, ServerHttpProtocol, Service, Violation,
};
use std::sync::Arc;

/// The `com-amazonaws-kinesis-kinesis-20131202` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same operations the generated table holds, each wrapped in [`StatedPerOperation`]
    /// so a refused request is worded the way this service words it. Keyed by the routing
    /// key the protocol resolves — the key the generated table uses — and built from the SAME
    /// shared engine, so a wrapped operation reads and writes the one store.
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
/// [`StatedPerOperation`] and keyed by its operation shape name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by.
macro_rules! wrap_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                stringify!($operation),
                ::std::boxed::Box::new(StatedPerOperation::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
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
            add_tags_to_stream::AddTagsToStream,
            create_channel::CreateChannel,
            create_stream::CreateStream,
            decrease_stream_retention_period::DecreaseStreamRetentionPeriod,
            delete_channel::DeleteChannel,
            delete_resource_policy::DeleteResourcePolicy,
            delete_stream::DeleteStream,
            deregister_stream_consumer::DeregisterStreamConsumer,
            describe_account_settings::DescribeAccountSettings,
            describe_channel::DescribeChannel,
            describe_limits::DescribeLimits,
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
            list_shards::ListShards,
            list_stream_consumers::ListStreamConsumers,
            list_streams::ListStreams,
            list_tags_for_resource::ListTagsForResource,
            list_tags_for_stream::ListTagsForStream,
            merge_shards::MergeShards,
            put_record::PutRecord,
            put_records::PutRecords,
            put_resource_policy::PutResourcePolicy,
            register_stream_consumer::RegisterStreamConsumer,
            remove_tags_from_stream::RemoveTagsFromStream,
            split_shard::SplitShard,
            start_stream_encryption::StartStreamEncryption,
            stop_stream_encryption::StopStreamEncryption,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_account_settings::UpdateAccountSettings,
            update_channel::UpdateChannel,
            update_max_record_size::UpdateMaxRecordSize,
            update_shard_count::UpdateShardCount,
            update_stream_mode::UpdateStreamMode,
            update_stream_record_distribution_strategy::UpdateStreamRecordDistributionStrategy,
            update_stream_warm_throughput::UpdateStreamWarmThroughput,
        );
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
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
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
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
    let stated = validation::refusal(&violations)?;
    Some((violations, stated))
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
            // The definition is read, used and dropped inside each statement:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = stated_refusal::<O>(svc.definition().protocol(), req);
            if let Some((violations, stated)) = refused {
                return svc
                    .definition()
                    .refusal_response(ProtocolError::Invalid(violations), Some(stated), req);
            }
            self.operation.run(svc, req, ctx).await
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
        &self.entry
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
            Refusal::Invalid(violations) => validation::refusal(violations),
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
        let model = std::fs::read_to_string(MODEL_PATH)
            .expect("the model that generated this crate ships beside it");
        let model: serde_json::Value =
            serde_json::from_str(&model).expect("the model that generated this crate is JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares shapes");
        let operations: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| shape["type"] == "operation")
            .filter_map(|(id, _)| id.rsplit('#').next())
            .collect();
        assert!(
            operations.len() >= 45,
            "case the model's surface: expected this service's operations, got {operations:?}"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        let mut served = 0usize;
        for operation in operations {
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
            served, 45,
            "case the served surface: expected the 45 operations this crate has a handler \
             for, got {served}"
        );
    }
}
