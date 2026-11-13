// SPDX-License-Identifier: CC0-1.0

//! A table's REPLICAS: the regional tables one global table spans, and the
//! `ReplicaUpdates` actions that add, change and remove them.
//!
//! "A new replica to be added to an existing regional table or global table. This request
//! invokes the `CreateTableReplica` action in the destination Region" / "An existing
//! replica to be deleted. The request invokes the `DeleteTableReplica` action in the
//! destination Region, deleting the replica and all if its items in the destination
//! Region" (the `ReplicationGroupUpdate` doc). So a replica is not a flag on the table it
//! was asked for: it is a TABLE in another Region, created and deleted there, with the
//! same name and schema. This module is therefore where the group of regional tables is
//! kept in step — each one records the other Regions of its group, the store holds the
//! group itself ([`super::ComAmazonawsDynamodbDynamodb20120810State::global_table_groups`],
//! which `ListGlobalTables` pages over), and an item written to any of them is written to
//! the others.
//!
//! Every status here is the clock's: a replica is `CREATING`, `UPDATING` or `DELETING`
//! until [`super::STATUS_TRANSITION`] after the call that asked for it, and `ACTIVE` — or
//! gone — from then on. Nothing sleeps for it: the due instant is stored and materialised
//! by the next request ([`super::ComAmazonawsDynamodbDynamodb20120810State::apply_due_transitions`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    ReplicaDescription, ReplicationGroupUpdate, TableClassSummary, TableWarmThroughputDescription,
};

use super::{
    index_arn, new_table_id, table_arn, CapacityBudget, ComAmazonawsDynamodbDynamodb20120810State,
    ContributorInsights, IndexStatus, Item, ItemKey, KeyWriteBudgets, MeteredCounters,
    OnDemandCeilings, PointInTimeRecovery, RequestError, ResourceTags, Table, ThroughputHistory,
    TimeToLive, STATUS_TRANSITION,
};

/// What one replica OVERRIDES of the table it is a replica of: the Region it lives in and
/// the per-Region settings a `Create` or `Update` action named for it.
///
/// A setting the action left out is not stored as a value of its own: "if not specified,
/// uses the source table's provisioned throughput settings"
/// (`CreateReplicationGroupMemberAction$ProvisionedThroughputOverride`), so an absent
/// override means the source table's setting and the description reports no override.
#[derive(Debug, Clone)]
pub struct TableReplicaSettings {
    /// The name the whole group shares — "The table must have the same name as all of the
    /// other replicas" — which is what addresses the replica's own regional table.
    pub table_name: String,
    /// "The Region where the new replica will be created"
    /// (`CreateReplicationGroupMemberAction$RegionName`).
    pub region: String,
    /// "The KMS key that should be used for KMS encryption in the new replica"
    /// (that action's `KMSMasterKeyId`), as the request named it — a key id, a key ARN, an
    /// alias name or an alias ARN. Taken out by [`resolve_replica_keys`], which asks KMS
    /// what key it names before anything is stored.
    pub requested_kms_key: Option<String>,
    /// The key ARN KMS resolved [`Self::requested_kms_key`] to, which is the key the
    /// replica's own table is encrypted with and what `ReplicaDescription$KMSMasterKeyId`
    /// reports. `None` for a replica the action named no key for, which leaves its table
    /// on the Amazon Web Services owned key, exactly as a table created without an
    /// `SSESpecification` is.
    pub kms_key_arn: Option<String>,
    /// "Replica-specific provisioned throughput" — its read capacity
    /// (`ProvisionedThroughputOverride$ReadCapacityUnits`).
    pub read_capacity_units: Option<i64>,
    /// "Overrides the maximum on-demand throughput ... You can only modify
    /// `MaxReadRequestUnits`, because you can't modify `MaxWriteRequestUnits` for
    /// individual replica tables" (that action's `OnDemandThroughputOverride`).
    pub max_read_request_units: Option<i64>,
    /// "Replica-specific table class. If not specified, uses the source table's table
    /// class" (that action's `TableClassOverride`).
    pub table_class: Option<String>,
    /// "Replica-specific global secondary index settings" (that action's
    /// `GlobalSecondaryIndexes`): one entry per index of the group this Region overrides
    /// a setting of ([`ReplicaIndexSettings`]), in the order the action declared them.
    /// Empty for a replica whose indexes all follow the source table's settings.
    pub indexes: Vec<ReplicaIndexSettings>,
}

/// What one replica OVERRIDES of ONE global secondary index of its group: "replica-specific
/// global secondary index settings" (`ReplicaGlobalSecondaryIndex`).
///
/// A setting the entry left out follows the source table's index, the way an absent
/// table-level override follows the source table, so it is stored as absent rather than as
/// a value of its own.
#[derive(Debug, Clone)]
pub struct ReplicaIndexSettings {
    /// "The name of the global secondary index" (`ReplicaGlobalSecondaryIndex$IndexName`),
    /// which must be one the group's table carries.
    pub index_name: String,
    /// "Replica table GSI-specific provisioned throughput" — its read capacity
    /// (`ProvisionedThroughputOverride$ReadCapacityUnits`).
    pub read_capacity_units: Option<i64>,
    /// "Overrides the maximum on-demand throughput for the specified global secondary
    /// index in the specified replica table" (that entry's `OnDemandThroughputOverride`).
    pub max_read_request_units: Option<i64>,
}

/// "Represents the version of global tables in use, if the table is replicated across
/// Amazon Web Services Regions" (`TableDescription$GlobalTableVersion`): the version
/// `ReplicaUpdates` establishes a group under, which is the current one — the legacy
/// 2017.11.29 version is no longer offered (see `crate::ops::create_global_table`).
pub const GLOBAL_TABLE_VERSION: &str = "2019.11.21";

