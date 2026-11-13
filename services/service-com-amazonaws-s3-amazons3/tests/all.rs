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
#[path = "all/access_point_data_plane.rs"]
mod access_point_data_plane;
#[path = "all/account_public_access_block.rs"]
mod account_public_access_block;
#[path = "all/anonymous_object_reads.rs"]
mod anonymous_object_reads;
#[path = "all/announced_response_urls.rs"]
mod announced_response_urls;
#[path = "all/aws_published_sample_data.rs"]
mod aws_published_sample_data;
#[path = "all/bucket_abac.rs"]
mod bucket_abac;
#[path = "all/bucket_accelerate_configuration.rs"]
mod bucket_accelerate_configuration;
#[path = "all/bucket_acl.rs"]
mod bucket_acl;
#[path = "all/bucket_analytics_configuration.rs"]
mod bucket_analytics_configuration;
#[path = "all/bucket_analytics_configuration_listing.rs"]
mod bucket_analytics_configuration_listing;
#[path = "all/bucket_configuration_id_addressing.rs"]
mod bucket_configuration_id_addressing;
#[path = "all/bucket_configuration_listing_result_element.rs"]
mod bucket_configuration_listing_result_element;
#[path = "all/bucket_cors.rs"]
mod bucket_cors;
#[path = "all/bucket_cors_delete.rs"]
mod bucket_cors_delete;
#[path = "all/bucket_creation_date_views.rs"]
mod bucket_creation_date_views;
#[path = "all/bucket_create_list_and_tagging_contracts.rs"]
mod bucket_create_list_and_tagging_contracts;
#[path = "all/bucket_deletion_emptiness.rs"]
mod bucket_deletion_emptiness;
#[path = "all/bucket_encryption.rs"]
mod bucket_encryption;
#[path = "all/bucket_encryption_delete.rs"]
mod bucket_encryption_delete;
#[path = "all/bucket_error_envelope.rs"]
mod bucket_error_envelope;
#[path = "all/bucket_intelligent_tiering.rs"]
mod bucket_intelligent_tiering;
#[path = "all/bucket_inventory_configuration.rs"]
mod bucket_inventory_configuration;
#[path = "all/bucket_inventory_configuration_deletion.rs"]
mod bucket_inventory_configuration_deletion;
#[path = "all/bucket_inventory_configuration_listing.rs"]
mod bucket_inventory_configuration_listing;
#[path = "all/bucket_lifecycle.rs"]
mod bucket_lifecycle;
#[path = "all/bucket_lifecycle_configuration.rs"]
mod bucket_lifecycle_configuration;
#[path = "all/bucket_location.rs"]
mod bucket_location;
#[path = "all/bucket_logging.rs"]
mod bucket_logging;
#[path = "all/bucket_metadata_annotation_role_permissions.rs"]
mod bucket_metadata_annotation_role_permissions;
#[path = "all/bucket_metadata_annotation_table_update.rs"]
mod bucket_metadata_annotation_table_update;
#[path = "all/bucket_metadata_annotation_table_update_sequences.rs"]
mod bucket_metadata_annotation_table_update_sequences;
#[path = "all/bucket_metadata_configuration.rs"]
mod bucket_metadata_configuration;
#[path = "all/bucket_metadata_configuration_delete_scope.rs"]
mod bucket_metadata_configuration_delete_scope;
#[path = "all/bucket_metadata_configuration_deletion.rs"]
mod bucket_metadata_configuration_deletion;
#[path = "all/bucket_metadata_inventory_table_namespace.rs"]
mod bucket_metadata_inventory_table_namespace;
#[path = "all/bucket_metadata_inventory_table_update.rs"]
mod bucket_metadata_inventory_table_update;
#[path = "all/bucket_metadata_journal_table_update.rs"]
mod bucket_metadata_journal_table_update;
#[path = "all/bucket_metadata_namespace_retention.rs"]
mod bucket_metadata_namespace_retention;
#[path = "all/bucket_metadata_unprovisioned_table_release.rs"]
mod bucket_metadata_unprovisioned_table_release;
#[path = "all/bucket_metrics_configuration.rs"]
mod bucket_metrics_configuration;
#[path = "all/bucket_metrics_configuration_documents.rs"]
mod bucket_metrics_configuration_documents;
#[path = "all/bucket_metrics_configuration_listing.rs"]
mod bucket_metrics_configuration_listing;
#[path = "all/bucket_name_namespace.rs"]
mod bucket_name_namespace;
#[path = "all/bucket_notification_configuration.rs"]
mod bucket_notification_configuration;
#[path = "all/bucket_object_lock_configuration.rs"]
mod bucket_object_lock_configuration;
#[path = "all/bucket_ownership_controls.rs"]
mod bucket_ownership_controls;
#[path = "all/bucket_policy_and_tagging.rs"]
mod bucket_policy_and_tagging;
#[path = "all/bucket_policy_delete.rs"]
mod bucket_policy_delete;
#[path = "all/bucket_policy_put_status.rs"]
mod bucket_policy_put_status;
#[path = "all/bucket_policy_status.rs"]
mod bucket_policy_status;
#[path = "all/bucket_public_access_block.rs"]
mod bucket_public_access_block;
#[path = "all/bucket_replication.rs"]
mod bucket_replication;
#[path = "all/bucket_replication_delete.rs"]
mod bucket_replication_delete;
#[path = "all/bucket_request_payment.rs"]
mod bucket_request_payment;
#[path = "all/bucket_subresource_absent_messages.rs"]
mod bucket_subresource_absent_messages;
#[path = "all/bucket_subresource_document_rules.rs"]
mod bucket_subresource_document_rules;
#[path = "all/bucket_tagging.rs"]
mod bucket_tagging;
#[path = "all/bucket_tagging_put_status.rs"]
mod bucket_tagging_put_status;
#[path = "all/bucket_versioning.rs"]
mod bucket_versioning;
#[path = "all/bucket_website.rs"]
mod bucket_website;
#[path = "all/bucket_website_delete.rs"]
mod bucket_website_delete;
#[path = "all/bucket_website_refusals.rs"]
mod bucket_website_refusals;
#[path = "all/checksum_mode_header_case.rs"]
mod checksum_mode_header_case;
#[path = "all/cli_download_legs.rs"]
mod cli_download_legs;
#[path = "all/configuration_subresource_put_status.rs"]
mod configuration_subresource_put_status;
#[path = "all/copy_source_header_envelopes.rs"]
mod copy_source_header_envelopes;
#[path = "all/copy_source_url_encoding.rs"]
mod copy_source_url_encoding;
#[path = "all/credential_scoped_authorization.rs"]
mod credential_scoped_authorization;
#[path = "all/default_object_checksum.rs"]
mod default_object_checksum;
#[path = "all/directory_bucket_conditional_deletes.rs"]
mod directory_bucket_conditional_deletes;
#[path = "all/directory_bucket_listings.rs"]
mod directory_bucket_listings;
#[path = "all/directory_bucket_renames.rs"]
mod directory_bucket_renames;
#[path = "all/directory_bucket_session_constraints.rs"]
mod directory_bucket_session_constraints;
#[path = "all/delete_marker_refusal_headers.rs"]
mod delete_marker_refusal_headers;
#[path = "all/directory_bucket_sessions_and_appends.rs"]
mod directory_bucket_sessions_and_appends;
#[path = "all/error_message_envelope.rs"]
mod error_message_envelope;
// Deliberately empty: this fix's own table for the error-message rule was folded into
// `error_message_envelope`, which spans the same rule from the model's own catalog.
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/express_session_tokens.rs"]
mod express_session_tokens;
#[path = "all/general_purpose_bucket_names.rs"]
mod general_purpose_bucket_names;
#[path = "all/global_condition_keys.rs"]
mod global_condition_keys;
#[path = "all/head_object_read_back.rs"]
mod head_object_read_back;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/inline_tagging_header_grammar.rs"]
mod inline_tagging_header_grammar;
#[path = "all/json_payload_media_type.rs"]
mod json_payload_media_type;
#[path = "all/kms_key_references.rs"]
mod kms_key_references;
#[path = "all/list_buckets.rs"]
mod list_buckets;
#[path = "all/listing_echoed_members.rs"]
mod listing_echoed_members;
#[path = "all/listing_encoding_type.rs"]
mod listing_encoding_type;
#[path = "all/lifecycle_transition_rules.rs"]
mod lifecycle_transition_rules;
#[path = "all/listing_optional_members.rs"]
mod listing_optional_members;
#[path = "all/listing_query_value_decoding.rs"]
mod listing_query_value_decoding;
#[path = "all/listing_resume_markers.rs"]
mod listing_resume_markers;
#[path = "all/metadata_table_update_journal_status.rs"]
mod metadata_table_update_journal_status;
#[path = "all/multipart_checksum_type.rs"]
mod multipart_checksum_type;
#[path = "all/multipart_part_checksums.rs"]
mod multipart_part_checksums;
#[path = "all/multipart_part_copy.rs"]
mod multipart_part_copy;
#[path = "all/multipart_part_copy_reported_members.rs"]
mod multipart_part_copy_reported_members;
#[path = "all/multipart_part_upload.rs"]
mod multipart_part_upload;
#[path = "all/multipart_upload_abort.rs"]
mod multipart_upload_abort;
#[path = "all/multipart_upload_completion.rs"]
mod multipart_upload_completion;
#[path = "all/multipart_upload_event_notifications.rs"]
mod multipart_upload_event_notifications;
#[path = "all/multipart_upload_initiation.rs"]
mod multipart_upload_initiation;
#[path = "all/multipart_upload_initiation_order.rs"]
mod multipart_upload_initiation_order;
#[path = "all/multipart_upload_listing.rs"]
mod multipart_upload_listing;
#[path = "all/multipart_upload_resume_position.rs"]
mod multipart_upload_resume_position;
#[path = "all/not_modified_version_headers.rs"]
mod not_modified_version_headers;
#[path = "all/notification_destination_validation.rs"]
mod notification_destination_validation;
#[path = "all/notification_filter_delivery.rs"]
mod notification_filter_delivery;
#[path = "all/notification_wildcard_event_overlap.rs"]
mod notification_wildcard_event_overlap;
#[path = "all/object_acl.rs"]
mod object_acl;
#[path = "all/object_and_policy.rs"]
mod object_and_policy;
#[path = "all/object_and_tagging_roundtrip.rs"]
mod object_and_tagging_roundtrip;
#[path = "all/object_annotation_listing.rs"]
mod object_annotation_listing;
#[path = "all/object_annotation_removal.rs"]
mod object_annotation_removal;
#[path = "all/object_annotations.rs"]
mod object_annotations;
#[path = "all/object_attributes.rs"]
mod object_attributes;
#[path = "all/object_batch_deletion.rs"]
mod object_batch_deletion;
#[path = "all/object_copy.rs"]
mod object_copy;
#[path = "all/object_content_type.rs"]
mod object_content_type;
#[path = "all/object_conditional_deletes.rs"]
mod object_conditional_deletes;
#[path = "all/object_conditional_reads.rs"]
mod object_conditional_reads;
#[path = "all/object_conditional_writes.rs"]
mod object_conditional_writes;
#[path = "all/object_customer_key_encryption.rs"]
mod object_customer_key_encryption;
#[path = "all/object_default_encryption.rs"]
mod object_default_encryption;
#[path = "all/object_encryption.rs"]
mod object_encryption;
#[path = "all/object_encryption_update.rs"]
mod object_encryption_update;
#[path = "all/object_event_notifications.rs"]
mod object_event_notifications;
#[path = "all/object_event_sequencer_order.rs"]
mod object_event_sequencer_order;
#[path = "all/object_key_label_verbatim.rs"]
mod object_key_label_verbatim;
#[path = "all/object_legal_hold.rs"]
mod object_legal_hold;
#[path = "all/object_listing_and_deletion.rs"]
mod object_listing_and_deletion;
#[path = "all/object_listing_encoding.rs"]
mod object_listing_encoding;
#[path = "all/object_listing_v1.rs"]
mod object_listing_v1;
#[path = "all/object_lock_enforcement.rs"]
mod object_lock_enforcement;
#[path = "all/object_lock_on_write.rs"]
mod object_lock_on_write;
#[path = "all/object_partial_reads.rs"]
mod object_partial_reads;
#[path = "all/object_plane_byte_budget.rs"]
mod object_plane_byte_budget;
#[path = "all/object_read_authorization.rs"]
mod object_read_authorization;
#[path = "all/object_read_request_headers.rs"]
mod object_read_request_headers;
#[path = "all/object_read_policy_union.rs"]
mod object_read_policy_union;
#[path = "all/object_replication.rs"]
mod object_replication;
#[path = "all/object_retention.rs"]
mod object_retention;
#[path = "all/object_restore.rs"]
mod object_restore;
#[path = "all/object_scoped_refusal_envelope.rs"]
mod object_scoped_refusal_envelope;
#[path = "all/object_select_content.rs"]
mod object_select_content;
#[path = "all/object_storage.rs"]
mod object_storage;
#[path = "all/object_subresource_version_id.rs"]
mod object_subresource_version_id;
#[path = "all/object_tag_set_removal.rs"]
mod object_tag_set_removal;
#[path = "all/object_tagging.rs"]
mod object_tagging;
#[path = "all/object_tagging_event_sub_types.rs"]
mod object_tagging_event_sub_types;
#[path = "all/object_tagging_on_put.rs"]
mod object_tagging_on_put;
#[path = "all/object_torrent.rs"]
mod object_torrent;
#[path = "all/object_versioning.rs"]
mod object_versioning;
#[path = "all/object_write_and_read_back.rs"]
mod object_write_and_read_back;
#[path = "all/object_write_authorization.rs"]
mod object_write_authorization;
#[path = "all/object_write_refusals.rs"]
mod object_write_refusals;
#[path = "all/object_write_tagging_header.rs"]
mod object_write_tagging_header;
#[path = "all/objects.rs"]
mod objects;
#[path = "all/partial_read_checksums.rs"]
mod partial_read_checksums;
#[path = "all/post_object_form_upload.rs"]
mod post_object_form_upload;
#[path = "all/presigned_object_reads.rs"]
mod presigned_object_reads;
#[path = "all/presigned_query_authentication.rs"]
mod presigned_query_authentication;
#[path = "all/presigned_url_serving.rs"]
mod presigned_url_serving;
#[path = "all/published_refusal_wording.rs"]
mod published_refusal_wording;
#[path = "all/public_access_block_default.rs"]
mod public_access_block_default;
#[path = "all/public_access_guardrails.rs"]
mod public_access_guardrails;
#[path = "all/refusal_class_fidelity.rs"]
mod refusal_class_fidelity;
#[path = "all/refusal_codes.rs"]
mod refusal_codes;
#[path = "all/refusal_detail_members.rs"]
mod refusal_detail_members;
#[path = "all/request_integrity_headers.rs"]
mod request_integrity_headers;
#[path = "all/request_latency_metrics.rs"]
mod request_latency_metrics;
#[path = "all/request_metrics.rs"]
mod request_metrics;
#[path = "all/request_time_skew.rs"]
mod request_time_skew;
#[path = "all/refusal_messages_published.rs"]
mod refusal_messages_published;
#[path = "all/refusal_resource_naming.rs"]
mod refusal_resource_naming;
#[path = "all/refused_argument_envelope.rs"]
mod refused_argument_envelope;
#[path = "all/refused_change_publishes_no_event.rs"]
mod refused_change_publishes_no_event;
#[path = "all/rename_object_query_parameter.rs"]
mod rename_object_query_parameter;
#[path = "all/replication_role_trust.rs"]
mod replication_role_trust;
#[path = "all/reserved_tag_key_prefix.rs"]
mod reserved_tag_key_prefix;
#[path = "all/role_session_object_authorization.rs"]
mod role_session_object_authorization;
#[path = "all/scratch_probe.rs"]
mod scratch_probe;
#[path = "all/served_envelope_values.rs"]
mod served_envelope_values;
#[path = "all/service_principal_object_access.rs"]
mod service_principal_object_access;
#[path = "all/service_set_tag_keys.rs"]
mod service_set_tag_keys;
#[path = "all/session_scoped_object_authorization.rs"]
mod session_scoped_object_authorization;
#[path = "all/signature_refusal_wording.rs"]
mod signature_refusal_wording;
#[path = "all/sse_kms_data_key_authorization.rs"]
mod sse_kms_data_key_authorization;
#[path = "all/sse_kms_decrypt_authorization.rs"]
mod sse_kms_decrypt_authorization;
#[path = "all/sse_kms_key_identity.rs"]
mod sse_kms_key_identity;
#[path = "all/sse_kms_relay_error_class.rs"]
mod sse_kms_relay_error_class;
#[path = "all/sse_kms_write_key_resolution.rs"]
mod sse_kms_write_key_resolution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stored_payload_checksums.rs"]
mod stored_payload_checksums;
#[path = "all/version_id_classification.rs"]
mod version_id_classification;
#[path = "all/version_listing_echoed_members.rs"]
mod version_listing_echoed_members;
#[path = "all/version_listing_owner.rs"]
mod version_listing_owner;
#[path = "all/version_listing_resume_markers.rs"]
mod version_listing_resume_markers;
#[path = "all/versioned_object_tag_sets.rs"]
mod versioned_object_tag_sets;
#[path = "all/versioned_removal_event_split.rs"]
mod versioned_removal_event_split;
#[path = "all/website_redirect_location.rs"]
mod website_redirect_location;
#[path = "all/website_endpoint.rs"]
mod website_endpoint;
#[path = "all/write_checksum_header_count.rs"]
mod write_checksum_header_count;
#[path = "all/write_response_identity.rs"]
mod write_response_identity;
#[path = "all/zz_host_style_probe.rs"]
mod zz_host_style_probe;
#[path = "all/zz_probe.rs"]
mod zz_probe;
#[path = "all/zzz_batch_probe.rs"]
mod zzz_batch_probe;
#[path = "all/zzz_marker_probe.rs"]
mod zzz_marker_probe;

