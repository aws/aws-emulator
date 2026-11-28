// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request names, so a caller whose identity-based policies do not grant the action — or
//! that an explicit `Deny` covers — is told it is not authorized rather than told anything
//! about the resource, and the call performs nothing: no resource is minted, no status
//! moves, no quota is consumed. This service models that refusal as `AccessDeniedException`
//! (`@httpError(403)`), which 132 of this service's 133 operations declare
//! (`smithy_query(query="operation",operation="CreateGuardrail,DeleteResourcePolicy,CreateModelInvocationJob")`,
//! and every other one but the single exception [`UNMODELLED_REFUSAL_OPERATIONS`] names);
//! an operation whose model declares no 403 is NOT judged here, because answering a caller
//! an error its operation does not model would be a different divergence.
//!
//! The decision is therefore taken for EVERY operation of this service rather than for the
//! few whose handler asks for it: it is a property of the request's identity, the action it
//! names and the resource it addresses rather than of what the handler goes on to do, so it
//! is taken ONCE, in the layer every operation is served behind
//! (`crate::service::EmulatedService::override_operation`), and nowhere else.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for (`sts_store::resolve_caller`, the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    owns the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's OWN accessors (`principal_policies`, `permissions_boundary_document`),
//!    where the collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes
//!    effect on the next request, deleting it lifts it again, and this service reads an
//!    identity's permissions exactly as IAM's own authorizer does;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! This module is a DIRECTORY: the three pieces above stay in this file, and the two parts
//! that are tables rather than decisions live beside it — WHICH RESOURCE a request
//! addressed, with the table of this model's URI spaces that answers it
//! ([`judged_resource`], re-exported here, so every name is still reached as
//! `authorization::…`), and the cases for both ([`tests`]). What this file does is take the
//! decision; nothing in either part serves a response or reads a credential.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes (AWS Certificate Manager, Amazon OpenSearch Service,
//! Billing and Cost Management Data Exports, Amazon EVS, CloudWatch RUM, Amazon Cognito
//! Identity), and the lookups stay in the service crate for the reason those keep theirs
//! there: hoisting them into `service-common` would make every service that depends on
//! that crate link the IAM and STS service crates.
//!
//! Amazon Bedrock's own resource policies (`PutResourcePolicy`) are attached to shapes no
//! operation gated here acts on, so the decision here is the identity-based one alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant. An attachment naming a
//!   CUSTOMER-managed policy the account does not hold grants nothing, because the account
//!   is the authority on its own policies.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use ::aws_smithy_types::body::SdkBody;
use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::ServerHttpProtocol;

use crate::ops::types::AccessDeniedException;

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request.
pub const ANY_RESOURCE: &str = "*";

/// The wire name of the refusal this gate answers with, and the status the model gives it:
/// `AccessDeniedException`, `@error("client")` at `@httpError(403)`
/// (`smithy_query(query="shape",shape="AccessDeniedException")`).
pub const ACCESS_DENIED_EXCEPTION: &str = "AccessDeniedException";
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type this restJson1 service renders an error in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The ONE operation of this service whose model declares no `AccessDeniedException`:
/// `GetUseCaseForModelAccess`, whose errors are `ResourceNotFoundException`,
/// `ThrottlingException`, `ValidationException` and `InternalServerException`
/// (`smithy_query(query="operation",operation="GetUseCaseForModelAccess")`).
///
/// A caller of it is left unjudged rather than answered an error its operation does not
/// model: that would be a different divergence from the one this gate removes.
pub const UNMODELLED_REFUSAL_OPERATIONS: &[&str] = &["GetUseCaseForModelAccess"];

/// Whether the layer every operation is served behind judges `operation` itself.
#[must_use]
pub fn layer_judges(operation: &str) -> bool {
    !UNMODELLED_REFUSAL_OPERATIONS.contains(&operation)
}

