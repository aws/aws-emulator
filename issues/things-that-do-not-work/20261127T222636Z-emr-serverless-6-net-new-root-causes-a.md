Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: emr-serverless.get-dashboard-for-job-run-unknown-attempt-rejected emr-serverless.list-job-runs-failed-filter-reports-raising-spark-script emr-serverless.list-job-runs-multi-state-filter-groups-by-state emr-serverless.list-job-runs-unissued-next-token-rejected emr-serverless.start-job-run-reused-client-token-with-changed-request-rejected emr-serverless.start-session-reused-client-token-with-changed-request-rejected
Issue-Shape-Id: com.amazonaws.emrserverless#AwsToledoWebService
Issue-Key: emr-serverless.get-dashboard-for-job-run-unknown-attempt-rejected|run#12|AssertionError|assert final["state"] == "SUCCESS", (
Issue-Key: emr-serverless.list-job-runs-failed-filter-reports-raising-spark-script|_await#17|AssertionError|raise AssertionError(f"{what} never reached {sorted(wanted)}; last read {value!r}")
Issue-Key: emr-serverless.list-job-runs-multi-state-filter-groups-by-state|run#18|AssertionError|assert changes == 1, f"the two-state filter returned states in the order {order!r}, not grouped by state"
Issue-Key: emr-serverless.list-job-runs-unissued-next-token-rejected|run#19|AssertionError|assert got == ("ValidationException", "Invalid token encountered"), f"an unissued nextToken was answered {got!r}"
Issue-Key: emr-serverless.start-job-run-reused-client-token-with-changed-request-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: emr-serverless.start-session-reused-client-token-with-changed-request-rejected|_refusal#15|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Commit: 0f328bb6ae93185eda2a76c9c7fc6646652b63ca

# emr-serverless: 6 net-new root causes — a local:// JAR sparkSubmit entry point is never executed, a job run can stick in SUBMITTED forever, the ListJobRuns multi-state filter is not grouped by state, its unissued-nextToken refusal is emulator prose, and StartJobRun/StartSession never detect a client-token conflict

> _Note: this issue was identified against the code base at commit `0f328bb6ae93185eda2a76c9c7fc6646652b63ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartJobRun

## Description
Six net-new root causes in the EMR Serverless job-run / session plane, all reached through the emr-serverless API (`ctx.client('emr-serverless')`), which is stood up and answering (46/52 of its own behavioral checks pass). Each cause is numbered with its own failing check ids; a fixer must close all six before submitting.

Smithy model used throughout: services/service-com-amazonaws-emrserverless-awstoledowebservice/smithy-model/com-amazonaws-emrserverless-awstoledowebservice.json (com.amazonaws.emrserverless#AwsToledoWebService).

--- Cause 1 of 6: a sparkSubmit job run whose entryPoint is a JAR already on the image (local:// URI) is never executed and settles FAILED. Failing check: emr-serverless.get-dashboard-for-job-run-unknown-attempt-rejected
AWS: Smithy StartJobRun takes jobDriver -> JobDriver -> SparkSubmit.entryPoint, documented only as "The entry point for the Spark submit job run." — the member is an arbitrary Spark entry point, not restricted to a Python object in Amazon S3; the companion sparkSubmitParameters carries `--class org.apache.spark.examples.SparkPi`, i.e. a JVM main class inside a JAR. JobRunState enumerates SUBMITTED, PENDING, SCHEDULED, RUNNING, SUCCESS, FAILED, CANCELLING, CANCELLED, QUEUED; FAILED is for a run whose driver actually ran and exited non-zero (or that could not be placed). A well-formed SparkPi run against `local:///usr/lib/spark/examples/jars/spark-examples.jar` with `--class org.apache.spark.examples.SparkPi` on a CREATED application with an assumable execution role progresses through the non-terminal states and settles SUCCESS, after which ListJobRunAttempts reports exactly attempt 1, GetDashboardForJobRun(attempt=1) mints an https URL, and GetDashboardForJobRun for an attempt the run never made is refused ResourceNotFoundException "Job Run with given attempt doesn't exist" (fixed text; no variable part). Recorded behavior only for the exact intermediate-state progression.
Emulator: StartJobRun returns 200 with a jobRunId, then GetJobRun settles terminal state 'FAILED' with stateDetails 'Job run not executed: this emulator runs a Spark submit whose entry point is a Python application in Amazon S3.' — the emulator's own prose, announcing that it only honours a Python entry point in S3. (Note: this is a DIFFERENT shape of gap from the already-open 20261022T154846Z-emr-serverless-job-runs-are-recorded-but-never-executed.md / 20261024T013257Z-emr-serverless-job-runs-never-execute-the-submitted.md, which cover a Python-in-S3 entry point not running at all; those are not satisfied by, and do not cover, a JAR/`--class` entry point being rejected by the executor as an unsupported entry-point form. Other checks in this same suite show Python-in-S3 entry points now DO reach SUCCESS, so the remaining gap is specifically the JAR entry point.)
Why: the whole class of JVM Spark work — every JAR entry point, which is how Scala/Java Spark jobs and the AWS-documented SparkPi smoke test are submitted — is unrunnable. A client that submits a JAR job and gates on SUCCESS takes its error path permanently, and because FAILED is also AWS's genuine-failure state the emulator makes a real job failure indistinguishable from "not supported". Everything downstream of a finished run (ListJobRunAttempts, GetDashboardForJobRun for a valid and an invalid attempt) is unreachable, so the dashboard refusal fidelity cannot be graded at all.
Fix: the job-run execution/state machine behind StartJobRun and GetJobRun in the emr-serverless ops layer (start_job_run.rs / get_job_run.rs and the run-lifecycle state it drives) must accept a JAR entry point with a `--class` main class as an executable driver form rather than short-circuiting it to FAILED with an entry-point-form complaint.
Traces: emr-serverless.get-dashboard-for-job-run-unknown-attempt-rejected (assertion `final["state"] == "SUCCESS"`).

