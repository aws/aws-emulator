// SPDX-License-Identifier: CC0-1.0

//! The security requirement UPDATE path of the SecurityAgent emulator, end to end over
//! the wire: `BatchUpdateSecurityRequirements` writing into records a
//! `BatchCreateSecurityRequirements` wrote, with every outcome re-read through
//! `BatchGetSecurityRequirements` and `ListSecurityRequirements` so a case counts only
//! when the SHARED STORE agrees.
//!
//! Every call goes through the real `MultiServiceRouter` the emulator boots (see
//! `crate::support`), as a `restJson1` `POST /<OperationName>` with a JSON body signed
//! for the `securityagent` signing name. So nothing here can pass against a handler that
//! answers a canned body: a written value must come back out of the shared store through
//! a read before a case counts.
//!
//! Each case asserts a RULE over a TABLE of inputs rather than one recorded call: a
//! mutation of a requirement is written to the one `(packId, name)` record every later
//! read projects — each member the update carries replaces the stored value, each member
//! it omits keeps it, `updatedAt` moves to the write and `createdAt` stays put, and a
//! record a delete removed is gone from every later read. So the rows differ along which
//! members an update carries, the form of each value, how many requirements the pack
//! holds, how many names a request names and where the names it cannot resolve sit.

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use serde_json::{json, Map, Value};
use service_com_amazonaws_securityagent_securityagent::state::{
    AWS_MANAGED_PACKS, BATCH_REQUIREMENT_ERROR_NOT_FOUND, BATCH_REQUIREMENT_ERROR_NOT_PROCESSED,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::support::{call, emulator, member};

/// The members of a stored requirement an update can write, and a read projects.
///
/// Source: `smithy_query(query="shape", shape="UpdateSecurityRequirementEntry")` — every
/// member of the update entry besides the immutable `name`.
const WRITABLE_MEMBERS: [&str; 4] = ["description", "domain", "evaluation", "remediation"];

/// `call`, requiring `expected` as the HTTP status and naming the case when it is not.
async fn expect_status(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    expected: u16,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        answer.status, expected,
        "case {case:?}: expected {operation} to answer {expected}, got {} ({}) with {}",
        answer.status, answer.error_code, answer.body
    );
    answer.body
}

/// `call`, requiring a refusal with `status` and `code`, and naming the case otherwise.
async fn expect_refusal(
    router: &MultiServiceRouter,
    operation: &str,
    body: &Value,
    status: u16,
    code: &str,
    case: &str,
) -> Value {
    let answer = call(router, operation, body).await;
    assert_eq!(
        (answer.status, answer.error_code.as_str()),
        (status, code),
        "case {case:?}: expected {operation} to answer {status} {code}, got {} ({}) with {}",
        answer.status,
        answer.error_code,
        answer.body
    );
    answer.body
}

/// A fresh customer managed pack, returning its minted `packId`. The model gives the
/// pack create `http {code: 201}`.
async fn create_pack(router: &MultiServiceRouter, name: &str, case: &str) -> String {
    let created = expect_status(
        router,
        "CreateSecurityRequirementPack",
        &json!({"name": name}),
        201,
        case,
    )
    .await;
    member(&created, "packId")
        .unwrap_or_else(|| panic!("case {case:?}: no packId was minted: {created}"))
}

/// Store the given entries. `BatchCreateSecurityRequirements` carries `http {code: 201}`
/// in the model.
async fn batch_create(
    router: &MultiServiceRouter,
    pack_id: &str,
    entries: &[Value],
    case: &str,
) -> Value {
    let created = expect_status(
        router,
        "BatchCreateSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirements": entries}),
        201,
        case,
    )
    .await;
    assert!(
        errors(&created, case).is_empty(),
        "case {case:?}: the fixture batch did not store cleanly: {created}"
    );
    created
}

/// Store one plain requirement per name in `names`.
async fn store(router: &MultiServiceRouter, pack_id: &str, names: &[&str], case: &str) {
    let entries: Vec<Value> = names.iter().map(|name| created_entry(name, None)).collect();
    batch_create(router, pack_id, &entries, case).await;
}

