// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it names — the decision
//! taken for EVERY call this front door serves, before the operation reads or writes
//! anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at what the
//! request named: a caller whose identity-based policies grant none of this service's
//! actions is told it is not authorized rather than told anything about the trail, the
//! event data store or the channel it addressed, and the call changes nothing. So the
//! decision is taken once, for every operation, where this service's operations are built
//! ([`Authorized`]) — not per handler, where an operation nobody remembered to edit would
//! serve an unauthorized caller.
//!
//! The refusal is `AccessDeniedException` with HTTP 403 — the shape this service's own
//! model declares for it, `httpError` 403 (`smithy_query(query="shape",
//! shape="AccessDeniedException")`) — travelling through
//! [`crate::scaffolded::Scaffolded::AccessDenied`], because the model lists that shape for
//! only two operations while the decision is taken for all of them.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from ([`sts_store::resolve_request_caller`])
//!    for a session credential, and from IAM's own access-key map (the map `iam:GetUser`
//!    resolves a caller through, read via [`sts_store::long_term_identity`]) for a
//!    long-term key;
//! 2. **what it may do** — the identity-based policy documents in force on that principal
//!    and the permissions boundary that caps them, read through the IAM service's OWN
//!    accessors (`principal_policies`, `permissions_boundary_document`), so a document
//!    `iam:PutUserPolicy` wrote takes effect on the next call and deleting it lifts it
//!    again — and the SESSION POLICIES the mint of this very credential scoped it with
//!    ([`sts_store::session_scoping_documents`]), which cap those documents the same way:
//!    "the resulting session's permissions are the intersection of the role's
//!    identity-based policy and the session policies" (`AssumeRoleRequest$PolicyArns`);
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation
//!    of those documents against `(action, resource)`, and the SENTENCE a refusal carries,
//!    which is that module's reason clauses and
//!    [`::service_common::iam_authorization::not_authorized_by_identity_policy`], so
//!    nothing of IAM's wording is spelled here.
//!
//! Only the store LOOKUPS live here; neither the collection nor the decision does. They
//! stay in this crate because the IAM service crate depends on `service-common`, so the
//! shared crate cannot read IAM's store without a dependency cycle. WHICH IAM entity a
//! principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] rather than parsed again here.
//!
//! ## The action and the resource
//!
//! The action is `cloudtrail:<OperationName>` ([`ACTION_PREFIX`] is this service's
//! `arnNamespace`, which is also its signing name), so one statement scoped to this
//! service's actions covers every operation of it and nothing outside it.
//!
//! The resource is the ARN of the CloudTrail resource the REQUEST addresses, read off the
//! same identifier members this service's records name a call's resource by
//! ([`super::management_events::addressed_identifiers`], the one statement of which member
//! addresses which resource) and built into an ARN by that resource kind's own builder —
//! an identifier the caller wrote as an ARN is judged as that ARN. A request addressing
//! SEVERAL resources is judged on each of them, since a grant has to cover every resource
//! a call acts on; one addressing NONE — a collection, a lookup over event history — is
//! judged against every resource of this service ([`EVERY_RESOURCE`]), which is how IAM
//! writes the resource of such a request, and the refusal of such a call states no
//! resource clause because there is nothing to name in it.
//!
//! Taking the identifiers from the REQUEST rather than from a stored record is also what
//! keeps a refused call from telling an unauthorized caller whether what it named exists.
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
//! * **An engine carrying no IAM store**, where no principal can hold a policy at all.
//!
//! An expired session credential speaks for nobody: it carries no document, so every action
//! it asks for is refused, and the refusal names that state rather than the key it
//! presented, which this module never quotes back.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::service_common::management_events::{recorded_shape, WithheldFromRecord};
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Service,
};

use crate::state::ResourceKind;

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The namespace this service's actions are published under, which is also its signing
/// name and the vendor segment of every ARN it mints
/// (`crate::state::SERVICE_NAMESPACE`): a policy statement spells one of them
/// `cloudtrail:<OperationName>`.
pub const ACTION_PREFIX: &str = crate::state::SERVICE_NAMESPACE;

/// How IAM writes the resource of a request that addresses no single resource: every
/// resource the statement's own `Resource` element covers.
pub const EVERY_RESOURCE: &str = "*";

/// The IAM action `operation` is performed under: `cloudtrail:<OperationName>`.
#[must_use]
pub fn action_for(operation: &str) -> String {
    format!("{ACTION_PREFIX}:{operation}")
}

