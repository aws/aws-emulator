// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-wellarchitected-wellarchitectedapiservicelambda` service: how it claims requests and how it words the
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
//! ## This service's refusal envelope
//!
//! Every operation here declares `ValidationException` (`@error("client")`,
//! `@httpError(400)`) with a **required** `Message` plus the optional `Reason`
//! (`ValidationExceptionReason`) and `Fields` (`ValidationExceptionField`) a client
//! branches on. The framework's own pre-dispatch envelope cannot carry those members —
//! `Refused` is a code and a sentence — so a request refused by the boundary walk
//! (`deserialize` -> constraint `validate`, both BEFORE any handler runs) would answer
//! in a shape that is not a `ValidationException` for this service at all.
//!
//! [`ModelledValidation`] closes that gap: `override_operation` routes every modelled
//! operation through it, it reads the request with that operation's own input shape and
//! schema, and a refusal is serialized as the MODELLED error. A valid request is
//! delegated, untouched, to the generated operation in the routing table — so the
//! handler, its state and its 501 are exactly as before.
//!
//! ## How this service names the error shape of a failure
//!
//! `x-amzn-errortype`, the header a restJson1 client resolves an error shape from, belongs
//! to the RESPONSE rather than to any modelled shape, so no handler can set it, and the
//! framework's renderers write the bare shape name where this service's wire carries the
//! QUALIFIED form ([`error_type_locator`]). It is therefore completed at the two layers
//! every response of this service passes through: [`ModelledValidation`], which every
//! modelled operation is served through, and the [`error_type_locator::ErrorTypeLocatorDefinition`]
//! wrapper around the generated entry, which covers a response no operation produced.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::Arc;

use aws_smithy_schema::{Schema, ShapeType};
use aws_smithy_schema_server::constraints::{self, RangeTrait, Violation, ViolationKind};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    OperationContext, Refusal, Refused, ServerHttpProtocol, Service,
};

/// How this service words a request that NAMES lens content it cannot resolve — the
/// other half of the refusal envelope described above, raised by the HANDLERS rather
/// than by the boundary walk, and shared by every operation that resolves such a value.
pub mod lens_content_validation;

/// How this service names, in `x-amzn-errortype`, the error shape a failure is refused
/// with — the one form every response of this service carries, success or failure.
pub mod error_type_locator;

use error_type_locator::{with_error_type_locator, ErrorTypeLocatorDefinition};

/// The `com-amazonaws-wellarchitected-wellarchitectedapiservicelambda` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its framework-level error renderers wrapped so a response
    /// no operation produced names its error shape the way this service names it.
    entry: ErrorTypeLocatorDefinition<crate::ServiceDefinition<Eng>>,
    /// One [`ModelledValidation`] per modelled operation, keyed by the routing key the
    /// model names that operation by ([`Operation::NAME`], the same key the generated
    /// routing table uses). Built once here, never per request; each entry is
    /// zero-sized, so this is a table of names, not a second set of operations.
    refusals: HashMap<&'static str, Box<dyn ErasedOperation>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: ErrorTypeLocatorDefinition::new(crate::ServiceDefinition::new(engine)),
            refusals: modelled_validation_table::<Eng>(),
        }
    }
}

