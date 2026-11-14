// SPDX-License-Identifier: CC0-1.0

//! The store-image and restore-image task stores, the capacity-manager settings and data
//! exports, and the route-server stores.
//!
//! Split out of the [`crate::state`] module root so each file holds one concern, and
//! re-exported from it, so a handler reaches these as `crate::state::<item>` like every
//! other primitive there.

use super::*;

/// "The state of the store task (`InProgress`, `Completed`, or `Failed`)"
/// (`StoreImageTaskResult$StoreTaskState`): the state of a task whose upload is still
/// running.
pub const STORE_TASK_STATE_IN_PROGRESS: &str = "InProgress";

/// `StoreImageTaskResult$StoreTaskState`'s `Completed`: the state of a task whose upload
/// has finished, after which "the AMI object appears in the bucket"
/// (`CreateStoreImageTaskRequest$Bucket`).
pub const STORE_TASK_STATE_COMPLETED: &str = "Completed";

/// `StoreImageTaskResult$StoreTaskState`'s `Failed`: the state of a task that did not
/// finish, which is the one state this emulator never puts a task into — nothing here can
/// fail an upload, and a state no work reached is not one to report.
pub const STORE_TASK_STATE_FAILED: &str = "Failed";

/// The percentage `StoreImageTaskResult$ProgressPercentage` reports for an upload that has
/// finished: all of it.
pub const STORE_IMAGE_TASK_COMPLETE_PERCENTAGE: i32 = 100;

/// The largest percentage a task still `InProgress` reports: an upload that has not
/// finished has not reached [`STORE_IMAGE_TASK_COMPLETE_PERCENTAGE`], so the estimate is
/// held one below it rather than rounding up to "done" before the task is.
pub const STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE: i32 =
    STORE_IMAGE_TASK_COMPLETE_PERCENTAGE - 1;

/// The suffix of the object a store task writes: AWS stores the AMI "as a single object"
/// named `<ami-id>.bin` in the bucket (`CreateStoreImageTask` / "Store and restore an AMI
/// using S3"), which is what `CreateStoreImageTaskResult$ObjectKey` and
/// `StoreImageTaskResult$S3objectKey` report. Derived from the AMI's id, never taken from a
/// request.
pub const STORE_IMAGE_OBJECT_KEY_SUFFIX: &str = ".bin";

/// How long a store task stays in the account's view: "Currently, only tasks from the past
/// 31 days can be viewed" (`DescribeStoreImageTasks`). A documented duration, so it keeps
/// its documented value.
pub const STORE_IMAGE_TASK_VISIBILITY: Duration = Duration::from_secs(31 * 24 * 60 * 60);

/// The largest number of store tasks this in-memory store holds.
///
/// A capacity bound in the style of [`MAX_SECONDARY_NETWORKS`], not an AWS quota: each task
/// is a record in this process's memory, so the number a caller can start has to be finite.
/// A count ceiling is not a memory bound on its own — see [`MAX_STORED_BYTES`] — so the
/// caller-chosen bytes of each task are charged against that budget too
/// ([`store_image_task_payload_bytes`]).
pub const MAX_STORE_IMAGE_TASKS: usize = 1_000;

/// The error code a `CreateStoreImageTask` past [`MAX_STORE_IMAGE_TASKS`] is refused with.
///
/// AWS publishes no quota code for this resource, so this follows EC2's own
/// `<Resource>LimitExceeded` spelling ([`VPC_LIMIT_EXCEEDED`],
/// [`SECONDARY_NETWORK_LIMIT_EXCEEDED`]).
pub const STORE_IMAGE_TASK_LIMIT_EXCEEDED: &str = "StoreImageTaskLimitExceeded";

/// The fixed overhead one stored task costs whatever it carries: the record itself and the
/// map entry holding it, as a CHOSEN constant in the same spirit as
/// [`VPC_ENDPOINT_CONNECTION_RECORD_BYTES`] — the point is that a task is not free against
/// [`MAX_STORED_BYTES`], not that it is byte-exact.
pub const STORE_IMAGE_TASK_RECORD_BYTES: usize = 256;

/// The prefix of the internal key a store task is held under.
///
/// A task has no id in the API — `StoreImageTaskResult` carries none — so this key is never
/// reported: it exists so the store has one stable, monotonically increasing order, which
/// is what makes "Tasks are listed in reverse chronological order"
/// (`DescribeStoreImageTasks`) a plain reverse iteration and lets a pagination token name
/// the task the next page starts at.
pub const STORE_IMAGE_TASK_KEY_PREFIX: &str = "store-image-task";

/// Refuse a store task a caller cannot start because the store is full: HTTP 400
/// [`STORE_IMAGE_TASK_LIMIT_EXCEEDED`].
pub fn store_image_task_limit_exceeded<E>(message: impl Into<String>) -> Scaffolded<E> {
    Scaffolded::service_error(STORE_IMAGE_TASK_LIMIT_EXCEEDED, 400, message)
}

/// One AMI store task: where it stores the AMI, when it started, and when its upload
/// finishes.
///
/// The record holds neither the task's `StoreTaskState` nor its `ProgressPercentage`: both
/// are facts of the task's age, so [`StoreImageTaskRecord::view_at`] derives them the way
/// [`SecondaryNetworkRecord::view_at`] derives a network's state — a reader observes the
/// progress the clock has made without writing anything, and two readers at the same
/// instant report the same task.
#[derive(Debug, Clone)]
pub struct StoreImageTaskRecord {
    /// "The ID of the AMI that is being stored" (`StoreImageTaskResult$AmiId`).
    pub ami_id: String,
    /// "The name of the Amazon S3 bucket that contains the stored AMI object"
    /// (`StoreImageTaskResult$Bucket`) — the one member of the task a request chooses.
    pub bucket: String,
    /// "The name of the stored AMI object in the bucket"
    /// (`StoreImageTaskResult$S3objectKey`), derived from the AMI's id.
    pub object_key: String,
    /// "The time the task started" (`StoreImageTaskResult$TaskStartTime`) — wall clock,
    /// because it is reported.
    pub started_at: SystemTime,
    /// The monotonic instant the upload started, which the progress estimate is measured
    /// from. Monotonic ([`Instant`]), because it is only ever compared against the clock,
    /// so a wall-clock adjustment cannot run a task's progress backwards.
    pub started_at_monotonic: Instant,
    /// The instant from which this task's `InProgress` -> `Completed` move is due: the
    /// upload is work the clock owes the task rather than the create call completing.
    pub completes_at: Instant,
    /// The instant from which this task is out of the account's view
    /// ([`STORE_IMAGE_TASK_VISIBILITY`] after it started), at which
    /// [`ComAmazonawsEc2Amazonec2State::materialize_due_transitions`] drops it.
    pub leaves_view_at: Instant,
}

impl StoreImageTaskRecord {
    /// A task storing `ami_id` into `bucket`, started now: `InProgress`, with its upload
    /// due [`TRANSITION_DELAY`] later — the span this crate gives every clock-owed
    /// transition, recorded rather than slept for.
    pub fn starting_now(ami_id: &str, bucket: &str) -> Self {
        let started_at_monotonic = Instant::now();
        Self {
            ami_id: ami_id.to_string(),
            bucket: bucket.to_string(),
            object_key: format!("{ami_id}{STORE_IMAGE_OBJECT_KEY_SUFFIX}"),
            started_at: SystemTime::now(),
            started_at_monotonic,
            completes_at: started_at_monotonic + TRANSITION_DELAY,
            leaves_view_at: started_at_monotonic + STORE_IMAGE_TASK_VISIBILITY,
        }
    }

