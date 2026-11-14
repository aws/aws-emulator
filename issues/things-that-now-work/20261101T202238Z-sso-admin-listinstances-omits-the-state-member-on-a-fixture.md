Issue-Source: trace-sweep-divergence
Issue-Trace: sso-admin-describe-list-edge-cases-r2
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sso-admin-describe-list-edge-cases-r2
Issue-Shape-Id: com.amazonaws.ssoadmin#SWBExternalService
Issue-Key: sso-admin|ListInstances|2xx->2xx|$.Instances[Name="sim2real-wire-idc-053f6efc"].State
Issue-Commit: d8c05a1e1ebae5e4b40ea6c1043fddacd8d2c3de

# ListInstances omits the State member on a fixture-created Identity Center instance

> _Note: this issue was identified against the code base at commit `d8c05a1e1ebae5e4b40ea6c1043fddacd8d2c3de`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListInstances

## Description
ONE cause: the shared InstanceMetadata serializer used by ListInstances never emits the instance's `State` view, so an instance created earlier in the same fixture is listed without it.

Cause 1 of 1: ListInstances entries carry no State member (fixture sso-admin-describe-list-edge-cases-r2, step task#4).
AWS: ListInstances returns 200 with `Instances` (Smithy: ListInstancesResponse.Instances -> InstanceList, @length 0..10, target InstanceMetadata; paginated inputToken/outputToken NextToken, items Instances). Each listed instance carries, beyond the modeled InstanceMetadata members (InstanceArn, IdentityStoreId, IdentityStoreArn, OwnerAccountId, Name, CreatedDate, Status, StatusReason, PrimaryRegion, Regions), a `State` structure whose `Name` is the instance's lifecycle value, mirroring the instance's own `Status` enum value — so an active instance is listed with Status "ACTIVE" and State {"Name": "ACTIVE"}. The on-disk model has NO State member on com.amazonaws.ssoadmin#InstanceMetadata, so this is recorded behavior only: the recording of this step is the authority. The State value is read from the listed instance's own stored status; the instance's Name, ARN, identity-store id and CreatedDate in any single recording are per-run values and are not requirements.
Emulator: 200, and the listed instance matches on every other member, but `State` is absent entirely — $.Instances[Name="<the instance created by this fixture>"].State expected {'Name': 'ACTIVE'}, got None.
Why: a client that reads or waits on an instance's lifecycle from the listing (State.Name) dereferences a missing member on every listed instance and cannot distinguish ACTIVE from CREATE_IN_PROGRESS / CREATE_FAILED without a second DescribeInstance call that is itself missing the same view.
Fix: the shared InstanceMetadata response serializer used by ListInstances — populate State.Name from the same stored instance status that already feeds the emitted `Status` member.
Traces: sso-admin-describe-list-edge-cases-r2#task#4.

Relation to open issues: 20261029T184546Z-sso-admin-instance-reads-drop-the-state-member-and-the.md describes the same omission, but its replay keys are bound to instance names from an earlier run (its Cause 2 key names Instances[Name="sim2real-wire-idc-c56c0824"]) and the replay report reports THIS cluster as unowned — its generated instance name differs, so the existing issue's pointer does not cover this cluster and it must be re-replayed on its own. 20261029T112447Z-sso-admin-listinstances-pagination.md concerns NextToken decoding on this operation, not the serialized instance shape.

## Evidence
Fixture sso-admin-describe-list-edge-cases-r2, step task#4, ListInstances, expected 200 / actual 200 (verdict: mismatch):
  $.Instances[Name="sim2real-wire-idc-053f6efc"].State: missing — expected {'Name': 'ACTIVE'}, got None

The instance name in the selector (`sim2real-wire-idc-053f6efc`) is a per-run generated value created by this fixture and is not part of the requirement; the requirement is that whatever instance is listed carries a State structure whose Name equals that instance's Status (here ACTIVE). All other members of that listed instance compared equal, so the only divergence is the absent State member.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the sso-admin wire corpus (trace set simparity-wire-aws-v1).
2. Replay fixture sso-admin-describe-list-edge-cases-r2 up to step task#4: the fixture creates an Identity Center instance, then calls ListInstances.
3. Inspect the 200 body's Instances entry for the created instance: it carries Status "ACTIVE" but no State member.
4. Expected: that entry also carries State: {"Name": "ACTIVE"}, with State.Name mirroring the instance's Status.
