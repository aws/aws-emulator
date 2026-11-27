// SPDX-License-Identifier: CC0-1.0

//! The ONTAP policy slice of this service's state: the SnapMirror policies
//! `CreateSnapmirrorPolicy` writes and the snapshot policies `CreateSnapshotPolicy` writes,
//! what `DescribeSnapmirrorPolicies` and `DescribeSnapshotPolicies` read, and what their
//! update and delete operations change and remove.
//!
//! Both families are modelled the same way and follow the same rules, so they share this
//! module: each policy is created on EITHER a file system or one of its SVMs — "You must
//! specify either a `FileSystemId` or a `StorageVirtualMachineId`, but not both" — which is
//! what its `Scope` then reports, each is a resource of its own with a system-generated id
//! and a UUID, and each belongs to the file system it was created on: the records go when
//! the file system does.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::ops::types::{
    SnapmirrorPolicy, SnapmirrorPolicyRetentionRule, SnapshotPolicy, SnapshotPolicySchedule,
};

use super::{
    fsx_arn, tag_list, within_error_message_length,
    ComAmazonawsFsxAwssimbaapiserviceV20180301State, DescribedRecord, PendingTransition,
    TransitionTarget, ONTAP_FILTER_FILE_SYSTEM_ID, ONTAP_FILTER_STORAGE_VIRTUAL_MACHINE_ID,
    ONTAP_LIFECYCLE_CREATED, ONTAP_LIFECYCLE_DELETING,
};

/// The resource type a SnapMirror policy's `ResourceARN` names, as the `<type>/<id>` half of
/// an FSx ARN (one of [`super::FSX_RESOURCE_TYPES`]): the resource name of the modelled
/// shape, hyphenated, as every other FSx resource type is in an ARN of this service.
pub const SNAPMIRROR_POLICY_RESOURCE_TYPE: &str = "snapmirror-policy";
/// See [`SNAPMIRROR_POLICY_RESOURCE_TYPE`].
pub const SNAPSHOT_POLICY_RESOURCE_TYPE: &str = "snapshot-policy";

/// The `Scope` of a policy created on a FILE SYSTEM, from the modelled enums
/// (`smithy_query(query="shape", shape="SnapmirrorPolicyScope")` and `SnapshotPolicyScope`:
/// FILESYSTEM | SVM): "`FILESYSTEM` – The SnapMirror policy was created on the file
/// system."
pub const POLICY_SCOPE_FILE_SYSTEM: &str = "FILESYSTEM";
/// See [`POLICY_SCOPE_FILE_SYSTEM`]. "`SVM` – The SnapMirror policy was created on an SVM."
pub const POLICY_SCOPE_STORAGE_VIRTUAL_MACHINE: &str = "SVM";

/// Whether SnapMirror compresses data over the network when a create states nothing, from
/// `CreateSnapmirrorPolicy$NetworkCompressionEnabled` ("This value defaults to false").
pub const DEFAULT_NETWORK_COMPRESSION_ENABLED: bool = false;

/// Whether a snapshot policy is enabled when a create states nothing, from
/// `CreateSnapshotPolicy$Enabled` ("This value defaults to true").
pub const DEFAULT_SNAPSHOT_POLICY_ENABLED: bool = true;

/// The modelled `SnapmirrorPolicySnapshotCopyMode` members whose documentation constrains
/// `RetentionRules` (`smithy_query(query="shape",
/// shape="SnapmirrorPolicySnapshotCopyMode")`: COPY_ALL_SOURCE_SNAPSHOTS |
/// COPY_LATEST_SOURCE_SNAPSHOT | NO_CREATE_SNAPSHOT_ON_SOURCE). See
/// [`retention_rules_refusal`].
pub const SNAPSHOT_COPY_MODE_COPY_ALL_SOURCE_SNAPSHOTS: &str = "COPY_ALL_SOURCE_SNAPSHOTS";
/// See [`SNAPSHOT_COPY_MODE_COPY_ALL_SOURCE_SNAPSHOTS`].
pub const SNAPSHOT_COPY_MODE_COPY_LATEST_SOURCE_SNAPSHOT: &str = "COPY_LATEST_SOURCE_SNAPSHOT";
/// See [`SNAPSHOT_COPY_MODE_COPY_ALL_SOURCE_SNAPSHOTS`].
pub const SNAPSHOT_COPY_MODE_NO_CREATE_SNAPSHOT_ON_SOURCE: &str = "NO_CREATE_SNAPSHOT_ON_SOURCE";