/// The most Regions one global table's replication group may span: the bound the model
/// puts on the per-replica settings list a single request may carry
/// (`UpdateGlobalTableSettings$ReplicaSettingsUpdate`, `@length max 50`), which is the
/// only figure either side of this API states about a group's size. It bounds the work
/// one `UpdateTable` can ask of the store, so a caller cannot grow a group without end.
pub const MAX_REPLICA_REGIONS: usize = 50;

/// How a `ReplicationGroupUpdate` entry that does not carry exactly one action is
/// refused: "Each `ReplicationGroupUpdate` contains exactly one action" (that shape's
/// doc).
pub const ONE_REPLICA_ACTION_MESSAGE: &str =
    "One or more parameter values were invalid: Each ReplicaUpdate must contain exactly one of \
     Create, Update, Delete or Disassociate";

/// How a `Disassociate` that is not the whole request is refused: "A `Disassociate` action
/// must be the only replica update in the request; including other updates causes the
/// request to fail" (the `ReplicationGroupUpdate` doc).
pub const LONE_DISASSOCIATE_MESSAGE: &str =
    "One or more parameter values were invalid: The Disassociate action must be the only replica \
     update in the request";

/// One replica of a table: the Region its regional table lives in, the settings that
/// Region overrides, and where the change in flight stands.
#[derive(Debug, Clone)]
pub struct TableReplica {
    /// "The name of the Region" the replica's own table lives in
    /// (`ReplicaDescription$RegionName`), which is also the key the group is kept by.
    pub region: String,
    /// The settings this Region overrides, as the `Create`/`Update` action named them.
    pub settings: TableReplicaSettings,
    /// The change the clock still owes this replica, and when it falls due — `None` for a
    /// settled replica, which is `ACTIVE`.
    pub pending: Option<PendingReplicaChange>,
}

/// A replica change in flight: what it is, and when the clock makes it due.
#[derive(Debug, Clone)]
pub struct PendingReplicaChange {
    /// Which of the three transient `ReplicaStatus` values this replica reports until the
    /// change falls due.
    pub kind: ReplicaChange,
    /// The instant it falls due, [`STATUS_TRANSITION`] after the call that asked for it.
    pub due_at: SystemTime,
}

/// The three kinds of change a replica can be in the middle of, each the `ReplicaStatus`
/// it reports while it is: "`CREATING` - The replica is being created", "`UPDATING` - The
/// replica is being updated", "`DELETING` - The replica is being deleted"
/// (`ReplicaDescription$ReplicaStatus`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplicaChange {
    Creating,
    Updating,
    Deleting,
}

impl ReplicaChange {
    /// The `ReplicaStatus` a replica in the middle of this change reports.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Creating => "CREATING",
            Self::Updating => "UPDATING",
            Self::Deleting => "DELETING",
        }
    }
}

/// "`ACTIVE` - The replica is ready for use" (`ReplicaDescription$ReplicaStatus`): what a
/// replica with no change in flight reports.
pub const REPLICA_ACTIVE: &str = "ACTIVE";

impl TableReplica {
    /// A replica as the call that asked for it leaves it: the settings it named, and the
    /// change the clock owes.
    pub fn starting(
        settings: TableReplicaSettings,
        kind: ReplicaChange,
        now: SystemTime,
    ) -> Self {
        Self {
            region: settings.region.clone(),
            settings,
            pending: Some(PendingReplicaChange {
                kind,
                due_at: now + STATUS_TRANSITION,
            }),
        }
    }

    /// Where this replica stands at `now`: the change in flight while it is still owed,
    /// and `ACTIVE` once the clock has made it due.
    pub fn status(&self, now: SystemTime) -> &str {
        match self.pending.as_ref() {
            Some(pending) if now < pending.due_at => pending.kind.as_str(),
            _ => REPLICA_ACTIVE,
        }
    }

    /// Whether the clock has made this replica's REMOVAL due, so the record and the
    /// regional table behind it are gone.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.kind == ReplicaChange::Deleting && now >= pending.due_at
        })
    }

    /// Start a change on this replica, with the settings it asks for: the clock owes the
    /// move back to `ACTIVE` a span from `now`.
    pub fn begin(&mut self, kind: ReplicaChange, now: SystemTime) {
        self.pending = Some(PendingReplicaChange {
            kind,
            due_at: now + STATUS_TRANSITION,
        });
    }

    /// This replica as `ReplicaDescription` reports it: every member read from the record,
    /// and a member the replica overrides nothing for left absent — "if not described,
    /// uses the source table's provisioned throughput settings"
    /// (`ReplicaDescription$ProvisionedThroughputOverride`).
    pub fn describe(&self, now: SystemTime, warm: TableWarmThroughputDescription) -> ReplicaDescription {
        ReplicaDescription {
            region_name: Some(self.region.clone()),
            replica_status: Some(self.status(now).to_string()),
            // "The Amazon Resource Name (ARN) of the global table replica"
            // (`ReplicaDescription$ReplicaArn`): the ARN the replica's own regional table
            // reports for itself, minted from its Region and the shared table name.
            replica_arn: Some(table_arn(&self.region, &self.settings.table_name)),
            // "The KMS key of the replica that will be used for KMS encryption"
            // (`ReplicaDescription$KMSMasterKeyId`): the key ARN KMS resolved the action's
            // identifier to, which is the key the replica's own table is encrypted with —
            // absent for a replica on the Amazon Web Services owned key.
            kms_master_key_id: self.settings.kms_key_arn.clone(),
            // "Detailed information about the replica status" and "the progress of a
            // Create, Update, or Delete action on the replica as a percentage": a
            // description of a replica this emulator has nothing further to say about
            // than its status, so neither is reported.
            replica_status_description: None,
            replica_status_percent_progress: None,
            provisioned_throughput_override: self.settings.read_capacity_units.map(|units| {
                crate::ops::types::ProvisionedThroughputOverride {
                    read_capacity_units: Some(units),
                }
            }),
            on_demand_throughput_override: self.settings.max_read_request_units.map(|units| {
                crate::ops::types::OnDemandThroughputOverride {
                    max_read_request_units: Some(units),
                }
            }),
            warm_throughput: Some(warm),
            // "Replica-specific global secondary index settings": one entry per index this
            // replica overrides a setting of, and no list at all for a replica whose
            // indexes all follow the source table's settings.
            global_secondary_indexes: (!self.settings.indexes.is_empty()).then(|| {
                self.settings
                    .indexes
                    .iter()
                    .map(|index| crate::ops::types::ReplicaGlobalSecondaryIndexDescription {
                        index_name: Some(index.index_name.clone()),
                        provisioned_throughput_override: index.read_capacity_units.map(|units| {
                            crate::ops::types::ProvisionedThroughputOverride {
                                read_capacity_units: Some(units),
                            }
                        }),
                        on_demand_throughput_override: index.max_read_request_units.map(|units| {
                            crate::ops::types::OnDemandThroughputOverride {
                                max_read_request_units: Some(units),
                            }
                        }),
                        // An index's warm throughput is the index's own, not a replica
                        // override: no replica action names one.
                        warm_throughput: None,
                    })
                    .collect()
            }),
            // "The time at which the replica was first detected as inaccessible": this
            // emulator's replicas stay reachable, so there is no such instant.
            replica_inaccessible_date_time: None,
            replica_table_class_summary: self.settings.table_class.as_ref().map(|class| {
                TableClassSummary {
                    table_class: Some(class.clone()),
                    last_update_date_time: None,
                }
            }),
            global_table_settings_replication_mode: None,
        }
    }
}

