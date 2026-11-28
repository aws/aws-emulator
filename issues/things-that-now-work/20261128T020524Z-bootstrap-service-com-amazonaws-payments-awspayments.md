Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 7d4be16e9d1c3a57c05891543f71c24974370ddd

# Bootstrap the AWS Payments emulator

_Filed: 2026-11-28T02:05:24Z_
_Operation: service-com-amazonaws-payments-awspayments | Class: missing_

> _Note: this issue was identified against the code base at commit `7d4be16e9d1c3a57c05891543f71c24974370ddd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold answering HTTP 501, and `src/state.rs` holds no fields at all (`// TODO: add service-specific state fields`). Implement a first functional slice so a realistic client can walk the account's payment instruments and financial records end-to-end.

This service is entirely read-only (all six operations carry `@readonly`; there is no Create/Update/Delete anywhere in the model), so the lifecycle to make work is SEED -> LIST -> GET -> BATCH-GET -> AGGREGATE. State must be a real, shared, seeded store reached through `crate::state::service_state(&*self.engine)`; every response must be COMPUTED from that store. No canned bodies, no echoing the request back, no `format!`-ing an ARN from the identifier the caller passed and returning it as if it were a stored instrument.

### Happy path (implement these 6, in this order)

1. **ListPaymentInstruments** — input `nextToken` (len 1..2048), `maxResults` (default 25). Output `paymentInstrumentSummaries` (required, len ..25) built from the seeded instruments, each a `PaymentInstrumentSummary{id, arn, paymentMethod, status, createdAt, updatedAt}` (`id`, `arn`, `paymentMethod`, `status` all required). Honour `maxResults` and return an opaque `nextToken` when more instruments remain; a follow-up call with that token must continue where the previous page stopped and must not repeat or drop an instrument.
2. **GetPaymentInstrument** — input `identifier` (required), which the pattern allows as EITHER a bare id (`^[a-z0-9-]{1,64}$`) or a full `arn:...:payment-instrument:<id>`; both forms must resolve to the SAME stored instrument ("Specify one of the following: The Amazon Resource Name (ARN) ... The ID of the instrument, as ListPaymentInstruments returns in id"). Output `paymentInstrument` = `PaymentInstrument{arn, paymentMethod, status, details, createdAt, updatedAt}` — the stored record, including the method-specific `details` union member (`card`, `usAchBankAccount`, `sepaBankAccount`, ...) that `ListPaymentInstruments` does not carry. Unknown id/ARN -> `ResourceNotFoundException` (404), not a 200 with a fabricated instrument.
3. **ListFinancialRecords** — input `types` (required, 1..20), `settlementStatuses` (1..2), `issuedAfter`, `issuedBefore`, `sortBy` (`ISSUED_AT`), `sortOrder` (`ASCENDING`|`DESCENDING`), `nextToken`, `maxResults` (default 100). Output `financialRecordSummaries` (required) + `nextToken`. Every filter must actually filter the seeded records: a record is returned only when its `type` is in `types`, its `settlementStatus` is in `settlementStatuses` when that member is present, and its `issuedAt` falls inside the `issuedAfter`/`issuedBefore` window. `sortOrder=DESCENDING` must reverse the `issuedAt` ordering. Pagination must be stable under the same filter set.
4. **GetFinancialRecord** — input is `identifier` (the Payments ID, pattern `^[A-Z]{2,3}-[0-9]{4}-[0-9]{4}-[0-9]{4}(-[0-9]{4})?$`) OR `invoiceIdentifier` (the Invoicing invoice ID); neither is `@required`, so the handler owns the "exactly one" rule — both or neither -> `ValidationException` (400). Output `financialRecord` = the stored `FinancialRecord{id, type, businessUnit, amount, balances, settlementStatus, status, issuedAt, closedAt, details}`. Looking a record up by `invoiceIdentifier` must find the same record whose `details.invoiceDetails.invoiceId` equals that value and return the identical body as the `identifier` lookup. Unknown key -> `ResourceNotFoundException` (404).
5. **BatchGetFinancialRecord** — input `keys` (required, 1..100) of `FinancialRecordKey` (a union of `identifier` | `invoiceIdentifier`). Output BOTH required lists: `financialRecords` for the keys that resolve, and `errors` for the ones that do not — one `FinancialRecordError{key, code, message}` per failed key with `code = NOT_FOUND` and `key` echoed back exactly as requested. A missing key must NOT fail the whole batch; a partially-bad batch is a 200 with a non-empty `errors` list. Share the lookup helper with GetFinancialRecord so the two cannot disagree.
6. **GetAccountBalance** — input `balanceType` (required: `INVOICES_DUE` | `INVOICES_PAST_DUE` | `OUTSTANDING_CREDIT_MEMOS`), `nextToken`, `maxResults` (default 100). Output `accountBalances` (required) of `AccountBalance{amount, merchantOfRecord}`. This must be DERIVED, not stored: sum the `balances.outstanding` of the seeded records with `status == OUTSTANDING`, grouped by (`businessUnit.merchantOfRecord`, `amount.currency`) and never summed across either, selecting `INVOICE` records with `details.invoiceDetails.dueStatus == DUE` for `INVOICES_DUE`, `PAST_DUE` for `INVOICES_PAST_DUE`, and `CREDIT_MEMO` records for `OUTSTANDING_CREDIT_MEMOS`. Changing the seeded records must change this answer — a constant list is a failure of this issue.

