Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-actions-disabled-notification-blocked|_poll_until#27|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Key: apigatewayv2__cloudwatch.obs-apigatewayv2-3-http-api-5xx-alarm-notifies-sqs-via-sns|_poll_until#27|Timeout|raise AssertionError(f"{description} not met within {timeout_s}s. Last result: {last!r}")
Issue-Commit: 67bb1f0baca61c1b5b95e0062b2898063e06b421

# apigatewayv2__cloudwatch: the HTTP-API 5xx alarm pipeline never reaches a verdict — both 5xx alarm checks hang to the harness cap

> _Note: this issue was identified against the code base at commit `67bb1f0baca61c1b5b95e0062b2898063e06b421`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutMetricAlarm

## Description
Cause 1 of 1: a deployed HTTP API's `5xx` series under AWS/ApiGateway never materialises, so neither an enabled alarm nor its actions-disabled twin ever evaluates, and both checks run out the harness budget with no verdict.

AWS: a deployed HTTP API stage with an AWS_PROXY integration serves the route at the stage invoke URL; an integration answering HTTP 500 is counted into the AWS/ApiGateway namespace as metric `5xx` dimensioned {ApiId, Stage} (and under the same dimensions for the stage totals). An alarm declared on that series with Namespace=AWS/ApiGateway / MetricName=5xx / Dimensions=[ApiId, Stage] transitions to ALARM once the configured datapoints breach, and CloudWatch then runs its AlarmActions (here an SNS topic subscribed by an SQS queue). An alarm created with ActionsEnabled=false still EVALUATES and still records state transitions in DescribeAlarmHistory, it simply does not execute the actions; after EnableAlarmActions the same alarm notifies on its next transition. Smithy (cloudwatch): PutMetricAlarm.ActionsEnabled (boolean), AlarmActions, EnableAlarmActions; the metric series and the API-Gateway dimension names are recorded behavior only.

Emulator: both checks are reported `mismatch` with `Error: Check timed out after 60.0s`. Nothing in the chain reaches its observable within the budget: the stage drives its requests but no `5xx` datapoints appear under {ApiId, Stage}, so neither alarm ever leaves INSUFFICIENT_DATA, no state transition is written to alarm history, and no notification is ever enqueued on the subscribing SQS queue. The suppressed-vs-enabled distinction the checks exist to prove therefore cannot be observed at all — the absence of a notification is indistinguishable from a pipeline that never ran.

Why: a client that builds API-Gateway observability on this emulator sees a permanently INSUFFICIENT_DATA alarm on real 5xx traffic, so error alarms never fire and ActionsEnabled is untestable. Any dashboard, runbook or auto-remediation keyed on AWS/ApiGateway 5xx is silently dead, and a test asserting "no notification" passes for the wrong reason.

Fix: the apigatewayv2 execute-api data plane's request accounting (the stage request path that must emit AWS/ApiGateway metrics) and the cloudwatch alarm evaluator that must turn those datapoints into a state transition and run/suppress AlarmActions. Related open issues describe neighbouring symptoms but do not carry these two checks' replay keys: 20261024T120951Z-apigatewayv2-cloudwatch-no-http-api-request-metrics-can.md and 20261029T182452Z-cloudwatch-apigatewayv2-http-api-returns-its-own-generic-message-internal.md.

Traces: the two failing checks listed under `### apigatewayv2__cloudwatch`, both timing out at the 60s cap.

## Evidence
Expected: after N requests to `GET /boom` on the deployed `live` stage whose AWS_PROXY integration returns statusCode 500, the AWS/ApiGateway `5xx` series under {ApiId=<api id>, Stage=live} accumulates one datapoint per request; the enabled alarm (ActionsEnabled=true) transitions OK/INSUFFICIENT_DATA -> ALARM, writes that transition to DescribeAlarmHistory and publishes to its SNS AlarmAction, which the subscribed SQS queue receives; the twin alarm with ActionsEnabled=false performs the same evaluation and records the same transition but publishes nothing, and after EnableAlarmActions it publishes on its next transition.

Actual: both checks are reported `[mismatch] ... Error: Check timed out after 60.0s`. No alarm state transition and no queue message is ever observed; the checks never reach their assertions, so the emulator produces no verdict for either arm.

## Steps to Reproduce
1. Create an HTTP API with an AWS_PROXY integration to a Lambda returning statusCode 500 and a route `GET /boom`; create an auto-deploy stage `live` and allow apigateway to invoke the function.
2. Create an SNS topic subscribed by an SQS queue.
3. PutMetricAlarm twice on Namespace=AWS/ApiGateway, MetricName=5xx, Dimensions=[{ApiId},{Stage=live}] — one with ActionsEnabled=true and one with ActionsEnabled=false, both with the SNS topic as AlarmActions.
4. Issue several GET requests to `<invoke url>/boom`.
5. Poll GetMetricStatistics/DescribeAlarms/DescribeAlarmHistory for the 5xx datapoints and the ALARM transition, and ReceiveMessage on the queue.
6. Observe that no datapoint, no transition and no notification ever appears; the check exhausts its budget (60s harness cap).
