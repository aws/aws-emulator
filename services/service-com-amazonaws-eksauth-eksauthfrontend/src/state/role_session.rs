// SPDX-License-Identifier: CC0-1.0

//! What the credentials this service vends ARE: a SESSION OF THE ROLE the pod identity
//! association binds the service account to.
//!
//! `AssumeRoleForPodIdentity` answers with "the Amazon Web Services Signature Version 4
//! type of temporary credentials" and an `assumedRoleUser` — "an object with the permanent
//! IAM role identity and the temporary session name" — so what the caller is handed is the
//! same thing `sts:AssumeRole` hands out: a temporary credential that SPEAKS FOR that role.
//! "The temporary security credentials … have the permissions of the role" (`AssumeRole`),
//! which means what the pod may then do is what the role's own policies allow, narrowed by
//! the association's own `policy` where it carries one — not what the caller of this
//! operation could do.
//!
//! That is why the credential is minted through the token service's shared role-session
//! mint ([`sts_store::mint_role_session`]) rather than created here: that mint is the one
//! place a session of a role is recorded where EVERY service resolves a temporary
//! credential's principal, so a request this pod then signs is judged on the role's
//! policies by whichever service serves it. Minting a bare credential instead would hand
//! out a session that speaks for nobody, and every service would serve it as the ambient
//! identity — which can do anything.
//!
//! Two values follow from the session rather than from this service's own records:
//!
//! * `assumedRoleUser.arn` is the ARN of the SESSION,
//!   `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`, which is
//!   what AWS reports for an assumed-role identity — not the ARN of the role, which is the
//!   `roleArn` the caller already holds on the association;
//! * `assumedRoleUser.assumeRoleId` is "a unique identifier that contains the role ID, a
//!   colon (:), and the role session name", so the role's OWN id is its first half: IAM's
//!   id for the role when the account holds one, and the id this service minted for the
//!   role otherwise ([`ComAmazonawsEksauthEksauthfrontendState::role_ids`]).
//!
//! The session also carries the pod identity SESSION TAGS ([`session_tags`]) and the
//! association's session policy, so an ABAC policy conditioned on
//! `aws:PrincipalTag/kubernetes-namespace` and a policy that narrows the role both decide
//! the pod's later requests.

use ::std::time::{Duration, SystemTime};

use ::emulator_server::CredentialStore;
use ::service_com_amazonaws_sts_awssecuritytokenservicev20110615::state as sts_store;

use super::ServiceAccountIdentity;

/// The session tag naming the ARN of the cluster the pod runs in.
///
/// The six tags below are the ones Amazon EKS documents EKS Pod Identity as putting on
/// the session it vends ("EKS Pod Identity adds session tags… you can use these session
/// tags for attribute-based access control"), so a policy conditioned on
/// `aws:PrincipalTag/<key>` decides a pod's requests by them.
pub const SESSION_TAG_CLUSTER_ARN: &str = "eks-cluster-arn";
/// The session tag naming the cluster the pod runs in.
pub const SESSION_TAG_CLUSTER_NAME: &str = "eks-cluster-name";
/// The session tag naming the pod's Kubernetes namespace.
pub const SESSION_TAG_NAMESPACE: &str = "kubernetes-namespace";
/// The session tag naming the pod's Kubernetes service account.
pub const SESSION_TAG_SERVICE_ACCOUNT: &str = "kubernetes-service-account";
/// The session tag naming the pod itself.
pub const SESSION_TAG_POD_NAME: &str = "kubernetes-pod-name";
/// The session tag naming that pod's `metadata.uid`.
pub const SESSION_TAG_POD_UID: &str = "kubernetes-pod-uid";

