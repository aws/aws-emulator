Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.start-speech-synthesis-task-lexicon-not-found-error polly.start-speech-synthesis-task-marks-rejected-for-mp3-format polly.start-speech-synthesis-task-text-length-exceeded-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.start-speech-synthesis-task-lexicon-not-found-error|run#5|AssertionError|assert code == "LexiconNotFoundException", code
Issue-Key: polly.start-speech-synthesis-task-marks-rejected-for-mp3-format|run#4|AssertionError|assert code == "MarksNotSupportedForFormatException", code
Issue-Key: polly.start-speech-synthesis-task-text-length-exceeded-rejected|run#6|AssertionError|assert code == "TextLengthExceededException", f"expected TextLengthExceededException, got {code!r}"
Issue-Commit: eb39d16500e074a8b39339f856d6c2c91f8a4fdc

# Polly StartSpeechSynthesisTask performs no request validation: lexicon, speech-mark/format and text-length rejections all answer NotImplemented instead of the modeled 4xx errors

> _Note: this issue was identified against the code base at commit `eb39d16500e074a8b39339f856d6c2c91f8a4fdc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
StartSpeechSynthesisTask is a scaffold that answers HTTP 501 / NotImplemented for every request, so none of the synchronous rejections the model declares for the async task API are produced. The open issue 'Implement polly StartSpeechSynthesisTask: unimplemented scaffold stub' (20261022T175007Z-polly-startspeechsynthesistask-stub.md) carries only the seven happy-path/SNS replay keys listed in its Issue-Trace-Ids; the three negative-class checks below are NOT among them and are not covered by it, so they are recorded here as the rejection semantics the implementation must also satisfy. 3 causes.

Cause 1 of 3: a never-stored lexicon named in LexiconNames must be refused synchronously.
AWS: Smithy: StartSpeechSynthesisTask models LexiconNames (LexiconNameList, @length 0..5) and the error LexiconNotFoundException, errorKind client, httpError 404 — doc: 'Amazon Polly can't find the specified lexicon.' Recorded behavior: HTTP 404, error code LexiconNotFoundException, message (fixed per code) 'Lexicon not found'; no task is created, and the same request without the bogus LexiconNames succeeds and reaches TaskStatus=completed.
Emulator: the call returns error code 'NotImplemented' (HTTP 501, 'operation StartSpeechSynthesisTask is scaffolded but not implemented in this emulator'), so the lexicon reference is never resolved.
Why: a client that branches on LexiconNotFoundException to repair a missing-lexicon reference sees an unmodeled NotImplemented instead, which the SDK treats as a non-retryable server fault rather than a correctable client error.
Fix: the operation's handler plus its validation — resolve each LexiconNames entry against the lexicon store before any task is created, and return the modeled error from crate::service::errors::lexicon_not_found().
Traces: polly.start-speech-synthesis-task-lexicon-not-found-error.

Cause 2 of 3: SpeechMarkTypes combined with a compressed OutputFormat must be refused, exactly as on the synchronous SynthesizeSpeech path.
AWS: Smithy: StartSpeechSynthesisTask models SpeechMarkTypes (@length 0..4) and MarksNotSupportedForFormatException, errorKind client, httpError 400 — doc: 'Speech marks are not supported for the OutputFormat selected. Speech marks are only available for content in json format.' Recorded behavior: HTTP 400, code MarksNotSupportedForFormatException, message 'Speech marks are not supported for this format'; no task is created and nothing is written to the destination bucket, while OutputFormat=json with the same SpeechMarkTypes is accepted and completes with output written to S3.
Emulator: error code 'NotImplemented' (HTTP 501) for both the rejected and the accepted variant.
Why: the marks/format incompatibility is only discoverable at request time; a client gets an unmodeled fault and cannot tell a bad combination from an outage.
Fix: the operation's validation (shared with the SynthesizeSpeech marks/format rule) plus the handler's task creation path; message via crate::service::errors::marks_not_supported_for_format().
Traces: polly.start-speech-synthesis-task-marks-rejected-for-mp3-format.

Cause 3 of 3: the async API's own, much larger text bound must be enforced.
AWS: Smithy: StartSpeechSynthesisTask models Text as required and TextLengthExceededException, errorKind client, httpError 400 — doc: 'For the StartSpeechSynthesisTask API, the maximum is 200,000 characters, of which no more than 100,000 can be billed characters.' Recorded behavior: 100,001 billable characters is rejected with HTTP 400, code TextLengthExceededException, message 'Maximum text length has been exceeded', and ListSpeechSynthesisTasks shows no task created by the rejected call; a short text then starts a real task visible through GetSpeechSynthesisTask.
Emulator: error code 'NotImplemented' (HTTP 501), so the bound is not enforced and the 3000-character SynthesizeSpeech limit is not distinguished from the task API's 100,000 billable-character limit.
Why: a client that chunks long text based on which limit applies cannot discover the async limit at all, and an over-long request is indistinguishable from a broken service.
Fix: the operation's validation (billable-character count, independent of the SynthesizeSpeech limit) plus the handler's state write, so a rejected call persists no task.
Traces: polly.start-speech-synthesis-task-text-length-exceeded-rejected.

## Evidence
All three checks fail on the StartSpeechSynthesisTask call:

polly.start-speech-synthesis-task-lexicon-not-found-error — expected code 'LexiconNotFoundException' (HTTP 404, message 'Lexicon not found'); AssertionError: NotImplemented.
polly.start-speech-synthesis-task-marks-rejected-for-mp3-format — expected code 'MarksNotSupportedForFormatException' (HTTP 400, message 'Speech marks are not supported for this format'); AssertionError: NotImplemented.
polly.start-speech-synthesis-task-text-length-exceeded-rejected — AssertionError: expected TextLengthExceededException, got 'NotImplemented'.

The emulator's response body is: An error occurred (NotImplemented) when calling the StartSpeechSynthesisTask operation: operation StartSpeechSynthesisTask is scaffolded but not implemented in this emulator (HTTP 501). The handler is services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs; the per-code message constants already exist in services/service-com-amazonaws-polly-parrot-v1/src/service.rs (LEXICON_NOT_FOUND, MARKS_NOT_SUPPORTED_FOR_FORMAT, TEXT_LENGTH_EXCEEDED) and are unused by it.

## Steps to Reproduce
1. Boot the polly emulator (plus s3, which the checks use for the output bucket).
2. Create an S3 bucket for output.
3. Call StartSpeechSynthesisTask with OutputFormat=mp3, VoiceId=Joanna, a valid Text and LexiconNames=[<a name never stored>]. Expect HTTP 404 LexiconNotFoundException; observe HTTP 501 NotImplemented.
4. Call StartSpeechSynthesisTask with OutputFormat=mp3 and SpeechMarkTypes=[sentence]. Expect HTTP 400 MarksNotSupportedForFormatException and no task created; observe HTTP 501 NotImplemented.
5. Call StartSpeechSynthesisTask with Text of 100,001 'a' characters. Expect HTTP 400 TextLengthExceededException and no new task in ListSpeechSynthesisTasks; observe HTTP 501 NotImplemented.
6. Replay polly.start-speech-synthesis-task-lexicon-not-found-error, polly.start-speech-synthesis-task-marks-rejected-for-mp3-format and polly.start-speech-synthesis-task-text-length-exceeded-rejected.
