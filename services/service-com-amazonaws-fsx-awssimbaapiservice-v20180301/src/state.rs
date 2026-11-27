// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-fsx-awssimbaapiservice-v20180301 service.
//!
//! This holds the file-system lifecycle slice: the records `CreateFileSystem` writes and
//! that `DescribeFileSystems`, `UpdateFileSystem`, `DeleteFileSystem`, `TagResource`,
//! `UntagResource` and `ListTagsForResource` read and mutate, plus the backup slice in
//! [`backups`]: the records `CreateBackup` writes and `DescribeBackups` reads. Every
//! handler reaches it through [`service_state`]; nothing here is per-request.
//!
//! `src/lib.rs` and `src/ops/mod.rs` are generated, so this module is also the only place
//! this crate can keep code more than one operation needs: the record projection onto the
//! modelled `FileSystem` shape, FSx ARN parsing, and the opaque pagination cursor.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    CreateFileSystemLustreConfiguration, CreateFileSystemOntapConfiguration,
    CreateFileSystemOpenZFSConfiguration, CreateFileSystemWindowsConfiguration,
    DataRepositoryConfiguration, DiskIopsConfiguration, FileSystem,
    FileSystemLustreMetadataConfiguration,
    LustreFileSystemConfiguration, LustreLogConfiguration, OntapFileSystemConfiguration,
    OpenZFSFileSystemConfiguration, SelfManagedActiveDirectoryAttributes, Tag,
    UpdateFileSystemLustreConfiguration, UpdateFileSystemOntapConfiguration,
    UpdateFileSystemOpenZFSConfiguration, UpdateFileSystemWindowsConfiguration,
    WindowsAuditLogConfiguration, WindowsFileSystemConfiguration,
};

/// `FileSystemLifecycle` members this slice uses, from the modelled enum
/// (`smithy_query(query="shape", shape="FileSystemLifecycle")`:
/// AVAILABLE | CREATING | FAILED | DELETING | MISCONFIGURED | UPDATING |
/// MISCONFIGURED_UNAVAILABLE).
pub const LIFECYCLE_CREATING: &str = "CREATING";
/// See [`LIFECYCLE_CREATING`].
pub const LIFECYCLE_AVAILABLE: &str = "AVAILABLE";
/// See [`LIFECYCLE_CREATING`].
pub const LIFECYCLE_UPDATING: &str = "UPDATING";
/// See [`LIFECYCLE_CREATING`].
pub const LIFECYCLE_DELETING: &str = "DELETING";
/// See [`LIFECYCLE_CREATING`].
pub const LIFECYCLE_MISCONFIGURED: &str = "MISCONFIGURED";

/// `StorageType` default. AWS documents "Default value is SSD" on
/// `CreateFileSystem$StorageType`.
pub const DEFAULT_STORAGE_TYPE: &str = "SSD";

/// `NetworkType` default. AWS documents "The default is IPV4" on
/// `CreateFileSystem$NetworkType`.
pub const DEFAULT_NETWORK_TYPE: &str = "IPV4";

/// `DiskIopsConfigurationMode` values, from the modelled enum
/// (`smithy_query(query="shape", shape="DiskIopsConfigurationMode")`:
/// AUTOMATIC | USER_PROVISIONED).
pub const DISK_IOPS_MODE_AUTOMATIC: &str = "AUTOMATIC";
/// See [`DISK_IOPS_MODE_AUTOMATIC`].
pub const DISK_IOPS_MODE_USER_PROVISIONED: &str = "USER_PROVISIONED";

/// The SSD IOPS a file system is provisioned per GiB of storage capacity while its
/// disk-IOPS mode is `AUTOMATIC`. AWS documents the rate twice: `DiskIopsConfiguration`
/// ("By default, Amazon FSx automatically provisions 3 IOPS per GB of storage capacity")
/// and `DiskIopsConfiguration$Iops` ("calculated as `StorageCapacity` * 3 * `HAPairs` (3
/// IOPS per GB of `StorageCapacity`)").
pub const AUTOMATIC_IOPS_PER_GIB: i64 = 3;

/// The high-availability pairs a file system is powered by when its configuration names
/// none, from `CreateFileSystemOntapConfiguration$HAPairs` ("The default value is 1").
/// ONTAP is the only configuration that models the member; the other file-system types
/// are powered by one pair.
pub const DEFAULT_HA_PAIRS: i32 = 1;

/// The largest `Iops` the modelled `DiskIopsConfiguration$Iops` long can report, from its
/// own `range 0..2147483647`: the computed `AUTOMATIC` value stays inside the member it is
/// reported in.
pub const MAX_DISK_IOPS: i64 = 2_147_483_647;

/// The region used when a request carries no SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long a clock-owed lifecycle transition (CREATING -> AVAILABLE,
/// UPDATING -> AVAILABLE, DELETING -> gone) stays pending.
///
/// CHOSEN, not documented by AWS: the real service takes minutes, which no test can wait
/// for. It is deliberately non-zero so a caller that acts immediately after the call still
/// observes the transient lifecycle and the state errors that go with it, and it is never
/// slept on — the due instant is stored and materialised by whichever later call observes
/// the record.
pub const LIFECYCLE_TRANSITION: Duration = Duration::from_millis(500);

/// The largest page `DescribeFileSystems` returns, from the modelled `FileSystems` list's
/// own `length: max 50` (`smithy_query(query="operation", operation="DescribeFileSystems")`).
pub const MAX_FILE_SYSTEMS_PER_PAGE: usize = 50;

/// The largest page `ListTagsForResource` returns, from the modelled `Tags` list's own
/// `length: max 50`.
pub const MAX_TAGS_PER_PAGE: usize = 50;

/// The most tags one resource can carry, from the same modelled `Tags` list
/// (`length: max 50`, "A list of Tag values, with a maximum of 50 elements"): a resource
/// whose tag map grew past that could not be reported back in `FileSystem$Tags` at all,
/// so `TagResource` refuses the merge that would exceed it instead of storing it.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most file systems this store holds at once.
///
/// AWS publishes a default quota of 100 file systems per account per Region, and models
/// the refusal as `ServiceLimitExceeded` with `ServiceLimit::FILE_SYSTEM_COUNT` ("The
/// types of limits on your service utilization. Limits include file system count ...
/// These limits apply for a specific account in a specific Amazon Web Services Region").
/// So the record store, which a caller grows one entry per `CreateFileSystem`, is bounded
/// by the service's own limit rather than by the size of the process: a create past it is
/// refused with that modelled error, and deleting a file system makes room again.
pub const MAX_FILE_SYSTEMS: usize = 100;

/// The longest `Message` an error of this service can carry, from the modelled
/// `ErrorMessage` string's own `length 1..256`
/// (`smithy_query(query="shape", shape="ErrorMessage")`).
pub const MAX_ERROR_MESSAGE_LEN: usize = 256;

/// The modelled `ServiceLimit` value that goes with [`MAX_FILE_SYSTEMS`]
/// (`smithy_query(query="shape", shape="ServiceLimit")`).
pub const SERVICE_LIMIT_FILE_SYSTEM_COUNT: &str = "FILE_SYSTEM_COUNT";

/// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_file_system`] refused the
/// record: the store already holds [`MAX_FILE_SYSTEMS`]. The caller answers with the
/// modelled `ServiceLimitExceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FileSystemCountLimitReached;

/// The most route-table ids one file system's configuration holds.
///
/// From the modelled `RouteTableIds` list (`length max 50`), which bounds the member on
/// both `OntapFileSystemConfiguration` and `OpenZFSFileSystemConfiguration`: a stored list
/// longer than that could not be reported back at all. `UpdateFileSystem`'s
/// `AddRouteTableIds` is applied through [`merged_route_table_ids`], which refuses the
/// merge that would exceed this instead of growing the stored list without end.
pub const MAX_ROUTE_TABLE_IDS: usize = 50;

/// [`merged_route_table_ids`] refused the merge: it would hold more than
/// [`MAX_ROUTE_TABLE_IDS`] route-table ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RouteTableIdLimitReached;

/// The most `ClientRequestToken` replays this service remembers.
///
/// The token map is the one piece of state a caller grows without creating a resource —
/// one entry per create call carrying a token — so it is bounded here and evicted
/// oldest-first. Well above any real client's in-flight retries, so a legitimate replay
/// is never forgotten before its retry arrives; a token evicted (or whose file system is
/// gone) simply replays nothing and the next call creates a new file system.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// What a record owes the clock, and when.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingTransition {
    /// When the transition falls due. Never slept on; [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`]
    /// applies it on the first call at or after this instant.
    pub due: Instant,
    pub target: TransitionTarget,
}

/// Where a [`PendingTransition`] lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionTarget {
    /// The file system becomes `AVAILABLE` (out of `CREATING` or `UPDATING`).
    Available,
    /// The file system is gone (out of `DELETING`).
    Removed,
}

