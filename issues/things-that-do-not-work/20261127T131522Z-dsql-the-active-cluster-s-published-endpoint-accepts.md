Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.concurrent-insert-same-key-second-commit-rejected dsql.explicit-transaction-snapshot-taken-at-begin
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.concurrent-insert-same-key-second-commit-rejected|_open_session#9|Timeout|time.sleep(2)
Issue-Key: dsql.explicit-transaction-snapshot-taken-at-begin|_login#9|Timeout|time.sleep(2)
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dsql: the ACTIVE cluster's published endpoint accepts no PostgreSQL connection on port 5432, so the two transaction-semantics checks are killed before they open a session

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCluster

## Description
Cause 1 of 1: a DSQL cluster that the control plane reports ACTIVE with a published `endpoint` does not accept a PostgreSQL wire connection at that endpoint on TCP 5432, so every check in the `transactions` group that needs a native SQL session never gets one and is killed by the harness while still retrying the connect. Failing check ids: dsql.concurrent-insert-same-key-second-commit-rejected, dsql.explicit-transaction-snapshot-taken-at-begin.

AWS: `CreateCluster`/`GetCluster` return `endpoint`, documented in the service's own model as "The connection endpoint for the created cluster" (the emulator's own `src/state/cluster_data_plane.rs` module docs quote the same line). The `Endpoint` member is a bare host — its pattern admits no `:port` — so a client dials it on PostgreSQL's engine port, 5432, which is exactly what both checks do: `db_host, port = ctx.endpoint_host(host, 5432)` and then libpq `connect(dbname=postgres, user=admin, password=<auth token>, sslmode=require)`. Once `GetCluster` reports `status` ACTIVE, that connect succeeds and the session serves SQL under Aurora DSQL's PostgreSQL-16-compatible, snapshot-isolated, optimistically-concurrent semantics. Behind that wire the two checks assert recorded behavior only (no API model covers SQL semantics): (a) two open transactions inserting the SAME new primary key both run their INSERT without waiting, the first commit wins, and the SECOND commit is refused with SQLSTATE `40001` and primary message `change conflicts with another transaction (OC000)` — fixed text, no variable part — leaving exactly the winner's row, after which a serial retry of that INSERT meets SQLSTATE `23505` with message `duplicate key value violates unique constraint "<table>_pkey"`, where `<table>` is the request's table name and only the constraint name varies; (b) an explicit transaction reads the snapshot taken when `BEGIN` ran, so a row another session commits after `BEGIN` but before the transaction's first statement stays invisible to it and becomes visible only after the transaction ends.

Emulator: both checks are reported `[mismatch] ... Error: Check timed out after 60.0s`. Neither reaches any of the assertions above, because neither opens a session: the connect to the published endpoint on 5432 is refused and the check sits in its 180 s dial-retry loop (`_open_session` / `_login`, `connect_timeout=20`, retry every 2 s) until the harness's 60 s budget kills it. The cluster data plane the emulator does serve is bound on a port the core assigned rather than on 5432 (same observation the already-filed `20261122T190000Z-dsql-the-served-cluster-data-plane-is-not-reachable-on-postgresqls-port.md` records verbatim as `connection to server at "127.0.0.1", port 5432 failed: Connection refused`); the loopback address and the assigned port are per-environment values, the invariant is that the address the client dials is not the address the cluster answers on. No HTTP status or error code is involved — the control-plane calls succeed.

Why: a client that follows the documented contract — poll GetCluster to ACTIVE, read `endpoint`, hand it to libpq with a DSQL auth token — cannot reach the database at all. Application code and tooling built on Aurora DSQL's transaction semantics (OCC retry loops keyed on 40001/OC000, snapshot-at-BEGIN read logic) cannot be exercised against the emulator, and a connection failure is indistinguishable from an outage rather than from a configuration error.

