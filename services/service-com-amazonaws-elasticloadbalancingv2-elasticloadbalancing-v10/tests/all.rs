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
#[path = "all/alb_data_plane.rs"]
mod alb_data_plane;
#[path = "all/alb_drop_invalid_header_fields.rs"]
mod alb_drop_invalid_header_fields;
#[path = "all/alb_https_listener_tls.rs"]
mod alb_https_listener_tls;
#[path = "all/alb_lambda_target.rs"]
mod alb_lambda_target;
#[path = "all/alb_lambda_target_access_log.rs"]
mod alb_lambda_target_access_log;
#[path = "all/alb_lambda_target_published_address.rs"]
mod alb_lambda_target_published_address;
#[path = "all/attribute_collections.rs"]
mod attribute_collections;
#[path = "all/aws_error_wording.rs"]
mod aws_error_wording;
#[path = "all/capacity_reservation.rs"]
mod capacity_reservation;
#[path = "all/cascaded_delete_idempotency.rs"]
mod cascaded_delete_idempotency;
#[path = "all/catalog_reads.rs"]
mod catalog_reads;
#[path = "all/certificate_consumer_registry.rs"]
mod certificate_consumer_registry;
#[path = "all/certificate_resolution_and_redirect_protocol.rs"]
mod certificate_resolution_and_redirect_protocol;
#[path = "all/declared_action_set.rs"]
mod declared_action_set;
#[path = "all/describe_input_bounds.rs"]
mod describe_input_bounds;
#[path = "all/forward_action_config_and_mutual_authentication.rs"]
mod forward_action_config_and_mutual_authentication;
#[path = "all/host_count_health_checks.rs"]
mod host_count_health_checks;
#[path = "all/input_rejection_classes.rs"]
mod input_rejection_classes;
#[path = "all/ip_targets.rs"]
mod ip_targets;
#[path = "all/listener_lifecycle_certificates_and_rules.rs"]
mod listener_lifecycle_certificates_and_rules;
#[path = "all/listener_rules.rs"]
mod listener_rules;
#[path = "all/listener_target_group_protocol_pairing.rs"]
mod listener_target_group_protocol_pairing;
#[path = "all/listener_tls_certificates.rs"]
mod listener_tls_certificates;
#[path = "all/load_balancer_attributes.rs"]
mod load_balancer_attributes;
#[path = "all/load_balancer_dns_name_form.rs"]
mod load_balancer_dns_name_form;
#[path = "all/load_balancer_input_rules.rs"]
mod load_balancer_input_rules;
#[path = "all/load_balancer_network_capacity_and_policy.rs"]
mod load_balancer_network_capacity_and_policy;
#[path = "all/management_event_recording.rs"]
mod management_event_recording;
#[path = "all/provisioning_loop.rs"]
mod provisioning_loop;
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/redirect_request_host.rs"]
mod redirect_request_host;
#[path = "all/refusal_class_by_failure_mode.rs"]
mod refusal_class_by_failure_mode;
#[path = "all/reported_member_defaults.rs"]
mod reported_member_defaults;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/rule_deletion_and_condition_value_spellings.rs"]
mod rule_deletion_and_condition_value_spellings;
#[path = "all/rule_modification.rs"]
mod rule_modification;
#[path = "all/rule_page_markers_and_redirect_validation.rs"]
mod rule_page_markers_and_redirect_validation;
#[path = "all/rule_priorities.rs"]
mod rule_priorities;
#[path = "all/rule_transforms_and_fixed_response_content_type.rs"]
mod rule_transforms_and_fixed_response_content_type;
#[path = "all/scheduled_target_health_check_port.rs"]
mod scheduled_target_health_check_port;
#[path = "all/shared_endpoint_routing.rs"]
mod shared_endpoint_routing;
#[path = "all/ssl_policies.rs"]
mod ssl_policies;
#[path = "all/ssl_policy_families.rs"]
mod ssl_policy_families;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subnet_vpc_membership.rs"]
mod subnet_vpc_membership;
#[path = "all/tag_not_found_wording.rs"]
mod tag_not_found_wording;
#[path = "all/tag_not_found_wording_by_kind.rs"]
mod tag_not_found_wording_by_kind;
#[path = "all/tag_removal.rs"]
mod tag_removal;
#[path = "all/tag_writes.rs"]
mod tag_writes;
#[path = "all/target_deregistration_and_health_spans.rs"]
mod target_deregistration_and_health_spans;
#[path = "all/target_group_attribute_writes.rs"]
mod target_group_attribute_writes;
#[path = "all/target_group_attributes_and_modification.rs"]
mod target_group_attributes_and_modification;
#[path = "all/target_group_health_check_port.rs"]
mod target_group_health_check_port;
#[path = "all/target_group_health_check_port_refusal.rs"]
mod target_group_health_check_port_refusal;
#[path = "all/target_group_health_check_schedule.rs"]
mod target_group_health_check_schedule;
#[path = "all/target_group_health_checks.rs"]
mod target_group_health_checks;
#[path = "all/target_group_in_use_refusal.rs"]
mod target_group_in_use_refusal;
#[path = "all/target_group_layer_7_health_check.rs"]
mod target_group_layer_7_health_check;
#[path = "all/target_validation_and_quotas.rs"]
mod target_validation_and_quotas;
#[path = "all/trust_store_lifecycle.rs"]
mod trust_store_lifecycle;
#[path = "all/trust_stores.rs"]
mod trust_stores;
#[path = "all/web_acl_before_a_lambda_target.rs"]
mod web_acl_before_a_lambda_target;
#[path = "all/web_acl_enforcement.rs"]
mod web_acl_enforcement;
#[path = "all/weighted_forward_and_target_group_stickiness.rs"]
mod weighted_forward_and_target_group_stickiness;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/zonal_shift_registration.rs"]
mod zonal_shift_registration;
// Declared after the sorted list on purpose: a line inserted inside it collides with every
// other suite landing in the same window. Fold it into the list whenever the list is next
// touched for another reason. The suite itself is an ordinary `tests/all/<name>.rs` file:
// what a NETWORK Load Balancer's own traffic is answered by, which is the load balancer its
// `alb` target group names.
#[path = "all/network_load_balancer_pass_on.rs"]
mod network_load_balancer_pass_on;
