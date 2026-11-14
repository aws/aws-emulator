// SPDX-License-Identifier: CC0-1.0

//! The reference DATA the read-only catalogs publish, driven over the wire
//! through the same `MultiServiceRouter` the production binary mounts.
//!
//! The other catalog suites cover the SHAPE of these listings — their filters,
//! their paging and the members each entry carries. This one covers what they
//! publish:
//!
//! * `DescribeAddonVersions` publishes the AWS Marketplace add-ons beside the
//!   Amazon EKS-managed ones, each with its vendor as publisher, the
//!   `aws-marketplace` owner, its own default namespace, the
//!   `marketplaceInformation` of the listing it is subscribed through and a
//!   populated build list — so a subscriber discovers it, version-pins it and
//!   installs it like any other add-on.
//! * `DescribeClusterVersions` publishes ONE row per Kubernetes minor version
//!   Amazon EKS has released, from the oldest version the calendar carries to the
//!   newest with no gap, each row reporting its own published dates and the
//!   lifecycle state those dates give at the clock — including the versions whose
//!   support has ended, which the listing reports rather than hides, while
//!   `CreateCluster` still refuses them.
//! * `ListAccessPolicies` publishes every managed cluster access policy Amazon
//!   EKS publishes, as `name` plus the service-owned ARN built from it.
//! * `DescribeAddonConfiguration` publishes the add-on's OWN schema document: a
//!   JSON Schema whose root is a `$ref` into its own `definitions`, whose named
//!   type declares that add-on's configurable keys, and whose object-valued keys
//!   reference definitions of their own — and that document is what an install is
//!   held to.
//!
//! Every suite asserts over a TABLE whose rows differ along what the rule
//! generalises over (which vendor, which Kubernetes version, which policy family,
//! which add-on and which key of its schema), with rows the issue behind this
//! suite never cited, and with both ends of the published version range plus one
//! version past each end.

use std::collections::BTreeSet;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    access_policy_arn, ADDON_OWNER_MARKETPLACE, ADDON_SCHEMA_DIALECT_DRAFT_06,
    ADDON_SCHEMA_DIALECT_DRAFT_07, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::marketplace_listing_reference_data::{
    is_marketplace_product_id, marketplace_listing_path,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

const REGION: &str = "us-west-2";

/// The whole published add-on catalog, narrowed by `query`.
async fn describe_addon_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = format!("/addons/supported-versions?maxResults=100&{query}");
    let answer = call(router, REGION, "GET", &uri, "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions({query}): expected HTTP 200, got {answer:?}"
    );
    answer
}

/// The `addons` of one listing.
fn addons(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .clone()
}

/// A string member of a JSON object, which every case names in its failure.
fn text(value: &serde_json::Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a {member} string, got {value:?}"))
        .to_string()
}

/// The published version calendar, narrowed by `query`.
async fn describe_cluster_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = format!("/cluster-versions?maxResults=100&{query}");
    let answer = call(router, REGION, "GET", &uri, "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions({query}): expected HTTP 200, got {answer:?}"
    );
    answer
}

/// The rows of one version listing.
fn version_rows(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .clone()
}

/// A Kubernetes minor version `"1.31"` as the pair `(1, 31)`, so two of them are
/// ordered numerically rather than lexically.
fn minor_version(version: &str) -> (u64, u64) {
    let (major, minor) = version
        .split_once('.')
        .unwrap_or_else(|| panic!("expected a <major>.<minor> version, got {version:?}"));
    (
        major.parse().unwrap_or_else(|_| panic!("major of {version:?}")),
        minor.parse().unwrap_or_else(|_| panic!("minor of {version:?}")),
    )
}

/// An epoch-seconds timestamp member, as the wire carries it.
fn epoch_secs(value: &serde_json::Value) -> f64 {
    value
        .as_f64()
        .unwrap_or_else(|| panic!("expected a timestamp, got {value:?}"))
}

/// Seconds since the epoch, as the handler's own clock reads them.
fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .expect("a clock after the epoch")
        .as_secs_f64()
}

