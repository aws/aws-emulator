Issue-Source: trace-sweep-divergence
Issue-Trace: iotdata-publish-mqtt5-option-guards
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iotdata-publish-mqtt5-option-guards iot-data-publish-unsupported-verb-refused iot-data-retained-message-unsupported-verb-refused iotdata-named-shadow-listing-unsupported-verb-refused iotdata-shadow-unrouted-verbs-and-listing-guards iotdata-thing-shadow-unsupported-verb-refused
Issue-Shape-Id: com.amazonaws.iotdataplane#IotMoonrakerService
Issue-Key: iot-data|Publish|2xx->5xx|$.message
Issue-Key: iot-data|Publish|2xx->5xx|$.traceId
Issue-Key: iot-data|Publish|2xx->5xx|status
Issue-Key: iot-data|Publish|4xx->5xx|$.message
Issue-Key: iot-data|Publish|4xx->5xx|$.traceId
Issue-Key: iot-data|Publish|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: iot-data|Publish|4xx->5xx|status
Issue-Key: iot-data|GetRetainedMessage|4xx->5xx|$.message
Issue-Key: iot-data|GetRetainedMessage|4xx->5xx|$.traceId
Issue-Key: iot-data|GetRetainedMessage|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: iot-data|GetRetainedMessage|4xx->5xx|status
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->5xx|$.message
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->5xx|$.traceId
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: iot-data|ListNamedShadowsForThing|4xx->5xx|status
Issue-Key: iot-data|UpdateThingShadow|4xx->5xx|$.message
Issue-Key: iot-data|UpdateThingShadow|4xx->5xx|$.traceId
Issue-Key: iot-data|UpdateThingShadow|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: iot-data|UpdateThingShadow|4xx->5xx|status
Issue-Key: iot-data|GetThingShadow|4xx->5xx|$.message
Issue-Key: iot-data|GetThingShadow|4xx->5xx|$.traceId
Issue-Key: iot-data|GetThingShadow|4xx->5xx|$headers.x-amzn-errortype
Issue-Key: iot-data|GetThingShadow|4xx->5xx|status
Issue-Commit: 244eae870d957920c579d40eef12d136da6f5bab

# iot-data routes answer 501 NotImplemented instead of serving Publish or refusing an unrouted method 404

