Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: comprehendmedical.start-rxnorm-job-missing-input-folder-rejected comprehendmedical.stop-rxnorm-job-after-stopped-rejected
Issue-Shape-Id: com.amazonaws.comprehendmedical#ComprehendMedical_20181030
Issue-Key: comprehendmedical|StartRxNormInferenceJob|NotImplemented
Issue-Commit: b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86

# Implement comprehendmedical StartRxNormInferenceJob: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `b60f6fc1d7d48b8ab01ee41fe8076d8fc438ec86`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartRxNormInferenceJob

## Description
`StartRxNormInferenceJob` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_rx_norm_inference_job.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

2 recorded unit(s) fail on their `StartRxNormInferenceJob` call, so each one unblocks only when `StartRxNormInferenceJob` works: `comprehendmedical.start-rxnorm-job-missing-input-folder-rejected`, `comprehendmedical.stop-rxnorm-job-after-stopped-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-comprehendmedical-comprehendmedical-20181030/src/ops/start_rx_norm_inference_job.rs`. Replaying the corpus, the emulator refused the `StartRxNormInferenceJob` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartRxNormInferenceJob operation: operation StartRxNormInferenceJob is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the comprehendmedical emulator
Call StartRxNormInferenceJob and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
