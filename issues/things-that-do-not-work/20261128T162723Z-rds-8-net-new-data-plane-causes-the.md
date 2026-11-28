Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.mysql-control-flow-and-string-functions rds.mysql-enum-column-stores-members-and-rejects-others rds.mysql-insert-ignore-skips-duplicate-keys rds.mysql-select-from-dual rds.mysql-duplicate-primary-key-rejects-the-whole-multi-row-insert rds.postgres-sequence-nextval-advances rds.postgres-gen-random-uuid-column-default rds.postgres-serial-column-assigns-increasing-ids rds.postgres-varchar-and-smallint-limits-rejected rds.postgres-date-plus-integer-adds-days rds.postgres-failed-statement-aborts-the-transaction-until-rollback-to-savepoint rds.mysql-table-privilege-denied-until-granted rds.postgres-table-privilege-denied-until-granted
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.mysql-control-flow-and-string-functions|_rows#4|AssertionError|raise AssertionError(f"the engine refused {statement!r}: {exc!r}") from exc
Issue-Key: rds.mysql-enum-column-stores-members-and-rejects-others|_rows#4|AssertionError|raise AssertionError(f"the engine refused {statement!r}: {exc!r}") from exc
Issue-Key: rds.mysql-insert-ignore-skips-duplicate-keys|run#4|AssertionError|raise AssertionError(f"INSERT IGNORE was refused: {exc!r}") from exc
Issue-Key: rds.mysql-select-from-dual|_rows#4|AssertionError|raise AssertionError(f"the engine refused {statement!r}: {exc!r}") from exc
Issue-Key: rds.mysql-duplicate-primary-key-rejects-the-whole-multi-row-insert|run#6|AssertionError|assert clash == (1062, f"Duplicate entry '2' for key '{_TABLE}.PRIMARY'"), (
Issue-Key: rds.postgres-sequence-nextval-advances|_rows#4|AssertionError|raise AssertionError(f"the engine refused {statement!r}: {exc!r}") from exc
Issue-Key: rds.postgres-gen-random-uuid-column-default|_rows#4|AssertionError|raise AssertionError(f"the engine refused {statement!r}: {exc!r}") from exc
Issue-Key: rds.postgres-serial-column-assigns-increasing-ids|run#4|AssertionError|assert first == [(1,)], f"the first SERIAL id returned was {first!r}"
Issue-Key: rds.postgres-varchar-and-smallint-limits-rejected|_sqlstate#4|AssertionError|raise AssertionError(f"expected {statement!r} to be refused, but it returned {rows!r}")
Issue-Key: rds.postgres-date-plus-integer-adds-days|run#4|AssertionError|assert got == [("2024-02-01", "2024-03-01")], f"date + integer returned {got!r}"
Issue-Key: rds.postgres-failed-statement-aborts-the-transaction-until-rollback-to-savepoint|_failure#10|AssertionError|raise AssertionError(f"the statement was accepted, returning {rows!r}: {sql}")
Issue-Key: rds.mysql-table-privilege-denied-until-granted|_login#8|OperationalError|return pymysql.connect(host=host, port=port, user=user, password=password,
Issue-Key: rds.postgres-table-privilege-denied-until-granted|_login#9|Timeout|time.sleep(min(2 ** attempt, 30))
Issue-Commit: f2a96a9f1fdbdd444ddfeaed949096426fea7442

# rds: 8 net-new data-plane causes — the served MySQL/PostgreSQL engines are not those dialects (functions, ENUM, INSERT IGNORE, DUAL, sequences, SERIAL, gen_random_uuid, date arithmetic), type/constraint limits and transaction abort are not enforced, and CREATE USER/ROLE + GRANT are not honoured

> _Note: this issue was identified against the code base at commit `f2a96a9f1fdbdd444ddfeaed949096426fea7442`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateDBInstance

## Description
Eight net-new root causes in the data plane the RDS emulator serves on a DB instance's published endpoint, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout. A fixer must close ALL EIGHT before submitting; each is numbered with its own failing check ids. None of these checks was reported blocked by an unimplemented stub, and none is marked OWNED by an open issue. They share one fault shape — the engine behind `data_plane::sql_engine` is a generic/SQLite-like SQL executor, not the MySQL and PostgreSQL dialects the instance's `Engine` claims — but each cause below is a separately observable, separately fixable behaviour.

---
Cause 1 of 8: an RDS for MySQL instance refuses core MySQL functions and operators with a syntax error.
AWS: recorded behavior only — the Smithy model (services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/com-amazonaws-rds-amazonrdsv19.json) models only the control plane (CreateDBInstance with Engine/EngineVersion and the DBInstance.Endpoint the client connects to); the SQL dialect of a served engine is fixed by the engine, not by the model. An `Engine="mysql"` instance executes `SELECT IF(2 > 1, 'yes', 'no'), DATE_FORMAT('2024-05-06', '%Y/%m'), 7 DIV 2` and returns one row ('yes', '2024/05', 3).
Emulator: the statement is refused over the MySQL wire protocol with error 1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '2' at line 1" — the parser does not know `IF()`, `DATE_FORMAT()` or the integer-division operator `DIV`.
Why: every client that speaks MySQL to an RDS for MySQL endpoint — an ORM, a migration tool, a report query — gets a syntax error for valid MySQL. The failure is indistinguishable from an application bug, and no application SQL of any complexity can be exercised against the emulator.
Fix: the SQL dialect/expression layer of the served MySQL engine (the `data_plane::sql_engine` component referenced from services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs): parse and evaluate the MySQL function and operator set for an instance whose Engine is mysql/mariadb.
Traces: rds.mysql-control-flow-and-string-functions.

---
Cause 2 of 8: the same MySQL engine refuses MySQL-specific DDL/DML forms — an ENUM column type, INSERT IGNORE, and SELECT ... FROM DUAL.
AWS: recorded behavior only. On RDS for MySQL: `CREATE TABLE t (id INT PRIMARY KEY, s ENUM('new','done') NOT NULL)` is accepted and stores only declared members; `INSERT IGNORE INTO t VALUES (...)` skips rows that would violate a unique key instead of failing the statement; `SELECT 1 + 1 AS two FROM DUAL` returns 2 (DUAL is a MySQL pseudo-table, not a real table).
Emulator: all three are refused — ENUM: 1064 "...right syntax to use near ''new','done') NOT NULL)' at line 1"; INSERT IGNORE: 1064 "...right syntax to use near 'IGNORE INTO sim2real_kv VALUES (1, 'second'), (2, 'other')' at line 1"; FROM DUAL: 1146 "Table 'simparitydb.DUAL' doesn't exist" (the engine resolves DUAL as an ordinary table).
Why: schema creation itself fails, so the check cannot even reach its assertions; an application whose schema uses ENUM, or whose upsert path uses INSERT IGNORE, cannot be deployed against the emulator at all, and `FROM DUAL` (emitted by many MySQL drivers and SQL generators for constant selects) fails with a table-not-found error that misdirects the reader to a missing table.
Fix: same component as Cause 1 — the MySQL DDL/DML grammar and the DUAL pseudo-table resolution in the served MySQL engine.
Traces: rds.mysql-enum-column-stores-members-and-rejects-others, rds.mysql-insert-ignore-skips-duplicate-keys, rds.mysql-select-from-dual.