/// The `CreateSecurityRequirementEntry` every row starts from: fixed values for the
/// `@required` members, and `remediation` only where the row asks for one.
fn created_entry(name: &str, remediation: Option<&str>) -> Value {
    let mut entry = json!({
        "name": name,
        "description": format!("the created description of {name}"),
        "domain": "DataProtection",
        "evaluation": format!("the created evaluation of {name}")
    });
    if let Some(remediation) = remediation {
        entry["remediation"] = json!(remediation);
    }
    entry
}

/// `BatchUpdateSecurityRequirements` on `entries`, requiring HTTP 200 (the model defaults
/// this operation's success status).
async fn batch_update(
    router: &MultiServiceRouter,
    pack_id: &str,
    entries: &[Value],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchUpdateSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirements": entries}),
        200,
        case,
    )
    .await
}

/// `BatchDeleteSecurityRequirements` on `names`, requiring HTTP 200.
async fn batch_delete(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchDeleteSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirementNames": names}),
        200,
        case,
    )
    .await
}

/// `BatchGetSecurityRequirements` on `names`, requiring HTTP 200.
async fn batch_get(
    router: &MultiServiceRouter,
    pack_id: &str,
    names: &[&str],
    case: &str,
) -> Value {
    expect_status(
        router,
        "BatchGetSecurityRequirements",
        &json!({"packId": pack_id, "securityRequirementNames": names}),
        200,
        case,
    )
    .await
}

/// The ONE record `name` names in `pack_id`, read back through the batch read.
async fn read_one(router: &MultiServiceRouter, pack_id: &str, name: &str, case: &str) -> Value {
    let read = batch_get(router, pack_id, &[name], case).await;
    let records = results(&read, case);
    assert_eq!(
        records.len(),
        1,
        "case {case:?}: expected exactly the record {name:?}, got {read}"
    );
    records[0].clone()
}

/// Every summary `ListSecurityRequirements` reports for `pack_id`, following the cursor
/// so a pack larger than one page is still read whole.
async fn list_all(router: &MultiServiceRouter, pack_id: &str, case: &str) -> Vec<Value> {
    let mut summaries = Vec::new();
    let mut token: Option<String> = None;
    loop {
        let body = match &token {
            None => json!({"packId": pack_id}),
            Some(cursor) => json!({"packId": pack_id, "nextToken": cursor}),
        };
        let page = expect_status(router, "ListSecurityRequirements", &body, 200, case).await;
        summaries.extend(
            page.get("securityRequirementSummaries")
                .and_then(Value::as_array)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no securityRequirementSummaries list in {page}")
                })
                .clone(),
        );
        token = member(&page, "nextToken");
        if token.is_none() {
            return summaries;
        }
    }
}

/// The `securityRequirements` list of a batch response — `@required`, so it is present
/// (and empty) even when every name failed.
fn results(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("securityRequirements")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no securityRequirements list in {payload}"))
        .clone()
}

/// The `errors` list of a batch response — `@required`, so it is present (and empty)
/// even when every entry succeeded.
fn errors(payload: &Value, case: &str) -> Vec<Value> {
    payload
        .get("errors")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: no errors list in {payload}"))
        .clone()
}

/// The `updatedSecurityRequirementNames` list of an update response — `@required`, so it
/// is present (and empty) even when every entry failed.
fn updated(payload: &Value, case: &str) -> Vec<String> {
    payload
        .get("updatedSecurityRequirementNames")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: no updatedSecurityRequirementNames list in {payload}")
        })
        .iter()
        .map(|name| {
            name.as_str()
                .unwrap_or_else(|| panic!("case {case:?}: an updated name is not a string: {name}"))
                .to_string()
        })
        .collect()
}

/// The result or error entry naming `name`, or `None` when the list carries none.
fn entry_named<'a>(entries: &'a [Value], field: &str, name: &str) -> Option<&'a Value> {
    entries
        .iter()
        .find(|entry| member(entry, field).as_deref() == Some(name))
}

/// The entry of `errors` naming `name`, required to carry the not-found code and a
/// message naming it.
fn expect_not_found(failed: &[Value], name: &str, case: &str) {
    let error = entry_named(failed, "securityRequirementName", name).unwrap_or_else(|| {
        panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
    });
    assert_eq!(
        member(error, "code").as_deref(),
        Some(BATCH_REQUIREMENT_ERROR_NOT_FOUND),
        "case {case:?}: {name:?} was reported with the wrong code: {error}"
    );
    assert!(
        member(error, "message").is_some_and(|message| message.contains(name)),
        "case {case:?}: {name:?} was reported without a message naming it: {error}"
    );
}

