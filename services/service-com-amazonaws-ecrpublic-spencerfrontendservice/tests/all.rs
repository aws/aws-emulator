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
#[path = "all/batch_delete_image_failure_order.rs"]
mod batch_delete_image_failure_order;
#[path = "all/client_error_message_templates.rs"]
mod client_error_message_templates;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/image_tag_details.rs"]
mod image_tag_details;
#[path = "all/invalid_parameter_messages.rs"]
mod invalid_parameter_messages;
#[path = "all/invalid_parameter_template.rs"]
mod invalid_parameter_template;
#[path = "all/json_document_members.rs"]
mod json_document_members;
#[path = "all/layer_part_uploads.rs"]
mod layer_part_uploads;
#[path = "all/layer_upload_completion.rs"]
mod layer_upload_completion;
#[path = "all/layer_upload_convergence.rs"]
mod layer_upload_convergence;
#[path = "all/layer_upload_refusal_messages.rs"]
mod layer_upload_refusal_messages;
#[path = "all/layer_upload_sessions.rs"]
mod layer_upload_sessions;
#[path = "all/pagination_filter_exclusion.rs"]
mod pagination_filter_exclusion;
#[path = "all/pagination_token_refusal.rs"]
mod pagination_token_refusal;
#[path = "all/put_image_layer_resolution.rs"]
mod put_image_layer_resolution;
#[path = "all/region_gate.rs"]
mod region_gate;
#[path = "all/registry_catalog_data.rs"]
mod registry_catalog_data;
#[path = "all/registry_catalog_write_published_regions.rs"]
mod registry_catalog_write_published_regions;
#[path = "all/registry_catalog_write_regions.rs"]
mod registry_catalog_write_regions;
#[path = "all/registry_catalog_write_wrong_region_is_inert.rs"]
mod registry_catalog_write_wrong_region_is_inert;
#[path = "all/registry_describe.rs"]
mod registry_describe;
#[path = "all/repository_catalog_data.rs"]
mod repository_catalog_data;
#[path = "all/repository_image_lifecycle.rs"]
mod repository_image_lifecycle;
#[path = "all/repository_logo_asset.rs"]
mod repository_logo_asset;
#[path = "all/repository_policy.rs"]
mod repository_policy;
#[path = "all/repository_policy_and_catalog_data.rs"]
mod repository_policy_and_catalog_data;
#[path = "all/repository_policy_canonical_form.rs"]
mod repository_policy_canonical_form;
#[path = "all/repository_policy_condition_values.rs"]
mod repository_policy_condition_values;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_field_fidelity.rs"]
mod response_field_fidelity;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/untag_resource.rs"]
mod untag_resource;
// `authorization_token` is declared AFTER the sorted list rather than in its
// alphabetical place at the top, for the reason `service-com-amazonaws-transfer-
// transferservice` records for its own out-of-place suite: every suite this crate gains
// is inserted into the sorted list, so the first lines of that list are exactly what a
// concurrent change also edits, and this declaration kept landing on top of one. Fold it
// into the list once the churn settles.
#[path = "all/authorization_token.rs"]
mod authorization_token;
