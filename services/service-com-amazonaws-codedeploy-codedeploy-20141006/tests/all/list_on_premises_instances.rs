// SPDX-License-Identifier: CC0-1.0

//! `ListOnPremisesInstances`, driven over the wire.
//!
//! RULE under test: the listing answers the names of the on-premises instance
//! records the store holds, in one deterministic order, one page of
//! `nextToken`-continued results at a time, RESTRICTED by the two restricting
//! input members and by nothing else — `registrationStatus` keeps only the
//! records in that registration state (absent: the hosts that hold a
//! registration, which is the fleet the listing reports), and `tagFilters` keeps
//! only the records matching ANY of its filters (absent or empty: no
//! restriction, and a list longer than the service scans is refused) — and it
//! changes nothing a later caller sees.
//!
//! Every case goes through the same front door a client uses — the shared
//! [`Emulator`] harness of `codedeploy_lifecycle` — so the boundary validator,
//! this service's refusal wording and the awsJson1_1 envelope are exercised.

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::codedeploy_lifecycle::{error_code, Emulator};

/// One row of the restriction table: the case name, the request to list with,
/// and the instance names it must answer.
type ListCase<'a> = (&'a str, Value, &'a [&'a str]);

/// An IAM user ARN for `name`: `arn:aws:iam::<account>:user/<name>`.
fn user_arn(name: &str) -> String {
    format!("arn:aws:iam::{}:user/{name}", engine::emulator_account_id())
}

/// Register `name` against an IAM user ARN built from it.
async fn register(emulator: &Emulator, name: &str) {
    emulator
        .ok(
            "RegisterOnPremisesInstance",
            json!({"instanceName": name, "iamUserArn": user_arn(name)}),
        )
        .await;
}

/// Store `tags` on `name`, as CodeDeploy's `Tag` shape names its members
/// (`smithy_query(query="shape",shape="Tag")`: `Key`, `Value`).
async fn tag(emulator: &Emulator, name: &str, tags: &[(&str, &str)]) {
    let tags: Vec<Value> = tags
        .iter()
        .map(|(key, value)| json!({"Key": key, "Value": value}))
        .collect();
    emulator
        .ok(
            "AddTagsToOnPremisesInstances",
            json!({"instanceNames": [name], "tags": tags}),
        )
        .await;
}

/// A `tagFilters` entry, as CodeDeploy's `TagFilter` shape names its members
/// (`smithy_query(query="shape",shape="TagFilter")`: `Key`, `Value`, `Type`).
/// A member passed as `None` is left out of the request entirely.
fn filter(key: Option<&str>, value: Option<&str>, kind: Option<&str>) -> Value {
    let mut entry = json!({});
    if let Some(key) = key {
        entry["Key"] = json!(key);
    }
    if let Some(value) = value {
        entry["Value"] = json!(value);
    }
    if let Some(kind) = kind {
        entry["Type"] = json!(kind);
    }
    entry
}

/// A `tagFilters` list of `count` filters, the first of which selects the
/// `env=prod` records and the rest nothing — used to drive the list LENGTH,
/// which is what `MAX_TAG_FILTERS_PER_LIST` bounds.
fn filters_of_length(count: usize) -> Value {
    let mut filters = vec![filter(Some("env"), Some("prod"), Some("KEY_AND_VALUE"))];
    for index in 1..count {
        filters.push(filter(
            Some(&format!("filler-{index}")),
            Some("nowhere"),
            Some("KEY_AND_VALUE"),
        ));
    }
    json!(filters)
}

