Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: firehose__kinesis__s3.kinesis-source-delivers-kpl-aggregated-user-records-de-aggregated firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative
Issue-Key: firehose__kinesis__s3.kinesis-source-delivers-kpl-aggregated-user-records-de-aggregated|run#19|AssertionError|assert not body.startswith(_MAGIC), f"the object holds the aggregated record verbatim: {body[:80]!r}"
Issue-Key: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative|run#19|Timeout|time.sleep(10)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# firehose__kinesis__s3: 2 net-new causes — the CFN-deployed missing-kinesis-permission scenario still reaches no verdict, and a Kinesis-sourced stream writes KPL-aggregated records to S3 verbatim instead of de-aggregating them

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Two independent root causes in the Kinesis-source ingestion path of Firehose. BOTH must be closed before this issue is resubmitted.

Cause 1 of 2: a Kinesis-sourced delivery stream does not de-aggregate KPL-aggregated records; the aggregated protobuf frame is written to S3 byte-for-byte.
  AWS: recorded behavior only (the Smithy model describes no record framing; Firehose documents de-aggregation of KPL records from a Kinesis Data Streams source). A Kinesis record produced by the KPL aggregation format begins with the 4-byte magic prefix 0xF3 0x89 0x9A 0xC2, followed by a protobuf AggregatedRecord carrying N user records and a trailing MD5 checksum. When Firehose ingests such a record from a Kinesis source it splits it into the N user records and delivers THOSE payloads; the delivered S3 object contains the user-record payloads (here 'user-one', 'user-two', 'user-three'), never the magic prefix and never the protobuf envelope.
  Emulator: the delivered S3 object holds the aggregated record verbatim — the check reports the object body starting b'\xf3\x89\x9a\xc2\n\x02u1\n\x02u2\n\x02u3\x1a\r\x08\x00\x1a\tuser-one\n...' — i.e. the ingestion loop copies the raw Kinesis record's Data through to the destination with no KPL de-aggregation step.
  Why: a consumer reading Firehose output expects plain user-record payloads, one per line/record. Any producer using the KPL (the default for the AWS Kinesis Producer Library) yields S3 objects of binary protobuf frames against the emulator, so parsers, Athena/Glue tables and record-count assertions that work in AWS break, and the record count per delivered object is wrong by the aggregation factor.
  Fix: the Kinesis-source ingestion path of the firehose service crate (the component that reads records from the configured KinesisStreamARN and hands them to the destination writer) — detect the KPL aggregation magic prefix, decode the AggregatedRecord, verify its trailing checksum, and emit each contained user record as a separate Firehose record before any processing/buffering stage. Records without the magic prefix must pass through unchanged.
  Traces: firehose__kinesis__s3.kinesis-source-delivers-kpl-aggregated-user-records-de-aggregated.

Cause 2 of 2: the CloudFormation-deployed Kinesis->Firehose->S3 missing-kinesis-permission scenario still produces no answer at all — the check consumes its entire 60 s budget without reaching a single assertion.
  AWS: recorded behavior only. The recording completes this scenario: a CFN stack deploys a Kinesis stream, two Firehose delivery streams over it (one role lacking only kinesis:GetRecords, one sibling holding it), an S3 bucket and a CloudWatch log group; PutRecord into the Kinesis stream results in the well-permissioned stream delivering its record to S3 while the under-permissioned stream delivers nothing and writes a log event with errorCode 'Kinesis.AccessDenied' to its configured log stream; granting the missing permission in place then restores delivery. Every step answers, so the scenario settles well inside the budget.
  Emulator: "Check timed out after 60.0s" — the check never reaches its first assertion, so neither the stack's settled status, nor any S3 object, nor any log event can be compared. One of the steps it performs (the CFN deploy of this estate, the Kinesis PutRecord, or the poll for delivery/log output) never settles, which means no part of this negative-delivery path is observable.
  Why: a client cannot distinguish 'no delivery because the role is denied' from 'no delivery because the pipeline is inert': the whole IAM-enforcement behaviour of a CFN-deployed Kinesis source is unverifiable, and any caller that waits on this pipeline hangs instead of receiving an answer.
  Fix: whichever step of the deploy/ingest/observe path fails to settle — the CloudFormation handling of this estate's resource set in the cloudformation crate and/or the Kinesis-source ingestion loop and its CloudWatch-Logs error reporting in the firehose crate. The scenario must reach a verdict: the permitted sibling delivers, the denied one delivers nothing and logs 'Kinesis.AccessDenied', and an in-place grant restores delivery. Prior issue 20261120T082455Z-firehose-kinesis-s3-the-cfn-deployed-kinesis-firehose.md was marked fixed for exactly this no-verdict symptom, and the check still reaches no verdict; this is not a refusal, so it is filed as a live cause rather than a reopen, but that file is the closest prior art.
  Traces: firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative.

