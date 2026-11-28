// SPDX-License-Identifier: CC0-1.0

//! The harness every suite in this crate drives the emulator through.
//!
//! Each case sends a real awsJson1_0 request — `POST /` with
//! `X-Amz-Target: AWSPayments.<Operation>` — through
//! [`MultiServiceRouter::dispatch`], the same path the emulator's front door uses,
//! so the assertions are about HTTP behaviour and not about a handler's internals.
//!
//! The store is reached through the state seam, because this service models no
//! create, update or delete: Amazon Web Services issues these records, so a case
//! that reasons about particular records installs them with
//! `financial_records.seed(..)` / `payment_instruments.seed(..)` and a case that
//! reasons about the account as the emulator starts it reads the inventory the store
//! already holds.

#![allow(dead_code)]

use std::sync::Arc;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_payments_awspayments as payments;

use payments::state::{
    BillingPeriodRange, BusinessUnitRecord, CreditMemoInfo, DueStatus, FinancialRecordRecord,
    FinancialRecordStatus, InvoiceFrequency, InvoiceInfo, LineOfBusiness, RecordBalances,
    RecordDetails,
};

/// A SigV4 `Authorization` header whose credential scope names this service's
/// signing name (`payments`, per the crate's OPERATIONS.md) — the primary routing
/// signal the router reads. The signature is not checked: request authentication is
/// off for these tests.
pub const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20261128/us-east-1/payments/aws4_request, SignedHeaders=host;x-amz-target, Signature=0000000000000000000000000000000000000000000000000000000000000000";

/// The media type an awsJson1_0 client sends, as the model's protocol trait
/// declares it.
pub const CONTENT_TYPE: &str = "application/x-amz-json-1.0";

/// One emulator: a shared engine, and a router with this service mounted on it.
pub struct Emulator {
    engine: Arc<engine::InMemoryEngine>,
    router: MultiServiceRouter,
}

impl Emulator {
    /// A fresh emulator: its store holds the inventory this account has.
    pub fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(payments::EmulatedService::new(Arc::clone(&engine)));
        Self { engine, router }
    }

    /// The same emulator with that inventory cleared, so a case can install exactly
    /// the records it reasons about and nothing else.
    pub fn empty() -> Self {
        let emulator = Self::new();
        emulator.with_state(|state| {
            state.payment_instruments.clear();
            state.financial_records.clear();
        });
        assert!(
            emulator.with_state(|state| state.payment_instruments.is_empty()
                && state.financial_records.is_empty()),
            "case \"empty emulator\": clearing the account's inventory must leave both stores empty"
        );
        emulator
    }

    /// Read or change this service's store directly — the seeding seam.
    pub fn with_state<R>(
        &self,
        act: impl FnOnce(&mut payments::ComAmazonawsPaymentsAwspaymentsState) -> R,
    ) -> R {
        let state = self
            .engine
            .state::<payments::ComAmazonawsPaymentsAwspaymentsState>();
        let mut held = state.lock();
        act(&mut held)
    }

    /// Install financial records through the state seam.
    pub fn seed_records(&self, records: Vec<FinancialRecordRecord>) {
        self.with_state(|state| state.financial_records.seed(records));
    }

    /// The IDs the store holds, in insertion order.
    pub fn held_record_ids(&self) -> Vec<String> {
        self.with_state(|state| {
            state
                .financial_records
                .records()
                .iter()
                .map(|record| record.id.clone())
                .collect()
        })
    }

    /// The instrument IDs the store holds, in insertion order.
    pub fn held_instrument_ids(&self) -> Vec<String> {
        self.with_state(|state| {
            state
                .payment_instruments
                .instruments()
                .iter()
                .map(|instrument| instrument.id.clone())
                .collect()
        })
    }

    /// The facts about each held financial record a case computes its expectation
    /// from, read off the STORE rather than off a response.
    pub fn held_records(&self) -> Vec<HeldRecord> {
        self.with_state(|state| {
            state
                .financial_records
                .records()
                .iter()
                .map(|record| HeldRecord {
                    id: record.id.clone(),
                    invoice_id: record.details.invoice_id().to_string(),
                    record_type: record.record_type().as_str().to_string(),
                    settlement_status: record.settlement_status().as_str().to_string(),
                    status: record.status.as_str().to_string(),
                    due_status: record.details.due_status().map(|due| due.as_str().to_string()),
                    merchant_of_record: record.business_unit.merchant_of_record.clone(),
                    currency: record.balances.outstanding.currency.clone(),
                    outstanding: record.balances.outstanding.value.to_string(),
                    issued_secs: record.issued_at.secs(),
                })
                .collect()
        })
    }

    /// Call `operation` with `body` as the request payload, reporting the response's
    /// status and payload.
    pub async fn call(&self, operation: &str, body: &Value) -> (u16, Value) {
        let request = http::Request::builder()
            .method("POST")
            .uri("/")
            .header("host", "payments.us-east-1.amazonaws.com")
            .header("content-type", CONTENT_TYPE)
            .header("x-amz-target", format!("AWSPayments.{operation}"))
            .header("authorization", AUTHORIZATION)
            .body(bytes::Bytes::from(
                serde_json::to_vec(body).expect("serialize the request body"),
            ))
            .expect("build the request");
        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let payload = if response.body().is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(response.body()).unwrap_or_else(|error| {
                panic!(
                    "response body is not JSON ({error}): {}",
                    String::from_utf8_lossy(response.body())
                )
            })
        };
        (status, payload)
    }

    /// `operation` with `body`, which must succeed; reports the payload.
    pub async fn ok(&self, operation: &str, body: &Value) -> Value {
        let (status, payload) = self.call(operation, body).await;
        assert_eq!(
            status, 200,
            "case {operation} {body}: expected 200, got {status} with {payload}"
        );
        payload
    }
}

