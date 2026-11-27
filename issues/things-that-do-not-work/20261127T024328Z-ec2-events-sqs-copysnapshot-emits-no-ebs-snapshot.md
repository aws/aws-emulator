Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously|_collect#26|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: b4d546d4232932d32ca317b0739ae1583d9e6982

# ec2/events/sqs: CopySnapshot emits no 'EBS Snapshot Notification' (copySnapshot succeeded/failed) to EventBridge

> _Note: this issue was identified against the code base at commit `b4d546d4232932d32ca317b0739ae1583d9e6982`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CopySnapshot

## Description
One net-new root cause, grounded in one failing check the suite lists under `ec2__events__sqs`.

Cause 1 of 1: EC2 publishes no `EBS Snapshot Notification` event for a CopySnapshot, for either outcome, so a rule filtered on detail.event=copySnapshot never fires. Failing check: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously.

AWS: recorded behavior only — EBS's EventBridge notifications are not expressed in the EC2 Smithy model (CopySnapshot itself is modeled: it is accepted synchronously for a source id whose snapshot has been deleted and mints a new `snap-` id, and the copy then settles into State=error with StateMessage 'Source snapshot is not found'; the emulator already does both of these correctly in this run). Every CopySnapshot that reaches a terminal state puts one event on the default bus of the destination region with source `aws.ec2`, detail-type `EBS Snapshot Notification`, and a detail object carrying `event` = the fixed token `copySnapshot`, `result` = the fixed token `succeeded` or `failed`, `cause` (empty string on success, the failure reason otherwise), `snapshot_id` = the ARN of the newly minted copy, and `source` = the ARN of the source snapshot — the ARNs, region and timestamps come from the request and the emulator's state, the detail-type and the `event`/`result` tokens are fixed text.

Emulator: CopySnapshot is accepted, the control copy reaches `completed` and the missing-source copy reaches State=error with the right StateMessage, but no event with detail.event=copySnapshot ever arrives on the rule's SQS target for either snapshot; the check's drain budget expires with nothing collected (TimeoutError: 'copySnapshot notifications for both <good> and <bad>: the run budget ended first; not graded'). The check proves the events→SQS path itself is live first (an explicit control event on the same rule/queue is received), so the gap is the EC2 side never publishing the notification, not the bus or the target.

Why: a client that watches EBS snapshot notifications learns asynchronously whether a copy succeeded or failed; against the emulator that signal never arrives, so copy-completion workflows (replication pipelines, backup orchestration, cleanup on copySnapshot/failed) hang forever instead of being driven by the bus, and the only way to observe the outcome is to poll DescribeSnapshots.

Fix: the EC2 snapshot state machine — the CopySnapshot handler and the terminal-state transition it drives must publish the `EBS Snapshot Notification` event (detail.event=copySnapshot, result succeeded/failed, cause, snapshot_id, source) onto the default event bus, alongside the other EBS notification emissions. Context: related open issues cover sibling EBS/EC2 notification gaps (20261107T075601Z-ec2-events-sqs-ec2-eventbridge-volume-instance-lifecycle-notifications-never-arrive.md, 20261116T003513Z-ec2-events-sqs-no-ebs-volume-notification-reaches.md, 20261107T081525Z-ec2-events-sqs-6-net-new-causes-ec2.md); none of them carries this check's replay key or names the copySnapshot notification, so this cause is filed separately.

Traces: ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously (collection at check line 94 / helper line 362).

## Evidence
ec2__events__sqs.events-ec2-lifecycle-copy-snapshot-from-missing-source-fails-asynchronously:
  File "...copy_snapshot_from_missing_source_fails_asynchronously.py", line 94, in run -> _collect(sqs, queue, got, ...)
  File "...", line 362, in _collect -> raise TimeoutError(f"{what}: the run budget ended first; not graded")
  TimeoutError: copySnapshot notifications for both snap-<control copy> and snap-<failed copy>: the run budget ended first; not graded

The rule under test is PutRule with pattern {"source": ["aws.ec2"], "detail-type": ["EBS Snapshot Notification"], "detail": {"event": ["copySnapshot"]}} targeting an SQS queue; the check first proves that same rule/queue delivers a control event, so EventBridge→SQS is working.

Expected on that queue: two envelopes, source 'aws.ec2', detail-type 'EBS Snapshot Notification' —
  - for the copy of a live snapshot: detail.result='succeeded', detail.cause='', detail.snapshot_id=<ARN of the new copy>, detail.source=<ARN of the live source snapshot>
  - for the copy whose source was deleted: detail.result='failed', detail.cause naming the missing source, detail.snapshot_id=<ARN of the new copy>, detail.source=<ARN of the deleted source>
  (ARNs/region/ids come from the request and state; 'copySnapshot', 'succeeded', 'failed' and the detail-type are fixed text.)
Actual: zero matching messages within the drain budget.

Already correct in this run, and therefore not part of this defect: CopySnapshot from a deleted source is accepted (not refused synchronously), mints a fresh snap- id, and that copy reaches State=error with StateMessage 'Source snapshot is not found'.

## Steps to Reproduce
1. PutRule with EventPattern {"source": ["aws.ec2"], "detail-type": ["EBS Snapshot Notification"], "detail": {"event": ["copySnapshot"]}} and PutTargets an SQS queue whose policy allows events.amazonaws.com; confirm the rule/queue pair delivers a control PutEvents entry.
2. CreateVolume, CreateSnapshot, and wait for the snapshot to reach 'completed'.
3. CopySnapshot from that live snapshot (SourceRegion = the current region) and wait for the copy to reach 'completed'.
4. CreateSnapshot a second snapshot, wait for 'completed', DeleteSnapshot it and wait until DescribeSnapshots no longer returns it; then CopySnapshot from that deleted id and wait for the copy to reach State=error.
5. ReceiveMessage the queue.
   Expected: one 'EBS Snapshot Notification' per copy — detail.event='copySnapshot' with result 'succeeded' (empty cause) for step 3's copy and 'failed' for step 4's, each naming snapshot_id (the copy) and source (the source snapshot).
   Emulator: no copySnapshot notification is ever delivered.
