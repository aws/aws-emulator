// SPDX-License-Identifier: CC0-1.0

//! WHEN the cluster an EKS Pod Identity association hangs off is resolved, relative to the
//! IAM question the request's `roleArn` raises.
//!
//! RULE: `CreatePodIdentityAssociation` and `UpdatePodIdentityAssociation` resolve the
//! CLUSTER their `@httpLabel` names against this service's state — answering one they
//! cannot resolve with the 404 `ResourceNotFoundException` that names only the cluster
//! ([`service_com_amazonaws_eks_awswesleyfrontend::state::cluster_not_found_message`]) —
//! BEFORE the trust relationship of the role the request carries is resolved against IAM.
//! An association is a child of a cluster, so a role is only judged for an association
//! there is a cluster to write it under; a caller whose cluster does not exist is pointed
//! at its only recovery (create the cluster, retry) instead of at a trust policy that has
//! nothing to be the trust policy of.
//!
//! The other precedences are unchanged and are pinned where they belong: the pass-role
//! authorization a caller-supplied role earns (403, `pass_role_authorization`) and a member
//! outside its own documented form (400, `pod_identity_and_rotation_refusals`) both name no
//! resource of this service at all, so they still precede the resolution. Every row here
//! therefore carries a well-formed role ARN of this account and well-formed Kubernetes
//! names.
//!
//! The tables span what the rule generalises over:
//!
//! * the two OPERATIONS that carry a `roleArn` onto an association;
//! * what IAM would answer about that role — a role it does not hold at all, and a role it
//!   holds whose trust policy lets the EKS Pod Identity principal assume NOTHING (the
//!   refusal the recorded service answers that class with);
//! * the CLUSTER NAME form (both documented bounds of `clusterName`, `length 1..100`), how
//!   it fails to resolve, and the Region the request is signed for.
//!
//! The controls are what show the trust gate was not simply deleted: on a cluster that DOES
//! resolve, the same role is still refused 400 with the trust sentence, and the role whose
//! trust policy admits the principal is created and reads back.
//!
//! IAM is mounted beside this service here (`crate::wire::emulator_with_identity`), because
//! a refusal grounded in a trust policy needs an IAM that holds one — the suites that drive
//! an emulator without one are `peer_resolution_is_grounded`.

use serde_json::json;
use service_com_amazonaws_eks_awswesleyfrontend::state::{
    cluster_not_found_message, MAX_CLUSTER_NAME_LEN, POD_IDENTITY_SERVICE_PRINCIPAL,
    POD_IDENTITY_TRUST_POLICY_REFUSAL,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator_with_identity, encode_label, error_code, query, Answer};

/// The Region every call below is signed for, and the one the fixture cluster lives in.
const REGION: &str = "us-west-2";

/// Another Region, where the fixture cluster does not exist — a cluster is resolved per
/// Region, so the same name signed for this one resolves to nothing.
const OTHER_REGION: &str = "eu-west-1";

/// The cluster that exists, which the controls write their association under.
const CLUSTER: &str = "podid-order-real";

/// The namespace and service account every row names: well-formed Kubernetes names, so no
/// row is refused for its body.
const NAMESPACE: &str = "default";

/// A role IAM holds whose trust policy admits NOTHING, so the trust question has a grounded
/// refusal to answer with.
const WITHHOLDING_ROLE: &str = "podIdentityOrderWithholdingRole";

/// A role IAM holds whose trust policy admits the EKS Pod Identity principal, which is what
/// an association may be written for.
const ADMITTING_ROLE: &str = "podIdentityOrderAdmittingRole";

/// A role of this account that IAM holds NO record of at all.
const ABSENT_ROLE: &str = "podIdentityOrderNoSuchRole";

/// The ARN of a role of the account this emulator serves, so the pass-role authorization
/// (403) is never what answers a row.
fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:role/{name}", engine::emulator_account_id())
}

/// A trust policy that lets `principal` assume the role, in the form AWS publishes for a
/// service principal.
fn service_trust_policy(principal: &str) -> String {
    json!({
        "Version": "2012-10-17",
        "Statement": [{
            "Effect": "Allow",
            "Principal": { "Service": principal },
            "Action": ["sts:AssumeRole", "sts:TagSession"],
        }],
    })
    .to_string()
}

