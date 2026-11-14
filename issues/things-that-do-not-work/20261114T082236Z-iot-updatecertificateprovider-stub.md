Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot.certificate-provider-lifecycle
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot.certificate-provider-lifecycle|run#17|AssertionError|assert update_code == ("ResourceNotFoundException", f"Certificate provider with name [{name}] not found"), (
Issue-Commit: dafb6a92f3304d60364f819845fa972c02211566

# Implement iot UpdateCertificateProvider: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `dafb6a92f3304d60364f819845fa972c02211566`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateCertificateProvider

## Description
`UpdateCertificateProvider` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-iot-awsiotservice/src/ops/update_certificate_provider.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateCertificateProvider` call, so each one unblocks only when `UpdateCertificateProvider` works: `iot.certificate-provider-lifecycle`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-iot-awsiotservice/src/ops/update_certificate_provider.rs`. Replaying the corpus, the emulator refused the `UpdateCertificateProvider` call: File "run.py", line 445, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_iot_certificate_provider_lifecycle.py", line 73, in run
    assert update_code == ("ResourceNotFoundException", f"Certificate provider with name [{name}] not found"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: expected ResourceNotFoundException on update of deleted provider, got ('NotImplemented', 'operation UpdateCertificateProvider is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the iot emulator
Call UpdateCertificateProvider and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
