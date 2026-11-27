// SPDX-License-Identifier: CC0-1.0

//! End-to-end restJson1 tests for MERGING ONE CASE INTO ANOTHER, driven through
//! the real `MultiServiceRouter` so every assertion goes over the wire the SDK
//! speaks.
//!
//! The rule under test: `StartMergeCases` starts a merge of one secondary case
//! into the primary case the request addresses and answers before it has run;
//! `GetMergeCasesStatus` reports that merge as `IN_PROGRESS` for either of its
//! two cases, as `COMPLETED` for the secondary case once the merge has
//! finished, and reports no merge at all for any other case. While the merge
//! runs the secondary case refuses every modification and neither case can be
//! deleted; once it has finished the primary case holds the selected field
//! values of the secondary case and the related items of the selected types,
//! the two cases are linked, the secondary case is closed, and each case
//! records one `Case.Merged` audit event carrying the merge's own metadata.
//!
//! The tables below are the class, not one recorded call: their rows differ in
//! which rule of the merge they break, which of the two cases breaks it, which
//! field the merge selects (every system field a merge may take, and the
//! reserved fields it may not), and which operation is put to a case while its
//! merge runs.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_connectcases_amazonconnectcases::state::{
    CASE_STATUS_CLOSED, MERGEABLE_SYSTEM_FIELDS, MERGE_STATUS_COMPLETED, MERGE_STATUS_IN_PROGRESS,
    RESERVED_SYSTEM_FIELDS,
};

use crate::harness::{
    domain, field_of_type, fields_of_every_type, is_minted_id, string_at, template, title_value,
    Emulator,
};

/// How long a poll waits for a merge the service already owes to have been
/// applied.
///
/// Generous on purpose: the span a merge runs for is the service's, not this
/// suite's, so the suite waits for the CONDITION — the status the merge reaches
/// — and only gives up after a deadline no loaded host can miss.
const MERGE_DEADLINE: Duration = Duration::from_secs(60);

/// How long one turn of a poll waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// An Amazon Connect user ARN, for the entity a request names as having
/// performed the merge.
const AGENT_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/agent/a-1";

/// An Amazon Connect attached-file ARN, as a `File` item links to one: the type
/// a merge never moves.
const FILE_ARN: &str = "arn:aws:connect:us-east-1:000000000000:instance/i-1/file/f-1";

/// Two customer-profile ARNs: the customer a case references, which both cases
/// of a merge have to agree on.
const CUSTOMER_ARN: &str = "arn:aws:profile:us-east-1:000000000000:domains/d-1/profiles/p-1";
const OTHER_CUSTOMER_ARN: &str = "arn:aws:profile:us-east-1:000000000000:domains/d-1/profiles/p-2";

/// The `AuditEventType` a finished merge records on each of its two cases, one
/// of `AUDIT_EVENT_TYPE_VALUES`.
const AUDIT_EVENT_CASE_MERGED: &str = "Case.Merged";

/// A case status that is not the closed one, for the row that reopens a case
/// the merge closed.
const CASE_STATUS_OPEN: &str = "Open";

/// The `StartMergeCases` path of one primary case.
fn merge_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/merge")
}

/// The `GetMergeCasesStatus` path of one case.
fn merge_status_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/merge-status")
}

/// The `CreateRelatedItem` path of one case.
fn related_items_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}/related-items/")
}

/// The `UpdateCase` / `DeleteCase` path of one case.
fn case_path(domain_id: &str, case_id: &str) -> String {
    format!("/domains/{domain_id}/cases/{case_id}")
}

/// One field value entry of a case request.
fn field(id: &str, value: Value) -> Value {
    json!({ "id": id, "value": value })
}

/// A string-valued field entry, as every `Text` and `SingleSelect` field takes
/// one.
fn text(id: &str, value: &str) -> Value {
    field(id, json!({ "stringValue": value }))
}

