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
#[path = "all/agent_runtime_endpoint_lifecycle.rs"]
mod agent_runtime_endpoint_lifecycle;
#[path = "all/agent_runtime_endpoint_listing.rs"]
mod agent_runtime_endpoint_listing;
#[path = "all/agent_runtime_lifecycle.rs"]
mod agent_runtime_lifecycle;
#[path = "all/api_key_credential_provider_lifecycle.rs"]
mod api_key_credential_provider_lifecycle;
#[path = "all/browser_lifecycle.rs"]
mod browser_lifecycle;
#[path = "all/browser_listing_pagination.rs"]
mod browser_listing_pagination;
#[path = "all/browser_profile_lifecycle.rs"]
mod browser_profile_lifecycle;
#[path = "all/browser_profile_listing.rs"]
mod browser_profile_listing;
#[path = "all/capacity_provider_runtime_versions.rs"]
mod capacity_provider_runtime_versions;
#[path = "all/code_artifact_object.rs"]
mod code_artifact_object;
#[path = "all/code_interpreter_lifecycle.rs"]
mod code_interpreter_lifecycle;
#[path = "all/configuration_bundle_deletion.rs"]
mod configuration_bundle_deletion;
#[path = "all/configuration_bundle_lifecycle.rs"]
mod configuration_bundle_lifecycle;
#[path = "all/control_plane_preconditions.rs"]
mod control_plane_preconditions;
#[path = "all/credential_provider_external_secret_decryption.rs"]
mod credential_provider_external_secret_decryption;
#[path = "all/dataset_lifecycle.rs"]
mod dataset_lifecycle;
#[path = "all/dataset_schema_validation.rs"]
mod dataset_schema_validation;
#[path = "all/gateway_dependent_children.rs"]
mod gateway_dependent_children;
#[path = "all/gateway_endpoint_url.rs"]
mod gateway_endpoint_url;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/gateway_listings.rs"]
mod gateway_listings;
#[path = "all/gateway_mcp_endpoint.rs"]
mod gateway_mcp_endpoint;
#[path = "all/gateway_mutation.rs"]
mod gateway_mutation;
#[path = "all/gateway_rate_limit_batch.rs"]
mod gateway_rate_limit_batch;
#[path = "all/gateway_rate_limit_creation.rs"]
mod gateway_rate_limit_creation;
#[path = "all/gateway_rule_creation.rs"]
mod gateway_rule_creation;
#[path = "all/gateway_target_deletion.rs"]
mod gateway_target_deletion;
#[path = "all/gateway_target_listing.rs"]
mod gateway_target_listing;
#[path = "all/gateway_target_mutation.rs"]
mod gateway_target_mutation;
#[path = "all/gateway_target_synchronization.rs"]
mod gateway_target_synchronization;
#[path = "all/gateway_target_update_validation.rs"]
mod gateway_target_update_validation;
#[path = "all/gateway_wire_shape.rs"]
mod gateway_wire_shape;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/harness_endpoint_lifecycle.rs"]
mod harness_endpoint_lifecycle;
#[path = "all/harness_lifecycle.rs"]
mod harness_lifecycle;
#[path = "all/harness_version_listing.rs"]
mod harness_version_listing;
#[path = "all/memory_deletion.rs"]
mod memory_deletion;
#[path = "all/memory_lifecycle.rs"]
mod memory_lifecycle;
#[path = "all/memory_mutation.rs"]
mod memory_mutation;
#[path = "all/memory_namespace_key_reference_wording.rs"]
mod memory_namespace_key_reference_wording;
#[path = "all/memory_read_reflects_last_mutation.rs"]
mod memory_read_reflects_last_mutation;
#[path = "all/memory_update_and_read_projection.rs"]
mod memory_update_and_read_projection;
#[path = "all/oauth2_credential_provider_configuration.rs"]
mod oauth2_credential_provider_configuration;
#[path = "all/oauth2_credential_provider_mutation.rs"]
mod oauth2_credential_provider_mutation;
#[path = "all/payment_manager_lifecycle.rs"]
mod payment_manager_lifecycle;
#[path = "all/payment_credential_provider.rs"]
mod payment_credential_provider;
#[path = "all/policy_deletion.rs"]
mod policy_deletion;
#[path = "all/policy_engine_creation.rs"]
mod policy_engine_creation;
#[path = "all/policy_engine_mutation.rs"]
mod policy_engine_mutation;
#[path = "all/policy_plane.rs"]
mod policy_plane;
#[path = "all/payment_credential_provider_external_secret.rs"]
mod payment_credential_provider_external_secret;
#[path = "all/payment_credential_provider_lifecycle.rs"]
mod payment_credential_provider_lifecycle;
#[path = "all/registry_lifecycle.rs"]
mod registry_lifecycle;
#[path = "all/resource_error_message_wording.rs"]
mod resource_error_message_wording;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/runtime_code_object_vended_logs.rs"]
mod runtime_code_object_vended_logs;
#[path = "all/runtime_vended_log_group.rs"]
mod runtime_vended_log_group;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/signing_name_routing.rs"]
mod signing_name_routing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/system_tools.rs"]
mod system_tools;
#[path = "all/token_vault_cmk.rs"]
mod token_vault_cmk;
#[path = "all/tool_and_bundle_listings.rs"]
mod tool_and_bundle_listings;
#[path = "all/tool_resource_statuses_and_name_conflicts.rs"]
mod tool_resource_statuses_and_name_conflicts;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/workload_identity_arn_region.rs"]
mod workload_identity_arn_region;
#[path = "all/workload_identity_lifecycle.rs"]
mod workload_identity_lifecycle;
#[path = "all/workload_identity_mutation.rs"]
mod workload_identity_mutation;
#[path = "all/workload_identity_read_back.rs"]
mod workload_identity_read_back;
