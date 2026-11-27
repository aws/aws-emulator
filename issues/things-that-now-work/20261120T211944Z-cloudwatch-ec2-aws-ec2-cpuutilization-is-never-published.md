Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected
Issue-Key: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: 4bb76b5ad778511f4e613dac77ae9e1eefd9f644

# cloudwatch__ec2: AWS/EC2 CPUUtilization is never published for a newly launched instance on basic monitoring, so GetMetricStatistics returns an empty series forever and the MonitorInstances-on-terminated-instance check never reaches a verdict

> _Note: this issue was identified against the code base at commit `4bb76b5ad778511f4e613dac77ae9e1eefd9f644`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
One net-new root cause (Cause 1 of 1): a freshly launched, running EC2 instance on BASIC monitoring never gets a single AWS/EC2 CPUUtilization datapoint into CloudWatch, so a client polling for its first datapoint polls forever.

AWS: recorded behavior only for the metric cadence — Smithy ec2#MonitorInstances and ec2#UnmonitorInstances both declare `errors: []` and the EC2 model says nothing about metric emission, and cloudwatch#GetMetricStatistics models no error for an empty series (it answers 200 with `Datapoints`). The recording requires that an instance launched with `Monitoring={"Enabled": false}` (basic monitoring) begins publishing AWS/EC2 CPUUtilization under the single dimension `InstanceId=<instance id>` shortly after it reaches `running`: GetMetricStatistics(Namespace=`AWS/EC2`, MetricName=`CPUUtilization`, Dimensions=[{InstanceId: <id>}], Period=60, Statistics=[Average, SampleCount]) returns at least one datapoint, on the 5-minute grid, well inside a 10-minute window. That history must then outlive the instance (it is still readable after termination). The variable parts here are the instance id and the timestamps, which come from the request and from when the instance launched; the requirement is that a non-empty series exists at all, not any particular value.

Emulator: the series stays empty. The check `metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected` is cut by the harness at its hard budget with `Error: Check timed out after 600.0s` and is never graded. The only long-running poll it can be sitting in at that point is the first-datapoint poll — `_poll_until(lambda: _series(cw, doomed_id, launched_at), lambda d: len(d) >= 1, timeout_s=1200.0, interval_s=30.0, description="instance <id> publishing its first CPUUtilization datapoint on basic monitoring")` — because every earlier poll in the check has its own 180–300 s budget and raises an AssertionError (not a harness timeout) when it expires. Hypothesis: the EC2 -> CloudWatch metric-emission path publishes no AWS/EC2 CPUUtilization series for this instance at all, so GetMetricStatistics keeps answering 200 with `Datapoints: []`.

Note this is a DIFFERENT cause from the open issue 20261116T091224Z-cloudwatch-ec2-2-net-new-causes-monitorinstances-on.md, which lists this check id and whose replay key for it is the AssertionError at `assert refusal == want` (MonitorInstances answering `IncorrectInstanceState` instead of `InvalidState`). That issue's own evidence states this check "got past its basic-monitoring first-datapoint poll", i.e. 5-minute publication used to work while only the 1-minute cadence was missing. It no longer gets past it: the check now dies before MonitorInstances is ever called, so basic (5-minute) publication is now broken too and is not covered by that issue's key.

Why: a client that waits for an instance's first CPUUtilization datapoint before proceeding — console graphs, autoscaling/target-tracking warm-up, any CloudWatch alarm on AWS/EC2 CPUUtilization — never advances. Alarms stay INSUFFICIENT_DATA indefinitely instead of transitioning, GetMetricStatistics and GetMetricData return empty result sets for a live instance, and ListMetrics/metric-history reads after termination have nothing to return. It also masks every downstream assertion in this check (the MonitorInstances refusal classification, the Monitoring.State invariants, and the "terminated instance publishes nothing but keeps its history" invariants), none of which can be graded.

Fix: the EC2 service's instance metric-emission component that feeds the AWS/EC2 namespace into CloudWatch — a running instance must register and publish a CPUUtilization series under dimension InstanceId on the basic 5-minute cadence from the moment it reaches `running`, independent of Monitoring.State, and CloudWatch's GetMetricStatistics read path must surface those datapoints for Period=60 queries.

Traces: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.

## Evidence
Failing check: [mismatch] cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected
Reported detail: `Error: Check timed out after 600.0s` (no traceback — the harness cut the check while it was still inside a poll).

Expected (AWS): after both t3.micro instances reach `running`, cloudwatch:GetMetricStatistics(Namespace='AWS/EC2', MetricName='CPUUtilization', Dimensions=[{'Name':'InstanceId','Value':<id>}], StartTime=launch-10min, EndTime=now+1min, Period=60, Statistics=['Average','SampleCount']) returns `Datapoints` with at least one entry within the check's 1200 s budget; the check then terminates one instance and grades ec2:MonitorInstances on it (expecting error code `InvalidState`, message exactly `Instance is not in a supported state.` — all fixed text, no interpolated values) plus Monitoring.State still `disabled`, and finally re-reads the pre-termination datapoints to confirm the history survived.

Actual (emulator): the first-datapoint poll never satisfies `len(d) >= 1`. The 600 s harness budget expires inside it, so the check produces no verdict and none of the later assertions run. Every earlier step in the check is bounded by its own shorter budget (180 s / 300 s) and would have surfaced as an AssertionError with a traceback rather than a harness timeout, which localises the hang to the CPUUtilization poll.

Corroborating: the sibling check cloudwatch__ec2.unmonitor-instances-restores-five-minute-cpu-publication-after-monitor-instances in the same service fails with the identical `Error: Check timed out after 600.0s` (already owned by 20261116T091224Z-cloudwatch-ec2-2-net-new-causes-monitorinstances-on.md), consistent with the AWS/EC2 CPUUtilization series not being published at all.

Smithy cited: ec2#MonitorInstances — input {InstanceIds (required, list), DryRun}, output {InstanceMonitorings}, errors: []; ec2#UnmonitorInstances — same shape, errors: []. Neither models anything about metric cadence, hence "recorded behavior only" for the emission requirement.

## Steps to Reproduce
1. Replay the behavioral suite and re-run cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.

Or by hand against the emulator:
1. Create a VPC (10.93.0.0/16), a subnet (10.93.1.0/24) in an AZ that offers t3.micro, and a security group with no ingress.
2. ec2:RunInstances with the latest Amazon-owned al2023 x86_64 AMI, InstanceType=t3.micro, MinCount=1, MaxCount=1, Monitoring={'Enabled': false}, CreditSpecification={'CpuCredits':'unlimited'}, InstanceInitiatedShutdownBehavior='terminate', UserData='#!/bin/bash\nshutdown -h +40\n'.
3. Poll ec2:DescribeInstances until State.Name == 'running'.
4. Poll cloudwatch:GetMetricStatistics(Namespace='AWS/EC2', MetricName='CPUUtilization', Dimensions=[{'Name':'InstanceId','Value':<instance id>}], StartTime=<launch>-10min, EndTime=now+1min, Period=60, Statistics=['Average','SampleCount']) every 30 s. EXPECTED: a non-empty Datapoints list within a few minutes. OBSERVED: Datapoints stays empty until the 600 s harness budget cuts the check.
5. (Blocked by step 4, and the reason the rest of the check is never graded.) Terminate the instance, wait for 'terminated', call ec2:MonitorInstances(InstanceIds=[<terminated id>]) and expect ('InvalidState', 'Instance is not in a supported state.') with Monitoring.State still 'disabled'; then re-read the metric and confirm the pre-termination datapoints are still returned and no datapoint has a timestamp at or after the termination.
