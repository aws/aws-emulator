Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: transfer.agreement-lifecycle-create-update-delete transfer.agreement-update-conflicting-directories-rejected
Issue-Shape-Id: com.amazonaws.transfer#TransferService
Issue-Key: transfer|UpdateAgreement|NotImplemented
Issue-Key: transfer.agreement-update-conflicting-directories-rejected|run#17|AssertionError|assert error == ("InvalidRequestException", expected_message), error
Issue-Commit: 04dc54a1e9195c8cf637d3927b453ed3a21231a5

# Implement transfer UpdateAgreement: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `04dc54a1e9195c8cf637d3927b453ed3a21231a5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateAgreement

## Description
`UpdateAgreement` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-transfer-transferservice/src/ops/update_agreement.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `UpdateAgreement` call, so each one unblocks only when `UpdateAgreement` works: `transfer.agreement-lifecycle-create-update-delete`, `transfer.agreement-update-conflicting-directories-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-transfer-transferservice/src/ops/update_agreement.rs`. Replaying the corpus, the emulator refused the `UpdateAgreement` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the UpdateAgreement operation: operation UpdateAgreement is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the transfer emulator
Call UpdateAgreement and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
