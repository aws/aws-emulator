Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-list-retained-messages-request-guards
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-list-retained-messages-request-guards iotdata-retained-message-listing-page-parameter-refusals iotdata-list-subscriptions-page-size-bounds iotdata-named-shadow-listing-page-parameter-refusals iotdata-shadow-unrouted-verbs-and-listing-guards iotdata-send-direct-message-request-guards
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|ListRetainedMessages|4xx->4xx|$.message
Issue-Key: iot-data|ListSubscriptions|4xx->4xx|$.message
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->4xx|$.message
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->4xx|$.traceId
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-data|ListRetainedMessages|4xx->4xx|$.traceId
Issue-Key: iot-data|ListRetainedMessages|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|$.results
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|$.timestamp
Issue-Key: iot-data|ListNamedShadowsForThing|2xx->4xx|status
Issue-Key: iot-data|SendDirectMessage|4xx->4xx|$.message
Issue-Key: iot-data|SendDirectMessage|4xx->4xx|$headers.x-amzn-errortype
Issue-Key: iot-data|SendDirectMessage|4xx->4xx|status
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot-data request-validation refusals carry generic or emulator-authored messages and the wrong error class

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListNamedShadowsForThing

## Description
iot-data's pagination and request guards refuse with the right HTTP status but the wrong body: a generic "Bad Request", a leaked Rust deserializer string, or the wrong error class entirely — and in one case they refuse a request AWS accepts. 8 causes, all in the input-validation layer of the listing and messaging operations.

Cause 1 of 8: ListRetainedMessages' maxResults refusal says "Bad Request".
AWS: Smithy: ListRetainedMessages.maxResults targets MaxResults with @range min 1 max 200; the operation declares InvalidRequestException (httpError 400). An out-of-range maxResults is refused 400 with the message "Invalid maxResults value: Invalid max results parameter" — the whole string is fixed text for this error class ("Invalid maxResults value: " plus the detail phrase "Invalid max results parameter"); nothing in it comes from the request.
Emulator: status 400 (correct) but $.message is the generic "Bad Request".
Why: a client cannot tell which parameter it got wrong, and any code or test matching on AWS's message text fails even though the status matches.
Fix: ListRetainedMessages' maxResults validation — emit AWS's message for the @range violation instead of falling back to a generic 400 body.
Traces: iotdata-list-retained-messages-request-guards#task#2 (also task#4, task#5) and iotdata-retained-message-listing-page-parameter-refusals#task#4, task#5, task#7 — 6 steps in the cluster.

Cause 2 of 8: ListSubscriptions' maxResults refusal says "Bad Request".
AWS: Smithy: ListSubscriptions.maxResults targets MaxResults with @range min 1 max 200; InvalidRequestException (httpError 400). Out-of-range is refused 400 with the fixed message "Invalid maxResults value." — note this operation's wording differs from ListRetainedMessages' and has no variable part.
Emulator: 400 with $.message "Bad Request".
Why: as Cause 1 — the refusal names no parameter, and the per-operation wording AWS uses is lost.
Fix: ListSubscriptions' maxResults validation.
Traces: iotdata-list-subscriptions-page-size-bounds#task#1 (also task#2).

Cause 3 of 8: ListNamedShadowsForThing's pageSize refusal says "Bad Request" and drops the rejected value.
AWS: Smithy: ListNamedShadowsForThing.pageSize targets PageSize with @range min 1 max 100; InvalidRequestException (httpError 400). An out-of-range pageSize is refused 400 with "Invalid pageSize: <value>" — fixed text "Invalid pageSize: " followed by the pageSize the request sent (0 in the recording).
Emulator: 400 with $.message "Bad Request".
Why: the client is told nothing about which bound it broke or what value was rejected.
Fix: ListNamedShadowsForThing's pageSize validation.
Traces: iotdata-named-shadow-listing-page-parameter-refusals#task#3 (also task#5) and iotdata-shadow-unrouted-verbs-and-listing-guards#task#8 — 3 steps in the cluster.

