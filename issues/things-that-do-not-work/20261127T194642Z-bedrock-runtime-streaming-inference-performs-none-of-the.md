Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.converse-stream-top-k-past-its-range-rejected-before-the-stream-opens bedrock-runtime.invoke-model-stream-top-k-past-its-range-rejected-before-the-stream-opens bedrock-runtime.invoke-model-response-stream-max-new-tokens-outside-range-rejected bedrock-runtime.invoke-model-with-response-stream-schema-violating-body-rejected bedrock-runtime.invoke-model-response-stream-mislabeled-image-bytes-rejected-in-stream bedrock-runtime.converse-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected bedrock-runtime.invoke-model-response-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected bedrock-runtime.invoke-model-with-response-stream-trace-value-outside-enum-rejected bedrock-runtime.invoke-model-with-response-stream-unknown-guardrail-identifier-rejected bedrock-runtime.invoke-model-with-response-stream-stop-sequence-cuts-the-stream
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.converse-stream-top-k-past-its-range-rejected-before-the-stream-opens|_refusal_at_open#5|AssertionError|raise AssertionError(f"the request was accepted and streamed events {names!r}")
Issue-Key: bedrock-runtime.invoke-model-stream-top-k-past-its-range-rejected-before-the-stream-opens|_refused_before_stream#6|AssertionError|raise AssertionError(f"the request was accepted and streamed {len(got)} chunk(s): {got!r}")
Issue-Key: bedrock-runtime.invoke-model-response-stream-max-new-tokens-outside-range-rejected|_refusal#13|AssertionError|raise AssertionError(f"max_new_tokens={max_new_tokens} opened a stream instead of being refused")
Issue-Key: bedrock-runtime.invoke-model-with-response-stream-schema-violating-body-rejected|_refusal#3|AssertionError|raise AssertionError(f"expected the streaming invocation to be refused; it opened: {answer!r}")
Issue-Key: bedrock-runtime.invoke-model-response-stream-mislabeled-image-bytes-rejected-in-stream|run#9|AssertionError|assert error == ("validationException", _MISMATCH), (
Issue-Key: bedrock-runtime.converse-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected|_refusal#9|AssertionError|raise AssertionError("the call and its whole stream completed without an error")
Issue-Key: bedrock-runtime.invoke-model-response-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected|run#4|AssertionError|assert stops == ["tool_use"], f"served chunks' messageStop reasons: {stops!r}"
Issue-Key: bedrock-runtime.invoke-model-with-response-stream-trace-value-outside-enum-rejected|run#4|AssertionError|assert refusal == expected, f"trace {NOT_A_MODE!r} answered {refusal!r}"
Issue-Key: bedrock-runtime.invoke-model-with-response-stream-unknown-guardrail-identifier-rejected|run#3|AssertionError|assert bare == ("ValidationException", INVALID_GUARDRAIL), (
Issue-Key: bedrock-runtime.invoke-model-with-response-stream-stop-sequence-cuts-the-stream|run#4|AssertionError|assert at >= 0, f"the unbounded request never produced {_STOP!r}: {full!r}"
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# bedrock-runtime streaming inference performs none of the provider-level request validation and never surfaces in-stream errors (8 causes)

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModelWithResponseStream

## Description
Eight distinct, independently fixable defects in the bedrock-runtime streaming data plane (ConverseStream and InvokeModelWithResponseStream). Every one of them is a case where the emulator answers HTTP 200 and streams a complete, well-formed turn for a request real Bedrock refuses, or where it omits an error Bedrock delivers inside the event stream. A fixer must close all eight before submitting.

Related open issues that do NOT cover these: 20261023T232022Z-bedrock-runtime-synchronous-inference-guardrailconfig-and-toolconfig-silently.md (synchronous InvokeModel/Converse only) and 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md (the constant notice text). Cause 6 and Cause 8 below touch generation, and are called out as such, but they are request-validation / stream-framing requirements that the generation issue does not state.

