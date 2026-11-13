// SPDX-License-Identifier: CC0-1.0

//! Whether IAM lets a request to this service perform the action it asks for — the
//! authorization step an operation runs BEFORE it reads or writes the resource the
//! request names.
//!
//! AWS evaluates authorization at the IAM layer, before the service looks at the
//! resource, so a caller whose identity policies do not grant the action is told it is
//! not authorized rather than told anything about the resource, and the call performs
//! nothing. Each operation that takes this decision models `AccessDeniedException`
//! (HTTP 403), which is the refusal that decision is returned as.
//!
//! Three pieces, each held in exactly one place:
//!
//! 1. **who** — the principal the request's access key id speaks for, read from the SAME
//!    place `sts:GetCallerIdentity` answers from (`sts_store::resolve_request_caller` over
//!    `sts_store::long_term_identity`), so the principal a refusal blames is the one the
//!    caller reads back from STS for the credentials it signed with: the session a
//!    temporary credential was minted for, or the IAM user that holds a long-term key;
//! 2. **what it may do** — the identity-based policy documents the IAM store holds for
//!    that principal: the identity's inline policies and the default version of each
//!    managed policy attached to it (and, for a user, the ones its groups carry), read out
//!    of the IAM service's OWN store, so a policy written by `iam:PutRolePolicy` or
//!    `iam:PutUserPolicy` takes effect on the next request and deleting it lifts it again;
//! 3. **the decision** — [`::service_common::iam_authorization`], the ONE shared
//!    evaluation of those documents against `(action, resource)`, so this service and
//!    every other one handed the same documents answer alike: an explicit `Deny` wins,
//!    otherwise an `Allow` about the request grants it, and with neither the request
//!    falls to IAM's default deny.
//!
//! Only the store LOOKUPS live here; the decision does not. That is the same split the
//! gates of the other services that enforce authorization make (AWS B2B Data Interchange,
//! Amazon EVS, CloudWatch RUM, AWS Transfer Family), and the lookups stay in the service
//! crate for the reason those keep theirs there: hoisting them into `service-common` would
//! make every service that depends on that crate link the IAM and STS service crates.
//!
//! ## Which callers this bounds
//!
//! A request is evaluated when its credentials name a principal whose identity policies
//! this emulator can read: an assumed-role session, whose ROLE carries them, or a
//! long-term access key an IAM user of this account holds, where the USER carries them.
//! These are served rather than evaluated, since no identity policy here bounds them:
//!
//! * a request whose access key id no IAM user of this account holds and no session was
//!   minted for — the long-term key this emulator publishes at start, which is the
//!   account's own administrator (`sts_store::ambient_identity`), the position AWS
//!   account root credentials are in;
//! * a credential that speaks for neither a role nor a user of this account (an
//!   `sts:GetFederationToken` session, whose federated principal carries no identity
//!   policy of its own);
//! * an engine that carries no STS or IAM store, where no session and no user can exist;
//! * an expired session, which the layer that authenticates a request refuses, not this
//!   one;
//! * a principal carrying a managed policy no document can be read for — an AWS-managed
//!   ARN outside the catalog the IAM emulator publishes — whose grant is evidence this
//!   emulator does not have: refusing on it would deny access AWS permits. An explicit
//!   `Deny` in a document that CAN be read still refuses.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam_store;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;
use ::service_common::iam_authorization::{self as iam, Decision};

use crate::ops::types::AccessDeniedException;

/// This service's IAM action prefix: an action is `es:<OperationName>` ("Actions defined
/// by Amazon OpenSearch Service", IAM Service Authorization Reference), which is this
/// service's `arnNamespace` (`smithy_query(query="service_info")`) — the same string
/// [`super::ARN_NAMESPACE`] spells into every ARN it builds.
pub const ACTION_PREFIX: &str = super::ARN_NAMESPACE;

/// The IAM store this gate reads the identity policies out of.
type IamState =
    ::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State;

/// The IAM store's map of one kind of entity's INLINE policies:
/// `<entity key> -> <policy name> -> the policy`.
type InlinePolicies = BTreeMap<String, BTreeMap<String, iam_store::InlinePolicy>>;

