// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the two read-only DISCOVERY catalogs this service
//! publishes — `ListAccessPolicies` and `DescribeClusterVersions` — driven over
//! the wire through the same `MultiServiceRouter` the production binary mounts.
//!
//! The RULE under test is the one both operations share: a catalog Amazon EKS
//! publishes is answered WHOLE and the same to every caller, each entry carrying
//! every member the model declares for it, and each entry is usable by the
//! operation that consumes it — a policy a client discovers is a `policyArn`
//! `AssociateAccessPolicy` accepts, and a version row a client discovers carries
//! the capability data (tiers, component configuration, patch level, support
//! window) a client sizes and schedules a cluster from.
//!
//! So each suite asserts over a TABLE: the access-policy rows differ along the
//! family of the policy (the Kubernetes cluster-role mirrors, the Amazon EKS
//! controllers, the cluster capabilities, service-account impersonation and the
//! Amazon Web Services services that orchestrate workloads ON a cluster) and
//! along the page size the catalog is discovered at; the version rows are every
//! row of the published calendar. Every assertion names its case and prints the
//! value it actually got.

use std::collections::BTreeMap;

use smithy_server_runtime::MultiServiceRouter;

use crate::wire::{call, emulator, encode_label, Answer};

const REGION: &str = "eu-central-1";

/// One policy of each FAMILY the published catalogue carries, with the family it
/// belongs to as its case name.
///
/// Source: the Amazon EKS "Access policy permissions" reference's table of
/// Amazon EKS access policies — the same source `state::ACCESS_POLICY_NAMES`
/// cites. These rows are a sample of that table spanning its families, including
/// families no issue report has cited (the Kubernetes `edit` mirror, the AWS
/// Controllers for Kubernetes, Argo CD); the whole catalogue is asserted entry by
/// entry by the loop below, which reads it from the API rather than from a list
/// written here.
const POLICY_FAMILIES: &[(&str, &str)] = &[
    ("the Kubernetes cluster-admin mirror", "AmazonEKSClusterAdminPolicy"),
    ("the Kubernetes edit mirror", "AmazonEKSEditPolicy"),
    ("an Amazon EKS Auto Mode controller", "AmazonEKSComputePolicy"),
    ("the cluster-scoped half of that controller", "AmazonEKSComputeClusterPolicy"),
    ("a cluster capability", "AmazonEKSPodIdentityPolicy"),
    ("the AWS Controllers for Kubernetes", "AmazonEKSACKPolicy"),
    ("Argo CD", "AmazonEKSArgoCDPolicy"),
    ("service-account impersonation", "AmazonEKSServiceAccountImpersonationPolicy"),
    ("Amazon EMR on EKS, cluster-scoped", "AmazonEMRClusterPolicy"),
    ("Amazon EMR on EKS, job-scoped", "AmazonEMRJobPolicy"),
    ("SageMaker HyperPod, cluster-scoped", "AmazonSagemakerHyperpodClusterPolicy"),
    ("SageMaker HyperPod, its controller", "AmazonSagemakerHyperpodControllerPolicy"),
    ("SageMaker HyperPod, inference", "AmazonSagemakerHyperpodInferencePolicy"),
    ("SageMaker HyperPod, a space", "AmazonSagemakerHyperpodSpacePolicy"),
    ("SageMaker HyperPod, a space template", "AmazonSagemakerHyperpodSpaceTemplatePolicy"),
];

/// The ARN form an access policy carries: partition `aws`, service `eks`, no
/// Region and the literal `aws` where an account-owned resource carries an
/// account id — the service owns these, not the caller.
fn policy_arn(name: &str) -> String {
    format!("arn:aws:eks::aws:cluster-access-policy/{name}")
}

