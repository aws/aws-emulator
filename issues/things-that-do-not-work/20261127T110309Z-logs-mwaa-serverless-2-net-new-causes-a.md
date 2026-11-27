Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__mwaa-serverless.create-workflow-log-group-name-outside-logs-grammar-rejected logs__mwaa-serverless.update-workflow-moves-task-logs-to-new-log-group logs__mwaa-serverless.workflow-task-output-reaches-configured-log-group
Issue-Key: logs__mwaa-serverless.create-workflow-log-group-name-outside-logs-grammar-rejected|run#20|AssertionError|assert msg == "Invalid logGroupName or KmsKeyId", f"refusal message {msg!r}"
Issue-Key: logs__mwaa-serverless.update-workflow-moves-task-logs-to-new-log-group|_until_finished#23|AssertionError|assert time.monotonic() < deadline, f"run still {detail['RunState']!r} at its deadline"
Issue-Key: logs__mwaa-serverless.workflow-task-output-reaches-configured-log-group|_wait_run#22|AssertionError|assert time.monotonic() < deadline, f"run still {detail['RunState']!r} at the deadline"
Issue-Commit: 558d2bea4a2bc55ff66bbf3c15524a570b71a39f

# logs__mwaa-serverless: 2 net-new causes — a started workflow run never leaves STARTING (no run ever becomes terminal, so no task output reaches CloudWatch Logs), and CreateWorkflow's bad-LogGroupName refusal carries a generic Smithy-constraint message instead of "Invalid logGroupName or KmsKeyId"

> _Note: this issue was identified against the code base at commit `558d2bea4a2bc55ff66bbf3c15524a570b71a39f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartWorkflowRun

## Description
Two distinct root causes in the mwaa-serverless workflow surface, both observed by the cross-service logs__mwaa-serverless checks. The single-service mwaa-serverless checks all pass (23/23), so these are specific to the run-lifecycle and the LoggingConfiguration validation path exercised here.

=== Cause 1 of 2: a workflow run started by StartWorkflowRun never leaves STARTING ===
Failing checks: logs__mwaa-serverless.update-workflow-moves-task-logs-to-new-log-group, logs__mwaa-serverless.workflow-task-output-reaches-configured-log-group

AWS: Smithy: StartWorkflowRun returns RunId (IdString) + Status (WorkflowRunStatus); GetWorkflowRun returns RunDetail (WorkflowRunDetail) whose RunState is a WorkflowRunStatus enum with values STARTING, QUEUED, RUNNING, SUCCESS, FAILED, TIMEOUT, STOPPING, STOPPED. WorkflowRunDetail documents CompletedOn/Duration as "null if the run is not complete" and TaskInstances as the task instance IDs of the run — i.e. a run is modelled to progress out of STARTING and reach a terminal state, and real AWS does so: on AWS the one-BashOperator definitions these checks deploy advance STARTING -> RUNNING -> SUCCESS within the checks' polling window, populate TaskInstances, and the task's stdout is delivered to the log group named by LoggingConfiguration.LogGroupName (GetTaskInstance.LogStream names the stream).

Emulator: GetWorkflowRun keeps answering RunDetail.RunState == "STARTING" indefinitely. Both checks poll GetWorkflowRun every 2 s until RunState is in {SUCCESS, FAILED, TIMEOUT, STOPPED} and are still reading 'STARTING' when their (minutes-long) deadline expires: "AssertionError: run still 'STARTING' at its deadline" and "AssertionError: run still 'STARTING' at the deadline". The run is therefore never terminal, TaskInstances is never populated, GetTaskInstance has no stream, and nothing is ever delivered to the configured CloudWatch Logs group — so every downstream assertion of these two checks (log delivery to the group named by LoggingConfiguration, one task instance per one-task definition, the stream identity, and the "old group stays empty after UpdateWorkflow repoints LoggingConfiguration" negative control) is unreachable.

Note this is not simply the already-fixed "run settles to FAILED" family (20261024T002400Z-mwaa-serverless-startworkflowrun-wrong-behavior.md, 20261024T132758Z-mwaa-serverless-getworkflowrun-wrong-behavior.md): the run no longer settles at all, it is stuck in the modelled initial state. Not filed as a REOPENS because the calls succeed (2xx) — nothing is refused; the lifecycle simply never advances.

Why: a client that starts a run and waits for a terminal RunState waits forever. Orchestration code (poll-until-done, then read task logs) hangs rather than completing or erroring, and any workflow whose observable effect is its task's log output is completely unobservable. The MWAA Serverless data plane cannot be exercised at all against the emulator.

Fix: the workflow-run lifecycle advance in the mwaa-serverless service crate — whatever moves a run out of its initial state and runs its task instances — plus the RunDetail projection that GetWorkflowRun reads. It must leave STARTING, execute the definition's task instances (populating TaskInstances and their LogStream), reach a terminal WorkflowRunStatus, and deliver the task's stdout to LoggingConfiguration.LogGroupName (the CURRENT one after UpdateWorkflow, never the replaced one).

=== Cause 2 of 2: CreateWorkflow's refusal of a LogGroupName outside the CloudWatch Logs grammar uses a generic Smithy-constraint message ===
Failing check: logs__mwaa-serverless.create-workflow-log-group-name-outside-logs-grammar-rejected

AWS: recorded behavior only for the message text — Smithy: CreateWorkflow.LoggingConfiguration is a LoggingConfiguration whose required LogGroupName targets a plain String with NO @length and NO @pattern, and CreateWorkflow models ValidationException (client, HTTP 400, members Message/Reason/FieldList). The recording shows AWS refusing a LogGroupName containing a colon (a name CloudWatch Logs itself refuses, its logGroupName pattern being [\.\-_/#A-Za-z0-9]+) with HTTP 400, error code ValidationException and the message, in full and fixed: "Invalid logGroupName or KmsKeyId". That message has no variable parts — it names neither the offending value nor the member — so it is required verbatim.

Emulator: the status and code are right (ValidationException), but the message is an emulator-authored Smithy-constraint rendering: "1 validation error detected: Value at 'LoggingConfiguration.LogGroupName' failed to satisfy constraint: Member must be 1 to 512 characters of letters, numbers and `. - _ / #`". Its fixed text is the "1 validation error detected: Value at '<member path>' failed to satisfy constraint: Member must ..." envelope and the member path 'LoggingConfiguration.LogGroupName' comes from the request; none of it is AWS's wording here, and the constraint it cites (1..512, that character class) is not on the modelled member at all.

