// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets the principal an INVOCATION speaks for invoke the function it names.
//!
//! An invocation is a signed call, and AWS decides such a call at the IAM layer BEFORE the
//! function is reached: "the Lambda API … requires the caller to have permission for the
//! `lambda:InvokeFunction` action" — so a caller holding no grant is refused rather than told
//! anything about the function it named, and no deployment package runs for it. Lambda's model
//! declares no error shape for that decision (no operation of this service lists an access
//! error, `smithy_query(query="digest")`), because the refusal is the AWS COMMON
//! `AccessDeniedException` answered by the layer ahead of a service's own code: it travels
//! here through [`crate::scaffolded::Scaffolded::AccessDenied`], which renders HTTP 403 with
//! the error code `AccessDeniedException`.
//!
//! The pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for: the session STS minted
//!    that key for ([`sts_store::resolve_caller`], the same record `sts:GetCallerIdentity`
//!    answers from, so the principal a refusal blames is the one the caller reads back for the
//!    credentials it signed with), else the IAM user that owns the long-term key, else the
//!    account's own published credential;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for that
//!    principal and the permissions boundary that caps them, read through the IAM service's
//!    OWN accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutUserPolicy` wrote takes effect on the
//!    next invocation and deleting it lifts it again;
//! 3. **what the FUNCTION allows** — the resource-based policy this store holds for the
//!    qualifier the invocation addressed, which `AddPermission` and `PutResourcePolicy` wrote
//!    ([`crate::state::FunctionRecord::policies`]);
//! 4. **the decision** — [`::service_common::iam_authorization`], the tree's ONE evaluation of
//!    policy documents against `(action, resource)`, so this service and every other one
//!    handed the same documents answer alike.
//!
//! Only the store LOOKUPS live here; neither the decision nor the readings of a principal ARN
//! do. That is the same split every other gate in this tree makes (AWS Secrets Manager, AWS
//! Certificate Manager, Amazon Textract, AWS Entity Resolution), and the lookups stay in this
//! service crate for the reason those keep theirs there: hoisting them into `service_common`
//! would make every service that depends on that crate link the IAM and STS service crates.
//!
//! ## What an invocation is judged against
//!
//! The action is the one the IAM Service Authorization Reference publishes for the operation
//! ("Actions defined by AWS Lambda"): [`INVOKE_FUNCTION_ACTION`] for the two synchronous
//! entry points, and [`INVOKE_ASYNC_ACTION`] for the deprecated `InvokeAsync`.
//!
//! The resource is the function AT the qualifier the request addressed, spelled the way the
//! ARN of that qualifier is spelled ([`addressed_resource`]): the UNQUALIFIED function ARN for
//! an invocation that names no qualifier (or names the unpublished version), and the
//! `:<qualifier>`-suffixed one otherwise — the same construction `AddPermission` grants a
//! statement on. It is built from the REQUEST alone, before anything is looked up, so the
//! decision does not depend on whether that function exists: an unauthorized caller must not
//! learn from the refusal what the account holds.
//!
//! The decision is taken on the UNION of the two kinds of document AWS evaluates together: the
//! caller's identity-based policies and the addressed function's own resource-based policy. An
//! explicit `Deny` in either refuses; an `Allow` in either is a grant, so a principal holding
//! no identity-based permissions at all may invoke a function whose policy names it; with
//! neither, the invocation falls to IAM's default deny. A statement naming the ACCOUNT rather
//! than the caller delegates to that account's own identity-based policies rather than
//! granting, so the account spellings
//! ([`::service_common::iam_authorization::account_principals`]) decide a `Deny` and never
//! make a grant on their own.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id neither STS nor
//!   IAM holds a record for resolves to the account's own identity, which is not a principal
//!   the IAM store holds policies for — the position AWS account root credentials are in. Such
//!   a request is allowed. Every in-process call another emulated service makes to this one
//!   (an event source mapping's delivery, a gateway integration, a state machine's task) is
//!   signed with exactly that credential, and each of those services decides for itself
//!   whether the ROLE it acts as may invoke.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed policy
//!   belongs to AWS rather than to this account, so the IAM store holds no document for it;
//!   refusing on a grant this emulator cannot see would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses, and a permissions boundary the store
//!   DOES hold still caps the grant.
//! * **An engine carrying no STS or IAM store**, where no session and no role can exist.
//!
//! An expired session credential speaks for nobody: it carries no documents and no principal
//! spelling, so every invocation it asks for is refused.
//!
//! ## What one decision costs, and what it reads
//!
//! The work is bounded by what the two stores hold for ONE principal and ONE function, and
//! does not grow with the request: the documents in force on the principal, each collected
//! once by IAM's own accessor, and the single policy document held for the addressed
//! qualifier. Each store guard is taken and released in turn — never one inside another — and
//! nothing here writes to either store this service owns. Every value read comes from the
//! REQUEST (the access key id in its credential scope, the Region it was signed for, the
//! function name and the qualifier) and from those in-process stores: nothing reads the host's
//! environment or filesystem, opens a socket, or spawns anything.

