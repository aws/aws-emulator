// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for `UpdateField` addressed at a field the domain
//! can no longer resolve, driven through the real `MultiServiceRouter`.
//!
//! The rule under test: an update ADDRESSED at a resource the service no longer
//! resolves — one the container holds no record for at all, and one whose record
//! is only a deletion tombstone alike — is refused with the modelled
//! `ResourceNotFoundException` (HTTP 404), whose message is the service's
//! generic sentence with no variable part and whose required `resourceId` and
//! `resourceType` members report the id the request carried and the lowercase
//! token of the kind it addressed. A field's delete is a SOFT delete, so the
//! record survives for the reads the service documents as surviving it
//! (`BatchGetField` still reports it with `deleted` true) — but it is no longer
//! a field an update can resolve.
//!
//! `UpdateField` = `PUT /domains/{domainId}/fields/{fieldId}` with
//! `ResourceNotFoundException` (404, `message`/`resourceId`/`resourceType` all
//! required) among its modelled errors
//! (`smithy_query(query="operation",operation="UpdateField")`,
//! `smithy_query(query="shape",shape="ResourceNotFoundException")`).
//!
//! The rows below span what the rule generalises over: which `FieldType` the
//! unresolvable field has (every value of `FIELD_TYPE_VALUES`), which members
//! the update carries (none, each one on its own, all of them), how the field
//! came to be unresolvable (never minted, deleted once, deleted twice through
//! the idempotent delete) and which kind of resource the update addresses
//! (a field and a layout, which the same rule governs).
//!
//! The documented deleted-field `ValidationException` is a DIFFERENT refusal,
//! raised by a call that merely REFERENCES a deleted field in its payload while
//! addressing another resource; the last test pins that boundary so this rule
//! does not swallow it.

use serde_json::{json, Value};

use crate::harness::{
    domain, field_of_type, fields_of_every_type, read_field, refused_as, string_at, title_value,
    Emulator, Refusal,
};
use crate::layout_lifecycle::{create_layout, top_panel};

/// The generic not-found sentence this service words a refusal with when it
/// reports the resource through the error's own members instead
/// (`state::RESOURCE_NOT_FOUND_MESSAGE`).
const NOT_FOUND_SENTENCE: &str = "Requested resource not found.";

/// `UpdateField`: `PUT /domains/{domainId}/fields/{fieldId}`.
fn update_path(domain_id: &str, field_id: &str) -> String {
    format!("/domains/{domain_id}/fields/{field_id}")
}

/// `DeleteField` on `field_id`, which is `@idempotent`, so a suite may repeat it.
async fn delete_field(emulator: &Emulator, case: &str, domain_id: &str, field_id: &str) {
    emulator
        .ok(
            case,
            "DELETE",
            &format!("/domains/{domain_id}/fields/{field_id}"),
            json!({}),
        )
        .await;
}

/// Assert the whole modelled 404 for `resource_id` of `resource_type`: the
/// status, the error code, the fixed sentence and the two members that report
/// what could not be resolved — through the one assertion of that body this
/// crate's suites share ([`refused_as`]).
async fn refused_as_missing(
    emulator: &Emulator,
    case: &str,
    path: &str,
    body: Value,
    resource_type: &str,
    resource_id: &str,
) {
    refused_as(
        emulator,
        case,
        "PUT",
        path,
        Some(body),
        &Refusal {
            resource_type,
            resource_id,
            sentence: NOT_FOUND_SENTENCE,
        },
    )
    .await;
}

/// The forms an `UpdateField` body takes: no member at all, each modifiable
/// member on its own, and all of them together. `attributes` is the model's
/// `FieldAttributes` union, whose one variant is `text`
/// (`smithy_query(query="shape",shape="FieldAttributes")`).
fn update_bodies() -> Vec<(&'static str, Value)> {
    vec![
        ("carrying no member", json!({})),
        (
            "carrying only a name",
            json!({ "name": "renamed-after-delete" }),
        ),
        (
            "carrying only a description",
            json!({ "description": "post-delete-update" }),
        ),
        (
            "carrying only attributes",
            json!({ "attributes": { "text": { "isMultiline": true } } }),
        ),
        (
            "carrying every modifiable member",
            json!({
                "name": "renamed-and-described",
                "description": "post-delete-update",
                "attributes": { "text": { "isMultiline": false } },
            }),
        ),
    ]
}

