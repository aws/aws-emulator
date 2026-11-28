Issue-Source: trace-sweep-divergence
Issue-Trace: qconnect-retrieve-validation
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: qconnect-retrieve-validation qconnect-send-message-and-retrieve-rejections qconnect-session-message-refusals
Issue-Shape-Id: com.amazonaws.qconnect#WisdomService
Issue-Key: wisdom|Retrieve|4xx->4xx|$.message
Issue-Key: wisdom|Retrieve|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|SendMessage|4xx->4xx|$.message
Issue-Key: wisdom|SendMessage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: wisdom|ListMessages|4xx->4xx|$.message
Issue-Key: wisdom|ListMessages|4xx->4xx|$headers.x-amzn-errortype
Issue-Commit: 8c779968f63825f959c68d9ef908c64428218fcd

# wisdom assistant message-plane constraint violations answer BadRequestException 'Invalid request body' instead of the modeled ValidationException

> _Note: this issue was identified against the code base at commit `8c779968f63825f959c68d9ef908c64428218fcd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Retrieve, SendMessage, ListMessages

## Description
This issue bundles THREE distinct net-new root causes found by replaying the wire corpus (trace set simparity-wire-aws-v1) against the `wisdom` service (the qconnect/Wisdom restJson1 surface, crate services/service-com-amazonaws-qconnect-wisdomservice). All THREE must be closed before submitting; each cause names its own fixture#step. They share one fault - a modeled input-constraint violation on the assistant message/retrieval plane is refused at the deserialize/validate boundary with an UNMODELED `BadRequestException` and the generic message `Invalid request body`, instead of the operation's modeled `ValidationException` carrying AWS's constraint-violation sentence - but each cause is a different operation and a different constraint kind (a @length bound, and two enum sets), so each needs its own rendered clause and its own re-replay.

Already-filed issues that do NOT cover these: the open BEHAVIORAL issue 20261127T111426Z-qconnect-seven-behavioral-divergences-constraint-violations-answered-badrequestexception.md names the same shared constraint layer for CreateContent/UpdateContent @length violations (its cause 1) but names none of the three operations below and carries no wire replay key for these steps; the open stub issues 20261127T111426Z-qconnect-retrieve-stub.md and 20261127T111426Z-qconnect-sendmessage-stub.md are about the handlers answering 501, whereas these refusals come from the pre-handler validate stage (the replay shows a 400, not a 501, and reported no stub_path for any cluster below). The replay reported none of the three clusters as owned by an open issue.

=== Cause 1 of 3: Retrieve's retrievalQuery @length violation is refused BadRequestException 'Invalid request body' instead of ValidationException with the length clause ===
AWS: Smithy (services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json) - `Retrieve` is POST /assistants/{assistantId}/retrieve; its required member `retrievalQuery` targets NonEmptySensitiveString with @length {min:1, max:4096}; the operation models ValidationException (errorKind client, httpError 400) and declares NO BadRequestException (no operation in this model does). AWS refuses a request that breaks a modeled constraint with HTTP 400, error code ValidationException and the standard constraint sentence: fixed text `<N> validation error detected: Value at '<member>' failed to satisfy constraint: Member must have length less than or equal to <max>`; its variable parts are the violation count, the member path (`retrievalQuery`, taken from the request) and the modeled maximum (4096). Because the member is @sensitive, the offending value is NOT echoed - the recorded message is `1 validation error detected: Value at 'retrievalQuery' failed to satisfy constraint: Member must have length less than or equal to 4096`. The recorded status (400) is already matched; the error code and message are not.
Emulator: HTTP 400, but x-amzn-errortype `BadRequestException` and message `Invalid request body` - the body is rejected wholesale by the generic deserialize/validate stage, naming neither the member, nor the constraint, nor the limit.
Why: `BadRequestException` is an error no AWS model declares for this service, so a client catching the modeled ValidationException (botocore's generated exception class on the client) does not catch this refusal at all - it surfaces as an unexpected error type. And a caller or operator that parses the `N validation error detected ... at <member>` sentence to report WHICH field is wrong learns nothing.
Fix: the shared constraint-validation / error-mapping stage of this service's centralized request walk (deserialize -> validate -> handle -> serialize), so a Smithy @length violation on any input member is mapped to the modeled ValidationException with AWS's constraint sentence rather than falling through to the front-door invalid-body BadRequestException; the operation is services/service-com-amazonaws-qconnect-wisdomservice/src/ops/retrieve.rs.
Traces: qconnect-retrieve-validation#task#6.

=== Cause 2 of 3: SendMessage's out-of-set `type` enum value is refused BadRequestException 'Invalid request body' instead of ValidationException with the enum-value-set clause ===
AWS: Smithy - `SendMessage` is POST /assistants/{assistantId}/sessions/{sessionId}/message; its required member `type` targets the `MessageType` enum (this on-disk model lists TEXT, TOOL_USE_RESULT, DATA); the operation models ValidationException (400) and no BadRequestException. An out-of-set enum value is refused HTTP 400 ValidationException with the standard sentence `<N> validation error detected: Value '<rejected value>' at '<member>' failed to satisfy constraint: Member must satisfy enum value set: [<admissible values>]` - the recorded refusal is `1 validation error detected: Value 'VOICE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [DATA, TEXT, TOOL_USE_RESULT]`. Fixed text apart from the rejected value (from the request), the member path (`type`) and the admissible set, which the live service renders in the order `[DATA, TEXT, TOOL_USE_RESULT]`; `VOICE` is one run's rejected value filling the template, not part of it.
Emulator: HTTP 400 with x-amzn-errortype `BadRequestException` and message `Invalid request body`.
Why: same as cause 1 - the modeled exception is never raised, so typed error handling misses it, and the caller is not told which member was wrong nor which values it may use; a client discovering the admissible set from the refusal (an SDK-less or forward-compatible caller) cannot.
Fix: the same shared enum-constraint refusal path, carrying the enum's admissible set in the rendered clause; the operation is services/service-com-amazonaws-qconnect-wisdomservice/src/ops/send_message.rs.
Traces: qconnect-send-message-and-retrieve-rejections#task#3.

