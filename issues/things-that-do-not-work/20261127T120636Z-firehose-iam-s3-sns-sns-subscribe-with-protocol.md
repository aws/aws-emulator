Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__iam__s3__sns.fifo-topic-firehose-subscription-rejected
Issue-Key: firehose__iam__s3__sns.fifo-topic-firehose-subscription-rejected|_refusal#23|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {got!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose__iam__s3__sns: SNS Subscribe with Protocol 'firehose' on a FIFO topic is accepted and creates a subscription, instead of being refused with 400 InvalidParameter

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Cause 1 of 1: SNS Subscribe does not validate the requested Protocol against the topic's FIFO-ness. A 'firehose' subscription on a FIFO topic is created and returned instead of refused, and the subscription is then listed on the FIFO topic.

AWS: Smithy: Subscribe takes required TopicArn and Protocol and models InvalidParameterException with awsQueryError code 'InvalidParameter' and HTTP 400. A FIFO topic supports only the sqs protocol; every other protocol — including firehose — is refused. The recording shows HTTP 400 with Error.Code 'InvalidParameter' and Error.Message "Invalid parameter: Invalid protocol type: firehose". The whole message is fixed text for this error class except the trailing protocol token, which is the Protocol value taken verbatim from the request. Because the call is refused, no subscription exists: ListSubscriptionsByTopic on the FIFO topic returns an empty list. The SAME stream, role and attributes subscribed to a STANDARD topic are accepted and yield a confirmed subscription (Protocol 'firehose', PendingConfirmation 'false', Endpoint equal to the delivery-stream ARN), so the refusal is specific to the topic being FIFO and is not an authorization or endpoint problem.

Emulator: the call returns HTTP 200 with a SubscriptionArn shaped '<fifo topic arn>:<uuid>' (the check reports "expected a refusal, but the call succeeded: 'arn:aws:sns:...:<name>.fifo:<uuid>'"), and the subscription is subsequently listed against the FIFO topic. No 400 and no error code or message are produced; the protocol/FIFO compatibility rule is simply absent from the Subscribe handler.

Why: a client relying on AWS's refusal silently ends up with a configuration AWS would never create — a Firehose subscription on a FIFO topic. Publishes to that topic will never reach the delivery stream in AWS, so the emulator reports a working fan-out that cannot exist; negative-path tests and guard rails that assert this InvalidParameter branch pass against the emulator and fail in AWS, and the modelled InvalidParameterException branch of Subscribe stays unreachable for this class.

Fix: the Subscribe handler's input validation in the sns service crate — before any subscription is persisted, resolve the topic from TopicArn, and when its FifoTopic attribute is true refuse every Protocol other than 'sqs' with HTTP 400, Error.Code 'InvalidParameter' and the message frame "Invalid parameter: Invalid protocol type: <protocol>" (the protocol echoed from the request). The standard-topic path must stay accepted, and nothing may be written to subscription state on the refused call.

Traces: firehose__iam__s3__sns.fifo-topic-firehose-subscription-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

[mismatch] firehose__iam__s3__sns.fifo-topic-firehose-subscription-rejected
  File "check_firehose__iam__s3__sns_fifo_topic_firehose_subscription_rejected.py", line 219, in run
    got = _refusal(lambda: subscribe(fifo))
  File "check_firehose__iam__s3__sns_fifo_topic_firehose_subscription_rejected.py", line 139, in _refusal
    raise AssertionError(f"expected a refusal, but the call succeeded: {got!r}")
  AssertionError: expected a refusal, but the call succeeded: 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-fhfifo-<id>.fifo:<uuid>'

The check asserts, at line 219-220:
  assert got == ("InvalidParameter", "Invalid parameter: Invalid protocol type: firehose")
and then that _firehose_endpoints(sns, fifo) == [].

Expected (AWS): Subscribe(TopicArn=<...>.fifo, Protocol='firehose', Endpoint=<delivery stream arn>, Attributes={'SubscriptionRoleArn': <role>}, ReturnSubscriptionArn=True) -> HTTP 400, Error.Code 'InvalidParameter', Error.Message 'Invalid parameter: Invalid protocol type: firehose'; ListSubscriptionsByTopic(<fifo topic>) lists no subscription.
Actual (emulator): HTTP 200 with SubscriptionArn '<fifo topic arn>:<uuid>', and the subscription listed on the FIFO topic.

The account id, region, generated topic/stream/role names and the subscription uuid are per-run values; the requirement is the 400, the code 'InvalidParameter' and the message frame whose only variable part is the request's Protocol value.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket and a firehose.amazonaws.com-trusted role with the S3 write set, then CreateDeliveryStream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={...}) and wait for ACTIVE; record its DeliveryStreamARN.
3. Create an sns.amazonaws.com-trusted role allowed firehose:PutRecord/PutRecordBatch/DescribeDeliveryStream on that stream ARN.
4. CreateTopic(Name=<plain>) and CreateTopic(Name=<name>.fifo, Attributes={'FifoTopic': 'true'}).
5. Subscribe(TopicArn=<standard topic>, Protocol='firehose', Endpoint=<stream arn>, Attributes={'SubscriptionRoleArn': <role>}, ReturnSubscriptionArn=True): this must succeed (positive control) and GetSubscriptionAttributes must show Protocol 'firehose', PendingConfirmation 'false'.
6. Subscribe with the same arguments but TopicArn=<fifo topic>. On AWS this is refused with HTTP 400, 'InvalidParameter', 'Invalid parameter: Invalid protocol type: firehose'. Observe a 200 and a SubscriptionArn instead.
7. ListSubscriptionsByTopic(<fifo topic>): on AWS empty; observe the subscription present.
8. Re-replay the check in trace_ids to confirm a fix.