/// The modelled `SnapmirrorPolicyFilterName`s `DescribeSnapmirrorPolicies` selects policies
/// by (`smithy_query(query="shape", shape="SnapmirrorPolicyFilterName")`: file-system-id |
/// storage-virtual-machine-id). `DescribeSnapshotPolicies` selects by the same two names
/// (`SnapshotPolicyFilterName`), so both listings share this slice.
pub const POLICY_FILTER_NAMES: &[&str] = &[
    ONTAP_FILTER_FILE_SYSTEM_ID,
    ONTAP_FILTER_STORAGE_VIRTUAL_MACHINE_ID,
];

/// The most policies of ONE family this store holds for ONE file system.
///
/// CHOSEN, not published by AWS: the model states no quota for either policy family, so each
/// store — which a caller grows one entry per create — is bounded at the most ONE page of
/// the matching describe operation can report (the modelled `SnapmirrorPolicies` and
/// `SnapshotPolicies` lists' own `length max 50`, which is also the most ids one request can
/// name).
///
/// A create past it is refused with `BadRequest` ("a generic error indicating a failure with
/// a client request") rather than the modelled `ServiceLimitExceeded`: that error's `Limit`
/// member is REQUIRED and the modelled `ServiceLimit` enum has no value naming either policy
/// family, so there is no limit value to report.
pub const MAX_POLICIES_PER_FILE_SYSTEM: usize = 50;

/// The largest page either describe operation returns, from the modelled `SnapmirrorPolicies`
/// and `SnapshotPolicies` lists' own `length max 50`.
pub const MAX_POLICIES_PER_PAGE: usize = 50;

/// What a create states the policy is created ON: a file system, or one of its SVMs.
///
/// "You must specify either a `FileSystemId` or a `StorageVirtualMachineId`, but not both"
/// (`CreateSnapmirrorPolicy$FileSystemId`), which both families document identically — so
/// the reading of those two members is written once, here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestedPolicyScope {
    /// Created on the file system this names: `Scope` is [`POLICY_SCOPE_FILE_SYSTEM`].
    FileSystem(String),
    /// Created on the SVM this names: `Scope` is
    /// [`POLICY_SCOPE_STORAGE_VIRTUAL_MACHINE`], and the policy's `FileSystemId` is the
    /// SVM's own file system.
    StorageVirtualMachine(String),
}

/// What a create states the policy is created on, or `None` when it states neither or both.
///
/// The caller answers `None` with `BadRequest` carrying [`POLICY_SCOPE_MESSAGE`], the
/// requirement AWS words on the members themselves.
pub fn requested_policy_scope(
    file_system_id: Option<&str>,
    storage_virtual_machine_id: Option<&str>,
) -> Option<RequestedPolicyScope> {
    match (file_system_id, storage_virtual_machine_id) {
        (Some(file_system_id), None) => Some(RequestedPolicyScope::FileSystem(
            file_system_id.to_string(),
        )),
        (None, Some(svm_id)) => Some(RequestedPolicyScope::StorageVirtualMachine(
            svm_id.to_string(),
        )),
        _ => None,
    }
}

/// The `BadRequest$Message` for a create naming neither or both of the two resources a policy
/// can be created on, in the words AWS documents the requirement with
/// (`CreateSnapmirrorPolicy$FileSystemId`, `CreateSnapshotPolicy$FileSystemId`). Entirely
/// fixed text: it states the requirement and carries no value from the request.
pub const POLICY_SCOPE_MESSAGE: &str =
    "You must specify either a FileSystemId or a StorageVirtualMachineId, but not both.";