/// The domain two cases are merged in, and the custom fields a merge selects.
struct Fixture {
    domain_id: String,
    template_id: String,
    text_field: String,
    number_field: String,
}

/// A domain with one field of every modelled type and a template requiring
/// nothing, so a case can be created with whatever values a row needs.
async fn merge_fixture(emulator: &Emulator, name: &str) -> Fixture {
    let domain_id = domain(emulator, name).await;
    let fields = fields_of_every_type(emulator, &domain_id).await;
    let text_field = field_of_type(&fields, "Text");
    let number_field = field_of_type(&fields, "Number");
    let template_id = template(emulator, &domain_id, &format!("{name}-template"), &[]).await;
    Fixture {
        domain_id,
        template_id,
        text_field,
        number_field,
    }
}

/// Create a case carrying `fields` beside the `title` every create must supply,
/// and return the id the service minted.
async fn case_with(
    emulator: &Emulator,
    label: &str,
    fixture: &Fixture,
    fields: Vec<Value>,
) -> String {
    // A row that writes `title` itself supplies the mandatory value, so the
    // body carries one entry for it rather than two.
    let names_title = fields
        .iter()
        .any(|entry| entry.get("id").and_then(Value::as_str) == Some("title"));
    let mut body = if names_title {
        Vec::new()
    } else {
        vec![title_value(label)]
    };
    body.extend(fields);
    let created = emulator
        .ok(
            label,
            "POST",
            &format!("/domains/{}/cases", fixture.domain_id),
            json!({ "templateId": fixture.template_id, "fields": body }),
        )
        .await;
    string_at(&created, "caseId", label)
}

/// Two fresh cases of the fixture's domain, carrying nothing but their titles:
/// one row's primary and secondary case, so no row is refused by the state
/// another row left behind.
async fn pair_of_cases(emulator: &Emulator, label: &str, fixture: &Fixture) -> (String, String) {
    let primary = case_with(emulator, &format!("{label}: the primary case"), fixture, vec![]).await;
    let secondary =
        case_with(emulator, &format!("{label}: the secondary case"), fixture, vec![]).await;
    (primary, secondary)
}

/// `GetMergeCasesStatus` for one case, as its HTTP status and body.
async fn merge_status(emulator: &Emulator, domain_id: &str, case_id: &str) -> (u16, Value) {
    emulator
        .send(
            "POST",
            &merge_status_path(domain_id, case_id),
            Some(json!({})),
        )
        .await
}

/// The `status` member `GetMergeCasesStatus` reports for one case, or `None`
/// when it answers the modelled 404 instead — the answer for a case that takes
/// part in no merge.
async fn reported_status(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    case_id: &str,
) -> Option<String> {
    let (status, body) = merge_status(emulator, domain_id, case_id).await;
    assert!(
        status == 200 || status == 404,
        "case {label:?}: GetMergeCasesStatus answers either a merge's status or the modelled \
         404, got {status} with body {body:?}"
    );
    (status == 200).then(|| string_at(&body, "status", label))
}

