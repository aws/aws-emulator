Issue-Source: trace-sweep-divergence
Issue-Trace: backup-restore-testing-plan-lifecycle
Issue-Step: task#6
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: backup-restore-testing-plan-lifecycle backup-restore-testing-selection-lifecycle backup-restore-testing-selection-lifecycle-wildcard backup-restore-testing-selection-conditions-and-errors backup-restore-testing-selection-conditions-and-overrides
Issue-Shape-Id: com.amazonaws.backup#CryoControllerUserManager
Issue-Key: backup|GetRestoreTestingSelection|2xx->2xx|$.RestoreTestingSelection.ProtectedResourceConditions
Issue-Key: backup|GetRestoreTestingSelection|2xx->2xx|$.RestoreTestingSelection.RestoreMetadataOverrides
Issue-Key: backup|GetRestoreTestingSelection|2xx->2xx|$.RestoreTestingSelection.ProtectedResourceArns
Issue-Key: backup|GetRestoreTestingSelection|2xx->2xx|$.RestoreTestingSelection.IamRoleArn
Issue-Commit: 16df41068e9592847de76789eb93312b3e770273

# AWS Backup restore testing selection read bodies omit modeled members and ignore updates

> _Note: this issue was identified against the code base at commit `16df41068e9592847de76789eb93312b3e770273`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetRestoreTestingSelection

