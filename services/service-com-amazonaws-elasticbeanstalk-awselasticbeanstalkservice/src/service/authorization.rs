// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation takes BEFORE it reads or writes anything.
//!
//! AWS decides authorization at the IAM layer, ahead of the service looking at the
//! resource, so a caller whose policies do not permit the action is told it is not
//! authorized and the call performs nothing: no resource is created, no status moves and
//! no quota is consumed. The answer is `AccessDenied` with HTTP 403, the refusal AWS
//! states for a call its identity-based policies do not allow.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the IAM
//!    store of the same shared engine, which holds both halves of that question: the
//!    role-session record a temporary credential's mint left there, and the registry
//!    `iam:CreateAccessKey` wrote for a long-term key. So the principal a refusal blames
//!    is the one the account really holds for the credential the request signed with.
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal and the permissions boundary that caps them, read through IAM's own
//!    accessors (`principal_policies`, `permissions_boundary_document`), where the
//!    collection rule itself lives — so a policy `iam:PutRolePolicy` wrote takes effect on
//!    the next request and deleting it lifts it again.
//! 3. **the decision** — [`::service_common::iam_authorization`], the tree's ONE
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request falls
//!    to IAM's default deny. A grant the permissions boundary or the SESSION POLICIES the
//!    credential's own mint passed do not also allow is not a permission the caller has,
//!    so each of those caps the decision.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split every
//! other gate in this tree makes, and the lookups stay in the service crate for the reason
//! those keep theirs there: hoisting them into `service-common` would make every service
//! that depends on that crate link the IAM service crate.
//!
//! ## The callers this gate does not bound
//!
//! * **The account's own published credential.** A request whose access key id the IAM
//!   store holds no record for speaks for the account itself — the position AWS account
//!   root credentials are in — which holds no identity-based policies, so such a request
//!   is allowed. An emulated AWS service calling this one through the in-process callback
//!   signs with that credential.
//! * **A principal holding an attachment this emulator cannot read.** An AWS-managed
//!   policy belongs to AWS rather than to this account, so the account's store holds no
//!   document for it; refusing on a grant this emulator cannot see would deny access AWS
//!   permits. An explicit `Deny` in a document that CAN be read still refuses, and a
//!   permissions boundary the store DOES hold still caps the grant.
//! * **An engine carrying no IAM store**, where no role and no policy can exist.

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_common::iam_authorization::{self as iam, Decision};

/// This service's IAM action prefix: an action is `elasticbeanstalk:<OperationName>`
/// ("Actions defined by AWS Elastic Beanstalk", IAM Service Authorization Reference),
/// which is this service's `arnNamespace` — the same string every ARN this service builds
/// carries.
pub const ACTION_PREFIX: &str = "elasticbeanstalk";

/// The refusal this request earns from the policies in force, or `None` when IAM permits
/// it (and for every caller this gate does not bound, see the module docs).
///
/// `operation` is the operation's own name ([`smithy_server_runtime::Operation::NAME`]) and
/// `resource` the ARN of the resource the request acts on, as the REQUEST names it rather
/// than as a stored record resolves it — so the decision does not depend on whether that
/// resource exists, which is also what keeps a refused call from telling an unauthorized
/// caller whether it does.
///
/// Call it before this service's own store is locked: the IAM store is reached here, and
/// nothing of this service's state is read for the decision.
#[must_use]
pub fn refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Option<String> {
    decision(engine, ctx, &format!("{ACTION_PREFIX}:{operation}"), resource)
}

/// Whether the policies in force let this caller perform `action` on `resource`, where
/// `action` is an IAM action of ANY service — `s3:GetObject`, say, for the object storage
/// read a create makes on the caller's behalf.
///
/// The same decision [`refusal`] takes, asked as a question: a call this service makes for
/// the caller is the CALLER's call, so it is permitted exactly when the caller's own
/// policies permit it.
#[must_use]
pub fn permits<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> bool {
    decision(engine, ctx, action, resource).is_none()
}