    /// The task's `StoreTaskState` at `now`.
    ///
    /// [`STORE_TASK_STATE_FAILED`] is never answered: nothing in this emulator fails an
    /// upload, and a state no work reached is not one to report.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.completes_at {
            STORE_TASK_STATE_IN_PROGRESS
        } else {
            STORE_TASK_STATE_COMPLETED
        }
    }

    /// "The progress of the task as a percentage"
    /// (`StoreImageTaskResult$ProgressPercentage`) at `now`: how much of the upload's span
    /// has elapsed, which is [`STORE_IMAGE_TASK_COMPLETE_PERCENTAGE`] once the task has
    /// finished and is held at [`STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE`] until then.
    pub fn progress_at(&self, now: Instant) -> i32 {
        if now >= self.completes_at {
            return STORE_IMAGE_TASK_COMPLETE_PERCENTAGE;
        }
        let span = self
            .completes_at
            .saturating_duration_since(self.started_at_monotonic)
            .as_nanos();
        if span == 0 {
            return STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE;
        }
        let elapsed = now
            .saturating_duration_since(self.started_at_monotonic)
            .as_nanos();
        let percentage = elapsed.saturating_mul(
            u128::try_from(STORE_IMAGE_TASK_COMPLETE_PERCENTAGE).unwrap_or_default(),
        ) / span;
        i32::try_from(percentage)
            .unwrap_or(STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE)
            .clamp(0, STORE_IMAGE_TASK_MAX_IN_PROGRESS_PERCENTAGE)
    }

    /// This task as of `now`, as the model shapes it.
    ///
    /// `StoreTaskFailureReason` stays absent: "If the task succeeds, `null` is returned"
    /// (`StoreImageTaskResult$StoreTaskFailureReason`), and no task here fails.
    pub fn view_at(&self, now: Instant) -> StoreImageTaskResult {
        StoreImageTaskResult {
            ami_id: Some(self.ami_id.clone()),
            task_start_time: Some(system_time_to_date_time(self.started_at)),
            bucket: Some(self.bucket.clone()),
            s3object_key: Some(self.object_key.clone()),
            progress_percentage: Some(self.progress_at(now)),
            store_task_state: Some(self.state_at(now).to_string()),
            store_task_failure_reason: None,
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's IMDS defaults as they stand.
    ///
    /// A pure read: the defaults have no clock-owed move — a modify's own response reports
    /// the settings it just put in place — so nothing about them changes with time.
    pub fn instance_metadata_defaults(&self) -> AccountInstanceMetadataDefaults {
        self.instance_metadata_defaults.clone()
    }

    /// Put the account's IMDS defaults at `defaults`.
    ///
    /// The one writer, so what a later `GetInstanceMetadataDefaults` reports is always what
    /// a modify left here.
    pub fn set_instance_metadata_defaults(&mut self, defaults: AccountInstanceMetadataDefaults) {
        self.instance_metadata_defaults = defaults;
    }
}

// ---------------------------------------------------------------------------
// EBS encryption by default (the account-level setting of a Region)
// ---------------------------------------------------------------------------

/// How many Regions this emulator holds the EBS encryption-by-default setting for.
///
/// CHOSEN, like [`MAX_STORED_VOLUMES`] and for the same reason: the Region comes from the
/// caller's SigV4 credential scope, so without a ceiling a caller could enable the setting
/// in unboundedly many Region names and nothing would ever reclaim the entries. It is well
/// above the number of Regions AWS publishes. An emulator storage bound, not an AWS-modelled
/// refusal — `EnableEbsEncryptionByDefault` models no error at all.
pub const MAX_ENCRYPTION_BY_DEFAULT_REGIONS: usize = 128;

impl ComAmazonawsEc2Amazonec2State {
    /// Whether EBS encryption by default is enabled for this account in `region`.
    ///
    /// A Region the account has never enabled it in is a Region it is OFF in: the setting
    /// is off until `EnableEbsEncryptionByDefault` turns it on for that Region (Amazon EBS
    /// User Guide, "Encryption by default"), which is exactly why the store holds the
    /// Regions it is on in. Reporting only: this is the read `GetEbsEncryptionByDefault`
    /// answers with, and the encryption state no volume created in `region` can fall below.
    pub fn ebs_encryption_by_default(&self, region: &str) -> bool {
        self.ebs_encryption_by_default_regions.contains(region)
    }

    /// Turn EBS encryption by default on or off for this account in `region`, and answer
    /// the setting as it now stands — "the updated status of encryption by default"
    /// (`EnableEbsEncryptionByDefaultResult$EbsEncryptionByDefault`).
    ///
    /// Idempotent: enabling a Region it is already on in, or disabling one it is already
    /// off in, changes nothing and answers the same value.
    pub fn set_ebs_encryption_by_default(&mut self, region: &str, enabled: bool) -> bool {
        if enabled {
            self.ebs_encryption_by_default_regions
                .insert(region.to_string());
        } else {
            self.ebs_encryption_by_default_regions.remove(region);
        }
        self.ebs_encryption_by_default(region)
    }

    /// Would enabling the setting in `region` take the store past
    /// [`MAX_ENCRYPTION_BY_DEFAULT_REGIONS`]?
    ///
    /// A Region the store already holds costs nothing, so re-enabling one is never refused.
    pub fn would_exceed_encryption_by_default_regions(&self, region: &str) -> bool {
        !self.ebs_encryption_by_default_regions.contains(region)
            && self.ebs_encryption_by_default_regions.len() >= MAX_ENCRYPTION_BY_DEFAULT_REGIONS
    }
}

// ---------------------------------------------------------------------------
// The default credit specification of a burstable instance family
// ---------------------------------------------------------------------------

/// The credit option under which a burstable instance spends only the CPU credits it has
/// earned: `standard` — one of the two values "Valid values are `standard` and `unlimited`"
/// (`InstanceFamilyCreditSpecification$CpuCredits`) documents.
pub const CPU_CREDITS_STANDARD: &str = "standard";

/// The other credit option: `unlimited`, under which a burstable instance may sustain high
/// CPU beyond the credits it has earned, from the same documented pair.
pub const CPU_CREDITS_UNLIMITED: &str = "unlimited";

/// The credit options a default credit specification can hold: "Valid values are
/// `standard` and `unlimited`" (`InstanceFamilyCreditSpecification$CpuCredits`, and
/// `ModifyDefaultCreditSpecificationRequest$CpuCredits` in the same words).
///
/// The member is a plain `String` in the model with no enum behind it, so nothing refuses a
/// word outside this pair before a handler runs and this list is the only place the
/// documented pair lives.
pub const CPU_CREDITS_VALUES: [&str; 2] = [CPU_CREDITS_STANDARD, CPU_CREDITS_UNLIMITED];

/// The [`CPU_CREDITS_VALUES`] member `requested` names, or `None` when it names none.
///
/// Answering a `&'static str` out of this module's own constants is what keeps request text
/// out of the store, the way [`image_block_public_access_mode`] does for its catalog.
pub fn cpu_credits_value(requested: &str) -> Option<&'static str> {
    catalog_member(&CPU_CREDITS_VALUES, requested)
}

/// The modelled `UnlimitedSupportedInstanceFamily` member `requested` names, or `None` when
/// it names none.
///
/// The catalog is the model's own generated member list, so a family the model gains is
/// accepted without touching this module, and the stored family is always one of its
/// members rather than the request's text. Shared because both operations that report a
/// family's default credit option resolve the family exactly this way.
pub fn unlimited_supported_instance_family(requested: &str) -> Option<&'static str> {
    catalog_member(
        crate::ops::types::UNLIMITED_SUPPORTED_INSTANCE_FAMILY_VALUES,
        requested,
    )
}

/// The code EC2 answers for an instance family the action does not serve:
/// `InvalidInstanceFamily`. EC2 declares no error SHAPES at all, so every documented code
/// is carried by the error channel ([`crate::scaffolded`]) rather than by a modelled error.
pub const INVALID_INSTANCE_FAMILY: &str = "InvalidInstanceFamily";

/// Is `allowed` the `UnlimitedSupportedInstanceFamily` catalog — the value set the model
/// declares for the `InstanceFamily` member that names a burstable performance family?
///
/// The boundary's constraint walk reports a refused enum value as the value set it failed
/// rather than as the shape that declared it, so the catalog IS how that type is recognised,
/// exactly as [`is_caller_role_catalog`] recognises its own. EC2 words a refused value per
/// PARAMETER TYPE, which is why the wording is keyed to the type's catalog rather than to
/// the operation that happened to carry it.
pub fn is_unlimited_supported_instance_family_catalog(allowed: &[String]) -> bool {
    let catalog = crate::ops::types::UNLIMITED_SUPPORTED_INSTANCE_FAMILY_VALUES;
    allowed.len() == catalog.len()
        && allowed
            .iter()
            .zip(catalog)
            .all(|(held, member)| held == member)
}

/// How EC2 states an instance family the burstable catalog does not hold: the family at
/// fault, that the credit option the request is about is for burstable performance families
/// only, and what to do about it.
///
/// A fixed sentence as EC2 words this refusal, with ONE variable part — the family the
/// request named, echoed back. On the boundary's refusal path that value arrives already
/// elided to the framework's own `MAX_ECHOED_VALUE`, so echoing it cannot amplify a request
/// into an arbitrarily long response.
pub fn instance_family_refusal_message(value: &str) -> String {
    format!(
        "The {value} instance family does not support Unlimited. Only burstable performance \
         instance families support Unlimited. Change the instance family and try again."
    )
}

/// Refuse an instance family outside that catalog the way EC2 does: HTTP 400
/// [`INVALID_INSTANCE_FAMILY`] with [`instance_family_refusal_message`].
///
/// The modelled enum constraint already refuses such a word before a handler runs — and the
/// service's own refusal override answers this same code and sentence for it
/// (`crate::service`) — so a handler reaches this only if that boundary ever stops covering
/// the member. Shared, so the two operations that resolve a family and the boundary cannot
/// word one refusal three ways.
pub fn invalid_instance_family<E>(value: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_INSTANCE_FAMILY,
        400,
        instance_family_refusal_message(value),
    )
}

/// The one burstable family whose instances launch `standard` unless the account says
/// otherwise: T2 — "T2 instances launch as `standard` by default", while "T3, T3a and T4g
/// instances launch as `unlimited` by default" (Amazon EC2 User Guide, "Burstable
/// performance instances"). Later unlimited-supported families follow T3 rather than T2,
/// which is what [`default_cpu_credits`] answers for them.
pub const STANDARD_BY_DEFAULT_INSTANCE_FAMILY: &str = "t2";

/// How many Regions this emulator holds default credit specifications for.
///
/// CHOSEN, like [`MAX_ENCRYPTION_BY_DEFAULT_REGIONS`] and for the same reason: the Region
/// comes from the caller's SigV4 credential scope, so without a ceiling a caller could set
/// a default in unboundedly many Region names and nothing would ever reclaim the entries.
/// An emulator storage bound, not an AWS-modelled refusal —
/// `ModifyDefaultCreditSpecification` models no error at all.
pub const MAX_DEFAULT_CREDIT_SPECIFICATION_REGIONS: usize = MAX_ENCRYPTION_BY_DEFAULT_REGIONS;

