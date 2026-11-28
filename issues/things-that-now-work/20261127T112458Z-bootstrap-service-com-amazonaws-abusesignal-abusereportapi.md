Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 50a80390bb1e2ddab0fc937c65e59ad0dcd543f4

# Bootstrap the Abuse Signal AbuseReportApi emulator

_Filed: 2026-11-27T11:24:58Z_
_Operation: service-com-amazonaws-abusesignal-abusereportapi | Class: missing_

> _Note: this issue was identified against the code base at commit `50a80390bb1e2ddab0fc937c65e59ad0dcd543f4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every operation in this crate is an unmodified scaffold answering HTTP 501 and holding no state (all three files under `src/ops/` carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` header). The service's entire surface is only 3 operations, so the first slice is the complete core loop:

1. **CreateAbuseReport** — `POST /v1/abuse` -> HTTP 202. Input: `report` (ReportData union, required — the only member is `xarf`, a Document), optional `correlationId` (1..256, `^[a-zA-Z0-9-_]{1,256}$`), optional `clientToken` (1..128, `^[\x21-\x7E]{1,128}$`). Output: `reportId` (required, 20..64 chars matching `^[a-zA-Z0-9]{20,64}$` — generate an id that actually satisfies the pattern and STORE it) and `correlationId` echoed back from the stored record. Idempotency per the model: "If you provide a `clientToken`, duplicate requests within 24 hours return the existing report ID" — so keep a token -> reportId index and return the SAME `reportId` for a repeat of the same token+payload; a token reused with a different payload is `ConflictException` (409) ("an idempotency token was reused with a different payload"). A `report` union with no member / unusable content is `SubmissionRequirementException` (422).

