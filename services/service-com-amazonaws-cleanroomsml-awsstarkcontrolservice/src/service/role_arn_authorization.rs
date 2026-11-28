// SPDX-License-Identifier: CC0-1.0

//! The role-ARN authorization this service takes BEFORE it validates a request.
//!
//! ## The rule this module implements
//!
//! Clean Rooms ML authorizes a call that names the IAM role it must act as AGAINST that role,
//! and it does so ahead of request validation: with no role in the request there is no
//! principal whose policies could allow the call, so the absent member surfaces as an
//! authorization failure — the modelled `AccessDeniedException`, HTTP 403, naming the member
//! the role was expected at and the IAM action the call asks for — and not as the
//! `ValidationException` (HTTP 400) a member outside its modelled constraints produces.
//!
//! The class is the operations whose input declares a `@required` role ARN that this service
//! takes on ([`ROLE_MEMBERS`]); the decision is the same for each, and the action named is
//! that operation's own.
//!
//! ## Why it is an operation rather than a refusal wording
//!
//! The refusal has to come BEFORE the framework's modelled-constraint walk, which has already
//! chosen 400 by the time a service can reword it ([`crate::service`]'s `override_refusal`
//! chooses the code and the message, and the protocol owns the status).
//! `Service::override_operation` is the seam that sees a whole request first, so the denial is
//! an [`ErasedOperation`] that answers that one request. The router still authenticates the
//! request against the modelled operation's auth schemes before running it. The 403 itself is
//! rendered by the one renderer every pre-validation gate of this service answers through
//! ([`super::resource_authorization::access_denied`]).

use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, ServerHttpProtocol, Service,
};

// The wire-level half of a denial is shared by both pre-validation gates of this service:
// the renderer the 403 goes out through, the fallback for a request no refusal can be read
// out of, and the namespace the read-only schemas below name themselves in.
use super::resource_authorization::{
    access_denied, insufficient_access, AUTHORIZATION_READ_NAMESPACE,
};

/// The member every operation below expects the role ARN at, as the wire spells it: the one
/// spelling, held with the rest of this service's role-resolution vocabulary
/// ([`crate::state::role_authorization`]).
const ROLE_ARN_MEMBER: &str = crate::state::role_authorization::ROLE_ARN_MEMBER;

/// The operations whose input declares a `@required` `IamRoleArn` member, each with the
/// one-member body schema that member is read through ([`ROLE_ARN_REQUEST`]).
///
/// These are exactly the operations of this service whose input carries such a member
/// (`smithy_query(query="operation",operation="…")`): the two creates that store a resource
/// Clean Rooms ML later acts on under the supplied role. The table is keyed by OPERATION, so
/// an operation that takes no role never has its body read here, and a future member of
/// another operation that happens to be spelled the same way is not judged by this gate until
/// it is listed.
const ROLE_MEMBERS: [(&str, &Schema); 2] = [
    ("CreateTrainingDataset", &ROLE_ARN_REQUEST),
    ("CreateConfiguredModelAlgorithm", &ROLE_ARN_REQUEST),
];

/// The `roleArn` member of a request, typed as the string it is on the wire.
///
/// The schema stands for the READ this gate performs, not for a shape of the model — the
/// model's own input shapes carry every other member of their operation, which this read
/// deliberately does not look at — so it is named after the gate.
static ROLE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "emulator.authorization#RoleArnRequest$roleArn",
        AUTHORIZATION_READ_NAMESPACE,
        ROLE_ARN_MEMBER,
    ),
    ShapeType::String,
    ROLE_ARN_MEMBER,
    0,
);

/// The body schema a request is read through: a structure declaring THAT ONE member.
///
/// Authorization is taken BEFORE the request is parsed as its operation's input, so the read
/// that feeds it is not the modelled input shape: a codec skips every member a schema does not
/// declare, so this reads whether the role was supplied and nothing else, and a body that also
/// misses another required member, or carries one outside its constraints, is still judged on
/// the role it named.
static ROLE_ARN_REQUEST: Schema = Schema::new_struct(
    ShapeId::from_static(
        "emulator.authorization#RoleArnRequest",
        AUTHORIZATION_READ_NAMESPACE,
        "RoleArnRequest",
    ),
    ShapeType::Structure,
    &[&ROLE_ARN],
);

