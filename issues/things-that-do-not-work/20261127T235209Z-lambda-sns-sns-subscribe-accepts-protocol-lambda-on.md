Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sns.lambda-subscription-to-a-fifo-topic-rejected
Issue-Key: lambda__sns.lambda-subscription-to-a-fifo-topic-rejected|_refusal#18|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Commit: 255279eeed7e5207bd6ac2895e89724e049cf5ef

# lambda__sns: SNS Subscribe accepts Protocol=lambda on a FIFO topic and mints a subscription, instead of refusing with InvalidParameter

> _Note: this issue was identified against the code base at commit `255279eeed7e5207bd6ac2895e89724e049cf5ef`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Cause 1 of 1: Subscribe with `Protocol=lambda` against a FIFO topic is accepted.
  AWS: Smithy: Subscribe takes required `TopicArn` and `Protocol` and models InvalidParameterException (awsQueryError code `InvalidParameter`, HTTP 400). A FIFO topic (`FifoTopic=true`, name ending `.fifo`) supports only the sqs protocol as a subscriber; a Lambda function endpoint is refused with HTTP 400, code `InvalidParameter`, message `Invalid parameter: Invalid protocol type: lambda` — fixed text "Invalid parameter: Invalid protocol type: " followed by the request's Protocol value. Nothing is created: ListSubscriptionsByTopic for the FIFO topic lists no subscription. The identical function endpoint on a standard topic is accepted and reads back with PendingConfirmation=false.
  Emulator: the call returns HTTP 200 with a minted SubscriptionArn on the FIFO topic (`{'SubscriptionArn': 'arn:aws:sns:<region>:<account>:<topic>.fifo:<uuid>'}`), so no refusal occurs and a subscription that cannot exist in AWS is stored.
  Why: a client wiring a Lambda consumer to the wrong (FIFO) topic gets a success and a subscription ARN, then silently receives no invocations; the AWS-side guardrail that catches this misconfiguration at Subscribe time is absent, and error-handling paths keyed on InvalidParameter are never taken.
  Fix: Subscribe's validation — reject every non-sqs protocol (lambda included) when the target topic has FifoTopic=true, before any subscription state is created. Related open issue 20261127T143715Z-lambda-sns-sqs-sns-subscribe-accepts-protocol-lambda-on-a-fifo.md reports the same acceptance under the lambda__sns__sqs suite; this check is listed under `### lambda__sns` and is not marked owned by it.
  Traces: lambda__sns.lambda-subscription-to-a-fifo-topic-rejected.

## Evidence
Expected: Subscribe(TopicArn=<...>.fifo, Protocol='lambda', Endpoint=<function arn>) refused with ('InvalidParameter', 'Invalid parameter: Invalid protocol type: lambda') and ListSubscriptionsByTopic on the FIFO topic returning []. Actual: HTTP 200 — `AssertionError: the call was expected to be refused but returned {'SubscriptionArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-fifo-<suffix>.fifo:<uuid>', ... 'HTTPStatusCode': 200 ...}`. The positive control (same endpoint on a standard topic) passed.

## Steps to Reproduce
1. Create an IAM role trusting lambda.amazonaws.com and a Lambda function; take its FunctionArn.
2. CreateTopic Name=<n> (standard) and CreateTopic Name=<n>.fifo with Attributes={"FifoTopic":"true","ContentBasedDeduplication":"true"}.
3. sns.Subscribe(TopicArn=<standard>, Protocol='lambda', Endpoint=<FunctionArn>) — succeeds (control).
4. sns.Subscribe(TopicArn=<fifo>, Protocol='lambda', Endpoint=<FunctionArn>, ReturnSubscriptionArn=true) — expect 400 InvalidParameter 'Invalid parameter: Invalid protocol type: lambda'; observe 200 with a SubscriptionArn.
5. ListSubscriptionsByTopic(TopicArn=<fifo>) — expect [], observe the stored subscription.
