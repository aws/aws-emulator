// SPDX-License-Identifier: CC0-1.0

//! `ListFinancialRecords`, `GetFinancialRecord` and `BatchGetFinancialRecord` end to
//! end: which records a filter selects, in which order they come back, and how one
//! record is identified.

use serde_json::{json, Value};

use crate::payments_harness::{error_code, ids, list, text, Emulator, HeldRecord};

const LIST: &str = "ListFinancialRecords";
const GET: &str = "GetFinancialRecord";
const BATCH: &str = "BatchGetFinancialRecord";

/// Both record types, as a request's `types` names them.
const BOTH_TYPES: [&str; 2] = ["INVOICE", "CREDIT_MEMO"];

/// The IDs a listing returns for `body`, paging to the end.
async fn listed_ids(emulator: &Emulator, body: &Value) -> Vec<String> {
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    let held = emulator.held_records().len();
    loop {
        let mut request = body.clone();
        if let Some(token) = &next_token {
            request["nextToken"] = json!(token);
        }
        let payload = emulator.ok(LIST, &request).await;
        seen.extend(ids(&payload, "financialRecordSummaries"));
        pages += 1;
        assert!(
            pages <= held + 1,
            "case {body}: paging did not terminate after {pages} pages"
        );
        next_token = payload
            .get("nextToken")
            .and_then(Value::as_str)
            .map(str::to_string);
        if next_token.is_none() {
            break;
        }
    }
    seen
}

/// The held records a case expects back, in the store's insertion order, as a sorted
/// set of IDs.
fn expected_ids(held: &[HeldRecord], select: impl Fn(&HeldRecord) -> bool) -> Vec<String> {
    let mut ids: Vec<String> = held
        .iter()
        .filter(|record| select(record))
        .map(|record| record.id.clone())
        .collect();
    ids.sort();
    ids
}

