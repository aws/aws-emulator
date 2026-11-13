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
#[path = "all/account_limits.rs"]
mod account_limits;
#[path = "all/agentcore_stack_outputs.rs"]
mod agentcore_stack_outputs;
#[path = "all/appconfig_estate_provisioning.rs"]
mod appconfig_estate_provisioning;
#[path = "all/appconfig_resource_types.rs"]
mod appconfig_resource_types;
#[path = "all/apigateway_vpc_link_resources.rs"]
mod apigateway_vpc_link_resources;
#[path = "all/application_auto_scaling_resources.rs"]
mod application_auto_scaling_resources;
#[path = "all/bedrock_knowledge_base_identity.rs"]
mod bedrock_knowledge_base_identity;
#[path = "all/braket_spending_limit_resource_type.rs"]
mod braket_spending_limit_resource_type;
#[path = "all/bucket_analytics_configurations.rs"]
mod bucket_analytics_configurations;
#[path = "all/bucket_default_encryption.rs"]
mod bucket_default_encryption;
#[path = "all/bucket_intelligent_tiering_configurations.rs"]
mod bucket_intelligent_tiering_configurations;
#[path = "all/bucket_inventory_configurations.rs"]
mod bucket_inventory_configurations;
#[path = "all/bucket_lifecycle_tag_filters.rs"]
mod bucket_lifecycle_tag_filters;
#[path = "all/bucket_metrics_and_lifecycle_configurations.rs"]
mod bucket_metrics_and_lifecycle_configurations;
#[path = "all/bucket_notification_configuration.rs"]
mod bucket_notification_configuration;
#[path = "all/bucket_property_updates.rs"]
mod bucket_property_updates;
#[path = "all/bucket_replacement_cleanup.rs"]
mod bucket_replacement_cleanup;
#[path = "all/bucket_replication_configuration.rs"]
mod bucket_replication_configuration;
#[path = "all/built_in_types_in_submitted_hierarchies.rs"]
mod built_in_types_in_submitted_hierarchies;
#[path = "all/callback_signature.rs"]
mod callback_signature;
#[path = "all/cancelled_update.rs"]
mod cancelled_update;
#[path = "all/cdk_asset_function_writes_its_granted_table.rs"]
mod cdk_asset_function_writes_its_granted_table;
#[path = "all/cdk_deployed_table_estate.rs"]
mod cdk_deployed_table_estate;
#[path = "all/cdk_metadata_resource_type.rs"]
mod cdk_metadata_resource_type;
#[path = "all/change_set_created_stack_template_reads.rs"]
mod change_set_created_stack_template_reads;
#[path = "all/change_set_early_validation.rs"]
mod change_set_early_validation;
#[path = "all/change_set_hooks.rs"]
mod change_set_hooks;
#[path = "all/change_set_import_and_drift.rs"]
mod change_set_import_and_drift;
#[path = "all/change_set_lifecycle.rs"]
mod change_set_lifecycle;
#[path = "all/change_set_preview.rs"]
mod change_set_preview;
#[path = "all/change_set_recreation.rs"]
mod change_set_recreation;
#[path = "all/change_set_status_refusals.rs"]
mod change_set_status_refusals;
#[path = "all/client_request_tokens.rs"]
mod client_request_tokens;
#[path = "all/cloudformation_ec2_instance_rollback.rs"]
mod cloudformation_ec2_instance_rollback;
#[path = "all/cognito_resource_types.rs"]
mod cognito_resource_types;
#[path = "all/cognito_user_pool_return_values.rs"]
mod cognito_user_pool_return_values;
#[path = "all/content_delivery_stack_outputs.rs"]
mod content_delivery_stack_outputs;
#[path = "all/continue_update_rollback_status_gate.rs"]
mod continue_update_rollback_status_gate;
#[path = "all/cross_stack_imports.rs"]
mod cross_stack_imports;
#[path = "all/cross_stack_name_collision.rs"]
mod cross_stack_name_collision;
#[path = "all/custom_resource_providers.rs"]
mod custom_resource_providers;
#[path = "all/custom_resource_response_identity.rs"]
mod custom_resource_response_identity;
// An EMPTY, moved-out suite: its rows now live in `all/delete_stack_export_in_use.rs`.
// Declared because the layout check refuses an undeclared file under `tests/all/`, and this
// fixer's tool set can write a file but not remove one; the next change to touch this
// directory should delete both lines and the file.
#[path = "all/delete_canceled_export_in_use.rs"]
mod delete_canceled_export_in_use;
#[path = "all/delete_stack_export_in_use.rs"]
mod delete_stack_export_in_use;
#[path = "all/delete_stack_instances.rs"]
mod delete_stack_instances;
#[path = "all/delete_stack_retain_resources.rs"]
mod delete_stack_retain_resources;
#[path = "all/delete_stack_single_writer.rs"]
mod delete_stack_single_writer;
#[path = "all/delete_stack_set.rs"]
mod delete_stack_set;
#[path = "all/denied_replacement_identity.rs"]
mod denied_replacement_identity;
#[path = "all/detect_resource_drift.rs"]
mod detect_resource_drift;
#[path = "all/drift_and_resource_reads.rs"]
mod drift_and_resource_reads;
#[path = "all/drift_resource_identity.rs"]
mod drift_resource_identity;
#[path = "all/duplicate_resource_identifiers.rs"]
mod duplicate_resource_identifiers;
#[path = "all/dynamic_reference_names.rs"]
mod dynamic_reference_names;
#[path = "all/dynamic_references.rs"]
mod dynamic_references;
#[path = "all/dynamodb_kinesis_streaming_estate.rs"]
mod dynamodb_kinesis_streaming_estate;
#[path = "all/dynamodb_table_resources.rs"]
mod dynamodb_table_resources;
#[path = "all/dynamodb_table_settings.rs"]
mod dynamodb_table_settings;
#[path = "all/ec2_network_identifiers_feed_rds.rs"]
mod ec2_network_identifiers_feed_rds;
#[path = "all/ec2_network_stack_resources.rs"]
mod ec2_network_stack_resources;
#[path = "all/ec2_security_group_stack_resources.rs"]
mod ec2_security_group_stack_resources;
#[path = "all/ec2_vpc_gateway_stack_resources.rs"]
mod ec2_vpc_gateway_stack_resources;
#[path = "all/ecs_elbv2_stack_identifiers.rs"]
mod ecs_elbv2_stack_identifiers;
// An EMPTY, moved-out suite: its rows now live in `all/nested_resource_outputs.rs` and
// `all/provisioned_resources.rs`. Declared because the layout check refuses an undeclared
// file under `tests/all/`, and this fixer's tool set can write a file but not remove one;
// the next change to touch this directory should delete both lines and the file.
#[path = "all/ecs_elbv2_stack_outputs.rs"]
mod ecs_elbv2_stack_outputs;
#[path = "all/elbv2_resource_provisioning.rs"]
mod elbv2_resource_provisioning;
#[path = "all/elbv2_stack_update_lifecycle.rs"]
mod elbv2_stack_update_lifecycle;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/estimate_template_cost.rs"]
mod estimate_template_cost;
#[path = "all/executed_change_set_create_outcomes.rs"]
mod executed_change_set_create_outcomes;
#[path = "all/export_pin_release.rs"]
mod export_pin_release;
#[path = "all/extension_registrations.rs"]
mod extension_registrations;
#[path = "all/extension_registry.rs"]
mod extension_registry;
#[path = "all/extension_registry_lifecycle.rs"]
mod extension_registry_lifecycle;
#[path = "all/failed_create_rollback.rs"]
mod failed_create_rollback;
#[path = "all/failed_resource_status_reason.rs"]
mod failed_resource_status_reason;
#[path = "all/failed_update_resource_rollback.rs"]
mod failed_update_resource_rollback;
#[path = "all/forced_deletion_is_gated_by_status.rs"]
mod forced_deletion_is_gated_by_status;
#[path = "all/forced_deletion_refusal_wording.rs"]
mod forced_deletion_refusal_wording;
#[path = "all/forced_stack_deletion.rs"]
mod forced_stack_deletion;
#[path = "all/foreach_expanded_output_values.rs"]
mod foreach_expanded_output_values;
#[path = "all/gated_update_terminal_status.rs"]
mod gated_update_terminal_status;
#[path = "all/generated_physical_names.rs"]
mod generated_physical_names;
#[path = "all/generated_template_body.rs"]
mod generated_template_body;
#[path = "all/generated_template_coverage.rs"]
mod generated_template_coverage;
#[path = "all/generated_template_deletion.rs"]
mod generated_template_deletion;
#[path = "all/generated_template_limit_wording.rs"]
mod generated_template_limit_wording;
#[path = "all/generated_template_reads.rs"]
mod generated_template_reads;
#[path = "all/generated_template_refusals.rs"]
mod generated_template_refusals;
#[path = "all/generated_template_updates.rs"]
mod generated_template_updates;
#[path = "all/generated_templates.rs"]
mod generated_templates;
#[path = "all/glue_job_resources.rs"]
mod glue_job_resources;
#[path = "all/handler_progress_reports.rs"]
mod handler_progress_reports;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/held_signal_waits.rs"]
mod held_signal_waits;
#[path = "all/hook_resource_return_values.rs"]
mod hook_resource_return_values;
#[path = "all/hook_result_detail.rs"]
mod hook_result_detail;
#[path = "all/hook_result_invocations.rs"]
mod hook_result_invocations;
#[path = "all/hotswapped_function_code.rs"]
mod hotswapped_function_code;
#[path = "all/iam_capability_gate.rs"]
mod iam_capability_gate;
#[path = "all/iam_identity_resources.rs"]
mod iam_identity_resources;
#[path = "all/import_stacks_to_stack_set.rs"]
mod import_stacks_to_stack_set;
#[path = "all/include_transform.rs"]
mod include_transform;
#[path = "all/independent_gate_ready_set.rs"]
mod independent_gate_ready_set;
#[path = "all/in_flight_resource_status.rs"]
mod in_flight_resource_status;
#[path = "all/initiating_resource_events.rs"]
mod initiating_resource_events;
#[path = "all/in_place_resource_updates.rs"]
mod in_place_resource_updates;
#[path = "all/kept_failed_create.rs"]
mod kept_failed_create;
#[path = "all/lambda_backed_custom_resources.rs"]
mod lambda_backed_custom_resources;
#[path = "all/lambda_function_stack_tags.rs"]
mod lambda_function_stack_tags;
#[path = "all/lambda_layer_version_reference.rs"]
mod lambda_layer_version_reference;
#[path = "all/lambda_permission_stack_updates.rs"]
mod lambda_permission_stack_updates;
#[path = "all/list_change_sets.rs"]
mod list_change_sets;
#[path = "all/list_element_refusals.rs"]
mod list_element_refusals;
#[path = "all/list_generated_templates.rs"]
mod list_generated_templates;
#[path = "all/list_imports_pages.rs"]
mod list_imports_pages;
#[path = "all/list_stack_instances.rs"]
mod list_stack_instances;
#[path = "all/list_stack_resources.rs"]
mod list_stack_resources;
#[path = "all/list_stack_set_auto_deployment_targets.rs"]
mod list_stack_set_auto_deployment_targets;
#[path = "all/list_stack_set_operation_results.rs"]
mod list_stack_set_operation_results;
#[path = "all/list_stack_set_operations.rs"]
mod list_stack_set_operations;
#[path = "all/list_stack_sets.rs"]
mod list_stack_sets;
#[path = "all/list_type_versions_registry_scope.rs"]
mod list_type_versions_registry_scope;
#[path = "all/list_types.rs"]
mod list_types;
#[path = "all/logs_and_iot_resources.rs"]
mod logs_and_iot_resources;
#[path = "all/logs_group_children_resources.rs"]
mod logs_group_children_resources;
#[path = "all/macro_acknowledgement_and_include_refresh.rs"]
mod macro_acknowledgement_and_include_refresh;
#[path = "all/malformed_policy_document_rollback.rs"]
mod malformed_policy_document_rollback;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/nested_resource_outputs.rs"]
mod nested_resource_outputs;
#[path = "all/nested_stack_output_attributes.rs"]
mod nested_stack_output_attributes;
#[path = "all/nested_stack_output_values.rs"]
mod nested_stack_output_values;
#[path = "all/nested_stacks.rs"]
mod nested_stacks;
#[path = "all/no_echo_parameters.rs"]
mod no_echo_parameters;
#[path = "all/operation_events.rs"]
mod operation_events;
#[path = "all/organizations_access.rs"]
mod organizations_access;
#[path = "all/out_of_band_resource_scan.rs"]
mod out_of_band_resource_scan;
#[path = "all/parameter_and_change_set_conflicts.rs"]
mod parameter_and_change_set_conflicts;
#[path = "all/provider_answer_follow_up.rs"]
mod provider_answer_follow_up;
#[path = "all/provisioned_ecr_repository.rs"]
mod provisioned_ecr_repository;
#[path = "all/provisioned_resources.rs"]
mod provisioned_resources;
#[path = "all/public_extension_arn_versions.rs"]
mod public_extension_arn_versions;
#[path = "all/public_extension_arns.rs"]
mod public_extension_arns;
#[path = "all/publish_type.rs"]
mod publish_type;
#[path = "all/publish_type_publisher_preflight.rs"]
mod publish_type_publisher_preflight;
#[path = "all/publish_type_version_in_arn.rs"]
mod publish_type_version_in_arn;
#[path = "all/published_type_schema.rs"]
mod published_type_schema;
#[path = "all/publisher_eligibility.rs"]
mod publisher_eligibility;
#[path = "all/publisher_gate_scope.rs"]
mod publisher_gate_scope;
#[path = "all/publisher_reads.rs"]
mod publisher_reads;
#[path = "all/publisher_registration.rs"]
mod publisher_registration;
#[path = "all/rds_generated_resource_names.rs"]
mod rds_generated_resource_names;
#[path = "all/rds_proxy_stack_resources.rs"]
mod rds_proxy_stack_resources;
#[path = "all/rds_published_name_identity.rs"]
mod rds_published_name_identity;
#[path = "all/rds_stack_resources.rs"]
mod rds_stack_resources;
#[path = "all/refusals_name_their_modelled_taxonomy.rs"]
mod refusals_name_their_modelled_taxonomy;
#[path = "all/registry_and_scan_read_members.rs"]
mod registry_and_scan_read_members;
#[path = "all/refused_delete_resource_views.rs"]
mod refused_delete_resource_views;
#[path = "all/refused_resource_sdk_exception.rs"]
mod refused_resource_sdk_exception;
#[path = "all/refused_stack_deletion.rs"]
mod refused_stack_deletion;
#[path = "all/regional_stack_namespace.rs"]
mod regional_stack_namespace;
#[path = "all/requested_rollback.rs"]
mod requested_rollback;
#[path = "all/required_resource_properties.rs"]
mod required_resource_properties;
#[path = "all/reserved_stack_tag_keys.rs"]
mod reserved_stack_tag_keys;
#[path = "all/resource_deletion_policies.rs"]
mod resource_deletion_policies;
#[path = "all/resource_scan_discovery.rs"]
mod resource_scan_discovery;
#[path = "all/resource_scan_inventory.rs"]
mod resource_scan_inventory;
#[path = "all/resource_scan_lifecycle.rs"]
mod resource_scan_lifecycle;
#[path = "all/resource_scan_related_resources.rs"]
mod resource_scan_related_resources;
#[path = "all/resource_scan_resources.rs"]
mod resource_scan_resources;
#[path = "all/resource_scans.rs"]
mod resource_scans;
#[path = "all/resource_signal_waits.rs"]
mod resource_signal_waits;
#[path = "all/resource_signals.rs"]
mod resource_signals;
// Declared in sorted position in this quiet span, rather than in the tail of this file,
// which is where every concurrently landing suite appends its line.
#[path = "all/route53_resource_declarations.rs"]
mod route53_resource_declarations;
#[path = "all/rollback_trigger_bounds.rs"]
mod rollback_trigger_bounds;
#[path = "all/rollback_trigger_types.rs"]
mod rollback_trigger_types;
#[path = "all/rolled_back_create_stays_describable.rs"]
mod rolled_back_create_stays_describable;
#[path = "all/rolled_back_resource_records.rs"]
mod rolled_back_resource_records;
#[path = "all/rolled_back_resource_retention.rs"]
mod rolled_back_resource_retention;
#[path = "all/rolled_back_stack_configuration.rs"]
mod rolled_back_stack_configuration;
#[path = "all/s3vectors_resource_types.rs"]
mod s3vectors_resource_types;
#[path = "all/resolved_properties.rs"]
mod resolved_properties;
#[path = "all/sam_api_definitions.rs"]
mod sam_api_definitions;
#[path = "all/sam_api_stages.rs"]
mod sam_api_stages;
#[path = "all/sam_deployed_resources.rs"]
mod sam_deployed_resources;
#[path = "all/sam_layer_version_references.rs"]
mod sam_layer_version_references;
#[path = "all/sam_policies_shorthand.rs"]
mod sam_policies_shorthand;
#[path = "all/sam_transform.rs"]
mod sam_transform;
#[path = "all/scan_and_registry_reads.rs"]
mod scan_and_registry_reads;
#[path = "all/scan_hook_and_progress_refusal_wording.rs"]
mod scan_hook_and_progress_refusal_wording;
#[path = "all/scratch.rs"]
mod scratch;
#[path = "all/scratch_probe.rs"]
mod scratch_probe;
#[path = "all/search_indexing_pipeline_outputs.rs"]
mod search_indexing_pipeline_outputs;
#[path = "all/section_loop_expansion.rs"]
mod section_loop_expansion;
#[path = "all/service_role_permissions.rs"]
mod service_role_permissions;
#[path = "all/signalling_instance_gates.rs"]
mod signalling_instance_gates;
#[path = "all/ssm_association_resources.rs"]
mod ssm_association_resources;
#[path = "all/ssm_typed_parameters.rs"]
mod ssm_typed_parameters;
#[path = "all/stack_configuration_and_provider_limits.rs"]
mod stack_configuration_and_provider_limits;
#[path = "all/stack_declared_function_identity.rs"]
mod stack_declared_function_identity;
#[path = "all/stack_delete_failure_reason.rs"]
mod stack_delete_failure_reason;
#[path = "all/stack_event_timeline.rs"]
mod stack_event_timeline;
#[path = "all/stack_exports.rs"]
mod stack_exports;
#[path = "all/stack_failure_reason.rs"]
mod stack_failure_reason;
#[path = "all/stack_fleet_resources.rs"]
mod stack_fleet_resources;
#[path = "all/stack_identifier_refusals.rs"]
mod stack_identifier_refusals;
#[path = "all/stack_input_contracts.rs"]
mod stack_input_contracts;
#[path = "all/stack_lifecycle.rs"]
mod stack_lifecycle;
#[path = "all/stack_not_found_wording.rs"]
mod stack_not_found_wording;
#[path = "all/stack_output_reads.rs"]
mod stack_output_reads;
#[path = "all/stack_output_resolution.rs"]
mod stack_output_resolution;
#[path = "all/stack_own_subnets_db_subnet_group.rs"]
mod stack_own_subnets_db_subnet_group;
#[path = "all/stack_outputs.rs"]
mod stack_outputs;
#[path = "all/stack_owned_secret_teardown.rs"]
mod stack_owned_secret_teardown;
#[path = "all/stack_parameter_driven_updates.rs"]
mod stack_parameter_driven_updates;
#[path = "all/stack_policy_by_reference.rs"]
mod stack_policy_by_reference;
#[path = "all/stack_policy_enforcement.rs"]
mod stack_policy_enforcement;
#[path = "all/stack_refactors.rs"]
mod stack_refactors;
#[path = "all/stack_refusal_wording.rs"]
mod stack_refusal_wording;
#[path = "all/stack_reported_members.rs"]
mod stack_reported_members;
#[path = "all/stack_resource_detail.rs"]
mod stack_resource_detail;
#[path = "all/stack_resource_drifts.rs"]
mod stack_resource_drifts;
#[path = "all/stack_resource_events.rs"]
mod stack_resource_events;
#[path = "all/stack_resource_providers.rs"]
mod stack_resource_providers;
#[path = "all/stack_resource_provisioning.rs"]
mod stack_resource_provisioning;
#[path = "all/stack_resources.rs"]
mod stack_resources;
#[path = "all/stack_role_trust.rs"]
mod stack_role_trust;
#[path = "all/stack_service_role_stickiness.rs"]
mod stack_service_role_stickiness;
#[path = "all/stack_set_drift_lifecycle.rs"]
mod stack_set_drift_lifecycle;
#[path = "all/stack_set_lifecycle.rs"]
mod stack_set_lifecycle;
#[path = "all/stack_set_mutations.rs"]
mod stack_set_mutations;
#[path = "all/stack_set_operation_refusal_wording.rs"]
mod stack_set_operation_refusal_wording;
#[path = "all/stack_set_operation_tokens_and_outcomes.rs"]
mod stack_set_operation_tokens_and_outcomes;
#[path = "all/stack_set_refusal_wording.rs"]
mod stack_set_refusal_wording;
#[path = "all/stack_set_revision_rollout.rs"]
mod stack_set_revision_rollout;
#[path = "all/stack_status_accepts_an_update.rs"]
mod stack_status_accepts_an_update;
#[path = "all/stack_tag_ascii_text.rs"]
mod stack_tag_ascii_text;
#[path = "all/stack_tag_propagation.rs"]
mod stack_tag_propagation;
#[path = "all/stack_teardown_events.rs"]
mod stack_teardown_events;
#[path = "all/stack_unique_id_identifiers.rs"]
mod stack_unique_id_identifiers;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/step_functions_state_machine_replacement.rs"]
mod step_functions_state_machine_replacement;
#[path = "all/step_functions_state_machine_stack_provenance.rs"]
mod step_functions_state_machine_stack_provenance;
#[path = "all/stop_stack_set_operation.rs"]
mod stop_stack_set_operation;
#[path = "all/stored_stack_policy_validation.rs"]
mod stored_stack_policy_validation;
#[path = "all/substitution_resource_attributes.rs"]
mod substitution_resource_attributes;
#[path = "all/superseded_failure_events.rs"]
mod superseded_failure_events;
#[path = "all/tags_only_stack_updates.rs"]
mod tags_only_stack_updates;
#[path = "all/template_by_reference.rs"]
mod template_by_reference;
#[path = "all/template_limits_and_declared_order.rs"]
mod template_limits_and_declared_order;
#[path = "all/template_outputs.rs"]
mod template_outputs;
#[path = "all/template_planning_surfaces.rs"]
mod template_planning_surfaces;
#[path = "all/template_quotas.rs"]
mod template_quotas;
#[path = "all/template_refusal_wording.rs"]
mod template_refusal_wording;
#[path = "all/template_stages_and_resource_events.rs"]
mod template_stages_and_resource_events;
#[path = "all/template_structure_refusals.rs"]
mod template_structure_refusals;
#[path = "all/template_summary.rs"]
mod template_summary;
#[path = "all/template_summary_resource_types.rs"]
mod template_summary_resource_types;
#[path = "all/template_validation.rs"]
mod template_validation;
#[path = "all/test_type_publisher_gate.rs"]
mod test_type_publisher_gate;
#[path = "all/third_party_extensions.rs"]
mod third_party_extensions;
#[path = "all/type_configurations.rs"]
mod type_configurations;
#[path = "all/type_deactivation.rs"]
mod type_deactivation;
#[path = "all/type_default_version.rs"]
mod type_default_version;
#[path = "all/type_deregistration.rs"]
mod type_deregistration;
#[path = "all/type_registrations.rs"]
mod type_registrations;
#[path = "all/type_testing.rs"]
mod type_testing;
#[path = "all/uncreated_custom_resource_rollback.rs"]
mod uncreated_custom_resource_rollback;
#[path = "all/unimported_exports.rs"]
mod unimported_exports;
#[path = "all/unique_export_names.rs"]
mod unique_export_names;
#[path = "all/unprovisioned_resources.rs"]
mod unprovisioned_resources;
#[path = "all/unrecognized_resource_types.rs"]
mod unrecognized_resource_types;
#[path = "all/unresolvable_imports.rs"]
mod unresolvable_imports;
#[path = "all/unreverted_rollback_resumption.rs"]
mod unreverted_rollback_resumption;
#[path = "all/unschedulable_update_leaves_the_deployment_alone.rs"]
mod unschedulable_update_leaves_the_deployment_alone;
#[path = "all/unsignalled_gate_hold.rs"]
mod unsignalled_gate_hold;
#[path = "all/update_after_a_refused_delete.rs"]
mod update_after_a_refused_delete;
#[path = "all/update_rollback_of_a_refused_update.rs"]
mod update_rollback_of_a_refused_update;
#[path = "all/update_rollback_recovery.rs"]
mod update_rollback_recovery;
#[path = "all/update_stack_export_in_use.rs"]
mod update_stack_export_in_use;
#[path = "all/update_stack_instances.rs"]
mod update_stack_instances;
#[path = "all/validate_template.rs"]
mod validate_template;
#[path = "all/validation_event_paths.rs"]
mod validation_event_paths;
#[path = "all/vpc_network_teardown.rs"]
mod vpc_network_teardown;
#[path = "all/waf_regional_ip_set_resources.rs"]
mod waf_regional_ip_set_resources;
#[path = "all/wait_conditions.rs"]
mod wait_conditions;
#[path = "all/deletion_policy_snapshot.rs"]
mod deletion_policy_snapshot;
#[path = "all/expired_wait_rollback.rs"]
mod expired_wait_rollback;
#[path = "all/ssm_parameter_types.rs"]
mod ssm_parameter_types;
#[path = "all/mwaa_environment_resource_type.rs"]
mod mwaa_environment_resource_type;
#[path = "all/dynamodb_table_creation_window.rs"]
mod dynamodb_table_creation_window;
