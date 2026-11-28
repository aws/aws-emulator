// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the CURRENT GENERATIONS of the instance-type catalog
//! `ListInstanceTypeDetails` publishes.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a
//! client sees, not what a handler returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule asserted here: the catalog a release's listing reports is the catalog the
//! SERVICE publishes for that release, so it carries every current-generation family the
//! service accepts an instance type of — the whole of `OpenSearchPartitionInstanceType`,
//! narrowed to the families that release offers. [`super::instance_type_catalog`] states
//! the listing's shape, order and feature members over the whole catalog; this suite is
//! the table of the current-generation families themselves, each row asserting that the
//! family is published, that the release before its prerequisite does not offer it, that
//! the `instanceType` filter reports the entry the listing reports, and that the entry
//! carries the node roles an instance of the family serves.
//!
//! Source of the table: the instance types the service's own accepted-value list
//! reports — the value set its refusal of an unknown `instanceType` enumerates, which
//! is the service's record of what it accepts and carries the Graviton2 general-purpose,
//! compute-optimized and memory-optimized families, BOTH halves of a generation the
//! service publishes as a pair (the EBS-backed family beside the NVMe-backed one of a
//! memory-optimized generation, `r6g` beside `r6gd`), and the OpenSearch-optimized
//! family. The sizes each family is published in
//! are the service's published ladder for it, as [`ListInstanceTypeDetails`] reports
//! them; this suite reads them back from the answer rather than restating them.

use std::collections::BTreeSet;

use serde_json::Value;
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES;

use super::domain_lifecycle::{Answer, Emulator, PREFIX};
use super::instance_type_catalog::list_type;

/// The roles a general-purpose, compute-optimized or memory-optimized family serves.
const DATA_MASTER_COORDINATOR: &[&str] = &["data", "master", "coordinator"];

/// The role a family published for the DATA TIER alone serves — the OpenSearch-optimized
/// families, and the storage-optimized generations that arrived after the engines forked:
/// its instances hold the domain's data and are never master-eligible.
const DATA_ONLY: &[&str] = &["data"];

/// One row per current-generation family, each naming a DIFFERENT published size so the
/// table spans the size ladder as well as the families: the instance type, a release
/// that offers it, the release before its family's prerequisite, and the roles an
/// instance of the family serves.
///
/// Every row is a family whose first release is one the catalog states, so the
/// "an earlier release offers none" half of each case has a release to ask about; a family
/// the service offers on every release it publishes has none and is tabled by its siblings.
const CURRENT_GENERATION: [(&str, &str, &str, &[&str]); 5] = [
    // The Graviton2 families, offered from the Graviton prerequisite onwards — so from an
    // Elasticsearch release and on every OpenSearch one.
    ("c6g.xlarge.search", "Elasticsearch_7.9", "Elasticsearch_7.8", DATA_MASTER_COORDINATOR),
    ("m6g.4xlarge.search", "Elasticsearch_7.9", "Elasticsearch_7.8", DATA_MASTER_COORDINATOR),
    // Both halves of the memory-optimized generation the service publishes as a pair: the
    // EBS-backed family and the NVMe-backed one.
    ("r6g.12xlarge.search", "Elasticsearch_7.9", "Elasticsearch_7.8", DATA_MASTER_COORDINATOR),
    ("r6gd.16xlarge.search", "Elasticsearch_7.9", "Elasticsearch_7.8", DATA_MASTER_COORDINATOR),
    // The OpenSearch-optimized family, offered for the OpenSearch engine alone.
    ("or1.2xlarge.search", "OpenSearch_2.11", "OpenSearch_2.9", DATA_ONLY),
];

/// A release every row above is offered on, so one listing answers for the whole table.
const CURRENT_RELEASE: &str = "OpenSearch_2.19";

/// `ListInstanceTypeDetails`:
/// `GET /2021-01-01/opensearch/instanceTypeDetails/{EngineVersion}`, with `query` as its
/// `@httpQuery` members (empty for a request that sends none).
async fn list(emulator: &Emulator, engine_version: &str, query: &str) -> Answer {
    let uri = if query.is_empty() {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}")
    } else {
        format!("{PREFIX}/opensearch/instanceTypeDetails/{engine_version}?{query}")
    };
    emulator.call("GET", &uri, None).await
}

/// The `InstanceTypeDetails` list of an answer, with the case named on failure.
fn details(answer: &Answer, case: &str) -> Vec<Value> {
    assert_eq!(
        answer.status, 200,
        "case {case}: expected HTTP 200, got {} {:?}",
        answer.status, answer.body
    );
    answer.body["InstanceTypeDetails"]
        .as_array()
        .unwrap_or_else(|| {
            panic!(
                "case {case}: expected an InstanceTypeDetails list, got {:?}",
                answer.body
            )
        })
        .clone()
}

/// The `InstanceType` of every entry of a listing, in the order reported.
fn instance_types(listing: &[Value]) -> Vec<String> {
    listing
        .iter()
        .map(|entry| {
            entry["InstanceType"]
                .as_str()
                .unwrap_or_else(|| panic!("expected an InstanceType string, got {entry:?}"))
                .to_owned()
        })
        .collect()
}

/// One entry's `InstanceRole`, with the case named on failure.
fn roles(entry: &Value, case: &str) -> Vec<String> {
    entry["InstanceRole"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case}: expected an InstanceRole array, got {entry:?}"))
        .iter()
        .map(|role| {
            role.as_str()
                .unwrap_or_else(|| panic!("case {case}: expected a role string, got {role:?}"))
                .to_owned()
        })
        .collect()
}

