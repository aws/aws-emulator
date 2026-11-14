// SPDX-License-Identifier: CC0-1.0

//! What a READ of the agent space store answers, end to end over the wire:
//! `BatchGetAgentSpaces` against the records `CreateAgentSpace`, `UpdateAgentSpace` and
//! `DeleteAgentSpace` left behind.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots, as a
//! `restJson1` `POST /<OperationName>` with a JSON body signed for the `securityagent`
//! signing name; the harness is `crate::support`. So nothing in this file can pass
//! against a handler that answers a canned body: a value must come back out of the
//! shared store through a read before a case counts.
//!
//! The suites beside this one own the create and the patch paths. This one owns the
//! three rules a read of the store follows:
//!
//! * every requested id the store resolves is reported in EVERY view the response
//!   carries — `agentSpaces`, the recorded `agentInstances` view of the same records and
//!   the recorded `ids` — and every id it does not resolve in `notFound`;
//! * a resolved record reports its COLLECTIONS, so `targetDomainIds` and the resolved
//!   `targetDomains` are arrays on every agent space a read answers, empty for a record
//!   that names no target domain;
//! * a record `DeleteAgentSpace` removed is no longer resolved by a later read, so its
//!   id is answered in `notFound`.
//!
//! Each case asserts the rule over a TABLE of inputs: the rows differ along how many ids
//! are asked for, how many of them resolve, how often each is repeated, the size and the
//! resolvability of the association, and what mutation ran before the read.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member, ok};

/// The sentence a refusal of an agent space the account does not hold states: wholly
/// FIXED text, naming the resource class under the name the service's own records use
/// for an agent space and carrying nothing the request sent.
const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The four lists a `BatchGetAgentSpaces` 200 carries: the modelled partition pair
/// (`smithy_query(query="operation",operation="BatchGetAgentSpaces",
/// model_path="services/service-com-amazonaws-securityagent-securityagent/smithy-model/com-amazonaws-securityagent-securityagent.json")`)
/// and the recorded pair beside it.
const RESPONSE_LISTS: [&str; 4] = ["agentSpaces", "notFound", "agentInstances", "ids"];

/// The whole 200 payload of a `BatchGetAgentSpaces` over `ids`.
async fn batch_get(router: &MultiServiceRouter, ids: &[String], case: &str) -> Value {
    ok(
        router,
        "BatchGetAgentSpaces",
        &json!({"agentSpaceIds": ids}),
        case,
    )
    .await
}

/// A member that must be present AND a JSON array.
fn array(payload: &Value, name: &str, case: &str) -> Vec<Value> {
    payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: expected a {name} member, got {payload}"))
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected {name} to be an array, got {payload}"))
        .clone()
}

/// A list of strings that must be present.
fn strings(payload: &Value, name: &str, case: &str) -> Vec<String> {
    array(payload, name, case)
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: a {name} entry is not a string: {entry}"))
                .to_string()
        })
        .collect()
}

/// The `agentSpaceId` of every entry of a list of agent space records.
fn ids_of(records: &[Value], case: &str) -> Vec<String> {
    records
        .iter()
        .map(|record| {
            member(record, "agentSpaceId")
                .unwrap_or_else(|| panic!("case {case:?}: a record carries no agentSpaceId: {record}"))
        })
        .collect()
}

/// `CreateAgentSpace`, as the id of the record it minted.
async fn create(router: &MultiServiceRouter, request: &Value, case: &str) -> String {
    let created = ok(router, "CreateAgentSpace", request, case).await;
    member(&created, "agentSpaceId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateAgentSpace returned no agentSpaceId"))
}

/// `CreateTargetDomain`, as the id of the record it minted.
async fn create_target_domain(router: &MultiServiceRouter, domain: &str, case: &str) -> String {
    let created = ok(
        router,
        "CreateTargetDomain",
        &json!({"targetDomainName": domain, "verificationMethod": "DNS_TXT"}),
        case,
    )
    .await;
    member(&created, "targetDomainId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateTargetDomain returned no targetDomainId"))
}

