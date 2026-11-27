// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for the Cases lifecycle
//! (domain -> field -> template -> case), driven through the real
//! `MultiServiceRouter` so every assertion goes over the wire the SDK speaks.
//!
//! The rule under test: every id a request carries is resolved against persisted
//! state before anything is written — an unknown domain, case or template is
//! refused with `ResourceNotFoundException` (404) and an unknown field id with
//! `ValidationException` (400) — and every successful response is computed from
//! the stored record, so a create's ids are the ids later reads accept and a
//! read hands back the field values that were last written, in the union variant
//! they were written with.

use std::collections::HashMap;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    MAX_BATCH_FIELD_IDS, MAX_CASE_FIELD_IDS, MAX_DOMAINS,
};

// The request plumbing and the lifecycle helpers live in `tests/all/harness.rs`,
// shared with this crate's other suites.
use crate::harness::{
    domain, field_of_type, field_value_variants, fields_of_every_type, get_case_fields,
    is_minted_id, string_at, template, template_required_field_refusal, title_value, Emulator,
    BODY_REJECTION_CODE, INVALID_REQUEST_BODY,
};

#[tokio::test]
async fn the_domain_field_template_case_lifecycle_round_trips_through_state() {
    let emulator = Emulator::new();

    // 1. CreateDomain mints and stores a domain, Active, with an addressable ARN.
    let created = emulator
        .ok(
            "CreateDomain",
            "POST",
            "/domains",
            json!({ "name": "lifecycle-domain" }),
        )
        .await;
    let domain_id = string_at(&created, "domainId", "CreateDomain");
    assert!(
        is_minted_id(&domain_id),
        "case \"CreateDomain\": the domainId must be a server-minted opaque id, got {domain_id:?}"
    );
    assert_eq!(
        string_at(&created, "domainStatus", "CreateDomain"),
        "Active",
        "case \"CreateDomain\": a created domain serves requests, got {created:?}"
    );
    assert_eq!(
        string_at(&created, "domainArn", "CreateDomain"),
        format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}"),
        "case \"CreateDomain\": domainArn must address the stored domain, got {created:?}"
    );

    // GetDomain reads back exactly what CreateDomain stored.
    let fetched = emulator
        .ok(
            "GetDomain",
            "POST",
            &format!("/domains/{domain_id}"),
            json!({}),
        )
        .await;
    for (member, expected) in [
        ("domainId", domain_id.clone()),
        (
            "domainArn",
            string_at(&created, "domainArn", "CreateDomain"),
        ),
        ("name", "lifecycle-domain".to_string()),
        ("domainStatus", "Active".to_string()),
    ] {
        assert_eq!(
            string_at(&fetched, member, "GetDomain"),
            expected,
            "case \"GetDomain\": {member} must come from the stored domain, got {fetched:?}"
        );
    }
    assert!(
        fetched.get("createdTime").is_some(),
        "case \"GetDomain\": the stored createdTime must be reported, got {fetched:?}"
    );

    // 2. CreateField stores one field per modeled FieldType.
    let created_fields = fields_of_every_type(&emulator, &domain_id).await;
    let field_ids: Vec<String> = created_fields.iter().map(|(_, id)| id.clone()).collect();

    // BatchGetField reads every stored field back, and reports only the id that
    // was never created as an error.
    let mut requested: Vec<Value> = field_ids.iter().map(|id| json!({ "id": id })).collect();
    requested.push(json!({ "id": "field-that-was-never-created" }));
    let batch = emulator
        .ok(
            "BatchGetField",
            "POST",
            &format!("/domains/{domain_id}/fields-batch"),
            json!({ "fields": requested }),
        )
        .await;
    let read_back = batch
        .get("fields")
        .and_then(Value::as_array)
        .expect("case \"BatchGetField\": a fields list")
        .clone();
    assert_eq!(
        read_back.len(),
        created_fields.len(),
        "case \"BatchGetField\": expected every created field back, got {batch:?}"
    );
    let by_id: HashMap<String, Value> = read_back
        .iter()
        .map(|field| (string_at(field, "fieldId", "BatchGetField"), field.clone()))
        .collect();
    for (field_type, field_id) in &created_fields {
        let case = format!("BatchGetField type={field_type}");
        let field = by_id
            .get(field_id)
            .unwrap_or_else(|| panic!("case {case:?}: field {field_id} missing from {batch:?}"));
        assert_eq!(
            string_at(field, "type", &case),
            *field_type,
            "case {case:?}: the stored field type must be reported, got {field:?}"
        );
        assert_eq!(
            string_at(field, "namespace", &case),
            "Custom",
            "case {case:?}: a field created through the API is Custom, got {field:?}"
        );
        assert_eq!(
            field.get("deleted").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a live field is not deleted, got {field:?}"
        );
    }
    let errors = batch
        .get("errors")
        .and_then(Value::as_array)
        .expect("case \"BatchGetField\": an errors list")
        .clone();
    assert_eq!(
        errors.len(),
        1,
        "case \"BatchGetField\": only the uncreated id must fail, got {batch:?}"
    );
    assert_eq!(
        string_at(&errors[0], "id", "BatchGetField"),
        "field-that-was-never-created",
        "case \"BatchGetField\": the error must name the unknown id, got {errors:?}"
    );
    assert_eq!(
        string_at(&errors[0], "errorCode", "BatchGetField"),
        "ResourceNotFound",
        "case \"BatchGetField\": an unknown id is the per-item not-found code, got {errors:?}"
    );

    // 3. CreateTemplate stores the template and its requiredFields. The model
    //    makes requiredFields the fields "that must have a value provided to
    //    create a case", so this template requires exactly the fields step 4
    //    writes.
    let variants = field_value_variants();
    let written: Vec<(String, Value)> = variants
        .iter()
        .map(|(_, field_type, value)| {
            (field_of_type(&created_fields, field_type), value.clone())
        })
        .collect();
    let required_ids: Vec<String> = written.iter().map(|(id, _)| id.clone()).collect();
    let template_id = template(&emulator, &domain_id, "lifecycle-template", &required_ids).await;
    let template_read = emulator
        .ok(
            "GetTemplate",
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({}),
        )
        .await;
    assert_eq!(
        string_at(&template_read, "templateId", "GetTemplate"),
        template_id,
        "case \"GetTemplate\": the id CreateTemplate minted must be the id read back, got {template_read:?}"
    );
    assert_eq!(
        string_at(&template_read, "templateArn", "GetTemplate"),
        format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/template/{template_id}"),
        "case \"GetTemplate\": templateArn must address the stored template, got {template_read:?}"
    );
    assert_eq!(
        string_at(&template_read, "status", "GetTemplate"),
        "Active",
        "case \"GetTemplate\": a template given no status defaults to Active, got {template_read:?}"
    );
    assert_eq!(
        template_read.get("deleted").and_then(Value::as_bool),
        Some(false),
        "case \"GetTemplate\": a live template is not deleted, got {template_read:?}"
    );
    let required_read: Vec<String> = template_read
        .get("requiredFields")
        .and_then(Value::as_array)
        .expect("case \"GetTemplate\": a requiredFields list")
        .iter()
        .map(|entry| string_at(entry, "fieldId", "GetTemplate"))
        .collect();
    assert_eq!(
        required_read, required_ids,
        "case \"GetTemplate\": the stored requiredFields must be reported, got {template_read:?}"
    );

    // 4. CreateCase persists one value per FieldValueUnion variant, each
    //    written against a field of the type that variant carries — beside the
    //    `title` the service requires of every create.
    let mut case_fields: Vec<Value> = vec![title_value("the lifecycle case")];
    case_fields.extend(
        written
            .iter()
            .map(|(id, value)| json!({ "id": id, "value": value })),
    );
    let case_created = emulator
        .ok(
            "CreateCase",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": case_fields,
                "tags": { "team": "support" },
            }),
        )
        .await;
    let case_id = string_at(&case_created, "caseId", "CreateCase");
    assert_eq!(
        string_at(&case_created, "caseArn", "CreateCase"),
        format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case/{case_id}"),
        "case \"CreateCase\": caseArn must address the stored case, got {case_created:?}"
    );

    /// The entries a read must answer for `requested`: one per requested id the
    /// case holds a value for, in the requested order and carrying the value
    /// that was written. A field the case was never written against is reported
    /// by its ABSENCE, so it contributes no entry.
    fn expected_entries(written: &[(String, Value)], requested: &[String]) -> Vec<Value> {
        requested
            .iter()
            .filter_map(|field_id| {
                written
                    .iter()
                    .find(|(id, _)| id == field_id)
                    .map(|(_, value)| json!({ "id": field_id, "value": value }))
            })
            .collect()
    }

    // 5. GetCase answers the requested ids the case holds a value for, in the
    //    order they were asked for, and nothing for the ones it does not.
    let read = get_case_fields(&emulator, "GetCase", &domain_id, &case_id, &field_ids).await;
    let expected = expected_entries(&written, &field_ids);
    assert_eq!(
        read, expected,
        "case \"GetCase\": expected {expected:?}, got {read:?}"
    );
    assert!(
        read.len() < field_ids.len(),
        "case \"GetCase\": the table must request at least one field the case never wrote, \
         got {read:?}"
    );
    let with_tags = emulator
        .ok(
            "GetCase tags",
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            json!({ "fields": [{ "id": field_ids[0] }] }),
        )
        .await;
    assert_eq!(
        string_at(&with_tags, "templateId", "GetCase tags"),
        template_id,
        "case \"GetCase tags\": the case's stored templateId must be reported, got {with_tags:?}"
    );
    assert_eq!(
        with_tags.get("tags"),
        Some(&json!({ "team": "support" })),
        "case \"GetCase tags\": the stored tags must be reported, got {with_tags:?}"
    );

    // 6. UpdateCase MERGES: the supplied id is overwritten, the rest untouched.
    let text_field = field_of_type(&created_fields, "Text");
    let updated_value = json!({ "stringValue": "the note, updated" });
    let update = emulator
        .send(
            "PUT",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({
                "fields": [{ "id": text_field, "value": updated_value }],
            })),
        )
        .await;
    assert_eq!(
        update.0, 200,
        "case \"UpdateCase\": expected HTTP 200, got {update:?}"
    );
    let merged: Vec<(String, Value)> = written
        .iter()
        .map(|(id, value)| {
            if *id == text_field {
                (id.clone(), updated_value.clone())
            } else {
                (id.clone(), value.clone())
            }
        })
        .collect();
    let after =
        get_case_fields(&emulator, "GetCase after update", &domain_id, &case_id, &field_ids).await;
    let expected = expected_entries(&merged, &field_ids);
    assert_eq!(
        after, expected,
        "case \"GetCase after update\": the merge must overwrite only the supplied id; \
         expected {expected:?}, got {after:?}"
    );

    // 7. DeleteCase removes the case, so the next read is the modeled 404.
    let deleted = emulator
        .send(
            "DELETE",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            None,
        )
        .await;
    assert_eq!(
        deleted.0, 200,
        "case \"DeleteCase\": expected HTTP 200, got {deleted:?}"
    );
    let (status, code) = emulator
        .failure(
            "GetCase after delete",
            "POST",
            &format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({ "fields": [{ "id": field_ids[0] }] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"GetCase after delete\": a deleted case is gone, got {status} / {code}"
    );
}

#[tokio::test]
async fn create_case_is_idempotent_per_client_token() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "idempotency-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let field_id = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "idempotency-template",
        std::slice::from_ref(&field_id),
    )
    .await;

    let create = |token: Option<&'static str>, note: &'static str| {
        let domain_id = domain_id.clone();
        let template_id = template_id.clone();
        let field_id = field_id.clone();
        let emulator = &emulator;
        async move {
            let mut body = json!({
                "templateId": template_id,
                "fields": [
                    title_value("an idempotent case"),
                    { "id": field_id, "value": { "stringValue": note } },
                ],
            });
            if let Some(token) = token {
                body["clientToken"] = json!(token);
            }
            let case = format!("CreateCase clientToken={token:?}");
            let response = emulator
                .ok(&case, "POST", &format!("/domains/{domain_id}/cases"), body)
                .await;
            string_at(&response, "caseId", &case)
        }
    };

    let first = create(Some("tok-1"), "first").await;
    let replay = create(Some("tok-1"), "first").await;
    let other_token = create(Some("tok-2"), "second").await;
    let untokened_a = create(None, "third").await;
    let untokened_b = create(None, "fourth").await;

    assert_eq!(
        first, replay,
        "case \"CreateCase replay\": the same clientToken must resolve to the same case, \
         got {first} then {replay}"
    );
    assert_ne!(
        first, other_token,
        "case \"CreateCase other token\": a different clientToken must mint a new case, \
         got {other_token}"
    );
    assert_ne!(
        untokened_a, untokened_b,
        "case \"CreateCase no token\": without a clientToken every call mints a case, \
         got {untokened_a} twice"
    );

    // The replay resolved to a stored case, so the case the token names is the
    // one a read finds — and once it is deleted the token mints a fresh case
    // rather than naming a case that no longer exists.
    let read = get_case_fields(
        &emulator,
        "GetCase replayed",
        &domain_id,
        &replay,
        std::slice::from_ref(&field_id),
    )
    .await;
    assert_eq!(
        read[0].get("value"),
        Some(&json!({ "stringValue": "first" })),
        "case \"GetCase replayed\": the replay must name the first case, got {read:?}"
    );
    let deleted = emulator
        .send("DELETE", &format!("/domains/{domain_id}/cases/{first}"), None)
        .await;
    assert_eq!(
        deleted.0, 200,
        "case \"DeleteCase replayed\": expected HTTP 200, got {deleted:?}"
    );
    let after_delete = create(Some("tok-1"), "fifth").await;
    assert_ne!(
        after_delete, first,
        "case \"CreateCase after delete\": a token must not resolve to a deleted case, \
         got {after_delete}"
    );
}