/// Whether `value` is an ARN of THIS service's own namespace — the resource a request
/// carrying it addresses.
///
/// The parse is the crate's one ARN parse (`service_common::arn`), with the vendor field
/// compared against [`crate::state::ARN_NAMESPACE`], so no two services disagree about
/// which ARNs are theirs.
#[must_use]
pub fn names_a_resource_of_this_service(value: &str) -> bool {
    ::service_common::arn::parse_of(value, crate::state::ARN_NAMESPACE).is_some()
}

/// The wire answer to a request this gate refused: the modelled `AccessDeniedException` at
/// its modelled 403, serialized through the SAME protocol a handler-returned error of that
/// shape goes through, so a refusal taken in the layer and one taken in a handler are
/// indistinguishable on the wire.
///
/// The hand-built fallback carries the same three things a client branches on — the status,
/// the shape in `x-amzn-errortype` and the `__type` body — for the one case the serializer
/// can fail: a value it cannot render.
///
/// CROSS-REFERENCE: `service_common::event_stream::error_response` renders a modelled error
/// the same way, for an operation that holds its error as a `dyn OperationError`. This gate
/// holds the error shape itself rather than an operation's error channel (the layer stands
/// in front of 132 of them), and its fallback keeps the refusal's own status and code where
/// that one answers a 500 — so the two are not one renderer today. Hoisting a renderer that
/// takes the shape, the schema, the code and the status is the change that would merge them,
/// and it belongs to whichever change needs it in a second service.
#[must_use]
pub fn refusal_response(
    protocol: &dyn ServerHttpProtocol,
    denial: &AccessDeniedException,
) -> RtResponse {
    protocol
        .serialize_error(
            denial,
            &crate::ops::types::SCHEMA_ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_EXCEPTION,
            ACCESS_DENIED_STATUS,
        )
        .unwrap_or_else(|_| {
            let body = ::serde_json::json!({
                "__type": ACCESS_DENIED_EXCEPTION,
                "message": denial.message.clone().unwrap_or_default(),
            });
            let mut response = RtResponse::new(
                StatusCode::try_from(ACCESS_DENIED_STATUS)
                    .expect("the status is a constant of this module"),
                SdkBody::from(body.to_string()),
            );
            response
                .headers_mut()
                .insert("content-type", JSON_CONTENT_TYPE);
            response
                .headers_mut()
                .insert("x-amzn-errortype", ACCESS_DENIED_EXCEPTION);
            response
        })
}

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `bedrock:<OperationName>` ("Actions
/// defined by Amazon Bedrock", IAM Service Authorization Reference), which is this
/// service's `endpointPrefix` / `sigv4Name` (`smithy_query(query="service_info")`) — the
/// same string [`crate::state::ARN_NAMESPACE`] spells into every ARN this service builds.
pub const ACTION_PREFIX: &str = crate::state::ARN_NAMESPACE;

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation returns as its
/// own `AccessDeniedException`. Call it BEFORE the store is read or written, and with the
/// ARN the request carried rather than one resolved from a stored record, so the decision
/// does not depend on whether that resource exists — which is also what keeps a refused
/// call from telling an unauthorized caller whether it does.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises
/// the session expiries the clock has already made due; that write is owed at the same
/// instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    authorize_action(engine, ctx, &format!("{ACTION_PREFIX}:{operation}"), resource)
}

/// `iam:PassRole`, the IAM action a principal has to hold to hand a service a role that
/// service will then take on ("Granting a user permissions to pass a role to an AWS
/// service", IAM User Guide).
///
/// It is an action of IAM rather than of this service, so it is not spelled with
/// [`ACTION_PREFIX`]: what IAM evaluates is `iam:PassRole` on the ROLE being passed.
pub const IAM_PASS_ROLE: &str = "iam:PassRole";