/// `Some(message)` when IAM does not let the calling principal perform
/// `cloudtrail:<operation>` on what this request addressed — the sentence the refusal
/// carries — and `None` when it does.
///
/// `request_parameters` is the request as a record reports it ([`recorded_shape`] of the
/// deserialized input), which is where the resources the call addresses are read from.
///
/// Asked with NO store guard of this service held, and before anything the request named is
/// resolved.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    operation: &str,
    request_parameters: &serde_json::Value,
) -> Option<String> {
    let action = action_for(operation);
    let caller = caller(engine, ctx);
    let region = crate::state::request_region(ctx.parts());
    // The principal's documents are READ ONCE for the whole request, however many resources
    // it addresses: "a caller asking about SEVERAL requests over the same documents (one
    // action on each of the resources it reaches, say) reads them once with
    // `prepare_documents` and asks through `identity_decision_prepared`"
    // ([`iam::identity_decision`]), which keeps the cost of n resources n statement walks
    // rather than n parses.
    let documents = iam::prepare_documents(&caller.documents);
    let boundary = caller.boundary.as_deref().map(iam::PreparedDocument::read);
    // A grant has to cover EVERY resource the call acts on, so the first resource nothing
    // allows refuses the call — and a request that addresses none is judged once, against
    // every resource of this service.
    judged_resources(operation, request_parameters, &region)
        .into_iter()
        .find_map(|resource| {
            decided(&caller, &documents, boundary.as_ref(), &action, &resource)
        })
}

/// The ARNs this request is judged against: one per CloudTrail resource it addresses, and
/// [`EVERY_RESOURCE`] for a request that addresses none.
///
/// An identifier the caller wrote as a CloudTrail resource ARN is that ARN; a bare name is
/// completed into the ARN of the kind its member names, in the Region the call was served
/// in. An identifier that names no CloudTrail resource of a known kind contributes nothing,
/// so it cannot be judged as a resource the service does not recognise. The list is as
/// bounded as the identifiers it is read from
/// (`super::management_events::MAX_ADDRESSED_IDENTIFIERS`), and a request carrying MORE
/// identifiers than that is judged against every resource of this service as well: a call
/// whose resources this gate did not all read is served only to a principal that may act on
/// all of them.
#[must_use]
fn judged_resources(
    operation: &str,
    request_parameters: &serde_json::Value,
    region: &str,
) -> Vec<String> {
    let addressed = super::management_events::addressed_identifiers(operation, request_parameters);
    let mut judged: Vec<String> = Vec::new();
    // Membership is answered by a SET, so building the list stays linear in the identifiers
    // read, while the order a refusal reports stays the order the request named them in.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for (kind, identifier) in addressed.named {
        if let Some(resource) = addressed_arn(kind, identifier, region) {
            if seen.insert(resource.clone()) {
                judged.push(resource);
            }
        }
    }
    if judged.is_empty() || addressed.beyond_the_bound {
        judged.push(EVERY_RESOURCE.to_string());
    }
    judged
}

/// The ARN `identifier` addresses, where `kind` is the kind the MEMBER carrying it names.
///
/// An `arn:` value is judged as written, once it is a CloudTrail resource ARN of a kind
/// this service holds; every other value is a bare name, completed into that kind's ARN in
/// `region`.
fn addressed_arn(kind: Option<ResourceKind>, identifier: &str, region: &str) -> Option<String> {
    if identifier.starts_with("arn:") {
        let (_region, _account, resource) = crate::state::cloudtrail_arn_fields(identifier)?;
        let named = ResourceKind::ALL.into_iter().any(|candidate| {
            resource
                .strip_prefix(candidate.resource_prefix())
                .is_some_and(|name| !name.is_empty())
        });
        return named.then(|| identifier.to_string());
    }
    let kind = kind?;
    (!identifier.is_empty()).then(|| kind.resource_arn(region, identifier))
}

