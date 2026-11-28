// SPDX-License-Identifier: CC0-1.0

//! A client walking the whole account: seed, list, get, batch-get, aggregate — the
//! lifecycle this service supports, in one sequence, twice.
//!
//! The second walk is the point: every operation here only REPORTS, so two
//! identical requests against one running emulator must answer identically. A
//! response that changed between the walks would mean a read wrote something, or
//! that the store is rebuilt per request instead of shared.

use serde_json::{json, Value};

use crate::payments_harness::{error_code, list, text, Emulator};

/// One walk of the account, as the payload of each call it makes.
async fn walk(emulator: &Emulator) -> Vec<(String, Value)> {
    let mut answers = Vec::new();

    // One instrument at a time, following the token to the next page.
    let first_page = emulator
        .ok("ListPaymentInstruments", &json!({ "maxResults": 1 }))
        .await;
    let summary = list(&first_page, "paymentInstrumentSummaries")
        .first()
        .cloned()
        .expect("the account holds a payment instrument");
    let token = text(&first_page, "nextToken");
    assert!(
        !token.is_empty(),
        "the account holds more instruments, so the first page hands back a nextToken: {first_page}"
    );
    let second_page = emulator
        .ok(
            "ListPaymentInstruments",
            &json!({ "maxResults": 1, "nextToken": token }),
        )
        .await;
    assert_ne!(
        list(&second_page, "paymentInstrumentSummaries")
            .first()
            .map(|next| text(next, "id")),
        Some(text(&summary, "id")),
        "the second page must not repeat the first instrument: {second_page}"
    );
    answers.push(("instruments page 1".to_string(), first_page));
    answers.push(("instruments page 2".to_string(), second_page));

    // The same instrument by ID and by ARN, then one the account does not hold.
    for (case, identifier) in [
        ("by id", text(&summary, "id")),
        ("by arn", text(&summary, "arn")),
    ] {
        let payload = emulator
            .ok("GetPaymentInstrument", &json!({ "identifier": identifier }))
            .await;
        answers.push((format!("instrument {case}"), payload));
    }
    let (status, payload) = emulator
        .call(
            "GetPaymentInstrument",
            &json!({ "identifier": "no-such-instrument" }),
        )
        .await;
    assert_eq!(
        status, 404,
        "an instrument the account does not hold is not found: {payload}"
    );
    answers.push(("instrument not found".to_string(), payload));

    // The newest invoices first, then the same invoice as a full record by each of
    // its two identifiers.
    let invoices = emulator
        .ok(
            "ListFinancialRecords",
            &json!({ "types": ["INVOICE"], "sortBy": "ISSUED_AT", "sortOrder": "DESCENDING" }),
        )
        .await;
    let newest = list(&invoices, "financialRecordSummaries")
        .first()
        .cloned()
        .expect("the account holds an invoice");
    assert_eq!(
        text(&newest, "type"),
        "INVOICE",
        "a listing filtered to invoices returns only invoices: {invoices}"
    );
    let record = emulator
        .ok(
            "GetFinancialRecord",
            &json!({ "identifier": text(&newest, "id") }),
        )
        .await;
    let invoice_id = record
        .get("financialRecord")
        .and_then(|record| record.get("details"))
        .and_then(|details| details.get("invoiceDetails"))
        .map(|invoice| text(invoice, "invoiceId"))
        .expect("an invoice carries its invoice details");
    let by_invoice_id = emulator
        .ok(
            "GetFinancialRecord",
            &json!({ "invoiceIdentifier": invoice_id.clone() }),
        )
        .await;
    assert_eq!(
        record, by_invoice_id,
        "the two identifiers of one record must answer identically: {record} and {by_invoice_id}"
    );
    answers.push(("invoices".to_string(), invoices));
    answers.push(("invoice record".to_string(), record));
    answers.push(("invoice by invoice id".to_string(), by_invoice_id));

    // Both identifier members, and neither.
    for (case, body) in [
        (
            "both",
            json!({ "identifier": text(&newest, "id"), "invoiceIdentifier": invoice_id }),
        ),
        ("neither", json!({})),
    ] {
        let (status, payload) = emulator.call("GetFinancialRecord", &body).await;
        assert_eq!(
            status, 400,
            "case {case}: exactly one identifier must be named: {payload}"
        );
        assert_eq!(
            error_code(&payload),
            "ValidationException",
            "case {case}: expected the modelled validation error: {payload}"
        );
        answers.push((format!("record {case}"), payload));
    }

    // A batch of one key that resolves and one that does not.
    let batch = emulator
        .ok(
            "BatchGetFinancialRecord",
            &json!({ "keys": [
                { "identifier": text(&newest, "id") },
                { "identifier": "ZZ-0000-0000-0000" },
            ] }),
        )
        .await;
    assert_eq!(
        list(&batch, "financialRecords").len(),
        1,
        "one key of the batch resolves: {batch}"
    );
    assert_eq!(
        list(&batch, "errors").len(),
        1,
        "the other key is reported in errors rather than failing the batch: {batch}"
    );
    answers.push(("batch".to_string(), batch));

    // Each balance of the account.
    for balance_type in [
        "INVOICES_DUE",
        "INVOICES_PAST_DUE",
        "OUTSTANDING_CREDIT_MEMOS",
    ] {
        let payload = emulator
            .ok("GetAccountBalance", &json!({ "balanceType": balance_type }))
            .await;
        assert!(
            !list(&payload, "accountBalances").is_empty(),
            "case {balance_type}: the account holds records of this kind: {payload}"
        );
        answers.push((format!("balance {balance_type}"), payload));
    }

    answers
}

#[tokio::test]
async fn walking_the_account_twice_answers_identically() {
    let emulator = Emulator::new();
    let first = walk(&emulator).await;
    let second = walk(&emulator).await;
    assert_eq!(
        first.len(),
        second.len(),
        "both walks make the same calls, got {} and {}",
        first.len(),
        second.len()
    );
    for ((case, before), (_, after)) in first.iter().zip(second.iter()) {
        assert_eq!(
            before, after,
            "case {case}: every operation here only reports, so a second identical request must answer identically; got {before} then {after}"
        );
    }
}
