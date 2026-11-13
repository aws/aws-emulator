// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PER-ADD-ON metadata the add-on catalog publishes,
//! driven over the wire through the same `MultiServiceRouter` the production
//! binary mounts.
//!
//! The RULE under test is one rule in four members: every member of an `AddonInfo`
//! and of the document `DescribeAddonConfiguration` publishes is a property of
//! THAT add-on, read from its own catalog record — so two unrelated add-ons
//! differ wherever their records differ, and the server-side filters that read
//! those members (`owners`, `kubernetesVersion`) really narrow the catalog:
//!
//! * `AddonInfo$owner` is the add-on's own owner, so the catalog carries more
//!   than one owner token and `owners` separates the add-ons Amazon Web Services
//!   builds from the ones it curates or a vendor sells;
//! * `AddonVersionInfo$addonVersion` walks the add-on's OWN published release
//!   line, so a history that crossed a major version lists every major it
//!   released;
//! * `AddonVersionInfo$compatibilities` is the add-on's own compatible
//!   cluster-version set, bounded by the Kubernetes version Amazon EKS first
//!   offered that add-on on, so a version real for one add-on can be outside
//!   another's and empty that add-on's filtered read;
//! * `configurationSchema` is the add-on's own JSON Schema document, so the
//!   dialect it declares is the dialect that add-on published.
//!
//! Each suite asserts the rule over a TABLE whose rows differ along what the rule
//! generalises over (which add-on, which owner kind, how many majors its line
//! spans, which dialect its document declares, which pair of add-ons is compared,
//! which build is read), and over an invariant across the WHOLE published catalog
//! so the rule is not asserted of the cited add-ons alone. Every assertion names
//! its case and prints the actual value.
//!
//! The request harness is the crate's own (`wire.rs`) and the catalog readers are
//! the ones `addon_versions.rs` owns.

use std::collections::BTreeSet;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_OWNER_AWS, ADDON_OWNER_COMMUNITY, ADDON_OWNER_MARKETPLACE, ADDON_SCHEMA_DIALECT_2019_09,
    ADDON_SCHEMA_DIALECT_DRAFT_06, ADDON_SCHEMA_DIALECT_DRAFT_07, DEFAULT_KUBERNETES_VERSION,
};

use crate::addon_catalog_coverage::{published_addon, supported_cluster_versions};
use crate::addon_catalog_publication::configuration_schema;
use crate::addon_lifecycle::{create_addon, describe_addon};
use crate::addon_versions::{
    active_cluster, addon_names, addons, describe_addon_configuration, describe_addon_versions,
    string_member, version_key, versions_of, whole_catalog,
};
use crate::wire::emulator;

/// The `addonVersion` of every build one reported add-on publishes, newest first.
fn published_versions(addon: &serde_json::Value) -> Vec<String> {
    versions_of(addon)
        .iter()
        .map(|build| string_member(build, "addonVersion"))
        .collect()
}

/// The MAJOR component of a `vX.Y.Z-eksbuild.N` add-on version.
fn major_of(version: &str) -> u32 {
    version_key(version).0
}

// ---------------------------------------------------------------------------
// RULE: `AddonInfo$owner` is the owner of THAT add-on — Amazon Web Services for
// the ones it builds, the community for the upstream projects Amazon EKS only
// curates, AWS Marketplace for the ones a caller subscribes to — so the catalog
// carries several owner tokens and `owners` narrows it.
// ---------------------------------------------------------------------------

