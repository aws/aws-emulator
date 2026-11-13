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
#[path = "all/authorization_before_existence.rs"]
mod authorization_before_existence;
#[path = "all/caller_principal_conditions.rs"]
mod caller_principal_conditions;
#[path = "all/conditioned_authorization.rs"]
mod conditioned_authorization;
#[path = "all/identity_based_authorization.rs"]
mod identity_based_authorization;
#[path = "all/secret_batch_get.rs"]
mod secret_batch_get;
#[path = "all/secret_client_request_token.rs"]
mod secret_client_request_token;
#[path = "all/secret_conditioned_wildcard_attach.rs"]
mod secret_conditioned_wildcard_attach;
#[path = "all/secret_cross_account_resource_policy.rs"]
mod secret_cross_account_resource_policy;
#[path = "all/secret_customer_key_caller_authority.rs"]
mod secret_customer_key_caller_authority;
#[path = "all/secret_delete_metadata_and_filters.rs"]
mod secret_delete_metadata_and_filters;
#[path = "all/secret_deletion_window.rs"]
mod secret_deletion_window;
#[path = "all/secret_denial_resource_clause.rs"]
mod secret_denial_resource_clause;
#[path = "all/secret_error_responses.rs"]
mod secret_error_responses;
#[path = "all/secret_key_change_rekeys_staged_versions.rs"]
mod secret_key_change_rekeys_staged_versions;
#[path = "all/secret_kms_access_denied.rs"]
mod secret_kms_access_denied;
#[path = "all/secret_kms_key_refusal_classes.rs"]
mod secret_kms_key_refusal_classes;
#[path = "all/secret_kms_key_state.rs"]
mod secret_kms_key_state;
#[path = "all/secret_lifecycle.rs"]
mod secret_lifecycle;
#[path = "all/secret_pending_eviction.rs"]
mod secret_pending_eviction;
#[path = "all/secret_policy_validation.rs"]
mod secret_policy_validation;
#[path = "all/secret_public_policy_conditions.rs"]
mod secret_public_policy_conditions;
#[path = "all/secret_public_policy_message.rs"]
mod secret_public_policy_message;
#[path = "all/secret_random_password.rs"]
mod secret_random_password;
#[path = "all/secret_refused_write_staging_report.rs"]
mod secret_refused_write_staging_report;
#[path = "all/secret_regional_replication.rs"]
mod secret_regional_replication;
#[path = "all/secret_resource_policy.rs"]
mod secret_resource_policy;
#[path = "all/secret_resource_policy_reported_form.rs"]
mod secret_resource_policy_reported_form;
#[path = "all/secret_resource_policy_targeting.rs"]
mod secret_resource_policy_targeting;
#[path = "all/secret_restore.rs"]
mod secret_restore;
#[path = "all/secret_rotation.rs"]
mod secret_rotation;
#[path = "all/secret_rotation_cancel.rs"]
mod secret_rotation_cancel;
#[path = "all/secret_rotation_driver.rs"]
mod secret_rotation_driver;
#[path = "all/secret_rotation_span.rs"]
mod secret_rotation_span;
#[path = "all/secret_service_owned_writes.rs"]
mod secret_service_owned_writes;
#[path = "all/secret_stop_replication_refusal.rs"]
mod secret_stop_replication_refusal;
#[path = "all/secret_tags_and_replication.rs"]
mod secret_tags_and_replication;
#[path = "all/secret_value_lookup_messages.rs"]
mod secret_value_lookup_messages;
#[path = "all/secret_version_history.rs"]
mod secret_version_history;
#[path = "all/secret_version_id_token_forms.rs"]
mod secret_version_id_token_forms;
#[path = "all/secret_version_ids.rs"]
mod secret_version_ids;
#[path = "all/secret_version_ids_are_caller_named.rs"]
mod secret_version_ids_are_caller_named;
#[path = "all/secret_version_stage_stale_remove_from.rs"]
mod secret_version_stage_stale_remove_from;
#[path = "all/secret_version_stages.rs"]
mod secret_version_stages;
#[path = "all/secret_write_targets.rs"]
mod secret_write_targets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
