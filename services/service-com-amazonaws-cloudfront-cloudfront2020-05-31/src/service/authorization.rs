// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! resource, so a caller whose identity-based policies do not grant the action on the ARN
//! the request addressed is told it is not authorized rather than told anything about that
//! resource, and the call performs nothing. The ORDER is part of the rule: a caller that
//! holds no grant and names a resource the account does not hold reads the authorization
//! refusal, not "no such resource" — otherwise a refused call reports whether a resource
//! exists to a caller that may not look.
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
//!    effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the split every other
//! gate in this tree makes, and the lookups stay in the service crate for the reason those
//! keep theirs there: hoisting them into `service-common` would make every service that
//! depends on that crate link the IAM and STS service crates.
//!
//! The ACTION is this service's namespace and the operation's own name
//! ([`action_of`]), and the RESOURCE is the ARN the request addressed — the distribution
//! or the distribution tenant whose invalidations the call is about
//! ([`crate::state::distribution_arn`] / [`crate::state::distribution_tenant_arn`]) — so a
//! policy scoped to one distribution's ARN permits that distribution's invalidations and no
//! other's.
//!
//! A CloudFront distribution carries no resource-based policy of its own, so the decision
//! here is the identity-based one alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to the account's own identity, which is not a
//!   principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed. An emulated service calling this one
//!   through the in-process callback signs with that credential, so this service's own
//!   internal calls are never refused for want of a grant.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no
//! principal spelling, so every action it asks for is refused. (A session the
//! authenticating layer has already refused never reaches a handler at all.)

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDenied;

/// This service's IAM action prefix: an action is `cloudfront:<OperationName>` ("Actions
/// defined by Amazon CloudFront", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string every
/// ARN this service mints carries ([`crate::state::distribution_arn`]).
pub const ACTION_PREFIX: &str = "cloudfront";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The IAM action `operation` performs: this service's namespace and the operation's own
/// name ([`smithy_server_runtime::Operation::NAME`]), which is how IAM publishes the
/// actions of a service.
#[must_use]
pub fn action_of(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise, which each operation returns as its
/// own `AccessDenied`. Call it BEFORE the store is read or written, and with the ARN built
/// from what the request carried rather than one resolved from a stored record, so the
/// decision does not depend on whether that resource exists.
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
) -> Result<(), AccessDenied> {
    let action = action_of(operation);
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and a distribution publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // The decision is taken FIRST, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated) || caller.is_unbounded();
    if !granted {
        return Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
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
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, resource, &reason));
    }
    Ok(())
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it — the ONE wording of that sentence
/// ([`::service_common::iam_authorization::not_authorized_by_identity_policy`]), so this
/// service's refusal reads as every other service's does. Every value in it comes from the
/// request and from the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> AccessDenied {
    AccessDenied {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
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
    fn is_unbounded(&self) -> bool {
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
        store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
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
    if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            let role_arn = held.arn.clone();
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The principal a request speaks for is read through the tree's ONE reading of an identity
/// ARN ([`::service_common::iam_role`]): `assumed_role_session_role_name` for the role behind
/// an STS session and `user_name` for an IAM user, both of which also settle that the ARN is
/// partition-global and of THIS account, so a value naming another account's identity
/// resolves to no principal of this one.
#[cfg(test)]
mod tests {
    use super::*;

    /// RULE: the action this gate asks about is `cloudfront:<OperationName>`, so a policy
    /// scoped to this service's actions covers every operation of it and nothing outside
    /// it, and a policy naming one operation covers that operation alone.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"cloudfront:*","Resource":"*"}]}"#;
        for operation in [
            "CreateInvalidation",
            "GetInvalidation",
            "ListInvalidations",
            "CreateInvalidationForDistributionTenant",
        ] {
            let action = action_of(operation);
            let reached = iam::identity_decision(
                &[document.to_string()],
                &iam::Request::new(&action, "*", &[]),
            );
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected cloudfront:* to allow {action}, got {reached:?}"
            );
        }
        let outside = iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new("s3:GetObject", "*", &[]),
        );
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected cloudfront:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies, in IAM's own wording (spelled out here as the LITERAL AWS
    /// states rather than through the shared builder, so a change to that wording fails
    /// this).
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/invalidator/session-1";
        let resource = crate::state::distribution_arn("E1EXAMPLE");
        let action = action_of("CreateInvalidation");
        let reason = iam::no_identity_policy_allows(&action);
        let refusal = not_authorized(principal, &action, &resource, &reason);
        let message = refusal.message.unwrap_or_default();
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: cloudfront:CreateInvalidation \
                 on resource: {resource} because no identity-based policy allows the \
                 cloudfront:CreateInvalidation action"
            ),
            "case \"a refused invalidation\": expected IAM's wording, got {message:?}"
        );
    }
}
