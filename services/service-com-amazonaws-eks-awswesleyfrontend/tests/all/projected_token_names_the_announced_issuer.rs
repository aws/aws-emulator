// SPDX-License-Identifier: CC0-1.0

//! WHICH issuer a projected service account token names, what the record of its issuance
//! says, and how long that record outlives the cluster.
//!
//! RULE: the `iss` of a token a cluster's Kubernetes API server issues is the OIDC issuer
//! URL that cluster ANNOUNCES (`Cluster$identity.oidc.issuer`), and the same minting records
//! the issuance in the registry a redeemer of a WEB IDENTITY reads — the issuer the cluster
//! announces, the audience the `TokenRequest` asked for and the service account username the
//! token stands for. The announced issuer is the URL an account registers an IAM OpenID
//! Connect provider for, so a token naming anything else could be federated against no
//! provider at all: `sts:AssumeRoleWithWebIdentity` resolves the presented token in that one
//! registry and decides the exchange on the issuer, audience and subject it finds there.
//!
//! The issuance lasts as long as the issuer does: deleting the cluster takes the issuer it
//! announced away, and a token of an issuer that no longer exists can be verified against no
//! key set — so those issuances go with it, and only those.
//!
//! The rows span what the rule generalises over: which audience the `TokenRequest` asks for
//! (the token exchange's, the pod identity agent's, a consumer's own), how many it asks for,
//! and asking for none at all — the issuer is the cluster's whichever it is.
//!
//! The harness is the crate's own: the emulator with both EKS planes, the cluster provision
//! and the Kubernetes client that dials the published endpoint (`kubernetes_api.rs`), and the
//! control-plane wire (`wire.rs`).

use serde_json::json;
use service_com_amazonaws_eks_awswesleyfrontend::state::kubernetes_service_account_tokens::API_SERVER_AUDIENCES;

use crate::certificate_authority::{str_member, REGION};
use crate::kubernetes_api::{
    dial, emulator_with_pod_identity, kubeconfig_of, poll_until_active, provisioned_cluster,
    Kubeconfig, PODS_AUDIENCE,
};
use crate::wire::{call, encode_label};

/// The audience a `TokenRequest` that asks for none is served with: "if no audiences are
/// specified, the audience will default to the audience of the API server"
/// (`TokenRequestSpec$audiences`), which is the first of the ones this API server consumes.
const API_SERVER_AUDIENCE: &str = API_SERVER_AUDIENCES[0];

/// The namespace Kubernetes creates with a cluster, which every row runs in.
const NAMESPACE: &str = "default";

/// The audience an IRSA client asks for, which is the token exchange's own.
const STS_AUDIENCE: &str = "sts.amazonaws.com";

/// One row: the service account the token is minted for, the audiences the `TokenRequest`
/// asks for, and the audience the recorded issuance then reports.
struct Row {
    case: &'static str,
    service_account: &'static str,
    audiences: &'static [&'static str],
    recorded_audience: &'static str,
}

/// The rows, in the order they are driven.
fn rows() -> Vec<Row> {
    vec![
        Row {
            case: "the token exchange's own audience",
            service_account: "irsa-sts",
            audiences: &[STS_AUDIENCE],
            recorded_audience: STS_AUDIENCE,
        },
        Row {
            case: "the pod identity agent's audience",
            service_account: "irsa-pods",
            audiences: &[PODS_AUDIENCE],
            recorded_audience: PODS_AUDIENCE,
        },
        Row {
            case: "a consumer's own audience",
            service_account: "irsa-consumer",
            audiences: &["https://kubernetes.default.svc"],
            recorded_audience: "https://kubernetes.default.svc",
        },
        Row {
            case: "two audiences: the first is the one the issuance reports",
            service_account: "irsa-two",
            audiences: &[STS_AUDIENCE, "other.example.com"],
            recorded_audience: STS_AUDIENCE,
        },
        Row {
            case: "no audience at all: the API server's own",
            service_account: "irsa-default",
            audiences: &[],
            recorded_audience: API_SERVER_AUDIENCE,
        },
    ]
}

/// The username Kubernetes authenticates `service_account` in [`NAMESPACE`] as, which is the
/// `sub` of the token projected for it.
fn subject(service_account: &str) -> String {
    format!("system:serviceaccount:{NAMESPACE}:{service_account}")
}

/// The registry a redeemer of a web identity token reads, in the shared engine.
fn redeemers(
    engine: &engine::InMemoryEngine,
) -> ::engine::ServiceState<::service_common::web_identity_tokens::IssuedWebIdentityTokens> {
    engine.state::<::service_common::web_identity_tokens::IssuedWebIdentityTokens>()
}

/// Mint a projected token for `service_account` over the cluster's OWN Kubernetes API: the
/// service account, then the `TokenRequest` for `audiences` — what a pod's kubelet does.
async fn projected_token(
    config: &Kubeconfig,
    case: &str,
    service_account: &str,
    audiences: &[&str],
) -> String {
    let created = dial(
        config,
        case,
        "POST",
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts"),
        Some(&json!({ "metadata": { "name": service_account } })),
    )
    .await;
    assert_eq!(
        created.status, 201,
        "case {case:?}: expected the service account to be created, got {created:?}"
    );
    // A `TokenRequest` that asks for no audience carries no such member at all, which is the
    // request a client makes when it wants the API server's own.
    let spec = if audiences.is_empty() {
        json!({})
    } else {
        json!({ "audiences": audiences })
    };
    let minted = dial(
        config,
        case,
        "POST",
        &format!("/api/v1/namespaces/{NAMESPACE}/serviceaccounts/{service_account}/token"),
        Some(&json!({ "kind": "TokenRequest", "spec": spec })),
    )
    .await;
    assert_eq!(
        minted.status, 201,
        "case {case:?}: expected the token to be issued, got {minted:?}"
    );
    let token = str_member(case, "status.token", &minted.body["status"]["token"]);
    assert!(
        !token.is_empty(),
        "case {case:?}: the answer must carry the minted token, got {:?}",
        minted.body
    );
    token
}

