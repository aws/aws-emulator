Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config.delivery-channel-bucket-policy-statement-required
Issue-Shape-Id: com.amazonaws.configservice#StarlingDoveService
Issue-Key: config.delivery-channel-bucket-policy-statement-required|_poll_until#25|Timeout|time.sleep(interval_s)
Issue-Commit: f420f96d635d512f65dbfe0e5d9b800a5c85b936

# config: PutDeliveryChannel never reads the delivery bucket's policy, so InsufficientDeliveryPolicyException is unreachable and a bucket that denies config.amazonaws.com s3:GetBucketAcl is accepted

> _Note: this issue was identified against the code base at commit `f420f96d635d512f65dbfe0e5d9b800a5c85b936`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutDeliveryChannel

## Description
One cause. Cause 1 of 1: PutDeliveryChannel admits a delivery channel whose target bucket policy does not grant config.amazonaws.com the s3:GetBucketAcl action, instead of refusing it, so the modeled InsufficientDeliveryPolicyException is unreachable.

AWS: PutDeliveryChannel reads the target bucket's policy before accepting the channel and refuses a bucket Config cannot read the ACL of. Smithy: PutDeliveryChannel models InsufficientDeliveryPolicyException (errorKind client, httpError 400 defaulted), documented as "Your Amazon S3 bucket policy does not allow Config to write to it." The recorded AWS refusal is HTTP 400, error code InsufficientDeliveryPolicyException, with the message template "Insufficient delivery policy to s3 bucket: <s3BucketName>, unable to get s3 bucket acl." — the fixed text is "Insufficient delivery policy to s3 bucket: " and ", unable to get s3 bucket acl."; the single variable part is the s3BucketName taken verbatim from the request's DeliveryChannel. The refusal must also have no effect: DescribeDeliveryChannels stays empty and DescribeDeliveryChannels(DeliveryChannelNames=[<name>]) answers NoSuchDeliveryChannelException with the message template "Cannot find delivery channel with specified name '<name>'." (fixed text plus the requested name). The identical request must be accepted again once the s3:GetBucketAcl statement for the config.amazonaws.com service principal is restored, so the refusal is attributable to that one statement and nothing else.

Emulator: the request is never refused. The check first proves PutDeliveryChannel is accepted against a fully policied bucket, deletes the channel, then re-issues the byte-identical request against the same bucket with only the s3:GetBucketAcl statement removed from its policy, polling for a refusal; the emulator keeps answering success for every attempt (the poll's `_put_once` helper deletes each accepted channel and returns None), so the check never observes a rejection and is killed by the 60 s budget — reported by the suite as "Error: Check timed out after 60.0s". Hypothesis: PutDeliveryChannel's handler performs no S3 bucket-policy evaluation at all, so there is no code path that can emit this error class.

Why: a client that relies on AWS validating the delivery target gets a channel that reports as configured but that Config could never deliver snapshots or configuration history into; the misconfiguration surfaces late or never. Any client error handling that branches on InsufficientDeliveryPolicyException is dead code against the emulator, and retry loops that wait out bucket-policy propagation by retrying past this code (a documented, common pattern, used by the suite's own helpers) behave differently here.

Fix: PutDeliveryChannel's handler validation — before persisting the channel, resolve the bucket's policy and require an Allow of s3:GetBucketAcl for the config.amazonaws.com service principal on the bucket ARN, raising 400 InsufficientDeliveryPolicyException with the message above and persisting nothing (services/service-com-amazonaws-configservice-starlingdoveservice/src/ops/put_delivery_channel.rs). Related open issue 20261022T190340Z-config-putconfigrule-and-putdeliverychannel-are-overly-permissive-inconsistent.md covers PutDeliveryChannel's other admission gaps (nonexistent bucket, malformed snsTopicARN/s3KmsKeyArn/name) but not the bucket-policy pre-check, which is a distinct validation and a distinct error class.

Traces: config.delivery-channel-bucket-policy-statement-required.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout (emulator serving; bootstrap_signal false; 165/182 checks passed).

Failing check config.delivery-channel-bucket-policy-statement-required — verdict mismatch, detail: "Error: Check timed out after 60.0s".

Check shape (from the replayed source): 
1. control leg — bucket created, policy granting config.amazonaws.com s3:GetBucketAcl + s3:ListBucket + s3:PutObject, PutDeliveryChannel accepted, DescribeDeliveryChannels lists exactly that channel; then DeleteDeliveryChannel so the negative leg is not testing MaxNumberOfDeliveryChannelsExceededException.
2. negative leg — the same bucket's policy is re-put WITHOUT the s3:GetBucketAcl statement (everything else byte-identical) and PutDeliveryChannel is retried on a 120 s/5 s poll until it is refused. Expected: code == "InsufficientDeliveryPolicyException" and message == "Insufficient delivery policy to s3 bucket: {bucket}, unable to get s3 bucket acl.". Observed: every attempt succeeds, the poll never satisfies its predicate, and the check is killed at the harness's 60 s budget without reaching a verdict.
3. the check then asserts no channel was created and that the name does not resolve (NoSuchDeliveryChannelException, "Cannot find delivery channel with specified name '<name>'."), and finally re-adds the single statement and requires the identical call to be accepted again.

Model grounding (services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json): PutDeliveryChannel errors include InsufficientDeliveryPolicyException {errorKind: client, httpError: 400 (defaulted)}; its doc: "Your Amazon S3 bucket policy does not allow Config to write to it."

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and re-replay the check id in trace_ids.
3. Manually: create an S3 bucket; PutBucketPolicy granting the config.amazonaws.com service principal s3:GetBucketAcl and s3:ListBucket on arn:aws:s3:::<bucket> and s3:PutObject on arn:aws:s3:::<bucket>/AWSLogs/<account>/Config/* (with the bucket-owner-full-control / SourceAccount conditions).
4. PutConfigurationRecorder so a recorder exists (it need not be started), then PutDeliveryChannel{name, s3BucketName=<bucket>} — accepted (control). DeleteDeliveryChannel.
5. PutBucketPolicy again on the same bucket with ONLY the s3:GetBucketAcl statement removed.
6. Re-issue the identical PutDeliveryChannel — the emulator accepts it (HTTP 200) and DescribeDeliveryChannels lists the channel. Expected: HTTP 400 InsufficientDeliveryPolicyException, message "Insufficient delivery policy to s3 bucket: <bucket>, unable to get s3 bucket acl.", and no channel created.