impl PendingTransition {
    /// A transition of [`LIFECYCLE_TRANSITION`] from now.
    pub fn due_now_plus_span(target: TransitionTarget) -> Self {
        Self {
            due: Instant::now() + LIFECYCLE_TRANSITION,
            target,
        }
    }
}

/// One stored file system. Mirrors the modelled `FileSystem` shape so
/// [`FileSystemRecord::to_file_system`] is a projection and never a second source of truth.
#[derive(Debug, Clone)]
pub struct FileSystemRecord {
    pub file_system_id: String,
    pub owner_id: String,
    pub creation_time: SystemTime,
    pub file_system_type: String,
    pub lifecycle: String,
    pub storage_capacity: Option<i32>,
    pub storage_type: String,
    /// The VPC the file system's subnets belong to, as [`resolve_requested_subnets`]
    /// resolved them when the file system was created: the VPC of the account's network
    /// the requested subnets sit in, never an invented id.
    pub vpc_id: Option<String>,
    pub subnet_ids: Vec<String>,
    /// Kept as stored: AWS documents that `SecurityGroupIds` "isn't returned in later
    /// requests to describe the file system", and the modelled `FileSystem` shape has no
    /// member for it.
    pub security_group_ids: Vec<String>,
    /// The elastic network interfaces FSx creates for the file system. Empty: the
    /// account's network model carries no interfaces, and an `eni-...` id for an interface
    /// nothing allocated would report a resource that does not exist.
    pub network_interface_ids: Vec<String>,
    pub dns_name: String,
    pub kms_key_id: Option<String>,
    pub resource_arn: String,
    pub file_system_type_version: Option<String>,
    pub network_type: String,
    pub windows_configuration: Option<WindowsFileSystemConfiguration>,
    pub lustre_configuration: Option<LustreFileSystemConfiguration>,
    pub ontap_configuration: Option<OntapFileSystemConfiguration>,
    pub open_zfs_configuration: Option<OpenZFSFileSystemConfiguration>,
    pub tags: BTreeMap<String, String>,
    /// The asynchronous work an `UpdateFileSystem` has STARTED on this file system, oldest
    /// first, which `FileSystem$AdministrativeActions` reports. Written only through
    /// [`FileSystemRecord::record_administrative_action`], which bounds it; settled by
    /// [`FileSystemRecord::settle_due_administrative_actions`]. See
    /// [`administrative_actions`].
    pub administrative_actions: Vec<AdministrativeActionRecord>,
    pub pending: Option<PendingTransition>,
}

impl FileSystemRecord {
    /// This record as the modelled `FileSystem` shape. Every member comes from the stored
    /// record, so a response can never disagree with what a later read sees.
    pub fn to_file_system(&self) -> FileSystem {
        FileSystem {
            owner_id: Some(self.owner_id.clone()),
            creation_time: Some(epoch_date_time(self.creation_time)),
            file_system_id: Some(self.file_system_id.clone()),
            file_system_type: Some(self.file_system_type.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            failure_details: None,
            storage_capacity: self.storage_capacity,
            storage_type: Some(self.storage_type.clone()),
            vpc_id: self.vpc_id.clone(),
            subnet_ids: Some(self.subnet_ids.clone()),
            network_interface_ids: Some(self.network_interface_ids.clone()),
            dns_name: Some(self.dns_name.clone()),
            kms_key_id: self.kms_key_id.clone(),
            resource_arn: Some(self.resource_arn.clone()),
            tags: tag_list(&self.tags),
            windows_configuration: self.windows_configuration.clone(),
            lustre_configuration: self.lustre_configuration.clone(),
            // The work an `UpdateFileSystem` started and the clock either still owes or has
            // completed, read off the stored actions: a storage-capacity target is carried
            // out by one of these rather than applied in line, so this is what a caller
            // polls. `None` rather than an empty list when there is none, so a file system
            // nothing has been asked of reports no action at all.
            administrative_actions: (!self.administrative_actions.is_empty()).then(|| {
                self.administrative_actions
                    .iter()
                    .map(AdministrativeActionRecord::to_administrative_action)
                    .collect()
            }),
            ontap_configuration: self.ontap_configuration.clone(),
            file_system_type_version: self.file_system_type_version.clone(),
            open_zfs_configuration: self.open_zfs_configuration.clone(),
            network_type: Some(self.network_type.clone()),
        }
    }

    /// Materialise the provisioned disk-IOPS configuration of every per-type configuration
    /// this record carries, from the record's own storage capacity.
    ///
    /// The ONE place the documented default is applied, so a file system HAS the member
    /// from the moment it is created and keeps it in step with its capacity: a store that
    /// kept the request's omission would report a file system with no provisioned SSD IOPS,
    /// which AWS never does. Called on every path that writes a record's capacity or its
    /// per-type configuration, so the stored record — and therefore every read of it, the
    /// copy a backup keeps included — agrees with it.
    ///
    /// Lustre is left out because the modelled `LustreFileSystemConfiguration` has no
    /// `DiskIopsConfiguration` member at all; `FileSystemLustreMetadataConfiguration$Iops`
    /// is a different, metadata-only value with its own modes.
    pub fn materialize_provisioned_disk_iops(&mut self) {
        let storage_capacity = self.storage_capacity;
        if let Some(windows) = self.windows_configuration.as_mut() {
            windows.disk_iops_configuration = provisioned_disk_iops(
                windows.disk_iops_configuration.as_ref(),
                storage_capacity,
                None,
            );
        }
        if let Some(ontap) = self.ontap_configuration.as_mut() {
            let ha_pairs = ontap.ha_pairs;
            ontap.disk_iops_configuration = provisioned_disk_iops(
                ontap.disk_iops_configuration.as_ref(),
                storage_capacity,
                ha_pairs,
            );
        }
        if let Some(open_zfs) = self.open_zfs_configuration.as_mut() {
            open_zfs.disk_iops_configuration = provisioned_disk_iops(
                open_zfs.disk_iops_configuration.as_ref(),
                storage_capacity,
                None,
            );
        }
    }
}

/// Per-service state for com-amazonaws-fsx-awssimbaapiservice-v20180301. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Stored file systems, keyed by `FileSystemId`. A `BTreeMap` (rather than a
    /// `HashMap`) because `DescribeFileSystems` pages over it: a cursor is only meaningful
    /// over a stable order, and minted ids sort in creation order.
    pub file_systems: BTreeMap<String, FileSystemRecord>,
    /// `ClientRequestToken` -> the replay it stands for, for the `@idempotencyToken`
    /// behaviour on `CreateFileSystem`. Written only through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::remember_idempotent_create`],
    /// which bounds it.
    pub idempotency_tokens: HashMap<String, IdempotentCreate>,
    /// Insertion order of `idempotency_tokens`, oldest first. This is what lets that map
    /// be bounded: it is memory a caller can grow by one entry per request, so the oldest
    /// remembered replay is dropped once [`MAX_IDEMPOTENCY_TOKENS`] is reached.
    token_order: VecDeque<String>,
    /// Source of minted `FileSystemId`s.
    pub next_id_counter: u64,
    /// Stored backups, keyed by `BackupId`. A `BTreeMap` for the same reason as
    /// `file_systems`: `DescribeBackups` pages over it, and minted ids sort in creation
    /// order. See [`backups`] for the record and what a backup keeps of its file system.
    /// BOUNDED by [`MAX_BACKUPS`], and per file system by
    /// [`MAX_USER_INITIATED_BACKUPS_PER_FILE_SYSTEM`]; write it through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_backup`], which enforces
    /// both.
    pub backups: BTreeMap<String, BackupRecord>,
    /// How many entries of `backups` were taken of each `FileSystemId`: the per-file-system
    /// bound asked in constant time instead of by walking every record under the shared
    /// lock. Maintained by `insert_backup` and `remove_backup`, the only two paths that
    /// change `backups`, and read through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::backup_count_of_file_system`] —
    /// private, so it cannot drift from the records.
    backups_per_file_system: HashMap<String, usize>,
    /// `ClientRequestToken` -> the replay it stands for, for the `@idempotencyToken`
    /// behaviour on `CreateBackup`. Written only through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::remember_idempotent_backup`],
    /// which bounds it. Separate from `idempotency_tokens` because a token is scoped to the
    /// operation that minted it: one `CreateFileSystem` replays cannot be served as a
    /// `CreateBackup` replay.
    pub backup_idempotency_tokens: HashMap<String, IdempotentBackup>,
    /// Insertion order of `backup_idempotency_tokens`, oldest first — what lets that map be
    /// bounded, exactly as `token_order` bounds the other one.
    backup_token_order: VecDeque<String>,
    /// Source of minted `BackupId`s.
    pub next_backup_id_counter: u64,
    /// `ClientRequestToken` -> the `BackupId` a `DeleteBackup` carrying that token removed,
    /// for the `@idempotencyToken` behaviour AWS documents as "idempotent deletion": a
    /// retried delete answers as the first one did instead of `BackupNotFound`. Written only
    /// through [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::remember_backup_deletion`],
    /// which bounds it.
    pub deleted_backup_tokens: HashMap<String, String>,
    /// Insertion order of `deleted_backup_tokens`, oldest first — what lets that map be
    /// bounded, exactly as `token_order` bounds the first one.
    deleted_backup_token_order: VecDeque<String>,
    /// Stored volumes, keyed by `VolumeId`. A `BTreeMap` for the same reason as
    /// `file_systems`: `DescribeVolumes` pages over it, and minted ids sort in creation
    /// order. See [`volumes`] for the record and which volumes this slice holds. BOUNDED by
    /// [`MAX_VOLUMES`], and per file system by [`MAX_VOLUMES_PER_FILE_SYSTEM`]; write it
    /// through [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_volume`], which
    /// enforces both.
    pub volumes: BTreeMap<String, VolumeRecord>,
    /// Source of minted `VolumeId`s.
    pub next_volume_id_counter: u64,
    /// Stored data repository associations, keyed by `AssociationId`. A `BTreeMap` for the
    /// same reason as `file_systems`: `DescribeDataRepositoryAssociations` pages over it, and
    /// minted ids sort in creation order. See [`data_repository_associations`] for the record.
    /// BOUNDED by [`MAX_DATA_REPOSITORY_ASSOCIATIONS`], and per file system by
    /// [`MAX_DATA_REPOSITORY_ASSOCIATIONS_PER_FILE_SYSTEM`]; write it through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_data_repository_association`],
    /// which enforces both.
    pub data_repository_associations: BTreeMap<String, DataRepositoryAssociationRecord>,
    /// Source of minted `AssociationId`s.
    pub next_data_repository_association_id_counter: u64,
    /// `ClientRequestToken` -> the replay it stands for, for the `@idempotencyToken`
    /// behaviour on `CreateDataRepositoryAssociation`. Written only through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::remember_idempotent_data_repository_association`],
    /// which bounds it. Separate from the other token maps because a token is scoped to the
    /// operation that minted it.
    pub data_repository_association_idempotency_tokens:
        HashMap<String, IdempotentDataRepositoryAssociation>,
    /// Insertion order of `data_repository_association_idempotency_tokens`, oldest first —
    /// what lets that map be bounded, exactly as `token_order` bounds the first one.
    data_repository_association_token_order: VecDeque<String>,
    /// Stored cluster peers, keyed by `ClusterPeerId`. A `BTreeMap` for the same reason as
    /// `file_systems`: `DescribeClusterPeers` pages over it, and minted ids sort in creation
    /// order. See [`cluster_peers`] for the record. BOUNDED per file system by
    /// [`MAX_CLUSTER_PEERS_PER_FILE_SYSTEM`]; write it through
    /// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_cluster_peer`], which
    /// enforces that.
    pub cluster_peers: BTreeMap<String, ClusterPeerRecord>,
    /// Source of minted `ClusterPeerId`s.
    pub next_cluster_peer_id_counter: u64,
    /// `ClientRequestToken` -> the replay it stands for, for the `@idempotencyToken`
    /// behaviour on `CreateClusterPeer`. See [`IdempotentCreates`], which bounds it.
    pub cluster_peer_tokens: IdempotentCreates<CreatedByToken>,
    /// Stored SnapMirror policies, keyed by `SnapmirrorPolicyId`, and the tokens
    /// `CreateSnapmirrorPolicy` replays — bounded exactly as the cluster-peer pair above is.
    /// See [`ontap_policies`] for the record.
    pub snapmirror_policies: BTreeMap<String, SnapmirrorPolicyRecord>,
    /// Source of minted `SnapmirrorPolicyId`s.
    pub next_snapmirror_policy_id_counter: u64,
    /// See [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::cluster_peer_tokens`], for
    /// `CreateSnapmirrorPolicy`.
    pub snapmirror_policy_tokens: IdempotentCreates<CreatedByToken>,
    /// Stored snapshot policies, keyed by `SnapshotPolicyId`. See [`ontap_policies`].
    pub snapshot_policies: BTreeMap<String, SnapshotPolicyRecord>,
    /// Source of minted `SnapshotPolicyId`s.
    pub next_snapshot_policy_id_counter: u64,
    /// See [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::cluster_peer_tokens`], for
    /// `CreateSnapshotPolicy`.
    pub snapshot_policy_tokens: IdempotentCreates<CreatedByToken>,
}