/// The family token of an instance type, which is `<family>.<size>.search`.
fn family(instance_type: &str) -> &str {
    instance_type.split('.').next().unwrap_or(instance_type)
}

#[tokio::test]
async fn the_catalog_publishes_the_current_generation_families_the_service_accepts() {
    let emulator = Emulator::new();
    let current = instance_types(&details(&list(&emulator, CURRENT_RELEASE, "").await, CURRENT_RELEASE));

    for (instance_type, from, before, expected_roles) in CURRENT_GENERATION {
        let case = format!("{instance_type} on {from}");

        // The release the family is offered from reports it, and so does every later
        // release: a catalog never loses a type a release offers.
        let offered = instance_types(&details(&list(&emulator, from, "").await, &case));
        assert!(
            offered.contains(&instance_type.to_string()),
            "case {case}: expected the release's catalog to publish the instance type, got \
             {} entries",
            offered.len()
        );
        assert!(
            current.contains(&instance_type.to_string()),
            "case {instance_type} on {CURRENT_RELEASE}: expected a current release to \
             publish the instance type too, got {} entries",
            current.len()
        );

        // The release before the family's published prerequisite does not offer it, so
        // the catalog is a function of the release and not one fixed list.
        let absent = instance_types(&details(&list(&emulator, before, "").await, &case));
        assert!(
            !absent.contains(&instance_type.to_string()),
            "case {instance_type} on {before}: expected a release before the family's \
             prerequisite not to offer it"
        );

        // The `instanceType` filter reports exactly the entry the listing reports, and
        // the entry carries the roles an instance of the family serves. The narrowed read
        // is made the way the service requires it
        // ([`super::instance_type_catalog::list_type`]): naming the type asks for its
        // Availability Zone information, which the retrieve-AZ flag has to enable.
        let filtered = details(&list_type(&emulator, from, instance_type).await, &case);
        assert_eq!(
            filtered.len(),
            1,
            "case {case}: the filter names one instance type, got {filtered:?}"
        );
        let reported = roles(&filtered[0], &case);
        assert_eq!(
            reported,
            expected_roles
                .iter()
                .map(|role| (*role).to_owned())
                .collect::<Vec<String>>(),
            "case {case}: expected the roles an instance of the {} family serves, got \
             {reported:?}",
            family(instance_type)
        );
    }
}

#[tokio::test]
async fn the_published_catalog_and_the_accepted_value_set_agree() {
    let emulator = Emulator::new();
    let current = instance_types(&details(&list(&emulator, CURRENT_RELEASE, "").await, CURRENT_RELEASE));
    let published: BTreeSet<&str> = current.iter().map(String::as_str).collect();

    // Every row of the table above is BOTH published for a current release and a value
    // the service accepts: a type the catalog offers a caller is a type the caller may
    // then name, so the catalog and the accepted value set cannot disagree about a
    // current generation.
    for (instance_type, _, _, _) in CURRENT_GENERATION {
        assert!(
            published.contains(instance_type),
            "case {instance_type}: expected a current-generation type to be published, got \
             {} entries",
            published.len()
        );
        assert!(
            OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&instance_type),
            "case {instance_type}: expected a published type to be one the service accepts"
        );
    }

    // A family is published as a LADDER of sizes, not as the one size the table names,
    // and every size of it carries the family's roles.
    for (instance_type, _, _, expected_roles) in CURRENT_GENERATION {
        let family = family(instance_type);
        let sizes: Vec<&str> = published
            .iter()
            .copied()
            .filter(|published| self::family(published) == family)
            .collect();
        assert!(
            sizes.len() >= 6,
            "case the {family} family: expected the family's published sizes, got {sizes:?}"
        );
        let listing = details(
            &list_type(&emulator, CURRENT_RELEASE, instance_type).await,
            instance_type,
        );
        let reported = roles(&listing[0], instance_type);
        assert_eq!(
            reported,
            expected_roles
                .iter()
                .map(|role| (*role).to_owned())
                .collect::<Vec<String>>(),
            "case the {family} family: expected one role set per family, got {reported:?}"
        );
    }
}

#[tokio::test]
async fn availability_zones_are_reported_for_the_current_generations_too() {
    let emulator = Emulator::new();

    // `retrieveAZs` is what puts Availability Zones in the answer; the entries of the
    // current generations carry them exactly as every other entry does.
    for (instance_type, from, _, _) in CURRENT_GENERATION {
        let case = format!("{instance_type} on {from} with retrieveAZs");
        let answer = list_type(&emulator, from, instance_type).await;
        let entry = details(&answer, &case);
        let zones: Vec<String> = entry[0]["AvailabilityZones"]
            .as_array()
            .unwrap_or_else(|| {
                panic!("case {case}: expected an AvailabilityZones array, got {:?}", entry[0])
            })
            .iter()
            .map(|zone| zone.as_str().unwrap_or_default().to_owned())
            .collect();
        assert_eq!(
            zones,
            vec!["us-east-1a", "us-east-1b", "us-east-1c"],
            "case {case}: expected the signing Region's zones, got {zones:?}"
        );

        // A read that asks for no zone information carries the member for no entry, so a
        // client reads no zones it did not ask for.
        let whole = details(&list(&emulator, from, "").await, &case);
        let entry = whole
            .iter()
            .find(|entry| entry["InstanceType"].as_str() == Some(instance_type))
            .unwrap_or_else(|| panic!("case {case}: expected the release to publish the type"));
        assert!(
            entry["AvailabilityZones"].is_null(),
            "case {case}: expected no zones for a request that did not ask, got {entry:?}"
        );
    }
}
