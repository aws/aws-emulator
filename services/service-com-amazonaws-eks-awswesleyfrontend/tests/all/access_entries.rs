// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the EKS access-entry slice: `CreateAccessEntry`,
//! `DescribeAccessEntry`, `AssociateAccessPolicy` and
//! `ListAssociatedAccessPolicies`, driven over the wire through the same
//! `MultiServiceRouter` the production binary mounts.
//!
//! The wire harness is the crate's one copy in `tests/all/wire.rs`, and the
//! cluster the entries hang off is created with `cluster_lifecycle`'s
//! `minimal_request`: no suite grows its own client.
//!
//! Every suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along what the rule generalises over (scope type and
//! namespace count, access-entry type, which optional members are supplied, page
//! size, how a policy ARN is malformed, which resource is missing), and each
//! assertion names its case and prints the actual value.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, encode_page_token, server_managed_kubernetes_groups,
    ACCESS_ENTRY_TYPE_STANDARD, ACCESS_POLICY_NAMES, DEFAULT_USERNAME_EC2_NODE,
    DEFAULT_USERNAME_FARGATE, RESOURCE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::cluster_lifecycle::{access_entry_cluster_request, REGION};
use crate::wire::{call, emulator, encode_label, Answer};

/// An EKS-managed access policy ARN: no region, `aws` as the account. Two of the
/// policies the Amazon EKS User Guide publishes ("Access policy permissions").
const VIEW_POLICY: &str = "arn:aws:eks::aws:cluster-access-policy/AmazonEKSViewPolicy";
/// See [`VIEW_POLICY`].
const CLUSTER_ADMIN_POLICY: &str =
    "arn:aws:eks::aws:cluster-access-policy/AmazonEKSClusterAdminPolicy";

/// The emulator's own canonical account id, which every ARN it mints carries.
const ACCOUNT: &str = "000000000000";

