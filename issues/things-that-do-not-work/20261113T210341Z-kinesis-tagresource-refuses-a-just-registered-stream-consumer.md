Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis.e2e-stream-consumer-fanout-lifecycle
Issue-Shape-Id: com.amazonaws.kinesis#Kinesis_20131202
Issue-Key: kinesis|TagResource|ResourceNotFoundException
Issue-Commit: 4ec947432582384e8f26e5a2677a4359ee4d3593

# kinesis TagResource refuses a just-registered stream consumer's ARN with ResourceNotFoundException instead of tagging it

> _Note: this issue was identified against the code base at commit `4ec947432582384e8f26e5a2677a4359ee4d3593`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Cause 1 of 1: TagResource (and the tag lookup it shares with ListTagsForResource/UntagResource) resolves only stream and channel ARNs, so a stream CONSUMER ARN — a taggable Kinesis resource — resolves to no resource and the call is refused as not-found. Failing check: kinesis.e2e-stream-consumer-fanout-lifecycle.

AWS: Kinesis treats a stream consumer as a first-class taggable resource. Smithy (com-amazonaws-kinesis-kinesis-20131202): RegisterStreamConsumer.Tags (TagMap, len 1..200) lets a consumer be tagged at registration, and TagResource.ResourceARN is modelled @length 1..2048 @pattern ^arn:aws.*:kinesis:.*:\d{12}:.*(stream|channel)/\S+$ — a consumer ARN of the form arn:aws:kinesis:<region>:<account>:stream/<stream-name>/consumer/<consumer-name>:<registration-epoch> (the ConsumerARN pattern on DescribeStreamConsumer.ConsumerARN, ^(arn):aws.*:kinesis:.*:\d{12}:.*stream\/[a-zA-Z0-9_.-]+\/consumer\/[a-zA-Z0-9_.-]+:[0-9]+$) satisfies that pattern, so the ARN is in-contract input, not a malformed one. Doc for TagResource: "Adds or updates tags for the specified Kinesis resource ... You can assign up to 50 tags to a Kinesis resource." For a consumer that RegisterStreamConsumer has just returned, AWS serves TagResource with HTTP 200 and the modelled empty body, and ListTagsForResource on that same consumer ARN then reports the assigned keys. ResourceNotFoundException is reserved for an ARN this account and region hold no resource under.

Emulator: immediately after RegisterStreamConsumer returns a ConsumerARN for an ACTIVE stream, TagResource on that exact ARN answers 400 ResourceNotFoundException with the consumer-not-found sentence. Recorded response: code ResourceNotFoundException, message "Consumer <consumerARN> under stream: <streamName>, account <accountId> not found." — fixed text "Consumer ", " under stream: ", ", account ", " not found."; the variable parts are the request's ResourceARN, the stream name parsed out of it and the caller's account id (in the replay: the ARN the immediately preceding RegisterStreamConsumer minted). The wording itself is AWS's consumer not-found phrasing; the defect is that this refusal is reached at all for a live consumer. The cause is that the one ARN→tag-set lookup the handler uses spans only the kinds this service stores tags for (streams and channels) and has no consumer arm, so every consumer ARN falls through to the not_found() arm.

Why: a client that tags a consumer the way it tags a stream — and every tool that registers an EFO consumer and then stamps ownership/cost-allocation tags on it, including an orchestrator that treats a 400 as fatal — cannot proceed. The refusal is also indistinguishable from "the consumer does not exist", so callers that poll the consumer in response to ResourceNotFoundException loop or abort, and the whole enhanced-fan-out lifecycle (register → tag → subscribe → deregister) stops at the tag step before anything downstream is exercised. Tags on consumers are also unreadable and unremovable for the same reason, since ListTagsForResource and UntagResource read and write that same lookup.

Fix: the ARN resolution behind the resource-tag family — the shared ARN→tag-set lookup in the kinesis service state (resource_tags_by_arn_mut) must recognise a consumer ARN and resolve it to that consumer's stored tag set, so TagResource, ListTagsForResource and UntagResource all address a consumer; the consumer record's state needs a tag set for it to resolve to, and RegisterStreamConsumer.Tags should seed it. Only an ARN naming no existing stream/channel/consumer should reach the not-found arm.

Traces: kinesis.e2e-stream-consumer-fanout-lifecycle (register_stream_consumer → tag_resource(ResourceARN=ConsumerARN)). Related open issues cover different causes in the same area and do NOT cover this one: 20261027T201859Z-kinesis-4-net-new-root-causes-subscribetoshard-is.md (SubscribeToShard unimplemented) and 20261029T200552Z-kinesis-9-net-new-root-causes-resource-policy.md (tag-key validation on streams).

## Evidence
Expected (AWS): RegisterStreamConsumer(StreamARN=<stream>, ConsumerName=<name>) returns Consumer.ConsumerARN; TagResource(ResourceARN=<that ConsumerARN>, Tags={...}) answers HTTP 200 with the modelled empty body, and ListTagsForResource(ResourceARN=<that ConsumerARN>) then lists the assigned keys.

Actual (emulator): TagResource on that same freshly-minted ConsumerARN answers HTTP 400 with error code ResourceNotFoundException and message "Consumer <consumerARN> under stream: <streamName>, account <accountId> not found." Replay detail from the suite:

  botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the TagResource operation: Consumer arn:aws:kinesis:us-west-2:000000000000:stream/<stream>/consumer/<consumer>:<epoch> under stream: <stream>, account 000000000000 not found.

(The ARN, stream name, consumer name, epoch suffix and account id are per-run values filling that message template, not requirements.)

Source: services/service-com-amazonaws-kinesis-kinesis-20131202/src/ops/tag_resource.rs lines 196-216 — apply_assignment() resolves the ARN only through state::resource_tags_by_arn_mut (documented at lines 12-21 and 202-207 as spanning "a stream or a channel, the two kinds that ARN may name") and maps a miss straight to not_found() (lines 186-191). No consumer arm exists in that lookup.

## Steps to Reproduce
1. kinesis.CreateStream(StreamName=<s>, ShardCount=1); wait for DescribeStreamSummary.StreamStatus == ACTIVE.
2. arn = DescribeStreamSummary(StreamName=<s>).StreamDescriptionSummary.StreamARN.
3. consumer_arn = RegisterStreamConsumer(StreamARN=arn, ConsumerName=<c>).Consumer.ConsumerARN.
4. TagResource(ResourceARN=consumer_arn, Tags={"sim2real": "e2e-stream-consumer-fanout-lifecycle"}).
   -> emulator: 400 ResourceNotFoundException "Consumer <consumer_arn> under stream: <s>, account <acct> not found."
   -> AWS: HTTP 200, empty body.
5. ListTagsForResource(ResourceARN=consumer_arn) -> AWS lists the tag; emulator refuses the same way.
Replay: run the behavioral check kinesis.e2e-stream-consumer-fanout-lifecycle.
