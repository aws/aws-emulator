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
#[path = "all/arn_addressing_cross_checks_domain.rs"]
mod arn_addressing_cross_checks_domain;
#[path = "all/arn_and_members_address_one_resource.rs"]
mod arn_and_members_address_one_resource;
#[path = "all/association_is_stamped_at_write_time.rs"]
mod association_is_stamped_at_write_time;
#[path = "all/associations_follow_their_registration.rs"]
mod associations_follow_their_registration;
#[path = "all/credentials_secret_resolution.rs"]
mod credentials_secret_resolution;
#[path = "all/deregistration_spares_what_it_was_not_asked_about.rs"]
mod deregistration_spares_what_it_was_not_asked_about;
#[path = "all/directory_fleet_resolution.rs"]
mod directory_fleet_resolution;
#[path = "all/fail_closed_refusals_answer_401.rs"]
mod fail_closed_refusals_answer_401;
#[path = "all/filter_operations.rs"]
mod filter_operations;
#[path = "all/front_door_refusal_header_set.rs"]
mod front_door_refusal_header_set;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/identity_provider_deregistration.rs"]
mod identity_provider_deregistration;
#[path = "all/identity_provider_subscriptions.rs"]
mod identity_provider_subscriptions;
#[path = "all/instance_authorization_is_hoisted.rs"]
mod instance_authorization_is_hoisted;
#[path = "all/instance_fleet_resolution.rs"]
mod instance_fleet_resolution;
#[path = "all/instance_gate_is_one_function.rs"]
mod instance_gate_is_one_function;
#[path = "all/instance_products_follow_the_registrations.rs"]
mod instance_products_follow_the_registrations;
#[path = "all/instance_status_check_date.rs"]
mod instance_status_check_date;
#[path = "all/instances_come_from_the_ec2_fleet.rs"]
mod instances_come_from_the_ec2_fleet;
#[path = "all/license_based_subscription_expiry.rs"]
mod license_based_subscription_expiry;
#[path = "all/license_server_endpoint_create_names_its_settings.rs"]
mod license_server_endpoint_create_names_its_settings;
#[path = "all/license_server_endpoint_delete_resolution.rs"]
mod license_server_endpoint_delete_resolution;
#[path = "all/license_server_endpoint_deletion.rs"]
mod license_server_endpoint_deletion;
#[path = "all/license_server_endpoints.rs"]
mod license_server_endpoints;
#[path = "all/license_servers_behind_an_endpoint.rs"]
mod license_servers_behind_an_endpoint;
#[path = "all/list_instances.rs"]
mod list_instances;
#[path = "all/optional_settings_registration.rs"]
mod optional_settings_registration;
#[path = "all/page_tokens_are_only_what_the_minter_writes.rs"]
mod page_tokens_are_only_what_the_minter_writes;
#[path = "all/pending_transition_comes_with_the_record.rs"]
mod pending_transition_comes_with_the_record;
#[path = "all/product_subscription_teardown.rs"]
mod product_subscription_teardown;
#[path = "all/product_user_removal_is_one_writer.rs"]
mod product_user_removal_is_one_writer;
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
#[path = "all/refusals.rs"]
mod refusals;
#[path = "all/registration_pair_resolution_is_shared.rs"]
mod registration_pair_resolution_is_shared;
#[path = "all/registration_resolves_the_network_its_owner_minted.rs"]
mod registration_resolves_the_network_its_owner_minted;
#[path = "all/resource_resolution_refusals.rs"]
mod resource_resolution_refusals;
#[path = "all/settings_network_resolution.rs"]
mod settings_network_resolution;
#[path = "all/settings_update_writes_at_write_time.rs"]
mod settings_update_writes_at_write_time;
#[path = "all/settings_write_stores_what_resolved.rs"]
mod settings_write_stores_what_resolved;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_refusal_names_what_the_request_named.rs"]
mod stop_refusal_names_what_the_request_named;
#[path = "all/subscription_product_names_the_subscription.rs"]
mod subscription_product_names_the_subscription;
#[path = "all/the_sweep_reads_what_the_records_owe.rs"]
mod the_sweep_reads_what_the_records_owe;
#[path = "all/tag_validation.rs"]
mod tag_validation;
#[path = "all/update_settings_network_propagation.rs"]
mod update_settings_network_propagation;
#[path = "all/update_settings_subnet_minimum.rs"]
mod update_settings_subnet_minimum;
#[path = "all/user_association_plane_states_one_refusal.rs"]
mod user_association_plane_states_one_refusal;
#[path = "all/user_association_registration_rule_is_shared.rs"]
mod user_association_registration_rule_is_shared;
#[path = "all/user_associations.rs"]
mod user_associations;
#[path = "all/user_disassociation.rs"]
mod user_disassociation;
#[path = "all/user_domain_names_the_user.rs"]
mod user_domain_names_the_user;
