Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds-data.continue-after-timeout-preserves-ddl-past-the-call-timeout
Issue-Shape-Id: com.amazonaws.rdsdata#RdsDataService
Issue-Key: rds-data|ExecuteStatement|DatabaseErrorException
Issue-Commit: 0d5fdb6d7a78988c971aec85849026eed9fa5958

# rds-data: the Data API's PostgreSQL engine has no system catalog — SELECT from pg_catalog.pg_tables is refused with DatabaseErrorException 42P01

> _Note: this issue was identified against the code base at commit `0d5fdb6d7a78988c971aec85849026eed9fa5958`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
One net-new cause.

Cause 1 of 1: ExecuteStatement against an aurora-postgresql cluster cannot read the PostgreSQL system catalog. A plain catalog-reflection query (`SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' AND tablename IN (...) ORDER BY tablename`) is refused, even though a `CREATE TABLE` issued on the same connection immediately before it succeeded.

AWS: Aurora PostgreSQL is PostgreSQL, so the `pg_catalog` schema and its standard views exist in EVERY database, always on the implicit search path, and `pg_catalog.pg_tables` is readable by any authenticated user. A `SELECT` over it returns one row per table in the named schema, with `tablename` delivered in the `stringValue` arm of the Data API record (`pg_tables.tablename` is `name`/text). Reflecting on freshly created objects through `pg_catalog` is the normal way a client confirms DDL took effect. Smithy (com-amazonaws-rdsdata-rdsdataservice, ExecuteStatement): `POST /Execute`, 200 on success with `records`/`columnMetadata`; `DatabaseErrorException` (client, HTTP 400) is modelled for an error the DATABASE raised, so emitting it here asserts PostgreSQL itself rejected the statement — which real Aurora PostgreSQL does not. There is no modelled error for "this engine has no catalog"; the correct response is a 200 with the matching rows (an empty `records` list when nothing matches). AWS does not document any exclusion of system catalogs from the Data API.

Emulator: the call is refused with HTTP 400 `DatabaseErrorException` and the message `ERROR: relation "pg_catalog.pg_tables" does not exist; SQLState: 42P01`. Fixed text of that class is PostgreSQL's undefined-table message (`ERROR: relation "<qualified-name>" does not exist`, SQLState 42P01); the variable part here is the relation name taken from the request's `sql`. The relation named is a built-in system view, so the emulator's SQL engine is reporting its own missing catalog as a user error. The same session created `sim2real_behav_cat_control` successfully one statement earlier, so this is specifically the catalog surface, not DDL or SELECT in general.

Why: a client that uses catalog reflection — to check whether a table exists before creating it, to enumerate tables after a migration, or (as here) to observe whether a statement's effect landed — gets a hard 400 instead of rows. Because the status is a client-side `DatabaseErrorException`, the client concludes its own SQL is invalid and aborts rather than retrying; ORMs and migration tools (Alembic, Flyway, psql-style introspection) issue exactly these queries at startup, so they fail on the first step. It also makes any downstream assertion about statement effects unobservable through the API.

Fix: the rds-data ExecuteStatement SQL execution path for the PostgreSQL engine — the embedded engine's catalog/metadata layer must expose the standard `pg_catalog` schema (at minimum `pg_tables`, with `schemaname`, `tablename` and the remaining modelled columns) and resolve `pg_catalog.`-qualified and unqualified references to it, so the query is answered with a 200 result set instead of being routed to the undefined-relation error path. Relation resolution for catalog names belongs with the engine's schema catalog, not in the Data API error mapping — the error mapping is correct for a genuinely missing user table.

Traces: rds-data.continue-after-timeout-preserves-ddl-past-the-call-timeout. The check's later legs (continueAfterTimeout cut-off timing, StatementTimeoutException message, DDL survival) were never reached, so this issue covers only the catalog-read refusal; re-replaying the check after the fix will reveal whether those legs hold.

## Evidence
Check: rds-data.continue-after-timeout-preserves-ddl-past-the-call-timeout (verdict: error)

Sequence (all on one aurora-postgresql Serverless v2 cluster with the Data API enabled, database `simparitydb`):
  1. ExecuteStatement sql="CREATE TABLE sim2real_behav_cat_control (id INT)" -> succeeded (HTTP 200)
  2. ExecuteStatement sql="SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' AND tablename IN ('sim2real_behav_cat_control','sim2real_behav_cat_cancelled','sim2real_behav_cat_continued') ORDER BY tablename"

Expected (real Aurora PostgreSQL via the Data API): HTTP 200, records = [[{"stringValue": "sim2real_behav_cat_control"}]] — the control table reflected through pg_catalog; rows for tables that do not exist are simply absent, and a no-match query returns an empty records list.

Actual (emulator): HTTP 400
  botocore.errorfactory.DatabaseErrorException: An error occurred (DatabaseErrorException) when calling the ExecuteStatement operation: ERROR: relation "pg_catalog.pg_tables" does not exist; SQLState: 42P01

The immediately preceding CREATE TABLE on the same resourceArn/secretArn/database succeeded, proving the refusal is confined to the system-catalog relation and is not a connectivity, auth or general-SQL failure.

## Steps to Reproduce
1. Create an aurora-postgresql Serverless v2 DB cluster with EnableHttpEndpoint=true and RDS-managed master credentials, plus a db.serverless member; wait for the cluster and member to become available and for the Data API to answer.
2. rds-data ExecuteStatement(resourceArn=<clusterArn>, secretArn=<managed secret ARN>, database="simparitydb", sql="CREATE TABLE sim2real_behav_cat_control (id INT)") — observe HTTP 200.
3. rds-data ExecuteStatement(resourceArn=<clusterArn>, secretArn=<secretArn>, database="simparitydb", sql="SELECT tablename FROM pg_catalog.pg_tables WHERE schemaname = 'public' AND tablename IN ('sim2real_behav_cat_control') ORDER BY tablename").
4. Observe HTTP 400 DatabaseErrorException with message 'ERROR: relation "pg_catalog.pg_tables" does not exist; SQLState: 42P01' instead of a 200 whose records contain {"stringValue": "sim2real_behav_cat_control"}.
5. The unqualified form (`SELECT tablename FROM pg_tables ...`) reproduces the same undefined-relation refusal.
