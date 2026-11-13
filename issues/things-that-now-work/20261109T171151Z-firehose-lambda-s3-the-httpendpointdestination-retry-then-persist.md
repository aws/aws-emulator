Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message
Issue-Key: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message|_poll_until#28|Timeout|time.sleep(_POLL_INTERVAL_S)
Issue-Commit: d6495890c37f2f3170333ee7f057f680b864ff4b

# firehose__lambda__s3: the HttpEndpointDestination retry-then-persist trajectory reaches no verdict — the check hangs to the 60 s harness cap

> _Note: this issue was identified against the code base at commit `d6495890c37f2f3170333ee7f057f680b864ff4b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: a DirectPut delivery stream with an HttpEndpointDestinationConfiguration never produces the observable retry-then-persist trajectory, so the check burns its whole budget and is cut off at the 60 s harness cap with no verdict. Failing check: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message.

AWS: recorded behavior only for the end-to-end timing; the behaviour the recording exercises is the documented HTTP-endpoint delivery contract (https://docs.aws.amazon.com/firehose/latest/dev/httpdeliveryrequestresponse.html). Firehose POSTs the buffered batch to EndpointConfiguration.Url with the x-amz-firehose-access-key and x-amz-firehose-request-id headers; a 200 with the {requestId,timestamp} envelope ends the delivery, while a 5XX with {requestId,timestamp,errorMessage} is RETRIABLE, so Firehose re-POSTs the SAME batch, with backoff starting at ~1 s, for up to RetryOptions.DurationInSeconds; when that window is exhausted and S3BackupMode is FailedDataOnly, the batch is persisted under the S3Configuration ErrorOutputPrefix with !{firehose:error-output-type} evaluated to http-endpoint-failed, and the endpoint's last errorMessage is copied into that error object alongside the recoverable record. An accepted (200) delivery on an otherwise identical stream persists nothing there.

Emulator: the check never reaches any of those observations — the replay ends with "Check timed out after 60.0s", i.e. no step of the trajectory (control-leg capture at the endpoint, a second attempt for the refused batch, or any object under err/ or backup/) completed in time for a verdict to be formed. Nothing about the HTTP-endpoint delivery path is observable.

Why: a client that configures an HTTP endpoint destination relies on Firehose actually POSTing to the endpoint, re-POSTing retriable refusals within the retry window and then persisting the exhausted batch with its error. In the emulator the records are accepted by PutRecordBatch (so the producer sees success) and then nothing observable happens: data is silently lost, no retry can be measured, and no error output object is produced, so failure handling built on this destination cannot be exercised at all.

Related, not duplicate: open issue 20261108T031654Z-firehose-lambda-s3-the-http-endpoint-error-output.md reports the error output record carrying a generic errorMessage instead of the endpoint's — that is about the CONTENT of an error object; the replay report does not mark this check as owned by it, and here no verdict is reached at all.

Fix: the Firehose delivery engine's HttpEndpointDestination path (the destination's POST/response-envelope handling, its RetryOptions backoff loop, and the S3Configuration error-output/backup writer that evaluates !{firehose:error-output-type} to http-endpoint-failed), plus whatever in that path blocks long enough that nothing observable happens within the harness cap.

Traces: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message.

## Evidence
Check firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message, [mismatch]:

Error: Check timed out after 60.0s

Expected (per the recording and the HTTP-delivery spec the check cites):
  - control leg (good x-amz-firehose-access-key): the put records are captured at the endpoint with accepted=True;
  - treated leg (key the endpoint answers 500 for): at least 2 attempts carrying exactly the same records, with the attempt timestamps at least ~1 s apart (documented backoff) inside RetryOptions.DurationInSeconds=60;
  - then at least one persisted object under err/ or backup/, with a key starting 'err/http-endpoint-failed/', containing the endpoint's errorMessage and the refused record (raw or base64);
  - nothing persisted for the accepted control leg.

Actual (emulator): none of the above was observed — the check produced no verdict and was cut off at the 60 s cap. No endpoint capture, no retry, and no error-output object was reached.

Status/error codes: PutRecordBatch itself is accepted (FailedPutCount 0 is what AWS also returns); the divergence is entirely in the delivery path after the put.

## Steps to Reproduce
1. Create an S3 bucket, a Lambda function with a function URL (AuthType NONE) that answers 200 {requestId,timestamp} for one x-amz-firehose-access-key and 500 {requestId,timestamp,errorMessage} for another, and an IAM role trusted by firehose.amazonaws.com with write access to the bucket.
2. Create two DirectPut delivery streams with HttpEndpointDestinationConfiguration pointing at that function URL — identical except for EndpointConfiguration.AccessKey (one good, one the endpoint refuses) — with RetryOptions.DurationInSeconds=60, S3BackupMode=FailedDataOnly and S3Configuration Prefix='backup/', ErrorOutputPrefix='err/!{firehose:error-output-type}/'. Wait for both ACTIVE.
3. PutRecordBatch the same 2 records to each stream.
4. Poll the bucket: no capture object appears for the control leg, the refused leg is never re-attempted, and no object ever appears under err/ or backup/. The check reaches no verdict and is cut off at the 60 s harness cap.