/// The IAM action a service needs to hand an IAM role to a resource it creates for the
/// caller ("to pass a role to a service, a user must have permission to pass the role",
/// IAM's `iam:PassRole`): handing the role is the CALLER's act, so IAM decides it against
/// the caller's own policies.
pub const ACTION_PASS_ROLE: &str = "iam:PassRole";

/// Elastic Beanstalk's own sentence for a call IAM does not let this caller make
/// because of the role it hands over: `Insufficient privileges for IAM PassRole
/// Action.`
///
/// WHOLLY FIXED text — it names the IAM action that was refused and nothing of
/// the request, so the role ARN and the principal are not echoed back to a caller
/// that may not pass it. The service states it for every call refused on that
/// grant, whichever error code the operation answers it under, so one condition
/// is worded one way.
pub const INSUFFICIENT_PASS_ROLE_PRIVILEGES: &str = "Insufficient privileges for IAM PassRole Action.";

/// Whether the policies in force let this caller hand the IAM role `role_arn` to
/// this service ([`ACTION_PASS_ROLE`] on that role).
///
/// The VERDICT alone, for a call that answers its own fixed sentence
/// ([`INSUFFICIENT_PASS_ROLE_PRIVILEGES`]) rather than IAM's; the refusal with
/// IAM's own message is [`pass_role_refusal`], which a launch records in the
/// environment's event stream.
#[must_use]
pub fn may_pass_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> bool {
    pass_role_refusal(engine, ctx, role_arn).is_none()
}

/// The refusal this caller earns for handing the IAM role `role_arn` to this service, or
/// `None` when the policies in force let it ([`ACTION_PASS_ROLE`] on that role).
///
/// Every role a request hands over is decided here — the operations role an environment
/// then acts under, and the role an instance profile carries
/// ([`pass_instance_profile_refusal`]) — so the one rule covers them all.
#[must_use]
pub fn pass_role_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    role_arn: &str,
) -> Option<String> {
    decision(engine, ctx, ACTION_PASS_ROLE, role_arn)
}

/// The refusal this caller earns for passing the IAM instance profile
/// `instance_profile` to the instances of an environment it launches, or `None` when the
/// policies in force let it ([`ACTION_PASS_ROLE`] on the role that profile names, see
/// [`crate::state::instance_profile_role_arn`]).
///
/// The MESSAGE is answered, not just the verdict: a launch nothing may pass the profile
/// for reports why in the environment's event stream, which is where a client reads the
/// progress of a launch.
#[must_use]
pub fn pass_instance_profile_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    instance_profile: &str,
) -> Option<String> {
    pass_role_refusal(
        engine,
        ctx,
        &crate::state::instance_profile_role_arn(instance_profile),
    )
}

/// The refusal `action` on `resource` earns from the policies in force on this request's
/// caller, or `None` when they permit it.
fn decision<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Option<String> {
    let (principal, _) = denial(engine, ctx, action, resource)?;
    Some(not_authorized(&principal, action, resource))
}

/// IAM's own statement of the refusal `action` on `resource` earns, as the service that
/// TOOK the decision words it: who was refused, which action, on which resource, and which
/// of IAM's rules refused it — or `None` when the policies in force permit it.
///
/// The classification clause is part of that statement where the SERVICE THAT REFUSED
/// carries it, which a service reporting another's refusal passes on as it arrived
/// ([`crate::state::downstream_refusal`]). This service's own refusals state the decision
/// without it ([`not_authorized`]).
#[must_use]
pub fn classified_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Option<String> {
    let (principal, reason) = denial(engine, ctx, action, resource)?;
    Some(format!(
        "{} {reason}.",
        not_authorized(&principal, action, resource)
    ))
}

