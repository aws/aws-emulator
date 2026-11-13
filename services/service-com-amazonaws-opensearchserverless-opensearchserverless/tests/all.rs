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
#[path = "all/absent_resource_wording.rs"]
mod absent_resource_wording;
#[path = "all/access_policy.rs"]
mod access_policy;
#[path = "all/access_policy_client_token_round_trip.rs"]
mod access_policy_client_token_round_trip;
#[path = "all/access_policy_listing.rs"]
mod access_policy_listing;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/account_settings_audit.rs"]
mod account_settings_audit;
#[path = "all/account_settings_write_outcome.rs"]
mod account_settings_write_outcome;
#[path = "all/announced_collection_endpoints.rs"]
mod announced_collection_endpoints;
#[path = "all/audit_capture.rs"]
mod audit_capture;
#[path = "all/batch_selector_rule.rs"]
mod batch_selector_rule;
#[path = "all/client_error_status.rs"]
mod client_error_status;
#[path = "all/client_token_scope.rs"]
mod client_token_scope;
#[path = "all/collection_data_plane.rs"]
mod collection_data_plane;
#[path = "all/collection_endpoint_authorization.rs"]
mod collection_endpoint_authorization;
#[path = "all/collection_group_capacity.rs"]
mod collection_group_capacity;
#[path = "all/collection_group_collection_count.rs"]
mod collection_group_collection_count;
#[path = "all/collection_group_delete.rs"]
mod collection_group_delete;
#[path = "all/collection_group_lifecycle.rs"]
mod collection_group_lifecycle;
#[path = "all/collection_group_listing.rs"]
mod collection_group_listing;
#[path = "all/collection_group_membership.rs"]
mod collection_group_membership;
#[path = "all/collection_lifecycle.rs"]
mod collection_lifecycle;
#[path = "all/collection_search_determinism.rs"]
mod collection_search_determinism;
#[path = "all/delete_token_replay.rs"]
mod delete_token_replay;
#[path = "all/effective_lifecycle_policy.rs"]
mod effective_lifecycle_policy;
#[path = "all/encryption_indexes_and_tagging.rs"]
mod encryption_indexes_and_tagging;
#[path = "all/encryption_policy_precondition.rs"]
mod encryption_policy_precondition;
#[path = "all/index_create_retry_precedence.rs"]
mod index_create_retry_precedence;
#[path = "all/index_delete.rs"]
mod index_delete;
#[path = "all/index_name_rule.rs"]
mod index_name_rule;
#[path = "all/index_not_found_wording.rs"]
mod index_not_found_wording;
#[path = "all/index_schema_update.rs"]
mod index_schema_update;
#[path = "all/lifecycle_policy.rs"]
mod lifecycle_policy;
#[path = "all/lifecycle_policy_delete.rs"]
mod lifecycle_policy_delete;
#[path = "all/lifecycle_policy_listing.rs"]
mod lifecycle_policy_listing;
#[path = "all/lifecycle_policy_update.rs"]
mod lifecycle_policy_update;
#[path = "all/list_length_refusal_clause.rs"]
mod list_length_refusal_clause;
#[path = "all/metadata_update_keeps_the_collection_deletable.rs"]
mod metadata_update_keeps_the_collection_deletable;
#[path = "all/minted_identifier_form.rs"]
mod minted_identifier_form;
#[path = "all/minted_identifiers.rs"]
mod minted_identifiers;
#[path = "all/policies_stats.rs"]
mod policies_stats;
#[path = "all/policy_and_group_writes.rs"]
mod policy_and_group_writes;
#[path = "all/policy_document_shape.rs"]
mod policy_document_shape;
#[path = "all/policy_text_parse.rs"]
mod policy_text_parse;
#[path = "all/refusal_plane.rs"]
mod refusal_plane;
#[path = "all/reported_pattern.rs"]
mod reported_pattern;
#[path = "all/security_config_update.rs"]
mod security_config_update;
#[path = "all/security_configs.rs"]
mod security_configs;
#[path = "all/security_plane_audit.rs"]
mod security_plane_audit;
#[path = "all/security_policy_listing.rs"]
mod security_policy_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_uniqueness.rs"]
mod tag_key_uniqueness;
#[path = "all/tag_key_uniqueness_precedence.rs"]
mod tag_key_uniqueness_precedence;
#[path = "all/tag_write_timestamps.rs"]
mod tag_write_timestamps;
#[path = "all/tagging_plane_resource_kinds.rs"]
mod tagging_plane_resource_kinds;
#[path = "all/tagging_resource_arn_precondition.rs"]
mod tagging_resource_arn_precondition;
#[path = "all/tagging_resource_arn_syntax.rs"]
mod tagging_resource_arn_syntax;
#[path = "all/update_collection_token_scope.rs"]
mod update_collection_token_scope;
#[path = "all/vpc_endpoint.rs"]
mod vpc_endpoint;
#[path = "all/vpc_endpoint_batch_read.rs"]
mod vpc_endpoint_batch_read;
#[path = "all/vpc_endpoint_delete.rs"]
mod vpc_endpoint_delete;
#[path = "all/vpc_endpoint_delete_token.rs"]
mod vpc_endpoint_delete_token;
#[path = "all/vpc_endpoint_id_set_uniqueness.rs"]
mod vpc_endpoint_id_set_uniqueness;
#[path = "all/vpc_endpoint_listing.rs"]
mod vpc_endpoint_listing;
#[path = "all/vpc_endpoint_retained_subnets.rs"]
mod vpc_endpoint_retained_subnets;
#[path = "all/vpc_endpoint_update.rs"]
mod vpc_endpoint_update;
