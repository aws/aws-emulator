Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: polly.start-speech-synthesis-task-generative-speech-marks-rejected polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected polly.start-speech-synthesis-task-malformed-ssml-rejected polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected polly.start-speech-synthesis-task-unlisted-language-code-rejected polly.stream-closed-event-counts-the-characters-of-every-text-event
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly.start-speech-synthesis-task-generative-speech-marks-rejected|_refusal#5|AssertionError|raise AssertionError(f"Polly accepted a task it should have refused: {accepted!r}")
Issue-Key: polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected|run#21|AssertionError|assert written == [expected_key], f"the task wrote {written!r}, expected [{expected_key!r}]"
Issue-Key: polly.start-speech-synthesis-task-malformed-ssml-rejected|run#14|AssertionError|assert keys == [f"ssml-audio.{task_id}.mp3"], f"objects in the task bucket: {keys!r}"
Issue-Key: polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected|run#28|KeyError|seen = [(m["value"], m["start"], m["end"]) for m in marks]
Issue-Key: polly|StartSpeechSynthesisTask|ValidationException
Issue-Key: polly.stream-closed-event-counts-the-characters-of-every-text-event|run#6|AssertionError|assert billed[:2] == plain, (
Issue-Commit: daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7

# Polly asynchronous synthesis and billing diverge: generative speech marks accepted, output key omits the '.' separator, SSML marks carry no offsets, en-CA refused, and billing counts edge whitespace (5 causes)

