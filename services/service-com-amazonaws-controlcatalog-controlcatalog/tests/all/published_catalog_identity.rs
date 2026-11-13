// SPDX-License-Identifier: CC0-1.0

//! The published identity the catalog's library and its Security Hub controls are served
//! with.
//!
//! The rule under test: the Control Catalog is an Amazon Web Services-published,
//! account- and Region-independent read-only publication — its ARNs carry neither an
//! account nor a Region
//! (`smithy_query(query="shape", shape="DomainArn,ObjectiveArn,ControlArn",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`)
//! — so every entry it publishes is served under the catalog's OWN opaque identifier and
//! with the catalog's own publication instants, not under an identifier or a date of the
//! emulator's making. Three consequences each have a test below:
//!
//! 1. A domain or an objective whose published identifier the catalog records is served
//!    under `arn:{PARTITION}:controlcatalog:::{KIND}/{IDENTIFIER}`, with its published
//!    name, and an objective reports the published ARN and name of the domain it belongs
//!    to (`ObjectiveSummary.Domain` is the "domain that the objective belongs to").
//! 2. The common-control library is published as a WHOLE, so every domain, objective and
//!    common control carries one publication instant as `CreateTime` and, unrevised,
//!    the same instant as `LastUpdateTime` — an entry is not dated by its position in
//!    the library.
//! 3. A published identifier is usable as the key it exists to be: a domain ARN harvested
//!    from `ListDomains` narrows `ListObjectives` to the objectives published under that
//!    domain.
//!
//! A fourth test covers the control side of the same rule: the Security Hub-implemented
//! controls the catalog publishes under their `SH.` designators are served under their
//! published control ARNs and, having been released together, with one release instant.
//!
//! Each test is a TABLE spanning the class rather than one entry picked by hand: the rows
//! differ in catalog kind (domain, objective, common control), in whether the entry's
//! published identifier is recorded, and — for the generated rows — cover every entry the
//! three library listings serve.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::control_catalog::{at_path, catalog_arn, catalog_kind_arn, drain, Emulator};

/// One entry of the library whose published identity the catalog records.
///
/// Source: the Control Catalog's own published ontology — the ARN
/// `arn:{PARTITION}:controlcatalog:::domain/{identifier}` or
/// `arn:{PARTITION}:controlcatalog:::objective/{identifier}` it names the entry by, and
/// the name it publishes for it. `domain` is the name of the domain an objective belongs
/// to, and `None` for a domain itself.
struct PublishedEntry {
    kind: &'static str,
    identifier: &'static str,
    name: &'static str,
    domain: Option<&'static str>,
}

/// The `domain` kind, as a catalog ARN spells it.
const DOMAIN: &str = "domain";
/// The `objective` kind, as a catalog ARN spells it.
const OBJECTIVE: &str = "objective";

/// The name of the domain the published objectives below belong to.
const ASSET_MANAGEMENT: &str = "Asset management";

/// Library entries whose published identity the catalog records, spanning both kinds the
/// two taxonomy listings serve and, within the objectives, one whose parent domain is
/// itself recorded here and one published under a different domain.
const PUBLISHED_ENTRIES: &[PublishedEntry] = &[
    PublishedEntry {
        kind: DOMAIN,
        identifier: "d4msesd9vvmzmmuvlv06m92uq",
        name: ASSET_MANAGEMENT,
        domain: None,
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "33mjpzadrlwo1by3c1012ai5i",
        name: "Business continuity and recovery",
        domain: None,
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "884eyhr622uy73h6i1mjb9lfb",
        name: "Data protection",
        domain: None,
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "avh1ncrx6oyndrbsv0rfywvat",
        name: "Identity and access management",
        domain: None,
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "ad11p1961s8erra9m185wa1nn",
        name: "Asset inventory management",
        domain: Some(ASSET_MANAGEMENT),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "90gifwthorhxhxq7m0rtss98u",
        name: "Asset classification",
        domain: Some(ASSET_MANAGEMENT),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "3frxxgl64u9kzttiuheywykf7",
        name: "Asset maintenance",
        domain: Some(ASSET_MANAGEMENT),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "14qj3s9axkdma059lhktc14dx",
        name: "Change management",
        domain: Some("Secure development lifecycle and change management"),
    },
];

