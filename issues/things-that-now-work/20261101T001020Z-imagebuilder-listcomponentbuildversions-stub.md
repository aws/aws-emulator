Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: imagebuilder.create-component-idempotent-same-token-returns-same-arn
Issue-Shape-Id: com.amazonaws.imagebuilder#imagebuilder
Issue-Key: imagebuilder|ListComponentBuildVersions|NotImplemented
Issue-Commit: e3579db6a1835cfcd1e7033eb2fb9f64dc458cca

# Implement imagebuilder ListComponentBuildVersions: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `e3579db6a1835cfcd1e7033eb2fb9f64dc458cca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListComponentBuildVersions

## Description
`ListComponentBuildVersions` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_component_build_versions.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `ListComponentBuildVersions` call, so each one unblocks only when `ListComponentBuildVersions` works: `imagebuilder.create-component-idempotent-same-token-returns-same-arn`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-imagebuilder-imagebuilder/src/ops/list_component_build_versions.rs`. Replaying the corpus, the emulator refused the `ListComponentBuildVersions` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ListComponentBuildVersions operation: operation ListComponentBuildVersions is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the imagebuilder emulator
Call ListComponentBuildVersions and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