/// What an FSx `ResourceARN` names, once resolved against the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceLookup {
    /// A file system this service stores, by `FileSystemId`.
    FileSystem(String),
    /// A backup this service stores, by `BackupId`.
    Backup(String),
    /// A volume this service stores, by `VolumeId`.
    Volume(String),
    /// A data repository association this service stores, by `AssociationId`.
    DataRepositoryAssociation(String),
    /// A cluster peer this service stores, by `ClusterPeerId`.
    ClusterPeer(String),
    /// A SnapMirror policy this service stores, by `SnapmirrorPolicyId`.
    SnapmirrorPolicy(String),
    /// A snapshot policy this service stores, by `SnapshotPolicyId`.
    SnapshotPolicy(String),
    /// An ARN naming a resource type FSx owns that holds no record here:
    /// `ResourceNotFound`. Carries the resource id from the request's ARN, because the
    /// error's `Message` is built from the identifier the caller sent (see
    /// [`resource_not_found_message`]).
    NotFound(String),
    /// An ARN that names no FSx resource type at all: `NotServiceResourceError`.
    NotFsxResource,
}

/// What a `CreateFileSystem` `ClientRequestToken` replays.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The file system the first call with this token created.
    pub file_system_id: String,
    /// A FIXED-SIZE digest of that call's parameters (see
    /// `create_file_system::create_parameter_fingerprint`). A replay whose parameters
    /// differ is `IncompatibleParameterError`, which AWS documents for this operation.
    /// A digest rather than the parameters themselves, so one remembered token costs a
    /// bounded amount of memory however large the request that minted it was.
    pub fingerprint: String,
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every lifecycle transition the clock has already made due.
    ///
    /// Called at the top of every handler, reporting ones included: this materialises a
    /// change that was owed at this instant whether or not anybody called, so it never
    /// changes what a later caller would otherwise have seen.
    pub fn settle_due_transitions(&mut self) {
        let now = Instant::now();
        self.settle_due_backup_transitions(now);
        self.settle_due_volume_transitions(now);
        self.settle_due_data_repository_association_transitions(now);
        self.settle_due_cluster_peer_transitions(now);
        self.settle_due_ontap_policy_transitions(now);
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.file_systems {
            // The asynchronous work the file system owes the clock, which is settled whatever
            // its lifecycle: a storage-capacity increase runs while the file system stays
            // `AVAILABLE`.
            record.settle_due_administrative_actions(now);
            let Some(pending) = record.pending else {
                continue;
            };
            if pending.due > now {
                continue;
            }
            match pending.target {
                TransitionTarget::Available => {
                    record.lifecycle = LIFECYCLE_AVAILABLE.to_string();
                    record.pending = None;
                }
                TransitionTarget::Removed => removed.push(id.clone()),
            }
        }
        for id in removed {
            self.file_systems.remove(&id);
            // A volume belongs to its file system, so the records go with it rather than
            // outliving the file system that had them.
            self.remove_volumes_of_file_system(&id);
            // A data repository association belongs to its file system too, for the same
            // reason.
            self.remove_data_repository_associations_of_file_system(&id);
            // A cluster peer and a policy belong to the file system they were created on,
            // for the same reason.
            self.remove_cluster_peers_of_file_system(&id);
            self.remove_ontap_policies_of_file_system(&id);
            // A token whose file system is gone replays nothing, so it is released with
            // the record rather than held for the life of the process.
            self.forget_tokens_for(&id);
        }
    }

    /// Store a newly created file system, refusing once [`MAX_FILE_SYSTEMS`] are held.
    ///
    /// The ONE write path for a new record, so the bound cannot be bypassed by a later
    /// operation that also creates file systems (`CreateFileSystemFromBackup`, say).
    /// Replacing a record that is already stored is not growth, so it is always allowed.
    pub fn insert_file_system(
        &mut self,
        record: FileSystemRecord,
    ) -> Result<(), FileSystemCountLimitReached> {
        let replaces_existing = self.file_systems.contains_key(&record.file_system_id);
        if !replaces_existing && self.file_systems.len() >= MAX_FILE_SYSTEMS {
            return Err(FileSystemCountLimitReached);
        }
        self.file_systems
            .insert(record.file_system_id.clone(), record);
        Ok(())
    }

    /// Remember a `ClientRequestToken` replay, dropping the oldest once
    /// [`MAX_IDEMPOTENCY_TOKENS`] are held.
    ///
    /// The ONE write path for `idempotency_tokens`, so the bound cannot be bypassed.
    pub fn remember_idempotent_create(&mut self, token: String, replay: IdempotentCreate) {
        remember_token(
            &mut self.idempotency_tokens,
            &mut self.token_order,
            token,
            replay,
        );
    }

    /// Forget one `ClientRequestToken`.
    pub fn forget_idempotency_token(&mut self, token: &str) {
        forget_token(&mut self.idempotency_tokens, &mut self.token_order, token);
    }

    /// Forget every `ClientRequestToken` that replays `file_system_id`.
    fn forget_tokens_for(&mut self, file_system_id: &str) {
        self.idempotency_tokens
            .retain(|_, replay| replay.file_system_id != file_system_id);
        let held = &self.idempotency_tokens;
        self.token_order.retain(|token| held.contains_key(token));
    }

    /// What an FSx `ResourceARN` resolves to in this store.
    ///
    /// The tagging operations (`TagResource`, `UntagResource`, `ListTagsForResource`) each
    /// model the same three answers, so the resolution lives here once rather than three
    /// times.
    pub fn resolve_resource(&self, resource_arn: &str) -> ResourceLookup {
        let Some(arn) = parse_fsx_arn(resource_arn) else {
            return ResourceLookup::NotFsxResource;
        };
        if !arn.names_fsx_resource_type() {
            return ResourceLookup::NotFsxResource;
        }
        if arn.resource_type == FILE_SYSTEM_RESOURCE_TYPE
            && self.file_systems.contains_key(&arn.resource_id)
        {
            return ResourceLookup::FileSystem(arn.resource_id);
        }
        if arn.resource_type == BACKUP_RESOURCE_TYPE && self.backups.contains_key(&arn.resource_id)
        {
            return ResourceLookup::Backup(arn.resource_id);
        }
        if arn.resource_type == VOLUME_RESOURCE_TYPE && self.volumes.contains_key(&arn.resource_id)
        {
            return ResourceLookup::Volume(arn.resource_id);
        }
        if arn.resource_type == ASSOCIATION_RESOURCE_TYPE
            && self
                .data_repository_associations
                .contains_key(&arn.resource_id)
        {
            return ResourceLookup::DataRepositoryAssociation(arn.resource_id);
        }
        if arn.resource_type == CLUSTER_PEER_RESOURCE_TYPE
            && self.cluster_peers.contains_key(&arn.resource_id)
        {
            return ResourceLookup::ClusterPeer(arn.resource_id);
        }
        if arn.resource_type == SNAPMIRROR_POLICY_RESOURCE_TYPE
            && self.snapmirror_policies.contains_key(&arn.resource_id)
        {
            return ResourceLookup::SnapmirrorPolicy(arn.resource_id);
        }
        if arn.resource_type == SNAPSHOT_POLICY_RESOURCE_TYPE
            && self.snapshot_policies.contains_key(&arn.resource_id)
        {
            return ResourceLookup::SnapshotPolicy(arn.resource_id);
        }
        // An FSx resource type this slice does not store yet (a snapshot, an SVM) holds no
        // record either, because the operations that would create one are still 501 — so
        // there is genuinely no such resource. The id the ARN named travels with the
        // answer, so the error can name it.
        ResourceLookup::NotFound(arn.resource_id)
    }

    /// The tags of the resource `lookup` resolved to, or `None` when it resolved to none.
    ///
    /// Every FSx resource type carries ONE tag map, and the tagging operations
    /// (`TagResource`, `UntagResource`, `ListTagsForResource`) each work on whichever
    /// resource the request's `ResourceARN` names, so the map is reached through the
    /// resolution rather than through one resource type's own field.
    pub fn resource_tags(&self, lookup: &ResourceLookup) -> Option<&BTreeMap<String, String>> {
        match lookup {
            ResourceLookup::FileSystem(id) => {
                self.file_systems.get(id).map(|record| &record.tags)
            }
            ResourceLookup::Backup(id) => self.backups.get(id).map(|record| &record.tags),
            ResourceLookup::Volume(id) => self.volumes.get(id).map(|record| &record.tags),
            ResourceLookup::DataRepositoryAssociation(id) => self
                .data_repository_associations
                .get(id)
                .map(|record| &record.tags),
            ResourceLookup::ClusterPeer(id) => self.cluster_peers.get(id).map(|record| &record.tags),
            ResourceLookup::SnapmirrorPolicy(id) => {
                self.snapmirror_policies.get(id).map(|record| &record.tags)
            }
            ResourceLookup::SnapshotPolicy(id) => {
                self.snapshot_policies.get(id).map(|record| &record.tags)
            }
            ResourceLookup::NotFound(_) | ResourceLookup::NotFsxResource => None,
        }
    }

    /// The tags of the resource `lookup` resolved to, for the operations that change them.
    /// See [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::resource_tags`].
    pub fn resource_tags_mut(
        &mut self,
        lookup: &ResourceLookup,
    ) -> Option<&mut BTreeMap<String, String>> {
        match lookup {
            ResourceLookup::FileSystem(id) => {
                self.file_systems.get_mut(id).map(|record| &mut record.tags)
            }
            ResourceLookup::Backup(id) => self.backups.get_mut(id).map(|record| &mut record.tags),
            ResourceLookup::Volume(id) => self.volumes.get_mut(id).map(|record| &mut record.tags),
            ResourceLookup::DataRepositoryAssociation(id) => self
                .data_repository_associations
                .get_mut(id)
                .map(|record| &mut record.tags),
            ResourceLookup::ClusterPeer(id) => self
                .cluster_peers
                .get_mut(id)
                .map(|record| &mut record.tags),
            ResourceLookup::SnapmirrorPolicy(id) => self
                .snapmirror_policies
                .get_mut(id)
                .map(|record| &mut record.tags),
            ResourceLookup::SnapshotPolicy(id) => self
                .snapshot_policies
                .get_mut(id)
                .map(|record| &mut record.tags),
            ResourceLookup::NotFound(_) | ResourceLookup::NotFsxResource => None,
        }
    }

    /// Mint the next `FileSystemId`.
    ///
    /// Shaped to the modelled `FileSystemId` pattern `^(fs-[0-9a-f]{8,})$` with
    /// `length 11..21`: `fs-` plus 17 lowercase hex digits, the 20-character form AWS
    /// documents as "the system-generated, unique 17-digit ID of the file system".
    pub fn mint_file_system_id(&mut self) -> String {
        self.next_id_counter += 1;
        format!("fs-{:017x}", self.next_id_counter)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsFsxAwssimbaapiserviceV20180301State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsFsxAwssimbaapiserviceV20180301State>)
}

