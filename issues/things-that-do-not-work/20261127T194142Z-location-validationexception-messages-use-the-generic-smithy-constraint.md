Issue-Source: trace-sweep-divergence
Issue-Trace: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry location-device-position-batch-update-over-ten-rejected location-device-position-id-validation location-geofence-evaluate-positions-and-dead-collection location-geofence-forecast-mismatched-units-rejected location-geofence-get-id-validation location-list-geofences-rejects-out-of-range-page-size location-map-style-descriptor-rejects-malformed-map-name location-search-place-index-for-position-rejects-out-of-range-input location-tracker-position-history-and-list-rejections
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|BatchPutGeofence|4xx->4xx|$.fieldList
Issue-Key: location|BatchPutGeofence|4xx->4xx|$.message
Issue-Key: location|BatchPutGeofence|4xx->4xx|$.reason
Issue-Key: location|BatchUpdateDevicePosition|4xx->4xx|$.fieldList
Issue-Key: location|BatchUpdateDevicePosition|4xx->4xx|$.message
Issue-Key: location|BatchUpdateDevicePosition|4xx->4xx|$.reason
Issue-Key: location|GetDevicePosition|4xx->4xx|$.fieldList
Issue-Key: location|GetDevicePosition|4xx->4xx|$.reason
Issue-Key: location|BatchEvaluateGeofences|4xx->4xx|$.fieldList
Issue-Key: location|BatchEvaluateGeofences|4xx->4xx|$.message
Issue-Key: location|BatchEvaluateGeofences|4xx->4xx|$.reason
Issue-Key: location|ForecastGeofenceEvents|4xx->4xx|$.fieldList
Issue-Key: location|ForecastGeofenceEvents|4xx->4xx|$.message
Issue-Key: location|ForecastGeofenceEvents|4xx->4xx|$.reason
Issue-Key: location|GetGeofence|4xx->4xx|$.fieldList
Issue-Key: location|GetGeofence|4xx->4xx|$.message
Issue-Key: location|GetGeofence|4xx->4xx|$.reason
Issue-Key: location|ListGeofences|4xx->4xx|$.fieldList
Issue-Key: location|ListGeofences|4xx->4xx|$.message
Issue-Key: location|ListGeofences|4xx->4xx|$.reason
Issue-Key: location|GetMapStyleDescriptor|4xx->4xx|$.fieldList
Issue-Key: location|GetMapStyleDescriptor|4xx->4xx|$.message
Issue-Key: location|GetMapStyleDescriptor|4xx->4xx|$.reason
Issue-Key: location|SearchPlaceIndexForPosition|4xx->4xx|$.fieldList
Issue-Key: location|SearchPlaceIndexForPosition|4xx->4xx|$.message
Issue-Key: location|SearchPlaceIndexForPosition|4xx->4xx|$.reason
Issue-Key: location|ListDevicePositions|4xx->4xx|$.fieldList
Issue-Key: location|ListDevicePositions|4xx->4xx|$.message
Issue-Key: location|ListDevicePositions|4xx->4xx|$.reason
Issue-Related: 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit.md
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# location ValidationException messages use the generic Smithy constraint preamble instead of the service's per-member sentence

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchPutGeofence

## Description
TEN causes, one shared fault. Every modeled @length/@range violation in the location crate is refused with the generic Smithy constraint-validator sentence ("<n> validation error detected: Value '<value>' at '<lowerCamelMember>' failed to satisfy constraint: Member must ...") instead of Amazon Location's own per-member sentence, and in every one of the ten the response also omits the two ValidationException members the model marks required. The HTTP status (400) and the error code (ValidationException) are correct in all ten; only the body is wrong.

