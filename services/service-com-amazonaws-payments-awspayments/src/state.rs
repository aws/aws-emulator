// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-payments-awspayments service.
//!
//! The service holds two kinds of record — the *payment instruments* registered on
//! the account and the *financial records* (invoices and credit memos) Amazon Web
//! Services issued to it — and every one of its six operations only REPORTS them:
//! the model binds no create, update or delete anywhere. So the store STARTS with
//! the inventory this account has ([`seed::account_fixture`], the stand-in for the
//! real service's own billing pipeline) and is re-filled through the seeding seams
//! on the two stores ([`records::PaymentInstrumentStore::seed`],
//! [`records::FinancialRecordStore::seed`]).
//!
//! That starting value is installed when the emulator BUILDS this service
//! ([`install_service_state`], called from `EmulatedService::new`), not when a
//! request first reaches the store: the engine builds a service's state on first
//! access, so leaving it to the first access would make the instants the fixture is
//! anchored at — issue dates, due dates, registration dates — a function of when
//! the first caller happened to read.
//!
//! Records are held in INSERTION ORDER. `ListPaymentInstruments`,
//! `ListFinancialRecords` and `GetAccountBalance` are all `@paginated`, and a
//! stable order is what makes one `nextToken` resume exactly where the previous
//! page stopped without duplicating or skipping a record; [`pagination`] is the one
//! token format all three use, so their `nextToken` semantics cannot drift apart.
//!
//! The submodules split the store by concern:
//!
//! - [`wire_enums`] — the model's `enum` shapes as Rust enums, so a stored record
//!   cannot carry a value the model does not declare.
//! - [`records`] — what a stored instrument and a stored financial record are, and
//!   the two stores that hold them, with the lookups the operations resolve through.
//! - [`seed`] — the inventory the account starts with.
//! - [`projections`] — a stored record rendered as the modelled wire shapes, so a
//!   listed record and the same record fetched by id can never disagree.
//! - [`balances`] — the account balance `GetAccountBalance` reports, DERIVED from
//!   the held records rather than stored.
//! - [`pagination`] — the opaque `nextToken` the three paginated operations share,
//!   minted and read through the workspace's own `page_tokens`.
//! - [`refusals`] — the wording the handlers state a shared failure in.

pub mod balances;
pub mod pagination;
pub mod projections;
pub mod records;
pub mod refusals;
pub mod seed;
pub mod wire_enums;

pub use balances::{account_balance_amounts, DerivedAmount};
pub use pagination::{page, query_scope, InvalidPaginationToken};
pub use records::{
    BillingPeriodRange, BusinessUnitRecord, CreditMemoInfo, FinancialRecordRecord,
    FinancialRecordStore, InvoiceInfo, Money, PaymentInstrumentRecord, PaymentInstrumentStore,
    RecordBalances, RecordDetails, RecordKey,
};
pub use service_common::page_tokens::PageTokenKey;
pub use wire_enums::{
    BalanceType, BankAccountType, BatchErrorCode, CardBrand, CardType, DueStatus,
    FinancialRecordSortField, FinancialRecordStatus, FinancialRecordType, InvoiceFrequency,
    LineOfBusiness, PaymentInstrumentStatus, PaymentMethod, SettlementStatus, SortOrder,
};

/// Per-service state for com-amazonaws-payments-awspayments. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsPaymentsAwspaymentsState {
    /// The payment instruments `ListPaymentInstruments` and `GetPaymentInstrument`
    /// report.
    pub payment_instruments: PaymentInstrumentStore,
    /// The invoices and credit memos `ListFinancialRecords`, `GetFinancialRecord`,
    /// `BatchGetFinancialRecord` and `GetAccountBalance` report.
    pub financial_records: FinancialRecordStore,
    /// The secret this store's `nextToken`s are authenticated under — one key for
    /// all three paginated operations, drawn when the store is built and never
    /// reported, so a caller cannot name a page position this store never offered.
    pub page_token_key: PageTokenKey,
}

impl Default for ComAmazonawsPaymentsAwspaymentsState {
    /// The inventory this account has — see [`seed::account_fixture`]. The store's
    /// INITIAL value, installed on the emulator's boot path by
    /// [`install_service_state`] rather than by whichever request reaches the store
    /// first, so every operation here stays a pure read: two callers see the same
    /// records whichever of them called first, and neither one's clock decides what
    /// they report.
    ///
    /// Both stores are built from ONE anchor instant, so the instants across them —
    /// when an instrument was registered, when a record was issued — belong to the
    /// same account history.
    fn default() -> Self {
        let fixture = seed::account_fixture(seed::anchor_now());
        Self {
            payment_instruments: PaymentInstrumentStore::new(fixture.payment_instruments),
            financial_records: FinancialRecordStore::new(fixture.financial_records),
            page_token_key: PageTokenKey::new(),
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPaymentsAwspaymentsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPaymentsAwspaymentsState>)
}

/// Build this service's store NOW, so its initial value is fixed when the emulator
/// is built rather than when a request first reads it.
///
/// The engine builds a service's state on first access, and every operation this
/// service binds is a report. Left to first access, the initial value — including
/// the anchor instant [`seed::account_fixture`] dates the inventory from — would be
/// decided by whichever request got there first: an emulator read an hour after it
/// started would report an invoice as issued an hour later than an emulator read
/// immediately. Called from `EmulatedService::new`, which the emulator runs once
/// per service while it boots, before any request is served.
///
/// Idempotent and lock-free for later callers: the engine keeps the state it built
/// first, so calling this again (or a reader that somehow got there first) observes
/// that one store. A `None` answer — an engine this crate's store does not live in —
/// leaves nothing to install, and the handler states it as a server error.
pub fn install_service_state<Eng: Send + Sync + 'static>(engine: &Eng) {
    // The handle itself is of no use here: resolving it is what builds the store,
    // and the handlers reach it through `service_state` per request. So the handle
    // is dropped deliberately, and a `None` needs no answer at this seam.
    drop(service_state(engine));
}