/// AWS's own default credit option for `family`: where a family sits in an account that has
/// never modified it — [`CPU_CREDITS_STANDARD`] for
/// [`STANDARD_BY_DEFAULT_INSTANCE_FAMILY`] and [`CPU_CREDITS_UNLIMITED`] for every other
/// unlimited-supported family.
pub fn default_cpu_credits(family: &str) -> &'static str {
    if family == STANDARD_BY_DEFAULT_INSTANCE_FAMILY {
        CPU_CREDITS_STANDARD
    } else {
        CPU_CREDITS_UNLIMITED
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's default credit option for `family` in `region`.
    ///
    /// A pure READ: a family the account has never modified there answers
    /// [`default_cpu_credits`] without storing anything, and the setting has no clock-owed
    /// move — the option a modify asks for is the option the next read reports — so nothing
    /// about it changes with time and `GetDefaultCreditSpecification` cannot change what a
    /// later caller sees.
    pub fn default_cpu_credits_for(&self, region: &str, family: &str) -> &'static str {
        self.default_credit_specifications
            .get(region)
            .and_then(|families| families.get(family).copied())
            .unwrap_or_else(|| default_cpu_credits(family))
    }

    /// That default as the modelled `InstanceFamilyCreditSpecification` both operations
    /// report it in.
    ///
    /// The ONE projection, so a modify answers exactly what a later get does. The shape is
    /// named by its full path rather than imported at the top of this module, the way the
    /// IMDS defaults' projection names its own.
    pub fn default_credit_specification(
        &self,
        region: &str,
        family: &'static str,
    ) -> crate::ops::types::InstanceFamilyCreditSpecification {
        crate::ops::types::InstanceFamilyCreditSpecification {
            instance_family: Some(family.to_string()),
            cpu_credits: Some(self.default_cpu_credits_for(region, family).to_string()),
        }
    }

    /// Put the account's default credit option for `family` in `region` at `cpu_credits`,
    /// and answer the specification as it now stands.
    ///
    /// `family` and `cpu_credits` are `&'static str` so only a modelled catalog member and a
    /// documented credit option can be stored ([`unlimited_supported_instance_family`] and
    /// [`cpu_credits_value`] are what turn a request's words into them), and the answer is
    /// read back OUT of the store rather than echoed, so a writer reports exactly what a
    /// later read will. Idempotent: setting a family to the option it already holds changes
    /// nothing and answers the same specification.
    pub fn set_default_credit_specification(
        &mut self,
        region: &str,
        family: &'static str,
        cpu_credits: &'static str,
    ) -> crate::ops::types::InstanceFamilyCreditSpecification {
        self.default_credit_specifications
            .entry(region.to_string())
            .or_default()
            .insert(family, cpu_credits);
        self.default_credit_specification(region, family)
    }

    /// Would setting a family's default in `region` take the store past
    /// [`MAX_DEFAULT_CREDIT_SPECIFICATION_REGIONS`]?
    ///
    /// A Region the store already holds a default for costs nothing, so modifying another
    /// family there is never refused.
    pub fn would_exceed_default_credit_specification_regions(&self, region: &str) -> bool {
        !self.default_credit_specifications.contains_key(region)
            && self.default_credit_specifications.len()
                >= MAX_DEFAULT_CREDIT_SPECIFICATION_REGIONS
    }
}

// ---------------------------------------------------------------------------
// EC2 Capacity Manager (an account-level setting, and the ingestion it starts)
// ---------------------------------------------------------------------------

/// Capacity Manager turned ON for the account: the `CapacityManagerStatus` enum's
/// `enabled` member — "the current status of Capacity Manager after the enable operation"
/// (`EnableCapacityManagerResult$CapacityManagerStatus`). The whole catalog is the
/// generated [`crate::ops::types::CAPACITY_MANAGER_STATUS_VALUES`].
pub const CAPACITY_MANAGER_ENABLED: &str = "enabled";

/// Capacity Manager turned OFF: the `CapacityManagerStatus` enum's `disabled` member,
/// which is also where an account that has never enabled it sits.
pub const CAPACITY_MANAGER_DISABLED: &str = "disabled";

/// The ingestion an enable starts, while it is still catching up: the `IngestionStatus`
/// enum's `initial-ingestion-in-progress` member. The whole catalog is the generated
/// [`crate::ops::types::INGESTION_STATUS_VALUES`].
pub const INGESTION_STATUS_INITIAL_IN_PROGRESS: &str = "initial-ingestion-in-progress";

/// The ingestion an enable started, once it has caught up: the `IngestionStatus` enum's
/// `ingestion-complete` member.
///
/// Its third member, `ingestion-failed`, names an ingestion that broke; nothing in this
/// emulator can fail one, so no record is ever put into it.
pub const INGESTION_STATUS_COMPLETE: &str = "ingestion-complete";

/// The account's EC2 Capacity Manager settings: whether it is on, whether its data may be
/// aggregated across the organization, and when the ingestion an enable started falls
/// complete.
///
/// The status is a `&'static str` so only a member of the modelled `CapacityManagerStatus`
/// catalog can ever be stored here: the word comes from this module's own constants, never
/// from a request.
#[derive(Debug, Clone)]
pub struct CapacityManagerSettings {
    /// [`CAPACITY_MANAGER_ENABLED`] or [`CAPACITY_MANAGER_DISABLED`].
    pub(super) status: &'static str,
    /// Whether "Capacity Manager can aggregate data from all accounts in your
    /// organization" (`EnableCapacityManagerRequest$OrganizationsAccess`). "Default is
    /// false", which is where an account that has never asked for it sits.
    pub(super) organizations_access: bool,
    /// When the initial ingestion an enable started falls complete, and `None` while no
    /// enable has started one.
    ///
    /// The INSTANT rather than a word, because the move is owed to the clock: the status a
    /// reader sees is derived from it ([`Self::view_at`]), so reporting the settings never
    /// writes.
    pub(super) ingestion_complete_at: Option<Instant>,
}

impl Default for CapacityManagerSettings {
    /// Where an account that has never called `EnableCapacityManager` sits: Capacity
    /// Manager `disabled`, no Organizations access, and no ingestion at all.
    fn default() -> Self {
        Self {
            status: CAPACITY_MANAGER_DISABLED,
            organizations_access: false,
            ingestion_complete_at: None,
        }
    }
}

/// The account's Capacity Manager settings as of ONE instant: what every operation that
/// reports them answers from.
///
/// A view rather than the stored record, because the ingestion status is a fact of elapsed
/// time that no store holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapacityManagerView {
    /// The account's `CapacityManagerStatus`.
    pub status: &'static str,
    /// The account's `OrganizationsAccess`.
    pub organizations_access: bool,
    /// The `IngestionStatus` of the ingestion an enable started, and `None` when no enable
    /// has started one — there is then no ingestion to report a status for.
    pub ingestion_status: Option<&'static str>,
}

impl CapacityManagerSettings {
    /// These settings as of `now`.
    ///
    /// The ingestion status is DERIVED from the instant the enable recorded, so two reads
    /// at the same instant report the same word and a read writes nothing: an ingestion
    /// whose instant has not yet fallen due is still
    /// [`INGESTION_STATUS_INITIAL_IN_PROGRESS`], and one that has is
    /// [`INGESTION_STATUS_COMPLETE`].
    pub fn view_at(&self, now: Instant) -> CapacityManagerView {
        CapacityManagerView {
            status: self.status,
            organizations_access: self.organizations_access,
            ingestion_status: self.ingestion_complete_at.map(|due| {
                if now >= due {
                    INGESTION_STATUS_COMPLETE
                } else {
                    INGESTION_STATUS_INITIAL_IN_PROGRESS
                }
            }),
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// The account's Capacity Manager settings as of `now`.
    ///
    /// Reporting only: the one read every operation that answers these settings goes
    /// through, so none of them can report the account two ways.
    pub fn capacity_manager_at(&self, now: Instant) -> CapacityManagerView {
        self.capacity_manager.view_at(now)
    }

    /// Turn Capacity Manager ON for the account with `organizations_access`, and answer the
    /// settings as they now stand as of `now`.
    ///
    /// The one writer of the enable, so what a later report answers is always what this
    /// left here. `organizations_access` is the request's own choice — "specifies whether
    /// to enable cross-account access for Amazon Web Services Organizations"
    /// (`EnableCapacityManagerRequest$OrganizationsAccess`) — so it is applied on every
    /// enable.
    ///
    /// The ingestion is started only by the enable that turns the setting ON: an account
    /// already enabled keeps the instant its first enable recorded, so re-enabling it does
    /// not send a caught-up ingestion back to [`INGESTION_STATUS_INITIAL_IN_PROGRESS`].
    /// That instant is one [`TRANSITION_DELAY`] out, the span every clock-owed move of this
    /// store falls due after.
    pub fn enable_capacity_manager(
        &mut self,
        organizations_access: bool,
        now: Instant,
    ) -> CapacityManagerView {
        if self.capacity_manager.status != CAPACITY_MANAGER_ENABLED {
            self.capacity_manager.status = CAPACITY_MANAGER_ENABLED;
            self.capacity_manager.ingestion_complete_at = Some(now + TRANSITION_DELAY);
        }
        self.capacity_manager.organizations_access = organizations_access;
        self.capacity_manager.view_at(now)
    }

    /// Turn Capacity Manager OFF for the account, and answer the settings as they now stand
    /// as of `now`.
    ///
    /// The one writer of the disable, so what a later report answers is always what this
    /// left here. Three things change together, each one documented by the operation:
    ///
    /// * the status goes to [`CAPACITY_MANAGER_DISABLED`] — "disables EC2 Capacity Manager
    ///   for your account";
    /// * the cross-account aggregation flag goes off — "indicates whether Organizations
    ///   access is enabled. This will be false after disabling Capacity Manager"
    ///   (`DisableCapacityManagerResult$OrganizationsAccess`);
    /// * the ingestion is dropped — "this stops data ingestion ... no new data will be
    ///   processed", so there is no ingestion left to report an `IngestionStatus` for, and
    ///   an enable that comes later starts a fresh one rather than inheriting the stopped
    ///   one's instant.
    ///
    /// Idempotent: an account that is already off is left exactly where it was, since each
    /// of those three is already in the state a disable puts it in.
    pub fn disable_capacity_manager(&mut self, now: Instant) -> CapacityManagerView {
        self.capacity_manager.status = CAPACITY_MANAGER_DISABLED;
        self.capacity_manager.organizations_access = false;
        self.capacity_manager.ingestion_complete_at = None;
        self.capacity_manager.view_at(now)
    }
}

/// `CapacityManager.Disabled` (HTTP 400): the account's Capacity Manager is OFF, so there is
/// nothing for a request that acts on it to act on.
///
/// EC2's own code for that condition, in the `<Feature>.<Condition>` form it words a
/// feature-level refusal in: the recorded contract for `DisableCapacityManager` answers it
/// for an account that has not enabled Capacity Manager, where the account-level setting that
/// WORD describes is [`CAPACITY_MANAGER_DISABLED`].
pub const CAPACITY_MANAGER_DISABLED_ERROR: &str = "CapacityManager.Disabled";

/// Refuse a request that acts on Capacity Manager while the account has it off: HTTP 400
/// [`CAPACITY_MANAGER_DISABLED_ERROR`], in the wording its recorded refusal carries.
///
/// One place the sentence is worded, so every operation that has to report that condition
/// reports it the same way.
pub fn capacity_manager_disabled<E>() -> Scaffolded<E> {
    Scaffolded::service_error(
        CAPACITY_MANAGER_DISABLED_ERROR,
        400,
        "Capacity Manager isn't enabled for this account.",
    )
}

// ---------------------------------------------------------------------------
// Capacity Manager DATA EXPORT configurations (a resource with an id)
// ---------------------------------------------------------------------------

/// The code EC2 answers for a data export id the account holds no export configuration for.
///
/// The recorded refusal behind
/// `issues/.../20261023T102725Z-ec2-deletecapacitymanagerdataexport-stub.md` is where this
/// code's spelling comes from: AWS answered `DeleteCapacityManagerDataExport` for an id no
/// configuration of the account carries with `InvalidCapacityManagerDataExportId.NotFound`
/// (HTTP 400), the `Invalid<Resource>Id.NotFound` form
/// [`INVALID_IMAGE_USAGE_REPORT_ID_NOT_FOUND`] takes as well.
pub const INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND: &str =
    "InvalidCapacityManagerDataExportId.NotFound";

/// The code EC2 answers for something that is not a data export id at all.
///
/// EC2 separates "not an id of this kind" from "no such resource" with the resource's own
/// `Invalid<Kind>.Malformed` code, refused BEFORE any lookup — the split
/// [`prefixed_id_is_well_formed`] documents and [`INVALID_IMAGE_USAGE_REPORT_ID_MALFORMED`]
/// spells for another resource of this crate — so the kind name of the `.NotFound` code
/// above carries it.
pub const INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED: &str =
    "InvalidCapacityManagerDataExportId.Malformed";

/// The id prefix every Capacity Manager data export id carries: `cmde-`.
///
/// `CapacityManagerDataExportId` is a plain `String` in the model
/// (`smithy_query(query="shape",shape="CapacityManagerDataExportId")`), so the prefix is
/// taken from the id AWS itself read as a "capacity-manager-data-export ID" in the recorded
/// refusal behind this resource's issue. It is the prefix an export id is minted under as
/// well ([`ComAmazonawsEc2Amazonec2State::next_id`]), so one id form serves the mint and the
/// readers.
pub const CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX: &str = "cmde";

/// Whether `id` is a well-formed Capacity Manager data export id — one EC2 could have
/// ISSUED: [`CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX`] followed by 8 or 17 lowercase
/// hexadecimal digits ([`prefixed_id_is_well_formed`]).
///
/// No null-id clause, unlike [`image_usage_report_id_is_well_formed`]: the recorded refusal
/// behind this resource's issue shows AWS answering the all-zero long id
/// `InvalidCapacityManagerDataExportId.NotFound` rather than `.Malformed`, so that id is
/// one this reader admits and the lookup then finds nothing for.
pub fn capacity_manager_data_export_id_is_well_formed(id: &str) -> bool {
    prefixed_id_is_well_formed(CAPACITY_MANAGER_DATA_EXPORT_ID_PREFIX, id)
}

/// Refuse a request that named a data export id the account holds no configuration for:
/// HTTP 400 [`INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND`].
///
/// The wording is AWS's own for this resource — "The capacity-manager-data-export ID
/// '<id>' does not exist" — built from the id the REQUEST carried, bounded by
/// [`echoed_resource_id`] the way every refusal of this crate that quotes a value the
/// caller chose is.
pub fn invalid_capacity_manager_data_export_id_not_found<E>(export_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_NOT_FOUND,
        400,
        format!(
            "The capacity-manager-data-export ID '{}' does not exist",
            echoed_resource_id(export_id)
        ),
    )
}

/// Refuse a data export id the request named that is not an export id at all: HTTP 400
/// [`INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED`], which EC2 answers BEFORE it looks
/// the id up.
///
/// The message is [`invalid_id_message`] — the ONE sentence this service words an unreadable
/// resource id with, whatever kind of id it is.
pub fn invalid_capacity_manager_data_export_id_malformed<E>(export_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_CAPACITY_MANAGER_DATA_EXPORT_ID_MALFORMED,
        400,
        invalid_id_message(export_id),
    )
}

