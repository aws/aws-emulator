// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-imagebuilder-imagebuilder` service: how it claims requests and how it words the
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
//!   correctly. This service serves the operation the model's own routing key names,
//!   wrapped (see "This service's front door" below) — and, for a request the key names
//!   nothing for, the operation whose modelled route it addressed
//!   ([`required_query_route`]).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. Nothing is reworded there: a refusal this service words
//!   itself is worded at its front door, which is the only layer that can place one.
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
//! ## This service's front door: a parameter whose value the model does not admit
//!
//! Image Builder refuses such a value as the one class its model documents for it,
//! `InvalidParameterValueException` (HTTP 400), naming the parameter the request carried
//! and the requirement its value had to satisfy. [`parameter_refusal`] owns that sentence;
//! what this file owns is WHERE it is stated, which cannot be [`Service::override_refusal`]:
//! the centralized walk hands that seam the violations ALONE — no operation, no request — and
//! this service quotes the expression an ARN member's value had to match in the form of the
//! layer that READ the value, while this model binds one member name both ways
//! (`imagePipelineArn` is `GetImagePipeline`'s `@httpQuery` parameter and
//! `StartImagePipelineExecution`'s body member). So two refusals that are indistinguishable
//! from the violations alone are worded differently on the wire.
//!
//! [`service_common::front_door::FrontDoor`] is where that distinction lives: every modelled
//! operation is served wrapped in it, and the wrapper reads the HTTP BINDING of each violated
//! member off THAT operation's own input schema, then words the body half and the
//! request-line half through the two wordings attached to the door.
//!
//! ## A request that leaves out a member the operation requires outside its body
//!
//! Such a request addresses the operation its method and path name and is refused as the bad
//! request it is, never reported as naming no operation: [`required_query_route`] owns that
//! rule, and this file only asks it at [`Service::override_operation`], where the request can
//! still be handed to the operation it addressed.
//!
//! ## Naming the request a response answers
//!
//! Every response Image Builder sends names the request it answered: a success through the
//! `requestId` member its output shape models, and a FAILURE through the same member in its
//! error body, which no error shape of this model declares — so no handler can set it, and
//! the framework's `restJson1` error renderers write the `__type` and the `message` alone.
//!
//! The rule is not restated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report this service's envelope takes (the
//! body member) and wires it at the two layers every response of this service passes through:
//!
//! * [`RequestIdReportingOperation::error_member`], wrapped around each operation's own front
//!   door, covers everything an operation answers — a success (untouched), a modelled error, a
//!   refusal its door worded, and a constraint refusal raised while it was being served;
//! * [`RequestIdEnvelopeDefinition::error_member`], the definition wrapper `definition` hands
//!   back, covers a response NO operation produced: a routing failure, a transport failure
//!   detected up front, and a pre-dispatch authentication refusal.
//!
//! ## And how an answer that carries NO CONTENT is framed
//!
//! The two tag mutations are the only operations of this model whose output structure declares
//! no member at all, so their success leaves the caller nothing to parse and answers
//! `204 No Content` (`SUCCESS_STATUS` in `src/ops/tag_resource.rs` and
//! `src/ops/untag_resource.rs`). Such a structure is not `smithy.api#Unit`, the one shape the
//! renderers treat as bodyless, so the protocol renders it as the empty JSON object — content
//! on an answer that states it has none. [`no_content_framing::NoContentRestJson`] is
//! therefore what this service serves at [`Service::override_protocol`]: the model's own
//! `restJson1` with a response at that status handed through the tree's one reading of that
//! framing, at the one place every response of this service passes through rather than per
//! operation.

pub mod no_content_framing;
pub mod parameter_refusal;
pub mod required_query_route;

use no_content_framing::NoContentRestJson;
use parameter_refusal::{
    body_parameter_refusal, pipeline_listing_body_refusal, unstated_pattern_refusal,
    uri_parameter_refusal,
};
use required_query_route::ModelledRoutes;
use service_common::front_door::{BodyWording, FrontDoor, UriWording};
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdReportingOperation,
};
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Operation, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// Serve `operation` behind this service's front door, under its own modelled name — the
/// routing key the protocol resolves and the generated table is keyed by, read off the
/// operation rather than spelled again here.
///
/// `uri_wording` is how THIS operation words a refusal of a value its request carries outside
/// the body, and `body_wording` how it words one the request carried IN the body. The
/// request-id report wraps the door, so every response the operation answers — through the
/// door or behind it — names the request it answered.
///
/// The operation's own modelled route is recorded in `routes` as well, so a request that
/// addresses it and leaves out a member it requires outside its body reaches it rather than
/// the unknown-operation tail ([`required_query_route`]).
fn serve_worded<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    routes: &mut ModelledRoutes,
    operation: O,
    body_wording: BodyWording,
    uri_wording: UriWording,
) {
    routes.serving::<O>();
    table.add_operation(
        O::NAME,
        Box::new(RequestIdReportingOperation::error_member(
            FrontDoor::worded(operation, body_wording).wording_uri(uri_wording),
        )) as Box<dyn ErasedOperation>,
    );
}

