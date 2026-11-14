// SPDX-License-Identifier: CC0-1.0

//! The FHIR type-level `search` interaction of a data store's REST data plane:
//! `GET [base]/{type}?<param>=<value>`.
//!
//! The rule under test, over a TABLE of criteria rather than the one recorded case: a
//! search answers HTTP 200 and a Bundle of type `searchset` holding one `entry` per
//! resource of that type the data store holds that satisfies EVERY parameter of the
//! query — the resource exactly as a read of it answers — with `total` stating how many
//! matched; a criterion nothing satisfies is that same 200 with no entries, never a 404.
//!
//! How a criterion composes is FHIR R4 §3.1.1's: distinct and repeated parameters are
//! AND-ed, the comma-separated values of one parameter are OR-ed, and a `token` parameter
//! (`identifier`) takes the four forms `code`, `system|code`, `|code` and `system|`
//! (§3.1.1.5.4). `_id` selects a resource by its logical id and `_count` asks for a page
//! size (§3.1.1.4, where the server "is not bound to return the number requested").
//!
//! The rows differ along what the rule generalises over: the token form, whether the
//! stored identifier carries a system, how many resources match (none, one, several),
//! whether the match is narrowed by one parameter or by two, the resource TYPE the search
//! is addressed to, and the page size asked for. The case the issue recorded
//! (`GET [base]/Patient?identifier=<id>` after `PUT [base]/Patient/<id>`) is one row.
//!
//! A `|` is not a character a URI may carry raw, so a token row sends it percent-encoded
//! (`%7C`) exactly as a client's URL builder does — which also drives the endpoint's
//! percent-decoding of a query value.
//!
//! The harness (`HealthLake`, `outcome_code`) is the sibling suites': one way of driving
//! this service through the production router, not two.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use super::fhir_datastore_lifecycle::EMULATOR_BASE;
use super::fhir_rest_and_validate_source::{outcome_code, HealthLake};

/// The system a seeded identifier carries, and a second one, so a `system|code` row can
/// be told from a `code` row.
const SYSTEM: &str = "http://example.org/mrn";

/// A second identifier system, for the row that names the other one.
const OTHER_SYSTEM: &str = "http://example.org/ssn";

/// Store `resource` at `[base]/{resource_type}/{id}` through the `update` interaction,
/// asserting it was stored, and return the id it is readable under.
async fn seeded(
    service: &HealthLake,
    path: &str,
    resource_type: &str,
    id: &str,
    resource: &Value,
) -> String {
    let (status, body, _) = service
        .fhir("PUT", &format!("{path}{resource_type}/{id}"), resource)
        .await;
    assert!(
        status == 200 || status == 201,
        "seeding {resource_type}/{id} must store the resource, got {status}: {body}"
    );
    id.to_string()
}

/// The logical ids the entries of a searchset Bundle carry.
fn matched_ids(case: &str, bundle: &Value) -> BTreeSet<String> {
    match bundle.get("entry") {
        None => BTreeSet::new(),
        Some(Value::Array(entries)) => entries
            .iter()
            .map(|entry| {
                entry["resource"]["id"]
                    .as_str()
                    .unwrap_or_else(|| {
                        panic!("case {case:?}: every entry carries a resource with an id, got {entry}")
                    })
                    .to_string()
            })
            .collect(),
        Some(other) => panic!("case {case:?}: entry must be a list, got {other}"),
    }
}

/// Assert everything every searchset answer must state, and return the Bundle.
async fn searched(service: &HealthLake, case: &str, uri: &str) -> Value {
    let (status, bundle, headers) = service.fhir("GET", uri, &Value::Null).await;
    assert_eq!(
        status, 200,
        "case {case:?}: a search answers 200 whether or not anything matched, got \
         {status}: {bundle}"
    );
    assert_eq!(
        headers.get("content-type").map(String::as_str),
        Some("application/fhir+json"),
        "case {case:?}: a FHIR response carries the FHIR media type, got {headers:?}"
    );
    assert_eq!(
        (
            bundle["resourceType"].as_str(),
            bundle["type"].as_str()
        ),
        (Some("Bundle"), Some("searchset")),
        "case {case:?}: a search answers a searchset Bundle, got {bundle}"
    );
    bundle
}

