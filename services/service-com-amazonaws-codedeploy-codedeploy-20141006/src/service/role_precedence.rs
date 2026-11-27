// SPDX-License-Identifier: CC0-1.0

//! What a deployment group's create and update answer BEFORE a refusal of the
//! request body: the SERVICE ROLE the group is to run as.
//!
//! CodeDeploy judges the role first. A `serviceRoleArn` that is no IAM role ARN,
//! and one naming a role of this account that does not let CodeDeploy assume it,
//! are both refused with the modelled `InvalidRoleException` — before the request's
//! other member VALUES are judged, as the recorded service does for a request that
//! carries an out-of-enum tag-filter type beside such a role.
//!
//! The handlers in `crate::ops` already keep that order among the checks they make
//! ([`crate::state::check_service_role_arn`] needs no state, and the trust verdict
//! is read before the store is touched). The framework's own walk does not: it
//! validates the whole input against the operation's generated schema and refuses a
//! modelled constraint BEFORE `handle` runs
//! (`vendor/smithy-server-runtime/src/operation.rs`), so for a request that breaks
//! one of those constraints the member refusal reached the caller first and the role
//! was never judged. A client catching the modelled `InvalidRoleException` — the
//! retry-on-a-freshly-created-role loop the error exists for — saw a parameter
//! complaint instead.
//!
//! This layer restores the order for exactly those requests. It runs in the one seam
//! a service has BEFORE dispatch
//! (`smithy_server_runtime::Service::override_operation`), and it claims a request
//! only when ALL of:
//!
//! * the operation it was sent to SETS a deployment group's service role —
//!   `CreateDeploymentGroup` or `UpdateDeploymentGroup`, the two operations that
//!   take a `serviceRoleArn` and model `InvalidRoleException` for it;
//! * the framework's validating walk would refuse the body. The walk is run HERE,
//!   through the same `aws_smithy_schema_server::constraints::validate` and the same
//!   generated input schema the dispatch sequence runs, so this layer cannot
//!   disagree with the walk that would otherwise follow it. A request whose body the
//!   walk accepts is NOT claimed: its handler already judges the role in the right
//!   order, and nothing here may change what it reports;
//! * the role actually earns a refusal. Otherwise the request falls through to the
//!   modelled operation, which refuses the body exactly as before.
//!
//! The answers are the service's own modelled shapes, rendered through the same
//! protocol call the framework's error tail makes, and built by the same functions
//! the handlers build them with — so a refusal worded here and one worded there are
//! indistinguishable on the wire.
//!
//! WHICH resource the trust verdict is given about: a role the account's IAM holds
//! and whose trust policy withholds the assume is refused only once everything the
//! request names AHEAD of the role has resolved — the deployment configuration it
//! names, the application a group is created under, the group an update names —
//! because the recorded service answers an application it does not hold first, and
//! the handler answers a configuration it does not hold before that. A request this
//! layer cannot resolve those for falls through to the handler, which answers the
//! missing one.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::validate;
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, OperationContext,
    OperationError, Refused, ServerHttpProtocol, Service,
};

use crate::ops::types::{
    CreateDeploymentGroupInput, InvalidRoleException, RoleRequiredException,
    UpdateDeploymentGroupInput, CREATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
    SCHEMA_INVALID_ROLE_EXCEPTION, SCHEMA_ROLE_REQUIRED_EXCEPTION,
    UPDATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
};

/// The operation that creates a deployment group, as its routing key names it.
const CREATE_DEPLOYMENT_GROUP: &str = "CreateDeploymentGroup";
/// The operation that updates one.
const UPDATE_DEPLOYMENT_GROUP: &str = "UpdateDeploymentGroup";

/// The error code a request naming no service role at all is refused with.
const ROLE_REQUIRED_EXCEPTION: &str = "RoleRequiredException";
/// The error code a request whose service role CodeDeploy will not take is refused
/// with.
const INVALID_ROLE_EXCEPTION: &str = "InvalidRoleException";

/// The role question one claimed request carries: the value it named, and the
/// resource the trust verdict is given about once that resource resolves.
#[derive(Debug)]
struct Addressed {
    /// The `serviceRoleArn` the request carries, or `None` where the request named
    /// none (`UpdateDeploymentGroup` leaves the group's role unchanged then, so
    /// there is nothing to judge).
    role_arn: Option<String>,
    /// Whether the role is REQUIRED of this request, so an absent or blank one is
    /// its own refusal: `CreateDeploymentGroup` models `serviceRoleArn` `@required`,
    /// `UpdateDeploymentGroup` does not.
    role_required: bool,
    /// The application the request is addressed to.
    application_name: String,
    /// The deployment group the request is addressed to, for an update; `None` for a
    /// create, which is what brings the group into being.
    deployment_group_name: Option<String>,
    /// The deployment configuration the request NAMES, if any: the handler resolves
    /// it ahead of the application, so a request naming one the account does not
    /// hold is answered for the configuration and not for the role.
    deployment_config_name: Option<String>,
}