/// The entry of `errors` naming `name`, required to carry the not-processed code — what a
/// name the pack DOES hold carries when another named requirement stopped the request.
fn expect_not_processed(failed: &[Value], name: &str, case: &str) {
    let error = entry_named(failed, "securityRequirementName", name).unwrap_or_else(|| {
        panic!("case {case:?}: {name:?} was not reported in errors: {failed:?}")
    });
    assert_eq!(
        member(error, "code").as_deref(),
        Some(BATCH_REQUIREMENT_ERROR_NOT_PROCESSED),
        "case {case:?}: {name:?} was reported with the wrong code: {error}"
    );
}

/// A `@timestampFormat("date-time")` member, in epoch seconds.
///
/// `BatchGetSecurityRequirementResult$createdAt`/`$updatedAt` and their
/// `SecurityRequirementSummary` counterparts all carry `timestampFormat: date-time`
/// (`smithy_query(query="shape", shape="SecurityRequirementSummary")`), which restJson1
/// renders as a string.
fn date_time(payload: &Value, name: &str, case: &str) -> f64 {
    let value = payload
        .get(name)
        .unwrap_or_else(|| panic!("case {case:?}: no {name} in {payload}"));
    let text = value.as_str().unwrap_or_else(|| {
        panic!(
            "case {case:?}: expected {name} as an RFC 3339 date-time string, got {value} \
             in {payload}"
        )
    });
    DateTime::from_str(text, Format::DateTime)
        .unwrap_or_else(|e| panic!("case {case:?}: {name} {text:?} is not a date-time: {e}"))
        .as_secs_f64()
}

/// One row of the update table: whether the created record carried a `remediation`, and
/// the members the update entry carries besides the immutable `name`.
struct UpdateRow {
    case: &'static str,
    created_remediation: Option<&'static str>,
    patch: Value,
}

/// The update rows.
///
/// They differ along WHICH writable members the entry carries (each one alone, several at
/// once, none at all), whether the record it addresses already held the optional
/// `remediation`, and the FORM of each written value — ASCII, Unicode, empty,
/// whitespace-bearing, long — none of which the rule distinguishes: a member the entry
/// carries is written and a member it omits is kept.
fn update_rows() -> Vec<UpdateRow> {
    vec![
        UpdateRow {
            case: "description alone",
            created_remediation: Some("the created remediation"),
            patch: json!({"description": "the updated description"}),
        },
        UpdateRow {
            case: "domain alone",
            created_remediation: Some("the created remediation"),
            patch: json!({"domain": "IdentityAndAccessManagement"}),
        },
        UpdateRow {
            case: "evaluation alone",
            created_remediation: None,
            patch: json!({"evaluation": "Check every store's KMS key."}),
        },
        UpdateRow {
            case: "remediation replacing the one the create carried",
            created_remediation: Some("the created remediation"),
            patch: json!({"remediation": "the updated remediation"}),
        },
        UpdateRow {
            case: "remediation on a record created without one",
            created_remediation: None,
            patch: json!({"remediation": "the first remediation this record holds"}),
        },
        UpdateRow {
            case: "description and evaluation together",
            created_remediation: Some("the created remediation"),
            patch: json!({
                "description": "the updated description",
                "evaluation": "the updated evaluation"
            }),
        },
        UpdateRow {
            case: "every writable member at once",
            created_remediation: None,
            patch: json!({
                "description": "the updated description",
                "domain": "ApplicationSecurity",
                "evaluation": "the updated evaluation",
                "remediation": "the updated remediation"
            }),
        },
        UpdateRow {
            case: "an empty description",
            created_remediation: Some("the created remediation"),
            patch: json!({"description": ""}),
        },
        UpdateRow {
            case: "values in unusual forms",
            created_remediation: Some("the created remediation"),
            patch: json!({
                "description": "  exigence de sécurité 🔐  ",
                "domain": "domaine/avec.des-caractères_0123"
            }),
        },
        UpdateRow {
            case: "a long description",
            created_remediation: None,
            patch: json!({"description": "d".repeat(4096)}),
        },
        UpdateRow {
            case: "no writable member at all",
            created_remediation: Some("the created remediation"),
            patch: json!({}),
        },
    ]
}

