// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-backup-cryocontrollerusermanager service.
//!
//! The vault / backup-job slice of AWS Backup is modelled here as the records
//! below plus one index, so every operation computes its answer from the SAME
//! stored resource:
//!
//! * [`BackupVault`] — what `CreateBackupVault` inserts and `DeleteBackupVault`
//!   removes, carrying the [`VaultLock`] `PutBackupVaultLockConfiguration`
//!   applies to it.
//! * [`RecoveryPoint`] — what `StartBackupJob` stores inside the named vault.
//! * [`BackupJob`] — the job that produced that recovery point.
//! * [`CopyJobRecord`] — the job `StartCopyJob` starts to put a COPY of one
//!   vault's recovery point into another vault, and the copy it writes there.
//! * [`BackupPlanRecord`] — the plan `CreateBackupPlan` inserts, holding every
//!   [`BackupPlanVersion`] of it an update has made, whose body every plan read
//!   projects through [`BackupPlanVersion::backup_plan`].
//! * [`BackupSelectionRecord`] — one resource selection of such a plan.
//! * [`FrameworkRecord`] — the audit framework `CreateFramework` inserts, whose
//!   controls and deployment status every framework read answers from.
//! * [`LegalHoldRecord`] — the legal hold `CreateLegalHold` inserts, whose
//!   stored members every legal-hold read projects through
//!   [`LegalHoldRecord::listed`] and whose stored selection decides, through
//!   [`LegalHoldRecord::holds`], which recovery points the hold is on.
//! * [`RestoreJobRecord`] — the restore `StartRestoreJob` starts from a stored
//!   recovery point, which every call that names a `RestoreJobId` resolves
//!   against (the `restore` module).
//! * [`TieringConfigurationRecord`] — the tiering configuration
//!   `CreateTieringConfiguration` stores under its account-unique name, which
//!   the tiering read, listing and delete all answer from (the `tiering`
//!   module).
//!
//! A vault's resource policy is one of [`BackupVault`]'s own members
//! ([`BackupVault::access_policy`]): `PutBackupVaultAccessPolicy` stores it
//! there and `GetBackupVaultAccessPolicy` reports it from there, so the policy
//! a client attaches is the policy the vault carries.
//!
//! A job's state is also ANNOUNCED, not only stored: the `events` module holds the one diff
//! that turns every state a stored job moves into after its start call into an event on the
//! account's default EventBridge bus, so a client watching a backup or a copy through a
//! rule sees the same transitions a reader of the job does.
//!
//! `recovery_points_by_vault` is the ONLY per-vault list: both
//! `ListRecoveryPointsByBackupVault`'s page and `DescribeBackupVault`'s
//! `NumberOfRecoveryPoints` are derived from it, so the two can never disagree.
//! Both it and `vaults` are keyed by [`vault_key`] — the (region, name) pair a
//! backup vault is identified by — so the same name in two regions is two
//! independent vaults, each with its own recovery points and its own share of
//! the vault allowance.
//! [`VaultView`] is likewise the ONLY vault read projection: `DescribeBackupVault`
//! and `ListBackupVaults` both answer from it, and [`RecoveryPointView`] is the
//! ONLY recovery-point read projection: `DescribeRecoveryPoint` and
//! `ListRecoveryPointsByBackupVault` both answer from it.
//!
//! A restore access backup vault is one of those [`BackupVault`] records too,
//! of vault type `RESTORE_ACCESS_BACKUP_VAULT`, and
//! [`BackupVault::source_backup_vault_arn`] is the ONLY link it keeps to the
//! vault it gives access to: `ListRestoreAccessBackupVaults`' page,
//! `RevokeRestoreAccessBackupVault`'s target and `DescribeBackupVault`'s
//! `SourceBackupVaultArn` are all derived from it, so none of the three can
//! disagree with the others.
//!
//! [`ComAmazonawsBackupCryocontrollerusermanagerState::visible_vault`] (and its
//! `_mut` twin, for a call that writes the vault it resolves) is the resolution
//! step every vault-scoped call AWS Backup authorizes against the named vault
//! shares — the vault READS and the vault-LOCK actions: it applies the access
//! decision before the existence lookup, so none of them can disclose a vault the
//! caller cannot see.
//!
//! [`authorized_recovery_point`] is the same shape of decision for a call AWS
//! Backup authorizes against the RECOVERY POINT the request names by ARN — the
//! delete, the read and the two jobs that take one as input: an ARN of another
//! account is refused for lack of privileges before the point is resolved.

pub mod schedule;

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// The EventBridge events a job's state changes publish, and the path that publishes
/// them. Re-exported here so a handler reaches them through `crate::state` like every
/// other part of this service's state.
mod events;
pub use events::*;

/// What a started backup job comes to: whether the backup it was started for can be
/// performed at all, which is what selects its terminal state. Re-exported for the reason
/// given on `events`.
mod job_outcome;
pub use job_outcome::*;

/// Whether the protected resource a job was started for exists at all, as the service that
/// owns it answers. Re-exported for the reason given on `events`.
mod protected_resource;
pub use protected_resource::*;

/// The restore jobs this service stores, and the resolution steps the calls that
/// name one share. Re-exported for the reason given on `events`.
mod restore;
pub use restore::*;

/// Performing a restore: re-creating the protected resource in the service that
/// owns it. Re-exported for the reason given on `events`.
mod restore_execution;
pub use restore_execution::*;

/// What a restore's `Metadata` has to carry for the type of resource it restores, which a
/// start that carries less is refused for. Re-exported for the reason given on `events`.
mod restore_metadata;
pub use restore_metadata::*;

/// The tiering configurations this service stores, and the two projections its
/// reads answer from. Re-exported for the reason given on `events`.
mod tiering;
pub use tiering::*;

/// The tags this service's resources carry, and the resolution step the tagging
/// calls share. Re-exported for the reason given on `events`.
mod tagging;
pub use tagging::*;

/// The authorization decision a call that addresses a recovery point by ARN takes
/// before resolving it. Re-exported for the reason given on `events`.
mod authorization;
pub use authorization::*;

use crate::ops::types::{
    AdvancedBackupSetting, AlreadyExistsException, BackupPlan, BackupPlanInput, BackupRule,
    BackupRuleInput,
    BackupSelection, CalculatedLifecycle, ConflictException, CopyJob, FrameworkControl,
    InvalidParameterValueException, InvalidRequestException, InvalidResourceStateException,
    LegalHold, Lifecycle, MissingParameterValueException, RecoveryPointSelection,
    ResourceNotFoundException, ScanSetting,
};

/// The ARN namespace this service stamps into every ARN it mints
/// (`OPERATIONS.md`: "ARN namespace: `backup`").
const ARN_NAMESPACE: &str = "backup";

/// The region an ARN carries when the request is unsigned and therefore names
/// no SigV4 credential scope to read a region from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// `VaultType` for an ordinary backup vault (model enum `VaultType`).
pub const VAULT_TYPE_BACKUP_VAULT: &str = "BACKUP_VAULT";
/// `VaultType` for a logically air-gapped backup vault (model enum `VaultType`).
pub const VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT: &str =
    "LOGICALLY_AIR_GAPPED_BACKUP_VAULT";
/// `VaultType` for a restore access backup vault — the vault
/// `CreateRestoreAccessBackupVault` makes to give temporary access to the
/// recovery points of a logically air-gapped one (model enum `VaultType`).
pub const VAULT_TYPE_RESTORE_ACCESS_BACKUP_VAULT: &str = "RESTORE_ACCESS_BACKUP_VAULT";
/// `VaultState` of a vault that is ready to receive backups (model enum `VaultState`).
pub const VAULT_STATE_AVAILABLE: &str = "AVAILABLE";
/// `VaultState` of a vault whose creation has not settled yet (model enum
/// `VaultState`).
pub const VAULT_STATE_CREATING: &str = "CREATING";
/// `EncryptionKeyType` when the caller supplied no `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_AWS_OWNED: &str = "AWS_OWNED_KMS_KEY";
/// `EncryptionKeyType` when the caller supplied an `EncryptionKeyArn`.
pub const ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED_KMS_KEY";

/// The ARN namespace of the KMS keys a vault is encrypted with.
const KMS_ARN_NAMESPACE: &str = "kms";

/// The ARN namespace of the multi-party approval teams an
/// `MpaApprovalTeamArn` names.
const MPA_ARN_NAMESPACE: &str = "mpa";

/// The resource type an approval team's ARN carries within
/// [`MPA_ARN_NAMESPACE`], as the ARN of a team this service is given names it:
/// `approval-team/<team>`.
const MPA_APPROVAL_TEAM_RESOURCE_PREFIX: &str = "approval-team/";

/// The number of digits an AWS account id carries, which every ARN that names
/// an account states.
const ACCOUNT_ID_DIGITS: usize = 12;

/// The longest region name an approval team's ARN states, as the owning
/// service's `ApprovalTeamArn` pattern bounds it (`[a-z0-9-]{1,20}`).
const MAX_MPA_ARN_REGION_LENGTH: usize = 20;

/// The least `ChangeableForDays` accepts, as the model documents it: "Backup
/// enforces a 72-hour cooling-off period before Vault Lock takes effect and
/// becomes immutable. Therefore, you must set `ChangeableForDays` to 3 or
/// greater."
pub const MIN_CHANGEABLE_FOR_DAYS: i64 = 3;

/// The most `ChangeableForDays` accepts, as the model documents it: "The maximum
/// value you can specify is 36,500 days (approximately 100 years)."
pub const MAX_CHANGEABLE_FOR_DAYS: i64 = 36_500;

/// The least a vault-lock retention bound accepts, as the model documents for
/// `MinRetentionDays`: "The shortest minimum retention period you can specify is
/// 1 day."
pub const MIN_LOCK_RETENTION_DAYS: i64 = 1;

/// The most a vault-lock retention bound accepts, as the model documents for
/// `MaxRetentionDays`: "The longest maximum retention period you can specify is
/// 36500 days (approximately 100 years)."
pub const MAX_LOCK_RETENTION_DAYS: i64 = 36_500;

/// `BackupJobState` a freshly started job is in (model enum `BackupJobState`).
pub const BACKUP_JOB_STATE_CREATED: &str = "CREATED";
/// `BackupJobState` a settled job is in.
pub const BACKUP_JOB_STATE_COMPLETED: &str = "COMPLETED";
/// `BackupJobState` a job that could not produce its recovery point ends in (same
/// enum, `ABORTED | ABORTING | COMPLETED | CREATED | EXPIRED | FAILED | PARTIAL |
/// PENDING | RUNNING`); it carries the reason as the job's `StatusMessage`
/// ([`job_outcome`]).
pub const BACKUP_JOB_STATE_FAILED: &str = "FAILED";
/// `CopyJobState` a freshly started copy job is in (model enum `CopyJobState`,
/// `CREATED | RUNNING | COMPLETED | FAILED | PARTIAL`).
pub const COPY_JOB_STATE_CREATED: &str = "CREATED";
/// `CopyJobState` a settled copy job is in.
pub const COPY_JOB_STATE_COMPLETED: &str = "COMPLETED";
/// `DestinationVaultLockState` of a destination vault a Backup Vault Lock has
/// been applied to (`CopyJob$DestinationVaultLockState`: "Valid values include
/// LOCKED and UNLOCKED").
pub const VAULT_LOCK_STATE_LOCKED: &str = "LOCKED";
/// `RecoveryPointStatus` of a recovery point whose job has not settled yet.
pub const RECOVERY_POINT_STATUS_CREATING: &str = "CREATING";
/// `RecoveryPointStatus` of a recovery point whose job has settled.
pub const RECOVERY_POINT_STATUS_COMPLETED: &str = "COMPLETED";
/// `StorageClass` of a recovery point still in warm storage (model enum
/// `StorageClass`, `WARM | COLD | DELETED`).
pub const STORAGE_CLASS_WARM: &str = "WARM";
/// `StorageClass` of a recovery point whose lifecycle has transitioned it to
/// cold storage (same enum).
pub const STORAGE_CLASS_COLD: &str = "COLD";
/// `IndexStatus` of a recovery point whose index has not been built yet.
pub const INDEX_STATUS_PENDING: &str = "PENDING";
/// `IndexStatus` of a recovery point whose index is queryable.
pub const INDEX_STATUS_ACTIVE: &str = "ACTIVE";

/// `PercentDone` of a job that has not settled yet.
pub const PERCENT_DONE_NONE: &str = "0.0";
/// `PercentDone` of a settled job.
pub const PERCENT_DONE_COMPLETE: &str = "100.0";

/// The smallest `MaxResults` this service accepts: the minimum of the model's
/// `@range` on the `MaxResults` shape (`smithy_query` operation
/// `ListProtectedResources`: `MaxResults` `range {min: 1, max: 1000}`), which
/// every paginated Backup operation's `MaxResults` targets.
pub const MAX_RESULTS_MIN: i32 = 1;
/// The largest `MaxResults` this service accepts: the maximum of that same
/// `@range`.
pub const MAX_RESULTS_MAX: i32 = 1000;

/// The page size a list operation uses when the caller names no `MaxResults`:
/// the model's own `MaxResults` maximum, so one unpaginated call returns
/// everything a paginated one could.
pub const MAX_PAGE_SIZE: usize = MAX_RESULTS_MAX as usize;

/// The maximum either backup window accepts, in minutes: 100 years, as the
/// model documents for both members (`StartWindowMinutes`: "This parameter has
/// a maximum value of 100 years (52,560,000 minutes)"; `CompleteWindowMinutes`:
/// "Like StartWindowMinutes, this parameter has a maximum value of 100 years
/// (52,560,000 minutes)").
pub const MAX_WINDOW_MINUTES: i64 = 52_560_000;

/// The minimum `StartWindowMinutes` accepts, in minutes, as the model documents
/// ("If this value is included, it must be at least 60 minutes to avoid
/// errors").
pub const MIN_START_WINDOW_MINUTES: i64 = 60;

/// The `StartWindowMinutes` a plan rule is created with when the request states
/// none: AWS Backup's own default backup window of 8 hours, which is the window
/// the jobs that rule schedules really get. Both members are optional in the
/// request and the service fills each with its default, so a read of the plan
/// reports the window in force rather than nothing.
pub const DEFAULT_START_WINDOW_MINUTES: i64 = 8 * 60;

/// The `CompletionWindowMinutes` a plan rule is created with when the request
/// states none: AWS Backup's own default completion window of 7 days, the
/// sibling default of [`DEFAULT_START_WINDOW_MINUTES`].
pub const DEFAULT_COMPLETION_WINDOW_MINUTES: i64 = 7 * 24 * 60;

/// The most backup vaults this emulator keeps IN ONE REGION of the account.
///
/// An EMULATOR-CHOSEN ceiling, not an AWS quota: the store is in memory and
/// every request can add to it, so the slice answers the modelled
/// `LimitExceededException` past this point rather than growing without bound.
/// AWS enforces a per-account, per-region vault quota of its own, so that
/// modelled error is the right answer to the same request there, and one
/// region's vaults never consume another's allowance.
pub const MAX_VAULTS: usize = 500;

/// The most backup vaults this emulator keeps ACROSS every region of the
/// account.
///
/// The region a vault is keyed under ([`vault_key`]) is a field of the caller's
/// credential scope, so [`MAX_VAULTS`] alone would let the store grow with the
/// regions a caller signs for; this is the ceiling on the store as a whole, for
/// the reason given on [`MAX_DEFAULT_ENCRYPTION_KEYS`]. Also EMULATOR-CHOSEN,
/// and past it the same modelled `LimitExceededException` is answered.
pub const MAX_VAULTS_ALL_REGIONS: usize = 4 * MAX_VAULTS;

/// The most recovery points this emulator keeps across all vaults. An
/// EMULATOR-CHOSEN ceiling, for the reason given on [`MAX_VAULTS`]; AWS caps
/// concurrent backup jobs per account and answers `LimitExceededException` too.
pub const MAX_RECOVERY_POINTS: usize = 10_000;

/// The most backup jobs this emulator keeps. An EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_VAULTS`].
pub const MAX_BACKUP_JOBS: usize = 10_000;

/// The most copy jobs this emulator keeps. An EMULATOR-CHOSEN ceiling, for the
/// reason given on [`MAX_VAULTS`]; AWS caps concurrent copy jobs per account and
/// answers `LimitExceededException` too.
pub const MAX_COPY_JOBS: usize = 10_000;

/// The most backup plans this emulator keeps. An EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_VAULTS`]; AWS caps the backup plans an account may
/// hold and answers `LimitExceededException` too.
pub const MAX_BACKUP_PLANS: usize = 500;

/// The most rules this emulator keeps in ONE backup plan. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_VAULTS`]: the rule list is sized by
/// the request, so it is capped before the plan is stored. AWS caps the rules a
/// plan may hold too, and answers `LimitExceededException` for a plan past it.
pub const MAX_RULES_PER_PLAN: usize = 100;

/// The most versions of ONE backup plan this emulator keeps readable. An
/// EMULATOR-CHOSEN ceiling, for the reason given on [`MAX_VAULTS`]: every
/// update of a plan appends a version, so the oldest superseded ones are
/// forgotten once a plan has this many rather than letting one plan's history
/// grow without bound. The plan's CURRENT version is never one of them.
pub const MAX_BACKUP_PLAN_VERSIONS: usize = 100;

/// The most resource selections this emulator keeps in ONE backup plan. An
/// EMULATOR-CHOSEN ceiling, for the reason given on [`MAX_VAULTS`]; AWS caps the
/// selections a plan may hold too, and answers `LimitExceededException` for a
/// create past it.
pub const MAX_SELECTIONS_PER_PLAN: usize = 100;

/// The most frameworks this emulator keeps. An EMULATOR-CHOSEN ceiling, for the
/// reason given on [`MAX_VAULTS`]; AWS caps the frameworks an account may hold
/// per Region too, and answers `LimitExceededException` for a create past it.
pub const MAX_FRAMEWORKS: usize = 500;

/// The most legal holds this emulator keeps. An EMULATOR-CHOSEN ceiling, for
/// the reason given on [`MAX_VAULTS`]; AWS caps the legal holds an account may
/// hold too, and answers `LimitExceededException` for a create past it.
pub const MAX_LEGAL_HOLDS: usize = 500;

/// The most controls this emulator keeps in ONE framework. An EMULATOR-CHOSEN
/// ceiling, for the reason given on [`MAX_RULES_PER_PLAN`]: the list is sized by
/// the request and stored verbatim.
pub const MAX_CONTROLS_PER_FRAMEWORK: usize = 100;

/// The most key-value pairs a control scope's tag selector admits, as the model
/// documents for `ControlScope$Tags`: "A maximum of one key-value pair can be
/// provided."
pub const MAX_CONTROL_SCOPE_TAGS: usize = 1;

/// The most `IdempotencyToken` records this emulator keeps. An EMULATOR-CHOSEN
/// ceiling: the key is a string the CALLER picks, so this is one of the two
/// stores whose keys a caller controls (the other is
/// [`MAX_DEFAULT_ENCRYPTION_KEYS`]'s). Past the ceiling the oldest record is
/// dropped — a dropped token simply stops replaying, which is the same answer a
/// caller gets for a token it never sent.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1_000;

/// The most default encryption keys this emulator keeps: one per region the
/// requests it has served have named. An EMULATOR-CHOSEN ceiling for the reason
/// given on [`MAX_VAULTS`] — the region comes from the caller's credential
/// scope, so this store's keys are caller-chosen too. Past the ceiling another
/// region's key is dropped; every vault keeps the key ARN recorded ON it, so a
/// drop only means the next vault created in that region is encrypted by a
/// newly created default key.
pub const MAX_DEFAULT_ENCRYPTION_KEYS: usize = 64;

/// How long a recorded `IdempotencyToken` stays replayable. An
/// EMULATOR-CHOSEN span: AWS documents no lifetime for Backup's idempotency
/// tokens, and an in-memory store that never forgets a caller-chosen key grows
/// with the request count.
pub const IDEMPOTENCY_TOKEN_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// How long after `StartBackupJob` the emulated job settles from `CREATED` to
/// `COMPLETED`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as copying the
/// protected resource needs. It is deliberately non-zero so a caller that
/// describes the job straight after starting it still observes the transient
/// `CREATED` state, and the due instant is stored on the job
/// ([`BackupJob::settles_at`]) so no handler ever sleeps for it.
pub const JOB_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How far back a backup-job listing reaches: "Returns a list of existing
/// backup jobs for an authenticated account for the last 30 days. For a longer
/// period of time, consider using these monitoring tools" (the `ListBackupJobs`
/// doc). A DOCUMENTED span, so it is the span the listing uses; a job older
/// than it is still stored and still reported by a read that names it.
pub const BACKUP_JOB_LIST_WINDOW: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How long after a create the vault settles from `CREATING` to `AVAILABLE`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as provisioning the
/// vault needs. It is deliberately non-zero so a caller that describes or
/// deletes the vault straight after creating it still observes the transient
/// `CREATING` state and the refusal that goes with it, and the due instant is
/// stored on the vault ([`BackupVault::settles_at`]) so no handler ever sleeps
/// for it.
pub const VAULT_SETTLE_AFTER: Duration = Duration::from_millis(500);