/// One answer this layer gives, as the shape and status the service models for it.
#[derive(Debug)]
enum Answer {
    /// The request carried no service role at all.
    RoleRequired(RoleRequiredException),
    /// The value is no IAM role ARN, or names a role CodeDeploy cannot assume.
    RoleInvalid(InvalidRoleException),
}

impl OperationError for Answer {
    fn as_serializable(&self) -> &dyn SerializableStruct {
        match self {
            Self::RoleRequired(exception) => exception,
            Self::RoleInvalid(exception) => exception,
        }
    }

    fn variant_schema(&self) -> &'static Schema {
        match self {
            Self::RoleRequired(_) => &SCHEMA_ROLE_REQUIRED_EXCEPTION,
            Self::RoleInvalid(_) => &SCHEMA_INVALID_ROLE_EXCEPTION,
        }
    }

    fn shape_name(&self) -> &'static str {
        match self {
            Self::RoleRequired(_) => ROLE_REQUIRED_EXCEPTION,
            Self::RoleInvalid(_) => INVALID_ROLE_EXCEPTION,
        }
    }

    /// The status the model pins on both shapes: `@httpError 400`
    /// (`smithy_query(query="operation",operation="CreateDeploymentGroup")` reports
    /// it for `RoleRequiredException` and `InvalidRoleException` alike).
    fn http_status(&self) -> u16 {
        400
    }

    fn query_error_code(&self) -> Option<&'static str> {
        // Neither wire format this service declares carries an `awsQueryError`
        // code at all; both shapes are reported by their own name.
        None
    }
}

impl std::fmt::Display for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RoleRequired(exception) => write!(f, "{exception}"),
            Self::RoleInvalid(exception) => write!(f, "{exception}"),
        }
    }
}

impl std::error::Error for Answer {}

impl Answer {
    /// This answer as a response in the service's own protocol envelope, serialized
    /// through the same protocol call the framework's error tail makes.
    ///
    /// FAIL CLOSED: a request is claimed only when this answer fires, so it must
    /// never fall through to a handler that would report something else. Where the
    /// modelled envelope cannot be rendered at all, the same code and message are
    /// answered through the service's framework-refusal envelope instead.
    fn response(self, definition: &dyn ErasedServiceDefinition, req: &RtRequest) -> RtResponse {
        let code = self.shape_name().to_string();
        let message = match &self {
            Self::RoleRequired(exception) => exception.message.clone(),
            Self::RoleInvalid(exception) => exception.message.clone(),
        }
        .unwrap_or_default();
        // The codec of the protocol THIS request arrived in, so an answer worded
        // here travels in the same envelope the framework's error tail would have
        // used for it, whichever of the service's wire formats the client speaks.
        definition
            .wire_protocol(req)
            .protocol()
            .serialize_error(
                self.as_serializable(),
                self.variant_schema(),
                self.shape_name(),
                self.http_status(),
            )
            .ok()
            .unwrap_or_else(|| {
                definition.refusal_response(
                    ProtocolError::Invalid(Vec::new()),
                    Some(Refused { code, message }),
                    req,
                )
            })
    }
}

/// Read the request into `T` through the protocol and schema the dispatch sequence
/// uses, so this layer reads exactly the value the handler would be given, or `None`
/// for a body it cannot read at all — which is the framework's refusal, not this
/// layer's question.
fn read<T: DeserializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static Schema,
    req: &RtRequest,
) -> Option<T> {
    let mut deserializer = protocol.deserialize_input(req, schema).ok()?;
    T::deserialize(&mut *deserializer).ok()
}

/// Whether the framework's own validating walk would refuse this input, which is the
/// only case this layer has a question about: it is what would otherwise be answered
/// BEFORE the role the operation judges first.
fn boundary_refuses(input: &dyn SerializableStruct, schema: &'static Schema) -> bool {
    !validate(input, schema).is_empty()
}

