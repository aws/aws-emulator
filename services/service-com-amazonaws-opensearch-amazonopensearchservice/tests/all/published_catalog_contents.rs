// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for WHAT the two published catalogs of this service carry: the
//! per-entry capabilities and node roles of the instance-type catalog, and the span of
//! the Reserved Instance offering catalog a walk of its pages reports.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`], the listing readers from
//! [`super::instance_type_catalog`], the limit reader from
//! [`super::instance_type_limits`] and the offering readers from
//! [`super::reserved_instance_offerings`]; this suite adds no second copy of any of them.
//!
//! The rules under test, each over a SET of inputs rather than one recorded call:
//!
//! * `InstanceTypeDetails$EncryptionEnabled` reports "whether encryption at rest and
//!   node-to-node encryption are supported for the instance type"
//!   (`smithy_query(query="doc",shape="InstanceTypeDetails",member="EncryptionEnabled")`),
//!   and each of the two needs BOTH an instance family that provides it and an engine
//!   release that has it — AWS documents encryption at rest from 5.1 and node-to-node
//!   encryption from 6.0 — so a release older than the later of the two reports the pair
//!   unsupported whatever the family, and fine-grained access control, which needs them,
//!   is never reported where they are not;
//! * `InstanceTypeDetails$InstanceRole` is the set of node roles an instance of the
//!   entry's FAMILY can be provisioned in, so a family the service publishes for the data
//!   tier alone reports the data role and nothing else — and the limit catalog, which
//!   publishes limits per role, publishes a block for exactly those roles;
//! * `DescribeReservedInstanceOfferings` publishes the WHOLE offering catalog of the
//!   Region: every instance type it publishes, each with both published terms and each
//!   payment option, read page by page as the service answers it.

use std::collections::BTreeSet;

use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::RESERVED_INSTANCE_PAYMENT_OPTION_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::RESERVED_INSTANCE_TERMS;

use super::domain_lifecycle::Emulator;
use super::instance_type_capability_flags::has_engine_feature;
use super::instance_type_catalog::{
    family, flag, instance_types, list_type, narrowed, roles, whole_listing, ENCRYPTION_RELEASE,
    UNENCRYPTED_FAMILIES,
};
use super::instance_type_limits::{describe_limits, limits_by_role};
use super::reserved_instance_offerings::whole_catalog;

/// The engine releases the capability table spans: both engines, and both sides of the
/// release the two encryption features are available from.
const RELEASES: [&str; 8] = [
    "Elasticsearch_1.5",
    "Elasticsearch_2.3",
    "Elasticsearch_5.1",
    "Elasticsearch_5.6",
    "Elasticsearch_6.0",
    "Elasticsearch_7.10",
    "OpenSearch_1.0",
    "OpenSearch_2.19",
];

// The release the features `EncryptionEnabled` reports are offered from
// (`ENCRYPTION_RELEASE`), and the families neither is available on at all
// (`UNENCRYPTED_FAMILIES`), are imported above: the rule is stated once, so a correction
// to it cannot reach one suite only.

/// The instance types the capability table asks about: a memory-optimized previous
/// generation (the one an old release's recorded listing reports unencrypted), a
/// general-purpose and a compute-optimized family, a storage-optimized one, a burstable
/// one, a family neither feature is available on, and the warm tier — so the rule is
/// stated over families, not over one cited entry.
const CAPABILITY_TYPES: [&str; 8] = [
    "r4.2xlarge.search",
    "r3.4xlarge.search",
    "m4.xlarge.search",
    "c4.2xlarge.search",
    "i3.large.search",
    "t3.medium.search",
    "m3.medium.search",
    "ultrawarm1.medium.search",
];

/// The families the service publishes for the DATA TIER alone, each with a size of its
/// own so the rows span the size ladder too: the OpenSearch-optimized family, which is the
/// one the model lets a caller name a size of.
const DATA_TIER_ONLY: [&str; 7] = [
    "or1.medium.search",
    "or1.large.search",
    "or1.xlarge.search",
    "or1.2xlarge.search",
    "or1.4xlarge.search",
    "or1.8xlarge.search",
    "or1.16xlarge.search",
];

/// Families that ARE master-eligible, so the rule above is a distinction and not a blanket
/// answer: a storage-optimized generation, a burstable one and both halves of a
/// memory-optimized generation.
const MASTER_ELIGIBLE: [&str; 4] = [
    "i3.4xlarge.search",
    "t4g.medium.search",
    "r6gd.large.search",
    "r6g.2xlarge.search",
];

/// A release that offers every family the role tables above name.
const CURRENT_RELEASE: &str = "OpenSearch_2.19";

