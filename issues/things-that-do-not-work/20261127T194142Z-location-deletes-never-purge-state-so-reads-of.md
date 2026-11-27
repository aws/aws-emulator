Issue-Source: trace-sweep-divergence
Issue-Trace: location-device-history-delete-and-unknown-tracker
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: location-device-history-delete-and-unknown-tracker location-geofence-batch-delete-unknown-collection location-geofence-batch-entry-refusals location-position-history-batch-delete-unknown-device-and-missing-tracker
Issue-Shape-Id: com.amazonaws.location#LocationService
Issue-Key: location|GetDevicePosition|4xx->2xx|$
Issue-Key: location|GetDevicePosition|4xx->2xx|$.message
Issue-Key: location|GetDevicePosition|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|GetDevicePosition|4xx->2xx|status
Issue-Key: location|GetGeofence|4xx->2xx|$.message
Issue-Key: location|GetGeofence|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: location|GetGeofence|4xx->2xx|status
Issue-Key: location|GetDevicePositionHistory|2xx->2xx|$.DevicePositions[*]
Issue-Commit: e2f414c8e0d435baa8e4d1479bd7220d9eecc351

# location deletes never purge state, so reads of deleted positions and geofences answer 200 instead of 404

> _Note: this issue was identified against the code base at commit `e2f414c8e0d435baa8e4d1479bd7220d9eecc351`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDevicePosition

## Description
THREE causes, one shared fault: the location crate's batch DELETE operations do not remove anything from the service's state, so every read that AWS answers as 'gone' after a delete still answers with the deleted data. On AWS each of these three reads is a hard behavioural signal (a 404 ResourceNotFoundException or an empty list) that a client uses to decide the delete took effect; here the emulator's read paths themselves are correct — they simply never see an empty store, because the deletes are unimplemented scaffolds that answer HTTP 501 and persist nothing (services/service-com-amazonaws-location-locationservice/src/ops/batch_delete_device_position_history.rs line 165 and services/service-com-amazonaws-location-locationservice/src/ops/batch_delete_geofence.rs still carry the '//! SCAFFOLDED, NOT IMPLEMENTED:' marker and return crate::scaffolded::Scaffolded::not_implemented).

Related open issue 20261127T164020Z-location-batchdeletedevicepositionhistory-stub asks for BatchDeleteDevicePositionHistory to be implemented from the behavioral corpus; it carries NO wire replay key, so the three wire reads below are not covered by it and will not be re-replayed when it is closed. There is no open issue for BatchDeleteGeofence.

Cause 1 of 3: GetDevicePosition still returns the last position after the device's history was deleted.
AWS: Smithy: GetDevicePosition GET /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/positions/latest, modeled error ResourceNotFoundException (httpError 404, required Message). BatchDeleteDevicePositionHistory (POST /tracking/v0/trackers/{TrackerName}/delete-positions) removes the named devices' samples, so the device afterwards holds no position records and the read answers 404 ResourceNotFoundException with the fixed message "records not found for given deviceId" — entirely fixed text, with no device id, tracker name or other request value interpolated.
Emulator: 200 with the full position body (DeviceId, SampleTime, ReceivedTime, Position) for the device whose history was just deleted, and it keeps answering 200 for the whole polling window (20 attempts), because the delete answered 501 and wrote nothing.
Why: a client that reads 404/ResourceNotFoundException as 'the history is gone' never observes the delete; data a caller asked to be erased stays readable, and any retry/poll loop waiting for the 404 spins to exhaustion.
Fix: implement the BatchDeleteDevicePositionHistory handler so it removes the named devices' samples from the tracker's position store in the location crate's state; GetDevicePosition's own not-found path already raises the right error and message.
Traces: location-device-history-delete-and-unknown-tracker#task#5.

