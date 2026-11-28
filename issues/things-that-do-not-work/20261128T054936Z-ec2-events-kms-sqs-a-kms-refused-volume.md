Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected
Issue-Key: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected|_drain#21|Timeout|return [json.loads(m["Body"]) for m in msgs]
Issue-Commit: cb20f68e94bbc6cb070c362820a8401b3373aae4

# ec2__events__kms__sqs: a KMS-refused volume's withdrawal publishes no deleteVolume/deleted EBS Volume Notification and the failed createVolume event omits the KMS key ARN from resources (2 causes)

> _Note: this issue was identified against the code base at commit `cb20f68e94bbc6cb070c362820a8401b3373aae4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVolume

## Description
Two net-new root causes in EC2's EBS Volume Notification publication, both reached by the same failing check. NOTE: open issue 20261116T003513Z-ec2-events-kms-sqs-a-createvolume-under-a.md describes an earlier state of this code in which NO "EBS Volume Notification" was published at all; the replay did NOT mark this check as owned by it, and the checkout now does publish the createVolume outcome (services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs builds crate::state::VolumeNotification::of_create and calls publish_when_due). What remains unpublished and misrendered is described below.

Cause 1 of 2 — the asynchronous WITHDRAWAL of a volume whose encryption key refused its data key publishes no notification at all. Failing check: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected.
AWS: recorded behavior only (the EC2 Smithy model declares no error shapes and no event schema for CreateVolume; smithy_query on CreateVolume reports errors: []). What the recording shows: CreateVolume(Encrypted=true, KmsKeyId=<key in KeyState Disabled>) is ACCEPTED synchronously and answers a volume whose State is `creating`; the refusal is asynchronous. EBS then publishes onto the account's default EventBridge bus exactly ONE envelope with source `aws.ec2`, detail-type `EBS Volume Notification`, `account` = the caller's account, `region` = the request's Region, detail.event `createVolume`, detail.result `failed`, detail.cause = the fixed text ` is disabled.` preceded by the KMS key ARN taken from the request, AND a SECOND envelope that withdraws the volume: detail.event `deleteVolume`, detail.result `deleted`, carrying the SAME detail.request-id as the failed createVolume envelope. After the withdrawal DescribeVolumes on that volume id is refused `InvalidVolume.NotFound` with the message `The volume '` + the requested volume id + `' does not exist.` (fixed text around a value from the request). A volume under an ENABLED key is never withdrawn and publishes only createVolume/`available` with detail.cause = the empty string.
Emulator: the create's outcome is published (one createVolume envelope), but the withdrawal is applied purely as a store sweep with no publication: crate::state::ComAmazonawsEc2Amazonec2State::withdraw_due_unencryptable_volumes removes the record via remove_volume and nothing builds or publishes VolumeNotification::of_delete for it (of_delete is reached only from the caller-initiated DeleteVolume path). The check therefore never satisfies its first delivery barrier, which requires createVolume for the control volume plus createVolume AND deleteVolume for the disabled-key volume, and is killed by the harness: `Error: Check timed out after 60.0s` (the check's own in-check delivery budget is 300s). Relatedly, VolumeNotification::of_delete mints a fresh request id (service_common::query_error_envelope::new_request_id), so a withdrawal published through it could not carry the failed create's request-id as AWS does; the withdrawal event must reuse the createVolume outcome's request-id (the identifier is a per-run value — what is required is that the two envelopes agree).
Why: a client that drives automation off a volume's fate (an alarm, a cleanup or remediation rule, a cost-inventory job) is told the volume FAILED but is never told it was withdrawn, and cannot correlate the two halves of one create by request-id. Code that waits for the deleteVolume/deleted event before releasing the key or the volume's quota waits forever, even though the volume has in fact disappeared from DescribeVolumes.
Fix: EC2's volume-lifecycle event publication — the withdrawal path for a key-refused volume (crate::state's withdraw-due-withdrawals sweep together with the CreateVolume handler's settlement that owes it) must publish one deleteVolume/`deleted` EBS Volume Notification per withdrawn volume, carrying the request-id of that volume's createVolume outcome.

Cause 2 of 2 — a FAILED createVolume notification's `resources` omits the KMS key ARN. Failing check: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected.
AWS: recorded behavior only. A createVolume notification whose detail.result is `failed` because of the encryption key carries TWO entries in the envelope's `resources`, in order: the volume's ARN, then the ARN of the KMS key that refused. A createVolume notification whose result is `available` carries only the volume's ARN.
Emulator: every volume notification carries exactly one resource. services/service-com-amazonaws-ec2-amazonec2/src/state/volume_notifications.rs holds only `volume_arn` on VolumeNotification and its publish() sets `.resources(&self.volume_arn)`, so the key ARN is not in the event even on the failed path where of_create was given the VolumeKeyRefusal. A consumer rule or handler that reads the second resource to learn WHICH key killed the volume gets nothing; the key is recoverable only by parsing the free-text detail.cause.
Why: a rule filtering or a remediation keyed on the KMS key ARN in `resources` never matches, so the failure cannot be routed to the key's owner the way it is on AWS.
Fix: the volume-notification renderer (VolumeNotification and its publish()) must carry the refusing key's ARN beside the volume ARN and emit both in `resources`, volume first, for a failed create only.

Traces: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected (cause 1 blocks the check at its delivery barrier; cause 2 is the next assertion the same check makes on the failed envelope).

## Evidence
From the behavioral replay (### ec2__events__kms__sqs):

[mismatch] ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected
Error: Check timed out after 60.0s

Expected (recorded AWS behavior, as the check asserts it):
- CreateVolume(Encrypted=true, KmsKeyId=<disabled key>) accepted, State == "creating".
- One envelope: source "aws.ec2", detail-type "EBS Volume Notification", account == caller account, region == request region, detail.event "createVolume", detail.result "failed", detail.cause "<kms key arn> is disabled.", resources == ["<volume arn>", "<kms key arn>"].
- One envelope: detail.event "deleteVolume", detail.result "deleted", detail.request-id identical to the failed createVolume envelope's.
- DescribeVolumes(VolumeIds=[<withdrawn volume>]) -> "InvalidVolume.NotFound", message "The volume '<volume-id>' does not exist."
- Enabled-key control: one createVolume envelope, result "available", cause "", resources == ["<volume arn>"], never withdrawn.

Actual, from the checkout the replay ran against:
- services/service-com-amazonaws-ec2-amazonec2/src/ops/create_volume.rs settles the key-refused volume by setting record.withdrawn_at and publishes exactly ONE event per create ("exactly ONE event per volume this call created"); the withdrawal itself is applied by crate::state::ComAmazonawsEc2Amazonec2State::withdraw_due_unencryptable_volumes, which calls remove_volume and publishes nothing. No deleteVolume/deleted envelope can ever reach the rule's SQS target, so the check's first barrier (createVolume for the control AND createVolume + deleteVolume for the disabled-key volume) is never satisfied and the harness kills the check at 60s.
- services/service-com-amazonaws-ec2-amazonec2/src/state/volume_notifications.rs: VolumeNotification carries a single `volume_arn` and publish() emits `.resources(&self.volume_arn)`, so a failed createVolume envelope's resources is [volume arn] where AWS sends [volume arn, kms key arn]; VolumeNotification::of_delete mints a fresh request id rather than reusing the create outcome's.

## Steps to Reproduce
1. Create two KMS keys A and B; DisableKey on A and confirm DescribeKey reports KeyState "Disabled".
2. Create an SQS queue; create an ENABLED default-bus EventBridge rule with EventPattern {"source":["aws.ec2"],"detail-type":["EBS Volume Notification"]}; set a queue policy allowing events.amazonaws.com sqs:SendMessage for that rule ARN; PutTargets the queue (FailedEntryCount must be 0).
3. CreateVolume(AvailabilityZone=<an available zone>, Size=1, VolumeType=gp3, Encrypted=true, KmsKeyId=<key B>) — the control — then the identical call with KmsKeyId=<key A> (disabled). Both are accepted with State "creating".
4. Poll ReceiveMessage on the queue. Expect three envelopes: createVolume/available for the key-B volume, createVolume/failed for the key-A volume, and deleteVolume/deleted for the key-A volume sharing the failed envelope's detail.request-id. Observed: the two createVolume envelopes only — no deleteVolume/deleted ever arrives, so a consumer waiting on the withdrawal blocks (the check is killed at the 60s harness cap).
5. Inspect the failed createVolume envelope: expect resources == ["<volume arn>", "<key A arn>"]. Observed: resources == ["<volume arn>"] only.
6. DescribeVolumes on the key-A volume id: expect InvalidVolume.NotFound, "The volume '<id>' does not exist."
7. EnableKey on A and repeat the CreateVolume: expect one createVolume/available envelope and DescribeVolumes reporting Encrypted=true with KmsKeyId = key A.
Replay: ec2__events__kms__sqs.xsvc-ec2-2-create-volume-under-disabled-key-emits-one-failed-notification-rejected.
