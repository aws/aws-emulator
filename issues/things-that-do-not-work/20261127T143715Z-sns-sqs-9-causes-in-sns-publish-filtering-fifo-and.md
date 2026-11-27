Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns__sqs.an-invalid-message-attribute-type-prefix-rejects-publish-and-publish-batch-alike sns__sqs.archive-replay-marks-each-replayed-notification sns__sqs.attribute-cidr-filter-with-prefix-length-32-is-rejected-and-31-delivers-its-pair sns__sqs.exists-false-withholds-a-message-published-without-attributes sns__sqs.fifo-topic-and-fifo-queue-conversion-to-standard-rejected sns__sqs.fifo-topic-standard-queue-subscriber-gets-no-message-group-id sns__sqs.message-body-exists-true-matches-only-a-leaf-value sns__sqs.message-body-filter-matches-a-key-inside-an-array-of-objects sns__sqs.publish-batch-total-bounded-by-a-raised-topic-maximum-message-size-rejected sns__sqs.topic-raised-to-one-mib-delivers-a-large-payload-intact
Issue-Key: sns__sqs.an-invalid-message-attribute-type-prefix-rejects-publish-and-publish-batch-alike|_expect_error#10|AssertionError|assert error.get("Message") == message, f"expected message {message!r}, got {error.get('Message')!r}"
Issue-Key: sns__sqs.archive-replay-marks-each-replayed-notification|run#12|AssertionError|assert envelope.get("Replayed") == "true", f"a replayed notification must carry Replayed=true; got {envelope!r}"
Issue-Key: sns__sqs.attribute-cidr-filter-with-prefix-length-32-is-rejected-and-31-delivers-its-pair|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Key: sns__sqs.exists-false-withholds-a-message-published-without-attributes|run#6|AssertionError|assert got["bare"] == [], (
Issue-Key: sns__sqs.fifo-topic-and-fifo-queue-conversion-to-standard-rejected|run#16|AssertionError|assert topic_refusal == (
Issue-Key: sns__sqs.fifo-topic-standard-queue-subscriber-gets-no-message-group-id|run#17|AssertionError|assert "MessageGroupId" not in std_attrs, (
Issue-Key: sns__sqs.message-body-exists-true-matches-only-a-leaf-value|run#6|AssertionError|assert got["node"] == [], (
Issue-Key: sns__sqs.message-body-filter-matches-a-key-inside-an-array-of-objects|run#6|AssertionError|assert got["key-match"] == [created], (
Issue-Key: sns|PublishBatch|BatchRequestTooLong
Issue-Key: sns__sqs|Publish|InvalidParameter
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 9 causes in SNS publish, filtering, FIFO and archive-replay behaviour

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
Nine causes.

Cause 1 of 9: the invalid-message-attribute-type refusal message omits the serial comma AWS writes.
AWS: Publish and PublishBatch refuse an unsupported message-attribute type prefix with `InvalidParameterValue` / awsQueryError `ParameterValueInvalid` (Smithy: com.amazonaws.sns InvalidParameterValueException, HTTP 400) and the message `The message attribute '<name>' has an invalid message attribute type, the set of supported type prefixes is Binary, Number, and String.` — only `<name>` is taken from the request; the rest, including the comma before `and`, is fixed text.
Emulator: the message reads `... the set of supported type prefixes is Binary, Number and String.` (no comma before `and`).
Why: a client or operator matching AWS's documented wording never matches, and a test asserting the refusal text fails against the emulator while passing against AWS.
Fix: the SNS Publish/PublishBatch message-attribute validation message.
Traces: sns__sqs.an-invalid-message-attribute-type-prefix-rejects-publish-and-publish-batch-alike.

Cause 2 of 9: a replayed notification's envelope does not carry `Replayed`.
AWS: when a subscription with a ReplayPolicy replays archived messages, each replayed notification's JSON envelope carries the member `"Replayed": "true"`; a notification delivered live afterwards has no such member.
Emulator: the replayed notification's envelope carries `Type`, `MessageId`, `TopicArn`, `Message`, `Timestamp`, `SequenceNumber`, `SignatureVersion`, `Signature`, `SigningCertURL` and `UnsubscribeURL` but no `Replayed`.
Why: a subscriber's only way to tell a replayed message from a live one is this member; without it, replay causes duplicate processing of messages the consumer already handled.
Fix: the SNS replay delivery path's notification envelope serializer.
Traces: sns__sqs.archive-replay-marks-each-replayed-notification.

