Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed
Issue-Key: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed|_poll_until#18|Timeout|time.sleep(interval_s)
Issue-Commit: d6e9bd85505b2fcf81ff15dd47c3206271b87270

# cloudformation__config__s3: snapshot delivery never reaches a terminal lastStatus, so a delivery denied by the removed bucket policy is indistinguishable from a successful one (1 cause)

> _Note: this issue was identified against the code base at commit `d6e9bd85505b2fcf81ff15dd47c3206271b87270`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryChannelStatus

## Description
ONE root cause. ONE CAUSE must be closed.

Cause 1 of 1: DeliverConfigSnapshot is accepted and returns a configSnapshotId, but the delivery it names never reports a terminal outcome, so neither a denied nor a successful delivery is ever observable.
AWS: recorded behavior only, with model grounding. Recorded flow: with a running recorder and a delivery channel whose bucket policy grants config.amazonaws.com s3:GetBucketAcl/s3:ListBucket/s3:PutObject, DeliverConfigSnapshot returns a configSnapshotId and DescribeDeliveryChannelStatus's DeliveryChannelsStatus[0].configSnapshotDeliveryInfo settles on a terminal lastStatus — SUCCESS with lastSuccessfulTime set and an object written under the bucket whose key contains that configSnapshotId, or FAILURE with a non-empty lastErrorCode and no object written when the bucket policy has been deleted (DeleteBucketPolicy) out from under the established channel. Smithy (com-amazonaws-configservice-starlingdoveservice.json): DeliverConfigSnapshot models only client/400 errors (NoSuchDeliveryChannelException, NoAvailableConfigurationRecorderException, NoRunningConfigurationRecorderException), i.e. the delivery outcome is never raised to the caller — configSnapshotDeliveryInfo is the only place it is reported.
Emulator: the check is killed with 'Check timed out after 60.0s' while polling DescribeDeliveryChannelStatus for `configSnapshotDeliveryInfo.lastStatus in ('SUCCESS','FAILURE')`; the status never becomes terminal (the poll's own budget is 420s, so the emulator is not merely slow for this harness — nothing ever transitions). Consequently lastErrorCode is never reported and no snapshot object is written to the delivery bucket for either the denied or the restored-policy leg. The timeout report carries no stack frame; everything before the poll (CreateBucket + PutBucketPolicy, PutConfigurationRecorder, PutDeliveryChannel — which does enforce the delivery policy — StartConfigurationRecorder, DeleteBucketPolicy, DeliverConfigSnapshot returning an id) is reached.
Why: Config's delivery pipeline has no observable effect and no failure signal. A client that verifies its delivery wiring, or that alarms on a delivery denied by an S3 policy change, waits forever on a field that is never populated; an operator removing a bucket policy sees no FAILURE and no lastErrorCode and would conclude delivery still works, while nothing is in fact being written to S3.
Fix: the snapshot-delivery pipeline and its status surface in Config (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/deliver_config_snapshot.rs, the delivery task behind put_delivery_channel.rs's channel state, and src/ops/describe_delivery_channel_status.rs), including the S3 write attempt whose authorization outcome produces lastStatus/lastErrorCode.
Related open issues, named for overlap but NOT relied on as cover (neither carries this check id): 20261022T190340Z-config-has-no-recording-delivery-side-effects-and.md and 20261024T114729Z-config-never-converges-on-an-evaluation-or-a.md (its Cause 3 is the periodic configSnapshotDeliveryInfo/nextDeliveryTime surface). This check is the on-demand DeliverConfigSnapshot leg plus the authorization-denied outcome.
Traces: cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.

## Evidence
Replay of simparity-behavioral-ga-prod-v1 against the emulator built from mainline (emulator serving, bootstrap_signal false; 49/115 checks passed).

[mismatch] cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed
  Error: Check timed out after 60.0s

What the check does before it stalls, and what it asserts (from the replayed source):
  - creates a delivery bucket with the three AWSConfigBucketPermissionsCheck / AWSConfigBucketExistenceCheck / AWSConfigBucketDelivery statements, a recorder and a delivery channel, then starts the recorder (PutDeliveryChannel would have been refused with InsufficientDeliveryPolicyException without the policy, so the policy is provably in place);
  - s3.delete_bucket_policy(Bucket=bucket) and polls GetBucketPolicy until it reports NoSuchBucketPolicy;
  - cfg.deliver_config_snapshot(deliveryChannelName=channel) -> configSnapshotId (accepted, as AWS does);
  - _poll_until(describe_delivery_channel_status(...)['DeliveryChannelsStatus'][0]['configSnapshotDeliveryInfo'], lastStatus in ('SUCCESS','FAILURE'), timeout_s=420, interval_s=10) — this is where the check is killed;
  - the assertions it never reaches: lastStatus == 'FAILURE', a non-empty lastErrorCode, no bucket key containing the denied snapshot id, and then, after PutBucketPolicy restores only that policy, a second delivery reaching SUCCESS with lastSuccessfulTime > the failed attempt's lastAttemptTime and a readable object whose key contains the new snapshot id.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): DeliverConfigSnapshot -> DeliverConfigSnapshotResponse{configSnapshotId}, errors NoSuchDeliveryChannelException / NoAvailableConfigurationRecorderException / NoRunningConfigurationRecorderException (all errorKind client, httpError 400) — the delivery result is reported only through DescribeDeliveryChannelStatus.configSnapshotDeliveryInfo.

## Steps to Reproduce
1. git clone the repo, build and boot the emulator from mainline.
2. Replay simparity-behavioral-ga-prod-v1 and re-replay cloudformation__config__s3.config-delivery-denied-when-bucket-policy-is-removed.
3. Manually: s3 create-bucket + put-bucket-policy granting config.amazonaws.com GetBucketAcl/ListBucket/PutObject; config put-configuration-recorder with a role trusting config.amazonaws.com; config put-delivery-channel at that bucket; config start-configuration-recorder.
4. config deliver-config-snapshot --delivery-channel-name <channel>; note the returned configSnapshotId; then poll config describe-delivery-channel-status --delivery-channel-names <channel> — configSnapshotDeliveryInfo.lastStatus never becomes SUCCESS or FAILURE, and s3 list-objects-v2 on the bucket shows no object naming that snapshot id.
5. s3 delete-bucket-policy on the delivery bucket and repeat step 4: the delivery is still accepted and still reports no terminal lastStatus and no lastErrorCode, so the denied delivery is indistinguishable from a successful one.
