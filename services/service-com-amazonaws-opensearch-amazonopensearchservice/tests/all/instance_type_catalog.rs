// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ListInstanceTypeDetails`.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The tests assert RULES over sets of inputs, not one recorded call, and they assert
//! them about the ANSWER rather than against a second copy of the service's table:
//!
//! * the listing is the catalog the service publishes for the named release, published
//!   family by family in the documented order — the current generations by family name,
//!   then the warm tier, then the previous generations, each family's types smallest
//!   size first — which is neither the modelled `OpenSearchPartitionInstanceType` value
//!   set nor its order: the catalog carries current-generation families that enum does
//!   not list at all, carries every value it does list, and leaves out a family the
//!   release does not offer;
//! * each entry's `InstanceRole` is the set of node roles an instance of its FAMILY
//!   serves — the same for every size of a family and on every release that offers it,
//!   never a coordinator without a master, never the warm role beside another — and
//!   each feature member follows the engine release the feature is available from AND
//!   what the family can serve; a feature available on one release stays available on a
//!   later one;
//! * the `instanceType` filter reports exactly the entry it names, over a table of
//!   types drawn from the listing, and answers an empty listing for a type the release
//!   does not offer — naming an instance type asks for that type's Availability Zone
//!   information, so such a read enables the retrieve-AZ flag
//!   ([`super::instance_type_az_scope`] owns that precondition) and the value is then
//!   resolved against those zones, so a value naming no published instance type at all is
//!   refused rather than reported as an empty listing
//!   ([`super::instance_type_az_resolution`]);
//! * the release's catalog is reported COMPLETE in a single answer, with no `NextToken`
//!   member at all, over a table of page sizes: `MaxResults` does not bound what is
//!   reported and no token is ever handed out, while a `MaxResults` outside the modelled
//!   range is still refused at either end; because no token is ever issued, a `NextToken`
//!   a request carries is inert — it neither resumes the listing nor turns the read into
//!   a refusal, over a table of token values and of the filters they are sent beside;
//! * `retrieveAZs` is what puts Availability Zones in the answer, and the zones are
//!   those of the Region the listing is for — the signing Region, or the Region of the
//!   domain the request names;
//! * a `domainName` that names no domain, and an `EngineVersion` that names no engine
//!   release, are refused with the modelled errors;
//! * the read changes nothing a later caller sees.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::publishes_every_modelled_instance_type;

use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS};

/// The engine versions every table below spans: the oldest Elasticsearch release, each
/// release an instance family or a feature is gated on, one either side of it, and
/// OpenSearch releases from the first to a current one. Both engines and both sides of
/// every gate, so no row is the only evidence for a rule.
const VERSIONS: [&str; 12] = [
    "Elasticsearch_1.5",
    "Elasticsearch_5.1",
    "Elasticsearch_6.7",
    "Elasticsearch_6.8",
    "Elasticsearch_7.9",
    "Elasticsearch_7.10",
    "OpenSearch_1.0",
    "OpenSearch_2.9",
    "OpenSearch_2.11",
    "OpenSearch_2.15",
    "OpenSearch_2.17",
    "OpenSearch_2.19",
];

/// The size tokens of `<family>.<size>.search`, smallest first. Source: the instance
/// sizes AWS publishes normalization factors for — `nano` is a quarter of a `small`,
/// a `large` four of them, and each `Nxlarge` N times an `xlarge` — which is the order
/// a family's types are published in.
const SIZES_SMALLEST_FIRST: [&str; 17] = [
    "nano",
    "micro",
    "small",
    "medium",
    "large",
    "xlarge",
    "2xlarge",
    "3xlarge",
    "4xlarge",
    "6xlarge",
    "8xlarge",
    "9xlarge",
    "10xlarge",
    "12xlarge",
    "16xlarge",
    "18xlarge",
    "24xlarge",
];

/// The families AWS publishes as PREVIOUS generation for this service. The published
/// catalog lists them after the current generations; restated here as the suite's own
/// expectation rather than imported from the service.
const PREVIOUS_GENERATION_FAMILIES: [&str; 8] = ["c4", "d2", "i2", "m3", "m4", "r3", "r4", "t2"];

/// The family that serves the warm tier, which the catalog publishes apart from the
/// hot-tier families.
const WARM_FAMILY: &str = "ultrawarm1";

/// The previous-generation BURSTABLE instance types, which the catalog offers on the
/// ELASTICSEARCH line alone: the `t3` family succeeded them, and the OpenSearch line carries
/// that one instead, so no OpenSearch release offers these. Read off the published catalog by
/// [`the_previous_generation_burstable_family_is_offered_on_the_elasticsearch_line_alone`]
/// rather than asserted against the service's own table.
pub(crate) const PREVIOUS_GENERATION_BURSTABLE: [&str; 3] =
    ["t2.micro.search", "t2.small.search", "t2.medium.search"];

/// The families whose instances hold the domain's data and are never master-eligible:
/// the OpenSearch-optimized ones, and the storage-optimized generations that arrived
/// after the engines forked.
const DATA_ONLY_FAMILIES: [&str; 7] = ["i7i", "i8g", "i8ge", "oi2", "om2", "or1", "or2"];

/// The families neither encryption at rest nor node-to-node encryption is available
/// on, whatever the version.
///
/// Shared with the sibling suites that state the same rule about one entry.
pub(crate) const UNENCRYPTED_FAMILIES: [&str; 2] = ["m3", "t2"];

/// The Elasticsearch release both features `InstanceTypeDetails$EncryptionEnabled`
/// reports are available from: AWS documents encryption at rest from 5.1 and
/// node-to-node encryption from 6.0, and the member reports the pair.
///
/// Shared with the sibling suites that state the same rule about one entry.
pub(crate) const ENCRYPTION_RELEASE: (u32, u32) = (6, 0);

