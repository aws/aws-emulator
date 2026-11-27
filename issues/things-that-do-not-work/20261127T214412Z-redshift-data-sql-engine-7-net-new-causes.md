Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.auto-commit-batch-keeps-statements-before-a-failure redshift-data.auto-commit-batch-reports-failed-but-keeps-the-rows-committed-before-the-failure redshift-data.list-databases-pages-name-each-database-once redshift-data.list-databases-pages-show-a-database-created-and-hide-it-once-dropped redshift-data.list-databases-through-a-dropped-database-rejected redshift-data.list-tables-reflects-created-and-dropped-relations redshift-data.get-statement-result-v2-csv-pages-hold-every-row-once redshift-data.secret-arn-statement-runs-as-the-secret-database-user redshift-data.get-statement-result-on-failed-cast-statement-rejected redshift-data.cancelled-transaction-batch-ends-aborted-and-rolls-back-its-finished-insert
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.auto-commit-batch-keeps-statements-before-a-failure|run#11|AssertionError|assert "does not exist" in subs[2].get("Error", ""), (
Issue-Key: redshift-data.auto-commit-batch-reports-failed-but-keeps-the-rows-committed-before-the-failure|run#14|AssertionError|assert auto["SubStatements"][1].get("Error") == expected_error, (
Issue-Key: redshift-data.list-databases-pages-name-each-database-once|run#11|AssertionError|assert dropped["Status"] == "FINISHED", f"DROP DATABASE {gone} ended {dropped['Status']!r}: {dropped.get('Error')!r}"
Issue-Key: redshift-data.list-databases-pages-show-a-database-created-and-hide-it-once-dropped|_run_sql#14|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: redshift-data.list-databases-through-a-dropped-database-rejected|_sql_finished#7|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: redshift-data.list-tables-reflects-created-and-dropped-relations|_run_sql#8|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: redshift-data.get-statement-result-v2-csv-pages-hold-every-row-once|_run_sql#6|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: redshift-data.secret-arn-statement-runs-as-the-secret-database-user|_run#18|AssertionError|assert final["Status"] == "FINISHED", (
Issue-Key: redshift-data.get-statement-result-on-failed-cast-statement-rejected|run#5|AssertionError|assert bad_final["Status"] == "FAILED", f"expected FAILED casting to uuid, got {bad_final!r}"
Issue-Key: redshift-data.cancelled-transaction-batch-ends-aborted-and-rolls-back-its-finished-insert|_wait_for#13|AssertionError|raise AssertionError(f"{label}: still not there after {budget_s:.0f}s, last {value!r}")
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# redshift-data SQL engine: 7 net-new causes — SQLite-shaped statement errors, DROP DATABASE and schema-qualified CREATE VIEW unparsed, LPAD/REPEAT and current_user missing, an impossible ::uuid cast FINISHES, batch sub-statement status never advances

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement (also BatchExecuteStatement, DescribeStatement)

## Description
Seven distinct defects in the SQL engine that backs redshift-data statements (serverless workgroup targets). Each cause below is separate work; all seven must be closed before this issue is submitted. Related but NOT covering: the open issues 20261029T193810Z-redshift-cluster-sql-engine-multi-statement-sql-refused.md, 20261121T211635Z-redshift-cluster-native-sql-endpoint-7-net-new.md, 20261122T064140Z-redshift-cluster-native-sql-endpoint-5-net-new.md and 20261122T160345Z-redshift-cluster-sql-engine-group-ddl-is-unparsed.md are filed against the `redshift` cluster SQL endpoint; the failures here are redshift-data Data API statements and none of them is marked owned or already filed.

Cause 1 of 7: a statement over a missing relation reports a SQLite error string instead of Redshift's.
AWS: recorded behavior only — the Smithy model carries no error text for an async statement; the failure is reported in DescribeStatement.Error with HTTP 200 on the submit. Redshift's engine text for a missing relation is the fixed text `ERROR: relation "` + the relation name taken from the submitted SQL + `" does not exist`.
Emulator: DescribeStatement.Error is `no such table: <relation>` (SQLite wording), e.g. 'no such table: sim2real_ac_22a1c7853570_absent' where AWS answers 'ERROR: relation "sim2real_ac_22a1c7853570_absent" does not exist'. Only the relation name is a per-run value; the rest of both strings is fixed text.
Why: a client that classifies statement failures by the engine's message (looking for 'does not exist', or parsing the 'ERROR: ' prefix Redshift always emits) sees an unrecognised message and cannot distinguish a missing relation from any other failure.
Fix: the statement-failure mapping in the redshift-data SQL engine — translate engine errors into Redshift/PostgreSQL-shaped text before storing them on the statement record read by DescribeStatement.
Traces: auto-commit-batch-keeps-statements-before-a-failure, auto-commit-batch-reports-failed-but-keeps-the-rows-committed-before-the-failure.

