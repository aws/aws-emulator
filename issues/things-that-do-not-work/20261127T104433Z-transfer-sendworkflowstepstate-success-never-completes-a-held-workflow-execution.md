Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.list-executions-pages-held-uploads-and-drops-each-once-released
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer.list-executions-pages-held-uploads-and-drops-each-once-released|_wait_for#68|AssertionError|raise AssertionError(f"gave up after {budget_s:.0f}s waiting for {what}; last {value!r}")
Issue-Commit: 7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08

# SendWorkflowStepState(SUCCESS) never completes a held workflow execution: ListExecutions keeps reporting it IN_PROGRESS forever

> _Note: this issue was identified against the code base at commit `7f2f9e8fbacb5ccb6dc94ed688d25f78f8760a08`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendWorkflowStepState

## Description
Cause 1 of 1: reporting SUCCESS for a held CUSTOM workflow step does not advance the execution's lifecycle, so the execution never leaves ListExecutions.

AWS: a CUSTOM step whose Lambda target returns without reporting leaves the execution in progress until the caller calls SendWorkflowStepState. Smithy: SendWorkflowStepState takes WorkflowId, ExecutionId, Token and Status (CustomStepStatus enum = SUCCESS|FAILURE), all required, and returns an empty output. ExecutionStatus is IN_PROGRESS|COMPLETED|EXCEPTION|HANDLING_EXCEPTION. Recorded behavior: after SendWorkflowStepState(Status='SUCCESS') for the workflow's only (and final) step, that execution resumes, reaches a terminal state and is no longer returned by ListExecutions(WorkflowId=...) — releasing one of two held executions leaves exactly the other listed, and once both are released ListExecutions(MaxResults=1) answers Executions=[] with no NextToken. Status reported by ListExecutions/DescribeExecution is IN_PROGRESS only while the step is actually held.

Emulator: SendWorkflowStepState(WorkflowId, ExecutionId=<first held>, Token=<token the custom step was handed>, Status='SUCCESS') returns without error, but the execution's state is unchanged: ListExecutions(WorkflowId=...) keeps returning BOTH execution ids for the full 18 s poll budget (last value {'00000007-0000-4000-8000-000000000007', '0000000a-0000-4000-8000-00000000000a'}), i.e. the released execution is still listed, still IN_PROGRESS. Everything up to the release is served correctly by the emulator: the custom step is invoked and hands out executionId/token, both executions are listed, MaxResults=1 paging lists each exactly once with InitialFileLocation/ServiceMetadata, and DescribeExecution reports IN_PROGRESS with the OnUpload ExecutionRole. Only the callback-driven state transition is missing — the token is accepted and then dropped.

Why: a client that uploads a file, waits for its callback token, reports SUCCESS and then polls ListExecutions/DescribeExecution for completion never sees the execution finish against the emulator. Any workflow orchestration that blocks on "no execution in progress" hangs until its own timeout, FAILURE handling (exception steps / HANDLING_EXCEPTION) can never be observed either, and a pending-work gauge built on ListExecutions grows without bound because released executions are never dropped.

Fix: SendWorkflowStepState's handler and the workflow-execution state in the Transfer service — validate the (ExecutionId, Token) pair, then advance the held execution: on SUCCESS run the remaining steps and mark the execution terminal (COMPLETED when the released step was the last one), on FAILURE enter the exception path; and make ListExecutions project only non-terminal executions so a completed one stops being listed and the empty listing carries no NextToken.

Traces: transfer.list-executions-pages-held-uploads-and-drops-each-once-released.

## Evidence
Check transfer.list-executions-pages-held-uploads-and-drops-each-once-released (fails at line 545, the post-release wait).

Setup served correctly by the emulator: workflow with one CUSTOM step (Lambda target that records executionId + token and returns), server with WorkflowDetails.OnUpload, two files delivered, both executions held and listed IN_PROGRESS, MaxResults=1 paging lists each exactly once, DescribeExecution(first) = IN_PROGRESS with the OnUpload ExecutionRole.

Call: SendWorkflowStepState(WorkflowId=<w-...>, ExecutionId=<first held id>, Token=<token handed to the custom step>, Status='SUCCESS')  -> returns, no error.

Expected (AWS, recorded): ListExecutions(WorkflowId) stops listing the released execution and holds only the unreleased one; after the second release ListExecutions(WorkflowId, MaxResults=1) answers {'Executions': []} with no NextToken.

Actual (emulator): ListExecutions(WorkflowId) keeps returning both ids for the whole budget ->
  AssertionError: gave up after 18s waiting for the released execution to leave the listing; last {'00000007-0000-4000-8000-000000000007', '0000000a-0000-4000-8000-00000000000a'}
(the two uuid values are this run's generated execution ids; the requirement is that the released one is dropped and the other stays)

## Steps to Reproduce
1. CreateWorkflow with a single CUSTOM step whose Lambda target writes out event['serviceMetadata']['executionDetails']['executionId'] and event['token'] and returns without reporting.
2. CreateServer (SFTP, SERVICE_MANAGED) with WorkflowDetails.OnUpload naming that workflow + an execution role, CreateUser, and deliver two files so two executions start and are held by the custom step.
3. ListExecutions(WorkflowId=<w-...>): both executions are listed with Status IN_PROGRESS (this works).
4. Call SendWorkflowStepState(WorkflowId, ExecutionId=<first>, Token=<its recorded token>, Status='SUCCESS').
5. Poll ListExecutions(WorkflowId=<w-...>): the first execution is still listed IN_PROGRESS indefinitely. AWS drops it and leaves only the second.
6. Release the second the same way and call ListExecutions(WorkflowId, MaxResults=1): AWS answers Executions=[] with no NextToken; the emulator still lists both.
