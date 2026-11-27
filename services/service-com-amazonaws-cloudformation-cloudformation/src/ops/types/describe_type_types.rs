// SPDX-License-Identifier: CC0-1.0

//! Type definitions reached only by the `describe_type` operation.
//!
//! This file is HAND-WRITTEN at a generated path, and the `@generated` marker is absent on
//! purpose: AWS answers this read with one element the published model snapshot does not
//! declare, and that snapshot is kept exactly as shipped (see
//! `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `SchemaHandlerPermissions` — the permissions the handlers declared in the described
//!   version's schema require. The element is on every recorded read of an extension, empty
//!   for one whose schema declares no handler permission.
//!
//! So the output shape is generated, like every other modelled shape, from a model file —
//! the small overlay `smithy-model/local-additions.json`, which restates the snapshot's own
//! members against the snapshot's own targets (so every length, pattern and enum constraint
//! is captured exactly as the snapshot spells it) and adds the one element besides. The
//! INPUT shape and `RequiredActivatedType` are unchanged, so they are still generated from
//! the snapshot: the overlay is named on the one attribute that needs it. Nothing about the
//! wire is hand-written — the member names, the shape ids and the constraints all come from
//! a model.
//!
//! `tests/all/registry_and_scan_read_members.rs` pins the element on the wire.
//!
//! Each structure/union is declared exactly once across this service's `types/`
//! modules, and `types/mod.rs` re-exports them all.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-cloudformation-cloudformation.json");

// `skip_validation = ["PublisherId"]`: the same rule `ActivateTypeInput` records — the
// modelled `PublisherId` form (`@pattern("^[0-9a-zA-Z]{12,40}$")` within `@length(1..40)`) is
// the form of an id CloudFormation MINTS for a publisher that registers, while the publisher
// id Amazon Web Services' own published extensions are named by is outside it
// (`crate::state::registry::AWS_HOOKS_PUBLISHER_ID`) — and a read naming that publisher is
// served, so refusing it at the boundary would make the whole Amazon-published Hook surface
// unreachable, under an error class this operation does not even model. The member is held to
// the modelled form in the handler instead, in the boundary's own words
// (`crate::state::registry::publisher_id_refusal`), which is the only place that can tell the
// one publisher from the other.
#[smithy_macros::smithy(
    shape = "com.amazonaws.cloudformation#DescribeTypeInput",
    skip_validation = ["PublisherId"]
)]
#[derive(Debug, Clone, Default)]
pub struct DescribeTypeInput {
    pub r#type: Option<String>,
    pub type_name: Option<String>,
    pub arn: Option<String>,
    pub version_id: Option<String>,
    pub publisher_id: Option<String>,
    pub public_version_number: Option<String>,
}

#[smithy_macros::smithy(
    shape = "com.amazonaws.cloudformation#DescribeTypeOutput",
    model = "smithy-model/local-additions.json"
)]
#[derive(Debug, Clone, Default)]
pub struct DescribeTypeOutput {
    pub arn: Option<String>,
    pub r#type: Option<String>,
    pub type_name: Option<String>,
    pub default_version_id: Option<String>,
    pub is_default_version: Option<bool>,
    pub type_tests_status: Option<String>,
    pub type_tests_status_description: Option<String>,
    pub description: Option<String>,
    pub schema: Option<String>,
    pub provisioning_type: Option<String>,
    pub deprecated_status: Option<String>,
    pub logging_config: Option<LoggingConfig>,
    pub required_activated_types: Option<Vec<RequiredActivatedType>>,
    pub execution_role_arn: Option<String>,
    pub visibility: Option<String>,
    pub source_url: Option<String>,
    pub documentation_url: Option<String>,
    pub last_updated: Option<::aws_smithy_types::DateTime>,
    pub time_created: Option<::aws_smithy_types::DateTime>,
    pub configuration_schema: Option<String>,
    pub publisher_id: Option<String>,
    pub original_type_name: Option<String>,
    pub original_type_arn: Option<String>,
    pub public_version_number: Option<String>,
    pub latest_public_version: Option<String>,
    pub is_activated: Option<bool>,
    pub auto_update: Option<bool>,
    /// The one element the snapshot does not declare, which the overlay appends after the
    /// modelled ones: the permissions the described version's schema requires of its
    /// handlers, read out of that schema
    /// ([`crate::state::resource_schema::schema_handler_permissions`]).
    pub schema_handler_permissions: Option<Vec<String>>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.cloudformation#RequiredActivatedType")]
#[derive(Debug, Clone, Default)]
pub struct RequiredActivatedType {
    pub type_name_alias: Option<String>,
    pub original_type_name: Option<String>,
    pub publisher_id: Option<String>,
    pub supported_major_versions: Option<Vec<i32>>,
}
