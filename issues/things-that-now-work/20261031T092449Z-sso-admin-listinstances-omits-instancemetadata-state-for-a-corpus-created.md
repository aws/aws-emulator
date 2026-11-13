Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-682d3c46"].State
Issue-Commit: 82ad778ed253471d0ebb5d9587965556bfb040b0

# ListInstances omits InstanceMetadata.State for a corpus-created Identity Center instance

> _Note: this issue was identified against the code base at commit `82ad778ed253471d0ebb5d9587965556bfb040b0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the shared InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so an instance created and then listed inside the same fixture comes back without the state member AWS serves. A fixer must close this one cause before submitting.

Cause 1 of 1: ListInstances entry omits State (fixture sso-admin-describe-list-edge-cases-r2, step task#4).

AWS: ListInstances returns `Instances`, a list of InstanceMetadata (Smithy: ListInstancesResponse.Instances -> InstanceList @length min 0 max 10; InstanceMetadata members InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status (InstanceStatus enum), StatusReason, PrimaryRegion, Regions; errors AccessDeniedException 403, InternalServerException 500, ThrottlingException 429, ValidationException 400). The on-disk model declares NO `State` member on InstanceMetadata, so for this member it is recorded behavior only: the recorded 200 carries, on each listed instance, an additional `State` structure whose `Name` mirrors that instance's `Status` enum value — `State: {"Name": "ACTIVE"}` beside `Status: "ACTIVE"` for an active instance. `State.Name` is the fixed member name and its value is read from the instance's own lifecycle status; the instance's Name, ARNs, identity-store id and CreatedDate in any one recording are per-run values and not part of the requirement.

Emulator: 200 whose listed instance matches on every other member but carries no `State` member at all — `$.Instances[Name="<instance name>"].State` expected `{'Name': 'ACTIVE'}`, got None. In this recording the selector resolved to the fixture-created instance `sim2real-wire-idc-682d3c46`; that generated name is a per-run value, the missing member is the defect.

Why: a client that reads an instance's lifecycle from the listing — filtering for instances whose `State.Name` is ACTIVE, or polling a just-created instance until it leaves CREATE_IN_PROGRESS — dereferences a member that is absent on every instance the emulator lists, so it either crashes or concludes no instance is ever ready.

Fix: the shared InstanceMetadata response serializer behind ListInstances (same serializer the DescribeInstance instance view reuses) — populate `State` from the instance record's status whenever an instance is serialized, including instances created earlier in the same session rather than only the ambient one.

Related open issue, which does NOT cover this: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission, but its replay keys bind to the instance named `sim2real-wire-idc-c56c0824` and to the IdentityCenter/ambient instances; this cluster's key (`$.Instances[Name="sim2real-wire-idc-682d3c46"].State`) is not carried by it, so the step is currently un-gated and the replay report reports it unowned. 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on the same operation, not the serialized instance shape.

Traces: sso-admin-describe-list-edge-cases-r2#task#4.

## Evidence
Cause 1 — ListInstances, fixture sso-admin-describe-list-edge-cases-r2 step task#4, expected 200 / actual 200:
  $.Instances[Name="sim2real-wire-idc-682d3c46"].State: missing — expected {'Name': 'ACTIVE'}, got None

Divergence key reported by the replay: sso-admin|ListInstances|status=200->200|keys=$.Instances[Name="sim2real-wire-idc-682d3c46"].State (verdict: mismatch, owned_by: none).

Requirement vs per-run values: the fixed part is the member `State` and its member `Name`, whose value equals the instance's own `Status` enum value (`ACTIVE` here). The instance's name `sim2real-wire-idc-682d3c46`, its InstanceArn, IdentityStoreId/IdentityStoreArn and CreatedDate are generated per run and are NOT requirements — no other member of this entry diverged.

Model evidence: smithy_query shape InstanceMetadata (com.amazonaws.ssoadmin#InstanceMetadata) lists no State member, and ListInstancesResponse exposes only Instances + NextToken, so the State view is grounded in the recording rather than the model.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4: the earlier steps create an Identity Center instance, and task#4 calls ListInstances.
3. Inspect the 200 body's Instances entry for the instance created by the fixture.
4. Observe the entry carries Status "ACTIVE" but has no State member; AWS's recorded response carries State {"Name": "ACTIVE"} on that same entry.
5. Expected after the fix: every serialized InstanceMetadata carries State whose Name equals that instance's Status.
