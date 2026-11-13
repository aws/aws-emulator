Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none
Issue-Key: cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none|_await_exact#13|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# cloudwatch__dynamodb: no AWS/DynamoDB ProvisionedWriteCapacityUnits metric is ever published, so provisioned-throughput tracking across UpdateTable (and its absence on PAY_PER_REQUEST) is unobservable (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListMetrics

## Description
Cause 1 of 1: CloudWatch never learns of any AWS/DynamoDB ProvisionedWriteCapacityUnits series for a provisioned table, so ListMetrics reports no dimension set for it.

AWS: a DynamoDB table created with BillingMode=PROVISIONED publishes the AWS/DynamoDB metric ProvisionedWriteCapacityUnits, dimensioned by TableName (and, for indexes, TableName+GlobalSecondaryIndexName). ListMetrics(Namespace="AWS/DynamoDB", MetricName="ProvisionedWriteCapacityUnits", Dimensions=[{Name:TableName,Value:<table>}]) returns exactly those dimension sets, and the value tracks the table's current WriteCapacityUnits across an UpdateTable; a PAY_PER_REQUEST (on-demand) table publishes no such series at all. Recorded behavior: the check waits for the exact dimension set to appear before grading the value change.

Emulator: the dimension set never appears. The check polls ListMetrics for the whole budget and ends with TimeoutError: "list_metrics(AWS/DynamoDB, ProvisionedWriteCapacityUnits, TableName='<table>') exact dimension set: the run budget ended first; not graded". No AWS/DynamoDB provisioned-throughput series is minted on CreateTable, and none appears after UpdateTable changes the write capacity, so the positive leg never even starts and the on-demand negative leg is vacuous.

Why: a client that monitors DynamoDB throughput — dashboards, provisioned-vs-consumed ratios, autoscaling target-tracking policies that read ProvisionedWriteCapacityUnits, alarms on capacity changes — sees an empty namespace. ListMetrics returning nothing is indistinguishable from a table that does not exist, so operational tooling silently reports healthy with no data, and the documented distinction between provisioned and on-demand billing modes cannot be observed.

Fix: the metric-emission path that must publish AWS/DynamoDB table metrics into CloudWatch's metric store — it belongs in the DynamoDB table state/UpdateTable handler (minting and updating the provisioned-capacity series and its TableName/index dimensions) and in the CloudWatch metric index that ListMetrics reads. A sibling open issue, 20261025T020208Z-cloudwatch-dynamodb-dynamodb-transactwriteitems-consumedcapacity-omits-writecapacityunits-and-no-aws.md, describes missing AWS/DynamoDB table metrics reached through other checks but does not carry this check's replay key or the provisioned-capacity series.

Traces: cloudwatch__dynamodb.obs-dynamodb-3-provisioned-write-capacity-tracks-update-on-demand-has-none

## Evidence
Expected (AWS): after CreateTable with BillingMode=PROVISIONED and ProvisionedThroughput{WriteCapacityUnits:3, ReadCapacityUnits:1}, ListMetrics(Namespace="AWS/DynamoDB", MetricName="ProvisionedWriteCapacityUnits") reports exactly the dimension set [{Name:"TableName", Value:<table>}] (plus per-index sets where indexes exist); the datapoint value follows UpdateTable from 3 to 4; an on-demand twin table yields no dimension set at all.

Actual (emulator): the dimension set never materialises.
  File "check_cloudwatch__dynamodb_obs_dynamodb_3_provisioned_write_capacity_tracks_update_on_demand_has_none.py", line 122, in run / line 73, in _await_exact
  TimeoutError: list_metrics(AWS/DynamoDB, ProvisionedWriteCapacityUnits, TableName='sim2real-xsvc-ddbm-pwp-594ea7453439') exact dimension set: the run budget ended first; not graded
(the table name is a per-run value; the requirement is that the series exists with the TableName dimension for any provisioned table.)

## Steps to Reproduce
1. CreateTable(BillingMode=PROVISIONED, ProvisionedThroughput={ReadCapacityUnits:1, WriteCapacityUnits:3}); wait for ACTIVE.
2. CreateTable a twin with BillingMode=PAY_PER_REQUEST; wait for ACTIVE.
3. ListMetrics(Namespace="AWS/DynamoDB", MetricName="ProvisionedWriteCapacityUnits", Dimensions=[{Name:"TableName",Value:<provisioned table>}]).
Observed: no metrics returned, ever. Expected: exactly the TableName dimension set for the provisioned table.
4. UpdateTable(WriteCapacityUnits=4) and re-read the datapoints; expected to track 4. Observed: still no series.
5. ListMetrics for the on-demand twin; expected: no dimension set (currently vacuously true).
