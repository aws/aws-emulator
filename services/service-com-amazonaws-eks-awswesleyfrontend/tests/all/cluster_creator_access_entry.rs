// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the access entry `CreateCluster` materialises for the
//! principal that CREATES a cluster.
//!
//! RULE: `accessConfig.bootstrapClusterCreatorAdminPermissions` "specifies whether
//! or not the cluster creator IAM principal was set as a cluster admin access entry
//! during cluster creation time. The default value is `true`" — so a create for
//! which the flag holds leaves the cluster carrying ONE access entry for the calling
//! IAM principal (type `STANDARD`, `username` that principal, no Kubernetes groups,
//! with the server-generated ARN and instants) and one cluster-scoped association of
//! the `cluster-admin` access policy, both readable through the ordinary access-entry
//! readers; a create that turns the flag off leaves the cluster with no access entry
//! at all.
//!
//! The rows span what the rule generalises over: how the flag is expressed (absent
//! `accessConfig`, an `accessConfig` without the flag, the flag set either way) and
//! the `authenticationMode` beside it, which the member's documentation ties nothing
//! to — the flag alone decides.
//!
//! WHICH principal the entry names is resolved from the request's own credential, a
//! question that needs the session registry and IAM beside this service: it is
//! pinned in `core/router/tests/all/eks_bootstrap_cluster_creator_access_entry.rs`.
//! Here every request signs as the account's own published credential, so the
//! creator is that credential's IAM user.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs` and the cluster
//! body comes from `cluster_lifecycle`: no suite grows its own client.

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_entry_authentication_mode_refusal, access_policy_arn,
    ACCESS_ENTRY_AUTHENTICATION_MODES, ACCESS_ENTRY_TYPE_STANDARD, ACCESS_SCOPE_TYPE_CLUSTER,
    CLUSTER_ADMIN_ACCESS_POLICY_NAME,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{minimal_request, REGION};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// The IAM principal every request in this suite speaks for: the account's own
/// published credential belongs to an IAM user of the account, and that user is the
/// identity `sts:GetCallerIdentity` reports for it. Read from the one definition of
/// it rather than written out again.
fn caller_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

async fn create_cluster(
    router: &MultiServiceRouter,
    name: &str,
    access_config: Option<serde_json::Value>,
) -> Answer {
    let mut body = minimal_request(name);
    if let Some(access_config) = access_config {
        body["accessConfig"] = access_config;
    }
    call(router, REGION, "POST", "/clusters", &body.to_string()).await
}

async fn list_entries(router: &MultiServiceRouter, cluster: &str, query: &str) -> Answer {
    let path = format!("/clusters/{}/access-entries", encode_label(cluster));
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// The principal ARNs a `ListAccessEntries` answer reports, in the order returned.
fn listed_principals(answer: &Answer) -> Vec<String> {
    answer.body["accessEntries"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn entry_path(cluster: &str, principal: &str) -> String {
    format!(
        "/clusters/{}/access-entries/{}",
        encode_label(cluster),
        encode_label(principal)
    )
}

async fn describe_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(router, REGION, "GET", &entry_path(cluster, principal), "").await
}

async fn create_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    let body = serde_json::json!({ "principalArn": principal });
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn delete_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &entry_path(cluster, principal),
        "",
    )
    .await
}

async fn list_associated(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!("{}/access-policies", entry_path(cluster, principal)),
        "",
    )
    .await
}

