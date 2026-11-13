// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DisassociateAccessPolicy`, driven over the wire through
//! the same `MultiServiceRouter` the production binary mounts.
//!
//! The rules under test:
//!
//! 1. a revocation removes EXACTLY the association the request names from the
//!    access entry it names, leaving every other association — on that entry and
//!    on every other entry — where it was;
//! 2. it is idempotent: asking for a policy not to be associated answers the same
//!    empty 200 when no record is keyed by that ARN, so a replayed revocation, a
//!    never-granted policy and an ARN that names no published policy all read the
//!    same;
//! 3. the association is addressed THROUGH the `(cluster, access entry)` pair, so
//!    a miss of either half is `ResourceNotFoundException` (404) — and a cluster
//!    name resolves only within the Region the request is signed for;
//! 4. a cluster `RegisterCluster` connected has no Amazon EKS-managed access, so
//!    the operation refuses it with `ClientException` (400).
//!
//! Each suite asserts its rule over a TABLE whose rows differ along what the rule
//! generalises over — how many policies the entry holds and which one is revoked,
//! the scope each association carries, how the named policy fails to be
//! associated, which half of the pair is missing — rather than over one recorded
//! request, and every assertion names its case and prints the actual value.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the cluster
//! body is `cluster_lifecycle`'s `access_entry_cluster_request`: no suite grows its own client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, CONNECTED_CLUSTER_REFUSAL_MESSAGE, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The emulator's own canonical account id, which every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

/// A Region other than the one the fixtures are created in, for the rule that a
/// cluster name resolves only within one Region.
const OTHER_REGION: &str = "eu-west-1";

fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
}

/// Four policies from the catalogue Amazon EKS publishes, named through
/// [`access_policy_arn`] so the rows come from the service's own source of truth
/// ([`service_com_amazonaws_eks_awswesleyfrontend::state::ACCESS_POLICY_NAMES`])
/// rather than from literals pasted here.
fn admin_policy() -> String {
    access_policy_arn("AmazonEKSAdminPolicy")
}
fn admin_view_policy() -> String {
    access_policy_arn("AmazonEKSAdminViewPolicy")
}
fn edit_policy() -> String {
    access_policy_arn("AmazonEKSEditPolicy")
}
fn networking_policy() -> String {
    access_policy_arn("AmazonEKSNetworkingPolicy")
}

async fn create_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        "/clusters",
        &access_entry_cluster_request(name).to_string(),
    )
    .await
}

/// A cluster Amazon EKS does not manage: `RegisterCluster` connected it, so it has
/// no access entries and no access policies of the EKS-managed plane.
async fn register_cluster(router: &MultiServiceRouter, name: &str) -> Answer {
    let body = serde_json::json!({
        "name": name,
        "connectorConfig": {
            "roleArn": role_arn("AmazonEKSConnectorRole"),
            "provider": "OTHER",
        },
    });
    call(
        router,
        REGION,
        "POST",
        "/cluster-registrations",
        &body.to_string(),
    )
    .await
}

async fn create_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &serde_json::json!({ "principalArn": principal }).to_string(),
    )
    .await
}

async fn associate(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    policy_arn: &str,
    access_scope: &serde_json::Value,
) -> Answer {
    let body = serde_json::json!({ "policyArn": policy_arn, "accessScope": access_scope });
    call(
        router,
        REGION,
        "POST",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(principal)
        ),
        &body.to_string(),
    )
    .await
}

async fn disassociate_in(
    router: &MultiServiceRouter,
    region: &str,
    cluster: &str,
    principal: &str,
    policy_arn: &str,
) -> Answer {
    call(
        router,
        region,
        "DELETE",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies/{}",
            encode_label(cluster),
            encode_label(principal),
            encode_label(policy_arn)
        ),
        "",
    )
    .await
}

async fn disassociate(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    policy_arn: &str,
) -> Answer {
    disassociate_in(router, REGION, cluster, principal, policy_arn).await
}

async fn list_associated(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}/access-policies",
            encode_label(cluster),
            encode_label(principal)
        ),
        "",
    )
    .await
}

