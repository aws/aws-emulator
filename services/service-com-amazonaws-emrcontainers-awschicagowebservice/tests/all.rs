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
#[path = "all/cluster_scoped_listing_scope.rs"]
mod cluster_scoped_listing_scope;
#[path = "all/created_metadata_wire_form.rs"]
mod created_metadata_wire_form;
#[path = "all/empty_collection_member.rs"]
mod empty_collection_member;
#[path = "all/job_driver_exactly_one.rs"]
mod job_driver_exactly_one;
#[path = "all/job_run_completion.rs"]
mod job_run_completion;
#[path = "all/job_run_outcome.rs"]
mod job_run_outcome;
#[path = "all/job_template_idempotency.rs"]
mod job_template_idempotency;
#[path = "all/job_template_lifecycle.rs"]
mod job_template_lifecycle;
#[path = "all/list_cursor_canonical_spelling.rs"]
mod list_cursor_canonical_spelling;
#[path = "all/list_cursor_region_scope.rs"]
mod list_cursor_region_scope;
#[path = "all/managed_endpoint_connection.rs"]
mod managed_endpoint_connection;
#[path = "all/managed_endpoint_create_admission.rs"]
mod managed_endpoint_create_admission;
#[path = "all/managed_endpoint_delete.rs"]
mod managed_endpoint_delete;
#[path = "all/managed_endpoint_describe.rs"]
mod managed_endpoint_describe;
#[path = "all/managed_endpoint_listing.rs"]
mod managed_endpoint_listing;
#[path = "all/managed_endpoint_session_credentials.rs"]
mod managed_endpoint_session_credentials;
#[path = "all/managed_endpoint_wire_form.rs"]
mod managed_endpoint_wire_form;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/paginated_page_bounds.rs"]
mod paginated_page_bounds;
#[path = "all/refusal_message_bound.rs"]
mod refusal_message_bound;
#[path = "all/refusal_wording_by_error_class.rs"]
mod refusal_wording_by_error_class;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/security_configuration.rs"]
mod security_configuration;
#[path = "all/security_configuration_delete.rs"]
mod security_configuration_delete;
#[path = "all/security_configuration_in_use.rs"]
mod security_configuration_in_use;
#[path = "all/security_configuration_list.rs"]
mod security_configuration_list;
#[path = "all/shared_listing_filters.rs"]
mod shared_listing_filters;
#[path = "all/spent_client_token_rule.rs"]
mod spent_client_token_rule;
#[path = "all/spent_token_with_a_different_request.rs"]
mod spent_token_with_a_different_request;
#[path = "all/start_job_run_job_template.rs"]
mod start_job_run_job_template;
#[path = "all/start_job_run_required_members.rs"]
mod start_job_run_required_members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/state_transition_refusal_wording.rs"]
mod state_transition_refusal_wording;
#[path = "all/tagging_resource_arn_form.rs"]
mod tagging_resource_arn_form;
#[path = "all/virtual_cluster_job_run_loop.rs"]
mod virtual_cluster_job_run_loop;
#[path = "all/virtual_cluster_listing.rs"]
mod virtual_cluster_listing;
#[path = "all/virtual_cluster_queue_depth.rs"]
mod virtual_cluster_queue_depth;
#[path = "all/virtual_cluster_references.rs"]
mod virtual_cluster_references;
#[path = "all/virtual_cluster_scheduler.rs"]
mod virtual_cluster_scheduler;
#[path = "all/virtual_cluster_scheduler_status.rs"]
mod virtual_cluster_scheduler_status;
#[path = "all/virtual_cluster_spent_token_parameters.rs"]
mod virtual_cluster_spent_token_parameters;
#[path = "all/virtual_cluster_teardown_cascade.rs"]
mod virtual_cluster_teardown_cascade;
#[path = "all/virtual_cluster_update.rs"]
mod virtual_cluster_update;