/// The sentence refusing `action` on `resource` for `caller`, or `None` when IAM allows it.
///
/// IAM's order: an explicit `Deny` in a document that CAN be read refuses the request
/// whatever else the principal carries; otherwise a grant is needed, which a caller this
/// gate does not bound has by definition (see the module docs); and a grant is capped BOTH
/// by the session policies the credential was minted with, since "the resulting session's
/// permissions are the intersection of the role's identity-based policy and the session
/// policies" (`AssumeRoleRequest$PolicyArns`), AND by the permissions boundary, since "a
/// permissions boundary sets the maximum permissions that an identity-based policy can
/// grant" (IAM, permissions boundaries). Each cap takes permissions away and grants none of
/// its own, so a call only a cap allows is refused as if that cap had allowed nothing.
///
/// `documents` and `boundary` are the principal's documents already READ (see [`refusal`]),
/// so deciding one more resource of the same request costs a statement walk and no parse.
fn decided(
    caller: &Caller,
    documents: &[iam::PreparedDocument],
    boundary: Option<&iam::PreparedDocument>,
    action: &str,
    resource: &str,
) -> Option<String> {
    // An identity-based document states no `Principal` element, so the decision rests on
    // the action and the resource the request addressed.
    let request = iam::Request::new(action, resource, &[]);
    let said = iam::identity_decision_prepared(documents, &request);
    // Whether anything the principal carries grants the call at all. A caller this gate
    // does not bound is granted by definition, but an explicit `Deny` in a document that
    // CAN be read still wins over every grant.
    let granted = iam::is_allowed(said, Decision::NotStated)
        || (said != Decision::Denied && caller.identity_is_unbounded());
    // What the SCOPE this credential was minted with says, through the tree's one reading
    // of a resolved scope: a credential no mint scoped is capped by nothing, which that
    // reading answers as allowed — and holds no document to read either.
    let scoped = caller.scope.decision(&request);
    // The ceiling in force on the principal. A call an identity-based policy explicitly
    // DENIES is already refused on that deny, so no ceiling is read for it.
    let capped = match (said == Decision::Denied, boundary) {
        (false, Some(boundary)) => {
            iam::identity_decision_prepared(::std::slice::from_ref(boundary), &request)
        }
        _ => Decision::Allowed,
    };
    if granted && scoped == Decision::Allowed && capped == Decision::Allowed {
        return None;
    }
    // An explicit deny is a fact about the document that carries it, so the refusal names
    // that document; otherwise the refusal names the document that WITHHELD the grant — the
    // principal's own policies where nothing granted the call, the scope where they granted
    // it and the mint's session policies did not, and the boundary where both did.
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
    Some(not_authorized(&caller.arn, action, resource, &reason))
}

/// The refusal IAM states an authorization failure with: who was refused, which action, on
/// which resource, and which of IAM's rules refused it.
///
/// A request judged against [`EVERY_RESOURCE`] names no resource in the sentence — there is
/// nothing to name — and every other one carries the shared
/// [`iam::not_authorized_by_identity_policy`], so nothing of IAM's wording is spelled here.
/// Every value comes from the request and from the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    if resource == EVERY_RESOURCE {
        format!("User: {principal} is not authorized to perform: {action} {reason}")
    } else {
        iam::not_authorized_by_identity_policy(principal, action, resource, reason)
    }
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
    /// every long-term access key — which is a credential capped by nothing.
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
///
/// Resolving the caller goes through STS's own resolution, which materialises the session
/// expiries the clock has already made due before it answers: that write is owed at the
/// same instant whether or not anybody called, which is why a REPORTING operation of this
/// service may take this decision too.
fn caller<Eng: Send + Sync + 'static>(engine: &Eng, ctx: &OperationContext<'_>) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let iam = iam_store::service_state(engine);
    // The identity a LONG-TERM key speaks for: the IAM user that holds it, read through
    // STS's own answer for such a key (the account's own published credential when IAM
    // holds no record of it). The guard is released before the session registry is reached.
    let long_term = {
        let held = iam.as_ref().map(|state| state.lock());
        sts_store::long_term_identity(held.as_deref(), &access_key_id)
    };
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            long_term.clone(),
        ),
        // No STS store on this engine: the only credential a request can present is a
        // long-term one, which no mint scoped.
        None => Ok(sts_store::Caller {
            identity: long_term.clone(),
            kind: None,
            source_identity: None,
            tags: Vec::new(),
            transitive_tag_keys: Vec::new(),
            role_arn: None,
            role_id: None,
            issued_at: None,
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
    // one answer for it: the role behind an assumed-role session, the IAM user behind a
    // federated-user session, and the principal itself otherwise.
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
    let Some(iam) = iam else {
        return caller;
    };
    let store = iam.lock();
    // What the mint scoped this credential to, as the documents read at the instant of this
    // request: a managed session policy stands for its DEFAULT version, so one edited since
    // the mint scopes by what it says now.
    caller.scope = sts_store::session_scoping_documents(Some(&store), &resolved.session_policies);
    // Which IAM identity the principal's policies are held under is read through the tree's
    // shared readings of a principal ARN.
    if federated_session {
        // A FEDERATED-user session is judged on the documents of the IAM user whose
        // long-term credentials minted it — "the session permissions are the intersection
        // of the IAM user policies and the session policies that you pass"
        // (`GetFederationToken`) — because the federated user is no IAM identity and
        // carries no document of its own.
        caller.read_policies_of(&store, &identity_policies_arn);
    } else if let Some(role_name) =
        ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
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

/// One modelled operation served behind this service's authorization gate: IAM decides the
/// call, and only a call it permits reaches the operation.
///
/// It wraps the operation at the TYPED seam ([`Operation`]), inside the audit recorder
/// (`super::management_events::audited_table`), for two reasons: the gate is handed the
/// deserialized input, so the resources the request addresses are read off the built shape
/// rather than parsed out of a body a second time; and a refusal travels through the
/// operation's own error channel, so the call it refused is recorded in the account's event
/// history like every other call this service serves.
pub struct Authorized<O, Eng> {
    /// The operation that serves a call IAM permits.
    operation: O,
    /// The shared engine: the IAM and STS stores the decision is read from live there.
    engine: Arc<Eng>,
}

impl<O, Eng> Authorized<O, Eng> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O, engine: Arc<Eng>) -> Self {
        Self { operation, engine }
    }
}

