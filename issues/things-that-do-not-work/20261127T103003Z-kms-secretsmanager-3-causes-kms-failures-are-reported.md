Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: kms__secretsmanager.create-secret-under-a-disabled-cmk-rejected-until-the-key-is-enabled kms__secretsmanager.create-secret-under-a-key-pending-deletion-rejected kms__secretsmanager.create-secret-under-an-asymmetric-key-rejected kms__secretsmanager.update-secret-to-a-disabled-cmk-rejected-and-the-old-key-kept kms__secretsmanager.aws-managed-secretsmanager-key-policy-restricts-use-to-the-service
Issue-Key: kms__secretsmanager.create-secret-under-a-disabled-cmk-rejected-until-the-key-is-enabled|run#14|AssertionError|assert got == ("InvalidParameterException", _DISABLED), (
Issue-Key: kms__secretsmanager.create-secret-under-a-key-pending-deletion-rejected|run#13|AssertionError|assert got == ("PreconditionNotMetException", f"{key_arn} is pending deletion."), (
Issue-Key: kms__secretsmanager.create-secret-under-an-asymmetric-key-rejected|run#8|AssertionError|assert got == ("InvalidRequestException", "You cannot generate a data key with an asymmetric CMK"), (
Issue-Key: kms__secretsmanager.update-secret-to-a-disabled-cmk-rejected-and-the-old-key-kept|_refusal#13|AssertionError|raise AssertionError(f"the call was expected to be refused, but it succeeded: {result!r}")
Issue-Key: kms__secretsmanager.aws-managed-secretsmanager-key-policy-restricts-use-to-the-service|run#10|AssertionError|assert via in cond and "kms:ViaService" in cond, (
Issue-Commit: 8133ebe260b7cb1c7e11e2995dae089129b7fabf

# kms__secretsmanager: 3 causes — KMS failures are reported as PreconditionNotMetException with a relayed KMS message, UpdateSecret ignores key state, and the aws/secretsmanager key policy is not ViaService-scoped

> _Note: this issue was identified against the code base at commit `8133ebe260b7cb1c7e11e2995dae089129b7fabf`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSecret

## Description
Three distinct root causes, grounded in five failing behavioral checks.

Cause 1 of 3: CreateSecret maps every KMS encryption failure onto PreconditionNotMetException and relays KMS's own sentence, instead of the per-class code and message AWS sends.
  AWS: Smithy: CreateSecret declares InvalidParameterException, InvalidRequestException and PreconditionNotMetException (all 400) as separate error classes. Recorded behavior, per class:
   - disabled key -> ('InvalidParameterException', 'The operation failed because the specified KMS key is disabled.') — fixed text, no variable parts;
   - key pending deletion -> ('PreconditionNotMetException', '<key arn> is pending deletion.') — the key ARN from the request plus ' is pending deletion.';
   - asymmetric key -> ('InvalidRequestException', 'You cannot generate a data key with an asymmetric CMK') — fixed text, no variable parts.
   In every case no secret is created (DescribeSecret answers the not-found class).
  Emulator: all three answer PreconditionNotMetException with an emulator-composed prefix plus a relayed KMS exception: "Secrets Manager can't encrypt the secret value: <key arn> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <id>; Proxy: null)", "Secrets Manager can't encrypt the secret value: <key arn> is pending deletion." and "Secrets Manager can't encrypt the secret value: <key arn> has KeySpec RSA_2048, which is not a symmetric encryption key spec and so cannot generate a data key."
  Why: a client branching on Secrets Manager's error code cannot distinguish a disabled key (InvalidParameterException) from an asymmetric key (InvalidRequestException) — both arrive as PreconditionNotMetException — and anything matching the message reads wording AWS never sends.
  Fix: the secretsmanager CreateSecret handler's KMS error mapping (classify the KMS failure into AWS's error class and emit AWS's message instead of relaying the KMS exception string).
  Traces: kms__secretsmanager.create-secret-under-a-disabled-cmk-rejected-until-the-key-is-enabled, kms__secretsmanager.create-secret-under-a-key-pending-deletion-rejected, kms__secretsmanager.create-secret-under-an-asymmetric-key-rejected.

