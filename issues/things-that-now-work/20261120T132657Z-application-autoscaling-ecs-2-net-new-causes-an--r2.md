Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits
Issue-Key: application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity|_poll_until#8|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch|_poll_until#11|AssertionError|raise AssertionError(f'{description} not observed within {timeout_s}s; last saw {last!r}')
Issue-Key: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits|run#44|AssertionError|assert _alarm_state(cw, in_alarm) == 'ALARM', (
Issue-Commit: 30f192be9b0f4575c60414230071fbe887bf0c81

# application-autoscaling__ecs: 2 net-new causes — an EC2 host with ECS_CLUSTER user data never registers as a container instance, and a CloudWatch alarm drops out of ALARM while its breaching datapoint is still the most recent non-missing one

> _Note: this issue was identified against the code base at commit `30f192be9b0f4575c60414230071fbe887bf0c81`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListContainerInstances

## Description
Two net-new root causes under the application-autoscaling__ecs heading. Both were reached by step-scaling checks that build a real EC2-launch-type ECS estate and drive it from CloudWatch alarms.

Cause 1 of 2: an EC2 instance launched from the ECS-optimized AMI, with an instance profile carrying AmazonEC2ContainerServiceforEC2Role and user data setting ECS_CLUSTER=<cluster>, never appears as a container instance of that cluster, so no EC2-launch-type capacity ever exists and every scaling leg downstream of it is unreachable. Failing checks: application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity, application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch.
AWS: Smithy (ecs) RegisterContainerInstance — the call the ECS agent on the instance makes, returning a ContainerInstance — and ListContainerInstances / DescribeContainerInstances, whose containerInstances rows carry ec2InstanceId, status and agentConnected. Recorded behavior: shortly after RunInstances, the agent reads /etc/ecs/ecs.config, registers with the named cluster, and the cluster lists exactly one container instance whose ec2InstanceId equals the launched instance id, with status 'ACTIVE' and agentConnected true; the service's tasks are then placed on it and runningCount converges to desiredCount.
Emulator: for the whole 240 s registration window, ListContainerInstances(cluster=<cluster>) returns no ARN whose DescribeContainerInstances row has ec2InstanceId equal to the launched instance; the probe's last observed value is None. RunInstances itself succeeded (an instance id was returned), as did CreateCluster, the VPC/subnet/IGW/route/SG setup, the role+instance profile, and GetParameter of the ECS-optimized AMI id. The cluster name and the instance id in the message are per-run values; the requirement is that the launched instance becomes a listable ACTIVE container instance of the cluster it names.
Why: every EC2-launch-type ECS workflow begins with container-instance registration. A client that waits for its host to join a cluster waits forever; EC2-launch-type services never get placement capacity, runningCount stays at 0, and because step scaling for ecs:service:DesiredCount reads the running baseline, no adjustment-type arithmetic (ChangeInCapacity / PercentChangeInCapacity / ExactCapacity) and no per-dimension suspension behavior can be observed at all.
Fix: the ECS container-instance registration path — the agent-side RegisterContainerInstance the emulator must synthesize for an instance launched with user data naming a cluster — and the ListContainerInstances / DescribeContainerInstances state it feeds. Note: an open issue describes this same cause under OTHER service headings and does NOT carry these checks' replay keys: 20261111T033241Z-ec2-ecs-events-sqs-an-ec2-instance-launched.md (ec2__ecs__events__sqs) and 20261113T122949Z-ec2-ecs-servicediscovery-runtask-ignores-container-instance-resource.md (a neighbouring ECS capacity cause).
Traces: application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity (_launch_container_instance, poll of ListContainerInstances/DescribeContainerInstances), application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch (same helper).

Cause 2 of 2: a metric alarm that has reached ALARM leaves ALARM once publishing stops, even though its single breaching datapoint is still the most recent non-missing datapoint in the evaluation range. Failing check: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits.
AWS: Smithy (cloudwatch) PutMetricAlarm.TreatMissingData — "Sets how this alarm is to handle missing data points... Valid Values: breaching | notBreaching | ignore | missing", pointing at "Configuring How CloudWatch Alarms Treats Missing Data"; DescribeAlarms.MetricAlarms[].StateValue reports the evaluated state. The alarm here is Period=10 (high resolution), EvaluationPeriods=1, DatapointsToAlarm=1, Statistic=Maximum, ComparisonOperator=LessThanThreshold, TreatMissingData=notBreaching. AWS evaluates an alarm over an attempted evaluation range wider than EvaluationPeriods and uses the most recent datapoints that are NOT missing; with one breaching datapoint and nothing published after it, the alarm therefore HOLDS StateValue='ALARM' rather than flipping on the absence of new data. Recorded behavior: the alarm stays in ALARM for the whole 60 s window in which no datapoint is published, and only leaves ALARM when a non-breaching datapoint is actually published.
Emulator: during that 60 s no-publish window, DescribeAlarms reports the scale-in alarm with a StateValue other than 'ALARM' — the check asserts StateValue == 'ALARM' on each 5 s read and fails with "the scale-in alarm <name> left ALARM during the suppression window". The alarm name in that message is a per-run value; the requirement is that an alarm whose most recent non-missing datapoint still breaches keeps StateValue='ALARM' until contradicting data arrives.
Why: alarm state is the trigger for every alarm-driven behavior. A client (or Application Auto Scaling itself) that relies on an alarm staying latched while a breach persists sees it un-latch purely because publishing paused: alarm actions stop being driven, cooldown/suppression semantics become unobservable, and any test or automation that waits on a sustained ALARM state gets the wrong answer. It also makes 'capacity did not change' unprovable, since an un-latched alarm explains the absence instead of the cooldown.
Fix: the CloudWatch alarm evaluation component — the missing-data handling in the alarm state machine that recomputes StateValue, which must apply TreatMissingData against an evaluation range that retains the most recent non-missing datapoints instead of resetting the state when the newest periods are empty — and the StateValue DescribeAlarms serializes from it. Note: 20261119T124426Z-application-autoscaling-cloudwatch-ec2-e-application-autoscaling-cloudwatch-ec2-ecs-sqs-a-step.md is about Application Auto Scaling re-firing from an alarm that merely STAYS in ALARM; it is a different cause and does not cover this one.
Traces: application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits (leg 2 suppression window, the per-read assert on DescribeAlarms StateValue).