use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::scaffolded::AccessDeniedException;

/// This service's IAM action prefix: an action is `lambda:<ApiName>` ("Actions defined by AWS
/// Lambda", IAM Service Authorization Reference), which is also this service's `arnNamespace`
/// (`smithy_query(query="service_info")`) and so the service segment of every ARN it mints.
pub const ACTION_PREFIX: &str = "lambda";

/// The action a SYNCHRONOUS invocation is performed under — `Invoke` and
/// `InvokeWithResponseStream`, each of which the API reference documents as needing
/// "permission for the `lambda:InvokeFunction` action".
pub const INVOKE_FUNCTION_ACTION: &str = "lambda:InvokeFunction";

/// The action the deprecated `InvokeAsync` entry point is performed under: the IAM Service
/// Authorization Reference publishes `lambda:InvokeAsync` as its own (deprecated) action,
/// distinct from [`INVOKE_FUNCTION_ACTION`], so a grant of one is not a grant of the other.
pub const INVOKE_ASYNC_ACTION: &str = "lambda:InvokeAsync";

/// The resource a request that names no single function is judged against, as IAM writes it
/// for such a request.
///
/// Reached only for a `FunctionName` that names no resource form at all, or one that names a
/// function of ANOTHER Region: neither addresses a function of this endpoint, so there is no
/// ARN of this account to decide on.
pub const ANY_RESOURCE: &str = "*";

/// What a refusal names as the principal of a request whose session credential has already
/// expired: that credential speaks for nobody, and the key it presented is not quoted back.
const EXPIRED_CREDENTIAL_PRINCIPAL: &str = "an expired session credential";

/// The ARN an invocation of `function_name` at `qualifier` addresses, which is the resource
/// the decision is made about.
///
/// Built from the request alone — no store is read — and spelled exactly as the ARN of that
/// qualifier is spelled ([`crate::state::FunctionRecord::versioned_arn`]): the unqualified
/// function ARN when the invocation names no qualifier or names the unpublished version, and
/// the `:<qualifier>`-suffixed ARN otherwise. That is what a statement's `Resource` element
/// has to match, on an identity-based policy scoped to one function and on the statement
/// `AddPermission` grants alike.
///
/// [`ANY_RESOURCE`] for a value that addresses no function of this endpoint.
#[must_use]
pub fn addressed_resource(region: &str, function_name: &str, qualifier: Option<&str>) -> String {
    let Some(reference) = crate::state::parse_function_reference(function_name) else {
        return ANY_RESOURCE.to_string();
    };
    if reference
        .region
        .as_deref()
        .is_some_and(|named| named != region)
    {
        return ANY_RESOURCE.to_string();
    }
    let arn = crate::state::function_arn(region, &reference.name);
    match reference.qualifier.as_deref().or(qualifier) {
        None | Some(crate::state::VERSION_LATEST) => arn,
        Some(named) => format!("{arn}:{named}"),
    }
}

