Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift-data.describe-table-in-a-never-created-database-rejected redshift-data.list-schemas-on-a-never-created-database-rejected-until-created redshift-data.list-tables-absent-database-rejected-while-connected-database-lists-none redshift-data.batch-into-a-closed-session-rejected redshift-data.batch-on-a-busy-session-rejected-until-the-session-frees redshift-data.statement-into-a-busy-session-rejected redshift-data.get-statement-result-unissued-next-token-rejected redshift-data.get-statement-result-v2-unissued-next-token-rejected redshift-data.list-schemas-unissued-next-token-rejected redshift-data.list-tables-unissued-next-token-rejected redshift-data.get-statement-result-v2-long-poll-on-running-statement-rejected redshift-data.list-databases-unissued-next-token-rejected
Issue-Shape-Id: com.amazonaws.redshiftdata#RedshiftData
Issue-Key: redshift-data.describe-table-in-a-never-created-database-rejected|run#10|AssertionError|assert refused == (
Issue-Key: redshift-data.list-schemas-on-a-never-created-database-rejected-until-created|run#7|AssertionError|assert refused == (
Issue-Key: redshift-data.list-tables-absent-database-rejected-while-connected-database-lists-none|run#10|AssertionError|assert got == ("ValidationException", 400, 'FATAL: database "nodb_absent" does not exist'), (
Issue-Key: redshift-data.batch-into-a-closed-session-rejected|run#18|AssertionError|assert got == ("ValidationException", "Session is not available"), got
Issue-Key: redshift-data.batch-on-a-busy-session-rejected-until-the-session-frees|_refusal#19|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded with {got!r}")
Issue-Key: redshift-data.statement-into-a-busy-session-rejected|run#22|AssertionError|raise AssertionError(f"a statement sent into a busy session was accepted: {accepted!r}")
Issue-Key: redshift-data.get-statement-result-unissued-next-token-rejected|run#11|AssertionError|assert err == ("ValidationException", _BAD_TOKEN), f"made-up token answered {err!r}"
Issue-Key: redshift-data.get-statement-result-v2-unissued-next-token-rejected|run#8|AssertionError|assert (code, message) == ("ValidationException", "Failed to process fetch result token."), (
Issue-Key: redshift-data.list-schemas-unissued-next-token-rejected|run#14|AssertionError|assert got == want, f"an unissued NextToken answered {got!r}, expected {want!r}"
Issue-Key: redshift-data.list-tables-unissued-next-token-rejected|run#12|AssertionError|assert (code, message) == ("ValidationException", "Invalid pagination token provided."), (
Issue-Key: redshift-data.get-statement-result-v2-long-poll-on-running-statement-rejected|run#19|AssertionError|assert got == ("ResourceNotFoundException", 400, _NO_RESULT), (
Issue-Key: redshift-data.list-databases-unissued-next-token-rejected|run#6|AssertionError|assert issued, f"a one-name first page carried no NextToken: {first!r}"
Issue-Commit: 7815a7c486f6433d295c8c37df76b6ba67dbf690

# redshift-data refusals and pagination: 7 net-new causes — absent-database and closed-session refusals carry the wrong error class and message, a busy session accepts a second statement, both result and catalog NextToken refusals say 'Invalid token value', a running statement is refused ValidationException, and ListDatabases issues no NextToken

> _Note: this issue was identified against the code base at commit `7815a7c486f6433d295c8c37df76b6ba67dbf690`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTables (also DescribeTable, ListSchemas, ListDatabases, GetStatementResult, GetStatementResultV2, ExecuteStatement, BatchExecuteStatement)

## Description
Seven distinct defects on the redshift-data API surface (error classification, refusal messages and pagination). Each cause below is separate work; all seven must be closed before this issue is submitted. None of these checks is marked ALREADY FILED or OWNED by an open issue.

Cause 1 of 7: a catalog call against a database that does not exist is refused with the wrong error class, status and message.
AWS: ValidationException, HTTP 400, message `FATAL: database "<database>" does not exist` — fixed text 'FATAL: database "' + the request's Database value + '" does not exist'. Smithy: DescribeTable/ListSchemas/ListTables/ListDatabases all model ValidationException (httpError 400) and ResourceNotFoundException (httpError 404); AWS surfaces the engine's connect failure as the 400 ValidationException, because the *resource* (workgroup) exists and only the named database does not.
Emulator: ResourceNotFoundException, HTTP 404, message `Database does not exist: <database>` — wrong error code, wrong status, and a message of its own wording.
Why: a client that retries/branches on ResourceNotFoundException vs ValidationException takes the wrong branch (404 reads as 'the workgroup or statement is gone'), and anything matching AWS's 'FATAL: database "…" does not exist' text never matches.
Fix: the absent-database refusal in the redshift-data catalog handlers (DescribeTable, ListSchemas, ListTables, ListDatabases) — the database-resolution step must raise ValidationException/400 with the FATAL wording.
Traces: describe-table-in-a-never-created-database-rejected, list-schemas-on-a-never-created-database-rejected-until-created, list-tables-absent-database-rejected-while-connected-database-lists-none.