/// The IAM store's map of one kind of entity's managed-policy ATTACHMENTS:
/// `<entity key> -> the ARNs attached to it`.
type Attachments = BTreeMap<String, BTreeSet<String>>;

/// The resource-name wildcard that stands for every resource of one type held by an
/// account and Region in a `Resource` element — IAM's own "any run of characters"
/// pattern.
const EVERY_RESOURCE: &str = "*";

/// The `Resource` an operation that addresses no ONE domain is decided against: every
/// domain of the account and Region the request was signed for.
///
/// This service's IAM actions take a DOMAIN resource, so an operation addressed by
/// something else — an application id, whose attachments link it to the data a domain
/// serves, or a package id, whose content a domain's indexes read — is decided against the
/// domain resource PATTERN rather than against one named domain. Built from the request,
/// so the decision does not depend on which domains the account holds.
pub fn account_domain_resource(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    super::domain_arn(EVERY_RESOURCE, &super::request_region(ctx.parts()))
}

/// The `Resource` an operation on an OpenSearch UI APPLICATION, or on one of the saved
/// object migration jobs that belong to one, is decided against: every application of the
/// account and Region the request was signed for.
///
/// The application plane carries its own service token in an ARN
/// ([`super::APPLICATION_ARN_NAMESPACE`]), so the resource an authorization decision about
/// an application is taken on is an `application/` ARN under that token rather than a
/// domain one — which is what the recorded refusal of a migration read names. Built from
/// the request, so the decision does not depend on which applications the account holds.
pub fn account_application_resource(ctx: &::smithy_server_runtime::OperationContext<'_>) -> String {
    ::engine::emulator_arn(
        super::APPLICATION_ARN_NAMESPACE,
        &super::request_region(ctx.parts()),
        &format!("application/{EVERY_RESOURCE}"),
    )
}

/// Authorize this request, as [`ACTION_PREFIX`] and `operation` name the action and
/// `resource` names the ARN the request addresses.
///
/// `Ok(())` for a request IAM permits (and for every caller this gate does not bound, see
/// the module docs); the modelled refusal otherwise. Call it BEFORE the store is read or
/// written, with the ARN the request NAMES rather than one resolved from a stored record,
/// so the decision does not depend on whether that resource exists — which is also what
/// keeps a refused call from telling an unauthorized caller whether it does.
///
/// The caller is resolved through STS's own `resolve_request_caller` over
/// `long_term_identity`, which first materialises the session expiries the clock has
/// already made due; that write is owed at the same instant whether or not anybody
/// called, which is why a REPORTING operation of this service may take this decision too.
pub fn authorize<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    operation: &str,
    resource: &str,
) -> Result<(), AccessDeniedException> {
    let Some(caller) = caller(engine, ctx) else {
        return Ok(());
    };
    let action = format!("{ACTION_PREFIX}:{operation}");
    // Identity-based documents name no principal, so the principal spellings a
    // resource-based policy would be matched against are not part of this decision.
    let request = iam::Request::new(&action, resource, &[]);
    match iam::identity_decision(&caller.documents, &request) {
        Decision::Allowed => Ok(()),
        // An explicit `Deny` in a document this emulator CAN read refuses the request
        // whatever else the role carries, which is why it is answered before the
        // unreadable attachment below.
        Decision::Denied => Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            "with an explicit deny in an identity-based policy",
        )),
        // Nothing the store holds grants the action — but a grant it cannot READ may.
        Decision::NotStated if caller.unreadable_attachment => Ok(()),
        Decision::NotStated => Err(not_authorized(
            &caller.arn,
            &action,
            resource,
            &format!("because no identity-based policy allows the {action} action"),
        )),
    }
}