/// One Security Hub-implemented control's published identity: `(designator, published
/// identifier)`.
///
/// Source: the Control Catalog's own published control list. The designator is the
/// control's `SH.` alias, which is "a human-readable designator, such as `SH.S3.1`"
/// (`smithy_query(query="shape", shape="ControlSummary")`).
const PUBLISHED_SECURITY_HUB_CONTROLS: &[(&str, &str)] = &[
    ("SH.CloudTrail.1", "435u8u4w5bsf02tkxga5jisbc"),
    ("SH.IAM.4", "ezfflnh8xgshxb78grtt5sjc5"),
    ("SH.IAM.5", "6g3gbim493yofkc9eg6vvnahm"),
];

/// The instant the catalog published the common-control library, in seconds since the
/// Unix epoch.
///
/// Source: the publication instant the catalog reports for the library's domains and
/// objectives. One instant, because the library is published as a whole.
const LIBRARY_PUBLICATION_TIME: f64 = 1_710_288_000.0;

/// The three listings that serve the library, and the member each carries its items in.
const LIBRARY_LISTINGS: [(&str, &str); 3] = [
    ("/domains", "Domains"),
    ("/objectives", "Objectives"),
    ("/common-controls", "CommonControls"),
];

/// The route and item member of the listing that serves entries of `kind`.
fn listing_for(kind: &str) -> (&'static str, &'static str) {
    match kind {
        DOMAIN => ("/domains", "Domains"),
        OBJECTIVE => ("/objectives", "Objectives"),
        other => panic!("case {other}: the table only names kinds a listing serves"),
    }
}

/// Every entry a listing serves, keyed by its `Arn`.
async fn listed_by_arn(emulator: &Emulator, path: &str, items_key: &str) -> BTreeMap<String, Value> {
    let items = drain(emulator, path, items_key, None, "{}").await;
    assert!(
        !items.is_empty(),
        "case {path}: the catalog must publish its {items_key}"
    );
    items
        .into_iter()
        .map(|item| {
            let arn = item["Arn"]
                .as_str()
                .unwrap_or_else(|| panic!("case {path}: an entry carries its ARN, got {item:?}"))
                .to_string();
            (arn, item)
        })
        .collect()
}

/// A timestamp member of a served entry, in seconds since the Unix epoch.
fn timestamp(case: &str, entry: &Value, member: &str) -> f64 {
    entry
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a timestamp, got {entry:?}"))
}

/// Every recorded published domain and objective is served under the catalog's own ARN,
/// with its published name, and an objective reports its owning domain's published ARN.
#[tokio::test]
async fn every_published_library_entry_is_served_under_its_published_arn() {
    let emulator = Emulator::new();
    let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for kind in [DOMAIN, OBJECTIVE] {
        let (path, items_key) = listing_for(kind);
        listings.insert(kind, listed_by_arn(&emulator, path, items_key).await);
    }

    // The published ARN of each recorded domain, by the name it is published under, so an
    // objective's owning-domain reference can be checked against it.
    let domain_arns: BTreeMap<&str, String> = PUBLISHED_ENTRIES
        .iter()
        .filter(|row| row.kind == DOMAIN)
        .map(|row| (row.name, catalog_kind_arn(DOMAIN, row.identifier)))
        .collect();

    for row in PUBLISHED_ENTRIES {
        let arn = catalog_kind_arn(row.kind, row.identifier);
        let case = format!("{} {}", row.kind, row.identifier);
        let entry = listings[row.kind]
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case}: the catalog must publish {arn}"));

        assert_eq!(
            entry["Name"].as_str(),
            Some(row.name),
            "case {case}: expected the published name {:?}, got {:?}",
            row.name,
            entry["Name"]
        );
        assert!(
            entry["Description"].as_str().is_some_and(|text| !text.is_empty()),
            "case {case}: a published entry carries its description, got {:?}",
            entry["Description"]
        );

        if let Some(domain_name) = row.domain {
            assert_eq!(
                at_path(entry, &["Domain", "Name"]),
                Some(domain_name),
                "case {case}: expected the owning domain {domain_name}, got {:?}",
                entry["Domain"]
            );
            // Where the owning domain's own published identity is recorded, the
            // back-reference must carry it: a client renders the taxonomy tree by
            // joining the two.
            if let Some(expected_domain_arn) = domain_arns.get(domain_name) {
                assert_eq!(
                    at_path(entry, &["Domain", "Arn"]),
                    Some(expected_domain_arn.as_str()),
                    "case {case}: expected the owning domain's published ARN \
                     {expected_domain_arn}, got {:?}",
                    entry["Domain"]
                );
                assert!(
                    listings[DOMAIN].contains_key(expected_domain_arn),
                    "case {case}: ListDomains must serve the domain {expected_domain_arn} \
                     this objective points at"
                );
            }
        }
    }
}