/// Create the role `name` in IAM with `trust` as its trust policy, through IAM's own
/// `CreateRole`, so what this suite asserts is grounded in what IAM holds.
async fn create_role(router: &MultiServiceRouter, name: &str, trust: String) {
    let (status, body) = query(
        router,
        "iam",
        "2010-05-08",
        "CreateRole",
        &[
            ("RoleName", name.to_string()),
            ("AssumeRolePolicyDocument", trust),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "setup: CreateRole {name:?} must succeed, got {status} {body}"
    );
}

/// The emulator the rows drive: this service beside IAM, with one cluster that exists and
/// the two roles IAM holds.
async fn fixture() -> MultiServiceRouter {
    let router = emulator_with_identity();
    let created = call(
        &router,
        REGION,
        "POST",
        "/clusters",
        &json!({
            "name": CLUSTER,
            "roleArn": role_arn("eksClusterRole"),
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        })
        .to_string(),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: the fixture cluster must be created, got {created:?}"
    );
    // A trust policy that admits only another service principal lets the EKS Pod Identity
    // principal assume nothing, which is the grounded refusal this suite needs.
    create_role(
        &router,
        WITHHOLDING_ROLE,
        service_trust_policy("ec2.amazonaws.com"),
    )
    .await;
    create_role(
        &router,
        ADMITTING_ROLE,
        service_trust_policy(POD_IDENTITY_SERVICE_PRINCIPAL),
    )
    .await;
    router
}

/// `CreatePodIdentityAssociation` of `service_account` under `cluster`, with `role`.
async fn create_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    role: &str,
    service_account: &str,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations",
            encode_label(cluster)
        ),
        &json!({
            "namespace": NAMESPACE,
            "serviceAccount": service_account,
            "roleArn": role,
        })
        .to_string(),
    )
    .await
}

/// `UpdatePodIdentityAssociation` of `association_id` under `cluster`, with `role`.
async fn update_association(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    association_id: &str,
    role: &str,
) -> Answer {
    call(
        router,
        region,
        "POST",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(cluster),
            encode_label(association_id)
        ),
        &json!({ "roleArn": role }).to_string(),
    )
    .await
}

/// Assert `answer` is the 404 that names `cluster` and nothing else.
fn assert_cluster_not_found(case: &str, cluster: &str, answer: &Answer) {
    assert_eq!(
        answer.status, 404,
        "case {case}: the unresolvable cluster must be reported before the role's trust \
         policy, got {answer:?}"
    );
    assert_eq!(
        error_code(answer),
        "ResourceNotFoundException",
        "case {case}: expected the not-found error class, got {answer:?}"
    );
    assert_eq!(
        answer.body["clusterName"].as_str(),
        Some(cluster),
        "case {case}: the payload must name the cluster that was not found, got {:?}",
        answer.body["clusterName"]
    );
    assert_eq!(
        answer.message(),
        cluster_not_found_message(cluster),
        "case {case}: expected the message built from the request, got {:?}",
        answer.message()
    );
}

/// The cluster names no row can resolve, and why each one cannot: `(why, region, cluster)`.
fn unresolvable_clusters() -> Vec<(&'static str, &'static str, String)> {
    vec![
        (
            "a cluster that was never created",
            REGION,
            "podid-order-ghost".to_string(),
        ),
        (
            "a cluster name differing only in case",
            REGION,
            CLUSTER.to_ascii_uppercase(),
        ),
        ("a one-character cluster name", REGION, "p".to_string()),
        (
            "a cluster name at the documented length ceiling",
            REGION,
            "p".repeat(MAX_CLUSTER_NAME_LEN),
        ),
        (
            "the real cluster name, signed for another Region",
            OTHER_REGION,
            CLUSTER.to_string(),
        ),
    ]
}

/// The roles a row may carry, and what IAM answers about each: `(why, role name)`. Both are
/// values a trust-policy refusal could be grounded in — one because IAM holds no such role,
/// one because the trust policy IAM holds admits nothing.
fn refusable_roles() -> Vec<(&'static str, &'static str)> {
    vec![
        ("a role IAM does not hold", ABSENT_ROLE),
        (
            "a role whose trust policy admits another principal",
            WITHHOLDING_ROLE,
        ),
    ]
}