/// Every `(arn, name)` the access-policy catalog publishes, discovered the way a
/// client discovers it: page through `ListAccessPolicies` to exhaustion at the
/// given page size (`None` for the whole page the operation gives by default).
async fn discover_policies(
    router: &MultiServiceRouter,
    page_size: Option<usize>,
) -> BTreeMap<String, String> {
    let case = match page_size {
        Some(size) => format!("maxResults={size}"),
        None => "maxResults absent".to_string(),
    };
    let mut discovered: BTreeMap<String, String> = BTreeMap::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let query = match (page_size, &token) {
            (Some(size), None) => format!("maxResults={size}"),
            (Some(size), Some(value)) => format!("maxResults={size}&nextToken={value}"),
            (None, None) => String::new(),
            (None, Some(value)) => format!("nextToken={value}"),
        };
        let uri = if query.is_empty() {
            "/access-policies".to_string()
        } else {
            format!("/access-policies?{query}")
        };
        let answer = call(router, REGION, "GET", &uri, "").await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200 from the catalog, got {} ({:?})",
            answer.status, answer.body
        );
        let page = answer.body["accessPolicies"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an accessPolicies list, got {:?}", answer.body)
            })
            .clone();
        for entry in &page {
            let name = entry["name"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a name, got {entry:?}"))
                .to_string();
            let arn = entry["arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case}: expected an arn, got {entry:?}"))
                .to_string();
            assert!(
                discovered.insert(arn.clone(), name.clone()).is_none(),
                "case {case}: {arn:?} was published twice"
            );
        }
        pages += 1;
        assert!(
            pages <= 200,
            "case {case}: paging did not terminate after {pages} pages"
        );
        match answer.body["nextToken"].as_str() {
            Some(next) => token = Some(next.to_string()),
            None => break,
        }
    }
    discovered
}

