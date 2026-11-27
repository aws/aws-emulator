Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.batch-get-query-execution-mixed-batch-refuses-each-unknown-entry-by-id athena.batch-get-query-execution-reports-runtime-failed-query-beside-succeeded-twin athena.get-query-results-unissued-next-token-rejected athena.list-calculation-executions-state-filter-isolates-failed-calculation athena.raising-calculation-reports-failed-and-session-stays-usable athena.spark-calculation-raising-error-reports-failed athena.start-calculation-on-busy-session-rejected athena.start-session-reused-token-returns-the-first-session athena.start-session-with-tags-rejected athena.workgroup-is-regional-and-absent-from-another-region
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena.batch-get-query-execution-mixed-batch-refuses-each-unknown-entry-by-id|_refusal#25|AssertionError|raise AssertionError(f"expected the batch to be refused, it answered {answer!r}")
Issue-Key: athena.batch-get-query-execution-reports-runtime-failed-query-beside-succeeded-twin|_wait_for#15|AssertionError|raise AssertionError(f"query never reached {want}; last seen {state!r}")
Issue-Key: athena.get-query-results-unissued-next-token-rejected|run#20|AssertionError|assert (code, message) == ("InvalidRequestException", f"Malformed nextPageToken {_FORGED}"), (
Issue-Key: athena.list-calculation-executions-state-filter-isolates-failed-calculation|run#21|KeyError|result = athena.get_calculation_execution(CalculationExecutionId=bad_id)["Result"]
Issue-Key: athena.raising-calculation-reports-failed-and-session-stays-usable|run#17|KeyError|stderr = _s3_text(s3, detail["Result"]["StdErrorS3Uri"])
Issue-Key: athena.spark-calculation-raising-error-reports-failed|run#13|KeyError|err_uri = urllib.parse.urlparse(got["Result"]["StdErrorS3Uri"])
Issue-Key: athena.start-calculation-on-busy-session-rejected|run#6|AssertionError|raise AssertionError(f"a BUSY session accepted a second calculation: {accepted!r}")
Issue-Key: athena|StartSession|InvalidRequestException
Issue-Key: athena.start-session-with-tags-rejected|run#8|AssertionError|raise AssertionError(f"StartSession accepted Tags on a Spark session: state {accepted.get('State')!r}")
Issue-Key: athena.workgroup-is-regional-and-absent-from-another-region|run#8|AssertionError|raise AssertionError(f"{other_region} reads the {ctx.region} workgroup: {leaked!r}")
Issue-Commit: da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03

# athena: 8 net-new causes — BatchGetQueryExecution never refuses an all-unknown batch, division-by-zero SUCCEEDS, GetCalculationExecution drops Result, Spark session admission/idempotency/Tags wrong, workgroups are not region-scoped

> _Note: this issue was identified against the code base at commit `da5b5c82f86ff32a5f1d8fa609cf9d0b9820cd03`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetQueryExecution

## Description
Eight independent root causes in the athena emulator, all grounded in failing behavioral checks. Each must be closed before this issue is submitted.

=== Cause 1 of 8: BatchGetQueryExecution answers 200 for a batch in which no id names an existing execution ===
AWS: Smithy: BatchGetQueryExecution input QueryExecutionIds @length min 1 max 50, required; modeled errors InvalidRequestException (client, HTTP 400) and InternalServerException. A mixed batch returns the executions that exist under QueryExecutions and every id that does not under UnprocessedQueryExecutionIds, each carrying ErrorCode QUERY_EXECUTION_NOT_FOUND and ErrorMessage "QueryExecution <id> was not found" (fixed text "QueryExecution " + the id echoed verbatim from the request + " was not found"). Recorded behavior only for the all-unknown case: when NO requested id resolves, the whole call is refused with InvalidRequestException and the fixed message "At least one valid query execution Id is required." (no variable part).
Emulator: the mixed batch is served correctly, but a batch containing only unknown/malformed ids answers HTTP 200 with QueryExecutions: [] and every requested id listed under UnprocessedQueryExecutionIds. No error is raised.
Why: a client that reads a 200 as "the batch was processed" treats a wholly unresolvable batch as an empty-but-valid result, and so never surfaces the bad input; on AWS the same call is a 400 that aborts the caller's lookup path.
Fix: the BatchGetQueryExecution handler's request validation — the all-unknown case must be refused before the per-id partial-failure list is built.
Traces: athena.batch-get-query-execution-mixed-batch-refuses-each-unknown-entry-by-id

