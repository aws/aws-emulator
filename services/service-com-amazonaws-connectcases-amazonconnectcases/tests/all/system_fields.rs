// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the reserved `System`-namespace fields a
//! Cases domain is provisioned with, for what a batch read reports per item,
//! for the message a create refused on a taken name renders, and for an update
//! of a field's properties.
//!
//! The rules under test:
//!
//! 1. A domain's field schema holds the service's reserved `System`-namespace
//!    fields from the moment the domain is created — under their reserved ids,
//!    display names and types, verbatim and identical in every domain — so
//!    every operation that resolves a field id (`CreateCase`, `UpdateCase`,
//!    `GetCase`, `BatchGetField`) resolves them exactly as it resolves a field
//!    `CreateField` made, and a listing reports the whole set.
//! 2. A batch read partially succeeds: HTTP 200, with each id it cannot resolve
//!    reported in its own `errors` list under the service's per-item error code
//!    (`ResourceNotFound`), not under the name of the modelled exception shape a
//!    whole call would fail with, and under the service's fixed per-item
//!    sentence, which interpolates no part of the request.
//! 3. A create refused because a live resource of the same kind already holds
//!    the requested name answers 409 `ConflictException` with the service's
//!    fixed message template, rendered from the REQUEST's own name and carrying
//!    no server-minted id.
//! 4. An accepted update of a resource's properties is durable: every property
//!    the request carried, and only those, is what every later read of that
//!    resource reports.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    BATCH_ITEM_NOT_FOUND_CODE, BATCH_ITEM_NOT_FOUND_MESSAGE, FIELD_NAMESPACE_SYSTEM,
    RESERVED_SYSTEM_FIELDS,
};

use crate::harness::{
    batch_get_fields, domain, field_of_type, fields_of_every_type, get_case_fields, is_minted_id,
    listed_fields, read_field, refused_as, string_at, template, Emulator, Refusal,
};

/// The reserved id whose value every row below leaves unwritten, so a read can
/// tell an in-schema field that was never written (no entry at all) from one
/// that is not in the schema at all (a refusal).
const NEVER_WRITTEN_RESERVED_ID: &str = "reference_number";

/// The value of a `GetCase` entry for `field_id`, which the handler answers in
/// the order the ids were requested.
fn value_of(fields: &[Value], field_id: &str, case: &str) -> Value {
    fields
        .iter()
        .find(|entry| entry.get("id").and_then(Value::as_str) == Some(field_id))
        .and_then(|entry| entry.get("value").cloned())
        .unwrap_or_else(|| panic!("case {case:?}: no entry for {field_id:?} in {fields:?}"))
}

