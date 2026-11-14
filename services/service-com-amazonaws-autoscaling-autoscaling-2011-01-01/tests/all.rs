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
#[path = "all/absent_group_refusal_messages.rs"]
mod absent_group_refusal_messages;
#[path = "all/account_level_reads_publish_service_data.rs"]
mod account_level_reads_publish_service_data;
#[path = "all/account_wide_read_authorization.rs"]
mod account_wide_read_authorization;
#[path = "all/auto_scaling_group_lifecycle.rs"]
mod auto_scaling_group_lifecycle;
#[path = "all/cancel_instance_refresh.rs"]
mod cancel_instance_refresh;
#[path = "all/cancel_instance_refresh_absent_group.rs"]
mod cancel_instance_refresh_absent_group;
#[path = "all/client_error_refusal_wording.rs"]
mod client_error_refusal_wording;
#[path = "all/delete_notification_configuration.rs"]
mod delete_notification_configuration;
#[path = "all/describe_lifecycle_hooks_projection.rs"]
mod describe_lifecycle_hooks_projection;
#[path = "all/describe_policies_page_size.rs"]
mod describe_policies_page_size;
#[path = "all/describe_tags.rs"]
mod describe_tags;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/group_tag_propagate_at_launch.rs"]
mod group_tag_propagate_at_launch;
#[path = "all/group_size_and_health_check_refusal_wording.rs"]
mod group_size_and_health_check_refusal_wording;
#[path = "all/instance_lifecycle_events.rs"]
mod instance_lifecycle_events;
#[path = "all/instance_refresh_lifecycle.rs"]
mod instance_refresh_lifecycle;
#[path = "all/instance_refresh_skip_matching.rs"]
mod instance_refresh_skip_matching;
#[path = "all/instance_scale_in_protection.rs"]
mod instance_scale_in_protection;
#[path = "all/launch_configurations_are_unavailable.rs"]
mod launch_configurations_are_unavailable;
#[path = "all/launch_template_instance_profiles.rs"]
mod launch_template_instance_profiles;
#[path = "all/launch_template_option_sets.rs"]
mod launch_template_option_sets;
#[path = "all/launch_template_security_group_vpc.rs"]
mod launch_template_security_group_vpc;
#[path = "all/launch_template_specifications.rs"]
mod launch_template_specifications;
#[path = "all/legacy_target_group_detach.rs"]
mod legacy_target_group_detach;
#[path = "all/lifecycle_hooks.rs"]
mod lifecycle_hooks;
#[path = "all/load_balancer_target_registration.rs"]
mod load_balancer_target_registration;
#[path = "all/metric_collection_catalog.rs"]
mod metric_collection_catalog;
#[path = "all/policy_read_projection.rs"]
mod policy_read_projection;
#[path = "all/reference_data_catalogs.rs"]
mod reference_data_catalogs;
#[path = "all/rollback_instance_refresh.rs"]
mod rollback_instance_refresh;
#[path = "all/scale_out_launches_ec2_instances.rs"]
mod scale_out_launches_ec2_instances;
#[path = "all/scaling_policies.rs"]
mod scaling_policies;
#[path = "all/scaling_activity_log.rs"]
mod scaling_activity_log;
#[path = "all/set_desired_capacity_on_subnet_placed_groups.rs"]
mod set_desired_capacity_on_subnet_placed_groups;
#[path = "all/sns_notifications.rs"]
mod sns_notifications;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// The shared harness the suites above drive the service through: the awsQuery POST
// and the XML readers, defined once.
#[path = "all/support.rs"]
mod support;
#[path = "all/target_tracking_alarm_series.rs"]
mod target_tracking_alarm_series;
#[path = "all/terminate_instance_in_auto_scaling_group.rs"]
mod terminate_instance_in_auto_scaling_group;
#[path = "all/vpc_zone_identifier_zones.rs"]
mod vpc_zone_identifier_zones;
#[path = "all/warm_pools.rs"]
mod warm_pools;
