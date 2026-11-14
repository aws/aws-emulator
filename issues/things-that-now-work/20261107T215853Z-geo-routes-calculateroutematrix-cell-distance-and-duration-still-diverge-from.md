Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-calculate-route-matrix-basic-success
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-calculate-route-matrix-basic-success geo-routes-calculate-route-matrix-cap-exceeded-rejected geo-routes-calculate-route-matrix-per-entry-error-codes
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RouteMatrix[*][*].Distance
Issue-Key: geo-routes|CalculateRouteMatrix|2xx->2xx|$.RouteMatrix[*][*].Duration
Issue-Commit: a77fa59e6e492b94323fed07c1655555de3da342

# CalculateRouteMatrix cell Distance and Duration still diverge from recorded road-network metrics at every matrix shape

> _Note: this issue was identified against the code base at commit `a77fa59e6e492b94323fed07c1655555de3da342`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CalculateRouteMatrix

## Description
CalculateRouteMatrix answers HTTP 200 with a structurally valid, correctly shaped matrix (RouteMatrix dimensions, RoutingBoundary echo, ErrorCount and per-cell Error codes all now agree with the recordings), but the two quantitative members of every successful cell are produced by a distance/duration model that is not the road-network model AWS answers with. In every cell of every replayed matrix the emulator's Distance is SHORTER than AWS's and its Duration is LONGER, so the implied average speed is far below a road speed and the two members are not even mutually consistent. The fault is one component -- the per-cell metric computation -- but it is reached on three independent matrix shapes, and this bundle therefore has THREE causes; all three must be closed before submitting.

NOTE ON PRIOR WORK: issue 20261029T172141Z-geo-routes-calculateroutematrix-matrix-synthesis-diverges-wrong-distance-duration-dropped.md is RESOLVED (status fixed) and so covers none of these failures; its per-entry-Error, ErrorCount and RoutingBoundary causes no longer reproduce, but its metric cause does, at all three matrix shapes below. The open issue 20261103T183228Z-geo-routes-route-synthesis-ignores-the-road-network.md covers the equivalent metric defect in CalculateRoutes/OptimizeWaypoints/SnapToRoads only and does not carry any CalculateRouteMatrix replay key; the shared road-graph traversal it asks for is the natural place to fix this operation too, but closing it does not close this one.

Cause 1 of 3: square 2x2 matrix -- every cell's Distance and Duration diverge (fixture geo-routes-calculate-route-matrix-basic-success, step task#1).
AWS: Smithy georoutes#CalculateRouteMatrix -> POST /v2/route-matrix, 200; CalculateRouteMatrixResponse.RouteMatrix @required (list of rows of RouteMatrixEntry, "Each row corresponds to one entry in Origins. Each entry in the row corresponds to the route from that entry in Origins to an entry in Destination positions"). RouteMatrixEntry.Distance (DistanceMeters, long, @required, @range 0..4294967295, sensitive) is "The total distance of travel for the route" and RouteMatrixEntry.Duration (DurationSeconds, long, @required, same range) is "The expected duration of travel for the route", unit seconds. The model does not specify the routing algorithm, so: recorded behavior only -- for a fixed Origins x Destinations set the recorded answer is a deterministic per-cell pair whose distance and duration are mutually consistent at a road-network average speed, and the cell ordering by duration follows the ordering by distance.
Emulator: for the same fixed fixture coordinates it returns, in all four cells, a distance roughly 20-26% short of the recorded one together with a duration 14-41% longer than the recorded one. These are reproducible values, not per-run noise: the request coordinates are fixed in the fixture, so this is simply a different (and internally inconsistent) metric model.
Why: a client that sizes a delivery window, picks a nearest depot, or filters origin/destination pairs against a duration budget gets both different ETAs and a different cell ranking than it gets from AWS; the duration inflation alone pushes pairs past client-side time limits that AWS satisfies, while the short distance under-bills distance-based cost models.
Fix: the CalculateRouteMatrix handler's per-cell metric computation (the matrix-synthesis component), which should derive Distance/Duration from the same road-graph traversal the other route-metric producers use -- not the serializer and not the response builder, both of which are now correct.
Traces: geo-routes-calculate-route-matrix-basic-success#task#1.