Why: a client that branches on the refusal message — or a test/assertion that pins the documented wording — sees a different refusal than AWS gives, and a client parsing the generic envelope for a FieldList-style member path gets one AWS does not emit for this error. The service-specific logging-configuration refusal is indistinguishable from an ordinary member-constraint failure.

Fix: CreateWorkflow's LoggingConfiguration validation in the mwaa-serverless service crate (services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs): the log-group-name check must raise ValidationException with Message exactly "Invalid logGroupName or KmsKeyId" rather than routing through the generic member-constraint message builder. Apply the same wording to the UpdateWorkflow path, which takes the same LoggingConfiguration.

=== Traces ===
logs__mwaa-serverless.update-workflow-moves-task-logs-to-new-log-group (Cause 1), logs__mwaa-serverless.workflow-task-output-reaches-configured-log-group (Cause 1), logs__mwaa-serverless.create-workflow-log-group-name-outside-logs-grammar-rejected (Cause 2). Components: services/service-com-amazonaws-mwaaserverless-amazonmwaaserverless/src/ops/create_workflow.rs, src/ops/start_workflow_run.rs, src/ops/get_workflow_run.rs, src/state.rs (3943 lines, the run-lifecycle state).

## Evidence
Cause 1 — expected: GetWorkflowRun(WorkflowArn, RunId).RunDetail.RunState leaves STARTING and reaches a terminal WorkflowRunStatus (SUCCESS for these one-BashOperator definitions), with TaskInstances holding exactly one IdString and the task's echoed line delivered to the configured log group's stream.
actual (replay):
  check logs__mwaa-serverless.update-workflow-moves-task-logs-to-new-log-group
    File "check_logs__mwaa_serverless_update_workflow_moves_task_logs_to_new_log_group.py", line 130, in _until_finished
      assert time.monotonic() < deadline, f"run still {detail['RunState']!r} at its deadline"
    AssertionError: run still 'STARTING' at its deadline
  check logs__mwaa-serverless.workflow-task-output-reaches-configured-log-group
    File "check_logs__mwaa_serverless_workflow_task_output_reaches_configured_log_group.py", line 178, in _wait_run
      assert time.monotonic() < deadline, f"run still {detail['RunState']!r} at the deadline"
    AssertionError: run still 'STARTING' at the deadline
