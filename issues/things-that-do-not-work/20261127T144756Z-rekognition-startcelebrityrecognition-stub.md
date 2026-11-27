Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rekognition.get-celebrity-recognition-of-a-face-detection-job-rejected-not-found rekognition.get-celebrity-recognition-unissued-next-token-rejected rekognition.start-celebrity-recognition-reused-token-changed-job-tag-rejected
Issue-Shape-Id: com.amazonaws.rekognition#RekognitionService
Issue-Key: rekognition|StartCelebrityRecognition|NotImplemented
Issue-Commit: fc909f682d274824e0181cbb84eb6914c2fe111f

# Implement rekognition StartCelebrityRecognition: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `fc909f682d274824e0181cbb84eb6914c2fe111f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCelebrityRecognition

## Description
`StartCelebrityRecognition` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/start_celebrity_recognition.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

3 recorded unit(s) fail on their `StartCelebrityRecognition` call, so each one unblocks only when `StartCelebrityRecognition` works: `rekognition.get-celebrity-recognition-of-a-face-detection-job-rejected-not-found`, `rekognition.get-celebrity-recognition-unissued-next-token-rejected`, `rekognition.start-celebrity-recognition-reused-token-changed-job-tag-rejected`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-rekognition-rekognitionservice/src/ops/start_celebrity_recognition.rs`. Replaying the corpus, the emulator refused the `StartCelebrityRecognition` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
           ^^^^^^^^^^^^^^^^^^^^^
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the StartCelebrityRecognition operation: operation StartCelebrityRecognition is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the rekognition emulator
Call StartCelebrityRecognition and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