Fix: the cluster data plane's listener/endpoint publication — `services/service-com-amazonaws-dsql-dsql/src/state/cluster_data_plane.rs` and the shared `services/common/src/sql_wire_planes` it builds on — must make the published `endpoint` host reachable on 5432, and the per-cluster SQL engine behind it must then serve the two contracts above (commit-time 40001/OC000 write-conflict refusal with the 23505 serial-retry follow-on, and a snapshot fixed at BEGIN rather than at first statement). Related open issues on this same data plane do NOT carry these two checks' replay keys: `20261122T190000Z-dsql-the-served-cluster-data-plane-is-not-reachable-on-postgresqls-port.md`, `20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md`, `20261107T211056Z-dsql-the-published-cluster-endpoint-accepts-no-postgresql.md`, `20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md`, `20261122T035959Z-dsql-the-published-cluster-endpoint-accepts-no-postgresql-67a5be.md`, `20261109T235613Z-dsql-publishes-a-cluster-endpoint-on-a-host.md`.

Traces: dsql.concurrent-insert-same-key-second-commit-rejected, dsql.explicit-transaction-snapshot-taken-at-begin.

## Evidence
Suite report, service heading `### dsql`:

  [mismatch] dsql.concurrent-insert-same-key-second-commit-rejected
  Error: Check timed out after 60.0s

  [mismatch] dsql.explicit-transaction-snapshot-taken-at-begin
  Error: Check timed out after 60.0s

Expected (AWS): CreateCluster -> poll GetCluster to status ACTIVE with a non-empty `endpoint` -> libpq connect to that host on 5432 (dbname=postgres, user=admin, password=GenerateDbConnectAdminAuthToken(...), sslmode=require) opens a session. Then:
  * concurrent-insert-same-key: concurrent INSERTs of DISTINCT keys both commit (table holds both rows); concurrent INSERTs of the SAME new key both execute, the first COMMIT succeeds and the second is refused with ('40001', 'change conflicts with another transaction (OC000)'); an independent session reads only the winner's row; a serial retry of the refused INSERT is refused with ('23505', 'duplicate key value violates unique constraint "<table>_pkey"'); the refused session can still commit a fresh key.
  * explicit-transaction-snapshot: after the reader's BEGIN, rows the writer commits are invisible to the reader (SELECT returns only the pre-BEGIN row, count stays 1) and all rows are visible after the reader's COMMIT.

Actual (emulator): no SQL session is ever established. Both checks remain inside their 180 s connect-retry loop (psycopg.connect to endpoint:5432, connect_timeout=20) and the harness terminates them at 60 s, so not one assertion above is evaluated. The cluster's served PostgreSQL wire is on a core-assigned port, not 5432; the connect to 5432 is refused.

## Steps to Reproduce
1. CreateCluster(deletionProtectionEnabled=false, tags={...}) and read `identifier`.
2. Poll GetCluster(identifier=...) until `status` == "ACTIVE"; assert `endpoint` is non-empty (both pass today).
3. GenerateDbConnectAdminAuthToken(Hostname=<endpoint>, Region=<region>, ExpiresIn=900).
4. Connect with libpq/psycopg to <endpoint> on port 5432 (dbname=postgres, user=admin, password=<token>, sslmode=require). Observe the connect is refused and retried until the harness kills the check; nothing is listening on 5432 for that cluster.
5. Once step 4 connects, re-replay the two contracts that are currently unreachable:
   a. Open two sessions. CREATE TABLE t(id INT PRIMARY KEY, who TEXT) and commit on session 1. INSERT (10,'first') on session 1 and (11,'second') on session 2; both commits succeed and a SELECT reads both rows. Then INSERT (20,'first') on session 1 and (20,'second') on session 2; commit session 1 (succeeds), commit session 2 -> must be refused with SQLSTATE 40001, message 'change conflicts with another transaction (OC000)'. An independent session must read only ('first',) for id=20 and a total count of 3. Re-running INSERT (20,'second') serially must be refused with SQLSTATE 23505, message 'duplicate key value violates unique constraint "t_pkey"'. INSERT (21,'second') must then commit.
   b. Open two autocommit sessions. Writer: CREATE TABLE t(id INT PRIMARY KEY, label TEXT); INSERT (1,'before-begin'). Reader: BEGIN (no statement yet). Writer: INSERT (2,'after-begin'). Reader's first SELECT must return only (1,'before-begin'); after the writer INSERTs (3,...), the reader's count must still be 1; after the reader's COMMIT, a new SELECT must return all three rows.