/// The names in a listing's `instanceNames` member.
fn instance_names(body: &Value) -> Vec<String> {
    body["instanceNames"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter_map(|name| name.as_str().map(str::to_string))
        .collect()
}

/// Every page of the listing `request` asks for, concatenated, following
/// `nextToken` to the end.
///
/// Asserts along the way what paging itself owes a caller: a page that carries a
/// continuation token is a FULL page (so the walk cannot be padded out with
/// short pages), no page repeats a name an earlier one carried, and the walk
/// terminates.
async fn list_every_page(emulator: &Emulator, case: &str, request: &Value) -> Vec<String> {
    let page_size = service_com_amazonaws_codedeploy_codedeploy_20141006::state::PAGE_SIZE;
    let mut collected: Vec<String> = Vec::new();
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut token: Option<String> = None;
    let mut pages = 0;
    loop {
        let mut page_request = request.clone();
        match &token {
            Some(token) => page_request["nextToken"] = json!(token),
            None => {
                page_request
                    .as_object_mut()
                    .expect("the listing request is an object")
                    .remove("nextToken");
            }
        }
        let page = emulator
            .ok("ListOnPremisesInstances", page_request)
            .await;
        let names = instance_names(&page);
        pages += 1;
        assert!(
            names.len() <= page_size,
            "case {case:?}: expected at most {page_size} names on page {pages}, got {names:?}"
        );
        for name in &names {
            assert!(
                seen.insert(name.clone()),
                "case {case:?}: page {pages} repeated {name}, got {names:?}"
            );
        }
        collected.extend(names.clone());
        token = page["nextToken"].as_str().map(str::to_string);
        match &token {
            Some(_) => assert_eq!(
                names.len(),
                page_size,
                "case {case:?}: a page carrying a nextToken must be full, got {names:?}"
            ),
            None => return collected,
        }
        assert!(
            pages <= 50,
            "case {case:?}: paging did not terminate after {pages} pages"
        );
    }
}

/// A store holding five on-premises instance records, registered in this order:
///
/// | name          | tags                      | registration   |
/// |---------------|---------------------------|----------------|
/// | `web-1`       | `env=prod`, `role=web`    | registered     |
/// | `web-2`       | `env=staging`, `role=web` | registered     |
/// | `db-1`        | `env=prod`, `role=db`     | registered     |
/// | `retired-1`   | `env=prod`, `role=web`    | DEREGISTERED   |
/// | `untagged-1`  | none                      | registered     |
async fn seeded_fleet() -> Emulator {
    let emulator = Emulator::new();
    let fleet: &[(&str, &[(&str, &str)])] = &[
        ("web-1", &[("env", "prod"), ("role", "web")]),
        ("web-2", &[("env", "staging"), ("role", "web")]),
        ("db-1", &[("env", "prod"), ("role", "db")]),
        ("retired-1", &[("env", "prod"), ("role", "web")]),
        ("untagged-1", &[]),
    ];
    for (name, tags) in fleet {
        register(&emulator, name).await;
        if !tags.is_empty() {
            tag(&emulator, name, tags).await;
        }
    }
    // Deregistered AFTER it was tagged: the record (and its tags) outlives the
    // registration, which is what the `Deregistered` status selects — and what
    // the unrestricted listing, which reports the fleet that holds a
    // registration, no longer carries.
    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": "retired-1"}),
        )
        .await;
    emulator
}

