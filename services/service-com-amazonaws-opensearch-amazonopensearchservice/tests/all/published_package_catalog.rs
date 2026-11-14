// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the plugin-package catalog the SERVICE publishes.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test: a package read reports every package AVAILABLE to the account —
//! the plugin packages the service publishes beside the ones the account created — and
//! each of them is resolvable by its id through every package read. So:
//!
//! * `DescribePackages` reports the published catalog, every entry projecting the
//!   `PackageDetails` members a plugin package has: a `pkg-` id, its name, type
//!   `ZIP-PLUGIN`, status `AVAILABLE`, the engine release it is built for, its
//!   available version, its plugin properties and its package configuration;
//! * the catalog spans the class it publishes: both engine lines, packages with one
//!   version and with several, licensed and unlicensed plugins, and one plugin
//!   published for several releases — whose packages are distinct resources with
//!   distinct ids and distinct instants;
//! * `GetPackageVersionHistory` answers EVERY published id with that package's history,
//!   newest version first, one entry per version, and the listing's `CreatedAt`,
//!   `LastUpdatedAt` and `AvailablePackageVersion` agree with it;
//! * the published catalog and the account's own packages are ONE set: a created
//!   package is reported beside the published ones, every filter selects across both,
//!   and an id in neither set is still refused;
//! * a published package can be put ON a domain and read back through the domain's own
//!   listing;
//! * reading the catalog writes nothing: it is reference data, so no read of it leaves
//!   a package record in the store.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::state::{
    bare_engine_release, catalog_publishes_published_releases, published_package_id,
    published_packages, DOMAIN_PACKAGE_STATUS_ACTIVE, PACKAGE_STATUS_AVAILABLE,
    PACKAGE_TYPE_ZIP_PLUGIN, REQUIREMENT_LEVEL_NONE, REQUIREMENT_LEVEL_REQUIRED,
};

use super::domain_lifecycle::{
    create_baseline, error_code, poll_until, string_member, Answer, Emulator, PREFIX,
};
use super::package_associations::{create_package, PACKAGE_TYPES};

/// The domain the association cases put a published package on.
const DOMAIN: &str = "published-package-domain";

/// `DescribePackages`: `POST /2021-01-01/packages/describe`.
async fn describe(emulator: &Emulator, body: Value) -> Answer {
    emulator
        .call("POST", &format!("{PREFIX}/packages/describe"), Some(body))
        .await
}

/// `GetPackageVersionHistory`: `GET /2021-01-01/packages/{PackageID}/history`.
async fn history(emulator: &Emulator, package_id: &str, query: &str) -> Answer {
    let suffix = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };
    emulator
        .call(
            "GET",
            &format!("{PREFIX}/packages/{package_id}/history{suffix}"),
            None,
        )
        .await
}

/// The `PackageDetailsList` of an answered listing, with the case named on failure.
fn reported(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageDetailsList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageDetailsList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The whole listing, walked through the continuation tokens the operation hands out,
/// so a catalog larger than one page is still read completely.
async fn whole_listing(emulator: &Emulator, case: &str) -> Vec<Value> {
    let mut collected: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut body = json!({});
        if let Some(token) = token.as_deref() {
            body["NextToken"] = json!(token);
        }
        let answer = describe(emulator, body).await;
        collected.extend(reported(&answer, case));
        match answer.body["NextToken"].as_str() {
            None => return collected,
            Some(next) => token = Some(next.to_owned()),
        }
    }
}

/// The entries of a listing that are packages the SERVICE publishes: a published
/// package is owned by its publisher, while a package the account created is owned by
/// the account.
fn service_published(listing: &[Value]) -> Vec<Value> {
    listing
        .iter()
        .filter(|package| {
            package["PackageOwner"].as_str() != Some(::engine::emulator_account_id())
        })
        .cloned()
        .collect()
}