2. **GetAbuseReportStatus** — `GET /v1/abuse/{reportId}`. Looks up the stored record by `reportId`; unknown id -> `ResourceNotFoundException` (404). Output must come from the stored record: `reportId`, `correlationId`, `status` (QUEUED | PROCESSING | COMPLETED, required), `description` (required, human-readable text matching the current status), `reasonCode` (only present at `COMPLETED`, per the model: "This value is present only after the report reaches the terminal `COMPLETED` status; it is absent while the report is `QUEUED` or `PROCESSING`"), `submittedAt` (required, the create time) and `lastUpdatedAt`. A newly created report must come back as QUEUED (or whatever state the crate's own progression sets) — never a canned status for an id that was never created.

3. **ListAbuseReportStatuses** — `GET /v1/abusestatuses`. Returns `reports` (required list of ReportStatusListItem: `reportId`, `correlationId`, `status`, `description`, `reasonCode`) built from the stored reports, "ordered by submission time". Honour `correlationId` as an exact-match filter, `maxResults` (1..50) as the page size, and `nextToken` (1..1024) as the resume cursor, emitting an output `nextToken` only when more reports remain. An empty store yields an empty list, not an error.

All three must compute their responses from the SAME shared per-service state reached via `crate::state::service_state(&*self.engine)` — no canned bodies, no echoing the input back without persisting, no random reportId that is never stored. A create followed by a get and a list must agree on id, correlationId, status and timestamps.

## State Model

In `src/state.rs` (`ComAmazonawsAbusesignalAbusereportapiState`):

- `reports: BTreeMap<String /* reportId */, AbuseReport>` — the authoritative store.
- `order: Vec<String>` — reportIds in submission order (or derive ordering from `submitted_at` + id as a tiebreaker) so List is deterministic and paginable.
- `idempotency: HashMap<String /* clientToken */, IdempotencyRecord>`.
- `struct AbuseReport { report_id: String, correlation_id: Option<String>, client_token: Option<String>, report: /* stored XARF document */, status: ReportSubmissionStatus, description: String, reason_code: Option<ReasonCode>, submitted_at: DateTime, last_updated_at: Option<DateTime> }`.
- `struct IdempotencyRecord { report_id: String, payload_fingerprint: String /* hash of report+correlationId, to detect a token reused with a different payload -> ConflictException */, created_at: DateTime }`.

List pages are cut from `order` and the `nextToken` encodes the position/last-seen reportId; Get reads `reports`; Create inserts into `reports`, pushes onto `order`, and records `idempotency`.

## Evidence

All handlers are scaffolds: `services/service-com-amazonaws-abusesignal-abusereportapi/src/ops/create_abuse_report.rs:4`, `.../src/ops/get_abuse_report_status.rs:4`, `.../src/ops/list_abuse_report_statuses.rs:4` each read `//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented` and`. README confirms the scaffold contract: "Every generated stub starts out **scaffolded, not implemented**: it is fully typed and registered, and it answers **HTTP 501 `NotImplemented`** until you write it." (`.../README.md:38-42`).

Contract, from OPERATIONS.md (`.../OPERATIONS.md:26-48`):
- "## CreateAbuseReport — **Route:** `POST /v1/abuse -> 202` — **Traits:** @idempotent — **Input:** `report` (ReportData, required), `correlationId` ..., `clientToken` ... — **Output:** `reportId` (string, len 20..64, pattern ^[a-zA-Z0-9]{20,64}$, required), `correlationId` ... — **Errors:** `ConflictException (HTTP 409)`, `SubmissionRequirementException (HTTP 422)`"
- "## GetAbuseReportStatus — **Route:** `GET /v1/abuse/{reportId}` ... **Output:** `reportId` ..., `correlationId` ..., `status` (ReportSubmissionStatus, one of: COMPLETED | PROCESSING | QUEUED, required), `description` (required), `reasonCode` (ReasonCode, ...), `submittedAt` (timestamp, required), `lastUpdatedAt` — **Errors:** `ResourceNotFoundException (HTTP 404)`"
- "## ListAbuseReportStatuses — **Route:** `GET /v1/abusestatuses` — **Traits:** @paginated(inputToken=nextToken, outputToken=nextToken, items=reports), @readonly ... **Output:** `reports` (list<ReportStatusListItem>, len 0..50, required), `nextToken`"

Smithy docs (via smithy_query) that pin the stateful behaviour:
- CreateAbuseReport: "This operation is idempotent within a time window. If you provide a `clientToken`, duplicate requests within 24 hours return the existing report ID."
- ConflictException: "The request conflicts with the current state of the resource. For example, an idempotency token was reused with a different payload."
- ReportStatusListItem.reasonCode: "This value is present only after the report reaches the terminal `COMPLETED` status; it is absent while the report is `QUEUED` or `PROCESSING`."
- ListAbuseReportStatuses: "Lists abuse report statuses for the calling account, ordered by submission time, with optional filtering and pagination. Use the `correlationId` parameter to filter results ... Use `nextToken` to retrieve additional results."

## Steps to Reproduce

1. Start the emulator and point an AWS SDK client for `abuse` (restJson1, signing name `abuse`) at it.
2. POST /v1/abuse with body {"report":{"xarf":{...}},"correlationId":"case-1","clientToken":"tok-1"} — expect HTTP 202 and a `reportId` matching ^[a-zA-Z0-9]{20,64}$ plus `correlationId`: "case-1". Today: HTTP 501 NotImplemented.
3. Repeat the identical POST with clientToken "tok-1" — expect the SAME `reportId` (idempotent replay), not a new one.
4. POST again with clientToken "tok-1" but a different `report` payload — expect ConflictException (HTTP 409).
5. GET /v1/abuse/{reportId} — expect the created report back: same `reportId`, `correlationId`: "case-1", a required `status` from {QUEUED,PROCESSING,COMPLETED}, a non-empty `description`, `submittedAt` equal to the create time, and NO `reasonCode` while status is not COMPLETED. Today: HTTP 501.
6. GET /v1/abuse/AAAAAAAAAAAAAAAAAAAA (never created) — expect ResourceNotFoundException (HTTP 404), not 501 and not a fabricated record.
7. Create 3 reports (two with correlationId "case-1", one with "case-2"), then GET /v1/abusestatuses — expect all 3 in submission order with their real ids/statuses. Today: HTTP 501.
8. GET /v1/abusestatuses?correlationId=case-1 — expect exactly the 2 matching reports.
9. GET /v1/abusestatuses?maxResults=1 — expect 1 item plus a `nextToken`; follow the token to walk all reports exactly once with no duplicates and no `nextToken` on the final page.