Cause 2 of 3: large 5x5 matrix -- all 25 cells diverge the same way, so the defect scales with the matrix rather than being an edge case of the small one (fixture geo-routes-calculate-route-matrix-cap-exceeded-rejected, step task#2).
AWS: as Cause 1; additionally the recorded long-haul cells show the same distance/duration consistency at long range (tens to hundreds of kilometres), i.e. the metric model does not change regime with matrix size or trip length. Recorded behavior only.
Emulator: every one of the 25 cells carries a short distance and a long duration, with the same sign of error as Cause 1 (e.g. first-row cells are ~20-25% short on distance and ~21-35% long on duration). This is the regression case that shows the divergence is systematic across the whole matrix, not confined to the first row/column, and that no per-cell path escapes it.
Why: a client that uses a large matrix to solve an assignment or clustering problem accumulates the per-cell error across the whole solution, so the chosen assignment differs from the one AWS's matrix produces even where individual cells look plausible.
Fix: same component as Cause 1; this step is its large-matrix regression case and must be re-replayed to confirm no cell is left on the old model.
Traces: geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#2.

Cause 3 of 3: matrix that mixes routable and unroutable pairs -- the routable cells still carry the diverging metrics (fixture geo-routes-calculate-route-matrix-per-entry-error-codes, step task#2).
AWS: as Cause 1, for the cells that DO have a connecting route; the cells with no route carry RouteMatrixEntry.Error (RouteMatrixErrorCode, e.g. NoRoute) and no meaningful metrics, and ErrorCount @required counts them. Recorded behavior only for the metric values.
Emulator: the error reporting on this step now matches the recording (per-cell Error and ErrorCount agree), but the two routable cells remain on the diverging metric model -- distance ~17% short and duration ~79-83% long versus the recording, a much larger duration error than in Causes 1 and 2.
Why: a client that correctly skips the failed cells and plans from the surviving ones is still planning on wrong numbers, and the larger duration error here means the metric model's divergence is not a constant factor a client could compensate for.
Fix: same component as Cause 1; this step is the mixed-success/failure regression case, confirming the metric path is wrong independently of the per-cell error path that was previously repaired.
Traces: geo-routes-calculate-route-matrix-per-entry-error-codes#task#2.

## Evidence
All three causes replay with expected_status 200 and actual_status 200; the divergence is entirely in response body values. The numbers below are the recorded fixture values and are EXAMPLES of the rule (a road-network distance with a mutually consistent duration), not literal requirements -- a fix must reproduce AWS's metric model, not these specific integers.

Cause 1 -- geo-routes-calculate-route-matrix-basic-success#task#1 (2 Origins x 2 Destinations), 200 -> 200:
  $.RouteMatrix[0][0].Distance: expected 22249, got 17458
  $.RouteMatrix[0][0].Duration: expected 976, got 1256
  $.RouteMatrix[0][1].Distance: expected 54104, got 40193
  $.RouteMatrix[0][1].Duration: expected 2047, got 2892
  $.RouteMatrix[1][0].Distance: expected 27518, got 19707
  $.RouteMatrix[1][0].Duration: expected 1247, got 1418
  $.RouteMatrix[1][1].Distance: expected 59553, got 43893
  $.RouteMatrix[1][1].Duration: expected 2354, got 3158
  (distance always short, duration always long -> a different, internally inconsistent speed model)

Cause 2 -- geo-routes-calculate-route-matrix-cap-exceeded-rejected#task#2 (5 x 5), 200 -> 200: all 25 cells diverge, e.g.
  $.RouteMatrix[0][0].Distance: expected 172242, got 133704; $.RouteMatrix[0][0].Duration: expected 7918, got 9619
  $.RouteMatrix[0][4].Distance: expected 189620, got 144365; $.RouteMatrix[0][4].Duration: expected 7674, got 10386
  $.RouteMatrix[1][0].Distance: expected 172409, got 131022; $.RouteMatrix[1][0].Duration: expected 8085, got 9426
  $.RouteMatrix[2][2].Distance: expected 186587, got 133672; $.RouteMatrix[2][2].Duration: expected 7601, got 9617
  (the replay detail enumerates the remaining cells of rows 0-4 with the same sign of error)

Cause 3 -- geo-routes-calculate-route-matrix-per-entry-error-codes#task#2, 200 -> 200:
  $.RouteMatrix[0][0].Distance: expected 280490, got 234016
  $.RouteMatrix[0][0].Duration: expected 9425, got 16836
  $.RouteMatrix[0][1].Distance: expected 1303955, got 1093216
  $.RouteMatrix[0][1].Duration: expected 42904, got 78649
  No $.ErrorCount, $.RouteMatrix[*][*].Error or $.RoutingBoundary divergence is reported on this step any more: only the metrics of the routable cells remain wrong.

No status, error code or error message diverges in any of the three causes -- every step is a 200 on both sides.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Cause 1: replay fixture geo-routes-calculate-route-matrix-basic-success, step task#1 (POST /v2/route-matrix, 2 Origins x 2 Destinations) and compare every $.RouteMatrix[i][j].Distance and .Duration against the recorded response. All four cells diverge, with distance short and duration long.
3. Cause 2: replay fixture geo-routes-calculate-route-matrix-cap-exceeded-rejected, step task#2 (5 x 5 matrix) and compare all 25 cells' Distance/Duration; confirm no row or column escapes the divergence.
4. Cause 3: replay fixture geo-routes-calculate-route-matrix-per-entry-error-codes, step task#2 (a matrix mixing routable pairs with pairs that have no connecting route) and compare the routable cells' Distance/Duration; the per-cell Error values and $.ErrorCount already match, so only the metrics diverge here.
5. All three steps return HTTP 200 both from AWS and from the emulator; the divergence is purely in the synthesized per-cell metrics. A fix is confirmed only when all three steps are re-replayed clean, since each exercises a different matrix shape through the same metric component.
