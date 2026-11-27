Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__rds.obs-rds-1-connection-load-drives-alarm-through-sns-to-sqs
Issue-Key: cloudwatch__rds.obs-rds-1-connection-load-drives-alarm-through-sns-to-sqs|_poll_until#44|Timeout|time.sleep(interval_s)
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# cloudwatch__rds: a connection-load alarm never fires — the AWS/RDS DatabaseConnections series never carries datapoints, so the alarm→SNS→SQS chain never completes

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStatistics

## Description
Cause 1 of 1: the observability chain from RDS connection load to a CloudWatch alarm to SNS to SQS never completes.

AWS: an available DB instance publishes AWS/RDS DatabaseConnections datapoints at its monitoring interval with dimension DBInstanceIdentifier; an alarm on that metric transitions OK->ALARM once the live-session count crosses its threshold for the configured periods, the transition publishes to the alarm's SNS AlarmAction, and the topic's SQS subscription receives the notification - all well inside the check's 900 s budget. Recorded behavior only for the timing; the metric's existence and the alarm-action delivery are the requirement.

Emulator: the check does not finish within 900 s. It opens sessions against the instance and then waits for the alarm's ALARM transition and the SQS message; neither arrives. Hypothesis: the AWS/RDS DatabaseConnections series is never populated for the instance, so the alarm stays INSUFFICIENT_DATA and no action ever fires - the rds-only check 'database-connections-metric-tracks-live-sessions' hangs the same way on the same metric.

Why: a client cannot build or test any RDS alarm. Autoscaling, paging and runbook logic keyed on DatabaseConnections (or on any alarm action reaching SNS) silently never triggers, and the caller has no error to observe - only an alarm stuck in INSUFFICIENT_DATA.

Fix: the RDS instance metric publisher that must emit AWS/RDS DatabaseConnections datapoints into CloudWatch state, and the CloudWatch alarm evaluator/action dispatch that must drive the SNS AlarmAction on transition.

Traces: cloudwatch__rds.obs-rds-1-connection-load-drives-alarm-through-sns-to-sqs.

## Evidence
Expected: AWS/RDS DatabaseConnections datapoints for the instance, an alarm transition to ALARM, an SNS notification and an SQS message, inside 900 s.
Actual: Check timed out after 900.0s with no alarm transition and no message.

## Steps to Reproduce
1. Create a DB instance, an SNS topic with an SQS subscription and a CloudWatch alarm on AWS/RDS DatabaseConnections (dimension DBInstanceIdentifier) with that topic as AlarmAction.
2. Open several concurrent sessions to the instance.
3. Poll GetMetricStatistics for DatabaseConnections and DescribeAlarms for the alarm state, then receive from the queue: no datapoints, no transition, no message.
