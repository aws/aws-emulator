Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rum.put-rum-events-id-not-matching-a-monitor-denied rum.put-rum-events-malformed-batch-rejected-and-not-stored
Issue-Shape-Id: com.amazonaws.rum#RUM
Issue-Key: rum.put-rum-events-id-not-matching-a-monitor-denied|run#17|AssertionError|assert got == DENIED, f"an Id breaking the UUID pattern gave {got!r}, expected {DENIED!r}"
Issue-Key: rum.put-rum-events-malformed-batch-rejected-and-not-stored|_refusal#8|AssertionError|raise AssertionError(f"expected PutRumEvents to be refused, but it was accepted: {result!r}")
Issue-Commit: 60ca35feb7abf53f0372b3f5577ee088cbe36d63

# RUM PutRumEvents refusals diverge: a pattern-breaking Id answers 400 ValidationException instead of 403 AccessDeniedException, and a 0-event RumEvents batch is accepted with 200

> _Note: this issue was identified against the code base at commit `60ca35feb7abf53f0372b3f5577ee088cbe36d63`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRumEvents

## Description
This issue bundles TWO distinct net-new root causes in the RUM data plane operation PutRumEvents. BOTH must be closed before submitting; each is numbered below and carries its own failing check ids.

Cause 1 of 2: an Id that breaks the app-monitor-Id pattern is refused as a constraint violation instead of a permission denial.
AWS: Smithy: PutRumEvents has http POST /appmonitors/{Id}/ with hostPrefix 'dataplane.'; its Id member is @required, @httpLabel, targets AppMonitorId with @length min 36 max 36 and @pattern ^[a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}$; modelled errors include AccessDeniedException (httpError 403) and ValidationException (httpError 400). Recorded behavior: on this data-plane route the resource/authorization decision is taken on the Id BEFORE the member constraint is reported, so ANY Id that is not an app monitor the caller may write to is refused with HTTP 403, code AccessDeniedException, message 'Caller principal does not have permissions for that resource.' — entirely fixed text, no variable parts. That holds both for a case-variant spelling of a live Id (the Id match is exact/case-sensitive) and for a value that cannot be an app monitor Id at all because it breaks the UUID pattern; AWS deliberately collapses the two into the same denial and never reveals, through a constraint message, that the value was merely malformed.
Emulator: the case-variant leg is correct (403 AccessDeniedException with that exact message), but an Id breaking the pattern is answered HTTP 400, code ValidationException, message "1 validation error detected: Value 'gggggggg-9ad1-4c1a-8dc8-e1b939d37e67' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}" — i.e. the path-label pattern check runs ahead of the authorization/resolution decision. (In that message the fixed text is '1 validation error detected: Value ... at ... failed to satisfy constraint: Member must satisfy regular expression pattern: ' and the variable parts are the rejected value, the member path and the pattern; none of it belongs on this route.)
Why: a client gets the wrong HTTP class and the wrong error code for the same logical outcome — 400 ValidationException is a never-retry input error while 403 AccessDeniedException is a credentials/permissions outcome, so SDK retry and error-branching logic diverges. It also turns PutRumEvents into an oracle: a caller can tell 'that id is syntactically impossible' from 'that id is not yours', a distinction AWS refuses to expose on an unauthenticated-telemetry endpoint. Note the related FIXED issue 20261102T105118Z-rum-putrumevents-accessdeniedexception-names-the-app-monitor-arn-instead.md: the denial message itself is now right (the case-variant leg passes), so this is not that defect — it is the ORDER in which the Id constraint and the denial are evaluated.
Fix: the PutRumEvents handler's request-validation ordering — the Id path-label length/pattern check must not be reported ahead of the 'does this Id resolve to an app monitor this caller may write to' decision; an Id that does not resolve, for whatever reason, must take the AccessDeniedException branch.
Traces: rum.put-rum-events-id-not-matching-a-monitor-denied.

