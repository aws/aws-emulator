// SPDX-License-Identifier: CC0-1.0

//! The cluster-peer slice of this service's state: the peer relationships
//! `CreateClusterPeer` writes between an FSx for NetApp ONTAP file system and another ONTAP
//! cluster, and what `DescribeClusterPeers` reads, `UpdateClusterPeer` changes and
//! `DeleteClusterPeer` removes.
//!
//! A cluster peer is a resource of its own — "the system-generated, unique ID of the cluster
//! peer" (`ClusterPeer$ClusterPeerId`) — so it is minted and stored here rather than folded
//! into the file system's record, and it belongs to the file system it was created on: the
//! records go when the file system does.
//!
//! The peer CLUSTER is not a resource of this emulator at all: its intercluster addresses,
//! its name and the Secrets Manager secret holding the peering passphrase are values the
//! caller states about a cluster somewhere else, and this operation models no error for any
//! of them, so they are stored as given and never checked against a cluster nothing here
//! holds. What IS checked is the half of the relationship this account owns: the file system
//! the peer is created on.

use std::collections::BTreeMap;
use std::time::Instant;

use crate::ops::types::ClusterPeer;

use super::{
    fsx_arn, tag_list, within_error_message_length,
    ComAmazonawsFsxAwssimbaapiserviceV20180301State, DescribedRecord, PendingTransition,
    TransitionTarget, ONTAP_FILTER_FILE_SYSTEM_ID, ONTAP_LIFECYCLE_CREATED,
    ONTAP_LIFECYCLE_DELETING,
};

/// The resource type a cluster peer's `ResourceARN` names, as the `<type>/<id>` half of an
/// FSx ARN (one of [`super::FSX_RESOURCE_TYPES`]).
///
/// The resource name of the modelled shape, hyphenated, which is the form every other FSx
/// resource type takes in an ARN of this service (`file-system`,
/// `storage-virtual-machine`).
pub const CLUSTER_PEER_RESOURCE_TYPE: &str = "cluster-peer";

/// The `AllowedSvmMode` value that preauthorizes NAMED SVMs for SVM peering, from the
/// modelled enum (`smithy_query(query="shape", shape="AllowedSvmMode")`: ALL | NONE |
/// SPECIFIED).
///
/// The only mode `AllowedSvmIds` may be given with: "You can specify `AllowedSvmIds` only
/// when `AllowedSvmMode` is `SPECIFIED`" (`CreateClusterPeerRequest$AllowedSvmIds`).
pub const ALLOWED_SVM_MODE_SPECIFIED: &str = "SPECIFIED";

/// The modelled `ClusterPeerFilterName`s `DescribeClusterPeers` selects peers by
/// (`smithy_query(query="shape", shape="ClusterPeerFilterName")`: file-system-id), which its
/// `Filters` member documents as "The supported name is file-system-id".
pub const CLUSTER_PEER_FILTER_NAMES: &[&str] = &[ONTAP_FILTER_FILE_SYSTEM_ID];

/// The most cluster peers this store holds for ONE file system.
///
/// CHOSEN, not published by AWS: the model states no quota for cluster peers, so the store —
/// which a caller grows one entry per `CreateClusterPeer` — is bounded at the most ONE page
/// of `DescribeClusterPeers` can report (the modelled `ClusterPeers` list's own
/// `length max 50`, which is also the most `ClusterPeerIds` can name in one request).
///
/// A create past it is refused with `BadRequest` ("a generic error indicating a failure with
/// a client request") rather than the modelled `ServiceLimitExceeded`: that error's `Limit`
/// member is REQUIRED and the modelled `ServiceLimit` enum
/// (`smithy_query(query="shape", shape="ServiceLimit")`) has no value naming cluster peers,
/// so there is no limit value to report.
pub const MAX_CLUSTER_PEERS_PER_FILE_SYSTEM: usize = 50;

/// The largest page `DescribeClusterPeers` returns, from the modelled `ClusterPeers` list's
/// own `length max 50` (`smithy_query(query="operation", operation="DescribeClusterPeers")`).
pub const MAX_CLUSTER_PEERS_PER_PAGE: usize = 50;

/// One stored cluster peer. Mirrors the modelled `ClusterPeer` shape so
/// [`ClusterPeerRecord::to_cluster_peer`] is a projection and never a second source of
/// truth.
#[derive(Debug, Clone)]
pub struct ClusterPeerRecord {
    pub cluster_peer_id: String,
    /// The file system the peer was created on; the record is removed with it.
    pub file_system_id: String,
    pub lifecycle: String,
    pub peer_addresses: Vec<String>,
    pub peer_cluster_name: Option<String>,
    pub passphrase_secret_arn: String,
    pub allowed_svm_mode: String,
    pub allowed_svm_ids: Vec<String>,
    /// The UUID this store derived from [`ClusterPeerRecord::cluster_peer_id`]
    /// ([`super::ontap_uuid`]), so every read of the peer reports the same one.
    pub uuid: String,
    pub resource_arn: String,
    pub tags: BTreeMap<String, String>,
    pub pending: Option<PendingTransition>,
}