/// RULE: a read of the store answers every requested id in exactly one place — an id it
/// resolves in each of the response's resolved views (`agentSpaces`, the recorded
/// `agentInstances` view of the same records, and the recorded `ids`), every other id in
/// `notFound` — once per DISTINCT id and in the order it was asked for, with all four
/// lists present on every answer.
#[tokio::test]
async fn a_read_reports_every_resolved_record_in_every_view() {
    let router = emulator();

    let mut stored: BTreeMap<String, String> = BTreeMap::new();
    for name in ["alpha", "beta", "gamma"] {
        let id = create(&router, &json!({"name": name}), name).await;
        stored.insert(id, name.to_string());
    }
    let known: Vec<String> = stored.keys().cloned().collect();
    // Ids no create minted: 32 hex `f`s (an opaque value of no resource class's id
    // form), a readable label, a name in another script.
    let unknown: Vec<String> = vec![
        "f".repeat(32),
        "no-such-agent-space".to_string(),
        "неизвестно".to_string(),
    ];

    // The rows differ along the number of ids asked for, how many resolve, their order
    // and how often each is repeated.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("no ids at all", vec![]),
        ("one known id", vec![known[0].clone()]),
        ("one unknown id", vec![unknown[0].clone()]),
        ("every known id", known.clone()),
        ("every unknown id", unknown.clone()),
        (
            "known and unknown interleaved",
            vec![
                unknown[0].clone(),
                known[2].clone(),
                unknown[1].clone(),
                known[0].clone(),
            ],
        ),
        (
            "known ids in reverse order",
            known.iter().rev().cloned().collect(),
        ),
        (
            "a known id asked for three times",
            vec![known[1].clone(), known[1].clone(), known[1].clone()],
        ),
        (
            "an unknown id asked for twice",
            vec![unknown[2].clone(), unknown[2].clone()],
        ),
    ];

    for (case, requested) in rows {
        let payload = batch_get(&router, &requested, case).await;

        // Every list is present, whatever fell on which side.
        for name in RESPONSE_LISTS {
            assert!(
                payload.get(name).is_some_and(Value::is_array),
                "case {case:?}: expected {name} as an array on every answer, got {payload}"
            );
        }

        // Exactly one answer per distinct requested id, on the right side, in the order
        // it was asked for.
        let mut expected_found: Vec<String> = Vec::new();
        let mut expected_missing: Vec<String> = Vec::new();
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        for id in &requested {
            if !seen.insert(id) {
                continue;
            }
            if stored.contains_key(id) {
                expected_found.push(id.clone());
            } else {
                expected_missing.push(id.clone());
            }
        }

        let spaces = array(&payload, "agentSpaces", case);
        let instances = array(&payload, "agentInstances", case);
        assert_eq!(
            ids_of(&spaces, case),
            expected_found,
            "case {case:?}: expected agentSpaces {expected_found:?}, got {payload}"
        );
        assert_eq!(
            strings(&payload, "notFound", case),
            expected_missing,
            "case {case:?}: expected notFound {expected_missing:?}, got {payload}"
        );
        // The recorded views report the SAME resolved records and their ids: an id in
        // `notFound` contributes to neither.
        assert_eq!(
            instances, spaces,
            "case {case:?}: expected agentInstances to report the same resolved records \
             as agentSpaces {spaces:?}, got {instances:?}"
        );
        assert_eq!(
            strings(&payload, "ids", case),
            expected_found,
            "case {case:?}: expected ids {expected_found:?}, got {payload}"
        );

        // Each record answered is the one stored under that id, not another and not a
        // placeholder — in both views.
        for record in spaces.iter().chain(instances.iter()) {
            let id = member(record, "agentSpaceId").expect("an id, asserted above");
            assert_eq!(
                member(record, "name").as_deref(),
                stored.get(&id).map(String::as_str),
                "case {case:?}: expected the stored name for {id:?}, got {record}"
            );
        }
    }
}

