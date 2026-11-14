Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds.disable-sso-without-enable-is-an-idempotent-no-op
Issue-Shape-Id: com.amazonaws.directoryservice#DirectoryService_20150416
Issue-Key: directory-service|DisableSso|NotImplemented
Issue-Commit: 855e46d3b98757a511fcacb47a7c9e35083e2051

# Implement ds DisableSso: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `855e46d3b98757a511fcacb47a7c9e35083e2051`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DisableSso

## Description
`DisableSso` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/disable_sso.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `DisableSso` call, so each one unblocks only when `DisableSso` works: `ds.disable-sso-without-enable-is-an-idempotent-no-op`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/disable_sso.rs`. Replaying the corpus, the emulator refused the `DisableSso` call: File "run.py", line 342, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ds_disable_sso_without_enable_is_an_idempotent_no_op.py", line 36, in run
    raise AssertionError(
AssertionError: expected DisableSso on a never-enabled directory to succeed as a no-op on real AWS, got NotImplemented

## Steps to Reproduce
Boot the ds emulator
Call DisableSso and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