Cause 2 of 3: GetGeofence still returns a deleted geofence.
AWS: Smithy: GetGeofence GET /geofencing/v0/collections/{CollectionName}/geofences/{GeofenceId}, modeled error ResourceNotFoundException (httpError 404, required Message). After BatchDeleteGeofence removes a geofence from its collection, a read of that id answers 404 ResourceNotFoundException whose message is the service's named-resource sentence "Geofence not found: <GeofenceId>." — fixed text "Geofence not found: ", the GeofenceId taken from the request, then a terminating period.
Emulator: 200 with the geofence still present (no error body at all, so no message and no x-amzn-errortype header), because BatchDeleteGeofence is an unimplemented scaffold that persists nothing.
Why: the geofence keeps matching device positions and keeps appearing to a client that re-reads it, so a caller cannot tell a delete succeeded, and geofence evaluation continues against boundaries that should no longer exist.
Fix: implement the BatchDeleteGeofence handler so it removes the named geofences from the collection in the location crate's state, and make sure GetGeofence's not-found path raises ResourceNotFoundException with the service's "Geofence not found: <GeofenceId>." sentence.
Traces: location-geofence-batch-delete-unknown-collection#task#6 (same cluster: location-geofence-batch-entry-refusals#task#6).

Cause 3 of 3: GetDevicePositionHistory still lists a sample whose history was deleted.
AWS: Smithy: GetDevicePositionHistory POST /tracking/v0/trackers/{TrackerName}/devices/{DeviceId}/list-positions, output DevicePositions required. After BatchDeleteDevicePositionHistory for that device, the history read answers 200 with an EMPTY DevicePositions list (the tracker and device still resolve; only the samples are gone).
Emulator: 200 whose DevicePositions still contains the pre-delete sample (DeviceId/SampleTime/ReceivedTime/Position), i.e. one extra element where AWS returns none.
Why: a client reconciling 'what is left after the delete' reads deleted history as live, and any test asserting the post-delete list is empty passes data it was promised was erased.
Fix: same state mutation as Cause 1 — the BatchDeleteDevicePositionHistory handler must purge the device's samples; the history read then projects an empty list on its own.
Traces: location-position-history-batch-delete-unknown-device-and-missing-tracker#task#6.

## Evidence
Cause 1 — location-device-history-delete-and-unknown-tracker#task#5 (GetDevicePosition): status expected 404, got 200; $.message expected 'records not found for given deviceId', got None; $headers.x-amzn-errortype expected (shaped, not asserted), got None; the poll gave up after 20 attempts with error code None instead of ResourceNotFoundException, the body still being the device's position record.
Cause 2 — location-geofence-batch-delete-unknown-collection#task#6 (GetGeofence; same cluster also location-geofence-batch-entry-refusals#task#6): status expected 404, got 200; $.message expected 'Geofence not found: <GeofenceId>.' (recorded with the request's own geofence id), got None; $headers.x-amzn-errortype expected (shaped, not asserted), got None.
Cause 3 — location-position-history-batch-delete-unknown-device-and-missing-tracker#task#6 (GetDevicePositionHistory): status 200 on both sides, but $.DevicePositions[0] was expected absent and the emulator returned a position record for the device whose history had been deleted.
Static corroboration in the checkout: services/service-com-amazonaws-location-locationservice/src/ops/batch_delete_device_position_history.rs:165 returns Scaffolded::not_implemented and keeps the SCAFFOLDED marker at lines 4-6; services/service-com-amazonaws-location-locationservice/src/ops/batch_delete_geofence.rs:4-12 carries the same marker. services/service-com-amazonaws-location-locationservice/src/ops/get_device_position.rs:173-186 already raises ResourceNotFoundException with the correct fixed message when the device holds no samples, which is why the defect is on the delete side.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay location-device-history-delete-and-unknown-tracker, location-geofence-batch-delete-unknown-collection (and location-geofence-batch-entry-refusals) and location-position-history-batch-delete-unknown-device-and-missing-tracker.
2. Cause 1/3: CreateTracker, BatchUpdateDevicePosition to store a sample for a device, then BatchDeleteDevicePositionHistory(TrackerName=<tracker>, DeviceIds=[<device>]). Then GetDevicePosition for that device — expect 404 ResourceNotFoundException with message 'records not found for given deviceId' — and GetDevicePositionHistory for it — expect 200 with DevicePositions == [].
3. Cause 2: CreateGeofenceCollection, PutGeofence/BatchPutGeofence a geofence, BatchDeleteGeofence(CollectionName=<collection>, GeofenceIds=[<id>]), then GetGeofence(<collection>, <id>) — expect 404 ResourceNotFoundException with message 'Geofence not found: <GeofenceId>.'
4. Observe instead that both deletes answer 501 NotImplemented and every read afterwards answers 200 with the data that should be gone.