/// RULE: a resolved record reports its COLLECTIONS, so `targetDomainIds` and the
/// resolved `targetDomains` beside it are arrays on every agent space a read answers —
/// the stored ids and the stored record of each id that resolves, and EMPTY arrays for a
/// record that names no target domain at all.
#[tokio::test]
async fn a_read_reports_the_record_collections_as_arrays() {
    let router = emulator();

    let registered = ["first.example.com", "second.example.com", "third.example.com"];
    let mut domain_ids: Vec<String> = Vec::new();
    for domain in registered {
        domain_ids.push(create_target_domain(&router, domain, "seed").await);
    }
    let unresolvable = "f".repeat(32);

    // The rows differ along whether the member is given at all, the size of the list,
    // which entries resolve to a registered domain and how often each is repeated.
    let rows: Vec<(&str, Option<Vec<String>>)> = vec![
        ("no targetDomainIds at all", None),
        ("an empty association", Some(Vec::new())),
        ("one registered domain", Some(vec![domain_ids[0].clone()])),
        (
            "every registered domain",
            Some(domain_ids.clone()),
        ),
        (
            "every registered domain, reversed",
            Some(domain_ids.iter().rev().cloned().collect()),
        ),
        (
            "one id that resolves to no registered domain",
            Some(vec![unresolvable.clone()]),
        ),
        (
            "registered and unresolvable interleaved, with repeats",
            Some(vec![
                unresolvable.clone(),
                domain_ids[1].clone(),
                domain_ids[1].clone(),
                unresolvable.clone(),
                domain_ids[2].clone(),
            ]),
        ),
    ];

    for (case, association) in rows {
        let mut request = json!({"name": format!("collections-{case}")});
        if let Some(ids) = association.as_ref() {
            request["targetDomainIds"] = json!(ids);
        }
        let id = create(&router, &request, case).await;

        let payload = batch_get(&router, std::slice::from_ref(&id), case).await;
        let spaces = array(&payload, "agentSpaces", case);
        let record = spaces
            .first()
            .unwrap_or_else(|| panic!("case {case:?}: the agent space must read back: {payload}"));

        // The stored ids, verbatim — the empty array for a record that names none.
        let stored_ids = association.clone().unwrap_or_default();
        assert_eq!(
            strings(record, "targetDomainIds", case),
            stored_ids,
            "case {case:?}: expected targetDomainIds {stored_ids:?} as an array, got {record}"
        );
        // And the resolved records: one per DISTINCT id a registration minted, in the
        // order first named, so an unresolvable id contributes no entry.
        let mut seen: BTreeSet<&String> = BTreeSet::new();
        let expected_domains: Vec<String> = stored_ids
            .iter()
            .filter(|domain_id| seen.insert(domain_id))
            .filter(|domain_id| domain_ids.contains(domain_id))
            .cloned()
            .collect();
        let resolved: Vec<String> = array(record, "targetDomains", case)
            .iter()
            .map(|domain| {
                member(domain, "targetDomainId")
                    .unwrap_or_else(|| panic!("case {case:?}: an entry carries no id: {domain}"))
            })
            .collect();
        assert_eq!(
            resolved, expected_domains,
            "case {case:?}: expected targetDomains {expected_domains:?} as an array, got \
             {record}"
        );

        // The recorded agent instance view of the same record reports the same two
        // collections.
        let instances = array(&payload, "agentInstances", case);
        assert_eq!(
            instances.first(),
            Some(record),
            "case {case:?}: expected the agent instance view of the same record, got \
             {payload}"
        );
    }
}

/// RULE: `DeleteAgentSpace` removes the record it names, so a later read no longer
/// resolves that id and answers it in `notFound` instead of serving the deleted record;
/// the answer names the id removed, every other record is left alone, and an id the
/// store holds no record for — including one a delete already removed — is refused with
/// the service's `ResourceNotFoundException` (HTTP 404).
#[tokio::test]
async fn a_deleted_agent_space_is_no_longer_resolved_by_a_read() {
    let router = emulator();

    // Several records, so a delete that removed more than the one named would show.
    let mut remaining: Vec<String> = Vec::new();
    for index in 0..5 {
        remaining.push(
            create(
                &router,
                &json!({
                    "name": format!("space-{index}"),
                    "description": format!("the {index}th space"),
                    "codeReviewSettings": {
                        "controlsScanning": index % 2 == 0,
                        "generalPurposeScanning": index % 3 == 0,
                    },
                }),
                "seed",
            )
            .await,
        );
    }
    let all: Vec<String> = remaining.clone();

    // The rows differ along WHICH of the stored records is deleted: the first, the last,
    // and one in the middle.
    let rows: Vec<(&str, usize)> = vec![
        ("the first stored record", 0),
        ("the last stored record", 3),
        ("a record in the middle", 1),
    ];

    for (case, index) in rows {
        let deleted = remaining.remove(index);
        let answer = call(
            &router,
            "DeleteAgentSpace",
            &json!({"agentSpaceId": deleted}),
        )
        .await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: expected DeleteAgentSpace to answer 200, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            member(&answer.body, "agentSpaceId").as_deref(),
            Some(deleted.as_str()),
            "case {case:?}: expected the delete to name the record it removed, got {}",
            answer.body
        );
        // Recorded, not modelled: the same value under the name the service's own
        // record uses for it, as every other agent space response carries it.
        assert_eq!(
            member(&answer.body, "id").as_deref(),
            Some(deleted.as_str()),
            "case {case:?}: expected the delete to report the removed record's id, got {}",
            answer.body
        );

        // The read no longer resolves it, in any of the response's views, and still
        // resolves every record that was not deleted.
        let payload = batch_get(&router, &all, case).await;
        let found = ids_of(&array(&payload, "agentSpaces", case), case);
        assert_eq!(
            found, remaining,
            "case {case:?}: expected only the records still stored {remaining:?}, got \
             {found:?}"
        );
        assert_eq!(
            ids_of(&array(&payload, "agentInstances", case), case),
            remaining,
            "case {case:?}: expected the agent instance view to drop the deleted record \
             too, got {payload}"
        );
        assert_eq!(
            strings(&payload, "ids", case),
            remaining,
            "case {case:?}: expected ids to drop the deleted record too, got {payload}"
        );
        let missing = strings(&payload, "notFound", case);
        assert!(
            missing.contains(&deleted),
            "case {case:?}: expected the deleted id {deleted:?} in notFound, got \
             {missing:?}"
        );

        // A second delete of the same id resolves nothing: the record is gone, so it is
        // refused rather than answered as a delete that happened.
        let again = call(
            &router,
            "DeleteAgentSpace",
            &json!({"agentSpaceId": deleted}),
        )
        .await;
        assert_eq!(
            again.status, 404,
            "case {case:?}: expected a second delete to be refused with 404, got {} ({}) \
             with {}",
            again.status, again.error_code, again.body
        );
        assert_eq!(
            again.error_code, "ResourceNotFoundException",
            "case {case:?}: expected the ResourceNotFoundException error code, got {:?} \
             with {}",
            again.error_code, again.body
        );
        assert_eq!(
            member(&again.body, "message").as_deref(),
            Some(AGENT_SPACE_NOT_FOUND),
            "case {case:?}: expected the service's fixed not-found sentence, got {}",
            again.body
        );
    }

    // Ids no create minted, and the empty id, which names no record and never could.
    let refusals: Vec<(&str, String, u16, &str)> = vec![
        (
            "32 hex f's, a value the id counter cannot reach",
            "f".repeat(32),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a readable label",
            "no-such-agent-space".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "an id in another script",
            "неизвестно".to_string(),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a stored id with a suffix",
            format!("{}-suffix", remaining[0]),
            404,
            "ResourceNotFoundException",
        ),
        ("the empty id", String::new(), 400, "ValidationException"),
    ];

    let before = batch_get(&router, &all, "before the refused deletes").await;
    for (case, id, status, code) in refusals {
        let answer = call(&router, "DeleteAgentSpace", &json!({"agentSpaceId": id})).await;
        assert_eq!(
            answer.status, status,
            "case {case:?}: expected a {status} refusal, got {} ({}) with {}",
            answer.status, answer.error_code, answer.body
        );
        assert_eq!(
            answer.error_code, code,
            "case {case:?}: expected the {code} error code, got {:?} with {}",
            answer.error_code, answer.body
        );
    }
    // A refused delete removed nothing: the read answers exactly what it did before.
    let after = batch_get(&router, &all, "after the refused deletes").await;
    assert_eq!(
        after, before,
        "expected the refused deletes to leave the store as it was, got {after}"
    );
}

