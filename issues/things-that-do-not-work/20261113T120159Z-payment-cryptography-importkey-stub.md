Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: payment-cryptography.export-import-tr31-key-roundtrip payment-cryptography.export-key-not-exportable-rejected payment-cryptography.import-key-corrupted-wrapped-block-rejected
Issue-Shape-Id: com.amazonaws.paymentcryptography#PaymentCryptographyControlPlane
Issue-Key: payment-cryptography|ImportKey|NotImplemented
Issue-Key: payment-cryptography.import-key-corrupted-wrapped-block-rejected|run#14|AssertionError|assert error.get("Code") == "ValidationException", (
Issue-Commit: f9cbb69e245527c0f1af31f0e23f7bc2deb4532a

# Implement payment-cryptography ImportKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f9cbb69e245527c0f1af31f0e23f7bc2deb4532a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ImportKey

## Description
`ImportKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/src/ops/import_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `ImportKey` call, so each one unblocks only when `ImportKey` works: `payment-cryptography.export-import-tr31-key-roundtrip`, `payment-cryptography.export-key-not-exportable-rejected`, `payment-cryptography.import-key-corrupted-wrapped-block-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-paymentcryptography-paymentcryptographycontrolplane/src/ops/import_key.rs`. Replaying the corpus, the emulator refused the `ImportKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ImportKey operation: operation ImportKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the payment-cryptography emulator
Call ImportKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
