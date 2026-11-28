// SPDX-License-Identifier: CC0-1.0

//! The account balance `GetAccountBalance` reports, DERIVED from the held records.
//!
//! The balance is not stored: the operation "retrieves one balance of your account,
//! selected by balance type, as one amount for each merchant of record and
//! currency", where "each amount is the sum of the outstanding balances of your
//! invoices or credit memos that have the status `OUTSTANDING`", and "amounts are
//! never added together across merchants of record or currencies". So the answer is
//! computed from the records the store holds at the moment of the call, and changing
//! those records changes it.

use std::collections::BTreeMap;

use bigdecimal::BigDecimal;

use super::records::{FinancialRecordRecord, Money, RecordDetails};
use super::wire_enums::{BalanceType, DueStatus, FinancialRecordStatus};

/// One amount of the balance: the total for one merchant of record in one currency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DerivedAmount {
    /// The merchant of record that issued the records making up this amount.
    pub merchant_of_record: String,
    /// The amount, in the currency those records were issued in.
    pub amount: Money,
}

/// The amounts of `balance_type` over `records`, one per (merchant of record,
/// currency) that has one.
///
/// Grouped in a [`BTreeMap`], so the amounts come out ordered by merchant of record
/// and then by currency: `GetAccountBalance` is `@paginated`, and a stable order is
/// what lets its `nextToken` resume where the previous page stopped.
///
/// A pair with no contributing record produces NO amount rather than a zero: the
/// balance is the sum of the records that make it up, and there is nothing to report
/// for a merchant and currency this account holds no such record in.
pub fn account_balance_amounts(
    records: &[FinancialRecordRecord],
    balance_type: BalanceType,
) -> Vec<DerivedAmount> {
    let mut totals: BTreeMap<(&str, &str), BigDecimal> = BTreeMap::new();
    for record in records
        .iter()
        .filter(|record| contributes(record, balance_type))
    {
        let outstanding = &record.balances.outstanding;
        let total = totals
            .entry((
                record.business_unit.merchant_of_record.as_str(),
                outstanding.currency.as_str(),
            ))
            .or_insert_with(|| Money::zero(&outstanding.currency).value);
        *total += &outstanding.value;
    }
    totals
        .into_iter()
        .map(|((merchant_of_record, currency), value)| DerivedAmount {
            merchant_of_record: merchant_of_record.to_string(),
            amount: Money::new(value, currency),
        })
        .collect()
}

/// Whether `record`'s outstanding balance is part of `balance_type`.
///
/// Two conditions, both from `GetAccountBalanceRequest$balanceType`: the record's
/// status is `OUTSTANDING` — "a record that is `PROCESSING`, `SETTLED`, or
/// `CANCELED` isn't included, even when part of its amount is outstanding" — and the
/// record is of the kind the balance type selects, which for the two invoice
/// balances is the invoice's `dueStatus`.
fn contributes(record: &FinancialRecordRecord, balance_type: BalanceType) -> bool {
    if record.status != FinancialRecordStatus::Outstanding {
        return false;
    }
    match balance_type {
        BalanceType::InvoicesDue => record.details.due_status() == Some(DueStatus::Due),
        BalanceType::InvoicesPastDue => record.details.due_status() == Some(DueStatus::PastDue),
        BalanceType::OutstandingCreditMemos => {
            matches!(record.details, RecordDetails::CreditMemo(_))
        }
    }
}