/// The `PackageVersionHistoryList` of an answered history read.
fn version_entries(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} with {:?}",
        answer.status, answer.body
    );
    answer.body["PackageVersionHistoryList"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected a PackageVersionHistoryList array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// A timestamp member of a reported record, as epoch seconds.
fn instant(record: &Value, member: &str, case: &str) -> f64 {
    record[member]
        .as_f64()
        .unwrap_or_else(|| panic!("case {case}: expected a {member} timestamp, got {record:?}"))
}

/// Whether `value` is a `PackageID` of the `pkg-<hex>` form the model's pattern
/// `^([FG][0-9]+)$|^(pkg-[a-f0-9]+)$` admits for a package addressed that way.
fn is_package_id(value: &str) -> bool {
    value.strip_prefix("pkg-").is_some_and(|body| {
        !body.is_empty()
            && body
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    })
}

#[tokio::test]
async fn the_listing_reports_the_published_plugin_catalog() {
    let emulator = Emulator::new();
    let listing = whole_listing(&emulator, "the published catalog").await;
    let published = service_published(&listing);

    assert!(
        !published.is_empty(),
        "case the published catalog: expected the plugin packages the service publishes, got \
         {listing:?}"
    );
    assert_eq!(
        published.len(),
        published_packages().len(),
        "case the published catalog: expected every package of the catalog, got {} entries",
        published.len()
    );

    let mut ids: BTreeSet<String> = BTreeSet::new();
    let mut engine_lines: BTreeSet<&str> = BTreeSet::new();
    let mut version_counts: BTreeSet<usize> = BTreeSet::new();
    let mut license_levels: BTreeSet<String> = BTreeSet::new();
    let mut by_name: BTreeMap<String, Vec<String>> = BTreeMap::new();

    for package in &published {
        let name = string_member(package, "PackageName", "a published package");
        let engine_version = string_member(package, "EngineVersion", &name);
        let case = format!("{name} for {engine_version}");
        let id = string_member(package, "PackageID", &case);

        // The id is the one the catalog derives from what the package IS, so the id a
        // client reads here is the id every other read resolves.
        assert!(
            is_package_id(&id),
            "case {case}: expected a pkg- PackageID, got {id}"
        );
        assert_eq!(
            id,
            published_package_id(&name, &engine_version),
            "case {case}: expected the id derived from the plugin and its release, got {id}"
        );
        assert!(
            ids.insert(id.clone()),
            "case {case}: expected an id of its own, got the already-reported {id}"
        );

        assert_eq!(
            package["PackageType"].as_str(),
            Some(PACKAGE_TYPE_ZIP_PLUGIN),
            "case {case}: a plugin package the service publishes carries the plugin type, got \
             {package:?}"
        );
        assert_eq!(
            package["PackageStatus"].as_str(),
            Some(PACKAGE_STATUS_AVAILABLE),
            "case {case}: a published package is in place, so it is available, got {package:?}"
        );
        assert_ne!(
            package["PackageOwner"].as_str(),
            Some(::engine::emulator_account_id()),
            "case {case}: a published package is owned by its publisher, not by the account, \
             got {package:?}"
        );
        assert!(
            package["PackageDescription"]
                .as_str()
                .is_some_and(|text| !text.is_empty()),
            "case {case}: expected the plugin's description, got {package:?}"
        );
        // "A list of users who are allowed to view and associate the package. This
        // field is only visible to the owner of a package": the account owns none of
        // these, so there is no scope of one to report.
        assert!(
            package["AllowListedUserList"].is_null(),
            "case {case}: expected no allow-list on a package the account does not own, got \
             {package:?}"
        );

        // The plugin's own properties: the service publishes the plugin, so it knows
        // what is inside the archive.
        let properties = &package["AvailablePluginProperties"];
        assert_eq!(
            properties["Name"].as_str(),
            Some(name.as_str()),
            "case {case}: expected the plugin named for the package, got {properties:?}"
        );
        assert_eq!(
            properties["Version"].as_str(),
            Some(bare_engine_release(&engine_version)),
            "case {case}: expected the plugin built for the release the package names, got \
             {properties:?}"
        );
        assert!(
            properties["ClassName"]
                .as_str()
                .is_some_and(|class| class.contains('.')),
            "case {case}: expected the plugin's entry-point class, got {properties:?}"
        );
        assert!(
            properties["UncompressedSizeInBytes"]
                .as_i64()
                .is_some_and(|size| size > 0),
            "case {case}: expected the unpacked size of the plugin, got {properties:?}"
        );

        // The configuration: a license path is reported exactly where a license is
        // required, and installing a plugin replaces the domain's nodes.
        let configuration = &package["AvailablePackageConfiguration"];
        let license_requirement =
            string_member(configuration, "LicenseRequirement", &case);
        assert_eq!(
            configuration["LicenseFilepath"].as_str().is_some(),
            license_requirement == REQUIREMENT_LEVEL_REQUIRED,
            "case {case}: a license path is reported exactly where a license is required, got \
             {configuration:?}"
        );
        assert_eq!(
            configuration["RequiresRestartForConfigurationUpdate"].as_bool(),
            Some(true),
            "case {case}: a plugin is installed by replacing the domain's nodes, got \
             {configuration:?}"
        );
        license_levels.insert(license_requirement);

        // The history of the same package: the listing's instants and available version
        // are that history's, so the two reads cannot disagree.
        let read = history(&emulator, &id, "").await;
        assert_eq!(
            read.body["PackageID"].as_str(),
            Some(id.as_str()),
            "case {case}: expected the requested id echoed, got {:?}",
            read.body
        );
        let versions = version_entries(&read, &case);
        assert!(
            !versions.is_empty(),
            "case {case}: expected the package's version history, got {:?}",
            read.body
        );
        version_counts.insert(versions.len());
        assert_eq!(
            package["AvailablePackageVersion"].as_str(),
            versions[0]["PackageVersion"].as_str(),
            "case {case}: expected the newest version of its history as the available one, got \
             {package:?} and {versions:?}"
        );
        assert!(
            (instant(package, "CreatedAt", &case)
                - instant(&versions[versions.len() - 1], "CreatedAt", &case))
            .abs()
                < f64::EPSILON,
            "case {case}: expected the package created when its first version was published, \
             got {package:?} and {versions:?}"
        );
        assert!(
            (instant(package, "LastUpdatedAt", &case) - instant(&versions[0], "CreatedAt", &case))
                .abs()
                < f64::EPSILON,
            "case {case}: expected the package last updated when its newest version was \
             published, got {package:?} and {versions:?}"
        );

        engine_lines.insert(if engine_version.starts_with("OpenSearch_") {
            "OpenSearch"
        } else {
            "Elasticsearch"
        });
        by_name.entry(name).or_default().push(engine_version);
    }

    // The catalog spans the class it publishes, rather than one corner of it: both
    // engine lines, packages with one version and with several, and both license
    // levels — so a client choosing a plugin has the choices the service offers.
    assert_eq!(
        engine_lines,
        ["Elasticsearch", "OpenSearch"].into(),
        "case the published catalog: expected packages for both engine lines, got \
         {engine_lines:?}"
    );
    assert!(
        version_counts.len() > 1 && version_counts.contains(&1),
        "case the published catalog: expected packages with one version and with several, got \
         {version_counts:?}"
    );
    assert_eq!(
        license_levels,
        [
            REQUIREMENT_LEVEL_NONE.to_string(),
            REQUIREMENT_LEVEL_REQUIRED.to_string()
        ]
        .into(),
        "case the published catalog: expected separately licensed plugins beside unlicensed \
         ones, got {license_levels:?}"
    );

    // A plugin published for several releases is several PACKAGES: one per release,
    // each with its own id, and no release published twice.
    let several: Vec<(&String, &Vec<String>)> = by_name
        .iter()
        .filter(|(_, releases)| releases.len() > 1)
        .collect();
    assert!(
        !several.is_empty(),
        "case the published catalog: expected a plugin published for more than one release, got \
         {by_name:?}"
    );
    for (name, releases) in several {
        let distinct: BTreeSet<&String> = releases.iter().collect();
        assert_eq!(
            distinct.len(),
            releases.len(),
            "case {name}: expected one package per release it is published for, got {releases:?}"
        );
    }
}

/// The names the catalog publishes a plugin under, and the kind of plugin each is.
///
/// Source: the plugin names of the OpenSearch distribution the service publishes
/// packages of, as the crate's published-package catalog
/// (`state::PUBLISHED_PACKAGES`) carries them. The rows span the kinds a client picks
/// between — an analysis plugin, an ingest processor, a field mapper and a separately
/// licensed third-party analyzer — including names no other case of this suite names.
const DISCOVERABLE_PLUGINS: [&str; 4] = [
    "analysis-icu",
    "ingest-attachment",
    "mapper-size",
    "babel-street-match",
];

#[tokio::test]
async fn a_published_plugin_is_discoverable_by_name_and_by_id() {
    let emulator = Emulator::new();

    for name in DISCOVERABLE_PLUGINS {
        let case = format!("DescribePackages(PackageName={name})");
        let listing = describe(
            &emulator,
            json!({ "Filters": [{ "Name": "PackageName", "Value": [name] }] }),
        )
        .await;
        let selected = reported(&listing, &case);
        assert!(
            !selected.is_empty(),
            "case {case}: expected the packages the service publishes for that plugin, got \
             {selected:?}"
        );
        for package in &selected {
            assert_eq!(
                package["PackageName"].as_str(),
                Some(name),
                "case {case}: the filter selects exactly the packages carrying that name, got \
                 {package:?}"
            );

            // The id the listing reported resolves through the by-id filter and through
            // the version history: one package, reachable by every path.
            let id = string_member(package, "PackageID", &case);
            let by_id = describe(
                &emulator,
                json!({ "Filters": [{ "Name": "PackageID", "Value": [id] }] }),
            )
            .await;
            let one = reported(&by_id, &case);
            assert_eq!(
                one,
                vec![package.clone()],
                "case {case}: expected the id to select exactly that package, got {one:?}"
            );
        }
    }

    // An id of the same form that no package of either set carries is still refused,
    // so the catalog resolves the ids it publishes and nothing else.
    let case = "GetPackageVersionHistory(an id the catalog does not publish)";
    let answer = history(&emulator, "pkg-00000000000000000000000000000000", "").await;
    assert_eq!(
        answer.status, 400,
        "case {case}: expected HTTP 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(&answer),
        "ValidationException",
        "case {case}: expected the modelled validation refusal, got {:?}",
        answer.body
    );
}

#[tokio::test]
async fn a_version_history_is_reported_newest_first_and_pages() {
    let emulator = Emulator::new();

    // The plugin of the catalog with the most versions, so the walk has more than one
    // page to cover; drawn from the catalog rather than named here.
    let catalog = published_packages();
    let most = catalog
        .iter()
        .max_by_key(|package| package.versions.len())
        .expect("the service publishes a package catalog");
    let case = format!("{} for {:?}", most.package_name, most.engine_version);
    assert!(
        most.versions.len() > 1,
        "case {case}: expected a package with several versions in the catalog, got {} version(s)",
        most.versions.len()
    );

    let whole = version_entries(&history(&emulator, &most.package_id, "").await, &case);
    assert_eq!(
        whole.len(),
        most.versions.len(),
        "case {case}: expected one entry per published version, got {whole:?}"
    );
    for pair in whole.windows(2) {
        assert!(
            instant(&pair[0], "CreatedAt", &case) > instant(&pair[1], "CreatedAt", &case),
            "case {case}: expected the history newest version first, got {pair:?}"
        );
    }
    for entry in &whole {
        // Each entry carries the plugin and the configuration of the version it is,
        // which is what a client pins a version by.
        assert_eq!(
            entry["PluginProperties"]["Name"].as_str(),
            Some(most.package_name.as_str()),
            "case {case}: expected the plugin properties of that version, got {entry:?}"
        );
        assert!(
            entry["PackageConfiguration"]["ConfigurationRequirement"]
                .as_str()
                .is_some(),
            "case {case}: expected the configuration of that version, got {entry:?}"
        );
        // "A message associated with the package version when it was uploaded":
        // nothing uploads a package the service publishes.
        assert!(
            entry["CommitMessage"].is_null(),
            "case {case}: expected no commit message on a published version, got {entry:?}"
        );
    }

    // Paged one version at a time, the walk reports the same history once, in the same
    // order, and only the last page carries no token.
    let mut walked: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let query = match token.as_deref() {
            None => "maxResults=1".to_string(),
            Some(token) => format!("maxResults=1&nextToken={token}"),
        };
        let answer = history(&emulator, &most.package_id, &query).await;
        let page = version_entries(&answer, &case);
        assert!(
            page.len() <= 1,
            "case {case}: a page is never larger than the request asked for, got {page:?}"
        );
        walked.extend(page);
        assert!(
            walked.len() <= whole.len(),
            "case {case}: the walk reported more versions than the history holds, got {} entries",
            walked.len()
        );
        match answer.body["NextToken"].as_str() {
            None => break,
            Some(next) => token = Some(next.to_owned()),
        }
    }
    assert_eq!(
        walked, whole,
        "case {case}: expected the paged walk to report the whole history once, got {walked:?}"
    );
}

