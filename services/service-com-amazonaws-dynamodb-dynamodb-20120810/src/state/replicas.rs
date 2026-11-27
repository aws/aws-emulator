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
//! under the CURRENT global tables version, which is the version every regional table of it
//! then reports), and an item written to any of them is written to the others.
//!
//! Every status here is the clock's: a replica is `CREATING`, `UPDATING` or `DELETING`
//! until [`super::STATUS_TRANSITION`] after the call that asked for it, and `ACTIVE` — or
//! gone — from then on. Nothing sleeps for it: the due instant is stored and materialised
//! by the next request ([`super::ComAmazonawsDynamodbDynamodb20120810State::apply_due_transitions`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    GlobalTableWitnessDescription, GlobalTableWitnessGroupUpdate, ReplicaDescription,
    ReplicationGroupUpdate, TableClassSummary, TableWarmThroughputDescription,
};

use super::{
    index_arn, new_table_id, table_arn, CapacityBudget, ComAmazonawsDynamodbDynamodb20120810State,
    ContributorInsights, IndexStatus, Item, ItemKey, KeyWriteBudgets, MeteredCounters,
    OnDemandCeilings, PointInTimeRecovery, RequestError, ResourceTags, Table, TableClassChanges,
    ThroughputHistory, TimeToLive, STATUS_TRANSITION,
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
    /// When the table HOLDING this record acted as the SOURCE Region for the add of this
    /// replica — the instant the `UpdateTable` that created it was made against that table
    /// — and `None` on every other table's record of it, which sourced nothing.
    ///
    /// What it is for is the protection window a newly sourced replica puts the source
    /// Region under ([`sourced_a_replica_within_window`]): the source has to keep serving
    /// the group while the new replica catches up, so it cannot be deleted yet.
    pub sourced_at: Option<SystemTime>,
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
/// replica with no change in flight reports. The witness statuses are the same three words
/// over the same clock (`WitnessStatus`), so one member of a replication group reports its
/// status exactly as another does ([`member_status`]).
pub const REPLICA_ACTIVE: &str = "ACTIVE";

/// Where a member of a replication group stands at `now`: the change in flight while it is
/// still owed, and the settled status once the clock has made it due.
///
/// One reading for every member of a group — a replica and a witness alike — so the two can
/// never disagree about what the clock has already made due.
fn member_status(pending: Option<&PendingReplicaChange>, now: SystemTime) -> &'static str {
    match pending {
        Some(pending) if now < pending.due_at => pending.kind.as_str(),
        _ => REPLICA_ACTIVE,
    }
}

/// Whether the clock has made a group member's REMOVAL due, so the record and whatever
/// stands behind it are gone.
fn member_removal_due(pending: Option<&PendingReplicaChange>, now: SystemTime) -> bool {
    pending.is_some_and(|pending| pending.kind == ReplicaChange::Deleting && now >= pending.due_at)
}

impl TableReplica {
    /// A replica as the call that asked for it leaves it: the settings it named, and the
    /// change the clock owes.
    ///
    /// `sourced` says whether the table this record is being put on is the one the call was
    /// made against, which is the Region that acted as the SOURCE for the add
    /// ([`Self::sourced_at`]).
    pub fn starting(
        settings: TableReplicaSettings,
        kind: ReplicaChange,
        now: SystemTime,
        sourced: bool,
    ) -> Self {
        Self {
            region: settings.region.clone(),
            settings,
            pending: Some(PendingReplicaChange {
                kind,
                due_at: now + STATUS_TRANSITION,
            }),
            sourced_at: (sourced && kind == ReplicaChange::Creating).then_some(now),
        }
    }

    /// Where this replica stands at `now`: the change in flight while it is still owed,
    /// and `ACTIVE` once the clock has made it due.
    pub fn status(&self, now: SystemTime) -> &str {
        member_status(self.pending.as_ref(), now)
    }

    /// Whether the clock has made this replica's REMOVAL due, so the record and the
    /// regional table behind it are gone.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        member_removal_due(self.pending.as_ref(), now)
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
            // "Represents the warm throughput value for this replica"
            // (`ReplicaDescription$WarmThroughput`) — the REPLICA's, not the source
            // table's: warm throughput is "the minimum provisioned capacity maintained for
            // immediate data access" (the `TableWarmThroughputDescription` doc), and the
            // read capacity this replica is provisioned for is the one it overrides
            // ([`TableReplicaSettings::read_capacity_units`]), so that is what it keeps
            // warm for reads. A replica that overrides nothing keeps the source table's
            // pair warm, which is what `warm` carries.
            warm_throughput: Some(TableWarmThroughputDescription {
                read_units_per_second: self
                    .settings
                    .read_capacity_units
                    .or(warm.read_units_per_second),
                ..warm
            }),
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
            return Err(local_region_add_or_delete());
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
            return Err(local_region_add_or_delete());
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

/// How a `Create` or a `Delete` naming the Region the table is already in is refused: the
/// local table is not a replica of itself, so `ReplicaUpdates` is not the member that adds
/// or removes it — AWS's own sentence for that, which names the operations that do and
/// takes nothing from the request.
pub const LOCAL_REGION_THROUGH_REPLICA_UPDATES: &str =
    "Cannot add or delete the local region through ReplicaUpdates. Use CreateTable, \
     DeleteTable, or UpdateTable as required.";

/// That refusal, as the request error this crate answers it with.
pub fn local_region_add_or_delete() -> RequestError {
    RequestError::Validation(LOCAL_REGION_THROUGH_REPLICA_UPDATES.to_string())
}

/// How an `Update` naming the Region the table is already in is refused: the local table's
/// own settings are changed by the members of the request beside `ReplicaUpdates`, not by a
/// replica update naming its Region. The sentence above states the rule for an ADD and a
/// DELETE, so it is not the one for this action, and no recording shows how AWS words this
/// one: the refusal states the rule inside the clause this service opens every refusal of
/// an invalid parameter value with.
pub fn local_region_update(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Cannot update a replica in {region}, which is \
         the Region the table is in"
    ))
}