impl TableReplicaSettings {
    /// What this replica overrides for the index named `index_name`, if anything.
    pub fn index_override(&self, index_name: &str) -> Option<&ReplicaIndexSettings> {
        self.indexes
            .iter()
            .find(|index| index.index_name == index_name)
    }
}

/// The provisioned throughput one index of a REPLICA carries: the source index's pair with
/// the replica's read-capacity override applied, if it names one.
///
/// "Replica table GSI-specific provisioned throughput" overrides the READ capacity only —
/// the write capacity of a global table's index is the group's, which is why the
/// per-replica override carries no write member at all.
fn overridden_index_throughput(
    index: &super::GlobalSecondaryIndex,
    settings: Option<&ReplicaIndexSettings>,
) -> Option<crate::ops::types::ProvisionedThroughput> {
    index
        .provisioned_throughput
        .as_ref()
        .map(|throughput| crate::ops::types::ProvisionedThroughput {
            read_capacity_units: settings
                .and_then(|settings| settings.read_capacity_units)
                .unwrap_or(throughput.read_capacity_units),
            write_capacity_units: throughput.write_capacity_units,
        })
}

/// Refuse a replica action whose per-index overrides name an index the table does not
/// carry: the entry addresses "the global secondary index" of the group
/// (`ReplicaGlobalSecondaryIndex$IndexName`), so a name no index of the group answers to
/// addresses nothing — the same refusal every other reader of an `IndexName` makes.
fn check_overridden_indexes(
    table: &Table,
    settings: &TableReplicaSettings,
) -> Result<(), RequestError> {
    for index in &settings.indexes {
        if !table
            .global_secondary_indexes
            .iter()
            .any(|carried| carried.name == index.index_name)
        {
            return Err(super::unknown_index_named(&index.index_name));
        }
    }
    Ok(())
}

/// One action a `ReplicaUpdates` entry asks for, after the request has been held to the
/// rules that do not need the stored table ([`validated_replica_updates`]).
#[derive(Debug, Clone)]
pub enum ReplicaAction {
    /// "A new replica to be added to an existing regional table or global table."
    Create(TableReplicaSettings),
    /// "New parameters for an existing replica."
    Update(TableReplicaSettings),
    /// "An existing replica to be deleted."
    Delete(String),
    /// "The local replica to be disassociated from a global table and retained as a
    /// standalone regional table."
    Disassociate(String),
}

impl ReplicaAction {
    /// The Region this action addresses.
    pub fn region(&self) -> &str {
        match self {
            Self::Create(settings) | Self::Update(settings) => &settings.region,
            Self::Delete(region) | Self::Disassociate(region) => region,
        }
    }
}

/// Read a request's `ReplicaUpdates` into the actions it asks for, refusing every
/// contradiction that is in the REQUEST rather than in the stored table.
///
/// `table_region` is the Region the request was made in, which is the local replica's:
/// a `Create` or `Delete` naming it addresses the table the request is already changing
/// rather than a replica of it, and a `Disassociate` names it and nothing else, because
/// it is "the LOCAL replica to be disassociated from a global table".
pub fn validated_replica_updates(
    updates: Option<&Vec<ReplicationGroupUpdate>>,
    table_name: &str,
    table_region: &str,
) -> Result<Vec<ReplicaAction>, RequestError> {
    let Some(updates) = updates.filter(|updates| !updates.is_empty()) else {
        return Ok(Vec::new());
    };
    if updates.len() > MAX_REPLICA_REGIONS {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: ReplicaUpdates may carry at most \
             {MAX_REPLICA_REGIONS} entries, got {}",
            updates.len()
        )));
    }
    let mut actions: Vec<ReplicaAction> = Vec::with_capacity(updates.len());
    for update in updates {
        actions.push(one_action(update, table_name, table_region)?);
    }
    // One entry per Region: two actions on one replica in one call contradict each other
    // whichever two they are, and the second would silently win.
    for (position, action) in actions.iter().enumerate() {
        if actions[..position]
            .iter()
            .any(|earlier| earlier.region() == action.region())
        {
            return Err(RequestError::Validation(format!(
                "One or more parameter values were invalid: ReplicaUpdates names the Region {} \
                 more than once",
                action.region()
            )));
        }
    }
    if actions
        .iter()
        .any(|action| matches!(action, ReplicaAction::Disassociate(_)))
        && actions.len() > 1
    {
        return Err(RequestError::Validation(
            LONE_DISASSOCIATE_MESSAGE.to_string(),
        ));
    }
    Ok(actions)
}