impl ComAmazonawsEc2Amazonec2State {
    /// How many data export configurations the account holds — "the number of active data
    /// export configurations for this account"
    /// (`GetCapacityManagerAttributesResult$DataExportCount`).
    ///
    /// Reporting only, and the one count every operation that answers it reads, so the
    /// report and the store cannot disagree. Saturating rather than wrapping, because the
    /// member is an `i32` and the map is not bounded by one.
    pub fn capacity_manager_data_export_count(&self) -> i32 {
        i32::try_from(self.capacity_manager_data_exports.len()).unwrap_or(i32::MAX)
    }

    /// Take the data export configuration `export_id` names out of the store, answering it
    /// when one was there and `None` when the account holds none under that id.
    ///
    /// The one remover, so "the export is gone" is a fact of this map rather than of a
    /// handler's own bookkeeping: a later read of that id finds nothing and the count above
    /// drops by one. It also drops every `ClientToken` that named the configuration — so a
    /// token can never name one that is gone — and gives the store's byte budget back exactly
    /// what [`capacity_manager_data_export_payload_bytes`] charged for them, the way
    /// [`ComAmazonawsEc2Amazonec2State::remove_image_usage_report`] does for a report.
    ///
    /// "This stops future scheduled exports but does not delete previously
    /// exported files from S3" (`DeleteCapacityManagerDataExport`) — nothing in this
    /// emulator holds those files, so removing the configuration is the whole of the delete.
    pub fn remove_capacity_manager_data_export(
        &mut self,
        export_id: &str,
    ) -> Option<CapacityManagerDataExportResponse> {
        let export = self.capacity_manager_data_exports.remove(export_id)?;
        let tokens: usize = self
            .capacity_manager_data_export_client_tokens
            .iter()
            .filter(|(_token, named)| named.as_str() == export_id)
            .map(|(token, _named)| token.len())
            .sum();
        self.capacity_manager_data_export_client_tokens
            .retain(|_token, named| named.as_str() != export_id);
        self.release_stored_bytes(
            capacity_manager_data_export_payload_bytes(&export, None).saturating_add(tokens),
        );
        Some(export)
    }
}

// ---------------------------------------------------------------------------
// Route server ENDPOINTS and their BGP peers
// ---------------------------------------------------------------------------
//
// The route servers these hang off are the "Route servers" section above
// ([`RouteServerRecord`]); an endpoint and a peer are stored as the modelled shapes
// themselves, with the one non-terminal state each has moved by the clock sweep.

/// A route server endpoint still coming up: `RouteServerEndpointState`'s `pending`.
pub const ROUTE_SERVER_ENDPOINT_STATE_PENDING: &str = "pending";

/// A route server endpoint that is usable: `RouteServerEndpointState`'s `available`.
pub const ROUTE_SERVER_ENDPOINT_STATE_AVAILABLE: &str = "available";

/// A route server endpoint being taken down: `RouteServerEndpointState`'s `deleting`, which
/// is what `DeleteRouteServerEndpoint` answers with until [`TRANSITION_DELAY`] has passed.
pub const ROUTE_SERVER_ENDPOINT_STATE_DELETING: &str = "deleting";

/// A route server endpoint whose deletion is finished: that same enum's terminal `deleted`
/// member, which the clock owes a `deleting` endpoint one [`TRANSITION_DELAY`] later.
pub const ROUTE_SERVER_ENDPOINT_STATE_DELETED: &str = "deleted";

/// A route server peer still coming up: `RouteServerPeerState`'s `pending`.
pub const ROUTE_SERVER_PEER_STATE_PENDING: &str = "pending";

/// A route server peer whose session is configured: `RouteServerPeerState`'s `available`.
pub const ROUTE_SERVER_PEER_STATE_AVAILABLE: &str = "available";

/// A route server peer being taken down: `RouteServerPeerState`'s `deleting`, which is what
/// `DeleteRouteServerPeer` answers with until [`TRANSITION_DELAY`] has passed.
pub const ROUTE_SERVER_PEER_STATE_DELETING: &str = "deleting";

/// A route server peer whose deletion is finished: that same enum's terminal `deleted`
/// member, which the clock owes a `deleting` peer one [`TRANSITION_DELAY`] later.
pub const ROUTE_SERVER_PEER_STATE_DELETED: &str = "deleted";

/// The liveness detection protocol a peer gets when the request names none: "Defaults to
/// `bgp-keepalive`" (`RouteServerBgpOptionsRequest$PeerLivenessDetection`).
pub const PEER_LIVENESS_BGP_KEEPALIVE: &str = "bgp-keepalive";

/// The other member of `RouteServerPeerLivenessMode`: "`bfd`: An additional Bidirectional
/// Forwarding Detection (BFD) protocol" (same member).
pub const PEER_LIVENESS_BFD: &str = "bfd";

/// A BGP or BFD session that is not established: the `down` member of
/// `RouteServerBgpState` and of `RouteServerBfdState`.
///
/// This is the only status either session can have in this emulator: a session comes up
/// only when a real BGP speaker on the peer address answers, and nothing here speaks BGP,
/// so reporting `up` would be reporting work nobody did.
pub const ROUTE_SERVER_SESSION_DOWN: &str = "down";

/// The `ResourceType` a `CreateRouteServerEndpoint` tag specification may name.
pub const RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT: &str = "route-server-endpoint";

/// The `ResourceType` a `CreateRouteServerPeer` tag specification may name.
pub const RESOURCE_TYPE_ROUTE_SERVER_PEER: &str = "route-server-peer";

/// The largest number of route server endpoints this in-memory store holds, and the same
/// ceiling for their peers.
///
/// A capacity bound in the same style as [`MAX_TRANSIT_GATEWAYS`] and
/// [`MAX_STORED_ROUTE_SERVERS`], not an AWS quota: each record lives in this process's
/// memory, so the number a caller can create has to be finite. It is well above the
/// published per-Region route server quota, so a test that creates more than a real
/// account's default quota is not refused for a reason this emulator cannot have.
pub const MAX_ROUTE_SERVER_RECORDS: usize = 1_000;

/// EC2's error code for a request naming a route server endpoint id this account holds no
/// endpoint for — the code the service answers `CreateRouteServerPeer` with when its
/// `RouteServerEndpointId` resolves to nothing.
pub const INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND: &str =
    "InvalidRouteServerEndpointId.NotFound";

