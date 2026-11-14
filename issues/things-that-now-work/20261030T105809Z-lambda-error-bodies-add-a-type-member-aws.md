Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-code-signing-config-enforcement
Issue-Step: task#10
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-code-signing-config-enforcement lambda-function-codesigning-attach-detach-lifecycle lambda-codesigning-config-crud-and-list lambda-function-codesigning-attach-error-paths lambda-resource-policy-conditional-update-and-stale-revision lambda-resource-policy-precondition-conflict lambda-resource-policy-revision-mismatch lambda-durable-execution-list-and-callback-errors
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|DeleteCodeSigningConfig|4xx->4xx|$.Type
Issue-Key: lambda|GetCodeSigningConfig|4xx->4xx|$.Type
Issue-Key: lambda|UpdateCodeSigningConfig|4xx->4xx|$.Type
Issue-Key: lambda|ListFunctionsByCodeSigningConfig|4xx->4xx|$.Type
Issue-Key: lambda|DeleteResourcePolicy|4xx->4xx|$.Type
Issue-Key: lambda|SendDurableExecutionCallbackSuccess|4xx->4xx|$.Type
Issue-Key: lambda|SendDurableExecutionCallbackFailure|4xx->4xx|$.Type
Issue-Key: lambda|SendDurableExecutionCallbackHeartbeat|4xx->4xx|$.Type
Issue-Related: 20261029T233239Z-lambda-durable-execution-errors-use-generic-validation-text.md
Issue-Commit: b3ba0990fd95b61e5d3be802f2374a3915556dd0

# Lambda error bodies add a Type member AWS does not return on code-signing, resource-policy and durable-callback refusals

