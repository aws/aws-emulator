// SPDX-License-Identifier: CC0-1.0

//! Promoting a secondary DB cluster of an Aurora global database to be its primary: the one
//! rule `FailoverGlobalCluster` and `SwitchoverGlobalCluster` both perform.
//!
//! Both operations "promote the specified secondary DB cluster to be the primary DB cluster
//! in the global database cluster" and differ only in WHY — a failover answers an unplanned
//! event and "can result in a loss of write transaction data", a switchover is planned and
//! "there is no data loss" — and so in the transient status the global database reports
//! while it runs. Everything else is one rule, which lives here so the two handlers cannot
//! drift from each other.

use super::*;

/// What a promotion of a global database's primary refuses a `TargetDbClusterIdentifier`
/// that names no DB cluster with.
///
/// The member takes "the Amazon Resource Name (ARN) for the identifier so that Aurora can
/// locate the cluster in its Amazon Web Services Region", and the refusal names the value
/// the request wrote, in the spelling it wrote it in, then points the caller at the thing to
/// change: another promotion target.
pub fn global_cluster_promotion_target_missing_message(named: &str) -> String {
    format!(
        "A DB cluster with ARN {named} doesn't exist. Choose another failover target and try \
         again."
    )
}

/// Why a promotion was refused, in terms of the FAULT the handler answers it with: each/// variant is a modelled error both operations declare, carrying the sentence for it.
#[derive(Debug, Clone)]
pub enum PromotionRejection {
    /// `GlobalClusterNotFoundFault`: "The GlobalClusterIdentifier doesn't refer to an
    /// existing global database cluster."
    GlobalClusterNotFound(String),
    /// `InvalidGlobalClusterStateFault`: "The global cluster is in an invalid state and
    /// can't perform the requested operation."
    GlobalClusterState(String),
    /// `DBClusterNotFoundFault`: "DBClusterIdentifier doesn't refer to an existing DB
    /// cluster."
    DbClusterNotFound(String),
    /// `InvalidDBClusterStateFault`: "The requested operation can't be performed while the
    /// cluster is in this state."
    DbClusterState(String),
    /// The Query API's `InvalidParameterCombination`: the target the request named is a DB
    /// cluster, but not one THIS global database can promote — it is not a member of it, or
    /// it is already its primary — so the two members cannot hold together.
    Combination(String),
}

