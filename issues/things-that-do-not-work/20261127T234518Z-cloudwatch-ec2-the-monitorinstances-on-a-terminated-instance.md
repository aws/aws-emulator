Issue-Severity: HIGH
Issue-Commit: 5cb989982ca164b028438556d2b2a85d4de8b473

# cloudwatch__ec2: the MonitorInstances-on-a-terminated-instance scenario reaches no verdict — the run is killed at its 600 s budget because the twin's 1-minute AWS/EC2 CPUUtilization datapoint is never published

> _Note: this issue was identified against the code base at commit `5cb989982ca164b028438556d2b2a85d4de8b473`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MonitorInstances

## Description
Cause 1 of 1: the whole MonitorInstances refusal scenario never converges on this emulator — the check is killed at its 600 s budget before it can read any verdict, so neither the refusal on a terminated instance nor the effect of the positive control on its running twin is ever observed. Failing check: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.

AWS: Smithy (ec2) MonitorInstances takes InstanceIds and answers InstanceMonitorings with Monitoring.State per instance; on a terminated instance AWS refuses it with error code `InvalidState` and the message `Instance is not in a supported state.` (fixed text in full; no variable part), leaving Monitoring.State at `disabled`. On a running instance the identical call answers Monitoring.State `pending` and settles at `enabled`, and from that moment AWS/EC2 CPUUtilization for that InstanceId is published at 1-minute resolution — datapoints that fall OFF the 5-minute grid (epoch seconds not a multiple of 300) — while a basic-monitoring instance publishes only on the 5-minute grid. A terminated instance publishes no further datapoint, and the datapoints published before termination stay readable through GetMetricStatistics (metric history outlives the instance). The model declares no modeled error member for MonitorInstances, so the refusal code/message above is recorded/documented AWS behavior.

Emulator: the check never reaches any of those assertions. The replay ends with `Check timed out after 600.0s`, i.e. one of the two convergence waits the scenario is built on never completes: the wait for the doomed instance's first CPUUtilization datapoint, or the wait for the live twin to publish a 1-minute, off-5-minute-grid CPUUtilization datapoint at or after the next 5-minute barrier once MonitorInstances has been accepted. Hypothesis (the replay output states only the timeout): the stall is the off-grid 1-minute datapoint — the emulator never publishes AWS/EC2 CPUUtilization at 1-minute resolution after MonitorInstances is accepted, so that predicate can never become true and the run burns its budget.

Why: a client cannot tell from this emulator whether MonitorInstances was refused on a terminated instance at all, whether a refusal left Monitoring.State untouched, or whether enabling detailed monitoring actually changed the published metric resolution. Worse, every caller that waits for AWS's documented effect of MonitorInstances (a 1-minute datapoint appearing) blocks forever rather than failing, so monitoring/alarm automation built on detailed monitoring hangs instead of converging.

Fix: the AWS/EC2 metric publication for EC2 instances (the CloudWatch metric feed driven by instance state and the per-instance monitoring flag) plus the MonitorInstances handler's state validation in the ec2 service: accepting MonitorInstances on a running instance must switch that instance's CPUUtilization publication to 1-minute buckets (off the 5-minute grid), a terminated instance must publish nothing further while its earlier buckets stay readable, and MonitorInstances on a terminated instance must be refused `InvalidState` / `Instance is not in a supported state.` without changing Monitoring.State.

Related but not covering this: open issues 20261124T094017Z-cloudwatch-ec2-monitorinstances-on-a-running-instance-never.md, 20261126T034854Z-cloudwatch-ec2-unmonitorinstances-still-cannot-be-shown-to.md and 20261125T150300Z-cloudwatch-ec2-a-basic-monitoring-instance-publishes-cpuutilization.md describe neighbouring monitoring-resolution defects in the same service, and 20261111T064629Z-cloudwatch-ec2-ssm-the-nat-gateway-instance-network-metric-scenario-never.md is the same never-converges shape in another scenario. None of them carries this check's replay key, and none covers the terminated-instance refusal beat this scenario exists to assert.

Traces: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.

## Evidence
Failing check cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected [mismatch].

  Error: Check timed out after 600.0s

The scenario launches two basic-monitoring t3.micro instances in a private VPC of its own, waits for both to be `running`, waits for the doomed instance's first AWS/EC2 CPUUtilization datapoint (GetMetricStatistics, Namespace=AWS/EC2, MetricName=CPUUtilization, Dimensions=[InstanceId], Period=60), terminates it, and only then makes the calls it asserts on:

  refusal = ec2.monitor_instances(InstanceIds=[doomed_id])
  want = ("InvalidState", "Instance is not in a supported state.")
  ... assert refusal == want
  assert _describe(ec2, doomed_id)["Monitoring"]["State"] == "disabled"
  resp = ec2.monitor_instances(InstanceIds=[live_id])   # positive control -> pending/enabled
  _poll_until(... "twin publishing a 1-minute (off-5-minute-grid) CPUUtilization datapoint at or after the barrier" ...)

Expected: the refusal `('InvalidState', 'Instance is not in a supported state.')` with Monitoring.State still `disabled`; the identical call on the running twin reporting `pending`/`enabled`; a CPUUtilization datapoint whose epoch second is NOT a multiple of 300 appearing after the switch; no CPUUtilization bucket at or after the doomed instance's termination; and the doomed instance's pre-termination buckets still readable.

Actual: no assertion is reached — the run is killed at 600 s with no answer, so the emulator never converges to the state the scenario needs. The 600 s figure is the harness budget and the instance ids/timestamps are per-run values; the requirement is that the scenario's reads converge, not any particular timing.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is serving.
2. Re-replay cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.
3. Manually: create a VPC + subnet + security group, RunInstances twice with Monitoring={'Enabled': False} on t3.micro from the latest Amazon-owned AL2023 x86_64 AMI, and wait for both to reach `running`.
4. Poll GetMetricStatistics(Namespace='AWS/EC2', MetricName='CPUUtilization', Dimensions=[{'Name':'InstanceId','Value':<doomed>}], Period=60) until the first datapoint exists — on the emulator this and/or step 7 never completes.
5. TerminateInstances on the first instance and wait for `terminated`.
6. Call MonitorInstances(InstanceIds=[<terminated id>]): it must be refused InvalidState / 'Instance is not in a supported state.' and DescribeInstances must still report Monitoring.State='disabled'.
7. Call MonitorInstances(InstanceIds=[<running twin>]) and poll GetMetricStatistics at Period=60 for a datapoint whose timestamp is not on the 5-minute grid; it must appear after the next 5-minute barrier.
8. Re-read the terminated instance's series: no bucket at or after termination, and every pre-termination bucket still present.
