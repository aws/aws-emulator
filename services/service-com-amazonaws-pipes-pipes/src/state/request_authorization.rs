// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the CALLER of a request to this service perform the action it asks for
//! — the decision every operation takes BEFORE it reads or writes a pipe.
//!
//! This is the other half of [`super::execution_role`], and the two must not be confused: a
//! pipe's own `RoleArn` is what its STAGES act as once the pipe is running, while every
//! control-plane call is made by whoever signed it and is decided at the IAM layer ahead of
//! the service. A principal whose policies do not allow the action is told it is not
//! authorized and nothing about the pipe it addressed — whether a pipe of that name exists,
//! what it is wired to and which state it is in are all answers this refusal withholds —
//! and the call has no effect: a denied `StartPipe` leaves the pipe where it was and a
//! denied `CreatePipe` stores nothing.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS
//!    minted that key for ([`sts_store::resolve_request_caller`], the same place
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one
//!    the caller reads back for the credentials it signed with), else the IAM user that
//!    holds the long-term key, else the account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutRolePolicy` wrote takes effect on the next request and deleting it lifts it
//!    again — and the SESSION POLICIES the mint of this very credential scoped it with
//!    ([`sts_store::session_scoping_documents`]), which cap those documents the same way:
//!    "the resulting session's permissions are the intersection of the role's
//!    identity-based policy and the session policies" (`AssumeRoleRequest$Policy`), so a
//!    session policy takes permissions away and grants none of its own;
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, and the SENTENCE a refusal carries,
//!    which is that module's `not_authorized_by_identity_policy` and the reason clauses
//!    beside it, so nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. The
//! lookups stay in this service crate because the IAM service crate depends on
//! `service-common`, so the shared crate cannot read IAM's store without a dependency
//! cycle. WHICH IAM entity a principal ARN names carries no such dependency, so it is read
//! through the shared [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `pipes:<OperationName>` ([`action_for`]): `pipes` is this service's
//! `arnNamespace` and its `sigv4Name` alike (`smithy_query(query="service_info")`), which
//! is the namespace its actions are published under — so a grant of the EventBridge bus
//! API's `events:*` covers no action of this service.
//!
//! The resource is the PIPE the request addresses, by the one ARN form this namespace
//! publishes for a pipe ([`super::pipe_arn`]). It is built from the name the request
//! carried and the Region it was signed for, never resolved out of the store, so the
//! decision is the same whether or not a pipe of that name exists here and a refusal tells
//! an unauthorized caller nothing about what the emulator holds. The three ARN-addressed
//! tagging operations are decided against the ARN the request itself names, and `ListPipes`
//! addresses no pipe at all — it reports the account's pipes rather than acting on one — so
//! it is decided against [`EVERY_RESOURCE`], which is what a statement must cover to permit
//! an action that names no resource of its own.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS
//!   nor IAM holds a record for resolves to [`sts_store::ambient_identity`], which is not a
//!   principal the IAM store holds policies for — the position AWS account root credentials
//!   are in. Such a request is allowed.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An
//!   explicit `Deny` in a document that CAN be read still refuses, and a permissions
//!   boundary the store DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The action namespace this service's operations are published under, which is its
/// `arnNamespace` and its SigV4 signing name alike
/// (`smithy_query(query="service_info")`): a policy statement spells one of them
/// `pipes:<OperationName>`.
pub const ACTION_PREFIX: &str = "pipes";

/// The resource an action that addresses no single pipe is decided against: every resource,
/// which is what a statement has to cover to permit such a call.
pub const EVERY_RESOURCE: &str = "*";

/// The IAM action `operation` is performed under: `pipes:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// The refusal a request to `operation` on `resource` gets when the calling principal's IAM
/// policies do not allow it, and `None` when they do.
///
/// `operation` is the operation's own name ([`smithy_server_runtime::Operation::NAME`]) and
/// `resource` the ARN of the pipe the request ADDRESSES ([`super::pipe_arn`] for a
/// name-addressed call, the ARN the request carries for a tagging one, and
/// [`EVERY_RESOURCE`] for a call that addresses no pipe).
///
/// Asked BEFORE this service's store is read or written, and with the identifier the request
/// carried rather than one resolved from a stored record, so the decision does not depend on
/// whether that pipe exists — which is what keeps a refused call from telling an
/// unauthorized caller whether it does.
///
/// Resolving the caller goes through STS's own resolution, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called, which is why a REPORTING operation of this service may
/// take this decision too.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    caller(engine, ctx).refusal(&action_for(operation), resource)
}

