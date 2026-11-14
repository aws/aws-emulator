// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeAddonVersions` and `DescribeAddonConfiguration`,
//! driven over the wire through the same `MultiServiceRouter` the production
//! binary mounts.
//!
//! The RULE under test is: both operations report the add-on catalog this
//! service's state holds — `DescribeAddonVersions` reports it narrowed by the
//! request's filters and served whole, newest build of each add-on first, with
//! exactly one default build per add-on per Kubernetes version;
//! `DescribeAddonConfiguration`
//! resolves one published `(addonName, addonVersion)` pair of that same catalog —
//! and `CreateAddon` installs the catalog's default build when the caller names
//! no version, so the versions a caller can pick and the version an installed
//! add-on reports have one source.
//!
//! So every suite asserts over a TABLE whose rows differ along what the rule
//! generalises over (which filter is applied and with how many values, the page
//! size asked for, which add-on and which build of it, whether the pair is
//! published at
//! all), not over one recorded request. Nothing here hard-codes the catalog's
//! contents: the add-ons, versions and Kubernetes versions each case uses are
//! read out of the catalog itself (`DescribeAddonVersions`) or out of the
//! published version calendar (`DescribeClusterVersions`), so the suites keep
//! asserting the rule as the published tables grow.

use std::collections::BTreeMap;
use std::time::Duration;

