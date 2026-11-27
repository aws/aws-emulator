Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__lambda__logs.function-group-subscription-rejected-while-feed-role-lacks-put-record
Issue-Key: kinesis__lambda__logs.function-group-subscription-rejected-while-feed-role-lacks-put-record|_refusal#19|AssertionError|raise AssertionError(f"expected the subscription filter to be refused; it returned {result!r}")
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# logs PutSubscriptionFilter accepts a Kinesis destination whose role may not PutRecord — the destination test delivery is never performed

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
Cause 1 of 1: a subscription filter from a Lambda function's log group to a Kinesis stream is created even though the supplied `roleArn` is not allowed `kinesis:PutRecord` on that stream.

AWS: PutSubscriptionFilter assumes `roleArn` and delivers a test record to `destinationArn` before storing the filter. When the role cannot write to the stream the call is refused with HTTP 400 `InvalidParameterException` and the message `Could not deliver test message to specified destination. Check if the destination is valid.` (entirely fixed text). After the role is granted `kinesis:PutRecord` on the stream, the same call succeeds. Smithy (logs): `PutSubscriptionFilter` models `InvalidParameterException`.

Emulator: the call returns HTTP 200 with an empty body (`content-length: '2'`) and the filter is created.

Why: the client's log feed is wired to a destination it can never write to, and nothing reports it — events are simply lost. A rejection-then-grant workflow is indistinguishable from the ungranted state, so permission errors in log pipelines surface only as missing data downstream.

Fix: the PutSubscriptionFilter handler's validation — resolve `roleArn`, evaluate `kinesis:PutRecord` on the destination stream and perform the test delivery, refusing with InvalidParameterException before the filter is stored.

Traces: kinesis__lambda__logs.function-group-subscription-rejected-while-feed-role-lacks-put-record.

## Evidence
Expected: ('InvalidParameterException', 'Could not deliver test message to specified destination. Check if the destination is valid.'), HTTP 400.
Actual: "expected the subscription filter to be refused; it returned {'ResponseMetadata': {... 'HTTPStatusCode': 200, 'content-length': '2' ...}}".

## Steps to Reproduce
1. Create a Lambda function (so its /aws/lambda/<name> log group exists), a Kinesis stream, and a logs-trusted role WITHOUT kinesis:PutRecord on the stream.
2. Call logs:PutSubscriptionFilter on the function's log group with that destinationArn and roleArn — HTTP 200 instead of InvalidParameterException.
