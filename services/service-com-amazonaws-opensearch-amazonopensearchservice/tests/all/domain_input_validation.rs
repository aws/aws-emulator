// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for how this service refuses a request whose INPUT it will not take:
//! what a clause says about the value, in which order it lists an enumerated member's
//! admitted values, and WHEN that refusal is decided relative to resolving the resource
//! the request names.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees. The harness comes from [`super::domain_lifecycle`] and the envelope and clause
//! builders from [`super::constraint_refusal_messages`]; this suite adds no second copy of
//! either.
//!
//! Three rules, each asserted over a SET of inputs:
//!
//! 1. a member the request broke ONE constraint of is stated with the value it submitted,
//!    and a member it broke SEVERAL constraints of is stated WITHOUT it — in every one of
//!    that member's clauses. What distinguishes the clauses of such a report is the
//!    requirement each names; the value is the same in all of them. Neither half is about
//!    the value's size or shape: the rows span a short value and a long dotted one, a
//!    value bound to a URI label and one in the body, and the constraint classes the model
//!    declares for them;
//! 2. an enumerated member's clause lists the member's WHOLE admitted set, in the order
//!    this front door walks its own hash table over those values — the same order whatever
//!    value was rejected, whichever operation carried it and whatever the store holds;
//! 3. an INPUT constraint is applied BEFORE the domain a request names is resolved, so a
//!    value outside a member's declared syntax is refused by naming that member rather
//!    than reported as a domain that does not exist. The rows span the shapes a value can
//!    miss the custom endpoint's FQDN syntax by, on the update path and on the create
//!    path, and the values it admits — which are carried past the constraint and stored.

use serde_json::{json, Value};

use super::constraint_refusal_messages::{clause, report, value_set};
use super::domain_lifecycle::{
    error_code, Answer, Emulator, PREFIX, RECORDED_NOT_FOUND_STATUS,
};

/// The requirement clause for `DomainName`'s upper length bound (`@length(3, 28)`,
/// `smithy_query(query="shape",shape="DomainName")`).
const AT_MOST_28: &str = "Member must have length less than or equal to 28";

/// The requirement clause for its lower bound.
const AT_LEAST_3: &str = "Member must have length greater than or equal to 3";

/// The requirement clause for `PackageName`'s upper length bound (`@length(3, 256)`,
/// `smithy_query(query="shape",shape="PackageName")`) — a different bound over the same
/// two constraint classes, so the row on that member states the rule rather than
/// repeating a domain name's numbers.
const AT_MOST_256: &str = "Member must have length less than or equal to 256";

/// The requirement clause for `DomainName`'s and `PackageName`'s `@pattern`, quoted as
/// this service quotes an expression.
const NAME_EXPRESSION: &str =
    r"Member must satisfy regular expression pattern: [a-z][a-z0-9\-]+";

/// The member path a custom endpoint's clause is stated at: the container and the member
/// inside it, each in the casing a validation path spells a member in.
const CUSTOM_ENDPOINT_PATH: &str = "domainEndpointOptions.customEndpoint";

/// The expression a custom endpoint has to satisfy, as the refusal quotes it — the model's
/// own text for `DomainNameFqdn` (`smithy_query(query="shape",shape="DomainNameFqdn")`).
const FQDN_EXPRESSION: &str = concat!(
    "Member must satisfy regular expression pattern: ",
    r"^(((?!-)[A-Za-z0-9-]{0,62}[A-Za-z0-9])\.)+((?!-)[A-Za-z0-9-]{1,62}[A-Za-z0-9])$"
);

/// The order `TLSSecurityPolicy`'s value set is published in.
///
/// Spelled out rather than derived, so a change to either the rule or the values fails
/// this suite: the model declares the type `[Policy-Min-TLS-1-0-2019-07,
/// Policy-Min-TLS-1-2-2019-07, Policy-Min-TLS-1-2-PFS-2023-10,
/// Policy-Min-TLS-1-2-RFC9151-FIPS-2024-08, Policy-Min-TLS-1-2-FIPS-PQ-2026-10]`
/// (`smithy_query(query="shape",shape="TLSSecurityPolicy")`) and codegen hands the walk
/// them sorted, so this is neither of those two orders.
const PUBLISHED_TLS_POLICIES: [&str; 5] = [
    "Policy-Min-TLS-1-2-FIPS-PQ-2026-10",
    "Policy-Min-TLS-1-0-2019-07",
    "Policy-Min-TLS-1-2-2019-07",
    "Policy-Min-TLS-1-2-RFC9151-FIPS-2024-08",
    "Policy-Min-TLS-1-2-PFS-2023-10",
];

