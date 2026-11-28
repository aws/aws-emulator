// SPDX-License-Identifier: CC0-1.0

//! The REFERENCED-resource authorization this service takes BEFORE it validates a request.
//!
//! ## The rule this module implements
//!
//! A Clean Rooms ML job start names the resource the job runs — the configured audience model
//! an audience generation job generates an audience from, the trained model an inference job
//! runs predictions with. Clean Rooms ML RESOLVES that reference, and asks whether a policy on
//! the resource it names allows the action, AHEAD of request validation: a reference that
//! resolves to no resource has no resource-based policy that could allow the call, so it is
//! refused the modelled `AccessDeniedException` (HTTP 403) — stating the reference that did not
//! resolve and the IAM action the call asks for — and not the `ValidationException` (HTTP 400)
//! a member outside its modelled constraints produces, even when the request also carries such
//! a member.
//!
//! What RESOLVING one comes to is the reference's own, and each refusal states what failed:
//!
//! * a configured audience model is resolved from the value itself — whether it names a
//!   configured audience model of this service at all — and a value that names no such
//!   resource is stated invalid
//!   ([`crate::state::resource_authorization::invalid_arn_message`]);
//! * a trained model is resolved through the configured model algorithm association it runs
//!   through, which this endpoint either HOLDS or does not, and an association it holds none of
//!   is stated not found
//!   ([`crate::state::resource_authorization::association_not_found_message`]).
//!
//! The class is the operations whose input declares a `@required` reference to the resource the
//! call RUNS ([`REFERENCES`]); the decision is the same for each, and the action named is that
//! operation's own. A request whose reference DOES resolve is left to the modelled walk, which
//! is what refuses its members on their own terms.
//!
//! ## Why it is an operation rather than a refusal wording
//!
//! The refusal has to come BEFORE the framework's modelled-constraint walk, which has already
//! chosen 400 by the time a service can reword it ([`crate::service`]'s `override_refusal`
//! chooses the code and the message, and the protocol owns the status).
//! `Service::override_operation` is the seam that sees a whole request first, so the denial is
//! an [`ErasedOperation`] that answers that one request — the same shape the role-ARN gate
//! beside it takes ([`super::role_arn_authorization`]), and both render their 403 through the
//! one renderer here.

use std::sync::Arc;

use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, ServerHttpProtocol, Service,
};

use crate::state::resource_authorization::{
    association_not_found_message, denied_by_no_resource_based_policy, iam_action,
    invalid_arn_message, CONFIGURED_AUDIENCE_MODEL_ARN_SHAPE,
};

/// The error code and HTTP status an authorization failure of this service is answered with.
///
/// `AccessDeniedException` is modelled by every operation that takes one, `@error("client")`
/// at `@httpError(403)`
/// (`smithy_query(query="operation",operation="StartAudienceGenerationJob")`).
pub(super) const ACCESS_DENIED_CODE: &str = "AccessDeniedException";
pub(super) const ACCESS_DENIED_STATUS: u16 = 403;

/// The sentence the modelled error states about itself, answered only in the unreachable case
/// where a denial is reached for a request whose refusal cannot be read out of it any more.
pub(super) const INSUFFICIENT_ACCESS: &str = "You do not have sufficient access to perform this action.";

/// The namespace the read-only schemas below name themselves in. They stand for the READ this
/// gate performs, not for a shape of the model — the model's own input shapes carry every
/// other member of their operation, which this read deliberately does not look at — so they
/// are named after the gate.
pub(super) const AUTHORIZATION_READ_NAMESPACE: &str = "emulator.authorization";

/// The member `StartAudienceGenerationJob` names the configured audience model at, as the wire
/// spells it.
const CONFIGURED_AUDIENCE_MODEL_ARN_MEMBER: &str = "configuredAudienceModelArn";

/// The member `StartTrainedModelInferenceJob` names the trained model at.
const TRAINED_MODEL_ARN_MEMBER: &str = "trainedModelArn";