use service_com_amazonaws_eks_awswesleyfrontend::state::{
    self, addon_schema_not_found_message, ADDON_ANY_PLATFORM_VERSION, ADDON_OWNER_AWS,
    ADDON_OWNER_COMMUNITY, ADDON_OWNER_MARKETPLACE, ADDON_PUBLISHER_EKS, CLUSTER_ACTIVATION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::addon_catalog_publication::schema_root;
use crate::cluster_lifecycle::REGION;
use crate::marketplace_listing_reference_data::{
    is_marketplace_product_id, marketplace_listing_path,
};
use crate::wire::{call, emulator, encode_label, error_code, Answer};

/// `GET /addons/supported-versions`, with an optional query string.
pub(crate) async fn describe_addon_versions(router: &MultiServiceRouter, query: &str) -> Answer {
    let uri = if query.is_empty() {
        "/addons/supported-versions".to_string()
    } else {
        format!("/addons/supported-versions?{query}")
    };
    call(router, REGION, "GET", &uri, "").await
}

/// `GET /addons/configuration-schemas`, for one `(addonName, addonVersion)` pair.
pub(crate) async fn describe_addon_configuration(
    router: &MultiServiceRouter,
    addon_name: &str,
    addon_version: &str,
) -> Answer {
    let uri = format!(
        "/addons/configuration-schemas?addonName={}&addonVersion={}",
        encode_label(addon_name),
        encode_label(addon_version)
    );
    call(router, REGION, "GET", &uri, "").await
}

/// The `addons` array of a successful answer.
pub(crate) fn addons(answer: &Answer) -> Vec<serde_json::Value> {
    answer.body["addons"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addons list, got {:?}", answer.body))
        .clone()
}

/// The `addonName` of every entry of an answer, in the order reported.
pub(crate) fn addon_names(answer: &Answer) -> Vec<String> {
    addons(answer)
        .iter()
        .map(|addon| string_member(addon, "addonName"))
        .collect()
}

/// A string member of a reported object, which every entry the catalog publishes
/// carries.
pub(crate) fn string_member(value: &serde_json::Value, member: &str) -> String {
    value[member]
        .as_str()
        .unwrap_or_else(|| panic!("expected a {member} string, got {value:?}"))
        .to_string()
}

/// The `addonVersions` array of one reported add-on.
pub(crate) fn versions_of(addon: &serde_json::Value) -> Vec<serde_json::Value> {
    addon["addonVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected an addonVersions list, got {addon:?}"))
        .clone()
}

/// The `compatibilities` array of one reported build.
pub(crate) fn compatibilities_of(version: &serde_json::Value) -> Vec<serde_json::Value> {
    version["compatibilities"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a compatibilities list, got {version:?}"))
        .clone()
}

/// An add-on version `"v1.19.0-eksbuild.2"` as the tuple `(1, 19, 0, 2)`, so two
/// builds are ordered numerically rather than lexically.
///
/// The form is the `vX.Y.Z-eksbuild.N` one every Amazon EKS add-on version takes.
pub(crate) fn version_key(version: &str) -> (u32, u32, u32, u32) {
    let (semantic, build) = version
        .strip_prefix('v')
        .and_then(|rest| rest.split_once("-eksbuild."))
        .unwrap_or_else(|| panic!("expected a vX.Y.Z-eksbuild.N version, got {version:?}"));
    let mut parts = semantic.split('.').map(|part| {
        part.parse::<u32>()
            .unwrap_or_else(|_| panic!("expected a numeric version part in {version:?}"))
    });
    let major = parts.next().unwrap_or_else(|| panic!("major of {version:?}"));
    let minor = parts.next().unwrap_or_else(|| panic!("minor of {version:?}"));
    let patch = parts.next().unwrap_or_else(|| panic!("patch of {version:?}"));
    let build = build
        .parse::<u32>()
        .unwrap_or_else(|_| panic!("expected a numeric eksbuild in {version:?}"));
    (major, minor, patch, build)
}

/// The Kubernetes versions `DescribeClusterVersions` publishes, newest first.
///
/// `include_all` widens the listing to the versions Amazon EKS no longer
/// supports, which the calendar still publishes. The add-on catalog's own window
/// is NARROWER than the calendar — the oldest rows carry no published add-on
/// build any more — so which versions have add-ons is read from the add-on
/// catalog itself rather than assumed from this list.
async fn calendar_cluster_versions(router: &MultiServiceRouter, include_all: bool) -> Vec<String> {
    let query = if include_all {
        "/cluster-versions?maxResults=100&includeAll=true"
    } else {
        "/cluster-versions?maxResults=100"
    };
    let answer = call(router, REGION, "GET", query, "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions: expected HTTP 200, got {answer:?}"
    );
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .iter()
        .map(|row| string_member(row, "clusterVersion"))
        .collect()
}

/// The Kubernetes versions a cluster can be CREATED on, newest first: the
/// published rows the clock has not carried past their own end of extended
/// support. The listing publishes the whole calendar, so the state on each row is
/// what says which of its versions `CreateCluster` accepts.
async fn creatable_cluster_versions(router: &MultiServiceRouter) -> Vec<String> {
    let answer = call(router, REGION, "GET", "/cluster-versions?maxResults=100", "").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeClusterVersions: expected HTTP 200, got {answer:?}"
    );
    answer.body["clusterVersions"]
        .as_array()
        .unwrap_or_else(|| panic!("expected a clusterVersions list, got {:?}", answer.body))
        .iter()
        .filter(|row| row["versionStatus"].as_str() != Some("UNSUPPORTED"))
        .map(|row| string_member(row, "clusterVersion"))
        .collect()
}

/// The whole catalog, in one page.
pub(crate) async fn whole_catalog(router: &MultiServiceRouter) -> Answer {
    let answer = describe_addon_versions(router, "maxResults=100").await;
    assert_eq!(
        answer.status, 200,
        "setup DescribeAddonVersions: expected HTTP 200, got {answer:?}"
    );
    answer
}

/// The smallest request `CreateCluster` accepts, on `version`.
fn cluster_request(name: &str, version: &str) -> serde_json::Value {
    serde_json::json!({
        "name": name,
        "version": version,
        "roleArn": "arn:aws:iam::000000000000:role/eksClusterRole",
        "resourcesVpcConfig": { "subnetIds": ["subnet-aaaa1111", "subnet-bbbb2222"] },
    })
}

/// One cluster on `version`, waited into `ACTIVE` so the add-on layer that
/// requires an active cluster is past.
pub(crate) async fn active_cluster(router: &MultiServiceRouter, name: &str, version: &str) {
    let answer = call(
        router,
        REGION,
        "POST",
        "/clusters",
        &cluster_request(name, version).to_string(),
    )
    .await;
    assert_eq!(
        answer.status, 200,
        "setup CreateCluster {name:?} on {version:?}: got {answer:?}"
    );
    tokio::time::sleep(CLUSTER_ACTIVATION_DELAY + Duration::from_millis(250)).await;
}

// ---------------------------------------------------------------------------
// RULE: the listing reports the published catalog from state — each add-on with
// its own identity and every build of it NEWEST FIRST, each build with the
// Kubernetes versions it supports, and exactly ONE default build per add-on per
// Kubernetes version.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_published_catalog_is_reported_from_state() {
    let router = emulator();

    let answer = whole_catalog(&router).await;
    let listed = addons(&answer);
    assert!(
        !listed.is_empty(),
        "case plain catalog query: the catalog must publish at least one add-on, got {:?}",
        answer.body
    );
    assert!(
        answer
            .body
            .get("nextToken")
            .is_none_or(serde_json::Value::is_null),
        "case plain catalog query: the whole catalog fits one page of 100, got nextToken {:?}",
        answer.body.get("nextToken")
    );

    let published_versions = calendar_cluster_versions(&router, true).await;
    // Whether the catalog publishes a build that cannot be installed without
    // configuration values, and one whose configuration is optional: the flag is a
    // property of the add-on, so a catalog that publishes only one of the two does
    // not exercise it.
    let mut configuring = false;
    let mut optional = false;

    for addon in &listed {
        let case = string_member(addon, "addonName");
        assert!(
            state::is_addon_name_shaped(&case),
            "case {case:?}: a published add-on name must be one CreateAddon accepts"
        );
        assert!(
            !string_member(addon, "type").is_empty(),
            "case {case:?}: expected a non-empty type, got {:?}",
            addon["type"]
        );
        assert!(
            [
                ADDON_OWNER_AWS,
                ADDON_OWNER_COMMUNITY,
                ADDON_OWNER_MARKETPLACE
            ]
            .iter()
            .any(|owner| addon["owner"].as_str() == Some(*owner)),
            "case {case:?}: an add-on is owned by {ADDON_OWNER_AWS:?}, \
             {ADDON_OWNER_COMMUNITY:?} or {ADDON_OWNER_MARKETPLACE:?}, got {:?}",
            addon["owner"]
        );
        assert!(
            !string_member(addon, "publisher").is_empty(),
            "case {case:?}: expected the add-on's publisher, got {:?}",
            addon["publisher"]
        );
        let listing = addon
            .get("marketplaceInformation")
            .filter(|listing| !listing.is_null());
        if addon["owner"].as_str() == Some(ADDON_OWNER_MARKETPLACE) {
            let listing = listing.unwrap_or_else(|| {
                panic!("case {case:?}: a Marketplace add-on reports its listing, got {addon:?}")
            });
            assert!(
                is_marketplace_product_id(&string_member(listing, "productId")),
                "case {case:?}: expected the listing's Marketplace product id, got {listing:?}"
            );
            assert!(
                marketplace_listing_path(&string_member(listing, "productUrl")).is_some(),
                "case {case:?}: expected the listing's Marketplace URL, got {listing:?}"
            );
        } else {
            // Amazon EKS builds and publishes the add-ons it manages and the
            // community ones it curates alike, and neither is sold through AWS
            // Marketplace.
            assert_eq!(
                addon["publisher"].as_str(),
                Some(ADDON_PUBLISHER_EKS),
                "case {case:?}: an add-on Amazon EKS publishes is published by \
                 {ADDON_PUBLISHER_EKS:?}, got {:?}",
                addon["publisher"]
            );
            assert!(
                listing.is_none(),
                "case {case:?}: an add-on Amazon EKS publishes has no Marketplace listing, got \
                 {listing:?}"
            );
        }
        assert!(
            !string_member(addon, "defaultNamespace").is_empty(),
            "case {case:?}: expected the namespace the add-on installs into, got {:?}",
            addon["defaultNamespace"]
        );

        let builds = versions_of(addon);
        assert!(
            !builds.is_empty(),
            "case {case:?}: a published add-on reports at least one build, got {addon:?}"
        );

        // Newest build first: the order the catalog publishes versions in.
        let keys: Vec<(u32, u32, u32, u32)> = builds
            .iter()
            .map(|build| version_key(&string_member(build, "addonVersion")))
            .collect();
        assert!(
            keys.windows(2).all(|pair| pair[0] > pair[1]),
            "case {case:?}: expected the newest build first, got {:?}",
            builds
                .iter()
                .map(|build| string_member(build, "addonVersion"))
                .collect::<Vec<_>>()
        );

        // One default build per Kubernetes version, over every build of the add-on.
        let mut defaults: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for build in &builds {
            let build_case = format!("{case}/{}", string_member(build, "addonVersion"));
            let architecture = build["architecture"]
                .as_array()
                .unwrap_or_else(|| panic!("expected an architecture list, got {build:?}"));
            assert!(
                !architecture.is_empty(),
                "case {build_case}: expected the architectures the build supports, got {build:?}"
            );
            assert!(
                build["requiresConfiguration"].as_bool().is_some(),
                "case {build_case}: expected the requiresConfiguration flag, got {build:?}"
            );
            if build["requiresConfiguration"].as_bool() == Some(true) {
                configuring = true;
            } else {
                optional = true;
            }
            assert!(
                build["requiresIamPermissions"].as_bool().is_some(),
                "case {build_case}: expected the requiresIamPermissions flag, got {build:?}"
            );

            let compatibilities = compatibilities_of(build);
            assert!(
                !compatibilities.is_empty(),
                "case {build_case}: a published build supports at least one Kubernetes \
                 version, got {build:?}"
            );
            for compatibility in &compatibilities {
                let cluster_version = string_member(compatibility, "clusterVersion");
                assert!(
                    published_versions.contains(&cluster_version),
                    "case {build_case}: a build is published for a Kubernetes version the \
                     calendar publishes, got {cluster_version:?} against {published_versions:?}"
                );
                assert_eq!(
                    compatibility["platformVersions"],
                    serde_json::json!([ADDON_ANY_PLATFORM_VERSION]),
                    "case {build_case}: expected the platform versions the build runs on, \
                     got {:?}",
                    compatibility["platformVersions"]
                );
                if compatibility["defaultVersion"].as_bool() == Some(true) {
                    defaults
                        .entry(cluster_version)
                        .or_default()
                        .push(string_member(build, "addonVersion"));
                }
            }
        }

        for (cluster_version, defaulted) in &defaults {
            assert_eq!(
                defaulted.len(),
                1,
                "case {case}/{cluster_version}: expected exactly one default build for a \
                 Kubernetes version, got {defaulted:?}"
            );
        }
        assert!(
            !defaults.is_empty(),
            "case {case}: an add-on must have a default build for some Kubernetes version, \
             got {addon:?}"
        );
    }

    // The catalog has a publication ORDER, which is the order the listing reports
    // it in.
    let names = addon_names(&answer);
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(
        names, sorted,
        "case ordering: the catalog is published in add-on name order, got {names:?}"
    );
    assert!(
        configuring && optional,
        "case requiresConfiguration: the catalog must publish both a build that requires \
         configuration values and one whose configuration is optional, got requiring \
         {configuring} and optional {optional}"
    );
}

