Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin
Issue-Key: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin|_assert_stays#34|Timeout|time.sleep(interval_s)
Issue-Commit: f0c814ee744767522be6756500cd69fea7713687

# cloudwatch__pipes__sqs: a pipe's TargetStageFailed is unobservable — no AWS/EventBridge/Pipes series is ever published, and a role-denied target failure is indistinguishable from a healthy pipe (1 cause)

> _Note: this issue was identified against the code base at commit `f0c814ee744767522be6756500cd69fea7713687`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreatePipe

## Description
Cause 1 of 1: the EventBridge Pipes data plane publishes no AWS/EventBridge/Pipes CloudWatch metrics, so a pipe that fails at the TARGET stage and a pipe that delivers successfully read identically at CloudWatch.
AWS: recorded behavior only for the metric contract (CloudWatch's Smithy model does not model service namespaces; EventBridge Pipes publishes them from the data plane — https://docs.aws.amazon.com/eventbridge/latest/userguide/eb-pipes-monitoring.html). A RUNNING pipe polls its source and attempts its target under its execution role. A pipe whose role has no sqs:SendMessage on its target queue fails at the TARGET stage and publishes AWS/EventBridge/Pipes TargetStageFailed with Sum >= 1 under the single dimension PipeName, with CloudWatch Unit 'None' (not 'Count'); the account-level Concurrency/Invocations series carry AwsAccountId instead, so the per-pipe series must carry PipeName alone and ListMetrics must catalogue it with exactly the dimension set ('PipeName',). A healthy pipe on identical wiring delivers the message to its target queue and materialises the SAME metric as an explicit 0 ('will be 0 if no executions failed') rather than omitting the series. On the control plane, DescribePipe reports CurrentState='RUNNING' for both pipes.
Emulator: nothing on that path is observable. The check creates two source/target SQS queue pairs and two pipes — one whose execution role grants sqs:ReceiveMessage on the source but NO sqs:SendMessage on the target, one whose role grants both — sends one marked message into each source, and then (a) waits for the healthy twin's target queue to receive the marker, (b) polls GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{PipeName}], Period=60) for Sum >= 1 on the failing pipe, (c) polls ListMetrics for that series, and (d) requires the healthy twin's TargetStageFailed to exist and read exactly 0.0. None of it converges: the run is killed at the 60 s replay cap with the check still inside those waits (its own budgets are 300 s for delivery and 900 s for the metric). Hypothesis for the attribution, consistent with the already-filed pipes issues 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md (a RUNNING pipe never polls its source or delivers to its target) and 20261027T030720Z-pipes-the-data-plane-never-authorizes-against-the.md (the data plane never authorizes against the execution role): with no runtime there is no execution to succeed or fail, hence no TargetStageFailed datapoint for either pipe. Neither of those issues covers the CloudWatch-facing half of this contract (the AWS/EventBridge/Pipes series, its 'None' unit, its PipeName-only dimension set, and the explicit zero on a healthy pipe), and neither carries this check's replay key.
Why: a client cannot tell 'the pipe refused to deliver' from 'the pipe never ran'. Pipes monitoring is the only signal that a misconfigured execution role is dropping events — an operator dashboard or an alarm on TargetStageFailed stays empty while events are silently lost, and an alarm built on the metric never leaves INSUFFICIENT_DATA because the series does not exist at all (not even as the documented zero).
Fix: the Pipes data-plane runtime and its metric emission — the component that drives a RUNNING pipe's source poll, target attempt and per-stage outcome must publish AWS/EventBridge/Pipes datapoints (TargetStageFailed, and the zero on success) into the CloudWatch metric store under dimension PipeName with Unit 'None', and register the series in the metric catalogue so ListMetrics reports the dimension set ('PipeName',). The CloudWatch read handlers are not implicated — they serve whatever is published.
Traces: cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin.

## Evidence
Failing check: [mismatch] cloudwatch__pipes__sqs.target-stage-failed-publishes-for-the-failing-pipe-and-zero-for-its-twin — "Error: Check timed out after 60.0s".

Expected (AWS): the healthy pipe's target queue yields the marker message; GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{'Name':'PipeName','Value':<failing pipe>}], Period=60, Statistics=['Sum','SampleCount','Maximum']) returns datapoints whose Sum totals >= 1.0 and whose Unit is exactly 'None'; ListMetrics for that namespace/PipeName reports the dimension set ('PipeName',); the same read for the healthy pipe returns a datapoint with Sum == 0.0 and keeps reading 0.0 for the hold window.
Actual (emulator): the check never gets a reading. It is killed at the 60 s cap inside these waits, each of which fails only by exhaustion:
    _poll_until(lambda: _sum(bad_pipe, "TargetStageFailed"), lambda v: v is not None and v >= 1.0, timeout_s=900.0, ...)
    _poll_until(lambda: _dimension_sets(cw, "AWS/EventBridge/Pipes", "PipeName", bad_pipe), lambda s: bool(s), timeout_s=900.0, ...)
    _poll_until(lambda: _sum(ok_pipe, "TargetStageFailed"), lambda v: v is not None, timeout_s=900.0, ...)
and, before them, `_drain_until(... the healthy pipe delivering to its target queue ..., timeout_s=300.0)`. No non-2xx response is involved; the metric series and the delivery simply never appear.

Related already-open issues (neither carries this replay key, and neither covers the AWS/EventBridge/Pipes metric contract): 20261023T235727Z-pipes-eventbridge-pipes-has-no-runtime-a-running-pipe.md, 20261027T030720Z-pipes-the-data-plane-never-authorizes-against-the.md.

## Steps to Reproduce
1. Create four SQS queues: bad-source, bad-target, ok-source, ok-target.
2. Create two IAM roles assumable by pipes.amazonaws.com: role_bad with sqs:ReceiveMessage/DeleteMessage/GetQueueAttributes on bad-source ONLY (no sqs:SendMessage on bad-target); role_ok with the same source permissions plus sqs:SendMessage on ok-target.
3. CreatePipe(Name=failing, Source=bad-source ARN, Target=bad-target ARN, RoleArn=role_bad) and CreatePipe(Name=healthy, Source=ok-source ARN, Target=ok-target ARN, RoleArn=role_ok).
4. Poll DescribePipe on both until CurrentState='RUNNING'.
5. SendMessage one marked body into ok-source and into bad-source.
6. ReceiveMessage on ok-target: the marker never arrives (AWS: it does).
7. GetMetricStatistics(Namespace='AWS/EventBridge/Pipes', MetricName='TargetStageFailed', Dimensions=[{'Name':'PipeName','Value':'failing'}], StartTime=now-25m, EndTime=now+5m, Period=60, Statistics=['Sum','SampleCount','Maximum']) -> no datapoints, indefinitely (AWS: Sum >= 1.0, Unit 'None').
8. The same read for PipeName='healthy' -> no datapoints (AWS: an explicit 0.0).
9. ListMetrics(Namespace='AWS/EventBridge/Pipes', Dimensions=[{'Name':'PipeName','Value':'failing'}]) -> empty (AWS: one metric with dimension set ('PipeName',)).