=== Cause 2 of 8: an integer division by zero inside a DML statement ends SUCCEEDED instead of FAILED ===
AWS: recorded behavior only (the Smithy model does not describe runtime expression semantics). A DML statement whose evaluation divides an integer by zero ends with QueryExecution.Status.State = FAILED and an AthenaError describing the division by zero; it never reaches SUCCEEDED.
Emulator: StartQueryExecution of `SELECT 6 / (7 - 7) AS answer` settles SUCCEEDED, so the check that waits for FAILED times out on "last seen 'SUCCEEDED'".
Why: a caller that branches on State to detect a bad computation silently consumes a wrong (or NULL) result instead of the failure AWS reports, so data-quality guards built on query failure never fire.
Fix: the query-engine's arithmetic evaluation / terminal-state classification used by StartQueryExecution's execution path. (The same defect is observed under the athena__s3 service heading as athena__s3.integer-division-by-zero-fails-the-query and is carried in the athena__s3 issue of this sweep; this cause covers the athena-heading check.)
Traces: athena.batch-get-query-execution-reports-runtime-failed-query-beside-succeeded-twin

=== Cause 3 of 8: GetQueryResults' malformed-NextToken refusal is re-worded ===
AWS: Smithy: GetQueryResults input NextToken target Token @length 1..1024; modeled error InvalidRequestException (client, HTTP 400). A NextToken that this emulator/service never issued is refused with code InvalidRequestException and the message "Malformed nextPageToken <token>" — fixed text "Malformed nextPageToken " followed by the token value exactly as supplied in the request.
Emulator: same code, different text: "The nextPageToken is malformed: <token>" (observed with the forged token echoed after a colon).
Why: clients and test harnesses that match on AWS's message text to distinguish a forged/expired token from other InvalidRequestException causes fail to classify the refusal.
Fix: the GetQueryResults handler's pagination-token validation message.
Traces: athena.get-query-results-unissued-next-token-rejected

=== Cause 4 of 8: GetCalculationExecution omits the Result member for a terminal calculation ===
AWS: Smithy: GetCalculationExecutionResponse member Result targets CalculationResult { StdOutS3Uri, StdErrorS3Uri, ResultS3Uri, ResultType } — "Contains information about an application-specific calculation result"; StdErrorS3Uri is "The Amazon S3 location of the stderr error messages file for the calculation." A calculation that has reached a terminal state (COMPLETED or FAILED) is read back with Result populated, its S3 URIs pointing under the workgroup's ResultConfiguration.OutputLocation bucket, and for a FAILED calculation the stderr object holds the raised traceback.
Emulator: the GetCalculationExecution response contains no Result key at all, so every caller indexing ["Result"] raises KeyError: 'Result'. This is observed on FAILED calculations in three separate checks.
Why: there is no way to reach a Spark calculation's stdout, stderr or result location, so a client cannot diagnose a failed calculation or read a successful one's output at all.
Fix: the GetCalculationExecution handler/serializer and the calculation state that must record the three S3 URIs and ResultType when a calculation terminates.
Traces: athena.list-calculation-executions-state-filter-isolates-failed-calculation, athena.raising-calculation-reports-failed-and-session-stays-usable, athena.spark-calculation-raising-error-reports-failed