/// [`serve_worded`] with the body wording every operation of this service shares.
fn serve<O: Operation + ErasedOperation + 'static>(
    table: &mut OperationRoutingTable,
    routes: &mut ModelledRoutes,
    operation: O,
    uri_wording: UriWording,
) {
    serve_worded(table, routes, operation, body_parameter_refusal, uri_wording);
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each served behind
/// this service's front door ([`serve`]).
///
/// A line carrying `=> <wording>` words a refusal of a value that operation carries outside
/// its body in that wording instead of [`uri_parameter_refusal`], which is what every other
/// operation states.
macro_rules! serve_operations {
    // The wording a line states, or the one every other operation states.
    (@wording) => { uri_parameter_refusal };
    (@wording $wording:path) => { $wording };
    ($table:expr, $routes:expr, $engine:expr, $($module:ident :: $operation:ident $(=> $wording:path)?),+ $(,)?) => {
        $(
            serve(
                &mut $table,
                &mut $routes,
                crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                serve_operations!(@wording $($wording)?),
            );
        )+
    };
}

/// The `com-amazonaws-imagebuilder-imagebuilder` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// The same modelled operations the generated table holds, each served behind this
    /// service's front door so a parameter whose value the model does not admit is refused
    /// as that parameter's value. Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store. Held on the service because
    /// [`Service::override_operation`] hands back a borrow of `self`.
    front_door: OperationRoutingTable,
    /// The `@http` route of each of those operations, so a request that addresses one of them
    /// and leaves out a member it requires outside its body is served by the operation it
    /// addressed rather than reported as naming no operation at all
    /// ([`required_query_route`]).
    routes: ModelledRoutes,
    /// This service's wire protocol, built ONCE here because
    /// [`Service::override_protocol`] hands back a borrow of it.
    protocol: NoContentRestJson,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door = OperationRoutingTable::new();
        let mut routes = ModelledRoutes::new();
        // EVERY modelled operation, so the rule is applied by one code path rather than per
        // operation: an operation that binds nothing to the body never has a body violation
        // to find, and one that binds nothing to its request line never has a request-line
        // violation, which the wrapper reads off each operation's own input schema.
        serve_operations!(
            front_door,
            routes,
            engine,
            cancel_image_creation::CancelImageCreation,
            cancel_lifecycle_execution::CancelLifecycleExecution,
            create_component::CreateComponent,
            create_container_recipe::CreateContainerRecipe,
            create_distribution_configuration::CreateDistributionConfiguration,
            create_image::CreateImage,
            create_image_pipeline::CreateImagePipeline,
            create_image_recipe::CreateImageRecipe,
            create_infrastructure_configuration::CreateInfrastructureConfiguration,
            create_lifecycle_policy::CreateLifecyclePolicy,
            create_workflow::CreateWorkflow,
            delete_component::DeleteComponent,
            delete_container_recipe::DeleteContainerRecipe,
            delete_distribution_configuration::DeleteDistributionConfiguration,
            delete_image::DeleteImage,
            delete_image_pipeline::DeleteImagePipeline,
            delete_image_recipe::DeleteImageRecipe,
            delete_infrastructure_configuration::DeleteInfrastructureConfiguration,
            delete_lifecycle_policy::DeleteLifecyclePolicy,
            delete_workflow::DeleteWorkflow,
            distribute_image::DistributeImage,
            get_component::GetComponent,
            get_component_policy::GetComponentPolicy,
            get_container_recipe::GetContainerRecipe,
            get_container_recipe_policy::GetContainerRecipePolicy,
            // The one operation whose own check echoes no expression at all.
            get_distribution_configuration::GetDistributionConfiguration
                => unstated_pattern_refusal,
            get_image::GetImage,
            get_image_pipeline::GetImagePipeline,
            get_image_policy::GetImagePolicy,
            get_image_recipe::GetImageRecipe,
            get_image_recipe_policy::GetImageRecipePolicy,
            get_infrastructure_configuration::GetInfrastructureConfiguration,
            get_lifecycle_execution::GetLifecycleExecution,
            get_lifecycle_policy::GetLifecyclePolicy,
            get_marketplace_resource::GetMarketplaceResource,
            get_workflow::GetWorkflow,
            get_workflow_execution::GetWorkflowExecution,
            get_workflow_step_execution::GetWorkflowStepExecution,
            import_component::ImportComponent,
            import_disk_image::ImportDiskImage,
            import_vm_image::ImportVmImage,
            list_component_build_versions::ListComponentBuildVersions,
            list_components::ListComponents,
            list_container_recipes::ListContainerRecipes,
            list_distribution_configurations::ListDistributionConfigurations,
            list_image_build_versions::ListImageBuildVersions,
            list_image_packages::ListImagePackages,
            list_image_pipeline_images::ListImagePipelineImages,
            list_image_recipes::ListImageRecipes,
            list_image_scan_finding_aggregations::ListImageScanFindingAggregations,
            list_image_scan_findings::ListImageScanFindings,
            list_images::ListImages,
            list_infrastructure_configurations::ListInfrastructureConfigurations,
            list_lifecycle_execution_resources::ListLifecycleExecutionResources,
            list_lifecycle_executions::ListLifecycleExecutions,
            list_lifecycle_policies::ListLifecyclePolicies,
            list_tags_for_resource::ListTagsForResource,
            list_waiting_workflow_steps::ListWaitingWorkflowSteps,
            list_workflow_build_versions::ListWorkflowBuildVersions,
            list_workflow_executions::ListWorkflowExecutions,
            list_workflow_step_executions::ListWorkflowStepExecutions,
            list_workflows::ListWorkflows,
            put_component_policy::PutComponentPolicy,
            put_container_recipe_policy::PutContainerRecipePolicy,
            put_image_policy::PutImagePolicy,
            put_image_recipe_policy::PutImageRecipePolicy,
            retry_image::RetryImage,
            send_workflow_step_action::SendWorkflowStepAction,
            start_image_pipeline_execution::StartImagePipelineExecution,
            start_resource_state_update::StartResourceStateUpdate,
            tag_resource::TagResource,
            untag_resource::UntagResource,
            update_distribution_configuration::UpdateDistributionConfiguration,
            update_image_pipeline::UpdateImagePipeline,
            update_infrastructure_configuration::UpdateInfrastructureConfiguration,
            update_lifecycle_policy::UpdateLifecyclePolicy,
        );
        // The one operation that states a COLLECTION's `@length` requirement its own way: the
        // pipeline listing's recorded refusals of a `filters` outside its bounds name the
        // modelled bound the member's length had to satisfy, where every other refusal of this
        // service states how many items the member has to carry. Which wording is stated is a
        // property of the operation the request addressed, so it is chosen here, where each
        // operation's own door is built.
        serve_worded(
            &mut front_door,
            &mut routes,
            crate::ops::list_image_pipelines::ListImagePipelines::new(Arc::clone(&engine)),
            pipeline_listing_body_refusal,
            uri_parameter_refusal,
        );
        Self {
            entry: RequestIdEnvelopeDefinition::error_member(crate::ServiceDefinition::new(engine)),
            front_door,
            routes,
            protocol: NoContentRestJson::new(),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what is
        // handed back is the shared `RequestIdEnvelopeDefinition` over that entry, which
        // reads identity, protocol and the routing table straight off it and names the
        // request in the error body of the three framework-level renderers.
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
        // and the operation it names is the one served — behind this service's front door,
        // so a parameter whose value the model does not admit is refused as that
        // parameter's value, and under the request-id report, so the response names the
        // request it answers. A key the model does not name resolves to `None` here exactly
        // as it does in the generated table, so the router's routing-error path (which names
        // the request itself, through the definition wrapper) is unchanged. The door
        // declares the wrapped operation's own auth schemes, so wrapping cannot change which
        // schemes sign it.
        //
        // A request the protocol resolves NO key for is handed to the operation whose
        // modelled route it addressed, when exactly one operation binds that method and path
        // ([`required_query_route`]): the framework withholds such a route when a member the
        // operation requires outside its body is absent, and that is a bad request for the
        // operation to refuse rather than a report that nobody has written it.
        let Ok(key) = self.entry.protocol().routing_key(req) else {
            return self.routes.addressed(req, &self.front_door);
        };
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // Nothing is reworded HERE, because nothing this service words differently can be
        // placed from the violations alone: a value its model does not admit is refused by
        // the front door each operation is served behind, which is the only layer that knows
        // which operation was addressed and where in the request the value travelled (see the
        // module docs). What reaches this seam is a refusal that door does not report — a body
        // no codec can read — and the framework's own wording, which states what it could not
        // read, is the right answer for it.
        let _ = refusal;
        None
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `restJson1`, with one framing correction on the way out: an answer
        // that carries no content — a tag mutation's success — carries no body rather than the
        // empty JSON object the renderer writes for a members-less output structure (see the
        // module docs). Every answer that does carry content is left exactly as rendered.
        Some(&self.protocol)
    }
}
