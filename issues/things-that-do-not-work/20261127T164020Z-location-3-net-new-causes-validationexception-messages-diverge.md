Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: location.batch-delete-device-position-history-spares-other-devices-and-accepts-unknown-ids location.batch-put-geofence-keys-each-refusal-to-its-entry location.device-position-history-rejects-unissued-next-token
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location.batch-delete-device-position-history-spares-other-devices-and-accepts-unknown-ids|run#13|AssertionError|assert got == ("ValidationException", r"'DeviceIds[1]' must match regex ^[-._\p{L}\p{N}]+$"), (
Issue-Key: location.batch-put-geofence-keys-each-refusal-to-its-entry|run#9|AssertionError|assert refused[gid].get("Message") == msg, (
Issue-Key: location.device-position-history-rejects-unissued-next-token|run#18|AssertionError|assert message == "'NextToken' must be a valid token.", (
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# location: 3 net-new causes — ValidationException messages diverge from AWS wording (list member paths rendered '<Member>.<1-based>.Member', BatchPutGeofence open-ring text, forged NextToken text)

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchDeleteDevicePositionHistory

## Description
Three independent refusal-text defects in the Location service crate, all surfacing on calls the emulator otherwise handles with the right HTTP status and error code. Related open issue 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit covers CreateTracker/DescribeTracker/GetDevicePosition/GetMapGlyphs refusal bodies — it does NOT cover any of the three operations below, which are net-new.

Cause 1 of 4 is not used; there are three causes.

Cause 1 of 3: BatchDeleteDevicePositionHistory renders a list element's member path as '<Member>.<1-based index>.Member' instead of AWS's '<Member>[<0-based index>]'.
AWS: Smithy: BatchDeleteDevicePositionHistory POST /tracking/v0/trackers/{TrackerName}/delete-positions; input member DeviceIds -> DeviceIdsList (@length 1..100), whose member targets Id with @length 1..100 and @pattern ^[-._\p{L}\p{N}]+$; modeled error ValidationException (httpError 400, required Message/Reason/FieldList). A DeviceIds element that violates the member pattern is refused 400 ValidationException whose message is the per-member sentence "'<path>' must match regex <pattern>": the fixed text is "'" ... "' must match regex ", the path is the modeled member name followed by the offending element's ZERO-BASED index in square brackets (so the second element is DeviceIds[1]), and the pattern is the one in the model. Nothing about the element's value is echoed.
Emulator: 400 ValidationException (status and code correct) with message "'DeviceIds.2.Member' must match regex ^[-._\p{L}\p{N}]+$" — a dotted path, a ONE-BASED index, and a literal trailing ".Member" segment that is not part of the API's member path vocabulary.
Why: a client that parses the quoted path to point an operator (or a form field) at the offending input is given a path no Location member path ever has, and the one-based index names a different element than the one that was refused; the same path is what AWS puts in ValidationException.FieldList[].name, so field-level highlighting is wrong too.
Fix: the location crate's shared input-constraint validation/message-rendering path for list members (the one BatchDeleteDevicePositionHistory's DeviceIds check goes through) — render a list element path as "<ModeledMemberName>[<zero-based index>]".
Traces: location.batch-delete-device-position-history-spares-other-devices-and-accepts-unknown-ids.

Cause 2 of 3: BatchPutGeofence's per-entry open-ring refusal message is re-worded.
AWS: Smithy: BatchPutGeofence POST /geofencing/v0/collections/{CollectionName}/put-geofences; each refused entry is reported in Errors[] as BatchPutGeofenceError{GeofenceId, Error: BatchItemError{Code: BatchItemErrorCode, Message}}; the ring list is LinearRings (@length min 1) whose member LinearRing has @length min 4. An entry whose Geometry.Polygon ring does not repeat its first position as its last is refused per entry (HTTP 200 overall) with Code "ValidationError" and Message "'Geometry.Polygon[<ring index>]' contains an open ring. Close the ring by ensuring the first and last positions are equal." — all of that is fixed text except the ring's zero-based bracketed index. The sibling winding refusal, "'Geometry.Polygon[0]' must be oriented counter-clockwise.", is already emitted verbatim by the emulator, so only the open-ring sentence diverges.
Emulator: same 200, same GeofenceId keying, same Code "ValidationError", but Message "'Geometry.Polygon[0]' must be closed: its first and last vertex must be the same."
Why: the per-entry Message is the only diagnosis a caller gets for a partially refused batch (the batch itself succeeds), so a client or test that matches/reports the service's sentence mis-classifies the entry; the emulator already gets the companion sentence right, which makes the inconsistency silent.
Fix: the BatchPutGeofence handler's polygon-geometry validation in the location service crate — emit the service's open-ring sentence.
Traces: location.batch-put-geofence-keys-each-refusal-to-its-entry.