/// EC2's error code for a request naming a route server peer id this account holds no peer
/// for.
pub const INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND: &str = "InvalidRouteServerPeerId.NotFound";

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_route_server_endpoint`] needs that it
/// does not derive itself.
#[derive(Debug, Clone)]
pub struct RouteServerEndpointSpec {
    /// "The ID of the route server for which to create an endpoint"
    /// (`CreateRouteServerEndpointRequest$RouteServerId`), already resolved against the
    /// store.
    pub route_server_id: String,
    /// "The ID of the subnet to place the route server endpoint into" (same request).
    pub subnet_id: String,
    /// The VPC of that subnet, read off the stored subnet rather than taken from the
    /// request: "The ID of the VPC containing the endpoint"
    /// (`RouteServerEndpoint$VpcId`).
    pub vpc_id: Option<String>,
    /// The address of the endpoint's elastic network interface, taken from the subnet's
    /// own CIDR block by [`ComAmazonawsEc2Amazonec2State::mint_route_server_eni_address`].
    pub eni_address: Option<String>,
    /// The tags of a `route-server-endpoint` tag specification.
    pub tags: Vec<Tag>,
}

/// Everything [`ComAmazonawsEc2Amazonec2State::insert_route_server_peer`] needs that it
/// does not derive itself. Every fact about the ENDPOINT the peer belongs to is read off
/// the stored endpoint by the insert, so a peer can never name a different route server,
/// VPC or subnet than its endpoint does.
#[derive(Debug, Clone)]
pub struct RouteServerPeerSpec {
    /// "The IPv4 address of the peer device" (`CreateRouteServerPeerRequest$PeerAddress`).
    pub peer_address: String,
    /// "The Border Gateway Protocol (BGP) Autonomous System Number (ASN) for the
    /// appliance" (`RouteServerBgpOptionsRequest$PeerAsn`).
    pub peer_asn: i64,
    /// The resolved liveness detection protocol. A `&'static str` so only a member of the
    /// modelled `RouteServerPeerLivenessMode` catalog can be stored.
    pub peer_liveness_detection: &'static str,
    /// The tags of a `route-server-peer` tag specification.
    pub tags: Vec<Tag>,
}

/// The caller-chosen bytes one stored route server endpoint holds: its tags. The ids, the
/// ENI address and the state are this module's own.
pub fn route_server_endpoint_payload_bytes(endpoint: &RouteServerEndpoint) -> usize {
    tag_list_bytes(endpoint.tags.as_deref().unwrap_or_default())
}

/// The caller-chosen bytes one stored route server peer holds: its tags and the peer
/// address the request named.
pub fn route_server_peer_payload_bytes(peer: &RouteServerPeer) -> usize {
    tag_list_bytes(peer.tags.as_deref().unwrap_or_default())
        .saturating_add(peer.peer_address.as_deref().map_or(0, str::len))
}

/// The liveness detection protocol a peer the request asked for `requested` with gets.
///
/// "Defaults to `bgp-keepalive`" (`RouteServerBgpOptionsRequest$PeerLivenessDetection`),
/// and a word no member of `RouteServerPeerLivenessMode` has is refused rather than stored.
pub fn route_server_peer_liveness<E>(
    requested: Option<&str>,
) -> Result<&'static str, Scaffolded<E>> {
    match requested {
        None | Some(PEER_LIVENESS_BGP_KEEPALIVE) => Ok(PEER_LIVENESS_BGP_KEEPALIVE),
        Some(PEER_LIVENESS_BFD) => Ok(PEER_LIVENESS_BFD),
        Some(value) => Err(invalid_parameter_value(format!(
            "Value ({value}) for parameter bgpOptions.peerLivenessDetection is invalid. \
             Valid values are {PEER_LIVENESS_BFD} and {PEER_LIVENESS_BGP_KEEPALIVE}."
        ))),
    }
}