/// The OIDC issuer URL the cluster `name` ANNOUNCES, read the way a client reads it.
async fn announced_issuer(
    router: &smithy_server_runtime::MultiServiceRouter,
    name: &str,
) -> (String, Kubeconfig) {
    let described = poll_until_active(router, REGION, name).await;
    let issuer = str_member(
        name,
        "cluster.identity.oidc.issuer",
        &described.cluster()["identity"]["oidc"]["issuer"],
    );
    (issuer, kubeconfig_of(name, &described))
}

/// RULE: every projected service account token names the issuer its cluster announces, and
/// the issuance recorded for a redeemer names that issuer, the audience asked for and the
/// service account the token stands for.
#[tokio::test]
async fn a_projected_token_names_the_issuer_its_cluster_announces() {
    let (router, engine) = emulator_with_pod_identity();
    let cluster = "projected-token-issuer";
    let config = provisioned_cluster(&router, REGION, cluster).await;
    let (announced, read_back) = announced_issuer(&router, cluster).await;
    assert_eq!(
        read_back.endpoint, config.endpoint,
        "setup: the cluster must keep publishing one endpoint"
    );
    let registry = redeemers(&engine);

    for row in rows() {
        let Row {
            case,
            service_account,
            audiences,
            recorded_audience,
        } = row;
        let token = projected_token(&config, case, service_account, audiences).await;

        // What the token itself claims, read the way any holder reads it.
        let claims = ::service_common::jwt::read_unsecured(&token).unwrap_or_else(|defect| {
            panic!("case {case:?}: the minted token must be a compact JWS, got {defect:?}")
        });
        assert_eq!(
            claims.claim_str("iss"),
            Some(announced.as_str()),
            "case {case:?}: expected the token to name the announced issuer {announced:?}, got \
             {:?}",
            claims.claim_str("iss")
        );
        assert_eq!(
            claims.claim_str("sub"),
            Some(subject(service_account).as_str()),
            "case {case:?}: expected the subject {:?}, got {:?}",
            subject(service_account),
            claims.claim_str("sub")
        );
        let expected_audiences: Vec<&str> = if audiences.is_empty() {
            vec![API_SERVER_AUDIENCE]
        } else {
            audiences.to_vec()
        };
        assert_eq!(
            claims.audiences(),
            expected_audiences,
            "case {case:?}: expected the audiences {expected_audiences:?}, got {:?}",
            claims.audiences()
        );

        // What the ISSUANCE recorded, which is what a redeemer of the token resolves it to.
        let now = std::time::SystemTime::now();
        let held = registry.lock().issued(&token, now).cloned();
        let held = held.unwrap_or_else(|| {
            panic!(
                "case {case:?}: the minting must record the issuance, so a redeemer can \
                 establish this emulator issued the token"
            )
        });
        assert_eq!(
            held.issuer, announced,
            "case {case:?}: expected the issuance to name {announced:?}, got {:?}",
            held.issuer
        );
        assert_eq!(
            held.audience, recorded_audience,
            "case {case:?}: expected the issuance's audience {recorded_audience:?}, got {:?}",
            held.audience
        );
        assert_eq!(
            held.subject,
            subject(service_account),
            "case {case:?}: expected the issuance's subject {:?}, got {:?}",
            subject(service_account),
            held.subject
        );
        assert!(
            held.expires_at > now,
            "case {case:?}: the issuance must still be live when it was just minted"
        );
    }
}

/// RULE (the issuer's lifetime): deleting a cluster takes away the issuer it announced, so
/// the issuances of the tokens it minted stop resolving — and the issuances of ANOTHER
/// cluster's tokens, whose issuer is still announced, do not.
#[tokio::test]
async fn the_issuances_of_a_deleted_clusters_tokens_stop_resolving() {
    let (router, engine) = emulator_with_pod_identity();
    let case = "a deleted cluster's issuer";
    let deleted = "projected-token-issuer-deleted";
    let kept = "projected-token-issuer-kept";
    let deleted_config = provisioned_cluster(&router, REGION, deleted).await;
    let kept_config = provisioned_cluster(&router, REGION, kept).await;
    let deleted_token =
        projected_token(&deleted_config, case, "irsa-deleted", &[STS_AUDIENCE]).await;
    let kept_token = projected_token(&kept_config, case, "irsa-kept", &[STS_AUDIENCE]).await;

    let registry = redeemers(&engine);
    let now = std::time::SystemTime::now();
    for (what, token) in [("deleted", &deleted_token), ("kept", &kept_token)] {
        assert!(
            registry.lock().issued(token, now).is_some(),
            "case {case:?}: the {what} cluster's issuance must resolve before the delete"
        );
    }

    let answered = call(
        &router,
        REGION,
        "DELETE",
        &format!("/clusters/{}", encode_label(deleted)),
        "",
    )
    .await;
    assert_eq!(
        answered.status, 200,
        "case {case:?}: expected 200 from DeleteCluster, got {answered:?}"
    );

    let now = std::time::SystemTime::now();
    assert!(
        registry.lock().issued(&deleted_token, now).is_none(),
        "case {case:?}: the deleted cluster's issuance must stop resolving, because no key \
         set can verify a token of an issuer that no longer exists"
    );
    assert!(
        registry.lock().issued(&kept_token, now).is_some(),
        "case {case:?}: the other cluster's issuance must stay, because its issuer is still \
         announced"
    );
}
