// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/channel_lifecycle.rs`) — which the layout check refuses. The `#[path]` is
// what points it into `tests/all/`.
#[path = "all/account_configuration.rs"]
mod account_configuration;

#[path = "all/account_configuration_key_fidelity.rs"]
mod account_configuration_key_fidelity;

#[path = "all/account_configuration_reads.rs"]
mod account_configuration_reads;

#[path = "all/batch_delete.rs"]
mod batch_delete;

#[path = "all/batch_start.rs"]
mod batch_start;

#[path = "all/batch_stop.rs"]
mod batch_stop;

#[path = "all/channel_alerts.rs"]
mod channel_alerts;

#[path = "all/channel_anywhere_placement_resolution.rs"]
mod channel_anywhere_placement_resolution;

#[path = "all/channel_class_pipeline_scope.rs"]
mod channel_class_pipeline_scope;

#[path = "all/channel_class_update.rs"]
mod channel_class_update;

#[path = "all/channel_configuration_schedule_consistency.rs"]
mod channel_configuration_schedule_consistency;

#[path = "all/channel_configuration_validation.rs"]
mod channel_configuration_validation;

#[path = "all/channel_engine_version_expiry.rs"]
mod channel_engine_version_expiry;

#[path = "all/channel_engine_version_pin.rs"]
mod channel_engine_version_pin;

#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;

#[path = "all/channel_maintenance_and_egress.rs"]
mod channel_maintenance_and_egress;

#[path = "all/channel_pipeline_restart.rs"]
mod channel_pipeline_restart;

#[path = "all/channel_placement_group_deletion.rs"]
mod channel_placement_group_deletion;

#[path = "all/channel_placement_group_idempotency_after_delete.rs"]
mod channel_placement_group_idempotency_after_delete;

#[path = "all/channel_placement_group_listing.rs"]
mod channel_placement_group_listing;

#[path = "all/channel_placement_group_node_resolution.rs"]
mod channel_placement_group_node_resolution;

#[path = "all/channel_placement_group_update.rs"]
mod channel_placement_group_update;

#[path = "all/channel_placement_groups.rs"]
mod channel_placement_groups;

#[path = "all/channel_schedule.rs"]
mod channel_schedule;

#[path = "all/channel_security_group_resolution.rs"]
mod channel_security_group_resolution;

#[path = "all/channel_thumbnails.rs"]
mod channel_thumbnails;

#[path = "all/channel_update.rs"]
mod channel_update;

#[path = "all/channel_used_engine_versions.rs"]
mod channel_used_engine_versions;

#[path = "all/claim_device.rs"]
mod claim_device;

#[path = "all/cloud_watch_alarm_template_datapoints.rs"]
mod cloud_watch_alarm_template_datapoints;

#[path = "all/cloud_watch_alarm_template_deletion.rs"]
mod cloud_watch_alarm_template_deletion;

#[path = "all/cloud_watch_alarm_template_group_listing.rs"]
mod cloud_watch_alarm_template_group_listing;

#[path = "all/cloud_watch_alarm_template_groups.rs"]
mod cloud_watch_alarm_template_groups;

#[path = "all/cloud_watch_alarm_template_listing.rs"]
mod cloud_watch_alarm_template_listing;

#[path = "all/cloud_watch_alarm_template_update.rs"]
mod cloud_watch_alarm_template_update;

#[path = "all/cloud_watch_alarm_templates.rs"]
mod cloud_watch_alarm_templates;

#[path = "all/cluster_alerts.rs"]
mod cluster_alerts;

#[path = "all/cluster_creation.rs"]
mod cluster_creation;

#[path = "all/cluster_deletion.rs"]
mod cluster_deletion;

#[path = "all/cluster_description.rs"]
mod cluster_description;

#[path = "all/cluster_listing.rs"]
mod cluster_listing;

#[path = "all/cluster_network_resolution.rs"]
mod cluster_network_resolution;

#[path = "all/cluster_update.rs"]
mod cluster_update;

#[path = "all/current_user_cut_point.rs"]
mod current_user_cut_point;

#[path = "all/deleted_resource_mutation.rs"]
mod deleted_resource_mutation;

#[path = "all/describe_node_reporting.rs"]
mod describe_node_reporting;

#[path = "all/describe_reservation_reporting.rs"]
mod describe_reservation_reporting;

#[path = "all/engine_version_catalog.rs"]
mod engine_version_catalog;

#[path = "all/error_envelope.rs"]
mod error_envelope;

#[path = "all/event_bridge_rule_template_group_listing.rs"]
mod event_bridge_rule_template_group_listing;

