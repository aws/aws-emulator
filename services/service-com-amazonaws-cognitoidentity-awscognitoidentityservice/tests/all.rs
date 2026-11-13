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
#[path = "all/ambiguous_role_resolution.rs"]
mod ambiguous_role_resolution;
#[path = "all/anonymous_credential_exchange.rs"]
mod anonymous_credential_exchange;
#[path = "all/anonymous_credential_vend.rs"]
mod anonymous_credential_vend;
#[path = "all/aws_refusal_wording.rs"]
mod aws_refusal_wording;
#[path = "all/basic_flow_gate.rs"]
mod basic_flow_gate;
#[path = "all/constraint_refusal_wording.rs"]
mod constraint_refusal_wording;
#[path = "all/control_plane_audit.rs"]
mod control_plane_audit;
#[path = "all/custom_role_arn_from_token_roles.rs"]
mod custom_role_arn_from_token_roles;
#[path = "all/delete_identities.rs"]
mod delete_identities;
#[path = "all/deleted_pool_and_developer_user_refusals.rs"]
mod deleted_pool_and_developer_user_refusals;
#[path = "all/describe_identity.rs"]
mod describe_identity;
#[path = "all/developer_credentials_authorization.rs"]
mod developer_credentials_authorization;
#[path = "all/developer_identity_resolution.rs"]
mod developer_identity_resolution;
#[path = "all/developer_identity_tokens.rs"]
mod developer_identity_tokens;
#[path = "all/developer_link_conflict_refusals.rs"]
mod developer_link_conflict_refusals;
#[path = "all/developer_login_linking.rs"]
mod developer_login_linking;
#[path = "all/developer_provider_name_immutability.rs"]
mod developer_provider_name_immutability;
#[path = "all/iam_oidc_federation.rs"]
mod iam_oidc_federation;
#[path = "all/identity_data_events.rs"]
mod identity_data_events;
#[path = "all/identity_link_lifecycle.rs"]
mod identity_link_lifecycle;
#[path = "all/identity_pool_flow.rs"]
mod identity_pool_flow;
#[path = "all/identity_pool_response_members.rs"]
mod identity_pool_response_members;
#[path = "all/identity_pool_roles.rs"]
mod identity_pool_roles;
#[path = "all/identity_pool_roles_presence.rs"]
mod identity_pool_roles_presence;
#[path = "all/list_identity_pools.rs"]
mod list_identity_pools;
#[path = "all/login_authorized_refusals.rs"]
mod login_authorized_refusals;
#[path = "all/login_issuer_precedence.rs"]
mod login_issuer_precedence;
#[path = "all/login_token_window.rs"]
mod login_token_window;
#[path = "all/lookup_developer_identity.rs"]
mod lookup_developer_identity;
#[path = "all/mapped_role_resolution.rs"]
mod mapped_role_resolution;
#[path = "all/merge_developer_identities.rs"]
mod merge_developer_identities;
#[path = "all/merged_identity_retention.rs"]
mod merged_identity_retention;
#[path = "all/minted_token_identity_binding.rs"]
mod minted_token_identity_binding;
#[path = "all/open_id_token.rs"]
mod open_id_token;
#[path = "all/pool_configuration_rules.rs"]
mod pool_configuration_rules;
#[path = "all/pool_without_identity_providers.rs"]
mod pool_without_identity_providers;
#[path = "all/principal_tag_attribute_map.rs"]
mod principal_tag_attribute_map;
#[path = "all/provider_configured_mappings.rs"]
mod provider_configured_mappings;
#[path = "all/redeemed_token_methods.rs"]
mod redeemed_token_methods;
#[path = "all/resource_not_found_precedence.rs"]
mod resource_not_found_precedence;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/server_side_token_check.rs"]
mod server_side_token_check;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stated_collection_order.rs"]
mod stated_collection_order;
#[path = "all/unlink_developer_identity.rs"]
mod unlink_developer_identity;
#[path = "all/unlink_identity.rs"]
mod unlink_identity;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_identity_pool.rs"]
mod update_identity_pool;
#[path = "all/vended_role_session.rs"]
mod vended_role_session;
#[path = "all/vended_session_identity_record.rs"]
mod vended_session_identity_record;
#[path = "all/vended_session_outlives_pool_lifecycle.rs"]
mod vended_session_outlives_pool_lifecycle;
#[path = "all/wire_error_envelope.rs"]
mod wire_error_envelope;
