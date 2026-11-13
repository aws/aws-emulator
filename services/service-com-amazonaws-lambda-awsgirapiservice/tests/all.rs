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
#[path = "all/account_allocated_concurrency.rs"]
mod account_allocated_concurrency;
#[path = "all/account_concurrency_floor_message.rs"]
mod account_concurrency_floor_message;
#[path = "all/account_concurrency_metering.rs"]
mod account_concurrency_metering;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/account_settings_deprecated_runtime.rs"]
mod account_settings_deprecated_runtime;
#[path = "all/alias_deletion.rs"]
mod alias_deletion;
#[path = "all/alias_description_member.rs"]
mod alias_description_member;
#[path = "all/alias_listing.rs"]
mod alias_listing;
#[path = "all/alias_name_constraint.rs"]
mod alias_name_constraint;
#[path = "all/alias_not_found_sentence.rs"]
mod alias_not_found_sentence;
#[path = "all/alias_update.rs"]
mod alias_update;
#[path = "all/alias_weighted_routing.rs"]
mod alias_weighted_routing;
#[path = "all/alias_update_not_found_sentence.rs"]
mod alias_update_not_found_sentence;
#[path = "all/alias_version_not_found_sentence.rs"]
mod alias_version_not_found_sentence;
#[path = "all/alias_write_not_found_sentence.rs"]
mod alias_write_not_found_sentence;
#[path = "all/announced_download_locations.rs"]
mod announced_download_locations;
#[path = "all/async_completion_destinations.rs"]
mod async_completion_destinations;
#[path = "all/async_delivery.rs"]
mod async_delivery;
#[path = "all/async_event_delivery_ownership.rs"]
mod async_event_delivery_ownership;
#[path = "all/async_invocation.rs"]
mod async_invocation;
#[path = "all/asynchronous_invoke_run.rs"]
mod asynchronous_invoke_run;
#[path = "all/aws_refusal_wording_and_region_isolation.rs"]
mod aws_refusal_wording_and_region_isolation;
#[path = "all/capacity_provider_authorization.rs"]
mod capacity_provider_authorization;
#[path = "all/capacity_provider_bounds.rs"]
mod capacity_provider_bounds;
#[path = "all/capacity_provider_duplicate_name.rs"]
mod capacity_provider_duplicate_name;
#[path = "all/capacity_provider_listing.rs"]
mod capacity_provider_listing;
#[path = "all/capacity_provider_managed_function_state.rs"]
mod capacity_provider_managed_function_state;
#[path = "all/capacity_provider_naming.rs"]
mod capacity_provider_naming;
#[path = "all/capacity_provider_operator_role.rs"]
mod capacity_provider_operator_role;
#[path = "all/capacity_provider_refusal_messages.rs"]
mod capacity_provider_refusal_messages;
#[path = "all/capacity_provider_update.rs"]
mod capacity_provider_update;
#[path = "all/capacity_providers.rs"]
mod capacity_providers;
#[path = "all/code_signing_config_deletion.rs"]
mod code_signing_config_deletion;
#[path = "all/code_signing_config_listing.rs"]
mod code_signing_config_listing;
#[path = "all/code_signing_config_update.rs"]
mod code_signing_config_update;
#[path = "all/code_signing_configs.rs"]
mod code_signing_configs;
#[path = "all/code_signing_enforcement.rs"]
mod code_signing_enforcement;
#[path = "all/collection_element_refusals.rs"]
mod collection_element_refusals;
#[path = "all/concurrency_quota_refusal_rules.rs"]
mod concurrency_quota_refusal_rules;
#[path = "all/container_image_pull_permission.rs"]
mod container_image_pull_permission;
#[path = "all/control_plane_request_rules.rs"]
mod control_plane_request_rules;
#[path = "all/durable_execution_bound.rs"]
mod durable_execution_bound;
#[path = "all/durable_execution_history.rs"]
mod durable_execution_history;
#[path = "all/durable_execution_listing.rs"]
mod durable_execution_listing;
#[path = "all/durable_execution_refusals.rs"]
mod durable_execution_refusals;
#[path = "all/durable_execution_run.rs"]
mod durable_execution_run;
#[path = "all/durable_execution_start.rs"]
mod durable_execution_start;
#[path = "all/durable_execution_status_events.rs"]
mod durable_execution_status_events;
#[path = "all/durable_execution_stop.rs"]
mod durable_execution_stop;
#[path = "all/dynamodb_stream_event_source_delivery.rs"]
mod dynamodb_stream_event_source_delivery;
#[path = "all/event_invoke_config.rs"]
mod event_invoke_config;
#[path = "all/event_invoke_config_destination_slots.rs"]
mod event_invoke_config_destination_slots;
#[path = "all/event_source_mapping_attempt_report.rs"]
mod event_source_mapping_attempt_report;
#[path = "all/event_source_mapping_batch_item_failures.rs"]
mod event_source_mapping_batch_item_failures;
#[path = "all/event_source_mapping_batching_window.rs"]
mod event_source_mapping_batching_window;
#[path = "all/event_source_mapping_delete_in_use.rs"]
mod event_source_mapping_delete_in_use;
#[path = "all/event_source_mapping_destination_service.rs"]
mod event_source_mapping_destination_service;
#[path = "all/event_source_mapping_deletion.rs"]
mod event_source_mapping_deletion;
#[path = "all/event_source_mapping_duplicate_source.rs"]
mod event_source_mapping_duplicate_source;
#[path = "all/event_source_mapping_event_counts.rs"]
mod event_source_mapping_event_counts;
#[path = "all/event_source_mapping_least_privilege_role.rs"]
mod event_source_mapping_least_privilege_role;
#[path = "all/event_source_mapping_metrics_duplicates.rs"]
mod event_source_mapping_metrics_duplicates;
#[path = "all/event_source_mapping_metrics_scope.rs"]
mod event_source_mapping_metrics_scope;
#[path = "all/event_source_mapping_paused_state_and_response_types.rs"]
mod event_source_mapping_paused_state_and_response_types;
#[path = "all/event_source_mapping_processing_visibility.rs"]
mod event_source_mapping_processing_visibility;
#[path = "all/event_source_mapping_queue_visibility.rs"]
mod event_source_mapping_queue_visibility;
#[path = "all/event_source_mapping_role_access.rs"]
mod event_source_mapping_role_access;
#[path = "all/event_source_mapping_shard_refusal_wording.rs"]
mod event_source_mapping_shard_refusal_wording;
#[path = "all/event_source_mapping_source_shape_and_window.rs"]
mod event_source_mapping_source_shape_and_window;
#[path = "all/event_source_mapping_sqs_delivery.rs"]
mod event_source_mapping_sqs_delivery;
#[path = "all/event_source_mapping_unsupported_source_arn.rs"]
mod event_source_mapping_unsupported_source_arn;
#[path = "all/event_source_mapping_update.rs"]
mod event_source_mapping_update;
#[path = "all/event_source_mapping_update_omitted_members.rs"]
mod event_source_mapping_update_omitted_members;
#[path = "all/error_fault_class.rs"]
mod error_fault_class;
#[path = "all/event_source_mappings.rs"]
mod event_source_mappings;
#[path = "all/event_source_poll_authorization.rs"]
mod event_source_poll_authorization;
#[path = "all/event_source_poll_passes.rs"]
mod event_source_poll_passes;
#[path = "all/event_source_poll_resource_policy.rs"]
mod event_source_poll_resource_policy;
// The file name is the one this suite was first written under; the layout check binds the
// module name to it.
#[path = "all/scratch_sqs_handler_run.rs"]
mod scratch_sqs_handler_run;
// Declared in this span of the sorted list rather than at the head of this file or in its
// appended tail: both of those are where concurrently landing suites put their line, so a
// declaration there loses a squash merge to every one of them, while these neighbouring
// lines are stable.
#[path = "all/execution_role_grant_withdrawal.rs"]
mod execution_role_grant_withdrawal;
#[path = "all/execution_role_resolution.rs"]
mod execution_role_resolution;
#[path = "all/function_aliases.rs"]
mod function_aliases;
#[path = "all/function_code_signing_association.rs"]
mod function_code_signing_association;
#[path = "all/function_code_signing_dissociation.rs"]
mod function_code_signing_dissociation;
#[path = "all/function_code_signing_removal.rs"]
mod function_code_signing_removal;
#[path = "all/function_code_update.rs"]
mod function_code_update;
#[path = "all/function_concurrency.rs"]
mod function_concurrency;
#[path = "all/function_concurrency_removal.rs"]
mod function_concurrency_removal;
#[path = "all/function_concurrency_reported_member.rs"]
mod function_concurrency_reported_member;
#[path = "all/function_configuration.rs"]
mod function_configuration;
#[path = "all/function_configuration_quotas.rs"]
mod function_configuration_quotas;
#[path = "all/function_last_update_status_reason.rs"]
mod function_last_update_status_reason;
#[path = "all/function_layer_unzipped_size.rs"]
mod function_layer_unzipped_size;
#[path = "all/function_lifecycle.rs"]
mod function_lifecycle;
#[path = "all/function_recursion_config.rs"]
mod function_recursion_config;
#[path = "all/function_scaling_config.rs"]
mod function_scaling_config;
#[path = "all/function_scaling_config_capacity_provider.rs"]
mod function_scaling_config_capacity_provider;
#[path = "all/function_scaling_config_convergence.rs"]
mod function_scaling_config_convergence;
#[path = "all/function_scaling_config_default.rs"]
mod function_scaling_config_default;
#[path = "all/function_update_window_conflict.rs"]
mod function_update_window_conflict;
#[path = "all/function_url_announced_endpoint.rs"]
mod function_url_announced_endpoint;
#[path = "all/function_url_config.rs"]
mod function_url_config;
#[path = "all/function_url_cors.rs"]
mod function_url_cors;
#[path = "all/function_url_data_plane.rs"]
mod function_url_data_plane;
#[path = "all/function_url_deletion.rs"]
mod function_url_deletion;
#[path = "all/function_url_listing.rs"]
mod function_url_listing;
#[path = "all/function_url_reported_members.rs"]
mod function_url_reported_members;
#[path = "all/function_url_unauthenticated_refusal.rs"]
mod function_url_unauthenticated_refusal;
#[path = "all/function_url_update.rs"]
mod function_url_update;
#[path = "all/function_version_listing.rs"]
mod function_version_listing;
#[path = "all/function_versions_by_capacity_provider.rs"]
mod function_versions_by_capacity_provider;
#[path = "all/functions_by_code_signing_config.rs"]
mod functions_by_code_signing_config;
#[path = "all/handler_appconfig_execution_role.rs"]
mod handler_appconfig_execution_role;
#[path = "all/handler_appconfig_sdk.rs"]
mod handler_appconfig_sdk;
#[path = "all/handler_aws_sdk.rs"]
mod handler_aws_sdk;
#[path = "all/handler_bucket_administration_sdk.rs"]
mod handler_bucket_administration_sdk;
#[path = "all/handler_customer_key_access.rs"]
mod handler_customer_key_access;
#[path = "all/handler_db_auth_token.rs"]
mod handler_db_auth_token;
#[path = "all/handler_lambda_sdk.rs"]
mod handler_lambda_sdk;
#[path = "all/handler_logging_facility.rs"]
mod handler_logging_facility;
#[path = "all/handler_metric_timestamp.rs"]
mod handler_metric_timestamp;
#[path = "all/handler_notification_sdk.rs"]
mod handler_notification_sdk;
#[path = "all/handler_object_store_sdk.rs"]
mod handler_object_store_sdk;
#[path = "all/handler_parameter_store_sdk.rs"]
mod handler_parameter_store_sdk;
#[path = "all/handler_query_compatible_error_code.rs"]
mod handler_query_compatible_error_code;
#[path = "all/handler_dynamodb_binary_attributes.rs"]
mod handler_dynamodb_binary_attributes;
#[path = "all/handler_dynamodb_resource.rs"]
mod handler_dynamodb_resource;
#[path = "all/handler_email_sdk.rs"]
mod handler_email_sdk;
#[path = "all/handler_object_encryption_members.rs"]
mod handler_object_encryption_members;
#[path = "all/handler_object_storage_sdk.rs"]
mod handler_object_storage_sdk;
#[path = "all/handler_rds_data_sdk.rs"]
mod handler_rds_data_sdk;
#[path = "all/handler_rest_api_sdk.rs"]
mod handler_rest_api_sdk;
#[path = "all/handler_step_functions_sdk.rs"]
mod handler_step_functions_sdk;
#[path = "all/handler_s3_bucket_policy_principal.rs"]
mod handler_s3_bucket_policy_principal;
#[path = "all/handler_s3_object_access.rs"]
mod handler_s3_object_access;
#[path = "all/handler_s3_prefix_deny_precedence.rs"]
mod handler_s3_prefix_deny_precedence;
#[path = "all/handler_s3_wildcard_grant.rs"]
mod handler_s3_wildcard_grant;
#[path = "all/handler_sdk_client_config.rs"]
mod handler_sdk_client_config;
#[path = "all/handler_sdk_error_classes.rs"]
mod handler_sdk_error_classes;
#[path = "all/handler_sdk_idempotency_token.rs"]
mod handler_sdk_idempotency_token;
#[path = "all/handler_sdk_paginator.rs"]
mod handler_sdk_paginator;
#[path = "all/handler_sdk_service_inventory.rs"]
mod handler_sdk_service_inventory;
#[path = "all/handler_secret_relayed_to_queue.rs"]
mod handler_secret_relayed_to_queue;
#[path = "all/handler_secret_sdk.rs"]
mod handler_secret_sdk;
#[path = "all/handler_workbook_pipeline.rs"]
mod handler_workbook_pipeline;
#[path = "all/inline_archive_request_quota.rs"]
mod inline_archive_request_quota;
#[path = "all/invocation_concurrency.rs"]
mod invocation_concurrency;
#[path = "all/invocation_count_series.rs"]
mod invocation_count_series;
#[path = "all/invocation_duration_report_agreement.rs"]
mod invocation_duration_report_agreement;
#[path = "all/invocation_duration_statistics_reads.rs"]
mod invocation_duration_statistics_reads;
#[path = "all/invocation_init_phase.rs"]
mod invocation_init_phase;
#[path = "all/invocation_log_delivery.rs"]
mod invocation_log_delivery;
#[path = "all/invocation_log_delivery_grant_carrier.rs"]
mod invocation_log_delivery_grant_carrier;
#[path = "all/invocation_log_group_search.rs"]
mod invocation_log_group_search;
#[path = "all/invocation_outcome_series.rs"]
mod invocation_outcome_series;
#[path = "all/invocation_refusal_diagnostics.rs"]
mod invocation_refusal_diagnostics;
#[path = "all/invocation_remaining_time_countdown.rs"]
mod invocation_remaining_time_countdown;
#[path = "all/invocation_reserved_concurrency_burst.rs"]
mod invocation_reserved_concurrency_burst;
#[path = "all/invocation_run_capacity.rs"]
mod invocation_run_capacity;
#[path = "all/invoke_addressed_function.rs"]
mod invoke_addressed_function;
#[path = "all/invoke_durable_execution_handle.rs"]
mod invoke_durable_execution_handle;
#[path = "all/invoke_function_error_document.rs"]
mod invoke_function_error_document;
#[path = "all/invoke_request_id.rs"]
mod invoke_request_id;
#[path = "all/invoke_authorization.rs"]
mod invoke_authorization;
#[path = "all/invoke_authorization_ceiling.rs"]
mod invoke_authorization_ceiling;
#[path = "all/invoke_identity_authorization.rs"]
mod invoke_identity_authorization;
#[path = "all/invoke_resource_policy_denial.rs"]
mod invoke_resource_policy_denial;
#[path = "all/kinesis_event_source_delivery.rs"]
mod kinesis_event_source_delivery;
#[path = "all/kinesis_event_source_handler_run.rs"]
mod kinesis_event_source_handler_run;
#[path = "all/kinesis_event_source_handler_writes_dynamodb.rs"]
mod kinesis_event_source_handler_writes_dynamodb;
#[path = "all/layer_archive_unzipped_size.rs"]
mod layer_archive_unzipped_size;
#[path = "all/layer_listing.rs"]
mod layer_listing;
#[path = "all/layer_listing_entry_members.rs"]
mod layer_listing_entry_members;
#[path = "all/layer_region_scope.rs"]
mod layer_region_scope;
#[path = "all/layer_version_by_arn.rs"]
mod layer_version_by_arn;
#[path = "all/layer_version_listing.rs"]
mod layer_version_listing;
#[path = "all/layer_version_permissions.rs"]
mod layer_version_permissions;
#[path = "all/layer_versions.rs"]
mod layer_versions;
#[path = "all/log_level_requires_json_format.rs"]
mod log_level_requires_json_format;
#[path = "all/missing_policy_refusal.rs"]
mod missing_policy_refusal;
#[path = "all/node_error_trace_lines.rs"]
mod node_error_trace_lines;
#[path = "all/node_handler_sdk_aggregated_client.rs"]
mod node_handler_sdk_aggregated_client;
#[path = "all/node_handler_sdk_esm.rs"]
mod node_handler_sdk_esm;
#[path = "all/node_initialization_type.rs"]
mod node_initialization_type;
#[path = "all/node_invoke_run.rs"]
mod node_invoke_run;
#[path = "all/node_loader_errors.rs"]
mod node_loader_errors;
#[path = "all/node_runtime_environment.rs"]
mod node_runtime_environment;
#[path = "all/nodejs_invoke_run.rs"]
mod nodejs_invoke_run;
#[path = "all/package_unzipped_ceiling.rs"]
mod package_unzipped_ceiling;
#[path = "all/permission_and_policy_refusals.rs"]
mod permission_and_policy_refusals;
#[path = "all/permission_principal_and_qualifier_rules.rs"]
mod permission_principal_and_qualifier_rules;
#[path = "all/policy_versions_and_resources.rs"]
mod policy_versions_and_resources;
#[path = "all/probe_loader.rs"]
mod probe_loader;
#[path = "all/provisioned_concurrency.rs"]
mod provisioned_concurrency;
#[path = "all/provisioned_concurrency_resolution.rs"]
mod provisioned_concurrency_resolution;
#[path = "all/published_function_arn.rs"]
mod published_function_arn;
#[path = "all/python_runtime_os_surface.rs"]
mod python_runtime_os_surface;
#[path = "all/python_sandbox_bound_isolation.rs"]
mod python_sandbox_bound_isolation;
#[path = "all/python_tls_surface.rs"]
mod python_tls_surface;
#[path = "all/queue_message_count_snapshot.rs"]
mod queue_message_count_snapshot;
#[path = "all/read_representation_and_download_plane.rs"]
mod read_representation_and_download_plane;
#[path = "all/recursive_loop_guardrail.rs"]
mod recursive_loop_guardrail;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/refused_value_echo.rs"]
mod refused_value_echo;
#[path = "all/regional_identifier_refusals.rs"]
mod regional_identifier_refusals;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_policy_document.rs"]
mod resource_policy_document;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_stream_invocation.rs"]
mod response_stream_invocation;
#[path = "all/response_stream_trailer.rs"]
mod response_stream_trailer;
#[path = "all/runtime_management_config.rs"]
mod runtime_management_config;
#[path = "all/s3_package_source.rs"]
mod s3_package_source;
#[path = "all/service_set_tag_keys.rs"]
mod service_set_tag_keys;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/sqs_event_source_handler_run.rs"]
mod sqs_event_source_handler_run;
#[path = "all/sqs_event_source_hold.rs"]
mod sqs_event_source_hold;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_mapping_iterator_age_series.rs"]
mod stream_mapping_iterator_age_series;
#[path = "all/synchronous_invoke_run.rs"]
mod synchronous_invoke_run;
#[path = "all/tag_key_shape.rs"]
mod tag_key_shape;
#[path = "all/tag_resource_arn_constraint.rs"]
mod tag_resource_arn_constraint;
#[path = "all/test_suite_declarations.rs"]
mod test_suite_declarations;
#[path = "all/untrusted_name_parsing.rs"]
mod untrusted_name_parsing;
#[path = "all/update_function_configuration.rs"]
mod update_function_configuration;
#[path = "all/update_window_outlasts_its_call.rs"]
mod update_window_outlasts_its_call;
#[path = "all/version_delete_alias_reference.rs"]
mod version_delete_alias_reference;
#[path = "all/version_publishing.rs"]
mod version_publishing;
#[path = "all/vpc_config_execution_role_permissions.rs"]
mod vpc_config_execution_role_permissions;
#[path = "all/async_invocation_delivery.rs"]
mod async_invocation_delivery;
#[path = "all/container_image_package_source.rs"]
mod container_image_package_source;
#[path = "all/durable_execution_completion.rs"]
mod durable_execution_completion;
#[path = "all/event_source_mapping_processed_batch.rs"]
mod event_source_mapping_processed_batch;
#[path = "all/invoke_caller_authorization.rs"]
mod invoke_caller_authorization;
#[path = "all/invoke_log_delivery.rs"]
mod invoke_log_delivery;
#[path = "all/invocation_logs.rs"]
mod invocation_logs;
// `invocation_run_capacity`, `layer_region_scope` and `update_window_outlasts_its_call` are
// NOT re-declared here: each is already declared in the sorted span above, and a second
// `mod` for one file is a redefinition (and `clippy::duplicate_mod`) that stops this target
// compiling. Check a suite is not already declared before appending it to this tail.
#[path = "all/handler_eventbridge_publication.rs"]
mod handler_eventbridge_publication;
#[path = "all/sqs_event_source_record_attributes.rs"]
mod sqs_event_source_record_attributes;
#[path = "all/xray_daemon_datagram.rs"]
mod xray_daemon_datagram;
#[path = "all/execution_role_session_identity.rs"]
mod execution_role_session_identity;
#[path = "all/handler_event_publication_authorization.rs"]
mod handler_event_publication_authorization;
#[path = "all/invocation_environment_reuse.rs"]
mod invocation_environment_reuse;
// The isolation half of the execution-environment suites (see the file's own module doc).
#[path = "all/zzprobe.rs"]
mod zzprobe;
// `invocation_run_capacity`, `layer_region_scope` and `update_window_outlasts_its_call`
// each stood here a second time: concurrent changes appended an entry for a suite the
// sorted span above already declares, which is a redefinition (`E0428`) and
// `clippy::duplicate_mod` besides, so this target did not compile. The duplicate lines are
// gone and the first declaration of each stands. Add a NEW suite in the sorted span its
// name belongs to, and check it is not declared already before appending it here.
// `function_url_reported_members` was appended here and now stands in the sorted span above,
// where its name belongs, so nothing is declared twice.