/// The modelled-refusal wrapper for EVERY operation this service serves.
///
/// Every operation of this service declares the same `ValidationException`, so every
/// one of them states a refused request the same way — there is no operation whose
/// boundary refusal may answer in the framework's envelope instead. An operation that
/// is still scaffolded is wrapped too: it answers 501 for a request it could serve, and
/// the modelled 400 for one no operation could, which is the order the real service
/// evaluates them in.
fn modelled_validation_table<Eng: Send + Sync + 'static>(
) -> HashMap<&'static str, Box<dyn ErasedOperation>> {
    let mut refusals: HashMap<&'static str, Box<dyn ErasedOperation>> = HashMap::new();
    register::<crate::ops::associate_lenses::AssociateLenses<Eng>>(&mut refusals);
    register::<crate::ops::associate_profiles::AssociateProfiles<Eng>>(&mut refusals);
    register::<crate::ops::create_agent_context::CreateAgentContext<Eng>>(&mut refusals);
    register::<crate::ops::create_agent_goal::CreateAgentGoal<Eng>>(&mut refusals);
    register::<crate::ops::create_agent_profile::CreateAgentProfile<Eng>>(&mut refusals);
    register::<crate::ops::create_lens_share::CreateLensShare<Eng>>(&mut refusals);
    register::<crate::ops::create_lens_version::CreateLensVersion<Eng>>(&mut refusals);
    register::<crate::ops::create_milestone::CreateMilestone<Eng>>(&mut refusals);
    register::<crate::ops::create_profile::CreateProfile<Eng>>(&mut refusals);
    register::<crate::ops::create_profile_share::CreateProfileShare<Eng>>(&mut refusals);
    register::<crate::ops::create_review_template::CreateReviewTemplate<Eng>>(&mut refusals);
    register::<crate::ops::create_template_share::CreateTemplateShare<Eng>>(&mut refusals);
    register::<crate::ops::create_workload::CreateWorkload<Eng>>(&mut refusals);
    register::<crate::ops::create_workload_share::CreateWorkloadShare<Eng>>(&mut refusals);
    register::<crate::ops::delete_agent_context::DeleteAgentContext<Eng>>(&mut refusals);
    register::<crate::ops::delete_agent_goal::DeleteAgentGoal<Eng>>(&mut refusals);
    register::<crate::ops::delete_agent_profile::DeleteAgentProfile<Eng>>(&mut refusals);
    register::<crate::ops::delete_lens::DeleteLens<Eng>>(&mut refusals);
    register::<crate::ops::delete_lens_share::DeleteLensShare<Eng>>(&mut refusals);
    register::<crate::ops::delete_profile::DeleteProfile<Eng>>(&mut refusals);
    register::<crate::ops::delete_profile_share::DeleteProfileShare<Eng>>(&mut refusals);
    register::<crate::ops::delete_review_template::DeleteReviewTemplate<Eng>>(&mut refusals);
    register::<crate::ops::delete_template_share::DeleteTemplateShare<Eng>>(&mut refusals);
    register::<crate::ops::delete_workload::DeleteWorkload<Eng>>(&mut refusals);
    register::<crate::ops::delete_workload_share::DeleteWorkloadShare<Eng>>(&mut refusals);
    register::<crate::ops::disassociate_lenses::DisassociateLenses<Eng>>(&mut refusals);
    register::<crate::ops::disassociate_profiles::DisassociateProfiles<Eng>>(&mut refusals);
    register::<crate::ops::export_lens::ExportLens<Eng>>(&mut refusals);
    register::<crate::ops::get_agent_context::GetAgentContext<Eng>>(&mut refusals);
    register::<crate::ops::get_agent_goal::GetAgentGoal<Eng>>(&mut refusals);
    register::<crate::ops::get_agent_profile::GetAgentProfile<Eng>>(&mut refusals);
    register::<crate::ops::get_agent_recommendation::GetAgentRecommendation<Eng>>(&mut refusals);
    register::<crate::ops::get_agent_recommendation_generation::GetAgentRecommendationGeneration<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::get_answer::GetAnswer<Eng>>(&mut refusals);
    register::<crate::ops::get_consolidated_report::GetConsolidatedReport<Eng>>(&mut refusals);
    register::<crate::ops::get_global_settings::GetGlobalSettings<Eng>>(&mut refusals);
    register::<crate::ops::get_lens::GetLens<Eng>>(&mut refusals);
    register::<crate::ops::get_lens_review::GetLensReview<Eng>>(&mut refusals);
    register::<crate::ops::get_lens_review_report::GetLensReviewReport<Eng>>(&mut refusals);
    register::<crate::ops::get_lens_version_difference::GetLensVersionDifference<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::get_milestone::GetMilestone<Eng>>(&mut refusals);
    register::<crate::ops::get_profile::GetProfile<Eng>>(&mut refusals);
    register::<crate::ops::get_profile_template::GetProfileTemplate<Eng>>(&mut refusals);
    register::<crate::ops::get_review_template::GetReviewTemplate<Eng>>(&mut refusals);
    register::<crate::ops::get_review_template_answer::GetReviewTemplateAnswer<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::get_review_template_lens_review::GetReviewTemplateLensReview<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::get_workload::GetWorkload<Eng>>(&mut refusals);
    register::<crate::ops::import_lens::ImportLens<Eng>>(&mut refusals);
    register::<crate::ops::list_agent_contexts::ListAgentContexts<Eng>>(&mut refusals);
    register::<crate::ops::list_agent_goals::ListAgentGoals<Eng>>(&mut refusals);
    register::<crate::ops::list_agent_profiles::ListAgentProfiles<Eng>>(&mut refusals);
    register::<crate::ops::list_agent_recommendation_generations::ListAgentRecommendationGenerations<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_agent_recommendation_items::ListAgentRecommendationItems<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_agent_recommendations::ListAgentRecommendations<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_answers::ListAnswers<Eng>>(&mut refusals);
    register::<crate::ops::list_check_details::ListCheckDetails<Eng>>(&mut refusals);
    register::<crate::ops::list_check_summaries::ListCheckSummaries<Eng>>(&mut refusals);
    register::<crate::ops::list_lens_review_improvements::ListLensReviewImprovements<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_lens_reviews::ListLensReviews<Eng>>(&mut refusals);
    register::<crate::ops::list_lens_shares::ListLensShares<Eng>>(&mut refusals);
    register::<crate::ops::list_lenses::ListLenses<Eng>>(&mut refusals);
    register::<crate::ops::list_milestones::ListMilestones<Eng>>(&mut refusals);
    register::<crate::ops::list_notifications::ListNotifications<Eng>>(&mut refusals);
    register::<crate::ops::list_profile_notifications::ListProfileNotifications<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_profile_shares::ListProfileShares<Eng>>(&mut refusals);
    register::<crate::ops::list_profiles::ListProfiles<Eng>>(&mut refusals);
    register::<crate::ops::list_review_template_answers::ListReviewTemplateAnswers<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::list_review_templates::ListReviewTemplates<Eng>>(&mut refusals);
    register::<crate::ops::list_share_invitations::ListShareInvitations<Eng>>(&mut refusals);
    register::<crate::ops::list_tags_for_resource::ListTagsForResource<Eng>>(&mut refusals);
    register::<crate::ops::list_template_shares::ListTemplateShares<Eng>>(&mut refusals);
    register::<crate::ops::list_workload_shares::ListWorkloadShares<Eng>>(&mut refusals);
    register::<crate::ops::list_workloads::ListWorkloads<Eng>>(&mut refusals);
    register::<crate::ops::put_agent_recommendation_feedback::PutAgentRecommendationFeedback<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::start_agent_recommendation_generation::StartAgentRecommendationGeneration<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::tag_resource::TagResource<Eng>>(&mut refusals);
    register::<crate::ops::untag_resource::UntagResource<Eng>>(&mut refusals);
    register::<crate::ops::update_agent_context::UpdateAgentContext<Eng>>(&mut refusals);
    register::<crate::ops::update_agent_goal::UpdateAgentGoal<Eng>>(&mut refusals);
    register::<crate::ops::update_agent_profile::UpdateAgentProfile<Eng>>(&mut refusals);
    register::<crate::ops::update_agent_recommendation_status::UpdateAgentRecommendationStatus<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::update_answer::UpdateAnswer<Eng>>(&mut refusals);
    register::<crate::ops::update_global_settings::UpdateGlobalSettings<Eng>>(&mut refusals);
    register::<crate::ops::update_integration::UpdateIntegration<Eng>>(&mut refusals);
    register::<crate::ops::update_lens_review::UpdateLensReview<Eng>>(&mut refusals);
    register::<crate::ops::update_profile::UpdateProfile<Eng>>(&mut refusals);
    register::<crate::ops::update_review_template::UpdateReviewTemplate<Eng>>(&mut refusals);
    register::<crate::ops::update_review_template_answer::UpdateReviewTemplateAnswer<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::update_review_template_lens_review::UpdateReviewTemplateLensReview<Eng>>(
        &mut refusals,
    );
    register::<crate::ops::update_share_invitation::UpdateShareInvitation<Eng>>(&mut refusals);
    register::<crate::ops::update_workload::UpdateWorkload<Eng>>(&mut refusals);
    register::<crate::ops::update_workload_share::UpdateWorkloadShare<Eng>>(&mut refusals);
    register::<crate::ops::upgrade_lens_review::UpgradeLensReview<Eng>>(&mut refusals);
    register::<crate::ops::upgrade_profile_version::UpgradeProfileVersion<Eng>>(&mut refusals);
    register::<crate::ops::upgrade_review_template_lens_review::UpgradeReviewTemplateLensReview<Eng>>(
        &mut refusals,
    );
    refusals
}

