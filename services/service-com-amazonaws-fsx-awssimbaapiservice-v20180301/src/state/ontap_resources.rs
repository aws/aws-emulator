// SPDX-License-Identifier: CC0-1.0

//! The pieces every FSx for NetApp ONTAP peering and policy resource of this store shares:
//! the lifecycle its records move through, the bounded `ClientRequestToken` memory its
//! creates replay from, the selection one of its describe operations reports, and the UUID
//! ONTAP gives such a resource.
//!
//! The cluster peers ([`super::cluster_peers`]) and the SnapMirror and snapshot policies
//! ([`super::ontap_policies`]) are modelled alike down to the member: each is created on a
//! file system or an SVM, each carries a lifecycle of the same four states, a UUID, a
//! `LifecycleTransitionReason` and tags, and each is read back by a describe operation whose
//! ids and `Filters` select TOGETHER. So the rule each of them follows is stated once here
//! rather than once per family.

use std::collections::{BTreeMap, HashMap, VecDeque};

use sha2::{Digest, Sha256};

/// The lifecycle states of a cluster peer, a SnapMirror policy, a snapshot policy, an SVM
/// peer and a SnapMirror relationship. Every one of those modelled enums
/// (`smithy_query(query="shape", shape="ClusterPeerLifecycle")` and its siblings) has the
/// same four members: CREATING | CREATED | DELETING | FAILED.
///
/// "`CREATING` – Amazon FSx is creating the cluster peer": a create returns while the
/// resource is still `CREATING`, which is what the clock then owes its way out of.
pub const ONTAP_LIFECYCLE_CREATING: &str = "CREATING";
/// See [`ONTAP_LIFECYCLE_CREATING`]. "`CREATED` – The cluster peer is fully available for
/// use."
pub const ONTAP_LIFECYCLE_CREATED: &str = "CREATED";
/// See [`ONTAP_LIFECYCLE_CREATING`]. "`DELETING` – Amazon FSx is deleting the cluster peer."
pub const ONTAP_LIFECYCLE_DELETING: &str = "DELETING";

/// The `file-system-id` filter name, which every describe operation of these families
/// selects by (`smithy_query(query="shape", shape="ClusterPeerFilterName")`: file-system-id;
/// `SnapmirrorPolicyFilterName` and `SnapshotPolicyFilterName`: file-system-id |
/// storage-virtual-machine-id).
pub const ONTAP_FILTER_FILE_SYSTEM_ID: &str = "file-system-id";
/// See [`ONTAP_FILTER_FILE_SYSTEM_ID`].
pub const ONTAP_FILTER_STORAGE_VIRTUAL_MACHINE_ID: &str = "storage-virtual-machine-id";

/// What one `ClientRequestToken` of a create replays: the resource it created, and a digest
/// of the parameters it was created with.
///
/// A digest rather than the parameters themselves, so one remembered token costs a bounded
/// amount of memory however large the request that minted it was. A replay whose parameters
/// differ is `IncompatibleParameterError`, which every one of these creates models.
#[derive(Debug, Clone)]
pub struct CreatedByToken {
    /// The id of the resource the first call with this token created.
    pub resource_id: String,
    /// A FIXED-SIZE digest of that call's parameters.
    pub fingerprint: String,
}

/// A bounded `ClientRequestToken` -> replay memory for ONE create operation.
///
/// A token map is the one piece of state a caller grows without creating a resource, so it
/// is bounded and evicted oldest-first by the same rule as every other token map here
/// ([`super::MAX_IDEMPOTENCY_TOKENS`], applied by [`super::remember_token`]). One of these
/// per create operation, because a token is scoped to the operation that minted it: a
/// `CreateClusterPeer` replay cannot be served as a `CreateSnapmirrorPolicy` replay.
#[derive(Debug)]
pub struct IdempotentCreates<V> {
    remembered: HashMap<String, V>,
    /// Insertion order of `remembered`, oldest first — what lets the map be bounded.
    order: VecDeque<String>,
}

impl<V> Default for IdempotentCreates<V> {
    fn default() -> Self {
        Self {
            remembered: HashMap::new(),
            order: VecDeque::new(),
        }
    }
}

impl<V> IdempotentCreates<V> {
    /// What `token` replays, if this operation remembers it.
    pub fn get(&self, token: &str) -> Option<&V> {
        self.remembered.get(token)
    }

    /// Remember what `token` replays, dropping the oldest once
    /// [`super::MAX_IDEMPOTENCY_TOKENS`] are held.
    pub fn remember(&mut self, token: String, replay: V) {
        super::remember_token(&mut self.remembered, &mut self.order, token, replay);
    }

    /// Forget one token: the resource it replayed is gone, so it replays nothing.
    pub fn forget(&mut self, token: &str) {
        super::forget_token(&mut self.remembered, &mut self.order, token);
    }
}

/// One stored record of these families, as the selection of a describe operation reads it.
///
/// `filtered_value` answers the value the named `Filter` selects on, read off the stored
/// record, and `None` for a filter name this record carries no value for — which selects
/// nothing rather than everything.
pub trait DescribedRecord {
    /// The stored value `filter_name` selects on.
    fn filtered_value(&self, filter_name: &str) -> Option<&str>;
}

