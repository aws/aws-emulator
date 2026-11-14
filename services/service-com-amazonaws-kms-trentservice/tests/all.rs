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
#[path = "all/account_level_authorization.rs"]
mod account_level_authorization;
#[path = "all/alias_lifecycle.rs"]
mod alias_lifecycle;
#[path = "all/aliases.rs"]
mod aliases;
#[path = "all/asymmetric_decrypt.rs"]
mod asymmetric_decrypt;
#[path = "all/asymmetric_encrypt.rs"]
mod asymmetric_encrypt;
#[path = "all/aws_managed_key_administration.rs"]
mod aws_managed_key_administration;
#[path = "all/aws_managed_keys.rs"]
mod aws_managed_keys;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/create_custom_key_store.rs"]
mod create_custom_key_store;
#[path = "all/cross_region_material.rs"]
mod cross_region_material;
#[path = "all/custom_key_store_connection.rs"]
mod custom_key_store_connection;
#[path = "all/custom_key_stores.rs"]
mod custom_key_stores;
#[path = "all/data_key.rs"]
mod data_key;
#[path = "all/data_key_length_exclusivity.rs"]
mod data_key_length_exclusivity;
#[path = "all/data_key_pair.rs"]
mod data_key_pair;
#[path = "all/data_key_pair_without_plaintext.rs"]
mod data_key_pair_without_plaintext;
#[path = "all/data_key_without_plaintext.rs"]
mod data_key_without_plaintext;
#[path = "all/decrypt_key_origin.rs"]
mod decrypt_key_origin;
#[path = "all/ed25519_prehash.rs"]
mod ed25519_prehash;
#[path = "all/encryption_context_conditions.rs"]
mod encryption_context_conditions;
#[path = "all/encryption_context_validation.rs"]
mod encryption_context_validation;
#[path = "all/error_semantics.rs"]
mod error_semantics;
#[path = "all/external_key_store_create_probe.rs"]
mod external_key_store_create_probe;
#[path = "all/external_key_store_proxy.rs"]
mod external_key_store_proxy;
#[path = "all/get_key_policy_reads.rs"]
mod get_key_policy_reads;
#[path = "all/get_key_rotation_status_reads.rs"]
mod get_key_rotation_status_reads;
#[path = "all/grant_authorization.rs"]
mod grant_authorization;
#[path = "all/grant_read_projection.rs"]
mod grant_read_projection;
#[path = "all/grants.rs"]
mod grants;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/import_parameters.rs"]
mod import_parameters;
#[path = "all/imported_key_material.rs"]
mod imported_key_material;
#[path = "all/key_deletion_cancellation.rs"]
mod key_deletion_cancellation;
#[path = "all/key_last_usage.rs"]
mod key_last_usage;
#[path = "all/key_last_usage_monotonicity.rs"]
mod key_last_usage_monotonicity;
#[path = "all/key_lifecycle.rs"]
mod key_lifecycle;
#[path = "all/key_policy.rs"]
mod key_policy;
#[path = "all/key_policy_conditions.rs"]
mod key_policy_conditions;
#[path = "all/key_policy_deny_scope.rs"]
mod key_policy_deny_scope;
#[path = "all/key_policy_lockout.rs"]
mod key_policy_lockout;
#[path = "all/key_rotation_on_demand.rs"]
mod key_rotation_on_demand;
#[path = "all/key_rotation_schedule.rs"]
mod key_rotation_schedule;
#[path = "all/key_specs.rs"]
mod key_specs;
#[path = "all/list_grants_reads.rs"]
mod list_grants_reads;
#[path = "all/list_key_rotations_reads.rs"]
mod list_key_rotations_reads;
#[path = "all/list_resource_tags_reads.rs"]
mod list_resource_tags_reads;
#[path = "all/message_authentication_codes.rs"]
mod message_authentication_codes;
#[path = "all/multi_region_primary.rs"]
mod multi_region_primary;
#[path = "all/multi_region_primary_serialization.rs"]
mod multi_region_primary_serialization;
#[path = "all/multi_region_primary_settlement.rs"]
mod multi_region_primary_settlement;
#[path = "all/multi_region_rotation_sharing.rs"]
mod multi_region_rotation_sharing;
#[path = "all/named_grant_identity.rs"]
mod named_grant_identity;
#[path = "all/no_allow_refusal_clause.rs"]
mod no_allow_refusal_clause;
#[path = "all/principal_attached_policy_authority.rs"]
mod principal_attached_policy_authority;
#[path = "all/public_key.rs"]
mod public_key;
#[path = "all/random_bytes.rs"]
mod random_bytes;
#[path = "all/re_encrypt.rs"]
mod re_encrypt;
#[path = "all/read_projection_fidelity.rs"]
mod read_projection_fidelity;
#[path = "all/refusal_and_rotation_contracts.rs"]
mod refusal_and_rotation_contracts;
#[path = "all/refusal_fidelity.rs"]
mod refusal_fidelity;
#[path = "all/replicate_key.rs"]
mod replicate_key;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/signature_verification.rs"]
mod signature_verification;
#[path = "all/signatures.rs"]
mod signatures;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_refusal_wording.rs"]
mod tagging_refusal_wording;
#[path = "all/update_custom_key_store.rs"]
mod update_custom_key_store;
#[path = "all/update_primary_region_preconditions.rs"]
mod update_primary_region_preconditions;
#[path = "all/via_service_confinement.rs"]
mod via_service_confinement;
