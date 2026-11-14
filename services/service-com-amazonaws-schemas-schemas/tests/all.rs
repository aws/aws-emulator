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
#[path = "all/aws_managed_and_versions.rs"]
mod aws_managed_and_versions;
#[path = "all/aws_managed_catalog_metadata.rs"]
mod aws_managed_catalog_metadata;
#[path = "all/aws_managed_event_catalog.rs"]
mod aws_managed_event_catalog;
#[path = "all/catalogue_publications.rs"]
mod catalogue_publications;
#[path = "all/catalogue_source_inventory.rs"]
mod catalogue_source_inventory;
#[path = "all/code_binding_implied_types.rs"]
mod code_binding_implied_types;
#[path = "all/code_binding_lifecycle.rs"]
mod code_binding_lifecycle;
#[path = "all/code_binding_source.rs"]
mod code_binding_source;
#[path = "all/code_bindings.rs"]
mod code_bindings;
#[path = "all/code_bindings_for_managed_schemas.rs"]
mod code_bindings_for_managed_schemas;
#[path = "all/create_discoverer.rs"]
mod create_discoverer;
#[path = "all/describe_discoverer.rs"]
mod describe_discoverer;
#[path = "all/discovered_schema_inference.rs"]
mod discovered_schema_inference;
#[path = "all/discoverer_deletion.rs"]
mod discoverer_deletion;
#[path = "all/discoverer_id_derivation.rs"]
mod discoverer_id_derivation;
#[path = "all/discoverer_state.rs"]
mod discoverer_state;
#[path = "all/draft4_openapi_vocabulary.rs"]
mod draft4_openapi_vocabulary;
#[path = "all/duplicate_create_refusals.rs"]
mod duplicate_create_refusals;
#[path = "all/empty_client_token.rs"]
mod empty_client_token;
#[path = "all/empty_next_token.rs"]
mod empty_next_token;
#[path = "all/empty_response_framing.rs"]
mod empty_response_framing;
#[path = "all/empty_result_statuses.rs"]
mod empty_result_statuses;
#[path = "all/empty_schema_version.rs"]
mod empty_schema_version;
#[path = "all/empty_scope.rs"]
mod empty_scope;
#[path = "all/error_body_codes.rs"]
mod error_body_codes;
#[path = "all/export_reads_the_stored_schema.rs"]
mod export_reads_the_stored_schema;
#[path = "all/export_roots_the_draft4_document.rs"]
mod export_roots_the_draft4_document;
#[path = "all/iso8601_timestamps.rs"]
mod iso8601_timestamps;
#[path = "all/json_refusal_diagnostics.rs"]
mod json_refusal_diagnostics;
#[path = "all/list_discoverers.rs"]
mod list_discoverers;
#[path = "all/malformed_content_and_policy_deletion.rs"]
mod malformed_content_and_policy_deletion;
#[path = "all/name_constraints.rs"]
mod name_constraints;
#[path = "all/next_token_lifetime.rs"]
mod next_token_lifetime;
#[path = "all/next_token_spellings.rs"]
mod next_token_spellings;
#[path = "all/policy_registry_name.rs"]
mod policy_registry_name;
#[path = "all/published_catalogue_entries.rs"]
mod published_catalogue_entries;
#[path = "all/published_document_bytes.rs"]
mod published_document_bytes;
#[path = "all/published_record_identity.rs"]
mod published_record_identity;
#[path = "all/registry_schema_lifecycle.rs"]
mod registry_schema_lifecycle;
#[path = "all/registry_write_resolution.rs"]
mod registry_write_resolution;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tag_application.rs"]
mod resource_tag_application;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/schema_last_modified.rs"]
mod schema_last_modified;
#[path = "all/schema_name_collation.rs"]
mod schema_name_collation;
#[path = "all/schema_read_resolution.rs"]
mod schema_read_resolution;
#[path = "all/schema_type_admission.rs"]
mod schema_type_admission;
#[path = "all/schema_type_after_version_deletion.rs"]
mod schema_type_after_version_deletion;
#[path = "all/schema_version_and_export_sequences.rs"]
mod schema_version_and_export_sequences;
#[path = "all/schema_version_deletion.rs"]
mod schema_version_deletion;
#[path = "all/schema_version_label_spellings.rs"]
mod schema_version_label_spellings;
#[path = "all/search_keyword_index.rs"]
mod search_keyword_index;
#[path = "all/search_schemas.rs"]
mod search_schemas;
#[path = "all/search_token_selection.rs"]
mod search_token_selection;
#[path = "all/service_owned_policy_writes.rs"]
mod service_owned_policy_writes;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_discoverer.rs"]
mod stop_discoverer;
#[path = "all/update_description.rs"]
mod update_description;
#[path = "all/update_discoverer.rs"]
mod update_discoverer;
#[path = "all/update_schema_versions.rs"]
mod update_schema_versions;
