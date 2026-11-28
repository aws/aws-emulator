// SPDX-License-Identifier: CC0-1.0

//! The inventory this account starts with.
//!
//! Every operation this service binds only REPORTS, so an account whose store is
//! filled only through the seeding seams reports nothing at all to a caller that
//! never seeded it: no instrument to get, no record to look up, and an account
//! balance that is empty whichever balance type is asked for. The store therefore
//! starts with the inventory this account has, the way the real service's billing
//! pipeline produces one; the stores' `seed` and `clear` stay the seam that installs
//! another.
//!
//! The fixture is a FIXED set of records — this emulator observes no billing usage
//! to derive one from — dated from ONE anchor instant, the emulator's own start.
//! Dating it from the anchor rather than from absolute calendar dates is what keeps
//! the records coherent as the emulator runs: an invoice whose due date has not
//! passed is `DUE` and one whose due date has passed is `PAST_DUE`, and that stays
//! true instead of becoming false as a fixed date slides into the past.
//!
//! Each row below states only the facts that are INDEPENDENT. Everything that
//! follows from them is derived as the record is built, so the invariants the model
//! documents hold by construction rather than by being typed out consistently:
//!
//! - `amount` is the four balances added up ([`RecordBalances::total`]);
//! - `settlementStatus` is the one the status implies, and `closedAt` is present
//!   exactly when that status is `CLOSED`;
//! - `dueStatus` is present only while the status is `OUTSTANDING`, and is `DUE` or
//!   `PAST_DUE` according to whether the due date has passed;
//! - `type` is the populated member of `details`.
//!
//! The set spans what the operations have to tell apart: two merchants of record,
//! three currencies, one merchant invoicing in two of them, every
//! `FinancialRecordStatus`, invoices both `DUE` and `PAST_DUE`, a partly settled
//! invoice whose outstanding balance is less than its amount, credit memos both
//! outstanding and settled, and payment instruments of four payment methods — one
//! of which has no details at all.

use ::aws_smithy_types::DateTime;

use crate::ops::types::{Card, PaymentInstrumentDetails, SepaBankAccount, UsAchBankAccount};

use super::records::{
    BillingPeriodRange, BusinessUnitRecord, CreditMemoInfo, FinancialRecordRecord, InvoiceInfo,
    PaymentInstrumentRecord, RecordBalances, RecordDetails,
};
use super::wire_enums::{
    BankAccountType, CardBrand, CardType, DueStatus, FinancialRecordStatus, InvoiceFrequency,
    LineOfBusiness, PaymentInstrumentStatus, PaymentMethod, SettlementStatus,
};

/// Seconds in a day — the unit the fixture's offsets are stated in.
const SECONDS_PER_DAY: i64 = 86_400;

/// How long after it was issued a closed record closed.
///
/// `FinancialRecord$closedAt` is "when its `status` last became `SETTLED` or
/// `CANCELED`", so it is after the record was issued and before now; the span
/// itself is this emulator's choice.
const DAYS_FROM_ISSUE_TO_CLOSE: i64 = 3;

/// How long a billing period the fixture's records bill for.
///
/// `BillingPeriod` is "the period of the usage it bills", and these are monthly
/// records, so each covers the thirty days ending when it was issued.
const BILLING_PERIOD_DAYS: i64 = 30;

/// The merchant of record that invoices this account's United States usage.
///
/// `BusinessUnit$merchantOfRecord` names this entity as one of its two examples of
/// "the Amazon Web Services entity that issued the invoice and collects payment for
/// it".
const MERCHANT_US: &str = "Amazon Web Services, Inc.";

/// The merchant of record that invoices this account's European usage — the other
/// entity `BusinessUnit$merchantOfRecord` names.
const MERCHANT_EMEA: &str = "Amazon Web Services EMEA SARL";

/// The ISO 4217 code `CurrencyAmount$currency` gives as its own example.
const USD: &str = "USD";
/// The euro, the currency the EMEA merchant of record invoices in.
const EUR: &str = "EUR";
/// Pound sterling — the second currency the EMEA merchant of record invoices in, so
/// that one merchant's amounts span two currencies.
const GBP: &str = "GBP";

