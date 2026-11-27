Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-mysql rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-mysql-nowait-lock-rejected-until-commit rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed
Issue-Key: rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-mysql|_grade#2|AssertionError|assert pending == [[{"longValue": 1}]], (
Issue-Key: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-mysql-nowait-lock-rejected-until-commit|_grade#2|AssertionError|assert message == (
Issue-Key: rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds|_grade#2|AssertionError|assert errno == 1205, (
Issue-Key: rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed|_grade#2|AssertionError|assert b_after == [(1, "new"), (2, "a-only")], (
Issue-Commit: 9f15073fe42cc4661a8a99924cff5eec339d1b59

# rds__rds-data__secretsmanager: 4 net-new causes — the shared cluster has no transaction isolation (uncommitted native writes are visible to the Data API; a READ COMMITTED transaction never sees later commits) and InnoDB lock conflicts are refused with PostgreSQL text and the wrong MySQL error numbers

> _Note: this issue was identified against the code base at commit `9f15073fe42cc4661a8a99924cff5eec339d1b59`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ExecuteStatement

## Description
Four net-new root causes in the transaction semantics of the one database the Data API and the native engine wire now share. All four are reached only now that a native session can connect (the still-open 20261121T083106Z-rds-rds-data-secretsmanager-every-cross-surface-data.md holds the superseded PublishedUrlRejected keys and covers none of these).

Cause 1 of 4: a native session's UNCOMMITTED INSERT is visible to the Data API.
AWS: recorded behavior only for the cross-surface contract; the engines' own rules are the requirement (MySQL 8.0 'Transaction Isolation Levels': InnoDB's default REPEATABLE READ never exposes another transaction's uncommitted rows). On Aurora MySQL, while a native autocommit=False session holds an uninserted-yet-uncommitted row, rds-data ExecuteStatement("SELECT id, label FROM t ORDER BY id") returns HTTP 200 with ONLY the committed rows; after the native session's ROLLBACK the row is gone from both surfaces.
Emulator: ExecuteStatement returns 200 but the result set INCLUDES the native session's uncommitted row — the check's read of the table returned both the committed baseline row and the pending row (two records where one is committed). The number of rows and their ids are per-run values; the requirement is that uncommitted rows of another session are absent. No error and no status divergence: the isolation is simply missing (dirty read).
Why: a client reading through the Data API sees rows that may never exist, so any read-decide-write workflow mixing a driver and the Data API acts on phantom data; a rolled-back migration appears applied.
Fix: the shared SQL engine's transaction visibility layer — the per-session transaction/snapshot state of the RDS data plane engine (services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/) and the Data API's read path onto the same database (services/service-com-amazonaws-rdsdata-rdsdataservice): an uncommitted write must be private to the session that made it, whichever surface reads.
Traces: rds__rds-data__secretsmanager.data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-mysql.

Cause 2 of 4: the MySQL NOWAIT lock refusal carries PostgreSQL's wording instead of ER_LOCK_NOWAIT's.
AWS: recorded behavior only for the wire; MySQL 8.0 'Locking Reads' is the rule. On Aurora MySQL, `SELECT ... FOR UPDATE NOWAIT` on a row another transaction holds is refused with error 3572 (ER_LOCK_NOWAIT) and the fixed message `Statement aborted because lock(s) could not be acquired immediately and NOWAIT is set.` — that sentence is entirely fixed text: it names no table, row or value.
Emulator: the error number 3572 is right, but the message is `could not obtain lock on row in relation "sim2real_seats"` — PostgreSQL's 55P03 lock_not_available wording, with the table name from the request interpolated. MySQL never names the relation in this error and never uses the phrase 'relation'.
Why: a client matching MySQL's documented message (as drivers, retry wrappers and test suites do) does not recognise the refusal, and the text again discloses that a PostgreSQL-shaped engine is answering a MySQL cluster.
Fix: the MySQL wire error mapping of the shared engine's lock manager (services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/, the path that turns an engine lock-conflict into a wire error for a mysql endpoint) — one engine condition, per-dialect text.
Traces: rds__rds-data__secretsmanager.data-api-row-lock-makes-a-native-mysql-nowait-lock-rejected-until-commit.

Cause 3 of 4: a blocking native UPDATE is cancelled by the endpoint's own statement budget (error 1317) instead of timing out on the InnoDB lock wait (error 1205).
AWS: recorded behavior only for the wire; MySQL 8.0 innodb_lock_wait_timeout is the rule. With `SET SESSION innodb_lock_wait_timeout = 1`, an UPDATE of a row an open transaction holds is refused after ~1s with error 1205 (ER_LOCK_WAIT_TIMEOUT) and the fixed message `Lock wait timeout exceeded; try restarting transaction` — again wholly fixed text — and it writes nothing: the held row keeps the holder's value until that transaction commits.
Emulator: the session variable reads back as 1 (that assertion passes), but the UPDATE is refused with error 1317 (ER_QUERY_INTERRUPTED) and the emulator's own sentence `canceling statement: it ran longer than this endpoint serves`. That text is authored by the emulator — it is neither MySQL's 1317 message (`Query execution was interrupted`) nor 1205's — and it says the endpoint gave up, not that a lock wait expired.
Why: lock contention is the thing clients retry on; keyed on 1205 they will not retry, and keyed on 1317 they will treat a normal lock conflict as a cancelled/killed query. The configured innodb_lock_wait_timeout has no observable effect.
Fix: the lock-wait path of the shared engine plus its MySQL error mapping (same component as Cause 2): a statement blocked on a row lock must wait up to the session's innodb_lock_wait_timeout and then raise the engine's lock-wait-timeout condition, mapped to 1205 with MySQL's sentence, rather than being swept up by the endpoint's generic statement-duration cancellation.
Traces: rds__rds-data__secretsmanager.mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds.

