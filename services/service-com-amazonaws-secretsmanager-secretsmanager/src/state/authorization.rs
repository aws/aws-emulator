// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step every operation runs BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! request's own input, so a caller whose identity-based policies do not grant the action
//! is told it is not authorized rather than told anything about the secret it named, and
//! the call performs nothing. Secrets Manager's model declares no error shape for that
//! decision — the Smithy model's `common_errors` are `InternalServiceError` and
//! `InvalidParameterException`, and no operation lists an access error
//! (`smithy_query(query="digest")`) — because the refusal is the AWS COMMON
//! `AccessDeniedException`, answered by the layer ahead of a service's own code. This
//! service speaks awsJson1_1, which discriminates a failure by the error type in the body
//! rather than by the transport status, so that refusal travels with this service's
//! client-error status ([`crate::scaffolded::ACCESS_DENIED_STATUS`]) and the error code
//! [`crate::scaffolded::ACCESS_DENIED_CODE`], through
//! [`crate::scaffolded::Scaffolded::AccessDenied`].
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
//!    where the collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes
//!    effect on the next request, deleting it lifts it again, and this service reads an
//!    identity's permissions exactly as IAM's own authorizer does — and the SESSION POLICIES
//!    the mint of this very credential scoped it with
//!    (`sts_store::session_scoping_documents`), which cap those documents the same way:
//!    "the resulting session's permissions are the intersection of the role's
//!    identity-based policy and the session policies" (`AssumeRoleRequest$PolicyArns`, the
//!    STS model), so a session policy takes permissions away and grants none of its own —
//!    read exactly as the boundary is (`sts_store::ScopingPolicies::decision`), which is the
//!    tree's one reading of a resolved scope;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny.
//!
//! Only the store LOOKUPS live here; neither the decision nor the ARN readings do. That is
//! the same split every other gate in this tree makes (AWS Certificate Manager, Amazon
//! Textract, Amazon OpenSearch Service, Amazon EVS, CloudWatch RUM, Amazon Cognito
//! Identity), and the lookups stay in the service crate for the reason those keep theirs
//! there: hoisting them into `service-common` would make every service that depends on that
//! crate link the IAM and STS service crates. WHICH IAM ENTITY a principal ARN names
//! carries no such dependency, so it is read through the shared
//! [`::service_common::iam_role`] — `assumed_role_session_role_name` for the role behind a
//! session and `user_name` for an IAM user — rather than parsed again here.
//!
//! ## What a call is judged against
//!
//! The action is `secretsmanager:<OperationName>` ([`ACTION_PREFIX`]), and the resource is
//! what the request ADDRESSES:
//!
//! * an operation carrying a `SecretId` is judged against that secret's ARN
//!   ([`secret_resource`]) — a statement's `Resource` element matches ARNs, so the decision
//!   resolves the id, whichever form it took;
//! * `CreateSecret` against the ARN of the secret it is about to create, which it allocates
//!   before it stores anything ([`authorize_create`]);
//! * an operation whose `SecretId` names a secret this account does NOT hold against the
//!   ARN that id spells (`unheld_secret_arn`). The decision is still taken, and a caller no
//!   statement allows is refused WITHOUT the request reaching the store, so a refusal says
//!   nothing about whether the secret exists;
//! * an operation that addresses no secret of its own — the account-wide list, the password
//!   generator, the batched read that selects by filter — against the resource IAM writes
//!   for such a request ([`ANY_RESOURCE`]), and the refusal of one states no resource
//!   clause, because there is nothing to name.
//!
//! WHAT A REFUSAL NAMES is a separate question from what the decision is taken against: the
//! resource clause of the refusal states the identifier the REQUEST supplied, verbatim — the
//! `SecretId` as it was spelled (a name, a partial ARN or a full one) and a create's `Name`
//! — never an ARN the service resolved for it. A refused caller is told which resource IT
//! asked about, so the sentence is reproducible from the request alone and a client matching
//! the clause against what it sent finds it there; it also keeps the refusal from publishing
//! a resolved ARN, whose six trailing characters belong to a secret the refused caller was
//! never allowed to learn anything about.
//!
//! The condition keys the request and the addressed secret populate TRAVEL with the request
//! handed to the shared evaluation ([`crate::state::condition_keys`]), so a statement
//! qualified by one of them is decided rather than left undecided: a conditioned `Allow`
//! permits exactly the requests that satisfy it, and a conditioned `Deny` refuses exactly
//! those. A clause over a key this service does not model is still undecided, where that
//! evaluation's fail-closed reading takes over — such a statement never grants and never
//! stops denying.
//!
//! The decision is taken on the UNION of the two kinds of document AWS evaluates together:
//! the caller's identity-based policies and the addressed secret's own resource-based
//! policy, which `PutResourcePolicy` stored. An explicit `Deny` in either refuses; an
//! `Allow` in either is a grant, so a principal holding no identity-based permissions at all
//! is served a secret whose policy names it; with neither the request falls to IAM's
//! default deny. A statement that names the ACCOUNT rather than the caller delegates to the
//! account's own identity-based policies rather than granting, so the account spellings
//! ([`::service_common::iam_authorization::account_principals`]) decide a `Deny` and never
//! make a grant on their own.
//!
//! An operation whose documentation requires FURTHER permissions beyond its own action —
//! the batched read's `secretsmanager:GetSecretValue` on each secret it answers for, a
//! rotation's `lambda:InvokeFunction` on the rotation function — is judged here on its own
//! action alone; a decision about another action or another service's resource belongs to
//! the operation that needs it.
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

