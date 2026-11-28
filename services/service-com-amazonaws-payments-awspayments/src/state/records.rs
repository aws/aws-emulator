// SPDX-License-Identifier: CC0-1.0

//! What this account holds: the payment instruments registered on it and the
//! financial records Amazon Web Services issued to it, plus the two stores that
//! hold them and the lookups the operations resolve through.
//!
//! Every member of a stored record is persisted in the form the operations report
//! it in, with two deliberate choices:
//!
//! - a member the model types as an `enum` is held as the Rust enum
//!   ([`super::wire_enums`]), so a held record cannot carry a value the model does
//!   not declare;
//! - a monetary amount is held as an exact [`BigDecimal`] ([`Money`]) rather than as
//!   the decimal string the wire carries, because `GetAccountBalance` ADDS these
//!   amounts up and a sum of strings is not a sum.
//!
//! A record that depends on another member — a settlement status, a due status, an
//! amount — is not stored twice: [`RecordBalances::total`] is the record's amount
//! and [`super::wire_enums::FinancialRecordStatus::settlement_status`] is its
//! settlement status, so the invariants the model documents between them cannot
//! drift.

use std::collections::HashMap;

use ::aws_smithy_types::DateTime;
use bigdecimal::num_bigint::BigInt;
use bigdecimal::BigDecimal;

use crate::ops::types::PaymentInstrumentDetails;

use super::wire_enums::{
    DueStatus, FinancialRecordStatus, FinancialRecordType, InvoiceFrequency, LineOfBusiness,
    PaymentInstrumentStatus, PaymentMethod, SettlementStatus,
};

/// The number of decimal places the minor-unit constructors build an amount with.
///
/// Two, which is the standard unit's subdivision for every currency this account's
/// records are issued in. `CurrencyAmount$value` lets the places differ between
/// amounts ("for example `0.0` and `1000.24`"), so this is the scale of the amounts
/// built here and not a property of the member.
const MINOR_UNIT_SCALE: i64 = 2;

/// An amount of money, as `CurrencyAmount` carries it: a value and the ISO 4217
/// code of the currency it is in.
///
/// Held as a [`BigDecimal`] so adding two amounts is exact and keeps the decimal
/// places the amounts were issued with; [`super::projections::currency_amount`]
/// renders it as the modelled decimal string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Money {
    /// `CurrencyAmount$value`.
    pub value: BigDecimal,
    /// `CurrencyAmount$currency` — an ISO 4217 code (`^[A-Z]{3}$`).
    pub currency: String,
}

impl Money {
    /// An amount of `currency` whose value is `value`.
    pub fn new(value: BigDecimal, currency: &str) -> Self {
        Self {
            value,
            currency: currency.to_string(),
        }
    }

    /// An amount built from the currency's minor units — 1234 is `12.34`.
    ///
    /// Exact and infallible, which is why the fixture states amounts this way
    /// rather than as strings to parse.
    pub fn minor(minor_units: i64, currency: &str) -> Self {
        Self::new(
            BigDecimal::new(BigInt::from(minor_units), MINOR_UNIT_SCALE),
            currency,
        )
    }

    /// Zero, in `currency`, with the decimal places a minor-unit amount carries.
    pub fn zero(currency: &str) -> Self {
        Self::minor(0, currency)
    }
}

/// How much of a record's amount is outstanding, processing, settled and canceled,
/// as `Balances` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordBalances {
    /// `Balances$outstanding`.
    pub outstanding: Money,
    /// `Balances$processing`.
    pub processing: Money,
    /// `Balances$settled`.
    pub settled: Money,
    /// `Balances$canceled`.
    pub canceled: Money,
}

impl RecordBalances {
    /// The four balances of one record, in `currency`, from that currency's minor
    /// units.
    pub fn minor(
        currency: &str,
        outstanding: i64,
        processing: i64,
        settled: i64,
        canceled: i64,
    ) -> Self {
        Self {
            outstanding: Money::minor(outstanding, currency),
            processing: Money::minor(processing, currency),
            settled: Money::minor(settled, currency),
            canceled: Money::minor(canceled, currency),
        }
    }

