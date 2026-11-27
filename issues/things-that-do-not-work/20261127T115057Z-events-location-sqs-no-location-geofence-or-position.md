Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__location__sqs.geofence-exit-reaches-default-bus-rule-with-its-detail events__location__sqs.geofence-properties-pattern-routes-only-the-matching-fence events__location__sqs.tracker-position-update-events-only-when-eventbridge-enabled events__location__sqs.distance-filtered-tracker-ignores-short-move-across-fence-edge events__location__sqs.unassociated-tracker-emits-no-geofence-event-until-associated
Issue-Key: events__location__sqs.geofence-exit-reaches-default-bus-rule-with-its-detail|_drain#29|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Key: events__location__sqs.geofence-properties-pattern-routes-only-the-matching-fence|_take#25|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Key: events__location__sqs.tracker-position-update-events-only-when-eventbridge-enabled|_pull#27|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Key: events__location__sqs.distance-filtered-tracker-ignores-short-move-across-fence-edge|_receive#28|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Key: events__location__sqs.unassociated-tracker-emits-no-geofence-event-until-associated|_collect#26|Timeout|got = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10, WaitTimeSeconds=1)
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# events__location__sqs: no Location geofence or position event is routed to a rule's queue — all five tracker/geofence routing checks exhaust their budget

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchUpdateDevicePosition

## Description
Cause 1 of 1: the Amazon Location → EventBridge → SQS path produces nothing: geofence ENTER/EXIT and device-position events are never published, so neither the positive routing nor the filtering/gating negatives can be observed.

AWS: on the default bus, source `aws.geo`, a tracker associated with a geofence collection publishes a `Location Geofence Event` (EXIT/ENTER in the detail, with the fence's GeofenceProperties available for pattern matching so only the matching fence's rule receives it) and a `Location Device Position Event` only while the tracker has EventBridge enabled; a tracker with a position-filtering distance configured does NOT publish for a move shorter than the filter even across a fence edge, and a tracker not associated with a collection publishes no geofence event until it is associated. Recorded behavior only (documented Amazon Location event shapes).
Emulator: all five checks run out of their 60.0s budget — nothing is ever delivered to the rule's queue, so every one of these behaviors is unobservable.
Why: geofence-driven applications (arrival/departure alerts, zone-based routing) get no events at all; and because nothing is published, a client cannot tell the difference between correct suppression (filtered/unassociated/disabled) and total breakage.
Fix: the location tracker/geofence evaluation state and its EventBridge publication path (including the position-filter and tracker-consumer-association gates), and the geofence-properties content carried in the published detail.
Traces: events__location__sqs.geofence-exit-reaches-default-bus-rule-with-its-detail, events__location__sqs.geofence-properties-pattern-routes-only-the-matching-fence, events__location__sqs.tracker-position-update-events-only-when-eventbridge-enabled, events__location__sqs.distance-filtered-tracker-ignores-short-move-across-fence-edge, events__location__sqs.unassociated-tracker-emits-no-geofence-event-until-associated.

## Evidence
All five checks: `Error: Check timed out after 60.0s` — no `aws.geo` event ever arrived on the rules' SQS queues.

## Steps to Reproduce
1. location CreateGeofenceCollection + PutGeofence (with GeofenceProperties); CreateTracker (EventBridgeEnabled, one with PositionFiltering=DistanceBased); AssociateTrackerConsumer for some trackers only.
2. events PutRule on {source:["aws.geo"]} (and one pattern filtering on the fence's properties) + PutTargets naming SQS queues.
3. BatchUpdateDevicePosition to cross the fence (and a sub-filter-distance move).
4. Observe no messages on any queue.
