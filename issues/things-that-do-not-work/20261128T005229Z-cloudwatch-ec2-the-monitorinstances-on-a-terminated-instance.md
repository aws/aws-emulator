Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected
Issue-Key: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected|_poll_until#31|Timeout|time.sleep(interval_s)
Issue-Commit: 38f62366cd2817d69083419007bb0aafef336c94

# cloudwatch__ec2: the MonitorInstances-on-a-terminated-instance scenario never reaches a verdict — the check stalls and times out at 600 s

> _Note: this issue was identified against the code base at commit `38f62366cd2817d69083419007bb0aafef336c94`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
MonitorInstances

## Description
Cause 1 of 1: the EC2 + CloudWatch lifecycle this check walks never progresses far enough for any of its assertions to run — the check is killed by the harness budget (`Check timed out after 600.0s`) instead of producing a pass or a mismatch, so the refusal that is the point of the check (MonitorInstances against a terminated instance) is never exercised. Failing check: cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected.

AWS: recorded behavior only for this flow. Smithy (services/service-com-amazonaws-ec2-amazonec2) models MonitorInstances with input {InstanceIds (required), DryRun} and output {InstanceMonitorings}, and TerminateInstances with {InstanceIds (required), ...} -> {TerminatingInstances}; neither declares any error shape (EC2 uses the ec2Query error envelope), and the CloudWatch model (services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801, GetMetricStatistics) says nothing about publication cadence, so the requirements below come from the recording. The recorded AWS behavior the check encodes is: (a) a just-launched t3.micro reaches state `running`, and after TerminateInstances reaches `terminated`; (b) an instance publishes AWS/EC2 CPUUtilization datapoints readable through GetMetricStatistics(Namespace="AWS/EC2", MetricName="CPUUtilization", Dimensions=[{InstanceId}]) while it runs; (c) MonitorInstances on a TERMINATED instance is refused with error code `InvalidState` and the message, whose text is entirely fixed with no variable part, `Instance is not in a supported state.`, leaving Monitoring.State at `disabled`; (d) the same call on the running twin answers InstanceMonitorings[].Monitoring.State in {`pending`,`enabled`} and the instance then reports `enabled`; (e) after the switch the twin publishes CPUUtilization on minute boundaries that are not multiples of 300 s, while the terminated instance publishes no bucket at or after its termination and keeps the buckets it published before it.

Emulator: no assertion in the check ever fired. The replay reports only `Error: Check timed out after 600.0s`, i.e. one of the sequential waits (instance -> `running`, first CPUUtilization datapoint for the doomed instance, instance -> `terminated`, twin -> `enabled`, or the post-switch off-5-minute-grid datapoint at or after the next 300 s boundary + 60 s) never satisfied its predicate inside the harness budget. Hypothesis (neither the model nor the recording states which stage stalls, because the harness kills the check before it reports one): instance state transitions and/or AWS/EC2 metric emission do not advance on the wall clock the way the recording does, so the scenario cannot complete. Note the separate open issue 20261125T150300Z-cloudwatch-ec2-a-basic-monitoring-instance-publishes-cpuutilization.md reports that basic-monitoring CPUUtilization is emitted every minute rather than every 300 s; it does NOT cover this check — that issue is an assertion mismatch on cadence, this one is a scenario that reaches no verdict at all, and it carries a different replay key.

Why: a client that drives the ordinary EC2 observability lifecycle (launch, read AWS/EC2 metrics, terminate, re-enable monitoring) hangs instead of getting an answer. Nothing in this path is verified against the emulator: a caller cannot rely on MonitorInstances being refused with InvalidState on a terminated instance (it may silently succeed and appear to revive monitoring), nor on a terminated instance stopping publication while its metric history survives, and automation that polls for `running`/`terminated` or for the first datapoint can block indefinitely.

Fix: the EC2 instance-lifecycle state machine and the AWS/EC2 instance-metric emitter that feeds the cloudwatch metric store (and MonitorInstances' state validation in the EC2 handler) — the run/terminate transitions and the metric buckets must become observable promptly enough for a polling client, and MonitorInstances must classify a terminated instance as an unsupported state rather than leaving the caller waiting.

Traces: check cloudwatch__ec2.metrics-ec2-ecs-monitor-instances-on-terminated-instance-rejected (verdict: mismatch, `Check timed out after 600.0s`).

## Evidence
Expected (recorded AWS): the check completes, with MonitorInstances on the terminated instance refused as (`InvalidState`, `Instance is not in a supported state.`) — fixed text, no variable parts — Monitoring.State still `disabled`; MonitorInstances on the running twin reporting Monitoring.State in {`pending`,`enabled`} and settling at `enabled`; GetMetricStatistics(AWS/EC2, CPUUtilization, Dimensions=[{InstanceId: <twin id>}], Period=60) returning at least one datapoint after the switch whose timestamp is not a multiple of 300 s; and GetMetricStatistics for the terminated instance returning its pre-termination datapoints and none at or after the termination instant.

Actual (emulator): the check produced no assertion result at all — replay verdict `mismatch` with detail `Error: Check timed out after 600.0s`. The instance ids and timestamps in such a run are per-run values and are not part of the requirement; what diverges is that the sequence of state and metric observations the recording completes does not complete here.

## Steps to Reproduce
1. Create a VPC (10.93.0.0/16) with one subnet and a no-ingress security group, and resolve the latest Amazon-owned AL2023 x86_64 AMI via DescribeImages.
2. RunInstances twice (t3.micro, Monitoring={Enabled: false}, InstanceInitiatedShutdownBehavior=terminate) and poll DescribeInstances until both report State.Name == `running`.
3. Poll GetMetricStatistics(Namespace="AWS/EC2", MetricName="CPUUtilization", Dimensions=[{Name: InstanceId, Value: <doomed id>}], Period=60, Statistics=[Average, SampleCount]) until at least one datapoint exists.
4. TerminateInstances on the first instance and poll until State.Name == `terminated`.
5. Call MonitorInstances(InstanceIds=[<terminated id>]) and expect the refusal (`InvalidState`, `Instance is not in a supported state.`) with Monitoring.State still `disabled`.
6. Call MonitorInstances(InstanceIds=[<live id>]), expect Monitoring.State in {pending, enabled}, then poll for `enabled` and for a CPUUtilization datapoint whose epoch second is not a multiple of 300 at or after the next 300 s boundary + 60 s.
7. Observe that the run never reaches step 5/6: the check is terminated by the 600 s harness budget with no assertion result.