> _Note: this issue was identified against the code base at commit `244eae870d957920c579d40eef12d136da6f5bab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
The iot-data data-plane router answers 501 with x-amzn-errortype: NotImplemented and the emulator-authored body "operation <METHOD> <path> is not implemented in this emulator" in two distinct situations where AWS never answers 501: for a Publish whose topic has more than one path segment, and for any HTTP method that is not bound to an existing data-plane URI. 7 causes, all in the same routing layer.

Cause 1 of 7: Publish with a multi-level MQTT topic is not routed at all.
AWS: Smithy: Publish is POST /topics/{topic} with topic as a required @httpLabel, qos @httpQuery with @range min 0 max 1, output Unit, HTTP code 200. An MQTT topic is hierarchical, so the label spans the whole remaining path (recorded behavior: a publish to a slash-separated topic such as <prefix>/telemetry/<thing> answers 200 with body {"message": "OK", "traceId": <per-run id>}).
Emulator: answers 501 NotImplemented, body message "operation POST /topics/<multi-segment topic> is not implemented in this emulator", and no traceId member. Single-segment topics on the same route do reach a handler, so the defect is that the {topic} label is matched as one non-greedy path segment.
Why: every realistic IoT client publishes to a hierarchical topic. A client gets a 501 — which the SDK surfaces as an unmodeled protocol error rather than any modeled Publish error — so no telemetry path works at all, and retry logic treats it as a permanent service fault.
Fix: the data-plane route table / URI matcher for Publish — the {topic} httpLabel must consume the rest of the path, not a single segment.
Traces: iotdata-publish-mqtt5-option-guards#task#1 (also task#3, task#5, task#8, task#10).

Cause 2 of 7: the Publish qos out-of-range refusal is unreachable behind the same routing gap.
AWS: Smithy: Publish.qos targets Qos with @range min 0 max 1, and Publish declares InvalidRequestException (httpError 400). A request carrying qos=2 is refused 400 with the fixed message "QoS 2 is currently not supported" (the only variable part is the rejected qos value).
Emulator: answers 501 NotImplemented with the "...is not implemented in this emulator" message instead of the 400 refusal, because the request never reaches Publish's validation.
Why: a client that probes MQTT5 option support cannot distinguish an unsupported QoS from a broken service, and code that handles InvalidRequestException never runs.
Fix: Publish's input validation, once Cause 1 makes the route reachable — range-check qos before any publish side effect.
Traces: iotdata-publish-mqtt5-option-guards#task#2 (also task#4, task#6, task#7, task#9, task#11).

Cause 3 of 7: an unbound method on /topics/{topic} answers 501 instead of 404.
AWS: Smithy: only POST is bound to /topics/{topic}. recorded behavior: AWS's data-plane front end answers an unrouted method on that URI with HTTP 404 and body {"message": "Not Found"} — fixed text, no variable part.
Emulator: status 501, message "operation PUT /topics/<topic> is not implemented in this emulator", header x-amzn-errortype: NotImplemented, and no traceId.
Why: a client cannot tell "this URI/method combination does not exist" (a client error, do not retry) from "the service failed" (a server error, retry). 501 is also classified retryable-server by several SDKs.
Fix: the data-plane router's fallback for a known URI with an unbound method — answer 404 {"message": "Not Found"} rather than the NotImplemented scaffold response.
Traces: iot-data-publish-unsupported-verb-refused#task#2.

Cause 4 of 7: an unbound method on /retainedMessage/{topic} answers 501 instead of 404.
AWS: Smithy: GetRetainedMessage is GET /retainedMessage/{topic}; no DELETE is bound. recorded behavior: 404 with body {"message": "Not Found"}.
Emulator: 501, message "operation DELETE /retainedMessage/<topic> is not implemented in this emulator", x-amzn-errortype: NotImplemented, no traceId.
Why: same as Cause 3 — an unrouted method is reported as a server-side gap.
Fix: the same router fallback.
Traces: iot-data-retained-message-unsupported-verb-refused#task#3.

Cause 5 of 7: an unbound method on the named-shadow listing URI answers 501 instead of 404 with AWS's URI/method wording.
AWS: Smithy: ListNamedShadowsForThing is GET /api/things/shadow/ListNamedShadowsForThing/{thingName}; no POST is bound. recorded behavior: 404 with message "URI: <request path> by HTTP method: <METHOD> is not found" — fixed text "URI: ", " by HTTP method: " and " is not found", with the request's own path and method as the variable parts.
Emulator: 501, message "operation POST <path> is not implemented in this emulator", x-amzn-errortype: NotImplemented, no traceId.
Why: the shadow REST surface reports a client-side routing mistake as an unimplemented operation, so a client never learns the method is simply wrong for that URI.
Fix: the shadow-subtree route fallback — this subtree has its own 404 wording, distinct from the plain "Not Found" of Causes 3/4.
Traces: iotdata-named-shadow-listing-unsupported-verb-refused#task#2.

Cause 6 of 7: PUT on /things/{thingName}/shadow answers 501 instead of 404 (shadow-verb fixture).
AWS: Smithy: /things/{thingName}/shadow binds GET (GetThingShadow), POST (UpdateThingShadow) and DELETE (DeleteThingShadow); PUT is bound to nothing. recorded behavior: 404 {"message": "Not Found"}.
Emulator: 501, message "operation PUT /things/<thingName>/shadow is not implemented in this emulator", x-amzn-errortype: NotImplemented, no traceId.
Why: a client probing which shadow verbs exist is told the service is incomplete rather than that PUT is not a shadow verb.
Fix: the same router fallback.
Traces: iotdata-shadow-unrouted-verbs-and-listing-guards#task#2 (also task#3).

Cause 7 of 7: PUT on /things/{thingName}/shadow answers 501 instead of 404 (thing-shadow-verb fixture, reported against GetThingShadow).
AWS: as Cause 6 — 404 {"message": "Not Found"}.
Emulator: 501 with the same NotImplemented scaffold message and no traceId; the replay attributes this cluster to GetThingShadow rather than UpdateThingShadow, so it is a separate cluster that must be re-replayed on its own.
Why: as Cause 6.
Fix: the same router fallback.
Traces: iotdata-thing-shadow-unsupported-verb-refused#task#2.

Related but not covering: the open issue "iot__sqs: a certificate-authenticated HTTPS publish (POST /topics/{topic}) at the published iot:Data-ATS address answers 404 UnknownService" concerns the iot service's mTLS endpoint answering 404 UnknownService; it is a different service token and a different status, and does not cover these sigv4 data-plane routes.

## Evidence
Cause 1 — iotdata-publish-mqtt5-option-guards#task#1: expected status 200 with $.message 'OK'; actual 501 with $.message 'operation POST /topics/sim2real/telemetry/<thing> is not implemented in this emulator', $.traceId missing. 5 steps in the cluster.
Cause 2 — iotdata-publish-mqtt5-option-guards#task#2: expected status 400 with $.message 'QoS 2 is currently not supported'; actual 501 with the NotImplemented message, $headers.x-amzn-errortype 'NotImplemented'. 6 steps in the cluster.
Cause 3 — iot-data-publish-unsupported-verb-refused#task#2: expected 404 $.message 'Not Found'; actual 501 'operation PUT /topics/<topic> is not implemented in this emulator', x-amzn-errortype 'NotImplemented'.
Cause 4 — iot-data-retained-message-unsupported-verb-refused#task#3: expected 404 $.message 'Not Found'; actual 501 'operation DELETE /retainedMessage/<topic> is not implemented in this emulator'.
Cause 5 — iotdata-named-shadow-listing-unsupported-verb-refused#task#2: expected 404 $.message 'URI: /api/things/shadow/ListNamedShadowsForThing/<thingName> by HTTP method: POST is not found'; actual 501 'operation POST <same path> is not implemented in this emulator'.
Cause 6 — iotdata-shadow-unrouted-verbs-and-listing-guards#task#2 (+task#3): expected 404 'Not Found'; actual 501 'operation PUT /things/<thingName>/shadow is not implemented in this emulator'.
Cause 7 — iotdata-thing-shadow-unsupported-verb-refused#task#2: expected 404 'Not Found'; actual 501 same NotImplemented message.

## Steps to Reproduce
Boot the emulator from this checkout and replay the wire corpus (trace set simparity-wire-aws-v1), or by hand against the data listener:
1. POST /topics/a/b/c with a payload -> expect 200 {"message":"OK","traceId":...}; emulator answers 501 NotImplemented.
2. POST /topics/a/b/c?qos=2 -> expect 400 "QoS 2 is currently not supported"; emulator answers 501.
3. PUT /topics/<topic> -> expect 404 {"message":"Not Found"}; emulator answers 501.
4. DELETE /retainedMessage/<topic> -> expect 404 {"message":"Not Found"}; emulator answers 501.
5. POST /api/things/shadow/ListNamedShadowsForThing/<thingName> -> expect 404 "URI: <path> by HTTP method: POST is not found"; emulator answers 501.
6/7. PUT /things/<thingName>/shadow -> expect 404 {"message":"Not Found"}; emulator answers 501.
