Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected
Issue-Shape-Id: com.amazonaws.redshift#RedshiftServiceVersion20121201
Issue-Key: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected|run#16|AssertionError|assert created["KmsKeyId"] == real_key_arn, (
Issue-Commit: 8bae25eab1a2870033e48170871633376c17feec

# redshift: CreateSnapshotCopyGrant stores and echoes only the bare KMS key id when the request supplies a full key ARN

> _Note: this issue was identified against the code base at commit `8bae25eab1a2870033e48170871633376c17feec`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSnapshotCopyGrant

## Description
Replay of simparity-behavioral-aws-v1 against the emulated Redshift control plane. ONE net-new cause; it must be closed before submitting.

=== Cause 1 of 1: an explicit KmsKeyId given as a full ARN is not echoed verbatim — the response (and DescribeSnapshotCopyGrants) carries only the key UUID ===
AWS: Smithy (com.amazonaws.redshift, redshiftserviceversion20121201): CreateSnapshotCopyGrantMessage.KmsKeyId is a plain String with no pattern or length narrowing, and SnapshotCopyGrant.KmsKeyId is documented as 'The unique identifier of the encrypted symmetric key in Amazon Web Services KMS to which Amazon Redshift is granted permission.' The model fixes no normalisation, so this is recorded behavior: when CreateSnapshotCopyGrant is called with KmsKeyId set to a key's full ARN ('arn:aws:kms:<region>:<account>:key/<key-uuid>'), the returned SnapshotCopyGrant.KmsKeyId is that same ARN, character for character, and DescribeSnapshotCopyGrants(SnapshotCopyGrantName=<name>) serves the same ARN for the grant. The identifier the caller supplied is persisted as supplied; Redshift does not rewrite an ARN into a bare id.
Emulator: the create succeeds but SnapshotCopyGrant.KmsKeyId comes back as the bare key UUID — the 'arn:aws:kms:<region>:<account>:key/' prefix is stripped — and the same truncated value is what DescribeSnapshotCopyGrants serves. In the replay, KmsKeyId='arn:aws:kms:us-west-2:000000000000:key/fa766f34-b6fe-4278-9f29-cace49d940b9' was answered with 'fa766f34-b6fe-4278-9f29-cace49d940b9' (the region/account/uuid here are per-run values; the requirement is that whatever identifier form the request used is returned unchanged).
Why: a client that creates a grant for a key ARN and then compares the grant it reads back against the ARN it owns — the normal way to confirm which key a cross-region snapshot copy is authorised for, and the way CloudFormation and IaC drift checks compare a resource's KmsKeyId property — sees a value that never equals what it sent. A grant for a key in another account or region cannot even be identified unambiguously from a bare uuid, because the account and region are exactly the parts that were dropped.
Fix: the CreateSnapshotCopyGrant handler and the snapshot-copy-grant state it writes — persist KmsKeyId exactly as the request supplied it and serialise that stored value in both CreateSnapshotCopyGrant's and DescribeSnapshotCopyGrants' SnapshotCopyGrant.KmsKeyId, instead of reducing it to the key id.
Note: the rest of this check passes — the nonexistent-key refusal (InvalidParameterValue with "Unable to create snapshot copy grant. The KMS key '<kms-key-id-from-the-request>' does not exist or is inaccessible.", then SnapshotCopyGrantNotFoundFault with "Snapshot copy grant '<name-from-the-request>' not found.") is already correct, so only the echo of a valid key is at fault. No open issue names this face.
Traces: redshift.create-snapshot-copy-grant-invalid-kms-key-rejected.

## Evidence
Suite: simparity-behavioral-aws-v1 — redshift 155/197 (79%), 42 failed (41 of them OWNED by existing issues/already filed and skipped).

Cause 1 — check_redshift_create_snapshot_copy_grant_invalid_kms_key_rejected.py:97
  request:  create_snapshot_copy_grant(SnapshotCopyGrantName='<unique-name>', KmsKeyId='<the ARN kms.create_key returned, i.e. arn:aws:kms:<region>:<account>:key/<uuid>>')
  expected: response['SnapshotCopyGrant']['KmsKeyId'] == that same ARN, and describe_snapshot_copy_grants(SnapshotCopyGrantName='<name>') -> [(<name>, that same ARN)]
  actual:   AssertionError: expected the explicit KmsKeyId 'arn:aws:kms:us-west-2:000000000000:key/fa766f34-b6fe-4278-9f29-cace49d940b9' to be used verbatim (as the full ARN), got 'fa766f34-b6fe-4278-9f29-cace49d940b9'

(the region, account and key uuid above are per-run values; the requirement is the identity transform, not those strings.)

Model corroboration (services/service-com-amazonaws-redshift-redshiftserviceversion20121201/smithy-model/com-amazonaws-redshift-redshiftserviceversion20121201.json): CreateSnapshotCopyGrantMessage = { SnapshotCopyGrantName (required String), KmsKeyId (String, no pattern), Tags }; SnapshotCopyGrant.KmsKeyId documented as the unique identifier of the KMS key the grant covers; errors InvalidParameterValue-class refusals modelled alongside SnapshotCopyGrantAlreadyExistsFault / SnapshotCopyGrantQuotaExceededFault.

## Steps to Reproduce
1. Clone the repo and boot the emulator; point botocore 'redshift' and 'kms' clients at it.
2. kms.create_key(...) and take KeyMetadata.Arn (the full 'arn:aws:kms:<region>:<account>:key/<uuid>').
3. redshift.create_snapshot_copy_grant(SnapshotCopyGrantName='scg-demo', KmsKeyId=<that ARN>).
4. Expect response['SnapshotCopyGrant']['KmsKeyId'] == the ARN passed in; observe the bare key uuid instead.
5. redshift.describe_snapshot_copy_grants(SnapshotCopyGrantName='scg-demo') — expect the same ARN; observe the bare uuid.
6. Re-replay redshift.create-snapshot-copy-grant-invalid-kms-key-rejected to confirm the fix.
