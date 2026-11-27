// SPDX-License-Identifier: CC0-1.0

//! The case-rule READ path: what `BatchGetCaseRule` reports for the identifiers
//! it was handed, and what a later read sees once `UpdateCaseRule` and
//! `DeleteCaseRule` have changed the stored record.
//!
//! Four rules are pinned here, each over a SET of inputs rather than one call:
//!
//! 1. a batch read always reports `unprocessedCaseRules`, as the empty list when
//!    it left nothing unprocessed;
//! 2. an identifier that is not a case-rule identifier at all refuses the WHOLE
//!    call with the modelled `ValidationException`, while a well-formed
//!    identifier the domain holds no rule for stays a per-item failure;
//! 3. a member `UpdateCaseRule` wrote is what a later read reports, and a member
//!    it left out is preserved;
//! 4. a deleted rule leaves the listing while the batch read still reports the
//!    record, flagged.

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state;

use crate::case_lifecycle::{create_case_rule, listed_case_rules};
use crate::harness::{
    batch_get_case_rules_response, domain, field_of_type, fields_of_every_type, instant_at,
    refused_as_not_found, string_at, Emulator,
};

/// The shape the wire reports a modelled 400 under, in the envelope and in the
/// `x-amzn-errortype` header alike.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The sentence the service refuses a batch read of case rules whose
/// identifiers are not case-rule identifiers with: fixed text for this error
/// class, interpolating nothing from the request.
const INVALID_CASE_RULE_IDS: &str = "The CaseRuleId(s) provided are not valid";

/// A `required` rule body with no condition at all: the smallest rule a create
/// accepts, used wherever a suite needs a rule whose body is beside the point.
fn plain_rule() -> Value {
    json!({ "required": { "defaultValue": true, "conditions": [] } })
}

/// A well-formed case-rule identifier nothing has ever minted: the 8-4-4-4-12
/// hexadecimal form the service's identifiers take, so a read of it reaches the
/// lookup and is reported per item rather than refused.
const UNMINTED_ID: &str = "00000000-0000-4000-8000-000000000000";

/// The `caseRules` list of a batch-read answer.
fn resolved(answer: &Value) -> Vec<Value> {
    answer
        .get("caseRules")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the answer reports no caseRules list: {answer:?}"))
}

/// The `errors` list of a batch-read answer.
fn failures(answer: &Value) -> Vec<Value> {
    answer
        .get("errors")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_else(|| panic!("the answer reports no errors list: {answer:?}"))
}

/// The one record a batch read reports for `id`, which must resolve.
async fn read_case_rule(emulator: &Emulator, case: &str, domain_id: &str, id: &str) -> Value {
    let answer = batch_get_case_rules_response(emulator, case, domain_id, &[id]).await;
    let records = resolved(&answer);
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected exactly one record back for {id}, got {answer:?}"
    );
    records[0].clone()
}

#[tokio::test]
async fn every_batch_read_of_case_rules_reports_an_unprocessed_list() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-unprocessed-domain").await;
    let (first, _) =
        create_case_rule(&emulator, "first", &domain_id, "first", None, &plain_rule()).await;
    let (second, _) =
        create_case_rule(&emulator, "second", &domain_id, "second", None, &plain_rule()).await;

    // The batch's shape is what varies: how many identifiers it carries, how
    // many of them resolve, whether one repeats, and the model's own ceiling of
    // `MAX_BATCH_CASE_RULE_IDS` entries. Whatever the shape, the answer reports
    // every requested identifier through `caseRules` or `errors` and leaves none
    // unprocessed, so `unprocessedCaseRules` is the EMPTY list and never absent.
    let full_batch: Vec<&str> = (0..state::MAX_BATCH_CASE_RULE_IDS)
        .map(|_| first.as_str())
        .collect();
    let rows: Vec<(String, Vec<&str>, usize, usize)> = vec![
        ("one identifier that resolves".to_string(), vec![first.as_str()], 1, 0),
        (
            "two identifiers that both resolve".to_string(),
            vec![first.as_str(), second.as_str()],
            2,
            0,
        ),
        (
            "the same identifier twice".to_string(),
            vec![first.as_str(), first.as_str()],
            2,
            0,
        ),
        (
            "one identifier that resolves and one that does not".to_string(),
            vec![first.as_str(), UNMINTED_ID],
            1,
            1,
        ),
        (
            "no identifier that resolves at all".to_string(),
            vec![UNMINTED_ID],
            0,
            1,
        ),
        (
            format!("the full batch of {} entries", state::MAX_BATCH_CASE_RULE_IDS),
            full_batch,
            state::MAX_BATCH_CASE_RULE_IDS,
            0,
        ),
    ];

    for (case, ids, records, errors) in rows {
        let answer = batch_get_case_rules_response(&emulator, &case, &domain_id, &ids).await;
        assert_eq!(
            answer.get("unprocessedCaseRules"),
            Some(&json!([])),
            "case {case:?}: the read must report an unprocessed list, empty when it left \
             nothing unprocessed, got {answer:?}"
        );
        assert_eq!(
            (resolved(&answer).len(), failures(&answer).len()),
            (records, errors),
            "case {case:?}: expected {records} records and {errors} errors, got {answer:?}"
        );
    }
}

