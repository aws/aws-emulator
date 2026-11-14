// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the BODY of the not-found refusal class,
//! driven through the real `MultiServiceRouter` so every assertion goes over the
//! wire the SDK speaks.
//!
//! The rule under test: a `ResourceNotFoundException` (404) reports the kind of
//! resource that could not be resolved in `resourceType` as the service's
//! LOWERCASE token for that kind — the wire vocabulary a client branches on,
//! never a capitalised type name — and its `message` is the sentence the service
//! words for THAT refusal: the class, and the operation raising it where that
//! operation words the same class differently, with the only interpolated value
//! being the identifier the request carried. Refusals the service words no
//! sentence of its own for carry its generic sentence.
//!
//! The tables below are the class, not one recorded call: their rows differ
//! along the kind of resource that could not be resolved, the operation that
//! could not resolve it, and — for the sentence that quotes an identifier —
//! along the FORM and LENGTH of that identifier, including each published bound
//! of the modelled length and one past it.

use serde_json::{json, Value};

use crate::harness::{domain, field_of_type, fields_of_every_type, string_at, template, Emulator};

/// The service's generic not-found sentence, written out here rather than taken
/// from the crate so the expectations are independent of the code under test.
const GENERIC_SENTENCE: &str = "Requested resource not found.";

/// The sentence the association of a related item words its missing-case
/// refusal with: entirely fixed text, quoting no identifier.
const RELATED_ITEM_CASE_SENTENCE: &str = "A case with the specified ID was not found.";

/// The published bounds of the `domainId` path label: `@length 1..500`
/// (`smithy_query(query="operation",operation="CreateCase")`, also this crate's
/// `OPERATIONS.md`).
const DOMAIN_ID_MIN_LENGTH: usize = 1;
const DOMAIN_ID_MAX_LENGTH: usize = 500;

/// One row of the refusal table: what it is a case of, the method, the path, the
/// body, the lowercase token the refusal must report, the identifier it must
/// report, and the sentence it must carry.
type Row = (
    &'static str,
    &'static str,
    String,
    Option<Value>,
    &'static str,
    String,
    String,
);

/// What a refusal must report: the service's lowercase token for the kind, the
/// identifier of the resource that could not be resolved, and the sentence the
/// refusing operation words it with.
struct Refusal<'a> {
    resource_type: &'a str,
    resource_id: &'a str,
    sentence: &'a str,
}

/// Assert one call is the modelled 404 whose body reports `expected`.
async fn refused_as_not_found(
    emulator: &Emulator,
    case: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
    expected: &Refusal<'_>,
) {
    let Refusal {
        resource_type,
        resource_id,
        sentence,
    } = *expected;
    let (status, response) = emulator.send(method, path, body).await;
    assert_eq!(
        (status, response.get("__type").and_then(Value::as_str)),
        (404, Some("ResourceNotFoundException")),
        "case {case:?}: expected 404 ResourceNotFoundException, got {status} {response:?}"
    );
    let reported = string_at(&response, "resourceType", case);
    assert_eq!(
        reported, resource_type,
        "case {case:?}: expected resourceType {resource_type:?}, got {reported:?} in {response:?}"
    );
    // The invariant behind the token, asserted on whatever the row reports: the
    // vocabulary is lowercase, so no row can pass by naming a capitalised type.
    assert_eq!(
        reported,
        reported.to_lowercase(),
        "case {case:?}: resourceType must be the service's lowercase token, got {reported:?}"
    );
    assert_eq!(
        string_at(&response, "resourceId", case),
        resource_id,
        "case {case:?}: expected resourceId {resource_id:?}, got {response:?}"
    );
    assert_eq!(
        response.get("message").and_then(Value::as_str),
        Some(sentence),
        "case {case:?}: expected the sentence {sentence:?}, got {response:?}"
    );
}