/// The member `StartTrainedModelInferenceJob` may name the association to run the model
/// through at. It is optional: a request that leaves it out runs the model through the
/// association the model was trained through.
const ASSOCIATION_ARN_MEMBER: &str = "configuredModelAlgorithmAssociationArn";

/// Which resource each operation of the class references, and the one-member body schema that
/// reference is read through.
///
/// Keyed by OPERATION, so an operation that references no resource of this service never has
/// its body read here, and a member of another operation that happens to be spelled the same
/// way is not judged by this gate until it is listed.
const REFERENCES: [(&str, ReferenceKind, &Schema); 2] = [
    (
        "StartAudienceGenerationJob",
        ReferenceKind::ConfiguredAudienceModel,
        &AUDIENCE_GENERATION_JOB_START,
    ),
    (
        "StartTrainedModelInferenceJob",
        ReferenceKind::TrainedModel,
        &TRAINED_MODEL_INFERENCE_JOB_START,
    ),
];

/// Which resource a start references, which decides both the read and what the refusal states.
#[derive(Debug, Clone, Copy)]
enum ReferenceKind {
    /// The configured audience model an audience generation job generates from.
    ConfiguredAudienceModel,
    /// The trained model an inference job runs, through a configured model algorithm
    /// association.
    TrainedModel,
}

/// The `configuredAudienceModelArn` member of a request, typed as the string it is on the wire.
static CONFIGURED_AUDIENCE_MODEL_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#AudienceGenerationJobStart$configuredAudienceModelArn",
        AUTHORIZATION_READ_NAMESPACE,
        CONFIGURED_AUDIENCE_MODEL_ARN_MEMBER,
    ),
    ShapeType::String,
    CONFIGURED_AUDIENCE_MODEL_ARN_MEMBER,
    0,
);

/// The body schema an audience generation job start is read through: a structure declaring
/// THAT ONE member.
///
/// Authorization is taken BEFORE the request is parsed as its operation's input, so the read
/// that feeds it is not the modelled input shape: a codec skips every member a schema does not
/// declare, so this reads the reference and nothing else, and a body that also misses another
/// required member, or carries one outside its constraints, is still judged on the resource it
/// referenced.
static AUDIENCE_GENERATION_JOB_START: Schema = Schema::new_struct(
    ShapeId::from_static(
        "emulator.authorization#AudienceGenerationJobStart",
        AUTHORIZATION_READ_NAMESPACE,
        "AudienceGenerationJobStart",
    ),
    ShapeType::Structure,
    &[&CONFIGURED_AUDIENCE_MODEL_ARN],
);

/// The `trainedModelArn` member of a request, typed as the string it is on the wire.
static TRAINED_MODEL_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#TrainedModelInferenceJobStart$trainedModelArn",
        AUTHORIZATION_READ_NAMESPACE,
        TRAINED_MODEL_ARN_MEMBER,
    ),
    ShapeType::String,
    TRAINED_MODEL_ARN_MEMBER,
    0,
);

/// The `configuredModelAlgorithmAssociationArn` member of a request, read the same way.
static ASSOCIATION_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#TrainedModelInferenceJobStart$configuredModelAlgorithmAssociationArn",
        AUTHORIZATION_READ_NAMESPACE,
        ASSOCIATION_ARN_MEMBER,
    ),
    ShapeType::String,
    ASSOCIATION_ARN_MEMBER,
    1,
);

/// The body schema a trained model inference job start is read through: the trained model it
/// runs, and the association the request named to run it through, if it named one.
static TRAINED_MODEL_INFERENCE_JOB_START: Schema = Schema::new_struct(
    ShapeId::from_static(
        "emulator.authorization#TrainedModelInferenceJobStart",
        AUTHORIZATION_READ_NAMESPACE,
        "TrainedModelInferenceJobStart",
    ),
    ShapeType::Structure,
    &[&TRAINED_MODEL_ARN, &ASSOCIATION_ARN],
);

