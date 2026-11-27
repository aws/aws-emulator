Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__firehose__s3.cfn-delivery-stream-type-change-replaces-the-stream
Issue-Key: cloudformation__firehose__s3.cfn-delivery-stream-type-change-replaces-the-stream|run#14|AssertionError|assert updated["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudformation__firehose__s3: a replacement update of AWS::KinesisFirehose::DeliveryStream (DeliveryStreamType change) is refused with an emulator-authored NotImplemented reason, so the stack rolls back instead of replacing the stream

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
Cause 1 of 1: CloudFormation cannot replace an AWS::KinesisFirehose::DeliveryStream whose DeliveryStreamType changed. The emulator's resource handler treats every change to an existing delivery stream as an unsupported in-place modification and fails the resource with a reason it authored itself, so the whole stack update rolls back.

AWS: the AWS::KinesisFirehose::DeliveryStream resource declares DeliveryStreamType with "Update requires: Replacement" (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-kinesisfirehose-deliverystream.html). An UpdateStack that changes DeliveryStreamType from DirectPut to KinesisStreamAsSource therefore creates a NEW delivery stream, repoints Ref/Outputs at the new physical name, and then deletes the old one; the stack ends UPDATE_COMPLETE. The replacement stream is created through Firehose CreateDeliveryStream (Smithy: CreateDeliveryStream input DeliveryStreamType enum + KinesisStreamSourceConfiguration {KinesisStreamARN, RoleARN}), so DescribeDeliveryStream on the new name reports DeliveryStreamType 'KinesisStreamAsSource' and Source.KinesisStreamSourceDescription.KinesisStreamARN equal to the template's Kinesis stream. AWS never emits a 'NotImplemented' resource-status reason; when a provider genuinely cannot act it returns a modelled service exception, and no AWS reason ever contains the phrase "this emulator".

Emulator: the update settles as UPDATE_ROLLBACK_COMPLETE and the stack's UPDATE_FAILED event for the delivery-stream resource carries ResourceStatusReason "NotImplemented: this emulator does not change the properties of an existing AWS::KinesisFirehose::DeliveryStream in place; the resource was left as it is." The resource is left untouched, Ref still names the original physical stream, and no KinesisStreamAsSource stream is ever created. The replacement path is not attempted at all: the handler does not distinguish a replacement-triggering property (DeliveryStreamType) from an in-place-updatable one.

Why: any stack whose Firehose delivery stream changes an immutable property is unusable against the emulator. IaC that migrates a stream from DirectPut to a Kinesis source — a routine, documented change — rolls back with a reason no AWS client can interpret, while the same template succeeds in AWS. Downstream resources that Ref the stream keep pointing at a stream of the wrong type, and the emulator-authored reason string leaks implementation prose into a field clients parse for AWS error envelopes.

Fix: the CloudFormation resource handler for AWS::KinesisFirehose::DeliveryStream in the cloudformation service crate — classify the changed properties against the resource's update behaviour, and for a replacement-triggering property (DeliveryStreamType) create the new delivery stream, swap the resource's physical id so Ref and Outputs resolve to it, and delete the predecessor after the update completes. In-place-updatable properties should go through Firehose UpdateDestination rather than failing. Never author a 'NotImplemented' ResourceStatusReason. Open issue 20261111T111216Z-cloudformation-firehose-s3-in-place-update-of-aws-kinesisfirehose-deliverystream-is.md describes the IN-PLACE update variant of the same handler gap; it does not cover the replacement path asserted here, so this is filed separately and named here for the fixer.

Traces: cloudformation__firehose__s3.cfn-delivery-stream-type-change-replaces-the-stream.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

[mismatch] cloudformation__firehose__s3.cfn-delivery-stream-type-change-replaces-the-stream
  File "check_cloudformation__firehose__s3_cfn_delivery_stream_type_change_replaces_the_stream.py", line 203, in run
    assert updated["StackStatus"] == "UPDATE_COMPLETE", (
  AssertionError: changing DeliveryStreamType ended 'UPDATE_ROLLBACK_COMPLETE'; failures [('Stream', 'NotImplemented: this emulator does not change the properties of an existing AWS::KinesisFirehose::DeliveryStream in place; the resource was left as it is.')]

Expected (AWS): DescribeStacks reports StackStatus 'UPDATE_COMPLETE'; the stack's Ref output for the delivery stream names a NEW physical stream name; DescribeDeliveryStream(new name) reports DeliveryStreamType 'KinesisStreamAsSource' with Source.KinesisStreamSourceDescription.KinesisStreamARN == the stack's Kinesis stream ARN; the old stream disappears from ListDeliveryStreams.
Actual (emulator): StackStatus 'UPDATE_ROLLBACK_COMPLETE'; UPDATE_FAILED event on the delivery-stream resource with the emulator-authored ResourceStatusReason quoted above; Ref still names the original stream and no KinesisStreamAsSource stream exists.

The logical id 'Stream' and the generated stack/stream names are per-run values; the requirement is that the replacement update completes and that no 'NotImplemented'/"this emulator" prose ever appears in ResourceStatusReason.

## Steps to Reproduce
1. Boot the emulator from this checkout.
2. CreateStack with a template holding an S3 bucket, a Kinesis stream, a firehose.amazonaws.com-trusted role with the S3 write set and the Kinesis read set, and an AWS::KinesisFirehose::DeliveryStream with DeliveryStreamType 'DirectPut' and an ExtendedS3DestinationConfiguration; Outputs expose Ref of the stream as 'Name' and the Kinesis stream ARN as 'KdsArn'. Wait for CREATE_COMPLETE and record Outputs.Name as `old`.
3. UpdateStack with the same template except DeliveryStreamType 'KinesisStreamAsSource' plus KinesisStreamSourceConfiguration naming the stack's Kinesis stream and role.
4. Poll DescribeStacks until the stack settles. On AWS it is UPDATE_COMPLETE; observe 'UPDATE_ROLLBACK_COMPLETE' instead, and DescribeStackEvents showing the UPDATE_FAILED reason 'NotImplemented: this emulator does not change the properties of an existing AWS::KinesisFirehose::DeliveryStream in place; the resource was left as it is.'
5. Read Outputs.Name again: on AWS it differs from `old`, DescribeDeliveryStream on it reports DeliveryStreamType 'KinesisStreamAsSource' with the Kinesis source description, and `old` is gone from ListDeliveryStreams.
6. Re-replay the check in trace_ids to confirm a fix.