impl<O, E, Eng> Operation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>>,
    E: Send + 'static,
    crate::scaffolded::Scaffolded<E>: smithy_server_runtime::OperationError + Send,
    Eng: Send + Sync + 'static,
{
    type Input = O::Input;
    type Output = O::Output;
    type Error = O::Error;

    const NAME: &'static str = O::NAME;
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = O::OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = O::SUCCESS_STATUS;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The request as a record reports it, which is where the resources it addresses are
        // read from. Nothing is withheld: no member of this service's model is `@sensitive`,
        // and the document is used for the decision and then dropped.
        let request_parameters =
            recorded_shape(&input, O::INPUT_SCHEMA, WithheldFromRecord::default());
        if let Some(message) = refusal(&*self.engine, ctx, O::NAME, &request_parameters) {
            return Err(crate::scaffolded::Scaffolded::access_denied(message));
        }
        self.operation.handle(input, ctx).await
    }
}

impl<O, E, Eng> ErasedOperation for Authorized<O, Eng>
where
    O: Operation<Error = crate::scaffolded::Scaffolded<E>> + ErasedOperation,
    E: Send + 'static,
    crate::scaffolded::Scaffolded<E>: smithy_server_runtime::OperationError + Send,
    Eng: Send + Sync + 'static,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The framework's own per-request sequence, driven with THIS wrapper as the
            // operation, so the handler it calls is the gated one. In the assembled service
            // the audit recorder drives that sequence instead and this is never reached;
            // it is here so an operation served through this wrapper alone is gated too.
            smithy_server_runtime::run_operation_http(
                self,
                svc,
                req,
                ctx,
                super::management_events::JSON_ENVELOPE,
            )
            .await
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's modelled schemes: gating a call must not change which
        // credentials sign it.
        self.operation.auth_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What one stored document says about `action` on `resource`, through the shared
    /// evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `cloudtrail:<OperationName>`, so a
    /// statement scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_namespace() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"cloudtrail:*","Resource":"*"}]}"#;
        for operation in [
            "ListTrails",
            "DescribeTrails",
            "DeleteTrail",
            "LookupEvents",
            "GetEventDataStore",
        ] {
            let action = action_for(operation);
            assert_eq!(
                action,
                format!("cloudtrail:{operation}"),
                "case {operation:?}: expected this service's namespace, got {action:?}"
            );
            let reached = decision(document, &action, EVERY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected cloudtrail:* to allow {action}, got {reached:?}"
            );
        }
        // The data plane's namespace is a different one, and grants nothing here.
        let outside = decision(
            r#"{"Statement":[{"Effect":"Allow","Action":"cloudtrail-data:*","Resource":"*"}]}"#,
            &action_for("ListTrails"),
            EVERY_RESOURCE,
        );
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another namespace\": expected no grant for this service, got {outside:?}"
        );
    }

    /// RULE: the resources a call is judged against are the ARNs of what its request
    /// addresses — an ARN the caller wrote as written, a bare name completed into that
    /// kind's ARN — and a request addressing none is judged against every resource.
    ///
    /// The rows span the forms an identifier takes and the kinds of resource this service's
    /// requests address.
    #[test]
    fn a_call_is_judged_against_what_its_request_addresses() {
        let region = "us-west-2";
        let store_arn = ResourceKind::EventDataStore
            .resource_arn(region, "f1f2f3f4-1111-4222-8333-444455556666");
        // (case, the operation, the request as a record reports it, the ARNs judged)
        let rows: Vec<(&str, &str, serde_json::Value, Vec<String>)> = vec![
            (
                "a collection, which addresses no resource",
                "ListTrails",
                json!({}),
                vec![EVERY_RESOURCE.to_string()],
            ),
            (
                "a lookup over event history",
                "LookupEvents",
                json!({"maxResults": 50}),
                vec![EVERY_RESOURCE.to_string()],
            ),
            (
                "one trail by its bare name",
                "GetTrailStatus",
                json!({"name": "audited"}),
                vec![ResourceKind::Trail.resource_arn(region, "audited")],
            ),
            (
                "one trail by its ARN",
                "DeleteTrail",
                json!({"name": ResourceKind::Trail.resource_arn(region, "audited")}),
                vec![ResourceKind::Trail.resource_arn(region, "audited")],
            ),
            (
                "several trails, each judged",
                "DescribeTrails",
                json!({"trailNameList": ["first", "second"]}),
                vec![
                    ResourceKind::Trail.resource_arn(region, "first"),
                    ResourceKind::Trail.resource_arn(region, "second"),
                ],
            ),
            (
                "one event data store, by the ARN its own member carries",
                "GetEventDataStore",
                json!({"eventDataStore": store_arn.clone()}),
                vec![store_arn.clone()],
            ),
            (
                "a tagging call, by the resource ARN it carries",
                "AddTags",
                json!({"resourceId": store_arn.clone()}),
                vec![store_arn.clone()],
            ),
            (
                "the same trail named twice, judged once",
                "DescribeTrails",
                json!({"trailNameList": ["first", "first"]}),
                vec![ResourceKind::Trail.resource_arn(region, "first")],
            ),
            (
                "an identifier naming no CloudTrail resource",
                "AddTags",
                json!({"resourceId": "arn:aws:s3:::a-bucket"}),
                vec![EVERY_RESOURCE.to_string()],
            ),
        ];
        for (case, operation, parameters, expected) in rows {
            let judged = judged_resources(operation, &parameters, region);
            assert_eq!(
                judged, expected,
                "case {case:?}: expected {expected:?}, got {judged:?}"
            );
        }
    }

    /// RULE: however many identifiers a request carries, the resources it is judged on are
    /// bounded — and a request carrying more than the bound is judged against every
    /// resource of this service as well, so a call whose resources were not all read is
    /// served only to a principal that may act on all of them.
    ///
    /// The rows are a list at the bound, one past it, and one far past it.
    #[test]
    fn a_request_is_judged_on_a_bounded_number_of_resources() {
        let region = "eu-west-3";
        let bound = super::super::management_events::MAX_ADDRESSED_IDENTIFIERS;
        for named in [bound, bound + 1, bound * 16] {
            let names: Vec<String> = (0..named).map(|index| format!("trail-{index}")).collect();
            let parameters = json!({"trailNameList": names});
            let judged = judged_resources("DescribeTrails", &parameters, region);
            let beyond = named > bound;
            assert_eq!(
                (judged.len(), judged.contains(&EVERY_RESOURCE.to_string())),
                (if beyond { bound + 1 } else { bound }, beyond),
                "case {named} identifiers: expected at most {bound} resources plus every \
                 resource once past the bound, got {} entries ({judged:?})",
                judged.len()
            );
        }
    }

    /// RULE: the refusal names the principal, the action and which of IAM's rules refused
    /// it — with a resource clause only where the request determined a resource to name.
    #[test]
    fn the_refusal_states_iams_sentence() {
        let principal = format!("arn:aws:iam::{}:user/analyst", ::engine::emulator_account_id());
        let action = action_for("ListTrails");
        let reason = iam::no_identity_policy_allows(&action);
        let stated = not_authorized(&principal, &action, EVERY_RESOURCE, &reason);
        assert_eq!(
            stated,
            format!("User: {principal} is not authorized to perform: {action} {reason}"),
            "case \"every resource\": expected no resource clause, got {stated:?}"
        );

        let trail = ResourceKind::Trail.resource_arn("us-west-2", "audited");
        let on_a_trail = action_for("DeleteTrail");
        let reason = iam::no_identity_policy_allows(&on_a_trail);
        let stated = not_authorized(&principal, &on_a_trail, &trail, &reason);
        assert_eq!(
            stated,
            format!(
                "User: {principal} is not authorized to perform: {on_a_trail} on resource: \
                 {trail} {reason}"
            ),
            "case \"one trail\": expected the resource clause, got {stated:?}"
        );
    }
}
