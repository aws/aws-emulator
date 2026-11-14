Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item
Issue-Key: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item|_poll_until#28|TimeoutError|raise TimeoutError(f"{description}: the run budget ended first; not graded")
Issue-Commit: c17c6e54c038586d9d6328bb68b9d1b3d6bcf360

# cloudformation__config__s3: a CloudFormation-provisioned Config delivery channel now records the bucket but the on-demand snapshot never reaches lastStatus SUCCESS and no snapshot object is written (1 cause)

> _Note: this issue was identified against the code base at commit `c17c6e54c038586d9d6328bb68b9d1b3d6bcf360`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeliverConfigSnapshot

## Description
ONE root cause. ONE CAUSE must be closed before submitting.

The check's service token is the composite suite token `cloudformation__config__s3`; the underlying services (cloudformation, configservice, s3) are all bootstrapped and the defect belongs to configservice's snapshot-delivery pipeline.

Cause 1 of 1: after a CloudFormation stack provisions the delivery bucket, its Config bucket policy and the recorder role, and the recorder is started and reports recording=true, DeliverConfigSnapshot returns a configSnapshotId but the delivery is never performed: DescribeDeliveryChannelStatus's DeliveryChannelsStatus[0].configSnapshotDeliveryInfo never settles on a successful terminal status, and no object is ever written to the delivery bucket.

AWS: recorded behavior only, with model grounding. Smithy (configservice): DeliverConfigSnapshot{deliveryChannelName required, @length 1..256} -> DeliverConfigSnapshotResponse{configSnapshotId}; its only modelled errors are NoSuchDeliveryChannelException / NoAvailableConfigurationRecorderException / NoRunningConfigurationRecorderException (all errorKind client, httpError 400), so once the call is ACCEPTED the delivery outcome is reported nowhere but configSnapshotDeliveryInfo and the S3 object. DescribeDeliveryChannelStatus -> {DeliveryChannelsStatus: [DeliveryChannelStatus{name, configSnapshotDeliveryInfo, configHistoryDeliveryInfo, configStreamDeliveryInfo}]}; ConfigExportDeliveryInfo carries lastStatus (enum DeliveryStatus = Success | Failure | Not_Applicable), lastErrorCode, lastErrorMessage, lastAttemptTime, lastSuccessfulTime, nextDeliveryTime. Recorded behavior for an accepted DeliverConfigSnapshot on a channel whose bucket policy grants config.amazonaws.com s3:GetBucketAcl, s3:ListBucket and s3:PutObject: configSnapshotDeliveryInfo settles on a SUCCESS lastStatus with lastAttemptTime and lastSuccessfulTime set, and exactly one object appears in the delivery bucket under the key prefix AWSLogs/<account-id>/Config/<region>/ whose key contains the returned configSnapshotId, is gzip-framed (first two bytes 0x1f 0x8b) and gunzips to JSON carrying a configurationItems array that includes the recorded bucket's item (ARN, resourceId = the bucket name, awsRegion = the request region). Fixed requirements are the AWSLogs/<account>/Config/<region>/ prefix shape, the gzip framing, the configurationItems payload and the terminal status transition; the account id, region, bucket/channel/recorder names and the snapshot id are values taken from the request and the emulator's state and are NOT part of the template.

Emulator: the run now gets FURTHER than the previously reported stall. The check no longer dies in the configuration-item poll — it reaches check line 511 and dies in `_poll_until(..., description="snapshot delivery reaches SUCCESS")` with TimeoutError: "snapshot delivery reaches SUCCESS: the run budget ended first; not graded". So the recorder's configuration item for the observed bucket is now produced, DeliverConfigSnapshot is accepted and returns a configSnapshotId, and the remaining divergence is strictly downstream: configSnapshotDeliveryInfo.lastStatus never becomes a successful terminal value (and no Failure with lastErrorCode/lastErrorMessage is reported either — the field simply never transitions), and the gzipped snapshot object naming that configSnapshotId never appears in the delivery bucket. The poll carries its own 420 s budget inside the check, so this is a pipeline that never transitions, not a slow one.

Related open issue, named for overlap but NOT relied on as cover (its replay key no longer matches this check — the failure has moved from the configuration-item leg to the snapshot-delivery leg, and the suite did not mark this check OWNED): 20261026T031628Z-cloudformation-config-s3-a-cloudformation-provisioned-config-delivery.md. Also related and named, not relied on: 20261025T014224Z-cloudformation-config-s3-snapshot-delivery-never-reaches-a.md (the delivery-DENIED leg of the same scenario) and 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md.

Why: a client that wires Config delivery through CloudFormation gets a pipeline that is accepted at every control-plane call and silently delivers nothing. An audit or inventory consumer that fetches and gunzips the delivered snapshot from the delivery bucket sees an empty bucket forever, and because configSnapshotDeliveryInfo never leaves its initial state there is no error anywhere to distinguish 'not yet delivered' from 'never will be' — code that waits for lastStatus Success or branches on Failure/lastErrorCode hangs indefinitely.

