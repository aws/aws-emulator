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
#[path = "all/control_plane_error_semantics.rs"]
mod control_plane_error_semantics;
#[path = "all/data_transformation_job.rs"]
mod data_transformation_job;
#[path = "all/data_transformation_job_listing.rs"]
mod data_transformation_job_listing;
#[path = "all/data_transformation_profile.rs"]
mod data_transformation_profile;
#[path = "all/data_transformation_refusal_wording.rs"]
mod data_transformation_refusal_wording;
#[path = "all/datastore_action_gate.rs"]
mod datastore_action_gate;
#[path = "all/datastore_backup_restore_window.rs"]
mod datastore_backup_restore_window;
#[path = "all/datastore_encryption_and_preload.rs"]
mod datastore_encryption_and_preload;
#[path = "all/datastore_reaping.rs"]
mod datastore_reaping;
#[path = "all/datastore_state_conflict.rs"]
mod datastore_state_conflict;
#[path = "all/datastore_transition_conflict_sentence.rs"]
mod datastore_transition_conflict_sentence;
#[path = "all/datastore_update_not_active.rs"]
mod datastore_update_not_active;
#[path = "all/datastore_update_transition.rs"]
mod datastore_update_transition;
#[path = "all/delete_data_transformation_profile.rs"]
mod delete_data_transformation_profile;
#[path = "all/error_envelopes.rs"]
mod error_envelopes;
#[path = "all/fhir_datastore_lifecycle.rs"]
mod fhir_datastore_lifecycle;
#[path = "all/fhir_export_job_describe.rs"]
mod fhir_export_job_describe;
#[path = "all/fhir_export_job_listing.rs"]
mod fhir_export_job_listing;
#[path = "all/fhir_export_job_start.rs"]
mod fhir_export_job_start;
#[path = "all/fhir_import_job_describe.rs"]
mod fhir_import_job_describe;
#[path = "all/fhir_import_job_listing.rs"]
mod fhir_import_job_listing;
#[path = "all/fhir_import_job_start.rs"]
mod fhir_import_job_start;
#[path = "all/fhir_rest_and_validate_source.rs"]
mod fhir_rest_and_validate_source;
#[path = "all/fhir_search_interaction.rs"]
mod fhir_search_interaction;
#[path = "all/fhir_update_interaction.rs"]
mod fhir_update_interaction;
#[path = "all/idempotency_replay_precedes_state_gates.rs"]
mod idempotency_replay_precedes_state_gates;
#[path = "all/job_end_time_is_the_due_instant.rs"]
mod job_end_time_is_the_due_instant;
#[path = "all/list_data_transformation_profile_versions.rs"]
mod list_data_transformation_profile_versions;
#[path = "all/list_data_transformation_profiles.rs"]
mod list_data_transformation_profiles;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/profile_client_token_lifetime.rs"]
mod profile_client_token_lifetime;
#[path = "all/profile_retained_byte_ceilings.rs"]
mod profile_retained_byte_ceilings;
#[path = "all/profile_source_variants.rs"]
mod profile_source_variants;
#[path = "all/publish_data_transformation_profile.rs"]
mod publish_data_transformation_profile;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/rest_tagging_surface.rs"]
mod rest_tagging_surface;
#[path = "all/restore_fhir_datastore.rs"]
mod restore_fhir_datastore;
#[path = "all/seen_client_token_is_ignored.rs"]
mod seen_client_token_is_ignored;
#[path = "all/source_format_enum_refusal.rs"]
mod source_format_enum_refusal;
#[path = "all/start_data_transformation_job.rs"]
mod start_data_transformation_job;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_wide_byte_ceiling_holders.rs"]
mod store_wide_byte_ceiling_holders;
#[path = "all/tag_mutation_requires_a_live_resource.rs"]
mod tag_mutation_requires_a_live_resource;
#[path = "all/timestamp_cutoff_filters.rs"]
mod timestamp_cutoff_filters;
#[path = "all/update_data_transformation_profile.rs"]
mod update_data_transformation_profile;
#[path = "all/update_fhir_datastore.rs"]
mod update_fhir_datastore;
#[path = "all/update_profile_with_agent.rs"]
mod update_profile_with_agent;
