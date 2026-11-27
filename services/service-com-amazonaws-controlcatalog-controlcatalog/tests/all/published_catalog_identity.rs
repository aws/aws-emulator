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
//! emulator's making. Four consequences each have a test below:
//!
//! 1. A domain, an objective or a common control whose published identifier the catalog
//!    records is served under `arn:{PARTITION}:controlcatalog:::{KIND}/{IDENTIFIER}`, with
//!    its published name, and an objective or common control reports the published ARN and
//!    name of the domain it belongs to (`ObjectiveSummary.Domain` is the "domain that the
//!    objective belongs to").
//! 2. The same entry is enumerated with the catalog's published WORDING, and the whole
//!    recorded set is enumerated whatever page size the caller walks the listing with:
//!    `MaxResults`/`NextToken` page the publication, they do not select which of it is
//!    published.
//! 3. The common-control library is published as a WHOLE, so every domain, objective and
//!    common control carries one publication instant as `CreateTime` and, unrevised,
//!    the same instant as `LastUpdateTime` — an entry is not dated by its position in
//!    the library.
//! 4. A published identifier is usable as the key it exists to be: a domain ARN harvested
//!    from `ListDomains` narrows `ListObjectives` to the objectives published under that
//!    domain.
//!
//! A fifth test covers the control side of the same rule: the Security Hub-implemented
//! controls the catalog publishes under their `SH.` designators are served under their
//! published control ARNs and, having been released together, with one release instant.
//!
//! The last two tests carry the WORDING half of the rule across every kind the catalog
//! publishes — domain, objective, common control and control: an entry whose published
//! description the catalog records is served with that text, character for character,
//! and with ONE such text wherever the catalog reports the entry (an unfiltered listing,
//! the filter that narrows to its parent, and, for a control, the read that resolves its
//! ARN). A description composed from the entry's subject is what those two tests refuse.
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
/// `arn:{PARTITION}:controlcatalog:::domain/{identifier}`,
/// `arn:{PARTITION}:controlcatalog:::objective/{identifier}`,
/// `arn:{PARTITION}:controlcatalog:::common-control/{identifier}` or
/// `arn:{PARTITION}:controlcatalog:::control/{identifier}` it names the entry by, and the
/// name it publishes for it. `domain` is the name of the domain an objective or a common
/// control belongs to, and `None` for a domain or a control itself. `description` is the
/// paragraph the catalog publishes for the entry, as a recorded response of the listings
/// quotes it, and `None` for an entry whose recorded quote is clipped or absent — so no
/// paragraph is written here that the publication's own wording does not carry; such an
/// entry still has to be served with a description, which the test below checks separately.
///
/// `pub(crate)`: the ONE table of published catalog text this test target holds, read by
/// the sibling common-control suite too rather than copied beside it.
pub(crate) struct PublishedEntry {
    pub(crate) kind: &'static str,
    pub(crate) identifier: &'static str,
    pub(crate) name: &'static str,
    pub(crate) domain: Option<&'static str>,
    pub(crate) description: Option<&'static str>,
}

/// The `domain` kind, as a catalog ARN spells it.
const DOMAIN: &str = "domain";
/// The `objective` kind, as a catalog ARN spells it.
const OBJECTIVE: &str = "objective";
/// The `common-control` kind, as a catalog ARN spells it.
pub(crate) const COMMON_CONTROL: &str = "common-control";
/// The `control` kind, as a catalog ARN spells it.
const CONTROL: &str = "control";

/// The name of the domain the published objectives below belong to.
const ASSET_MANAGEMENT: &str = "Asset management";
/// The name of the domain the change-management common controls below belong to.
const SECURE_DEVELOPMENT_LIFECYCLE: &str = "Secure development lifecycle and change management";

