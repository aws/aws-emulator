// SPDX-License-Identifier: CC0-1.0

//! The published identity of the common-control library, and the set an objective
//! filter enumerates.
//!
//! The rule under test: the common-control library is an Amazon Web Services-published,
//! account- and Region-independent read-only publication — a `CommonControlArn` carries
//! neither an account nor a Region
//! (`smithy_query(query="shape", shape="CommonControlArn,CommonControlSummary",
//! model_path="services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model/com-amazonaws-controlcatalog-controlcatalog.json")`)
//! — so each entry is served under the catalog's OWN published opaque identifier, and a
//! listing narrowed to one objective enumerates exactly the common controls published
//! under that objective, each reporting its own published name and the domain and
//! objective that own it. An identifier of the emulator's own making resolves nowhere
//! for a client holding the published one, so a client that walks domain -> objective ->
//! common control, or that joins a harvested ARN to its own records, must get the
//! published identity back.
//!
//! Each test is a TABLE rather than one entry picked by hand. The rows span the
//! objectives whose published sets the catalog records, and differ in the SIZE of that
//! set (two, three, four, five and seven common controls) and in the domain that owns it;
//! the per-entry rows additionally cover an entry published under an objective whose whole
//! set is not recorded, and every page size the set is walked with.

use std::collections::BTreeSet;

use serde_json::Value;

use crate::control_catalog::{at_path, catalog_kind_arn, drain, Emulator};

/// The `common-control` kind, as a catalog ARN spells it.
const COMMON_CONTROL: &str = "common-control";
/// The `objective` kind, as a catalog ARN spells it.
const OBJECTIVE: &str = "objective";
/// The `domain` kind, as a catalog ARN spells it.
const DOMAIN: &str = "domain";

/// One objective of the library, with the common controls published under it.
///
/// Source: the Control Catalog's own published ontology — the published opaque
/// identifiers it names the objective, its domain and each common control by, and the name
/// each is served under (the same table `src/state/published_ontology.rs` records, whose
/// header carries that provenance and says which entries' wording follows the published
/// library rather than being quoted from it).
struct PublishedObjective {
    identifier: &'static str,
    name: &'static str,
    domain_identifier: &'static str,
    domain_name: &'static str,
    /// `(published common-control identifier, published name)`, the WHOLE set published
    /// under this objective.
    common_controls: &'static [(&'static str, &'static str)],
}

/// The name of the domain the asset objectives below belong to.
const ASSET_MANAGEMENT: &str = "Asset management";
/// The published identifier of that domain.
const ASSET_MANAGEMENT_ID: &str = "d4msesd9vvmzmmuvlv06m92uq";