/// The ARN of the principal a request speaks for, as `sts:GetCallerIdentity` reports it.
///
/// What a decision taken OUTSIDE a modelled operation's own gate needs — the domain's REST
/// data plane ([`crate::service::domain_rest`]), which names no modelled operation and so
/// has no `authorize` call of its own, yet has to know who is asking. The same resolution
/// [`caller`] performs, so the principal a data-plane refusal names is the one the control
/// plane names for the same credential.
///
/// `None` for an engine that carries no STS or IAM store, where no session and no user can
/// exist, and for an expired session, which the layer that authenticates a request refuses.
pub fn request_principal_arn<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<String> {
    let sts = sts_store::service_state(engine)?;
    let iam = iam_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    let long_term = sts_store::long_term_identity(Some(&iam.lock()), &access_key_id);
    let identity = sts_store::resolve_request_caller(
        &mut sts.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term,
    )
    .ok()?
    .identity;
    Some(identity.arn)
}

/// What this emulator can show about the identity-based policies in force on the principal
/// a request speaks for, or `None` for a caller this gate does not bound (see the module
/// docs) — including the account's own published credential, for which "no evidence" is the
/// answer rather than "no permissions".
///
/// The companion of [`request_principal_arn`] for a decision taken OUTSIDE a modelled
/// operation's own gate, and the SAME lookup [`authorize`] performs, so the documents a
/// data-plane decision reads are the ones the control plane reads for that credential. The
/// grant this emulator CANNOT read is reported beside them ([`HeldIdentity`]) rather than
/// collapsed into "no documents": to a decision that also reads a resource-based policy the
/// two mean different things.
pub fn request_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<HeldIdentity> {
    caller(engine, ctx).map(HeldIdentity::of_caller)
}

/// What this emulator can show about the identity-based policies in force on ONE principal:
/// the documents it holds for it, and whether the principal carries a grant whose document
/// cannot be read here.
///
/// The pair every decision of this service reads the identity side from, so the gate that
/// authorizes a modelled operation and the one that authorizes a data-plane request are
/// shown the same two facts about a caller.
#[derive(Debug, Clone, Default)]
pub struct HeldIdentity {
    /// The identity-based policy documents in force on the principal.
    pub documents: Vec<String>,
    /// Set when the principal carries a managed policy whose document cannot be read here —
    /// an AWS-managed ARN outside the catalog the IAM emulator publishes — so what that
    /// grant allows is unknown (see the module docs).
    pub unreadable_grant: bool,
}

impl HeldIdentity {
    /// The evidence as a data-plane decision reads it ([`super::ingest::iam_decision`]).
    #[must_use]
    pub fn policies(&self) -> super::ingest::IdentityPolicies<'_> {
        super::ingest::IdentityPolicies {
            documents: Some(&self.documents),
            unreadable_grant: self.unreadable_grant,
        }
    }

    /// What one resolved caller carries.
    fn of_caller(caller: Caller) -> Self {
        Self {
            documents: caller.documents,
            unreadable_grant: caller.unreadable_attachment,
        }
    }
}

/// What this emulator can show about the identity-based policies in force on the IAM ROLE
/// `role_arn` names, or `None` when this emulator holds none for it: an engine carrying no
/// IAM store, a value that is not a role ARN, or a role the IAM store does not hold.
///
/// The same lookup [`identity_policies`] performs for a request's own principal, reached
/// by a caller that does not hold the request — a producer OUTSIDE this service writing to
/// a domain as the role it was configured with ([`super::ingest`]), where the principal is
/// the configured role rather than whoever signed. `None` is "no evidence", not "no
/// permissions": a caller decides what to do with that, the way this module's own gate
/// does for a grant it cannot read.
pub fn role_identity<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
) -> Option<HeldIdentity> {
    let iam = iam_store::service_state(engine)?;
    let role_name = ::service_common::arn::parse(role_arn)
        .filter(|parsed| parsed.vendor == IAM_ARN_NAMESPACE)
        .and_then(|parsed| {
            parsed
                .resource
                .strip_prefix("role/")
                .and_then(|path| path.split('/').next_back())
                .filter(|name| !name.is_empty())
                .map(::std::string::ToString::to_string)
        })?;
    let iam = iam.lock();
    // A role the store does not hold carries no policy this emulator can read, which is
    // not the same as a role that carries none.
    if !iam.roles.contains_key(&iam_store::name_key(&role_name)) {
        return None;
    }
    Some(HeldIdentity::of_caller(identity_policies(
        &iam,
        NamedIdentity::Role(&role_name),
    )))
}