use crate::scaffolded::AccessDeniedException;
use crate::state::condition_keys::Conditions;

/// The resource an action that is not resource-scoped is judged against, as IAM writes it
/// for such a request.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// This service's IAM action prefix: an action is `secretsmanager:<OperationName>`
/// ("Actions defined by AWS Secrets Manager", IAM Service Authorization Reference), which
/// is this service's `arnNamespace` (`smithy_query(query="service_info")`) — the same
/// string this service spells into every ARN it builds.
pub const ACTION_PREFIX: &str = "secretsmanager";

/// The resource type a secret's ARN names: `arn:aws:secretsmanager:<region>:<account>:\
/// secret:<name>-<six characters>` (`::service_common::arn::SECRETS_MANAGER_SECRET`).
const SECRET_RESOURCE_TYPE: &str = "secret";

/// The reason clause a refusal states when an explicit `Deny` in the ADDRESSED SECRET's own
/// resource-based policy refuses the call: a fact about that document, so the sentence names
/// it rather than the action.
///
/// Stated once because two gates word it: a signed request of this service ([`decide`]) and a
/// retrieval another service makes on a role's behalf
/// ([`crate::state::secret_retrieval`]).
pub(crate) const RESOURCE_POLICY_EXPLICIT_DENY: &str =
    "with an explicit deny in a resource-based policy";

/// Authorize this request, where `operation` is the operation's own name
/// ([`smithy_server_runtime::Operation::NAME`]) and `resource` the ARN the request NAMES.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the refusal otherwise, which each operation returns through
/// [`crate::scaffolded::Scaffolded::AccessDenied`]. Call it BEFORE the store is read or
/// written, so a refused call performs nothing — and before the operation's own answer for
/// the secret it named, so a refusal tells an unauthorized caller nothing about whether
/// that secret exists.
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
    authorize_with(engine, ctx, operation, resource, Conditions::none())
}

/// [`authorize`] for a request that publishes condition keys of its own: `conditions` are
/// the keys its members populate ([`crate::state::condition_keys`]), which the statements'
/// `Condition` blocks are decided against.
///
/// The resource a request of this shape addresses is not a stored secret — an account-wide
/// action addresses nothing — so no resource-based policy takes part in the decision.
pub fn authorize_with<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    conditions: Conditions,
) -> Result<(), AccessDeniedException> {
    decide(engine, ctx, operation, resource, resource, &conditions, None)
}

