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
#[path = "all/codegen_job_lifecycle.rs"]
mod codegen_job_lifecycle;

#[path = "all/codegen_job_list.rs"]
mod codegen_job_list;

#[path = "all/codegen_job_scope.rs"]
mod codegen_job_scope;

#[path = "all/codegen_job_tagging.rs"]
mod codegen_job_tagging;

#[path = "all/component_lifecycle.rs"]
mod component_lifecycle;

#[path = "all/component_refusals.rs"]
mod component_refusals;

#[path = "all/component_reserved_property_name.rs"]
mod component_reserved_property_name;

#[path = "all/component_wire_fidelity.rs"]
mod component_wire_fidelity;

#[path = "all/emulator_store_bound_refusals.rs"]
mod emulator_store_bound_refusals;

#[path = "all/exchanged_session_names.rs"]
mod exchanged_session_names;

#[path = "all/export_components.rs"]
mod export_components;

#[path = "all/export_forms.rs"]
mod export_forms;

#[path = "all/export_themes.rs"]
mod export_themes;

#[path = "all/form_delete.rs"]
mod form_delete;

#[path = "all/form_lifecycle.rs"]
mod form_lifecycle;

#[path = "all/form_list.rs"]
mod form_list;

#[path = "all/form_sectional_element_values.rs"]
mod form_sectional_element_values;

#[path = "all/form_update.rs"]
mod form_update;

#[path = "all/invalid_parameter_refusals.rs"]
mod invalid_parameter_refusals;

#[path = "all/metadata_flags.rs"]
mod metadata_flags;

#[path = "all/page_token_collection_scope.rs"]
mod page_token_collection_scope;

#[path = "all/page_token_cursor_bounds.rs"]
mod page_token_cursor_bounds;

#[path = "all/provider_token_refresh.rs"]
mod provider_token_refresh;

#[path = "all/provider_tokens.rs"]
mod provider_tokens;

#[path = "all/read_and_delete_scope_classes.rs"]
mod read_and_delete_scope_classes;

#[path = "all/resource_error_messages.rs"]
mod resource_error_messages;

#[path = "all/resource_name_uniqueness.rs"]
mod resource_name_uniqueness;

#[path = "all/resource_tagging.rs"]
mod resource_tagging;

#[path = "all/response_front_door.rs"]
mod response_front_door;

#[path = "all/shared_page_walk.rs"]
mod shared_page_walk;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/tag_member_shape.rs"]
mod tag_member_shape;

#[path = "all/theme_deletion.rs"]
mod theme_deletion;

#[path = "all/theme_lifecycle.rs"]
mod theme_lifecycle;

#[path = "all/theme_list.rs"]
mod theme_list;

#[path = "all/theme_scope_classes.rs"]
mod theme_scope_classes;

#[path = "all/theme_update.rs"]
mod theme_update;

#[path = "all/theme_updates.rs"]
mod theme_updates;

#[path = "all/token_refusal_draws_no_secret.rs"]
mod token_refusal_draws_no_secret;

#[path = "all/update_absent_id_refusal_class.rs"]
mod update_absent_id_refusal_class;

#[path = "all/widened_error_contract.rs"]
mod widened_error_contract;

#[path = "all/write_stamp_under_the_store_lock.rs"]
mod write_stamp_under_the_store_lock;

#[path = "all/wrong_scope_sentence.rs"]
mod wrong_scope_sentence;
