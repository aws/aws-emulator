Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.stop-workflow-run-rejected-once-the-run-has-completed glue.stop-workflow-run-rejected-once-the-run-has-stopped glue.stop-workflow-run-terminates-the-running-job-and-never-starts-its-successor glue.workflow-run-stop-halts-live-run-and-rejects-second-stop
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.stop-workflow-run-rejected-once-the-run-has-completed|run#16|AssertionError|assert (code, message) == ("IllegalWorkflowStateException", "Cannot stop an already completed Workflow run"), (
Issue-Key: glue|StopWorkflowRun|NotImplemented
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# Implement glue StopWorkflowRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StopWorkflowRun

## Description
`StopWorkflowRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-glue-awsglue/src/ops/stop_workflow_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `StopWorkflowRun` call, so each one unblocks only when `StopWorkflowRun` works: `glue.stop-workflow-run-rejected-once-the-run-has-completed`, `glue.stop-workflow-run-rejected-once-the-run-has-stopped`, `glue.stop-workflow-run-terminates-the-running-job-and-never-starts-its-successor`, `glue.workflow-run-stop-halts-live-run-and-rejects-second-stop`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-glue-awsglue/src/ops/stop_workflow_run.rs`. Replaying the corpus, the emulator refused the `StopWorkflowRun` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_glue_stop_workflow_run_rejected_once_the_run_has_completed.py", line 152, in run
    assert (code, message) == ("IllegalWorkflowStateException", "Cannot stop an already completed Workflow run"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected stopping a completed run to be refused as an illegal state, got NotImplemented: 'operation StopWorkflowRun is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the glue emulator
Call StopWorkflowRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
