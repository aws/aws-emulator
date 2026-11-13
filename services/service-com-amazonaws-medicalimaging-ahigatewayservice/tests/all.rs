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
#[path = "all/content_reads_shared_rules.rs"]
mod content_reads_shared_rules;
#[path = "all/content_type_reporting.rs"]
mod content_type_reporting;
#[path = "all/datastore_delete_refusal_texts.rs"]
mod datastore_delete_refusal_texts;
#[path = "all/datastore_deleted_window.rs"]
mod datastore_deleted_window;
#[path = "all/datastore_lifecycle.rs"]
mod datastore_lifecycle;
#[path = "all/datastore_listing_page_tokens.rs"]
mod datastore_listing_page_tokens;
#[path = "all/datastore_status_guard.rs"]
mod datastore_status_guard;
#[path = "all/dicom_part10_reading.rs"]
mod dicom_part10_reading;
#[path = "all/error_message_texts.rs"]
mod error_message_texts;
#[path = "all/image_frames.rs"]
mod image_frames;
#[path = "all/image_set_access_time.rs"]
mod image_set_access_time;
#[path = "all/image_set_capacity.rs"]
mod image_set_capacity;
#[path = "all/image_set_copy.rs"]
mod image_set_copy;
#[path = "all/image_set_copy_into_held_content.rs"]
mod image_set_copy_into_held_content;
#[path = "all/image_set_copy_subsets.rs"]
mod image_set_copy_subsets;
#[path = "all/image_set_copy_tombstones.rs"]
mod image_set_copy_tombstones;
#[path = "all/image_set_deletion.rs"]
mod image_set_deletion;
#[path = "all/image_set_lock_visibility.rs"]
mod image_set_lock_visibility;
#[path = "all/image_set_metadata.rs"]
mod image_set_metadata;
#[path = "all/image_set_metadata_updates.rs"]
mod image_set_metadata_updates;
#[path = "all/image_set_metadata_versions.rs"]
mod image_set_metadata_versions;
#[path = "all/image_set_promotion.rs"]
mod image_set_promotion;
#[path = "all/image_set_properties.rs"]
mod image_set_properties;
#[path = "all/image_set_search.rs"]
mod image_set_search;
#[path = "all/image_set_series_attributes.rs"]
mod image_set_series_attributes;
#[path = "all/image_set_tombstone_content.rs"]
mod image_set_tombstone_content;
#[path = "all/image_set_version_listing.rs"]
mod image_set_version_listing;
#[path = "all/import_job_datastore_conflicts.rs"]
mod import_job_datastore_conflicts;
#[path = "all/import_job_idempotent_replay.rs"]
mod import_job_idempotent_replay;
#[path = "all/import_job_image_set_delivery.rs"]
mod import_job_image_set_delivery;
#[path = "all/import_job_listing.rs"]
mod import_job_listing;
#[path = "all/import_job_output_manifest.rs"]
mod import_job_output_manifest;
#[path = "all/list_element_refusal_paths.rs"]
mod list_element_refusal_paths;
#[path = "all/listing_filter_page_tokens.rs"]
mod listing_filter_page_tokens;
#[path = "all/listing_page_size_rule.rs"]
mod listing_page_size_rule;
#[path = "all/metadata_document_shape.rs"]
mod metadata_document_shape;
#[path = "all/metadata_document_structure.rs"]
mod metadata_document_structure;
#[path = "all/metadata_merge_depth.rs"]
mod metadata_merge_depth;
#[path = "all/modelled_attribute_constraints.rs"]
mod modelled_attribute_constraints;
#[path = "all/page_token_payload_forgery.rs"]
mod page_token_payload_forgery;
#[path = "all/regional_isolation.rs"]
mod regional_isolation;
#[path = "all/relational_refusal_values.rs"]
mod relational_refusal_values;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/search_sort_key_page_token.rs"]
mod search_sort_key_page_token;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/study_date_range_bounds.rs"]
mod study_date_range_bounds;
#[path = "all/study_update_membership.rs"]
mod study_update_membership;
#[path = "all/tag_map_rule_order.rs"]
mod tag_map_rule_order;
#[path = "all/tag_map_validation.rs"]
mod tag_map_validation;
#[path = "all/tag_resource_key_pattern.rs"]
mod tag_resource_key_pattern;
#[path = "all/tagging_not_found_text.rs"]
mod tagging_not_found_text;
#[path = "all/tagging_write_state_guard.rs"]
mod tagging_write_state_guard;
#[path = "all/untag_key_validation.rs"]
mod untag_key_validation;