/// The session tags EKS Pod Identity puts on the session it vends for `identity` in
/// `cluster`.
///
/// Every value comes from the cluster's record and from the token the cluster issued —
/// never from the request — so a caller cannot choose the tags its own session is judged
/// by. A tag whose value the token does not carry (the pod's name and uid, for a token
/// that was not bound to a pod) is left off rather than carried as an empty value: a tag
/// with no value is a tag the session does not have.
#[must_use]
pub fn session_tags(
    cluster_name: &str,
    cluster_arn: &str,
    identity: &ServiceAccountIdentity,
) -> Vec<(String, String)> {
    let mut tags = vec![
        (SESSION_TAG_CLUSTER_ARN.to_string(), cluster_arn.to_string()),
        (
            SESSION_TAG_CLUSTER_NAME.to_string(),
            cluster_name.to_string(),
        ),
        (
            SESSION_TAG_NAMESPACE.to_string(),
            identity.namespace.clone(),
        ),
        (
            SESSION_TAG_SERVICE_ACCOUNT.to_string(),
            identity.service_account.clone(),
        ),
    ];
    if let Some(pod_name) = identity.pod_name.clone() {
        tags.push((SESSION_TAG_POD_NAME.to_string(), pod_name));
    }
    if let Some(pod_uid) = identity.pod_uid.clone() {
        tags.push((SESSION_TAG_POD_UID.to_string(), pod_uid));
    }
    tags
}

/// What a session of a pod identity association's role is minted for.
#[derive(Debug, Clone)]
pub struct PodIdentitySessionPlan {
    /// The role the association binds the service account to.
    pub role_arn: String,
    /// The role the association CHAINS into, when it names one: "you can set a target IAM
    /// role in the same or a different account" (`PodIdentityAssociation$targetRoleArn`).
    /// The credentials vended are then the TARGET role's, assumed by the association's own
    /// role.
    pub target_role_arn: Option<String>,
    /// The role session name: `eks-<cluster name>-<pod name>-<random UUID>`, the form the
    /// model documents for the second half of `assumeRoleId`.
    pub session_name: String,
    /// The id to use for the role when the account's IAM holds no record of it — the one
    /// this service minted for the role.
    pub fallback_role_id: String,
    /// How long the session lasts.
    pub duration: Duration,
    /// The pod identity session tags ([`session_tags`]), empty for an association whose
    /// `disableSessionTags` turns them off.
    pub tags: Vec<(String, String)>,
    /// The association's own session policy, when it carries one.
    pub inline_policy: Option<String>,
    /// The external id the control plane generated for the association's target-role
    /// chain, when it has one.
    pub external_id: Option<String>,
}

/// One vended session: the credential to hand the caller, and the two identity values the
/// answer reports for it.
#[derive(Debug, Clone)]
pub struct VendedSession {
    pub access_key_id: String,
    pub secret_access_key: String,
    pub session_token: String,
    pub expires_at: SystemTime,
    /// `arn:<partition>:sts::<account>:assumed-role/<role name>/<session name>`.
    pub assumed_role_user_arn: String,
    /// `<role id>:<session name>`.
    pub assume_role_id: String,
}

/// Why no session could be vended for an exchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionRefusal {
    /// Nothing could be minted at all: a `roleArn` that is not an ARN, or a bounded
    /// session registry with no room left.
    Unvendable,
    /// The association names a TARGET role its own role may not assume: the account holds
    /// no such role, or that role's trust policy does not admit the association's role
    /// with the external id the control plane generated for the chain.
    TargetRoleNotTrusted,
}

/// Mint the session `plan` describes, and record it everywhere a request signed with it is
/// resolved from.
///
/// For an association that names a TARGET role, what is vended is a session of THAT role —
/// assumed by the association's own role, and only if the target role's trust policy admits
/// it with the association's external id, which is what that generated value is for. The
/// association's own role then appears in no credential the caller holds: the pod is handed
/// the end of the chain, as EKS Pod Identity hands it.
pub fn vend_pod_identity_session<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    now: SystemTime,
    plan: &PodIdentitySessionPlan,
) -> Result<VendedSession, SessionRefusal> {
    let association_role = resolved_role(engine, &plan.role_arn, &plan.fallback_role_id)
        .ok_or(SessionRefusal::Unvendable)?;
    let Some(target_role_arn) = plan.target_role_arn.as_deref() else {
        return mint(engine, store, now, plan, &association_role, plan.tags.clone());
    };
    let target = chained_role(engine, &association_role, target_role_arn, plan)?;
    // Role chaining does not carry the first session's tags into the second: a session tag
    // travels only where it was declared transitive, and EKS Pod Identity declares none.
    mint(engine, store, now, plan, &target, Vec::new())
}

