// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/archive_creation_frequency.rs"]
mod archive_creation_frequency;
#[path = "all/configuration_kind_from_policy_type.rs"]
mod configuration_kind_from_policy_type;
#[path = "all/create_policy_kind_materialization.rs"]
mod create_policy_kind_materialization;
#[path = "all/default_policy_configuration_one_place_each.rs"]
mod default_policy_configuration_one_place_each;
#[path = "all/default_policy_configuration_precedence.rs"]
mod default_policy_configuration_precedence;
#[path = "all/default_policy_managed_type.rs"]
mod default_policy_managed_type;
#[path = "all/default_policy_only_body_parameter_set.rs"]
mod default_policy_only_body_parameter_set;
#[path = "all/default_policy_only_parameters.rs"]
mod default_policy_only_parameters;
#[path = "all/default_policy_only_refusal_composition.rs"]
mod default_policy_only_refusal_composition;
#[path = "all/default_policy_requires_a_resource_type.rs"]
mod default_policy_requires_a_resource_type;
#[path = "all/default_policy_resource_type.rs"]
mod default_policy_resource_type;
#[path = "all/effective_cadence_ordering.rs"]
mod effective_cadence_ordering;
#[path = "all/error_paths.rs"]
mod error_paths;
#[path = "all/error_payload_codes.rs"]
mod error_payload_codes;
#[path = "all/execution_role_pass_role.rs"]
mod execution_role_pass_role;
#[path = "all/interval_validation.rs"]
mod interval_validation;
#[path = "all/invalid_request_parameter_lists.rs"]
mod invalid_request_parameter_lists;
#[path = "all/lifecycle_policy_crud.rs"]
mod lifecycle_policy_crud;
#[path = "all/materialized_create_interval_round_trip.rs"]
mod materialized_create_interval_round_trip;
#[path = "all/pass_role_precedes_shape_refusals.rs"]
mod pass_role_precedes_shape_refusals;
#[path = "all/policy_configuration_validation.rs"]
mod policy_configuration_validation;
#[path = "all/policy_identity_immutability.rs"]
mod policy_identity_immutability;
#[path = "all/policy_language_immutability.rs"]
mod policy_language_immutability;
#[path = "all/policy_read_projection.rs"]
mod policy_read_projection;
#[path = "all/policy_type.rs"]
mod policy_type;
#[path = "all/refused_parameter_element_names.rs"]
mod refused_parameter_element_names;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/reserved_tag_key_refusal_class.rs"]
mod reserved_tag_key_refusal_class;
#[path = "all/schedule_retention_floor.rs"]
mod schedule_retention_floor;
#[path = "all/schedule_retention_required.rs"]
mod schedule_retention_required;
#[path = "all/schedule_rule_forms.rs"]
mod schedule_rule_forms;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_arn_resolution.rs"]
mod tag_arn_resolution;
#[path = "all/tag_write_date_modified.rs"]
mod tag_write_date_modified;
#[path = "all/unresolvable_policy_id.rs"]
mod unresolvable_policy_id;
#[path = "all/validation_refusal_message_templates.rs"]
mod validation_refusal_message_templates;
#[path = "all/custom_policy_only_parameters.rs"]
mod custom_policy_only_parameters;