#[tokio::test]
async fn an_identifier_that_is_not_a_case_rule_identifier_refuses_the_whole_batch_read() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-identifier-domain").await;
    let (minted, _) =
        create_case_rule(&emulator, "minted", &domain_id, "minted", None, &plain_rule()).await;

    // `CaseRuleId` is constrained by LENGTH alone — `@length(1..500)`, with no
    // pattern (`smithy_query(query="shape", shape="CaseRuleId")`) — so every row
    // below reaches the handler. A row that is not the 8-4-4-4-12 hexadecimal
    // form the service's identifiers take names no rule the service could have
    // minted, so the whole call is refused; the rows that ARE that form reach the
    // lookup, whatever they resolve to.
    let hyphenless = minted.replace('-', "");
    let short_group = format!("{}-{}", &minted[..7], &minted[8..]);
    let long_group = format!("{}0{}", &minted[..8], &minted[8..]);
    let non_hex = format!("{}g", &minted[..minted.len() - 1]);
    let extra_group = format!("{minted}-0000");
    let missing_group = minted[..minted.len() - 13].to_string();
    let trailing_space = format!("{minted} ");
    let malformed: Vec<(String, String)> = vec![
        ("a word that is not an identifier".to_string(), "no-such-case-rule".to_string()),
        ("a single character".to_string(), "x".to_string()),
        ("the identifier with its hyphens stripped".to_string(), hyphenless),
        ("a group one hexadecimal digit short".to_string(), short_group),
        ("a group one hexadecimal digit long".to_string(), long_group),
        ("a group carrying a non-hexadecimal character".to_string(), non_hex),
        ("one group too many".to_string(), extra_group),
        ("one group too few".to_string(), missing_group),
        ("the identifier with a trailing space".to_string(), trailing_space),
    ];

    for (case, id) in &malformed {
        // Alone, and beside an identifier that WOULD resolve: the refusal is of
        // the whole call, so the resolvable identifier is not reported either.
        for (case, body) in [
            (format!("{case}, alone"), json!({ "caseRules": [{ "id": id }] })),
            (
                format!("{case}, beside one that resolves"),
                json!({ "caseRules": [{ "id": minted }, { "id": id }] }),
            ),
        ] {
            let (status, error_type, message) = emulator
                .failure_error_type(
                    &case,
                    "POST",
                    &format!("/domains/{domain_id}/rules-batch"),
                    Some(body),
                )
                .await;
            assert_eq!(
                (status, error_type.as_str()),
                (400, VALIDATION_EXCEPTION),
                "case {case:?}: expected 400 {VALIDATION_EXCEPTION}, got {status} \
                 {error_type}: {message:?}"
            );
            assert_eq!(
                message, INVALID_CASE_RULE_IDS,
                "case {case:?}: expected the fixed sentence of this error class, got \
                 {message:?}"
            );
            assert!(
                !message.contains(id.as_str()),
                "case {case:?}: the refusal must not echo the identifier, got {message:?}"
            );
        }
    }

    // The boundary of the rule: an identifier of the right FORM is looked up,
    // and reported per item when the domain holds no rule for it — including the
    // minted identifier in the other hexadecimal case, which is a well-formed
    // identifier this domain holds no record under.
    let well_formed: Vec<(String, String, usize)> = vec![
        ("the minted identifier".to_string(), minted.clone(), 1),
        ("a well-formed identifier nothing minted".to_string(), UNMINTED_ID.to_string(), 0),
        (
            "the minted identifier in upper case".to_string(),
            minted.to_uppercase(),
            0,
        ),
    ];
    for (case, id, records) in well_formed {
        let answer = batch_get_case_rules_response(&emulator, &case, &domain_id, &[&id]).await;
        assert_eq!(
            (resolved(&answer).len(), failures(&answer).len()),
            (records, 1 - records),
            "case {case:?}: a well-formed identifier is looked up rather than refused, \
             got {answer:?}"
        );
    }
}

