Issue-Source: trace-sweep-divergence
Issue-Trace: group-tag-and-update-refusals
Issue-Step: setup#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: group-tag-and-update-refusals resource-groups-error-config-query-conflict resource-groups-group-tags-crud resource-groups-tag-query-group-lifecycle resource-groups-error-group-lifecycle resource-groups-generic-group-configuration
Issue-Shape-Id: com.amazonaws.resourcegroups#Ardi
Issue-Key: resource-groups|CreateGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|DeleteGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|GetGroup|2xx->2xx|$.Group.OwnerId
Issue-Key: resource-groups|UpdateGroup|2xx->2xx|$.Group.OwnerId
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# Resource Groups Group response omits OwnerId on create, get, update and delete

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateGroup

## Description
Every operation that returns the Group structure drops the OwnerId member, so the account that owns a resource group is never visible to a client. One root cause (the Group serializer / the stored group record does not carry an owner account id) shows up in four operations; all four must be closed.

AWS: recorded behavior only for the member itself - the checked-in Smithy model's com.amazonaws.resourcegroups#Group declares GroupArn, Name, Description, Criticality, Owner, DisplayName and ApplicationTag and does NOT declare OwnerId, but every recorded Group-returning response in this corpus carries Group.OwnerId set to the 12-digit AWS account id that owns the group (the same account id that appears in the group's GroupArn). The value is the request's account, not a per-run random, so it is a requirement: it must be the emulator's account id, rendered as the 12-digit account number.
Emulator: Group.OwnerId is absent (reported as `$.Group.OwnerId: missing (expected '000000000000', got None)`) on a 200 response that is otherwise correct.
Why: a client that reads Group.OwnerId to decide whether a group is owned by the calling account (cross-account group listing, ownership filters, audit tooling) gets null and either crashes on the missing key or concludes the group is unowned.
Fix: the Group serializer shared by these handlers, plus the group record in the service's state so an owner account id is stored at create time and echoed on read/update/delete.

Cause 1 of 4: CreateGroup - the 200 CreateGroupOutput omits Group.OwnerId. Fixture group-tag-and-update-refusals step setup#1 (same cluster: resource-groups-error-config-query-conflict#setup#1, resource-groups-group-tags-crud#setup#1, resource-groups-tag-query-group-lifecycle#setup#1).
Cause 2 of 4: DeleteGroup - the 200 DeleteGroupOutput omits Group.OwnerId. Fixture resource-groups-error-config-query-conflict step teardown#7 (same cluster: teardown#8, resource-groups-error-group-lifecycle#task#3, resource-groups-generic-group-configuration#teardown#5, resource-groups-group-tags-crud#teardown#6, resource-groups-tag-query-group-lifecycle#task#8).
Cause 3 of 4: GetGroup - the 200 GetGroupOutput omits Group.OwnerId. Fixture resource-groups-tag-query-group-lifecycle step task#2 (same cluster: task#5).
Cause 4 of 4: UpdateGroup - the 200 UpdateGroupOutput omits Group.OwnerId. Fixture resource-groups-tag-query-group-lifecycle step task#4.

No open issue covers this; the three open resource-groups issues concern tag-query membership of Scheduler resources and CloudFormation stack groups, not the Group payload shape.

## Evidence
CreateGroup (group-tag-and-update-refusals#setup#1): status 200 expected and got, body diff `$.Group.OwnerId: missing (expected '000000000000', got None)`.
DeleteGroup (resource-groups-error-config-query-conflict#teardown#7): status 200/200, `$.Group.OwnerId: missing (expected '000000000000', got None)`.
GetGroup (resource-groups-tag-query-group-lifecycle#task#2): status 200/200, `$.Group.OwnerId: missing (expected '000000000000', got None)`.
UpdateGroup (resource-groups-tag-query-group-lifecycle#task#4): status 200/200, `$.Group.OwnerId: missing (expected '000000000000', got None)`.
In each case the expected value is the owning account id of the group, identical to the account segment of the group's GroupArn in the same response; the emulator returns no OwnerId key at all. 15 recorded steps across 6 fixtures diverge on exactly this key.

## Steps to Reproduce
1. Boot the emulator and replay trace set simparity-wire-aws-v1.
2. CreateGroup with a Name and a TagFilter resource query (fixture group-tag-and-update-refusals, step setup#1) and inspect the response Group object: no OwnerId member.
3. Call GetGroup / UpdateGroup / DeleteGroup on that group (resource-groups-tag-query-group-lifecycle steps task#2, task#4, task#8) - each 200 response's Group object likewise has no OwnerId.
4. Compare with the recorded responses, which all carry Group.OwnerId = the caller's 12-digit account id.
