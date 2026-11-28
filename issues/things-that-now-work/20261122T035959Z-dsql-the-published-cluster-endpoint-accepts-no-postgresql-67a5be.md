Issue-Severity: CRITICAL
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dsql.dsql-occ-write-conflict-rejects-the-losing-commit
Issue-Shape-Id: com.amazonaws.dsql#DSQL
Issue-Key: dsql.dsql-occ-write-conflict-rejects-the-losing-commit|_connect_with_retry#7|Timeout|conn = psycopg.connect(
Issue-Commit: c65ba2a529bc0d324725a72f916017f9d3d2f141

# dsql: the published cluster endpoint accepts no PostgreSQL wire connection, so the OCC write-conflict check times out at connect

## Resolution

The cause is FIXED as far as this service crate serves it: the address a cluster publishes
now answers the PostgreSQL wire AND serves the Aurora DSQL SEMANTICS this report enumerates.

* The wire itself and the auth-token login were opened by
  `issues/things-that-now-work/20261121T070609Z-dsql-no-postgresql-wire-listener-answers-the-published.md`
  (`src/state/cluster_data_plane.rs` over `services/common/src/sql_wire_planes`): a core-owned
  listener on the emulator's own interface, the in-band `SSLRequest` upgrade run by the core's
  TLS upgrader, one in-process SQL engine per cluster, `current_user` -> `admin` on database
  `postgres`.
* A cluster's engine now resolves two writers OPTIMISTICALLY
  (`services/common/src/sql_wire_planes/optimistic_writers.rs`, stated per service through
  `ServedDatabase::concurrency`): each transaction runs on a page-for-page copy of the database
  taken when it opened, so a second `UPDATE` of the same row answers at once instead of waiting,
  exactly one of the two commits stands, and the loser is refused SQLSTATE `40001` with Aurora
  DSQL's own sentence `change conflicts with another transaction (OC000)` at severity `ERROR`,
  leaving no effect — after which the same write, run serially, commits.
* A session reports this engine's own isolation (`default_transaction_isolation` /
  `transaction_isolation` -> `repeatable read`, through `SHOW` and `current_setting()`) and a
  `version()` sentence naming the PostgreSQL version the cluster publishes
  (`src/state/cluster_sql.rs`).
* A feature this engine does not implement is refused under its own class and creates nothing:
  the whole `serial` family (`type "<name>" does not exist`, `42704`), a TEMPORARY relation and
  a `CREATE SEQUENCE` (`0A000`), with `information_schema` reporting only the relations the
  cluster really holds.

Pinned by `services/service-com-amazonaws-dsql-dsql/tests/all/cluster_sql_semantics.rs` (over the
wire, in one process) and by the unit cases of `optimistic_writers.rs`, `cluster_sql.rs` and
`sql_wire_planes/dialect.rs`.

What REMAINS for this check, and it is not this service crate's to serve: the recorded client
dials the published host on PostgreSQL's port 5432, and the cluster's wire answers on the port
the CORE assigned its listener. `Listeners::tcp_plaintext` (`vendor/server/src/listeners.rs:230`,
and the binds behind it at lines 88-107) takes no requested port, a service crate may not bind a
socket of its own, and `vendor/` is read-only from a service fix. That gap is filed as
`issues/things-that-do-not-work/20261122T190000Z-dsql-the-served-cluster-data-plane-is-not-reachable-on-postgresqls-port.md`,
which carries its own replay key and the core change it needs; until it lands this check still
times out inside its connect-retry loop, with every assertion behind the connect now served.

