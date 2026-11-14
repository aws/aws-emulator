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
#[path = "all/arn_resolution_during_domain_deletion.rs"]
mod arn_resolution_during_domain_deletion;
#[path = "all/caller_sized_refusal_bound.rs"]
mod caller_sized_refusal_bound;
#[path = "all/client_refusal_is_modelled.rs"]
mod client_refusal_is_modelled;
#[path = "all/create_domain_idempotent_repeat.rs"]
mod create_domain_idempotent_repeat;
#[path = "all/create_domain_optional_member_presence.rs"]
mod create_domain_optional_member_presence;
#[path = "all/deleted_domain_stops_contained_transitions.rs"]
mod deleted_domain_stops_contained_transitions;
#[path = "all/domain_account_cap_and_lookup_semantics.rs"]
mod domain_account_cap_and_lookup_semantics;
#[path = "all/domain_liveness_agreement.rs"]
mod domain_liveness_agreement;
#[path = "all/domain_response_projection.rs"]
mod domain_response_projection;
#[path = "all/domain_subscription_lifecycle.rs"]
mod domain_subscription_lifecycle;
#[path = "all/domain_web_app_identity.rs"]
mod domain_web_app_identity;
#[path = "all/encryption_context_projection.rs"]
mod encryption_context_projection;
#[path = "all/error_detail_member.rs"]
mod error_detail_member;
#[path = "all/list_domains_account_scope.rs"]
mod list_domains_account_scope;
#[path = "all/list_domains_filter_cursor_scope.rs"]
mod list_domains_filter_cursor_scope;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/medical_scribe_listening_session_stream.rs"]
mod medical_scribe_listening_session_stream;
#[path = "all/pagination_token_issuance.rs"]
mod pagination_token_issuance;
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;
#[path = "all/patient_insights_job_lifecycle.rs"]
mod patient_insights_job_lifecycle;
#[path = "all/patient_insights_job_terminal_output.rs"]
mod patient_insights_job_terminal_output;
#[path = "all/patient_insights_output_uri_character_bound.rs"]
mod patient_insights_output_uri_character_bound;
#[path = "all/patient_insights_runtime_refusals.rs"]
mod patient_insights_runtime_refusals;
#[path = "all/recorded_domain_identity_projection.rs"]
mod recorded_domain_identity_projection;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/reserved_tag_key_prefix.rs"]
mod reserved_tag_key_prefix;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/shared_due_materialisation.rs"]
mod shared_due_materialisation;
#[path = "all/shared_pagination_policy.rs"]
mod shared_pagination_policy;
#[path = "all/start_patient_insights_job_request_identity.rs"]
mod start_patient_insights_job_request_identity;
#[path = "all/start_patient_insights_job_write_discipline.rs"]
mod start_patient_insights_job_write_discipline;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_unreachable_channel.rs"]
mod store_unreachable_channel;
#[path = "all/subscription_activation_stamp.rs"]
mod subscription_activation_stamp;
#[path = "all/subscription_response_projection.rs"]
mod subscription_response_projection;
#[path = "all/subscription_transition_step.rs"]
mod subscription_transition_step;
#[path = "all/tag_bound_precedence.rs"]
mod tag_bound_precedence;
#[path = "all/tag_bound_unit.rs"]
mod tag_bound_unit;
#[path = "all/tag_count_cap.rs"]
mod tag_count_cap;
#[path = "all/tag_entry_refusal_completeness.rs"]
mod tag_entry_refusal_completeness;
#[path = "all/tag_map_bounds.rs"]
mod tag_map_bounds;
