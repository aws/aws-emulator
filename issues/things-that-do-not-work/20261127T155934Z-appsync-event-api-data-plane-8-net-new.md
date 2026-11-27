Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appsync.event-api-http-publish-without-api-key-rejected appsync.event-namespace-iam-publish-mode-refuses-api-key appsync.event-publish-iam-only-namespace-rejects-api-key-until-updated appsync.event-publish-to-undeclared-namespace-rejected appsync.event-publish-to-undefined-namespace-rejected appsync.event-websocket-subscribe-to-undeclared-namespace-rejected appsync.event-channel-path-of-six-segments-rejected appsync.event-channel-segment-of-51-characters-rejected appsync.event-publish-channel-over-five-segments-rejected appsync.event-publish-channel-segment-over-fifty-characters-rejected appsync.event-websocket-subscribe-six-segment-channel-rejected appsync.event-publish-batch-of-six-events-rejected appsync.event-publish-batch-over-five-events-rejected appsync.event-ws-publish-six-events-rejected appsync.event-publish-malformed-event-rejected-while-siblings-publish appsync.event-websocket-unsubscribe-unknown-id-rejected appsync.event-websocket-duplicate-subscription-id-rejected appsync.event-ws-connect-unknown-api-key-rejected appsync.event-ws-on-subscribe-unauthorized-channel-rejected
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync.event-api-http-publish-without-api-key-rejected|run#27|AssertionError|assert (status, body) == (401, {"errors": [{"errorType": "UnauthorizedException"}]}), (
Issue-Key: appsync.event-namespace-iam-publish-mode-refuses-api-key|run#23|AssertionError|assert (status, refused) == (401, {"errors": [{"errorType": "UnauthorizedException"}]}), (
Issue-Key: appsync.event-publish-iam-only-namespace-rejects-api-key-until-updated|run#17|AssertionError|assert (status, body) == (401, {"errors": [{"errorType": "UnauthorizedException"}]}), (
Issue-Key: appsync.event-publish-to-undeclared-namespace-rejected|run#15|AssertionError|assert (status, refused) == (401, {"errors": [{"errorType": "UnauthorizedException"}]}), (
Issue-Key: appsync.event-publish-to-undefined-namespace-rejected|run#14|AssertionError|assert refused == expected, f"the undefined namespace gave {refused!r}, want {expected!r}"
Issue-Key: appsync.event-websocket-subscribe-to-undeclared-namespace-rejected|run#11|AssertionError|assert refused.get("errors") == [{"errorType": "UnauthorizedException"}], (
Issue-Key: appsync.event-channel-path-of-six-segments-rejected|run#21|AssertionError|assert status == 400, f"publishing to a six-segment channel answered {status}: {body!r}"
Issue-Key: appsync.event-channel-segment-of-51-characters-rejected|run#21|AssertionError|assert status == 400, f"publishing to a 51-character segment answered {status}: {body!r}"
Issue-Key: appsync.event-publish-channel-over-five-segments-rejected|run#19|AssertionError|assert (status, body) == (400, {"errors": [{
Issue-Key: appsync.event-publish-channel-segment-over-fifty-characters-rejected|run#19|AssertionError|assert (status, body) == (400, {"errors": [{
Issue-Key: appsync.event-websocket-subscribe-six-segment-channel-rejected|run#10|AssertionError|assert refused.get("type") == "subscribe_error", (
Issue-Key: appsync.event-publish-batch-of-six-events-rejected|run#23|AssertionError|assert errors == [("BadRequestException", "Input exceeded 5 event limit")], (
Issue-Key: appsync.event-publish-batch-over-five-events-rejected|run#23|AssertionError|assert (status, body) == (400, {"errors": [{
Issue-Key: appsync.event-ws-publish-six-events-rejected|run#18|AssertionError|assert refused.get("type") == "error", f"the six-event publish answered {refused!r}"
Issue-Key: appsync.event-publish-malformed-event-rejected-while-siblings-publish|run#18|AssertionError|assert failed == [{
Issue-Key: appsync.event-websocket-unsubscribe-unknown-id-rejected|run#9|AssertionError|assert _errors(refused) == [("UnknownOperationError", "Unknown operation id never-subscribed")], (
Issue-Key: appsync.event-websocket-duplicate-subscription-id-rejected|run#10|AssertionError|assert [code for code, _ in errors] == ["DuplicatedOperationError"], (
Issue-Key: appsync.event-ws-connect-unknown-api-key-rejected|run#14|AssertionError|assert got == {"handshake": 101, "first": refusal, "after": {"opcode": 8, "status": 1000}}, (
Issue-Key: appsync.event-ws-on-subscribe-unauthorized-channel-rejected|run#14|AssertionError|assert errors == [("Unauthorized", "Not authorized to access channel /guarded/shut")], (
Issue-Commit: 85a8ef40f44f446d2f7f13d4131def4b1abce9f7

# appsync: Event API data plane — 8 net-new causes (refusal bodies, channel validation not enforced, WebSocket error vocabulary)

> _Note: this issue was identified against the code base at commit `85a8ef40f44f446d2f7f13d4131def4b1abce9f7`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PublishEvents