/// One `ReplicaUpdates` entry as the action it asks for.
fn one_action(
    update: &ReplicationGroupUpdate,
    table_name: &str,
    table_region: &str,
) -> Result<ReplicaAction, RequestError> {
    let named = [
        update.create.is_some(),
        update.update.is_some(),
        update.delete.is_some(),
        update.disassociate.is_some(),
    ]
    .into_iter()
    .filter(|named| *named)
    .count();
    if named != 1 {
        return Err(RequestError::Validation(
            ONE_REPLICA_ACTION_MESSAGE.to_string(),
        ));
    }
    if let Some(create) = update.create.as_ref() {
        let region = checked_region(&create.region_name)?;
        if region == table_region {
            return Err(local_region_create(&region));
        }
        return Ok(ReplicaAction::Create(TableReplicaSettings {
            table_name: table_name.to_string(),
            region,
            requested_kms_key: create.kms_master_key_id.clone(),
            kms_key_arn: None,
            read_capacity_units: create
                .provisioned_throughput_override
                .as_ref()
                .and_then(|override_| override_.read_capacity_units),
            max_read_request_units: create
                .on_demand_throughput_override
                .as_ref()
                .and_then(|override_| override_.max_read_request_units),
            table_class: create.table_class_override.clone(),
            indexes: index_overrides(create.global_secondary_indexes.as_ref())?,
        }));
    }
    if let Some(change) = update.update.as_ref() {
        let region = checked_region(&change.region_name)?;
        if region == table_region {
            return Err(local_region_update(&region));
        }
        return Ok(ReplicaAction::Update(TableReplicaSettings {
            table_name: table_name.to_string(),
            region,
            requested_kms_key: change.kms_master_key_id.clone(),
            kms_key_arn: None,
            read_capacity_units: change
                .provisioned_throughput_override
                .as_ref()
                .and_then(|override_| override_.read_capacity_units),
            max_read_request_units: change
                .on_demand_throughput_override
                .as_ref()
                .and_then(|override_| override_.max_read_request_units),
            table_class: change.table_class_override.clone(),
            indexes: index_overrides(change.global_secondary_indexes.as_ref())?,
        }));
    }
    if let Some(delete) = update.delete.as_ref() {
        let region = checked_region(&delete.region_name)?;
        if region == table_region {
            return Err(local_region_delete(&region));
        }
        return Ok(ReplicaAction::Delete(region));
    }
    // The remaining action is the disassociation, which addresses the LOCAL replica.
    let region = update
        .disassociate
        .as_ref()
        .map(|disassociate| checked_region(&disassociate.region_name))
        .transpose()?
        .unwrap_or_default();
    if region != table_region {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: The Disassociate action addresses the \
             local replica, which is in {table_region}, not {region}"
        )));
    }
    Ok(ReplicaAction::Disassociate(region))
}

/// The per-index overrides one `Create`/`Update` action carries, held to the rules that
/// are about the REQUEST: at most one entry per index of a table
/// ([`super::MAX_GLOBAL_SECONDARY_INDEXES`]), and one entry per index name — two entries
/// for one index contradict each other whatever they say, and the second would silently
/// win. Whether the names are indexes the table CARRIES is decided against the stored
/// table ([`ComAmazonawsDynamodbDynamodb20120810State::check_replica_actions`]).
fn index_overrides(
    declared: Option<&Vec<crate::ops::types::ReplicaGlobalSecondaryIndex>>,
) -> Result<Vec<ReplicaIndexSettings>, RequestError> {
    let Some(declared) = declared else {
        return Ok(Vec::new());
    };
    if declared.len() > super::MAX_GLOBAL_SECONDARY_INDEXES {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: a replica may override at most {} global \
             secondary indexes, got {}",
            super::MAX_GLOBAL_SECONDARY_INDEXES,
            declared.len()
        )));
    }
    let mut overrides: Vec<ReplicaIndexSettings> = Vec::with_capacity(declared.len());
    for index in declared {
        if overrides
            .iter()
            .any(|earlier| earlier.index_name == index.index_name)
        {
            return Err(RequestError::Validation(format!(
                "One or more parameter values were invalid: GlobalSecondaryIndexes names the \
                 index {} more than once",
                index.index_name
            )));
        }
        overrides.push(ReplicaIndexSettings {
            index_name: index.index_name.clone(),
            read_capacity_units: index
                .provisioned_throughput_override
                .as_ref()
                .and_then(|override_| override_.read_capacity_units),
            max_read_request_units: index
                .on_demand_throughput_override
                .as_ref()
                .and_then(|override_| override_.max_read_request_units),
        });
    }
    Ok(overrides)
}

/// Ask KMS which key each `Create`/`Update` action's `KMSMasterKeyId` names, in the
/// REPLICA's own Region — a KMS key is regional, so the key a replica is encrypted with is
/// one its own Region's endpoint serves.
///
/// Done before anything is written, so an identifier KMS does not resolve refuses the whole
/// update exactly as the same identifier on an `SSESpecification` does
/// ([`super::resolve_sse_key`]), rather than leaving a replica reporting a key ARN nothing
/// minted. An action that names no key leaves its replica on the Amazon Web Services owned
/// key and asks KMS nothing.
pub fn resolve_replica_keys<Eng: Send + Sync + 'static>(
    engine: &Eng,
    actions: &mut [ReplicaAction],
) -> Result<(), super::SseFault> {
    for action in actions.iter_mut() {
        let settings = match action {
            ReplicaAction::Create(settings) | ReplicaAction::Update(settings) => settings,
            ReplicaAction::Delete(_) | ReplicaAction::Disassociate(_) => continue,
        };
        let Some(requested) = settings.requested_kms_key.clone() else {
            continue;
        };
        settings.kms_key_arn = Some(super::resolve_sse_key(
            engine,
            &settings.region,
            Some(&requested),
        )?);
    }
    Ok(())
}

