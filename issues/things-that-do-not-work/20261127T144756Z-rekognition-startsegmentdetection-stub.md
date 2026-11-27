Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.start-segment-detection-repeated-segment-type-rejected-invalid-parameter
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition.start-segment-detection-repeated-segment-type-rejected-invalid-parameter|run#11|AssertionError|assert got == ("InvalidParameterException", "Request has invalid parameters"), (
Issue-Commit: fc909f682d274824e0181cbb84eb6914c2fe111f

# Implement rekognition StartSegmentDetection: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fc909f682d274824e0181cbb84eb6914c2fe111f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSegmentDetection

## Description
`StartSegmentDetection` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/start_segment_detection.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

1 recorded unit(s) fail on their `StartSegmentDetection` call, so each one unblocks only when `StartSegmentDetection` works: `rekognition.start-segment-detection-repeated-segment-type-rejected-invalid-parameter`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/start_segment_detection.rs`. Replaying the corpus, the emulator refused the `StartSegmentDetection` call: File "run.py", line 461, in <lambda>
    body = calls.in_phase(calls.BODY, lambda: fn(ctx))
                                              ^^^^^^^
  File "check_rekognition_start_segment_detection_repeated_segment_type_rejected_invalid_parameter.py", line 112, in run
    assert got == ("InvalidParameterException", "Request has invalid parameters"), (
           ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
AssertionError: SegmentTypes ['SHOT', 'SHOT'] answered ('NotImplemented', 'operation StartSegmentDetection is scaffolded but not implemented in this emulator')

## Steps to Reproduce
Boot the rekognition emulator
Call StartSegmentDetection and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
