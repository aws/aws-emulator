// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the BATCH operations over several cases of
//! one domain, driven through the real `MultiServiceRouter` so every assertion
//! goes over the wire the SDK speaks.
//!
//! The rule under test: `BatchUpdateCase` and `BatchCreateRelatedItem` process
//! each case of their `cases` list INDEPENDENTLY — an entry that fails is
//! reported in the answer's own failure list while every other entry is still
//! applied — and refuse the WHOLE request, writing nothing at all, for the
//! request-level mistakes the service states a rule for. A retry under a
//! client token already answered is answered with that first answer rather than
//! applied a second time, and the same token with different parameters is a
//! conflict.
//!
//! The tables below are the class, not one recorded call: their rows differ in
//! how an entry addresses its case (by id, by reference id, by an id the domain
//! does not hold), in how a field is identified (by id, by name), in which
//! request-level rule is broken, and in which content type a batch association
//! carries.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    BATCH_CASE_NOT_FOUND_CODE, BATCH_CASE_NOT_FOUND_MESSAGE,
};

use crate::harness::{
    domain, field_of_type, fields_of_every_type, is_minted_id, string_at, template, title_value,
    Emulator,
};

/// The `BatchUpdateCase` route: an operation over cases of one domain, whose
/// own URI names no domain (`PUT /cases:batchUpdateCase`).
const BATCH_UPDATE_PATH: &str = "/cases:batchUpdateCase";

/// The `BatchCreateRelatedItem` route (`POST /cases:batchCreateRelatedItem`).
const BATCH_CREATE_RELATED_ITEM_PATH: &str = "/cases:batchCreateRelatedItem";

/// A customer-profile ARN, as the top-level `customerId` of a batch update
/// carries one.
const CUSTOMER_ARN: &str = "arn:aws:profile:us-east-1:000000000000:domains/d-1/profiles/p-1";

/// A case id of the minted form that no domain of this suite holds.
const UNHELD_CASE_ID: &str = "11111111-2222-3333-4444-555555555555";

/// The reference id one case of the fixture carries, which an entry addresses
/// it by instead of by its own id.
const REFERENCE_ID: &str = "REF-0001";

/// The resources a batch is applied to: one domain, two cases (one of them
/// carrying a reference id) and the fields a batch writes.
struct Fixture {
    domain_id: String,
    template_id: String,
    first_case: String,
    second_case: String,
    text_field: String,
    number_field: String,
    number_field_name: String,
}

/// One field value entry of a case request.
fn field(id: &str, value: Value) -> Value {
    json!({ "id": id, "value": value })
}

/// A string-valued field entry.
fn text(id: &str, value: &str) -> Value {
    field(id, json!({ "stringValue": value }))
}

/// One `cases` entry addressing a case by its own id.
fn by_case_id(case_id: &str) -> Value {
    json!({ "caseIdentifier": { "caseId": case_id } })
}

/// One `cases` entry addressing a case by the reference id it holds.
fn by_reference_id(reference_id: &str) -> Value {
    json!({ "caseIdentifier": { "referenceId": reference_id } })
}

/// One `fields` entry of a batch update, identifying its field by id.
fn write_by_id(field_id: &str, value: Value) -> Value {
    json!({ "fieldIdentifier": { "id": field_id }, "value": value })
}

/// One `fields` entry of a batch update, identifying its field by name.
fn write_by_name(name: &str, value: Value) -> Value {
    json!({ "fieldIdentifier": { "name": name }, "value": value })
}

/// Create a case carrying `fields` beside the `title` every create must
/// supply, and return the id the service minted.
async fn case_with(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    template_id: &str,
    fields: Vec<Value>,
) -> String {
    let mut body = vec![title_value(label)];
    body.extend(fields);
    let created = emulator
        .ok(
            label,
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": body }),
        )
        .await;
    string_at(&created, "caseId", label)
}