/// Whether the IAM boundary in force on this caller WITHHOLDS the `iam:PassRole` of
/// `role_arn` — the account's pass-role boundary over the role a request asks this service
/// to take on.
///
/// Handing a service a role is a permission of the CALLER rather than a property of the
/// role: the principal making the call has to be allowed `iam:PassRole` for that very role,
/// and the account bounds which of its roles each of its principals may hand out, so two
/// accounts can answer the same well-formed, same-account, correctly-trusted destination
/// differently. The decision is the identity-based one every action of this service is
/// judged by ([`authorize`]) asked for the IAM action on the role's own ARN, so a policy
/// `iam:PutRolePolicy` wrote takes effect on the next request and removing it lifts the
/// refusal again.
///
/// `false` — withhold nothing — for every caller this gate does not bound (the module
/// docs: the account's own published credential, a principal holding a grant this emulator
/// cannot read, an engine with no IAM store), because a refusal has to be grounded in a
/// document that was READ.
#[must_use]
pub fn pass_role_is_withheld<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> bool {
    authorize_action(engine, ctx, IAM_PASS_ROLE, role_arn).is_err()
}

/// The identity-based decision [`authorize`] and [`pass_role_is_withheld`] share, taken for
/// the IAM `action` spelled whole against the `resource` it names.
fn authorize_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, so the decision below rests on the action and the resource.
    let request = iam::Request::new(action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            action,
            resource,
            "with an explicit deny in an identity-based policy",
        ));
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(not_authorized(&caller.arn, action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's rules refused it. Every value in it comes from
/// the request and from the stored policies — the principal from the credential the
/// request presented, the action from the operation it named, the resource from the ARN it
/// addressed.
///
/// An action that addresses no resource of its own is judged against [`ANY_RESOURCE`], and
/// the sentence states no resource clause for it: there is nothing to name.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    let on_resource = if resource == ANY_RESOURCE {
        String::new()
    } else {
        format!(" on resource: {resource}")
    };
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
        )),
    }
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds
    /// no IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not
    /// hold — an AWS-managed policy (see the module docs).
    unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no
    /// IAM policies, or a principal carrying a grant this emulator cannot read.
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    ///
    /// `principal_policies` is where the collection rule itself lives — a role's or user's
    /// inline policies, the default version of each managed policy attached to it, and for
    /// a user the policies of the groups it belongs to — so this service reads an
    /// identity's permissions exactly as IAM's own authorizer does.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's
            // own catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The principal this request speaks for, with the identity-based policies in force on it.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let identity = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The
        // key id it presented is NOT quoted back — this module echoes no value of the
        // request — and in the assembled emulator the layer that authenticates a request
        // refuses an expired credential before any handler runs.
        Err(expired) => {
            let _ = expired;
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                principals: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the
    // user's policies rather than served as the account. A key the IAM store does not hold
    // either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = assumed_role_name(&caller.arn) {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            caller.read_policies_of(&store, &held.arn.clone());
        }
    } else if iam_user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The role name an STS assumed-role session ARN names.
///
/// `sts:AssumeRole` reports the session as
/// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, so the role
/// is the first segment after `assumed-role/`.
fn assumed_role_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "sts")?;
    let name = resource.strip_prefix("assumed-role/")?.split('/').next()?;
    (!name.is_empty()).then_some(name)
}

/// The user name an IAM user ARN names: the LAST segment of `user/<path…>/<name>`, since
/// the path is optional and only the last segment is the name.
fn iam_user_name(arn: &str) -> Option<&str> {
    let resource = arn_resource(arn, "iam")?;
    let name = resource.strip_prefix("user/")?.split('/').next_back()?;
    (!name.is_empty()).then_some(name)
}

/// The resource part of an ARN whose service segment is `service`.
fn arn_resource<'a>(arn: &'a str, service: &str) -> Option<&'a str> {
    let mut fields = arn.splitn(6, ':');
    let (Some("arn"), Some(_partition), Some(named), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return None;
    };
    (named == service).then_some(resource)
}

/// WHICH RESOURCE a request addressed, which is the other half of the decision this gate
/// takes: the table of this model's URI spaces and the readers that place a request target
/// in it. Re-exported, so [`judged_resource`] and the ARNs it builds are reached as
/// `authorization::…` exactly as before.
mod judged_resource;
pub use judged_resource::*;

/// The cases for both halves, which need no built service.
#[cfg(test)]
mod tests;
