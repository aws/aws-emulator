// SPDX-License-Identifier: CC0-1.0

//! The FEATURE STORE control-plane slice of this service's state: the feature group
//! `CreateFeatureGroup` writes, `DescribeFeatureGroup` replays and
//! `DeleteFeatureGroup` releases, with its quota, its statuses, the clock spans it
//! moves on and the sentences its refusals are worded with.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit. Every name is re-exported from [`super`], so a handler keeps
//! naming `crate::state::…`.

use super::*;

/// How long after `CreateFeatureGroup` the clock owes the `Creating` -> `Created`
/// transition.
///
/// Same contract as [`ENDPOINT_IN_SERVICE_AFTER`]: a caller that describes the feature
/// group straight after creating it still observes `Creating`, and one that polls (as
/// `aws sagemaker wait` and the Feature Store SDK's own wait loop do) observes
/// `Created`. The model's doc puts the real service at "approximately 10-15 minutes to
/// provision an OnlineStore FeatureGroup with the InMemory StorageType", so the span is
/// compressed, never zero.
pub const FEATURE_GROUP_CREATED_AFTER: Duration = Duration::from_millis(300);

/// How long after `DeleteFeatureGroup` the clock owes the `Deleting` -> gone transition
/// of a feature group.
///
/// The same compressed, never-zero span as [`FEATURE_GROUP_CREATED_AFTER`]: a caller
/// that describes the group straight after asking for its deletion still observes
/// `Deleting`, and one that polls observes the group gone. The model's doc puts the real
/// service at "approximately 10-15 minutes to delete an OnlineStore FeatureGroup with the
/// InMemory StorageType", so the span is compressed, never zero.
pub const FEATURE_GROUP_RELEASED_AFTER: Duration = FEATURE_GROUP_CREATED_AFTER;

/// See [`MODEL_QUOTA`]. `CreateFeatureGroup`'s doc points at the published quota
/// ("Check Amazon Web Services service quotas to see the FeatureGroups quota for your
/// Amazon Web Services account"), which is per-account and adjustable, so this is the
/// emulator's own bound rather than a claim about any account's limit.
pub const FEATURE_GROUP_QUOTA: usize = 1_000;

/// `FeatureGroupStatus`'s members this emulator moves a feature group through (the
/// modelled enum also carries `DeleteFailed`, which this slice does not reach: nothing here
/// fails a group's DELETION). Named once here so no handler compares
/// a bare string literal. Deliberately separate from [`STATUS_CREATING`] even though the
/// two spell the same value today: they are members of two different modelled enums, and
/// either may be re-spelled without the other.
pub const FEATURE_GROUP_STATUS_CREATING: &str = "Creating";
/// See [`FEATURE_GROUP_STATUS_CREATING`].
pub const FEATURE_GROUP_STATUS_CREATED: &str = "Created";
/// See [`FEATURE_GROUP_STATUS_CREATING`]. What a group whose offline store could not be set
/// up settles in: "The `FeatureGroup` could not be created in the `OfflineStore`"
/// ([`FeatureGroup::failure_reason`]).
pub const FEATURE_GROUP_STATUS_CREATE_FAILED: &str = "CreateFailed";
/// See [`FEATURE_GROUP_STATUS_CREATING`]. What a group reports from the
/// `DeleteFeatureGroup` that asked for its deletion until the clock completes it.
pub const FEATURE_GROUP_STATUS_DELETING: &str = "Deleting";

/// The `OfflineStoreStatusValue` an offline store reports while the replication it
/// describes is being made: "An `OfflineStore` status."
pub const OFFLINE_STORE_STATUS_ACTIVE: &str = "Active";

/// The `OfflineStoreStatusValue` an offline store reports while the replication CANNOT be
/// made: the role it runs under may not write the objects at the group's offline location,
/// so nothing a client ingests is retained there ([`offline_store_status`]).
pub const OFFLINE_STORE_STATUS_BLOCKED: &str = "Blocked";