/// The policy ARNs a `ListAssociatedAccessPolicies` answer reports, in order.
fn associated_arns(answer: &Answer) -> Vec<String> {
    answer.body["associatedAccessPolicies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| item["policyArn"].as_str().unwrap_or_default().to_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// The whole `AssociatedAccessPolicy` the listing reports for `policy_arn`, if any.
fn associated_policy(answer: &Answer, policy_arn: &str) -> Option<serde_json::Value> {
    answer.body["associatedAccessPolicies"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["policyArn"].as_str() == Some(policy_arn))
                .cloned()
        })
}

/// A cluster with one `STANDARD` access entry on it. Returns the principal ARN the
/// entry is keyed by.
async fn cluster_with_entry(router: &MultiServiceRouter, cluster: &str, role: &str) -> String {
    let created = create_cluster(router, cluster).await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {cluster:?} must succeed, got {created:?}"
    );
    let principal = role_arn(role);
    let entry = create_entry(router, cluster, &principal).await;
    assert_eq!(
        entry.status, 200,
        "setup: creating the access entry for {principal:?} must succeed, got {entry:?}"
    );
    principal
}

/// A cluster scope and two namespace scopes, so the rows under test differ in what
/// the removed and the surviving associations carry.
fn cluster_scope() -> serde_json::Value {
    serde_json::json!({ "type": "cluster" })
}
fn namespace_scope(namespaces: &[&str]) -> serde_json::Value {
    serde_json::json!({ "type": "namespace", "namespaces": namespaces })
}

/// One grant to lay down before a revocation: the policy ARN, and the scope it is
/// associated with.
type Grant = (String, serde_json::Value);

