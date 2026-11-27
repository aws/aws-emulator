Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.proxy-default-endpoint-postgres-session-reaches-target rds.proxy-require-tls-rejects-plaintext-login rds.database-connections-metric-tracks-live-sessions rds.serverless-v2-capacity-auto-adjusts-within-bounds rds.deep-rds-sql-truth-iam-token-past-expiry-rejected
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.proxy-default-endpoint-postgres-session-reaches-target|<lambda>#20|StopIteration|lambda: next(t for t in rds.describe_db_proxy_targets(DBProxyName=proxy_name)["Targets"]
Issue-Key: rds.proxy-require-tls-rejects-plaintext-login|<lambda>#21|StopIteration|lambda: next(t for t in rds.describe_db_proxy_targets(DBProxyName=proxy_name)["Targets"]
Issue-Key: rds.database-connections-metric-tracks-live-sessions|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Key: rds.serverless-v2-capacity-auto-adjusts-within-bounds|_poll_capacity_metric#16|Timeout|time.sleep(30)
Issue-Key: rds.deep-rds-sql-truth-iam-token-past-expiry-rejected|run#15|Timeout|time.sleep(2)
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds: 4 net-new causes — proxy targets never become registered, and the connections metric, Serverless v2 capacity and IAM-token expiry paths never settle

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDBProxyTargets

## Description
4 net-new root causes where an RDS surface never reaches the state a client waits for. Related open issues (20261104T053605Z-rds-db-proxy-targets-never-become-registrable-or.md) describe neighbouring proxy-target behaviour but carry none of these replay keys.

Cause 1 of 4: DescribeDBProxyTargets never reports a target in the state a session needs.
AWS: after RegisterDBProxyTargets, DescribeDBProxyTargets lists the target with TargetHealth.State progressing to AVAILABLE, and only then does the proxy's endpoint accept a session that reaches the database.
Emulator: the poll over DescribeDBProxyTargets finds no matching target at all - the generator raises StopIteration, i.e. Targets never contains an entry satisfying the health predicate for the whole wait.
Why: the proxy data plane is unusable and unobservable: a client cannot learn whether its registration took effect, and its connection attempts have no explanation.
Fix: the proxy target registration/health state machine behind RegisterDBProxyTargets and DescribeDBProxyTargets in the RDS service.
Traces: rds.proxy-default-endpoint-postgres-session-reaches-target, rds.proxy-require-tls-rejects-plaintext-login.

Cause 2 of 4: the AWS/RDS DatabaseConnections metric never tracks live sessions.
AWS: an available instance publishes AWS/RDS DatabaseConnections datapoints (dimension DBInstanceIdentifier) at its monitoring interval, and the value follows the number of open sessions within one period.
Emulator: the check opens sessions and waits for the metric to track them; it does not finish within 900 s, so no datapoint ever reflects the session count (recorded behavior only - no datapoint was observed at all).
Why: connection-based alarms, dashboards and autoscaling rules never fire; the client sees an empty metric rather than any error.
Fix: the RDS instance metric publisher for DatabaseConnections (session accounting -> CloudWatch state).
Traces: rds.database-connections-metric-tracks-live-sessions.

Cause 3 of 4: an Aurora Serverless v2 cluster's capacity never moves inside its configured bounds.
AWS: with a ServerlessV2ScalingConfiguration of MinCapacity/MaxCapacity, the cluster's ServerlessV2 capacity rises under load and falls when idle, and DescribeDBClusters reflects the change within minutes.
Emulator: the check does not finish within 900 s - the capacity it polls never changes (recorded behavior only).
Why: a client cannot observe or test any Serverless v2 scaling behaviour, and a wait on a capacity change never returns.
Fix: the Serverless v2 capacity model behind ModifyDBCluster/DescribeDBClusters in the RDS service.
Traces: rds.serverless-v2-capacity-auto-adjusts-within-bounds.

Cause 4 of 4: the IAM-DB-auth token expiry path never settles.
AWS: an IAM DB auth token is valid for 15 minutes from signing; a login with a token signed outside that window is refused promptly by the engine, so the probe returns inside the check's 180 s budget.
Emulator: the check does not finish within 180 s - neither an accepted nor a refused login is ever observed (recorded behavior only).
Why: a client cannot tell an expired token from a hung endpoint; nothing times out and nothing is refused, so credential-rotation logic stalls.
Fix: the IAM DB authentication path in the SQL front-end (token validation and its refusal), together with whatever makes the connection attempt hang rather than answer.
Traces: rds.deep-rds-sql-truth-iam-token-past-expiry-rejected.

## Evidence
Cause 1: StopIteration from next(t for t in rds.describe_db_proxy_targets(DBProxyName=<proxy>)['Targets'] ...) - no target ever matches (two checks).
Cause 2: Check timed out after 900.0s.
Cause 3: Check timed out after 900.0s.
Cause 4: Check timed out after 180.0s.

## Steps to Reproduce
Cause 1: CreateDBProxy, RegisterDBProxyTargets with a DB instance, then poll DescribeDBProxyTargets for a target whose TargetHealth.State is AVAILABLE.
Cause 2: create an instance, open N sessions, poll GetMetricStatistics for AWS/RDS DatabaseConnections.
Cause 3: create an Aurora Serverless v2 cluster with Min/MaxCapacity, apply load, poll DescribeDBClusters for the capacity.
Cause 4: generate an IAM DB auth token, wait past its validity window, and attempt a login with it.
