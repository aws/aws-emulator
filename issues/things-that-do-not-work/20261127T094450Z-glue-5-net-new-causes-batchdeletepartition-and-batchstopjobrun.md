Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.batch-delete-partition-reports-missing-value-entity-not-found glue.batch-stop-job-run-stops-the-running-run-and-refuses-an-unknown-id glue.create-partition-values-over-aggregate-byte-limit-rejected glue.get-statement-nonexistent-rejected glue.start-workflow-run-over-max-concurrent-runs-rejected
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.batch-delete-partition-reports-missing-value-entity-not-found|run#26|AssertionError|assert got == ("EntityNotFoundException", "Partition not found"), (
Issue-Key: glue.batch-stop-job-run-stops-the-running-run-and-refuses-an-unknown-id|run#24|AssertionError|assert (detail.get("ErrorCode"), detail.get("ErrorMessage")) == (
Issue-Key: glue.create-partition-values-over-aggregate-byte-limit-rejected|_refusal#17|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {answer!r}")
Issue-Key: glue.get-statement-nonexistent-rejected|_retry_get_statement_error#11|Timeout|time.sleep(interval_s)
Issue-Key: glue.start-workflow-run-over-max-concurrent-runs-rejected|run#19|AssertionError|assert (code, message.replace(workflow, "<workflow>")) == (
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# glue: 5 net-new causes — BatchDeletePartition and BatchStopJobRun per-entry messages are emulator-authored, CreatePartition/GetPartition never enforce the 1024-byte aggregate partition-value limit, the interactive-session GetStatement path makes no progress, and StartWorkflowRun's ConcurrentRunsExceededException carries a trailing period

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchStopJobRun

## Description
Five net-new root causes in the glue service, each with its own failing check.

Cause 1 of 5: BatchDeletePartition's per-entry ErrorDetail message for a value set the table never held is emulator-authored.
AWS: BatchDeletePartition returns 200 with Errors[], one PartitionError per value set that could not be deleted, whose ErrorDetail is ErrorCode `EntityNotFoundException` with ErrorMessage `Partition not found` (Smithy: BatchDeletePartition -> PartitionErrors, ErrorDetail{ErrorCode,ErrorMessage}; EntityNotFoundException is modeled on the operation). The whole message is fixed text — it names no request value.
Emulator: the same entry carries ErrorCode `EntityNotFoundException` but ErrorMessage `Cannot find partition.` — different fixed text, plus a trailing period AWS does not send.
Why: a client that classifies partial-batch failures on the message (and the many tools that log/compare it) sees text no AWS region produces, so partial-failure handling keyed on it diverges.
Fix: the per-entry error construction in the BatchDeletePartition handler of the glue service crate.
Traces: glue.batch-delete-partition-reports-missing-value-entity-not-found.

Cause 2 of 5: BatchStopJobRun's per-entry ErrorDetail message for an unknown run id is the GetJobRun wording, not the batch wording.
AWS: BatchStopJobRun returns 200 with SuccessfulSubmissions for the live runs and one Errors entry per unknown id, naming JobName and JobRunId, whose ErrorDetail is ErrorCode `EntityNotFoundException` with ErrorMessage `Job Run not found.` (Smithy: BatchStopJobRun -> BatchStopJobRunErrorList with ErrorDetail). That fixed text differs from the single-run read path, where GetJobRun's refusal reads `Job run not found for the given job name and runId.`.
Emulator: the batch entry carries `{'ErrorCode': 'EntityNotFoundException', 'ErrorMessage': 'Job run not found for the given job name and runId.'}` — the GetJobRun sentence reused for the batch entry.
Why: two distinct AWS error texts are collapsed into one, so a client cannot tell a batch-entry rejection from a read rejection and any message assertion on the batch path fails.
Fix: the per-entry error construction in the BatchStopJobRun handler (it must use the batch-specific message constant, not the job-run read one).
Traces: glue.batch-stop-job-run-stops-the-running-run-and-refuses-an-unknown-id.

Cause 3 of 5: the 1024-byte aggregate limit on partition values is not enforced by CreatePartition or GetPartition.
AWS: a partition's values are limited in aggregate to 1024 bytes (UTF-8 bytes of each value plus one per value); a PartitionInput one byte over is refused with 400 `InvalidInputException` and message `Aggregated size of partition values cannot exceed 1024 bytes` (fixed text, no request values in it; Smithy: CreatePartition/GetPartition both model InvalidInputException). A lookup by the same over-limit values is held to the same limit, and the partition is not stored. The value set exactly at 1024 bytes is accepted and readable (positive control passes today).
Emulator: CreatePartition with 1025 aggregate bytes answers HTTP 200 with an empty body (no refusal at all), so the over-limit partition is stored and listed; GetPartition by those values likewise does not refuse. Note that each individual value is within its own length limit, so only the aggregate rule can reject it.
Why: a client is allowed to create partitions AWS rejects; the catalog then holds entities production could never hold, and the client's error path for over-long partition values is never exercised — the divergence only surfaces later, against real AWS.
Fix: input validation in the CreatePartition and GetPartition handlers (an aggregate byte-length check on PartitionInput.Values / PartitionValues, computed over UTF-8 bytes, not characters).
Traces: glue.create-partition-values-over-aggregate-byte-limit-rejected.

