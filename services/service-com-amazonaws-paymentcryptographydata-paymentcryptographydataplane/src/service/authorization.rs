// SPDX-License-Identifier: CC0-1.0

//! Whether the CALLER's own identity-based IAM policies let a request to this data plane
//! perform the action it asks for — the decision every implemented operation takes BEFORE
//! it resolves the key the request names.
//!
//! `AccessDeniedException` (HTTP 403, "You do not have sufficient access to perform this
//! action.") is a service-common error of this model
//! (`smithy_query(query="service_info")` lists it under `common_errors`), so it is the
//! answer every operation here gives a caller whose permissions do not cover the action it
//! named. AWS takes that decision at the IAM layer, ahead of the service reading anything,
//! so the refusal says nothing about the key the request addressed — not whether it
//! exists, what algorithm it carries or which modes of use it was created with. That is
//! what keeps a MISSING GRANT distinguishable from this service's own verdicts: the first
//! is HTTP 403 `AccessDeniedException`, the second one of the modelled refusals
//! (`ResourceNotFoundException`, `ValidationException`, `VerificationFailedException`).
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_caller`], the same record
//!    `sts:GetCallerIdentity` answers from), else the IAM user that owns the long-term
//!    key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through the IAM
//!    service's own accessors (`principal_policies`, `permissions_boundary_document`), so
//!    a document `iam:PutRolePolicy` wrote takes effect on the next request and deleting
//!    it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision, nor the wording of a refusal,
//! nor the reading of a principal ARN does. The lookups stay in this crate because the IAM
//! service crate itself depends on `service-common`, so the shared crate cannot read IAM's
//! store without a dependency cycle.
//!
//! ## The action and the resource
//!
//! The action is `payment-cryptography:<OperationName>`: this endpoint's actions are
//! published under the signing name its requests are signed with
//! (`aws.auth#sigv4` `name`, `payment-cryptography` — `smithy_query(query="service_info")`),
//! which is read off the request's own resolved identity rather than restated here, so a
//! policy written for `payment-cryptography:*` covers these calls and nothing outside the
//! namespace does.
//!
//! The resource an IAM statement names for one of these calls is the KEY's ARN, the ARN
//! form this namespace publishes for a Payment Cryptography key, so a request that
//! addresses its key by alias is decided against the ARN the CONTROL plane holds that key
//! under ([`key_resource`]) — the same ARN a statement scoped to that key states. An
//! identifier the control plane does not hold is decided AS CARRIED: there is no ARN to map
//! it to, so no statement scoped to a key ARN covers it, while a statement over every
//! resource still does — and the caller learns nothing from the verdict about whether that
//! key exists, since the decision is taken before any handler resolves it.
//!
//! This service publishes no resource-based policy of its own — neither plane models a key
//! policy operation or a policy member — so the decision here is the identity-based one
//! alone.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to the account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no user can exist.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::state as key_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This endpoint's IAM action namespace, which is the name every request to it is signed
/// under — read off the request's own resolved identity, so the actions this gate asks
/// about cannot name a different service from the one that served the request.
fn action_prefix<'ctx>(ctx: &'ctx ::smithy_server_runtime::OperationContext<'_>) -> &'ctx str {
    ctx.identity().signing_name()
}

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `key_identifier` the `KeyIdentifier` it
/// addressed.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled `AccessDeniedException` otherwise, which each operation
/// lifts into its own error channel. Call it FIRST in a handler, before the key is resolved
/// and before anything else about the request is judged.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a reporting operation of this service may
/// take this decision too.
///
/// Asked with NO store guard of this service held.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    key_identifier: &str,
) -> Result<(), AccessDeniedException> {
    let action = format!("{}:{operation}", action_prefix(ctx));
    let resource = key_resource(engine, key_identifier);
    let caller = caller(engine, ctx);
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element would be matched against; an identity-based document states no
    // such element, and this service publishes no resource-based policy (see the module
    // docs), so the decision below rests on the action and the resource.
    let request = iam::Request::new(&action, &resource, &caller.principals);
    let identity_said = iam::identity_decision(&caller.documents, &request);
    // Taken FIRST, so an explicit `Deny` in a document that CAN be read refuses the request
    // whatever else the principal carries — including a grant this emulator cannot read,
    // which only lifts the DEFAULT deny of a request no readable statement allows.
    if identity_said == Decision::Denied {
        return Err(not_authorized(
            &caller.arn,
            &action,
            &resource,
            iam::IDENTITY_POLICY_EXPLICIT_DENY,
        ));
    }
    if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Err(not_authorized(
            &caller.arn,
            &action,
            &resource,
            &iam::no_identity_policy_allows(&action),
        ));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow is
    // not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string(),
            Decision::NotStated => iam::no_permissions_boundary_allows(&action),
        };
        return Err(not_authorized(&caller.arn, &action, &resource, &reason));
    }
    Ok(())
}

