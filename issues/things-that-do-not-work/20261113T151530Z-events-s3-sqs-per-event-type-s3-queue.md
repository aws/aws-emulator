Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__s3__sqs.s3-multi-event-type-routing-sqs
Issue-Key: events__s3__sqs.s3-multi-event-type-routing-sqs|_drain#18|Timeout|resp = sqs.receive_message(
Issue-Commit: fdfcfa2cda546449d36bf7f86b864972a870ca03

# events__s3__sqs: per-event-type S3 queue notification routing never settles — ObjectCreated/ObjectRemoved are not delivered to their own QueueConfiguration destination

> _Note: this issue was identified against the code base at commit `fdfcfa2cda546449d36bf7f86b864972a870ca03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketNotificationConfiguration

## Description
1 net-new root cause in this service.

Cause 1 of 1: a bucket whose notification configuration carries TWO QueueConfigurations — one scoped to `s3:ObjectCreated:*` on queue Q_created and one scoped to `s3:ObjectRemoved:*` on queue Q_removed — never reaches the state AWS reaches. Neither the ObjectCreated positive control on Q_created nor the ObjectRemoved positive control on Q_removed is observed, so the per-event-type routing isolation the configuration declares is never demonstrated. Failing check id: events__s3__sqs.s3-multi-event-type-routing-sqs

AWS: PutBucketNotificationConfiguration takes a NotificationConfiguration whose `QueueConfigurations` is a list of independent destinations (Smithy: s3#NotificationConfiguration member `QueueConfigurations` -> s3#QueueConfigurationList, "The Amazon Simple Queue Service queues to publish messages to and the events for which to publish messages"; s3#QueueConfiguration requires `QueueArn` (xmlName `Queue`) and `Events` (xmlName `Event`, s3#EventList of s3#Event), with optional `Id` and `Filter`). s3#Event is an enum that includes `s3:ObjectCreated:*`, `s3:ObjectCreated:Put`, `s3:ObjectRemoved:*` and `s3:ObjectRemoved:Delete`. Each configured entry is matched independently against the event that just occurred and S3 publishes a `Records` message ONLY to the entries whose `Events` cover it: a PutObject on such a bucket yields exactly one message on Q_created whose `Records[0].eventName` begins with `ObjectCreated` (`ObjectCreated:Put` for a PUT) and NOTHING on Q_removed; a subsequent DeleteObject of that key yields exactly one message on Q_removed whose `Records[0].eventName` begins with `ObjectRemoved` (`ObjectRemoved:Delete` for a delete of an unversioned object) and NOTHING on Q_created. Separately, when a queue is first attached as a notification destination, S3 posts exactly one validation handshake message {"Service":"Amazon S3","Event":"s3:TestEvent",...} that carries no `Records` and is not an object event. The routing rule, the eventName spellings and the s3:TestEvent handshake are documented behavior (https://docs.aws.amazon.com/AmazonS3/latest/userguide/notification-how-to-event-types-and-destinations.html), not modeled; the event literals and the required members are modeled as cited.

Emulator: the scenario never completes — the replay ends with `Check timed out after 60.0s`, with no assertion text, no HTTP status and no error code. The run never gets past waiting for the first real (non-s3:TestEvent) `Records` notification on Q_created after PutObject, so neither positive control (ObjectCreated on Q_created, ObjectRemoved on Q_removed) is ever observed and the two negative controls (no leak in either direction) are never reached. The silent absence is indistinguishable, from a client's point of view, from "the write never happened".

Why: a client that fans one bucket out to per-event-type queues — the standard shape for separating an ingest worker from a tombstone/cleanup worker — receives nothing on either queue. The ingest consumer blocks forever on an upload that did succeed, the removal consumer never learns a key was deleted, and because no error is returned anywhere there is no signal that delivery is broken; a consumer cannot tell "correctly isolated/empty" from "not wired at all".

Fix: the S3 native notification dispatch path and the PutBucketNotificationConfiguration state it reads — a single object mutation must be matched against EVERY stored QueueConfiguration entry independently and produce exactly one `Records` message per matching entry (stamped with that entry's `Id` as `s3.configurationId` and the correct `eventName`), and no message at all for non-matching entries; the one-time `s3:TestEvent` handshake per newly attached destination belongs in the same component.

Traces: events__s3__sqs.s3-multi-event-type-routing-sqs.

This cause is not covered by the open events__s3__sqs issues: 20261107T164117Z-events-s3-sqs-s3-eventbridge-envelopes-are-not.md covers the S3→EventBridge envelope for a form POST and the EventBridgeConfiguration-removal/dual-destination case (EventBridge leg, not two native queue entries scoped to different event types); 20261107T185637Z-events-s3-sqs-s3-event-notification-records-under-encode-the-object.md covers key percent-encoding of '~' in records that DO arrive; 20261028T043345Z-events-s3-sqs-s3-notification-payloads-copy-the.md covers the quoted ETag in detail.object.etag; 20261101T182649Z-events-s3-sqs-puttargets-accepts-an-sqs-fifo.md covers EventBridge PutTargets FIFO validation. None of them is registered as owning this check.

## Evidence
Expected (AWS):
1. PutBucketNotificationConfiguration(B, {QueueConfigurations:[{Id:"created", QueueArn:<Q_created>, Events:["s3:ObjectCreated:*"]}, {Id:"removed", QueueArn:<Q_removed>, Events:["s3:ObjectRemoved:*"]}]}) is accepted (HTTP 200, no error code) once each queue's policy allows sqs:SendMessage for Principal Service s3.amazonaws.com with Condition ArnEquals aws:SourceArn = the bucket ARN.
2. Each newly attached queue receives at most one handshake body {"Service":"Amazon S3","Event":"s3:TestEvent",...} with no "Records" member.
3. PutObject(B, <key>) -> exactly one message on Q_created whose parsed Body has Records[0].eventName starting with "ObjectCreated" ("ObjectCreated:Put" for a PUT); Q_removed yields zero real (non-TestEvent) records over a hold window.
4. DeleteObject(B, <key>) -> exactly one message on Q_removed whose Records[0].eventName starts with "ObjectRemoved" ("ObjectRemoved:Delete" for an unversioned delete); Q_created yields zero real records over a hold window.

Actual (emulator): `Check timed out after 60.0s`. No assertion text, no HTTP status and no error code are produced. The first wait for a real S3 `Records` notification on Q_created after PutObject never satisfies, so neither ObjectCreated nor ObjectRemoved delivery is ever observed and the leak assertions are never evaluated.

Note on values: the bucket name, queue names/URLs/ARNs, the object key and the per-run receipt handles, ETags and sequencers are per-run and are not part of the requirement. The requirements are: independent per-entry matching of `Events` against the mutation, exactly one `Records` message per matching entry and none for non-matching entries, the `eventName` spellings `ObjectCreated:Put` / `ObjectRemoved:Delete`, and the single non-`Records` `s3:TestEvent` handshake per newly attached destination.

## Steps to Reproduce
1. s3:CreateBucket B.
2. sqs:CreateQueue Q_created and Q_removed; for each, sqs:SetQueueAttributes with Policy = {Version:"2012-10-17", Statement:[{Sid:"AllowS3SendMessage", Effect:"Allow", Principal:{Service:"s3.amazonaws.com"}, Action:"sqs:SendMessage", Resource:<that queue ARN>, Condition:{ArnEquals:{"aws:SourceArn": "arn:aws:s3:::"+B}}}]}.
3. s3:PutBucketNotificationConfiguration(B, {QueueConfigurations:[{Id:"created", QueueArn:<Q_created ARN>, Events:["s3:ObjectCreated:*"]}, {Id:"removed", QueueArn:<Q_removed ARN>, Events:["s3:ObjectRemoved:*"]}]}).
4. s3:PutObject(B, "route-me.txt", Body=b"x"). Poll Q_removed... first poll Q_created with sqs:ReceiveMessage (MaxNumberOfMessages=10, WaitTimeSeconds=5), discarding any body whose "Event" == "s3:TestEvent": expect one message with Records[0].eventName starting with "ObjectCreated". Observed: nothing arrives and the run exhausts its budget (`Check timed out after 60.0s`).
5. Drain Q_removed over a hold window and expect zero real (non-TestEvent) records.
6. s3:DeleteObject(B, "route-me.txt"); expect one message on Q_removed with Records[0].eventName starting with "ObjectRemoved", and zero real records on Q_created over a hold window.
7. Re-replay check id events__s3__sqs.s3-multi-event-type-routing-sqs to confirm the fix.