/// Hold a `RegionName` to the form a Region code takes, which is what makes it a Region
/// this store can key a replica by.
fn checked_region(region: &str) -> Result<String, RequestError> {
    if !super::regions::is_region_code(region) {
        return Err(RequestError::Validation(format!(
            "One or more parameter values were invalid: RegionName {region} is not an Amazon Web \
             Services Region name"
        )));
    }
    Ok(region.to_string())
}

/// How a `Create` naming the Region the table is already in is refused: the local table
/// is not a replica of itself, so there is nothing in that Region to create.
pub fn local_region_create(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Cannot create a replica in {region}, which is \
         the Region the table is in"
    ))
}

/// The same for an `Update`: the local table's own settings are changed by the members of
/// the request beside `ReplicaUpdates`, not by a replica update naming its Region.
pub fn local_region_update(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Cannot update a replica in {region}, which is \
         the Region the table is in"
    ))
}

/// The same for a `Delete`: the local table is removed by `DeleteTable`, and disassociated
/// from its group by the `Disassociate` action.
pub fn local_region_delete(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Cannot delete the replica in {region}, which \
         is the Region the table is in"
    ))
}

/// How an `Update` or `Delete` naming a Region the table has no replica in is refused: the
/// action addresses a replica of the replication group, and that group has none there.
pub fn replica_not_in_group(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: The replica in {region} is not part of this \
         table's replication group"
    ))
}

/// How a `Create` naming a Region the table already has a replica in is refused.
pub fn replica_already_in_group(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: The replica in {region} is already part of \
         this table's replication group"
    ))
}

/// How a `Create` is refused when the destination Region already holds a table of that
/// name that is NOT a replica of this one: the replica is created there under the group's
/// name ("The table must have the same name as all of the other replicas"), and that name
/// is taken.
pub fn replica_name_taken(region: &str, name: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Region {region} already has a table named \
         {name}"
    ))
}

/// How a `Disassociate` on a table that belongs to no replication group is refused: there
/// is nothing to disassociate it from.
pub fn not_a_global_table(name: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Table {name} is not part of a global table"
    ))
}

impl Table {
    /// This table's replica in `region`, if it has one.
    pub fn replica(&self, region: &str) -> Option<&TableReplica> {
        self.replicas.iter().find(|replica| replica.region == region)
    }

    /// The same, for a caller that then changes it.
    pub fn replica_mut(&mut self, region: &str) -> Option<&mut TableReplica> {
        self.replicas
            .iter_mut()
            .find(|replica| replica.region == region)
    }

    /// The Regions this table replicates to whose replica is not on its way out — the
    /// tables an item written here is written to as well.
    pub fn replicating_regions(&self, now: SystemTime) -> Vec<String> {
        self.replicas
            .iter()
            .filter(|replica| !replica.removal_due(now))
            .filter(|replica| {
                replica
                    .pending
                    .as_ref()
                    .is_none_or(|pending| pending.kind != ReplicaChange::Deleting)
            })
            .map(|replica| replica.region.clone())
            .collect()
    }

    /// This table's replicas as `TableDescription$Replicas` reports them, or `None` for a
    /// table that is not replicated at all — a standalone table has no `Replicas` list,
    /// which is also what a disassociated one reports again.
    pub fn described_replicas(&self, now: SystemTime) -> Option<Vec<ReplicaDescription>> {
        if self.replicas.is_empty() {
            return None;
        }
        let (read_units, write_units) = self.warm_throughput.in_force();
        let warm = TableWarmThroughputDescription {
            read_units_per_second: Some(read_units),
            write_units_per_second: Some(write_units),
            status: Some(self.warm_throughput.status(now).as_str().to_string()),
        };
        Some(
            self.replicas
                .iter()
                .map(|replica| replica.describe(now, warm.clone()))
                .collect(),
        )
    }

    /// The global tables version this table is replicated under, reported only for a
    /// table that IS replicated ("if the table is replicated across Amazon Web Services
    /// Regions" — `TableDescription$GlobalTableVersion`).
    pub fn global_table_version(&self) -> Option<String> {
        (!self.replicas.is_empty()).then(|| GLOBAL_TABLE_VERSION.to_string())
    }

    /// Materialise every replica change the clock has already made due, answering the
    /// Regions whose replicas are now GONE.
    ///
    /// A replica whose delete has fallen due is dropped from the record; every other
    /// settled change leaves the replica `ACTIVE`.
    pub fn settle_replicas(&mut self, now: SystemTime) -> Vec<String> {
        let departed: Vec<String> = self
            .replicas
            .iter()
            .filter(|replica| replica.removal_due(now))
            .map(|replica| replica.region.clone())
            .collect();
        self.replicas.retain(|replica| !replica.removal_due(now));
        for replica in &mut self.replicas {
            if replica
                .pending
                .as_ref()
                .is_some_and(|pending| now >= pending.due_at)
            {
                replica.pending = None;
            }
        }
        departed
    }

