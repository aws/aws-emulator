// SPDX-License-Identifier: CC0-1.0

//! The NOTEBOOK INSTANCE slice of this service's state: the record
//! `CreateNotebookInstance` writes, the statuses and clock spans it moves through, and
//! the presigned-URL reading its console sign-in needs.
//!
//! A notebook instance is accepted before SageMaker resolves the role it would run as, so
//! the answer about that role travels on the INSTANCE: the create records it
//! ([`NotebookInstance::creation_failure_reason`]) and the clock reveals it as the modelled
//! `Failed` status with the `FailureReason` naming the role, the way a custom image's
//! `CREATE_FAILED` works.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit. Every name is re-exported from [`super`], so a handler keeps
//! naming `crate::state::…`.

use super::*;

/// How long after `CreateNotebookInstance` the clock owes the `Pending` -> `InService`
/// transition. Same shape as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the
/// notebook instance straight after creating it still observes `Pending`, one that waits
/// (as `aws sagemaker wait notebook-instance-in-service` does) observes `InService`.
/// Real SageMaker takes minutes; the span is compressed, never zero.
pub const NOTEBOOK_INSTANCE_IN_SERVICE_AFTER: Duration = Duration::from_millis(300);

/// See [`MODEL_QUOTA`]: the same emulator-owned bound, for the notebook-instance store.
pub const NOTEBOOK_INSTANCE_QUOTA: usize = 1_000;

/// The `NotebookInstanceStatus` member a fresh instance carries. The modelled enum also
/// carries the update, delete and maintenance states, which this slice does not
/// reach; `InService` is spelled the same as an endpoint's, so [`STATUS_IN_SERVICE`]
/// stands for both.
pub const NOTEBOOK_INSTANCE_STATUS_PENDING: &str = "Pending";

/// The `NotebookInstanceStatus` an instance whose provisioning could not be completed
/// settles into, from the modelled `NotebookInstanceStatus` enum
/// (`smithy_query shape=NotebookInstanceStatus`).
///
/// A notebook instance is accepted before SageMaker finds out whether it can assume the role
/// the request named, so the answer travels on the INSTANCE — this status and the
/// `FailureReason` naming the role — rather than as a refusal of the create (see
/// [`NotebookInstance::creation_failure_reason`]).
pub const NOTEBOOK_INSTANCE_STATUS_FAILED: &str = "Failed";

/// The `NotebookInstanceStatus` an instance carries while `StopNotebookInstance` is
/// terminating its ML compute instance, from the modelled `NotebookInstanceStatus` enum
/// (`smithy_query shape=NotebookInstanceStatus`).
pub const NOTEBOOK_INSTANCE_STATUS_STOPPING: &str = "Stopping";

/// The `NotebookInstanceStatus` an instance carries once its ML compute instance is
/// terminated, from the modelled `NotebookInstanceStatus` enum.
pub const NOTEBOOK_INSTANCE_STATUS_STOPPED: &str = "Stopped";

/// How long after `StopNotebookInstance` the clock owes the `Stopping` -> `Stopped`
/// transition. Same contract as [`NOTEBOOK_INSTANCE_IN_SERVICE_AFTER`]: a caller that
/// describes the instance straight after stopping it still observes `Stopping`, one that
/// waits (as `aws sagemaker wait notebook-instance-stopped` does) observes `Stopped`.
/// Real SageMaker takes minutes to detach the ML storage volume and terminate the
/// compute instance; the span is this emulator's own compressed choice, never zero, and
/// the due instant is stored rather than slept on.
pub const NOTEBOOK_INSTANCE_STOPPED_AFTER: Duration = Duration::from_millis(300);

/// The `NotebookInstanceStatus` an instance carries while SageMaker is removing it, from
/// the modelled `NotebookInstanceStatus` enum (`smithy_query shape=NotebookInstanceStatus`).
/// It is the status a refused `DeleteNotebookInstance` names as the one it could not move
/// the instance to.
pub const NOTEBOOK_INSTANCE_STATUS_DELETING: &str = "Deleting";

/// The statuses a `DeleteNotebookInstance` takes an instance away FROM: the ones in which
/// no ML compute instance is running for it.
///
/// `DeleteNotebookInstance`'s own doc states the rule — "before you can delete a notebook
/// instance, you must call the StopNotebookInstance API" — so an instance that is serving,
/// coming up or still stopping is refused rather than removed, and one whose provisioning
/// failed has nothing running either and is removed.
pub const NOTEBOOK_INSTANCE_DELETABLE_STATUSES: [&str; 2] = [
    NOTEBOOK_INSTANCE_STATUS_STOPPED,
    NOTEBOOK_INSTANCE_STATUS_FAILED,
];