/// The modelled refusal of an unauthorized request: IAM's own sentence — the principal that
/// signed, the action it asked for, the key ARN the request addressed and which of IAM's
/// rules withheld the grant — built by the shared helper, so this service and every other
/// one that reaches the same decision word it alike.
fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(iam::not_authorized_by_identity_policy(
            principal, action, resource, reason,
        )),
    }
}

/// The resource the decision for a request addressing `key_identifier` is taken against:
/// the `KeyArn` the Payment Cryptography CONTROL plane holds that key under, and otherwise
/// the identifier the request carried.
///
/// The ARN is what an IAM statement names a key by, so a request addressing its key by an
/// alias is decided against the same ARN a statement scoped to that key states. The control
/// plane's store is read without this service's own guard held, and nothing here writes to
/// it.
fn key_resource<Eng: Send + Sync + 'static>(engine: &Eng, key_identifier: &str) -> String {
    let registered = key_store::service_state(engine)
        .and_then(|keys| keys.lock().resolve_key_arn(key_identifier));
    registered.unwrap_or_else(|| key_identifier.to_string())
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
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
    /// OWN accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
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
    let access_key_id = ::emulator_server::extract_akid(&ctx.parts().headers);
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
    // action it asks for falls to the default deny — and the refusal names the credential's
    // STATE rather than the key id that was presented, which this module never quotes back.
    let identity = match identity {
        Ok(identity) => identity,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            }
        }
    };
    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        arn: identity.arn,
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
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
            caller.principals = vec![user.arn.clone()];
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
            caller.principals.push(role_arn.clone());
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
    use super::{iam, Decision};

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// A Payment Cryptography key ARN of this emulator's own account, in the form the
    /// control plane mints: `arn:aws:payment-cryptography:<region>:<account>:key/<id>`.
    fn key_arn(region: &str, key_id: &str) -> String {
        ::engine::emulator_arn("payment-cryptography", region, &format!("key/{key_id}"))
    }

    /// RULE: the action this gate asks about carries the Payment Cryptography namespace, so
    /// a document written over that namespace covers these calls and one written over
    /// another service's covers none of them.
    #[test]
    fn the_action_asked_about_carries_the_payment_cryptography_namespace() {
        let resource = key_arn("us-west-2", "rce20kn5nxph148t");
        let action = "payment-cryptography:GenerateCardValidationData";
        // (case, the document in force, what it says about this action)
        let rows: &[(&str, &str, Decision)] = &[
            (
                "every action of this namespace",
                r#"{"Statement":[{"Effect":"Allow","Action":"payment-cryptography:*",
                 "Resource":"*"}]}"#,
                Decision::Allowed,
            ),
            (
                "exactly this action",
                r#"{"Statement":[{"Effect":"Allow",
                 "Action":"payment-cryptography:GenerateCardValidationData","Resource":"*"}]}"#,
                Decision::Allowed,
            ),
            (
                "another operation of this namespace",
                r#"{"Statement":[{"Effect":"Allow","Action":"payment-cryptography:GenerateMac",
                 "Resource":"*"}]}"#,
                Decision::NotStated,
            ),
            (
                "another service entirely",
                r#"{"Statement":[{"Effect":"Allow","Action":"kms:*","Resource":"*"}]}"#,
                Decision::NotStated,
            ),
            (
                "an explicit deny of this action beside an allow of the namespace",
                r#"{"Statement":[
                 {"Effect":"Allow","Action":"payment-cryptography:*","Resource":"*"},
                 {"Effect":"Deny","Action":"payment-cryptography:GenerateCardValidationData",
                  "Resource":"*"}]}"#,
                Decision::Denied,
            ),
        ];
        for (case, document, expected) in rows {
            let reached = decision(document, action, &resource);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {expected:?} for {action}, got {reached:?}"
            );
        }
    }

    /// RULE: the decision is taken against the KEY's ARN, so a statement scoped to one
    /// key's ARN covers that key and no other — not another key of the same account and
    /// Region, and not the same key id in another Region.
    #[test]
    fn a_statement_is_read_for_the_key_arn_the_request_resolves_to() {
        let addressed = key_arn("us-west-2", "rce20kn5nxph148t");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Deny",
             "Action":"payment-cryptography:EncryptData","Resource":"{addressed}"}}]}}"#
        );
        // (case, the resource the decision is taken against, whether the deny covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the key the request names", addressed.clone(), true),
            (
                "another key of the same account and Region",
                key_arn("us-west-2", "7pqr3stuvwx9yz01"),
                false,
            ),
            (
                "the same key id in another Region",
                key_arn("eu-west-3", "rce20kn5nxph148t"),
                false,
            ),
            (
                "an alias, which names no key ARN",
                "alias/card-verification".to_string(),
                false,
            ),
        ];
        for (case, resource, covered) in rows {
            let said = decision(&document, "payment-cryptography:EncryptData", resource);
            let denied = said == Decision::Denied;
            assert_eq!(
                denied, *covered,
                "case {case:?}: expected the deny to {}cover {resource}, got {said:?}",
                if *covered { "" } else { "not " }
            );
        }
    }
}
