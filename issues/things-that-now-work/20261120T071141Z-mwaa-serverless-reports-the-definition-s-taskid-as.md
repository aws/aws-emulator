Issue-Source: trace-sweep-divergence
Issue-Trace: mwaa-serverless-workflow-run-two-dependent-tasks-succeed
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: mwaa-serverless-workflow-run-two-dependent-tasks-succeed
Issue-Shape-Id: com.amazonaws.mwaaserverless#AmazonMWAAServerless
Issue-Key: mwaa-serverless|GetWorkflowRun|2xx->2xx|$.RunDetail.TaskInstances[*]
Issue-Key: mwaa-serverless|ListTaskInstances|2xx->2xx|$.TaskInstances[*].TaskInstanceId
Issue-Commit: 833bd49ae97861524104d3818062153b5a89a9b0

# mwaa-serverless reports the definition's TaskId as TaskInstanceId, so a run's task-instance identifiers are never minted per run

> _Note: this issue was identified against the code base at commit `833bd49ae97861524104d3818062153b5a89a9b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowRun

## Description
This issue bundles 2 causes, both of them one root defect surfacing on two readers of the same state: the emulator uses the task identifier declared in the workflow definition AS the `TaskInstanceId`, instead of minting an identifier for the instance the run creates for that task. Both causes must be closed before submitting.

Shared root cause: a run's execution plan sets each planned task's instance identifier to the declared task id verbatim, and the run stores and reports its task instances under that key, so every reader of a task-instance identifier reports a definition-authored task name rather than a run-scoped instance identifier.

Cause 1 of 2: GetWorkflowRun reports `RunDetail.TaskInstances` as the declared task names.
AWS: `GetWorkflowRun` returns `RunDetail` (`WorkflowRunDetail`), whose `TaskInstances` member targets `TaskInstanceIds` and is documented "A list of task instance IDs that are part of this workflow run" (Smithy: `WorkflowRunDetail$TaskInstances` -> `TaskInstanceIds`; element `IdString`, @length 1..255, @pattern `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`). The model keeps the instance identifier and the definition's task id as SEPARATE members: `GetTaskInstanceResponse` carries both `TaskInstanceId` (required) and `TaskId` (Smithy: `GetTaskInstance` output), so the identifier of an instance is not the identifier of the task it executes — AWS mints the instance identifier when the run creates the instance, and the recorded response's values are dynamic (the fixture asserts their shape, not a literal), which a definition-authored task name never is.
Emulator: status 200 with a correctly shaped body, but the identifier VALUES are the declared task names. Replay reports `$.RunDetail.TaskInstances[0]: value_mismatch (expected '<shaped-not-asserted>', got 'first_task')` and `$.RunDetail.TaskInstances[1]: value_mismatch (expected '<shaped-not-asserted>', got 'second_task')` — `first_task`/`second_task` are exactly the task ids the replayed workflow definition declares.
Why: a client that treats `TaskInstanceId` as the per-run handle for an instance is handed a value that is constant across every run of the workflow, so instance identifiers collide between runs; correlating a task instance with its run, keying a store by instance id, or distinguishing a re-run's instance from the previous run's instance all silently fuse separate instances into one. A client that reads `TaskId` and `TaskInstanceId` as distinct values (as the model declares them) sees them as identical and cannot tell the task from its instance.
Fix: the run-execution plan that assigns the instance identifier, and the run state that keys its task instances by it — a run must mint an instance identifier per (run, task, attempt) and keep the declared task id in `TaskId` only; `GetWorkflowRun`'s projection of `RunDetail.TaskInstances` then reports the minted ids. The `GetTaskInstance`/`ListTaskInstances` lookup path and the task-instance listing cursor must address instances by the minted identifier.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#8.

Cause 2 of 2: ListTaskInstances reports `TaskInstances[*].TaskInstanceId` as the declared task names.
AWS: `ListTaskInstances` returns `TaskInstances` (`TaskInstanceSummaries` of `TaskInstanceSummary`), whose `TaskInstanceId` member is documented "The unique identifier of this task instance" (Smithy: `TaskInstanceSummary$TaskInstanceId` -> `IdString`, @length 1..255, @pattern `^[a-zA-Z0-9]+[a-zA-Z0-9\.\-_]*$`). That member is the value `GetTaskInstance` takes as its required `TaskInstanceId` input, and `GetTaskInstanceResponse` reports it alongside a separate `TaskId`, so the summary's identifier is the instance's own minted identifier and not the definition's task name. The recorded values are dynamic (shape-asserted).
Emulator: status 200, correct body shape, diverging values — `$.TaskInstances[0].TaskInstanceId: value_mismatch (expected '<shaped-not-asserted>', got 'first_task')` and `$.TaskInstances[1].TaskInstanceId: value_mismatch (expected '<shaped-not-asserted>', got 'second_task')`.
Why: the listing is the discovery path for instance identifiers, so every identifier a client obtains from it is a definition-authored task name. Pagination makes this worse: the listing's cursor encodes the last `TaskInstanceId` on the page as its position, so the page boundary is expressed in definition-authored names instead of run-scoped identifiers, and a client that caches listing results across runs of the same workflow cannot distinguish them.
Fix: the same component — the run-execution plan's instance-identifier assignment and the run state keyed by it; `ListTaskInstances`' serializer then reports the minted identifier in each `TaskInstanceSummary`, and the task-instance listing-key helper must carry the minted identifier as its cursor position.
Traces: mwaa-serverless-workflow-run-two-dependent-tasks-succeed#task#9.

No status code or error code diverges here: both responses are 200 and no error is involved, so there is no error message to reproduce. The open mwaa-serverless issues (CreateWorkflow definition/role validation, UpdateWorkflow ordering of ValidationException, the CloudFormation resource-type issues, CreateCliToken's WebServerHostname port) are about different operations and do not cover this identifier defect.

## Evidence
Fixture: mwaa-serverless-workflow-run-two-dependent-tasks-succeed (4 fixtures replayed, 34 steps, 2 mismatched, score 94.1%).

Step task#8 — GetWorkflowRun, expected 200, actual 200, verdict mismatch:
  $.RunDetail.TaskInstances[0]: value_mismatch (expected '<shaped-not-asserted>', got 'first_task')
  $.RunDetail.TaskInstances[1]: value_mismatch (expected '<shaped-not-asserted>', got 'second_task')

Step task#9 — ListTaskInstances, expected 200, actual 200, verdict mismatch:
  $.TaskInstances[0].TaskInstanceId: value_mismatch (expected '<shaped-not-asserted>', got 'first_task')
  $.TaskInstances[1].TaskInstanceId: value_mismatch (expected '<shaped-not-asserted>', got 'second_task')

Expected (AWS): a run-scoped, service-minted instance identifier per task instance — a dynamic value the recording asserts by shape; distinct from the declared task id, which the model reports separately as `GetTaskInstanceResponse$TaskId`.
Actual (emulator): the declared task id itself ('first_task', 'second_task'), identical in `TaskInstanceId` and `TaskId` and identical across every run of the workflow.

Source evidence in the checkout:
  services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state/workflow_execution.rs:273 — `task_instance_id: (*task_id).to_string()` in `plan_tasks`, alongside `task_id: (*task_id).to_string()` on the next line, so `PlannedTask` carries one value under both names (see also the member doc at line 80-83: "It is the task's own identifier").
  services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:489 — `pub task_instances: BTreeMap<String, TaskInstance>` keyed by that same value; :557 `task_instance_ids()` is what `GetWorkflowRun` reports as `RunDetail.TaskInstances`; :781 the stored `TaskInstance.task_instance_id` is `task.task_instance_id.clone()`.
  services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/state.rs:2560 — `format_task_instance_listing_key` uses the same value as the listing cursor position.

Model evidence: smithy_query(query="shape", shape="WorkflowRunDetail,TaskInstanceSummary") and smithy_query(query="operation", operation="GetTaskInstance") on services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/smithy-model/com-amazonaws-mwaaserverless-amazonmwaaserverless.json.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay fixture mwaa-serverless-workflow-run-two-dependent-tasks-succeed from trace set simparity-wire-aws-v1.
2. The fixture creates an mwaa-serverless workflow whose definition declares two dependent tasks (task ids `first_task` and `second_task`), starts a run of it, and lets the run settle to SUCCESS.
3. Step task#8 calls GetWorkflowRun for that run and compares `$.RunDetail.TaskInstances`: the emulator answers 200 with the two declared task ids instead of minted, run-scoped task-instance identifiers.
4. Step task#9 calls ListTaskInstances for the same run and compares `$.TaskInstances[*].TaskInstanceId`: again the declared task ids.
5. To see the identifier collision directly, start a SECOND run of the same workflow and list its task instances: both runs report the same `TaskInstanceId` values, and GetTaskInstance reports `TaskInstanceId` equal to `TaskId` for every instance.