#[tokio::test]
async fn a_type_level_search_answers_every_resource_its_criterion_selects() {
    let service = HealthLake::new();
    let (_, path) = service.active_datastore("search-datastore").await;

    // Three resources, differing in what a search can select them by: one Patient whose
    // identifier carries a system, one carrying two identifiers (one of them with no
    // system at all), and an Observation carrying the SAME identifier value as the first
    // Patient, so a search of one type never answers a resource of another.
    let ada = seeded(
        &service,
        &path,
        "Patient",
        "sim2real-patient-1",
        &json!({
            "resourceType": "Patient",
            "identifier": [{ "system": SYSTEM, "value": "mrn-0001" }],
            "name": [{ "family": "Lovelace", "given": ["Ada"] }],
        }),
    )
    .await;
    let grace = seeded(
        &service,
        &path,
        "Patient",
        "patient.2",
        &json!({
            "resourceType": "Patient",
            "identifier": [
                { "system": OTHER_SYSTEM, "value": "mrn-0002" },
                { "value": "systemless-0003" },
            ],
            "name": [{ "family": "Hopper", "given": ["Grace"] }],
        }),
    )
    .await;
    let observation = seeded(
        &service,
        &path,
        "Observation",
        "obs-1",
        &json!({
            "resourceType": "Observation",
            "identifier": [{ "system": SYSTEM, "value": "mrn-0001" }],
            "status": "final",
        }),
    )
    .await;

    let both: BTreeSet<String> = [ada.clone(), grace.clone()].into_iter().collect();
    let only_ada: BTreeSet<String> = [ada.clone()].into_iter().collect();
    let only_grace: BTreeSet<String> = [grace.clone()].into_iter().collect();
    let none: BTreeSet<String> = BTreeSet::new();

    // (case, the type and query the search is addressed with, the ids that must match)
    let rows: Vec<(&str, String, BTreeSet<String>)> = vec![
        (
            "the recorded case: a bare identifier value",
            "Patient?identifier=mrn-0001".to_string(),
            only_ada.clone(),
        ),
        (
            "a system-qualified identifier",
            format!("Patient?identifier={SYSTEM}%7Cmrn-0001"),
            only_ada.clone(),
        ),
        (
            "the right value in the wrong system",
            format!("Patient?identifier={OTHER_SYSTEM}%7Cmrn-0001"),
            none.clone(),
        ),
        (
            "any value in one system",
            format!("Patient?identifier={OTHER_SYSTEM}%7C"),
            only_grace.clone(),
        ),
        (
            "a value held with no system",
            "Patient?identifier=%7Csystemless-0003".to_string(),
            only_grace.clone(),
        ),
        (
            "a systemless search for a value stored with a system",
            "Patient?identifier=%7Cmrn-0001".to_string(),
            none.clone(),
        ),
        (
            "two alternatives of one parameter are OR-ed",
            "Patient?identifier=mrn-0001,mrn-0002".to_string(),
            both.clone(),
        ),
        (
            "an identifier nothing carries",
            "Patient?identifier=mrn-9999".to_string(),
            none.clone(),
        ),
        (
            "the logical id",
            format!("Patient?_id={ada}"),
            only_ada.clone(),
        ),
        (
            "two logical ids",
            format!("Patient?_id={ada},{grace}"),
            both.clone(),
        ),
        (
            "two parameters that agree are AND-ed",
            format!("Patient?_id={grace}&identifier=mrn-0002"),
            only_grace.clone(),
        ),
        (
            "two parameters that disagree are AND-ed",
            format!("Patient?_id={ada}&identifier=mrn-0002"),
            none.clone(),
        ),
        (
            "no parameter at all selects every resource of the type",
            "Patient".to_string(),
            both.clone(),
        ),
        (
            "the same identifier value under another resource type",
            "Observation?identifier=mrn-0001".to_string(),
            [observation.clone()].into_iter().collect(),
        ),
        (
            "a type the data store holds nothing of",
            "Practitioner?identifier=mrn-0001".to_string(),
            none.clone(),
        ),
    ];

    for (case, query, expected) in &rows {
        let bundle = searched(&service, case, &format!("{path}{query}")).await;
        assert_eq!(
            bundle["total"].as_u64(),
            Some(expected.len() as u64),
            "case {case:?}: total states how many matched, got {bundle}"
        );
        let matched = matched_ids(case, &bundle);
        assert_eq!(
            &matched, expected,
            "case {case:?}: expected the matches {expected:?}, got {matched:?} in {bundle}"
        );
        if expected.is_empty() {
            assert!(
                bundle.get("entry").is_none(),
                "case {case:?}: an empty result carries no entry member, got {bundle}"
            );
        }
        if let Some(Value::Array(entries)) = bundle.get("entry") {
            for entry in entries {
                let id = entry["resource"]["id"].as_str().unwrap_or_default();
                let resource_type = entry["resource"]["resourceType"]
                    .as_str()
                    .unwrap_or_default();
                assert_eq!(
                    entry["search"]["mode"].as_str(),
                    Some("match"),
                    "case {case:?}: an entry the criterion selected is a match, got {entry}"
                );
                assert!(
                    entry["fullUrl"]
                        .as_str()
                        .is_some_and(|url| url.ends_with(&format!("{path}{resource_type}/{id}"))),
                    "case {case:?}: the entry's fullUrl addresses the resource at this \
                     endpoint, got {entry}"
                );
                // The entry carries the resource as it is STORED: the same bytes a read
                // of it answers, server-owned `meta` included.
                let (status, read, _) = service
                    .fhir(
                        "GET",
                        &format!("{path}{resource_type}/{id}"),
                        &Value::Null,
                    )
                    .await;
                assert_eq!(
                    (status, &entry["resource"]),
                    (200, &read),
                    "case {case:?}: the entry must be the stored resource, got {entry}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_search_parameter_this_endpoint_does_not_implement_is_refused() {
    let service = HealthLake::new();
    let (_, path) = service.active_datastore("search-refusals").await;
    seeded(
        &service,
        &path,
        "Patient",
        "refusal-patient",
        &json!({
            "resourceType": "Patient",
            "identifier": [{ "value": "mrn-0001" }],
            "name": [{ "family": "Lovelace" }],
        }),
    )
    .await;

    // A parameter the endpoint does not implement is REFUSED rather than dropped: a
    // dropped filter would answer resources the caller never asked for and call them
    // matches. One row per kind of parameter a FHIR client sends — a resource-specific
    // search parameter, a result parameter that reorders, one that includes further
    // resources, and a modifier on a parameter that IS implemented.
    for (case, query, code) in [
        ("a resource search parameter", "Patient?name=Lovelace", "not-supported"),
        ("a result parameter", "Patient?_sort=_id", "not-supported"),
        (
            "an inclusion parameter",
            "Patient?_include=Patient:organization",
            "not-supported",
        ),
        (
            "a modifier on an implemented parameter",
            "Patient?identifier:exact=mrn-0001",
            "not-supported",
        ),
        ("a page size that is not a number", "Patient?_count=many", "invalid"),
    ] {
        let (status, body, _) = service
            .fhir("GET", &format!("{path}{query}"), &Value::Null)
            .await;
        assert_eq!(
            (status, outcome_code(&body).as_str()),
            (400, code),
            "case {case:?}: expected 400 {code}, got {status}: {body}"
        );
        assert_eq!(
            body["resourceType"].as_str(),
            Some("OperationOutcome"),
            "case {case:?}: a FHIR refusal is an OperationOutcome, got {body}"
        );
    }

    // The capability statement is its own interaction at a lower-case segment, not a
    // search of a resource type: it keeps the "not implemented yet" answer every other
    // unimplemented interaction on this path space gets.
    let (status, body, _) = service
        .fhir("GET", &format!("{path}metadata"), &Value::Null)
        .await;
    assert_eq!(
        (status, outcome_code(&body).as_str()),
        (501, "not-supported"),
        "the capability statement must stay an unimplemented interaction, got {status}: \
         {body}"
    );

    // The implemented parameters still answer, so the refusals above are the endpoint
    // declining what it does not serve rather than search being broken.
    let bundle = searched(
        &service,
        "an implemented parameter after the refusals",
        &format!("{path}Patient?identifier=mrn-0001"),
    )
    .await;
    assert_eq!(
        bundle["total"].as_u64(),
        Some(1),
        "the seeded Patient must still be searchable, got {bundle}"
    );
}

#[tokio::test]
async fn a_page_size_bounds_the_entries_it_carries_but_not_the_total() {
    let service = HealthLake::new();
    let (_, path) = service.active_datastore("search-paging").await;

    // Three resources one criterion selects, so a page smaller than the match count has
    // something to leave out.
    let held = 3;
    for index in 0..held {
        seeded(
            &service,
            &path,
            "Patient",
            &format!("paged-{index}"),
            &json!({
                "resourceType": "Patient",
                "identifier": [{ "value": "mrn-paged" }],
            }),
        )
        .await;
    }

    // (case, the page size asked for, how many entries the page carries)
    for (case, count, entries) in [
        ("no page size asked for", None, held),
        ("a page smaller than the match count", Some(2), 2),
        ("a page of one", Some(1), 1),
        ("a page of none", Some(0), 0),
        ("a page larger than the match count", Some(50), held),
        ("a page larger than anything one data store retains", Some(100_000), held),
    ] {
        let query = match count {
            Some(count) => format!("{path}Patient?identifier=mrn-paged&_count={count}"),
            None => format!("{path}Patient?identifier=mrn-paged"),
        };
        let bundle = searched(&service, case, &query).await;
        assert_eq!(
            bundle["total"].as_u64(),
            Some(held as u64),
            "case {case:?}: total states every match, not the page's size, got {bundle}"
        );
        assert_eq!(
            matched_ids(case, &bundle).len(),
            entries,
            "case {case:?}: expected {entries} entries, got {bundle}"
        );
    }
}

#[tokio::test]
async fn the_advertised_endpoint_is_this_emulators_own_and_serves_a_search_sent_to_it() {
    let service = HealthLake::new();
    let (id, path) = service.active_datastore("search-endpoint").await;
    let (status, described) = service
        .rpc("DescribeFHIRDatastore", &json!({ "DatastoreId": id }))
        .await;
    assert_eq!(status, 200, "DescribeFHIRDatastore got {status}: {described}");
    let endpoint = described["DatastoreProperties"]["DatastoreEndpoint"]
        .as_str()
        .unwrap_or_else(|| panic!("the data store reports a DatastoreEndpoint, got {described}"))
        .to_string();

    // The address a caller SENDS its FHIR interactions to leads back to this emulator: it
    // is this emulator's own base URL followed by the data store's path, never the real
    // service's host, where nothing would answer.
    assert_eq!(
        endpoint,
        format!("{EMULATOR_BASE}{path}"),
        "the advertised endpoint must be this emulator's own, got {endpoint:?}"
    );

    // And it is served AS ADVERTISED: both the write and the search below are addressed
    // with the absolute URL the data store published, exactly as a client that was handed
    // that endpoint sends them.
    let (status, written, _) = service
        .fhir(
            "PUT",
            &format!("{endpoint}Patient/endpoint-patient"),
            &json!({
                "resourceType": "Patient",
                "identifier": [{ "value": "mrn-endpoint" }],
            }),
        )
        .await;
    assert!(
        status == 200 || status == 201,
        "the advertised endpoint must serve a write, got {status}: {written}"
    );
    let bundle = searched(
        &service,
        "the advertised endpoint",
        &format!("{endpoint}Patient?identifier=mrn-endpoint"),
    )
    .await;
    assert_eq!(
        (bundle["total"].as_u64(), matched_ids("the advertised endpoint", &bundle).len()),
        (Some(1), 1),
        "the advertised endpoint must serve a search of what it stored, got {bundle}"
    );
}

#[tokio::test]
async fn a_search_reports_without_changing_what_a_later_caller_sees() {
    let service = HealthLake::new();
    let (_, path) = service.active_datastore("search-reporting").await;
    let id = seeded(
        &service,
        &path,
        "Patient",
        "reported-patient",
        &json!({
            "resourceType": "Patient",
            "identifier": [{ "system": SYSTEM, "value": "mrn-report" }],
        }),
    )
    .await;

    let query = format!("{path}Patient?identifier=mrn-report");
    let first = searched(&service, "the first search", &query).await;
    let second = searched(&service, "the second search", &query).await;
    assert_eq!(
        first, second,
        "two searches of one criterion must answer identically, got {first} then {second}"
    );

    // Everything the resource was before the searches, it still is: readable by id, at
    // the same version, and still the one match of the criterion.
    let (status, read, _) = service
        .fhir("GET", &format!("{path}Patient/{id}"), &Value::Null)
        .await;
    assert_eq!(
        (status, read["meta"]["versionId"].as_str()),
        (200, Some("1")),
        "the searched resource must be unchanged, got {status}: {read}"
    );
    let third = searched(&service, "a search after the read", &query).await;
    assert_eq!(
        third, first,
        "a read between two searches changes neither, got {third}"
    );

    // A search is addressed to a data store's endpoint like every other interaction, so
    // an endpoint no ACTIVE data store serves is the same not-found the read answers.
    let (status, body, _) = service
        .fhir(
            "GET",
            "/datastore/0123456789abcdef0123456789abcdef/r4/Patient?identifier=mrn-report",
            &Value::Null,
        )
        .await;
    assert_eq!(
        (status, outcome_code(&body).as_str()),
        (404, "not-found"),
        "a search of an endpoint no data store serves must be refused, got {status}: {body}"
    );
}