## Description
Four net-new causes in the READ path of the AWS Backup restore-testing-SELECTION surface: GetRestoreTestingSelection drops modeled members of RestoreTestingSelectionForGet, and UpdateRestoreTestingSelection does not persist what it accepted. The open issue 20261114T123121Z-backup-aws-backup-restore-testing-plan-reads-omit-lastupdatetime.md records the analogous read-body fault on the restore-testing PLAN operations (GetRestoreTestingPlan's LastUpdateTime / ScheduleExpressionTimezone and plan updates not persisting); it names none of the SELECTION operations, members or fixtures below, so it does not cover these.

Cause 1 of 4: GetRestoreTestingSelection omits ProtectedResourceConditions and RestoreMetadataOverrides.
AWS: Smithy GetRestoreTestingSelection -> GET /restore-testing/plans/{RestoreTestingPlanName}/selections/{RestoreTestingSelectionName}, success 200, output member RestoreTestingSelection (required) targeting RestoreTestingSelectionForGet. That structure models ProtectedResourceConditions (target ProtectedResourceConditions, a structure of StringEquals/StringNotEquals KeyValueLists) and RestoreMetadataOverrides (SensitiveStringMap). The recording shows AWS serializing both even when the selection was created without them, as the empty-but-present forms: ProtectedResourceConditions {StringEquals: [], StringNotEquals: []} and RestoreMetadataOverrides {}.
Emulator: 200, but both members are absent from the body (replay reports them 'missing ... got None').
Why: a client that reads selection.ProtectedResourceConditions.StringEquals or iterates RestoreMetadataOverrides after a successful get dereferences null instead of an empty collection, so an SDK with non-optional accessors or any code that round-trips a get into an update crashes or silently drops the conditions/overrides it meant to preserve.
Fix: the GetRestoreTestingSelection response serializer / RestoreTestingSelectionForGet projection in the backup service crate - it must emit these members from stored selection state, defaulting to the empty structure and empty map rather than omitting them.
Traces: backup-restore-testing-plan-lifecycle#task#6 (also affects backup-restore-testing-selection-lifecycle#task#3 and backup-restore-testing-selection-lifecycle-wildcard#task#4).

Cause 2 of 4: the same omission of RestoreMetadataOverrides on a selection created WITH ProtectedResourceConditions.
AWS: as above, RestoreMetadataOverrides is a modeled member of RestoreTestingSelectionForGet and is serialized as {} when no overrides were supplied; the recording for a conditions-bearing selection returns the conditions AND RestoreMetadataOverrides {}.
Emulator: 200 with the conditions present but RestoreMetadataOverrides absent, showing the omission is in the member projection itself and not only in the all-defaults case.
Why: same null-vs-empty-map hazard as Cause 1, on the path a client is most likely to take (read a configured selection, then merge overrides).
Fix: same serializer; the member must be unconditionally present.
Traces: backup-restore-testing-selection-conditions-and-errors#task#4.

Cause 3 of 4: GetRestoreTestingSelection omits ProtectedResourceArns when the selection was created with the wildcard form.
AWS: RestoreTestingSelectionForGet.ProtectedResourceArns is a stringList, and its documentation states the caller may include specific ARNs or the wildcard ProtectedResourceArns: ["*"], but not both. The recording shows the wildcard read back verbatim as ["*"].
Emulator: 200 with ProtectedResourceArns absent entirely, so the wildcard the caller supplied at create time is not reported back.
Why: the selection's resource scope becomes unreadable - a client cannot tell a wildcard selection from one with no ARN scope at all, and a read-modify-write of the selection loses the wildcard.
Fix: the restore-testing-selection state module plus the same read projection - the wildcard list must be stored and echoed like any other ProtectedResourceArns value.
Traces: backup-restore-testing-selection-conditions-and-overrides#task#4.

Cause 4 of 4: UpdateRestoreTestingSelection is accepted but its IamRoleArn change is never applied.
AWS: Smithy UpdateRestoreTestingSelection -> PUT /restore-testing/plans/{RestoreTestingPlanName}/selections/{RestoreTestingSelectionName}, success 200, input member RestoreTestingSelection targeting RestoreTestingSelectionForUpdate whose members include IamRoleArn. A successful update must be visible to the next GetRestoreTestingSelection: the recording updates the selection's role and the following get returns the NEW role ARN.
Emulator: the update returns success, but the subsequent GetRestoreTestingSelection still returns the role ARN from create time (replay: IamRoleArn value_mismatch, the pre-update value) - and the same response also omits ProtectedResourceConditions and RestoreMetadataOverrides as in Cause 1.
Why: the mutation is silently discarded. A client that rotates the restore-testing role, or narrows a selection, gets a 200 and then reads back the stale configuration, so it believes a change landed that did not; restore tests keep running under the old role.
Fix: the UpdateRestoreTestingSelection handler and the restore-testing-selection state module - the accepted RestoreTestingSelectionForUpdate members must be merged into the stored record before the operation returns.
Traces: backup-restore-testing-selection-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 (emulator up: /__admin/health answered 200 and the data listener accepts).

Cause 1 - GetRestoreTestingSelection, expected 200 / actual 200:
  $.RestoreTestingSelection.ProtectedResourceConditions: missing (expected {'StringEquals': [], 'StringNotEquals': []}, got None)
  $.RestoreTestingSelection.RestoreMetadataOverrides: missing (expected {}, got None)
  fixtures: backup-restore-testing-plan-lifecycle#task#6; backup-restore-testing-selection-lifecycle#task#3; backup-restore-testing-selection-lifecycle-wildcard#task#4

Cause 2 - GetRestoreTestingSelection, expected 200 / actual 200:
  $.RestoreTestingSelection.RestoreMetadataOverrides: missing (expected {}, got None)
  fixture: backup-restore-testing-selection-conditions-and-errors#task#4

Cause 3 - GetRestoreTestingSelection, expected 200 / actual 200:
  $.RestoreTestingSelection.ProtectedResourceArns: missing (expected ['*'], got None)
  fixture: backup-restore-testing-selection-conditions-and-overrides#task#4

Cause 4 - GetRestoreTestingSelection after UpdateRestoreTestingSelection, expected 200 / actual 200:
  $.RestoreTestingSelection.IamRoleArn: value_mismatch (expected the role ARN supplied to the update, got the role ARN supplied at create time)
  $.RestoreTestingSelection.ProtectedResourceConditions: missing (expected {'StringEquals': [], 'StringNotEquals': []}, got None)
  $.RestoreTestingSelection.RestoreMetadataOverrides: missing (expected {}, got None)
  fixture: backup-restore-testing-selection-lifecycle#task#6

Model references (services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json): GetRestoreTestingSelection GET /restore-testing/plans/{RestoreTestingPlanName}/selections/{RestoreTestingSelectionName} code 200, output {RestoreTestingSelection: RestoreTestingSelectionForGet (required)}; RestoreTestingSelectionForGet members {CreationTime, CreatorRequestId, IamRoleArn, ProtectedResourceArns: stringList, ProtectedResourceConditions, ProtectedResourceType, RestoreMetadataOverrides: SensitiveStringMap, RestoreTestingPlanName, RestoreTestingSelectionName, ValidationWindowHours}; ProtectedResourceConditions members {StringEquals: KeyValueList, StringNotEquals: KeyValueList}; UpdateRestoreTestingSelection PUT .../selections/{RestoreTestingSelectionName} code 200, input RestoreTestingSelectionForUpdate members {IamRoleArn, ProtectedResourceArns, ProtectedResourceConditions, RestoreMetadataOverrides, ValidationWindowHours}.

## Steps to Reproduce
1. git clone the repository and boot the emulator built from that checkout.
2. Replay the wire corpus simparity-wire-aws-v1.
3. Cause 1: replay backup-restore-testing-plan-lifecycle through step task#6 - CreateRestoreTestingSelection then GetRestoreTestingSelection; the 200 body carries neither ProtectedResourceConditions nor RestoreMetadataOverrides.
4. Cause 2: replay backup-restore-testing-selection-conditions-and-errors step task#4 - a selection created with ProtectedResourceConditions reads back with the conditions but no RestoreMetadataOverrides member.
5. Cause 3: replay backup-restore-testing-selection-conditions-and-overrides step task#4 - a selection created with ProtectedResourceArns ["*"] reads back with no ProtectedResourceArns member.
6. Cause 4: replay backup-restore-testing-selection-lifecycle through step task#6 - UpdateRestoreTestingSelection changes IamRoleArn and returns success, then GetRestoreTestingSelection still reports the create-time role ARN.