Cause 2 of 3: UpdateSecret re-keys a secret to a disabled CMK.
  AWS: recorded behavior only. UpdateSecret(SecretId=..., KmsKeyId=<disabled key>) is refused ('InvalidParameterException', 'The operation failed because the specified KMS key is disabled.') and the secret keeps its previous key.
  Emulator: the call returns 200 ({'ARN': ..., 'Name': ...}) and the secret is re-keyed.
  Why: a secret is moved onto a key that cannot encrypt it; subsequent writes/reads fail instead of the re-key being refused, and the old key may then be retired while it is still the only usable one.
  Fix: the secretsmanager UpdateSecret handler's KmsKeyId validation (key state check in KMS before the re-key).
  Traces: kms__secretsmanager.update-secret-to-a-disabled-cmk-rejected-and-the-old-key-kept.

Cause 3 of 3: the AWS managed aws/secretsmanager key's default policy is the generic root policy, not the service-scoped one.
  AWS: recorded behavior only. GetKeyPolicy on the account's aws/secretsmanager key returns statements that allow the key's use only through Secrets Manager: every Decrypt-granting statement is conditioned on kms:ViaService 'secretsmanager.<region>.amazonaws.com' (and a CallerAccount condition), so the key cannot be used directly.
  Emulator: the policy is a single statement {'Sid': 'Enable IAM User Permissions', 'Principal': {'AWS': 'arn:aws:iam::<account>:root'}, 'Action': 'kms:*', 'Resource': '*'} with no ViaService condition.
  Why: a client reading the AWS managed key's policy (to prove a key can only be used through its service, or to model the same constraint) reads a policy AWS never publishes, and the emulator's managed key is in fact usable outside Secrets Manager.
  Fix: the KMS AWS-managed-key catalogue — seed aws/secretsmanager with the service-scoped default policy that GetKeyPolicy projects.
  Traces: kms__secretsmanager.aws-managed-secretsmanager-key-policy-restricts-use-to-the-service.

## Evidence
kms__secretsmanager.create-secret-under-a-disabled-cmk-rejected-until-the-key-is-enabled — AssertionError: CreateSecret under the disabled key answered ('PreconditionNotMetException', "Secrets Manager can't encrypt the secret value: <key arn> is disabled. (Service: AWSKMS; Status Code: 400; Error Code: DisabledException; Request ID: <id>; Proxy: null)"); expected ('InvalidParameterException', 'The operation failed because the specified KMS key is disabled.').
kms__secretsmanager.create-secret-under-a-key-pending-deletion-rejected — got the same emulator prefix; expected ('PreconditionNotMetException', '<key arn> is pending deletion.').
kms__secretsmanager.create-secret-under-an-asymmetric-key-rejected — got ('PreconditionNotMetException', "Secrets Manager can't encrypt the secret value: <key arn> has KeySpec RSA_2048, which is not a symmetric encryption key spec and so cannot generate a data key."); expected ('InvalidRequestException', 'You cannot generate a data key with an asymmetric CMK').

kms__secretsmanager.update-secret-to-a-disabled-cmk-rejected-and-the-old-key-kept — AssertionError: the call was expected to be refused, but it succeeded: {'ARN': 'arn:aws:secretsmanager:us-west-2:<account>:secret:sim2real-xsvc-rekey-<id>-y9Q3vg', 'Name': ...}; expected ('InvalidParameterException', 'The operation failed because the specified KMS key is disabled.').

kms__secretsmanager.aws-managed-secretsmanager-key-policy-restricts-use-to-the-service — AssertionError: a statement granting Decrypt is not limited to kms:ViaService secretsmanager.us-west-2.amazonaws.com: {'Sid': 'Enable IAM User Permissions', 'Effect': 'Allow', 'Principal': {'AWS': 'arn:aws:iam::<account>:root'}, 'Action': 'kms:*', 'Resource': '*'}.

## Steps to Reproduce
1. Cause 1: create a symmetric key and disable it -> CreateSecret with that KmsKeyId; create another and schedule its deletion -> CreateSecret; create an RSA_2048 key -> CreateSecret. Expect, respectively, InvalidParameterException/'...KMS key is disabled.', PreconditionNotMetException/'<key arn> is pending deletion.', InvalidRequestException/'You cannot generate a data key with an asymmetric CMK'; observe PreconditionNotMetException with relayed KMS text for all three.
2. Cause 2: create a secret under an enabled key, disable a second key, then secretsmanager.update_secret(SecretId=..., KmsKeyId=<disabled key>) -> expect the disabled-key InvalidParameterException and the old key retained; observe 200.
3. Cause 3: kms.describe_key(KeyId='alias/aws/secretsmanager') then kms.get_key_policy(..., PolicyName='default') -> expect every Decrypt-granting statement conditioned on kms:ViaService secretsmanager.<region>.amazonaws.com; observe the generic root 'kms:*' statement.
