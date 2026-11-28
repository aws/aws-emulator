// SPDX-License-Identifier: CC0-1.0

//! `DescribeInstanceOutput`, declared from this crate's overlay model.
//!
//! AWS answers a reported Identity Center instance with the service's instance VIEW — the
//! projection an enumeration serves for it, its `State` structure included — as an
//! `Instance` member beside the flat members the read reports, while the upstream model
//! declares the instance only through those flat members. The model snapshot is kept
//! exactly as the refresh wrote it (see `smithy-model/LOCAL-ADDITIONS.md`), so this one
//! shape is generated from the small overlay model `smithy-model/local-additions.json`,
//! which restates it with that member against the snapshot's own member targets. Nothing
//! about the wire is hand-written: the shape is still declared by
//! `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The instance a `DescribeInstance` reports.
///
/// `instance` is the view [`crate::state::Instance::to_metadata`] projects — the one
/// `ListInstances` serves for the same instance — built from the SAME stored record the
/// flat members are read from, so the two views of one instance cannot disagree.
#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#DescribeInstanceResponse")]
#[derive(Debug, Clone, Default)]
pub struct DescribeInstanceOutput {
    pub instance_arn: Option<String>,
    pub identity_store_id: Option<String>,
    pub identity_store_arn: Option<String>,
    pub owner_account_id: Option<String>,
    pub name: Option<String>,
    pub created_date: Option<::aws_smithy_types::DateTime>,
    pub status: Option<String>,
    pub status_reason: Option<String>,
    pub primary_region: Option<String>,
    pub regions: Option<Vec<RegionMetadata>>,
    pub encryption_configuration_details: Option<EncryptionConfigurationDetails>,
    pub network_configuration_details: Option<NetworkConfigurationInfo>,
    pub permission_sets_enabled: Option<bool>,
    pub instance: Option<InstanceMetadata>,
}
