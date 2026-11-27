Issue-Source: trace-sweep-divergence
Issue-Trace: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry location-geofence-batch-entry-refusals
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|BatchPutGeofence|2xx->2xx|$.Errors[*].Error.Message
Issue-Key: location|BatchPutGeofence|2xx->2xx|$.Errors[*]
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# location BatchPutGeofence per-entry ValidationError messages are re-worded

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutGeofence

## Description
TWO causes, one shared fault: BatchPutGeofence correctly refuses the offending entries per entry (HTTP 200 overall, the right GeofenceId keying, Code "ValidationError"), but the Message it puts in each entry's error is emulator-authored rather than the sentence Amazon Location emits for that geometry rule. The per-entry Message is the ONLY diagnosis a caller gets for a partially refused batch, because the batch call itself succeeds.

Shared model basis: Smithy: BatchPutGeofence POST /geofencing/v0/collections/{CollectionName}/put-geofences; each refused entry is reported in Errors[] as BatchPutGeofenceError{GeofenceId, Error: BatchItemError{Code: BatchItemErrorCode, Message}}; the geometry is GeofenceGeometry{Polygon, MultiPolygon, Circle, Geobuf} and its documentation states that "Including multiple geometry types in the same request will return a validation error" and that Circle.Radius "[m]ust be greater than zero and no larger than 100,000 (100 kilometers)". The sentences below are the service's recorded wording for those two rules.

Related open issue 20261127T164020Z-location-3-net-new-causes-validationexception-messages-diverge covers BatchPutGeofence's OPEN-RING entry sentence from the behavioral corpus; the two sentences below are different rules and are not covered by it.

Cause 1 of 2: the mixed-geometry entry refusal is re-worded and loses the enumeration of the mutually exclusive members.
AWS: an entry whose Geometry sets more than one of Polygon/Circle/MultiPolygon/Geobuf is refused per entry with Code "ValidationError" and Message "Only one of the following fields may be set within 'Geometry': 'Circle', 'Polygon', 'MultiPolygon', 'Geobuf'." — entirely fixed text: the sentence names the containing member ('Geometry') and lists the four mutually exclusive members in that order, each individually quoted, and ends with a period. Nothing in it is request-derived.
Emulator: same 200, same GeofenceId, same Code "ValidationError", but Message "'Geometry' must specify only one of [Polygon, Circle, MultiPolygon, Geobuf]." — a different sentence shape (member-first, bracketed unquoted list, different member order).
Why: a client or operator tool that matches or surfaces the service's sentence to explain WHICH entry of the batch was dropped and why sees text AWS never emits, so the entry is mis-classified even though the batch-level shape is right.
Fix: the BatchPutGeofence handler's geometry validation in the location service crate — emit the service's mixed-geometry sentence verbatim.
Traces: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry#task#5.

Cause 2 of 2: the Circle.Radius refusal states the upper bound instead of the rule that was broken.
AWS: an entry whose Geometry.Circle.Radius is not greater than zero is refused per entry with Code "ValidationError" and Message "'Geometry.Circle.Radius' must be greater than 0 m." — fixed text, quoting the full dotted member path and ending with the unit abbreviation and a period. The sentence states only the violated lower bound; it does not restate the 100,000 m ceiling.
Emulator: same 200, same GeofenceId, same Code, but Message "'Geometry.Circle.Radius' must be greater than 0 and no larger than 100000." — the member path is right, the sentence folds in the ceiling and drops the unit.
Why: same as Cause 1 — the per-entry Message is the whole diagnosis for a partially refused batch, so wording that AWS never emits breaks text-matching clients and misreports which bound was violated.
Fix: the BatchPutGeofence handler's circle-geometry validation in the location service crate — emit the lower-bound sentence with the unit, keeping the dotted member path it already renders correctly.
Traces: location-geofence-batch-entry-refusals#task#2.

## Evidence
Cause 1 — location-batch-put-geofence-rejects-entry-count-and-mixed-geometry#task#5 (BatchPutGeofence, 200 both sides): $.Errors[0].Error.Message expected "Only one of the following fields may be set within 'Geometry': 'Circle', 'Polygon', 'MultiPolygon', 'Geobuf'.", got "'Geometry' must specify only one of [Polygon, Circle, MultiPolygon, Geobuf]." The GeofenceId keying and Error.Code ('ValidationError') matched.
Cause 2 — location-geofence-batch-entry-refusals#task#2 (BatchPutGeofence, 200 both sides): $.Errors[1] expected {'GeofenceId': <the zero-radius entry's id>, 'Error': {'Code': 'ValidationError', 'Message': "'Geometry.Circle.Radius' must be greater than 0 m."}}, got the same GeofenceId and Code with Message "'Geometry.Circle.Radius' must be greater than 0 and no larger than 100000." Both expected sentences are fixed text end to end; only the GeofenceId that keys the entry is request-derived, and that part already matches.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay location-batch-put-geofence-rejects-entry-count-and-mixed-geometry and location-geofence-batch-entry-refusals.
2. Cause 1: CreateGeofenceCollection, then BatchPutGeofence with one entry whose Geometry sets BOTH Polygon and Circle. The call returns 200; read Errors[].Error.Message for that GeofenceId and compare with "Only one of the following fields may be set within 'Geometry': 'Circle', 'Polygon', 'MultiPolygon', 'Geobuf'."
3. Cause 2: BatchPutGeofence with one entry whose Geometry.Circle has Radius 0. The call returns 200; compare that entry's Error.Message with "'Geometry.Circle.Radius' must be greater than 0 m."
4. In both cases Error.Code ('ValidationError') and the GeofenceId keying already match — only the Message text diverges.
