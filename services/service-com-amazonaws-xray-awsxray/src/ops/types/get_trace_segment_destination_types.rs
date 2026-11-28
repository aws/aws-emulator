// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `get_trace_segment_destination` operation.
//!
//! The `@generated` marker is absent on purpose, and this file is HAND-WRITTEN at a
//! generated path: the read's answer carries the account's trace segment destination, and
//! the set of destinations the service admits is not the one the published snapshot
//! declares, so the answer is declared from the small overlay model
//! `smithy-model/local-additions.json` instead (see `smithy-model/LOCAL-ADDITIONS.md` and
//! `super::update_trace_segment_destination_types`, where the same overlay declares the
//! switch that configures what this read reports). The overlay restates the shape against
//! the snapshot's own target shapes, so the member names and the status type are captured
//! exactly as the snapshot spells them, and only the destination set is the service's.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-xray-awsxray.json");

#[smithy_macros::smithy(shape = "com.amazonaws.xray#GetTraceSegmentDestinationRequest")]
#[derive(Debug, Clone, Default)]
pub struct GetTraceSegmentDestinationInput {
}

/// The read's answer, declared from the overlay model: the destination it reports is one
/// of the set the service admits (see the module docs).
#[smithy_macros::smithy(
    shape = "com.amazonaws.xray#GetTraceSegmentDestinationResult",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct GetTraceSegmentDestinationOutput {
    pub destination: Option<String>,
    pub status: Option<String>,
}