/// The statuses a `StopNotebookInstance` transitions an instance OUT of: the ones where
/// an ML compute instance exists to terminate, because it is serving (`InService`) or
/// coming up (`Pending`). From every other `NotebookInstanceStatus` there is nothing to
/// stop, so the call is refused rather than moving the status.
pub const NOTEBOOK_INSTANCE_STOPPABLE_STATUSES: [&str; 2] =
    [NOTEBOOK_INSTANCE_STATUS_PENDING, STATUS_IN_SERVICE];

/// The IMDS version a notebook instance reports when the create named none:
/// `MinimumInstanceMetadataServiceVersion`'s doc says that "when passed as part of
/// `CreateNotebookInstance`, if no value is selected, then it defaults to IMDSv1".
pub const DEFAULT_IMDS_VERSION: &str = "1";

/// `DirectInternetAccess`'s default. Its `CreateNotebookInstance` doc points at
/// "Notebook Instances Are Internet-Enabled by Default" and allows `Disabled` only
/// alongside a `SubnetId`.
pub const DEFAULT_DIRECT_INTERNET_ACCESS: &str = "Enabled";

/// `RootAccess`'s default: its `CreateNotebookInstance` doc says "the default value is
/// Enabled".
pub const DEFAULT_ROOT_ACCESS: &str = "Enabled";

/// `IpAddressType`'s default. AWS's `CreateNotebookInstance` API reference documents the
/// member as defaulting to IPv4-only connectivity when the request names no value.
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "ipv4";

/// `PlatformIdentifier`'s default: its `CreateNotebookInstance` doc says "the default
/// value is notebook-al2023-v1".
pub const DEFAULT_PLATFORM_IDENTIFIER: &str = "notebook-al2023-v1";

/// `VolumeSizeInGB`'s default: its `CreateNotebookInstance` doc says "the default value
/// is 5 GB".
pub const DEFAULT_VOLUME_SIZE_IN_GB: i32 = 5;

/// How long the presigned URL `CreatePresignedNotebookInstanceUrl` answers stays valid.
///
/// A DOCUMENTED duration, so it keeps its published value rather than being compressed:
/// the operation's own doc says "the URL that you get from a call to
/// CreatePresignedNotebookInstanceUrl is valid only for 5 minutes".
pub const PRESIGNED_NOTEBOOK_URL_TTL: Duration = Duration::from_secs(5 * 60);

/// The session a presigned notebook-instance URL opens when the request names no
/// `SessionExpirationDurationInSeconds`.
///
/// Also DOCUMENTED, by the member itself: "The duration of the session, in seconds. The
/// default is 12 hours." — which is the top of its modelled `range 1800..43200`.
pub const DEFAULT_NOTEBOOK_SESSION_DURATION_SECONDS: i32 = 12 * 60 * 60;

/// The resource kind a notebook instance's ARN carries, as `NotebookInstanceArn`'s
/// documented `arn:aws:sagemaker:<region>:<account>:notebook-instance/<name>` form
/// spells it. Named once so the create and the reader agree; both pass it to
/// [`resource_arn`].
pub const NOTEBOOK_INSTANCE_ARN_KIND: &str = "notebook-instance";

/// The Jupyter host a notebook instance serves on:
/// `<name>.notebook.<region>.sagemaker.aws`, the host `DescribeNotebookInstance`'s
/// `Url` carries and `CreatePresignedNotebookInstanceUrl` signs against.
///
/// Derived from the instance name and the region the caller signed for, so it is stable
/// across reads and distinct per instance.
pub fn notebook_instance_url(region: &str, name: &str) -> String {
    format!("{name}.notebook.{region}.sagemaker.aws")
}

/// The presigned URL `CreatePresignedNotebookInstanceUrl` answers: the instance's own
/// Jupyter host (the `Url` [`notebook_instance_url`] minted and
/// `DescribeNotebookInstance` reports) with the grant this call recorded on the instance
/// carried as the `authToken` query parameter — the same shape the presigned partner-app
/// URL takes, so one scheme covers both.
pub fn presigned_notebook_instance_url(host: &str, token: &str) -> String {
    format!("https://{}/auth?authToken={token}", host.trim_end_matches('/'))
}

