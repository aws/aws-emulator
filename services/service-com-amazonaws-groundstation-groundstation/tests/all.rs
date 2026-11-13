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
#[path = "all/addressable_ground_stations.rs"]
mod addressable_ground_stations;
#[path = "all/agent_carried_endpoints.rs"]
mod agent_carried_endpoints;
#[path = "all/agent_configuration_reads.rs"]
mod agent_configuration_reads;
#[path = "all/agent_lifecycle.rs"]
mod agent_lifecycle;
#[path = "all/agent_registration_tags.rs"]
mod agent_registration_tags;
#[path = "all/agent_task_response_url.rs"]
mod agent_task_response_url;
#[path = "all/antenna_inventory.rs"]
mod antenna_inventory;
#[path = "all/availability_search_references.rs"]
mod availability_search_references;
#[path = "all/available_contacts.rs"]
mod available_contacts;
#[path = "all/capacity_refusal_precedence.rs"]
mod capacity_refusal_precedence;
#[path = "all/config_defaults_and_references.rs"]
mod config_defaults_and_references;
#[path = "all/config_listing.rs"]
mod config_listing;
#[path = "all/config_reference_integrity.rs"]
mod config_reference_integrity;
#[path = "all/config_update_endpoint_group_agreement.rs"]
mod config_update_endpoint_group_agreement;
#[path = "all/config_update_reach.rs"]
mod config_update_reach;
#[path = "all/config_update_refusal_precedence.rs"]
mod config_update_refusal_precedence;
#[path = "all/contact_cancellation.rs"]
mod contact_cancellation;
#[path = "all/contact_dataflows_and_pass_stages.rs"]
mod contact_dataflows_and_pass_stages;
#[path = "all/contact_endpoint_settle_consistency.rs"]
mod contact_endpoint_settle_consistency;
#[path = "all/contact_ephemeris_filter.rs"]
mod contact_ephemeris_filter;
#[path = "all/contact_pointing_ephemeris.rs"]
mod contact_pointing_ephemeris;
#[path = "all/contact_satellite_reference_integrity.rs"]
mod contact_satellite_reference_integrity;
#[path = "all/contact_version_description.rs"]
mod contact_version_description;
#[path = "all/contact_version_last_updated.rs"]
mod contact_version_last_updated;
#[path = "all/contact_version_listing.rs"]
mod contact_version_listing;
#[path = "all/contact_versions.rs"]
mod contact_versions;
#[path = "all/contact_window_rules.rs"]
mod contact_window_rules;
#[path = "all/dataflow_edge_rules.rs"]
mod dataflow_edge_rules;
#[path = "all/dataflow_endpoint_addressing.rs"]
mod dataflow_endpoint_addressing;
#[path = "all/dataflow_endpoint_group_deletion.rs"]
mod dataflow_endpoint_group_deletion;
#[path = "all/dataflow_endpoint_group_in_use_guard.rs"]
mod dataflow_endpoint_group_in_use_guard;
#[path = "all/dataflow_endpoint_group_listing.rs"]
mod dataflow_endpoint_group_listing;
#[path = "all/dataflow_endpoint_group_reads.rs"]
mod dataflow_endpoint_group_reads;
#[path = "all/dataflow_endpoint_groups.rs"]
mod dataflow_endpoint_groups;
#[path = "all/dataflow_endpoint_groups_v2.rs"]
mod dataflow_endpoint_groups_v2;
#[path = "all/dataflow_endpoint_mtu.rs"]
mod dataflow_endpoint_mtu;
#[path = "all/empty_list_filters.rs"]
mod empty_list_filters;
#[path = "all/ended_contact_takes_no_version.rs"]
mod ended_contact_takes_no_version;
#[path = "all/endpoint_ipv4_addresses.rs"]
mod endpoint_ipv4_addresses;
#[path = "all/ephemeris_az_el_validation.rs"]
mod ephemeris_az_el_validation;
#[path = "all/ephemeris_catalog.rs"]
mod ephemeris_catalog;
#[path = "all/ephemeris_deletion.rs"]
mod ephemeris_deletion;
#[path = "all/ephemeris_description.rs"]
mod ephemeris_description;
#[path = "all/ephemeris_expiration.rs"]
mod ephemeris_expiration;
#[path = "all/ephemeris_in_force_agreement.rs"]
mod ephemeris_in_force_agreement;
#[path = "all/ephemeris_in_use_writes.rs"]
mod ephemeris_in_use_writes;
#[path = "all/ephemeris_supersession.rs"]
mod ephemeris_supersession;
#[path = "all/ephemeris_updates.rs"]
mod ephemeris_updates;
#[path = "all/ephemeris_window_selection.rs"]
mod ephemeris_window_selection;
#[path = "all/ground_station_inventory.rs"]
mod ground_station_inventory;
#[path = "all/ground_station_reservations.rs"]
mod ground_station_reservations;
#[path = "all/identifier_parameter_refusals.rs"]
mod identifier_parameter_refusals;
#[path = "all/lifecycle_writes.rs"]
mod lifecycle_writes;
#[path = "all/minute_usage.rs"]
mod minute_usage;
#[path = "all/mission_profile_deletion.rs"]
mod mission_profile_deletion;
#[path = "all/mission_profile_endpoint_group_agreement.rs"]
mod mission_profile_endpoint_group_agreement;
#[path = "all/mission_profile_update_reach.rs"]
mod mission_profile_update_reach;
#[path = "all/non_empty_response_members.rs"]
mod non_empty_response_members;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/owed_transition_spans.rs"]
mod owed_transition_spans;
#[path = "all/page_boundaries.rs"]
mod page_boundaries;
#[path = "all/page_token_field_lists.rs"]
mod page_token_field_lists;
#[path = "all/placement_reads_the_clock.rs"]
mod placement_reads_the_clock;
#[path = "all/program_track_reference_integrity.rs"]
mod program_track_reference_integrity;
#[path = "all/recording_bucket_refusal_message.rs"]
mod recording_bucket_refusal_message;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/request_validation.rs"]
mod request_validation;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/s3_recording_buckets.rs"]
mod s3_recording_buckets;
#[path = "all/satellite_catalog.rs"]
mod satellite_catalog;
#[path = "all/satellite_current_ephemeris.rs"]
mod satellite_current_ephemeris;
#[path = "all/satellite_reads.rs"]
mod satellite_reads;
#[path = "all/scheduling_loop.rs"]
mod scheduling_loop;
#[path = "all/search_window_rule.rs"]
mod search_window_rule;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_bound_limit_error.rs"]
mod store_bound_limit_error;
#[path = "all/tag_mutation_no_content.rs"]
mod tag_mutation_no_content;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/tag_rules.rs"]
mod tag_rules;
#[path = "all/tagging_not_found_refusal.rs"]
mod tagging_not_found_refusal;
#[path = "all/uplink_echo_reference_rule.rs"]
mod uplink_echo_reference_rule;
