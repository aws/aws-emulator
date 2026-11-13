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
#[path = "all/arn_minting.rs"]
mod arn_minting;
#[path = "all/audit_records.rs"]
mod audit_records;
#[path = "all/concurrent_execution_idempotency.rs"]
mod concurrent_execution_idempotency;
#[path = "all/constrained_text_grammars.rs"]
mod constrained_text_grammars;
#[path = "all/create_report_client_token.rs"]
mod create_report_client_token;
#[path = "all/create_report_conflict_message.rs"]
mod create_report_conflict_message;
#[path = "all/create_report_idempotent_replay.rs"]
mod create_report_idempotent_replay;
#[path = "all/create_report_token_spent.rs"]
mod create_report_token_spent;
#[path = "all/create_time_tag_merge.rs"]
mod create_time_tag_merge;
#[path = "all/cursor_spelling.rs"]
mod cursor_spelling;
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;
#[path = "all/dashboard_name_uniqueness.rs"]
mod dashboard_name_uniqueness;
#[path = "all/date_time_value_message_bound.rs"]
mod date_time_value_message_bound;
#[path = "all/delete_dashboard_idempotency.rs"]
mod delete_dashboard_idempotency;
#[path = "all/delete_scheduled_report.rs"]
mod delete_scheduled_report;
#[path = "all/error_message_length_bound.rs"]
mod error_message_length_bound;
#[path = "all/execution_token_scope.rs"]
mod execution_token_scope;
#[path = "all/listing_pagination.rs"]
mod listing_pagination;
#[path = "all/managed_dashboard_catalog.rs"]
mod managed_dashboard_catalog;
#[path = "all/name_pattern_enforcement.rs"]
mod name_pattern_enforcement;
#[path = "all/range_refusal_wording.rs"]
mod range_refusal_wording;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_not_found.rs"]
mod resource_not_found;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/schedule_period_ceiling.rs"]
mod schedule_period_ceiling;
#[path = "all/schedule_period_defaults.rs"]
mod schedule_period_defaults;
#[path = "all/schedule_period_start_time.rs"]
mod schedule_period_start_time;
#[path = "all/scheduled_report_config_validation.rs"]
mod scheduled_report_config_validation;
#[path = "all/scheduled_report_execution.rs"]
mod scheduled_report_execution;
#[path = "all/scheduled_report_execution_events.rs"]
mod scheduled_report_execution_events;
#[path = "all/scheduled_report_execution_role.rs"]
mod scheduled_report_execution_role;
#[path = "all/scheduled_report_execution_role_permissions.rs"]
mod scheduled_report_execution_role_permissions;
#[path = "all/scheduled_report_health_derivation.rs"]
mod scheduled_report_health_derivation;
#[path = "all/scheduled_report_health_views.rs"]
mod scheduled_report_health_views;
#[path = "all/scheduled_report_lifecycle.rs"]
mod scheduled_report_lifecycle;
#[path = "all/scheduled_report_listing.rs"]
mod scheduled_report_listing;
#[path = "all/shared_arn_cursor_parse.rs"]
mod shared_arn_cursor_parse;
#[path = "all/shared_media_type_rule.rs"]
mod shared_media_type_rule;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_dashboard_partial_update.rs"]
mod update_dashboard_partial_update;
#[path = "all/update_schedule_config_merge.rs"]
mod update_schedule_config_merge;
#[path = "all/update_schedule_state_preservation.rs"]
mod update_schedule_state_preservation;
#[path = "all/update_scheduled_report.rs"]
mod update_scheduled_report;
#[path = "all/validation_pattern_rendering.rs"]
mod validation_pattern_rendering;
#[path = "all/widget_metric_validation.rs"]
mod widget_metric_validation;
#[path = "all/widget_time_range_validation.rs"]
mod widget_time_range_validation;