/// The id of the network interface `CreateNotebookInstance` reports it created:
/// `eni-` plus 17 lowercase hex digits, the form EC2 network-interface ids take.
///
/// The digits come from this module's one id mint, [`mint_opaque_resource_id`], so an ENI
/// id is server-generated and derived from nothing in the request, with the same
/// emulator-grade, non-cryptographic caveats that mint documents: it identifies a record
/// in this store and is never a secret. `CreateNotebookInstance` mints it ONCE and stores
/// it, so every later read of the same instance reports the same id.
pub fn mint_network_interface_id() -> String {
    let digits = mint_opaque_resource_id();
    // The mint renders a 32-digit hex id, but the slice is taken defensively so this
    // helper does not depend on that width: a shorter id is used whole.
    format!("eni-{}", digits.get(..17).unwrap_or(&digits))
}

/// A notebook instance created by `CreateNotebookInstance`, as
/// `DescribeNotebookInstance` replays it.
///
/// Every member the create carried is kept as given, so the reader replays the request
/// rather than re-deriving it; the members the create left out are kept at the default
/// the model documents for them (the `DEFAULT_*` constants above). `in_service_at` is the
/// instant the clock owes the `Pending` -> `InService` transition; it is stored, never
/// slept on, and materialised by whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct NotebookInstance {
    pub name: String,
    pub arn: String,
    pub status: String,
    /// Why the instance failed, for the `Failed` status: `None` until a transition the
    /// clock owes puts the instance there ([`NotebookInstance::settle_status`]).
    pub failure_reason: Option<String>,
    /// Why this instance's provisioning cannot complete, resolved at create time and
    /// revealed by the clock: the instance settles into
    /// [`NOTEBOOK_INSTANCE_STATUS_FAILED`] carrying this as its `FailureReason` instead of
    /// reaching `InService`. `None` for a create nothing was answered against, which is
    /// the one that comes up.
    pub creation_failure_reason: Option<String>,
    /// The Jupyter host the instance serves on, minted at create from the name and the
    /// signed region (see [`notebook_instance_url`]).
    pub url: String,
    pub instance_type: String,
    pub ip_address_type: String,
    pub subnet_id: Option<String>,
    pub security_groups: Vec<String>,
    pub role_arn: String,
    pub kms_key_id: Option<String>,
    /// The id of the network interface the create minted for the instance.
    pub network_interface_id: String,
    pub lifecycle_config_name: Option<String>,
    pub direct_internet_access: String,
    pub volume_size_in_gb: i32,
    pub accelerator_types: Vec<String>,
    pub default_code_repository: Option<String>,
    pub additional_code_repositories: Vec<String>,
    pub root_access: String,
    pub platform_identifier: String,
    pub instance_metadata_service_configuration:
        crate::ops::types::InstanceMetadataServiceConfiguration,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_at: SystemTime,
    /// The instant the clock owes the `Stopping` -> `Stopped` transition, set by
    /// `StopNotebookInstance` and cleared once the transition is materialised. `None`
    /// while no stop is in flight.
    pub stopped_at: Option<SystemTime>,
    /// Whether the instance ever finished provisioning, i.e. the `Pending` ->
    /// `InService` transition was materialised. Kept beside the status rather than
    /// derived from it, because a stop that lands while the instance is still `Pending`
    /// leaves it `Stopping` and then `Stopped` without it ever having come up — and an
    /// instance that never came up has no Jupyter endpoint and no network interface to
    /// report (see [`NotebookInstance::is_provisioned`]).
    pub provisioned: bool,
    /// Every presigned-URL grant `CreatePresignedNotebookInstanceUrl` minted for this
    /// instance whose session could still be in use, oldest first. Same record as a
    /// partner app's, [`PresignedUrlGrant`], because the two grants carry the same thing.
    pub url_grants: Vec<PresignedUrlGrant>,
}

