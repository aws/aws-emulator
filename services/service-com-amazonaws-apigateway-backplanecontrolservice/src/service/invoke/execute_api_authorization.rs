// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the caller of an `AWS_IAM`-authorized method invoke it.
//!
//! `Method$authorizationType` gives `AWS_IAM` "for using AWS IAM permissions", and AWS takes
//! TWO decisions for such a request: it AUTHENTICATES the SigV4 credential — which is the
//! emulator's own check, never a service's — and then AUTHORIZES `execute-api:Invoke` on the
//! method the request addressed, for the principal that credential speaks for. Without the
//! second decision an authenticated but unprivileged caller is served, which is the one answer
//! AWS never gives: a federated role session holding no `execute-api:Invoke` grant is refused
//! `403` with the sentence IAM words an authorization failure in.
//!
//! The three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`sts_store::resolve_request_caller`], the same record
//!    `sts:GetCallerIdentity` answers from, so the principal a refusal blames is the one the
//!    caller reads back for the credentials it signed with), else the IAM user that owns the
//!    long-term key, else the account's own published credential. A credential an identity pool
//!    VENDED is a session of that registry like any other, which is what makes a
//!    Cognito-federated caller a principal this decision can be taken about at all;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal, the permissions boundary that caps them, and the SESSION POLICIES the mint of
//!    the presented credential passed, read through IAM's own accessors
//!    (`principal_policies`, `permissions_boundary_document`,
//!    [`sts_store::session_scoping_documents`]), so a document `iam:PutRolePolicy` wrote takes
//!    effect on the next invoke, deleting it lifts it again, and a credential vended with a
//!    reduced scope reaches no further than that scope;
//! 3. **the decision** — [`::service_common::iam_authorization`], the one shared evaluation of
//!    those documents, asked about the method's own ARN
//!    ([`super::super::resource_policy::Call::method_arn`]) — so this gate and the api's
//!    RESOURCE policy, whose statement about the same method is handed in
//!    ([`resource_policy::invoke_decision`]), are asked about one resource.
//!
//! A refusal states all four: who was refused, the action, the method, and WHY — the clause
//! IAM closes an authorization failure with, naming the document class that refused the call
//! and whether its deny was explicit ([`not_authorized`]). That clause is what tells a caller
//! whether a grant would lift the refusal, so it is part of the answer rather than a comment
//! on it.
//!
//! ## How the two grants combine
//!
//! For a caller in the api's own account either grant suffices: AWS evaluates the api's
//! resource policy and the caller's identity-based policies together, and "if the API Gateway
//! resource policy allows the action, the identity-based policy need not" — so the request is
//! admitted when the document the developer attached allows it OR the caller's own policies do,
//! and refused when neither does. An explicit `Deny` in either refuses whatever the other says,
//! and the two are decided HERE, in one place, because the clause a refusal closes with names
//! which of them refused it.
//!
//! ## The callers this gate does not bound
//!
//! * **the account's own published credential** — a request whose access key id neither STS nor
//!   IAM holds a record for resolves to the account's own identity, which is not a principal
//!   the IAM store holds policies for: the position AWS account root credentials are in, and
//!   the credential every in-process caller of this emulator signs with;
//! * **a principal holding an attachment this emulator cannot read** — an AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it, and
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a boundary the store DOES hold
//!   still caps the grant;
//! * **an engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! Nothing here writes to this service's records. Resolving the caller goes through STS's own
//! `resolve_caller`, which materialises the session expiries the clock has already made due —
//! a write owed at the same instant whether or not anybody invoked anything.
//!
//! Only the store LOOKUPS live here; neither the decision nor the reading of a principal ARN
//! does. That is the split every other gate in this tree makes (`src/service/authorization.rs`
//! in the acm, bedrock and backup-search crates among them), and the lookups stay in this
//! crate for the reason those keep theirs there: hoisting them into `service-common` would
//! make every service depending on that crate link the IAM and STS service crates. WHICH IAM
//! entity a principal ARN names carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`], and the DECISION through the shared
//! [`::service_common::iam_authorization`].
//!
//! ## The other plane asking the same question
//!
//! This service's CONTROL plane takes the same decision about its own actions and resources —
//! `apigateway:<verb>` on the ARN of the resource the request addressed — and it asks it HERE,
//! through [`refusing_reason`] ([`crate::service::control_plane_authorization`] is the gate
//! that puts the question and states the sentence its caller reads). One reading of a caller
//! and one evaluation order for both planes: the documents the IAM store holds cannot refuse a
//! principal on one and admit it on the other, and the only difference is that a control-plane
//! call has no resource-based document to combine with, since an api's policy is about
//! invoking it.
//!
//! LOCK ORDER: the STS registry's lock and then the IAM store's, each taken and released in
//! turn while this service's own store lock is held. Neither of those services reads this
//! one's state, so that order is the only one taken.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use super::super::resource_policy::{self, INVOKE_ACTION};