#[tokio::test]
async fn an_id_the_domain_does_not_hold_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "reference-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let field_id = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "reference-template",
        std::slice::from_ref(&field_id),
    )
    .await;
    let case_created = emulator
        .ok(
            "CreateCase for reference table",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [
                    title_value("the reference case"),
                    { "id": field_id, "value": { "stringValue": "x" } },
                ],
            }),
        )
        .await;
    let case_id = string_at(&case_created, "caseId", "CreateCase for reference table");

    /// One row of the reference table: case name, HTTP method, path, body,
    /// expected status and expected wire error code.
    type Row = (&'static str, &'static str, String, Option<Value>, u16, &'static str);

    let value = json!({ "stringValue": "x" });
    // Every way a request can name something the store does not hold: an unknown
    // resource is 404, an unknown field reference inside the body is 400.
    let rows: Vec<Row> = vec![
        (
            "GetDomain unknown domain",
            "POST",
            "/domains/no-such-domain".to_string(),
            Some(json!({})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateField unknown domain",
            "POST",
            "/domains/no-such-domain/fields".to_string(),
            Some(json!({ "name": "f", "type": "Text" })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "BatchGetField unknown domain",
            "POST",
            "/domains/no-such-domain/fields-batch".to_string(),
            Some(json!({ "fields": [{ "id": field_id }] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateTemplate unknown domain",
            "POST",
            "/domains/no-such-domain/templates".to_string(),
            Some(json!({ "name": "t" })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetTemplate unknown template",
            "POST",
            format!("/domains/{domain_id}/templates/no-such-template"),
            Some(json!({})),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateCase unknown domain",
            "POST",
            "/domains/no-such-domain/cases".to_string(),
            Some(json!({ "templateId": template_id, "fields": [] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateCase unknown template",
            "POST",
            format!("/domains/{domain_id}/cases"),
            Some(json!({ "templateId": "no-such-template", "fields": [] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetCase unknown case",
            "POST",
            format!("/domains/{domain_id}/cases/no-such-case"),
            Some(json!({ "fields": [{ "id": field_id }] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "GetCase unknown domain",
            "POST",
            format!("/domains/no-such-domain/cases/{case_id}"),
            Some(json!({ "fields": [{ "id": field_id }] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "UpdateCase unknown case",
            "PUT",
            format!("/domains/{domain_id}/cases/no-such-case"),
            Some(json!({ "fields": [{ "id": field_id, "value": value }] })),
            404,
            "ResourceNotFoundException",
        ),
        (
            "DeleteCase unknown case",
            "DELETE",
            format!("/domains/{domain_id}/cases/no-such-case"),
            None,
            404,
            "ResourceNotFoundException",
        ),
        (
            "CreateTemplate unknown required field",
            "POST",
            format!("/domains/{domain_id}/templates"),
            Some(json!({
                "name": "template-with-a-dangling-field",
                "requiredFields": [{ "fieldId": "no-such-field" }],
            })),
            400,
            "ValidationException",
        ),
        (
            "CreateCase unknown field",
            "POST",
            format!("/domains/{domain_id}/cases"),
            Some(json!({
                "templateId": template_id,
                "fields": [{ "id": "no-such-field", "value": value }],
            })),
            400,
            "ValidationException",
        ),
        (
            "UpdateCase unknown field",
            "PUT",
            format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({ "fields": [{ "id": "no-such-field", "value": value }] })),
            400,
            "ValidationException",
        ),
        (
            "GetCase unknown field",
            "POST",
            format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({ "fields": [{ "id": "no-such-field" }] })),
            400,
            "ValidationException",
        ),
        (
            "CreateCase duplicate field id",
            "POST",
            format!("/domains/{domain_id}/cases"),
            Some(json!({
                "templateId": template_id,
                "fields": [
                    { "id": field_id, "value": value },
                    { "id": field_id, "value": value },
                ],
            })),
            400,
            "ValidationException",
        ),
        (
            "CreateTemplate rule naming a case rule the domain has none of",
            "POST",
            format!("/domains/{domain_id}/templates"),
            Some(json!({
                "name": "template-with-a-dangling-rule",
                "rules": [{ "caseRuleId": "no-such-case-rule" }],
            })),
            400,
            "ValidationException",
        ),
        (
            "CreateTemplate defaultLayout naming a layout the domain has none of",
            "POST",
            format!("/domains/{domain_id}/templates"),
            Some(json!({
                "name": "template-with-a-dangling-layout",
                "layoutConfiguration": { "defaultLayout": "no-such-layout" },
            })),
            400,
            "ValidationException",
        ),
        (
            "UpdateCase duplicate field id",
            "PUT",
            format!("/domains/{domain_id}/cases/{case_id}"),
            Some(json!({
                "fields": [
                    { "id": field_id, "value": value },
                    { "id": field_id, "value": value },
                ],
            })),
            400,
            "ValidationException",
        ),
    ];

    for (case, method, path, body, expected_status, expected_code) in rows {
        let (status, code) = emulator.failure(case, method, &path, body).await;
        assert_eq!(
            (status, code.as_str()),
            (expected_status, expected_code),
            "case {case:?}: expected {expected_status} {expected_code}, got {status} {code}"
        );
    }

    // Nothing above was written: the case still holds only what it was created with.
    let read = get_case_fields(
        &emulator,
        "GetCase after refusals",
        &domain_id,
        &case_id,
        std::slice::from_ref(&field_id),
    )
    .await;
    assert_eq!(
        read[0].get("value"),
        Some(&json!({ "stringValue": "x" })),
        "case \"GetCase after refusals\": a refused write must leave the case alone, got {read:?}"
    );
}

#[tokio::test]
async fn a_client_token_is_remembered_per_domain() {
    // The idempotency token identifies one request, and a request against
    // another domain is a different request: the same token must not make a
    // create in domain B answer with domain A's case, nor stop domain A's
    // replay from resolving.
    let emulator = Emulator::new();
    /// What one domain of the table below contributes: its id, the field the
    /// case was written against, and the case the shared token minted there.
    type Scope = (String, String, String);
    let mut scopes: Vec<Scope> = Vec::new();
    for name in ["token-scope-a", "token-scope-b"] {
        let domain_id = domain(&emulator, name).await;
        let fields = fields_of_every_type(&emulator, &domain_id).await;
        let field_id = field_of_type(&fields, "Text");
        let template_id = template(
            &emulator,
            &domain_id,
            "token-scope-template",
            std::slice::from_ref(&field_id),
        )
        .await;
        let case = format!("CreateCase in {name}");
        let created = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                json!({
                    "templateId": template_id,
                    "fields": [
                        title_value("a token-scoped case"),
                        { "id": field_id, "value": { "stringValue": name } },
                    ],
                    "clientToken": "the-same-token",
                }),
            )
            .await;
        let case_id = string_at(&created, "caseId", &case);
        scopes.push((domain_id, field_id, case_id));
    }

    assert_ne!(
        scopes[0].2, scopes[1].2,
        "case \"CreateCase same token in another domain\": each domain must mint its own case, \
         got {} twice",
        scopes[0].2
    );

    // Each domain's case holds what that domain wrote, and each domain's replay
    // resolves to its own case.
    for (index, (domain_id, field_id, case_id)) in scopes.iter().enumerate() {
        let case = format!("token scope #{index}");
        let read = get_case_fields(
            &emulator,
            &case,
            domain_id,
            case_id,
            std::slice::from_ref(field_id),
        )
        .await;
        let expected = if index == 0 { "token-scope-a" } else { "token-scope-b" };
        assert_eq!(
            read[0].get("value"),
            Some(&json!({ "stringValue": expected })),
            "case {case:?}: the case must hold the value written in its own domain, got {read:?}"
        );

        let replay = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                json!({
                    "templateId": "ignored-on-replay",
                    "fields": [],
                    "clientToken": "the-same-token",
                }),
            )
            .await;
        assert_eq!(
            string_at(&replay, "caseId", &case),
            *case_id,
            "case {case:?}: a replay must resolve to that domain's case, got {replay:?}"
        );
    }
}

#[tokio::test]
async fn the_store_refuses_to_grow_past_its_ceiling() {
    // Every map in the store is request-driven, so each carries a ceiling; past
    // it a create is refused with the modelled quota error instead of growing.
    // The domain ceiling stands for all of them (it is the smallest).
    let emulator = Emulator::new();
    for index in 0..MAX_DOMAINS {
        let case = format!("CreateDomain #{index}");
        emulator
            .ok(
                &case,
                "POST",
                "/domains",
                json!({ "name": format!("ceiling-domain-{index}") }),
            )
            .await;
    }
    let (status, code) = emulator
        .failure(
            "CreateDomain past the ceiling",
            "POST",
            "/domains",
            Some(json!({ "name": "one-domain-too-many" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (402, "ServiceQuotaExceededException"),
        "case \"CreateDomain past the ceiling\": expected 402 ServiceQuotaExceededException \
         once the store holds {MAX_DOMAINS} domains, got {status} {code}"
    );
}

#[tokio::test]
async fn a_field_id_list_is_bounded_and_a_repeat_is_refused_only_on_a_write() {
    // Every operation that takes a caller-chosen field-id list answers one
    // entry per id, so the list is held to the cap its member declares. A
    // repeated id is refused only where it is ambiguous — on a write, one id
    // carrying two values — and answered once per occurrence on a read, which
    // the cap already bounds.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "bounded-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let field_id = field_of_type(&fields, "Text");
    let template_id = template(
        &emulator,
        &domain_id,
        "bounded-template",
        std::slice::from_ref(&field_id),
    )
    .await;
    let created = emulator
        .ok(
            "CreateCase for the bounds table",
            "POST",
            &format!("/domains/{domain_id}/cases"),
            json!({
                "templateId": template_id,
                "fields": [
                    title_value("the bounds-table case"),
                    { "id": field_id, "value": { "stringValue": "x" } },
                ],
            }),
        )
        .await;
    let case_id = string_at(&created, "caseId", "CreateCase for the bounds table");

    /// Builds the request body around a field-id list, which differs per
    /// operation (identifiers vs values, and CreateCase's `templateId`).
    type BodyFor = Box<dyn Fn(&[String]) -> Value>;
    let identifiers: BodyFor = Box::new(|ids: &[String]| {
        json!({ "fields": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>() })
    });
    let values = |template_id: Option<String>| -> BodyFor {
        Box::new(move |ids: &[String]| {
            let entries: Vec<Value> = ids
                .iter()
                .map(|id| json!({ "id": id, "value": { "stringValue": "x" } }))
                .collect();
            match &template_id {
                Some(template_id) => json!({ "templateId": template_id, "fields": entries }),
                None => json!({ "fields": entries }),
            }
        })
    };

    // The cap on each row is the one the model puts on that member:
    // GetCaseRequest$fields 1..220, CreateCaseRequest$fields ..220,
    // UpdateCaseRequest$fields ..220, BatchGetFieldRequest$fields 1..50.
    // The last column is whether a repeated id is refused: only on the WRITE
    // paths, where one id carrying two values has no defined meaning. A read
    // answers one entry per occurrence, bounded by the same cap.
    let rows: Vec<(&str, &str, String, usize, BodyFor, bool)> = vec![
        (
            "GetCase",
            "POST",
            format!("/domains/{domain_id}/cases/{case_id}"),
            MAX_CASE_FIELD_IDS,
            Box::new(|ids: &[String]| {
                json!({ "fields": ids.iter().map(|id| json!({ "id": id })).collect::<Vec<_>>() })
            }),
            false,
        ),
        (
            "BatchGetField",
            "POST",
            format!("/domains/{domain_id}/fields-batch"),
            MAX_BATCH_FIELD_IDS,
            identifiers,
            false,
        ),
        (
            "CreateCase",
            "POST",
            format!("/domains/{domain_id}/cases"),
            MAX_CASE_FIELD_IDS,
            values(Some(template_id.clone())),
            true,
        ),
        (
            "UpdateCase",
            "PUT",
            format!("/domains/{domain_id}/cases/{case_id}"),
            MAX_CASE_FIELD_IDS,
            values(None),
            true,
        ),
    ];

    for (operation, method, path, cap, body_for, refuses_repeat) in rows {
        // One past the cap is refused. For an operation the body-validating layer fronts
        // that is the body rejection it states for every modelled constraint a body breaks
        // — one fixed sentence, naming neither the member nor the cap — while `CreateCase`,
        // the one operation it does not front, is refused behind the door, naming the
        // member (see `tests/all/request_body_rejection.rs`).
        let fronted = operation != "CreateCase";
        let over: Vec<String> = (0..=cap).map(|index| format!("id-{index}")).collect();
        let case = format!("{operation} with {} field ids", over.len());
        let (status, code, message) = emulator
            .failure_detail(&case, method, &path, Some(body_for(&over)))
            .await;
        let expected_code = if fronted {
            BODY_REJECTION_CODE
        } else {
            "ValidationException"
        };
        assert_eq!(
            (status, code.as_str()),
            (400, expected_code),
            "case {case:?}: expected 400 {expected_code} past the cap of {cap}, \
             got {status} {code}: {message}"
        );
        if fronted {
            assert_eq!(
                message, INVALID_REQUEST_BODY,
                "case {case:?}: past the cap of {cap} the body rejection is the fixed \
                 sentence, got {message:?}"
            );
        } else {
            assert!(
                message.contains("fields") && message.contains(&cap.to_string()),
                "case {case:?}: past the cap of {cap} the refusal names the member and the \
                 cap, got {message:?}"
            );
        }

        // Exactly the cap is within bounds, so the refusal above is the cap and
        // not an off-by-one: this one may still fail on the ids themselves, but
        // never on the list's length.
        let at_cap: Vec<String> = (0..cap).map(|index| format!("id-{index}")).collect();
        let case = format!("{operation} with exactly {cap} field ids");
        let (status, json) = emulator.send(method, &path, Some(body_for(&at_cap))).await;
        let message = json
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        assert!(
            !message.contains("at most"),
            "case {case:?}: a list at the cap must not be refused for its length, \
             got {status} with {message:?}"
        );

        // A repeated id: refused on a write, naming the id it saw twice;
        // answered once per occurrence on a read.
        let repeated = vec![field_id.clone(), field_id.clone()];
        let case = format!("{operation} with the same field id twice");
        if refuses_repeat {
            let (status, code, message) = emulator
                .failure_detail(&case, method, &path, Some(body_for(&repeated)))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
            );
            assert!(
                message.contains(&field_id),
                "case {case:?}: the refusal must name the repeated id {field_id}, got {message:?}"
            );
        } else {
            let answered = emulator.ok(&case, method, &path, body_for(&repeated)).await;
            let entries = answered
                .get("fields")
                .and_then(Value::as_array)
                .map(Vec::len)
                .unwrap_or_default();
            assert_eq!(
                entries, 2,
                "case {case:?}: a read answers one entry per requested occurrence, \
                 got {answered:?}"
            );
        }
    }

    // The same rule on the other list of field ids the service takes.
    let case = "CreateTemplate with the same required field twice";
    let (status, code, message) = emulator
        .failure_detail(
            case,
            "POST",
            &format!("/domains/{domain_id}/templates"),
            Some(json!({
                "name": "template-with-a-repeated-field",
                "requiredFields": [{ "fieldId": field_id }, { "fieldId": field_id }],
            })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, "ValidationException"),
        "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
    );
    assert!(
        message.contains(&field_id),
        "case {case:?}: the refusal must name the repeated id {field_id}, got {message:?}"
    );
}

#[tokio::test]
async fn a_field_value_must_be_a_value_of_its_field_s_declared_type() {
    // A case field's value has the field's own declared type, so the union
    // variant and the FieldType have to agree. One row per FieldType the model
    // declares, each written with a variant that is NOT a value of that type.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "typed-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let template_id = template(
        &emulator,
        &domain_id,
        "typed-template",
        // No required fields: each row below writes exactly one field, so the
        // only rule under test is the value's type.
        &[],
    )
    .await;

    // The variant each FieldType accepts, and one it must refuse. Source: the
    // model's `FieldValueUnion` and `FieldType` shapes — a Boolean field holds
    // `booleanValue`, a Number field `doubleValue`, a User field
    // `userArnValue`, and DateTime/SingleSelect/Text/Url hold `stringValue`.
    let rows: Vec<(&str, Value, Value)> = vec![
        (
            "Boolean",
            json!({ "booleanValue": false }),
            json!({ "stringValue": "not a boolean" }),
        ),
        (
            "DateTime",
            json!({ "stringValue": "2026-10-21T21:58:09Z" }),
            json!({ "doubleValue": 1.0 }),
        ),
        (
            "Number",
            json!({ "doubleValue": 7.0 }),
            json!({ "booleanValue": true }),
        ),
        (
            "SingleSelect",
            json!({ "stringValue": "an-option" }),
            json!({ "doubleValue": 2.0 }),
        ),
        (
            "Text",
            json!({ "stringValue": "free text" }),
            json!({ "booleanValue": true }),
        ),
        (
            "Url",
            json!({ "stringValue": "https://example.invalid/ticket" }),
            json!({ "doubleValue": 3.0 }),
        ),
        (
            "User",
            json!({ "userArnValue": "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1" }),
            json!({ "stringValue": "not an arn-valued member" }),
        ),
    ];

    for (field_type, accepted, refused) in rows {
        let field_id = field_of_type(&fields, field_type);

        // The matching variant is stored, and reads back as written.
        let case = format!("CreateCase {field_type} with its own variant");
        let created = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                json!({
                    "templateId": template_id,
                    "fields": [
                        title_value("a typed case"),
                        { "id": field_id, "value": accepted },
                    ],
                }),
            )
            .await;
        let case_id = string_at(&created, "caseId", &case);
        let read = get_case_fields(
            &emulator,
            &case,
            &domain_id,
            &case_id,
            std::slice::from_ref(&field_id),
        )
        .await;
        assert_eq!(
            read[0].get("value"),
            Some(&accepted),
            "case {case:?}: expected {accepted:?} back, got {:?}",
            read[0].get("value")
        );

        // A variant of another type is refused, on the create and on the update,
        // and the refusal names the field and the variant its stored type takes
        // — the variant the accepted value above is written in.
        let expected_variant = accepted
            .as_object()
            .and_then(|value| value.keys().next().cloned())
            .unwrap_or_else(|| panic!("the accepted {field_type} value names no variant"));
        for (operation, method, path) in [
            (
                "CreateCase",
                "POST",
                format!("/domains/{domain_id}/cases"),
            ),
            (
                "UpdateCase",
                "PUT",
                format!("/domains/{domain_id}/cases/{case_id}"),
            ),
        ] {
            let case = format!("{operation} {field_type} with a value of another type");
            let mut body = json!({ "fields": [{ "id": field_id, "value": refused }] });
            if operation == "CreateCase" {
                body["templateId"] = json!(template_id);
            }
            let (status, code, message) = emulator
                .failure_detail(&case, method, &path, Some(body))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: expected 400 ValidationException, got {status} {code}: {message}"
            );
            assert_eq!(
                message,
                format!("Field '{field_id}' value should be a {expected_variant}"),
                "case {case:?}: the refusal must name the field and the variant its type \
                 takes, got {message:?}"
            );
        }

        // And the refused update left the stored value alone.
        let case = format!("GetCase {field_type} after a refused update");
        let read = get_case_fields(
            &emulator,
            &case,
            &domain_id,
            &case_id,
            std::slice::from_ref(&field_id),
        )
        .await;
        assert_eq!(
            read[0].get("value"),
            Some(&accepted),
            "case {case:?}: a refused update must leave the value alone, got {:?}",
            read[0].get("value")
        );
    }
}

#[tokio::test]
async fn a_name_is_unique_within_its_container() {
    let emulator = Emulator::new();
    let first_domain = domain(&emulator, "shared-name").await;
    let other_domain = domain(&emulator, "other-domain").await;

    // A second domain under a name already taken is refused, and the first
    // domain is untouched.
    let (status, code) = emulator
        .failure(
            "CreateDomain duplicate name",
            "POST",
            "/domains",
            Some(json!({ "name": "shared-name" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case \"CreateDomain duplicate name\": expected 409 ConflictException, got {status} {code}"
    );
    let still_there = emulator
        .ok(
            "GetDomain after conflict",
            "POST",
            &format!("/domains/{first_domain}"),
            json!({}),
        )
        .await;
    assert_eq!(
        string_at(&still_there, "domainId", "GetDomain after conflict"),
        first_domain,
        "case \"GetDomain after conflict\": the first domain must survive, got {still_there:?}"
    );

    // Fields and templates are named within their domain: the same name
    // collides inside one domain and is free in another.
    for (kind, collection, body) in [
        ("field", "fields", json!({ "name": "duplicate", "type": "Text" })),
        ("template", "templates", json!({ "name": "duplicate" })),
    ] {
        let case = format!("Create{kind} first");
        emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{first_domain}/{collection}"),
                body.clone(),
            )
            .await;
        let conflict = format!("Create{kind} duplicate name in the same domain");
        let (status, code) = emulator
            .failure(
                &conflict,
                "POST",
                &format!("/domains/{first_domain}/{collection}"),
                Some(body.clone()),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {conflict:?}: expected 409 ConflictException, got {status} {code}"
        );
        let elsewhere = format!("Create{kind} same name in another domain");
        emulator
            .ok(
                &elsewhere,
                "POST",
                &format!("/domains/{other_domain}/{collection}"),
                body.clone(),
            )
            .await;
    }
}

#[tokio::test]
async fn an_arn_is_built_in_the_region_the_caller_signed_for() {
    // The region is read from the request's SigV4 credential scope, so an ARN is
    // stamped with the region the resource was created in rather than a fixed one.
    for region in ["us-east-1", "eu-west-1", "ap-southeast-2"] {
        let emulator = Emulator::in_region(region);
        let case = format!("CreateDomain region={region}");
        let created = emulator
            .ok(&case, "POST", "/domains", json!({ "name": "regional" }))
            .await;
        let domain_id = string_at(&created, "domainId", &case);
        assert_eq!(
            string_at(&created, "domainArn", &case),
            format!("arn:aws:cases:{region}:000000000000:domain/{domain_id}"),
            "case {case:?}: the ARN must carry the signed region, got {created:?}"
        );
        let fetched = emulator
            .ok(&case, "POST", &format!("/domains/{domain_id}"), json!({}))
            .await;
        assert_eq!(
            string_at(&fetched, "domainArn", &case),
            string_at(&created, "domainArn", &case),
            "case {case:?}: the stored ARN must be the one read back, got {fetched:?}"
        );
    }
}

#[tokio::test]
async fn every_minted_id_is_distinct_and_opaque() {
    // The invariant behind `state::new_id`: an id is unique across every mint and
    // every resource kind, carries nothing from the request, and takes the
    // 8-4-4-4-12 lowercase-hex form the service's ids have. Byte-identical
    // requests (same field name, same template shape, in different domains) must
    // still get different ids, which is what a counter-plus-entropy mint
    // guarantees and a request-derived id would not.
    const DOMAINS: usize = 8;

    let emulator = Emulator::new();
    let mut minted: Vec<(String, String)> = Vec::new();

    for index in 0..DOMAINS {
        // Only the domain name differs between rounds; every other request body
        // below is byte-identical from round to round.
        let case = format!("round {index}");
        let domain_id = domain(&emulator, &format!("mint-{index}")).await;
        minted.push(("domainId".to_string(), domain_id.clone()));

        let field = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({ "name": "same-name-every-round", "type": "Text" }),
            )
            .await;
        let field_id = string_at(&field, "fieldId", &case);
        minted.push(("fieldId".to_string(), field_id.clone()));

        let template_id = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/templates"),
                json!({ "name": "same-template-every-round" }),
            )
            .await;
        let template_id = string_at(&template_id, "templateId", &case);
        minted.push(("templateId".to_string(), template_id.clone()));

        let created = emulator
            .ok(
                &case,
                "POST",
                &format!("/domains/{domain_id}/cases"),
                json!({
                    "templateId": template_id,
                    "fields": [
                        title_value("a minted case"),
                        { "id": field_id, "value": { "stringValue": "same value" } },
                    ],
                }),
            )
            .await;
        minted.push(("caseId".to_string(), string_at(&created, "caseId", &case)));
    }

    for (member, id) in &minted {
        assert!(
            is_minted_id(id),
            "case {member:?}: every minted id must take the 8-4-4-4-12 lowercase-hex \
             form, got {id:?}"
        );
    }

    let mut seen: HashMap<&str, &str> = HashMap::new();
    for (member, id) in &minted {
        let previous = seen.insert(id.as_str(), member.as_str());
        assert!(
            previous.is_none(),
            "case {member:?}: id {id:?} was already minted for {previous:?} — ids must be \
             unique across every mint"
        );
    }
    assert_eq!(
        minted.len(),
        DOMAINS * 4,
        "case \"mint count\": expected {} ids, got {}",
        DOMAINS * 4,
        minted.len()
    );
}

#[tokio::test]
async fn a_case_must_provide_a_value_for_every_field_its_template_requires() {
    // The model's rule for a template's requiredFields: "List of fields that
    // must have a value provided to create a case"
    // (smithy_query(query="doc",shape="RequiredField")). So CreateCase is
    // refused while any required id has no entry in `fields`, and a value of
    // any union variant — `emptyValue` included, since the model calls it "An
    // empty value", i.e. a value of the union — counts as provided.
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "required-fields-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let number = field_of_type(&fields, "Number");
    let boolean = field_of_type(&fields, "Boolean");
    let url = field_of_type(&fields, "Url");

    let demanding = template(
        &emulator,
        &domain_id,
        "requires-three",
        &[text.clone(), number.clone(), boolean.clone()],
    )
    .await;
    let undemanding = template(&emulator, &domain_id, "requires-nothing", &[]).await;

    let text_value = json!({ "id": text, "value": { "stringValue": "a note" } });
    let number_value = json!({ "id": number, "value": { "doubleValue": 1.5 } });
    let boolean_value = json!({ "id": boolean, "value": { "booleanValue": false } });
    let url_value = json!({ "id": url, "value": { "stringValue": "https://example.test/" } });
    let text_empty = json!({ "id": text, "value": { "emptyValue": {} } });

    // (case, templateId, fields, accepted, the required ids a refusal must name)
    type Row = (&'static str, String, Vec<Value>, bool, Vec<String>);
    let rows: Vec<Row> = vec![
        (
            "none of the three required fields",
            demanding.clone(),
            vec![],
            false,
            vec![text.clone(), number.clone(), boolean.clone()],
        ),
        (
            "one of the three",
            demanding.clone(),
            vec![text_value.clone()],
            false,
            vec![number.clone(), boolean.clone()],
        ),
        (
            "two of the three",
            demanding.clone(),
            vec![text_value.clone(), number_value.clone()],
            false,
            vec![boolean.clone()],
        ),
        (
            "a field the template does not require, instead of one it does",
            demanding.clone(),
            vec![url_value.clone()],
            false,
            vec![text.clone(), number.clone(), boolean.clone()],
        ),
        (
            "all three",
            demanding.clone(),
            vec![
                text_value.clone(),
                number_value.clone(),
                boolean_value.clone(),
            ],
            true,
            vec![],
        ),
        (
            "all three plus a field the template does not require",
            demanding.clone(),
            vec![
                text_value.clone(),
                number_value.clone(),
                boolean_value.clone(),
                url_value.clone(),
            ],
            true,
            vec![],
        ),
        (
            "a required field provided as the union's emptyValue",
            demanding.clone(),
            vec![text_empty, number_value.clone(), boolean_value.clone()],
            true,
            vec![],
        ),
        (
            "a template that requires nothing, given no fields",
            undemanding.clone(),
            vec![],
            true,
            vec![],
        ),
        (
            "a template that requires nothing, given one field",
            undemanding,
            vec![text_value],
            true,
            vec![],
        ),
    ];

    for (case, template_id, case_fields, accepted, must_name) in rows {
        // Every body carries the `title` the service requires of every create,
        // so the rule under test is the TEMPLATE's `requiredFields` alone.
        let mut supplied = vec![title_value("a required-fields case")];
        supplied.extend(case_fields);
        let body = json!({ "templateId": template_id, "fields": supplied });
        let path = format!("/domains/{domain_id}/cases");
        if accepted {
            let created = emulator.ok(case, "POST", &path, body).await;
            let case_id = string_at(&created, "caseId", case);
            assert!(
                is_minted_id(&case_id),
                "case {case:?}: an accepted create must mint a case id, got {created:?}"
            );
        } else {
            let (status, code, message) = emulator
                .failure_detail(case, "POST", &path, Some(body))
                .await;
            assert_eq!(
                (status, code.as_str()),
                (400, "ValidationException"),
                "case {case:?}: a missing required field is a 400 ValidationException, \
                 got {status} {code}: {message}"
            );
            // The refusal names the FIRST required field the request provided
            // no value for, in the order the template declares them, and nothing
            // else: the template is addressed by the request, so its id is not
            // part of the message.
            let expected = template_required_field_refusal(&must_name[0]);
            assert_eq!(
                message, expected,
                "case {case:?}: expected {expected:?}, got {message:?}"
            );
            assert!(
                !message.contains(template_id.as_str()),
                "case {case:?}: the refusal must not carry the template id, got {message:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Case rules: the resource `CreateCaseRule` mints, `BatchGetCaseRule` reports
// and a template references.
// ---------------------------------------------------------------------------

/// `CreateCaseRule` for `rule`, as the `(caseRuleId, caseRuleArn)` it minted.
///
/// `pub(crate)` so a sibling suite that needs a case rule to enumerate reaches
/// this create rather than growing a second copy of it.
pub(crate) async fn create_case_rule(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    name: &str,
    description: Option<&str>,
    rule: &Value,
) -> (String, String) {
    let mut body = json!({ "name": name, "rule": rule });
    if let Some(description) = description {
        body["description"] = json!(description);
    }
    let created = emulator
        .ok(case, "POST", &format!("/domains/{domain_id}/case-rules"), body)
        .await;
    (
        string_at(&created, "caseRuleId", case),
        string_at(&created, "caseRuleArn", case),
    )
}

/// `BatchGetCaseRule` over `ids`, as its `(caseRules, errors)` lists.
async fn batch_get_case_rules(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    ids: &[&str],
) -> (Vec<Value>, Vec<Value>) {
    let response =
        crate::harness::batch_get_case_rules_response(emulator, case, domain_id, ids).await;
    let rules = response
        .get("caseRules")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetCaseRule reported no caseRules: {response:?}")
        })
        .clone();
    let errors = response
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: BatchGetCaseRule reported no errors list: {response:?}")
        })
        .clone();
    (rules, errors)
}

/// The one case rule `BatchGetCaseRule` reports for `id`, which must resolve.
async fn read_case_rule(emulator: &Emulator, case: &str, domain_id: &str, id: &str) -> Value {
    let (rules, errors) = batch_get_case_rules(emulator, case, domain_id, &[id]).await;
    assert!(
        errors.is_empty(),
        "case {case:?}: BatchGetCaseRule must resolve {id}, got errors {errors:?}"
    );
    assert_eq!(
        rules.len(),
        1,
        "case {case:?}: expected exactly one rule back for {id}, got {rules:?}"
    );
    rules[0].clone()
}

/// One row per member of the `CaseRuleDetails` union the model declares —
/// `required`, `fieldOptions` and `hidden`
/// (`smithy_query(query="shape", shape="CaseRuleDetails")`) — spanning the forms
/// a rule body takes: a single comparison, no comparison at all, comparisons
/// nested through `andAll`/`orAll`, each `OperandTwo` variant, and a
/// field-options mapping with and without the optional parent and child fields.
/// Every row is a rule a create must store and a read must hand back exactly as
/// written.
fn case_rule_bodies(fields: &[(&'static str, String)], child_select: &str) -> Vec<(String, Value)> {
    let text = field_of_type(fields, "Text");
    let number = field_of_type(fields, "Number");
    let boolean = field_of_type(fields, "Boolean");
    let select = field_of_type(fields, "SingleSelect");
    vec![
        (
            "required with one equalTo comparison on a string".to_string(),
            json!({
                "required": {
                    "defaultValue": true,
                    "conditions": [{
                        "equalTo": {
                            "operandOne": { "fieldId": text },
                            "operandTwo": { "stringValue": "escalated" },
                            "result": true,
                        }
                    }],
                }
            }),
        ),
        (
            "required with no condition at all".to_string(),
            json!({ "required": { "defaultValue": false, "conditions": [] } }),
        ),
        (
            "required comparing a number".to_string(),
            json!({
                "required": {
                    "defaultValue": false,
                    "conditions": [{
                        "notEqualTo": {
                            "operandOne": { "fieldId": number },
                            "operandTwo": { "doubleValue": 42.5 },
                            "result": true,
                        }
                    }],
                }
            }),
        ),
        (
            "hidden with conditions nested through andAll and orAll".to_string(),
            json!({
                "hidden": {
                    "defaultValue": false,
                    "conditions": [{
                        "andAll": {
                            "conditions": [
                                {
                                    "equalTo": {
                                        "operandOne": { "fieldId": boolean },
                                        "operandTwo": { "booleanValue": true },
                                        "result": true,
                                    }
                                },
                                {
                                    "orAll": {
                                        "conditions": [{
                                            "notEqualTo": {
                                                "operandOne": { "fieldId": text },
                                                "operandTwo": { "emptyValue": {} },
                                                "result": false,
                                            }
                                        }],
                                    }
                                },
                            ],
                        }
                    }],
                }
            }),
        ),
        (
            "fieldOptions mapping a parent select field onto a child".to_string(),
            json!({
                "fieldOptions": {
                    "parentFieldId": select,
                    "childFieldId": child_select,
                    "parentChildFieldOptionsMappings": [
                        { "parentFieldOptionValue": "gold", "childFieldOptionValues": ["gold-a", "gold-b"] },
                        { "parentFieldOptionValue": "silver", "childFieldOptionValues": [] },
                    ],
                }
            }),
        ),
        (
            "fieldOptions naming neither a parent nor a child field".to_string(),
            json!({
                "fieldOptions": {
                    "parentChildFieldOptionsMappings": [
                        { "parentFieldOptionValue": "", "childFieldOptionValues": ["only-child"] },
                    ],
                }
            }),
        ),
    ]
}

#[tokio::test]
async fn a_case_rule_round_trips_through_state_in_every_form_the_model_declares() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    // A second single-select field, so a field-options rule has a child field to
    // map onto that is not its own parent.
    let child_select = string_at(
        &emulator
            .ok(
                "CreateField child select",
                "POST",
                &format!("/domains/{domain_id}/fields"),
                json!({ "name": "child-select", "type": "SingleSelect" }),
            )
            .await,
        "fieldId",
        "CreateField child select",
    );

    for (index, (case, rule)) in case_rule_bodies(&fields, &child_select)
        .into_iter()
        .enumerate()
    {
        // Half the rows carry the optional description, so a read reports the
        // member when it was written and leaves it out when it was not.
        let description = (index % 2 == 0).then(|| format!("rule {index} of the table"));
        let name = format!("rule-{index}");
        let (case_rule_id, case_rule_arn) = create_case_rule(
            &emulator,
            &case,
            &domain_id,
            &name,
            description.as_deref(),
            &rule,
        )
        .await;
        assert!(
            is_minted_id(&case_rule_id),
            "case {case:?}: the caseRuleId must be a server-minted opaque id, got {case_rule_id:?}"
        );
        assert_eq!(
            case_rule_arn,
            format!("arn:aws:cases:us-east-1:000000000000:domain/{domain_id}/case-rule/{case_rule_id}"),
            "case {case:?}: caseRuleArn must address the stored rule, got {case_rule_arn:?}"
        );

        // The read is of the STORED record: the rule body comes back in the
        // union variant it was written with, member for member.
        let read = read_case_rule(&emulator, &case, &domain_id, &case_rule_id).await;
        assert_eq!(
            read.get("caseRuleId").and_then(Value::as_str),
            Some(case_rule_id.as_str()),
            "case {case:?}: the read must report the id it was asked for, got {read:?}"
        );
        assert_eq!(
            read.get("caseRuleArn").and_then(Value::as_str),
            Some(case_rule_arn.as_str()),
            "case {case:?}: the read must report the stored ARN, got {read:?}"
        );
        assert_eq!(
            read.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: the read must report the stored name, got {read:?}"
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            description.as_deref(),
            "case {case:?}: the read must report the description as written, got {read:?}"
        );
        assert_eq!(
            read.get("rule"),
            Some(&rule),
            "case {case:?}: the read must hand back the rule as written, got {read:?}"
        );
        assert_eq!(
            read.get("deleted").and_then(Value::as_bool),
            Some(false),
            "case {case:?}: a rule nothing deleted reports deleted false, got {read:?}"
        );
        assert_eq!(
            read.get("tags"),
            Some(&json!({})),
            "case {case:?}: an untagged rule reports an empty tag set, got {read:?}"
        );
        // `createdTime` and `lastModifiedTime` are the record's own instants:
        // nothing has changed the rule since it was created, so they agree.
        let created = crate::harness::instant_at(&read, "createdTime", &case);
        let last_modified = crate::harness::instant_at(&read, "lastModifiedTime", &case);
        assert_eq!(
            created, last_modified,
            "case {case:?}: an unchanged rule's instants must agree, got {read:?}"
        );

        // Tagging the rule through the ARN the create handed out, and reading
        // the tags back through the rule itself: the stored record owns its tag
        // set, so the two reads cannot disagree.
        crate::harness::tag_resource(
            &emulator,
            &case,
            &case_rule_arn,
            &json!({ "owner": name.clone() }),
        )
        .await;
        assert_eq!(
            crate::harness::listed_tags(&emulator, &case, &case_rule_arn).await,
            json!({ "owner": name.clone() }),
            "case {case:?}: ListTagsForResource must report the tags written to {case_rule_arn}"
        );
        let tagged = read_case_rule(&emulator, &case, &domain_id, &case_rule_id).await;
        assert_eq!(
            tagged.get("tags"),
            Some(&json!({ "owner": name })),
            "case {case:?}: the rule must report its own tag set, got {tagged:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_read_of_case_rules_is_scoped_to_its_domain_and_reports_each_id() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-batch-domain").await;
    let other_domain = domain(&emulator, "case-rule-other-domain").await;
    let rule = json!({ "required": { "defaultValue": true, "conditions": [] } });
    let (first, _) = create_case_rule(&emulator, "first", &domain_id, "first", None, &rule).await;
    let (second, _) = create_case_rule(&emulator, "second", &domain_id, "second", None, &rule).await;
    let (elsewhere, _) =
        create_case_rule(&emulator, "elsewhere", &other_domain, "elsewhere", None, &rule).await;

    // A repeated id is answered once per occurrence, as on `BatchGetField`; an
    // id this domain holds no rule for — including one that exists in ANOTHER
    // domain — is a per-item failure rather than a failure of the whole call.
    // The unresolvable id is a WELL-FORMED case-rule identifier: one that is not
    // a case-rule identifier at all is a bad request, which
    // `case_rule_reads.rs` pins.
    let unknown = "00000000-0000-4000-8000-000000000000";
    let requested = [
        first.as_str(),
        unknown,
        second.as_str(),
        first.as_str(),
        elsewhere.as_str(),
    ];
    let (rules, errors) =
        batch_get_case_rules(&emulator, "a mixed batch", &domain_id, &requested).await;
    let resolved: Vec<&str> = rules
        .iter()
        .filter_map(|rule| rule.get("caseRuleId").and_then(Value::as_str))
        .collect();
    assert_eq!(
        resolved,
        vec![first.as_str(), second.as_str(), first.as_str()],
        "case \"a mixed batch\": every resolvable id is reported once per occurrence, \
         got {rules:?}"
    );
    let failed: Vec<(&str, &str)> = errors
        .iter()
        .map(|error| {
            (
                error.get("id").and_then(Value::as_str).unwrap_or_default(),
                error
                    .get("errorCode")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        failed,
        vec![
            (unknown, "ResourceNotFound"),
            (elsewhere.as_str(), "ResourceNotFound"),
        ],
        "case \"a mixed batch\": an id this domain holds no rule for is a per-item \
         failure, got {errors:?}"
    );
    assert_eq!(
        errors[0].get("message").and_then(Value::as_str),
        Some("This resource cannot be found"),
        "case \"a mixed batch\": the per-item message is the service's fixed sentence, \
         got {errors:?}"
    );

    // A domain this store does not hold is a failure of the whole call.
    let (status, code) = emulator
        .failure(
            "a batch read in a domain that does not exist",
            "POST",
            "/domains/no-such-domain/rules-batch",
            Some(json!({ "caseRules": [{ "id": first }] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a batch read in a domain that does not exist\": expected 404 \
         ResourceNotFoundException, got {status} {code}"
    );
}

/// One create of a case rule the refusal table drives: the case, the domain the
/// call addresses, its request body, and the `(status, code)` it must be refused
/// with — `None` for a row the service accepts.
type CaseRuleCreateRow = (String, String, Value, Option<(u16, &'static str)>);

#[tokio::test]
async fn a_case_rule_a_write_cannot_resolve_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-refusal-domain").await;
    let other_domain = domain(&emulator, "case-rule-refusal-other").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let other_fields = fields_of_every_type(&emulator, &other_domain).await;
    let text_elsewhere = field_of_type(&other_fields, "Text");
    let condition = |field_id: &str| {
        json!({
            "equalTo": {
                "operandOne": { "fieldId": field_id },
                "operandTwo": { "stringValue": "escalated" },
                "result": true,
            }
        })
    };
    let required = |conditions: Value| json!({ "required": { "defaultValue": true, "conditions": conditions } });
    create_case_rule(
        &emulator,
        "a rule holding a name",
        &domain_id,
        "taken",
        None,
        &required(json!([condition(&text)])),
    )
    .await;

    // Conditions nested one level past the ceiling the walk admits, built from
    // the constant rather than from a literal depth.
    let deepest_admitted = service_com_amazonaws_connectcases_amazonconnectcases::state::MAX_CASE_RULE_CONDITION_DEPTH;
    let mut deepest = condition(&text);
    for _ in 0..deepest_admitted {
        deepest = json!({ "andAll": { "conditions": [deepest] } });
    }
    // One level inside it is a tree the walk does admit.
    let at_the_ceiling = deepest["andAll"]["conditions"][0].clone();

    // `CaseRuleName` is `@length(1..100)` with `@pattern("^.*[\\S]$")` and
    // `CaseRuleDescription` is `@length(max: 255)`
    // (`smithy_query(query="operation", operation="CreateCaseRule")`), so each
    // bound is tested at its published value and one past it. A value outside one of
    // those is a BODY rejection, refused ahead of the handler under the code that layer
    // owns; the rows that name a reference the store cannot resolve are the handler's own
    // modelled refusals.
    let rows: Vec<CaseRuleCreateRow> = vec![
        (
            "a domain this store does not hold".to_string(),
            "no-such-domain".to_string(),
            json!({ "name": "fine", "rule": required(json!([])) }),
            Some((404, "ResourceNotFoundException")),
        ),
        (
            "a name a live rule of the domain already holds".to_string(),
            domain_id.clone(),
            json!({ "name": "taken", "rule": required(json!([])) }),
            Some((409, "ConflictException")),
        ),
        (
            "a condition comparing a field the domain does not define".to_string(),
            domain_id.clone(),
            json!({ "name": "dangling-field", "rule": required(json!([condition("no-such-field")])) }),
            Some((400, "ValidationException")),
        ),
        (
            "a condition comparing a field of ANOTHER domain".to_string(),
            domain_id.clone(),
            json!({
                "name": "field-of-another-domain",
                "rule": required(json!([condition(&text_elsewhere)])),
            }),
            Some((400, "ValidationException")),
        ),
        (
            "a field-options rule naming a parent field the domain does not define".to_string(),
            domain_id.clone(),
            json!({
                "name": "dangling-parent",
                "rule": {
                    "fieldOptions": {
                        "parentFieldId": "no-such-field",
                        "parentChildFieldOptionsMappings": [
                            { "parentFieldOptionValue": "gold", "childFieldOptionValues": [] },
                        ],
                    }
                },
            }),
            Some((400, "ValidationException")),
        ),
        (
            format!("conditions nested one level past {deepest_admitted}"),
            domain_id.clone(),
            json!({ "name": "too-deep", "rule": required(json!([deepest])) }),
            Some((400, "ValidationException")),
        ),
        (
            format!("conditions nested exactly {deepest_admitted} levels deep"),
            domain_id.clone(),
            json!({ "name": "deep-enough", "rule": required(json!([at_the_ceiling])) }),
            None,
        ),
        (
            "a name of the published maximum 100 characters".to_string(),
            domain_id.clone(),
            json!({ "name": "n".repeat(100), "rule": required(json!([])) }),
            None,
        ),
        (
            "a name one past the published maximum 100 characters".to_string(),
            domain_id.clone(),
            json!({ "name": "n".repeat(101), "rule": required(json!([])) }),
            Some((400, BODY_REJECTION_CODE)),
        ),
        (
            "a name one short of the published minimum 1 character".to_string(),
            domain_id.clone(),
            json!({ "name": "", "rule": required(json!([])) }),
            Some((400, BODY_REJECTION_CODE)),
        ),
        (
            "a name that does not end in a non-whitespace character".to_string(),
            domain_id.clone(),
            json!({ "name": "trailing space ", "rule": required(json!([])) }),
            Some((400, BODY_REJECTION_CODE)),
        ),
        (
            "a description of the published maximum 255 characters".to_string(),
            domain_id.clone(),
            json!({
                "name": "longest-description",
                "description": "d".repeat(255),
                "rule": required(json!([])),
            }),
            None,
        ),
        (
            "a description one past the published maximum 255 characters".to_string(),
            domain_id.clone(),
            json!({
                "name": "overlong-description",
                "description": "d".repeat(256),
                "rule": required(json!([])),
            }),
            Some((400, BODY_REJECTION_CODE)),
        ),
    ];

    for (case, domain, body, expected) in rows {
        let path = format!("/domains/{domain}/case-rules");
        match expected {
            None => {
                let created = emulator.ok(&case, "POST", &path, body).await;
                let case_rule_id = string_at(&created, "caseRuleId", &case);
                assert!(
                    is_minted_id(&case_rule_id),
                    "case {case:?}: an accepted create must mint a rule id, got {created:?}"
                );
            }
            Some((status, code)) => {
                let (seen_status, seen_code, message) = emulator
                    .failure_detail(&case, "POST", &path, Some(body))
                    .await;
                assert_eq!(
                    (seen_status, seen_code.as_str()),
                    (status, code),
                    "case {case:?}: expected {status} {code}, got {seen_status} \
                     {seen_code}: {message}"
                );
            }
        }
    }

    // The two refusals this emulator words itself carry the service's own
    // sentences, with every variable part taken from the request and the stored
    // record.
    let (_, _, conflict) = emulator
        .failure_detail(
            "the conflict sentence",
            "POST",
            &format!("/domains/{domain_id}/case-rules"),
            Some(json!({ "name": "taken", "rule": required(json!([])) })),
        )
        .await;
    assert_eq!(
        conflict,
        "Case rule creation failed: case-rule resource with the name taken already exists!",
        "case \"the conflict sentence\": a taken name is refused with the kind's own \
         sentence, got {conflict:?}"
    );
    let (_, _, unresolved) = emulator
        .failure_detail(
            "the unresolved field sentence",
            "POST",
            &format!("/domains/{domain_id}/case-rules"),
            Some(json!({
                "name": "dangling",
                "rule": required(json!([condition("no-such-field")])),
            })),
        )
        .await;
    assert_eq!(
        unresolved,
        "The following field IDs provided in rule do not exist: no-such-field",
        "case \"the unresolved field sentence\": the refusal attributes the ids to the \
         member they rode in, got {unresolved:?}"
    );
}

#[tokio::test]
async fn a_template_references_the_case_rules_of_its_own_domain() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "template-rule-domain").await;
    let other_domain = domain(&emulator, "template-rule-other").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let boolean = field_of_type(&fields, "Boolean");
    let rule_body = json!({ "required": { "defaultValue": true, "conditions": [] } });
    let (first, _) = create_case_rule(&emulator, "first", &domain_id, "first", None, &rule_body).await;
    let (second, _) =
        create_case_rule(&emulator, "second", &domain_id, "second", None, &rule_body).await;
    let (elsewhere, _) = create_case_rule(
        &emulator,
        "elsewhere",
        &other_domain,
        "elsewhere",
        None,
        &rule_body,
    )
    .await;

    // A create stores the associations as given: their order, and the optional
    // field each one names, are part of the record.
    let associations = json!([
        { "caseRuleId": first, "fieldId": text },
        { "caseRuleId": second },
        { "caseRuleId": first, "fieldId": boolean },
    ]);
    let created = emulator
        .ok(
            "CreateTemplate with case rules",
            "POST",
            &format!("/domains/{domain_id}/templates"),
            json!({ "name": "ruled-template", "rules": associations }),
        )
        .await;
    let template_id = string_at(&created, "templateId", "CreateTemplate with case rules");
    let fetched = emulator
        .ok(
            "GetTemplate after the create",
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            Value::Null,
        )
        .await;
    assert_eq!(
        fetched.get("rules"),
        Some(&associations),
        "case \"GetTemplate after the create\": the template must hand back the \
         associations as written, got {fetched:?}"
    );

    // An update replaces the list, and a read reports the replacement.
    let replacement = json!([{ "caseRuleId": second, "fieldId": text }]);
    emulator
        .ok(
            "UpdateTemplate replacing the rules",
            "PUT",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            json!({ "rules": replacement }),
        )
        .await;
    let updated = emulator
        .ok(
            "GetTemplate after the update",
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            Value::Null,
        )
        .await;
    assert_eq!(
        updated.get("rules"),
        Some(&replacement),
        "case \"GetTemplate after the update\": the replacement list must be the stored \
         one, got {updated:?}"
    );

    // Every reference a write takes out is resolved against the domain's own
    // records first, so neither a create nor an update can store one that
    // cannot be followed.
    let rows: Vec<(String, &str, String, Value, String)> = vec![
        (
            "a create naming a case rule nothing holds".to_string(),
            "POST",
            format!("/domains/{domain_id}/templates"),
            json!({ "name": "dangling-rule", "rules": [{ "caseRuleId": "no-such-case-rule" }] }),
            "The following case rule IDs provided in rules do not exist: no-such-case-rule"
                .to_string(),
        ),
        (
            "a create naming a case rule of ANOTHER domain".to_string(),
            "POST",
            format!("/domains/{domain_id}/templates"),
            json!({ "name": "rule-of-another-domain", "rules": [{ "caseRuleId": elsewhere }] }),
            format!("The following case rule IDs provided in rules do not exist: {elsewhere}"),
        ),
        (
            "a create whose association names a field the domain does not define".to_string(),
            "POST",
            format!("/domains/{domain_id}/templates"),
            json!({
                "name": "dangling-rule-field",
                "rules": [{ "caseRuleId": first, "fieldId": "no-such-field" }],
            }),
            "The following field IDs provided in rules do not exist: no-such-field".to_string(),
        ),
        (
            "an update naming a case rule nothing holds".to_string(),
            "PUT",
            format!("/domains/{domain_id}/templates/{template_id}"),
            json!({ "rules": [{ "caseRuleId": "no-such-case-rule" }] }),
            "The following case rule IDs provided in rules do not exist: no-such-case-rule"
                .to_string(),
        ),
        (
            "an update whose association names a field the domain does not define".to_string(),
            "PUT",
            format!("/domains/{domain_id}/templates/{template_id}"),
            json!({ "rules": [{ "caseRuleId": first, "fieldId": "no-such-field" }] }),
            "The following field IDs provided in rules do not exist: no-such-field".to_string(),
        ),
    ];
    for (case, method, path, body, expected) in rows {
        let (status, code, message) = emulator
            .failure_detail(&case, method, &path, Some(body))
            .await;
        assert_eq!(
            (status, code.as_str(), message.as_str()),
            (400, "ValidationException", expected.as_str()),
            "case {case:?}: expected a 400 ValidationException worded {expected:?}, \
             got {status} {code}: {message}"
        );
    }

    // A refused write changed nothing: the template still carries the list the
    // update that WAS accepted left on it.
    let unchanged = emulator
        .ok(
            "GetTemplate after the refusals",
            "POST",
            &format!("/domains/{domain_id}/templates/{template_id}"),
            Value::Null,
        )
        .await;
    assert_eq!(
        unchanged.get("rules"),
        Some(&replacement),
        "case \"GetTemplate after the refusals\": a refused write must leave the stored \
         associations alone, got {unchanged:?}"
    );
}

/// Every case rule summary `ListCaseRules` reports for `domain_id`, following its
/// pages to exhaustion so the answer is every rule of the domain and not only the
/// first page. `maxResults` and `nextToken` are `@httpQuery` members of this
/// operation, so both travel in the query string rather than in the body.
///
/// `pub(crate)` so a sibling suite that has to see what the listing enumerates
/// after a delete reaches this walk rather than growing a second copy of it.
pub(crate) async fn listed_case_rules(
    emulator: &Emulator,
    case: &str,
    domain_id: &str,
    max_results: Option<i32>,
) -> Vec<Value> {
    let mut listed: Vec<Value> = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let mut query: Vec<String> = Vec::new();
        if let Some(max_results) = max_results {
            query.push(format!("maxResults={max_results}"));
        }
        if let Some(token) = &token {
            query.push(format!("nextToken={token}"));
        }
        let query = if query.is_empty() {
            String::new()
        } else {
            format!("?{}", query.join("&"))
        };
        let answer = emulator
            .ok(
                case,
                "POST",
                &format!("/domains/{domain_id}/rules-list/{query}"),
                json!({}),
            )
            .await;
        let page = answer
            .get("caseRules")
            .and_then(Value::as_array)
            .unwrap_or_else(|| {
                panic!("case {case:?}: ListCaseRules reported no caseRules: {answer:?}")
            })
            .clone();
        if let Some(max_results) = max_results {
            assert!(
                page.len() <= max_results as usize,
                "case {case:?}: a page must hold at most the {max_results} entries asked \
                 for, got {page:?}"
            );
        }
        listed.extend(page);
        token = answer
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if token.is_none() {
            return listed;
        }
    }
}

#[tokio::test]
async fn a_listing_of_case_rules_reports_its_own_domain_in_creation_order() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-listing-domain").await;
    let other_domain = domain(&emulator, "case-rule-listing-other").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let select = field_of_type(&fields, "SingleSelect");

    // One rule of each `RuleType` the model declares, created in this order, so
    // the listing's order and the `ruleType` it reports are both asserted over
    // every variant. `RULE_TYPE_VALUES` is the model's own value list.
    let bodies: Vec<(&str, &str, Value)> = vec![
        (
            "first",
            "Required",
            json!({ "required": { "defaultValue": true, "conditions": [] } }),
        ),
        (
            "second",
            "Hidden",
            json!({ "hidden": { "defaultValue": false, "conditions": [] } }),
        ),
        (
            "third",
            "FieldOptions",
            json!({
                "fieldOptions": {
                    "parentFieldId": select,
                    "parentChildFieldOptionsMappings": [
                        { "parentFieldOptionValue": "gold", "childFieldOptionValues": ["gold-a"] },
                    ],
                }
            }),
        ),
    ];
    let mut expected: Vec<(String, String, String)> = Vec::new();
    for (name, rule_type, body) in &bodies {
        let case = format!("CreateCaseRule {name}");
        let description = format!("the {name} rule");
        let (id, arn) = create_case_rule(
            &emulator,
            &case,
            &domain_id,
            name,
            Some(&description),
            body,
        )
        .await;
        expected.push((id, arn, (*rule_type).to_string()));
    }
    // A rule of ANOTHER domain, which this domain's listing must not report.
    let (elsewhere, _) = create_case_rule(
        &emulator,
        "elsewhere",
        &other_domain,
        "elsewhere",
        None,
        &json!({ "required": { "defaultValue": true, "conditions": [] } }),
    )
    .await;

    // The whole listing, and the same listing walked one entry at a time: a
    // caller that follows the tokens to exhaustion sees every rule exactly once,
    // in the same order.
    for page_size in [None, Some(1), Some(2), Some(100)] {
        let case = format!("ListCaseRules maxResults={page_size:?}");
        let listed = listed_case_rules(&emulator, &case, &domain_id, page_size).await;
        let reported: Vec<(String, String, String, String)> = listed
            .iter()
            .map(|summary| {
                (
                    string_at(summary, "caseRuleId", &case),
                    string_at(summary, "caseRuleArn", &case),
                    string_at(summary, "ruleType", &case),
                    string_at(summary, "name", &case),
                )
            })
            .collect();
        let wanted: Vec<(String, String, String, String)> = expected
            .iter()
            .zip(bodies.iter())
            .map(|((id, arn, rule_type), (name, _, _))| {
                (
                    id.clone(),
                    arn.clone(),
                    rule_type.clone(),
                    (*name).to_string(),
                )
            })
            .collect();
        assert_eq!(
            reported, wanted,
            "case {case:?}: the listing reports this domain's rules in creation order, \
             each summarised from the stored record, got {listed:?}"
        );
        for (summary, (name, _, _)) in listed.iter().zip(bodies.iter()) {
            assert_eq!(
                summary.get("description").and_then(Value::as_str),
                Some(format!("the {name} rule").as_str()),
                "case {case:?}: the summary reports the stored description, got {summary:?}"
            );
        }
        assert!(
            !listed
                .iter()
                .any(|summary| summary.get("caseRuleId").and_then(Value::as_str)
                    == Some(elsewhere.as_str())),
            "case {case:?}: a rule of another domain must not be listed, got {listed:?}"
        );
    }

    // `MaxResults` is `@range(1..100)`
    // (`smithy_query(query="operation", operation="ListCaseRules")`), so a page
    // size outside it is refused rather than clamped, and a cursor this listing
    // never issued is refused rather than resumed from.
    let rows: Vec<(&str, String)> = vec![
        (
            "a page size one short of the published minimum 1",
            format!("/domains/{domain_id}/rules-list/?maxResults=0"),
        ),
        (
            "a page size one past the published maximum 100",
            format!("/domains/{domain_id}/rules-list/?maxResults=101"),
        ),
        (
            "a cursor this listing never issued",
            format!("/domains/{domain_id}/rules-list/?nextToken=deadbeef"),
        ),
    ];
    for (case, path) in rows {
        let (status, code) = emulator
            .failure(case, "POST", &path, Some(json!({})))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {case:?}: expected a 400 ValidationException, got {status} {code}"
        );
    }
    let (status, code) = emulator
        .failure(
            "a listing of a domain this store does not hold",
            "POST",
            "/domains/no-such-domain/rules-list/",
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"a listing of a domain this store does not hold\": expected 404 \
         ResourceNotFoundException, got {status} {code}"
    );
}