/// Poll `GetMergeCasesStatus` for one case until it reports `wanted`, which it
/// must within [`MERGE_DEADLINE`], and hand back that answer.
///
/// The merge runs asynchronously, so the suite waits for the CONDITION rather
/// than for a span of its own: it looks again until the status the service owes
/// is the one it reports.
async fn wait_for_status(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    case_id: &str,
    wanted: &str,
) -> Value {
    let deadline = ::std::time::Instant::now() + MERGE_DEADLINE;
    loop {
        let (status, body) = merge_status(emulator, domain_id, case_id).await;
        if status == 200 && body.get("status").and_then(Value::as_str) == Some(wanted) {
            return body;
        }
        assert!(
            ::std::time::Instant::now() < deadline,
            "case {label:?}: expected GetMergeCasesStatus to report {wanted:?} for {case_id}, \
             last answer was {status} with body {body:?}"
        );
        ::tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Every audit event `GetCaseAuditEvents` reports for one case.
async fn audit_events(
    emulator: &Emulator,
    label: &str,
    domain_id: &str,
    case_id: &str,
) -> Vec<Value> {
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
        .clone()
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
            &case_path(domain_id, case_id),
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

/// Every related item of one domain, as `(caseId, content)` pairs, read back
/// through the search of the whole domain so each item is reported under the
/// case it is associated with.
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

/// The contents of the related items one case holds, in the order the search
/// reported them.
fn items_of<'a>(items: &'a [(String, Value)], case_id: &str) -> Vec<&'a Value> {
    items
        .iter()
        .filter(|(held_by, _)| held_by == case_id)
        .map(|(_, content)| content)
        .collect()
}

#[tokio::test]
async fn a_merge_runs_then_moves_what_it_selected_and_closes_the_secondary_case() {
    let emulator = Emulator::new();
    let fixture = merge_fixture(&emulator, "merge-lifecycle-domain").await;
    let domain_id = fixture.domain_id.clone();
    let primary = case_with(
        &emulator,
        "the primary case",
        &fixture,
        vec![
            text(&fixture.text_field, "the primary value"),
            text("customer_id", CUSTOMER_ARN),
        ],
    )
    .await;
    let secondary = case_with(
        &emulator,
        "the secondary case",
        &fixture,
        vec![
            text(&fixture.text_field, "the secondary value"),
            text("summary", "what the secondary case was about"),
        ],
    )
    .await;

    // One comment on the secondary case, which the merge is told to move, and
    // one file association, which a merge never moves.
    emulator
        .ok(
            "a comment to move",
            "POST",
            &related_items_path(&domain_id, &secondary),
            json!({
                "type": "Comment",
                "content": {
                    "comment": { "body": "moved by the merge", "contentType": "Text/Plain" }
                }
            }),
        )
        .await;
    emulator
        .ok(
            "a file that stays",
            "POST",
            &related_items_path(&domain_id, &secondary),
            json!({ "type": "File", "content": { "file": { "fileArn": FILE_ARN } } }),
        )
        .await;

    // Neither case takes part in a merge yet.
    for (label, case_id) in [
        ("the primary case before the merge", &primary),
        ("the secondary case before the merge", &secondary),
    ] {
        assert_eq!(
            reported_status(&emulator, label, &domain_id, case_id).await,
            None,
            "case {label:?}: a case in no merge has no merge status"
        );
    }

    let started = emulator
        .ok(
            "StartMergeCases",
            "POST",
            &merge_path(&domain_id, &primary),
            json!({
                "secondaryCaseIds": [secondary],
                "fieldsConfiguration": [{
                    "fieldIds": [{ "id": fixture.text_field }, { "id": "summary" }],
                    "sourceCaseId": secondary,
                }],
                "relatedItemsConfiguration": { "comment": true },
                "performedBy": { "userArn": AGENT_ARN },
            }),
        )
        .await;
    assert_eq!(
        started,
        json!({}),
        "case \"StartMergeCases\": the operation's output declares no member, so a success \
         carries an empty object, got {started:?}"
    );

    // The merge runs, so both of its cases report it, each with the same two
    // identifiers.
    for (label, case_id) in [
        ("the primary case while merging", &primary),
        ("the secondary case while merging", &secondary),
    ] {
        let (status, body) = merge_status(&emulator, &domain_id, case_id).await;
        assert_eq!(
            (
                status,
                body.get("status").and_then(Value::as_str),
                body.get("primaryCaseId").and_then(Value::as_str),
                body.get("secondaryCaseIds").cloned(),
            ),
            (
                200,
                Some(MERGE_STATUS_IN_PROGRESS),
                Some(primary.as_str()),
                Some(json!([secondary])),
            ),
            "case {label:?}: a running merge is reported for both of its cases, got {status} \
             {body:?}"
        );
    }

    // While it runs, the secondary case refuses every modification and neither
    // case can be deleted. The rows differ in which operation is put to which
    // of the two cases.
    let held: Vec<(&str, &str, String, Option<Value>)> = vec![
        (
            "UpdateCase on the secondary case while merging",
            "PUT",
            case_path(&domain_id, &secondary),
            Some(json!({ "fields": [text(&fixture.text_field, "a write while merging")] })),
        ),
        (
            "CreateRelatedItem on the secondary case while merging",
            "POST",
            related_items_path(&domain_id, &secondary),
            Some(json!({
                "type": "Comment",
                "content": { "comment": { "body": "while merging", "contentType": "Text/Plain" } }
            })),
        ),
        (
            "DeleteCase on the secondary case while merging",
            "DELETE",
            case_path(&domain_id, &secondary),
            None,
        ),
        (
            "DeleteCase on the primary case while merging",
            "DELETE",
            case_path(&domain_id, &primary),
            None,
        ),
    ];
    for (label, method, path, body) in &held {
        let (status, code) = emulator.failure(label, method, path, body.clone()).await;
        assert_eq!(
            (status, code.as_str()),
            (409, "ConflictException"),
            "case {label:?}: a request a running merge holds is refused as a conflict, got \
             {status} {code}"
        );
    }
    // The PRIMARY case stays writable while its merge runs: only the secondary
    // case and the delete of either are held.
    emulator
        .ok(
            "UpdateCase on the primary case while merging",
            "PUT",
            &case_path(&domain_id, &primary),
            json!({ "fields": [field(&fixture.number_field, json!({ "doubleValue": 7.0 }))] }),
        )
        .await;

    // The completion is owed by the clock, so it lands without anything asking
    // for it: the secondary case reports the finished merge.
    let completed = wait_for_status(
        &emulator,
        "the secondary case after the merge",
        &domain_id,
        &secondary,
        MERGE_STATUS_COMPLETED,
    )
    .await;
    assert_eq!(
        (
            completed.get("primaryCaseId").and_then(Value::as_str),
            completed.get("secondaryCaseIds").cloned(),
        ),
        (Some(primary.as_str()), Some(json!([secondary]))),
        "case \"the secondary case after the merge\": the finished merge is reported with both \
         of its cases, got {completed:?}"
    );
    // "For any other case, it returns a ResourceNotFoundException. This
    // includes the primary case after its merge finishes."
    assert_eq!(
        reported_status(
            &emulator,
            "the primary case after the merge",
            &domain_id,
            &primary
        )
        .await,
        None,
        "case \"the primary case after the merge\": the primary case of a finished merge takes \
         part in no merge"
    );

    // The selected values are on the primary case, the value it wrote itself is
    // untouched, and the secondary case is closed.
    assert_eq!(
        read_field(
            &emulator,
            "the merged text value",
            &domain_id,
            &primary,
            &fixture.text_field
        )
        .await,
        Some(json!({ "stringValue": "the secondary value" })),
        "case \"the merged text value\": a selected field takes the secondary case's value"
    );
    assert_eq!(
        read_field(&emulator, "the merged summary", &domain_id, &primary, "summary").await,
        Some(json!({ "stringValue": "what the secondary case was about" })),
        "case \"the merged summary\": a selected system field takes the secondary case's value"
    );
    assert_eq!(
        read_field(
            &emulator,
            "the primary case's own value",
            &domain_id,
            &primary,
            &fixture.number_field
        )
        .await,
        Some(json!({ "doubleValue": 7.0 })),
        "case \"the primary case's own value\": a field the merge did not select keeps the \
         primary case's value"
    );
    assert_eq!(
        read_field(
            &emulator,
            "the closed secondary case",
            &domain_id,
            &secondary,
            "status"
        )
        .await,
        Some(json!({ "stringValue": CASE_STATUS_CLOSED })),
        "case \"the closed secondary case\": the merge closes the secondary case"
    );

    // The comment moved to the primary case, the file stayed, and each case
    // holds one link to the other.
    let items = related_items(&emulator, "the merged related items", &domain_id).await;
    let on_primary = items_of(&items, &primary);
    let on_secondary = items_of(&items, &secondary);
    assert!(
        on_primary
            .iter()
            .any(|content| content.get("comment").is_some()),
        "case \"the merged related items\": the comment of a selected type moves to the primary \
         case, got {on_primary:?}"
    );
    assert!(
        on_secondary
            .iter()
            .all(|content| content.get("comment").is_none()),
        "case \"the merged related items\": a moved item leaves the secondary case, got \
         {on_secondary:?}"
    );
    assert!(
        on_secondary
            .iter()
            .any(|content| content.get("file").is_some()),
        "case \"the merged related items\": a file association is never moved, got \
         {on_secondary:?}"
    );
    assert_eq!(
        on_primary
            .iter()
            .filter_map(|content| content.get("connectCase"))
            .cloned()
            .collect::<Vec<Value>>(),
        vec![json!({ "caseId": secondary })],
        "case \"the merged related items\": the merge links the primary case to the secondary \
         case, got {on_primary:?}"
    );
    assert_eq!(
        on_secondary
            .iter()
            .filter_map(|content| content.get("connectCase"))
            .cloned()
            .collect::<Vec<Value>>(),
        vec![json!({ "caseId": primary })],
        "case \"the merged related items\": the merge links the secondary case to the primary \
         case, got {on_secondary:?}"
    );

    // Both cases record the merge, and the two events carry the SAME merge
    // metadata, which is what groups the changes one merge made.
    let mut merge_event_ids: Vec<String> = Vec::new();
    for (label, case_id) in [
        ("the primary case's history", &primary),
        ("the secondary case's history", &secondary),
    ] {
        let events = audit_events(&emulator, label, &domain_id, case_id).await;
        let merged: Vec<&Value> = events
            .iter()
            .filter(|event| {
                event.get("type").and_then(Value::as_str) == Some(AUDIT_EVENT_CASE_MERGED)
            })
            .collect();
        assert_eq!(
            merged.len(),
            1,
            "case {label:?}: a finished merge records exactly one {AUDIT_EVENT_CASE_MERGED} \
             event, got {events:?}"
        );
        let metadata = merged[0]
            .get("mergeMetadata")
            .cloned()
            .unwrap_or_else(|| panic!("case {label:?}: no mergeMetadata in {:?}", merged[0]));
        assert_eq!(
            (
                metadata.get("primaryCaseId").and_then(Value::as_str),
                metadata.get("secondaryCaseId").and_then(Value::as_str),
            ),
            (Some(primary.as_str()), Some(secondary.as_str())),
            "case {label:?}: the merge metadata names both cases of the merge, got {metadata:?}"
        );
        let merge_event_id = string_at(&metadata, "mergeEventId", label);
        assert!(
            is_minted_id(&merge_event_id),
            "case {label:?}: the mergeEventId must be a server-minted opaque id, got \
             {merge_event_id:?}"
        );
        merge_event_ids.push(merge_event_id);
        // "Present only on audit events produced by a merge": every other
        // event of the case carries none.
        for event in &events {
            if event.get("type").and_then(Value::as_str) != Some(AUDIT_EVENT_CASE_MERGED) {
                assert_eq!(
                    event.get("mergeMetadata"),
                    None,
                    "case {label:?}: only an event a merge produced carries mergeMetadata, got \
                     {event:?}"
                );
            }
        }
    }
    assert_eq!(
        merge_event_ids[0], merge_event_ids[1],
        "case \"the merge metadata\": both cases of one merge record the same mergeEventId, got \
         {merge_event_ids:?}"
    );

    // "After the merge, the secondary case can be reopened, updated, and
    // deleted, but it can't be merged again."
    emulator
        .ok(
            "UpdateCase on the secondary case after the merge",
            "PUT",
            &case_path(&domain_id, &secondary),
            json!({ "fields": [text("status", CASE_STATUS_OPEN)] }),
        )
        .await;
    let third = case_with(&emulator, "a third case", &fixture, vec![]).await;
    let (status, code) = emulator
        .failure(
            "a second merge of a case already merged away",
            "POST",
            &merge_path(&domain_id, &third),
            Some(json!({ "secondaryCaseIds": [secondary] })),
        )
        .await;
    assert_eq!(
        (status, code.as_str()),
        (409, "ConflictException"),
        "case \"a second merge of a case already merged away\": a case merged into another one \
         as the secondary case cannot be merged again, got {status} {code}"
    );
}

/// One row of the refusal table: how a failing assertion names it, the answer
/// the service owes it, the request it owes that answer to, and the primary
/// case the row addressed — which must take part in no merge afterwards,
/// because a refused request starts none.
struct Refusal {
    label: String,
    status: u16,
    code: &'static str,
    path: String,
    body: Value,
    primary: String,
}

#[tokio::test]
async fn a_merge_is_refused_before_it_starts_for_each_rule_the_service_states() {
    let emulator = Emulator::new();
    let fixture = merge_fixture(&emulator, "merge-refusal-domain").await;
    let domain_id = fixture.domain_id.clone();
    for row in refusal_rows(&emulator, &fixture).await {
        let (status, code) = emulator
            .failure(&row.label, "POST", &row.path, Some(row.body.clone()))
            .await;
        assert_eq!(
            (status, code.as_str()),
            (row.status, row.code),
            "case {:?}: expected {} {}, got {status} {code}",
            row.label,
            row.status,
            row.code
        );
        assert_eq!(
            reported_status(&emulator, &row.label, &domain_id, &row.primary).await,
            None,
            "case {:?}: a refused merge starts none, so its primary case takes part in no merge",
            row.label
        );
    }
}

/// Every row of the refusal table: the rules `StartMergeCases` states for a
/// request it refuses before any merge starts — a case merged into itself, a
/// case the domain does not hold, a closed case (either of the two), two cases
/// with different customers, a selection naming a field the domain has no
/// record of, one the secondary case holds no value for, and a selection whose
/// source is not the secondary case.
async fn refusal_rows(emulator: &Emulator, fixture: &Fixture) -> Vec<Refusal> {
    let domain_id = fixture.domain_id.clone();
    let mut rows: Vec<Refusal> = Vec::new();

    let (same, _) = pair_of_cases(emulator, "one case merged into itself", fixture).await;
    rows.push(Refusal {
        label: "one case merged into itself".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &same),
        body: json!({ "secondaryCaseIds": [same] }),
        primary: same.clone(),
    });

    let (unknown, _) =
        pair_of_cases(emulator, "a secondary case the domain lacks", fixture).await;
    rows.push(Refusal {
        label: "a secondary case the domain lacks".to_string(),
        status: 404,
        code: "ResourceNotFoundException",
        path: merge_path(&domain_id, &unknown),
        body: json!({ "secondaryCaseIds": ["11111111-2222-3333-4444-555555555555"] }),
        primary: unknown.clone(),
    });

    let closed_primary = case_with(
        emulator,
        "a closed primary case",
        fixture,
        vec![text("status", CASE_STATUS_CLOSED)],
    )
    .await;
    let (_, open_secondary) = pair_of_cases(emulator, "a closed primary case", fixture).await;
    rows.push(Refusal {
        label: "a closed primary case".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &closed_primary),
        body: json!({ "secondaryCaseIds": [open_secondary] }),
        primary: closed_primary.clone(),
    });

    let (open_primary, _) = pair_of_cases(emulator, "a closed secondary case", fixture).await;
    let closed_secondary = case_with(
        emulator,
        "a closed secondary case",
        fixture,
        vec![text("status", CASE_STATUS_CLOSED)],
    )
    .await;
    rows.push(Refusal {
        label: "a closed secondary case".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &open_primary),
        body: json!({ "secondaryCaseIds": [closed_secondary] }),
        primary: open_primary.clone(),
    });

    let one_customer = case_with(
        emulator,
        "a case of one customer",
        fixture,
        vec![text("customer_id", CUSTOMER_ARN)],
    )
    .await;
    let another_customer = case_with(
        emulator,
        "a case of another customer",
        fixture,
        vec![text("customer_id", OTHER_CUSTOMER_ARN)],
    )
    .await;
    rows.push(Refusal {
        label: "two cases with different customers".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &one_customer),
        body: json!({ "secondaryCaseIds": [another_customer] }),
        primary: one_customer.clone(),
    });

    let (unknown_field_primary, unknown_field_secondary) =
        pair_of_cases(emulator, "a selection naming an unknown field", fixture).await;
    rows.push(Refusal {
        label: "a selection naming an unknown field".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &unknown_field_primary),
        body: json!({
            "secondaryCaseIds": [unknown_field_secondary],
            "fieldsConfiguration": [{
                "fieldIds": [{ "id": "a-field-this-domain-does-not-hold" }],
                "sourceCaseId": unknown_field_secondary,
            }],
        }),
        primary: unknown_field_primary.clone(),
    });

    let (unheld_primary, unheld_secondary) = pair_of_cases(
        emulator,
        "a selection naming a field the secondary case lacks",
        fixture,
    )
    .await;
    rows.push(Refusal {
        label: "a selection naming a field the secondary case lacks".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &unheld_primary),
        body: json!({
            "secondaryCaseIds": [unheld_secondary],
            "fieldsConfiguration": [{
                "fieldIds": [{ "id": fixture.text_field }],
                "sourceCaseId": unheld_secondary,
            }],
        }),
        primary: unheld_primary.clone(),
    });

    let (sourced_primary, sourced_secondary) =
        pair_of_cases(emulator, "a selection sourced from the primary case", fixture).await;
    rows.push(Refusal {
        label: "a selection sourced from the primary case".to_string(),
        status: 400,
        code: "ValidationException",
        path: merge_path(&domain_id, &sourced_primary),
        body: json!({
            "secondaryCaseIds": [sourced_secondary],
            "fieldsConfiguration": [{
                "fieldIds": [{ "id": "summary" }],
                "sourceCaseId": sourced_primary,
            }],
        }),
        primary: sourced_primary.clone(),
    });

    rows
}