=== Cause 5 of 8: StartCalculationExecution is accepted on a session that is already BUSY ===
AWS: Smithy: StartCalculationExecution input SessionId (required) and CodeBlock; modeled errors InvalidRequestException (client, HTTP 400), ResourceNotFoundException, InternalServerException. A Spark session that is BUSY running a calculation refuses a second submission with code InvalidRequestException and the message "Session is in the BUSY state; needs to be IDLE to accept Calculations." (entirely fixed text; the session state word BUSY is the session's actual state), and creates nothing — the session continues to list only the first calculation. The identical request is accepted once the session is back to IDLE.
Emulator: the second StartCalculationExecution on the BUSY session returns HTTP 200 with a new CalculationExecutionId and State "CREATED".
Why: a client relying on the refusal to serialise work onto one session (or to back off and retry) instead creates concurrent calculations that AWS would never admit, and the session's calculation listing gains an execution that must not exist.
Fix: StartCalculationExecution's handler admission check against the session's current state.
Traces: athena.start-calculation-on-busy-session-rejected

=== Cause 6 of 8: StartSession refuses a repeated ClientRequestToken whose Description changed, instead of replaying the first session ===
AWS: Smithy: StartSession input ClientRequestToken target IdempotencyToken @length 32..128; its documentation says "A unique case-sensitive string used to ensure the request to create the session is idempotent (executes only once). If another StartSessionRequest is received, the same response is returned and another session is not created." Recorded behavior: both a byte-identical repeat AND a repeat that only changes Description return the FIRST session's SessionId and start no second session, and the session keeps its original Description; a fresh token starts a new session. (The same member's doc also carries the sentence "If a parameter has changed, an error is returned" — the recording shows AWS does not treat Description as an idempotency-discriminating parameter, so the recorded behavior is the requirement here.)
Emulator: the repeat with a changed Description is refused with HTTP 400 InvalidRequestException "Idempotent parameters do not match".
Why: SDKs auto-generate and retry with the same ClientRequestToken; a caller that re-sends after a timeout with a slightly different Description gets a hard 400 instead of the existing session, so a safe retry becomes an unrecoverable failure.
Fix: StartSession's idempotency handling — which request members participate in the token's stored request signature, in the handler/state that records tokens.
Traces: athena.start-session-reused-token-returns-the-first-session

=== Cause 7 of 8: StartSession accepts Tags on a Spark session ===
AWS: Smithy: StartSession input models a Tags member (TagList) and modeled error InvalidRequestException (client, HTTP 400). Recorded behavior: Athena refuses Tags on a Spark session with code InvalidRequestException and the message "tags are not allowed for Athena Spark session " (fixed text, including its trailing space), and starts NO session — the workgroup's session listing is unchanged by the refused call.
Emulator: the request is accepted; StartSession returns a SessionId with State "CREATING", so a session that AWS never creates now exists in the workgroup.
Why: a client that tags sessions believes the tags were applied (they are not readable anywhere) and leaks a live, billable-shaped session the real service would have rejected; the workgroup's session inventory diverges.
Fix: StartSession's input validation in its handler.
Traces: athena.start-session-with-tags-rejected

