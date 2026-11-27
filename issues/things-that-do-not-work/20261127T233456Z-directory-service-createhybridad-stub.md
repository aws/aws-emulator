Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ds__secretsmanager.create-hybrid-ad-partial-secret-arn-rejected ds__secretsmanager.create-hybrid-ad-reads-only-the-awscurrent-secret-version ds__secretsmanager.create-hybrid-ad-secret-missing-password-key-rejected ds__secretsmanager.create-hybrid-ad-secret-pending-deletion-rejected-until-restored ds__secretsmanager.create-hybrid-ad-secret-with-wrong-key-names-rejected
Issue-Key: ds__secretsmanager.create-hybrid-ad-partial-secret-arn-rejected|run#16|AssertionError|assert (code, message) == (
Issue-Key: ds__secretsmanager.create-hybrid-ad-reads-only-the-awscurrent-secret-version|run#20|AssertionError|assert (code, message) == ("ClientException", mismatch), (
Issue-Key: ds__secretsmanager.create-hybrid-ad-secret-missing-password-key-rejected|run#9|AssertionError|assert (code, message) == (
Issue-Key: ds__secretsmanager.create-hybrid-ad-secret-pending-deletion-rejected-until-restored|run#9|AssertionError|assert (code, message) == _ASSESSMENT_MISSING, (
Issue-Key: ds__secretsmanager.create-hybrid-ad-secret-with-wrong-key-names-rejected|run#11|AssertionError|assert (code, message) == ("ClientException", expected), (
Issue-Commit: c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6

# Implement directory-service CreateHybridAD: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateHybridAD

## Description
`CreateHybridAD` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_hybrid_ad.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

5 recorded unit(s) fail on their `CreateHybridAD` call, so each one unblocks only when `CreateHybridAD` works: `ds__secretsmanager.create-hybrid-ad-partial-secret-arn-rejected`, `ds__secretsmanager.create-hybrid-ad-reads-only-the-awscurrent-secret-version`, `ds__secretsmanager.create-hybrid-ad-secret-missing-password-key-rejected`, `ds__secretsmanager.create-hybrid-ad-secret-pending-deletion-rejected-until-restored`, `ds__secretsmanager.create-hybrid-ad-secret-with-wrong-key-names-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-directoryservice-directoryservice-20150416/src/ops/create_hybrid_ad.rs`. Replaying the corpus, the emulator refused the `CreateHybridAD` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_ds__secretsmanager_create_hybrid_ad_partial_secret_arn_rejected.py", line 77, in run
    assert (code, message) == (
           ^^^^^^^^^^^^^^^^^^^^
AssertionError: the full ARN of the same secret gave ('NotImplemented', 'operation CreateHybridAD is scaffolded but not implemented in this emulator'), not the assessment lookup

## Steps to Reproduce
Boot the directory-service emulator
Call CreateHybridAD and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