/// The TARGET role an association chains into, resolved and checked: the account must hold
/// it, and its trust policy must admit the association's role with the external id the
/// control plane generated for the chain.
///
/// The decision is the token service's own trust-policy evaluation
/// ([`sts_store::ResolvedRole::trust_policy_admits`]) over the context an assume by that
/// role carries, so a trust policy that conditions on `sts:ExternalId`, on
/// `aws:PrincipalArn` or on the session's own name decides this chain exactly as it decides
/// an `sts:AssumeRole` call.
fn chained_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    association_role: &ResolvedRole,
    target_role_arn: &str,
    plan: &PodIdentitySessionPlan,
) -> Result<ResolvedRole, SessionRefusal> {
    let iam = sts_store::iam_state(engine).ok_or(SessionRefusal::TargetRoleNotTrusted)?;
    let target = {
        let held = iam.lock();
        sts_store::resolve_role(&held, target_role_arn).ok_or(SessionRefusal::TargetRoleNotTrusted)?
    };
    let session_arn = assumed_role_user_arn(association_role, &plan.session_name);
    // The spellings a trust policy may name the assuming identity by: the role itself and
    // the session of it this chain assumes from.
    let principals = vec![association_role.arn.clone(), session_arn.clone()];
    let mut context = vec![
        (
            "aws:PrincipalArn".to_string(),
            association_role.arn.clone(),
        ),
        (
            "aws:PrincipalAccount".to_string(),
            association_role.account.clone(),
        ),
        ("aws:PrincipalType".to_string(), "AssumedRole".to_string()),
        (
            "aws:userid".to_string(),
            format!("{}:{}", association_role.id, plan.session_name),
        ),
        (
            "sts:RoleSessionName".to_string(),
            plan.session_name.clone(),
        ),
    ];
    if let Some(external_id) = plan.external_id.clone() {
        context.push((
            sts_store::EXTERNAL_ID_CONDITION_KEY.to_string(),
            external_id,
        ));
    }
    if !target.trust_policy_admits(
        ASSUME_ROLE_ACTION,
        &principals,
        &context,
        sts_store::ASSUME_ROLE_PUBLISHED_CONDITION_KEYS,
    ) {
        ::tracing::info!(
            target_role_arn = %target_role_arn,
            "an EKS Pod Identity association's target role does not admit the association's \
             role"
        );
        return Err(SessionRefusal::TargetRoleNotTrusted);
    }
    let parsed =
        ::service_common::arn::parse(&target.arn).ok_or(SessionRefusal::TargetRoleNotTrusted)?;
    Ok(ResolvedRole {
        arn: target.arn.clone(),
        name: target.role_name,
        id: target.role_id,
        account: target.account,
        partition: parsed.partition.to_string(),
    })
}

/// The IAM action a role chain is authorized under, which is what a trust policy grants.
const ASSUME_ROLE_ACTION: &str = "sts:AssumeRole";

