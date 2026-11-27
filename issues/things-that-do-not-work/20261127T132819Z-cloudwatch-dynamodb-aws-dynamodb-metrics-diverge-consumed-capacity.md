Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb.consumed-capacity-metrics-carry-the-count-unit cloudwatch__dynamodb.on-demand-max-request-units-metrics-report-the-configured-maximums cloudwatch__dynamodb.user-errors-metric-counts-refused-requests
Issue-Key: cloudwatch__dynamodb.consumed-capacity-metrics-carry-the-count-unit|run#18|AssertionError|assert units == ["Count"], f"ConsumedWriteCapacityUnits datapoints carry unit(s) {units!r}"
Issue-Key: cloudwatch__dynamodb.on-demand-max-request-units-metrics-report-the-configured-maximums|run#17|AssertionError|assert read == float(_MAX_READ), (
Issue-Key: cloudwatch__dynamodb.user-errors-metric-counts-refused-requests|run#18|AssertionError|assert total >= expected_floor, (
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# cloudwatch__dynamodb: AWS/DynamoDB metrics diverge — consumed-capacity datapoints carry no unit, OnDemandMax*RequestUnits is never published, and UserErrors never counts refusals (3 causes)

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Three net-new root causes in the AWS/DynamoDB metric publication path.

Cause 1 of 3: ConsumedReadCapacityUnits / ConsumedWriteCapacityUnits datapoints are published with no unit.
AWS: those metrics are published with `Unit: Count`, so GetMetricStatistics datapoints carry `Unit: 'Count'` and a unit-filtered query matches.
Emulator: the datapoints carry unit `None` — the unit is omitted when the metric is put.
Why: a GetMetricStatistics or GetMetricData call that filters on `Unit=Count` returns nothing, and dashboards and alarms built on the unit cannot resolve the series.
Fix: the DynamoDB metric emitter for consumed capacity — set Unit to Count on the PutMetricData datum.
Traces: cloudwatch__dynamodb.consumed-capacity-metrics-carry-the-count-unit.

Cause 2 of 3: OnDemandMaxReadRequestUnits / OnDemandMaxWriteRequestUnits are not published for an on-demand table with configured maximums.
AWS: a table whose OnDemandThroughput sets maximum request units publishes the AWS/DynamoDB `OnDemandMaxReadRequestUnits` and `OnDemandMaxWriteRequestUnits` metrics with the `TableName` dimension, whose latest Maximum equals the configured value.
Emulator: the latest Maximum is `None` — the series does not exist.
Why: the configured on-demand ceiling cannot be observed or alarmed on; a capacity dashboard reads empty.
Fix: the DynamoDB metric emitter — publish the on-demand maximum request-unit metrics for tables that configure them.
Traces: cloudwatch__dynamodb.on-demand-max-request-units-metrics-report-the-configured-maximums.

Cause 3 of 3: the UserErrors metric does not count 4xx-refused requests.
AWS: every request DynamoDB refuses with a 400-class error increments the account/region-level AWS/DynamoDB `UserErrors` metric, so the Sum over the window rises by the number of refusals.
Emulator: the emulator refused six requests with HTTP 400 in the probe window and `UserErrors` Sum stayed at its baseline 0.0 for the full 600s window — the metric is never incremented.
Why: the standard signal for client-side error rate is flat at zero, so an alarm on UserErrors can never fire and an operator cannot see a misbehaving client.
Fix: the DynamoDB request pipeline's metric hook — increment UserErrors from the same counter that produces the 4xx refusal.
Traces: cloudwatch__dynamodb.user-errors-metric-counts-refused-requests.

## Evidence
Cause 1: `ConsumedWriteCapacityUnits datapoints carry unit(s) ['None']` — expected ['Count'].
Cause 2: `OnDemandMaxReadRequestUnits{TableName} latest Maximum is None, expected 100` (100 is the value configured by the request).
Cause 3: `AWS/DynamoDB UserErrors Sum must rise by at least 6 over its baseline 0.0 within 600s for the 6 requests this check rejected with HTTP 400; observed 0.0`.

## Steps to Reproduce
1. Drive writes against a table and call GetMetricStatistics for AWS/DynamoDB ConsumedWriteCapacityUnits; inspect each datapoint's Unit.
2. Create an on-demand table with OnDemandThroughput maximum request units and query OnDemandMaxReadRequestUnits with the TableName dimension.
3. Issue several requests DynamoDB refuses with HTTP 400, then query AWS/DynamoDB UserErrors Sum over the window.