#[tokio::test]
async fn the_account_s_packages_are_reported_beside_the_published_catalog() {
    let emulator = Emulator::new();
    let published = service_published(&whole_listing(&emulator, "before the create").await);

    let created = create_package(&emulator, "my-own-dictionary", PACKAGE_TYPES[0]).await;
    poll_until("the created package is available", || async {
        describe(
            &emulator,
            json!({ "Filters": [{ "Name": "PackageID", "Value": [created.clone()] }] }),
        )
        .await
        .body["PackageDetailsList"][0]["PackageStatus"]
            .as_str()
            == Some(PACKAGE_STATUS_AVAILABLE)
    })
    .await;

    let listing = whole_listing(&emulator, "after the create").await;
    let ids: Vec<String> = listing
        .iter()
        .map(|package| string_member(package, "PackageID", "after the create"))
        .collect();
    assert!(
        ids.contains(&created),
        "case after the create: expected the account's own package in the listing, got {ids:?}"
    );
    assert_eq!(
        service_published(&listing),
        published,
        "case after the create: expected the published catalog unchanged by a create"
    );
    assert_eq!(
        ids.len(),
        published.len() + 1,
        "case after the create: expected the published catalog plus the created package, got \
         {} entries",
        ids.len()
    );

    // The listing is one SET, so its order is the one the ids are keyed in: a page of
    // it names a place in that order rather than in one of the two halves.
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(
        ids, sorted,
        "case after the create: expected one order-stable set keyed by PackageID, got {ids:?}"
    );

    // A filter selects across both halves: the owner filter picks the account's own
    // package out, and the plugin type picks the published ones.
    let own = describe(
        &emulator,
        json!({
            "Filters": [{
                "Name": "PackageOwner",
                "Value": [::engine::emulator_account_id()],
            }],
        }),
    )
    .await;
    let own_ids: Vec<String> = reported(&own, "DescribePackages(PackageOwner=the account)")
        .iter()
        .map(|package| string_member(package, "PackageID", "the account's own packages"))
        .collect();
    assert_eq!(
        own_ids,
        vec![created.clone()],
        "case DescribePackages(PackageOwner=the account): expected the account's own package, \
         got {own_ids:?}"
    );

    let plugins = describe(
        &emulator,
        json!({ "Filters": [{ "Name": "PackageType", "Value": [PACKAGE_TYPE_ZIP_PLUGIN] }] }),
    )
    .await;
    let plugin_ids: Vec<String> = reported(&plugins, "DescribePackages(PackageType=ZIP-PLUGIN)")
        .iter()
        .map(|package| string_member(package, "PackageID", "the plugin packages"))
        .collect();
    assert_eq!(
        plugin_ids.len(),
        published.len(),
        "case DescribePackages(PackageType=ZIP-PLUGIN): expected the published plugin packages, \
         got {plugin_ids:?}"
    );
    assert!(
        !plugin_ids.contains(&created),
        "case DescribePackages(PackageType=ZIP-PLUGIN): the created dictionary is no plugin, got \
         {plugin_ids:?}"
    );
}

