Issue-Source: trace-sweep-divergence
Issue-Trace: location-device-position-id-validation
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-device-position-id-validation location-geofence-get-id-validation location-map-style-descriptor-rejects-malformed-map-name location-start-job-request-validation location-batch-get-device-position-omits-unknown-device
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|StartJob|4xx->4xx|$.fieldList
Issue-Key: location|StartJob|4xx->4xx|$.reason
Issue-Key: location|BatchGetDevicePosition|4xx->4xx|$.fieldList
Issue-Key: location|BatchGetDevicePosition|4xx->4xx|$.message
Issue-Key: location|BatchGetDevicePosition|4xx->4xx|$.reason
Issue-Key: location|StartJob|4xx->4xx|$.message
Issue-Related: 20261127T194142Z-location-validationexception-messages-use-the-generic-smithy-constraint.md
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# location ValidationException bodies omit required reason and fieldList and misrender list/nested member paths

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDevicePosition

## Description
SIX causes, one shared fault in the location crate's ValidationException construction. In the first four the per-member message text is already correct and ONLY the two required members Reason and FieldList are missing; in the last two the member PATH the message quotes (and that fieldList[].name would carry) is rendered in a vocabulary Location never uses. Status 400 and error code ValidationException are correct in all six.

Shared model basis: Smithy: com.amazonaws.location#ValidationException (httpError 400) requires Message (jsonName "message"), Reason (jsonName "reason", ValidationExceptionReason — FieldValidationFailed for an input-constraint failure) and FieldList (jsonName "fieldList", list of ValidationExceptionField{Name, Message} — both required). AWS populates all three on every validation refusal: fieldList has one entry per offending member, with name = that member's path and message = the same sentence as the top-level message.

