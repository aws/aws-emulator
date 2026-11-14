Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__redshift.single-node-cluster-leader-health-alarm-stays-insufficient-data
Issue-Key: cloudwatch__redshift.single-node-cluster-leader-health-alarm-stays-insufficient-data|_poll#14|Timeout|time.sleep(interval_s)
Issue-Commit: 6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8

# CloudWatch alarms on AWS/Redshift HealthStatus never evaluate, so the single-node leader/shared-node alarm differential can never be observed (1 cause)

> _Note: this issue was identified against the code base at commit `6b8dcd825499bdd91675cbbb3bc6eaac525bf5b8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeAlarms

## Description
One net-new root cause.

=== Cause 1 of 1: metric alarms watching a live Redshift cluster's service-published HealthStatus never leave their initial state ===
Failing check: cloudwatch__redshift.single-node-cluster-leader-health-alarm-stays-insufficient-data

AWS: a cluster that is 'available' publishes the AWS/Redshift HealthStatus metric, dimensioned by ClusterIdentifier and (per node) by NodeID. CloudWatch then evaluates each PutMetricAlarm-created alarm against that series and DescribeAlarms reports the outcome in StateValue. Smithy (cloudwatch): StateValue is an enum with exactly OK | ALARM | INSUFFICIENT_DATA; DescribeAlarms returns MetricAlarms[].StateValue and DescribeAlarmHistory returns AlarmHistoryItems with HistoryItemType StateUpdate for each transition. On a single-node cluster the one node's NodeRole is SHARED, so an alarm dimensioned NodeID=Shared receives data and evaluates (here to OK against LessThanThreshold 1.0), an alarm dimensioned NodeID=Leader receives no series at all and must stay INSUFFICIENT_DATA with an empty StateUpdate history, and the cluster-wide alarm (GreaterThanThreshold 0.5 on a healthy cluster, i.e. misconfigured) must reach ALARM.

Emulator: the check produces no verdict — 'Check timed out after 60.0s'. The differential barrier it waits on (the cluster-wide alarm in ALARM AND the NodeID=Shared twin in OK) is never satisfied, so DescribeAlarms never reports an evaluated StateValue for either alarm within the budget. Recorded behavior only for the exact stall point; the replay output carries no frame. Hypothesis: either the cluster never reaches 'available' and so publishes no HealthStatus series, or no alarm evaluator runs against AWS/Redshift metrics at all, leaving every alarm parked at its creation state.

Note: open issues 20261024T030130Z-cloudwatch-rds-cloudwatch-metric-alarms-never-evaluate-or-fire-their.md and 20261024T055421Z-cloudwatch-opensearch-sns-sqs-cloudwatch-accepts-putmetricdata-into-the-reserved-aws-namespace.md describe alarms failing to evaluate for other service pairings; neither carries this check's replay key and the suite did not mark this failure as owned, so it is filed here and those issues are named for context.

Why: a client that wires an alarm to a Redshift cluster's health gets an alarm that never fires and never resolves — a monitoring configuration cannot be validated against the emulator, and the distinction that matters operationally (a dimensioned alarm with NO data staying INSUFFICIENT_DATA versus one that evaluates) is unobservable because nothing evaluates in the first place.

Fix: the CloudWatch alarm evaluator (alarm state machine + its DescribeAlarms/DescribeAlarmHistory projection) must evaluate alarms against published metric series, including multi-dimension (ClusterIdentifier + NodeID) selection, and the Redshift cluster lifecycle must publish the AWS/Redshift HealthStatus series for an available cluster and its nodes.

Traces: cloudwatch__redshift.single-node-cluster-leader-health-alarm-stays-insufficient-data

## Evidence
Expected: after CreateCluster(ClusterType=single-node) reaches available with ClusterNodes[].NodeRole == ['SHARED'], and three PutMetricAlarm alarms on AWS/Redshift HealthStatus (Period 60, EvaluationPeriods 1, DatapointsToAlarm 1) are created, DescribeAlarms converges to StateValue {cluster-wide: 'ALARM', NodeID=Shared: 'OK', NodeID=Leader: 'INSUFFICIENT_DATA'} and DescribeAlarmHistory(HistoryItemType='StateUpdate') for the Leader alarm returns an empty list. Actual: 'Check timed out after 60.0s' — the cluster-wide/Shared pair never reaches ('ALARM','OK'), so no StateValue verdict is produced at all.

## Steps to Reproduce
1) redshift CreateCluster with ClusterType=single-node, Encrypted=true, PubliclyAccessible=false; poll DescribeClusters until ClusterStatus=available and confirm ClusterNodes[].NodeRole == ['SHARED']. 2) cloudwatch PutMetricAlarm three times on Namespace=AWS/Redshift, MetricName=HealthStatus, Statistic=Average, Period=60, EvaluationPeriods=1, DatapointsToAlarm=1, ActionsEnabled=false: (a) Dimensions=[ClusterIdentifier], Threshold=0.5, GreaterThanThreshold; (b) Dimensions=[ClusterIdentifier, NodeID=Leader], Threshold=1.0, LessThanThreshold; (c) Dimensions=[ClusterIdentifier, NodeID=Shared], Threshold=1.0, LessThanThreshold. 3) poll DescribeAlarms for all three names and observe that no StateValue ever becomes ALARM or OK. 4) DescribeAlarmHistory(AlarmName=<leader>, HistoryItemType='StateUpdate'). Replay: cloudwatch__redshift.single-node-cluster-leader-health-alarm-stays-insufficient-data.