/// How an `Update` or `Delete` entry of `ReplicaUpdates` naming a Region the table has no
/// replica in is refused: the action addresses a replica of the replication group, and that
/// group has none there — AWS's own sentence for it, which names the two actions it is the
/// answer to and takes nothing from the request.
pub fn replica_update_target_not_found() -> RequestError {
    RequestError::Validation(
        "Replica specified in the Replica Update or Replica Delete action of the request was not \
         found."
            .to_string(),
    )
}

/// The same question on the AUTO SCALING surface, where the member naming a Region is
/// `UpdateTableReplicaAutoScaling`'s own `ReplicaUpdates`: that operation models
/// `ReplicaNotFoundException` of its own and no recording shows its wording, so the refusal
/// states the rule inside this service's invalid-parameter clause rather than borrowing the
/// sentence of another operation's action.
pub fn replica_not_in_group(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: The replica in {region} is not part of this \
         table's replication group"
    ))
}

/// How a `Create` naming a Region that ALREADY HOLDS a table of the group's name is
/// refused — whether that table is a replica of this group or a table of its own, because
/// the replica is created in that Region under the group's name ("The table must have the
/// same name as all of the other replicas") and that name is taken either way.
///
/// AWS's own sentence for it, typographic quotes and all, whose only variable is the table
/// name the request named.
pub fn replica_table_already_exists(name: &str) -> RequestError {
    RequestError::Validation(format!(
        "Failed to create a the new replica of table with name: \u{2018}{name}\u{2019} because \
         one or more replicas already existed as tables."
    ))
}

// =============================================================================
// THE SOURCE REGION'S PROTECTION WINDOW
//
// A Region that has just sourced a new replica cannot be taken out of the group: the
// replica it sourced is still catching up with it, so the group would lose the copy that
// is ahead.
// =============================================================================

/// How long after sourcing a replica add a Region cannot be deleted: 24 hours, the window
/// AWS states in the refusal itself ([`sourced_replica_recently`]).
pub const REPLICA_SOURCE_PROTECTION_WINDOW: Duration = super::DAY;

/// Whether the table acted as the SOURCE Region for a replica added inside the protection
/// window ending at `now`, which is what makes it one the group cannot lose yet.
fn sourced_a_replica_within_window(table: &Table, now: SystemTime) -> bool {
    table.replicas.iter().any(|replica| {
        replica.sourced_at.is_some_and(|sourced_at| {
            now.duration_since(sourced_at)
                .is_ok_and(|elapsed| elapsed < REPLICA_SOURCE_PROTECTION_WINDOW)
                // An add recorded at an instant later than `now` is inside every window
                // that ends at `now`.
                || sourced_at > now
        })
    })
}

/// How the delete of a replica that has sourced an add inside the window is refused: AWS's
/// own sentence for it, which states the role the Region played and the window.
pub fn sourced_replica_recently() -> RequestError {
    RequestError::Validation(
        "Replica cannot be deleted because it has acted as a source region for new replica(s) \
         being added to the table in the last 24 hours."
            .to_string(),
    )
}

/// How a `Disassociate` on a table that belongs to no replication group is refused: there
/// is nothing to disassociate it from.
pub fn not_a_global_table(name: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: Table {name} is not part of a global table"
    ))
}

// =============================================================================
// MULTI-REGION CONSISTENCY
//
// Which replication groups multi-Region STRONG consistency (MRSC) is offered for: the
// `MultiRegionConsistency` member states the mode the group is configured for, and
// `GlobalTableWitnessUpdates` adds or removes the WITNESS that stands in for the group's
// third replica. Both are recorded on every regional table of the group, which is what
// each one's description then reports.
//
// A configuration the service does not serve is refused with AWS's OWN sentence for that
// refusal, which is fixed text carrying nothing of the request. The few refusals in this
// family no recording shows the wording of state the rule in the MODEL's own words, inside
// the clause this service opens every refusal of an invalid parameter value with.
// =============================================================================

