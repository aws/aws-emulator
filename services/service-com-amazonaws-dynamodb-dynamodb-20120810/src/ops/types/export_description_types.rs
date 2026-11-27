// SPDX-License-Identifier: CC0-1.0

//! The EXPORT catalogue's two reader shapes, as AWS carries them.
//!
//! `ExportDescription` and `ExportSummary` are declared here rather than beside the other
//! shapes of the export operations because AWS answers with one member the published model
//! snapshot does not declare, and that snapshot is kept as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `DestinationType` — the KIND of destination an export's data was written to, which
//!   `DescribeExport` and `ListExports` carry beside the destination itself. The snapshot
//!   declares neither the member nor a shape of that name.
//!
//! So the types are generated, like every other modelled shape, from a model file — the small
//! overlay `smithy-model/local-additions.json`, which restates the members the snapshot
//! declares, with their own targets so every length, pattern and enum value is captured
//! exactly as the snapshot spells it, and adds the one AWS carries. An open `string`, because
//! nothing publishes a value set for it.
//!
//! What the member CARRIES is not decided here: it is
//! [`crate::state::Export::destination_type`], so both readers report the same destination
//! kind for one recorded export.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// One export as the catalogue's single-export reader and the call that started it report it.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#ExportDescription")]
#[derive(Debug, Clone, Default)]
pub struct ExportDescription {
    pub export_arn: Option<String>,
    pub export_status: Option<String>,
    pub start_time: Option<::aws_smithy_types::DateTime>,
    pub end_time: Option<::aws_smithy_types::DateTime>,
    pub export_manifest: Option<String>,
    pub table_arn: Option<String>,
    pub table_id: Option<String>,
    pub export_time: Option<::aws_smithy_types::DateTime>,
    pub client_token: Option<String>,
    pub s3_bucket: Option<String>,
    pub s3_bucket_owner: Option<String>,
    pub s3_prefix: Option<String>,
    pub s3_sse_algorithm: Option<String>,
    pub s3_sse_kms_key_id: Option<String>,
    pub failure_code: Option<String>,
    pub failure_message: Option<String>,
    pub export_format: Option<String>,
    pub billed_size_bytes: Option<i64>,
    pub item_count: Option<i64>,
    pub export_type: Option<String>,
    pub incremental_export_specification: Option<IncrementalExportSpecification>,
    pub filter_specification: Option<FilterSpecification>,
    /// The member the snapshot does not declare: the kind of destination the data landed
    /// in, reported by the catalogue's readers.
    pub destination_type: Option<String>,
}

/// One entry of the page `ListExports` answers with.
#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#ExportSummary")]
#[derive(Debug, Clone, Default)]
pub struct ExportSummary {
    pub export_arn: Option<String>,
    pub export_status: Option<String>,
    pub export_type: Option<String>,
    /// See [`ExportDescription::destination_type`]: the same member on a listing's entry.
    pub destination_type: Option<String>,
}
