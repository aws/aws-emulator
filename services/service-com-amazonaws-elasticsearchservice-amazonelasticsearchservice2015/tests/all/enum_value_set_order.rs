// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the ORDER an enumerated member's refusal lists the admitted
//! values in, on this API's 2015-01-01 surface.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the router
//! into the mounted service — so what is asserted is what a client sees. The harness is
//! [`crate::harness`], this crate's one copy.
//!
//! RULE: a value outside an enumerated member's set is refused with the standard
//! constraint report over that member's WHOLE admitted set, and the ORDER the set is
//! listed in is the order this front door walks its own hash table over those values —
//! neither the order the model declares them in nor lexicographic. The order belongs to
//! the enumerated TYPE and is computed from the values themselves, so it is the same
//! whatever value was rejected, whichever member carried it and whichever operation it
//! arrived at; it is also the order a HANDLER-stated clause of this surface already lists
//! a set in (`src/state/request_constraints.rs`, for a map's keys).
//!
//! The table varies what the rule generalises over: the binding the value arrives on (a
//! query parameter, a body member nested in a structure), the operation, the size of the
//! set (two values, four, and the model's largest enumerated type) and the shape of the
//! rejected value.

use serde_json::{json, Value};
use service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::ops::types::{
    ENGINE_TYPE_VALUES, E_S_PARTITION_INSTANCE_TYPE_VALUES,
    E_S_WARM_PARTITION_INSTANCE_TYPE_VALUES, T_L_S_SECURITY_POLICY_VALUES,
};
use service_common::validation_message::{enum_value_bucket, hashed_enum_order};

use crate::harness::{call_in, router};

/// The Region every call is signed for.
const REGION: &str = "us-east-1";

/// A `DomainName` this model's own constraints admit, so a refusal under test is about the
/// member the row exercises and nothing else.
const WELL_FORMED_DOMAIN: &str = "enum-order-probe";

/// The order `EngineType`'s value set is published in.
///
/// Spelled out rather than derived, so a change to either the rule or the values fails
/// this suite: the model declares the type `[OpenSearch, Elasticsearch]`
/// (`smithy_query(query="shape",shape="EngineType")`) and codegen hands the walk them
/// sorted, so neither of those orders is what the clause states — this one is.
const PUBLISHED_ENGINE_TYPES: [&str; 2] = ["OpenSearch", "Elasticsearch"];

/// `Member must satisfy enum value set: [<values>]`, the framework's own requirement
/// sentence for an enumerated member.
fn value_set(stated: &[&str]) -> String {
    format!("Member must satisfy enum value set: [{}]", stated.join(", "))
}