> _Note: this issue was identified against the code base at commit `daaa9dcfb0a4f141a4fa2b0cdadbffeb8f4f06f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartSpeechSynthesisTask

## Description
Five independent, net-new root causes in the Polly emulator's asynchronous-synthesis path and in the character-billing rule it shares with SynthesizeSpeech. All five were found by the behavioral suite replay against this checkout (polly 54/66). 5 causes — a fixer must close all five before submitting.

Cause 1 of 5: StartSpeechSynthesisTask accepts speech marks on the generative engine instead of refusing them.
Failing check: polly.start-speech-synthesis-task-generative-speech-marks-rejected.
AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) declares Engine = [standard, neural, long-form, generative] and SpeechMarkType = [sentence, ssml, viseme, word], and binds StartSpeechSynthesisTask to MarksNotSupportedForFormatException and SsmlMarksNotSupportedForTextTypeException only — it models NO engine/mark-type error, so this refusal is recorded behavior only. The recording shows a 400 with error code ValidationException and the message "The selected speech mark type - word - is not supported for this engine: generative". Fixed text of that message: "The selected speech mark type - ", " - is not supported for this engine: "; variable parts: the offending value from the request's SpeechMarkTypes and the value of the request's Engine. No task is created, and nothing is written to the output bucket. The same request with Engine=neural is accepted and completes.
Emulator: answers 200 and mints the task — the replay records the accepted SynthesisTask {'Engine': 'generative', 'TaskStatus': 'scheduled', 'OutputFormat': 'json', 'SpeechMarkTypes': ['word'], 'TextType': 'text', 'VoiceId': 'Ruth'}. The handler validates marks-vs-format and ssml-marks-vs-text-type but has no engine-vs-mark-type rule at all.
Why: a client that relies on the refusal to fall back to a supported engine instead receives an accepted task id, waits for it, and consumes a speech-mark stream AWS would never have produced; any code path guarded by that ValidationException is never exercised.
Fix: the request validation in the operation's handler, alongside the existing marks/format and ssml/text-type refusals (services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs, the validation block at lines 344-362). Apply the same rule in the SynthesizeSpeech validation so the two operations cannot disagree.
Traces: polly.start-speech-synthesis-task-generative-speech-marks-rejected.

Cause 2 of 5: the task's S3 output key omits the '.' separator between OutputS3KeyPrefix and the TaskId.
Failing checks: polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected, polly.start-speech-synthesis-task-malformed-ssml-rejected.
AWS: Smithy models OutputS3KeyPrefix as a string with pattern ^[0-9a-zA-Z\/\!\-_\.\*\'\(\):;\$@=+\,\?&]{0,800}$ and documents OutputUri only as "Pathway for the output speech file", so the key template is recorded behavior only. Both recordings show the completed task writing exactly ONE object whose key is <OutputS3KeyPrefix>.<TaskId>.<suffix-of-OutputFormat> — the fixed part is the literal '.' joining the prefix to the task id and the '.' before the format suffix; the prefix comes from the request, the task id is minted per run, and the suffix follows OutputFormat (mp3 for mp3, marks for json). A request naming no prefix writes <TaskId>.<suffix> at the bucket root, which the emulator already gets right.
Emulator: writes the prefix and the task id concatenated with nothing between them. Replay: "the task wrote ['narration-drafts/4c877bd3-7fcf-4315-86d9-e131f95af79c.mp3'], expected ['narration-drafts/.4c877bd3-7fcf-4315-86d9-e131f95af79c.mp3']" and "objects in the task bucket: ['ssml-audio92fad2d5-d0dc-4200-bc1a-4f6d45b375eb.mp3']" where the key must be ssml-audio.<task id>.mp3. The announced OutputUri carries the same malformed key (e.g. .../marks248d3d48-...marks for prefix "marks").
Why: a client that computes the output key itself — the documented <prefix>.<taskId>.<ext> form — gets NoSuchKey on GetObject/HeadObject, and a client listing by a prefix that ends in a delimiter finds the object under a different name than AWS puts it under.
Fix: the output-location derivation of the operation's handler — output_key() in services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_task.rs lines 282-292, whose doc comment asserts "the identifier and the suffix follow it with nothing inserted"; the same key feeds the stored TaskOutputDelivery and the announced OutputUri. This is NOT covered by the open issue 20261121T155022Z-polly-startspeechsynthesistask-announces-an-outputuri-that-does-not.md, which is about the announced AUTHORITY and explicitly (and wrongly) states that the key derivation "is already correct and should not change" — that recording used no OutputS3KeyPrefix, so the missing separator was invisible to it.
Traces: polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected, polly.start-speech-synthesis-task-malformed-ssml-rejected.

Cause 3 of 5: SSML speech marks are emitted without their start/end offsets.
Failing check: polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected.
AWS: Smithy declares SpeechMarkType value "ssml"; the JSON speech-mark stream's shape is recorded behavior only. The recording shows one JSON object per line, and for an ssml mark the object carries time, type ("ssml"), start, end and value, where value is the <mark> element's name attribute and start/end are the byte offsets in Text of that <mark/> element (start = byte index of the element, end = start + its byte length). Fixed text: the member names time/type/start/end/value; the numbers and the mark name are values taken from the request's Text.
Emulator: the marks it writes have the right type set ({"ssml"}) but no offset members at all — the check dies with KeyError: 'start' when reading m["start"] from the emulator's mark objects.
Why: SSML marks exist so a consumer can align the audio back to a position in its own source document; without start/end a client cannot map a mark to the text it annotates, and a strongly typed deserializer of the mark stream fails outright on the missing members.
Fix: the speech-mark serializer of the synthesis path — the marks writer in services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs (the json/speech-mark stream described in its module doc, lines 49-54) as consumed by the task's output delivery. Distinct from the open issue 20261030T071447Z-polly-synthesizespeech-output-stream-diverges-from-the-rendered.md, which covers mp3 payload and WORD-mark timings, not the absent ssml offsets.
Traces: polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected.

Cause 4 of 5: a well-formed LanguageCode outside the refusal's value set (en-CA) is refused instead of accepted.
Failing check: polly.start-speech-synthesis-task-unlisted-language-code-rejected.
AWS: Smithy's LanguageCode enum lists 42 values (including en-SG, excluding en-CA), while the ValidationException the real service sends enumerates a different 41-value set (no en-SG, no en-CA). Recorded behavior: AWS refuses a code that names no language (xx-XX) with 400 ValidationException "1 validation error detected: Value '<value>' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [<the 41 values, in the service's order>]" — the emulator already matches this — but it ACCEPTS LanguageCode=en-CA, starts the task, keeps LanguageCode=en-CA on it and completes it. So the member's accepted domain is wider than the set the message lists.
Emulator: refuses en-CA with 400 ValidationException "1 validation error detected: Value 'en-CA' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en-IE, ar-AE, en-US, ...]" — enum-set validation is applied as a closed gate, so no task is created.
Why: a client synthesizing with a language code AWS accepts gets a hard 400 from the emulator; the operation is unusable for those codes and the refusal cannot be distinguished from a genuinely bogus code.
Fix: the LanguageCode validation shared by StartSpeechSynthesisTask and SynthesizeSpeech (the boundary enum check plus the custom value-set message introduced for 20261029T203859Z-polly-synthesizespeech-s-languagecode-validationexception-message-lists-the-enum.md): the value set drives the MESSAGE, but acceptance must not be restricted to it.
Traces: polly.start-speech-synthesis-task-unlisted-language-code-rejected.

Cause 5 of 5: RequestCharacters bills whitespace at the edges of plain text.
Failing check: polly.stream-closed-event-counts-the-characters-of-every-text-event.
AWS: Smithy documents SynthesisTask$RequestCharacters / SynthesizeSpeechOutput$RequestCharacters as the "Number of billable characters synthesized"; the exact rule is recorded behavior only. The recording shows that for plain text (TextType=text) the billed count is the length of the text with surrounding whitespace removed: "Counting begins in one event, " (30 characters, one trailing space) bills 29, and "continues in a second one, " (27 characters) bills 26. The requirement is the rule, not these numbers: leading/trailing whitespace of the submitted text is not billable.
Emulator: bills the raw length — replay: "SynthesizeSpeech billed [30, 27] for plain text of stripped lengths [29, 26]".
Why: every billed-character figure a client reads back (SynthesizeSpeech's x-amzn-RequestCharacters header, SynthesisTask$RequestCharacters, and StreamClosedEvent$RequestCharacters, which is the sum of these counts) is inflated for any text with edge whitespace, so quota accounting and cost estimation built on it are wrong.
Fix: the character-billing component shared by the synthesis paths — billed_characters(), used by services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs and by start_speech_synthesis_task.rs (line 375) and by the stream's StreamClosedEvent accumulation; its module doc states the rule as "every character of plain text", which is what must change for the text's edges. Note the same check further asserts that a stream's StreamClosedEvent counts each plain-text TextEvent IN FULL (the joining spaces included) plus the SSML event's billed count — it never reached that assertion, so the stream-side accumulation is unverified here.
Traces: polly.stream-closed-event-counts-the-characters-of-every-text-event.

## Evidence
Behavioral suite replay against this checkout: 54/68 passed (79.4%); per-service polly 54/66 (82%), 12 failed. bootstrap_signal=false, the emulator served every call below.

Cause 1 — polly.start-speech-synthesis-task-generative-speech-marks-rejected [mismatch]
  expected: ClientError ("ValidationException", "The selected speech mark type - word - is not supported for this engine: generative"), no task created
  actual:   200 + AssertionError: Polly accepted a task it should have refused: {'Engine': 'generative', 'TaskId': '248d3d48-cd0f-469f-9e09-e87418dc6849', 'TaskStatus': 'scheduled', 'OutputUri': 'https://127.0.0.1:45117/sim2real-polly-marks-76cd7e7a4c09/marks248d3d48-cd0f-469f-9e09-e87418dc6849.marks', 'RequestCharacters': 35, 'OutputFormat': 'json', 'SpeechMarkTypes': ['word'], 'TextType': 'text', 'VoiceId': 'Ruth'}

Cause 2 — polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected [mismatch]
  AssertionError: the task wrote ['narration-drafts/4c877bd3-7fcf-4315-86d9-e131f95af79c.mp3'], expected ['narration-drafts/.4c877bd3-7fcf-4315-86d9-e131f95af79c.mp3']
  (the preceding refusal of the illegal prefix 'narration#drafts/' with the pattern ValidationException PASSED)
        polly.start-speech-synthesis-task-malformed-ssml-rejected [mismatch]
  AssertionError: objects in the task bucket: ['ssml-audio92fad2d5-d0dc-4200-bc1a-4f6d45b375eb.mp3'] (expected ['ssml-audio.92fad2d5-d0dc-4200-bc1a-4f6d45b375eb.mp3'])
  Corroboration: src/ops/start_speech_synthesis_task.rs:288-292 — format!("{prefix}{task_id}.{suffix}").

Cause 3 — polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected [error]
  the ssml-marks refusal for TextType=text and the <task id>.marks object name both PASSED; reading the marks then raised
  KeyError: 'start' at `seen = [(m["value"], m["start"], m["end"]) for m in marks]` — expected one mark ('turnpoint', <byte offset of <mark name="turnpoint"/>>, offset+len(tag)).

Cause 4 — polly.start-speech-synthesis-task-unlisted-language-code-rejected [error]
  botocore.errorfactory.ValidationException: An error occurred (ValidationException) when calling the StartSpeechSynthesisTask operation: 1 validation error detected: Value 'en-CA' at 'languageCode' failed to satisfy constraint: Member must satisfy enum value set: [en-IE, ar-AE, en-US, fr-BE, en-IN, es-MX, en-ZA, tr-TR, ru-RU, ro-RO, pt-PT, pl-PL, nl-NL, it-IT, is-IS, fr-FR, fi-FI, es-ES, de-DE, yue-CN, ko-KR, en-NZ, en-GB-WLS, hi-IN, de-CH, arb, nl-BE, cy-GB, cs-CZ, cmn-CN, da-DK, en-AU, pt-BR, nb-NO, sv-SE, ja-JP, es-US, ca-ES, fr-CA, en-GB, de-AT]
  AWS accepts that call (and the recording's ja-JP call too), keeps LanguageCode on the task and completes it; the xx-XX refusal with this same message is correct and PASSED.

Cause 5 — polly.stream-closed-event-counts-the-characters-of-every-text-event [mismatch]
  AssertionError: SynthesizeSpeech billed [30, 27] for plain text of stripped lengths [29, 26]
  texts: "Counting begins in one event, " and "continues in a second one, " (each with a trailing space).

Model facts used: smithy_query(query="digest") and smithy_query(query="shape", shape="LanguageCode,SpeechMarkType,Engine,OutputS3KeyPrefix,SynthesisTask") on services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json.

## Steps to Reproduce
Boot the emulator from this checkout, then:

Cause 1: create an S3 bucket; call StartSpeechSynthesisTask(Engine="generative", VoiceId="Ruth", OutputFormat="json", SpeechMarkTypes=["word"], Text="Every spoken word gets its own mark", OutputS3BucketName=<bucket>, OutputS3KeyPrefix="marks"). Expect 400 ValidationException "The selected speech mark type - word - is not supported for this engine: generative" and no task; the emulator returns 200 with a scheduled task. The same request with Engine="neural" must still be accepted and complete.

Cause 2: call StartSpeechSynthesisTask(Engine="standard", OutputFormat="mp3", VoiceId="Matthew", Text="A short note read aloud into the archive.", OutputS3BucketName=<bucket>, OutputS3KeyPrefix="narration-drafts/"); poll GetSpeechSynthesisTask until completed, then ListObjectsV2 the bucket. Expect exactly ['narration-drafts/.<TaskId>.mp3']; the emulator writes 'narration-drafts/<TaskId>.mp3'. Also check SynthesisTask.OutputUri's path ends in /<bucket>/narration-drafts/.<TaskId>.mp3.

Cause 3: call StartSpeechSynthesisTask(Engine="standard", TextType="ssml", OutputFormat="json", SpeechMarkTypes=["ssml"], VoiceId="Joanna", Text='<speak>The bell rang twice before <mark name="turnpoint"/>the doors opened.</speak>', OutputS3BucketName=<bucket>); wait for completed and GetObject the <TaskId>.marks object. Each JSON line must carry time, type, start, end and value; the emulator's lines carry no start/end.

Cause 4: call StartSpeechSynthesisTask(LanguageCode="en-CA", OutputFormat="mp3", VoiceId="Joanna", OutputS3BucketName=<bucket>, OutputS3KeyPrefix="lang", Text="A short sentence read aloud whatever language code the task names."). Expect 200, a task that reports LanguageCode="en-CA" and completes; the emulator answers 400 ValidationException. LanguageCode="xx-XX" must still be refused with the 41-value enum message.

Cause 5: call SynthesizeSpeech(Text="Counting begins in one event, ", TextType="text", OutputFormat="pcm", Engine="generative", VoiceId="Ruth") and read RequestCharacters. Expect 29 (the text has 30 characters, one of them a trailing space); the emulator reports 30.

Re-replay these checks to confirm: polly.start-speech-synthesis-task-generative-speech-marks-rejected, polly.start-speech-synthesis-task-invalid-s3-key-prefix-rejected, polly.start-speech-synthesis-task-malformed-ssml-rejected, polly.start-speech-synthesis-task-ssml-marks-for-plain-text-rejected, polly.start-speech-synthesis-task-unlisted-language-code-rejected, polly.stream-closed-event-counts-the-characters-of-every-text-event.