#[path = "all/event_bridge_rule_template_groups.rs"]
mod event_bridge_rule_template_groups;

#[path = "all/event_bridge_rule_template_listing.rs"]
mod event_bridge_rule_template_listing;

#[path = "all/event_bridge_rule_template_not_found.rs"]
mod event_bridge_rule_template_not_found;

#[path = "all/event_bridge_rule_template_targets.rs"]
mod event_bridge_rule_template_targets;

#[path = "all/event_bridge_rule_templates.rs"]
mod event_bridge_rule_templates;

#[path = "all/held_reference_resolution.rs"]
mod held_reference_resolution;

#[path = "all/idempotency_token_after_terminal_delete.rs"]
mod idempotency_token_after_terminal_delete;

#[path = "all/identity_name_rule.rs"]
mod identity_name_rule;

#[path = "all/input_attachment_lifecycle.rs"]
mod input_attachment_lifecycle;

#[path = "all/input_configuration_validation.rs"]
mod input_configuration_validation;

#[path = "all/input_deletion.rs"]
mod input_deletion;

#[path = "all/input_device_description.rs"]
mod input_device_description;

#[path = "all/input_device_listing.rs"]
mod input_device_listing;

#[path = "all/input_device_maintenance_window.rs"]
mod input_device_maintenance_window;

#[path = "all/input_device_reboot.rs"]
mod input_device_reboot;

#[path = "all/input_device_run_state_independence.rs"]
mod input_device_run_state_independence;

#[path = "all/input_device_streaming.rs"]
mod input_device_streaming;

#[path = "all/input_device_thumbnail.rs"]
mod input_device_thumbnail;

#[path = "all/input_device_transfer_accept.rs"]
mod input_device_transfer_accept;

#[path = "all/input_device_transfer_cancel.rs"]
mod input_device_transfer_cancel;

#[path = "all/input_device_transfer_independence.rs"]
mod input_device_transfer_independence;

#[path = "all/input_device_transfer_reject.rs"]
mod input_device_transfer_reject;

#[path = "all/input_device_transfers.rs"]
mod input_device_transfers;

#[path = "all/input_device_update.rs"]
mod input_device_update;

#[path = "all/input_endpoint_release.rs"]
mod input_endpoint_release;

#[path = "all/input_idempotency_after_delete.rs"]
mod input_idempotency_after_delete;

#[path = "all/input_idempotency_token_scope.rs"]
mod input_idempotency_token_scope;

#[path = "all/input_lifecycle.rs"]
mod input_lifecycle;

#[path = "all/input_listing.rs"]
mod input_listing;

#[path = "all/input_push_endpoints.rs"]
mod input_push_endpoints;

#[path = "all/input_security_group.rs"]
mod input_security_group;

#[path = "all/input_security_group_listing.rs"]
mod input_security_group_listing;

#[path = "all/input_update.rs"]
mod input_update;

#[path = "all/list_page_size.rs"]
mod list_page_size;

#[path = "all/managed_monitor_catalog.rs"]
mod managed_monitor_catalog;

#[path = "all/monitor_deployment.rs"]
mod monitor_deployment;

#[path = "all/monitor_deployment_deletion.rs"]
mod monitor_deployment_deletion;

#[path = "all/multiplex_alert_instants.rs"]
mod multiplex_alert_instants;

#[path = "all/multiplex_alerts.rs"]
mod multiplex_alerts;

#[path = "all/multiplex_delete_refusal_message.rs"]
mod multiplex_delete_refusal_message;

#[path = "all/multiplex_deletion.rs"]
mod multiplex_deletion;

#[path = "all/multiplex_idempotency_after_delete.rs"]
mod multiplex_idempotency_after_delete;

#[path = "all/multiplex_lifecycle.rs"]
mod multiplex_lifecycle;

#[path = "all/multiplex_listing.rs"]
mod multiplex_listing;

#[path = "all/multiplex_program_deletion.rs"]
mod multiplex_program_deletion;

#[path = "all/multiplex_program_idempotency_after_delete.rs"]
mod multiplex_program_idempotency_after_delete;

#[path = "all/multiplex_program_listing.rs"]
mod multiplex_program_listing;

#[path = "all/multiplex_program_packet_identifiers.rs"]
mod multiplex_program_packet_identifiers;

#[path = "all/multiplex_program_update.rs"]
mod multiplex_program_update;

#[path = "all/multiplex_programs.rs"]
mod multiplex_programs;

#[path = "all/multiplex_start.rs"]
mod multiplex_start;

#[path = "all/multiplex_start_after_delete.rs"]
mod multiplex_start_after_delete;