    /// The record's total amount: the four balances added up.
    ///
    /// `Balances` documents that "the four balances add up to `amount`" and that
    /// "each balance is in the currency of `amount`", so the amount is DERIVED from
    /// the balances here instead of being a fifth number that could disagree with
    /// them.
    pub fn total(&self) -> Money {
        let value = [
            &self.outstanding,
            &self.processing,
            &self.settled,
            &self.canceled,
        ]
        .into_iter()
        .fold(Money::zero(&self.outstanding.currency).value, |sum, balance| {
            sum + &balance.value
        });
        Money::new(value, &self.outstanding.currency)
    }
}

/// The billing period a record covers, as `BillingPeriod` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BillingPeriodRange {
    /// `BillingPeriod$startDate`.
    pub start_date: DateTime,
    /// `BillingPeriod$endDate`.
    pub end_date: DateTime,
}

/// The business unit that issued a record, as `BusinessUnit` carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusinessUnitRecord {
    /// `BusinessUnit$merchantOfRecord` — the invoicing entity, which is one of the
    /// two keys `GetAccountBalance` groups by.
    pub merchant_of_record: String,
    /// `BusinessUnit$lineOfBusiness` — absent when it is not known for the record.
    pub line_of_business: Option<LineOfBusiness>,
}

/// The details of an invoice, as `InvoiceDetails` carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvoiceInfo {
    /// `InvoiceDetails$invoiceId` — what `invoiceIdentifier` looks a record up by.
    pub invoice_id: String,
    /// `InvoiceDetails$dueStatus` — present only while the record's status is
    /// `OUTSTANDING`.
    pub due_status: Option<DueStatus>,
    /// `InvoiceDetails$billingPeriod`.
    pub billing_period: BillingPeriodRange,
    /// `InvoiceDetails$dueDate`.
    pub due_date: DateTime,
    /// `InvoiceDetails$paymentTerms` — absent when Invoicing issued none.
    pub payment_terms: Option<String>,
    /// `InvoiceDetails$invoiceFrequency`.
    pub invoice_frequency: InvoiceFrequency,
}

/// The details of a credit memo, as `CreditMemoDetails` carries them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreditMemoInfo {
    /// `CreditMemoDetails$invoiceId` — what `invoiceIdentifier` looks a record up
    /// by.
    pub invoice_id: String,
    /// `CreditMemoDetails$billingPeriod`.
    pub billing_period: BillingPeriodRange,
}

/// The attributes that exist only for a record's type, as the
/// `FinancialRecordDetails` union carries them.
///
/// The populated member is the record's type: there is no way to hold an invoice's
/// details on a credit memo, which is what lets [`FinancialRecordRecord::record_type`]
/// be read off the details instead of stored beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecordDetails {
    /// `FinancialRecordDetails$invoiceDetails`.
    Invoice(InvoiceInfo),
    /// `FinancialRecordDetails$creditMemoDetails`.
    CreditMemo(CreditMemoInfo),
}

impl RecordDetails {
    /// The record type whose details these are.
    pub const fn record_type(&self) -> FinancialRecordType {
        match self {
            Self::Invoice(_) => FinancialRecordType::Invoice,
            Self::CreditMemo(_) => FinancialRecordType::CreditMemo,
        }
    }

    /// The invoice ID Amazon Web Services Invoicing issued for the record — both
    /// members of the union carry one, and `invoiceIdentifier` accepts either.
    pub fn invoice_id(&self) -> &str {
        match self {
            Self::Invoice(invoice) => &invoice.invoice_id,
            Self::CreditMemo(memo) => &memo.invoice_id,
        }
    }

    /// Whether the invoice is due or past its due date, for an invoice that carries
    /// a due status; `None` for a credit memo, which has none.
    pub const fn due_status(&self) -> Option<DueStatus> {
        match self {
            Self::Invoice(invoice) => invoice.due_status,
            Self::CreditMemo(_) => None,
        }
    }
}

/// One invoice or credit memo held for this account: every member of the wire
/// `FinancialRecord` shape, persisted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinancialRecordRecord {
    /// `FinancialRecord$id` — the ID Amazon Web Services Payments issued
    /// (`^[A-Z]{2,3}-[0-9]{4}-[0-9]{4}-[0-9]{4}(-[0-9]{4})?$`).
    pub id: String,
    /// `FinancialRecord$businessUnit`.
    pub business_unit: BusinessUnitRecord,
    /// `FinancialRecord$balances`, from which `FinancialRecord$amount` follows.
    pub balances: RecordBalances,
    /// `FinancialRecord$status`, from which `FinancialRecord$settlementStatus`
    /// follows.
    pub status: FinancialRecordStatus,
    /// `FinancialRecord$issuedAt`.
    pub issued_at: DateTime,
    /// `FinancialRecord$closedAt` — present only while the settlement status is
    /// `CLOSED`.
    pub closed_at: Option<DateTime>,
    /// `FinancialRecord$details`, from which `FinancialRecord$type` follows.
    pub details: RecordDetails,
}