=== Cause 3 of 3: ListMessages' out-of-set `filter` query enum value is refused BadRequestException 'Invalid request body' instead of ValidationException with the enum-value-set clause ===
AWS: Smithy - `ListMessages` is GET /assistants/{assistantId}/sessions/{sessionId}/messages; its member `filter` is bound to the query string (@httpQuery `filter`) and targets the `MessageFilterType` enum, whose values are [ALL, TEXT_ONLY]; the operation models ValidationException (400), ResourceNotFoundException (404) and AccessDeniedException (403), and no BadRequestException. An out-of-set value is refused HTTP 400 ValidationException with the same constraint sentence: recorded as `1 validation error detected: Value 'VOICE_ONLY' at 'filter' failed to satisfy constraint: Member must satisfy enum value set: [ALL, TEXT_ONLY]` - fixed text apart from the rejected value (from the request's query string), the member path (`filter`) and the admissible set [ALL, TEXT_ONLY].
Emulator: HTTP 400 with x-amzn-errortype `BadRequestException` and message `Invalid request body` - note the offending value arrives in the QUERY STRING, not the body, so `Invalid request body` is doubly wrong: it misattributes the violation to a body this GET does not have.
Why: same client break as above, plus the diagnostic points at the wrong part of the request, so an operator debugging the refusal looks for a malformed payload on an operation that sends none.
Fix: the same shared constraint stage, extended to @httpQuery-bound enum members so a query-string enum violation renders the ValidationException clause naming the query member; the operation is services/service-com-amazonaws-qconnect-wisdomservice/src/ops/list_messages.rs.
Traces: qconnect-session-message-refusals#task#5.

## Evidence
Replay of trace set simparity-wire-aws-v1: 21 fixtures, 198 steps - 147 passed, 50 mismatched, 1 blocked, score 74.2%, emulator_up=true, bootstrap_signal=false. 10 divergence clusters, every one service `wisdom`; none of the three below carries stub_path or owned_by. Verbatim expected-vs-actual:

Cause 1 - qconnect-retrieve-validation#task#6, Retrieve (divergence_key wisdom|Retrieve|status=400->400|keys=$.message,$headers.x-amzn-errortype), status 400 -> 400:
  $.message expected: "1 validation error detected: Value at 'retrievalQuery' failed to satisfy constraint: Member must have length less than or equal to 4096"
  $.message actual:   'Invalid request body'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Cause 2 - qconnect-send-message-and-retrieve-rejections#task#3, SendMessage (divergence_key wisdom|SendMessage|status=400->400|keys=$.message,$headers.x-amzn-errortype), status 400 -> 400:
  $.message expected: "1 validation error detected: Value 'VOICE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [DATA, TEXT, TOOL_USE_RESULT]"
  $.message actual:   'Invalid request body'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Cause 3 - qconnect-session-message-refusals#task#5, ListMessages (divergence_key wisdom|ListMessages|status=400->400|keys=$.message,$headers.x-amzn-errortype), status 400 -> 400:
  $.message expected: "1 validation error detected: Value 'VOICE_ONLY' at 'filter' failed to satisfy constraint: Member must satisfy enum value set: [ALL, TEXT_ONLY]"
  $.message actual:   'Invalid request body'
  $headers.x-amzn-errortype: shape_mismatch (expected '<shaped-not-asserted>', got 'BadRequestException')

Model grounding (smithy_query against services/service-com-amazonaws-qconnect-wisdomservice/smithy-model/com-amazonaws-qconnect-wisdomservice.json): Retrieve.retrievalQuery = NonEmptySensitiveString, required, @length {min 1, max 4096}, @sensitive; Retrieve errors = AccessDenied 403, Conflict 409, DependencyFailed 424, RequestTimeout 408, ResourceNotFound 404, ServiceQuotaExceeded 402, Throttling 400, Validation 400. SendMessage.type = MessageType, required; MessageType values = [TEXT, TOOL_USE_RESULT, DATA] (the live service renders the set as [DATA, TEXT, TOOL_USE_RESULT]); SendMessage errors as Retrieve. ListMessages.filter = MessageFilterType bound @httpQuery 'filter'; MessageFilterType values = [ALL, TEXT_ONLY]; ListMessages errors = AccessDenied 403, ResourceNotFound 404, Validation 400. No operation in this model declares BadRequestException.

## Steps to Reproduce
Boot the emulator from this checkout and replay the three wisdom fixtures of simparity-wire-aws-v1 named below. Minimal reproduction (create an assistant, and for causes 2/3 a session on it, first):

1) Cause 1 - POST /assistants/<assistantId>/retrieve with retrievalQuery = a 4097-character string (and a valid retrievalConfiguration): observe 400 with x-amzn-errortype BadRequestException and message 'Invalid request body'; AWS answers 400 ValidationException with "1 validation error detected: Value at 'retrievalQuery' failed to satisfy constraint: Member must have length less than or equal to 4096" (the value is not echoed - the member is @sensitive). Re-replay qconnect-retrieve-validation#task#6.
2) Cause 2 - POST /assistants/<assistantId>/sessions/<sessionId>/message with type='VOICE' and a valid message: observe 400 BadRequestException 'Invalid request body'; AWS answers 400 ValidationException with "1 validation error detected: Value 'VOICE' at 'type' failed to satisfy constraint: Member must satisfy enum value set: [DATA, TEXT, TOOL_USE_RESULT]". Re-replay qconnect-send-message-and-retrieve-rejections#task#3.
3) Cause 3 - GET /assistants/<assistantId>/sessions/<sessionId>/messages?filter=VOICE_ONLY: observe 400 BadRequestException 'Invalid request body'; AWS answers 400 ValidationException with "1 validation error detected: Value 'VOICE_ONLY' at 'filter' failed to satisfy constraint: Member must satisfy enum value set: [ALL, TEXT_ONLY]". Re-replay qconnect-session-message-refusals#task#5.

All three steps must pass before this is closed.