=== Cause 1 of 8: topK past its range is accepted on both streaming entry points ===
AWS: topK reaches the provider through ConverseStream.additionalModelRequestFields (an untyped Document in the model) and through the native body of InvokeModelWithResponseStream. For Amazon Nova the provider bounds topK to 1..128 and refuses a larger value at request time, BEFORE any event stream opens, with HTTP 400 ValidationException. Smithy: ConverseStream and InvokeModelWithResponseStream both model ValidationException (errorKind client, httpError 400); the range itself is the provider's, so recorded behavior only. The recorded message is "The model returned the following errors: Malformed input request: the provided topK must be between 1 and <max>, please reformat your input and try again." — everything outside <max> is fixed text; <max> is the named model's topK ceiling (128 for amazon.nova-lite-v1:0), taken from the model, not hard-coded.
Emulator: both calls return HTTP 200 and stream a full turn. ConverseStream streamed ['messageStart', 'contentBlockDelta', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata']; InvokeModelWithResponseStream streamed 5 chunks ending in messageStop stopReason 'end_turn' plus a metadata chunk.
Why: a client that sends an out-of-range tuning parameter gets a plausible answer instead of the 400 that tells it the parameter was wrong, so bad inference configuration ships to production unnoticed and the client's error path is never exercised.
Fix: the provider-level inference-config validation shared by the ConverseStream and InvokeModelWithResponseStream handlers, run before the stream is opened.
Traces: bedrock-runtime.converse-stream-top-k-past-its-range-rejected-before-the-stream-opens, bedrock-runtime.invoke-model-stream-top-k-past-its-range-rejected-before-the-stream-opens

=== Cause 2 of 8: maxTokens/max_new_tokens outside its range is accepted ===
AWS: the native Nova body's inferenceConfig token budget (max_new_tokens / maxTokens) is bounded to 1..10240; 0 is refused before the stream opens with HTTP 400 ValidationException whose message is "Malformed input request: the provided maxTokens must be between 1 and <ceiling>, please reformat your input and try again." — fixed text, with <ceiling> the model's own token ceiling (10240 for amazon.nova-lite-v1:0). Smithy: ValidationException, httpError 400; the numeric bound is the provider's (recorded behavior only).
Emulator: max_new_tokens=0 opened a stream instead of being refused.
Why: a zero or negative token budget is a programming error that AWS catches; the emulator invents a budget and answers, so the client never learns its config is invalid.
Fix: native-body inference-config validation in the InvokeModelWithResponseStream handler (same validation stage as Cause 1).
Traces: bedrock-runtime.invoke-model-response-stream-max-new-tokens-outside-range-rejected

=== Cause 3 of 8: a schema-violating native body is accepted instead of refused with the offending JSON path ===
AWS: InvokeModelWithResponseStream validates the native body against the model's request schema before opening the stream and names the offending JSON path. A body whose "messages" member is a String rather than an array draws HTTP 400 ValidationException with message "Malformed input request: #/messages: expected type: JSONArray, found: String, please reformat your input and try again." The fixed text is "Malformed input request: ", ": expected type: ", ", found: " and the trailing ", please reformat your input and try again."; the variable parts are the JSON pointer of the offending member (#/messages), the expected JSON type and the found JSON type, all derived from the request and the model's schema. Smithy: body is an httpPayload blob with no structure, so the schema is the provider's — recorded behavior only; ValidationException is modeled at httpError 400.
Emulator: HTTP 200 with content-type application/vnd.amazon.eventstream and an opened EventStream (x-amzn-bedrock-content-type application/json, content-length 1376) — the malformed body was swallowed and a turn was generated.
Why: clients debug native-body shape errors from this message. Accepting the body makes a schema bug look like a working call, and masks the one diagnostic that locates it.
Fix: native-body schema validation in the InvokeModelWithResponseStream handler, before the response stream is opened.
Traces: bedrock-runtime.invoke-model-with-response-stream-schema-violating-body-rejected

=== Cause 4 of 8: mislabeled image bytes never produce the in-stream validationException ===
AWS: when a content block declares an image format but carries bytes of another type, Bedrock sniffs the real MIME type and ends the HTTP 200 event stream with an exception frame whose :exception-type is validationException and whose payload message is "The detected file MIME type <detected> does not match the expected type <declared>. Reformat your input and try again." Fixed text is "The detected file MIME type ", " does not match the expected type " and ". Reformat your input and try again."; <detected> is the MIME type sniffed from the supplied bytes (text/plain for ASCII text) and <declared> is the MIME type implied by the declared image format (image/png for format "png"). Recorded behavior only — the model types image bytes as an opaque blob.
Emulator: the stream carried no error at all; the check observed None where it expected ('validationException', <that message>).
Why: a client that uploads the wrong bytes under an image label gets a generated answer about an image that was never decoded, instead of the error that identifies the mislabeling.
Fix: content-block media validation in the InvokeModelWithResponseStream handler, with the failure serialized as an event-stream exception frame rather than an HTTP error.
Traces: bedrock-runtime.invoke-model-response-stream-mislabeled-image-bytes-rejected-in-stream

=== Cause 5 of 8: an unfinishable tool turn completes cleanly instead of ending in modelStreamErrorException ===
AWS: when a tool turn cannot be completed within the token budget, the stream does not finish — it ends with an event-stream exception frame, inside an HTTP 200 response, whose :message-type is "exception", whose :exception-type is "modelStreamErrorException" and whose payload message is the fixed text "Model produced invalid sequence as part of ToolUse. Please refer to the model tool use troubleshooting guide." No messageStop and no metadata frame precede it, and nothing follows it. Smithy: ModelStreamErrorException is modeled on InvokeModelWithResponseStream (errorKind client, httpError 424) and is carried as an event-stream exception member, so the transport status stays 200.
Emulator: the call and its whole stream completed without any error.
Why: a client cannot distinguish a tool turn that failed from one that succeeded; it will treat a truncated or absent toolUse block as a valid model answer and act on it. It also never exercises its retry path for a mid-stream model failure.
Fix: the ConverseStream handler's tool-turn generation path and its event-stream error serializer (exception frames, not HTTP errors).
Traces: bedrock-runtime.converse-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected

=== Cause 6 of 8: a forced tool turn reports stopReason end_turn instead of tool_use ===
AWS: a turn whose toolConfig forces the model to call a tool ends with a messageStop whose stopReason is "tool_use" (and, when the turn cannot be finished, the modelStreamErrorException of Cause 5 instead). stopReason is a modeled enum member of the stream's messageStop event; "tool_use" is its value for a tool invocation. Recorded behavior for the mapping of a forced tool turn onto that value.
Emulator: the served chunks' messageStop reasons were ['end_turn'].
Why: the stopReason is how a client knows it must execute a tool and send a toolResult back; reporting end_turn silently terminates every agent loop after the first turn. (Partly adjacent to 20261028T194707Z-bedrock-runtime-generation-stage-serves-a-constant-emulator.md, which does not state the stopReason requirement.)
Fix: stopReason selection in the InvokeModelWithResponseStream streaming-generation stage.
Traces: bedrock-runtime.invoke-model-response-stream-unfinishable-tool-turn-ends-in-model-stream-error-rejected

=== Cause 7 of 8: the trace enum-constraint message lists the enum values in the wrong order ===
AWS: an X-Amzn-Bedrock-Trace header value outside the Trace enum draws HTTP 400 ValidationException with the standard AWS constraint message "1 validation error detected: Value '<value>' at 'trace' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, ENABLED_FULL, DISABLED]". The fixed text is the whole sentence including the bracketed value set in that order; the only variable part is <value>, echoed from the request, and 'trace' which names the member. Note the Smithy model lists Trace as [ENABLED, DISABLED, ENABLED_FULL] — the recorded AWS message uses [ENABLED, ENABLED_FULL, DISABLED], so the recorded order is the requirement, not the model's declaration order.
Emulator: right code and status, wrong message — ValidationException "1 validation error detected: Value 'VERBOSE' at 'trace' failed to satisfy constraint: Member must satisfy enum value set: [ENABLED, DISABLED, ENABLED_FULL]".
Why: clients and test suites that assert on the constraint message, and humans reading it, see a value set AWS never emits.
Fix: the enum-constraint message formatter used by the InvokeModelWithResponseStream input validation (emit AWS's recorded value-set order).
Traces: bedrock-runtime.invoke-model-with-response-stream-trace-value-outside-enum-rejected

=== Cause 8 of 8: an unknown guardrail identifier draws the wrong ValidationException message ===
AWS: a streaming invoke naming a guardrail identifier that matches the GuardrailIdentifier pattern but that the account does not hold is refused with HTTP 400 ValidationException and the fixed message "The provided guardrail identifier is invalid." — identical with and without guardrailVersion, and the stream never opens. Smithy: guardrailIdentifier is an httpHeader (X-Amzn-Bedrock-GuardrailIdentifier) with pattern ^(|([a-z0-9]+)|(arn:...:guardrail/[a-z0-9]+))$, so a syntactically valid but absent id is a lookup failure, not a pattern failure; ValidationException is modeled at httpError 400.
Emulator: ValidationException with the unrelated message 'Guardrail was enabled but input is in incorrect format.' for both the bare and the versioned call.
Why: the emulator blames the request body for a missing resource. A client cannot tell "this guardrail does not exist" from "my input is malformed", so it retries a request that can never succeed instead of creating or correcting the guardrail.
Fix: guardrail resolution in the InvokeModelWithResponseStream handler (look the identifier up in guardrail state and emit the invalid-identifier message on a miss).
Traces: bedrock-runtime.invoke-model-with-response-stream-unknown-guardrail-identifier-rejected

## Evidence
Cause 1 — expected HTTP 400 ValidationException "The model returned the following errors: Malformed input request: the provided topK must be between 1 and 128, please reformat your input and try again." before any frame.
Actual (ConverseStream): 200, events ['messageStart', 'contentBlockDelta', 'contentBlockDelta', 'contentBlockStop', 'messageStop', 'metadata'].
Actual (InvokeModelWithResponseStream): 200, 5 chunks: [{'messageStart': {'role': 'assistant'}}, {'contentBlockDelta': {'contentBlockIndex': 0, 'delta': {'text': 'No language model runs in this emulator.'}}}, {'contentBlockStop': {'contentBlockIndex': 0}}, {'messageStop': {'stopReason': 'end_turn'}}, {'metadata': {...}}].

Cause 2 — expected HTTP 400 ValidationException "Malformed input request: the provided maxTokens must be between 1 and 10240, please reformat your input and try again."; actual: "max_new_tokens=0 opened a stream instead of being refused".

Cause 3 — expected ('ValidationException', 'Malformed input request: #/messages: expected type: JSONArray, found: String, please reformat your input and try again.') with HTTP 400; actual: HTTP 200, content-type application/vnd.amazon.eventstream, x-amzn-bedrock-content-type application/json, an opened EventStream.

Cause 4 — expected the stream to end with ('validationException', 'The detected file MIME type text/plain does not match the expected type image/png. Reformat your input and try again.'); actual: None (no error frame in the stream).

Cause 5 — expected ('modelStreamErrorException', 'Model produced invalid sequence as part of ToolUse. Please refer to the model tool use troubleshooting guide.') as the final event-stream exception frame under HTTP 200, with no preceding messageStop/metadata; actual: "the call and its whole stream completed without an error".

Cause 6 — expected messageStop stopReason ['tool_use']; actual ['end_turn'].

Cause 7 — expected message "... Member must satisfy enum value set: [ENABLED, ENABLED_FULL, DISABLED]"; actual "... Member must satisfy enum value set: [ENABLED, DISABLED, ENABLED_FULL]" (code ValidationException correct).

Cause 8 — expected ('ValidationException', 'The provided guardrail identifier is invalid.') for both the bare and the version-1 call; actual ('ValidationException', 'Guardrail was enabled but input is in incorrect format.').

## Steps to Reproduce
All against bedrock-runtime with model amazon.nova-lite-v1:0 unless noted.
1. Cause 1a: converse_stream(modelId=MODEL, messages=[{'role':'user','content':[{'text':'Answer with one word: what colour is the sky?'}]}], inferenceConfig={'maxTokens':16,'temperature':0.0}, additionalModelRequestFields={'inferenceConfig':{'topK':129}}) — expect ValidationException at open, observe a full stream.
2. Cause 1b: invoke_model_with_response_stream(modelId=MODEL, contentType='application/json', accept='application/json', body=json with inferenceConfig {'maxTokens':16,'temperature':0.0,'topK':129}) — same.
3. Cause 2: invoke_model_with_response_stream with body inferenceConfig {'max_new_tokens': 0}.
4. Cause 3: invoke_model_with_response_stream with body {'inferenceConfig':{'maxTokens':8,'temperature':0}, 'messages':'Name a colour.'} (messages as a String).
5. Cause 4: invoke_model_with_response_stream with an image content block declaring format 'png' whose bytes are ASCII text (b'plain words, not pixels').
6. Cause 5: converse_stream(modelId=MODEL, messages=<ask>, toolConfig=<one find_station tool>, inferenceConfig={'maxTokens':1}) and drain the stream; inspect the raw event-stream frames.
7. Cause 6: the same forced tool turn over invoke_model_with_response_stream with a workable budget; read the messageStop stopReason.
8. Cause 7: invoke_model_with_response_stream(..., trace='VERBOSE').
9. Cause 8: invoke_model_with_response_stream(..., guardrailIdentifier='absentguardrail0') and again with guardrailVersion='1'.
Replay the ten check ids in trace_ids to confirm.