/// Mint one session of `role` through the token service's shared role-session mint.
fn mint<Eng: Send + Sync + 'static>(
    engine: &Eng,
    store: &CredentialStore,
    now: SystemTime,
    plan: &PodIdentitySessionPlan,
    role: &ResolvedRole,
    tags: Vec<(String, String)>,
) -> Result<VendedSession, SessionRefusal> {
    let assumed_role_user_arn = assumed_role_user_arn(role, &plan.session_name);
    let assume_role_id = format!("{}:{}", role.id, plan.session_name);
    let identity = sts_store::Identity {
        user_id: assume_role_id.clone(),
        account: role.account.clone(),
        arn: assumed_role_user_arn.clone(),
    };
    // The modelled `durationSeconds` of a session is an `i32`; a span past what that holds
    // is clamped to it rather than wrapping into a session that has already expired.
    let duration_seconds = i32::try_from(plan.duration.as_secs()).unwrap_or(i32::MAX);
    let registry = sts_store::service_state(engine).ok_or(SessionRefusal::Unvendable)?;
    let minted = sts_store::mint_role_session(
        engine,
        store,
        &registry,
        now,
        sts_store::NewSession {
            tags,
            // The association's own policy narrows what the session may do: "the
            // permissions for the Pod Identity association"
            // (`PodIdentityAssociation$policy`) is a session policy, and the service
            // deciding a request signed with this credential reads it from the session.
            policies: sts_store::SessionPolicies {
                inline: plan.inline_policy.clone(),
                managed_arns: Vec::new(),
                ..sts_store::SessionPolicies::default()
            },
            external_id: plan.external_id.clone(),
            role_arn: Some(role.arn.clone()),
            ..sts_store::NewSession::new(
                identity,
                sts_store::SessionKind::AssumedRole,
                duration_seconds,
            )
        },
    )
    .map_err(|_full| SessionRefusal::Unvendable)?;
    Ok(VendedSession {
        access_key_id: minted.credentials.access_key_id,
        secret_access_key: minted.credentials.secret_access_key,
        session_token: minted.credentials.session_token,
        expires_at: minted.session.expires_at,
        assumed_role_user_arn,
        assume_role_id,
    })
}

/// The ARN of the session `session_name` of `role`, which is what AWS reports for an
/// assumed-role identity.
fn assumed_role_user_arn(role: &ResolvedRole, session_name: &str) -> String {
    format!(
        "arn:{}:sts::{}:assumed-role/{}/{session_name}",
        role.partition, role.account, role.name
    )
}

/// What a session of one role is named from.
struct ResolvedRole {
    /// The role's ARN, exactly as the association names it — the spelling a policy names
    /// the role by.
    arn: String,
    /// Its name, which the session's own ARN carries.
    name: String,
    /// Its unique id, which `assumeRoleId` is built from.
    id: String,
    /// The account whose IAM holds it, read off its ARN.
    account: String,
    /// The partition that ARN names, so a session of a role of another partition is named
    /// in that partition.
    partition: String,
}

/// The role `role_arn` names: its record in the account's IAM when that store holds one,
/// and the ARN's own reading otherwise.
///
/// An emulator whose IAM holds no record of the role still vends a session of it — the
/// association named it, and refusing here would make the exchange depend on a record EKS
/// does not require — with the id this service minted for the role standing in for IAM's.
///
/// `None` only for a value that is not an ARN at all, which names no role in any account.
fn resolved_role<Eng: Send + Sync + 'static>(
    engine: &Eng,
    role_arn: &str,
    fallback_role_id: &str,
) -> Option<ResolvedRole> {
    let parsed = ::service_common::arn::parse(role_arn)?;
    // An IAM role's resource is `role/<path><name>`, and IAM addresses the role by its
    // NAME, so the path it sits under is not part of it.
    let name = parsed
        .resource
        .strip_prefix("role/")
        .filter(|name| !name.is_empty())?
        .rsplit('/')
        .next()?
        .to_string();
    let held = sts_store::iam_state(engine).and_then(|iam| {
        let store = iam.lock();
        store
            .roles
            .values()
            .find(|role| role.arn == role_arn)
            .map(|role| (role.role_name.clone(), role.role_id.clone()))
    });
    let (name, id) = match held {
        Some((held_name, held_id)) => (held_name, held_id),
        None => (name, fallback_role_id.to_string()),
    };
    Some(ResolvedRole {
        arn: role_arn.to_string(),
        name,
        id,
        account: parsed.account.to_string(),
        partition: parsed.partition.to_string(),
    })
}
