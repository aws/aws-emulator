// SPDX-License-Identifier: CC0-1.0

//! The shapes `describe_application_provider` reaches, as the upstream model declares
//! them.
//!
//! Every shape the operation reaches except its output, which this service answers with
//! the `ApplicationProvider` member the upstream model declares nowhere and which is
//! therefore declared in [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-ssoadmin-swbexternalservice.json");

#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#DescribeApplicationProviderRequest")]
#[derive(Debug, Clone, Default)]
pub struct DescribeApplicationProviderInput {
    pub application_provider_arn: String,
}
