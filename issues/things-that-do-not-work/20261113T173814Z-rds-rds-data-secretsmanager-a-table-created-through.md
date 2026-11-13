Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit
Issue-Key: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit|_rows#2|UndefinedTable|cur.execute(statement, args)
Issue-Commit: 603e6927c4f4e28f4fb6d30537cb70c01235758c

# rds__rds-data__secretsmanager: a table created through the Data API does not exist for a native session on the same cluster's writer endpoint, so no cross-surface row-locking behavior can be observed

> _Note: this issue was identified against the code base at commit `603e6927c4f4e28f4fb6d30537cb70c01235758c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
ONE net-new behavioral root cause for this check. The failing check is marked neither ALREADY FILED nor OWNED by the replay (the replay key it now fails on is UndefinedTable in _rows, not the SyntaxError key an existing issue carries for this check).

Cause 1 of 1: the RDS Data API and the Aurora cluster's native engine endpoint do not address the same database — DDL and rows written through rds-data ExecuteStatement against a cluster ARN are invisible to a native PostgreSQL session connected to that same cluster's published writer endpoint.
AWS: recorded behavior only (the Smithy model of rds-data describes the signed HTTP front door, not storage; there is no modeled error for this). On AWS the Data API is another door onto the SAME Aurora cluster: a CREATE TABLE and INSERT issued through ExecuteStatement(resourceArn, secretArn, database) are committed in that cluster, so a native psycopg session that logs in as the same database user on the cluster's writer endpoint immediately sees the table and its committed rows (and 'SELECT ... FOR UPDATE NOWAIT' on an uncontended row of it succeeds, returning that row).
Emulator: the native session reports the table does not exist — psycopg raises UndefinedTable with the message 'no such table: <table>' when it selects the table the Data API just created in the same check. Two consequences: (a) the two surfaces are backed by separate stores keyed per-surface rather than per-cluster; (b) the refusal's wording is SQLite's 'no such table: <table>', not Aurora PostgreSQL's 'relation "<table>" does not exist' (SQLSTATE 42P01), so even the not-found refusal is not the engine's. The fixed text in AWS's form is 'relation "' / '" does not exist' with the relation name taken from the statement; the emulator's 'no such table: ' prefix is not an engine message.
Why: clients bootstrap schema or seed rows over the Data API (migrations, admin jobs, Lambda setup) and then connect natively, or the reverse. Against this emulator that client sees an empty database, every read-your-writes guarantee across the two surfaces is lost, and all lock/visibility semantics between a Data API transaction and a native session are unobservable — the check never reaches its 55P03 grading because the table is missing.
Fix: the rds-data ExecuteStatement/BatchExecuteStatement handler and the RDS cluster's native-endpoint session state must resolve to ONE backing database per cluster, keyed by the cluster (resourceArn / DBClusterIdentifier) rather than per-surface; and the native endpoint's missing-relation refusal must use the cluster engine's own error class and wording. Open issue 20261108T035337Z-rds-rds-data-secretsmanager-5-net-new-causes.md describes this same cross-surface split (its Cause 1) and even lists this check id, but the replay keys it carries for this check are the now-superseded SyntaxError on 'SELECT ... FOR UPDATE' (that clause now parses); it does not carry this check's current UndefinedTable key, so this failure is not covered by it.
Traces: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit.

## Evidence
rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit [error]:
  File "check_rds__rds_data__secretsmanager_data_api_row_lock_makes_a_native_nowait_lock_rejected_until_commit.py", line 227, in _rows
    cur.execute(statement, args)
  psycopg.errors.UndefinedTable: no such table: <table>

Sequence in the check: ExecuteStatement 'CREATE TABLE <table> (id INT PRIMARY KEY, holder TEXT NOT NULL)' then ExecuteStatement 'INSERT INTO <table> (id, holder) VALUES (1, ...), (2, ...)' on the Data API; BeginTransaction + 'UPDATE <table> SET holder = ... WHERE id = 1'; then from the native psycopg session on the cluster's writer endpoint 'SELECT holder FROM <table> WHERE id = 2 FOR UPDATE NOWAIT'.
Expected: that native SELECT returns the uncontended neighbouring row (the table and its committed rows exist on the cluster); the check then grades the contended NOWAIT as SQLSTATE 55P03 with the primary message 'could not obtain lock on row in relation "<table>"', and after CommitTransaction the NOWAIT sees the Data API's value.
Actual: the native session cannot see the Data API's table at all — UndefinedTable 'no such table: <table>'. Nothing after the table creation is reached.
Also diverging in that refusal: Aurora PostgreSQL words a missing relation as 'relation "<table>" does not exist' with SQLSTATE 42P01; 'no such table: <table>' is not an engine message. The table name is a per-run generated value, not a requirement.

## Steps to Reproduce
Replay the behavioral suite (run_behavioral_suite) and re-run rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-nowait-lock-rejected-until-commit. Manually, against one Aurora PostgreSQL Serverless v2 cluster with the Data API enabled and its admin credential in Secrets Manager:
1. rds-data ExecuteStatement(resourceArn=<cluster arn>, secretArn=<secret arn>, database=<db>) 'CREATE TABLE t (id INT PRIMARY KEY, holder TEXT NOT NULL)', then an INSERT of two rows.
2. Open a native psycopg session to the cluster's published writer endpoint as the same database user (password read from the same secret), autocommit off.
3. Run 'SELECT holder FROM t WHERE id = 2 FOR UPDATE NOWAIT' in that native session. On AWS it returns the row; the emulator raises psycopg UndefinedTable 'no such table: t'.
4. For the reverse direction, CREATE a table natively and SELECT it through ExecuteStatement — it is likewise not found.
