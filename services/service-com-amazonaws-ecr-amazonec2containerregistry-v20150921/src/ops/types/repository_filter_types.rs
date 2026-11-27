// SPDX-License-Identifier: CC0-1.0

//! The repository filter a replication rule carries, declared with the one member whose
//! value class this service polices itself.
//!
//! What a caller is held to for `RepositoryFilter$filter` is the expression ECR's own
//! validator publishes, which admits the `*` wildcard a namespace is covered with and is
//! WIDER than the `@pattern` the published model snapshot spells for that member (see
//! `crate::state::replication_filter` for the whole rule and its evidence). The request
//! boundary's walk is driven by the model's constraints, so it would hold a caller to the
//! narrower spelling and refuse a wildcard ECR accepts; `skip_validation` keeps that one
//! member off the walk, and the class is judged in
//! [`crate::state::refused_repository_filters`] instead, which both accepts the class and
//! quotes the published expression in its refusal.
//!
//! That is why the shape is declared here rather than in `shared_types.rs`: the member list
//! and every constraint come from the snapshot exactly as a generated declaration takes
//! them, but which member the boundary polices is this crate's own statement about the
//! service, so it lives in a file of the crate's rather than in one rendered from the model
//! and rewritten by the next refresh of it.
//!
//! The filter's `filterType` is untouched by this: its enumerated type is a constraint the
//! model and the service agree on, so the boundary walk still states it.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ecr-amazonec2containerregistry-v20150921.json");

/// One repository filter of one replication rule: which repositories the rule covers, and
/// how the value is read.
#[smithy_macros::smithy(
    shape = "com.amazonaws.ecr#RepositoryFilter",
    skip_validation = ["filter"]
)]
#[derive(Debug, Clone, Default)]
pub struct RepositoryFilter {
    pub filter: String,
    pub filter_type: String,
}