/// The whole one-clause report for `value` at `path`.
fn one_clause(value: &str, path: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{path}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The values a bracketed set lists, in the order it lists them, read back out of
/// `message`.
fn stated_set(message: &str, case: &str) -> Vec<String> {
    let opened = message
        .split_once("Member must satisfy enum value set: [")
        .unwrap_or_else(|| panic!("case {case:?}: expected an enum value set, got {message:?}"))
        .1;
    let set = opened
        .split_once(']')
        .unwrap_or_else(|| panic!("case {case:?}: expected a closed value set, got {message:?}"))
        .0;
    set.split(", ").map(str::to_owned).collect()
}

/// One row of the table below: a request carrying a value outside an enumerated member's
/// set, and the clause its refusal has to state.
struct Row {
    case: &'static str,
    method: &'static str,
    path: String,
    body: Option<Value>,
    /// The value the clause echoes.
    value: &'static str,
    /// The member path the clause is stated at.
    member: &'static str,
    /// The requirement the clause states, the set as this front door lists it.
    requirement: String,
}

/// RULE: every enumerated member's clause lists its type's whole set in the published
/// order, whatever binding the value arrived on and whichever operation took it.
#[tokio::test]
async fn an_enumerated_member_lists_its_set_in_the_published_order() {
    let router = router();

    let rows: Vec<Row> = vec![
        Row {
            // A query parameter, over the set published above.
            case: "an engine type on a query parameter",
            method: "GET",
            path: "/2015-01-01/domain?engineType=opensearch".to_string(),
            body: None,
            value: "opensearch",
            member: "engineType",
            requirement: value_set(&PUBLISHED_ENGINE_TYPES),
        },
        Row {
            // The same type, another value rejected: the order belongs to the type, so
            // it does not move with the value.
            case: "another engine type on the same parameter",
            method: "GET",
            path: "/2015-01-01/domain?engineType=ELASTICSEARCH".to_string(),
            body: None,
            value: "ELASTICSEARCH",
            member: "engineType",
            requirement: value_set(&PUBLISHED_ENGINE_TYPES),
        },
        Row {
            // A body member nested in a structure, on the create path.
            case: "a warm instance type nested in the cluster configuration",
            method: "POST",
            path: "/2015-01-01/es/domain".to_string(),
            body: Some(json!({
                "DomainName": WELL_FORMED_DOMAIN,
                "ElasticsearchClusterConfig": { "WarmType": "ultrawarm9.huge.elasticsearch" },
            })),
            value: "ultrawarm9.huge.elasticsearch",
            member: "elasticsearchClusterConfig.warmType",
            requirement: value_set(&hashed_enum_order(
                E_S_WARM_PARTITION_INSTANCE_TYPE_VALUES,
            )),
        },
        Row {
            // A nested body member on the update path, over a four-value set.
            case: "a TLS policy nested in the endpoint options",
            method: "POST",
            path: format!("/2015-01-01/es/domain/{WELL_FORMED_DOMAIN}/config"),
            body: Some(json!({
                "DomainEndpointOptions": { "TLSSecurityPolicy": "policy-min-tls-1-2-2019-07" }
            })),
            value: "policy-min-tls-1-2-2019-07",
            member: "domainEndpointOptions.tLSSecurityPolicy",
            requirement: value_set(&hashed_enum_order(T_L_S_SECURITY_POLICY_VALUES)),
        },
    ];

    for row in rows {
        let Row {
            case,
            method,
            path,
            body,
            value,
            member,
            requirement,
        } = row;
        let answer = call_in(&router, REGION, method, &path, body.as_ref()).await;
        assert_eq!(
            answer.status, 400,
            "case {case:?}: expected 400, got {} {}",
            answer.status, answer.body
        );
        assert!(
            answer.error_type.contains("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?} / {}",
            answer.error_type,
            answer.body
        );
        let message = answer.text(case, "/message");
        let expected = one_clause(value, member, &requirement);
        assert_eq!(
            message, expected,
            "case {case:?}: expected {expected:?}, got {message:?}"
        );
    }
}

/// RULE: the published order is the hash table's, not the sort — asserted on the sets
/// small enough to compare whole, so no row above passes by the two orders coinciding.
#[test]
fn the_published_order_is_not_the_sort() {
    // (case, the set as published) over the sets the suite above states in full.
    for (case, published) in [
        ("EngineType", PUBLISHED_ENGINE_TYPES.to_vec()),
        (
            "TLSSecurityPolicy",
            hashed_enum_order(T_L_S_SECURITY_POLICY_VALUES),
        ),
    ] {
        let mut sorted = published.clone();
        sorted.sort_unstable();
        assert_ne!(
            published, sorted,
            "case {case}: the published order must differ from the sort, got {published:?}"
        );
    }
}

/// RULE: the same order holds for the model's LARGEST enumerated type, which is too big to
/// spell out — so it is asserted as the property the order has: each value sits in the
/// bucket its own text hashes to, and the set is listed bucket by bucket.
#[tokio::test]
async fn the_largest_enumerated_set_is_listed_bucket_by_bucket() {
    let router = router();

    /// A value no instance type of the model's enumerated type spells.
    const NOT_AN_INSTANCE_TYPE: &str = "t9.nonexistent.elasticsearch";
    /// Instance types the model declares, spread across the families it carries, so the
    /// set read back is checked for membership and not only for order.
    const DECLARED: [&str; 3] = [
        "m3.medium.elasticsearch",
        "r5.large.elasticsearch",
        "ultrawarm1.large.elasticsearch",
    ];

    let case = "an instance type outside the cluster configuration's set";
    let body = json!({
        "DomainName": WELL_FORMED_DOMAIN,
        "ElasticsearchClusterConfig": { "InstanceType": NOT_AN_INSTANCE_TYPE },
    });
    let answer = call_in(
        &router,
        REGION,
        "POST",
        "/2015-01-01/es/domain",
        Some(&body),
    )
    .await;
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} {}",
        answer.status, answer.body
    );
    let message = answer.text(case, "/message");
    let stated = stated_set(&message, case);

    let buckets: Vec<u32> = stated
        .iter()
        .map(|value| enum_value_bucket(value))
        .collect();
    assert!(
        buckets.windows(2).all(|pair| pair[0] <= pair[1]),
        "case {case:?}: expected the set listed bucket by bucket, got {:?}",
        stated.iter().zip(&buckets).collect::<Vec<_>>()
    );
    let mut sorted = stated.clone();
    sorted.sort_unstable();
    assert_ne!(
        stated, sorted,
        "case {case:?}: the order stated must differ from the sort, got {stated:?}"
    );
    assert_eq!(
        stated.len(),
        E_S_PARTITION_INSTANCE_TYPE_VALUES.len(),
        "case {case:?}: expected the member's whole declared set, got {stated:?}"
    );
    for declared in DECLARED {
        assert!(
            stated.iter().any(|value| value == declared),
            "case {case:?}: expected the declared set, which {declared:?} is in, got {stated:?}"
        );
    }
}

/// RULE: a value the set ADMITS states no value set at all — the clause is about a value
/// outside the set, so every admitted value is carried past the boundary.
#[tokio::test]
async fn an_admitted_value_states_no_value_set() {
    let router = router();

    for admitted in ENGINE_TYPE_VALUES {
        let path = format!("/2015-01-01/domain?engineType={admitted}");
        let answer = call_in(&router, REGION, "GET", &path, None).await;
        assert_eq!(
            answer.status, 200,
            "case {admitted:?}: an admitted engine type is served, got {} {}",
            answer.status, answer.body
        );
        assert!(
            !answer.body.to_string().contains("enum value set"),
            "case {admitted:?}: an admitted value states no value set, got {}",
            answer.body
        );
    }
}
