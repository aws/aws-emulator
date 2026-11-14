Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.converse-malformed-message-structure-rejected bedrock-runtime.invoke-model-parameter-out-of-range-rejected bedrock-runtime.oversized-input-payload-rejected-not-truncated bedrock-runtime.start-async-invoke-invalid-model-rejected
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.converse-malformed-message-structure-rejected|_error#3|AssertionError|raise AssertionError(f"expected Converse to be refused, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-runtime.invoke-model-parameter-out-of-range-rejected|_error#3|AssertionError|raise AssertionError(f"real AWS rejects this call, but it SUCCEEDED and returned {result!r}")
Issue-Key: bedrock-runtime.oversized-input-payload-rejected-not-truncated|_error_code#7|AssertionError|raise AssertionError(
Issue-Key: bedrock-runtime.start-async-invoke-invalid-model-rejected|run#15|AssertionError|assert control["status"] == "Completed", (
Issue-Commit: 275bab82189ff4a1ebabf88d78dc96a921701e7c

# bedrock-runtime synchronous and async inference input validation is over-permissive: empty Message.content, out-of-range temperature and context-window-exceeding payloads are all accepted with 200, and async jobs still never reach Completed

> _Note: this issue was identified against the code base at commit `275bab82189ff4a1ebabf88d78dc96a921701e7c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Converse

## Description
This issue bundles 4 DISTINCT root causes on the bedrock-runtime inference surface (Converse, InvokeModel, StartAsyncInvoke/GetAsyncInvoke). A fixer must close all 4 before submitting; each cause below names its own failing check ids.

Smithy consulted for all four: services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json. Converse POST /model/{modelId}/converse -> 200; InvokeModel POST /model/{modelId}/invoke -> 200; both model ValidationException (errorKind client, httpError 400). StartAsyncInvoke POST /async-invoke -> 200, GetAsyncInvoke.status is the enum AsyncInvokeStatus = InProgress|Completed|Failed.

None of the four causes below is an assertion on generated model text: each is a status/error-class/state-machine contract that holds whatever the model would have written. (Separately, 19 other failing checks in this run fail only because the emulator serves the placeholder completion "No language model runs in this emulator. <modelId> generated no text for this request..."; those are assertions on generated model text and are deliberately NOT filed here.)

---
Cause 1 of 4: Converse accepts a Message whose `content` list is EMPTY and forwards the turn to the model, instead of refusing it with ValidationException.
AWS: Smithy Message.content -> ContentBlocks (list of the ContentBlock union) is @required but carries no @length min, so the non-empty rule is enforced semantically by the service rather than statically by the model — recorded behavior only for the exact text. Converse with messages=[{"role":"user","content":[]}] is refused with HTTP 400, Error.Code `ValidationException`, and the message 'The content field in the Message object at messages.<N> is empty. Add a ContentBlock object to the content field and try again.' The fixed text is 'The content field in the Message object at messages.' / ' is empty. Add a ContentBlock object to the content field and try again.'; the only variable part is the ZERO-BASED INDEX of the offending message in the request's `messages` list (0 for the first message), taken from the request — not a literal from any one recording.
Emulator: the call SUCCEEDS with HTTP 200, stopReason='end_turn', usage={'inputTokens': 0, 'outputTokens': 52, 'totalTokens': 52}, and an assistant text block. The empty message is treated as a valid turn and billed as a completion.
Why: a client that ships a structurally malformed conversation (an empty content list is the common symptom of a dropped/filtered content block) is served a successful-looking 200 here and gets a hard 400 in production, so the bug is only found against real AWS. It also makes the emulator unusable for validating conversation-assembly code, since absence of content and presence of content are made interchangeable.
Fix: Converse input validation in the Converse handler — reject a Message with an empty ContentBlocks list before the model-invocation stage, with the 400 ValidationException and the indexed member message above. The same rule applies to ConverseStream, which shares this input shape.
Traces: bedrock-runtime.converse-malformed-message-structure-rejected.

---
Cause 2 of 4: InvokeModel accepts an inference `temperature` far outside the model's documented range and serves a 200 instead of refusing it.
AWS: the InvokeModel `body` is an opaque blob (Smithy: Body, @httpPayload, @sensitive, @length max 25000000), so the per-model native schema is not in the model; the equivalent modeled constraint is InferenceConfiguration.temperature -> Float @range min 0 max 1 on Converse, which is the same [0.0, 1.0] contract the native body carries. Recorded behavior: InvokeModel on an Amazon Nova model with inferenceConfig.temperature=5.0 is refused with HTTP 400, Error.Code `ValidationException`, message 'Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.' That message is entirely fixed text for this error class (the bound values 0.0/1.0 come from the model's documented range, not from the request).
Emulator: the call SUCCEEDS with HTTP 200 (headers content-type application/json, x-amzn-bedrock-performanceconfig-latency: standard) and returns a body. The out-of-range value is neither rejected nor clamped — it is simply ignored.
Why: real AWS refuses the request, so a caller that builds temperature from user input or config discovers the 400 only in production; a caller that intends to probe the valid range gets no signal at all. Combined with Cause 1 this means the native InvokeModel body is not validated against the model's inference-parameter contract anywhere.
Fix: the InvokeModel handler's native-body validation stage — apply the provider's inference-parameter ranges (temperature/topP in [0,1], maxTokens >= 1) to the decoded body and answer 400 ValidationException with the message above. The same range check belongs on Converse's modeled InferenceConfiguration, where the @range trait states it.
Traces: bedrock-runtime.invoke-model-parameter-out-of-range-rejected.

---
Cause 3 of 4: an input far beyond every model's context window is accepted rather than refused — the emulator neither rejects nor reports truncation.
AWS: recorded behavior only (the token limit is a per-model property, not a modeled constraint; the only modeled size bound is InvokeModel.body @length max 25000000, which this ~4 MB payload does not reach, so the refusal is token-count based and not a payload-size refusal). InvokeModel with a prompt whose token count exceeds the model's context window is refused with HTTP 400, Error.Code `ValidationException`, message 'Input Tokens Exceeded: Number of input tokens exceeds maximum length. Please update the input to try again.' Converse refuses the equivalent input with the same code and the message 'The model returned the following errors: ' + that same sentence. Both messages are fixed text for this error class.
Emulator: the oversized InvokeModel call SUCCEEDS (no ClientError raised at all) and a normal response is returned; the request is silently accepted rather than refused. The in-limit positive control in the same check is served, so this is not an outage — it is a missing limit.
Why: this is the over-permissive case the operation's own contract exists to prevent. A client whose prompt assembly has no size guard is served here and fails in production; worse, because nothing in the response says the input was shortened, a caller cannot distinguish "the model saw my whole prompt" from "the service silently dropped most of it", which is precisely the data-loss mode the refusal prevents.
Fix: the InvokeModel and Converse handlers' input-size/token accounting stage — count the request's input tokens against the resolved model's context window before serving, and answer 400 ValidationException with the per-operation message above. A truncate-and-serve path must not exist.
Traces: bedrock-runtime.oversized-input-payload-rejected-not-truncated.

---
Cause 4 of 4: StartAsyncInvoke jobs still never reach Completed — a well-formed async invocation goes straight to terminal Failed. SAME ROOT CAUSE as Cause 4 of the open issue 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md, which does NOT carry this check's replay key; this entry exists to add that key, not to describe a new defect. A fixer closing that issue's Cause 4 should re-replay this check too.
AWS: GetAsyncInvoke.status is the modeled enum AsyncInvokeStatus = InProgress|Completed|Failed. An accepted async video job (here amazon.nova-reel-v1:0 in us-east-1, which is where ListFoundationModels offers the reel models) transitions InProgress -> Completed and the invocation's artifacts are written under the configured outputDataConfig.s3OutputDataConfig.s3Uri; failureMessage is absent on the success path. The artifact's bytes are model output and are NOT the requirement — the terminal status value and the existence of the S3 write are.
Emulator: the job is minted, then goes terminal with status='Failed' and failureMessage='This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.' Nothing is written to the configured prefix.
Why: Completed is unreachable, so any check or client that uses an accepted async job as a control — as this one does, after the emulator correctly answered the negative leg (bogus modelId -> 400 ValidationException 'The provided model identifier is invalid.') — cannot attribute its real assertion, and the whole async state machine stays unobservable.
Fix: the async-invoke job state machine and its output writer (the worker that advances a job and writes to outputDataConfig). A synthetic artifact plus a Completed transition is sufficient.
Traces: bedrock-runtime.start-async-invoke-invalid-model-rejected.

## Evidence
Behavioral suite simparity-behavioral-ga-prod-v1. bedrock-runtime answered every one of these calls (200s and modeled 4xx were both returned), so the service is up and answering wrongly, not unbootstrapped.

Cause 1 — bedrock-runtime.converse-malformed-message-structure-rejected
  Request: Converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[]}])
  Expected (AWS): HTTP 400, Error.Code='ValidationException', Error.Message='The content field in the Message object at messages.0 is empty. Add a ContentBlock object to the content field and try again.' (index 0 is taken from the request).
  Actual: AssertionError: expected Converse to be refused, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'content-length': '331', ...}}, 'output': {'message': {'role': 'assistant', 'content': [{'text': 'No language model runs in this emulator. ...'}]}}, 'stopReason': 'end_turn', 'usage': {'inputTokens': 0, 'outputTokens': 52, 'totalTokens': 52}, 'metrics': {'latencyMs': 1}}

Cause 2 — bedrock-runtime.invoke-model-parameter-out-of-range-rejected
  Request: InvokeModel(modelId='amazon.nova-lite-v1:0', contentType='application/json', body={'messages':[{'role':'user','content':[{'text':'hi'}]}],'inferenceConfig':{'maxTokens':10,'temperature':5.0}})
  Expected (AWS): HTTP 400, Error.Code='ValidationException', Error.Message='Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.'
  Actual: AssertionError: real AWS rejects this call, but it SUCCEEDED and returned {'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/json', 'x-amzn-bedrock-performanceconfig-latency': 'standard', 'content-length': '364', ...}}, 'contentType': 'application/json', 'performanceConfigLatency': 'standard', 'body': <StreamingBody>}

Cause 3 — bedrock-runtime.oversized-input-payload-rejected-not-truncated
  Request: InvokeModel(modelId='amazon.nova-lite-v1:0', body=Nova native body whose single text block is 'lorem ipsum dolor sit amet ' repeated 150000 times — far beyond any model's context window and ~4 MB, i.e. well under the modeled body @length max 25000000)
  Expected (AWS): HTTP 400, Error.Code='ValidationException', Error.Message='Input Tokens Exceeded: Number of input tokens exceeds maximum length. Please update the input to try again.'; the same input via Converse -> 'The model returned the following errors: Input Tokens Exceeded: Number of input tokens exceeds maximum length. Please update the input to try again.'
  Actual: AssertionError: expected the oversized call to be rejected, but it SUCCEEDED and returned None (no ClientError raised; the in-limit control in the same check was served normally, so the service is up)

Cause 4 — bedrock-runtime.start-async-invoke-invalid-model-rejected
  The negative leg PASSED: StartAsyncInvoke(modelId='amazon.definitely-not-a-real-model-v1:0') was refused 400 ValidationException 'The provided model identifier is invalid.' and wrote no S3 output.
  The positive control failed: StartAsyncInvoke(modelId='amazon.nova-reel-v1:0', TEXT_VIDEO modelInput, s3OutputDataConfig=s3://<bucket>/output/) then GetAsyncInvoke polled to a terminal state.
  Expected (AWS): status='Completed' with artifacts under the configured s3Uri and no failureMessage.
  Actual: AssertionError: expected the control job to reach Completed, got Failed (This Amazon Bedrock Runtime emulator does not run model inference for asynchronous invocations, so no output was written to the configured S3 location.)

Model evidence: smithy_query query=operation Converse/InvokeModel/StartAsyncInvoke and query=shape Message/ContentBlock/ContentBlocks/InferenceConfiguration on services/service-com-amazonaws-bedrockruntime-amazonbedrockfrontendservice/smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json — Message.content -> ContentBlocks @required; InferenceConfiguration.temperature -> Float @range {min:0, max:1}; InferenceConfiguration.maxTokens -> Integer @range {min:1}; InvokeModel.body -> Body blob @httpPayload @sensitive @length {max:25000000}; ValidationException errorKind=client httpError=400 on Converse, InvokeModel and StartAsyncInvoke; AsyncInvokeStatus = InProgress|Completed|Failed.

## Steps to Reproduce
1. git_clone, then run_behavioral_suite so the emulator is built and booted.
2. Re-replay the four check ids in trace_ids.
3. Cause 1: bedrock-runtime.converse(modelId='amazon.nova-lite-v1:0', messages=[{'role':'user','content':[]}]). Expected 400 ValidationException naming 'messages.0'; observed HTTP 200 with stopReason='end_turn' and usage.inputTokens=0.
4. Cause 2: bedrock-runtime.invoke_model(modelId='amazon.nova-lite-v1:0', contentType='application/json', body=json({'messages':[{'role':'user','content':[{'text':'hi'}]}],'inferenceConfig':{'maxTokens':10,'temperature':5.0}})). Expected 400 ValidationException 'Malformed input request: the provided temperature must be between 0.0 and 1.0, please reformat your input and try again.'; observed HTTP 200. Repeat with temperature=0.5 to confirm the in-range control is unaffected.
5. Cause 3: build a Nova native body whose text block is 'lorem ipsum dolor sit amet ' * 150000 and call invoke_model with it; then call converse with the same text as a single content block. Expected 400 ValidationException ('Input Tokens Exceeded: ...' for InvokeModel, 'The model returned the following errors: Input Tokens Exceeded: ...' for Converse); observed both accepted.
6. Cause 4: in us-east-1, s3.create_bucket(<out-bucket>), then start_async_invoke(modelId='amazon.nova-reel-v1:0', modelInput={'taskType':'TEXT_VIDEO','textToVideoParams':{'text':'...'},'videoGenerationConfig':{'durationSeconds':6,'fps':24,'dimension':'1280x720','seed':42}}, outputDataConfig={'s3OutputDataConfig':{'s3Uri':'s3://<out-bucket>/output/'}}), and poll get_async_invoke to a terminal state. Expected status='Completed' and objects under the prefix; observed status='Failed' with the emulator's failureMessage and zero objects. (Same cause as Cause 4 of 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md.)