impl NotebookInstance {
    /// Materialise whichever status transition the clock already owes, the way
    /// [`Endpoint::settle_status`] does for an endpoint: `Pending` -> `InService` after a
    /// create, `Stopping` -> `Stopped` after a stop. Each was due at its stored instant
    /// whether or not anybody called, and `LastModifiedTime` moves to the instant it was
    /// due rather than to "now", so the value is stable across later reads.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == NOTEBOOK_INSTANCE_STATUS_PENDING && now >= self.in_service_at {
            self.last_modified_time = self.in_service_at;
            // What the create resolved against the instance decides WHICH status the clock
            // owed: an instance whose provisioning could not complete fails with the reason
            // that create recorded, and never comes up.
            if let Some(reason) = self.creation_failure_reason.clone() {
                self.status = NOTEBOOK_INSTANCE_STATUS_FAILED.to_string();
                self.failure_reason = Some(reason);
                return true;
            }
            self.status = STATUS_IN_SERVICE.to_string();
            self.provisioned = true;
            return true;
        }
        // The stop's own due instant, kept beside the create's so a stop that arrives
        // while the instance is still `Pending` does not later snap back to `InService`.
        let stop_due = match self.stopped_at {
            Some(due) if self.status == NOTEBOOK_INSTANCE_STATUS_STOPPING && now >= due => {
                Some(due)
            }
            _ => None,
        };
        if let Some(due) = stop_due {
            self.status = NOTEBOOK_INSTANCE_STATUS_STOPPED.to_string();
            self.last_modified_time = due;
            self.stopped_at = None;
            return true;
        }
        false
    }

    /// Whether `StopNotebookInstance` has an ML compute instance to terminate, i.e. the
    /// status is one of [`NOTEBOOK_INSTANCE_STOPPABLE_STATUSES`].
    pub fn is_stoppable(&self) -> bool {
        NOTEBOOK_INSTANCE_STOPPABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Whether `DeleteNotebookInstance` can take the instance away, i.e. the status is one
    /// of [`NOTEBOOK_INSTANCE_DELETABLE_STATUSES`].
    pub fn is_deletable(&self) -> bool {
        NOTEBOOK_INSTANCE_DELETABLE_STATUSES.contains(&self.status.as_str())
    }

    /// Start terminating the ML compute instance: the status moves to `Stopping` now and
    /// the clock owes `Stopped` at [`NOTEBOOK_INSTANCE_STOPPED_AFTER`] from `now`, which
    /// [`NotebookInstance::settle_status`] materialises once it comes due.
    ///
    /// Every stored member is left as it stands — SageMaker preserves the ML storage
    /// volume across a stop — so a later start replays the same configuration.
    pub fn begin_stop(&mut self, now: SystemTime) {
        self.status = NOTEBOOK_INSTANCE_STATUS_STOPPING.to_string();
        self.last_modified_time = now;
        self.stopped_at = Some(now + NOTEBOOK_INSTANCE_STOPPED_AFTER);
    }

    /// Record a presigned-URL grant against this instance and answer it: the opaque token
    /// the URL carries, the [`PRESIGNED_NOTEBOOK_URL_TTL`] the URL itself is good for, and
    /// the session `session_duration_seconds` opens once the URL is followed.
    ///
    /// A grant whose session the clock has already ended is released first — it was over
    /// at its stored instant whether or not anybody called — and the history is bounded by
    /// [`GRANT_HISTORY_CAP`], exactly as a partner app's grants are, so a caller asking
    /// for URL after URL cannot grow the store without end.
    pub fn grant_presigned_url(
        &mut self,
        now: SystemTime,
        session_duration_seconds: i32,
    ) -> PresignedUrlGrant {
        let grant = PresignedUrlGrant {
            token: mint_grant_token(),
            issued_at: now,
            expires_at: now + PRESIGNED_NOTEBOOK_URL_TTL,
            session_expires_at: now
                + Duration::from_secs(u64::try_from(session_duration_seconds).unwrap_or_default()),
        };
        self.url_grants.retain(|held| held.session_expires_at > now);
        self.url_grants.push(grant.clone());
        if self.url_grants.len() > GRANT_HISTORY_CAP {
            let overflow = self.url_grants.len() - GRANT_HISTORY_CAP;
            self.url_grants.drain(..overflow);
        }
        grant
    }

    /// Whether the instance has finished provisioning, i.e. the `Pending` ->
    /// `InService` transition the create owed has been materialised.
    ///
    /// An instance that has not come up has no Jupyter endpoint and no network interface
    /// to report, so `DescribeNotebookInstance` omits `Url` and `NetworkInterfaceId`
    /// until it is provisioned — including for one stopped before it ever came up.
    pub fn is_provisioned(&self) -> bool {
        self.provisioned
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another notebook instance fits, i.e. the store is below
    /// [`NOTEBOOK_INSTANCE_QUOTA`].
    pub fn notebook_instances_below_quota(&self) -> bool {
        self.notebook_instances.len() < NOTEBOOK_INSTANCE_QUOTA
    }
}