/// Whether `principal_arn` is the principal the account's OWN published credential speaks
/// for — the long-term key this emulator publishes at start
/// ([`sts_store::ambient_identity`]), which is the position AWS account root credentials are
/// in.
///
/// No identity-based policy bounds it, which is why [`caller`] answers `None` for it and this
/// gate SERVES it (see the module docs). A decision taken OUTSIDE a modelled operation's own
/// gate — the domain's data plane, where a resource-based policy is read as well — asks this
/// to tell that caller apart from a principal whose identity documents simply could not be
/// read: the two are both "no documents", and only this one is a caller nothing bounds.
pub fn is_account_credential(principal_arn: &str) -> bool {
    !principal_arn.is_empty() && principal_arn == sts_store::ambient_identity().arn
}

/// The refusal IAM states an authorization failure with: who was refused, which action,
/// on which resource, and which of IAM's two rules refused it. Every value in it comes
/// from the request and from the stored policies.
pub(crate) fn not_authorized(
    principal: &str,
    action: &str,
    resource: &str,
    reason: &str,
) -> AccessDeniedException {
    AccessDeniedException {
        message: Some(format!(
            "User: {principal} is not authorized to perform: {action} on resource: \
             {resource} {reason}"
        )),
    }
}

/// The principal a request speaks for, and the identity-based documents in force on it.
#[derive(Debug, Clone, Default)]
struct Caller {
    /// The principal's ARN, as `sts:GetCallerIdentity` reports it — what a refusal names.
    arn: String,
    /// The identity-based policy documents in force on the principal.
    documents: Vec<String>,
    /// Set when the principal carries a managed policy whose document cannot be read
    /// here, so what it grants is unknown. See the module docs.
    unreadable_attachment: bool,
}

/// The principal this request speaks for with the documents in force on it, or `None` for
/// a caller this gate does not bound (see the module docs).
fn caller<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> Option<Caller> {
    let sts = sts_store::service_state(engine)?;
    let iam = iam_store::service_state(engine)?;
    let access_key_id = ::engine::extract_akid(&ctx.parts().headers);
    // A long-term access key speaks for the IAM user that HOLDS it — the identity STS
    // itself reports for that key — and for the account's own published credential when
    // no user of this account holds it.
    let long_term = sts_store::long_term_identity(Some(&iam.lock()), &access_key_id);
    let identity = sts_store::resolve_request_caller(
        &mut sts.lock(),
        &access_key_id,
        SystemTime::now(),
        ctx.credential_store(),
        long_term,
    )
    .ok()?
    .identity;
    // The account's own published credential carries no identity-based policy at all, so
    // nothing here bounds it.
    if identity.arn == sts_store::ambient_identity().arn {
        return None;
    }
    let mut caller = identity_policies(&iam.lock(), named_identity(&identity.arn)?);
    caller.arn = identity.arn;
    Some(caller)
}

/// The IAM identity whose identity-based policies are in force on a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NamedIdentity<'a> {
    /// A role — the principal an assumed-role session speaks for.
    Role(&'a str),
    /// An IAM user — the principal its own long-term access key speaks for.
    User(&'a str),
}

/// Which identity the principal ARN `identity_arn` names, or `None` for a principal form
/// this gate does not bound (see the module docs).
fn named_identity(identity_arn: &str) -> Option<NamedIdentity<'_>> {
    if let Some(role_name) = assumed_role_name(identity_arn) {
        return Some(NamedIdentity::Role(role_name));
    }
    iam_user_name(identity_arn).map(NamedIdentity::User)
}

/// AWS STS's `arnNamespace`, which the ARN of a session principal carries.
const STS_ARN_NAMESPACE: &str = "sts";

/// AWS IAM's `arnNamespace`, which the ARN of an IAM entity carries.
const IAM_ARN_NAMESPACE: &str = "iam";

/// The role name an assumed-role session's ARN names
/// (`arn:<partition>:sts::<account>:assumed-role/<RoleName>/<SessionName>`), or `None` for
/// every other principal form.
fn assumed_role_name(identity_arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(identity_arn)?;
    let role_name = parsed
        .resource
        .strip_prefix("assumed-role/")?
        .split('/')
        .next()?;
    (parsed.vendor == STS_ARN_NAMESPACE && !role_name.is_empty()).then_some(role_name)
}