#[tokio::test]
async fn a_merge_takes_every_mergeable_system_field_and_no_other_reserved_one() {
    let emulator = Emulator::new();
    let fixture = merge_fixture(&emulator, "merge-field-catalog-domain").await;
    let domain_id = fixture.domain_id.clone();

    // The catalog is the set the model's own documentation names — "You can
    // only merge custom fields and the following system fields: assigned_queue,
    // assigned_user, summary, and case_reason"
    // (`smithy_query(query="operation", operation="StartMergeCases")`, member
    // `fieldsConfiguration`) — and every entry is put to the service, including
    // the ones no single example names.
    for field_id in MERGEABLE_SYSTEM_FIELDS {
        let label = format!("a merge of the system field {field_id}");
        let primary = case_with(&emulator, &label, &fixture, vec![]).await;
        let secondary = case_with(
            &emulator,
            &label,
            &fixture,
            vec![text(field_id, "the secondary case's value")],
        )
        .await;
        emulator
            .ok(
                &label,
                "POST",
                &merge_path(&domain_id, &primary),
                json!({
                    "secondaryCaseIds": [secondary],
                    "fieldsConfiguration": [{
                        "fieldIds": [{ "id": field_id }],
                        "sourceCaseId": secondary,
                    }],
                }),
            )
            .await;
        assert_eq!(
            reported_status(&emulator, &label, &domain_id, &primary).await,
            Some(MERGE_STATUS_IN_PROGRESS.to_string()),
            "case {label:?}: a merge of a mergeable system field starts"
        );
        let merged =
            wait_for_status(&emulator, &label, &domain_id, &secondary, MERGE_STATUS_COMPLETED)
                .await;
        assert_eq!(
            merged.get("primaryCaseId").and_then(Value::as_str),
            Some(primary.as_str()),
            "case {label:?}: the finished merge names the primary case, got {merged:?}"
        );
        assert_eq!(
            read_field(&emulator, &label, &domain_id, &primary, field_id).await,
            Some(json!({ "stringValue": "the secondary case's value" })),
            "case {label:?}: the primary case takes the secondary case's value for the field"
        );
    }

    // Every OTHER reserved field of a Cases domain is one the service maintains
    // itself, so a selection naming one is refused. The rows are drawn from the
    // same reserved set the domain is provisioned with, so they span the class
    // rather than naming one example.
    let unmergeable: Vec<&str> = RESERVED_SYSTEM_FIELDS
        .iter()
        .filter(|(field_id, _, field_type)| {
            *field_type == "Text" && !MERGEABLE_SYSTEM_FIELDS.contains(field_id)
        })
        .map(|(field_id, _, _)| *field_id)
        .collect();
    assert!(
        unmergeable.len() > 1,
        "the reserved set must hold more than one unmergeable Text field for this table to span \
         the class, got {unmergeable:?}"
    );
    for field_id in unmergeable {
        let label = format!("a merge of the reserved field {field_id}");
        let primary = case_with(&emulator, &label, &fixture, vec![]).await;
        let secondary = case_with(
            &emulator,
            &label,
            &fixture,
            vec![text(field_id, "a reserved value")],
        )
        .await;
        let (status, code) = emulator
            .failure(
                &label,
                "POST",
                &merge_path(&domain_id, &primary),
                Some(json!({
                    "secondaryCaseIds": [secondary],
                    "fieldsConfiguration": [{
                        "fieldIds": [{ "id": field_id }],
                        "sourceCaseId": secondary,
                    }],
                })),
            )
            .await;
        assert_eq!(
            (status, code.as_str()),
            (400, "ValidationException"),
            "case {label:?}: a field the service maintains itself cannot be merged, got {status} \
             {code}"
        );
        assert_eq!(
            reported_status(&emulator, &label, &domain_id, &primary).await,
            None,
            "case {label:?}: a refused merge starts none"
        );
    }
}

