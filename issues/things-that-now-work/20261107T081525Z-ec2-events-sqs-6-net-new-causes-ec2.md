Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down ec2__events__sqs.events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop ec2__events__sqs.xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create ec2__events__sqs.xsvc-ec2-1-create-image-ami-state-change-available-then-deregistered ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down|run#17|AssertionError|assert message == expected, (
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification|_drain#26|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop|_drain#19|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs.xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create|_drain#17|Timeout|msgs = sqs.receive_message(QueueUrl=url, MaxNumberOfMessages=10,
Issue-Key: ec2__events__sqs|CreateImage|NotImplemented
Issue-Key: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume|_collect#24|TimeoutError|raise TimeoutError(f"{what}: the run budget ended first; not graded")
Issue-Commit: 145c20a94e0bd5c99471752c6f247be888f6ff73

# ec2__events__sqs: 6 net-new causes — EC2 publishes no instance/volume/AMI/multi-volume-snapshot lifecycle events to EventBridge, CreateImage is a 501 stub, and the termination-protection refusal loses its final period

> _Note: this issue was identified against the code base at commit `145c20a94e0bd5c99471752c6f247be888f6ff73`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RunInstances

## Description
Six distinct root causes in this scenario service. Cause 1 is a refusal-message defect; Causes 2-4 and 6 are missing service-emitted EventBridge events; Cause 5 is an unimplemented operation that blocks its event entirely.

Cause 1 of 6: the TerminateInstances termination-protection refusal message is missing its terminating period.
AWS: Smithy models `TerminateInstances` with no modeled errors (ec2Query common errors), so recorded behavior only for the wording. The recording shows HTTP 400 with Code `OperationNotPermitted` and the message template `The instance '<instance-id>' may not be terminated. Modify its 'disableApiTermination' instance attribute and try again.` — the fixed text is everything but `<instance-id>`, which is the instance id from the request; the sentence ends with a period.
Emulator: identical status and code, but the message ends `... and try again` with no period.
Why: a client matching AWS's exact refusal sentence (or asserting on complete sentences in operator output) fails on a correct refusal; the divergence is purely in the message AWS specifies.
Fix: the TerminateInstances handler's refusal message construction (services/service-com-amazonaws-ec2-amazonec2/src/ops/terminate_instances.rs).
Traces: ec2__events__sqs.events-ec2-lifecycle-protected-terminate-rejected-emits-no-shutting-down.

Cause 2 of 6: no `EBS Volume Notification` event is published for volume deletion, so the VolumeInUse/attached-volume leg never settles.
AWS: recorded behavior only. DeleteVolume on an attached volume is refused `VolumeInUse` and EC2 emits no deleteVolume event for it, while an unattached sibling's delete emits an `aws.ec2` / `EBS Volume Notification` with `event: deleteVolume` and `result: deleted` on the same rule; after detaching, the same volume's delete emits that event too.
Emulator: no `EBS Volume Notification` ever arrives on the rule's queue, so the check cannot even observe the positive control and exhausts its budget (`Check timed out after 60.0s`).
Why: EBS lifecycle observability is silent — a consumer that reacts to volume deletion (cost cleanup, inventory) never fires, and the negative guarantee (no event for a refused delete) cannot be distinguished from "no events at all".
Fix: the EBS volume state component and its EventBridge publication (the ec2 service's volume lifecycle, alongside services/service-com-amazonaws-ec2-amazonec2/src/ops/delete_volume.rs).
Traces: ec2__events__sqs.events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification.

Cause 3 of 6: no `EC2 Instance State-change Notification` event is published for instance start/stop transitions.
AWS: recorded behavior only. Starting instance A publishes an `aws.ec2` / `EC2 Instance State-change Notification` with `detail.state` `running` naming A; stopping instance B publishes one with `detail.state` `stopped`; a rule pinned to `state: running` receives exactly A's running event and nothing for B, and a broad rule over the same wiring receives B's stopped event, with both rules carrying the same event id for A.
Emulator: neither rule ever receives an instance state-change event; the check exhausts its 60s budget.
Why: instance state-change events are the canonical EC2 lifecycle signal; every pattern-precision and fan-out guarantee built on them is unobservable, and clients waiting on `running`/`stopped` hang.
Fix: the EC2 instance state machine's EventBridge publication (the ec2 service's instance lifecycle/state component driving run/start/stop/terminate transitions).
Traces: ec2__events__sqs.events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop.

Cause 4 of 6: `CreateVolume` publishes no `EBS Volume Notification` (createVolume/available), so the attach/detach negative guarantee cannot be shown.
AWS: recorded behavior only. CreateVolume emits an `aws.ec2` / `EBS Volume Notification` with `event: createVolume` and `result: available`, while a successful AttachVolume and DetachVolume emit none: a rule pinned to `attachVolume` stays silent while an unpinned rule receives exactly the createVolume event, and re-patterning the pinned rule to `createVolume` makes it deliver.
Emulator: the unpinned rule never receives the createVolume event either; the check exhausts its 60s budget.
Why: same as Cause 2 for the create side — the only positive event in the scenario is missing, so a client cannot tell "attach emits nothing" (correct) from "EC2 emits nothing" (the emulator).
Fix: the volume-creation path's EventBridge publication (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs and the volume state component).
Traces: ec2__events__sqs.xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create.