// ---------------------------------------------------------------------------
// RULE: the access-policy catalog publishes every policy family Amazon EKS
// manages — including the policies published for the Amazon Web Services
// services that orchestrate workloads on a cluster — each with the ARN its name
// determines, and a client finds every one of them by looking the ARN up in the
// listing, at any page size.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn every_published_policy_family_is_discoverable_by_arn() {
    let router = emulator();

    // Rows: the page sizes a client discovers the catalog at — one entry at a
    // time, a size that divides it unevenly, the modelled maximum, and the
    // absent case — crossed with one policy of every family.
    for page_size in [Some(1), Some(7), Some(100), None] {
        let case = match page_size {
            Some(size) => format!("maxResults={size}"),
            None => "maxResults absent".to_string(),
        };
        let discovered = discover_policies(&router, page_size).await;
        for (family, name) in POLICY_FAMILIES {
            let arn = policy_arn(name);
            let found = discovered.get(&arn);
            assert_eq!(
                found.map(String::as_str),
                Some(*name),
                "case {case}/{family}: the catalog must publish {name:?} at {arn:?}, got \
                 {found:?} among {:?}",
                discovered.values().collect::<Vec<_>>()
            );
        }
        // The catalog does not depend on the page size it is read at.
        let whole = discover_policies(&router, None).await;
        assert_eq!(
            discovered, whole,
            "case {case}: paging must yield the same catalog as one whole page, got {discovered:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: a policy ARN the catalog publishes is a `policyArn`
// `AssociateAccessPolicy` accepts and reports back — so a client that
// DISCOVERS a policy, instead of hardcoding its ARN, can use what it found.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_discovered_policy_of_every_family_can_be_associated() {
    let router = emulator();
    let cluster = "catalog-discovery";
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
    assert_eq!(
        created.status, 200,
        "setup: expected the cluster to be created, got {} ({:?})",
        created.status, created.body
    );
    let principal = "arn:aws:iam::000000000000:role/catalog-discovery-role";
    let entry = call(
        &router,
        REGION,
        "POST",
        &format!("/clusters/{cluster}/access-entries"),
        &serde_json::json!({ "principalArn": principal }).to_string(),
    )
    .await;
    assert_eq!(
        entry.status, 200,
        "setup: expected the access entry to be created, got {} ({:?})",
        entry.status, entry.body
    );

    // Rows: one policy of every family, with its ARN taken from the LISTING
    // rather than from this file.
    let discovered = discover_policies(&router, None).await;
    for (family, name) in POLICY_FAMILIES {
        let arn = policy_arn(name);
        assert!(
            discovered.contains_key(&arn),
            "case {family}: {name:?} must be discoverable before it is associated, got {:?}",
            discovered.values().collect::<Vec<_>>()
        );
        let associated = call(
            &router,
            REGION,
            "POST",
            &format!(
                "/clusters/{cluster}/access-entries/{}/access-policies",
                encode_label(principal)
            ),
            &serde_json::json!({ "policyArn": arn, "accessScope": { "type": "cluster" } })
                .to_string(),
        )
        .await;
        assert_eq!(
            associated.status, 200,
            "case {family} ({name}): a discovered ARN must associate, got {} ({:?})",
            associated.status, associated.body
        );
        assert_eq!(
            associated.body["associatedAccessPolicy"]["policyArn"].as_str(),
            Some(arn.as_str()),
            "case {family} ({name}): the association must report the ARN it was given, got {:?}",
            associated.body["associatedAccessPolicy"]
        );
    }

    // And every association reads back through the listing that reports them.
    let listed = call(
        &router,
        REGION,
        "GET",
        &format!(
            "/clusters/{cluster}/access-entries/{}/access-policies?maxResults=100",
            encode_label(principal)
        ),
        "",
    )
    .await;
    assert_eq!(
        listed.status, 200,
        "expected HTTP 200 reading the associations back, got {} ({:?})",
        listed.status, listed.body
    );
    let held: Vec<String> = listed.body["associatedAccessPolicies"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "expected an associatedAccessPolicies list, got {:?}",
                listed.body
            )
        })
        .iter()
        .filter_map(|item| item["policyArn"].as_str().map(str::to_owned))
        .collect();
    for (family, name) in POLICY_FAMILIES {
        let arn = policy_arn(name);
        assert!(
            held.contains(&arn),
            "case {family} ({name}): the entry must hold the policy it was given, got {held:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: every row the version catalog publishes is MEMBER-COMPLETE — it carries
// every member `ClusterVersionInformation` declares, including the nested
// control-plane scaling tiers with their three capability numbers and their
// component-configuration overrides, so a client reading the catalog never sees
// a 200 with capability data missing behind it.
// ---------------------------------------------------------------------------

/// The `VersionStatus` members, from the model:
/// `smithy_query(query="shape", shape="VersionStatus")` gives
/// `UNSUPPORTED | STANDARD_SUPPORT | EXTENDED_SUPPORT`.
const VERSION_STATUS: &[&str] = &["UNSUPPORTED", "STANDARD_SUPPORT", "EXTENDED_SUPPORT"];

/// The three component groups a published control-plane configuration carries,
/// from `smithy_query(query="shape", shape="ControlPlaneConfigInfo")`.
const COMPONENT_GROUPS: &[&str] = &[
    "kubeApiServerConfig",
    "kubeSchedulerConfig",
    "kubeControllerManagerConfig",
];

/// The `clusterVersions` list of an answer.
fn version_rows(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "expected a clusterVersions list, got {:?}",
                answer.body
            )
        })
        .clone()
}

