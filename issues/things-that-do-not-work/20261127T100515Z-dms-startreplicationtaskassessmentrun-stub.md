Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dms.assessment-run-start-rejected-for-absent-result-key
Issue-Shape-Id: com.amazonaws.databasemigrationservice#AmazonDMSv20160101
Issue-Key: dms.assessment-run-start-rejected-for-absent-result-key|run#50|AssertionError|assert code == "S3AccessDeniedFault", (
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# Implement dms StartReplicationTaskAssessmentRun: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartReplicationTaskAssessmentRun

## Description
`StartReplicationTaskAssessmentRun` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/start_replication_task_assessment_run.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartReplicationTaskAssessmentRun` call, so each one unblocks only when `StartReplicationTaskAssessmentRun` works: `dms.assessment-run-start-rejected-for-absent-result-key`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-databasemigrationservice-amazondmsv20160101/src/ops/start_replication_task_assessment_run.rs`. Replaying the corpus, the emulator refused the `StartReplicationTaskAssessmentRun` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_dms_assessment_run_start_rejected_for_absent_result_key.py", line 353, in run
    assert code == "S3AccessDeniedFault", (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: an assessment run naming an absent result key was refused with 'NotImplemented': 'operation StartReplicationTaskAssessmentRun is scaffolded but not implemented in this emulator'

## Steps to Reproduce
Boot the dms emulator
Call StartReplicationTaskAssessmentRun and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