/// The associations a `ListAssociatedAccessPolicies` answer reports, in order.
fn listed_associations(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["associatedAccessPolicies"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

/// The ARN of the policy the bootstrap association names.
fn cluster_admin_policy_arn() -> String {
    access_policy_arn(CLUSTER_ADMIN_ACCESS_POLICY_NAME)
}

/// Assert the whole bootstrap surface of `cluster`: the listing, the entry itself and
/// its one association.
async fn assert_bootstrap_entry(router: &MultiServiceRouter, case: &str, cluster: &str) {
    let principal = caller_arn();

    let listed = list_entries(router, cluster, "").await;
    assert_eq!(
        (listed.status, listed_principals(&listed)),
        (200, vec![principal.clone()]),
        "case {case:?}: the creating principal must be the cluster's one access entry, \
         got {listed:?}"
    );

    let described = describe_entry(router, cluster, &principal).await;
    assert_eq!(
        described.status, 200,
        "case {case:?}: the creator's access entry must be readable, got {described:?}"
    );
    let entry = &described.body["accessEntry"];
    assert_eq!(
        (
            entry["clusterName"].as_str(),
            entry["principalArn"].as_str(),
            entry["username"].as_str(),
            entry["type"].as_str(),
        ),
        (
            Some(cluster),
            Some(principal.as_str()),
            Some(principal.as_str()),
            Some(ACCESS_ENTRY_TYPE_STANDARD),
        ),
        "case {case:?}: expected the creator's STANDARD entry named by its principal, \
         got {entry:?}"
    );
    let groups = entry["kubernetesGroups"].as_array().cloned().unwrap_or_default();
    assert!(
        groups.is_empty(),
        "case {case:?}: the bootstrap entry is given no Kubernetes group, got {entry:?}"
    );
    let entry_arn = entry["accessEntryArn"].as_str().unwrap_or_default();
    assert!(
        entry_arn.starts_with(&format!(
            "arn:aws:eks:{REGION}:000000000000:access-entry/{cluster}/"
        )),
        "case {case:?}: expected a generated access-entry ARN of this cluster, got \
         {entry_arn:?}"
    );
    for member in ["createdAt", "modifiedAt"] {
        assert!(
            entry[member].as_f64().is_some(),
            "case {case:?}: {member} must be a timestamp, got {entry:?}"
        );
    }

    let associated = list_associated(router, cluster, &principal).await;
    assert_eq!(
        associated.status, 200,
        "case {case:?}: the creator's associations must be readable, got {associated:?}"
    );
    let associations = listed_associations(&associated);
    assert_eq!(
        associations.len(),
        1,
        "case {case:?}: expected exactly one association, got {associated:?}"
    );
    let association = &associations[0];
    assert_eq!(
        (
            association["policyArn"].as_str(),
            association["accessScope"]["type"].as_str(),
        ),
        (
            Some(cluster_admin_policy_arn().as_str()),
            Some(ACCESS_SCOPE_TYPE_CLUSTER),
        ),
        "case {case:?}: expected the cluster-scoped cluster-admin association, got \
         {association:?}"
    );
    for member in ["associatedAt", "modifiedAt"] {
        assert!(
            association[member].as_f64().is_some(),
            "case {case:?}: {member} must be a timestamp, got {association:?}"
        );
    }
}

/// Assert `cluster` carries no access entry at all.
async fn assert_no_bootstrap_entry(router: &MultiServiceRouter, case: &str, cluster: &str) {
    let principal = caller_arn();

    let listed = list_entries(router, cluster, "").await;
    assert_eq!(
        (listed.status, listed_principals(&listed)),
        (200, Vec::new()),
        "case {case:?}: a create that turns the flag off must leave no access entry, \
         got {listed:?}"
    );

    let described = describe_entry(router, cluster, &principal).await;
    assert_eq!(
        (described.status, error_code(&described).as_str()),
        (404, "ResourceNotFoundException"),
        "case {case:?}: there is no entry for the creator to read, got {described:?}"
    );
}

/// Assert the access-entry API of `cluster` is refused for the cluster's
/// authentication mode.
///
/// A cluster whose `authenticationMode` does not include the API authenticates
/// only through the `aws-auth` ConfigMap, so its access-entry API is refused
/// whatever the bootstrap flag wrote: what the create DECIDED is then read off the
/// create's own answer, which is where the rows below read it.
async fn assert_access_entry_api_refused(
    router: &MultiServiceRouter,
    case: &str,
    cluster: &str,
) {
    let listed = list_entries(router, cluster, "").await;
    assert_eq!(
        (listed.status, error_code(&listed).as_str()),
        (400, "InvalidRequestException"),
        "case {case:?}: a cluster authenticating through the ConfigMap alone serves no \
         access-entry API, got {listed:?}"
    );
    assert_eq!(
        listed.body["message"].as_str(),
        Some(access_entry_authentication_mode_refusal().as_str()),
        "case {case:?}: expected the mode refusal, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the effective `bootstrapClusterCreatorAdminPermissions` — the value the
// request gave, or `true` when it gave none — decides whether the create
// materialises the creating principal's cluster-admin access entry.
// ---------------------------------------------------------------------------

/// The table spans how the flag is expressed and what sits beside it: no
/// `accessConfig` at all, an `accessConfig` carrying only the authentication mode,
/// and the flag set either way under each of the three `AuthenticationMode` members
/// (`smithy_query(query="shape",shape="AuthenticationMode")`), which the flag's own
/// documentation ties nothing to.
#[tokio::test]
async fn the_bootstrap_flag_decides_whether_the_creator_gets_an_access_entry() {
    let router = emulator();

    let cases: Vec<(&str, Option<serde_json::Value>, bool)> = vec![
        ("no accessConfig at all, so the flag defaults", None, true),
        (
            "an accessConfig carrying only the authentication mode",
            Some(serde_json::json!({ "authenticationMode": "API_AND_CONFIG_MAP" })),
            true,
        ),
        (
            "the flag true beside API_AND_CONFIG_MAP",
            Some(serde_json::json!({
                "authenticationMode": "API_AND_CONFIG_MAP",
                "bootstrapClusterCreatorAdminPermissions": true,
            })),
            true,
        ),
        (
            "the flag true beside API",
            Some(serde_json::json!({
                "authenticationMode": "API",
                "bootstrapClusterCreatorAdminPermissions": true,
            })),
            true,
        ),
        (
            "the flag true beside CONFIG_MAP",
            Some(serde_json::json!({
                "authenticationMode": "CONFIG_MAP",
                "bootstrapClusterCreatorAdminPermissions": true,
            })),
            true,
        ),
        (
            "the flag true and no authentication mode",
            Some(serde_json::json!({ "bootstrapClusterCreatorAdminPermissions": true })),
            true,
        ),
        (
            "the flag false beside API_AND_CONFIG_MAP",
            Some(serde_json::json!({
                "authenticationMode": "API_AND_CONFIG_MAP",
                "bootstrapClusterCreatorAdminPermissions": false,
            })),
            false,
        ),
        (
            "the flag false and no authentication mode",
            Some(serde_json::json!({ "bootstrapClusterCreatorAdminPermissions": false })),
            false,
        ),
    ];

    for (index, (case, access_config, expected)) in cases.iter().enumerate() {
        let cluster = format!("bootstrap-{index}");
        let created = create_cluster(&router, &cluster, access_config.clone()).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: creating the cluster must succeed, got {created:?}"
        );
        // The create's own answer reports the decision it took, which is the same
        // fact the entry it wrote (or did not write) is.
        assert_eq!(
            created.cluster()["accessConfig"]["bootstrapClusterCreatorAdminPermissions"].as_bool(),
            Some(*expected),
            "case {case:?}: the create must report the effective flag, got {created:?}"
        );

        // What the access-entry API can be ASKED about that decision depends on the
        // cluster's own authentication mode: a cluster that serves no access-entry
        // API refuses every call of it, so for those rows the create's answer above
        // is the whole observation.
        let mode = created.cluster()["accessConfig"]["authenticationMode"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        if !ACCESS_ENTRY_AUTHENTICATION_MODES.contains(&mode.as_str()) {
            assert_access_entry_api_refused(&router, case, &cluster).await;
            continue;
        }
        if *expected {
            assert_bootstrap_entry(&router, case, &cluster).await;
        } else {
            assert_no_bootstrap_entry(&router, case, &cluster).await;
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: the entry the bootstrap writes is an access entry like any other — the
// same record `CreateAccessEntry` writes — so every operation of the slice acts
// on it: a create for the same principal is the conflict a second create of one
// principal always is, the associated-policy filter finds it, and a delete
// removes it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_bootstrap_entry_is_an_access_entry_like_any_other() {
    let router = emulator();
    let cluster = "bootstrap-lifecycle";
    let principal = caller_arn();
    // The flag defaults to true, so the creator's entry is written; the mode beside
    // it is the one that serves the access-entry API, which every call below uses.
    let created = create_cluster(
        &router,
        cluster,
        Some(serde_json::json!({ "authenticationMode": "API_AND_CONFIG_MAP" })),
    )
    .await;
    assert_eq!(
        created.status, 200,
        "setup: creating the cluster must succeed, got {created:?}"
    );

    // "You can specify one ARN for each access entry. You can't specify the same ARN
    // in more than one access entry": the principal AWS already holds an entry for
    // cannot be given a second one.
    let again = create_entry(&router, cluster, &principal).await;
    assert_eq!(
        (again.status, error_code(&again).as_str()),
        (409, "ResourceInUseException"),
        "creating the creator's entry a second time must be refused, got {again:?}"
    );

    // "When you specify an access policy ARN, only the access entries associated to
    // that access policy are returned" — the bootstrap association is a real one, so
    // the filter finds the entry under the cluster-admin policy and not under
    // another published policy.
    let admin = cluster_admin_policy_arn();
    let filtered = list_entries(
        &router,
        cluster,
        &format!("associatedPolicyArn={}", encode_label(&admin)),
    )
    .await;
    assert_eq!(
        (filtered.status, listed_principals(&filtered)),
        (200, vec![principal.clone()]),
        "the creator's entry must be found under {admin:?}, got {filtered:?}"
    );
    let other = access_policy_arn("AmazonEKSViewPolicy");
    let unmatched = list_entries(
        &router,
        cluster,
        &format!("associatedPolicyArn={}", encode_label(&other)),
    )
    .await;
    assert_eq!(
        (unmatched.status, listed_principals(&unmatched)),
        (200, Vec::new()),
        "the creator's entry carries no association with {other:?}, got {unmatched:?}"
    );

    // The policy it is associated to is one of the policies this service PUBLISHES,
    // which is what a client discovers through `ListAccessPolicies` instead of
    // hardcoding an ARN.
    let catalog = call(&router, REGION, "GET", "/access-policies?maxResults=100", "").await;
    let published: Vec<String> = catalog.body["accessPolicies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["arn"].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    assert!(
        published.contains(&admin),
        "the bootstrap association must name a published access policy, got {published:?}"
    );

    // And it is removable, like any entry: once it is gone the cluster has none, and
    // the principal can be given an entry again.
    let deleted = delete_entry(&router, cluster, &principal).await;
    assert_eq!(
        deleted.status, 200,
        "deleting the creator's entry must succeed, got {deleted:?}"
    );
    let listed = list_entries(&router, cluster, "").await;
    assert_eq!(
        (listed.status, listed_principals(&listed)),
        (200, Vec::new()),
        "the cluster must hold no entry once the creator's is deleted, got {listed:?}"
    );
    let recreated = create_entry(&router, cluster, &principal).await;
    assert_eq!(
        recreated.status, 200,
        "the principal must be grantable an entry again, got {recreated:?}"
    );
    // A `CreateAccessEntry` grants no policy, so the re-created entry carries none:
    // the cluster-admin association came from the bootstrap, not from the principal.
    let associated = list_associated(&router, cluster, &principal).await;
    assert_eq!(
        (associated.status, listed_associations(&associated).len()),
        (200, 0),
        "a freshly created entry carries no association, got {associated:?}"
    );
}