/// Library entries whose published identity the catalog records, spanning all three kinds
/// the library listings serve and, within the objectives and common controls, entries under
/// a parent domain that is itself recorded here as well as entries published under a
/// different domain. The common-control rows additionally span four objectives, published
/// descriptions of one, two and three sentences, and records whose published paragraph is
/// not recorded at all.
///
/// `pub(crate)`: read by the sibling common-control suite, which cross-checks the
/// common-control rows through an objective filter as well.
pub(crate) const PUBLISHED_ENTRIES: &[PublishedEntry] = &[
    PublishedEntry {
        kind: DOMAIN,
        identifier: "d4msesd9vvmzmmuvlv06m92uq",
        name: ASSET_MANAGEMENT,
        domain: None,
        description: Some(
            "This control domain focuses on asset management and the systematic tracking \
             and maintenance of physical or digital assets throughout their lifecycle, \
             including acquisition, utilization, and disposal. This reduces risks related \
             to accidents, malfunctions, and other issues that may cause damage to \
             property or harm to people.",
        ),
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "33mjpzadrlwo1by3c1012ai5i",
        name: "Business continuity and recovery",
        domain: None,
        description: Some(
            "This control domain focuses on planning and preparation of procedures and \
             resources to ensure the continued operation of critical business functions \
             in the event of a disruption, and to facilitate the recovery of normal \
             operations afterwards.",
        ),
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "884eyhr622uy73h6i1mjb9lfb",
        name: "Data protection",
        domain: None,
        description: Some(
            "This control domain focuses on data protection measures and practices used \
             to safeguard information, including confidentiality, integrity, and \
             availability, while ensuring compliance with applicable laws and regulations \
             governing data handling, cryptography, and encryption key management.",
        ),
    },
    PublishedEntry {
        kind: DOMAIN,
        identifier: "avh1ncrx6oyndrbsv0rfywvat",
        name: "Identity and access management",
        domain: None,
        description: Some(
            "This control domain focuses on identity and access management (IAM) policies \
             and technologies used to control and manage access to digital resources, \
             systems, and data based on the identity and roles of users and devices.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "ad11p1961s8erra9m185wa1nn",
        name: "Asset inventory management",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "This control objective focuses on maintaining an accurate and up-to-date \
             inventory of assets, including hardware, software, and data, to protect \
             organization investments from harm or loss.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "90gifwthorhxhxq7m0rtss98u",
        name: "Asset classification",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "This control objective focuses on classifying assets based on their value, \
             sensitivity, and criticality to the organization to manage investment risk \
             and unauthorized access to assets and information.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "3frxxgl64u9kzttiuheywykf7",
        name: "Asset maintenance",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "This control objective focuses on maintaining the availability and integrity \
             of assets, including performance management, regular maintenance, and repairs \
             to protect and extract the maximum value of the organization's IT investments.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "5ve4jodybrg8wnky75fp50sbf",
        name: "Asset lifecycle management",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "This control objective focuses on managing assets throughout their entire \
             lifecycle, including acquisition, deployment, use, and retirement. This helps \
             manage risks associated with asset costs by ensuring optimum asset \
             productivity, performance, efficiency, and profitability.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "ags5wgkyvwriix77zegtwhyo9",
        name: "Asset loss prevention, response, and recovery",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "This control objective focuses on preventing asset loss, and responding to \
             and recovering lost, stolen, or damaged assets to contribute to the \
             organization's profitability by reducing losses.",
        ),
    },
    PublishedEntry {
        kind: OBJECTIVE,
        identifier: "14qj3s9axkdma059lhktc14dx",
        name: "Change management",
        domain: Some(SECURE_DEVELOPMENT_LIFECYCLE),
        description: None,
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "1nxqoydhsjdyboc4frsrrmev",
        name: "Change request and approval",
        domain: Some(SECURE_DEVELOPMENT_LIFECYCLE),
        description: Some(
            "Define, document, implement, and maintain a formal process for submitting \
             and approving change requests. Change requests should outline the proposed \
             changes, their impact, and any associated risks. The requests should then be \
             reviewed and approved by the appropriate stakeholders, ensuring that changes \
             are properly assessed, authorized, and tracked.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "8xy67kb7kojr38ozqpmrm60d7",
        name: "Change deployment and automation",
        domain: Some(SECURE_DEVELOPMENT_LIFECYCLE),
        description: Some(
            "Implement an automated change deployment pipeline that automates deployment \
             of systems, software, and infrastructure as code (IaC) to production \
             environments through a controlled and consistent process. Enforce automated \
             security controls, perform validation checks, and provide a rollback \
             mechanism in case of deployment failures.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "2o50zyyv219ir9r7brjq0y37f",
        name: "Rollback and recovery procedures",
        domain: Some(SECURE_DEVELOPMENT_LIFECYCLE),
        description: None,
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "ez1ccu4xkxrmlekcvjqo3jk59",
        name: "Asset performance management",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Monitor and optimize the performance and security of assets, including \
             hardware and software.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "ez4hcdf2fpirhuyojurkval9w",
        name: "Asset maintenance scheduling",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Schedule and track maintenance activities for assets, including preventive \
             maintenance and repairs.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "7encqm6cfsw704eoahh3ujr7y",
        name: "Asset valuation",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Assign a value to assets based on their cost, replacement value, or other \
             relevant factors.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "5m9jk2j28m8m4nr53u4c3br0h",
        name: "Asset classification and categorization",
        domain: Some(ASSET_MANAGEMENT),
        description: None,
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "gr180qll01cux9k41amnf002",
        name: "Asset labeling",
        domain: Some(ASSET_MANAGEMENT),
        description: Some("Label assets with unique identifiers."),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "1ukpmkewk4i92tjmhsvewi4y7",
        name: "Inventory of authorized assets and automated discovery",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Maintain an asset inventory of organization authorized and existing \
             hardware, software, and media. Where possible, utilize automated tools to \
             facilitate the discovery and ongoing tracking of such assets.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "c0qrxhefhmxkbq22tiejp3enn",
        name: "Unauthorized asset management",
        domain: Some(ASSET_MANAGEMENT),
        description: None,
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "aaaq1vhzpslfrk1o4hvgk3ti4",
        name: "Asset acquisition",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Assess potential security and compliance risks as part of the asset \
             acquisition process.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "eboqkjlzo44e0ia5wba1770tw",
        name: "Asset deployment",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Configure assets securely prior to deployment in the environment. Confirm \
             the proper integration of new assets with existing systems.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "76akwzuoa1j6cxqfsorr12e9k",
        name: "Secure asset management",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Implement asset management processes to securely track, control, and \
             document all organizational assets. Maintain configuration control of \
             systems undergoing service, repair, or pending return to service.",
        ),
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "bme9uq29isv0jdrdy4yj2c702",
        name: "Asset retirement and disposition",
        domain: Some(ASSET_MANAGEMENT),
        description: Some(
            "Retire and dispose of assets, including media, that are no longer needed. \
             Securely erase or destroy any sensitive data before asset disposal.",
        ),
    },
    // Two of the common controls published under the asset loss objective: the catalog
    // names each of them by the identifier recorded here, and this crate has the published
    // NAME of these two but not their published paragraph, so no description is recorded
    // for them (the test below still requires one to be served).
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "4kdrp1fqw2zy6shq95defui2y",
        name: "Asset loss prevention, recovery, and response",
        domain: Some(ASSET_MANAGEMENT),
        description: None,
    },
    PublishedEntry {
        kind: COMMON_CONTROL,
        identifier: "6rxe3eaojbhyha52wtvahsjxn",
        name: "Asset damage prevention and response",
        domain: Some(ASSET_MANAGEMENT),
        description: None,
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
        COMMON_CONTROL => ("/common-controls", "CommonControls"),
        CONTROL => ("/list-controls", "Controls"),
        other => panic!("case {other}: the table only names kinds a listing serves"),
    }
}