async fn fixture(emulator: &Emulator, name: &str) -> Fixture {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");
    let number_field = field_of_type(&fields, "Number");
    let template_id = template(emulator, &domain_id, &format!("{name}-template"), &[]).await;
    let first_case = case_with(
        emulator,
        "the first case",
        &domain_id,
        &template_id,
        vec![text(&text_field, "what the first case held")],
    )
    .await;
    let second_case = case_with(
        emulator,
        "the second case",
        &domain_id,
        &template_id,
        vec![
            text(&text_field, "what the second case held"),
            text("reference_id", REFERENCE_ID),
        ],
    )
    .await;
    Fixture {
        domain_id,
        template_id,
        first_case,
        second_case,
        text_field,
        number_field,
        // `fields_of_every_type` names each field after the type it declares,
        // which is the name a `fields` entry can address it by.
        number_field_name: "field-Number".to_string(),
    }
}

/// The value `GetCase` reports for one field of one case, or `None` when the
/// answer carries none for it.
async fn read_field(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    case_id: &str,
    field_id: &str,
) -> Option<Value> {
    let answer = emulator
        .ok(
            label,
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({ "fields": [{ "id": field_id }] }),
        )
        .await;
    answer
        .get("fields")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no fields list in {answer:?}"))
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(field_id))
        .and_then(|entry| entry.get("value").cloned())
}

/// Every audit event one case has recorded, as `type` strings.
async fn audit_event_types(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    case_id: &str,
) -> Vec<String> {
    let answer = emulator
        .ok(
            label,
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}/audit-history"),
            json!({}),
        )
        .await;
    answer
        .get("auditEvents")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no auditEvents list in {answer:?}"))
        .iter()
        .map(|event| string_at(event, "type", label))
        .collect()
}