impl ClusterPeerRecord {
    /// This record as the modelled `ClusterPeer` shape. Every member comes from the stored
    /// record, so a response can never disagree with what a later read sees.
    pub fn to_cluster_peer(&self) -> ClusterPeer {
        ClusterPeer {
            resource_arn: Some(self.resource_arn.clone()),
            cluster_peer_id: Some(self.cluster_peer_id.clone()),
            file_system_id: Some(self.file_system_id.clone()),
            lifecycle: Some(self.lifecycle.clone()),
            // Nothing failed: a peer this slice stores was accepted, and the one state that
            // would fail it — a file system the account does not have — is refused on the
            // create instead of stored as a failed peer.
            lifecycle_transition_reason: None,
            tags: tag_list(&self.tags),
            peer_addresses: Some(self.peer_addresses.clone()),
            peer_cluster_name: self.peer_cluster_name.clone(),
            passphrase_secret_arn: Some(self.passphrase_secret_arn.clone()),
            allowed_svm_mode: Some(self.allowed_svm_mode.clone()),
            // Only a `SPECIFIED` peer preauthorizes NAMED SVMs, so any other mode holds no
            // ids to report.
            allowed_svm_ids: (!self.allowed_svm_ids.is_empty())
                .then(|| self.allowed_svm_ids.clone()),
            // "The status of the connection to the peer cluster" and "the authentication
            // status of the cluster peer relationship" are what ONTAP reports of the peer
            // CLUSTER, which is not a resource of this emulator: no cluster answers for
            // this relationship, so there is no connection state to report, and a value
            // here would describe a handshake nothing performed.
            status: None,
            authentication_status: None,
            uuid: Some(self.uuid.clone()),
        }
    }
}

impl DescribedRecord for ClusterPeerRecord {
    fn filtered_value(&self, filter_name: &str) -> Option<&str> {
        match filter_name {
            ONTAP_FILTER_FILE_SYSTEM_ID => Some(self.file_system_id.as_str()),
            _ => None,
        }
    }
}

impl ComAmazonawsFsxAwssimbaapiserviceV20180301State {
    /// Apply every cluster-peer lifecycle transition the clock has already made due. Called
    /// from [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::settle_due_transitions`],
    /// which every handler runs.
    pub(super) fn settle_due_cluster_peer_transitions(&mut self, now: Instant) {
        let mut removed: Vec<String> = Vec::new();
        for (id, record) in &mut self.cluster_peers {
            let Some(pending) = record.pending else {
                continue;
            };
            if pending.due > now {
                continue;
            }
            match pending.target {
                // The terminal ready state of this family: "CREATED – The cluster peer is
                // fully available for use."
                TransitionTarget::Available => {
                    record.lifecycle = ONTAP_LIFECYCLE_CREATED.to_string();
                    record.pending = None;
                }
                TransitionTarget::Removed => removed.push(id.clone()),
            }
        }
        for id in removed {
            self.cluster_peers.remove(&id);
        }
    }

    /// Store a newly created cluster peer, refusing once its file system holds
    /// [`MAX_CLUSTER_PEERS_PER_FILE_SYSTEM`].
    ///
    /// The ONE write path for a new record, so the bound cannot be bypassed. Replacing a
    /// record that is already stored is not growth, so it is always allowed.
    pub fn insert_cluster_peer(
        &mut self,
        record: ClusterPeerRecord,
    ) -> Result<(), ClusterPeerLimitReached> {
        if self.cluster_peers.contains_key(&record.cluster_peer_id) {
            self.cluster_peers
                .insert(record.cluster_peer_id.clone(), record);
            return Ok(());
        }
        if self.cluster_peer_count_of_file_system(&record.file_system_id)
            >= MAX_CLUSTER_PEERS_PER_FILE_SYSTEM
        {
            return Err(ClusterPeerLimitReached);
        }
        self.cluster_peers
            .insert(record.cluster_peer_id.clone(), record);
        Ok(())
    }

    /// How many stored cluster peers belong to `file_system_id`.
    pub fn cluster_peer_count_of_file_system(&self, file_system_id: &str) -> usize {
        self.cluster_peers
            .values()
            .filter(|record| record.file_system_id == file_system_id)
            .count()
    }

