Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kinesis__scheduler.kinesis-target-partition-key-at-256-delivered-and-257-refused-on-update kinesis__scheduler.scheduler-kinesis-partition-key-over-256-characters-rejected kinesis__scheduler.scheduler-kinesis-target-writes-the-input-under-the-configured-partition-key
Issue-Key: kinesis__scheduler.kinesis-target-partition-key-at-256-delivered-and-257-refused-on-update|run#32|AssertionError|assert time.monotonic() < deadline, (
Issue-Key: kinesis__scheduler.scheduler-kinesis-partition-key-over-256-characters-rejected|_records#22|AssertionError|raise AssertionError(f"no record reached stream {stream!r} within {budget_s:.0f} s")
Issue-Key: kinesis__scheduler.scheduler-kinesis-target-writes-the-input-under-the-configured-partition-key|_records#13|AssertionError|raise AssertionError(f"no record reached stream {stream!r} within {budget_s:.0f} s")
Issue-Commit: f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2

# kinesis__scheduler: a schedule with an Amazon Kinesis stream target never puts a record on the stream

> _Note: this issue was identified against the code base at commit `f73c567c518c4ea32fbc68f3ca5cb3fd81df2cf2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSchedule

## Description
One net-new cause.

Cause 1 of 1: an EventBridge Scheduler occurrence whose Target.Arn names a Kinesis data stream never reaches the stream.
AWS: when the occurrence fires, Scheduler assumes the execution role and calls kinesis:PutRecord on the target stream, using the schedule's Target.Input as the record Data and Target.KinesisParameters.PartitionKey as the record's partition key, so a consumer reading the stream's shards sees exactly one record per occurrence carrying that data under that partition key. This holds for a one-time at() schedule and for a rate() schedule.
Emulator: nothing is ever written — three independent checks waited out their budgets with no record on the stream ("no record reached stream <name> within 240 s" for two of them, and "no record reached stream <name> from the schedule set to at(<instant>) in time" for the third). No dead-letter event was recorded either.
Why: the Kinesis target is a silent no-op. A client cannot observe delivery at all, so none of the Kinesis-target behaviour that depends on an actual record — the PartitionKey the record lands under, and the 256-vs-257-character PartitionKey boundary that the delivered/refused pair is graded on — can be reached. Nothing distinguishes "delivered" from "never ran".
Fix: the occurrence-delivery path in the scheduler crate — the Kinesis (KinesisParameters) target invoker.
Traces: kinesis__scheduler.kinesis-target-partition-key-at-256-delivered-and-257-refused-on-update, kinesis__scheduler.scheduler-kinesis-partition-key-over-256-characters-rejected, kinesis__scheduler.scheduler-kinesis-target-writes-the-input-under-the-configured-partition-key

## Evidence
kinesis__scheduler.scheduler-kinesis-target-writes-the-input-under-the-configured-partition-key:
  expected one record on the target stream carrying the schedule's Input as Data under Target.KinesisParameters.PartitionKey
  got      AssertionError: no record reached stream '<stream name>' within 240 s
kinesis__scheduler.scheduler-kinesis-partition-key-over-256-characters-rejected:
  got      AssertionError: no record reached stream '<stream name>' within 240 s
kinesis__scheduler.kinesis-target-partition-key-at-256-delivered-and-257-refused-on-update:
  got      AssertionError: no record reached stream <stream name> from the schedule set to at(<instant>) in time
In all three the schedule was created successfully (200) and no dead-letter event was produced.

## Steps to Reproduce
1. Boot the emulator. Create a Kinesis data stream and an IAM role trusted by scheduler.amazonaws.com granting kinesis:PutRecord on it.
2. CreateSchedule with ScheduleExpression at(<now+40s>), FlexibleTimeWindow {Mode: OFF}, State ENABLED and Target{Arn=<stream ARN>, RoleArn=<role>, Input="{\"k\":\"v\"}", KinesisParameters={"PartitionKey": "pk"}}.
3. Get a shard iterator (TRIM_HORIZON) on the stream and poll GetRecords for up to 240 s — expect one record whose Data is the schedule's Input and whose PartitionKey is "pk"; observe no records, and no dead-letter event on a configured DeadLetterConfig queue.