/// How long after a create or an update the framework's deployment settles to
/// `COMPLETED`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as deploying the
/// framework's controls needs. It is deliberately non-zero so a caller that
/// describes, updates or deletes the framework straight after creating it still
/// observes the transient `*_IN_PROGRESS` deployment status and the refusals
/// that go with it, and the due instant is stored on the framework
/// ([`FrameworkRecord::settles_at`]) so no handler ever sleeps for it.
pub const FRAMEWORK_SETTLE_AFTER: Duration = Duration::from_secs(3);

/// How long after a create the legal hold settles from `CREATING` to `ACTIVE`.
///
/// A CHOSEN span, not a documented one: AWS takes as long as placing the hold on
/// the recovery points its selection matches needs. It is deliberately non-zero
/// so a caller that reads the hold straight after creating it still observes the
/// transient `CREATING` status, and the due instant is stored on the hold
/// ([`LegalHoldRecord::settles_at`]) so no handler ever sleeps for it.
pub const LEGAL_HOLD_SETTLE_AFTER: Duration = Duration::from_secs(3);

/// `LegalHoldStatus` of a legal hold whose creation has not settled yet (model
/// enum `LegalHoldStatus`: `CREATING | ACTIVE | CANCELING | CANCELED`).
pub const LEGAL_HOLD_STATUS_CREATING: &str = "CREATING";

/// `LegalHoldStatus` of a legal hold that is in force (same enum).
pub const LEGAL_HOLD_STATUS_ACTIVE: &str = "ACTIVE";

/// `LegalHoldStatus` of a legal hold a cancellation has released (same enum).
/// A cancelled hold is still reported: `CancelLegalHold` records why and when it
/// was cancelled, which the hold's reads answer from.
pub const LEGAL_HOLD_STATUS_CANCELED: &str = "CANCELED";

/// `DeploymentStatus` of a framework whose deployment has not settled yet, as
/// the model lists the statuses for `Framework$DeploymentStatus`
/// (`CREATE_IN_PROGRESS | UPDATE_IN_PROGRESS | DELETE_IN_PROGRESS | COMPLETED |
/// FAILED`).
pub const FRAMEWORK_DEPLOYMENT_STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";

/// `DeploymentStatus` of a framework whose controls are being re-deployed by an
/// update that has not settled yet (same list).
pub const FRAMEWORK_DEPLOYMENT_STATUS_UPDATE_IN_PROGRESS: &str = "UPDATE_IN_PROGRESS";

/// `DeploymentStatus` of a framework whose deployment has settled (same list).
pub const FRAMEWORK_DEPLOYMENT_STATUS_COMPLETED: &str = "COMPLETED";

/// `FrameworkStatus` of a deployed framework: "ACTIVE when recording is turned
/// on for all resources governed by the framework" (model doc for
/// `DescribeFrameworkOutput$FrameworkStatus`) — which is what this emulator
/// does, since nothing here turns recording off for a resource type.
pub const FRAMEWORK_STATUS_ACTIVE: &str = "ACTIVE";

/// `FrameworkStatus` of a framework whose deployment has not settled yet:
/// "UNAVAILABLE when Backup is unable to validate recording status at this
/// time" (same doc) — until the controls are deployed there is nothing whose
/// recording status can be validated.
pub const FRAMEWORK_STATUS_UNAVAILABLE: &str = "UNAVAILABLE";

/// The smallest `MinRetentionDays` a logically air-gapped vault accepts, as the
/// model documents for the member ("This setting specifies the minimum
/// retention period that the vault retains its recovery points. The minimum
/// value accepted is 7 days").
pub const MIN_RETENTION_DAYS_FLOOR: i64 = 7;

/// The `Code` AWS Backup carries on the `InvalidRequestException` that refuses
/// an operation on a vault still in `CREATING` state.
pub const ERROR_CODE_VAULT_STILL_CREATING: &str = "ERROR_9100";

/// A seconds-per-day constant for turning day counts a request states — a
/// `Lifecycle`'s retention, a cancelled legal hold's record retention — into the
/// instants a read reports.
pub const SECONDS_PER_DAY: u64 = 24 * 60 * 60;

/// The ARN of a backup vault: `arn:aws:backup:<region>:<account>:backup-vault:<name>`.
pub fn backup_vault_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("backup-vault:{name}"))
}

/// The key a backup vault's record — and its recovery-point index — is stored
/// under: the (region, name) pair that IDENTIFIES a vault.
///
/// "Backup vaults are identified by names that are unique to the account used
/// to create them and the Amazon Web Services Region where they are created"
/// (model doc for `BackupVaultName`), so a vault name is unique within one
/// region of the account and the same name in another region is a different,
/// independent vault. Every vault lookup, insert, delete and count therefore
/// resolves within ONE region, which this one key is what makes true of all of
/// them.
///
/// Region-first, so one region's vaults are contiguous in the store and a read
/// of them is a range of the one map rather than a second index beside it.
pub fn vault_key(region: &str, vault_name: &str) -> (String, String) {
    (region.to_string(), vault_name.to_string())
}

/// The ARN of a KMS key: `arn:aws:kms:<region>:<account>:key/<key id>`.
pub fn kms_key_arn(region: &str, key_id: &str) -> String {
    ::engine::emulator_arn(KMS_ARN_NAMESPACE, region, &format!("key/{key_id}"))
}

/// The ARN of a recovery point: `arn:aws:backup:<region>:<account>:recovery-point:<id>`.
pub fn recovery_point_arn(region: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("recovery-point:{id}"))
}

/// The ARN of a backup plan: `arn:aws:backup:<region>:<account>:backup-plan:<id>`.
///
/// `backup-plan` is the resource type a Backup plan is addressed by — the form
/// an IAM policy and a CloudFormation `BackupPlanArn` carry — so it is spelled
/// in full here, as [`backup_vault_arn`] spells `backup-vault`; the model's doc
/// example for the member abbreviates it.
pub fn backup_plan_arn(region: &str, plan_id: &str) -> String {
    ::engine::emulator_arn(ARN_NAMESPACE, region, &format!("backup-plan:{plan_id}"))
}

/// The ARN of a framework:
/// `arn:aws:backup:<region>:<account>:framework:<name>-<uniquifier>`.
///
/// A framework is addressed by NAME, and the name alone is not unique over time
/// — a deleted framework's name can be taken again — so the ARN AWS mints for
/// one carries a uniquifier after the name; `uniquifier` is a value only this
/// service mints ([`ComAmazonawsBackupCryocontrollerusermanagerState::next_id`]).
pub fn framework_arn(region: &str, name: &str, uniquifier: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("framework:{name}-{uniquifier}"),
    )
}

/// The ARN of a legal hold:
/// `arn:aws:backup:<region>:<account>:legal-hold:<id>`.
///
/// `legal-hold` is the resource type a legal hold is addressed by, spelled in
/// full and separated by `:` as [`backup_vault_arn`] spells `backup-vault` and
/// [`recovery_point_arn`] spells `recovery-point`; the model's doc example for
/// `LegalHold$LegalHoldArn` shows a recovery point's ARN instead, which names no
/// legal hold at all.
pub fn legal_hold_arn(region: &str, legal_hold_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("legal-hold:{legal_hold_id}"),
    )
}

/// The ARN of a tiering configuration:
/// `arn:aws:backup:<region>:<account>:tiering-configuration:<name>`.
///
/// A tiering configuration is addressed by NAME — the name "cannot be changed
/// after creation" (model doc for `TieringConfigurationName`) and is unique to
/// the account — so the name is what its ARN carries, spelled with the
/// resource type separated by `:` as [`backup_vault_arn`] spells
/// `backup-vault` and [`legal_hold_arn`] spells `legal-hold`.
pub fn tiering_configuration_arn(region: &str, name: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("tiering-configuration:{name}"),
    )
}