Cause 2 of 7: a batch sent into a session that has been closed is refused with the wrong error class and message.
AWS: ValidationException (httpError 400 per the model) with the fixed message `Session is not available`.
Emulator: ResourceNotFoundException with the message `Session not found.`
Why: a client that distinguishes a closed/unavailable session (ValidationException — stop reusing the SessionId, open a new session) from a missing resource misroutes the failure and may retry indefinitely.
Fix: the session-lookup/validation step shared by ExecuteStatement and BatchExecuteStatement in the redshift-data handlers.
Traces: batch-into-a-closed-session-rejected.

Cause 3 of 7: a statement or batch sent into a session that is already running a statement is accepted instead of refused.
AWS: while a session is busy, a second ExecuteStatement/BatchExecuteStatement naming that SessionId is refused ValidationException with the fixed message `Session is not available`; it succeeds again once the session frees.
Emulator: HTTP 200 with a new statement Id — e.g. {'Id': '…', 'SessionId': '…', 'Status': 'SUBMITTED', 'RedshiftPid': 1000} — so the session serialisation AWS enforces does not exist.
Why: a client relying on AWS's one-statement-per-session rule (to serialise work and to detect contention) gets silent concurrent execution in one session, so transaction scoping and ordering inside a session are unsound.
Fix: session busy-state tracking in the redshift-data session state plus the refusal in the ExecuteStatement/BatchExecuteStatement handlers.
Traces: batch-on-a-busy-session-rejected-until-the-session-frees, statement-into-a-busy-session-rejected.

Cause 4 of 7: an unissued NextToken on a result read is refused with the wrong message.
AWS: ValidationException, HTTP 400, fixed message `Failed to process fetch result token.` for GetStatementResult and GetStatementResultV2 (no variable part).
Emulator: ValidationException with the message `Invalid token value`.
Why: the error code matches but the message does not, so a client (or an assertion/log classifier) keyed on AWS's result-token wording cannot recognise a forged or expired fetch token.
Fix: the NextToken decode/validation path of the GetStatementResult and GetStatementResultV2 handlers.
Traces: get-statement-result-unissued-next-token-rejected, get-statement-result-v2-unissued-next-token-rejected.

Cause 5 of 7: an unissued NextToken on a catalog listing is refused with the wrong message.
AWS: ValidationException, HTTP 400, fixed message `Invalid pagination token provided.` for ListSchemas, ListTables and ListDatabases (no variable part) — note this is a different message from the result-read one in Cause 4.
Emulator: ValidationException, HTTP 400, message `Invalid token value`.
Why: same as Cause 4 — an invalid catalog pagination token is unrecognisable to a client matching AWS's wording, and the emulator collapses two distinct AWS messages into one.
Fix: the NextToken decode/validation path shared by the catalog listing handlers (ListSchemas, ListTables, ListDatabases).
Traces: list-schemas-unissued-next-token-rejected, list-tables-unissued-next-token-rejected.

