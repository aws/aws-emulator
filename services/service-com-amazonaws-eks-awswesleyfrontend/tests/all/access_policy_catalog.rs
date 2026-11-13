// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ListAccessPolicies`: the access-policy catalogue Amazon
//! EKS publishes, driven over the wire through the same `MultiServiceRouter` the
//! production binary mounts.
//!
//! Each suite asserts a RULE over a TABLE of inputs rather than one recorded
//! request: the rows differ along the Region the request is signed for, the page
//! size the listing is driven at, the shape of a foreign page token, and whether
//! the account holds any cluster at all.

use std::collections::BTreeSet;

use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, Answer};

/// The access policies this emulator must report, in the order a sorted listing
/// walks them.
///
/// Written out here rather than imported from `crate::state`, so the test pins the
/// catalogue against its SOURCE instead of against the handler's own copy. That
/// source is the EKS "Access policy permissions" reference's table of Amazon EKS
/// access policies: the policies whose permissions mirror the Kubernetes
/// `cluster-admin`, `admin`, `edit` and `view` cluster roles that the model's own
/// `AccessPolicy` documentation points at, the read-only mirror of `admin`, the
/// policies Amazon EKS publishes for the controllers and Amazon Web Services
/// services that work on a cluster (each Auto Mode capability controller both
/// namespace-scoped, `AmazonEKS<Capability>Policy`, and cluster-scoped,
/// `AmazonEKS<Capability>ClusterPolicy`), and the policies for the cluster
/// capabilities EKS manages (events, the Kube Resource Orchestrator, Pod Identity
/// and secret admin/reader access), service-account impersonation, and the
/// policies published for the Amazon Web Services services that orchestrate
/// workloads on a cluster (Amazon EMR on EKS and Amazon SageMaker HyperPod,
/// whose family covers its cluster and controller access, its inference, space
/// and space-template components, the system namespaces its orchestration owns,
/// its training jobs and the cluster-wide access a HyperPod user is granted).
const EXPECTED_POLICIES: &[&str] = &[
    "AIDevOpsAgentAccessPolicy",
    "AWSBatchClusterPolicy",
    "AWSBatchNamespacePolicy",
    "AmazonAIOpsAssistantPolicy",
    "AmazonARCRegionSwitchScalingPolicy",
    "AmazonEKSACKPolicy",
    "AmazonEKSAdminPolicy",
    "AmazonEKSAdminViewPolicy",
    "AmazonEKSArgoCDClusterPolicy",
    "AmazonEKSArgoCDPolicy",
    "AmazonEKSAutoNodePolicy",
    "AmazonEKSBlockStorageClusterPolicy",
    "AmazonEKSBlockStoragePolicy",
    "AmazonEKSClusterAdminPolicy",
    "AmazonEKSComputeClusterPolicy",
    "AmazonEKSComputePolicy",
    "AmazonEKSEditPolicy",
    "AmazonEKSEventPolicy",
    "AmazonEKSHybridPolicy",
    "AmazonEKSKROPolicy",
    "AmazonEKSLoadBalancingClusterPolicy",
    "AmazonEKSLoadBalancingPolicy",
    "AmazonEKSNetworkingClusterPolicy",
    "AmazonEKSNetworkingPolicy",
    "AmazonEKSPodIdentityPolicy",
    "AmazonEKSSecretAdminPolicy",
    "AmazonEKSSecretReaderPolicy",
    "AmazonEKSServiceAccountImpersonationPolicy",
    "AmazonEKSViewPolicy",
    "AmazonEMRClusterPolicy",
    "AmazonEMRJobPolicy",
    "AmazonSagemakerHyperpodClusterPolicy",
    "AmazonSagemakerHyperpodControllerPolicy",
    "AmazonSagemakerHyperpodInferencePolicy",
    "AmazonSagemakerHyperpodSpacePolicy",
    "AmazonSagemakerHyperpodSpaceTemplatePolicy",
    "AmazonSagemakerHyperpodSystemNamespacePolicy",
    "AmazonSagemakerHyperpodTrainingPolicy",
    "AmazonSagemakerHyperpodUserClusterPolicy",
];

/// The ARN prefix an access policy carries: partition `aws`, service `eks`, EMPTY
/// Region, and the literal `aws` where an account-owned resource carries an
/// account id — the service, not the caller's account, owns these.
const EXPECTED_ARN_PREFIX: &str = "arn:aws:eks::aws:cluster-access-policy/";