/// The value of `MultiRegionConsistency` that asks for multi-Region STRONG consistency:
/// "`STRONG`: Configures a new global table for multi-Region strong consistency (MRSC)"
/// (`UpdateTable$MultiRegionConsistency`).
pub const MULTI_REGION_CONSISTENCY_STRONG: &str = "STRONG";

/// How many Regions one MRSC replication group spans: "A MRSC global table can be
/// configured with either three replicas, or with two replicas and one witness"
/// (`CreateGlobalTableWitnessGroupMemberAction`) — three Regions either way, of which at
/// most one is a witness.
pub const MRSC_GROUP_REGIONS: usize = 3;

/// How many of those Regions may be a WITNESS rather than a full replica: one, in the
/// second of the two configurations that sentence states.
pub const MRSC_GROUP_WITNESSES: usize = 1;

/// One WITNESS Region of a replication group: "a witness provides a cost-effective
/// alternative to a full replica in a MRSC global table by maintaining replicated change
/// data written to global table replicas. You cannot perform read or write operations on a
/// witness" (`UpdateTable$GlobalTableWitnessUpdates`).
///
/// A witness is therefore a Region of the group that holds NO table of the group's name —
/// which is why it is recorded as a member of the group here rather than as a
/// [`TableReplica`], and why nothing is created in its Region.
#[derive(Debug, Clone)]
pub struct TableWitness {
    /// "The name of the Amazon Web Services Region that serves as a witness for the MRSC
    /// global table" (`GlobalTableWitnessDescription$RegionName`).
    pub region: String,
    /// The change the clock still owes this witness, and when it falls due — `None` for a
    /// settled witness, which is `ACTIVE`. A witness carries no settings of its own, so the
    /// only changes it is ever in the middle of are its creation and its removal.
    pub pending: Option<PendingReplicaChange>,
}

impl TableWitness {
    /// A witness as the call that asked for it leaves it: the clock owes the end of the
    /// change a span from `now`, exactly as it does for a replica
    /// ([`TableReplica::starting`]).
    pub fn starting(region: String, kind: ReplicaChange, now: SystemTime) -> Self {
        Self {
            region,
            pending: Some(PendingReplicaChange {
                kind,
                due_at: now + STATUS_TRANSITION,
            }),
        }
    }

    /// Where this witness stands at `now` — "`CREATING`", "`DELETING`" or "`ACTIVE`"
    /// (`WitnessStatus`), read off the clock the way every member of a group's status is
    /// ([`member_status`]).
    pub fn status(&self, now: SystemTime) -> &str {
        member_status(self.pending.as_ref(), now)
    }

    /// Whether the clock has made this witness's REMOVAL due, so the group no longer holds
    /// it.
    pub fn removal_due(&self, now: SystemTime) -> bool {
        member_removal_due(self.pending.as_ref(), now)
    }

    /// Start a change on this witness: the clock owes its end a span from `now`.
    pub fn begin(&mut self, kind: ReplicaChange, now: SystemTime) {
        self.pending = Some(PendingReplicaChange {
            kind,
            due_at: now + STATUS_TRANSITION,
        });
    }

    /// This witness as `GlobalTableWitnessDescription` reports it: the Region it serves as a
    /// witness for and the status the clock gives it.
    pub fn describe(&self, now: SystemTime) -> GlobalTableWitnessDescription {
        GlobalTableWitnessDescription {
            region_name: Some(self.region.clone()),
            witness_status: Some(self.status(now).to_string()),
        }
    }
}

/// One action a `GlobalTableWitnessUpdates` entry asks for, after the request has been held
/// to the rules that do not need the stored group ([`validated_witness_updates`]).
#[derive(Debug, Clone)]
pub enum WitnessAction {
    /// "A new witness to be added to a new global table."
    Create(String),
    /// "An existing witness to be removed from an existing global table."
    Delete(String),
}

impl WitnessAction {
    /// The Region this action addresses.
    pub fn region(&self) -> &str {
        match self {
            Self::Create(region) | Self::Delete(region) => region,
        }
    }
}

/// Read a request's `GlobalTableWitnessUpdates` into the actions they ask for, refusing the
/// contradictions that are in the REQUEST rather than in the stored group: an entry states
/// exactly one action ([`ONE_WITNESS_ACTION_MESSAGE`]), and the Region it names has the form
/// a Region code takes ([`checked_region`]).
///
/// How MANY entries the member carries is the model's own `@length min 1, max 1`, which the
/// request walk holds the list to before this runs — "you can create or delete only one
/// witness per UpdateTable operation" (that member's doc) — so the walk below is over at
/// most one entry.
pub fn validated_witness_updates(
    updates: Option<&Vec<GlobalTableWitnessGroupUpdate>>,
) -> Result<Vec<WitnessAction>, RequestError> {
    let entries: &[GlobalTableWitnessGroupUpdate] =
        updates.map_or(&[], |updates| updates.as_slice());
    let mut actions: Vec<WitnessAction> = Vec::with_capacity(entries.len());
    for update in entries {
        actions.push(match (update.create.as_ref(), update.delete.as_ref()) {
            (Some(create), None) => WitnessAction::Create(checked_region(&create.region_name)?),
            (None, Some(delete)) => WitnessAction::Delete(checked_region(&delete.region_name)?),
            _ => {
                return Err(RequestError::Validation(
                    ONE_WITNESS_ACTION_MESSAGE.to_string(),
                ))
            }
        });
    }
    Ok(actions)
}

