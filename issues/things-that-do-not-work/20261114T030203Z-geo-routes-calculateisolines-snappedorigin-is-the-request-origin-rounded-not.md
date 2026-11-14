Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-calculate-isolines-distance-threshold
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-calculate-isolines-distance-threshold
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateIsolines|2xx->2xx|$.SnappedOrigin[*]
Issue-Commit: 2fe7204e4697cf4de9a1d25ac1d449d622f376b5

# CalculateIsolines SnappedOrigin is the request origin rounded, not the road-snapped point

> _Note: this issue was identified against the code base at commit `2fe7204e4697cf4de9a1d25ac1d449d622f376b5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateIsolines

## Description
The geo-routes emulator answers CalculateIsolines with HTTP 200 and a structurally valid body (the Isolines themselves replay clean), but the SnappedOrigin it reports is not a point taken from the road network: it is the request's Origin coordinate re-emitted at reduced precision. This bundle has ONE cause.

Cause 1 of 1: SnappedOrigin is the request Origin truncated/rounded to 4 decimal places instead of the road-snapped position (fixture geo-routes-calculate-isolines-distance-threshold, step task#1).
AWS: Smithy com.amazonaws.georoutes#CalculateIsolines -> CalculateIsolinesResponse.SnappedOrigin, target `Position` (list of Double, @length min 2 max 2, @sensitive), documented as "The actual point on the road network used for calculations, which may differ from the requested origin if Origin was not directly on a road." So SnappedOrigin is a value derived from the road graph, carried at full geodetic precision (the recordings show 7 decimal places), and it is expected to differ laterally from the submitted Origin whenever the submitted Origin is off the carriageway. Operation is POST /v2/isolines, success code 200; modeled errors are ValidationException (400), AccessDeniedException (403), ThrottlingException (429), InternalServerException (500) — none is involved here, both sides are 200.
Emulator: SnappedOrigin[0] and SnappedOrigin[1] come back as the request's own Origin rounded to 4 decimal places, with no snapping applied at all, so the point sits wherever the caller put it (quantized) rather than on a road. The recorded response for the same request carries a 7-decimal position displaced from the request coordinate — i.e. a genuine snap.
Why: a client that uses SnappedOrigin to show the caller where the isoline computation actually started, or that feeds it back into a follow-up CalculateRoutes/CalculateIsolines call as a road-anchored waypoint, is handed an off-road point that is additionally quantized by up to ~10 m per coordinate. Because the response is a 200 with a plausible-looking Position, the error is silent: the client cannot tell that no snap occurred, and any "is my origin on a road?" check built on comparing Origin to SnappedOrigin always reports "already on a road".
Fix: the CalculateIsolines handler's response synthesis — resolve the request Origin against the road graph and emit the resulting full-precision position into SnappedOrigin (and, for the same reason, SnappedDestination) instead of echoing the rounded request coordinate. Note this is the same root defect class that the now-RESOLVED issue 20261103T145348Z-geo-routes-calculateisolines-response-synthesis-diverges-departuretime-omitted-origin-not.md reported for this operation ("origin not snapped"): DepartureTime and the pricing bucket now replay clean, but the snapping half is still not implemented, so this re-files it as a live divergence. The open issue 20261103T183228Z-geo-routes-route-synthesis-ignores-the-road-network.md describes the same rounding-instead-of-snapping behavior for CalculateRoutes/OptimizeWaypoints/SnapToRoads and does NOT cover CalculateIsolines.
Traces: geo-routes-calculate-isolines-distance-threshold#task#1.

## Evidence
Replay of fixture geo-routes-calculate-isolines-distance-threshold, step task#1: expected_status 200, actual_status 200 — the divergence is entirely in body values.

  $.SnappedOrigin[0]: value_mismatch (expected -122.3421045, got -122.342)
  $.SnappedOrigin[1]: value_mismatch (expected 47.6097019, got 47.6097)

The recorded numbers above are that recording's road position and are examples, not literal requirements. The requirement is the rule: SnappedOrigin must be the position produced by snapping the request Origin onto the road network, emitted at the full precision of that computed position (the recording carries 7 decimal places), not the request coordinate rounded to 4 decimals. Both the actual values here are exactly the request coordinate at 4 dp, which is what identifies the behavior as "no snap + rounding" rather than "a slightly different snap".

No status, error code or error message diverges for this step: the emulator returns 200 as AWS does, and no modeled error (ValidationException/AccessDeniedException/ThrottlingException/InternalServerException) is in play. divergence_key: service=geo-routes|CalculateIsolines|status=200->200|keys=$.SnappedOrigin[0],$.SnappedOrigin[1]; affected_count 1.

## Steps to Reproduce
1. Boot the emulator built from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture geo-routes-calculate-isolines-distance-threshold, step task#1 — POST /v2/isolines with the recorded request (Origin off the carriageway, Thresholds carrying a distance threshold).
3. The response is HTTP 200 and $.Isolines matches the recording; compare $.SnappedOrigin against the recorded response.
4. Observe $.SnappedOrigin equals the request's Origin rounded to 4 decimal places (got -122.342, 47.6097) where the recording carries the full-precision road-snapped point (expected -122.3421045, 47.6097019).
5. The fix is confirmed when SnappedOrigin is a position derived from the road graph at full precision rather than the quantized request coordinate, and this step replays clean.
