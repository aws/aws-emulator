// SPDX-License-Identifier: CC0-1.0

//! The EKS Auth plane this service PROGRAMS: the cluster, its pod identity
//! associations and the service account tokens its Kubernetes API server issues.
//!
//! `AssumeRoleForPodIdentity` is bound to the EKS Auth service, not to this one:
//! the EKS Pod Identity Agent presents a pod's projected service account token
//! there and is answered the credentials of the role the cluster's association
//! binds that service account to. That service models no create, list or delete —
//! "the cluster and the EKS Pod Identity association that operation authenticates
//! against are created through the EKS control plane", which is THIS service — so
//! it owns a registration seam for the control plane to drive, and this module is
//! the one place that drives it.
//!
//! Three things travel across it, and nothing else:
//!
//! * the CLUSTER, when `CreateCluster` provisions one and when `DeleteCluster`
//!   takes it away ([`register_cluster`], [`forget_cluster`]);
//! * its pod identity ASSOCIATIONS, whenever the set this service holds for a
//!   cluster changes — a create, a re-point, a delete, an add-on install
//!   ([`program_cluster_associations`]). The whole set is stated each time, so an
//!   association this service no longer holds resolves to nothing there: that is
//!   what makes an exchange against a DELETED association fail;
//! * the TOKENS this cluster's API server issues
//!   ([`issue_service_account_token`]), because the plane that answers for a
//!   presented token is the one that must hold the record of what was issued.
//!   There is therefore ONE registry of issued tokens rather than a copy on each
//!   side. The same minting records the issuance in the shared registry a
//!   redeemer of a WEB IDENTITY reads ([`record_redeemable_identity`]), because a
//!   projected service account token is also the web identity
//!   `sts:AssumeRoleWithWebIdentity` exchanges for a role session.
//!
//! Nothing travels the other way: this service reads no decision of that plane.

use ::service_com_amazonaws_eksauth_eksauthfrontend::state::{
    self as auth_state, ComAmazonawsEksauthEksauthfrontendState as AuthStore,
    ProgrammedAssociation, ServiceAccountTokenRequest,
};

use super::kubernetes_objects::TokenPlan;
use super::ComAmazonawsEksAwswesleyfrontendState as ServiceStore;

/// The EKS Auth store in the same shared engine, as this service reaches it.
pub type AuthPlane = ::engine::ServiceState<AuthStore>;

// WHICH cluster a value programmed onto that plane belongs to is its NAME, not its
// `(region, name)` key: `AssumeRoleForPodIdentity` is addressed as
// `/clusters/{clusterName}/assume-role-for-pod-identity` and carries no Region of its own,
// so a `clusterName` is the whole identity a request to it has, and the plane holds one
// cluster per name (`ComAmazonawsEksauthEksauthfrontendState::clusters`). Two clusters of
// one name in two Regions are two clusters HERE and one there: the later create's
// registration keeps the Region of the first, and whichever of them last changed its
// associations is the set that plane holds. That follows from the surface EKS Auth
// publishes rather than from a choice made here — nothing in a pod identity exchange says
// which Region the cluster is in.
//
// Every function below therefore takes the cluster's NAME from this service's own key, and
// passes the Region only where that plane mints a regional value of its own (an
// association's role id).

/// The EKS Auth plane in `engine`, or `None` for an emulator that carries none —
/// where a cluster is created and its associations recorded here as usual, and
/// nothing is programmed.
#[must_use]
pub fn auth_plane<Eng: Send + Sync + 'static>(engine: &Eng) -> Option<AuthPlane> {
    auth_state::service_state(engine)
}

/// Register the cluster `name` in `region` on the plane, so a token its API server
/// issues names a cluster that plane knows, publishing `oidc_issuer` — the issuer
/// URL the control plane announced for it — as the issuer those tokens carry.
pub fn register_cluster(plane: &AuthPlane, region: &str, name: &str, oidc_issuer: Option<&str>) {
    let mut store = plane.lock();
    match store.register_cluster(name, region) {
        // The issuer a cluster's tokens name is the one its control plane PUBLISHES
        // (`Cluster$identity.oidc.issuer`): that is the URL an account registers an IAM
        // OpenID Connect provider for, so a token naming anything else could be federated
        // against no provider at all.
        Some(cluster) => {
            if let Some(issuer) = oidc_issuer {
                cluster.publish_oidc_issuer(issuer);
            }
        }
        None => {
            // A name EKS Auth itself refuses can reach no request there, so nothing is
            // registered for it; the cluster is still this service's.
            ::tracing::warn!(
                cluster = %name,
                "the EKS Auth plane holds no cluster of this name, so no pod identity token can \
                 be exchanged for it"
            );
        }
    }
}

