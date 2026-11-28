// SPDX-License-Identifier: CC0-1.0

//! `GetAccountBalance` end to end: the balance is the sum of the held records'
//! outstanding balances, grouped by merchant of record and currency, and never added
//! up across either.

use serde_json::{json, Value};

use crate::payments_harness::{credit_memo, invoice, list, text, Emulator};

use service_com_amazonaws_payments_awspayments::state::{DueStatus, FinancialRecordStatus};

const BALANCE: &str = "GetAccountBalance";

/// The two merchants of record `BusinessUnit$merchantOfRecord` names as its
/// examples.
const MERCHANT_US: &str = "Amazon Web Services, Inc.";
const MERCHANT_EMEA: &str = "Amazon Web Services EMEA SARL";

/// One amount of a balance, as a case states what it expects.
type Amount = (String, String, String);

/// The amounts `balance_type` reports, paging to the end.
async fn amounts(emulator: &Emulator, balance_type: &str, page_size: Option<usize>) -> Vec<Amount> {
    let mut seen = Vec::new();
    let mut next_token: Option<String> = None;
    let mut pages = 0usize;
    loop {
        let mut body = json!({ "balanceType": balance_type });
        if let Some(page_size) = page_size {
            body["maxResults"] = json!(page_size);
        }
        if let Some(token) = &next_token {
            body["nextToken"] = json!(token);
        }
        let payload = emulator.ok(BALANCE, &body).await;
        for entry in list(&payload, "accountBalances") {
            let amount = entry.get("amount").cloned().unwrap_or(Value::Null);
            seen.push((
                text(entry, "merchantOfRecord"),
                text(&amount, "currency"),
                text(&amount, "value"),
            ));
        }
        pages += 1;
        assert!(
            pages <= seen.len() + 2,
            "case {balance_type} at {page_size:?}: paging did not terminate after {pages} pages"
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

/// The amounts a case expects, in the order the operation reports them: by merchant
/// of record, then by currency.
fn ordered(mut expected: Vec<Amount>) -> Vec<Amount> {
    expected.sort_by(|left, right| (&left.0, &left.1).cmp(&(&right.0, &right.1)));
    expected
}

/// One amount of a balance.
fn amount(merchant: &str, currency: &str, value: &str) -> Amount {
    (
        merchant.to_string(),
        currency.to_string(),
        value.to_string(),
    )
}

/// An account whose records span what the three balance types have to tell apart:
/// two merchants of record, two currencies, one merchant invoicing in both, invoices
/// due and past due, credit memos outstanding and applied, and records in every
/// status that no balance counts.
fn seeded_account() -> Emulator {
    let emulator = Emulator::empty();
    emulator.seed_records(vec![
        // Two outstanding invoices due, same merchant and currency: their
        // outstanding balances add up.
        invoice(
            "IN-1000-0000-0001",
            "INVOICE-0001",
            MERCHANT_US,
            "USD",
            (15_000, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            Some(DueStatus::Due),
            1_700_000_000,
        ),
        // Partly settled and still due: its OUTSTANDING balance counts, not its
        // amount.
        invoice(
            "IN-1000-0000-0002",
            "INVOICE-0002",
            MERCHANT_US,
            "USD",
            (2_550, 0, 10_000, 0),
            FinancialRecordStatus::Outstanding,
            Some(DueStatus::Due),
            1_700_086_400,
        ),
        // The same merchant invoicing in another currency: a separate amount.
        invoice(
            "IN-1000-0000-0003",
            "INVOICE-0003",
            MERCHANT_US,
            "EUR",
            (1_000, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            Some(DueStatus::Due),
            1_700_172_800,
        ),
        // Past its due date: a different balance type.
        invoice(
            "IN-2000-0000-0001",
            "INVOICE-1001",
            MERCHANT_EMEA,
            "USD",
            (9_999, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            Some(DueStatus::PastDue),
            1_700_259_200,
        ),
        // A settlement in progress: "a record that is PROCESSING, SETTLED, or
        // CANCELED isn't included, even when part of its amount is outstanding".
        invoice(
            "IN-1000-0000-0004",
            "INVOICE-0004",
            MERCHANT_US,
            "USD",
            (50_000, 10_000, 0, 0),
            FinancialRecordStatus::Processing,
            None,
            1_700_345_600,
        ),
        // Paid, and voided: neither counts.
        invoice(
            "IN-1000-0000-0005",
            "INVOICE-0005",
            MERCHANT_US,
            "USD",
            (0, 0, 30_000, 0),
            FinancialRecordStatus::Settled,
            None,
            1_700_432_000,
        ),
        invoice(
            "IN-1000-0000-0006",
            "INVOICE-0006",
            MERCHANT_US,
            "USD",
            (0, 0, 0, 7_700),
            FinancialRecordStatus::Canceled,
            None,
            1_700_518_400,
        ),
        // Credit not yet applied, for each merchant.
        credit_memo(
            "CM-1000-0000-0001",
            "CREDIT-0001",
            MERCHANT_US,
            "USD",
            (4_000, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            1_700_604_800,
        ),
        credit_memo(
            "CM-2000-0000-0001",
            "CREDIT-1001",
            MERCHANT_EMEA,
            "EUR",
            (6_000, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            1_700_691_200,
        ),
        // Credit already applied in full: closed, so nothing of it is outstanding
        // and no balance counts it.
        credit_memo(
            "CM-1000-0000-0002",
            "CREDIT-0002",
            MERCHANT_US,
            "USD",
            (0, 0, 2_500, 0),
            FinancialRecordStatus::Settled,
            1_700_777_600,
        ),
    ]);
    emulator
}

#[tokio::test]
async fn each_balance_type_sums_the_outstanding_balances_of_the_records_it_selects() {
    let emulator = seeded_account();
    let cases = [
        (
            "INVOICES_DUE",
            vec![
                amount(MERCHANT_US, "USD", "175.50"),
                amount(MERCHANT_US, "EUR", "10.00"),
            ],
        ),
        (
            "INVOICES_PAST_DUE",
            vec![amount(MERCHANT_EMEA, "USD", "99.99")],
        ),
        (
            "OUTSTANDING_CREDIT_MEMOS",
            vec![
                amount(MERCHANT_US, "USD", "40.00"),
                amount(MERCHANT_EMEA, "EUR", "60.00"),
            ],
        ),
    ];
    for (balance_type, expected) in cases {
        let answered = amounts(&emulator, balance_type, None).await;
        assert_eq!(
            answered,
            ordered(expected.clone()),
            "case {balance_type}: expected {expected:?} grouped by merchant and currency, got {answered:?}"
        );
    }
}

#[tokio::test]
async fn amounts_are_never_added_up_across_merchants_or_currencies() {
    let emulator = Emulator::empty();
    // The same amount, due, in every combination of two merchants and two
    // currencies: four amounts, not one sum and not two.
    let mut records = Vec::new();
    for (index, (merchant, currency)) in [
        (MERCHANT_US, "USD"),
        (MERCHANT_US, "EUR"),
        (MERCHANT_EMEA, "USD"),
        (MERCHANT_EMEA, "EUR"),
    ]
    .into_iter()
    .enumerate()
    {
        records.push(invoice(
            &format!("IN-3000-0000-000{index}"),
            &format!("SPLIT-{index}"),
            merchant,
            currency,
            (11_100, 0, 0, 0),
            FinancialRecordStatus::Outstanding,
            Some(DueStatus::Due),
            1_700_000_000 + i64::try_from(index).unwrap_or(0) * 86_400,
        ));
    }
    emulator.seed_records(records);
    let answered = amounts(&emulator, "INVOICES_DUE", None).await;
    let expected = ordered(vec![
        amount(MERCHANT_US, "USD", "111.00"),
        amount(MERCHANT_US, "EUR", "111.00"),
        amount(MERCHANT_EMEA, "USD", "111.00"),
        amount(MERCHANT_EMEA, "EUR", "111.00"),
    ]);
    assert_eq!(
        answered, expected,
        "case \"two merchants, two currencies\": expected one amount per pair {expected:?}, got {answered:?}"
    );
}

#[tokio::test]
async fn changing_the_held_records_changes_the_balance() {
    let emulator = seeded_account();
    let before = amounts(&emulator, "INVOICES_DUE", None).await;
    // One more invoice, due, for a merchant and currency the balance already
    // reports: its outstanding balance joins that amount and nothing else moves.
    emulator.seed_records(vec![invoice(
        "IN-1000-0000-0099",
        "INVOICE-0099",
        MERCHANT_US,
        "USD",
        (1_025, 0, 0, 0),
        FinancialRecordStatus::Outstanding,
        Some(DueStatus::Due),
        1_700_864_000,
    )]);
    let after = amounts(&emulator, "INVOICES_DUE", None).await;
    let expected = ordered(vec![
        amount(MERCHANT_US, "USD", "185.75"),
        amount(MERCHANT_US, "EUR", "10.00"),
    ]);
    assert_eq!(
        after, expected,
        "case \"one more due invoice\": the balance must follow the records, was {before:?}, expected {expected:?}, got {after:?}"
    );
    // And a record that leaves the account leaves the balance.
    emulator.with_state(|state| state.financial_records.clear());
    let emptied = amounts(&emulator, "INVOICES_DUE", None).await;
    assert!(
        emptied.is_empty(),
        "case \"no records\": an account with no record has no balance to report, got {emptied:?}"
    );
}

#[tokio::test]
async fn the_balance_pages_without_repeating_or_dropping_an_amount() {
    let emulator = Emulator::empty();
    // Five amounts, one per currency, so there is something to page.
    let currencies = ["AUD", "CAD", "EUR", "GBP", "USD"];
    let records: Vec<_> = currencies
        .iter()
        .enumerate()
        .map(|(index, currency)| {
            invoice(
                &format!("IN-4000-0000-000{index}"),
                &format!("PAGE-{index}"),
                MERCHANT_US,
                currency,
                (1_000 + i64::try_from(index).unwrap_or(0), 0, 0, 0),
                FinancialRecordStatus::Outstanding,
                Some(DueStatus::Due),
                1_700_000_000 + i64::try_from(index).unwrap_or(0) * 86_400,
            )
        })
        .collect();
    emulator.seed_records(records);
    let unpaged = amounts(&emulator, "INVOICES_DUE", None).await;
    assert_eq!(
        unpaged.len(),
        currencies.len(),
        "case \"five currencies\": expected one amount per currency, got {unpaged:?}"
    );
    for page_size in [1usize, 2, 3, 5, 100] {
        let paged = amounts(&emulator, "INVOICES_DUE", Some(page_size)).await;
        assert_eq!(
            paged, unpaged,
            "case page size {page_size}: paging must report the same amounts in the same order as {unpaged:?}, got {paged:?}"
        );
    }
}

#[tokio::test]
async fn the_balance_of_the_account_the_emulator_starts_with_is_derived_from_its_records() {
    let emulator = Emulator::new();
    let held = emulator.held_records();
    let mut answers = Vec::new();
    for balance_type in ["INVOICES_DUE", "INVOICES_PAST_DUE", "OUTSTANDING_CREDIT_MEMOS"] {
        let answered = amounts(&emulator, balance_type, None).await;
        assert!(
            !answered.is_empty(),
            "case {balance_type}: the account holds records of this kind, so the balance is not empty"
        );
        // Every amount reported belongs to a (merchant, currency) that an
        // OUTSTANDING record of the selected kind was issued in.
        for (merchant, currency, value) in &answered {
            let contributing: Vec<&String> = held
                .iter()
                .filter(|record| {
                    &record.merchant_of_record == merchant
                        && &record.currency == currency
                        && record.status == "OUTSTANDING"
                        && match balance_type {
                            "INVOICES_DUE" => record.due_status.as_deref() == Some("DUE"),
                            "INVOICES_PAST_DUE" => {
                                record.due_status.as_deref() == Some("PAST_DUE")
                            }
                            _ => record.record_type == "CREDIT_MEMO",
                        }
                })
                .map(|record| &record.id)
                .collect();
            assert!(
                !contributing.is_empty(),
                "case {balance_type}: {merchant} / {currency} reports {value} with no held record behind it"
            );
        }
        answers.push((balance_type, answered));
    }
    // The three balances are three different answers over the same records: a
    // constant list would make them equal.
    for (left, right) in [(0usize, 1usize), (0, 2), (1, 2)] {
        assert_ne!(
            answers[left].1, answers[right].1,
            "case \"{} against {}\": the balance types select different records, got {:?} and {:?}",
            answers[left].0, answers[right].0, answers[left].1, answers[right].1
        );
    }
    // And more than one merchant of record is reported, which is what the grouping
    // is for.
    let merchants: std::collections::BTreeSet<&String> = answers
        .iter()
        .flat_map(|(_, amounts)| amounts.iter().map(|(merchant, _, _)| merchant))
        .collect();
    assert!(
        merchants.len() >= 2,
        "case \"account inventory\": the account must hold records from more than one merchant of record, got {merchants:?}"
    );
}

#[tokio::test]
async fn a_balance_type_the_model_does_not_declare_is_refused() {
    let emulator = Emulator::new();
    for balance_type in ["INVOICES", "invoices_due", "OUTSTANDING"] {
        let (status, payload) = emulator
            .call(BALANCE, &json!({ "balanceType": balance_type }))
            .await;
        assert_eq!(
            status, 400,
            "case {balance_type}: a value outside the modelled set must be refused, got {status} with {payload}"
        );
    }
}

#[tokio::test]
async fn a_next_token_minted_for_another_balance_type_is_refused() {
    let emulator = seeded_account();
    let minted = emulator
        .ok(
            BALANCE,
            &json!({ "balanceType": "INVOICES_DUE", "maxResults": 1 }),
        )
        .await
        .get("nextToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .expect("a balance with more amounts left hands back a nextToken");
    let (status, payload) = emulator
        .call(
            BALANCE,
            &json!({ "balanceType": "OUTSTANDING_CREDIT_MEMOS", "maxResults": 1, "nextToken": minted.clone() }),
        )
        .await;
    assert_eq!(
        status, 400,
        "case \"another balance type\": a token minted for one balance must not resume another, got {status} with {payload}"
    );
    assert_eq!(
        text(&payload, "reason"),
        "INVALID_PAGINATION_TOKEN",
        "case \"another balance type\": the model gives this refusal its own reason, got {payload}"
    );
    let resumed = emulator
        .ok(
            BALANCE,
            &json!({ "balanceType": "INVOICES_DUE", "maxResults": 1, "nextToken": minted }),
        )
        .await;
    assert_eq!(
        list(&resumed, "accountBalances").len(),
        1,
        "case \"own token\": the token resumes the balance it was minted for, got {resumed}"
    );
}