/// Every related item of one domain, as `(caseId, content)` pairs.
async fn related_items(emulator: &Emulator, label: &str, domain_id: &str) -> Vec<(String, Value)> {
    let answer = emulator
        .ok(
            label,
            "POST",
            &format!("/domains/{domain_id}/related-items-search"),
            json!({}),
        )
        .await;
    answer
        .get("relatedItems")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no relatedItems list in {answer:?}"))
        .iter()
        .map(|item| {
            (
                string_at(item, "caseId", label),
                item.get("content").cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// The entries of one list member of a batch answer.
fn entries(answer: &Value, member: &str, label: &str) -> Vec<Value> {
    answer
        .get(member)
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {label:?}: no {member} list in {answer:?}"))
        .clone()
}

/// The `caseIdentifier.caseId` of one entry.
fn entry_case_id(entry: &Value, label: &str) -> String {
    let identifier = entry
        .get("caseIdentifier")
        .unwrap_or_else(|| panic!("case {label:?}: no caseIdentifier in {entry:?}"));
    string_at(identifier, "caseId", label)
}

#[tokio::test]
async fn a_batch_update_writes_every_case_it_resolves_and_fails_only_the_entry_it_cannot() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "batch-update-domain").await;
    let domain_id = fixture.domain_id.clone();

    // The entries differ in how each addresses its case: by its own id, by the
    // reference id it holds, and by an id the domain does not hold. The fields
    // differ in how each is identified: by id and by name.
    let answer = emulator
        .ok(
            "BatchUpdateCase",
            "PUT",
            BATCH_UPDATE_PATH,
            json!({
                "domainId": domain_id,
                "cases": [
                    by_case_id(&fixture.first_case),
                    by_reference_id(REFERENCE_ID),
                    by_case_id(UNHELD_CASE_ID),
                ],
                "fields": [
                    write_by_id(&fixture.text_field, json!({ "stringValue": "written by the batch" })),
                    write_by_name(&fixture.number_field_name, json!({ "doubleValue": 12.5 })),
                ],
                "title": "a batched title",
                "status": "Open",
                "customerId": CUSTOMER_ARN,
            }),
        )
        .await;

    let updated = entries(&answer, "successfulCases", "BatchUpdateCase");
    assert_eq!(
        updated
            .iter()
            .map(|entry| entry_case_id(entry, "BatchUpdateCase"))
            .collect::<Vec<String>>(),
        vec![fixture.first_case.clone(), fixture.second_case.clone()],
        "case \"BatchUpdateCase\": every entry the request resolved is reported as updated, in \
         the order it addressed them, got {updated:?}"
    );
    // The identity an entry reports is the case's own, so an entry addressed by
    // reference id reports the id of the case it resolved to AND the reference
    // the case holds.
    assert_eq!(
        updated[1].get("caseIdentifier"),
        Some(&json!({ "caseId": fixture.second_case, "referenceId": REFERENCE_ID })),
        "case \"BatchUpdateCase\": an entry reports the identity of the case it resolved, got \
         {:?}",
        updated[1]
    );
    let failed = entries(&answer, "failedCases", "BatchUpdateCase");
    assert_eq!(
        failed,
        vec![json!({
            "caseIdentifier": { "caseId": UNHELD_CASE_ID },
            "errorCode": BATCH_CASE_NOT_FOUND_CODE,
            "errorMessage": BATCH_CASE_NOT_FOUND_MESSAGE,
        })],
        "case \"BatchUpdateCase\": an entry naming a case the domain does not hold fails on its \
         own, got {failed:?}"
    );

    // Each resolved case holds every value the one request wrote, read back
    // through `GetCase`.
    for (label, case_id) in [
        ("the first case", &fixture.first_case),
        ("the second case", &fixture.second_case),
    ] {
        let written: Vec<(&str, Value)> = vec![
            (
                fixture.text_field.as_str(),
                json!({ "stringValue": "written by the batch" }),
            ),
            (
                fixture.number_field.as_str(),
                json!({ "doubleValue": 12.5 }),
            ),
            ("title", json!({ "stringValue": "a batched title" })),
            ("status", json!({ "stringValue": "Open" })),
            ("customer_id", json!({ "stringValue": CUSTOMER_ARN })),
        ];
        for (field_id, value) in written {
            assert_eq!(
                read_field(&emulator, label, &domain_id, case_id, field_id).await,
                Some(value.clone()),
                "case {label:?}: the batch wrote {field_id} as {value:?}"
            );
        }
        assert_eq!(
            audit_event_types(&emulator, label, &domain_id, case_id).await,
            vec!["Case.Updated".to_string(), "Case.Created".to_string()],
            "case {label:?}: a batched write is recorded in the case's audit history exactly \
             once, most recent first"
        );
    }
}

/// One row of the refusal table: how a failing assertion names it, the answer
/// the service owes it, and the body it owes that answer to.
struct Refusal {
    label: &'static str,
    status: u16,
    code: &'static str,
    body: Value,
}

#[tokio::test]
async fn a_batch_update_that_breaks_a_request_level_rule_writes_nothing_at_all() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "batch-update-refusal-domain").await;
    let domain_id = fixture.domain_id.clone();
    let cases = json!([by_case_id(&fixture.first_case)]);

    // The rows are the rules `BatchUpdateCase` states for a request that fails
    // as a whole: a domain nothing holds, a reference id that resolves to
    // nothing, one case addressed twice, a request asking for no update, a
    // `fields` entry naming a value the request sets through its own
    // parameter, a field name carrying one of the two characters the service
    // reserves, a name and an id that resolve to no field, and a value whose
    // type the field does not declare.
    let rows: Vec<Refusal> = vec![
        Refusal {
            label: "a domain nothing holds",
            status: 404,
            code: "ResourceNotFoundException",
            body: json!({
                "domainId": "a-domain-nothing-holds",
                "cases": cases,
                "fields": [write_by_id(&fixture.text_field, json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "a reference id that resolves to nothing",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": [by_reference_id("REF-NOTHING")],
                "fields": [write_by_id(&fixture.text_field, json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "one case addressed twice",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": [by_case_id(&fixture.first_case), by_case_id(&fixture.first_case)],
                "fields": [write_by_id(&fixture.text_field, json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "one case addressed by its id and by its reference id",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": [by_case_id(&fixture.second_case), by_reference_id(REFERENCE_ID)],
                "fields": [write_by_id(&fixture.text_field, json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "a request that asks for no update",
            status: 400,
            code: "ValidationException",
            body: json!({ "domainId": domain_id, "cases": cases, "fields": [] }),
        },
        Refusal {
            label: "a fields entry naming the title",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_id("title", json!({ "stringValue": "a title in fields" }))],
            }),
        },
        Refusal {
            label: "a fields entry naming the status",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_id("status", json!({ "stringValue": "Open" }))],
            }),
        },
        Refusal {
            label: "a fields entry naming the customer",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_id("customer_id", json!({ "stringValue": CUSTOMER_ARN }))],
            }),
        },
        Refusal {
            label: "a field name carrying a pipe",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_name("field|Number", json!({ "doubleValue": 1.0 }))],
            }),
        },
        Refusal {
            label: "a field name carrying a comma",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_name("field,Number", json!({ "doubleValue": 1.0 }))],
            }),
        },
        Refusal {
            label: "a field name no field of the domain carries",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_name("a name no field carries", json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "a field id the domain holds no field for",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_id("a-field-the-domain-lacks", json!({ "stringValue": "x" }))],
            }),
        },
        Refusal {
            label: "a value of a type the field does not declare",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "cases": cases,
                "fields": [write_by_id(&fixture.text_field, json!({ "doubleValue": 1.0 }))],
            }),
        },
    ];

    for row in &rows {
        let (status, code) = emulator
            .failure(row.label, "PUT", BATCH_UPDATE_PATH, Some(row.body.clone()))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (row.status, row.code),
            "case {:?}: expected {} {}, got {status} {code}",
            row.label,
            row.status,
            row.code
        );
    }

    // "The entire request fails, and no cases are updated": every case still
    // holds exactly what it held before the table ran.
    for (label, case_id, held) in [
        (
            "the first case after the refusals",
            &fixture.first_case,
            "what the first case held",
        ),
        (
            "the second case after the refusals",
            &fixture.second_case,
            "what the second case held",
        ),
    ] {
        assert_eq!(
            read_field(&emulator, label, &domain_id, case_id, &fixture.text_field).await,
            Some(json!({ "stringValue": held })),
            "case {label:?}: a refused batch writes nothing"
        );
    }
}

