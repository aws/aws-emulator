Issue-Source: trace-sweep-divergence
Issue-Trace: polly-generative-engine-speech-marks-rejected
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: polly-generative-engine-speech-marks-rejected polly-list-speech-synthesis-tasks-filter-rejected polly-list-speech-synthesis-tasks-paging-rejected
Issue-Shape-Id: com.amazonaws.polly#Parrot_v1
Issue-Key: polly|SynthesizeSpeech|4xx->2xx|$
Issue-Key: polly|SynthesizeSpeech|4xx->2xx|$headers.content-type
Issue-Key: polly|SynthesizeSpeech|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: polly|SynthesizeSpeech|4xx->2xx|status
Issue-Key: polly|SynthesizeSpeech|4xx->2xx|$.message
Issue-Key: polly|ListSpeechSynthesisTasks|4xx->4xx|$.message
Issue-Commit: 28742f8d6c57d4e890e53dd305c6c6fd7ecee06b

# Polly request-shape validation diverges: generative-engine speech marks accepted, ValidationException enum value set in declaration order

> _Note: this issue was identified against the code base at commit `28742f8d6c57d4e890e53dd305c6c6fd7ecee06b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SynthesizeSpeech

## Description
Two independent, net-new root causes in the Polly emulator's request-shape validation, both found by replaying the wire corpus against this checkout (simparity-wire-aws-v1, 54/64 steps passed, bootstrap_signal=false, the emulator served every call). 2 causes — a fixer must close BOTH before submitting.

Cause 1 of 2: SynthesizeSpeech accepts SpeechMarkTypes together with Engine=generative instead of refusing the request, for every mark type.
AWS: Smithy (services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json) declares SynthesizeSpeech with input members Engine (enum Engine = [standard, neural, long-form, generative]) and SpeechMarkTypes (list of SpeechMarkType = [sentence, ssml, viseme, word], @length 0..4), and binds the operation to EngineNotSupportedException, InvalidSampleRateException, InvalidSsmlException, LanguageNotSupportedException, LexiconNotFoundException, MarksNotSupportedForFormatException, SsmlMarksNotSupportedForTextTypeException, TextLengthExceededException and ServiceFailureException — it models NO engine-vs-mark-type error, so this refusal is recorded behavior only. The recording shows the request answered with HTTP 400, an error response whose Content-Type is application/json and which carries the x-amzn-errortype header, and a body whose `message` member reads "The selected speech mark type - word - is not supported for this engine: generative". That message is a template: its FIXED text is "The selected speech mark type - " ... " - is not supported for this engine: "; its VARIABLE parts are the offending value taken from the request's SpeechMarkTypes (the recording also shows "sentence" in the same template) and the value of the request's Engine. No audio and no speech-mark stream is produced. The same request on a non-generative engine is accepted and returns the mark stream.
Emulator: answers HTTP 200 with Content-Type application/x-json-stream and serves the speech-mark stream anyway (one JSON object per line, type "word"/"sentence"), sends no x-amzn-errortype header and no `message` member at all. The validation listed in the handler's module doc (services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs lines 19-54) checks lexicon names, engine-vs-voice, SSML, SampleRate and the text limits, and explicitly promises json output "in all four modelled types" with no engine condition — there is no engine-vs-mark-type rule.
Why: a client that relies on the 400 to fall back to a supported engine (or to surface the error to its caller) instead receives a 200 and consumes a speech-mark stream the real service would never produce; the error branch guarded by that ValidationException is never exercised, and the response's content type and error headers differ too, so even a generic error-handling client sees success.
Fix: the request validation of the operation's handler in services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs, alongside the existing marks-vs-format and ssml-marks-vs-text-type refusals, so the rule holds for every SpeechMarkType value and is applied before any stream is serialized. The same rule must hold for StartSpeechSynthesisTask so the two operations cannot disagree; the asynchronous side of this same gap is reported behaviorally in the open issue 20261127T095437Z-polly-asynchronous-synthesis-and-billing-diverge-generative-speech.md (Cause 1 there, StartSpeechSynthesisTask), which does NOT cover SynthesizeSpeech and does not carry these fixture steps.
Traces: polly-generative-engine-speech-marks-rejected#task#4 (also #task#5, SpeechMarkTypes=[word]) and polly-generative-engine-speech-marks-rejected#task#6 (SpeechMarkTypes=[sentence]).

Cause 2 of 2: the ValidationException message for an out-of-domain ListSpeechSynthesisTasks Status lists the enum value set in the model's declaration order instead of the order the service uses.
AWS: Smithy models ListSpeechSynthesisTasks$Status as httpQuery "Status" targeting enum TaskStatus, whose declaration order is [scheduled, inProgress, completed, failed]; the operation models only InvalidNextTokenException and ServiceFailureException, so the request-shape refusal is recorded behavior only. The recording shows HTTP 400 with message "1 validation error detected: Value 'finished' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [completed, scheduled, inProgress, failed]". Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: Member must satisfy enum value set: [", "]"; variable parts: the rejected value from the request, the member path ("status"), and the value set — which the service renders in ITS order, [completed, scheduled, inProgress, failed], not the model's declaration order. The status code and the value 'finished' are per-request/per-recording; the order of the listed set is the requirement.
Emulator: answers 400 with the right code and the right fixed text but the set in model order — "... Member must satisfy enum value set: [scheduled, inProgress, completed, failed]".
Why: a client or test that matches the full ValidationException message (the usual way this refusal is asserted, since the error carries no structured member/field list) fails to match, and a client that parses the bracketed set to offer the caller the allowed values presents them in an order AWS never sends.
Fix: the enum value-set rendering of the shared request-shape boundary validator that builds this message for Polly's query/body enum members (it currently enumerates the Smithy declaration order); the Status member of ListSpeechSynthesisTasks must render the service's order. Note the same message template is already tuned for LanguageCode by 20261029T203859Z-polly-synthesizespeech-s-languagecode-validationexception-message-lists-the-enum.md — TaskStatus needs the same treatment and is not covered there.
Traces: polly-list-speech-synthesis-tasks-filter-rejected#task#3 (also polly-list-speech-synthesis-tasks-paging-rejected#task#4).

## Evidence
Wire corpus replay (simparity-wire-aws-v1) against this checkout: 15 fixtures replayed, 64 steps, 54 passed, 10 mismatched, 0 errored, 0 blocked, score 84.4%. emulator_up=true, bootstrap_signal=false.

Cause 1 — fixture polly-generative-engine-speech-marks-rejected, step task#4 (cluster also covers task#5), operation SynthesizeSpeech, verdict mismatch:
  status: expected 400, got 200
  $: type_mismatch — expected {'message': 'The selected speech mark type - word - is not supported for this engine: generative'}, got the mark stream '{"time":0,"type":"word","value":"Marks","start":0,"end":5}\n{"time":500,...}\n...'
  $headers.x-amzn-errortype: missing (expected a shaped error type header, got None)
  $headers.content-type: expected 'application/json', got 'application/x-json-stream'

Cause 1 — fixture polly-generative-engine-speech-marks-rejected, step task#6, same operation, verdict mismatch:
  status: expected 400, got 200
  $.message: missing — expected 'The selected speech mark type - sentence - is not supported for this engine: generative', got None
  $headers.x-amzn-errortype: missing; $headers.content-type: expected 'application/json', got 'application/x-json-stream'

Cause 2 — fixture polly-list-speech-synthesis-tasks-filter-rejected, step task#3 (cluster also covers polly-list-speech-synthesis-tasks-paging-rejected#task#4), operation ListSpeechSynthesisTasks, verdict mismatch (status 400 matched):
  $.message: expected "1 validation error detected: Value 'finished' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [completed, scheduled, inProgress, failed]"
           got      "1 validation error detected: Value 'finished' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [scheduled, inProgress, completed, failed]"

Model facts used: smithy_query(query="operation", operation="SynthesizeSpeech,ListSpeechSynthesisTasks") and smithy_query(query="shape", shape="TaskStatus,SpeechMarkType,Engine") on services/service-com-amazonaws-polly-parrot-v1/smithy-model/com-amazonaws-polly-parrot-v1.json; handler doc read at services/service-com-amazonaws-polly-parrot-v1/src/ops/synthesize_speech.rs lines 1-90.

Not filed from this replay (already owned): the SynthesizeSpeech mp3 payload / word-mark timing divergences (polly-synthesize-speech-roundtrip#task#1-#3, polly-generative-engine-speech-marks-rejected#task#1) are owned by 20261030T071447Z-polly-synthesizespeech-output-stream-diverges-from-the-rendered.md, and the StartSpeechSynthesisTask OutputUri authority divergence (polly-speech-synthesis-task-lifecycle#task#2) by 20261121T155022Z-polly-startspeechsynthesistask-announces-an-outputuri-that-does-not.md.

## Steps to Reproduce
Boot the emulator from this checkout, then:

Cause 1: POST /v1/speech with Engine="generative", VoiceId="Ruth", OutputFormat="json", SpeechMarkTypes=["word"], TextType="text", Text="Marks follow each word". Expect HTTP 400, Content-Type application/json, an x-amzn-errortype header, and body {"message": "The selected speech mark type - word - is not supported for this engine: generative"}; the emulator returns 200, Content-Type application/x-json-stream and a word-mark stream. Repeat with SpeechMarkTypes=["sentence"] and expect the same 400 with "sentence" in the template. The identical request with Engine="neural" must still be accepted and return the mark stream.

Cause 2: GET /v1/synthesisTasks?Status=finished. Expect HTTP 400 with message "1 validation error detected: Value 'finished' at 'status' failed to satisfy constraint: Member must satisfy enum value set: [completed, scheduled, inProgress, failed]"; the emulator lists the set as [scheduled, inProgress, completed, failed]. Same for the paging variant (Status=finished alongside NextToken/MaxResults).

Re-replay these fixture steps to confirm the fix: polly-generative-engine-speech-marks-rejected#task#4, polly-generative-engine-speech-marks-rejected#task#6, polly-list-speech-synthesis-tasks-filter-rejected#task#3.