Cause 4 of 8: a non-numeric pageSize leaks the Rust deserializer's error string.
AWS: Smithy: pageSize is an integer @httpQuery. A query value that is not an integer is refused 400 with "Invalid pageSize" — fixed text with no value appended (contrast Cause 3, where the parsed value is echoed).
Emulator: 400 with $.message "invalid i32 `abc`: invalid digit found in string" and $headers.x-amzn-errortype "ValidationException" (a shape iot-data does not model — the modeled client error is InvalidRequestException), and no traceId.
Why: the emulator exposes its own implementation language in a wire-visible message, and tags the error with an errortype no iot-data client knows, so error-class dispatch picks the wrong branch.
Fix: ListNamedShadowsForThing's query-parameter deserialization — catch the integer parse failure and raise the modeled InvalidRequestException with AWS's message, instead of surfacing the parser's Display output.
Traces: iotdata-named-shadow-listing-page-parameter-refusals#task#4.

Cause 5 of 8: a non-numeric maxResults leaks the same deserializer string on ListRetainedMessages.
AWS: Smithy: maxResults is an integer @httpQuery; InvalidRequestException (httpError 400). A non-numeric value is refused 400 with "Invalid maxResults value: <value>" — fixed text "Invalid maxResults value: " plus the raw query value the request sent.
Emulator: 400 with $.message "invalid i32 `abc`: invalid digit found in string", x-amzn-errortype "ValidationException", no traceId.
Why: as Cause 4.
Fix: ListRetainedMessages' query-parameter deserialization.
Traces: iotdata-retained-message-listing-page-parameter-refusals#task#6.

Cause 6 of 8: ListNamedShadowsForThing refuses a pageSize AWS accepts.
AWS: Smithy: pageSize @range min 1 max 100 — an in-range pageSize is valid, and the call answers 200 with the output shape's results (the thing's shadow names) and timestamp members.
Emulator: answers 400, with no results and no timestamp, for a listing request that the recording shows succeeding against AWS.
Why: a correct client is refused, so named-shadow enumeration is impossible with an explicit page size — the operation is effectively unusable, not merely mis-worded. Hypothesis: the bound is applied exclusively (or the nextToken/pageSize combination is rejected), so a value at or near the legal edge is misclassified as out of range.
Fix: ListNamedShadowsForThing's pageSize validation — the @range check must be inclusive of min 1 and max 100 and must not reject a page size that accompanies a nextToken.
Traces: iotdata-named-shadow-listing-page-parameter-refusals#task#6.

Cause 7 of 8: an invalid thingName is answered 404 ResourceNotFoundException instead of a 400 validation refusal.
AWS: Smithy: ListNamedShadowsForThing.thingName targets ThingName with @length 1..128 and @pattern ^[a-zA-Z0-9:_-]+$; the operation declares both InvalidRequestException (httpError 400) and ResourceNotFoundException (httpError 404). A thingName that violates the pattern is a malformed request, refused 400 with "Invalid ThingName: \"<thingName>\"" — fixed text "Invalid ThingName: " plus the request's thing name in double quotes. 404 is reserved for a well-formed name that does not exist.
Emulator: status 404 with x-amzn-errortype "ResourceNotFoundException" and the emulator-authored message "Invalid thing name specified in request", which neither matches AWS's 400 wording nor echoes the offending name.
Why: a client cannot distinguish "you sent a syntactically illegal thing name" (fix the request) from "that thing does not exist" (create it / stop polling); retry and provisioning logic branch on exactly that.
Fix: ListNamedShadowsForThing's input validation — run the ThingName @pattern/@length check before the registry lookup and raise InvalidRequestException.
Traces: iotdata-shadow-unrouted-verbs-and-listing-guards#task#7.