/// [`authorize_with`] for a CREATE, which addresses the secret it is about to create:
/// judged against the ARN that create allocates ([`created_secret_arn`]) and refused naming
/// the `name` the request carries.
///
/// The decision is made against the ARN because that is what a statement's `Resource`
/// element matches, while what the refusal NAMES is the identifier the request supplied —
/// the `Name` it carries — the same reading every other refusal of this service makes.
///
/// A create addresses a secret that does not exist yet, so no resource-based policy takes
/// part in the decision.
pub fn authorize_create<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    name: &str,
    conditions: Conditions,
) -> Result<(), AccessDeniedException> {
    decide(engine, ctx, operation, resource, name, &conditions, None)
}

/// Whether the documents in force let this request perform `action` on `resource`.
///
/// `reported` is what a refusal NAMES as the resource, which is the identifier the request
/// supplied, verbatim; `resource_policy` is the addressed secret's own
/// stored policy, when it carries one, and `conditions` the condition keys the request and
/// that secret populate. The rule is IAM's own order, over the union of the two kinds of
/// document — see the module docs.
fn decide<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
    reported: &str,
    conditions: &Conditions,
    resource_policy: Option<&str>,
) -> Result<(), AccessDeniedException> {
    let action = format!("{ACTION_PREFIX}:{operation}");
    let caller = caller(engine, ctx);
    let refusal = |reason: &str| not_authorized(&caller.arn, &action, reported, reason);
    // The keys the CALLER populates travel with the request too, so a statement scoped to
    // one principal or one account — how a resource-based policy admits an account's
    // callers without admitting every caller — is decided rather than left undecided. The
    // principal they name is the IDENTITY whose permissions are in force, which for a
    // session is the role behind it ([`Caller::identity_arn`]). A credential that speaks
    // for nobody populates neither.
    let mut conditions = conditions.clone();
    if !caller.principals.is_empty() {
        conditions.add_caller(&caller.identity_arn);
    }
    let conditions = &conditions;
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element is matched against; an identity-based document states no such
    // element, so for those documents the decision rests on the action, the resource and
    // the conditions. The caller's OWN spellings are what a GRANT is decided under, because
    // a statement naming the account delegates rather than grants.
    let granting = iam::Request::new(&action, resource, &caller.principals)
        .with_context(conditions.values())
        .with_published_keys(conditions.published());
    // Every spelling, the account's included: a statement that DENIES the account denies
    // each of its principals.
    let mut named = caller.principals.clone();
    named.extend(iam::account_principals(&caller.arn));
    let denying = iam::Request::new(&action, resource, &named)
        .with_context(conditions.values())
        .with_published_keys(conditions.published());

    // An explicit `Deny` in the SECRET's own policy refuses whatever any grant says —
    // including the account's own credential, which the rest of this gate does not bound.
    if resource_policy.is_some_and(|document| {
        iam::resource_decision(document, &denying) == Decision::Denied
    }) {
        return Err(refusal(RESOURCE_POLICY_EXPLICIT_DENY));
    }
    // What the SCOPE this credential was minted with says, through the tree's one reading of
    // a resolved scope ([`sts_store::ScopingPolicies::decision`]): a credential no mint
    // scoped is capped by nothing, which that reading answers as allowed.
    let scoped = caller.scope.decision(&granting);
    let identity_said = iam::identity_decision(&caller.documents, &granting);
    // Taken before any grant is read, so an explicit `Deny` in a document that CAN be read
    // refuses the request whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(refusal("with an explicit deny in an identity-based policy"));
    }
    // An explicit `Deny` in a session policy refuses the same way, and before any grant is
    // read: "the resulting session's permissions are the intersection of the role's
    // identity-based policy and the session policies" (`AssumeRoleRequest$PolicyArns`, the
    // STS model), so a deny there is a deny whatever grants the call.
    if scoped == Decision::Denied {
        return Err(refusal(iam::SESSION_POLICY_EXPLICIT_DENY));
    }
    // ... and a call the scope does not ALLOW is refused even where something grants it: a
    // session policy takes permissions away and grants none of its own, which caps a grant
    // in the secret's own policy as well as one in the identity's documents.
    if scoped != Decision::Allowed {
        return Err(refusal(&iam::no_session_policy_allows(&action)));
    }
    // A grant in the secret's own policy names this principal directly, and "a permissions
    // boundary does not limit the permissions granted to the entity by a resource-based
    // policy that names it as a principal" (IAM, permissions boundaries): such a grant
    // stands on its own, so a principal holding no identity-based policy at all is served.
    if resource_policy.is_some_and(|document| {
        iam::resource_decision(document, &granting) == Decision::Allowed
    }) {
        return Ok(());
    }
    let granted =
        iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded();
    if !granted {
        return Err(refusal(&format!(
            "because no identity-based policy allows the {action} action"
        )));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &granting) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names
            // it; a boundary that merely states nothing withheld the grant the
            // identity-based policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(refusal(&reason));
    }
    Ok(())
}

