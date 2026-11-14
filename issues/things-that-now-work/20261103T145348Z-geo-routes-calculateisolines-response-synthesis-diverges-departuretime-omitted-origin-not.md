Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-calculate-isolines-distance-threshold
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-calculate-isolines-distance-threshold
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateIsolines|2xx->2xx|$.DepartureTime
Issue-Key: geo-routes|CalculateIsolines|2xx->2xx|$.SnappedOrigin[*]
Issue-Key: geo-routes|CalculateIsolines|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Commit: 66471a963a64e2b8a9116c3903f472052675d9f3

# CalculateIsolines response synthesis diverges: DepartureTime omitted, origin not snapped, wrong pricing bucket

> _Note: this issue was identified against the code base at commit `66471a963a64e2b8a9116c3903f472052675d9f3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateIsolines

## Description
This issue bundles 3 net-new root causes in the geo-routes CalculateIsolines success-response synthesis path (POST /v2/isolines). The step is a 200 whose body and response header content diverges from the recording. A fixer must close ALL THREE causes before submitting.

The open issue 20261029T172141Z-geo-routes-success-response-synthesis-diverges-across-calculateroutes-optimizewaypoints-and.md covers the equivalent faults for CalculateRoutes, OptimizeWaypoints and SnapToRoads only; CalculateIsolines is NOT in its cluster set or its Issue-Keys, so these causes are net-new and not covered by it.

Cause 1 of 3: CalculateIsolines never echoes the resolved DepartureTime, so the member is absent from the response.
AWS: Smithy: CalculateIsolines -> CalculateIsolinesResponse.DepartureTime targets TimestampWithTimezoneOffset (@sensitive, @pattern '^([1-2][0-9]{3})-(0[1-9]|1[0-2])-(0[1-9]|[12][0-9]|3[01])T([01][0-9]|2[0-3]):([0-5][0-9]):([0-5][0-9]|60)(\.[0-9]{0,9})?(Z|[+-]([01][0-9]|2[0-3]):[0-5][0-9])$'), the mirror of the input member CalculateIsolinesRequest.DepartureTime; the response also models ArrivalTime for the arrival-anchored form. Recorded behavior: when the request anchors the isoline in time (DepartureTime, or DepartNow which resolves to a concrete instant), the answer reports that resolved departure instant back, formatted as a local time with an explicit UTC offset to the pattern above (the recorded value is an offset-bearing timestamp, not a 'Z' instant).
Emulator: omits DepartureTime from the response body entirely (the key is absent, not null).
Why: an isoline is only meaningful relative to the instant it was computed for, because traffic-dependent reachability changes with departure time. A client that reads back DepartureTime to label, cache-key or re-request the isoline, or to confirm which instant DepartNow resolved to, has nothing to read and cannot tell what time basis the returned polygons describe.
Fix: the CalculateIsolines handler's response synthesis (resolve the departure instant from DepartureTime/DepartNow/Traffic and populate the output member), plus its serializer so the offset-bearing timestamp format is emitted.
Traces: geo-routes-calculate-isolines-distance-threshold#task#1.

Cause 2 of 3: SnappedOrigin is the submitted coordinate re-emitted through float arithmetic rather than the origin snapped onto the road network.
AWS: Smithy: CalculateIsolinesResponse.SnappedOrigin targets Position (list of Double, @length 2..2, @sensitive), the counterpart of the request's Origin; SnappedDestination is modelled the same way. Recorded behavior: the snapped position differs from the submitted Origin by a map-matching offset - it is the point on the road network the isoline is actually grown from - and is reported at the full coordinate precision of that matched point.
Emulator: returns the request's own Origin coordinate, re-serialized through binary floating point so it carries artefact digits and loses the trailing precision of the matched point instead of gaining a snapped one. Because the fixture's Origin is a fixed literal, this is a reproducible model output and not a per-run value.
Why: SnappedOrigin is how a client learns where the computation was actually anchored - the offset between its requested point and the road it was matched to. Echoing the input back makes that offset always zero, so a client cannot detect that its origin was off-network, and any rendering or distance accounting that measures from SnappedOrigin is measured from a point that is not on a road.
Fix: the CalculateIsolines handler's origin/destination snapping component (the same map-matching step the isoline geometry is grown from), not the serializer.
Traces: geo-routes-calculate-isolines-distance-threshold#task#1.

Cause 3 of 3: the x-amz-geo-pricing-bucket response header reports the wrong pricing bucket for CalculateIsolines.
AWS: Smithy: CalculateIsolinesResponse.PricingBucket is a String, @required, bound with @httpHeader('x-amz-geo-pricing-bucket'). The model does not enumerate the bucket names, so: recorded behavior only - CalculateIsolines is answered with the 'Advanced' bucket, and the bucket is a per-operation constant rather than a value derived from the request.
Emulator: answers 'Core' on the CalculateIsolines response. The header is present and plumbed, so the defect is the per-operation bucket mapping, not the header binding.
Why: clients and cost-attribution tooling read this header to assign a request to a pricing tier; reporting Core for an Advanced-tier operation under-reports spend and makes any emulator-driven cost assertion wrong.
Fix: the per-operation pricing-bucket mapping used by the CalculateIsolines response builder (the same mapping table the sibling issue flags for OptimizeWaypoints and SnapToRoads - CalculateIsolines must be corrected there too).
Traces: geo-routes-calculate-isolines-distance-threshold#task#1.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator (12 fixtures, 19 steps, 6 passed, 13 mismatched, score 31.6%). Emulator was up and serving (/__admin/health 200).

Causes 1-3 - fixture geo-routes-calculate-isolines-distance-threshold, step task#1, expected status 200 / actual status 200 (verdict mismatch, cluster affected_count 1):
  $.DepartureTime: missing - an offset-bearing local timestamp was expected (the recording's value is '2026-09-18T11:23:08-07:00'; the requirement is the resolved departure instant in that pattern, not that literal), got None (member absent)
  $.SnappedOrigin[0]: value_mismatch - expected a snapped longitude (recorded -122.3421045), got -122.34199999999998 (the submitted Origin longitude re-emitted through float arithmetic)
  $.SnappedOrigin[1]: value_mismatch - expected a snapped latitude (recorded 47.6097019), got 47.6097 (submitted Origin latitude, precision lost)
  $headers.x-amz-geo-pricing-bucket: value_mismatch - expected 'Advanced', got 'Core'. Both are fixed text for this operation: the bucket name is a per-operation constant with no variable part.

The status code itself is correct (200 expected, 200 returned) and no error code or error message is involved; the divergence is entirely in the synthesized body members and the pricing-bucket header. This cluster carries no stub_path (the operation is implemented) and no owned_by (no open issue carries its replay key).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture geo-routes-calculate-isolines-distance-threshold, step task#1 - a POST /v2/isolines with a fixed Origin, a distance Thresholds entry and a time anchor on the request.
3. Cause 1: inspect the 200 response body for the DepartureTime member. It is absent; it must carry the resolved departure instant formatted to the TimestampWithTimezoneOffset pattern (local time with an explicit UTC offset).
4. Cause 2: compare $.SnappedOrigin against the request's Origin array. The emulator returns the submitted coordinate (with float artefact digits); it must return the road-network-snapped position, which differs from the input.
5. Cause 3: inspect the x-amz-geo-pricing-bucket response header. The emulator sends 'Core'; CalculateIsolines must send 'Advanced'.
6. Re-replay the fixture after the fix: all four diverging keys ($.DepartureTime, $.SnappedOrigin[0], $.SnappedOrigin[1], $headers.x-amz-geo-pricing-bucket) must agree and the step must pass.
