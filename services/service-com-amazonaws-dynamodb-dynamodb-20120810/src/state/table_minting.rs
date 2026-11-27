// SPDX-License-Identifier: CC0-1.0

//! Minting one stored table from what its creator DECIDED about it.
//!
//! A table is created from more than one plane — this service's own `CreateTable`, and a
//! generic control plane acting on a table of this account — and each of them decides the
//! same handful of things about it. Everything ELSE a fresh table starts life with belongs to
//! the store rather than to the caller: the `TableId` it is minted with, the capacity budgets
//! its first request is admitted against, the metered counters its description reports, and
//! the settings only a later call of their own can turn on (contributor insights, Time to
//! Live, a Kinesis destination, a replica, auto scaling).
//!
//! So the record is assembled ONCE, here, from the decisions ([`MintedTable`]): a plane that
//! cannot see a field of [`Table`] cannot get it wrong, and a field added to that record is
//! settled in one place instead of in each plane that creates one.

use super::*;

/// What a plane DECIDES about a table it creates; everything else a new table carries is
/// settled by [`mint_table`].
#[derive(Debug)]
pub struct MintedTable {
    /// The Region the table lives in — the scope its name is unique in.
    pub region: String,
    /// The table's own name, already held to the form a table name takes.
    pub name: String,
    /// Its primary key, and the attributes that key is declared over.
    pub key_schema: Vec<KeySchemaElement>,
    /// The attribute definitions declared with it.
    pub attribute_definitions: Vec<AttributeDefinition>,
    /// The read/write capacity mode it is created in.
    pub billing_mode: String,
    /// The capacity a provisioned table holds, and `None` for an on-demand one.
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    /// The warm throughput in force from creation.
    pub warm_throughput: WarmThroughputSettings,
    /// The global secondary indexes declared with it, in declaration order.
    pub global_secondary_indexes: Vec<GlobalSecondaryIndex>,
    /// The local secondary indexes declared with it, which can be declared only here.
    pub local_secondary_indexes: Vec<LocalSecondaryIndex>,
    /// The vector indexes declared with it.
    pub vector_indexes: Vec<VectorIndex>,
    /// The storage class it is created in, or `None` for the service's own default.
    pub table_class: Option<String>,
    /// Whether it is protected against deletion.
    pub deletion_protection_enabled: bool,
    /// The tag set the create seeds, which propagates like any other tag change.
    pub tags: TagSet,
    /// When the table was created — the instant its budgets, its counters and its tag
    /// propagation are measured from.
    pub creation_date_time: SystemTime,
    /// When the clock owes it its `CREATING` -> `ACTIVE` transition. The creating plane
    /// decides it, because a plane that reports the create complete only once the resource
    /// has stabilised has already waited that span out.
    pub active_at: SystemTime,
    /// The resource-based policy seeded in force, which a create may attach strongly
    /// consistently.
    pub resource_policy: ResourcePolicySlot,
    /// The stream the create registered for it in the DynamoDB Streams service, or `None`
    /// for a table that publishes to none.
    pub stream: Option<TableStream>,
    /// Its server-side encryption with a KMS key, or `None` for the Amazon Web Services
    /// owned key.
    pub encryption: Option<TableEncryption>,
    /// The on-demand capacity ceilings in force for it.
    pub on_demand_throughput: OnDemandCeilings,
}