## Description
The Event API data plane (HTTP /event publish and the realtime WebSocket) diverges from AWS in eight distinct ways. Each cause below must be closed before submitting.

Cause 1 of 8: an unauthorized HTTP publish carries a message member AWS does not send.
AWS: an Event API HTTP publish that is not authorized — no x-api-key header, or an API key against a namespace whose publishAuthModes are AWS_IAM only — is answered HTTP 401 with the body exactly {"errors": [{"errorType": "UnauthorizedException"}]}. The entry carries errorType and NOTHING else; no message member is present (recorded behavior only).
Emulator: answers HTTP 401 with {"errors": [{"errorType": "UnauthorizedException", "message": "You are not authorized to make this call."}]} — the status and code are right, the extra message member is not AWS's.
Why: a client that compares the refusal body, or forwards it verbatim, sees a field AWS never emits.
Fix: the Event API publish authorizer's refusal serializer.
Traces: appsync.event-api-http-publish-without-api-key-rejected, appsync.event-namespace-iam-publish-mode-refuses-api-key, appsync.event-publish-iam-only-namespace-rejects-api-key-until-updated

Cause 2 of 8: publishing or subscribing to an undeclared channel namespace is answered as not-found instead of unauthorized.
AWS: a publish to a channel whose namespace does not exist on the API is answered HTTP 401 with {"errors": [{"errorType": "UnauthorizedException"}]} — AWS does not disclose that the namespace is missing. The same holds for a WebSocket subscribe to an undeclared namespace.
Emulator: answers HTTP 404 with {"errors": [{"errorType": "NotFoundException", "message": "Channel namespace not found."}]} (and the same NotFoundException entry on the subscribe_error frame).
Why: the status, the error code and the existence of the namespace all leak; a client probing channels can enumerate which namespaces exist, and 404-vs-401 retry logic diverges.
Fix: the Event API publish/subscribe authorization path — namespace resolution failure must fall through to the unauthorized refusal.
Traces: appsync.event-publish-to-undeclared-namespace-rejected, appsync.event-publish-to-undefined-namespace-rejected, appsync.event-websocket-subscribe-to-undeclared-namespace-rejected

Cause 3 of 8: channel-path format is not validated at all.
AWS: a channel path carries at most 5 segments and each segment is at most 50 characters. A publish to a 6-segment channel or to a channel with a 51-character segment is refused HTTP 400 with errors == [{"errorType": "BadRequestException", "message": "Invalid Channel Format"}] and no "successful" member in the body; the same channel on a WebSocket subscribe is answered a subscribe_error frame carrying ("BadRequestException", "Invalid Channel Format"). A 5-segment channel is accepted (HTTP 200).
Emulator: accepts both — HTTP 200 with the event in "successful", and subscribe_success on the WebSocket.
Why: events are accepted and broadcast on channels AWS refuses, so a client's malformed topic silently works here and fails in production.
Fix: channel-path validation in the Event API publish and subscribe handlers.
Traces: appsync.event-channel-path-of-six-segments-rejected, appsync.event-channel-segment-of-51-characters-rejected, appsync.event-publish-channel-over-five-segments-rejected, appsync.event-publish-channel-segment-over-fifty-characters-rejected, appsync.event-websocket-subscribe-six-segment-channel-rejected

Cause 4 of 8: the over-five-events refusal is worded differently.
AWS: a publish carrying more than 5 events is refused HTTP 400 with errors == [{"errorType": "BadRequestException", "message": "Input exceeded 5 event limit"}] and no "successful" member; the WebSocket publish answers a publish_error frame carrying the same pair. The message is fixed text — the request's event count does not appear in it.
Emulator: answers the same status and errorType but the message "A publish carries at most 5 events; this one carries 6.", interpolating the count.
Why: the message is part of the error contract and is not AWS's; anything matching it fails.
Fix: the batch-size check in the Event API publish handler (HTTP and WebSocket).
Traces: appsync.event-publish-batch-of-six-events-rejected, appsync.event-publish-batch-over-five-events-rejected, appsync.event-ws-publish-six-events-rejected

Cause 5 of 8: a per-event failure entry carries the wrong code type and message.
AWS: in a partially accepted batch the "failed" entry is {"identifier": <uuid>, "index": <index>, "code": "InvalidEventException", "message": "Event should be a valid JSON string."} — code is the error-name STRING, and the siblings still publish at their own indexes.
Emulator: emits {"identifier": <uuid>, "index": 1, "code": 400, "message": "An event must be a JSON string."} — an integer HTTP status in "code" and its own message text.
Why: a client reading entry["code"] gets a number where AWS gives a name, so error classification per event breaks.
Fix: the per-event failure serializer in the Event API publish handler.
Traces: appsync.event-publish-malformed-event-rejected-while-siblings-publish