### State Model

Add to `ComAmazonawsPaymentsAwspaymentsState` in `src/state.rs` (replacing the `TODO`), seeded with a small deterministic fixture at state construction since no operation can create data:

- `payment_instruments: Vec<PaymentInstrumentRecord>` (insertion-ordered so paging is stable), each `PaymentInstrumentRecord { id: String, arn: String, payment_method: PaymentMethod, status: PaymentInstrumentStatus, details: Option<PaymentInstrumentDetails>, created_at: Timestamp, updated_at: Timestamp }`. `arn` ends in `:payment-instrument:{id}`; lookup resolves a bare id or a full ARN to one entry.
- `financial_records: Vec<FinancialRecordRecord>` (insertion-ordered), each `FinancialRecordRecord { id: String, type_: FinancialRecordType, business_unit: BusinessUnit{merchant_of_record, line_of_business}, amount: CurrencyAmount{value, currency}, balances: Balances{outstanding, processing, settled, canceled}, settlement_status: SettlementStatus, status: FinancialRecordStatus, issued_at: Timestamp, closed_at: Option<Timestamp>, details: FinancialRecordDetails }` where `details` is `invoiceDetails{invoice_id, due_status, billing_period, due_date, payment_terms, invoice_frequency}` for an `INVOICE` and `creditMemoDetails` for a `CREDIT_MEMO`.
- `invoice_id_index: HashMap<String, usize>` mapping `details.invoiceDetails.invoiceId` -> position in `financial_records`, so `invoiceIdentifier` lookups in both GetFinancialRecord and BatchGetFinancialRecord hit the same record.
- Seed invariants to uphold so derived answers are coherent: the four `balances` add up to `amount` in the same currency; `settlementStatus == OPEN` iff `status` is `OUTSTANDING`/`PROCESSING` and `CLOSED` iff `SETTLED`/`CANCELED`; `closedAt` present only when `CLOSED`; `dueStatus` present only while `status == OUTSTANDING`. Include at least two merchants of record and two currencies, and both an `INVOICE` with `dueStatus=DUE` and one with `PAST_DUE`, so GetAccountBalance's grouping is observable.
- Pagination: one shared opaque-token helper (offset or last-key based) used by all three paginated operations, so `nextToken` semantics cannot drift between them.

Summaries must be PROJECTIONS of the stored records (`FinancialRecordSummary` is the record minus `details`; `PaymentInstrumentSummary` is the instrument minus `details`), so a listed record and the same record fetched by id can never disagree.

Operations are all in scope here because the service has only these six; anything added to the model later stays 501 and is filed separately.

## Evidence

All six handlers are untouched scaffolds. `services/service-com-amazonaws-payments-awspayments/src/ops/get_payment_instrument.rs:4` — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and"; :177 — `Err(crate::scaffolded::Scaffolded::not_implemented("GetPaymentInstrument"))`. The same pair exists in `batch_get_financial_record.rs`, `get_account_balance.rs`, `get_financial_record.rs`, `list_financial_records.rs`, `list_payment_instruments.rs`.

No state exists yet: `services/service-com-amazonaws-payments-awspayments/src/state.rs:7-9` — `pub struct ComAmazonawsPaymentsAwspaymentsState {` / `    // TODO: add service-specific state fields` / `}`.

Contract, from `services/service-com-amazonaws-payments-awspayments/OPERATIONS.md`:
- :11 — "**Request routing:** `POST /` with header `X-Amz-Target: AWSPayments.<OperationName>`", protocol `awsJson1_0` (:10).
- :65-66 (ListPaymentInstruments) — "**Input:** `nextToken` (string, len 1..2048), `maxResults` (integer, default 25, required)" / "**Output:** `paymentInstrumentSummaries` (list<PaymentInstrumentSummary>, len ..25, required), `nextToken` (string, len 1..2048)".
- :51-53 (GetPaymentInstrument) — "**Input:** `identifier` (string, len 1..128, pattern ^(arn:[a-z0-9-]+:payments::[0-9]{12}:payment-instrument:[a-z0-9-]{1,64}|[a-z0-9-]{1,64})$, required)" / "**Output:** `paymentInstrument` (PaymentInstrument, required)" / errors include "`ResourceNotFoundException (HTTP 404)`".
- :58-59 (ListFinancialRecords) — "**Input:** `types` (list<FinancialRecordType>, len 1..20, required), `settlementStatuses` (...), `issuedAfter` (timestamp), `issuedBefore` (timestamp), `sortBy` (FinancialRecordSortField, one of: ISSUED_AT), `sortOrder` (SortOrder, one of: ASCENDING | DESCENDING), `nextToken` ..., `maxResults` (integer, default 100, required)".
- :44-46 (GetFinancialRecord) — "**Input:** `identifier` (string, len 17..23, pattern ^[A-Z]{2,3}-[0-9]{4}-[0-9]{4}-[0-9]{4}(-[0-9]{4})?$), `invoiceIdentifier` (string, len 1..1024 ...)" — neither marked required.
- :30-31 (BatchGetFinancialRecord) — "**Input:** `keys` (list<FinancialRecordKey>, len 1..100, required)" / "**Output:** `financialRecords` (list<FinancialRecord>, required), `errors` (list<FinancialRecordError>, required)".
- :37-38 (GetAccountBalance) — "**Input:** `balanceType` (BalanceType, one of: INVOICES_DUE | INVOICES_PAST_DUE | OUTSTANDING_CREDIT_MEMOS, required) ..." / "**Output:** `accountBalances` (list<AccountBalance>, required), `nextToken`".

