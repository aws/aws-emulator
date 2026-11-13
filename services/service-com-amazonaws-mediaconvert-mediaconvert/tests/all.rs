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
#[path = "all/acceleration_status.rs"]
mod acceleration_status;
#[path = "all/account_endpoints.rs"]
mod account_endpoints;
#[path = "all/blank_category_filter.rs"]
mod blank_category_filter;
#[path = "all/blank_delete_name.rs"]
mod blank_delete_name;
#[path = "all/blank_filter_value.rs"]
mod blank_filter_value;
#[path = "all/blank_job_reference.rs"]
mod blank_job_reference;
#[path = "all/blank_jobs_query_id.rs"]
mod blank_jobs_query_id;
#[path = "all/blank_output_preset.rs"]
mod blank_output_preset;
#[path = "all/blank_read_name.rs"]
mod blank_read_name;
#[path = "all/blank_resource_name.rs"]
mod blank_resource_name;
#[path = "all/caller_sent_key_identity.rs"]
mod caller_sent_key_identity;
#[path = "all/cancel_job_idempotence.rs"]
mod cancel_job_idempotence;
#[path = "all/certificate_association.rs"]
mod certificate_association;
#[path = "all/clock_sweep_scope.rs"]
mod clock_sweep_scope;
#[path = "all/create_job_validation.rs"]
mod create_job_validation;
#[path = "all/created_resource_defaults.rs"]
mod created_resource_defaults;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
#[path = "all/foreign_arn_reference.rs"]
mod foreign_arn_reference;
#[path = "all/hop_destination_refusal_order.rs"]
mod hop_destination_refusal_order;
#[path = "all/idempotent_repeats.rs"]
mod idempotent_repeats;
#[path = "all/input_probe.rs"]
mod input_probe;
#[path = "all/job_clock_consistency.rs"]
mod job_clock_consistency;
#[path = "all/job_engine_version_resolution.rs"]
mod job_engine_version_resolution;
#[path = "all/job_engine_versions.rs"]
mod job_engine_versions;
#[path = "all/job_from_template.rs"]
mod job_from_template;
#[path = "all/job_lifecycle.rs"]
mod job_lifecycle;
#[path = "all/job_search.rs"]
mod job_search;
#[path = "all/job_state_change_events.rs"]
mod job_state_change_events;
#[path = "all/job_template_crud.rs"]
mod job_template_crud;
#[path = "all/job_template_queue_reference.rs"]
mod job_template_queue_reference;
#[path = "all/job_template_settings_validation.rs"]
mod job_template_settings_validation;
#[path = "all/job_templates.rs"]
mod job_templates;
#[path = "all/jobs_query.rs"]
mod jobs_query;
#[path = "all/jobs_query_output_codecs.rs"]
mod jobs_query_output_codecs;
#[path = "all/list_jobs_queue_filter.rs"]
mod list_jobs_queue_filter;
#[path = "all/local_refusals_cost_no_input_read.rs"]
mod local_refusals_cost_no_input_read;
#[path = "all/next_token_spelling.rs"]
mod next_token_spelling;
#[path = "all/optional_free_text.rs"]
mod optional_free_text;
#[path = "all/output_preset_reference.rs"]
mod output_preset_reference;
#[path = "all/policy_enforcement.rs"]
mod policy_enforcement;
#[path = "all/policy_lifecycle.rs"]
mod policy_lifecycle;
#[path = "all/preset_deletion.rs"]
mod preset_deletion;
#[path = "all/preset_lifecycle.rs"]
mod preset_lifecycle;
#[path = "all/preset_listing.rs"]
mod preset_listing;
#[path = "all/preset_settings_validation.rs"]
mod preset_settings_validation;
#[path = "all/preset_update.rs"]
mod preset_update;
#[path = "all/preset_update_container_integrity.rs"]
mod preset_update_container_integrity;
#[path = "all/queue_capacity_validation.rs"]
mod queue_capacity_validation;
#[path = "all/queue_concurrency_pool.rs"]
mod queue_concurrency_pool;
#[path = "all/queue_deletion.rs"]
mod queue_deletion;
#[path = "all/queue_hopping.rs"]
mod queue_hopping;
#[path = "all/queue_listing.rs"]
mod queue_listing;
#[path = "all/queue_name_identity.rs"]
mod queue_name_identity;
#[path = "all/queue_pause_gating.rs"]
mod queue_pause_gating;
#[path = "all/queue_reference_projection.rs"]
mod queue_reference_projection;
#[path = "all/queue_reservation_term.rs"]
mod queue_reservation_term;
#[path = "all/queue_reserved_concurrency.rs"]
mod queue_reserved_concurrency;
#[path = "all/queue_reserved_concurrency_member.rs"]
mod queue_reserved_concurrency_member;
#[path = "all/queue_update.rs"]
mod queue_update;
#[path = "all/resource_projection_defaults.rs"]
mod resource_projection_defaults;
#[path = "all/resource_share.rs"]
mod resource_share;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/shared_job_filters.rs"]
mod shared_job_filters;
#[path = "all/shared_next_token_refusal.rs"]
mod shared_next_token_refusal;
#[path = "all/shared_not_found_refusal.rs"]
mod shared_not_found_refusal;
#[path = "all/shared_paging.rs"]
mod shared_paging;
#[path = "all/shared_set_selection.rs"]
mod shared_set_selection;
#[path = "all/shared_tag_refusal.rs"]
mod shared_tag_refusal;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/system_job_templates.rs"]
mod system_job_templates;
#[path = "all/system_presets.rs"]
mod system_presets;
#[path = "all/tag_key_identity.rs"]
mod tag_key_identity;