#[tokio::test]
async fn a_merge_is_reported_for_no_case_of_another_domain_and_for_no_unknown_case() {
    let emulator = Emulator::new();
    let fixture = merge_fixture(&emulator, "merge-status-scope-domain").await;
    let other = merge_fixture(&emulator, "merge-status-other-domain").await;
    let domain_id = fixture.domain_id.clone();
    let primary = case_with(&emulator, "the primary case", &fixture, vec![]).await;
    let secondary = case_with(
        &emulator,
        "the secondary case",
        &fixture,
        vec![text("summary", "a summary to merge")],
    )
    .await;
    emulator
        .ok(
            "StartMergeCases",
            "POST",
            &merge_path(&domain_id, &primary),
            json!({ "secondaryCaseIds": [secondary] }),
        )
        .await;

    // The rows differ in what the request names: the merging case's id but in
    // another domain, a case id no domain holds, and a domain nothing holds.
    let rows: Vec<(&str, String)> = vec![
        (
            "the merging case id in another domain",
            merge_status_path(&other.domain_id, &primary),
        ),
        (
            "a case id no domain holds",
            merge_status_path(&domain_id, "99999999-8888-7777-6666-555555555555"),
        ),
        (
            "a domain nothing holds",
            merge_status_path("a-domain-nothing-holds", &primary),
        ),
    ];
    for (label, path) in rows {
        let (status, code) = emulator.failure(label, "POST", &path, Some(json!({}))).await;
        assert_eq!(
            (status, code.as_str()),
            (404, "ResourceNotFoundException"),
            "case {label:?}: a merge is reported only for a case of the domain that holds it, \
             got {status} {code}"
        );
    }
}