Cause 2 of 7: DROP DATABASE is not parsed.
AWS: recorded behavior only — `DROP DATABASE <name>` submitted through ExecuteStatement ends FINISHED and the database disappears from ListDatabases.
Emulator: the statement ends FAILED with Error `near "DATABASE": syntax error in DROP DATABASE <name> at offset 5` (fixed text 'near "DATABASE": syntax error in ' + the submitted SQL + ' at offset 5'; the name is the request's). Both upper- and lower-case forms fail ('near "database": syntax error ...').
Why: no database created through the Data API can ever be dropped, so database lifecycle and every catalog listing that depends on a drop is wrong, and clean-up leaves state behind.
Fix: DROP DATABASE support in the redshift-data SQL engine's DDL parser plus the database-catalog state it mutates.
Traces: list-databases-pages-name-each-database-once, list-databases-pages-show-a-database-created-and-hide-it-once-dropped, list-databases-through-a-dropped-database-rejected.

Cause 3 of 7: a schema-qualified relation name in DDL is read as a database reference (and folded into one quoted identifier).
AWS: recorded behavior only — `CREATE VIEW <schema>.<view> AS SELECT ... FROM <schema>.<table> WHERE ...` ends FINISHED and the view then appears in ListTables.
Emulator: FAILED with Error `unknown database <schema> in CREATE VIEW <schema>.<view> AS SELECT id FROM "<schema>.<table>" WHERE ... at offset 12`. Two symptoms of one cause: the leading qualifier is resolved as a *database* rather than a schema, and the qualified source relation is re-quoted as the single identifier `"<schema>.<table>"`.
Why: schema-qualified DDL/DML — the normal way to address a Redshift relation — cannot run at all, so any client that qualifies its objects by schema fails.
Fix: identifier resolution in the redshift-data SQL engine's parser/name resolver (schema qualification, and quoting of qualified names).
Traces: list-tables-reflects-created-and-dropped-relations.

Cause 4 of 7: Redshift scalar string functions LPAD/REPEAT are not implemented.
AWS: recorded behavior only — LPAD(string, length, pad) and REPEAT(string, count) are Redshift SQL functions; `CREATE TABLE ... AS SELECT ..., REPEAT(LPAD(CAST(... AS VARCHAR), 8, '0'), 500) ...` ends FINISHED.
Emulator: FAILED with Error `no such function: LPAD in <submitted SQL> at offset 93` (fixed text 'no such function: ' + the function name + ' in ' + the SQL + ' at offset <n>').
Why: ordinary Redshift SQL is rejected, so data generation and any query using these built-ins cannot run; the failure is also reported with a SQLite phrasing AWS never emits.
Fix: the scalar-function registry of the redshift-data SQL engine.
Traces: get-statement-result-v2-csv-pages-hold-every-row-once.

Cause 5 of 7: `current_user` does not resolve.
AWS: recorded behavior only — `select current_user` ends FINISHED and returns one row holding the database user the statement ran as (the user named by the SecretArn, or the IAM-derived user).
Emulator: FAILED with Error `no such column: current_user in select current_user at offset 7`.
Why: the connected identity is unobservable, so a client cannot confirm which database user its SecretArn/DbUser authentication mapped to.
Fix: the session/identity context of the redshift-data SQL engine — expose current_user (and the same niladic session functions) bound to the statement's resolved database user.
Traces: secret-arn-statement-runs-as-the-secret-database-user.

Cause 6 of 7: a cast to a type Redshift does not have succeeds instead of failing.
AWS: recorded behavior only — Redshift has no uuid type, so `SELECT 1::uuid` ends FAILED with the engine's own error, HasResultSet false, and GetStatementResult on it is then refused ResourceNotFoundException with the message 'Query does not have result. Please check query status with DescribeStatement.'
Emulator: the statement ends Status 'FINISHED' with HasResultSet true, ResultRows 1 — the impossible cast is accepted and a row is served.
Why: a client gets a fabricated result for SQL real Redshift rejects; type errors are silently swallowed, so wrong data flows downstream instead of a visible failure.
Fix: type resolution/cast validation in the redshift-data SQL engine (unknown target type must fail the statement, which also makes the dependent GetStatementResult refusal reachable).
Traces: get-statement-result-on-failed-cast-statement-rejected.

Cause 7 of 7: a batch's sub-statement statuses never advance while the batch runs.
AWS: recorded behavior only — while a batch is STARTED, DescribeStatement's SubStatements advance individually: the finished leading statement reads FINISHED and the one being executed reads STARTED, so a caller can see which statement the batch has reached (and cancel it there).
Emulator: the parent reads Status 'STARTED' while every entry in SubStatements stays 'SUBMITTED' with Duration 0 for the whole wait (29 s observed), so the batch never reports reaching its slow statement.
Why: per-statement progress inside a batch is unobservable; a client that waits for a given sub-statement, or cancels once a batch reaches a particular statement, waits forever.
Fix: the batch executor's per-sub-statement state transitions and the DescribeStatement serializer that reports SubStatements.
Traces: cancelled-transaction-batch-ends-aborted-and-rolls-back-its-finished-insert.

## Evidence
Cause 1 — expected DescribeStatement.Error 'ERROR: relation "sim2real_ac_22a1c7853570_absent" does not exist'; got 'no such table: sim2real_ac_22a1c7853570_absent'. Same in the other check: third sub-statement error 'no such table: sim2real_ac_36d3f502f64f_absent'.
Cause 2 — 'DROP DATABASE sim2real_ldp_a_b4aef9666363' ended 'FAILED': 'near "DATABASE": syntax error in DROP DATABASE sim2real_ldp_a_b4aef9666363 at offset 5'; lower-case form: 'near "database": syntax error in drop database sim2real_lddb_d603e4508f34 at offset 5'. Expected FINISHED.
Cause 3 — 'CREATE VIEW tabs_524889e1916a.big_orders AS SELECT id FROM tabs_524889e1916a.orders WHERE total > 100' ended 'FAILED' with 'unknown database tabs_524889e1916a in CREATE VIEW tabs_524889e1916a.big_orders AS SELECT id FROM "tabs_524889e1916a.orders" WHERE total > 100 at offset 12'; expected FINISHED.
Cause 4 — CREATE TABLE AS SELECT ... REPEAT(LPAD(CAST(... AS VARCHAR), 8, '0'), 500) ended FAILED: 'no such function: LPAD in CREATE TABLE ... at offset 93'; expected FINISHED.
Cause 5 — statement 'select current_user' ended 'FAILED': 'no such column: current_user in select current_user at offset 7'; expected FINISHED with one row naming the secret's database user.
Cause 6 — 'SELECT 1::uuid' returned {'Status': 'FINISHED', 'HasResultSet': True, 'ResultRows': 1, 'ResultSize': 7, ...}; expected Status FAILED with HasResultSet False, then GetStatementResult refused ResourceNotFoundException 'Query does not have result. Please check query status with DescribeStatement.'
Cause 7 — DescribeStatement of batch 458e1bc5-…: parent Status 'STARTED' with SubStatements [{'Status': 'SUBMITTED', 'Duration': 0, QueryString insert …}, {'Status': 'SUBMITTED', 'Duration': 0, QueryString select count(*) … 4-way cross join}] unchanged for 29 s; expected the insert FINISHED and the cross join STARTED.
Smithy grounding: ExecuteStatement/BatchExecuteStatement return 200 with an Id (errors ValidationException 400, ResourceNotFoundException 404, ActiveStatementsExceededException 400 …); the SQL failure is carried in the async DescribeStatement record, so all seven causes are engine-side, not wire-side.

## Steps to Reproduce
1. Create a redshift-serverless namespace + workgroup (baseCapacity 8) and wait for AVAILABLE; use it as the redshift-data target (WorkgroupName + Database).
2. Cause 1: ExecuteStatement 'SELECT * FROM <absent_table>' (or a batch whose last Sql reads an absent table); DescribeStatement and read Error.
3. Cause 2: ExecuteStatement 'CREATE DATABASE d1', then 'DROP DATABASE d1'; DescribeStatement the drop.
4. Cause 3: create schema s and table s.orders, then ExecuteStatement 'CREATE VIEW s.big_orders AS SELECT id FROM s.orders WHERE total > 100'.
5. Cause 4: ExecuteStatement "SELECT REPEAT(LPAD(CAST(1 AS VARCHAR), 8, '0'), 500)".
6. Cause 5: ExecuteStatement 'select current_user' (with SecretArn authentication).
7. Cause 6: ExecuteStatement 'SELECT 1::uuid'; DescribeStatement, then GetStatementResult on the Id.
8. Cause 7: BatchExecuteStatement with ExecutionMode TRANSACTION, Sqls = [a quick insert, a deliberately slow 4-way cross self-join]; poll DescribeStatement and watch SubStatements[*].Status.