/// How an expired session credential's invoke is refused: the sentence AWS words an expired
/// credential with, rather than an authorization-failure sentence naming a principal — that
/// credential speaks for nobody, so there is no principal to name, and the key it presented is
/// not quoted back either. In the assembled emulator the layer that authenticates a request
/// refuses such a credential before this is reached.
const EXPIRED_CREDENTIAL_MESSAGE: &str = "The security token included in the request is expired";

/// The clause IAM closes a refusal with when a readable identity-based document DENIES the
/// action rather than merely not allowing it: adding a grant will not lift such a deny, which
/// is exactly what the clause tells whoever reads the refusal.
const IDENTITY_EXPLICIT_DENY: &str = iam::IDENTITY_POLICY_EXPLICIT_DENY;

/// The clause IAM closes a refusal with when the ceiling in force on the principal carries an
/// explicit `Deny` of the action — a fact about the BOUNDARY document, so the refusal names
/// that document class rather than the policies that granted the action.
const BOUNDARY_EXPLICIT_DENY: &str = iam::PERMISSIONS_BOUNDARY_EXPLICIT_DENY;

/// The clause IAM closes a refusal with when one of the SESSION POLICIES the presented
/// credential was minted with denies the action outright.
const SESSION_EXPLICIT_DENY: &str = iam::SESSION_POLICY_EXPLICIT_DENY;

/// The clause IAM closes a refusal with when the api's OWN document — a resource-based policy
/// — denies the action: a fact about that document, which no identity-based grant lifts.
///
/// The ONE clause [`resource_policy`] words such a refusal with, called rather than copied:
/// that module refuses a call the method authenticates nobody for in the same words, and the
/// two gates must not word one deny differently.
const RESOURCE_POLICY_EXPLICIT_DENY: &str = resource_policy::RESOURCE_EXPLICIT_DENY;

/// The principal an invoke speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
pub struct SigningCaller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what the api's resource
    /// policy matches and what a refusal names.
    pub arn: String,
    /// Every spelling of the principal a policy may name it by: the session ARN STS reports,
    /// and the ARN of the IAM role behind an assumed-role session.
    spellings: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on it, when it carries one whose document this
    /// emulator holds.
    boundary: Option<String>,
    /// The SESSION POLICIES the mint of the presented credential passed, which cap what any
    /// grant gives this request ([`sts_store::ScopingPolicies`]). A credential no mint scoped
    /// — every long-term access key — caps nothing.
    scope: sts_store::ScopingPolicies,
    /// The CLASS of the credential the request was signed with, as STS reports it for that
    /// credential (`sts_store::Caller::principal_type`): the value the global condition key
    /// naming the calling principal's type answers with. `None` for a request this gate
    /// resolved no credential for, which carries no value for the key at all.
    principal_type: Option<&'static str>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    account_credential: bool,
    /// Set when the principal has an attachment whose document the IAM store does not hold.
    unreadable_attachment: bool,
    /// Set when the credential the request presented is a session that has already expired, so
    /// it speaks for nobody at all ([`EXPIRED_CREDENTIAL_MESSAGE`]).
    expired: bool,
    /// Who that principal IS, as `sts:GetCallerIdentity` reports it — its unique id, the
    /// account it belongs to and its ARN. The facts a call names its caller by
    /// ([`Self::authenticated`]); empty for a credential that speaks for nobody.
    identity: sts_store::Identity,
    /// The access key id the request presented, which is the credential a call reports the
    /// caller signed with ([`Self::authenticated`]).
    access_key_id: String,
}

/// Who API Gateway AUTHENTICATED for one call, as the call names its caller.
///
/// An `AWS_IAM` method's request is authenticated as the principal its SigV4 credential speaks
/// for ([`signing_caller`]), so such a call knows who made it and reports it: the access key
/// the request signed with, the account that principal belongs to, the principal's own unique
/// id and its ARN. A method that authenticates nobody has none of this to report.
#[derive(Debug, Clone)]
pub struct AuthenticatedCaller {
    /// The access key id the request signed with.
    pub access_key: String,
    /// The account the principal belongs to.
    pub account_id: String,
    /// The principal's unique id, as `sts:GetCallerIdentity` reports it — an IAM user's own
    /// id, and a role session's `<role id>:<session name>`.
    pub principal_id: String,
    /// The principal's ARN, the same value a refusal naming it carries.
    pub arn: String,
}

