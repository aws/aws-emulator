// SPDX-License-Identifier: CC0-1.0

//! `GetStreamGroupOutput`, declared from this crate's overlay model.
//!
//! AWS answers every operation that reports a stream group with the capacity the group
//! holds for itself — `DesiredCapacity`, the capacity its locations have most recently
//! requested, `ActiveCapacity`, the capacity the service has provisioned across them, and
//! `IdleCapacity`, how much of that provisioned capacity is not streaming — while the
//! published model declares capacity only per location, inside `LocationStates`. The
//! published snapshot is kept exactly as shipped (see `smithy-model/LOCAL-ADDITIONS.md`),
//! so this one shape is generated from the small overlay model
//! `smithy-model/local-additions.json`, which restates it with those three members against
//! the snapshot's own member targets. Nothing about the wire is hand-written: the shape is
//! still declared by `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The stream group a `GetStreamGroup` reports.
///
/// The three capacity members are the roll-up of the group's stored locations
/// ([`crate::state::StreamGroup::capacity_summary`]) — the same projection
/// `LocationStates` is reported from, so no value is invented for them.
#[smithy_macros::smithy(shape = "com.amazonaws.gameliftstreams#GetStreamGroupOutput")]
#[derive(Debug, Clone, Default)]
pub struct GetStreamGroupOutput {
    pub arn: String,
    pub description: Option<String>,
    pub default_application: Option<DefaultApplication>,
    pub location_states: Option<Vec<LocationState>>,
    pub stream_class: Option<String>,
    pub id: Option<String>,
    pub status: Option<String>,
    pub status_reason: Option<String>,
    pub last_updated_at: Option<::aws_smithy_types::DateTime>,
    pub created_at: Option<::aws_smithy_types::DateTime>,
    pub expires_at: Option<::aws_smithy_types::DateTime>,
    pub associated_applications: Option<Vec<String>>,
    pub desired_capacity: Option<i32>,
    pub active_capacity: Option<i32>,
    pub idle_capacity: Option<i32>,
}