Cause 4 of 5: the interactive-session statement path makes no progress, so the GetStatement refusal is unreachable.
AWS: CreateSession on a glue-assumable role reaches Session.Status READY, RunStatement returns a statement Id that reaches AVAILABLE (or ERROR), GetStatement on that Id succeeds, and GetStatement for an Id the session never issued is refused with `IllegalSessionStateException` and message `Illegal Session Status` (Smithy: GetStatement models IllegalSessionStateException and EntityNotFoundException; the exact text is recorded behavior).
Emulator: the check reaches no verdict at all — it is cut off with `Check timed out after 60.0s` while driving CreateSession -> READY -> RunStatement -> AVAILABLE -> GetStatement, so no refusal is ever observed. Hypothesis: the session or its statements never leave their initial state, so the poll never terminates.
Why: every client of Glue interactive sessions hangs instead of receiving a terminal session/statement state, and the IllegalSessionStateException class is unobservable. This same check was previously filed and marked fixed as 20261121T093122Z-glue-the-getstatement-refusal-check-on-an-interactive.md; it again reaches no verdict (a timeout, not a refusal), so that fix does not hold for this path.
Fix: the session/statement state machine behind CreateSession, RunStatement, GetStatement and GetSession in the glue service crate (state must advance to READY/AVAILABLE), and then the GetStatement refusal for an unknown Id.
Traces: glue.get-statement-nonexistent-rejected.

Cause 5 of 5: StartWorkflowRun's ConcurrentRunsExceededException message carries a trailing period AWS does not send.
AWS: with MaxConcurrentRuns 2 and two RUNNING runs, the third StartWorkflowRun is refused 400 `ConcurrentRunsExceededException` with message `Concurrent runs exceeded for workflow: <workflow-name>` — the fixed text ends at the colon-space and the workflow name from the request, with no terminal period (Smithy: StartWorkflowRun models ConcurrentRunsExceededException).
Emulator: the message is `Concurrent runs exceeded for workflow: <workflow-name>.` — the same fixed text plus a trailing '.'.
Why: a client matching the refusal text exactly (or diffing it) sees a message no AWS region emits. This is the same trailing-period defect class as the already-open 20261126T202144Z-glue-createcatalog-duplicate-name-alreadyexistsexception-message-carries-a--r2.md, which covers CreateCatalog only and not StartWorkflowRun.
Fix: the message constant used by the concurrency check in the StartWorkflowRun handler of the glue service crate.
Traces: glue.start-workflow-run-over-max-concurrent-runs-rejected.

## Evidence
Cause 1: `AssertionError: the never-held value's ErrorDetail was ('EntityNotFoundException', 'Cannot find partition.')` — expected ('EntityNotFoundException', 'Partition not found').

Cause 2: `AssertionError: the unknown id was refused with {'ErrorCode': 'EntityNotFoundException', 'ErrorMessage': 'Job run not found for the given job name and runId.'}` — expected ('EntityNotFoundException', 'Job Run not found.'). (The same check also asserts the single-run read wording 'Job run not found for the given job name and runId.' for GetJobRun, which is the text wrongly reused here.)

Cause 3: `AssertionError: expected a refusal, the call succeeded: {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', 'content-length': '2', ...}}}` — expected ('InvalidInputException', 'Aggregated size of partition values cannot exceed 1024 bytes'). Request: PartitionInput.Values = ['é'*250, 'b'*523] (1025 aggregate bytes, under 1024 characters); the 1024-byte control ['é'*250, 'b'*522] is accepted, as AWS does.

Cause 4: `Error: Check timed out after 60.0s` — the check never reaches its graded assertion (`code == 'IllegalSessionStateException'`, message `'Illegal Session Status'`).

Cause 5: `AssertionError: expected the third start to exceed MaxConcurrentRuns, got ConcurrentRunsExceededException: 'Concurrent runs exceeded for workflow: <workflow-name>.'` — expected code ConcurrentRunsExceededException with message 'Concurrent runs exceeded for workflow: <workflow-name>' (no trailing period). The workflow name is the request value; the rest is fixed text.

## Steps to Reproduce
Cause 1: CreateDatabase, CreateTable with PartitionKeys, CreatePartition for one value set, then BatchDeletePartition with PartitionsToDelete containing a value set the table never held; read Errors[0].ErrorDetail.
Cause 2: CreateJob (pythonshell, MaxCapacity 0.0625), StartJobRun, wait for RUNNING, then BatchStopJobRun(JobName, JobRunIds=[`jr_<128 hex>` never issued, running id]); read Errors[0].ErrorDetail.
Cause 3: CreateDatabase; CreateTable with PartitionKeys [site, tag]; CreatePartition with Values ['é'*250, 'b'*522] (accepted, control); CreatePartition with Values ['é'*250, 'b'*523] — answered 200 instead of InvalidInputException; GetPartition with the same over-limit values — also not refused; GetPartitions then lists both partitions.
Cause 4: create an IAM role trusting glue.amazonaws.com; CreateSession(Id, Role, Command={'Name':'glueetl','PythonVersion':'3'}, GlueVersion='5.0', Timeout=15, IdleTimeout=15); poll GetSession for Status READY; RunStatement(SessionId, Code='1 + 1'); poll GetStatement for AVAILABLE/ERROR. The sequence does not converge within the check budget.
Cause 5: CreateWorkflow(MaxConcurrentRuns=2) with an ON_DEMAND trigger starting a pythonshell job; StartWorkflowRun twice (both accepted, both RUNNING); StartWorkflowRun a third time and read Error.Code/Error.Message.
