Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanroomsml.ml-configuration-put-replaces-and-delete-clears
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml.ml-configuration-put-replaces-and-delete-clears|run#22|AssertionError|assert before == missing, f"a membership with no ML configuration answered {before!r}"
Issue-Commit: 2a4a70a5faf0988854d86ea4e60d7f7575e84c25

# Implement cleanroomsml GetMLConfiguration: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2a4a70a5faf0988854d86ea4e60d7f7575e84c25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMLConfiguration

## Description
`GetMLConfiguration` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice/src/ops/get_ml_configuration.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `GetMLConfiguration` call, so each one unblocks only when `GetMLConfiguration` works: `cleanroomsml.ml-configuration-put-replaces-and-delete-clears`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice/src/ops/get_ml_configuration.rs`. Replaying the corpus, the emulator refused the `GetMLConfiguration` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cleanroomsml_ml_configuration_put_replaces_and_delete_clears.py", line 140, in run
    assert before == missing, f"a membership with no ML configuration answered {before!r}"
           ^^^^^^^^^^^^^^^^^
AssertionError: a membership with no ML configuration answered ('NotImplemented', 'operation GetMLConfiguration is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cleanroomsml emulator
Call GetMLConfiguration and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