Fix: the snapshot-delivery task of the Config service and the status surface that reports it — the delivery work behind services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/deliver_config_snapshot.rs and the delivery-channel state created by src/ops/put_delivery_channel.rs must actually perform the gzip-framed S3 PutObject under AWSLogs/<account>/Config/<region>/ with the snapshot id in the key and the recorded configurationItems as the payload, and src/ops/describe_delivery_channel_status.rs must project the resulting terminal lastStatus/lastAttemptTime/lastSuccessfulTime (or Failure plus lastErrorCode/lastErrorMessage).

Traces: cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item (check line 511, helper _poll_until at line 151).

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 70/113 checks passed, 29 mismatched / 14 errored). This check is marked neither ALREADY FILED nor OWNED.

[error] cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item
  File "check_cloudformation__config__s3_deploy_config_delivery_channel_by_cloudformation_delivers_a_snapshot_containing_the_recorded_item.py", line 511, in run
    info = _poll_until(
  File "...", line 151, in _poll_until
    raise TimeoutError(f"{description}: the run budget ended first; not graded")
  TimeoutError: snapshot delivery reaches SUCCESS: the run budget ended first; not graded

Expected (AWS, recorded): DeliverConfigSnapshot(deliveryChannelName=<chan>) returns configSnapshotId; DescribeDeliveryChannelStatus(DeliveryChannelNames=[<chan>]) then settles with DeliveryChannelsStatus[0].configSnapshotDeliveryInfo.lastStatus == SUCCESS and lastAttemptTime/lastSuccessfulTime set; ListObjectsV2 on the delivery bucket shows a key starting with AWSLogs/<account-id>/Config/<region>/ that contains that configSnapshotId; GetObject on it returns a body whose first two bytes are 0x1f 0x8b and which gunzips to JSON with a configurationItems array including the recorded bucket's item (resourceId == bucket name, awsRegion == request region).

Actual (emulator): the preceding legs now pass — the stack reaches CREATE_COMPLETE, PutConfigurationRecorder/PutDeliveryChannel/StartConfigurationRecorder succeed with recording=true, and the configuration-item poll for the observed bucket (which is what this check used to die on) is satisfied, so the run advances to line 511. There configSnapshotDeliveryInfo never reaches SUCCESS within the check's own 420 s budget, and no key containing the returned configSnapshotId ever appears in the delivery bucket. Nothing is refused; no Failure status with lastErrorCode/lastErrorMessage is reported either.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DeliverConfigSnapshot -> {configSnapshotId}, errors NoSuchDeliveryChannelException / NoAvailableConfigurationRecorderException / NoRunningConfigurationRecorderException (client, 400); DescribeDeliveryChannelStatus -> {DeliveryChannelsStatus}; ConfigExportDeliveryInfo{lastStatus: DeliveryStatus(Success|Failure|Not_Applicable), lastErrorCode, lastErrorMessage, lastAttemptTime, lastSuccessfulTime, nextDeliveryTime}.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay cloudformation__config__s3.deploy-config-delivery-channel-by-cloudformation-delivers-a-snapshot-containing-the-recorded-item.
3. Manually: cloudformation create-stack with a template declaring AWS::S3::Bucket, an AWS::S3::BucketPolicy granting config.amazonaws.com s3:GetBucketAcl / s3:ListBucket / s3:PutObject on that bucket (the PutObject statement conditioned on s3:x-amz-acl = bucket-owner-full-control), and an AWS::IAM::Role trusting config.amazonaws.com (Capabilities=CAPABILITY_IAM), with Outputs BucketName and RoleArn; wait for CREATE_COMPLETE.
4. config put-configuration-recorder --configuration-recorder name=<rec>,roleARN=<RoleArn>,recordingGroup={allSupported=true,includeGlobalResourceTypes=false}; config put-delivery-channel --delivery-channel name=<chan>,s3BucketName=<BucketName>; config start-configuration-recorder --configuration-recorder-name <rec>.
5. s3 create-bucket --bucket <observed>; poll config get-resource-config-history --resource-type AWS::S3::Bucket --resource-id <observed> until a configuration item appears (this now succeeds).
6. config deliver-config-snapshot --delivery-channel-name <chan> — note the returned configSnapshotId — then poll config describe-delivery-channel-status --delivery-channel-names <chan>: configSnapshotDeliveryInfo.lastStatus never becomes SUCCESS and lastAttemptTime/lastSuccessfulTime are never set.
7. s3 list-objects-v2 --bucket <BucketName>: no key under AWSLogs/<account-id>/Config/<region>/ contains the returned configSnapshotId, so there is no gzipped snapshot to gunzip and no configurationItems payload naming <observed>.