Smithy docs that define the derived/shared behaviour above:
- GetAccountBalance: "Retrieves one balance of your account, selected by balance type, as one amount for each merchant of record and currency. Each amount is the sum of the outstanding balances of your invoices or credit memos that have the status OUTSTANDING. Amounts are never added together across merchants of record or currencies."
- `FinancialRecordKey` (union): "Specify exactly one member: the ID that Amazon Web Services Payments issued, in identifier, or the invoice ID that Amazon Web Services Invoicing issued, in invoiceIdentifier. If you specify both or neither, the request fails with ValidationException."
- `FinancialRecordError.code`: "NOT_FOUND – No financial record on your account matches the key." and `FinancialRecordError`: "The reason that one of the records requested from BatchGetFinancialRecord couldn't be returned. The other records in the request are unaffected."
- `PaymentInstrumentSummary`: "A payment instrument as it appears in ListPaymentInstruments. Call GetPaymentInstrument for the full instrument, including the details specific to its payment method such as a card's last four digits and expiration."
- `FinancialRecordSummary`: "A financial record as ListFinancialRecords returns it, without the details that are specific to its type."
- `Balances`: "The four balances add up to amount."
- `InvoiceDetails.invoiceId`: "GetFinancialRecord and BatchGetFinancialRecord also accept it, as invoiceIdentifier."

No behavioural tests exist yet beyond `tests/all/state_roundtrip.rs` (the scaffold's state-API example, per README.md:20-22).

## Steps to Reproduce

1. Start the emulator and point an AWS SDK/`aws payments` client at it (awsJson1_0, `POST /`, `X-Amz-Target: AWSPayments.<Op>`).
2. `ListPaymentInstruments` with `maxResults: 1` — expect 200 with exactly one `paymentInstrumentSummaries` entry carrying `id`, `arn`, `paymentMethod`, `status`, plus a `nextToken`. Today: HTTP 501 NotImplemented.
3. Call `ListPaymentInstruments` again with that `nextToken` — expect the NEXT instrument, never the first one again, and no `nextToken` on the final page.
4. `GetPaymentInstrument` with the `id` from step 2, then again with that instrument's `arn` — both must return 200 with the same `paymentInstrument` (same `paymentMethod`, `status`, `createdAt`) and a populated method-specific `details` member.
5. `GetPaymentInstrument` with `identifier: "no-such-instrument"` — expect `ResourceNotFoundException` (404), not a 200 containing a fabricated instrument.
6. `ListFinancialRecords` with `types: ["INVOICE"]`, `sortBy: "ISSUED_AT"`, `sortOrder: "DESCENDING"` — expect only INVOICE summaries, newest `issuedAt` first; repeat with `settlementStatuses: ["OPEN"]` and verify every returned record has `settlementStatus == OPEN`; repeat with an `issuedBefore` earlier than every seeded record and expect an empty list.
7. `GetFinancialRecord` with `identifier` = an `id` from step 6 — expect 200 whose `financialRecord` matches the summary field-for-field and adds `details.invoiceDetails`.
8. `GetFinancialRecord` with `invoiceIdentifier` = that record's `details.invoiceDetails.invoiceId` — expect the identical record body.
9. `GetFinancialRecord` with BOTH `identifier` and `invoiceIdentifier` set, and then with NEITHER set — expect `ValidationException` (400) in both cases.
10. `BatchGetFinancialRecord` with `keys: [{identifier: <real id>}, {identifier: "ZZ-0000-0000-0000"}]` — expect 200 with one entry in `financialRecords` and one in `errors` with `code: "NOT_FOUND"` and `key` echoed as sent.
11. `GetAccountBalance` with `balanceType: "INVOICES_DUE"` — expect one `accountBalances` entry per (merchantOfRecord, currency) whose `amount.value` equals the sum of `balances.outstanding` over the seeded OUTSTANDING/DUE invoices for that merchant and currency; `INVOICES_PAST_DUE` and `OUTSTANDING_CREDIT_MEMOS` must return different, correspondingly-derived totals.
12. Re-run the whole sequence against the same running emulator — every response must be identical (state is shared and persistent across calls, not rebuilt per request).
