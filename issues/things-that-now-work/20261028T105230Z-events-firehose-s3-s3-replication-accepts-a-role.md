Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: events__firehose__s3.s3-role-without-trust-policy-rejected
Issue-Key: events__firehose__s3.s3-role-without-trust-policy-rejected|run#12|AssertionError|assert status == "FAILED", (
Issue-Commit: a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d

# events__firehose__s3: S3 replication accepts a role that does not trust s3.amazonaws.com and then reports ReplicationStatus COMPLETED instead of failing the replication

> _Note: this issue was identified against the code base at commit `a991e58b63dd0e61a72e74cc1f818b9fb1c8de8d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketReplication

## Description
1 root cause.

Cause 1 of 1: the replication role's trust policy is never evaluated, so a role S3 cannot assume still replicates successfully.

AWS: PutBucketReplication is a configuration call and accepts a `ReplicationConfiguration.Role` whose trust policy does NOT allow `s3.amazonaws.com` to assume it — GetBucketReplication reads the same Role back, so acceptance at configuration time is correct. Replication itself, however, requires S3 to assume that role: because the sts:AssumeRole is refused, the object's replication reaches the TERMINAL state `FAILED` (reported by HeadObject/GetObject as `x-amz-replication-status`, Smithy: `ReplicationStatus`) and the object NEVER arrives in the destination bucket. An identically-permissioned role whose trust policy does allow `s3.amazonaws.com` replicates the same object to `COMPLETED`, so FAILED is not a blanket outcome. Recorded behavior only: `ReplicationStatus` is a modeled response member but the role-assumption outcome that drives it is not expressed in the Smithy model.

Emulator: the mistrusted-role configuration is accepted and read back correctly (that half matches), but the replication of the object under it reports `ReplicationStatus` = `COMPLETED`. The emulator never attempts (or never checks) the role assumption, so a role that S3 could not possibly assume produces the success status.

Why: the role trust policy is the single most common replication misconfiguration, and `ReplicationStatus` is the only per-object signal AWS gives a client. A client relying on AWS's behaviour treats COMPLETED as "the replica exists" and stops checking — against the emulator it gets COMPLETED for a replication that AWS would have failed, so a cross-region/DR copy that does not exist is reported as present and a trust-policy regression is invisible to monitoring and to tests. The two legs of the check also become indistinguishable: the mistrusted role and the correct role both report COMPLETED, so the control proves nothing.

Fix: the s3 crate's replication engine must assume the configured replication role before replicating (evaluating the role's trust policy for the `s3.amazonaws.com` service principal) and, when the assumption is refused, leave the destination untouched and drive the source object's ReplicationStatus to the terminal `FAILED`; only an assumable, adequately-permissioned role may reach `COMPLETED`. PutBucketReplication itself must keep accepting the configuration. A related open issue, 20261025T002357Z-events-firehose-s3-firehose-accepts-an-s3-destination-role-whose-trust.md, covers the analogous gap for a FIREHOSE destination role and does not carry this check or the S3 replication path.

Traces: events__firehose__s3.s3-role-without-trust-policy-rejected.

## Evidence
Expected: after PutBucketReplication with a role trusting only ec2.amazonaws.com, the source object's replication reaches the terminal status FAILED and the object never appears in the destination bucket; the healthy twin role (trusting s3.amazonaws.com) reaches COMPLETED.

Actual: AssertionError at check_events__firehose__s3_s3_role_without_trust_policy_rejected.py line 42 — "replication with a role that does not trust s3.amazonaws.com should terminally FAIL (expected status=FAILED, head_object ReplicationStatus reported 'COMPLETED')".

The configuration half matched: GetBucketReplication returned the same mistrusted Role that PutBucketReplication accepted, so the divergence is purely in the replication data plane's role-assumption check and the resulting ReplicationStatus.

## Steps to Reproduce
Replay the behavioral suite and re-run check events__firehose__s3.s3-role-without-trust-policy-rejected.

Manually:
1. Create two versioned buckets (source and destination).
2. Create an IAM role whose trust policy names only ec2.amazonaws.com as the assuming principal, and attach the usual replication permissions (s3:GetObjectVersionForReplication/Acl/Tagging on the source, s3:ReplicateObject/ReplicateDelete/ReplicateTags on the destination).
3. Call s3.put_bucket_replication(Bucket=source, ReplicationConfiguration={"Role": <that role ARN>, "Rules": [...]}) — it is accepted, and get_bucket_replication reads the same Role back.
4. s3.put_object into the source bucket, then poll s3.head_object on the source object and read ReplicationStatus.
5. Observe it report COMPLETED; AWS reports FAILED and the destination bucket stays empty.
6. Repeat with a role whose trust policy allows s3.amazonaws.com to confirm both legs report the same (COMPLETED) status in the emulator.
