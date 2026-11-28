// SPDX-License-Identifier: CC0-1.0

//! A held record rendered as the modelled wire shapes.
//!
//! Every response this service produces is built here, from the held record and
//! nothing else, so a record a listing summarises and the same record a get returns
//! cannot disagree: `FinancialRecordSummary` is [`financial_record_summary`] and
//! `FinancialRecord` is [`financial_record`], and both read the same members off the
//! same stored record — the summary is "a financial record as
//! `ListFinancialRecords` returns it, without the details that are specific to its
//! type", so it is literally the full record's projection minus `details`. The same
//! holds for an instrument and its summary.

use crate::ops::types::{
    AccountBalance, Balances, BillingPeriod, BusinessUnit, CreditMemoDetails, CurrencyAmount,
    FinancialRecord, FinancialRecordDetails, FinancialRecordSummary, InvoiceDetails,
    PaymentInstrument, PaymentInstrumentSummary,
};

use super::balances::DerivedAmount;
use super::records::{
    BillingPeriodRange, BusinessUnitRecord, FinancialRecordRecord, Money, PaymentInstrumentRecord,
    RecordBalances, RecordDetails,
};

/// A held amount as `CurrencyAmount`.
///
/// The value is rendered with the decimal places it is held with, which is what
/// `CurrencyAmount$value` describes ("the number of decimal places can differ
/// between amounts").
pub fn currency_amount(money: &Money) -> CurrencyAmount {
    CurrencyAmount {
        value: money.value.to_string(),
        currency: money.currency.clone(),
    }
}

/// The four held balances as `Balances`.
pub fn balances(held: &RecordBalances) -> Balances {
    Balances {
        outstanding: currency_amount(&held.outstanding),
        processing: currency_amount(&held.processing),
        settled: currency_amount(&held.settled),
        canceled: currency_amount(&held.canceled),
    }
}

/// A held business unit as `BusinessUnit`.
pub fn business_unit(held: &BusinessUnitRecord) -> BusinessUnit {
    BusinessUnit {
        merchant_of_record: held.merchant_of_record.clone(),
        line_of_business: held.line_of_business.map(|line| line.as_str().to_string()),
    }
}

/// A held billing period as `BillingPeriod`.
pub fn billing_period(held: &BillingPeriodRange) -> BillingPeriod {
    BillingPeriod {
        start_date: held.start_date,
        end_date: held.end_date,
    }
}

/// A held record's details as the `FinancialRecordDetails` union, with the member
/// that matches the record's type populated.
pub fn financial_record_details(held: &RecordDetails) -> FinancialRecordDetails {
    match held {
        RecordDetails::Invoice(invoice) => {
            FinancialRecordDetails::invoiceDetails(InvoiceDetails {
                invoice_id: invoice.invoice_id.clone(),
                due_status: invoice.due_status.map(|due| due.as_str().to_string()),
                billing_period: billing_period(&invoice.billing_period),
                due_date: invoice.due_date,
                payment_terms: invoice.payment_terms.clone(),
                invoice_frequency: invoice.invoice_frequency.as_str().to_string(),
            })
        }
        RecordDetails::CreditMemo(memo) => {
            FinancialRecordDetails::creditMemoDetails(CreditMemoDetails {
                invoice_id: memo.invoice_id.clone(),
                billing_period: billing_period(&memo.billing_period),
            })
        }
    }
}

/// A held record as the full `FinancialRecord`.
pub fn financial_record(held: &FinancialRecordRecord) -> FinancialRecord {
    FinancialRecord {
        id: held.id.clone(),
        r#type: held.record_type().as_str().to_string(),
        business_unit: business_unit(&held.business_unit),
        amount: currency_amount(&held.amount()),
        balances: balances(&held.balances),
        settlement_status: held.settlement_status().as_str().to_string(),
        status: held.status.as_str().to_string(),
        issued_at: held.issued_at,
        closed_at: held.closed_at,
        details: financial_record_details(&held.details),
    }
}

/// A held record as the `FinancialRecordSummary` a listing carries: the full record
/// without the details that are specific to its type.
pub fn financial_record_summary(held: &FinancialRecordRecord) -> FinancialRecordSummary {
    FinancialRecordSummary {
        id: held.id.clone(),
        r#type: held.record_type().as_str().to_string(),
        business_unit: business_unit(&held.business_unit),
        amount: currency_amount(&held.amount()),
        balances: balances(&held.balances),
        settlement_status: held.settlement_status().as_str().to_string(),
        status: held.status.as_str().to_string(),
        issued_at: held.issued_at,
        closed_at: held.closed_at,
    }
}

/// A held instrument as the full `PaymentInstrument`, including the details that
/// are specific to its payment method.
pub fn payment_instrument(held: &PaymentInstrumentRecord) -> PaymentInstrument {
    PaymentInstrument {
        arn: held.arn.clone(),
        payment_method: held.payment_method.as_str().to_string(),
        status: held.status.as_str().to_string(),
        details: held.details.clone(),
        created_at: Some(held.created_at),
        updated_at: Some(held.updated_at),
    }
}

/// A held instrument as the `PaymentInstrumentSummary` a listing carries: the full
/// instrument without its method-specific details, plus the `id` the summary names
/// it by.
pub fn payment_instrument_summary(held: &PaymentInstrumentRecord) -> PaymentInstrumentSummary {
    PaymentInstrumentSummary {
        id: held.id.clone(),
        arn: held.arn.clone(),
        payment_method: held.payment_method.as_str().to_string(),
        status: held.status.as_str().to_string(),
        created_at: Some(held.created_at),
        updated_at: Some(held.updated_at),
    }
}

/// One derived amount of the account balance as `AccountBalance`.
pub fn account_balance(derived: &DerivedAmount) -> AccountBalance {
    AccountBalance {
        amount: currency_amount(&derived.amount),
        merchant_of_record: derived.merchant_of_record.clone(),
    }
}