/// How an entry of `GlobalTableWitnessUpdates` that does not state exactly one action is
/// refused: the entry "represents one of the following: a new witness to be added ... an
/// existing witness to be removed" (`GlobalTableWitnessGroupUpdate`), so one of them and
/// not both or neither — the same rule a `ReplicaUpdates` entry is held to
/// ([`ONE_REPLICA_ACTION_MESSAGE`]).
pub const ONE_WITNESS_ACTION_MESSAGE: &str =
    "One or more parameter values were invalid: Each GlobalTableWitnessUpdate must contain \
     exactly one of Create or Delete";

/// How a `MultiRegionConsistency` sent by a request that creates no replica is refused:
/// "this parameter is only valid when you create a global table by specifying one or more
/// Create actions in the ReplicaUpdates action list" (that member's doc).
pub const CONSISTENCY_WITHOUT_CREATE_MESSAGE: &str =
    "One or more parameter values were invalid: MultiRegionConsistency is only valid when the \
     request creates a global table with one or more Create actions in ReplicaUpdates";

/// How a witness `Create` on a request that is not creating an MRSC global table is
/// refused: "the witness must be added when creating the MRSC global table"
/// (`GlobalTableWitnessGroupUpdate$Create`), so there is no group for it to join.
pub const WITNESS_WITHOUT_MRSC_MESSAGE: &str =
    "One or more parameter values were invalid: A witness Region can only be added when creating \
     a global table with MultiRegionConsistency STRONG";

/// How a configuration whose Regions are not one of the two shapes an MRSC group takes is
/// refused: AWS's own sentence for it, which is fixed text carrying nothing of the request.
pub const UNSUPPORTED_REPLICA_COUNT_MESSAGE: &str =
    "Unsupported replica count for global tables with MultiRegionConsistency set to STRONG.";

/// How a table that carries a LOCAL secondary index is refused MRSC: AWS's own sentence for
/// it, fixed text like the one above — a local secondary index is declared with the table
/// and shares its partition key, and such a table is not one an MRSC group admits.
pub const MRSC_LOCAL_SECONDARY_INDEX_MESSAGE: &str =
    "Local secondary indexes are not supported for global tables with MultiRegionConsistency \
     set to STRONG.";

/// How a witness `Delete` naming a witness the group does not hold is refused: the action
/// addresses "an existing witness to be removed from an existing global table"
/// (`GlobalTableWitnessGroupUpdate$Delete`), and this group has none in that Region.
pub fn witness_not_in_group(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: The witness in {region} is not part of this \
         table's global table"
    ))
}

/// How a witness `Create` naming a Region the group holds a replica in is refused: "the
/// witness must be in a different Region than the replicas"
/// (`CreateGlobalTableWitnessGroupMemberAction$RegionName`).
pub fn witness_region_is_a_replica(region: &str) -> RequestError {
    RequestError::Validation(format!(
        "One or more parameter values were invalid: The witness Region {region} must be a Region \
         other than the Regions of this table's replicas"
    ))
}

/// How a witness `Delete` that is not accompanied by a replica `Delete` is refused: "you
/// cannot delete a single witness from a MRSC global table - you must delete both a replica
/// and the witness together" (`DeleteGlobalTableWitnessGroupMemberAction`), because a group
/// that lost only its witness would be an MRSC group of two Regions, which is not one of the
/// two shapes AWS serves.
pub const WITNESS_DELETE_WITHOUT_REPLICA_MESSAGE: &str =
    "One or more parameter values were invalid: You cannot delete a single witness from a MRSC \
     global table - you must delete both a replica and the witness together";