#[tokio::test]
async fn every_not_found_refusal_reports_the_lowercase_token_and_its_own_sentence() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "refusal-wording-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "refusal-wording-template",
        std::slice::from_ref(&text),
    )
    .await;

    // Every kind of resource this service addresses, each through an operation
    // of its own: the kinds are the resources the model binds under a domain —
    // domain, field, template, layout and case (this crate's `OPERATIONS.md`
    // routes, `smithy_query(query="digest")`) — and the rows deliberately
    // include operations no recording of this refusal names (`DeleteField`,
    // `DeleteTemplate`, `DeleteLayout`, `GetCaseAuditEvents`, `ListLayouts`)
    // beside the ones that do.
    let missing = "11111111-2222-3333-4444-555555555555";
    let rows: Vec<Row> = vec![
        (
            "GetCase on a case the domain does not hold",
            "POST",
            format!("/domains/{domain_id}/cases/{missing}"),
            Some(json!({ "fields": [{ "id": text }] })),
            "case",
            missing.to_string(),
            format!("A case with id '{missing}' could not be found."),
        ),
        (
            "GetCaseAuditEvents on a case the domain does not hold",
            "POST",
            format!("/domains/{domain_id}/cases/{missing}/audit-history"),
            Some(json!({})),
            "case",
            missing.to_string(),
            format!("A case with id '{missing}' could not be found."),
        ),
        (
            "CreateRelatedItem for a case the domain does not hold",
            "POST",
            format!("/domains/{domain_id}/cases/{missing}/related-items/"),
            Some(json!({
                "type": "Comment",
                "content": { "comment": { "body": "a note", "contentType": "Text/Plain" } },
            })),
            "case",
            missing.to_string(),
            RELATED_ITEM_CASE_SENTENCE.to_string(),
        ),
        (
            "CreateCase in a domain that does not exist",
            "POST",
            "/domains/no-such-domain/cases".to_string(),
            Some(json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "x" } }],
            })),
            "domain",
            "no-such-domain".to_string(),
            "Domain 'no-such-domain' does not exist.".to_string(),
        ),
        (
            "ListLayouts in a domain that does not exist",
            "POST",
            "/domains/no-such-domain/layouts-list".to_string(),
            Some(json!({})),
            "domain",
            "no-such-domain".to_string(),
            GENERIC_SENTENCE.to_string(),
        ),
        (
            "UpdateLayout on a layout that does not exist",
            "PUT",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            Some(json!({ "name": "a-name" })),
            "layout",
            "no-such-layout".to_string(),
            GENERIC_SENTENCE.to_string(),
        ),
        (
            "DeleteLayout on a layout that does not exist",
            "DELETE",
            format!("/domains/{domain_id}/layouts/no-such-layout"),
            None,
            "layout",
            "no-such-layout".to_string(),
            GENERIC_SENTENCE.to_string(),
        ),
        (
            "DeleteTemplate on a template that does not exist",
            "DELETE",
            format!("/domains/{domain_id}/templates/no-such-template"),
            None,
            "template",
            "no-such-template".to_string(),
            GENERIC_SENTENCE.to_string(),
        ),
        (
            "DeleteField on a field that does not exist",
            "DELETE",
            format!("/domains/{domain_id}/fields/no-such-field"),
            None,
            "field",
            "no-such-field".to_string(),
            GENERIC_SENTENCE.to_string(),
        ),
    ];

    for (case, method, path, body, resource_type, resource_id, sentence) in rows {
        refused_as_not_found(
            &emulator,
            case,
            method,
            &path,
            body,
            &Refusal {
                resource_type,
                resource_id: &resource_id,
                sentence: &sentence,
            },
        )
        .await;
    }

    // The refusals wrote nothing: the template the rows named is still there.
    emulator
        .ok(
            "GetTemplate after the refusals",
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await;
}

