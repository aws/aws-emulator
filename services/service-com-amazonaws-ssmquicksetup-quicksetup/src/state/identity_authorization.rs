// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this service
//! perform the action it asks for — the decision every operation takes before it reads or
//! writes anything.
//!
//! AWS decides a signed call at the IAM layer, ahead of the service looking at the
//! resource the request names, so a principal whose policies refuse the action is told it
//! is not authorized rather than told anything about that resource.
//! `AccessDeniedException` is a SERVICE-COMMON error of this service's model
//! (`smithy_query(query="digest")` lists it under `common_errors`, HTTP 403), so it is the
//! answer every one of the fourteen operations gives such a caller — and the status and
//! code are what lets a client tell a MISSING PERMISSION from a resource that resolves to
//! nothing (the modelled `ResourceNotFoundException`, HTTP 404) or from input it may not
//! send (`ValidationException`, HTTP 400).
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_caller`]) for a
//!    session credential, and from IAM's own `access_keys` map (the map `iam:GetUser`
//!    resolves a caller through) for a long-term key;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), so a policy
//!    `iam:PutRolePolicy` wrote takes effect on the next call and deleting it lifts it
//!    again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, and the SENTENCE a
//!    refusal carries, which is that module's
//!    [`::service_common::iam_authorization::not_authorized_by_identity_policy`] and the
//!    reason clauses beside it, so nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle. WHICH IAM entity a principal ARN names carries no such dependency, so it is read
//! through the shared [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `ssm-quicksetup:<OperationName>`: this service's actions are published
//! under its `arnNamespace`, which is also its SigV4 signing name and the namespace every
//! ARN it mints carries ([`super::ARN_SERVICE`], `smithy_query(query="service_info")`).
//!
//! The resource is the ARN the REQUEST named — the configuration manager a route labels,
//! or the taggable resource a tag route labels — taken from the request rather than from a
//! resolved record, so the decision is the same whether or not that ARN names anything,
//! which is what keeps a refusal from telling an unauthorized caller what the account
//! holds. An operation whose own input names no ARN at all (the creates, the lists, the
//! service settings, and `GetConfiguration`, whose label is a configuration id rather than
//! an ARN) is judged against [`ANY_RESOURCE`], IAM's own spelling of "no particular
//! resource".
//!
//! ## The two callers the decision cannot refuse
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], which is not
//!   a principal the IAM store holds policies for — the position AWS account root
//!   credentials are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to the account, so the IAM store holds no document
//!   for it; refusing on a grant this emulator cannot see would deny access AWS permits.
//!   An explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//!
//! An expired session credential speaks for nobody: it carries no document, so every
//! action it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// The resource a call that names no single one is judged against, spelled with IAM's own
/// wildcard.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The IAM action `operation` is performed under: `ssm-quicksetup:<OperationName>`, this
/// service's own action namespace (see the module docs).
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{}:{operation}", super::ARN_SERVICE)
}

/// The refusal IAM states for this request, or `None` when it permits it.
///
/// `operation` is the operation's own name ([`smithy_server_runtime::Operation::NAME`]) and
/// `resource` the ARN the request NAMED, or [`ANY_RESOURCE`] for an operation that names
/// none. Called as the FIRST thing a handler does, with no store guard of this service
/// held: the decision is taken before the request's own input is judged and before any
/// record is read or written, so a refused call leaves the store exactly as it was.
#[must_use]
pub fn access_denied<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<AccessDeniedException> {
    refusal(engine, ctx, operation, resource).map(|message| AccessDeniedException {
        message: Some(message),
    })
}

/// The sentence [`access_denied`] carries, or `None` when IAM allows the request.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this
/// gate does not bound has by definition (see the module docs); and a grant is capped by
/// the permissions boundary, since "a permissions boundary sets the maximum permissions
/// that an identity-based policy can grant" (IAM, permissions boundaries).
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(&action, resource, &[]);
    let said = iam::identity_decision(&caller.documents, &request);
    if said == Decision::Denied {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Some(iam::not_authorized_by_identity_policy(
            &caller.arn,
            &action,
            resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    let boundary = caller.boundary.as_ref()?;
    let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => return None,
        // An explicit deny is a fact about the boundary document, so the refusal names it;
        // a boundary that merely states nothing withheld the grant the policies gave.
        Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
        Decision::NotStated => iam::no_permissions_boundary_allows(&action),
    };
    Some(iam::not_authorized_by_identity_policy(
        &caller.arn,
        &action,
        resource,
        &reason,
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
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
    /// OWN accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
    fn read_policies_of(
        &mut self,
        store: &super::local_administration_role::IamState,
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
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due before it answers: that write is owed
/// at the same instant whether or not anybody called, which is why a REPORTING operation
/// of this service may take this decision too.
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
        // account's own published credential, which holds no identity policy.
        None => Ok(sts_store::ambient_identity()),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal names the credential's STATE rather
    // than the key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            };
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = super::local_administration_role::iam_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to
    // the user that owns it (the map `iam:GetUser` resolves a caller through), so a request
    // signed with one speaks for THAT user and is judged on the user's policies. A key the
    // IAM store does not hold either really is the account's own published credential.
    if caller.account_credential {
        if let Some(user) = store
            .access_keys
            .get(&access_key_id)
            .and_then(|record| store.users.get(&record.user_key))
        {
            caller.account_credential = false;
            caller.arn = user.arn.clone();
        }
    }
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the session
        // ARN does not.
        if let Some(role_arn) = store
            .roles
            .get(&iam_store::name_key(role_name))
            .map(|held| held.arn.clone())
        {
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        // An IAM user's permissions are its own, under the ARN it was resolved by.
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `ssm-quicksetup:<OperationName>`, so a
    /// document scoped to this service's actions decides every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"ssm-quicksetup:*","Resource":"*"}]}"#;
        for operation in [
            "CreateConfigurationManager",
            "GetConfigurationManager",
            "ListConfigurationManagers",
            "UpdateConfigurationDefinition",
            "ListTagsForResource",
            "UpdateServiceSettings",
        ] {
            let action = action_for(operation);
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected ssm-quicksetup:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "ssm:GetParameter", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected ssm-quicksetup:* to state nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: an explicit `Deny` about the request wins over every `Allow`, and it refuses
    /// only the action it names.
    #[test]
    fn an_explicit_deny_wins_over_a_wildcard_grant() {
        let document = r#"{"Version":"2012-10-17","Statement":[
            {"Effect":"Allow","Action":"ssm-quicksetup:*","Resource":"*"},
            {"Effect":"Deny","Action":"ssm-quicksetup:GetConfigurationManager",
             "Resource":"*"}]}"#;
        let manager = super::super::manager_arn(
            "us-west-2",
            "11111111-2222-3333-4444-555555555555",
        );
        let denied = decision(document, &action_for("GetConfigurationManager"), &manager);
        assert_eq!(
            denied,
            Decision::Denied,
            "case \"the denied action\": expected Denied, got {denied:?}"
        );
        let allowed = decision(
            document,
            &action_for("ListConfigurationManagers"),
            ANY_RESOURCE,
        );
        assert_eq!(
            allowed,
            Decision::Allowed,
            "case \"an action the deny does not name\": expected Allowed, got {allowed:?}"
        );
    }
}