/// The owner the catalog publishes for one add-on.
///
/// SOURCE: the Amazon EKS User Guide's add-on sections — the "Amazon EKS add-ons"
/// Amazon Web Services builds, the "community add-ons" it curates from upstream
/// projects, and the AWS Marketplace add-ons a caller subscribes to. The rows span
/// all three kinds and include add-ons the catalog's own suites do not otherwise
/// name.
const EXPECTED_OWNERS: &[(&str, &str)] = &[
    ("vpc-cni", ADDON_OWNER_AWS),
    ("coredns", ADDON_OWNER_AWS),
    ("aws-efs-csi-driver", ADDON_OWNER_AWS),
    ("amazon-sagemaker-hyperpod-training-operator", ADDON_OWNER_AWS),
    ("cert-manager", ADDON_OWNER_COMMUNITY),
    ("metrics-server", ADDON_OWNER_COMMUNITY),
    ("prometheus-node-exporter", ADDON_OWNER_COMMUNITY),
    ("kubecost_kubecost", ADDON_OWNER_MARKETPLACE),
    ("datadog_operator", ADDON_OWNER_MARKETPLACE),
];

#[tokio::test]
async fn each_addon_is_published_with_its_own_owner() {
    let router = emulator();

    for (addon_name, expected) in EXPECTED_OWNERS {
        let addon = published_addon(&router, addon_name, None).await;
        assert_eq!(
            string_member(&addon, "owner"),
            *expected,
            "case {addon_name:?}: expected the add-on's own owner {expected:?}, got {:?}",
            addon["owner"]
        );
    }

    // The members a client discriminates add-ons by are the add-on's own, so two
    // add-ons of different kinds do not report the same owner. The pairs span the
    // three kinds.
    for (left, right) in [
        ("cert-manager", "amazon-sagemaker-hyperpod-training-operator"),
        ("cert-manager", "kubecost_kubecost"),
        ("vpc-cni", "datadog_operator"),
    ] {
        let left_owner = string_member(&published_addon(&router, left, None).await, "owner");
        let right_owner = string_member(&published_addon(&router, right, None).await, "owner");
        assert!(
            !left_owner.is_empty() && left_owner != right_owner,
            "case {left:?} against {right:?}: two add-ons of different kinds report distinct \
             owners, got {left_owner:?} and {right_owner:?}"
        );
    }
}