/// Register one operation's modelled-refusal wrapper under the key the model names that
/// operation by, read off the operation itself so the table cannot be keyed by a
/// hand-typed name that drifts from the model.
fn register<Op: Operation>(refusals: &mut HashMap<&'static str, Box<dyn ErasedOperation>>) {
    refusals.insert(Op::NAME, Box::new(ModelledValidation::<Op>::new()));
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
        // Serve every modelled operation through its [`ModelledValidation`] wrapper, so
        // a request this service refuses before the handler runs is stated in the
        // service's OWN modelled `ValidationException` instead of the framework's
        // generic envelope. The wrapper delegates a valid request to the generated
        // operation in the routing table, which is the only thing that serves it.
        let key = self.definition().protocol().routing_key(req).ok()?;
        self.refusals.get(key).map(|wrapper| &**wrapper)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
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
}

/// One modelled operation, wrapped so a request this service refuses BEFORE the handler
/// runs is answered in its own modelled `ValidationException` rather than the
/// framework's generic envelope.
///
/// Zero-sized: the wrapper holds no engine and no operation. It carries the operation
/// TYPE, which is all it needs — the input shape to read the request into and the input
/// `Schema` to validate it against — and delegates a valid request to the one operation
/// instance the generated routing table built from the shared engine. There is
/// therefore no second instance of any operation and no second store.
struct ModelledValidation<Op> {
    operation: PhantomData<fn() -> Op>,
}

