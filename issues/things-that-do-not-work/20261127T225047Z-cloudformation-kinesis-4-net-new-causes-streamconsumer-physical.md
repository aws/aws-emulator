Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kinesis.deploy-cfn-stream-consumer-move-onto-taken-name-rejected-until-freed cloudformation__kinesis.deploy-cfn-stream-consumer-stream-arn-change-replaces-consumer cloudformation__kinesis.deploy-cfn-kinesis-streamconsumer-rename-replaces-consumer cloudformation__kinesis.deploy-cfn-kinesis-stream-max-record-size-applied cloudformation__kinesis.deploy-cfn-kinesis-stream-read-only-warm-throughput-object-in-template-ignored
Issue-Key: cloudformation__kinesis.deploy-cfn-stream-consumer-move-onto-taken-name-rejected-until-freed|run#13|AssertionError|assert _is_consumer_arn(ctx, original, source, consumer), (
Issue-Key: cloudformation__kinesis.deploy-cfn-stream-consumer-stream-arn-change-replaces-consumer|run#14|AssertionError|assert _is_consumer_arn(ctx, old_arn, first, consumer), (
Issue-Key: cloudformation__kinesis.deploy-cfn-kinesis-streamconsumer-rename-replaces-consumer|_until#12|Timeout|time.sleep(2)
Issue-Key: cloudformation__kinesis.deploy-cfn-kinesis-stream-max-record-size-applied|_deploy#2|AssertionError|assert stack["StackStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation__kinesis.deploy-cfn-kinesis-stream-read-only-warm-throughput-object-in-template-ignored|_deploy#4|AssertionError|assert (status, failures) == ("CREATE_COMPLETE", []), (
Issue-Commit: 37bf0949a3a203e7d1b7ebeb80108d58d67e6afd

# cloudformation + kinesis: 4 net-new causes — StreamConsumer physical ids are not consumer ARNs, a consumer rename never settles, MaxRecordSizeInKiB is not carried, and a read-only WarmThroughput object rolls the stack back

> _Note: this issue was identified against the code base at commit `37bf0949a3a203e7d1b7ebeb80108d58d67e6afd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RegisterStreamConsumer

## Description
Four net-new causes on CloudFormation-provisioned Kinesis resources.

Cause 1 of 4: the PhysicalResourceId of an AWS::Kinesis::StreamConsumer is not the consumer ARN.
AWS: the physical id of the resource is the ARN Kinesis minted for the consumer — `arn:<partition>:kinesis:<region>:<account>:stream/<stream-name>/consumer/<consumer-name>:<creation-timestamp>` — so DescribeStackResource and DescribeStreamConsumer agree, and the physical id names both the stream and the consumer name the template asked for. Recorded behaviour only for the exact timestamp suffix.
Emulator: the deployed resource's physical id is not a consumer ARN for the declared stream and consumer name at all, so a caller cannot derive the consumer from the stack resource.
Why: the ARN of an enhanced-fan-out consumer is what SubscribeToShard and every IAM policy reference. A physical id that is not that ARN means the stack resource cannot be used to address the consumer, and a replacement cannot be told apart from an update.
Fix: the AWS::Kinesis::StreamConsumer handler's create/read path — return the consumer ARN as the physical resource id.
Checks: deploy-cfn-stream-consumer-move-onto-taken-name-rejected-until-freed, deploy-cfn-stream-consumer-stream-arn-change-replaces-consumer.

Cause 2 of 4: a stack update that renames an AWS::Kinesis::StreamConsumer never settles. AWS: ConsumerName is create-only, so the update replaces the consumer and the stack reaches UPDATE_COMPLETE within the normal settle window. Emulator: the check reaches its 60 s budget with the stack still not in a terminal state — the replacement hangs. Why: a deploy that renames a consumer never finishes, so the stack is stuck in UPDATE_IN_PROGRESS and the next operation on it is refused. Fix: the AWS::Kinesis::StreamConsumer handler's replace path / the stack-operation progress loop that waits on it. Checks: deploy-cfn-kinesis-streamconsumer-rename-replaces-consumer.

Cause 3 of 4: MaxRecordSizeInKiB declared on AWS::Kinesis::Stream is refused rather than applied. AWS: the property is accepted and carried to Kinesis Data Streams; the stack reaches CREATE_COMPLETE and DescribeStreamSummary reports the configured value. Emulator: the resource fails with `Resource handler returned message: "this emulator does not carry the MaxRecordSizeInKiB property of an AWS::Kinesis::Stream to Amazon Kinesis Data Streams" (RequestToken: <token>, HandlerErrorCode: InvalidRequest)` and the stack settles ROLLBACK_COMPLETE — an emulator-authored refusal of a valid template. Why: any template that sets the larger record size cannot be deployed. Fix: the AWS::Kinesis::Stream handler — pass MaxRecordSizeInKiB through to the stream state. Checks: deploy-cfn-kinesis-stream-max-record-size-applied.

Cause 4 of 4: a READ-ONLY WarmThroughput object present in the template rolls the stack back. AWS: WarmThroughput is a read-only attribute of AWS::Kinesis::Stream; a template that carries it is accepted, the value is ignored, and the stack reaches CREATE_COMPLETE with no VALIDATION_ERROR event. Emulator: the stack settles ROLLBACK_COMPLETE and emits no validation failure either, so the caller is given neither the deploy nor a diagnosis. Why: templates round-tripped out of a stack read commonly carry read-only members; against the emulator they fail to deploy. Fix: the AWS::Kinesis::Stream handler's input projection — strip read-only members before validation. Checks: deploy-cfn-kinesis-stream-read-only-warm-throughput-object-in-template-ignored.

## Evidence
Cause 1: 'the deployed Fanout physical id is not a consumer ARN for the source stream and consumer name'; 'the deployed Reader physical id is not a consumer ARN for the first stream and consumer name'.
Cause 2: 'Check timed out after 60.0s'.
Cause 3: "stack ended 'ROLLBACK_COMPLETE'; failure reasons ['Resource handler returned message: \"this emulator does not carry the MaxRecordSizeInKiB property of an AWS::Kinesis::Stream to Amazon Kinesis Data Streams\" (..., HandlerErrorCode: InvalidRequest)']".
Cause 4: "stack with_read_only=True ended 'ROLLBACK_COMPLETE', validation failures []".

## Steps to Reproduce
Cause 1: CreateStack with an AWS::Kinesis::Stream plus an AWS::Kinesis::StreamConsumer; DescribeStackResource for the consumer and compare the PhysicalResourceId with the ConsumerARN DescribeStreamConsumer reports.
Cause 2: deploy that stack, then UpdateStack changing only ConsumerName and wait for a terminal stack status.
Cause 3: CreateStack with an AWS::Kinesis::Stream that sets MaxRecordSizeInKiB — expect CREATE_COMPLETE.
Cause 4: CreateStack with an AWS::Kinesis::Stream whose Properties include a WarmThroughput object — expect CREATE_COMPLETE with the value ignored.
