// SPDX-License-Identifier: CC0-1.0

//! The shapes `describe_instance` reaches, as the upstream model declares them.
//!
//! Every shape the operation reaches except its output, which this service answers with the
//! `Instance` member the upstream model declares nowhere and which is therefore declared in
//! [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#DescribeInstanceRequest")]
#[derive(Debug, Clone, Default)]
pub struct DescribeInstanceInput {
    pub instance_arn: String,
}

#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#EncryptionConfigurationDetails")]
#[derive(Debug, Clone, Default)]
pub struct EncryptionConfigurationDetails {
    pub key_type: Option<String>,
    pub kms_key_arn: Option<String>,
    pub encryption_status: Option<String>,
    pub encryption_status_reason: Option<String>,
}
