Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-cancel-ingestion-job-not-found
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-cancel-ingestion-job-not-found opensearchserverless-cancel-ingestion-job-validation-errors
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|ListIngestionJobs|2xx->5xx|$.ingestionJobSummaries
Issue-Key: aoss|ListIngestionJobs|2xx->5xx|status
Issue-Key: aoss|CancelIngestionJob|4xx->5xx|$.__type
Issue-Key: aoss|CancelIngestionJob|4xx->5xx|$.message
Issue-Key: aoss|CancelIngestionJob|4xx->5xx|status
Issue-Key: aoss|GetIngestionJob|4xx->5xx|$.__type
Issue-Key: aoss|GetIngestionJob|4xx->5xx|$.message
Issue-Key: aoss|GetIngestionJob|4xx->5xx|status
Issue-Commit: c8784648d724ab7f7f79a4b02e58fed3e3e81b74

# aoss ingestion-job plane unimplemented: ListIngestionJobs, CancelIngestionJob and GetIngestionJob answer 501 NotImplemented

> _Note: this issue was identified against the code base at commit `c8784648d724ab7f7f79a4b02e58fed3e3e81b74`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListIngestionJobs

## Description
The OpenSearch Serverless (aoss) ingestion-job plane is not served at all: every one of the three ingestion-job operations the corpus exercises is answered with HTTP 501, __type 'NotImplemented' and the emulator-authored message 'operation <Name> is not implemented in this emulator'. These operations are absent from the checked-out Smithy model (services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/..., 46 bound operations, none of them ingestion-job related), so the requirement here is recorded behavior only; the recordings show the real service answering them in the com.amazon.aoss namespace. 3 causes.

Cause 1 of 3 — ListIngestionJobs returns no listing.
AWS: recorded behavior only (operation not in the on-disk model). The recorded response is HTTP 200 with a body carrying the list member `ingestionJobSummaries`, present and empty when the account has no ingestion jobs — i.e. the empty list is SERIALIZED, not omitted.
Emulator: HTTP 501, body {"__type": "NotImplemented", "message": "operation ListIngestionJobs is not implemented in this emulator"}; `$.ingestionJobSummaries` is absent entirely (expected [], got None).
Why: a client enumerating ingestion jobs (or polling for the absence of one) gets a transport-level 501 instead of an empty page, so SDK paginators raise an unmodeled error rather than yielding zero results, and any 'no jobs running' precondition can never be established.
Fix: the ListIngestionJobs handler and its registration in the aoss operation dispatch — the operation needs a real handler backed by ingestion-job state, returning 200 with `ingestionJobSummaries` always present (empty list when there are none).
Traces: opensearchserverless-cancel-ingestion-job-not-found#task#5.

Cause 2 of 3 — CancelIngestionJob does not refuse an unknown job id.
AWS: recorded behavior only. The recorded response for a job id that does not exist is HTTP 400 with __type 'com.amazon.aoss#ResourceNotFoundException' and message 'Ingestion Job ID <jobId> is not found'. Fixed text: 'Ingestion Job ID ' and ' is not found'; the single variable part is the ingestion job id taken verbatim from the request (the recording's 'nojobwiththisidexists0' is one run's value filling that slot, not part of the template). Note the status is 400, consistent with the aoss front door answering client errors with 400 rather than the modeled httpError.
Emulator: HTTP 501, __type 'NotImplemented', message 'operation CancelIngestionJob is not implemented in this emulator'.
Why: a client cancelling a job cannot distinguish 'no such job' from a broken service: the modeled ResourceNotFoundException never arrives, so retry/ignore logic keyed on that error code takes the unmodeled-error branch, and the same 501 also hides the request-validation refusals the validation-errors fixture expects for malformed input.
Fix: the CancelIngestionJob handler — input validation first, then an ingestion-job state lookup raising ResourceNotFoundException with the message above when the id is unknown; plus its registration in the aoss operation dispatch.
Traces: opensearchserverless-cancel-ingestion-job-not-found#task#6, opensearchserverless-cancel-ingestion-job-validation-errors#task#1, #task#2, #task#3.