impl FinancialRecordRecord {
    /// `FinancialRecord$type` — the type of the record, read off the populated
    /// member of its details so the two cannot disagree.
    pub const fn record_type(&self) -> FinancialRecordType {
        self.details.record_type()
    }

    /// `FinancialRecord$amount` — the four balances added up.
    pub fn amount(&self) -> Money {
        self.balances.total()
    }

    /// `FinancialRecord$settlementStatus` — the one that follows from the status.
    pub const fn settlement_status(&self) -> SettlementStatus {
        self.status.settlement_status()
    }

    /// The instant this record was issued, as a totally ordered key — what
    /// `ListFinancialRecords` sorts and filters on.
    pub fn issued_key(&self) -> (i64, u32) {
        instant_key(&self.issued_at)
    }
}

/// An instant as a totally ordered key, so two instants can be compared and sorted
/// without losing the sub-second part.
pub fn instant_key(at: &DateTime) -> (i64, u32) {
    (at.secs(), at.subsec_nanos())
}

/// One payment instrument registered on this account: every member of the wire
/// `PaymentInstrument` shape, persisted, plus the `id`
/// `ListPaymentInstruments` reports.
#[derive(Debug, Clone)]
pub struct PaymentInstrumentRecord {
    /// `PaymentInstrumentSummary$id` — unique within the account, and the last
    /// segment of [`Self::arn`] (`^[a-z0-9-]+$`).
    pub id: String,
    /// `PaymentInstrument$arn`.
    pub arn: String,
    /// `PaymentInstrument$paymentMethod`.
    pub payment_method: PaymentMethod,
    /// `PaymentInstrument$status`.
    pub status: PaymentInstrumentStatus,
    /// `PaymentInstrument$details` — absent for a payment method that has no
    /// additional details. `ListPaymentInstruments` does not carry it.
    pub details: Option<PaymentInstrumentDetails>,
    /// `PaymentInstrument$createdAt`.
    pub created_at: DateTime,
    /// `PaymentInstrument$updatedAt`.
    pub updated_at: DateTime,
}

/// The payment instruments this account holds, in insertion order.
#[derive(Debug, Default)]
pub struct PaymentInstrumentStore {
    instruments: Vec<PaymentInstrumentRecord>,
}

impl PaymentInstrumentStore {
    /// A store holding `instruments`, in the order given.
    pub fn new(instruments: Vec<PaymentInstrumentRecord>) -> Self {
        let mut store = Self::default();
        store.seed(instruments);
        store
    }

    /// The held instruments, in insertion order — the order `ListPaymentInstruments`
    /// pages through.
    pub fn instruments(&self) -> &[PaymentInstrumentRecord] {
        &self.instruments
    }

    /// The instrument `identifier` names, by its ID or by its ARN.
    ///
    /// `GetPaymentInstrument$identifier` accepts either form ("the Amazon Resource
    /// Name (ARN) of the instrument, as `ListPaymentInstruments` returns in `arn`"
    /// or "the ID of the instrument, as `ListPaymentInstruments` returns in `id`"),
    /// and both resolve to the one held record — the ARN because it is the held
    /// ARN, not because its last segment looks like an ID, so an ARN for another
    /// partition or another account names no instrument on this account.
    pub fn lookup(&self, identifier: &str) -> Option<&PaymentInstrumentRecord> {
        self.instruments
            .iter()
            .find(|held| held.id == identifier || held.arn == identifier)
    }

    /// Install instruments, keeping insertion order.
    ///
    /// THE SEEDING SEAM. The service models no create operation — an instrument is
    /// registered through the Amazon Web Services Billing and Cost Management
    /// console — so this is how the account's instruments are populated. An
    /// instrument whose `id` is already held is REPLACED in place, so re-seeding a
    /// changed instrument neither duplicates it nor moves it to the end of the page
    /// order.
    pub fn seed<I: IntoIterator<Item = PaymentInstrumentRecord>>(&mut self, instruments: I) {
        for instrument in instruments {
            match self
                .instruments
                .iter_mut()
                .find(|held| held.id == instrument.id)
            {
                Some(held) => *held = instrument,
                None => self.instruments.push(instrument),
            }
        }
    }