    /// Move one stored cluster peer into `DELETING` and let the clock remove it, answering
    /// the record as it now stands.
    ///
    /// `None` when the store holds no such peer, which the caller answers with the modelled
    /// `ClusterPeerNotFound`. A peer that is ALREADY deleting is answered as it stands
    /// rather than moved again, so a retried delete answers as the first one did (the
    /// operation is `@idempotent`).
    pub fn begin_cluster_peer_deletion(
        &mut self,
        cluster_peer_id: &str,
    ) -> Option<&ClusterPeerRecord> {
        let record = self.cluster_peers.get_mut(cluster_peer_id)?;
        if record.lifecycle != ONTAP_LIFECYCLE_DELETING {
            record.lifecycle = ONTAP_LIFECYCLE_DELETING.to_string();
            record.pending = Some(PendingTransition::due_now_plus_span(
                TransitionTarget::Removed,
            ));
        }
        Some(record)
    }

    /// Remove every cluster peer of `file_system_id`.
    ///
    /// A cluster peer belongs to the file system it was created on, so the records go when
    /// the file system does — which is also what keeps this store bounded by the live file
    /// systems.
    pub fn remove_cluster_peers_of_file_system(&mut self, file_system_id: &str) {
        self.cluster_peers
            .retain(|_, record| record.file_system_id != file_system_id);
    }

    /// Mint the next `ClusterPeerId`.
    ///
    /// Shaped to the modelled `ClusterPeerId` pattern `^(clusterpeer-[0-9a-f]{17})$`:
    /// `clusterpeer-` plus exactly 17 lowercase hex digits, the form AWS documents as "the
    /// system-generated, unique ID of the cluster peer".
    pub fn mint_cluster_peer_id(&mut self) -> String {
        self.next_cluster_peer_id_counter += 1;
        format!("clusterpeer-{:017x}", self.next_cluster_peer_id_counter)
    }
}

/// [`ComAmazonawsFsxAwssimbaapiserviceV20180301State::insert_cluster_peer`] refused the
/// record: the file system already holds [`MAX_CLUSTER_PEERS_PER_FILE_SYSTEM`] peers. See
/// that constant for which error the caller answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClusterPeerLimitReached;

/// The `ClusterPeerNotFound$Message` for a cluster-peer id this store holds nothing for,
/// from the shared wording in [`super::ontap_not_found_message`] under this resource's own
/// name.
pub fn cluster_peer_not_found_message(cluster_peer_id: &str) -> String {
    super::ontap_not_found_message(CLUSTER_PEER, cluster_peer_id)
}

/// The `BadRequest$Message` for an update of a cluster peer the service is deleting, from
/// the shared wording in [`super::ontap_is_deleting_message`].
pub fn cluster_peer_is_deleting_message(cluster_peer_id: &str) -> String {
    super::ontap_is_deleting_message(CLUSTER_PEER, cluster_peer_id)
}

/// This resource's name as AWS writes it in prose, which its messages are built from.
const CLUSTER_PEER: &str = "Cluster peer";

/// The `BadRequest$Message` for a create that would hold more cluster peers than
/// [`MAX_CLUSTER_PEERS_PER_FILE_SYSTEM`], worded once beside the bound itself.
pub fn cluster_peer_limit_message() -> String {
    within_error_message_length(format!(
        "This file system already has the maximum of {MAX_CLUSTER_PEERS_PER_FILE_SYSTEM} \
         cluster peers."
    ))
}

/// The `BadRequest$Message` for a `DescribeClusterPeers` `Filter` whose name this listing
/// does not select by, from the shared wording in
/// [`super::unsupported_filter_name_message_for`] with this listing's own accepted names.
pub fn unsupported_cluster_peer_filter_name_message(filter_name: &str) -> String {
    super::unsupported_filter_name_message_for(
        "DescribeClusterPeers",
        CLUSTER_PEER_FILTER_NAMES,
        filter_name,
    )
}

/// The pagination scope of one `DescribeClusterPeers` listing, from the shared
/// [`super::filtered_page_scope`] under this listing's own name.
pub fn cluster_peers_page_scope(selected: &[String], filters: &[(String, Vec<String>)]) -> String {
    super::filtered_page_scope("cluster-peers", selected, filters)
}

/// The ARN of a stored cluster peer, through the one canonical builder ([`super::fsx_arn`]).
pub fn cluster_peer_arn(region: &str, cluster_peer_id: &str) -> String {
    fsx_arn(region, CLUSTER_PEER_RESOURCE_TYPE, cluster_peer_id)
}
