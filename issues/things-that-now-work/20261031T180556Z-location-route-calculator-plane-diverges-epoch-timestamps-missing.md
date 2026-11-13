Issue-Source: trace-sweep-divergence
Issue-Trace: location-route-calculator-crud-and-conflict
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-route-calculator-crud-and-conflict
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|DescribeRouteCalculator|2xx->2xx|$.CreateTime
Issue-Key: location|DescribeRouteCalculator|2xx->2xx|$.Tags
Issue-Key: location|DescribeRouteCalculator|2xx->2xx|$.UpdateTime
Issue-Key: location|DescribeRouteCalculator|4xx->2xx|$.message
Issue-Key: location|DescribeRouteCalculator|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|DescribeRouteCalculator|4xx->2xx|status
Issue-Commit: a4dc6e21f65e74528fa71c599283ec5aa084cf98

# Location route calculator plane diverges: epoch timestamps, missing Tags, and delete never removes the calculator

> _Note: this issue was identified against the code base at commit `a4dc6e21f65e74528fa71c599283ec5aa084cf98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeRouteCalculator

## Description
Two causes, one shared fault: the route-calculator resource plane in the location emulator does not render or retire a calculator the way AWS does — the describe read serializes the modelled date-time timestamps as bare epoch numbers and drops the Tags member, and the delete leaves the calculator readable so the subsequent describe answers 200 instead of the modelled ResourceNotFoundException.

Cause 1 of 2: DescribeRouteCalculator serializes CreateTime/UpdateTime as epoch-second numbers and omits Tags.
AWS: Smithy: DescribeRouteCalculator GET /routes/v0/calculators/{CalculatorName}, output DescribeRouteCalculatorResponse with CreateTime (required, Timestamp, @timestampFormat date-time, @sensitive), UpdateTime (required, Timestamp, @timestampFormat date-time, @sensitive) and Tags (TagMap, @length max 50). Timestamps are therefore ISO 8601 date-time strings on the wire, and the recording for a calculator created without tags carries Tags as an empty JSON object.
Emulator: CreateTime and UpdateTime come back as JSON numbers (epoch seconds with a fractional part) instead of date-time strings, and Tags is absent from the body entirely.
Why: an SDK decoding a @timestampFormat date-time member against a bare number either errors out or yields a wrong instant, so every client reading a calculator's create/update time breaks; and because AWS answers an untagged calculator with an empty map rather than omitting the member, a client cannot distinguish "no tags" from "field not returned" and a read-after-create round-trip comparison fails.
Fix: the DescribeRouteCalculator response serializer and the route-calculator state projection in the location service crate — render Timestamp members with the modelled date-time format (as the tracker read paths must also do, see open issue 20261029T234705Z-location-tracker-read-responses-serialize-timestamps-as-epoch.md, which covers DescribeTracker/ListTrackers and not this operation) and always project the calculator's tag map, as an empty map when untagged.
Traces: location-route-calculator-crud-and-conflict#task#2.

Cause 2 of 2: after DeleteRouteCalculator succeeds, DescribeRouteCalculator still answers 200 with the deleted calculator instead of 404 ResourceNotFoundException.
AWS: Smithy: DeleteRouteCalculator DELETE /routes/v0/calculators/{CalculatorName} (@idempotent) retires the calculator; DescribeRouteCalculator then models ResourceNotFoundException (client error, HTTP 404, required member Message serialized with jsonName "message", documented "The resource that you've entered was not found in your AWS account"), and the REST-JSON refusal carries the x-amzn-errortype header naming that shape. The recorded refusal body message is "Route calculator not found: <CalculatorName>." — fixed text "Route calculator not found: " plus the CalculatorName taken from the request URI label and a trailing period; the name in the recording is a per-run probe value, not part of the requirement.
Emulator: the describe after the delete returns HTTP 200 with the calculator's body — no message member and no x-amzn-errortype header — so the delete left the resource readable (either it never removed it from state or the read path does not consult the deletion).
Why: a client that deletes a calculator and polls describe to confirm removal never sees the resource go away, so teardown loops, idempotent re-create flows and "does this calculator still exist?" checks all conclude the wrong thing; worse, callers that treat a 200 as liveness will keep routing requests at a calculator AWS has already retired, and error handling for the modelled 404 path is never exercised.
Fix: the DeleteRouteCalculator handler and the route-calculator state store in the location service crate — remove the calculator from state on delete and have the describe read path raise ResourceNotFoundException (404, x-amzn-errortype: ResourceNotFoundException, message as worded above) for an absent name. Note the open issue 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md covers emulator-authored refusal wording on other location operations; it does not cover this operation and does not cover a delete that fails to take effect.
Traces: location-route-calculator-crud-and-conflict#task#6.

## Evidence
Cause 1 — fixture location-route-calculator-crud-and-conflict, step task#2 (DescribeRouteCalculator, 200 expected and 200 returned): $.CreateTime expected an ISO 8601 date-time string (shape asserted, value not), got the number 1793469900.554; $.UpdateTime likewise expected a date-time string, got the number 1793469900.554; $.Tags expected {} (an empty map), got None (member absent).

Cause 2 — fixture location-route-calculator-crud-and-conflict, step task#6 (DescribeRouteCalculator after the calculator was deleted): status expected 404, got 200; $.message expected 'Route calculator not found: <CalculatorName>.' (recorded as 'Route calculator not found: sim2real-probe-location-routecalc-d3f67e2a.' — fixed text plus the requested calculator name), got None; response header x-amzn-errortype expected to be present naming the error shape, got None. Divergence keys: service=location|DescribeRouteCalculator|status=200->200|keys=$.CreateTime,$.Tags,$.UpdateTime and service=location|DescribeRouteCalculator|status=404->200|keys=$.message,$headers.x-amzn-errortype,status.

## Steps to Reproduce
1. Replay fixture location-route-calculator-crud-and-conflict against the emulator.
2. Step task#2 — GET /routes/v0/calculators/{CalculatorName} (DescribeRouteCalculator) for a calculator just created with no tags: observe CreateTime and UpdateTime emitted as JSON epoch numbers instead of ISO 8601 date-time strings, and the Tags member missing where AWS returns an empty map.
3. Step task#5 — DELETE /routes/v0/calculators/{CalculatorName} (DeleteRouteCalculator) for that same calculator.
4. Step task#6 — GET /routes/v0/calculators/{CalculatorName} again: observe HTTP 200 with the deleted calculator's body instead of HTTP 404 ResourceNotFoundException with x-amzn-errortype set and message 'Route calculator not found: <CalculatorName>.'