Cause 2 of 2: a RumEvents batch of zero events is accepted instead of refused.
AWS: Smithy: PutRumEvents.RumEvents is @required and targets RumEventList, which carries NO @length trait, so the batch-size rule is service-side and is recorded behavior only: a request whose RumEvents list is empty is refused with HTTP 400, code ValidationException, message 'Invalid RumEvents size. Valid size is in range [1, 100]' — fixed text in which 1 and 100 are the service's batch bounds (the same message and bounds govern the upper end, a 101-event batch), and nothing is ingested.
Emulator: the empty-batch request is ACCEPTED — HTTP 200 with the empty PutRumEventsResponse body ({}), no error at all. The size rule is simply not applied, so the refusal class, status and message are all absent.
Why: a client that sends a degenerate or truncated batch (a common result of a client-side buffer flush bug) is told the call succeeded while no telemetry was stored, so the fault is silent and undiagnosable; code that branches on ValidationException to drop and rebuild a bad batch never runs. Because the 1..100 rule is one check, the upper bound is unverifiable here too — the check never got past the empty-batch leg.
Fix: the PutRumEvents handler's input validation — enforce the RumEvents batch-size bounds (1..100) before ingestion and emit ValidationException with the service's size message.
Traces: rum.put-rum-events-malformed-batch-rejected-and-not-stored.

## Evidence
Suite: 74/85 passed; all 11 failures are in service `rum` (bootstrapped as service-com-amazonaws-rum-rum). Nine of the eleven are marked OWNED by 20261029T192752Z-rum-app-monitor-data-plane-resource-policy-tagging.md and are skipped; the two below are not owned and not ALREADY FILED.

Cause 1 — rum.put-rum-events-id-not-matching-a-monitor-denied [mismatch]:
  AssertionError: an Id breaking the UUID pattern gave (400, 'ValidationException', "1 validation error detected: Value 'gggggggg-9ad1-4c1a-8dc8-e1b939d37e67' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}"), expected (403, 'AccessDeniedException', 'Caller principal does not have permissions for that resource.')
  The preceding leg of the same check — the UPPER-CASE spelling of the live app monitor Id — passed, i.e. the emulator already answers 403 AccessDeniedException with exactly that message for an Id that is well-formed but not the monitor's. Only the pattern-breaking Id (the live Id with its first 8 hex digits replaced by 'g') diverges.

Cause 2 — rum.put-rum-events-malformed-batch-rejected-and-not-stored [mismatch]:
  AssertionError: expected PutRumEvents to be refused, but it was accepted: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '2', ...}}}
  The refused request was PutRumEvents(Id=<live monitor Id>, BatchId=<uuid>, AppMonitorDetails={id,version}, UserDetails={sessionId,userId}, RumEvents=[]). Expected (400, 'ValidationException', 'Invalid RumEvents size. Valid size is in range [1, 100]'). The check stops at this first leg, so its later legs (101-event batch, non-UUID BatchId/userId, 256-character Alias, and the 'none of the refused events is stored' read-back) were never exercised and are not claimed here.

## Steps to Reproduce
Cause 1: rum.create_app_monitor(Name=m, Domain='example.com', CwLogEnabled=False) -> Id (a lower-case UUID). Then call PutRumEvents with a well-formed batch (BatchId=<uuid>, AppMonitorDetails={id,version}, UserDetails={sessionId:<uuid>,userId:<uuid>}, RumEvents=[one event with a UUID id, a timestamp, type 'com.amazon.rum.session_start_event', metadata and details JSON strings]) three times, varying only Id:
  (a) Id=<the live Id>            -> must be accepted (200).
  (b) Id=<the live Id>.upper()    -> must be 403 AccessDeniedException 'Caller principal does not have permissions for that resource.' (passes today).
  (c) Id='g'*8 + <live Id>[8:]    -> observed 400 ValidationException "1 validation error detected: Value '<that value>' at 'id' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-fA-F0-9]{8}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{4}-[a-fA-F0-9]{12}"; expected the SAME 403 AccessDeniedException as (b). Neither refused Id may appear in ListAppMonitors, and only the events from (a) may be readable through GetAppMonitorData.

Cause 2: with the same live monitor, call PutRumEvents(Id=<live Id>, BatchId=<uuid>, AppMonitorDetails={id,version}, UserDetails={sessionId,userId}, RumEvents=[]) -> observed HTTP 200 with body {}; expected 400 ValidationException 'Invalid RumEvents size. Valid size is in range [1, 100]' and nothing ingested. The same message and refusal are expected for a batch of 101 events, while a batch of exactly 100 must be accepted and all 100 stored.