#[tokio::test]
async fn every_published_version_row_is_member_complete() {
    let router = emulator();

    // Rows: the listing as a client first calls it (no filters at all), and the
    // widened listing that also publishes the versions Amazon EKS no longer
    // offers — the rule is about every row either one reports.
    for query in ["", "includeAll=true"] {
        let uri = if query.is_empty() {
            "/cluster-versions".to_string()
        } else {
            format!("/cluster-versions?{query}")
        };
        let answer = call(&router, REGION, "GET", &uri, "").await;
        assert_eq!(
            answer.status, 200,
            "case {query:?}: expected HTTP 200, got {} ({:?})",
            answer.status, answer.body
        );
        let rows = version_rows(&answer);
        assert!(
            !rows.is_empty(),
            "case {query:?}: the catalog must publish its version rows, got {:?}",
            answer.body
        );
        for row in &rows {
            let version = row["clusterVersion"]
                .as_str()
                .unwrap_or_else(|| panic!("case {query:?}: expected a clusterVersion, got {row:?}"))
                .to_string();
            let case = format!("{query:?}/{version}");
            assert!(
                !row["clusterType"].as_str().unwrap_or_default().is_empty(),
                "case {case}: expected a clusterType, got {row:?}"
            );
            let platform = row["defaultPlatformVersion"].as_str().unwrap_or_default();
            assert!(
                platform.starts_with("eks."),
                "case {case}: expected an eks.<revision> platform version, got {platform:?}"
            );
            assert!(
                row["defaultVersion"].is_boolean(),
                "case {case}: expected a defaultVersion boolean, got {row:?}"
            );
            let release = row["releaseDate"].as_f64().unwrap_or_default();
            let standard_end = row["endOfStandardSupportDate"].as_f64().unwrap_or_default();
            let extended_end = row["endOfExtendedSupportDate"].as_f64().unwrap_or_default();
            assert!(
                release > 0.0 && standard_end > release && extended_end > standard_end,
                "case {case}: expected a release date followed by its two support ends, got \
                 {release}, {standard_end}, {extended_end}"
            );
            let status = row["status"].as_str().unwrap_or_default();
            let version_status = row["versionStatus"].as_str().unwrap_or_default();
            assert!(
                VERSION_STATUS.contains(&version_status),
                "case {case}: expected one of {VERSION_STATUS:?}, got {version_status:?}"
            );
            assert_eq!(
                status, version_status,
                "case {case}: the deprecated member reports the same state as its replacement, \
                 got {status:?}"
            );
            let patch = row["kubernetesPatchVersion"].as_str().unwrap_or_default();
            assert!(
                patch.starts_with(&format!("{version}.")),
                "case {case}: expected a patch version of this minor version, got {patch:?}"
            );

            // The nested capability sub-tree: the tiers, their three capability
            // numbers, and the component configuration published for the version
            // and for each tier.
            let tiers = row["controlPlaneScalingTiers"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {case}: expected a controlPlaneScalingTiers list, got {row:?}")
                })
                .clone();
            // `ControlPlaneScalingTierList` is modelled @length 0..10.
            assert!(
                !tiers.is_empty() && tiers.len() <= 10,
                "case {case}: expected between one and ten published tiers, got {} ({tiers:?})",
                tiers.len()
            );
            for tier in &tiers {
                let tier_name = tier["tierName"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: expected a tierName, got {tier:?}"))
                    .to_string();
                for member in [
                    "apiRequestConcurrency",
                    "podSchedulingRatePerSecond",
                    "clusterDatabaseSizeGb",
                ] {
                    let value = tier[member].as_i64().unwrap_or_default();
                    assert!(
                        value > 0,
                        "case {case}/{tier_name}: expected a positive {member}, got {:?}",
                        tier[member]
                    );
                }
                for group in COMPONENT_GROUPS {
                    assert!(
                        tier["controlPlaneComponentConfigOverrides"][group].is_object(),
                        "case {case}/{tier_name}: expected {group} in the tier's component \
                         configuration, got {:?}",
                        tier["controlPlaneComponentConfigOverrides"]
                    );
                }
            }
            for group in COMPONENT_GROUPS {
                assert!(
                    row["controlPlaneComponentConfig"][group].is_object(),
                    "case {case}: expected {group} in the version's component configuration, got \
                     {:?}",
                    row["controlPlaneComponentConfig"]
                );
            }
        }
    }
}
