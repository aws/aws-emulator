// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `update_trace_segment_destination` operation.
//!
//! The `@generated` marker is absent on purpose, and this file is HAND-WRITTEN at a
//! generated path: the published model snapshot beside it is kept EXACTLY as shipped, and
//! the one shape whose published value set is not the one the service admits —
//! `TraceSegmentDestination`, which the snapshot declares with two destinations where the
//! service admits a third — is declared from the small overlay model
//! `smithy-model/local-additions.json` instead (see `smithy-model/LOCAL-ADDITIONS.md`).
//! The overlay restates this operation's input and output against the snapshot's own
//! target shapes, so every other part of them — the member names, the nested
//! configuration, the length bound on the KMS key it carries, the status type — is
//! captured exactly as the snapshot spells it, and only the destination set is the
//! service's. Nothing about the wire is hand-written: the member names, the shape ids and
//! the constraints all come from a model file.
//!
//! The input and the output are declared from the SAME file, so the destinations a request
//! may carry and the ones an answer reports can never be two different sets.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-xray-awsxray.json");

/// The destination switch's input, declared from the overlay model — the shape whose
/// destination member targets the enum the snapshot declares short (see the module docs).
#[smithy_macros::smithy(
    shape = "com.amazonaws.xray#UpdateTraceSegmentDestinationRequest",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct UpdateTraceSegmentDestinationInput {
    pub destination: Option<String>,
    pub cloud_watch_logs_configuration: Option<CloudWatchLogsConfiguration>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.xray#CloudWatchLogsConfiguration")]
#[derive(Debug, Clone, Default)]
pub struct CloudWatchLogsConfiguration {
    pub spans_log_group_kms_key_id: Option<String>,
}

/// The switch's answer, declared from the overlay model for the same reason as the input:
/// the destination it reports is one of the set the service admits.
#[smithy_macros::smithy(
    shape = "com.amazonaws.xray#UpdateTraceSegmentDestinationResult",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct UpdateTraceSegmentDestinationOutput {
    pub destination: Option<String>,
    pub status: Option<String>,
}

/// The values the overlay's `TraceSegmentDestination` enum declares, in the form every
/// generated enum value list takes — the catalogue a configured destination is admitted
/// from and reported out of.
///
/// Written out here because the shape is declared in the overlay rather than in the
/// snapshot, whose own catalogue
/// ([`TRACE_SEGMENT_DESTINATION_VALUES`](super::shared_types::TRACE_SEGMENT_DESTINATION_VALUES))
/// is short of the destination the service admits beyond it;
/// `tests/all/trace_segment_destination_local_addition.rs` checks this list against the
/// overlay file itself.
pub const ADMITTED_TRACE_SEGMENT_DESTINATIONS: &[&str] =
    &["CloudWatchLogs", "Composite", "XRay"];