    /// This table copied into `region` as a REPLICA of it: the same name, schema, indexes
    /// and items, with the identifiers and lifecycle of a table of that Region.
    ///
    /// "None of the replica tables in the global table can contain any data" is the LEGACY
    /// version's precondition; the current version "creates a replica of the table in the
    /// specified Region" and populates it, so the items are copied — the replica answers
    /// the data the source table already held, rather than an empty table a reader would
    /// have to be told about.
    ///
    /// Settings the replica overrides are applied here: its read capacity, its on-demand
    /// read ceiling, its table class ("Replica-specific table class. If not specified,
    /// uses the source table's table class" — `CreateReplicationGroupMemberAction$TableClassOverride`),
    /// its encryption key, and the per-index overrides it carries
    /// ([`ReplicaIndexSettings`]) — so the regional table this returns IS the replica the
    /// action asked for rather than a copy that reports the overrides without applying
    /// them.
    fn replicated_into(&self, settings: &TableReplicaSettings, now: SystemTime) -> Table {
        let region = settings.region.clone();
        let provisioned_throughput = self.provisioned_throughput.as_ref().map(|throughput| {
            crate::ops::types::ProvisionedThroughput {
                read_capacity_units: settings
                    .read_capacity_units
                    .unwrap_or(throughput.read_capacity_units),
                write_capacity_units: throughput.write_capacity_units,
            }
        });
        Table {
            name: self.name.clone(),
            arn: table_arn(&region, &self.name),
            // Nothing has been charged against the replica: what the source table was
            // charged is published for the source table.
            pending_consumed: crate::state::consumed_capacity_metrics::Pending::default(),
            // Each regional table is a resource of its own, so it carries its own
            // `TableId` rather than the source table's.
            id: new_table_id(),
            key_schema: self.key_schema.clone(),
            attribute_definitions: self.attribute_definitions.clone(),
            billing_mode: self.billing_mode.clone(),
            provisioned_throughput,
            warm_throughput: self.warm_throughput.clone(),
            throughput_history: ThroughputHistory::default(),
            global_secondary_indexes: self
                .global_secondary_indexes
                .iter()
                .map(|index| super::GlobalSecondaryIndex {
                    arn: index_arn(&region, &self.name, &index.name),
                    throughput_history: ThroughputHistory::default(),
                    reported_counters: MeteredCounters::new(now),
                    contributor_insights: ContributorInsights::default(),
                    transient_status: IndexStatus::Creating,
                    active_at: now + STATUS_TRANSITION,
                    // An index the action overrides a setting of carries that setting in
                    // this Region; every other index follows the source table's.
                    provisioned_throughput: overridden_index_throughput(
                        index,
                        settings.index_override(&index.name),
                    ),
                    ..index.clone()
                })
                .collect(),
            local_secondary_indexes: self
                .local_secondary_indexes
                .iter()
                .map(|index| super::LocalSecondaryIndex {
                    arn: index_arn(&region, &self.name, &index.name),
                    reported_counters: MeteredCounters::new(now),
                    ..index.clone()
                })
                .collect(),
            vector_indexes: self
                .vector_indexes
                .iter()
                .map(|index| super::VectorIndex {
                    arn: index_arn(&region, &self.name, &index.name),
                    reported_counters: MeteredCounters::new(now),
                    transient_status: IndexStatus::Creating,
                    active_at: now + STATUS_TRANSITION,
                    ..index.clone()
                })
                .collect(),
            table_class: settings
                .table_class
                .clone()
                .or_else(|| self.table_class.clone()),
            table_class_updated_at: None,
            deletion_protection_enabled: false,
            point_in_time_recovery: PointInTimeRecovery::default(),
            tags: ResourceTags::seeded(BTreeMap::new(), now),
            creation_date_time: now,
            active_at: now + STATUS_TRANSITION,
            updated_at: None,
            pay_per_request_at: (self.billing_mode == "PAY_PER_REQUEST").then_some(now),
            removed_at: None,
            // A resource-based policy is attached to ONE table by its own ARN, so the
            // replica carries none until a policy is put on it.
            resource_policy: crate::state::ResourcePolicySlot::default(),
            // Its stream's policy is the stream's own, and the replica is created with no
            // stream at all.
            stream_resource_policy: crate::state::ResourcePolicySlot::default(),
            // The replica's change feed is a stream of its own Region, which only a
            // `StreamSpecification` on this table creates; the replica is created without
            // one rather than reporting an ARN nothing registered.
            stream: None,
            items: self.items.clone(),
            reported_counters: MeteredCounters::new(now),
            stored_bytes: self.stored_bytes,
            write_budget: CapacityBudget::new(now),
            read_budget: CapacityBudget::new(now),
            key_write_budgets: KeyWriteBudgets::default(),
            contributor_insights: ContributorInsights::default(),
            time_to_live: TimeToLive::default(),
            kinesis_destinations: Vec::new(),
            restore: None,
            // Filled in by the caller, which knows the whole group this replica joins.
            replicas: Vec::new(),
            // Encryption is settled per regional table: the replica's own key is the one
            // its `Create` action named, as KMS resolved it ([`resolve_replica_keys`]), in
            // force from the replica's first instant — and a replica whose action named no
            // key stays on the Amazon Web Services owned key, which has no description.
            encryption: settings
                .kms_key_arn
                .clone()
                .map(|key_arn| super::TableEncryption {
                    key_arn,
                    enabled_at: now,
                }),
            on_demand_throughput: OnDemandCeilings {
                max_read_request_units: settings
                    .max_read_request_units
                    .or(self.on_demand_throughput.max_read_request_units),
                max_write_request_units: self.on_demand_throughput.max_write_request_units,
            },
            // The settings the group's capacities are scaled by travel with the replica:
            // the write capacity of a global table is the group's, and the new replica's
            // read capacity follows the table it is a replica of until a replica auto
            // scaling update states its own.
            auto_scaling: self.auto_scaling.clone(),
            region,
        }
    }
}

impl ComAmazonawsDynamodbDynamodb20120810State {
    /// Hold a request's replica actions to the STORED group, refusing every action that
    /// cannot apply — before anything is written, so a refused `UpdateTable` leaves the
    /// group exactly as it was.
    pub fn check_replica_actions(
        &self,
        region: &str,
        name: &str,
        actions: &[ReplicaAction],
    ) -> Result<(), RequestError> {
        let table = self.table(region, name)?;
        for action in actions {
            match action {
                ReplicaAction::Create(settings) => {
                    if table.replica(&settings.region).is_some() {
                        return Err(replica_already_in_group(&settings.region));
                    }
                    if self.contains_table(&settings.region, name) {
                        return Err(replica_name_taken(&settings.region, name));
                    }
                    check_overridden_indexes(table, settings)?;
                }
                ReplicaAction::Update(settings) => {
                    if table.replica(&settings.region).is_none() {
                        return Err(replica_not_in_group(&settings.region));
                    }
                    check_overridden_indexes(table, settings)?;
                }
                ReplicaAction::Delete(region) => {
                    if table.replica(region).is_none() {
                        return Err(replica_not_in_group(region));
                    }
                }
                ReplicaAction::Disassociate(_) => {
                    if table.replicas.is_empty() {
                        return Err(not_a_global_table(name));
                    }
                }
            }
        }
        Ok(())
    }