Cause 3 of 9: a subscription filter policy with a `cidr` operand whose prefix length is /32 is accepted.
AWS: the `cidr` filter operand requires a prefix length shorter than the address width, so a /32 IPv4 block is refused on both Subscribe and SetSubscriptionAttributes with `InvalidParameter` (Smithy: com.amazonaws.sns Subscribe/SetSubscriptionAttributes error InvalidParameterException, awsQueryError `InvalidParameter`, HTTP 400); no subscription is created and an existing FilterPolicy is left unchanged.
Emulator: Subscribe with a /32 cidr filter succeeds with HTTP 200 and returns a SubscriptionArn.
Why: the client ships a filter policy AWS refuses, and the emulator's matching behaviour for /32 has no AWS counterpart, so messages routed in the emulator will not be routed in production.
Fix: the SNS filter-policy validator's `cidr` operand bounds.
Traces: sns__sqs.attribute-cidr-filter-with-prefix-length-32-is-rejected-and-31-delivers-its-pair.

Cause 4 of 9: an attribute filter policy's `exists: false` matches a message published with no attributes at all.
AWS: `{"<key>": [{"exists": false}]}` in an attribute-scoped policy matches only a message that carries at least one message attribute and not `<key>`; a message published with no MessageAttributes at all does not match and is withheld.
Emulator: the message published with no attributes is delivered.
Why: the client's "everything except X" filter also captures attribute-less traffic, so a subscriber receives messages the policy was written to exclude.
Fix: the SNS attribute-scope filter matcher's `exists: false` semantics.
Traces: sns__sqs.exists-false-withholds-a-message-published-without-attributes.

Cause 5 of 9: SetTopicAttributes refuses the `FifoTopic` attribute with the wrong code and message.
AWS: converting a FIFO topic to standard is not supported; SetTopicAttributes with AttributeName `FifoTopic` is refused with `InvalidParameter` and the message `Invalid parameter: FifoTopic: Modifying topic type is not supported` (the whole string is fixed text).
Emulator: the refusal reads `('InvalidParameter', 'Invalid parameter: AttributeName')` — the attribute is rejected as unknown rather than as immutable.
Why: the code is right but the reason is lost: a client cannot tell "this attribute does not exist" from "this attribute cannot be modified", and any handler matching AWS's wording fails.
Fix: the SNS SetTopicAttributes handler's attribute table and its immutable-attribute refusal message.
Traces: sns__sqs.fifo-topic-and-fifo-queue-conversion-to-standard-rejected.

Cause 6 of 9: a FIFO topic's delivery to a standard queue subscriber carries MessageGroupId.
AWS: MessageGroupId is a FIFO concept. When a FIFO topic delivers to a standard SQS queue, the enqueued message carries the standard system attributes (SentTimestamp and so on) but no `MessageGroupId`.
Emulator: the standard queue's message carries `MessageGroupId` (the publisher's group value).
Why: a consumer that branches on the presence of MessageGroupId — the usual way to tell FIFO from standard delivery — takes the FIFO path on a standard queue, where ordering and deduplication guarantees do not hold.
Fix: the SNS-to-SQS delivery path's system-attribute projection (set MessageGroupId only for FIFO destinations).
Traces: sns__sqs.fifo-topic-standard-queue-subscriber-gets-no-message-group-id.

Cause 7 of 9: a message-body filter policy's `exists: true` matches an intermediate object node.
AWS: with `FilterPolicyScope: MessageBody`, `exists: true` matches only when the named key is present with a leaf (scalar) value; a key whose value is an object does not match and the message is withheld.
Emulator: a body whose key is an intermediate object (`{"order": {"id": "ord-1"}}`) is delivered.
Why: the client's body filter is more permissive than AWS's, so a subscriber receives messages whose shape it was configured to exclude and processes them wrongly.
Fix: the SNS message-body filter matcher's `exists` operator (leaf-only semantics).
Traces: sns__sqs.message-body-exists-true-matches-only-a-leaf-value.

Cause 8 of 9: a message-body filter policy does not match a key inside an array of objects.
AWS: with `FilterPolicyScope: MessageBody`, a policy path that traverses an array of objects (e.g. `Records[].s3.object.key` with a `prefix` operand, or `Records[].eventName`) matches when any element satisfies it, so the matching event is delivered and a non-matching one withheld.
Emulator: the matching event is not delivered at all (the subscriber receives nothing).
Why: the canonical S3-notification-through-SNS filter — the shape AWS documents for this feature — never matches, so the subscriber receives nothing while its policy looks correct.
Fix: the SNS message-body filter matcher's array traversal.
Traces: sns__sqs.message-body-filter-matches-a-key-inside-an-array-of-objects.

