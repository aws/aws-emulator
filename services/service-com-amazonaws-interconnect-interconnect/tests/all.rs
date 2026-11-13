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
#[path = "all/accept_connection_proposal.rs"]
mod accept_connection_proposal;
#[path = "all/acceptance_attach_point_arn_resolves_in_full.rs"]
mod acceptance_attach_point_arn_resolves_in_full;
#[path = "all/acceptance_does_not_race_the_clock.rs"]
mod acceptance_does_not_race_the_clock;
#[path = "all/activation_key_is_base64.rs"]
mod activation_key_is_base64;
#[path = "all/arn_region_segment_is_not_a_scope.rs"]
mod arn_region_segment_is_not_a_scope;
#[path = "all/arn_scope_refusals.rs"]
mod arn_scope_refusals;
#[path = "all/arn_scope_is_relative_to_the_request.rs"]
mod arn_scope_is_relative_to_the_request;
#[path = "all/arn_scope_mismatch_wording.rs"]
mod arn_scope_mismatch_wording;
#[path = "all/attach_point_arn_is_scoped_to_the_request.rs"]
mod attach_point_arn_is_scoped_to_the_request;
#[path = "all/attach_point_arn_needs_provisioned_connectivity.rs"]
mod attach_point_arn_needs_provisioned_connectivity;
#[path = "all/attach_point_arn_stops_at_the_delete.rs"]
mod attach_point_arn_stops_at_the_delete;
#[path = "all/attach_point_judgement_is_shared.rs"]
mod attach_point_judgement_is_shared;
#[path = "all/attach_point_listing_needs_a_resolved_account.rs"]
mod attach_point_listing_needs_a_resolved_account;
#[path = "all/attach_point_member_constraints.rs"]
mod attach_point_member_constraints;
#[path = "all/attach_points.rs"]
mod attach_points;
#[path = "all/attach_points_are_scoped_to_the_environment.rs"]
mod attach_points_are_scoped_to_the_environment;
#[path = "all/bandwidth_availability.rs"]
mod bandwidth_availability;
#[path = "all/billing_tier.rs"]
mod billing_tier;
#[path = "all/caller_created_gateways_are_attach_points.rs"]
mod caller_created_gateways_are_attach_points;
#[path = "all/client_fault_status_and_validation_wording.rs"]
mod client_fault_status_and_validation_wording;
#[path = "all/client_refusals_carry_the_envelope.rs"]
mod client_refusals_carry_the_envelope;
#[path = "all/connection_lifecycle.rs"]
mod connection_lifecycle;
#[path = "all/connection_proposal.rs"]
mod connection_proposal;
#[path = "all/connection_state_progression.rs"]
mod connection_state_progression;
#[path = "all/create_connection_attach_point_validation.rs"]
mod create_connection_attach_point_validation;
#[path = "all/create_connection_client_token_replay.rs"]
mod create_connection_client_token_replay;
#[path = "all/create_connection_remote_account.rs"]
mod create_connection_remote_account;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/list_connections_environment_filter.rs"]
mod list_connections_environment_filter;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/page_cursors_are_only_what_the_listing_minted.rs"]
mod page_cursors_are_only_what_the_listing_minted;
#[path = "all/proposal_parameters_are_captured.rs"]
mod proposal_parameters_are_captured;
#[path = "all/provider_member_constraints.rs"]
mod provider_member_constraints;
#[path = "all/published_environment_catalog.rs"]
mod published_environment_catalog;
#[path = "all/refusals_and_gateway_attach_points.rs"]
mod refusals_and_gateway_attach_points;
#[path = "all/region_scoped_connections.rs"]
mod region_scoped_connections;
#[path = "all/region_scoped_environment_catalog.rs"]
mod region_scoped_environment_catalog;
#[path = "all/request_faults_precede_resource_verdicts.rs"]
mod request_faults_precede_resource_verdicts;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_merge_is_shared.rs"]
mod tag_merge_is_shared;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tag_set_has_one_representation.rs"]
mod tag_set_has_one_representation;
#[path = "all/update_connection_idempotent_retry.rs"]
mod update_connection_idempotent_retry;
#[path = "all/update_settles_back_into_the_state_it_came_from.rs"]
mod update_settles_back_into_the_state_it_came_from;
