Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity
Issue-Key: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity|run#39|AssertionError|assert repaired[0]['StatusCode'] == 'InProgress', (
Issue-Commit: 1cb2c89e78a43c5f64deffffb0f79f2010a0d95b

# application-autoscaling__ecs: a scaling activity is marked terminal 'Successful' the moment the engine sets desiredCount, even when ECS has not fulfilled (and cannot fulfil) the change

> _Note: this issue was identified against the code base at commit `1cb2c89e78a43c5f64deffffb0f79f2010a0d95b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeScalingActivities

## Description
Cause 1 of 1: the emulator's scaling-activity ledger writes the TERMINAL status 'Successful' as soon as Application Auto Scaling hands the new capacity to the target dimension, instead of keeping the activity in the in-flight state until the target service actually realises that capacity. Failing check: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity.

AWS: Smithy (application-autoscaling) — ScalingActivity.StatusCode is required and targets the enum ScalingActivityStatusCode with values [Pending, InProgress, Successful, Overridden, Unfulfilled, Failed]; ScalingActivity.StatusMessage is 'A simple message about the current status of the scaling activity' and ScalingActivity.EndTime is 'The Unix timestamp for when the scaling activity ended'. The model states the enum but not the transition rule, so for the lifecycle this is recorded behavior only: after a step policy applies a ChangeInCapacity adjustment to an ecs:service:DesiredCount target, the activity reports StatusCode 'InProgress' with StatusMessage 'Successfully set desired count to <N>. Waiting for change to be fulfilled by ecs.' — fixed text is that sentence pair, and <N> is the capacity the adjustment requested, taken from the request/state. It stays InProgress (no EndTime) for as long as ECS has not fulfilled the new desiredCount. On the estate this check builds — an ECS cluster with NO container instances, EC2 launch type — ECS can never place the tasks, so AWS never moves the activity to any terminal state at all; 'Successful' means the requested capacity was realised, and 'Unfulfilled'/'Failed' are the terminal states for a change that was not.

Emulator: immediately after the repaired alarm fired and desiredCount moved from 6 to 3, DescribeScalingActivities (IncludeNotScaledActivities=True) returned the newest activity as StatusCode 'Successful' with Description 'Setting desired count to 3.' — a terminal success on an estate where no task can ever be placed. The activity's status is derived from the engine's own write, not from the ECS dimension's realised capacity.

Why: a client cannot distinguish 'the capacity change landed' from 'Application Auto Scaling asked for it and nothing happened'. Deployment gates, CI checks and runbooks that poll DescribeScalingActivities until StatusCode is terminal and then treat 'Successful' as proof the fleet resized get a false positive on a target that never scaled; conversely nothing is ever reported as Unfulfilled/Failed, so a genuinely stuck estate (no capacity to place tasks on) looks healthy and alarming/retry logic built on the activity ledger never triggers.

Fix: the application-autoscaling scaling-activity state machine (the activity ledger in the scaling engine) — the transition out of InProgress must be gated on the target dimension reporting the requested capacity as realised, stamping EndTime and 'Successful' only then, and 'Unfulfilled'/'Failed' when the change was not achieved; while the change is outstanding it must stay InProgress with the 'Waiting for change to be fulfilled by ecs.' StatusMessage.

Related open issues that do NOT cover this and in fact describe the opposite polarity (the activity never reaching 'Successful'): 20261123T142820Z-application-autoscaling-ecs-a-scaling-activity-that-really.md and 20261125T221450Z-application-autoscaling-ecs-2-net-new-causes-scaling.md (Cause 1). This check was not marked owned by either; it is grounded in the terminal status being written WITHOUT regard to fulfilment.

Traces: application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity, leg 3 (assertion on repaired[0]['StatusCode']).

## Evidence
Check application-autoscaling__ecs.a-metric-that-never-breaches-produces-no-scaling-activity
Expected (AWS): after the subject's alarm threshold is raised so the published value breaches and the StepScaling policy applies ChangeInCapacity -3, the newest entry from DescribeScalingActivities(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', IncludeNotScaledActivities=True) reports StatusCode 'InProgress', Description 'Setting desired count to <N>.' and StatusMessage 'Successfully set desired count to <N>. Waiting for change to be fulfilled by ecs.', with no EndTime, because the cluster has no container instances and ECS can never fulfil the new desiredCount.
Actual (emulator): the newest activity is ('Successful', 'Setting desired count to 3.') — a terminal success.
AssertionError: "leg 3: the subject's newest activity is [('Successful', 'Setting desired count to 3.')], expected InProgress -- this estate has no container instances, so ECS can never finish fulfilling the new desiredCount and the activity cannot reach a different terminal state"
Fixed text vs values: the requirement is the StatusCode value 'InProgress' and the StatusMessage template 'Successfully set desired count to <N>. Waiting for change to be fulfilled by ecs.'; the 3 (and the 6 it came down from) are this run's capacities, computed from MinCapacity 1 / start count 6 / adjustment -3, and are not part of the requirement. Earlier legs of the check PASS: the never-breaching subject produced no activity at all, the control scaled in on the same value and wiring, and the repaired subject's Description matched exactly — only the status lifecycle diverges.
Model citation used: ScalingActivity.StatusCode required -> ScalingActivityStatusCode enum [Pending, InProgress, Successful, Overridden, Unfulfilled, Failed]; ScalingActivity.StatusMessage optional; ScalingActivity.EndTime optional timestamp 'for when the scaling activity ended'.

## Steps to Reproduce
1. CreateCluster (no container instances ever registered), RegisterTaskDefinition (networkMode 'bridge', requiresCompatibilities ['EC2']), CreateService(launchType='EC2', desiredCount=6). Confirm DescribeServices reports desiredCount 6.
2. RegisterScalableTarget(ServiceNamespace='ecs', ResourceId='service/<cluster>/<service>', ScalableDimension='ecs:service:DesiredCount', MinCapacity=1, MaxCapacity=40).
3. PutScalingPolicy(PolicyType='StepScaling', AdjustmentType='ChangeInCapacity', MetricAggregationType='Maximum', Cooldown=0, StepAdjustments=[{MetricIntervalUpperBound: 0.0, ScalingAdjustment: -3}]).
4. PutMetricAlarm(Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, ComparisonOperator='LessThanThreshold', Threshold=90.0, TreatMissingData='notBreaching', AlarmActions=[policy ARN]) and PutMetricData the value 50.0 every 5 s so the alarm goes to ALARM.
5. Poll DescribeServices until desiredCount drops below 6, then call DescribeScalingActivities(..., IncludeNotScaledActivities=True) and read the newest activity's StatusCode/StatusMessage/EndTime.
6. AWS: StatusCode 'InProgress', StatusMessage 'Successfully set desired count to <N>. Waiting for change to be fulfilled by ecs.', no EndTime, and it stays that way because no container instance exists to place the tasks. Emulator: StatusCode 'Successful' right away.
