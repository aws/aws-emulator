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
#[path = "all/bridge_add_references.rs"]
mod bridge_add_references;
#[path = "all/bridge_lifecycle.rs"]
mod bridge_lifecycle;
#[path = "all/bridge_members_and_deletion.rs"]
mod bridge_members_and_deletion;
#[path = "all/bridge_output_admission.rs"]
mod bridge_output_admission;
#[path = "all/bridge_source_admission.rs"]
mod bridge_source_admission;
#[path = "all/bridge_update_references.rs"]
mod bridge_update_references;
#[path = "all/create_bridge_direction_contract.rs"]
mod create_bridge_direction_contract;
#[path = "all/create_flow_stored_formats.rs"]
mod create_flow_stored_formats;
#[path = "all/create_router_input.rs"]
mod create_router_input;
#[path = "all/create_router_network_interface_rules.rs"]
mod create_router_network_interface_rules;
#[path = "all/entitlement_listing.rs"]
mod entitlement_listing;
#[path = "all/entitlement_subscribers.rs"]
mod entitlement_subscribers;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/flow_entitlements.rs"]
mod flow_entitlements;
#[path = "all/flow_identity_and_description.rs"]
mod flow_identity_and_description;
#[path = "all/flow_lifecycle.rs"]
mod flow_lifecycle;
#[path = "all/flow_listing_default_page.rs"]
mod flow_listing_default_page;
#[path = "all/flow_media_streams.rs"]
mod flow_media_streams;
#[path = "all/flow_name_references.rs"]
mod flow_name_references;
#[path = "all/flow_ndi_resize.rs"]
mod flow_ndi_resize;
#[path = "all/flow_ndi_sizing.rs"]
mod flow_ndi_sizing;
#[path = "all/flow_output_ndi_defaults.rs"]
mod flow_output_ndi_defaults;
#[path = "all/flow_outputs.rs"]
mod flow_outputs;
#[path = "all/flow_projection.rs"]
mod flow_projection;
#[path = "all/flow_source_failover_bound.rs"]
mod flow_source_failover_bound;
#[path = "all/flow_source_metadata.rs"]
mod flow_source_metadata;
#[path = "all/flow_source_thumbnail.rs"]
mod flow_source_thumbnail;
#[path = "all/flow_sources.rs"]
mod flow_sources;
#[path = "all/flow_sub_resource_tag_retention.rs"]
mod flow_sub_resource_tag_retention;
#[path = "all/flow_sub_resource_tags.rs"]
mod flow_sub_resource_tags;
#[path = "all/flow_vpc_interfaces.rs"]
mod flow_vpc_interfaces;
#[path = "all/gateway_instances.rs"]
mod gateway_instances;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/get_router_input.rs"]
mod get_router_input;
#[path = "all/global_resource_tag_listing.rs"]
mod global_resource_tag_listing;
#[path = "all/global_resource_tags.rs"]
mod global_resource_tags;
#[path = "all/global_resource_untagging.rs"]
mod global_resource_untagging;
#[path = "all/lifecycle_preconditions.rs"]
mod lifecycle_preconditions;
#[path = "all/offering_listing.rs"]
mod offering_listing;
#[path = "all/ops_name_bindings.rs"]
mod ops_name_bindings;
#[path = "all/offering_description.rs"]
mod offering_description;
#[path = "all/remove_flow_media_stream.rs"]
mod remove_flow_media_stream;
#[path = "all/remove_flow_output.rs"]
mod remove_flow_output;
#[path = "all/remove_flow_vpc_interface.rs"]
mod remove_flow_vpc_interface;
#[path = "all/reservation_description.rs"]
mod reservation_description;
#[path = "all/reservation_listing.rs"]
mod reservation_listing;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/revoke_flow_entitlement.rs"]
mod revoke_flow_entitlement;
#[path = "all/router_input_batch_get.rs"]
mod router_input_batch_get;
#[path = "all/router_input_configuration_targets.rs"]
mod router_input_configuration_targets;
#[path = "all/router_input_deletion.rs"]
mod router_input_deletion;
#[path = "all/router_input_restart.rs"]
mod router_input_restart;
#[path = "all/router_input_start.rs"]
mod router_input_start;
#[path = "all/router_network_interface_deletion.rs"]
mod router_network_interface_deletion;
#[path = "all/router_network_interfaces.rs"]
mod router_network_interfaces;
#[path = "all/router_output_configuration_targets.rs"]
mod router_output_configuration_targets;
#[path = "all/router_output_lifecycle.rs"]
mod router_output_lifecycle;
#[path = "all/router_output_restart.rs"]
mod router_output_restart;
#[path = "all/router_output_start.rs"]
mod router_output_start;
#[path = "all/router_output_stop.rs"]
mod router_output_stop;
#[path = "all/router_outputs.rs"]
mod router_outputs;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_bridge.rs"]
mod update_bridge;
#[path = "all/update_bridge_output.rs"]
mod update_bridge_output;
#[path = "all/update_bridge_source.rs"]
mod update_bridge_source;
#[path = "all/update_bridge_state.rs"]
mod update_bridge_state;
#[path = "all/update_flow.rs"]
mod update_flow;
#[path = "all/update_flow_entitlement.rs"]
mod update_flow_entitlement;
#[path = "all/update_flow_media_stream.rs"]
mod update_flow_media_stream;
#[path = "all/update_flow_output.rs"]
mod update_flow_output;
#[path = "all/update_flow_source.rs"]
mod update_flow_source;
#[path = "all/update_router_input.rs"]
mod update_router_input;
#[path = "all/update_router_network_interface.rs"]
mod update_router_network_interface;
#[path = "all/update_router_output.rs"]
mod update_router_output;
// Declared here rather than in alphabetical order on purpose: this list is appended to by
// every mediaconnect change at once, and the middle of it is where those insertions land.
#[path = "all/describe_gateway_instance.rs"]
mod describe_gateway_instance;
#[path = "all/gateway_instance_listing.rs"]
mod gateway_instance_listing;
#[path = "all/update_gateway_instance.rs"]
mod update_gateway_instance;
#[path = "all/router_output_listing.rs"]
mod router_output_listing;
#[path = "all/router_network_interface_listing.rs"]
mod router_network_interface_listing;
#[path = "all/router_input_listing.rs"]
mod router_input_listing;
#[path = "all/create_router_output_request_bounds.rs"]
mod create_router_output_request_bounds;
#[path = "all/get_router_output.rs"]
mod get_router_output;
#[path = "all/router_input_source_metadata.rs"]
mod router_input_source_metadata;
#[path = "all/router_input_thumbnail.rs"]
mod router_input_thumbnail;
#[path = "all/router_deletion_supersedes_owed_transitions.rs"]
mod router_deletion_supersedes_owed_transitions;
#[path = "all/get_router_network_interface.rs"]
mod get_router_network_interface;
#[path = "all/router_tier_bitrate_rule.rs"]
mod router_tier_bitrate_rule;
#[path = "all/router_input_stop.rs"]
mod router_input_stop;
#[path = "all/router_output_take_input.rs"]
mod router_output_take_input;
#[path = "all/failover_primary_source.rs"]
mod failover_primary_source;
#[path = "all/router_network_interface_association_integrity.rs"]
mod router_network_interface_association_integrity;
#[path = "all/flow_removal_clears_router_references.rs"]
mod flow_removal_clears_router_references;
#[path = "all/remove_source_keeps_failover_resolvable.rs"]
mod remove_source_keeps_failover_resolvable;
#[path = "all/bridge_output_network_name_paths_agree.rs"]
mod bridge_output_network_name_paths_agree;
#[path = "all/flow_deletion_bridge_source_integrity.rs"]
mod flow_deletion_bridge_source_integrity;
#[path = "all/flow_source_bridge_and_ingest_port.rs"]
mod flow_source_bridge_and_ingest_port;
#[path = "all/bridge_flow_outputs.rs"]
mod bridge_flow_outputs;
#[path = "all/bridge_retirement_refuses_changes.rs"]
mod bridge_retirement_refuses_changes;
#[path = "all/create_bridge_references.rs"]
mod create_bridge_references;
#[path = "all/bridge_retirement_refuses_state_changes.rs"]
mod bridge_retirement_refuses_state_changes;
#[path = "all/retired_bridge_reconfiguration.rs"]
mod retired_bridge_reconfiguration;
#[path = "all/bridge_source_removal_failover_integrity.rs"]
mod bridge_source_removal_failover_integrity;
#[path = "all/bridge_deletion_flow_source_integrity.rs"]
mod bridge_deletion_flow_source_integrity;
#[path = "all/flow_ndi_discovery_servers.rs"]
mod flow_ndi_discovery_servers;
#[path = "all/required_query_parameters.rs"]
mod required_query_parameters;
#[path = "all/gateway_deletion_counts_placed_bridges.rs"]
mod gateway_deletion_counts_placed_bridges;
#[path = "all/entitlement_fee_percent.rs"]
mod entitlement_fee_percent;
#[path = "all/bridge_source_flow_outputs.rs"]
mod bridge_source_flow_outputs;
#[path = "all/flow_name_addressed_sub_resource_tags.rs"]
mod flow_name_addressed_sub_resource_tags;
#[path = "all/flow_delete_and_bridge_source_writes_agree.rs"]
mod flow_delete_and_bridge_source_writes_agree;
#[path = "all/flow_start_stop_preconditions.rs"]
mod flow_start_stop_preconditions;
#[path = "all/maintenance_window_follows_the_preference.rs"]
mod maintenance_window_follows_the_preference;
#[path = "all/router_maintenance_reschedule.rs"]
mod router_maintenance_reschedule;
#[path = "all/router_route_region_scope.rs"]
mod router_route_region_scope;
#[path = "all/flow_source_ingest_endpoint.rs"]
mod flow_source_ingest_endpoint;
#[path = "all/bridge_flow_output_capacity.rs"]
mod bridge_flow_output_capacity;
#[path = "all/router_input_configuration_consistency.rs"]
mod router_input_configuration_consistency;
#[path = "all/add_bridge_outputs_bridge_retirement.rs"]
mod add_bridge_outputs_bridge_retirement;
#[path = "all/tag_removal_without_tag_keys.rs"]
mod tag_removal_without_tag_keys;
#[path = "all/published_offering_form.rs"]
mod published_offering_form;
#[path = "all/flow_label_not_found_wording.rs"]
mod flow_label_not_found_wording;
#[path = "all/router_filter_limits.rs"]
mod router_filter_limits;
#[path = "all/flow_ndi_machine_name.rs"]
mod flow_ndi_machine_name;
#[path = "all/vpc_placement_resolution.rs"]
mod vpc_placement_resolution;
#[path = "all/update_bridge_declared_output_bound.rs"]
mod update_bridge_declared_output_bound;
#[path = "all/flow_source_report_preconditions.rs"]
mod flow_source_report_preconditions;
#[path = "all/vpc_interface_role_assumption.rs"]
mod vpc_interface_role_assumption;