/// The region this request's ARNs carry: the one the caller signed for, from the
/// SigV4 credential scope, falling back to [`DEFAULT_REGION`] for an unsigned
/// request — or for a credential scope whose region field is not shaped like a
/// region name, which names no region this service can mint an ARN for.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_like(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `value` is shaped like an AWS Region name: 1 to 32 lowercase
/// letters, digits and hyphens, which is the form every published region name
/// takes (`us-east-1`, `ap-southeast-4`, `us-gov-west-1`).
///
/// The region reaches this service as a free-form field of the caller's SigV4
/// credential scope, so it is checked before it is stitched into an ARN or used
/// as a key of this service's own state.
fn is_region_like(value: &str) -> bool {
    (1..=32).contains(&value.len())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// Encode a pagination cursor: the key of the first item of the NEXT page.
///
/// Hex-encoded so the token is opaque to the caller and safe to carry in a
/// `@httpQuery` parameter whatever the key holds (a recovery-point ARN holds
/// both `:` and `/`).
pub fn encode_page_token(key: &str) -> String {
    hex::encode(key)
}

/// Decode a cursor produced by [`encode_page_token`]; `None` when the token was
/// not produced by this service.
pub fn decode_page_token(token: &str) -> Option<String> {
    hex::decode(token)
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
}

/// Whether `value` satisfies the model's `ResourceType` pattern
/// (`^[a-zA-Z0-9\-\_\.]{1,50}$`), so a derived resource type is never reported
/// in a form the model refuses.
fn is_modelled_resource_type(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 50
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

/// Whether `value` satisfies the model's `BackupVaultName` pattern
/// (`^[a-zA-Z0-9\-\_]{2,50}$`).
///
/// The operations that take the vault name in the URI path (`CreateBackupVault`,
/// `ListRecoveryPointsByBackupVault`) target the patterned `BackupVaultName`
/// shape, so the framework refuses a bad name for them before any handler runs.
/// `DescribeBackupVault` and `DeleteBackupVault` target a plain `string`, so
/// they apply the same rule here rather than reporting a name the service could
/// never have stored as merely absent.
pub fn is_modelled_vault_name(value: &str) -> bool {
    (2..=50).contains(&value.chars().count())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The `VaultNames` entry that selects EVERY vault rather than one vault.
///
/// A recovery-point selection's entries are plain strings, so this one cannot be
/// read as the name of a vault: the `BackupVaultName` pattern
/// ([`is_modelled_vault_name`]) admits no `*`, so no vault of this account can
/// ever be called that, and an entry read literally would narrow the hold to a
/// vault that cannot exist. It is the form a client uses to place a hold across
/// all vaults.
pub const VAULT_NAMES_WILDCARD: &str = "*";

/// Whether `value` is structurally an ARN: the `arn` scheme and the six
/// colon-separated fields `arn:<partition>:<service>:<region>:<account>:<resource>`
/// every AWS ARN carries (the resource field may itself contain `:`).
///
/// The model types these members as `ARN` with no pattern, so the framework
/// cannot refuse a malformed one; AWS does, with
/// `InvalidParameterValueException`.
pub fn is_arn(value: &str) -> bool {
    let mut fields = value.splitn(6, ':');
    let (Some("arn"), Some(partition), Some(service), Some(_region), Some(_account), Some(resource)) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    ) else {
        return false;
    };
    !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// The fields a backup-vault ARN names, as [`backup_vault_arn_parts`] reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackupVaultArnParts {
    /// The region the vault, and anything stored in it, belongs to — which is
    /// not this request's region when the ARN names a vault in another one.
    pub region: String,
    /// The account that owns the vault.
    pub account_id: String,
    /// The `BackupVaultName`, in a form this service could have stored it under
    /// ([`is_modelled_vault_name`]).
    pub name: String,
}

/// The fields a backup-vault ARN names, or `None` when `value` is not one:
/// `arn:<partition>:backup:<region>:<account>:backup-vault:<name>`, the form
/// [`backup_vault_arn`] mints and the form the model's own doc gives for every
/// member that carries a vault ARN ("for example,
/// `arn:aws:backup:us-east-1:123456789012:backup-vault:aBackupVault`").
pub fn backup_vault_arn_parts(value: &str) -> Option<BackupVaultArnParts> {
    let mut fields = value.splitn(6, ':');
    let (
        Some("arn"),
        Some(partition),
        Some(ARN_NAMESPACE),
        Some(region),
        Some(account_id),
        Some(resource),
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    let name = resource.strip_prefix("backup-vault:")?;
    (!partition.is_empty() && is_region_like(region) && is_modelled_vault_name(name)).then(|| {
        BackupVaultArnParts {
            region: region.to_string(),
            account_id: account_id.to_string(),
            name: name.to_string(),
        }
    })
}

/// The region and the account a multi-party approval team's ARN names, or
/// `None` when `value` is not one.
///
/// The form is the one the service that MINTS a team declares for it —
/// `arn:aws(-<partition>)?:mpa:<region>:<12 digit account>:approval-team/<team>`
/// with the team made of letters, digits, `.`, `_` and `-`
/// (`smithy_query(query="shape", shape="ApprovalTeamArn",
/// model_path="services/service-com-amazonaws-mpa-awsfluffycoreservice/smithy-model/com-amazonaws-mpa-awsfluffycoreservice.json")`)
/// — not a form this service invents: a member that names ONE resource of ONE
/// service is only an ARN of that resource, and this service's model types
/// `MpaApprovalTeamArn` as `ARN` with no pattern, so the framework admits the
/// ARN of anything at all.
pub fn mpa_approval_team_arn_parts(value: &str) -> Option<(&str, &str)> {
    let mut fields = value.split(':');
    let (
        Some("arn"),
        Some(partition),
        Some(MPA_ARN_NAMESPACE),
        Some(region),
        Some(account_id),
        Some(resource),
        None,
    ) = (
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
        fields.next(),
    )
    else {
        return None;
    };
    let partition_named = partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|suffix| !suffix.is_empty());
    let region_named = (1..=MAX_MPA_ARN_REGION_LENGTH).contains(&region.len())
        && region
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    let account_named =
        account_id.len() == ACCOUNT_ID_DIGITS && account_id.chars().all(|c| c.is_ascii_digit());
    let team_named = resource
        .strip_prefix(MPA_APPROVAL_TEAM_RESOURCE_PREFIX)
        .is_some_and(|team| {
            !team.is_empty()
                && team
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        });
    (partition_named && region_named && account_named && team_named)
        .then_some((region, account_id))
}

// ===========================================================================
// This service's error vocabulary
// ===========================================================================
//
// Every AWS Backup client-error shape carries `Code`, `Message`, `Type` and
// `Context` members (`smithy_query(query="shape", shape="InvalidParameterValueException")`),
// and a real response fills `Code` with this service's own token for the
// CONDITION — not for the exception class, since one class answers several
// conditions with different tokens. Each condition whose token this emulator
// has observed names it once here and builds its exception once below, so two
// operations can never answer one condition with two codes or two sentences.
//
// A condition with no observed token leaves `Code` unset (the model makes the
// member optional): inventing an `ERROR_nnnn` would hand a client branching on
// the code a token AWS never sends, which is worse than the absent member.
//
// A token below, and a sentence marked FIXED, is one AWS states for the
// condition rather than for the request — it names no part of the request, so
// it is a constant here and not a format string.

/// The sentence AWS states for "this vault's TYPE does not support the
/// multi-party approval call that was made on it" — fixed, since it names no
/// part of the request.
///
/// Kept as the vocabulary of the condition, for the operation that STATES it:
/// `DisassociateBackupVaultMpaApprovalTeam`, which carries no member naming a
/// team, so its refusal can only speak of the vault
/// (`20261029T175130Z-backup-aws-backup-recovery-point-restore-job-mpa-and`
/// records that wording and is the issue that implements it). The association,
/// which does carry one, reports the member it cannot honour instead
/// ([`rejected_parameter`]).
pub const MESSAGE_UNSUPPORTED_VAULT_TYPE: &str =
    "Operation not supported for the provided vault type";

/// `Code` AWS fills in for "a member of the request carries a value this
/// service cannot accept". ONE token for the whole class: which member was
/// rejected is reported in `Context`, not in the token.
pub const ERROR_CODE_INVALID_PARAMETER_VALUE: &str = "INVALID_PARAMETER_VALUE";

/// `Code` for "the source vault a restore access create names is of a type that
/// cannot grant restore access".
pub const ERROR_CODE_SOURCE_VAULT_UNSUPPORTED: &str = "ERROR_4134";
/// The sentence AWS states for that condition — fixed: neither the vault nor
/// its type appears in it.
pub const MESSAGE_SOURCE_VAULT_UNSUPPORTED: &str =
    "Provided backup vault does not support this operation.";

/// `Code` for "the vault named by a call that only a logically air-gapped vault
/// answers is of another type".
pub const ERROR_CODE_VAULT_TYPE_INVALID: &str = "ERROR_3936";
/// The sentence AWS states for that condition — fixed: the accepted type it
/// names is the `VaultType` enum's own token
/// ([`VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT`]), not a value of the
/// request.
pub const MESSAGE_VAULT_TYPE_NOT_AIR_GAPPED: &str =
    "The vault type is invalid. Accepted type is LOGICALLY_AIR_GAPPED_BACKUP_VAULT.";

/// The sentence AWS states for "the `RestoreAccessBackupVaultArn` the request
/// carries does not name a restore access backup vault of the named source
/// vault" — fixed: it names no part of the request, so the ARN asked about does
/// not appear in it. No `Code`: this condition has no observed token.
pub const MESSAGE_RESTORE_ACCESS_VAULT_ARN_INVALID: &str =
    "The provided restore access backup vault ARN is not valid.";

/// The sentence AWS states for "a restore access backup vault was asked for
/// against a source vault that has no MPA approval team to approve it" — fixed:
/// it names no part of the request, so the vault asked about does not appear in
/// it. No `Code`: this condition has no observed token.
pub const MESSAGE_NO_MPA_APPROVAL_TEAM: &str = "Cannot create restore access backup vault. \
                                               No MPA approval team associated with the backup \
                                               vault.";

/// `Code` for "the backup job asked about is not one this account holds".
pub const ERROR_CODE_BACKUP_JOB_NOT_FOUND: &str = "ERROR_2403";
/// The sentence AWS states for that condition. A FIXED string: it names no part
/// of the request, so the id asked about does not appear in it.
pub const MESSAGE_BACKUP_JOB_NOT_FOUND: &str = "Cannot find Backup job";

/// `Code` for "the copy job asked about is not one this account holds".
pub const ERROR_CODE_COPY_JOB_NOT_FOUND: &str = "ERROR_21001";
/// The sentence AWS states for that condition — fixed: it names no part of the
/// request, so the id asked about does not appear in it.
pub const MESSAGE_COPY_JOB_NOT_FOUND: &str = "Copy job not found";

/// `Code` for "a backup vault of this name already exists".
pub const ERROR_CODE_VAULT_ALREADY_EXISTS: &str = "ERROR_1101";
/// The sentence AWS states for that condition — fixed, like the one above.
pub const MESSAGE_VAULT_ALREADY_EXISTS: &str = "Backup vault with the same name already exists";

/// The sentence AWS states for "the backup plan this call resolves is not one
/// this account holds" on a call that resolves one WITHOUT reading a version of
/// it — fixed, and worded as this service words the same condition for a backup
/// job ([`MESSAGE_BACKUP_JOB_NOT_FOUND`]): it names no part of the request. A
/// plan READ states the version it could not read instead
/// ([`MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE`]). No `Code`: this condition has
/// no observed token.
pub const MESSAGE_BACKUP_PLAN_NOT_FOUND: &str = "Cannot find Backup plan";

/// `Code` for "the version of the backup plan this read asks for cannot be
/// read".
pub const ERROR_CODE_BACKUP_PLAN_VERSION_UNREADABLE: &str = "ERROR_2202";
/// The sentence AWS states for that condition — fixed: it names neither the
/// plan id nor the version asked about. A plan READ answers from the plan at a
/// version, so it is the sentence for EVERY way such a read can fail to resolve
/// one: the plan's deletion is recorded, the `VersionId` names a version the
/// plan never had, or the account holds no plan of that id at all — in each,
/// the version the read asked for is not there to read.
pub const MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE: &str =
    "Failed reading Backup plan with provided version";

/// The FIXED text AWS states for "the LATEST version of the backup plan this
/// call must read before it acts is not there to read", up to and after the one
/// variable part: the ARN of the plan the call addressed.
///
/// Unlike [`MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE`], which a READ of a named
/// version answers, this is the sentence the call that reads a plan's latest
/// version states, and it names the plan — built from the partition, region and
/// account of the request and the `BackupPlanId` it carried
/// ([`backup_plan_arn`]), never from anything else the request sent. Composed by
/// [`latest_backup_plan_version_unreadable`], the one place it is built.
pub const MESSAGE_LATEST_BACKUP_PLAN_VERSION_UNREADABLE_PREFIX: &str =
    "Failed reading latest version of backup plan ";
/// The text that follows the plan ARN in that sentence.
pub const MESSAGE_LATEST_BACKUP_PLAN_VERSION_UNREADABLE_SUFFIX: &str =
    " - backup plan does not exist!";

/// `Code` for "a RULE of the backup plan body carries something AWS Backup
/// refuses". ONE token for the whole per-rule validation class: which rule and
/// which reason is reported in `Context` and in the sentence, not in the token.
pub const ERROR_CODE_BACKUP_PLAN_RULE_INVALID: &str = "ERROR_3015";

/// The FIXED text that opens the sentence AWS states for that class; the
/// per-rule validation reason follows it, and is also carried whole in
/// `Context` ([`rejected_backup_plan_rule`]).
pub const MESSAGE_BACKUP_PLAN_RULES_PREFIX: &str = "Error in some rules due to : ";

/// The FIXED text that opens the per-rule reason for a `ScheduleExpression`
/// that is not a cron expression; the parse failure follows it.
pub const MESSAGE_CRON_INVALID_PREFIX: &str = "Cron expression is not valid: ";

/// The parse failure stated for an expression that is not a cron expression at
/// all — the whole condition [`schedule::rule_schedule_refusal`] decides, since
/// what fails is the parse and no part of the expression is quoted back.
pub const MESSAGE_CRON_INVALID: &str = "Invalid cron expression";

/// The service principal a role AWS Backup acts under has to let assume it:
/// this service's own, which the model states — the service shape's
/// `aws.api#service` trait carries `cloudTrailEventSource` `backup.amazonaws.com`
/// (`smithy_query(query="service_info")`).
pub const SERVICE_PRINCIPAL: &str = "backup.amazonaws.com";

/// `Code` for "the IAM role this call would act under is not one AWS Backup can
/// assume".
pub const ERROR_CODE_ROLE_NOT_ASSUMABLE: &str = "ERROR_3017";
/// The sentence AWS states for that condition — FIXED text: it names neither
/// the role nor the resource the call addressed.
pub const MESSAGE_ROLE_NOT_ASSUMABLE: &str = "IAM Role cannot be assumed by AWS Backup";

/// `Code` for "the `BackupPlanId` a selection READ carries names no backup plan
/// of this account".
pub const ERROR_CODE_SELECTION_READ_PLAN_NOT_FOUND: &str = "ERROR_2304";
/// `Code` for the same condition on the selection DELETE: AWS Backup numbers
/// its tokens per call, so the two differ even though the condition is one.
pub const ERROR_CODE_SELECTION_DELETE_PLAN_NOT_FOUND: &str = "ERROR_2302";
/// The sentence AWS states for that condition — FIXED text: the plan id asked
/// about does not appear in it.
pub const MESSAGE_BACKUP_PLAN_NOT_FOUND_BY_ID: &str = "Cannot find Backup plan with ID";

/// `Code` for "the selection asked about is not one the named plan holds".
pub const ERROR_CODE_BACKUP_SELECTION_NOT_FOUND: &str = "ERROR_2303";

/// The sentence AWS states for "the recovery point asked about is not one the
/// named vault holds" — fixed, and worded as this service words the same
/// condition for a backup job ([`MESSAGE_BACKUP_JOB_NOT_FOUND`]): it names no
/// part of the request, so neither the vault nor the ARN asked about appears in
/// it. It answers every way a (vault, ARN) pair can fail to name a stored
/// recovery point. No `Code`: this condition has no observed token.
pub const MESSAGE_RECOVERY_POINT_NOT_FOUND: &str = "Cannot find recovery point";

/// The sentence AWS states for "the vault holding this recovery point is
/// configured with a Backup Vault Lock, which still retains it" — fixed: it
/// names the condition (the lock), not the request. No `Code`: this condition
/// has no observed token.
pub const MESSAGE_RECOVERY_POINT_LOCKED: &str =
    "RecoveryPoint cannot be deleted or updated (Backup vault configured with Lock)";

/// The sentence AWS states for "an active legal hold is on this recovery point,
/// so the action just requested is restrained": "Any actions to delete or
/// disassociate a recovery point will fail with an error if one or more active
/// legal holds are on the recovery point" (model doc for `CreateLegalHold`).
/// Fixed text, like the restraint the vault's own lock states
/// ([`MESSAGE_RECOVERY_POINT_LOCKED`]): it names the condition, not the
/// request, so neither the point nor the hold appears in it. No `Code`: this
/// condition has no observed token.
pub const MESSAGE_RECOVERY_POINT_HELD: &str =
    "The recovery point cannot be deleted because it has active legal hold(s).";

/// The sentence AWS states for "the backup selection asked about is not one the
/// named plan holds" — fixed, and worded as this service words the same
/// condition for a backup plan ([`MESSAGE_BACKUP_PLAN_NOT_FOUND`]): it names no
/// part of the request. Its token is
/// [`ERROR_CODE_BACKUP_SELECTION_NOT_FOUND`].
pub const MESSAGE_BACKUP_SELECTION_NOT_FOUND: &str = "Cannot find Backup selection";

/// The sentence AWS states for "a backup plan of this name already exists",
/// worded as this service words the same condition for a vault
/// ([`MESSAGE_VAULT_ALREADY_EXISTS`]) — fixed, and with no observed `Code`.
pub const MESSAGE_PLAN_ALREADY_EXISTS: &str = "Backup plan with the same name already exists";

/// The sentence AWS states for "a resource selection of this name already
/// exists in the named plan", worded as this service words the same condition
/// for a plan ([`MESSAGE_PLAN_ALREADY_EXISTS`]) — fixed, and with no observed
/// `Code`.
pub const MESSAGE_SELECTION_ALREADY_EXISTS: &str =
    "Backup selection with the same name already exists";

/// The sentence AWS states for "the framework asked about is not one this
/// account holds": fixed text around the framework the call addressed and the
/// account it was made in, so a client that surfaces or matches the refusal can
/// tell WHICH framework and WHICH account was missing. The variable parts are
/// the name the request carried and the account this emulator presents. No
/// `Code`: this condition has no observed token.
pub fn framework_not_found_message(name: &str) -> String {
    format!(
        "Framework {name} not found in account {}.",
        ::engine::emulator_account_id()
    )
}

/// The sentences AWS states for "this account already holds a framework the one
/// being created would duplicate": fixed text that attributes the conflict to
/// the controls and configuration and tells the caller what to change for the
/// name the create asked for. The variable parts are the account this emulator
/// presents and the name the request carried; no observed `Code`.
pub fn framework_already_exists_message(name: &str) -> String {
    format!(
        "A framework with these controls and configurations already exists in this account {}. \
         To create new framework {name}, change the number or scope of the controls.",
        ::engine::emulator_account_id()
    )
}

/// The sentence AWS states for "a previous action on this framework is still
/// running, so the one just requested cannot be performed": fixed text naming
/// the framework the call addressed and the account it was made in, as the two
/// framework refusals above do. No `Code`: this condition has no observed
/// token.
pub fn framework_action_in_progress_message(name: &str) -> String {
    format!(
        "Operation failed because of a concurrent update for the framework {name} in account \
         {}. Try again later.",
        ::engine::emulator_account_id()
    )
}

/// The sentence AWS states for "a control the request names is not one Backup
/// Audit Manager supports": fixed text around the control name, taken verbatim
/// from the request's `FrameworkControls` entry.
pub fn unsupported_framework_control_message(control_name: &str) -> String {
    format!("You can't create the framework control {control_name} because it is unsupported.")
}

/// `Code` for "`MaxResults` is outside the range the model declares".
pub const ERROR_CODE_MAX_RESULTS_INVALID: &str = "ERROR_3607";
/// The sentence AWS states for that condition — fixed, like the ones above.
pub const MESSAGE_MAX_RESULTS_INVALID: &str = "Max Results is invalid";

/// `Code` for "the legal hold a hold-scoped call addresses is not one this
/// account holds".
pub const ERROR_CODE_LEGAL_HOLD_NOT_FOUND: &str = "ERROR_4218";
/// The sentence AWS states for that condition — fixed: it names no part of the
/// request, so neither the id nor the ARN asked about appears in it. AWS speaks
/// of the hold's ARN whichever member named it, since the id a hold-scoped call
/// carries is what the hold's ARN is built from ([`legal_hold_arn`]).
pub const MESSAGE_LEGAL_HOLD_NOT_FOUND: &str = "Can not find Legal Hold ARN.";

/// `Code` for "the legal hold named is not one a cancellation can release" —
/// the condition's own token, constant across the states that raise it.
pub const ERROR_CODE_LEGAL_HOLD_NOT_ACTIVE: &str = "ERROR_22003";

/// The FIXED text AWS states for "the legal hold named is not one a
/// cancellation can release": it is still being placed, or a cancellation has
/// already released it. Unlike the sentences above this one has a VARIABLE
/// part — the stored hold's current status, appended followed by the sentence's
/// final `.` — so the refusal says WHICH state blocked the cancel. Built by
/// [`legal_hold_not_active`], the one place the sentence is composed.
pub const MESSAGE_LEGAL_HOLD_NOT_ACTIVE_PREFIX: &str =
    "Only ACTIVE legal holds can be canceled, the current state of provided legal hold is ";

/// The sentence AWS states for "the legal hold this create would make selects no
/// recovery points" — fixed, like the ones above: it names no part of the
/// request. No `Code`: this condition has no observed token.
pub const MESSAGE_MISSING_RESOURCE_SELECTION: &str = "Missing Resource Selection.";

/// The values the model's `AggregationPeriod` enum admits, in model order
/// (`smithy_query(query="shape", shape="AggregationPeriod")`). The summary
/// operations name this list in the message they refuse an unknown period with,
/// so the list a caller is told about is the list the model declares.
pub const AGGREGATION_PERIODS: [&str; 3] = ["ONE_DAY", "SEVEN_DAYS", "FOURTEEN_DAYS"];

/// The sentence AWS states for "the restore job asked about is not one this
/// account holds" — fixed, and worded as this service words the same condition
/// for a backup job ([`MESSAGE_BACKUP_JOB_NOT_FOUND`]) and for a recovery point
/// ([`MESSAGE_RECOVERY_POINT_NOT_FOUND`]): it names no part of the request, so
/// the id asked about does not appear in it. It answers every call that
/// addresses a restore job by id — the read, the metadata read and the
/// validation result. No `Code`: this condition has no observed token.
pub const MESSAGE_RESTORE_JOB_NOT_FOUND: &str = "Cannot find restore job";

/// The sentence AWS states for "the tiering configuration asked about is not
/// one this account holds" — fixed, and worded as the conditions above are: it
/// names no part of the request, so the name asked about does not appear in it.
/// No `Code`: this condition has no observed token.
pub const MESSAGE_TIERING_CONFIGURATION_NOT_FOUND: &str = "Cannot find tiering configuration";

/// The sentence AWS states for "a tiering configuration of this name already
/// exists", worded as this service words the same condition for a vault
/// ([`MESSAGE_VAULT_ALREADY_EXISTS`]) — fixed, and with no observed `Code`.
pub const MESSAGE_TIERING_CONFIGURATION_ALREADY_EXISTS: &str =
    "Tiering configuration with the same name already exists";

/// The FIXED text AWS states for "the vault named carries no resource policy to
/// report", up to and after the one variable part: the ARN of the vault the
/// read addressed. Composed by [`vault_access_policy_not_found`], the one place
/// the sentence is built. No `Code`: this condition has no observed token.
pub const MESSAGE_VAULT_ACCESS_POLICY_NOT_FOUND_PREFIX: &str = "Vault ";
/// The text that follows the vault ARN in that sentence.
pub const MESSAGE_VAULT_ACCESS_POLICY_NOT_FOUND_SUFFIX: &str =
    " has no associated vault policy";

/// The sentence AWS states for "the document offered as this vault's resource
/// policy is not a policy" — fixed text: it names neither the vault nor any
/// part of the document, and tells the caller what to do instead. No `Code`:
/// this condition has no observed token.
pub const MESSAGE_VAULT_ACCESS_POLICY_INVALID: &str =
    "Provided Policy is not valid. Please input a valid policy and retry the request.";

/// The sentence AWS states for "the ARN a tagging call carries names no
/// resource of this account" — fixed, and worded as the not-found conditions
/// above are: it names no part of the request, so the ARN asked about does not
/// appear in it. No `Code`: this condition has no observed token.
pub const MESSAGE_TAGGED_RESOURCE_NOT_FOUND: &str = "Cannot find resource";

/// The modelled refusal for an unknown backup job id, with its `Code`.
pub fn backup_job_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_BACKUP_JOB_NOT_FOUND.to_string()),
        message: Some(MESSAGE_BACKUP_JOB_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a vault that already exists.
///
/// `Arn` and `CreatorRequestId` are left UNSET: the shape declares them, but a
/// real response to this condition does not disclose the existing vault's ARN
/// or the token its creator sent.
pub fn vault_already_exists() -> AlreadyExistsException {
    AlreadyExistsException {
        code: Some(ERROR_CODE_VAULT_ALREADY_EXISTS.to_string()),
        message: Some(MESSAGE_VAULT_ALREADY_EXISTS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a multi-party approval call whose named vault is of
/// a type that supports none — see [`MESSAGE_UNSUPPORTED_VAULT_TYPE`] for which
/// call states it and why. No `Code`: this condition has no observed token.
///
/// A vault-type precondition whose token AWS states builds its own exception
/// instead — the source vault of a restore access create
/// ([`source_vault_unsupported`]) and the vault of a call only a logically
/// air-gapped vault answers ([`vault_type_not_air_gapped`]) — since each
/// condition carries its own token and its own sentence.
pub fn unsupported_vault_type() -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(MESSAGE_UNSUPPORTED_VAULT_TYPE.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a MEMBER of the request whose value is not one this
/// service can accept, named by the member it rejected.
///
/// ONE builder for the class, so no operation answers it with a sentence of its
/// own: AWS reports the class's own `Code`
/// ([`ERROR_CODE_INVALID_PARAMETER_VALUE`]), carries the rejected member's name
/// in `Context` — the member a client reads to learn WHICH input to fix — and
/// states a sentence whose only variable part is that same member name.
pub fn rejected_parameter(member: &str) -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_INVALID_PARAMETER_VALUE.to_string()),
        message: Some(format!("Parameter {member} is invalid.")),
        context: Some(member.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a restore access create whose source vault is of a
/// type that cannot grant restore access — see
/// [`MESSAGE_SOURCE_VAULT_UNSUPPORTED`], with the condition's own token.
pub fn source_vault_unsupported() -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_SOURCE_VAULT_UNSUPPORTED.to_string()),
        message: Some(MESSAGE_SOURCE_VAULT_UNSUPPORTED.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a call that only a logically air-gapped vault
/// answers naming a vault of another type — see
/// [`MESSAGE_VAULT_TYPE_NOT_AIR_GAPPED`], with the condition's own token.
pub fn vault_type_not_air_gapped() -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_VAULT_TYPE_INVALID.to_string()),
        message: Some(MESSAGE_VAULT_TYPE_NOT_AIR_GAPPED.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `RestoreAccessBackupVaultArn` this service cannot
/// accept — see [`MESSAGE_RESTORE_ACCESS_VAULT_ARN_INVALID`].
///
/// A value that is not a backup vault's ARN at all and one that resolves to no
/// restore access vault of the named source vault are ONE condition: the member
/// does not name a vault whose access can be revoked. Both are answered with
/// this one sentence, so the refusal discloses nothing about which vaults exist.
///
/// The PARAMETER-value class, not the not-found one: the ARN is a member of the
/// request, and AWS Backup reports a resource ARN that resolves to nothing as a
/// value it cannot accept.
pub fn rejected_restore_access_vault_arn() -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(MESSAGE_RESTORE_ACCESS_VAULT_ARN_INVALID.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a restore access create whose source vault has no
/// MPA approval team — see [`MESSAGE_NO_MPA_APPROVAL_TEAM`].
pub fn no_mpa_approval_team() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(MESSAGE_NO_MPA_APPROVAL_TEAM.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a copy job id this account does not hold, with the
/// condition's own token.
pub fn copy_job_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_COPY_JOB_NOT_FOUND.to_string()),
        message: Some(MESSAGE_COPY_JOB_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a backup vault this account does not hold, named by
/// the vault the request addressed.
///
/// ONE sentence for every call that resolves a vault it is asked to write into —
/// the backup it starts and the copy it starts — so the two cannot drift.
pub fn backup_vault_not_found(vault_name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!("Backup vault {vault_name} not found")),
        ..Default::default()
    }
}

/// The modelled refusal for a backup plan id this account does not hold, for
/// the call that resolves a plan WITHOUT reading a version of it — the update
/// that stores the next version is the one that does.
pub fn backup_plan_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_BACKUP_PLAN_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a plan READ that cannot resolve the version it
/// asks for: the plan read answers from the plan at a version — the one named,
/// or the plan's latest — so an id the account holds no plan under, a plan
/// whose deletion is recorded and a `VersionId` the plan never had are all this
/// one condition. See [`MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE`], with the
/// condition's own token.
pub fn backup_plan_version_unreadable() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_BACKUP_PLAN_VERSION_UNREADABLE.to_string()),
        message: Some(MESSAGE_BACKUP_PLAN_VERSION_UNREADABLE.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a call that has to read a plan's LATEST version
/// before it acts and finds no plan of that id to read it from — see
/// [`MESSAGE_LATEST_BACKUP_PLAN_VERSION_UNREADABLE_PREFIX`].
///
/// `plan_arn` is the ARN of the plan the call addressed ([`backup_plan_arn`] of
/// the request's region and the `BackupPlanId` it carried), which is the only
/// variable part of the sentence. No `Code`: this condition has no observed
/// token.
pub fn latest_backup_plan_version_unreadable(plan_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{MESSAGE_LATEST_BACKUP_PLAN_VERSION_UNREADABLE_PREFIX}{plan_arn}\
             {MESSAGE_LATEST_BACKUP_PLAN_VERSION_UNREADABLE_SUFFIX}"
        )),
        ..Default::default()
    }
}

/// The modelled refusal for a RULE of a backup plan body AWS Backup does not
/// accept, stated by the per-rule validation `reason`.
///
/// ONE builder for the class, so no operation that stores a plan body words it
/// differently: AWS reports the class's own `Code`
/// ([`ERROR_CODE_BACKUP_PLAN_RULE_INVALID`]), carries the per-rule reason whole
/// in `Context` — the member a client reads to learn what the rule got wrong —
/// and states that same reason after the class's fixed prefix
/// ([`MESSAGE_BACKUP_PLAN_RULES_PREFIX`]).
pub fn rejected_backup_plan_rule(reason: &str) -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_BACKUP_PLAN_RULE_INVALID.to_string()),
        message: Some(format!("{MESSAGE_BACKUP_PLAN_RULES_PREFIX}{reason}")),
        context: Some(reason.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a call that would act under an IAM role AWS Backup
/// cannot assume — see [`MESSAGE_ROLE_NOT_ASSUMABLE`], with the condition's own
/// token.
pub fn role_not_assumable() -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_ROLE_NOT_ASSUMABLE.to_string()),
        message: Some(MESSAGE_ROLE_NOT_ASSUMABLE.to_string()),
        ..Default::default()
    }
}

/// Why the `IamRoleArn` a backup selection would be stored with names no role
/// AWS Backup could assume, or `None` when nothing shows it could not.
///
/// A selection assigns resources to be backed up AS that role, so the role is
/// resolved BEFORE anything is persisted and a role this service could not
/// assume is refused ([`role_not_assumable`]) rather than stored on a selection
/// whose jobs could never run. The value is unusable in either of two ways:
///
/// * it names no IAM role of THIS account at all
///   (`::service_common::iam_role::role_name`, the tree's one reading of an IAM
///   ARN): the model types the member as an `ARN` with no pattern, so a bucket,
///   a user or another account's role reaches here, and none of those is a role
///   this account's Backup service could assume for it;
/// * this account's IAM ANSWERED that the role is one this service cannot
///   assume: it holds no role under that name, or the trust policy the role
///   carries does not grant [`SERVICE_PRINCIPAL`] `sts:AssumeRole`. That whole
///   question is the ONE reading every emulated service that resolves a
///   caller-supplied role shares
///   (`::service_common::iam_role::role_is_unassumable`), so this service states
///   only WHICH principal has to be admitted.
///
/// IAM is reached the way any emulated service reaches another: over the
/// emulator's own in-process callback, so nothing here opens a socket.
/// Everything that is not one of those two answers is accepted — an emulator
/// with no IAM mounted to ask included — because a refusal grounded in a record
/// has to be grounded in what IAM actually said. Awaited with NO store guard
/// held.
pub async fn rejected_selection_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<InvalidParameterValueException> {
    let names_a_role_of_this_account = ::service_common::iam_role::role_name(role_arn).is_some();
    let unassumable = !names_a_role_of_this_account
        || ::service_common::iam_role::role_is_unassumable(ctx, region, role_arn, SERVICE_PRINCIPAL)
            .await;
    unassumable.then(role_not_assumable)
}

