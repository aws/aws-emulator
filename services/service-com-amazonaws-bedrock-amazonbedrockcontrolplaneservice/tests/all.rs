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
#[path = "all/account_data_retention.rs"]
mod account_data_retention;
#[path = "all/advanced_prompt_optimization_jobs.rs"]
mod advanced_prompt_optimization_jobs;
#[path = "all/arn_inputs.rs"]
mod arn_inputs;
#[path = "all/automated_reasoning_policies.rs"]
mod automated_reasoning_policies;
#[path = "all/automated_reasoning_policy_annotations.rs"]
mod automated_reasoning_policy_annotations;
#[path = "all/automated_reasoning_policy_build_workflow_deletion.rs"]
mod automated_reasoning_policy_build_workflow_deletion;
#[path = "all/automated_reasoning_policy_build_workflow_progression.rs"]
mod automated_reasoning_policy_build_workflow_progression;
#[path = "all/automated_reasoning_policy_build_workflows.rs"]
mod automated_reasoning_policy_build_workflows;
#[path = "all/automated_reasoning_policy_definition_export.rs"]
mod automated_reasoning_policy_definition_export;
#[path = "all/automated_reasoning_policy_deletion.rs"]
mod automated_reasoning_policy_deletion;
#[path = "all/automated_reasoning_policy_listing.rs"]
mod automated_reasoning_policy_listing;
#[path = "all/automated_reasoning_policy_tagging.rs"]
mod automated_reasoning_policy_tagging;
#[path = "all/automated_reasoning_policy_test_case_deletion.rs"]
mod automated_reasoning_policy_test_case_deletion;
#[path = "all/automated_reasoning_policy_test_case_updates.rs"]
mod automated_reasoning_policy_test_case_updates;
#[path = "all/automated_reasoning_policy_test_cases.rs"]
mod automated_reasoning_policy_test_cases;
#[path = "all/automated_reasoning_policy_test_workflows.rs"]
mod automated_reasoning_policy_test_workflows;
#[path = "all/automated_reasoning_policy_updates.rs"]
mod automated_reasoning_policy_updates;
#[path = "all/automated_reasoning_policy_versions.rs"]
mod automated_reasoning_policy_versions;
#[path = "all/batch_delete_evaluation_jobs.rs"]
mod batch_delete_evaluation_jobs;
#[path = "all/batch_submission_acceptance.rs"]
mod batch_submission_acceptance;
#[path = "all/control_plane_authorization.rs"]
mod control_plane_authorization;
#[path = "all/custom_model_authorization.rs"]
mod custom_model_authorization;
#[path = "all/custom_model_deletion.rs"]
mod custom_model_deletion;
#[path = "all/custom_model_deployments.rs"]
mod custom_model_deployments;
#[path = "all/custom_model_source_bucket.rs"]
mod custom_model_source_bucket;
#[path = "all/custom_models.rs"]
mod custom_models;
#[path = "all/data_automation_tagging.rs"]
mod data_automation_tagging;
#[path = "all/default_prompt_routers.rs"]
mod default_prompt_routers;
#[path = "all/enforced_guardrail_configuration.rs"]
mod enforced_guardrail_configuration;
#[path = "all/enforced_guardrail_configuration_not_found_message.rs"]
mod enforced_guardrail_configuration_not_found_message;
#[path = "all/enforced_guardrail_inference.rs"]
mod enforced_guardrail_inference;
#[path = "all/enum_value_set_listing.rs"]
mod enum_value_set_listing;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/evaluation_job_completion.rs"]
mod evaluation_job_completion;
#[path = "all/evaluation_job_execution.rs"]
mod evaluation_job_execution;
#[path = "all/evaluation_job_listing.rs"]
mod evaluation_job_listing;
#[path = "all/evaluation_jobs.rs"]
mod evaluation_jobs;
#[path = "all/explicit_deny_names_the_addressed_guardrail.rs"]
mod explicit_deny_names_the_addressed_guardrail;
#[path = "all/foundation_model_agreement_offers.rs"]
mod foundation_model_agreement_offers;
#[path = "all/foundation_model_availability.rs"]
mod foundation_model_availability;
#[path = "all/foundation_model_capability_projection.rs"]
mod foundation_model_capability_projection;
#[path = "all/foundation_model_capability_records.rs"]
mod foundation_model_capability_records;
#[path = "all/foundation_model_catalogue_resolution.rs"]
mod foundation_model_catalogue_resolution;
#[path = "all/foundation_models.rs"]
mod foundation_models;
#[path = "all/get_foundation_model.rs"]
mod get_foundation_model;
#[path = "all/guardrail_not_found_message.rs"]
mod guardrail_not_found_message;
#[path = "all/guardrail_timestamp_wire_form.rs"]
mod guardrail_timestamp_wire_form;
#[path = "all/guardrail_wire_shape.rs"]
mod guardrail_wire_shape;
#[path = "all/guardrails.rs"]
mod guardrails;
#[path = "all/id_addressed_resource_authorization.rs"]
mod id_addressed_resource_authorization;
#[path = "all/imported_models.rs"]
mod imported_models;
#[path = "all/inference_profile_listing.rs"]
mod inference_profile_listing;
#[path = "all/inference_profiles.rs"]
mod inference_profiles;
#[path = "all/job_pass_role_source_arn.rs"]
mod job_pass_role_source_arn;
#[path = "all/marketplace_endpoint_deregistration.rs"]
mod marketplace_endpoint_deregistration;
#[path = "all/marketplace_endpoint_registration.rs"]
mod marketplace_endpoint_registration;
#[path = "all/marketplace_model_endpoints.rs"]
mod marketplace_model_endpoints;
#[path = "all/model_copy_jobs.rs"]
mod model_copy_jobs;
#[path = "all/model_customization_jobs.rs"]
mod model_customization_jobs;
#[path = "all/model_customization_pass_role.rs"]
mod model_customization_pass_role;
#[path = "all/model_import_jobs.rs"]
mod model_import_jobs;
#[path = "all/model_invocation_job_elapsed_time.rs"]
mod model_invocation_job_elapsed_time;
#[path = "all/model_invocation_job_execution.rs"]
mod model_invocation_job_execution;
#[path = "all/model_invocation_job_poll_interval.rs"]
mod model_invocation_job_poll_interval;
#[path = "all/model_invocation_job_terminal_status.rs"]
mod model_invocation_job_terminal_status;
#[path = "all/model_invocation_jobs.rs"]
mod model_invocation_jobs;
#[path = "all/model_invocation_logging.rs"]
mod model_invocation_logging;
#[path = "all/model_invocation_logging_pass_role_boundary.rs"]
mod model_invocation_logging_pass_role_boundary;
#[path = "all/path_label_values_are_not_echoed.rs"]
mod path_label_values_are_not_echoed;
#[path = "all/prompt_routers.rs"]
mod prompt_routers;
#[path = "all/published_catalogues.rs"]
mod published_catalogues;
#[path = "all/published_routing_metadata.rs"]
mod published_routing_metadata;
#[path = "all/range_refusal_wording.rs"]
mod range_refusal_wording;
#[path = "all/refusal_message_fidelity.rs"]
mod refusal_message_fidelity;
#[path = "all/refused_subject_follows_the_member_shape.rs"]
mod refused_subject_follows_the_member_shape;
#[path = "all/refused_value_is_quoted_by_binding.rs"]
mod refused_value_is_quoted_by_binding;
#[path = "all/provisioned_model_throughputs.rs"]
mod provisioned_model_throughputs;
#[path = "all/provisioned_model_units_wire_form.rs"]
mod provisioned_model_units_wire_form;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_policy_empty_document.rs"]
mod resource_policy_empty_document;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_quota_across_resource_families.rs"]
mod tag_quota_across_resource_families;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/use_case_for_model_access.rs"]
mod use_case_for_model_access;
#[path = "all/unresolved_model_identifier.rs"]
mod unresolved_model_identifier;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