/// The objectives whose whole published common-control set the catalog records, spanning
/// set sizes from two to seven and two different owning domains.
///
/// The ARN SET of each row is the rule under test, and every identifier below is the
/// catalog's own. A row's `Name` values are the names the catalog publishes where this
/// crate has the publication's wording for them — "Asset acquisition", "Asset deployment",
/// "Secure asset management", "Asset retirement and disposition", "Asset performance
/// management", "Asset maintenance scheduling", "Change request and approval", "Change
/// deployment and automation", "Rollback and recovery procedures", "Asset valuation",
/// "Asset sensitivity", "Asset criticality", "Asset classification and categorization",
/// "Asset labeling", "Inventory of authorized assets and automated discovery", "Asset
/// inventory reconciliation and audit", "Unauthorized asset management", "Asset loss
/// prevention, recovery, and response" and "Asset damage prevention and response" — and
/// otherwise the entry's subject written in the library's voice (as
/// `src/state/published_ontology.rs`'s header records); for the latter the name assertion
/// pins the catalog against its own table rather than against the publication, which is why
/// the set, paging and filtered-versus-unfiltered assertions are what carry this suite.
const PUBLISHED_OBJECTIVES: &[PublishedObjective] = &[
    PublishedObjective {
        identifier: "5ve4jodybrg8wnky75fp50sbf",
        name: "Asset lifecycle management",
        domain_identifier: ASSET_MANAGEMENT_ID,
        domain_name: ASSET_MANAGEMENT,
        // One common control per stage of the lifecycle this objective's published
        // paragraph enumerates: acquisition, deployment, use and retirement.
        common_controls: &[
            ("aaaq1vhzpslfrk1o4hvgk3ti4", "Asset acquisition"),
            ("eboqkjlzo44e0ia5wba1770tw", "Asset deployment"),
            ("76akwzuoa1j6cxqfsorr12e9k", "Secure asset management"),
            (
                "bme9uq29isv0jdrdy4yj2c702",
                "Asset retirement and disposition",
            ),
        ],
    },
    PublishedObjective {
        identifier: "ags5wgkyvwriix77zegtwhyo9",
        name: "Asset loss prevention, response, and recovery",
        domain_identifier: ASSET_MANAGEMENT_ID,
        domain_name: ASSET_MANAGEMENT,
        // The three common controls published under this objective: losing an asset,
        // damaging one, and the security incident either becomes.
        common_controls: &[
            (
                "4kdrp1fqw2zy6shq95defui2y",
                "Asset loss prevention, recovery, and response",
            ),
            (
                "6rxe3eaojbhyha52wtvahsjxn",
                "Asset damage prevention and response",
            ),
            (
                "6i6o73iaofk1dk6h4rgm6sccd",
                "Asset-related security incident procedures",
            ),
        ],
    },
    PublishedObjective {
        identifier: "3frxxgl64u9kzttiuheywykf7",
        name: "Asset maintenance",
        domain_identifier: ASSET_MANAGEMENT_ID,
        domain_name: ASSET_MANAGEMENT,
        common_controls: &[
            ("ez1ccu4xkxrmlekcvjqo3jk59", "Asset performance management"),
            ("ez4hcdf2fpirhuyojurkval9w", "Asset maintenance scheduling"),
        ],
    },
    PublishedObjective {
        identifier: "14qj3s9axkdma059lhktc14dx",
        name: "Change management",
        domain_identifier: "eyrstdrz8kmo28x5j8vgal6au",
        domain_name: "Secure development lifecycle and change management",
        common_controls: &[
            ("1nxqoydhsjdyboc4frsrrmev", "Change request and approval"),
            ("8xy67kb7kojr38ozqpmrm60d7", "Change deployment and automation"),
            (
                "2o50zyyv219ir9r7brjq0y37f",
                "Rollback and recovery procedures",
            ),
            ("bgzs5hpn3gsbmtvdzi7vuu7nb", "Emergency change review"),
        ],
    },
    PublishedObjective {
        identifier: "90gifwthorhxhxq7m0rtss98u",
        name: "Asset classification",
        domain_identifier: ASSET_MANAGEMENT_ID,
        domain_name: ASSET_MANAGEMENT,
        common_controls: &[
            ("7encqm6cfsw704eoahh3ujr7y", "Asset valuation"),
            ("3t7xstyxx02pdwx9qas1h0quh", "Asset sensitivity"),
            ("dlukqke1ha1ean2txc1juehes", "Asset criticality"),
            (
                "5m9jk2j28m8m4nr53u4c3br0h",
                "Asset classification and categorization",
            ),
            ("gr180qll01cux9k41amnf002", "Asset labeling"),
        ],
    },
    PublishedObjective {
        identifier: "ad11p1961s8erra9m185wa1nn",
        name: "Asset inventory management",
        domain_identifier: ASSET_MANAGEMENT_ID,
        domain_name: ASSET_MANAGEMENT,
        common_controls: &[
            (
                "1ukpmkewk4i92tjmhsvewi4y7",
                "Inventory of authorized assets and automated discovery",
            ),
            (
                "d4s7ik8fgv8082v3x31hifzcc",
                "Asset inventory reconciliation and audit",
            ),
            ("c0qrxhefhmxkbq22tiejp3enn", "Unauthorized asset management"),
            ("1tejgq26c0djpzgskw31uscm4", "Asset ownership assignment"),
            ("5u2qgwuw3z1y0lrof60yf6264", "Asset inventory attributes"),
            (
                "ec1fxlvgtcxlf2nzremqcca7r",
                "Asset decommissioning and inventory removal",
            ),
            ("eg1hxxu2e77a7w2wv79quwaxl", "Software inventory management"),
        ],
    },
];

