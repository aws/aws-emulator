Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected redshift-data.session-persistence-across-statements redshift-data.get-statement-result-pagination redshift-data.cancel-statement-running-then-finished-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected|run#22|AssertionError|assert no_result[1] == 400, (
Issue-Key: redshift-data.session-persistence-across-statements|run#21|AssertionError|assert fresh_final["Status"] == "FAILED", fresh_final
Issue-Key: redshift-data.get-statement-result-pagination|run#10|AssertionError|assert select_final["Status"] == "FINISHED", select_final.get("Error")
Issue-Key: redshift-data.cancel-statement-running-then-finished-rejected|_poll_until#11|Timeout|time.sleep(interval_s)
Issue-Commit: df0e92648e2e716917662ffab5a7edf9e8dd8fc2

# redshift-data: 4 net-new causes — no-result GetStatementResult refused at 404 instead of 400, sessions do not scope temp-table/connection state, a 100 MB retention cap FAILS a statement AWS serves (so NextToken paging is unreachable), and a lock-blocked statement never reports a running state

> _Note: this issue was identified against the code base at commit `df0e92648e2e716917662ffab5a7edf9e8dd8fc2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetStatementResult

## Description
Four net-new root causes in the redshift-data (Redshift Data API) emulator, exercised against a self-provisioned Redshift Serverless workgroup.

Cause 1 of 4: GetStatementResult for a terminal statement that produced no result set is refused ResourceNotFoundException at HTTP 404; the service answers this refusal at HTTP 400.
AWS: reading the result of an ABORTED/FAILED/FINISHED-without-result-set statement is refused with code `ResourceNotFoundException` and the fixed message "Query does not have result. Please check query status with DescribeStatement." (entirely fixed text — no request values) at HTTP 400. Recorded behavior: the check asserts "ResourceNotFoundException here is an HTTP 400, not a 404". Smithy declares ResourceNotFoundException with @httpError(404) — that is the shape's default rendering; on this path the service uses the client-input status 400, which the emulator already models for the sibling "Query does not exist." refusal (state.rs UNKNOWN_QUERY_STATUS = 400, served through Scaffolded::modeled_with_status).
Emulator: code and message match, status is 404.
Why: clients and SDK/proxy classification branch on status — 404 reads as a resource-addressing failure, AWS's 400 as a client-input failure for a statement that exists but has no result, so error handling calibrated on AWS takes the wrong branch.
Fix: the no-result branch of the GetStatementResult handler (services/service-com-amazonaws-redshiftdata-redshiftdata/src/ops/get_statement_result.rs, the `!statement.has_fetchable_result()` terminal path, ~lines 215-233) must serve the modeled error at the service's status, as no_such_query in the same file already does. Error code and message unchanged. (The same defect is reported under the `redshift` heading as cause 2 of the redshift Data API issue filed in this sweep.)
Failing checks: redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected.

Cause 2 of 4: a session's state is global, not session-scoped — a TEMP TABLE created in a kept-alive session is visible to a brand-new session, which FINISHES and returns the row instead of FAILING.
AWS: ExecuteStatement models `SessionKeepAliveSeconds` and `SessionId`; a session is one database connection held open across statements. A TEMP table lives in the session's own temporary schema, so a statement submitted on a NEW session (WorkgroupName/Database, no SessionId) cannot resolve it and the statement reaches Status FAILED with a relation-does-not-exist error. Recorded behavior: the check requires the non-reused statement to be FAILED.
Emulator: DescribeStatement for the fresh statement reports Status FINISHED, HasResultSet true, ResultRows 1 — i.e. it read the other session's temp table (observed ExecutionMode AUTO_COMMIT, a single shared connection).
Why: session semantics are unobservable and unsafe — temp tables, SET-scoped settings and transactions leak between unrelated callers, so one client's scratch state is readable and collides with another's, and code that depends on temp-table isolation behaves differently here than on AWS.
Fix: session/connection ownership in the redshift-data state store (services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs) and the submit path (src/ops/execute_statement.rs): each SessionId must own a connection whose temporary and transaction state is private to it, and a statement without that SessionId must not see it.
Failing checks: redshift-data.session-persistence-across-statements.

Cause 3 of 4: an emulator-internal 100 MiB result-retention cap FAILS a statement the service runs to completion, so GetStatementResult's NextToken paging is never reachable for a large result set.
AWS: a SELECT returning ~90,000 rows of ~2 KB (≈180 MB of row data) runs to Status FINISHED, and GetStatementResult returns it across SEVERAL pages linked by `NextToken`, with `TotalNumRows` reporting the full count; GetStatementResult is modeled @paginated (inputToken NextToken, outputToken NextToken, items Records). Recorded behavior: the statement FINISHES and every row id comes back exactly once across the pages.
Emulator: DescribeStatement reports the statement NOT FINISHED, with the emulator-authored error "the result set exceeded the 104857600 bytes this submission may still retain, of this emulator's bound of 104857600". The cap is MAX_RESULT_BYTES = 100 * 1024 * 1024 in services/service-com-amazonaws-redshiftdata-redshiftdata/src/state.rs (~lines 279-289), charged against this process's retention cost (per-cell/per-row overhead plus 3x expansion for text), so it refuses well below the 100 MB of result data it is meant to model.
Why: a client paging a large result set gets a failed statement and a message no AWS error vocabulary contains, instead of a FINISHED statement and several NextToken pages — large-result workflows (exports, backfills) cannot run at all, and the emulator's only paging path for GetStatementResult goes untested.
Why (secondary): the failure text is emulator prose rather than any AWS status/error, so a client cannot classify it.
Fix: the result-retention accounting and cap in src/state.rs (MAX_RESULT_BYTES and the charging that uses it) and the statement execution path that marks the submission failed — the bound must be measured in the result's own byte size (not process retention overhead) and must not refuse a result the service serves; if a ceiling must remain, it has to be expressed as a modeled Redshift refusal, not free prose.
Failing checks: redshift-data.get-statement-result-pagination.

Cause 4 of 4: a statement blocked on an ACCESS EXCLUSIVE lock is never observable in a running state, so CancelStatement's positive control and its already-terminal refusal cannot be reached — the check reaches no verdict inside the harness's 60 s cap.
AWS: a SELECT contending with a session that holds `LOCK TABLE ... IN ACCESS EXCLUSIVE MODE` inside an open transaction stays non-terminal (DescribeStatement Status STARTED or PICKED) until it is cancelled or the holder commits; CancelStatement then answers Status true and the statement lands ABORTED, and a second CancelStatement on the now-terminal statement is refused `ValidationException` at HTTP 400 with the message "Could not cancel a query that is already in ABORTED state with ID: <Id>" (fixed text; the state name and the statement Id are the variable parts taken from the statement). Recorded behavior for the lifecycle; Smithy: CancelStatement errors include ValidationException (400) and ResourceNotFoundException.
Emulator: the check never completes — "Check timed out after 60.0s" — while polling this lifecycle, after the same workgroup provisioning other checks in this service complete. Hypothesis: with every statement on one shared auto-committing connection (see cause 2), the holder's LOCK TABLE is not held across calls and the contending SELECT runs to a terminal status immediately, so it is never seen as STARTED/PICKED and the cancel lifecycle is unreachable.
Why: the whole cancel contract is unobservable — no client can verify that a running statement is cancellable, that a cancel drives it to ABORTED, or that a second cancel is refused with AWS's wording and status.
Fix: statement lifecycle/state transitions and lock contention in src/state.rs (the execution scheduling and materialise_due_transitions path) together with src/ops/cancel_statement.rs — a statement that cannot proceed must remain non-terminal and cancellable, and the already-terminal cancel must be refused ValidationException/400 with AWS's sentence.
Failing checks: redshift-data.cancel-statement-running-then-finished-rejected.

Related open issues, none of which cover these causes: 20261108T070550Z-redshift-data-the-result-reader-format-mismatch-validationexception.md (CSV/JSON reader message), 20261105T195000Z-redshift-data-executestatement-target-validation-reports-the-mutual.md (target validation), 20261109T193330Z-redshift-data-api-serverless-workgroup-statement-responses-diverge.md (statement response members).

## Evidence
Cause 1 — redshift-data.cancel-running-statement-aborts-and-terminal-cancel-rejected:
  AssertionError: ResourceNotFoundException here is an HTTP 400, not a 404, got 404
  Expected: GetStatementResult on the aborted statement -> HTTP 400, ResourceNotFoundException. Actual: HTTP 404 with that code.
  Source: src/ops/get_statement_result.rs returns GetStatementResultErrorModeled::ResourceNotFoundException without a status override (renders @httpError(404)); the unknown-query path in the same file overrides to 400 (state.rs UNKNOWN_QUERY_STATUS).

Cause 2 — redshift-data.session-persistence-across-statements:
  Expected: a brand-new (non-reused) session running `SELECT id FROM <temp table>` -> Status FAILED.
  Actual (DescribeStatement): {'Status': 'FINISHED', 'HasResultSet': True, 'ResultRows': 1, 'ResultSize': 8, 'QueryString': 'SELECT id FROM sess_tmp_...', 'ExecutionMode': 'AUTO_COMMIT', ...} — HTTP 200.
  AssertionError: fresh_final["Status"] == "FAILED" failed.

Cause 3 — redshift-data.get-statement-result-pagination:
  Expected: `SELECT id, payload FROM <90,000-row table> ORDER BY id` -> Status FINISHED, then several GetStatementResult pages linked by NextToken covering every id exactly once.
  Actual: AssertionError: the result set exceeded the 104857600 bytes this submission may still retain, of this emulator's bound of 104857600 (the statement never reached FINISHED).
  Source: src/state.rs MAX_RESULT_BYTES = 100 * 1024 * 1024, charged against process retention cost (RETAINED_CELL_OVERHEAD, RETAINED_ROW_OVERHEAD, LOSSY_TEXT_EXPANSION = 3).

Cause 4 — redshift-data.cancel-statement-running-then-finished-rejected:
  Expected: the lock-blocked SELECT observable as STARTED/PICKED, CancelStatement -> Status true, statement -> ABORTED, second CancelStatement -> ValidationException/400 "Could not cancel a query that is already in ABORTED state with ID: <Id>".
  Actual: Check timed out after 60.0s — no verdict; the lifecycle never advances to an observable running state (other checks in this service provision the same workgroup and complete).

## Steps to Reproduce
Common setup: create a Redshift Serverless namespace + workgroup (base capacity 8) and wait for AVAILABLE; use its name/database for every redshift-data call below.

Cause 1:
1. ExecuteStatement a long-running statement; poll DescribeStatement until STARTED.
2. CancelStatement(Id); poll until Status == ABORTED.
3. GetStatementResult(Id) -> AWS: HTTP 400 + ResourceNotFoundException "Query does not have result. Please check query status with DescribeStatement."; emulator: HTTP 404 with the same code/message.

Cause 2:
1. ExecuteStatement(Sql="CREATE TEMP TABLE t (id INT); INSERT INTO t VALUES (42);", SessionKeepAliveSeconds=90); wait FINISHED; keep SessionId.
2. ExecuteStatement(Sql="SELECT id FROM t", SessionId=<that session>) -> FINISHED, GetStatementResult returns 42 (both AWS and emulator).
3. ExecuteStatement(WorkgroupName/Database, Sql="SELECT id FROM t") with NO SessionId; poll DescribeStatement.
4. AWS: Status FAILED (the temp table does not exist outside its session). Emulator: FINISHED, HasResultSet true, ResultRows 1.

Cause 3:
1. ExecuteStatement "CREATE TABLE t (id INT, payload VARCHAR(2000))"; wait FINISHED.
2. ExecuteStatement an INSERT that produces 90,000 rows each padded to ~2000 bytes of payload; wait FINISHED.
3. ExecuteStatement "SELECT id, payload FROM t ORDER BY id" and poll DescribeStatement.
4. AWS: FINISHED, then page GetStatementResult with NextToken until exhausted. Emulator: the statement is not FINISHED and reports "the result set exceeded the 104857600 bytes this submission may still retain, of this emulator's bound of 104857600".

Cause 4:
1. ExecuteStatement "CREATE TABLE t (id INT)"; wait FINISHED.
2. ExecuteStatement(Sql="BEGIN; LOCK TABLE t IN ACCESS EXCLUSIVE MODE;", SessionKeepAliveSeconds=120); wait FINISHED (holder session stays open).
3. ExecuteStatement(Sql="SELECT * FROM t") on a new default session; poll DescribeStatement for STARTED/PICKED.
4. AWS: it stays non-terminal; CancelStatement answers Status true, the statement becomes ABORTED, and a second CancelStatement is refused ValidationException/400 "Could not cancel a query that is already in ABORTED state with ID: <Id>". Emulator: step 3 never observes a running statement and the scenario reaches no verdict within the 60 s harness cap.