Related open issues, neither of which covers these six: 20261029T234705Z-location-refusal-bodies-are-emulator-authored-and-omit (same omission, but for GetMapGlyphs' FontUnicodeRange only — that cluster is owned and excluded here), and 20261127T164020Z-location-3-net-new-causes-validationexception-messages-diverge (the '<Member>.<1-based>.Member' list-path rendering, but observed on BatchDeleteDevicePositionHistory from the BEHAVIORAL corpus; Cause 5 below is the same rendering defect reached through a different operation on the WIRE corpus, and carries a wire replay key that issue does not have).

Cause 1 of 6: GetDevicePosition DeviceId @pattern refusal carries no reason/fieldList.
AWS: Smithy: GetDevicePosition GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest; DeviceId -> Id, required, @pattern ^[-._\p{L}\p{N}]+$. A DeviceId that violates the pattern is refused 400 ValidationException with message "'DeviceId' must match regex ^[-._\p{L}\p{N}]+$" (fixed text "' ... ' must match regex " + the modeled member name + the model's pattern), reason "FieldValidationFailed", and fieldList [{name: "DeviceId", message: <the same sentence>}].
Emulator: the message matches, but reason and fieldList are absent from the body.
Why: two members the model marks required are missing, so a strict SDK deserialization yields nulls where an enum and a list are mandatory, and a client that branches on reason == FieldValidationFailed or points a form field at fieldList[].name gets nothing.
Fix: the location crate's shared ValidationException construction/serializer path — always populate Reason and FieldList from the constraint failure that raised it.
Traces: location-device-position-id-validation#task#5.

Cause 2 of 6: GetGeofence GeofenceId @pattern refusal carries no reason/fieldList.
AWS: Smithy: GetGeofence GET /geofencing/v0/collections/{CollectionName}/geofences/{GeofenceId}; GeofenceId -> Id, required, @pattern ^[-._\p{L}\p{N}]+$. Refused 400 with message "'GeofenceId' must match regex ^[-._\p{L}\p{N}]+$", reason "FieldValidationFailed", fieldList [{name: "GeofenceId", message: <same sentence>}].
Emulator: message correct; reason and fieldList absent.
Why / Fix: as Cause 1.
Traces: location-geofence-get-id-validation#task#6.

Cause 3 of 6: GetMapStyleDescriptor MapName @pattern refusal carries no reason/fieldList.
AWS: Smithy: GetMapStyleDescriptor GET /maps/v0/maps/{MapName}/style-descriptor; MapName -> ResourceName, required, @pattern ^[-._\w]+$. Refused 400 with message "'MapName' must match regex ^[-._\w]+$", reason "FieldValidationFailed", fieldList [{name: "MapName", message: <same sentence>}].
Emulator: message correct; reason and fieldList absent.
Why / Fix: as Cause 1.
Traces: location-map-style-descriptor-rejects-malformed-map-name#task#4.

Cause 4 of 6: StartJob Name @pattern refusal carries no reason/fieldList.
AWS: Smithy: StartJob POST /metadata/v0/jobs; Name -> ResourceName, @length 1..100, @pattern ^[-._\w]+$. Refused 400 with message "'Name' must match regex ^[-._\w]+$", reason "FieldValidationFailed", fieldList [{name: "Name", message: <same sentence>}].
Emulator: message correct; reason and fieldList absent.
Why / Fix: as Cause 1.
Traces: location-start-job-request-validation#task#2.

Cause 5 of 6: a LIST element's path is rendered '<Member>.<1-based index>.Member' instead of '<Member>[<0-based index>]'.
AWS: Smithy: BatchGetDevicePosition POST /tracking/v0/trackers/{TrackerName}/get-positions; DeviceIds -> IdList, required, @length 1..10, whose member targets Id with @pattern ^[-._\p{L}\p{N}]+$. When an ELEMENT violates the member pattern, the path AWS quotes is the modeled list member name followed by the offending element's ZERO-BASED index in square brackets, so the second element is DeviceIds[1]: message "'DeviceIds[1]' must match regex ^[-._\p{L}\p{N}]+$", reason "FieldValidationFailed", fieldList [{name: "DeviceIds[1]", message: <the same sentence>}].
Emulator: 400 with message "'DeviceIds.2.Member' must match regex ^[-._\p{L}\p{N}]+$" — a dotted path, a ONE-BASED index and a literal trailing ".Member" segment that is not part of this API's member-path vocabulary — plus the missing reason/fieldList.
Why: a client that parses the quoted path to point an operator or a form field at the offending element is handed a path no Location member path ever has, and the one-based index names a different element than the one refused; the same path is what fieldList[].name must carry, so field-level highlighting is wrong as well.
Fix: the shared list-element path renderer in the location crate's constraint-validation path — render "<ModeledMemberName>[<zero-based index>]".
Traces: location-batch-get-device-position-omits-unknown-device#task#5.

Cause 6 of 6: a NESTED member's refusal quotes the dotted path in the message where AWS quotes only the leaf member name.
AWS: Smithy: StartJob POST /metadata/v0/jobs; OutputOptions -> JobOutputOptions (required), whose Location member carries the pattern ^(arn:aws(-[a-z]+)*:[a-z0-9-]+:[a-z0-9-]*:(\d{12})?:[\w/+=,.-]+|s3://[a-z0-9][a-z0-9._-]{2,254}(/[^/]+)*/)$. A bad value is refused 400 with the message quoting the LEAF member name — "'Location' must match regex <pattern>" — while the structured fieldList entry carries the full dotted path: [{name: "OutputOptions.Location", message: "'Location' must match regex <pattern>"}], reason "FieldValidationFailed". So the two places deliberately differ: message = leaf name, fieldList[].name = dotted path.
Emulator: 400 with message "'OutputOptions.Location' must match regex <pattern>" (the dotted path hoisted into the sentence) and no reason/fieldList at all, so the dotted path appears in the one place AWS does not put it and is missing from the one place it does.
Why: a client matching the service's sentence, or reading fieldList to attribute the failure to a nested input member, gets neither form AWS emits.
Fix: the same shared validation/serializer path — the message renders the offending member's own name, the fieldList entry renders its full dotted path.
Traces: location-start-job-request-validation#task#1.

## Evidence
All six clusters: expected 400, actual 400, error code ValidationException on both sides.
Cause 1 — location-device-position-id-validation#task#5 (GetDevicePosition): $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'DeviceId', 'message': "'DeviceId' must match regex ^[-._\\p{L}\\p{N}]+$"}] got None; $.message already matched.
Cause 2 — location-geofence-get-id-validation#task#6 (GetGeofence): $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'GeofenceId', 'message': "'GeofenceId' must match regex ^[-._\\p{L}\\p{N}]+$"}] got None.
Cause 3 — location-map-style-descriptor-rejects-malformed-map-name#task#4 (GetMapStyleDescriptor): $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'MapName', 'message': "'MapName' must match regex ^[-._\\w]+$"}] got None.
Cause 4 — location-start-job-request-validation#task#2 (StartJob): $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'Name', 'message': "'Name' must match regex ^[-._\\w]+$"}] got None.
Cause 5 — location-batch-get-device-position-omits-unknown-device#task#5 (BatchGetDevicePosition): $.message expected "'DeviceIds[1]' must match regex ^[-._\\p{L}\\p{N}]+$", got "'DeviceIds.2.Member' must match regex ^[-._\\p{L}\\p{N}]+$"; $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'DeviceIds[1]', 'message': "'DeviceIds[1]' must match regex ^[-._\\p{L}\\p{N}]+$"}] got None. Fixed text and pattern are right; the variable part (the element path) is wrong in form and in index base.
Cause 6 — location-start-job-request-validation#task#1 (StartJob): $.message expected "'Location' must match regex ^(arn:aws(-[a-z]+)*:[a-z0-9-]+:[a-z0-9-]*:(\\d{12})?:[\\w/+=,.-]+|s3://[a-z0-9][a-z0-9._-]{2,254}(/[^/]+)*/)$", got the same sentence with 'OutputOptions.Location' quoted instead of 'Location'; $.reason expected 'FieldValidationFailed' got None; $.fieldList expected [{'name': 'OutputOptions.Location', 'message': "'Location' must match regex <pattern>"}] got None.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the five fixtures named in Traces/trace_ids.
2. Causes 1-4: call GetDevicePosition(DeviceId="has a space"), GetGeofence(GeofenceId="has a space"), GetMapStyleDescriptor(MapName="bad name!") and StartJob(Name="bad name!"). Each answers 400 ValidationException with the correct sentence; assert the body ALSO carries reason == 'FieldValidationFailed' and a one-entry fieldList whose name is the member and whose message repeats the sentence.
3. Cause 5: BatchGetDevicePosition(TrackerName=<tracker>, DeviceIds=["<valid-id>", "has a space"]). The quoted path in the 400 message (and in fieldList[0].name) must be 'DeviceIds[1]' — square brackets, zero-based — not 'DeviceIds.2.Member'.
4. Cause 6: StartJob with OutputOptions.Location set to a value matching neither the ARN nor the s3:// form. The message must quote the leaf member ('Location'); fieldList[0].name must be the dotted path ('OutputOptions.Location').