/// Authorize a request that addresses the secret `secret_id` names, judged against that
/// secret's ARN ([`secret_resource`]) and against that secret's own stored resource-based
/// policy.
///
/// The one call site shape for every operation of this service that carries a `SecretId`,
/// so no two of them can judge the same request against different resources.
pub fn authorize_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    secret_id: Option<&str>,
) -> Result<(), AccessDeniedException> {
    authorize_secret_with(engine, ctx, operation, secret_id, Conditions::none())
}

/// [`authorize_secret`] for a request that publishes condition keys of its own.
///
/// `conditions` are the keys the request's OWN members populate; the keys the ADDRESSED
/// secret populates — its tags, and the `SecretId` the request named it by — are added
/// here, from the one store read this gate already makes, so no operation collects them
/// twice.
pub fn authorize_secret_with<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    secret_id: Option<&str>,
    conditions: Conditions,
) -> Result<(), AccessDeniedException> {
    let addressed = addressed_secret(engine, ctx, secret_id);
    let mut conditions = conditions.text(
        crate::state::condition_keys::SECRET_ID_KEY,
        secret_id.filter(|id| !id.is_empty()),
    );
    conditions.add_resource_tags(&addressed.tags);
    decide(
        engine,
        ctx,
        operation,
        &addressed.arn,
        &addressed.reported,
        &conditions,
        addressed.policy.as_deref(),
    )
}

/// What the store holds about the secret a request addresses, read ONCE: the ARN the
/// request is judged against, what a refusal NAMES as the resource — the `SecretId` the
/// request supplied — the tags that populate the resource-tag condition keys and the
/// resource-based policy that may itself grant the call.
#[derive(Debug, Clone, Default)]
struct AddressedSecret {
    arn: String,
    reported: String,
    tags: Vec<crate::state::StoredTag>,
    policy: Option<String>,
}