/// The ids one describe operation of these families reports, in stored order.
///
/// The rule every one of them documents on its own ids member: "If you also specify
/// `Filters`, only \<resources\> that match both the IDs and the filters are returned. If any
/// IDs aren't found, or if a filter excludes any of them, a \<Resource\>NotFound error
/// occurs." So ids and filters select TOGETHER — unlike `DescribeBackups`, whose ids
/// "override any filters" — and a named id that is missing or excluded is a refusal rather
/// than a shorter page. `Err` carries that id, so the caller's error can name it.
///
/// A request naming no id selects every stored record the filters admit, where a filter
/// VALUE is a selector and not an assertion that what it names exists: one matching no
/// record simply leaves the page empty.
pub fn described_keys<R: DescribedRecord>(
    records: &BTreeMap<String, R>,
    requested: &[String],
    filters: &[(String, Vec<String>)],
) -> Result<Vec<String>, String> {
    if requested.is_empty() {
        return Ok(records
            .iter()
            .filter(|(_, record)| matches_filters(*record, filters))
            .map(|(id, _)| id.clone())
            .collect());
    }
    for id in requested {
        match records.get(id) {
            Some(record) if matches_filters(record, filters) => {}
            _ => return Err(id.clone()),
        }
    }
    // Stored order, deduplicated: the page is walked by a cursor over the same ordered set
    // of keys the store holds.
    Ok(records
        .keys()
        .filter(|id| requested.contains(id))
        .cloned()
        .collect())
}

/// Whether one stored record satisfies every filter: a value of each, which is how AWS
/// documents a `Filter` — results that meet all applied filter requirements, over "the
/// values for any of the applied filters".
fn matches_filters<R: DescribedRecord>(record: &R, filters: &[(String, Vec<String>)]) -> bool {
    filters.iter().all(|(name, values)| {
        record
            .filtered_value(name)
            .is_some_and(|stored| values.iter().any(|value| value == stored))
    })
}

/// A request's `Filters` as `(name, values)` pairs, in a normal form: each name's values
/// sorted and deduplicated, and the pairs sorted by name.
///
/// So the same selection written in a different order is the same selection — which is what
/// lets the page scope be built from it — and a filter that names nothing carries the empty
/// name, which no supported name matches. The caller maps its own modelled filter shape
/// (`ClusterPeerFilter`, `SnapmirrorPolicyFilter`, `SnapshotPolicyFilter` — three shapes with
/// the same two members) onto these pairs, so the normal form is written once.
pub fn normalised_filters<I>(filters: I) -> Vec<(String, Vec<String>)>
where
    I: IntoIterator<Item = (Option<String>, Option<Vec<String>>)>,
{
    let mut normalised: Vec<(String, Vec<String>)> = filters
        .into_iter()
        .map(|(name, values)| {
            let mut values = values.unwrap_or_default();
            values.sort();
            values.dedup();
            (name.unwrap_or_default(), values)
        })
        .collect();
    normalised.sort();
    normalised
}

/// The UUID ONTAP gives one of these resources, as the modelled `UUID` string
/// (`length 0..36`) carries it.
///
/// Derived from the id this store minted for the resource, through the one hyphenated-UUID
/// layout this tree has ([`service_common::uuid_from_random_bytes`]): unique per resource,
/// stable across every read of it, and taken from nothing in the request. A caller's value
/// never reaches it, so no request can choose another resource's UUID.
pub fn ontap_uuid(resource_id: &str) -> String {
    let digest = Sha256::digest(resource_id.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    ::service_common::uuid_from_random_bytes(bytes)
}

/// The `<Resource>NotFound$Message` for an id one of these stores holds nothing for.
///
/// The same rule as [`super::file_system_not_found_message`]: FSx builds this `Message` from
/// the identifier the caller sent — it is what tells a client WHICH of the ids it named was
/// missing — so it is templated from the requested id and never hand-written at a call site,
/// and clamped to the modelled `ErrorMessage` `length 1..256`. `resource` is the resource's
/// own name as AWS writes it in prose ("Cluster peer", "SnapMirror policy"); stated once
/// here, so no two families word the same refusal differently.
pub fn ontap_not_found_message(resource: &str, resource_id: &str) -> String {
    super::within_error_message_length(format!("{resource} '{resource_id}' does not exist."))
}

/// The `BadRequest$Message` for an update of one of these resources while the service is
/// deleting it: it is on its way out, so there is nothing left to change about it. Worded
/// once here, for the same reason as [`ontap_not_found_message`].
pub fn ontap_is_deleting_message(resource: &str, resource_id: &str) -> String {
    super::within_error_message_length(format!(
        "{resource} '{resource_id}' is being deleted and can't be updated."
    ))
}

/// The `StorageVirtualMachineNotFound$Message` for an SVM id this store holds nothing for,
/// from the shared wording in [`ontap_not_found_message`].
///
/// Every operation of these families that is addressed by a `StorageVirtualMachineId` words
/// the refusal this way, so the sentence lives here once.
pub fn storage_virtual_machine_not_found_message(storage_virtual_machine_id: &str) -> String {
    ontap_not_found_message(STORAGE_VIRTUAL_MACHINE, storage_virtual_machine_id)
}

/// The name AWS writes a storage virtual machine with in prose, which the message above is
/// built from.
const STORAGE_VIRTUAL_MACHINE: &str = "Storage virtual machine";