#[tokio::test]
async fn the_encryption_member_follows_the_family_and_the_engine_release() {
    let emulator = Emulator::new();

    let mut cases = 0_usize;
    let mut unsupported = 0_usize;
    for release in RELEASES {
        // Each row's premise — that the release offers the type — is read off the ANSWER,
        // so a release that does not offer a family contributes no case rather than a
        // wrong one.
        let listed = instance_types(&whole_listing(&emulator, release, "").await);
        for instance_type in CAPABILITY_TYPES {
            if !listed.contains(&instance_type.to_string()) {
                continue;
            }
            let case = format!("{instance_type} on {release}");
            let entries = narrowed(&list_type(&emulator, release, instance_type).await, &case);
            assert_eq!(
                entries.len(),
                1,
                "case {case}: the filter names one instance type, got {entries:?}"
            );
            let entry = &entries[0];

            let expected = !UNENCRYPTED_FAMILIES.contains(&family(instance_type))
                && has_engine_feature(release, ENCRYPTION_RELEASE);
            let reported = flag(entry, "EncryptionEnabled", &case);
            assert_eq!(
                reported, expected,
                "case {case}: expected the two encryption features to need the family and \
                 the release ({expected}), got {reported}"
            );

            // Fine-grained access control needs both encryption features, so it is never
            // reported where the pair is not.
            let advanced = flag(entry, "AdvancedSecurityEnabled", &case);
            assert!(
                !advanced || reported,
                "case {case}: fine-grained access control is never reported without \
                 encryption, got {entry:?}"
            );

            cases += 1;
            if !reported {
                unsupported += 1;
            }
        }
    }

    // The table really spans both sides of the rule, so neither branch went untested.
    assert!(
        cases > CAPABILITY_TYPES.len() && unsupported > 0,
        "the table must answer both sides of the rule, got {cases} case(s) of which \
         {unsupported} reported the pair unsupported"
    );
}

#[tokio::test]
async fn a_family_published_for_the_data_tier_alone_reports_the_data_role() {
    let emulator = Emulator::new();

    for instance_type in DATA_TIER_ONLY {
        let case = format!("{instance_type} on {CURRENT_RELEASE}");
        let entries = narrowed(
            &list_type(&emulator, CURRENT_RELEASE, instance_type).await,
            &case,
        );
        assert_eq!(
            entries.len(),
            1,
            "case {case}: the filter names one instance type, got {entries:?}"
        );
        let reported = roles(&entries[0], &case);
        assert_eq!(
            reported,
            vec!["data".to_string()],
            "case {case}: expected a family published for the data tier alone to report \
             that one role, got {reported:?}"
        );

        // The limit catalog publishes limits per role, so it publishes a block for
        // exactly the roles the listing reports: the data role, and no dedicated master.
        let limits = limits_by_role(
            &describe_limits(&emulator, CURRENT_RELEASE, instance_type, "").await,
            &case,
        );
        assert!(
            limits["data"].is_object(),
            "case {case}: expected the data role's limits, got {limits:?}"
        );
        assert!(
            limits["master"].is_null(),
            "case {case}: a family that is never master-eligible publishes no master \
             limits, got {:?}",
            limits["master"]
        );
    }

    // The rule is a distinction: a master-eligible family reports the master role and
    // publishes its limits.
    for instance_type in MASTER_ELIGIBLE {
        let case = format!("{instance_type} on {CURRENT_RELEASE}");
        let entries = narrowed(
            &list_type(&emulator, CURRENT_RELEASE, instance_type).await,
            &case,
        );
        let reported = roles(&entries[0], &case);
        assert!(
            reported.contains(&"master".to_string()),
            "case {case}: expected a master-eligible family to report the master role, \
             got {reported:?}"
        );
        let limits = limits_by_role(
            &describe_limits(&emulator, CURRENT_RELEASE, instance_type, "").await,
            &case,
        );
        assert!(
            limits["master"].is_object(),
            "case {case}: expected the master role's limits, got {limits:?}"
        );
    }
}

#[tokio::test]
async fn the_published_offering_catalog_spans_every_instance_type_term_and_option() {
    let emulator = Emulator::new();

    // The whole catalog the Region publishes, walked page by page the way a client walks
    // it — the reader the offering suite already owns.
    let case = "the unfiltered offering catalog";
    let catalog = whole_catalog(&emulator, case).await;

    // What the catalog has to span, read off the answer: every instance type the
    // service publishes a listing entry for, each with both published terms and each
    // payment option. The instance types are the published catalog's own — taken from the
    // sibling catalog read rather than restated here — so the two catalogs cannot disagree
    // about which instance types exist.
    let reserved: BTreeSet<(String, i64, String)> = catalog
        .iter()
        .map(|offering| {
            (
                offering["InstanceType"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: every offering names its type"))
                    .to_owned(),
                offering["Duration"]
                    .as_i64()
                    .unwrap_or_else(|| panic!("case {case}: every offering names its term")),
                offering["PaymentOption"]
                    .as_str()
                    .unwrap_or_else(|| panic!("case {case}: every offering names its option"))
                    .to_owned(),
            )
        })
        .collect();
    assert_eq!(
        reserved.len(),
        catalog.len(),
        "case {case}: expected one offering per instance type, term and payment option, \
         got {} offerings over {} combinations",
        catalog.len(),
        reserved.len()
    );

    let published = instance_types(&whole_listing(&emulator, CURRENT_RELEASE, "").await);
    for instance_type in &published {
        for term in RESERVED_INSTANCE_TERMS {
            for option in RESERVED_INSTANCE_PAYMENT_OPTION_VALUES {
                let combination = (
                    instance_type.clone(),
                    i64::from(term),
                    (*option).to_owned(),
                );
                assert!(
                    reserved.contains(&combination),
                    "case {case}: expected an offering for {instance_type} / {term}s / \
                     {option} in the walked catalog, got {} offerings",
                    catalog.len()
                );
            }
        }
    }

    // The completeness is not an artifact of a small catalog: the catalog is larger than
    // the largest page `MaxResults` admits, which is the bound a request may name.
    assert!(
        catalog.len() > 100,
        "case {case}: expected a catalog larger than the largest page a caller can ask \
         for, got {} offerings",
        catalog.len()
    );
}
