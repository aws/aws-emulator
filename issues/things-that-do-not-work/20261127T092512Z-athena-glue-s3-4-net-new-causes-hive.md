Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.alter-table-add-partition-registers-the-glue-partition athena__glue__s3.approximate-aggregates-evaluate-over-a-table athena__glue__s3.conditional-and-by-value-aggregates-over-a-table athena__glue__s3.describe-table-lists-the-columns-and-types athena__glue__s3.e2e-json-event-log-daily-rollup-parses-iso8601-timestamps athena__glue__s3.explain-returns-the-query-plan athena__glue__s3.show-columns-and-show-tblproperties-list-table-metadata athena__glue__s3.show-create-table-returns-the-table-ddl
Issue-Key: athena__glue__s3.alter-table-add-partition-registers-the-glue-partition|run#10|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.approximate-aggregates-evaluate-over-a-table|_select#7|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.conditional-and-by-value-aggregates-over-a-table|_select#7|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.describe-table-lists-the-columns-and-types|run#9|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.e2e-json-event-log-daily-rollup-parses-iso8601-timestamps|_select#11|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.explain-returns-the-query-plan|run#10|AssertionError|assert [c["Name"] for c in cols] == ["Query Plan"], (
Issue-Key: athena__glue__s3.show-columns-and-show-tblproperties-list-table-metadata|run#8|AssertionError|assert execution["Status"]["State"] == "SUCCEEDED", (
Issue-Key: athena__glue__s3.show-create-table-returns-the-table-ddl|run#9|AssertionError|assert status["State"] == "SUCCEEDED", (
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# athena__glue__s3: 4 net-new causes — Hive DDL/SHOW/DESCRIBE statements are refused as SQLite syntax errors, EXPLAIN serves a SQLite bytecode plan, and Trino aggregate and ISO-8601 timestamp functions are absent

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartQueryExecution

## Description
Four independent root causes, all observed as query executions that settle FAILED (or return the wrong shape) over Glue-catalogued tables backed by S3. Each must be closed before this issue is submitted. Related, already-filed context: 20261111T090701Z-athena-s-sql-engine-refuses-trino-hive-statements.md and 20261104T163600Z-athena-statements-are-executed-by-a-sqlite-engine.md were marked fixed, but these statements are still not parsed or evaluated; 20261123T003930Z-athena-3-net-new-causes-trino-nested-type.md is open for nested-type projection and does not cover any of the statements below.

=== Cause 1 of 4: Hive catalog statements (ALTER TABLE ... ADD PARTITION, DESCRIBE, SHOW COLUMNS, SHOW TBLPROPERTIES, SHOW CREATE TABLE) are refused as syntax errors ===
AWS: recorded behavior only (the Smithy model carries no SQL grammar; StartQueryExecution takes an opaque QueryString). Athena accepts Hive DDL and metadata statements: `ALTER TABLE <db>.<tbl> ADD PARTITION (<k> = '<v>') LOCATION '<s3 uri>'` ends SUCCEEDED as a DDL execution and registers the partition in the Glue catalog; `DESCRIBE <db>.<tbl>`, `SHOW COLUMNS IN <db>.<tbl>`, `SHOW TBLPROPERTIES <db>.<tbl>` and `SHOW CREATE TABLE <db>.<tbl>` all end SUCCEEDED and return the table's columns/types, properties and DDL text as rows.
Emulator: every one of them settles State FAILED with StateChangeReason and AthenaError.ErrorMessage of the form 'near "<token>": syntax error in <the statement> at offset <n>' — i.e. a SQLite parser diagnostic naming the offending Hive keyword ('(' for ADD PARTITION, 'DESCRIBE', 'SHOW'). AthenaError is reported as ErrorCategory 2 / ErrorType 1000.
Why: a client cannot register partitions or read table metadata through Athena at all; the failure text is a SQLite diagnostic that exists nowhere in AWS, so error handling keyed on Athena's own categories/messages cannot classify it.
Fix: the SQL front-end used by StartQueryExecution's execution path must parse these Hive statement forms and route them to the Glue-catalog metadata operations (partition registration, column/property/DDL projection).
Traces: athena__glue__s3.alter-table-add-partition-registers-the-glue-partition, athena__glue__s3.describe-table-lists-the-columns-and-types, athena__glue__s3.show-columns-and-show-tblproperties-list-table-metadata, athena__glue__s3.show-create-table-returns-the-table-ddl

=== Cause 2 of 4: EXPLAIN returns a SQLite bytecode listing instead of Athena's single 'Query Plan' column ===
AWS: recorded behavior only. `EXPLAIN <query>` ends SUCCEEDED and its ResultSet has exactly one column, named "Query Plan", whose rows carry the textual plan.
Emulator: the execution succeeds but the result set has eight columns — 'addr', 'opcode', 'p1', 'p2', 'p3', 'p4', 'p5', 'comment' — with SQLite VDBE instruction rows ('Init', 'OpenRead', 'Rewind', ...).
Why: any client that reads the plan (a cost/plan assertion, a console view, a regression guard on plan shape) gets a result set of the wrong arity and column names, and the plan text is for a different engine entirely.
Fix: EXPLAIN handling in the query engine plus the ResultSet/ColumnInfo serializer for that statement type.
Traces: athena__glue__s3.explain-returns-the-query-plan

