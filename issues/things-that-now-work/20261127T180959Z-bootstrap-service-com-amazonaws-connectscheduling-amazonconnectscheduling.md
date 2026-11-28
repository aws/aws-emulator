Issue-Severity: CRITICAL
Issue-Gap-Class: missing
Issue-Commit: 6bf5602f354238abc50672155d03ed4fbeacf648

# Bootstrap the AmazonConnectScheduling emulator

_Filed: 2026-11-27T18:09:59Z_
_Operation: service-com-amazonaws-connectscheduling-amazonconnectscheduling | Class: missing_

> _Note: this issue was identified against the code base at commit `6bf5602f354238abc50672155d03ed4fbeacf648`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Every one of the 6 operations in this crate is an unmodified scaffold: `src/ops/create_time_off.rs:229` returns `Err(crate::scaffolded::Scaffolded::not_implemented("CreateTimeOff"))` and `src/state.rs:8` holds only `// TODO: add service-specific state fields`, so the service holds zero state.

Implement this FIRST SLICE — the time off request lifecycle — statefully, in order:

1. **PutTimeOffBalance** (`PUT /scheduling/instances/{InstanceId}/users/{UserId}/timeoff-balances/{ShiftActivityId}`) — inputs `InstanceId`, `UserId`, `ShiftActivityId` (all @httpLabel, required) and `Balance` (`Duration{Value:int -52560000..52560000, Unit:enum}`, required). Empty output. The stored balance must REPLACE any prior value for the (instance, user, activity) key, per the model: "The value replaces the existing balance rather than adjusting it, so you can safely retry the call."
2. **CreateTimeOff** (`POST .../users/{UserId}/timeoffs`) — inputs `ShiftActivityId`, `StartAt`, `EndAt`, `ApprovalType` (`AUTOMATIC | AWAIT_APPROVAL`, required), optional `Description`, `RuleFailureAction` (`OVERRIDE | REJECT`), `ClientToken` (@idempotencyToken). Output `TimeOffId` (required, must match `^[a-z0-9-]+$`, len 1..64). Generate the id, persist the whole request (window, activity, description, approval type, status, created-by/at), and return the id of the record actually stored — not a random id without storage. Status is assigned by the emulator, never taken from input: `AWAIT_APPROVAL` → `AWAITING_APPROVAL`; `AUTOMATIC` → decide at once from stored state (balance check + overlap with the user's `APPROVED` time off) and end `APPROVED` or `REJECTED`. Repeating the same `ClientToken` must return the same `TimeOffId` without creating a second record.
3. **GetTimeOff** (`GET .../timeoffs/{TimeOffId}`) — must read back the persisted record: `TimeOffId`, `UserId`, `ShiftActivityId`, `StartAt`, `EndAt`, `Status` (`APPROVED | AWAITING_APPROVAL | CANCELLED | PENDING | REJECTED`), `Description`, `ValidationResults` (list of `TimeOffValidationResult{Severity, Code}`), `CreatedBy`/`CreatedAt`, `UpdatedBy`/`UpdatedAt`. Unknown id ⇒ `ResourceNotFoundException` (404). This is the read-only verification step — it must never write.
4. **ApproveTimeOff** (`POST .../timeoffs/{TimeOffId}/approve`) — optional `Description`, `RuleFailureAction`. Empty output. Flip the stored record to `APPROVED`, bump `UpdatedAt`/`UpdatedBy`, and record `ValidationResults`: on an overlap with another `APPROVED` request for the same user, store `Severity=ERROR, Code=CONFLICTING_TIME_OFF_REQUEST` and end `REJECTED`; on an insufficient stored balance, store `INSUFFICIENT_USER_TIME_OFF_BALANCE` as `ERROR` with `RuleFailureAction=REJECT` or as `WARNING` (and approve) with `OVERRIDE`. Approving an already-`APPROVED` request succeeds without change; `PENDING` or `CANCELLED` ⇒ `ConflictException` (409).
5. **RejectTimeOff** (`POST .../timeoffs/{TimeOffId}/reject`) — set the stored status to `REJECTED`, bump `UpdatedAt`; rejecting a `REJECTED` request succeeds without change; `PENDING`/`CANCELLED` ⇒ `ConflictException`.
6. **CancelTimeOff** (`POST .../timeoffs/{TimeOffId}/cancel`) — set the stored status to `CANCELLED` and release the window it reserved; cancelling a `CANCELLED` request succeeds without a change; `PENDING` ⇒ `ConflictException` (409).

Every response must be computed from persisted state: no canned bodies, no echoing the input back, no generating a `TimeOffId` that is never stored. A `CreateTimeOff` followed by `GetTimeOff` must return exactly what was created; an `ApproveTimeOff`/`CancelTimeOff` must be observable in the next `GetTimeOff`. Errors must be the modeled shapes (`ConflictException` 409, `ResourceNotFoundException` 404, `ValidationException` 400), not 501.

## State Model

In `ComAmazonawsConnectschedulingAmazonconnectschedulingState` (`src/state.rs`):

- `instances: HashMap<String /*normalized InstanceId (id or ARN tail)*/, Instance>`
- `Instance { time_offs: HashMap<String /*TimeOffId*/, TimeOffRequest>, balances: HashMap<(String /*UserId*/, String /*ShiftActivityId*/), Duration>, client_tokens: HashMap<String /*ClientToken*/, String /*TimeOffId*/> }`
- `TimeOffRequest { time_off_id: String, user_id: String, shift_activity_id: String, start_at: DateTime, end_at: DateTime, status: TimeOffStatus, approval_type: ApprovalType, rule_failure_action: Option<RuleFailureAction>, description: Option<String>, validation_results: Vec<TimeOffValidationResult>, created_by: ActorInfo, created_at: DateTime, updated_by: Option<ActorInfo>, updated_at: DateTime }`
- `Duration { value: i32, unit: DurationUnit }` stored as sent so `PutTimeOffBalance` replaces, never accumulates.

Lookups are keyed by (instance, user) so one user's requests and balances never leak into another's. The remaining behaviours outside this slice (asynchronous `PENDING` settling, group/staffing-group allowances, overtime and voluntary-time-off overlap, state-change events) stay stubbed and will be filed as individual gap issues later.

## Evidence

All handlers are scaffolds: `services/service-com-amazonaws-connectscheduling-amazonconnectscheduling/src/ops/create_time_off.rs:4` — "//! SCAFFOLDED, NOT IMPLEMENTED: this operation answers HTTP 501 `NotImplemented`" — and :229 `Err(crate::scaffolded::Scaffolded::not_implemented("CreateTimeOff"))`. No state exists: `src/state.rs:7-9` `pub struct ComAmazonawsConnectschedulingAmazonconnectschedulingState { // TODO: add service-specific state fields }`.

Operation surface, from `OPERATIONS.md:25-69`:
- `OPERATIONS.md:42-45` CreateTimeOff — "**Route:** `POST /scheduling/instances/{InstanceId}/users/{UserId}/timeoffs -> 200`"; input `ShiftActivityId`, `StartAt`, `EndAt`, `ApprovalType (ApprovalType, one of: AUTOMATIC | AWAIT_APPROVAL, required)`, `ClientToken (... @idempotencyToken)`; output "`TimeOffId` (string, len 1..64, pattern ^[a-z0-9-]+$, required)".
- `OPERATIONS.md:49-52` GetTimeOff — "@readonly"; output "`Status` (TimeOffStatus, one of: APPROVED | AWAITING_APPROVAL | CANCELLED | PENDING | REJECTED, required), `Description` ..., `ValidationResults` (list<TimeOffValidationResult>), `CreatedBy` (ActorInfo, required), `CreatedAt` (timestamp, required), `UpdatedBy` (ActorInfo), `UpdatedAt` (timestamp, required)".
- `OPERATIONS.md:57-61` PutTimeOffBalance — "`Balance` (Duration, required)".

Smithy docs grounding the lifecycle (smithy_query query="doc"):
- CreateTimeOff: "You don't provide a status, because Connect Customer owns the request lifecycle. To get the decided status, call `GetTimeOff`" ... "`AWAIT_APPROVAL`: the request waits in `AWAITING_APPROVAL` until you call `ApproveTimeOff` or `RejectTimeOff`." ... "If it overlaps another time off request for the user, the requested time off ends `REJECTED` with `CONFLICTING_TIME_OFF_REQUEST`."
- ApproveTimeOff: "The request is rejected with `CONFLICTING_TIME_OFF_REQUEST` if it overlaps another time off request for the user that is `APPROVED` or being approved." ... "Approving an `APPROVED` request succeeds without a change." ... "Returns `ConflictException` if the request is `PENDING` or `CANCELLED`."
- CancelTimeOff: "Cancelling a `CANCELLED` request succeeds without a change. Returns `ConflictException` if the request is `PENDING`."
- PutTimeOffBalance: "Sets the time off balance for one user and shift activity. The value replaces the existing balance rather than adjusting it, so you can safely retry the call."
- `PutTimeOffBalanceRequest.Balance`: "The new time off balance for the user and activity. It replaces the existing balance."
- `TimeOffValidationResult.Severity`: "`WARNING` is a breach of the configured rules that is not enough to reject the request ... `ERROR` is a reason the request was rejected."

Wire protocol: `OPERATIONS.md:10-12` "**Protocol:** `restJson1`", "**SDK id:** `ConnectScheduling`", "**sigv4 signing name:** `connect`".

## Steps to Reproduce

1. Start the emulator and point an AWS SDK `connectscheduling` client at it with an instance id like 11111111-2222-3333-4444-555555555555.
2. PutTimeOffBalance(InstanceId, UserId="user-1", ShiftActivityId="vacation", Balance={Value: 2400, Unit: MINUTES}) → expect HTTP 200 and the balance persisted; today returns 501 NotImplemented.
3. CreateTimeOff(InstanceId, UserId="user-1", ShiftActivityId="vacation", StartAt, EndAt, ApprovalType="AWAIT_APPROVAL", Description="spring break", ClientToken="tok-1") → expect 200 with a TimeOffId matching ^[a-z0-9-]+$.
4. Repeat the identical CreateTimeOff with ClientToken="tok-1" → expect the SAME TimeOffId and no second record.
5. GetTimeOff(InstanceId, UserId="user-1", TimeOffId) → expect Status=AWAITING_APPROVAL and StartAt/EndAt/ShiftActivityId/Description identical to what was created, plus CreatedAt/CreatedBy/UpdatedAt populated.
6. ApproveTimeOff(InstanceId, UserId="user-1", TimeOffId, RuleFailureAction="REJECT") → 200; GetTimeOff → Status=APPROVED with a bumped UpdatedAt.
7. Create a second overlapping request with ApprovalType="AUTOMATIC" and approve/evaluate it → GetTimeOff → Status=REJECTED with ValidationResults containing {Severity: ERROR, Code: CONFLICTING_TIME_OFF_REQUEST}.
8. CancelTimeOff(InstanceId, UserId="user-1", TimeOffId of the approved request) → 200; GetTimeOff → Status=CANCELLED; call CancelTimeOff again → still 200 and still CANCELLED.
9. ApproveTimeOff on the CANCELLED request → expect ConflictException (HTTP 409), not 501.
10. GetTimeOff with an unknown TimeOffId → expect ResourceNotFoundException (HTTP 404).