/// The ASN `asn` if the published range admits it, or the refusal EC2 answers for one it
/// does not.
///
/// "Valid values are from 1 to 4294967295" is published identically for the Amazon side of
/// the session ([`AMAZON_SIDE_ASN_RANGE`], `CreateRouteServerRequest$AmazonSideAsn`) and for
/// the appliance's side (`RouteServerBgpOptionsRequest$PeerAsn`), so the peer's side is held
/// to that same one pair of published bounds and `member` only names which one the message
/// is about.
pub fn require_route_server_asn<E>(asn: i64, member: &str) -> Result<i64, Scaffolded<E>> {
    let (min, max) = AMAZON_SIDE_ASN_RANGE;
    if (min..=max).contains(&asn) {
        return Ok(asn);
    }
    Err(invalid_parameter_value(format!(
        "Value ({asn}) for parameter {member} is invalid. Valid values are from {min} to {max}."
    )))
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route server endpoint fits inside the store's ceiling
    /// ([`MAX_ROUTE_SERVER_RECORDS`]).
    ///
    /// The same contract as [`Self::has_transit_gateway_capacity`]: a create asks first and
    /// refuses with [`route_server_limit_exceeded`] when it answers `false`. The endpoint
    /// and the peer store are asked about separately, so a full store of one kind cannot
    /// refuse a create of another.
    pub fn has_route_server_endpoint_capacity(&self) -> bool {
        self.route_server_endpoints.len() < MAX_ROUTE_SERVER_RECORDS
    }

    /// Whether another route server peer fits inside that same ceiling.
    pub fn has_route_server_peer_capacity(&self) -> bool {
        self.route_server_peers.len() < MAX_ROUTE_SERVER_RECORDS
    }

    /// Insert a route server endpoint in `pending` and answer the
    /// `RouteServerEndpointId` it was given.
    ///
    /// The endpoint's elastic network interface is the service's, not the caller's, so its
    /// id is minted here beside the endpoint's own.
    pub fn insert_route_server_endpoint(&mut self, spec: RouteServerEndpointSpec) -> String {
        let route_server_endpoint_id = self.next_id("rse");
        let eni_id = self.next_id(NETWORK_INTERFACE_ID_PREFIX);
        let endpoint = RouteServerEndpoint {
            route_server_id: Some(spec.route_server_id),
            route_server_endpoint_id: Some(route_server_endpoint_id.clone()),
            vpc_id: spec.vpc_id,
            subnet_id: Some(spec.subnet_id),
            eni_id: Some(eni_id),
            eni_address: spec.eni_address,
            state: Some(ROUTE_SERVER_ENDPOINT_STATE_PENDING.to_string()),
            // "The reason for any failure in endpoint creation or operation"
            // (`RouteServerEndpoint$FailureReason`): this endpoint is coming up, so there
            // is no failure to report.
            failure_reason: None,
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(route_server_endpoint_payload_bytes(&endpoint));
        self.route_server_endpoints
            .insert(route_server_endpoint_id.clone(), endpoint);
        self.route_server_endpoint_available_at.insert(
            route_server_endpoint_id.clone(),
            Instant::now() + TRANSITION_DELAY,
        );
        route_server_endpoint_id
    }

    /// Start taking down the route server endpoint `route_server_endpoint_id` and answer it
    /// as the store then holds it — `None` when the account holds no endpoint of that id.
    ///
    /// The counterpart of [`Self::begin_route_server_peer_deletion`], on the same clock and
    /// under the same rule: the endpoint is put into the non-terminal
    /// [`ROUTE_SERVER_ENDPOINT_STATE_DELETING`] now and the terminal
    /// [`ROUTE_SERVER_ENDPOINT_STATE_DELETED`] falls due one [`TRANSITION_DELAY`] later,
    /// which is STORED rather than slept for, so a caller reading straight after the delete
    /// still sees the endpoint going down. Any `pending` -> `available` move still owed is
    /// dropped here: an endpoint asked to delete while it was coming up must not come up
    /// afterwards.
    ///
    /// The record STAYS in the store, because an endpoint in either delete state is still
    /// reported — `deleting` and `deleted` are both `RouteServerEndpointState` members, which
    /// the `state` filter of `DescribeRouteServerEndpoints` selects on — so its
    /// caller-chosen bytes stay charged to the store's budget, which still holds them, its
    /// network interface address stays held, and it still counts against
    /// [`MAX_ROUTE_SERVER_RECORDS`], which bounds the records this process holds.
    pub fn begin_route_server_endpoint_deletion(
        &mut self,
        route_server_endpoint_id: &str,
    ) -> Option<RouteServerEndpoint> {
        let endpoint = self
            .route_server_endpoints
            .get_mut(route_server_endpoint_id)?;
        endpoint.state = Some(ROUTE_SERVER_ENDPOINT_STATE_DELETING.to_string());
        let endpoint = endpoint.clone();
        self.route_server_endpoint_available_at
            .remove(route_server_endpoint_id);
        self.route_server_endpoint_deleted_at.insert(
            route_server_endpoint_id.to_string(),
            Instant::now() + TRANSITION_DELAY,
        );
        Some(endpoint)
    }

    /// Insert a route server peer of the endpoint `route_server_endpoint_id` in `pending`
    /// and answer the `RouteServerPeerId` it was given, or `None` when no endpoint of that
    /// id is stored.
    ///
    /// The peer's route server, VPC, subnet and endpoint network interface are read off the
    /// STORED endpoint, so they cannot disagree with it whatever the request said.
    pub fn insert_route_server_peer(
        &mut self,
        route_server_endpoint_id: &str,
        spec: RouteServerPeerSpec,
    ) -> Option<String> {
        let endpoint = self
            .route_server_endpoints
            .get(route_server_endpoint_id)?
            .clone();
        let route_server_peer_id = self.next_id("rsp");
        let peer = RouteServerPeer {
            route_server_peer_id: Some(route_server_peer_id.clone()),
            route_server_endpoint_id: endpoint.route_server_endpoint_id.clone(),
            route_server_id: endpoint.route_server_id.clone(),
            vpc_id: endpoint.vpc_id.clone(),
            subnet_id: endpoint.subnet_id.clone(),
            state: Some(ROUTE_SERVER_PEER_STATE_PENDING.to_string()),
            // Nothing has failed: "The reason for any failure in peer creation or
            // operation" (`RouteServerPeer$FailureReason`).
            failure_reason: None,
            endpoint_eni_id: endpoint.eni_id.clone(),
            endpoint_eni_address: endpoint.eni_address.clone(),
            peer_address: Some(spec.peer_address),
            bgp_options: Some(RouteServerBgpOptions {
                peer_asn: Some(spec.peer_asn),
                peer_liveness_detection: Some(spec.peer_liveness_detection.to_string()),
            }),
            // The session with the appliance is not established — nothing in this emulator
            // speaks BGP to the peer address — so it is reported as
            // [`ROUTE_SERVER_SESSION_DOWN`] rather than as a session nobody opened.
            bgp_status: Some(RouteServerBgpStatus {
                status: Some(ROUTE_SERVER_SESSION_DOWN.to_string()),
            }),
            // BFD is a session only a peer that asked for that liveness mode has, so a
            // `bgp-keepalive` peer carries no BFD status at all.
            bfd_status: (spec.peer_liveness_detection == PEER_LIVENESS_BFD).then(|| {
                RouteServerBfdStatus {
                    status: Some(ROUTE_SERVER_SESSION_DOWN.to_string()),
                }
            }),
            tags: Some(spec.tags),
        };
        self.add_stored_bytes(route_server_peer_payload_bytes(&peer));
        self.route_server_peers
            .insert(route_server_peer_id.clone(), peer);
        self.route_server_peer_available_at.insert(
            route_server_peer_id.clone(),
            Instant::now() + TRANSITION_DELAY,
        );
        Some(route_server_peer_id)
    }

    /// Start taking down the route server `route_server_id` and answer it as the store then
    /// holds it — `None` when the account holds no route server of that id.
    ///
    /// The counterpart of [`Self::begin_route_server_endpoint_deletion`], on the same clock
    /// and under the same rule: the route server is put into the non-terminal
    /// [`ROUTE_SERVER_STATE_DELETING`] now and the terminal [`ROUTE_SERVER_STATE_DELETED`]
    /// falls due one [`TRANSITION_DELAY`] later, which is STORED rather than slept for, so a
    /// caller reading straight after the delete still sees the route server going down. The
    /// delete state is reported whatever moves the create or a modification still owed, so a
    /// route server asked to delete while it was coming up does not come up afterwards.
    ///
    /// The record STAYS in the store, because a route server in either delete state is still
    /// reported — `deleting` and `deleted` are both `RouteServerState` members
    /// (`smithy_query(query="shape",shape="RouteServerState")`), and the `state` filter of
    /// `DescribeRouteServers` selects on them — so its caller-chosen bytes stay charged to
    /// the store's budget, which still holds them, and it still counts against
    /// [`MAX_STORED_ROUTE_SERVERS`].
    ///
    /// Its endpoints and peers are resources OF the route server, so they go down with it on
    /// the same clock: one already in a delete state is left on the instant its own delete
    /// set, so a cascade cannot restart a move that was already due.
    ///
    /// The `ClientToken` that named this route server keeps naming it, unlike the tokens a
    /// removal path gives back ([`Self::remove_traffic_mirror_filter`]): the record is still
    /// there, so a create replayed with that token reports this route server — in the state
    /// it has reached — rather than minting a second one, and it still counts against
    /// [`MAX_STORED_ROUTE_SERVERS`] for as long as it is reported.
    pub fn begin_route_server_deletion(&mut self, route_server_id: &str) -> Option<RouteServer> {
        let now = Instant::now();
        let record = self.route_servers.get_mut(route_server_id)?;
        record.deleted_at = Some(now + TRANSITION_DELAY);
        let view = record.view_at(now);
        let endpoints: Vec<String> = self
            .route_server_endpoints
            .iter()
            .filter(|(_, endpoint)| {
                endpoint.route_server_id.as_deref() == Some(route_server_id)
                    && !matches!(
                        endpoint.state.as_deref(),
                        Some(ROUTE_SERVER_ENDPOINT_STATE_DELETING)
                            | Some(ROUTE_SERVER_ENDPOINT_STATE_DELETED)
                    )
            })
            .map(|(endpoint_id, _)| endpoint_id.clone())
            .collect();
        for endpoint_id in &endpoints {
            self.begin_route_server_endpoint_deletion(endpoint_id);
        }
        let peers: Vec<String> = self
            .route_server_peers
            .iter()
            .filter(|(_, peer)| {
                peer.route_server_id.as_deref() == Some(route_server_id)
                    && !matches!(
                        peer.state.as_deref(),
                        Some(ROUTE_SERVER_PEER_STATE_DELETING)
                            | Some(ROUTE_SERVER_PEER_STATE_DELETED)
                    )
            })
            .map(|(peer_id, _)| peer_id.clone())
            .collect();
        for peer_id in &peers {
            self.begin_route_server_peer_deletion(peer_id);
        }
        Some(view)
    }

    /// Start taking down the route server peer `route_server_peer_id` and answer it as the
    /// store then holds it — `None` when the account holds no peer of that id.
    ///
    /// The deletion is a CLOCK-OWED move, like the creation: the peer is put into the
    /// non-terminal [`ROUTE_SERVER_PEER_STATE_DELETING`] now and the terminal
    /// [`ROUTE_SERVER_PEER_STATE_DELETED`] falls due one [`TRANSITION_DELAY`] later, which
    /// is STORED rather than slept for, so a caller reading straight after the delete still
    /// sees the peer going down. Any `pending` -> `available` move still owed is dropped
    /// here: a peer asked to delete while it was coming up must not come up afterwards.
    ///
    /// The record STAYS in the store, because a peer in either delete state is still
    /// reported — `deleting` and `deleted` are both `RouteServerPeerState` members, which
    /// the `state` filter of `DescribeRouteServerPeers` selects on — so its caller-chosen
    /// bytes stay charged to the store's budget, which still holds them, and it still counts
    /// against [`MAX_ROUTE_SERVER_RECORDS`], which bounds the records this process holds.
    pub fn begin_route_server_peer_deletion(
        &mut self,
        route_server_peer_id: &str,
    ) -> Option<RouteServerPeer> {
        let peer = self.route_server_peers.get_mut(route_server_peer_id)?;
        peer.state = Some(ROUTE_SERVER_PEER_STATE_DELETING.to_string());
        let peer = peer.clone();
        self.route_server_peer_available_at
            .remove(route_server_peer_id);
        self.route_server_peer_deleted_at.insert(
            route_server_peer_id.to_string(),
            Instant::now() + TRANSITION_DELAY,
        );
        Some(peer)
    }

    /// The address the next route server endpoint in the subnet `cidr_block` gets for its
    /// elastic network interface, or `None` when the block holds no free address.
    ///
    /// The lowest assignable address of the block that nothing in the account already holds
    /// — not another route server endpoint, and not an instance's primary private address —
    /// so one subnet cannot give the same address to two interfaces. That is
    /// [`Self::free_subnet_address`], the one scan every subnet address comes out of: AWS
    /// "reserves the first four IPv4 addresses and the last IPv4 address in each subnet
    /// CIDR block" (the [`SUBNET_RESERVED_ADDRESSES`] this crate already counts against a
    /// subnet's available addresses), and the address is the service's choice, never the
    /// caller's.
    pub fn mint_route_server_eni_address(&self, cidr_block: &str) -> Option<String> {
        self.free_subnet_address(cidr_block)
    }
}

/// Refuse a request that named a route server endpoint id no endpoint of the account has:
/// HTTP 400 `InvalidRouteServerEndpointId.NotFound`, naming the resource type the way EC2
/// names it ([`RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT`]) — the wording rule
/// [`invalid_route_server_id_not_found`] records for the whole family.
pub fn invalid_route_server_endpoint_id_not_found<E>(endpoint_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_SERVER_ENDPOINT_ID_NOT_FOUND,
        400,
        format!(
            "The {RESOURCE_TYPE_ROUTE_SERVER_ENDPOINT} ID '{endpoint_id}' does not exist"
        ),
    )
}

/// Refuse a request that named a route server peer id no peer of the account has: HTTP 400
/// `InvalidRouteServerPeerId.NotFound`, named the same way
/// ([`RESOURCE_TYPE_ROUTE_SERVER_PEER`]).
pub fn invalid_route_server_peer_id_not_found<E>(peer_id: &str) -> Scaffolded<E> {
    Scaffolded::service_error(
        INVALID_ROUTE_SERVER_PEER_ID_NOT_FOUND,
        400,
        format!("The {RESOURCE_TYPE_ROUTE_SERVER_PEER} ID '{peer_id}' does not exist"),
    )
}

// ---------------------------------------------------------------------------
// Route server associations
// ---------------------------------------------------------------------------

/// An association that is being established: the `associating` member of
/// `RouteServerAssociationState`
/// (`smithy_query(query="shape",shape="RouteServerAssociationState")`).
pub const ROUTE_SERVER_ASSOCIATION_STATE_ASSOCIATING: &str = "associating";

/// An association that is established: that same enum's `associated` member.
pub const ROUTE_SERVER_ASSOCIATION_STATE_ASSOCIATED: &str = "associated";

/// One stored route server association: the instant the clock owes it its `associating` ->
/// `associated` move, and nothing else.
///
/// The (route server, VPC) PAIR is the whole identity of an association — "A route server
/// association is the connection established between a route server and a VPC"
/// (`RouteServerAssociation`), and the modelled shape carries no association id — so the
/// pair is the key of [`ComAmazonawsEc2Amazonec2State::route_server_associations`] and is
/// not repeated on the record. `State` is a fact of the record's age, so
/// [`RouteServerAssociationRecord::view_at`] derives it on read rather than letting two
/// writers store two different words for one association.
///
/// The record holds NO caller-chosen bytes: both ids name resources the store already
/// holds and charges for, so an association adds nothing to the store's byte budget —
/// only to the record count [`ComAmazonawsEc2Amazonec2State::has_route_server_association_capacity`]
/// bounds.
#[derive(Debug, Clone)]
pub struct RouteServerAssociationRecord {
    /// The instant from which this association's `associating` -> `associated` move is
    /// due. Monotonic ([`Instant`]) for the reason [`RouteServerRecord::available_at`] is:
    /// it is only ever compared against the clock, never reported, so a wall-clock
    /// adjustment cannot move an association back to `associating`.
    pub associated_at: Instant,
}