impl ComAmazonawsRdsAmazonrdsv19State {
    /// Promote the member DB cluster `target` names to be the PRIMARY of the global database
    /// `global_cluster` names, reporting `in_status` while the promotion runs.
    ///
    /// `region` is the region the request was signed for, which is what a `target` naming a
    /// bare identifier resolves in; a global database spans regions, so a target named by its
    /// ARN is resolved in the region that ARN carries ("Use the Amazon Resource Name (ARN) for
    /// the identifier so that Aurora can locate the cluster in its Amazon Web Services
    /// Region").
    ///
    /// Every refusal is decided before anything is written, so a promotion this service
    /// cannot perform leaves the global database and both clusters exactly as they were. The
    /// promotion itself is the member order: the primary of a global database is its first
    /// member ([`GlobalClusterRecord::member_cluster_arns`]), which is what
    /// `GlobalClusterMember$IsWriter` reports, so moving the target to the front IS the
    /// promotion — the old primary becomes a secondary of the cluster that replaced it.
    pub fn promote_global_cluster_primary(
        &mut self,
        region: &str,
        global_cluster: &str,
        target: &str,
        in_status: &str,
    ) -> Result<GlobalClusterRecord, PromotionRejection> {
        // `CreateGlobalCluster` stores the identifier "as a lowercase string", so this
        // lookup folds the name the way every other lookup of it does.
        let global_identifier = global_cluster.to_ascii_lowercase();
        let target_key: RegionalKey = (
            arn_region(target).unwrap_or(region).to_string(),
            db_cluster_identifier_of(target).to_ascii_lowercase(),
        );
        // The transitions the clock has already made due — in the region the request was
        // signed for and, when the ARN named one, in the target's own — so every
        // precondition below is judged against the state the records are really in.
        self.settle_due_transitions(region);
        if target_key.0 != region {
            self.settle_due_transitions(&target_key.0);
        }

        let Some(record) = self.global_clusters.get(&global_identifier) else {
            return Err(PromotionRejection::GlobalClusterNotFound(format!(
                "Global cluster '{global_identifier}' not found"
            )));
        };
        // A global database with a change still being applied is not in a state this one can
        // be taken in — including a promotion that is still running, which is what makes two
        // promotions of one global database serialise.
        if !record.is_available() {
            return Err(PromotionRejection::GlobalClusterState(
                global_cluster_not_available_message(&record.arn(), &record.status),
            ));
        }
        let members = record.member_cluster_arns.clone();

        // The target has to be a DB cluster this service holds: a value that resolves to
        // none is the modelled `DBClusterNotFoundFault`, and nothing is promoted for it.
        let Some(cluster) = self.db_clusters.get(&target_key) else {
            return Err(PromotionRejection::DbClusterNotFound(
                global_cluster_promotion_target_missing_message(target),
            ));
        };
        let target_arn = cluster.arn();
        let target_identifier = cluster.identifier.clone();
        let target_status = cluster.status.clone();
        // A cluster the global database does not hold cannot be promoted WITHIN it, which is
        // the same fault and the same reason `RemoveFromGlobalCluster` answers a detach of a
        // cluster that is not a member.
        if !members.contains(&target_arn) {
            return Err(PromotionRejection::Combination(format!(
                "DB cluster {target_identifier} is not a member of global cluster \
                 {global_identifier}"
            )));
        }
        // "Promotes the specified SECONDARY DB cluster": the cluster that is already the
        // primary has nothing to be promoted to, so the global database is left as it is
        // rather than reporting a promotion that moved nothing.
        if members.first() == Some(&target_arn) {
            return Err(PromotionRejection::Combination(format!(
                "DB cluster {target_identifier} is already the primary cluster of global \
                 cluster {global_identifier}"
            )));
        }
        // A secondary that is not running cannot take over the writes, so the promotion is
        // refused for the state that cluster is in. The status the clock has yet to settle is
        // real, so a promotion issued immediately after the secondary's create is refused.
        if target_status != DB_CLUSTER_STATUS_AVAILABLE {
            return Err(PromotionRejection::DbClusterState(format!(
                "DB cluster {target_identifier} is not available to be promoted (status \
                 {target_status})"
            )));
        }

        // Accepted. Both clusters change roles, so both report the failover they are in and
        // the clock owes each `available`: the global database's writes move from the old
        // primary to the new one, and a caller polling either straight after the call sees
        // the promotion in progress.
        let old_primary = members.first().cloned();
        let Some(record) = self.global_clusters.get_mut(&global_identifier) else {
            return Err(PromotionRejection::GlobalClusterNotFound(format!(
                "Global cluster '{global_identifier}' not found"
            )));
        };
        record.member_cluster_arns.retain(|arn| *arn != target_arn);
        record.member_cluster_arns.insert(0, target_arn.clone());
        record.status = in_status.to_string();
        record.pending_status = Some(PendingStatus::in_a_moment(GLOBAL_CLUSTER_STATUS_AVAILABLE));
        let promoted = record.clone();
        for key in self.global_member_keys(&[Some(target_arn), old_primary]) {
            if let Some(cluster) = self.db_clusters.get_mut(&key) {
                if cluster.status == DB_CLUSTER_STATUS_AVAILABLE {
                    cluster.status = DB_CLUSTER_STATUS_FAILING_OVER.to_string();
                    cluster.pending_status =
                        Some(PendingStatus::in_a_moment(DB_CLUSTER_STATUS_AVAILABLE));
                }
            }
        }
        Ok(promoted)
    }

    /// The store keys of the member clusters `arns` names, skipping the entries that name
    /// none: a global database's members are held as ARNs, and each ARN carries the region
    /// its cluster lives in.
    fn global_member_keys(&self, arns: &[Option<String>]) -> Vec<RegionalKey> {
        arns.iter()
            .flatten()
            .filter_map(|arn| {
                arn_region(arn).map(|region| {
                    (
                        region.to_string(),
                        db_cluster_identifier_of(arn).to_ascii_lowercase(),
                    )
                })
            })
            .collect()
    }
}
