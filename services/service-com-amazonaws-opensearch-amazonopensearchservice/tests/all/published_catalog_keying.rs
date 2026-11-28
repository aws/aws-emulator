// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the two published catalogs of this service report: the
//! instance-type listing `ListInstanceTypeDetails` publishes for an engine release, and
//! the offering catalog `DescribeReservedInstanceOfferings` publishes for the Region.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the per-operation calls from the suites that own those operations
//! ([`super::instance_type_catalog`], [`super::reserved_instance_offerings`]); this suite
//! adds no second copy of them, and states the rules those suites do not:
//!
//! * a published catalog reports the catalog the SERVICE publishes. The
//!   `OpenSearchPartitionInstanceType` value set this model carries is an older SNAPSHOT
//!   of that catalog — it declares neither the families the service has published since
//!   nor their sizes — so it bounds what a REQUEST may carry and not what a read reports:
//!   both catalogs therefore carry entries for instance types outside it, and the offering
//!   catalog spans the whole published catalog times each term times each payment option;
//! * the instance-type listing is KEYED on the `EngineVersion` the request names: a
//!   release publishes the generations it offers and none of the ones that postdate it, so
//!   a release predating a hardware generation publishes no type of it while a later one
//!   publishes every type of it — over a table of generations and of releases either side
//!   of each one, including the oldest Elasticsearch releases, whose listing is the
//!   previous generations alone;
//! * `InstanceTypeDetails$EncryptionEnabled` reports a capability OF THE INSTANCE TYPE
//!   ("whether encryption at rest and node-to-node encryption are supported for the
//!   instance type"), withheld only on a release older than the one the service offers
//!   encryption from — so a listing never denies it across every entry of a release that
//!   has it;
//! * the offering catalog is published in ONE order, a function of the published records
//!   alone, and that order interleaves instance type, term and payment option: the page a
//!   `MaxResults` cuts therefore spans several instance families and both terms rather
//!   than one type's corner of the catalog.

use std::collections::{BTreeMap, BTreeSet};

use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::published_instance_types;

use super::domain_lifecycle::{string_member, Emulator};
use super::instance_type_catalog::{family, flag, instance_types, whole_listing, UNENCRYPTED_FAMILIES};
use super::instance_type_limits::describe_limits;
use super::reserved_instance_offerings::{describe, whole_catalog};

/// The largest page `MaxResults` can ask for: it is `@range(max: 100)`
/// (`smithy_query(query="operation",operation="DescribeReservedInstanceOfferings")`).
const MAX_PAGE_SIZE: usize = 100;

/// One row of the generation table: an instance type of the generation, a release that
/// offers the generation, and a release that predates it — `None` where the catalog offers
/// the generation on every release it publishes.
///
/// Source of the rows: the instance types the service's own catalog publishes
/// (`crate::state::published_instance_types`, which both catalog reads serve), one per
/// generation the catalog gates on a release, including generations the report that asked
/// for this suite did not cite. The releases are the ones `ListVersions` advertises.
const GENERATIONS: [(&str, &str, Option<&str>); 8] = [
    // The previous generations, which the catalog offers on every release it publishes.
    ("r4.2xlarge.search", "Elasticsearch_1.5", None),
    ("c4.2xlarge.search", "Elasticsearch_1.5", None),
    // The storage-optimized family that succeeded the previous generation's.
    ("i3.4xlarge.search", "Elasticsearch_1.5", None),
    // The fifth generation of the hot-tier line, and the burstable family of it.
    ("m5.xlarge.search", "Elasticsearch_5.1", Some("Elasticsearch_2.3")),
    ("t3.medium.search", "Elasticsearch_5.1", Some("Elasticsearch_1.5")),
    // The Graviton2 generation.
    ("r6gd.4xlarge.search", "Elasticsearch_7.9", Some("Elasticsearch_7.8")),
    // The OpenSearch-optimized line, and the seventh-generation Intel families — two
    // generations the modelled snapshot does not declare every family of.
    ("or1.4xlarge.search", "OpenSearch_2.11", Some("OpenSearch_2.9")),
    ("c7i.16xlarge.search", "OpenSearch_2.13", Some("OpenSearch_2.11")),
];

