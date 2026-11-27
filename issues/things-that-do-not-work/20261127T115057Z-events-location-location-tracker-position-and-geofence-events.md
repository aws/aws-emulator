Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__location.device-position-update-event-names-only-the-tracker events__location.tracker-geofence-enter-event-reaches-rule-queue events__location.position-update-event-withheld-until-tracker-enables-eventbridge events__location.out-of-range-latitude-update-rejected-and-emits-no-event
Issue-Key: events__location.device-position-update-event-names-only-the-tracker|_next_event#14|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=1, WaitTimeSeconds=2)
Issue-Key: events__location.tracker-geofence-enter-event-reaches-rule-queue|_collect#29|Timeout|resp = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=2)
Issue-Key: events__location.position-update-event-withheld-until-tracker-enables-eventbridge|pull#23|Timeout|resp = self.sqs.receive_message(QueueUrl=self.url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Key: events__location.out-of-range-latitude-update-rejected-and-emits-no-event|_events_for#20|Timeout|for msg in sqs.receive_message(
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__location: Location tracker position and geofence events never reach EventBridge — all four tracker checks exhaust their budget

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
Cause 1 of 1: Amazon Location trackers publish no events to EventBridge, and an out-of-range position update is not refused, so no tracker scenario completes.

AWS: a tracker created with EventBridgeEnabled publishes to the default bus on source `aws.geo`: a device position update produces a `Location Device Position Event` whose resources name ONLY the tracker, and a device crossing a geofence of an associated collection produces a `Location Geofence Event` with ENTER/EXIT in its detail; a tracker without EventBridge enabled publishes nothing until it is updated to enable it. BatchUpdateDevicePosition with a latitude outside [-90, 90] reports that entry as an error and emits no event. Recorded behavior only for the event detail shapes (documented for Amazon Location); the per-entry error channel is modeled on BatchUpdateDevicePosition.
Emulator: all four checks run out of their 60.0s budget — the awaited `aws.geo` events never reach the rule's queue, and the out-of-range refusal/no-event pair cannot be established either. (The separate BatchEvaluateGeofences stub failure in this service is already filed and is not part of this issue.)
Why: a client built on Location's EventBridge integration (geofence entry alerts, position-driven workflows) receives nothing, while the tracker reports updates accepted; and an invalid coordinate is not reported back, so bad data is stored.
Fix: the location tracker state and its event publication (BatchUpdateDevicePosition / geofence evaluation must put `aws.geo` events on the default bus, gated on the tracker's EventBridgeEnabled flag) and the coordinate-range validation in BatchUpdateDevicePosition.
Traces: events__location.device-position-update-event-names-only-the-tracker, events__location.tracker-geofence-enter-event-reaches-rule-queue, events__location.position-update-event-withheld-until-tracker-enables-eventbridge, events__location.out-of-range-latitude-update-rejected-and-emits-no-event.

## Evidence
All four checks: `Error: Check timed out after 60.0s` — no `aws.geo` Location Device Position Event or Location Geofence Event ever reached the rule's queue after BatchUpdateDevicePosition on an EventBridge-enabled tracker.

## Steps to Reproduce
1. location CreateTracker(EventBridgeEnabled=true) (and a second with it disabled); CreateGeofenceCollection + PutGeofence; AssociateTrackerConsumer.
2. events PutRule on {source:["aws.geo"]} + PutTargets naming an SQS queue.
3. location BatchUpdateDevicePosition moving a device into the fence; then one with Latitude=120.
4. Observe no messages on the queue and no per-entry error for the out-of-range update.
