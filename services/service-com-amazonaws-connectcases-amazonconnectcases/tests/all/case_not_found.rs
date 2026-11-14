// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the wording of ONE refusal class: a call that
//! addresses a case the domain does not hold, driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: a `ResourceNotFoundException` (404) about a CASE reports
//! the class through the lowercase token `case` and carries the sentence the
//! service words for THAT refusal — a call that addresses the case quotes the
//! case id the request carried (`A case with id '<caseId>' could not be found.`),
//! the association of a related item answers its own fixed sentence about the
//! same class — whether the case was deleted or never existed; the refusals the
//! service words no sentence of its own for keep answering its generic one, so
//! the text is chosen per refusal rather than from one template for every class.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::RESOURCE_NOT_FOUND_MESSAGE;

use crate::harness::{domain, field_of_type, fields_of_every_type, string_at, template, Emulator};

/// How a refusal about a case is worded, written out here rather than taken
/// from the crate so the expectations are independent of the code under test:
/// a function of the case id, which only one of the two sentences interpolates.
type Sentence = fn(&str) -> String;

/// The sentence a call that ADDRESSES a case must carry for one the domain does
/// not hold: it quotes the id the request carried.
fn addressed_case_sentence(case_id: &str) -> String {
    format!("A case with id '{case_id}' could not be found.")
}

/// The sentence `CreateRelatedItem` must carry for a case the domain does not
/// hold: entirely fixed text, quoting no id.
fn related_item_case_sentence(_case_id: &str) -> String {
    "A case with the specified ID was not found.".to_string()
}

/// Assert one call is the modelled 404 about the case `case_id` names, worded
/// as `sentence` words it.
async fn refused_as_unknown_case(
    emulator: &Emulator,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    case_id: &str,
    sentence: Sentence,
) {
    let (status, response) = emulator.send(method, path, body).await;
    assert_eq!(
        (status, response.get("__type").and_then(Value::as_str)),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: expected 404 ResourceNotFoundException, got {status} {response:?}"
    );
    let expected = sentence(case_id);
    assert_eq!(
        response.get("message").and_then(Value::as_str),
        Some(expected.as_str()),
        "case {case:?}: expected the sentence {expected:?}, got {response:?}"
    );
    assert_eq!(
        string_at(&response, "resourceType", case),
        "case",
        "case {case:?}: resourceType must name the kind of resource, got {response:?}"
    );
    assert_eq!(
        string_at(&response, "resourceId", case),
        case_id,
        "case {case:?}: resourceId must name the case the request addressed, got {response:?}"
    );
}

/// Every implemented operation that addresses a case by its `caseId` path
/// label: what it is called, its method, the path it takes after
/// `/domains/{domainId}/cases/{caseId}`, and a body that would otherwise be
/// accepted. `field_id` is a live field of the domain, so nothing in the body
/// is itself unresolvable and the refusal under test is the only one available.
///
/// Source: this crate's `OPERATIONS.md` routes — `GetCase`
/// (`POST /domains/{domainId}/cases/{caseId}`), `UpdateCase` (`PUT` on the same
/// path), `DeleteCase` (`DELETE`), `GetCaseAuditEvents` (`.../audit-history`)
/// and `CreateRelatedItem` (`.../related-items/`). The remaining case-addressed
/// routes (`UpdateRelatedItem`, `DeleteRelatedItem`, `SearchRelatedItems`) are
/// still scaffolded stubs and answer 501, so they carry no refusal to assert.
/// One row of a refusal table: what it is a case of (an operation name, or the
/// call in prose), the method, the path or the path suffix after the addressed
/// case, the request body, and the sentence that operation words the refusal
/// with.
type Route = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    Sentence,
);

fn case_addressed_operations(field_id: &str) -> Vec<Route> {
    vec![
        (
            "GetCase",
            "POST",
            String::new(),
            Some(json!({ "fields": [{ "id": field_id }] })),
            addressed_case_sentence as Sentence,
        ),
        (
            "UpdateCase",
            "PUT",
            String::new(),
            Some(json!({ "fields": [{ "id": field_id, "value": { "stringValue": "x" } }] })),
            addressed_case_sentence as Sentence,
        ),
        (
            "DeleteCase",
            "DELETE",
            String::new(),
            None,
            addressed_case_sentence as Sentence,
        ),
        (
            "GetCaseAuditEvents",
            "POST",
            "/audit-history".to_string(),
            Some(json!({})),
            addressed_case_sentence as Sentence,
        ),
        (
            "CreateRelatedItem",
            "POST",
            "/related-items/".to_string(),
            Some(json!({
                "type": "Comment",
                "content": { "comment": { "body": "a note", "contentType": "Text/Plain" } },
            })),
            related_item_case_sentence as Sentence,
        ),
    ]
}

