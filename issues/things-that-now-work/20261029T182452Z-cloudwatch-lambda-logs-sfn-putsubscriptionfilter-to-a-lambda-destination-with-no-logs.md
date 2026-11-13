Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected
Issue-Key: cloudwatch__lambda__logs__stepfunctions.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected|_error#11|AssertionError|raise AssertionError(f"the call was expected to be refused but SUCCEEDED: {resp!r}")
Issue-Commit: ec258a82c643322f3f1b10a7ea424abc4c176038

# PutSubscriptionFilter to a Lambda destination with no logs.amazonaws.com invoke permission is accepted instead of refused

> _Note: this issue was identified against the code base at commit `ec258a82c643322f3f1b10a7ea424abc4c176038`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutSubscriptionFilter

## Description
This issue carries 1 net-new root cause.

Cause 1 of 1: the subscription-filter create does not verify that the destination Lambda grants invoke permission to the CloudWatch Logs service principal.
AWS: PutSubscriptionFilter validates the destination at CREATE time. If the destination function has no resource policy statement allowing `lambda:InvokeFunction` to principal `logs.amazonaws.com` (optionally scoped by SourceArn to the log group), the call is REFUSED with a client error and no filter is stored — the failure is synchronous, not a silent non-delivery later.
Emulator: the call returns HTTP 200 with an empty body (content-type application/x-amz-json-1.1, content-length 2), so the filter is created against a function Logs can never invoke.
Why: a Step Functions express-workflow log pipeline appears wired in the emulator and then delivers nothing in the real account — and because the refusal is what tells the operator a permission is missing, the misconfiguration is discovered only by the absence of data. (The open issue 20261029T174654Z-cloudwatch-ecr-ecs-logs-cloudwatch-logs-delivers-subscription-filter-events-to-kinesis.md records the same permission-blindness for a Kinesis destination; this Lambda-destination replay key is not on it.)
Fix: PutSubscriptionFilter's destination authorization check in the CloudWatch Logs handler (resolve the destination's resource policy before storing the filter).
Traces: cloudwatch__lambda__logs__sfn.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected.

## Evidence
cloudwatch__lambda__logs__sfn.obs-sfn-3-express-fail-events-to-unpermitted-lambda-subscriber-rejected:
  AssertionError: the call was expected to be refused but SUCCEEDED: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}, 'RetryAttempts': 0}}

## Steps to Reproduce
Replay the check id: create a log group and a Lambda function WITHOUT an AddPermission statement for principal logs.amazonaws.com, then call PutSubscriptionFilter pointing the log group at that function; expect a client-error refusal and no filter stored.