// ---------------------------------------------------------------------------
// RULE: each filter keeps only the catalog entries that satisfy it — several
// values in one member widen it, the members combine, a value nothing matches
// gives an EMPTY listing, and `kubernetesVersion` also narrows each kept build's
// reported compatibilities to that version.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn filters_narrow_the_listing() {
    let router = emulator();
    let whole = whole_catalog(&router).await;
    let catalog = addons(&whole);
    let every_name = addon_names(&whole);

    // The rows are built from the catalog itself, so they span the filters rather
    // than one recorded request: a member matching one entry, matching several,
    // matching nothing, and two members at once.
    let first = string_member(&catalog[0], "addonName");
    let last = string_member(&catalog[catalog.len() - 1], "addonName");
    let first_type = string_member(&catalog[0], "type");
    let of_first_type: Vec<String> = catalog
        .iter()
        .filter(|addon| string_member(addon, "type") == first_type)
        .map(|addon| string_member(addon, "addonName"))
        .collect();
    let named_by = |member: &str, value: &str| -> Vec<String> {
        catalog
            .iter()
            .filter(|addon| string_member(addon, member) == value)
            .map(|addon| string_member(addon, "addonName"))
            .collect()
    };
    let aws_owned = named_by("owner", ADDON_OWNER_AWS);
    let community_owned = named_by("owner", ADDON_OWNER_COMMUNITY);
    let marketplace_owned = named_by("owner", ADDON_OWNER_MARKETPLACE);
    let published_by_eks = named_by("publisher", ADDON_PUBLISHER_EKS);
    // The owner of an add-on is a property of the add-on, so the catalog carries
    // each kind and the owner filter is a real narrowing rather than a no-op.
    assert!(
        !aws_owned.is_empty() && !community_owned.is_empty() && !marketplace_owned.is_empty(),
        "the catalog must publish Amazon EKS-managed, community and Marketplace add-ons, got \
         {aws_owned:?}, {community_owned:?} and {marketplace_owned:?}"
    );
    let a_vendor = string_member(
        catalog
            .iter()
            .find(|addon| string_member(addon, "owner") == ADDON_OWNER_MARKETPLACE)
            .expect("a Marketplace add-on"),
        "publisher",
    );
    let published_by_vendor = named_by("publisher", &a_vendor);
    // An add-on the Marketplace owner filter does NOT keep, so the two-member row
    // below is satisfied by a different add-on in each member whatever order the
    // catalog publishes its rows in.
    let an_eks_managed_addon = aws_owned
        .first()
        .cloned()
        .expect("an Amazon EKS-managed add-on");

    let cases: Vec<(&str, String, Vec<String>)> = vec![
        (
            "addonName naming one published add-on",
            format!("addonName={first}"),
            vec![first.clone()],
        ),
        (
            "addonName naming the last add-on the catalog publishes",
            format!("addonName={last}"),
            vec![last.clone()],
        ),
        (
            "addonName naming an add-on the catalog does not publish",
            "addonName=not-a-published-addon".to_string(),
            Vec::new(),
        ),
        (
            "types naming one published category",
            format!("types={first_type}"),
            of_first_type.clone(),
        ),
        (
            "types naming a category nothing is published under",
            "types=not-a-category".to_string(),
            Vec::new(),
        ),
        (
            "owners naming the owner of the EKS-managed add-ons",
            format!("owners={ADDON_OWNER_AWS}"),
            aws_owned.clone(),
        ),
        (
            "owners naming the Marketplace owner",
            format!("owners={ADDON_OWNER_MARKETPLACE}"),
            marketplace_owned.clone(),
        ),
        (
            "owners naming the community owner",
            format!("owners={ADDON_OWNER_COMMUNITY}"),
            community_owned.clone(),
        ),
        (
            "owners naming an owner nothing is published by",
            "owners=not-an-owner".to_string(),
            Vec::new(),
        ),
        (
            "publishers naming the publisher of the EKS-managed add-ons",
            format!("publishers={ADDON_PUBLISHER_EKS}"),
            published_by_eks.clone(),
        ),
        (
            "publishers naming one Marketplace vendor",
            // A seller's published name may carry a space, which travels
            // percent-encoded in the query.
            format!("publishers={}", a_vendor.replace(' ', "%20")),
            published_by_vendor.clone(),
        ),
        (
            "publishers naming a publisher nothing is published by",
            "publishers=not-a-publisher".to_string(),
            Vec::new(),
        ),
        (
            "two values in one member widen it",
            format!("owners={ADDON_OWNER_AWS}&owners={ADDON_OWNER_MARKETPLACE}"),
            every_name
                .iter()
                .filter(|name| !community_owned.contains(name))
                .cloned()
                .collect(),
        ),
        (
            "every owner the catalog publishes widens to the whole catalog",
            format!(
                "owners={ADDON_OWNER_AWS}&owners={ADDON_OWNER_COMMUNITY}&\
                 owners={ADDON_OWNER_MARKETPLACE}"
            ),
            every_name.clone(),
        ),
        (
            "two members combine, both satisfied",
            format!("addonName={first}&types={first_type}"),
            vec![first.clone()],
        ),
        (
            "two members combine, the second satisfied by nothing",
            format!("addonName={first}&types=not-a-category"),
            Vec::new(),
        ),
        (
            "two members combine, each satisfied by a different add-on",
            format!("addonName={an_eks_managed_addon}&owners={ADDON_OWNER_MARKETPLACE}"),
            Vec::new(),
        ),
    ];

    for (case, query, expected) in cases {
        let answer = describe_addon_versions(&router, &format!("{query}&maxResults=100")).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            addon_names(&answer),
            expected,
            "case {case}: expected exactly the add-ons the filter keeps, got {:?}",
            answer.body["addons"]
        );
    }

    // `kubernetesVersion` keeps the builds that support that version, and reports
    // each kept build's compatibility for that version alone. Every published
    // Kubernetes version is a row, plus one the calendar does not publish.
    //
    // Which versions have builds at all comes from the CATALOG — the cluster
    // versions its published builds name as compatible — so a calendar row older
    // than the published add-on build lines is expected to keep nothing rather
    // than assumed to keep something.
    let with_builds: std::collections::BTreeSet<String> = catalog
        .iter()
        .flat_map(versions_of)
        .flat_map(|build| compatibilities_of(&build))
        .map(|compatibility| string_member(&compatibility, "clusterVersion"))
        .collect();
    let published_versions = calendar_cluster_versions(&router, true).await;
    for cluster_version in &published_versions {
        let answer = describe_addon_versions(
            &router,
            &format!("kubernetesVersion={cluster_version}&maxResults=100"),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case kubernetesVersion {cluster_version}: expected HTTP 200, got {answer:?}"
        );
        let narrowed = addons(&answer);
        if !with_builds.contains(cluster_version) {
            assert!(
                narrowed.is_empty(),
                "case kubernetesVersion {cluster_version}: no published build names this \
                 version, so the listing keeps nothing, got {:?}",
                answer.body["addons"]
            );
            continue;
        }
        assert!(
            !narrowed.is_empty(),
            "case kubernetesVersion {cluster_version}: a Kubernetes version the catalog \
             publishes builds for has add-ons, got {:?}",
            answer.body
        );
        for addon in &narrowed {
            let case = format!("{}/{cluster_version}", string_member(addon, "addonName"));
            let builds = versions_of(addon);
            let mut defaults: Vec<String> = Vec::new();
            for build in &builds {
                let reported: Vec<String> = compatibilities_of(build)
                    .iter()
                    .map(|compatibility| string_member(compatibility, "clusterVersion"))
                    .collect();
                assert_eq!(
                    reported,
                    vec![cluster_version.clone()],
                    "case {case}: a listing filtered to one Kubernetes version reports that \
                     version's compatibility alone, got {reported:?}"
                );
                if compatibilities_of(build)
                    .iter()
                    .any(|compatibility| compatibility["defaultVersion"].as_bool() == Some(true))
                {
                    defaults.push(string_member(build, "addonVersion"));
                }
            }
            assert_eq!(
                defaults.len(),
                1,
                "case {case}: expected exactly one default build for the Kubernetes version \
                 asked for, got {defaults:?}"
            );
        }
    }

    for unpublished in ["1.10", "0.1", "9.99"] {
        let answer =
            describe_addon_versions(&router, &format!("kubernetesVersion={unpublished}")).await;
        assert_eq!(
            answer.status, 200,
            "case kubernetesVersion {unpublished}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            addon_names(&answer),
            Vec::<String>::new(),
            "case kubernetesVersion {unpublished}: a Kubernetes version the calendar does not \
             publish has no add-ons, got {:?}",
            answer.body["addons"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: the catalog is served WHOLE — every add-on that matches the filters in
// one answer with no `nextToken`, whatever `maxResults` and `nextToken` say.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_catalog_is_served_whole_whatever_the_page_size() {
    let router = emulator();
    let whole = addon_names(&whole_catalog(&router).await);

    // The rows span the page sizes a caller may ask for, from the documented
    // minimum to the documented maximum: none of them truncates the catalog or
    // offers a token.
    for page_size in [1usize, 2, 3, 5, 100] {
        let answer = describe_addon_versions(&router, &format!("maxResults={page_size}")).await;
        assert_eq!(
            answer.status, 200,
            "case page size {page_size}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            addon_names(&answer),
            whole,
            "case page size {page_size}: expected the whole catalog, got {:?}",
            answer.body["addons"]
        );
        assert!(
            answer
                .body
                .get("nextToken")
                .is_none_or(serde_json::Value::is_null),
            "case page size {page_size}: a service that serves the catalog whole offers no \
             nextToken, got {:?}",
            answer.body.get("nextToken")
        );
    }

    // And with no `maxResults` at all.
    let unbounded = describe_addon_versions(&router, "").await;
    assert_eq!(
        addon_names(&unbounded),
        whole,
        "case no maxResults: expected the whole catalog, got {:?}",
        unbounded.body["addons"]
    );
    assert!(
        unbounded
            .body
            .get("nextToken")
            .is_none_or(serde_json::Value::is_null),
        "case no maxResults: expected no nextToken, got {:?}",
        unbounded.body.get("nextToken")
    );

    // A `nextToken` names a position to resume from, and this listing has no page
    // after the first: the recorded service answers a call carrying one with the
    // whole catalog, so a token changes nothing — whatever it is.
    let ignored = [
        (
            "a token this service never minted",
            "nextToken=not-a-token".to_string(),
        ),
        (
            "a hex string that is not a token",
            "nextToken=deadbeefdeadbeef".to_string(),
        ),
        (
            "a well-formed token minted by another listing",
            format!("nextToken={}", state::encode_page_token(&whole[0])),
        ),
        (
            "a token alongside a page size",
            format!(
                "nextToken={}&maxResults=1",
                state::encode_page_token(&whole[0])
            ),
        ),
    ];
    for (case, query) in ignored {
        let answer = describe_addon_versions(&router, &query).await;
        assert_eq!(
            answer.status, 200,
            "case {case}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            addon_names(&answer),
            whole,
            "case {case}: expected the whole catalog, got {:?}",
            answer.body["addons"]
        );
        assert!(
            answer
                .body
                .get("nextToken")
                .is_none_or(serde_json::Value::is_null),
            "case {case}: expected no nextToken, got {:?}",
            answer.body.get("nextToken")
        );
    }

    // A filter still narrows a call that carries a token.
    let filtered = describe_addon_versions(
        &router,
        &format!(
            "nextToken={}&owners={ADDON_OWNER_MARKETPLACE}",
            state::encode_page_token(&whole[0])
        ),
    )
    .await;
    assert_eq!(
        filtered.status, 200,
        "case a token alongside a filter: expected HTTP 200, got {filtered:?}"
    );
    assert!(
        !addons(&filtered).is_empty() && addon_names(&filtered).len() < whole.len(),
        "case a token alongside a filter: expected the filter to narrow the catalog, got {:?}",
        addon_names(&filtered)
    );

    // The real service does not enforce the 1..100 bound its documentation states
    // for `maxResults`: the recorded corpus has it serving a value past that
    // maximum, and ignoring it. So every value is accepted and none of them
    // truncates the answer — the rows span the documented bounds, one past each,
    // and a value no page size could be.
    for (case, value) in [
        ("the documented minimum", "1"),
        ("the documented maximum", "100"),
        ("below the documented minimum", "0"),
        ("above the documented maximum", "101"),
        ("negative", "-1"),
    ] {
        let answer = describe_addon_versions(&router, &format!("maxResults={value}")).await;
        assert_eq!(
            answer.status, 200,
            "case maxResults {case}: expected HTTP 200, got {answer:?}"
        );
        assert_eq!(
            addon_names(&answer),
            whole,
            "case maxResults {case}: expected the whole catalog, got {:?}",
            answer.body["addons"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `DescribeAddonConfiguration` resolves one published `(addonName,
// addonVersion)` pair of the same catalog: it answers the schema and the pod
// identity configuration the catalog holds for it — the schema requiring a
// property exactly when the build's `requiresConfiguration` says so — and
// refuses a pair the catalog does not publish.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn the_configuration_schema_matches_the_catalog_flag() {
    let router = emulator();
    let catalog = addons(&whole_catalog(&router).await);

    // Every published add-on is a row, and for each one the newest and the oldest
    // build it publishes — the rows differ along the add-on and the build, which is
    // what the pair the operation resolves generalises over.
    for addon in &catalog {
        let addon_name = string_member(addon, "addonName");
        let builds = versions_of(addon);
        let newest = builds.first().expect("a published build");
        let oldest = builds.last().expect("a published build");
        for build in [newest, oldest] {
            let addon_version = string_member(build, "addonVersion");
            let case = format!("{addon_name}/{addon_version}");
            let answer = describe_addon_configuration(&router, &addon_name, &addon_version).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {answer:?}"
            );
            assert_eq!(
                answer.body["addonName"].as_str(),
                Some(addon_name.as_str()),
                "case {case}: expected the add-on asked for, got {:?}",
                answer.body["addonName"]
            );
            assert_eq!(
                answer.body["addonVersion"].as_str(),
                Some(addon_version.as_str()),
                "case {case}: expected the version asked for, got {:?}",
                answer.body["addonVersion"]
            );

            let schema: serde_json::Value = serde_json::from_str(&string_member(
                &answer.body,
                "configurationSchema",
            ))
            .unwrap_or_else(|error| {
                panic!("case {case}: configurationSchema must be JSON: {error}")
            });
            assert!(
                schema["$schema"].as_str().is_some_and(|dialect| dialect
                    .contains("json-schema.org")),
                "case {case}: expected a JSON Schema dialect, got {:?}",
                schema["$schema"]
            );
            // The published document is a reference into its own definitions, and
            // the type it names is the add-on's chart values object.
            let root = schema_root(&schema);
            assert_eq!(
                root["type"].as_str(),
                Some("object"),
                "case {case}: an add-on configuration is an object, got {:?}",
                root["type"]
            );
            assert_eq!(
                root["properties"]["namespace"]["default"].as_str(),
                addon["defaultNamespace"].as_str(),
                "case {case}: the schema's namespace default is the add-on's own namespace, \
                 got {:?}",
                root["properties"]["namespace"]
            );
            let required = root["required"]
                .as_array()
                .unwrap_or_else(|| panic!("case {case}: expected a required list, got {schema:?}"));
            assert_eq!(
                !required.is_empty(),
                build["requiresConfiguration"].as_bool() == Some(true),
                "case {case}: the schema requires a property exactly when the catalog's \
                 requiresConfiguration says so, got required {required:?} against {:?}",
                build["requiresConfiguration"]
            );

            // The pod identity configuration is the service accounts the add-on runs
            // as; an add-on that runs as none reports an empty list, not a missing one.
            let pod_identity = answer.body["podIdentityConfiguration"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: expected a podIdentityConfiguration list, got {:?}",
                        answer.body
                    )
                });
            for entry in pod_identity {
                assert!(
                    !string_member(entry, "serviceAccount").is_empty(),
                    "case {case}: expected the service account the add-on runs as, got {entry:?}"
                );
                let policies = entry["recommendedManagedPolicies"]
                    .as_array()
                    .unwrap_or_else(|| {
                        panic!("case {case}: expected a policy list, got {entry:?}")
                    });
                for policy in policies {
                    let arn = policy.as_str().unwrap_or_default();
                    assert!(
                        arn.starts_with("arn:aws:iam::aws:policy/"),
                        "case {case}: a recommended policy is an AWS managed policy ARN, got \
                         {arn:?}"
                    );
                }
            }
        }
    }

    // A pair the catalog publishes no build for is refused the same way whichever
    // half of it misses: the pair names ONE published build, so an add-on the
    // catalog does not carry and a version it has no build of are the same miss.
    let published_name = string_member(&catalog[0], "addonName");
    let published_version = string_member(&versions_of(&catalog[0])[0], "addonVersion");
    let other_name = string_member(&catalog[1], "addonName");
    for (case, addon_name, addon_version) in [
        (
            "an add-on the catalog does not publish",
            "not-a-published-addon",
            published_version.as_str(),
        ),
        (
            "an add-on name that is not a name at all",
            "",
            published_version.as_str(),
        ),
        (
            "a published add-on at a version it has no build for",
            published_name.as_str(),
            "v0.0.0-eksbuild.1",
        ),
        (
            "a published add-on at a version of another form entirely",
            published_name.as_str(),
            "latest",
        ),
        (
            "a published add-on at another published add-on's version",
            other_name.as_str(),
            published_version.as_str(),
        ),
    ] {
        let answer = describe_addon_configuration(&router, addon_name, addon_version).await;
        assert_eq!(
            answer.status, 404,
            "case {case}: expected HTTP 404, got {answer:?}"
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {case}: expected ResourceNotFoundException, got {answer:?}"
        );
        assert_eq!(
            answer.body["message"].as_str(),
            Some(
                addon_schema_not_found_message(addon_name, addon_version)
                    .as_str()
            ),
            "case {case}: expected the sentence naming the pair whose schema was not \
             found, got {:?}",
            answer.body["message"]
        );
    }
}

// ---------------------------------------------------------------------------
// RULE: `CreateAddon` with no `addonVersion` installs the build the catalog
// publishes as the DEFAULT for the cluster's own Kubernetes version, so the
// version an installed add-on reports is one `DescribeAddonVersions` offers.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_created_addon_runs_the_catalog_default_version() {
    let router = emulator();
    let published_versions = creatable_cluster_versions(&router).await;

    // The rows differ along the cluster's Kubernetes version — the newest and the
    // oldest a cluster can be created on — and along the add-on, so the default is
    // read from the catalog rather than from one add-on's version line.
    let cluster_versions = [
        published_versions
            .first()
            .expect("a published Kubernetes version")
            .clone(),
        published_versions
            .last()
            .expect("a published Kubernetes version")
            .clone(),
    ];

    for cluster_version in cluster_versions {
        let listing = describe_addon_versions(
            &router,
            &format!("kubernetesVersion={cluster_version}&maxResults=100"),
        )
        .await;
        assert_eq!(
            listing.status, 200,
            "setup DescribeAddonVersions for {cluster_version}: got {listing:?}"
        );
        let cluster = format!("addons-{}", cluster_version.replace('.', "-"));
        active_cluster(&router, &cluster, &cluster_version).await;

        for addon in addons(&listing) {
            let addon_name = string_member(&addon, "addonName");
            let case = format!("{addon_name}/{cluster_version}");
            let default_version = versions_of(&addon)
                .iter()
                .find(|build| {
                    compatibilities_of(build).iter().any(|compatibility| {
                        compatibility["defaultVersion"].as_bool() == Some(true)
                    })
                })
                .map(|build| string_member(build, "addonVersion"))
                .unwrap_or_else(|| panic!("case {case}: the catalog publishes a default build"));

            let created = call(
                &router,
                REGION,
                "POST",
                &format!("/clusters/{}/addons", encode_label(&cluster)),
                &serde_json::json!({ "addonName": addon_name }).to_string(),
            )
            .await;
            assert_eq!(
                created.status, 200,
                "case {case}: expected HTTP 200 from CreateAddon, got {created:?}"
            );
            assert_eq!(
                created.body["addon"]["addonVersion"].as_str(),
                Some(default_version.as_str()),
                "case {case}: an add-on created without a version runs the catalog's default \
                 build, got {:?}",
                created.body["addon"]["addonVersion"]
            );

            // Read back through the implemented reader, so the catalog and the
            // installed record are proven to agree rather than assumed to.
            let described = call(
                &router,
                REGION,
                "GET",
                &format!(
                    "/clusters/{}/addons/{}",
                    encode_label(&cluster),
                    encode_label(&addon_name)
                ),
                "",
            )
            .await;
            assert_eq!(
                described.status, 200,
                "case {case}: expected HTTP 200 from DescribeAddon, got {described:?}"
            );
            assert_eq!(
                described.body["addon"]["addonVersion"].as_str(),
                Some(default_version.as_str()),
                "case {case}: the installed version reads back unchanged, got {:?}",
                described.body["addon"]["addonVersion"]
            );

            // And that version is one the catalog publishes a configuration for.
            let configuration =
                describe_addon_configuration(&router, &addon_name, &default_version).await;
            assert_eq!(
                configuration.status, 200,
                "case {case}: the installed version is one the catalog publishes, got \
                 {configuration:?}"
            );
        }
    }
}
