Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datasync.cancel-rejected-once-execution-has-ended datasync.cancel-task-execution-rejects-completed-execution datasync.cancelled-execution-task-report-result-is-error datasync.cancelled-queued-execution-lists-as-error-and-next-run-copies datasync.execution-include-override-applies-to-one-run-only datasync.update-task-execution-rejects-completed-execution
Issue-Shape-Id: com.amazonaws.datasync#FmrsService
Issue-Key: datasync|StartTaskExecution|InvalidRequestException
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# DataSync tasks never leave CREATING, so StartTaskExecution refuses every run and the whole task-execution surface is unreachable

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTaskExecution

## Description
One root cause behind all six failing datasync behavioral checks.

Cause 1 of 1: a task created by CreateTask is stuck in Status=CREATING, so StartTaskExecution refuses it with InvalidRequestException and no execution can ever be started.

AWS: Smithy model services/service-com-amazonaws-datasync-fmrsservice/smithy-model/com-amazonaws-datasync-fmrsservice.json — CreateTask(SourceLocationArn, DestinationLocationArn, ...) -> TaskArn; DescribeTask.Status targets the enum TaskStatus [AVAILABLE, CREATING, QUEUED, RUNNING, UNAVAILABLE]; StartTaskExecution(TaskArn required) -> TaskExecutionArn, errors InvalidRequestException (client, HTTP 400) and InternalException (server, 500). CREATING is a transient state: after CreateTask, DataSync validates the task and it settles on AVAILABLE, which is the state every later DescribeTask/ListTasks read reports and the state a task must be in for StartTaskExecution to be accepted. StartTaskExecution on an AVAILABLE task therefore succeeds (HTTP 200) and returns a TaskExecutionArn matching ^arn:aws:datasync:<region>:<account>:task/task-[0-9a-f]{17}/execution/exec-[0-9a-f]{17}$; the execution then runs through LAUNCHING/PREPARING/TRANSFERRING/VERIFYING to SUCCESS (or ERROR when cancelled). The InvalidRequestException naming a status is the refusal AWS reserves for a task that genuinely is NOT in a startable state (e.g. UNAVAILABLE); it is not the answer for a freshly created, valid S3-to-S3 task.

Emulator: every one of the six checks fails on its first StartTaskExecution call, immediately after creating two S3 locations and a task, with HTTP 400 / error code InvalidRequestException and message `Task arn:aws:datasync:<region>:<account>:task/task-<id> is in status CREATING and cannot start an execution.` In that message the fixed text is `Task `, ` is in status `, ` and cannot start an execution.`; the task ARN and the status word are values taken from the request and from the emulator's stored task state (the ARN and status shown in the replay output are one run's values, not requirements). The task state is assigned CREATING at creation and is never advanced, so the refusal is permanent and no amount of polling clears it — exactly the stuck lifecycle the already-open wire issue 20261029T180920Z-datasync-task-reads-never-settle-to-available-omit.md reports for the READ surface (its Cause 1, DescribeTask/ListTasks reporting Status='CREATING'); this issue is that same unsettled state seen from the execution surface, where it turns into a refusal. Because the refusal happens on the first call of each check, nothing downstream of it was exercised at all: CancelTaskExecution, UpdateTaskExecution, DescribeTaskExecution, task reports and the queued/next-run behaviour the checks grade are all blocked behind it.

Why: AVAILABLE is the readiness signal for the whole service, and starting a run is the point of DataSync. A client that does the documented thing — create locations, create a task, optionally poll until AVAILABLE, then StartTaskExecution — either polls forever or gets a 400 InvalidRequestException telling it the task is still being created, for a task that will never finish being created. Every transfer, cancellation, override and task-report workflow is unreachable, and clients that treat InvalidRequestException as a permanent client-side fault (bad request) will abort rather than retry.