/// What one request references, read off its body before it is validated.
#[derive(Debug)]
enum Reference {
    /// The configured audience model the audience generation job would generate from.
    ConfiguredAudienceModel { arn: String },
    /// The trained model the inference job would run, and the association the request named
    /// for it, when it named one.
    TrainedModel {
        arn: String,
        association_arn: Option<String>,
    },
}

impl Reference {
    /// What the refusal STATES about this reference when it resolves to nothing, and `None`
    /// when it resolves — or when nothing is established at all, because this engine carries
    /// no store of this service to resolve a held resource against.
    ///
    /// `region` is the endpoint the call reached: a Clean Rooms ML endpoint holds the resources
    /// created under its own region and no others, so it is the scope a held resource is
    /// resolved in — the same scope every handler of this service reads under.
    fn unresolved<Eng: Send + Sync + 'static>(&self, engine: &Eng, region: &str) -> Option<String> {
        match self {
            // A configured audience model is resolved from the value itself: it either names
            // one of this service's configured audience models or it names no such resource,
            // and a value that names none is what the refusal states as invalid.
            Self::ConfiguredAudienceModel { arn } => {
                (!crate::state::is_configured_audience_model_arn(arn))
                    .then(|| invalid_arn_message(CONFIGURED_AUDIENCE_MODEL_ARN_SHAPE, arn))
            }
            Self::TrainedModel {
                arn,
                association_arn,
            } => {
                let state = crate::state::service_state(engine)?;
                let store = state.lock();
                // The association the job runs through: the one the request named, else the
                // one the model was trained through, which is held on the model's own record.
                // Either way it is the store that has to hold it, so a model this endpoint
                // does not serve resolves to no association at all.
                let resolved = store.trained_model(arn, region).is_some_and(|model| {
                    let association = association_arn
                        .as_deref()
                        .unwrap_or(&model.configured_model_algorithm_association_arn);
                    store
                        .configured_model_algorithm_association(association, region)
                        .is_some()
                });
                (!resolved).then(|| association_not_found_message(arn))
            }
        }
    }
}

