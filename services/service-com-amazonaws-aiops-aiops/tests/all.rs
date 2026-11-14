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
#[path = "all/arn_field_split_is_shared.rs"]
mod arn_field_split_is_shared;
#[path = "all/create_encryption_configuration_forms.rs"]
mod create_encryption_configuration_forms;
#[path = "all/cross_account_configuration_names_an_account.rs"]
mod cross_account_configuration_names_an_account;
#[path = "all/encryption_configuration_keeps_the_named_key.rs"]
mod encryption_configuration_keeps_the_named_key;
#[path = "all/encryption_configuration_names_a_usable_key.rs"]
mod encryption_configuration_names_a_usable_key;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/group_reader_and_id_shape.rs"]
mod group_reader_and_id_shape;
#[path = "all/investigation_group_identifier_forms.rs"]
mod investigation_group_identifier_forms;
#[path = "all/investigation_group_name_identity.rs"]
mod investigation_group_name_identity;
#[path = "all/investigation_group_policy_delete.rs"]
mod investigation_group_policy_delete;
#[path = "all/investigation_group_role_resolution.rs"]
mod investigation_group_role_resolution;
#[path = "all/investigation_group_singleton.rs"]
mod investigation_group_singleton;
#[path = "all/investigation_groups.rs"]
mod investigation_groups;
#[path = "all/list_element_refusal_positions.rs"]
mod list_element_refusal_positions;
#[path = "all/map_member_refusal_paths.rs"]
mod map_member_refusal_paths;
#[path = "all/modelled_map_key_constraints.rs"]
mod modelled_map_key_constraints;
#[path = "all/page_token_refusal_is_modelled.rs"]
mod page_token_refusal_is_modelled;
#[path = "all/page_token_resumes_only_at_a_stored_position.rs"]
mod page_token_resumes_only_at_a_stored_position;
#[path = "all/policy_resource_two_step.rs"]
mod policy_resource_two_step;
#[path = "all/regional_identifier_resolution.rs"]
mod regional_identifier_resolution;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/service_worded_refusals.rs"]
mod service_worded_refusals;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stored_encryption_configuration.rs"]
mod stored_encryption_configuration;
#[path = "all/tag_write_last_modified_stamp.rs"]
mod tag_write_last_modified_stamp;
#[path = "all/tagging_frontend_errors.rs"]
mod tagging_frontend_errors;
#[path = "all/untag_resource_record_stamp.rs"]
mod untag_resource_record_stamp;
#[path = "all/update_investigation_group_identifier.rs"]
mod update_investigation_group_identifier;
#[path = "all/update_omitted_encryption_configuration.rs"]
mod update_omitted_encryption_configuration;
#[path = "all/write_configuration_rejection_chain.rs"]
mod write_configuration_rejection_chain;