#[tokio::test]
async fn an_update_addressed_at_a_deleted_field_is_refused_as_not_found() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-after-delete").await;

    // One row per `FieldType` the model declares, each with a different form of
    // update body and each deleted once or twice (the delete is `@idempotent`,
    // so a field deleted twice is in exactly the same state).
    let bodies = update_bodies();
    for (index, (field_type, field_id)) in fields_of_every_type(&emulator, &domain_id)
        .await
        .into_iter()
        .enumerate()
    {
        let (form, body) = &bodies[index % bodies.len()];
        let repeats = 1 + index % 2;
        let case = format!("a deleted {field_type} field, deleted {repeats}x, {form}");
        let case = case.as_str();

        let before = read_field(&emulator, case, &domain_id, &field_id).await;
        for _ in 0..repeats {
            delete_field(&emulator, case, &domain_id, &field_id).await;
        }

        refused_as_missing(
            &emulator,
            case,
            &update_path(&domain_id, &field_id),
            body.clone(),
            "field",
            &field_id,
        )
        .await;

        // The refused update wrote nothing: the tombstone the delete left is
        // still exactly what `BatchGetField` reported before it, with `deleted`
        // now true and every other member untouched.
        let after = read_field(&emulator, case, &domain_id, &field_id).await;
        assert_eq!(
            (
                after.get("name"),
                after.get("description"),
                after.get("attributes"),
                after.get("type"),
                after.get("deleted"),
            ),
            (
                before.get("name"),
                before.get("description"),
                before.get("attributes"),
                before.get("type"),
                Some(&json!(true)),
            ),
            "case {case:?}: a refused update must leave the deleted record alone, got {after:?} \
             from {before:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_field_reference_that_was_never_minted_is_refused_alike() {
    // The same refusal, from the other way a field fails to resolve: an id no
    // domain ever minted, a well-formed id belonging to another domain's
    // schema, and a reserved `System` id addressed in a domain that does not
    // exist. Each reports the id the REQUEST carried, never a stored one.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-unresolvable").await;
    let other_domain = domain(&emulator, "field-update-unresolvable-elsewhere").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");

    // Each row carries the pair the refusal must report: the identifier that
    // could not be resolved, and its kind's token. A domain the store does not
    // hold is reported as the DOMAIN, because that is the reference the handler
    // resolves first.
    let rows: Vec<(&str, String, String, &str, String)> = vec![
        (
            "an id the service never minted",
            domain_id.clone(),
            "no-such-field".to_string(),
            "field",
            "no-such-field".to_string(),
        ),
        (
            "an id of another domain's schema",
            other_domain.clone(),
            text_field.clone(),
            "field",
            text_field.clone(),
        ),
        (
            "a reserved System id in a domain the store does not hold",
            "no-such-domain-for-a-field-update".to_string(),
            "reference_number".to_string(),
            "domain",
            "no-such-domain-for-a-field-update".to_string(),
        ),
    ];

    for (case, domain_label, field_id, expected_type, expected_id) in rows {
        refused_as_missing(
            &emulator,
            case,
            &update_path(&domain_label, &field_id),
            json!({ "description": "post-delete-update" }),
            expected_type,
            &expected_id,
        )
        .await;
    }
}

#[tokio::test]
async fn an_update_addressed_at_a_deleted_layout_is_refused_the_same_way() {
    // The rule is about the resource an update ADDRESSES, not about fields: a
    // layout's delete is a soft delete too, and an update of a deleted layout
    // answers the same modelled 404 with its own `resourceType` token.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-update-after-delete").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");
    let case = "a deleted layout";

    let (layout_id, _) = create_layout(
        &emulator,
        case,
        &domain_id,
        "the-layout-to-delete",
        &top_panel(&[text_field]),
    )
    .await;
    emulator
        .ok(
            case,
            "DELETE",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            json!({}),
        )
        .await;

    refused_as_missing(
        &emulator,
        case,
        &format!("/domains/{domain_id}/layouts/{layout_id}"),
        json!({ "name": "renamed-after-delete" }),
        "layout",
        &layout_id,
    )
    .await;
}

#[tokio::test]
async fn a_live_field_still_takes_every_form_of_update() {
    // The control for the refusal above: the branch that refuses an
    // unresolvable field must not refuse a LIVE one, whatever its type and
    // whichever members the update carries.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-live").await;
    let bodies = update_bodies();
    for (index, (field_type, field_id)) in fields_of_every_type(&emulator, &domain_id)
        .await
        .into_iter()
        .enumerate()
    {
        let (form, template) = &bodies[index % bodies.len()];
        let case = format!("a live {field_type} field, {form}");
        let case = case.as_str();
        // A field name is unique among a domain's live fields, so a row that
        // renames gets a name of its own rather than the one an earlier row
        // already took.
        let mut body = template.clone();
        if let Some(name) = body.get("name").and_then(Value::as_str) {
            let unique = format!("{name}-{index}");
            body["name"] = json!(unique);
        }
        let answer = emulator
            .ok(case, "PUT", &update_path(&domain_id, &field_id), body.clone())
            .await;
        assert_eq!(
            answer,
            json!({}),
            "case {case:?}: `UpdateFieldResponse` declares no members, got {answer:?}"
        );
        let read = read_field(&emulator, case, &domain_id, &field_id).await;
        assert_eq!(
            read.get("deleted"),
            Some(&json!(false)),
            "case {case:?}: an updated live field must stay live, got {read:?}"
        );
        if let Some(expected) = body.get("name") {
            assert_eq!(
                read.get("name"),
                Some(expected),
                "case {case:?}: the update must be durable, got {read:?}"
            );
        }
        if let Some(expected) = body.get("description") {
            assert_eq!(
                read.get("description"),
                Some(expected),
                "case {case:?}: the update must be durable, got {read:?}"
            );
        }
        if let Some(expected) = body.get("attributes") {
            assert_eq!(
                read.get("attributes"),
                Some(expected),
                "case {case:?}: the update must be durable, got {read:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_payload_reference_to_a_deleted_field_is_still_the_documented_validation_refusal() {
    // The boundary: "Calling CreateCase with a deleted field throws a
    // ValidationException denoting which field identifiers in the request have
    // been deleted ... Calling BatchPutFieldOptions with a deleted field ID
    // throw a ValidationException"
    // (`smithy_query(query="doc",shape="DeleteField")`). Those calls ADDRESS a
    // case and a field's option set respectively while REFERENCING the deleted
    // field, so they keep their own documented 400 — only an update addressed at
    // the deleted field itself is the 404.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-reference-after-delete").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");
    let select_field = field_of_type(&fields, "SingleSelect");
    let template_id = {
        let created = emulator
            .ok(
                "a template over the live fields",
                "POST",
                &format!("/domains/{domain_id}/templates"),
                json!({ "name": "a-template-over-live-fields" }),
            )
            .await;
        string_at(&created, "templateId", "a template over the live fields")
    };
    for field_id in [&text_field, &select_field] {
        delete_field(&emulator, "DeleteField", &domain_id, field_id).await;
    }

    let rows: Vec<(&str, &str, String, Value)> = vec![
        (
            "CreateCase writing a value for a deleted field",
            "POST",
            format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [
                    title_value("a case referencing a deleted field"),
                    { "id": text_field, "value": { "stringValue": "a customer note" } },
                ],
            }),
        ),
        (
            "BatchPutFieldOptions on a deleted field",
            "PUT",
            format!("/domains/{domain_id}/fields/{select_field}/options"),
            json!({ "options": [{ "name": "Urgent", "value": "urgent", "active": true }] }),
        ),
    ];

    for (case, method, path, body) in rows {
        let (status, code, message) = emulator
            .failure_detail(case, method, &path, Some(body))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected the documented 400 ValidationException, got {status} {code}: \
             {message}"
        );
    }
}
