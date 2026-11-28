Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: opensearchserverless.cancel-completed-ingestion-job-conflict-rejected
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: opensearchserverless|CreateIngestionJob|NotImplemented
Issue-Commit: 429bdfecbf56545571d755c8b6ad852b4e3ed1bc

# opensearchserverless: 2 net-new causes — CreateIngestionJob is unimplemented (501), so no ingestion job can be started, and the COMPLETED-job cancel refusal (ConflictException) is unreachable

> _Note: this issue was identified against the code base at commit `429bdfecbf56545571d755c8b6ad852b4e3ed1bc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateIngestionJob

## Description
The OpenSearch Serverless (aoss) ingestion-job plane is still missing its write entry point. The already-open issue 20261127T161519Z-aoss-ingestion-job-plane-unimplemented-listingestionjobs-cancelingestionjob-and.md covers ListIngestionJobs, GetIngestionJob and CancelIngestionJob-on-an-unknown-id only; it does NOT cover CreateIngestionJob, and it does not cover the state-machine refusal CancelIngestionJob owes a job in a terminal status. Neither operation appears in the checked-out Smithy model (services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json lists 46 bound operations, none of them ingestion-job related), so both causes below are 'recorded behavior only': the requirement is what the behavioral check records the real service doing in the com.amazon.aoss namespace. 2 causes.

Cause 1 of 2 — CreateIngestionJob is not served at all.
AWS: recorded behavior only (operation absent from the on-disk model). CreateIngestionJob(collectionId, ingestionJobName, indexName, source={'s3': {bucket, bucketOwnerAccountId, keyPrefix}}, tags) is accepted on an ACTIVE collection by a caller whose identity policy grants aoss:CreateIngestionJob and aoss:APIAccessAll on the collection and whose data access policy grants the index write permissions. The response is HTTP 200 with an `ingestionJobDetail` object carrying `ingestionJobId` (a generated resource id), `ingestionJobName` echoed verbatim from the request, and a `status` that starts at CREATING and advances through ACTIVE to COMPLETED; once terminal, `statistics.s3.objectsIngested` counts the objects read under the key prefix and `statistics.s3.objectsFailed` is 0. A permission not yet in effect is refused with error code AccessDeniedException, not with a server error.
Emulator: HTTP 501, body {"__type": "NotImplemented", "message": "operation CreateIngestionJob is not implemented in this emulator"} — botocore raises ClientError (NotImplemented) when calling the CreateIngestionJob operation: operation CreateIngestionJob is not implemented in this emulator. The fixed text of the emulator's message is 'operation ' / ' is not implemented in this emulator'; the variable part is the operation name.
Why: a client cannot start an ingestion job at all, so the whole ingestion-job plane is unreachable from the API — nothing can be listed, polled, cancelled or completed, because no job can ever come into existence. Code paths that create a job and wait for COMPLETED see an unmodeled 501 instead of a job id, and a 501 is not retry-classified like AccessDeniedException, so even the 'permission not in effect yet' retry loop a real caller needs collapses immediately.
Fix: a CreateIngestionJob handler and its registration in the aoss operation dispatch, backed by the same ingestion-job state the open List/Get/Cancel issue needs: validate the input (collection must exist and be ACTIVE, the index and the s3 source must be well formed), persist a job with a generated ingestionJobId and the requested name/tags, and return 200 with ingestionJobDetail and an initial CREATING status that progresses to ACTIVE then COMPLETED with s3 statistics.
Traces: opensearchserverless.cancel-completed-ingestion-job-conflict-rejected.

Cause 2 of 2 — CancelIngestionJob owes ConflictException for a job that is no longer cancellable.
AWS: recorded behavior only. CancelIngestionJob is accepted only while the job is in ACTIVE, CREATE_FAILED or CANCEL_FAILED status; an accepted cancel answers 200 with ingestionJobId, collectionId and a status of CANCELLING or CANCELLED, and the job later settles at CANCELLED. Called on a job in any other status (notably COMPLETED) it is refused with error code ConflictException and the message 'Ingestion job cannot be cancelled because it is not in ACTIVE, CREATE_FAILED, or CANCEL_FAILED status.' — that message is entirely fixed text, with no part taken from the request or from state. The refused cancel is a no-op: a subsequent GetIngestionJob still reads status COMPLETED with unchanged statistics, and ListIngestionJobs still holds the job exactly once as COMPLETED.
Emulator: unobservable because Cause 1 refuses the create — the first CreateIngestionJob call returns 501 NotImplemented, so the check never reaches the cancel. The emulator's CancelIngestionJob handler is in any case still the 501 NotImplemented stub (the open List/Get/Cancel issue records that), and that issue specifies only the unknown-id ResourceNotFoundException path, so nothing yet carries this status-machine refusal.
Why: a client that treats ConflictException as 'the job already finished, nothing to cancel' cannot distinguish that from a broken service, and a cleanup path that cancels-then-polls either loops forever or aborts. Conversely, if a future implementation accepted the cancel of a COMPLETED job, it would silently mutate a terminal job's status and statistics, destroying the record of an ingestion that in fact succeeded.
Fix: the CancelIngestionJob handler's validation/state-transition step — after the existence lookup, gate on the job's current status and raise ConflictException with the fixed message above for anything outside {ACTIVE, CREATE_FAILED, CANCEL_FAILED}, leaving the stored job's status and statistics untouched; for a cancellable job, transition to CANCELLING/CANCELLED and return ingestionJobId, collectionId and status.
Traces: opensearchserverless.cancel-completed-ingestion-job-conflict-rejected.

## Evidence
Behavioral suite simparity-behavioral-aws-v1: 113/120 passed. Failing check opensearchserverless.cancel-completed-ingestion-job-conflict-rejected [error].

Cause 1 — the check builds an encryption security policy, a NEXTGEN collection group, an ACTIVE SEARCH collection, an S3 source bucket with an NDJSON object under 'catalog/', an IAM caller role granting aoss:CreateIngestionJob/GetIngestionJob/ListIngestionJobs/CancelIngestionJob + aoss:APIAccessAll + s3:GetObject/ListBucket, and a data access policy for the index; all of that succeeds. The first ingestion call then fails:
  expected: 200 with ingestionJobDetail{ingestionJobId: <generated id>, ingestionJobName: <echoed request name>, status: CREATING}
  actual:   botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateIngestionJob operation: operation CreateIngestionJob is not implemented in this emulator
  (raised at client.py line 1094, _make_api_call -> raise error_class(parsed_response, operation_name))

Cause 2 — the assertions the check would make next, blocked by Cause 1:
  after polling GetIngestionJob to a terminal status, status == 'COMPLETED' and statistics.s3 == {objectsIngested: 1, objectsFailed: 0};
  CancelIngestionJob on that COMPLETED job: expected error code 'ConflictException' with message exactly 'Ingestion job cannot be cancelled because it is not in ACTIVE, CREATE_FAILED, or CANCEL_FAILED status.';
  GetIngestionJob after the refused cancel: still 'COMPLETED' with identical statistics; ListIngestionJobs: the job present exactly once with status 'COMPLETED';
  positive control: CancelIngestionJob on a second, still-ACTIVE job: 200 with status in {CANCELLING, CANCELLED}, settling at 'CANCELLED'.
  actual: none of these is reached — the emulator refuses the create.

Model check: smithy_query list_operations on services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json returns 46 operations, none named CreateIngestionJob, GetIngestionJob, ListIngestionJobs or CancelIngestionJob — hence 'recorded behavior only' for both causes, with status, error code and message taken from the recorded behavior the check asserts.

Related but not covering: open issue 20261127T161519Z-aoss-ingestion-job-plane-unimplemented-listingestionjobs-cancelingestionjob-and.md (ListIngestionJobs 200/empty page, Get/Cancel ResourceNotFoundException for an unknown job id). It names neither CreateIngestionJob nor the COMPLETED-job ConflictException.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the behavioral suite simparity-behavioral-aws-v1.
2. Run check opensearchserverless.cancel-completed-ingestion-job-conflict-rejected. It provisions: encryption security policy, collection group (generation NEXTGEN, standbyReplicas ENABLED), SEARCH collection in that group polled to ACTIVE, S3 bucket with 'catalog/items.ndjson' plus ten 'bulk/part-NN.ndjson' objects, an IAM role with the aoss ingestion-job + APIAccessAll + s3 read permissions, and a data access policy for index 'products*'.
3. As that assumed role, call CreateIngestionJob(collectionId=<collection id>, ingestionJobName=<name>, indexName='products', source={'s3': {'bucket': 'arn:aws:s3:::<bucket>', 'bucketOwnerAccountId': <account>, 'keyPrefix': 'catalog/'}}, tags=[{'key':'sim2real','value':<check>}]).
   -> emulator answers 501 {"__type":"NotImplemented","message":"operation CreateIngestionJob is not implemented in this emulator"} instead of 200 with ingestionJobDetail (Cause 1).
4. With Cause 1 fixed, poll GetIngestionJob until the job is COMPLETED with statistics.s3.objectsIngested == 1 / objectsFailed == 0, then call CancelIngestionJob(collectionId=<collection id>, ingestionJobId=<that job>).
   -> expect error code ConflictException, message 'Ingestion job cannot be cancelled because it is not in ACTIVE, CREATE_FAILED, or CANCEL_FAILED status.', and the job unchanged on a following GetIngestionJob/ListIngestionJobs (Cause 2).
5. Positive control: create a second job over the 'bulk/' prefix, wait for it to leave CREATING into ACTIVE, and cancel it — expect 200 with status CANCELLING or CANCELLED, settling at CANCELLED.