    /// Which QUOTA one request's replica creates would cross, if any: the message the
    /// caller is answered with, so the caller backs off and asks for fewer.
    ///
    /// A created replica is a TABLE in another Region holding a copy of the group's data,
    /// so it is counted against the same ceilings a create is: how many Regions one group
    /// may span, how many tables one Region may hold ([`super::MAX_TABLES`]), how many
    /// Regions this emulator keeps tables for ([`super::MAX_REGIONS`]), and how many bytes
    /// of item data it holds at all ([`super::MAX_STORE_SIZE_BYTES`]) — the last two being
    /// this emulator's own host-protection ceilings rather than AWS quotas. Measured here,
    /// before anything is copied.
    pub fn replica_creates_over_limit(
        &self,
        region: &str,
        name: &str,
        actions: &[ReplicaAction],
    ) -> Option<String> {
        let table = self.table(region, name).ok()?;
        let mut group = table.replicas.len() + 1;
        let mut projected_bytes = self.stored_bytes();
        for settings in actions.iter().filter_map(|action| match action {
            ReplicaAction::Create(settings) => Some(settings),
            _ => None,
        }) {
            group += 1;
            if group > MAX_REPLICA_REGIONS {
                return Some(format!(
                    "Subscriber limit exceeded: a global table may span at most \
                     {MAX_REPLICA_REGIONS} Regions"
                ));
            }
            if self.table_count_in(&settings.region) >= super::MAX_TABLES {
                return Some(format!(
                    "Subscriber limit exceeded: this account already holds the maximum of {} \
                     tables in {}",
                    super::MAX_TABLES, settings.region
                ));
            }
            if self.region_bucket_exhausted(&settings.region) {
                return Some(format!(
                    "Subscriber limit exceeded: this emulator holds tables for at most {} \
                     regions, and {} is not one of them",
                    super::MAX_REGIONS, settings.region
                ));
            }
            projected_bytes = projected_bytes.saturating_add(table.stored_bytes);
            if projected_bytes > super::MAX_STORE_SIZE_BYTES {
                return Some(format!(
                    "Subscriber limit exceeded: this emulator holds at most {} bytes of item \
                     data",
                    super::MAX_STORE_SIZE_BYTES
                ));
            }
        }
        None
    }

    /// Apply the replica actions of one `UpdateTable`, having checked them
    /// ([`Self::check_replica_actions`]).    ///
    /// Every regional table of the group is kept in step here: a created replica is a new
    /// table in its own Region holding the group's data, and every table of the group —
    /// the local one included — records it. A deleted replica's table is removed on the
    /// same span its record reports `DELETING` for.
    pub fn apply_replica_actions(
        &mut self,
        region: &str,
        name: &str,
        actions: &[ReplicaAction],
        now: SystemTime,
    ) {
        for action in actions {
            match action {
                ReplicaAction::Create(settings) => self.create_replica(region, name, settings, now),
                ReplicaAction::Update(settings) => {
                    self.change_replica(name, settings, now);
                }
                ReplicaAction::Delete(target) => self.delete_replica(name, target, now),
                ReplicaAction::Disassociate(_) => self.disassociate_replica(region, name, now),
            }
        }
    }

