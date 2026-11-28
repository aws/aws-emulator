// SPDX-License-Identifier: CC0-1.0

//! WHICH records `DescribeAddonVersions` selects out of the published add-on
//! catalog, and which Kubernetes versions each published record covers — driven
//! over the wire through the same `MultiServiceRouter` the production binary
//! mounts.
//!
//! The RULE the version suites assert: a listing narrowed by `kubernetesVersion`
//! names exactly the add-ons the publication carries a build for on that
//! Kubernetes version — every one of them and nothing else — and each named
//! record is the catalog's own row for that add-on, with only its builds narrowed
//! to the ones that support the version asked for. A Kubernetes version no
//! published build covers selects nothing, which is an empty listing rather than
//! a refusal.
//!
//! The RULE the rebuild suite asserts: a published build history belongs to the
//! add-on that publishes it. A vendor's REBUILD of an Amazon EKS-managed add-on
//! is a listing of the vendor's own, so it carries no build for a Kubernetes
//! version older than the window the catalog's newer add-ons share: it does not
//! inherit the first Kubernetes version Amazon EKS has offered the rebuilt
//! add-on on, which for a long-published add-on reaches back years before the
//! vendor's listing existed.
//!
//! The RULE the selection suite asserts: `types`, `publishers`, `owners` and
//! `addonName` SELECT rows of the publication — several values in one member
//! widen it, the members combine — and a selected row is the catalog's record
//! for that add-on unchanged.
//!
//! The rows of every table are read from the publication itself, so they differ
//! along what the rules generalise over: each Kubernetes version the catalog
//! names and one past each end of that run, add-ons published by Amazon EKS and
//! ones sold through AWS Marketplace, vendors' own products and their rebuilds of
//! Amazon EKS-managed add-ons, and single- and multi-valued filter members.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    ADDON_CATALOG_FIRST_CLUSTER_VERSION, ADDON_OWNER_MARKETPLACE,
};
use smithy_server_runtime::MultiServiceRouter;

// The readers every suite of this crate drives this listing through: one copy of
// "what a client sees", so a second place where it can drift is not created here.
use crate::addon_versions::{
    addons, compatibilities_of, describe_addon_versions, string_member, versions_of,
};
use crate::wire::emulator;

/// The published add-on catalog, narrowed by `query`: this listing reports a
/// catalog rather than resolving a resource, so a filter that keeps nothing is
/// answered with an empty list rather than a refusal.
async fn listing(router: &MultiServiceRouter, query: &str) -> Vec<serde_json::Value> {
    let answer = describe_addon_versions(router, query).await;
    assert_eq!(
        answer.status, 200,
        "case {query:?}: expected HTTP 200, got {answer:?}"
    );
    addons(&answer)
}

/// The whole publication, keyed by add-on name.
async fn catalog(router: &MultiServiceRouter) -> BTreeMap<String, serde_json::Value> {
    let rows: BTreeMap<String, serde_json::Value> = listing(router, "")
        .await
        .into_iter()
        .map(|addon| (name(&addon), addon))
        .collect();
    assert!(
        rows.len() > 1,
        "setup: the catalog must publish add-ons, got {rows:?}"
    );
    rows
}

fn name(addon: &serde_json::Value) -> String {
    string_member(addon, "addonName")
}

/// The Kubernetes versions one build names, in the order it names them.
fn compatible_versions(build: &serde_json::Value) -> Vec<String> {
    compatibilities_of(build)
        .iter()
        .map(|row| string_member(row, "clusterVersion"))
        .collect()
}

/// Every Kubernetes version a record publishes a build for.
fn published_versions(addon: &serde_json::Value) -> BTreeSet<String> {
    versions_of(addon)
        .iter()
        .flat_map(compatible_versions)
        .collect()
}

/// The minor number of a `1.NN` Kubernetes version, which is what orders the
/// published calendar.
fn minor(version: &str) -> u32 {
    version
        .split('.')
        .next_back()
        .and_then(|minor| minor.parse().ok())
        .unwrap_or_else(|| panic!("expected a 1.NN Kubernetes version, got {version:?}"))
}

/// The oldest Kubernetes version a record publishes a build for.
fn oldest_version(addon: &serde_json::Value) -> String {
    published_versions(addon)
        .into_iter()
        .min_by_key(|version| minor(version))
        .unwrap_or_else(|| panic!("expected published builds of {}", name(addon)))
}

/// Every Kubernetes version the publication names, oldest first, with one
/// version past each end of that run — the bounds a filtered listing is asked
/// about and the two values just outside them.
fn version_table(catalog: &BTreeMap<String, serde_json::Value>) -> Vec<String> {
    let published: BTreeSet<u32> = catalog
        .values()
        .flat_map(published_versions)
        .map(|version| minor(&version))
        .collect();
    let oldest = *published.iter().next().expect("a published version");
    let newest = *published.iter().next_back().expect("a published version");
    (oldest - 1..=newest + 1)
        .map(|minor| format!("1.{minor}"))
        .collect()
}