/// Answers one request `AccessDeniedException` (HTTP 403), with the denial built from that
/// request and from this service's own store.
///
/// Holds the shared engine and nothing else: the reference is read off the request and
/// resolved against the store every handler of this service reads, so the gate and the refusal
/// it answers with cannot disagree about which resource the endpoint holds.
pub(crate) struct DenyUnauthorizedReference<Eng> {
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> DenyUnauthorizedReference<Eng> {
    /// Build the denial from the shared engine — the same engine the operations are
    /// constructed from.
    pub(crate) fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// Does `req` reference a resource that resolves to nothing?
    ///
    /// The read is [`unauthorized_reference`], the one the refusal is built from too, so the
    /// dispatch gate and the message it answers with cannot disagree about which request is
    /// refused.
    pub(crate) fn applies(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> bool {
        unauthorized_reference(&*self.engine, protocol, req).is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for DenyUnauthorizedReference<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        // Nothing about this refusal is read from the request context: it is decided from the
        // request's own operation and body, and from this service's store.
        let _ = ctx;
        // The answer is computed in one call that reads, uses and drops the service
        // definition: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
        // await below would make this future non-`Send`.
        let response = refusal(&*self.engine, svc, req);
        Box::pin(async move { response })
    }
}

/// The 403 for `req`, or the error's own sentence for a request this can no longer read a
/// refusal out of (unreachable: `Service::override_operation` read one out of these same bytes
/// to get here).
fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
) -> aws_smithy_runtime_api::http::Response {
    let definition = svc.definition();
    // The protocol THIS request is served in: the request is read, and the denial answered,
    // in the one codec the request arrived in.
    let Some((operation, stated)) =
        unauthorized_reference(engine, definition.wire_protocol(req).protocol(), req)
    else {
        return insufficient_access(svc, req);
    };
    let message = denied_by_no_resource_based_policy(
        &stated,
        &iam_action(definition.identity().signing_name(), operation),
    );
    access_denied(svc, req, message)
}

/// The operation `req` names and what the refusal states about the resource it references, when
/// `req` is one of [`REFERENCES`] and that reference resolves to nothing.
///
/// `None` — leave the request to the modelled walk — for an operation that references nothing,
/// for a reference that DOES resolve, and for a body this cannot read at all (one that is not
/// the serving protocol's object, or whose reference member is absent or is not a string): then
/// nothing has been established about the resource, and the modelled walk refuses the request
/// on its own terms.
///
/// `protocol` is the codec the request is SERVED in, so the read is the one the rest of the
/// request uses: an authorization decision is taken from what a request states, never from the
/// encoding it stated it in.
fn unauthorized_reference<Eng: Send + Sync + 'static>(
    engine: &Eng,
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<(&'static str, String)> {
    let routing_key = protocol.routing_key(req).ok()?;
    let (operation, kind, request_schema) = REFERENCES
        .iter()
        .find(|(candidate, _, _)| *candidate == routing_key)?;
    let reference = referenced(protocol, req, *kind, request_schema)?;
    reference
        .unresolved(engine, &signed_region(req))
        .map(|stated| (*operation, stated))
}

/// What `req` references, read through the codec the request is SERVED in and against
/// `request_schema`'s members. `None` when the body could not be read, or carried no reference
/// at all — neither of which establishes anything about any resource.
fn referenced(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    kind: ReferenceKind,
    request_schema: &Schema,
) -> Option<Reference> {
    let mut reader = protocol.deserialize_input(req, request_schema).ok()?;
    let mut referenced_arn = None;
    let mut association_arn = None;
    reader
        .read_struct(request_schema, &mut |member, value| {
            let read = value.read_string(member)?;
            if member.member_name() == Some(ASSOCIATION_ARN_MEMBER) {
                association_arn = Some(read);
            } else {
                referenced_arn = Some(read);
            }
            Ok(())
        })
        .ok()?;
    let arn = referenced_arn?;
    Some(match kind {
        ReferenceKind::ConfiguredAudienceModel => Reference::ConfiguredAudienceModel { arn },
        ReferenceKind::TrainedModel => Reference::TrainedModel {
            arn,
            association_arn,
        },
    })
}

/// The region `req` was signed for — the scope the referenced resource is resolved under, read
/// exactly as a handler reads it ([`crate::state::signed_region`]).
///
/// The parse is the framework's own and takes a header map, so the one header a credential
/// scope is carried in is handed to it; a request carrying none resolves under the same
/// default region a handler would mint an ARN in.
fn signed_region(req: &aws_smithy_runtime_api::http::Request) -> String {
    let mut headers = ::http::HeaderMap::new();
    if let Some(value) = req
        .headers()
        .get(::http::header::AUTHORIZATION.as_str())
        .and_then(|presented| ::http::HeaderValue::from_str(presented).ok())
    {
        headers.insert(::http::header::AUTHORIZATION, value);
    }
    crate::state::signed_region(&headers)
}

/// The 403 `AccessDeniedException` carrying `message`, rendered through this service's own
/// protocol — the same envelope (`__type` + `message`, and the same error-type header) a
/// handler-raised `AccessDeniedException` goes out in, so a refusal taken before validation and
/// one taken in a handler are indistinguishable on the wire.
///
/// The one renderer both pre-validation gates of this service answer through.
pub(super) fn access_denied(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
    message: String,
) -> aws_smithy_runtime_api::http::Response {
    let definition = svc.definition();
    let wire = definition.wire_protocol(req);
    let denial = crate::ops::types::AccessDeniedException { message };
    wire.protocol()
        .serialize_error(
            &denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_CODE,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|error| definition.framework_error_response(error, req))
}

/// The 403 for a request that names no action at all, carrying the error's own sentence.
pub(super) fn insufficient_access(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
) -> aws_smithy_runtime_api::http::Response {
    svc.definition().framework_error_response(
        aws_smithy_schema_server::ProtocolError::Unauthenticated {
            code: ACCESS_DENIED_CODE,
            message: INSUFFICIENT_ACCESS,
        },
        req,
    )
}