/// The `message` of an answer that must be a `400 ValidationException`, with the case
/// named on failure.
fn refusal(answer: &Answer, case: &str) -> String {
    assert_eq!(
        answer.status, 400,
        "case {case:?}: expected 400, got {} with {:?}",
        answer.status, answer.body
    );
    assert_eq!(
        error_code(answer),
        "ValidationException",
        "case {case:?}: expected ValidationException, got {:?}",
        answer.body
    );
    answer.body["message"]
        .as_str()
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected a refusal message, got {:?}",
                answer.body
            )
        })
        .to_owned()
}

/// A `DomainEndpointOptions` container naming `endpoint` as the domain's custom endpoint.
///
/// The container does not ENABLE the custom endpoint: an enabled one is served by a
/// certificate of this account that covers the host, which is a rule of its own (the
/// `custom_endpoint_certificate` suite) rather than the member syntax this table is about,
/// and the syntax is applied to the value whenever a request supplies it.
fn endpoint_options(endpoint: &str) -> Value {
    json!({
        "DomainEndpointOptions": {
            "EnforceHTTPS": true,
            "CustomEndpoint": endpoint,
        }
    })
}

/// One row of the SEVERAL-violations table: a request that breaks several constraints of
/// one member.
struct Several {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The member path every clause names.
    member: &'static str,
    /// The value the request submitted, which no clause may state.
    value: String,
    /// The requirement each clause states, in the order the report states them.
    requirements: Vec<&'static str>,
}

/// RULE 1: a member with SEVERAL violations is stated without the value it submitted, in
/// every one of its clauses — and every requirement it broke is still reported.
#[tokio::test]
async fn a_member_with_several_violations_is_stated_without_its_value() {
    let emulator = Emulator::new();

    let rows: Vec<Several> = vec![
        Several {
            // A domain name that is an ENDPOINT: past the upper bound and outside the
            // expression, on a URI label.
            case: "a URI label too long and outside its pattern",
            method: "GET",
            uri: format!(
                "{PREFIX}/opensearch/domain/vpc-probe-domain.us-east-1.es.amazonaws.com"
            ),
            body: None,
            member: "domainName",
            value: "vpc-probe-domain.us-east-1.es.amazonaws.com".to_string(),
            requirements: vec![AT_MOST_28, NAME_EXPRESSION],
        },
        Several {
            // The other bound, on another operation's label: below the floor AND outside
            // the expression.
            case: "a URI label too short and outside its pattern",
            method: "DELETE",
            uri: format!("{PREFIX}/opensearch/domain/A"),
            body: None,
            member: "domainName",
            value: "A".to_string(),
            requirements: vec![AT_LEAST_3, NAME_EXPRESSION],
        },
        Several {
            // The same member on the mutation path, which carries a body as well.
            case: "a label too long and outside its pattern on the update path",
            method: "POST",
            uri: format!(
                "{PREFIX}/opensearch/domain/PROBE.DOMAIN.THAT.IS.FAR.TOO.LONG/config"
            ),
            body: Some(json!({ "ClusterConfig": { "InstanceCount": 2 } })),
            member: "domainName",
            value: "PROBE.DOMAIN.THAT.IS.FAR.TOO.LONG".to_string(),
            requirements: vec![AT_MOST_28, NAME_EXPRESSION],
        },
        Several {
            // The same member in the BODY rather than on a label.
            case: "a body member too long and outside its pattern",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain"),
            body: Some(json!({ "DomainName": "Probe.Domain.Name.That.Is.Too.Long" })),
            member: "domainName",
            value: "Probe.Domain.Name.That.Is.Too.Long".to_string(),
            requirements: vec![AT_MOST_28, NAME_EXPRESSION],
        },
        Several {
            // Another resource's name member, over the same two constraint classes under
            // a different bound: `PackageName` is capped at 256 characters.
            case: "a package name too long and outside its pattern",
            method: "POST",
            uri: format!("{PREFIX}/packages"),
            body: Some(json!({
                "PackageName": "PROBE-PACKAGE-".repeat(20),
                "PackageType": "TXT-DICTIONARY",
                "PackageSource": { "S3BucketName": "pkg-bucket", "S3Key": "v1.txt" },
            })),
            member: "packageName",
            value: "PROBE-PACKAGE-".repeat(20),
            requirements: vec![AT_MOST_256, NAME_EXPRESSION],
        },
    ];

    for row in rows {
        let Several {
            case,
            method,
            uri,
            body,
            member,
            value,
            requirements,
        } = row;
        let answer = emulator.call(method, &uri, body).await;
        let stated = refusal(&answer, case);
        let expected = report(
            &requirements
                .iter()
                .map(|requirement| {
                    format!("Value at '{member}' failed to satisfy constraint: {requirement}")
                })
                .collect::<Vec<String>>(),
        );
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
        assert!(
            !stated.contains(&value),
            "case {case:?}: no clause may state the submitted value {value:?}, got {stated:?}"
        );
    }
}

