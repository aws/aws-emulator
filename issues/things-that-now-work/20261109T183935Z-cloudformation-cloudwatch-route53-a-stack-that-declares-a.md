Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-cloudwatch-metric-round-trip-and-delete cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected
Issue-Key: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-cloudwatch-metric-round-trip-and-delete|run#12|AssertionError|assert got == [{"AlarmName": out["AlarmRef"], "MetricName": metric_name, **_ALARM}], (
Issue-Key: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected|run#15|AssertionError|assert alarms == [out["AlarmRef"]] == [good_alarm], f"control alarm {alarms!r}"
Issue-Commit: c33038d6637516191c9271b81207503148b3a194

# cloudformation__cloudwatch__route53: a stack that declares a Route 53 health check and an alarm over AWS/Route53 HealthCheckStatus creates no alarm at all — DescribeAlarms is empty after CREATE_COMPLETE

> _Note: this issue was identified against the code base at commit `c33038d6637516191c9271b81207503148b3a194`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Cause 1 of 1: an AWS::CloudWatch::Alarm declared alongside an AWS::Route53::HealthCheck is not created when the stack deploys, so both the round-trip arm and the invalid-InsufficientDataHealthStatus rejection arm read an empty DescribeAlarms (the second one cannot even reach its negative assertion because its own positive control alarm is missing).

AWS: CloudFormation provisions each AWS::CloudWatch::Alarm in a template as a real PutMetricAlarm, so after the stack reports CREATE_COMPLETE DescribeAlarms(AlarmNames=[<the deployed name>]) returns exactly one MetricAlarm, and an alarm over Namespace=AWS/Route53, MetricName=HealthCheckStatus, Dimensions=[{HealthCheckId}] evaluates the health checker's series. Smithy (cloudwatch 2010-08-01): DescribeAlarms returns MetricAlarms; PutMetricAlarm is the write CloudFormation performs.

Emulator: the stack settles, but DescribeAlarms for the deployed alarm returns [] — the alarm resource is never written into CloudWatch state.

Why: a client that deploys monitoring as part of its stack gets a stack that claims success and no monitoring. Nothing alarms on health-check failure, and 'the alarm exists' assertions that gate later operations (update, delete, rollback) all read an absent resource.

Fix: the CloudFormation AWS::CloudWatch::Alarm resource handler — it must perform the PutMetricAlarm against CloudWatch state during stack create (and the matching delete on stack delete). This is the same class of defect as the open issue 20261025T020208Z-cloudwatch-cfn-deployed-alarms-are-never-created-the.md, which does not carry these two checks' replay keys.

Traces: cloudformation__cloudwatch__route53.deploy-route53-healthcheck-cloudwatch-metric-round-trip-and-delete, cloudformation__cloudwatch__route53.deploy-route53-healthcheck-invalid-insufficient-data-status-rejected.

## Evidence
Expected: exactly one MetricAlarm from DescribeAlarms for the stack-deployed alarm name after CREATE_COMPLETE.
Actual: 'AssertionError: DescribeAlarms []' and, in the rejection arm, 'AssertionError: control alarm []'.

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::Route53::HealthCheck and an AWS::CloudWatch::Alarm over AWS/Route53 HealthCheckStatus dimensioned on the health check's id.
2. Wait for CREATE_COMPLETE.
3. Call cloudwatch DescribeAlarms(AlarmNames=[<deployed alarm name>]).
4. Observe MetricAlarms == [].