---
Cause 3 of 8: a MySQL duplicate-key violation is reported with the underlying store's sentence instead of MySQL's "Duplicate entry" message.
AWS: recorded behavior only. A multi-row INSERT repeating a primary-key value is refused whole, with MySQL error 1062 and message "Duplicate entry '<value>' for key '<table>.PRIMARY'" — fixed text "Duplicate entry '", "' for key '", ".PRIMARY'"; the variable parts are the duplicated key value and the table name, both taken from the request and the schema (SQLSTATE 23000).
Emulator: the errno is right (1062) but the message is the backing store's — "UNIQUE constraint failed: s2r_parts.id" — naming the column, not the key, and in SQLite's wording.
Why: clients and drivers parse this message to identify which key collided (upsert/retry logic, user-facing "record already exists" handling); the emulator's sentence matches no MySQL pattern, so that logic falls through to a generic failure. The same wrong sentence reaches API surfaces built on the data plane (see the Data-API/AppSync duplicate-key divergence already filed as 20261127T155934Z-appsync-rds-4-net-new-causes-not-filter.md, which covers those checks and not this native-session one).
Fix: the error-mapping layer of the served MySQL engine — translate a unique/primary-key violation into MySQL's 1062/23000 envelope with the "Duplicate entry ... for key '<table>.PRIMARY'" template built from the violated key, not pass the store's text through.
Traces: rds.mysql-duplicate-primary-key-rejects-the-whole-multi-row-insert.