fn role_arn(name: &str) -> String {
    format!("arn:aws:iam::{ACCOUNT}:role/{name}")
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

async fn create_entry(
    router: &MultiServiceRouter,
    cluster: &str,
    body: &serde_json::Value,
) -> Answer {
    call(
        router,
        REGION,
        "POST",
        &format!("/clusters/{}/access-entries", encode_label(cluster)),
        &body.to_string(),
    )
    .await
}

async fn describe_entry(router: &MultiServiceRouter, cluster: &str, principal: &str) -> Answer {
    call(
        router,
        REGION,
        "GET",
        &format!(
            "/clusters/{}/access-entries/{}",
            encode_label(cluster),
            encode_label(principal)
        ),
        "",
    )
    .await
}

async fn delete_cluster(router: &MultiServiceRouter, cluster: &str) -> Answer {
    call(
        router,
        REGION,
        "DELETE",
        &format!("/clusters/{}", encode_label(cluster)),
        "",
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

async fn list_associated(
    router: &MultiServiceRouter,
    cluster: &str,
    principal: &str,
    query: &str,
) -> Answer {
    let path = format!(
        "/clusters/{}/access-entries/{}/access-policies",
        encode_label(cluster),
        encode_label(principal)
    );
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// A cluster with one `STANDARD` access entry on it, ready to associate policies
/// to. Returns the principal ARN the entry is keyed by.
async fn cluster_with_entry(router: &MultiServiceRouter, cluster: &str, role: &str) -> String {
    let created = create_cluster(router, cluster).await;
    assert_eq!(
        created.status, 200,
        "setup: creating cluster {cluster:?} must succeed, got {created:?}"
    );
    let principal = role_arn(role);
    let entry = create_entry(
        router,
        cluster,
        &serde_json::json!({ "principalArn": principal }),
    )
    .await;
    assert_eq!(
        entry.status, 200,
        "setup: creating the access entry for {principal:?} must succeed, got {entry:?}"
    );
    principal
}

/// The associated policies a `ListAssociatedAccessPolicies` answer carries, as
/// `(policyArn, whole AssociatedAccessPolicy)` pairs in the order returned.
fn listed_policies(answer: &Answer) -> Vec<(String, serde_json::Value)> {
    answer.body["associatedAccessPolicies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["policyArn"].as_str().unwrap_or_default().to_owned(),
                        item.clone(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// RULE: an access policy association reads back exactly as it was stored — the
// policy ARN and the scope the request gave, plus the server-generated
// `associatedAt` / `modifiedAt` — whatever the scope is.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: the two `AccessScopeType`
/// members, and for the namespace scope the NUMBER and SHAPE of the namespaces,
/// including ordering.
#[tokio::test]
async fn an_association_reads_back_exactly_as_stored() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "read-back", "read-back-role").await;

    let cases: Vec<(&str, &str, serde_json::Value)> = vec![
        (
            "cluster scope",
            CLUSTER_ADMIN_POLICY,
            serde_json::json!({ "type": "cluster" }),
        ),
        (
            "namespace scope, one namespace",
            VIEW_POLICY,
            serde_json::json!({ "type": "namespace", "namespaces": ["kube-system"] }),
        ),
        (
            "namespace scope, three namespaces in a non-sorted order",
            "arn:aws:eks::aws:cluster-access-policy/AmazonEKSEditPolicy",
            serde_json::json!({ "type": "namespace", "namespaces": ["zeta", "alpha", "mid"] }),
        ),
        (
            "namespace scope, a 63-character namespace name (the Kubernetes maximum)",
            "arn:aws:eks::aws:cluster-access-policy/AmazonEKSAdminPolicy",
            serde_json::json!({ "type": "namespace", "namespaces": ["n".repeat(63)] }),
        ),
    ];

    for (case, policy_arn, access_scope) in &cases {
        let answer = associate(&router, "read-back", &principal, policy_arn, access_scope).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        assert_eq!(
            answer.body["clusterName"].as_str(),
            Some("read-back"),
            "case {case:?}: the answer must echo the cluster, got {answer:?}"
        );
        assert_eq!(
            answer.body["principalArn"].as_str(),
            Some(principal.as_str()),
            "case {case:?}: the answer must echo the principal, got {answer:?}"
        );
        let association = &answer.body["associatedAccessPolicy"];
        assert_eq!(
            association["policyArn"].as_str(),
            Some(*policy_arn),
            "case {case:?}: expected the requested policyArn, got {answer:?}"
        );
        assert_eq!(
            &association["accessScope"]["type"], &access_scope["type"],
            "case {case:?}: expected the requested scope type, got {answer:?}"
        );
        let expected_namespaces = access_scope
            .get("namespaces")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        assert_eq!(
            &association["accessScope"]["namespaces"], &expected_namespaces,
            "case {case:?}: expected the requested namespaces, in order, got {answer:?}"
        );
        for member in ["associatedAt", "modifiedAt"] {
            assert!(
                association[member].as_f64().is_some(),
                "case {case:?}: {member} must be a timestamp, got {answer:?}"
            );
        }

        // The read-back: what the list reports for this policy is byte-for-byte the
        // association the write answered with.
        let listed = list_associated(&router, "read-back", &principal, "").await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: listing must succeed, got {listed:?}"
        );
        let found = listed_policies(&listed)
            .into_iter()
            .find(|(arn, _)| arn == policy_arn);
        assert_eq!(
            found.map(|(_, value)| value),
            Some(association.clone()),
            "case {case:?}: the list must report exactly what was stored, got {listed:?}"
        );
    }

    // Every row accumulated on the same entry, each under its own policy ARN.
    let listed = list_associated(&router, "read-back", &principal, "").await;
    assert_eq!(
        listed_policies(&listed).len(),
        cases.len(),
        "the entry must hold one association per row, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an access policy is associated TO a resource, so the cluster and the
// access entry are resolved BEFORE the request body is judged: a request naming a
// resource that does not exist is `ResourceNotFoundException` (404) whatever its
// scope says — including a scope that is itself invalid.
// ---------------------------------------------------------------------------

/// The table crosses every shape of scope (valid, invalid and absent) with every
/// resource that can be missing.
#[tokio::test]
async fn a_missing_resource_is_not_found_whatever_the_scope_says() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "present", "present-role").await;

    let scopes: Vec<(&str, serde_json::Value)> = vec![
        ("valid cluster scope", serde_json::json!({ "type": "cluster" })),
        (
            "valid namespace scope",
            serde_json::json!({ "type": "namespace", "namespaces": ["kube-system"] }),
        ),
        (
            "namespace scope with an empty namespace list",
            serde_json::json!({ "type": "namespace", "namespaces": [] }),
        ),
        (
            "namespace scope with no namespaces at all",
            serde_json::json!({ "type": "namespace" }),
        ),
        ("no scope type", serde_json::json!({})),
        // An `accessScope.type` OUTSIDE the `AccessScopeType` enum is not in this
        // table: the schema validator refuses it (400 `InvalidParameterException`)
        // before any handler runs, so it never reaches the resource lookup this rule
        // is about. `an_invalid_scope_or_policy_arn_is_refused` covers it.
    ];
    // Each target names a (cluster, principal) pair where something is missing.
    let targets: Vec<(&str, &str, String)> = vec![
        ("no such cluster", "absent-cluster", principal.clone()),
        (
            "the cluster exists, the access entry does not",
            "present",
            role_arn("never-created"),
        ),
    ];

    for (target, cluster, target_principal) in &targets {
        for (scope_case, access_scope) in &scopes {
            let answer =
                associate(&router, cluster, target_principal, VIEW_POLICY, access_scope).await;
            assert_eq!(
                answer.status, 404,
                "case {target:?} + {scope_case:?}: expected 404, got {answer:?}"
            );
            assert_eq!(
                answer.error_type.as_deref(),
                Some("ResourceNotFoundException"),
                "case {target:?} + {scope_case:?}: expected ResourceNotFoundException, \
                 got {answer:?}"
            );
            // One not-found message across the whole access-entry slice, whichever
            // resource is missing: a client branches on the code, not the text.
            assert_eq!(
                answer.body["message"].as_str(),
                Some(RESOURCE_NOT_FOUND_MESSAGE),
                "case {target:?} + {scope_case:?}: expected the slice's not-found \
                 message, got {answer:?}"
            );
        }
    }

    // None of those refusals wrote anything to the entry that does exist.
    let listed = list_associated(&router, "present", &principal, "").await;
    assert!(
        listed_policies(&listed).is_empty(),
        "a refused association must store nothing, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: with both resources present, the scope must be one of the two
// `AccessScopeType` members, a `namespace` scope must name at least one non-blank
// namespace, a `cluster` scope must name none, and `policyArn` must be an
// EKS-managed access-policy ARN — each refused with `InvalidParameterException`
// (400) and nothing stored.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_invalid_scope_or_policy_arn_is_refused() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "validation", "validation-role").await;

    let bad_scopes: Vec<(&str, serde_json::Value)> = vec![
        (
            "namespace scope with an empty namespace list",
            serde_json::json!({ "type": "namespace", "namespaces": [] }),
        ),
        (
            "namespace scope with no namespaces member",
            serde_json::json!({ "type": "namespace" }),
        ),
        (
            "namespace scope with a blank namespace among valid ones",
            serde_json::json!({ "type": "namespace", "namespaces": ["kube-system", "  "] }),
        ),
        (
            "cluster scope carrying namespaces",
            serde_json::json!({ "type": "cluster", "namespaces": ["kube-system"] }),
        ),
        // The next two are refused by the schema validator rather than by the
        // handler — `AccessScope$type` is the `AccessScopeType` enum, whose members
        // are lowercase — and both paths answer 400 `InvalidParameterException`.
        (
            "a scope type in the wrong case (the enum is lowercase)",
            serde_json::json!({ "type": "Cluster" }),
        ),
        ("an unknown scope type", serde_json::json!({ "type": "node" })),
        ("no scope type", serde_json::json!({})),
    ];
    for (case, access_scope) in &bad_scopes {
        let answer = associate(&router, "validation", &principal, VIEW_POLICY, access_scope).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }

    // The rows differ along HOW the ARN is not an EKS access-policy ARN: a bare
    // name, another service's managed policy, an account- and region-scoped ARN,
    // an empty policy name, and a name carrying a further path segment.
    let bad_policy_arns = [
        "AmazonEKSViewPolicy",
        "arn:aws:iam::aws:policy/ReadOnlyAccess",
        &format!("arn:aws:eks:{REGION}:{ACCOUNT}:cluster-access-policy/AmazonEKSViewPolicy"),
        "arn:aws:eks::aws:cluster-access-policy/",
        "arn:aws:eks::aws:cluster-access-policy/nested/AmazonEKSViewPolicy",
        "arn:aws:eks::aws:access-policy/AmazonEKSViewPolicy",
        "",
    ];
    for case in bad_policy_arns {
        let answer = associate(
            &router,
            "validation",
            &principal,
            case,
            &serde_json::json!({ "type": "cluster" }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a policyArn that is not an EKS access \
             policy, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }

    let listed = list_associated(&router, "validation", &principal, "").await;
    assert!(
        listed_policies(&listed).is_empty(),
        "no refused association may be stored, got {listed:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: an association is keyed by policy ARN — re-associating a policy already on
// the entry REPLACES its scope, keeping `associatedAt` and moving `modifiedAt`,
// and never adds a second record.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn re_associating_a_policy_replaces_its_scope() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "replace", "replace-role").await;

    // The rows walk every transition between the scope shapes, so the rule is
    // asserted in both directions rather than once.
    let scopes = [
        serde_json::json!({ "type": "cluster" }),
        serde_json::json!({ "type": "namespace", "namespaces": ["kube-system"] }),
        serde_json::json!({ "type": "namespace", "namespaces": ["a", "b", "c"] }),
        serde_json::json!({ "type": "cluster" }),
    ];

    let first = associate(&router, "replace", &principal, VIEW_POLICY, &scopes[0]).await;
    assert_eq!(first.status, 200, "the first association: {first:?}");
    let associated_at = first.body["associatedAccessPolicy"]["associatedAt"]
        .as_f64()
        .expect("the first association carries associatedAt");

    for (step, scope) in scopes.iter().enumerate().skip(1) {
        let answer = associate(&router, "replace", &principal, VIEW_POLICY, scope).await;
        assert_eq!(
            answer.status, 200,
            "case step {step} ({scope}): expected 200, got {answer:?}"
        );
        let association = &answer.body["associatedAccessPolicy"];
        assert_eq!(
            association["associatedAt"].as_f64(),
            Some(associated_at),
            "case step {step} ({scope}): associatedAt must keep the instant the policy \
             first landed, got {answer:?}"
        );
        let modified_at = association["modifiedAt"]
            .as_f64()
            .expect("an association carries modifiedAt");
        assert!(
            modified_at >= associated_at,
            "case step {step} ({scope}): modifiedAt {modified_at} must not precede \
             associatedAt {associated_at}"
        );
        assert_eq!(
            &association["accessScope"]["type"], &scope["type"],
            "case step {step} ({scope}): the scope must be the new one, got {answer:?}"
        );

        let listed = list_associated(&router, "replace", &principal, "").await;
        let policies = listed_policies(&listed);
        assert_eq!(
            policies.len(),
            1,
            "case step {step} ({scope}): re-associating must not add a record, got \
             {listed:?}"
        );
        assert_eq!(
            policies[0].1,
            *association,
            "case step {step} ({scope}): the stored association must be the new one, \
             got {listed:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `ListAssociatedAccessPolicies` walks the entry's associations in sorted
// policy-ARN order; whatever page size it is driven at, following `nextToken` to
// exhaustion yields each association exactly once and the last page carries no
// token. A token this service did not issue is refused.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_associated_policies_at_any_size_yields_each_once() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "paging", "paging-role").await;

    // The catalogue's own names, whose sorted ARN order is not the order they are
    // associated in below.
    let mut names: Vec<&&str> = ACCESS_POLICY_NAMES.iter().collect();
    names.reverse();
    let mut stored: Vec<String> = Vec::new();
    for name in names {
        let policy_arn = access_policy_arn(name);
        let answer = associate(
            &router,
            "paging",
            &principal,
            &policy_arn,
            &serde_json::json!({ "type": "cluster" }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "setup {policy_arn:?}: expected 200, got {answer:?}"
        );
        stored.push(policy_arn);
    }
    stored.sort();

    for page_size in [1, 2, 3, 5, 6, 100] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match &token {
                None => format!("maxResults={page_size}"),
                Some(value) => format!("maxResults={page_size}&nextToken={value}"),
            };
            let answer = list_associated(&router, "paging", &principal, &query).await;
            assert_eq!(
                answer.status, 200,
                "case maxResults={page_size}: expected 200, got {answer:?}"
            );
            let page = listed_policies(&answer);
            assert!(
                page.len() <= page_size,
                "case maxResults={page_size}: a page must not exceed the limit, got \
                 {answer:?}"
            );
            seen.extend(page.iter().map(|(arn, _)| arn.clone()));
            pages += 1;
            assert!(
                pages <= stored.len() + 1,
                "case maxResults={page_size}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case maxResults={page_size}: a token was returned for an empty \
                         page, got {answer:?}"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, stored,
            "case maxResults={page_size}: paging must yield every policy once, in \
             sorted ARN order; got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case maxResults={page_size}: a policy was repeated across pages: {seen:?}"
        );
    }

    // A token this service did not issue is refused rather than resumed from, as the
    // MALFORMED PARAMETER it is — the class every listing of this service answers a
    // page token it did not mint with, rather than the state-dependent
    // `InvalidRequestException`. The rows differ along HOW the token is foreign: not
    // hex, an odd length, a well-formed encoding carrying no provenance tag, and —
    // the one a scope-less token check would wave through — a token another listing
    // of this same service minted (`ListClusters`).
    let untagged: String = VIEW_POLICY.bytes().map(|byte| format!("{byte:02x}")).collect();
    let other_listing = encode_page_token("paging");
    for case in [
        "not-hex",
        "zz",
        "abc",
        "ffffffffff",
        untagged.as_str(),
        other_listing.as_str(),
    ] {
        let answer =
            list_associated(&router, "paging", &principal, &format!("nextToken={case}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400 for a foreign nextToken, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the policies that can be associated are exactly the catalogue
// `ListAccessPolicies` publishes — every ARN in it associates, a well-formed ARN
// naming an unpublished policy is refused, and since each is keyed once an entry
// holds at most one record per published policy however often it is called.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn only_a_published_access_policy_can_be_associated() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "catalogue", "catalogue-role").await;

    // Every row of the published catalogue, not only the policies this issue cited.
    for name in ACCESS_POLICY_NAMES {
        let answer = associate(
            &router,
            "catalogue",
            &principal,
            &access_policy_arn(name),
            &serde_json::json!({ "type": "cluster" }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?}: a published access policy must associate, got {answer:?}"
        );
    }
    // Associating the whole catalogue a second time re-scopes each record rather
    // than adding one.
    for name in ACCESS_POLICY_NAMES {
        let answer = associate(
            &router,
            "catalogue",
            &principal,
            &access_policy_arn(name),
            &serde_json::json!({ "type": "namespace", "namespaces": ["kube-system"] }),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {name:?} re-associated: expected 200, got {answer:?}"
        );
    }
    let listed = list_associated(&router, "catalogue", &principal, "maxResults=100").await;
    assert_eq!(
        listed_policies(&listed).len(),
        ACCESS_POLICY_NAMES.len(),
        "the entry must hold one record per published policy, got {listed:?}"
    );

    // The rows differ along how the name misses the catalogue: an invented policy,
    // a published name in the wrong case, and a name that is a prefix of one.
    for case in [
        "AmazonEKSNoSuchPolicy",
        "amazoneksviewpolicy",
        "AmazonEKSView",
        "",
    ] {
        let answer = associate(
            &router,
            "catalogue",
            &principal,
            &access_policy_arn(case),
            &serde_json::json!({ "type": "cluster" }),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: an unpublished policy name must be refused, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
    }
    let after = list_associated(&router, "catalogue", &principal, "maxResults=100").await;
    assert_eq!(
        listed_policies(&after).len(),
        ACCESS_POLICY_NAMES.len(),
        "a refused association must change nothing, got {after:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: a created access entry reads back exactly as stored — the members the
// request supplied, plus the server-generated `accessEntryArn`, `createdAt`,
// `modifiedAt` and the `type` / `username` EKS fills in when the caller does not.
// ---------------------------------------------------------------------------

/// The table spans what the rule generalises over: which optional members are
/// supplied, and the entry `type`, which is what decides the username EKS picks.
#[tokio::test]
async fn a_created_access_entry_reads_back_exactly_as_stored() {
    let router = emulator();
    let created = create_cluster(&router, "entries").await;
    assert_eq!(created.status, 200, "setup: {created:?}");

    let cases: Vec<(&str, serde_json::Value, &str, String)> = vec![
        (
            "only the required member",
            serde_json::json!({ "principalArn": role_arn("standard") }),
            ACCESS_ENTRY_TYPE_STANDARD,
            role_arn("standard"),
        ),
        (
            "every member this slice stores",
            serde_json::json!({
                "principalArn": role_arn("full"),
                "type": "STANDARD",
                "username": "my-own-username",
                "kubernetesGroups": ["viewers", "auditors"],
                "tags": { "env": "dev", "team": "platform" },
                "clientRequestToken": "token-full",
            }),
            ACCESS_ENTRY_TYPE_STANDARD,
            "my-own-username".to_string(),
        ),
        (
            "an IAM user principal on a STANDARD entry",
            serde_json::json!({
                "principalArn": format!("arn:aws:iam::{ACCOUNT}:user/alice"),
                "kubernetesGroups": ["viewers"],
            }),
            ACCESS_ENTRY_TYPE_STANDARD,
            format!("arn:aws:iam::{ACCOUNT}:user/alice"),
        ),
        (
            "a blank username, which names nobody and so falls back to the default",
            serde_json::json!({ "principalArn": role_arn("blank-username"), "username": "   " }),
            ACCESS_ENTRY_TYPE_STANDARD,
            role_arn("blank-username"),
        ),
        (
            "a self-managed Linux node entry, whose username EKS templates",
            serde_json::json!({ "principalArn": role_arn("ec2-linux"), "type": "EC2_LINUX" }),
            "EC2_LINUX",
            DEFAULT_USERNAME_EC2_NODE.to_string(),
        ),
        (
            "a self-managed Windows node entry",
            serde_json::json!({ "principalArn": role_arn("ec2-win"), "type": "EC2_WINDOWS" }),
            "EC2_WINDOWS",
            DEFAULT_USERNAME_EC2_NODE.to_string(),
        ),
        (
            "a Fargate entry, whose username is session-based",
            serde_json::json!({ "principalArn": role_arn("fargate"), "type": "FARGATE_LINUX" }),
            "FARGATE_LINUX",
            DEFAULT_USERNAME_FARGATE.to_string(),
        ),
        (
            "a hybrid-node entry",
            serde_json::json!({ "principalArn": role_arn("hybrid"), "type": "HYBRID_LINUX" }),
            "HYBRID_LINUX",
            role_arn("hybrid"),
        ),
    ];

    for (case, request, expected_type, expected_username) in &cases {
        let answer = create_entry(&router, "entries", request).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected 200, got {answer:?}"
        );
        let entry = &answer.body["accessEntry"];
        let principal = request["principalArn"].as_str().expect("a principal ARN");
        assert_eq!(
            entry["clusterName"].as_str(),
            Some("entries"),
            "case {case:?}: expected the cluster name, got {answer:?}"
        );
        assert_eq!(
            entry["principalArn"].as_str(),
            Some(principal),
            "case {case:?}: expected the request's principal, got {answer:?}"
        );
        assert_eq!(
            entry["type"].as_str(),
            Some(*expected_type),
            "case {case:?}: expected type {expected_type}, got {answer:?}"
        );
        assert_eq!(
            entry["username"].as_str(),
            Some(expected_username.as_str()),
            "case {case:?}: expected username {expected_username:?}, got {answer:?}"
        );
        // The groups a created entry reports: the request's own when it sent any,
        // and otherwise the ones Amazon EKS derives for the entry's TYPE — the node
        // types get the node-authorization group even though the request sends none
        // (`server_managed_kubernetes_groups`).
        let expected_groups = request
            .get("kubernetesGroups")
            .filter(|groups| groups.as_array().is_some_and(|groups| !groups.is_empty()))
            .cloned()
            .unwrap_or_else(|| {
                serde_json::json!(server_managed_kubernetes_groups(expected_type))
            });
        assert_eq!(
            &entry["kubernetesGroups"], &expected_groups,
            "case {case:?}: expected {expected_groups:?}, got {answer:?}"
        );
        assert_eq!(
            &entry["tags"],
            &request.get("tags").cloned().unwrap_or(serde_json::json!({})),
            "case {case:?}: expected the request's tags, got {answer:?}"
        );
        for member in ["createdAt", "modifiedAt"] {
            assert!(
                entry[member].as_f64().is_some(),
                "case {case:?}: {member} must be a timestamp, got {answer:?}"
            );
        }

        // The ARN EKS publishes for an access entry: regional, account-scoped, and
        // carrying the principal's own resource type, account and name plus a
        // server-generated id.
        let arn = entry["accessEntryArn"].as_str().unwrap_or_default();
        let (principal_account, principal_resource) = principal
            .split_once(":iam::")
            .and_then(|(_, tail)| tail.split_once(':'))
            .expect("the table's principals are IAM ARNs");
        let expected_prefix = format!(
            "arn:aws:eks:{REGION}:{ACCOUNT}:access-entry/entries/{}/{principal_account}/{}/",
            principal_resource
                .split_once('/')
                .expect("an IAM principal ARN names a resource type")
                .0,
            principal_resource
                .split_once('/')
                .expect("an IAM principal ARN names a resource")
                .1,
        );
        assert!(
            arn.starts_with(&expected_prefix),
            "case {case:?}: expected an access-entry ARN starting {expected_prefix:?}, \
             got {arn:?}"
        );
        let id = &arn[expected_prefix.len()..];
        let widths: Vec<usize> = id.split('-').map(str::len).collect();
        assert_eq!(
            widths,
            vec![8, 4, 4, 4, 12],
            "case {case:?}: expected a 8-4-4-4-12 identifier, got {id:?}"
        );
        assert!(
            id.chars().all(|c| c.is_ascii_hexdigit() || c == '-'),
            "case {case:?}: expected a hex identifier, got {id:?}"
        );

        // The read-back: Describe reports exactly what Create stored, twice over —
        // a read must not change what the next reader sees.
        for attempt in 1..=2 {
            let described = describe_entry(&router, "entries", principal).await;
            assert_eq!(
                described.status, 200,
                "case {case:?} read {attempt}: expected 200, got {described:?}"
            );
            assert_eq!(
                described.body["accessEntry"], *entry,
                "case {case:?} read {attempt}: Describe must report what Create stored, \
                 got {described:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: `CreateAccessEntry` refuses a request the access-entry contract forbids:
// a principal that is not an IAM user or role, a node-type entry whose principal
// is not a role, Kubernetes groups on a self-managed EC2 node entry, and a second
// entry for a principal that already has one (absorbed only as an idempotent
// retry).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn create_access_entry_refuses_what_the_contract_forbids() {
    let router = emulator();
    let created = create_cluster(&router, "refusals").await;
    assert_eq!(created.status, 200, "setup: {created:?}");

    // The rows differ along HOW the principal is not an IAM user or role ARN.
    let bad_requests: Vec<(&str, serde_json::Value)> = vec![
        (
            "a bare role name",
            serde_json::json!({ "principalArn": "eksAdminRole" }),
        ),
        (
            "an STS session principal",
            serde_json::json!({
                "principalArn": format!("arn:aws:sts::{ACCOUNT}:assumed-role/admin/session"),
            }),
        ),
        (
            "a service principal rather than an IAM one",
            serde_json::json!({ "principalArn": format!("arn:aws:eks:{REGION}:{ACCOUNT}:cluster/x") }),
        ),
        (
            "an IAM ARN with no resource type",
            serde_json::json!({ "principalArn": format!("arn:aws:iam::{ACCOUNT}:root") }),
        ),
        (
            "an IAM ARN with a non-numeric account",
            serde_json::json!({ "principalArn": "arn:aws:iam::not-an-account:role/admin" }),
        ),
        (
            "an IAM ARN whose account is not a 12-digit id",
            serde_json::json!({ "principalArn": "arn:aws:iam::12345:role/admin" }),
        ),
        (
            "an IAM ARN carrying a region",
            serde_json::json!({
                "principalArn": format!("arn:aws:iam:{REGION}:{ACCOUNT}:role/admin"),
            }),
        ),
        (
            "an unknown access-entry type",
            serde_json::json!({ "principalArn": role_arn("typed"), "type": "EC2_PLAN9" }),
        ),
        (
            "a node-type entry whose principal is an IAM user",
            serde_json::json!({
                "principalArn": format!("arn:aws:iam::{ACCOUNT}:user/node"),
                "type": "EC2_LINUX",
            }),
        ),
        (
            "Kubernetes groups on a self-managed Linux node entry",
            serde_json::json!({
                "principalArn": role_arn("grouped-node"),
                "type": "EC2_LINUX",
                "kubernetesGroups": ["viewers"],
            }),
        ),
        (
            "Kubernetes groups on a self-managed Windows node entry",
            serde_json::json!({
                "principalArn": role_arn("grouped-node-win"),
                "type": "EC2_WINDOWS",
                "kubernetesGroups": ["viewers"],
            }),
        ),
    ];
    for (case, request) in &bad_requests {
        let answer = create_entry(&router, "refusals", request).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?}: expected InvalidParameterException, got {answer:?}"
        );
        let principal = request["principalArn"].as_str().unwrap_or_default();
        let described = describe_entry(&router, "refusals", principal).await;
        assert_eq!(
            described.status, 404,
            "case {case:?}: a refused create must store nothing, got {described:?}"
        );
    }

    // A cluster that does not exist is not found, whatever the entry looks like.
    let absent = create_entry(
        &router,
        "no-such-cluster",
        &serde_json::json!({ "principalArn": role_arn("anything") }),
    )
    .await;
    assert_eq!(absent.status, 404, "an absent cluster: {absent:?}");
    assert_eq!(
        absent.error_type.as_deref(),
        Some("ResourceNotFoundException"),
        "an absent cluster: {absent:?}"
    );

    // One principal, one access entry. A second create conflicts — unless it
    // carries the idempotency token of the create that already succeeded, which is
    // a retry of that same call.
    let request = serde_json::json!({
        "principalArn": role_arn("unique"),
        "clientRequestToken": "token-unique",
    });
    let first = create_entry(&router, "refusals", &request).await;
    assert_eq!(first.status, 200, "the first create: {first:?}");
    let retry = create_entry(&router, "refusals", &request).await;
    assert_eq!(
        retry.status, 200,
        "case idempotent retry: expected the stored entry, got {retry:?}"
    );
    assert_eq!(
        retry.body["accessEntry"], first.body["accessEntry"],
        "case idempotent retry: must answer with the entry the create wrote, got {retry:?}"
    );
    for (case, token) in [
        ("a different token", Some("token-other")),
        ("no token at all", None),
    ] {
        let mut conflicting = serde_json::json!({ "principalArn": role_arn("unique") });
        if let Some(token) = token {
            conflicting["clientRequestToken"] = serde_json::json!(token);
        }
        let answer = create_entry(&router, "refusals", &conflicting).await;
        assert_eq!(
            answer.status, 409,
            "case {case:?}: expected 409, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("ResourceInUseException"),
            "case {case:?}: expected ResourceInUseException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a self-managed Amazon EC2 node access entry (`EC2_LINUX`, `EC2_WINDOWS`)
// carries no access policies, so associating one is refused with
// `InvalidRequestException` given the state of the entry.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_self_managed_node_entry_takes_no_access_policy() {
    let router = emulator();
    let created = create_cluster(&router, "node-entries").await;
    assert_eq!(created.status, 200, "setup: {created:?}");

    // The rows cross the two node types that forbid policies with the two scope
    // shapes, and include a type that DOES take policies as the control.
    let cases = [
        ("EC2_LINUX", true),
        ("EC2_WINDOWS", true),
        ("FARGATE_LINUX", false),
        ("STANDARD", false),
    ];
    for (entry_type, refused) in cases {
        let principal = role_arn(&format!("node-{}", entry_type.to_ascii_lowercase()));
        let entry = create_entry(
            &router,
            "node-entries",
            &serde_json::json!({ "principalArn": principal, "type": entry_type }),
        )
        .await;
        assert_eq!(
            entry.status, 200,
            "setup {entry_type}: creating the entry must succeed, got {entry:?}"
        );

        for scope in [
            serde_json::json!({ "type": "cluster" }),
            serde_json::json!({ "type": "namespace", "namespaces": ["kube-system"] }),
        ] {
            let answer =
                associate(&router, "node-entries", &principal, VIEW_POLICY, &scope).await;
            if refused {
                assert_eq!(
                    answer.status, 400,
                    "case {entry_type} + {scope}: expected 400, got {answer:?}"
                );
                assert_eq!(
                    answer.error_type.as_deref(),
                    Some("InvalidRequestException"),
                    "case {entry_type} + {scope}: expected InvalidRequestException, got \
                     {answer:?}"
                );
                let listed = list_associated(&router, "node-entries", &principal, "").await;
                assert!(
                    listed_policies(&listed).is_empty(),
                    "case {entry_type} + {scope}: nothing may be stored, got {listed:?}"
                );
            } else {
                assert_eq!(
                    answer.status, 200,
                    "case {entry_type} + {scope}: this type takes access policies, got \
                     {answer:?}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: an access entry belongs to its cluster — deleting the cluster takes its
// access entries, and their associations, with it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn deleting_a_cluster_deletes_its_access_entries() {
    let router = emulator();
    let principal = cluster_with_entry(&router, "ephemeral", "ephemeral-role").await;
    let associated = associate(
        &router,
        "ephemeral",
        &principal,
        VIEW_POLICY,
        &serde_json::json!({ "type": "cluster" }),
    )
    .await;
    assert_eq!(associated.status, 200, "setup: {associated:?}");

    let deleted = delete_cluster(&router, "ephemeral").await;
    assert_eq!(deleted.status, 200, "deleting the cluster: {deleted:?}");

    for (case, answer) in [
        (
            "describing the entry",
            describe_entry(&router, "ephemeral", &principal).await,
        ),
        (
            "listing its policies",
            list_associated(&router, "ephemeral", &principal, "").await,
        ),
        (
            "associating another policy",
            associate(
                &router,
                "ephemeral",
                &principal,
                CLUSTER_ADMIN_POLICY,
                &serde_json::json!({ "type": "cluster" }),
            )
            .await,
        ),
    ] {
        assert_eq!(
            answer.status, 404,
            "case {case:?}: expected 404 once the cluster is gone, got {answer:?}"
        );
    }

    // A cluster recreated under the same name starts with no access entries: the
    // deleted record took them with it.
    let recreated = create_cluster(&router, "ephemeral").await;
    assert_eq!(recreated.status, 200, "recreating the cluster: {recreated:?}");
    let described = describe_entry(&router, "ephemeral", &principal).await;
    assert_eq!(
        described.status, 404,
        "a recreated cluster must hold no access entries, got {described:?}"
    );
}
