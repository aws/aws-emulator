// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the VALUE SET a refusal of an instance type enumerates.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees rather than what a renderer returns. The harness comes from
//! [`super::domain_lifecycle`]; this suite adds no second copy of it.
//!
//! The rule under test: a refusal of a value outside an enumerated type names the values
//! that type ADMITS, and for an instance type that list is the catalog the SERVICE
//! publishes — the record of which instance types exist, which every operation resolving
//! one against it refuses a value outside, and which `ListInstanceTypeDetails` serves. The
//! `OpenSearchPartitionInstanceType` value set this model carries is an older SNAPSHOT of
//! that catalog, so a clause stating the snapshot would tell a caller that instance types
//! the service does publish are invalid and omit the ones it has to choose from. The
//! message is the only place the accepted list is stated.
//!
//! The set belongs to the TYPE rather than to a member, so the rows span the members that
//! target it — a URI label and two members nested in a structure, on two operations — and
//! the values a row submits span what a bad instance type can look like. The order inside
//! the bracketed set is the sibling suites' subject
//! ([`super::constraint_refusal_messages`]); what this one adds is the MEMBERSHIP.

use std::collections::BTreeSet;

use serde_json::{json, Value};
use service_com_amazonaws_opensearch_amazonopensearchservice::ops::types::OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::published_instance_types;
use service_common::validation_message::hashed_enum_order;

use super::constraint_refusal_messages::{clause, message, report, value_set};
use super::domain_lifecycle::{error_code, Answer, Emulator, PREFIX};

/// An engine version the service offers, so a row's refusal is about the instance type it
/// carries rather than about the release it names.
const OFFERED_VERSION: &str = "OpenSearch_2.19";

/// Current-generation instance types the checked-in value set does not list, each a FAMILY
/// the service's catalog publishes: the seventh-generation Intel, Graviton4,
/// dense-storage and second-generation OpenSearch-optimized families.
///
/// Source: the instance types the service's own catalog publishes
/// (`crate::state::published_instance_types`, which `ListInstanceTypeDetails` serves and
/// [`super::current_generation_instance_types`] tables), sampled one size per family —
/// including families the report that asked for this listing did not cite.
const BEYOND_THE_SNAPSHOT: [&str; 6] = [
    "c7i.xlarge.search",
    "m7i.2xlarge.search",
    "c8g.16xlarge.search",
    "r8gd.medium.search",
    "oi2.4xlarge.search",
    "om2.8xlarge.search",
];

/// `DescribeInstanceTypeLimits`:
/// `GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}`.
async fn limits(emulator: &Emulator, instance_type: &str) -> Answer {
    let uri = format!("{PREFIX}/opensearch/instanceTypeLimits/{OFFERED_VERSION}/{instance_type}");
    emulator.call("GET", &uri, None).await
}

/// `CreateDomain`: `POST /2021-01-01/opensearch/domain`, with `cluster` as its
/// `ClusterConfig`.
async fn create_domain(emulator: &Emulator, name: &str, cluster: Value) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain"),
            Some(json!({"DomainName": name, "ClusterConfig": cluster})),
        )
        .await
}

/// The bracketed value set a clause states for the instance-type type: the catalog the
/// service publishes, in the order this front door states a value set.
fn published_value_set() -> String {
    let published = published_instance_types();
    value_set(&hashed_enum_order(&published))
}

