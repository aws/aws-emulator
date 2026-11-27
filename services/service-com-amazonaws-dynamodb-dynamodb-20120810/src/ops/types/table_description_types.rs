// SPDX-License-Identifier: CC0-1.0

//! `TableDescription`: the table object every table operation reports, as AWS carries it.
//!
//! This is the ONE `TableDescription` type of this crate — the shape `CreateTable`,
//! `DescribeTable`, `UpdateTable` and `DeleteTable` all answer with, and the one
//! [`crate::state::Table::describe`] projects a stored table into.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers with
//! one member the published model snapshot does not declare, and that snapshot is kept as
//! shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `TableThroughputModeSummary` — the read/write throughput mode in force on the table,
//!   which every recorded table description carries beside `BillingModeSummary`. The
//!   snapshot declares neither the member nor a shape of that name.
//!
//! So the types are generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every length, pattern and enum value is captured
//! exactly as the snapshot spells it, and adds the one AWS carries. Nothing about the wire is
//! hand-written: the member names, the shape ids and the constraints all come from that file.
//!
//! The two REQUESTS that state the same mode carry a local member of their own, and are
//! declared beside this one ([`super::table_throughput_mode_types`]).
//!
//! What the members CARRY is not decided here: it is [`crate::state::Table::describe`], so
//! every operation that reports a table reports the same thing for one stored table.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The properties of one table, as every table operation reports them.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#TableDescription")]
#[derive(Debug, Clone, Default)]
pub struct TableDescription {
    pub attribute_definitions: Option<Vec<AttributeDefinition>>,
    pub table_name: Option<String>,
    pub key_schema: Option<Vec<KeySchemaElement>>,
    pub table_status: Option<String>,
    pub creation_date_time: Option<::aws_smithy_types::DateTime>,
    pub provisioned_throughput: Option<ProvisionedThroughputDescription>,
    pub table_size_bytes: Option<i64>,
    pub item_count: Option<i64>,
    pub table_arn: Option<String>,
    pub table_id: Option<String>,
    pub billing_mode_summary: Option<BillingModeSummary>,
    /// The member the snapshot does not declare: the read/write throughput mode in force on
    /// the table, which every recorded description carries.
    pub table_throughput_mode_summary: Option<TableThroughputModeSummary>,
    pub local_secondary_indexes: Option<Vec<LocalSecondaryIndexDescription>>,
    pub global_secondary_indexes: Option<Vec<GlobalSecondaryIndexDescription>>,
    pub stream_specification: Option<StreamSpecification>,
    pub latest_stream_label: Option<String>,
    pub latest_stream_arn: Option<String>,
    pub global_table_version: Option<String>,
    pub replicas: Option<Vec<ReplicaDescription>>,
    pub global_table_witnesses: Option<Vec<GlobalTableWitnessDescription>>,
    pub global_table_settings_replication_mode: Option<String>,
    pub restore_summary: Option<RestoreSummary>,
    pub sse_description: Option<SSEDescription>,
    pub archival_summary: Option<ArchivalSummary>,
    pub table_class_summary: Option<TableClassSummary>,
    pub deletion_protection_enabled: Option<bool>,
    pub disassociation_protection_enabled: Option<bool>,
    pub on_demand_throughput: Option<OnDemandThroughput>,
    pub warm_throughput: Option<TableWarmThroughputDescription>,
    pub multi_region_consistency: Option<String>,
    pub vector_indexes: Option<Vec<VectorIndexDescription>>,
}

/// The read/write throughput mode of one table: the mode in force, and the instant
/// `PAY_PER_REQUEST` was last set on a table that has been in that mode.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#TableThroughputModeSummary")]
#[derive(Debug, Clone, Default)]
pub struct TableThroughputModeSummary {
    pub table_throughput_mode: Option<String>,
    pub last_update_to_pay_per_request_date_time: Option<::aws_smithy_types::DateTime>,
}