/// The resource-based policy document the addressed qualifier of `function_name` holds, or
/// `None` when this endpoint holds no such function or that qualifier carries no policy.
///
/// Keyed by the qualifier label the permission was granted on, which is the key
/// `AddPermission` and `PutResourcePolicy` store under, so an invocation reads the very
/// statement those calls wrote. A pure READ: the record is not materialised and nothing is
/// written, so asking this changes nothing a later caller sees.
fn addressed_resource_policy<Eng: Send + Sync + 'static>(
    engine: &Eng,
    function_name: &str,
    qualifier: Option<&str>,
) -> Option<String> {
    let reference = crate::state::parse_function_reference(function_name)?;
    let label = reference
        .qualifier
        .as_deref()
        .or(qualifier)
        .unwrap_or(crate::state::VERSION_LATEST);
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    store
        .functions
        .get(&reference.name)?
        .policies
        .get(label)
        .map(crate::state::PolicyRecord::to_json)
}

/// The refusal AWS words an authorization failure with: who was refused, which action, on
/// which resource, and which rule withheld the grant. Every value in it comes from the request
/// and the stored policies.
fn not_authorized(principal: &str, action: &str, resource: &str, reason: &str) -> String {
    format!(
        "User: {principal} is not authorized to perform: {action} on resource: {resource} {reason}"
    )
}

/// Whether IAM lets this request perform `action` on the function `function_name` at
/// `qualifier`.
///
/// `Ok(())` for an invocation IAM permits (and for every caller this gate does not bound, see
/// the module docs); the refusal otherwise, which each invoking operation returns through
/// [`crate::scaffolded::Scaffolded::AccessDenied`]. Call it BEFORE the function is resolved and
/// before any run is attempted, so a refused invocation runs nothing and tells an unauthorized
/// caller nothing about whether the function exists.
///
/// Resolving the caller goes through STS's own `resolve_caller`, which first materialises the
/// session expiries the clock has already made due; that write is owed at the same instant
/// whether or not anybody called.
pub fn authorize_invocation<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    region: &str,
    function_name: &str,
    qualifier: Option<&str>,
) -> Result<(), AccessDeniedException> {
    let resource = addressed_resource(region, function_name, qualifier);
    let caller = caller(engine, ctx);
    let refusal = |reason: &str| {
        AccessDeniedException::new(not_authorized(&caller.arn, action, &resource, reason))
    };
    // The principal spellings travel with the request because that is what a statement's
    // `Principal` element is matched against; an identity-based document states no such
    // element, so for those the decision rests on the action and the resource. The caller's
    // OWN spellings are what a GRANT is decided under, because a statement naming the account
    // delegates rather than grants.
    let granting = iam::Request::new(action, &resource, &caller.principals);
    // Every spelling, the account's included: a statement that DENIES the account denies each
    // of its principals.
    let mut named = caller.principals.clone();
    named.extend(iam::account_principals(&caller.arn));
    let denying = iam::Request::new(action, &resource, &named);

    let function_policy = addressed_resource_policy(engine, function_name, qualifier);
    // An explicit `Deny` in the FUNCTION's own policy refuses whatever any grant says —
    // including the account's own credential, which the rest of this gate does not bound.
    if function_policy
        .as_deref()
        .is_some_and(|document| iam::resource_decision(document, &denying) == Decision::Denied)
    {
        return Err(refusal("with an explicit deny in a resource-based policy"));
    }
    let identity_said = iam::identity_decision(&caller.documents, &granting);
    // Taken before any grant is read, so an explicit `Deny` in a document that CAN be read
    // refuses the invocation whatever else the principal carries — including a grant this
    // emulator cannot read, which only lifts the DEFAULT deny of a request no readable
    // statement allows (see the module docs).
    if identity_said == Decision::Denied {
        return Err(refusal("with an explicit deny in an identity-based policy"));
    }
    // A grant in the function's own policy names this principal directly, and "a permissions
    // boundary does not limit the permissions granted to the entity by a resource-based policy
    // that names it as a principal" (IAM, permissions boundaries): such a grant stands on its
    // own, so a principal holding no identity-based policy at all is served.
    if function_policy
        .as_deref()
        .is_some_and(|document| iam::resource_decision(document, &granting) == Decision::Allowed)
    {
        return Ok(());
    }
    if !(iam::is_allowed(identity_said, Decision::NotStated) || caller.identity_is_unbounded()) {
        return Err(refusal(&format!(
            "because no identity-based policy allows the {action} action"
        )));
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy can
    // grant" (IAM, permissions boundaries): a grant the boundary does not also allow is not a
    // permission the principal has. The account's own credential carries none.
    if let Some(boundary) = &caller.boundary {
        let reason = match iam::identity_decision(::std::slice::from_ref(boundary), &granting) {
            Decision::Allowed => return Ok(()),
            // An explicit deny is a fact about the boundary document, so the refusal names it;
            // a boundary that merely states nothing withheld the grant the identity-based
            // policies gave.
            Decision::Denied => "with an explicit deny in a permissions boundary".to_string(),
            Decision::NotStated => {
                format!("because no permissions boundary allows the {action} action")
            }
        };
        return Err(refusal(&reason));
    }
    Ok(())
}

/// The principal a request speaks for, and everything the decision is made from.
///
/// Visible to the rest of this crate because the CONTROL-PLANE gate
/// ([`super::caller_authorization`]) asks the same question about the same request: who
/// signed it and what the documents in force on that principal say. One reading of a
/// request's principal, shared rather than copied, so the two gates cannot blame different
/// principals for one credential.
#[derive(Debug, Clone)]
pub(crate) struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    pub(crate) arn: String,
    /// Every spelling of the principal: the session ARN STS reports, and the ARN of the IAM
    /// role behind an assumed-role session.
    pub(crate) principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    pub(crate) documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose document
    /// this emulator holds.
    pub(crate) boundary: Option<String>,
    /// Set when this request presents the account's own published credential, which holds no
    /// IAM policies (see the module docs).
    pub(crate) account_credential: bool,
    /// Set when the principal has a policy attached whose document the IAM store does not hold
    /// — an AWS-managed policy (see the module docs).
    pub(crate) unreadable_attachment: bool,
}