/// The members of `patch`, as a map.
fn patched(patch: &Value, case: &str) -> Map<String, Value> {
    patch
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: the row's patch is not an object: {patch}"))
        .clone()
}

/// RULE: an update writes each member it carries to the one `(packId, name)` record every
/// later read projects, keeps each member it omits, leaves `createdAt` where the create
/// put it, and moves `updatedAt` to the write — so a read issued after a successful update
/// answers the NEW values, never the ones the create stored.
#[tokio::test]
async fn an_update_writes_the_members_it_carries_and_keeps_the_rest() {
    let router = emulator();
    let name = "requirement-under-update";

    for row in update_rows() {
        let case = row.case;
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        batch_create(
            &router,
            &pack_id,
            &[created_entry(name, row.created_remediation)],
            case,
        )
        .await;

        let before = read_one(&router, &pack_id, name, case).await;
        let created_at = date_time(&before, "createdAt", case);
        assert_eq!(
            date_time(&before, "updatedAt", case),
            created_at,
            "case {case:?}: nothing has updated the record yet: {before}"
        );

        // The update, with the row's members beside the name the record is keyed by.
        let mut entry = patched(&row.patch, case);
        entry.insert("name".to_string(), json!(name));
        let answer = batch_update(&router, &pack_id, &[Value::Object(entry)], case).await;
        assert_eq!(
            updated(&answer, case),
            vec![name.to_string()],
            "case {case:?}: expected the name reported updated, got {answer}"
        );
        assert!(
            errors(&answer, case).is_empty(),
            "case {case:?}: an update that wrote everything reported errors: {answer}"
        );

        // The STORE, through the batch read: the members the entry carried, and the
        // created values for the ones it did not.
        let wanted = patched(&row.patch, case);
        let after = read_one(&router, &pack_id, name, case).await;
        for field in WRITABLE_MEMBERS {
            let expected = wanted.get(field).or_else(|| before.get(field));
            assert_eq!(
                after.get(field),
                expected,
                "case {case:?}: expected {field} {expected:?} after the update, got {after}"
            );
        }
        assert_eq!(
            member(&after, "name").as_deref(),
            Some(name),
            "case {case:?}: the name is the record's identity and cannot change: {after}"
        );
        assert_eq!(
            member(&after, "packId").as_deref(),
            Some(pack_id.as_str()),
            "case {case:?}: the record must stay in the pack that holds it: {after}"
        );
        assert_eq!(
            date_time(&after, "createdAt", case),
            created_at,
            "case {case:?}: an update must not move createdAt: {after}"
        );
        let updated_at = date_time(&after, "updatedAt", case);
        if wanted.is_empty() {
            assert_eq!(
                updated_at, created_at,
                "case {case:?}: an update that wrote nothing must not move updatedAt: \
                 {after}"
            );
        } else {
            assert!(
                updated_at > created_at,
                "case {case:?}: expected updatedAt {updated_at} to be later than createdAt \
                 {created_at}: {after}"
            );
        }

        // ...and through the listing, which projects the SAME record.
        let summaries = list_all(&router, &pack_id, case).await;
        let summary = entry_named(&summaries, "name", name).unwrap_or_else(|| {
            panic!("case {case:?}: the listing lost the requirement: {summaries:?}")
        });
        assert_eq!(
            summary.get("description"),
            after.get("description"),
            "case {case:?}: the summary must report the stored description: {summary}"
        );
        assert_eq!(
            date_time(summary, "createdAt", case),
            created_at,
            "case {case:?}: the summary must report the stored createdAt: {summary}"
        );
        assert_eq!(
            date_time(summary, "updatedAt", case),
            updated_at,
            "case {case:?}: the summary must report the stored updatedAt: {summary}"
        );
    }
}