/// Read one claimed operation's request and name the role question it carries, or
/// `None` when the walk accepts the body (so there is no question) or the body cannot
/// be read at all.
fn addressed_with<T: DeserializableStruct + SerializableStruct>(
    protocol: &dyn ServerHttpProtocol,
    schema: &'static Schema,
    req: &RtRequest,
    members: impl FnOnce(&T) -> Addressed,
) -> Option<Addressed> {
    let input: T = read(protocol, schema, req)?;
    if !boundary_refuses(&input, schema) {
        return None;
    }
    Some(members(&input))
}

/// The role question the operation `key` names carries, for this request.
///
/// The two operations here are this layer's whole scope: they are the ones that SET
/// a deployment group's service role and model the refusals of it. `None` for every
/// other operation, which keeps the generated routing table as its only source. On
/// every wire format this service declares a routing key is the operation's own
/// shape name, so one table of keys serves them all.
fn addressed(key: &str, protocol: &dyn ServerHttpProtocol, req: &RtRequest) -> Option<Addressed> {
    match key {
        CREATE_DEPLOYMENT_GROUP => addressed_with(
            protocol,
            CREATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
            req,
            |input: &CreateDeploymentGroupInput| Addressed {
                role_arn: Some(input.service_role_arn.clone()),
                role_required: true,
                application_name: input.application_name.clone(),
                deployment_group_name: None,
                deployment_config_name: input.deployment_config_name.clone(),
            },
        ),
        UPDATE_DEPLOYMENT_GROUP => addressed_with(
            protocol,
            UPDATE_DEPLOYMENT_GROUP_INPUT_SCHEMA,
            req,
            |input: &UpdateDeploymentGroupInput| Addressed {
                role_arn: input.service_role_arn.clone(),
                role_required: false,
                application_name: input.application_name.clone(),
                deployment_group_name: Some(input.current_deployment_group_name.clone()),
                deployment_config_name: input.deployment_config_name.clone(),
            },
        ),
        _ => None,
    }
}

/// The operation this service serves for a request whose body the walk refuses and
/// whose operation judges the service role first.
///
/// An [`ErasedOperation`] rather than a response built in
/// `Service::override_operation`, because that seam hands the router a BORROWED
/// operation. It holds the shared engine, because the trust half of the question is
/// answered from the account's IAM records and this service's own store.
pub struct RolePrecedence<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> RolePrecedence<Eng> {
    /// Hold the shared engine, the same one every operation of this service is
    /// constructed from — never a store of its own.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> std::fmt::Debug for RolePrecedence<Eng> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RolePrecedence").finish_non_exhaustive()
    }
}