impl Default for Emulator {
    fn default() -> Self {
        Self::new()
    }
}

/// What a case needs to know about one held financial record to say what a
/// filtered, sorted or summed answer over the store must be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeldRecord {
    pub id: String,
    pub invoice_id: String,
    pub record_type: String,
    pub settlement_status: String,
    pub status: String,
    pub due_status: Option<String>,
    pub merchant_of_record: String,
    pub currency: String,
    pub outstanding: String,
    pub issued_secs: i64,
}

/// The wire error code of a failure payload: the last segment of its `__type`.
pub fn error_code(payload: &Value) -> String {
    payload
        .get("__type")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .rsplit(['#', ':'])
        .next()
        .unwrap_or_default()
        .to_string()
}

/// One member of a payload as a list, or an empty list when it is absent.
pub fn list<'a>(payload: &'a Value, member: &str) -> &'a [Value] {
    payload
        .get(member)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
}

/// One string member of a payload, or the empty string when it is absent.
pub fn text(payload: &Value, member: &str) -> String {
    payload
        .get(member)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The `id` of each entry of a list member.
pub fn ids(payload: &Value, member: &str) -> Vec<String> {
    list(payload, member)
        .iter()
        .map(|entry| text(entry, "id"))
        .collect()
}

/// A billing period of one day ending at `end_secs` — enough for a record whose
/// billing period no case reasons about.
pub fn a_billing_period(end_secs: i64) -> BillingPeriodRange {
    BillingPeriodRange {
        start_date: aws_smithy_types::DateTime::from_secs(end_secs - 86_400),
        end_date: aws_smithy_types::DateTime::from_secs(end_secs),
    }
}

/// An invoice held for the account, with the balances and status a case needs.
///
/// `outstanding_minor` and `settled_minor` are minor units of `currency`, so the
/// amount this record reports is their sum.
#[allow(clippy::too_many_arguments)]
pub fn invoice(
    id: &str,
    invoice_id: &str,
    merchant_of_record: &str,
    currency: &str,
    balances: (i64, i64, i64, i64),
    status: FinancialRecordStatus,
    due_status: Option<DueStatus>,
    issued_secs: i64,
) -> FinancialRecordRecord {
    let (outstanding, processing, settled, canceled) = balances;
    FinancialRecordRecord {
        id: id.to_string(),
        business_unit: BusinessUnitRecord {
            merchant_of_record: merchant_of_record.to_string(),
            line_of_business: Some(LineOfBusiness::AwsCloudServices),
        },
        balances: RecordBalances::minor(currency, outstanding, processing, settled, canceled),
        status,
        issued_at: aws_smithy_types::DateTime::from_secs(issued_secs),
        closed_at: closed_at(status, issued_secs),
        details: RecordDetails::Invoice(InvoiceInfo {
            invoice_id: invoice_id.to_string(),
            due_status,
            billing_period: a_billing_period(issued_secs),
            due_date: aws_smithy_types::DateTime::from_secs(issued_secs + 30 * 86_400),
            payment_terms: Some("Net30".to_string()),
            invoice_frequency: InvoiceFrequency::Recurring,
        }),
    }
}

/// A credit memo held for the account.
pub fn credit_memo(
    id: &str,
    invoice_id: &str,
    merchant_of_record: &str,
    currency: &str,
    balances: (i64, i64, i64, i64),
    status: FinancialRecordStatus,
    issued_secs: i64,
) -> FinancialRecordRecord {
    let (outstanding, processing, settled, canceled) = balances;
    FinancialRecordRecord {
        id: id.to_string(),
        business_unit: BusinessUnitRecord {
            merchant_of_record: merchant_of_record.to_string(),
            line_of_business: Some(LineOfBusiness::AwsMarketplace),
        },
        balances: RecordBalances::minor(currency, outstanding, processing, settled, canceled),
        status,
        issued_at: aws_smithy_types::DateTime::from_secs(issued_secs),
        closed_at: closed_at(status, issued_secs),
        details: RecordDetails::CreditMemo(CreditMemoInfo {
            invoice_id: invoice_id.to_string(),
            billing_period: a_billing_period(issued_secs),
        }),
    }
}

/// When a record of `status` closed: the invariant the model states, so a record a
/// case builds carries `closedAt` exactly when it is closed.
fn closed_at(status: FinancialRecordStatus, issued_secs: i64) -> Option<aws_smithy_types::DateTime> {
    matches!(
        status,
        FinancialRecordStatus::Settled | FinancialRecordStatus::Canceled
    )
    .then(|| aws_smithy_types::DateTime::from_secs(issued_secs + 86_400))
}
