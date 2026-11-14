// SPDX-License-Identifier: CC0-1.0

//! The resize a cluster's last shape change asked for, and how a read reports its
//! progress.
//!
//! `DescribeResize` "returns information about the LAST resize operation for the
//! specified cluster. If no resize operation has ever been initiated for the
//! specified cluster, a HTTP 404 error is returned. If a resize operation was
//! initiated and completed, the status of the resize remains as `SUCCEEDED` until
//! the next resize" (`smithy_query(query="doc", operation="DescribeResize")`). So
//! a cluster holds at most ONE such record — the last one — it is created by the
//! call that changes the cluster's shape, and it never leaves the cluster again:
//! what moves is only its status, which the clock owes.
//!
//! Which call creates one is the shape change itself: `ModifyCluster`'s own
//! documentation says that when "you decide to change your cluster's node type or
//! number of nodes … you can use `DescribeResize` to track the progress of the
//! resize request" (`smithy_query(query="doc", operation="ModifyCluster")`), so a
//! modify that changes the node type, the cluster type or the node count IS a
//! resize of the cluster and a modify that changes anything else is not.
//!
//! Every number a read reports is computed from this record and the clock —
//! nothing is drawn per run and nothing is invented about data that was moved,
//! because this control plane moves none: a resize reports WHICH shape the
//! cluster is moving to, how long the move has been running and how long is left,
//! and says nothing about table imports or megabytes.

use super::*;

/// `ResizeProgressMessage$Status` while the resize is still running.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="Status")` — "Valid Values: `NONE` | `IN_PROGRESS` | `FAILED` |
/// `SUCCEEDED` | `CANCELLING`".
pub const RESIZE_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";
/// `ResizeProgressMessage$Status` once the resize has finished, which it stays at
/// "until the next resize". See [`RESIZE_STATUS_IN_PROGRESS`].
pub const RESIZE_STATUS_SUCCEEDED: &str = "SUCCEEDED";
/// `ResizeProgressMessage$Status` while the cancellation a `CancelResize` asked
/// for is being applied. See [`RESIZE_STATUS_IN_PROGRESS`] for the published
/// vocabulary this is one of.
pub const RESIZE_STATUS_CANCELLING: &str = "CANCELLING";
/// `ResizeProgressMessage$Status` of a resize that did NOT complete, which is
/// what a cancelled one comes to: the published vocabulary
/// ([`RESIZE_STATUS_IN_PROGRESS`]) carries no terminal value for "cancelled", and
/// the one terminal value that is not success is this.
pub const RESIZE_STATUS_FAILED: &str = "FAILED";

/// `ResizeProgressMessage$ResizeType` of a resize that moves the cluster onto new
/// nodes.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="ResizeType")` — "An enum with possible values of `ClassicResize` and
/// `ElasticResize`" — and `ResizeInfo$ResizeType`, whose own doc is "Returns the
/// value `ClassicResize`". A `ModifyCluster` that changes the node type or the
/// cluster type rebuilds the cluster on new nodes, which is the classic resize;
/// the elastic one is what `ResizeCluster` performs by default.
pub const RESIZE_TYPE_CLASSIC: &str = "ClassicResize";

/// `ResizeProgressMessage$ResizeType` of a resize that adds or removes nodes on
/// the cluster as it stands, which is what `ResizeCluster` performs unless its
/// `Classic` member asks for the other kind: "Classic — … the only type of resize
/// available to … a request that sets `Classic` to `true`"
/// (`smithy_query(query="doc", shape="ResizeClusterMessage", member="Classic")`).
/// See [`RESIZE_TYPE_CLASSIC`] for the vocabulary's source.
pub const RESIZE_TYPE_ELASTIC: &str = "ElasticResize";

/// The SHAPE of a cluster: the three members a resize changes, and the three a
/// cancelled resize puts back.
///
/// One value rather than three loose members, so the shape a resize is moving a
/// cluster TO and the shape it moved it FROM are the same kind of thing and a
/// cancel can restore one from the other.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ClusterShape {
    /// `Cluster$NodeType` — the node type the cluster runs on.
    pub node_type: Option<String>,
    /// The cluster's `single-node` / `multi-node` type, which the `Cluster` shape
    /// models no member for and this service keeps on
    /// [`StoredCluster::cluster_type`](super::StoredCluster::cluster_type).
    pub cluster_type: Option<String>,
    /// `Cluster$NumberOfNodes` — how many nodes the cluster runs on.
    pub number_of_nodes: Option<i32>,
}

/// `ResizeProgressMessage$TargetEncryptionType` of a resize whose target cluster
/// is encrypted, and of one whose target cluster is not.
///
/// Source: `smithy_query(query="doc", shape="ResizeProgressMessage",
/// member="TargetEncryptionType")` — "The type of encryption for the cluster after
/// the resize is complete. Possible values are `KMS` and `None`."
pub const TARGET_ENCRYPTION_KMS: &str = "KMS";
/// See [`TARGET_ENCRYPTION_KMS`].
pub const TARGET_ENCRYPTION_NONE: &str = "None";

