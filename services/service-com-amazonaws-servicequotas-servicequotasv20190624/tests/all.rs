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
#[path = "all/api_rate_quota_rows.rs"]
mod api_rate_quota_rows;
#[path = "all/applied_catalog_published_rows.rs"]
mod applied_catalog_published_rows;
#[path = "all/applied_level_filter.rs"]
mod applied_level_filter;
#[path = "all/applied_quota_listing_pages.rs"]
mod applied_quota_listing_pages;
#[path = "all/applied_quota_records.rs"]
mod applied_quota_records;
#[path = "all/auto_management.rs"]
mod auto_management;
#[path = "all/auto_management_configuration.rs"]
mod auto_management_configuration;
#[path = "all/auto_management_exclusions.rs"]
mod auto_management_exclusions;
#[path = "all/auto_management_opt_in_type.rs"]
mod auto_management_opt_in_type;
#[path = "all/auto_management_stop.rs"]
mod auto_management_stop;
#[path = "all/aws_minted_catalog_coverage.rs"]
mod aws_minted_catalog_coverage;
#[path = "all/aws_quota_metadata_per_code.rs"]
mod aws_quota_metadata_per_code;
#[path = "all/catalog_identity.rs"]
mod catalog_identity;
#[path = "all/catalog_pagination.rs"]
mod catalog_pagination;
#[path = "all/cognito_user_pool_default_quotas.rs"]
mod cognito_user_pool_default_quotas;
#[path = "all/context_id_domain.rs"]
mod context_id_domain;
#[path = "all/cross_region_settlement.rs"]
mod cross_region_settlement;
#[path = "all/default_catalogs_batch_through_codepipeline.rs"]
mod default_catalogs_batch_through_codepipeline;
#[path = "all/default_catalogs_keyed_on_aws_codes.rs"]
mod default_catalogs_keyed_on_aws_codes;
#[path = "all/default_catalogs_sns_through_xray.rs"]
mod default_catalogs_sns_through_xray;
#[path = "all/default_quota_catalog.rs"]
mod default_quota_catalog;
#[path = "all/default_quota_catalog_fidelity.rs"]
mod default_quota_catalog_fidelity;
#[path = "all/default_quota_code_binding.rs"]
mod default_quota_code_binding;
#[path = "all/default_quota_published_codes.rs"]
mod default_quota_published_codes;
#[path = "all/default_quota_recorded_bindings.rs"]
mod default_quota_recorded_bindings;
#[path = "all/default_quota_row_text.rs"]
mod default_quota_row_text;
#[path = "all/default_quota_rows_under_aws_codes.rs"]
mod default_quota_rows_under_aws_codes;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/history_filter_chain.rs"]
mod history_filter_chain;
#[path = "all/history_token_under_a_moving_selection.rs"]
mod history_token_under_a_moving_selection;
#[path = "all/integrated_service_display_names.rs"]
mod integrated_service_display_names;
#[path = "all/no_such_quota_wording.rs"]
mod no_such_quota_wording;
#[path = "all/no_such_service_wording.rs"]
mod no_such_service_wording;
#[path = "all/open_request_plane_scope.rs"]
mod open_request_plane_scope;
#[path = "all/pagination_token_position.rs"]
mod pagination_token_position;
#[path = "all/pagination_token_scope.rs"]
mod pagination_token_scope;
#[path = "all/published_default_quota_codes.rs"]
mod published_default_quota_codes;
#[path = "all/published_quota_codes.rs"]
mod published_quota_codes;
#[path = "all/published_row_class_wording.rs"]
mod published_row_class_wording;
#[path = "all/quota_plane_rule.rs"]
mod quota_plane_rule;
#[path = "all/quota_request_template.rs"]
mod quota_request_template;
#[path = "all/quota_utilization_usage.rs"]
mod quota_utilization_usage;
#[path = "all/quota_workflow.rs"]
mod quota_workflow;
#[path = "all/quotas_catalog.rs"]
mod quotas_catalog;
#[path = "all/recorded_default_catalog_rows.rs"]
mod recorded_default_catalog_rows;
#[path = "all/recorded_rows_access_analyzer_through_amplify.rs"]
mod recorded_rows_access_analyzer_through_amplify;
#[path = "all/recorded_rows_config_through_signin.rs"]
mod recorded_rows_config_through_signin;
#[path = "all/recorded_rows_events_through_imagebuilder.rs"]
mod recorded_rows_events_through_imagebuilder;
#[path = "all/refusal_message_contract.rs"]
mod refusal_message_contract;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/report_token_before_status.rs"]
mod report_token_before_status;
#[path = "all/request_id_resolution_scope.rs"]
mod request_id_resolution_scope;
#[path = "all/requested_change_history.rs"]
mod requested_change_history;
#[path = "all/resource_quota_context.rs"]
mod resource_quota_context;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_code_coverage.rs"]
mod service_code_coverage;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/support_case_and_templates.rs"]
mod support_case_and_templates;
#[path = "all/support_case_lifecycle.rs"]
mod support_case_lifecycle;
#[path = "all/support_cases.rs"]
mod support_cases;
#[path = "all/tags.rs"]
mod tags;
#[path = "all/timestream_rate_class.rs"]
mod timestream_rate_class;
#[path = "all/unasked_page_size.rs"]
mod unasked_page_size;
#[path = "all/utilization_report.rs"]
mod utilization_report;
#[path = "all/value_in_force_single_home.rs"]
mod value_in_force_single_home;
