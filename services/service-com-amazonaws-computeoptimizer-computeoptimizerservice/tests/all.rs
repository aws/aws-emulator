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
#[path = "all/analysis_due_export_agreement.rs"]
mod analysis_due_export_agreement;
#[path = "all/analysis_due_gate.rs"]
mod analysis_due_gate;
#[path = "all/auto_scaling_group_recommendations.rs"]
mod auto_scaling_group_recommendations;
#[path = "all/auto_scaling_group_recommendations_export.rs"]
mod auto_scaling_group_recommendations_export;
#[path = "all/auto_scaling_group_recommendations_export_file.rs"]
mod auto_scaling_group_recommendations_export_file;
#[path = "all/classified_resource_type_summaries.rs"]
mod classified_resource_type_summaries;
#[path = "all/client_refusal_wording.rs"]
mod client_refusal_wording;
#[path = "all/cross_account_requests.rs"]
mod cross_account_requests;
#[path = "all/cross_account_resource_arns.rs"]
mod cross_account_resource_arns;
#[path = "all/ebs_volume_recommendations.rs"]
mod ebs_volume_recommendations;
#[path = "all/ebs_volume_recommendations_export.rs"]
mod ebs_volume_recommendations_export;
#[path = "all/ec2_instance_recommendations.rs"]
mod ec2_instance_recommendations;
#[path = "all/ec2_instance_recommendations_export.rs"]
mod ec2_instance_recommendations_export;
#[path = "all/ec2_instance_recommendations_export_file.rs"]
mod ec2_instance_recommendations_export_file;
#[path = "all/ec2_recommendation_projected_metrics.rs"]
mod ec2_recommendation_projected_metrics;
#[path = "all/ecs_service_recommendation_projected_metrics.rs"]
mod ecs_service_recommendation_projected_metrics;
#[path = "all/ecs_service_recommendations.rs"]
mod ecs_service_recommendations;
#[path = "all/ecs_service_recommendations_export.rs"]
mod ecs_service_recommendations_export;
#[path = "all/effective_preferred_resource_candidates.rs"]
mod effective_preferred_resource_candidates;
#[path = "all/effective_status_preference_is_always_reported.rs"]
mod effective_status_preference_is_always_reported;
#[path = "all/enrollment_and_preferences.rs"]
mod enrollment_and_preferences;
#[path = "all/export_preference_gated_columns.rs"]
mod export_preference_gated_columns;
#[path = "all/export_recommendation_preferences.rs"]
mod export_recommendation_preferences;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/idle_recommendation_summaries.rs"]
mod idle_recommendation_summaries;
#[path = "all/idle_recommendations.rs"]
mod idle_recommendations;
#[path = "all/idle_recommendations_export.rs"]
mod idle_recommendations_export;
#[path = "all/include_member_accounts.rs"]
mod include_member_accounts;
#[path = "all/instance_arn_refusal_wording.rs"]
mod instance_arn_refusal_wording;
#[path = "all/lambda_function_recommendations.rs"]
mod lambda_function_recommendations;
#[path = "all/lambda_function_recommendations_export.rs"]
mod lambda_function_recommendations_export;
#[path = "all/license_recommendations.rs"]
mod license_recommendations;
#[path = "all/license_recommendations_export.rs"]
mod license_recommendations_export;
#[path = "all/modelled_constraint_refusal_class.rs"]
mod modelled_constraint_refusal_class;
#[path = "all/organization_enrollment_statuses.rs"]
mod organization_enrollment_statuses;
#[path = "all/preference_records_hold_a_preference.rs"]
mod preference_records_hold_a_preference;
#[path = "all/preference_resource_type_support.rs"]
mod preference_resource_type_support;
#[path = "all/preference_support_refusal_wording.rs"]
mod preference_support_refusal_wording;
#[path = "all/rds_database_inventory_analysis.rs"]
mod rds_database_inventory_analysis;
#[path = "all/rds_database_recommendation_options.rs"]
mod rds_database_recommendation_options;
#[path = "all/rds_database_recommendation_projected_metrics.rs"]
mod rds_database_recommendation_projected_metrics;
#[path = "all/rds_database_recommendations.rs"]
mod rds_database_recommendations;
#[path = "all/rds_database_recommendations_export.rs"]
mod rds_database_recommendations_export;
#[path = "all/rds_database_recommendations_export_file.rs"]
mod rds_database_recommendations_export_file;
#[path = "all/recommendation_export_admission.rs"]
mod recommendation_export_admission;
#[path = "all/recommendation_export_jobs.rs"]
mod recommendation_export_jobs;
#[path = "all/recommendation_export_objects.rs"]
mod recommendation_export_objects;
#[path = "all/recommendation_summaries.rs"]
mod recommendation_summaries;
#[path = "all/reported_defaults_and_preference_validation.rs"]
mod reported_defaults_and_preference_validation;
#[path = "all/shared_paging_rules.rs"]
mod shared_paging_rules;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/summary_block_sets.rs"]
mod summary_block_sets;
#[path = "all/summary_resource_type_surface.rs"]
mod summary_resource_type_surface;
#[path = "all/timestamp_probe.rs"]
mod timestamp_probe;
#[path = "all/tracked_resource_type_summaries.rs"]
mod tracked_resource_type_summaries;
#[path = "all/unanalysed_resources_are_empty_results.rs"]
mod unanalysed_resources_are_empty_results;
#[path = "all/unrestricted_preference_resource_types.rs"]
mod unrestricted_preference_resource_types;