    /// The Regions of one table's replication group: the Region of every regional table
    /// that holds it, read from the group the store keeps.
    fn group_regions(&self, name: &str) -> Vec<String> {
        self.global_table_groups
            .get(name)
            .map(|regions| regions.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Create one replica: the regional table in its own Region, the record of it on every
    /// table of the group, and the group itself.
    fn create_replica(
        &mut self,
        region: &str,
        name: &str,
        settings: &TableReplicaSettings,
        now: SystemTime,
    ) {
        let Ok(source) = self.table(region, name) else {
            return;
        };
        // The new replica's own view of the group: every OTHER Region that holds the
        // table, each already settled.
        let mut peers: Vec<TableReplicaSettings> = source
            .replicas
            .iter()
            .map(|replica| replica.settings.clone())
            .collect();
        peers.push(TableReplicaSettings {
            table_name: name.to_string(),
            region: region.to_string(),
            // The table the request was made against overrides nothing of itself: it is
            // the source every other replica's absent override follows.
            requested_kms_key: None,
            kms_key_arn: source
                .encryption
                .as_ref()
                .map(|encryption| encryption.key_arn.clone()),
            read_capacity_units: None,
            max_read_request_units: None,
            table_class: None,
            indexes: Vec::new(),
        });
        let mut replica_table = source.replicated_into(settings, now);
        replica_table.replicas = peers
            .into_iter()
            .map(|settings| TableReplica {
                region: settings.region.clone(),
                settings,
                pending: None,
            })
            .collect();
        let regions_to_record = self.group_regions(name);
        self.insert_table(replica_table);
        // Every table already in the group records the new replica, and so does the local
        // one the request was made against.
        for peer_region in regions_to_record
            .into_iter()
            .chain([region.to_string()])
            .filter(|peer| peer != &settings.region)
        {
            if let Ok(table) = self.table_mut(&peer_region, name) {
                if table.replica(&settings.region).is_none() {
                    table.replicas.push(TableReplica::starting(
                        settings.clone(),
                        ReplicaChange::Creating,
                        now,
                    ));
                }
            }
        }
        let group = self.global_table_groups.entry(name.to_string()).or_default();
        group.insert(region.to_string());
        group.insert(settings.region.clone());
    }

    /// Apply new parameters to one replica: the record every table of the group holds for
    /// it, and the replica's own table — its class, its read capacity, its on-demand read
    /// ceiling, its encryption key and the index settings the action overrides.
    ///
    /// An `Update` states the replica's parameters, so the ones it names replace what the
    /// replica had; a setting it does not name is one the replica's own table keeps.
    fn change_replica(&mut self, name: &str, settings: &TableReplicaSettings, now: SystemTime) {
        for peer_region in self.group_regions(name) {
            if let Ok(table) = self.table_mut(&peer_region, name) {
                if let Some(replica) = table.replica_mut(&settings.region) {
                    replica.settings = settings.clone();
                    replica.begin(ReplicaChange::Updating, now);
                }
            }
        }
        if let Ok(table) = self.table_mut(&settings.region, name) {
            if let Some(class) = settings.table_class.clone() {
                table.table_class_updated_at = Some(now);
                table.table_class = Some(class);
            }
            if let Some(units) = settings.read_capacity_units {
                if let Some(throughput) = table.provisioned_throughput.as_mut() {
                    throughput.read_capacity_units = units;
                }
            }
            table.on_demand_throughput.max_read_request_units = settings
                .max_read_request_units
                .or(table.on_demand_throughput.max_read_request_units);
            // The key the action named, as KMS resolved it: the replica's own table is the
            // resource encrypted with it, so this is where it lands.
            if let Some(key_arn) = settings.kms_key_arn.clone() {
                table.encryption = Some(super::TableEncryption {
                    key_arn,
                    enabled_at: now + STATUS_TRANSITION,
                });
            }
            // Each index the action overrides a setting of carries it in this Region.
            for index in &mut table.global_secondary_indexes {
                let Some(overridden) = settings.index_override(&index.name) else {
                    continue;
                };
                index.provisioned_throughput = overridden_index_throughput(index, Some(overridden));
                index.transient_status = IndexStatus::Updating;
                index.active_at = now + STATUS_TRANSITION;
            }
            table.updated_at = Some(now + STATUS_TRANSITION);
        }
    }

    /// Delete one replica: its record reports `DELETING` on every table of the group for
    /// the span, and its own regional table is removed on the same span — "deleting the
    /// replica and all if its items in the destination Region".
    fn delete_replica(&mut self, name: &str, target: &str, now: SystemTime) {
        for peer_region in self.group_regions(name) {
            if let Ok(table) = self.table_mut(&peer_region, name) {
                if let Some(replica) = table.replica_mut(target) {
                    replica.begin(ReplicaChange::Deleting, now);
                }
            }
        }
        if let Ok(table) = self.table_mut(target, name) {
            table.removed_at = Some(now + STATUS_TRANSITION);
        }
    }

    /// Disassociate the LOCAL replica: it keeps its data and becomes a standalone regional
    /// table, and the rest of the group stops holding a record of it.
    fn disassociate_replica(&mut self, region: &str, name: &str, now: SystemTime) {
        for peer_region in self.group_regions(name) {
            if peer_region == region {
                continue;
            }
            if let Ok(table) = self.table_mut(&peer_region, name) {
                table.replicas.retain(|replica| replica.region != region);
            }
        }
        if let Ok(table) = self.table_mut(region, name) {
            table.replicas.clear();
            table.updated_at = Some(now + STATUS_TRANSITION);
        }
        if let Some(group) = self.global_table_groups.get_mut(name) {
            group.remove(region);
        }
        self.close_emptied_group(name);
    }

    /// Drop the record of one replication group once the table it was about has left it:
    /// a group of fewer than two Regions is no replication group, because a table on its
    /// own is not a global table — which is what no `Replicas` list and no
    /// `GlobalTableVersion` say about it.
    ///
    /// Only the named group is touched, so a group no call of this request changed is left
    /// exactly as it is.
    fn close_emptied_group(&mut self, name: &str) {
        if self
            .global_table_groups
            .get(name)
            .is_some_and(|regions| regions.len() < 2)
        {
            self.global_table_groups.remove(name);
        }
    }

    /// Materialise every replica change the clock has made due, over every table this
    /// store holds, and shrink the groups the departed replicas were part of.
    pub fn apply_due_replica_transitions(&mut self, now: SystemTime) {
        let mut departed: Vec<(String, String)> = Vec::new();
        for tables in self.regions.values_mut() {
            for table in tables.values_mut() {
                for region in table.settle_replicas(now) {
                    departed.push((table.name.clone(), region));
                }
            }
        }
        for (name, region) in departed {
            if let Some(group) = self.global_table_groups.get_mut(&name) {
                group.remove(&region);
            }
            self.close_emptied_group(&name);
        }
    }

    /// Write one item change through to every replica of `name` — the other regional
    /// tables of its replication group.
    ///
    /// "A global table ... is a collection of one or more replica tables, all owned by a
    /// single Amazon Web Services account" whose data is the same in each Region, so an
    /// item that lands in one of them lands in all of them. The work is bounded by the
    /// group's size ([`MAX_REPLICA_REGIONS`]), and a Region whose replica is on its way
    /// out is left alone.
    pub(super) fn replicate_item_change(
        &mut self,
        region: &str,
        name: &str,
        now: SystemTime,
        key: &ItemKey,
        item: Option<&Item>,
    ) {
        let Ok(source) = self.table(region, name) else {
            return;
        };
        if source.replicas.is_empty() {
            return;
        }
        let targets = source.replicating_regions(now);
        for target in targets {
            let Ok(table) = self.table_mut(&target, name) else {
                continue;
            };
            let replaced = match item {
                Some(item) => table.items.insert(key.clone(), item.clone()),
                None => table.items.remove(key),
            };
            table.stored_bytes = table
                .stored_bytes
                .saturating_sub(replaced.as_ref().map_or(0, super::item_size))
                .saturating_add(item.map_or(0, super::item_size));
            // A replicated change is an item-level change of the replica, so the version
            // it replaced is kept for that table's own continuous backups.
            table.record_item_version(now, key, replaced.as_ref());
        }
    }
}
