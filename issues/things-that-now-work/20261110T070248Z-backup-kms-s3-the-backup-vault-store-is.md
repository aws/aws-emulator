Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only
Issue-Key: backup|CreateBackupVault|AlreadyExistsException
Issue-Commit: 4007f6f2df62523caa3f6729b6a64eeef2a70f5b

# backup__kms__s3: the backup-vault store is keyed by vault name alone, so CreateBackupVault refuses AlreadyExistsException for the same name in a second region

> _Note: this issue was identified against the code base at commit `4007f6f2df62523caa3f6729b6a64eeef2a70f5b`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateBackupVault

## Description
One net-new root cause for the backup__kms__s3 scenario service (1 of 1).

Cause 1 of 1: backup-vault name uniqueness is enforced account-wide instead of per account+region, so a CreateBackupVault that reuses a name in a DIFFERENT region is refused `AlreadyExistsException` instead of creating a second, independent vault. Failing check: `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only`.

AWS: Smithy (com.amazonaws.backup#CryoControllerUserManager) `CreateBackupVault` input member `BackupVaultName` is documented as: "Backup vaults are identified by names that are unique to the account used to create them and the Amazon Web Services Region where they are created" — the same statement is repeated on the output member. Name uniqueness is therefore scoped to account + region: re-creating the name in the region that already holds it is refused with the modeled `AlreadyExistsException` (HTTP 400, message `Backup vault with the same name already exists` — fixed text, no variable part), while the SAME name sent to another region succeeds with HTTP 200 and returns a vault whose ARN carries that other region (`arn:aws:backup:<region>:<account>:backup-vault:<name>`, where region/account/name come from the request and the caller's identity), with its own CreationDate and a fresh, empty recovery-point set.

Emulator: the second-region create is refused. The call raises `botocore.errorfactory.AlreadyExistsException: An error occurred (AlreadyExistsException) when calling the CreateBackupVault operation: Backup vault with the same name already exists` — the status, code and message AWS uses for a SAME-region duplicate, served for a request in a region that holds no such vault. The cause is in state keying, not in the refusal wording: the handler looks the collision up as `store.vaults.get(&input.backup_vault_name)` on a single map keyed by vault name only, even though it has already resolved the request's region (`crate::state::request_region(ctx)`) and uses it to mint the ARN and the default key. Every other read/write path over `store.vaults` (describe, delete, recovery-point map `recovery_points_by_vault`, `MAX_VAULTS` quota) is keyed the same way, so two regions of one account share one vault namespace and one quota.

Why: a client that deploys the same backup-vault name into several regions — the normal pattern for a cross-region backup/copy reference architecture, and what CFN/CDK multi-region stacks do — has its second and later regions refused with a 400 that says the vault already exists, so the deployment cannot proceed and cross-region copy destinations can never be created. Conversely a caller that relies on per-region isolation can have one region's vault satisfy a lookup issued against another region, and one region's vaults consume the other's quota.

Fix: the vault store and its lookups — `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/state.rs` (`vaults`, `recovery_points_by_vault`) and the collision/idempotency branch in `services/service-com-amazonaws-backup-cryocontrollerusermanager/src/ops/create_backup_vault.rs` — must key vault records by (region, name) and resolve every vault lookup, delete and quota count within the request's region, so `AlreadyExistsException` is raised only when the SAME region already holds the name.

Related already-filed issue: `20261102T115153Z-backup-kms-s3-listtags-answers-501-notimplemented-so.md` carries this same check's replay key for a DIFFERENT cause (ListTags answering 501). That earlier blocker no longer stops the check — it now reaches the positive control at the end — and that issue says nothing about region scoping, so it does not cover this cause.

Traces: `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only` (verdict: error).

## Evidence
Check `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only` (verdict: error). The check creates a uniquely-named vault in ctx.region, asserts the same-region duplicate is refused (that assertion is reached and passes), re-reads the surviving vault and its tags, then runs its positive control: the same name created through a client bound to a second region (`us-east-1` unless that is already ctx.region, else `us-west-2`).

Expected (AWS): `far.create_backup_vault(BackupVaultName=<name>, BackupVaultTags={...})` answers HTTP 200 with `BackupVaultArn == arn:aws:backup:<far_region>:<account>:backup-vault:<name>`, an ARN different from the first region's vault, and `describe_backup_vault(BackupVaultName=<name>)` in that region reports `NumberOfRecoveryPoints == 0`.

Actual (emulator): the create is refused and the exception escapes the check:
```
botocore.errorfactory.AlreadyExistsException: An error occurred (AlreadyExistsException) when calling the CreateBackupVault operation: Backup vault with the same name already exists
```
Message breakdown: `Backup vault with the same name already exists` is entirely fixed text (AWS's duplicate-name sentence) — it has no variable part, and the divergence is that it is served at all for a region that holds no vault of that name, not that its wording differs. HTTP 400 / code `AlreadyExistsException`.

Code: `create_backup_vault.rs` resolves `let region = crate::state::request_region(ctx);` and uses it only for `backup_vault_arn(&region, ..)` and `default_encryption_key_arn(&region, ..)`; the existence test is `store.vaults.get(&input.backup_vault_name)` and the insert is `store.vaults.insert(vault.name.clone(), vault)` — region never participates in the key.

## Steps to Reproduce
1. Boot the emulator and create two botocore `backup` clients for one account, one bound to region A (e.g. us-west-2) and one to region B (e.g. us-east-1).
2. Region A: `create_backup_vault(BackupVaultName="sim2real-bkp-vault-<suffix>", BackupVaultTags={"sim2real": "refarch-bkp"})` — succeeds.
3. Region A again, same name: `create_backup_vault(...)` with no CreatorRequestId — correctly refused 400 `AlreadyExistsException` / "Backup vault with the same name already exists".
4. Region B, SAME name: `create_backup_vault(BackupVaultName="<the same name>", BackupVaultTags={"sim2real": "refarch-bkp"})` — observe the same 400 `AlreadyExistsException` instead of a 200 whose `BackupVaultArn` is `arn:aws:backup:<region B>:<account>:backup-vault:<name>`.
5. Equivalently, replay `backup__kms__s3.refarch-bkp-duplicate-vault-name-rejected-in-region-only`.