/// The releases the encryption table spans: the oldest Elasticsearch releases, the
/// release the service offers encryption from and one either side of it, the engine-line
/// crossing and a current OpenSearch release.
const ENCRYPTION_RELEASES: [(&str, bool); 7] = [
    ("Elasticsearch_1.5", false),
    ("Elasticsearch_2.3", false),
    ("Elasticsearch_5.1", true),
    ("Elasticsearch_5.6", true),
    ("Elasticsearch_7.10", true),
    ("OpenSearch_1.0", true),
    ("OpenSearch_2.19", true),
];

/// Every entry of one release's listing, keyed by its instance type.
async fn listing_by_type(emulator: &Emulator, release: &str) -> BTreeMap<String, serde_json::Value> {
    whole_listing(emulator, release, "")
        .await
        .into_iter()
        .map(|entry| {
            let instance_type = string_member(&entry, "InstanceType", release);
            (instance_type, entry)
        })
        .collect()
}

/// The sizes the published catalog offers one family in.
fn published_family(name: &str) -> Vec<&'static str> {
    published_instance_types()
        .into_iter()
        .filter(|instance_type| family(instance_type) == name)
        .collect()
}

#[tokio::test]
async fn a_published_catalog_reports_the_catalog_the_service_publishes() {
    let emulator = Emulator::new();

    // The instance-type listing of a release carries the types that release offers,
    // whether or not the model's older snapshot declares them — asserted over a table of
    // releases spanning both engine lines, each case naming the types it found outside
    // the snapshot.
    for release in ["Elasticsearch_7.10", "OpenSearch_2.13", "OpenSearch_2.19"] {
        let reported = instance_types(&whole_listing(&emulator, release, "").await);
        let beyond: Vec<&String> = reported
            .iter()
            .filter(|instance_type| {
                !OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type.as_str())
            })
            .collect();
        assert!(
            !beyond.is_empty(),
            "case {release}: expected the listing to publish types the modelled snapshot \
             does not declare, got {} entries",
            reported.len()
        );
    }

    // And nothing the snapshot DOES declare is dropped: the published catalog is a
    // superset of it, never a replacement.
    let published_set: BTreeSet<&str> = published_instance_types().into_iter().collect();
    for modelled in OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES {
        assert!(
            published_set.contains(modelled),
            "case {modelled}: expected the published catalog to carry every instance type \
             the model declares, got {} types",
            published_set.len()
        );
    }

    // The offering catalog spans the WHOLE published catalog: one entry per published
    // instance type, times each term, times each payment option — so an instance type a
    // client can run is one it can reserve, including the types outside the snapshot.
    let catalog = whole_catalog(&emulator, "the offering catalog").await;
    let reserved: BTreeSet<String> = catalog
        .iter()
        .map(|offering| string_member(offering, "InstanceType", "offering"))
        .collect();
    let published: BTreeSet<&str> = published_instance_types().into_iter().collect();
    assert_eq!(
        reserved.len(),
        published.len(),
        "case the offering catalog: expected an offering for every published instance \
         type, got {} of {}",
        reserved.len(),
        published.len()
    );
    for instance_type in &published {
        assert!(
            reserved.contains(*instance_type),
            "case {instance_type}: expected the offering catalog to publish this instance \
             type, got {} types",
            reserved.len()
        );
    }
    let beyond: Vec<&String> = reserved
        .iter()
        .filter(|instance_type| {
            !OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type.as_str())
        })
        .collect();
    assert!(
        !beyond.is_empty(),
        "case the offering catalog: expected offerings for types the modelled snapshot \
         does not declare, got {} types",
        reserved.len()
    );

    // And a type outside the snapshot is published by the SIBLING catalog read too — while
    // what a REQUEST may name stays the snapshot the boundary enforces, which is the other
    // half of rule 1: naming such a type on a member that targets the enum is refused
    // there, and the refusal states the set it does accept rather than the wider catalog
    // ([`super::published_instance_type_value_set`] owns that clause), so a client is never
    // offered a value this emulator would then refuse.
    for (instance_type, release) in [
        ("m7i.2xlarge.search", "OpenSearch_2.13"),
        ("i7i.4xlarge.search", "OpenSearch_2.19"),
        ("r8g.16xlarge.search", "OpenSearch_2.19"),
    ] {
        let case = format!("{instance_type} on {release}");
        // The listing publishes it — read off the answer, not off a table of this suite's.
        let listed = instance_types(&whole_listing(&emulator, release, "").await);
        assert!(
            listed.contains(&instance_type.to_string()),
            "case {case}: expected the release's listing to publish it, got {} entries",
            listed.len()
        );
        let answer = describe_limits(&emulator, release, instance_type, "").await;
        assert_eq!(
            answer.status, 400,
            "case {case}: expected the modelled snapshot to bound what a request may name, \
             got {} {:?}",
            answer.status, answer.body
        );
        let message = answer.body["message"].as_str().unwrap_or_default().to_owned();
        let accepted = message
            .rsplit_once("enum value set: [")
            .and_then(|(_, tail)| tail.strip_suffix(']'))
            .unwrap_or_else(|| panic!("case {case}: expected a bracketed value set, got {message:?}"))
            .split(", ")
            .collect::<BTreeSet<&str>>();
        assert!(
            !accepted.contains(instance_type),
            "case {case}: expected the accepted set to leave out the type the boundary \
             refuses, got {} values",
            accepted.len()
        );
        assert!(
            accepted.contains("m5.large.search"),
            "case {case}: expected the accepted set to state the types a request may name, \
             got {} values",
            accepted.len()
        );
    }
}