Shared model basis (Smithy: com.amazonaws.location#ValidationException, httpError 400) — it has THREE required members: Message (jsonName "message"), Reason (jsonName "reason", enum ValidationExceptionReason whose values include FieldValidationFailed) and FieldList (jsonName "fieldList", list of ValidationExceptionField{Name required, Message required}). For an input-constraint violation AWS always sets reason = "FieldValidationFailed" and fieldList = one entry per offending member, {name: <the modeled member path>, message: <the same sentence as the top-level message>}.

Shared AWS message templates (fixed text, with only the member name and the modeled limit substituted; the offending VALUE is never echoed):
  - string @length max: "'<Member>' must have length at most <max>"
  - list @length min/max: "'<Member>' must have length at least <min>" / "'<Member>' must have length at most <max>"
  - integer @range max/min: "'<Member>' must not be greater than <max>" / "'<Member>' must not be less than <min>"
The quoted name is the member as MODELLED (PascalCase), and the number comes from the model's constraint, not from the request.

Related open issue 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit records this same fault for GetMapGlyphs' FontUnicodeRange pattern only; none of the ten operation/member pairs below is covered by it, and the already-owned GetMapGlyphs cluster is deliberately not repeated here.

Cause 1 of 10: BatchPutGeofence with an empty Entries list.
AWS: Smithy: BatchPutGeofence POST /geofencing/v0/collections/{CollectionName}/put-geofences; input member Entries -> BatchPutGeofenceRequestEntryList, required, @length 1..10. An empty list is refused 400 ValidationException with message "'Entries' must have length at least 1", reason "FieldValidationFailed" and fieldList [{name: "Entries", message: "'Entries' must have length at least 1"}].
Emulator: 400 with message "1 validation error detected: Value '0 items' at 'entries' failed to satisfy constraint: Member must have length greater than or equal to 1", and reason/fieldList absent entirely.
Why: a strict SDK deserialization of ValidationException has two required members missing (null/absent where an enum and a list are mandatory), and a client that routes on reason == FieldValidationFailed or highlights fieldList[].name has nothing to read; the message is also lower-cased to the wire spelling and wrapped in a preamble the service never emits.
Fix: the location crate's shared request-validation + ValidationException serializer path (the one that renders constraint failures for every operation), not this handler alone.
Traces: location-batch-put-geofence-rejects-entry-count-and-mixed-geometry#task#3 (same cluster: #task#4).

Cause 2 of 10: BatchUpdateDevicePosition with more Updates than the list allows.
AWS: Smithy: BatchUpdateDevicePosition POST /tracking/v0/trackers/{TrackerName}/positions; Updates -> DevicePositionUpdateList, required, @length 1..10. An over-long list is refused 400 ValidationException with message "'Updates' must have length at most 10", reason "FieldValidationFailed", fieldList [{name: "Updates", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<n> items' at 'updates' failed to satisfy constraint: Member must have length less than or equal to 10"; no reason, no fieldList.
Why / Fix: as Cause 1 — same shared validator path.
Traces: location-device-position-batch-update-over-ten-rejected#task#3 (same cluster: #task#4).

Cause 3 of 10: GetDevicePosition with an over-long DeviceId.
AWS: Smithy: GetDevicePosition GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest; DeviceId -> Id, @httpLabel, required, @length 1..100. Over 100 characters is refused 400 ValidationException with message "'DeviceId' must have length at most 100", reason "FieldValidationFailed", fieldList [{name: "DeviceId", message: <same sentence>}]. The offending id is NOT echoed.
Emulator: 400 with "1 validation error detected: Value '<the whole 101-char id>' at 'deviceId' failed to satisfy constraint: Member must have length less than or equal to 100"; no reason, no fieldList — and it echoes the rejected value AWS keeps out of the sentence.
Why / Fix: as Cause 1.
Traces: location-device-position-id-validation#task#4.

Cause 4 of 10: BatchEvaluateGeofences with more DevicePositionUpdates than allowed.
AWS: Smithy: BatchEvaluateGeofences POST /geofencing/v0/collections/{CollectionName}/positions; DevicePositionUpdates -> DevicePositionUpdateList, required, @length 1..10. Refused 400 with message "'DevicePositionUpdates' must have length at most 10", reason "FieldValidationFailed", fieldList [{name: "DevicePositionUpdates", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<n> items' at 'devicePositionUpdates' failed to satisfy constraint: Member must have length less than or equal to 10"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-geofence-evaluate-positions-and-dead-collection#task#5.

Cause 5 of 10: ForecastGeofenceEvents with MaxResults above its range.
AWS: Smithy: ForecastGeofenceEvents POST /geofencing/v0/collections/{CollectionName}/forecast-geofence-events; MaxResults -> Integer @range 1..20. Above 20 is refused 400 with message "'MaxResults' must not be greater than 20", reason "FieldValidationFailed", fieldList [{name: "MaxResults", message: <same sentence>}]. Note the range form of the sentence ("must not be greater than <max>"), which differs from the length form.
Emulator: 400 with "1 validation error detected: Value '<n>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-geofence-forecast-mismatched-units-rejected#task#5.

Cause 6 of 10: GetGeofence with an over-long GeofenceId.
AWS: Smithy: GetGeofence GET /geofencing/v0/collections/{CollectionName}/geofences/{GeofenceId}; GeofenceId -> Id, @httpLabel, required, @length 1..100. Refused 400 with message "'GeofenceId' must have length at most 100", reason "FieldValidationFailed", fieldList [{name: "GeofenceId", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<the rejected id>' at 'geofenceId' failed to satisfy constraint: Member must have length less than or equal to 100"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-geofence-get-id-validation#task#4.

Cause 7 of 10: ListGeofences with MaxResults above its range.
AWS: Smithy: ListGeofences POST /geofencing/v0/collections/{CollectionName}/list-geofences; MaxResults -> Integer @range 1..100. Refused 400 with message "'MaxResults' must not be greater than 100", reason "FieldValidationFailed", fieldList [{name: "MaxResults", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<n>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-list-geofences-rejects-out-of-range-page-size#task#4 (same cluster: #task#6, #task#7).

Cause 8 of 10: GetMapStyleDescriptor with an over-long MapName.
AWS: Smithy: GetMapStyleDescriptor GET /maps/v0/maps/{MapName}/style-descriptor; MapName -> ResourceName, @httpLabel, required, @length 1..100. Refused 400 with message "'MapName' must have length at most 100", reason "FieldValidationFailed", fieldList [{name: "MapName", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<the rejected map name>' at 'mapName' failed to satisfy constraint: Member must have length less than or equal to 100"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-map-style-descriptor-rejects-malformed-map-name#task#3.

Cause 9 of 10: SearchPlaceIndexForPosition with MaxResults below its range.
AWS: Smithy: SearchPlaceIndexForPosition POST /places/v0/indexes/{IndexName}/search/position; MaxResults -> PlaceIndexSearchResultLimit @range 1..50. Below 1 is refused 400 with message "'MaxResults' must not be less than 1", reason "FieldValidationFailed", fieldList [{name: "MaxResults", message: <same sentence>}] — the lower-bound form of the range sentence.
Emulator: 400 with "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-search-place-index-for-position-rejects-out-of-range-input#task#5.

Cause 10 of 10: ListDevicePositions with MaxResults above its range.
AWS: Smithy: ListDevicePositions POST /tracking/v0/trackers/{TrackerName}/list-positions; MaxResults -> Integer @range 1..100. Refused 400 with message "'MaxResults' must not be greater than 100", reason "FieldValidationFailed", fieldList [{name: "MaxResults", message: <same sentence>}].
Emulator: 400 with "1 validation error detected: Value '<n>' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100"; no reason, no fieldList.
Why / Fix: as Cause 1.
Traces: location-tracker-position-history-and-list-rejections#task#6.

## Evidence
All ten clusters: expected status 400, actual status 400; error code ValidationException on both sides. In every one, $.reason expected 'FieldValidationFailed' got None, and $.fieldList expected a one-entry list [{name: <member>, message: <the sentence>}] got None. Message diffs (expected -> actual):
1. location-batch-put-geofence-rejects-entry-count-and-mixed-geometry#task#3 (BatchPutGeofence): "'Entries' must have length at least 1" -> "1 validation error detected: Value '0 items' at 'entries' failed to satisfy constraint: Member must have length greater than or equal to 1".
2. location-device-position-batch-update-over-ten-rejected#task#3 (BatchUpdateDevicePosition): "'Updates' must have length at most 10" -> "1 validation error detected: Value '11 items' at 'updates' failed to satisfy constraint: Member must have length less than or equal to 10".
3. location-device-position-id-validation#task#4 (GetDevicePosition): "'DeviceId' must have length at most 100" -> "1 validation error detected: Value 'dev-aaaa...a' at 'deviceId' failed to satisfy constraint: Member must have length less than or equal to 100".
4. location-geofence-evaluate-positions-and-dead-collection#task#5 (BatchEvaluateGeofences): "'DevicePositionUpdates' must have length at most 10" -> "1 validation error detected: Value '11 items' at 'devicePositionUpdates' failed to satisfy constraint: Member must have length less than or equal to 10".
5. location-geofence-forecast-mismatched-units-rejected#task#5 (ForecastGeofenceEvents): "'MaxResults' must not be greater than 20" -> "1 validation error detected: Value '21' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 20".
6. location-geofence-get-id-validation#task#4 (GetGeofence): "'GeofenceId' must have length at most 100" -> "1 validation error detected: Value 'qqq...q' at 'geofenceId' failed to satisfy constraint: Member must have length less than or equal to 100".
7. location-list-geofences-rejects-out-of-range-page-size#task#4 (ListGeofences): "'MaxResults' must not be greater than 100" -> "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
8. location-map-style-descriptor-rejects-malformed-map-name#task#3 (GetMapStyleDescriptor): "'MapName' must have length at most 100" -> "1 validation error detected: Value 'sim2real-probe-location-style-overlong-...' at 'mapName' failed to satisfy constraint: Member must have length less than or equal to 100".
9. location-search-place-index-for-position-rejects-out-of-range-input#task#5 (SearchPlaceIndexForPosition): "'MaxResults' must not be less than 1" -> "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1".
10. location-tracker-position-history-and-list-rejections#task#6 (ListDevicePositions): "'MaxResults' must not be greater than 100" -> "1 validation error detected: Value '101' at 'maxResults' failed to satisfy constraint: Member must have value less than or equal to 100".
In each expected message the fixed text is the quoted modeled member name plus " must have length at most/at least " or " must not be greater/less than "; the only substituted parts are the member name and the model's own limit. The actual messages additionally echo the rejected request value, which AWS's sentences never contain.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the ten fixtures named in Traces/trace_ids.
2. For a list bound, e.g. BatchPutGeofence(CollectionName=<existing collection>, Entries=[]) and BatchUpdateDevicePosition(TrackerName=<tracker>, Updates=[<11 updates>]).
3. For a string length bound, e.g. GetDevicePosition(TrackerName=<tracker>, DeviceId=<101 chars>) and GetGeofence(CollectionName=<collection>, GeofenceId=<101 chars>).
4. For an integer range bound, e.g. ListGeofences(MaxResults=101), ListDevicePositions(MaxResults=101), ForecastGeofenceEvents(MaxResults=21), SearchPlaceIndexForPosition(MaxResults=0).
5. In every 400 body check three things: message equals the service's per-member sentence (quoted PascalCase member name + the model's limit, with no request value in it), reason equals 'FieldValidationFailed', and fieldList has one entry whose name is the member and whose message repeats the sentence.