/// The inventory of one account: what the two stores start holding.
#[derive(Debug)]
pub struct AccountFixture {
    /// The payment instruments registered on the account.
    pub payment_instruments: Vec<PaymentInstrumentRecord>,
    /// The invoices and credit memos issued to the account.
    pub financial_records: Vec<FinancialRecordRecord>,
}

/// The instant the fixture is dated from: the emulator's own start, because
/// [`super::install_service_state`] builds the store on the boot path.
///
/// It is NOT a reader's clock: a record nobody changed keeps the instants it was
/// built with however long the emulator runs before it is first read.
pub fn anchor_now() -> DateTime {
    DateTime::from(::std::time::SystemTime::now())
}

/// The inventory this account holds, dated from `anchor`.
pub fn account_fixture(anchor: DateTime) -> AccountFixture {
    AccountFixture {
        payment_instruments: payment_instruments(anchor),
        financial_records: financial_records(anchor),
    }
}

/// One row of the invoice half of the fixture: the facts about an invoice that
/// follow from nothing else.
struct InvoiceRow {
    /// `FinancialRecord$id`.
    id: &'static str,
    /// `InvoiceDetails$invoiceId`.
    invoice_id: &'static str,
    /// `BusinessUnit$merchantOfRecord`.
    merchant_of_record: &'static str,
    /// `BusinessUnit$lineOfBusiness`.
    line_of_business: Option<LineOfBusiness>,
    /// `CurrencyAmount$currency` for the amount and all four balances.
    currency: &'static str,
    /// The four balances in minor units, in the order `Balances` declares them:
    /// outstanding, processing, settled, canceled.
    balances: [i64; 4],
    /// `FinancialRecord$status`.
    status: FinancialRecordStatus,
    /// How long before the anchor the invoice was issued.
    issued_days_ago: i64,
    /// How long after the anchor the invoice falls due; negative for a due date
    /// that has already passed.
    due_in_days: i64,
    /// `InvoiceDetails$paymentTerms`.
    payment_terms: Option<&'static str>,
    /// `InvoiceDetails$invoiceFrequency`.
    invoice_frequency: InvoiceFrequency,
}

/// One row of the credit-memo half of the fixture.
struct CreditMemoRow {
    /// `FinancialRecord$id`.
    id: &'static str,
    /// `CreditMemoDetails$invoiceId`.
    invoice_id: &'static str,
    /// `BusinessUnit$merchantOfRecord`.
    merchant_of_record: &'static str,
    /// `BusinessUnit$lineOfBusiness`.
    line_of_business: Option<LineOfBusiness>,
    /// `CurrencyAmount$currency` for the amount and all four balances.
    currency: &'static str,
    /// The four balances in minor units, as [`InvoiceRow::balances`].
    balances: [i64; 4],
    /// `FinancialRecord$status`.
    status: FinancialRecordStatus,
    /// How long before the anchor the credit memo was issued.
    issued_days_ago: i64,
}