=== Cause 8 of 8: workgroups are not Region-scoped — one Region reads and lists another Region's workgroup ===
AWS: recorded behavior only (the model has no Region member; Athena's endpoint carries the Region). A workgroup exists only in the Region it was created in: GetWorkGroup for that name from a second Region is refused with code InvalidRequestException and the message "WorkGroup is not found." (entirely fixed text — it does NOT interpolate the workgroup name), and ListWorkGroups in the second Region does not list it.
Emulator: GetWorkGroup issued against a different Region's endpoint (us-east-2 for a workgroup created in us-west-2) answers HTTP 200 with the full WorkGroup body — Name, State ENABLED, Configuration and the Description set in the creating Region.
Why: a multi-Region client cannot distinguish "deployed here" from "deployed elsewhere": a Region-isolation precondition always passes, cross-Region reads that must fail succeed, and a per-Region inventory built from ListWorkGroups double-counts.
Fix: the workgroup state store must be partitioned by Region, and GetWorkGroup/ListWorkGroups must resolve only within the request's Region.
Traces: athena.workgroup-is-regional-and-absent-from-another-region

## Evidence
Cause 1: expected ClientError InvalidRequestException "At least one valid query execution Id is required."; got HTTP 200 {'QueryExecutions': [], 'UnprocessedQueryExecutionIds': [ ... three QUERY_EXECUTION_NOT_FOUND entries ... ]} (AssertionError: expected the batch to be refused, it answered ...).
Cause 2: expected QueryExecution.Status.State == 'FAILED' for `SELECT 6 / (7 - 7) AS answer`; got AssertionError: query never reached FAILED; last seen 'SUCCEEDED'.
Cause 3: expected ('InvalidRequestException', 'Malformed nextPageToken c2ltMnJlYWwtZm9yZ2VkLXBhZ2UtdG9rZW4tMDAwMQ=='); got ('InvalidRequestException', 'The nextPageToken is malformed: c2ltMnJlYWwtZm9yZ2VkLXBhZ2UtdG9rZW4tMDAwMQ==') — only the token value is request-derived, the surrounding text is fixed.
Cause 4: athena.get_calculation_execution(CalculationExecutionId=<failed calc>)['Result'] -> KeyError: 'Result' (three checks, on both the FAILED calculation of list-calculation-executions-state-filter and the raising calculations of the other two).
Cause 5: expected ClientError ('InvalidRequestException', 'Session is in the BUSY state; needs to be IDLE to accept Calculations.'); got AssertionError: a BUSY session accepted a second calculation: {'CalculationExecutionId': '<id>', 'State': 'CREATED', ... HTTPStatusCode 200}.
Cause 6: botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the StartSession operation: Idempotent parameters do not match — raised on the third StartSession, which reuses the token with Description 'changed request'; AWS returns the first SessionId.
Cause 7: expected ClientError ('InvalidRequestException', 'tags are not allowed for Athena Spark session '); got AssertionError: StartSession accepted Tags on a Spark session: state 'CREATING'.
Cause 8: expected ClientError ('InvalidRequestException', 'WorkGroup is not found.') from the second Region; got AssertionError: us-east-2 reads the us-west-2 workgroup: {'Name': 'sim2real-behav-wg-...', 'State': 'ENABLED', 'Configuration': {...}, 'Description': 'home', 'CreationTime': ...}.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly the checks in trace_ids.
Cause 1: start two queries in a workgroup, let them SUCCEED, then call BatchGetQueryExecution with a list containing only unknown ids (two random UUIDs and one non-UUID string) — expect a 400 InvalidRequestException; a mixed batch of real+unknown ids must still answer 200.
Cause 2: StartQueryExecution 'SELECT 6 / (7 - 7) AS answer' and poll GetQueryExecution — expect terminal State FAILED.
Cause 3: run a query to SUCCEEDED, then GetQueryResults(QueryExecutionId=<id>, NextToken=<a base64 string never issued>) and read Error.Message.
Cause 4: create a PySpark workgroup with ResultConfiguration.OutputLocation, StartSession, StartCalculationExecution with CodeBlock `raise ValueError('x')`, wait for FAILED, then GetCalculationExecution(CalculationExecutionId=<id>) and read Result.StdErrorS3Uri.
Cause 5: on an IDLE Spark session StartCalculationExecution with CodeBlock 'import time\ntime.sleep(90)\n', then immediately StartCalculationExecution again with any other code — expect the 400 refusal and the session to still list only the first calculation.
Cause 6: StartSession(WorkGroup=wg, EngineConfiguration=..., Description='first request', ClientRequestToken=<32-hex>), then repeat with the same token and Description='changed request' — expect the first SessionId back and GetSession to still report Description 'first request'.
Cause 7: StartSession(WorkGroup=<Spark workgroup>, EngineConfiguration=..., Tags=[{'Key':'k','Value':'v'}]) — expect the 400 refusal and no new session in ListSessions for that workgroup.
Cause 8: CreateWorkGroup in us-west-2, then GetWorkGroup and ListWorkGroups against the us-east-2 endpoint with the same name.