impl<Op> ModelledValidation<Op> {
    fn new() -> Self {
        Self {
            operation: PhantomData,
        }
    }
}

impl<Op: Operation> ErasedOperation for ModelledValidation<Op> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let started = std::time::Instant::now();
            let protocol = svc.definition().protocol();
            // Read off the definition BEFORE any await: `&dyn ErasedServiceDefinition` is
            // not `Sync`, so holding one across an await would make this future non-`Send`.
            let locator = error_type_locator::locator(svc.definition().identity().shape_id());
            if let Some(exception) = modelled_refusal::<Op>(protocol, req) {
                // Serialized through the service's own protocol, exactly as a
                // handler-returned `ValidationException` is: same body member names
                // (`Message`, `Reason`, `Fields`), same `x-amzn-errortype`, and the
                // status the model declares (`@httpError(400)`).
                if let Ok(response) = protocol.serialize_error(
                    &exception,
                    &crate::ops::types::SCHEMA_VALIDATION_EXCEPTION,
                    VALIDATION_EXCEPTION_CODE,
                    VALIDATION_EXCEPTION_STATUS,
                ) {
                    record_refusal::<Op>(ctx, started.elapsed());
                    return with_error_type_locator(response, &locator);
                }
            }
            // Nothing to reword: the request is served by the generated operation,
            // which runs the whole framework sequence (deserialize, validate, handle,
            // serialize) over the same bytes. Whatever it answers passes through this one
            // layer, so every operation's failure names its error shape the way this
            // service's wire names it and a success keeps every byte.
            let response = match svc.definition().routing_table().get(Op::NAME) {
                Some(operation) => operation.run(svc, req, ctx).await,
                // Unreachable in practice: this wrapper is only reachable under the key
                // the generated table registered. Answer the framework's own
                // no-such-operation response rather than inventing one.
                None => svc.definition().routing_error_response(req),
            };
            with_error_type_locator(response, &locator)
        })
    }
}