/// RULE: every later read of a pack projects what the mutations left — so after an update
/// and a delete, the batch read and the listing both report the updated description and
/// neither reports a deleted name, and once every requirement is deleted the listing is
/// empty (the member is `@required`, so it is present and empty, not omitted).
///
/// The rows differ along how many requirements the pack holds, so the rule is not asserted
/// on one pack size.
#[tokio::test]
async fn updates_and_deletes_are_visible_to_every_later_read() {
    let router = emulator();

    for held in [1_usize, 3, 5] {
        let case: &str = &format!("a pack of {held} requirements");
        let names: Vec<String> = (0..held).map(|index| format!("req-{index:02}")).collect();
        let borrowed: Vec<&str> = names.iter().map(String::as_str).collect();

        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        store(&router, &pack_id, &borrowed, case).await;

        // One requirement is updated...
        let updated_name = borrowed[0];
        let new_description = "the description the update wrote";
        let answer = batch_update(
            &router,
            &pack_id,
            &[json!({"name": updated_name, "description": new_description})],
            case,
        )
        .await;
        assert_eq!(
            updated(&answer, case),
            vec![updated_name.to_string()],
            "case {case:?}: expected the updated name reported, got {answer}"
        );

        // ...and the batch read answers the NEW description.
        let read = read_one(&router, &pack_id, updated_name, case).await;
        assert_eq!(
            member(&read, "description").as_deref(),
            Some(new_description),
            "case {case:?}: the batch read still projects the pre-update description: {read}"
        );

        // One requirement is deleted, and the listing now reports the updated description
        // and no entry for the deleted name.
        let deleted_name = borrowed[held - 1];
        batch_delete(&router, &pack_id, &[deleted_name], case).await;
        let summaries = list_all(&router, &pack_id, case).await;
        if deleted_name != updated_name {
            let summary = entry_named(&summaries, "name", updated_name).unwrap_or_else(|| {
                panic!("case {case:?}: the listing lost the updated requirement: {summaries:?}")
            });
            assert_eq!(
                member(summary, "description").as_deref(),
                Some(new_description),
                "case {case:?}: the listing still projects the pre-update description: \
                 {summary}"
            );
        }
        assert!(
            entry_named(&summaries, "name", deleted_name).is_none(),
            "case {case:?}: the listing still reports the deleted requirement \
             {deleted_name:?}: {summaries:?}"
        );
        assert_eq!(
            summaries.len(),
            held - 1,
            "case {case:?}: expected {} summaries after one delete, got {summaries:?}",
            held - 1
        );

        // Every remaining requirement is deleted, and the listing is empty.
        let remaining: Vec<&str> = borrowed
            .iter()
            .copied()
            .filter(|name| *name != deleted_name)
            .collect();
        batch_delete(&router, &pack_id, &remaining, case).await;
        let emptied = list_all(&router, &pack_id, case).await;
        assert!(
            emptied.is_empty(),
            "case {case:?}: expected an empty listing once every requirement was deleted, \
             got {emptied:?}"
        );

        // A deleted name is not a name the update can write either: the record is gone.
        let refused = batch_update(
            &router,
            &pack_id,
            &[json!({"name": updated_name, "description": "after the delete"})],
            case,
        )
        .await;
        assert!(
            updated(&refused, case).is_empty(),
            "case {case:?}: an update wrote a record the delete removed: {refused}"
        );
        expect_not_found(&errors(&refused, case), updated_name, case);
    }
}