/// WHO was refused `action` on `resource` and WHICH of IAM's rules refused it, or `None`
/// when the policies in force permit it — the one evaluation both renderings above read.
fn denial<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    action: &str,
    resource: &str,
) -> Option<(String, String)> {
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
        return Some((
            caller.arn,
            "with an explicit deny in an identity-based policy".to_string(),
        ));
    }
    let granted = iam::is_allowed(identity_said, Decision::NotStated) || caller.is_unbounded();
    if !granted {
        return Some((
            caller.arn,
            format!("because no identity-based policy allows the {action} action"),
        ));
    }
    // "The resulting session's permissions are the intersection of the role's
    // identity-based policy and the session policies" (`AssumeRole`): a session policy the
    // MINT of this very credential passed caps what the identity's own documents grant and
    // grants nothing of its own, so it is read exactly as the permissions boundary below is.
    // A credential no mint scoped caps nothing.
    if !caller.scope.caps_nothing() {
        match caller.scope.decision(&request) {
            Decision::Allowed => {}
            Decision::Denied => {
                return Some((
                    caller.arn,
                    "with an explicit deny in a session policy".to_string(),
                ))
            }
            Decision::NotStated => {
                return Some((
                    caller.arn,
                    format!("because no session policy allows the {action} action"),
                ))
            }
        }
    }
    // "A permissions boundary sets the maximum permissions that an identity-based policy
    // can grant" (IAM, permissions boundaries): a grant the boundary does not also allow
    // is not a permission the principal has. The account's own credential carries none.
    let boundary = caller.boundary.as_ref()?;
    match iam::identity_decision(::std::slice::from_ref(boundary), &request) {
        Decision::Allowed => None,
        // An explicit deny is a fact about the boundary document, so the refusal names it;
        // a boundary that merely states nothing withheld the grant the identity-based
        // policies gave.
        Decision::Denied => Some((
            caller.arn,
            "with an explicit deny in a permissions boundary".to_string(),
        )),
        Decision::NotStated => Some((
            caller.arn,
            format!("because no permissions boundary allows the {action} action"),
        )),
    }
}

/// The refusal IAM states an authorization failure with: who was refused, which action and
/// on which resource. Every value in it comes from the request and from the stored
/// policies — the principal from the credential the request presented, the action from the
/// operation it named, the resource from the ARN it addressed — and nothing else: the
/// sentence states the decision, not WHICH of IAM's rules reached it, which is what a
/// caller reading it can act on.
fn not_authorized(principal: &str, action: &str, resource: &str) -> String {
    format!("User: {principal} is not authorized to perform: {action} on resource: {resource}")
}

/// The principal a request speaks for, and everything the decision is made from.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// Every spelling of the principal: the session ARN a temporary credential speaks
    /// under, and the ARN of the IAM role behind it.
    principals: Vec<String>,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// The permissions boundary in force on the principal, when it carries one whose
    /// document this emulator holds.
    boundary: Option<String>,
    /// What the MINT of this credential scoped the session down to: the session policies
    /// an `AssumeRole` passed, as they read at the instant of this request. Empty for a
    /// credential no mint scoped, which caps nothing.
    scope: iam_store::ScopingPolicies,
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
}