/// The modelled refusal for a selection-scoped call whose `BackupPlanId` names
/// no backup plan this account holds — see
/// [`MESSAGE_BACKUP_PLAN_NOT_FOUND_BY_ID`].
///
/// The PARAMETER-value class, not the not-found one: what the call could not
/// resolve is the plan the request named, and AWS Backup tells that condition
/// apart from the selection the plan does not hold
/// ([`backup_selection_not_found`]) by both the class and the token. `code` is
/// the asking call's own, since AWS numbers this condition per call.
pub fn selection_plan_not_found(code: &str) -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(code.to_string()),
        message: Some(MESSAGE_BACKUP_PLAN_NOT_FOUND_BY_ID.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a legal hold id this account does not hold, with
/// the condition's own token.
///
/// The PARAMETER-value class, not the not-found one: what the call could not
/// resolve is the hold the request's `LegalHoldId` named, and AWS Backup reports
/// a hold id that resolves to nothing as a value it cannot accept — the same
/// reading this service already applies to a resource ARN that resolves to
/// nothing ([`rejected_restore_access_vault_arn`]). ONE refusal for the
/// condition, so every hold-scoped call answers it with one class, one token and
/// one sentence.
pub fn legal_hold_not_found() -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_LEGAL_HOLD_NOT_FOUND.to_string()),
        message: Some(MESSAGE_LEGAL_HOLD_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a cancellation of a legal hold that is not in force
/// — see [`MESSAGE_LEGAL_HOLD_NOT_ACTIVE_PREFIX`]. The hold's STATE is what
/// refuses the call, which is the condition `InvalidResourceStateException`
/// names, so `status` is the stored hold's CURRENT `LegalHoldStatus` — read
/// from state, never from the request — and it is reported twice: in `Context`,
/// which a client reads to learn which state blocked the cancel, and
/// interpolated into the sentence.
pub fn legal_hold_not_active(status: &str) -> InvalidResourceStateException {
    InvalidResourceStateException {
        code: Some(ERROR_CODE_LEGAL_HOLD_NOT_ACTIVE.to_string()),
        message: Some(format!("{MESSAGE_LEGAL_HOLD_NOT_ACTIVE_PREFIX}{status}.")),
        context: Some(status.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a legal-hold create that names no recovery-point
/// selection: `RecoveryPointSelection` carries no `@required` trait, so the
/// boundary validator passes a request without it, while a hold that selects no
/// recovery points holds nothing — AWS answers the missing-parameter refusal.
pub fn missing_recovery_point_selection() -> MissingParameterValueException {
    MissingParameterValueException {
        message: Some(MESSAGE_MISSING_RESOURCE_SELECTION.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a selection id the named plan does not hold, with
/// its `Code`.
///
/// Only that condition: a `BackupPlanId` that names no plan of this account is
/// a DIFFERENT condition, which the plan-scoped refusal states
/// ([`selection_plan_not_found`]), so a client can tell "no such selection of
/// this plan" from "no such plan".
pub fn backup_selection_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        code: Some(ERROR_CODE_BACKUP_SELECTION_NOT_FOUND.to_string()),
        message: Some(MESSAGE_BACKUP_SELECTION_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a (vault, `RecoveryPointArn`) pair that names no
/// recovery point the account holds — see [`MESSAGE_RECOVERY_POINT_NOT_FOUND`].
pub fn recovery_point_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_RECOVERY_POINT_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a call that would delete or change a recovery point
/// its vault's lock still retains — see [`MESSAGE_RECOVERY_POINT_LOCKED`].
pub fn recovery_point_locked() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(MESSAGE_RECOVERY_POINT_LOCKED.to_string()),
        ..Default::default()
    }
}

/// The SAME condition as [`recovery_point_not_found`] — the recovery point
/// named is not one the account holds — in the PARAMETER-value class, for the
/// call that carries the recovery point as a member of its own request rather
/// than as the resource it addresses.
///
/// `StartRestoreJob` is that call: the restore it starts is a resource of its
/// own, and the `RecoveryPointArn` it names is an input to it, so AWS Backup
/// reports an ARN that resolves to nothing there as a value it cannot accept —
/// the same reading as for a `RestoreAccessBackupVaultArn`
/// ([`rejected_restore_access_vault_arn`]) — while the calls that ADDRESS a
/// stored recovery point report it absent. The sentence is the condition's, so
/// it is the same one either way: a client reads which INPUT to fix from the
/// class, and what went wrong from the sentence.
pub fn recovery_point_not_found_value() -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(MESSAGE_RECOVERY_POINT_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `RestoreJobId` this account holds no restore job
/// under — see [`MESSAGE_RESTORE_JOB_NOT_FOUND`].
pub fn restore_job_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_RESTORE_JOB_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `TieringConfigurationName` this account holds no
/// tiering configuration under — see
/// [`MESSAGE_TIERING_CONFIGURATION_NOT_FOUND`].
pub fn tiering_configuration_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_TIERING_CONFIGURATION_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a tiering configuration that
/// already exists.
///
/// `Arn` and `CreatorRequestId` are left UNSET for the reason given on
/// [`vault_already_exists`].
pub fn tiering_configuration_already_exists() -> AlreadyExistsException {
    AlreadyExistsException {
        message: Some(MESSAGE_TIERING_CONFIGURATION_ALREADY_EXISTS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `ResourceArn` naming no taggable resource of this
/// account — see [`MESSAGE_TAGGED_RESOURCE_NOT_FOUND`].
pub fn tagged_resource_not_found() -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(MESSAGE_TAGGED_RESOURCE_NOT_FOUND.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a read of the resource policy of a vault that
/// carries none — see [`MESSAGE_VAULT_ACCESS_POLICY_NOT_FOUND_PREFIX`].
///
/// `vault_arn` is the stored vault's own ARN ([`BackupVault::arn`]), which is
/// the only variable part of the sentence.
pub fn vault_access_policy_not_found(vault_arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "{MESSAGE_VAULT_ACCESS_POLICY_NOT_FOUND_PREFIX}{vault_arn}\
             {MESSAGE_VAULT_ACCESS_POLICY_NOT_FOUND_SUFFIX}"
        )),
        ..Default::default()
    }
}

/// The modelled refusal for a `Policy` that is not a policy document — see
/// [`MESSAGE_VAULT_ACCESS_POLICY_INVALID`].
///
/// This condition has a sentence of its OWN rather than the member-named one
/// every other rejected value gets ([`rejected_parameter`]): AWS states what
/// the document failed to be and what to do about it, and states it the same
/// way for every document outside the policy language.
pub fn rejected_vault_access_policy() -> InvalidParameterValueException {
    InvalidParameterValueException {
        message: Some(MESSAGE_VAULT_ACCESS_POLICY_INVALID.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a call that would delete or disassociate a recovery
/// point an active legal hold is on — see [`MESSAGE_RECOVERY_POINT_HELD`].
pub fn recovery_point_held() -> InvalidRequestException {
    InvalidRequestException {
        message: Some(MESSAGE_RECOVERY_POINT_HELD.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a plan that already exists.
///
/// `Arn` and `CreatorRequestId` are left UNSET for the reason given on
/// [`vault_already_exists`].
pub fn plan_already_exists() -> AlreadyExistsException {
    AlreadyExistsException {
        message: Some(MESSAGE_PLAN_ALREADY_EXISTS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a create naming a selection the plan already holds.
///
/// `Arn` and `CreatorRequestId` are left UNSET for the reason given on
/// [`vault_already_exists`].
pub fn selection_already_exists() -> AlreadyExistsException {
    AlreadyExistsException {
        message: Some(MESSAGE_SELECTION_ALREADY_EXISTS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a framework name this account does not hold, worded
/// by [`framework_not_found_message`] from the name the call addressed.
pub fn framework_not_found(name: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(framework_not_found_message(name)),
        ..Default::default()
    }
}

/// The modelled refusal for a create this account already holds the framework
/// for, worded by [`framework_already_exists_message`] from the name the
/// request carried.
///
/// `Arn` and `CreatorRequestId` are left UNSET for the reason given on
/// [`vault_already_exists`].
pub fn framework_already_exists(name: &str) -> AlreadyExistsException {
    AlreadyExistsException {
        message: Some(framework_already_exists_message(name)),
        ..Default::default()
    }
}

/// The modelled refusal for an action on a framework whose previous action has
/// not finished — see [`framework_action_in_progress_message`]. AWS Backup
/// serializes the deployment of one framework, so the action that loses the race
/// is refused rather than blended into the one already running.
pub fn framework_action_in_progress(name: &str) -> ConflictException {
    ConflictException {
        message: Some(framework_action_in_progress_message(name)),
        ..Default::default()
    }
}

/// The control names AWS Backup Audit Manager supports — the `ControlName`
/// identifiers its published control catalog names, which are the only values
/// `FrameworkControls` may carry. The model types `ControlName` as a plain
/// string ("The name of a control. This name is between 1 and 256 characters",
/// `smithy_query` shape `FrameworkControl`, member `ControlName`), so the
/// catalog is listed here: AWS refuses any other value rather than storing a
/// framework that evaluates a control which does not exist.
///
/// Source: the AWS Backup Developer Guide's "Choosing controls" page of the
/// Backup Audit Manager chapter, which lists each control and its identifier.
/// AWS adds controls over time, so re-read that page against this list when a
/// control it names is refused here.
pub const SUPPORTED_FRAMEWORK_CONTROL_NAMES: [&str; 10] = [
    "BACKUP_RESOURCES_PROTECTED_BY_BACKUP_PLAN",
    "BACKUP_PLAN_MIN_FREQUENCY_AND_MIN_RETENTION_CHECK",
    "BACKUP_RECOVERY_POINT_MINIMUM_RETENTION_CHECK",
    "BACKUP_RECOVERY_POINT_ENCRYPTED",
    "BACKUP_RECOVERY_POINT_MANUAL_DELETION_DISABLED",
    "BACKUP_RESOURCES_PROTECTED_BY_BACKUP_VAULT_LOCK",
    "BACKUP_LAST_RECOVERY_POINT_CREATED",
    "BACKUP_RESOURCES_PROTECTED_BY_CROSS_REGION_COPY",
    "BACKUP_RESOURCES_PROTECTED_BY_CROSS_ACCOUNT_COPY",
    "RESTORE_TIME_FOR_RESOURCES_MEET_TARGET",
];

/// Whether `name` is one of the controls Backup Audit Manager supports
/// ([`SUPPORTED_FRAMEWORK_CONTROL_NAMES`]). The comparison is exact: a control
/// is addressed by its identifier, so a differently-spelled one names no
/// control at all.
pub fn is_supported_framework_control(name: &str) -> bool {
    SUPPORTED_FRAMEWORK_CONTROL_NAMES.contains(&name)
}

/// The modelled refusal for a `FrameworkControls` list carrying a value AWS does
/// not accept, or `None` when every control in it is one it does.
///
/// ONE check for every operation that takes a framework's controls — the create
/// and the update carry the same shape — so the two cannot drift in rules or in
/// wording. Each rule is the model's own or the service's published catalog: a
/// control must be one Backup Audit Manager supports
/// ([`SUPPORTED_FRAMEWORK_CONTROL_NAMES`]), "Each framework must contain at
/// least one control" (`FrameworkControl` doc), a control is named by its
/// `ControlName` so no two controls of one framework may share one, and a
/// control scope's tag selector takes "a maximum of one key-value pair"
/// ([`MAX_CONTROL_SCOPE_TAGS`], `ControlScope$Tags` doc).
///
/// The size of the list is NOT checked here: a list past
/// [`MAX_CONTROLS_PER_FRAMEWORK`] is a quota condition, which AWS answers with a
/// different modelled exception than a value it refuses, so every caller resolves
/// that quota BEFORE asking this — which is also what keeps the walk below sized
/// by the quota rather than by the request.
pub fn rejected_framework_controls(
    controls: &[FrameworkControl],
) -> Option<InvalidParameterValueException> {
    let message = framework_controls_refusal(controls)?;
    Some(InvalidParameterValueException {
        message: Some(message),
        ..Default::default()
    })
}

/// Why `controls` is not a framework's control list, or `None` when it is.
///
/// ONE pass over the list: the names already seen are carried in a set, so the
/// cost is linear in the number of controls rather than quadratic, whatever the
/// quota the callers resolve first admits.
fn framework_controls_refusal(controls: &[FrameworkControl]) -> Option<String> {
    if controls.is_empty() {
        return Some("FrameworkControls must contain at least one control".to_string());
    }
    let mut named = ::std::collections::BTreeSet::new();
    for control in controls {
        // The control the entry names before anything else about the entry: a
        // name no control in the catalog carries cannot be stored, whatever
        // parameters or scope accompany it.
        if !is_supported_framework_control(&control.control_name) {
            return Some(unsupported_framework_control_message(&control.control_name));
        }
        if !named.insert(control.control_name.as_str()) {
            return Some(format!(
                "ControlName {} is used by more than one control; control names must be \
                 unique within a framework",
                control.control_name
            ));
        }
        let scope_tags = control
            .control_scope
            .as_ref()
            .and_then(|scope| scope.tags.as_ref())
            .map_or(0, ::std::collections::HashMap::len);
        if scope_tags > MAX_CONTROL_SCOPE_TAGS {
            return Some(format!(
                "ControlScope of control {} carries {scope_tags} Tags entries; at most \
                 {MAX_CONTROL_SCOPE_TAGS} key-value pair can be provided",
                control.control_name
            ));
        }
    }
    None
}

/// The modelled refusal for a backup window outside the span the model
/// documents for it, or `None` when the caller's value is one it admits
/// (including an absent one).
///
/// ONE check for every window this service takes — a backup job's own windows
/// and the windows of a backup plan's rules — so the two cannot drift in bounds
/// or in wording. `minimum` is the member's own documented floor
/// ([`MIN_START_WINDOW_MINUTES`] for a start window); the ceiling is
/// [`MAX_WINDOW_MINUTES`], which the model documents for both.
pub fn rejected_window_minutes(
    member: &str,
    minutes: Option<i64>,
    minimum: i64,
) -> Option<InvalidParameterValueException> {
    let value = minutes?;
    if (minimum..=MAX_WINDOW_MINUTES).contains(&value) {
        return None;
    }
    Some(InvalidParameterValueException {
        message: Some(format!(
            "{member} must be between {minimum} and {MAX_WINDOW_MINUTES} minutes"
        )),
        ..Default::default()
    })
}

/// The modelled refusal for a member the model types as `ARN` with no pattern
/// whose value is not one, or `None` when it is.
///
/// ONE check for every such member in this service, for the reason given on
/// [`rejected_window_minutes`]; see [`is_arn`] for what being one means. The
/// refusal itself is the class's own ([`rejected_parameter`]), so the member
/// whose value was rejected is the only thing that varies between two of them.
pub fn rejected_arn(member: &str, value: &str) -> Option<InvalidParameterValueException> {
    if is_arn(value) {
        return None;
    }
    Some(rejected_parameter(member))
}

/// Why a `BackupPlan` body is not one AWS stores, as
/// [`rejected_backup_plan_body`] decides it.
///
/// The two refusals are kept apart because the operations that store a plan
/// body do not model the same errors for them: one states a value AWS does not
/// accept, the other that the body is past a limit, and each operation answers
/// the latter with the error ITS model declares.
#[derive(Debug)]
pub enum RejectedBackupPlanBody {
    /// A member of the body carries a value AWS does not accept, as this
    /// modelled refusal states.
    Value(InvalidParameterValueException),
    /// The body carries more rules than a backup plan may hold; the sentence
    /// states the limit.
    TooManyRules(String),
}

/// Whether `plan` is a backup plan body this service stores, and why not when
/// it is not.
///
/// The ONE check every operation that stores a plan body applies \u2014 the create
/// and the update \u2014 so the two cannot drift in what they accept. Modelled
/// constraints are already refused before a handler runs, so what is left is
/// what the model admits and AWS does not:
///
/// * a plan "includes a BackupPlanName and one or more sets of Rules" (model
///   doc for the member), so a rule list the model admits as present but empty
///   schedules nothing;
/// * the rule list is sized by the request, so it is capped before the
///   per-rule checks below walk it and before any of it is stored;
/// * a rule is identified within its plan by its name, so two rules of one
///   plan cannot share one;
/// * both backup windows carry a documented range ([`rejected_window_minutes`]);
/// * every rule's `ScheduleExpression` is an expression jobs can be initiated
///   from ([`schedule::rule_schedule_refusal`]), rather than a rule being
///   stored whose schedule nothing resolves;
/// * the members the model types as `ARN` with no pattern carry one
///   ([`rejected_arn`]), rather than a rule being stored whose destination no
///   ARN resolves to.
pub fn rejected_backup_plan_body(plan: &BackupPlanInput) -> Option<RejectedBackupPlanBody> {
    if plan.rules.is_empty() {
        return Some(RejectedBackupPlanBody::Value(InvalidParameterValueException {
            message: Some("Rules must contain at least one backup rule".to_string()),
            ..Default::default()
        }));
    }
    if plan.rules.len() > MAX_RULES_PER_PLAN {
        return Some(RejectedBackupPlanBody::TooManyRules(format!(
            "a backup plan may hold at most {MAX_RULES_PER_PLAN} rules"
        )));
    }
    for (index, rule) in plan.rules.iter().enumerate() {
        if plan.rules[..index]
            .iter()
            .any(|earlier| earlier.rule_name == rule.rule_name)
        {
            return Some(RejectedBackupPlanBody::Value(InvalidParameterValueException {
                message: Some(format!(
                    "RuleName {} is used by more than one rule; rule names must be \
                     unique within a backup plan",
                    rule.rule_name
                )),
                ..Default::default()
            }));
        }
        for (member, minutes, minimum) in [
            (
                "StartWindowMinutes",
                rule.start_window_minutes,
                MIN_START_WINDOW_MINUTES,
            ),
            ("CompletionWindowMinutes", rule.completion_window_minutes, 0),
        ] {
            if let Some(refusal) = rejected_window_minutes(member, minutes, minimum) {
                return Some(RejectedBackupPlanBody::Value(refusal));
            }
        }
        // A rule initiates its jobs from its `ScheduleExpression`, so a rule
        // whose expression is not one this service resolves to a schedule
        // schedules nothing: it is refused with the per-rule validation
        // envelope, which states the reason this rule failed.
        if let Some(reason) = schedule::rule_schedule_refusal(rule) {
            return Some(RejectedBackupPlanBody::Value(rejected_backup_plan_rule(
                &reason,
            )));
        }
        let arns = rule
            .target_logically_air_gapped_backup_vault_arn
            .as_deref()
            .map(|arn| ("TargetLogicallyAirGappedBackupVaultArn", arn))
            .into_iter()
            .chain(rule.copy_actions.iter().flatten().map(|copy| {
                (
                    "DestinationBackupVaultArn",
                    copy.destination_backup_vault_arn.as_str(),
                )
            }));
        for (member, arn) in arns {
            if let Some(refusal) = rejected_arn(member, arn) {
                return Some(RejectedBackupPlanBody::Value(refusal));
            }
        }
    }
    None
}

/// The modelled refusal for a member that names an MPA APPROVAL TEAM by ARN
/// whose value is not the ARN of a team at all, or `None` when it has the form
/// of one.
///
/// A sibling of [`rejected_backup_vault_arn`] for the same narrower rule: the
/// model types the member as `ARN` with no pattern, but only one resource of one
/// service can be named by it, so a value that is the ARN of something else is a
/// parameter value AWS refuses rather than a team it looks for. See
/// [`mpa_approval_team_arn_parts`] for the form, which is the owning service's.
///
/// Whether a team of that form EXISTS is not decided here: that is the owning
/// service's to answer, so the operation asks it (see
/// `crate::ops::associate_backup_vault_mpa_approval_team`) and answers this same
/// refusal when the answer is that it does not.
pub fn rejected_mpa_approval_team_arn(
    member: &str,
    value: &str,
) -> Option<InvalidParameterValueException> {
    mpa_approval_team_arn_parts(value)
        .is_none()
        .then(|| rejected_parameter(member))
}

/// The modelled refusal for a member that names a BACKUP VAULT by ARN whose
/// value is not a backup vault's ARN, or `None` when it is.
///
/// A sibling of [`rejected_arn`] for the narrower rule such a member carries: the
/// model types it as `ARN` with no pattern, but only one resource of one service
/// can be named by it, so a value that is an ARN of something else is a parameter
/// value AWS refuses rather than a resource it looks for. See
/// [`backup_vault_arn_parts`] for the form.
pub fn rejected_backup_vault_arn(
    member: &str,
    value: &str,
) -> Option<InvalidParameterValueException> {
    resolved_backup_vault_arn(member, value).err()
}

/// The fields a backup-vault ARN names, or the modelled refusal for a value
/// that is not one.
///
/// The same check and the same refusal as [`rejected_backup_vault_arn`] — that
/// one is this one's refusal half — for a caller that goes on to USE the parsed
/// fields, so the parse and the refusal cannot drift apart and neither needs a
/// second parse that could fail.
pub fn resolved_backup_vault_arn(
    member: &str,
    value: &str,
) -> Result<BackupVaultArnParts, InvalidParameterValueException> {
    backup_vault_arn_parts(value).ok_or_else(|| rejected_parameter(member))
}

/// The modelled refusal for a `MaxResults` outside
/// [`MAX_RESULTS_MIN`]`..=`[`MAX_RESULTS_MAX`], or `None` when the caller's
/// page size is one the model admits (including an absent one).
///
/// The bound is the model's own, so an operation calls this instead of spelling
/// the range again. The check belongs to the OPERATION rather than to the
/// framework's boundary validator: AWS answers it with this modelled exception,
/// and only an exception the operation returns is serialized as the shape the
/// model declares — the body a client reads `Code` and `Message` out of.
pub fn rejected_max_results(max_results: Option<i32>) -> Option<InvalidParameterValueException> {
    let value = max_results?;
    if (MAX_RESULTS_MIN..=MAX_RESULTS_MAX).contains(&value) {
        return None;
    }
    Some(InvalidParameterValueException {
        code: Some(ERROR_CODE_MAX_RESULTS_INVALID.to_string()),
        message: Some(MESSAGE_MAX_RESULTS_INVALID.to_string()),
        ..Default::default()
    })
}

/// The modelled refusal for a `RecoveryPointSelection` AWS does not accept, or
/// `None` when every member of the selection is one it does.
///
/// ONE check for the selection in this service, for the reason given on
/// [`rejected_window_minutes`]: the members target unpatterned `string`
/// shapes, so the framework's boundary validator passes anything, while each one
/// still has a form it must take —
///
/// * `VaultNames` names backup vaults, but the model types its entries as
///   plain `string` with neither a pattern nor a length, so NO vault-name rule
///   is applied to them: a client narrows a hold to a set of vaults by naming
///   them and widens it to every vault with [`VAULT_NAMES_WILDCARD`], and both
///   are values AWS accepts;
/// * `ResourceIdentifiers` names protected resources, which are addressed by ARN
///   ([`rejected_arn`]);
///
/// and a member whose value is not of that form is refused as the VALUE of that
/// member ([`rejected_parameter`]), like every other such member of this
/// service. A `DateRange` bound after `now` is a different condition — not a
/// malformed value but one that selects recovery points which cannot exist:
/// "Both values are required. Future DateTime values are not permitted" (the
/// model doc for `DateRange`).
pub fn rejected_recovery_point_selection(
    selection: &RecoveryPointSelection,
    now: SystemTime,
) -> Option<InvalidParameterValueException> {
    for identifier in selection.resource_identifiers.iter().flatten() {
        if let Some(refusal) = rejected_arn("ResourceIdentifiers", identifier) {
            return Some(refusal);
        }
    }
    let range = selection.date_range.as_ref()?;
    for (member, at) in [("FromDate", range.from_date), ("ToDate", range.to_date)] {
        let in_future = SystemTime::try_from(at).is_ok_and(|at| at > now);
        if in_future {
            return Some(InvalidParameterValueException {
                message: Some(format!(
                    "RecoveryPointSelection DateRange {member} is in the future, and future \
                     DateTime values are not permitted"
                )),
                ..Default::default()
            });
        }
    }
    None
}

/// The modelled refusal for an `AggregationPeriod` the model's enum does not
/// list, or `None` when the caller named one it does (including none at all).
///
/// The message is BUILT FROM THE REQUEST: it names the rejected value and then
/// the values the model admits, so a caller can fix the request from the
/// response alone. No `Code` is set — this condition has no observed token.
pub fn rejected_aggregation_period(
    aggregation_period: Option<&str>,
) -> Option<InvalidParameterValueException> {
    let value = aggregation_period?;
    if AGGREGATION_PERIODS.contains(&value) {
        return None;
    }
    Some(InvalidParameterValueException {
        message: Some(format!(
            "Invalid input aggregation id {value}. Input aggregation period must be within {}",
            AGGREGATION_PERIODS.join(", ")
        )),
        ..Default::default()
    })
}

/// The Backup resource type of a protected resource, derived from its ARN.
///
/// AWS Backup names the protected resource's type after the service that owns
/// the ARN; the catalog below is the subset of the resource types the Backup
/// API documents (`ListBackupJobs` `ByResourceType`) that the model's
/// `ResourceType` pattern admits — the pattern forbids the space in the
/// documented `Storage Gateway` / `SAP HANA on Amazon EC2` spellings, so those
/// namespaces fall through to the rule below. A namespace the catalog does not
/// name is reported as the namespace itself, which is what the ARN says about
/// the resource; anything the pattern refuses is reported as absent.
pub fn resource_type_of(resource_arn: &str) -> Option<String> {
    let mut fields = resource_arn.split(':');
    let (Some("arn"), Some(_partition), Some(service)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    // Skip region and account; what is left is the resource portion, which may
    // itself contain `:`.
    let resource = fields.skip(2).collect::<Vec<_>>().join(":");
    let resource_type = match service {
        "dynamodb" => "DynamoDB",
        "elasticfilesystem" => "EFS",
        "fsx" => "FSx",
        "s3" => "S3",
        "neptune" => "Neptune",
        "docdb" => "DocumentDB",
        "redshift" => "Redshift",
        "timestream" => "Timestream",
        "cloudformation" => "CloudFormation",
        // One namespace, two Backup resource types: an RDS cluster is Aurora,
        // a single instance is RDS.
        "rds" => {
            if resource.starts_with("cluster") {
                "Aurora"
            } else {
                "RDS"
            }
        }
        // Likewise: an EC2 volume is backed up as EBS, an instance as EC2.
        "ec2" => {
            if resource.starts_with("volume/") {
                "EBS"
            } else {
                "EC2"
            }
        }
        other => other,
    };
    is_modelled_resource_type(resource_type).then(|| resource_type.to_string())
}

/// The Backup Vault Lock configuration applied to a vault by
/// `PutBackupVaultLockConfiguration`.
///
/// Its PRESENCE is what the vault read projections report as `Locked`, so a
/// vault can never report retention bounds it is not locked by, nor a lock with
/// no bounds behind it.
#[derive(Debug, Clone)]
pub struct VaultLock {
    pub min_retention_days: Option<i64>,
    pub max_retention_days: Option<i64>,
    /// When the configuration becomes immutable — set only when the request
    /// named `ChangeableForDays`, since the model documents that "if this
    /// parameter is not specified, you can delete Vault Lock from the vault
    /// [...] or change the Vault Lock configuration [...] at any time".
    pub lock_date: Option<SystemTime>,
}

impl VaultLock {
    /// Whether this configuration can no longer be changed or deleted: the
    /// model documents that "on and after the lock date, the Vault Lock becomes
    /// immutable and cannot be changed or deleted".
    pub fn is_immutable_at(&self, now: SystemTime) -> bool {
        self.lock_date.is_some_and(|lock_date| now >= lock_date)
    }
}

/// The resource types this service supports, which is what
/// `GetSupportedResourceTypes` reports and what every other operation that
/// names a `ResourceType` is measured against.
///
/// SOURCE, for all but the last two entries: the model's own documentation for
/// the member being reported —
/// `smithy_query(query="doc", shape="GetSupportedResourceTypesOutput",
/// member="ResourceTypes", model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")`
/// — which enumerates each supported type with the service it stands for.
///
/// The last two, `DSQL` (Aurora DSQL) and `Redshift Serverless`, are services
/// AWS Backup supports whose types that member's documentation predates; AWS
/// reports them in the list today. Membership is Region- and era-dependent at
/// AWS — a Region that has a newer service adds its type — so the catalog is a
/// set this emulator OWNS: it is what this emulator supports, and an operation
/// that answers for a resource type reads it from here rather than carrying a
/// list of its own.
///
/// Three of the spellings (`Storage Gateway`, `SAP HANA on Amazon EC2`,
/// `Redshift Serverless`) contain spaces, which the `ResourceType` shape's own
/// `@pattern` (`^[a-zA-Z0-9\-\_\.]{1,50}$`) refuses. That pattern governs what
/// a CALLER may send — AWS reports these spellings and refuses them as input
/// just the same — so they are reported here as AWS spells them. See
/// `is_modelled_resource_type`, which is the other side of the same line: a
/// type this service DERIVES for a stored resource is only reported in a form
/// the pattern admits.
pub const SUPPORTED_RESOURCE_TYPES: &[&str] = &[
    "Aurora",
    "CloudFormation",
    "DSQL",
    "DocumentDB",
    "DynamoDB",
    "EBS",
    "EC2",
    "EFS",
    "EKS",
    "FSx",
    "Neptune",
    "RDS",
    "Redshift",
    "Redshift Serverless",
    "S3",
    "SAP HANA on Amazon EC2",
    "Storage Gateway",
    "Timestream",
    "VirtualMachine",
];

/// A backup vault: the logical container `CreateBackupVault` inserts.
#[derive(Debug, Clone)]
pub struct BackupVault {
    pub name: String,
    /// The region this vault was created in, the other half of the key it is
    /// stored under ([`vault_key`]): a backup vault's name identifies it within
    /// one account AND one region, so the same name in another region is
    /// another vault.
    pub region: String,
    pub arn: String,
    /// The KMS key this vault is encrypted with. ALWAYS set: the model
    /// documents that "if you do not specify a key, Backup creates a KMS key
    /// for you by default", so a vault always has one to report.
    pub encryption_key_arn: String,
    /// Whether [`Self::encryption_key_arn`] is the key the create request
    /// named, rather than the one this service created for the account by
    /// default ([`ComAmazonawsBackupCryocontrollerusermanagerState::default_encryption_key_arn`]).
    pub customer_managed_key: bool,
    /// `EncryptionKeyType`, as this vault's type reports it. `None` for a
    /// standard `BACKUP_VAULT`: AWS leaves the member unset for one, and a read
    /// projects what is stored rather than defaulting an enum nobody set.
    pub encryption_key_type: Option<String>,
    pub creator_request_id: Option<String>,
    pub creation_date: SystemTime,
    /// [`VAULT_TYPE_BACKUP_VAULT`] for a vault made by `CreateBackupVault`,
    /// [`VAULT_TYPE_LOGICALLY_AIR_GAPPED_BACKUP_VAULT`] for one made by
    /// `CreateLogicallyAirGappedBackupVault` and
    /// [`VAULT_TYPE_RESTORE_ACCESS_BACKUP_VAULT`] for one made by
    /// `CreateRestoreAccessBackupVault`.
    pub vault_type: String,
    /// `VaultState`, as this vault's type reports it. `None` for a standard
    /// `BACKUP_VAULT`, for the reason given on [`Self::encryption_key_type`];
    /// the vault types whose create models the member do carry one.
    pub vault_state: Option<String>,
    /// When the clock owes this vault its `CREATING` -> `AVAILABLE`
    /// transition, for a vault whose create reports a non-terminal state. See
    /// [`VAULT_SETTLE_AFTER`]. `None` for a vault created ready.
    pub settles_at: Option<SystemTime>,
    pub tags: BTreeMap<String, String>,
    /// The vault-lock configuration, once one has been applied.
    pub lock: Option<VaultLock>,
    /// The ARN of the vault a [`VAULT_TYPE_RESTORE_ACCESS_BACKUP_VAULT`] vault
    /// was created against: the source vault whose recovery points it gives
    /// temporary access to. `None` for every other vault type, which has no
    /// source vault, and the link `ListRestoreAccessBackupVaults` pages over.
    pub source_backup_vault_arn: Option<String>,
    /// The ARN of the MPA approval team `AssociateBackupVaultMpaApprovalTeam`
    /// associated with this vault, once one has been. Multi-party approval is a
    /// facility of a logically air-gapped vault, and the team it names is what
    /// approves access to the recovery points that vault holds, so a source
    /// vault with none can grant no restore access at all.
    pub mpa_approval_team_arn: Option<String>,
    /// When the access a restore access vault asks for was approved, which is
    /// the instant its creation settled: such a vault is created "subject to
    /// MPA approval" (model doc for `CreateRestoreAccessBackupVault`), so it is
    /// usable exactly once that approval is in. `None` while one is still
    /// [`VAULT_STATE_CREATING`], and for every vault type that asks for no
    /// approval at all.
    pub approval_date: Option<SystemTime>,
    /// The resource policy `PutBackupVaultAccessPolicy` attached to this vault,
    /// once one has been, in the one layout an attached policy document is
    /// reported in (`::service_common::iam_policy::canonical_document`). `None`
    /// for a vault carrying none, which is what the policy read reports absent.
    pub access_policy: Option<String>,
}

impl BackupVault {
    /// The `EncryptionKeyArn` the create request NAMED, as opposed to the key
    /// this service created for a request that named none — what a repeat
    /// create is compared against to decide whether it replays the original.
    pub fn named_encryption_key_arn(&self) -> Option<&str> {
        self.customer_managed_key
            .then_some(self.encryption_key_arn.as_str())
    }
}

/// The members a vault read projects, computed ONCE from the stored vault so
/// `DescribeBackupVault` and `ListBackupVaults` can never disagree about the
/// same vault. A member the stored vault does not carry stays `None` here, and
/// so is absent from both responses.
#[derive(Debug, Clone)]
pub struct VaultView {
    pub name: String,
    pub arn: String,
    pub vault_type: String,
    pub vault_state: Option<String>,
    pub encryption_key_arn: String,
    pub encryption_key_type: Option<String>,
    pub creation_date: SystemTime,
    pub creator_request_id: Option<String>,
    pub number_of_recovery_points: i64,
    pub locked: bool,
    pub min_retention_days: Option<i64>,
    pub max_retention_days: Option<i64>,
    pub lock_date: Option<SystemTime>,
    pub source_backup_vault_arn: Option<String>,
    pub approval_date: Option<SystemTime>,
    pub mpa_approval_team_arn: Option<String>,
}

/// A recovery point stored inside a vault by `StartBackupJob`.
#[derive(Debug, Clone)]
pub struct RecoveryPoint {
    pub arn: String,
    pub backup_vault_name: String,
    pub backup_vault_arn: String,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub status: String,
    pub creation_date: SystemTime,
    /// When the clock owes this recovery point its `CREATING` -> `COMPLETED`
    /// transition. See [`JOB_SETTLE_AFTER`].
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    pub backup_size_in_bytes: i64,
    pub lifecycle: Option<Lifecycle>,
    pub encryption_key_arn: Option<String>,
    pub is_encrypted: bool,
    pub is_parent: bool,
    pub tags: BTreeMap<String, String>,
    /// `IndexStatus`, set only when `StartBackupJob` asked for an index
    /// (`Index: ENABLED`); it settles from `PENDING` to `ACTIVE` alongside the
    /// job that built it.
    pub index_status: Option<String>,
    /// The id of the job that created it: the `StartBackupJob` that backed the
    /// protected resource up, or the `StartCopyJob` that copied it into this
    /// vault.
    pub created_by_job_id: String,
}

impl RecoveryPoint {
    /// The instants the stored `Lifecycle` makes due, as
    /// `CalculatedLifecycle` reports them: the day counts the caller gave
    /// counted forward from this recovery point's creation.
    pub fn calculated_lifecycle(&self) -> Option<(Option<SystemTime>, Option<SystemTime>)> {
        let lifecycle = self.lifecycle.as_ref()?;
        let after = |days: Option<i64>| -> Option<SystemTime> {
            // `-1` is the documented sentinel for "keep the recovery point
            // indefinitely" (model doc for `Lifecycle`), so it makes nothing
            // due; and a day count too large to express as an instant has
            // nothing due either. Both answer `None` rather than overflowing.
            let days = u64::try_from(days?).ok()?;
            let seconds = days.checked_mul(SECONDS_PER_DAY)?;
            self.creation_date.checked_add(Duration::from_secs(seconds))
        };
        Some((
            after(lifecycle.move_to_cold_storage_after_days),
            after(lifecycle.delete_after_days),
        ))
    }

    /// The `EncryptionKeyType` this recovery point reports, derived from the key
    /// it is stored under.
    pub fn encryption_key_type(&self) -> &'static str {
        if self.encryption_key_arn.is_some() {
            ENCRYPTION_KEY_TYPE_CUSTOMER_MANAGED
        } else {
            ENCRYPTION_KEY_TYPE_AWS_OWNED
        }
    }

    /// Which storage class this recovery point is in at `now`, derived from the
    /// stored lifecycle: "The lifecycle defines when a protected resource is
    /// transitioned to cold storage" (model doc for `Lifecycle`), so a point is
    /// in warm storage until the transition its lifecycle makes due has come and
    /// in cold storage from that instant — the SAME instant
    /// [`Self::calculated_lifecycle`] reports as `MoveToColdStorageAt`, so the
    /// two can never disagree. A point whose lifecycle asks for no transition
    /// stays warm.
    ///
    /// `DELETED` is not reachable from a read: a deleted recovery point is no
    /// longer stored, so nothing can project it.
    pub fn storage_class(&self, now: SystemTime) -> &'static str {
        let moved = self
            .calculated_lifecycle()
            .and_then(|(move_to_cold_storage_at, _)| move_to_cold_storage_at)
            .is_some_and(|due| now >= due);
        if moved {
            STORAGE_CLASS_COLD
        } else {
            STORAGE_CLASS_WARM
        }
    }
}

/// The members a recovery-point read projects, computed ONCE from the stored
/// point and the vault holding it, so `DescribeRecoveryPoint` and
/// `ListRecoveryPointsByBackupVault` can never disagree about the same point. A
/// member the stored point does not carry stays `None` here, and so is absent
/// from both responses.
#[derive(Debug, Clone)]
pub struct RecoveryPointView {
    pub arn: String,
    pub backup_vault_name: String,
    pub backup_vault_arn: String,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub status: String,
    pub creation_date: SystemTime,
    pub completion_date: Option<SystemTime>,
    pub backup_size_in_bytes: i64,
    pub lifecycle: Option<Lifecycle>,
    pub calculated_lifecycle: Option<CalculatedLifecycle>,
    pub encryption_key_arn: Option<String>,
    pub encryption_key_type: String,
    pub is_encrypted: bool,
    pub is_parent: bool,
    /// The type of the vault holding the point, read off that vault rather than
    /// stored on the point; `None` when the account no longer holds it.
    pub vault_type: Option<String>,
    pub index_status: Option<String>,
    /// Which storage class the point is in, as its lifecycle decides — see
    /// [`RecoveryPoint::storage_class`].
    pub storage_class: String,
}

/// A backup job: what `StartBackupJob` starts and `DescribeBackupJob` reports.
#[derive(Debug, Clone)]
pub struct BackupJob {
    pub job_id: String,
    pub backup_vault_name: String,
    pub backup_vault_arn: String,
    pub recovery_point_arn: String,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub state: String,
    pub status_message: Option<String>,
    /// The class a failed job's `StatusMessage` belongs to, recorded when the job
    /// settles `FAILED` for the same reason that message is: `None` for a job that
    /// has not failed, so neither can be reported without the other.
    pub message_category: Option<String>,
    /// Why this job can create no recovery point, decided before the job was
    /// stored ([`job_outcome`]).
    ///
    /// Its PRESENCE is what makes the terminal state the clock owes this job
    /// [`BACKUP_JOB_STATE_FAILED`] rather than [`BACKUP_JOB_STATE_COMPLETED`], and
    /// what it carries is the `StatusMessage` and the `MessageCategory` that state
    /// reports — so a job can never report a failure with no reason, nor a reason it
    /// did not fail for. It is also why no recovery point was stored for the job at
    /// all. `None` for a job nothing shows cannot be performed.
    pub failure: Option<BackupFailure>,
    pub percent_done: String,
    /// The emulator copies no bytes, so a job it ran moved nothing: this stays
    /// 0 rather than reporting a size no backup produced.
    pub backup_size_in_bytes: i64,
    pub bytes_transferred: i64,
    pub creation_date: SystemTime,
    /// When the clock owes this job its `CREATED` -> `COMPLETED` transition.
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    /// The deadline `StartWindowMinutes` sets, when the caller gave one.
    pub start_by: Option<SystemTime>,
    pub lifecycle: Option<Lifecycle>,
    pub backup_options: BTreeMap<String, String>,
    pub encryption_key_arn: Option<String>,
    pub is_encrypted: bool,
    pub is_parent: bool,
    pub idempotency_token: Option<String>,
}

/// What a recorded `IdempotencyToken` replays, and when it was recorded so it
/// can be forgotten after [`IDEMPOTENCY_TOKEN_TTL`].
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub job_id: String,
    pub recorded_at: SystemTime,
}

/// A copy job: what `StartCopyJob` starts and `DescribeCopyJob` reports.
///
/// A copy job is the job that puts a COPY of one vault's recovery point into
/// another vault, so it records both ends: the recovery point it reads and the
/// one it writes ([`Self::destination_recovery_point_arn`], a
/// [`RecoveryPoint`] stored in the destination vault), which is why a read of
/// the destination vault reports the copy and the copy job agrees with it.
#[derive(Debug, Clone)]
pub struct CopyJobRecord {
    pub copy_job_id: String,
    pub source_backup_vault_name: String,
    pub source_backup_vault_arn: String,
    pub source_recovery_point_arn: String,
    /// The destination vault's `BackupVaultName`, the key its record is stored
    /// under.
    pub destination_backup_vault_name: String,
    /// The destination vault's ARN as the request named it, which carries the
    /// destination region — not this request's — for a copy across regions.
    pub destination_backup_vault_arn: String,
    pub destination_recovery_point_arn: String,
    /// The key the copy in the destination vault is stored under, when that
    /// vault was created with one of its own; see [`BackupVault::named_encryption_key_arn`].
    pub destination_encryption_key_arn: Option<String>,
    /// The lifecycle the request asked the COPY to be kept under; the source
    /// recovery point keeps its own.
    pub destination_recovery_point_lifecycle: Option<Lifecycle>,
    pub resource_arn: String,
    pub resource_type: Option<String>,
    pub iam_role_arn: String,
    pub state: String,
    pub status_message: Option<String>,
    /// The emulator copies no bytes, so a copy job it ran moved nothing: this
    /// stays 0 rather than reporting a size no copy produced.
    pub backup_size_in_bytes: i64,
    pub creation_date: SystemTime,
    /// When the clock owes this copy job its `CREATED` -> `COMPLETED`
    /// transition. See [`JOB_SETTLE_AFTER`].
    pub settles_at: SystemTime,
    pub completion_date: Option<SystemTime>,
    pub is_parent: bool,
    pub idempotency_token: Option<String>,
}

/// ONE version of a backup plan: the body a create or an update was given,
/// plus the `VersionId` this service minted for it and the instant it was
/// stored.
///
/// A plan is identified by the ID this service mints, never by its body, so the
/// body is kept verbatim (with a `RuleId` added to each rule, which only the
/// service can mint) and every read projects it through [`Self::backup_plan`]
/// \u2014 one projection, so two readers of a version can never report two
/// different bodies.
#[derive(Debug, Clone)]
pub struct BackupPlanVersion {
    /// This version's id: "Unique, randomly generated, Unicode, UTF-8 encoded
    /// strings that are at most 1,024 bytes long. Version Ids cannot be edited"
    /// (model doc for `VersionId`), so a version keeps the id it was stored
    /// under and an update mints another for the version it adds.
    pub version_id: String,
    /// The plan's display name as this version carries it: a plan's body
    /// "includes a BackupPlanName and one or more sets of Rules" (model doc for
    /// `BackupPlanInput`), so a version states the name the plan had while it
    /// was the current one.
    pub name: String,
    /// The version's rules in the order the caller gave them, each carrying the
    /// `RuleId` this service minted for it.
    pub rules: Vec<BackupRule>,
    pub advanced_backup_settings: Vec<AdvancedBackupSetting>,
    pub scan_settings: Vec<ScanSetting>,
    /// When this version was stored: the instant the create or the update that
    /// made it was served.
    pub creation_date: SystemTime,
}

impl BackupPlanVersion {
    /// The plan body a read of this version reports. The ONE place a stored
    /// plan's wire body is computed.
    pub fn backup_plan(&self) -> BackupPlan {
        BackupPlan {
            backup_plan_name: self.name.clone(),
            rules: self.rules.clone(),
            advanced_backup_settings: self.advanced_backup_settings_view(),
            scan_settings: self.scan_settings_view(),
        }
    }

    /// `AdvancedBackupSettings` as a response reports it: "The list is populated
    /// only if the advanced option is set for the backup plan" (model doc), so a
    /// plan configured with none reports none rather than an empty list.
    pub fn advanced_backup_settings_view(&self) -> Option<Vec<AdvancedBackupSetting>> {
        (!self.advanced_backup_settings.is_empty()).then(|| self.advanced_backup_settings.clone())
    }

    /// `ScanSettings` as a response reports it, for the same reason as the
    /// settings above: a plan configured with no scanning reports none rather
    /// than an empty list.
    pub fn scan_settings_view(&self) -> Option<Vec<ScanSetting>> {
        (!self.scan_settings.is_empty()).then(|| self.scan_settings.clone())
    }
}

/// A backup plan as `CreateBackupPlan` stored it: the identifiers this service
/// minted for it, the account-level members an update leaves alone, and every
/// [`BackupPlanVersion`] of its body.
///
/// A plan is a VERSIONED resource: `UpdateBackupPlan` "updates the specified
/// backup plan. The new version is uniquely identified by its ID" (model doc),
/// so an update stores the body it was given as ANOTHER version of this same
/// plan \u2014 same plan id, same ARN \u2014 and the version it replaces is kept
/// readable by the id it was stored under.
#[derive(Debug, Clone)]
pub struct BackupPlanRecord {
    pub plan_id: String,
    pub arn: String,
    /// The plan's current version: the one a read that names no version
    /// answers from, and the one an update supersedes.
    pub current: BackupPlanVersion,
    /// The versions an update has superseded, oldest first. Bounded by
    /// [`MAX_BACKUP_PLAN_VERSIONS`].
    pub superseded: Vec<BackupPlanVersion>,
    pub creator_request_id: Option<String>,
    /// When the plan was deleted; `None` while the account still holds it.
    pub deletion_date: Option<SystemTime>,
    /// The last time one of this plan's rules ran. `None` until one does: this
    /// emulator starts a backup job only when a caller asks for one, so nothing
    /// has run a scheduled rule.
    pub last_execution_date: Option<SystemTime>,
    /// The tags `BackupPlanTags` assigned to the plan.
    pub tags: BTreeMap<String, String>,
}

impl BackupPlanRecord {
    /// Every version of this plan, oldest first \u2014 the superseded ones and then
    /// the current one. The ONE walk of a plan's history, so what a read of a
    /// version resolves and what a list of them reports cannot disagree.
    pub fn versions(&self) -> impl Iterator<Item = &BackupPlanVersion> {
        self.superseded
            .iter()
            .chain(::std::iter::once(&self.current))
    }

    /// The version stored under `version_id`, if this plan has one: a version
    /// stays readable by its own id after an update supersedes it.
    pub fn version(&self, version_id: &str) -> Option<&BackupPlanVersion> {
        self.versions()
            .find(|version| version.version_id == version_id)
    }

    /// Store `version` as this plan's current version, keeping the one it
    /// replaces readable. The ONE place a plan's history grows, so the bound on
    /// it holds however many updates a plan is given: once the plan holds
    /// [`MAX_BACKUP_PLAN_VERSIONS`], the oldest superseded version is forgotten
    /// \u2014 never the current one.
    pub fn supersede_current_version(&mut self, version: BackupPlanVersion) {
        let replaced = ::std::mem::replace(&mut self.current, version);
        self.superseded.push(replaced);
        let overflow = (self.superseded.len() + 1).saturating_sub(MAX_BACKUP_PLAN_VERSIONS);
        self.superseded.drain(..overflow);
    }
}

/// One rule of a plan as it is STORED: the rule the caller configured, plus the
/// `RuleId` this service minted for it ("RuleId: Uniquely identifies a rule that
/// is used to schedule the backup of a selection of resources", a member only
/// the service fills) and the backup windows the service fills in where the
/// request stated none ([`DEFAULT_START_WINDOW_MINUTES`],
/// [`DEFAULT_COMPLETION_WINDOW_MINUTES`]) — a stored rule carries the windows
/// its jobs are really scheduled with, so every read of the plan reports them.
pub fn stored_backup_rule(rule: BackupRuleInput, rule_id: String) -> BackupRule {
    BackupRule {
        rule_name: rule.rule_name,
        target_backup_vault_name: rule.target_backup_vault_name,
        target_logically_air_gapped_backup_vault_arn: rule
            .target_logically_air_gapped_backup_vault_arn,
        schedule_expression: rule.schedule_expression,
        start_window_minutes: Some(
            rule.start_window_minutes
                .unwrap_or(DEFAULT_START_WINDOW_MINUTES),
        ),
        completion_window_minutes: Some(
            rule.completion_window_minutes
                .unwrap_or(DEFAULT_COMPLETION_WINDOW_MINUTES),
        ),
        lifecycle: rule.lifecycle,
        recovery_point_tags: rule.recovery_point_tags,
        rule_id: Some(rule_id),
        copy_actions: rule.copy_actions,
        enable_continuous_backup: rule.enable_continuous_backup,
        schedule_expression_timezone: rule.schedule_expression_timezone,
        index_actions: rule.index_actions,
        scan_actions: rule.scan_actions,
    }
}

/// One resource selection of a backup plan as `CreateBackupSelection` stored it:
/// the selection body the caller gave, plus the `SelectionId` this service minted
/// for it.
///
/// A selection belongs to exactly one plan — every operation on one addresses it
/// by the PAIR (`BackupPlanId`, `SelectionId`), both `@httpLabel`s of
/// `GetBackupSelection` and `DeleteBackupSelection` — so that pair is the key
/// this record is stored under.
#[derive(Debug, Clone)]
pub struct BackupSelectionRecord {
    pub selection_id: String,
    pub plan_id: String,
    /// The body the create was given, kept verbatim: a selection has no member
    /// the service fills, so a read reports what was stored.
    pub selection: BackupSelection,
    pub creator_request_id: Option<String>,
    pub creation_date: SystemTime,
}

/// A framework as `CreateFramework` stored it: the controls the caller gave,
/// plus the ARN this service minted, the instant it recorded and the deployment
/// status its own clock moves.
///
/// A framework is addressed by `FrameworkName` (the `@httpLabel` of every
/// framework-scoped operation), so that name is the key this record is stored
/// under, and the control list is kept verbatim: a control has no member the
/// service fills, so a read reports what was stored.
#[derive(Debug, Clone)]
pub struct FrameworkRecord {
    pub name: String,
    pub arn: String,
    pub description: Option<String>,
    /// The framework's controls in the order the caller gave them.
    pub controls: Vec<FrameworkControl>,
    pub creation_time: SystemTime,
    /// The `IdempotencyToken` the create carried, which every later read of the
    /// framework reports and which a repeated create replays.
    pub idempotency_token: Option<String>,
    /// The `IdempotencyToken` of the update that last wrote this framework, so
    /// retrying that update "results in a success message with no action taken"
    /// (model doc for `UpdateFrameworkInput$IdempotencyToken`). `None` until an
    /// update carrying one is applied.
    pub update_idempotency_token: Option<String>,
    /// The tags `FrameworkTags` assigned to the framework.
    pub tags: BTreeMap<String, String>,
    /// Where the framework is in its deployment: one of the statuses the model
    /// lists for `Framework$DeploymentStatus`.
    pub deployment_status: String,
    /// When the deployment this record is in becomes due to settle.
    pub settles_at: SystemTime,
}

impl FrameworkRecord {
    /// Whether a deployment action on this framework is still in flight: its
    /// `DeploymentStatus` is one of the model's non-terminal `*_IN_PROGRESS`
    /// values. AWS Backup deploys a framework's controls one action at a time,
    /// so another action on it is refused while this one runs
    /// ([`framework_action_in_progress`]).
    ///
    /// Read AFTER [`ComAmazonawsBackupCryocontrollerusermanagerState::settle_frameworks`],
    /// so a deployment the clock has already made due is terminal here.
    pub fn deployment_in_flight(&self) -> bool {
        self.deployment_status == FRAMEWORK_DEPLOYMENT_STATUS_CREATE_IN_PROGRESS
            || self.deployment_status == FRAMEWORK_DEPLOYMENT_STATUS_UPDATE_IN_PROGRESS
    }

    /// The `FrameworkStatus` a read reports, which follows the deployment: a
    /// framework whose controls are deployed has its recording status validated
    /// ([`FRAMEWORK_STATUS_ACTIVE`]), and one still deploying has nothing to
    /// validate yet ([`FRAMEWORK_STATUS_UNAVAILABLE`]).
    pub fn framework_status(&self) -> &'static str {
        if self.deployment_status == FRAMEWORK_DEPLOYMENT_STATUS_COMPLETED {
            FRAMEWORK_STATUS_ACTIVE
        } else {
            FRAMEWORK_STATUS_UNAVAILABLE
        }
    }
}

/// A legal hold as `CreateLegalHold` stored it: the title, description and
/// recovery-point selection the caller gave, plus the id and ARN this service
/// minted, the instant it recorded and the status its own clock moves.
///
/// A legal hold is addressed by `LegalHoldId` (the `@httpLabel` of every
/// hold-scoped operation), so that id is the key this record is stored under,
/// and the selection is kept verbatim: it has no member the service fills, so a
/// read reports what was stored.
#[derive(Debug, Clone)]
pub struct LegalHoldRecord {
    pub id: String,
    /// The region the hold was created in, which its ARN carries. A legal hold
    /// restrains the recovery points of its OWN region: the vaults its
    /// selection names are vaults of that region, since a vault name
    /// identifies a vault only within one region ([`vault_key`]).
    pub region: String,
    pub arn: String,
    pub title: String,
    pub description: String,
    pub creation_date: SystemTime,
    /// Where the hold is in its life: one of the statuses the model lists for
    /// `LegalHoldStatus`.
    pub status: String,
    /// When the creation this record is in becomes due to settle.
    pub settles_at: SystemTime,
    /// The `IdempotencyToken` the create carried, which a repeated create
    /// replays.
    pub idempotency_token: Option<String>,
    /// The criteria the hold selects recovery points by, exactly as given. Not
    /// optional on the record: a hold IS the hold it places on the recovery
    /// points its selection names, so a create without one is refused.
    pub recovery_point_selection: RecoveryPointSelection,
    /// The tags the create assigned to the hold.
    pub tags: BTreeMap<String, String>,
    /// When the hold was cancelled, which `CancelLegalHold` records; `None`
    /// while the hold has never been cancelled.
    pub cancellation_date: Option<SystemTime>,
    /// Why the hold was cancelled, which `CancelLegalHold` records with the
    /// cancellation instant above.
    pub cancel_description: Option<String>,
    /// How long the cancelled hold's record is retained, which
    /// `CancelLegalHold` records from the days the caller asked for.
    pub retain_record_until: Option<SystemTime>,
}

impl LegalHoldRecord {
    /// The ONE list projection of a stored legal hold, so every operation that
    /// reports holds reports the same members of the same record.
    pub fn listed(&self) -> LegalHold {
        LegalHold {
            title: Some(self.title.clone()),
            status: Some(self.status.clone()),
            description: Some(self.description.clone()),
            legal_hold_id: Some(self.id.clone()),
            legal_hold_arn: Some(self.arn.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            cancellation_date: self
                .cancellation_date
                .map(::aws_smithy_types::DateTime::from),
        }
    }

    /// Whether `point` is one of the recovery points this hold is ON — the ONE
    /// place that question is decided, so what a read reports the hold covers
    /// is what its stored selection names.
    ///
    /// A hold is "a restraint on altering or deleting a backup" (model doc for
    /// `CreateLegalHold`), placed when it was created on the recovery points
    /// its `RecoveryPointSelection` named then. Each member the selection
    /// states NARROWS the set, and one it states none of restricts nothing:
    ///
    /// * `VaultNames` are "the names of the vaults in which the selected
    ///   recovery points are contained", so a point in a vault the selection
    ///   does not name is not held — unless the selection names every vault
    ///   ([`VAULT_NAMES_WILDCARD`]), which narrows nothing;
    /// * `ResourceIdentifiers` are "the resources included in the resource
    ///   selection", which a recovery point names by its protected resource;
    /// * `DateRange` is a window of creation instants, its `FromDate` "the
    ///   beginning date, inclusive" and its `ToDate` "the end date, inclusive".
    ///
    /// A point of ANOTHER REGION is held by none of them: a hold is a regional
    /// resource, and a vault name in its selection names a vault of the hold's
    /// own region, so the region a point's vault belongs to is settled before
    /// the selection is applied at all.
    ///
    /// A point created after the hold was placed is held by none of them:
    /// "Future DateTime values are not permitted" for a selection's range (model
    /// doc for `DateRange`), so a selection names recovery points that already
    /// exist.
    pub fn holds(&self, point: &RecoveryPoint) -> bool {
        let point_region = backup_vault_arn_parts(&point.backup_vault_arn).map(|parts| parts.region);
        if point_region.as_deref() != Some(self.region.as_str()) {
            return false;
        }
        if point.creation_date > self.creation_date {
            return false;
        }
        let selection = &self.recovery_point_selection;
        let vault_names = selection.vault_names.as_deref().unwrap_or_default();
        let every_vault = vault_names.iter().any(|name| name == VAULT_NAMES_WILDCARD);
        if !vault_names.is_empty() && !every_vault && !vault_names.contains(&point.backup_vault_name)
        {
            return false;
        }
        let resources = selection.resource_identifiers.as_deref().unwrap_or_default();
        if !resources.is_empty() && !resources.contains(&point.resource_arn) {
            return false;
        }
        match &selection.date_range {
            None => true,
            Some(range) => {
                // Compared as the epoch offsets the model's timestamps carry,
                // so a bound outside the span a `SystemTime` can express still
                // orders against the point's creation instant.
                let created = ::aws_smithy_types::DateTime::from(point.creation_date).as_secs_f64();
                created >= range.from_date.as_secs_f64()
                    && created <= range.to_date.as_secs_f64()
            }
        }
    }
}

/// Per-service state for com-amazonaws-backup-cryocontrollerusermanager. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsBackupCryocontrollerusermanagerState {
    /// Vaults by the (region, name) pair a vault is identified by
    /// ([`vault_key`]). Bounded by [`MAX_VAULTS`] per region.
    pub vaults: BTreeMap<(String, String), BackupVault>,
    /// Recovery points by `RecoveryPointArn`. Bounded by [`MAX_RECOVERY_POINTS`].
    pub recovery_points: BTreeMap<String, RecoveryPoint>,
    /// Per-vault recovery-point ARNs in insertion order, keyed by the vault's
    /// own [`vault_key`] — the single index both the list operation and
    /// `NumberOfRecoveryPoints` read, so it holds exactly the ARNs
    /// `recovery_points` does and one region's vault never reports another
    /// region's points.
    pub recovery_points_by_vault: BTreeMap<(String, String), Vec<String>>,
    /// Backup jobs by `BackupJobId`. Bounded by [`MAX_BACKUP_JOBS`].
    pub backup_jobs: BTreeMap<String, BackupJob>,
    /// Copy jobs by `CopyJobId`. Bounded by [`MAX_COPY_JOBS`].
    pub copy_jobs: BTreeMap<String, CopyJobRecord>,
    /// Backup plans by `BackupPlanId`. Bounded by [`MAX_BACKUP_PLANS`].
    pub backup_plans: BTreeMap<String, BackupPlanRecord>,
    /// Resource selections by the (`BackupPlanId`, `SelectionId`) pair AWS Backup
    /// addresses one by. Keyed plan-first, so one plan's selections are
    /// contiguous and a read of them is a range of this one map rather than a
    /// second index beside it. Bounded by [`MAX_SELECTIONS_PER_PLAN`] per plan.
    pub backup_selections: BTreeMap<(String, String), BackupSelectionRecord>,
    /// Frameworks by `FrameworkName`. Bounded by [`MAX_FRAMEWORKS`].
    pub frameworks: BTreeMap<String, FrameworkRecord>,
    /// Legal holds by `LegalHoldId`. Bounded by [`MAX_LEGAL_HOLDS`].
    pub legal_holds: BTreeMap<String, LegalHoldRecord>,
    /// Restore jobs by `RestoreJobId`. Bounded by [`MAX_RESTORE_JOBS`].
    pub restore_jobs: BTreeMap<String, RestoreJobRecord>,
    /// Tiering configurations by `TieringConfigurationName` — the name a
    /// tiering configuration is addressed by, so the map itself is what makes
    /// that name unique to the account. Bounded by
    /// [`MAX_TIERING_CONFIGURATIONS`].
    pub tiering_configurations: BTreeMap<String, TieringConfigurationRecord>,
    /// `StartBackupJob`'s `IdempotencyToken` -> what it replays. Bounded by
    /// [`MAX_IDEMPOTENCY_TOKENS`] and [`IDEMPOTENCY_TOKEN_TTL`].
    pub idempotency_tokens: BTreeMap<String, IdempotencyRecord>,
    /// `StartCopyJob`'s `IdempotencyToken` -> what it replays, bounded the same
    /// way. A token distinguishes "otherwise identical calls to" ONE operation
    /// (the model's doc for each `IdempotencyToken` member), so a copy job's
    /// tokens are their own namespace: a token a caller used to start a backup
    /// says nothing about a copy.
    pub copy_job_idempotency_tokens: BTreeMap<String, IdempotencyRecord>,
    /// The KMS key this service created for the account per region, for vaults
    /// whose create request named none. One key per region, minted on first
    /// use, so every default-key vault in a region reports the same key on
    /// every read. Bounded by [`MAX_DEFAULT_ENCRYPTION_KEYS`].
    pub default_encryption_keys: BTreeMap<String, String>,
    /// What has already been published about each stored job, and the state changes
    /// waiting to be published — see [`JobEventLog`]. Bounded by
    /// [`MAX_PENDING_EVENTS`] and pruned to the jobs the store still holds.
    pub job_events: JobEventLog,
    /// The single clock pump that publishes a transition when it falls due, with no
    /// further call from anybody — see [`drive_job_clock`].
    pub job_clock: JobClock,
    /// Monotonic counter behind [`Self::next_id`]; never reused within a run.
    id_seq: u64,
}

impl ComAmazonawsBackupCryocontrollerusermanagerState {
    /// A fresh, unique id in the hyphenated 8-4-4-4-12 hex form AWS Backup job
    /// and recovery-point ids carry.
    ///
    /// Uniqueness comes from `id_seq`, a per-engine counter, so two ids minted
    /// within one nanosecond still differ; the creation instant supplies the
    /// high bits so ids from separate runs do not collide.
    pub fn next_id(&mut self, now: SystemTime) -> String {
        self.id_seq = self.id_seq.wrapping_add(1);
        let nanos = u64::try_from(
            now.duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
        )
        .unwrap_or(u64::MAX);
        let seq = self.id_seq;
        format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (nanos >> 32) as u32,
            ((nanos >> 16) & 0xffff) as u16,
            (nanos & 0xffff) as u16,
            ((seq >> 48) & 0xffff) as u16,
            seq & 0xffff_ffff_ffff,
        )
    }

    /// The stored version a plan body becomes: the body the caller gave, with
    /// the `VersionId` and the per-rule `RuleId`s only this service can mint.
    ///
    /// The ONE place a plan body becomes a stored version, shared by the create
    /// and the update, so a plan stored by either carries the same members
    /// filled the same way and every read of it reports them.
    pub fn minted_backup_plan_version(
        &mut self,
        plan: BackupPlanInput,
        now: SystemTime,
    ) -> BackupPlanVersion {
        let version_id = self.next_id(now);
        let rules = plan
            .rules
            .into_iter()
            .map(|rule| {
                let rule_id = self.next_id(now);
                stored_backup_rule(rule, rule_id)
            })
            .collect();
        BackupPlanVersion {
            version_id,
            name: plan.backup_plan_name,
            rules,
            advanced_backup_settings: plan.advanced_backup_settings.unwrap_or_default(),
            scan_settings: plan.scan_settings.unwrap_or_default(),
            creation_date: now,
        }
    }

    /// Resolve a backup plan for a read, by the id it is stored under and
    /// optionally the version asked for.
    ///
    /// A plan read answers from the plan AT A VERSION — the one the request
    /// named, or the plan's latest where it named none — so every way such a
    /// read can fail to resolve one is the SAME condition: the version it was
    /// asked for is not there to be read ([`backup_plan_version_unreadable`]).
    /// That covers an id this account holds no plan under, a plan whose
    /// deletion `DeleteBackupPlan` recorded, and a `VersionId` the plan never
    /// had; the refusal does not tell the caller which of them it was.
    ///
    /// A read that names NO version answers from the plan's current version,
    /// the one the latest update stored; one that names a version answers from
    /// exactly that version, which an update that superseded it left readable.
    ///
    /// A deleted plan's record is KEPT (the deletion instant is part of it),
    /// but it is no longer readable: no version of a plan the account has
    /// deleted is reported again.
    pub fn backup_plan(
        &self,
        plan_id: &str,
        version_id: Option<&str>,
    ) -> Result<(&BackupPlanRecord, &BackupPlanVersion), ResourceNotFoundException> {
        let plan = self
            .backup_plans
            .get(plan_id)
            .ok_or_else(backup_plan_version_unreadable)?;
        if plan.deletion_date.is_some() {
            return Err(backup_plan_version_unreadable());
        }
        let version = match version_id {
            None => &plan.current,
            Some(asked) => plan.version(asked).ok_or_else(backup_plan_version_unreadable)?,
        };
        Ok((plan, version))
    }

    /// The plan a `CreatorRequestId` names, if this account holds one: "If the
    /// request includes a CreatorRequestId that matches an existing backup plan,
    /// that plan is returned" (model doc for the member), so a create that
    /// repeats a token answers with the plan it already made rather than making
    /// a second one.
    pub fn backup_plan_by_creator_request_id(
        &self,
        creator_request_id: &str,
    ) -> Option<&BackupPlanRecord> {
        self.backup_plans
            .values()
            .find(|plan| plan.creator_request_id.as_deref() == Some(creator_request_id))
    }

    /// Whether this account already holds a LIVE backup plan of this name. A
    /// plan whose deletion was recorded no longer holds its name: the account
    /// may create another plan under it, exactly as it may after a plan it never
    /// created.
    /// The name a plan holds is the one its CURRENT version carries: a plan's
    /// body includes its name, so an update that states another name moves the
    /// plan to it.
    pub fn holds_backup_plan_named(&self, name: &str) -> bool {
        self.backup_plans
            .values()
            .any(|plan| plan.current.name == name && plan.deletion_date.is_none())
    }

    /// The plan a mutating call must find: one this account holds whose deletion
    /// has not been recorded. A deleted plan's record is kept, but nothing is
    /// added to it, nothing deletes it twice and no read reports its body
    /// ([`Self::backup_plan`]).
    pub fn live_backup_plan(&self, plan_id: &str) -> Option<&BackupPlanRecord> {
        self.backup_plans
            .get(plan_id)
            .filter(|plan| plan.deletion_date.is_none())
    }

    /// The same resolution for a call that WRITES the plan it names, so a
    /// read-modify-write of one plan resolves it exactly as a read does and the
    /// two cannot disagree about which plans the account holds.
    pub fn live_backup_plan_mut(&mut self, plan_id: &str) -> Option<&mut BackupPlanRecord> {
        self.backup_plans
            .get_mut(plan_id)
            .filter(|plan| plan.deletion_date.is_none())
    }

    /// Resolve one resource selection for a read, by the pair AWS Backup
    /// addresses one by.
    ///
    /// This answers for the SELECTION half of the pair only: whether the plan
    /// half names a plan this account holds is a different condition, with its
    /// own class and its own token ([`selection_plan_not_found`]), which the
    /// calling operation resolves FIRST — so the caller is told which half of
    /// the pair was wrong.
    pub fn backup_selection(
        &self,
        plan_id: &str,
        selection_id: &str,
    ) -> Result<&BackupSelectionRecord, ResourceNotFoundException> {
        self.backup_selections
            .get(&(plan_id.to_string(), selection_id.to_string()))
            .ok_or_else(backup_selection_not_found)
    }

    /// Every selection the named plan holds, in `SelectionId` order — the range
    /// of the one selection map this plan owns, so a read of them cannot disagree
    /// with what a create inserted.
    pub fn backup_selections_of(&self, plan_id: &str) -> Vec<&BackupSelectionRecord> {
        self.backup_selections
            .range((plan_id.to_string(), String::new())..)
            .take_while(|((plan, _), _)| plan == plan_id)
            .map(|(_, selection)| selection)
            .collect()
    }

    /// The selection of this plan a `CreatorRequestId` names, if the plan holds
    /// one: a create that repeats a token answers with the selection it already
    /// made rather than making a second one.
    pub fn backup_selection_by_creator_request_id(
        &self,
        plan_id: &str,
        creator_request_id: &str,
    ) -> Option<&BackupSelectionRecord> {
        self.backup_selections_of(plan_id)
            .into_iter()
            .find(|selection| selection.creator_request_id.as_deref() == Some(creator_request_id))
    }

    /// Whether the named plan already holds a selection of this display name. A
    /// selection is displayed by its `SelectionName` within its plan, so two
    /// selections of one plan cannot share one.
    pub fn holds_backup_selection_named(&self, plan_id: &str, name: &str) -> bool {
        self.backup_selections_of(plan_id)
            .into_iter()
            .any(|selection| selection.selection.selection_name == name)
    }

    /// Resolve a framework for a read, by the name it is stored under.
    pub fn framework(&self, name: &str) -> Result<&FrameworkRecord, ResourceNotFoundException> {
        self.frameworks
            .get(name)
            .ok_or_else(|| framework_not_found(name))
    }

    /// The same resolution for a call that WRITES the framework it names, so a
    /// read-modify-write of one framework resolves it exactly as a read does and
    /// the two cannot disagree about which names the account holds.
    pub fn framework_mut(
        &mut self,
        name: &str,
    ) -> Result<&mut FrameworkRecord, ResourceNotFoundException> {
        self.frameworks
            .get_mut(name)
            .ok_or_else(|| framework_not_found(name))
    }

    /// The framework an `IdempotencyToken` created, if this account still holds
    /// one: "Retrying a successful request with the same idempotency token
    /// results in a success message with no action taken" (model doc for the
    /// member), so a create that repeats a token answers with the framework it
    /// already made rather than making a second one.
    pub fn framework_by_idempotency_token(&self, token: &str) -> Option<&FrameworkRecord> {
        self.frameworks
            .values()
            .find(|framework| framework.idempotency_token.as_deref() == Some(token))
    }

    /// Materialise the transition the clock has already made due for every
    /// framework: the deployment a create or an update that reports a
    /// non-terminal status owes, which finishes as `COMPLETED`. Like
    /// [`Self::settle_vaults`] this is the only write a reporting operation
    /// performs — each framework was owed its deployment at its own
    /// `settles_at` whether or not anybody called.
    pub fn settle_frameworks(&mut self, now: SystemTime) {
        for framework in self.frameworks.values_mut() {
            if framework.deployment_in_flight() && now >= framework.settles_at {
                framework.deployment_status = FRAMEWORK_DEPLOYMENT_STATUS_COMPLETED.to_string();
            }
        }
    }

    /// Resolve a legal hold for a read, by the id it is stored under.
    pub fn legal_hold(
        &self,
        id: &str,
    ) -> Result<&LegalHoldRecord, InvalidParameterValueException> {
        self.legal_holds.get(id).ok_or_else(legal_hold_not_found)
    }

    /// The legal hold an `IdempotencyToken` created, if this account still holds
    /// one: "Retrying a successful request with the same idempotency token
    /// results in a success message with no action taken" (model doc for the
    /// member), so a create that repeats a token answers with the hold it
    /// already made rather than making a second one.
    pub fn legal_hold_by_idempotency_token(&self, token: &str) -> Option<&LegalHoldRecord> {
        self.legal_holds
            .values()
            .find(|hold| hold.idempotency_token.as_deref() == Some(token))
    }

    /// Every stored recovery point `hold` is on, in `RecoveryPointArn` order —
    /// the recovery points "of the specified legal hold" a read of the hold's
    /// coverage reports.
    ///
    /// `recovery_points` is a `BTreeMap` keyed by that ARN, so this walk is
    /// already in ARN order, which is what makes an ARN-valued page cursor
    /// resolve to the same position on every request. Membership is
    /// [`LegalHoldRecord::holds`]'s single decision, so the holds a point is
    /// under and the points a hold covers can never disagree.
    pub fn recovery_points_held_by(&self, hold: &LegalHoldRecord) -> Vec<&RecoveryPoint> {
        self.recovery_points
            .values()
            .filter(|point| hold.holds(point))
            .collect()
    }

    /// Whether an ACTIVE legal hold is on `point`, so a call that would delete
    /// or disassociate it is refused ([`recovery_point_held`]): "Any actions to
    /// delete or disassociate a recovery point will fail with an error if one or
    /// more active legal holds are on the recovery point" (model doc for
    /// `CreateLegalHold`).
    ///
    /// A hold still being placed is not in force yet and a cancelled one has
    /// released what it held, so only a hold in the model's `ACTIVE` status
    /// restrains anything — read AFTER [`Self::settle_legal_holds`], so a hold
    /// the clock has already activated counts here. Membership is
    /// [`LegalHoldRecord::holds`]'s single decision, so what restrains a point
    /// is exactly what a read of the hold's coverage reports.
    pub fn legal_hold_restrains_recovery_point(&self, point: &RecoveryPoint) -> bool {
        self.legal_holds
            .values()
            .any(|hold| hold.status == LEGAL_HOLD_STATUS_ACTIVE && hold.holds(point))
    }

    /// Materialise the transition the clock has already made due for every legal
    /// hold: the `CREATING` -> `ACTIVE` transition a create that reports a
    /// non-terminal status owes. Like [`Self::settle_frameworks`] this is the
    /// only write a reporting operation performs — each hold was owed its
    /// activation at its own `settles_at` whether or not anybody called.
    pub fn settle_legal_holds(&mut self, now: SystemTime) {
        for hold in self.legal_holds.values_mut() {
            if hold.status == LEGAL_HOLD_STATUS_CREATING && now >= hold.settles_at {
                hold.status = LEGAL_HOLD_STATUS_ACTIVE.to_string();
            }
        }
    }

    /// Materialise the transition the clock has already made due, for ONE
    /// vault: the `CREATING` -> `AVAILABLE` transition a create that reports a
    /// non-terminal state owes. Like [`Self::settle_job`] this is the only
    /// write a reporting operation performs — the vault was owed its
    /// availability at `settles_at` whether or not anybody called — and only
    /// the vault asked about is touched.
    pub fn settle_vault(&mut self, region: &str, vault_name: &str, now: SystemTime) {
        if let Some(vault) = self.vaults.get_mut(&vault_key(region, vault_name)) {
            if vault.vault_state.as_deref() == Some(VAULT_STATE_CREATING)
                && vault.settles_at.is_some_and(|due| now >= due)
            {
                vault.vault_state = Some(VAULT_STATE_AVAILABLE.to_string());
                // A restore access vault becomes available exactly when the
                // access it asked for is approved, so the instant that
                // transition CAME DUE — not the instant somebody looked — is
                // the approval date it reports from then on.
                if vault.vault_type == VAULT_TYPE_RESTORE_ACCESS_BACKUP_VAULT {
                    vault.approval_date = vault.settles_at;
                }
            }
        }
    }

    /// The same for every stored vault, for an operation that reports them all:
    /// a list and a describe of the same vault can then never disagree about
    /// its state.
    pub fn settle_vaults(&mut self, now: SystemTime) {
        let keys: Vec<(String, String)> = self
            .vaults
            .iter()
            .filter(|(_, vault)| vault.vault_state.as_deref() == Some(VAULT_STATE_CREATING))
            .map(|(key, _)| key.clone())
            .collect();
        for (region, name) in keys {
            self.settle_vault(&region, &name, now);
        }
    }

    /// The vault the (region, name) pair a request resolves to names, if that
    /// region of the account holds one.
    ///
    /// The ONE lookup every vault-keyed read goes through, so no call can
    /// resolve a name against a region that does not hold it ([`vault_key`]).
    pub fn vault(&self, region: &str, vault_name: &str) -> Option<&BackupVault> {
        self.vaults.get(&vault_key(region, vault_name))
    }

    /// The same lookup for a call that WRITES the vault it resolves, so a
    /// read-modify-write of one vault resolves it exactly as a read does.
    pub fn vault_mut(&mut self, region: &str, vault_name: &str) -> Option<&mut BackupVault> {
        self.vaults.get_mut(&vault_key(region, vault_name))
    }

    /// The vault a `BackupVaultArn` names, if this account holds one.
    ///
    /// A vault ARN carries the region as well as the name
    /// ([`backup_vault_arn_parts`]), so it resolves to the one vault it names
    /// even when another region holds a vault of the same name — which is what
    /// lets a record that kept only the vault's ARN (a job, a recovery point)
    /// find its own vault.
    pub fn vault_by_arn(&self, backup_vault_arn: &str) -> Option<&BackupVault> {
        let parts = backup_vault_arn_parts(backup_vault_arn)?;
        self.vault(&parts.region, &parts.name)
    }

    /// Store a vault under its own key, with the empty recovery-point index
    /// that vault starts life with — the ONE insert, so a vault and its index
    /// are always keyed alike and a newly created vault holds no recovery
    /// points.
    pub fn insert_vault(&mut self, vault: BackupVault) {
        let key = vault_key(&vault.region, &vault.name);
        self.recovery_points_by_vault
            .entry(key.clone())
            .or_default();
        self.vaults.insert(key, vault);
    }

    /// Remove a vault and the recovery-point index it owns, so the name is free
    /// again in THAT region and nothing of the vault is left behind.
    pub fn remove_vault(&mut self, region: &str, vault_name: &str) {
        let key = vault_key(region, vault_name);
        self.vaults.remove(&key);
        self.recovery_points_by_vault.remove(&key);
    }

    /// Every vault this region of the account holds, in `BackupVaultName`
    /// order.
    ///
    /// `vaults` is keyed region-first ([`vault_key`]), so one region's vaults
    /// are a contiguous range of the one map and this walk is already in name
    /// order — which is what makes a name-valued page cursor resolve to the
    /// same position on every request.
    pub fn vaults_in(&self, region: &str) -> impl Iterator<Item = &BackupVault> {
        let region = region.to_string();
        self.vaults
            .range(vault_key(&region, "")..)
            .take_while(move |((vault_region, _), _)| *vault_region == region)
            .map(|(_, vault)| vault)
    }

    /// How many vaults this region of the account holds, which is what
    /// [`MAX_VAULTS`] bounds: one region's vaults never consume another's
    /// allowance.
    pub fn vault_count_in(&self, region: &str) -> usize {
        self.vaults_in(region).count()
    }

    /// Why this store will keep no further vault in `region`, or `None` while it
    /// will — the ONE vault allowance decision, so all three vault creates
    /// answer the same condition the same way.
    ///
    /// Two ceilings, because a vault's key has two halves ([`vault_key`]): the
    /// per-region [`MAX_VAULTS`], which is the shape of the quota AWS enforces
    /// per account and region, and [`MAX_VAULTS_ALL_REGIONS`] over the store as
    /// a whole, because the region half comes from the caller's credential
    /// scope and a per-region ceiling alone would let the store grow with the
    /// regions a caller signs for.
    pub fn exceeded_vault_allowance(&self, region: &str) -> Option<String> {
        if self.vault_count_in(region) >= MAX_VAULTS {
            return Some(format!(
                "this emulator keeps at most {MAX_VAULTS} backup vaults per region"
            ));
        }
        if self.vaults.len() >= MAX_VAULTS_ALL_REGIONS {
            return Some(format!(
                "this emulator keeps at most {MAX_VAULTS_ALL_REGIONS} backup vaults \
                 across all regions"
            ));
        }
        None
    }

    /// The KMS key this service encrypts a vault with when the create request
    /// named none: "if you do not specify a key, Backup creates a KMS key for
    /// you by default" (model doc for `EncryptionKeyArn`).
    ///
    /// Created on first use and then kept, so it is one key per region for the
    /// account — stable across reads and across vaults, as an account's default
    /// key is. The store is bounded by [`MAX_DEFAULT_ENCRYPTION_KEYS`]: the
    /// region is a field of the caller's credential scope, so the keys of this
    /// map are caller-chosen and it is pruned rather than left to grow.
    pub fn default_encryption_key_arn(&mut self, region: &str, now: SystemTime) -> String {
        if let Some(arn) = self.default_encryption_keys.get(region) {
            return arn.clone();
        }
        let arn = kms_key_arn(region, &self.next_id(now));
        self.default_encryption_keys
            .insert(region.to_string(), arn.clone());
        while self.default_encryption_keys.len() > MAX_DEFAULT_ENCRYPTION_KEYS {
            // Never the key just created: it is the one this request answers
            // with, and the vault being created records it.
            let evictable = self
                .default_encryption_keys
                .keys()
                .find(|key| key.as_str() != region)
                .cloned();
            let Some(evictable) = evictable else { break };
            self.default_encryption_keys.remove(&evictable);
        }
        arn
    }

    /// Project one stored vault for a read. The ONE place a vault's wire view
    /// is computed; both vault read operations go through it.
    pub fn vault_view(&self, vault: &BackupVault) -> VaultView {
        VaultView {
            name: vault.name.clone(),
            arn: vault.arn.clone(),
            vault_type: vault.vault_type.clone(),
            vault_state: vault.vault_state.clone(),
            encryption_key_arn: vault.encryption_key_arn.clone(),
            encryption_key_type: vault.encryption_key_type.clone(),
            creation_date: vault.creation_date,
            creator_request_id: vault.creator_request_id.clone(),
            number_of_recovery_points: self.recovery_point_count(&vault.region, &vault.name),
            // A vault reports itself locked exactly while it holds a vault-lock
            // configuration, and reports that configuration's bounds.
            locked: vault.lock.is_some(),
            min_retention_days: vault.lock.as_ref().and_then(|lock| lock.min_retention_days),
            max_retention_days: vault.lock.as_ref().and_then(|lock| lock.max_retention_days),
            lock_date: vault.lock.as_ref().and_then(|lock| lock.lock_date),
            source_backup_vault_arn: vault.source_backup_vault_arn.clone(),
            approval_date: vault.approval_date,
            mpa_approval_team_arn: vault.mpa_approval_team_arn.clone(),
        }
    }

    /// Every restore access backup vault created against the vault whose ARN is
    /// `source_backup_vault_arn`, in `BackupVaultName` order.
    ///
    /// The ONE place that link is resolved, so `ListRestoreAccessBackupVaults`'
    /// page and `RevokeRestoreAccessBackupVault`'s target are read out of the
    /// same set: a vault the list reports is exactly one the revoke can find.
    /// The source is matched by ARN rather than by name because the ARN is what
    /// the create was given, and it names the account and region as well as the
    /// name. The source's ARN names its region, and a restore access vault is
    /// created in that region, so [`Self::vaults_in`] walks exactly the vaults
    /// that can match, already in name order — which is what makes a
    /// name-valued page cursor resolve to the same position on every request.
    pub fn restore_access_vaults_of(&self, source_backup_vault_arn: &str) -> Vec<&BackupVault> {
        let region = backup_vault_arn_parts(source_backup_vault_arn)
            .map(|parts| parts.region)
            .unwrap_or_default();
        self.vaults_in(&region)
            .filter(|vault| vault.vault_type == VAULT_TYPE_RESTORE_ACCESS_BACKUP_VAULT)
            .filter(|vault| {
                vault.source_backup_vault_arn.as_deref() == Some(source_backup_vault_arn)
            })
            .collect()
    }

    /// The name this service gives a restore access backup vault whose create
    /// request named none: `CreateRestoreAccessBackupVault`'s `BackupVaultName`
    /// is optional, while a vault is ADDRESSED by name, so one is minted.
    ///
    /// The minted name has to satisfy the model's `BackupVaultName` pattern
    /// (`^[a-zA-Z0-9\-\_]{2,50}$`) or the vault could not be named by a later
    /// request, which rules out carrying a whole [`Self::next_id`]; the last
    /// group of one is the per-engine counter, so it is unique on its own.
    pub fn minted_restore_access_vault_name(&mut self, now: SystemTime) -> String {
        let id = self.next_id(now);
        let suffix = id.split('-').next_back().unwrap_or(id.as_str());
        format!("restore-access-{suffix}")
    }

    /// Project one stored recovery point for a read. The ONE place a recovery
    /// point's wire view is computed; both recovery-point read operations go
    /// through it.
    pub fn recovery_point_view(&self, point: &RecoveryPoint, now: SystemTime) -> RecoveryPointView {
        let calculated_lifecycle =
            point
                .calculated_lifecycle()
                .map(|(move_to_cold_storage_at, delete_at)| CalculatedLifecycle {
                    move_to_cold_storage_at: move_to_cold_storage_at
                        .map(::aws_smithy_types::DateTime::from),
                    delete_at: delete_at.map(::aws_smithy_types::DateTime::from),
                });
        RecoveryPointView {
            arn: point.arn.clone(),
            backup_vault_name: point.backup_vault_name.clone(),
            backup_vault_arn: point.backup_vault_arn.clone(),
            resource_arn: point.resource_arn.clone(),
            resource_type: point.resource_type.clone(),
            iam_role_arn: point.iam_role_arn.clone(),
            status: point.status.clone(),
            creation_date: point.creation_date,
            completion_date: point.completion_date,
            backup_size_in_bytes: point.backup_size_in_bytes,
            lifecycle: point.lifecycle.clone(),
            calculated_lifecycle,
            encryption_key_arn: point.encryption_key_arn.clone(),
            encryption_key_type: point.encryption_key_type().to_string(),
            is_encrypted: point.is_encrypted,
            is_parent: point.is_parent,
            vault_type: self.vault_type_of(&point.backup_vault_arn),
            index_status: point.index_status.clone(),
            storage_class: point.storage_class(now).to_string(),
        }
    }

    /// How many recovery points the named vault holds, derived from the index.
    pub fn recovery_point_count(&self, region: &str, vault_name: &str) -> i64 {
        self.recovery_points_by_vault
            .get(&vault_key(region, vault_name))
            .map_or(0, |arns| i64::try_from(arns.len()).unwrap_or(i64::MAX))
    }

    /// Resolve the recovery point a (`BackupVaultName`, `RecoveryPointArn`) pair
    /// addresses, within the region the request resolves to: one the named vault
    /// holds whose backup has settled.
    ///
    /// A vault this region of the account does not hold, an ARN no stored
    /// recovery point
    /// answers to, an ARN another vault holds, and one whose backup has not
    /// reached a terminal state yet are ONE condition as far as a call that
    /// addresses a recovery point is concerned: the recovery point asked about is
    /// not one the named vault holds. All four answer the same modelled refusal
    /// ([`recovery_point_not_found`]) and none of them tells the caller which it
    /// was. A point whose backup is still running is deliberately on that side of
    /// the line: the backup has reserved its ARN, but until the backup finishes
    /// there is no stored recovery point to address — so every call that names
    /// one, the reads included, resolves it here.
    pub fn stored_recovery_point(
        &self,
        region: &str,
        vault_name: &str,
        recovery_point_arn: &str,
    ) -> Result<&RecoveryPoint, ResourceNotFoundException> {
        let key = vault_key(region, vault_name);
        let held = self
            .recovery_points_by_vault
            .get(&key)
            .is_some_and(|arns| arns.iter().any(|arn| arn == recovery_point_arn));
        self.recovery_points
            .get(recovery_point_arn)
            .filter(|_| held)
            .filter(|point| point.status != RECOVERY_POINT_STATUS_CREATING)
            .ok_or_else(recovery_point_not_found)
    }

    /// Whether the lock on the vault holding `point` still retains it at `now`,
    /// so a call that would delete or change it is refused
    /// ([`recovery_point_locked`]).
    ///
    /// A Backup Vault Lock is what makes a recovery point's retention enforced by
    /// the SERVICE rather than by permissions: while a vault holds one, a point in
    /// it can only go once the retention it was stored under has elapsed. That
    /// retention is the point's own lifecycle, counted from its creation — the
    /// instant [`RecoveryPoint::calculated_lifecycle`] already computes for
    /// `CalculatedLifecycle`, so the two can never disagree — and a point whose
    /// lifecycle makes no deletion due is kept for as long as the lock stands.
    /// The lock's own `MinRetentionDays` is not a second deadline here: it bounds
    /// the retention a backup INTO the vault may be stored under, not when a
    /// stored point may be deleted. A vault with no lock retains nothing this way.
    pub fn lock_retains_recovery_point(&self, point: &RecoveryPoint, now: SystemTime) -> bool {
        let locked = self
            .vault_by_arn(&point.backup_vault_arn)
            .is_some_and(|vault| vault.lock.is_some());
        locked
            && point
                .calculated_lifecycle()
                .and_then(|(_, delete_at)| delete_at)
                .is_none_or(|delete_at| now < delete_at)
    }

    /// Remove one recovery point from the vault that holds it, from BOTH the
    /// record map and the per-vault index, so the index keeps holding exactly the
    /// ARNs `recovery_points` does and every count derived from it follows the
    /// deletion.
    pub fn remove_recovery_point(&mut self, region: &str, vault_name: &str, recovery_point_arn: &str) {
        self.recovery_points.remove(recovery_point_arn);
        if let Some(arns) = self
            .recovery_points_by_vault
            .get_mut(&vault_key(region, vault_name))
        {
            arns.retain(|held| held != recovery_point_arn);
        }
    }

    /// Resolve a vault for a vault-scoped READ, deciding access before existence.
    ///
    /// AWS Backup authorizes a vault-scoped read against the named vault as the
    /// resource, so a caller that may not see that vault is refused for lack of
    /// privileges and told nothing about whether it exists. A vault is outside the
    /// caller's visibility when the region of the account the request is scoped to
    /// does not hold it
    /// — it was never created in that region, or it has been deleted — or when the
    /// request addresses
    /// another account's vault with `BackupVaultAccountId`; this emulator holds
    /// exactly one account's vaults ([`engine::emulator_account_id`]), so any other
    /// account id is another account's.
    ///
    /// Reporting only `Err(AccessDeniedException)` is the point: a caller cannot ask
    /// this step whether an invisible vault exists, so no read can leak it.
    pub fn visible_vault(
        &self,
        region: &str,
        vault_name: &str,
        vault_account_id: Option<&str>,
    ) -> Result<&BackupVault, crate::scaffolded::AccessDeniedException> {
        let own_account = Self::is_own_account(vault_account_id);
        self.vault(region, vault_name)
            .filter(|_| own_account)
            .ok_or_else(crate::scaffolded::AccessDeniedException::insufficient_privileges)
    }

    /// The SAME decision for a vault-scoped call that WRITES the vault it resolves.
    ///
    /// AWS Backup authorizes a vault-lock action (`PutBackupVaultLockConfiguration`,
    /// `DeleteBackupVaultLockConfiguration`) against the named vault as the resource,
    /// so a vault-lock call naming a vault outside the caller's visibility is refused
    /// for lack of privileges and told nothing about whether that vault exists —
    /// exactly as a vault-scoped read is. Both paths therefore share ONE decision
    /// rather than each deciding for itself.
    ///
    /// The vault-lock inputs model no `BackupVaultAccountId` (`smithy_query(
    /// query="operation", operation="PutBackupVaultLockConfiguration")` lists
    /// `BackupVaultName` plus the three day counts only), so those handlers pass
    /// `None`; the parameter is kept so that every vault-scoped resolution, read or
    /// write, states the account it is scoped to.
    pub fn visible_vault_mut(
        &mut self,
        region: &str,
        vault_name: &str,
        vault_account_id: Option<&str>,
    ) -> Result<&mut BackupVault, crate::scaffolded::AccessDeniedException> {
        let own_account = Self::is_own_account(vault_account_id);
        self.vault_mut(region, vault_name)
            .filter(|_| own_account)
            .ok_or_else(crate::scaffolded::AccessDeniedException::insufficient_privileges)
    }

    /// Whether a `BackupVaultAccountId` names the one account whose vaults this
    /// emulator holds ([`engine::emulator_account_id`]); `None` is the caller's own
    /// account, so it always does.
    fn is_own_account(vault_account_id: Option<&str>) -> bool {
        vault_account_id.is_none_or(|account| account == ::engine::emulator_account_id())
    }

    /// Materialise the transition the clock has already made due, for ONE job.
    ///
    /// This is the ONLY write a reporting operation performs: a job whose
    /// `settles_at` has passed was owed its terminal state at that instant
    /// whether or not anybody called, so a describe settles it before reporting.
    /// Nothing else about the job changes, and the completion instant recorded
    /// is the instant it came due — not the instant somebody happened to look.
    /// Only the job asked about is touched, so the cost of a read does not grow
    /// with the store.
    ///
    /// WHICH terminal state is owed was decided when the job was started: a job
    /// whose backup could not be performed at all carries why
    /// ([`BackupJob::failure`]) and settles `FAILED` with that reason as its
    /// `StatusMessage` and that class as its `MessageCategory`, having transferred
    /// nothing; every other job settles `COMPLETED`.
    pub fn settle_job(&mut self, job_id: &str, now: SystemTime) {
        let Some(job) = self.backup_jobs.get_mut(job_id) else {
            return;
        };
        if job.state != BACKUP_JOB_STATE_CREATED || now < job.settles_at {
            return;
        }
        job.completion_date = Some(job.settles_at);
        let Some(failure) = job.failure.clone() else {
            job.state = BACKUP_JOB_STATE_COMPLETED.to_string();
            job.percent_done = PERCENT_DONE_COMPLETE.to_string();
            job.bytes_transferred = job.backup_size_in_bytes;
            return;
        };
        // A failed job produced nothing, so it reports no progress and no bytes
        // moved, and no recovery point was ever written for it (`StartBackupJob`
        // stores one only for a job that can produce it).
        job.state = BACKUP_JOB_STATE_FAILED.to_string();
        job.status_message = Some(failure.status_message);
        job.message_category = Some(failure.message_category);
    }

    /// The same for every stored job, for an operation that reports them all:
    /// a list and a describe of the same job can then never disagree about its
    /// state, its completion instant or how far it got.
    pub fn settle_jobs(&mut self, now: SystemTime) {
        let job_ids: Vec<String> = self
            .backup_jobs
            .iter()
            .filter(|(_, job)| job.state == BACKUP_JOB_STATE_CREATED)
            .map(|(job_id, _)| job_id.clone())
            .collect();
        for job_id in job_ids {
            self.settle_job(&job_id, now);
        }
    }

    /// The same for a COPY job, which the clock owes its `CREATED` ->
    /// `COMPLETED` transition in the same way, and for the recovery point that
    /// copy writes into the destination vault, so the job and the vault holding
    /// its copy can never disagree about whether the copy is there.
    pub fn settle_copy_job(&mut self, copy_job_id: &str, now: SystemTime) {
        let Some(job) = self.copy_jobs.get_mut(copy_job_id) else {
            return;
        };
        if job.state != COPY_JOB_STATE_CREATED || now < job.settles_at {
            return;
        }
        job.state = COPY_JOB_STATE_COMPLETED.to_string();
        job.completion_date = Some(job.settles_at);
        let destination = job.destination_backup_vault_arn.clone();
        if let Some(parts) = backup_vault_arn_parts(&destination) {
            self.settle_recovery_points_in(&parts.region, &parts.name, now);
        }
    }

    /// The same for every stored copy job, for a caller that materialises what the
    /// clock owes the store as a whole rather than one job's own read — the clock
    /// pump ([`drive_job_clock`]) is the one that does.
    pub fn settle_copy_jobs(&mut self, now: SystemTime) {
        let copy_job_ids: Vec<String> = self
            .copy_jobs
            .iter()
            .filter(|(_, job)| job.state == COPY_JOB_STATE_CREATED)
            .map(|(copy_job_id, _)| copy_job_id.clone())
            .collect();
        for copy_job_id in copy_job_ids {
            self.settle_copy_job(&copy_job_id, now);
        }
    }

    /// The members a copy-job read reports, computed ONCE from the stored job
    /// and the stored destination vault, so two readers of one copy job can
    /// never report two different bodies.
    ///
    /// The destination vault's type and lock state are read off that vault
    /// rather than stored on the job: they are the vault's own facts, and a lock
    /// applied after the copy started is in force for the copy it holds. A
    /// destination vault with no lock reports no lock state at all, as a vault
    /// read reports no `VaultState` for a type that carries none.
    pub fn copy_job_view(&self, job: &CopyJobRecord) -> CopyJob {
        let destination_vault = self.vault_by_arn(&job.destination_backup_vault_arn);
        CopyJob {
            account_id: Some(::engine::emulator_account_id().to_string()),
            copy_job_id: Some(job.copy_job_id.clone()),
            source_backup_vault_arn: Some(job.source_backup_vault_arn.clone()),
            source_recovery_point_arn: Some(job.source_recovery_point_arn.clone()),
            destination_backup_vault_arn: Some(job.destination_backup_vault_arn.clone()),
            destination_vault_type: destination_vault.map(|vault| vault.vault_type.clone()),
            destination_vault_lock_state: destination_vault
                .filter(|vault| vault.lock.is_some())
                .map(|_| VAULT_LOCK_STATE_LOCKED.to_string()),
            destination_recovery_point_arn: Some(job.destination_recovery_point_arn.clone()),
            destination_encryption_key_arn: job.destination_encryption_key_arn.clone(),
            destination_recovery_point_lifecycle: job
                .destination_recovery_point_lifecycle
                .clone(),
            resource_arn: Some(job.resource_arn.clone()),
            resource_type: job.resource_type.clone(),
            creation_date: Some(::aws_smithy_types::DateTime::from(job.creation_date)),
            completion_date: job.completion_date.map(::aws_smithy_types::DateTime::from),
            state: Some(job.state.clone()),
            status_message: job.status_message.clone(),
            backup_size_in_bytes: Some(job.backup_size_in_bytes),
            iam_role_arn: Some(job.iam_role_arn.clone()),
            is_parent: job.is_parent,
            // A copy a caller asked for has no backup plan behind it and no
            // parent or child jobs, and this slice stores no resource name or
            // message category for one.
            created_by: None,
            created_by_backup_job_id: None,
            parent_job_id: None,
            composite_member_identifier: None,
            number_of_child_jobs: None,
            child_jobs_in_state: None,
            resource_name: None,
            message_category: None,
        }
    }

    /// The `VaultType` of the vault a job or recovery point was stored in, or
    /// `None` when the account no longer holds that vault. Derived from the
    /// stored vault the record's `BackupVaultArn` names, so a job read never
    /// states a type the vault disagrees with and a same-named vault in another
    /// region cannot answer for it.
    pub fn vault_type_of(&self, backup_vault_arn: &str) -> Option<String> {
        self.vault_by_arn(backup_vault_arn)
            .map(|vault| vault.vault_type.clone())
    }

    /// The same, for the recovery points of ONE vault: the `CREATING` ->
    /// `COMPLETED` transition, and the `PENDING` -> `ACTIVE` one for a point
    /// whose job was asked to build an index.
    pub fn settle_recovery_points_in(&mut self, region: &str, vault_name: &str, now: SystemTime) {
        let arns = self
            .recovery_points_by_vault
            .get(&vault_key(region, vault_name))
            .cloned()
            .unwrap_or_default();
        for arn in arns {
            let Some(point) = self.recovery_points.get_mut(&arn) else {
                continue;
            };
            if now < point.settles_at {
                continue;
            }
            if point.status == RECOVERY_POINT_STATUS_CREATING {
                point.status = RECOVERY_POINT_STATUS_COMPLETED.to_string();
                point.completion_date = Some(point.settles_at);
            }
            if point.index_status.as_deref() == Some(INDEX_STATUS_PENDING) {
                point.index_status = Some(INDEX_STATUS_ACTIVE.to_string());
            }
        }
    }

    /// Forget every `IdempotencyToken` record past [`IDEMPOTENCY_TOKEN_TTL`],
    /// then the oldest records beyond [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// This is one of the two stores whose KEYS the caller chooses, so it is
    /// pruned on
    /// every `StartBackupJob` rather than left to grow with the request count.
    /// Both token namespaces are pruned by the one rule, so neither grows
    /// because the other operation is the one being called.
    pub fn prune_idempotency_tokens(&mut self, now: SystemTime) {
        Self::prune_token_store(&mut self.idempotency_tokens, now);
        Self::prune_token_store(&mut self.copy_job_idempotency_tokens, now);
    }

    /// The pruning rule itself, applied to one token namespace.
    fn prune_token_store(tokens: &mut BTreeMap<String, IdempotencyRecord>, now: SystemTime) {
        tokens.retain(|_, record| {
            record
                .recorded_at
                .checked_add(IDEMPOTENCY_TOKEN_TTL)
                .is_none_or(|expires_at| now < expires_at)
        });
        while tokens.len() > MAX_IDEMPOTENCY_TOKENS {
            let oldest = tokens
                .iter()
                .min_by_key(|(_, record)| record.recorded_at)
                .map(|(token, _)| token.clone());
            let Some(oldest) = oldest else { break };
            tokens.remove(&oldest);
        }
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsBackupCryocontrollerusermanagerState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsBackupCryocontrollerusermanagerState>)
}