/// The user name an IAM user's ARN names
/// (`arn:<partition>:iam::<account>:user/<path…>/<UserName>`): the LAST segment of the
/// resource, since the path is optional and only the last segment is the name. `None` for
/// every other principal form.
fn iam_user_name(identity_arn: &str) -> Option<&str> {
    let parsed = ::service_common::arn::parse(identity_arn)?;
    let user_name = parsed
        .resource
        .strip_prefix("user/")?
        .split('/')
        .next_back()?;
    (parsed.vendor == IAM_ARN_NAMESPACE && !user_name.is_empty()).then_some(user_name)
}

/// The identity policies in force on one identity: for a ROLE its inline policies and the
/// document of each managed policy attached to it, and for a USER those plus the ones in
/// force on every group it belongs to, because a user also holds the permissions of its
/// groups. An identity the IAM store holds nothing for carries none, which IAM's default
/// deny refuses.
fn identity_policies(iam: &IamState, named: NamedIdentity<'_>) -> Caller {
    let mut caller = Caller::default();
    match named {
        NamedIdentity::Role(role_name) => {
            let role_key = iam_store::name_key(role_name);
            collect_entity_policies(
                iam,
                &iam.role_inline_policies,
                &iam.role_attached_policies,
                &role_key,
                &mut caller,
            );
        }
        NamedIdentity::User(user_name) => {
            let user_key = iam_store::name_key(user_name);
            collect_entity_policies(
                iam,
                &iam.user_inline_policies,
                &iam.user_attached_policies,
                &user_key,
                &mut caller,
            );
            for group_key in iam.group_keys_held_by(&user_key) {
                collect_entity_policies(
                    iam,
                    &iam.group_inline_policies,
                    &iam.group_attached_policies,
                    group_key,
                    &mut caller,
                );
            }
        }
    }
    caller
}

/// Add the documents one entity carries to `caller`: the inline policies embedded in it,
/// plus the default version of each managed policy attached to it — the two kinds of
/// identity policy IAM evaluates, read the same way whichever kind of entity carries
/// them.
fn collect_entity_policies(
    iam: &IamState,
    inline: &InlinePolicies,
    attached: &Attachments,
    entity_key: &str,
    caller: &mut Caller,
) {
    caller.documents.extend(
        inline
            .get(entity_key)
            .into_iter()
            .flat_map(|held| held.values())
            .map(|policy| policy.policy_document.clone()),
    );
    for arn in attached.get(entity_key).into_iter().flatten() {
        // A managed policy is either the ACCOUNT's, whose versions the store holds, or
        // one of AWS's own, whose document the IAM emulator publishes as a catalog entry.
        let document = iam
            .policy_by_arn(arn)
            .and_then(default_version_document)
            .or_else(|| {
                iam_store::aws_managed_policy(arn).map(|managed| managed.document.to_string())
            });
        match document {
            Some(document) => caller.documents.push(document),
            // An AWS-managed ARN the catalog does not carry is a grant AWS owns and this
            // emulator cannot read; an attachment naming a CUSTOMER-managed policy the
            // account does not hold grants nothing, because the account is the authority
            // on its own policies.
            None if iam_store::is_aws_managed_policy_arn(arn) => {
                caller.unreadable_attachment = true;
            }
            None => {}
        }
    }
}