/// A common control published under an objective whose whole set the catalog does not
/// record: the same published-identity rule holds for it, so it is checked by the
/// per-entry test even though no exact-set assertion can be made for its objective.
///
/// Source: the Control Catalog's own published ontology, which names this entry
/// `arn:{PARTITION}:controlcatalog:::common-control/blplx8cx7l3qy5jd6eryax5e7`
/// "Development testing".
const PUBLISHED_ENTRY_OF_AN_UNRECORDED_SET: (&str, &str) =
    ("blplx8cx7l3qy5jd6eryax5e7", "Development testing");

/// Every page size a published set is walked with: the whole set at once, one entry at a
/// time (so the filter's `NextToken` is resumed), and a size that splits the larger rows'
/// sets across pages unevenly.
const PAGE_SIZES: [Option<u32>; 3] = [None, Some(1), Some(2)];

/// The common controls an objective filter returns, drained across pages.
async fn common_controls_of(
    emulator: &Emulator,
    objective_arn: &str,
    page_size: Option<u32>,
) -> Vec<Value> {
    let body =
        format!(r#"{{"CommonControlFilter":{{"Objectives":[{{"Arn":"{objective_arn}"}}]}}}}"#);
    drain(emulator, "/common-controls", "CommonControls", page_size, &body).await
}

/// The `Arn` of every item, as a set.
fn arns(items: &[Value]) -> BTreeSet<&str> {
    items
        .iter()
        .filter_map(|item| at_path(item, &["Arn"]))
        .collect()
}

/// An objective filter enumerates EXACTLY the common controls published under that
/// objective, each under its published ARN and name and reporting the domain and
/// objective that own it — at every page size.
#[tokio::test]
async fn an_objective_filter_enumerates_exactly_its_published_common_controls() {
    let emulator = Emulator::new();

    for row in PUBLISHED_OBJECTIVES {
        let objective_arn = catalog_kind_arn(OBJECTIVE, row.identifier);
        let domain_arn = catalog_kind_arn(DOMAIN, row.domain_identifier);
        let expected: BTreeSet<String> = row
            .common_controls
            .iter()
            .map(|(identifier, _)| catalog_kind_arn(COMMON_CONTROL, identifier))
            .collect();

        for page_size in PAGE_SIZES {
            let case = format!("objective {} maxResults={page_size:?}", row.identifier);
            let served = common_controls_of(&emulator, &objective_arn, page_size).await;
            let served_arns: BTreeSet<String> =
                arns(&served).into_iter().map(str::to_string).collect();
            assert_eq!(
                served_arns, expected,
                "case {case}: expected exactly the published common controls {expected:?}, \
                 got {served_arns:?}"
            );

            for (identifier, name) in row.common_controls {
                let arn = catalog_kind_arn(COMMON_CONTROL, identifier);
                let entry = served
                    .iter()
                    .find(|item| at_path(item, &["Arn"]) == Some(arn.as_str()))
                    .unwrap_or_else(|| {
                        panic!("case {case}: {arn} is served, so it can be looked up")
                    });
                assert_eq!(
                    at_path(entry, &["Name"]),
                    Some(*name),
                    "case {case} {identifier}: expected the published name {name:?}, got {:?}",
                    entry["Name"]
                );
                assert!(
                    at_path(entry, &["Description"]).is_some_and(|text| !text.is_empty()),
                    "case {case} {identifier}: a published common control carries its \
                     description, got {:?}",
                    entry["Description"]
                );
                assert_eq!(
                    at_path(entry, &["Objective", "Arn"]),
                    Some(objective_arn.as_str()),
                    "case {case} {identifier}: expected the objective filtered on, got {:?}",
                    entry["Objective"]
                );
                assert_eq!(
                    at_path(entry, &["Objective", "Name"]),
                    Some(row.name),
                    "case {case} {identifier}: expected the objective's published name {:?}, \
                     got {:?}",
                    row.name,
                    entry["Objective"]
                );
                assert_eq!(
                    at_path(entry, &["Domain", "Arn"]),
                    Some(domain_arn.as_str()),
                    "case {case} {identifier}: expected the owning domain {domain_arn}, got {:?}",
                    entry["Domain"]
                );
                assert_eq!(
                    at_path(entry, &["Domain", "Name"]),
                    Some(row.domain_name),
                    "case {case} {identifier}: expected the domain's published name {:?}, got {:?}",
                    row.domain_name,
                    entry["Domain"]
                );
            }
        }
    }
}

/// Every recorded published common control is served under its published ARN by the
/// UNFILTERED listing too, identically to the filtered one: the ARN a client harvests
/// resolves to one entry wherever it reads the library.
#[tokio::test]
async fn a_published_common_control_arn_resolves_across_the_whole_library() {
    let emulator = Emulator::new();
    let whole = drain(&emulator, "/common-controls", "CommonControls", None, "{}").await;
    assert!(
        !whole.is_empty(),
        "case library: the catalog must publish its common controls"
    );

    // Every recorded entry: the four objectives' sets, plus one entry published under an
    // objective whose whole set the catalog does not record.
    let recorded = PUBLISHED_OBJECTIVES
        .iter()
        .flat_map(|row| row.common_controls.iter().copied())
        .chain(std::iter::once(PUBLISHED_ENTRY_OF_AN_UNRECORDED_SET));

    for (identifier, name) in recorded {
        let arn = catalog_kind_arn(COMMON_CONTROL, identifier);
        let case = format!("common control {identifier}");
        let entry = whole
            .iter()
            .find(|item| at_path(item, &["Arn"]) == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the unfiltered library must publish {arn}, got {:?}",
                    arns(&whole)
                )
            });
        assert_eq!(
            at_path(entry, &["Name"]),
            Some(name),
            "case {case}: expected the published name {name:?}, got {:?}",
            entry["Name"]
        );
        // The identifier the served ARN carries is lower-case alphanumeric, as the modeled
        // `CommonControlArn` pattern (`common-control/[0-9a-z]+`) admits.
        let served = at_path(entry, &["Arn"]).unwrap_or_default();
        let served_identifier = served
            .strip_prefix("arn:aws:controlcatalog:::common-control/")
            .unwrap_or_else(|| panic!("case {case}: expected the global form, got {served:?}"));
        assert!(
            served_identifier
                .chars()
                .all(|character| character.is_ascii_lowercase() || character.is_ascii_digit()),
            "case {case}: a published identifier is lower-case alphanumeric, got \
             {served_identifier:?}"
        );

        // The entry the objective filter serves is the same record, member for member.
        let objective_arn = at_path(entry, &["Objective", "Arn"])
            .unwrap_or_else(|| panic!("case {case}: the entry reports its objective"))
            .to_string();
        let filtered = common_controls_of(&emulator, &objective_arn, None).await;
        let through_filter = filtered
            .iter()
            .find(|item| at_path(item, &["Arn"]) == Some(arn.as_str()))
            .unwrap_or_else(|| {
                panic!(
                    "case {case}: the objective {objective_arn} it reports must serve it, got {:?}",
                    arns(&filtered)
                )
            });
        assert_eq!(
            through_filter, entry,
            "case {case}: the filtered and the unfiltered listing must report one record"
        );
    }
}

