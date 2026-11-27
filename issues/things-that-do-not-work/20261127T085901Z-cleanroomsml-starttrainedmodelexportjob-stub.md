Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cleanroomsml.trained-model-export-without-model-output-ability-rejected
Issue-Shape-Id: com.amazonaws.cleanroomsml#AWSStarkControlService
Issue-Key: cleanroomsml.trained-model-export-without-model-output-ability-rejected|run#12|AssertionError|assert refused == (
Issue-Commit: 2a4a70a5faf0988854d86ea4e60d7f7575e84c25

# Implement cleanroomsml StartTrainedModelExportJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `2a4a70a5faf0988854d86ea4e60d7f7575e84c25`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartTrainedModelExportJob

## Description
`StartTrainedModelExportJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice/src/ops/start_trained_model_export_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartTrainedModelExportJob` call, so each one unblocks only when `StartTrainedModelExportJob` works: `cleanroomsml.trained-model-export-without-model-output-ability-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cleanroomsml-awsstarkcontrolservice/src/ops/start_trained_model_export_job.rs`. Replaying the corpus, the emulator refused the `StartTrainedModelExportJob` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_cleanroomsml_trained_model_export_without_model_output_ability_rejected.py", line 123, in run
    assert refused == (
           ^^^^^^^^^^^^
AssertionError: export by a member without the model output ability answered ('NotImplemented', 'operation StartTrainedModelExportJob is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the cleanroomsml emulator
Call StartTrainedModelExportJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
