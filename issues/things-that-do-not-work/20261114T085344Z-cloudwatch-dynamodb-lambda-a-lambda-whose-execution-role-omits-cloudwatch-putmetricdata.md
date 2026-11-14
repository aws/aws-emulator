Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative
Issue-Key: cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative|run#25|AssertionError|assert starved_points == [], f"expected no datapoints for {starved_metric}, got {starved_points}"
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# A Lambda whose execution role omits cloudwatch:PutMetricData still lands its custom metric, so the permission boundary is never enforced

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricData

## Description
Cause 1 of 1: PutMetricData called from inside a Lambda function is served without authorizing it against the function's execution role.

AWS: an IAM principal that has no cloudwatch:PutMetricData grant is denied the call — PutMetricData answers an AccessDenied refusal and NO datapoint is stored, so GetMetricStatistics over the publish window returns an empty Datapoints list for that metric. Once the grant is added to the same role, a later invocation's publish succeeds and the metric appears. Smithy: PutMetricData is a normal sigv4-authenticated CloudWatch operation (model states no exemption); the refusal itself is "recorded behavior only" in this replay — the recording shows the metric simply must not exist while the role lacks the grant.

Emulator: the stream-triggered function whose role omits cloudwatch:PutMetricData processes the record AND its custom metric lands — GetMetricStatistics returns a datapoint (Sum 1.0, Unit None) for the starved metric ItemsProcessed, where AWS returns none.

Why: a client's least-privilege test passes against the emulator and fails in production: the emulator cannot distinguish a correctly denied publish from an allowed one, so every negative-permission assertion over custom metrics is silently inverted, and IAM boundaries around CloudWatch writes appear to be enforced when they are not.

Fix: CloudWatch PutMetricData's authorization — the call must be evaluated against the caller's identity policy (here the Lambda execution role assumed for the invocation) before any datapoint is stored. This is the same root cause family as the open issue 20261109T074412Z-cloudwatch-dynamodb-lambda-putmetricdata-is-never-authorized-against.md, which does not carry this check's replay key.

Traces: cloudwatch__dynamodb__lambda.deploy-lambda-execution-role-missing-cloudwatch-permission-negative.

## Evidence
Expected: no datapoints for the custom metric ItemsProcessed while the execution role lacks cloudwatch:PutMetricData (and datapoints only after the grant is added and a fresh record flows).
Actual (replay): AssertionError: expected no datapoints for ItemsProcessed, got [{'Timestamp': <ts>, 'Sum': 1.0, 'Unit': 'None'}]. The timestamp is a per-run value; the invariant is that any datapoint exists at all.

## Steps to Reproduce
1. Create a DynamoDB source table, a results table and a stream-triggered Lambda whose execution role grants dynamodb/logs access but NOT cloudwatch:PutMetricData.
2. Put an item so the function runs and calls PutMetricData for its custom metric (namespace of the check, metric ItemsProcessed).
3. Call GetMetricStatistics over the publish window: the emulator returns a Sum datapoint; AWS returns an empty Datapoints list because the publish was denied.