/// The invoices this account holds.
const INVOICE_ROWS: &[InvoiceRow] = &[
    // A whole monthly invoice still to pay, not yet due.
    InvoiceRow {
        id: "INV-1001-2001-3001-4001",
        invoice_id: "AWSINV-2026-1001",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: USD,
        balances: [150_000, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 30,
        due_in_days: 15,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
    // Partly settled and still outstanding: its outstanding balance is less than
    // its amount, so a balance that reported the amount would be wrong for it.
    InvoiceRow {
        id: "INV-1001-2001-3001-4002",
        invoice_id: "AWSINV-2026-1002",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: USD,
        balances: [10_000, 0, 5_000, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 45,
        due_in_days: 12,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
    // Past its due date and still outstanding, from another line of business.
    InvoiceRow {
        id: "INV-1001-2001-3001-4003",
        invoice_id: "AWSINV-2026-1003",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::ProserveMarketingAndTraining),
        currency: USD,
        balances: [24_050, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 75,
        due_in_days: -45,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::OneTime,
    },
    // The other merchant of record, invoicing in euros.
    InvoiceRow {
        id: "INV-1002-2002-3002-4001",
        invoice_id: "AWSINV-2026-2001",
        merchant_of_record: MERCHANT_EMEA,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: EUR,
        balances: [98_075, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 20,
        due_in_days: 10,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
    // The same merchant of record invoicing in a SECOND currency, so that the two
    // amounts it owes are never added together.
    InvoiceRow {
        id: "INV-1002-2002-3002-4002",
        invoice_id: "AWSINV-2026-2002",
        merchant_of_record: MERCHANT_EMEA,
        line_of_business: Some(LineOfBusiness::AwsMarketplace),
        currency: GBP,
        balances: [31_000, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 90,
        due_in_days: -60,
        payment_terms: None,
        invoice_frequency: InvoiceFrequency::OneTime,
    },
    // A payment is submitted and not yet confirmed: open, but not outstanding, so
    // no balance counts it and it carries no due status.
    InvoiceRow {
        id: "INV-1003-2003-3003-4001",
        invoice_id: "AWSINV-2026-1004",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: USD,
        balances: [0, 64_000, 0, 0],
        status: FinancialRecordStatus::Processing,
        issued_days_ago: 10,
        due_in_days: 20,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
    // Paid in full: closed, so it carries a `closedAt` and no due status.
    InvoiceRow {
        id: "INV-1003-2003-3003-4002",
        invoice_id: "AWSINV-2026-1005",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: USD,
        balances: [0, 0, 120_000, 0],
        status: FinancialRecordStatus::Settled,
        issued_days_ago: 40,
        due_in_days: -10,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
    // Voided: closed with the whole amount canceled.
    InvoiceRow {
        id: "INV-1004-2004-3004-4001",
        invoice_id: "AWSINV-2026-2003",
        merchant_of_record: MERCHANT_EMEA,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: EUR,
        balances: [0, 0, 0, 5_525],
        status: FinancialRecordStatus::Canceled,
        issued_days_ago: 15,
        due_in_days: 15,
        payment_terms: Some("Net30"),
        invoice_frequency: InvoiceFrequency::Recurring,
    },
];

/// The credit memos this account holds.
const CREDIT_MEMO_ROWS: &[CreditMemoRow] = &[
    // Credit not yet applied to an invoice, for the US merchant of record.
    CreditMemoRow {
        id: "CM-1001-2001-3001",
        invoice_id: "AWSCM-2026-1001",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: USD,
        balances: [30_000, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 25,
    },
    // The same, for the EMEA merchant of record in euros.
    CreditMemoRow {
        id: "CM-1002-2002-3002",
        invoice_id: "AWSCM-2026-2001",
        merchant_of_record: MERCHANT_EMEA,
        line_of_business: Some(LineOfBusiness::AwsCloudServices),
        currency: EUR,
        balances: [12_040, 0, 0, 0],
        status: FinancialRecordStatus::Outstanding,
        issued_days_ago: 18,
    },
    // Already applied in full, so no outstanding credit remains on it.
    CreditMemoRow {
        id: "CM-1003-2003-3003",
        invoice_id: "AWSCM-2026-1002",
        merchant_of_record: MERCHANT_US,
        line_of_business: Some(LineOfBusiness::AwsMarketplace),
        currency: USD,
        balances: [0, 0, 7_500, 0],
        status: FinancialRecordStatus::Settled,
        issued_days_ago: 60,
    },
];

/// The account's records, invoices first, in the order the rows declare them —
/// which is the insertion order the store keeps and the listings page through.
fn financial_records(anchor: DateTime) -> Vec<FinancialRecordRecord> {
    INVOICE_ROWS
        .iter()
        .map(|row| invoice_record(row, anchor))
        .chain(
            CREDIT_MEMO_ROWS
                .iter()
                .map(|row| credit_memo_record(row, anchor)),
        )
        .collect()
}

/// One invoice row as a held record, with every dependent member derived.
fn invoice_record(row: &InvoiceRow, anchor: DateTime) -> FinancialRecordRecord {
    let issued_at = days_before(anchor, row.issued_days_ago);
    let due_date = midnight(days_after(anchor, row.due_in_days));
    FinancialRecordRecord {
        id: row.id.to_string(),
        business_unit: BusinessUnitRecord {
            merchant_of_record: row.merchant_of_record.to_string(),
            line_of_business: row.line_of_business,
        },
        balances: RecordBalances::minor(
            row.currency,
            row.balances[0],
            row.balances[1],
            row.balances[2],
            row.balances[3],
        ),
        status: row.status,
        issued_at,
        closed_at: closed_at(row.status, issued_at),
        details: RecordDetails::Invoice(InvoiceInfo {
            invoice_id: row.invoice_id.to_string(),
            due_status: due_status(row.status, due_date, anchor),
            billing_period: billing_period(issued_at),
            due_date,
            payment_terms: row.payment_terms.map(str::to_string),
            invoice_frequency: row.invoice_frequency,
        }),
    }
}

/// One credit-memo row as a held record, with every dependent member derived.
fn credit_memo_record(row: &CreditMemoRow, anchor: DateTime) -> FinancialRecordRecord {
    let issued_at = days_before(anchor, row.issued_days_ago);
    FinancialRecordRecord {
        id: row.id.to_string(),
        business_unit: BusinessUnitRecord {
            merchant_of_record: row.merchant_of_record.to_string(),
            line_of_business: row.line_of_business,
        },
        balances: RecordBalances::minor(
            row.currency,
            row.balances[0],
            row.balances[1],
            row.balances[2],
            row.balances[3],
        ),
        status: row.status,
        issued_at,
        closed_at: closed_at(row.status, issued_at),
        details: RecordDetails::CreditMemo(CreditMemoInfo {
            invoice_id: row.invoice_id.to_string(),
            billing_period: billing_period(issued_at),
        }),
    }
}

/// When a record closed: `DAYS_FROM_ISSUE_TO_CLOSE` after it was issued for a
/// record whose status is closed, and absent for one that is still open.
///
/// `FinancialRecord$closedAt` "is absent while the `settlementStatus` is `OPEN`",
/// so presence follows from the status and is not a separate fact.
fn closed_at(status: FinancialRecordStatus, issued_at: DateTime) -> Option<DateTime> {
    (status.settlement_status() == SettlementStatus::Closed)
        .then(|| days_after(issued_at, DAYS_FROM_ISSUE_TO_CLOSE))
}

/// Whether an invoice is due or past due, for an invoice that carries a due status.
///
/// `InvoiceDetails$dueStatus` "is present only while the invoice's `status` is
/// `OUTSTANDING`", and `DUE` is "the due date hasn't passed" against `PAST_DUE`
/// "the due date has passed", so both the presence and the value follow from
/// members already stated.
fn due_status(
    status: FinancialRecordStatus,
    due_date: DateTime,
    anchor: DateTime,
) -> Option<DueStatus> {
    (status == FinancialRecordStatus::Outstanding).then(|| {
        if super::records::instant_key(&due_date) >= super::records::instant_key(&anchor) {
            DueStatus::Due
        } else {
            DueStatus::PastDue
        }
    })
}

/// The billing period a record issued at `issued_at` bills: the
/// `BILLING_PERIOD_DAYS` ending that day.
fn billing_period(issued_at: DateTime) -> BillingPeriodRange {
    let end_date = midnight(issued_at);
    BillingPeriodRange {
        start_date: days_before(end_date, BILLING_PERIOD_DAYS),
        end_date,
    }
}

/// The payment instruments registered on this account, in insertion order.
fn payment_instruments(anchor: DateTime) -> Vec<PaymentInstrumentRecord> {
    vec![
        // A card Amazon Web Services can charge.
        instrument(
            "pi-0a1b2c3d4e5f",
            PaymentMethod::Card,
            PaymentInstrumentStatus::Active,
            (400, 400),
            anchor,
            Some(PaymentInstrumentDetails::Card(Card {
                brand: Some(CardBrand::Visa.as_str().to_string()),
                last4: "4242".to_string(),
                expiration_month: Some(11),
                expiration_year: Some(2031),
                card_type: Some(CardType::Credit.as_str().to_string()),
                issuing_bank: Some("Example Bank".to_string()),
                issuer_country: Some("US".to_string()),
            })),
        ),
        // A US bank account debited over ACH.
        instrument(
            "pi-1b2c3d4e5f60",
            PaymentMethod::UsAchBankAccount,
            PaymentInstrumentStatus::Active,
            (320, 320),
            anchor,
            Some(PaymentInstrumentDetails::usAchBankAccount(
                UsAchBankAccount {
                    account_number_tail: Some("6789".to_string()),
                    account_holder_name: Some("Example Corporation".to_string()),
                    bank_name: Some("Example Bank, N.A.".to_string()),
                    account_type: Some(BankAccountType::Checking.as_str().to_string()),
                },
            )),
        ),
        // A SEPA mandate still waiting for confirmation: registered recently and
        // changed more recently still, so `createdAt` and `updatedAt` differ.
        instrument(
            "pi-2c3d4e5f6071",
            PaymentMethod::SepaBankAccount,
            PaymentInstrumentStatus::SetupInProgress,
            (5, 2),
            anchor,
            Some(PaymentInstrumentDetails::sepaBankAccount(SepaBankAccount {
                account_number_tail: Some("4321".to_string()),
                account_holder_name: Some("Example Corporation SARL".to_string()),
                bank_name: Some("Example Bank Europe".to_string()),
                account_type: Some(BankAccountType::Checking.as_str().to_string()),
            })),
        ),
        // Pay by invoice: a payment method with no details of its own, so the
        // instrument carries none.
        instrument(
            "pi-3d4e5f607182",
            PaymentMethod::PayByInvoice,
            PaymentInstrumentStatus::Active,
            (400, 400),
            anchor,
            None,
        ),
        // A card Amazon Web Services can no longer charge.
        instrument(
            "pi-4e5f60718293",
            PaymentMethod::Card,
            PaymentInstrumentStatus::Invalid,
            (500, 30),
            anchor,
            Some(PaymentInstrumentDetails::Card(Card {
                brand: Some(CardBrand::Mastercard.as_str().to_string()),
                last4: "5454".to_string(),
                expiration_month: Some(1),
                expiration_year: Some(2030),
                card_type: Some(CardType::Debit.as_str().to_string()),
                issuing_bank: Some("Example Bank Europe".to_string()),
                issuer_country: Some("DE".to_string()),
            })),
        ),
    ]
}

/// One held instrument. Its ARN is built through [`engine::emulator_arn`], so the
/// account segment is the one this emulator presents and the last segment is the
/// `id` — which is what makes `GetPaymentInstrument` resolve the two forms of its
/// `identifier` to one record.
fn instrument(
    id: &str,
    payment_method: PaymentMethod,
    status: PaymentInstrumentStatus,
    registered_and_changed_days_ago: (i64, i64),
    anchor: DateTime,
    details: Option<PaymentInstrumentDetails>,
) -> PaymentInstrumentRecord {
    let (created_days_ago, updated_days_ago) = registered_and_changed_days_ago;
    PaymentInstrumentRecord {
        id: id.to_string(),
        arn: ::engine::emulator_arn("payments", "", &format!("payment-instrument:{id}")),
        payment_method,
        status,
        details,
        created_at: days_before(anchor, created_days_ago),
        updated_at: days_before(anchor, updated_days_ago),
    }
}

/// `days` before `at`.
fn days_before(at: DateTime, days: i64) -> DateTime {
    days_after(at, -days)
}

/// `days` after `at`; a negative `days` is before it.
fn days_after(at: DateTime, days: i64) -> DateTime {
    DateTime::from_secs(at.secs().saturating_add(days.saturating_mul(SECONDS_PER_DAY)))
}

/// `at` truncated to 00:00:00 UTC of the day it falls on.
///
/// `InvoiceDetails$dueDate` is "the calendar date by which the invoice must be
/// paid, as the timestamp of 00:00:00 UTC on that day", and a billing period is a
/// span of whole days, so both are stored on a day boundary.
fn midnight(at: DateTime) -> DateTime {
    DateTime::from_secs(at.secs() - at.secs().rem_euclid(SECONDS_PER_DAY))
}
