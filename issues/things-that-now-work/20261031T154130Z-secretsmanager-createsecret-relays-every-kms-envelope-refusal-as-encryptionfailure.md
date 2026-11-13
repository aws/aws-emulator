Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.create-secret-invalid-kms-key-rejected secretsmanager.create-secret-under-a-cmk-pending-deletion-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.create-secret-invalid-kms-key-rejected|run#7|AssertionError|assert code == "InvalidParameterException", f"expected InvalidParameterException, got {code}"
Issue-Key: secretsmanager.create-secret-under-a-cmk-pending-deletion-rejected|run#7|AssertionError|assert code == "PreconditionNotMetException", (
Issue-Commit: c0f2084881d9020926616aaa2d19581de4851935

# CreateSecret relays every KMS envelope refusal as EncryptionFailure instead of the class AWS picks for the key's failure kind (2 causes)

> _Note: this issue was identified against the code base at commit `c0f2084881d9020926616aaa2d19581de4851935`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
TWO net-new root causes in the secretsmanager crate, both reached through plain single-service CreateSecret calls, both grounded in a failing behavioral check marked neither "ALREADY FILED" nor "OWNED by". A fixer must close BOTH before submitting. The common shape: CreateSecret now does call KMS to seal the initial value (progress), but it collapses EVERY KMS refusal into the single modeled error EncryptionFailure, whereas AWS picks the error class from WHY the key could not be used. Two distinct key-failure kinds are therefore reported under the wrong code.

Related but NOT covering (named for context): open issue 20261029T205254Z-secretsmanager-secrets-manager-kmskeyid-is-never-validated-against-kms.md lists secretsmanager.create-secret-invalid-kms-key-rejected among its trace ids, but its recorded replay key is the over-permissive accept ("expected rejection, but the call SUCCEEDED and returned ..."): the 200 accept is gone and the check no longer fails that way, so that issue does not carry this failure's key and the suite did not mark it OWNED. What survives — and what is filed here — is the error-CLASS divergence on the refusal path, plus the pending-deletion cause that issue never names.

=== Cause 1 of 2: a well-formed but nonexistent KmsKeyId is refused EncryptionFailure instead of InvalidParameterException ===
AWS: Smithy: CreateSecret.KmsKeyId targets KmsKeyIdType (@length 0-2048); CreateSecret models InvalidParameterException (client error, HTTP 400, "The parameter name or value is invalid.") AND EncryptionFailure (client error, HTTP 400, "Secrets Manager can't encrypt the protected secret text using the provided KMS key. Check that the KMS key is available, enabled, and not in an invalid state."). Secrets Manager resolves the named key before it encrypts, so a key ARN that resolves to NO key is a bad PARAMETER, not an encryption failure: the call is refused 400 InvalidParameterException with the message template "The operation failed because of an invalid KMS key: Key '<the KmsKeyId exactly as supplied in the request>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <the downstream KMS call's per-request request id>; Proxy: null)". Everything outside the two angle-bracketed slots is fixed text; the key ARN is a request value and the request id is per-run (the check matches it as [0-9a-f-]+). No secret is created: a following DescribeSecret on that name answers ResourceNotFoundException "Secrets Manager can't find the specified secret."
Emulator: the refusal happens and nothing is stored (good), but the code is EncryptionFailure and the message is the envelope's own relay wording, "Secrets Manager can't encrypt the secret value: Key '<arn>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)" — the fixed prefix "Secrets Manager can't encrypt the secret value:" replaces AWS's fixed prefix "The operation failed because of an invalid KMS key:".
Why: a client that catches InvalidParameterException to report a mistyped, cross-account or deleted CMK (the normal validation path) does not catch EncryptionFailure, which AWS reserves for a key that EXISTS but cannot encrypt right now. The two demand different operator action — fix the request vs. fix the key — and the emulator makes the first indistinguishable from the second, including in the message prefix clients and log scrapers key off.
Fix: the KMS-refusal classification of the CreateSecret handler's seal path — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/create_secret.rs (the `.map_err(encryption_failure)` on `crate::state::envelope::seal` and the `encryption_failure` helper) together with the refusal classification in services/service-com-amazonaws-secretsmanager-secretsmanager/src/state/envelope.rs (`call_failure` / `KmsRefusal`, which today carries only a relay string and no KMS error code for the handler to branch on). A KMS NotFoundException for the named key must become InvalidParameterException with AWS's invalid-KMS-key prefix.
Traces: secretsmanager.create-secret-invalid-kms-key-rejected.