/// A `SystemTime` as the modelled `CreationTime` timestamp.
pub fn epoch_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let millis = at
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    ::aws_smithy_types::DateTime::from_millis(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// A stored tag map as the modelled `Tags` list, or `None` when there are none: the
/// modelled `Tags` list has `length min 1`, so an empty list is not a legal value.
pub fn tag_list(tags: &BTreeMap<String, String>) -> Option<Vec<Tag>> {
    if tags.is_empty() {
        return None;
    }
    Some(
        tags.iter()
            .map(|(key, value)| Tag {
                key: key.clone(),
                value: value.clone(),
            })
            .collect(),
    )
}

/// The region a request is signed for, which every ARN and DNS name this service mints is
/// built from. `us-east-1` ([`DEFAULT_REGION`]) when the request carries no SigV4 scope.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// A `Message` clamped to the modelled `ErrorMessage` `length 1..256`, cut on a character
/// boundary so a truncated message is still valid UTF-8.
///
/// Every not-found `Message` below goes through this, so the modelled bound holds whatever
/// the request carried — it does not depend on an input constraint being enforced ahead of
/// the handler.
fn within_error_message_length(mut message: String) -> String {
    if message.len() > MAX_ERROR_MESSAGE_LEN {
        let mut cut = MAX_ERROR_MESSAGE_LEN;
        while cut > 0 && !message.is_char_boundary(cut) {
            cut -= 1;
        }
        message.truncate(cut);
    }
    message
}

/// The `FileSystemNotFound$Message` for an operation addressed by a `FileSystemId`
/// (`DescribeFileSystems`, `UpdateFileSystem`): a sentence naming the id the caller asked
/// about, single-quoted and terminated with a period.
///
/// FSx builds this `Message` from the request rather than returning fixed prose — the
/// identifier is what tells a client WHICH of the ids it named was missing — so it is
/// templated here from the requested id and never hand-written at a call site, and clamped
/// to the modelled `ErrorMessage` `length 1..256` by [`within_error_message_length`].
pub fn file_system_not_found_message(file_system_id: &str) -> String {
    within_error_message_length(format!("File system '{file_system_id}' does not exist."))
}

/// The `FileSystemNotFound$Message` `DeleteFileSystem` answers with: the requested
/// `FileSystemId` alone.
///
/// Same rule as [`file_system_not_found_message`] — the `Message` is built from the
/// identifier the caller sent — but this operation's recorded template is the bare id, with
/// no sentence around it, so it has its own constructor rather than silently reusing the
/// other one.
pub fn deleted_file_system_not_found_message(file_system_id: &str) -> String {
    within_error_message_length(file_system_id.to_string())
}

/// The `ResourceNotFound$Message` the tagging operations (`TagResource`, `UntagResource`,
/// `ListTagsForResource`) answer with: a sentence naming the resource id from the request's
/// `ResourceARN`, single-quoted.
///
/// The same rule as [`file_system_not_found_message`], over whatever resource type the ARN
/// names (a file system, a backup, a volume): the identifier in the request is what tells a
/// client which resource could not be found.
pub fn resource_not_found_message(resource_id: &str) -> String {
    within_error_message_length(format!("Resource '{resource_id}' does not exist"))
}

/// `arn:aws:fsx:<region>:<account>:<resource_type>/<resource_id>`, through the one
/// canonical ARN builder so the account segment is the emulator's.
pub fn fsx_arn(region: &str, resource_type: &str, resource_id: &str) -> String {
    ::emulator_server::emulator_arn("fsx", region, &format!("{resource_type}/{resource_id}"))
}

/// Resolving the subnet identifiers a create names against the subnets the ACCOUNT has,
/// which is Amazon EC2's state and not this service's. Re-exported below, so a handler
/// reaches [`resolve_requested_subnets`] through `crate::state` the way it reaches
/// everything else here.
mod ec2_subnets;
pub use ec2_subnets::resolve_requested_subnets;

/// The backup slice: the records `CreateBackup` writes and `DescribeBackups` reads, and
/// what a backup keeps of the file system it was taken of. Re-exported, so a handler
/// reaches it through `crate::state` like everything else here.
mod backups;
pub use backups::*;

/// The volume slice: the volumes a file-system create provisions (an OpenZFS file
/// system's root volume) and `DescribeVolumes` reads. Re-exported, so a handler reaches it
/// through `crate::state` like everything else here.
mod volumes;
pub use volumes::*;

/// The administrative-action slice: the asynchronous work an `UpdateFileSystem` starts, and
/// the storage-capacity target rule that decides whether it may start. Re-exported, so a
/// handler reaches it through `crate::state` like everything else here.
mod administrative_actions;
pub use administrative_actions::*;

/// The data-repository-association slice: the links `CreateDataRepositoryAssociation` writes
/// and `DescribeDataRepositoryAssociations` reads. Re-exported, as above.
mod data_repository_associations;
pub use data_repository_associations::*;

/// Resolving the Amazon S3 bucket an association's `DataRepositoryPath` names against the
/// service that OWNS buckets, which is not this one. Re-exported, as above.
mod s3_data_repository;
pub use s3_data_repository::*;

/// What every FSx for NetApp ONTAP peering and policy resource of this store shares: its
/// lifecycle, its bounded create-token memory, the selection its describe operation reports
/// and the UUID ONTAP gives it. Re-exported, as above.
mod ontap_resources;
pub use ontap_resources::*;

/// The cluster-peer slice: the peer relationships `CreateClusterPeer` writes between an FSx
/// for NetApp ONTAP file system and another ONTAP cluster, and what `DescribeClusterPeers`
/// reads. Re-exported, as above.
mod cluster_peers;
pub use cluster_peers::*;

/// The ONTAP policy slice: the SnapMirror and snapshot policies the two create operations
/// write and their describe operations read. Re-exported, as above.
mod ontap_policies;
pub use ontap_policies::*;

/// Remember one `ClientRequestToken` replay in a bounded map, dropping the oldest once
/// [`MAX_IDEMPOTENCY_TOKENS`] are held.
///
/// Shared by every operation that remembers replays, so each one is bounded by the same
/// rule and the eviction is written once. `order` is the insertion order of `remembered`,
/// oldest first.
fn remember_token<V>(
    remembered: &mut HashMap<String, V>,
    order: &mut VecDeque<String>,
    token: String,
    replay: V,
) {
    if remembered.insert(token.clone(), replay).is_none() {
        order.push_back(token);
    }
    while order.len() > MAX_IDEMPOTENCY_TOKENS {
        if let Some(oldest) = order.pop_front() {
            remembered.remove(&oldest);
        }
    }
}

/// Forget one remembered `ClientRequestToken`, from both halves of a bounded map.
fn forget_token<V>(
    remembered: &mut HashMap<String, V>,
    order: &mut VecDeque<String>,
    token: &str,
) {
    remembered.remove(token);
    order.retain(|held| held != token);
}

/// One subnet of the account, as the service that OWNS subnets reports it: the identifier a
/// request names, the VPC it belongs to and the Availability Zone it sits in.
///
/// Every field is read off EC2's own record (see [`resolve_requested_subnets`]); nothing
/// here is derived from a Region or a request, so a placement this service reports is the
/// network the account really has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetRecord {
    /// The EC2 subnet identifier, as a request names it.
    pub subnet_id: String,
    /// The VPC this subnet belongs to.
    pub vpc_id: String,
    /// The Availability Zone this subnet sits in.
    pub availability_zone: String,
}

