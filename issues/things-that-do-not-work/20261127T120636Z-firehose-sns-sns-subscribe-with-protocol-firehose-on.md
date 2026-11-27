Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__sns.firehose-subscribe-on-a-fifo-topic-rejected
Issue-Key: firehose__sns.firehose-subscribe-on-a-fifo-topic-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose__sns: SNS Subscribe with Protocol 'firehose' on a FIFO topic returns a SubscriptionArn instead of refusing with 400 InvalidParameter

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Subscribe

## Description
Cause 1 of 1: the Subscribe handler applies no protocol/FIFO compatibility rule, so a Firehose delivery-stream subscription on a FIFO topic is accepted.

AWS: Smithy: Subscribe takes required TopicArn and Protocol and models InvalidParameterException with awsQueryError code 'InvalidParameter' and HTTP 400. A FIFO topic accepts only the sqs protocol. The recording shows the firehose attempt refused with HTTP 400, Error.Code 'InvalidParameter' and Error.Message "Invalid parameter: Invalid protocol type: firehose" — fixed text "Invalid parameter: Invalid protocol type: " plus the Protocol value echoed from the request. Nothing is persisted: ListSubscriptionsByTopic on the FIFO topic stays empty. The identical Subscribe against a STANDARD topic with the same stream, role and attributes is accepted and reports Protocol 'firehose', PendingConfirmation 'false' and Endpoint equal to the delivery-stream ARN, which isolates the refusal to the topic's FIFO-ness.

Emulator: the FIFO Subscribe answers HTTP 200 with a full response envelope — {'SubscriptionArn': 'arn:aws:sns:<region>:<account>:<name>.fifo:<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} — and the subscription is created. No 400, no error code, no message.

Why: a client that depends on AWS rejecting this configuration is handed a subscription AWS would never create, and will believe a FIFO topic fans out to a Firehose delivery stream. In AWS that topic cannot deliver to Firehose at all, so the emulator validates an unshippable topology; rejection tests and policy guards for this class pass here and fail in AWS.

Fix: the Subscribe handler's input validation in the sns service crate — resolve the topic named by TopicArn and, when its FifoTopic attribute is true, refuse any Protocol other than 'sqs' with HTTP 400 / 'InvalidParameter' / "Invalid parameter: Invalid protocol type: <protocol>" before writing subscription state, leaving the standard-topic path untouched. Note: firehose__iam__s3__sns.fifo-topic-firehose-subscription-rejected exercises the same rule under a different suite service heading and is filed separately as that heading's issue; a single fix in the Subscribe validation closes both.

Traces: firehose__sns.firehose-subscribe-on-a-fifo-topic-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

[mismatch] firehose__sns.firehose-subscribe-on-a-fifo-topic-rejected
  File "check_firehose__sns_firehose_subscribe_on_a_fifo_topic_rejected.py", line 198, in run
    got = _refusal(lambda: caller.subscribe(TopicArn=fifo, Protocol="firehose", Endpoint=stream_arn, ...))
  File "check_firehose__sns_firehose_subscribe_on_a_fifo_topic_rejected.py", line 24, in _refusal
    raise AssertionError(f"the call was expected to be refused but returned {result!r}")
  AssertionError: the call was expected to be refused but returned {'SubscriptionArn': 'arn:aws:sns:us-west-2:000000000000:sim2real-xsvc-fhfifo-<id>.fifo:<uuid>', 'ResponseMetadata': {'RequestId': '<id>', 'HTTPStatusCode': 200, ...}}

The check's expectation (line 203-204) is:
  want = ("InvalidParameter", "Invalid parameter: Invalid protocol type: firehose")
followed by: the refused subscribe must leave 0 subscriptions on the FIFO topic.

Expected (AWS): HTTP 400, Error.Code 'InvalidParameter', Error.Message 'Invalid parameter: Invalid protocol type: firehose'; ListSubscriptionsByTopic(<fifo topic>) == [].
Actual (emulator): HTTP 200 with a SubscriptionArn, and the subscription present on the FIFO topic.

The region, account id, generated topic name, subscription uuid and RequestId are per-run values; the 400, the code and the message frame (only the Protocol token varies, taken from the request) are the requirement.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. Create an S3 bucket plus a firehose.amazonaws.com-trusted role with the S3 write set, CreateDeliveryStream(DeliveryStreamType='DirectPut', ExtendedS3DestinationConfiguration={RoleARN, BucketARN, BufferingHints={'IntervalInSeconds':0,'SizeInMBs':1}}) and wait for ACTIVE.
3. Create an sns.amazonaws.com-trusted role allowed firehose:PutRecord/PutRecordBatch/DescribeDeliveryStream on that stream.
4. CreateTopic(Name=<plain>) and CreateTopic(Name=<name>.fifo, Attributes={'FifoTopic':'true'}).
5. Subscribe(TopicArn=<standard>, Protocol='firehose', Endpoint=<stream arn>, Attributes={'SubscriptionRoleArn':<role>}, ReturnSubscriptionArn=True) — must succeed (control).
6. Subscribe(TopicArn=<fifo>, same other arguments). On AWS: ClientError with HTTP 400, Code 'InvalidParameter', Message 'Invalid parameter: Invalid protocol type: firehose'. Observe a 200 and a SubscriptionArn.
7. ListSubscriptionsByTopic(<fifo>) — on AWS empty; observe the created subscription.
8. Re-replay the check in trace_ids to confirm a fix.
