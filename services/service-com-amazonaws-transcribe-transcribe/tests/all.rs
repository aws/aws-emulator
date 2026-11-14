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
#[path = "all/absent_record_resolution.rs"]
mod absent_record_resolution;
#[path = "all/call_analytics_categories.rs"]
mod call_analytics_categories;
#[path = "all/call_analytics_category_listing.rs"]
mod call_analytics_category_listing;
#[path = "all/call_analytics_category_quota.rs"]
mod call_analytics_category_quota;
#[path = "all/call_analytics_category_refusals.rs"]
mod call_analytics_category_refusals;
#[path = "all/call_analytics_category_update_rule_validation.rs"]
mod call_analytics_category_update_rule_validation;
#[path = "all/call_analytics_category_updates.rs"]
mod call_analytics_category_updates;
#[path = "all/call_analytics_job_delete.rs"]
mod call_analytics_job_delete;
#[path = "all/call_analytics_job_queueing.rs"]
mod call_analytics_job_queueing;
#[path = "all/call_analytics_job_results.rs"]
mod call_analytics_job_results;
#[path = "all/call_analytics_job_start_limits.rs"]
mod call_analytics_job_start_limits;
#[path = "all/call_analytics_job_start_preconditions.rs"]
mod call_analytics_job_start_preconditions;
#[path = "all/call_analytics_job_tagging.rs"]
mod call_analytics_job_tagging;
#[path = "all/call_analytics_jobs.rs"]
mod call_analytics_jobs;
#[path = "all/call_analytics_rule_negate.rs"]
mod call_analytics_rule_negate;
#[path = "all/call_analytics_rule_time_ranges.rs"]
mod call_analytics_rule_time_ranges;
#[path = "all/channel_definition_distinctness.rs"]
mod channel_definition_distinctness;
#[path = "all/client_error_wire_status.rs"]
mod client_error_wire_status;
#[path = "all/clock_owed_record_access.rs"]
mod clock_owed_record_access;
#[path = "all/clock_read_after_the_cross_service_read.rs"]
mod clock_read_after_the_cross_service_read;
#[path = "all/delete_requires_the_entity.rs"]
mod delete_requires_the_entity;
#[path = "all/describe_language_model.rs"]
mod describe_language_model;
#[path = "all/get_call_analytics_category_reads.rs"]
mod get_call_analytics_category_reads;
#[path = "all/get_medical_vocabulary_reads.rs"]
mod get_medical_vocabulary_reads;
#[path = "all/job_concurrency_quota.rs"]
mod job_concurrency_quota;
#[path = "all/job_output_objects.rs"]
mod job_output_objects;
#[path = "all/job_output_withdrawal.rs"]
mod job_output_withdrawal;
#[path = "all/job_start_language_model_status.rs"]
mod job_start_language_model_status;
#[path = "all/job_start_vocabulary_state.rs"]
mod job_start_vocabulary_state;
#[path = "all/language_id_settings_filter_method.rs"]
mod language_id_settings_filter_method;
#[path = "all/language_model_delete_requires_the_model.rs"]
mod language_model_delete_requires_the_model;
#[path = "all/language_model_encryption_updates.rs"]
mod language_model_encryption_updates;
#[path = "all/language_model_listing.rs"]
mod language_model_listing;
#[path = "all/language_model_quota.rs"]
mod language_model_quota;
#[path = "all/language_model_training_data.rs"]
mod language_model_training_data;
#[path = "all/language_models.rs"]
mod language_models;
#[path = "all/list_cursor_name_rule.rs"]
mod list_cursor_name_rule;
#[path = "all/list_cursor_rule.rs"]
mod list_cursor_rule;
#[path = "all/list_cursor_scope_rule.rs"]
mod list_cursor_scope_rule;
#[path = "all/list_cursor_spelling_rule.rs"]
mod list_cursor_spelling_rule;
#[path = "all/list_paging_rule.rs"]
mod list_paging_rule;
#[path = "all/media_object_resolution.rs"]
mod media_object_resolution;
#[path = "all/medical_job_start_vocabulary_state.rs"]
mod medical_job_start_vocabulary_state;
#[path = "all/medical_scribe_clinical_note_template.rs"]
mod medical_scribe_clinical_note_template;
#[path = "all/medical_scribe_job_listing.rs"]
mod medical_scribe_job_listing;
#[path = "all/medical_scribe_jobs.rs"]
mod medical_scribe_jobs;
#[path = "all/medical_scribe_region_availability.rs"]
mod medical_scribe_region_availability;
#[path = "all/medical_scribe_settings_references.rs"]
mod medical_scribe_settings_references;
#[path = "all/medical_transcription_job_delete.rs"]
mod medical_transcription_job_delete;
#[path = "all/medical_transcription_job_listing.rs"]
mod medical_transcription_job_listing;
#[path = "all/medical_transcription_job_name_lifetime.rs"]
mod medical_transcription_job_name_lifetime;
#[path = "all/medical_transcription_jobs.rs"]
mod medical_transcription_jobs;
#[path = "all/medical_vocabularies.rs"]
mod medical_vocabularies;
#[path = "all/medical_vocabulary_constraints.rs"]
mod medical_vocabulary_constraints;
#[path = "all/medical_vocabulary_delete.rs"]
mod medical_vocabulary_delete;
#[path = "all/medical_vocabulary_updates.rs"]
mod medical_vocabulary_updates;
#[path = "all/queued_jobs_hold_no_concurrency_slot.rs"]
mod queued_jobs_hold_no_concurrency_slot;
#[path = "all/refusal_sentences.rs"]
mod refusal_sentences;
#[path = "all/regional_resource_scoping.rs"]
mod regional_resource_scoping;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/rpc_success_status.rs"]
mod rpc_success_status;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/settings_co_requirements.rs"]
mod settings_co_requirements;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_key_uniqueness.rs"]
mod tag_key_uniqueness;
#[path = "all/tag_set_dispatch.rs"]
mod tag_set_dispatch;
#[path = "all/tagging_arn_qualifiers.rs"]
mod tagging_arn_qualifiers;
#[path = "all/tagging_resource_families.rs"]
mod tagging_resource_families;
#[path = "all/transcription_job_absent_media_state.rs"]
mod transcription_job_absent_media_state;
#[path = "all/transcription_job_identified_language.rs"]
mod transcription_job_identified_language;
#[path = "all/transcription_job_language_identification.rs"]
mod transcription_job_language_identification;
#[path = "all/transcription_job_media_format.rs"]
mod transcription_job_media_format;
#[path = "all/transcription_job_output_encryption.rs"]
mod transcription_job_output_encryption;
#[path = "all/transcription_job_queueing.rs"]
mod transcription_job_queueing;
#[path = "all/transcription_job_redacted_output.rs"]
mod transcription_job_redacted_output;
#[path = "all/transcription_job_request_rules.rs"]
mod transcription_job_request_rules;
#[path = "all/transcription_job_single_lookup.rs"]
mod transcription_job_single_lookup;
#[path = "all/transcription_jobs.rs"]
mod transcription_jobs;
#[path = "all/vocabularies.rs"]
mod vocabularies;
#[path = "all/vocabulary_delete_and_tag_sets.rs"]
mod vocabulary_delete_and_tag_sets;
#[path = "all/vocabulary_download_content.rs"]
mod vocabulary_download_content;
#[path = "all/vocabulary_download_isolation.rs"]
mod vocabulary_download_isolation;
#[path = "all/vocabulary_download_uri_state.rs"]
mod vocabulary_download_uri_state;
#[path = "all/vocabulary_file_names_an_object.rs"]
mod vocabulary_file_names_an_object;
#[path = "all/vocabulary_file_objects.rs"]
mod vocabulary_file_objects;
#[path = "all/vocabulary_file_uri.rs"]
mod vocabulary_file_uri;
#[path = "all/vocabulary_filter_delete_requires_the_filter.rs"]
mod vocabulary_filter_delete_requires_the_filter;
#[path = "all/vocabulary_filter_file_uri.rs"]
mod vocabulary_filter_file_uri;
#[path = "all/vocabulary_update_term_source.rs"]
mod vocabulary_update_term_source;
#[path = "all/vocabulary_update_terminal_state.rs"]
mod vocabulary_update_terminal_state;