/// The status of a feature group's offline store, as every report of that group carries it:
/// `None` for a group created with no `OfflineStoreConfig` (there is no replication to
/// report on), `Blocked` with a `BlockedReason` while the role the replication runs under
/// may not write the objects at the group's resolved output path, and `Active` otherwise.
///
/// "The status of the OfflineStore. Notifies you if replicating data into the OfflineStore
/// has failed" (`DescribeFeatureGroup$OfflineStoreStatus`): what can fail is the WRITE, made
/// under the group's own `RoleArn` — "the IAM execution role used to persist data into the
/// OfflineStore" — so the status is a function of the permission that role holds at the
/// moment the group is reported, which is why a client that repairs the role's policy sees
/// the status return to `Active` and one that revokes it sees `Blocked`. The permission
/// question is the shared one the data plane authorizes each of its own writes with
/// ([`::service_common::feature_store_offline_store::write_is_withheld`]), so a status this
/// control plane reports and a record that plane retains cannot disagree.
///
/// Reporting only: nothing is written here, and a role this emulator's IAM holds nothing
/// under withholds nothing (see the shared question's own docs).
pub async fn offline_store_status(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    offline_store_config: Option<&crate::ops::types::OfflineStoreConfig>,
    role_arn: Option<&str>,
) -> Option<crate::ops::types::OfflineStoreStatus> {
    let config = offline_store_config?;
    let active = crate::ops::types::OfflineStoreStatus {
        status: OFFLINE_STORE_STATUS_ACTIVE.to_string(),
        blocked_reason: None,
    };
    let storage = &config.s3_storage_config;
    // The resolved output path is what the records are written under, so it is the location
    // the permission is asked about; a group whose create could not resolve one falls back
    // to the `S3Uri` the create named, which is the only location it has.
    let resolved = storage
        .resolved_output_s3_uri
        .as_deref()
        .unwrap_or(storage.s3_uri.as_str());
    let (Some(location), Some(role_arn)) = (
        ::service_common::feature_store_offline_store::OfflineStoreLocation::parse(resolved),
        role_arn,
    ) else {
        return Some(active);
    };
    let key = ::service_common::feature_store_offline_store::status_key(&location);
    if ::service_common::feature_store_offline_store::write_is_withheld(
        callback,
        region,
        role_arn,
        &location,
        &key,
    )
    .await
    {
        return Some(crate::ops::types::OfflineStoreStatus {
            status: OFFLINE_STORE_STATUS_BLOCKED.to_string(),
            blocked_reason: Some(
                ::service_common::feature_store_offline_store::blocked_reason(
                    role_arn,
                    &location.uri(),
                ),
            ),
        });
    }
    Some(active)
}

/// How SageMaker words a `CreateFeatureGroup` it REFUSES because the role it was handed may
/// not write the offline store the request asks for: the Amazon S3 access-denied cause,
/// naming the role the store would be written as, the permission it is missing and the
/// location it is missing it on.
///
/// SageMaker probes the offline-store location WITH that role before it admits the group, so
/// this travels as the refusal of the call rather than on a resource: the caller is told at
/// the call site, which is where an application reports it and fails fast.
///
/// Every variable part is taken from the REQUEST: the `RoleArn` it supplied and the `S3Uri`
/// it named, as the caller spelled them, which is how every other report of an Amazon S3
/// location this service could not reach words it ([`could_not_access_model_data`]). The
/// action is the one an object write is authorized under
/// (`::service_common::feature_store_offline_store::WRITE_ACTION`), so the sentence names the
/// permission a client actually grants. The per-call assumed-role SESSION name and the Amazon
/// S3 request ids the real service's text also carries are values of one run, so the sentence
/// states the cause and omits them rather than inventing any.
#[must_use]
pub fn offline_store_access_denied(role_arn: &str, s3_uri: &str) -> String {
    format!(
        "The FeatureGroup could not be created in the OfflineStore: AccessDenied — the IAM \
         role {role_arn} is not authorized to perform: {} on the OfflineStore location \
         {s3_uri}. Grant the role that permission and create the FeatureGroup again.",
        ::service_common::feature_store_offline_store::WRITE_ACTION
    )
}