/// Answers one request `AccessDeniedException` (HTTP 403), with the denial built from that
/// request.
///
/// Holds nothing: both variable parts of the sentence — the member the role was expected at
/// and the action the call asks for — are read off the request and this service's own
/// identity, so there is no state to carry.
pub(crate) struct DenyAbsentRoleArn;

impl ErasedOperation for DenyAbsentRoleArn {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        // Nothing about this refusal is read from the request context: it is decided from the
        // request's own operation and body.
        let _ = ctx;
        // The answer is computed in one call that reads, uses and drops the service
        // definition: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the
        // await below would make this future non-`Send`.
        let response = refusal(svc, req);
        Box::pin(async move { response })
    }
}

/// The 403 for `req`, rendered through this service's own protocol — the same envelope
/// (`__type` + `message`, and the same error-type header) a handler-raised
/// `AccessDeniedException` goes out in, so a refusal taken here and one taken in a handler are
/// indistinguishable on the wire.
fn refusal(
    svc: &dyn Service,
    req: &aws_smithy_runtime_api::http::Request,
) -> aws_smithy_runtime_api::http::Response {
    let definition = svc.definition();
    // The protocol THIS request is served in: the request is read, and the denial answered,
    // in the one codec the request arrived in.
    let Some((operation, member)) = absent_role_arn(definition.wire_protocol(req).protocol(), req)
    else {
        // Unreachable: this operation is reached only for a request whose operation and
        // absent role member `Service::override_operation` has already read out of these same
        // bytes. A request this cannot read names no action, so the refusal names none
        // either, and falls back to the error's own sentence.
        return insufficient_access(svc, req);
    };
    let message = crate::state::role_authorization::role_arn_not_present_message(
        member,
        &crate::state::role_authorization::iam_action(
            definition.identity().signing_name(),
            operation,
        ),
    );
    access_denied(svc, req, message)
}

/// The operation `req` names and the member its role was expected at, when `req` is one of
/// [`ROLE_MEMBERS`] and carries no role at all — the one read both the dispatch gate and the
/// refusal it answers with are taken from.
///
/// `None` — leave the request to the modelled walk — for an operation that takes no role, for
/// a request that DOES name one, and for a body this cannot read at all (one that is not the
/// serving protocol's object, or whose role member is not a string): then nothing has been
/// established about the role, and the modelled walk refuses the request on its own terms.
///
/// `protocol` is the codec the request is SERVED in, so the read is the one the rest of the
/// request uses: an authorization decision is taken from what a request states, never from the
/// encoding it stated it in.
pub(crate) fn absent_role_arn(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<(&'static str, &'static str)> {
    let routing_key = protocol.routing_key(req).ok()?;
    let (operation, request_schema) = ROLE_MEMBERS
        .iter()
        .find(|(candidate, _)| *candidate == routing_key)?;
    supplied_role_arn(protocol, req, request_schema)?
        .is_none()
        .then_some((*operation, ROLE_ARN_MEMBER))
}

/// What `req` supplies for the role member, read through the codec the request is SERVED in
/// and against `request_schema`'s one member.
///
/// The outer `None` says the body could not be read at all, which establishes nothing; the
/// inner `None` says the body was read and carried no such member.
fn supplied_role_arn(
    protocol: &dyn ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    request_schema: &Schema,
) -> Option<Option<String>> {
    let mut reader = protocol.deserialize_input(req, request_schema).ok()?;
    let mut supplied = None;
    reader
        .read_struct(request_schema, &mut |member, value| {
            supplied = Some(value.read_string(member)?);
            Ok(())
        })
        .ok()?;
    Some(supplied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::role_authorization::{iam_action, role_arn_not_present_message};

    /// The sentence names the member the role was expected at and the action the call asks
    /// for, and nothing else. The rows are the operations of this service that take a role,
    /// so the action half is read off the request rather than fixed to one call.
    #[test]
    fn the_refusal_names_the_member_and_the_action() {
        for (operation, _) in ROLE_MEMBERS {
            let action = iam_action("cleanrooms-ml", operation);
            let stated = role_arn_not_present_message(ROLE_ARN_MEMBER, &action);
            let expected = format!(
                "Role Arn not present in request. Please provide a valid role arn in the \
                 path: roleArn. because no resource-based policy allows the \
                 cleanrooms-ml:{operation} action"
            );
            assert_eq!(
                stated, expected,
                "case {operation:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}