impl RouteServerAssociationRecord {
    /// The association of `route_server_id` with `vpc_id` as of `now`: the pair it joins,
    /// with the state the clock has reached. A read observes the move the clock already
    /// owed without writing anything.
    pub fn view_at(
        &self,
        route_server_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> RouteServerAssociation {
        RouteServerAssociation {
            route_server_id: Some(route_server_id.to_string()),
            vpc_id: Some(vpc_id.to_string()),
            state: Some(self.state_at(now).to_string()),
        }
    }

    /// The association's `RouteServerAssociationState` at `now`.
    pub fn state_at(&self, now: Instant) -> &'static str {
        if now < self.associated_at {
            ROUTE_SERVER_ASSOCIATION_STATE_ASSOCIATING
        } else {
            ROUTE_SERVER_ASSOCIATION_STATE_ASSOCIATED
        }
    }
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether another route server association fits inside the store's record ceiling
    /// ([`MAX_ROUTE_SERVER_RECORDS`]) — the same contract as
    /// [`Self::has_route_server_endpoint_capacity`], asked of its own store so a full
    /// store of one kind cannot refuse a write of another.
    pub fn has_route_server_association_capacity(&self) -> bool {
        self.route_server_associations.len() < MAX_ROUTE_SERVER_RECORDS
    }

    /// The route server the VPC `vpc_id` is already associated with, or `None` when it has
    /// no association at all.
    ///
    /// The ONE place that question is answered, so an associate and anything else that has
    /// to know whether a VPC is already served by a route server cannot disagree about it.
    pub fn route_server_association_of_vpc(&self, vpc_id: &str) -> Option<&str> {
        self.route_server_associations
            .keys()
            .find(|(_, associated_vpc)| associated_vpc == vpc_id)
            .map(|(route_server_id, _)| route_server_id.as_str())
    }

    /// Associate `route_server_id` with `vpc_id` from `now`, and answer the association as
    /// of that instant.
    ///
    /// The association is written in the non-terminal `associating` state with the clock
    /// owing it `associated` one [`TRANSITION_DELAY`] later, the same span every other
    /// clock-owed move of this crate takes, so a caller reading straight after the write
    /// still sees `associating`.
    pub fn insert_route_server_association(
        &mut self,
        route_server_id: &str,
        vpc_id: &str,
        now: Instant,
    ) -> RouteServerAssociation {
        let record = RouteServerAssociationRecord {
            associated_at: now + TRANSITION_DELAY,
        };
        let view = record.view_at(route_server_id, vpc_id, now);
        self.route_server_associations
            .insert((route_server_id.to_string(), vpc_id.to_string()), record);
        view
    }

    /// Every association of the route server `route_server_id` as of `now`, in `VpcId`
    /// order — the order the store itself is kept in.
    pub fn route_server_associations_at(
        &self,
        route_server_id: &str,
        now: Instant,
    ) -> Vec<RouteServerAssociation> {
        self.route_server_associations
            .iter()
            .filter(|((associated_server, _), _)| associated_server == route_server_id)
            .map(|((associated_server, vpc_id), record)| {
                record.view_at(associated_server, vpc_id, now)
            })
            .collect()
    }
}

// ---------------------------------------------------------------------------
// Longer-ID (17-character ID) format settings
// ---------------------------------------------------------------------------

/// Every resource type that has a longer-ID setting, as
/// `DescribePrincipalIdFormatRequest$Resources` publishes them ("The type of resource:
/// `bundle` | `conversion-task` | … | `vpn-gateway`") — the same list
/// `DescribePrincipalIdFormat`'s own documentation gives for "the following resource types
/// support longer IDs".
///
/// Held in the order that documentation lists them in, which is also sorted, so it is the
/// stable order a describe reports one principal's statuses in.
pub const LONGER_ID_RESOURCE_TYPES: [&str; 31] = [
    "bundle",
    "conversion-task",
    "customer-gateway",
    "dhcp-options",
    "elastic-ip-allocation",
    "elastic-ip-association",
    "export-task",
    "flow-log",
    "image",
    "import-task",
    "instance",
    "internet-gateway",
    "network-acl",
    "network-acl-association",
    "network-interface",
    "network-interface-attachment",
    "prefix-list",
    "reservation",
    "route-table",
    "route-table-association",
    "security-group",
    "snapshot",
    "subnet",
    "subnet-cidr-block-association",
    "volume",
    "vpc",
    "vpc-cidr-block-association",
    "vpc-endpoint",
    "vpc-peering-connection",
    "vpn-connection",
    "vpn-gateway",
];

/// The resource types whose longer-ID setting a caller can still CHANGE, as
/// `ModifyIdentityIdFormatRequest$Resource` publishes them: "This request can only be used
/// to modify longer ID settings for resource types that are within the opt-in period.
/// Resources currently in their opt-in period include: `bundle` | … | `vpn-gateway`".
///
/// That is [`LONGER_ID_RESOURCE_TYPES`] without
/// [`PERMANENT_LONGER_ID_RESOURCE_TYPES`], the four the modify list leaves out.
pub const OPT_IN_LONGER_ID_RESOURCE_TYPES: [&str; 27] = [
    "bundle",
    "conversion-task",
    "customer-gateway",
    "dhcp-options",
    "elastic-ip-allocation",
    "elastic-ip-association",
    "export-task",
    "flow-log",
    "image",
    "import-task",
    "internet-gateway",
    "network-acl",
    "network-acl-association",
    "network-interface",
    "network-interface-attachment",
    "prefix-list",
    "route-table",
    "route-table-association",
    "security-group",
    "subnet",
    "subnet-cidr-block-association",
    "vpc",
    "vpc-cidr-block-association",
    "vpc-endpoint",
    "vpc-peering-connection",
    "vpn-connection",
    "vpn-gateway",
];

/// The resource types [`LONGER_ID_RESOURCE_TYPES`] holds that
/// `ModifyIdentityIdFormatRequest$Resource` does NOT: the ones past their opt-in period,
/// which are "permanently switched over to using longer IDs"
/// (`IdFormat$Deadline`) — so their setting is always on and no request can change it.
pub const PERMANENT_LONGER_ID_RESOURCE_TYPES: [&str; 4] =
    ["instance", "reservation", "snapshot", "volume"];

/// `all-current`: "use the `all-current` option to include all resource types that are
/// currently within their opt-in period for longer IDs"
/// (`ModifyIdentityIdFormatRequest$Resource`) — every member of
/// [`OPT_IN_LONGER_ID_RESOURCE_TYPES`] in one request.
pub const ALL_CURRENT_RESOURCES: &str = "all-current";

/// `all`: "Specify `all` to modify the ID format for all IAM users, IAM roles, and the
/// root user of the account" (`ModifyIdentityIdFormatRequest$PrincipalArn`).
pub const ALL_PRINCIPALS: &str = "all";

/// The longer-ID setting in force for a resource type nobody has overridden.
///
/// `true`, because every id this emulator mints is already of the longer form —
/// [`ComAmazonawsEc2Amazonec2State::next_id`] appends 17 lowercase hexadecimal digits to
/// every resource-id prefix — so reporting anything else for a type nobody has set would
/// describe ids this service does not hand out.
pub const DEFAULT_USE_LONG_IDS: bool = true;

/// The resource portion of an account's root-user ARN.
pub const IAM_ROOT_RESOURCE: &str = "root";

/// The resource prefix of an IAM ROLE's ARN (`arn:aws:iam::<account>:role/<name>`).
pub const IAM_ROLE_RESOURCE_PREFIX: &str = "role/";

/// The resource prefix of an IAM INSTANCE PROFILE's ARN
/// (`arn:aws:iam::<account>:instance-profile/<name>`) — the kind of IAM entity an instance
/// runs as.
pub const IAM_INSTANCE_PROFILE_RESOURCE_PREFIX: &str = "instance-profile/";

/// The resource prefixes of the two kinds of IAM principal that are not the root user,
/// as an IAM ARN spells them (`arn:aws:iam::<account>:user/<name>`,
/// `…:role/<name>`).
pub(super) const IAM_PRINCIPAL_PREFIXES: [&str; 2] = ["user/", IAM_ROLE_RESOURCE_PREFIX];

/// The most bytes the path-and-name part of an IAM user or role ARN may carry: the IAM
/// quotas on the two ("Path: 512 characters", "Role name / User name: 64 characters" — IAM
/// identifiers and quotas), so an ARN a request names is bounded before it is stored.
pub(super) const IAM_PRINCIPAL_PATH_AND_NAME_MAX: usize = 512 + 64;

/// The ARN of the account's root user — the principal `DescribePrincipalIdFormat` always
/// reports ("Describes the ID format settings for the root user and all IAM roles and IAM
/// users that have explicitly specified a longer ID … preference").
///
/// Built through [`::engine::emulator_arn`] with the empty Region IAM's global ARNs carry,
/// so it names the same account every resource of this store is owned by.
pub fn root_principal_arn() -> String {
    ::engine::emulator_arn("iam", "", IAM_ROOT_RESOURCE)
}

/// The [`LONGER_ID_RESOURCE_TYPES`] member `requested` names, or `None` when it names
/// none.
///
/// Answering a `&'static str` out of this module's own catalog is what keeps request text
/// out of the store: a writer stores what this returns.
pub fn longer_id_resource_type(requested: &str) -> Option<&'static str> {
    LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .find(|resource| *resource == requested)
}

/// The [`OPT_IN_LONGER_ID_RESOURCE_TYPES`] member `requested` names, or `None` when it
/// names none — the types a modify may write, for the same reason as
/// [`longer_id_resource_type`].
pub fn opt_in_longer_id_resource_type(requested: &str) -> Option<&'static str> {
    OPT_IN_LONGER_ID_RESOURCE_TYPES
        .into_iter()
        .find(|resource| *resource == requested)
}

/// Is `resource` a type whose longer-ID setting is permanently on
/// ([`PERMANENT_LONGER_ID_RESOURCE_TYPES`])?
pub fn longer_ids_are_permanent_for(resource: &str) -> bool {
    PERMANENT_LONGER_ID_RESOURCE_TYPES.contains(&resource)
}