Cause 8 of 8: SendDirectMessage checks the client-id state before validating the topic, so a reserved-prefix topic is answered 404.
AWS: Smithy: SendDirectMessage.topic is a required @httpQuery targeting Topic, and the operation declares InvalidRequestException (httpError 400) alongside ResourceNotFoundException (httpError 404). A topic beginning with "$" is a malformed request and is refused 400 with the fixed message "Topic can't start with $" — no variable part. Request validation precedes the client-session lookup.
Emulator: answers 404 with x-amzn-errortype "ResourceNotFoundException" and the message "The client ID is not found. Verify that the client is connected and try again", i.e. it resolves the clientId first and never reaches the topic guard.
Why: the reserved-topic rule becomes untestable and is masked by connection state: the same bad request returns 400 or 404 depending on whether some client happens to be connected, so a client misdiagnoses a permanent request defect as a transient connectivity problem and retries forever.
Fix: SendDirectMessage's input validation — order the topic guard ahead of the client-session lookup in the handler.
Traces: iotdata-send-direct-message-request-guards#task#4 (also task#6, task#10) — 3 steps in the cluster.

Note on linkage: Cause 7's binding is iotdata-shadow-unrouted-verbs-and-listing-guards#task#7.

## Evidence
Cause 1 — iotdata-list-retained-messages-request-guards#task#2: $.message value_mismatch, expected 'Invalid maxResults value: Invalid max results parameter', got 'Bad Request' (status 400 both). Cluster of 6 steps across iotdata-list-retained-messages-request-guards (task#2/4/5) and iotdata-retained-message-listing-page-parameter-refusals (task#4/5/7).
Cause 2 — iotdata-list-subscriptions-page-size-bounds#task#1 (+task#2): expected 'Invalid maxResults value.', got 'Bad Request'.
Cause 3 — iotdata-named-shadow-listing-page-parameter-refusals#task#3 (+task#5, +iotdata-shadow-unrouted-verbs-and-listing-guards#task#8): expected 'Invalid pageSize: 0', got 'Bad Request'.
Cause 4 — iotdata-named-shadow-listing-page-parameter-refusals#task#4: expected 'Invalid pageSize', got 'invalid i32 `abc`: invalid digit found in string'; $headers.x-amzn-errortype expected shaped, got 'ValidationException'; $.traceId missing.
Cause 5 — iotdata-retained-message-listing-page-parameter-refusals#task#6: expected 'Invalid maxResults value: abc', got 'invalid i32 `abc`: invalid digit found in string'; x-amzn-errortype 'ValidationException'; $.traceId missing.
Cause 6 — iotdata-named-shadow-listing-page-parameter-refusals#task#6: expected status 200 with $.results ['<shadow name>'] and $.timestamp present; actual status 400 with $.results and $.timestamp absent.
Cause 7 — iotdata-shadow-unrouted-verbs-and-listing-guards#task#7: expected status 400 with $.message 'Invalid ThingName: "verb!thing"'; actual 404 with 'Invalid thing name specified in request' and x-amzn-errortype 'ResourceNotFoundException'.
Cause 8 — iotdata-send-direct-message-request-guards#task#4 (+task#6, task#10): expected status 400 with $.message "Topic can't start with $"; actual 404 with 'The client ID is not found. Verify that the client is connected and try again' and x-amzn-errortype 'ResourceNotFoundException'.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), or by hand against the data listener:
1. GET /retainedMessage?maxResults=<out of 1..200> -> expect 400 'Invalid maxResults value: Invalid max results parameter'; emulator says 'Bad Request'.
2. GET /connections/<clientId>/subscriptions?maxResults=<out of 1..200> -> expect 400 'Invalid maxResults value.'; emulator says 'Bad Request'.
3. GET /api/things/shadow/ListNamedShadowsForThing/<thingName>?pageSize=0 -> expect 400 'Invalid pageSize: 0'; emulator says 'Bad Request'.
4. Same URI with pageSize=abc -> expect 400 'Invalid pageSize'; emulator leaks 'invalid i32 `abc`: invalid digit found in string' with x-amzn-errortype ValidationException.
5. GET /retainedMessage?maxResults=abc -> expect 400 'Invalid maxResults value: abc'; same leaked string.
6. Same listing URI with an in-range pageSize for a thing that has a named shadow -> expect 200 with results/timestamp; emulator answers 400.
7. GET /api/things/shadow/ListNamedShadowsForThing/verb!thing -> expect 400 'Invalid ThingName: "verb!thing"'; emulator answers 404 ResourceNotFoundException.
8. POST /connections/<clientId>/messages?topic=$something -> expect 400 "Topic can't start with $"; emulator answers 404 client-ID-not-found.