Cause 9 of 9: a topic's `MaximumMessageSize` attribute is not honoured; the default 256 KiB bound is enforced instead.
AWS: a topic's MaximumMessageSize attribute raises the per-message and per-batch size bound for that topic (the check reads it back from GetTopicAttributes before publishing). A single Publish up to that size succeeds and is delivered intact (same bytes, same MD5OfBody at the queue); a PublishBatch whose entries total exactly the bound succeeds and one byte over is refused `BatchRequestTooLong` (Smithy: com.amazonaws.sns PublishBatch error BatchRequestTooLongException, awsQueryError `BatchRequestTooLong`, HTTP 400) with the fixed message `The length of all the messages put together is more than the limit.`
Emulator: the raised bound is ignored. A 1 MiB publish is refused `InvalidParameter` / `Invalid parameter: Message too long`, and a batch totalling exactly the raised bound is refused `BatchRequestTooLong`.
Why: a client that raises its topic's message size — the documented way to carry large payloads — has every oversized-by-the-old-limit publish refused, so the feature is unusable and well-formed traffic is dropped at the API.
Fix: the SNS Publish and PublishBatch size validation (read the bound from the topic's MaximumMessageSize attribute rather than a constant).
Traces: sns__sqs.topic-raised-to-one-mib-delivers-a-large-payload-intact, sns__sqs.publish-batch-total-bounded-by-a-raised-topic-maximum-message-size-rejected.

## Evidence
Cause 1: expected `The message attribute '<name>' has an invalid message attribute type, the set of supported type prefixes is Binary, Number, and String.`; actual `... is Binary, Number and String.`
Cause 2: `AssertionError: a replayed notification must carry Replayed=true; got {'Type': 'Notification', 'MessageId': ..., 'TopicArn': ..., 'Message': ..., 'Timestamp': ..., 'SequenceNumber': ..., 'SignatureVersion': '1', 'Signature': ..., 'SigningCertURL': ..., 'UnsubscribeURL': ...}` — no Replayed member.
Cause 3: `AssertionError: the call was expected to be refused but returned {'SubscriptionArn': 'arn:aws:sns:...:<topic>:<uuid>', 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`.
Cause 4: `AssertionError: exists:false must not match a message with no attributes at all; the queue received ['<bare message>']`.
Cause 5: expected ('InvalidParameter', 'Invalid parameter: FifoTopic: Modifying topic type is not supported'); actual `('InvalidParameter', 'Invalid parameter: AttributeName')`.
Cause 6: `AssertionError: the standard queue message carries MessageGroupId '<group>'`.
Cause 7: `AssertionError: exists:true must not match an intermediate object node; the queue received ['{"order": {"id": "ord-1"}}']`.
Cause 8: `AssertionError: a Records[].s3.object.key prefix policy must deliver the matching event; got []`.
Cause 9: single Publish — expected 200 and an intact delivery; actual `An error occurred (InvalidParameter) when calling the Publish operation: Invalid parameter: Message too long`. Batch at exactly the bound — expected every entry Successful; actual `An error occurred (BatchRequestTooLong) when calling the PublishBatch operation: The length of all the messages put together is more than the limit.` The bound itself is the topic's MaximumMessageSize attribute value, not a literal.

## Steps to Reproduce
Replay each listed check id. In outline: (1) Publish with MessageAttributes type `NotAType.custom`; (2) archive a FIFO topic, subscribe with a ReplayPolicy and inspect the replayed envelope; (3) Subscribe with a FilterPolicy using a `cidr` operand of prefix length 32; (4) attribute policy `{"store": [{"exists": false}]}` and publish with no MessageAttributes; (5) SetTopicAttributes AttributeName=FifoTopic AttributeValue=false on a FIFO topic; (6) subscribe a standard queue to a FIFO topic, publish with MessageGroupId and read the queue's system attributes; (7) MessageBody policy `{"order": [{"exists": true}]}` with body `{"order": {"id": "ord-1"}}`; (8) MessageBody policy over `Records[].s3.object.key` with a prefix operand; (9) CreateTopic with MaximumMessageSize=1048576, then Publish a payload just under it and PublishBatch entries totalling exactly it.
