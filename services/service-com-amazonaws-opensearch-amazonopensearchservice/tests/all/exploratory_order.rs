// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER the instance-type value set is listed in, beside the
//! MEMBERSHIP its sibling suite pins ([`super::published_instance_type_value_set`]).
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`]; this suite adds no second copy
//! of it.
//!
//! The rule under test: the order a refusal lists an enumerated type's values in is
//! COMPUTED from the values themselves — the order this front door walks its own hash table
//! over the admitted values — rather than read off a declaration. So the order belongs to
//! the TYPE: every member targeting it is listed the same way, the listing is the same on
//! every call, and it is neither the order the catalog publishes the types in nor
//! lexicographic.

use serde_json::json;
use service_com_amazonaws_opensearch_amazonopensearchservice::state::published_instance_types;
use service_common::validation_message::enum_value_bucket;

use super::constraint_refusal_messages::message;
use super::domain_lifecycle::{Answer, Emulator, PREFIX};

/// An engine version the service offers, so a call's refusal is about the instance type it
/// carries rather than about the release it names.
const OFFERED_VERSION: &str = "OpenSearch_2.19";

/// An instance type the service publishes none of, so every call below is refused for the
/// value set rather than served.
const UNKNOWN_TYPE: &str = "t9.nonexistent.search";

/// `DescribeInstanceTypeLimits`:
/// `GET /2021-01-01/opensearch/instanceTypeLimits/{EngineVersion}/{InstanceType}`.
async fn limits(emulator: &Emulator, instance_type: &str) -> Answer {
    let uri = format!("{PREFIX}/opensearch/instanceTypeLimits/{OFFERED_VERSION}/{instance_type}");
    emulator.call("GET", &uri, None).await
}

/// `CreateDomain`: `POST /2021-01-01/opensearch/domain`, naming `instance_type` as the data
/// nodes' type.
async fn create_domain(emulator: &Emulator, name: &str, instance_type: &str) -> Answer {
    emulator
        .call(
            "POST",
            &format!("{PREFIX}/opensearch/domain"),
            Some(json!({
                "DomainName": name,
                "ClusterConfig": {"InstanceType": instance_type}
            })),
        )
        .await
}

/// The values a refusal's bracketed set lists, in the order it lists them.
fn listed(answer: &Answer, case: &str) -> Vec<String> {
    let stated = message(answer, case);
    stated
        .rsplit_once("enum value set: [")
        .and_then(|(_, tail)| tail.strip_suffix(']'))
        .unwrap_or_else(|| panic!("case {case}: expected a bracketed value set, got {stated:?}"))
        .split(", ")
        .map(str::to_owned)
        .collect()
}

/// RULE: the order is computed from the values — ascending bucket of each value's own
/// string hash — so it is the same for every member targeting the type and on every call,
/// and it is neither the catalog's order nor lexicographic.
#[tokio::test]
async fn the_value_set_is_listed_in_the_order_computed_from_the_values() {
    let emulator = Emulator::new();

    let published = published_instance_types();

    // The rows span the members the type is carried in and the calls one client makes: a
    // URI label asked twice, and a member nested in a structure on another operation.
    let rows = vec![
        ("the URI label", limits(&emulator, UNKNOWN_TYPE).await),
        ("the same label asked again", limits(&emulator, UNKNOWN_TYPE).await),
        (
            "a data node's type, nested in the body",
            create_domain(&emulator, "value-set-order", UNKNOWN_TYPE).await,
        ),
    ];

    let mut listings: Vec<(&str, Vec<String>)> = Vec::new();
    for (case, answer) in rows {
        let stated = listed(&answer, case);

        // The ORDER rule itself, asserted over the listing rather than by re-running the
        // renderer: the bucket of each value's own string hash never decreases along the
        // listing, which is what "the order the table holding them is walked" states.
        let buckets: Vec<u32> = stated.iter().map(|value| enum_value_bucket(value)).collect();
        for pair in buckets.windows(2) {
            assert!(
                pair[0] <= pair[1],
                "case {case}: the listing must not decrease in bucket, got {:?} after {:?}",
                pair[1],
                pair[0]
            );
        }

        // Nothing is lost or invented by the ordering: it is a permutation of the catalog
        // the clause states as accepted.
        let mut sorted = stated.clone();
        sorted.sort();
        let mut catalog: Vec<String> = published.iter().map(|value| (*value).to_owned()).collect();
        catalog.sort();
        assert_eq!(
            sorted, catalog,
            "case {case}: the listing must be a permutation of the published catalog, got {} \
             values against {}",
            stated.len(),
            published.len()
        );

        // The order is COMPUTED, not read off a declaration: neither the order the catalog
        // publishes the types in nor lexicographic could have produced it.
        assert_ne!(
            stated, catalog,
            "case {case}: the listing must not be lexicographic"
        );
        let publication: Vec<String> = published.iter().map(|value| (*value).to_owned()).collect();
        assert_ne!(
            stated, publication,
            "case {case}: the listing must not be the catalog's own publication order"
        );
        listings.push((case, stated));
    }

    // One type, one listing: every member states the set the same way, and so does the same
    // member asked twice.
    let (first_case, first) = listings
        .first()
        .expect("the rows above state at least one listing");
    for (case, stated) in &listings {
        assert_eq!(
            stated, first,
            "case {case}: expected the listing {first_case} states, got {} values starting {:?}",
            stated.len(),
            &stated[..stated.len().min(4)]
        );
    }
}

/// The value half of the same clause, for contrast: the ORDER changes nothing about which
/// member is named or which value is echoed, whatever the member.
#[tokio::test]
async fn the_order_leaves_the_member_and_the_echoed_value_alone() {
    let emulator = Emulator::new();

    for (case, answer, path) in [
        ("the URI label", limits(&emulator, UNKNOWN_TYPE).await, "instanceType"),
        (
            "a data node's type",
            create_domain(&emulator, "value-set-echo", UNKNOWN_TYPE).await,
            "clusterConfig.instanceType",
        ),
    ] {
        let stated = message(&answer, case);
        let expected_subject = format!("Value '{UNKNOWN_TYPE}' at '{path}' failed to satisfy");
        assert!(
            stated.contains(&expected_subject),
            "case {case}: expected the clause to state {expected_subject:?}, got {stated:?}"
        );
    }
}