=== Cause 3 of 4: Trino aggregate functions are absent (approx_distinct, approx_percentile, count_if, bool_or, max_by, min_by) ===
AWS: recorded behavior only. These are Trino built-ins available to every Athena engine-v3 DML query over a catalogued table; the statements end SUCCEEDED and return the aggregated values.
Emulator: the execution settles FAILED with the message 'no such function: <name> in <the rewritten statement> at offset <n>' — a SQLite function-resolution diagnostic (observed for approx_distinct and count_if; the companion functions in the same statements are unreachable behind the first failure).
Why: analytic queries using Athena's standard aggregate library fail outright, and the failure is reported as a nonexistent-function error rather than any result.
Fix: the query engine's function registry/evaluation used by StartQueryExecution.
Traces: athena__glue__s3.approximate-aggregates-evaluate-over-a-table, athena__glue__s3.conditional-and-by-value-aggregates-over-a-table

=== Cause 4 of 4: from_iso8601_timestamp (with date_format) is absent, so an ISO-8601 event log cannot be rolled up ===
AWS: recorded behavior only. `SELECT date_format(from_iso8601_timestamp(ts), '%Y-%m-%d') AS day, count(*) AS n FROM <db>.<tbl> GROUP BY 1 ORDER BY 1` ends SUCCEEDED and returns one row per calendar day.
Emulator: the execution settles FAILED with 'no such function: from_iso8601_timestamp in <the rewritten statement> at offset 19'.
Why: the common JSON-event-log rollup pattern (parse an ISO-8601 string column, bucket by day) cannot run, so no time-series aggregation over string timestamps is possible.
Fix: the query engine's function registry — the Trino date/time parsing and formatting functions.
Traces: athena__glue__s3.e2e-json-event-log-daily-rollup-parses-iso8601-timestamps

## Evidence
Cause 1: ADD PARTITION ended {'State': 'FAILED', 'StateChangeReason': 'near "(": syntax error in ALTER TABLE <db>.<tbl> ADD PARTITION (dt = \'<value>\') LOCATION \'s3://<bucket>/data/sales/dt=<value>/\' at offset 89', 'AthenaError': {'ErrorCategory': 2, 'ErrorType': 1000, ...}}; DESCRIBE ended 'FAILED': 'near "DESCRIBE": syntax error in DESCRIBE <db>.<tbl> at offset 0'; SHOW COLUMNS ended 'FAILED': 'near "SHOW": syntax error in SHOW COLUMNS IN <db>.<tbl> at offset 0'; SHOW CREATE TABLE ended 'FAILED': 'near "SHOW": syntax error in SHOW CREATE TABLE <db>.<tbl> at offset 0'. Expected State SUCCEEDED in every case (and the partition visible in Glue).
Cause 2: expected ColumnInfo names ['Query Plan']; got ['addr', 'opcode', 'p1', 'p2', 'p3', 'p4', 'p5', 'comment'] with first rows [['0', 'Init', '0', '10', '0', None, '0', None], ['1', 'OpenRead', ...], ['2', 'Rewind', ...]].
Cause 3: 'SELECT approx_distinct(name) AS d, approx_percentile(amount, 0.5) AS p50 FROM <db>.<tbl>' ended 'FAILED': 'no such function: approx_distinct ... at offset 7'; 'SELECT count_if(amount > 8) AS big, bool_or(amount > 15) AS any_big, max_by(name, amount) AS top, min_by(name, amount) AS low FROM <db>.<tbl>' ended 'FAILED': 'no such function: count_if ... at offset 7'. Expected State SUCCEEDED with the aggregate values.
Cause 4: "SELECT date_format(from_iso8601_timestamp(ts), '%Y-%m-%d') AS day, count(*) AS n FROM <db>.<tbl> GROUP BY 1 ORDER BY 1" ended 'FAILED': 'no such function: from_iso8601_timestamp ... at offset 19'. Expected State SUCCEEDED with one row per day.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids. For each: create a Glue database and an external table over CSV/JSON objects in S3, create a workgroup with a ResultConfiguration.OutputLocation, then StartQueryExecution the statement and poll GetQueryExecution to a terminal state (and GetQueryResults where a result set is asserted).
Cause 1: `ALTER TABLE <db>.<tbl> ADD PARTITION (dt = '2024-01-03') LOCATION 's3://<bucket>/data/sales/dt=2024-01-03/'`, `DESCRIBE <db>.<tbl>`, `SHOW COLUMNS IN <db>.<tbl>`, `SHOW TBLPROPERTIES <db>.<tbl>`, `SHOW CREATE TABLE <db>.<tbl>`.
Cause 2: `EXPLAIN SELECT ... FROM <db>.<tbl>` then GetQueryResults and read ResultSet.ResultSetMetadata.ColumnInfo.
Cause 3: `SELECT approx_distinct(name) AS d, approx_percentile(amount, 0.5) AS p50 FROM <db>.<tbl>` and `SELECT count_if(amount > 8) AS big, bool_or(amount > 15) AS any_big, max_by(name, amount) AS top, min_by(name, amount) AS low FROM <db>.<tbl>`.
Cause 4: `SELECT date_format(from_iso8601_timestamp(ts), '%Y-%m-%d') AS day, count(*) AS n FROM <db>.<tbl> GROUP BY 1 ORDER BY 1` over a JSON event log whose ts column holds ISO-8601 strings.
