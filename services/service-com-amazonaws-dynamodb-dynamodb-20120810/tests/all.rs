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
#[path = "all/account_capacity_limits.rs"]
mod account_capacity_limits;
#[path = "all/arn_addressed_tag_refusals.rs"]
mod arn_addressed_tag_refusals;
#[path = "all/attribute_value_admission_rules.rs"]
mod attribute_value_admission_rules;
#[path = "all/authentication_refusal_wording.rs"]
mod authentication_refusal_wording;
#[path = "all/backup_feature_snapshot_statuses.rs"]
mod backup_feature_snapshot_statuses;
#[path = "all/backup_path_table_state.rs"]
mod backup_path_table_state;
#[path = "all/batch_and_transact_items.rs"]
mod batch_and_transact_items;
#[path = "all/batch_execute_statement.rs"]
mod batch_execute_statement;
#[path = "all/batch_get_response_budget.rs"]
mod batch_get_response_budget;
#[path = "all/batch_write_capacity_residue.rs"]
mod batch_write_capacity_residue;
#[path = "all/batch_write_provisioned_budget.rs"]
mod batch_write_provisioned_budget;
#[path = "all/billing_mode_and_table_class_summaries.rs"]
mod billing_mode_and_table_class_summaries;
#[path = "all/cmk_shared_by_several_tables.rs"]
mod cmk_shared_by_several_tables;
#[path = "all/collection_cap_constraint_reports.rs"]
mod collection_cap_constraint_reports;
#[path = "all/conditioned_resource_policy_attach.rs"]
mod conditioned_resource_policy_attach;
#[path = "all/consumed_capacity_arms.rs"]
mod consumed_capacity_arms;
#[path = "all/consumed_capacity_index_metrics.rs"]
mod consumed_capacity_index_metrics;
#[path = "all/consumed_capacity_metrics.rs"]
mod consumed_capacity_metrics;
#[path = "all/continuous_backups.rs"]
mod continuous_backups;
#[path = "all/contributor_insights.rs"]
mod contributor_insights;
#[path = "all/contributor_insights_cloudwatch_rules.rs"]
mod contributor_insights_cloudwatch_rules;
#[path = "all/create_backup_source_table.rs"]
mod create_backup_source_table;
#[path = "all/delete_table_description_projection.rs"]
mod delete_table_description_projection;
#[path = "all/describe_continuous_backups_reads.rs"]
mod describe_continuous_backups_reads;
#[path = "all/describe_contributor_insights_reports.rs"]
mod describe_contributor_insights_reports;
#[path = "all/describe_time_to_live_addressing.rs"]
mod describe_time_to_live_addressing;
#[path = "all/disable_kinesis_streaming_destination.rs"]
mod disable_kinesis_streaming_destination;
#[path = "all/element_expression_envelopes.rs"]
mod element_expression_envelopes;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/enable_kinesis_streaming_destination.rs"]
mod enable_kinesis_streaming_destination;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/endpoint_discovery.rs"]
mod endpoint_discovery;
#[path = "all/endpoint_front_door.rs"]
mod endpoint_front_door;
#[path = "all/export_conflicts_and_capacity_no_ops.rs"]
mod export_conflicts_and_capacity_no_ops;
#[path = "all/export_recovery_precondition.rs"]
mod export_recovery_precondition;
#[path = "all/expression_parameter_generations.rs"]
mod expression_parameter_generations;
#[path = "all/expression_refusal_families.rs"]
mod expression_refusal_families;
#[path = "all/expression_rules_and_write_admission.rs"]
mod expression_rules_and_write_admission;
#[path = "all/global_secondary_indexes.rs"]
mod global_secondary_indexes;
#[path = "all/global_tables.rs"]
mod global_tables;
#[path = "all/import_item_validation_outcomes.rs"]
mod import_item_validation_outcomes;
#[path = "all/index_and_projection_reads.rs"]
mod index_and_projection_reads;
#[path = "all/index_capacity_and_item_collections.rs"]
mod index_capacity_and_item_collections;
#[path = "all/index_key_types_and_range_refusals.rs"]
mod index_key_types_and_range_refusals;
#[path = "all/index_projection_and_path_reports.rs"]
mod index_projection_and_path_reports;
#[path = "all/index_write_capacity_changes.rs"]
mod index_write_capacity_changes;
#[path = "all/item_write_expressions.rs"]
mod item_write_expressions;
#[path = "all/item_write_vocabularies.rs"]
mod item_write_vocabularies;
#[path = "all/key_refusal_classes_and_batch_bounds.rs"]
mod key_refusal_classes_and_batch_bounds;
#[path = "all/key_schema_and_vector_parameter_refusals.rs"]
mod key_schema_and_vector_parameter_refusals;
#[path = "all/key_schema_element_refusals.rs"]
mod key_schema_element_refusals;
#[path = "all/kinesis_destination_consumer_positions.rs"]
mod kinesis_destination_consumer_positions;
#[path = "all/kinesis_destination_delivery.rs"]
mod kinesis_destination_delivery;
#[path = "all/kinesis_destination_record_delivery.rs"]
mod kinesis_destination_record_delivery;
#[path = "all/kinesis_destination_region_inventory.rs"]
mod kinesis_destination_region_inventory;
#[path = "all/kinesis_destination_state_refusals.rs"]
mod kinesis_destination_state_refusals;
#[path = "all/kinesis_destination_stream_resolution.rs"]
mod kinesis_destination_stream_resolution;
#[path = "all/kinesis_streaming_destinations.rs"]
mod kinesis_streaming_destinations;
#[path = "all/kms_key_state_refusals.rs"]
mod kms_key_state_refusals;
#[path = "all/legacy_conditional_operator_refusals.rs"]
mod legacy_conditional_operator_refusals;
#[path = "all/limit_reached_page_stop_keys.rs"]
mod limit_reached_page_stop_keys;
#[path = "all/list_exports.rs"]
mod list_exports;
#[path = "all/list_imports.rs"]
mod list_imports;
#[path = "all/multi_item_key_defects_and_precedence.rs"]
mod multi_item_key_defects_and_precedence;
#[path = "all/new_table_polling.rs"]
mod new_table_polling;
#[path = "all/number_canonicalization.rs"]
mod number_canonicalization;
#[path = "all/on_demand_backups.rs"]
mod on_demand_backups;
#[path = "all/on_demand_throughput_capacity_mode.rs"]
mod on_demand_throughput_capacity_mode;
#[path = "all/paging_start_keys.rs"]
mod paging_start_keys;
#[path = "all/parallel_scan_segment_refusals.rs"]
mod parallel_scan_segment_refusals;
#[path = "all/partiql_non_dml_statements.rs"]
mod partiql_non_dml_statements;
#[path = "all/partiql_refusal_wording.rs"]
mod partiql_refusal_wording;
#[path = "all/partiql_statement_boundaries.rs"]
mod partiql_statement_boundaries;
#[path = "all/partiql_statement_defect_classes.rs"]
mod partiql_statement_defect_classes;
#[path = "all/partiql_statements.rs"]
mod partiql_statements;
#[path = "all/partiql_transaction_responses.rs"]
mod partiql_transaction_responses;
#[path = "all/partiql_transactions.rs"]
mod partiql_transactions;
#[path = "all/partiql_unsupported_operations.rs"]
mod partiql_unsupported_operations;
#[path = "all/point_in_time_item_history.rs"]
mod point_in_time_item_history;
#[path = "all/probe.rs"]
mod probe;
#[path = "all/provisioned_capacity_metrics.rs"]
mod provisioned_capacity_metrics;
#[path = "all/provisioned_read_capacity.rs"]
mod provisioned_read_capacity;
#[path = "all/query_indexes_and_projections.rs"]
mod query_indexes_and_projections;
#[path = "all/query_mixed_parameter_order.rs"]
mod query_mixed_parameter_order;
#[path = "all/refusal_contracts_and_expression_pre_images.rs"]
mod refusal_contracts_and_expression_pre_images;
#[path = "all/required_member_refusals.rs"]
mod required_member_refusals;
#[path = "all/request_latency_metrics.rs"]
mod request_latency_metrics;
#[path = "all/request_shape_validation_classes.rs"]
mod request_shape_validation_classes;
#[path = "all/request_validation_refusals.rs"]
mod request_validation_refusals;
#[path = "all/region_from_the_request_endpoint.rs"]
mod region_from_the_request_endpoint;
#[path = "all/regions_indexes_and_capacity.rs"]
mod regions_indexes_and_capacity;
#[path = "all/replica_auto_scaling.rs"]
mod replica_auto_scaling;
#[path = "all/resource_policy_arn_scope_refusals.rs"]
mod resource_policy_arn_scope_refusals;
#[path = "all/resource_policy_conditions_and_read_lag.rs"]
mod resource_policy_conditions_and_read_lag;
#[path = "all/resource_policy_default_version.rs"]
mod resource_policy_default_version;
#[path = "all/resource_policy_not_found_messages.rs"]
mod resource_policy_not_found_messages;
#[path = "all/resource_policy_revision_tokens.rs"]
mod resource_policy_revision_tokens;
#[path = "all/resource_policy_stored_version_round_trip.rs"]
mod resource_policy_stored_version_round_trip;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/restore_table_from_backup.rs"]
mod restore_table_from_backup;
#[path = "all/resume_token_schema_refusals.rs"]
mod resume_token_schema_refusals;
#[path = "all/scan_filter_value_types_and_segments.rs"]
mod scan_filter_value_types_and_segments;
#[path = "all/scan_indexes_and_projections.rs"]
mod scan_indexes_and_projections;
#[path = "all/scan_segment_partitions.rs"]
mod scan_segment_partitions;
#[path = "all/schema_duplicates_and_capacity_floors.rs"]
mod schema_duplicates_and_capacity_floors;
#[path = "all/select_rules_and_policy_resources.rs"]
mod select_rules_and_policy_resources;
#[path = "all/result_filter_expressions.rs"]
mod result_filter_expressions;
#[path = "all/restore_table_to_point_in_time.rs"]
mod restore_table_to_point_in_time;
#[path = "all/start_key_boundary_refusals.rs"]
mod start_key_boundary_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_checkpoint_resume.rs"]
mod stream_checkpoint_resume;
#[path = "all/stream_enabled_table_round_trip.rs"]
mod stream_enabled_table_round_trip;
#[path = "all/stream_identity_is_minted_once.rs"]
mod stream_identity_is_minted_once;
#[path = "all/stream_record_images.rs"]
mod stream_record_images;
#[path = "all/stream_specification_presence.rs"]
mod stream_specification_presence;
#[path = "all/stream_specification_with_other_members.rs"]
mod stream_specification_with_other_members;
#[path = "all/table_control_plane.rs"]
mod table_control_plane;
#[path = "all/table_declaration_admission_rules.rs"]
mod table_declaration_admission_rules;
#[path = "all/table_description_members.rs"]
mod table_description_members;
#[path = "all/table_description_shape.rs"]
mod table_description_shape;
#[path = "all/table_exports.rs"]
mod table_exports;
#[path = "all/table_imports.rs"]
mod table_imports;
#[path = "all/table_encryption_and_ceilings.rs"]
mod table_encryption_and_ceilings;
#[path = "all/table_items.rs"]
mod table_items;
#[path = "all/table_replicas.rs"]
mod table_replicas;
#[path = "all/table_scan.rs"]
mod table_scan;
#[path = "all/table_streams.rs"]
mod table_streams;
#[path = "all/transact_write_expression_parse_defects.rs"]
mod transact_write_expression_parse_defects;
#[path = "all/table_throughput_mode_and_warm_capacity.rs"]
mod table_throughput_mode_and_warm_capacity;
#[path = "all/tag_read_propagation.rs"]
mod tag_read_propagation;
#[path = "all/time_to_live.rs"]
mod time_to_live;
#[path = "all/transactional_consumed_capacity_metrics.rs"]
mod transactional_consumed_capacity_metrics;
#[path = "all/transactional_undefined_expression_tokens.rs"]
mod transactional_undefined_expression_tokens;
#[path = "all/transaction_token_fingerprints.rs"]
mod transaction_token_fingerprints;
#[path = "all/transitional_status_span.rs"]
mod transitional_status_span;
#[path = "all/undefined_expression_tokens.rs"]
mod undefined_expression_tokens;
#[path = "all/unused_substitution_key_envelopes.rs"]
mod unused_substitution_key_envelopes;
#[path = "all/update_contributor_insights_writes.rs"]
mod update_contributor_insights_writes;
#[path = "all/update_operand_type_sets.rs"]
mod update_operand_type_sets;
#[path = "all/update_table_transient_status.rs"]
mod update_table_transient_status;
#[path = "all/validation_message_fidelity.rs"]
mod validation_message_fidelity;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
#[path = "all/value_rules_and_resource_policy.rs"]
mod value_rules_and_resource_policy;
#[path = "all/vector_index_updates.rs"]
mod vector_index_updates;
#[path = "all/vector_indexes.rs"]
mod vector_indexes;
#[path = "all/vector_search.rs"]
mod vector_search;
#[path = "all/vector_write_capacity_cost.rs"]
mod vector_write_capacity_cost;
#[path = "all/vector_write_validation.rs"]
mod vector_write_validation;
#[path = "all/legacy_filters_and_statement_values.rs"]
mod legacy_filters_and_statement_values;