Cause 5 of 6: CreateImage answers 501 NotImplemented, so no `EC2 AMI State Change` event can exist.
AWS: Smithy models `CreateImage` (required `InstanceId`, required `Name` @length 3-128, optional `NoReboot`, `BlockDeviceMappings`, `TagSpecifications`) returning `ImageId`. The recording shows it accepted, the AMI reaching `available` with an `aws.ec2` / `EC2 AMI State Change` event, and `DeregisterImage` producing the `deregistered` state change.
Emulator: `ClientError: An error occurred (NotImplemented) when calling the CreateImage operation: operation CreateImage is scaffolded but not implemented in this emulator` — HTTP 501 where AWS answers 200 with an ImageId.
Why: AMI creation is unavailable, and the whole AMI state-change event chain behind it is unreachable; a client that bakes an image cannot proceed at all.
Fix: implement the CreateImage handler (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_image.rs) and have the image state machine publish `EC2 AMI State Change` for `available` and `deregistered`. The open issue 20261023T102725Z-ec2-createimage-stub.md names the stub itself; it does not name the AMI state-change event this scenario needs.
Traces: ec2__events__sqs.xsvc-ec2-1-create-image-ami-state-change-available-then-deregistered.

Cause 6 of 6: `CreateSnapshots` publishes no `EBS Multi-Volume Snapshots Completion Status` event.
AWS: recorded behavior only. A multi-volume CreateSnapshots run publishes one `aws.ec2` / `EBS Multi-Volume Snapshots Completion Status` event whose detail lists every snapshot produced (one per data volume) once they complete.
Emulator: the snapshots are returned but no completion event is ever delivered; the check's collector raises `TimeoutError: the multi-volume completion event for [<snapshot ids>]: the run budget ended first; not graded` (the snapshot ids there are per-run values).
Why: a client that waits for the multi-volume completion signal before promoting a backup set waits forever.
Fix: the CreateSnapshots handler and snapshot state component must publish the completion event (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_snapshots.rs). Related but not covering: 20261025T002357Z-ec2-events-sqs-ec2-createsnapshot-is-an-unimplemented-scaffold-stub-so.md and 20261104T221115Z-ec2-events-sqs-deletesnapshot-answers-501-notimplemented-and.md cover single-snapshot CreateSnapshot/DeleteSnapshot and the `EBS Snapshot Notification`, not the multi-volume completion event.
Traces: ec2__events__sqs.xsvc-ec2-1-create-snapshots-completion-lists-every-data-volume.

## Evidence
Cause 1 — AssertionError: OperationNotPermitted message: expected "The instance 'i-00000000000000001' may not be terminated. Modify its 'disableApiTermination' instance attribute and try again.", got "The instance 'i-00000000000000001' may not be terminated. Modify its 'disableApiTermination' instance attribute and try again" (instance id is a per-run value; the divergence is the missing final period).
Cause 2 — events-ec2-lifecycle-ebs-attached-volume-delete-rejected-emits-no-notification: 'Check timed out after 60.0s' polling for aws.ec2 / 'EBS Volume Notification' (deleteVolume, result=deleted).
Cause 3 — events-ec2-lifecycle-running-rule-fires-on-start-not-on-stop: 'Check timed out after 60.0s' polling for aws.ec2 / 'EC2 Instance State-change Notification' on both a state=running rule and a broad rule.
Cause 4 — xsvc-ec2-1-attach-and-detach-emit-no-volume-notification-unlike-create: 'Check timed out after 60.0s' polling for the createVolume/available EBS Volume Notification on the unpinned rule.
Cause 5 — botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the CreateImage operation: operation CreateImage is scaffolded but not implemented in this emulator.
Cause 6 — TimeoutError: the multi-volume completion event for ['snap-...','snap-...']: the run budget ended first; not graded (check line 371, _collect).

## Steps to Reproduce
Shared setup: a VPC + subnet, an AL2023 t3.micro instance, and an EventBridge rule on the default bus with an SQS target and a queue policy allowing events.amazonaws.com.
1. Cause 1: run an instance with DisableApiTermination=true, call TerminateInstances on it and read the OperationNotPermitted message — it lacks the final period.
2. Cause 2: create two volumes, attach one; DeleteVolume the attached one (refused VolumeInUse) and DeleteVolume the unattached sibling; poll the rule's queue for an 'EBS Volume Notification' — none arrives.
3. Cause 3: start instance A and stop instance B behind a rule pinned to {"detail":{"state":["running"]}} plus a broad rule; poll both queues — no 'EC2 Instance State-change Notification' arrives.
4. Cause 4: CreateVolume, then AttachVolume/DetachVolume, behind rules pinned to attachVolume and to the broad pattern; poll — the createVolume/available event never arrives.
5. Cause 5: call CreateImage(InstanceId=<id>, Name=<name>) — 501 NotImplemented.
6. Cause 6: attach two data volumes and call CreateSnapshots(InstanceSpecification={...}); poll for 'EBS Multi-Volume Snapshots Completion Status' — none arrives.