Cause 6 of 7: a result read of a statement that has not finished is refused with the wrong error class and message.
AWS: ResourceNotFoundException (recorded at HTTP 400 for this refusal; the model's default for the shape is 404) with the fixed message `Query does not have result. Please check query status with DescribeStatement.` — no variable part. This is how a GetStatementResultV2 long poll that expires while the query still runs ends.
Emulator: ValidationException, HTTP 400, message `Query <statement id> has not finished running; its status is STARTED` — an error code and a message AWS does not use for this case (the id and status are per-run values; the whole template is wrong).
Why: a client that polls with WaitTimeSeconds and treats ResourceNotFoundException-with-that-message as 'not ready yet, keep polling' sees a ValidationException and aborts as if its request were malformed.
Fix: the not-yet-available-result refusal in the GetStatementResult/GetStatementResultV2 handlers (including the long-poll expiry path).
Traces: get-statement-result-v2-long-poll-on-running-statement-rejected.

Cause 7 of 7: ListDatabases returns a page filled to MaxResults without a NextToken.
AWS: recorded behavior only — Smithy marks ListDatabases @paginated (inputToken/outputToken NextToken, pageSize MaxResults, items Databases); with MaxResults=1 the first page carries one name AND a NextToken that resumes the walk.
Emulator: ListDatabases(MaxResults=1) answers 200 {'Databases': ['dev']} with no NextToken at all, so the listing cannot be resumed and the token-refusal path is unreachable.
Why: a paginating client (botocore's paginator included) stops after the first page and silently misses every remaining database.
Fix: NextToken issuance in the ListDatabases handler — honour MaxResults and emit a continuation token for a filled page, keyed to the same token format its validator accepts.
Traces: list-databases-unissued-next-token-rejected.

## Evidence
Cause 1 — DescribeTable(Database=<never created>) answered ('ResourceNotFoundException', 404, 'Database does not exist: sim2real_dtdb_e358404d9125'); expected ('ValidationException', 400, 'FATAL: database "sim2real_dtdb_e358404d9125" does not exist'). ListSchemas answered ('ResourceNotFoundException', 404, 'Database does not exist: sim2real_lsdb_9c9196ac66a8'); expected ('ValidationException', 400, 'FATAL: database "sim2real_lsdb_9c9196ac66a8" does not exist'). ListTables answered ('ResourceNotFoundException', 404, 'Database does not exist: nodb_absent'); expected ('ValidationException', 400, 'FATAL: database "nodb_absent" does not exist').
Cause 2 — BatchExecuteStatement(SessionId=<closed>) answered ('ResourceNotFoundException', 'Session not found.'); expected ('ValidationException', 'Session is not available').
Cause 3 — BatchExecuteStatement(Sqls=['select 4'], SessionId=<busy>, StatementName=…) returned HTTP 200 {'Id': 'df42eb18-134d-4504-9689-24c2c4ad7433', 'SessionId': 'b9b84e64-…', 'Status': 'SUBMITTED', 'RedshiftPid': 1000}; ExecuteStatement into a busy session likewise returned 200 {'Id': '19ee7fa1-…', 'SessionId': '5f6cb619-…', 'Status': 'SUBMITTED'}. Expected a refusal ('ValidationException', 'Session is not available').
Cause 4 — GetStatementResult(NextToken=<forged>) answered ('ValidationException', 'Invalid token value'); GetStatementResultV2(NextToken='page-two-of-nothing') answered ('ValidationException', 'Invalid token value'); expected ('ValidationException', 'Failed to process fetch result token.').
Cause 5 — ListSchemas(NextToken=<forged>) answered ('ValidationException', 400, 'Invalid token value'); ListTables(NextToken='page-two-of-nothing') answered ('ValidationException', 'Invalid token value'); expected ('ValidationException', 400, 'Invalid pagination token provided.').
Cause 6 — GetStatementResultV2 long poll that expired while the query ran answered ('ValidationException', 400, 'Query 024b123c-76b5-4df8-8945-0bfcf8451c36 has not finished running; its status is STARTED'); expected ('ResourceNotFoundException', 400, 'Query does not have result. Please check query status with DescribeStatement.').
Cause 7 — ListDatabases(MaxResults=1, WorkgroupName=…, Database='dev') answered 200 {'Databases': ['dev']} with no NextToken key; expected a one-name page carrying a NextToken.
Smithy grounding (com-amazonaws-redshiftdata-redshiftdata.json): ValidationException httpError 400, ResourceNotFoundException httpError 404 on DescribeTable/ListSchemas/ListTables/ListDatabases/GetStatementResult/GetStatementResultV2; ListDatabases/ListSchemas/ListTables are @paginated on NextToken/MaxResults.

## Steps to Reproduce
1. Create a redshift-serverless namespace + workgroup and wait for AVAILABLE; point redshift-data at it (WorkgroupName + Database).
2. Cause 1: call DescribeTable / ListSchemas / ListTables with Database set to a name never created; read Error.Code, HTTP status and Error.Message.
3. Cause 2: ExecuteStatement with SessionKeepAliveSeconds small to open a session, let it lapse until ListStatements shows the session CLOSED, then BatchExecuteStatement with that SessionId.
4. Cause 3: open a session and start a long-running statement in it, then (a) BatchExecuteStatement and (b) ExecuteStatement naming the same SessionId while it is still running.
5. Cause 4: run a statement that returns rows, then GetStatementResult / GetStatementResultV2 on its Id with NextToken='page-two-of-nothing'.
6. Cause 5: ListSchemas and ListTables with NextToken='page-two-of-nothing' (or any base64 string never issued).
7. Cause 6: submit a very long-running SELECT and call GetStatementResultV2(Id=…, WaitTimeSeconds=…) so the poll expires while the statement is STARTED.
8. Cause 7: ListDatabases(MaxResults=1) on the workgroup and inspect the response for NextToken.