impl SigningCaller {
    /// Every spelling of this caller a statement's `Principal` element may name it by: the
    /// session ARN STS reports, and the ARN of the IAM role behind an assumed-role session.
    ///
    /// Read by the api's own RESOURCE policy beside this gate
    /// ([`super::super::resource_policy::Caller::Signed`]), because a resource-based
    /// statement naming a ROLE is about every session of that role — so both decisions are
    /// taken about one caller rather than about two different readings of it.
    #[must_use]
    pub fn spellings(&self) -> &[String] {
        &self.spellings
    }

    /// The condition context every decision about this caller is taken over: the CLASS of the
    /// credential it signed with, which is what a statement conditioned on `aws:PrincipalType`
    /// selects on. Empty for a credential this gate resolved nobody for.
    ///
    /// `pub` for the reason [`Self::spellings`] is: the CONTROL plane's gate decides over the
    /// same caller this module resolves, so the two planes answer the key alike.
    #[must_use]
    pub fn conditions(&self) -> Vec<(String, String)> {
        ::service_common::temporary_credential_classes::principal_type_conditions(
            self.principal_type,
        )
    }

    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action (see the module docs).
    fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Whether the credential this request presented is a session the clock has already run
    /// out on, which speaks for nobody at all.
    ///
    /// `pub` because the CONTROL plane's gate
    /// ([`crate::service::control_plane_authorization`]) states its own sentence for such a
    /// credential, over the same caller this module resolves: there is one resolution of who
    /// a request speaks for in this crate, and one evaluation ORDER
    /// ([`refusing_reason`]), with each plane wording its own refusal.
    #[must_use]
    pub fn expired(&self) -> bool {
        self.expired
    }

    /// Who this credential authenticates its request as, for the call to report
    /// ([`AuthenticatedCaller`]).
    ///
    /// `None` for a credential that speaks for nobody: a session the clock has run out on, and
    /// a request that presented no access key id at all.
    #[must_use]
    pub fn authenticated(&self) -> Option<AuthenticatedCaller> {
        if self.expired || self.access_key_id.is_empty() || self.identity.arn.is_empty() {
            return None;
        }
        Some(AuthenticatedCaller {
            access_key: self.access_key_id.clone(),
            account_id: self.identity.account.clone(),
            principal_id: self.identity.user_id.clone(),
            arn: self.identity.arn.clone(),
        })
    }

    /// Read what the IAM identity `identity_arn` names may do, through IAM's own accessors.
    fn read_policies_of(
        &mut self,
        store: &::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State,
        identity_arn: &str,
    ) {
        if let Some(policies) = store.principal_policies(identity_arn) {
            self.documents = policies.documents;
            // An attachment the collection resolved no document for is an entry of AWS's own
            // catalog this emulator does not carry: a grant it cannot read.
            self.unreadable_attachment = !policies.uncarried.is_empty();
        }
        self.boundary = store.permissions_boundary_document(identity_arn);
    }
}

/// The identity of a credential that speaks for NOBODY: a session the clock has run out on
/// names no principal, so there is no id, no account and no ARN for a call to report.
fn nobody() -> sts_store::Identity {
    sts_store::Identity {
        user_id: String::new(),
        account: String::new(),
        arn: String::new(),
    }
}