#[tokio::test]
async fn the_create_case_domain_sentence_quotes_the_requested_domain_identifier() {
    let emulator = Emulator::new();
    // A live domain, so the rows below differ from a request that works only in
    // the identifier they carry.
    let domain_id = domain(&emulator, "create-case-domain-sentence").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "create-case-domain-template",
        std::slice::from_ref(&text),
    )
    .await;

    // The rows differ along the FORM and the LENGTH of the identifier that
    // resolves to nothing, including both published bounds of the modelled
    // `domainId` length.
    let identifiers: Vec<(&'static str, String)> = vec![
        (
            "a name-shaped identifier",
            "a-domain-that-was-never-created".to_string(),
        ),
        (
            "an identifier of the minted 8-4-4-4-12 form",
            "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string(),
        ),
        (
            "the shortest identifier the model allows",
            "d".repeat(DOMAIN_ID_MIN_LENGTH),
        ),
        (
            "the longest identifier the model allows",
            "d".repeat(DOMAIN_ID_MAX_LENGTH),
        ),
        (
            "an identifier that is of no minted form at all",
            "Not-A_Domain.Id~0".to_string(),
        ),
    ];

    for (identifier_case, missing_domain) in &identifiers {
        let case = format!("CreateCase with {identifier_case}");
        refused_as_not_found(
            &emulator,
            &case,
            "POST",
            &format!("/domains/{missing_domain}/cases"),
            Some(json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "x" } }],
            })),
            &Refusal {
                resource_type: "domain",
                resource_id: missing_domain,
                sentence: &format!("Domain '{missing_domain}' does not exist."),
            },
        )
        .await;
    }

    // One past the published bound is not this refusal at all: the request is
    // rejected for its length before any domain is resolved.
    let case = "CreateCase one character past the longest identifier the model allows";
    let too_long = "d".repeat(DOMAIN_ID_MAX_LENGTH + 1);
    let (status, code) = emulator
        .failure(
            case,
            "POST",
            &format!("/domains/{too_long}/cases"),
            Some(json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "x" } }],
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: an identifier past the modelled bound must be refused as invalid, \
         got {status} {code}"
    );

    // The identifier is the ONLY variable part: nothing else of the request
    // reaches the sentence.
    let case = "CreateCase in a domain that does not exist";
    let (_, response) = emulator
        .send(
            "POST",
            "/domains/another-domain-never-created/cases",
            Some(json!({
                "templateId": template_id,
                "fields": [{ "id": text, "value": { "stringValue": "x" } }],
            })),
        )
        .await;
    let message = string_at(&response, "message", case);
    for (member, value) in [("templateId", &template_id), ("fieldId", &text)] {
        assert!(
            !message.contains(value.as_str()),
            "case {case:?}: the sentence must carry no {member}, got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_case_refusal_is_worded_by_the_operation_that_raises_it() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-refusal-wording").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "case-refusal-wording-template",
        std::slice::from_ref(&text),
    )
    .await;
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
        "case \"DeleteCase to delete\": expected HTTP 200, got {status} {body:?}"
    );

    // The rows differ in the form of the id that resolves to nothing and in how
    // it came to resolve to nothing (deleted versus never held); each one is put
    // to BOTH operations, which word the same class differently.
    let ids: Vec<(&'static str, String)> = vec![
        ("the id of a case that was deleted", deleted_case_id),
        (
            "an id of the minted form the domain never held",
            "99999999-8888-7777-6666-555555555555".to_string(),
        ),
        ("a one-character id", "c".to_string()),
        (
            "an id that is of no minted form at all",
            "Not-A_Case.Id~0".to_string(),
        ),
    ];

    for (id_case, case_id) in &ids {
        let read = format!("GetCase with {id_case}");
        refused_as_not_found(
            &emulator,
            &read,
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({ "fields": [{ "id": text }] })),
            &Refusal {
                resource_type: "case",
                resource_id: case_id,
                sentence: &format!("A case with id '{case_id}' could not be found."),
            },
        )
        .await;

        let associate = format!("CreateRelatedItem with {id_case}");
        refused_as_not_found(
            &emulator,
            &associate,
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}/related-items/"),
            Some(json!({
                "type": "Comment",
                "content": { "comment": { "body": "a note", "contentType": "Text/Plain" } },
            })),
            &Refusal {
                resource_type: "case",
                resource_id: case_id,
                sentence: RELATED_ITEM_CASE_SENTENCE,
            },
        )
        .await;

        // The two sentences are genuinely different wordings of the same class:
        // the read's quotes the identifier, the association's does not.
        assert_ne!(
            RELATED_ITEM_CASE_SENTENCE,
            format!("A case with id '{case_id}' could not be found."),
            "case {associate:?}: the two operations must word the same class differently"
        );
    }
}