Fix: the task state machine in the datasync service's state layer — a task must leave CREATING and settle on AVAILABLE once CreateTask has accepted it, and the StartTaskExecution handler's status precondition must consult that settled value (refusing only genuinely non-startable statuses) so it creates an execution and returns a TaskExecutionArn for a validated task.

Traces: datasync.cancel-rejected-once-execution-has-ended, datasync.cancel-task-execution-rejects-completed-execution, datasync.cancelled-execution-task-report-result-is-error, datasync.cancelled-queued-execution-lists-as-error-and-next-run-copies, datasync.execution-include-override-applies-to-one-run-only, datasync.update-task-execution-rejects-completed-execution — all six fail on StartTaskExecution with the identical message.

## Evidence
Replay of simparity-behavioral-aws-v1 against the booted emulator: datasync 12/18 passed, 6 failed, all six with verdict=error and the same exception from the first StartTaskExecution call:

  botocore.errorfactory.InvalidRequestException: An error occurred (InvalidRequestException) when calling the StartTaskExecution operation: Task arn:aws:datasync:us-west-2:000000000000:task/task-00000000000000001 is in status CREATING and cannot start an execution.

Expected: HTTP 200 with TaskExecutionArn (pattern ^arn:...:task/task-[0-9a-f]{17}/execution/exec-[0-9a-f]{17}$), because the task created one call earlier is a valid agentless S3-to-S3 task and settles on TaskStatus=AVAILABLE.
Actual: HTTP 400, Error.Code=InvalidRequestException, Error.Message as quoted above, permanently.

Message anatomy — fixed text: 'Task ', ' is in status ', ' and cannot start an execution.'; variable parts: the TaskArn from the request and the status from stored task state. The ARN, region and account in the quoted line are one run's values and are not part of the requirement.

Failing checks: datasync.cancel-rejected-once-execution-has-ended; datasync.cancel-task-execution-rejects-completed-execution; datasync.cancelled-execution-task-report-result-is-error; datasync.cancelled-queued-execution-lists-as-error-and-next-run-copies; datasync.execution-include-override-applies-to-one-run-only; datasync.update-task-execution-rejects-completed-execution.

Model references: TaskStatus enum [AVAILABLE, CREATING, QUEUED, RUNNING, UNAVAILABLE]; StartTaskExecutionRequest.TaskArn @required; StartTaskExecutionResponse.TaskExecutionArn; StartTaskExecution errors {InvalidRequestException 400, InternalException 500}; DescribeTaskExecution.Status targets TaskExecutionStatus.

Related open issue, which does not cover this: 20261029T180920Z-datasync-task-reads-never-settle-to-available-omit.md records the same unsettled CREATING state on the task-READ surface (DescribeTask/ListTasks body mismatches) from the wire corpus; it carries no StartTaskExecution refusal and none of these behavioral check ids.

## Steps to Reproduce
Boot the emulator from this checkout and run the behavioral suite's datasync execution-control checks.

1. Create an S3 source location and an S3 destination location (CreateLocationS3), then CreateTask against them; note the returned TaskArn.
2. DescribeTask on that TaskArn: observe Status='CREATING' and that it never advances to 'AVAILABLE' however long you poll.
3. Call StartTaskExecution(TaskArn=<that ARN>). Observe HTTP 400 with Error.Code='InvalidRequestException' and Error.Message 'Task <TaskArn> is in status CREATING and cannot start an execution.', instead of HTTP 200 with a TaskExecutionArn.
4. Because step 3 never yields an execution ARN, every following call in these checks (DescribeTaskExecution, CancelTaskExecution, UpdateTaskExecution, ListTaskExecutions, task-report reads) is unreachable.

Replay to confirm a fix: datasync.cancel-rejected-once-execution-has-ended, datasync.cancel-task-execution-rejects-completed-execution, datasync.cancelled-execution-task-report-result-is-error, datasync.cancelled-queued-execution-lists-as-error-and-next-run-copies, datasync.execution-include-override-applies-to-one-run-only, datasync.update-task-execution-rejects-completed-execution.
