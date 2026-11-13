// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/access_grant_usage_limit.rs"]
mod access_grant_usage_limit;
#[path = "all/access_grants_arns.rs"]
mod access_grants_arns;
#[path = "all/access_grants_data_plane.rs"]
mod access_grants_data_plane;
#[path = "all/access_grants_instance.rs"]
mod access_grants_instance;
#[path = "all/access_grants_instance_resource_policy.rs"]
mod access_grants_instance_resource_policy;
#[path = "all/access_grants_listings.rs"]
mod access_grants_listings;
#[path = "all/access_grants_location_delete.rs"]
mod access_grants_location_delete;
#[path = "all/access_grants_location_register.rs"]
mod access_grants_location_register;
#[path = "all/access_grants_location_update.rs"]
mod access_grants_location_update;
#[path = "all/access_grants_refusal_identities.rs"]
mod access_grants_refusal_identities;
#[path = "all/access_point_create_referential_integrity.rs"]
mod access_point_create_referential_integrity;
#[path = "all/access_point_lifecycle.rs"]
mod access_point_lifecycle;
#[path = "all/access_point_policy_delete.rs"]
mod access_point_policy_delete;
#[path = "all/access_point_policy_grammar.rs"]
mod access_point_policy_grammar;
#[path = "all/access_point_public_access_block_default.rs"]
mod access_point_public_access_block_default;
#[path = "all/access_point_reads_and_refusals.rs"]
mod access_point_reads_and_refusals;
#[path = "all/account_authorization.rs"]
mod account_authorization;
#[path = "all/account_public_access_block_bucket_policy.rs"]
mod account_public_access_block_bucket_policy;
#[path = "all/batch_job_absent_refusal.rs"]
mod batch_job_absent_refusal;
#[path = "all/batch_job_execution.rs"]
mod batch_job_execution;
#[path = "all/batch_job_lifecycle.rs"]
mod batch_job_lifecycle;
#[path = "all/batch_job_list.rs"]
mod batch_job_list;
#[path = "all/batch_job_manifest_addressing.rs"]
mod batch_job_manifest_addressing;
#[path = "all/batch_job_priority.rs"]
mod batch_job_priority;
#[path = "all/batch_job_refusal_words.rs"]
mod batch_job_refusal_words;
#[path = "all/batch_job_role_trust.rs"]
mod batch_job_role_trust;
#[path = "all/batch_job_status.rs"]
mod batch_job_status;
#[path = "all/batch_job_tagging.rs"]
mod batch_job_tagging;
#[path = "all/batch_job_terminal_status_and_report.rs"]
mod batch_job_terminal_status_and_report;
#[path = "all/batch_job_unconfirmed_run.rs"]
mod batch_job_unconfirmed_run;
#[path = "all/empty_body_success_status.rs"]
mod empty_body_success_status;
#[path = "all/multi_region_access_point_create_outcomes.rs"]
mod multi_region_access_point_create_outcomes;
#[path = "all/multi_region_access_point_lifecycle.rs"]
mod multi_region_access_point_lifecycle;
#[path = "all/multi_region_access_point_member_regions.rs"]
mod multi_region_access_point_member_regions;
#[path = "all/multi_region_access_point_route_submit.rs"]
mod multi_region_access_point_route_submit;
#[path = "all/multi_region_access_point_sub_resources.rs"]
mod multi_region_access_point_sub_resources;
#[path = "all/object_store.rs"]
mod object_store;
#[path = "all/public_access_block.rs"]
mod public_access_block;
#[path = "all/refusal_resource_members.rs"]
mod refusal_resource_members;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_lens_configuration.rs"]
mod storage_lens_configuration;
#[path = "all/storage_lens_configuration_delete.rs"]
mod storage_lens_configuration_delete;
#[path = "all/storage_lens_configuration_envelope.rs"]
mod storage_lens_configuration_envelope;
#[path = "all/storage_lens_configuration_list.rs"]
mod storage_lens_configuration_list;
#[path = "all/storage_lens_configuration_tagging_delete.rs"]
mod storage_lens_configuration_tagging_delete;
#[path = "all/storage_lens_group.rs"]
mod storage_lens_group;
#[path = "all/storage_lens_group_absent_bounds.rs"]
mod storage_lens_group_absent_bounds;
#[path = "all/storage_lens_group_delete.rs"]
mod storage_lens_group_delete;
#[path = "all/storage_lens_group_update.rs"]
mod storage_lens_group_update;