/// The `BadRequest$Message` for a `SnapshotCopyMode` and `RetentionRules` combination the
/// documentation refuses, or `None` when the combination is allowed.
///
/// From `CreateSnapmirrorPolicy$SnapshotCopyMode`, member by member: with
/// `COPY_ALL_SOURCE_SNAPSHOTS` and with `COPY_LATEST_SOURCE_SNAPSHOT` "you can't specify
/// `RetentionRules` along with this property"; with `NO_CREATE_SNAPSHOT_ON_SOURCE` "you must
/// specify `RetentionRules` along with this property"; and with no mode at all
/// "`RetentionRules` is optional".
///
/// The rule is about the policy's STORED mode and the rules it would hold, so the update
/// operation — which can replace `RetentionRules` but not the mode — applies it against the
/// stored mode rather than carrying a second reading of the same sentence.
pub fn retention_rules_refusal(
    snapshot_copy_mode: Option<&str>,
    retention_rules: &[SnapmirrorPolicyRetentionRule],
) -> Option<String> {
    let mode = snapshot_copy_mode?;
    match mode {
        SNAPSHOT_COPY_MODE_COPY_ALL_SOURCE_SNAPSHOTS
        | SNAPSHOT_COPY_MODE_COPY_LATEST_SOURCE_SNAPSHOT
            if !retention_rules.is_empty() =>
        {
            Some(within_error_message_length(format!(
                "You can't specify RetentionRules when SnapshotCopyMode is {mode}."
            )))
        }
        SNAPSHOT_COPY_MODE_NO_CREATE_SNAPSHOT_ON_SOURCE if retention_rules.is_empty() => {
            Some(within_error_message_length(format!(
                "You must specify RetentionRules when SnapshotCopyMode is {mode}."
            )))
        }
        _ => None,
    }
}

/// One stored SnapMirror policy. Mirrors the modelled `SnapmirrorPolicy` shape so
/// [`SnapmirrorPolicyRecord::to_snapmirror_policy`] is a projection and never a second
/// source of truth.
#[derive(Debug, Clone)]
pub struct SnapmirrorPolicyRecord {
    pub snapmirror_policy_id: String,
    /// The file system the policy belongs to; the record is removed with it.
    pub file_system_id: String,
    /// The SVM the policy was created on, when it was created on one.
    pub storage_virtual_machine_id: Option<String>,
    pub name: String,
    pub snapshot_copy_mode: Option<String>,
    pub retention_rules: Vec<SnapmirrorPolicyRetentionRule>,
    pub transfer_schedule: Option<String>,
    pub network_compression_enabled: bool,
    pub throttle_in_kbps: Option<i64>,
    pub scope: String,
    pub lifecycle: String,
    /// The UUID this store derived from the policy id ([`super::ontap_uuid`]), so every read
    /// of the policy reports the same one.
    pub uuid: String,
    pub resource_arn: String,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<PendingTransition>,
}

impl SnapmirrorPolicyRecord {
    /// This record as the modelled `SnapmirrorPolicy` shape. Every member comes from the
    /// stored record, so a response can never disagree with what a later read sees.
    pub fn to_snapmirror_policy(&self) -> SnapmirrorPolicy {
        SnapmirrorPolicy {
            resource_arn: Some(self.resource_arn.clone()),
            snapmirror_policy_id: Some(self.snapmirror_policy_id.clone()),
            file_system_id: Some(self.file_system_id.clone()),
            storage_virtual_machine_id: self.storage_virtual_machine_id.clone(),
            name: Some(self.name.clone()),
            snapshot_copy_mode: self.snapshot_copy_mode.clone(),
            // A policy with no retention rules holds none, so there is no list to report
            // rather than an empty one.
            retention_rules: (!self.retention_rules.is_empty())
                .then(|| self.retention_rules.clone()),
            transfer_schedule: self.transfer_schedule.clone(),
            network_compression_enabled: Some(self.network_compression_enabled),
            throttle_in_kbps: self.throttle_in_kbps,
            scope: Some(self.scope.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            uuid: Some(self.uuid.clone()),
            // Nothing failed: a policy this slice stores was accepted, and the states that
            // would fail it are refused on the create instead of stored as a failed policy.
            lifecycle_transition_reason: None,
            tags: tag_list(&self.tags),
        }
    }
}

impl DescribedRecord for SnapmirrorPolicyRecord {
    fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        policy_filtered_value(
            filter_name,
            &self.file_system_id,
            self.storage_virtual_machine_id.as_deref(),
        )
    }
}

