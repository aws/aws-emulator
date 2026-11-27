Issue-Source: trace-sweep-divergence
Issue-Trace: lightsail-instance-snapshot-create-and-delete
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lightsail-instance-snapshot-create-and-delete lightsail-gui-session-and-verification-refusals
Issue-Shape-Id: com.amazonaws.lightsail#Lightsail_20161128
Issue-Key: lightsail|CreateInstanceSnapshot|2xx->2xx|$.operations[*]
Issue-Key: lightsail|GetInstanceSnapshot|2xx->2xx|$.instanceSnapshot.location.availabilityZone
Issue-Key: lightsail|DeleteInstanceSnapshot|2xx->2xx|$.operations[*].location.availabilityZone
Issue-Key: lightsail|CreateInstanceSnapshot|4xx->4xx|$.code
Issue-Key: lightsail|CreateInstanceSnapshot|4xx->4xx|$.message
Issue-Key: lightsail|GetStaticIp|2xx->2xx|$.staticIp.tags
Issue-Commit: b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1

# Lightsail response payloads diverge from AWS: snapshot availability zone, operations list and omitted empty tags

> _Note: this issue was identified against the code base at commit `b7738e26e4fa21b25d6f40a3ea7ebe92b6b6f4d1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInstanceSnapshot

## Description
Five successful-path and one error-path Lightsail responses carry members whose values or presence diverge from AWS: the snapshot resource's availability zone, the operations list returned by the snapshot lifecycle, the duplicate-name error's code and message, and an empty list member dropped from a getter. This bundle has 5 causes and all 5 must be closed.

Cause 1 of 5: CreateInstanceSnapshot returns one Operation where AWS returns two, with different members.
  AWS: 200 with `operations` holding TWO Operation entries — one for the snapshot being created (`resourceType` = "InstanceSnapshot", `resourceName` = the new snapshot name, `location.availabilityZone` = "all" because an instance snapshot is a regional resource, and `operationDetails` = the SOURCE INSTANCE name) and one for the instance being snapshotted (`resourceType` = "Instance", `resourceName` = the source instance name, `location.availabilityZone` = that instance's zone, and `operationDetails` = the NEW SNAPSHOT name). Both carry `operationType` = "CreateInstanceSnapshot", `status` = "Started", `isTerminal` = false. Smithy: CreateInstanceSnapshotResult.operations -> OperationList of Operation, whose members include operationDetails ("Details about the operation"), location -> ResourceLocation and resourceType -> ResourceType. The two-entry pairing and the "all" zone are recorded behavior.
  Emulator: 200 with a SINGLE Operation entry: only the InstanceSnapshot one, with `location.availabilityZone` = the instance's zone instead of "all", and NO `operationDetails` member at all. The second, instance-side operation is absent.
  Why: a client that tracks both sides of the snapshot (polling the instance operation as well as the snapshot operation, or reading operationDetails to learn which instance a snapshot came from) finds nothing to track and no linkage between snapshot and source; a zone-aware caller also mis-places a regional snapshot into one zone.
  Fix: CreateInstanceSnapshot's handler and the Operation construction it shares in the lightsail service crate's ops module — emit both operations, set operationDetails to the counterpart resource's name, and report "all" as the snapshot's availability zone.
  Traces: lightsail-instance-snapshot-create-and-delete#task#3.

Cause 2 of 5: GetInstanceSnapshot reports a zonal availability zone for a regional snapshot.
  AWS: 200 with `instanceSnapshot.location.availabilityZone` = "all" — an instance snapshot is not pinned to a zone, so AWS publishes the fixed literal "all" there, with `regionName` the snapshot's region. Smithy: InstanceSnapshot.location -> ResourceLocation {availabilityZone, regionName}. The "all" value is recorded behavior.
  Emulator: 200 with `instanceSnapshot.location.availabilityZone` = the source instance's zone.
  Why: the snapshot appears zone-bound, so a caller choosing where to restore it believes it is restricted to a single availability zone.
  Fix: the snapshot representation in the lightsail service crate's state/serializer for instance snapshots.
  Traces: lightsail-instance-snapshot-create-and-delete#task#4.

Cause 3 of 5: DeleteInstanceSnapshot's operation carries the same wrong availability zone.
  AWS: 200 with `operations[0].location.availabilityZone` = "all" for the snapshot-scoped delete operation.
  Emulator: 200 with the source instance's zone.
  Why: as Cause 2, on the delete path — confirming the "all" rule belongs to the shared snapshot location, not to one getter.
  Fix: DeleteInstanceSnapshot's handler and the shared snapshot-location construction in the lightsail ops module.
  Traces: lightsail-instance-snapshot-create-and-delete#task#6.

Cause 4 of 5: CreateInstanceSnapshot with a snapshot name already in use returns the wrong error code and message.
  AWS: 400 `InvalidInputException` with `code` = "NameExists" and `message` = "Some names are already in use: <names>" — "Some names are already in use: " is fixed text and `<names>` is the colliding name(s) from the request. Smithy: CreateInstanceSnapshot.instanceSnapshotName -> ResourceName, required; errors include InvalidInputException (400), which carries the `code` member. The "NameExists" code and the sentence are recorded behavior.
  Emulator: 400 with `code` = "InvalidParams" and `message` = "a snapshot named '<name>' already exists" — its own code and wording.
  Why: "NameExists" is the code a client uses to distinguish a name collision (retry under a new name) from any other invalid argument; under "InvalidParams" that branch never fires and idempotent create-or-reuse logic breaks.
  Fix: CreateInstanceSnapshot's duplicate-name check and error construction in the lightsail ops module.
  Traces: lightsail-instance-snapshot-create-and-delete#task#5.

Cause 5 of 5: GetStaticIp omits the empty `tags` list from the static-IP payload.
  AWS: recorded behavior only — the recorded 200 response carries `staticIp.tags` = [] (an empty list, present rather than omitted). The on-disk Smithy StaticIp shape lists {name, arn, supportCode, createdAt, location, resourceType, ipAddress, attachedTo, isAttached} and no `tags` member, so the model says nothing about it and the recording is the authority here.
  Emulator: 200 with `staticIp.tags` absent from the body entirely.
  Why: a client that reads the tag list off a static IP without a null guard — the natural shape when the member is always present — faults on a resource that simply has no tags, and tag-driven inventory sees a different payload shape from AWS.
  Fix: the StaticIp serializer/state in the lightsail service crate — emit `tags` as an empty list rather than dropping the member.
  Traces: lightsail-gui-session-and-verification-refusals#task#2.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator, 5 divergence clusters (verdict=mismatch, service=lightsail):

1. lightsail-instance-snapshot-create-and-delete#task#3 CreateInstanceSnapshot — expected operations[0] = {operationType 'CreateInstanceSnapshot', resourceType 'InstanceSnapshot', resourceName <snapshot>, operationDetails <source instance>, location {availabilityZone 'all', regionName 'us-west-2'}, status 'Started', isTerminal False} and operations[1] = {resourceType 'Instance', resourceName <source instance>, operationDetails <snapshot>, location {availabilityZone 'us-west-2a', regionName 'us-west-2'}, ...}; actual a single entry with location.availabilityZone 'us-west-2a', no operationDetails, and operations[1] = None.

2. lightsail-instance-snapshot-create-and-delete#task#4 GetInstanceSnapshot — $.instanceSnapshot.location.availabilityZone: expected 'all', got 'us-west-2a'.

3. lightsail-instance-snapshot-create-and-delete#task#6 DeleteInstanceSnapshot — $.operations[0].location.availabilityZone: expected 'all', got 'us-west-2a'.

4. lightsail-instance-snapshot-create-and-delete#task#5 CreateInstanceSnapshot (duplicate name) — $.code: expected 'NameExists', got 'InvalidParams'; $.message: expected 'Some names are already in use: sim2real-probe-lightsail-snap-e475c08e', got "a snapshot named 'sim2real-probe-lightsail-snap-e475c08e' already exists".

5. lightsail-gui-session-and-verification-refusals#task#2 GetStaticIp — $.staticIp.tags: missing (expected [], got None).

Model check: Operation members include operationDetails, location -> ResourceLocation and resourceType; CreateInstanceSnapshotResult.operations -> OperationList; InvalidInputException (httpError 400) has a `code` member; the StaticIp shape in the on-disk model has no tags member, so Cause 5 rests on the recording.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the simparity-wire-aws-v1 wire corpus.
2. Replay lightsail-instance-snapshot-create-and-delete step task#3 (CreateInstanceSnapshot from an existing instance) and count $.operations: AWS returns two entries (snapshot-side with availabilityZone 'all' plus operationDetails, instance-side with the instance zone), the emulator returns one with no operationDetails.
3. Replay the same fixture's task#4 (GetInstanceSnapshot) and check $.instanceSnapshot.location.availabilityZone == 'all'.
4. Replay task#5 (CreateInstanceSnapshot reusing the snapshot name just created) and check $.code == 'NameExists' and the 'Some names are already in use: <name>' message.
5. Replay task#6 (DeleteInstanceSnapshot) and check $.operations[0].location.availabilityZone == 'all'.
6. Replay lightsail-gui-session-and-verification-refusals#task#2 (GetStaticIp on an allocated static IP) and check that $.staticIp.tags is present as [].