Cause 4 of 4: under READ COMMITTED a session's later statement in the same open transaction does not see what another session committed — the snapshot is pinned at transaction start.
AWS: recorded behavior only for the wire; the rule is PostgreSQL's documented READ COMMITTED (the Aurora PostgreSQL default, and what `SHOW transaction_isolation` reports): each statement sees a snapshot taken at the START OF THAT STATEMENT, so once session A commits, session B's NEXT statement inside its still-open transaction sees A's committed rows.
Emulator: `SHOW transaction_isolation` correctly reports 'read committed' and B correctly does not see A's uncommitted writes, but after A commits, B's next SELECT in the same open transaction still returns the pre-commit snapshot — one row with the old value, missing both A's UPDATE and A's inserted row. That is REPEATABLE READ behavior under a READ COMMITTED label. The row values are per-run; the requirement is that B's post-commit statement observes A's committed effects.
Why: the engine advertises one isolation level and implements a stricter one, so read-your-neighbour's-commit patterns (queue pollers, leader election, polling for a row another worker inserts) never make progress against the emulator while they work on Aurora, and no retry or timeout diagnoses it.
Fix: the snapshot acquisition of the shared engine's MVCC/visibility layer (services/service-com-amazonaws-rds-amazonrdsv19/src/state/data_plane/) — under READ COMMITTED take the snapshot per statement, not per transaction, and keep the reported transaction_isolation consistent with the implemented rule.
Traces: rds__rds-data__secretsmanager.native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed.

## Evidence
Replay of the behavioral suite on the cloned branch; group rds__rds-data__secretsmanager scored 14/23 (61%).

Cause 1 — data-api-rollback-and-a-native-rollback-leave-no-trace-on-aurora-mysql (mismatch, check line 352):
AssertionError: the Data API must not see an uncommitted native INSERT; got [[{'longValue': 1}], [{'longValue': 3}]] (expected [[{'longValue': 1}]]).

Cause 2 — data-api-row-lock-makes-a-native-mysql-nowait-lock-rejected-until-commit (mismatch, check line 337):
Expected errno 3572 with message 'Statement aborted because lock(s) could not be acquired immediately and NOWAIT is set.'
Actual: AssertionError: unexpected 3572 message: 'could not obtain lock on row in relation "sim2real_seats"'. The preceding assertions pass: an unlocked neighbouring row IS lockable with NOWAIT and the errno IS 3572 — only the message diverges.

Cause 3 — mysql-lock-wait-timeout-rejects-an-update-of-a-row-a-data-api-transaction-holds (mismatch, check line 370):
Expected errno 1205 with 'Lock wait timeout exceeded; try restarting transaction'.
Actual: AssertionError: an UPDATE of a row the Data API transaction holds must time out with 1205; got 1317: 'canceling statement: it ran longer than this endpoint serves'. SET SESSION innodb_lock_wait_timeout = 1 and its read-back assertion both passed first.

Cause 4 — native-uncommitted-write-is-invisible-to-a-second-session-under-read-committed (mismatch, check line 370):
Expected after A's commit: B's next statement returns [(1, 'new'), (2, 'a-only')].
Actual: AssertionError: under READ COMMITTED, B's next statement in the same open transaction must see what A committed; got [(1, 'old')]. The earlier assertions pass — SHOW transaction_isolation = 'read committed', A reads its own uncommitted writes, and neither B nor the Data API sees them before the commit.

## Steps to Reproduce
1. Boot the emulator; point rds, rds-data, secretsmanager and ec2 clients at it. For each cause create an Aurora Serverless v2 cluster (Engine aurora-mysql for causes 1-3, aurora-postgresql for cause 4) with DatabaseName="simparitydb", MasterUsername="simparity", ManageMasterUserPassword=True, EnableHttpEndpoint=True, plus a db.serverless writer; wait for 'available' and read the managed secret for the native login.
2. Cause 1: through the Data API create `t (id INT PRIMARY KEY, label VARCHAR(64) NOT NULL) ENGINE=InnoDB` and commit one row. Open a pymysql session with autocommit=False, INSERT a second row and do NOT commit. Now call rds-data ExecuteStatement("SELECT id, label FROM t ORDER BY id") — expect only the committed row; observe the uncommitted row included.
3. Cause 2: create `seats (id INT PRIMARY KEY, holder VARCHAR(16) NOT NULL) ENGINE=InnoDB` with rows 1 and 2. BeginTransaction + ExecuteStatement("UPDATE seats SET holder='data-api' WHERE id=1", transactionId=...). From a native session run `SELECT holder FROM seats WHERE id=1 FOR UPDATE NOWAIT` — expect 3572 with MySQL's NOWAIT sentence; observe 3572 with 'could not obtain lock on row in relation "seats"'.
4. Cause 3: same setup; in the native session `SET SESSION innodb_lock_wait_timeout = 1`, then `UPDATE seats SET holder='native' WHERE id=1` while the Data API transaction holds row 1 — expect 1205 'Lock wait timeout exceeded; try restarting transaction'; observe 1317 'canceling statement: it ran longer than this endpoint serves'.
5. Cause 4: on the aurora-postgresql cluster create `v (id INT PRIMARY KEY, v TEXT NOT NULL)` with row (1,'old'). Open psycopg sessions A and B with autocommit=False. In A: UPDATE row 1 to 'new' and INSERT (2,'a-only'). In B: SELECT (sees only (1,'old') — correct). Commit A. Re-run the SELECT in B's still-open transaction — expect [(1,'new'),(2,'a-only')]; observe [(1,'old')].