/// One stored snapshot policy. Mirrors the modelled `SnapshotPolicy` shape so
/// [`SnapshotPolicyRecord::to_snapshot_policy`] is a projection and never a second source of
/// truth.
#[derive(Debug, Clone)]
pub struct SnapshotPolicyRecord {
    pub snapshot_policy_id: String,
    /// The file system the policy belongs to; the record is removed with it.
    pub file_system_id: String,
    /// The SVM the policy was created on, when it was created on one.
    pub storage_virtual_machine_id: Option<String>,
    pub name: String,
    pub enabled: bool,
    pub schedules: Vec<SnapshotPolicySchedule>,
    pub scope: String,
    pub lifecycle: String,
    /// The UUID this store derived from the policy id ([`super::ontap_uuid`]), so every read
    /// of the policy reports the same one.
    pub uuid: String,
    pub resource_arn: String,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<PendingTransition>,
}

impl SnapshotPolicyRecord {
    /// This record as the modelled `SnapshotPolicy` shape. Every member comes from the
    /// stored record, so a response can never disagree with what a later read sees.
    pub fn to_snapshot_policy(&self) -> SnapshotPolicy {
        SnapshotPolicy {
            resource_arn: Some(self.resource_arn.clone()),
            snapshot_policy_id: Some(self.snapshot_policy_id.clone()),
            file_system_id: Some(self.file_system_id.clone()),
            storage_virtual_machine_id: self.storage_virtual_machine_id.clone(),
            name: Some(self.name.clone()),
            enabled: Some(self.enabled),
            // The modelled `SnapshotPolicyScheduleList` has `length min 1` and the create
            // requires it, so a stored policy always holds at least one schedule.
            schedules: Some(self.schedules.clone()),
            scope: Some(self.scope.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            uuid: Some(self.uuid.clone()),
            lifecycle_transition_reason: None,
            tags: tag_list(&self.tags),
        }
    }
}

impl DescribedRecord for SnapshotPolicyRecord {
    fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        policy_filtered_value(
            filter_name,
            &self.file_system_id,
            self.storage_virtual_machine_id.as_deref(),
        )
    }
}

