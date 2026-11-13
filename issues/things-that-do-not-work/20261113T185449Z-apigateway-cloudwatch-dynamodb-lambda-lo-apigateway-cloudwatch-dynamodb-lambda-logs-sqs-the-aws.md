Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config|_assert_alarm_configuration#9|AssertionError|assert [r["AlarmName"] for r in rows] == [name], f"DescribeAlarms for {name} returned {rows!r}"
Issue-Key: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree|never_reached#9|AssertionError|raise AssertionError(f"{what}; not reached after {waited_s:.0f}s of polling")
Issue-Commit: 11bf1cc5011aa260df7304f2661cde6d9105badf

# apigateway__cloudwatch__dynamodb__lambda__logs__sqs: the AWS::CloudWatch::Alarm in the CDK-synthesized, asset-free CloudFormation deploy is never created — DescribeAlarms answers an empty MetricAlarms

> _Note: this issue was identified against the code base at commit `11bf1cc5011aa260df7304f2661cde6d9105badf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: after the CDK-synthesized order-service template is deployed asset-free through plain CloudFormation and the stack reports CREATE_COMPLETE, the AWS::CloudWatch::Alarm the template declares does not exist in CloudWatch. DescribeAlarms(AlarmNames=[<the alarm name the stack was given>]) returns MetricAlarms = [], so the alarm can neither be read for its configuration nor ever evaluate into ALARM.

AWS: recorded behavior only for the CFN-to-CloudWatch wiring — the recording shows that a stack which reaches CREATE_COMPLETE has actually created every resource it declares, so DescribeAlarms(AlarmNames=[name]) returns exactly one MetricAlarm for a declared AWS::CloudWatch::Alarm whose AlarmName is that name, carrying the declared Namespace/MetricName/Period/Threshold/ComparisonOperator/EvaluationPeriods. CloudWatch then evaluates it: with no datapoints it reads INSUFFICIENT_DATA, and once a datapoint over the threshold is published it transitions to ALARM and DescribeAlarmHistory(HistoryItemType="StateUpdate") records an INSUFFICIENT_DATA->ALARM item whose HistoryData JSON names the old and new states. On stack delete the alarm disappears from DescribeAlarms. Both failing checks depend only on that: the alarm has no actions, so its state is read by polling DescribeAlarms.

Emulator: DescribeAlarms(AlarmNames=[name]) returns [] after the deploy leg succeeded and the earlier legs (API Gateway -> Lambda -> DynamoDB wiring, the SQS worker, the structured logs, and the custom metric datapoint) all PASSED. The alarm-config check asserts the alarm exists and fails immediately with an empty row set; the end-to-end check reads StateValue as None (its reader returns rows[0]["StateValue"] if rows else None) and polls for 424 s before reporting that the alarm never reached ALARM although a metric datapoint over its threshold of 1 had been observed by the preceding leg. One absent resource, two failing checks.

Why: the alarm is the application's only observability gate. A client that deploys this stack, sees CREATE_COMPLETE and then reads its alarm gets an empty answer: nothing tells it the alarm is missing, so the deploy looks healthy while no threshold is ever evaluated and no state transition is ever recorded. Code that waits for an alarm to settle hangs; code that only reads it concludes "no alarm state" and proceeds unmonitored. Because the metric datapoint IS published (the metrics leg passed), the divergence is specifically the CFN-created alarm resource, not metric ingestion.

Fix: the CloudFormation provisioning of AWS::CloudWatch::Alarm — the resource must be created in CloudWatch state (registered under the template's AlarmName, with its declared metric/period/threshold/evaluation settings) before the stack is reported CREATE_COMPLETE, and deleted again on stack delete; once registered it must be picked up by alarm evaluation so the published datapoint drives the INSUFFICIENT_DATA->ALARM transition and a StateUpdate history item. The CDK-synthesized/asset-free path must go through the same provisioning as the hand-written template path.

Traces: apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config (fails on the alarm's absence) and apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree (fails on the state that absence makes unreachable).

Note: the sibling flavors of this same scenario are already owned — xflavor-order-service-cfn by 20261111T110051Z-apigateway-cloudwatch-dynamodb-lambda-lo-apigateway-cloudwatch-dynamodb-lambda-logs-sqs-a-cloudformation.md, xflavor-order-service-cfn-alarm-echoes-declared-period by 20261110T214424Z-...-two-more.md and xflavor-order-service-sam by 20261109T143515Z-...-rolls-back-generated.md. Neither of those carries the replay key of the two cdk-assetfree checks above, which is why they are filed here. Open issues describing the same shape in OTHER services (e.g. 20261111T111216Z-cloudformation-cloudwatch-describealarms-returns-nothing-for-an-alarm-cloudformation-reported.md, 20261109T223435Z-cloudformation-cloudwatch-alarms-a-stack-reports-create-complete.md) do not cover this service's checks.

## Evidence
Check apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree-alarm-config:

  File "...xflavor_order_service_cdk_assetfree_alarm_config.py", line 888, in _assert_alarm_configuration
  assert [r["AlarmName"] for r in rows] == [name], f"DescribeAlarms for {name} returned {rows!r}"
  AssertionError: DescribeAlarms for sim2real-xsvc-alarm-fa6b638a9858 returned []

Expected: DescribeAlarms(AlarmNames=[<AlarmName the stack was deployed with>]) -> MetricAlarms = [ one alarm with that AlarmName and the declared Namespace/MetricName/Period/Threshold ].
Actual: MetricAlarms = [].

Check apigateway__cloudwatch__dynamodb__lambda__logs__sqs.xflavor-order-service-cdk-assetfree:

  File "...xflavor_order_service_cdk_assetfree.py", line 908, in _assert_alarm_transition
  legs.never_reached(...)
  AssertionError: alarm sim2real-xsvc-alarm-beb9347acfa5 still reads None with a OrdersFulfilled datapoint over its threshold of 1; not reached after 424s of polling

The state reader is `rows = cw.describe_alarms(AlarmNames=[name])["MetricAlarms"]; return rows[0]["StateValue"] if rows else None` — StateValue None is literally "DescribeAlarms returned no rows", the same empty answer as the first check. The preceding legs (wiring, worker, logs, metrics) PASSED, so the OrdersFulfilled datapoint over the threshold was observed before the alarm leg ran; the check then also wanted DescribeAlarmHistory(HistoryItemType="StateUpdate") to hold an INSUFFICIENT_DATA->ALARM item, which it never got to read.

(The alarm names shown — sim2real-xsvc-alarm-<hex> — are generated per run by the check and passed INTO the stack; the requirement is that DescribeAlarms answers for whatever AlarmName the template declared, not for these literals.)

## Steps to Reproduce
1. Synthesize the CDK order-service application asset-free and deploy the resulting template through plain CloudFormation (CreateStack), passing AlarmName/MetricNamespace/table/queue/function/API names as parameters; wait for CREATE_COMPLETE and read the stack Outputs.
2. POST an order to the deployed REST API stage, confirm the DynamoDB row, confirm the SQS worker fulfils it, confirm the structured log lines, and confirm a datapoint for the custom metric (OrdersFulfilled) in the declared namespace — all of these pass today.
3. Call cloudwatch DescribeAlarms(AlarmNames=[<AlarmName passed to the stack>]).
   Expected: exactly one MetricAlarm with that AlarmName and the declared metric/period/threshold.
   Observed: MetricAlarms = [].
4. Poll DescribeAlarms for StateValue to reach ALARM (the datapoint is already over the threshold of 1) and then read DescribeAlarmHistory(AlarmName=..., HistoryItemType="StateUpdate").
   Expected: INSUFFICIENT_DATA -> ALARM and a StateUpdate history item recording it.
   Observed: StateValue reads None forever (no rows); polling exhausts ~424 s.
5. Delete the stack and re-check DescribeAlarms: the alarm must be gone (unobservable today, since it was never there).
