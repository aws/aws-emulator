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
#[path = "all/admission_bounds.rs"]
mod admission_bounds;
#[path = "all/agentic_rft_job_lifecycle.rs"]
mod agentic_rft_job_lifecycle;
#[path = "all/ai_benchmark_job_lifecycle.rs"]
mod ai_benchmark_job_lifecycle;
#[path = "all/ai_benchmark_job_release.rs"]
mod ai_benchmark_job_release;
#[path = "all/ai_recommendation_job_lifecycle.rs"]
mod ai_recommendation_job_lifecycle;
#[path = "all/ai_recommendation_job_stop.rs"]
mod ai_recommendation_job_stop;
#[path = "all/ai_workload_config_lifecycle.rs"]
mod ai_workload_config_lifecycle;
#[path = "all/algorithm_lifecycle.rs"]
mod algorithm_lifecycle;
#[path = "all/app_image_config_registry.rs"]
mod app_image_config_registry;
#[path = "all/artifact_deletion.rs"]
mod artifact_deletion;
#[path = "all/artifact_lineage.rs"]
mod artifact_lineage;
#[path = "all/artifact_update_visibility.rs"]
mod artifact_update_visibility;
#[path = "all/artifact_updates.rs"]
mod artifact_updates;
#[path = "all/automl_job_v2_execution.rs"]
mod automl_job_v2_execution;
#[path = "all/automl_job_v2_lifecycle.rs"]
mod automl_job_v2_lifecycle;
#[path = "all/batch_describe_model_packages.rs"]
mod batch_describe_model_packages;
#[path = "all/batch_transform_jobs.rs"]
mod batch_transform_jobs;
#[path = "all/cluster_scheduler_config_lifecycle.rs"]
mod cluster_scheduler_config_lifecycle;
#[path = "all/code_repositories.rs"]
mod code_repositories;
#[path = "all/code_repository_deletion.rs"]
mod code_repository_deletion;
#[path = "all/code_repository_updates.rs"]
mod code_repository_updates;
#[path = "all/compute_quota_lifecycle.rs"]
mod compute_quota_lifecycle;
#[path = "all/control_plane_preconditions.rs"]
mod control_plane_preconditions;
#[path = "all/create_admission.rs"]
mod create_admission;
#[path = "all/create_refusal_message_fidelity.rs"]
mod create_refusal_message_fidelity;
#[path = "all/custom_image_deletion.rs"]
mod custom_image_deletion;
#[path = "all/custom_image_version_deletion.rs"]
mod custom_image_version_deletion;
#[path = "all/custom_images.rs"]
mod custom_images;
#[path = "all/default_lineage_group_identity.rs"]
mod default_lineage_group_identity;
#[path = "all/delete_and_stop_preconditions.rs"]
mod delete_and_stop_preconditions;
#[path = "all/endpoint_config_deletion.rs"]
mod endpoint_config_deletion;
#[path = "all/endpoint_hosting_quota.rs"]
mod endpoint_hosting_quota;
#[path = "all/execution_role_admission.rs"]
mod execution_role_admission;
#[path = "all/experiment_deletion.rs"]
mod experiment_deletion;
#[path = "all/experiment_lifecycle.rs"]
mod experiment_lifecycle;
#[path = "all/experiment_not_found_refusal.rs"]
mod experiment_not_found_refusal;
#[path = "all/experiment_tracking_updates.rs"]
mod experiment_tracking_updates;
#[path = "all/feature_group_deletion.rs"]
mod feature_group_deletion;
#[path = "all/feature_group_lifecycle.rs"]
mod feature_group_lifecycle;
#[path = "all/feature_group_updates.rs"]
mod feature_group_updates;
#[path = "all/hub_content_references.rs"]
mod hub_content_references;
#[path = "all/hub_contents_listing.rs"]
mod hub_contents_listing;
#[path = "all/hub_registry.rs"]
mod hub_registry;
#[path = "all/hyperparameter_tuning_job.rs"]
mod hyperparameter_tuning_job;
#[path = "all/hyperpod_cluster_audit_record.rs"]
mod hyperpod_cluster_audit_record;
#[path = "all/hyperpod_cluster_listing.rs"]
mod hyperpod_cluster_listing;
#[path = "all/inference_deployment_lifecycle.rs"]
mod inference_deployment_lifecycle;
#[path = "all/inference_recommendation_refusals.rs"]
mod inference_recommendation_refusals;
#[path = "all/inference_recommendations_jobs.rs"]
mod inference_recommendations_jobs;
#[path = "all/job_experiment_tracking.rs"]
mod job_experiment_tracking;
#[path = "all/lineage_action_lifecycle.rs"]
mod lineage_action_lifecycle;
#[path = "all/lineage_and_config_updates.rs"]
mod lineage_and_config_updates;
#[path = "all/lineage_associations.rs"]
mod lineage_associations;
#[path = "all/lineage_context_lifecycle.rs"]
mod lineage_context_lifecycle;
#[path = "all/lineage_entity_association_guard.rs"]
mod lineage_entity_association_guard;
#[path = "all/lineage_group_policy.rs"]
mod lineage_group_policy;
#[path = "all/lineage_read_authorization.rs"]
mod lineage_read_authorization;
#[path = "all/lineage_group_reporting.rs"]
mod lineage_group_reporting;
#[path = "all/lineage_groups.rs"]
mod lineage_groups;
#[path = "all/list_artifacts.rs"]
mod list_artifacts;
#[path = "all/list_associations.rs"]
mod list_associations;
#[path = "all/list_code_repositories.rs"]
mod list_code_repositories;
#[path = "all/list_endpoints.rs"]
mod list_endpoints;
#[path = "all/list_images.rs"]
mod list_images;
#[path = "all/list_model_card_export_jobs.rs"]
mod list_model_card_export_jobs;
#[path = "all/list_model_package_groups.rs"]
mod list_model_package_groups;
#[path = "all/list_model_packages.rs"]
mod list_model_packages;
#[path = "all/list_pipeline_execution_steps.rs"]
mod list_pipeline_execution_steps;
#[path = "all/list_pipelines.rs"]
mod list_pipelines;
#[path = "all/list_training_jobs.rs"]
mod list_training_jobs;
#[path = "all/list_transform_jobs.rs"]
mod list_transform_jobs;
#[path = "all/list_trial_components.rs"]
mod list_trial_components;
#[path = "all/mlflow_app_lifecycle.rs"]
mod mlflow_app_lifecycle;
#[path = "all/mlflow_app_presigned_url.rs"]
mod mlflow_app_presigned_url;
#[path = "all/mlflow_tracking_server.rs"]
mod mlflow_tracking_server;
#[path = "all/model_artifact_admission.rs"]
mod model_artifact_admission;
#[path = "all/model_card_deletion.rs"]
mod model_card_deletion;
#[path = "all/model_card_export_jobs.rs"]
mod model_card_export_jobs;
#[path = "all/model_card_governance.rs"]
mod model_card_governance;
#[path = "all/model_card_updates.rs"]
mod model_card_updates;
#[path = "all/model_package_admission.rs"]
mod model_package_admission;
#[path = "all/model_package_group_lifecycle.rs"]
mod model_package_group_lifecycle;
#[path = "all/model_package_group_policy.rs"]
mod model_package_group_policy;
#[path = "all/model_package_group_policy_names.rs"]
mod model_package_group_policy_names;
#[path = "all/model_package_pending_window.rs"]
mod model_package_pending_window;
#[path = "all/model_package_registry.rs"]
mod model_package_registry;
#[path = "all/model_package_updates.rs"]
mod model_package_updates;
#[path = "all/not_found_refusal_wording.rs"]
mod not_found_refusal_wording;
#[path = "all/notebook_instance_lifecycle.rs"]
mod notebook_instance_lifecycle;
#[path = "all/notebook_instance_deletion.rs"]
mod notebook_instance_deletion;
#[path = "all/notebook_instance_stop.rs"]
mod notebook_instance_stop;
#[path = "all/notebook_lifecycle_config.rs"]
mod notebook_lifecycle_config;
#[path = "all/not_found_message_fidelity.rs"]
mod not_found_message_fidelity;
#[path = "all/partner_app_creation.rs"]
mod partner_app_creation;
#[path = "all/partner_apps.rs"]
mod partner_apps;
#[path = "all/pipeline_definition_diagnosis.rs"]
mod pipeline_definition_diagnosis;
#[path = "all/pipeline_execution.rs"]
mod pipeline_execution;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/pipeline_role_resolution.rs"]
mod pipeline_role_resolution;
#[path = "all/presigned_notebook_instance_url.rs"]
mod presigned_notebook_instance_url;
#[path = "all/processing_job_lifecycle.rs"]
mod processing_job_lifecycle;
#[path = "all/processing_job_not_found_refusals.rs"]
mod processing_job_not_found_refusals;
#[path = "all/processing_job_stop.rs"]
mod processing_job_stop;
#[path = "all/processing_job_stop_absent_target.rs"]
mod processing_job_stop_absent_target;
#[path = "all/read_responses_report_what_a_resource_holds.rs"]
mod read_responses_report_what_a_resource_holds;
#[path = "all/refusal_subject_wording.rs"]
mod refusal_subject_wording;
#[path = "all/rejection_fidelity.rs"]
mod rejection_fidelity;
#[path = "all/resource_precondition_gates.rs"]
mod resource_precondition_gates;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/search_resources.rs"]
mod search_resources;
#[path = "all/search_suggestions.rs"]
mod search_suggestions;
#[path = "all/servicecatalog_portfolio_status.rs"]
mod servicecatalog_portfolio_status;
#[path = "all/service_resolved_members.rs"]
mod service_resolved_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/studio_app_lifecycle.rs"]
mod studio_app_lifecycle;
#[path = "all/studio_domain_registry.rs"]
mod studio_domain_registry;
#[path = "all/training_job_admission.rs"]
mod training_job_admission;
#[path = "all/training_job_algorithm_reference.rs"]
mod training_job_algorithm_reference;
#[path = "all/training_job_constraint_admission.rs"]
mod training_job_constraint_admission;
#[path = "all/training_job_execution.rs"]
mod training_job_execution;
#[path = "all/training_job_hyperparameters.rs"]
mod training_job_hyperparameters;
#[path = "all/training_job_phase_progression.rs"]
mod training_job_phase_progression;
#[path = "all/training_job_stop.rs"]
mod training_job_stop;
#[path = "all/trial_component_association.rs"]
mod trial_component_association;
#[path = "all/trial_component_deletion.rs"]
mod trial_component_deletion;
#[path = "all/trial_component_disassociation.rs"]
mod trial_component_disassociation;
#[path = "all/trial_component_not_found_refusal.rs"]
mod trial_component_not_found_refusal;
#[path = "all/trial_component_tracking.rs"]
mod trial_component_tracking;
#[path = "all/trial_deletion.rs"]
mod trial_deletion;
#[path = "all/trial_not_found_refusal.rs"]
mod trial_not_found_refusal;
#[path = "all/update_action_after_deletion.rs"]
mod update_action_after_deletion;
// Declared after the sorted list rather than among the other `hyperpod_*` suites: the
// cluster operations are being implemented one issue at a time, so that run of lines is
// where every concurrent cluster change inserts its own suite.
#[path = "all/hyperpod_cluster_lifecycle.rs"]
mod hyperpod_cluster_lifecycle;
#[path = "all/create_cluster_admission.rs"]
mod create_cluster_admission;
#[path = "all/hyperpod_cluster_node_listing.rs"]
mod hyperpod_cluster_node_listing;
#[path = "all/hyperpod_cluster_deletion.rs"]
mod hyperpod_cluster_deletion;
#[path = "all/hyperpod_cluster_node_replacement.rs"]
mod hyperpod_cluster_node_replacement;
#[path = "all/hyperpod_cluster_node_deletion.rs"]
mod hyperpod_cluster_node_deletion;
#[path = "all/hyperpod_cluster_node_reboot.rs"]
mod hyperpod_cluster_node_reboot;
#[path = "all/hyperpod_node_identity_reporting.rs"]
mod hyperpod_node_identity_reporting;
#[path = "all/hyperpod_slurm_node_convergence.rs"]
mod hyperpod_slurm_node_convergence;
#[path = "all/notebook_lifecycle_hook_execution.rs"]
mod notebook_lifecycle_hook_execution;
