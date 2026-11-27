Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: drs.start-execution-on-deleted-recovery-plan-rejected drs.start-recovery-plan-execution-on-deleted-plan-returns-not-found drs.start-recovery-plan-execution-rejects-plan-with-no-steps
Issue-Shape-Id: com.amazonaws.drs#ElasticDisasterRecoveryService
Issue-Key: drs.start-execution-on-deleted-recovery-plan-rejected|run#8|AssertionError|assert live == ("ValidationException", _NO_STEPS.format(plan_id)), (
Issue-Key: drs.start-recovery-plan-execution-on-deleted-plan-returns-not-found|run#9|AssertionError|code == "ValidationException"
Issue-Key: drs.start-recovery-plan-execution-rejects-plan-with-no-steps|run#9|AssertionError|code == "ValidationException"
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# Implement drs StartRecoveryPlanExecution: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRecoveryPlanExecution

## Description
`StartRecoveryPlanExecution` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/start_recovery_plan_execution.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `StartRecoveryPlanExecution` call, so each one unblocks only when `StartRecoveryPlanExecution` works: `drs.start-execution-on-deleted-recovery-plan-rejected`, `drs.start-recovery-plan-execution-on-deleted-plan-returns-not-found`, `drs.start-recovery-plan-execution-rejects-plan-with-no-steps`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-drs-elasticdisasterrecoveryservice/src/ops/start_recovery_plan_execution.rs`. Replaying the corpus, the emulator refused the `StartRecoveryPlanExecution` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_drs_start_execution_on_deleted_recovery_plan_rejected.py", line 89, in run
    assert live == ("ValidationException", _NO_STEPS.format(plan_id)), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: starting a live plan with no steps must be refused as invalid, naming it; got ('NotImplemented', 'operation StartRecoveryPlanExecution is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the drs emulator
Call StartRecoveryPlanExecution and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