/// Every entry of the library — domains, objectives and common controls, not only the
/// recorded rows — carries the library's one publication instant, as `CreateTime` and,
/// unrevised, as `LastUpdateTime`.
#[tokio::test]
async fn every_library_entry_carries_the_librarys_publication_instant() {
    let emulator = Emulator::new();

    for (path, items_key) in LIBRARY_LISTINGS {
        let listed = listed_by_arn(&emulator, path, items_key).await;
        for (arn, entry) in &listed {
            let case = format!("{path} {arn}");
            assert_eq!(
                timestamp(&case, entry, "CreateTime"),
                LIBRARY_PUBLICATION_TIME,
                "case {case}: expected the library's publication instant \
                 {LIBRARY_PUBLICATION_TIME}, got {:?}",
                entry["CreateTime"]
            );
            assert_eq!(
                timestamp(&case, entry, "LastUpdateTime"),
                LIBRARY_PUBLICATION_TIME,
                "case {case}: an unrevised entry reports LastUpdateTime equal to its \
                 CreateTime, got {:?}",
                entry["LastUpdateTime"]
            );
        }
    }
}

/// A domain ARN harvested from `ListDomains` narrows `ListObjectives` to the objectives
/// published under that domain: the published identifier is usable as the filter key it
/// exists to be.
#[tokio::test]
async fn a_published_domain_arn_filters_the_objectives_under_it() {
    let emulator = Emulator::new();
    let domains = listed_by_arn(&emulator, "/domains", "Domains").await;

    for row in PUBLISHED_ENTRIES.iter().filter(|row| row.kind == DOMAIN) {
        let arn = catalog_kind_arn(DOMAIN, row.identifier);
        let case = format!("domain {}", row.identifier);
        assert!(
            domains.contains_key(&arn),
            "case {case}: ListDomains must publish {arn}"
        );

        let body = format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{arn}"}}]}}}}"#);
        // Page size 1 as well, so the filtered walk is resumed across pages.
        for page_size in [None, Some(1u32)] {
            let filtered = drain(&emulator, "/objectives", "Objectives", page_size, &body).await;
            assert!(
                !filtered.is_empty(),
                "case {case} maxResults={page_size:?}: the domain's objectives must come \
                 back for the published ARN the client harvested"
            );
            for objective in &filtered {
                assert_eq!(
                    at_path(objective, &["Domain", "Arn"]),
                    Some(arn.as_str()),
                    "case {case} maxResults={page_size:?}: a filtered objective must \
                     belong to the domain filtered on, got {objective:?}"
                );
            }
        }
    }
}

/// Every Security Hub control the catalog records a published identity for is served
/// under its published control ARN, and the controls the catalog released as one carry
/// one `CreateTime`.
#[tokio::test]
async fn published_security_hub_controls_carry_their_published_identity() {
    let emulator = Emulator::new();
    let controls = drain(&emulator, "/list-controls", "Controls", None, "{}").await;
    assert!(
        !controls.is_empty(),
        "case catalog: ListControls must publish the catalog's controls"
    );

    let mut release_instants: BTreeSet<i64> = BTreeSet::new();
    for (designator, identifier) in PUBLISHED_SECURITY_HUB_CONTROLS {
        let case = *designator;
        let expected_arn = catalog_arn(identifier);
        let summary = controls
            .iter()
            .find(|control| {
                control["Aliases"]
                    .as_array()
                    .is_some_and(|aliases| aliases.iter().any(|alias| alias == designator))
            })
            .unwrap_or_else(|| panic!("case {case}: the catalog must publish this control"));

        assert_eq!(
            summary["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: expected the catalog's published ARN {expected_arn}, got {:?}",
            summary["Arn"]
        );
        // The control reads back by the ARN a client harvested from the listing.
        let read_back = emulator
            .ok(
                "/get-control",
                &format!(r#"{{"ControlArn":"{expected_arn}"}}"#),
            )
            .await;
        assert_eq!(
            read_back["Arn"].as_str(),
            Some(expected_arn.as_str()),
            "case {case}: GetControl must resolve the published ARN, got {read_back:?}"
        );
        release_instants.insert(timestamp(case, summary, "CreateTime") as i64);
    }

    assert_eq!(
        release_instants.len(),
        1,
        "case Security Hub controls: controls the catalog released together must share \
         one CreateTime, got {release_instants:?}"
    );
}
