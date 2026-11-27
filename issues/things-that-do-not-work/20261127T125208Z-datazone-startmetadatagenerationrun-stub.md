Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: datazone.cancel-metadata-generation-run-unknown-run-rejected datazone.metadata-generation-cancel-after-success-rejected datazone.metadata-generation-run-missing-asset-rejected datazone.metadata-generation-run-repeat-cancel-idempotent datazone.metadata-generation-run-unsupported-asset-type-rejected datazone.metadata-generation-runs-cancel-to-canceled
Issue-Shape-Id: com.amazonaws.datazone#DataZone
Issue-Key: datazone|StartMetadataGenerationRun|NotImplemented
Issue-Key: datazone.metadata-generation-run-missing-asset-rejected|run#10|AssertionError|assert code == "ResourceNotFoundException", f"missing asset answered {code!r}: {message!r}"
Issue-Key: datazone.metadata-generation-run-unsupported-asset-type-rejected|run#10|AssertionError|assert code == "ValidationException", f"unsupported asset type answered {code!r}: {message!r}"
Issue-Commit: cd3ed932ac942c4bb826a6f713c43cf93575b0d9

# Implement datazone StartMetadataGenerationRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `cd3ed932ac942c4bb826a6f713c43cf93575b0d9`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartMetadataGenerationRun

## Description
`StartMetadataGenerationRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-datazone-datazone/src/ops/start_metadata_generation_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `StartMetadataGenerationRun` call, so each one unblocks only when `StartMetadataGenerationRun` works: `datazone.cancel-metadata-generation-run-unknown-run-rejected`, `datazone.metadata-generation-cancel-after-success-rejected`, `datazone.metadata-generation-run-missing-asset-rejected`, `datazone.metadata-generation-run-repeat-cancel-idempotent`, `datazone.metadata-generation-run-unsupported-asset-type-rejected`, `datazone.metadata-generation-runs-cancel-to-canceled`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-datazone-datazone/src/ops/start_metadata_generation_run.rs`. Replaying the corpus, the emulator refused the `StartMetadataGenerationRun` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartMetadataGenerationRun operation: operation StartMetadataGenerationRun is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the datazone emulator
Call StartMetadataGenerationRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