/// Every entry a listing serves, keyed by its `Arn`, walked with `max_results` items per
/// page (`None` for the listing's own default page).
async fn listed_by_arn(
    emulator: &Emulator,
    path: &str,
    items_key: &str,
    max_results: Option<u32>,
) -> BTreeMap<String, Value> {
    let items = drain(emulator, path, items_key, max_results, "{}").await;
    assert!(
        !items.is_empty(),
        "case {path} maxResults={max_results:?}: the catalog must publish its {items_key}"
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

/// Every listing a recorded row names, each keyed by catalog kind and then by `Arn`, walked
/// with `max_results` items per page.
async fn taxonomy_listings(
    emulator: &Emulator,
    max_results: Option<u32>,
) -> BTreeMap<&'static str, BTreeMap<String, Value>> {
    let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
    for kind in [DOMAIN, OBJECTIVE, COMMON_CONTROL] {
        let (path, items_key) = listing_for(kind);
        listings.insert(
            kind,
            listed_by_arn(emulator, path, items_key, max_results).await,
        );
    }
    listings
}

/// A timestamp member of a served entry, in seconds since the Unix epoch.
fn timestamp(case: &str, entry: &Value, member: &str) -> f64 {
    entry
        .get(member)
        .and_then(Value::as_f64)
        .unwrap_or_else(|| panic!("case {case}: {member} must be a timestamp, got {entry:?}"))
}

/// Every recorded published entry — domain, objective or common control — is served under
/// the catalog's own ARN, with its published name, and an objective or a common control
/// reports its owning domain's published ARN.
#[tokio::test]
async fn every_published_library_entry_is_served_under_its_published_arn() {
    let emulator = Emulator::new();
    let listings = taxonomy_listings(&emulator, None).await;

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

/// Every recorded published entry is enumerated by its listing, with the catalog's own
/// name and published paragraph, at every page size the listing is walked with.
///
/// The rows span every kind the library listings serve, objectives and common controls under
/// two different domains, and entries whose published paragraph this project records as well
/// as entries it does not (which still have to be served with a description of some kind).
/// The listing is walked both in its default page and one item per page: `control_catalog`'s
/// `every_list_operation_pages_its_whole_collection_exactly_once` already pins the item
/// set and order across page sizes, so what is added here is that the published WORDING
/// survives a resumed walk.
#[tokio::test]
async fn every_published_entry_is_served_with_the_catalogs_published_wording() {
    let emulator = Emulator::new();

    // The listing's own page, and a one-item-per-page walk: paging must not change WHICH
    // entries the catalog publishes, nor what it publishes about them.
    for page_size in [None, Some(1u32)] {
        let listings = taxonomy_listings(&emulator, page_size).await;

        for row in PUBLISHED_ENTRIES {
            let arn = catalog_kind_arn(row.kind, row.identifier);
            let case = format!("{} {} maxResults={page_size:?}", row.kind, row.identifier);
            let entry = listings[row.kind].get(&arn).unwrap_or_else(|| {
                panic!(
                    "case {case}: the catalog must enumerate {arn}, got {:?}",
                    listings[row.kind].keys()
                )
            });

            assert_eq!(
                entry["Name"].as_str(),
                Some(row.name),
                "case {case}: expected the published name {:?}, got {:?}",
                row.name,
                entry["Name"]
            );
            match row.description {
                Some(published) => assert_eq!(
                    entry["Description"].as_str(),
                    Some(published),
                    "case {case}: expected the catalog's published paragraph \
                     {published:?}, got {:?}",
                    entry["Description"]
                ),
                None => assert!(
                    entry["Description"]
                        .as_str()
                        .is_some_and(|text| !text.is_empty()),
                    "case {case}: a published entry carries a description, got {:?}",
                    entry["Description"]
                ),
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
        let listed = listed_by_arn(&emulator, path, items_key, None).await;
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
    let domains = listed_by_arn(&emulator, "/domains", "Domains", None).await;

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

/// One entry outside the taxonomy listings whose published DESCRIPTION the catalog
/// records: a common control, or a control.
///
/// Source: the Control Catalog's own published records — the opaque identifier its ARN
/// carries, and the text it publishes as the entry's description (for a Security
/// Hub-implemented control, the sentence the catalog publishes for that control; for a
/// common control, the paragraph the common-control library publishes under its
/// objective). The rows span what the rule generalises over: both kinds, descriptions of
/// one sentence and of several, the two implementing surfaces a control is published
/// over, and four different owning objectives — and most of them are entries no step this
/// suite was written for named, so the rule is asserted beyond the cited ones.
struct PublishedDescription {
    kind: &'static str,
    identifier: &'static str,
    description: &'static str,
}

/// Common controls and controls whose published description the catalog records.
const PUBLISHED_DESCRIPTIONS: &[PublishedDescription] = &[
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "1ukpmkewk4i92tjmhsvewi4y7",
        description: "Maintain an asset inventory of organization authorized and \
                      existing hardware, software, and media. Where possible, utilize \
                      automated tools to facilitate the discovery and ongoing tracking \
                      of such assets.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "7encqm6cfsw704eoahh3ujr7y",
        description: "Assign a value to assets based on their cost, replacement value, \
                      or other relevant factors.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "ez1ccu4xkxrmlekcvjqo3jk59",
        description: "Monitor and optimize the performance and security of assets, \
                      including hardware and software.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "ez4hcdf2fpirhuyojurkval9w",
        description: "Schedule and track maintenance activities for assets, including \
                      preventive maintenance and repairs.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "aaaq1vhzpslfrk1o4hvgk3ti4",
        description: "Assess potential security and compliance risks as part of the \
                      asset acquisition process.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "eboqkjlzo44e0ia5wba1770tw",
        description: "Configure assets securely prior to deployment in the environment. \
                      Confirm the proper integration of new assets with existing systems.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "bme9uq29isv0jdrdy4yj2c702",
        description: "Retire and dispose of assets, including media, that are no longer \
                      needed. Securely erase or destroy any sensitive data before asset \
                      disposal.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "1nxqoydhsjdyboc4frsrrmev",
        description: "Define, document, implement, and maintain a formal process for \
                      submitting and approving change requests. Change requests should \
                      outline the proposed changes, their impact, and any associated \
                      risks. The requests should then be reviewed and approved by the \
                      appropriate stakeholders, ensuring that changes are properly \
                      assessed, authorized, and tracked.",
    },
    PublishedDescription {
        kind: COMMON_CONTROL,
        identifier: "8xy67kb7kojr38ozqpmrm60d7",
        description: "Implement an automated change deployment pipeline that automates \
                      deployment of systems, software, and infrastructure as code (IaC) \
                      to production environments through a controlled and consistent \
                      process. Enforce automated security controls, perform validation \
                      checks, and provide a rollback mechanism in case of deployment \
                      failures.",
    },
    PublishedDescription {
        kind: CONTROL,
        identifier: "ezfflnh8xgshxb78grtt5sjc5",
        description: "This AWS control checks whether the root user access key is \
                      available.",
    },
    PublishedDescription {
        kind: CONTROL,
        identifier: "6g3gbim493yofkc9eg6vvnahm",
        description: "This AWS control checks whether AWS Multi-Factor Authentication \
                      (MFA) is enabled for all AWS Identity and Access Management (IAM) \
                      users that use a console password.",
    },
    PublishedDescription {
        kind: CONTROL,
        identifier: "435u8u4w5bsf02tkxga5jisbc",
        description: "This AWS control checks that there is at least one multi-region \
                      AWS CloudTrail trail includes read and write management events.",
    },
    PublishedDescription {
        kind: CONTROL,
        identifier: "624yg0j9d8swiglwfc1m4kvnm",
        description: "This AWS control checks whether CloudTrail log file validation is \
                      enabled.",
    },
    PublishedDescription {
        kind: CONTROL,
        identifier: "62smpoz33dsy0oa7u1iwa58lz",
        description: "This AWS control checks whether Amazon GuardDuty is enabled in \
                      your AWS account and region.",
    },
];

/// Page sizes the listings below are walked with: the listing's own default page, and a
/// size that splits every one of them across several pages, so a resumed walk has to
/// carry the published wording too.
const WORDING_PAGE_SIZES: [Option<u32>; 2] = [None, Some(7)];

/// Every entry whose published description the catalog records, as
/// `(kind, identifier, published description)` — the taxonomy rows above whose paragraph
/// is recorded, plus the common controls and controls of [`PUBLISHED_DESCRIPTIONS`].
fn recorded_descriptions() -> Vec<(&'static str, &'static str, &'static str)> {
    PUBLISHED_ENTRIES
        .iter()
        .filter_map(|row| {
            row.description
                .map(|description| (row.kind, row.identifier, description))
        })
        .chain(
            PUBLISHED_DESCRIPTIONS
                .iter()
                .map(|row| (row.kind, row.identifier, row.description)),
        )
        .collect()
}

/// Every entry whose published description the catalog records is served with THAT text,
/// character for character, by the listing that publishes it — at the listing's own page
/// size and at one that splits it across pages.
///
/// The rows span all four kinds the catalog publishes (domain, objective, common control
/// and control), descriptions of one sentence and of several, and both surfaces a control
/// is implemented over. A description the emulator composes from the entry's subject —
/// shorter, re-worded, or naming the implementing rule — fails this test, which is the
/// point of it: the catalog is a publication, and its text is published content rather
/// than something to be summarised.
#[tokio::test]
async fn every_recorded_entry_is_served_with_the_catalogs_published_description() {
    let emulator = Emulator::new();
    let rows = recorded_descriptions();
    assert!(
        rows.len() > 10,
        "case table: the published-wording rows must span the catalog, got {}",
        rows.len()
    );

    for page_size in WORDING_PAGE_SIZES {
        let mut listings: BTreeMap<&str, BTreeMap<String, Value>> = BTreeMap::new();
        for (kind, _, _) in &rows {
            if !listings.contains_key(kind) {
                let (path, items_key) = listing_for(kind);
                listings.insert(
                    kind,
                    listed_by_arn(&emulator, path, items_key, page_size).await,
                );
            }
        }

        for (kind, identifier, published) in &rows {
            let arn = catalog_kind_arn(kind, identifier);
            let case = format!("{kind} {identifier} maxResults={page_size:?}");
            let entry = listings[kind]
                .get(&arn)
                .unwrap_or_else(|| panic!("case {case}: the catalog must publish {arn}"));
            assert_eq!(
                entry["Description"].as_str(),
                Some(*published),
                "case {case}: expected the catalog's published description \
                 {published:?}, got {:?}",
                entry["Description"]
            );
        }
    }
}

/// One published description per entry, wherever the catalog reports it: an objective and
/// a common control carry the same text in the listing narrowed to their parent as in the
/// unfiltered one, and a control carries the same text in `GetControl` as in
/// `ListControls`.
///
/// The rows are the same table as above, so this adds the agreement BETWEEN the readers of
/// one entry: a client that re-reads a catalog entry through the filter it harvested the
/// parent ARN from, or through the ARN itself, must not be handed a second wording.
#[tokio::test]
async fn a_published_description_is_one_text_wherever_the_catalog_reports_it() {
    let emulator = Emulator::new();
    let mut compared_kinds: BTreeSet<&str> = BTreeSet::new();

    for (kind, identifier, published) in recorded_descriptions() {
        let arn = catalog_kind_arn(kind, identifier);
        let case = format!("{kind} {identifier}");
        let (path, items_key) = listing_for(kind);
        let listed = listed_by_arn(&emulator, path, items_key, None).await;
        let entry = listed
            .get(&arn)
            .unwrap_or_else(|| panic!("case {case}: the catalog must publish {arn}"));

        // The second reader of this entry: the filter that narrows to its parent, or —
        // for a control — the read that resolves its ARN.
        let second = match kind {
            CONTROL => emulator
                .ok("/get-control", &format!(r#"{{"ControlArn":"{arn}"}}"#))
                .await,
            OBJECTIVE => {
                let domain_arn = at_path(entry, &["Domain", "Arn"])
                    .unwrap_or_else(|| panic!("case {case}: an objective reports its domain"));
                let body =
                    format!(r#"{{"ObjectiveFilter":{{"Domains":[{{"Arn":"{domain_arn}"}}]}}}}"#);
                one_of(&emulator, path, items_key, &body, &arn, &case).await
            }
            COMMON_CONTROL => {
                let objective_arn = at_path(entry, &["Objective", "Arn"]).unwrap_or_else(|| {
                    panic!("case {case}: a common control reports its objective")
                });
                let body = format!(
                    r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{objective_arn}"}}]}}}}"#
                );
                one_of(&emulator, path, items_key, &body, &arn, &case).await
            }
            // A domain is served by one reader only, so there is no second wording of it
            // to disagree with; the listing itself is covered by the test above.
            _ => continue,
        };

        assert_eq!(
            second["Description"].as_str(),
            Some(published),
            "case {case}: the second reader of the entry must report the published \
             description {published:?}, got {:?}",
            second["Description"]
        );
        assert_eq!(
            second["Description"], entry["Description"],
            "case {case}: two readers of one entry must report one description, got {:?} \
             and {:?}",
            second["Description"], entry["Description"]
        );
        compared_kinds.insert(kind);
    }

    for kind in [OBJECTIVE, COMMON_CONTROL, CONTROL] {
        assert!(
            compared_kinds.contains(kind),
            "case table: the rows must reach a {kind} with two readers, saw \
             {compared_kinds:?}"
        );
    }
}

/// The one entry `arn` names in the listing at `path` narrowed by `body`, or a panic
/// naming the case.
async fn one_of(
    emulator: &Emulator,
    path: &str,
    items_key: &str,
    body: &str,
    arn: &str,
    case: &str,
) -> Value {
    let items = drain(emulator, path, items_key, None, body).await;
    items
        .into_iter()
        .find(|item| at_path(item, &["Arn"]) == Some(arn))
        .unwrap_or_else(|| panic!("case {case}: the filtered listing must serve {arn}"))
}
