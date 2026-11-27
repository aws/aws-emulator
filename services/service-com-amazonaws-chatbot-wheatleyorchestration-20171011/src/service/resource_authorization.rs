// SPDX-License-Identifier: CC0-1.0

//! The RESOURCE-SCOPED authorization decision `DisassociateFromConfiguration` takes on the
//! `Resource` it addresses BEFORE the modelled constraints on that member are judged.
//!
//! Both association operations name a resource to link to a channel configuration
//! (`Resource`, target `ResourceIdentifier`, `@pattern` of a `custom-action/` ARN), and both
//! model `InvalidRequestException` (400) for malformed client input and
//! `UnauthorizedException` (403) for credentials that are not valid for the target — but AWS
//! Chatbot does NOT order the two checks the same way on the two operations, and the
//! recorded refusals are what decides the order:
//!
//! * the UNLINK is recorded answering a `Resource` that is not an ARN at all — a value the
//!   member's `@length`/`@pattern` would also reject — with 403 `UnauthorizedException` and
//!   the fixed sentence [`crate::state::UNAUTHORIZED_MESSAGE`], so its resource-scoped
//!   authorization decision is taken first and shadows the member's constraints;
//! * the LINK is recorded answering a `Resource` outside that same `@pattern` with 400
//!   `InvalidRequestException` and this service's fixed boundary sentence, so it keeps the
//!   centralized constraint boundary every other operation of this service is judged at and
//!   needs no gate — its handler takes the resource-scoped decision afterwards, on an ARN
//!   the boundary already found well formed.
//!
//! This seam therefore stands in front of the UNLINK alone. The question it asks is the
//! NAMESPACE question ([`crate::state::is_mintable_custom_action_arn`]: this partition, this
//! service, the empty region a custom action is account-scoped under, this account's id, a
//! `custom-action/` resource). What the ARN says about the action's own NAME is a property of
//! the name rather than of the namespace, so a value the namespace admits keeps the member's
//! modelled constraints and the refusal they earn.
//!
//! The framework judges every modelled constraint BEFORE a handler runs and answers a
//! violation in this service's input-constraint wording (400 `InvalidRequestException`,
//! `crate::service`), so a decision taken inside `handle` cannot come first — `handle` runs
//! after the validator, and the status a pre-dispatch refusal is served on is the
//! protocol's. That is why this is a seam: the gate below answers the authorization
//! question from the SAME reading the handler uses
//! ([`crate::state::is_mintable_custom_action_arn`]) and renders the modelled error with the
//! status the model gives it, then hands every request it does not refuse to the generated
//! deserialize -> validate -> handle walk unchanged — so a malformed `ChatConfiguration`, an
//! absent member and every other constraint keep the refusal they had.

use std::sync::Arc;

use smithy_server_runtime::{BoxFuture, ErasedOperation, Operation, OperationError, Service};

use crate::ops::disassociate_from_configuration::{
    DisassociateFromConfigurationError as DisassociateError,
    DisassociateFromConfigurationErrorModeled as DisassociateModeled,
};
use crate::ops::types::UnauthorizedException;

/// The member the unlink names the resource to unlink in: `Resource`
/// (`smithy_query(query="operation",operation="DisassociateFromConfiguration")`), a
/// top-level member of the input, which this service's `restJson1` wire carries under that
/// name in the request document.
const RESOURCE_MEMBER: &str = "Resource";

/// Serves `DisassociateFromConfiguration` with its resource-scoped authorization decision
/// taken BEFORE the modelled input constraints, then hands an authorized request to the
/// generated walk unchanged.
///
/// It is a pure READ — of the request's own `Resource` and of the ARN form this account
/// mints custom actions under — so it writes nothing and cannot change what a later caller
/// sees, and two identical requests get the identical answer.
///
/// Generic over the engine type for the same reason the rest of this crate is: the
/// operation it stands in front of is, so naming a concrete engine here would let this
/// seam drift from the service the router mounts.
pub struct ResourceAuthorizationGate<Eng> {
    /// The shared engine, so the generated walk this gate delegates to — and the handler
    /// behind it — reads THIS service's store rather than a private one.
    engine: Arc<Eng>,
}

impl<Eng: Send + Sync + 'static> ResourceAuthorizationGate<Eng> {
    /// Build the gate from the shared engine.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }

    /// The routing key the model names this gate's operation by, read off the operation
    /// itself rather than spelled out here.
    pub fn operation_name(&self) -> &'static str {
        <crate::ops::disassociate_from_configuration::DisassociateFromConfiguration<Eng> as Operation>::NAME
    }

    /// The resource-scoped refusal this operation states, in its own error channel: the
    /// modelled `UnauthorizedException` with the one fixed sentence this service words an
    /// authorization failure with.
    fn unauthorized(&self) -> Box<dyn OperationError> {
        Box::new(DisassociateError::Modeled(
            DisassociateModeled::UnauthorizedException(UnauthorizedException {
                message: Some(crate::state::UNAUTHORIZED_MESSAGE.to_string()),
            }),
        ))
    }

    /// IAM's own refusal, in this operation's error channel: the decision belongs to the
    /// layer in front of the service, which is why it travels through the same channel the
    /// handler lifts it into rather than through a modelled variant.
    fn access_denied(&self, denied: crate::scaffolded::AccessDeniedException) -> Box<dyn OperationError> {
        Box::new(DisassociateError::AccessDenied(denied))
    }
}

/// The `Resource` the request names, as it carries it — read from the request document
/// rather than from a deserialized input, because this decision is taken before the input
/// is validated.
///
/// `None` when the request carries no readable document, or no textual `Resource` member:
/// there is then no resource to authorize, and the request keeps whatever the generated
/// walk answers it (the modelled refusal for an absent or non-textual required member).
fn named_resource(req: &aws_smithy_runtime_api::http::Request) -> Option<String> {
    let body = req.body().bytes()?;
    let document: ::serde_json::Value = ::serde_json::from_slice(body).ok()?;
    document
        .get(RESOURCE_MEMBER)?
        .as_str()
        .map(ToString::to_string)
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ResourceAuthorizationGate<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            let refusal = named_resource(req)
                .filter(|resource| !crate::state::is_mintable_custom_action_arn(resource))
                .map(|resource| {
                    // IAM is asked first, exactly as the handler asks it, so a principal
                    // the account's policies do not allow this action for is told that
                    // rather than told about the target.
                    match crate::service::authorization::authorize(
                        &*self.engine,
                        ctx,
                        self.operation_name(),
                        &resource,
                    ) {
                        Err(denied) => self.access_denied(denied),
                        Ok(()) => self.unauthorized(),
                    }
                });
            if let Some(refusal) = refusal {
                return svc
                    .definition()
                    .protocol()
                    .serialize_error(
                        refusal.as_serializable(),
                        refusal.variant_schema(),
                        refusal.shape_name(),
                        refusal.http_status(),
                    )
                    .unwrap_or_else(|error| {
                        svc.definition().framework_error_response(error, req)
                    });
            }
            // Authorized for the target it named: run the generated deserialize ->
            // validate -> handle -> serialize walk. Reached through the routing table
            // rather than through `override_operation`, which would route straight back
            // here.
            match svc.definition().routing_table().get(self.operation_name()) {
                Some(op) => op.run(svc, req, ctx).await,
                None => svc.definition().routing_error_response(req),
            }
        })
    }
}