/// Forget the cluster `name` on the plane — what deleting the cluster does to it:
/// the tokens its API server issued and the associations it held go with it. The
/// issuances recorded for a redeemer of those tokens go too, because a token of an
/// issuer that no longer exists can be verified against no key set at all.
pub fn forget_cluster(
    plane: &AuthPlane,
    name: &str,
    redeemers: Option<&WebIdentityRedeemers>,
) {
    let announced = {
        let mut store = plane.lock();
        // Read before the cluster goes, because the issuer it announced is held on its
        // own record there.
        let announced = store.cluster(name).map(|cluster| cluster.oidc_issuer.clone());
        store.forget_cluster(name);
        announced
    };
    if let (Some(issuer), Some(redeemers)) = (announced, redeemers) {
        redeemers.lock().forget_issuer(&issuer);
    }
}

/// Program the pod identity associations this service holds for the cluster keyed
/// `key` onto the plane, under the ids and ARNs this service published for them.
///
/// Called wherever that set changes, so the plane and the control plane never
/// disagree: a created association can be exchanged, a re-pointed one is exchanged
/// for its new role, and a deleted one is exchanged for nothing.
pub fn program_cluster_associations(
    plane: &AuthPlane,
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
) {
    let programmed: Vec<ProgrammedAssociation> = {
        let store = state.lock();
        store
            .pod_identity_associations_in(&key.0, &key.1)
            .map(|record| ProgrammedAssociation {
                association_id: record.association_id.clone(),
                association_arn: record.association_arn.clone(),
                namespace: record.namespace.clone(),
                service_account: record.service_account.clone(),
                role_arn: record.role_arn.clone(),
                policy: record.policy.clone(),
                target_role_arn: record.target_role_arn.clone(),
                external_id: record.external_id.clone(),
                disable_session_tags: record.disable_session_tags,
            })
            .collect()
    };
    let count = plane
        .lock()
        .program_pod_identity_associations(&key.1, &key.0, &programmed);
    ::tracing::debug!(
        region = %key.0,
        cluster = %key.1,
        associations = count,
        "programmed a cluster's EKS Pod Identity associations"
    );
}

/// Program the associations of the cluster keyed `key`, resolving the plane from
/// `engine` — the form an operation handler calls, which has the engine and not
/// the plane.
///
/// Does nothing on an emulator that carries no EKS Auth plane.
pub fn program_cluster_associations_on<Eng: Send + Sync + 'static>(
    engine: &Eng,
    state: &::engine::ServiceState<ServiceStore>,
    key: &(String, String),
) {
    if let Some(plane) = auth_plane(engine) {
        program_cluster_associations(&plane, state, key);
    }
}

