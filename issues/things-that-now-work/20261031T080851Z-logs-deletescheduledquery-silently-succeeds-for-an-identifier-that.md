Issue-Source: trace-sweep-divergence
Issue-Trace: logs-scheduled-query-workflow
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-scheduled-query-workflow
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DeleteScheduledQuery|4xx->2xx|$
Issue-Key: logs|DeleteScheduledQuery|4xx->2xx|status
Issue-Commit: 9a7530f5beb5c94af0d9f6e087fc2caa8680119d

# logs DeleteScheduledQuery silently succeeds for an identifier that names no scheduled query

> _Note: this issue was identified against the code base at commit `9a7530f5beb5c94af0d9f6e087fc2caa8680119d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteScheduledQuery

## Description
Cause 1 of 1: DeleteScheduledQuery accepts an identifier that resolves to no scheduled query and answers an empty 200 instead of the modeled not-found refusal.

AWS: DeleteScheduledQuery refuses an identifier that names no existing scheduled query with HTTP 400 and error code ResourceNotFoundException, message 'Scheduled query with identifier <identifier> not found'. Fixed text: 'Scheduled query with identifier ' ... ' not found'; the single variable part is the identifier exactly as supplied in the request (a scheduled-query name or ARN) — the value in the recording ('sim2real-probe-logs-scheduled-query-f03f6305') is a per-run generated name and is NOT part of the requirement. Smithy (com.amazonaws.cloudwatchlogs#Logs_20140328): DeleteScheduledQuery.identifier is required, target ScheduledQueryIdentifier @length 1..300, and the operation binds ResourceNotFoundException (httpError 400, defaulted) alongside ValidationException/AccessDeniedException/ThrottlingException/InternalServerException; its output shape DeleteScheduledQueryResponse has no members, so an empty body is only correct on a delete that actually removed a query. The not-found wording is the same 'Scheduled query with identifier <identifier> not found' template the corpus records for GetScheduledQuery on the same surface.

Emulator: answers HTTP 200 with an empty body ({}) — the delete is treated as unconditionally idempotent, so no lookup of the identifier against stored state is performed (or its failure is swallowed) and neither __type nor message is emitted.

Why: a client cannot distinguish 'the scheduled query was deleted by this call' from 'the identifier never existed / was already deleted'. Delete-then-confirm and cleanup-reconciliation loops that rely on the ResourceNotFoundException to decide a resource is already gone instead conclude every delete succeeded, so typo'd or stale identifiers are reported as removed resources, and error-handling paths keyed on ResourceNotFoundException are never exercised.

Fix: the identifier resolution and the not-found branch of the DeleteScheduledQuery handler in services/service-com-amazonaws-cloudwatchlogs-logs-20140328/src/ops/ — resolve both the name and ARN forms of identifier against scheduled-query state and raise ResourceNotFoundException (400) with the templated message when nothing is found, returning the empty response body only on an actual deletion.

Related open issue 20261029T222444Z-logs-scheduled-query-operations-accept-invalid-input-and.md covers CreateScheduledQuery/GetScheduledQuery/UpdateScheduledQuery causes on this surface and carries different replay keys; it does NOT cover DeleteScheduledQuery, which still has no filed replay key.

Traces: logs-scheduled-query-workflow#task#5.

## Evidence
Replay of logs-scheduled-query-workflow step task#5 (DeleteScheduledQuery after the query was already deleted): "status: expected 400, got 200" and "$: empty_body (expected {'__type': 'ResourceNotFoundException', 'message': 'Scheduled query with identifier sim2real-probe-logs-scheduled-query-f03f6305 not found'}, got {})". Expected status 400 with __type 'ResourceNotFoundException'; actual status 200 with an empty JSON body and no __type/message. Divergence key: service=logs|DeleteScheduledQuery|status=400->200|keys=$,status. In the expected message, 'Scheduled query with identifier ' and ' not found' are fixed text and the middle token is the identifier taken from the request.

## Steps to Reproduce
1. CreateScheduledQuery with some name N (or pick any name never created).
2. DeleteScheduledQuery(identifier=N) — succeeds, removing the query.
3. Call DeleteScheduledQuery(identifier=N) again (equivalently: call it once with an identifier that names no scheduled query).
4. Expected: HTTP 400 with __type 'ResourceNotFoundException' and message 'Scheduled query with identifier N not found'. Observed: HTTP 200 with an empty body.
Replay: logs-scheduled-query-workflow#task#5.