> _Note: this issue was identified against the code base at commit `b3ba0990fd95b61e5d3be802f2374a3915556dd0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCodeSigningConfig

## Description
Shared fault: the emulator populates the optional `Type` member of every Lambda error structure with the constant string `User`, on operations where the recorded AWS refusal returns an error body WITHOUT `Type` at all. The HTTP status is correct in all nine clusters; only the body carries a member AWS does not emit. `Type` is modeled as an optional member on these shapes, so it is not a member a client may assume is populated, and Lambda only returns it on some error classes (the code-signing deploy refusals in this same corpus DO carry `Type: User`). Emitting it unconditionally is therefore a serializer-level defect, not a per-operation one.

AWS: each of these refusals returns the modeled error shape with `message`/`Message` only and no `Type` member. Smithy (com.amazonaws.lambda): ResourceNotFoundException httpError 404, members {Type, Message}, both optional; ResourceConflictException httpError 409, members {Type, message}, both optional; PreconditionFailedException httpError 412, members {Type, message}, both optional; the three SendDurableExecutionCallback* operations model InvalidParameterValueException/CallbackTimeoutException at httpError 400 with optional {Type, message}. Recorded behavior only for which classes carry `Type`: the recordings for these nine steps show the member absent.

Emulator: every one of these responses adds `"Type": "User"`. The status, and in the code-signing/resource-policy causes the message too, are otherwise correct.

Why: a client that branches on the presence of `Type` — AWS SDK error unmarshallers surface it as a distinct field, and retry/classification wrappers test it to tell a user-fault refusal from a service fault — sees a user-fault marker on refusals where AWS supplies none, so an error that should be classified only by its code and status is reclassified. Round-trip comparisons and golden-response assertions against recorded AWS bodies also fail on the extra key.

Fix: the lambda error serializer (the shared code that renders a modeled Lambda exception into its JSON body and x-amzn-errortype header). `Type` must be emitted only where the error class actually sets it, not defaulted to `User` for every raised exception; the handlers that raise ResourceNotFoundException / ResourceConflictException / PreconditionFailedException for code-signing configs, resource policies and durable-execution callbacks must leave it unset.

Cause 1 of 9: DeleteCodeSigningConfig, 409 ResourceConflictException (config still attached to functions) — fixture lambda-code-signing-config-enforcement step task#10. Expected 409 with no `Type`; emulator returned 409 with `Type: User`.
Cause 2 of 9: GetCodeSigningConfig, 404 ResourceNotFoundException (unknown CodeSigningConfigArn) — fixture lambda-code-signing-config-enforcement step task#15. Same extra `Type: User`.
Cause 3 of 9: UpdateCodeSigningConfig, 404 ResourceNotFoundException — fixture lambda-codesigning-config-crud-and-list step task#8. Same extra `Type: User`.
Cause 4 of 9: ListFunctionsByCodeSigningConfig, 404 ResourceNotFoundException — fixture lambda-codesigning-config-crud-and-list step task#9. Same extra `Type: User`.
Cause 5 of 9: DeleteCodeSigningConfig, 404 ResourceNotFoundException (already-deleted config) — fixture lambda-codesigning-config-crud-and-list step task#10. Same extra `Type: User`.
Cause 6 of 9: DeleteResourcePolicy, 412 PreconditionFailedException (stale RevisionId) — fixture lambda-resource-policy-conditional-update-and-stale-revision step task#8. Same extra `Type: User`.
Cause 7 of 9: SendDurableExecutionCallbackSuccess, 400 for an unusable callback id — fixture lambda-durable-execution-list-and-callback-errors step task#2. Extra `Type: User`; AWS's body is the fixed text `Invalid callback id` with no `Type`.
Cause 8 of 9: SendDurableExecutionCallbackFailure, 400 — fixture lambda-durable-execution-list-and-callback-errors step task#3. Same defect.
Cause 9 of 9: SendDurableExecutionCallbackHeartbeat, 400 — fixture lambda-durable-execution-list-and-callback-errors step task#4. Same defect.

Relation to open issues: the open issue `20261029T233239Z-lambda-error-bodies-omit-the-modeled-type-member.md` reports the OPPOSITE direction (Type absent where AWS returns it) on function-lifecycle and concurrency operations; taken together, `Type` is not being driven by the error class at all. The open issue `20261029T233239Z-lambda-durable-execution-errors-use-generic-validation-text.md` already owns the message/x-amzn-errortype halves of causes 7-9 (generic `1 validation error detected: ... [A-Za-z0-9+/]+={0,2}` under `ValidationException` instead of the fixed `Invalid callback id`); neither issue covers the spurious `Type` member, which is what this issue is for. Fixing only the message there would leave these three steps still diverging.

## Evidence
Replay cluster details, all nine with expected_status == actual_status:
- lambda-code-signing-config-enforcement#task#10 DeleteCodeSigningConfig 409->409: `$.Type: type_mismatch (expected None, got 'User')`
- lambda-code-signing-config-enforcement#task#15 GetCodeSigningConfig 404->404: `$.Type: type_mismatch (expected None, got 'User')` (also affects lambda-codesigning-config-crud-and-list#task#7, lambda-function-codesigning-attach-detach-lifecycle#task#11)
- lambda-codesigning-config-crud-and-list#task#8 UpdateCodeSigningConfig 404->404: `$.Type: type_mismatch (expected None, got 'User')`
- lambda-codesigning-config-crud-and-list#task#9 ListFunctionsByCodeSigningConfig 404->404: `$.Type: type_mismatch (expected None, got 'User')` (also lambda-function-codesigning-attach-error-paths#task#9)
- lambda-codesigning-config-crud-and-list#task#10 DeleteCodeSigningConfig 404->404: `$.Type: type_mismatch (expected None, got 'User')`
- lambda-resource-policy-conditional-update-and-stale-revision#task#8 DeleteResourcePolicy 412->412: `$.Type: type_mismatch (expected None, got 'User')` (also lambda-resource-policy-precondition-conflict#task#6, lambda-resource-policy-revision-mismatch#task#6)
- lambda-durable-execution-list-and-callback-errors#task#2/#task#3/#task#4 SendDurableExecutionCallbackSuccess/Failure/Heartbeat 400->400: `$.Type: type_mismatch (expected None, got 'User')`, plus `$.message: value_mismatch (expected 'Invalid callback id', got "1 validation error detected: Value '<callbackId>' at 'callbackId' failed to satisfy constraint: Member must satisfy regular expression pattern: [A-Za-z0-9+/]+={0,2}")` and `$headers.x-amzn-errortype: shape_mismatch (got 'ValidationException')`.

Expected (AWS): error body = the modeled exception with its message member only, e.g. 404 ResourceNotFoundException `{"Message": "<not-found text>"}` and 400 `{"message": "Invalid callback id"}` — fixed text, no variable part echoed — with no `Type` key.
Actual (emulator): the same body plus `"Type": "User"`.

## Steps to Reproduce
Replay the corpus against the emulator.
1. Fixture lambda-code-signing-config-enforcement step task#10: DeleteCodeSigningConfig for a config still attached to a function — expect 409 whose body has no `Type`; observe `Type: User`.
2. Fixture lambda-codesigning-config-crud-and-list steps task#8/#9/#10: Update/ListFunctionsBy/Delete against a deleted CodeSigningConfigArn — expect 404 with no `Type`.
3. Fixture lambda-resource-policy-conditional-update-and-stale-revision step task#8: DeleteResourcePolicy with a stale RevisionId — expect 412 with no `Type`.
4. Fixture lambda-durable-execution-list-and-callback-errors steps task#2/#3/#4: SendDurableExecutionCallbackSuccess/Failure/Heartbeat with a callback id that is not a usable token — expect 400 `{"message": "Invalid callback id"}` with no `Type`.