const REGION: &str = "us-west-2";

async fn list_in(router: &MultiServiceRouter, region: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/access-policies".to_string()
    } else {
        format!("/access-policies?{query}")
    };
    call(router, region, "GET", &uri, "").await
}

async fn list(router: &MultiServiceRouter, query: &str) -> Answer {
    list_in(router, REGION, query).await
}

/// The `(name, arn)` pairs one page reports, in the order they arrived.
fn entries_of(answer: &Answer) -> Vec<(String, String)> {
    answer.body["accessPolicies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .map(|item| {
                    (
                        item["name"].as_str().unwrap_or_default().to_owned(),
                        item["arn"].as_str().unwrap_or_default().to_owned(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

fn names_of(answer: &Answer) -> Vec<String> {
    entries_of(answer)
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

// ---------------------------------------------------------------------------
// RULE: the listing reports the service-managed access-policy catalogue — every
// published policy exactly once, each with the ARN its name determines — and that
// answer depends on nothing in the request or in the account's own resources.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_catalogue_is_reported_whole_and_is_the_same_for_every_caller() {
    let router = emulator();

    // Rows: the Region the caller's SigV4 credential scope names (the only place a
    // request states one), including Regions in three different geographies, and
    // the same Region asked twice — a read must not change what a later read sees.
    let rows = [
        ("us-east-1", "first ask"),
        ("us-east-1", "asked again"),
        ("eu-west-1", "another geography"),
        ("ap-northeast-1", "a third geography"),
    ];
    for (region, case) in rows {
        let answer = list_in(&router, region, "").await;
        assert_eq!(
            answer.status, 200,
            "case {region}/{case}: expected 200, got {answer:?}"
        );
        let entries = entries_of(&answer);
        let expected: Vec<(String, String)> = EXPECTED_POLICIES
            .iter()
            .map(|name| ((*name).to_owned(), format!("{EXPECTED_ARN_PREFIX}{name}")))
            .collect();
        assert_eq!(
            entries, expected,
            "case {region}/{case}: expected the published catalogue, got {entries:?}"
        );
        assert!(
            answer.body["nextToken"].is_null(),
            "case {region}/{case}: the whole catalogue fits one page, so no nextToken; got {answer:?}"
        );
    }

    // The catalogue is not a per-account resource, so holding clusters cannot change
    // it. Rows: a cluster created in the listing's own Region, and one in another.
    let before = entries_of(&list(&router, "").await);
    for (region, name) in [(REGION, "catalogue-local"), ("eu-west-1", "catalogue-far")] {
        let body = serde_json::json!({
            "name": name,
            "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        });
        let created = call(&router, region, "POST", "/clusters", &body.to_string()).await;
        assert_eq!(
            created.status, 200,
            "setup create {name:?} in {region}: got {created:?}"
        );
        let after = entries_of(&list(&router, "").await);
        assert_eq!(
            after, before,
            "case cluster {name:?} in {region}: the catalogue must not depend on account \
             resources, got {after:?}"
        );
    }
}

/// Every ARN the listing reports is the service-owned form: it names no Region and
/// no account, and its resource part is the policy's own name.
#[tokio::test]
async fn every_access_policy_arn_is_the_service_owned_form() {
    let router = emulator();
    let answer = list(&router, "").await;
    assert_eq!(answer.status, 200, "expected 200, got {answer:?}");
    let entries = entries_of(&answer);
    assert!(
        !entries.is_empty(),
        "the catalogue must not be empty, got {answer:?}"
    );
    for (name, arn) in &entries {
        let parts: Vec<&str> = arn.splitn(6, ':').collect();
        assert_eq!(
            parts.len(),
            6,
            "case {name:?}: an ARN has six colon-separated parts, got {arn:?}"
        );
        assert_eq!(
            parts[0], "arn",
            "case {name:?}: expected the `arn` prefix, got {arn:?}"
        );
        assert_eq!(
            parts[1], "aws",
            "case {name:?}: expected the `aws` partition, got {arn:?}"
        );
        assert_eq!(
            parts[2], "eks",
            "case {name:?}: expected the `eks` service, got {arn:?}"
        );
        assert_eq!(
            parts[3], "",
            "case {name:?}: an access policy is not regional, got {arn:?}"
        );
        assert_eq!(
            parts[4], "aws",
            "case {name:?}: an access policy is owned by the service, not an account, got {arn:?}"
        );
        assert_eq!(
            parts[5],
            format!("cluster-access-policy/{name}"),
            "case {name:?}: the resource part names the policy, got {arn:?}"
        );
    }
    // A specific row, so the shape check above cannot pass on an empty-ish catalogue:
    // the `edit`-equivalent policy from the source table.
    assert!(
        entries.contains(&(
            "AmazonEKSEditPolicy".to_owned(),
            format!("{EXPECTED_ARN_PREFIX}AmazonEKSEditPolicy"),
        )),
        "the catalogue must report AmazonEKSEditPolicy with its ARN, got {entries:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: whatever page size the listing is driven at, following `nextToken` to
// exhaustion yields every published policy exactly once, in one order, and the
// last page carries no token.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn paging_at_any_size_yields_every_policy_exactly_once() {
    let router = emulator();
    let whole: Vec<String> = names_of(&list(&router, "").await);
    assert!(
        !whole.is_empty(),
        "setup: the catalogue must not be empty, got {whole:?}"
    );

    // Rows span the modelled 1..100 range of `maxResults`: both published bounds, a
    // size that divides the catalogue unevenly, one that exceeds it, and the absent
    // case (a whole page).
    let sizes: Vec<Option<usize>> = vec![
        Some(1),
        Some(2),
        Some(3),
        Some(whole.len()),
        Some(whole.len() + 1),
        Some(100),
        None,
    ];
    for size in sizes {
        let case = match size {
            Some(size) => format!("maxResults={size}"),
            None => "maxResults absent".to_string(),
        };
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        let mut pages = 0;
        loop {
            let query = match (size, &token) {
                (Some(size), None) => format!("maxResults={size}"),
                (Some(size), Some(value)) => format!("maxResults={size}&nextToken={value}"),
                (None, None) => String::new(),
                (None, Some(value)) => format!("nextToken={value}"),
            };
            let answer = list(&router, &query).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected 200, got {answer:?}"
            );
            let page = names_of(&answer);
            if let Some(size) = size {
                assert!(
                    page.len() <= size,
                    "case {case}: a page may not exceed the requested size, got {page:?}"
                );
            }
            seen.extend(page.clone());
            pages += 1;
            assert!(
                pages <= whole.len() + 1,
                "case {case}: paging did not terminate after {pages} pages"
            );
            match answer.body["nextToken"].as_str() {
                Some(next) => {
                    assert!(
                        !page.is_empty(),
                        "case {case}: a token was returned for an empty page"
                    );
                    token = Some(next.to_owned());
                }
                None => break,
            }
        }
        assert_eq!(
            seen, whole,
            "case {case}: paging to exhaustion must yield the whole catalogue once, got {seen:?}"
        );
        let unique: BTreeSet<&String> = seen.iter().collect();
        assert_eq!(
            unique.len(),
            seen.len(),
            "case {case}: no policy may be reported twice, got {seen:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a `nextToken` this listing did not mint is refused 400
// `InvalidParameterException` — never resumed from, and never answered as an
// empty page.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_foreign_next_token_is_refused() {
    let router = emulator();

    // A token another listing of the same service minted: `ListClusters` pages
    // cluster names with the same mechanism, and its token must not be honoured here.
    let cluster_body = |name: &str| {
        serde_json::json!({
            "name": name,
            "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
        })
        .to_string()
    };
    for name in ["token-donor-a", "token-donor-b"] {
        let created = call(&router, REGION, "POST", "/clusters", &cluster_body(name)).await;
        assert_eq!(created.status, 200, "setup create {name:?}: got {created:?}");
    }
    let clusters_page = call(&router, REGION, "GET", "/clusters?maxResults=1", "").await;
    let cluster_token = clusters_page.body["nextToken"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    assert!(
        !cluster_token.is_empty(),
        "setup: ListClusters must offer a token to borrow, got {clusters_page:?}"
    );

    // A well-formed hex encoding of a catalogue entry carrying no provenance tag.
    let untagged: String = "AmazonEKSAdminPolicy"
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();

    // Rows differ along the shape of the token: not hex at all, too short to carry a
    // tag, hex of an odd length, a tag with no name, a plausible-looking hex string,
    // another listing's token, and an untagged encoding of a real entry.
    let cases: Vec<(&str, String)> = vec![
        ("not hex", "not-hex".to_string()),
        ("too short", "zz".to_string()),
        ("odd length", "abc".to_string()),
        ("tag only", "ffffffff".to_string()),
        ("plausible hex", "ffffffffff".to_string()),
        ("another listing's token", cluster_token),
        ("untagged entry", untagged),
    ];
    for (case, token) in cases {
        let answer = list(&router, &format!("nextToken={token}")).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?} ({token:?}): expected 400, got {answer:?}"
        );
        assert_eq!(
            answer.error_type.as_deref(),
            Some("InvalidParameterException"),
            "case {case:?} ({token:?}): expected InvalidParameterException, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: an ARN the listing reports is an ARN `AssociateAccessPolicy` accepts — a
// client that DISCOVERS the policy set from the API, instead of hardcoding it,
// can associate every policy it discovered.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_discovered_policy_arn_can_be_associated() {
    let router = emulator();

    // A cluster with one access entry to associate the discovered policies to.
    let cluster = "discovered-policies";
    let principal = "arn:aws:iam::000000000000:role/discovered-policies-role";
    let created = call(
        &router,
        REGION,
        "POST",
        "/clusters",
        &serde_json::json!({
            "name": cluster,
            "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
            "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
            // The access-entry API is served only by a cluster whose
            // `authenticationMode` includes it.
            "accessConfig": { "authenticationMode": "API_AND_CONFIG_MAP" },
        })
        .to_string(),
    )
    .await;
    assert_eq!(created.status, 200, "setup create cluster: got {created:?}");
    let entry = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{cluster}/access-entries"),
        &serde_json::json!({ "principalArn": principal }).to_string(),
    )
    .await;
    assert_eq!(entry.status, 200, "setup create access entry: got {entry:?}");

    // Rows: every entry the listing reported, paged through to exhaustion — the
    // ARNs come from the API, not from a constant in this file.
    let discovered = entries_of(&list(&router, "").await);
    assert_eq!(
        discovered.len(),
        EXPECTED_POLICIES.len(),
        "setup: expected the whole catalogue in one page, got {discovered:?}"
    );
    for (name, arn) in &discovered {
        let associated = call(
            &router,
            REGION,
            "POST",
            &format!("/clusters/{cluster}/access-entries/{}/access-policies", encode(principal)),
            &serde_json::json!({ "policyArn": arn, "accessScope": { "type": "cluster" } })
                .to_string(),
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {name:?} ({arn:?}): a discovered policy ARN must associate, got {associated:?}"
        );
        assert_eq!(
            associated.body["associatedAccessPolicy"]["policyArn"].as_str(),
            Some(arn.as_str()),
            "case {name:?}: the association must report the ARN it was given, got {associated:?}"
        );
    }

    // And the entry now holds one record per discovered policy, read back through
    // the listing that reports them.
    let listed = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{cluster}/access-entries/{}/access-policies?maxResults=100",
            encode(principal)
        ),
        "",
    )
    .await;
    let held = listed.body["associatedAccessPolicies"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item["policyArn"].as_str().map(str::to_owned))
                .collect::<BTreeSet<String>>()
        })
        .unwrap_or_default();
    let expected: BTreeSet<String> = discovered.iter().map(|(_, arn)| arn.clone()).collect();
    assert_eq!(
        held, expected,
        "case read back: the entry must hold every discovered policy exactly once, got {held:?}"
    );
}

/// Percent-encode a value travelling in ONE path segment, as an AWS SDK encodes a
/// non-greedy `@httpLabel`: the crate's one copy of that rule lives in the wire
/// harness.
fn encode(value: &str) -> String {
    crate::wire::encode_label(value)
}

#[tokio::test]
async fn max_results_is_held_to_its_modelled_range() {
    let router = emulator();
    // Source: `ListAccessPoliciesRequestMaxResults`'s range from
    // `smithy_query query="operation" operation="ListAccessPolicies"` — min 1,
    // max 100. Rows: each bound and one past it on each side.
    let cases: [(&str, u16); 4] = [("0", 400), ("1", 200), ("100", 200), ("101", 400)];
    for (value, expected) in cases {
        let answer = list(&router, &format!("maxResults={value}")).await;
        assert_eq!(
            answer.status, expected,
            "case maxResults={value}: expected {expected}, got {answer:?}"
        );
    }
}
