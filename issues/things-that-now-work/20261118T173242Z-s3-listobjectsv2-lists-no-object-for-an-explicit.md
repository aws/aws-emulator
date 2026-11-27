Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-extended-s3-explicit-defaults-read-back-tag-change-remove-last-and-not-found
Issue-Step: task#14
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-extended-s3-explicit-defaults-read-back-tag-change-remove-last-and-not-found
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.Contents
Issue-Key: s3|ListObjectsV2|2xx->2xx|$.ListBucketResult.KeyCount
Issue-Related: 20261117T165539Z-s3-listobjectsv2-never-lists-the-object-an-explicit.md
Issue-Commit: 003b19ea8201179ec46ed1bcee5f22a6bf4a799a

# s3 ListObjectsV2 lists no object for an explicit-defaults ExtendedS3 DirectPut stream after a tag change

> _Note: this issue was identified against the code base at commit `003b19ea8201179ec46ed1bcee5f22a6bf4a799a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListObjectsV2

## Description
Cause 1 of 1: a record put into a DirectPut delivery stream whose ExtendedS3Destination was created with the buffering hints stated EXPLICITLY at their documented default values, and whose tags were then changed and removed, never becomes listable in the destination bucket. The S3 read-back of the destination bucket stays empty for the whole polling window.

AWS: recorded behavior only for the delivery timing; the model grounds the configuration. Smithy (firehose): BufferingHints.SizeInMBs @range 1-128, doc "The default value is 5"; BufferingHints.IntervalInSeconds @range 0-900, doc "The default value is 300"; the structure doc states the two are "treated as hints, and therefore Firehose might choose to use different values when it is optimal". Stating those default values explicitly is therefore equivalent to omitting them, and the tagging operations (TagDeliveryStream / UntagDeliveryStream) carry no delivery semantics. The recording shows that after the PutRecord the destination bucket holds exactly one delivered object: s3 ListObjectsV2 answers 200 with IsTruncated 'false', KeyCount '1' and one Contents entry keyed under the yyyy/MM/dd/HH/ time prefix, with Size equal to the record's byte length and StorageClass 'STANDARD'. The bucket name, the generated object key, its ETag, LastModified and Size are per-run values and are not part of the requirement; the requirement is that exactly one delivered object becomes listable within the buffering window.

Emulator: ListObjectsV2 answers 200 with a well-formed but EMPTY listing — KeyCount '0' and no Contents member at all — for the entire read-back window. The step polled 19 times and was cut off at the 180 s poll cap with 0 items. Status and error code do not diverge (200 vs 200, no error envelope); the divergence is the permanent absence of the delivered object. Other fixtures in the same corpus do deliver (the direct-put-batch and buffering-update fixtures both list a delivered object), so the delivery path is not globally dead: it is this stream — defaults spelled out, then re-tagged and untagged — that never flushes.

Why: a client that writes through Firehose and verifies by reading the destination bucket (the canonical delivery-stream verification, and the premise of every downstream consumer: Athena/Glue over the delivery prefix, S3 event notifications, log pipelines) observes an empty bucket forever. Nothing is refused, so there is no error to handle: the record is silently dropped and any caller gated on the object's existence hangs until its own timeout.

Fix: the firehose delivery/flush path for ExtendedS3 destinations — the buffering timer that must flush a DirectPut record to the destination bucket when SizeInMBs/IntervalInSeconds are supplied explicitly at their default values, and its interaction with the tag-mutation path on the same stream (a TagDeliveryStream/UntagDeliveryStream must not stop or reset delivery). The s3 ListObjectsV2 handler is reporting the bucket's real (empty) contents correctly and is only where the divergence is observed.

Traces: firehose-extended-s3-explicit-defaults-read-back-tag-change-remove-last-and-not-found#task#14. The replay reported this cluster with an EMPTY owned_by and no blocking stub, so no open issue carries its replay key. Open issue 20261117T165539Z-s3-listobjectsv2-never-lists-the-object-an-explicit.md describes the same fixture step but its recorded Issue-Key (s3|ListObjectsV2|2xx->2xx|$) does not match this cluster's divergence key (service=s3|ListObjectsV2|status=200->200|keys=$,$.ListBucketResult.Contents,$.ListBucketResult.KeyCount), which is why the replay does not treat it as owning this failure; a fixer should treat the two together. Also related but non-covering: 20261111T153432Z-s3-listobjectsv2-reports-keycount-0-and-no-contents.md.

## Evidence
Replayed against the emulator built from this checkout (trace set simparity-wire-aws-v1; emulator_up true, bootstrap_signal false).

Fixture firehose-extended-s3-explicit-defaults-read-back-tag-change-remove-last-and-not-found, step task#14, service s3, operation ListObjectsV2, expected status 200, actual status 200:
  $.ListBucketResult.KeyCount: value_mismatch — expected '1', got '0'
  $.ListBucketResult.Contents: missing — expected one entry (Key under a yyyy/MM/dd/HH/ prefix, LastModified shape-asserted, ETag, ChecksumAlgorithm 'CRC32', ChecksumType 'FULL_OBJECT', Size '31', StorageClass 'STANDARD'), got None
  $.ListBucketResult.Contents: poll: 0 item(s), not 1 after 19 attempt(s)
  $: poll capped at 180s

Emulator body: {'ListBucketResult': {'IsTruncated': 'false', 'Name': <destination bucket>, 'Prefix': '', 'MaxKeys': '1000', 'KeyCount': '0'}}.

The recorded Key, ETag and Size (31 bytes) are that run's values and are not the requirement; the requirement is that one delivered object is listable. No error code or message is involved — both sides answer 200 with a valid ListBucketResult envelope.

## Steps to Reproduce
1. Call git_clone(), then replay_wire_corpus() so the emulator is built and booted from this checkout.
2. Replay fixture firehose-extended-s3-explicit-defaults-read-back-tag-change-remove-last-and-not-found: it creates the destination S3 bucket and a DirectPut delivery stream with an ExtendedS3Destination whose BufferingHints are set explicitly to the documented defaults (SizeInMBs 5, IntervalInSeconds 300), tags the stream, changes and then removes the tags, and PutRecords one small record.
3. At step task#14 the fixture polls s3 ListObjectsV2 on the destination bucket.
4. Observe KeyCount '0' and no Contents for the full 19-attempt / 180 s window, where AWS lists exactly one delivered object. Compare the direct-put-batch fixture in the same corpus, where a delivered object does appear, to confirm the flush path is not globally dead.
