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
#[path = "all/config_aggregation_authorization.rs"]
mod config_aggregation_authorization;
#[path = "all/config_aggregator_lifecycle.rs"]
mod config_aggregator_lifecycle;
#[path = "all/config_compliance_by_config_rule.rs"]
mod config_compliance_by_config_rule;
#[path = "all/config_connectors.rs"]
mod config_connectors;
#[path = "all/config_delivery_channel_status.rs"]
mod config_delivery_channel_status;
#[path = "all/config_discovered_resource_counts.rs"]
mod config_discovered_resource_counts;
// Declared HERE, inside the sorted list, rather than after it: the tail of this file is
// where concurrent suites append, so a line added there conflicts with every other one
// landing in the same window, while this span is untouched by those appends.
#[path = "all/config_external_evaluation_compliance.rs"]
mod config_external_evaluation_compliance;
#[path = "all/config_item_change_notification.rs"]
mod config_item_change_notification;
#[path = "all/config_read_shape_and_compliance.rs"]
mod config_read_shape_and_compliance;
#[path = "all/config_recorded_resource_compliance.rs"]
mod config_recorded_resource_compliance;
#[path = "all/config_remediation_configurations.rs"]
mod config_remediation_configurations;
#[path = "all/config_retention_configuration_lifecycle.rs"]
mod config_retention_configuration_lifecycle;
#[path = "all/config_rule_evaluation_status.rs"]
mod config_rule_evaluation_status;
#[path = "all/config_service_linked_recorder.rs"]
mod config_service_linked_recorder;
#[path = "all/config_setup_and_rule_loop.rs"]
mod config_setup_and_rule_loop;
#[path = "all/config_stored_queries.rs"]
mod config_stored_queries;
#[path = "all/config_tagging.rs"]
mod config_tagging;
#[path = "all/config_third_party_service_linked_recorder.rs"]
mod config_third_party_service_linked_recorder;
// Declared inside the sorted list, where `connector_*` sorts, rather than appended at
// the tail block below: every suite that lands appends there, so a declaration added
// there conflicts with the next one that lands.
#[path = "all/connector_credential_verification.rs"]
mod connector_credential_verification;
#[path = "all/discovered_resources.rs"]
mod discovered_resources;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/list_configuration_recorders.rs"]
mod list_configuration_recorders;
#[path = "all/pending_aggregation_requests.rs"]
mod pending_aggregation_requests;
#[path = "all/resource_evaluations.rs"]
mod resource_evaluations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Declared after the sorted list on purpose: every new suite this crate gains is
// inserted into the `config_*` block above, and a declaration added there conflicts
// with the next one that lands. Fold these lines into the sorted list whenever the
// list is next touched for another reason.
#[path = "all/config_custom_rule_policy.rs"]
mod config_custom_rule_policy;
#[path = "all/compliance_details_by_config_rule.rs"]
mod compliance_details_by_config_rule;
#[path = "all/proactive_resource_evaluation.rs"]
mod proactive_resource_evaluation;
#[path = "all/config_deliver_config_snapshot.rs"]
mod config_deliver_config_snapshot;
#[path = "all/aggregate_resource_config.rs"]
mod aggregate_resource_config;
#[path = "all/resource_evaluation_surface.rs"]
mod resource_evaluation_surface;
#[path = "all/delete_service_linked_recorder.rs"]
mod delete_service_linked_recorder;
#[path = "all/config_rules_on_demand_evaluation.rs"]
mod config_rules_on_demand_evaluation;
#[path = "all/config_delete_configuration_aggregator.rs"]
mod config_delete_configuration_aggregator;
#[path = "all/conformance_packs.rs"]
mod conformance_packs;
#[path = "all/select_resource_config.rs"]
mod select_resource_config;
#[path = "all/configuration_recorder_refusal_messages.rs"]
mod configuration_recorder_refusal_messages;
#[path = "all/list_stored_queries.rs"]
mod list_stored_queries;
#[path = "all/delete_stored_query.rs"]
mod delete_stored_query;
#[path = "all/delete_pending_aggregation_request.rs"]
mod delete_pending_aggregation_request;
#[path = "all/aggregate_config_rule_compliance_summary.rs"]
mod aggregate_config_rule_compliance_summary;
#[path = "all/external_evaluation.rs"]
mod external_evaluation;
#[path = "all/configuration_aggregator_sources_status.rs"]
mod configuration_aggregator_sources_status;
#[path = "all/aggregate_compliance_by_config_rules.rs"]
mod aggregate_compliance_by_config_rules;
#[path = "all/put_evaluations.rs"]
mod put_evaluations;
#[path = "all/conformance_pack_rule_names.rs"]
mod conformance_pack_rule_names;
#[path = "all/remediation_execution.rs"]
mod remediation_execution;
#[path = "all/config_refusal_message_templates.rs"]
mod config_refusal_message_templates;
#[path = "all/compliance_summary_by_resource_type.rs"]
mod compliance_summary_by_resource_type;
#[path = "all/compliance_details_by_resource.rs"]
mod compliance_details_by_resource;
#[path = "all/recorded_resource_arn.rs"]
mod recorded_resource_arn;
#[path = "all/remediation_execution_failed_items.rs"]
mod remediation_execution_failed_items;
#[path = "all/resource_config_deletion.rs"]
mod resource_config_deletion;
#[path = "all/compliance_by_resource.rs"]
mod compliance_by_resource;
#[path = "all/resource_type_association.rs"]
mod resource_type_association;
#[path = "all/delete_remediation_exceptions.rs"]
mod delete_remediation_exceptions;
#[path = "all/select_aggregate_resource_config.rs"]
mod select_aggregate_resource_config;
#[path = "all/recorded_refusal_wording.rs"]
mod recorded_refusal_wording;
#[path = "all/put_remediation_exceptions.rs"]
mod put_remediation_exceptions;
#[path = "all/describe_conformance_packs.rs"]
mod describe_conformance_packs;
#[path = "all/delete_evaluation_results.rs"]
mod delete_evaluation_results;
#[path = "all/conformance_pack_compliance.rs"]
mod conformance_pack_compliance;
#[path = "all/remediation_runs_its_runbook.rs"]
mod remediation_runs_its_runbook;
#[path = "all/batch_get_resource_config.rs"]
mod batch_get_resource_config;
#[path = "all/conformance_pack_compliance_details.rs"]
mod conformance_pack_compliance_details;
#[path = "all/recorder_discovers_other_service_resources.rs"]
mod recorder_discovers_other_service_resources;
#[path = "all/aggregate_conformance_pack_compliance_summary.rs"]
mod aggregate_conformance_pack_compliance_summary;
#[path = "all/conformance_pack_in_use_message.rs"]
mod conformance_pack_in_use_message;
#[path = "all/aggregate_compliance_by_conformance_packs.rs"]
mod aggregate_compliance_by_conformance_packs;
#[path = "all/get_resource_config_history.rs"]
mod get_resource_config_history;
#[path = "all/config_write_path_validation.rs"]
mod config_write_path_validation;
#[path = "all/recorded_inventory_drives_the_read_surface.rs"]
mod recorded_inventory_drives_the_read_surface;
#[path = "all/recorded_resource_config_history.rs"]
mod recorded_resource_config_history;
#[path = "all/aggregate_discovered_resources.rs"]
mod aggregate_discovered_resources;
// Declared beside the suite it is the counting half of, rather than appended at the
// tail: every suite that lands appends there, so a declaration added there conflicts
// with the next one that lands.
#[path = "all/aggregate_discovered_resource_counts.rs"]
mod aggregate_discovered_resource_counts;
#[path = "all/recorded_configuration_item_capture_time.rs"]
mod recorded_configuration_item_capture_time;
#[path = "all/unrecorded_resource_keys.rs"]
mod unrecorded_resource_keys;
#[path = "all/config_refusal_identifier_and_position.rs"]
mod config_refusal_identifier_and_position;
#[path = "all/refusals_that_share_an_error_code.rs"]
mod refusals_that_share_an_error_code;
#[path = "all/config_refusal_class_and_wording.rs"]
mod config_refusal_class_and_wording;
#[path = "all/config_snapshot_delivery.rs"]
mod config_snapshot_delivery;
#[path = "all/delivery_channel_bucket_policy.rs"]
mod delivery_channel_bucket_policy;
// Declared beside the suite whose policy helpers it reuses, rather than appended at the
// tail: every suite that lands appends there, so a declaration added there conflicts with
// the next one that lands.
#[path = "all/snapshot_delivery_bucket_policy.rs"]
mod snapshot_delivery_bucket_policy;
#[path = "all/delivery_channel_recorder_role.rs"]
mod delivery_channel_recorder_role;
#[path = "all/validation_clause_quotes_the_submitted_value.rs"]
mod validation_clause_quotes_the_submitted_value;
#[path = "all/registry_resource_types_and_remediation_targets.rs"]
mod registry_resource_types_and_remediation_targets;
#[path = "all/conformance_pack_rule_evaluation.rs"]
mod conformance_pack_rule_evaluation;
#[path = "all/recorder_discovers_iam_roles.rs"]
mod recorder_discovers_iam_roles;
#[path = "all/conformance_pack_deletion.rs"]
mod conformance_pack_deletion;
#[path = "all/custom_lambda_rule_evaluation.rs"]
mod custom_lambda_rule_evaluation;
#[path = "all/pack_rule_evaluates_a_recorded_resource.rs"]
mod pack_rule_evaluates_a_recorded_resource;
#[path = "all/read_paths_and_assigned_ids.rs"]
mod read_paths_and_assigned_ids;
#[path = "all/conformance_pack_template_refusal_message.rs"]
mod conformance_pack_template_refusal_message;
#[path = "all/conformance_pack_not_found_message.rs"]
mod conformance_pack_not_found_message;
#[path = "all/remediation_needs_a_non_compliant_finding.rs"]
mod remediation_needs_a_non_compliant_finding;
#[path = "all/conformance_pack_type_and_stack_identity.rs"]
mod conformance_pack_type_and_stack_identity;
#[path = "all/recorded_item_status_and_resumed_recording.rs"]
mod recorded_item_status_and_resumed_recording;
// The suite in `all/scratch_resume.rs`: the recorded configuration document and the
// digest a caller compares two configuration items by. The layout check requires the
// module name to match the file name.
#[path = "all/scratch_resume.rs"]
mod scratch_resume;
