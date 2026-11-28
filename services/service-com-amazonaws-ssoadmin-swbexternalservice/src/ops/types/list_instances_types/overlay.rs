// SPDX-License-Identifier: CC0-1.0

//! The instance VIEW this service serves, declared from this crate's overlay model.
//!
//! AWS answers every read that reports an Identity Center instance view with a `State`
//! structure whose single `Name` is that instance's own lifecycle value, while the upstream
//! model declares the lifecycle on the view only through `Status`. The model snapshot is
//! kept exactly as the refresh wrote it (see `smithy-model/LOCAL-ADDITIONS.md`), so the
//! view and the structure it carries are generated from the small overlay model
//! `smithy-model/local-additions.json`, which restates the view with that member against
//! the snapshot's own member targets. Nothing about the wire is hand-written: both shapes
//! are still declared by `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// One Identity Center instance as every read that enumerates or repeats one reports it.
///
/// `state` is the same lifecycle `status` carries, read off the one stored field
/// ([`crate::state::Instance::state`]), so the two views of one instance's lifecycle
/// cannot disagree.
#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#InstanceMetadata")]
#[derive(Debug, Clone, Default)]
pub struct InstanceMetadata {
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
    pub state: Option<InstanceState>,
}

/// The lifecycle structure an instance view carries: one `Name`, the instance's status.
#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#InstanceState")]
#[derive(Debug, Clone, Default)]
pub struct InstanceState {
    pub name: Option<String>,
}