Related open issue 20261126T154215Z-firehose-kinesis-s3-createdeliverystream-accepts-a-kinesisstreamassource-delivery.md covers the SYNCHRONOUS CreateDeliveryStream refusal of an under-permissioned source role and owns its own check; it does not cover either cause above.

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout.

Cause 1:
[mismatch] firehose__kinesis__s3.kinesis-source-delivers-kpl-aggregated-user-records-de-aggregated
  File "check_firehose__kinesis__s3_kinesis_source_delivers_kpl_aggregated_user_records_de_aggregated.py", line 60, in run
    assert not body.startswith(_MAGIC), f"the object holds the aggregated record verbatim: {body[:80]!r}"
  AssertionError: the object holds the aggregated record verbatim: b'\xf3\x89\x9a\xc2\n\x02u1\n\x02u2\n\x02u3\x1a\r\x08\x00\x1a\tuser-one\n\x1a\r\x08\x01\x1a\tuser-two\n\x1a\x0f\x08\x02\x1a\x0buser-three\n\x88\xed\x00?\xcc\xfa\x88\xd6\xddb\x0f\xa8\x1f\xaa\xb1\xbd\xf3'
Expected (AWS): the delivered S3 object does NOT begin with the KPL magic prefix 0xF3899AC2 and contains the de-aggregated user-record payloads, one per contained user record.
Actual (emulator): the object begins with the magic prefix and holds the entire protobuf AggregatedRecord plus its trailing checksum.
The user-record payload strings and the partition keys shown are the check's inputs; the requirement is that the aggregated frame is split and never delivered verbatim.

Cause 2:
[mismatch] firehose__kinesis__s3.cfn-deploy-firehose-missing-kinesis-permission-negative
  Error: Check timed out after 60.0s
Expected (AWS): the scenario settles — stack CREATE_COMPLETE, the sibling stream with kinesis:GetRecords delivers its marker to S3, the stream whose role lacks only kinesis:GetRecords delivers nothing and writes a log event with errorCode 'Kinesis.AccessDenied' to its configured CloudWatch log stream, and granting the permission in place restores delivery.
Actual (emulator): no step answers within the whole 60 s per-check budget; no status, object or log event exists to compare.

## Steps to Reproduce
Cause 1 (KPL de-aggregation):
1. Boot the emulator from this checkout. CreateStream(ShardCount=1), wait ACTIVE; create an S3 bucket and a firehose.amazonaws.com-trusted role with the S3 write set plus kinesis:DescribeStream/GetRecords/GetShardIterator/ListShards on the stream.
2. CreateDeliveryStream(DeliveryStreamType='KinesisStreamAsSource', KinesisStreamSourceConfiguration={KinesisStreamARN, RoleARN}, ExtendedS3DestinationConfiguration={RoleARN, BucketARN, BufferingHints={'SizeInMBs':1,'IntervalInSeconds':0}}); wait ACTIVE.
3. Build one KPL-aggregated Kinesis record: 4-byte magic 0xF3 0x89 0x9A 0xC2 + protobuf AggregatedRecord containing three user records with payloads 'user-one', 'user-two', 'user-three' + the trailing MD5 checksum. kinesis.put_record(Data=<that frame>, PartitionKey=...).
4. Poll the bucket. On AWS the delivered object contains the three user payloads and does not start with the magic prefix; observe the aggregated frame delivered verbatim.

Cause 2 (no verdict):
5. Deploy the check's CFN template: a Kinesis stream, an S3 bucket, a CloudWatch log group with two log streams, a 'good' firehose role with kinesis:GetRecords and a 'bad' role identical except for the missing kinesis:GetRecords, and two KinesisStreamAsSource delivery streams (one per role) writing into the bucket with CloudWatchLoggingOptions enabled.
6. PutRecord two distinct markers into the Kinesis stream.
7. Poll the bucket and both log streams. On AWS the good stream's marker lands and the bad stream logs errorCode 'Kinesis.AccessDenied'; observe that the check never reaches these assertions and exhausts its 60 s budget.
8. Re-replay both checks in trace_ids to confirm the fix.