/// RULE: the update is ATOMIC in the names it names — unless the pack holds every one of
/// them nothing is written, each name it does not hold is reported as not found and every
/// other named requirement as one the service never processed.
///
/// The rows differ along the request's LENGTH, HOW MANY of its names the pack does not
/// hold and WHERE they sit (first, middle, last, nowhere, everywhere).
#[tokio::test]
async fn an_update_is_atomic_in_the_names_it_names() {
    // (case, the names the pack holds, the names the update addresses)
    let rows: Vec<(&str, Vec<&str>, Vec<&str>)> = vec![
        ("the only name is held", vec!["a-one"], vec!["a-one"]),
        ("the only name is missing", vec!["b-one"], vec!["b-absent"]),
        (
            "the first of three is missing",
            vec!["c-two", "c-three"],
            vec!["c-one", "c-two", "c-three"],
        ),
        (
            "the middle of three is missing",
            vec!["d-one", "d-three"],
            vec!["d-one", "d-two", "d-three"],
        ),
        (
            "the last of three is missing",
            vec!["e-one", "e-two"],
            vec!["e-one", "e-two", "e-three"],
        ),
        (
            "every name of four is held",
            vec!["f-one", "f-two", "f-three", "f-four"],
            vec!["f-one", "f-two", "f-three", "f-four"],
        ),
        (
            "every name of three is missing",
            vec!["g-kept"],
            vec!["g-one", "g-two", "g-three"],
        ),
    ];

    let router = emulator();
    let new_description = "the description this request would write";

    for (case, held, requested) in rows {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        store(&router, &pack_id, &held, case).await;

        let entries: Vec<Value> = requested
            .iter()
            .map(|name| json!({"name": name, "description": new_description}))
            .collect();
        let answer = batch_update(&router, &pack_id, &entries, case).await;

        let missing: Vec<&str> = requested
            .iter()
            .copied()
            .filter(|name| !held.contains(name))
            .collect();
        if missing.is_empty() {
            assert_eq!(
                updated(&answer, case),
                requested
                    .iter()
                    .map(|name| (*name).to_string())
                    .collect::<Vec<String>>(),
                "case {case:?}: wrong updated names in {answer}"
            );
            assert!(
                errors(&answer, case).is_empty(),
                "case {case:?}: an update that wrote everything reported errors: {answer}"
            );
        } else {
            assert!(
                updated(&answer, case).is_empty(),
                "case {case:?}: a request naming {missing:?} reported a write: {answer}"
            );
            let failed = errors(&answer, case);
            assert_eq!(
                failed.len(),
                requested.len(),
                "case {case:?}: expected one error per named requirement, got {failed:?}"
            );
            for name in &requested {
                if missing.contains(name) {
                    expect_not_found(&failed, name, case);
                } else {
                    expect_not_processed(&failed, name, case);
                }
            }
        }

        // The STORE decides the case: every held name carries the written description
        // exactly when the request resolved, and the created one otherwise.
        for name in &held {
            let record = read_one(&router, &pack_id, name, case).await;
            let expected = if missing.is_empty() && requested.contains(name) {
                new_description.to_string()
            } else {
                format!("the created description of {name}")
            };
            assert_eq!(
                member(&record, "description").as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {name:?} to hold {expected:?}, got {record}"
            );
        }

        // A name the pack does not hold is still absent: a refused batch created nothing.
        for name in &missing {
            let read = batch_get(&router, &pack_id, &[name], case).await;
            assert!(
                results(&read, case).is_empty(),
                "case {case:?}: the update minted a record for {name:?}: {read}"
            );
        }
    }
}

/// RULE: a name the request repeats names ONE record, so it is reported once and the
/// entries are applied in request order — the later write to a member stands.
#[tokio::test]
async fn a_name_repeated_within_one_request_is_applied_in_request_order() {
    let router = emulator();
    let name = "repeated-requirement";

    // (case, the descriptions the repeated entries carry, in request order)
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("the same name twice", vec!["first", "second"]),
        ("the same name three times", vec!["first", "second", "third"]),
        (
            "the same name four times",
            vec!["first", "second", "third", "fourth"],
        ),
    ];

    for (case, descriptions) in rows {
        let pack_id = create_pack(&router, &format!("pack for {case}"), case).await;
        store(&router, &pack_id, &[name], case).await;

        let entries: Vec<Value> = descriptions
            .iter()
            .map(|description| json!({"name": name, "description": description}))
            .collect();
        let answer = batch_update(&router, &pack_id, &entries, case).await;
        assert_eq!(
            updated(&answer, case),
            vec![name.to_string()],
            "case {case:?}: expected the repeated name reported exactly once, got {answer}"
        );

        let last = descriptions
            .last()
            .expect("every row carries at least one entry");
        let record = read_one(&router, &pack_id, name, case).await;
        assert_eq!(
            member(&record, "description").as_deref(),
            Some(*last),
            "case {case:?}: expected the last write {last:?} to stand, got {record}"
        );
    }
}

