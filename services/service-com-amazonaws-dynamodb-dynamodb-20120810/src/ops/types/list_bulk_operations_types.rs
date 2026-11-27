// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `list_bulk_operations` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#ListBulkOperationsInput")]
#[derive(Debug, Clone, Default)]
pub struct ListBulkOperationsInput {
    pub max_results: Option<i32>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#ListBulkOperationsOutput")]
#[derive(Debug, Clone, Default)]
pub struct ListBulkOperationsOutput {
    pub bulk_operations_summary_list: Option<Vec<BulkOperationSummary>>,
    pub next_token: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#BulkOperationSummary")]
#[derive(Debug, Clone)]
pub struct BulkOperationSummary {
    pub bulk_operation_arn: String,
    pub bulk_operation_status: String,
    pub start_time: ::aws_smithy_types::DateTime,
    pub end_time: Option<::aws_smithy_types::DateTime>,
}