/// Read what the store holds about the secret `secret_id` names.
///
/// A READ of this service's own store, taken before the decision and under its own
/// short-lived guard: nothing is written, so what a later caller sees is unchanged. A
/// `SecretId` this account holds no secret for carries no tags and no policy, and is judged
/// against the ARN that id spells ([`secret_resource`]).
fn addressed_secret<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    secret_id: Option<&str>,
) -> AddressedSecret {
    let Some(secret_id) = secret_id.filter(|id| !id.is_empty()) else {
        return AddressedSecret {
            arn: ANY_RESOURCE.to_string(),
            reported: ANY_RESOURCE.to_string(),
            ..AddressedSecret::default()
        };
    };
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        // A request is addressed to one Region, so the secret it names is the one
        // THAT Region holds: the decision is made against the resource the
        // operation will act on, which is the one the operation itself resolves.
        // The tags and the policy a statement is decided against are the SECRET's,
        // which for a replica are the primary's it tracks.
        store
            .view(&crate::state::request_region(ctx.parts()), secret_id)
            .map(|view| AddressedSecret {
                arn: view.secret.arn.clone(),
                // The id the request supplied, whether or not this account holds a secret
                // under it: a refusal states the resource the CALLER asked about, so the
                // clause is reproducible from the request alone (see the module docs).
                reported: secret_id.to_string(),
                tags: view.content.tags.clone(),
                policy: view.content.resource_policy.clone(),
            })
    });
    held.unwrap_or_else(|| AddressedSecret {
        arn: unheld_secret_arn(ctx, secret_id),
        reported: secret_id.to_string(),
        ..AddressedSecret::default()
    })
}

/// The ARN the secret a `SecretId` names is judged against.
///
/// A `SecretId` is "the ARN or name of the secret", and the partial ARN (the full one
/// without its six trailing characters) names it too — while a policy statement's
/// `Resource` element matches ARNs, and the ARN of a secret this account holds is the FULL
/// one. So the id is resolved through the store's own resolution of a `SecretId`
/// ([`crate::state::ComAmazonawsSecretsmanagerSecretsmanagerState::get`]), which is the
/// same reading every operation addresses a secret by; a request naming a secret this
/// account does not hold is judged against the ARN that id spells — itself when it is an
/// ARN already, else the ARN a secret of that name would carry in the Region the caller
/// signed for.
///
/// `None`, for an operation whose `SecretId` is optional and absent, is judged against
/// [`ANY_RESOURCE`]: no secret is named, so there is none to judge against.
#[must_use]
pub fn secret_resource<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    secret_id: Option<&str>,
) -> String {
    addressed_secret(engine, ctx, secret_id).arn
}

/// The ARN a `SecretId` this account holds no secret for is judged against: itself when it
/// is an ARN of this service already, else the ARN a secret of that name would carry in the
/// Region the caller signed for.
fn unheld_secret_arn(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    secret_id: &str,
) -> String {
    if ::service_common::arn::is_resource_arn_of(secret_id, ACTION_PREFIX) {
        return secret_id.to_string();
    }
    secret_arn(&crate::state::request_region(ctx.parts()), secret_id)
}

/// The ARN a secret of name `name` carries in `region`, through the emulator's own ARN
/// builder — the same one `CreateSecret` allocates a secret's ARN from, so a policy scoped
/// to this account's secrets covers it.
///
/// The six random characters a created secret's ARN ends in belong to the secret rather
/// than to its name, so a name this account holds no secret for yields the ARN without
/// them: there is no stored secret to take them from, and inventing a value here would
/// name a resource nothing else addresses.
#[must_use]
pub fn secret_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ACTION_PREFIX,
        region,
        &format!("{SECRET_RESOURCE_TYPE}:{name}"),
    )
}

