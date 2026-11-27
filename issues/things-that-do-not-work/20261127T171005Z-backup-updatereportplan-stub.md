Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup.report-plan-update-with-unknown-format-rejected
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup.report-plan-update-with-unknown-format-rejected|run#14|AssertionError|assert (code, message) == ("InvalidParameterValueException", expected), (
Issue-Commit: f26705532556bda9b53da6f6657203498f448a8c

# Implement backup UpdateReportPlan: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `f26705532556bda9b53da6f6657203498f448a8c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateReportPlan

## Description
`UpdateReportPlan` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_report_plan.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `UpdateReportPlan` call, so each one unblocks only when `UpdateReportPlan` works: `backup.report-plan-update-with-unknown-format-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/update_report_plan.rs`. Replaying the corpus, the emulator refused the `UpdateReportPlan` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_backup_report_plan_update_with_unknown_format_rejected.py", line 105, in run
    assert (code, message) == ("InvalidParameterValueException", expected), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: an update naming format XML was refused with 'NotImplemented' 'operation UpdateReportPlan is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the backup emulator
Call UpdateReportPlan and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