/// The stored table `minted` describes: the decisions it carries, and the state every new
/// table starts life in.
///
/// The ARN and the `TableId` are the OWNING service's to assign, so they are minted here
/// rather than taken from the caller: the ARN is the one [`table_arn`] builds from the Region
/// and the name, and the id is "a unique identifier for the table, in UUID format, generated
/// by DynamoDB when the table is created" (`TableDescription$TableId`).
pub fn mint_table(minted: MintedTable) -> Table {
    let budgets_from = unused_capacity_since(minted.creation_date_time, minted.active_at);
    let MintedTable {
        region,
        name,
        key_schema,
        attribute_definitions,
        billing_mode,
        provisioned_throughput,
        warm_throughput,
        global_secondary_indexes,
        local_secondary_indexes,
        vector_indexes,
        table_class,
        deletion_protection_enabled,
        tags,
        creation_date_time,
        active_at,
        resource_policy,
        stream,
        encryption,
        on_demand_throughput,
    } = minted;
    Table {
        arn: table_arn(&region, &name),
        name,
        // Nothing has been charged against a table nobody has used yet.
        pending_consumed: consumed_capacity_metrics::Pending::default(),
        // Minted once, and echoed by every later description of this table.
        id: new_table_id(),
        region,
        key_schema,
        attribute_definitions,
        // A table created on-demand has `PAY_PER_REQUEST` SET as its capacity mode by the
        // create, so the instant `BillingModeSummary.LastUpdateToPayPerRequestDateTime`
        // records is the instant the table was created at.
        pay_per_request_at: (billing_mode == BILLING_MODE_PAY_PER_REQUEST)
            .then_some(creation_date_time),
        billing_mode,
        provisioned_throughput,
        warm_throughput,
        throughput_history: ThroughputHistory::default(),
        global_secondary_indexes,
        local_secondary_indexes,
        vector_indexes,
        table_class,
        // The create SET the table's class; no call has updated it since, so there is no
        // update instant to report under `TableClassSummary` and the quota's window holds
        // no change of this table's.
        table_class_changes: TableClassChanges::default(),
        deletion_protection_enabled,
        // A new table's continuous backups are on and its point-in-time recovery is off: no
        // create carries a `PointInTimeRecoverySpecification`.
        point_in_time_recovery: PointInTimeRecovery::default(),
        // The seeded set propagates to the tag reader like any other tag change.
        tags: ResourceTags::seeded(tags, creation_date_time),
        // A stream's tags are the stream's own, and a table minted here publishes to a
        // stream nothing has tagged yet.
        stream_tags: ResourceTags::default(),
        creation_date_time,
        active_at,
        updated_at: None,
        // Nothing has moved this table's capacity: it was created with the capacity it
        // serves.
        capacity_moving_until: None,
        removed_at: None,
        resource_policy,
        // A stream's policy is the stream's own, and a table minted here publishes to a
        // stream nothing has attached one to yet.
        stream_resource_policy: ResourcePolicySlot::default(),
        stream,
        items: BTreeMap::new(),
        stored_bytes: 0,
        // A new table has consumed none of the capacity it is provisioned for, so what its
        // first write has behind it is the capacity that went unused since its creation began
        // ([`unused_capacity_since`]), and the same for its reads.
        write_budget: CapacityBudget::new(budgets_from),
        read_budget: CapacityBudget::new(budgets_from),
        key_write_budgets: KeyWriteBudgets::default(),
        // A new table has been metered once, at zero: `ItemCount`/`TableSizeBytes` are
        // refreshed "approximately every six hours", not by the writes that follow.
        reported_counters: MeteredCounters::new(creation_date_time),
        // Each of these is turned on by a call of its own, which no create makes.
        contributor_insights: ContributorInsights::default(),
        time_to_live: TimeToLive::default(),
        kinesis_destinations: Vec::new(),
        // This table was created, not restored, so it has no `RestoreSummary` to report.
        restore: None,
        // A created table is a regional table of its own: a replication group is established
        // by `UpdateTable`'s `ReplicaUpdates`, so it has no replica and is no global table.
        replicas: Vec::new(),
        // ...and no multi-Region consistency configuration either: both members that state
        // one belong to `UpdateTable`, which is what establishes the group they configure.
        multi_region_consistency: None,
        witnesses: Vec::new(),
        encryption,
        on_demand_throughput,
        // Nothing scales a new table's capacities.
        auto_scaling: TableAutoScaling::default(),
    }
}

/// From when the capacity that went UNUSED behind a fresh table is counted
/// ([`CapacityBudget::new`]): the instant its creation BEGAN.
///
/// A table's own create answers it `CREATING` and the clock owes it [`STATUS_TRANSITION`]
/// before it serves anything, so a caller's first request can only arrive once that span has
/// passed and the credit behind it is that span's worth. A plane that mints a table already
/// `ACTIVE` — one that reports a create complete only once the resource has stabilised — has
/// waited the same span out BEFORE it minted the table, so the table's creation began that
/// much earlier and the same credit stands behind its first request. Counting it from
/// `creation_date_time` alone would hand that plane's caller a table that cannot serve one
/// write.
///
/// Stated once, and taken by every path that builds a table's budgets — the creates here, and
/// the restores and the replica that assemble a [`Table`] of their own — so a path that mints
/// a table already `ACTIVE` cannot silently get one that serves nothing.
pub fn unused_capacity_since(
    creation_date_time: SystemTime,
    active_at: SystemTime,
) -> SystemTime {
    active_at
        .checked_sub(STATUS_TRANSITION)
        .unwrap_or(creation_date_time)
        .min(creation_date_time)
}

/// The `BillingMode` value that puts a table in the on-demand capacity mode, as the model's
/// own `BillingMode` enum spells it.
pub const BILLING_MODE_PAY_PER_REQUEST: &str = "PAY_PER_REQUEST";
