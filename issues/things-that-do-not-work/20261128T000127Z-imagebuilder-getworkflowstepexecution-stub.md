Issue-Source: trace-sweep-divergence
Issue-Trace: imagebuilder-get-workflow-step-execution-refuses-missing-malformed-and-unknown-step-ids
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: imagebuilder-get-workflow-step-execution-refuses-missing-malformed-and-unknown-step-ids
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|stub:GetWorkflowStepExecution
Issue-Commit: 7e7f1782eb72d186b4a02ad6793c94a317471370

# Implement imagebuilder GetWorkflowStepExecution: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `7e7f1782eb72d186b4a02ad6793c94a317471370`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWorkflowStepExecution

## Description
`GetWorkflowStepExecution` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/get_workflow_step_execution.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetWorkflowStepExecution` call, so each one unblocks only when `GetWorkflowStepExecution` works: `imagebuilder-get-workflow-step-execution-refuses-missing-malformed-and-unknown-step-ids#task#1`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/get_workflow_step_execution.rs`. Replaying the corpus, the emulator refused the `GetWorkflowStepExecution` call: status: expected 400, got 501
$.message: value_mismatch (expected "Your request is invalid. Request must contain ['stepExecutionId'] properties. Verify your request and try again.", got 'operation GET /GetWorkflowStepExecution is not implemented in this emulator')
$headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'NotImplemented')

## Steps to Reproduce
Boot the imagebuilder emulator
Call GetWorkflowStepExecution and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