/// Grant every `(policy, scope)` pair to `principal` on `cluster`.
async fn grant_all(router: &MultiServiceRouter, cluster: &str, principal: &str, grants: &[Grant]) {
    for (policy_arn, scope) in grants {
        let granted = associate(router, cluster, principal, policy_arn, scope).await;
        assert_eq!(
            granted.status, 200,
            "setup: granting {policy_arn:?} must succeed, got {granted:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1: a revocation removes exactly the association it names and answers an
// empty 200; every other association survives.
// ---------------------------------------------------------------------------

/// The table spans how many associations the entry holds (1, 2 and 4), which of
/// them is revoked (only, first, middle, last in the listing's sorted order) and
/// the scope each one carries (cluster, one namespace, several namespaces).
#[tokio::test]
async fn a_revocation_removes_exactly_the_named_association() {
    // The case name, the grants the entry starts with, and the policy ARN revoked.
    let cases: Vec<(&str, Vec<Grant>, String)> = vec![
        (
            "the only association, cluster-scoped",
            vec![(admin_policy(), cluster_scope())],
            admin_policy(),
        ),
        (
            "one of two, the namespace-scoped one",
            vec![
                (admin_policy(), cluster_scope()),
                (edit_policy(), namespace_scope(&["kube-system"])),
            ],
            edit_policy(),
        ),
        (
            "the first of four in sorted order",
            vec![
                (admin_policy(), cluster_scope()),
                (admin_view_policy(), namespace_scope(&["team-a", "team-b"])),
                (edit_policy(), namespace_scope(&["kube-system"])),
                (networking_policy(), cluster_scope()),
            ],
            admin_policy(),
        ),
        (
            "the middle of four in sorted order",
            vec![
                (admin_policy(), cluster_scope()),
                (admin_view_policy(), namespace_scope(&["team-a", "team-b"])),
                (edit_policy(), namespace_scope(&["kube-system"])),
                (networking_policy(), cluster_scope()),
            ],
            edit_policy(),
        ),
        (
            "the last of four in sorted order",
            vec![
                (admin_policy(), cluster_scope()),
                (admin_view_policy(), namespace_scope(&["team-a", "team-b"])),
                (edit_policy(), namespace_scope(&["kube-system"])),
                (networking_policy(), cluster_scope()),
            ],
            networking_policy(),
        ),
    ];

    for (index, (case, grants, revoked)) in cases.iter().enumerate() {
        let router = emulator();
        let cluster = format!("revoke-{index}");
        let principal = cluster_with_entry(&router, &cluster, "revoke-role").await;
        grant_all(&router, &cluster, &principal, grants).await;

        let before = list_associated(&router, &cluster, &principal).await;
        let surviving: Vec<String> = associated_arns(&before)
            .into_iter()
            .filter(|arn| arn != revoked)
            .collect();
        let surviving_records: Vec<Option<serde_json::Value>> = surviving
            .iter()
            .map(|arn| associated_policy(&before, arn))
            .collect();

        let answer = disassociate(&router, &cluster, &principal, revoked).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected an empty 200, got {answer:?}"
        );
        assert!(
            answer.body.as_object().is_none_or(serde_json::Map::is_empty),
            "case {case:?}: the answer carries no members, got {answer:?}"
        );

        let after = list_associated(&router, &cluster, &principal).await;
        assert_eq!(
            after.status, 200,
            "case {case:?}: listing after the revocation must succeed, got {after:?}"
        );
        assert_eq!(
            associated_arns(&after),
            surviving,
            "case {case:?}: only {revoked:?} must be gone, got {after:?}"
        );
        for (arn, record) in surviving.iter().zip(surviving_records.iter()) {
            assert_eq!(
                associated_policy(&after, arn).as_ref(),
                record.as_ref(),
                "case {case:?}: the surviving association {arn:?} must be untouched, \
                 got {after:?}"
            );
        }

        // The access entry itself survives the revocation: only the association was
        // removed, so the entry still reads back and still accepts grants.
        let entry = call(
            &router,
            REGION,
            "GET",
            &format!(
                "/clusters/{}/access-entries/{}",
                encode_label(&cluster),
                encode_label(&principal)
            ),
            "",
        )
        .await;
        assert_eq!(
            entry.status, 200,
            "case {case:?}: the access entry must survive the revocation, got {entry:?}"
        );
    }
}

/// The same rule across entries: two access entries holding the SAME policy, and a
/// revocation on one of them leaves the other's record alone.
#[tokio::test]
async fn a_revocation_touches_only_the_entry_it_names() {
    let router = emulator();
    let cluster = "per-entry";
    let kept = cluster_with_entry(&router, cluster, "kept-role").await;
    let revoked_from = role_arn("revoked-role");
    let entry = create_entry(&router, cluster, &revoked_from).await;
    assert_eq!(
        entry.status, 200,
        "setup: the second access entry must be created, got {entry:?}"
    );

    let policy = admin_policy();
    for principal in [&kept, &revoked_from] {
        grant_all(
            &router,
            cluster,
            principal,
            &[(policy.clone(), namespace_scope(&["kube-system"]))],
        )
        .await;
    }
    let kept_before = associated_policy(&list_associated(&router, cluster, &kept).await, &policy);

    let answer = disassociate(&router, cluster, &revoked_from, &policy).await;
    assert_eq!(
        answer.status, 200,
        "case {revoked_from:?}: expected an empty 200, got {answer:?}"
    );
    assert_eq!(
        associated_arns(&list_associated(&router, cluster, &revoked_from).await),
        Vec::<String>::new(),
        "case {revoked_from:?}: the named entry must hold no association any more"
    );
    assert_eq!(
        associated_policy(&list_associated(&router, cluster, &kept).await, &policy),
        kept_before,
        "case {kept:?}: the other entry's association must be untouched"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: a revocation is idempotent — it asks for the policy NOT to be
// associated, which is already true when no record is keyed by that ARN, so the
// answer is the same empty 200 and the listing is unchanged.
// ---------------------------------------------------------------------------

/// The table spans the ways the named policy can fail to be associated: never
/// granted, granted and already revoked, a published policy on an entry that holds
/// none, an ARN with the right shape naming no published policy, and a value that
/// is not an ARN at all.
#[tokio::test]
async fn revoking_a_policy_that_is_not_associated_is_a_no_op() {
    let router = emulator();
    let cluster = "idempotent";
    let principal = cluster_with_entry(&router, cluster, "idempotent-role").await;
    grant_all(
        &router,
        cluster,
        &principal,
        &[
            (admin_policy(), cluster_scope()),
            (edit_policy(), namespace_scope(&["kube-system"])),
        ],
    )
    .await;

    // The one row that IS associated goes first, so every later row runs against an
    // entry that has already served a revocation.
    let cases: Vec<(&str, String, bool)> = vec![
        ("an associated policy", edit_policy(), true),
        ("the same policy a second time", edit_policy(), false),
        ("the same policy a third time", edit_policy(), false),
        (
            "a published policy that was never granted",
            networking_policy(),
            false,
        ),
        (
            "an access-policy ARN naming no published policy",
            access_policy_arn("AmazonEKSNoSuchPolicy"),
            false,
        ),
        ("a value that is not an ARN", "not-an-arn".to_owned(), false),
        (
            "an ARN of another resource kind",
            format!("arn:aws:eks:{REGION}:{ACCOUNT}:cluster/{cluster}"),
            false,
        ),
    ];

    let mut expected = associated_arns(&list_associated(&router, cluster, &principal).await);
    for (case, policy_arn, removes_a_record) in &cases {
        let answer = disassociate(&router, cluster, &principal, policy_arn).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected an empty 200, got {answer:?}"
        );
        assert_eq!(
            answer.error_type, None,
            "case {case:?}: a no-op revocation is not an error, got {answer:?}"
        );
        if *removes_a_record {
            expected.retain(|arn| arn != policy_arn);
        }
        let listed = list_associated(&router, cluster, &principal).await;
        assert_eq!(
            associated_arns(&listed),
            expected,
            "case {case:?}: expected the associations {expected:?}, got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 1, read the other way: what a revocation removes is the record, so the
// same policy can be granted again afterwards and comes back with the scope and
// the timestamps of the NEW grant.
// ---------------------------------------------------------------------------

/// The table spans the scope the policy is re-granted with: the same one it
/// carried, and a different one, for both scope types.
#[tokio::test]
async fn a_revoked_policy_can_be_granted_again() {
    let cases: Vec<(&str, serde_json::Value, serde_json::Value)> = vec![
        (
            "cluster scope, re-granted cluster-scoped",
            cluster_scope(),
            cluster_scope(),
        ),
        (
            "cluster scope, re-granted namespace-scoped",
            cluster_scope(),
            namespace_scope(&["kube-system", "team-a"]),
        ),
        (
            "namespace scope, re-granted cluster-scoped",
            namespace_scope(&["team-a"]),
            cluster_scope(),
        ),
        (
            "namespace scope, re-granted over other namespaces",
            namespace_scope(&["team-a"]),
            namespace_scope(&["team-b", "team-c"]),
        ),
    ];

    for (index, (case, first, second)) in cases.iter().enumerate() {
        let router = emulator();
        let cluster = format!("re-grant-{index}");
        let principal = cluster_with_entry(&router, &cluster, "re-grant-role").await;
        let policy = admin_view_policy();

        let granted = associate(&router, &cluster, &principal, &policy, first).await;
        assert_eq!(
            granted.status, 200,
            "case {case:?}: the first grant must succeed, got {granted:?}"
        );
        let first_associated_at = granted.body["associatedAccessPolicy"]["associatedAt"].clone();

        let revoked = disassociate(&router, &cluster, &principal, &policy).await;
        assert_eq!(
            revoked.status, 200,
            "case {case:?}: the revocation must succeed, got {revoked:?}"
        );
        let listed = list_associated(&router, &cluster, &principal).await;
        assert_eq!(
            associated_policy(&listed, &policy),
            None,
            "case {case:?}: the revoked policy must be gone, got {listed:?}"
        );

        let again = associate(&router, &cluster, &principal, &policy, second).await;
        assert_eq!(
            again.status, 200,
            "case {case:?}: the policy must be grantable again, got {again:?}"
        );
        let listed = list_associated(&router, &cluster, &principal).await;
        let record = associated_policy(&listed, &policy);
        assert_eq!(
            record.as_ref().map(|value| value["accessScope"].clone()),
            Some(again.body["associatedAccessPolicy"]["accessScope"].clone()),
            "case {case:?}: the re-granted association must carry the new scope, \
             got {listed:?}"
        );
        // `associatedAt` is "the date and time the AccessPolicy was associated with
        // an AccessEntry": the record the first grant wrote is GONE, so the
        // re-grant's instant is not the first one unless the clock has not moved.
        let regranted_at = record
            .as_ref()
            .map(|value| value["associatedAt"].clone())
            .unwrap_or(serde_json::Value::Null);
        assert!(
            regranted_at.as_f64().is_some()
                && regranted_at.as_f64() >= first_associated_at.as_f64(),
            "case {case:?}: the re-grant must carry its own associatedAt, \
             got {regranted_at:?} after {first_associated_at:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: the association is addressed through the `(cluster, access entry)`
// pair, so a miss of either half is a 404 `ResourceNotFoundException` — and the
// cluster resolves only within the Region the request is signed for.
// ---------------------------------------------------------------------------

/// The table crosses which half of the pair is missing (and how: never created,
/// created in another Region, a principal with no entry) with the shape of the
/// policy ARN, because the rule holds whatever the policy ARN says.
#[tokio::test]
async fn a_missing_resource_is_not_found() {
    let router = emulator();
    let cluster = "present";
    let principal = cluster_with_entry(&router, cluster, "present-role").await;
    let policy = admin_policy();
    grant_all(&router, cluster, &principal, &[(policy.clone(), cluster_scope())]).await;

    let targets: Vec<(&str, &str, String, String)> = vec![
        (
            "no such cluster",
            REGION,
            "absent-cluster".to_owned(),
            principal.clone(),
        ),
        (
            "the cluster exists, the access entry does not",
            REGION,
            cluster.to_owned(),
            role_arn("never-created"),
        ),
        (
            "both names exist, but in another Region",
            OTHER_REGION,
            cluster.to_owned(),
            principal.clone(),
        ),
    ];
    let policies: Vec<(&str, String)> = vec![
        ("an associated policy", policy.clone()),
        ("a published policy that was never granted", edit_policy()),
        ("a value that is not an ARN", "not-an-arn".to_owned()),
    ];

    for (target, region, target_cluster, target_principal) in &targets {
        for (policy_case, policy_arn) in &policies {
            let answer =
                disassociate_in(&router, region, target_cluster, target_principal, policy_arn)
                    .await;
            assert_eq!(
                answer.status, 404,
                "case {target:?} with {policy_case:?}: expected 404, got {answer:?}"
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {target:?} with {policy_case:?}: expected ResourceNotFoundException, \
                 got {answer:?}"
            );
            assert_eq!(
                answer.body["message"].as_str(),
                Some(RESOURCE_NOT_FOUND_MESSAGE),
                "case {target:?} with {policy_case:?}: expected the generic not-found \
                 payload, got {answer:?}"
            );
        }
    }

    // None of the refused calls removed anything.
    let listed = list_associated(&router, cluster, &principal).await;
    assert_eq!(
        associated_arns(&listed),
        vec![policy.clone()],
        "a refused revocation must leave the association in place, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: a cluster `RegisterCluster` connected has no Amazon EKS-managed access,
// so this operation refuses it as soon as the cluster is resolved.
// ---------------------------------------------------------------------------

/// The table spans the shape of the request made against the connected cluster:
/// which principal and which policy ARN it names, because the refusal is a fact
/// about the CLUSTER and not about either of them.
#[tokio::test]
async fn a_connected_cluster_is_refused() {
    let router = emulator();
    let cluster = "connected";
    let registered = register_cluster(&router, cluster).await;
    assert_eq!(
        registered.status, 200,
        "setup: registering {cluster:?} must succeed, got {registered:?}"
    );

    let cases: Vec<(&str, String, String)> = vec![
        (
            "an IAM role principal and a published policy",
            role_arn("ConnectedPrincipal"),
            admin_policy(),
        ),
        (
            "an IAM user principal and another published policy",
            format!("arn:aws:iam::{ACCOUNT}:user/ConnectedUser"),
            edit_policy(),
        ),
        (
            "a principal and a policy ARN that names nothing",
            role_arn("ConnectedPrincipal"),
            "not-an-arn".to_owned(),
        ),
    ];

    for (case, principal, policy_arn) in &cases {
        let answer = disassociate(&router, cluster, principal, policy_arn).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ClientException",
            "case {case:?}: expected ClientException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(CONNECTED_CLUSTER_REFUSAL_MESSAGE),
            "case {case:?}: expected the connected-cluster refusal, got {answer:?}"
        );
    }
}
