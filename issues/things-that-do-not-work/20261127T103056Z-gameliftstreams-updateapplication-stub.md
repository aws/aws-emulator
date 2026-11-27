Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.list-applications-reflects-create-update-and-delete gameliftstreams.update-application-log-bucket-without-write-policy-rejected gameliftstreams.update-application-log-paths-without-output-uri-rejected gameliftstreams.update-application-log-settings-disable-round-trip
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams|UpdateApplication|NotImplemented
Issue-Key: gameliftstreams.update-application-log-bucket-without-write-policy-rejected|_sans_internal_id#15|AssertionError|assert sep and end.endswith(")"), f"expected an internal id ending on {message!r}"
Issue-Commit: 51355cfc1cfbb5e6f009c7f9575dfb9e65707db6

# Implement gameliftstreams UpdateApplication: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `51355cfc1cfbb5e6f009c7f9575dfb9e65707db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateApplication

## Description
`UpdateApplication` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/update_application.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

4 recorded unit(s) fail on their `UpdateApplication` call, so each one unblocks only when `UpdateApplication` works: `gameliftstreams.list-applications-reflects-create-update-and-delete`, `gameliftstreams.update-application-log-bucket-without-write-policy-rejected`, `gameliftstreams.update-application-log-paths-without-output-uri-rejected`, `gameliftstreams.update-application-log-settings-disable-round-trip`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-gameliftstreams-gameliftstreams/src/ops/update_application.rs`. Replaying the corpus, the emulator refused the `UpdateApplication` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateApplication operation: operation UpdateApplication is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the gameliftstreams emulator
Call UpdateApplication and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