/// Mint the projected service account token the plan resolved, through the EKS
/// Auth plane's own issuer — so the record of what was issued lives where a
/// presented token is resolved — and record the issuance in `redeemers`, the
/// registry the service that REDEEMS a web identity token reads.
///
/// `None` when this emulator carries no such plane, when the cluster is not
/// registered there, and when the plan names an identity or a window that plane
/// would not issue for; the API server then answers that it could not issue the
/// token rather than handing back a value nothing would accept.
#[must_use]
pub fn issue_service_account_token(
    plane: Option<&AuthPlane>,
    cluster_name: &str,
    plan: &TokenPlan,
    redeemers: Option<&WebIdentityRedeemers>,
) -> Option<String> {
    let plane = plane?;
    let mut request = ServiceAccountTokenRequest::for_pod(
        &plan.namespace,
        &plan.service_account,
        plan.pod_name.as_deref().unwrap_or(&plan.service_account),
    )
    .issued_at(plan.issued_at)
    .expiring_at(plan.expires_at);
    // A token the request bound to no pod is not a pod-bound token: it names the
    // service account only, which is what a `TokenRequest` with no
    // `boundObjectRef` is issued as.
    if plan.pod_name.is_none() {
        request.pod_name = None;
    }
    if let Some(uid) = plan.pod_uid.as_deref() {
        request = request.bound_to_pod_uid(uid);
    }
    // The audiences the request asked for, as it asked for them: the audience is
    // what a consumer of the token checks, and the pod identity audience is only
    // one of them.
    let audiences: Vec<&str> = plan
        .audiences
        .iter()
        .map(::std::string::String::as_str)
        .collect();
    if !audiences.is_empty() {
        request = request.for_audience(&audiences);
    }
    let (token, issuer) = {
        let mut store = plane.lock();
        let cluster = store.cluster_mut(cluster_name)?;
        let token = cluster.issue_service_account_token(&request)?;
        (token, cluster.oidc_issuer.clone())
    };
    record_redeemable_identity(redeemers, &token, &issuer, &request);
    Some(token)
}

/// The registry of web identity tokens this emulator ITSELF issued, as this service
/// reaches it: one store in the shared engine, written by the service that issues
/// such a token and read by the service that redeems it
/// (`sts:AssumeRoleWithWebIdentity`).
pub type WebIdentityRedeemers =
    ::engine::ServiceState<::service_common::web_identity_tokens::IssuedWebIdentityTokens>;

/// That registry in `engine`, or `None` for an engine carrying none — on which a
/// cluster still issues its tokens, and no redeemer can establish that this
/// emulator issued one.
#[must_use]
pub fn web_identity_redeemers<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<WebIdentityRedeemers> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<
            ::service_common::web_identity_tokens::IssuedWebIdentityTokens,
        >)
}

/// Record, in the registry the REDEEMER of a web identity token reads, that this
/// cluster's Kubernetes API server issued `token` for the identity `request` names.
///
/// A projected service account token is the web identity an IAM role is assumed
/// with: a client hands it to `sts:AssumeRoleWithWebIdentity`, which "validates
/// the identity of the caller by using a token from the web identity provider".
/// That redeemer cannot verify a signature against the key set the issuer
/// publishes — this emulator fetches nothing from outside itself — so what it CAN
/// establish is that the token is one this emulator issued, and that is what this
/// record says: the issuer the cluster publishes, the audience the token was issued
/// to, the service account username it names and the window it was issued for,
/// exactly the claims the minted token carries.
///
/// Best-effort, as every recorder in this tree is: an engine carrying no registry
/// still gets its token, because failing a `TokenRequest` because the redemption
/// registry is unreachable would be a worse answer than issuing it unrecorded.
fn record_redeemable_identity(
    redeemers: Option<&WebIdentityRedeemers>,
    token: &str,
    issuer: &str,
    request: &ServiceAccountTokenRequest,
) {
    let Some(redeemers) = redeemers else {
        ::tracing::warn!(
            "the engine carries no issued-web-identity registry: the service account token \
             was issued unrecorded, so no redeemer can establish that this emulator issued it"
        );
        return;
    };
    redeemers.lock().record(
        token.to_string(),
        ::service_common::web_identity_tokens::IssuedWebIdentity {
            issuer: issuer.to_string(),
            // RFC 7519 §4.1.3 writes `aud` as a string or an array of them, and a token
            // issued to one consumer carries that one value either way, so the first
            // audience it was issued to is the audience its redemption reports.
            audience: request.audience.first().cloned().unwrap_or_default(),
            // The `sub` of a projected token: the username Kubernetes authenticates that
            // service account as, in this service's one spelling of it.
            subject: super::kubernetes_rbac::service_account_username(
                &request.namespace,
                &request.service_account,
            ),
            // A projected service account token states no authentication method of its
            // own, so the provider publishes no such condition key for it.
            authentication_methods: Vec::new(),
            expires_at: request.expires_at,
            // A Kubernetes API server tags no identity: the claim an issuer carries
            // session tags in is not one a projected token has.
            principal_tags: Vec::new(),
        },
        request.issued_at,
    );
}
