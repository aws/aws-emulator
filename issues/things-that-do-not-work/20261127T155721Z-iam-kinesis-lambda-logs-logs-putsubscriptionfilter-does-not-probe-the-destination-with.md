Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__lambda__logs.function-log-filter-to-stream-rejected-until-role-may-put-records
Issue-Key: iam__kinesis__lambda__logs.function-log-filter-to-stream-rejected-until-role-may-put-records|_refused#21|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs PutSubscriptionFilter does not probe the destination with the given role — a filter to a Kinesis stream is accepted while the role may not PutRecord

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: PutSubscriptionFilter accepts a Kinesis destination whose `roleArn` is not allowed `kinesis:PutRecord` on the target stream.

AWS: PutSubscriptionFilter validates the destination by assuming `roleArn` and delivering a test record. If the role cannot write to the stream the call is refused with HTTP 400 `InvalidParameterException` and the message `Could not deliver test message to specified destination. Check if the destination is valid.` — that sentence is fixed text with no variable parts. Once the role's policy grants `kinesis:PutRecord` on the stream, the identical call succeeds. Smithy (logs): `PutSubscriptionFilter` models `InvalidParameterException` and takes required `destinationArn` with optional `roleArn`.

Emulator: the call returns HTTP 200 with an empty body (`content-length: '2'`) and the filter is created, so the ungranted role is never detected.

Why: the whole point of the destination probe is to fail at configuration time rather than silently drop every delivery afterwards. A client gets a filter that looks healthy and loses every log event; negative tests that assert the refusal pass vacuously, and the later 'grant then retry' step cannot be distinguished from the ungranted state.

Fix: the PutSubscriptionFilter handler's validation — resolve `roleArn`, evaluate `kinesis:PutRecord` against the destination stream (and perform the test delivery), and refuse with InvalidParameterException and AWS's sentence before any filter is stored.

Traces: iam__kinesis__lambda__logs.function-log-filter-to-stream-rejected-until-role-may-put-records.

## Evidence
Expected: ('InvalidParameterException', 'Could not deliver test message to specified destination. Check if the destination is valid.'), HTTP 400.
Actual: HTTPStatusCode 200, empty response body. Assertion: "expected a refusal, but the call succeeded".

## Steps to Reproduce
1. Create a Kinesis stream, a log group, and an IAM role trusted by logs.amazonaws.com whose policy does NOT allow kinesis:PutRecord on the stream.
2. Call logs:PutSubscriptionFilter with that destinationArn and roleArn — HTTP 200 instead of InvalidParameterException.
3. Grant kinesis:PutRecord and repeat — indistinguishable from step 2.