/// The resource types FSx owns, as the `<type>/<id>` half of a `ResourceARN`.
///
/// One per id shape the model declares (`FileSystemId` `fs-`, `BackupId` `backup-`,
/// `VolumeId` `fsvol-`, `SnapshotId` `(fs)?volsnap-`, `StorageVirtualMachineId` `svm-`,
/// `TaskId` `task-`, `AssociationId` `dra-`, `FileCacheId` `fc-`, `ClusterPeerId`
/// `clusterpeer-`, `SnapmirrorPolicyId` `snapmirrorpol-`, `SnapshotPolicyId`
/// `snapshotpol-`, `StorageVirtualMachinePeerId` `svmpeer-`, `SnapmirrorRelationshipId`
/// `snapmirror-`). An ARN naming a type NOT in this list is `NotServiceResourceError`; one
/// naming a type in it that holds no record is `ResourceNotFound`.
pub const FSX_RESOURCE_TYPES: &[&str] = &[
    "file-system",
    "backup",
    "volume",
    "snapshot",
    "storage-virtual-machine",
    "task",
    "association",
    "file-cache",
    CLUSTER_PEER_RESOURCE_TYPE,
    SNAPMIRROR_POLICY_RESOURCE_TYPE,
    SNAPSHOT_POLICY_RESOURCE_TYPE,
    "storage-virtual-machine-peer",
    "snapmirror-relationship",
];

/// The resource type this slice stores records for.
pub const FILE_SYSTEM_RESOURCE_TYPE: &str = "file-system";

/// The parts of an FSx `ResourceARN` this service resolves against its store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsxArn {
    pub partition: String,
    pub region: String,
    pub account_id: String,
    pub resource_type: String,
    pub resource_id: String,
}

impl FsxArn {
    /// Whether this ARN names a resource type FSx owns at all.
    pub fn names_fsx_resource_type(&self) -> bool {
        FSX_RESOURCE_TYPES.contains(&self.resource_type.as_str())
    }
}

/// Split an FSx ARN into its parts: `arn:<partition>:fsx:<region>:<account>:<type>/<id>`.
///
/// `None` when the string is not an `fsx` ARN carrying a `<type>/<id>` resource portion.
pub fn parse_fsx_arn(arn: &str) -> Option<FsxArn> {
    let fields: Vec<&str> = arn.splitn(6, ':').collect();
    let [prefix, partition, service, region, account_id, resource] = fields.as_slice() else {
        return None;
    };
    if *prefix != "arn" || *service != "fsx" {
        return None;
    }
    let (resource_type, resource_id) = resource.split_once('/')?;
    if resource_type.is_empty() || resource_id.is_empty() {
        return None;
    }
    Some(FsxArn {
        partition: (*partition).to_string(),
        region: (*region).to_string(),
        account_id: (*account_id).to_string(),
        resource_type: resource_type.to_string(),
        resource_id: resource_id.to_string(),
    })
}

/// Marks a `NextToken` as one this service minted, and for what.
///
/// A pagination token is opaque to a client, but a service still has to tell its own
/// token from an arbitrary string that happens to decode: without a marker, any decodable
/// string becomes a "cursor" and silently yields an empty page.
const PAGE_TOKEN_MARKER: &str = "fsx-page-v1";

/// Separator inside a page token, between marker, scope and cursor, and the byte that pads
/// the payload to an even length.
///
/// A control character, so it cannot occur in a cursor: the modelled `FileSystemId` is
/// `fs-` plus hex and the modelled `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$`
/// admits no control characters either.
const PAGE_TOKEN_SEPARATOR: char = '\u{1f}';

/// Encode a pagination cursor as the opaque `NextToken` the model declares.
///
/// The marker, the `scope` the token belongs to (which listing, over which resource) and
/// the cursor — the last key of the page just returned — are hex-encoded together. Hex
/// satisfies the modelled pattern
/// `^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$`: its alphabet is a
/// subset of that character class, and two hex characters per byte over an even-length
/// payload make the token's length a multiple of the pattern's four-character groups. A
/// client must not read the token; this service only decodes its own, for the scope it was
/// minted for.
pub fn encode_page_token(scope: &str, resume_after: &str) -> String {
    let mut payload =
        format!("{PAGE_TOKEN_MARKER}{PAGE_TOKEN_SEPARATOR}{scope}{PAGE_TOKEN_SEPARATOR}{resume_after}")
            .into_bytes();
    if payload.len() % 2 != 0 {
        payload.push(PAGE_TOKEN_SEPARATOR as u8);
    }
    hex::encode(payload)
}