#[tokio::test]
async fn the_instance_type_listing_is_keyed_on_the_engine_version() {
    let emulator = Emulator::new();

    let mut gated = 0_usize;
    for (instance_type, offered_on, predating) in GENERATIONS {
        let case = format!("{instance_type} on {offered_on}");
        // A release that offers the generation publishes the WHOLE family — every size
        // the catalog offers it in — so the keying is about a generation and not about
        // one sampled type.
        let listed = instance_types(&whole_listing(&emulator, offered_on, "").await);
        for sibling in published_family(family(instance_type)) {
            assert!(
                listed.contains(&sibling.to_string()),
                "case {case}: expected the release to publish {sibling}, which its \
                 generation offers, got {} entries",
                listed.len()
            );
        }

        // A release that PREDATES the generation publishes no type of it at all.
        let Some(predating) = predating else {
            continue;
        };
        gated += 1;
        let earlier = instance_types(&whole_listing(&emulator, predating, "").await);
        for sibling in published_family(family(instance_type)) {
            assert!(
                !earlier.contains(&sibling.to_string()),
                "case {sibling} on {predating}: expected a release predating the \
                 generation to publish none of it, got {} entries",
                earlier.len()
            );
        }
    }
    assert!(
        gated >= 5,
        "case the generation table: expected several generations the catalog gates on a \
         release, got {gated}"
    );

    // The oldest releases are keyed too, and their listing is the one the generations
    // above are gated against: the previous generations and the storage-optimized family
    // that succeeded the previous generation's, and nothing newer.
    for release in ["Elasticsearch_1.5", "Elasticsearch_2.3"] {
        let listed = instance_types(&whole_listing(&emulator, release, "").await);
        let families: BTreeSet<&str> = listed.iter().map(|listed| family(listed)).collect();
        assert_eq!(
            families,
            ["c4", "d2", "i2", "i3", "m3", "m4", "r3", "r4", "t2"]
                .into_iter()
                .collect::<BTreeSet<&str>>(),
            "case {release}: expected the release's own generations, got {families:?}"
        );
        assert!(
            listed.len() < published_instance_types().len() / 2,
            "case {release}: expected the oldest release to publish a fraction of the \
             catalog, got {} of {}",
            listed.len(),
            published_instance_types().len()
        );
    }
}

