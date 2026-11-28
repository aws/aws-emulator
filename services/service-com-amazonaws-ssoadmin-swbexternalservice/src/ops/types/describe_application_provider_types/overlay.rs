// SPDX-License-Identifier: CC0-1.0

//! `DescribeApplicationProviderOutput`, declared from this crate's overlay model.
//!
//! AWS answers this read with the whole application-provider catalog entry — the same
//! `ApplicationProvider` structure `ListApplicationProviders` reports — as a member beside
//! the flat members the upstream model declares. The model snapshot is kept exactly as the
//! refresh wrote it (see `smithy-model/LOCAL-ADDITIONS.md`), so this one shape is generated
//! from the small overlay model `smithy-model/local-additions.json`, which restates it with
//! that member against the snapshot's own member targets, the modelled members keeping the
//! model's own order and the local one stated LAST. Nothing about the wire is hand-written:
//! the shape is still declared by `#[smithy_macros::smithy]` from a model.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The application provider a `DescribeApplicationProvider` reports.
///
/// `application_provider` is the catalog entry itself, the one projection
/// `ListApplicationProviders` serves for it, so the flat members and the entry cannot
/// disagree about one provider.
#[smithy_macros::smithy(shape = "com.amazonaws.ssoadmin#DescribeApplicationProviderResponse")]
#[derive(Debug, Clone, Default)]
pub struct DescribeApplicationProviderOutput {
    pub application_provider_arn: String,
    pub federation_protocol: Option<String>,
    pub display_data: Option<DisplayData>,
    pub resource_server_config: Option<ResourceServerConfig>,
    pub application_provider: Option<ApplicationProvider>,
}