/// One row of the ONE-violation table: a request that breaks exactly one constraint of
/// one member.
struct One {
    case: &'static str,
    method: &'static str,
    uri: String,
    body: Option<Value>,
    /// The value the clause echoes.
    value: &'static str,
    /// The member path the clause is stated at.
    path: &'static str,
    /// The requirement the clause states.
    requirement: String,
}

/// RULE 1, the other half: a member with ONE violation is stated WITH the value it
/// submitted, whatever that value is and wherever it was bound.
#[tokio::test]
async fn a_member_with_one_violation_states_the_value_it_submitted() {
    let emulator = Emulator::new();

    let rows: Vec<One> = vec![
        One {
            case: "a URI label outside its pattern",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/PROBEDOMAIN"),
            body: None,
            value: "PROBEDOMAIN",
            path: "domainName",
            requirement: NAME_EXPRESSION.to_string(),
        },
        One {
            case: "a URI label below its length floor",
            method: "GET",
            uri: format!("{PREFIX}/opensearch/domain/ab/config"),
            body: None,
            value: "ab",
            path: "domainName",
            requirement: AT_LEAST_3.to_string(),
        },
        One {
            // A nested body member, whose value is a long dotted one: not label-shaped,
            // and still echoed in full, because it broke ONE constraint.
            case: "a nested body member outside its pattern",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/probe-echo-domain/config"),
            body: Some(endpoint_options(
                "https://internal-probe-0123456789.us-east-1.elb.amazonaws.com",
            )),
            value: "https://internal-probe-0123456789.us-east-1.elb.amazonaws.com",
            path: CUSTOM_ENDPOINT_PATH,
            requirement: FQDN_EXPRESSION.to_string(),
        },
        One {
            // A nested body member outside its enumerated set.
            case: "a nested body member outside its value set",
            method: "POST",
            uri: format!("{PREFIX}/opensearch/domain/probe-echo-domain/config"),
            body: Some(json!({
                "DomainEndpointOptions": { "TLSSecurityPolicy": "policy-min-tls-1-2-2019-07" }
            })),
            value: "policy-min-tls-1-2-2019-07",
            path: "domainEndpointOptions.tLSSecurityPolicy",
            requirement: value_set(&PUBLISHED_TLS_POLICIES),
        },
    ];

    for row in rows {
        let One {
            case,
            method,
            uri,
            body,
            value,
            path,
            requirement,
        } = row;
        let answer = emulator.call(method, &uri, body).await;
        let stated = refusal(&answer, case);
        let expected = report(&[clause(value, path, &requirement)]);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE 2: an enumerated member's clause lists the whole admitted set in the order this
/// service publishes it — the same order whatever value was rejected and whichever
/// operation carried the member.
#[tokio::test]
async fn an_enumerated_member_lists_its_set_in_the_published_order() {
    let emulator = Emulator::new();

    // The published order is not the sort, so a set listed alphabetically fails the rows
    // below rather than passing them by coincidence.
    let mut sorted: Vec<&str> = PUBLISHED_TLS_POLICIES.to_vec();
    sorted.sort_unstable();
    assert_ne!(
        PUBLISHED_TLS_POLICIES.to_vec(),
        sorted,
        "the published order under test must differ from the sort"
    );

    // One row per shape of rejected value: an admitted value in another case, a policy
    // that does not exist, a near-miss of one that does, and one character.
    let rejected = [
        "policy-min-tls-1-2-2019-07",
        "Policy-Min-TLS-1-3-2030-01",
        "Policy-Min-TLS-1-2-PFS",
        "x",
    ];
    let expected_requirement = value_set(&PUBLISHED_TLS_POLICIES);

    for value in rejected {
        // The member travels on the create path and on the update path, and the update
        // path's row names a domain the store does not hold: the order belongs to the
        // TYPE, so neither the operation nor the state can change it.
        let updated = emulator
            .update_config(
                "probe-tls-order",
                json!({ "DomainEndpointOptions": { "TLSSecurityPolicy": value } }),
            )
            .await;
        let created = emulator
            .create(json!({
                "DomainName": "probe-tls-order",
                "EngineVersion": "OpenSearch_2.11",
                "DomainEndpointOptions": { "TLSSecurityPolicy": value },
            }))
            .await;
        for (operation, answer) in [("UpdateDomainConfig", updated), ("CreateDomain", created)] {
            let case = format!("{operation} with {value:?}");
            let stated = refusal(&answer, &case);
            let expected = report(&[clause(
                value,
                "domainEndpointOptions.tLSSecurityPolicy",
                &expected_requirement,
            )]);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
        }
    }
}

/// One row of the FQDN table: a custom endpoint and whether the member's syntax admits it.
struct Endpoint {
    case: &'static str,
    value: String,
    admitted: bool,
}

/// The custom endpoints the rule is asserted over: the shapes a value can miss the FQDN
/// syntax by, and the shapes it admits — including each published bound of a label's
/// length and one past it (`DomainNameFqdn`'s expression caps a label at 63 characters).
fn endpoint_rows() -> Vec<Endpoint> {
    vec![
        Endpoint {
            case: "a URL rather than a host",
            value: "https://internal-probe-0123456789.us-east-1.elb.amazonaws.com".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a single label with no dot",
            value: "searchprobe".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a label opening with a hyphen",
            value: "-probe.example.com".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a label closing with a hyphen",
            value: "probe-.example.com".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a character no label admits",
            value: "search_probe.example.com".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a trailing dot",
            value: "search.example.com.".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "an empty label",
            value: "search..example.com".to_string(),
            admitted: false,
        },
        Endpoint {
            case: "a label one past the 63-character bound",
            value: format!("{}.example.com", "a".repeat(64)),
            admitted: false,
        },
        Endpoint {
            case: "a label at the 63-character bound",
            value: format!("{}.example.com", "a".repeat(63)),
            admitted: true,
        },
        Endpoint {
            case: "the shortest fully qualified name",
            value: "a.bc".to_string(),
            admitted: true,
        },
        Endpoint {
            case: "a host of several labels",
            value: "search.probe-domain.example.com".to_string(),
            admitted: true,
        },
    ]
}

/// RULE 3: the custom endpoint's syntax is applied BEFORE the domain is resolved, so a
/// request carrying a malformed one is refused by naming that member — and a request
/// carrying one the syntax admits is carried past the constraint to whatever the store
/// says about the domain it names.
#[tokio::test]
async fn an_input_constraint_is_stated_before_the_domain_is_resolved() {
    let emulator = Emulator::new();

    // No domain of this name was ever created, so a lookup taken first would answer that
    // it does not exist — which is what pins the ORDER of the two steps.
    let absent = "probe-absent-domain";

    for row in endpoint_rows() {
        let Endpoint {
            case,
            value,
            admitted,
        } = row;
        let answer = emulator
            .update_config(absent, endpoint_options(&value))
            .await;
        if admitted {
            assert_eq!(
                answer.status, RECORDED_NOT_FOUND_STATUS,
                "case {case:?}: an admitted endpoint must be carried past the constraint, \
                 got {} with {:?}",
                answer.status, answer.body
            );
            assert_eq!(
                error_code(&answer),
                "ResourceNotFoundException",
                "case {case:?}: expected the missing-domain refusal, got {:?}",
                answer.body
            );
            continue;
        }
        let stated = refusal(&answer, case);
        let expected = report(&[clause(&value, CUSTOM_ENDPOINT_PATH, FQDN_EXPRESSION)]);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE 3 on the create path: the same constraint, stated the same way, and no domain is
/// created by a request it refuses — while a request whose endpoint it admits creates one
/// that reports the endpoint back.
#[tokio::test]
async fn the_create_path_states_the_same_constraint_and_stores_what_it_admits() {
    let emulator = Emulator::new();

    for (index, row) in endpoint_rows().into_iter().enumerate() {
        let Endpoint {
            case,
            value,
            admitted,
        } = row;
        // One domain name per row, so no row is answered for a name another row created.
        let name = format!("probe-create-{index}");
        let mut body = json!({ "DomainName": name, "EngineVersion": "OpenSearch_2.11" });
        body["DomainEndpointOptions"] = endpoint_options(&value)["DomainEndpointOptions"].clone();
        let answer = emulator.create(body).await;

        if !admitted {
            let stated = refusal(&answer, case);
            let expected = report(&[clause(&value, CUSTOM_ENDPOINT_PATH, FQDN_EXPRESSION)]);
            assert_eq!(
                stated, expected,
                "case {case:?}: expected {expected:?}, got {stated:?}"
            );
            // And nothing was created: the refusal is the whole of what the call did.
            let read = emulator.describe(&name).await;
            assert_eq!(
                read.status, RECORDED_NOT_FOUND_STATUS,
                "case {case:?}: a refused create must leave no domain behind, got {} with {:?}",
                read.status, read.body
            );
            continue;
        }

        assert_eq!(
            answer.status, 200,
            "case {case:?}: an admitted endpoint must be accepted, got {} with {:?}",
            answer.status, answer.body
        );
        // Read the stored record back through the configuration reader: what was admitted
        // is what the domain reports.
        let read = emulator.describe_config(&name).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the created configuration, got {} with {:?}",
            read.status, read.body
        );
        let stored = read.body["DomainConfig"]["DomainEndpointOptions"]["Options"]["CustomEndpoint"]
            .as_str()
            .unwrap_or_default();
        assert_eq!(
            stored, value,
            "case {case:?}: expected the stored custom endpoint {value:?}, got {stored:?}"
        );
    }
}