Cause 6 of 8: WebSocket subscription-management errors use an invented vocabulary.
AWS: an unsubscribe naming an id this connection holds no subscription for (including an id whose subscribe was refused) is answered an unsubscribe_error frame carrying errorType "UnknownOperationError" and message "Unknown operation id <id>", where <id> is the id from the request. A duplicate subscribe id is likewise refused in AWS's own vocabulary, not AppSync control-plane exception names.
Emulator: answers ("NotFoundException", "This connection holds no subscription under that id.") for the unknown unsubscribe and ("BadRequestException", "This connection already holds a subscription under that id.") for the duplicated subscribe id — neither code nor message is AWS's.
Why: realtime clients dispatch on errorType; UnknownOperationError is the only value AWS sends here.
Fix: the realtime WebSocket frame error mapping for unsubscribe/subscribe id handling.
Traces: appsync.event-websocket-unsubscribe-unknown-id-rejected, appsync.event-websocket-duplicate-subscription-id-rejected

Cause 7 of 8: a connection with an unknown API key is refused at the HTTP handshake instead of in-protocol.
AWS: the WebSocket handshake SUCCEEDS (HTTP 101) whatever the key is; after the client sends connection_init the endpoint answers a frame whose errors[0] is {"errorType": "UnauthorizedException", "message": "You are not authorized to make this call.", "errorCode": 401} and then closes the socket with close status 1000.
Emulator: fails the handshake itself with HTTP 401, so no frame is ever sent and the client sees a transport error rather than a protocol error.
Why: realtime clients only learn about auth failures from the post-connection_init frame; a handshake rejection surfaces as an unrecoverable socket error and no errorType at all.
Fix: the Event API realtime endpoint's connection authorization — move it after the handshake, into connection_init handling, and close with 1000.
Traces: appsync.event-ws-connect-unknown-api-key-rejected

Cause 8 of 8: the channel-namespace JS handler runtime does not provide `util`.
AWS: an onSubscribe handler in a channel namespace runs in the APPSYNC_JS runtime with the `util` helpers available, so a handler calling util.unauthorized() refuses the subscription with that denial.
Emulator: the handler throws, and the subscribe_error frame carries ("UnauthorizedException", "Error: util is not defined\n    at onSubscribe (channel-namespace-handlers:4:5)\n") — the runtime has no `util` binding, so the refusal reports a ReferenceError from the emulator's own evaluator instead of the handler's decision.
Why: any channel-namespace handler using util.* (the documented way to authorize or shape events) fails for the wrong reason, and the client sees emulator internals in the error message.
Fix: the channel-namespace handler runtime's global environment (`util` must be injected as it is for resolvers).
Traces: appsync.event-ws-on-subscribe-unauthorized-channel-rejected

## Evidence
Cause 1: expected (401, {'errors': [{'errorType': 'UnauthorizedException'}]}), got (401, {'errors': [{'errorType': 'UnauthorizedException', 'message': 'You are not authorized to make this call.'}]}).
Cause 2: expected (401, {'errors': [{'errorType': 'UnauthorizedException'}]}), got (404, {'errors': [{'errorType': 'NotFoundException', 'message': 'Channel namespace not found.'}]}).
Cause 3: publishing to 'orders/region/store/aisle/shelf/bin' answered 200 {'failed': [], 'successful': [{'identifier': ..., 'index': 0}]} where 400 + [('BadRequestException', 'Invalid Channel Format')] was required; the six-segment WebSocket subscribe answered {'type': 'subscribe_success', 'id': 'six-deep'}.
Cause 4: expected [('BadRequestException', 'Input exceeded 5 event limit')], got [{'errorType': 'BadRequestException', 'message': 'A publish carries at most 5 events; this one carries 6.'}].
Cause 5: expected [{'code': 'InvalidEventException', 'index': 1, 'message': 'Event should be a valid JSON string.'}], got [{'identifier': ..., 'index': 1, 'code': 400, 'message': 'An event must be a JSON string.'}].
Cause 6: expected [('UnknownOperationError', 'Unknown operation id never-subscribed')], got [('NotFoundException', 'This connection holds no subscription under that id.')]; duplicate id got ('BadRequestException', 'This connection already holds a subscription under that id.').
Cause 7: expected {'handshake': 101, 'first': {'errors': [{'errorType': 'UnauthorizedException', 'message': 'You are not authorized to make this call.', 'errorCode': 401}]}, 'after': {'opcode': 8, 'status': 1000}}, got {'handshake': 401}.
Cause 8: subscribe_error carried [('UnauthorizedException', 'Error: util is not defined\n    at onSubscribe (channel-namespace-handlers:4:5)\n')].

## Steps to Reproduce
Create an Event API (CreateApi with eventConfig) plus a channel namespace and an API key, then: (1) POST /event with no x-api-key, and POST with a key to a namespace whose publishAuthModes are AWS_IAM only; (2) POST /event to a channel in a namespace that was never created, and subscribe to one over the WebSocket; (3) POST to 'ns/a/b/c/d/e' and to a channel with a 51-character segment, and subscribe to the same; (4) POST six events in one call and publish six over the WebSocket; (5) POST three events where one is not valid JSON; (6) send unsubscribe for an id never subscribed, and subscribe twice with one id; (7) dial the realtime endpoint with an unknown API key and send connection_init; (8) create a channel namespace whose onSubscribe code calls util.unauthorized() and subscribe. Replay the checks in trace_ids.
