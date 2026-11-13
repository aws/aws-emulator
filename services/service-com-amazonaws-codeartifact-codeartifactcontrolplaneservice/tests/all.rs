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
#[path = "all/delete_idempotence.rs"]
mod delete_idempotence;
#[path = "all/domain_permissions_policy.rs"]
mod domain_permissions_policy;
#[path = "all/domain_repository_lifecycle.rs"]
mod domain_repository_lifecycle;
#[path = "all/external_connection.rs"]
mod external_connection;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/package_group_association.rs"]
mod package_group_association;
#[path = "all/package_group_hierarchy.rs"]
mod package_group_hierarchy;
#[path = "all/package_listing.rs"]
mod package_listing;
#[path = "all/package_version_publish.rs"]
mod package_version_publish;
#[path = "all/repository_endpoint.rs"]
mod repository_endpoint;
#[path = "all/repository_permissions_policy.rs"]
mod repository_permissions_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

// Declared last rather than in sorted position, deliberately: every concurrent change to
// this crate adds its suite in sorted order, so an entry appended here is the one that
// never collides with theirs when two of them land together. The build only requires that
// a suite under `tests/all/` be declared (see `vendor/build-support`); ordering is a
// readability convention, and the sorted list above is intact.
#[path = "all/external_connection_disassociation.rs"]
mod external_connection_disassociation;
#[path = "all/error_message_composition.rs"]
mod error_message_composition;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/removals_and_copies.rs"]
mod removals_and_copies;
#[path = "all/package_version_description.rs"]
mod package_version_description;
#[path = "all/policy_document_rendering.rs"]
mod policy_document_rendering;
#[path = "all/update_operations_persist.rs"]
mod update_operations_persist;
#[path = "all/package_group_arn_encoding.rs"]
mod package_group_arn_encoding;
#[path = "all/package_version_asset_download.rs"]
mod package_version_asset_download;
#[path = "all/package_group_allowed_repositories.rs"]
mod package_group_allowed_repositories;
#[path = "all/package_group_origin_configuration.rs"]
mod package_group_origin_configuration;
#[path = "all/package_version_asset_listing.rs"]
mod package_version_asset_listing;
#[path = "all/sub_package_group_listing.rs"]
mod sub_package_group_listing;
#[path = "all/authorization_token.rs"]
mod authorization_token;
#[path = "all/package_version_listing.rs"]
mod package_version_listing;
#[path = "all/package_version_input_status.rs"]
mod package_version_input_status;
#[path = "all/authorization_token_issuance.rs"]
mod authorization_token_issuance;
#[path = "all/npm_registry_endpoint.rs"]
mod npm_registry_endpoint;
#[path = "all/authorization_token_caller_binding.rs"]
mod authorization_token_caller_binding;
#[path = "all/package_version_state_change_events.rs"]
mod package_version_state_change_events;
#[path = "all/upstream_chain_resolution.rs"]
mod upstream_chain_resolution;
#[path = "all/package_coordinate_refusals.rs"]
mod package_coordinate_refusals;
#[path = "all/repository_authorization.rs"]
mod repository_authorization;
#[path = "all/copy_from_upstream.rs"]
mod copy_from_upstream;
#[path = "all/service_rule_refusals.rs"]
mod service_rule_refusals;
#[path = "all/package_group_listing.rs"]
mod package_group_listing;
#[path = "all/resource_policy_authorization.rs"]
mod resource_policy_authorization;
#[path = "all/policy_delete_idempotence.rs"]
mod policy_delete_idempotence;
#[path = "all/package_listing_filter_pairing.rs"]
mod package_listing_filter_pairing;
#[path = "all/package_version_publish_conflict.rs"]
mod package_version_publish_conflict;
#[path = "all/package_delete_cascade.rs"]
mod package_delete_cascade;
#[path = "all/package_version_not_found_coordinate.rs"]
mod package_version_not_found_coordinate;
#[path = "all/package_version_summary_published_time.rs"]
mod package_version_summary_published_time;
#[path = "all/asset_response_media_type.rs"]
mod asset_response_media_type;
#[path = "all/control_plane_refusal_sentences.rs"]
mod control_plane_refusal_sentences;
#[path = "all/pypi_data_plane.rs"]
mod pypi_data_plane;
#[path = "all/repository_endpoint_credentials.rs"]
mod repository_endpoint_credentials;
#[path = "all/package_version_readme.rs"]
mod package_version_readme;
#[path = "all/domain_resource_fields.rs"]
mod domain_resource_fields;
#[path = "all/arn_addressed_authorization.rs"]
mod arn_addressed_authorization;
#[path = "all/root_package_group_description.rs"]
mod root_package_group_description;
#[path = "all/external_connection_not_found_sentence.rs"]
mod external_connection_not_found_sentence;