#[tokio::test]
async fn every_new_domain_is_provisioned_with_the_reserved_system_fields() {
    // The catalog in `state::RESERVED_SYSTEM_FIELDS` is the table: every
    // reserved id resolves in a domain nothing but `CreateDomain` has touched,
    // in the `System` namespace, under its own declared `FieldType` — including
    // the ids the issue never cited.
    let emulator = Emulator::new();
    let first = domain(&emulator, "system-fields-domain").await;
    let second = domain(&emulator, "another-system-fields-domain").await;

    // Entries of the catalog pinned as literals, so a wrong catalog entry
    // fails here rather than being carried into the expectations below. The
    // display name is the API's own wording for the field, every word
    // capitalised and an instant written as `Date/Time`.
    for (field_id, name, field_type) in [
        ("title", "Title", "Text"),
        ("created_datetime", "Date/Time Opened", "DateTime"),
        ("last_updated_user", "Last Updated User", "User"),
    ] {
        assert!(
            RESERVED_SYSTEM_FIELDS.contains(&(field_id, name, field_type)),
            "case \"the reserved catalog\": {field_id} must be reserved as a {field_type} field \
             named {name:?}, got {RESERVED_SYSTEM_FIELDS:?}"
        );
    }

    for domain_id in [&first, &second] {
        let case = format!("BatchGetField every reserved id in {domain_id}");
        let requested: Vec<Value> = RESERVED_SYSTEM_FIELDS
            .iter()
            .map(|(field_id, _, _)| json!({ "id": field_id }))
            .collect();
        let response = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/fields-batch"),
                json!({ "fields": requested }),
            )
            .await;
        let errors = response
            .get("errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no errors list in {response:?}"))
            .clone();
        assert!(
            errors.is_empty(),
            "case {case:?}: every reserved id must resolve in a new domain, got {errors:?}"
        );
        let read = response
            .get("fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no fields list in {response:?}"))
            .clone();
        let by_id: HashMap<String, Value> = read
            .iter()
            .map(|field| (string_at(field, "fieldId", &case), field.clone()))
            .collect();
        assert_eq!(
            by_id.len(),
            RESERVED_SYSTEM_FIELDS.len(),
            "case {case:?}: expected one entry per reserved id, got {read:?}"
        );
        for (field_id, name, field_type) in RESERVED_SYSTEM_FIELDS {
            let field = by_id
                .get(*field_id)
                .unwrap_or_else(|| panic!("case {case:?}: {field_id} missing from {read:?}"));
            assert_eq!(
                string_at(field, "type", &case),
                *field_type,
                "case {case:?}: {field_id} must carry its reserved FieldType, got {field:?}"
            );
            assert_eq!(
                string_at(field, "namespace", &case),
                FIELD_NAMESPACE_SYSTEM,
                "case {case:?}: {field_id} is a service-defined field, got {field:?}"
            );
            assert_eq!(
                string_at(field, "name", &case),
                *name,
                "case {case:?}: a reserved field's name is the service's display name for it, \
                 got {field:?}"
            );
            assert_eq!(
                field.get("deleted").and_then(Value::as_bool),
                Some(false),
                "case {case:?}: {field_id} must be live, got {field:?}"
            );
            assert_eq!(
                string_at(field, "fieldArn", &case),
                format!(
                    "arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/field/{field_id}"
                ),
                "case {case:?}: {field_id} must be addressable in its own domain, got {field:?}"
            );
            assert!(
                !is_minted_id(field_id),
                "case {case:?}: a reserved id is the service's own word, not a minted id, \
                 got {field_id:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_case_is_written_and_read_through_the_reserved_system_fields() {
    // A reserved field is in the domain's schema, so a case writes and reads it
    // like any other field — including the type check its own `FieldType`
    // carries — and one that was never written is left out of the read.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-on-system-fields").await;
    let custom = emulator
        .ok(
            "CreateField beside the reserved ones",
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": "a-custom-note", "type": "Text" }),
        )
        .await;
    let custom_id = string_at(&custom, "fieldId", "CreateField beside the reserved ones");
    let template_id = template(
        &emulator,
        &domain_id,
        "system-fields-template",
        std::slice::from_ref(&custom_id),
    )
    .await;

    // One row per reserved field this case writes, each in the union variant
    // its catalogued `FieldType` takes: the string-valued types in
    // `stringValue`, a `User` field in `userArnValue`, and `emptyValue` for a
    // field written with the union's absent value.
    let written: Vec<(&str, Value)> = vec![
        ("title", json!({ "stringValue": "a lost order" })),
        (
            "customer_id",
            json!({ "stringValue": "arn:aws:profile:us-east-1:000000000000:domains/d-1/profiles/p-1" }),
        ),
        ("status", json!({ "stringValue": "open" })),
        (
            "summary",
            json!({ "stringValue": "the customer never received the order" }),
        ),
        (
            "created_datetime",
            json!({ "stringValue": "2026-10-22T14:45:10.000Z" }),
        ),
        (
            "assigned_queue",
            json!({ "stringValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/queue/q-1" }),
        ),
        (
            "assigned_user",
            json!({ "stringValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1" }),
        ),
        (
            "created_by",
            json!({ "userArnValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1" }),
        ),
        ("case_reason", json!({ "emptyValue": {} })),
    ];

    let mut body_fields: Vec<Value> = written
        .iter()
        .map(|(field_id, value)| json!({ "id": field_id, "value": value }))
        .collect();
    body_fields.push(json!({
        "id": custom_id,
        "value": { "stringValue": "written beside the reserved fields" },
    }));
    let created = emulator
        .ok(
            "CreateCase over the reserved fields",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({ "templateId": template_id, "fields": body_fields }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase over the reserved fields");
    assert!(
        is_minted_id(&case_id),
        "case \"CreateCase over the reserved fields\": the caseId must be a minted id, \
         got {case_id:?}"
    );
    assert_eq!(
        string_at(&created, "caseArn", "CreateCase over the reserved fields"),
        format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case/{case_id}"),
        "case \"CreateCase over the reserved fields\": the caseArn must address the stored case, \
         got {created:?}"
    );

    let mut requested: Vec<String> = written
        .iter()
        .map(|(field_id, _)| (*field_id).to_string())
        .collect();
    requested.push(custom_id.clone());
    requested.push(NEVER_WRITTEN_RESERVED_ID.to_string());
    let read = get_case_fields(
        &emulator,
        "GetCase over the reserved fields",
        &domain_id,
        &case_id,
        &requested,
    )
    .await;
    for (field_id, value) in &written {
        assert_eq!(
            &value_of(&read, field_id, "GetCase over the reserved fields"),
            value,
            "case \"GetCase over the reserved fields\": {field_id} must read back what was \
             written, got {read:?}"
        );
    }
    assert!(
        !read
            .iter()
            .any(|entry| entry.get("id").and_then(Value::as_str) == Some(NEVER_WRITTEN_RESERVED_ID)),
        "case \"GetCase unwritten reserved field\": a reserved field the case never wrote holds \
         no value, so it must not be reported, got {read:?}"
    );

    // An update writes a reserved field the same way, and the read reports the
    // new value while the rest of the case is untouched.
    emulator
        .ok(
            "UpdateCase a reserved field",
            "PUT",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({ "fields": [{ "id": "title", "value": { "stringValue": "a lost order, reopened" } }] }),
        )
        .await;
    let after = get_case_fields(
        &emulator,
        "GetCase after the reserved-field update",
        &domain_id,
        &case_id,
        &["title".to_string(), "summary".to_string()],
    )
    .await;
    assert_eq!(
        value_of(&after, "title", "GetCase after the reserved-field update"),
        json!({ "stringValue": "a lost order, reopened" }),
        "case \"GetCase after the reserved-field update\": the update must be reported, \
         got {after:?}"
    );
    assert_eq!(
        value_of(&after, "summary", "GetCase after the reserved-field update"),
        json!({ "stringValue": "the customer never received the order" }),
        "case \"GetCase after the reserved-field update\": an unwritten field must be left \
         alone, got {after:?}"
    );

    // The reserved field's own declared type is enforced, so a value that is
    // not a value of that type is refused rather than stored — and the refusal
    // names the reserved field and the union variant its type takes.
    for (field_id, field_type, expected_variant, bad_value) in [
        ("title", "Text", "stringValue", json!({ "booleanValue": true })),
        (
            "created_by",
            "User",
            "userArnValue",
            json!({ "stringValue": "not a user arn" }),
        ),
        (
            "case_reason",
            "SingleSelect",
            "stringValue",
            json!({ "doubleValue": 1.5 }),
        ),
    ] {
        let case = format!("UpdateCase {field_id} with a non-{field_type} value");
        let (status, code, message) = emulator
            .failure_detail(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/cases/{case_id}"),
                Some(json!({ "fields": [{ "id": field_id, "value": bad_value }] })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        assert_eq!(
            message,
            format!("Field '{field_id}' value should be a {expected_variant}"),
            "case {case:?}: the refusal must name the field and the variant its type takes, \
             got {message:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_read_reports_an_unresolvable_id_under_the_per_item_error_code() {
    // The whole call succeeds (HTTP 200) and each id it could not resolve is
    // reported in `errors` under the per-item code, whatever the FORM of the id
    // and however many of them a request mixes with resolvable ones.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "batch-per-item-errors").await;
    let created = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&created, "Text");
    let number = field_of_type(&created, "Number");

    let rows: Vec<(&str, Vec<String>, Vec<String>)> = vec![
        (
            "one unresolvable id alone",
            vec![],
            vec!["no-such-field".to_string()],
        ),
        (
            "a resolvable id and an unresolvable one",
            vec![text.clone()],
            vec!["sim2real-nonexistent-field-id-0000000000".to_string()],
        ),
        (
            "an id one character long, the modelled FieldId minimum",
            vec![text.clone(), number.clone()],
            vec!["a".to_string()],
        ),
        (
            "an id at the modelled FieldId maximum of 500",
            vec![number.clone()],
            vec!["x".repeat(500)],
        ),
        (
            "an id that only looks minted",
            vec![],
            vec!["00000000-0000-4000-8000-000000000000".to_string()],
        ),
        (
            "ids that differ from a reserved word only in case or spelling",
            vec![text.clone()],
            vec![
                "Title".to_string(),
                "titles".to_string(),
                "customer-id".to_string(),
            ],
        ),
        (
            "a reserved id of another domain's schema is still in this one",
            vec!["title".to_string(), "customer_id".to_string()],
            vec!["not_a_reserved_id".to_string()],
        ),
    ];

    for (case, resolvable, unresolvable) in rows {
        // The unresolvable ids go FIRST, so the partition cannot be an artefact
        // of the order the request listed them in.
        let requested: Vec<Value> = unresolvable
            .iter()
            .chain(resolvable.iter())
            .map(|id| json!({ "id": id }))
            .collect();
        let response = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields-batch"),
                json!({ "fields": requested }),
            )
            .await;
        let fields = response
            .get("fields")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no fields list in {response:?}"))
            .clone();
        let errors = response
            .get("errors")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no errors list in {response:?}"))
            .clone();
        assert_eq!(
            fields.len(),
            resolvable.len(),
            "case {case:?}: expected {} resolved field(s), got {fields:?}",
            resolvable.len()
        );
        assert_eq!(
            errors.len(),
            unresolvable.len(),
            "case {case:?}: expected {} per-item error(s), got {errors:?}",
            unresolvable.len()
        );
        for (index, id) in unresolvable.iter().enumerate() {
            let error = &errors[index];
            assert_eq!(
                &string_at(error, "id", case),
                id,
                "case {case:?}: the error must name the id that failed, got {error:?}"
            );
            let code = string_at(error, "errorCode", case);
            assert_eq!(
                code, BATCH_ITEM_NOT_FOUND_CODE,
                "case {case:?}: expected the per-item code {BATCH_ITEM_NOT_FOUND_CODE:?}, \
                 got {code:?}"
            );
            assert_ne!(
                code, "ResourceNotFoundException",
                "case {case:?}: the per-item code must not be the modelled exception shape's \
                 name, got {error:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_field_name_already_taken_is_refused_with_the_services_message_template() {
    // Whatever the form of the requested name, a loser's message is the
    // service's fixed template filled from the name the REQUEST carried, and
    // only one create wins the name.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "duplicate-field-names").await;
    let path = format!("/domains/{domain_id}/fields");

    // Rows differ along the form of the name; the last one is a name the domain
    // already holds because it is a reserved System field, so nothing creates it
    // first.
    let rows: Vec<(&str, String, bool)> = vec![
        ("an ascii name", "customer-note".to_string(), false),
        ("a name with spaces", "Customer note two".to_string(), false),
        ("a name outside ascii", "título del caso".to_string(), false),
        (
            "a name at the modelled 100-character maximum",
            "n".repeat(100),
            false,
        ),
        (
            "a reserved System field's own name",
            "Title".to_string(),
            true,
        ),
    ];

    for (form, name, already_held) in rows {
        let case = format!("CreateField with {form}");
        let winner = if already_held {
            None
        } else {
            let created = emulator
                .ok(&case, "POST", &path, json!({ "name": name, "type": "Text" }))
                .await;
            let field_id = string_at(&created, "fieldId", &case);
            assert!(
                is_minted_id(&field_id),
                "case {case:?}: the winner's fieldId must be a minted id, got {field_id:?}"
            );
            Some(field_id)
        };
        let expected =
            format!("Field creation failed: field resource with the name {name} already exists!");
        for attempt in 0..3 {
            let conflict = format!("{case}, loser #{attempt}");
            let (status, code, message) = emulator
                .failure_detail(
                    &conflict,
                    "POST",
                    &path,
                    Some(json!({ "name": name, "type": "Text" })),
                )
                .await;
            assert_eq!(
                (status, code.as_str()),
                (409, "ConflictException"),
                "case {conflict:?}: expected 409 ConflictException, got {status} {code}: {message}"
            );
            assert_eq!(
                message, expected,
                "case {conflict:?}: expected the service's template {expected:?}, got {message:?}"
            );
            if let Some(winner) = winner.as_deref() {
                assert!(
                    !message.contains(winner),
                    "case {conflict:?}: the message must carry no server-minted id the loser \
                     never saw, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn a_layout_may_not_place_a_reserved_field_that_is_not_configurable() {
    // The model states this for the layout operations in particular: "Title and
    // Status fields cannot be part of layouts since they are not configurable"
    // (`smithy_query(query="doc",shape="CreateLayout")`). The domain's schema
    // DOES hold those reserved fields now, so the refusal is its own check
    // rather than a side effect of the id not resolving — while every other
    // reserved field is placeable like a Custom one.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "layout-reserved-fields").await;
    let content = |ids: &[&str]| {
        let fields: Vec<Value> = ids.iter().map(|id| json!({ "id": id })).collect();
        json!({ "basic": { "topPanel": { "sections": [{ "fieldGroup": { "fields": fields } }] } } })
    };

    // Rows differ in which non-configurable id is placed, where in the list it
    // sits, which panel holds it, and how many of them a request carries.
    for (case, placed, refused_ids) in [
        ("title alone", vec!["title"], vec!["title"]),
        ("status alone", vec!["status"], vec!["status"]),
        (
            "a non-configurable id after a placeable one",
            vec!["summary", "status"],
            vec!["status"],
        ),
        (
            "both non-configurable ids, title first",
            vec!["title", "summary", "status"],
            vec!["title", "status"],
        ),
    ] {
        let (status, code, message) = emulator
            .failure_detail(
                case,
                "POST",
                &format!("/domains/{domain_id}/layouts"),
                Some(json!({ "name": format!("layout-{case}"), "content": content(&placed) })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
        );
        for refused in &refused_ids {
            assert!(
                message.contains(refused),
                "case {case:?}: the refusal must name {refused}, got {message:?}"
            );
        }
    }

    // Every other reserved field is a field of the domain like any other, so a
    // layout may place it, and the stored layout reads back with it.
    let placeable = ["summary", "customer_id", "assigned_user", "case_reason"];
    let created = emulator
        .ok(
            "a layout over the placeable reserved fields",
            "POST",
            &format!("/domains/{domain_id}/layouts"),
            json!({ "name": "placeable-reserved", "content": content(&placeable) }),
        )
        .await;
    let layout_id = string_at(&created, "layoutId", "a layout over the placeable reserved fields");
    let fetched = emulator
        .ok(
            "GetLayout over the placeable reserved fields",
            "POST",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            json!({}),
        )
        .await;
    let listed = fetched
        .get("content")
        .and_then(|content| content.get("basic"))
        .and_then(|basic| basic.get("topPanel"))
        .and_then(|panel| panel.get("sections"))
        .and_then(Value::as_array)
        .and_then(|sections| sections.first())
        .and_then(|section| section.get("fieldGroup"))
        .and_then(|group| group.get("fields"))
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case \"GetLayout over the placeable reserved fields\": no fields in {fetched:?}")
        })
        .clone();
    let ids: Vec<String> = listed
        .iter()
        .map(|field| string_at(field, "id", "GetLayout over the placeable reserved fields"))
        .collect();
    assert_eq!(
        ids,
        placeable.map(str::to_string).to_vec(),
        "case \"GetLayout over the placeable reserved fields\": the stored layout must report \
         the reserved fields it placed, in order, got {fetched:?}"
    );

    // And the same refusal guards the update path, so a layout cannot be
    // rewritten into one the create path would have refused.
    let (status, code, message) = emulator
        .failure_detail(
            "UpdateLayout placing a non-configurable id",
            "PUT",
            &format!("/domains/{domain_id}/layouts/{layout_id}"),
            Some(json!({ "content": content(&["title"]) })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case \"UpdateLayout placing a non-configurable id\": expected 400 ValidationException, \
         got {status} {code}: {message}"
    );
}

#[tokio::test]
async fn a_name_conflict_is_worded_by_the_scope_the_name_is_unique_in() {
    // One rule per scope: a kind named uniquely inside the domain the request
    // addresses reads the requested name back, while a domain — unique per
    // account — is refused with the account the name is already taken in.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "conflict-template-domain").await;

    let rows: Vec<(&str, String, Value, &str)> = vec![
        (
            "domain",
            "/domains".to_string(),
            json!({ "name": "conflict-template-domain" }),
            // The one account a generated emulator serves, which every ARN it
            // mints carries too (`engine::emulator_account_id`).
            "Domain name already exists for account: 000000000000",
        ),
        (
            "field",
            format!("/domains/{domain_id}/fields"),
            json!({ "name": "conflicting-name", "type": "Text" }),
            "Field creation failed: field resource with the name conflicting-name already exists!",
        ),
        (
            "template",
            format!("/domains/{domain_id}/templates"),
            json!({ "name": "conflicting-name" }),
            "Template creation failed: template resource with the name conflicting-name \
             already exists!",
        ),
        (
            "layout",
            format!("/domains/{domain_id}/layouts"),
            json!({ "name": "conflicting-name", "content": { "basic": {} } }),
            "Layout creation failed: layout resource with the name conflicting-name \
             already exists!",
        ),
    ];

    for (kind, path, body, expected) in rows {
        let case = format!("Create a second {kind} under a taken name");
        if kind != "domain" {
            emulator
                .ok(&format!("Create the first {kind}"), "POST", &path, body.clone())
                .await;
        }
        let (status, code, message) = emulator
            .failure_detail(&case, "POST", &path, Some(body.clone()))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code}: {message}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's template {expected:?}, got {message:?}"
        );
    }

    // A RENAME onto a name another live resource of the same kind holds is the
    // same clash, so it renders the same sentence — and still carries no
    // server-minted id.
    let taken = "the-name-that-is-taken";
    for (kind, collection, body, expected) in [
        (
            "field",
            "fields",
            json!({ "name": taken, "type": "Text" }),
            "Field creation failed: field resource with the name the-name-that-is-taken \
             already exists!",
        ),
        (
            "template",
            "templates",
            json!({ "name": taken }),
            "Template creation failed: template resource with the name the-name-that-is-taken \
             already exists!",
        ),
        (
            "layout",
            "layouts",
            json!({ "name": taken, "content": { "basic": {} } }),
            "Layout creation failed: layout resource with the name the-name-that-is-taken \
             already exists!",
        ),
    ] {
        let holder = emulator
            .ok(
                &format!("the {kind} that holds the name"),
                "POST",
                &format!("/domains/{domain_id}/{collection}"),
                body.clone(),
            )
            .await;
        let mut renamed_body = body.clone();
        renamed_body["name"] = json!(format!("the-{kind}-to-rename"));
        let renamed = emulator
            .ok(
                &format!("the {kind} to rename"),
                "POST",
                &format!("/domains/{domain_id}/{collection}"),
                renamed_body,
            )
            .await;
        let id_member = format!("{kind}Id");
        let holder_id = string_at(&holder, &id_member, "the holder");
        let renamed_id = string_at(&renamed, &id_member, "the one to rename");

        let case = format!("Rename a {kind} onto a taken name");
        let (status, code, message) = emulator
            .failure_detail(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/{collection}/{renamed_id}"),
                Some(json!({ "name": taken })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {case:?}: expected 409 ConflictException, got {status} {code}: {message}"
        );
        assert_eq!(
            message, expected,
            "case {case:?}: expected the service's template {expected:?}, got {message:?}"
        );
        assert!(
            !message.contains(&holder_id),
            "case {case:?}: the message must carry no server-minted id, got {message:?}"
        );

        // Renaming a resource to the name it already has is not a clash with
        // itself, so the same call against the holder succeeds.
        emulator
            .ok(
                &format!("Rename the {kind} to the name it already has"),
                "PUT",
                &format!("/domains/{domain_id}/{collection}/{holder_id}"),
                json!({ "name": taken }),
            )
            .await;
    }
}

/// The service's whole reserved `System` field set, as `(fieldId, name,
/// FieldType)`, written out here as the expectation `state::RESERVED_SYSTEM_FIELDS`
/// is checked against.
///
/// Source: a `ListFields` of a domain nothing but `CreateDomain` has touched,
/// which is the only place the set is stated — the model enumerates none of it
/// (`FieldId` is a free-form string, `smithy_query(query="shape",shape="FieldId")`,
/// and `FieldNamespace` names only `Custom | System`). The set covers the
/// entries the issue cited and the entries it did not (`summary`, `title`,
/// `last_updated_user`, `reference_id`, `next_sla_breach_datetime`).
const SYSTEM_FIELD_SET: &[(&str, &str, &str)] = &[
    ("assigned_queue", "Assigned Queue", "Text"),
    ("assigned_user", "Assigned User", "Text"),
    ("case_id", "Case Id", "Text"),
    ("case_reason", "Case Reason", "SingleSelect"),
    ("created_by", "Created By", "User"),
    ("created_datetime", "Date/Time Opened", "DateTime"),
    ("customer_id", "Customer Id", "Text"),
    ("last_closed_datetime", "Date/Time Last Closed", "DateTime"),
    ("last_reopened_datetime", "Date/Time Last Reopened", "DateTime"),
    ("last_updated_user", "Last Updated User", "User"),
    ("last_updated_datetime", "Date/Time Updated", "DateTime"),
    ("next_sla_breach_datetime", "Next SLA Breach Date/Time", "DateTime"),
    ("reference_id", "Reference Id", "Text"),
    ("reference_number", "Reference Number", "Text"),
    ("status", "Status", "SingleSelect"),
    ("summary", "Summary", "Text"),
    ("title", "Title", "Text"),
];

#[tokio::test]
async fn a_new_domain_holds_the_services_whole_system_field_set() {
    // The table is `SYSTEM_FIELD_SET`: every entry of the service's reserved
    // vocabulary is listed for a domain nothing but `CreateDomain` has touched,
    // under its reserved id, the service's own display name and its declared
    // `FieldType`. The rows below run it in two regions, so the ARN each entry
    // is addressable by is derived from the request rather than fixed.
    assert_eq!(
        RESERVED_SYSTEM_FIELDS, SYSTEM_FIELD_SET,
        "case \"the reserved catalog\": the provisioned set must be the service's own \
         vocabulary, got {RESERVED_SYSTEM_FIELDS:?}"
    );

    for region in ["us-east-1", "eu-west-2"] {
        let emulator = Emulator::in_region(region);
        let domain_id = domain(&emulator, &format!("system-field-set-{region}")).await;
        let case = format!("ListFields on a new domain in {region}");
        let listed = listed_fields(&emulator, &case, &domain_id).await;

        assert_eq!(
            listed.len(),
            SYSTEM_FIELD_SET.len(),
            "case {case:?}: a new domain holds exactly the reserved set, got {listed:?}"
        );
        for (field_id, name, field_type) in SYSTEM_FIELD_SET {
            let row = format!("{case}: the reserved field {field_id}");
            let summary = listed
                .iter()
                .find(|entry| entry.get("fieldId").and_then(Value::as_str) == Some(field_id))
                .unwrap_or_else(|| panic!("case {row:?}: {field_id} missing from {listed:?}"));
            assert_eq!(
                (
                    string_at(summary, "name", &row).as_str(),
                    string_at(summary, "type", &row).as_str(),
                    string_at(summary, "namespace", &row).as_str(),
                ),
                (*name, *field_type, FIELD_NAMESPACE_SYSTEM),
                "case {row:?}: expected the reserved name, type and namespace, got {summary:?}"
            );
            assert_eq!(
                string_at(summary, "fieldArn", &row),
                format!(
                    "arn:aws:cases:{region}:000000000000:domain/{domain_id}/field/{field_id}"
                ),
                "case {row:?}: the fieldArn must address the provisioned field in the \
                 request's own region, got {summary:?}"
            );
            assert!(
                summary.get("attributes").is_none(),
                "case {row:?}: a reserved field is provisioned with no field attributes, \
                 got {summary:?}"
            );
        }

        // A field a caller creates is listed BESIDE the reserved set, in its own
        // namespace, so the set is the schema a new domain starts from rather
        // than the whole of what a listing can report.
        let created = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({ "name": "a-custom-field", "type": "Text" }),
            )
            .await;
        let custom_id = string_at(&created, "fieldId", &case);
        let after = listed_fields(&emulator, &case, &domain_id).await;
        assert_eq!(
            after.len(),
            SYSTEM_FIELD_SET.len() + 1,
            "case {case:?}: the created field is listed beside the reserved set, got {after:?}"
        );
        let custom = after
            .iter()
            .find(|entry| entry.get("fieldId").and_then(Value::as_str) == Some(custom_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {custom_id} missing from {after:?}"));
        assert_ne!(
            string_at(custom, "namespace", &case),
            FIELD_NAMESPACE_SYSTEM,
            "case {case:?}: a field a caller created is not service-defined, got {custom:?}"
        );
    }
}

/// One row of the update table: the body one `UpdateField` carries, and what a
/// later read of the field must then report. A `None` expectation means "what
/// the request asked for, else what the field already held".
struct UpdateRow {
    case: &'static str,
    update: Value,
    name: Option<String>,
    description: Option<String>,
    attributes: Option<Value>,
}

#[tokio::test]
async fn an_update_of_a_fields_properties_is_what_every_later_read_reports() {
    // One row per shape of update, each applied to a field of its own: the
    // members the request carries are written and the members it leaves out are
    // preserved, whatever their form — and the write is visible to BOTH readers
    // of a field (`BatchGetField` and `ListFields`), not just to the one the
    // update went through.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-domain").await;

    let original_description = "the description the field was created with";
    let row = |case: &'static str,
               update: Value,
               name: Option<&str>,
               description: Option<&str>,
               attributes: Option<Value>| UpdateRow {
        case,
        update,
        name: name.map(str::to_string),
        description: description.map(str::to_string),
        attributes,
    };
    let rows: Vec<UpdateRow> = vec![
        row(
            "a new name alone",
            json!({ "name": "renamed-by-name-alone" }),
            Some("renamed-by-name-alone"),
            Some(original_description),
            None,
        ),
        row(
            "a new description alone",
            json!({ "description": "the description after the update" }),
            None,
            Some("the description after the update"),
            None,
        ),
        row(
            "a new name and a new description together",
            json!({ "name": "renamed-with-a-description", "description": "both at once" }),
            Some("renamed-with-a-description"),
            Some("both at once"),
            None,
        ),
        row(
            "a name outside ascii",
            json!({ "name": "título del campo" }),
            Some("título del campo"),
            Some(original_description),
            None,
        ),
        row(
            // `FieldName` is `@length 1..100`
            // (`smithy_query(query="shape",shape="FieldName")`).
            "a name at the modelled FieldName maximum of 100",
            json!({ "name": "n".repeat(100) }),
            None,
            Some(original_description),
            None,
        ),
        row(
            // `FieldDescription` is `@length ..255`
            // (`smithy_query(query="shape",shape="FieldDescription")`).
            "a description at the modelled FieldDescription maximum of 255",
            json!({ "description": "d".repeat(255) }),
            None,
            None,
            None,
        ),
        row(
            "a description emptied",
            json!({ "description": "" }),
            None,
            Some(""),
            None,
        ),
        row(
            "field attributes alone",
            json!({ "attributes": { "text": { "isMultiline": true } } }),
            None,
            Some(original_description),
            Some(json!({ "text": { "isMultiline": true } })),
        ),
        row(
            "an update that carries no modifiable member",
            json!({}),
            None,
            Some(original_description),
            None,
        ),
    ];

    for (index, expected) in rows.iter().enumerate() {
        let case = expected.case;
        let update = &expected.update;
        let created = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({
                    "name": format!("field-before-update-{index}"),
                    "type": "Text",
                    "description": original_description,
                }),
            )
            .await;
        let field_id = string_at(&created, "fieldId", case);
        let field_arn = string_at(&created, "fieldArn", case);
        let before = read_field(&emulator, case, &domain_id, &field_id).await;

        emulator
            .ok(
                case,
                "PUT",
                &format!("/domains/{domain_id}/fields/{field_id}"),
                update.clone(),
            )
            .await;

        // What the update asked for, with every unnamed member left as stored:
        // an absent member in the request is "no change", never a reset.
        let expected_name = expected
            .name
            .clone()
            .or_else(|| update.get("name").and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| string_at(&before, "name", case));
        let expected_description = expected
            .description
            .clone()
            .or_else(|| {
                update
                    .get("description")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_else(|| string_at(&before, "description", case));
        let expected_attributes = expected.attributes.clone();

        let read = read_field(&emulator, case, &domain_id, &field_id).await;
        assert_eq!(
            string_at(&read, "name", case),
            expected_name,
            "case {case:?}: BatchGetField must report the updated name, got {read:?}"
        );
        assert_eq!(
            string_at(&read, "description", case),
            expected_description,
            "case {case:?}: BatchGetField must report the updated description, got {read:?}"
        );
        assert_eq!(
            read.get("attributes").cloned(),
            expected_attributes.clone(),
            "case {case:?}: BatchGetField must report the updated field attributes, got {read:?}"
        );
        // The identity of the field is not a property an update may change.
        assert_eq!(
            (
                string_at(&read, "fieldId", case),
                string_at(&read, "fieldArn", case),
                string_at(&read, "type", case),
                string_at(&read, "namespace", case),
            ),
            (
                field_id.clone(),
                field_arn.clone(),
                "Text".to_string(),
                string_at(&before, "namespace", case),
            ),
            "case {case:?}: an update must leave the field's identity alone, got {read:?}"
        );

        // The other reader of a field reports the same stored record: the
        // update is in the state, not in one projection of it.
        let listed = listed_fields(&emulator, case, &domain_id).await;
        let summary = listed
            .iter()
            .find(|entry| entry.get("fieldId").and_then(Value::as_str) == Some(field_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: {field_id} missing from {listed:?}"));
        assert_eq!(
            string_at(summary, "name", case),
            expected_name,
            "case {case:?}: ListFields must report the updated name, got {summary:?}"
        );
        assert_eq!(
            summary.get("attributes").cloned(),
            expected_attributes.clone(),
            "case {case:?}: ListFields must report the updated field attributes, got {summary:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_field_the_domain_cannot_resolve_is_refused() {
    // A field has to exist, and be live, before anything is written to it: an
    // update addressed at a field the domain cannot resolve — one it holds no
    // record for, and one whose record is a deletion tombstone alike — is the
    // modelled 404.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-refusals").await;
    let other_domain = domain(&emulator, "field-update-refusals-elsewhere").await;
    let created = emulator
        .ok(
            "the field to delete",
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({ "name": "the-field-to-delete", "type": "Text" }),
        )
        .await;
    let deleted_id = string_at(&created, "fieldId", "the field to delete");
    emulator
        .ok(
            "DeleteField",
            "DELETE",
            &format!("/domains/{domain_id}/fields/{deleted_id}"),
            json!({}),
        )
        .await;

    // Rows differ in WHICH reference cannot be resolved, and in the form of the
    // update that carries it.
    let rows: Vec<(&str, String, Value, u16, &str)> = vec![
        (
            "a domain the store does not hold",
            format!("/domains/no-such-domain/fields/{deleted_id}"),
            json!({ "name": "a-name" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a field id the domain never minted",
            format!("/domains/{domain_id}/fields/no-such-field"),
            json!({ "name": "a-name" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            // The field is resolved BEFORE the name is judged, so a field the
            // domain does not hold is reported as missing even when the name
            // the request carries is one a live field already holds.
            "a field id the domain never minted, renamed onto a taken name",
            format!("/domains/{domain_id}/fields/no-such-field"),
            json!({ "name": "Title" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a field of another domain's schema",
            format!("/domains/{other_domain}/fields/{deleted_id}"),
            json!({ "description": "a description" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a field that has been deleted",
            format!("/domains/{domain_id}/fields/{deleted_id}"),
            json!({ "name": "a-name-for-a-deleted-field" }),
            404,
            "ResourceNotFoundException",
        ),
    ];

    for (case, path, update, expected_status, expected_code) in rows {
        let (status, code, message) = emulator
            .failure_detail(case, "PUT", &path, Some(update))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code}: \
             {message}"
        );
    }

    // The refusal reports WHICH field could not be resolved through the error's
    // own required members rather than inside its sentence, which carries no
    // variable part at all — asserted through the one body assertion this
    // crate's suites share. `field_update_after_delete.rs` spans that rule over
    // every field type and every form of update.
    refused_as(
        &emulator,
        "the deleted field's refusal",
        "PUT",
        &format!("/domains/{domain_id}/fields/{deleted_id}"),
        Some(json!({ "name": "a-name-for-a-deleted-field" })),
        &Refusal {
            resource_type: "field",
            resource_id: &deleted_id,
            sentence: "Requested resource not found.",
        },
    )
    .await;
}

#[tokio::test]
async fn an_update_that_changes_nothing_leaves_the_stored_record_alone() {
    // `lastModifiedTime` is "Timestamp at which the resource was created or
    // last modified", so it moves when a call modified the field and stays put
    // when it did not — whichever member the request re-sent with the value the
    // field already holds.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-no-op").await;
    let case = "a field with every modifiable member set";
    let created = emulator
        .ok(
            case,
            "POST",
            &format!("/domains/{domain_id}/fields"),
            json!({
                "name": "the-field-that-is-re-sent",
                "type": "Text",
                "description": "the description it already holds",
                "attributes": { "text": { "isMultiline": true } },
            }),
        )
        .await;
    let field_id = string_at(&created, "fieldId", case);
    let stored = read_field(&emulator, case, &domain_id, &field_id).await;

    // Rows differ in which members the no-op update re-sends.
    for (no_op, update) in [
        ("no modifiable member at all", json!({})),
        (
            "the name it already holds",
            json!({ "name": "the-field-that-is-re-sent" }),
        ),
        (
            "the description it already holds",
            json!({ "description": "the description it already holds" }),
        ),
        (
            "the field attributes it already holds",
            json!({ "attributes": { "text": { "isMultiline": true } } }),
        ),
        (
            "every member it already holds at once",
            json!({
                "name": "the-field-that-is-re-sent",
                "description": "the description it already holds",
                "attributes": { "text": { "isMultiline": true } },
            }),
        ),
    ] {
        let case = format!("UpdateField re-sending {no_op}");
        emulator
            .ok(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/fields/{field_id}"),
                update,
            )
            .await;
        let read = read_field(&emulator, &case, &domain_id, &field_id).await;
        assert_eq!(
            read, stored,
            "case {case:?}: an update that changes nothing must leave the whole stored record \
             alone, got {read:?}"
        );
    }

    // And one member really changed moves the modification instant, so the
    // equality above is the no-op rule rather than a reader that never reports
    // a change.
    let case = "UpdateField changing the description";
    emulator
        .ok(
            case,
            "PUT",
            &format!("/domains/{domain_id}/fields/{field_id}"),
            json!({ "description": "a description it did not hold" }),
        )
        .await;
    let read = read_field(&emulator, case, &domain_id, &field_id).await;
    assert_eq!(
        string_at(&read, "description", case),
        "a description it did not hold",
        "case {case:?}: the new description must be reported, got {read:?}"
    );
    assert_ne!(
        read.get("lastModifiedTime"),
        stored.get("lastModifiedTime"),
        "case {case:?}: a real change must move the modification instant, got {read:?}"
    );
    assert_eq!(
        read.get("createdTime"),
        stored.get("createdTime"),
        "case {case:?}: an update must leave the creation instant alone, got {read:?}"
    );
}

#[tokio::test]
async fn a_batch_reads_per_item_message_is_the_services_fixed_sentence() {
    // The sentence is fixed text: the id that failed is reported through the
    // entry's own `id` member, so nothing of the request is interpolated into
    // the message — whatever the form of the unresolvable id, however many of
    // them one call carries, and whichever domain the call addresses.
    assert_eq!(
        BATCH_ITEM_NOT_FOUND_MESSAGE, "This resource cannot be found",
        "case \"the per-item sentence\": the fixed sentence must be the service's own"
    );

    let emulator = Emulator::new();
    let first = domain(&emulator, "fixed-sentence-domain").await;
    let second = domain(&emulator, "fixed-sentence-domain-two").await;
    let created = fields_of_every_type(&emulator, &first).await;
    let resolvable = field_of_type(&created, "Text");

    // Rows differ along the form of the unresolvable id and how many of them a
    // request mixes with ids that do resolve.
    let rows: Vec<(&str, Vec<String>)> = vec![
        ("an id nothing minted", vec!["no-such-field".to_string()]),
        (
            "an id at the modelled FieldId maximum of 500",
            vec!["x".repeat(500)],
        ),
        (
            "an id that only looks minted",
            vec!["00000000-0000-4000-8000-000000000000".to_string()],
        ),
        (
            "an id that differs from a reserved word in case alone",
            vec!["Title".to_string()],
        ),
        (
            "several unresolvable ids at once",
            vec![
                "sim2real-probe-missing-field-0".to_string(),
                "sim2real-probe-missing-field-1".to_string(),
                "sim2real-probe-missing-field-2".to_string(),
            ],
        ),
    ];

    for domain_id in [&first, &second] {
        for (form, unresolvable) in &rows {
            let case = format!("BatchGetField with {form} in {domain_id}");
            let mut requested: Vec<Value> =
                unresolvable.iter().map(|id| json!({ "id": id })).collect();
            if domain_id == &first {
                requested.push(json!({ "id": resolvable }));
            }
            let response = emulator
                .ok(
                    &case,
                    "POST",
                    &format!("/domains/{domain_id}/fields-batch"),
                    json!({ "fields": requested }),
                )
                .await;
            let errors = response
                .get("errors")
                .and_then(Value::as_array)
                .unwrap_or_else(|| panic!("case {case:?}: no errors list in {response:?}"))
                .clone();
            assert_eq!(
                errors.len(),
                unresolvable.len(),
                "case {case:?}: expected {} per-item error(s), got {errors:?}",
                unresolvable.len()
            );
            for (index, id) in unresolvable.iter().enumerate() {
                let error = &errors[index];
                assert_eq!(
                    &string_at(error, "id", &case),
                    id,
                    "case {case:?}: the entry must carry the id that failed, got {error:?}"
                );
                assert_eq!(
                    string_at(error, "errorCode", &case),
                    BATCH_ITEM_NOT_FOUND_CODE,
                    "case {case:?}: expected the per-item code, got {error:?}"
                );
                let message = string_at(error, "message", &case);
                assert_eq!(
                    message, BATCH_ITEM_NOT_FOUND_MESSAGE,
                    "case {case:?}: expected the service's fixed sentence, got {message:?}"
                );
                assert!(
                    !message.contains(id) && !message.contains(domain_id.as_str()),
                    "case {case:?}: the sentence interpolates nothing, got {message:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn the_last_accepted_update_is_what_every_later_batch_read_reports() {
    // An accepted write of a field's properties is durable STATE, not an
    // answer: every later read of that field reports what the last accepted
    // write stored — the read immediately after it, a read that asks for other
    // ids in the same batch, and a read later in the session with other calls
    // in between — and a read stores nothing of its own, so the record it
    // reports is the same one every time.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "field-update-durability").await;

    // Ids the domain holds no record for, mixed into one of the reads below: a
    // batch read answers each of them in its own `errors` entry under the
    // service's fixed sentence and still reports every field it resolved.
    let unresolvable = [
        "no-field-of-this-id".to_string(),
        "00000000-0000-4000-8000-000000000001".to_string(),
    ];

    // One row per chain of accepted writes, each applied to a field of its
    // own: rows differ along how many updates the chain holds, which member
    // each update carries and the form of the description it stores. The last
    // entry of a chain is what every read below must report.
    let row = |case: &'static str, created_with: &str, updates: Vec<Value>, expected: &str| {
        (
            case,
            created_with.to_string(),
            updates,
            expected.to_string(),
        )
    };
    let rows: Vec<(&str, String, Vec<Value>, String)> = vec![
        row(
            "one update to a new description",
            "the description at create",
            vec![json!({ "description": "the description after one update" })],
            "the description after one update",
        ),
        row(
            "two updates, where the last one is what the field holds",
            "the description at create",
            vec![
                json!({ "description": "the description after the first update" }),
                json!({ "description": "the description after the second update" }),
            ],
            "the description after the second update",
        ),
        row(
            "a description change followed by a rename that names no description",
            "the description at create",
            vec![
                json!({ "description": "the description a rename must not revert" }),
                json!({ "name": "renamed-after-its-description-changed" }),
            ],
            "the description a rename must not revert",
        ),
        row(
            "a description emptied and then set again",
            "the description at create",
            vec![
                json!({ "description": "" }),
                json!({ "description": "the description set after it was emptied" }),
            ],
            "the description set after it was emptied",
        ),
        row(
            "an update re-sending the description the field already holds",
            "the description at create",
            vec![json!({ "description": "the description at create" })],
            "the description at create",
        ),
        row(
            "a description outside ascii",
            "the description at create",
            vec![json!({ "description": "descripción del campo" })],
            "descripción del campo",
        ),
        row(
            // `FieldDescription` is `@length ..255`
            // (`smithy_query(query="shape",shape="FieldDescription")`).
            "a description at the modelled FieldDescription maximum of 255",
            "the description at create",
            vec![json!({ "description": "d".repeat(255) })],
            &"d".repeat(255),
        ),
    ];

    for (index, (case, created_with, updates, expected)) in rows.iter().enumerate() {
        let created = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({
                    "name": format!("field-of-the-durability-row-{index}"),
                    "type": "Text",
                    "description": created_with,
                }),
            )
            .await;
        let field_id = string_at(&created, "fieldId", case);

        for update in updates {
            let answer = emulator
                .ok(
                    case,
                    "PUT",
                    &format!("/domains/{domain_id}/fields/{field_id}"),
                    update.clone(),
                )
                .await;
            // `UpdateFieldResponse` declares no members, so the documented
            // success carries the empty structure and nothing else.
            assert_eq!(
                answer,
                json!({}),
                "case {case:?}: UpdateField answers the empty structure, got {answer:?}"
            );
        }

        // The read IMMEDIATELY after the chain.
        let first = read_field(&emulator, case, &domain_id, &field_id).await;
        assert_eq!(
            &string_at(&first, "description", case),
            expected,
            "case {case:?}: the first read after the update must report what it stored, \
             got {first:?}"
        );

        // The same read, mixed with ids the domain cannot resolve: the
        // resolved field carries the same stored record, and one unresolvable
        // id neither fails the call nor hides the rest.
        let mixed = vec![
            unresolvable[0].clone(),
            field_id.clone(),
            unresolvable[1].clone(),
        ];
        let (resolved, errors) = batch_get_fields(&emulator, case, &domain_id, &mixed).await;
        assert_eq!(
            resolved,
            vec![first.clone()],
            "case {case:?}: a mixed batch must report the stored record, got {resolved:?}"
        );
        assert_eq!(
            errors.len(),
            unresolvable.len(),
            "case {case:?}: expected one error entry per unresolvable id, got {errors:?}"
        );
        for id in &unresolvable {
            let entry = errors
                .iter()
                .find(|entry| entry.get("id").and_then(Value::as_str) == Some(id.as_str()))
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no errors entry for {id:?} in {errors:?}")
                });
            assert_eq!(
                string_at(entry, "errorCode", case),
                BATCH_ITEM_NOT_FOUND_CODE,
                "case {case:?}: expected the per-item code, got {entry:?}"
            );
            let message = string_at(entry, "message", case);
            assert_eq!(
                message, BATCH_ITEM_NOT_FOUND_MESSAGE,
                "case {case:?}: expected the service's fixed sentence, got {message:?}"
            );
            assert!(
                !message.contains(id.as_str()) && !message.contains(domain_id.as_str()),
                "case {case:?}: the sentence interpolates nothing, got {message:?}"
            );
        }

        // A LATER read, with other calls of the same session in between:
        // another field's create and a listing of the domain leave the record
        // the chain stored alone, and the field asked for twice in one batch
        // is answered once per occurrence with that same record.
        emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({
                    "name": format!("unrelated-field-of-the-durability-row-{index}"),
                    "type": "Text",
                    "description": "the description of another field entirely",
                }),
            )
            .await;
        let listed = listed_fields(&emulator, case, &domain_id).await;
        assert!(
            listed
                .iter()
                .any(|entry| entry.get("fieldId").and_then(Value::as_str)
                    == Some(field_id.as_str())),
            "case {case:?}: the updated field must still be listed, got {listed:?}"
        );
        let twice = vec![field_id.clone(), field_id.clone()];
        let (later, later_errors) = batch_get_fields(&emulator, case, &domain_id, &twice).await;
        assert!(
            later_errors.is_empty(),
            "case {case:?}: a resolvable id reports no error, got {later_errors:?}"
        );
        assert_eq!(
            later,
            vec![first.clone(), first.clone()],
            "case {case:?}: every later read must report the record the last accepted update \
             stored, got {later:?}"
        );
    }
}