/// RULE: a name identifies a requirement only WITHIN its pack, so an update addressed to
/// one pack cannot write another pack's record of the same name.
#[tokio::test]
async fn an_update_reaches_only_the_pack_it_names() {
    let router = emulator();
    let case = "the same name in two packs";
    let shared_name = "shared-requirement";

    let target = create_pack(&router, "the pack the update names", case).await;
    let other = create_pack(&router, "the pack beside it", case).await;
    store(&router, &target, &[shared_name], case).await;
    store(&router, &other, &[shared_name], case).await;

    batch_update(
        &router,
        &target,
        &[json!({"name": shared_name, "description": "written in the named pack"})],
        case,
    )
    .await;

    let written = read_one(&router, &target, shared_name, case).await;
    assert_eq!(
        member(&written, "description").as_deref(),
        Some("written in the named pack"),
        "case {case:?}: the update did not write the named pack's record: {written}"
    );
    let untouched = read_one(&router, &other, shared_name, case).await;
    assert_eq!(
        member(&untouched, "description").as_deref(),
        Some(format!("the created description of {shared_name}").as_str()),
        "case {case:?}: the other pack's record of the same name was written too: \
         {untouched}"
    );
}

/// The refusals the update answers for the REQUEST as a whole, over a table of bad
/// requests: an empty pack id, a pack id that names nothing, a blank requirement name and
/// a pack AWS manages. Each one writes nothing.
#[tokio::test]
async fn whole_request_refusals() {
    let router = emulator();

    // An empty pack id names no pack and never could: a bad request, not a missing one.
    expect_refusal(
        &router,
        "BatchUpdateSecurityRequirements",
        &json!({"packId": "", "securityRequirements": [{"name": "anything"}]}),
        400,
        "ValidationException",
        "an empty packId",
    )
    .await;

    // A blank name locates no requirement and never could, so the whole call is refused
    // rather than reporting the entry as a name the pack does not hold. The rows are the
    // forms a blank name takes.
    let case = "a blank requirement name";
    let pack_id = create_pack(&router, "pack for blank names", case).await;
    store(&router, &pack_id, &["kept-requirement"], case).await;
    for blank in ["", " ", "\t", "   "] {
        let refusal = expect_refusal(
            &router,
            "BatchUpdateSecurityRequirements",
            &json!({
                "packId": pack_id,
                "securityRequirements": [
                    {"name": "kept-requirement", "description": "written?"},
                    {"name": blank, "description": "no record to write"}
                ]
            }),
            400,
            "ValidationException",
            case,
        )
        .await;
        assert!(
            member(&refusal, "message").is_some_and(|message| message.contains("name")),
            "case {case:?}: the refusal did not name the member it refused: {refusal}"
        );
        // Refused means nothing was written, including for the entry beside the blank one.
        let kept = read_one(&router, &pack_id, "kept-requirement", case).await;
        assert_eq!(
            member(&kept, "description").as_deref(),
            Some("the created description of kept-requirement"),
            "case {case:?}: a refused request wrote the entry beside the blank name: {kept}"
        );
    }

    // An unknown pack: a requirement is reached through the pack that holds it, so there
    // is no pack to update in — 404 for the whole call, naming the id it could not find.
    let case = "a packId nothing minted";
    let unknown = "00000000000000000000000000000fff";
    let refusal = expect_refusal(
        &router,
        "BatchUpdateSecurityRequirements",
        &json!({
            "packId": unknown,
            "securityRequirements": [{"name": "anything", "description": "nowhere to write"}]
        }),
        404,
        "ResourceNotFoundException",
        case,
    )
    .await;
    assert!(
        member(&refusal, "message").is_some_and(|message| message.contains(unknown)),
        "case {case:?}: the refusal did not name the pack it could not find: {refusal}"
    );

    // The contents of a pack AWS ships are the service's, not a caller's to write: the
    // update conflicts with that pack's state. The row is every entry of the seeded
    // catalog, so the rule is not asserted on one.
    for managed in AWS_MANAGED_PACKS {
        let case = managed.pack_id;
        let refusal = expect_refusal(
            &router,
            "BatchUpdateSecurityRequirements",
            &json!({
                "packId": managed.pack_id,
                "securityRequirements": [{"name": "anything", "description": "not mine"}]
            }),
            409,
            "ConflictException",
            case,
        )
        .await;
        assert!(
            member(&refusal, "message").is_some_and(|message| message.contains(managed.pack_id)),
            "case {case:?}: the refusal did not name the pack: {refusal}"
        );
    }
}