/// The authorization refusal of this request as any operation's error channel carries it —
/// HTTP 403 `AccessDeniedException` with the sentence [`refusal`] states — or `None` when
/// the calling principal's policies allow the call.
///
/// The one form every operation asks the gate in, so no handler words the refusal or
/// chooses its status and code for itself. Generic over the operation's own modelled error
/// enum because the refusal is NOT one of them: this model declares no `AccessDenied` shape
/// for any operation, so it travels on the error channel's own variant.
#[must_use]
pub fn refused<Eng: Send + Sync + 'static, E>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    refusal(engine, ctx, operation, resource).map(crate::scaffolded::Scaffolded::access_denied)
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
    /// The session policy documents the mint of this request's credential scoped it with,
    /// as they read at the instant of this request. Empty for a credential no mint scoped —
    /// every long-term access key, and every session minted without session policies —
    /// which is a credential capped by nothing.
    scope: sts_store::ScopingPolicies,
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

    /// `Some(message)` when IAM does not allow `action` on `resource` for this caller — the
    /// sentence the refusal carries — and `None` when it does.
    ///
    /// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
    /// whatever else the principal carries; otherwise a grant is needed, which a caller this
    /// gate does not bound has by definition (see the module docs); and a grant is capped
    /// BOTH by the session policies the credential was minted with, since "the resulting
    /// session's permissions are the intersection of the role's identity-based policy and
    /// the session policies" (`AssumeRoleRequest$Policy`), AND by the permissions boundary,
    /// since "a permissions boundary sets the maximum permissions that an identity-based
    /// policy can grant" (IAM, permissions boundaries). Each cap takes permissions away and
    /// grants none of its own, so a call only a cap allows is refused as if that cap had
    /// allowed nothing.
    ///
    /// Which document withheld the grant is a different fact, and needs a different fix,
    /// from an explicit deny in it — so the refusal names the one the decision rested on, in
    /// the order IAM reads them.
    fn refusal(&self, action: &str, resource: &str) -> Option<String> {
        // An identity-based document states no `Principal` element, so the decision rests
        // on the action and the resource the request addressed.
        let request = iam::Request::new(action, resource, &[]);
        let said = iam::identity_decision(&self.documents, &request);
        let granted = iam::is_allowed(said, Decision::NotStated)
            || (said != Decision::Denied && self.identity_is_unbounded());
        // What the SCOPE this credential was minted with says, through the tree's one
        // reading of a resolved scope: a credential no mint scoped is capped by nothing,
        // which that reading answers as allowed.
        let scoped = self.scope.decision(&request);
        // The ceiling in force on the principal. A call an identity-based policy explicitly
        // DENIES is already refused on that deny, so no ceiling is read for it.
        let capped = match (said == Decision::Denied, self.boundary.as_ref()) {
            (false, Some(boundary)) => {
                iam::identity_decision(::std::slice::from_ref(boundary), &request)
            }
            _ => Decision::Allowed,
        };
        if granted && scoped == Decision::Allowed && capped == Decision::Allowed {
            return None;
        }
        let reason = if said == Decision::Denied {
            iam::IDENTITY_POLICY_EXPLICIT_DENY.to_string()
        } else if scoped == Decision::Denied {
            iam::SESSION_POLICY_EXPLICIT_DENY.to_string()
        } else if capped == Decision::Denied {
            iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY.to_string()
        } else if !granted {
            iam::no_identity_policy_allows(action)
        } else if scoped != Decision::Allowed {
            iam::no_session_policy_allows(action)
        } else {
            iam::no_permissions_boundary_allows(action)
        };
        Some(iam::not_authorized_by_identity_policy(
            &self.arn, action, resource, &reason,
        ))
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it, whether it carries an attachment whose
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
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // Who the credential speaks for AND what its mint scoped it to, read together from the
    // session registry: the scope travels with the credential, so the only thing a later
    // request carries is where it has to be read from.
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            sts_store::ambient_identity(),
        ),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which holds no identity policy and which no
        // mint scoped.
        None => Ok(sts_store::Caller {
            identity: sts_store::ambient_identity(),
            kind: None,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            role_arn: None,
            federating_user_arn: None,
            federated_claims: Vec::new(),
            session_policies: sts_store::SessionPolicies::default(),
            mfa_authenticated: false,
        }),
    };
    // An expired session credential speaks for nobody: it carries no document, so every
    // action it asks for is refused — and the refusal names the credential's STATE rather
    // than the key id that was presented, which this module never quotes back.
    let resolved = match resolved {
        Ok(resolved) => resolved,
        Err(_) => {
            return Caller {
                arn: EXPIRED_CREDENTIAL_PRINCIPAL.to_string(),
                ..Caller::default()
            };
        }
    };
    // WHOSE identity-based documents are in force for this credential, read through STS's
    // one answer for it: the role behind an assumed-role session (resolved below to the ARN
    // the account holds the role under, which carries its path), the IAM user behind a
    // federated-user session, and the principal itself otherwise. Taken before the resolved
    // caller is broken up below.
    let identity_policies_arn = resolved.identity_policies_arn().to_string();
    let federated_session = resolved.federating_user_arn.is_some();
    let mut caller = Caller {
        account_credential: resolved.identity.arn == sts_store::ambient_identity().arn,
        arn: resolved.identity.arn,
        // The inline document the mint passed scopes the credential whether or not an IAM
        // store is there; where there is one, the scope is re-read below so a managed
        // session policy counts at the version in force NOW.
        scope: sts_store::session_scoping_documents(None, &resolved.session_policies),
        ..Caller::default()
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // What the mint scoped this credential to, as the documents read at the instant of this
    // request: a managed session policy stands for its DEFAULT version, so one edited since
    // the mint scopes by what it says now.
    caller.scope = sts_store::session_scoping_documents(Some(&store), &resolved.session_policies);
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
    if federated_session {
        // A FEDERATED-user session is judged on the documents of the IAM user whose
        // long-term credentials minted it — "the session permissions are the intersection of
        // the IAM user policies and the session policies that you pass"
        // (`GetFederationToken`) — because the federated user is no IAM identity and carries
        // no document of its own.
        caller.read_policies_of(&store, &identity_policies_arn);
    } else if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
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
    use super::{action_for, iam, Decision, EVERY_RESOURCE};

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about carries this service's OWN namespace, so a
    /// document scoped to the EventBridge bus API's actions states nothing about it.
    ///
    /// The operations are the ten this model binds, as `smithy_query(query="digest")` lists
    /// them.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        for operation in [
            "CreatePipe",
            "DeletePipe",
            "DescribePipe",
            "ListPipes",
            "ListTagsForResource",
            "StartPipe",
            "StopPipe",
            "TagResource",
            "UntagResource",
            "UpdatePipe",
        ] {
            let action = action_for(operation);
            assert_eq!(
                action,
                format!("pipes:{operation}"),
                "case {operation:?}: expected this service's namespace, got {action:?}"
            );
            let resource = crate::state::pipe_arn("us-west-2", "orders-to-results");
            // (case, the document in force, what it says about this action)
            let rows: &[(&str, String, Decision)] = &[
                (
                    "every action of this service",
                    r#"{"Statement":[{"Effect":"Allow","Action":"pipes:*","Resource":"*"}]}"#
                        .to_string(),
                    Decision::Allowed,
                ),
                (
                    "exactly this action",
                    format!(
                        r#"{{"Statement":[{{"Effect":"Allow","Action":"{action}",
                         "Resource":"*"}}]}}"#
                    ),
                    Decision::Allowed,
                ),
                (
                    "every action of the event bus API",
                    r#"{"Statement":[{"Effect":"Allow","Action":"events:*","Resource":"*"}]}"#
                        .to_string(),
                    Decision::NotStated,
                ),
                (
                    "an explicit deny of this action beside an allow of the namespace",
                    format!(
                        r#"{{"Statement":[
                         {{"Effect":"Allow","Action":"pipes:*","Resource":"*"}},
                         {{"Effect":"Deny","Action":"{action}","Resource":"*"}}]}}"#
                    ),
                    Decision::Denied,
                ),
            ];
            for (case, document, expected) in rows {
                let reached = decision(document, &action, &resource);
                assert_eq!(
                    reached, *expected,
                    "case {case:?} / {operation}: expected {expected:?} for {action}, got \
                     {reached:?}"
                );
            }
        }
    }

    /// RULE: the decision is taken against the PIPE's own ARN, so a grant scoped to one
    /// pipe covers that pipe and no other — not another pipe of the same account and
    /// Region, not the same name in another Region, and not a call that addresses no pipe
    /// at all.
    #[test]
    fn a_grant_is_read_for_the_pipe_the_request_addressed() {
        let addressed = crate::state::pipe_arn("us-west-2", "orders-to-results");
        assert_eq!(
            addressed, "arn:aws:pipes:us-west-2:000000000000:pipe/orders-to-results",
            "case \"the ARN form\": expected this namespace's pipe ARN, got {addressed:?}"
        );
        let action = action_for("DescribePipe");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
             "Action":"{action}","Resource":"{addressed}"}}]}}"#
        );
        // (case, the resource the decision is taken against, whether the grant covers it)
        let rows: &[(&str, String, bool)] = &[
            ("the pipe the request names", addressed.clone(), true),
            (
                "another pipe of the same account and Region",
                crate::state::pipe_arn("us-west-2", "orders-to-archive"),
                false,
            ),
            (
                "the same pipe name in another Region",
                crate::state::pipe_arn("eu-west-3", "orders-to-results"),
                false,
            ),
            (
                "a call that addresses no pipe at all",
                EVERY_RESOURCE.to_string(),
                false,
            ),
        ];
        for (case, resource, covered) in rows {
            let said = decision(&document, &action, resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, *covered,
                "case {case:?}: expected the grant to {}cover {resource}, got {said:?}",
                if *covered { "" } else { "not " }
            );
        }
    }
}