impl Caller {
    /// Whether nothing the identity-based decision found can refuse this caller once no
    /// readable document DENIES the action: the account's own credential, which holds no IAM
    /// policies, or a principal carrying a grant this emulator cannot read.
    pub(crate) fn identity_is_unbounded(&self) -> bool {
        self.account_credential || self.unreadable_attachment
    }

    /// Read what the IAM identity `identity_arn` names may do, through the IAM service's OWN
    /// accessors: the documents in force on it — its inline policies, the default version of
    /// each managed policy attached to it and, for a user, the policies in force on the groups
    /// it belongs to — whether it carries an attachment whose document this emulator cannot
    /// read, and the permissions boundary that caps it.
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

/// The principal this request speaks for, with the identity-based policies in force on it.
pub(crate) fn caller<Eng: Send + Sync + 'static>(
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
        // account's own published credential.
        None => Ok(sts_store::ambient_identity()),
    };
    let identity = match identity {
        Ok(identity) => identity,
        // The session credential has expired, so it speaks for nobody: no documents, no
        // principal spelling, and every action it asks for falls to the default deny. The key
        // id it presented is NOT quoted back — this module echoes no value of the request —
        // and in the assembled emulator the layer that authenticates a request refuses an
        // expired credential before any handler runs.
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
    // minted is the account's ambient identity — but IAM's store maps an access key id to the
    // user that owns it (`access_keys`, the map IAM's own `GetUser` resolves a caller
    // through), so a request signed with one speaks for THAT user and is judged on the user's
    // policies rather than served as the account. A key the IAM store does not hold either
    // really is the account's own published credential.
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
    use super::*;

    /// The Region every row below is decided in.
    const REGION: &str = "us-east-1";

    /// What one document says about `action` on `resource`, through the shared evaluator this
    /// gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the resource an invocation is decided on is the ARN of the qualifier it
    /// addressed, spelled the way that qualifier's own ARN is — unqualified for the
    /// unpublished version, `:<qualifier>`-suffixed for a published version or an alias —
    /// whichever documented `FunctionName` form the request used.
    #[test]
    fn the_resource_is_the_arn_of_the_qualifier_the_invocation_addressed() {
        let unqualified = crate::state::function_arn(REGION, "orders");
        // (case, FunctionName, Qualifier, the resource the decision is made on)
        let rows: Vec<(&str, String, Option<&str>, String)> = vec![
            (
                "a bare name with no qualifier",
                "orders".to_string(),
                None,
                unqualified.clone(),
            ),
            (
                "a bare name naming the unpublished version",
                "orders".to_string(),
                Some(crate::state::VERSION_LATEST),
                unqualified.clone(),
            ),
            (
                "a bare name with a published version",
                "orders".to_string(),
                Some("7"),
                format!("{unqualified}:7"),
            ),
            (
                "a bare name with an alias",
                "orders".to_string(),
                Some("live"),
                format!("{unqualified}:live"),
            ),
            (
                "the name:qualifier form",
                "orders:live".to_string(),
                None,
                format!("{unqualified}:live"),
            ),
            (
                "the full function ARN",
                unqualified.clone(),
                None,
                unqualified.clone(),
            ),
            (
                "a qualified function ARN",
                format!("{unqualified}:7"),
                None,
                format!("{unqualified}:7"),
            ),
            (
                "an ARN of another Region",
                crate::state::function_arn("eu-west-1", "orders"),
                None,
                ANY_RESOURCE.to_string(),
            ),
            (
                "a value that names no documented form",
                "arn:aws:s3:::orders".to_string(),
                None,
                ANY_RESOURCE.to_string(),
            ),
        ];
        for (case, function_name, qualifier, expected) in rows {
            let actual = addressed_resource(REGION, &function_name, qualifier);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    /// RULE: the action asked about is the one the IAM reference publishes for the operation,
    /// under this service's own prefix — so a policy scoped to this service's actions covers
    /// an invocation and a policy scoped to another service's grants nothing.
    #[test]
    fn the_invocation_actions_carry_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"lambda:*","Resource":"*"}]}"#;
        for action in [INVOKE_FUNCTION_ACTION, INVOKE_ASYNC_ACTION] {
            assert!(
                action.starts_with(&format!("{ACTION_PREFIX}:")),
                "case {action:?}: expected the {ACTION_PREFIX} prefix, got {action:?}"
            );
            let reached = decision(document, action, ANY_RESOURCE);
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {action:?}: expected lambda:* to allow it, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", ANY_RESOURCE);
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected lambda:* to grant nothing outside this service, \
             got {outside:?}"
        );
        assert_ne!(
            INVOKE_FUNCTION_ACTION, INVOKE_ASYNC_ACTION,
            "case \"the two invoke actions\": expected distinct actions, got one"
        );
    }

    /// RULE: a grant is read for the ACTION and the RESOURCE it names, so a document scoped to
    /// one function or one qualifier allows an invocation of that one and no other.
    #[test]
    fn a_grant_is_read_for_the_action_and_the_resource_it_names() {
        let granted = crate::state::function_arn(REGION, "orders");
        let document = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow",
            "Action":"{INVOKE_FUNCTION_ACTION}","Resource":"{granted}"}}]}}"#
        );
        // (case, action, resource, allowed)
        let rows: Vec<(&str, &str, String, bool)> = vec![
            (
                "the granted action on the granted function",
                INVOKE_FUNCTION_ACTION,
                granted.clone(),
                true,
            ),
            (
                "the deprecated asynchronous action on the same function",
                INVOKE_ASYNC_ACTION,
                granted.clone(),
                false,
            ),
            (
                "the granted action on a published version of it",
                INVOKE_FUNCTION_ACTION,
                format!("{granted}:7"),
                false,
            ),
            (
                "the granted action on another function",
                INVOKE_FUNCTION_ACTION,
                crate::state::function_arn(REGION, "other"),
                false,
            ),
            (
                "the granted action on the same name in another Region",
                INVOKE_FUNCTION_ACTION,
                crate::state::function_arn("eu-west-1", "orders"),
                false,
            ),
            (
                "the granted action with no single function named",
                INVOKE_FUNCTION_ACTION,
                ANY_RESOURCE.to_string(),
                false,
            ),
        ];
        for (case, action, resource, expected) in rows {
            let said = decision(&document, action, &resource);
            let actual = iam::is_allowed(said, Decision::NotStated);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected allowed == {expected}, got {actual} ({said:?})"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource and the rule that
    /// withheld the grant, in the one sentence AWS words an authorization failure with.
    #[test]
    fn the_refusal_names_the_principal_the_action_the_resource_and_the_reason() {
        let principal = format!(
            "arn:aws:iam::{}:user/noperm-probe",
            ::engine::emulator_account_id()
        );
        let resource = crate::state::function_arn(REGION, "orders");
        // (case, the clause the message has to carry)
        let rows: Vec<(&str, String)> = vec![
            (
                "no policy allows it",
                format!("because no identity-based policy allows the {INVOKE_FUNCTION_ACTION} action"),
            ),
            (
                "a policy denies it",
                "with an explicit deny in an identity-based policy".to_string(),
            ),
            (
                "the function's own policy denies it",
                "with an explicit deny in a resource-based policy".to_string(),
            ),
            (
                "the permissions boundary withholds it",
                format!("because no permissions boundary allows the {INVOKE_FUNCTION_ACTION} action"),
            ),
            (
                "the permissions boundary denies it",
                "with an explicit deny in a permissions boundary".to_string(),
            ),
        ];
        for (case, reason) in rows {
            let message = not_authorized(&principal, INVOKE_FUNCTION_ACTION, &resource, &reason);
            let expected = format!(
                "User: {principal} is not authorized to perform: {INVOKE_FUNCTION_ACTION} on \
                 resource: {resource} {reason}"
            );
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
        }
    }

    /// RULE: a principal a request speaks for is read through the tree's SHARED readings of a
    /// principal ARN, so a session ARN names the role whose policies this gate reads and a
    /// user ARN names the user whose policies it reads, while every other form names neither.
    #[test]
    fn a_principal_arn_names_the_entity_its_policies_are_read_from() {
        let account = ::engine::emulator_account_id();
        // (case, the ARN, the role it names, the user it names)
        let rows: Vec<(&str, String, Option<&str>, Option<&str>)> = vec![
            (
                "an assumed-role session",
                format!("arn:aws:sts::{account}:assumed-role/invoker/session-1"),
                Some("invoker"),
                None,
            ),
            (
                "an IAM user",
                format!("arn:aws:iam::{account}:user/noperm-probe"),
                None,
                Some("noperm-probe"),
            ),
            (
                "a path'd IAM user",
                format!("arn:aws:iam::{account}:user/team/noperm-probe"),
                None,
                Some("noperm-probe"),
            ),
            (
                "the account's own identity",
                format!("arn:aws:iam::{account}:root"),
                None,
                None,
            ),
            ("a value that is no ARN", "AKIDTESTONLY".to_string(), None, None),
        ];
        for (case, arn, role, user) in rows {
            let named_role = ::service_common::iam_role::assumed_role_session_role_name(&arn);
            let named_user = ::service_common::iam_role::user_name(&arn);
            assert_eq!(
                named_role, role,
                "case {case:?}: expected role {role:?}, got {named_role:?}"
            );
            assert_eq!(
                named_user, user,
                "case {case:?}: expected user {user:?}, got {named_user:?}"
            );
        }
    }
}
