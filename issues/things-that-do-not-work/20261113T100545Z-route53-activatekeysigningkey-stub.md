Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: route53.dnssec-enable-without-active-ksk-rejected route53.e2e-dnssec-enable-verify-disable-teardown route53.e2e-dnssec-sign-activate-resolve-and-teardown route53.e2e-dnssec-signing-full-flow route53.ksk-activate-already-active-is-idempotent route53.ksk-concurrent-activate-race route53.ksk-lifecycle-create-activate-deactivate-delete
Issue-Shape-Id: com.amazonaws.route53#AWSDnsV20130401
Issue-Key: route-53|ActivateKeySigningKey|NotImplemented
Issue-Commit: a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c

# Implement route53 ActivateKeySigningKey: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ActivateKeySigningKey

## Description
`ActivateKeySigningKey` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/activate_key_signing_key.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

7 recorded unit(s) fail on their `ActivateKeySigningKey` call, so each one unblocks only when `ActivateKeySigningKey` works: `route53.dnssec-enable-without-active-ksk-rejected`, `route53.e2e-dnssec-enable-verify-disable-teardown`, `route53.e2e-dnssec-sign-activate-resolve-and-teardown`, `route53.e2e-dnssec-signing-full-flow`, `route53.ksk-activate-already-active-is-idempotent`, `route53.ksk-concurrent-activate-race`, `route53.ksk-lifecycle-create-activate-deactivate-delete`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-route53-awsdnsv20130401/src/ops/activate_key_signing_key.rs`. Replaying the corpus, the emulator refused the `ActivateKeySigningKey` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the ActivateKeySigningKey operation: operation ActivateKeySigningKey is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the route53 emulator
Call ActivateKeySigningKey and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