/// RULE: a read reports the record as it now stands, so the members a preceding
/// `UpdateAgentSpace` wrote are the ones it answers — in every view of the response —
/// and the members the patch did not carry are unchanged.
#[tokio::test]
async fn a_read_reports_the_record_as_the_last_write_left_it() {
    let router = emulator();

    let domain_id = create_target_domain(&router, "update.example.com", "seed").await;

    // The rows differ along which members the patch carries and the form of each value.
    let rows: Vec<(&str, Value)> = vec![
        ("the description alone", json!({"description": "after the update"})),
        (
            "the code review settings alone",
            json!({"codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": true}}),
        ),
        (
            "the association alone, naming a registered domain",
            json!({"targetDomainIds": [domain_id.clone()]}),
        ),
        ("the association alone, emptied", json!({"targetDomainIds": []})),
        ("the name alone", json!({"name": "renamed"})),
        (
            "every member the patch can carry",
            json!({
                "name": "fully-updated",
                "description": "",
                "awsResources": {"s3Buckets": ["arn:aws:s3:::sec-agent-read"]},
                "targetDomainIds": [domain_id.clone()],
                "codeReviewSettings": {"controlsScanning": true, "generalPurposeScanning": true},
            }),
        ),
    ];

    for (case, patch) in rows {
        let id = create(
            &router,
            &json!({
                "name": "before-the-update",
                "description": "the agent space as it was created",
                "codeReviewSettings": {"controlsScanning": false, "generalPurposeScanning": false},
            }),
            case,
        )
        .await;

        let mut request = patch.clone();
        request["agentSpaceId"] = json!(id);
        ok(&router, "UpdateAgentSpace", &request, case).await;

        let payload = batch_get(&router, std::slice::from_ref(&id), case).await;
        for view in ["agentSpaces", "agentInstances"] {
            let records = array(&payload, view, case);
            let record = records.first().unwrap_or_else(|| {
                panic!("case {case:?}: expected the record in {view}: {payload}")
            });
            for (name, written) in patch.as_object().expect("the patch is an object") {
                assert_eq!(
                    record.get(name),
                    Some(written),
                    "case {case:?}: expected {view} to report the written {name} \
                     {written:?}, got {record}"
                );
            }
            // A member the patch did not carry is untouched.
            if patch.get("name").is_none() {
                assert_eq!(
                    member(record, "name").as_deref(),
                    Some("before-the-update"),
                    "case {case:?}: expected {view} to leave the name as created, got \
                     {record}"
                );
            }
        }
    }
}
