Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: swf.closed-execution-filters-scope-and-exclusivity swf.failed-execution-leaves-open-list-and-lists-closed-failed
Issue-Shape-Id: com.amazonaws.swf#SimpleWorkflowService
Issue-Key: swf|CountClosedWorkflowExecutions|NotImplemented
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# Implement swf CountClosedWorkflowExecutions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CountClosedWorkflowExecutions

## Description
`CountClosedWorkflowExecutions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/count_closed_workflow_executions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `CountClosedWorkflowExecutions` call, so each one unblocks only when `CountClosedWorkflowExecutions` works: `swf.closed-execution-filters-scope-and-exclusivity`, `swf.failed-execution-leaves-open-list-and-lists-closed-failed`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-swf-simpleworkflowservice/src/ops/count_closed_workflow_executions.rs`. Replaying the corpus, the emulator refused the `CountClosedWorkflowExecutions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CountClosedWorkflowExecutions operation: operation CountClosedWorkflowExecutions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the swf emulator
Call CountClosedWorkflowExecutions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
