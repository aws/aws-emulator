Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kinesis__kms__logs.subscription-filter-to-cmk-stream-without-role-key-grant-rejected
Issue-Key: iam__kinesis__kms__logs.subscription-filter-to-cmk-stream-without-role-key-grant-rejected|_refusal#31|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# iam__kinesis__kms__logs: PutSubscriptionFilter to a CMK-encrypted stream is accepted although the role holds no kms:GenerateDataKey

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: the subscription filter's destination is not test-delivered through the role's KMS permissions, so a role that cannot encrypt into the stream is accepted.
  AWS: recorded behavior only. PutSubscriptionFilter test-delivers to the destination with the supplied roleArn; when that role may kinesis:PutRecord but holds no kms:GenerateDataKey on the stream's customer managed key, the call is refused 400 InvalidParameterException with the message 'Could not deliver test message to specified Kinesis stream. User <role id>:<session> is not authorized to encrypt records in stream <stream>' — fixed text with the role id, session name and stream taken from the request and the caller's session — and no filter is created.
  Emulator: the call returns 200 with an empty body ({'ResponseMetadata': {'HTTPStatusCode': 200, ...}}), so the filter is created.
  Why: a client is told its log subscription is wired up when the role cannot encrypt into the destination; log events then silently never arrive, instead of the misconfiguration being refused at put time.
  Fix: the logs PutSubscriptionFilter handler's destination test-delivery — authorize the supplied role against the destination stream's CMK (kms:GenerateDataKey) and refuse with the delivery-failure message.
  Traces: iam__kinesis__kms__logs.subscription-filter-to-cmk-stream-without-role-key-grant-rejected.

## Evidence
iam__kinesis__kms__logs.subscription-filter-to-cmk-stream-without-role-key-grant-rejected —
AssertionError: expected the call to be refused, but it succeeded: {'ResponseMetadata': {'RequestId': ..., 'HTTPStatusCode': 200, ..., 'content-length': '2'}}.
Expected ('InvalidParameterException', 'Could not deliver test message to specified Kinesis stream. User <role-id>:<session> is not authorized to encrypt records in stream <stream arn>').

## Steps to Reproduce
1. Create a symmetric CMK and a Kinesis stream with SSE-KMS under it.
2. Create a role trusted by logs.<region>.amazonaws.com allowed kinesis:PutRecord on the stream but no kms: action on the key.
3. logs.put_subscription_filter(logGroupName=..., filterName=..., filterPattern='', destinationArn=<stream arn>, roleArn=<that role>).
4. Observe 200; AWS refuses with InvalidParameterException 'Could not deliver test message to specified Kinesis stream. User ... is not authorized to encrypt records in stream ...'.
