Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__cloudwatch.cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm cloudformation__cloudwatch.cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm cloudformation__cloudwatch.cfn-cloudwatch-alarm-nested-required-members-rejected
Issue-Key: cloudformation__cloudwatch.cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm|run#19|IndexError|composite_desc = cw.describe_alarms(AlarmNames=[composite], AlarmTypes=["CompositeAlarm"])[
Issue-Key: cloudformation__cloudwatch.cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm|run#19|IndexError|composite_desc = cw.describe_alarms(AlarmNames=[composite], AlarmTypes=["CompositeAlarm"])[
Issue-Key: cloudformation__cloudwatch.cfn-cloudwatch-alarm-nested-required-members-rejected|run#9|AssertionError|assert status == "CREATE_COMPLETE", f"control stack: expected CREATE_COMPLETE, got {status!r}"
Issue-Commit: dbbfc43f271727daf6a36bf70721e9eb6030bb1c

# cloudformation__cloudwatch: 2 net-new causes — a CFN/CDK-deployed AWS::CloudWatch::CompositeAlarm is invisible to DescribeAlarms, and a conforming metric-math AWS::CloudWatch::Alarm stack rolls back

> _Note: this issue was identified against the code base at commit `dbbfc43f271727daf6a36bf70721e9eb6030bb1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
Two independent root causes in the CloudFormation/CloudWatch alarm path. Both are net-new: no open issue carries either replay key (the closest open items, 20261110T232635Z-cloudwatch-ecs-ssm-a-cloudformation-created-aws-cloudwatch-alarm-is-invisible.md and 20261108T001650Z-cloudwatch-3-net-new-causes-a-composite-alarmrule.md, are about metric alarms and about AlarmRule evaluation, not about these two).

Cause 1 of 2: a CompositeAlarm created through CloudFormation/CDK is not returned by DescribeAlarms when the caller filters for composite alarms.
AWS:      Smithy (cloudwatch): DescribeAlarms input carries `AlarmNames` and `AlarmTypes` (list of AlarmType); its output carries a separate `CompositeAlarms` list beside `MetricAlarms` and `LogAlarms`. A composite alarm that exists answers DescribeAlarms(AlarmNames=[<name>], AlarmTypes=["CompositeAlarm"]) with exactly one row in `CompositeAlarms` carrying that AlarmName, its AlarmRule and its StateValue. There is no modeled error for a name that matches nothing — the list is simply empty, which is why a caller indexing [0] is entitled to a row for an alarm it just created.
Emulator: for a composite alarm the stack reports created, DescribeAlarms(AlarmNames=[<composite>], AlarmTypes=["CompositeAlarm"]) answers 200 with an EMPTY `CompositeAlarms` list, so the checks fail at `IndexError: list index out of range` on the first read-back of the alarm. The composite alarm is therefore either never created by the resource handler or never indexed under the CompositeAlarm type.
Why:      a client that provisions a composite alarm (AND/OR rollup over child metric alarms) and then reads its state back — exactly what every alarm-rollup dashboard, runbook and CDK assertion does — sees no alarm at all. Nothing downstream of the rollup (state transitions, suppression, actions) can ever be observed, and a deploy-then-verify pipeline reports the alarm as missing even though the stack is CREATE_COMPLETE.
Fix:      the AWS::CloudWatch::CompositeAlarm resource handler (its create path must reach PutCompositeAlarm in the cloudwatch service) and cloudwatch's DescribeAlarms handler/state: the composite-alarm registry must be populated and must be returned under `CompositeAlarms` when `AlarmTypes` includes "CompositeAlarm".
Traces:   cloudformation__cloudwatch.cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm, cloudformation__cloudwatch.cdk-deploy-composite-alarm-or-rule-reflects-either-child-alarm.

Cause 2 of 2: a valid metric-math AWS::CloudWatch::Alarm stack reaches ROLLBACK_COMPLETE instead of CREATE_COMPLETE.
AWS:      recorded behavior only for the CloudFormation resource schema; the properties used are all modeled CloudWatch concepts — an alarm declared with ComparisonOperator, EvaluationPeriods, Threshold, a `Metrics` list of one MetricDataQuery (`Id`, `ReturnData`, `MetricStat` with `Metric`+`Period`+`Stat`) and `ActionConfiguration.AlarmTagsPropagation.Enabled` is a conforming alarm. AWS creates it: the stack reaches CREATE_COMPLETE and DescribeAlarms(AlarmNames=[<name>]) returns it in `MetricAlarms`. This is the check's positive control, deployed before the three negative cases.
Emulator: the control stack settles at ROLLBACK_COMPLETE, so the valid alarm is never created and the check stops before it can exercise the refusals at all.
Why:      a client cannot deploy a metric-math alarm with the properties AWS accepts; the whole template is rolled back, and every resource that depends on the alarm (composite alarms, autoscaling policies, SNS wiring) is torn down with it.
Fix:      the AWS::CloudWatch::Alarm resource handler's property validation and create path — a `Metrics`/`MetricStat` alarm with an `ActionConfiguration` must validate and provision rather than fail; the per-resource failure reason recorded on the rollback event names which property the handler refused.
Traces:   cloudformation__cloudwatch.cfn-cloudwatch-alarm-nested-required-members-rejected.

## Evidence
Cause 1 — cdk-deploy-composite-alarm-and-rule-requires-both-children-in-alarm (and the ...-or-rule-... sibling):
  Expected: DescribeAlarms(AlarmNames=[<composite>], AlarmTypes=["CompositeAlarm"])["CompositeAlarms"] == [ {AlarmName: <composite>, ...} ] (one row).
  Actual:   CompositeAlarms == [] — the check raises
    File "check_cloudformation__cloudwatch_cdk_deploy_composite_alarm_and_rule_requires_both_children_in_alarm.py", line 88, in run
    composite_desc = cw.describe_alarms(AlarmNames=[composite], AlarmTypes=["CompositeAlarm"])[...]
    IndexError: list index out of range

Cause 2 — cfn-cloudwatch-alarm-nested-required-members-rejected:
  Expected: the control stack, whose single AWS::CloudWatch::Alarm declares
    ComparisonOperator=GreaterThanThreshold, EvaluationPeriods=1, Threshold=90,
    Metrics=[{Id: m1, ReturnData: true, MetricStat: {Metric: {Namespace, MetricName}, Period: 300, Stat: Average}}],
    ActionConfiguration={AlarmTagsPropagation: {Enabled: true}}
  reaches CREATE_COMPLETE and the alarm reads back through DescribeAlarms.
  Actual:   AssertionError: control stack: expected CREATE_COMPLETE, got 'ROLLBACK_COMPLETE'

(Stack names, alarm names and ids in the recordings are per-run values; the requirement is the status and the presence of the row, not the generated names.)

## Steps to Reproduce
Cause 1:
1. Deploy a stack (or CDK app) containing two AWS::CloudWatch::Alarm metric alarms and one AWS::CloudWatch::CompositeAlarm whose AlarmRule combines them (ALARM(a) AND ALARM(b), and separately OR).
2. Wait for CREATE_COMPLETE.
3. Call cloudwatch DescribeAlarms(AlarmNames=[<composite name>], AlarmTypes=["CompositeAlarm"]).
4. Observe CompositeAlarms == [] instead of one row for the composite alarm.

Cause 2:
1. CreateStack with one AWS::CloudWatch::Alarm whose Properties are ComparisonOperator=GreaterThanThreshold, EvaluationPeriods=1, Threshold=90, Metrics=[{Id:"m1",ReturnData:true,MetricStat:{Metric:{Namespace,MetricName},Period:300,Stat:"Average"}}], ActionConfiguration={AlarmTagsPropagation:{Enabled:true}}, AlarmName=<name>.
2. Poll DescribeStacks until terminal.
3. Observe ROLLBACK_COMPLETE instead of CREATE_COMPLETE, and no alarm from DescribeAlarms(AlarmNames=[<name>]).
