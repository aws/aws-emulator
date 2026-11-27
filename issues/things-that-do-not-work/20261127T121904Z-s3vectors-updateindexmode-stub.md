Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3vectors.update-index-mode-classic-rejected-in-new-bucket
Issue-Shape-Id: com.amazonaws.s3vectors#S3Vectors
Issue-Key: s3vectors|UpdateIndexMode|NotImplemented
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# Implement s3vectors UpdateIndexMode: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateIndexMode

## Description
`UpdateIndexMode` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/update_index_mode.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateIndexMode` call, so each one unblocks only when `UpdateIndexMode` works: `s3vectors.update-index-mode-classic-rejected-in-new-bucket`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-s3vectors-s3vectors/src/ops/update_index_mode.rs`. Replaying the corpus, the emulator refused the `UpdateIndexMode` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateIndexMode operation: operation UpdateIndexMode is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the s3vectors emulator
Call UpdateIndexMode and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