/// Why the offline store an `OfflineStoreConfig` asks for could NOT be created — the sentence
/// `CreateFeatureGroup` is REFUSED with ([`offline_store_access_denied`]) — or `None` when
/// nothing establishes that.
///
/// "The Amazon Resource Name (ARN) of the IAM execution role used to persist data into the
/// OfflineStore if an OfflineStoreConfig is provided" (`RoleArn`): the store is SET UP as that
/// role before the group is admitted, so a location that role may not write backs no offline
/// store and there is nothing to create — the create is refused SYNCHRONOUSLY, with the
/// access-denied cause, and nothing is stored. A create that names no offline store asks for no
/// replication and is refused for nothing here.
///
/// The question is the shared one for this store
/// ([`::service_common::feature_store_offline_store::write_is_withheld`]) — the one the group's
/// `OfflineStoreStatus` is reported from ([`offline_store_status`]) and the one the data plane
/// makes each record write under — so the permission that refuses a create, the status a group
/// reports and the objects its store holds cannot disagree.
///
/// Only what IAM and Amazon S3 ESTABLISHED refuses a create: a role this emulator's IAM holds
/// nothing under, and a bucket carrying no policy, establish nothing, so such a group is
/// admitted.
///
/// Nothing is written and no store guard is held: the questions are awaited.
pub async fn offline_store_admission_refusal(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    offline_store_config: Option<&crate::ops::types::OfflineStoreConfig>,
    role_arn: Option<&str>,
) -> Option<String> {
    let config = offline_store_config?;
    let role_arn = role_arn?;
    let storage = &config.s3_storage_config;
    // The resolved output path is what the records are written under, so it is the location
    // the permission is asked about; a create whose `S3Uri` resolved none falls back to that
    // `S3Uri`, which is then the only location the group has.
    let resolved = storage
        .resolved_output_s3_uri
        .as_deref()
        .unwrap_or(storage.s3_uri.as_str());
    let location =
        ::service_common::feature_store_offline_store::OfflineStoreLocation::parse(resolved)?;
    // The key every record object of this group is written under, which is the key the group's
    // own status is decided for as well.
    let key = ::service_common::feature_store_offline_store::status_key(&location);
    ::service_common::feature_store_offline_store::write_is_withheld(
        ctx.callback(),
        region,
        role_arn,
        &location,
        &key,
    )
    .await
    .then(|| offline_store_access_denied(role_arn, &storage.s3_uri))
}

/// The `ThroughputMode` a feature group runs under when its create stated no
/// `ThroughputConfig`: the mode that bills per read and write and needs no stated
/// capacity ("With on-demand mode, you are charged for data reads and writes that your
/// application performs on your feature group. You do not need to specify read and write
/// throughput" — `ThroughputConfig`), spelled as the model's `ThroughputMode` enum spells
/// it.
pub const THROUGHPUT_MODE_ON_DEMAND: &str = "OnDemand";

/// The throughput configuration a feature group whose create stated none runs under.
pub fn default_throughput_config() -> crate::ops::types::ThroughputConfig {
    crate::ops::types::ThroughputConfig {
        throughput_mode: THROUGHPUT_MODE_ON_DEMAND.to_string(),
        // "This field is not applicable for on-demand feature groups."
        provisioned_read_capacity_units: None,
        provisioned_write_capacity_units: None,
    }
}

/// The `<kind>` component of a feature group's ARN, i.e. the `<kind>` in
/// [`resource_arn`], from `FeatureGroupArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:feature-group/`). Named once here
/// so the create mints, the readers resolve and the refusals match the one kind.
pub const FEATURE_GROUP_ARN_KIND: &str = "feature-group";

/// How SageMaker words a feature group this account does not hold: the `Resource Not
/// Found:` prefix, then the resource type and the name the request resolved to — a
/// sentence that ends with NO period.
///
/// Stated once here so the feature-group readers and the delete cannot word the same fact
/// differently. The name is the one the request addressed the group by, reduced from the
/// ARN form where the member accepts one ([`name_from_arn_or_name`]).
pub fn feature_group_not_found(name: &str) -> String {
    format!("Resource Not Found: Amazon SageMaker can't find a FeatureGroup with name {name}")
}

/// How SageMaker words a refusal to delete a feature group that is not provisioned yet:
/// the `Validation Error:` prefix, the group's name, and the status that stands in the
/// way — reported in the upper case this message spells a `FeatureGroupStatus` in.
pub fn feature_group_not_deletable(name: &str, status: &str) -> String {
    format!(
        "Validation Error: FeatureGroup with name {name} cannot be deleted, because \
         FeatureGroupStatus is '{}'.",
        status.to_uppercase()
    )
}

/// How SageMaker words a refusal to update a feature group that is not provisioned: the
/// `Validation Error:` prefix, the group's name, and the status that stands in the way —
/// reported in the upper case this message spells a `FeatureGroupStatus` in, exactly as
/// [`feature_group_not_deletable`] words the same fact for a delete.
///
/// A group whose creation FAILED carries no provisioned schema or store for an update to
/// change, so there is nothing for this call to apply to; the remedy is to delete it and
/// create it again.
pub fn feature_group_not_updatable(name: &str, status: &str) -> String {
    format!(
        "Validation Error: FeatureGroup with name {name} cannot be updated, because \
         FeatureGroupStatus is '{}'.",
        status.to_uppercase()
    )
}

