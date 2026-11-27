// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-appstream-photonadminproxyservice` service: how it claims requests and how it words the
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
//! - `can_handle` — which requests this service claims. EDITED HERE: the default
//!   matches the request's resolved route key against this service's signing name,
//!   and this service also claims the streaming-session addresses a
//!   `CreateStreamingURL` points a client at ([`streaming_session`]), one of which
//!   is fetched by a browser that signs nothing and so resolves to no route key.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE for two reasons: this service claims the streaming-session
//!   addresses a `CreateStreamingURL` points a client at ([`streaming_session`]), which
//!   the model binds no operation to; and every MODELLED operation is served through
//!   [`StatedRefusal`] under the key the generated table itself resolves, so a request
//!   refused before its handler ran is worded the way this service words it
//!   ([`refusal_wording`]) — which needs the request the violations were found in and so
//!   cannot be done in `override_refusal`.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED HERE: the same renderer [`StatedRefusal`] uses
//!   ([`refusal_wording`]), so a refusal taken on a path that wrapper does not cover
//!   cannot be worded differently from one it does.
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

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationRoutingTable, Refusal, Refused, ServerHttpProtocol, ServiceIdentity, Service,
    WireProtocol,
};
use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

pub mod streaming_session;

mod list_length_clause;
mod refusal_wording;

use streaming_session::{Serves, StreamingSessionRoute};

/// The media type a request of this service's JSON protocol carries, which is how a
/// request NAMES that protocol when it carries no `X-Amz-Target`
/// ([`smithy_server_runtime::WireProtocolKind::detect`]).
///
/// This service declares two protocols (`awsJson1_1` and `rpcv2Cbor`), so a request that
/// names neither is refused before any route is asked to serve it. The streaming-session
/// addresses are fetched by a browser, which names no protocol at all, so this service
/// answers them in its JSON protocol — see [`EmulatedService::wire_protocol`].
const JSON_PROTOCOL_MEDIA_TYPE: &str = "application/x-amz-json-1.1";