/// One `UpdateCaseRule` the read-back table drives: the case, the request body,
/// and the `(name, description, rule)` a read must report once it was applied —
/// `None` for a description the record must report no value for.
type UpdateRow = (String, Value, String, Option<String>, Value);

#[tokio::test]
async fn an_update_of_a_case_rule_is_reported_by_a_later_read() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-update-domain").await;
    let fields = fields_of_every_type(&emulator, &domain_id).await;
    let text = field_of_type(&fields, "Text");
    let select = field_of_type(&fields, "SingleSelect");
    let (rule_id, rule_arn) = create_case_rule(
        &emulator,
        "the rule to update",
        &domain_id,
        "before",
        None,
        &plain_rule(),
    )
    .await;

    let hidden = json!({
        "hidden": {
            "defaultValue": false,
            "conditions": [{
                "equalTo": {
                    "operandOne": { "fieldId": text },
                    "operandTwo": { "stringValue": "escalated" },
                    "result": true,
                }
            }],
        }
    });
    let field_options = json!({
        "fieldOptions": {
            "parentFieldId": select,
            "parentChildFieldOptionsMappings": [
                { "parentFieldOptionValue": "gold", "childFieldOptionValues": ["gold-a"] },
            ],
        }
    });
    // Each row is applied to the SAME rule in turn, so every row's expectation
    // is also a check that the members the previous row wrote survived a request
    // that named none of them: `name`, `description` and `rule` are the three
    // modifiable members the model declares
    // (`smithy_query(query="operation", operation="UpdateCaseRule")`).
    let rows: Vec<UpdateRow> = vec![
        (
            "a description onto a rule created without one".to_string(),
            json!({ "description": "the first description" }),
            "before".to_string(),
            Some("the first description".to_string()),
            plain_rule(),
        ),
        (
            "a description replacing the stored one".to_string(),
            json!({ "description": "the second description" }),
            "before".to_string(),
            Some("the second description".to_string()),
            plain_rule(),
        ),
        (
            "a rename, leaving the description alone".to_string(),
            json!({ "name": "after" }),
            "after".to_string(),
            Some("the second description".to_string()),
            plain_rule(),
        ),
        (
            "a rule body of another union variant".to_string(),
            json!({ "rule": hidden }),
            "after".to_string(),
            Some("the second description".to_string()),
            hidden.clone(),
        ),
        (
            "a rename to the name it already holds".to_string(),
            json!({ "name": "after" }),
            "after".to_string(),
            Some("the second description".to_string()),
            hidden.clone(),
        ),
        (
            "every modifiable member at once".to_string(),
            json!({
                "name": "renamed-again",
                "description": "the third description",
                "rule": field_options,
            }),
            "renamed-again".to_string(),
            Some("the third description".to_string()),
            field_options.clone(),
        ),
        (
            "an empty description, which is a value of its own".to_string(),
            json!({ "description": "" }),
            "renamed-again".to_string(),
            Some(String::new()),
            field_options.clone(),
        ),
        (
            "a request naming no modifiable member at all".to_string(),
            json!({}),
            "renamed-again".to_string(),
            Some(String::new()),
            field_options.clone(),
        ),
    ];

    let created_at = instant_at(
        &read_case_rule(&emulator, "the created rule", &domain_id, &rule_id).await,
        "createdTime",
        "the created rule",
    );
    let mut last_modified = created_at;
    for (case, body, name, description, rule) in rows {
        emulator
            .ok(
                &case,
                "PUT",
                &format!("/domains/{domain_id}/case-rules/{rule_id}"),
                body,
            )
            .await;
        let read = read_case_rule(&emulator, &case, &domain_id, &rule_id).await;
        assert_eq!(
            read.get("name").and_then(Value::as_str),
            Some(name.as_str()),
            "case {case:?}: the read must report the stored name, got {read:?}"
        );
        assert_eq!(
            read.get("description").and_then(Value::as_str),
            description.as_deref(),
            "case {case:?}: the read must report the stored description, got {read:?}"
        );
        assert_eq!(
            read.get("rule"),
            Some(&rule),
            "case {case:?}: the read must report the stored rule body, got {read:?}"
        );
        // The identity of the record never moves, and neither does the instant
        // it was created at; the last-modified instant never moves backwards.
        assert_eq!(
            (
                read.get("caseRuleId").and_then(Value::as_str),
                read.get("caseRuleArn").and_then(Value::as_str),
                read.get("deleted").and_then(Value::as_bool),
            ),
            (Some(rule_id.as_str()), Some(rule_arn.as_str()), Some(false)),
            "case {case:?}: an update must not move the record's identity, got {read:?}"
        );
        assert_eq!(
            instant_at(&read, "createdTime", &case),
            created_at,
            "case {case:?}: createdTime must stay the instant the rule was created, got {read:?}"
        );
        let modified_at = instant_at(&read, "lastModifiedTime", &case);
        assert!(
            modified_at >= last_modified,
            "case {case:?}: lastModifiedTime must not move backwards, got {modified_at} \
             after {last_modified}"
        );
        last_modified = modified_at;

        // The listing summarises the same record, so it cannot disagree with the
        // batch read about the name or the description.
        let listed = listed_case_rules(&emulator, &case, &domain_id, None).await;
        let summary = listed
            .iter()
            .find(|summary| summary.get("caseRuleId").and_then(Value::as_str) == Some(&rule_id))
            .unwrap_or_else(|| panic!("case {case:?}: the rule must still be listed: {listed:?}"));
        assert_eq!(
            (
                summary.get("name").and_then(Value::as_str),
                summary.get("description").and_then(Value::as_str),
            ),
            (Some(name.as_str()), description.as_deref()),
            "case {case:?}: the summary must report the same record as the read, got \
             {summary:?}"
        );
    }

    // A request naming no modifiable member changed nothing, so the record's
    // last-modified instant is the one the update before it left.
    let unchanged = read_case_rule(&emulator, "after the no-op update", &domain_id, &rule_id).await;
    assert_eq!(
        instant_at(&unchanged, "lastModifiedTime", "after the no-op update"),
        last_modified,
        "a request that modified nothing must not move lastModifiedTime, got {unchanged:?}"
    );
}