/// The wire error code of this service's `ValidationException` — the `__type` /
/// `x-amzn-errortype` a client matches on, and the same name a handler-returned one
/// carries.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// `ValidationException`'s modelled `@httpError`.
const VALIDATION_EXCEPTION_STATUS: u16 = 400;

/// Record the refusal answered above as the operation row the framework sequence would
/// have recorded, so a refused request is not a hole in the telemetry series: the same
/// classification (from this response's status and error shape), the same folded handle
/// (this service's signing name and the operation's own name), and the same already
/// sanitized request metadata. Everything is gated on a client collecting, exactly as
/// the framework gates it.
fn record_refusal<Op: Operation>(ctx: &OperationContext<'_>, latency: std::time::Duration) {
    if !emulator_telemetry::recording_active() {
        return;
    }
    let handle =
        emulator_telemetry::OperationHandle::folded(ctx.identity().signing_name_static(), Op::NAME);
    emulator_telemetry::record_generated_operation_with_handle(
        &handle,
        latency,
        Some(emulator_telemetry::OperationFailure::classify(
            VALIDATION_EXCEPTION_STATUS,
            VALIDATION_EXCEPTION_CODE,
        )),
        smithy_server_runtime::RequestTelemetry::from_headers(&ctx.parts().headers).into_metadata(),
    );
}

/// How this service states the request `Op` cannot be served from, or `None` when it
/// has nothing of its own to say — in which case the request goes on to the generated
/// operation and the framework answers as it always did.
///
/// This is the boundary walk the framework runs, in the framework's order and with the
/// framework's own code: read the request into the operation's input shape, then run
/// constraint-trait validation over what was read. Only the WORDS and the envelope are
/// this service's.
fn modelled_refusal<Op: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<crate::ops::types::ValidationException> {
    let input = {
        let mut deserializer = protocol.deserialize_input(req, Op::INPUT_SCHEMA).ok()?;
        match <Op::Input as DeserializableStruct>::deserialize(&mut *deserializer) {
            Ok(input) => input,
            // A member the shape requires is absent. It is found while READING the
            // request — the layer that reads it reports the PROPERTY the object does not
            // carry, not a per-member field finding — so the answer is the schema report
            // for a request that could not be read as its shape requires.
            Err(aws_smithy_schema::serde::SerdeError::MissingMember { member_name }) => {
                return Some(crate::state::absent_member_exception(&missing_property(
                    &member_name,
                    Op::INPUT_SCHEMA,
                )));
            }
            // Any other codec failure is not a refusal this service rewords.
            Err(_) => return None,
        }
    };
    let violations = constraints::validate(&input, Op::INPUT_SCHEMA);
    (!violations.is_empty()).then(|| constraint_refusal(&violations, Op::INPUT_SCHEMA))
}

/// The property an absent-member finding names: the member as the SCHEMA declares it
/// (`ReviewOwner`), which is what a caller would have had to send.
///
/// The deserializer reports the member by the path it unwound to, spelled lowerCamel the
/// way every member path is, so the declared name is recovered from the input shape. A path
/// into a NESTED shape is named by its last segment, the member of that shape: the input's
/// own members are the only ones its schema declares.
fn missing_property(member_path: &str, input_schema: &Schema) -> String {
    let segment = member_path.split('.').next_back().unwrap_or(member_path);
    top_level_member(segment, input_schema)
        .and_then(Schema::member_name)
        .unwrap_or(segment)
        .to_string()
}