/// The members of a record that belong to the ADD-ON rather than to one build,
/// which a filter narrows the builds of but never rewrites.
const ADDON_MEMBERS: &[&str] = &[
    "addonName",
    "type",
    "publisher",
    "owner",
    "marketplaceInformation",
    "defaultNamespace",
];

// ---------------------------------------------------------------------------
// RULE: a listing narrowed by kubernetesVersion names exactly the add-ons the
// publication carries a build for on that version, each as the catalog's own
// record with its builds narrowed to the ones that support it.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_version_filtered_listing_names_the_add_ons_published_for_that_version() {
    let router = emulator();
    let catalog = catalog(&router).await;
    let versions = version_table(&catalog);
    assert!(
        versions.len() > 3,
        "setup: the published calendar must span several versions, got {versions:?}"
    );

    for version in &versions {
        let case = format!("kubernetesVersion={version}");
        let narrowed = listing(&router, &case).await;

        let selected: BTreeSet<String> = narrowed.iter().map(name).collect();
        let expected: BTreeSet<String> = catalog
            .iter()
            .filter(|(_, addon)| published_versions(addon).contains(version))
            .map(|(addon_name, _)| addon_name.clone())
            .collect();
        assert_eq!(
            selected, expected,
            "case {case:?}: the listing must name the add-ons the catalog publishes a build \
             for on {version}, got {selected:?}"
        );

        let names: Vec<String> = narrowed.iter().map(name).collect();
        let mut in_order = names.clone();
        in_order.sort();
        assert_eq!(
            names, in_order,
            "case {case:?}: the catalog is published in add-on name order, got {names:?}"
        );

        for addon in &narrowed {
            let addon_name = name(addon);
            let whole = &catalog[&addon_name];
            for member in ADDON_MEMBERS {
                assert_eq!(
                    addon[*member], whole[*member],
                    "case {case:?}: {addon_name} must be served as the catalog publishes it, \
                     {member} came back as {:?}",
                    addon[*member]
                );
            }

            let published = versions_of(whole);
            for build in versions_of(addon) {
                let addon_version = string_member(&build, "addonVersion");
                assert_eq!(
                    compatible_versions(&build),
                    vec![version.clone()],
                    "case {case:?}: {addon_name} at {addon_version} must report the \
                     compatibility asked about alone, got {:?}",
                    build["compatibilities"]
                );
                let same = published
                    .iter()
                    .find(|candidate| string_member(candidate, "addonVersion") == addon_version)
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case:?}: {addon_name} served a build the catalog does not \
                             publish, got {addon_version:?} among {published:?}"
                        )
                    });
                assert!(
                    compatible_versions(same).contains(version),
                    "case {case:?}: {addon_name} at {addon_version} is published for {:?}, \
                     which does not include {version}",
                    compatible_versions(same)
                );
                for member in ["architecture", "computeTypes", "requiresConfiguration", "requiresIamPermissions"] {
                    assert_eq!(
                        build[member], same[member],
                        "case {case:?}: {addon_name} at {addon_version} must be served as the \
                         catalog publishes it, {member} came back as {:?}",
                        build[member]
                    );
                }
            }
        }

        if expected.is_empty() {
            assert!(
                narrowed.is_empty(),
                "case {case:?}: a Kubernetes version no build covers selects nothing, got {:?}",
                narrowed.iter().map(name).collect::<Vec<String>>()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: a published build history belongs to the add-on that publishes it, so a
// vendor's rebuild of an Amazon EKS-managed add-on carries no build older than
// its own listing's window and does not inherit the rebuilt add-on's.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_vendor_rebuild_publishes_its_own_build_window() {
    let router = emulator();
    let catalog = catalog(&router).await;
    let window = minor(ADDON_CATALOG_FIRST_CLUSTER_VERSION);

    // The rebuilds the publication carries: a Marketplace add-on sold as
    // `<vendor>_<add-on>-fips`, beside the Amazon EKS-managed add-on of that name.
    let rebuilds: Vec<(String, String)> = catalog
        .iter()
        .filter(|(_, addon)| string_member(addon, "owner") == ADDON_OWNER_MARKETPLACE)
        .filter_map(|(addon_name, _)| {
            let product = addon_name.split_once('_')?.1.strip_suffix("-fips")?;
            catalog
                .contains_key(product)
                .then(|| (addon_name.clone(), product.to_string()))
        })
        .collect();
    assert!(
        rebuilds.len() > 1,
        "setup: the catalog must publish vendor rebuilds, got {rebuilds:?}"
    );

    // The window the catalog publishes a vendor's listing over, read as a
    // LITERAL for two rebuilds of add-ons whose own windows differ (the EFS CSI
    // driver has been offered since the oldest row the calendar carries, the VPC
    // CNI only over the newer window), so the published start cannot move with
    // the constant above unnoticed.
    for (addon_name, expected) in [
        ("chainguard_aws-efs-csi-driver-fips", "1.30"),
        ("chainguard_vpc-cni-fips", "1.30"),
    ] {
        let published = catalog.get(addon_name).unwrap_or_else(|| {
            panic!("setup: the catalog must publish {addon_name}, got {:?}", catalog.keys())
        });
        assert_eq!(
            oldest_version(published),
            expected,
            "case {addon_name}: the vendor's listing is published from {expected} on, got {:?}",
            published_versions(published)
        );
    }

    for (rebuild_name, rebuilt_name) in &rebuilds {
        let case = rebuild_name.as_str();
        let rebuild = &catalog[rebuild_name];
        let rebuilt = &catalog[rebuilt_name];
        let rebuild_oldest = oldest_version(rebuild);
        let rebuilt_oldest = oldest_version(rebuilt);

        assert!(
            minor(&rebuild_oldest) >= window,
            "case {case}: a vendor's listing publishes no build older than \
             {ADDON_CATALOG_FIRST_CLUSTER_VERSION}, got {rebuild_oldest:?}"
        );
        if minor(&rebuilt_oldest) < window {
            assert!(
                minor(&rebuild_oldest) > minor(&rebuilt_oldest),
                "case {case}: the rebuild must not inherit the window of {rebuilt_name}, \
                 which starts at {rebuilt_oldest:?}, got {rebuild_oldest:?}"
            );
        }

        // And a listing filtered to a version older than the rebuild's own
        // window names the rebuilt add-on where it is published for it, never the
        // rebuild.
        for older in version_table(&catalog)
            .into_iter()
            .filter(|version| minor(version) < minor(&rebuild_oldest))
        {
            let query = format!("kubernetesVersion={older}");
            let selected: BTreeSet<String> = listing(&router, &query).await.iter().map(name).collect();
            assert!(
                !selected.contains(rebuild_name),
                "case {case}: the rebuild publishes no build for {older}, yet the listing \
                 filtered to it names the rebuild, got {selected:?}"
            );
            assert_eq!(
                selected.contains(rebuilt_name),
                published_versions(rebuilt).contains(&older),
                "case {case}: the listing filtered to {older} must name {rebuilt_name} exactly \
                 when it publishes a build for it, got {selected:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// RULE: types / publishers / owners / addonName select rows of the publication —
// several values widen one member, the members combine — and a selected row is
// the catalog's record unchanged.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_filtered_listing_selects_the_catalogs_own_records() {
    let router = emulator();
    let catalog = catalog(&router).await;

    type Keeps = Box<dyn Fn(&serde_json::Value) -> bool>;
    let storage: Keeps = Box::new(|addon| string_member(addon, "type") == "storage");
    let marketplace_storage: Keeps = Box::new(|addon| {
        string_member(addon, "type") == "storage"
            && string_member(addon, "owner") == ADDON_OWNER_MARKETPLACE
    });
    let netapp: Keeps = Box::new(|addon| string_member(addon, "publisher") == "NetApp");
    let two_types: Keeps =
        Box::new(|addon| ["storage", "networking"].contains(&string_member(addon, "type").as_str()));
    let by_name: Keeps = Box::new(|addon| name(addon) == "vpc-cni");
    let unpublished: Keeps = Box::new(|_| false);

    // Each row: the query a client sends, and which published rows it keeps.
    let cases: Vec<(&str, Keeps)> = vec![
        ("types=storage", storage),
        (
            "types=storage&owners=aws-marketplace",
            marketplace_storage,
        ),
        ("publishers=NetApp", netapp),
        ("types=storage&types=networking", two_types),
        ("addonName=vpc-cni", by_name),
        ("addonName=no-such-add-on", unpublished),
    ];

    for (query, keeps) in cases {
        let selected = listing(&router, query).await;
        let names: Vec<String> = selected.iter().map(name).collect();
        let expected: Vec<String> = catalog
            .iter()
            .filter(|(_, addon)| keeps(addon))
            .map(|(addon_name, _)| addon_name.clone())
            .collect();
        assert_eq!(
            names, expected,
            "case {query:?}: the listing must select the published rows the filters keep, in \
             name order, got {names:?}"
        );
        for addon in &selected {
            let addon_name = name(addon);
            assert_eq!(
                *addon, catalog[&addon_name],
                "case {query:?}: {addon_name} must be served exactly as the catalog publishes \
                 it, got {addon:?}"
            );
        }
    }
}