/// The burstable families, which UltraWarm cannot be attached to.
const BURSTABLE_FAMILIES: [&str; 3] = ["t2", "t3", "t4g"];

/// The node roles `InstanceTypeDetails$InstanceRole` reports, from that member's own
/// documentation ("whether the instance acts as a data node, a dedicated master node,
/// or an UltraWarm node") plus the dedicated-coordinator role a family that can run one
/// reports.
const PUBLISHED_ROLES: [&str; 4] = ["data", "master", "coordinator", "ultrawarm"];

/// `ListInstanceTypeDetails`:
/// `GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}`, with `query` as
/// its `@httpQuery` members (empty for a request that sends none).
///
/// Shared with [`super::catalog_publication_order`], which states the rules about the
/// listing's publication this suite does not.
pub(crate) async fn list(emulator: &Emulator, engine_version: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}")
    } else {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// A read NARROWED to one instance type, as such a read has to be made.
///
/// Naming an instance type asks the service for that type's Availability Zone
/// information, so a narrowed read enables the retrieve-AZ flag: without it the service
/// refuses the read for the flag, which is [`super::instance_type_az_scope`]'s subject.
/// Shared with the sibling suites that read one entry back.
pub(crate) async fn list_type(
    emulator: &Emulator,
    engine_version: &str,
    instance_type: &str,
) -> Answer {
    list(
        emulator,
        engine_version,
        &format!("instanceType={instance_type}&retrieveAZs=true"),
    )
    .await
}

/// The entries of a narrowed read, each asserted to carry the Availability Zones the
/// read had to ask for and reported WITHOUT them, so a case can state the rest of the
/// entry without restating the Region's zones — which
/// [`retrieve_azs_reports_the_zones_of_the_listing_region`] owns.
pub(crate) fn narrowed(answer: &Answer, case: &str) -> Vec<Value> {
    details(answer, case)
        .iter()
        .map(|entry| {
            assert!(
                entry["AvailabilityZones"].is_array(),
                "case {case}: expected the zones the narrowed read asked for, got {entry:?}"
            );
            let mut without = entry.clone();
            if let Some(members) = without.as_object_mut() {
                members.remove("AvailabilityZones");
            }
            without
        })
        .collect()
}

/// The `InstanceTypeDetails` list of an answer, with the case named on failure.
///
/// Shared with [`super::published_family_is_whole`], which reads a filtered listing back
/// the same way.
pub(crate) fn details(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["InstanceTypeDetails"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an InstanceTypeDetails array, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The whole listing of one version: this operation reports the version's catalog
/// complete in a single answer, so the answer carries no continuation token.
pub(crate) async fn whole_listing(
    emulator: &Emulator,
    engine_version: &str,
    query: &str,
) -> Vec<Value> {
    let answer = list(emulator, engine_version, query).await;
    assert_eq!(
        answer.body.get("NextToken"),
        None,
        "case {engine_version} ({query:?}): the whole listing is reported, so the answer \
         carries no NextToken, got {:?}",
        answer.body
    );
    details(&answer, engine_version)
}

/// The `InstanceType` of every entry of a listing, in the order reported.
pub(crate) fn instance_types(listing: &[Value]) -> Vec<String> {
    listing
        .iter()
        .map(|entry| {
            entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("every entry carries an InstanceType, got {entry:?}"))
                .to_owned()
        })
        .collect()
}

/// A boolean feature member of one entry, with the case named on failure.
///
/// Shared with the sibling suites that read one entry's capability members.
pub(crate) fn flag(entry: &Value, member: &str, case: &str) -> bool {
    entry[member]
        .as_bool()
        .unwrap_or_else(|| panic!("case {case}: expected a boolean {member}, got {entry:?}"))
}

/// One entry's `InstanceRole`, with the case named on failure.
///
/// Shared with the sibling suites that read one entry's roles back.
pub(crate) fn roles(entry: &Value, case: &str) -> Vec<String> {
    entry["InstanceRole"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an InstanceRole array, got {entry:?}"))
        .iter()
        .map(|role| {
            role.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a string role, got {role:?}"))
                .to_owned()
        })
        .collect()
}

/// The family token of an instance type, which is `<family>.<size>.search`.
///
/// Shared with the sibling suites that read an entry's family back.
pub(crate) fn family(instance_type: &str) -> &str {
    instance_type.split('.').next().unwrap_or(instance_type)
}

/// The size token of an instance type.
fn size(instance_type: &str) -> &str {
    instance_type.split('.').nth(1).unwrap_or(instance_type)
}

/// Where one size sits among the published sizes, smallest first.
fn size_rank(instance_type: &str) -> usize {
    let size = size(instance_type);
    SIZES_SMALLEST_FIRST
        .iter()
        .position(|candidate| *candidate == size)
        .unwrap_or_else(|| {
            panic!("case {instance_type}: expected a published instance size, got {size}")
        })
}

/// Which group of the published catalog a family belongs to: the current generations
/// lead it, then the warm tier, then the previous generations.
fn family_group(instance_type: &str) -> u8 {
    let family = family(instance_type);
    if family == WARM_FAMILY {
        1
    } else if PREVIOUS_GENERATION_FAMILIES.contains(&family) {
        2
    } else {
        0
    }
}

/// An engine version read apart, as the test states the rules against it.
fn parse(engine_version: &str) -> (&str, u32, u32) {
    let (engine, release) = engine_version
        .split_once('_')
        .unwrap_or_else(|| panic!("the version table holds <Engine>_<X.Y> values, got {engine_version}"));
    let (major, minor) = release
        .split_once('.')
        .unwrap_or_else(|| panic!("the version table holds <Engine>_<X.Y> values, got {engine_version}"));
    (
        engine,
        major.parse().expect("a numeric major release"),
        minor.parse().expect("a numeric minor release"),
    )
}

fn is_opensearch(engine_version: &str) -> bool {
    parse(engine_version).0 == "OpenSearch"
}

/// Whether a capability introduced in an Elasticsearch release is available on this
/// version. OpenSearch forked from Elasticsearch 7.10, so every OpenSearch release
/// carries each of the capabilities below.
fn since_elasticsearch(engine_version: &str, release: (u32, u32)) -> bool {
    let (_, major, minor) = parse(engine_version);
    is_opensearch(engine_version) || (major, minor) >= release
}

#[tokio::test]
async fn the_listing_is_published_in_the_catalogs_documented_order() {
    let emulator = Emulator::new();

    for version in VERSIONS {
        let reported = instance_types(&whole_listing(&emulator, version, "").await);
        assert!(
            reported.len() > 50,
            "case {version}: expected the release's whole catalog, got {} entries",
            reported.len()
        );

        let distinct: BTreeSet<&String> = reported.iter().collect();
        assert_eq!(
            distinct.len(),
            reported.len(),
            "case {version}: expected one entry per instance type, got {} entries for \
             {} types",
            reported.len(),
            distinct.len()
        );

        // A family's types are published together, smallest size first: the listing is
        // a catalog of families and not a flat sort, so a client walking it reads one
        // family's sizes in order.
        let mut seen_families: Vec<&str> = Vec::new();
        for pair in reported.windows(2) {
            let (left, right) = (pair[0].as_str(), pair[1].as_str());
            if family(left) == family(right) {
                assert!(
                    size_rank(left) < size_rank(right),
                    "case {version}: expected {} to publish its sizes smallest first, \
                     got {left} before {right}",
                    family(left)
                );
                continue;
            }
            // A family boundary: the family just closed is never reopened, and the
            // documented order decides which family follows.
            assert!(
                !seen_families.contains(&family(right)),
                "case {version}: expected one contiguous run per family, got {} reopened \
                 at {right}",
                family(right)
            );
            seen_families.push(family(left));
            let (left_group, right_group) = (family_group(left), family_group(right));
            assert!(
                left_group <= right_group,
                "case {version}: expected the current generations, then the warm tier, \
                 then the previous generations, got {left} ({left_group}) before {right} \
                 ({right_group})"
            );
            if left_group == right_group {
                assert!(
                    family(left) < family(right),
                    "case {version}: expected the families of one group published by \
                     name, got {left} before {right}"
                );
            }
        }
    }

    // The catalog is the SERVICE's and not the modelled value set: the value set is the
    // instance types that EXIST, one list for the whole service, while a listing is the
    // ones one RELEASE offers — so a release's listing is a strict subset of that set and
    // no listing is the set itself. A listing read off the enum could not answer this.
    for version in ["OpenSearch_2.15", "OpenSearch_2.19", "Elasticsearch_7.10"] {
        let reported = instance_types(&whole_listing(&emulator, version, "").await);
        for instance_type in &reported {
            assert!(
                OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type.as_str()),
                "case {version}: expected every published type to be one the service accepts, \
                 got {instance_type}"
            );
        }
        assert!(
            reported.len() < OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.len(),
            "case {version}: expected one release's catalog to be narrower than the whole \
             accepted set, got {} of {}",
            reported.len(),
            OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.len()
        );
    }

    // Nor is the ORDER that set's, which is sorted: the catalog is published family by
    // family, and its groups are not in name order.
    for version in ["Elasticsearch_7.10", "OpenSearch_2.11", "OpenSearch_2.19"] {
        let reported = instance_types(&whole_listing(&emulator, version, "").await);
        let sorted = {
            let mut sorted = reported.clone();
            sorted.sort();
            sorted
        };
        assert_ne!(
            reported, sorted,
            "case {version}: expected the catalog's published order rather than a sorted \
             value set"
        );
    }

    // The modelled value set is a SUBSET of the published catalog: every type the model
    // declares is published for the releases that offer its family, so no type the model
    // names has been dropped along the way — which the service holds for itself too.
    assert!(
        publishes_every_modelled_instance_type(),
        "case the modelled value set: expected the catalog to publish every instance \
         type the model declares"
    );
    let mut published: BTreeSet<String> = BTreeSet::new();
    for version in VERSIONS {
        published.extend(instance_types(&whole_listing(&emulator, version, "").await));
    }
    for modelled in OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES {
        assert!(
            published.contains(*modelled),
            "case {modelled}: expected some release to offer every instance type the \
             model declares, got {} published types",
            published.len()
        );
    }

    // The catalog is not filtered by instance GENERATION: a previous-generation family
    // is published for a current release too, after the current generations — every one of
    // them except the burstable previous generation, which the OpenSearch line does not
    // carry at all ([`PREVIOUS_GENERATION_BURSTABLE`]).
    let current = instance_types(&whole_listing(&emulator, "OpenSearch_2.19", "").await);
    for previous_generation in ["c4.4xlarge.search", "m4.large.search", "r4.2xlarge.search"] {
        assert!(
            current.contains(&previous_generation.to_string()),
            "case {previous_generation} on OpenSearch_2.19: expected the catalog to carry \
             the previous generations too, got {} entries",
            current.len()
        );
    }
    // And the Elasticsearch line carries the burstable one, which is where it is published.
    let legacy = instance_types(&whole_listing(&emulator, "Elasticsearch_7.10", "").await);
    assert!(
        legacy.contains(&"t2.small.search".to_string()),
        "case t2.small.search on Elasticsearch_7.10: expected the legacy line to carry the \
         burstable previous generation, got {} entries",
        legacy.len()
    );
}

#[tokio::test]
async fn the_listing_is_the_catalog_the_named_release_offers() {
    let emulator = Emulator::new();

    // The release gates, stated as the difference one release makes: the release a
    // family is offered from reports it and the release before it does not. One row per
    // published prerequisite — the warm tier's, the Graviton families', the two
    // generations that arrived after the engines forked, and the two
    // OpenSearch-optimized generations'.
    for (family_member, before, from) in [
        ("ultrawarm1.medium.search", "Elasticsearch_6.7", "Elasticsearch_6.8"),
        ("r6g.large.search", "Elasticsearch_7.8", "Elasticsearch_7.9"),
        ("t4g.small.search", "Elasticsearch_7.8", "Elasticsearch_7.9"),
        ("or1.medium.search", "OpenSearch_2.9", "OpenSearch_2.11"),
        ("m7i.2xlarge.search", "OpenSearch_2.11", "OpenSearch_2.13"),
        ("r7i.12xlarge.search", "OpenSearch_2.11", "OpenSearch_2.13"),
        ("c8g.12xlarge.search", "OpenSearch_2.13", "OpenSearch_2.15"),
        ("oi2.8xlarge.search", "OpenSearch_2.15", "OpenSearch_2.17"),
    ] {
        let absent = instance_types(&whole_listing(&emulator, before, "").await);
        let present = instance_types(&whole_listing(&emulator, from, "").await);
        assert!(
            !absent.contains(&family_member.to_string()),
            "case {family_member} on {before}: expected a release before the family was \
             introduced not to offer it"
        );
        assert!(
            present.contains(&family_member.to_string()),
            "case {family_member} on {from}: expected the release the family is offered \
             from to report it"
        );
    }

    // A family the service publishes for the OpenSearch engine alone — one that arrived
    // after the engines forked — is offered on no Elasticsearch release, however late.
    let last_elasticsearch = instance_types(&whole_listing(&emulator, "Elasticsearch_7.10", "").await);
    for opensearch_only in [
        "or1.large.search",
        "oi2.8xlarge.search",
        "m7i.2xlarge.search",
        "r7i.large.search",
        "c8g.8xlarge.search",
    ] {
        assert!(
            !last_elasticsearch.contains(&opensearch_only.to_string()),
            "case {opensearch_only} on Elasticsearch_7.10: expected a family published for \
             the OpenSearch engine alone to be offered on no Elasticsearch release"
        );
    }

    // A release offers at least what an earlier release of the SAME LINE offers: a family is
    // introduced for a release and is offered from it onwards, so the catalog grows along the
    // succession and loses nothing. Across the FORK the rule is weaker, because one family did
    // not cross it: the burstable previous generation the OpenSearch line does not carry
    // ([`PREVIOUS_GENERATION_BURSTABLE`]), which is the only thing an OpenSearch release may
    // have lost from an Elasticsearch one.
    let crosses_the_fork: BTreeSet<String> = PREVIOUS_GENERATION_BURSTABLE
        .iter()
        .map(|instance_type| (*instance_type).to_string())
        .collect();
    for earlier in VERSIONS {
        for later in VERSIONS {
            let same_line = parse(earlier).0 == parse(later).0;
            let succeeds = if same_line {
                (parse(later).1, parse(later).2) >= (parse(earlier).1, parse(earlier).2)
            } else {
                is_opensearch(later)
            };
            if !succeeds {
                continue;
            }
            let earlier_types: BTreeSet<String> =
                instance_types(&whole_listing(&emulator, earlier, "").await)
                    .into_iter()
                    .collect();
            let later_types: BTreeSet<String> =
                instance_types(&whole_listing(&emulator, later, "").await)
                    .into_iter()
                    .collect();
            let lost: Vec<&String> = earlier_types
                .difference(&later_types)
                .filter(|instance_type| same_line || !crosses_the_fork.contains(*instance_type))
                .collect();
            assert!(
                lost.is_empty(),
                "case {earlier} -> {later}: expected a release to offer every instance \
                 type an earlier one offers, lost {lost:?}"
            );
        }
    }
}

#[tokio::test]
async fn the_previous_generation_burstable_family_is_offered_on_the_elasticsearch_line_alone() {
    let emulator = Emulator::new();

    // Every release of the version table, so the rule is read off the published catalog on
    // both engine lines: the burstable previous generation is offered on every Elasticsearch
    // release, however early or late, and on no OpenSearch release — the T3 family succeeded
    // it there, so a domain of an OpenSearch release cannot be provisioned on one.
    for version in VERSIONS {
        let offered = instance_types(&whole_listing(&emulator, version, "").await);
        for instance_type in PREVIOUS_GENERATION_BURSTABLE {
            assert_eq!(
                offered.contains(&instance_type.to_string()),
                !is_opensearch(version),
                "case {instance_type} on {version}: expected the burstable previous generation \
                 to be offered on the Elasticsearch line alone, got {} entries",
                offered.len()
            );
        }
    }
}

#[tokio::test]
async fn roles_and_features_follow_the_family_and_the_engine_release() {
    let emulator = Emulator::new();

    // The roles are the FAMILY's, so they are collected per family and compared across
    // sizes and releases rather than against a second copy of the service's table.
    let mut roles_by_family: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();

    for version in VERSIONS {
        for entry in whole_listing(&emulator, version, "").await {
            let instance_type = instance_types(std::slice::from_ref(&entry))
                .pop()
                .expect("one entry has one InstanceType");
            let family = family(&instance_type);
            let case = format!("{instance_type} on {version}");

            // `EncryptionEnabled` reports the two encryption features together, and each
            // needs hardware that provides it AND an engine release that has it, so the
            // member follows the family and the release.
            let encryption = flag(&entry, "EncryptionEnabled", &case);
            assert_eq!(
                encryption,
                !UNENCRYPTED_FAMILIES.contains(&family)
                    && since_elasticsearch(version, ENCRYPTION_RELEASE),
                "case {case}: expected encryption support to follow the family and the \
                 release, got {encryption}"
            );

            let cognito = flag(&entry, "CognitoEnabled", &case);
            assert_eq!(
                cognito,
                since_elasticsearch(version, (5, 1)),
                "case {case}: expected Cognito support to follow the release, got {cognito}"
            );

            let app_logs = flag(&entry, "AppLogsEnabled", &case);
            assert_eq!(
                app_logs,
                since_elasticsearch(version, (5, 1)),
                "case {case}: expected application-log support to follow the release, got \
                 {app_logs}"
            );

            // Fine-grained access control requires both encryption features, so it is
            // available only where they are.
            let advanced = flag(&entry, "AdvancedSecurityEnabled", &case);
            assert_eq!(
                advanced,
                encryption && since_elasticsearch(version, (6, 7)),
                "case {case}: expected fine-grained access control to require encryption \
                 and the release, got {advanced}"
            );
            assert!(
                !advanced || encryption,
                "case {case}: fine-grained access control is never reported without \
                 encryption"
            );

            let warm = flag(&entry, "WarmEnabled", &case);
            assert_eq!(
                warm,
                !BURSTABLE_FAMILIES.contains(&family),
                "case {case}: expected UltraWarm support to follow the family, got {warm}"
            );

            // The role set is one of the published vocabulary, carries no repeat, and
            // holds together: a node that is master-eligible or runs as a coordinator
            // also holds data, a coordinator is only published beside a master, and the
            // warm role is published alone.
            let reported = roles(&entry, &case);
            assert!(
                !reported.is_empty(),
                "case {case}: expected the roles an instance of the family serves"
            );
            for role in &reported {
                assert!(
                    PUBLISHED_ROLES.contains(&role.as_str()),
                    "case {case}: expected a published role, got {role}"
                );
            }
            let distinct: BTreeSet<&String> = reported.iter().collect();
            assert_eq!(
                distinct.len(),
                reported.len(),
                "case {case}: expected each role once, got {reported:?}"
            );
            let has = |role: &str| reported.iter().any(|reported| reported == role);
            if family == WARM_FAMILY {
                assert_eq!(
                    reported,
                    vec!["ultrawarm".to_string()],
                    "case {case}: expected the warm tier's one role, got {reported:?}"
                );
            } else {
                assert!(
                    has("data") && !has("ultrawarm"),
                    "case {case}: expected a hot-tier family to hold data and not serve \
                     the warm tier, got {reported:?}"
                );
            }
            if DATA_ONLY_FAMILIES.contains(&family) {
                assert_eq!(
                    reported,
                    vec!["data".to_string()],
                    "case {case}: expected a family whose instances are data nodes only \
                     to publish that one role, got {reported:?}"
                );
            }
            assert!(
                !has("coordinator") || has("master"),
                "case {case}: expected the coordinator role only beside the master one, \
                 got {reported:?}"
            );

            // The roles are the family's: every size of it, on every release that offers
            // it, reports the same set.
            let remembered = roles_by_family
                .entry(family.to_string())
                .or_insert_with(|| (case.clone(), reported.clone()));
            assert_eq!(
                reported, remembered.1,
                "case {case}: expected the roles the family serves, the same as {}, got \
                 {reported:?}",
                remembered.0
            );

            assert!(
                entry["AvailabilityZones"].is_null(),
                "case {case}: a request that did not ask for Availability Zones carries \
                 no AvailabilityZones member, got {:?}",
                entry["AvailabilityZones"]
            );
        }
    }

    // The role sets really differ between families, so the rule above is not vacuous:
    // one row per published set, each naming a family of its own.
    for (family, expected) in [
        ("m7i", vec!["data", "master", "coordinator"]),
        ("c8g", vec!["data", "master", "coordinator"]),
        ("r5", vec!["data", "master", "coordinator"]),
        ("i3", vec!["data", "master"]),
        ("c4", vec!["data", "master"]),
        ("t3", vec!["data", "master"]),
        ("i7i", vec!["data"]),
        ("oi2", vec!["data"]),
        ("or1", vec!["data"]),
        ("ultrawarm1", vec!["ultrawarm"]),
    ] {
        let (_, reported) = roles_by_family
            .get(family)
            .unwrap_or_else(|| panic!("the listing reports the {family} family"));
        assert_eq!(
            reported,
            &expected
                .iter()
                .map(|role| (*role).to_string())
                .collect::<Vec<_>>(),
            "case {family}: expected the roles an instance of the family serves"
        );
    }
}

#[tokio::test]
async fn a_feature_available_on_one_release_stays_available_on_a_later_one() {
    let emulator = Emulator::new();
    let members = [
        "EncryptionEnabled",
        "CognitoEnabled",
        "AppLogsEnabled",
        "AdvancedSecurityEnabled",
        "WarmEnabled",
    ];

    // Every ordered pair of the version table that shares an engine, plus each
    // Elasticsearch release paired with an OpenSearch one, which succeeds it.
    for earlier in VERSIONS {
        for later in VERSIONS {
            let (earlier_engine, earlier_major, earlier_minor) = parse(earlier);
            let (later_engine, later_major, later_minor) = parse(later);
            let succeeds = if earlier_engine == later_engine {
                (later_major, later_minor) >= (earlier_major, earlier_minor)
            } else {
                is_opensearch(later)
            };
            if !succeeds {
                continue;
            }

            let earlier_listing = whole_listing(&emulator, earlier, "").await;
            let later_listing = whole_listing(&emulator, later, "").await;
            for entry in &earlier_listing {
                let instance_type = instance_types(std::slice::from_ref(entry))
                    .pop()
                    .expect("one entry has one InstanceType");
                // A family the later release's engine offers no entry of carries no
                // feature forward: every type an earlier listing published is in the later
                // one, EXCEPT across the fork, where the burstable previous generation did
                // not cross ([`PREVIOUS_GENERATION_BURSTABLE`]).
                let case = format!("{instance_type}: {earlier} -> {later}");
                if earlier_engine != later_engine
                    && PREVIOUS_GENERATION_BURSTABLE.contains(&instance_type.as_str())
                {
                    continue;
                }
                let successor = later_listing
                    .iter()
                    .find(|candidate| candidate["InstanceType"].as_str() == Some(&instance_type))
                    .unwrap_or_else(|| {
                        panic!(
                            "case {case}: an instance type offered on a release is offered \
                             on every later one"
                        )
                    });
                for member in members {
                    let before = flag(entry, member, &case);
                    let after = flag(successor, member, &case);
                    assert!(
                        !before || after,
                        "case {case}: expected {member} to stay available, got {before} \
                         then {after}"
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn anchored_entries_are_reported_exactly() {
    let emulator = Emulator::new();

    // Hand-written anchors: the WHOLE entry, written out, for one instance type of
    // each family rule on releases either side of each feature gate. They pin the
    // parallel expectations above to fixed values, so a rule error replicated in both
    // the handler and this suite's own gates still fails here. The burstable class is
    // anchored on BOTH of its rules — `t3` on a current release, where encryption and
    // fine-grained access control are available and the warm tier is not, and `t2`,
    // which offers none of the three on any release.
    let rows = [
        (
            "OpenSearch_2.19",
            json!({
                "InstanceType": "m7i.2xlarge.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master", "coordinator"],
            }),
        ),
        (
            "OpenSearch_2.19",
            json!({
                "InstanceType": "c8g.12xlarge.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master", "coordinator"],
            }),
        ),
        (
            "OpenSearch_2.19",
            json!({
                "InstanceType": "oi2.8xlarge.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data"],
            }),
        ),
        (
            "OpenSearch_2.19",
            json!({
                "InstanceType": "i7i.4xlarge.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "m5.large.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master", "coordinator"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "t3.micro.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "ultrawarm1.medium.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["ultrawarm"],
            }),
        ),
        (
            "OpenSearch_2.11",
            json!({
                "InstanceType": "or1.medium.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data"],
            }),
        ),
        (
            "Elasticsearch_1.5",
            json!({
                "InstanceType": "m3.medium.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": false,
                "AppLogsEnabled": false,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "Elasticsearch_5.1",
            json!({
                "InstanceType": "c4.large.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master"],
            }),
        ),
        // A release older than fine-grained access control but not older than the two
        // encryption features: the capability the INSTANCE provides and the ones the
        // engine has are reported, and the one it does not have is not.
        (
            "Elasticsearch_6.0",
            json!({
                "InstanceType": "r5.large.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master", "coordinator"],
            }),
        ),
        (
            "Elasticsearch_6.7",
            json!({
                "InstanceType": "t2.micro.search",
                "EncryptionEnabled": false,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": false,
                "WarmEnabled": false,
                "InstanceRole": ["data", "master"],
            }),
        ),
        (
            "Elasticsearch_6.8",
            json!({
                "InstanceType": "r5.large.search",
                "EncryptionEnabled": true,
                "CognitoEnabled": true,
                "AppLogsEnabled": true,
                "AdvancedSecurityEnabled": true,
                "WarmEnabled": true,
                "InstanceRole": ["data", "master", "coordinator"],
            }),
        ),
    ];

    for (version, expected) in rows {
        let instance_type = expected["InstanceType"]
            .as_str()
            .expect("every anchor names its instance type")
            .to_owned();
        let case = format!("{instance_type} on {version}");
        let answer = list_type(&emulator, version, &instance_type).await;
        let entry = narrowed(&answer, &case);
        assert_eq!(
            entry.len(),
            1,
            "case {case}: the filter names one instance type, got {entry:?}"
        );
        assert_eq!(
            entry[0], expected,
            "case {case}: expected the anchored entry, got {:?}",
            entry[0]
        );
    }
}

#[tokio::test]
async fn instance_type_filter_reports_exactly_that_type() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.19";
    let listing = whole_listing(&emulator, version, "").await;

    // One row per way a filtered type differs: the first and last of the listing, one
    // in the middle, and one of each family with a rule of its own.
    let mut rows: Vec<String> = instance_types(&[
        listing[0].clone(),
        listing[listing.len() / 2].clone(),
        listing[listing.len() - 1].clone(),
    ]);
    rows.extend(
        [
            "ultrawarm1.large.search",
            "or1.2xlarge.search",
            "oi2.medium.search",
            "m7i.24xlarge.search",
            "r7i.large.search",
            "t4g.medium.search",
            "c4.8xlarge.search",
        ]
        .map(str::to_owned),
    );

    for instance_type in rows {
        let expected = listing
            .iter()
            .find(|entry| entry["InstanceType"].as_str() == Some(&instance_type))
            .unwrap_or_else(|| panic!("the listing reports {instance_type}"));
        let answer = list_type(&emulator, version, &instance_type).await;
        let filtered = narrowed(&answer, &instance_type);
        assert_eq!(
            filtered.len(),
            1,
            "case {instance_type}: the filter names one instance type, got {filtered:?}"
        );
        assert_eq!(
            &filtered[0], expected,
            "case {instance_type}: expected the entry the listing reports, got {:?}",
            filtered[0]
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {instance_type}: one entry is the whole answer, so it carries no \
             NextToken, got {:?}",
            answer.body["NextToken"]
        );
    }

    // A published instance type the release does not offer exists but is not in its
    // listing, so the listing it narrows is empty rather than refused.
    for (not_offered, version) in [
        ("or1.medium.search", "Elasticsearch_7.10"),
        ("oi2.medium.search", "OpenSearch_2.15"),
        ("m7i.large.search", "Elasticsearch_7.10"),
        ("c8g.8xlarge.search", "OpenSearch_2.13"),
        ("ultrawarm1.medium.search", "Elasticsearch_6.7"),
        ("r6g.large.search", "Elasticsearch_7.8"),
    ] {
        let answer = list_type(&emulator, version, not_offered).await;
        let empty = narrowed(&answer, not_offered);
        assert!(
            empty.is_empty(),
            "case {not_offered} on {version}: expected an empty listing, got {empty:?}"
        );
        assert!(
            answer.body["NextToken"].is_null(),
            "case {not_offered} on {version}: an empty listing carries no NextToken, got \
             {:?}",
            answer.body["NextToken"]
        );
    }

    // The filter decides what is reported, and a `NextToken` beside it decides nothing:
    // this operation hands out no token, so one a request carries names no place in any
    // listing and the filtered answer is the same one entry it is without it.
    let first = instance_types(&listing)[0].clone();
    let second = instance_types(&listing)[1].clone();
    for token in [second.as_str(), "not-a-token"] {
        let case = format!("instanceType={first} beside nextToken={token}");
        let reported = whole_listing(
            &emulator,
            version,
            &format!("instanceType={first}&retrieveAZs=true&nextToken={token}"),
        )
        .await;
        assert_eq!(
            instance_types(&reported),
            vec![first.clone()],
            "case {case}: expected the filtered listing, got {:?}",
            instance_types(&reported)
        );
    }
}

#[tokio::test]
async fn the_whole_listing_is_reported_whatever_the_pagination_members_ask() {
    let emulator = Emulator::new();

    // One row per way a request can ask for a bounded answer: the smallest page size,
    // sizes that do not divide the listing, the largest the modelled range allows, and
    // the modelled default of zero. Each is asked on a version of either engine, so the
    // rule holds over listings of two different lengths.
    //
    // The recorded answers of this operation report the version's catalog complete and
    // no `NextToken` for such a request: the catalog is published complete in a single
    // answer, so `MaxResults` does not bound what is reported and no token is ever handed
    // out.
    for version in ["Elasticsearch_6.7", "OpenSearch_2.19"] {
        let listing = whole_listing(&emulator, version, "").await;
        for query in [
            "maxResults=1",
            "maxResults=2",
            "maxResults=7",
            "maxResults=100",
            "maxResults=0",
        ] {
            let case = format!("{version} query {query:?}");
            let reported = whole_listing(&emulator, version, query).await;
            assert_eq!(
                reported, listing,
                "case {case}: expected the whole listing in one answer, got {} entries",
                reported.len()
            );
        }

        // No token is ever handed out, so a `NextToken` a request carries names no place
        // in the listing and resumes nothing: the answer is the same complete catalog,
        // from its head, whatever the token's value and whatever page size it comes with.
        // Rows: an entry of this very listing, an entry from the middle and the end of it,
        // a value of the member's shape this service never mints, an identifier-shaped
        // one, and one at the far end of what the member admits.
        let types = instance_types(&listing);
        let tokens: Vec<String> = vec![
            types[1].clone(),
            types[types.len() / 2].clone(),
            types[types.len() - 1].clone(),
            "m5.large.search.page2".to_owned(),
            "00000000-0000-0000-0000-000000000000".to_owned(),
            "not-a-token".to_owned(),
            "t".repeat(128),
        ];
        for token in &tokens {
            for query in [
                format!("nextToken={token}"),
                format!("maxResults=5&nextToken={token}"),
                format!("maxResults=100&nextToken={token}&retrieveAZs=false"),
            ] {
                let case = format!("{version} query {query:?}");
                let reported = whole_listing(&emulator, version, &query).await;
                assert_eq!(
                    reported, listing,
                    "case {case}: expected the whole listing from its head, got {} entries \
                     headed by {:?}",
                    reported.len(),
                    instance_types(&reported).first()
                );
            }
        }
    }

    // A page size outside the modelled bound is still refused at either end — the bound
    // is the model's `@range(max: 100)` on `MaxResults`, and a negative count is no page
    // size at all; the bound itself is not refused.
    let at_bound = list(&emulator, "OpenSearch_2.11", "maxResults=100").await;
    assert_eq!(
        at_bound.status, 200,
        "case maxResults=100: the published bound is accepted, got {} {:?}",
        at_bound.status, at_bound.body
    );
    for out_of_bound in ["maxResults=101", "maxResults=1000", "maxResults=-1"] {
        let answer = list(&emulator, "OpenSearch_2.11", out_of_bound).await;
        assert_eq!(
            answer.status, 400,
            "case {out_of_bound}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {out_of_bound}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn retrieve_azs_reports_the_zones_of_the_listing_region() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";

    // The harness signs every request for `us-east-1`, so an unscoped listing is for
    // that Region's zones.
    for query in ["retrieveAZs=true", "maxResults=4&retrieveAZs=true"] {
        let listing = details(&list(&emulator, version, query).await, query);
        assert!(
            !listing.is_empty(),
            "case {query}: expected entries to report zones for"
        );
        for entry in &listing {
            let zones: Vec<String> = entry["AvailabilityZones"]
                .as_array()
                .unwrap_or_else(|| {
                    panic!("case {query}: expected an AvailabilityZones array, got {entry:?}")
                })
                .iter()
                .map(|zone| zone.as_str().unwrap_or_default().to_owned())
                .collect();
            assert_eq!(
                zones,
                vec!["us-east-1a", "us-east-1b", "us-east-1c"],
                "case {query}: expected the signing Region's zones, got {zones:?}"
            );
        }
    }

    // A request that asks for no zones carries no member at all, whatever else it sends.
    for query in ["", "retrieveAZs=false", "maxResults=3"] {
        let listing = details(&list(&emulator, version, query).await, query);
        for entry in &listing {
            assert!(
                entry["AvailabilityZones"].is_null(),
                "case {query:?}: expected no AvailabilityZones member, got {:?}",
                entry["AvailabilityZones"]
            );
        }
    }

    // Scoped to a domain, the zones are the ones of THAT domain's Region — which the
    // domain record carries, so a domain in another Region reports that Region's zones.
    let name = "zoned-domain";
    let created = emulator.create(json!({ "DomainName": name })).await;
    assert_eq!(
        created.status, 200,
        "case create {name}: expected HTTP 200, got {} {:?}",
        created.status, created.body
    );
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        {
            let store = emulator.store();
            let mut guard = store.lock();
            let domain = guard
                .domains
                .get_mut(name)
                .unwrap_or_else(|| panic!("case {region}: the created domain is in the store"));
            domain.region = region.to_string();
        }
        let query = format!("domainName={name}&retrieveAZs=true&maxResults=1");
        let listing = details(&list(&emulator, version, &query).await, region);
        let zones: Vec<String> = listing[0]["AvailabilityZones"]
            .as_array()
            .unwrap_or_else(|| {
                panic!(
                    "case {region}: expected an AvailabilityZones array, got {:?}",
                    listing[0]
                )
            })
            .iter()
            .map(|zone| zone.as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            zones,
            vec![format!("{region}a"), format!("{region}b"), format!("{region}c")],
            "case {region}: expected the domain Region's zones, got {zones:?}"
        );
    }

    // The catalog itself does not depend on the domain: a scoped listing reports the
    // same instance types as an unscoped one. Naming a domain asks for that domain's
    // zones, so the scoped read enables the flag that reports them.
    let scoped = instance_types(
        &whole_listing(&emulator, version, &format!("domainName={name}&retrieveAZs=true")).await,
    );
    let unscoped = instance_types(&whole_listing(&emulator, version, "").await);
    assert_eq!(
        scoped, unscoped,
        "case scoped listing: expected the same instance types as an unscoped listing, \
         got {} and {} entries",
        scoped.len(),
        unscoped.len()
    );
}

#[tokio::test]
async fn a_request_naming_no_domain_or_no_release_is_refused() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";

    // A `domainName` the account holds no domain for names no resource, over a table of
    // names inside the member's own length bounds. Naming a domain asks for that domain's
    // Availability Zone information, so every scoped read below enables the retrieve-AZ
    // flag ([`super::instance_type_az_scope`] holds the refusal of one that does not).
    for absent in ["nosuch", "absent-domain", "zzz"] {
        let answer = list(
            &emulator,
            version,
            &format!("domainName={absent}&retrieveAZs=true"),
        )
        .await;
        assert_eq!(
            answer.status, RECORDED_NOT_FOUND_STATUS,
            "case {absent}: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ResourceNotFoundException",
            "case {absent}: expected the modelled not-found refusal, got {:?}",
            answer.body
        );
    }

    // A deleted domain holds no record either, so the same refusal follows a delete.
    let name = "transient-domain";
    emulator.create(json!({ "DomainName": name })).await;
    let before = list(
        &emulator,
        version,
        &format!("domainName={name}&retrieveAZs=true"),
    )
    .await;
    assert_eq!(
        before.status, 200,
        "case created domain: expected HTTP 200, got {} {:?}",
        before.status, before.body
    );
    emulator
        .call("DELETE", &format!("{PREFIX}/opensearch/domain/{name}"), None)
        .await;
    let after = list(
        &emulator,
        version,
        &format!("domainName={name}&retrieveAZs=true"),
    )
    .await;
    assert_eq!(
        after.status, RECORDED_NOT_FOUND_STATUS,
        "case deleted domain: expected HTTP {RECORDED_NOT_FOUND_STATUS}, got {} {:?}",
        after.status, after.body
    );

    // An `EngineVersion` of the member's own length that names no engine release is a
    // malformed request: the catalog is a function of the release.
    for malformed in [
        "Opensearch_2.11",
        "OpenSearch_abc",
        "Elasticsearch_x.y",
        "AmazonSearch_2.11",
    ] {
        let answer = list(&emulator, malformed, "").await;
        assert_eq!(
            answer.status, 400,
            "case {malformed}: expected HTTP 400, got {} {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {malformed}: expected the modelled validation refusal, got {:?}",
            answer.body
        );
    }
}

#[tokio::test]
async fn reading_the_catalog_changes_nothing() {
    let emulator = Emulator::new();
    let version = "OpenSearch_2.11";
    let first = whole_listing(&emulator, version, "").await;

    // Paged, filtered, zone-carrying and refused reads, all of which only report.
    list(&emulator, version, "maxResults=3").await;
    list_type(&emulator, version, "m5.large.search").await;
    list(&emulator, version, "instanceType=m5.large.search").await;
    list(&emulator, version, "retrieveAZs=true").await;
    list(&emulator, version, "domainName=nosuch&retrieveAZs=true").await;
    list(&emulator, version, "domainName=nosuch").await;
    list(&emulator, "Elasticsearch_1.5", "").await;

    let second = whole_listing(&emulator, version, "").await;
    assert_eq!(
        first, second,
        "case repeat read: expected the same listing the first read reported, got {} \
         entries",
        second.len()
    );

    let store = emulator.store();
    let state = store.lock();
    assert!(
        state.domains.is_empty(),
        "case stored domains: reporting instance types creates no domain, got {:?}",
        state.domains.keys().collect::<Vec<_>>()
    );
}