/// `ids`, sorted, so a set can be compared whatever order the answer came in.
fn as_set(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

#[tokio::test]
async fn the_types_filter_selects_exactly_the_records_of_the_named_types() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    // Every non-empty subset of the modelled `FinancialRecordType` value set, which
    // is what a request may name: the member is required and `uniqueItems`.
    let cases: [Vec<&str>; 3] = [
        vec!["INVOICE"],
        vec!["CREDIT_MEMO"],
        BOTH_TYPES.to_vec(),
    ];
    for types in cases {
        let answered = as_set(listed_ids(&emulator, &json!({ "types": types })).await);
        let expected = expected_ids(&held, |record| types.contains(&record.record_type.as_str()));
        assert!(
            !expected.is_empty(),
            "case {types:?}: the account must hold a record of each type for this to mean anything"
        );
        assert_eq!(
            answered, expected,
            "case {types:?}: expected exactly the held records of those types {expected:?}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn the_settlement_status_filter_selects_by_the_records_settlement_status() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    // Absent means "records with both settlement statuses"; otherwise the named
    // set, which the model bounds at 1..2 unique members.
    let cases: [Option<Vec<&str>>; 4] = [
        None,
        Some(vec!["OPEN"]),
        Some(vec!["CLOSED"]),
        Some(vec!["OPEN", "CLOSED"]),
    ];
    for statuses in cases {
        let mut body = json!({ "types": BOTH_TYPES });
        if let Some(statuses) = &statuses {
            body["settlementStatuses"] = json!(statuses);
        }
        let answered = as_set(listed_ids(&emulator, &body).await);
        let expected = expected_ids(&held, |record| {
            statuses
                .as_ref()
                .is_none_or(|statuses| statuses.contains(&record.settlement_status.as_str()))
        });
        assert!(
            !expected.is_empty(),
            "case {statuses:?}: the account must hold a record of each settlement status"
        );
        assert_eq!(
            answered, expected,
            "case {statuses:?}: expected {expected:?}, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn the_issue_window_is_inclusive_at_its_start_and_exclusive_at_its_end() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let oldest = held
        .iter()
        .map(|record| record.issued_secs)
        .min()
        .expect("the account holds records");
    let newest = held
        .iter()
        .map(|record| record.issued_secs)
        .max()
        .expect("the account holds records");
    let pivot = held
        .iter()
        .map(|record| record.issued_secs)
        .find(|secs| *secs != oldest && *secs != newest)
        .expect("the account holds a record issued between the oldest and the newest");
    struct Case {
        name: &'static str,
        after: Option<i64>,
        before: Option<i64>,
    }
    let cases = [
        Case {
            name: "the whole history",
            after: Some(oldest),
            before: Some(newest + 1),
        },
        Case {
            name: "issuedAfter at a record's own instant includes it",
            after: Some(pivot),
            before: None,
        },
        Case {
            name: "issuedBefore at a record's own instant excludes it",
            after: None,
            before: Some(pivot),
        },
        Case {
            name: "a window around one instant",
            after: Some(pivot),
            before: Some(pivot + 1),
        },
        Case {
            name: "a window before every record",
            after: None,
            before: Some(oldest),
        },
        Case {
            name: "a window after every record",
            after: Some(newest + 1),
            before: None,
        },
    ];
    for case in cases {
        let mut body = json!({ "types": BOTH_TYPES });
        if let Some(after) = case.after {
            body["issuedAfter"] = json!(after);
        }
        if let Some(before) = case.before {
            body["issuedBefore"] = json!(before);
        }
        let answered = as_set(listed_ids(&emulator, &body).await);
        let expected = expected_ids(&held, |record| {
            case.after.is_none_or(|after| record.issued_secs >= after)
                && case
                    .before
                    .is_none_or(|before| record.issued_secs < before)
        });
        assert_eq!(
            answered, expected,
            "case {}: expected {expected:?}, got {answered:?}",
            case.name
        );
    }
}

#[tokio::test]
async fn an_issue_window_that_holds_no_instant_is_refused() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let anchor = held.first().map(|record| record.issued_secs).unwrap_or(0);
    // "When you specify both issuedAfter and issuedBefore, issuedBefore must be
    // later than issuedAfter": equal is not later, and earlier is not either.
    for (case, after, before) in [
        ("the same instant", anchor, anchor),
        ("issuedBefore one second earlier", anchor, anchor - 1),
        ("issuedBefore a day earlier", anchor, anchor - 86_400),
    ] {
        let (status, payload) = emulator
            .call(
                LIST,
                &json!({ "types": BOTH_TYPES, "issuedAfter": after, "issuedBefore": before }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case {case}: expected 400 for a window that holds no instant, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case}: expected the modelled validation error, got {payload}"
        );
        assert_eq!(
            text(&payload, "reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case}: a member failed validation, so the model gives that reason, got {payload}"
        );
        let named: Vec<String> = list(&payload, "fieldList")
            .iter()
            .map(|field| text(field, "name"))
            .collect();
        assert_eq!(
            named,
            vec!["issuedBefore".to_string()],
            "case {case}: fieldList names the member that must change, got {payload}"
        );
    }
}

#[tokio::test]
async fn the_sort_order_orders_the_records_by_the_instant_they_were_issued() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let issued_at = |id: &str| {
        held.iter()
            .find(|record| record.id == id)
            .map(|record| record.issued_secs)
            .unwrap_or_else(|| panic!("the answer named a record the store does not hold: {id}"))
    };
    let ascending = listed_ids(
        &emulator,
        &json!({ "types": BOTH_TYPES, "sortBy": "ISSUED_AT", "sortOrder": "ASCENDING" }),
    )
    .await;
    let descending = listed_ids(
        &emulator,
        &json!({ "types": BOTH_TYPES, "sortBy": "ISSUED_AT", "sortOrder": "DESCENDING" }),
    )
    .await;
    // "The default is DESCENDING, which returns the newest records first."
    let defaulted = listed_ids(&emulator, &json!({ "types": BOTH_TYPES })).await;
    assert_eq!(
        defaulted, descending,
        "case \"no sortOrder\": the default order is DESCENDING, got {defaulted:?} against {descending:?}"
    );
    for (case, order) in [("ASCENDING", &ascending), ("DESCENDING", &descending)] {
        let instants: Vec<i64> = order.iter().map(|id| issued_at(id)).collect();
        let sorted = match case {
            "ASCENDING" => instants.windows(2).all(|pair| pair[0] <= pair[1]),
            _ => instants.windows(2).all(|pair| pair[0] >= pair[1]),
        };
        assert!(
            sorted,
            "case {case}: the records must come back ordered by issuedAt, got {instants:?}"
        );
    }
    let mut reversed = descending.clone();
    reversed.reverse();
    assert_eq!(
        ascending, reversed,
        "case \"both orders\": one order is the reverse of the other, got {ascending:?} and {descending:?}"
    );
}

#[tokio::test]
async fn paging_is_stable_under_one_filter_set() {
    let emulator = Emulator::new();
    let held = emulator.held_records().len();
    let filter = json!({
        "types": BOTH_TYPES,
        "settlementStatuses": ["OPEN"],
        "sortOrder": "ASCENDING",
    });
    let unpaged = listed_ids(&emulator, &filter).await;
    assert!(
        unpaged.len() >= 3,
        "case \"filter set\": the filter must select enough records to page, got {unpaged:?}"
    );
    // The modelled `maxResults` range is 1..100: one record a page, a few, and more
    // than the account holds.
    for page_size in [1usize, 2, 3, held, 100] {
        let mut paged = filter.clone();
        paged["maxResults"] = json!(page_size);
        let seen = listed_ids(&emulator, &paged).await;
        assert_eq!(
            seen, unpaged,
            "case page size {page_size}: paging must return the same records in the same order as {unpaged:?}, got {seen:?}"
        );
    }
}

#[tokio::test]
async fn a_next_token_is_bound_to_the_filter_set_it_was_minted_under() {
    let emulator = Emulator::new();
    let minted = emulator
        .ok(LIST, &json!({ "types": BOTH_TYPES, "maxResults": 1 }))
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("a listing with more records left hands back a nextToken");
    // The same SET of types, sent in the other order: both lists are modelled
    // `uniqueItems`, so this is the same question and the token resumes it.
    let mut reversed_types = BOTH_TYPES;
    reversed_types.reverse();
    let resumed = emulator
        .ok(
            LIST,
            &json!({ "types": reversed_types, "maxResults": 1, "nextToken": minted.clone() }),
        )
        .await;
    assert_eq!(
        list(&resumed, "financialRecordSummaries").len(),
        1,
        "case \"the same type set in another order\": the token resumes the same listing, got {resumed}"
    );
    // A different question: the token does not resume it.
    for (case, body) in [
        (
            "another type filter",
            json!({ "types": ["INVOICE"], "maxResults": 1, "nextToken": minted.clone() }),
        ),
        (
            "another settlement-status filter",
            json!({ "types": BOTH_TYPES, "settlementStatuses": ["OPEN"], "maxResults": 1, "nextToken": minted.clone() }),
        ),
        (
            "another order",
            json!({ "types": BOTH_TYPES, "sortOrder": "ASCENDING", "maxResults": 1, "nextToken": minted.clone() }),
        ),
        (
            "another window",
            json!({ "types": BOTH_TYPES, "issuedAfter": 0, "maxResults": 1, "nextToken": minted.clone() }),
        ),
    ] {
        let (status, payload) = emulator.call(LIST, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: a token minted for another filter set must be refused, got {status} with {payload}"
        );
        assert_eq!(
            text(&payload, "reason"),
            "INVALID_PAGINATION_TOKEN",
            "case {case}: the model gives this refusal its own reason, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_page_size_outside_the_modelled_range_is_refused() {
    let emulator = Emulator::new();
    // `smithy_query(query="operation", operation="ListFinancialRecords")` reports
    // `maxResults` as range 1..100: both bounds hold, and a value past either is
    // refused rather than clamped.
    for page_size in [0, 101, 1_000] {
        let (status, payload) = emulator
            .call(
                LIST,
                &json!({ "types": BOTH_TYPES, "maxResults": page_size }),
            )
            .await;
        assert_eq!(
            status, 400,
            "case maxResults {page_size}: expected 400 for a page size outside 1..100, got {status} with {payload}"
        );
    }
    for page_size in [1, 100] {
        let payload = emulator
            .ok(
                LIST,
                &json!({ "types": BOTH_TYPES, "maxResults": page_size }),
            )
            .await;
        assert!(
            !list(&payload, "financialRecordSummaries").is_empty(),
            "case maxResults {page_size}: a page size at a bound of the range is served, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_listed_summary_agrees_with_the_full_record_and_the_record_adds_its_details() {
    let emulator = Emulator::new();
    let listing = emulator.ok(LIST, &json!({ "types": BOTH_TYPES })).await;
    let summaries = list(&listing, "financialRecordSummaries");
    assert!(
        !summaries.is_empty(),
        "case \"account inventory\": the account must hold records to summarise"
    );
    for summary in summaries {
        let id = text(summary, "id");
        let payload = emulator.ok(GET, &json!({ "identifier": id.clone() })).await;
        let record = payload.get("financialRecord").cloned().unwrap_or(Value::Null);
        for member in [
            "id",
            "type",
            "businessUnit",
            "amount",
            "balances",
            "settlementStatus",
            "status",
            "issuedAt",
            "closedAt",
        ] {
            assert_eq!(
                record.get(member),
                summary.get(member),
                "case {id}: the summary and the record must agree on {member}, got {record} and {summary}"
            );
        }
        assert!(
            summary.get("details").is_none(),
            "case {id}: a summary carries no type-specific details, got {summary}"
        );
        let details = record
            .get("details")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {id}: the record must carry its details, got {record}"));
        // "The populated member matches `type`: `invoiceDetails` for an `INVOICE`,
        // `creditMemoDetails` for a `CREDIT_MEMO`."
        let expected_member = match text(summary, "type").as_str() {
            "INVOICE" => "invoiceDetails",
            _ => "creditMemoDetails",
        };
        assert_eq!(
            details.keys().collect::<Vec<_>>(),
            vec![expected_member],
            "case {id}: the populated details member must match the record's type, got {record}"
        );
    }
}

#[tokio::test]
async fn a_record_is_found_by_either_of_its_identifiers() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    assert!(
        held.len() >= 2,
        "case \"account inventory\": the account must hold records to look up"
    );
    for record in &held {
        let by_id = emulator
            .ok(GET, &json!({ "identifier": record.id.clone() }))
            .await;
        let by_invoice_id = emulator
            .ok(
                GET,
                &json!({ "invoiceIdentifier": record.invoice_id.clone() }),
            )
            .await;
        assert_eq!(
            by_id, by_invoice_id,
            "case {}: the Payments ID and the invoice ID must name the same record, got {by_id} and {by_invoice_id}",
            record.id
        );
        assert_eq!(
            text(
                &by_id.get("financialRecord").cloned().unwrap_or(Value::Null),
                "id"
            ),
            record.id,
            "case {}: the record returned must be the one asked for, got {by_id}",
            record.id
        );
    }
}

#[tokio::test]
async fn naming_both_identifiers_or_neither_is_refused() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let known = held.first().expect("the account holds records");
    for (case, body) in [
        ("neither", json!({})),
        (
            "both",
            json!({ "identifier": known.id.clone(), "invoiceIdentifier": known.invoice_id.clone() }),
        ),
        (
            "both, where only the invoice ID matches a record",
            json!({ "identifier": "ZZ-0000-0000-0000", "invoiceIdentifier": known.invoice_id.clone() }),
        ),
    ] {
        let (status, payload) = emulator.call(GET, &body).await;
        assert_eq!(
            status, 400,
            "case {case}: exactly one identifier must be named, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case}: expected the modelled validation error, got {payload}"
        );
        assert_eq!(
            text(&payload, "reason"),
            "FIELD_VALIDATION_FAILED",
            "case {case}: the refusal is about the request's members, got {payload}"
        );
        let named: Vec<String> = list(&payload, "fieldList")
            .iter()
            .map(|field| text(field, "name"))
            .collect();
        assert_eq!(
            named,
            vec![
                "identifier".to_string(),
                "invoiceIdentifier".to_string()
            ],
            "case {case}: fieldList names both members the rule is about, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_key_no_record_on_the_account_matches_is_not_found() {
    let emulator = Emulator::new();
    // Each identifier satisfies its member's pattern, so the framework passes it to
    // the handler: an unheld Payments ID in both its lengths, and invoice IDs the
    // account has no record for.
    for (member, value, resource) in [
        ("identifier", "ZZ-0000-0000-0000", "ZZ-0000-0000-0000"),
        (
            "identifier",
            "ZZZ-9999-9999-9999-9999",
            "ZZZ-9999-9999-9999-9999",
        ),
        ("invoiceIdentifier", "AWSINV-0000-0000", "AWSINV-0000-0000"),
        ("invoiceIdentifier", "not an invoice", "not an invoice"),
    ] {
        let (status, payload) = emulator.call(GET, &json!({ member: value })).await;
        assert_eq!(
            status, 404,
            "case {member}={value}: expected 404 for a key the account has no record for, got {status} with {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ResourceNotFoundException",
            "case {member}={value}: expected the modelled not-found error, got {payload}"
        );
        assert_eq!(
            text(&payload, "resourceId"),
            resource,
            "case {member}={value}: the error names the resource that wasn't found, got {payload}"
        );
        assert_eq!(
            text(&payload, "resourceType"),
            "financial-record",
            "case {member}={value}: the error names the type of the resource, got {payload}"
        );
    }
}

#[tokio::test]
async fn a_batch_answers_every_key_once_and_one_missing_key_does_not_fail_it() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let first = held.first().expect("the account holds records");
    let second = held.get(1).expect("the account holds two records");
    struct Case {
        name: &'static str,
        keys: Vec<Value>,
        found: Vec<String>,
        missing: Vec<Value>,
    }
    let cases = [
        Case {
            name: "every key resolves",
            keys: vec![
                json!({ "identifier": first.id }),
                json!({ "invoiceIdentifier": second.invoice_id }),
            ],
            found: vec![first.id.clone(), second.id.clone()],
            missing: vec![],
        },
        Case {
            name: "one key of two is missing",
            keys: vec![
                json!({ "identifier": first.id }),
                json!({ "identifier": "ZZ-0000-0000-0000" }),
            ],
            found: vec![first.id.clone()],
            missing: vec![json!({ "identifier": "ZZ-0000-0000-0000" })],
        },
        Case {
            name: "no key resolves",
            keys: vec![
                json!({ "identifier": "ZZ-0000-0000-0000" }),
                json!({ "invoiceIdentifier": "AWSINV-0000-0000" }),
            ],
            found: vec![],
            missing: vec![
                json!({ "identifier": "ZZ-0000-0000-0000" }),
                json!({ "invoiceIdentifier": "AWSINV-0000-0000" }),
            ],
        },
        Case {
            name: "the same record under both of its keys",
            keys: vec![
                json!({ "identifier": first.id }),
                json!({ "invoiceIdentifier": first.invoice_id }),
            ],
            found: vec![first.id.clone(), first.id.clone()],
            missing: vec![],
        },
    ];
    for case in cases {
        let payload = emulator
            .ok(BATCH, &json!({ "keys": case.keys.clone() }))
            .await;
        let records = list(&payload, "financialRecords");
        let errors = list(&payload, "errors");
        assert_eq!(
            records.len() + errors.len(),
            case.keys.len(),
            "case {}: every requested key appears once, got {payload}",
            case.name
        );
        assert_eq!(
            as_set(records.iter().map(|record| text(record, "id")).collect()),
            as_set(case.found.clone()),
            "case {}: expected the records for the keys that resolve, got {payload}",
            case.name
        );
        let returned_keys: Vec<Value> = errors
            .iter()
            .map(|error| error.get("key").cloned().unwrap_or(Value::Null))
            .collect();
        assert_eq!(
            returned_keys, case.missing,
            "case {}: each error carries the key as it was requested, got {payload}",
            case.name
        );
        for error in errors {
            assert_eq!(
                text(error, "code"),
                "NOT_FOUND",
                "case {}: a key no record matches is NOT_FOUND, got {error}",
                case.name
            );
            assert_eq!(
                text(error, "message"),
                "No financial record on your account matches the key.",
                "case {}: the error describes why the record couldn't be retrieved, got {error}",
                case.name
            );
        }
        // The batch and the single get cannot disagree about a record.
        for record in records {
            let id = text(record, "id");
            let single = emulator
                .ok(GET, &json!({ "identifier": id.clone() }))
                .await;
            assert_eq!(
                single.get("financialRecord"),
                Some(record),
                "case {}: the batch and GetFinancialRecord must return the same record for {id}, got {single}",
                case.name
            );
        }
    }
}