/// The document of an account-managed policy's DEFAULT version — the one in force on
/// every entity it is attached to. `None` when the store holds no document for it.
fn default_version_document(managed: &iam_store::ManagedPolicy) -> Option<String> {
    managed
        .policy
        .default_version_id
        .as_deref()
        .and_then(|version_id| managed.versions.get(version_id))
        .or_else(|| {
            managed
                .versions
                .values()
                .find(|version| version.is_default_version)
        })
        .and_then(|version| version.document.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision one stored document reaches about an action on a resource, through
    /// the shared evaluator this gate consults.
    fn decision(document: &str, action: &str, resource: &str) -> Decision {
        iam::identity_decision(
            &[document.to_string()],
            &iam::Request::new(action, resource, &[]),
        )
    }

    /// RULE: the action this gate asks about is `es:<OperationName>`, so a policy scoped
    /// to this service's actions covers every operation of it and nothing outside it.
    #[test]
    fn the_action_asked_about_carries_this_services_prefix() {
        let document = r#"{"Statement":[{"Effect":"Allow","Action":"es:*","Resource":"*"}]}"#;
        for operation in ["UpdateIndex", "GetIndex", "CreateIndex", "DeleteIndex"] {
            let action = format!("{ACTION_PREFIX}:{operation}");
            assert_eq!(
                decision(document, &action, "*"),
                Decision::Allowed,
                "case {operation:?}: expected es:* to allow {action}, got {:?}",
                decision(document, &action, "*")
            );
        }
        assert_eq!(
            decision(document, "s3:GetObject", "*"),
            Decision::NotStated,
            "case \"another service\": expected es:* to grant nothing outside this service"
        );
    }

    /// RULE: a principal ARN names the identity whose policies are read — a session ARN
    /// the role it is a session of, a user ARN the user itself — and every other form
    /// names none, so its request is not evaluated here.
    #[test]
    fn a_principal_arn_names_the_identity_its_policies_are_read_from() {
        // (case, the ARN, the role it names, the user it names)
        let rows: &[(&str, &str, Option<&str>, Option<&str>)] = &[
            (
                "an assumed-role session",
                "arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
                Some("search-reader"),
                None,
            ),
            (
                "a session name carrying a slash",
                "arn:aws:sts::000000000000:assumed-role/search-reader/one/two",
                Some("search-reader"),
                None,
            ),
            (
                "an IAM user",
                "arn:aws:iam::000000000000:user/search-user",
                None,
                Some("search-user"),
            ),
            (
                "an IAM user under a path",
                "arn:aws:iam::000000000000:user/team/search-user",
                None,
                Some("search-user"),
            ),
            (
                "the account's own identity",
                "arn:aws:iam::000000000000:root",
                None,
                None,
            ),
            (
                "a federated user, whose own policies IAM does not hold",
                "arn:aws:sts::000000000000:federated-user/search-federated",
                None,
                None,
            ),
            (
                "a user spelling under another vendor",
                "arn:aws:es:us-east-1:000000000000:user/search-user",
                None,
                None,
            ),
            ("a value that is no ARN", "AKIDTESTONLY", None, None),
        ];
        for (case, arn, role, user) in rows {
            assert_eq!(
                assumed_role_name(arn),
                *role,
                "case {case:?}: expected the role {role:?}, got {:?}",
                assumed_role_name(arn)
            );
            assert_eq!(
                iam_user_name(arn),
                *user,
                "case {case:?}: expected the user {user:?}, got {:?}",
                iam_user_name(arn)
            );
            let named = named_identity(arn);
            let expected = role
                .map(NamedIdentity::Role)
                .or_else(|| user.map(NamedIdentity::User));
            assert_eq!(
                named, expected,
                "case {case:?}: expected the identity {expected:?}, got {named:?}"
            );
        }
    }

    /// RULE: the refusal names the principal, the action, the resource and which of IAM's
    /// rules refused — every part taken from the request and the stored policies.
    #[test]
    fn the_refusal_names_the_principal_the_action_and_the_resource() {
        let action = format!("{ACTION_PREFIX}:UpdateIndex");
        let refusal = not_authorized(
            "arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
            &action,
            "arn:aws:es:us-east-1:000000000000:domain/search-target",
            &format!("because no identity-based policy allows the {action} action"),
        );
        let message = refusal.message.unwrap_or_default();
        for part in [
            "User: arn:aws:sts::000000000000:assumed-role/search-reader/session-1",
            " is not authorized to perform: es:UpdateIndex",
            " on resource: arn:aws:es:us-east-1:000000000000:domain/search-target",
            " because no identity-based policy allows the es:UpdateIndex action",
        ] {
            assert!(
                message.contains(part),
                "case {part:?}: expected the refusal to carry it, got {message:?}"
            );
        }
    }
}
