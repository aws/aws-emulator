Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__glue.cfn-glue-table-in-place-column-update-retained-replacement-keeps-partitions-then-not-found
Issue-Key: cloudformation__glue.cfn-glue-table-in-place-column-update-retained-replacement-keeps-partitions-then-not-found|_assert_replaced_and_retained#33|AssertionError|assert (
Issue-Commit: e18aca9896c14b293d2c6f422ce254ac7004d2d3

# cloudformation__glue: a replaced AWS::Glue::Table is reported with CREATE_*/DELETE_* stack events instead of UPDATE_* with the replacement reason, and the retained old table is deleted despite UpdateReplacePolicy Retain

> _Note: this issue was identified against the code base at commit `e18aca9896c14b293d2c6f422ce254ac7004d2d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateStack

## Description
One net-new root cause, observed on an AWS::Glue::Table whose create-only table name changes during a stack update while the resource declares "UpdateReplacePolicy": "Retain". The failing check is marked neither ALREADY FILED nor OWNED by any open issue.

Cause 1 of 1: the resource events of a replacement are emitted as a fresh CREATE plus a real DELETE, not as an UPDATE that announces the replacement and skips the retained delete.
AWS: Smithy (cloudformation) DescribeStackEvents -> StackEvent{ResourceStatus, ResourceStatusReason, PhysicalResourceId}; ResourceStatus is a modeled enum that includes UPDATE_IN_PROGRESS, UPDATE_COMPLETE and DELETE_SKIPPED. An update that changes a create-only property replaces the resource INSIDE the update: for that logical id CloudFormation emits UPDATE_IN_PROGRESS whose ResourceStatusReason is the fixed text 'Requested update requires the creation of a new physical resource; hence creating one.', then UPDATE_IN_PROGRESS 'Resource creation Initiated', then UPDATE_COMPLETE carrying the NEW PhysicalResourceId (for AWS::Glue::Table the table name, a request value). With UpdateReplacePolicy Retain the old physical resource is NOT deleted in the cleanup phase: exactly one DELETE_SKIPPED event carrying the OLD PhysicalResourceId is emitted and no DELETE_IN_PROGRESS/DELETE_COMPLETE for that logical id, so the old Glue table and its partitions survive the update (glue GetTable/GetPartitions on the old name keep answering) while GetTable on the stack-owned name that was removed is refused with EntityNotFoundException.
Emulator: for the replaced logical id the update emits CREATE_IN_PROGRESS (ResourceStatusReason absent, PhysicalResourceId the empty string), CREATE_IN_PROGRESS 'Resource creation Initiated', CREATE_COMPLETE on the new table name, then DELETE_IN_PROGRESS (PhysicalResourceId absent) and DELETE_COMPLETE carrying the OLD table name — no UPDATE_* event, no replacement reason, no DELETE_SKIPPED, and the table the policy says to retain is really deleted. The table names in the observed rows are per-run request values; the missing statuses and the missing fixed reason text are the requirement.
Why: a client reading DescribeStackEvents cannot tell a replacement from a first-time create and never sees the reason that announces a catalog table (and the partitions hanging off it) is being re-created, so change-review tooling that greps for the replacement reason finds none. Worse, UpdateReplacePolicy Retain is the documented way to keep catalog metadata across a replacement; the emulator destroys it and records the destruction as a successful delete, so the loss is invisible in the stack's own record.
Fix: the CloudFormation update engine — the resource-event emission on the replacement path, and the cleanup phase that must honour UpdateReplacePolicy (Retain/Snapshot/Delete) and emit DELETE_SKIPPED with the old physical id when it is Retain. Not the Glue provisioner. The same engine behaviour is already recorded for AWS::Glue::Database in the open issue 20261111T102615Z-cloudformation-glue-a-replaced-aws-glue-database-is.md (and for other resource types in 20261110T232635Z / 20261111T003921Z / 20261110T232651Z); none of their replay sets names this check, so this recorded unit is not re-replayed by any of them.
Traces: cloudformation__glue.cfn-glue-table-in-place-column-update-retained-replacement-keeps-partitions-then-not-found.

## Evidence
Replay output (behavioral suite, section ### cloudformation__glue):
  [mismatch] cloudformation__glue.cfn-glue-table-in-place-column-update-retained-replacement-keeps-partitions-then-not-found
  AssertionError: Table must announce its replacement, its events were [('CREATE_IN_PROGRESS', None, ''), ('CREATE_IN_PROGRESS', 'Resource creation Initiated', '<new table>'), ('CREATE_COMPLETE', None, '<new table>'), ('DELETE_IN_PROGRESS', None, None), ('DELETE_COMPLETE', None, '<old table>')]

Rows are (ResourceStatus, ResourceStatusReason, PhysicalResourceId) for the replaced 'Table' logical id, oldest first, from DescribeStackEvents; the table names are per-run request values.

Expected (AWS), in order, for that logical id:
  ('UPDATE_IN_PROGRESS', 'Requested update requires the creation of a new physical resource; hence creating one.', <any>)
  ('UPDATE_COMPLETE', None, '<new table>')
  ('DELETE_SKIPPED', None, '<old table>')
and NO DELETE_IN_PROGRESS / DELETE_COMPLETE for it; afterwards glue GetTable and GetPartitions on '<old table>' still answer with the table and its partitions.

Actual (emulator): the CREATE_* triple above (first row with no reason and an empty PhysicalResourceId), then DELETE_IN_PROGRESS + DELETE_COMPLETE on the old table, which is gone with its partitions.

## Steps to Reproduce
1. Build and boot the emulator from this checkout (cloudformation and glue both serve).
2. CreateStack with one AWS::Glue::Database and one AWS::Glue::Table (pinned table Name, StorageDescriptor with Columns/Location/InputFormat/OutputFormat/SerdeInfo and PartitionKeys [{Name: dt, Type: string}]) where the Table declares "UpdateReplacePolicy": "Retain". Wait for CREATE_COMPLETE, then glue CreatePartition on that table.
3. UpdateStack with the same template but a different table Name (create-only, so the update must replace). Wait for UPDATE_COMPLETE.
4. DescribeStackEvents for the 'Table' logical id: expect UPDATE_IN_PROGRESS with ResourceStatusReason 'Requested update requires the creation of a new physical resource; hence creating one.' then UPDATE_COMPLETE carrying the new table name; observe CREATE_IN_PROGRESS/CREATE_COMPLETE with no reason.
5. Expect exactly one DELETE_SKIPPED carrying the OLD table name and no DELETE_* progress events for it; observe DELETE_IN_PROGRESS + DELETE_COMPLETE.
6. glue GetTable / GetPartitions on the OLD table name: expect the table and its partitions to survive the update (and the later DeleteStack); observe EntityNotFoundException because the retained table was deleted.
7. Or re-replay: cloudformation__glue.cfn-glue-table-in-place-column-update-retained-replacement-keeps-partitions-then-not-found.
