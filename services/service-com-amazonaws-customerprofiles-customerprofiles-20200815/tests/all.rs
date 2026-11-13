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
#[path = "all/batch_get_profile.rs"]
mod batch_get_profile;
#[path = "all/batch_put_profile_object.rs"]
mod batch_put_profile_object;
#[path = "all/calculated_attribute_definitions.rs"]
mod calculated_attribute_definitions;
#[path = "all/calculated_attribute_readiness.rs"]
mod calculated_attribute_readiness;
#[path = "all/delete_calculated_attribute_definition.rs"]
mod delete_calculated_attribute_definition;
#[path = "all/delete_profile_object_type.rs"]
mod delete_profile_object_type;
#[path = "all/domain_profile_lifecycle.rs"]
mod domain_profile_lifecycle;
#[path = "all/merge_profiles.rs"]
mod merge_profiles;
#[path = "all/modelled_refusal_envelope.rs"]
mod modelled_refusal_envelope;
#[path = "all/profile_collection_messages.rs"]
mod profile_collection_messages;
#[path = "all/profile_keys.rs"]
mod profile_keys;
#[path = "all/profile_not_found_message.rs"]
mod profile_not_found_message;
#[path = "all/profile_object_type_identity.rs"]
mod profile_object_type_identity;
#[path = "all/profile_object_types.rs"]
mod profile_object_types;
#[path = "all/profile_objects.rs"]
mod profile_objects;
#[path = "all/profile_string_alternatives.rs"]
mod profile_string_alternatives;
#[path = "all/recommender_schemas.rs"]
mod recommender_schemas;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_domain.rs"]
mod update_domain;
#[path = "all/update_domain_not_found_envelope.rs"]
mod update_domain_not_found_envelope;
#[path = "all/update_profile.rs"]
mod update_profile;
#[path = "all/upload_job_results_summary.rs"]
mod upload_job_results_summary;
#[path = "all/upload_jobs.rs"]
mod upload_jobs;
