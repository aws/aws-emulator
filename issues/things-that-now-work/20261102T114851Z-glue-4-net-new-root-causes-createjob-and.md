Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.gjwr-create-job-unknown-glue-version-rejected glue.gjwr-create-job-worker-type-without-worker-count-rejected glue.glue-tsd-get-tables-forged-next-token-rejected glue.glue-tsd-get-tables-max-results-over-1000-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.gjwr-create-job-unknown-glue-version-rejected|_error#5|AssertionError|raise AssertionError("the call under test SUCCEEDED where real AWS refuses it")
Issue-Key: glue.gjwr-create-job-worker-type-without-worker-count-rejected|_error#5|AssertionError|raise AssertionError("the call under test SUCCEEDED where real AWS refuses it")
Issue-Key: glue.glue-tsd-get-tables-forged-next-token-rejected|_error#15|AssertionError|raise AssertionError(f"the call succeeded and returned {result!r}; real AWS rejects it")
Issue-Key: glue.glue-tsd-get-tables-max-results-over-1000-rejected|_error#6|AssertionError|raise AssertionError("the call succeeded; real AWS rejects it")
Issue-Commit: 9b689e7a3706c4d3e9e7bd76513ebd3e34560844

# Glue: 4 net-new root causes — CreateJob and GetTables accept input AWS refuses (unknown GlueVersion, WorkerType without NumberOfWorkers, forged NextToken, MaxResults over the ceiling) and answer 200

> _Note: this issue was identified against the code base at commit `9b689e7a3706c4d3e9e7bd76513ebd3e34560844`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateJob

## Description
Four request shapes that real Glue refuses with a 400 are accepted by the emulator and answered 200, so the emulator mints state (a job) or serves a page where AWS produces an error. The open issue 20261029T201859Z-glue-accepts-requests-aws-refuses-ten-create-delete.md covers ten OTHER create/delete paths and names none of the four below, so it does not cover these causes. FOUR causes must be closed before this issue is submitted.

Cause 1 of 4: CreateJob with a GlueVersion the service does not offer.
AWS: Smithy: CreateJob.GlueVersion targets GlueVersionString (length 1-255, pattern ^(\\w+\\.)+\\w+$), so '9.9' is syntactically valid — the value is additionally checked against the service's catalogue of offered versions and refused with the modeled InvalidInputException (client, httpError 400), message "Glue version: <value> is invalid" (fixed text 'Glue version: ' / ' is invalid' around the GlueVersion from the request). Nothing is created: a following GetJob is EntityNotFoundException.
Emulator: the call SUCCEEDS (200) and the job exists afterwards with GlueVersion '9.9'.
Why: a typo or a version that has been retired is silently accepted, so the client believes it has a runnable job on a runtime that does not exist, and the failure surfaces far later (or never) instead of at CreateJob.
Fix: the CreateJob handler's validation — validate GlueVersion against the offered-version catalogue, not only the modeled pattern.
Traces: glue.gjwr-create-job-unknown-glue-version-rejected

Cause 2 of 4: CreateJob with WorkerType but no NumberOfWorkers.
AWS: Smithy: WorkerType (enum) and NumberOfWorkers (NullableInteger) are both optional members, but the worker capacity model requires them together; the half-specified pair is refused with InvalidInputException (client, 400), message "Please set both Worker Type and Number of Workers." (fixed text, no variable parts), and no job is left behind (GetJob -> EntityNotFoundException, 'Job not found.').
Emulator: the call SUCCEEDS (200) and creates the job with a worker type and no worker count.
Why: the job is created in a capacity configuration AWS cannot run; the client gets no signal that its capacity request was incomplete.
Fix: the CreateJob handler's capacity validation (the same branch as the mutually-exclusive MaxCapacity check), which currently only rejects the over-specified combination.
Traces: glue.gjwr-create-job-worker-type-without-worker-count-rejected

Cause 3 of 4: GetTables with a NextToken the service never issued.
AWS: Smithy: GetTables.NextToken targets Token and the operation declares InvalidInputException (client, 400). A pagination token is validated before use: a forged token is refused with InvalidInputException, message "Invalid token value" (fixed text, no variable parts). A token the service DID issue stays replayable and returns the same page twice.
Emulator: GetTables(NextToken='forged-token-0000') answers 200 with a page of results (recorded: the TableList page containing table 'second'), i.e. an unissued token is treated as a valid cursor.
Why: a truncated, corrupted or cross-database token silently yields an arbitrary page instead of an error, so a paginating client can skip or duplicate tables and never learns its cursor was invalid.
Fix: the GetTables pagination-token decode path — reject tokens the emulator did not issue for this request shape instead of falling back to a default page.
Traces: glue.glue-tsd-get-tables-forged-next-token-rejected

Cause 4 of 4: GetTables with MaxResults above the ceiling.
AWS: Smithy: GetTables.MaxResults targets CatalogGetterPageSize with @range min 1, max 100 in the on-disk model; the recorded service accepts 1000 (its effective documented ceiling) and refuses 1001 with ValidationException, message "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000" — fixed text apart from the value from the request and the ceiling.
Emulator: GetTables(MaxResults=1001) SUCCEEDS (200); no range validation is applied to MaxResults at all.
Why: the server-side page-size guard is gone, so a caller asking for an out-of-range page gets a 200 and cannot discover the limit from the API; the refusal that bounds a request's cost is missing.
Fix: the GetTables input validation (the MaxResults range check), which must emit the standard ValidationException sentence rooted at 'maxResults'.
Traces: glue.glue-tsd-get-tables-max-results-over-1000-rejected

## Evidence
Cause 1 — expected ('InvalidInputException', 'Glue version: 9.9 is invalid'); got: 'the call under test SUCCEEDED where real AWS refuses it' (CreateJob 200, job present afterwards).
Cause 2 — expected ('InvalidInputException', 'Please set both Worker Type and Number of Workers.'); got: 'the call under test SUCCEEDED where real AWS refuses it'.
Cause 3 — expected ('InvalidInputException', 'Invalid token value'); got HTTP 200 with {'TableList': [{'Name': 'second', 'DatabaseName': 'sim2real_tsd_tok_<id>', ...}], 'ResponseMetadata': {'HTTPStatusCode': 200, ...}} for NextToken='forged-token-0000'.
Cause 4 — expected ('ValidationException', "1 validation error detected: Value '1001' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 1000"); got: 'the call succeeded; real AWS rejects it' (MaxResults=1001 answered 200; the MaxResults=1000 control also succeeds).

## Steps to Reproduce
Replay the behavioral suite and re-run the listed checks.
1. CreateJob(Name=<n>, Role=<arn>, GlueVersion='9.9', Command={'Name':'glueetl','PythonVersion':'3','ScriptLocation':'s3://.../job.py'}) -> expect 400 InvalidInputException; observe 200, then GetJob(<n>) resolves.
2. CreateJob(Name=<n>, Role=<arn>, GlueVersion='4.0', WorkerType='G.1X') with no NumberOfWorkers -> expect 400 InvalidInputException; observe 200.
3. CreateDatabase, CreateTable x2, then GetTables(DatabaseName=<db>, NextToken='forged-token-0000') -> expect 400 InvalidInputException 'Invalid token value'; observe a 200 page.
4. GetTables(DatabaseName=<db>, MaxResults=1001) -> expect 400 ValidationException; observe 200.
