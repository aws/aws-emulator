Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: entityresolution.start-id-mapping-job-unknown-workflow-not-found-rejected
Issue-Shape-Id: com.amazonaws.entityresolution#AWSVeniceService
Issue-Key: entityresolution.start-id-mapping-job-unknown-workflow-not-found-rejected|run#32|AssertionError|assert got == expected, f"StartIdMappingJob on a never-created workflow gave {got!r}, expected {expected!r}"
Issue-Commit: 0f328bb6ae93185eda2a76c9c7fc6646652b63ca

# Implement entityresolution StartIdMappingJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `0f328bb6ae93185eda2a76c9c7fc6646652b63ca`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartIdMappingJob

## Description
`StartIdMappingJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/start_id_mapping_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartIdMappingJob` call, so each one unblocks only when `StartIdMappingJob` works: `entityresolution.start-id-mapping-job-unknown-workflow-not-found-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-entityresolution-awsveniceservice/src/ops/start_id_mapping_job.rs`. Replaying the corpus, the emulator refused the `StartIdMappingJob` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_entityresolution_start_id_mapping_job_unknown_workflow_not_found_rejected.py", line 351, in run
    assert got == expected, f"StartIdMappingJob on a never-created workflow gave {got!r}, expected {expected!r}"
           ^^^^^^^^^^^^^^^
AssertionError: StartIdMappingJob on a never-created workflow gave ('NotImplemented', 'operation StartIdMappingJob is scaffolded but not implemented in this emulator'), expected ('ResourceNotFoundException', 'Workflow not found.')

## Steps to Reproduce
Boot the entityresolution emulator
Call StartIdMappingJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