// ---------------------------------------------------------------------------
// RULE: the cluster is resolved first, so a create or an update against one that does not
// exist is the 404 that names the cluster — whatever IAM would say about the role.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_pod_identity_create_on_an_unresolvable_cluster_reports_the_cluster_not_the_role() {
    let router = fixture().await;

    for (why_role, role) in refusable_roles() {
        for (why_cluster, region, cluster) in unresolvable_clusters() {
            let case = format!("CreatePodIdentityAssociation / {why_role} / {why_cluster}");
            let answer = create_association(
                &router,
                region,
                &cluster,
                &role_arn(role),
                "podid-order-create",
            )
            .await;
            assert_cluster_not_found(&case, &cluster, &answer);
        }
    }
}

#[tokio::test]
async fn a_pod_identity_update_on_an_unresolvable_cluster_reports_the_cluster_not_the_role() {
    let router = fixture().await;

    // A well-formed association id this emulator never minted: the update's own not-found
    // is about the association, and it comes AFTER the cluster's.
    let association_id = "a-00000000000000000";
    for (why_role, role) in refusable_roles() {
        for (why_cluster, region, cluster) in unresolvable_clusters() {
            let case = format!("UpdatePodIdentityAssociation / {why_role} / {why_cluster}");
            let answer = update_association(
                &router,
                region,
                &cluster,
                association_id,
                &role_arn(role),
            )
            .await;
            assert_cluster_not_found(&case, &cluster, &answer);
        }
    }
}

// ---------------------------------------------------------------------------
// CONTROL: the rule is about ORDER, not about dropping the trust gate — once the cluster
// resolves, a role whose trust policy admits nothing is still refused, with the sentence
// that class is answered with.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn once_the_cluster_resolves_a_withholding_trust_policy_is_still_refused() {
    let router = fixture().await;

    let answer = create_association(
        &router,
        REGION,
        CLUSTER,
        &role_arn(WITHHOLDING_ROLE),
        "podid-order-withheld",
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case CreatePodIdentityAssociation on a cluster that resolves: the trust policy is \
         still judged, got {answer:?}"
    );
    assert_eq!(
        error_code(&answer),
        "InvalidParameterException",
        "case CreatePodIdentityAssociation on a cluster that resolves: expected the \
         parameter error class, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        POD_IDENTITY_TRUST_POLICY_REFUSAL,
        "case CreatePodIdentityAssociation on a cluster that resolves: expected the trust \
         policy sentence, got {:?}",
        answer.message()
    );

    // The same question on the UPDATE, over an association written with the admitting role.
    let created = create_association(
        &router,
        REGION,
        CLUSTER,
        &role_arn(ADMITTING_ROLE),
        "podid-order-updated",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: an association with an admitting role must be created, got {created:?}"
    );
    let association_id = created.association()["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let answer = update_association(
        &router,
        REGION,
        CLUSTER,
        &association_id,
        &role_arn(WITHHOLDING_ROLE),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case UpdatePodIdentityAssociation on a cluster that resolves: the trust policy is \
         still judged, got {answer:?}"
    );
    assert_eq!(
        answer.message(),
        POD_IDENTITY_TRUST_POLICY_REFUSAL,
        "case UpdatePodIdentityAssociation on a cluster that resolves: expected the trust \
         policy sentence, got {:?}",
        answer.message()
    );
}

// ---------------------------------------------------------------------------
// ROUND TRIP: a create whose cluster resolves and whose role IAM admits is written and
// reads back, so the resolution-first order did not cost the create itself.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_association_for_an_admitted_role_on_a_resolvable_cluster_reads_back() {
    let router = fixture().await;

    let created = create_association(
        &router,
        REGION,
        CLUSTER,
        &role_arn(ADMITTING_ROLE),
        "podid-order-roundtrip",
    )
    .await;
    assert_eq!(
        created.status, 200,
        "an association for an admitted role must be created, got {created:?}"
    );
    let association_id = created.association()["associationId"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    assert!(
        !association_id.is_empty(),
        "the create must report an associationId, got {created:?}"
    );

    let read = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/pod-identity-associations/{}",
            encode_label(CLUSTER),
            encode_label(&association_id)
        ),
        "",
    )
    .await;
    assert_eq!(
        read.status, 200,
        "the stored association must read back, got {read:?}"
    );
    assert_eq!(
        read.association()["roleArn"].as_str(),
        Some(role_arn(ADMITTING_ROLE).as_str()),
        "the association read back must carry the role it was created with, got {:?}",
        read.association()
    );
    assert_eq!(
        read.association()["clusterName"].as_str(),
        Some(CLUSTER),
        "the association read back must name its cluster, got {:?}",
        read.association()
    );
}