/// The principal the credential `access_key_id` speaks for, with the identity-based policies in
/// force on it.
///
/// `credentials` is the emulator's own credential store, which is what STS resolves a session
/// key through; `now` is the instant the session windows are read at.
pub fn signing_caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    access_key_id: &str,
    credentials: &::emulator_server::CredentialStore,
    now: SystemTime,
) -> SigningCaller {
    let resolved = sts_store::service_state(engine).map(|sts| {
        sts_store::resolve_request_caller(
            &mut sts.lock(),
            access_key_id,
            now,
            credentials,
            sts_store::ambient_identity(),
        )
    });
    // No STS store on this engine: the only identity a request can present is the account's
    // own published credential, which no mint scoped.
    let (principal_type, identity, session_policies) = match resolved {
        Some(Ok(caller)) => (
            Some(caller.principal_type()),
            caller.identity.clone(),
            caller.session_policies.clone(),
        ),
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and the invoke it asks for is refused as an expired credential
        // rather than as an authorization failure. The key id it presented is NOT quoted back,
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before this is reached.
        Some(Err(expired)) => {
            let _ = expired;
            return SigningCaller {
                arn: String::new(),
                spellings: Vec::new(),
                documents: Vec::new(),
                boundary: None,
                scope: sts_store::ScopingPolicies::default(),
                principal_type: None,
                account_credential: false,
                unreadable_attachment: false,
                expired: true,
                identity: nobody(),
                access_key_id: access_key_id.to_string(),
            };
        }
        None => (
            Some(::service_common::temporary_credential_classes::PRINCIPAL_TYPE_USER),
            sts_store::ambient_identity(),
            sts_store::SessionPolicies::default(),
        ),
    };

    let identity_arn = identity.arn.clone();
    let mut caller = SigningCaller {
        account_credential: identity_arn == sts_store::ambient_identity().arn,
        spellings: vec![identity_arn.clone()],
        arn: identity_arn,
        documents: Vec::new(),
        boundary: None,
        // Resolved against the IAM store below where there is one; a managed session policy
        // an engine carrying no IAM store cannot read scopes nothing, while the inline
        // document the mint passed scopes the session whether or not a store is there.
        scope: sts_store::session_scoping_documents(None, &session_policies),
        // WHICH CLASS of credential signed this request, read through STS's one answer for it
        // (`sts_store::Caller::principal_type`).
        principal_type,
        unreadable_attachment: false,
        expired: false,
        identity,
        access_key_id: access_key_id.to_string(),
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    caller.scope = sts_store::session_scoping_documents(Some(&store), &session_policies);
    // A LONG-TERM access key: STS's store holds SESSIONS, so its answer for a key it never
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it, so a request signed with one speaks for THAT user and is judged on the
    // user's policies. A key the IAM store does not hold either really is the account's own
    // published credential. WHICH identity a long-term key speaks for is STS's own reading of
    // IAM's records ([`sts_store::long_term_identity`]), called rather than repeated here.
    if caller.account_credential {
        let long_term = sts_store::long_term_identity(Some(&store), access_key_id);
        if long_term.arn != sts_store::ambient_identity().arn {
            caller.account_credential = false;
            caller.arn = long_term.arn.clone();
            caller.spellings = vec![long_term.arn.clone()];
            caller.identity = long_term;
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed by
        // the ARN the account holds it under — which carries its PATH, where the session ARN
        // does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            let role_arn = held.arn.clone();
            caller.spellings.push(role_arn.clone());
            caller.read_policies_of(&store, &role_arn);
        }
    } else if ::service_common::iam_role::user_name(&caller.arn).is_some() {
        let user_arn = caller.arn.clone();
        caller.read_policies_of(&store, &user_arn);
    }
    caller
}

/// The sentence an invoke whose credential the clock has already run out on is refused with, or
/// `None` for every live credential.
///
/// Decided BEFORE anything else is asked about the caller — before the api's own resource
/// policy, which [`crate::service::invoke`] applies next — because such a credential speaks for
/// nobody: there is no principal for a statement's `Principal` element to match, so neither a
/// wildcard grant nor a refusal naming a principal is about this request.
#[must_use]
pub fn expired_refusal(caller: &SigningCaller) -> Option<String> {
    caller
        .expired
        .then(|| EXPIRED_CREDENTIAL_MESSAGE.to_string())
}

/// The sentence this invoke is refused with when the caller's identity-based policies, the
/// scope its credential was minted with and the api's own resource policy do not between them
/// allow `execute-api:Invoke` on the method it addressed, or `None` when the request may
/// proceed.
///
/// The sentence closes with the clause IAM words the REASON of the failure in
/// ([`not_authorized`]): which document class refused the call, and whether its deny was
/// explicit or the default one of a call nothing allowed.
///
/// `resource_policy_said` is what the document the api carries states about this call
/// ([`resource_policy::invoke_decision`]): either grant suffices, so an allow there admits a
/// caller whose own policies say nothing, and an explicit deny there refuses whatever the
/// caller's own policies grant.
#[must_use]
pub fn refusal(
    caller: &SigningCaller,
    call: &resource_policy::Call<'_>,
    resource_policy_said: Decision,
) -> Option<String> {
    if let Some(expired) = expired_refusal(caller) {
        return Some(expired);
    }
    let method_arn = call.method_arn();
    // The resource the SENTENCE reports is the masked one the endpoint publishes, while the
    // DECISION is taken about the method's own ARN — which is what a statement names it by.
    let reported = call.reported_resource();
    // The decision rests on the action, the method ARN, the principal spellings — and the
    // CLASS of the credential presented, which is what a statement conditioned on
    // `aws:PrincipalType` selects on. That key is answered, and declared answered, for every
    // request whose credential this gate resolved, so such a clause decides rather than
    // standing undecided.
    let context = caller.conditions();
    let published =
        [::service_common::temporary_credential_classes::PRINCIPAL_TYPE_CONDITION_KEY];
    let request = iam::Request::new(INVOKE_ACTION, &method_arn, &caller.spellings)
        .with_context(&context)
        .with_published_keys(&published);
    let reason = refusing_reason(caller, &request, INVOKE_ACTION, resource_policy_said)?;
    Some(not_authorized(&caller.arn, &reported, &reason))
}

/// The CLAUSE naming which document class refuses `request` and why, in IAM's own
/// evaluation order, or `None` when nothing consulted refuses it.
///
/// The ONE place that order lives, for both planes of this service: the `execute-api` data
/// plane asks it about `execute-api:Invoke` on a method ARN with the api's own resource
/// policy's decision in hand ([`refusal`]), and the control plane asks it about
/// `apigateway:<method>` on the ARN of the path a request addressed
/// ([`crate::service::control_plane_authorization`]), which publishes no resource-based
/// policy and therefore passes [`Decision::NotStated`]. Each plane states the sentence its
/// own caller reads; the PRECEDENCE is this function's, so the two cannot drift.
///
/// The order, as IAM reads it:
///
/// * an EXPLICIT deny wins over everything, whichever document class carries it — the api's
///   own document first, because its deny is a fact about the api and applies "regardless of
///   the caller and Method configuration", then the caller's identity-based policies, then
///   the session policies its credential was minted with;
/// * then a GRANT is needed: an identity-based allow, a caller this gate does not bound
///   ([`SigningCaller::identity_is_unbounded`]), or a resource-based allow, since for a
///   caller in the api's own account either grant suffices;
/// * then each CAP applies — the session policies ("the resulting session's permissions are
///   the intersection of the role's identity-based policy and the session policies",
///   `AssumeRoleRequest$PolicyArns`) and the permissions boundary ("a permissions boundary
///   sets the maximum permissions that an identity-based policy can grant") — each taking
///   permissions away and granting none of its own. A grant the RESOURCE policy made is not
///   an identity-based one, so no boundary caps it.
#[must_use]
pub fn refusing_reason(
    caller: &SigningCaller,
    request: &iam::Request<'_>,
    action: &str,
    resource_policy_said: Decision,
) -> Option<String> {
    if resource_policy_said == Decision::Denied {
        return Some(RESOURCE_POLICY_EXPLICIT_DENY.to_string());
    }
    let identity_said = iam::identity_decision(&caller.documents, request);
    if identity_said == Decision::Denied {
        return Some(IDENTITY_EXPLICIT_DENY.to_string());
    }
    let scope_said = caller.scope.decision(request);
    if scope_said == Decision::Denied {
        return Some(SESSION_EXPLICIT_DENY.to_string());
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated)
        || caller.identity_is_unbounded()
        || resource_policy_said == Decision::Allowed;
    if !granted {
        // IAM's DEFAULT deny: no document the decision could read allows the call, so a grant
        // is what would lift it — which is what the clause naming the identity-based class
        // tells whoever reads the refusal.
        return Some(iam::no_identity_policy_allows(action));
    }
    if scope_said != Decision::Allowed {
        return Some(iam::no_session_policy_allows(action));
    }
    if resource_policy_said == Decision::Allowed || caller.identity_is_unbounded() {
        return None;
    }
    let boundary = caller.boundary.as_ref()?;
    match iam::identity_decision(::std::slice::from_ref(boundary), request) {
        Decision::Allowed => None,
        // An explicit deny is a fact about the boundary document, so the refusal names it;
        // a boundary that merely states nothing withheld the grant the identity-based
        // policies gave.
        Decision::Denied => Some(BOUNDARY_EXPLICIT_DENY.to_string()),
        Decision::NotStated => Some(iam::no_permissions_boundary_allows(action)),
    }
}

/// The sentence the `execute-api` endpoint refuses an unauthorized invoke with: the principal
/// that asked, the action it asked for, the method it asked for as the endpoint REPORTS it
/// ([`resource_policy::Call::reported_resource`]) and `reason` — the clause naming WHICH of the
/// documents consulted refused it and whether that deny was explicit.
///
/// The part up to the resource IS the sentence the api's own resource policy refuses a call
/// with — the one formatter [`resource_policy::not_authorized`] owns, called rather than
/// copied, because it is the same decision about the same method stated by the same endpoint.
/// What this adds is the reason clause, which is a fact about the IDENTITY-based decision
/// taken here: a caller reading it can tell an implicit deny, which a grant would lift, from
/// an explicit one, which no grant lifts.
fn not_authorized(principal: &str, reported_resource: &str, reason: &str) -> String {
    format!(
        "{sentence} {reason}",
        sentence = resource_policy::not_authorized(INVOKE_ACTION, principal, reported_resource)
    )
}

#[cfg(test)]
mod tests {
    //! The pure rule this module owns: how the two grants combine, and the sentence a refusal
    //! states. What a deployed stage SERVES for an `AWS_IAM` method is pinned end to end in
    //! `core/router/tests/all/apigateway_iam_authorized_invoke.rs`.

    use super::*;

    /// One invoke of `GET /orders` on stage `prod` of api `abc1230000`, speaking for the
    /// session `spellings` names.
    fn call<'a>(spellings: &'a [String]) -> resource_policy::Call<'a> {
        resource_policy::Call {
            region: "us-east-1",
            api_id: "abc1230000",
            stage: "prod",
            verb: "GET",
            resource_path: "/orders",
            request_path: "/orders",
            source_vpce: None,
            source_ip: Some("198.51.100.17"),
            caller: resource_policy::Caller::Signed {
                named: &spellings[0],
                spellings,
            },
        }
    }

    /// A federated role session carrying `documents`, with no boundary, no unreadable
    /// attachment and no scope — the position a Cognito-vended credential is in.
    fn session(documents: &[&str]) -> SigningCaller {
        SigningCaller {
            arn: "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity"
                .to_string(),
            spellings: vec![
                "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity"
                    .to_string(),
                "arn:aws:iam::000000000000:role/pool-authenticated".to_string(),
            ],
            documents: documents.iter().map(|document| (*document).to_string()).collect(),
            boundary: None,
            scope: sts_store::ScopingPolicies::default(),
            // A session of a role, which is the class `aws:PrincipalType` answers
            // `AssumedRole` for.
            principal_type: Some(
                ::service_common::temporary_credential_classes::PRINCIPAL_TYPE_ASSUMED_ROLE,
            ),
            account_credential: false,
            unreadable_attachment: false,
            expired: false,
            identity: sts_store::Identity {
                user_id: "AROAEXAMPLEPOOLAUTH01:CognitoIdentity".to_string(),
                account: "000000000000".to_string(),
                arn: "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity"
                    .to_string(),
            },
            access_key_id: "ASIAEXAMPLESESSION001".to_string(),
        }
    }

    /// RULE: a credential that authenticates reports the principal it speaks for — the key it
    /// signed with, that principal's account, its unique id and its ARN — and one that speaks
    /// for nobody reports none of it.
    #[test]
    fn a_caller_reports_the_principal_its_credential_speaks_for() {
        let live = session(&[]);
        let authenticated = live
            .authenticated()
            .expect("case \"a live session\": expected a principal to be reported");
        assert_eq!(
            (
                authenticated.access_key.as_str(),
                authenticated.account_id.as_str(),
                authenticated.principal_id.as_str(),
                authenticated.arn.as_str(),
            ),
            (
                "ASIAEXAMPLESESSION001",
                "000000000000",
                "AROAEXAMPLEPOOLAUTH01:CognitoIdentity",
                "arn:aws:sts::000000000000:assumed-role/pool-authenticated/CognitoIdentity",
            ),
            "case \"a live session\": got {authenticated:?}"
        );

        // Rows differ along WHY the credential speaks for nobody.
        let mut expired = session(&[]);
        expired.expired = true;
        expired.identity = nobody();
        let mut unsigned = session(&[]);
        unsigned.access_key_id = String::new();
        for (case, caller) in [
            ("a session the clock has run out on", expired),
            ("a request carrying no access key id", unsigned),
        ] {
            let reported = caller.authenticated();
            assert!(
                reported.is_none(),
                "case {case:?}: expected no principal to be reported, got {reported:?}"
            );
        }
    }

    /// The resolved scope of a credential minted with the inline session policy `document`.
    fn scoped(document: &str) -> sts_store::ScopingPolicies {
        sts_store::session_scoping_documents(
            None,
            &sts_store::SessionPolicies {
                inline: Some(document.to_string()),
                managed_arns: Vec::new(),
                none_means_no_permissions: false,
                ..sts_store::SessionPolicies::default()
            },
        )
    }

    /// RULE: either grant admits the invoke, neither refuses it, and an explicit deny refuses
    /// it whatever the other grant says.
    #[test]
    fn either_grant_admits_the_invoke_and_neither_refuses_it() {
        let allow_method = "{\"Statement\":[{\"Effect\":\"Allow\",\
                            \"Action\":\"execute-api:Invoke\",\
                            \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                            abc1230000/prod/GET/orders\"}]}";
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let allow_another_method = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                    \"Action\":\"execute-api:Invoke\",\
                                    \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                                    abc1230000/prod/GET/other\"}]}";
        let allow_another_action = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                    \"Action\":\"execute-api:InvalidateCache\",\
                                    \"Resource\":\"*\"}]}";
        let deny_every_invoke = "{\"Statement\":[{\"Effect\":\"Deny\",\
                                 \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        // Rows differ along what the identity-based documents state about this method and along
        // what the api's own resource policy states about it.
        let rows: &[(&str, &[&str], Decision, bool)] = &[
            ("the method itself", &[allow_method], Decision::NotStated, true),
            ("every invoke", &[allow_every_invoke], Decision::NotStated, true),
            (
                "another method of the same stage",
                &[allow_another_method],
                Decision::NotStated,
                false,
            ),
            (
                "another action on every resource",
                &[allow_another_action],
                Decision::NotStated,
                false,
            ),
            ("no policy at all", &[], Decision::NotStated, false),
            ("no policy, but the resource policy allows", &[], Decision::Allowed, true),
            ("an explicit deny", &[deny_every_invoke], Decision::NotStated, false),
            (
                "an explicit deny with the resource policy allowing",
                &[deny_every_invoke],
                Decision::Allowed,
                false,
            ),
            (
                "an allow beside a deny",
                &[allow_method, deny_every_invoke],
                Decision::NotStated,
                false,
            ),
            (
                "the resource policy denies what the identity grants",
                &[allow_every_invoke],
                Decision::Denied,
                false,
            ),
        ];
        for (case, documents, by_resource_policy, admitted) in rows {
            let caller = session(documents);
            let refused = refusal(&caller, &call(caller.spellings()), *by_resource_policy);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: the SCOPE the presented credential was minted with caps what any grant gives — the
    /// effective permission is the intersection of the identity's documents and the session
    /// policies, and an explicit deny in the scope refuses whatever a grant says.
    #[test]
    fn a_session_policy_caps_what_the_identity_grants() {
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let scope_another_verb = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\
                                  \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                                  abc1230000/prod/POST/orders\"}]}";
        let scope_deny = "{\"Statement\":[{\"Effect\":\"Deny\",\
                          \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        // Rows differ along what the scope states about this method and along which grant the
        // request would otherwise rest on — the identity's own, or the api's document.
        type Row<'r> = (&'r str, Option<&'r str>, Decision, bool);
        let rows: &[Row<'_>] = &[
            ("no scope at all", None, Decision::NotStated, true),
            ("a scope allowing this method", Some(allow_every_invoke), Decision::NotStated, true),
            (
                "a scope allowing only another verb",
                Some(scope_another_verb),
                Decision::NotStated,
                false,
            ),
            ("a scope stating nothing", Some("{\"Statement\":[]}"), Decision::NotStated, false),
            ("a scope denying the action", Some(scope_deny), Decision::NotStated, false),
            (
                "a scope allowing only another verb, with the resource policy allowing",
                Some(scope_another_verb),
                Decision::Allowed,
                false,
            ),
        ];
        for (case, scope, by_resource_policy, admitted) in rows {
            let mut caller = session(&[allow_every_invoke]);
            caller.scope = scope.map(scoped).unwrap_or_default();
            let refused = refusal(&caller, &call(caller.spellings()), *by_resource_policy);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: a permissions boundary caps what an identity-based policy granted — a grant the
    /// boundary does not also allow is not a permission the principal has.
    #[test]
    fn a_permissions_boundary_caps_the_identity_grant() {
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let boundary_other = "{\"Statement\":[{\"Effect\":\"Allow\",\
                              \"Action\":\"execute-api:Invoke\",\
                              \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                              other/prod/GET/orders\"}]}";
        // Rows differ along the boundary in force on the principal.
        let rows: &[(&str, Option<&str>, bool)] = &[
            ("no boundary at all", None, true),
            ("a boundary allowing this method", Some(allow_every_invoke), true),
            ("a boundary allowing only another api", Some(boundary_other), false),
            ("a boundary stating nothing", Some("{\"Statement\":[]}"), false),
        ];
        for (case, boundary, admitted) in rows {
            let mut caller = session(&[allow_every_invoke]);
            caller.boundary = boundary.map(str::to_string);
            let refused = refusal(&caller, &call(caller.spellings()), Decision::NotStated);
            assert_eq!(
                refused.is_none(),
                *admitted,
                "case {case:?}: expected admitted={admitted}, got {refused:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the `execute-api:Invoke` action, the ARN of the
    /// method the request addressed and the clause IAM closes an authorization failure with —
    /// which document class refused the call and whether its deny was explicit.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_method_and_the_reason() {
        let allow_every_invoke = "{\"Statement\":[{\"Effect\":\"Allow\",\
                                  \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let deny_every_invoke = "{\"Statement\":[{\"Effect\":\"Deny\",\
                                 \"Action\":\"execute-api:Invoke\",\"Resource\":\"*\"}]}";
        let boundary_other = "{\"Statement\":[{\"Effect\":\"Allow\",\
                              \"Action\":\"execute-api:Invoke\",\
                              \"Resource\":\"arn:aws:execute-api:us-east-1:000000000000:\
                              other/prod/GET/orders\"}]}";
        let sentence = "User: arn:aws:sts::000000000000:assumed-role/pool-authenticated/\
                        CognitoIdentity is not authorized to perform: execute-api:Invoke on \
                        resource: arn:aws:execute-api:us-east-1:********0000:\
                        abc1230000/prod/GET/orders";
        // Rows differ along WHICH document class refused the invoke and along whether that
        // class stated an explicit deny or merely failed to allow — every reading this gate
        // can reach.
        type Row<'r> = (&'r str, &'r [&'r str], Option<&'r str>, Option<&'r str>, Decision, &'r str);
        let rows: &[Row<'_>] = &[
            (
                "no document allows the action",
                &[],
                None,
                None,
                Decision::NotStated,
                " because no identity-based policy allows the execute-api:Invoke action",
            ),
            (
                "a readable document denies it",
                &[deny_every_invoke],
                None,
                None,
                Decision::NotStated,
                " with an explicit deny in an identity-based policy",
            ),
            (
                "the api's own document denies it",
                &[allow_every_invoke],
                None,
                None,
                Decision::Denied,
                " with an explicit deny in a resource-based policy",
            ),
            (
                "the scope states nothing about the action",
                &[allow_every_invoke],
                None,
                Some("{\"Statement\":[]}"),
                Decision::NotStated,
                " because no session policy allows the execute-api:Invoke action",
            ),
            (
                "the scope denies it",
                &[allow_every_invoke],
                None,
                Some(deny_every_invoke),
                Decision::NotStated,
                " with an explicit deny in a session policy",
            ),
            (
                "the boundary states nothing about the action",
                &[allow_every_invoke],
                Some("{\"Statement\":[]}"),
                None,
                Decision::NotStated,
                " because no permissions boundary allows the execute-api:Invoke action",
            ),
            (
                "the boundary allows only another api",
                &[allow_every_invoke],
                Some(boundary_other),
                None,
                Decision::NotStated,
                " because no permissions boundary allows the execute-api:Invoke action",
            ),
            (
                "the boundary denies it",
                &[allow_every_invoke],
                Some(deny_every_invoke),
                None,
                Decision::NotStated,
                " with an explicit deny in a permissions boundary",
            ),
        ];
        for (case, documents, boundary, scope, by_resource_policy, reason) in rows {
            let mut caller = session(documents);
            caller.boundary = boundary.map(str::to_string);
            caller.scope = scope.map(scoped).unwrap_or_default();
            let refused = refusal(&caller, &call(caller.spellings()), *by_resource_policy)
                .unwrap_or_else(|| panic!("case {case:?}: an unauthorized invoke is refused"));
            assert_eq!(
                refused,
                format!("{sentence}{reason}"),
                "case {case:?}: got {refused:?}"
            );
        }
    }

    /// RULE: the account's own published credential holds no IAM policies, so it is not bounded
    /// by this gate.
    #[test]
    fn the_accounts_own_published_credential_is_not_bounded() {
        let mut caller = session(&[]);
        caller.account_credential = true;
        let refused = refusal(&caller, &call(caller.spellings()), Decision::NotStated);
        assert!(
            refused.is_none(),
            "case \"the account's own credential\": expected the invoke to be admitted, got \
             {refused:?}"
        );
    }
}
