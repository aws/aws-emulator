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
#[path = "all/account_targeting_agreement.rs"]
mod account_targeting_agreement;
#[path = "all/action_catalog.rs"]
mod action_catalog;
#[path = "all/action_catalog_coverage.rs"]
mod action_catalog_coverage;
#[path = "all/action_order_is_acyclic.rs"]
mod action_order_is_acyclic;
// WHICH role's documents decide whether an action may be carried out, and WHAT in them
// authorizes it: any one of the grants the action documents.
#[path = "all/action_privileges_gate.rs"]
mod action_privileges_gate;
#[path = "all/control_plane_rules.rs"]
mod control_plane_rules;
#[path = "all/delete_target_account_configuration.rs"]
mod delete_target_account_configuration;
#[path = "all/execution_role_is_one_lookup.rs"]
mod execution_role_is_one_lookup;
#[path = "all/experiment_action_state.rs"]
mod experiment_action_state;
// What a started experiment DOES to the resources its targets resolved to: the
// `aws:ec2:reboot-instances` action carried out on the EC2 store of the same engine, and the
// progress the action itself then reports.
#[path = "all/experiment_actions_reach_their_targets.rs"]
mod experiment_actions_reach_their_targets;
#[path = "all/experiment_lifecycle.rs"]
mod experiment_lifecycle;
#[path = "all/experiment_logging.rs"]
mod experiment_logging;
#[path = "all/experiment_options_decide_the_run.rs"]
mod experiment_options_decide_the_run;
#[path = "all/experiment_run.rs"]
mod experiment_run;
#[path = "all/experiment_state_reason_and_report.rs"]
mod experiment_state_reason_and_report;
#[path = "all/experiment_target_account_configurations.rs"]
mod experiment_target_account_configurations;
#[path = "all/experiment_template_not_found.rs"]
mod experiment_template_not_found;
#[path = "all/experiment_template_shape_floors.rs"]
mod experiment_template_shape_floors;
#[path = "all/fresh_start_reports_the_runs_it_ends.rs"]
mod fresh_start_reports_the_runs_it_ends;
#[path = "all/get_target_resource_type.rs"]
mod get_target_resource_type;
#[path = "all/length_refusal_wording.rs"]
mod length_refusal_wording;
#[path = "all/list_actions.rs"]
mod list_actions;
#[path = "all/list_experiment_resolved_targets.rs"]
mod list_experiment_resolved_targets;
#[path = "all/list_experiment_target_account_configurations.rs"]
mod list_experiment_target_account_configurations;
#[path = "all/list_page_walk.rs"]
mod list_page_walk;
#[path = "all/list_tags_for_resource.rs"]
mod list_tags_for_resource;
#[path = "all/list_target_account_configurations.rs"]
mod list_target_account_configurations;
#[path = "all/list_target_resource_types.rs"]
mod list_target_resource_types;
#[path = "all/modelled_map_key_bounds.rs"]
mod modelled_map_key_bounds;
#[path = "all/published_catalog_entries.rs"]
mod published_catalog_entries;
#[path = "all/published_catalog_text.rs"]
mod published_catalog_text;
#[path = "all/refusal_class_and_message.rs"]
mod refusal_class_and_message;
#[path = "all/refused_calls_deliver_their_own_settle.rs"]
mod refused_calls_deliver_their_own_settle;
#[path = "all/report_follows_whoever_ends_the_run.rs"]
mod report_follows_whoever_ends_the_run;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/safety_lever.rs"]
mod safety_lever;
#[path = "all/safety_lever_exists_and_refusal_wording.rs"]
mod safety_lever_exists_and_refusal_wording;
#[path = "all/safety_lever_settles_and_reports.rs"]
mod safety_lever_settles_and_reports;
#[path = "all/start_experiment_replay.rs"]
mod start_experiment_replay;
#[path = "all/start_experiment_token_normalization.rs"]
mod start_experiment_token_normalization;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stop_condition_halts_the_run.rs"]
mod stop_condition_halts_the_run;
#[path = "all/tagging_refusal_error_shape.rs"]
mod tagging_refusal_error_shape;
#[path = "all/tagging_response_envelope.rs"]
mod tagging_response_envelope;
#[path = "all/target_account_configuration_description.rs"]
mod target_account_configuration_description;
#[path = "all/target_account_configuration_errors.rs"]
mod target_account_configuration_errors;
#[path = "all/target_account_configuration_idempotency.rs"]
mod target_account_configuration_idempotency;
#[path = "all/target_account_configuration_page_walk.rs"]
mod target_account_configuration_page_walk;
#[path = "all/target_account_configuration_refusal_wording.rs"]
mod target_account_configuration_refusal_wording;
#[path = "all/target_account_configurations.rs"]
mod target_account_configurations;
#[path = "all/target_account_id_is_an_account_number.rs"]
mod target_account_id_is_an_account_number;
#[path = "all/target_account_role_ownership.rs"]
mod target_account_role_ownership;
// What a target RESOLVES to: the resources it names, and the resources of other services
// carrying the tags it describes, narrowed by its filters.
#[path = "all/targets_resolve_against_tagged_resources.rs"]
mod targets_resolve_against_tagged_resources;
#[path = "all/template_admissibility_sequence.rs"]
mod template_admissibility_sequence;
#[path = "all/template_target_and_action_rules.rs"]
mod template_target_and_action_rules;
#[path = "all/template_target_resource_type_rules.rs"]
mod template_target_resource_type_rules;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/update_experiment_template.rs"]
mod update_experiment_template;
