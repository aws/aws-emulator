// SPDX-License-Identifier: CC0-1.0

//! The shapes `get_resource_sync_status` reaches, as the published model declares them.
//!
//! Every shape the operation reaches except its output, which this service answers with a
//! narrower member set than the published model declares and which is therefore declared
//! in [`super::overlay`].
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-codestarconnections-codestar-connections-20191201.json");

#[smithy_macros::smithy(shape = "com.amazonaws.codestarconnections#GetResourceSyncStatusInput")]
#[derive(Debug, Clone, Default)]
pub struct GetResourceSyncStatusInput {
    pub resource_name: String,
    pub sync_type: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.codestarconnections#Revision")]
#[derive(Debug, Clone, Default)]
pub struct Revision {
    pub branch: String,
    pub directory: String,
    pub owner_id: String,
    pub repository_name: String,
    pub provider_type: String,
    pub sha: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.codestarconnections#ResourceSyncAttempt")]
#[derive(Debug, Clone)]
pub struct ResourceSyncAttempt {
    pub events: Vec<ResourceSyncEvent>,
    pub initial_revision: Revision,
    pub started_at: ::aws_smithy_types::DateTime,
    pub status: String,
    pub target_revision: Revision,
    pub target: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.codestarconnections#ResourceSyncEvent")]
#[derive(Debug, Clone)]
pub struct ResourceSyncEvent {
    pub event: String,
    pub external_id: Option<String>,
    pub time: ::aws_smithy_types::DateTime,
    pub r#type: String,
}