/// The resize a cluster's last shape change started, as this service stores it.
#[derive(Debug, Clone)]
pub struct StoredResize {
    /// `ResizeProgressMessage$TargetNodeType` — "the node type that the cluster
    /// will have after the resize operation is complete".
    pub target_node_type: Option<String>,
    /// `ResizeProgressMessage$TargetNumberOfNodes` — "the number of nodes that
    /// the cluster will have after the resize operation is complete".
    pub target_number_of_nodes: Option<i32>,
    /// `ResizeProgressMessage$TargetClusterType` — "the cluster type after the
    /// resize operation is complete".
    pub target_cluster_type: Option<String>,
    /// `ResizeProgressMessage$TargetEncryptionType`, one of
    /// [`TARGET_ENCRYPTION_KMS`] / [`TARGET_ENCRYPTION_NONE`].
    pub target_encryption_type: String,
    /// `ResizeProgressMessage$ResizeType`.
    pub resize_type: String,
    /// The instant the call that asked for the resize was served at, which every
    /// elapsed time is measured from.
    pub started_at: Instant,
    /// When the clock-owed [`RESIZE_STATUS_IN_PROGRESS`] ->
    /// [`RESIZE_STATUS_SUCCEEDED`] transition falls due. See
    /// [`PROVISIONING_SPAN`]: moving a cluster onto new nodes is the same slow
    /// work provisioning one is.
    pub completed_at: Instant,
    /// The shape the cluster had BEFORE this resize started, which is what a
    /// `CancelResize` puts back: "cancels a resize operation for a cluster", so
    /// the cluster the cancel leaves behind is the cluster the resize found.
    pub source: ClusterShape,
    /// The instant a `CancelResize` asked for this resize to be given up on, and
    /// `None` for a resize nobody cancelled.
    pub cancelled_at: Option<Instant>,
}

impl StoredResize {
    /// The resize that moves a cluster from the shape `source` to the shape
    /// `target` at `started`, on a cluster whose data is `encrypted`, of the kind
    /// `resize_type` ([`RESIZE_TYPE_CLASSIC`] / [`RESIZE_TYPE_ELASTIC`]).
    pub fn started(
        started: Instant,
        source: ClusterShape,
        target: ClusterShape,
        encrypted: bool,
        resize_type: &str,
    ) -> Self {
        Self {
            target_node_type: target.node_type,
            target_number_of_nodes: target.number_of_nodes,
            target_cluster_type: target.cluster_type,
            target_encryption_type: if encrypted {
                TARGET_ENCRYPTION_KMS
            } else {
                TARGET_ENCRYPTION_NONE
            }
            .to_string(),
            resize_type: resize_type.to_string(),
            started_at: started,
            completed_at: started + PROVISIONING_SPAN,
            source,
            cancelled_at: None,
        }
    }

    /// Give this resize up at `now`: it is being cancelled from this instant, and
    /// the cancellation itself falls due a span later — the same span the resize
    /// was running under, because putting the cluster back on its old shape is the
    /// same work as moving it onto the new one.
    pub fn cancel(&mut self, now: Instant) {
        self.cancelled_at = Some(now);
        self.completed_at = now + PROVISIONING_SPAN;
    }

    /// Whether this resize is still running at `now` — the one state a
    /// `CancelResize` can give up on.
    pub fn is_in_progress(&self, now: Instant) -> bool {
        self.status_at(now) == RESIZE_STATUS_IN_PROGRESS
    }

    /// The status this resize is in at `now`: running until its completion falls
    /// due, and `SUCCEEDED` from then on — which it stays at until the next
    /// resize REPLACES this record.
    /// A resize a `CancelResize` gave up on never reaches
    /// [`RESIZE_STATUS_SUCCEEDED`]: it is [`RESIZE_STATUS_CANCELLING`] while the
    /// cancellation is being applied and [`RESIZE_STATUS_FAILED`] from then on,
    /// because it did not complete.
    pub fn status_at(&self, now: Instant) -> &'static str {
        let done = now >= self.completed_at;
        match (self.cancelled_at.is_some(), done) {
            (true, false) => RESIZE_STATUS_CANCELLING,
            (true, true) => RESIZE_STATUS_FAILED,
            (false, false) => RESIZE_STATUS_IN_PROGRESS,
            (false, true) => RESIZE_STATUS_SUCCEEDED,
        }
    }

    /// `ResizeProgressMessage$ElapsedTimeInSeconds` at `now` — "the amount of
    /// seconds that have elapsed since the resize operation began. After the
    /// resize operation completes, this value shows the total actual time, in
    /// seconds, for the resize operation", so it stops growing when the resize
    /// does.
    ///
    /// The member counts WHOLE seconds, and the span this emulator compresses a
    /// resize into is shorter than one ([`PROVISIONING_SPAN`]) — so a resize that
    /// has finished reports no seconds elapsed, which is what that span honestly
    /// comes to rather than a duration nothing took.
    pub fn elapsed_seconds(&self, now: Instant) -> i64 {
        let until = now.min(self.completed_at);
        seconds_between(self.started_at, until)
    }

    /// `ResizeProgressMessage$EstimatedTimeToCompletionInSeconds` at `now` — "the
    /// estimated time remaining, in seconds, until the resize operation is
    /// complete … Once the resize operation is complete, this value will be 0".
    pub fn remaining_seconds(&self, now: Instant) -> i64 {
        seconds_between(now, self.completed_at)
    }
}

/// Whole seconds from `from` to `to`, and zero where `to` is not after `from`.
///
/// Saturating on both ends: an instant pair this cannot subtract reports no time
/// rather than panicking, and the count cannot overflow the modelled `long`.
fn seconds_between(from: Instant, to: Instant) -> i64 {
    i64::try_from(to.saturating_duration_since(from).as_secs()).unwrap_or(i64::MAX)
}

/// Why a cluster that has never been resized reports no resize.
///
/// Recorded Amazon Redshift wording for `ResizeNotFoundFault` (whose own doc is
/// "A resize operation for the specified cluster is not found"): the fixed text is
/// `No Resize found for Cluster ` followed by the cluster identifier the request
/// named.
pub fn resize_not_found_message(cluster_identifier: &str) -> String {
    format!("No Resize found for Cluster {cluster_identifier}")
}
