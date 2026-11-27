Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns__sqs.fifo-topic-lambda-subscription-rejected
Issue-Key: lambda__sns__sqs.fifo-topic-lambda-subscription-rejected|_refusal#5|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# SNS Subscribe accepts Protocol=lambda on a FIFO topic

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
One cause.

Cause 1 of 1: subscribing a Lambda function to a FIFO topic is accepted instead of refused.

AWS: a FIFO topic supports only the `sqs` delivery protocol. Subscribe with `Protocol: lambda` against a FIFO topic is refused with `InvalidParameter` (Smithy: com.amazonaws.sns Subscribe error InvalidParameterException, awsQueryError code `InvalidParameter`, HTTP 400) and the message is the `Invalid parameter: <parameter>: <reason>` form naming the unsupported protocol for a FIFO topic; no subscription is created.

Emulator: the call succeeds with HTTP 200 and returns a SubscriptionArn on the `.fifo` topic (`arn:aws:sns:<region>:<account>:<topic>.fifo:<uuid>`).

Why: the client believes its function is subscribed to the FIFO topic. Because AWS never creates such a subscription, code written against the emulator is configured in a way real SNS refuses, and the mistake surfaces only at deploy time against AWS — or, worse, the subscription exists in the emulator and silently changes the ordering guarantees the client reasoned about.

Fix: the SNS Subscribe handler's protocol validation for FIFO topics.

Traces: lambda__sns__sqs.fifo-topic-lambda-subscription-rejected.

## Evidence
Expected: Subscribe refused with InvalidParameter (HTTP 400) and no subscription created.
Actual: `AssertionError: the call was expected to be refused but returned {'SubscriptionArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-fifolam-1fba5a00f64c.fifo:0a471604-1451-46a9-a3ef-69de54d80861', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The topic name and subscription uuid are per-run values; the requirement is the refusal, not the ARN.

## Steps to Reproduce
1. Create a FIFO topic (`FifoTopic=true`, name ending `.fifo`).
2. Create a Lambda function and call Subscribe with TopicArn=<fifo topic>, Protocol='lambda', Endpoint=<function arn>.
3. The call returns 200 with a SubscriptionArn instead of being refused.
Replay: lambda__sns__sqs.fifo-topic-lambda-subscription-rejected.