/// Why one request's multi-Region consistency configuration is not one AWS serves, or
/// `None` when the request asks for a configuration that is.
///
/// Both members state a configuration of the REPLICATION GROUP, so what they ask for is
/// decided against the group the request would leave behind: the local table, the replicas
/// it already has and the ones this request creates or deletes, plus the witnesses it holds
/// and the one this request adds or removes. The rules are the model's own:
///
/// * a witness `Delete` addresses a witness the group holds ([`witness_not_in_group`]) and
///   takes a replica with it ([`WITNESS_DELETE_WITHOUT_REPLICA_MESSAGE`]);
/// * `MultiRegionConsistency` "is only valid when you create a global table by specifying
///   one or more Create actions in the ReplicaUpdates action list"
///   ([`CONSISTENCY_WITHOUT_CREATE_MESSAGE`]), and a witness belongs to the MRSC group that
///   create establishes ([`WITNESS_WITHOUT_MRSC_MESSAGE`]);
/// * an MRSC group spans three Regions, "either three replicas, or ... two replicas and one
///   witness" ([`UNSUPPORTED_REPLICA_COUNT_MESSAGE`]), whose witness is in a Region of its
///   own ([`witness_region_is_a_replica`]);
/// * and the table the group is formed from carries no LOCAL secondary index
///   ([`MRSC_LOCAL_SECONDARY_INDEX_MESSAGE`]).
///
/// A request naming neither member asks for no such configuration and is left alone.
pub fn unsupported_consistency_configuration(
    consistency: Option<&str>,
    witnesses: &[WitnessAction],
    actions: &[ReplicaAction],
    table: &Table,
) -> Option<RequestError> {
    for action in witnesses {
        // A `Delete` addresses "an existing witness to be removed from an existing global
        // table", so a Region this group holds no witness in addresses nothing.
        if let WitnessAction::Delete(region) = action {
            if table.witness(region).is_none() {
                return Some(witness_not_in_group(region));
            }
        }
    }
    let added: Vec<&str> = witnesses
        .iter()
        .filter_map(|action| match action {
            WitnessAction::Create(region) => Some(region.as_str()),
            WitnessAction::Delete(_) => None,
        })
        .collect();
    // A witness leaves the group together with a replica — "you must delete both a replica
    // and the witness together" — so a request that removes the witness alone is refused
    // and the group is left as it was ([`WITNESS_DELETE_WITHOUT_REPLICA_MESSAGE`]).
    if witnesses.len() > added.len()
        && !actions
            .iter()
            .any(|action| matches!(action, ReplicaAction::Delete(_)))
    {
        return Some(RequestError::Validation(
            WITNESS_DELETE_WITHOUT_REPLICA_MESSAGE.to_string(),
        ));
    }
    if consistency.is_none() && witnesses.is_empty() {
        return None;
    }
    let creates = actions
        .iter()
        .filter(|action| matches!(action, ReplicaAction::Create(_)))
        .count();
    if consistency.is_some() && creates == 0 {
        return Some(RequestError::Validation(
            CONSISTENCY_WITHOUT_CREATE_MESSAGE.to_string(),
        ));
    }
    let strong = consistency == Some(MULTI_REGION_CONSISTENCY_STRONG);
    if !strong {
        return added
            .first()
            .map(|_| RequestError::Validation(WITNESS_WITHOUT_MRSC_MESSAGE.to_string()));
    }
    // A local secondary index is declared with the table and indexes its own partition key,
    // which is a table an MRSC group does not admit — asked before the Regions are counted,
    // because it is the TABLE rather than the configuration that cannot join.
    if !table.local_secondary_indexes.is_empty() {
        return Some(RequestError::Validation(
            MRSC_LOCAL_SECONDARY_INDEX_MESSAGE.to_string(),
        ));
    }
    // The local table is a replica of the group it is about to become part of, so the
    // group's replica Regions are it, the replicas it already has and the ones this request
    // creates, less the ones it removes; its witness Regions are the ones it holds plus the
    // one this request adds, less the one it removes. Of the two shapes the model admits,
    // the first spans three replicas and no witness and the second two replicas and one.
    let replica_removals = actions
        .iter()
        .filter(|action| {
            matches!(
                action,
                ReplicaAction::Delete(_) | ReplicaAction::Disassociate(_)
            )
        })
        .count();
    let witness_removals = witnesses.len() - added.len();
    let replicas_after = (1 + table.replicas.len() + creates).saturating_sub(replica_removals);
    let witnesses_after = (table.witnesses.len() + added.len()).saturating_sub(witness_removals);
    let admitted = witnesses_after <= MRSC_GROUP_WITNESSES
        && replicas_after + witnesses_after == MRSC_GROUP_REGIONS;
    if !admitted {
        return Some(RequestError::Validation(
            UNSUPPORTED_REPLICA_COUNT_MESSAGE.to_string(),
        ));
    }
    for region in added {
        if region == table.region
            || table.replica(region).is_some()
            || actions.iter().any(|action| action.region() == region)
        {
            return Some(witness_region_is_a_replica(region));
        }
    }
    None
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

    /// This table's replication group's witness in `region`, if it has one.
    pub fn witness(&self, region: &str) -> Option<&TableWitness> {
        self.witnesses
            .iter()
            .find(|witness| witness.region == region)
    }

    /// The same, for a caller that then changes it.
    pub fn witness_mut(&mut self, region: &str) -> Option<&mut TableWitness> {
        self.witnesses
            .iter_mut()
            .find(|witness| witness.region == region)
    }

    /// This table's group's witnesses as `TableDescription$GlobalTableWitnesses` reports
    /// them, or `None` for a group that has none — a member a table with no witness carries
    /// no list for, the way an unreplicated table carries no `Replicas`.
    pub fn described_witnesses(&self, now: SystemTime) -> Option<Vec<GlobalTableWitnessDescription>> {
        (!self.witnesses.is_empty()).then(|| {
            self.witnesses
                .iter()
                .map(|witness| witness.describe(now))
                .collect()
        })
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
    /// settled change leaves the replica `ACTIVE`. The group's witnesses are settled by the
    /// same clock here, because they are members of the same group: a witness whose removal
    /// has fallen due is dropped and every other one is `ACTIVE`. A table the group has left
    /// with no replica at all is a single-Region table again — "the deletion of both a
    /// witness and replica converts the remaining replica to a single-Region DynamoDB table"
    /// (`GlobalTableWitnessGroupUpdate$Delete`) — so it keeps neither a witness nor the
    /// group's consistency mode.
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
        self.witnesses.retain(|witness| !witness.removal_due(now));
        for witness in &mut self.witnesses {
            if witness
                .pending
                .as_ref()
                .is_some_and(|pending| now >= pending.due_at)
            {
                witness.pending = None;
            }
        }
        if self.replicas.is_empty() {
            self.witnesses.clear();
            self.multi_region_consistency = None;
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
    ///
    /// `stream` is the stream REGISTERED for this replica's own Region, which the caller
    /// takes from the streams registry before anything is written: a global table
    /// replicates over "DynamoDB Streams enabled, with the stream containing both the new
    /// and the old images of the item", and a stream is a resource of ONE Region, so the
    /// replica's change feed is its own rather than the source table's. `None` for a replica
    /// of a table that has no enabled stream, which has no change feed to mirror.
    fn replicated_into(
        &self,
        settings: &TableReplicaSettings,
        stream: Option<super::TableStream>,
        now: SystemTime,
    ) -> Table {
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
                // An index whose delete this table has accepted is on its way out
                // ([`super::GlobalSecondaryIndex::being_deleted`]), so a replica created now
                // is given the index set the source table is left with — the same rule its
                // vector indexes follow.
                .filter(|index| !index.being_deleted())
                .map(|index| super::GlobalSecondaryIndex {
                    arn: index_arn(&region, &self.name, &index.name),
                    throughput_history: ThroughputHistory::default(),
                    reported_counters: MeteredCounters::new(now),
                    contributor_insights: ContributorInsights::default(),
                    transient_status: IndexStatus::Creating,
                    active_at: now + STATUS_TRANSITION,
                    // The replica's index is created WITH the replica's own table, so it
                    // reports no `Backfilling` whatever the source index was reporting
                    // ([`super::IndexBackfill`]).
                    backfill: super::IndexBackfill::WithTable,
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
                // An index whose delete this table has accepted is on its way out
                // ([`super::VectorIndex::being_deleted`]), so a replica created now is
                // given the index set the source table is left with.
                .filter(|index| !index.being_deleted())
                .map(|index| super::VectorIndex {
                    arn: index_arn(&region, &self.name, &index.name),
                    reported_counters: MeteredCounters::new(now),
                    transient_status: IndexStatus::Creating,
                    active_at: now + STATUS_TRANSITION,
                    // Built with the replica's own table from now, so its resources are
                    // being allocated for that span
                    // ([`super::VectorIndex::in_resource_allocation`]).
                    allocated_at: now + STATUS_TRANSITION,
                    deleting_at: None,
                    // Created WITH the replica's own table, by the same rule its secondary
                    // indexes follow ([`super::IndexBackfill`]).
                    backfill: super::IndexBackfill::WithTable,
                    ..index.clone()
                })
                .collect(),
            table_class: settings
                .table_class
                .clone()
                .or_else(|| self.table_class.clone()),
            table_class_changes: TableClassChanges::default(),
            deletion_protection_enabled: false,
            point_in_time_recovery: PointInTimeRecovery::default(),
            tags: ResourceTags::seeded(BTreeMap::new(), now),
            // A tag set is held per resource ARN, so the replica's stream carries its own,
            // which nothing has tagged yet.
            stream_tags: ResourceTags::default(),
            creation_date_time: now,
            active_at: now + STATUS_TRANSITION,
            updated_at: None,
            // The replica is being created with the capacity it will serve, so no capacity
            // move is in flight on it.
            capacity_moving_until: None,
            pay_per_request_at: (self.billing_mode == "PAY_PER_REQUEST").then_some(now),
            removed_at: None,
            // A resource-based policy is attached to ONE table by its own ARN, so the
            // replica carries none until a policy is put on it.
            resource_policy: crate::state::ResourcePolicySlot::default(),
            // Its stream's policy is the stream's own, and no policy is put on a stream the
            // replica's own create registered.
            stream_resource_policy: crate::state::ResourcePolicySlot::default(),
            // The replica's change feed is a stream of its OWN Region, registered for it
            // before anything was written: a global table's replicas are added over a
            // stream carrying both images of each item, and that stream is a resource of
            // the Region the replica lives in.
            stream,
            items: self.items.clone(),
            reported_counters: MeteredCounters::new(now),
            stored_bytes: self.stored_bytes,
            // A new table has banked no credit: what stands behind its first request is the
            // capacity that went unused since its creation began
            // ([`unused_capacity_since`]).
            write_budget: CapacityBudget::new(crate::state::unused_capacity_since(
                now,
                now + STATUS_TRANSITION,
            )),
            read_budget: CapacityBudget::new(crate::state::unused_capacity_since(
                now,
                now + STATUS_TRANSITION,
            )),
            key_write_budgets: KeyWriteBudgets::default(),
            contributor_insights: ContributorInsights::default(),
            time_to_live: TimeToLive::default(),
            kinesis_destinations: Vec::new(),
            restore: None,
            // Filled in by the caller, which knows the whole group this replica joins.
            replicas: Vec::new(),
            // The group's multi-Region consistency configuration is recorded on every
            // regional table of it by the call that establishes the group
            // ([`ComAmazonawsDynamodbDynamodb20120810State::apply_consistency_configuration`]),
            // which reaches this table once it is in the group.
            multi_region_consistency: None,
            witnesses: Vec::new(),
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
        now: SystemTime,
    ) -> Result<(), RequestError> {
        let table = self.table(region, name)?;
        for action in actions {
            match action {
                ReplicaAction::Create(settings) => {
                    // One refusal for a destination Region that already holds a table of the
                    // group's name, whether that table is a replica of this group or a
                    // table of its own ([`replica_table_already_exists`]).
                    if table.replica(&settings.region).is_some()
                        || self.contains_table(&settings.region, name)
                    {
                        return Err(replica_table_already_exists(name));
                    }
                    check_overridden_indexes(table, settings)?;
                }
                ReplicaAction::Update(settings) => {
                    if table.replica(&settings.region).is_none() {
                        return Err(replica_update_target_not_found());
                    }
                    check_overridden_indexes(table, settings)?;
                }
                ReplicaAction::Delete(region) => {
                    if table.replica(region).is_none() {
                        return Err(replica_update_target_not_found());
                    }
                    // A replica that has just SOURCED an add is one the group cannot lose
                    // yet, whichever Region the delete is asked from: the question is asked
                    // of the addressed replica's own regional table, which is where the
                    // record of what it sourced lives ([`sourced_replica_recently`]).
                    if self
                        .table(region, name)
                        .is_ok_and(|replica| sourced_a_replica_within_window(replica, now))
                    {
                        return Err(sourced_replica_recently());
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

    /// Whether the table in `region` is one the replication group cannot lose yet: it acted
    /// as the SOURCE Region for a replica added inside the protection window
    /// ([`REPLICA_SOURCE_PROTECTION_WINDOW`]), so deleting it would take away the copy the
    /// new replica is still catching up with.
    ///
    /// What the two destroying calls ask before they accept: `DeleteTable` of that regional
    /// table, and a `ReplicaUpdates` `Delete` naming it.
    pub fn sourced_a_replica_recently(&self, region: &str, name: &str, now: SystemTime) -> bool {
        self.table(region, name)
            .is_ok_and(|table| sourced_a_replica_within_window(table, now))
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

    /// Whether a replica action's `TableClassOverride` would take that replica's own table
    /// past the table-class change quota, and the sentence it is refused with.
    ///
    /// A `TableClassOverride` moves the CLASS of the replica's regional table ("replica-specific
    /// table class", `UpdateReplicationGroupMemberAction$TableClassOverride`), so it is one of
    /// that table's changes and is held to the same quota a class change made against the table
    /// itself is ([`super::TableClassChanges`]) — asked here, before anything is written, so a
    /// refused update leaves every regional table as it was. An override that re-states the
    /// class the replica is already in changes nothing and spends none of the quota.
    pub fn replica_class_changes_over_limit(
        &self,
        name: &str,
        actions: &[ReplicaAction],
        now: SystemTime,
    ) -> Option<String> {
        for settings in actions.iter().filter_map(|action| match action {
            ReplicaAction::Create(settings) | ReplicaAction::Update(settings) => Some(settings),
            _ => None,
        }) {
            let Some(class) = settings.table_class.as_deref() else {
                continue;
            };
            let Ok(replica) = self.table(&settings.region, name) else {
                // A replica this request CREATES is a new table, whose class the create sets
                // rather than changes.
                continue;
            };
            if replica.table_class.as_deref() != Some(class)
                && replica.table_class_changes.exhausted(now)
            {
                return Some(super::table_class_change_limit_message());
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
    ///
    /// `streams` carries the stream REGISTERED for each created replica's own Region, by
    /// Region ([`Self::create_replica`]): a stream is a resource of one Region and of the
    /// streams service, so the registration is made by the handler before anything here is
    /// written, and what reaches the store is the stream it answered.
    pub fn apply_replica_actions(
        &mut self,
        region: &str,
        name: &str,
        actions: &[ReplicaAction],
        streams: &[(String, super::TableStream)],
        now: SystemTime,
    ) {
        for action in actions {
            match action {
                ReplicaAction::Create(settings) => {
                    let stream = streams
                        .iter()
                        .find(|(registered_region, _)| registered_region == &settings.region)
                        .map(|(_, stream)| stream.clone());
                    self.create_replica(region, name, settings, stream, now);
                }
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
    pub(super) fn group_regions(&self, name: &str) -> Vec<String> {
        self.global_table_groups
            .get(name)
            .map(|group| group.regions.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Record the multi-Region consistency configuration one `UpdateTable` asks for, having
    /// held it to the configurations the service serves
    /// ([`unsupported_consistency_configuration`]) and applied the replica actions beside
    /// it.
    ///
    /// Both members state a configuration of the GROUP rather than of one table, so it is
    /// written to every regional table of the group — the local one and each replica, the
    /// ones this same request created included — which is what makes every one of their
    /// descriptions report the same mode and the same witnesses. A witness `Create` is work
    /// in a Region of its own, so the witness is `CREATING` for the span the clock owes it
    /// and `ACTIVE` from then on, and a `Delete` leaves it `DELETING` until the clock drops
    /// it ([`TableWitness`]).
    pub fn apply_consistency_configuration(
        &mut self,
        region: &str,
        name: &str,
        consistency: Option<&str>,
        witnesses: &[WitnessAction],
        now: SystemTime,
    ) {
        if consistency.is_none() && witnesses.is_empty() {
            return;
        }
        let mut regions = self.group_regions(name);
        if !regions.iter().any(|held| held == region) {
            regions.push(region.to_string());
        }
        for peer_region in regions {
            let Ok(table) = self.table_mut(&peer_region, name) else {
                continue;
            };
            if let Some(mode) = consistency {
                table.multi_region_consistency = Some(mode.to_string());
            }
            for action in witnesses {
                match action {
                    WitnessAction::Create(witness_region) => {
                        if table.witness(witness_region).is_none() {
                            table.witnesses.push(TableWitness::starting(
                                witness_region.clone(),
                                ReplicaChange::Creating,
                                now,
                            ));
                        }
                    }
                    WitnessAction::Delete(witness_region) => {
                        if let Some(witness) = table.witness_mut(witness_region) {
                            witness.begin(ReplicaChange::Deleting, now);
                        }
                    }
                }
            }
        }
    }

    /// Create one replica: the regional table in its own Region — carrying the `stream`
    /// registered for that Region, if the group's table publishes its changes — the record
    /// of it on every table of the group, and the group itself.
    fn create_replica(
        &mut self,
        region: &str,
        name: &str,
        settings: &TableReplicaSettings,
        stream: Option<super::TableStream>,
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
        let mut replica_table = source.replicated_into(settings, stream, now);
        replica_table.replicas = peers
            .into_iter()
            .map(|settings| TableReplica {
                region: settings.region.clone(),
                settings,
                pending: None,
                // The NEW replica sourced none of the group's members: it is the one being
                // added.
                sourced_at: None,
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
                        // Only the Region the request was MADE against acted as the source
                        // for this add; the other members of the group just record it.
                        peer_region == region,
                    ));
                }
            }
        }
        // Adding a replica to a table establishes a group of the CURRENT version — "to
        // create a global table by specifying one or more Create actions in the
        // ReplicaUpdates" — which is the version `DescribeTable` then reports for every
        // regional table of it, and the one that decides which surface addresses the
        // group ([`super::GlobalTableGroup`]).
        let group = self
            .global_table_groups
            .entry(name.to_string())
            .or_insert_with(|| {
                super::GlobalTableGroup::established_under(super::GlobalTablesVersion::Current)
            });
        group.regions.insert(region.to_string());
        group.regions.insert(settings.region.clone());
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
                if table.table_class.as_deref() != Some(class.as_str()) {
                    table.table_class_changes.record(now);
                }
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
            // A table that has left its group is a single-Region table, which carries
            // neither the group's witnesses nor its consistency mode.
            table.witnesses.clear();
            table.multi_region_consistency = None;
            table.updated_at = Some(now + STATUS_TRANSITION);
        }
        if let Some(group) = self.global_table_groups.get_mut(name) {
            group.regions.remove(region);
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
            .is_some_and(|group| group.regions.len() < 2)
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
                group.regions.remove(&region);
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
        // What each replica's own change feed owes for this change, queued once the walk
        // over the regional tables is done, because the queue is the STORE's.
        let mut replicated: Vec<super::PendingStreamRecord> = Vec::new();
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
            // ...and "DynamoDB Streams captures a time-ordered sequence of item-level
            // modifications in any DynamoDB table": the change landed in the replica's own
            // table, so the replica's own stream owes a record for it. Charged to no
            // caller's access: the write the caller made was the one in the source Region.
            if let Some(stream_arn) = table.enabled_stream_arn() {
                let new_image = item.cloned();
                let keys = match new_image.as_ref().or(replaced.as_ref()) {
                    Some(image) => table.stream_keys(image),
                    None => Item::default(),
                };
                replicated.push(super::PendingStreamRecord {
                    region: target.clone(),
                    table_name: table.name.clone(),
                    keys,
                    stream_arn: Some(stream_arn),
                    old_image: replaced,
                    new_image,
                    by_caller: false,
                });
            }
        }
        for change in replicated {
            self.queue_stream_record(Some(change));
        }
    }
}