/// Decode a `NextToken` minted by [`encode_page_token`] for `scope`.
///
/// `None` for a token this service did not mint, or minted for a different listing, which
/// a handler answers with its modelled bad-request error rather than an empty page.
pub fn decode_page_token(scope: &str, token: &str) -> Option<String> {
    let bytes = hex::decode(token).ok()?;
    let payload = String::from_utf8(bytes).ok()?;
    let mut fields = payload.splitn(3, PAGE_TOKEN_SEPARATOR);
    let (Some(PAGE_TOKEN_MARKER), Some(token_scope), Some(cursor)) =
        (fields.next(), fields.next(), fields.next())
    else {
        return None;
    };
    // The cursor is the last field, so the even-length padding (if any) is the only
    // separator that can trail it.
    (token_scope == scope).then(|| cursor.trim_end_matches(PAGE_TOKEN_SEPARATOR).to_string())
}

/// One page of `keys`, plus the `NextToken` for the page after it.
///
/// `resume_after` is the decoded cursor: the page starts at the first key strictly greater
/// than it. Keys must be sorted, which both callers' `BTreeMap` iteration guarantees.
pub fn paginate<K: Ord + AsRef<str> + Clone>(
    scope: &str,
    keys: &[K],
    resume_after: Option<&str>,
    page_size: usize,
) -> (Vec<K>, Option<String>) {
    let start = match resume_after {
        Some(cursor) => keys.partition_point(|key| key.as_ref() <= cursor),
        None => 0,
    };
    let rest = &keys[start.min(keys.len())..];
    let page: Vec<K> = rest.iter().take(page_size).cloned().collect();
    let next_token = match page.last() {
        Some(last) if rest.len() > page.len() => Some(encode_page_token(scope, last.as_ref())),
        _ => None,
    };
    (page, next_token)
}

/// The pagination scope of `DescribeFileSystems`.
const FILE_SYSTEMS_PAGE_SCOPE: &str = "file-systems";

/// The pagination scope of one `DescribeFileSystems` listing: the constant for an
/// unfiltered listing, and the selection itself for a `FileSystemIds`-filtered one.
///
/// A cursor only means something over the same ordered set of keys, so a token minted for
/// one selection is not accepted for another. `selected` must be sorted and deduplicated,
/// so the same selection in a different request order is the same scope.
pub fn file_systems_page_scope(selected: &[String]) -> String {
    if selected.is_empty() {
        return FILE_SYSTEMS_PAGE_SCOPE.to_string();
    }
    format!("{FILE_SYSTEMS_PAGE_SCOPE}/{}", selected.join(","))
}

/// The pagination scope of `ListTagsForResource` for one resource, so a token minted for
/// one resource's tags is not accepted for another's.
pub fn tags_page_scope(resource_id: &str) -> String {
    format!("tags/{resource_id}")
}

/// The pagination scope of one FILTERED listing (`DescribeBackups`, `DescribeVolumes`):
/// the listing's own name for an unrestricted page, and the selection itself otherwise.
///
/// A cursor only means something over the same ordered set of keys, so a token minted for
/// one selection is not accepted for another: the ids the request named and its `Filters`
/// both take part. They are caller-supplied text of unbounded shape, so the scope is their
/// DIGEST — a fixed-length, separator-free string — rather than the values themselves.
/// `selected` must be sorted and deduplicated, and `filters` normalised the same way, so
/// one selection written in a different order is the same scope.
///
/// Stated once here, so two listings cannot drift apart in how they scope a cursor.
pub fn filtered_page_scope(
    listing: &str,
    selected: &[String],
    filters: &[(String, Vec<String>)],
) -> String {
    if selected.is_empty() && filters.is_empty() {
        return listing.to_string();
    }
    let rendered = format!("{selected:?}|{filters:?}");
    format!(
        "{listing}/{}",
        hex::encode(Sha256::digest(rendered.as_bytes()))
    )
}

/// The `BadRequest$Message` for a `Filter` whose name a listing does not select by: the
/// name the request carried, and the names that listing accepts.
///
/// The accepted names are rendered from the listing's own slice rather than written out
/// again, so the sentence cannot drift from what the handler applies. Clamped to the
/// modelled `ErrorMessage` `length 1..256`, because the name comes from the request.
/// Stated once here, so two listings cannot word the same refusal differently.
pub fn unsupported_filter_name_message_for(
    operation: &str,
    supported: &[&str],
    filter_name: &str,
) -> String {
    within_error_message_length(format!(
        "The filter name '{filter_name}' is not supported by {operation}. The supported names \
         are {}.",
        supported.join(", ")
    ))
}

/// The page size a request asks for, clamped to what the modelled output list can hold.
pub fn page_size(max_results: Option<i32>, cap: usize) -> usize {
    match max_results.and_then(|requested| usize::try_from(requested).ok()) {
        Some(requested) if requested > 0 => requested.min(cap),
        _ => cap,
    }
}

/// The `FileSystemTypeVersion` a create call settles on.
///
/// AWS documents the default on `CreateFileSystem$FileSystemTypeVersion`, for FSx for
/// Lustre only: "Default value is 2.10, except for the following deployments: Default
/// value is 2.12 when DeploymentType is set to PERSISTENT_2 without a metadata
/// configuration mode. Default value is 2.15 when DeploymentType is set to PERSISTENT_2
/// with a metadata configuration mode." The modelled `FileSystem$FileSystemTypeVersion`
/// is likewise "The Lustre version of the Amazon FSx for Lustre file system", so no
/// default is invented for the other file-system types.
pub fn file_system_type_version(
    requested: Option<&str>,
    file_system_type: &str,
    lustre: Option<&CreateFileSystemLustreConfiguration>,
) -> Option<String> {
    if let Some(version) = requested {
        return Some(version.to_string());
    }
    if file_system_type != "LUSTRE" {
        return None;
    }
    let persistent_2 = lustre
        .and_then(|config| config.deployment_type.as_deref())
        .is_some_and(|deployment| deployment == "PERSISTENT_2");
    let metadata_configured = lustre.is_some_and(|config| config.metadata_configuration.is_some());
    Some(
        match (persistent_2, metadata_configured) {
            (true, true) => "2.15",
            (true, false) => "2.12",
            (false, _) => "2.10",
        }
        .to_string(),
    )
}

/// The DNS name of a file system: `<id>.fsx.<region>.amazonaws.com`.
///
/// Matches the modelled `DNSName` pattern `^((fs|fc)i?-[0-9a-f]{8,}\..{4,253})$`.
pub fn file_system_dns_name(file_system_id: &str, region: &str) -> String {
    format!("{file_system_id}.fsx.{region}.amazonaws.com")
}

/// The `DiskIopsConfiguration` a file system HAS, from the one its configuration carries
/// (if any), its own stored storage capacity and its high-availability pairs.
///
/// The member is a SERVER-SIDE default, never an echo of the request: AWS documents that
/// "by default, Amazon FSx automatically provisions 3 IOPS per GB of storage capacity", so
/// a file system of a type that models the member always has one. A configuration that
/// names no mode, or the `AUTOMATIC` mode, reports that rate applied to the file system's
/// current capacity and HA pairs; `USER_PROVISIONED` reports the IOPS the caller
/// provisioned, so an explicitly provisioned value is never overwritten.
pub fn provisioned_disk_iops(
    configured: Option<&DiskIopsConfiguration>,
    storage_capacity: Option<i32>,
    ha_pairs: Option<i32>,
) -> Option<DiskIopsConfiguration> {
    let mode = configured
        .and_then(|config| config.mode.clone())
        .unwrap_or_else(|| DISK_IOPS_MODE_AUTOMATIC.to_string());
    if mode == DISK_IOPS_MODE_USER_PROVISIONED {
        return Some(DiskIopsConfiguration {
            mode: Some(mode),
            iops: configured.and_then(|config| config.iops),
        });
    }
    // The automatic value is the documented rate applied to the file system's OWN stored
    // capacity, so a file system that states no storage capacity (a fully elastic one) has
    // no SSD IOPS count to report rather than a made-up one.
    let capacity = storage_capacity?;
    let pairs = i64::from(ha_pairs.unwrap_or(DEFAULT_HA_PAIRS).max(DEFAULT_HA_PAIRS));
    Some(DiskIopsConfiguration {
        mode: Some(mode),
        iops: Some(
            (i64::from(capacity).saturating_mul(AUTOMATIC_IOPS_PER_GIB))
                .saturating_mul(pairs)
                .min(MAX_DISK_IOPS),
        ),
    })
}

