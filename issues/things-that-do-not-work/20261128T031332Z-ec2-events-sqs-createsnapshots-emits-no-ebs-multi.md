Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume
Issue-Key: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume|_collect#24|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 7da368b21ee7d04c70045aa1c38fe932bbd09ef4

# ec2__events__sqs: CreateSnapshots emits no "EBS Multi-Volume Snapshots Completion Status" event, so the multi-volume snapshot completion scenario never reaches a verdict

> _Note: this issue was identified against the code base at commit `7da368b21ee7d04c70045aa1c38fe932bbd09ef4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshots

## Description
Cause 1 of 1: a multi-volume CreateSnapshots completes its snapshots but publishes no EventBridge completion event.

AWS: recorded behavior only — the EC2 Smithy model (services/service-com-amazonaws-ec2-amazonec2) models CreateSnapshots as input InstanceSpecification/Description/TagSpecifications/CopyTagsFromSource/OutpostArn -> output Snapshots (SnapshotSet) with no modeled errors, and models no eventing at all; EventBridge emission is documented behavior, not modeled. AWS EBS publishes, on completion of a CreateSnapshots set, ONE EventBridge event per set on the default bus with source "aws.ec2" and detail-type "EBS Multi-Volume Snapshots Completion Status". Its detail carries event="createSnapshots", result="succeeded" and cause="" (the empty string) for a set that completed; detail.snapshots[] holds one entry per snapshot in the set, each entry being {snapshot_id, source, status} where snapshot_id is the snapshot's event-form ARN arn:aws:ec2::<region>:snapshot/<snapshot-id> (account field empty, region in the account slot), source is the source volume's event-form ARN arn:aws:ec2::<region>:volume/<volume-id>, and status is "completed"; detail.startTime and detail.endTime are ISO-8601 UTC instants with milliseconds (YYYY-MM-DDTHH:MM:SS.mmmZ) with startTime <= endTime. The envelope's resources[] is exactly that same set of snapshot ARNs, and the whole set produces a single event id — no per-snapshot event and no event naming a subset. With InstanceSpecification.ExcludeBootVolume=true the instance's root volume appears in neither Snapshots nor detail.snapshots[].

Emulator: the emulator accepts the request and mints the set correctly — CreateSnapshots returns Snapshots pairing exactly the two attached data volumes (the ExcludeBootVolume assertion passes) and DescribeSnapshots drives each snapshot to State=completed — but nothing is ever published to EventBridge for the set. The check's positive control runs first and passes (a PutEvents event on a sibling rule reaches its SQS target), so the rule, the target permission and EventBridge->SQS delivery all work; only the aws.ec2 multi-volume completion event is absent. The queue drains empty until the runner's budget is consumed and the check dies in _collect with "TimeoutError: the multi-volume completion event for ['snap-...', 'snap-...']: the run budget ended first; not graded", so none of the detail/resources/envelope assertions after it are ever reached.

Why: a client that reacts to snapshot completion through EventBridge — backup orchestration, DLM-style post-processing, a Lambda or Step Functions workflow waiting on "EBS Multi-Volume Snapshots Completion Status" before copying or sharing snapshots — never fires against the emulator. The snapshots exist and read back completed, so the divergence is invisible to a describe-based poll and only shows up as a workflow that hangs forever, which is exactly the shape of failure seen here. Any consumer asserting the paired snapshot/volume ARNs, the completed status, or the single-event-per-set guarantee cannot be exercised at all.

Fix: the CreateSnapshots handler and the EC2 snapshot state machine that settles a snapshot to 'completed' must publish the completion event for the set once the last snapshot in it reaches 'completed' — emitting one envelope to the default event bus with source aws.ec2, detail-type "EBS Multi-Volume Snapshots Completion Status", resources set to the snapshot event-form ARNs, and the detail (event/result/cause/snapshots[]/startTime/endTime) built from the set's own snapshot and source-volume ids and its start and completion instants. The event-form ARN builder (empty account field, region in the account slot) belongs with the existing EC2 snapshot event serialization; this is the same missing-emission family as the already-open issue 20261127T024328Z-ec2-events-sqs-copysnapshot-emits-no-ebs-snapshot.md, which covers the single-snapshot "EBS Snapshot Notification" for copySnapshot and does NOT cover this detail-type or CreateSnapshots.

Traces: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume (error at check line 74, _collect).

## Evidence
Failing check: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume [error]

Emulator (replay output):
  File "check_ec2__events__sqs_xsvc_ec2_1_create_snapshots_completion_lists_every_data_volume.py", line 74, in run
    _collect(sqs, queue, got, lambda g: bool(_for_set(g, expected_res)),
  File "check_ec2__events__sqs_xsvc_ec2_1_create_snapshots_completion_lists_every_data_volume.py", line 371, in _collect
    raise TimeoutError(f"{what}: the run budget ended first; not graded")
  TimeoutError: the multi-volume completion event for ['snap-000000000000000e3', 'snap-000000000000000e4']: the run budget ended first; not graded

Steps that PASSED before the failure (so they are not in question):
  - _prove_pipeline: PutEvents -> rule -> SQS target delivered the control event with the right source and account, proving EventBridge->SQS works in this account.
  - Two 1 GiB gp3 data volumes created, reached 'available', attached to a running instance at /dev/sdf and /dev/sdg and reached 'in-use'; the instance has its own distinct root volume.
  - CreateSnapshots(InstanceSpecification={InstanceId, ExcludeBootVolume: true}) returned Snapshots pairing exactly the two data volumes (root excluded).
  - Each returned snapshot reached DescribeSnapshots State='completed'.

Expected (AWS): one event on the default bus, envelope source "aws.ec2", detail-type "EBS Multi-Volume Snapshots Completion Status", resources = the set's snapshot ARNs in event form arn:aws:ec2::<region>:snapshot/<snapshot-id>; detail = {"event": "createSnapshots", "result": "succeeded", "cause": "", "snapshots": [{"snapshot_id": "arn:aws:ec2::<region>:snapshot/<snapshot-id>", "source": "arn:aws:ec2::<region>:volume/<source-volume-id>", "status": "completed"}, ...], "startTime": "<ISO-8601 UTC with milliseconds>", "endTime": "<ISO-8601 UTC with milliseconds>"}, one entry per snapshot in the set and none for the excluded boot volume, startTime <= endTime, and a single event id for the whole set. Fixed text here is the source, the detail-type and the literal values "createSnapshots", "succeeded", "" and "completed"; the snapshot ids, volume ids, region, account and the two timestamps are values taken from the request and the emulator's own state (the snap-/vol- ids and instants above are one run's values, not requirements).

Actual (emulator): no message of any kind on the rule's queue for the snapshot set; the drain loop received nothing until the run budget expired. The snapshots themselves were created and completed, so the gap is purely the missing EventBridge emission.

## Steps to Reproduce
1. Create a VPC and a subnet in an AZ that offers t3.micro; launch one t3.micro instance there and wait for 'running'.
2. Create an SQS queue, grant events.amazonaws.com sqs:SendMessage on it conditioned on the rule ARN, PutRule on the default bus with EventPattern {"source": ["aws.ec2"], "detail-type": ["EBS Multi-Volume Snapshots Completion Status"], "detail": {"event": ["createSnapshots"]}}, and PutTargets the queue.
3. (Positive control) On a second rule/queue pair, PutEvents a custom event and confirm it reaches its queue — it does, so the eventing path is healthy.
4. Create two gp3 volumes in that AZ, attach both to the instance (/dev/sdf, /dev/sdg) and wait for 'in-use'.
5. Call CreateSnapshots(InstanceSpecification={"InstanceId": <instance>, "ExcludeBootVolume": true}).
6. Poll DescribeSnapshots until both returned snapshots report State='completed'.
7. ReceiveMessage from the first queue. Expected: exactly one EventBridge envelope with source aws.ec2, detail-type "EBS Multi-Volume Snapshots Completion Status", resources = both snapshot event-form ARNs, and detail.snapshots[] pairing each snapshot with its source volume at status "completed". Observed: the queue stays empty until the budget expires.