/// RULE: every member targeting the instance-type type is refused with the value set the
/// service publishes, whatever the member and wherever the value was bound.
#[tokio::test]
async fn an_instance_type_outside_the_published_catalog_is_refused_with_that_catalog() {
    let emulator = Emulator::new();
    let expected_set = published_value_set();

    // (case, the answer, the member path the clause is stated at, the value it echoes).
    // The rows span the bindings this type is carried in — a URI label, and two members
    // nested in a structure — and what a bad instance type looks like: a family that does
    // not exist, a size a family is not published in, and a value that is not an instance
    // type at all.
    let rows = vec![
        (
            "a nonexistent family on a URI label",
            limits(&emulator, "t9.nonexistent.search").await,
            "instanceType",
            "t9.nonexistent.search",
        ),
        (
            "a size no family is published in, on a URI label",
            limits(&emulator, "m5.128xlarge.search").await,
            "instanceType",
            "m5.128xlarge.search",
        ),
        (
            "a value that is not an instance type at all, on a URI label",
            limits(&emulator, "not-an-instance-type").await,
            "instanceType",
            "not-an-instance-type",
        ),
        (
            "a data node's type, nested in the body",
            create_domain(
                &emulator,
                "published-set-data",
                json!({"InstanceType": "t9.nonexistent.search"}),
            )
            .await,
            "clusterConfig.instanceType",
            "t9.nonexistent.search",
        ),
        (
            "a dedicated master's type, nested in the body",
            create_domain(
                &emulator,
                "published-set-master",
                json!({
                    "InstanceType": "m5.large.search",
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": "t9.nonexistent.search"
                }),
            )
            .await,
            "clusterConfig.dedicatedMasterType",
            "t9.nonexistent.search",
        ),
    ];

    for (case, answer, path, value) in rows {
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected HTTP 400, got {} with {:?}",
            answer.status, answer.body
        );
        assert_eq!(
            error_code(&answer),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {:?}",
            answer.body
        );
        let stated = message(&answer, case);
        let expected = report(&[clause(value, path, &expected_set)]);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected the published catalog as the value set, got {stated:?}"
        );
    }
}

/// The MEMBERSHIP half of the rule, stated against the two things the set has to agree
/// with: it loses nothing the model declares, and it carries the current-generation
/// families the model's snapshot does not list.
#[tokio::test]
async fn the_stated_set_is_the_catalog_and_not_the_modelled_snapshot() {
    let emulator = Emulator::new();

    let answer = limits(&emulator, "t9.nonexistent.search").await;
    let stated = message(&answer, "a nonexistent family");
    let listed: BTreeSet<&str> = stated
        .rsplit_once("enum value set: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("expected a bracketed value set, got {stated:?}"))
        .split(", ")
        .collect();

    // Nothing the model declares is dropped: the snapshot is a subset of the catalog, so
    // a value the boundary admits is always one the stated set names.
    for modelled in OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES {
        assert!(
            listed.contains(modelled),
            "case {modelled:?}: a modelled instance type must still be stated as accepted, \
             got {} values",
            listed.len()
        );
    }

    // And the current generations the snapshot does not list are stated, each of them a
    // type `ListInstanceTypeDetails` publishes for an offered release.
    for current in BEYOND_THE_SNAPSHOT {
        assert!(
            !OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.contains(&current),
            "case {current:?}: the row must name a type the modelled snapshot does not list"
        );
        assert!(
            listed.contains(current),
            "case {current:?}: a current-generation type the service publishes must be \
             stated as accepted, got {} values",
            listed.len()
        );
        assert!(
            served_by_the_listing(&emulator, current).await,
            "case {current:?}: a type the refusal states as accepted must be one the \
             service's own catalog listing publishes"
        );
    }

    assert!(
        listed.len() > OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.len(),
        "the stated set must be the catalog rather than the snapshot, got {} values against \
         the model's {}",
        listed.len(),
        OPEN_SEARCH_PARTITION_INSTANCE_TYPE_VALUES.len()
    );
}

/// Whether `ListInstanceTypeDetails` publishes `instance_type` for the offered release —
/// the catalog the refusal's value set has to agree with, read back through the operation
/// that serves it.
async fn served_by_the_listing(emulator: &Emulator, instance_type: &str) -> bool {
    let uri = format!(
        "{PREFIX}/opensearch/instanceTypeDetails/{OFFERED_VERSION}?instanceType={instance_type}"
    );
    let answer = emulator.call("GET", &uri, None).await;
    answer.body["InstanceTypeDetails"]
        .as_array()
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|entry| entry["InstanceType"].as_str() == Some(instance_type))
        })
}
