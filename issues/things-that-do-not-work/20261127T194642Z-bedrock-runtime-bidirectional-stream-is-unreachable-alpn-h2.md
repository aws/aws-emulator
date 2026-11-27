Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: bedrock-runtime.bidirectional-stream-model-without-bidirectional-support-rejected bedrock-runtime.bidirectional-stream-prompt-start-before-session-start-rejected bedrock-runtime.bidirectional-stream-text-input-before-its-content-start-rejected bedrock-runtime.bidirectional-stream-text-turn-delivers-chunk-events bedrock-runtime.bidirectional-stream-unserved-model-rejected-before-stream-opens bedrock-runtime.bidirectional-stream-first-event-not-session-start-rejected bedrock-runtime.bidirectional-stream-input-gap-past-55-seconds-ends-with-validation-exception bedrock-runtime.bidirectional-stream-text-input-for-an-unopened-content-rejected-in-stream bedrock-runtime.async-invoke-output-retention-window
Issue-Shape-Id: com.amazonaws.bedrockruntime#AmazonBedrockFrontendService
Issue-Key: bedrock-runtime.bidirectional-stream-model-without-bidirectional-support-rejected|run#9|AssertionError|assert meta["error"] is None, f"the refused stream did not end cleanly: {meta!r}"
Issue-Key: bedrock-runtime.bidirectional-stream-prompt-start-before-session-start-rejected|__init__#4|AwsCrtError|).result(30)
Issue-Key: bedrock-runtime.bidirectional-stream-text-input-before-its-content-start-rejected|__init__#4|AwsCrtError|).result(30)
Issue-Key: bedrock-runtime.bidirectional-stream-text-turn-delivers-chunk-events|__init__#2|AwsCrtError|).result(30)
Issue-Key: bedrock-runtime.bidirectional-stream-unserved-model-rejected-before-stream-opens|_open_session#1|AwsCrtError|).result(_WAIT_S)
Issue-Key: bedrock-runtime.bidirectional-stream-first-event-not-session-start-rejected|run#3|AssertionError|raise AssertionError(f"Converse served the speech model: {answer.get('stopReason')!r}")
Issue-Key: bedrock-runtime.bidirectional-stream-input-gap-past-55-seconds-ends-with-validation-exception|_unary_refusal#2|AssertionError|raise AssertionError(f"a unary call to the speech-to-speech model succeeded: {answer!r}")
Issue-Key: bedrock-runtime.bidirectional-stream-text-input-for-an-unopened-content-rejected-in-stream|_unary_refusal#2|AssertionError|raise AssertionError(f"a unary call to the speech-to-speech model succeeded: {answer!r}")
Issue-Key: bedrock-runtime.async-invoke-output-retention-window|run#10|Timeout|time.sleep(2)
Issue-Commit: 723e24d0ff9083736505f1fa49c337be40d1d9c1

# bedrock-runtime bidirectional stream is unreachable (ALPN h2 refused), unary inference serves speech-to-speech models it must refuse, and a Completed async invoke is never reached (3 causes)

