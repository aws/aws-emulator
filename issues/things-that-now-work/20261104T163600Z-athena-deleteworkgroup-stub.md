Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena.delete-workgroup-requires-recursive-option-rejected athena.delete-workgroup-with-named-queries-rejected athena.workgroup-delete-without-recursive-fails-when-named-query-exists
Issue-Shape-Id: com.amazonaws.athena#AmazonAthena
Issue-Key: athena|DeleteWorkGroup|NotImplemented
Issue-Commit: 1b83187822ca52f98e00143d7b0b9c4d0812f404

# Implement athena DeleteWorkGroup: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1b83187822ca52f98e00143d7b0b9c4d0812f404`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteWorkGroup

## Description
`DeleteWorkGroup` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-athena-amazonathena/src/ops/delete_work_group.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `DeleteWorkGroup` call, so each one unblocks only when `DeleteWorkGroup` works: `athena.delete-workgroup-requires-recursive-option-rejected`, `athena.delete-workgroup-with-named-queries-rejected`, `athena.workgroup-delete-without-recursive-fails-when-named-query-exists`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-athena-amazonathena/src/ops/delete_work_group.rs`. Replaying the corpus, the emulator refused the `DeleteWorkGroup` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DeleteWorkGroup operation: operation DeleteWorkGroup is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the athena emulator
Call DeleteWorkGroup and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