--- Cause 2 of 6: a job run submitted alongside another in-flight run of the same application never leaves SUBMITTED. Failing check: emr-serverless.list-job-runs-failed-filter-reports-raising-spark-script
AWS: SUBMITTED is the first JobRunState; a run leaves it and advances through PENDING/SCHEDULED/QUEUED/RUNNING to a terminal SUCCESS/FAILED/CANCELLED. JobRunSummary requires `state` and `stateDetails`, and a run whose driver script raises reaches FAILED with stateDetails naming the driver exception (the check asserts the Python RuntimeError and its message appear there). Two runs submitted back-to-back on one application with autoStartConfiguration.enabled=true and maximumCapacity 8vCPU/32GB both get placed — serialized behind QUEUED if capacity requires — and both settle terminally. Recorded behavior only for the pace.
Emulator: the first (succeeding) run is read back as 'SUBMITTED' on every GetJobRun poll for the full wait budget and never advances to any other state; the check dies on "succeeding job never reached ['CANCELLED', 'FAILED', 'SUCCESS']; last read 'SUBMITTED'". Hypothesis: the run-lifecycle driver only advances one run per application, or only advances a run once the application has auto-started, so a second concurrently submitted run (and here the first of a pair) is left parked in the initial state with nothing scheduled to move it.
Why: a client that submits more than one job run to an application — the normal use of EMR Serverless — hangs forever on the first poll loop instead of getting a terminal state. There is no timeout and no error: the run is simply never scheduled, so orchestrators (Step Functions .sync, Airflow sensors, shell pollers) block until their own deadline. It also makes the ListJobRuns `states` filter untestable for the FAILED/SUCCESS split, and the driver-exception text AWS puts in stateDetails never appears.
Fix: the job-run scheduler/state machine in the emr-serverless run-lifecycle state (the component that advances SUBMITTED -> ... -> terminal behind StartJobRun/GetJobRun) must advance every submitted run of an application, concurrently or via QUEUED, rather than leaving a run in SUBMITTED with no transition pending; the terminal FAILED stateDetails must carry the driver's own exception type and message.
Traces: emr-serverless.list-job-runs-failed-filter-reports-raising-spark-script (assertion in `_await` for the succeeding job's terminal state).

--- Cause 3 of 6: ListJobRuns with a multi-state `states` filter returns runs in creation order, not grouped by state. Failing check: emr-serverless.list-job-runs-multi-state-filter-groups-by-state
AWS: Smithy ListJobRuns takes `states` -> JobRunStateSet (httpQuery `states`, uniqueItems, 0..8) and returns `jobRuns` -> JobRuns plus `nextToken`; it is @paginated (inputToken/outputToken nextToken, pageSize maxResults, items jobRuns). Recorded behavior only for the ordering: when the filter names two states, AWS returns the matching summaries grouped by state — the listing contains exactly one state change across the sequence — and paging the same filter one run per page concatenates to exactly the single-page sequence.
Emulator: for three runs submitted in the order early/middle/late and settling SUCCESS, CANCELLED, SUCCESS, a `states=[SUCCESS, CANCELLED]` listing returns the order ['SUCCESS', 'CANCELLED', 'SUCCESS'] — two state changes — i.e. creation-time order, ungrouped. The run set itself is correct, only the ordering diverges.
Why: a client that pages a two-state listing and relies on the state blocks (to stop reading once it has passed the state it cares about, or to batch per state) reads the wrong runs; the page boundaries an emulator-backed test records do not match the ones real AWS produces, so pagination tests written against AWS fail or silently pass on the wrong data.
Fix: the ordering/sort applied to the filtered summaries in the ListJobRuns handler in the emr-serverless ops layer (list_job_runs.rs), before pagination is applied, so that single-page and paged listings agree.
Traces: emr-serverless.list-job-runs-multi-state-filter-groups-by-state (assertion `changes == 1`).

--- Cause 4 of 6: ListJobRuns refuses a well-formed but never-issued nextToken with an emulator-authored message. Failing check: emr-serverless.list-job-runs-unissued-next-token-rejected
AWS: Smithy ListJobRuns.nextToken -> NextToken, httpQuery `nextToken`, @length 1..1024, @pattern ^[A-Za-z0-9_=-]+$; modeled errors are ValidationException (400) and InternalServerException (500). A token that satisfies the pattern but was never handed out by a previous page is refused 400 ValidationException with the message "Invalid token encountered" — that whole sentence is fixed text, with no part taken from the request (the token value is NOT echoed and no member name is appended).
Emulator: the same call is refused ValidationException with the message 'Member must be a nextToken returned by a previous ListJobRuns call: nextToken' — emulator-authored prose in the shape of a constraint-violation report, with the member name 'nextToken' appended after a colon. The error code and status are right; only the message text diverges.
Why: a client or test that matches on the refusal text (a retry/pagination-reset path keyed on "Invalid token encountered", or a snapshot assertion on the message) does not match and takes its unknown-error branch, and anything that surfaces the message to a user shows wording AWS never emits.
Fix: the nextToken validation/refusal text in the ListJobRuns handler in the emr-serverless ops layer (list_job_runs.rs) — emit AWS's fixed "Invalid token encountered" sentence for a token that does not resolve to an issued page, instead of the generic member-constraint template.
Traces: emr-serverless.list-job-runs-unissued-next-token-rejected (assertion `got == ("ValidationException", "Invalid token encountered")`).

--- Cause 5 of 6: StartJobRun does not detect a client-token conflict — a reused token with a changed request is accepted instead of refused ConflictException. Failing check: emr-serverless.start-job-run-reused-client-token-with-changed-request-rejected
AWS: Smithy StartJobRun.clientToken -> ClientToken is @required and @idempotencyToken (@length 1..64, @pattern ^[A-Za-z0-9._-]+$); the operation is @idempotent and models ConflictException (409, "The request could not be processed because of conflict in the current state of the resource"). Replaying the IDENTICAL request under the same token returns the original run's ids (idempotent replay). Reusing that same token with any request member changed — here `name` changed from the first run's name to a different one — is refused 409 ConflictException with the message "Request did not match the original": the entire sentence is fixed text with no value from the request in it. No second run is started, and the original run is left untouched (its `name` is unchanged, and ListJobRuns still reports only the first run). The changed request IS legal under a fresh token and then produces a new jobRunId.
Emulator: the conflicting StartJobRun returns HTTP 200 with a StartJobRunResponse — in the replay `{'applicationId': ..., 'jobRunId': ..., 'arn': 'arn:aws:emr-serverless:<region>:<account>:/applications/<applicationId>/jobruns/<jobRunId>'}`, whose jobRunId is the ORIGINAL run's id (the generated ids/region/account in that response are per-run values, not part of the requirement). So the token is treated as a pure replay key and the request body is never compared against the one the token was first used with; the 409 is never produced.
Why: the idempotency contract inverts. A caller that retries with a token after editing its request believes AWS will protect it and silently gets the OLD run back under the NEW request's intent — the edited job never runs and nothing reports that. A client with a ConflictException branch (fix the token, re-submit) never reaches it, and any negative test of client-token reuse passes vacuously.
Fix: client-token handling in the StartJobRun handler in the emr-serverless ops layer (start_job_run.rs) and the idempotency record it keeps in service state — store a fingerprint of the first request under the token and refuse ConflictException when a later call presents the same token with a different request, rather than returning the stored run unconditionally.
Traces: emr-serverless.start-job-run-reused-client-token-with-changed-request-rejected (assertion in `_refusal`: "the call was expected to be refused but returned ...").

--- Cause 6 of 6: StartSession does not detect a client-token conflict — a reused token with a changed request is accepted instead of refused ValidationException. Failing check: emr-serverless.start-session-reused-client-token-with-changed-request-rejected
AWS: Smithy StartSession.clientToken -> ClientToken is @required and @idempotencyToken; the operation is @idempotent and models ValidationException (400) and ConflictException (409). On a session-enabled application (interactiveConfiguration.sessionEnabled=true), replaying the identical request under the same token returns the original sessionId. Reusing that token with a changed member (`name`) is refused 400 ValidationException with the message "Request did not match the original" — whole-sentence fixed text, nothing taken from the request. No second session is opened (ListSessions still reports only the first) and the original session's `name` is unchanged; the changed request is legal under a fresh token and yields a new sessionId.
Emulator: the conflicting StartSession returns HTTP 200 with a StartSessionResponse — in the replay `{'applicationId': ..., 'sessionId': ..., 'arn': 'arn:aws:emr-serverless:<region>:<account>:/applications/<applicationId>/sessions/<sessionId>'}` carrying the ORIGINAL session's id (those ids are per-run values, not the requirement) — so the request body is never compared with the one first seen under that token and the refusal never happens. Note AWS's refusal code here is ValidationException, NOT the ConflictException used by StartJobRun in cause 5; the two must not be unified onto one code.
Why: the same inversion as cause 5, on the interactive-session path: a notebook/IDE client that re-sends an edited StartSession under the old token gets the previous session handed back and runs its new configuration nowhere, with no error. Clients branching on ValidationException "Request did not match the original" never see it.
Fix: client-token handling in the StartSession handler in the emr-serverless ops layer (start_session.rs) and its idempotency record in service state — compare the presented request against the one stored under the token and refuse ValidationException "Request did not match the original" on a mismatch.
Traces: emr-serverless.start-session-reused-client-token-with-changed-request-rejected (assertion in `_refusal`: "the call was expected to be refused but returned ...").

## Evidence
Replay of simparity-behavioral-aws-v1 against this checkout: 46/56 passed (82.1%), 9 mismatched, 1 errored. Per-service: emr-serverless 46/52 (88%), 6 failed. The other 4 failures (athena__emr-serverless__glue__s3 x2, cloudformation__emr-serverless x2) are already OWNED by open issues and are not part of this issue.

Cause 1 — [mismatch] emr-serverless.get-dashboard-for-job-run-unknown-attempt-rejected
  check_emr_serverless_get_dashboard_for_job_run_unknown_attempt_rejected.py line 145: assert final["state"] == "SUCCESS"
  AssertionError: the SparkPi run ended 'FAILED': 'Job run not executed: this emulator runs a Spark submit whose entry point is a Python application in Amazon S3.'
  Expected (AWS): GetJobRun eventually reports jobRun.state == 'SUCCESS' for the SparkPi run whose entryPoint is 'local:///usr/lib/spark/examples/jars/spark-examples.jar' with '--class org.apache.spark.examples.SparkPi'; then ListJobRunAttempts == [1], GetDashboardForJobRun(attempt=1) returns an https url, and GetDashboardForJobRun(attempt=2) is refused ResourceNotFoundException "Job Run with given attempt doesn't exist".
  Actual (emulator): terminal FAILED with the emulator's own stateDetails; the attempt/dashboard assertions are never reached.

Cause 2 — [mismatch] emr-serverless.list-job-runs-failed-filter-reports-raising-spark-script
  check_emr_serverless_list_job_runs_failed_filter_reports_raising_spark_script.py line 84 -> line 203 in _await
  AssertionError: succeeding job never reached ['CANCELLED', 'FAILED', 'SUCCESS']; last read 'SUBMITTED'
  Expected (AWS): both runs submitted to the application settle terminally (SUCCESS for the good script, FAILED for the raising one, with stateDetails naming the RuntimeError and its message).
  Actual (emulator): the first run is read back 'SUBMITTED' on every poll for the whole budget and never transitions.

Cause 3 — [mismatch] emr-serverless.list-job-runs-multi-state-filter-groups-by-state
  check_emr_serverless_list_job_runs_multi_state_filter_groups_by_state.py line 219: assert changes == 1
  AssertionError: the two-state filter returned states in the order ['SUCCESS', 'CANCELLED', 'SUCCESS'], not grouped by state
  Expected (AWS): states=[SUCCESS, CANCELLED] returns the summaries grouped by state (exactly one state change in the sequence), and PageSize=1 paging concatenates to the same sequence.
  Actual (emulator): creation-time order, two state changes.

Cause 4 — [mismatch] emr-serverless.list-job-runs-unissued-next-token-rejected
  check_emr_serverless_list_job_runs_unissued_next_token_rejected.py line 228: assert got == ("ValidationException", "Invalid token encountered")
  AssertionError: an unissued nextToken was answered ('ValidationException', 'Member must be a nextToken returned by a previous ListJobRuns call: nextToken')
  Expected (AWS): 400 ValidationException, message exactly "Invalid token encountered" (all fixed text).
  Actual (emulator): 400 ValidationException, message 'Member must be a nextToken returned by a previous ListJobRuns call: nextToken'.

Cause 5 — [mismatch] emr-serverless.start-job-run-reused-client-token-with-changed-request-rejected
  check_emr_serverless_start_job_run_reused_client_token_with_changed_request_rejected.py line 146 -> line 34 in _refusal
  AssertionError: the call was expected to be refused but returned {'applicationId': '0000000000000001', 'jobRunId': '0000000000000001', 'arn': 'arn:aws:emr-serverless:us-west-2:000000000000:/applications/0000000000000001/jobruns/0000000000000001'} (those ids are this run's generated values)
  Expected (AWS): 409 ConflictException, message exactly "Request did not match the original"; no second run created; original run's name unchanged.
  Actual (emulator): HTTP 200 StartJobRunResponse echoing the ORIGINAL run's jobRunId.

Cause 6 — [mismatch] emr-serverless.start-session-reused-client-token-with-changed-request-rejected
  check_emr_serverless_start_session_reused_client_token_with_changed_request_rejected.py line 135 -> line 24 in _refusal
  AssertionError: the call was expected to be refused but returned {'applicationId': '0000000000000001', 'sessionId': '0000000000000001', 'arn': 'arn:aws:emr-serverless:us-west-2:000000000000:/applications/0000000000000001/sessions/0000000000000001'} (generated values)
  Expected (AWS): 400 ValidationException, message exactly "Request did not match the original"; no second session; original session's name unchanged.
  Actual (emulator): HTTP 200 StartSessionResponse echoing the ORIGINAL sessionId.

## Steps to Reproduce
Boot the emulator from this checkout, point botocore at it, and replay the six checks in trace_ids. By hand:

Cause 1 (local:// JAR entry point never executed):
1. iam CreateRole trusted by emr-serverless.amazonaws.com.
2. emr-serverless CreateApplication(releaseLabel='emr-7.13.0', type='SPARK'); poll GetApplication until state CREATED.
3. StartJobRun(executionRoleArn=<role>, jobDriver={'sparkSubmit': {'entryPoint': 'local:///usr/lib/spark/examples/jars/spark-examples.jar', 'entryPointArguments': ['1'], 'sparkSubmitParameters': '--class org.apache.spark.examples.SparkPi --conf spark.executor.instances=1 --conf spark.executor.cores=1 --conf spark.executor.memory=2g --conf spark.driver.cores=1 --conf spark.driver.memory=2g'}}).
4. Poll GetJobRun -> expect SUCCESS; observe FAILED with stateDetails 'Job run not executed: this emulator runs a Spark submit whose entry point is a Python application in Amazon S3.'
5. Then: ListJobRunAttempts -> expect [1]; GetDashboardForJobRun(attempt=1) -> expect an https url; GetDashboardForJobRun(attempt=2) -> expect ResourceNotFoundException "Job Run with given attempt doesn't exist".

Cause 2 (run stuck in SUBMITTED):
1. Create a bucket, put two PySpark scripts (one that completes, one that raises RuntimeError) under code/.
2. CreateApplication(releaseLabel='emr-7.0.0', type='SPARK', autoStartConfiguration={'enabled': True}, autoStopConfiguration={'enabled': True, 'idleTimeoutMinutes': 1}, maximumCapacity={'cpu':'8vCPU','memory':'32GB','disk':'200GB'}); poll to CREATED.
3. StartJobRun for the good script, then immediately StartJobRun for the raising script (both s3:// Python entry points).
4. Poll GetJobRun for the first run -> expect it to leave SUBMITTED and reach SUCCESS; observe 'SUBMITTED' on every poll for the whole budget.

Cause 3 (multi-state ordering):
1. CreateApplication(releaseLabel='emr-7.9.0', autoStart enabled); poll to CREATED.
2. StartJobRun three times in the order early, middle, late with a trivial s3:// PySpark entry point, waiting for each to appear in ListJobRuns.
3. CancelJobRun the middle run; wait for all three terminal states (SUCCESS, CANCELLED, SUCCESS).
4. ListJobRuns(states=['SUCCESS','CANCELLED']) -> expect the states grouped (one change); observe ['SUCCESS','CANCELLED','SUCCESS'].
5. Repeat with PaginationConfig PageSize=1 and compare to the single-page listing.

Cause 4 (unissued nextToken message):
1. On an application with two listable job runs, ListJobRuns(maxResults=1) and keep the issued nextToken.
2. ListJobRuns(maxResults=1, nextToken='Zm9yZ2VkLXBhZ2UtdG9rZW4=') — pattern-valid, never issued -> expect ValidationException "Invalid token encountered"; observe 'Member must be a nextToken returned by a previous ListJobRuns call: nextToken'.
3. Confirm the issued token still resumes the listing at the second run.

Cause 5 (StartJobRun token conflict):
1. CreateApplication, poll to CREATED; build request R = {applicationId, clientToken=T, executionRoleArn, jobDriver=<SparkPi>, name='pi-first'}.
2. StartJobRun(R) -> note jobRunId; StartJobRun(R) again -> expect the same jobRunId (idempotent replay; this part passes).
3. StartJobRun({**R, 'name': 'pi-second'}) -> expect 409 ConflictException "Request did not match the original"; observe HTTP 200 returning the original jobRunId.
4. Confirm no second run exists (ListJobRuns names only 'pi-first') and GetJobRun still reports name 'pi-first'; then confirm the changed request succeeds under a FRESH clientToken with a new jobRunId.

Cause 6 (StartSession token conflict):
1. CreateApplication(releaseLabel='emr-7.13.0', type='SPARK', interactiveConfiguration={'sessionEnabled': True}); poll to CREATED.
2. StartSession(R = {applicationId, clientToken=T, executionRoleArn, name='notebook-first'}) -> note sessionId; replay R -> expect the same sessionId.
3. StartSession({**R, 'name': 'notebook-second'}) -> expect 400 ValidationException "Request did not match the original"; observe HTTP 200 returning the original sessionId.
4. Confirm ListSessions names only 'notebook-first', GetSession still reports 'notebook-first', and the changed request succeeds under a fresh clientToken.