/// RULE: the listing answers exactly the stored records the request's two
/// restricting members select — `registrationStatus` by registration state, an
/// absent one selecting the hosts that hold a registration, and `tagFilters` by
/// ANY of its filters, an absent one restricting nothing — and the same request
/// answers the same names every time.
#[tokio::test]
async fn the_listing_answers_the_records_the_request_selects() {
    let emulator = seeded_fleet().await;
    // Rows differ along the registration status asked for, the NUMBER of tag
    // filters (none, an empty list, one, two), and the filter's `Type` — every
    // value of `TagFilterType`
    // (`smithy_query(query="shape",shape="TagFilterType")`: KEY_ONLY |
    // VALUE_ONLY | KEY_AND_VALUE) plus a filter that states no type at all —
    // including filters that select everything and filters that select nothing.
    let cases: &[ListCase] = &[
        (
            "no restriction at all: the registered hosts",
            json!({}),
            &["web-1", "web-2", "db-1", "untagged-1"],
        ),
        (
            "an EMPTY tag filter list restricts nothing",
            json!({"tagFilters": []}),
            &["web-1", "web-2", "db-1", "untagged-1"],
        ),
        (
            "only the registered records",
            json!({"registrationStatus": "Registered"}),
            &["web-1", "web-2", "db-1", "untagged-1"],
        ),
        (
            "only the deregistered records",
            json!({"registrationStatus": "Deregistered"}),
            &["retired-1"],
        ),
        (
            "KEY_AND_VALUE: one key/value pair",
            json!({"tagFilters": [filter(Some("env"), Some("prod"), Some("KEY_AND_VALUE"))]}),
            &["web-1", "db-1"],
        ),
        (
            "KEY_AND_VALUE, narrowed to the registered records",
            json!({
                "registrationStatus": "Registered",
                "tagFilters": [filter(Some("env"), Some("prod"), Some("KEY_AND_VALUE"))],
            }),
            &["web-1", "db-1"],
        ),
        (
            "KEY_AND_VALUE, narrowed to the deregistered records",
            json!({
                "registrationStatus": "Deregistered",
                "tagFilters": [filter(Some("env"), Some("prod"), Some("KEY_AND_VALUE"))],
            }),
            &["retired-1"],
        ),
        (
            "KEY_ONLY: the value is not looked at",
            json!({"tagFilters": [filter(Some("role"), Some("ignored"), Some("KEY_ONLY"))]}),
            &["web-1", "web-2", "db-1"],
        ),
        (
            "VALUE_ONLY: the key is not looked at",
            json!({"tagFilters": [filter(Some("ignored"), Some("db"), Some("VALUE_ONLY"))]}),
            &["db-1"],
        ),
        (
            "no Type, both members named: the pair has to match",
            json!({"tagFilters": [filter(Some("env"), Some("staging"), None)]}),
            &["web-2"],
        ),
        (
            "no Type, key only: the key has to match",
            json!({"tagFilters": [filter(Some("role"), None, None)]}),
            &["web-1", "web-2", "db-1"],
        ),
        (
            "no Type, value only: the value has to match",
            json!({"tagFilters": [filter(None, Some("web"), None)]}),
            &["web-1", "web-2"],
        ),
        (
            "two filters: a record matching EITHER is selected",
            json!({"tagFilters": [
                filter(Some("env"), Some("staging"), Some("KEY_AND_VALUE")),
                filter(Some("role"), Some("db"), Some("KEY_AND_VALUE")),
            ]}),
            &["web-2", "db-1"],
        ),
        (
            "two filters, one of them matching nothing",
            json!({"tagFilters": [
                filter(Some("env"), Some("nowhere"), Some("KEY_AND_VALUE")),
                filter(Some("role"), Some("db"), Some("KEY_AND_VALUE")),
            ]}),
            &["db-1"],
        ),
        (
            "a tag no record carries selects nothing",
            json!({"tagFilters": [filter(Some("env"), Some("nowhere"), Some("KEY_AND_VALUE"))]}),
            &[],
        ),
        (
            "KEY_AND_VALUE naming only a key describes no tag",
            json!({"tagFilters": [filter(Some("env"), None, Some("KEY_AND_VALUE"))]}),
            &[],
        ),
        (
            "a filter naming neither member describes no tag",
            json!({"tagFilters": [filter(None, None, None)]}),
            &[],
        ),
        (
            "a filter list AT the bound this service scans",
            json!({"tagFilters": filters_of_length(
                service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_TAG_FILTERS_PER_LIST,
            )}),
            &["web-1", "db-1"],
        ),
        (
            "a status and a filter that disagree select nothing",
            json!({
                "registrationStatus": "Deregistered",
                "tagFilters": [filter(Some("role"), Some("db"), Some("KEY_AND_VALUE"))],
            }),
            &[],
        ),
    ];

    for (case, request, expected) in cases {
        let answered = list_every_page(&emulator, case, request).await;
        let expected_set: BTreeSet<String> = expected.iter().map(|name| name.to_string()).collect();
        let answered_set: BTreeSet<String> = answered.iter().cloned().collect();
        assert_eq!(
            answered_set, expected_set,
            "case {case:?}: expected {expected:?}, got {answered:?}"
        );
        assert_eq!(
            answered.len(),
            expected.len(),
            "case {case:?}: expected each name once, got {answered:?}"
        );

        // Reporting changes nothing a later caller sees: the same request
        // answers the same names, in the same order, afterwards.
        let again = list_every_page(&emulator, case, request).await;
        assert_eq!(
            again, answered,
            "case {case:?}: expected a repeatable listing, got {again:?}"
        );
    }

    // Nor did any listing disturb the records themselves: each registered
    // instance still reads back through the single-instance reader, with the
    // tags it was given.
    for name in ["web-1", "web-2", "db-1", "untagged-1"] {
        let info = emulator
            .ok("GetOnPremisesInstance", json!({"instanceName": name}))
            .await["instanceInfo"]
            .clone();
        assert_eq!(
            info["instanceName"].as_str(),
            Some(name),
            "case {name:?}: expected the listing to leave the record alone, got {info:?}"
        );
        assert!(
            info["deregisterTime"].is_null(),
            "case {name:?}: expected it to still be registered, got {info:?}"
        );
    }
}

