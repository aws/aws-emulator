// SPDX-License-Identifier: CC0-1.0

//! The two table requests that state a THROUGHPUT MODE, as AWS carries them.
//!
//! `CreateTableInput` and `UpdateTableInput` are declared here rather than beside the other
//! types of those operations because AWS accepts a member the published model snapshot does
//! not declare, and that snapshot is kept as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `TableThroughputMode` — the read/write throughput mode the request asks the table to be
//!   in, which a client states instead of — or beside — `BillingMode`, and which is held to
//!   the same `BillingMode` value set as that member.
//!
//! So the types are generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every length and enum value is captured exactly as the
//! snapshot spells it, and adds the one AWS accepts. Nothing about the wire is hand-written:
//! the member names, the shape ids and the constraints all come from that file — so a value
//! outside the declared set is refused by the same validating walk that refuses one in
//! `BillingMode`.
//!
//! The DESCRIPTION the two operations answer with carries the mode too, through a local
//! member of its own, and is declared beside these ([`super::table_description_types`]).
//!
//! What the member MEANS is not decided here: the mode a request resolves to is
//! [`crate::state::throughput_mode`]'s, so the two members cannot ask for different things.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The `CreateTable` request.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#CreateTableInput")]
#[derive(Debug, Clone, Default)]
pub struct CreateTableInput {
    pub attribute_definitions: Option<Vec<AttributeDefinition>>,
    pub table_name: String,
    pub key_schema: Option<Vec<KeySchemaElement>>,
    pub local_secondary_indexes: Option<Vec<LocalSecondaryIndex>>,
    pub global_secondary_indexes: Option<Vec<GlobalSecondaryIndex>>,
    pub billing_mode: Option<String>,
    /// The member the snapshot does not declare: the read/write throughput mode the create
    /// asks for.
    pub table_throughput_mode: Option<String>,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    pub stream_specification: Option<StreamSpecification>,
    pub sse_specification: Option<SSESpecification>,
    pub tags: Option<Vec<Tag>>,
    pub table_class: Option<String>,
    pub deletion_protection_enabled: Option<bool>,
    pub warm_throughput: Option<WarmThroughput>,
    pub resource_policy: Option<String>,
    pub on_demand_throughput: Option<OnDemandThroughput>,
    pub global_table_source_arn: Option<String>,
    pub global_table_settings_replication_mode: Option<String>,
    pub vector_indexes: Option<Vec<VectorIndex>>,
}

/// The `UpdateTable` request.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#UpdateTableInput")]
#[derive(Debug, Clone, Default)]
pub struct UpdateTableInput {
    pub attribute_definitions: Option<Vec<AttributeDefinition>>,
    pub table_name: String,
    pub billing_mode: Option<String>,
    /// See [`CreateTableInput::table_throughput_mode`]: the same member on the update.
    pub table_throughput_mode: Option<String>,
    pub provisioned_throughput: Option<ProvisionedThroughput>,
    pub global_secondary_index_updates: Option<Vec<GlobalSecondaryIndexUpdate>>,
    pub stream_specification: Option<StreamSpecification>,
    pub sse_specification: Option<SSESpecification>,
    pub replica_updates: Option<Vec<ReplicationGroupUpdate>>,
    pub table_class: Option<String>,
    pub deletion_protection_enabled: Option<bool>,
    pub multi_region_consistency: Option<String>,
    pub global_table_witness_updates: Option<Vec<GlobalTableWitnessGroupUpdate>>,
    pub on_demand_throughput: Option<OnDemandThroughput>,
    pub warm_throughput: Option<WarmThroughput>,
    pub global_table_settings_replication_mode: Option<String>,
    pub disassociation_protection_enabled: Option<bool>,
    pub vector_index_updates: Option<Vec<VectorIndexUpdate>>,
}