## Evidence
Cause 1 of 2 — application-autoscaling__ecs.step-scaling-three-adjustment-types-from-one-capacity (_launch_container_instance line 335, _poll_until line 71):
  AssertionError: EC2 instance 'i-00000000000000001' registering as an ACTIVE ECS container instance on cluster 'sim2real-behav-aas-adj-cl-86d5b756fc37' not observed within 240.0s; last saw None

application-autoscaling__ecs.suspended-scaling-dimensions-are-independent-not-one-switch (_launch_container_instance line 347, _poll_until line 83):
  AssertionError: EC2 instance 'i-00000000000000001' registering as an ACTIVE ECS container instance on cluster 'sim2real-behav-aas-suspsub-cl-16cd3b8a0c3a' not observed within 240.0s; last saw None

Polled condition: ListContainerInstances(cluster=<cluster>) returns at least one ARN AND DescribeContainerInstances on those ARNs yields a row with ec2InstanceId == the launched instance id and status == 'ACTIVE'.
Expected: within the registration window the cluster lists exactly that one container instance, ACTIVE; the EC2-launch-type service then places tasks and runningCount converges.
Actual: the cluster lists no container instance for the full 240 s (probe value None). Instance ids and cluster names shown are per-run values.

Cause 2 of 2 — application-autoscaling__ecs.step-scaling-scale-in-cooldown-suppresses-then-permits (check line 479):
  AssertionError: leg 2: the scale-in alarm 'sim2real-behav-aas-coolinal-16efd46b584e' left ALARM during the suppression window, so an unchanged capacity would prove nothing about the cooldown

Sequence: PutMetricAlarm(Namespace='sim2real/behav/application-autoscaling', MetricName='CooldownDriver', Statistic='Maximum', Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=50.0, ComparisonOperator='LessThanThreshold', TreatMissingData='notBreaching', AlarmActions=[<scale-in policy ARN>]); PutMetricData is published repeatedly with Value=45.0 (below threshold) until DescribeAlarms reports StateValue='ALARM'; publishing then STOPS and DescribeAlarms is read every 5 s for 60 s.
Expected: StateValue stays 'ALARM' for the whole 60 s — the breaching datapoint is still the most recent non-missing datapoint, so notBreaching has no missing-only range to act on.
Actual: within that window DescribeAlarms reports the alarm no longer in 'ALARM'. The alarm name is a per-run value.

## Steps to Reproduce
Cause 1 of 2 (container-instance registration):
1. CreateCluster(clusterName=<cluster>). Create a VPC (10.94.0.0/16) with DNS support/hostnames, a public subnet with MapPublicIpOnLaunch, an IGW attached with a 0.0.0.0/0 route, and use the VPC's default security group.
2. CreateRole trusting ec2.amazonaws.com, AttachRolePolicy arn:aws:iam::aws:policy/service-role/AmazonEC2ContainerServiceforEC2Role, CreateInstanceProfile + AddRoleToInstanceProfile, then GetInstanceProfile for its ARN.
3. GetParameter /aws/service/ecs/optimized-ami/amazon-linux-2023/recommended/image_id and RunInstances that AMI (t3.medium, MinCount=MaxCount=1) in the subnet with that instance profile and UserData (base64) '#!/bin/bash\necho ECS_CLUSTER=<cluster> >> /etc/ecs/ecs.config\n'.
4. Poll ListContainerInstances(cluster=<cluster>) and DescribeContainerInstances on the returned ARNs for a row with ec2InstanceId == the launched instance id and status == 'ACTIVE'.
   Expected: the instance registers and is reported ACTIVE.
   Emulator: no container instance is ever listed for the cluster (240 s).
5. (Blocked by step 4) RegisterTaskDefinition (networkMode=bridge, requiresCompatibilities=['EC2']), CreateService(launchType='EC2', desiredCount=4), RegisterScalableTarget for ecs:service:DesiredCount and drive PutScalingPolicy StepScaling legs with ChangeInCapacity / PercentChangeInCapacity / ExactCapacity, or suspend only DynamicScalingInSuspended and confirm the other dimensions still act.

Cause 2 of 2 (alarm leaves ALARM with no new data):
1. PutMetricAlarm(AlarmName=<name>, Namespace='sim2real/behav/application-autoscaling', MetricName='CooldownDriver', Dimensions=[...], Statistic='Maximum', Period=10, EvaluationPeriods=1, DatapointsToAlarm=1, Threshold=50.0, ComparisonOperator='LessThanThreshold', TreatMissingData='notBreaching', AlarmActions=[<Application Auto Scaling step-policy ARN>]).
2. PutMetricData for that metric with Value=45.0 (StorageResolution=1) every 5 s until DescribeAlarms(AlarmNames=[<name>]) reports MetricAlarms[0].StateValue == 'ALARM'.
3. STOP publishing. Read DescribeAlarms(AlarmNames=[<name>]) every 5 s for 60 s.
   Expected: StateValue remains 'ALARM' throughout (the breaching datapoint is still the most recent non-missing datapoint in the evaluation range).
   Emulator: StateValue leaves 'ALARM' inside that window.
