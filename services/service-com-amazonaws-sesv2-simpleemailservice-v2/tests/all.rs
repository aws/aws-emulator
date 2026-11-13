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
#[path = "all/account.rs"]
mod account;
#[path = "all/account_dedicated_ip_warmup_attributes.rs"]
mod account_dedicated_ip_warmup_attributes;
#[path = "all/account_details.rs"]
mod account_details;
#[path = "all/account_pricing_attributes.rs"]
mod account_pricing_attributes;
#[path = "all/account_sending_attributes.rs"]
mod account_sending_attributes;
#[path = "all/account_suppression_attributes.rs"]
mod account_suppression_attributes;
#[path = "all/account_vdm_attributes.rs"]
mod account_vdm_attributes;
#[path = "all/blacklist_reports.rs"]
mod blacklist_reports;
#[path = "all/boundary_validation_message.rs"]
mod boundary_validation_message;
#[path = "all/cloud_watch_dimension_source.rs"]
mod cloud_watch_dimension_source;
#[path = "all/cloud_watch_event_destination_delivery.rs"]
mod cloud_watch_event_destination_delivery;
#[path = "all/configuration_set_deletion.rs"]
mod configuration_set_deletion;
#[path = "all/configuration_set_event_destination_capacity.rs"]
mod configuration_set_event_destination_capacity;
#[path = "all/configuration_set_event_destination_ceiling.rs"]
mod configuration_set_event_destination_ceiling;
#[path = "all/configuration_set_event_destination_deletion.rs"]
mod configuration_set_event_destination_deletion;
#[path = "all/configuration_set_event_destinations.rs"]
mod configuration_set_event_destinations;
#[path = "all/configuration_set_event_publication_surface.rs"]
mod configuration_set_event_publication_surface;
#[path = "all/configuration_set_firehose_destination_role.rs"]
mod configuration_set_firehose_destination_role;
#[path = "all/configuration_set_firehose_role_refusal_is_repeatable.rs"]
mod configuration_set_firehose_role_refusal_is_repeatable;
#[path = "all/configuration_set_listing.rs"]
mod configuration_set_listing;
#[path = "all/configuration_set_message_security.rs"]
mod configuration_set_message_security;
#[path = "all/configuration_set_not_found_message.rs"]
mod configuration_set_not_found_message;
#[path = "all/configuration_set_option_writes.rs"]
mod configuration_set_option_writes;
#[path = "all/configuration_set_sending_options.rs"]
mod configuration_set_sending_options;
#[path = "all/configuration_set_sending_pause.rs"]
mod configuration_set_sending_pause;
#[path = "all/configuration_set_sns_destination_authorization.rs"]
mod configuration_set_sns_destination_authorization;
#[path = "all/configuration_set_sns_event_destinations.rs"]
mod configuration_set_sns_event_destinations;
#[path = "all/configuration_set_suppression_options.rs"]
mod configuration_set_suppression_options;
#[path = "all/configuration_sets.rs"]
mod configuration_sets;
#[path = "all/contact_lists.rs"]
mod contact_lists;
#[path = "all/contact_plane_wire_fidelity.rs"]
mod contact_plane_wire_fidelity;
#[path = "all/contact_updates.rs"]
mod contact_updates;
#[path = "all/dedicated_ip_in_pool.rs"]
mod dedicated_ip_in_pool;
#[path = "all/dedicated_ip_pool_deletion.rs"]
mod dedicated_ip_pool_deletion;
#[path = "all/dedicated_ip_pool_scaling.rs"]
mod dedicated_ip_pool_scaling;
#[path = "all/dedicated_ip_pools.rs"]
mod dedicated_ip_pools;
#[path = "all/dedicated_ip_warmup_attributes.rs"]
mod dedicated_ip_warmup_attributes;
#[path = "all/dedicated_ips.rs"]
mod dedicated_ips;
#[path = "all/email_address_insights.rs"]
mod email_address_insights;
#[path = "all/email_identity_certificates.rs"]
mod email_identity_certificates;
#[path = "all/email_identity_configuration_set_association.rs"]
mod email_identity_configuration_set_association;
#[path = "all/email_identity_cursor_refusal_wording.rs"]
mod email_identity_cursor_refusal_wording;
#[path = "all/email_identity_dkim_attributes.rs"]
mod email_identity_dkim_attributes;
#[path = "all/email_identity_dkim_inheritance.rs"]
mod email_identity_dkim_inheritance;
#[path = "all/email_identity_feedback_attributes.rs"]
mod email_identity_feedback_attributes;
#[path = "all/email_identity_lifecycle.rs"]
mod email_identity_lifecycle;
#[path = "all/email_identity_listing_route.rs"]
mod email_identity_listing_route;
#[path = "all/email_identity_mail_from_attributes.rs"]
mod email_identity_mail_from_attributes;
#[path = "all/email_identity_not_found_message.rs"]
mod email_identity_not_found_message;
#[path = "all/email_identity_policies.rs"]
mod email_identity_policies;
#[path = "all/email_identity_policies_read.rs"]
mod email_identity_policies_read;
#[path = "all/email_identity_verification_info.rs"]
mod email_identity_verification_info;
#[path = "all/email_template_listing.rs"]
mod email_template_listing;
#[path = "all/email_template_refusal_wording.rs"]
mod email_template_refusal_wording;
#[path = "all/email_template_render.rs"]
mod email_template_render;
#[path = "all/email_template_updates.rs"]
mod email_template_updates;
#[path = "all/email_templates.rs"]
mod email_templates;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/export_job_cancellation.rs"]
mod export_job_cancellation;
#[path = "all/export_job_creation.rs"]
mod export_job_creation;
#[path = "all/export_job_reads.rs"]
mod export_job_reads;
#[path = "all/export_jobs.rs"]
mod export_jobs;
#[path = "all/import_job_capacity.rs"]
mod import_job_capacity;
#[path = "all/import_jobs.rs"]
mod import_jobs;
#[path = "all/json_wire.rs"]
mod json_wire;
#[path = "all/list_tenants.rs"]
mod list_tenants;
#[path = "all/message_insights.rs"]
mod message_insights;
#[path = "all/metric_data.rs"]
mod metric_data;
#[path = "all/multi_region_endpoints.rs"]
mod multi_region_endpoints;
#[path = "all/page_cursor_refusal_wording.rs"]
mod page_cursor_refusal_wording;
#[path = "all/recommendations.rs"]
mod recommendations;
#[path = "all/reputation_entities.rs"]
mod reputation_entities;
#[path = "all/reputation_entity_policy.rs"]
mod reputation_entity_policy;
#[path = "all/request_claim.rs"]
mod request_claim;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/send_bulk_email.rs"]
mod send_bulk_email;
#[path = "all/send_list_management_options.rs"]
mod send_list_management_options;
#[path = "all/send_suppression_from_feedback.rs"]
mod send_suppression_from_feedback;
#[path = "all/send_unverified_identity_refusal.rs"]
mod send_unverified_identity_refusal;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/suppressed_destinations.rs"]
mod suppressed_destinations;
#[path = "all/suppression_list_scope_refusal.rs"]
mod suppression_list_scope_refusal;
#[path = "all/template_policy_and_tag_reads.rs"]
mod template_policy_and_tag_reads;
#[path = "all/tenant_deletion.rs"]
mod tenant_deletion;
#[path = "all/tenant_deletion_reputation_entity.rs"]
mod tenant_deletion_reputation_entity;
#[path = "all/tenant_duplicate_name_refusal.rs"]
mod tenant_duplicate_name_refusal;
#[path = "all/tenant_resource_association_removal.rs"]
mod tenant_resource_association_removal;
#[path = "all/tenant_resource_associations.rs"]
mod tenant_resource_associations;
#[path = "all/tenant_suppression_attributes.rs"]
mod tenant_suppression_attributes;
#[path = "all/tenants.rs"]
mod tenants;