#[tokio::test]
async fn the_encryption_member_reports_the_instance_types_own_support() {
    let emulator = Emulator::new();

    // One row per instance family the two features ARE available on, spanning the classes
    // and the size ladder, plus the two families they are available on for no release.
    let types = [
        "r5.large.search",
        "m4.xlarge.search",
        "c4.2xlarge.search",
        "i3.4xlarge.search",
        "r4.2xlarge.search",
        "d2.8xlarge.search",
        "m3.medium.search",
        "t2.medium.search",
    ];

    let mut supported = 0_usize;
    let mut withheld = 0_usize;
    for (release, offers_encryption) in ENCRYPTION_RELEASES {
        let listing = listing_by_type(&emulator, release).await;
        // The release's listing is never a blanket denial: a release that offers
        // encryption reports it for the families that provide it.
        let mut reported_on_release = 0_usize;
        for instance_type in types {
            // Each row's premise — that the release offers the type — is read off the
            // ANSWER, so a release that does not offer a family contributes no case.
            let Some(entry) = listing.get(instance_type) else {
                continue;
            };
            let case = format!("{instance_type} on {release}");
            let expected =
                offers_encryption && !UNENCRYPTED_FAMILIES.contains(&family(instance_type));
            let reported = flag(entry, "EncryptionEnabled", &case);
            assert_eq!(
                reported, expected,
                "case {case}: expected the instance type's own encryption support \
                 ({expected}), got {reported}"
            );
            if reported {
                reported_on_release += 1;
                supported += 1;
            } else {
                withheld += 1;
            }
        }
        if offers_encryption {
            assert!(
                reported_on_release > 0,
                "case {release}: expected a release that offers encryption to report it \
                 for the families that provide it, got none of {} cases",
                types.len()
            );
        }
    }

    // The table really spans both sides of the rule.
    assert!(
        supported > 0 && withheld > 0,
        "case the encryption table: expected rows on both sides of the rule, got \
         {supported} supported and {withheld} withheld"
    );
}

#[tokio::test]
async fn the_offering_catalog_is_published_in_one_interleaved_order() {
    // Two emulators, each with its own engine and store: an order that belonged to the
    // run rather than to the catalog would differ between them.
    let first = Emulator::new();
    let second = Emulator::new();
    let catalog = whole_catalog(&first, "the published order").await;
    assert_eq!(
        catalog,
        whole_catalog(&second, "the published order").await,
        "case two emulators: expected one published order, got {} offerings",
        catalog.len()
    );
    assert_eq!(
        catalog,
        whole_catalog(&first, "the published order").await,
        "case a repeated read: expected the same offerings in the same order, got {} \
         offerings",
        catalog.len()
    );

    // The order interleaves the three things an offering is of, so a page a `MaxResults`
    // cuts spans several instance families, both terms and every payment option —
    // asserted over a table of page sizes, the largest being the cap itself.
    for page_size in [10_usize, 50, MAX_PAGE_SIZE] {
        let case = format!("page size {page_size}");
        let answer = describe(&first, &format!("maxResults={page_size}")).await;
        let page = answer.body["ReservedInstanceOfferings"]
            .as_array()
            .cloned()
            .unwrap_or_else(|| panic!("case {case}: expected a page of offerings, got {:?}", answer.body));
        assert_eq!(
            page.len(),
            page_size.min(catalog.len()),
            "case {case}: expected a full page, got {} offerings",
            page.len()
        );
        assert_eq!(
            page,
            catalog[..page.len()].to_vec(),
            "case {case}: expected the head of the published catalog, got {} offerings",
            page.len()
        );

        let families: BTreeSet<&str> = page
            .iter()
            .map(|offering| family(offering["InstanceType"].as_str().unwrap_or_default()))
            .collect();
        assert!(
            families.len() > 1,
            "case {case}: expected a page to span several instance families, got \
             {families:?}"
        );
        // Both terms and every payment option are asserted on the pages large enough for
        // the claim to be about the ORDER rather than about how many rows fit: a page of
        // ten rows of a catalog spanning every type, term and option is a sample.
        if page_size < 50 {
            continue;
        }
        let terms: BTreeSet<i64> = page
            .iter()
            .map(|offering| offering["Duration"].as_i64().unwrap_or_default())
            .collect();
        assert_eq!(
            terms.len(),
            2,
            "case {case}: expected a page to span both published terms, got {terms:?}"
        );
        let options: BTreeSet<String> = page
            .iter()
            .map(|offering| string_member(offering, "PaymentOption", &case))
            .collect();
        assert_eq!(
            options.len(),
            3,
            "case {case}: expected a page to span every payment option, got {options:?}"
        );
    }
}