#[tokio::test]
async fn a_batch_update_retried_under_one_client_token_is_answered_once() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "batch-update-idempotency-domain").await;
    let domain_id = fixture.domain_id.clone();
    let token = "a-client-token-for-one-batch";
    let body = json!({
        "domainId": domain_id,
        "cases": [by_case_id(&fixture.first_case)],
        "fields": [write_by_id(&fixture.text_field, json!({ "stringValue": "written once" }))],
        "clientToken": token,
    });

    let first = emulator
        .ok("the first call", "PUT", BATCH_UPDATE_PATH, body.clone())
        .await;
    let retried = emulator
        .ok("the retry", "PUT", BATCH_UPDATE_PATH, body.clone())
        .await;
    assert_eq!(
        retried, first,
        "case \"the retry\": a retry under the same client token is answered with the first \
         answer, got {retried:?}"
    );
    // The retry was ANSWERED, not applied: the case records the one write.
    assert_eq!(
        audit_event_types(&emulator, "the retry", &domain_id, &fixture.first_case)
            .await
            .iter()
            .filter(|event| *event == "Case.Updated")
            .count(),
        1,
        "case \"the retry\": a retry under the same client token is not applied a second time"
    );

    // The same token with different parameters is a conflict, and changes
    // nothing.
    let (status, code) = emulator
        .failure(
            "the same token with other parameters",
            "PUT",
            BATCH_UPDATE_PATH,
            Some(json!({
                "domainId": domain_id,
                "cases": [by_case_id(&fixture.first_case)],
                "fields": [write_by_id(
                    &fixture.text_field,
                    json!({ "stringValue": "written by another request" }),
                )],
                "clientToken": token,
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case \"the same token with other parameters\": reusing a client token for different \
         parameters is a conflict, got {status} {code}"
    );
    assert_eq!(
        read_field(
            &emulator,
            "the case after the reuse",
            &domain_id,
            &fixture.first_case,
            &fixture.text_field
        )
        .await,
        Some(json!({ "stringValue": "written once" })),
        "case \"the case after the reuse\": a refused reuse writes nothing"
    );

    // A DIFFERENT token is a different request, so it is applied.
    emulator
        .ok(
            "another token",
            "PUT",
            BATCH_UPDATE_PATH,
            json!({
                "domainId": domain_id,
                "cases": [by_case_id(&fixture.first_case)],
                "fields": [write_by_id(
                    &fixture.text_field,
                    json!({ "stringValue": "written by another request" }),
                )],
                "clientToken": "another-client-token",
            }),
        )
        .await;
    assert_eq!(
        read_field(
            &emulator,
            "the case after another token",
            &domain_id,
            &fixture.first_case,
            &fixture.text_field
        )
        .await,
        Some(json!({ "stringValue": "written by another request" })),
        "case \"the case after another token\": a request under a token of its own is applied"
    );
}

#[tokio::test]
async fn a_batch_association_puts_one_comment_on_every_case_it_resolves() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "batch-related-item-domain").await;
    let domain_id = fixture.domain_id.clone();
    let comment = json!({
        "comment": { "body": "one comment on several cases", "contentType": "Text/Plain" }
    });

    let answer = emulator
        .ok(
            "BatchCreateRelatedItem",
            "POST",
            BATCH_CREATE_RELATED_ITEM_PATH,
            json!({
                "domainId": domain_id,
                "content": comment,
                "cases": [
                    by_case_id(&fixture.first_case),
                    by_reference_id(REFERENCE_ID),
                    by_case_id(UNHELD_CASE_ID),
                ],
            }),
        )
        .await;

    let created = entries(&answer, "successfulItems", "BatchCreateRelatedItem");
    assert_eq!(
        created
            .iter()
            .map(|entry| entry_case_id(entry, "BatchCreateRelatedItem"))
            .collect::<Vec<String>>(),
        vec![fixture.first_case.clone(), fixture.second_case.clone()],
        "case \"BatchCreateRelatedItem\": every entry the request resolved is reported as \
         created, in the order it addressed them, got {created:?}"
    );
    for entry in &created {
        let case_id = entry_case_id(entry, "BatchCreateRelatedItem");
        let related_item_id = string_at(entry, "relatedItemId", "BatchCreateRelatedItem");
        assert!(
            is_minted_id(&related_item_id),
            "case \"BatchCreateRelatedItem\": the relatedItemId must be a server-minted opaque \
             id, got {related_item_id:?}"
        );
        assert_eq!(
            string_at(entry, "relatedItemArn", "BatchCreateRelatedItem"),
            format!(
                "arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case/{case_id}/related-item/{related_item_id}"
            ),
            "case \"BatchCreateRelatedItem\": the ARN addresses the item under its own case, got \
             {entry:?}"
        );
    }
    assert_eq!(
        entries(&answer, "failedItems", "BatchCreateRelatedItem"),
        vec![json!({
            "caseIdentifier": { "caseId": UNHELD_CASE_ID },
            "errorCode": BATCH_CASE_NOT_FOUND_CODE,
            "errorMessage": BATCH_CASE_NOT_FOUND_MESSAGE,
        })],
        "case \"BatchCreateRelatedItem\": an entry naming a case the domain does not hold fails \
         on its own, got {answer:?}"
    );

    // Each resolved case holds the comment, read back through the search of the
    // whole domain, and the association is recorded in the case's history.
    let items = related_items(&emulator, "the batched comments", &domain_id).await;
    for (label, case_id) in [
        ("the first case", &fixture.first_case),
        ("the second case", &fixture.second_case),
    ] {
        assert_eq!(
            items
                .iter()
                .filter(|(held_by, _)| held_by == case_id)
                .map(|(_, content)| content.clone())
                .collect::<Vec<Value>>(),
            vec![comment.clone()],
            "case {label:?}: the case holds the one comment the batch associated with it"
        );
        assert_eq!(
            audit_event_types(&emulator, label, &domain_id, case_id).await,
            vec![
                "RelatedItem.Created".to_string(),
                "Case.Created".to_string()
            ],
            "case {label:?}: a batched association is recorded in the case's audit history"
        );
    }

    // A retry under the same client token is answered once; a token reused for
    // other parameters is a conflict.
    let token = "a-client-token-for-one-association";
    let body = json!({
        "domainId": domain_id,
        "content": comment,
        "cases": [by_case_id(&fixture.first_case)],
        "clientToken": token,
    });
    let first = emulator
        .ok("the first association", "POST", BATCH_CREATE_RELATED_ITEM_PATH, body.clone())
        .await;
    let retried = emulator
        .ok("the retried association", "POST", BATCH_CREATE_RELATED_ITEM_PATH, body)
        .await;
    assert_eq!(
        retried, first,
        "case \"the retried association\": a retry under the same client token is answered with \
         the first answer, got {retried:?}"
    );
    let after = related_items(&emulator, "after the retry", &domain_id).await;
    assert_eq!(
        after
            .iter()
            .filter(|(held_by, _)| held_by == &fixture.first_case)
            .count(),
        2,
        "case \"after the retry\": the retry associated no second item, got {after:?}"
    );
    let (status, code) = emulator
        .failure(
            "the same token with other content",
            "POST",
            BATCH_CREATE_RELATED_ITEM_PATH,
            Some(json!({
                "domainId": domain_id,
                "content": {
                    "comment": { "body": "a different comment", "contentType": "Text/Plain" }
                },
                "cases": [by_case_id(&fixture.first_case)],
                "clientToken": token,
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case \"the same token with other content\": reusing a client token for different \
         parameters is a conflict, got {status} {code}"
    );
}

#[tokio::test]
async fn a_batch_association_of_anything_but_a_comment_creates_nothing() {
    let emulator = Emulator::new();
    let fixture = fixture(&emulator, "batch-related-item-content-domain").await;
    let domain_id = fixture.domain_id.clone();
    let linked = case_with(
        &emulator,
        "a case to link to",
        &domain_id,
        &fixture.template_id,
        vec![],
    )
    .await;

    // "This operation supports only the Comment related item type": the rows
    // are the other five members of `RelatedItemInputContent`, which
    // `CreateRelatedItem` accepts one case at a time.
    let rows: Vec<(&str, Value)> = vec![
        (
            "contact content",
            json!({
                "contact": {
                    "contactArn": "arn:aws:connect:us-east-1:000000000000:instance/i-1/contact/c-1"
                }
            }),
        ),
        (
            "file content",
            json!({
                "file": {
                    "fileArn": "arn:aws:connect:us-east-1:000000000000:instance/i-1/file/f-1"
                }
            }),
        ),
        (
            "sla content",
            json!({
                "sla": {
                    "slaInputConfiguration": {
                        "name": "resolve in an hour",
                        "type": "CaseField",
                        "fieldId": fixture.text_field,
                        "targetFieldValues": [{ "stringValue": "resolved" }],
                        "targetSlaMinutes": 60,
                    }
                }
            }),
        ),
        (
            "connectCase content",
            json!({ "connectCase": { "caseId": linked } }),
        ),
        (
            "custom content",
            json!({
                "custom": {
                    "fields": [field(&fixture.number_field, json!({ "doubleValue": 3.0 }))]
                }
            }),
        ),
    ];

    for (label, content) in &rows {
        let (status, code) = emulator
            .failure(
                label,
                "POST",
                BATCH_CREATE_RELATED_ITEM_PATH,
                Some(json!({
                    "domainId": domain_id,
                    "content": content,
                    "cases": [by_case_id(&fixture.first_case)],
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: only comment content can be associated with several cases at once, \
             got {status} {code}"
        );
    }

    // A request-level mistake creates nothing either: a duplicate entry, a
    // reference id that resolves to nothing, and a domain nothing holds.
    let refusals: Vec<Refusal> = vec![
        Refusal {
            label: "one case addressed twice",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "content": {
                    "comment": { "body": "twice over", "contentType": "Text/Plain" }
                },
                "cases": [by_case_id(&fixture.first_case), by_case_id(&fixture.first_case)],
            }),
        },
        Refusal {
            label: "a reference id that resolves to nothing",
            status: 400,
            code: "ValidationException",
            body: json!({
                "domainId": domain_id,
                "content": {
                    "comment": { "body": "nowhere to go", "contentType": "Text/Plain" }
                },
                "cases": [by_reference_id("REF-NOTHING")],
            }),
        },
        Refusal {
            label: "a domain nothing holds",
            status: 404,
            code: "ResourceNotFoundException",
            body: json!({
                "domainId": "a-domain-nothing-holds",
                "content": {
                    "comment": { "body": "no domain", "contentType": "Text/Plain" }
                },
                "cases": [by_case_id(&fixture.first_case)],
            }),
        },
    ];
    for row in &refusals {
        let (status, code) = emulator
            .failure(
                row.label,
                "POST",
                BATCH_CREATE_RELATED_ITEM_PATH,
                Some(row.body.clone()),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (row.status, row.code),
            "case {:?}: expected {} {}, got {status} {code}",
            row.label,
            row.status,
            row.code
        );
    }

    assert!(
        related_items(&emulator, "after every refusal", &domain_id)
            .await
            .is_empty(),
        "case \"after every refusal\": a refused batch association creates nothing"
    );
}
