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
#[path = "all/asset_processing_configuration.rs"]
mod asset_processing_configuration;
#[path = "all/control_plane_reference_resolution.rs"]
mod control_plane_reference_resolution;
#[path = "all/cross_account_profile_authorization.rs"]
mod cross_account_profile_authorization;
#[path = "all/data_automation_invocations.rs"]
mod data_automation_invocations;
#[path = "all/input_image_classification.rs"]
mod input_image_classification;
#[path = "all/job_completion_events.rs"]
mod job_completion_events;
#[path = "all/job_event_region_scope.rs"]
mod job_event_region_scope;
#[path = "all/job_output_location_and_outcome.rs"]
mod job_output_location_and_outcome;
#[path = "all/job_standard_output_objects.rs"]
mod job_standard_output_objects;
#[path = "all/minted_arn_region_segment.rs"]
mod minted_arn_region_segment;
#[path = "all/minted_invocation_id_form.rs"]
mod minted_invocation_id_form;
#[path = "all/refusal_echoes_no_request_value.rs"]
mod refusal_echoes_no_request_value;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_unavailable_refusal.rs"]
mod store_unavailable_refusal;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_key_refusal.rs"]
mod tag_key_refusal;
#[path = "all/tag_key_refusal_element_path.rs"]
mod tag_key_refusal_element_path;
#[path = "all/tag_key_refusal_list_bound.rs"]
mod tag_key_refusal_list_bound;
#[path = "all/tag_key_unicode_class.rs"]
mod tag_key_unicode_class;
#[path = "all/unknown_invocation_refusal.rs"]
mod unknown_invocation_refusal;