#[tokio::test]
async fn the_owners_filter_narrows_the_catalog_to_that_owner() {
    let router = emulator();
    let catalog = addons(&whole_catalog(&router).await);
    let every_name: BTreeSet<String> = catalog
        .iter()
        .map(|addon| string_member(addon, "addonName"))
        .collect();

    let owners: BTreeSet<String> = catalog
        .iter()
        .map(|addon| string_member(addon, "owner"))
        .collect();
    assert!(
        owners.len() > 1,
        "case the published owners: a catalog whose add-ons carry one owner cannot be narrowed \
         by owner at all, got {owners:?}"
    );

    // Every owner the catalog publishes is a row, plus one nothing is owned by:
    // each read is exactly the add-ons carrying that owner, a strict subset of the
    // catalog, and the reads partition it.
    let mut union: BTreeSet<String> = BTreeSet::new();
    for owner in owners.iter().chain(std::iter::once(&"not-an-owner".to_string())) {
        let expected: BTreeSet<String> = catalog
            .iter()
            .filter(|addon| string_member(addon, "owner") == *owner)
            .map(|addon| string_member(addon, "addonName"))
            .collect();
        let answer = describe_addon_versions(&router, &format!("owners={owner}")).await;
        assert_eq!(
            answer.status, 200,
            "case owners={owner:?}: expected HTTP 200, got {answer:?}"
        );
        let reported: BTreeSet<String> = addon_names(&answer).into_iter().collect();
        assert_eq!(
            reported, expected,
            "case owners={owner:?}: expected exactly the add-ons that owner owns, got {reported:?}"
        );
        if owners.contains(owner) {
            assert!(
                !reported.is_empty() && reported.len() < every_name.len(),
                "case owners={owner:?}: an owner the catalog publishes narrows it to a strict \
                 subset, got {reported:?} of {} add-ons",
                every_name.len()
            );
        }
        union.extend(reported);
    }
    assert_eq!(
        union, every_name,
        "case every published owner: the owner-filtered reads cover the whole catalog, got \
         {union:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: the versions an add-on publishes are its OWN release line — unique,
// well formed and newest first — so an add-on whose project released several
// major versions has all of them in its published history.
// ---------------------------------------------------------------------------

/// The fewest DISTINCT major versions an add-on's published history spans.
///
/// SOURCE: the releases each project published over the Kubernetes versions this
/// service's calendar carries — the Amazon EFS CSI driver's majors 1, 2 and 3, the
/// Mountpoint for Amazon S3 CSI driver's major 2 beside its major 1, and the
/// Amazon CloudWatch Observability agent's successive majors. The last rows are
/// add-ons whose line stayed inside ONE major over the same window, so the rule is
/// "the add-on's own history", not "every add-on has many majors".
const EXPECTED_MAJOR_SPAN: &[(&str, usize)] = &[
    ("aws-efs-csi-driver", 3),
    ("aws-mountpoint-s3-csi-driver", 2),
    ("amazon-cloudwatch-observability", 3),
    ("cert-manager", 1),
    ("coredns", 1),
];

#[tokio::test]
async fn an_addons_published_history_is_its_own_release_line() {
    let router = emulator();

    for (addon_name, expected_majors) in EXPECTED_MAJOR_SPAN {
        let addon = published_addon(&router, addon_name, None).await;
        let versions = published_versions(&addon);
        let majors: BTreeSet<u32> = versions.iter().map(|version| major_of(version)).collect();
        assert!(
            majors.len() >= *expected_majors,
            "case {addon_name:?}: expected a published history spanning at least \
             {expected_majors} major version(s), got {majors:?} from {versions:?}"
        );
    }

    // And across the WHOLE catalog the history is well formed: every add-on
    // publishes at least one build, no version twice, every version in the
    // `vX.Y.Z-eksbuild.N` form, newest first.
    for addon in addons(&whole_catalog(&router).await) {
        let case = string_member(&addon, "addonName");
        let versions = published_versions(&addon);
        assert!(
            !versions.is_empty(),
            "case {case:?}: a published add-on has a release line, got {versions:?}"
        );
        let distinct: BTreeSet<&String> = versions.iter().collect();
        assert_eq!(
            distinct.len(),
            versions.len(),
            "case {case:?}: an add-on publishes each version once, got {versions:?}"
        );
        let keys: Vec<(u32, u32, u32, u32)> =
            versions.iter().map(|version| version_key(version)).collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] > pair[1]),
            "case {case:?}: the published history is newest first, got {versions:?}"
        );
        assert!(
            keys.first().map(|key| key.0) >= keys.last().map(|key| key.0),
            "case {case:?}: the newest build is of the newest major the add-on released, got \
             {versions:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the compatible cluster-version set is the ADD-ON's own, bounded by the
// Kubernetes version Amazon EKS first offered it on — so a cluster version real
// for one add-on can be outside another's set, and it empties that add-on's
// `kubernetesVersion`-filtered read while keeping the first's.
// ---------------------------------------------------------------------------

/// Pairs of add-ons Amazon EKS began offering at DIFFERENT Kubernetes versions:
/// the long-standing add-on first, the one introduced later second.
///
/// SOURCE: the Kubernetes versions the Amazon EKS User Guide's add-on
/// compatibility tables list for each add-on. The rows differ in the kind of
/// add-on on each side (a community one against a machine-learning operator, a
/// networking one against a monitoring agent) so the rule is not asserted of one
/// recorded pair.
const EXPECTED_DISTINCT_COMPATIBILITIES: &[(&str, &str)] = &[
    ("cert-manager", "amazon-sagemaker-hyperpod-training-operator"),
    ("vpc-cni", "amazon-sagemaker-hyperpod-taskgovernance"),
    ("coredns", "eks-node-monitoring-agent"),
    ("kube-proxy", "aws-network-flow-monitoring-agent"),
];

#[tokio::test]
async fn compatibilities_are_the_addons_own_cluster_versions() {
    let router = emulator();

    for (longstanding, introduced_later) in EXPECTED_DISTINCT_COMPATIBILITIES {
        let case = format!("{longstanding} against {introduced_later}");
        let left = supported_cluster_versions(&published_addon(&router, longstanding, None).await);
        let right =
            supported_cluster_versions(&published_addon(&router, introduced_later, None).await);
        assert!(
            !left.is_empty() && !right.is_empty(),
            "case {case}: both add-ons support a cluster version, got {left:?} and {right:?}"
        );
        assert_ne!(
            left, right,
            "case {case}: two add-ons offered from different Kubernetes versions name different \
             compatible sets, got {left:?} for both"
        );

        // A cluster version real for the long-standing add-on and outside the
        // newer one's set empties the newer add-on's filtered read, and keeps the
        // other's: that is what makes `kubernetesVersion` answer "can I install
        // this add-on on this cluster".
        let outside = left
            .difference(&right)
            .next()
            .unwrap_or_else(|| panic!("case {case}: expected a version only {longstanding} supports, got {left:?} and {right:?}"));
        for (addon_name, expected) in [(longstanding, true), (introduced_later, false)] {
            let answer = describe_addon_versions(
                &router,
                &format!("addonName={addon_name}&kubernetesVersion={outside}"),
            )
            .await;
            assert_eq!(
                answer.status, 200,
                "case {case}/{addon_name} on {outside}: expected HTTP 200, got {answer:?}"
            );
            assert_eq!(
                !addons(&answer).is_empty(),
                expected,
                "case {case}/{addon_name} on {outside}: expected the add-on to be {}, got {:?}",
                if expected { "offered" } else { "absent" },
                answer.body["addons"]
            );
        }
    }

    // Across the WHOLE catalog: the compatible sets are not one set shared by
    // every add-on, and each add-on's set is a non-empty set of versions the
    // calendar publishes.
    let catalog = addons(&whole_catalog(&router).await);
    let mut sets: BTreeSet<Vec<String>> = BTreeSet::new();
    for addon in &catalog {
        let case = string_member(addon, "addonName");
        let supported = supported_cluster_versions(addon);
        assert!(
            !supported.is_empty(),
            "case {case:?}: a published add-on supports a cluster version, got {supported:?}"
        );
        sets.insert(supported.into_iter().collect());
    }
    assert!(
        sets.len() > 1,
        "case the published compatibilities: a catalog in which every add-on names the same \
         cluster-version set cannot answer which versions an add-on runs on, got {sets:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE: `configurationSchema` is the add-on's OWN published JSON Schema
// document, so the `$schema` dialect it declares is that add-on's dialect rather
// than one fixed for the service.
// ---------------------------------------------------------------------------

/// The JSON Schema dialect each add-on's published configuration document
/// declares.
///
/// SOURCE: the `$schema` keyword of the document `aws eks
/// describe-addon-configuration` reports for the add-on. The rows carry every
/// dialect the catalog publishes — the charts Amazon EKS has published schemas
/// for the longest declare draft-06, the newer generation declares draft-07, and
/// the documents it has reissued declare the 2019-09 dialect — and more than one
/// add-on of the ones it publishes more than one document in.
const EXPECTED_DIALECTS: &[(&str, &str)] = &[
    (
        "amazon-sagemaker-hyperpod-training-operator",
        ADDON_SCHEMA_DIALECT_DRAFT_07,
    ),
    (
        "amazon-sagemaker-hyperpod-taskgovernance",
        ADDON_SCHEMA_DIALECT_DRAFT_07,
    ),
    ("cert-manager", ADDON_SCHEMA_DIALECT_DRAFT_07),
    ("metrics-server", ADDON_SCHEMA_DIALECT_DRAFT_07),
    ("vpc-cni", ADDON_SCHEMA_DIALECT_DRAFT_06),
    ("coredns", ADDON_SCHEMA_DIALECT_DRAFT_06),
    ("aws-efs-csi-driver", ADDON_SCHEMA_DIALECT_2019_09),
];

#[tokio::test]
async fn the_configuration_schema_declares_the_addons_own_dialect() {
    let router = emulator();

    for (addon_name, expected) in EXPECTED_DIALECTS {
        let addon = published_addon(&router, addon_name, None).await;
        let versions = published_versions(&addon);
        // Both ends of the add-on's published history: the dialect belongs to the
        // add-on's document, so every build of it declares the same one.
        let builds = [
            versions.first().expect("a published build").clone(),
            versions.last().expect("a published build").clone(),
        ];
        for addon_version in builds {
            let case = format!("{addon_name}/{addon_version}");
            let schema = configuration_schema(&router, addon_name, &addon_version).await;
            assert_eq!(
                schema["$schema"].as_str(),
                Some(*expected),
                "case {case}: expected the dialect the add-on's own schema declares \
                 {expected:?}, got {:?}",
                schema["$schema"]
            );
        }
    }

    // The dialect is per add-on, so the catalog publishes more than one of them.
    let catalog = addons(&whole_catalog(&router).await);
    let mut dialects: BTreeSet<String> = BTreeSet::new();
    for addon in &catalog {
        let addon_name = string_member(addon, "addonName");
        let addon_version = string_member(&versions_of(addon)[0], "addonVersion");
        let schema = configuration_schema(&router, &addon_name, &addon_version).await;
        let dialect = schema["$schema"]
            .as_str()
            .unwrap_or_else(|| {
                panic!("case {addon_name:?}: a published schema names its dialect, got {schema:?}")
            })
            .to_string();
        assert!(
            [
                ADDON_SCHEMA_DIALECT_DRAFT_06,
                ADDON_SCHEMA_DIALECT_DRAFT_07,
                ADDON_SCHEMA_DIALECT_2019_09,
            ]
            .contains(&dialect.as_str()),
            "case {addon_name:?}: expected one of the JSON Schema dialects the catalog publishes \
             its documents in, got {dialect:?}"
        );
        dialects.insert(dialect);
    }
    assert!(
        dialects.len() > 1,
        "case the published dialects: a service that fixes one dialect for every add-on does not \
         publish the add-ons' own documents, got {dialects:?}"
    );
}

#[tokio::test]
async fn the_operator_publishes_its_pod_identity_policy_and_cert_manager_none() {
    let router = emulator();

    // The add-on that needs IAM permissions of its own names the service account
    // it runs as with the managed policy Amazon Web Services recommends for it;
    // the one that calls no AWS API reports an EMPTY list, not a missing one.
    for (addon_name, expected_policy) in [
        (
            "amazon-sagemaker-hyperpod-training-operator",
            Some("arn:aws:iam::aws:policy/AmazonSageMakerHyperPodTrainingOperatorAccess"),
        ),
        ("cert-manager", None),
    ] {
        let addon = published_addon(&router, addon_name, None).await;
        let addon_version = string_member(&versions_of(&addon)[0], "addonVersion");
        let answer = describe_addon_configuration(&router, addon_name, &addon_version).await;
        assert_eq!(
            answer.status, 200,
            "case {addon_name:?}: expected HTTP 200, got {answer:?}"
        );
        let entries = answer.body["podIdentityConfiguration"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {addon_name:?}: expected a podIdentityConfiguration list, got {:?}",
                    answer.body
                )
            })
            .clone();
        match expected_policy {
            None => assert!(
                entries.is_empty(),
                "case {addon_name:?}: an add-on that calls no AWS API of its own recommends \
                 nothing, got {entries:?}"
            ),
            Some(policy) => {
                let recommended: Vec<String> = entries
                    .iter()
                    .flat_map(|entry| {
                        entry["recommendedManagedPolicies"]
                            .as_array()
                            .cloned()
                            .unwrap_or_default()
                    })
                    .map(|value| value.as_str().unwrap_or_default().to_string())
                    .collect();
                assert!(
                    recommended.iter().any(|arn| arn == policy),
                    "case {addon_name:?}: expected the managed policy recommended for it \
                     {policy:?}, got {recommended:?}"
                );
                for entry in &entries {
                    assert!(
                        !string_member(entry, "serviceAccount").is_empty(),
                        "case {addon_name:?}: a recommendation names the service account it is \
                         for, got {entry:?}"
                    );
                }
                // The flag and the recommendation have one source: an add-on with
                // permissions of its own reports them on every published build.
                for build in versions_of(&addon) {
                    assert_eq!(
                        build["requiresIamPermissions"].as_bool(),
                        Some(true),
                        "case {addon_name:?}/{}: an add-on that needs IAM permissions reports \
                         the flag on every build, got {build:?}",
                        string_member(&build, "addonVersion")
                    );
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: who publishes an add-on, who owns it and the AWS Marketplace listing it
// is sold through are what the CATALOG states about an add-on NAME — the
// `AddonInfo$publisher`, `$owner` and `$marketplaceInformation`
// `DescribeAddonVersions` publishes (asserted above) — and not part of an
// INSTALLED add-on: the recorded service answers an install and a read of one
// with `Addon$publisher`, `$owner` and `$marketplaceInformation` absent, for an
// add-on Amazon EKS builds and for one bought through a listing alike. An
// `Addon` is the add-on's installed state; a client that wants the metadata
// reads the catalog for the name it installed.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn an_installed_addon_reports_no_catalog_metadata_of_its_own() {
    let router = emulator();
    active_cluster(&router, "addon-metadata", DEFAULT_KUBERNETES_VERSION).await;

    // The rows span the owner kinds an install can take: one Amazon Web Services
    // builds, an upstream project Amazon EKS curates, and one subscribed through
    // AWS Marketplace (the kind that carries a listing at all).
    for addon_name in ["vpc-cni", "cert-manager", "kubecost_kubecost"] {
        let published = published_addon(&router, addon_name, None).await;
        let created = create_addon(
            &router,
            "addon-metadata",
            &serde_json::json!({ "addonName": addon_name }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {addon_name:?}: expected HTTP 200 from CreateAddon, got {created:?}"
        );
        let described = describe_addon(&router, "addon-metadata", addon_name).await;
        assert_eq!(
            described.status, 200,
            "case {addon_name:?}: expected HTTP 200 from DescribeAddon, got {described:?}"
        );

        for member in ["owner", "publisher", "marketplaceInformation"] {
            assert_eq!(
                created.addon()[member],
                serde_json::Value::Null,
                "case {addon_name:?}: an installed add-on reports no {member}, got {:?}",
                created.addon()[member]
            );
            assert_eq!(
                described.addon()[member],
                serde_json::Value::Null,
                "case {addon_name:?}: a read of the add-on reports no {member} either, \
                 got {:?}",
                described.addon()[member]
            );
        }

        // The other side of the rule: the metadata the add-on does not report is
        // the metadata the catalog publishes for that same name.
        assert!(
            !string_member(&published, "publisher").is_empty(),
            "case {addon_name:?}: the catalog publishes who builds the add-on, got {published:?}"
        );
        let sold_on_marketplace = string_member(&published, "owner") == ADDON_OWNER_MARKETPLACE;
        let listing = published
            .get("marketplaceInformation")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        if sold_on_marketplace {
            assert!(
                !string_member(&listing, "productId").is_empty(),
                "case {addon_name:?}: the catalog publishes the listing a Marketplace add-on \
                 is sold through, got {listing:?}"
            );
        } else {
            assert!(
                listing.is_null(),
                "case {addon_name:?}: an add-on that is not sold on Marketplace is published \
                 with no listing, got {listing:?}"
            );
        }
    }
}
