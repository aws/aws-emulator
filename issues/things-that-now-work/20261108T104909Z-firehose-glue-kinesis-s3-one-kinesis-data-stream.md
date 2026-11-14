Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record
Issue-Key: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record|_keys#14|Timeout|return out
Issue-Commit: b173a8e674d7ba975def7d68c06d3de96ae43616

# firehose__glue__kinesis__s3: one Kinesis data stream feeding two Firehose delivery streams delivers every record to neither destination

> _Note: this issue was identified against the code base at commit `b173a8e674d7ba975def7d68c06d3de96ae43616`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: a single Kinesis data stream configured as the KinesisStreamSourceConfiguration of two separate Firehose delivery streams fans out to neither — no delivered object ever lands for either stream.
AWS: each delivery stream with a KinesisStreamAsSource source consumes the shard independently, so a record put once to the stream is delivered by BOTH delivery streams and each destination prefix eventually contains that record's bytes. Recorded behavior only: the Smithy models (kinesis PutRecord/PutRecords, firehose CreateDeliveryStream KinesisStreamSourceConfiguration / S3 destination configuration) describe the wiring and say nothing about delivery; the expectation above is what the recording shows.
Emulator: after records are put to the source stream, neither delivery stream's destination prefix ever receives an object, so the check polling both prefixes is killed by its own budget: "Check timed out after 180.0s".
Why: a client that fans one stream out to two consumers (for example a raw archive plus a converted table) loses all data silently — the delivery streams report healthy configuration while nothing is delivered, and no retry or error surfaces.
Fix: the Firehose Kinesis-source consumer in the firehose service crate (per-delivery-stream shard subscription and the S3 destination writer), so that each delivery stream independently consumes the shared source stream.
Note: the open issue 20261108T043811Z-firehose-glue-kinesis-s3-one-kinesis-stream-feeding-two-firehose-delivery-streams.md describes the same symptom but does not carry this check's replay key, so this check is filed here and that issue is named for context.
Traces: firehose__glue__kinesis__s3.kds-one-kinesis-stream-feeds-two-delivery-streams-each-with-every-record.

## Evidence
Expected (AWS): every record put to the single source Kinesis stream appears, after the buffering interval, under BOTH delivery streams' S3 destination prefixes (each delivery stream delivers the full record set). Actual (emulator): both destination prefixes stay empty for the whole polling window and the check ends "Check timed out after 180.0s"; no object is delivered by either delivery stream. Stream names, bucket names and prefixes in the recording are per-run values.

## Steps to Reproduce
1. CreateStream one Kinesis data stream.
2. CreateDeliveryStream twice with DeliveryStreamType=KinesisStreamAsSource, both naming that same stream ARN in KinesisStreamSourceConfiguration, each with its own S3 destination bucket/prefix.
3. Wait for both delivery streams to report ACTIVE.
4. PutRecord/PutRecords a known record set to the source stream.
5. Poll ListObjectsV2 on both destination prefixes. Observed: no objects ever land for either delivery stream (check times out at 180 s); expected both prefixes to contain every record.