/// How this service states a value that broke a modelled input constraint.
///
/// A value the request BODY carried is measured by the layer that READ the body, and that
/// layer reports what it found rather than naming the member: such a refusal is stated as
/// its findings ([`schema_findings`]), with the required `Message` alone. A value bound
/// OUTSIDE the body is parsed by the service itself, so a value outside the set its
/// enumerated type admits is a field validation naming that TYPE
/// ([`enumerated_type_fields`]), and a value outside a modelled numeric `@range` is a
/// service LIMIT naming the member and its published bounds. Every other constraint keeps
/// the facts the framework's envelope carried, in the modelled shape.
fn constraint_refusal(
    violations: &[Violation],
    input_schema: &Schema,
) -> crate::ops::types::ValidationException {
    if let Some(findings) = schema_findings(violations, input_schema) {
        return crate::state::unclassified_exception(findings);
    }
    if let Some(fields) = enumerated_type_fields(violations, input_schema) {
        return crate::state::field_validation_exception(&fields);
    }
    let bounded: Vec<(String, String)> = violations
        .iter()
        .filter_map(|violation| bounded_member(violation, input_schema))
        .collect();
    if bounded.is_empty() {
        crate::state::unclassified_exception(
            constraints::envelope(violations)
                // `violations` is non-empty at every call site, so this is unreachable;
                // the modelled `Message` is required, so it still gets a value.
                .unwrap_or_else(|| "The user input is not valid.".to_string()),
        )
    } else {
        crate::state::field_validation_exception(&bounded)
    }
}

/// The whole message this service states for a request whose BODY broke the rules the
/// layer that reads it measures: the schema findings that layer lists, one per offending
/// value, in the order the validating walk found them.
///
/// The service reads a request BODY through a JSON-schema layer, and that layer reports
/// what it FOUND rather than naming the member: a finding quotes the value the request
/// carried and the rule it was measured against — the values the MODEL declares for that
/// member's type, the expression the member's `@pattern` declares, or the `@length` bound
/// it broke with the length that was counted. Every half therefore comes from the request
/// and the model, never from a table here, and each sentence is the shared renderer's.
///
/// ALL OR NOTHING, for the reason the shared renderer's own report gives: `None` when a
/// violation is of a class this layer states no finding for, when it is about a value
/// bound to the URI or a header rather than to the body — those travel outside the layer
/// that words findings this way — or when the value is withheld (a `@sensitive` member),
/// so a message is never half in one voice and half in the framework's.
fn schema_findings(violations: &[Violation], input_schema: &Schema) -> Option<String> {
    if violations.is_empty() {
        return None;
    }
    let findings: Vec<String> = violations
        .iter()
        .map(|violation| schema_finding(violation, input_schema))
        .collect::<Option<Vec<_>>>()?;
    Some(service_common::validation_message::bracketed_findings(
        &findings,
    ))
}

/// One finding of [`schema_findings`], or `None` when `violation` is not one that layer
/// reports about a value the request body carried.
fn schema_finding(violation: &Violation, input_schema: &Schema) -> Option<String> {
    if !service_common::front_door::binds_to_body(input_schema, &violation.path) {
        return None;
    }
    let value = violation.value.as_deref()?;
    match &violation.kind {
        // The values the MODEL declares for the enumerated type, in its declared order —
        // the order the validating walk carries them in — and the value the caller sent.
        // Reached through the member, so a member at any depth of the body is stated the
        // same way.
        ViolationKind::EnumValue(permitted) => Some(
            service_common::validation_message::enum_value_not_found(value, permitted),
        ),
        // The expression the member declares, as the model writes it, and the string the
        // caller submitted. `@pattern` applies to a string shape, so a pattern finding
        // always describes one at whatever depth it was found.
        ViolationKind::Pattern(pattern) => Some(
            service_common::validation_message::pattern_mismatch(pattern, value),
        ),
        // `@length` counts a STRING's characters here. `@length` also bounds a list or a
        // map, which that layer reports as the collection it is rather than as a string,
        // and whose shape is only knowable for a member of the input itself
        // ([`is_string_member`]) — so a bound broken by anything but a string member of the
        // input keeps the framework's wording.
        ViolationKind::MinLength(min) => {
            if !is_string_member(&violation.path, input_schema) {
                return None;
            }
            let length = value.chars().count();
            ((length as i64) < *min)
                .then(|| service_common::validation_message::string_too_short(value, length, *min))
        }
        ViolationKind::MaxLength(max) => {
            if !is_string_member(&violation.path, input_schema) {
                return None;
            }
            // The quoted value is the framework's echo of it, which elides a value past
            // its ceiling: a finding is stated only while the value it quotes still shows
            // the length it states, so no message states a length its own text contradicts.
            let length = value.chars().count();
            ((length as i64) > *max)
                .then(|| service_common::validation_message::string_too_long(value, length, *max))
        }
        _ => None,
    }
}

