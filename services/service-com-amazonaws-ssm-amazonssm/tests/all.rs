// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//!
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.

// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/parameter_store.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/association_dispatch_options.rs"]
mod association_dispatch_options;
#[path = "all/association_listing.rs"]
mod association_listing;
#[path = "all/association_updates_and_deletes.rs"]
mod association_updates_and_deletes;
#[path = "all/automation_execution_reports.rs"]
mod automation_execution_reports;
#[path = "all/automation_executions.rs"]
mod automation_executions;
#[path = "all/automation_step_executions.rs"]
mod automation_step_executions;
#[path = "all/boundary_refusal_wording.rs"]
mod boundary_refusal_wording;
#[path = "all/document_content.rs"]
mod document_content;
#[path = "all/document_default_version.rs"]
mod document_default_version;
#[path = "all/document_identity_and_deletion.rs"]
mod document_identity_and_deletion;
#[path = "all/document_listing.rs"]
mod document_listing;
#[path = "all/document_versions.rs"]
mod document_versions;
#[path = "all/cloud_connectors.rs"]
mod cloud_connectors;
#[path = "all/command_history.rs"]
mod command_history;
#[path = "all/default_patch_baselines.rs"]
mod default_patch_baselines;
#[path = "all/documents.rs"]
mod documents;
#[path = "all/execution_previews.rs"]
mod execution_previews;
#[path = "all/hybrid_activations.rs"]
mod hybrid_activations;
#[path = "all/instance_properties.rs"]
mod instance_properties;
#[path = "all/just_in_time_node_access.rs"]
mod just_in_time_node_access;
#[path = "all/maintenance_windows.rs"]
mod maintenance_windows;
#[path = "all/managed_node_filter_validation.rs"]
mod managed_node_filter_validation;
#[path = "all/managed_node_information.rs"]
mod managed_node_information;
#[path = "all/node_inventory.rs"]
mod node_inventory;
#[path = "all/ops_item_deletion.rs"]
mod ops_item_deletion;
#[path = "all/ops_item_events.rs"]
mod ops_item_events;
#[path = "all/ops_item_queries.rs"]
mod ops_item_queries;
#[path = "all/ops_item_related_items.rs"]
mod ops_item_related_items;
#[path = "all/ops_item_updates.rs"]
mod ops_item_updates;
#[path = "all/ops_items.rs"]
mod ops_items;
#[path = "all/parameter_deletion.rs"]
mod parameter_deletion;
#[path = "all/parameter_deletion_visibility.rs"]
mod parameter_deletion_visibility;
#[path = "all/parameter_history.rs"]
mod parameter_history;
#[path = "all/parameter_labels.rs"]
mod parameter_labels;
#[path = "all/parameter_last_modified_user.rs"]
mod parameter_last_modified_user;
#[path = "all/parameter_metadata.rs"]
mod parameter_metadata;
#[path = "all/parameter_not_found_payload.rs"]
mod parameter_not_found_payload;
#[path = "all/parameter_read_payload_shape.rs"]
mod parameter_read_payload_shape;
#[path = "all/parameter_refusal_messages.rs"]
mod parameter_refusal_messages;
#[path = "all/parameter_store.rs"]
mod parameter_store;
#[path = "all/parameters_batch.rs"]
mod parameters_batch;
#[path = "all/parameters_by_path.rs"]
mod parameters_by_path;
#[path = "all/patch_baseline_approval_coverage.rs"]
mod patch_baseline_approval_coverage;
#[path = "all/patch_baseline_details.rs"]
mod patch_baseline_details;
#[path = "all/patch_baseline_listing.rs"]
mod patch_baseline_listing;
#[path = "all/patch_properties_catalog.rs"]
mod patch_properties_catalog;
#[path = "all/public_amazon_linux_alias_namespace.rs"]
mod public_amazon_linux_alias_namespace;
#[path = "all/public_image_metadata_hierarchies.rs"]
mod public_image_metadata_hierarchies;
#[path = "all/public_parameter_catalog.rs"]
mod public_parameter_catalog;
#[path = "all/published_documents.rs"]
mod published_documents;
#[path = "all/resource_data_syncs.rs"]
mod resource_data_syncs;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/run_command.rs"]
mod run_command;
#[path = "all/secretsmanager_reference.rs"]
mod secretsmanager_reference;
#[path = "all/secure_string_decryption.rs"]
mod secure_string_decryption;
#[path = "all/secure_string_encryption_at_rest.rs"]
mod secure_string_encryption_at_rest;
#[path = "all/secure_string_key_state.rs"]
mod secure_string_key_state;
#[path = "all/service_settings.rs"]
mod service_settings;
#[path = "all/state_manager_associations.rs"]
mod state_manager_associations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
