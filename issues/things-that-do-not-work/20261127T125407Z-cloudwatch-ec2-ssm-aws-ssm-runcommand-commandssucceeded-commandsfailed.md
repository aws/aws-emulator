Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__ec2__ssm.ssm-run-command-publishes-succeeded-and-failed-command-metrics
Issue-Key: cloudwatch__ec2__ssm.ssm-run-command-publishes-succeeded-and-failed-command-metrics|_poll#10|Timeout|time.sleep(interval_s)
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# cloudwatch__ec2__ssm: AWS/SSM-RunCommand CommandsSucceeded / CommandsFailed are never published, so the Run Command metric scenario reaches no verdict

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: Run Command publishes no command metrics to CloudWatch.
AWS: after commands succeed and fail, Run Command publishes `CommandsSucceeded` and `CommandsFailed` to the `AWS/SSM-RunCommand` namespace (once a minute), so GetMetricStatistics over the run window sums each to at least 1.
Emulator: the scenario makes no progress and is cut at the harness cap (`Check timed out after 60.0s`) while polling the two metrics; no datapoint is observed for either. Hypothesis: the SSM crate never calls PutMetricData for Run Command, so the namespace stays empty.
Why: dashboards and alarms on AWS/SSM-RunCommand read empty, so a fleet-wide command failure is invisible.
Fix: the Run Command metric publisher in the SSM crate (emit CommandsSucceeded/CommandsFailed into the CloudWatch crate as invocations settle).
Traces: cloudwatch__ec2__ssm.ssm-run-command-publishes-succeeded-and-failed-command-metrics.

## Evidence
'Check timed out after 60.0s'. The check sends one succeeding and one failing command, then polls GetMetricStatistics(Namespace='AWS/SSM-RunCommand', MetricName in {CommandsSucceeded, CommandsFailed}, Period=60, Statistics=['Sum']) for a Sum >= 1; no datapoint was ever returned.

## Steps to Reproduce
Send one command that succeeds and one that fails on a managed node, then call GetMetricStatistics for Namespace 'AWS/SSM-RunCommand', MetricName 'CommandsSucceeded' and 'CommandsFailed' with Period 60 and Statistics ['Sum'] over the run window.