/// The stored Windows configuration for a `CreateFileSystemWindowsConfiguration`.
///
/// Members the create shape does not carry are server-generated endpoints and addresses
/// (`RemoteAdministrationEndpoint`, `PreferredFileServerIp`, `PreferredFileServerIpv6`),
/// which this emulator has no network plane to allocate, or belong to an operation still
/// at 501 (`Aliases`, served by `AssociateFileSystemAliases` /
/// `DescribeFileSystemAliases`); they stay absent rather than invented.
pub fn stored_windows_configuration(
    requested: &CreateFileSystemWindowsConfiguration,
    subnet_ids: &[String],
) -> WindowsFileSystemConfiguration {
    WindowsFileSystemConfiguration {
        active_directory_id: requested.active_directory_id.clone(),
        // The self-managed AD *attributes* shape deliberately has no `Password` member:
        // the secret the caller supplied is never stored or echoed.
        self_managed_active_directory_configuration: requested
            .self_managed_active_directory_configuration
            .as_ref()
            .map(|ad| SelfManagedActiveDirectoryAttributes {
                domain_name: Some(ad.domain_name.clone()),
                organizational_unit_distinguished_name: ad
                    .organizational_unit_distinguished_name
                    .clone(),
                file_system_administrators_group: ad.file_system_administrators_group.clone(),
                user_name: ad.user_name.clone(),
                dns_ips: Some(ad.dns_ips.clone()),
                domain_join_service_account_secret: ad.domain_join_service_account_secret.clone(),
            }),
        deployment_type: requested.deployment_type.clone(),
        remote_administration_endpoint: None,
        // AWS documents `PreferredSubnetID` as the subnet the file server lives in; a
        // single-subnet file system has only one, so it is that one.
        preferred_subnet_id: requested
            .preferred_subnet_id
            .clone()
            .or_else(|| subnet_ids.first().cloned()),
        preferred_file_server_ip: None,
        throughput_capacity: Some(requested.throughput_capacity),
        maintenance_operations_in_progress: None,
        weekly_maintenance_start_time: requested.weekly_maintenance_start_time.clone(),
        daily_automatic_backup_start_time: requested.daily_automatic_backup_start_time.clone(),
        automatic_backup_retention_days: requested.automatic_backup_retention_days,
        copy_tags_to_backups: requested.copy_tags_to_backups,
        aliases: None,
        audit_log_configuration: requested.audit_log_configuration.as_ref().map(|audit| {
            WindowsAuditLogConfiguration {
                file_access_audit_log_level: audit.file_access_audit_log_level.clone(),
                file_share_access_audit_log_level: audit.file_share_access_audit_log_level.clone(),
                audit_log_destination: audit.audit_log_destination.clone(),
            }
        }),
        disk_iops_configuration: requested.disk_iops_configuration.clone(),
        preferred_file_server_ipv6: None,
        fsrm_configuration: requested.fsrm_configuration.clone(),
    }
}

/// The stored Lustre configuration for a `CreateFileSystemLustreConfiguration`.
///
/// `MountName` is server-generated and `DataRepositoryConfiguration` belongs to the data
/// repository operations still at 501, so both stay absent.
pub fn stored_lustre_configuration(
    requested: &CreateFileSystemLustreConfiguration,
) -> LustreFileSystemConfiguration {
    LustreFileSystemConfiguration {
        weekly_maintenance_start_time: requested.weekly_maintenance_start_time.clone(),
        data_repository_configuration: None::<DataRepositoryConfiguration>,
        deployment_type: requested.deployment_type.clone(),
        per_unit_storage_throughput: requested.per_unit_storage_throughput,
        mount_name: None,
        daily_automatic_backup_start_time: requested.daily_automatic_backup_start_time.clone(),
        automatic_backup_retention_days: requested.automatic_backup_retention_days,
        copy_tags_to_backups: requested.copy_tags_to_backups,
        drive_cache_type: requested.drive_cache_type.clone(),
        data_compression_type: requested.data_compression_type.clone(),
        log_configuration: requested
            .log_configuration
            .as_ref()
            .map(|log| LustreLogConfiguration {
                level: log.level.clone(),
                destination: log.destination.clone(),
            }),
        root_squash_configuration: requested.root_squash_configuration.clone(),
        metadata_configuration: requested.metadata_configuration.as_ref().map(|metadata| {
            FileSystemLustreMetadataConfiguration {
                iops: metadata.iops,
                mode: metadata.mode.clone(),
            }
        }),
        efa_enabled: requested.efa_enabled,
        throughput_capacity: requested.throughput_capacity,
        data_read_cache_configuration: requested.data_read_cache_configuration.clone(),
    }
}

/// The stored ONTAP configuration for a `CreateFileSystemOntapConfiguration`.
///
/// `Endpoints` are server-allocated DNS names and addresses this emulator has no network
/// plane for, and `FsxAdminPassword` is a secret the service never echoes; both stay
/// absent.
pub fn stored_ontap_configuration(
    requested: &CreateFileSystemOntapConfiguration,
    subnet_ids: &[String],
) -> OntapFileSystemConfiguration {
    OntapFileSystemConfiguration {
        automatic_backup_retention_days: requested.automatic_backup_retention_days,
        daily_automatic_backup_start_time: requested.daily_automatic_backup_start_time.clone(),
        deployment_type: Some(requested.deployment_type.clone()),
        endpoint_ip_address_range: requested.endpoint_ip_address_range.clone(),
        endpoints: None,
        disk_iops_configuration: requested.disk_iops_configuration.clone(),
        preferred_subnet_id: requested
            .preferred_subnet_id
            .clone()
            .or_else(|| subnet_ids.first().cloned()),
        route_table_ids: requested.route_table_ids.clone(),
        throughput_capacity: requested.throughput_capacity,
        weekly_maintenance_start_time: requested.weekly_maintenance_start_time.clone(),
        fsx_admin_password: None,
        ha_pairs: requested.ha_pairs,
        throughput_capacity_per_ha_pair: requested.throughput_capacity_per_ha_pair,
        endpoint_ipv6_address_range: requested.endpoint_ipv6_address_range.clone(),
    }
}

/// The stored OpenZFS configuration for a `CreateFileSystemOpenZFSConfiguration`.
///
/// `RootVolumeId` belongs to the volume operations still at 501, and `EndpointIpAddress` /
/// `EndpointIpv6Address` are server-allocated addresses; all stay absent.
pub fn stored_open_zfs_configuration(
    requested: &CreateFileSystemOpenZFSConfiguration,
    subnet_ids: &[String],
) -> OpenZFSFileSystemConfiguration {
    OpenZFSFileSystemConfiguration {
        automatic_backup_retention_days: requested.automatic_backup_retention_days,
        copy_tags_to_backups: requested.copy_tags_to_backups,
        copy_tags_to_volumes: requested.copy_tags_to_volumes,
        daily_automatic_backup_start_time: requested.daily_automatic_backup_start_time.clone(),
        deployment_type: Some(requested.deployment_type.clone()),
        throughput_capacity: Some(requested.throughput_capacity),
        weekly_maintenance_start_time: requested.weekly_maintenance_start_time.clone(),
        disk_iops_configuration: requested.disk_iops_configuration.clone(),
        root_volume_id: None,
        preferred_subnet_id: requested
            .preferred_subnet_id
            .clone()
            .or_else(|| subnet_ids.first().cloned()),
        endpoint_ip_address_range: requested.endpoint_ip_address_range.clone(),
        endpoint_ipv6_address_range: requested.endpoint_ipv6_address_range.clone(),
        route_table_ids: requested.route_table_ids.clone(),
        endpoint_ip_address: None,
        endpoint_ipv6_address: None,
        read_cache_configuration: requested.read_cache_configuration.clone(),
    }
}

/// Merge the route-table changes an update asks for into a stored list.
///
/// Refuses the merge that would take the list past [`MAX_ROUTE_TABLE_IDS`] rather than
/// growing it without end, and dedups through a `BTreeSet`, so the cost is logarithmic
/// per added id instead of a linear scan of a list the caller can grow.
pub fn merged_route_table_ids(
    stored: Option<&Vec<String>>,
    add: Option<&Vec<String>>,
    remove: Option<&Vec<String>>,
) -> Result<Option<Vec<String>>, RouteTableIdLimitReached> {
    if add.is_none() && remove.is_none() {
        return Ok(stored.cloned());
    }
    let mut ids: BTreeSet<String> = stored.into_iter().flatten().cloned().collect();
    for id in add.into_iter().flatten() {
        ids.insert(id.clone());
    }
    for id in remove.into_iter().flatten() {
        ids.remove(id);
    }
    if ids.len() > MAX_ROUTE_TABLE_IDS {
        return Err(RouteTableIdLimitReached);
    }
    Ok(Some(ids.into_iter().collect()))
}