> _Note: this issue was identified against the code base at commit `c65ba2a529bc0d324725a72f916017f9d3d2f141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCluster

## Description
Cause 1 of 1: an Aurora DSQL cluster that the control plane settles to ACTIVE publishes an `endpoint`, but nothing in the emulator speaks the PostgreSQL wire protocol at that address, so a libpq client never gets a session and the whole DSQL data plane is unreachable. Failing check: dsql.dsql-occ-write-conflict-rejects-the-losing-commit (times out inside its connect-retry loop _connect_with_retry, so not one of its SQL-level assertions is ever reached).

NOTE for triage: the open issue 20261023T121409Z-dsql-clusters-report-active-with-a-published-endpoint.md describes this same root cause and even lists this check in its Issue-Trace-Ids, but the replay report did NOT mark this check as owned by it (its recorded replay key names a different retry-loop iteration than the one this run produced), so the check is re-filed here with its current replay key bound. Related open issues on the same data-plane gap: 20261022T193920Z-dsql-publishes-a-cluster-endpoint-but-serves-no.md, 20261107T211056Z-dsql-the-published-cluster-endpoint-accepts-no-postgresql.md, 20261109T235613Z-dsql-publishes-a-cluster-endpoint-on-a-host.md, 20261121T070609Z-dsql-no-postgresql-wire-listener-answers-the-published.md. Close them together; this one is the entry that carries this check's replay key.

AWS: recorded behavior only — the DSQL Smithy model is control-plane only. smithy_query shows no SQL-carrying operation bound to com.amazonaws.dsql#DSQL; the data plane is exposed only as CreateCluster/GetCluster's optional `endpoint` member. The recording shows that endpoint is a live PostgreSQL server: once GetCluster reports status ACTIVE, a libpq client connects to <endpoint> on TCP 5432, database `postgres`, user `admin`, with a DSQL IAM auth token as the password (GenerateDbConnectAdminAuthToken is a client-side presigner needing no server support), over TLS (sslmode=require, ssl_in_use true, negotiated cipher >= 128 key bits), and gets a working session in which SELECT version() reports a server string beginning "PostgreSQL ", SELECT current_user returns 'admin', and SELECT current_setting('default_transaction_isolation') returns 'repeatable read' (DSQL's default, not stock PostgreSQL's 'read committed'). In that session DSQL's documented semantics hold: two transactions updating the same row do not block each other (the second UPDATE returns at once rather than waiting on a row lock), exactly one of the two commits is refused, and that refusal arrives as an ErrorResponse at severity ERROR carrying SQLSTATE 40001 (serialization_failure) in the diagnostic record — the loser's write leaves no effect, and re-running it serially succeeds. DSQL also refuses features stock PostgreSQL accepts: SERIAL columns with SQLSTATE 42704 (undefined_object, DSQL has no sequences) and CREATE TEMP TABLE with SQLSTATE 0A000 (feature_not_supported), in both cases leaving no row in information_schema.tables. Those SQLSTATEs and the ERROR severity are the requirement; the table and payload names in the recording are values the check supplies, not part of it.

Emulator: the control plane mints, persists and reports an endpoint and settles the cluster to ACTIVE, but no listener answers it. The check's libpq connect is retried to its budget and never accepted, and the harness kills the check: "Error: Check timed out after 60.0s", with the failure inside _connect_with_retry's retry loop. Nothing downstream of the connection — the TLS requirement, current_user == admin, the 'repeatable read' default, the non-blocking second UPDATE, the single 40001 commit refusal and its diag severity/SQLSTATE, the absent effect of the loser, the serial retry, and the 42704 / 0A000 refusals — is reached or observable. This is not a missing runner tool (psycopg is importable; the check raises its own AssertionError rather than a skip when a driver or endpoint is unusable) and no assertion depends on a live-account value.

Why: a client that follows the normal DSQL flow (CreateCluster, poll to ACTIVE, presign an auth token, connect, run SQL) hangs on connect instead of either working or failing fast, because the service reports the most misleading state possible: healthy and endpoint-bearing, yet unconnectable. Any application or test that relies on DSQL's distinguishing data-plane behavior — optimistic concurrency with a 40001 loser instead of PostgreSQL lock-waiting, REPEATABLE READ by default, and the refusal of sequences and TEMP tables — gets no answer at all, while a grader looking only at control-plane envelopes sees a passing service.

Fix: the dsql service crate needs a data-plane component mounted alongside the control-plane service module — a PostgreSQL wire-protocol listener reachable at exactly the address the control plane publishes for a cluster (port 5432) that requires TLS, authenticates the password as a DSQL auth token scoped to the connecting cluster's host and the requested action/user, and executes SQL under DSQL's semantics (repeatable-read default, optimistic write-conflict detection raising SQLSTATE 40001 at commit on the losing transaction, 42704 for SERIAL, 0A000 for TEMP tables). The published endpoint must also be an address the emulator actually answers on. Until such a listener exists in the service's module list, none of the per-statement semantics can be reached.

Traces: dsql.dsql-occ-write-conflict-rejects-the-losing-commit — timing out at the connect step, before any SQL assertion.

## Evidence
Expected (recorded AWS): after GetCluster(identifier=<id>) reports status=ACTIVE with an `endpoint`, psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<GenerateDbConnectAdminAuthToken token>, sslmode="require") opens a TLS session (ssl_in_use True, key_bits >= 128) in which SELECT version() starts with "PostgreSQL ", SELECT current_user == 'admin', and SELECT current_setting('default_transaction_isolation') == 'repeatable read'. With session A holding an uncommitted UPDATE of a row, session B's UPDATE of the same row returns in well under 5 s (no lock wait), a third session still reads the pre-update value, and exactly one of the two commits is refused with SQLSTATE 40001 (serialization_failure) — psycopg exc.sqlstate == '40001', exc.diag.sqlstate == '40001', exc.diag.severity == 'ERROR'. The loser leaves no effect (the independent session reads only the survivor's value, count(*) == 1) and re-running it serially then commits. In the same session, CREATE TABLE ... (id SERIAL PRIMARY KEY) is refused SQLSTATE 42704 and CREATE TEMP TABLE is refused SQLSTATE 0A000, neither leaving a row in information_schema.tables, while an ordinary CREATE TABLE succeeds.

Actual (emulator): the control plane answers — CreateCluster returns an identifier and GetCluster settles the cluster to ACTIVE and returns an endpoint — but no TCP/PostgreSQL connection to that endpoint on 5432 is ever accepted. The check reports exactly:
  [mismatch] dsql.dsql-occ-write-conflict-rejects-the-losing-commit
  Error: Check timed out after 60.0s
with the stack inside _connect_with_retry's retry/sleep loop, i.e. the failure is at connect, so no statement was ever executed and none of the SQLSTATE/severity/isolation assertions above produced a value.

## Steps to Reproduce
1. CreateCluster(deletionProtectionEnabled=false, tags={...}).
2. Poll GetCluster(identifier=<id>) until status == "ACTIVE"; note the returned `endpoint`.
3. GenerateDbConnectAdminAuthToken(Hostname=<endpoint>, Region=<region>, ExpiresIn=900) (client-side presigner).
4. psycopg.connect(host=<endpoint>, port=5432, dbname="postgres", user="admin", password=<token>, sslmode="require", connect_timeout=15).
5. Observe the connection is never accepted; the client retries until the check budget expires, where AWS returns a usable TLS session reporting current_user 'admin' and default_transaction_isolation 'repeatable read'.
6. Consequently unreachable: open two non-autocommit sessions, UPDATE the same row from each (B's UPDATE must return in <5 s, proving optimistic rather than lock-based concurrency), commit both and observe exactly one refusal with SQLSTATE 40001 / diag severity ERROR, the loser's write absent to a third session, the serial retry succeeding, and CREATE TABLE ... SERIAL / CREATE TEMP TABLE refused with 42704 / 0A000.
