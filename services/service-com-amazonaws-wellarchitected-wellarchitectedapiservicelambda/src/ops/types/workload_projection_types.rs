// SPDX-License-Identifier: CC0-1.0

//! `Workload`: the projection of a stored workload that `GetWorkload`, `UpdateWorkload`
//! and the `Milestone` of `GetMilestone` all answer with.
//!
//! This is the ONE workload projection of that shape this crate has, so what it declares
//! is what every workload read of this service reports.
//!
//! It is declared here rather than beside the other modelled shapes because AWS answers
//! those reads with one member the upstream model does not declare, and that model is kept
//! exactly as the update wrote it (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `FTRSupported` — whether the workload is supported for a Foundational Technical
//!   Review, emitted as `false` for a workload that is not. The model declares neither the
//!   member nor a shape of that name.
//!
//! So the type is generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the members the model
//! declares, with their own targets so every length, pattern, range and enum constraint is
//! captured exactly as the model spells it, and adds the one it lacks. Nothing about the
//! wire is hand-written: the member names, the shape ids and the constraints all come from
//! that file, and the epoch-second form this service's timestamps take comes from the
//! protocol it serves (`crate::service`), not from a schema written out here.
//!
//! `types/mod.rs` re-exports this `Workload` EXPLICITLY, so it is the one every
//! `use super::types::*` resolves: the generated `shared_types` declares the modelled
//! shape of the same name, and an explicit re-export takes precedence over the glob that
//! would otherwise make the name ambiguous.
//!
//! `tests/all/workload_ftr_support.rs` pins the member on the wire for every read that
//! projects a workload.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

#[smithy_macros::smithy(shape = "com.amazonaws.wellarchitected#Workload")]
#[derive(Debug, Clone, Default)]
pub struct Workload {
    /// The member the upstream model does not declare. Reported rather than omitted, so a
    /// caller reading it can tell "not supported" from "unknown"
    /// ([`crate::state::WorkloadRecord::to_workload`] decides the value).
    pub ftr_supported: Option<bool>,
    pub workload_id: Option<String>,
    pub workload_arn: Option<String>,
    pub workload_name: Option<String>,
    pub description: Option<String>,
    pub environment: Option<String>,
    pub updated_at: Option<::aws_smithy_types::DateTime>,
    pub account_ids: Option<Vec<String>>,
    pub aws_regions: Option<Vec<String>>,
    pub non_aws_regions: Option<Vec<String>>,
    pub architectural_design: Option<String>,
    pub review_owner: Option<String>,
    pub review_restriction_date: Option<::aws_smithy_types::DateTime>,
    pub is_review_owner_update_acknowledged: Option<bool>,
    pub industry_type: Option<String>,
    pub industry: Option<String>,
    pub notes: Option<String>,
    pub improvement_status: Option<String>,
    pub risk_counts: Option<std::collections::HashMap<String, i32>>,
    pub pillar_priorities: Option<Vec<String>>,
    pub lenses: Option<Vec<String>>,
    pub owner: Option<String>,
    pub share_invitation_id: Option<String>,
    pub tags: Option<std::collections::HashMap<String, String>>,
    pub discovery_config: Option<WorkloadDiscoveryConfig>,
    pub applications: Option<Vec<String>>,
    pub profiles: Option<Vec<WorkloadProfile>>,
    pub prioritized_risk_counts: Option<std::collections::HashMap<String, i32>>,
    pub jira_configuration: Option<WorkloadJiraConfigurationOutput>,
}
