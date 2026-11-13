Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes
Issue-Key: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes|_await_exact#14|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: 9737b41b9bb889e100c3bf9348295e20ff42401a

# AWS/DynamoDB SuccessfulRequestLatency and ConsumedWriteCapacityUnits are never published, so the PutItem-vs-TransactWriteItems write accounting is unobservable in CloudWatch

> _Note: this issue was identified against the code base at commit `9737b41b9bb889e100c3bf9348295e20ff42401a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause in the cloudwatch__dynamodb group (Cause 1 of 1).

Cause 1 of 1: a metered DynamoDB write emits no AWS/DynamoDB CloudWatch series at all — neither the ConsumedWriteCapacityUnits sum for the table nor a per-Operation SuccessfulRequestLatency series — so a client can never see in CloudWatch what the data plane already told it in ConsumedCapacity.
Failing check: cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes.

AWS: `GetMetricStatistics` is a modelled, served operation (Smithy, service com.amazonaws.cloudwatch#GraniteServiceVersion20100801: GetMetricStatistics input Namespace @required @length 1..255 @pattern ^[^:], MetricName @required @length 1..255, Dimensions list @length 0..30, StartTime/EndTime @required timestamp, Period @required @range min 1, Statistics @length 1..5; output Datapoints; modelled errors InvalidParameterValue/InvalidParameterCombination/MissingParameter/InternalServiceError). The metric catalogue itself is not in the Smithy model, so for the metric names and dimension sets this is recorded behavior only plus what the check asserts: after DynamoDB writes, AWS publishes in namespace `AWS/DynamoDB`
  - `ConsumedWriteCapacityUnits` with dimension set {TableName}, whose Sum over the write window equals the sum of the `ConsumedCapacity.CapacityUnits` the same calls returned; and
  - `SuccessfulRequestLatency` with dimension set {TableName, Operation}, whose SampleCount equals the number of successful calls of that operation against that table, booked under the operation that was actually called and under no other — a table written only with `PutItem` has a {TableName, Operation=PutItem} series and NO {TableName, Operation=TransactWriteItems} series, and a table written only with `TransactWriteItems` has the mirror image.
The check drives two twin PAY_PER_REQUEST tables with six identical writes each (1024 B and 1025 B items, three copies), one table via PutItem and one via a single-Put TransactWriteItems, and then expects plain ConsumedWriteCapacityUnits Sum = 9.0, transactional Sum = 18.0, SuccessfulRequestLatency SampleCount = 6 under each table's own Operation and absent (no datapoints) under the other operation.

Emulator: the API plane is correct and the check gets past every data-plane assertion — TransactWriteItems reports exactly double the PutItem CapacityUnits, WriteCapacityUnits equals CapacityUnits, and both tables hold byte-identical items. It then fails in the CloudWatch poll: every one of the six probed series returns no datapoints, so the snapshot never equals the expected reading and the poll exhausts its ceiling. Reported verdict `error`, at check line 171 in `_await_exact`: `TimeoutError: AWS/DynamoDB write accounting for twin tables 'sim2real-xsvc-ddbm-txp-<id>' (PutItem, 9.0 WCU) and 'sim2real-xsvc-ddbm-txt-<id>' (TransactWriteItems, 18.0 WCU): the run budget ended first; not graded`. The table names, the ids in them and the elapsed budget are per-run values and are not part of the requirement; the requirement is that the six series exist with the stated sums/sample counts and the two absences.

Why: a client that reconciles billing or capacity against CloudWatch, or that alarms/dashboards on DynamoDB write cost, sees an empty namespace. `GetMetricStatistics` answers 200 with `Datapoints: []`, which is indistinguishable from "the write never happened" — so transactional write cost (double) can never be confirmed out of band, no alarm on write capacity can transition, and per-operation attribution (which call type drove the load) is unavailable.

Related but NOT covering this: open issue 20261029T180026Z-cloudwatch-dynamodb-no-aws-dynamodb-cloudwatch-metrics-are-published-and.md describes the absent AWS/DynamoDB consumption metrics for seven sibling checks, but it neither carries this check's replay key nor names the `SuccessfulRequestLatency` {TableName, Operation} family or the per-operation booking/absence requirement, so this check is still open and is filed here.

Fix: the DynamoDB request path's metric emission into the CloudWatch metric store — specifically (a) emitting ConsumedWriteCapacityUnits under dimension {TableName} with the same capacity number the handler already returns in ConsumedCapacity, and (b) emitting SuccessfulRequestLatency under {TableName, Operation} keyed by the operation actually invoked, including for TransactWriteItems, so that the operation not used produces no series at all.

## Evidence
Expected (AWS): after 6 PutItem writes to table A and 6 single-Put TransactWriteItems to twin table B (1024 B and 1025 B items x3), GetMetricStatistics(Namespace="AWS/DynamoDB", Period=60) returns:
  - ConsumedWriteCapacityUnits, Dimensions=[{TableName: A}], Sum = 9.0
  - ConsumedWriteCapacityUnits, Dimensions=[{TableName: B}], Sum = 18.0
  - SuccessfulRequestLatency, Dimensions=[{TableName: A},{Operation: PutItem}], SampleCount = 6.0
  - SuccessfulRequestLatency, Dimensions=[{TableName: B},{Operation: TransactWriteItems}], SampleCount = 6.0
  - SuccessfulRequestLatency, Dimensions=[{TableName: A},{Operation: TransactWriteItems}] -> no datapoints
  - SuccessfulRequestLatency, Dimensions=[{TableName: B},{Operation: PutItem}] -> no datapoints

Actual (emulator): all six calls return an empty `Datapoints` list for the whole window; the poll never reaches the expected snapshot and ends with
  File "check_cloudwatch__dynamodb_metrics_dynamodb_transactional_write_costs_double_in_both_planes.py", line 171, in run -> _await_exact(...)
  TimeoutError: AWS/DynamoDB write accounting for twin tables '<plain table>' (PutItem, 9.0 WCU) and '<txn table>' (TransactWriteItems, 18.0 WCU): the run budget ended first; not graded

The data-plane half agrees with AWS and is not the defect: TransactWriteItems ConsumedCapacity names exactly the transactional table, CapacityUnits is exactly 2x the PutItem cost for the same item, WriteCapacityUnits == CapacityUnits, and ConsistentRead GetItem shows byte-identical items in both tables. Only the CloudWatch plane is empty.

## Steps to Reproduce
1. Boot the emulator and replay cloudwatch__dynamodb.metrics-dynamodb-transactional-write-costs-double-in-both-planes.
2. Minimal repro: CreateTable two PAY_PER_REQUEST tables with a single HASH key `pk` (String) and wait for ACTIVE.
3. PutItem a ~1024 B item into table A with ReturnConsumedCapacity="TOTAL"; TransactWriteItems a single Put of the same item into table B with ReturnConsumedCapacity="TOTAL". Note the returned CapacityUnits (B is double A).
4. Call cloudwatch GetMetricStatistics(Namespace="AWS/DynamoDB", MetricName="ConsumedWriteCapacityUnits", Dimensions=[{"Name":"TableName","Value":A}], StartTime=now-2min, EndTime=now+5min, Period=60, Statistics=["Sum"]). Observe `Datapoints: []` instead of a Sum equal to the capacity the PutItem reported.
5. Call GetMetricStatistics(Namespace="AWS/DynamoDB", MetricName="SuccessfulRequestLatency", Dimensions=[{"Name":"TableName","Value":B},{"Name":"Operation","Value":"TransactWriteItems"}], ..., Statistics=["SampleCount"]). Observe `Datapoints: []` instead of SampleCount equal to the number of transactional writes — and note that the mirror query with Operation=PutItem on table B is also empty, so the correct absence cannot be distinguished from the incorrect one.
