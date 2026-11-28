// SPDX-License-Identifier: CC0-1.0

//! The two S3 Batch Operations shapes that carry a CALLER-SUPPLIED MAP, declared
//! over an ordered map so a job's own document reads back the same way every
//! time.
//!
//! `LambdaInvokeOperation$UserArguments` and `S3ObjectMetadata$UserMetadata` are
//! echoed verbatim by every answer about the job that carried them
//! (`CreateJob`'s record, `DescribeJob`'s report, and the payload a
//! `LambdaInvoke` task is handed), so the ORDER their entries are written in is
//! part of what a read reports: a `BTreeMap` is what makes two reads of one
//! unchanged job answer the same document, where a hash map's iteration order
//! differs per map instance and per process.
//!
//! The ordering is not something the codec can be told — it reads a map member's
//! entries in the order the map holds them — and the map type comes from the
//! declared Rust field (`#[smithy_macros::smithy(shape = ...)]` reads it off the
//! field, accepting either kind), so these two shapes are declared HERE, in a
//! file no model refresh rewrites, rather than beside the other shapes several
//! operations reach (`types::shared_types`, which is rendered from the model with
//! a hash map and is rewritten whenever the model is refreshed). Everything else
//! about them — the member names, the member order, the shape ids and the targets
//! — still comes from the model. `types/mod.rs` re-exports this module like every
//! other, so both shapes resolve unchanged wherever they are named.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-s3control-awss3controlservicev20180820.json");

/// What a `LambdaInvoke` job invokes, and the arguments it hands every task.
#[smithy_macros::smithy(shape = "com.amazonaws.s3control#LambdaInvokeOperation")]
#[derive(Debug, Clone, Default)]
pub struct LambdaInvokeOperation {
    pub function_arn: Option<String>,
    pub invocation_schema_version: Option<String>,
    pub user_arguments: Option<std::collections::BTreeMap<String, String>>,
}

/// The metadata a copy job writes on the objects it creates.
#[smithy_macros::smithy(shape = "com.amazonaws.s3control#S3ObjectMetadata")]
#[derive(Debug, Clone, Default)]
pub struct S3ObjectMetadata {
    pub cache_control: Option<String>,
    pub content_disposition: Option<String>,
    pub content_encoding: Option<String>,
    pub content_language: Option<String>,
    pub user_metadata: Option<std::collections::BTreeMap<String, String>>,
    pub content_length: Option<i64>,
    pub content_md5: Option<String>,
    pub content_type: Option<String>,
    pub http_expires_date: Option<::aws_smithy_types::DateTime>,
    pub requester_charged: bool,
    pub sse_algorithm: Option<String>,
}