#[tokio::test]
async fn a_published_package_can_be_put_on_a_domain_and_read_back() {
    let emulator = Emulator::new();
    let domain = create_baseline(&emulator, DOMAIN).await;

    // A package of the catalog, picked by what it IS — a plugin published for the
    // release THIS domain runs — rather than by a name written here.
    let engine_version = string_member(&domain, "EngineVersion", "the created domain");
    let catalog = published_packages();
    let package = catalog
        .iter()
        .find(|package| package.engine_version.as_deref() == Some(engine_version.as_str()))
        .unwrap_or_else(|| {
            panic!("the catalog publishes a plugin for {engine_version}");
        });
    let case = format!("{} for {engine_version}", package.package_name);

    let associated = emulator
        .call(
            "POST",
            &format!(
                "{PREFIX}/packages/associate/{}/{DOMAIN}",
                package.package_id
            ),
            None,
        )
        .await;
    assert_eq!(
        associated.status, 200,
        "case AssociatePackage({case}): expected 200, got {} with {:?}",
        associated.status, associated.body
    );
    assert_eq!(
        associated.body["DomainPackageDetails"]["PackageName"].as_str(),
        Some(package.package_name.as_str()),
        "case AssociatePackage({case}): expected the published package's name, got {:?}",
        associated.body
    );

    // Read back through the domain's own listing once the clock has owed the
    // association its active status.
    let listed = || async {
        emulator
            .call("GET", &format!("{PREFIX}/domain/{DOMAIN}/packages"), None)
            .await
    };
    poll_until("the association becomes active", || async {
        listed().await.body["DomainPackageDetailsList"][0]["DomainPackageStatus"].as_str()
            == Some(DOMAIN_PACKAGE_STATUS_ACTIVE)
    })
    .await;

    let answer = listed().await;
    let on_domain = answer.body["DomainPackageDetailsList"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        on_domain.len(),
        1,
        "case ListPackagesForDomain({case}): expected the one package on the domain, got \
         {on_domain:?}"
    );
    assert_eq!(
        on_domain[0]["PackageID"].as_str(),
        Some(package.package_id.as_str()),
        "case ListPackagesForDomain({case}): expected the published package, got {:?}",
        on_domain[0]
    );
    assert_eq!(
        on_domain[0]["PackageType"].as_str(),
        Some(PACKAGE_TYPE_ZIP_PLUGIN),
        "case ListPackagesForDomain({case}): expected the plugin type the catalog publishes, got \
         {:?}",
        on_domain[0]
    );
    assert_eq!(
        on_domain[0]["PackageVersion"].as_str(),
        package.versions.last().map(|latest| latest.version.to_string()).as_deref(),
        "case ListPackagesForDomain({case}): expected the version the package has available, got \
         {:?}",
        on_domain[0]
    );

    // The other side: the domains a published package is on is reported for its id.
    let domains = emulator
        .call(
            "GET",
            &format!("{PREFIX}/packages/{}/domains", package.package_id),
            None,
        )
        .await;
    let names: Vec<&str> = domains.body["DomainPackageDetailsList"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|details| details["DomainName"].as_str())
                .collect()
        })
        .unwrap_or_default();
    assert_eq!(
        names,
        vec![DOMAIN],
        "case ListDomainsForPackage({case}): expected the domain the package was put on, got \
         {:?}",
        domains.body
    );

    // Taking it off again leaves the domain holding nothing, so the association is a
    // record of this account's and not of the catalog's.
    let dissociated = emulator
        .call(
            "POST",
            &format!(
                "{PREFIX}/packages/dissociate/{}/{DOMAIN}",
                package.package_id
            ),
            None,
        )
        .await;
    assert_eq!(
        dissociated.status, 200,
        "case DissociatePackage({case}): expected 200, got {} with {:?}",
        dissociated.status, dissociated.body
    );
    poll_until("the association is removed", || async {
        listed().await.body["DomainPackageDetailsList"]
            .as_array()
            .is_some_and(Vec::is_empty)
    })
    .await;
}

