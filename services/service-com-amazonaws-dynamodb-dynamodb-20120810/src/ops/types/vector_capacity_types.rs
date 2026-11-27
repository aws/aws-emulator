// SPDX-License-Identifier: CC0-1.0

//! `VectorCapacity`: what one vector index cost an operation, as AWS carries it.
//!
//! This is the ONE `VectorCapacity` type of this crate — the shape `SearchVectors` reports
//! its own `ConsumedCapacity` through and the one each entry of a write's `VectorIndexes`
//! breakdown carries. It is declared here rather than beside the other shared shapes because
//! AWS answers with one member the published model snapshot does not declare, and that
//! snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `VectorWriteRequestUnits` — the write figure AWS reports beside
//!   `VectorWriteRequestBytes` on every recorded entry of that breakdown. The snapshot
//!   declares the two byte members only.
//!
//! So the type is generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the two members the snapshot
//! declares, with their own target so the figure keeps the type the snapshot gives it, and
//! adds the one AWS carries.
//!
//! What the members CARRY is not decided here: it is one metered figure
//! ([`crate::state::capacity_accounting`]), so the two write members can never disagree
//! about what a write cost.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// What one vector index cost: the bytes a search read, and the bytes — reported under both
/// names AWS carries — a write spent.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#VectorCapacity")]
#[derive(Debug, Clone, Default)]
pub struct VectorCapacity {
    pub vector_search_request_bytes: Option<f64>,
    pub vector_write_request_bytes: Option<f64>,
    /// The member the snapshot does not declare: the same write figure, under the name AWS
    /// reports beside the byte one.
    pub vector_write_request_units: Option<f64>,
}