/// The `com-amazonaws-appstream-photonadminproxyservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The route that resolves a streaming URL this service signed, and the route that
    /// serves the session address it redirects to. Held rather than built per request
    /// because [`Service::override_operation`] hands the router a BORROW of one of them,
    /// and separate because each is reached by a different kind of caller (see
    /// [`ErasedOperation::auth_schemes`] on them).
    issued_streaming_url: StreamingSessionRoute<Eng>,
    streaming_session_endpoint: StreamingSessionRoute<Eng>,
    /// Every modelled operation the generated table holds, as a [`StatedRefusal`] wrapper
    /// keyed by the routing key the protocol resolves — the key the generated table itself
    /// uses. The wrappers hold no engine and no operation instance: a request none of them
    /// refuses is served by the generated table's own instance, so there is no second copy
    /// of any operation and no second store.
    stated_refusals: OperationRoutingTable,
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table` as a
/// [`StatedRefusal`] wrapper, keyed by the operation's own modelled name — the routing key
/// `ServerHttpProtocol::routing_key` resolves and the generated table is keyed by, read off
/// the operation rather than typed again here.
macro_rules! wrap_operations {
    ($table:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(
                    StatedRefusal::<crate::ops::$module::$operation<Eng>>::new(),
                ) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            issued_streaming_url: StreamingSessionRoute::new(
                Arc::clone(&engine),
                Serves::IssuedUrl,
            ),
            streaming_session_endpoint: StreamingSessionRoute::new(
                Arc::clone(&engine),
                Serves::SessionEndpoint,
            ),
            entry: crate::ServiceDefinition::new(engine),
            stated_refusals: Self::stated_refusals(),
        }
    }

    /// The wrapper table: EVERY operation this model declares, because the wording rule is
    /// the service's and not one operation's — an operation whose input declares no
    /// constraint simply never has a violation to state, which the wrapper reads off that
    /// operation's own input schema. The inline test
    /// `every_modelled_operation_states_its_refusals` holds this list to the model's own
    /// surface.
    fn stated_refusals() -> OperationRoutingTable {
        let mut table = OperationRoutingTable::new();
        wrap_operations!(
            table,
            associate_app_block_builder_app_block::AssociateAppBlockBuilderAppBlock,
            associate_application_fleet::AssociateApplicationFleet,
            associate_application_to_entitlement::AssociateApplicationToEntitlement,
            associate_fleet::AssociateFleet,
            associate_software_to_image_builder::AssociateSoftwareToImageBuilder,
            batch_associate_user_stack::BatchAssociateUserStack,
            batch_disassociate_user_stack::BatchDisassociateUserStack,
            copy_image::CopyImage,
            create_app_block::CreateAppBlock,
            create_app_block_builder::CreateAppBlockBuilder,
            create_app_block_builder_streaming_url::CreateAppBlockBuilderStreamingURL,
            create_application::CreateApplication,
            create_directory_config::CreateDirectoryConfig,
            create_entitlement::CreateEntitlement,
            create_export_image_task::CreateExportImageTask,
            create_fleet::CreateFleet,
            create_image_builder::CreateImageBuilder,
            create_image_builder_streaming_url::CreateImageBuilderStreamingURL,
            create_imported_image::CreateImportedImage,
            create_stack::CreateStack,
            create_streaming_url::CreateStreamingURL,
            create_theme_for_stack::CreateThemeForStack,
            create_updated_image::CreateUpdatedImage,
            create_usage_report_subscription::CreateUsageReportSubscription,
            create_user::CreateUser,
            delete_app_block::DeleteAppBlock,
            delete_app_block_builder::DeleteAppBlockBuilder,
            delete_application::DeleteApplication,
            delete_directory_config::DeleteDirectoryConfig,
            delete_entitlement::DeleteEntitlement,
            delete_fleet::DeleteFleet,
            delete_image::DeleteImage,
            delete_image_builder::DeleteImageBuilder,
            delete_image_permissions::DeleteImagePermissions,
            delete_stack::DeleteStack,
            delete_theme_for_stack::DeleteThemeForStack,
            delete_usage_report_subscription::DeleteUsageReportSubscription,
            delete_user::DeleteUser,
            describe_app_block_builder_app_block_associations::DescribeAppBlockBuilderAppBlockAssociations,
            describe_app_block_builders::DescribeAppBlockBuilders,
            describe_app_blocks::DescribeAppBlocks,
            describe_app_license_usage::DescribeAppLicenseUsage,
            describe_application_fleet_associations::DescribeApplicationFleetAssociations,
            describe_applications::DescribeApplications,
            describe_directory_configs::DescribeDirectoryConfigs,
            describe_entitlements::DescribeEntitlements,
            describe_fleets::DescribeFleets,
            describe_image_builders::DescribeImageBuilders,
            describe_image_permissions::DescribeImagePermissions,
            describe_images::DescribeImages,
            describe_sessions::DescribeSessions,
            describe_software_associations::DescribeSoftwareAssociations,
            describe_stacks::DescribeStacks,
            describe_theme_for_stack::DescribeThemeForStack,
            describe_usage_report_subscriptions::DescribeUsageReportSubscriptions,
            describe_user_stack_associations::DescribeUserStackAssociations,
            describe_users::DescribeUsers,
            disable_user::DisableUser,
            disassociate_app_block_builder_app_block::DisassociateAppBlockBuilderAppBlock,
            disassociate_application_fleet::DisassociateApplicationFleet,
            disassociate_application_from_entitlement::DisassociateApplicationFromEntitlement,
            disassociate_fleet::DisassociateFleet,
            disassociate_software_from_image_builder::DisassociateSoftwareFromImageBuilder,
            drain_session_instance::DrainSessionInstance,
            enable_user::EnableUser,
            expire_session::ExpireSession,
            get_export_image_task::GetExportImageTask,
            list_associated_fleets::ListAssociatedFleets,
            list_associated_stacks::ListAssociatedStacks,
            list_entitled_applications::ListEntitledApplications,
            list_export_image_tasks::ListExportImageTasks,
            list_tags_for_resource::ListTagsForResource,
            start_app_block_builder::StartAppBlockBuilder,
            start_fleet::StartFleet,
            start_image_builder::StartImageBuilder,
            start_software_deployment_to_image_builder::StartSoftwareDeploymentToImageBuilder,
            stop_app_block_builder::StopAppBlockBuilder,
            stop_fleet::StopFleet,
            stop_image_builder::StopImageBuilder,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_app_block_builder::UpdateAppBlockBuilder,
            update_application::UpdateApplication,
            update_directory_config::UpdateDirectoryConfig,
            update_entitlement::UpdateEntitlement,
            update_fleet::UpdateFleet,
            update_image_builder::UpdateImageBuilder,
            update_image_permissions::UpdateImagePermissions,
            update_stack::UpdateStack,
            update_theme_for_stack::UpdateThemeForStack,
        );
        table
    }

    /// The streaming-session route that serves `req`, if either does.
    ///
    /// The one place that decision is made, so `can_handle` claims exactly the requests
    /// `override_operation` then serves.
    fn streaming_session_route(&self, method: &str, uri: &str) -> Option<&dyn ErasedOperation> {
        [
            &self.issued_streaming_url,
            &self.streaming_session_endpoint,
        ]
        .into_iter()
        .find(|route| route.claims(method, uri))
        .map(|route| route as &dyn ErasedOperation)
    }
}

/// One modelled operation, served so that a request refused before its handler ran is
/// worded the way this service words it ([`refusal_wording`]).
///
/// [`Service::override_refusal`] cannot do it alone: that seam is handed the violations and
/// never the request they came from, while two halves of this service's wording are the
/// REQUEST's — the list a clause stated at a collection member quotes, and the input shape
/// the element sets are read off. Both are collected here, per operation, where the request
/// can be re-read.
///
/// Zero-sized: the wrapper holds no engine and no operation instance. It carries the
/// operation TYPE, which is all it needs — the input shape to read the request into, and
/// that shape's `Schema` to read it against — and hands a request it does not refuse to the
/// one instance the generated routing table built from the shared engine, so there is no
/// second instance of any operation and no second store.
///
/// Nothing else is re-implemented and no check is repeated with different words: the
/// violations come from the framework's own machinery (the protocol's deserializer and
/// `constraints::validate` over the built input, through
/// `service_common::validation::refusing_violations_with_lists`), the clauses and the
/// envelope from [`refusal_wording`] — the same renderer [`Service::override_refusal`] uses,
/// so the two cannot drift — and the status and the wire envelope from the framework's
/// refusal channel. The cost is DELIBERATE: a request this wrapper does not refuse is read
/// twice, once here and once in the operation's own sequence, because the framework hands no
/// already-built input to an operation.
struct StatedRefusal<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> StatedRefusal<Op> {
    /// The wrapper for one operation type. Const because it holds nothing.
    const fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for StatedRefusal<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let started = ::std::time::Instant::now();
            // The definition is read, used and dropped inside this one block:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            let refused = {
                let definition = svc.definition();
                let (violations, lists) =
                    ::service_common::validation::refusing_violations_with_lists::<Op>(
                        definition.protocol(),
                        req,
                    );
                // A violation set this renderer has no wording for — and a failure that is
                // not a refusal at all, which hands over no violations — leaves the request
                // to the layers behind this wrapper: the fail-safe direction.
                refusal_wording::refused(
                    &Refusal::Invalid(violations.clone()),
                    &[Op::INPUT_SCHEMA],
                    &lists,
                )
                .map(|stated| {
                    let code = stated.code.clone();
                    (
                        definition.refusal_response(
                            ProtocolError::Invalid(violations),
                            Some(stated),
                            req,
                        ),
                        code,
                    )
                })
            };
            if let Some((response, code)) = refused {
                // The row the framework's own sequence would have recorded for this request,
                // classified from the response that was actually produced, so the row cannot
                // drift from the answer it describes.
                record_refusal::<Op>(ctx, started.elapsed(), response.status().as_u16(), &code);
                return response;
            }
            // Nothing to state: the request is served by the generated operation, which runs
            // the whole framework sequence (deserialize, validate, handle, serialize) plus
            // its telemetry over the same bytes. Reached through the routing table rather
            // than through `override_operation`, which would route straight back here. Bound
            // in its own statement so nothing borrowed from the definition is held across
            // the await.
            let served = svc.definition().routing_table().get(Op::NAME);
            match served {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key the
                // generated table registered. Answer the framework's own no-such-operation
                // response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            }
        })
    }

    /// `None` — the framework reads the schemes off the MODELLED operation the request's
    /// routing key names, which is what the model bakes them on, so wrapping an operation
    /// cannot change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        None
    }
}

/// Record the refusal [`StatedRefusal`] answered as the operation row the framework's own
/// sequence would have recorded: the same classification (`status` and `code` read off the
/// response the framework's refusal channel produced, so neither is restated here), the same
/// folded handle (this service's signing name and the operation's own name), and the same
/// already-sanitized request metadata. Gated on a client collecting, exactly as the framework
/// gates it.
fn record_refusal<Op: Operation>(
    ctx: &OperationContext<'_>,
    latency: ::std::time::Duration,
    status: u16,
    code: &str,
) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(status, code)),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry, which
        // the definition below forwards every one of them to. It is implemented HERE
        // rather than handed back bare for ONE reason: the protocol a streaming-session
        // address is served in, which no request to those addresses can name (see
        // [`EmulatedService::wire_protocol`]).
        self
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // A streaming session's two addresses are claimed by their path: the session
        // address a resolved streaming URL redirects to is fetched by a browser, which
        // signs nothing and names no route key, so nothing else would claim it.
        if self
            .streaming_session_route(parts.method.as_str(), &parts.uri.to_string())
            .is_some()
        {
            return true;
        }
        // Otherwise claim a request whose resolved route key is this service's signing
        // name.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The model binds no operation to the streaming-session addresses — they are what
        // a `CreateStreamingURL` points a client at rather than API calls — so they are
        // served here.
        if let Some(route) = self.streaming_session_route(req.method(), req.uri()) {
            return Some(route);
        }
        // Every other request is NOT re-routed: the key is the one the generated table's own
        // protocol resolves, and the operation it names is the one served — wrapped in
        // [`StatedRefusal`] so a refused request is worded the way this service words it. A
        // key the model does not name resolves to `None` here exactly as it does in the
        // generated table, so the router's routing-error path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated_refusals.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it. The SAME renderer [`StatedRefusal`] uses ([`refusal_wording`]), so a
        // refusal taken on a path that wrapper does not cover is worded the one way this
        // service words one. What this seam cannot hand over is the REQUEST, so the two
        // halves that are read from it — the list a clause stated at a collection member
        // quotes, and the input shape an element set is read off — are empty here, and such
        // a clause keeps the framework's own.
        refusal_wording::refused(refusal, &[], &HashMap::new())
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

/// This service's model-derived half, forwarded to the generated entry — except for the
/// ONE thing the entry cannot decide: the protocol a streaming-session address is served
/// in.
///
/// This service declares `awsJson1_1` AND `rpcv2Cbor`, so the framework picks the protocol
/// of a request from the marker the request carries, and refuses one that names neither
/// before any route is asked to serve it. The streaming-session addresses
/// ([`streaming_session`]) carry no marker at all — a signed URL is opened by a browser,
/// which sends no `X-Amz-Target` and no AWS media type — so this definition names the JSON
/// protocol for them, the same one every operation of this service is served in. Nothing
/// else about the request changes: it still goes through the router's own authentication
/// and through [`Service::override_operation`], which is what serves it.
impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for EmulatedService<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.entry.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.entry.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        self.entry.refusal_response(error, chosen, req)
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        if self
            .streaming_session_route(req.method(), req.uri())
            .is_none()
        {
            return self.entry.wire_protocol(req);
        }
        // The entry decides from the request, so it is asked about a request that names
        // this service's JSON protocol: the answer is the protocol the entry declares for
        // that marker, borrowed from the entry rather than from this probe.
        let mut probe = RtRequest::new(SdkBody::empty());
        if probe.set_method(req.method()).is_err() || probe.set_uri(req.uri()).is_err() {
            return self.entry.wire_protocol(req);
        }
        probe
            .headers_mut()
            .insert("content-type", JSON_PROTOCOL_MEDIA_TYPE);
        self.entry.wire_protocol(&probe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// This service's Smithy model, as it ships beside this crate. Read at test time rather
    /// than embedded: it is megabytes of JSON that no non-test build needs.
    const MODEL_PATH: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/smithy-model/com-amazonaws-appstream-photonadminproxyservice.json"
    );

    /// RULE applied to the whole surface: every operation the model declares is served
    /// through [`StatedRefusal`], under the same key the generated routing table is keyed
    /// by — so a refusal is worded this service's way wherever a constraint is broken, not
    /// only on the operations a recording covered.
    ///
    /// The names come from the MODEL rather than from a second hand-written list, so an
    /// operation added to the model and not to `wrap_operations!` fails here instead of
    /// silently answering a refusal in the framework's wording — and a key spelled
    /// differently from the generated table's fails here instead of silently never being
    /// reached by `override_operation`.
    #[test]
    fn every_modelled_operation_states_its_refusals() {
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
            !operations.is_empty(),
            "case \"the model's surface\": the model declares operations"
        );

        let service = EmulatedService::new(Arc::new(engine::InMemoryEngine::new()));
        for operation in operations {
            assert!(
                service.entry.routing_table().get(operation).is_some(),
                "case {operation}: the generated table keys an operation by its shape name; \
                 this one resolved to nothing, so the wrapper's key spelling is wrong"
            );
            assert!(
                service.stated_refusals.get(operation).is_some(),
                "case {operation}: every modelled operation is served through StatedRefusal; \
                 this one is missing from wrap_operations!, so a constraint refusal of it is \
                 worded by the framework instead of by this service"
            );
        }
    }
}
