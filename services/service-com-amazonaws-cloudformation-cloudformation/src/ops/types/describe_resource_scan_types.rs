// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_resource_scan` operation.
//!
//! This file is HAND-WRITTEN at a generated path, and the `@generated` marker is absent on
//! purpose: AWS answers this read with one element the published model snapshot does not
//! declare, and that snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `ResourceTypeSummaries` — a list of `ResourceTypeSummary`, each carrying the
//!   `ResourceType` it counts and how many resources of that type the scan listed. The
//!   element is on every recorded read of a scan, empty while the scan has no per-type
//!   result to report.
//!
//! So the output shape and that list's element are generated, like every other modelled
//! shape, from a model file — the small overlay `smithy-model/local-additions.json`, which
//! restates the snapshot's own members against the snapshot's own targets (so every length
//! and enum constraint is captured exactly as the snapshot spells it) and adds the one
//! element besides. The INPUT shape is unchanged, so it is still generated from the
//! snapshot: the overlay is named on the one attribute that needs it. Nothing about the wire
//! is hand-written — the member names, the shape ids and the constraints all come from a
//! model.
//!
//! `tests/all/registry_and_scan_read_members.rs` pins the element on the wire.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

#[smithy_macros::smithy(shape = "com.amazonaws.cloudformation#DescribeResourceScanInput")]
#[derive(Debug, Clone, Default)]
pub struct DescribeResourceScanInput {
    pub resource_scan_id: String,
}

/// One resource type among the rows a scan listed, with how many of them it accounts for.
///
/// Declared from the overlay model, which is where the shape itself lives: the snapshot
/// declares neither the list nor its element.
#[smithy_macros::smithy(
    shape = "com.amazonaws.cloudformation#ResourceTypeSummary",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct ResourceTypeSummary {
    pub resource_type: Option<String>,
    pub resources_scanned: Option<i32>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.cloudformation#DescribeResourceScanOutput",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct DescribeResourceScanOutput {
    pub resource_scan_id: Option<String>,
    pub status: Option<String>,
    pub status_reason: Option<String>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub percentage_completed: Option<f64>,
    pub resource_types: Option<Vec<String>>,
    pub resources_scanned: Option<i32>,
    pub resources_read: Option<i32>,
    pub scan_filters: Option<Vec<ScanFilter>>,
    /// The one element the snapshot does not declare, which the overlay appends after the
    /// modelled ones: the per-resource-type breakdown of the rows the scan listed.
    pub resource_type_summaries: Option<Vec<ResourceTypeSummary>>,
}