/// Whether `path` names a member the input itself declares as a STRING shape — the one
/// position where a length clause is known to describe a string, because a member schema
/// carries no target and so says nothing about the shapes of a nested path's segments.
fn is_string_member(path: &str, input_schema: &Schema) -> bool {
    path.split('.').count() == 1
        && top_level_member(path, input_schema)
            .is_some_and(|member| matches!(member.shape_type(), ShapeType::String))
}

/// Every enumerated type a request of this service binds OUTSIDE its body, by the value
/// set the model declares for it.
///
/// A value bound to the query string is parsed by the SERVICE into the type the member
/// targets, rather than read through the schema layer that measures the body, so a value
/// outside the set is reported as that TYPE's own failure and the type has to be named.
/// A member schema carries the member's name and not its target's
/// (`Schema::new_member`), so the type is recognised by the value set the violation
/// carries — the one the model declares for it — against the generated value sets below.
///
/// SOURCE: the members these operations bind outside the body
/// (`smithy_query(query="operation", operation="DeleteLens,ListLenses,ListLensShares,ListTemplateShares,ListWorkloadShares,ListProfileShares,ListShareInvitations,GetConsolidatedReport,ListAnswers,ListLensReviewImprovements,ListProfiles,ListAgentRecommendations,ListAgentRecommendationItems,ListAgentRecommendationGenerations,GetAgentRecommendation")`):
/// `LensStatus` -> `LensStatusType`, `LensType` -> `LensType`, `Status` -> `ShareStatus`,
/// `ShareResourceType` -> `ShareResourceType`, `Format` -> `ReportFormat`,
/// `QuestionPriority` -> `QuestionPriority`, `ProfileOwnerType` -> `ProfileOwnerType`,
/// `state` -> `RecommendationState`, `pillar` -> `Pillar`, `type` ->
/// `RecommendationItemType`, `RecommendationType` -> `RecommendationType`,
/// `remediationType` -> `RemediationType`. No two of them declare the same value set, so
/// the set identifies the type.
const ENUMERATED_TYPES_OUTSIDE_THE_BODY: &[(&str, &[&str])] = &[
    ("LensStatusType", crate::ops::types::LENS_STATUS_TYPE_VALUES),
    ("LensType", crate::ops::types::LENS_TYPE_VALUES),
    ("ShareStatus", crate::ops::types::SHARE_STATUS_VALUES),
    (
        "ShareResourceType",
        crate::ops::types::SHARE_RESOURCE_TYPE_VALUES,
    ),
    ("ReportFormat", crate::ops::types::REPORT_FORMAT_VALUES),
    (
        "QuestionPriority",
        crate::ops::types::QUESTION_PRIORITY_VALUES,
    ),
    (
        "ProfileOwnerType",
        crate::ops::types::PROFILE_OWNER_TYPE_VALUES,
    ),
    (
        "RecommendationState",
        crate::ops::types::RECOMMENDATION_STATE_VALUES,
    ),
    ("Pillar", crate::ops::types::PILLAR_VALUES),
    (
        "RecommendationItemType",
        crate::ops::types::RECOMMENDATION_ITEM_TYPE_VALUES,
    ),
    (
        "RecommendationType",
        crate::ops::types::RECOMMENDATION_TYPE_VALUES,
    ),
    (
        "RemediationType",
        crate::ops::types::REMEDIATION_TYPE_VALUES,
    ),
];

/// The `(field, message)` findings for the values bound OUTSIDE the body that are outside
/// the set their enumerated type admits, or `None` when the refusal holds anything else.
///
/// ALL OR NOTHING for the same reason [`schema_findings`] is: a refusal reported half as a
/// field validation and half in the framework's envelope sends a client looking in
/// `Fields` for a finding that is not there.
fn enumerated_type_fields(
    violations: &[Violation],
    input_schema: &Schema,
) -> Option<Vec<(String, String)>> {
    if violations.is_empty() {
        return None;
    }
    violations
        .iter()
        .map(|violation| enumerated_type_field(violation, input_schema))
        .collect::<Option<Vec<_>>>()
}