impl<Eng: Send + Sync + 'static> RolePrecedence<Eng> {
    /// Whether this layer has a question about the request `key` names — the
    /// decision `crate::service::EmulatedService::override_operation` makes.
    ///
    /// It is the WHOLE decision: the layer claims a request only when it has an
    /// answer for it, so a request it claims is never handed back to the modelled
    /// operation. [`RolePrecedence::run`] re-derives the answer from the same
    /// request and falls through to that operation if there is none after all.
    pub fn has_a_question(
        &self,
        key: &str,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
    ) -> bool {
        self.answer_for(key, protocol, req).is_some()
    }

    /// The answer this layer owes the request `key` names, or `None` when it owes
    /// none and the modelled operation serves it as it always did.
    fn answer_for(
        &self,
        key: &str,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
    ) -> Option<Answer> {
        self.answer(&addressed(key, protocol, req)?)
    }

    /// The answer this layer owes `addressed`, or `None` when the role earns no
    /// refusal and the modelled operation should serve the request after all.
    fn answer(&self, addressed: &Addressed) -> Option<Answer> {
        let role_arn = addressed.role_arn.as_deref()?;
        // THE VALUE FIRST, decided from the request alone through the crate's one
        // gate for a service role ARN.
        match crate::state::check_service_role_arn(role_arn) {
            Err(crate::state::RoleRefusal::Missing) if addressed.role_required => {
                return Some(Answer::RoleRequired(RoleRequiredException {
                    message: Some(crate::state::ROLE_MISSING.to_string()),
                }));
            }
            Err(refusal) => {
                // A role the request did not give where the operation does not
                // require one is still a REPLACEMENT the group cannot run under —
                // a group always has a service role — so it is the invalid value
                // its handler words it as, and not a member left unset.
                return Some(Answer::RoleInvalid(InvalidRoleException {
                    message: Some(crate::state::invalid_role_message(refusal, role_arn)),
                }));
            }
            Ok(()) => {}
        }
        // THE TRUST POLICY NEXT, and only for a request whose addressed resource
        // this emulator holds: an application — or, for an update, a group — the
        // account does not hold is the earlier answer, and it is the HANDLER that
        // gives it.
        if !self.holds_addressed_resource(addressed) {
            return None;
        }
        let message =
            crate::state::service_role::unassumable_role_message(&*self.engine, role_arn.trim())?;
        Some(Answer::RoleInvalid(InvalidRoleException {
            message: Some(message),
        }))
    }

    /// Whether this service's store holds everything `addressed` names ahead of the
    /// role: the deployment configuration where the request names one, the
    /// application, and the deployment group too where the request addresses one.
    ///
    /// These are the refusals the HANDLER answers before the role, so a request
    /// earning one of them is left to the handler — this layer answers only what
    /// comes after them and before the body. The store is read through
    /// [`crate::state::with_state`], the one way this service reads it, so the
    /// built-in deployment configurations are seeded exactly as they are for the
    /// handler; nothing here writes.
    fn holds_addressed_resource(&self, addressed: &Addressed) -> bool {
        crate::state::with_state(&*self.engine, |state| {
            if addressed
                .deployment_config_name
                .as_deref()
                .is_some_and(|name| !state.deployment_configs.contains_key(name))
            {
                return false;
            }
            let Some(application) = state.applications.get(&addressed.application_name) else {
                return false;
            };
            match addressed.deployment_group_name.as_deref() {
                Some(group) => application.deployment_groups.contains_key(group),
                None => true,
            }
        })
        .unwrap_or(false)
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for RolePrecedence<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The service's model-derived half is read inside this block and not held
            // past it: `&dyn ErasedServiceDefinition` is not `Sync`, and the future
            // returned here is `Send`. Only the modelled operation — which is
            // `Send + Sync` — crosses the await below.
            let served = {
                let definition = svc.definition();
                // The protocol this request arrived in, not the first the model
                // declares: the key, the body and the answer all belong to it.
                let wire = definition.wire_protocol(req);
                let protocol = wire.protocol();
                let key = protocol.routing_key(req).ok();
                // The same decision `override_operation` made, re-derived from the
                // same request, and then the answer it owes. When the role earns no
                // refusal, the modelled operation the routing key names serves the
                // request exactly as it would have.
                match key.and_then(|key| self.answer_for(key, protocol, req)) {
                    Some(answer) => {
                        // An answer this layer gives is MEASURED exactly as the
                        // framework's own dispatch sequence measures a modelled
                        // failure, so a request served here is not invisible to a
                        // collector (`vendor/smithy-server-runtime/src/operation.rs`
                        // records the same row around its own handler call).
                        let recording = ::emulator_telemetry::recording_active();
                        let started = ::std::time::Instant::now();
                        let metadata = recording.then(|| {
                            ::smithy_server_runtime::RequestTelemetry::from_headers(
                                &ctx.parts().headers,
                            )
                            .into_metadata()
                        });
                        let _in_flight = recording
                            .then(::emulator_telemetry::global)
                            .flatten()
                            .map(::emulator_telemetry::TelemetryClient::begin_request);
                        let failure = ::emulator_telemetry::OperationFailure::classify(
                            answer.http_status(),
                            answer.shape_name(),
                        );
                        let response = answer.response(definition, req);
                        if let Some(metadata) = metadata {
                            let handle = ::emulator_telemetry::OperationHandle::folded(
                                ctx.identity().signing_name_static(),
                                key.unwrap_or_default(),
                            );
                            ::emulator_telemetry::record_generated_operation_with_handle(
                                &handle,
                                started.elapsed(),
                                Some(failure),
                                metadata,
                            );
                        }
                        Ok(response)
                    }
                    None => match key.and_then(|key| definition.routing_table().get(key)) {
                        // The modelled operation records its own row, so nothing is
                        // measured here for a request that falls through.
                        Some(operation) => Err(operation),
                        // Unreachable: this layer is only ever handed a request
                        // whose routing key named a modelled operation. It must
                        // not answer the framework's routing failure — that is a
                        // 501 `NotImplemented`, which would report a request this
                        // service DOES serve as an operation nobody wrote — so the
                        // request is refused as the invalid input it was claimed
                        // for.
                        None => Ok(definition.framework_error_response(
                            ProtocolError::Invalid(Vec::new()),
                            req,
                        )),
                    },
                }
            };
            match served {
                Ok(response) => response,
                Err(operation) => operation.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so this
        // layer cannot weaken the schemes the model declares for it.
        None
    }
}