/// The ARN a create of a secret named `name` is judged against, which is also the ARN it
/// allocates for the secret: the one the account ALREADY holds a secret of that name under
/// — a repeat of a create addresses the resource the first one created, and answers from it
/// — else a fresh one, the name plus the six random characters a secret's ARN ends in.
///
/// Allocating it before the decision is what lets a create be judged against the secret it
/// is about to create, which is the resource the action is taken on, and is why a policy
/// scoping a create states the characters it ends in as a wildcard; nothing is stored by
/// reading it.
#[must_use]
pub fn created_secret_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    region: &str,
    name: &str,
) -> String {
    let held = crate::state::service_state(engine).and_then(|state| {
        let store = state.lock();
        store
            .named(region, name)
            .map(|secret| secret.arn.clone())
    });
    held.unwrap_or_else(|| {
        ::engine::emulator_arn(
            ACTION_PREFIX,
            region,
            &format!(
                "{SECRET_RESOURCE_TYPE}:{name}-{}",
                crate::state::new_arn_suffix()
            ),
        )
    })
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's two rules refused it. Every value in it comes
/// from the request and from the stored policies — the principal from the credential the
/// request presented, the action from the operation it named, the resource from the
/// identifier the request supplied for it, verbatim (see the module docs).
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
    AccessDeniedException::new(format!(
        "User: {principal} is not authorized to perform: {action}{on_resource} {reason}"
    ))
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the
    /// IAM role behind an assumed-role session.
    principals: Vec<String>,
    /// The ARN of the IDENTITY whose permissions are in force: the ROLE behind an
    /// assumed-role session, else the principal's own ARN. It is what the global condition
    /// key naming the calling principal answers with — "for IAM roles, the request context
    /// returns the ARN of the role, not the ARN of the user that assumed the role"
    /// (`aws:PrincipalArn`, IAM's global condition keys) — while the ARN a refusal NAMES
    /// stays [`Caller::arn`], which is what STS reports for the credential presented.
    identity_arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// The session policy documents the mint of this request's credential scoped it with, as
    /// they read at the instant of this request, and whether a managed one among them names a
    /// document this emulator does not hold. Empty for a credential no mint scoped — every
    /// long-term access key, and every session minted without session policies — which is a
    /// credential capped by nothing.
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
    /// own accessors: the documents in force on it, whether it carries an attachment whose
    /// document this emulator cannot read, and the permissions boundary that caps it.
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
    // The caller is read with the ROLE its credential is a session of, where it is one:
    // that role is the identity whose permissions are in force, and the one the global
    // principal condition key answers with (see [`Caller::identity_arn`]). STS records it
    // at the mint, so it is read from there rather than spelled from the session's ARN,
    // which carries no path.
    let resolved = match sts_store::service_state(engine) {
        Some(sts) => sts_store::resolve_request_caller(
            &mut sts.lock(),
            &access_key_id,
            SystemTime::now(),
            ctx.credential_store(),
            sts_store::ambient_identity(),
        )
        .map(|caller| (caller.identity, caller.role_arn, caller.session_policies)),
        // No STS store on this engine: the only identity a request can present is the
        // account's own published credential, which is a session of no role and which no
        // mint scoped.
        None => Ok((
            sts_store::ambient_identity(),
            None,
            sts_store::SessionPolicies::default(),
        )),
    };
    let (identity, session_role_arn, session_policies) = match resolved {
        Ok(resolved) => resolved,
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
                identity_arn: String::new(),
                documents: Vec::new(),
                boundary: None,
                scope: sts_store::ScopingPolicies::default(),
                account_credential: false,
                unreadable_attachment: false,
            };
        }
    };

    let mut caller = Caller {
        account_credential: identity.arn == sts_store::ambient_identity().arn,
        principals: vec![identity.arn.clone()],
        identity_arn: session_role_arn.clone().unwrap_or_else(|| identity.arn.clone()),
        arn: identity.arn,
        documents: Vec::new(),
        boundary: None,
        // The inline document the mint passed scopes the credential whether or not an IAM
        // store is there; where there is one, the scope is re-read below so a managed session
        // policy counts at the version in force NOW.
        scope: sts_store::session_scoping_documents(None, &session_policies),
        unreadable_attachment: false,
    };
    let Some(iam) = iam_store::service_state(engine) else {
        return caller;
    };
    let store = iam.lock();
    // What the mint scoped this credential to, as the documents read at the instant of this
    // request: a managed session policy stands for its DEFAULT version, so one edited since
    // the mint scopes by what it says now.
    caller.scope = sts_store::session_scoping_documents(Some(&store), &session_policies);
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
            caller.identity_arn = user.arn.clone();
            caller.principals = vec![user.arn.clone()];
        }
    }
    if let Some(role_name) = ::service_common::iam_role::assumed_role_session_role_name(&caller.arn)
    {
        // An assumed-role session's permissions are the ROLE's, and the role is addressed
        // by the ARN the account holds it under — which carries its PATH, where the
        // session ARN does not.
        if let Some(held) = store.roles.get(&iam_store::name_key(role_name)) {
            caller.principals.push(held.arn.clone());
            // The ARN the ACCOUNT holds the role under, which carries its path: the one a
            // statement names the role by, and so the one the principal condition key
            // answers with for a session of it.
            caller.identity_arn = held.arn.clone();
            caller.read_policies_of(&store, &held.arn.clone());
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

    /// The decision one stored document reaches about an action on a resource, through the
    /// shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `secretsmanager:<OperationName>`, so a
    /// policy scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"secretsmanager:*","Resource":"*"}]}"#;
        for operation in [
            "ListSecrets",
            "GetSecretValue",
            "CreateSecret",
            "UpdateSecretVersionStage",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected secretsmanager:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected secretsmanager:* to grant nothing outside this \
             service, got {outside:?}"
        );
    }

    /// RULE: the principal a request speaks for is read through the SHARED readings of an
    /// identity ARN, so a session ARN names the role whose policies this gate reads and an
    /// IAM user ARN names the user whose policies it reads, while every other principal
    /// form names neither (and the account's own identity carries no policies at all).
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        let account = ::engine::emulator_account_id();
        // (case, the ARN, the role it names, the user it names)
        let rows: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
            (
                "an assumed-role session",
                format!("arn:aws:sts::{account}:assumed-role/secrets-reader/session-1"),
                Some("secrets-reader"),
                None,
            ),
            (
                "a session name carrying a slash",
                format!("arn:aws:sts::{account}:assumed-role/secrets-reader/one/two"),
                Some("secrets-reader"),
                None,
            ),
            (
                "an IAM user",
                format!("arn:aws:iam::{account}:user/secrets-user"),
                None,
                Some("secrets-user"),
            ),
            (
                "an IAM user under a path",
                format!("arn:aws:iam::{account}:user/team/secrets-user"),
                None,
                Some("secrets-user"),
            ),
            (
                "the account's own identity",
                format!("arn:aws:iam::{account}:root"),
                None,
                None,
            ),
            (
                "a value that is no ARN",
                "AKIDTESTONLY".to_string(),
                None,
                None,
            ),
        ];
        for (case, arn, role, user) in rows {
            let read_role = ::service_common::iam_role::assumed_role_session_role_name(&arn);
            let read_user = ::service_common::iam_role::user_name(&arn);
            assert_eq!(
                read_role, role,
                "case {case:?}: expected the role {role:?}, got {read_role:?}"
            );
            assert_eq!(
                read_user, user,
                "case {case:?}: expected the user {user:?}, got {read_user:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource the request
    /// addressed and which of IAM's rules refused — every part taken from the request and
    /// the stored policies — and it names no resource for an action that addresses none.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let account = ::engine::emulator_account_id();
        let principal = format!("arn:aws:iam::{account}:user/secrets-user");
        let secret = secret_arn("us-west-2", "cdn-credentials-AbCdEf");
        // (case, the operation, the resource it addressed, the clause the sentence carries)
        let rows: &[(&str, &str, &str, String)] = &[
            (
                "a secret-scoped action",
                "GetSecretValue",
                secret.as_str(),
                format!(" on resource: {secret}"),
            ),
            (
                "an action that addresses no secret",
                "ListSecrets",
                ANY_RESOURCE,
                String::new(),
            ),
        ];
        for (case, operation, resource, clause) in rows {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reason = format!("because no identity-based policy allows the {action} action");
            let refusal = not_authorized(&principal, &action, resource, &reason);
            assert_eq!(
                refusal.message,
                format!(
                    "User: {principal} is not authorized to perform: {action}{clause} {reason}"
                ),
                "case {case:?}: expected IAM's wording, got {:?}",
                refusal.message
            );
        }
    }
}
