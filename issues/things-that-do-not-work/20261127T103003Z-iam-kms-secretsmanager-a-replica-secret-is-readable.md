Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iam__kms__secretsmanager.replica-read-denied-to-a-role-holding-decrypt-only-on-the-primary-region-key
Issue-Key: iam__kms__secretsmanager.replica-read-denied-to-a-role-holding-decrypt-only-on-the-primary-region-key|_error#28|AssertionError|raise AssertionError("the call succeeded where a refusal was expected")
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# iam__kms__secretsmanager: a replica secret is readable by a role holding kms:Decrypt only on the primary-Region key

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetSecretValue

## Description
One root cause, grounded in a failing behavioral check.

Cause 1 of 1: GetSecretValue against a replica secret does not evaluate the caller's kms:Decrypt on the replica Region's key.
  AWS: recorded behavior only. A replica secret's value is encrypted under the replica Region's key, so a role granted kms:Decrypt only on the primary Region's key is refused when it reads the replica: ('AccessDeniedException', 'Access to KMS is not allowed') — a fixed sentence with no variable parts.
  Emulator: the read succeeds, so no refusal is produced at all ('the call succeeded where a refusal was expected').
  Why: a client reasoning about per-Region key access — the whole point of replica-specific keys — can read replica secret material it is not permitted to decrypt; a least-privilege boundary silently does not hold.
  Fix: the secretsmanager GetSecretValue handler's decryption path for replica secrets — resolve the replica Region's key and authorize the caller's kms:Decrypt on it before returning the value.
  Traces: iam__kms__secretsmanager.replica-read-denied-to-a-role-holding-decrypt-only-on-the-primary-region-key.

## Evidence
iam__kms__secretsmanager.replica-read-denied-to-a-role-holding-decrypt-only-on-the-primary-region-key —
File "check_...py", line 222, in run -> AssertionError: the call succeeded where a refusal was expected.
Expected ('AccessDeniedException', 'Access to KMS is not allowed') for the replica read.

## Steps to Reproduce
1. Create a symmetric CMK in the primary Region and one in the replica Region.
2. CreateSecret with KmsKeyId=<primary key> and replicate it to the second Region with that Region's key.
3. Create a role granted secretsmanager:GetSecretValue on both secrets and kms:Decrypt on the PRIMARY key only.
4. As that role, GetSecretValue against the replica secret in the replica Region.
5. Observe a successful read; AWS refuses with AccessDeniedException 'Access to KMS is not allowed'.