/// The feature names a `FeatureGroup` may not declare, from `FeatureDefinitions`' own
/// doc: "FeatureNames cannot be any of the following: is_deleted, write_time,
/// api_invocation_time" — Feature Store writes those three columns itself.
///
/// The SHARED list ([`::service_common::feature_store_groups::RESERVED_FEATURE_NAMES`]):
/// the FeatureStore Runtime data plane refuses a record that posts a feature value under
/// one of these names for the same reason this plane refuses a schema that declares one, so
/// the two planes cannot disagree about which names are reserved.
pub const RESERVED_FEATURE_NAMES: &[&str] =
    ::service_common::feature_store_groups::RESERVED_FEATURE_NAMES;

/// A feature group created by `CreateFeatureGroup`, as `DescribeFeatureGroup` replays
/// it.
///
/// `created_due_at` is the instant the clock owes the `Creating` -> `Created` transition;
/// like [`Endpoint::in_service_at`] it is stored, never slept on, and materialised by
/// whichever reporting call first runs at or after it.
#[derive(Debug, Clone)]
pub struct FeatureGroup {
    pub name: String,
    pub arn: String,
    pub record_identifier_feature_name: String,
    pub event_time_feature_name: String,
    pub feature_definitions: Vec<crate::ops::types::FeatureDefinition>,
    pub online_store_config: Option<crate::ops::types::OnlineStoreConfig>,
    pub offline_store_config: Option<crate::ops::types::OfflineStoreConfig>,
    pub throughput_config: Option<crate::ops::types::ThroughputConfig>,
    pub role_arn: Option<String>,
    pub description: Option<String>,
    pub tags: Vec<Tag>,
    pub status: String,
    /// Why a group's provisioning failed, i.e. the `FailureReason` a reader reports beside
    /// the status. `None` for every group this emulator admits: the one failure it
    /// establishes — an offline store the group's role may not write — is settled BEFORE the
    /// group is stored, and refuses the create
    /// ([`offline_store_admission_refusal`]), so no healthy-looking group is left behind for
    /// it. The member is kept because `FeatureGroupStatus` carries `CreateFailed` and
    /// `DescribeFeatureGroup` reports the reason beside it, which is what
    /// [`FeatureGroup::settle_status`] materialises for a group that does carry one.
    pub failure_reason: Option<String>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    /// WHEN the `Created` status falls due, not when the group was created — that is
    /// `creation_time`. Named the way [`Endpoint::in_service_at`] names the same thing.
    pub created_due_at: SystemTime,
    /// The instant the clock owes this group's removal, set by
    /// [`FeatureGroup::begin_deletion`]; `None` while no `DeleteFeatureGroup` has asked
    /// for one. Past it the group is gone for every caller
    /// ([`FeatureGroup::is_released`]), whether or not a sweep has dropped the record
    /// yet — the way [`Cluster::removal_due_at`] carries the same fact.
    pub removal_due_at: Option<SystemTime>,
}

