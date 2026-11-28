// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CAPABILITY members of the published instance-type catalog:
//! what `ListInstanceTypeDetails` reports an instance type supports.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The harness comes from [`super::domain_lifecycle`]
//! and the listing helpers from [`super::instance_type_catalog`]; this suite adds no
//! second copy of either.
//!
//! The rule under test: every capability member of an entry is documented "supported for
//! the instance type", so each reports a capability OF THAT TYPE —
//!
//! * a capability the INSTANCE provides (the warm tier) is reported off the instance
//!   family alone: the same answer on every release that offers the type, including a
//!   release older than the feature, and the same answer the create path refuses a
//!   configuration on;
//! * a capability the ENGINE provides (Amazon Cognito authentication, application
//!   logging, fine-grained access control) is reported off the release that introduced
//!   it, so it is reported off wherever the engine does not have it and stays reported
//!   once it is;
//! * a capability that needs BOTH (encryption at rest together with node-to-node
//!   encryption: hardware that provides them and an engine release that has them) is
//!   reported off the pairing, so a family that offers it is still reported unsupported
//!   on a release older than the features.
//!
//! The rows differ along what the rule generalises over: the instance family (burstable,
//! general purpose, compute-, memory- and storage-optimized, OpenSearch-optimized, the
//! unencrypted previous generation and the warm tier), the size within a family, and the
//! engine release — both engines, and both sides of each release a feature arrived in.

use std::collections::BTreeMap;

use serde_json::Value;

use super::domain_lifecycle::Emulator;
// `ENCRYPTION_RELEASE` is the release BOTH features `EncryptionEnabled` reports are
// available from — AWS documents encryption at rest from 5.1 and node-to-node encryption
// from 6.0 — stated once in the suite that owns the listing's rules.
use super::instance_type_catalog::{list_type, whole_listing, ENCRYPTION_RELEASE};

/// The engine releases every table below spans: the oldest Elasticsearch release, each
/// release one of the features arrived in, one either side of it, and OpenSearch
/// releases from the first to a current one.
const RELEASES: [&str; 12] = [
    "Elasticsearch_1.5",
    "Elasticsearch_5.1",
    "Elasticsearch_6.0",
    "Elasticsearch_6.7",
    "Elasticsearch_6.8",
    "Elasticsearch_7.9",
    "Elasticsearch_7.10",
    "OpenSearch_1.0",
    "OpenSearch_2.11",
    "OpenSearch_2.13",
    "OpenSearch_2.17",
    "OpenSearch_2.19",
];

/// The Elasticsearch release Amazon Cognito authentication and application logging are
/// available from. Source: the releases AWS documents those two features for, which the
/// service's own catalog reports them off.
const ENGINE_FEATURE_RELEASE: (u32, u32) = (5, 1);

/// The Elasticsearch release fine-grained access control is available from.
const ADVANCED_SECURITY_RELEASE: (u32, u32) = (6, 7);

/// One row of the capability table: an instance type, whether its family is one the two
/// encryption features are available on at all, and whether its family offers the warm
/// tier.
///
/// The expectations are the FAMILY's: the unencrypted previous generations are the `m3`
/// and `t2` families, and the warm tier is offered on every family except the burstable
/// ones (`t2`, `t3`, `t4g`) — the warm answer being the one the create path reads.
const CAPABILITIES: [(&str, bool, bool); 11] = [
    ("r5.large.search", true, true),
    ("r5.24xlarge.search", true, true),
    ("m3.medium.search", false, true),
    ("m3.2xlarge.search", false, true),
    ("t2.micro.search", false, false),
    ("t3.medium.search", true, false),
    ("t4g.small.search", true, false),
    ("c4.large.search", true, true),
    ("i3.large.search", true, true),
    ("or1.medium.search", true, true),
    ("ultrawarm1.medium.search", true, true),
];

/// An instance type the issue's own cases did not cite, drawn from a generation the
/// catalog gained later: the rule holds for it too.
const UNCITED_TYPE: (&str, bool, bool) = ("r6gd.large.search", true, true);

/// A release read apart as this suite states the rules against it.
fn parse(release: &str) -> (&str, u32, u32) {
    let (engine, numbers) = release
        .split_once('_')
        .unwrap_or_else(|| panic!("the release table holds <Engine>_<X.Y> values, got {release}"));
    let (major, minor) = numbers
        .split_once('.')
        .unwrap_or_else(|| panic!("the release table holds <Engine>_<X.Y> values, got {release}"));
    (
        engine,
        major.parse().expect("a numeric major release"),
        minor.parse().expect("a numeric minor release"),
    )
}

/// Whether a feature an Elasticsearch release introduced is available on `release`.
/// OpenSearch 1.0 forked from Elasticsearch 7.10, so every OpenSearch release has them.
///
/// Shared with the sibling suites that state a release-gated capability.
pub(crate) fn has_engine_feature(release: &str, since: (u32, u32)) -> bool {
    let (engine, major, minor) = parse(release);
    engine == "OpenSearch" || (major, minor) >= since
}

/// A boolean member of one entry, with the case named on failure.
fn flag(entry: &Value, member: &str, case: &str) -> bool {
    entry[member]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a boolean {member}, got {entry:?}"))
}

/// Every entry of one release's listing, keyed by its instance type.
async fn listing_by_type(emulator: &Emulator, release: &str) -> BTreeMap<String, Value> {
    whole_listing(emulator, release, "")
        .await
        .into_iter()
        .map(|entry| {
            let instance_type = entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("case {release}: every entry carries an InstanceType, got {entry:?}"))
                .to_owned();
            (instance_type, entry)
        })
        .collect()
}

