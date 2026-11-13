Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message
Issue-Key: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message|run#30|AssertionError|assert error_message.encode() in blob, (
Issue-Commit: f3f9216a884ddb496f260c89a030775b09cc5916

# firehose__lambda__s3: the HTTP-endpoint error output record carries a generic errorMessage instead of the message the endpoint returned

> _Note: this issue was identified against the code base at commit `f3f9216a884ddb496f260c89a030775b09cc5916`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecordBatch

## Description
Cause 1 of 1: records that an HTTP endpoint destination rejected are persisted to the S3 error output location with an emulator-authored errorMessage, losing the endpoint's own error text.
AWS: recorded behavior only (the rule the check states from the service spec): when delivery to an HttpEndpoint destination fails, Firehose retries for the configured RetryOptions duration and then writes the failed records under the S3BackupMode error output prefix as JSON envelopes carrying attemptsMade, arrivalTimestamp, errorCode, errorMessage, attemptEndingTimestamp and rawData — and the errorMessage is the LAST error message the endpoint returned, copied into the error output location. A delivery refused by an endpoint that answers with a distinctive message therefore yields an object containing that message.
Emulator: the error output object is written, but its errorMessage is a fixed sentence, `the HTTP endpoint destination did not acknowledge the delivery`, and the endpoint's actual message never appears in the object. Observed envelope: `{"attemptsMade":1,"arrivalTimestamp":…,"errorCode":"HttpEndpoint.DestinationException","errorMessage":"the HTTP endpoint destination did not acknowledge the delivery","attemptEndingTimestamp":…,"rawData":"…"}`. The same envelope records attemptsMade:1 for a 60 s retry window where the check expects at least two attempts, so the retry accounting looks short too — the graded assertion here is the missing errorMessage.
Why: the error output location is the only place a client can learn WHY a batch was rejected. Replacing the endpoint's message with a constant removes the diagnosis (bad auth, schema rejection, throttling) and makes every HTTP-endpoint failure look alike; automated triage on errorMessage cannot work.
Fix: the firehose HTTP-endpoint delivery path's error-output serializer — propagate the endpoint's last response error message (and the real attempt count) into the errorMessage/attemptsMade fields of the record written under the error output prefix.
Traces: firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message.

## Evidence
Expected: the persisted error-output object contains the errorMessage the endpoint returned (here a per-run marker string the test endpoint answers with). Actual: `AssertionError: the persisted object(s) ['err/http-endpoint-failed/…'] (535 bytes) do not contain the errorMessage the endpoint returned, '<endpoint marker>'. The spec says the last instance of the error message is copied to the error output location. Object begins b'{"attemptsMade":1,"arrivalTimestamp":…,"errorCode":"HttpEndpoint.DestinationException","errorMessage":"the HTTP endpoint destination did not acknowledge the delivery","attemptEndingTimestamp":…,"rawData":"…"}'`. Object keys, timestamps and the marker string are per-run values; the requirement is that the endpoint's message, not a fixed sentence, is what lands in errorMessage.

## Steps to Reproduce
Create a delivery stream with an HttpEndpointDestinationConfiguration pointing at an endpoint that refuses deliveries with a distinctive error message, RetryOptions ~60 s and S3 backup configured with an error output prefix; PutRecord/PutRecordBatch a couple of records; after the retry window, read the objects under the error output prefix. Observe the fixed errorMessage and attemptsMade:1. Re-replay firehose__lambda__s3.firehose-http-endpoint-rejected-batch-is-retried-then-persisted-with-the-endpoint-error-message.