#[tokio::test]
async fn an_update_a_write_cannot_resolve_is_refused_with_the_modeled_error() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-update-refusal-domain").await;
    let other_domain = domain(&emulator, "case-rule-update-refusal-other").await;
    let (rule_id, _) =
        create_case_rule(&emulator, "the rule", &domain_id, "the-rule", None, &plain_rule()).await;
    let (taken, _) =
        create_case_rule(&emulator, "taken", &domain_id, "taken", None, &plain_rule()).await;
    let (elsewhere, _) = create_case_rule(
        &emulator,
        "elsewhere",
        &other_domain,
        "elsewhere",
        None,
        &plain_rule(),
    )
    .await;
    let condition = |field_id: &str| {
        json!({
            "rule": {
                "required": {
                    "defaultValue": true,
                    "conditions": [{
                        "equalTo": {
                            "operandOne": { "fieldId": field_id },
                            "operandTwo": { "stringValue": "escalated" },
                            "result": true,
                        }
                    }],
                }
            }
        })
    };

    let rows: Vec<(String, String, String, Value, u16, &str)> = vec![
        (
            "a domain this store does not hold".to_string(),
            "no-such-domain".to_string(),
            rule_id.clone(),
            json!({ "description": "d" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a rule this domain holds no record for".to_string(),
            domain_id.clone(),
            UNMINTED_ID.to_string(),
            json!({ "description": "d" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a rule of ANOTHER domain".to_string(),
            domain_id.clone(),
            elsewhere.clone(),
            json!({ "description": "d" }),
            404,
            "ResourceNotFoundException",
        ),
        (
            "a rename onto a name another live rule holds".to_string(),
            domain_id.clone(),
            rule_id.clone(),
            json!({ "name": "taken" }),
            409,
            "ConflictException",
        ),
        (
            "a rule body comparing a field the domain does not define".to_string(),
            domain_id.clone(),
            rule_id.clone(),
            condition("no-such-field"),
            400,
            VALIDATION_EXCEPTION,
        ),
        (
            "a description one past the published maximum 255 characters".to_string(),
            domain_id.clone(),
            rule_id.clone(),
            json!({ "description": "d".repeat(256) }),
            400,
            crate::harness::BODY_REJECTION_CODE,
        ),
        (
            "a name one past the published maximum 100 characters".to_string(),
            domain_id.clone(),
            rule_id.clone(),
            json!({ "name": "n".repeat(101) }),
            400,
            crate::harness::BODY_REJECTION_CODE,
        ),
    ];

    for (case, domain_addressed, addressed, body, status, code) in rows {
        let (seen_status, seen_code, message) = emulator
            .failure_detail(
                &case,
                "PUT",
                &format!("/domains/{domain_addressed}/case-rules/{addressed}"),
                Some(body),
            )
            .await;
        assert_eq!(
            (seen_status, seen_code.as_str()),
            (status, code),
            "case {case:?}: expected {status} {code}, got {seen_status} {seen_code}: {message}"
        );
    }

    // A refused update wrote nothing: the record is the one the create left, and
    // the rule whose name the rename asked for still holds it.
    let read = read_case_rule(&emulator, "after the refusals", &domain_id, &rule_id).await;
    assert_eq!(
        (
            read.get("name").and_then(Value::as_str),
            read.get("description"),
            read.get("rule"),
        ),
        (Some("the-rule"), None, Some(&plain_rule())),
        "a refused update must leave the stored record alone, got {read:?}"
    );
    let still_taken = read_case_rule(&emulator, "the rule holding the name", &domain_id, &taken)
        .await;
    assert_eq!(
        still_taken.get("name").and_then(Value::as_str),
        Some("taken"),
        "the rule holding the name must keep it, got {still_taken:?}"
    );
    // The field reference that could not be resolved is attributed to the member
    // it rode in, exactly as the create path attributes it.
    let (_, _, unresolved) = emulator
        .failure_detail(
            "the unresolved field sentence",
            "PUT",
            &format!("/domains/{domain_id}/case-rules/{rule_id}"),
            Some(condition("no-such-field")),
        )
        .await;
    assert_eq!(
        unresolved, "The following field IDs provided in rule do not exist: no-such-field",
        "case \"the unresolved field sentence\": the refusal names the member the \
         references rode in, got {unresolved:?}"
    );
}

#[tokio::test]
async fn a_deleted_case_rule_leaves_the_listing_and_is_read_back_flagged() {
    let emulator = Emulator::new();
    let domain_id = domain(&emulator, "case-rule-delete-domain").await;
    let mut created: Vec<String> = Vec::new();
    for name in ["first", "second", "third"] {
        let (id, _) = create_case_rule(
            &emulator,
            name,
            &domain_id,
            name,
            Some(&format!("the {name} rule")),
            &plain_rule(),
        )
        .await;
        created.push(id);
    }
    let deleted_id = created[1].clone();
    let survivors = [created[0].clone(), created[2].clone()];

    // The delete, then the state every read of the removed rule reports. The
    // whole block runs twice: the second pass drives the delete of a rule that
    // is already gone, which is refused rather than answered again, and nothing
    // a read reports changes between the passes.
    for attempt in ["the delete", "the repeat delete"] {
        let path = format!("/domains/{domain_id}/case-rules/{deleted_id}");
        if attempt == "the delete" {
            emulator.ok(attempt, "DELETE", &path, json!({})).await;
        } else {
            // A rule that has been removed is no longer a resource a write can
            // address, so the repeat is the modelled 404 and not a second
            // success.
            let refusal = refused_as_not_found(&emulator, attempt, "DELETE", &path, None).await;
            assert_eq!(
                refusal.get("resourceId").and_then(Value::as_str),
                Some(deleted_id.as_str()),
                "case {attempt:?}: the refusal must report the rule it could not resolve, \
                 got {refusal:?}"
            );
        }
        let listed = listed_case_rules(&emulator, attempt, &domain_id, None).await;
        let enumerated: Vec<String> = listed
            .iter()
            .map(|summary| string_at(summary, "caseRuleId", attempt))
            .collect();
        assert_eq!(
            enumerated,
            survivors.to_vec(),
            "case {attempt:?}: the listing must enumerate only the rules that are still \
             there, got {listed:?}"
        );

        // The record survives the delete, flagged: `GetCaseRuleResponse` carries
        // a `deleted` member, so the batch read reports the record rather than a
        // per-item failure, with everything the rule held.
        let read = read_case_rule(&emulator, attempt, &domain_id, &deleted_id).await;
        assert_eq!(
            (
                read.get("deleted").and_then(Value::as_bool),
                read.get("name").and_then(Value::as_str),
                read.get("description").and_then(Value::as_str),
            ),
            (Some(true), Some("second"), Some("the second rule")),
            "case {attempt:?}: the deleted rule is reported flagged, got {read:?}"
        );
    }

    // What a deleted rule is no longer: a rule a write can change, or one a new
    // reference can be taken out to. Its NAME is free again, as a create of a
    // rule under it shows.
    let (status, code, message) = emulator
        .failure_detail(
            "an update of a deleted rule",
            "PUT",
            &format!("/domains/{domain_id}/case-rules/{deleted_id}"),
            Some(json!({ "description": "after the delete" })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (404, "ResourceNotFoundException"),
        "case \"an update of a deleted rule\": expected 404 ResourceNotFoundException, \
         got {status} {code}: {message}"
    );
    let (status, code, message) = emulator
        .failure_detail(
            "a template referencing a deleted rule",
            "POST",
            &format!("/domains/{domain_id}/templates"),
            Some(json!({ "name": "deleted-rule-template", "rules": [{ "caseRuleId": deleted_id }] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (400, VALIDATION_EXCEPTION),
        "case \"a template referencing a deleted rule\": expected 400 \
         {VALIDATION_EXCEPTION}, got {status} {code}: {message}"
    );
    let (reused, _) = create_case_rule(
        &emulator,
        "a rule taking the deleted rule's name",
        &domain_id,
        "second",
        None,
        &plain_rule(),
    )
    .await;
    assert_ne!(
        reused, deleted_id,
        "the create must mint a new rule rather than resurrect the deleted one"
    );

    // A delete the store cannot resolve is the modelled 404, whether it is the
    // domain or the rule that does not exist.
    for (case, path) in [
        (
            "a delete in a domain this store does not hold",
            format!("/domains/no-such-domain/case-rules/{deleted_id}"),
        ),
        (
            "a delete of a rule this domain holds no record for",
            format!("/domains/{domain_id}/case-rules/{UNMINTED_ID}"),
        ),
    ] {
        let refusal = refused_as_not_found(&emulator, case, "DELETE", &path, None).await;
        assert_eq!(
            refusal.get("resourceType").and_then(Value::as_str),
            Some(if case.contains("domain this store") {
                "domain"
            } else {
                "case-rule"
            }),
            "case {case:?}: the refusal must name the kind that could not be resolved, \
             got {refusal:?}"
        );
    }
}
