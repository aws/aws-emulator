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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/accepted_request_classes.rs"]
mod accepted_request_classes;
#[path = "all/agent_plane.rs"]
mod agent_plane;
#[path = "all/arn_account_and_refusal_classes.rs"]
mod arn_account_and_refusal_classes;
#[path = "all/arn_region_scope.rs"]
mod arn_region_scope;
#[path = "all/attachment_state_reports.rs"]
mod attachment_state_reports;
#[path = "all/awsvpc_configuration_store.rs"]
mod awsvpc_configuration_store;
#[path = "all/awsvpc_interface_subnet_and_identity.rs"]
mod awsvpc_interface_subnet_and_identity;
#[path = "all/capacity_provider_in_use.rs"]
mod capacity_provider_in_use;
#[path = "all/capacity_provider_service_describe_polls.rs"]
mod capacity_provider_service_describe_polls;
#[path = "all/capacity_provider_surface.rs"]
mod capacity_provider_surface;
#[path = "all/capacity_providers.rs"]
mod capacity_providers;
#[path = "all/classic_load_balancer_attachments.rs"]
mod classic_load_balancer_attachments;
#[path = "all/cluster_capacity_and_statistics.rs"]
mod cluster_capacity_and_statistics;
#[path = "all/cluster_mutations.rs"]
mod cluster_mutations;
#[path = "all/cluster_projection.rs"]
mod cluster_projection;
#[path = "all/cluster_scoped_capacity_providers.rs"]
mod cluster_scoped_capacity_providers;
#[path = "all/cluster_tombstone.rs"]
mod cluster_tombstone;
#[path = "all/container_health_checks.rs"]
mod container_health_checks;
#[path = "all/container_instance_not_found.rs"]
mod container_instance_not_found;
#[path = "all/container_instances.rs"]
mod container_instances;
#[path = "all/container_reservation_refusals.rs"]
mod container_reservation_refusals;
#[path = "all/container_runtime.rs"]
mod container_runtime;
#[path = "all/continue_service_deployment_validation.rs"]
mod continue_service_deployment_validation;
#[path = "all/daemon_reference_family_refusals.rs"]
mod daemon_reference_family_refusals;
#[path = "all/daemon_task_definition_deletion.rs"]
mod daemon_task_definition_deletion;
#[path = "all/daemon_task_definitions.rs"]
mod daemon_task_definitions;
#[path = "all/daemons.rs"]
mod daemons;
#[path = "all/default_capacity_provider_strategy_placement.rs"]
mod default_capacity_provider_strategy_placement;
#[path = "all/delete_attributes.rs"]
mod delete_attributes;
#[path = "all/deployment_circuit_breaker.rs"]
mod deployment_circuit_breaker;
#[path = "all/deployment_rollout_report_coupling.rs"]
mod deployment_rollout_report_coupling;
#[path = "all/describe_failure_identifiers.rs"]
mod describe_failure_identifiers;
#[path = "all/describe_service_deployments.rs"]
mod describe_service_deployments;
#[path = "all/domain_refusal_wording.rs"]
mod domain_refusal_wording;
#[path = "all/draining_instance_relocation.rs"]
mod draining_instance_relocation;
#[path = "all/ecr_image_pull.rs"]
mod ecr_image_pull;
#[path = "all/ecs_wire.rs"]
mod ecs_wire;
#[path = "all/enum_value_refusals.rs"]
mod enum_value_refusals;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/exec_task_role_resolution.rs"]
mod exec_task_role_resolution;
#[path = "all/execution_role_trust.rs"]
mod execution_role_trust;
#[path = "all/express_gateway_services.rs"]
mod express_gateway_services;
#[path = "all/forced_service_delete_status.rs"]
mod forced_service_delete_status;
#[path = "all/image_pull_placement.rs"]
mod image_pull_placement;
#[path = "all/inactive_cluster_refusals.rs"]
mod inactive_cluster_refusals;
#[path = "all/integer_bound_refusals.rs"]
mod integer_bound_refusals;
#[path = "all/invalid_request_refusals.rs"]
mod invalid_request_refusals;
#[path = "all/launch_role_step_order.rs"]
mod launch_role_step_order;
#[path = "all/launch_steps_before_start.rs"]
mod launch_steps_before_start;
#[path = "all/list_attributes.rs"]
mod list_attributes;
#[path = "all/list_clusters.rs"]
mod list_clusters;
#[path = "all/list_daemon_task_definitions.rs"]
mod list_daemon_task_definitions;
#[path = "all/list_service_deployments.rs"]
mod list_service_deployments;
#[path = "all/list_services.rs"]
mod list_services;
#[path = "all/list_task_definition_families.rs"]
mod list_task_definition_families;
#[path = "all/list_task_definitions.rs"]
mod list_task_definitions;
#[path = "all/load_balanced_task_health_check_port.rs"]
mod load_balanced_task_health_check_port;
#[path = "all/load_balanced_task_propagation.rs"]
mod load_balanced_task_propagation;
#[path = "all/load_balancer_pairing_refusal_wording.rs"]
mod load_balancer_pairing_refusal_wording;
#[path = "all/load_balancer_target_group_placement.rs"]
mod load_balancer_target_group_placement;
#[path = "all/load_balancer_target_groups.rs"]
mod load_balancer_target_groups;
// The spellings a Managed Instances launch template's `monitoring` option is accepted in,
// beside the option the stored template reports (`probe_monitoring`).
#[path = "all/managed_instances_monitoring_spelling.rs"]
mod managed_instances_monitoring_spelling;
// The members BESIDE `monitoring` a Managed Instances provider configuration carries: every
// one of them optional, and what a create makes of each.
#[path = "all/managed_instances_provider_members.rs"]
mod managed_instances_provider_members;
#[path = "all/mutating_request_authorization.rs"]
mod mutating_request_authorization;
#[path = "all/name_pattern_refusals.rs"]
mod name_pattern_refusals;
#[path = "all/page_size_range_refusals.rs"]
mod page_size_range_refusals;
#[path = "all/placement_resolution.rs"]
mod placement_resolution;
#[path = "all/primary_task_set.rs"]
mod primary_task_set;
// Pins the Managed Instances launch template's `monitoring` option; the module name
// follows its file, as the layout rule requires.
#[path = "all/probe_monitoring.rs"]
mod probe_monitoring;
#[path = "all/registration_credential_rules.rs"]
mod registration_credential_rules;
#[path = "all/reserved_tag_prefix.rs"]
mod reserved_tag_prefix;
#[path = "all/resource_scoping_and_agent_plane.rs"]
mod resource_scoping_and_agent_plane;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_delete_drain.rs"]
mod service_delete_drain;
#[path = "all/service_deployment_lifecycle_hooks.rs"]
mod service_deployment_lifecycle_hooks;
#[path = "all/service_deployment_stops.rs"]
mod service_deployment_stops;
#[path = "all/service_derived_members.rs"]
mod service_derived_members;
#[path = "all/service_desired_count_convergence.rs"]
mod service_desired_count_convergence;
#[path = "all/service_lifecycle.rs"]
mod service_lifecycle;
#[path = "all/service_listing.rs"]
mod service_listing;
#[path = "all/service_load_balancer_cross_field.rs"]
mod service_load_balancer_cross_field;
#[path = "all/service_not_active_refusals.rs"]
mod service_not_active_refusals;
#[path = "all/service_projection.rs"]
mod service_projection;
#[path = "all/service_refusal_sentences.rs"]
mod service_refusal_sentences;
#[path = "all/service_registry_cloud_map_instances.rs"]
mod service_registry_cloud_map_instances;
#[path = "all/service_registry_dns_record_rules.rs"]
mod service_registry_dns_record_rules;
#[path = "all/service_revisions.rs"]
mod service_revisions;
#[path = "all/service_run_state.rs"]
mod service_run_state;
#[path = "all/service_validation.rs"]
mod service_validation;
#[path = "all/services_by_namespace.rs"]
mod services_by_namespace;
#[path = "all/start_task.rs"]
mod start_task;
#[path = "all/start_task_container_instance_ids.rs"]
mod start_task_container_instance_ids;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_set_reads.rs"]
mod tag_set_reads;
#[path = "all/tagging_writes.rs"]
mod tagging_writes;
#[path = "all/task_definition_compatibility_validation.rs"]
mod task_definition_compatibility_validation;
#[path = "all/task_definition_deletion.rs"]
mod task_definition_deletion;
#[path = "all/task_definition_deregistration.rs"]
mod task_definition_deregistration;
#[path = "all/task_definition_projection.rs"]
mod task_definition_projection;
#[path = "all/task_definition_reference_refusals.rs"]
mod task_definition_reference_refusals;
#[path = "all/task_definition_rendering.rs"]
mod task_definition_rendering;
#[path = "all/task_definition_revision_requirement.rs"]
mod task_definition_revision_requirement;
#[path = "all/task_interface_allocation.rs"]
mod task_interface_allocation;
#[path = "all/task_interface_attach_and_connectivity.rs"]
mod task_interface_attach_and_connectivity;
#[path = "all/task_lifecycle.rs"]
mod task_lifecycle;
#[path = "all/task_listing_filters.rs"]
mod task_listing_filters;
#[path = "all/task_override_and_size_validation.rs"]
mod task_override_and_size_validation;
#[path = "all/task_protection_scale_in.rs"]
mod task_protection_scale_in;
#[path = "all/task_provisioning.rs"]
mod task_provisioning;
#[path = "all/task_set_creation.rs"]
mod task_set_creation;
#[path = "all/task_set_deletion.rs"]
mod task_set_deletion;
#[path = "all/task_set_failure_arns.rs"]
mod task_set_failure_arns;
#[path = "all/task_set_reports.rs"]
mod task_set_reports;
#[path = "all/task_sets.rs"]
mod task_sets;
#[path = "all/task_state_change_events.rs"]
mod task_state_change_events;
#[path = "all/task_state_reports.rs"]
mod task_state_reports;
#[path = "all/update_container_instances_state_failures.rs"]
mod update_container_instances_state_failures;
#[path = "all/update_task_set_refusals.rs"]
mod update_task_set_refusals;
#[path = "all/ec2_launched_cluster_capacity.rs"]
mod ec2_launched_cluster_capacity;