    /// Drop every held instrument — the seam a caller uses to model an account with
    /// no payment instrument registered on it.
    pub fn clear(&mut self) {
        self.instruments.clear();
    }

    /// How many instruments are held.
    pub fn len(&self) -> usize {
        self.instruments.len()
    }

    /// Whether no instrument is held.
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }
}

/// A way to identify a financial record, as the `FinancialRecordKey` union and the
/// `GetFinancialRecord` request both spell it.
///
/// ONE enum for both, so `GetFinancialRecord` and `BatchGetFinancialRecord` resolve
/// a key through the same [`FinancialRecordStore::lookup`] and cannot disagree
/// about which record a key names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKey<'a> {
    /// The ID Amazon Web Services Payments issued for the record.
    Id(&'a str),
    /// The invoice ID Amazon Web Services Invoicing issued for the record.
    InvoiceId(&'a str),
}

/// The invoices and credit memos this account holds, in insertion order.
///
/// Both identifiers a key can name are indexed, so a lookup by either one is a
/// single map hit into the SAME `Vec` position: the record
/// `GetFinancialRecord(identifier=…)` returns and the record
/// `GetFinancialRecord(invoiceIdentifier=…)` returns are one record, not two copies.
#[derive(Debug, Default)]
pub struct FinancialRecordStore {
    records: Vec<FinancialRecordRecord>,
    /// `FinancialRecord$id` -> position in [`Self::records`].
    id_index: HashMap<String, usize>,
    /// `details.invoiceDetails.invoiceId` / `details.creditMemoDetails.invoiceId`
    /// -> position in [`Self::records`].
    invoice_id_index: HashMap<String, usize>,
}

impl FinancialRecordStore {
    /// A store holding `records`, in the order given.
    pub fn new(records: Vec<FinancialRecordRecord>) -> Self {
        let mut store = Self::default();
        store.seed(records);
        store
    }

    /// The held records, in insertion order.
    pub fn records(&self) -> &[FinancialRecordRecord] {
        &self.records
    }

    /// The record `key` names, or `None` when no record on this account matches it.
    pub fn lookup(&self, key: RecordKey<'_>) -> Option<&FinancialRecordRecord> {
        let position = match key {
            RecordKey::Id(id) => self.id_index.get(id),
            RecordKey::InvoiceId(invoice_id) => self.invoice_id_index.get(invoice_id),
        }?;
        self.records.get(*position)
    }

    /// Install records, keeping insertion order.
    ///
    /// THE SEEDING SEAM, for the same reason as
    /// [`PaymentInstrumentStore::seed`]: Amazon Web Services issues these records,
    /// so the service models no operation that creates one. A record whose `id` is
    /// already held is REPLACED in place.
    pub fn seed<I: IntoIterator<Item = FinancialRecordRecord>>(&mut self, records: I) {
        for record in records {
            match self.records.iter_mut().find(|held| held.id == record.id) {
                Some(held) => *held = record,
                None => self.records.push(record),
            }
        }
        self.reindex();
    }

    /// Drop every held record — the seam a caller uses to model an account with no
    /// invoice or credit memo on it.
    pub fn clear(&mut self) {
        self.records.clear();
        self.reindex();
    }

    /// How many records are held.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Whether no record is held.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// Rebuild both lookup indexes from the held records.
    ///
    /// Rebuilt whole on every change rather than patched, so an index can never
    /// name a position the records no longer hold. Where two records would claim
    /// one key, the FIRST one held keeps it: the seeding seam appends, so a key a
    /// later record repeats cannot take a lookup away from the record that already
    /// answered it.
    fn reindex(&mut self) {
        self.id_index = Self::index_by(&self.records, |record| record.id.clone());
        self.invoice_id_index =
            Self::index_by(&self.records, |record| record.details.invoice_id().to_string());
    }

    /// The position of each held record, keyed by `key_of`, keeping the first
    /// record that claims a key.
    fn index_by(
        records: &[FinancialRecordRecord],
        key_of: impl Fn(&FinancialRecordRecord) -> String,
    ) -> HashMap<String, usize> {
        let mut index = HashMap::new();
        for (position, record) in records.iter().enumerate() {
            index.entry(key_of(record)).or_insert(position);
        }
        index
    }
}