/// RULE: the listing is PAGED on `nextToken` — a page carries at most one page
/// of names, and following the token walks every selected record exactly once,
/// whatever the restriction and however many records are selected.
#[tokio::test]
async fn the_listing_pages_without_repeating_or_skipping() {
    let emulator = Emulator::new();
    let page_size = service_com_amazonaws_codedeploy_codedeploy_20141006::state::PAGE_SIZE;
    // One page's worth plus a remainder, so a walk takes more than one page and
    // the last page is a short one. Every third instance is deregistered and
    // every other one is tagged, so each restriction selects a different count;
    // an unrestricted listing reports the hosts that hold a registration, so a
    // tag filter with no status alongside it selects the tagged ones among those.
    let total = page_size + page_size / 2 + 3;
    let mut registered: Vec<String> = Vec::new();
    let mut deregistered: Vec<String> = Vec::new();
    let mut tagged: Vec<String> = Vec::new();
    for index in 0..total {
        let name = format!("paged-{index:04}");
        register(&emulator, &name).await;
        if index % 2 == 0 {
            tag(&emulator, &name, &[("tier", "edge")]).await;
        }
        if index % 3 == 0 {
            emulator
                .ok(
                    "DeregisterOnPremisesInstance",
                    json!({"instanceName": name.clone()}),
                )
                .await;
            deregistered.push(name.clone());
        } else {
            registered.push(name.clone());
            if index % 2 == 0 {
                tagged.push(name.clone());
            }
        }
    }

    // Rows differ along the restriction, and so along how many records the walk
    // has to page through.
    let cases: &[(&str, Value, &Vec<String>)] = &[
        ("no restriction: the registered ones", json!({}), &registered),
        (
            "the registered ones",
            json!({"registrationStatus": "Registered"}),
            &registered,
        ),
        (
            "the deregistered ones",
            json!({"registrationStatus": "Deregistered"}),
            &deregistered,
        ),
        (
            "the tagged ones that hold a registration",
            json!({"tagFilters": [filter(Some("tier"), Some("edge"), Some("KEY_AND_VALUE"))]}),
            &tagged,
        ),
    ];

    for (case, request, expected) in cases {
        let answered = list_every_page(&emulator, case, request).await;
        assert_eq!(
            answered.len(),
            expected.len(),
            "case {case:?}: expected {} names, got {}",
            expected.len(),
            answered.len()
        );
        let answered_set: BTreeSet<&String> = answered.iter().collect();
        let expected_set: BTreeSet<&String> = expected.iter().collect();
        assert_eq!(
            answered_set, expected_set,
            "case {case:?}: expected the selected records, got {answered:?}"
        );
        // More than one page was actually walked where more than a page is
        // selected, so the token path is exercised rather than assumed.
        if expected.len() > page_size {
            let first = emulator
                .ok("ListOnPremisesInstances", request.clone())
                .await;
            assert_eq!(
                instance_names(&first).len(),
                page_size,
                "case {case:?}: expected a full first page, got {first:?}"
            );
            assert!(
                first["nextToken"].as_str().is_some(),
                "case {case:?}: expected a nextToken on a full page, got {first:?}"
            );
        }
    }
}