The polling loops read GetWorkflowRun every 2 s against {SUCCESS, FAILED, TIMEOUT, STOPPED} until a deadline of min(900 s, budget-150 s); 'STARTING' is the modelled initial WorkflowRunStatus, so the run never advances at all.

Cause 2 — expected: CreateWorkflow(LoggingConfiguration={LogGroupName: "<stem>:out"}) -> HTTP 400, Error.Code "ValidationException", Error.Message "Invalid logGroupName or KmsKeyId" (the control in the same check confirms CreateLogGroup refuses the same name with InvalidParameterException and its own pattern message, and that the dash-spelled neighbour "<stem>-out" is accepted by both services).
actual: HTTP 400, Error.Code "ValidationException" (correct), Error.Message "1 validation error detected: Value at 'LoggingConfiguration.LogGroupName' failed to satisfy constraint: Member must be 1 to 512 characters of letters, numbers and `. - _ / #`"
  File "check_logs__mwaa_serverless_create_workflow_log_group_name_outside_logs_grammar_rejected.py", line 53, in run
    assert msg == "Invalid logGroupName or KmsKeyId", f"refusal message {msg!r}"
  AssertionError: refusal message "1 validation error detected: Value at 'LoggingConfiguration.LogGroupName' failed to satisfy constraint: Member must be 1 to 512 characters of letters, numbers and `. - _ / #`"
Model evidence: LoggingConfiguration.LogGroupName targets String with no @length/@pattern, so the 1..512 constraint the message cites is emulator-authored.

## Steps to Reproduce
Cause 2 (CreateWorkflow refusal wording):
1. logs.CreateLogGroup(logGroupName="<stem>-out") — accepted.
2. Put a one-task BashOperator YAML definition in an S3 bucket and create an execution role for it.
3. mwaa-serverless.CreateWorkflow(Name=<name>, DefinitionS3Location={Bucket,ObjectKey}, RoleArn=<role>, LoggingConfiguration={"LogGroupName": "<stem>:out"}) — a name containing a colon, which CloudWatch Logs also refuses.
4. Observe the ClientError: code is ValidationException (correct) but Message is the generic "1 validation error detected: Value at 'LoggingConfiguration.LogGroupName' failed to satisfy constraint: ..." envelope instead of AWS's "Invalid logGroupName or KmsKeyId".

Cause 1 (run never leaves STARTING):
1. logs.CreateLogGroup for two groups; put two one-task BashOperator definitions (v1.yaml echoing anything, v2.yaml echoing a unique token) in an S3 bucket; create the execution role.
2. mwaa-serverless.CreateWorkflow(..., LoggingConfiguration={"LogGroupName": first}).
3. mwaa-serverless.UpdateWorkflow(WorkflowArn, DefinitionS3Location=v2.yaml, RoleArn, LoggingConfiguration={"LogGroupName": second}); GetWorkflow confirms the second group.
4. mwaa-serverless.StartWorkflowRun(WorkflowArn) -> RunId.
5. Poll GetWorkflowRun(WorkflowArn, RunId) every 2 s: RunDetail.RunState stays "STARTING" for the whole window (minutes) and never reaches SUCCESS/FAILED/TIMEOUT/STOPPED; RunDetail.TaskInstances stays empty and no event ever appears in either log group (FilterLogEvents on the configured group returns nothing).
6. The same stall reproduces without any UpdateWorkflow: create a workflow with one log group, StartWorkflowRun, poll GetWorkflowRun.