---
Cause 4 of 8: an RDS for PostgreSQL instance refuses core PostgreSQL DDL — CREATE SEQUENCE, a DEFAULT gen_random_uuid() column, and SERIAL does not assign ids.
AWS: recorded behavior only. On RDS for PostgreSQL (13+): `CREATE SEQUENCE s START 100 INCREMENT 10` is accepted and `nextval('s')` returns 100, 110, ...; `CREATE TABLE t (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), t text)` is accepted (pgcrypto's gen_random_uuid is built in from PG13) and an INSERT omitting id gets a generated uuid; a `SERIAL` column assigns 1, 2, 3 ... and `INSERT ... RETURNING id` returns the assigned value.
Emulator: CREATE SEQUENCE is refused with SyntaxError 'syntax error at or near "SEQUENCE"'; the gen_random_uuid default is refused with SyntaxError 'syntax error at or near "("' (a function call is not accepted in a column DEFAULT); and a SERIAL column yields no value at all — the first inserted row returns `[(None,)]` where AWS returns `[(1,)]`.
Why: these are the ordinary ways a PostgreSQL schema generates keys. The DDL fails outright, or worse, succeeds and then silently produces NULL identifiers — a client inserting rows gets NULL primary keys instead of a sequence, which corrupts every later read and foreign-key use while reporting no error.
Fix: the PostgreSQL dialect layer of `data_plane::sql_engine` — sequence objects (CREATE SEQUENCE/nextval/currval), function calls in column DEFAULTs including gen_random_uuid(), and SERIAL/BIGSERIAL as an implicit sequence-backed default.
Traces: rds.postgres-sequence-nextval-advances, rds.postgres-gen-random-uuid-column-default, rds.postgres-serial-column-assigns-increasing-ids.

---
Cause 5 of 8: PostgreSQL column type limits are not enforced — a value longer than varchar(n) and a value outside smallint's range are stored instead of refused.
AWS: recorded behavior only; PostgreSQL refuses `INSERT INTO t VALUES ('abcd', 1)` into a `varchar(3)` column with SQLSTATE 22001 ("value too long for type character varying(3)"), and a smallint outside -32768..32767 with SQLSTATE 22003 ("smallint out of range").
Emulator: the INSERT is accepted and returns no error at all (the check's "expected ... to be refused, but it returned []"), so the row lands with an over-long value.
Why: the type declaration is decoration. Data that AWS would never admit is persisted, and a client relying on the database as the last line of validation (length-limited identifiers, bounded counters) silently accepts bad input; the 22001/22003 paths are unreachable.
Fix: the type/constraint enforcement stage of the PostgreSQL side of `data_plane::sql_engine` — apply declared length and numeric range on write and raise the matching SQLSTATE.
Traces: rds.postgres-varchar-and-smallint-limits-rejected.

---
Cause 6 of 8: PostgreSQL date arithmetic is wrong — `date + integer` returns a number, not a date shifted by that many days.
AWS: recorded behavior only; in PostgreSQL `date + integer` yields a date (e.g. '2024-01-01'::date + 31 = 2024-02-01, + 60 = 2024-03-01), which the check reads back as [('2024-02-01', '2024-03-01')].
Emulator: the same query returns [('2025', '2026')] — the date operand is coerced to a number (its year) and the integer added to it, so the result is neither a date nor the right value.
Why: the query succeeds and returns plausible-looking values, so there is no error to catch: any scheduling, retention or expiry computation done in SQL returns wrong data silently.
Fix: the PostgreSQL type system and operator resolution in `data_plane::sql_engine` — `date + integer`/`date - integer` must resolve to the date interval operators, not numeric addition.
Traces: rds.postgres-date-plus-integer-adds-days.

---
Cause 7 of 8: a failed statement inside a PostgreSQL transaction does not abort the transaction — subsequent statements are accepted instead of being refused until ROLLBACK (TO SAVEPOINT).
AWS: recorded behavior only; PostgreSQL marks a transaction as aborted after any statement error: every following command in that transaction is refused with SQLSTATE 25P02, "current transaction is aborted, commands ignored until end of transaction block", until ROLLBACK or ROLLBACK TO SAVEPOINT.
Emulator: after the failing statement, `INSERT INTO s2r_keys VALUES (20)` is accepted and returns [] — the session continues in a normal, non-aborted transaction.
Why: this is the single most load-bearing PostgreSQL transaction rule. Code that deliberately provokes an error inside a transaction (an upsert probe, a constraint test) and then expects 25P02 before rolling back to a savepoint takes the wrong branch, and work that AWS would discard is committed.
Fix: the transaction/session state machine of the PostgreSQL side of `data_plane::sql_engine` — set the aborted flag on any statement error and refuse every later statement with 25P02 until ROLLBACK or ROLLBACK TO SAVEPOINT clears it.
Traces: rds.postgres-failed-statement-aborts-the-transaction-until-rollback-to-savepoint.

---
Cause 8 of 8: engine-level users/roles and grants are not honoured — a user the master session creates cannot authenticate, and table privileges are never enforced.
AWS: recorded behavior only (AWS documents the master-account model at https://docs.aws.amazon.com/AmazonRDS/latest/UserGuide/UsingWithRDS.MasterAccounts.html and the PostgreSQL role tasks at .../Appendix.PostgreSQL.CommonDBATasks.Roles.html). On MySQL, `CREATE USER '<app>'@'%' IDENTIFIED BY '<pw>'` followed by `GRANT SELECT ON <schema>.<table> TO '<app>'@'%'` produces a user that logs in, reads the granted table, and is refused other access with errno 1142 and "SELECT command denied to user '<user>'@'<host>' for table '<table>'" (fixed text "<VERB> command denied to user '", "'@'", "' for table '", "'"; variable parts are the verb, user, host and table from the session and request). On PostgreSQL, `CREATE ROLE <app> LOGIN PASSWORD '...'` produces a role that connects, whose ungranted access is refused with SQLSTATE 42501 and "permission denied for table <table>", and that gains exactly what GRANT SELECT / GRANT INSERT give it.
Emulator: the master session accepts CREATE USER/CREATE ROLE and GRANT without error, but the created principal cannot authenticate. MySQL: connecting as that user fails with OperationalError (1045, "Access denied for user 's2r_app'@'127.0.0.1' (using password: YES)"). PostgreSQL: the connection as the created role never reaches a server that answers (no SQLSTATE), so the check's bounded connect-retry loop consumes the whole budget and the check ends in a timeout rather than a verdict. In both cases only the master credentials authenticate and no privilege is ever checked, so the 1142 / 42501 denial classes are unreachable.
Why: least-privilege application users are the normal way to use RDS. Against the emulator the only usable identity is the master user, so an application that connects as its own user cannot connect at all, and a security test asserting that an ungranted read is denied gets a connection failure (or no answer) instead of the denial.
Fix: the authentication and privilege layer of the served engines (`data_plane::sql_engine` plus the credential store the native MySQL/PostgreSQL listeners authenticate against) — register users/roles created through CREATE USER / CREATE ROLE with their passwords, admit their logins, and evaluate GRANT/REVOKE per table and verb, raising MySQL 1142 and PostgreSQL 42501 with the sentences above.
Traces: rds.mysql-table-privilege-denied-until-granted, rds.postgres-table-privilege-denied-until-granted.

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replayed against the emulator built from this checkout: 613/752 passed (81.5%), 121 mismatched, 18 errored; service rds 348/438 (79%), 90 failed. The failures below are the rds data-plane ones the report returns with no owning issue and no blocking stub. Hosts, ports, generated instance/table ids and timestamps in the excerpts are per-run values and are not part of any requirement.

Cause 1 — [mismatch] rds.mysql-control-flow-and-string-functions:
  AssertionError: the engine refused "SELECT IF(2 > 1, 'yes', 'no'), DATE_FORMAT('2024-05-06', '%Y/%m'), 7 DIV 2": ProgrammingError(1064, "You have an error in your SQL syntax; check the manual that corresponds to your MySQL server version for the right syntax to use near '2' at line 1")

Cause 2 — [mismatch] rds.mysql-enum-column-stores-members-and-rejects-others:
  AssertionError: the engine refused "CREATE TABLE sim2real_status (id INT PRIMARY KEY, s ENUM('new','done') NOT NULL)": ProgrammingError(1064, "...right syntax to use near ''new','done') NOT NULL)' at line 1")
[mismatch] rds.mysql-insert-ignore-skips-duplicate-keys:
  AssertionError: INSERT IGNORE was refused: ProgrammingError(1064, "...right syntax to use near 'IGNORE INTO sim2real_kv VALUES (1, 'second'), (2, 'other')' at line 1")
[mismatch] rds.mysql-select-from-dual:
  AssertionError: the engine refused 'SELECT 1 + 1 AS two FROM DUAL': ProgrammingError(1146, "Table 'simparitydb.DUAL' doesn't exist")

Cause 3 — [mismatch] rds.mysql-duplicate-primary-key-rejects-the-whole-multi-row-insert:
  assert clash == (1062, f"Duplicate entry '2' for key '{_TABLE}.PRIMARY'")
  AssertionError: a multi-row INSERT repeating key 2 must be refused with 1062 naming the key; got (1062, 'UNIQUE constraint failed: s2r_parts.id')

Cause 4 — [mismatch] rds.postgres-sequence-nextval-advances:
  AssertionError: the engine refused 'CREATE SEQUENCE sim2real_seq START 100 INCREMENT 10': SyntaxError('syntax error at or near "SEQUENCE"')
[mismatch] rds.postgres-gen-random-uuid-column-default:
  AssertionError: the engine refused 'CREATE TABLE sim2real_docs (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), t text)': SyntaxError('syntax error at or near "("')
[mismatch] rds.postgres-serial-column-assigns-increasing-ids:
  assert first == [(1,)]  ->  AssertionError: the first SERIAL id returned was [(None,)]

Cause 5 — [mismatch] rds.postgres-varchar-and-smallint-limits-rejected:
  AssertionError: expected "INSERT INTO sim2real_limits VALUES ('abcd', 1)" to be refused, but it returned []

Cause 6 — [mismatch] rds.postgres-date-plus-integer-adds-days:
  assert got == [("2024-02-01", "2024-03-01")]  ->  AssertionError: date + integer returned [('2025', '2026')]

Cause 7 — [mismatch] rds.postgres-failed-statement-aborts-the-transaction-until-rollback-to-savepoint:
  AssertionError: the statement was accepted, returning []: INSERT INTO s2r_keys VALUES (20)
(the check's _failure() helper raises only when a statement AWS refuses is admitted)

Cause 8 — [error] rds.mysql-table-privilege-denied-until-granted:
  pymysql.err.OperationalError: (1045, "Access denied for user 's2r_app'@'127.0.0.1' (using password: YES)")
raised on the login that follows a CREATE USER + GRANT SELECT the same admin session accepted; the check's graded expectations are (1142, "SELECT command denied to user 's2r_app'@'<host>' for table 's2r_payroll'") and the matching INSERT denial.
[mismatch] rds.postgres-table-privilege-denied-until-granted: "Check timed out after 60.0s". This check has no long fixed wait: its only waits are ctx-bounded polls plus _login(), which retries a connect ONLY while the failure carries no SQLSTATE (no server answered) up to 10 attempts with exponential backoff. The connection as the freshly created role `s2r_clerk` is what burns the budget; its graded expectation is ("42501", "permission denied for table s2r_ledger").

## Steps to Reproduce
Common setup for every cause: CreateDBInstance (Engine="mysql" for causes 1-3 and the MySQL half of 8, Engine="postgres" for causes 4-7 and the PostgreSQL half of 8) with a DB subnet group over the run's network, wait for DBInstanceStatus=available, then connect to the published Endpoint.Address/Port with the native driver (pymysql / psycopg) as the master user.
1. Cause 1: run `SELECT IF(2 > 1, 'yes', 'no'), DATE_FORMAT('2024-05-06', '%Y/%m'), 7 DIV 2`. AWS: one row ('yes','2024/05',3). Emulator: 1064 syntax error.
2. Cause 2: run `CREATE TABLE t (id INT PRIMARY KEY, s ENUM('new','done') NOT NULL)`, then `INSERT IGNORE INTO t2 VALUES (1,'second'),(2,'other')` over a table holding key 1, then `SELECT 1 + 1 AS two FROM DUAL`. AWS accepts all three (the IGNORE form skipping the duplicate row). Emulator: 1064, 1064, 1146.
3. Cause 3: create a table with an INT PRIMARY KEY, then run one multi-row INSERT repeating a key value. AWS: errno 1062, "Duplicate entry '2' for key '<table>.PRIMARY'", nothing inserted. Emulator: errno 1062 with 'UNIQUE constraint failed: <table>.<column>'.
4. Cause 4: run `CREATE SEQUENCE s START 100 INCREMENT 10` and `SELECT nextval('s')`; run `CREATE TABLE d (id uuid PRIMARY KEY DEFAULT gen_random_uuid(), t text)` and insert without id; create a table with a SERIAL column and `INSERT ... RETURNING id`. AWS: 100; a generated uuid; 1. Emulator: syntax error at "SEQUENCE"; syntax error at "("; [(None,)].
5. Cause 5: `CREATE TABLE l (s varchar(3), n smallint)` then `INSERT INTO l VALUES ('abcd', 1)` and an INSERT with n outside -32768..32767. AWS: 22001 and 22003. Emulator: both accepted.
6. Cause 6: `SELECT ('2024-01-01'::date + 31)::text, ('2024-01-01'::date + 60)::text`. AWS: ('2024-02-01','2024-03-01'). Emulator: ('2025','2026').
7. Cause 7: BEGIN; provoke a statement error (e.g. a duplicate key); then issue any further statement. AWS: 25P02 "current transaction is aborted, commands ignored until end of transaction block" until ROLLBACK [TO SAVEPOINT]. Emulator: the next statement is accepted.
8. Cause 8 (MySQL): as master, `CREATE USER '<app>'@'%' IDENTIFIED BY '<pw>'` and `GRANT SELECT ON <schema>.<open_table> TO '<app>'@'%'`; then connect as <app>. AWS: the login succeeds, the granted SELECT reads, an ungranted SELECT/INSERT is refused 1142 naming the table. Emulator: the login fails with 1045 Access denied. Cause 8 (PostgreSQL): as master, `CREATE ROLE <app> LOGIN PASSWORD '<pw>'`, then connect as <app>. AWS: connects; ungranted access refused 42501 "permission denied for table <table>". Emulator: the connection is never answered by a server, so the session is never established.
9. Re-replay to confirm a fix: the check ids in trace_ids.