/// A common control's `Name` and `Description` are the catalog's PUBLISHED text for that
/// ARN, served byte for byte: the catalog is a publication, so the seeded record holds
/// the published string and the handler and serializer pass it through — neither clipped
/// to its first sentence, nor re-worded, nor composed from the entry's name — identically
/// through the unfiltered listing and through its objective's filter, at every page size.
///
/// The rows are the common-control rows of `published_catalog_identity`'s
/// `PUBLISHED_ENTRIES`, the one table of published catalog text this test target holds
/// (which is where the published wording is checked for the domain and objective kinds);
/// what this test adds for them is the objective-filter cross-check a client that walks
/// the taxonomy depends on.
#[tokio::test]
async fn a_recorded_common_control_is_served_with_the_published_text_for_its_arn() {
    let emulator = Emulator::new();
    let recorded: Vec<_> = crate::published_catalog_identity::PUBLISHED_ENTRIES
        .iter()
        .filter(|row| row.kind == COMMON_CONTROL)
        .collect();
    assert!(
        !recorded.is_empty(),
        "case table: the published catalog text table must record common controls"
    );

    // The per-objective sets above carry the identifier SET each objective publishes and
    // name its entries; the published text lives in the one table. Every recorded record
    // belongs to one of those sets and the two must name it identically, so the tables
    // cannot drift apart unnoticed.
    for row in &recorded {
        let case = format!("common control {}", row.identifier);
        let in_a_set = PUBLISHED_OBJECTIVES
            .iter()
            .flat_map(|objective| objective.common_controls.iter())
            .find(|(identifier, _)| *identifier == row.identifier);
        let (_, name) = in_a_set.unwrap_or_else(|| {
            panic!(
                "case {case}: a recorded common control belongs to one of the objectives' \
                 published sets"
            )
        });
        assert_eq!(
            *name, row.name,
            "case {case}: the objective's set and the published text table must name one \
             record, got {name:?} and {:?}",
            row.name
        );
    }

    for page_size in PAGE_SIZES {
        let whole = drain(
            &emulator,
            "/common-controls",
            "CommonControls",
            page_size,
            "{}",
        )
        .await;

        for row in &recorded {
            let arn = catalog_kind_arn(COMMON_CONTROL, row.identifier);
            let case = format!("common control {} maxResults={page_size:?}", row.identifier);
            let entry = whole
                .iter()
                .find(|item| at_path(item, &["Arn"]) == Some(arn.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: the library must publish {arn}, got {:?}",
                        arns(&whole)
                    )
                });

            assert_eq!(
                at_path(entry, &["Name"]),
                Some(row.name),
                "case {case}: expected the published name {:?}, got {:?}",
                row.name,
                entry["Name"]
            );
            match row.description {
                Some(published) => assert_eq!(
                    at_path(entry, &["Description"]),
                    Some(published),
                    "case {case}: expected the published description {published:?}, got {:?}",
                    entry["Description"]
                ),
                None => assert!(
                    at_path(entry, &["Description"]).is_some_and(|text| !text.is_empty()),
                    "case {case}: a published common control carries its description, \
                     got {:?}",
                    entry["Description"]
                ),
            }

            // The same text, read the way a client that walks the taxonomy reads it: the
            // objective filter must report the one record, member for member.
            let objective_arn = at_path(entry, &["Objective", "Arn"])
                .unwrap_or_else(|| panic!("case {case}: the entry reports its objective"))
                .to_string();
            let filtered = common_controls_of(&emulator, &objective_arn, page_size).await;
            let through_filter = filtered
                .iter()
                .find(|item| at_path(item, &["Arn"]) == Some(arn.as_str()))
                .unwrap_or_else(|| {
                    panic!(
                        "case {case}: the objective {objective_arn} it reports must serve it, \
                         got {:?}",
                        arns(&filtered)
                    )
                });
            assert_eq!(
                through_filter, entry,
                "case {case}: the filtered and the unfiltered listing must report one record"
            );
        }
    }
}