/// The stored value a policy `Filter` selects on, for either family: the file system the
/// policy belongs to, or the SVM it was created on.
///
/// A policy created on a FILE SYSTEM is on no SVM, so no value of the
/// `storage-virtual-machine-id` filter selects it — which selects nothing rather than
/// everything.
fn policy_filtered_value<'record>(
    filter_name: &str,
    file_system_id: &'record str,
    storage_virtual_machine_id: Option<&'record str>,
) -> Option<&'record str> {
    match filter_name {
        ONTAP_FILTER_FILE_SYSTEM_ID => Some(file_system_id),
        ONTAP_FILTER_STORAGE_VIRTUAL_MACHINE_ID => storage_virtual_machine_id,
        _ => None,
    }
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every policy lifecycle transition the clock has already made due, for both
    /// families. Called from
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`], which
    /// every handler runs.
    pub(super) fn settle_due_ontap_policy_transitions(&mut self, now: Instant) {
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.snapmirror_policies {
            match due_target(record.pending, now) {
                Some(TransitionTarget::Available) => {
                    record.lifecycle = ONTAP_LIFECYCLE_CREATED.to_string();
                    record.pending = None;
                }
                Some(TransitionTarget::Removed) => removed.push(id.clone()),
                None => {}
            }
        }
        for id in removed.drain(..) {
            self.snapmirror_policies.remove(&id);
        }
        for (id, record) in &mut self.snapshot_policies {
            match due_target(record.pending, now) {
                Some(TransitionTarget::Available) => {
                    record.lifecycle = ONTAP_LIFECYCLE_CREATED.to_string();
                    record.pending = None;
                }
                Some(TransitionTarget::Removed) => removed.push(id.clone()),
                None => {}
            }
        }
        for id in removed {
            self.snapshot_policies.remove(&id);
        }
    }

    /// Store a newly created SnapMirror policy, refusing once its file system holds
    /// [`MAX_POLICIES_PER_FILE_SYSTEM`] of them.
    ///
    /// The ONE write path for a new record, so the bound cannot be bypassed. Replacing a
    /// record that is already stored is not growth, so it is always allowed.
    pub fn insert_snapmirror_policy(
        &mut self,
        record: SnapmirrorPolicyRecord,
    ) -> Result<(), PolicyLimitReached> {
        if self
            .snapmirror_policies
            .contains_key(&record.snapmirror_policy_id)
        {
            self.snapmirror_policies
                .insert(record.snapmirror_policy_id.clone(), record);
            return Ok(());
        }
        if self
            .snapmirror_policies
            .values()
            .filter(|held| held.file_system_id == record.file_system_id)
            .count()
            >= MAX_POLICIES_PER_FILE_SYSTEM
        {
            return Err(PolicyLimitReached);
        }
        self.snapmirror_policies
            .insert(record.snapmirror_policy_id.clone(), record);
        Ok(())
    }

    /// Store a newly created snapshot policy, refusing once its file system holds
    /// [`MAX_POLICIES_PER_FILE_SYSTEM`] of them. The ONE write path for a new record, as
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_snapmirror_policy`] is for
    /// the other family.
    pub fn insert_snapshot_policy(
        &mut self,
        record: SnapshotPolicyRecord,
    ) -> Result<(), PolicyLimitReached> {
        if self
            .snapshot_policies
            .contains_key(&record.snapshot_policy_id)
        {
            self.snapshot_policies
                .insert(record.snapshot_policy_id.clone(), record);
            return Ok(());
        }
        if self
            .snapshot_policies
            .values()
            .filter(|held| held.file_system_id == record.file_system_id)
            .count()
            >= MAX_POLICIES_PER_FILE_SYSTEM
        {
            return Err(PolicyLimitReached);
        }
        self.snapshot_policies
            .insert(record.snapshot_policy_id.clone(), record);
        Ok(())
    }

    /// Move one stored SnapMirror policy into `DELETING` and let the clock remove it,
    /// answering the record as it now stands.
    ///
    /// `None` when the store holds no such policy, which the caller answers with the
    /// modelled `SnapmirrorPolicyNotFound`. A policy that is ALREADY deleting is answered as
    /// it stands rather than moved again, so a retried delete answers as the first one did
    /// (the operation is `@idempotent`).
    pub fn begin_snapmirror_policy_deletion(
        &mut self,
        snapmirror_policy_id: &str,
    ) -> Option<&SnapmirrorPolicyRecord> {
        let record = self.snapmirror_policies.get_mut(snapmirror_policy_id)?;
        if record.lifecycle != ONTAP_LIFECYCLE_DELETING {
            record.lifecycle = ONTAP_LIFECYCLE_DELETING.to_string();
            record.pending = Some(PendingTransition::due_now_plus_span(
                TransitionTarget::Removed,
            ));
        }
        Some(record)
    }

    /// Move one stored snapshot policy into `DELETING` and let the clock remove it. See
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::begin_snapmirror_policy_deletion`].
    pub fn begin_snapshot_policy_deletion(
        &mut self,
        snapshot_policy_id: &str,
    ) -> Option<&SnapshotPolicyRecord> {
        let record = self.snapshot_policies.get_mut(snapshot_policy_id)?;
        if record.lifecycle != ONTAP_LIFECYCLE_DELETING {
            record.lifecycle = ONTAP_LIFECYCLE_DELETING.to_string();
            record.pending = Some(PendingTransition::due_now_plus_span(
                TransitionTarget::Removed,
            ));
        }
        Some(record)
    }

    /// Remove every policy of `file_system_id`, of both families.
    ///
    /// A policy belongs to the file system it was created on, so the records go when the
    /// file system does — which is also what keeps these stores bounded by the live file
    /// systems.
    pub fn remove_ontap_policies_of_file_system(&mut self, file_system_id: &str) {
        self.snapmirror_policies
            .retain(|_, record| record.file_system_id != file_system_id);
        self.snapshot_policies
            .retain(|_, record| record.file_system_id != file_system_id);
    }

    /// Mint the next `SnapmirrorPolicyId`.
    ///
    /// Shaped to the modelled `SnapmirrorPolicyId` pattern
    /// `^(snapmirrorpol-[0-9a-f]{17})$`: `snapmirrorpol-` plus exactly 17 lowercase hex
    /// digits, the form AWS documents as "the system-generated, unique ID of the SnapMirror
    /// policy".
    pub fn mint_snapmirror_policy_id(&mut self) -> String {
        self.next_snapmirror_policy_id_counter += 1;
        format!(
            "snapmirrorpol-{:017x}",
            self.next_snapmirror_policy_id_counter
        )
    }

    /// Mint the next `SnapshotPolicyId`.
    ///
    /// Shaped to the modelled `SnapshotPolicyId` pattern `^snapshotpol-[0-9a-f]{17}$` with
    /// `length 29..29`: `snapshotpol-` plus exactly 17 lowercase hex digits, the form AWS
    /// documents as "the system-generated, unique ID of the snapshot policy".
    pub fn mint_snapshot_policy_id(&mut self) -> String {
        self.next_snapshot_policy_id_counter += 1;
        format!("snapshotpol-{:017x}", self.next_snapshot_policy_id_counter)
    }
}

/// The transition `pending` owes at `now`, or `None` when it owes none yet.
fn due_target(pending: Option<PendingTransition>, now: Instant) -> Option<TransitionTarget> {
    pending
        .filter(|pending| pending.due <= now)
        .map(|pending| pending.target)
}

/// An insert was refused: the file system already holds [`MAX_POLICIES_PER_FILE_SYSTEM`]
/// policies of that family. See that constant for which error the caller answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PolicyLimitReached;

/// The `SnapmirrorPolicyNotFound$Message` for a policy id this store holds nothing for,
/// from the shared wording in [`super::ontap_not_found_message`] under this resource's own
/// name.
pub fn snapmirror_policy_not_found_message(snapmirror_policy_id: &str) -> String {
    super::ontap_not_found_message(SNAPMIRROR_POLICY, snapmirror_policy_id)
}

/// The `SnapshotPolicyNotFound$Message` for a policy id this store holds nothing for. See
/// [`snapmirror_policy_not_found_message`].
pub fn snapshot_policy_not_found_message(snapshot_policy_id: &str) -> String {
    super::ontap_not_found_message(SNAPSHOT_POLICY, snapshot_policy_id)
}

/// The names AWS writes these two resources with in prose, which their messages are built
/// from.
const SNAPMIRROR_POLICY: &str = "SnapMirror policy";
/// See [`SNAPMIRROR_POLICY`].
const SNAPSHOT_POLICY: &str = "Snapshot policy";

/// The `BadRequest$Message` for a create that would hold more policies of one family than
/// [`MAX_POLICIES_PER_FILE_SYSTEM`], worded once beside the bound itself.
pub fn policy_limit_message(policies: &str) -> String {
    within_error_message_length(format!(
        "This file system already has the maximum of {MAX_POLICIES_PER_FILE_SYSTEM} \
         {policies}."
    ))
}

/// The `BadRequest$Message` for a `Filter` whose name a policy listing does not select by,
/// from the shared wording in [`super::unsupported_filter_name_message_for`] with the
/// listing's own accepted names.
pub fn unsupported_policy_filter_name_message(operation: &str, filter_name: &str) -> String {
    super::unsupported_filter_name_message_for(operation, POLICY_FILTER_NAMES, filter_name)
}

/// The `BadRequest$Message` for an update of a SnapMirror policy the service is deleting,
/// from the shared wording in [`super::ontap_is_deleting_message`].
pub fn snapmirror_policy_is_deleting_message(snapmirror_policy_id: &str) -> String {
    super::ontap_is_deleting_message(SNAPMIRROR_POLICY, snapmirror_policy_id)
}

/// The `BadRequest$Message` for an update of a snapshot policy the service is deleting. See
/// [`snapmirror_policy_is_deleting_message`].
pub fn snapshot_policy_is_deleting_message(snapshot_policy_id: &str) -> String {
    super::ontap_is_deleting_message(SNAPSHOT_POLICY, snapshot_policy_id)
}

/// The pagination scope of one policy listing, from the shared [`super::filtered_page_scope`]
/// under that listing's own name.
pub fn policies_page_scope(
    listing: &str,
    selected: &[String],
    filters: &[(String, Vec<String>)],
) -> String {
    super::filtered_page_scope(listing, selected, filters)
}

/// The ARN of a stored SnapMirror policy, through the one canonical builder
/// ([`super::fsx_arn`]).
pub fn snapmirror_policy_arn(region: &str, snapmirror_policy_id: &str) -> String {
    fsx_arn(
        region,
        SNAPMIRROR_POLICY_RESOURCE_TYPE,
        snapmirror_policy_id,
    )
}

/// The ARN of a stored snapshot policy, through the one canonical builder
/// ([`super::fsx_arn`]).
pub fn snapshot_policy_arn(region: &str, snapshot_policy_id: &str) -> String {
    fsx_arn(region, SNAPSHOT_POLICY_RESOURCE_TYPE, snapshot_policy_id)
}
