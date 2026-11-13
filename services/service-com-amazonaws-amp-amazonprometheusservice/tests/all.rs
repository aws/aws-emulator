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
#[path = "all/alert_manager_definition.rs"]
mod alert_manager_definition;
#[path = "all/announced_endpoint_composition.rs"]
mod announced_endpoint_composition;
#[path = "all/anomaly_detector_creation_by_put.rs"]
mod anomaly_detector_creation_by_put;
#[path = "all/anomaly_detector_deletion.rs"]
mod anomaly_detector_deletion;
#[path = "all/anomaly_detector_lifecycle.rs"]
mod anomaly_detector_lifecycle;
#[path = "all/anomaly_detector_listing.rs"]
mod anomaly_detector_listing;
#[path = "all/anomaly_detector_update.rs"]
mod anomaly_detector_update;
#[path = "all/audit_records.rs"]
mod audit_records;
#[path = "all/create_on_a_workspace_whose_removal_is_owed.rs"]
mod create_on_a_workspace_whose_removal_is_owed;
#[path = "all/create_while_a_removal_is_owed.rs"]
mod create_while_a_removal_is_owed;
#[path = "all/default_scraper_configuration.rs"]
mod default_scraper_configuration;
#[path = "all/describe_scraper.rs"]
mod describe_scraper;
#[path = "all/kms_key_arn_and_parent_resolution.rs"]
mod kms_key_arn_and_parent_resolution;
#[path = "all/list_paging_rule.rs"]
mod list_paging_rule;
#[path = "all/logging_configuration.rs"]
mod logging_configuration;
#[path = "all/logging_configuration_deletion.rs"]
mod logging_configuration_deletion;
#[path = "all/map_key_constraints.rs"]
mod map_key_constraints;
#[path = "all/namespace_tag_removal.rs"]
mod namespace_tag_removal;
#[path = "all/prometheus_data_plane.rs"]
mod prometheus_data_plane;
#[path = "all/query_logging_configuration.rs"]
mod query_logging_configuration;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/rejection_codes.rs"]
mod rejection_codes;
#[path = "all/replay_while_a_removal_is_owed.rs"]
mod replay_while_a_removal_is_owed;
#[path = "all/resource_policy_lifecycle.rs"]
mod resource_policy_lifecycle;
#[path = "all/rule_group_name_bound.rs"]
mod rule_group_name_bound;
#[path = "all/rule_groups_namespace_deletion.rs"]
mod rule_groups_namespace_deletion;
#[path = "all/rule_groups_namespace_lifecycle.rs"]
mod rule_groups_namespace_lifecycle;
#[path = "all/rule_groups_namespace_listing.rs"]
mod rule_groups_namespace_listing;
#[path = "all/rule_groups_namespace_listing_scope.rs"]
mod rule_groups_namespace_listing_scope;
#[path = "all/rule_groups_namespace_update.rs"]
mod rule_groups_namespace_update;
#[path = "all/scraper_creation.rs"]
mod scraper_creation;
#[path = "all/scraper_deletion.rs"]
mod scraper_deletion;
#[path = "all/scraper_destination_resolution.rs"]
mod scraper_destination_resolution;
#[path = "all/scraper_listing.rs"]
mod scraper_listing;
#[path = "all/scraper_logging_configuration_deletion.rs"]
mod scraper_logging_configuration_deletion;
#[path = "all/scraper_logging_configuration_describe.rs"]
mod scraper_logging_configuration_describe;
#[path = "all/scraper_logging_configuration_update.rs"]
mod scraper_logging_configuration_update;
#[path = "all/scraper_update.rs"]
mod scraper_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/status_reason_and_instant_resolution.rs"]
mod status_reason_and_instant_resolution;
#[path = "all/tag_key_constraints.rs"]
mod tag_key_constraints;
#[path = "all/taggable_resource_kinds.rs"]
mod taggable_resource_kinds;
#[path = "all/tagging_api_arn_resolution.rs"]
mod tagging_api_arn_resolution;
#[path = "all/wire_fidelity.rs"]
mod wire_fidelity;
#[path = "all/workspace_alias_publication.rs"]
mod workspace_alias_publication;
#[path = "all/workspace_configuration.rs"]
mod workspace_configuration;
#[path = "all/workspace_lifecycle.rs"]
mod workspace_lifecycle;
#[path = "all/workspace_not_found_composition.rs"]
mod workspace_not_found_composition;
#[path = "all/workspace_rename.rs"]
mod workspace_rename;
