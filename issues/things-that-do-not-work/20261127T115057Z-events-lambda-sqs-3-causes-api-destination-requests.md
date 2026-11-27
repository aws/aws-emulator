Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__lambda__sqs.api-destination-request-carries-the-documented-default-headers events__lambda__sqs.api-destination-request-keeps-its-own-user-agent-and-range-headers events__lambda__sqs.api-destination-request-forwards-custom-cache-control-and-origin-headers
Issue-Key: events__lambda__sqs.api-destination-request-carries-the-documented-default-headers|run#15|AssertionError|assert got == want, f"default headers were {got!r}, expected {want!r} (all headers: {headers!r})"
Issue-Key: events__lambda__sqs.api-destination-request-keeps-its-own-user-agent-and-range-headers|run#14|AssertionError|assert headers.get("user-agent") == "Amazon/EventBridge/ApiDestinations", (
Issue-Key: events__lambda__sqs.api-destination-request-forwards-custom-cache-control-and-origin-headers|_deliver_until#11|AssertionError|raise AssertionError(f"no request carrying {marker!r} reached the API destination within {budget_s}s")
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__lambda__sqs: 3 causes — API destination requests omit the documented Range/Accept-Encoding defaults, let a target override User-Agent, and never arrive when custom Cache-Control/Origin headers are configured

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutTargets

## Description
Cause 1 of 3: the default headers EventBridge adds to an API destination request are incomplete. AWS: every API destination invocation carries User-Agent `Amazon/EventBridge/ApiDestinations`, Content-Type `application/json; charset=utf-8`, Range `bytes=0-1048575` and Accept-Encoding `gzip,deflate` (the documented default header set). Recorded behavior only (the header set is documented for EventBridge API destinations; HttpParameters in Smithy only models the caller's own additions). Emulator: the request arrives with `range: None, accept-encoding: None` — the two documented defaults are missing. Checks: events__lambda__sqs.api-destination-request-carries-the-documented-default-headers.

Cause 2 of 3: a target's HeaderParameters can override User-Agent. AWS: EventBridge does not let a target override its own User-Agent — the header arrives as `Amazon/EventBridge/ApiDestinations` whatever the target's HttpParameters say (and the caller's Range is likewise bounded). Recorded behavior only. Emulator: the destination received `User-Agent: custom-agent/1.0` — the target's value won. Checks: events__lambda__sqs.api-destination-request-keeps-its-own-user-agent-and-range-headers.

Cause 3 of 3: configuring the allowed custom headers Cache-Control/Origin stops the delivery entirely. AWS: Cache-Control and Origin are forwardable header parameters — the API destination receives the request with those headers and the event body. Recorded behavior only. Emulator: `no request carrying '<marker>' reached the API destination within 30.0s` — nothing is delivered at all, i.e. the header configuration makes the invocation fail silently rather than forwarding it.

Why: a receiving API cannot identify EventBridge as the caller (or is handed an arbitrary caller identity), cannot rely on the bounded Range/encoding contract, and — worst — a destination configured with ordinary custom headers receives nothing with no dead-letter or error visible to the client.
Fix: the API destination invocation path in the events crate (default-header construction, the reserved-header list that must not be overridable, and the header-parameter forwarding that currently drops the request).
Traces: the three check ids listed per cause above.

## Evidence
Cause 1: default headers were {'user-agent': 'Amazon/EventBridge/ApiDestinations', 'content-type': 'application/json; charset=utf-8', 'range': None, 'accept-encoding': None}, expected {... 'range': 'bytes=0-1048575', 'accept-encoding': 'gzip,deflate'}
Cause 2: `AssertionError: User-Agent arrived as 'custom-agent/1.0'; EventBridge does not let a target override it`
Cause 3: `AssertionError: no request carrying 'm-c95862c4a2d2' reached the API destination within 30.0s`

## Steps to Reproduce
1. events CreateConnection + CreateApiDestination pointing at an HTTP endpoint that records its received headers.
2. PutRule + PutTargets naming the destination: (a) with no HttpParameters, (b) with HeaderParameters overriding User-Agent and Range, (c) with HeaderParameters Cache-Control and Origin.
3. PutEvents and inspect the recorded request headers / whether a request arrives at all.