/// Who this request speaks for, resolved from the access key id it signed with.
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Caller {
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let Some(iam) = iam_store::service_state(engine) else {
        return account_caller();
    };
    let store = iam.lock();
    let now = ::aws_smithy_types::DateTime::from(::std::time::SystemTime::now());
    // A TEMPORARY credential of a role session: the principal is the SESSION, and the
    // policies in force are the ROLE's ("the temporary security credentials … have the
    // permissions of the role", `AssumeRole`). The role's own ARN is a spelling a policy
    // names the caller by too.
    if let Some(session) = store.role_session(&access_key_id, now) {
        let mut caller = Caller {
            arn: session.session_arn.clone(),
            principals: vec![session.session_arn.clone(), session.role_arn.clone()],
            // "The resulting session's permissions are the intersection of the role's
            // identity-based policy and the session policies": what this credential's own
            // mint passed, resolved through IAM's own reading of it, so a managed session
            // policy says what its default version says NOW.
            scope: iam_store::session_scoping_documents(Some(&store), &session.scope),
            ..Caller::default()
        };
        let role_arn = session.role_arn.clone();
        read_policies_of(&store, &role_arn, &mut caller);
        return caller;
    }
    // A LONG-TERM access key speaks for the IAM user that holds it, which is the identity
    // `sts:GetCallerIdentity` reports for it; a key the account holds no record for is the
    // account's own published credential.
    let user_arn = store
        .access_keys
        .get(&access_key_id)
        .and_then(|record| store.users.get(&record.user_key))
        .map(|user| user.arn.clone());
    match user_arn {
        Some(user_arn) => {
            let mut caller = Caller {
                arn: user_arn.clone(),
                principals: vec![user_arn.clone()],
                ..Caller::default()
            };
            read_policies_of(&store, &user_arn, &mut caller);
            caller
        }
        None => account_caller(),
    }
}

/// The caller a request presenting the account's own published credential speaks for: the
/// principal `sts:GetCallerIdentity` reports for it, which the IAM store holds no policies
/// for (see the module docs).
fn account_caller() -> Caller {
    let arn = ::service_common::caller_principal::ambient_user_arn();
    Caller {
        arn: arn.clone(),
        principals: vec![arn],
        account_credential: true,
        ..Caller::default()
    }
}

/// Read what the IAM identity `identity_arn` names may do, through the IAM service's own
/// accessors: the documents in force on it, whether it carries an attachment whose
/// document this emulator cannot read, and the permissions boundary that caps it.
fn read_policies_of(
    store: &iam_store::ComAmazonawsIamAwsidentitymanagementv20100508State,
    identity_arn: &str,
    caller: &mut Caller,
) {
    if let Some(policies) = store.principal_policies(identity_arn) {
        caller.documents = policies.documents;
        // An attachment the collection resolved no document for is an entry of AWS's own
        // catalog this emulator does not carry: a grant it cannot read.
        caller.unreadable_attachment = !policies.uncarried.is_empty();
    }
    caller.boundary = store.permissions_boundary_document(identity_arn);
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

    /// RULE: the action this gate asks about is `elasticbeanstalk:<OperationName>`, so a
    /// policy scoped to this service's actions covers every operation of it and nothing
    /// outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document =
            r#"{"Statement":[{"Effect":"Allow","Action":"elasticbeanstalk:*","Resource":"*"}]}"#;
        for operation in [
            "CreatePlatformVersion",
            "CreateEnvironment",
            "UpdateTagsForResource",
        ] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            let reached = decision(document, &action, "*");
            assert_eq!(
                reached,
                Decision::Allowed,
                "case {operation:?}: expected elasticbeanstalk:* to allow {action}, got {reached:?}"
            );
        }
        let outside = decision(document, "s3:GetObject", "*");
        assert_eq!(
            outside,
            Decision::NotStated,
            "case \"another service\": expected elasticbeanstalk:* to grant nothing outside \
             this service, got {outside:?}"
        );
    }

    /// RULE: the refusal names the principal, the action and the resource the request
    /// addressed — every part taken from the request and the stored policies, and nothing
    /// else.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let principal = "arn:aws:sts::000000000000:assumed-role/eb-restricted/session-1";
        let resource = "arn:aws:elasticbeanstalk:us-west-2:000000000000:platform/custom/1.0.0";
        let action = format!("{ACTION_PREFIX}:CreatePlatformVersion");
        let message = not_authorized(principal, &action, resource);
        assert_eq!(
            message,
            format!(
                "User: {principal} is not authorized to perform: {action} on resource: \
                 {resource}"
            ),
            "case \"a default deny\": expected IAM's wording, got {message:?}"
        );
    }
}

