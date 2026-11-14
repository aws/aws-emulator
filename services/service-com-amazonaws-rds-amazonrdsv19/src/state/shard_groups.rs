// SPDX-License-Identifier: CC0-1.0

use super::*;

/// The key a DB shard group is stored and looked up under: its identifier folded to
/// lower case.
///
/// `DescribeDBShardGroupsMessage$DBShardGroupIdentifier` "isn't case-sensitive", so two
/// requests differing only in case name ONE shard group — which is what folding the key
/// gives, while the record keeps the identifier in the case `CreateDBShardGroup` named it
/// (nothing in the model says a DB shard group identifier is stored lowercased, and
/// `DBShardGroupIdentifier`'s own pattern admits upper-case letters).
pub fn db_shard_group_key(identifier: &str) -> String {
    identifier.to_ascii_lowercase()
}

/// `arn:aws:rds:<region>:<account>:shard-group:<identifier>` — the ARN form RDS gives a
/// DB shard group, with the `shard-group` resource type its own tagging documentation
/// names it by.
pub fn db_shard_group_arn(region: &str, identifier: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("shard-group:{identifier}"))
}

/// The immutable resource id RDS reports as `DBShardGroupResourceId`: "the Amazon Web
/// Services Region-unique, immutable identifier for the DB shard group", of the same
/// `<prefix>-` plus 26 server-minted characters form as [`mint_db_cluster_resource_id`].
/// Both draw on the one store-wide counter, so no two resources of this service can be
/// minted the same id.
pub fn mint_db_shard_group_resource_id(mint: u64) -> String {
    format!("shardgroup-{}", minted_token("db-shard-group", mint, 26))
}

/// A stored Aurora Limitless Database DB shard group: everything the service persists
/// about it.
///
/// The ARN and the endpoint DNS name are NOT stored as text: they are functions of the
/// stored region, identifier and minted endpoint host, and [`Self::to_wire`] computes
/// them — the same reason [`DbInstanceRecord`] computes its own.
#[derive(Debug, Clone)]
pub struct DbShardGroupRecord {
    /// `DBShardGroupIdentifier`, in the case the create named it.
    pub identifier: String,
    /// The region the shard group was created in; the scope of its ARN and endpoint.
    pub region: String,
    /// `DBClusterIdentifier`: "the name of the primary DB cluster for the DB shard
    /// group", resolved in the store before the shard group was stored.
    pub db_cluster_identifier: String,
    /// `DBShardGroupResourceId`, minted once.
    pub resource_id: String,
    /// The server-assigned middle label of the endpoint DNS name, minted once.
    pub endpoint_host: String,
    /// `MaxACU`: "the maximum capacity of the DB shard group in Aurora capacity units
    /// (ACUs)".
    pub max_acu: f64,
    /// `MinACU`, for the creates that named one.
    pub min_acu: Option<f64>,
    /// `ComputeRedundancy`: how many standby DB data access shards the shard group has.
    pub compute_redundancy: i32,
    /// `Status`.
    pub status: String,
    /// The transition the clock owes this record, if any.
    pub pending_status: Option<PendingStatus>,
    /// `PubliclyAccessible`.
    pub publicly_accessible: bool,
    /// `TagList`.
    pub tags: Vec<Tag>,
}

impl DbShardGroupRecord {
    /// Apply a change whose due instant has passed. Returns whether the record is now
    /// gone, exactly as [`DbInstanceRecord::settle`] does.
    pub(super) fn settle(&mut self, now: Instant) -> bool {
        let due = self
            .pending_status
            .as_ref()
            .is_some_and(|pending| now >= pending.due_at);
        if !due {
            return false;
        }
        match self.pending_status.take().map(|pending| pending.change) {
            Some(PendingChange::Status(status)) => self.status = status,
            Some(PendingChange::Removal) => return true,
            // Unreachable: the change was there a line ago, under this one `&mut self`.
            // Answered as "nothing was due" rather than with a panic in a request path.
            None => {}
        }
        false
    }

    /// `arn:aws:rds:<region>:<account>:shard-group:<identifier>`, from the stored region
    /// and identifier.
    pub fn arn(&self) -> String {
        db_shard_group_arn(&self.region, &self.identifier)
    }

    /// `Endpoint`: "the connection endpoint for the DB shard group", in the RDS endpoint
    /// form `<name>.<server-assigned label>.<region>.rds.amazonaws.com` that
    /// [`DbClusterRecord::endpoint`] states for a cluster, with the label a DB shard
    /// group's own endpoint carries.
    pub fn endpoint(&self) -> String {
        format!(
            "{}.shardgrp-{}.{}.rds.amazonaws.com",
            self.identifier, self.endpoint_host, self.region
        )
    }

    /// Whether the shard group is up: the state `InvalidDBShardGroupStateFault` ("the DB
    /// shard group must be in the available state") is answered for every other one,
    /// `deleting` included.
    pub fn is_available(&self) -> bool {
        self.status == DB_SHARD_GROUP_STATUS_AVAILABLE
    }

    /// Accept a delete: the shard group reports `deleting` from now on, and the clock owes
    /// its removal from the store [`STATUS_TRANSITION_DELAY`] from now, after which every
    /// read of the identifier is a `DBShardGroupNotFoundFault`. Nothing sleeps: the due
    /// instant is stored and materialised by
    /// [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`], exactly as
    /// [`DbClusterRecord::begin_deletion`] does for a DB cluster.
    pub fn begin_deletion(&mut self) {
        self.status = DB_SHARD_GROUP_STATUS_DELETING.to_string();
        self.pending_status = Some(PendingStatus::removal_in_a_moment());
    }

    /// Accept a reboot: the shard group reports [`DB_SHARD_GROUP_STATUS_REBOOTING`] from
    /// now on, and the clock owes it [`DB_SHARD_GROUP_STATUS_AVAILABLE`] again
    /// [`STATUS_TRANSITION_DELAY`] from now, once the momentary outage a restart causes is
    /// over. Nothing sleeps: the due instant is stored and materialised by
    /// [`ComAmazonawsRdsAmazonrdsv19State::settle_due_transitions`], exactly as
    /// [`Self::begin_deletion`] does for an accepted delete.
    pub fn begin_reboot(&mut self) {
        self.status = DB_SHARD_GROUP_STATUS_REBOOTING.to_string();
        self.pending_status = Some(PendingStatus::in_a_moment(DB_SHARD_GROUP_STATUS_AVAILABLE));
    }

    /// Project the stored record onto the modelled `DBShardGroup` wire shape — the output
    /// of every DB shard group operation, and one entry of the `DescribeDBShardGroups`
    /// report, so the two cannot disagree about a shard group.
    pub fn to_wire(&self) -> CreateDBShardGroupOutput {
        CreateDBShardGroupOutput {
            db_shard_group_identifier: Some(self.identifier.clone()),
            db_shard_group_resource_id: Some(self.resource_id.clone()),
            db_shard_group_arn: Some(self.arn()),
            db_cluster_identifier: Some(self.db_cluster_identifier.clone()),
            max_acu: Some(self.max_acu),
            min_acu: self.min_acu,
            compute_redundancy: Some(self.compute_redundancy),
            status: Some(self.status.clone()),
            publicly_accessible: Some(self.publicly_accessible),
            endpoint: Some(self.endpoint()),
            tag_list: Some(self.tags.clone()),
        }
    }
}