> _Note: this issue was identified against the code base at commit `723e24d0ff9083736505f1fa49c337be40d1d9c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
InvokeModelWithBidirectionalStream

## Description
Three distinct, independently fixable defects. A fixer must close all three before submitting.

=== Cause 1 of 3: the HTTPS front door does not negotiate h2, so InvokeModelWithBidirectionalStream can never be reached ===
AWS: InvokeModelWithBidirectionalStream is POST /model/{modelId}/invoke-with-bidirectional-stream with a streaming httpPayload request body AND a streaming httpPayload response body (Smithy: input body -> InvokeModelWithBidirectionalStreamInput, output body -> InvokeModelWithBidirectionalStreamOutput, both required+streaming). A full-duplex event stream is only expressible over HTTP/2, so the endpoint must advertise "h2" in the TLS ALPN list and complete the handshake for an h2 client.
Emulator: the TLS listener does not offer h2 and sends a fatal ALPN alert. Clients using the AWS CRT see "AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE: TLS (SSL) negotiation failed"; clients using OpenSSL see "tlsv1 alert no application protocol ... SSL alert number 120" and the stream is cancelled with status None and no headers. No request ever reaches the operation, so every bidirectional behavior — the SessionStart-first ordering rule, the PromptStart ordering rule, the ContentStart-before-TextInput rule, chunk delivery for a text turn, and the pre-open rejection of an unserved model — is unobservable.
Why: a client cannot open the operation at all. It gets a transport/TLS failure instead of either a working duplex stream or the modeled service error, which is indistinguishable from a broken network and is not retryable into success.
NOTE: the open issue 20261118T165638Z-bedrock-runtime-the-https-front-door-refuses-h2.md describes the same ALPN refusal for ConverseStream's HTTP/2 data plane; it does not carry these bidirectional replay keys and did not own these checks. Fixing the ALPN list should resolve the transport failure here; the per-event ordering rules behind it still need to be verified once the stream opens.
Fix: the HTTPS front door's TLS configuration (add "h2" to the advertised ALPN protocols and serve HTTP/2 on the bedrock-runtime endpoint), then the InvokeModelWithBidirectionalStream handler.
Traces: bedrock-runtime.bidirectional-stream-model-without-bidirectional-support-rejected, bedrock-runtime.bidirectional-stream-prompt-start-before-session-start-rejected, bedrock-runtime.bidirectional-stream-text-input-before-its-content-start-rejected, bedrock-runtime.bidirectional-stream-text-turn-delivers-chunk-events, bedrock-runtime.bidirectional-stream-unserved-model-rejected-before-stream-opens

=== Cause 2 of 3: Converse and InvokeModel serve a speech-to-speech model instead of refusing it as unsupported ===
AWS: a speech-to-speech model served only on the bidirectional stream (e.g. amazon.nova-2-sonic-v1:0) is not a text or chat model, so the unary entry points refuse it rather than generating a turn. Converse / ConverseStream / InvokeModel answer HTTP 400 ValidationException with the fixed message "This action doesn't support the model that you provided. Try again with a supported text or chat model." Smithy: ValidationException is modeled on these operations at httpError 400, and ConverseStream's modelId targets ConversationalModelId — a non-conversational model is outside what the action accepts. The per-model capability set is the service's, so recorded behavior only for which models fall in this class.
Emulator: the calls succeed with HTTP 200. Converse returned stopReason 'end_turn' (and 'max_tokens' in the other check) with output text "No language model runs in this emulator. amazon.nova-2-sonic-v1:0 generated no text for this request. The request carried 1 message(s) and 2 input token(s)." and usage/metrics; InvokeModel returned 200 with contentType application/json and a body stream. The emulator applies no model-capability gate on the unary path and treats every model id as a chat model.
Why: a client that points a conversational call at a speech model gets a fabricated text answer instead of the 400 that tells it the model is wrong for the action. The mistake is invisible in the emulator and fails only in production.
Fix: model-capability resolution in the Converse / ConverseStream / InvokeModel handlers — reject a model whose modality does not support the action before generation.
Traces: bedrock-runtime.bidirectional-stream-first-event-not-session-start-rejected, bedrock-runtime.bidirectional-stream-input-gap-past-55-seconds-ends-with-validation-exception, bedrock-runtime.bidirectional-stream-text-input-for-an-unopened-content-rejected-in-stream

=== Cause 3 of 3: an async invoke never becomes resolvable as Completed with a stable S3 output, so the retention window cannot be observed ===
AWS: StartAsyncInvoke accepts a job, the job advances to a terminal status, and a Completed record stays resolvable through GetAsyncInvoke with an unchanged outputDataConfig.s3OutputDataConfig.s3Uri while its artifact remains present under that prefix in S3. The suite only asserts a lower bound (the record and artifact survive ~60s past completion); retention itself is undocumented, so this is recorded behavior only.
Emulator: the check never reached its observation window — it ran out of its 60s budget while polling, i.e. the invoke did not become resolvable as Completed with an artifact in time. This is the same symptom the open issue 20261024T094806Z-bedrock-runtime-async-invoke-startasyncinvoke-over-validates-modelinput.md records as "jobs never reach Completed"; that issue does not carry this replay key and did not own this check, so it is filed here. If the fix for that issue makes jobs terminal, re-replay this check to confirm the post-completion window as well.
Why: a client that submits an async invoke and polls for completion never finishes, and the output artifact it was promised never appears — the whole async workflow is unusable.
Fix: the async-invoke job state machine (status progression to Completed) and the output-artifact writer behind GetAsyncInvoke/ListAsyncInvokes.
Traces: bedrock-runtime.async-invoke-output-retention-window

## Evidence
Cause 1 — expected: a POST to /model/{modelId}/invoke-with-bidirectional-stream over HTTP/2 opens a duplex event stream (or returns a modeled ValidationException/ResourceNotFoundException for an unserved model).
Actual, awscrt clients: "awscrt.exceptions.AwsCrtError: AWS_IO_TLS_ERROR_NEGOTIATION_FAILURE: TLS (SSL) negotiation failed".
Actual, OpenSSL client: {'status': None, 'headers': {}, 'error': 'stream: The pending stream has been canceled (caused by: error:0A000460:SSL routines:ssl3_read_bytes:tlsv1 alert no application protocol:ssl/record/rec_layer_s3.c:918:SSL alert number 120)'} — a fatal ALPN no_application_protocol alert (120); the emulator never offered h2.

Cause 2 — expected: HTTP 400 ValidationException "This action doesn't support the model that you provided. Try again with a supported text or chat model." for modelId amazon.nova-2-sonic-v1:0.
Actual (Converse): HTTPStatusCode 200, {'output': {'message': {'role': 'assistant', 'content': [{'text': 'No language model runs in this emulator. amazon.nova-2-sonic-v1:0 generated no text for this request. The request carried 1 message(s) and 2 input token(s).'}]}}, 'stopReason': 'end_turn', 'usage': {...}} — and stopReason 'max_tokens' in the other Converse check.
Actual (InvokeModel): HTTPStatusCode 200, contentType 'application/json', performanceConfigLatency 'standard', a StreamingBody payload.

Cause 3 — expected: StartAsyncInvoke -> status Completed, then repeated GetAsyncInvoke over ~60s returning status Completed with an unchanged s3Uri and the artifact still present under that S3 prefix.
Actual: "Check timed out after 60.0s" while polling for the terminal Completed status — the job never became resolvable as Completed with an output artifact.

## Steps to Reproduce
1. Cause 1: sign a SigV4 POST to https://<bedrock-runtime endpoint>/model/amazon.nova-2-sonic-v1%3A0/invoke-with-bidirectional-stream with content-type application/vnd.amazon.eventstream and open it with an HTTP/2 client (awscrt http, or node:http2). The TLS handshake fails: no "h2" in the server's ALPN list, fatal alert 120. Repeat with modelId amazon.nova-lite-v1:0 (a model that must be refused on this operation) — the same transport failure hides the refusal.
2. Cause 2: bedrock-runtime converse(modelId='amazon.nova-2-sonic-v1:0', messages=[{'role':'user','content':[{'text':'sim-prompt'}]}]) — expect HTTP 400 ValidationException "This action doesn't support the model that you provided. Try again with a supported text or chat model.", observe HTTP 200 with generated text. Repeat with invoke_model(modelId='amazon.nova-2-sonic-v1:0', ...) — same.
3. Cause 3: create an S3 output bucket; start_async_invoke(modelId='luma.ray-v2:0', modelInput=<video request>, outputDataConfig={'s3OutputDataConfig':{'s3Uri':'s3://<bucket>/<prefix>'}}); poll get_async_invoke(invocationArn=<arn>) for a terminal status, then re-read it and list the S3 prefix every 2s for 60s. The job never reports Completed with an artifact.
Replay the nine check ids in trace_ids to confirm.
