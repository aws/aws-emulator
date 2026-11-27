// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `cancel_bulk_operation` operation.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-dynamodb-dynamodb-20120810.json");

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#CancelBulkOperationInput")]
#[derive(Debug, Clone, Default)]
pub struct CancelBulkOperationInput {
    pub bulk_operation_arn: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.dynamodb#CancelBulkOperationOutput")]
#[derive(Debug, Clone, Default)]
pub struct CancelBulkOperationOutput {
    pub bulk_operation_arn: String,
    pub bulk_operation_status: String,
}