#[tokio::test]
async fn reading_the_published_catalog_changes_nothing() {
    let emulator = Emulator::new();

    let first = whole_listing(&emulator, "first read").await;
    let first_history = version_entries(
        &history(
            &emulator,
            &string_member(&first[0], "PackageID", "first read"),
            "",
        )
        .await,
        "first history read",
    );

    // A refused read reports nothing and changes nothing either.
    history(&emulator, "pkg-ffffffffffffffffffffffffffffffff", "").await;

    let second = whole_listing(&emulator, "second read").await;
    assert_eq!(
        first, second,
        "case repeat read: expected the same catalog the first read reported, got {} entries",
        second.len()
    );
    assert_eq!(
        first_history,
        version_entries(
            &history(
                &emulator,
                &string_member(&second[0], "PackageID", "second read"),
                ""
            )
            .await,
            "second history read",
        ),
        "case repeat history read: expected the same history the first read reported"
    );

    // The catalog is reference data computed from the published table, so reading it
    // leaves no package record behind in the store.
    let store = emulator.store();
    let state = store.lock();
    assert!(
        state.packages.is_empty(),
        "case stored packages: reporting the published catalog stores no package, got {:?}",
        state.packages.keys().collect::<Vec<_>>()
    );
}

#[tokio::test]
async fn every_published_package_names_a_release_the_service_offers() {
    let emulator = Emulator::new();

    // The table-level invariant first: no row of the catalog names a release the
    // service does not offer at all, whatever any read reports.
    assert!(
        catalog_publishes_published_releases(),
        "case the published table: every release a plugin is published for is one the \
         service offers"
    );

    // The releases the service publishes, read over the wire from `ListVersions` — the
    // catalog a client picks an engine version from — so a package built for a release
    // nothing runs would be a package no domain could take.
    let answer = emulator
        .call("GET", &format!("{PREFIX}/opensearch/versions"), None)
        .await;
    let offered: BTreeSet<String> = answer.body["Versions"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case ListVersions: expected a Versions array, got {:?}",
                answer.body
            )
        })
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();

    for package in service_published(&whole_listing(&emulator, "the published catalog").await) {
        let name = string_member(&package, "PackageName", "a published package");
        let engine_version = string_member(&package, "EngineVersion", &name);
        assert!(
            offered.contains(&engine_version),
            "case {name} for {engine_version}: expected a release the service offers, got the \
             offered set {offered:?}"
        );
    }
}
