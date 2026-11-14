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
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/authorization_token.rs"]
mod authorization_token;
#[path = "all/batch_get_image.rs"]
mod batch_get_image;
#[path = "all/create_repository_contract.rs"]
mod create_repository_contract;
#[path = "all/error_message_construction.rs"]
mod error_message_construction;
#[path = "all/image_action_events.rs"]
mod image_action_events;
#[path = "all/image_metadata.rs"]
mod image_metadata;
#[path = "all/image_replication_status.rs"]
mod image_replication_status;
#[path = "all/image_scan.rs"]
mod image_scan;
#[path = "all/image_scan_filesystem.rs"]
mod image_scan_filesystem;
#[path = "all/image_scan_outcome.rs"]
mod image_scan_outcome;
#[path = "all/image_signing_status.rs"]
mod image_signing_status;
#[path = "all/image_storage_class_identification.rs"]
mod image_storage_class_identification;
#[path = "all/image_storage_class_transitions.rs"]
mod image_storage_class_transitions;
#[path = "all/image_tag_mutability.rs"]
mod image_tag_mutability;
#[path = "all/layer_availability.rs"]
mod layer_availability;
#[path = "all/layer_download_signature.rs"]
mod layer_download_signature;
#[path = "all/layer_download_url.rs"]
mod layer_download_url;
#[path = "all/layer_part_upload.rs"]
mod layer_part_upload;
#[path = "all/layer_upload_commit.rs"]
mod layer_upload_commit;
#[path = "all/layer_upload_completion_digest_set.rs"]
mod layer_upload_completion_digest_set;
#[path = "all/layer_upload_sessions.rs"]
mod layer_upload_sessions;
#[path = "all/lifecycle_policy.rs"]
mod lifecycle_policy;
#[path = "all/lifecycle_policy_document_identity.rs"]
mod lifecycle_policy_document_identity;
#[path = "all/lifecycle_policy_preview.rs"]
mod lifecycle_policy_preview;
#[path = "all/lifecycle_policy_validator_report.rs"]
mod lifecycle_policy_validator_report;
#[path = "all/manifest_media_type_negotiation.rs"]
mod manifest_media_type_negotiation;
#[path = "all/paginated_read_scoping.rs"]
mod paginated_read_scoping;
#[path = "all/parameter_constraint_clauses.rs"]
mod parameter_constraint_clauses;
#[path = "all/policy_document_account_principal.rs"]
mod policy_document_account_principal;
#[path = "all/policy_document_canonical_form.rs"]
mod policy_document_canonical_form;
#[path = "all/policy_document_principal_forms.rs"]
mod policy_document_principal_forms;
#[path = "all/policy_document_refusal_messages.rs"]
mod policy_document_refusal_messages;
#[path = "all/policy_document_scope.rs"]
mod policy_document_scope;
#[path = "all/pull_through_cache_rule_deletion.rs"]
mod pull_through_cache_rule_deletion;
#[path = "all/pull_through_cache_rule_validation.rs"]
mod pull_through_cache_rule_validation;
#[path = "all/pull_through_cache_rules.rs"]
mod pull_through_cache_rules;
#[path = "all/pull_time_update_exclusion_quota.rs"]
mod pull_time_update_exclusion_quota;
#[path = "all/pull_time_update_exclusion_removal.rs"]
mod pull_time_update_exclusion_removal;
#[path = "all/pull_time_update_exclusions.rs"]
mod pull_time_update_exclusions;
#[path = "all/published_registry_address.rs"]
mod published_registry_address;
#[path = "all/refusal_message_sources.rs"]
mod refusal_message_sources;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/registry_policy.rs"]
mod registry_policy;
#[path = "all/registry_replication_configuration.rs"]
mod registry_replication_configuration;
#[path = "all/registry_resource_reads.rs"]
mod registry_resource_reads;
#[path = "all/registry_scanning_configuration.rs"]
mod registry_scanning_configuration;
#[path = "all/registry_v2_pull.rs"]
mod registry_v2_pull;
#[path = "all/repository_access_authorization.rs"]
mod repository_access_authorization;
#[path = "all/repository_creation_template_encryption.rs"]
mod repository_creation_template_encryption;
#[path = "all/repository_creation_template_updates.rs"]
mod repository_creation_template_updates;
#[path = "all/repository_creation_templates.rs"]
mod repository_creation_templates;
#[path = "all/repository_image_lifecycle.rs"]
mod repository_image_lifecycle;
#[path = "all/repository_policy.rs"]
mod repository_policy;
#[path = "all/repository_policy_principal.rs"]
mod repository_policy_principal;
#[path = "all/request_preconditions.rs"]
mod request_preconditions;
#[path = "all/resource_tag_mutations.rs"]
mod resource_tag_mutations;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scanning_configuration.rs"]
mod scanning_configuration;
#[path = "all/signing_configuration.rs"]
mod signing_configuration;
#[path = "all/signing_configuration_delete.rs"]
mod signing_configuration_delete;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/wire.rs"]
mod wire;
