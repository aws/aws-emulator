Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__kinesisvideo.deploy-kinesisvideo-stream-round-trip-and-delete cloudformation__kinesisvideo.deploy-kinesisvideo-signalingchannel-round-trip-and-delete
Issue-Key: cloudformation__kinesisvideo|CreateStack|ValidationError
Issue-Commit: 0b683bb56f50ee73f9a0ea465d8433b1b96cfcc7

# cloudformation__kinesisvideo: no AWS::KinesisVideo::* resource type is registered — CreateStack refuses every Kinesis Video Stream/SignalingChannel template as "Unrecognized resource types"

> _Note: this issue was identified against the code base at commit `0b683bb56f50ee73f9a0ea465d8433b1b96cfcc7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's template resource-type registry knows no AWS::KinesisVideo::* type, so a template declaring AWS::KinesisVideo::Stream or AWS::KinesisVideo::SignalingChannel is refused outright at CreateStack and no Kinesis Video resource can ever be deployed by CloudFormation. Failing checks: cloudformation__kinesisvideo.deploy-kinesisvideo-stream-round-trip-and-delete, cloudformation__kinesisvideo.deploy-kinesisvideo-signalingchannel-round-trip-and-delete.

AWS: AWS::KinesisVideo::Stream and AWS::KinesisVideo::SignalingChannel are supported CloudFormation resource types. AWS accepts the template, creates the stack, and provisions each resource against Kinesis Video Streams — the Stream resource maps onto CreateStream (Smithy: CreateStream input StreamName required, @length 1..256, @pattern ^[a-zA-Z0-9_.-]+$; DataRetentionInHours @range min 0; Tags ResourceTags map; output StreamARN) and the SignalingChannel resource maps onto CreateSignalingChannel (Smithy: CreateSignalingChannel input ChannelName required @length 1..256, ChannelType enum, SingleMasterConfiguration.MessageTtlSeconds, Tags TagOnCreateList; output ChannelARN). On AWS, Ref on the resource yields the StreamName/ChannelName, Fn::GetAtt "Arn" yields the StreamARN/ChannelARN that DescribeStream/DescribeSignalingChannel and ListStreams/ListSignalingChannels report, the declared DataRetentionInHours / ChannelType / MessageTtlSeconds read back, stack-level and resource-level tags propagate, the resource reaches ACTIVE by CREATE_COMPLETE, and DeleteStack removes the underlying stream/channel so DescribeStream answers ResourceNotFoundException "The requested stream is not found or not active." and DescribeSignalingChannel answers ResourceNotFoundException "The requested channel is not found or not active.". Recorded behavior only for the exact registry contents; the two checks show AWS accepting both templates.

Emulator: CreateStack fails the request with HTTP 400 ValidationError and the message "Template format error: Unrecognized resource types: [<type>]" — observed with [AWS::KinesisVideo::Stream] and [AWS::KinesisVideo::SignalingChannel]. In that message, "Template format error: Unrecognized resource types: " is fixed text and the bracketed list is the set of unknown type names taken from the submitted template; the bracketed value is NOT part of the requirement, it is whatever the request declared. Because the refusal happens at CreateStack, no stack id is minted and nothing downstream (Outputs, Ref, Fn::GetAtt, ACTIVE status, tag propagation, delete-removes-the-resource) is exercised at all.

Why: any client, CDK app or SAM template that deploys Kinesis Video Streams infrastructure through CloudFormation cannot deploy at all — the whole stack is rejected before creation, rather than the resources being created and addressable at KinesisVideo. A client relying on AWS's behavior sees a template-format error for a template AWS accepts, and has no path to provision a KVS stream or WebRTC signaling channel declaratively.

Fix: CloudFormation's template resource-type registry plus a resource handler per type (create/read/delete + Ref and Fn::GetAtt attribute resolution) that calls the kinesisvideo emulator's CreateStream / CreateSignalingChannel, DescribeStream / DescribeSignalingChannel, TagStream / TagResource and DeleteStream / DeleteSignalingChannel, so the provisioned resource is the same record the kinesisvideo control plane serves and DeleteStack removes it.

Traces: cloudformation__kinesisvideo.deploy-kinesisvideo-stream-round-trip-and-delete (AWS::KinesisVideo::Stream, with DataRetentionInHours and a Tags entry; Outputs Ref=StreamName, GetAtt Arn), cloudformation__kinesisvideo.deploy-kinesisvideo-signalingchannel-round-trip-and-delete (AWS::KinesisVideo::SignalingChannel, Type SINGLE_MASTER, MessageTtlSeconds, Tags; Outputs Ref=ChannelName, GetAtt Arn).

## Evidence
Expected (AWS): CreateStack accepts the template, returns a StackId, the stack reaches CREATE_COMPLETE, Outputs resolve Ref -> declared StreamName/ChannelName and Fn::GetAtt Arn -> the ARN ListStreams/ListSignalingChannels and DescribeStream/DescribeSignalingChannel report, declared properties and tags read back, and DeleteStack removes the stream/channel (subsequent DescribeStream -> ResourceNotFoundException "The requested stream is not found or not active."; DescribeSignalingChannel -> ResourceNotFoundException "The requested channel is not found or not active.").

Actual (emulator): CreateStack raises
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::KinesisVideo::Stream]
and
  botocore.exceptions.ClientError: An error occurred (ValidationError) when calling the CreateStack operation: Template format error: Unrecognized resource types: [AWS::KinesisVideo::SignalingChannel]
HTTP 400, error code ValidationError. No stack is created, so every downstream assertion is unreached.

## Steps to Reproduce
1. Build and boot the emulator; point botocore at it.
2. cloudformation.create_stack(StackName=<name>, TemplateBody=json with Resources.Stream = {"Type": "AWS::KinesisVideo::Stream", "Properties": {"Name": <stream>, "DataRetentionInHours": N, "Tags": [{"Key": "k", "Value": "v"}]}} and Outputs {StreamName: Ref Stream, StreamArn: Fn::GetAtt [Stream, Arn]}).
   -> ClientError ValidationError "Template format error: Unrecognized resource types: [AWS::KinesisVideo::Stream]".
3. Repeat with Resources.Channel = {"Type": "AWS::KinesisVideo::SignalingChannel", "Properties": {"Name": <channel>, "Type": "SINGLE_MASTER", "MessageTtlSeconds": N, "Tags": [...]}}.
   -> ClientError ValidationError "Template format error: Unrecognized resource types: [AWS::KinesisVideo::SignalingChannel]".
4. Expected instead: both stacks reach CREATE_COMPLETE and the resources are addressable via kinesisvideo DescribeStream / DescribeSignalingChannel, ListStreams / ListSignalingChannels and ListTagsForStream / ListTagsForResource; DeleteStack removes them.