Cause 3 of 3 — GetIngestionJob does not refuse an unknown job id.
AWS: recorded behavior only. The recorded response for an unknown job id is HTTP 400, __type 'com.amazon.aoss#ResourceNotFoundException', message 'Ingestion Job ID <jobId> is not found' — same fixed text as Cause 2, with the job id taken from the request.
Emulator: HTTP 501, __type 'NotImplemented', message 'operation GetIngestionJob is not implemented in this emulator'.
Why: a client polling an ingestion job for completion gets 501 on every read, so neither the job's state nor its non-existence can ever be observed; code that treats ResourceNotFoundException as 'job already reaped' never terminates.
Fix: the GetIngestionJob handler and its dispatch registration, reading the same ingestion-job state Causes 1 and 2 need and raising ResourceNotFoundException for an unknown id.
Traces: opensearchserverless-cancel-ingestion-job-not-found#task#7.

No open issue covers these: the only open aoss issue (20261122T183952Z-aoss-opensearchserverless-access-policy-created-with-a-clienttoken-cannot.md) concerns GetAccessPolicy/DeleteAccessPolicy after an idempotent clientToken create, and the resolved opensearchserverless '...-stub' issues cover other operations, none of them on the ingestion-job plane.

## Evidence
Replay of simparity-wire-aws-v1 (164 steps, 154 passed, 8 mismatched, score 93.9%).

Cause 1 — fixture opensearchserverless-cancel-ingestion-job-not-found, step task#5, ListIngestionJobs:
  expected: 200, body contains $.ingestionJobSummaries = []
  actual:   501, $.ingestionJobSummaries missing (None)
  detail: "status: expected 200, got 501 / $.ingestionJobSummaries: missing (expected [], got None)"

Cause 2 — fixture opensearchserverless-cancel-ingestion-job-not-found, step task#6, CancelIngestionJob (cluster also covers opensearchserverless-cancel-ingestion-job-validation-errors task#1, task#2, task#3):
  expected: 400, $.__type = 'com.amazon.aoss#ResourceNotFoundException', $.message = 'Ingestion Job ID <jobId> is not found'
  actual:   501, $.__type = 'NotImplemented', $.message = 'operation CancelIngestionJob is not implemented in this emulator'

Cause 3 — fixture opensearchserverless-cancel-ingestion-job-not-found, step task#7, GetIngestionJob:
  expected: 400, $.__type = 'com.amazon.aoss#ResourceNotFoundException', $.message = 'Ingestion Job ID <jobId> is not found'
  actual:   501, $.__type = 'NotImplemented', $.message = 'operation GetIngestionJob is not implemented in this emulator'

Model check: smithy_query list_operations on services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json returns 46 operations and none named ListIngestionJobs, GetIngestionJob or CancelIngestionJob — hence 'recorded behavior only' for all three, with status/error code/message taken from the recorded responses.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus simparity-wire-aws-v1.
2. Replay fixture opensearchserverless-cancel-ingestion-job-not-found:
   - step task#5 calls ListIngestionJobs -> emulator answers 501 NotImplemented instead of 200 with an empty ingestionJobSummaries list.
   - step task#6 calls CancelIngestionJob with an ingestion job id that was never created -> emulator answers 501 NotImplemented instead of 400 com.amazon.aoss#ResourceNotFoundException 'Ingestion Job ID <jobId> is not found'.
   - step task#7 calls GetIngestionJob with that same unknown id -> emulator answers 501 NotImplemented instead of the same 400 ResourceNotFoundException.
3. Replay fixture opensearchserverless-cancel-ingestion-job-validation-errors steps task#1..task#3 (malformed CancelIngestionJob input): every one is answered 501 NotImplemented instead of the recorded 400 client-error refusal.
