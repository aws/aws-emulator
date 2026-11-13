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
#[path = "all/add_custom_attributes.rs"]
mod add_custom_attributes;
#[path = "all/add_user_pool_client_secret.rs"]
mod add_user_pool_client_secret;
#[path = "all/admin_confirm_sign_up.rs"]
mod admin_confirm_sign_up;
#[path = "all/admin_delete_user_attributes.rs"]
mod admin_delete_user_attributes;
#[path = "all/admin_get_user_auth_factors.rs"]
mod admin_get_user_auth_factors;
#[path = "all/admin_group_listing.rs"]
mod admin_group_listing;
#[path = "all/admin_provider_links.rs"]
mod admin_provider_links;
#[path = "all/admin_remove_user_from_group.rs"]
mod admin_remove_user_from_group;
#[path = "all/admin_reset_user_password.rs"]
mod admin_reset_user_password;
#[path = "all/admin_respond_to_auth_challenge.rs"]
mod admin_respond_to_auth_challenge;
#[path = "all/admin_set_user_mfa_preference.rs"]
mod admin_set_user_mfa_preference;
#[path = "all/admin_set_user_settings.rs"]
mod admin_set_user_settings;
#[path = "all/admin_update_user_attributes.rs"]
mod admin_update_user_attributes;
#[path = "all/admin_user_enabled_state.rs"]
mod admin_user_enabled_state;
#[path = "all/admin_user_global_sign_out.rs"]
mod admin_user_global_sign_out;
#[path = "all/admin_user_pool_lifecycle.rs"]
mod admin_user_pool_lifecycle;
#[path = "all/alias_attribute_ownership.rs"]
mod alias_attribute_ownership;
#[path = "all/app_client_and_pool_configuration.rs"]
mod app_client_and_pool_configuration;
#[path = "all/app_client_refusal_wording.rs"]
mod app_client_refusal_wording;
#[path = "all/app_client_validity_defaults.rs"]
mod app_client_validity_defaults;
#[path = "all/auth_flow_not_enabled.rs"]
mod auth_flow_not_enabled;
#[path = "all/branding_and_domain_generated_reads.rs"]
mod branding_and_domain_generated_reads;
#[path = "all/authentication_refusal_classes.rs"]
mod authentication_refusal_classes;
#[path = "all/authentication_refusal_wire.rs"]
mod authentication_refusal_wire;
#[path = "all/authentication_success_shape.rs"]
mod authentication_success_shape;
#[path = "all/case_insensitive_user_resolution.rs"]
mod case_insensitive_user_resolution;
#[path = "all/change_password.rs"]
mod change_password;
#[path = "all/client_secret_absence_sentence.rs"]
mod client_secret_absence_sentence;
#[path = "all/client_secret_not_found_message.rs"]
mod client_secret_not_found_message;
#[path = "all/clock_dependent_state.rs"]
mod clock_dependent_state;
#[path = "all/code_delivery_destination_masking.rs"]
mod code_delivery_destination_masking;
#[path = "all/confidential_client_challenge_response.rs"]
mod confidential_client_challenge_response;
#[path = "all/confirm_forgot_password_validation_order.rs"]
mod confirm_forgot_password_validation_order;
#[path = "all/constraint_refusal_values.rs"]
mod constraint_refusal_values;
#[path = "all/custom_email_sender_node_handler.rs"]
mod custom_email_sender_node_handler;
#[path = "all/custom_email_sender_trigger.rs"]
mod custom_email_sender_trigger;
#[path = "all/delete_group.rs"]
mod delete_group;
#[path = "all/delete_identity_provider.rs"]
mod delete_identity_provider;
#[path = "all/delete_resource_server.rs"]
mod delete_resource_server;
#[path = "all/delete_terms.rs"]
mod delete_terms;
#[path = "all/delete_user.rs"]
mod delete_user;
#[path = "all/delete_user_attributes.rs"]
mod delete_user_attributes;
#[path = "all/delete_user_pool_client.rs"]
mod delete_user_pool_client;
#[path = "all/delete_user_pool_client_secret.rs"]
mod delete_user_pool_client_secret;
#[path = "all/delete_user_pool_domain.rs"]
mod delete_user_pool_domain;
#[path = "all/delete_web_authn_credential.rs"]
mod delete_web_authn_credential;
#[path = "all/describe_terms_by_client.rs"]
mod describe_terms_by_client;
#[path = "all/device_refusal_wire.rs"]
mod device_refusal_wire;
#[path = "all/device_remembering.rs"]
mod device_remembering;
#[path = "all/domain_lookup_and_condition_messages.rs"]
mod domain_lookup_and_condition_messages;
#[path = "all/duplicate_resource_refusal_wording.rs"]
mod duplicate_resource_refusal_wording;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/error_wire.rs"]
mod error_wire;
#[path = "all/explicit_auth_flows.rs"]
mod explicit_auth_flows;
#[path = "all/feature_plan_gate.rs"]
mod feature_plan_gate;
#[path = "all/forgot_password_caller_identity.rs"]
mod forgot_password_caller_identity;
#[path = "all/get_client_token.rs"]
mod get_client_token;
#[path = "all/get_identity_provider_by_identifier.rs"]
mod get_identity_provider_by_identifier;
#[path = "all/get_tokens_from_refresh_token.rs"]
mod get_tokens_from_refresh_token;
#[path = "all/get_user.rs"]
mod get_user;
#[path = "all/get_user_auth_factors.rs"]
mod get_user_auth_factors;
#[path = "all/group_lifecycle.rs"]
mod group_lifecycle;
#[path = "all/group_membership.rs"]
mod group_membership;
#[path = "all/group_role_claims.rs"]
mod group_role_claims;
#[path = "all/iam_authorization.rs"]
mod iam_authorization;
#[path = "all/identity_provider_lifecycle.rs"]
mod identity_provider_lifecycle;
#[path = "all/identity_provider_type_defaults.rs"]
mod identity_provider_type_defaults;
#[path = "all/initiate_auth.rs"]
mod initiate_auth;
#[path = "all/input_validation_refusal_wording.rs"]
mod input_validation_refusal_wording;
#[path = "all/list_devices.rs"]
mod list_devices;
#[path = "all/list_groups.rs"]
mod list_groups;
#[path = "all/list_identity_providers.rs"]
mod list_identity_providers;
#[path = "all/list_resource_servers.rs"]
mod list_resource_servers;
#[path = "all/list_terms.rs"]
mod list_terms;
#[path = "all/list_user_pool_client_secrets.rs"]
mod list_user_pool_client_secrets;
#[path = "all/list_user_pool_clients.rs"]
mod list_user_pool_clients;
#[path = "all/list_user_pools.rs"]
mod list_user_pools;
#[path = "all/list_users.rs"]
mod list_users;
#[path = "all/list_users_in_group.rs"]
mod list_users_in_group;
#[path = "all/list_web_authn_credentials.rs"]
mod list_web_authn_credentials;
#[path = "all/managed_login_branding.rs"]
mod managed_login_branding;
#[path = "all/managed_login_branding_updates.rs"]
mod managed_login_branding_updates;
#[path = "all/log_delivery_configuration.rs"]
mod log_delivery_configuration;
#[path = "all/log_delivery_destinations.rs"]
mod log_delivery_destinations;
#[path = "all/message_delivery_and_live_codes.rs"]
mod message_delivery_and_live_codes;
#[path = "all/not_found_sentence_per_operation.rs"]
mod not_found_sentence_per_operation;
#[path = "all/per_operation_refusal_wording.rs"]
mod per_operation_refusal_wording;
#[path = "all/pool_configuration_refusal_wire.rs"]
mod pool_configuration_refusal_wire;
#[path = "all/pool_plan_and_csv_header.rs"]
mod pool_plan_and_csv_header;
#[path = "all/pool_well_known_documents.rs"]
mod pool_well_known_documents;
#[path = "all/post_confirmation_trigger.rs"]
mod post_confirmation_trigger;
#[path = "all/pre_token_generation_overrides.rs"]
mod pre_token_generation_overrides;
#[path = "all/provider_link_source_user_claim.rs"]
mod provider_link_source_user_claim;
#[path = "all/provisioned_limits.rs"]
mod provisioned_limits;
#[path = "all/public_user_not_found_message.rs"]
mod public_user_not_found_message;
#[path = "all/refresh_token_form.rs"]
mod refresh_token_form;
#[path = "all/refusal_reports_the_broken_rule.rs"]
mod refusal_reports_the_broken_rule;
#[path = "all/request_validation_gates.rs"]
mod request_validation_gates;
#[path = "all/resend_confirmation_code.rs"]
mod resend_confirmation_code;
#[path = "all/resource_server_lifecycle.rs"]
mod resource_server_lifecycle;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/revocation_links_and_mfa_setup.rs"]
mod revocation_links_and_mfa_setup;
#[path = "all/revoke_token.rs"]
mod revoke_token;
#[path = "all/revoked_session_refusal_sentence.rs"]
mod revoked_session_refusal_sentence;
#[path = "all/risk_configuration.rs"]
mod risk_configuration;
#[path = "all/saml_signing_certificate.rs"]
mod saml_signing_certificate;
#[path = "all/self_service_refusal_wire.rs"]
mod self_service_refusal_wire;
#[path = "all/self_service_sign_up.rs"]
mod self_service_sign_up;
#[path = "all/sensitive_member_refusal_values.rs"]
mod sensitive_member_refusal_values;
#[path = "all/set_user_mfa_preference.rs"]
mod set_user_mfa_preference;
#[path = "all/set_user_settings.rs"]
mod set_user_settings;
#[path = "all/sign_in_lambda_triggers.rs"]
mod sign_in_lambda_triggers;
#[path = "all/sign_in_metric_reads.rs"]
mod sign_in_metric_reads;
#[path = "all/sign_up_client_secret.rs"]
mod sign_up_client_secret;
#[path = "all/software_token_setup.rs"]
mod software_token_setup;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stated_enum_value_set_order.rs"]
mod stated_enum_value_set_order;
#[path = "all/stored_state_normalization.rs"]
mod stored_state_normalization;
#[path = "all/terms_branding_and_passkey_refusal_wire.rs"]
mod terms_branding_and_passkey_refusal_wire;
#[path = "all/terms_documents.rs"]
mod terms_documents;
#[path = "all/terms_not_found_message.rs"]
mod terms_not_found_message;
#[path = "all/terms_pattern_expression_wording.rs"]
mod terms_pattern_expression_wording;
#[path = "all/token_issuer_endpoint.rs"]
mod token_issuer_endpoint;
#[path = "all/token_signing.rs"]
mod token_signing;
#[path = "all/token_signing_algorithm.rs"]
mod token_signing_algorithm;
#[path = "all/ui_customization.rs"]
mod ui_customization;
#[path = "all/update_group.rs"]
mod update_group;
#[path = "all/update_identity_provider.rs"]
mod update_identity_provider;
#[path = "all/update_resource_server.rs"]
mod update_resource_server;
#[path = "all/update_terms.rs"]
mod update_terms;
#[path = "all/update_user_attributes.rs"]
mod update_user_attributes;
#[path = "all/update_user_pool.rs"]
mod update_user_pool;
#[path = "all/update_user_pool_client.rs"]
mod update_user_pool_client;
#[path = "all/update_user_pool_domain.rs"]
mod update_user_pool_domain;
#[path = "all/update_user_pool_domain_not_found.rs"]
mod update_user_pool_domain_not_found;
#[path = "all/user_attribute_verification_code.rs"]
mod user_attribute_verification_code;
#[path = "all/user_auth_event_history.rs"]
mod user_auth_event_history;
#[path = "all/user_import_job_lifecycle.rs"]
mod user_import_job_lifecycle;
#[path = "all/user_migration_trigger.rs"]
mod user_migration_trigger;
#[path = "all/user_pool_auth_metrics.rs"]
mod user_pool_auth_metrics;
#[path = "all/user_pool_domain.rs"]
mod user_pool_domain;
#[path = "all/user_pool_mfa_config.rs"]
mod user_pool_mfa_config;
#[path = "all/user_pool_mfa_posture_rewrite.rs"]
mod user_pool_mfa_posture_rewrite;
#[path = "all/user_pool_replica_deletion.rs"]
mod user_pool_replica_deletion;
#[path = "all/user_pool_replicas.rs"]
mod user_pool_replicas;
#[path = "all/user_pool_request_preconditions.rs"]
mod user_pool_request_preconditions;
#[path = "all/user_pool_server_set_configuration.rs"]
mod user_pool_server_set_configuration;
#[path = "all/user_pool_web_acl_edge.rs"]
mod user_pool_web_acl_edge;
#[path = "all/user_read_projection.rs"]
mod user_read_projection;
#[path = "all/verification_code_pipeline.rs"]
mod verification_code_pipeline;
#[path = "all/web_authn_credential_registration.rs"]
mod web_authn_credential_registration;
#[path = "all/web_authn_refusal_wording.rs"]
mod web_authn_refusal_wording;
#[path = "all/web_authn_registration.rs"]
mod web_authn_registration;
#[path = "all/zz_probe.rs"]
mod zz_probe;