/// Whether the route-table merge an update asks for fits within
/// [`MAX_ROUTE_TABLE_IDS`], without changing anything.
///
/// A handler checks this for every per-type configuration BEFORE it mutates the record,
/// so a refused update leaves the record exactly as it was.
pub fn route_table_merge_fits(
    stored: Option<&Vec<String>>,
    add: Option<&Vec<String>>,
    remove: Option<&Vec<String>>,
) -> bool {
    merged_route_table_ids(stored, add, remove).is_ok()
}

/// Apply an `UpdateFileSystemWindowsConfiguration` to the stored Windows configuration.
///
/// Only the members the request carries change: an absent member means "leave as stored",
/// which is what makes a following `DescribeFileSystems` show the update and nothing else.
pub fn apply_windows_update(
    stored: &mut WindowsFileSystemConfiguration,
    update: &UpdateFileSystemWindowsConfiguration,
) {
    if update.weekly_maintenance_start_time.is_some() {
        stored.weekly_maintenance_start_time = update.weekly_maintenance_start_time.clone();
    }
    if update.daily_automatic_backup_start_time.is_some() {
        stored.daily_automatic_backup_start_time = update.daily_automatic_backup_start_time.clone();
    }
    if update.automatic_backup_retention_days.is_some() {
        stored.automatic_backup_retention_days = update.automatic_backup_retention_days;
    }
    if update.throughput_capacity.is_some() {
        stored.throughput_capacity = update.throughput_capacity;
    }
    if let Some(ad) = update.self_managed_active_directory_configuration.as_ref() {
        let attributes = stored
            .self_managed_active_directory_configuration
            .get_or_insert_with(SelfManagedActiveDirectoryAttributes::default);
        if ad.user_name.is_some() {
            attributes.user_name = ad.user_name.clone();
        }
        if ad.dns_ips.is_some() {
            attributes.dns_ips = ad.dns_ips.clone();
        }
        if ad.domain_name.is_some() {
            attributes.domain_name = ad.domain_name.clone();
        }
        if ad.organizational_unit_distinguished_name.is_some() {
            attributes.organizational_unit_distinguished_name =
                ad.organizational_unit_distinguished_name.clone();
        }
        if ad.file_system_administrators_group.is_some() {
            attributes.file_system_administrators_group =
                ad.file_system_administrators_group.clone();
        }
        if ad.domain_join_service_account_secret.is_some() {
            attributes.domain_join_service_account_secret =
                ad.domain_join_service_account_secret.clone();
        }
    }
    if let Some(audit) = update.audit_log_configuration.as_ref() {
        stored.audit_log_configuration = Some(WindowsAuditLogConfiguration {
            file_access_audit_log_level: audit.file_access_audit_log_level.clone(),
            file_share_access_audit_log_level: audit.file_share_access_audit_log_level.clone(),
            audit_log_destination: audit.audit_log_destination.clone(),
        });
    }
    if update.disk_iops_configuration.is_some() {
        stored.disk_iops_configuration = update.disk_iops_configuration.clone();
    }
    if update.fsrm_configuration.is_some() {
        stored.fsrm_configuration = update.fsrm_configuration.clone();
    }
}

/// Apply an `UpdateFileSystemLustreConfiguration` to the stored Lustre configuration.
pub fn apply_lustre_update(
    stored: &mut LustreFileSystemConfiguration,
    update: &UpdateFileSystemLustreConfiguration,
) {
    if update.weekly_maintenance_start_time.is_some() {
        stored.weekly_maintenance_start_time = update.weekly_maintenance_start_time.clone();
    }
    if update.daily_automatic_backup_start_time.is_some() {
        stored.daily_automatic_backup_start_time = update.daily_automatic_backup_start_time.clone();
    }
    if update.automatic_backup_retention_days.is_some() {
        stored.automatic_backup_retention_days = update.automatic_backup_retention_days;
    }
    if update.data_compression_type.is_some() {
        stored.data_compression_type = update.data_compression_type.clone();
    }
    if let Some(log) = update.log_configuration.as_ref() {
        stored.log_configuration = Some(LustreLogConfiguration {
            level: log.level.clone(),
            destination: log.destination.clone(),
        });
    }
    if update.root_squash_configuration.is_some() {
        stored.root_squash_configuration = update.root_squash_configuration.clone();
    }
    if update.per_unit_storage_throughput.is_some() {
        stored.per_unit_storage_throughput = update.per_unit_storage_throughput;
    }
    if let Some(metadata) = update.metadata_configuration.as_ref() {
        let target = stored
            .metadata_configuration
            .get_or_insert_with(FileSystemLustreMetadataConfiguration::default);
        if metadata.iops.is_some() {
            target.iops = metadata.iops;
        }
        if let Some(mode) = metadata.mode.as_ref() {
            target.mode = mode.clone();
        }
    }
    if update.throughput_capacity.is_some() {
        stored.throughput_capacity = update.throughput_capacity;
    }
    if update.data_read_cache_configuration.is_some() {
        stored.data_read_cache_configuration = update.data_read_cache_configuration.clone();
    }
    // `AutoImportPolicy` lives on `DataRepositoryConfiguration`, which belongs to the
    // data repository operations still at 501.
}

/// Apply an `UpdateFileSystemOntapConfiguration` to the stored ONTAP configuration.
///
/// The route-table merge is computed and bounds-checked FIRST, so a refused update leaves
/// the stored configuration exactly as it was.
pub fn apply_ontap_update(
    stored: &mut OntapFileSystemConfiguration,
    update: &UpdateFileSystemOntapConfiguration,
) -> Result<(), RouteTableIdLimitReached> {
    let route_table_ids = merged_route_table_ids(
        stored.route_table_ids.as_ref(),
        update.add_route_table_ids.as_ref(),
        update.remove_route_table_ids.as_ref(),
    )?;
    if update.automatic_backup_retention_days.is_some() {
        stored.automatic_backup_retention_days = update.automatic_backup_retention_days;
    }
    if update.daily_automatic_backup_start_time.is_some() {
        stored.daily_automatic_backup_start_time = update.daily_automatic_backup_start_time.clone();
    }
    if update.weekly_maintenance_start_time.is_some() {
        stored.weekly_maintenance_start_time = update.weekly_maintenance_start_time.clone();
    }
    if update.disk_iops_configuration.is_some() {
        stored.disk_iops_configuration = update.disk_iops_configuration.clone();
    }
    if update.throughput_capacity.is_some() {
        stored.throughput_capacity = update.throughput_capacity;
    }
    if update.throughput_capacity_per_ha_pair.is_some() {
        stored.throughput_capacity_per_ha_pair = update.throughput_capacity_per_ha_pair;
    }
    if update.ha_pairs.is_some() {
        stored.ha_pairs = update.ha_pairs;
    }
    if update.endpoint_ipv6_address_range.is_some() {
        stored.endpoint_ipv6_address_range = update.endpoint_ipv6_address_range.clone();
    }
    stored.route_table_ids = route_table_ids;
    // `FsxAdminPassword` is a secret: an update may set it, but it is never stored or
    // echoed back.
    Ok(())
}

/// Apply an `UpdateFileSystemOpenZFSConfiguration` to the stored OpenZFS configuration.
///
/// The route-table merge is computed and bounds-checked FIRST, so a refused update leaves
/// the stored configuration exactly as it was.
pub fn apply_open_zfs_update(
    stored: &mut OpenZFSFileSystemConfiguration,
    update: &UpdateFileSystemOpenZFSConfiguration,
) -> Result<(), RouteTableIdLimitReached> {
    let route_table_ids = merged_route_table_ids(
        stored.route_table_ids.as_ref(),
        update.add_route_table_ids.as_ref(),
        update.remove_route_table_ids.as_ref(),
    )?;
    if update.automatic_backup_retention_days.is_some() {
        stored.automatic_backup_retention_days = update.automatic_backup_retention_days;
    }
    if update.copy_tags_to_backups.is_some() {
        stored.copy_tags_to_backups = update.copy_tags_to_backups;
    }
    if update.copy_tags_to_volumes.is_some() {
        stored.copy_tags_to_volumes = update.copy_tags_to_volumes;
    }
    if update.daily_automatic_backup_start_time.is_some() {
        stored.daily_automatic_backup_start_time = update.daily_automatic_backup_start_time.clone();
    }
    if update.throughput_capacity.is_some() {
        stored.throughput_capacity = update.throughput_capacity;
    }
    if update.weekly_maintenance_start_time.is_some() {
        stored.weekly_maintenance_start_time = update.weekly_maintenance_start_time.clone();
    }
    if update.disk_iops_configuration.is_some() {
        stored.disk_iops_configuration = update.disk_iops_configuration.clone();
    }
    if update.read_cache_configuration.is_some() {
        stored.read_cache_configuration = update.read_cache_configuration.clone();
    }
    if update.endpoint_ipv6_address_range.is_some() {
        stored.endpoint_ipv6_address_range = update.endpoint_ipv6_address_range.clone();
    }
    stored.route_table_ids = route_table_ids;
    Ok(())
}
