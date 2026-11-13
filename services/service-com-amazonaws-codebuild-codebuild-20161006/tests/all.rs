// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/absent_resource_preconditions.rs"]
mod absent_resource_preconditions;
#[path = "all/absent_resource_wording.rs"]
mod absent_resource_wording;
#[path = "all/batch_delete_builds.rs"]
mod batch_delete_builds;
#[path = "all/batch_get_reports.rs"]
mod batch_get_reports;
#[path = "all/build_artifacts_destination.rs"]
mod build_artifacts_destination;
#[path = "all/build_batch_fan_out.rs"]
mod build_batch_fan_out;
#[path = "all/build_batch_projection.rs"]
mod build_batch_projection;
#[path = "all/buildspec_execution.rs"]
mod buildspec_execution;
#[path = "all/buildspec_file_tree.rs"]
mod buildspec_file_tree;
#[path = "all/command_execution_lifecycle.rs"]
mod command_execution_lifecycle;
#[path = "all/command_substitution.rs"]
mod command_substitution;
#[path = "all/delete_build_batch.rs"]
mod delete_build_batch;
#[path = "all/delete_fleet.rs"]
mod delete_fleet;
#[path = "all/delete_project_webhook_disposition.rs"]
mod delete_project_webhook_disposition;
#[path = "all/delete_report_group.rs"]
mod delete_report_group;
#[path = "all/fleet_lifecycle.rs"]
mod fleet_lifecycle;
#[path = "all/identifier_shape_refusals.rs"]
mod identifier_shape_refusals;
#[path = "all/input_constraint_wording.rs"]
mod input_constraint_wording;
#[path = "all/list_build_batches.rs"]
mod list_build_batches;
#[path = "all/list_build_batches_for_project.rs"]
mod list_build_batches_for_project;
#[path = "all/list_builds.rs"]
mod list_builds;
#[path = "all/list_report_groups.rs"]
mod list_report_groups;
#[path = "all/list_sandboxes.rs"]
mod list_sandboxes;
#[path = "all/list_sandboxes_for_project.rs"]
mod list_sandboxes_for_project;
#[path = "all/list_sandboxes_for_project_paging.rs"]
mod list_sandboxes_for_project_paging;
#[path = "all/list_shared_projects.rs"]
mod list_shared_projects;
#[path = "all/list_shared_report_groups.rs"]
mod list_shared_report_groups;
#[path = "all/name_character_refusals.rs"]
mod name_character_refusals;
#[path = "all/project_build_batch_lifecycle.rs"]
mod project_build_batch_lifecycle;
#[path = "all/project_build_lifecycle.rs"]
mod project_build_lifecycle;
#[path = "all/project_effective_configuration.rs"]
mod project_effective_configuration;
#[path = "all/project_source_and_identity.rs"]
mod project_source_and_identity;
#[path = "all/project_visibility.rs"]
mod project_visibility;
#[path = "all/read_response_shape.rs"]
mod read_response_shape;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/report_group_lifecycle.rs"]
mod report_group_lifecycle;
#[path = "all/report_group_reads.rs"]
mod report_group_reads;
#[path = "all/report_group_trend.rs"]
mod report_group_trend;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/retry_build.rs"]
mod retry_build;
#[path = "all/runtime_versions.rs"]
mod runtime_versions;
#[path = "all/sandbox_connection.rs"]
mod sandbox_connection;
#[path = "all/sandbox_idle_timeout.rs"]
mod sandbox_idle_timeout;
#[path = "all/sandbox_lifecycle.rs"]
mod sandbox_lifecycle;
#[path = "all/sandbox_sessions.rs"]
mod sandbox_sessions;
#[path = "all/shell_probe.rs"]
mod shell_probe;
#[path = "all/source_credential_arn_shape.rs"]
mod source_credential_arn_shape;
#[path = "all/source_credentials.rs"]
mod source_credentials;
#[path = "all/start_build_batch_validation.rs"]
mod start_build_batch_validation;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_build_batch.rs"]
mod stop_build_batch;
#[path = "all/update_fleet.rs"]
mod update_fleet;
#[path = "all/update_project.rs"]
mod update_project;
#[path = "all/update_report_group.rs"]
mod update_report_group;
#[path = "all/webhook_absence_precedes_source_rules.rs"]
mod webhook_absence_precedes_source_rules;
#[path = "all/webhook_access_token.rs"]
mod webhook_access_token;
#[path = "all/webhook_delete_is_idempotent.rs"]
mod webhook_delete_is_idempotent;
#[path = "all/webhook_lifecycle.rs"]
mod webhook_lifecycle;
#[path = "all/webhook_project_resolution.rs"]
mod webhook_project_resolution;
#[path = "all/webhook_secret_is_minted_once.rs"]
mod webhook_secret_is_minted_once;
#[path = "all/buildspec_parse_refusal.rs"]
mod buildspec_parse_refusal;