impl FeatureGroup {
    /// Materialise the `Creating` -> `Created` transition the clock already owes — or the
    /// `Creating` -> `CreateFailed` one, for a group whose offline store could not be
    /// created ([`Self::failure_reason`]).
    ///
    /// The one write a reporting operation may make, for the reason spelled out on
    /// [`Endpoint::settle_status`]: the transition was due at `created_due_at` whether or
    /// not anybody described the group.
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        if self.status == FEATURE_GROUP_STATUS_CREATING && now >= self.created_due_at {
            self.status = if self.failure_reason.is_some() {
                FEATURE_GROUP_STATUS_CREATE_FAILED.to_string()
            } else {
                FEATURE_GROUP_STATUS_CREATED.to_string()
            };
            self.last_modified_time = self.created_due_at;
            return true;
        }
        false
    }

    /// This group's `FailureReason` as a reader reports it: the reason its offline store
    /// could not be created, once the clock has SETTLED the group into
    /// [`FEATURE_GROUP_STATUS_CREATE_FAILED`].
    ///
    /// "The reason that the `FeatureGroup` failed to be replicated in the `OfflineStore`"
    /// (`DescribeFeatureGroup$FailureReason`) is a fact about a failure that has HAPPENED, so
    /// a group still being created reports none even where the create already resolved what
    /// will fail it.
    pub fn reported_failure_reason(&self) -> Option<String> {
        (self.status == FEATURE_GROUP_STATUS_CREATE_FAILED)
            .then(|| self.failure_reason.clone())
            .flatten()
    }

    /// Whether this group's creation FAILED, so it holds no provisioned store or schema: the
    /// state every mutation of a group refuses ([`feature_group_not_updatable`]) and the one
    /// a client resolves by deleting the group.
    pub fn creation_failed(&self) -> bool {
        self.status == FEATURE_GROUP_STATUS_CREATE_FAILED
    }

    /// Start the deletion `DeleteFeatureGroup` asked for: the group reports `Deleting`,
    /// and the clock owes its removal [`FEATURE_GROUP_RELEASED_AFTER`] later.
    ///
    /// The record is NOT dropped here: `FeatureGroupStatus` carries `Deleting` and
    /// SageMaker answers it until the deletion finishes, so the transient status is
    /// observable and the removal is the clock's — stored as an instant and never slept
    /// on. The status change is an update to the group, so it moves
    /// `last_modified_time` too.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = FEATURE_GROUP_STATUS_DELETING.to_string();
        self.last_modified_time = now;
        self.removal_due_at = Some(now + FEATURE_GROUP_RELEASED_AFTER);
    }

    /// Whether a `DeleteFeatureGroup` has asked for this group's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the deletion the clock owed has completed, i.e. the group is gone.
    ///
    /// A lookup answers `None` for such a group and a listing leaves it out, so no caller
    /// sees a group whose removal was already due — the record itself is dropped by
    /// [`ComAmazonawsSagemakerSagemakerState::release_deleted_feature_groups`], which
    /// frees the name and the quota slot it held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }

    /// This group's throughput configuration as `DescribeFeatureGroup` reports it.
    ///
    /// `ThroughputConfig` (what a create states) and `ThroughputConfigDescription`
    /// (what a describe reports) are two shapes with the same three members, so the
    /// active configuration is the one stored for the group, re-shaped. Every group has
    /// one: a create that stated none is stored with
    /// [`default_throughput_config`], the mode the group then runs under.
    pub fn throughput_config_description(
        &self,
    ) -> Option<crate::ops::types::ThroughputConfigDescription> {
        self.throughput_config.as_ref().map(|config| {
            crate::ops::types::ThroughputConfigDescription {
                throughput_mode: config.throughput_mode.clone(),
                provisioned_read_capacity_units: config.provisioned_read_capacity_units,
                provisioned_write_capacity_units: config.provisioned_write_capacity_units,
            }
        })
    }

    /// Whether this group keeps an `OnlineStore`: "Turn OnlineStore off by specifying
    /// False for the EnableOnlineStore flag. … The default value is False."
    /// (`OnlineStoreConfig$EnableOnlineStore`.)
    pub fn online_store_enabled(&self) -> bool {
        self.online_store_config
            .as_ref()
            .and_then(|config| config.enable_online_store)
            .unwrap_or(false)
    }
}

/// The feature-group half of the store's API, in its own `impl` block for the same
/// reason as the section: a resource's state reads as one block.
impl ComAmazonawsSagemakerSagemakerState {
    /// Whether another feature group fits, i.e. the store is below
    /// [`FEATURE_GROUP_QUOTA`].
    pub fn feature_groups_below_quota(&self) -> bool {
        self.feature_groups.len() < FEATURE_GROUP_QUOTA
    }

    /// The feature group a `FeatureGroupName` or `FeatureGroupNameOrArn` names, or `None`
    /// when this account holds none under it.
    ///
    /// The one lookup every feature-group operation resolves that member with, so no
    /// handler re-implements it: the ARN form is reduced to the name the store is keyed by
    /// ([`name_from_arn_or_name`]), and a group whose removal the clock already completed
    /// resolves to nothing — past that instant the group is gone whether or not the record
    /// has been dropped yet ([`FeatureGroup::is_released`]).
    pub fn feature_group_mut(&mut self, name_or_arn: &str) -> Option<&mut FeatureGroup> {
        let now = SystemTime::now();
        self.feature_groups
            .get_mut(name_from_arn_or_name(name_or_arn))
            .filter(|group| !group.is_released(now))
    }

    /// Drop every feature group whose removal the clock already completed, answering
    /// their ARNs in store order.
    ///
    /// Materialises only what was already due: the removal was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a group
    /// ([`FeatureGroup::is_released`]), so this changes nothing a later caller sees — it
    /// frees the name and the quota slot the gone group still held.
    pub fn release_deleted_feature_groups(&mut self, now: SystemTime) -> Vec<String> {
        let released: Vec<(String, String)> = self
            .feature_groups
            .values()
            .filter(|group| group.is_released(now))
            .map(|group| (group.name.clone(), group.arn.clone()))
            .collect();
        for (name, _) in &released {
            self.feature_groups.remove(name);
        }
        released.into_iter().map(|(_, arn)| arn).collect()
    }
}