/// The table above plus the uncited row, which every case spans.
fn rows() -> Vec<(&'static str, bool, bool)> {
    let mut rows = CAPABILITIES.to_vec();
    rows.push(UNCITED_TYPE);
    rows
}

#[tokio::test]
async fn a_capability_the_instance_provides_is_reported_on_every_release_that_offers_the_type() {
    let emulator = Emulator::new();

    // Each row's premise — that the release offers the type — is read off the ANSWER, so
    // a release that does not offer a family contributes no case rather than a wrong one.
    let mut covered: BTreeMap<&str, usize> = BTreeMap::new();
    for release in RELEASES {
        let listing = listing_by_type(&emulator, release).await;
        for (instance_type, _, warm) in rows() {
            let Some(entry) = listing.get(instance_type) else {
                continue;
            };
            *covered.entry(instance_type).or_default() += 1;
            let case = format!("{instance_type} on {release}");

            let reported = flag(entry, "WarmEnabled", &case);
            assert_eq!(
                reported, warm,
                "case {case}: expected the family's warm-tier support ({warm}) whatever \
                 the release, got {reported}"
            );
        }
    }

    // No row is vacuous, and each of them is answered on more than one release — which
    // is what makes "the same answer whatever the release" a statement about a set.
    for (instance_type, _, _) in rows() {
        let releases = covered.get(instance_type).copied().unwrap_or_default();
        assert!(
            releases > 1,
            "case {instance_type}: expected the published catalog to offer this type on \
             more than one release of the table, got {releases}"
        );
    }
}

#[tokio::test]
async fn a_capability_the_engine_provides_is_reported_off_the_release() {
    let emulator = Emulator::new();

    for release in RELEASES {
        let listing = listing_by_type(&emulator, release).await;
        for (instance_type, encryption, _) in rows() {
            let Some(entry) = listing.get(instance_type) else {
                continue;
            };
            let case = format!("{instance_type} on {release}");

            for member in ["CognitoEnabled", "AppLogsEnabled"] {
                let reported = flag(entry, member, &case);
                let expected = has_engine_feature(release, ENGINE_FEATURE_RELEASE);
                assert_eq!(
                    reported, expected,
                    "case {case}: expected {member} to follow the release ({expected}), \
                     got {reported}"
                );
            }

            // The two encryption features need hardware that provides them AND an engine
            // release that has them, so the pairing is what the member reports.
            let expected = encryption && has_engine_feature(release, ENCRYPTION_RELEASE);
            let reported = flag(entry, "EncryptionEnabled", &case);
            assert_eq!(
                reported, expected,
                "case {case}: expected EncryptionEnabled to follow the family's support \
                 and the release ({expected}), got {reported}"
            );

            // Fine-grained access control needs an engine that has it AND both
            // encryption features, so it is reported only where the two meet.
            let advanced = flag(entry, "AdvancedSecurityEnabled", &case);
            let expected = encryption && has_engine_feature(release, ADVANCED_SECURITY_RELEASE);
            assert_eq!(
                advanced, expected,
                "case {case}: expected fine-grained access control to follow the release \
                 and the family's encryption support ({expected}), got {advanced}"
            );
            assert!(
                !advanced || flag(entry, "EncryptionEnabled", &case),
                "case {case}: fine-grained access control is never reported without \
                 encryption"
            );
        }
    }
}

#[tokio::test]
async fn an_entry_reports_the_instances_capabilities_on_a_release_without_the_engine_features() {
    let emulator = Emulator::new();

    // The combination the two kinds of capability produce together, which neither rule
    // alone states: on a release OLDER than fine-grained access control, an entry still
    // reports the warm-tier support its instance type provides, reports fine-grained
    // access control off, and reports the two encryption features exactly where the
    // release has them — off on a release before node-to-node encryption, on from it.
    // One row per family group that offers both capabilities, read through the
    // `instanceType` filter so the case names one entry.
    for release in ["Elasticsearch_5.1", "Elasticsearch_6.0"] {
        for instance_type in [
            "r5.large.search",
            "m5.large.search",
            "c5.xlarge.search",
            "i3.large.search",
        ] {
            let case = format!("{instance_type} on {release}");
            // The narrowed read is made the way the service requires it
            // ([`super::instance_type_catalog::list_type`]): naming the type asks for its
            // Availability Zone information, which the retrieve-AZ flag has to enable.
            let answer = list_type(&emulator, release, instance_type).await;
            assert_eq!(
                answer.status, 200,
                "case {case}: expected HTTP 200, got {} {:?}",
                answer.status, answer.body
            );
            let entries = answer.body["InstanceTypeDetails"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                entries.len(),
                1,
                "case {case}: the filter names one instance type, got {entries:?}"
            );
            let entry = &entries[0];
            for (member, expected) in [
                (
                    "EncryptionEnabled",
                    has_engine_feature(release, ENCRYPTION_RELEASE),
                ),
                ("WarmEnabled", true),
                ("CognitoEnabled", true),
                ("AppLogsEnabled", true),
                ("AdvancedSecurityEnabled", false),
            ] {
                let reported = flag(entry, member, &case);
                assert_eq!(
                    reported, expected,
                    "case {case}: expected {member} {expected}, got {reported}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_reported_capabilities_do_not_change_what_a_later_caller_sees() {
    let emulator = Emulator::new();

    // The catalog is published BY the service, so reading it twice answers identically.
    let release = "OpenSearch_2.19";
    let first = listing_by_type(&emulator, release).await;
    let second = listing_by_type(&emulator, release).await;
    assert_eq!(
        first, second,
        "case {release}: expected a pure read to report the same catalog twice"
    );
}
