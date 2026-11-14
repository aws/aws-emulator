Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes|run#8|AssertionError|assert t[0].get("WriteCapacityUnits") == t[0]["CapacityUnits"], (
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns|_await_state#22|Timeout|time.sleep(interval_s)
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none|_await_exact#13|Timeout|time.sleep(interval_s)
Issue-Commit: d26de989ce4778e024797c8509c4713b425f0550

# DynamoDB: TransactWriteItems ConsumedCapacity omits WriteCapacityUnits, and no AWS/DynamoDB table metrics are ever published (2 causes)

> _Note: this issue was identified against the code base at commit `d26de989ce4778e024797c8509c4713b425f0550`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TransactWriteItems

## Description
Two net-new root causes.

Cause 1 of 2: a write-only transaction's ConsumedCapacity reports CapacityUnits but omits WriteCapacityUnits.
AWS: TransactWriteItems with ReturnConsumedCapacity=TOTAL returns a ConsumedCapacity entry per table in which the write-only transaction's WriteCapacityUnits equals its CapacityUnits (a transactional write costs twice a normal write, and both members report that same doubled figure).
Emulator: the entry carries CapacityUnits=2.0 but WriteCapacityUnits is absent (None).
Why: a client that meters transactional cost from WriteCapacityUnits — the member that separates read from write cost — reads nothing, so capacity accounting and the agreement between the data-plane figure and the AWS/DynamoDB ConsumedWriteCapacityUnits metric cannot be checked.
Fix: the ConsumedCapacity serializer / capacity accounting in the TransactWriteItems handler.
Traces: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes.

Cause 2 of 2: DynamoDB publishes no AWS/DynamoDB table metrics, so neither a capacity series nor an alarm over one is ever observable.
AWS: table activity and provisioned-throughput configuration are published to the AWS/DynamoDB namespace dimensioned by TableName — ConsumedWriteCapacityUnits and, for a PROVISIONED table, ProvisionedWriteCapacityUnits, which tracks an UpdateTable throughput change, while an ON_DEMAND table has no such series at all. An alarm over such a metric therefore crosses its threshold and notifies its SNS topic (and onward to a subscribed SQS queue).
Emulator: both checks poll for those datapoints / the resulting notification and are killed at the harness 60 s cap — no AWS/DynamoDB series ever materializes, so the provisioned-vs-on-demand differential and the alarm-notification chain cannot be observed. (The notification leg also depends on the CloudWatch alarm evaluation engine, filed separately for service `cloudwatch`.)
Why: every DynamoDB capacity dashboard, throttle alarm and autoscaling policy is driven by these series; with none published they are permanently blind, and a client cannot distinguish "on-demand table, no provisioned series" from "no metrics at all".
Fix: AWS/DynamoDB metric publication from the DynamoDB data plane and UpdateTable throughput configuration into the cloudwatch metric store.
Traces: cloudwatch__dynamodb.obs-dynamodb-2-ccf-alarm-notifies-sqs-via-sns, cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none.

## Evidence
Cause 1: assert t[0].get("WriteCapacityUnits") == t[0]["CapacityUnits"] -> "AssertionError: a write-only transaction's WriteCapacityUnits None must equal its CapacityUnits 2.0".
Cause 2: both checks "Error: Check timed out after 60.0s".

## Steps to Reproduce
1. CreateTable, then TransactWriteItems with a single Put and ReturnConsumedCapacity='TOTAL' -> ConsumedCapacity[0] has CapacityUnits=2.0 and no WriteCapacityUnits.
2. CreateTable in PROVISIONED mode, write items, UpdateTable to change ProvisionedThroughput, and poll GetMetricStatistics for AWS/DynamoDB ProvisionedWriteCapacityUnits / ConsumedWriteCapacityUnits dimensioned by TableName -> no datapoints ever appear.
3. PutMetricAlarm over AWS/DynamoDB ConsumedWriteCapacityUnits with an SNS AlarmAction fanning to SQS, then drive write traffic -> no notification arrives.