=== Cause 2 of 2: a CMK that is PENDING DELETION is refused EncryptionFailure instead of PreconditionNotMetException ===
AWS: Smithy: CreateSecret models PreconditionNotMetException (client error, HTTP 400, "The request failed because you did not complete all the prerequisite steps."). A CMK whose deletion has been scheduled is a prerequisite the caller has not met (the key must be restored and enabled first), so CreateSecret under such a key is refused 400 PreconditionNotMetException, and the refused create leaves no secret (a following DescribeSecret on the name answers ResourceNotFoundException "Secrets Manager can't find the specified secret."). Once CancelKeyDeletion + EnableKey have run, the identical CreateSecret succeeds and the value reads back. Recorded behavior only for the message: the check pins the CODE and documents that the message embeds the downstream KMS call's per-request request id, so the message is not the requirement here — the code is.
Emulator: 400 EncryptionFailure with message "Secrets Manager can't encrypt the secret value: <key arn> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <id>; Proxy: null)". The key state is correctly consulted; only the class is wrong.
Why: PreconditionNotMetException tells a client the request can be retried unchanged once a prerequisite is restored, which is exactly the key-pending-deletion recovery story; EncryptionFailure tells it the key is unusable for crypto. Retry/remediation logic that branches on the two codes takes the wrong branch, and provisioning code that treats EncryptionFailure as fatal abandons a secret AWS would have created after CancelKeyDeletion.
Fix: the same classification seam as Cause 1 — services/service-com-amazonaws-secretsmanager-secretsmanager/src/ops/create_secret.rs's `encryption_failure` mapping, fed by a KmsRefusal that preserves the KMS error code from services/service-com-amazonaws-secretsmanager-secretsmanager/src/state/envelope.rs (`call_failure`). A KMS KMSInvalidStateException for a key pending deletion must become PreconditionNotMetException.
Traces: secretsmanager.create-secret-under-a-cmk-pending-deletion-rejected.

## Evidence
Suite: secretsmanager 93/190 (49%). Both checks are in the failing list and carry neither the ALREADY FILED nor the OWNED by marker.

Cause 1 — [mismatch] secretsmanager.create-secret-invalid-kms-key-rejected
  AssertionError: expected InvalidParameterException, got EncryptionFailure
  Call: CreateSecret(Name=<new name>, SecretString='x', KmsKeyId='arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000').
  Expected: 400 InvalidParameterException, Message matching the fixed template "The operation failed because of an invalid KMS key: Key '<that ARN>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <hex-and-hyphens>; Proxy: null)" (the check builds this as a regex with only the request id variable).
  Actual: 400 EncryptionFailure with the envelope relay message beginning "Secrets Manager can't encrypt the secret value: ".
  The remaining legs of the check (DescribeSecret -> ResourceNotFoundException "Secrets Manager can't find the specified secret.", and a plain CreateSecret without KmsKeyId succeeding) were never reached.

Cause 2 — [mismatch] secretsmanager.create-secret-under-a-cmk-pending-deletion-rejected
  AssertionError: CreateSecret under a pending-deletion key: got EncryptionFailure: "Secrets Manager can't encrypt the secret value: arn:aws:kms:us-west-2:<account>:key/<key id> is pending deletion. (Service: AWSKMS; Status Code: 400; Error Code: KMSInvalidStateException; Request ID: <id>; Proxy: null)"
  Call sequence: kms CreateKey(SYMMETRIC_DEFAULT/ENCRYPT_DECRYPT) -> kms ScheduleKeyDeletion(PendingWindowInDays=7) -> CreateSecret(Name=<new>, KmsKeyId=<that key>, SecretString='v1', Tags=[...]).
  Expected: 400 PreconditionNotMetException (message per-run, not asserted).
  Actual: 400 EncryptionFailure as quoted. The key id, account and request id above are per-run values filling the template, not part of the requirement.

## Steps to Reproduce
Replay the behavioral suite and re-run exactly these checks:

1. secretsmanager.create-secret-invalid-kms-key-rejected
   a. sts GetCallerIdentity -> <account>.
   b. CreateSecret(Name=<fresh name>, SecretString='x', KmsKeyId='arn:aws:kms:<region>:<account>:key/00000000-0000-0000-0000-000000000000').
   c. Observe Error.Code == 'EncryptionFailure'; expect 'InvalidParameterException' with Message "The operation failed because of an invalid KMS key: Key '<that ARN>' does not exist (Service: AWSKMS; Status Code: 400; Error Code: NotFoundException; Request ID: <id>; Proxy: null)".
   d. DescribeSecret(SecretId=<that name>) must answer ResourceNotFoundException "Secrets Manager can't find the specified secret." (no secret left behind).
   e. CreateSecret(Name=<other fresh name>, SecretString='y', Tags=[...]) must still succeed, and DescribeSecret on its ARN must report that Name.

2. secretsmanager.create-secret-under-a-cmk-pending-deletion-rejected
   a. kms CreateKey(KeyUsage='ENCRYPT_DECRYPT', KeySpec='SYMMETRIC_DEFAULT') -> <key arn>.
   b. kms ScheduleKeyDeletion(KeyId=<key arn>, PendingWindowInDays=7).
   c. CreateSecret(Name=<fresh name>, KmsKeyId=<key arn>, SecretString='v1', Tags=[...]) -> observe EncryptionFailure; expect PreconditionNotMetException (HTTP 400).
   d. DescribeSecret(SecretId=<that name>) must answer ResourceNotFoundException "Secrets Manager can't find the specified secret."
   e. kms CancelKeyDeletion(KeyId=<key arn>); kms EnableKey(KeyId=<key arn>); the identical CreateSecret must now succeed and GetSecretValue(SecretId=<arn>).SecretString must be 'v1'.
