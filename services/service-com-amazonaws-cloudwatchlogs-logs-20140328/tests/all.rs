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
#[path = "all/account_policies.rs"]
mod account_policies;
#[path = "all/account_policy_release.rs"]
mod account_policy_release;
#[path = "all/account_policy_removal.rs"]
mod account_policy_removal;
#[path = "all/account_policy_selection_criteria.rs"]
mod account_policy_selection_criteria;
#[path = "all/anomaly_suppression.rs"]
mod anomaly_suppression;
#[path = "all/cmk_group_delivery_byte_fidelity.rs"]
mod cmk_group_delivery_byte_fidelity;
#[path = "all/cmk_group_fan_out_isolation.rs"]
mod cmk_group_fan_out_isolation;
#[path = "all/cmk_key_policy_scope.rs"]
mod cmk_key_policy_scope;
#[path = "all/configuration_templates.rs"]
mod configuration_templates;
#[path = "all/dangling_query_commands.rs"]
mod dangling_query_commands;
#[path = "all/data_protection_masking.rs"]
mod data_protection_masking;
#[path = "all/data_protection_policies.rs"]
mod data_protection_policies;
#[path = "all/data_protection_policy.rs"]
mod data_protection_policy;
#[path = "all/deletion_protection.rs"]
mod deletion_protection;
#[path = "all/deliveries.rs"]
mod deliveries;
#[path = "all/delivery_configuration_updates.rs"]
mod delivery_configuration_updates;
#[path = "all/delivery_creation.rs"]
mod delivery_creation;
#[path = "all/delivery_destination_enumeration.rs"]
mod delivery_destination_enumeration;
#[path = "all/delivery_destination_policies.rs"]
mod delivery_destination_policies;
#[path = "all/delivery_destination_policy_reads.rs"]
mod delivery_destination_policy_reads;
#[path = "all/delivery_destination_policy_removal.rs"]
mod delivery_destination_policy_removal;
#[path = "all/delivery_destinations.rs"]
mod delivery_destinations;
#[path = "all/delivery_effective_configuration.rs"]
mod delivery_effective_configuration;
#[path = "all/delivery_idempotence.rs"]
mod delivery_idempotence;
#[path = "all/delivery_identity_authorization.rs"]
mod delivery_identity_authorization;
#[path = "all/delivery_object_layout_defaults.rs"]
mod delivery_object_layout_defaults;
#[path = "all/delivery_region_isolation.rs"]
mod delivery_region_isolation;
#[path = "all/delivery_resource_in_use_refusals.rs"]
mod delivery_resource_in_use_refusals;
#[path = "all/delivery_role_trust_spellings.rs"]
mod delivery_role_trust_spellings;
#[path = "all/delivery_source_log_types.rs"]
mod delivery_source_log_types;
#[path = "all/delivery_source_region_support.rs"]
mod delivery_source_region_support;
#[path = "all/delivery_source_resource_resolution.rs"]
mod delivery_source_resource_resolution;
#[path = "all/delivery_source_resource_pairings.rs"]
mod delivery_source_resource_pairings;
#[path = "all/delivery_sources.rs"]
mod delivery_sources;
#[path = "all/destination_access_authorization.rs"]
mod destination_access_authorization;
#[path = "all/destination_policies.rs"]
mod destination_policies;
#[path = "all/destination_removal.rs"]
mod destination_removal;
#[path = "all/describe_log_groups_filters.rs"]
mod describe_log_groups_filters;
#[path = "all/destinations.rs"]
mod destinations;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/event_cursor_envelope.rs"]
mod event_cursor_envelope;
#[path = "all/event_ingestion_time_independence.rs"]
mod event_ingestion_time_independence;
#[path = "all/event_window_selection.rs"]
mod event_window_selection;
#[path = "all/export_task_account_slot.rs"]
mod export_task_account_slot;
#[path = "all/export_task_active_window.rs"]
mod export_task_active_window;
#[path = "all/export_task_cancellation.rs"]
mod export_task_cancellation;
#[path = "all/export_task_objects.rs"]
mod export_task_objects;
#[path = "all/export_task_quota_delivery.rs"]
mod export_task_quota_delivery;
#[path = "all/export_task_status_messages.rs"]
mod export_task_status_messages;
#[path = "all/export_tasks.rs"]
mod export_tasks;
#[path = "all/export_time_windows.rs"]
mod export_time_windows;
#[path = "all/field_indexes.rs"]
mod field_indexes;
#[path = "all/filter_log_events.rs"]
mod filter_log_events;
#[path = "all/filter_log_events_search_reporting.rs"]
mod filter_log_events_search_reporting;
#[path = "all/fixed_text_refusals.rs"]
mod fixed_text_refusals;
#[path = "all/get_log_object_authorization.rs"]
mod get_log_object_authorization;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_policy_authorization.rs"]
mod identity_policy_authorization;
#[path = "all/import_task_creation.rs"]
mod import_task_creation;
#[path = "all/ingestion_metrics.rs"]
mod ingestion_metrics;
#[path = "all/import_tasks.rs"]
mod import_tasks;
#[path = "all/index_policies.rs"]
mod index_policies;
#[path = "all/index_policy_puts.rs"]
mod index_policy_puts;
#[path = "all/index_policy_removal.rs"]
mod index_policy_removal;
#[path = "all/ingestion_window.rs"]
mod ingestion_window;
#[path = "all/insights_queries.rs"]
mod insights_queries;
#[path = "all/integrations.rs"]
mod integrations;
#[path = "all/json_filter_pattern_element_selectors.rs"]
mod json_filter_pattern_element_selectors;
#[path = "all/json_filter_pattern_value_types.rs"]
mod json_filter_pattern_value_types;
#[path = "all/kms_key_resolution.rs"]
mod kms_key_resolution;
#[path = "all/list_anomalies.rs"]
mod list_anomalies;
#[path = "all/live_tail_sessions.rs"]
mod live_tail_sessions;
#[path = "all/log_anomaly_detector_enumeration.rs"]
mod log_anomaly_detector_enumeration;
#[path = "all/log_anomaly_detector_removal.rs"]
mod log_anomaly_detector_removal;
#[path = "all/log_anomaly_detector_updates.rs"]
mod log_anomaly_detector_updates;
#[path = "all/log_anomaly_detectors.rs"]
mod log_anomaly_detectors;
#[path = "all/log_core_loop.rs"]
mod log_core_loop;
#[path = "all/log_fields.rs"]
mod log_fields;
#[path = "all/log_group_auth_and_encryption.rs"]
mod log_group_auth_and_encryption;
#[path = "all/log_group_fields.rs"]
mod log_group_fields;
#[path = "all/log_group_name_pattern_lists.rs"]
mod log_group_name_pattern_lists;
#[path = "all/log_group_pattern_projection.rs"]
mod log_group_pattern_projection;
#[path = "all/log_group_tags.rs"]
mod log_group_tags;
#[path = "all/log_groups_for_query.rs"]
mod log_groups_for_query;
#[path = "all/log_objects.rs"]
mod log_objects;
#[path = "all/log_records.rs"]
mod log_records;
#[path = "all/log_stream_removal.rs"]
mod log_stream_removal;
#[path = "all/lookup_table_column_names.rs"]
mod lookup_table_column_names;
#[path = "all/lookup_table_enumeration.rs"]
mod lookup_table_enumeration;
#[path = "all/lookup_table_queries.rs"]
mod lookup_table_queries;
#[path = "all/lookup_table_removal.rs"]
mod lookup_table_removal;
#[path = "all/lookup_table_tagging.rs"]
mod lookup_table_tagging;
#[path = "all/lookup_table_updates.rs"]
mod lookup_table_updates;
#[path = "all/lookup_tables.rs"]
mod lookup_tables;
#[path = "all/metric_filter_dimension_refusal_wording.rs"]
mod metric_filter_dimension_refusal_wording;
#[path = "all/metric_filter_enumeration.rs"]
mod metric_filter_enumeration;
#[path = "all/metric_filter_extraction.rs"]
mod metric_filter_extraction;
#[path = "all/metric_filter_testing.rs"]
mod metric_filter_testing;
#[path = "all/metric_filters.rs"]
mod metric_filters;
#[path = "all/metric_transformation_exclusion_wording.rs"]
mod metric_transformation_exclusion_wording;
#[path = "all/missing_destination_refusal.rs"]
mod missing_destination_refusal;
#[path = "all/parameter_pair_refusals.rs"]
mod parameter_pair_refusals;
#[path = "all/put_log_events_batch_constraints.rs"]
mod put_log_events_batch_constraints;
#[path = "all/probe_scratch.rs"]
mod probe_scratch;
#[path = "all/query_and_stream_refusal_text.rs"]
mod query_and_stream_refusal_text;
#[path = "all/query_cancellation.rs"]
mod query_cancellation;
#[path = "all/query_compilation.rs"]
mod query_compilation;
#[path = "all/query_definitions.rs"]
mod query_definitions;
#[path = "all/query_language_admission.rs"]
mod query_language_admission;
#[path = "all/query_results.rs"]
mod query_results;
#[path = "all/query_row_limit_and_handle_refusals.rs"]
mod query_row_limit_and_handle_refusals;
#[path = "all/query_window_refusal.rs"]
mod query_window_refusal;
#[path = "all/refusal_error_codes.rs"]
mod refusal_error_codes;
#[path = "all/refusal_message_fidelity.rs"]
mod refusal_message_fidelity;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/refusal_text_variable_parts.rs"]
mod refusal_text_variable_parts;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_policy_revision_ids.rs"]
mod resource_policy_revision_ids;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/retention_policy.rs"]
mod retention_policy;
#[path = "all/s3_table_integration_sources.rs"]
mod s3_table_integration_sources;
#[path = "all/scheduled_queries.rs"]
mod scheduled_queries;
#[path = "all/scheduled_query_configuration_rules.rs"]
mod scheduled_query_configuration_rules;
#[path = "all/scheduled_query_enumeration.rs"]
mod scheduled_query_enumeration;
#[path = "all/scheduled_query_history.rs"]
mod scheduled_query_history;
#[path = "all/scheduled_query_identifier_resolution.rs"]
mod scheduled_query_identifier_resolution;
#[path = "all/scheduled_query_insights_start.rs"]
mod scheduled_query_insights_start;
#[path = "all/scheduled_query_name_conflict.rs"]
mod scheduled_query_name_conflict;
#[path = "all/scheduled_query_removal.rs"]
mod scheduled_query_removal;
#[path = "all/scheduled_query_result_delivery.rs"]
mod scheduled_query_result_delivery;
#[path = "all/scheduled_query_schedule_expressions.rs"]
mod scheduled_query_schedule_expressions;
#[path = "all/scheduled_query_updates.rs"]
mod scheduled_query_updates;
#[path = "all/scratch_queries.rs"]
mod scratch_queries;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_tier_policy.rs"]
mod storage_tier_policy;
#[path = "all/subscription_delivery_authorization.rs"]
mod subscription_delivery_authorization;
#[path = "all/subscription_delivery_telemetry.rs"]
mod subscription_delivery_telemetry;
#[path = "all/subscription_destination_health_check.rs"]
mod subscription_destination_health_check;
#[path = "all/subscription_destination_plumbing.rs"]
mod subscription_destination_plumbing;
#[path = "all/subscription_fan_out.rs"]
mod subscription_fan_out;
#[path = "all/subscription_filter_delivery.rs"]
mod subscription_filter_delivery;
#[path = "all/subscription_filter_independence.rs"]
mod subscription_filter_independence;
#[path = "all/subscription_filter_lambda_permission.rs"]
mod subscription_filter_lambda_permission;
#[path = "all/subscription_filters.rs"]
mod subscription_filters;
#[path = "all/syslog_configurations.rs"]
mod syslog_configurations;
#[path = "all/syslog_vpc_endpoint_validation.rs"]
mod syslog_vpc_endpoint_validation;
#[path = "all/transformed_log_filters.rs"]
mod transformed_log_filters;
#[path = "all/transformed_subscription_delivery.rs"]
mod transformed_subscription_delivery;
#[path = "all/transformers.rs"]
mod transformers;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/vended_log_template_catalog_rows.rs"]
mod vended_log_template_catalog_rows;
#[path = "all/vended_log_template_schemas.rs"]
mod vended_log_template_schemas;
#[path = "all/vended_parser_position.rs"]
mod vended_parser_position;
#[path = "all/zz_probe_defaults.rs"]
mod zz_probe_defaults;
