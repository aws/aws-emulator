// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/account_audit_configuration.rs"]
mod account_audit_configuration;
#[path = "all/authorization_decision.rs"]
mod authorization_decision;
#[path = "all/authorizer_registry.rs"]
mod authorizer_registry;
#[path = "all/authorizer_teardown.rs"]
mod authorizer_teardown;
#[path = "all/bulk_thing_registration.rs"]
mod bulk_thing_registration;
#[path = "all/bulk_thing_registration_reports.rs"]
mod bulk_thing_registration_reports;
#[path = "all/certificate_deletion.rs"]
mod certificate_deletion;
#[path = "all/certificate_providers.rs"]
mod certificate_providers;
#[path = "all/certificate_status_update.rs"]
mod certificate_status_update;
#[path = "all/command_execution_listing.rs"]
mod command_execution_listing;
#[path = "all/command_execution_read.rs"]
mod command_execution_read;
#[path = "all/command_listing.rs"]
mod command_listing;
#[path = "all/command_registry.rs"]
mod command_registry;
#[path = "all/describe_endpoint.rs"]
mod describe_endpoint;
#[path = "all/device_certificate_issuance.rs"]
mod device_certificate_issuance;
#[path = "all/domain_configurations.rs"]
mod domain_configurations;
#[path = "all/effective_policies.rs"]
mod effective_policies;
#[path = "all/endpoint_discovery_reachability.rs"]
mod endpoint_discovery_reachability;
#[path = "all/fleet_index_description.rs"]
mod fleet_index_description;
#[path = "all/fleet_metrics.rs"]
mod fleet_metrics;
#[path = "all/indexing_configuration.rs"]
mod indexing_configuration;
#[path = "all/job_cancellation.rs"]
mod job_cancellation;
#[path = "all/job_description.rs"]
mod job_description;
#[path = "all/job_document_read.rs"]
mod job_document_read;
#[path = "all/job_execution_deletion.rs"]
mod job_execution_deletion;
#[path = "all/job_execution_listing_for_thing.rs"]
mod job_execution_listing_for_thing;
#[path = "all/job_execution_read.rs"]
mod job_execution_read;
#[path = "all/job_targeting.rs"]
mod job_targeting;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/on_demand_audit_task.rs"]
mod on_demand_audit_task;
#[path = "all/ota_update_deployment.rs"]
mod ota_update_deployment;
#[path = "all/ota_update_job_document.rs"]
mod ota_update_job_document;
#[path = "all/package_catalog.rs"]
mod package_catalog;
#[path = "all/package_default_version.rs"]
mod package_default_version;
#[path = "all/package_deletion.rs"]
mod package_deletion;
#[path = "all/package_version_update.rs"]
mod package_version_update;
#[path = "all/package_versions.rs"]
mod package_versions;
#[path = "all/policy_attachments.rs"]
mod policy_attachments;
#[path = "all/policy_deletion.rs"]
mod policy_deletion;
#[path = "all/policy_registry.rs"]
mod policy_registry;
#[path = "all/policy_versions.rs"]
mod policy_versions;
#[path = "all/provisioning_templates.rs"]
mod provisioning_templates;
#[path = "all/role_alias_deletion.rs"]
mod role_alias_deletion;
#[path = "all/role_alias_registry.rs"]
mod role_alias_registry;
#[path = "all/role_alias_update.rs"]
mod role_alias_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/streams.rs"]
mod streams;
#[path = "all/thing_group_hierarchy.rs"]
mod thing_group_hierarchy;
#[path = "all/thing_group_membership.rs"]
mod thing_group_membership;
#[path = "all/thing_group_update.rs"]
mod thing_group_update;
#[path = "all/thing_listing.rs"]
mod thing_listing;
#[path = "all/thing_registry_lifecycle.rs"]
mod thing_registry_lifecycle;
#[path = "all/thing_type_registry.rs"]
mod thing_type_registry;
#[path = "all/thing_update.rs"]
mod thing_update;
#[path = "all/topic_rule_deletion.rs"]
mod topic_rule_deletion;
#[path = "all/topic_rule_destination_lifecycle.rs"]
mod topic_rule_destination_lifecycle;
#[path = "all/topic_rule_enablement.rs"]
mod topic_rule_enablement;
#[path = "all/topic_rule_kinesis_action.rs"]
mod topic_rule_kinesis_action;
#[path = "all/topic_rule_listing.rs"]
mod topic_rule_listing;
#[path = "all/topic_rule_registry.rs"]
mod topic_rule_registry;
#[path = "all/topic_rule_replacement.rs"]
mod topic_rule_replacement;
#[path = "all/v2_logging_options.rs"]
mod v2_logging_options;
#[path = "all/principal_policy_attachment.rs"]
mod principal_policy_attachment;
