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
#[path = "all/aws_refusal_wording.rs"]
mod aws_refusal_wording;

#[path = "all/enablement_then_period_bucketing.rs"]
mod enablement_then_period_bucketing;

#[path = "all/encryption_config.rs"]
mod encryption_config;

#[path = "all/encryption_key_resolution.rs"]
mod encryption_key_resolution;

#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;

#[path = "all/filter_expression_entity_ids.rs"]
mod filter_expression_entity_ids;

#[path = "all/filter_expression_unary_forms.rs"]
mod filter_expression_unary_forms;

#[path = "all/graphs_rules_retrievals_and_insights.rs"]
mod graphs_rules_retrievals_and_insights;

#[path = "all/group_control_plane.rs"]
mod group_control_plane;

#[path = "all/group_identifier_exclusivity.rs"]
mod group_identifier_exclusivity;

#[path = "all/indexing_rules.rs"]
mod indexing_rules;

#[path = "all/inferred_service_type.rs"]
mod inferred_service_type;

#[path = "all/insight_impact_graph.rs"]
mod insight_impact_graph;

#[path = "all/insight_summaries.rs"]
mod insight_summaries;

#[path = "all/malformed_retrieval_token_message.rs"]
mod malformed_retrieval_token_message;

#[path = "all/query_surface_selection.rs"]
mod query_surface_selection;

#[path = "all/quoted_collection_clause.rs"]
mod quoted_collection_clause;

#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;

#[path = "all/request_deserialization.rs"]
mod request_deserialization;

#[path = "all/request_validation_before_resolution.rs"]
mod request_validation_before_resolution;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/retrieval_token_case_sensitivity.rs"]
mod retrieval_token_case_sensitivity;

#[path = "all/retrieval_token_exact_byte_integrity.rs"]
mod retrieval_token_exact_byte_integrity;

#[path = "all/retrieval_token_integrity_classification.rs"]
mod retrieval_token_integrity_classification;

#[path = "all/root_cause_attribution.rs"]
mod root_cause_attribution;

#[path = "all/sampling_rule_default_and_update.rs"]
mod sampling_rule_default_and_update;

#[path = "all/sampling_rule_deletion.rs"]
mod sampling_rule_deletion;

#[path = "all/sampling_rule_fixed_refusals.rs"]
mod sampling_rule_fixed_refusals;

#[path = "all/sampling_rules.rs"]
mod sampling_rules;

#[path = "all/sampling_target_cadence_and_processed_window.rs"]
mod sampling_target_cadence_and_processed_window;

#[path = "all/sampling_targets.rs"]
mod sampling_targets;

#[path = "all/sampling_targets_for_a_divided_reservoir.rs"]
mod sampling_targets_for_a_divided_reservoir;

#[path = "all/service_graph.rs"]
mod service_graph;

#[path = "all/service_map_reachability.rs"]
mod service_map_reachability;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/telemetry_records.rs"]
mod telemetry_records;

#[path = "all/terminal_cancel_and_reported_windows.rs"]
mod terminal_cancel_and_reported_windows;

#[path = "all/time_series_service_statistics.rs"]
mod time_series_service_statistics;

#[path = "all/trace_data_plane_rules.rs"]
mod trace_data_plane_rules;

#[path = "all/trace_graph.rs"]
mod trace_graph;

#[path = "all/trace_ingest_and_groups.rs"]
mod trace_ingest_and_groups;

#[path = "all/trace_query_bounds_and_group_scope.rs"]
mod trace_query_bounds_and_group_scope;

#[path = "all/trace_retrieval_lifecycle.rs"]
mod trace_retrieval_lifecycle;

#[path = "all/trace_segment_destination.rs"]
mod trace_segment_destination;

#[path = "all/transaction_search_span_delivery.rs"]
mod transaction_search_span_delivery;

#[path = "all/unconvertible_time_member.rs"]
mod unconvertible_time_member;

#[path = "all/unissued_pagination_token_message.rs"]
mod unissued_pagination_token_message;

#[path = "all/update_group.rs"]
mod update_group;

#[path = "all/xray_wire.rs"]
mod xray_wire;