/// RULE: a request the listing will not accept is refused with HTTP 400 and the
/// modelled error for the member at fault — a continuation token this service
/// did not issue, a registration status outside the modelled enum, a tag filter
/// type outside it, a tag filter list longer than the service scans — and the
/// listing still answers afterwards.
#[tokio::test]
async fn a_listing_the_service_will_not_accept_is_refused_with_its_modelled_code() {
    let emulator = seeded_fleet().await;
    // Rows differ along the member at fault and the FORM of the bad value: a
    // token that is not one this service renders, one that is well-formed but
    // names a record no listing carries, an empty one, a status of the right
    // spelling in the wrong case, and a tag filter type that is not a value of
    // `TagFilterType`.
    let cases: &[(&str, Value, &str)] = &[
        (
            "a token that is not one this service issues",
            json!({"nextToken": "not-a-token"}),
            "InvalidNextTokenException",
        ),
        (
            "a well-formed token naming a record the listing does not carry",
            json!({"nextToken": format!("p{}", hex::encode("ghost-instance"))}),
            "InvalidNextTokenException",
        ),
        (
            "a token whose payload is not hex",
            json!({"nextToken": "pzz"}),
            "InvalidNextTokenException",
        ),
        (
            "a registration status in the wrong case",
            json!({"registrationStatus": "registered"}),
            "InvalidRegistrationStatusException",
        ),
        (
            "a registration status outside the enum",
            json!({"registrationStatus": "Retired"}),
            "InvalidRegistrationStatusException",
        ),
        (
            "a tag filter type outside the enum",
            json!({"tagFilters": [filter(Some("env"), Some("prod"), Some("KEY"))]}),
            "InvalidTagFilterException",
        ),
        (
            "a filter list one past the bound this service scans",
            json!({"tagFilters": filters_of_length(
                service_com_amazonaws_codedeploy_codedeploy_20141006::state::MAX_TAG_FILTERS_PER_LIST + 1,
            )}),
            "InvalidTagFilterException",
        ),
    ];

    for (case, request, expected) in cases {
        let (status, body) = emulator
            .call("ListOnPremisesInstances", request.clone())
            .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, *expected),
            "case {case:?}: expected 400 {expected}, got {status} {body:?}"
        );
    }

    // A refusal left the store alone: the unrestricted listing still answers
    // every registered record.
    let answered = list_every_page(&emulator, "after the refusals", &json!({})).await;
    assert_eq!(
        answered.len(),
        4,
        "expected the four registered records after the refusals, got {answered:?}"
    );
}

/// RULE: the listing answers from the STORE, so a record appears when it is
/// registered and moves between the two registration statuses as the
/// registering and deregistering calls move it — a re-registration putting it
/// back under `Registered`.
#[tokio::test]
async fn the_listing_follows_the_registration_lifecycle() {
    let emulator = Emulator::new();
    let name = "lifecycle-1";
    let registered = json!({"registrationStatus": "Registered"});
    let deregistered = json!({"registrationStatus": "Deregistered"});

    // The steps of the lifecycle, in order: before the registration, after it,
    // after the deregistration, and after registering the same name again. Each
    // step asserts what BOTH statuses answer, and what the unrestricted listing
    // answers, so a record can neither hide from one nor be counted twice.
    let only = vec![name.to_string()];
    let nothing: Vec<String> = Vec::new();

    let case = "before any registration";
    let names = list_every_page(&emulator, case, &json!({})).await;
    assert_eq!(
        names, nothing,
        "case {case:?}: expected an empty listing, got {names:?}"
    );

    register(&emulator, name).await;
    let case = "after the registration";
    let names = list_every_page(&emulator, case, &registered).await;
    assert_eq!(
        names, only,
        "case {case:?}: expected the record under Registered, got {names:?}"
    );
    let names = list_every_page(&emulator, case, &deregistered).await;
    assert_eq!(
        names, nothing,
        "case {case:?}: expected nothing under Deregistered, got {names:?}"
    );

    emulator
        .ok(
            "DeregisterOnPremisesInstance",
            json!({"instanceName": name}),
        )
        .await;
    let case = "after the deregistration";
    let names = list_every_page(&emulator, case, &deregistered).await;
    assert_eq!(
        names, only,
        "case {case:?}: expected the record under Deregistered, got {names:?}"
    );
    let names = list_every_page(&emulator, case, &registered).await;
    assert_eq!(
        names, nothing,
        "case {case:?}: expected nothing under Registered, got {names:?}"
    );
    // The retired record is NOT part of the fleet the unrestricted listing
    // reports: deregistration ended its registration.
    let names = list_every_page(&emulator, case, &json!({})).await;
    assert_eq!(
        names, nothing,
        "case {case:?}: expected the retired record to be gone from the listing, got {names:?}"
    );

    register(&emulator, name).await;
    let case = "after the re-registration";
    let names = list_every_page(&emulator, case, &registered).await;
    assert_eq!(
        names, only,
        "case {case:?}: expected the record back under Registered, got {names:?}"
    );
    let names = list_every_page(&emulator, case, &deregistered).await;
    assert_eq!(
        names, nothing,
        "case {case:?}: expected nothing under Deregistered, got {names:?}"
    );
    // One record, listed once, however many times its registration moved.
    let names = list_every_page(&emulator, case, &json!({})).await;
    assert_eq!(
        names, only,
        "case {case:?}: expected exactly one record, got {names:?}"
    );
}
