Issue-Source: trace-sweep-divergence
Issue-Trace: geo-routes-optimize-waypoints-appointment-without-service-duration-rejected
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: geo-routes-optimize-waypoints-appointment-without-service-duration-rejected geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers
Issue-Shape-Id: com.amazonaws.georoutes#RoutesService
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.fieldList
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.message
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$.reason
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: geo-routes|OptimizeWaypoints|4xx->2xx|status
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.OptimizedWaypoints[*]
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$headers.x-amz-geo-pricing-bucket
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.Connections
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.ImpedingWaypoints[Id="bakery"].FailedConstraints[*]
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.ImpedingWaypoints[Id="pharmacy"].FailedConstraints[*]
Issue-Key: geo-routes|OptimizeWaypoints|2xx->2xx|$.OptimizedWaypoints
Issue-Related: 20261125T151644Z-geo-routes-optimizewaypoints-returns-the-requested-waypoint-order-rather-than.md
Issue-Commit: 47d3968c23878270cbb97f1f35a0ca51e5d34ce1

# OptimizeWaypoints diverges on appointment validation, impeding-waypoint reporting, default waypoint ids and pricing bucket

> _Note: this issue was identified against the code base at commit `47d3968c23878270cbb97f1f35a0ca51e5d34ce1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
OptimizeWaypoints

## Description
geo-routes answers OptimizeWaypoints (POST /v2/optimize-waypoints) with a 200 and a structurally plausible body in every case, but the parts of the contract that depend on the waypoint *constraints* in the request are not implemented: a request that AWS refuses with ValidationException is accepted, waypoints that cannot satisfy their constraints are reported as if the tour succeeded, waypoints the caller did not name are given emulator-invented ids, and the pricing tier reported in the x-amz-geo-pricing-bucket header is understated. This bundle carries FOUR causes.

=== Cause 1 of 4: a waypoint whose AppointmentTime has no ServiceDuration is accepted with 200 instead of being refused with a 400 ValidationException ===
AWS: Smithy com.amazonaws.georoutes#OptimizeWaypoints models ValidationException (errorKind client, httpError 400) with @required Message (jsonName "message"), @required Reason (jsonName "reason", enum ValidationExceptionReason = UnknownOperation|Missing|CannotParse|FieldValidationFailed|Other|UnknownField) and @required FieldList (jsonName "fieldList", list of ValidationExceptionField with @required Name/jsonName "name" and @required Message/jsonName "message"); the protocol also sets the x-amzn-errortype header to the error's shape name. WaypointOptimizationWaypoint carries both AppointmentTime (TimestampWithTimezoneOffset) and ServiceDuration (DurationSeconds, default 0), and ServiceDuration is documented "Service time spent at the waypoint. At an appointment, the service time should be the appointment duration" - i.e. the two members are coupled. The recorded response refuses the request with HTTP 400, reason "FieldValidationFailed", message "'Waypoints[0].ServiceDuration' must be set when 'Waypoints[0].AppointmentTime' is set." and a fieldList entry per offending waypoint ([{name: "Waypoints[0].ServiceDuration", message: "'Waypoints[0].ServiceDuration' must be set when 'Waypoints[0].AppointmentTime' is set."}, {name: "Waypoints[2].ServiceDuration", message: "'Waypoints[2].ServiceDuration' must be set when 'Waypoints[2].AppointmentTime' is set."}]). The message template is fixed text "' must be set when '" ... "' is set." wrapping two single-quoted member paths; the variable parts are the member paths themselves - the waypoint INDEX in Waypoints[<n>] comes from the request, one message and one fieldList entry per waypoint that sets AppointmentTime without ServiceDuration, and the top-level message is the first such entry's message. Indices 0 and 2 in the recording are that request's values, not part of the template.
Emulator: the request is accepted. The step returns HTTP 200 with a success body and no error payload at all - $.message, $.reason and $.fieldList are all absent (got None) and the x-amzn-errortype response header is not emitted. So the status (400 -> 200), the error code (ValidationException -> none) and the error message (the AppointmentTime/ServiceDuration coupling message -> none) all diverge; the validation rule is simply missing.
Why: a client that submits an appointment-constrained stop and forgets its service duration gets a plan back instead of the refusal AWS gives. The plan is built on an undefined dwell time, so every ArrivalTime/DepartureTime after that stop is fiction, and the caller has no error to surface to the user or to branch on - code that catches ValidationException and re-prompts for the missing duration never runs here, while it does against AWS.
Fix: OptimizeWaypoints' request validation in the geo-routes service crate - add the cross-member check "AppointmentTime set implies ServiceDuration set", collected over all Waypoints entries, and raise ValidationException with Reason FieldValidationFailed, the per-waypoint message above and one FieldList entry per offending index, serialized as a 400 with x-amzn-errortype.
Traces: geo-routes-optimize-waypoints-appointment-without-service-duration-rejected#task#2 (representative, affected_count 1).

=== Cause 2 of 4: waypoints submitted without an Id are given index ids ("0", "1", "2") instead of AWS's generated "Waypoint<n>" ids, and the returned sequence is not AWS's ===
AWS: WaypointOptimizationWaypoint.Id is OPTIONAL (WaypointId, @length 1..100, @pattern ^[^,;!]+$), while WaypointOptimizationOptimizedWaypoint.Id and WaypointOptimizationConnection's From/To are @required. When the caller omits Id, the service therefore has to synthesize one, and the recording shows the synthesized form is the literal prefix "Waypoint" followed by the waypoint's zero-based position in the request Waypoints list - "Waypoint0", "Waypoint1", "Waypoint2" (recorded behavior only; the model states the member is optional and the output ids required, but not the generated form). The same ids are used consistently in OptimizedWaypoints[*].Id and in Connections[*].From/.To, which chain Origin -> ... -> Destination in the optimized order; in the recording that order is Waypoint1, Waypoint0, Waypoint2.
Emulator: the ids are the bare decimal index - $.OptimizedWaypoints[1..3].Id come back "0", "2", "1" where the recording has "Waypoint1", "Waypoint0", "Waypoint2", and the Connections chain carries the same bare-index tokens ($.Connections[0].To "0" vs "Waypoint1", $.Connections[1].From/.To "0"/"2" vs "Waypoint1"/"Waypoint0", $.Connections[2].From/.To "2"/"1" vs "Waypoint0"/"Waypoint2"). The visiting order also differs from the recorded optimized order, so the Position/ArrivalTime/DepartureTime tuples line up against different list slots (the replay reports each OptimizedWaypoints entry as both missing and extra for that reason).
Why: the ids are the only handle a client has on an unnamed waypoint, and they are the join key between OptimizedWaypoints and Connections. A caller that correlates the returned sequence back to its input by the AWS-generated id ("Waypoint2") finds no such entry, and any stored manifest or log keyed on those ids is incompatible between emulator and AWS. Combined with the wrong visiting order, a dispatcher reading the plan sends the driver to the wrong stop first.
Fix: the OptimizeWaypoints handler in the geo-routes service crate - generate the default waypoint id as "Waypoint" + the request index when Waypoints[n].Id is absent, and use that same id everywhere the response names the waypoint (OptimizedWaypoints[*].Id, Connections[*].From/.To, ImpedingWaypoints[*].Id); the sequencing half is the same sequencing defect tracked for named waypoints in open issue 20261125T151644Z-geo-routes-optimizewaypoints-returns-the-requested-waypoint-order-rather-than.md, which does NOT own this cluster (replay reports owned_by empty) and says nothing about generated ids.
Traces: geo-routes-optimize-waypoints-appointment-without-service-duration-rejected#task#1 (affected_count 1).

=== Cause 3 of 4: the x-amz-geo-pricing-bucket header reports a lower tier than AWS for the same request ===
AWS: OptimizeWaypointsResponse.PricingBucket is @required and bound to the response header x-amz-geo-pricing-bucket (httpHeader). Its value is the pricing tier AWS bills the call under, and it is a function of the request's feature set, not of the result: the recorded response for a waypoint-optimization request carrying appointment/access-hours style constraints is "Premium" (recorded behavior only - the member is modeled as a plain String with no enumerated values, so the tier names and the rule mapping request features to a tier come from the recording).
Emulator: the header is emitted but always carries the lower tier - $headers.x-amz-geo-pricing-bucket is "Advanced" where the recording has "Premium", on both steps of the impeding-waypoints fixture. The header is present and well-formed, so the shape is right and only the tier classification diverges.
Why: this header is what cost-attribution and quota tooling keys on. A client that aggregates spend per pricing bucket, or that refuses to issue premium-tier calls, reads the call as a cheaper tier than AWS will bill, so budget checks pass that should fail and cost reports understate the premium traffic.
Fix: the OptimizeWaypoints handler's response serializer in the geo-routes service crate - derive PricingBucket from the request's feature set (the constraint-bearing waypoint options that push the call into the premium tier) instead of emitting a constant tier.
Traces: geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers#task#1 (affected_count 1); the same header mismatch recurs on #task#2 of that fixture, bundled as Cause 4.

=== Cause 4 of 4: when every waypoint is impeding, the emulator still returns a tour and reports only one failed constraint per waypoint ===
AWS: WaypointOptimizationImpedingWaypoint has @required Id, @required Position and @required FailedConstraints, a list of WaypointOptimizationFailedConstraint {Constraint (enum WaypointOptimizationConstraint = AccessHours|AppointmentTime|Before|Heading|ServiceDuration|SideOfStreet), Reason (free text)}. A waypoint that violates more than one constraint carries one entry PER violated constraint. The recording shows each impeding waypoint carrying a SECOND entry {Constraint: "ServiceDuration", Reason: "This waypoint cannot be reached in time."} - fixed text, no variable parts - in addition to its first failed constraint. The recording also shows that when no feasible tour exists because every waypoint impedes, the response reports the impediments ONLY: Connections and OptimizedWaypoints are absent from the body even though both are @required in the model, because there is no sequence to report.
Emulator: it reports a single FailedConstraints entry per impeding waypoint - the second entry {Constraint ServiceDuration, Reason "This waypoint cannot be reached in time."} is missing for every impeding waypoint in the response - and it nonetheless emits a full successful tour: $.Connections and $.OptimizedWaypoints are both present (a complete Origin -> waypoints -> Destination chain with distances, travel durations, arrival and departure times) where the recording has neither. The same step also carries the Cause 3 header divergence (x-amz-geo-pricing-bucket "Advanced" vs "Premium").
Why: the response says two contradictory things at once - "these waypoints cannot be served" and "here is the plan that serves them". A client that checks ImpedingWaypoints only when OptimizedWaypoints is empty, or that simply consumes OptimizedWaypoints when present, dispatches a route AWS declared infeasible. Dropping the second failed constraint also hides WHY the stop is unreachable, so a caller cannot tell the user which constraint to relax (extending the service duration vs widening the access hours) and cannot retry with a correction.
Fix: the OptimizeWaypoints handler and its response serializer in the geo-routes service crate - accumulate every violated constraint per waypoint into FailedConstraints (including the reachability/ServiceDuration failure with the wording above), and when no feasible sequence exists omit Connections and OptimizedWaypoints rather than synthesizing a tour over the impeding waypoints.
Traces: geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers#task#2 (affected_count 1).

Related open issues, none of which covers these four clusters (replay reports owned_by empty for all four): 20261125T151644Z-geo-routes-optimizewaypoints-returns-the-requested-waypoint-order-rather-than.md (OptimizeWaypoints sequencing and synthesized metrics on NAMED waypoints), 20261122T201204Z-geo-routes-calculateroutematrix-distances-and-durations-are-synthesized-geometric-estimates.md, 20261124T112417Z-geo-routes-calculateisolines-snappedorigin-is-not-the-road-snapped-position.md and 20261127T083515Z-geo-routes-road-snapping-still-synthesized-positions-quantized.md. Earlier issues touching the pricing-bucket header and OptimizeWaypoints validation wording are RESOLVED and so cover nothing here.

## Evidence
Replay of corpus simparity-wire-aws-v1 against the emulator built from this checkout: emulator_up true, bootstrap_signal false, 30 steps total, 10 passed, 20 mismatched, 14 divergence clusters. All four causes below are verdict "mismatch", service geo-routes, operation OptimizeWaypoints, stub_path empty and owned_by empty.

Cause 1 - fixture geo-routes-optimize-waypoints-appointment-without-service-duration-rejected, step task#2 (expected_status 400, actual_status 200):
  status: expected 400, got 200
  $.message: expected "'Waypoints[0].ServiceDuration' must be set when 'Waypoints[0].AppointmentTime' is set.", got None
  $.reason: expected 'FieldValidationFailed', got None
  $.fieldList: expected [{'name': 'Waypoints[0].ServiceDuration', 'message': "'Waypoints[0].ServiceDuration' must be set when 'Waypoints[0].AppointmentTime' is set."}, {'name': 'Waypoints[2].ServiceDuration', 'message': "'Waypoints[2].ServiceDuration' must be set when 'Waypoints[2].AppointmentTime' is set."}], got None
  $headers.x-amzn-errortype: expected present, got None
divergence_key: service=geo-routes|OptimizeWaypoints|status=400->200|keys=$.fieldList,$.message,$.reason,$headers.x-amzn-errortype,status; affected_count 1.
Requirement: HTTP 400, error ValidationException (x-amzn-errortype), reason FieldValidationFailed, and the message template "'Waypoints[<n>].ServiceDuration' must be set when 'Waypoints[<n>].AppointmentTime' is set." with <n> taken from the request - one message/fieldList entry per offending waypoint. The indices 0 and 2 are this recording's values.

Cause 2 - fixture geo-routes-optimize-waypoints-appointment-without-service-duration-rejected, step task#1 (200 -> 200, body only):
  $.Connections[0].To: expected 'Waypoint1', got '0'
  $.Connections[1].From: expected 'Waypoint1', got '0'
  $.Connections[1].To: expected 'Waypoint0', got '2'
  $.Connections[2].From: expected 'Waypoint0', got '2'
  $.Connections[2].To: expected 'Waypoint2', got '1'
  $.Connections[3].From: expected 'Waypoint2', got '1'
  $.OptimizedWaypoints[1..3]: reported both missing and extra - expected Ids 'Waypoint1','Waypoint0','Waypoint2' with Positions [-0.1357,51.5101], [-0.1195,51.5033], [-0.0997,51.5079]; got Ids '0','2','1' with Positions [-0.1195,51.5033], [-0.0997,51.5079], [-0.1357,51.5101]
affected_count 1. The id-to-position mapping shows the emulator's token is the bare request index while AWS's is "Waypoint" + the same index, and the slot order differs because the visiting sequence differs.

Cause 3 - fixture geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers, step task#1 (200 -> 200):
  $headers.x-amz-geo-pricing-bucket: expected 'Premium', got 'Advanced'
divergence_key: service=geo-routes|OptimizeWaypoints|status=200->200|keys=$headers.x-amz-geo-pricing-bucket; affected_count 1. Sole divergence in the cluster - the rest of that response replays clean.

Cause 4 - fixture geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers, step task#2 (200 -> 200):
  $.ImpedingWaypoints[Id="pharmacy"].FailedConstraints[1]: expected {'Constraint': 'ServiceDuration', 'Reason': 'This waypoint cannot be reached in time.'}, got None
  $.ImpedingWaypoints[Id="bakery"].FailedConstraints[1]: expected {'Constraint': 'ServiceDuration', 'Reason': 'This waypoint cannot be reached in time.'}, got None
  $.Connections: expected None, got [{From 'Origin' -> To 'pharmacy', ...}, {'pharmacy' -> 'bakery'}, {'bakery' -> 'Destination'}]
  $.OptimizedWaypoints: expected None, got a full 4-entry Origin/pharmacy/bakery/Destination tour with ArrivalTime/DepartureTime
  $headers.x-amz-geo-pricing-bucket: expected 'Premium', got 'Advanced'
affected_count 1. The waypoint ids, positions, metres, seconds and timestamps quoted above are that recording's values; the requirements are the rules - one FailedConstraints entry per violated constraint (including the reachability failure worded "This waypoint cannot be reached in time." against Constraint ServiceDuration), and no Connections/OptimizedWaypoints when no feasible sequence exists.

## Steps to Reproduce
1. git_clone() the repo and boot the emulator built from that checkout.
2. Replay the wire corpus: replay_wire_corpus().
3. Cause 1: replay fixture geo-routes-optimize-waypoints-appointment-without-service-duration-rejected, step task#2 - POST /v2/optimize-waypoints with Waypoints where entries 0 and 2 set AppointmentTime but omit ServiceDuration. Observe HTTP 200 with a success body; AWS answers HTTP 400 with x-amzn-errortype ValidationException, reason 'FieldValidationFailed', message "'Waypoints[0].ServiceDuration' must be set when 'Waypoints[0].AppointmentTime' is set." and a fieldList entry for each of Waypoints[0].ServiceDuration and Waypoints[2].ServiceDuration. Vary which waypoint index omits ServiceDuration and confirm the index in the message/fieldList follows the request.
4. Cause 2: replay the same fixture's step task#1 - a valid request whose Waypoints omit Id. Observe $.OptimizedWaypoints[*].Id and $.Connections[*].From/.To carrying '0','1','2' where AWS carries 'Waypoint0','Waypoint1','Waypoint2', and the visiting order differing from the recorded optimized order.
5. Cause 3: replay fixture geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers, step task#1 and read the x-amz-geo-pricing-bucket response header: 'Advanced' where the recording has 'Premium'.
6. Cause 4: replay step task#2 of that same fixture - a request in which every waypoint violates its constraints. Observe that each entry of $.ImpedingWaypoints has only one FailedConstraints element (the {Constraint: 'ServiceDuration', Reason: 'This waypoint cannot be reached in time.'} element is absent) and that $.Connections and $.OptimizedWaypoints are populated with a full tour, where the recording omits both; the pricing-bucket header is also 'Advanced' instead of 'Premium'.
7. The fix is confirmed when: the appointment-without-service-duration request is refused with the 400/ValidationException/FieldValidationFailed payload and message template above; unnamed waypoints are reported as 'Waypoint<index>' consistently across OptimizedWaypoints and Connections; the pricing bucket reflects the request's feature tier; an all-impeding request reports every failed constraint per waypoint and omits Connections/OptimizedWaypoints - and all four fixture steps (geo-routes-optimize-waypoints-appointment-without-service-duration-rejected#task#1 and #task#2, geo-routes-optimize-waypoints-every-waypoint-impeding-still-answers#task#1 and #task#2) replay clean.