/// One finding of [`enumerated_type_fields`]: the enumerated TYPE the value was parsed
/// into, which is both the field a client attributes the failure to and the subject of the
/// sentence that states it.
fn enumerated_type_field(
    violation: &Violation,
    input_schema: &Schema,
) -> Option<(String, String)> {
    let ViolationKind::EnumValue(permitted) = &violation.kind else {
        return None;
    };
    if service_common::front_door::binds_to_body(input_schema, &violation.path) {
        return None;
    }
    let declared = enumerated_type_name(permitted)?;
    Some((
        declared.to_string(),
        crate::state::invalid_parameter_sentence(declared),
    ))
}

/// The enumerated type whose declared value set is `permitted`, or `None` when no type
/// this service binds outside its body declares that set.
fn enumerated_type_name(permitted: &[String]) -> Option<&'static str> {
    let mut sent: Vec<&str> = permitted.iter().map(String::as_str).collect();
    sent.sort_unstable();
    ENUMERATED_TYPES_OUTSIDE_THE_BODY
        .iter()
        .find(|(_, declared)| {
            let mut declared: Vec<&str> = declared.to_vec();
            declared.sort_unstable();
            declared == sent
        })
        .map(|(name, _)| *name)
}

/// The `(member, message)` finding for a value outside a modelled numeric `@range`, or
/// `None` for any other violation.
///
/// The member is named as the MODEL names it (`MaxResults`), not as the validating walk
/// spells its path (`maxResults`): the path is lowerCamel by construction, while
/// `Fields$Name` is "the field name for which validation failed" — the member a caller
/// sent. Both bounds are read off the member's own `@range` trait, so the sentence
/// states the published range rather than only the bound that failed.
fn bounded_member(violation: &Violation, input_schema: &Schema) -> Option<(String, String)> {
    if !matches!(
        violation.kind,
        ViolationKind::MinValue(_) | ViolationKind::MaxValue(_)
    ) {
        return None;
    }
    let member = top_level_member(&violation.path, input_schema)?;
    let name = member.member_name()?.to_string();
    let range = range_of(member)?;
    let message = match (range.min, range.max) {
        (Some(min), Some(max)) => format!(
            "{name} must be between {} and {}.",
            render_bound(min),
            render_bound(max)
        ),
        (Some(min), None) => format!(
            "{name} must be greater than or equal to {}.",
            render_bound(min)
        ),
        (None, Some(max)) => {
            format!("{name} must be less than or equal to {}.", render_bound(max))
        }
        (None, None) => return None,
    };
    Some((name, message))
}

/// The input member a violation path starts at, by matching the path's first segment
/// against each member's lowerCamel name. `None` for a path into a nested shape, which
/// no `@range` member of this service has.
fn top_level_member<'a>(path: &str, input_schema: &'a Schema) -> Option<&'a Schema> {
    let segment = path.split('.').next()?;
    input_schema.members().iter().copied().find(|member| {
        member
            .member_name()
            .is_some_and(|name| lower_camel(name) == segment)
    })
}

/// A member's `@range` trait, if it declares one.
fn range_of(member: &Schema) -> Option<RangeTrait> {
    let traits = member.traits()?;
    let range = traits.get(&RangeTrait::TRAIT_ID)?;
    range.as_any().downcast_ref::<RangeTrait>().copied()
}

/// Lowercase the first character, the spelling the validating walk gives a member's
/// path segment (`MaxResults` -> `maxResults`).
fn lower_camel(name: &str) -> String {
    let mut characters = name.chars();
    match characters.next() {
        Some(first) => first.to_lowercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// A `@range` bound as a message states it: an integral bound without a decimal point
/// (`1`, not `1.0`), which is how AWS words a bound on an integer member.
fn render_bound(bound: f64) -> String {
    if bound.fract() == 0.0 && bound.abs() < 1e15 {
        format!("{}", bound as i64)
    } else {
        format!("{bound}")
    }
}
