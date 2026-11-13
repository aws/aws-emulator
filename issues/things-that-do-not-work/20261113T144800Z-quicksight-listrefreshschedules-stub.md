Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: quicksight.refresh-schedule-quota-exceeded-rejected
Issue-Shape-Id: com.amazonaws.quicksight#QuickSight_20180401
Issue-Key: quicksight|ListRefreshSchedules|NotImplemented
Issue-Commit: 92e6b2d8a55271b9e46b89d4e3342f522bc16f2a

# Implement quicksight ListRefreshSchedules: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `92e6b2d8a55271b9e46b89d4e3342f522bc16f2a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListRefreshSchedules

## Description
`ListRefreshSchedules` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/list_refresh_schedules.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListRefreshSchedules` call, so each one unblocks only when `ListRefreshSchedules` works: `quicksight.refresh-schedule-quota-exceeded-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-quicksight-quicksight-20180401/src/ops/list_refresh_schedules.rs`. Replaying the corpus, the emulator refused the `ListRefreshSchedules` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListRefreshSchedules operation: operation ListRefreshSchedules is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the quicksight emulator
Call ListRefreshSchedules and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