#[path = "all/multiplex_stop.rs"]
mod multiplex_stop;

#[path = "all/multiplex_update.rs"]
mod multiplex_update;

#[path = "all/network_deletion.rs"]
mod network_deletion;

#[path = "all/network_update.rs"]
mod network_update;

#[path = "all/networks.rs"]
mod networks;

#[path = "all/node_creation_cluster_phase.rs"]
mod node_creation_cluster_phase;

#[path = "all/node_deletion_idle_precondition.rs"]
mod node_deletion_idle_precondition;

#[path = "all/node_idempotency_after_deregistration.rs"]
mod node_idempotency_after_deregistration;

#[path = "all/node_lifecycle.rs"]
mod node_lifecycle;

#[path = "all/node_listing.rs"]
mod node_listing;

#[path = "all/node_name_uniqueness.rs"]
mod node_name_uniqueness;

#[path = "all/node_registration_script.rs"]
mod node_registration_script;

#[path = "all/node_registration_script_token_liveness_scope.rs"]
mod node_registration_script_token_liveness_scope;

#[path = "all/node_settings_update.rs"]
mod node_settings_update;

#[path = "all/not_found_message.rs"]
mod not_found_message;

#[path = "all/not_found_refusal_wording.rs"]
mod not_found_refusal_wording;

#[path = "all/offering_catalog.rs"]
mod offering_catalog;

#[path = "all/offering_description.rs"]
mod offering_description;

#[path = "all/partner_input.rs"]
mod partner_input;

#[path = "all/post_delete_mutation_classification.rs"]
mod post_delete_mutation_classification;

#[path = "all/region_isolation.rs"]
mod region_isolation;

#[path = "all/rejection_messages.rs"]
mod rejection_messages;

#[path = "all/reservation_deletion.rs"]
mod reservation_deletion;

#[path = "all/reservation_listing.rs"]
mod reservation_listing;

#[path = "all/reservation_purchase.rs"]
mod reservation_purchase;

#[path = "all/reservation_update.rs"]
mod reservation_update;

#[path = "all/reserved_tag_namespace.rs"]
mod reserved_tag_namespace;

#[path = "all/resource_tags.rs"]
mod resource_tags;

#[path = "all/schedule_channel_references.rs"]
mod schedule_channel_references;

#[path = "all/sdi_source_lifecycle.rs"]
mod sdi_source_lifecycle;

#[path = "all/sdi_source_mapping_precondition.rs"]
mod sdi_source_mapping_precondition;

#[path = "all/sdi_source_name_uniqueness.rs"]
mod sdi_source_name_uniqueness;

#[path = "all/sdi_source_tags_and_statuses.rs"]
mod sdi_source_tags_and_statuses;

#[path = "all/shared_alert_state_filter_rule.rs"]
mod shared_alert_state_filter_rule;

#[path = "all/shared_batch_outcome_rule.rs"]
mod shared_batch_outcome_rule;

#[path = "all/shared_idempotency_token_after_delete_rule.rs"]
mod shared_idempotency_token_after_delete_rule;

#[path = "all/shared_minted_token_spelling_rule.rs"]
mod shared_minted_token_spelling_rule;

#[path = "all/shared_page_size_rule.rs"]
mod shared_page_size_rule;

#[path = "all/shared_position_cursor_rule.rs"]
mod shared_position_cursor_rule;

#[path = "all/shared_request_rules.rs"]
mod shared_request_rules;

#[path = "all/shared_settle_first_rule.rs"]
mod shared_settle_first_rule;

#[path = "all/shared_token_replay_parent_liveness_rule.rs"]
mod shared_token_replay_parent_liveness_rule;

#[path = "all/signal_map_deletion.rs"]
mod signal_map_deletion;

#[path = "all/signal_map_deployment_window.rs"]
mod signal_map_deployment_window;

#[path = "all/signal_map_listing.rs"]
mod signal_map_listing;

#[path = "all/signal_map_update.rs"]
mod signal_map_update;

#[path = "all/signal_maps.rs"]
mod signal_maps;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/template_deletion.rs"]
mod template_deletion;

#[path = "all/template_updates.rs"]
mod template_updates;

// Not a suite of its own: the cloudwatch alarm template group request helpers
// the group suites share, layered on `wire`. Declared because every file under
// `tests/all/` must be.
#[path = "all/support.rs"]
mod support;

#[path = "all/timestamp_wire_form.rs"]
mod timestamp_wire_form;

/// The shared restJson1 harness the suites above drive the service through.
#[path = "all/wire.rs"]
mod wire;