/// The smallest `CreateCluster` request, on `version`.
fn cluster_request(name: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "version": version,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

// ---------------------------------------------------------------------------
// RULE: an AWS Marketplace add-on is published in the same catalog as the Amazon
// EKS-managed ones — reachable by its own `addonName`, by its vendor through
// `publishers` and by `owners=aws-marketplace` — and reports that owner, its
// vendor as publisher, the category it is published under, the namespace its
// workloads are installed into, the `marketplaceInformation` of the listing it is
// subscribed through, and every build of it.
// ---------------------------------------------------------------------------

/// The Marketplace add-ons this case walks, as `(addonName, publisher, type,
/// defaultNamespace)`.
///
/// Source: the AWS Marketplace listings Amazon EKS publishes as add-ons, whose
/// `<vendor>_<product>` names are what `CreateAddon$addonName` takes, with the
/// publisher the seller is published under, and the category and the namespace
/// each vendor's own install documentation gives. The NetApp, Upbound and
/// Kubecost rows are add-ons no report behind this suite cited, so the rule is
/// asserted beyond the two names it did.
const MARKETPLACE_ADDONS: &[(&str, &str, &str, &str)] = &[
    ("accuknox_kubearmor", "AccuKnox", "security", "kubearmor"),
    (
        "eg-innovations_eg-universal-agent-operator",
        "EG Innovations",
        "monitoring",
        "eg-agents",
    ),
    ("netapp_trident-operator", "NetApp", "storage", "trident"),
    (
        "upbound_universal-crossplane",
        "Upbound",
        "infra-management",
        "upbound-system",
    ),
    ("kubecost_kubecost", "Kubecost", "cost-management", "kubecost"),
];

#[tokio::test]
async fn marketplace_addons_are_published_with_their_listing_and_builds() {
    let router = emulator();

    let mut listings: BTreeSet<String> = BTreeSet::new();
    for (addon_name, publisher, addon_type, namespace) in MARKETPLACE_ADDONS {
        let case = *addon_name;
        let answer = describe_addon_versions(&router, &format!("addonName={addon_name}")).await;
        let listed = addons(&answer);
        assert_eq!(
            listed.len(),
            1,
            "case {case}: a name the catalog publishes answers exactly that add-on, got {:?}",
            answer.body["addons"]
        );
        let addon = &listed[0];
        for (member, expected) in [
            ("addonName", *addon_name),
            ("publisher", *publisher),
            ("type", *addon_type),
            ("defaultNamespace", *namespace),
            ("owner", ADDON_OWNER_MARKETPLACE),
        ] {
            assert_eq!(
                text(addon, member),
                expected,
                "case {case}: expected {member} {expected:?}, got {:?}",
                addon[member]
            );
        }

        // The listing the add-on is subscribed through: a product id, and the
        // address that product is published at.
        let listing = &addon["marketplaceInformation"];
        let product_id = text(listing, "productId");
        let product_url = text(listing, "productUrl");
        assert!(
            is_marketplace_product_id(&product_id),
            "case {case}: expected the listing's Marketplace product id, got {listing:?}"
        );
        assert!(
            marketplace_listing_path(&product_url).is_some(),
            "case {case}: expected a Marketplace listing address, got {product_url:?}"
        );
        assert!(
            listings.insert(product_id.clone()),
            "case {case}: two add-ons must not report one listing, got {product_id:?} twice"
        );

        // And the builds a subscriber version-pins against: every one of them
        // carries the version, the architectures it is built for and the cluster
        // versions it supports.
        let builds = addon["addonVersions"]
            .as_array()
            .unwrap_or_else(|| panic!("case {case}: expected an addonVersions list, got {addon:?}"))
            .clone();
        assert!(
            !builds.is_empty(),
            "case {case}: a published add-on has builds, got {addon:?}"
        );
        for build in &builds {
            let version = text(build, "addonVersion");
            assert!(
                build["architecture"]
                    .as_array()
                    .is_some_and(|architectures| !architectures.is_empty()),
                "case {case}/{version}: expected the architectures it is built for, got {build:?}"
            );
            assert!(
                build["compatibilities"]
                    .as_array()
                    .is_some_and(|compatibilities| !compatibilities.is_empty()),
                "case {case}/{version}: expected the cluster versions it supports, got {build:?}"
            );
            assert!(
                build["requiresConfiguration"].is_boolean()
                    && build["requiresIamPermissions"].is_boolean(),
                "case {case}/{version}: expected both install flags, got {build:?}"
            );
        }

        // A read of the same add-on reports the same listing: the catalog is a
        // publication, so nothing about it moves between reads.
        let again = describe_addon_versions(&router, &format!("addonName={addon_name}")).await;
        assert_eq!(
            addons(&again)[0]["marketplaceInformation"],
            *listing,
            "case {case}: the published listing must be stable across reads, got {:?}",
            addons(&again)[0]["marketplaceInformation"]
        );

        // The vendor's own publisher filter keeps it, and the Marketplace owner
        // filter keeps it too.
        // A publisher is the seller's published name, which carries the spaces
        // that name has: the filter value travels percent-encoded in the query.
        let filtered_by = publisher.replace(' ', "%20");
        let by_publisher =
            describe_addon_versions(&router, &format!("publishers={filtered_by}")).await;
        let published_by_vendor: Vec<String> = addons(&by_publisher)
            .iter()
            .map(|addon| text(addon, "addonName"))
            .collect();
        assert!(
            published_by_vendor.contains(&(*addon_name).to_string()),
            "case {case}: the vendor's publisher filter must keep it, got \
             {published_by_vendor:?}"
        );
    }

    // `owners=aws-marketplace` is the Marketplace catalogue: it keeps every row
    // above and nothing an Amazon EKS owner owns.
    let by_owner =
        describe_addon_versions(&router, &format!("owners={ADDON_OWNER_MARKETPLACE}")).await;
    let marketplace_owned: Vec<String> = addons(&by_owner)
        .iter()
        .map(|addon| text(addon, "addonName"))
        .collect();
    for (addon_name, _, _, _) in MARKETPLACE_ADDONS {
        assert!(
            marketplace_owned.contains(&(*addon_name).to_string()),
            "case owners filter: expected {addon_name} among the Marketplace add-ons, got \
             {marketplace_owned:?}"
        );
    }
    for addon in addons(&by_owner) {
        let case = text(&addon, "addonName");
        assert_eq!(
            text(&addon, "owner"),
            ADDON_OWNER_MARKETPLACE,
            "case owners filter/{case}: expected only Marketplace-owned add-ons, got {:?}",
            addon["owner"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the version calendar publishes one row per Kubernetes minor version
// Amazon EKS has released — an unbroken run from the oldest version it carries to
// the newest — each row reporting its own published release and end-of-support
// dates and the lifecycle state those dates give at the clock. A version whose
// support has ended is PUBLISHED, not hidden; it is simply not one a cluster can
// be created on.
// ---------------------------------------------------------------------------

/// The calendar rows this case names, as `(version, release, end of standard
/// support, end of extended support)` — each date the UTC day the published
/// calendar gives.
///
/// Source: the "Amazon EKS Kubernetes release calendar" of the Amazon EKS User
/// Guide, which lists one row per minor version with those three days. The rows
/// are BOTH ends of the published range (the oldest version the calendar carries
/// and the newest) plus versions in between that no report behind this suite
/// cited — a version whose extended support ran out years ago, one that is in
/// extended support, and one in standard support — so the rule is asserted across
/// the whole lifecycle rather than at one point of it.
const CALENDAR_ROWS: &[(&str, &str, &str, &str)] = &[
    // The oldest row the published calendar carries.
    ("1.23", "2022-08-11", "2023-10-11", "2024-10-11"),
    ("1.26", "2023-04-11", "2024-06-11", "2025-06-11"),
    ("1.29", "2024-01-23", "2025-03-23", "2026-03-23"),
    ("1.33", "2025-05-28", "2026-07-28", "2027-07-28"),
    // The newest row.
    ("1.37", "2026-10-01", "2027-12-01", "2028-12-01"),
];

/// The first instant (UTC) of a published calendar day `YYYY-MM-DD`, in epoch
/// seconds — what a row's three date members carry on the wire.
///
/// The day is widened to an RFC-3339 instant and parsed by `aws-smithy-types`,
/// which is the parser the wire layer itself uses, rather than by calendar
/// arithmetic written here.
fn published_day(day: &str) -> f64 {
    let text = format!("{day}T00:00:00Z");
    let parsed = aws_smithy_types::DateTime::from_str(
        &text,
        aws_smithy_types::date_time::Format::DateTime,
    )
    .unwrap_or_else(|error| panic!("{text} is not an RFC-3339 instant: {error}"));
    parsed.secs() as f64
}

/// One version below the oldest published row and one above the newest: neither
/// is a version the calendar publishes.
const UNPUBLISHED_VERSIONS: &[&str] = &["1.22", "1.38"];

#[tokio::test]
async fn the_version_calendar_publishes_every_released_minor_version() {
    let router = emulator();

    let published = version_rows(&describe_cluster_versions(&router, "").await);
    let versions: Vec<String> = published
        .iter()
        .map(|row| text(row, "clusterVersion"))
        .collect();

    // One unbroken run of minor versions: a caller walking the catalogue reads
    // every version between the ends, so a version it has never heard of is one
    // outside the range rather than a hole inside it.
    let keys: Vec<(u64, u64)> = versions.iter().map(|version| minor_version(version)).collect();
    assert!(
        keys.windows(2)
            .all(|pair| pair[0].0 == pair[1].0 && pair[0].1 == pair[1].1 + 1),
        "case contiguity: expected one unbroken run of minor versions, got {versions:?}"
    );

    // Each named row is published, with its own dates and the state those dates
    // give at the clock.
    let clock = now_secs();
    for (version, release, end_of_standard, end_of_extended) in CALENDAR_ROWS {
        let row = published
            .iter()
            .find(|row| text(row, "clusterVersion") == *version)
            .unwrap_or_else(|| {
                panic!("case {version}: the calendar must publish it, got {versions:?}")
            });
        let end_of_standard = published_day(end_of_standard);
        let end_of_extended = published_day(end_of_extended);
        for (member, expected) in [
            ("releaseDate", published_day(release)),
            ("endOfStandardSupportDate", end_of_standard),
            ("endOfExtendedSupportDate", end_of_extended),
        ] {
            assert_eq!(
                epoch_secs(&row[member]),
                expected,
                "case {version}: expected the published {member}, got {:?}",
                row[member]
            );
        }
        let expected_state = if clock < end_of_standard {
            "STANDARD_SUPPORT"
        } else if clock < end_of_extended {
            "EXTENDED_SUPPORT"
        } else {
            "UNSUPPORTED"
        };
        assert_eq!(
            text(row, "versionStatus"),
            expected_state,
            "case {version}: expected the state its own dates give, got {:?}",
            row["versionStatus"]
        );
        assert_eq!(
            text(row, "status"),
            expected_state,
            "case {version}: both state members report one lifecycle state, got {:?}",
            row["status"]
        );
        // A row carries the scaling tiers a provisioned control plane of that
        // version can run on, each named and sized.
        let tiers = row["controlPlaneScalingTiers"]
            .as_array()
            .unwrap_or_else(|| panic!("case {version}: expected the scaling tiers, got {row:?}"))
            .clone();
        assert!(
            !tiers.is_empty(),
            "case {version}: expected the provisioned scaling tiers, got {row:?}"
        );
        for tier in &tiers {
            let name = text(tier, "tierName");
            assert!(
                tier["apiRequestConcurrency"].is_i64()
                    && tier["podSchedulingRatePerSecond"].is_i64()
                    && tier["clusterDatabaseSizeGb"].is_i64(),
                "case {version}/{name}: expected the tier's capabilities, got {tier:?}"
            );
        }
        // And a filter naming it answers exactly that row.
        let narrowed =
            version_rows(&describe_cluster_versions(&router, &format!("clusterVersions={version}")).await);
        let reported: Vec<String> = narrowed
            .iter()
            .map(|row| text(row, "clusterVersion"))
            .collect();
        assert_eq!(
            reported,
            vec![(*version).to_string()],
            "case {version}: a filter naming one published version answers it, got {reported:?}"
        );
    }

    // One version past each end of the published range is published by nothing.
    for unpublished in UNPUBLISHED_VERSIONS {
        assert!(
            !versions.contains(&(*unpublished).to_string()),
            "case {unpublished}: a version outside the published range is not a row, got \
             {versions:?}"
        );
        let narrowed = version_rows(
            &describe_cluster_versions(&router, &format!("clusterVersions={unpublished}")).await,
        );
        assert!(
            narrowed.is_empty(),
            "case {unpublished}: a filter naming it answers nothing, got {narrowed:?}"
        );
    }

    // The versions whose support has ended are PART of the published listing —
    // the rule the newest minors and the oldest rows share — and `includeAll`
    // asks for exactly what is already published.
    let out_of_support: Vec<String> =
        version_rows(&describe_cluster_versions(&router, "versionStatus=UNSUPPORTED").await)
            .iter()
            .map(|row| text(row, "clusterVersion"))
            .collect();
    assert!(
        !out_of_support.is_empty(),
        "case out of support: the calendar carries versions whose support has ended, got \
         {versions:?}"
    );
    for version in &out_of_support {
        assert!(
            versions.contains(version),
            "case {version}: an out-of-support row is published, not hidden, got {versions:?}"
        );
    }
    let widened: Vec<String> =
        version_rows(&describe_cluster_versions(&router, "includeAll=true").await)
            .iter()
            .map(|row| text(row, "clusterVersion"))
            .collect();
    assert_eq!(
        widened, versions,
        "case includeAll: expected the same published calendar, got {widened:?}"
    );

    // Published is not the same as offered: a cluster is created only on a
    // version whose support has not ended, and the state on the row is what says
    // which those are.
    for (index, version) in out_of_support.iter().enumerate() {
        let name = format!("unsupported-{index}");
        let answer = call(
            &router,
            REGION,
            "POST",
            "/clusters",
            &cluster_request(&name, version).to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 400,
            "case {version}: a version past extended support creates no cluster, got {answer:?}"
        );
    }
    let in_support: Vec<String> = published
        .iter()
        .filter(|row| text(row, "versionStatus") != "UNSUPPORTED")
        .map(|row| text(row, "clusterVersion"))
        .collect();
    assert!(
        in_support.len() >= 2,
        "case offered versions: the calendar keeps offering a choice of versions, got \
         {versions:?}"
    );
    for (index, version) in in_support.iter().enumerate() {
        let name = format!("offered-{index}");
        let answer = call(
            &router,
            REGION,
            "POST",
            "/clusters",
            &cluster_request(&name, version).to_string(),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {version}: a version still in support creates a cluster, got {answer:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `ListAccessPolicies` publishes every managed cluster access policy Amazon
// EKS publishes, each as its `name` and the service-owned ARN built from that
// name — which is the value `AssociateAccessPolicy` takes back.
// ---------------------------------------------------------------------------

/// The policies this case names.
///
/// Source: the "Access policy permissions" reference's table of Amazon EKS access
/// policies. The Amazon SageMaker HyperPod family covers the access its
/// orchestration is granted on a cluster; `AmazonEKSKROPolicy` and
/// `AmazonEKSAdminViewPolicy` are policies no report behind this suite cited.
const PUBLISHED_POLICIES: &[&str] = &[
    "AmazonSagemakerHyperpodSystemNamespacePolicy",
    "AmazonSagemakerHyperpodTrainingPolicy",
    "AmazonSagemakerHyperpodUserClusterPolicy",
    "AmazonEKSKROPolicy",
    "AmazonEKSAdminViewPolicy",
];

#[tokio::test]
async fn the_managed_access_policy_catalogue_publishes_every_policy_by_name() {
    let router = emulator();

    let answer = call(&router, REGION, "GET", "/access-policies?maxResults=100", "").await;
    assert_eq!(answer.status, 200, "setup ListAccessPolicies: got {answer:?}");
    let published = answer.body["accessPolicies"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an accessPolicies list, got {:?}", answer.body))
        .clone();
    let names: Vec<String> = published.iter().map(|entry| text(entry, "name")).collect();

    for policy in PUBLISHED_POLICIES {
        let entry = published
            .iter()
            .find(|entry| text(entry, "name") == *policy)
            .unwrap_or_else(|| {
                panic!("case {policy}: the catalogue must publish it, got {names:?}")
            });
        assert_eq!(
            text(entry, "arn"),
            access_policy_arn(policy),
            "case {policy}: expected the service-owned ARN built from its name, got {:?}",
            entry["arn"]
        );
    }

    // Every entry is published once, under a name its own ARN ends with.
    let distinct: BTreeSet<&String> = names.iter().collect();
    assert_eq!(
        distinct.len(),
        names.len(),
        "case uniqueness: expected every policy once, got {names:?}"
    );
    for entry in &published {
        let name = text(entry, "name");
        assert!(
            text(entry, "arn").ends_with(&format!("/{name}")),
            "case {name}: an entry's ARN names the entry, got {:?}",
            entry["arn"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DescribeAddonConfiguration` publishes the add-on's OWN schema document:
// a JSON Schema in the dialect that add-on publishes, whose root is a `$ref` into
// the document's own `definitions`, whose referenced type declares the add-on's
// configurable keys and allows no other key, and whose object-valued keys
// reference definitions that declare those objects' own keys. That document is
// what `CreateAddon` holds a `configurationValues` document to.
// ---------------------------------------------------------------------------

/// The add-ons and schema definitions this case walks, as `(addonName, dialect,
/// root definition, [(object key, its definition, a key that definition
/// declares)])`.
///
/// Source: the schema each add-on publishes — what `aws eks
/// describe-addon-configuration --addon-name <name> --addon-version <build>`
/// reports — whose definitions are titled after the chart values types they
/// describe. The EBS CSI driver and CoreDNS rows are add-ons no report behind
/// this suite cited.
/// One object-valued key of a published schema document: the key, the definition
/// it references, and a key that definition declares (empty for a definition
/// whose own keys the catalog publishes none of).
type SchemaObjectKey = (&'static str, &'static str, &'static str);

/// One published schema document: the add-on, the dialect it is written in, the
/// definition its root references, and the object-valued keys that definition
/// carries.
type SchemaDocument = (
    &'static str,
    &'static str,
    &'static str,
    &'static [SchemaObjectKey],
);

const SCHEMA_DOCUMENTS: &[SchemaDocument] = &[
    (
        "vpc-cni",
        ADDON_SCHEMA_DIALECT_DRAFT_06,
        "VpcCni",
        &[
            ("env", "Env", "ENABLE_PREFIX_DELEGATION"),
            ("eniConfig", "EniConfig", "subnets"),
            ("nodeAgent", "NodeAgent", "healthzBindAddr"),
        ],
    ),
    (
        "aws-ebs-csi-driver",
        ADDON_SCHEMA_DIALECT_DRAFT_06,
        "AwsEbsCsiDriver",
        &[
            ("controller", "Controller", "replicaCount"),
            ("node", "Node", "volumeAttachLimit"),
        ],
    ),
    (
        "coredns",
        ADDON_SCHEMA_DIALECT_DRAFT_06,
        "Coredns",
        &[("autoScaling", "AutoScaling", "minReplicas")],
    ),
    (
        "external-dns",
        ADDON_SCHEMA_DIALECT_DRAFT_07,
        "ExternalDns",
        &[("resources", "Resources", "")],
    ),
];

/// The newest build of `addon_name` the catalog publishes.
async fn newest_build(router: &MultiServiceRouter, addon_name: &str) -> String {
    let answer = describe_addon_versions(router, &format!("addonName={addon_name}")).await;
    let listed = addons(&answer);
    let addon = listed
        .first()
        .unwrap_or_else(|| panic!("setup {addon_name}: the catalog must publish it"));
    let builds = addon["addonVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("setup {addon_name}: expected builds, got {addon:?}"));
    text(
        builds
            .first()
            .unwrap_or_else(|| panic!("setup {addon_name}: expected a published build")),
        "addonVersion",
    )
}

/// The published schema document for one build, parsed.
async fn schema_document(
    router: &MultiServiceRouter,
    addon_name: &str,
    addon_version: &str,
) -> serde_json::Value {
    let uri = format!(
        "/addons/configuration-schemas?addonName={}&addonVersion={}",
        encode_label(addon_name),
        encode_label(addon_version)
    );
    let answer = call(router, REGION, "GET", &uri, "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonConfiguration({addon_name}, {addon_version}): got {answer:?}"
    );
    let document = text(&answer.body, "configurationSchema");
    serde_json::from_str(&document).unwrap_or_else(|error| {
        panic!("({addon_name}, {addon_version}): configurationSchema must be JSON: {error}")
    })
}

#[tokio::test]
async fn the_published_configuration_schema_is_the_addons_own_document() {
    let router = emulator();

    for (addon_name, dialect, root_title, object_keys) in SCHEMA_DOCUMENTS {
        let addon_version = newest_build(&router, addon_name).await;
        let case = format!("{addon_name}/{addon_version}");
        let document = schema_document(&router, addon_name, &addon_version).await;

        assert_eq!(
            text(&document, "$schema"),
            *dialect,
            "case {case}: expected the dialect this add-on publishes, got {:?}",
            document["$schema"]
        );
        assert_eq!(
            text(&document, "$ref"),
            format!("#/definitions/{root_title}"),
            "case {case}: the document's root references its own definition of the add-on's \
             values type, got {:?}",
            document["$ref"]
        );
        let root = &document["definitions"][root_title];
        assert_eq!(
            text(root, "type"),
            "object",
            "case {case}: the add-on's values type is an object, got {root:?}"
        );
        assert_eq!(
            root["additionalProperties"],
            serde_json::json!(false),
            "case {case}: a key the add-on does not take is not allowed, got {:?}",
            root["additionalProperties"]
        );

        for (key, definition_title, declared) in *object_keys {
            let reference = &root["properties"][key];
            assert_eq!(
                text(reference, "$ref"),
                format!("#/definitions/{definition_title}"),
                "case {case}/{key}: an object-valued key references its own definition, got \
                 {reference:?}"
            );
            let definition = &document["definitions"][definition_title];
            assert_eq!(
                text(definition, "title"),
                *definition_title,
                "case {case}/{key}: the definition is titled after the type it describes, got \
                 {definition:?}"
            );
            assert_eq!(
                text(definition, "type"),
                "object",
                "case {case}/{key}: expected an object definition, got {definition:?}"
            );
            if !declared.is_empty() {
                assert!(
                    definition["properties"][declared]["type"].is_string(),
                    "case {case}/{key}: expected the definition to declare {declared}, got \
                     {definition:?}"
                );
            }
        }
    }

    // And the document is what an install is held to, THROUGH its references: a
    // nested key the referenced definition declares is accepted with the type it
    // declares and refused with another, named by the path at fault.
    let in_support = version_rows(&describe_cluster_versions(&router, "defaultOnly=true").await);
    let cluster_version = text(
        in_support
            .first()
            .expect("the calendar marks one version the default"),
        "clusterVersion",
    );
    let created = call(
        &router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request("schema-driven", &cluster_version).to_string(),
    )
    .await;
    assert_eq!(created.status, 200, "setup CreateCluster: got {created:?}");
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;

    // (case, the document a create sends, the violation it must be refused with).
    // The refused rows come FIRST, so each of them is judged against the schema on
    // a cluster that has no add-on installed yet, and the accepted row last.
    let rows: &[(&str, &str, Option<&str>)] = &[
        (
            "a nested key of another type",
            "{\"env\": {\"WARM_IP_TARGET\": 3}}",
            Some("$.env.WARM_IP_TARGET: integer found, string expected"),
        ),
        (
            "an object-valued key given a scalar",
            "{\"env\": \"all of them\"}",
            Some("$.env: string found, object expected"),
        ),
        (
            "a nested key of the type its definition declares",
            "{\"env\": {\"WARM_IP_TARGET\": \"3\"}}",
            None,
        ),
    ];
    for (case, values, violation) in rows {
        let request = serde_json::json!({
            "addonName": "vpc-cni",
            "configurationValues": values,
        });
        let answer = call(
            &router,
            REGION,
            "POST",
            "/clusters/schema-driven/addons",
            &request.to_string(),
        )
        .await;
        match violation {
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case}: a document the published schema accepts installs, got \
                     {answer:?}"
                );
                let described = call(
                    &router,
                    REGION,
                    "GET",
                    "/clusters/schema-driven/addons/vpc-cni",
                    "",
                )
                .await;
                assert_eq!(
                    described.status, 200,
                    "case {case}: the installed add-on must read back, got {described:?}"
                );
                assert_eq!(
                    described.addon()["configurationValues"],
                    serde_json::json!(values),
                    "case {case}: the stored document reads back as supplied, got {:?}",
                    described.addon()["configurationValues"]
                );
            }
            Some(expected) => {
                assert_eq!(
                    answer.status, 400,
                    "case {case}: expected HTTP 400, got {answer:?}"
                );
                assert_eq!(
                    error_code(&answer),
                    "InvalidParameterException",
                    "case {case}: expected the bad-parameter channel, got {answer:?}"
                );
                assert!(
                    answer.message().contains(expected),
                    "case {case}: expected the refusal to name {expected:?}, got {:?}",
                    answer.message()
                );
            }
        }
    }
}