#[tokio::test]
async fn every_operation_that_addresses_a_missing_case_answers_the_case_sentence() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-not-found-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "case-not-found-template",
        std::slice::from_ref(&text),
    )
    .await;

    // A case that was created and then deleted: the recorded sequence, and the
    // only row whose id the store ever minted.
    let created = emulator
        .ok(
            "CreateCase to delete",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "to-delete" } }],
            }),
        )
        .await;
    let deleted_case_id = string_at(&created, "caseId", "CreateCase to delete");
    let (status, body) = emulator
        .send(
            "DELETE",
            &format!("/domains/{domain_id}/cases/{deleted_case_id}"),
            None,
        )
        .await;
    assert_eq!(
        status, 200,
        "case \"DeleteCase\": expected HTTP 200, got {status} {body:?}"
    );

    // The rows differ in the FORM of the id that resolves to nothing and in how
    // it came to resolve to nothing (deleted versus never held). The model
    // bounds `caseId` at `len 1..500` (this crate's `OPERATIONS.md`), so the
    // shortest and the longest id a request may carry are both rows.
    let ids: Vec<(&'static str, String)> = vec![
        ("the id of a case that was deleted", deleted_case_id.clone()),
        (
            "an id of the minted 8-4-4-4-12 form the domain never held",
            "11111111-2222-3333-4444-555555555555".to_string(),
        ),
        (
            "a one-character id, the shortest the model allows",
            "c".to_string(),
        ),
        (
            "a 500-character id, the longest the model allows",
            "c".repeat(500),
        ),
        (
            "an id that is not of the minted form at all",
            "Not-A_Case.Id~0".to_string(),
        ),
    ];

    for (id_case, case_id) in &ids {
        for (operation, method, suffix, body, sentence) in case_addressed_operations(&text) {
            let case = format!("{operation} with {id_case}");
            let path = format!("/domains/{domain_id}/cases/{case_id}{suffix}");
            refused_as_unknown_case(&emulator, &case, method, &path, body, case_id, sentence).await;
        }
    }
}

#[tokio::test]
async fn the_case_sentence_is_about_the_case_the_request_names() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-sentence-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "case-sentence-template",
        std::slice::from_ref(&text),
    )
    .await;
    let created = emulator
        .ok(
            "CreateCase that survives",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "a note" } }],
            }),
        )
        .await;
    let live_case_id = string_at(&created, "caseId", "CreateCase that survives");

    // The refusal is driven by what the store holds, not by the shape of the
    // id: the live case answers 200 on the same route the rows above are
    // refused on.
    emulator
        .ok(
            "GetCase on the live case",
            "POST",
            &format!("/domains/{domain_id}/cases/{live_case_id}"),
            json!({ "fields": [{ "id": text }] }),
        )
        .await;

    // A reference a BODY carries is the case the sentence is about, not the
    // case the path addresses: `CreateRelatedItem` with `connectCase` content
    // links one case to another, and words either refusal the way it words its
    // missing-case refusal.
    let dangling = "99999999-8888-7777-6666-555555555555";
    refused_as_unknown_case(
        &emulator,
        "CreateRelatedItem linking to a case the domain does not hold",
        "POST",
        &format!("/domains/{domain_id}/cases/{live_case_id}/related-items/"),
        Some(json!({ "type": "ConnectCase", "content": { "connectCase": { "caseId": dangling } } })),
        dangling,
        related_item_case_sentence as Sentence,
    )
    .await;

    // The sentence carries the case id and nothing else of the request: no
    // domain id, no template id, no resource-type word beyond the one the
    // service's own wording uses.
    let missing = "44444444-3333-2222-1111-000000000000";
    let (_, response) = emulator
        .send(
            "POST",
            &format!("/domains/{domain_id}/cases/{missing}"),
            Some(json!({ "fields": [{ "id": text }] })),
        )
        .await;
    let message = string_at(&response, "message", "GetCase on a case never held");
    for (member, value) in [("domainId", &domain_id), ("templateId", &template_id)] {
        assert!(
            !message.contains(value.as_str()),
            "case \"GetCase on a case never held\": the sentence must carry no {member}, \
             got {message:?}"
        );
    }

    // The per-refusal choice: a refusal the service words no sentence of its
    // own for still answers its generic one, so this change did not retemplate
    // every refusal.
    let generic: Vec<(&'static str, &'static str, String, Option<Value>)> = vec![
        (
            "GetDomain on a domain that does not exist",
            "POST",
            "/domains/no-such-domain".to_string(),
            Some(json!({})),
        ),
        (
            "GetTemplate on a template that does not exist",
            "POST",
            format!("/domains/{domain_id}/templates/no-such-template"),
            Some(json!({})),
        ),
        (
            "GetLayout on a layout that does not exist",
            "POST",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            Some(json!({})),
        ),
        (
            "DeleteField on a field that does not exist",
            "DELETE",
            format!("/domains/{domain_id}/fields/no-such-field"),
            None,
        ),
    ];
    for (case, method, path, body) in generic {
        let (status, response) = emulator.send(method, &path, body).await;
        assert_eq!(
            (status, response.get("message").and_then(Value::as_str)),
            (404, Some(RESOURCE_NOT_FOUND_MESSAGE)),
            "case {case:?}: expected 404 with the generic sentence \
             {RESOURCE_NOT_FOUND_MESSAGE:?}, got {status} {response:?}"
        );
    }
}
