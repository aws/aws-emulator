// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `start_bulk_put_items` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#StartBulkPutItemsInput")]
#[derive(Debug, Clone, Default)]
pub struct StartBulkPutItemsInput {
    pub client_token: Option<String>,
    pub input_s3_configuration: InputS3Configuration,
    pub output_s3_configuration: OutputS3Configuration,
    pub table_name: String,
    pub max_write_request_units: Option<i64>,
    pub performance_tier: Option<String>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#StartBulkPutItemsOutput")]
#[derive(Debug, Clone, Default)]
pub struct StartBulkPutItemsOutput {
    pub bulk_operation_description: BulkOperationDescription,
}