/// Is `arn` the ARN of an IAM principal of one of `prefixes` — `root` too when `allow_root`
/// — and, when `this_account_only`, of the account this store belongs to?
///
/// `arn:<partition>:iam::<account>:root`, `…:user/<path><name>` or `…:role/<path><name>`,
/// with the empty Region segment IAM's global ARNs carry. The path-and-name part is bounded
/// by [`IAM_PRINCIPAL_PATH_AND_NAME_MAX`] and limited to the characters IAM admits in a user
/// or role name, so a request cannot name an unbounded principal.
///
/// The ONE reading of an IAM principal ARN in this crate, so every operation that takes one
/// agrees on its shape: [`iam_principal_arn_is_well_formed`] for the ID format operations'
/// "an IAM user, IAM role, or the root user", and [`iam_role_arn_is_well_formed`] for the
/// members that take a role the SERVICE assumes.
pub(super) fn iam_principal_arn_names(
    arn: &str,
    prefixes: &[&str],
    allow_root: bool,
    this_account_only: bool,
) -> bool {
    let fields: Vec<&str> = arn.split(':').collect();
    let [prefix, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let partition_is_well_formed = !partition.is_empty()
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-');
    let account_is_well_formed = if this_account_only {
        *account == ::engine::emulator_account_id()
    } else {
        !account.is_empty() && account.bytes().all(|byte| byte.is_ascii_digit())
    };
    if *prefix != "arn"
        || !partition_is_well_formed
        || *service != "iam"
        || !region.is_empty()
        || !account_is_well_formed
    {
        return false;
    }
    if allow_root && *resource == IAM_ROOT_RESOURCE {
        return true;
    }
    prefixes
        .iter()
        .filter_map(|kind| resource.strip_prefix(kind))
        .any(|path_and_name| {
            !path_and_name.is_empty()
                && path_and_name.len() <= IAM_PRINCIPAL_PATH_AND_NAME_MAX
                && !path_and_name.ends_with('/')
                && path_and_name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || b"+=,.@_-/".contains(&byte)
                })
        })
}

/// Is `arn` the ARN of an IAM principal of the account this store belongs to — "an IAM
/// user, IAM role, or the root user" (`ModifyIdentityIdFormatRequest$PrincipalArn`)?
///
/// `<account>` has to be the account this emulator serves: the ID format settings of another
/// account's principals are not this account's to set.
pub fn iam_principal_arn_is_well_formed(arn: &str) -> bool {
    iam_principal_arn_names(arn, &IAM_PRINCIPAL_PREFIXES, true, true)
}

/// Is `arn` the ARN of an IAM ROLE — `arn:<partition>:iam::<account>:role/<path><name>`?
///
/// The shape of a member that names a role the SERVICE assumes, as
/// `SpotFleetRequestConfigData$IamFleetRole` does ("the Amazon Resource Name (ARN) of an
/// Identity and Access Management (IAM) role that grants the Spot Fleet the permission to
/// request, launch, terminate, and tag instances on your behalf"). The ACCOUNT is not held to
/// this emulator's own: the role is the caller's, and a request naming a role of the account
/// it is made for is not this emulator's to second-guess. The root user is not a role, so it
/// is not one of these either.
pub fn iam_role_arn_is_well_formed(arn: &str) -> bool {
    iam_principal_arn_names(arn, &[IAM_ROLE_RESOURCE_PREFIX], false, false)
}

/// Is `arn` the ARN of an IAM INSTANCE PROFILE —
/// `arn:<partition>:iam::<account>:instance-profile/<path><name>`?
///
/// The shape of a member that names the instance profile an INSTANCE runs as
/// (`IamInstanceProfileSpecification$Arn` — "the Amazon Resource Name (ARN) of the instance
/// profile"). Read through the same one reading as the user and role ARNs above, so the
/// partition, the empty Region segment IAM's global ARNs carry and the bound on the
/// path-and-name part are decided in one place. The ACCOUNT is not held to this emulator's
/// own, for the reason [`iam_role_arn_is_well_formed`] gives.
pub fn iam_instance_profile_arn_is_well_formed(arn: &str) -> bool {
    iam_principal_arn_names(arn, &[IAM_INSTANCE_PROFILE_RESOURCE_PREFIX], false, false)
}

impl ComAmazonawsEc2Amazonec2State {
    /// Whether longer IDs are in force for `resource` for the principal `arn`.
    ///
    /// The three rules the ID format API documents, in order: a type past its opt-in
    /// period is permanently on ([`PERMANENT_LONGER_ID_RESOURCE_TYPES`]); otherwise the
    /// principal's own explicit setting wins; otherwise "all IAM roles and IAM users
    /// default to the same ID settings as the root user, unless they explicitly override
    /// the settings" (`DescribePrincipalIdFormat`), and the root user itself falls back to
    /// [`DEFAULT_USE_LONG_IDS`].
    ///
    /// A pure read: nothing about the setting changes with time, so no caller of this can
    /// change what a later one sees.
    pub fn use_long_ids_for(&self, arn: &str, resource: &str) -> bool {
        self.use_long_ids_against_root(arn, resource, &root_principal_arn())
    }

    /// [`Self::use_long_ids_for`] with the root user's ARN already in hand, so a caller
    /// answering for many principals and many resource types formats it ONCE rather than
    /// per status.
    pub(super) fn use_long_ids_against_root(&self, arn: &str, resource: &str, root: &str) -> bool {
        if longer_ids_are_permanent_for(resource) {
            return true;
        }
        let setting_of = |principal: &str| {
            self.id_format_overrides
                .get(principal)
                .and_then(|overrides| overrides.get(resource))
                .copied()
        };
        setting_of(arn)
            .or_else(|| (arn != root).then(|| setting_of(root)).flatten())
            .unwrap_or(DEFAULT_USE_LONG_IDS)
    }

    /// The statuses ONE principal is reported with for every type in `resources`:
    /// "information about the ID format for the resources"
    /// (`DescribeIdentityIdFormatResult$Statuses`), in the order `resources` gives.
    ///
    /// The one projection of a principal's settings into the modelled
    /// [`crate::ops::types::IdFormat`] shape, so `DescribeIdentityIdFormat` and the
    /// per-principal statuses of [`Self::principal_id_formats`] cannot report one
    /// principal's setting two ways.
    pub fn identity_id_formats(
        &self,
        arn: &str,
        resources: &[&'static str],
    ) -> Vec<crate::ops::types::IdFormat> {
        self.id_formats_against_root(arn, resources, &root_principal_arn())
    }

    /// [`Self::identity_id_formats`] with the root user's ARN already in hand, so a caller
    /// answering for many principals formats it ONCE rather than per principal.
    fn id_formats_against_root(
        &self,
        arn: &str,
        resources: &[&'static str],
        root: &str,
    ) -> Vec<crate::ops::types::IdFormat> {
        resources
            .iter()
            .map(|resource| crate::ops::types::IdFormat {
                // "The date in UTC at which you are permanently switched over to using
                // longer IDs. If a deadline is not yet available for this resource type,
                // this field is not returned" (`IdFormat$Deadline`): this emulator mints
                // longer ids for every type already, so there is no future switch-over
                // date to report for any of them.
                deadline: None,
                resource: Some((*resource).to_string()),
                use_long_ids: Some(self.use_long_ids_against_root(arn, resource, root)),
            })
            .collect()
    }

    /// The principals `DescribePrincipalIdFormat` reports, each with its setting for every
    /// type in `resources`.
    ///
    /// "the root user and all IAM roles and IAM users that have explicitly specified a
    /// longer ID (17-character ID) preference": the root user is always reported, whether
    /// or not anything has ever been set for it, and every other principal only once it
    /// holds an explicit setting. Ordered by ARN — the order
    /// [`Self::id_format_overrides`] is kept in — so one page follows the previous one and
    /// a token can name the ARN the next page starts at.
    pub fn principal_id_formats(
        &self,
        resources: &[&'static str],
    ) -> Vec<crate::ops::types::PrincipalIdFormat> {
        let root = root_principal_arn();
        let mut principals: BTreeSet<&str> = self
            .id_format_overrides
            .iter()
            .filter(|(_, overrides)| !overrides.is_empty())
            .map(|(arn, _)| arn.as_str())
            .collect();
        principals.insert(root.as_str());
        principals
            .into_iter()
            .map(|arn| crate::ops::types::PrincipalIdFormat {
                arn: Some(arn.to_string()),
                statuses: Some(self.id_formats_against_root(arn, resources, &root)),
            })
            .collect()
    }

    /// The caller-chosen bytes storing a setting for `arn` would add: the ARN itself for a
    /// principal this store holds nothing for yet, and nothing for one it already has,
    /// since a further setting of the same principal only adds a `&'static str` key and a
    /// `bool`.
    pub fn id_format_principal_bytes(&self, arn: &str) -> usize {
        if self.id_format_overrides.contains_key(arn) {
            0
        } else {
            arn.len()
        }
    }

    /// Record `use_long_ids` as the principal `arn`'s EXPLICIT setting for `resource`.
    ///
    /// `resource` is a `&'static str`, so only a member of this module's own catalog can
    /// be stored; the ARN is the one the request named, charged against
    /// [`MAX_STORED_BYTES`] by its caller through [`Self::id_format_principal_bytes`].
    pub fn set_id_format(&mut self, arn: &str, resource: &'static str, use_long_ids: bool) {
        self.id_format_overrides
            .entry(arn.to_string())
            .or_default()
            .insert(resource, use_long_ids);
    }

    /// Record `use_long_ids` as the setting of `resource` for EVERY principal of the
    /// account: "Specify `all` to modify the ID format for all IAM users, IAM roles, and
    /// the root user of the account" (`ModifyIdentityIdFormatRequest$PrincipalArn`).
    ///
    /// The root user's setting is what every principal that has not overridden the type
    /// follows ([`Self::use_long_ids_for`]), so writing it is what reaches the principals
    /// this store has never heard of; the principals it HAS heard of are written too, so a
    /// setting one of them overrode earlier cannot survive a request that covers it.
    pub fn set_id_format_for_all_principals(&mut self, resource: &'static str, use_long_ids: bool) {
        self.set_id_format(&root_principal_arn(), resource, use_long_ids);
        for overrides in self.id_format_overrides.values_mut() {
            overrides.insert(resource, use_long_ids);
        }
    }
}