Cause 3 of 3: GetDevicePositionHistory's refusal of a token it never issued uses emulator-authored text and leaks an internal pagination-collection name.
AWS: Smithy: GetDevicePositionHistory POST /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/list-positions, paginated (inputToken/outputToken NextToken, pageSize MaxResults); NextToken targets Token (@length 1..2000); modeled error ValidationException (httpError 400). A syntactically acceptable NextToken that the service did not issue is refused 400 ValidationException with message "'NextToken' must be a valid token." — entirely fixed text in the same per-member form as the service's other validation sentences: the quoted modeled member name plus " must be a valid token." No token value, device id, tracker name or collection name appears in it.
Emulator: 400 ValidationException (status and code correct) with message "NextToken is not a valid pagination token for device-positions" — the member name is not quoted, the sentence is re-worded, there is no terminating period, and it appends an emulator-internal pagination-collection identifier ("device-positions") that is not an API concept.
Why: a client that distinguishes an expired/forged page token from other validation failures by message text sees a sentence AWS never emits, and the response exposes emulator internals in a customer-visible message.
Fix: the pagination-token validation path the GetDevicePositionHistory handler calls in the location service crate — raise ValidationException with the service's fixed per-member sentence and no internal collection name.
Traces: location.device-position-history-rejects-unissued-next-token.

## Evidence
Cause 1 — check location.batch-delete-device-position-history-spares-other-devices-and-accepts-unknown-ids: BatchDeleteDevicePositionHistory with DeviceIds whose second entry contains a space. Expected (code, message) == ('ValidationException', "'DeviceIds[1]' must match regex ^[-._\\p{L}\\p{N}]+$"); got ('ValidationException', "'DeviceIds.2.Member' must match regex ^[-._\\p{L}\\p{N}]+$"). Fixed text "' ... ' must match regex " and the pattern are correct; the variable part (the member path) is wrong in form (dotted + ".Member") and in index base (1-based vs 0-based).

Cause 2 — check location.batch-put-geofence-keys-each-refusal-to-its-entry: BatchPutGeofence with four entries (closed CCW polygon, same square wound CW, circle, open CCW ring). Errors[] correctly names only the CW and the open-ring entries with Code 'ValidationError'; the CW entry's message matches AWS exactly. The open-ring entry's Error.Message was "'Geometry.Polygon[0]' must be closed: its first and last vertex must be the same.", expected "'Geometry.Polygon[0]' contains an open ring. Close the ring by ensuring the first and last positions are equal." Only the ring index inside the brackets is request-derived; the rest of the expected sentence is fixed text.

Cause 3 — check location.device-position-history-rejects-unissued-next-token: after a verified positive control (an issued NextToken resumes the two-sample history correctly), GetDevicePositionHistory(MaxResults=1, NextToken=<forged base64 token>) was refused with code 'ValidationException' (correct) and message 'NextToken is not a valid pagination token for device-positions', expected the fixed sentence "'NextToken' must be a valid token."

## Steps to Reproduce
1. Build and boot the emulator, then replay the three behavioral checks listed in trace_ids.
2. Cause 1: CreateTracker, then call BatchDeleteDevicePositionHistory(TrackerName=<tracker>, DeviceIds=["<valid-id>", "has a space"]). Read Error.Code and Error.Message from the 400: the message's quoted path must be 'DeviceIds[1]' (zero-based, square brackets), not 'DeviceIds.2.Member'.
3. Cause 2: CreateGeofenceCollection, then BatchPutGeofence with one entry whose Geometry.Polygon ring is counter-clockwise but whose last position does not repeat its first. The call returns 200; read Errors[].Error.Message for that GeofenceId and compare with "'Geometry.Polygon[0]' contains an open ring. Close the ring by ensuring the first and last positions are equal." (Compare the clockwise-ring entry in the same batch as a control — its message already matches AWS.)
4. Cause 3: CreateTracker with PositionFiltering=AccuracyBased, write two device positions with BatchUpdateDevicePosition, confirm an issued NextToken pages correctly, then call GetDevicePositionHistory(MaxResults=1, NextToken="ZGV2aWNlLWhpc3RvcnktZm9yZ2VkLXRva2Vu"). Compare the 400's message with "'NextToken' must be a valid token."
