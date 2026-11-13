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
//
// `query_results` is declared HERE, between that note and the first suite below,
// rather than in the sorted list or at the END of the file: those are the two
// stretches every concurrent iotsitewise change puts a declaration of its own in,
// and each attempt to land this one in either collided with a branch that had just
// inserted beside it. Fold it into the list once the churn settles.
// `task_delete` is declared HERE, at the head of the file, rather than in the
// sorted list or at the END: those are the two stretches every concurrent
// iotsitewise change puts a declaration of its own in. Fold it into the list once
// the churn settles.
// `describe_dataset_reads` is declared HERE, at the head of the file, rather than
// in the sorted list or at the END: those are the two stretches every concurrent
// iotsitewise change puts a declaration of its own in. Fold it into the list once
// the churn settles.
// `asset_model_definition_validation` is declared HERE, beside the suites below,
// for the reason the notes above give: the sorted list and the END of this file
// are both stretches every concurrent iotsitewise change puts a declaration of
// its own in. Fold it into the list once the churn settles.
#[path = "all/asset_model_definition_validation.rs"]
mod asset_model_definition_validation;

// `assistant_invocation` is declared HERE, beside the suite above, for the reason
// the notes above give: the sorted list and the END of this file are both
// stretches every concurrent iotsitewise change puts a declaration of its own in.
// Fold it into the list once the churn settles.
#[path = "all/assistant_invocation.rs"]
mod assistant_invocation;

// `variable_reference_resolution` is declared HERE, beside the suite above whose
// refusal rules it is the resolution half of, for the reason the notes above
// give: the sorted list and the END of this file are both stretches every
// concurrent iotsitewise change puts a declaration of its own in. Fold it into
// the list once the churn settles.
#[path = "all/variable_reference_resolution.rs"]
mod variable_reference_resolution;

// `property_paths_and_notifications` is declared HERE, beside the suite above,
// for the reason the notes above give: the sorted list and the END of this file
// are both stretches every concurrent iotsitewise change puts a declaration of
// its own in. Fold it into the list once the churn settles.
#[path = "all/property_paths_and_notifications.rs"]
mod property_paths_and_notifications;
// `asset_delete_reference_cleanup` is declared HERE, beside the suite above, for
// the reason the notes above give: the sorted list and the END of this file are
// both stretches every concurrent iotsitewise change puts a declaration of its
// own in. Fold it into the list once the churn settles.
#[path = "all/asset_delete_reference_cleanup.rs"]
mod asset_delete_reference_cleanup;

// `dataset_export_job_token_scope` is declared HERE, beside the suite above, for
// the reason the notes above give: the sorted list and the END of this file are
// both stretches every concurrent iotsitewise change puts a declaration of its
// own in. Fold it into the list once the churn settles.
#[path = "all/dataset_export_job_token_scope.rs"]
mod dataset_export_job_token_scope;

// `create_dashboard_client_token` is declared HERE, beside the suite above, for
// the reason the notes above give: the sorted list and the END of this file are
// both stretches every concurrent iotsitewise change puts a declaration of its
// own in. Fold it into the list once the churn settles.
#[path = "all/create_dashboard_client_token.rs"]
mod create_dashboard_client_token;

// `asset_model_delete_interface_applications` is declared HERE, beside the
// suites below, for the reason the notes above give. Fold it into the list once
// the churn settles.
#[path = "all/asset_model_delete_interface_applications.rs"]
mod asset_model_delete_interface_applications;

#[path = "all/describe_dataset_reads.rs"]
mod describe_dataset_reads;

#[path = "all/task_delete.rs"]
mod task_delete;

// `published_property_history` is declared HERE, beside the suite above, for the
// reason the note above gives: the sorted list and the END of this file are both
// stretches every concurrent iotsitewise change puts a declaration of its own in.
// Fold it into the list once the churn settles.
#[path = "all/published_property_history.rs"]
mod published_property_history;

// `composite_model_property_update` is declared HERE, beside the suite above, for
// the reason its note gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/composite_model_property_update.rs"]
mod composite_model_property_update;

// `not_found_refusal_families` is declared HERE, beside the suite above, for the
// reason its note gives: the sorted list and the END of this file are both where
// every concurrent iotsitewise change puts a declaration of its own. Fold it into
// the list once the churn settles.
#[path = "all/not_found_refusal_families.rs"]
mod not_found_refusal_families;

// `monitor_resource_grant_sweep` is declared HERE, beside the suite above, for the
// reason its note gives: the sorted list and the END of this file are both where
// every concurrent iotsitewise change puts a declaration of its own. Fold it into
// the list once the churn settles.
#[path = "all/monitor_resource_grant_sweep.rs"]
mod monitor_resource_grant_sweep;

// `refusal_resource_arn` is declared HERE, beside the suite above, for the reason
// its note gives: the sorted list and the END of this file are both where every
// concurrent iotsitewise change puts a declaration of its own. Fold it into the
// list once the churn settles.
#[path = "all/refusal_resource_arn.rs"]
mod refusal_resource_arn;

// `client_token_conflict_names_a_held_record` is declared HERE, beside the suite
// above, for the reason its note gives: the sorted list and the END of this file
// are both where every concurrent iotsitewise change puts a declaration of its own.
// Fold it into the list once the churn settles.
#[path = "all/client_token_conflict_names_a_held_record.rs"]
mod client_token_conflict_names_a_held_record;

// `pipeline_absence_depth` is declared HERE, beside the suite above, for the
// reason its note gives: the sorted list and the END of this file are both where
// every concurrent iotsitewise change puts a declaration of its own. Fold it into
// the list once the churn settles.
#[path = "all/pipeline_absence_depth.rs"]
mod pipeline_absence_depth;

// `asset_model_name_uniqueness` is declared HERE, beside the suite above, for the
// reason its note gives: the sorted list and the END of this file are both where
// every concurrent iotsitewise change puts a declaration of its own. Fold it into
// the list once the churn settles.
#[path = "all/asset_model_name_uniqueness.rs"]
mod asset_model_name_uniqueness;

#[path = "all/query_results.rs"]
mod query_results;

// `asset_model_delete_dependents` is declared HERE, beside the suite above, for
// the reason the note above gives: the sorted list and the END of this file are
// both stretches every concurrent iotsitewise change puts a declaration of its
// own in. Fold it into the list once the churn settles.
#[path = "all/asset_model_delete_dependents.rs"]
mod asset_model_delete_dependents;

// `composite_model_definition_views` is declared HERE, beside the suite above, for
// the reason its note gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/composite_model_definition_views.rs"]
mod composite_model_definition_views;

// `component_model_property_inheritance` is declared HERE, beside the suite above,
// for the reason that note gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/component_model_property_inheritance.rs"]
mod component_model_property_inheritance;

// Declared HERE, beside the suite above, for the reason that note gives.
#[path = "all/component_model_definition_reach.rs"]
mod component_model_definition_reach;

// `pagination_token_binding` is declared HERE, beside the suite above, for the
// reason the note above gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/pagination_token_binding.rs"]
mod pagination_token_binding;

// `pagination_token_spelling` is declared HERE, beside the suite it belongs with,
// for the reason that suite's note gives: the sorted list and the END of this file
// are both where every concurrent iotsitewise change puts a declaration of its own.
// Fold it into the list once the churn settles.
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;

// Declared here beside the suite above, for the reason its note gives.
#[path = "all/asset_model_precondition_wording.rs"]
mod asset_model_precondition_wording;

// `task_create_time_tags` is declared HERE, beside the note above, for the
// reason it gives: the sorted list and the END of this file are both stretches
// every concurrent iotsitewise change puts a declaration of its own in. Fold it
// into the list once the churn settles.
#[path = "all/task_create_time_tags.rs"]
mod task_create_time_tags;

// `composite_model_property_streams` is declared HERE, beside the suite above, for
// the reason its note gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/composite_model_property_streams.rs"]
mod composite_model_property_streams;

// `batch_latest_composite_entries` is declared HERE, beside the suite whose
// fixture it reuses, for the reason that suite's note gives: the sorted list and
// the END of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/batch_latest_composite_entries.rs"]
mod batch_latest_composite_entries;

// `batch_history_composite_entries` is declared HERE, beside the suite whose
// fixture it reuses, for the reason that suite's note gives: the sorted list and
// the END of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/batch_history_composite_entries.rs"]
mod batch_history_composite_entries;

// `composite_property_aggregate_windows` is declared HERE, beside the suite above
// it carries to `GetAssetPropertyAggregates`, for the reason that suite's note
// gives: the sorted list and the END of this file are both where every concurrent
// iotsitewise change puts a declaration of its own. Fold it into the list once the
// churn settles.
#[path = "all/composite_property_aggregate_windows.rs"]
mod composite_property_aggregate_windows;

// `client_token_spent_on_another_request` is declared HERE, beside the note
// above, for the reason it gives: the sorted list and the END of this file are
// both stretches every concurrent iotsitewise change puts a declaration of its
// own in. Fold it into the list once the churn settles.
#[path = "all/client_token_spent_on_another_request.rs"]
mod client_token_spent_on_another_request;

// `client_token_spent_on_another_create` is declared HERE, beside the suite it
// belongs with, for the reason that suite's note gives: the sorted list and the
// END of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/client_token_spent_on_another_create.rs"]
mod client_token_spent_on_another_create;

// `workspace_scoped_path_resolution` is declared HERE, beside the note above,
// for the reason it gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold
// it into the list once the churn settles.
#[path = "all/workspace_scoped_path_resolution.rs"]
mod workspace_scoped_path_resolution;

// `workspace_scoped_job_reads` is declared HERE, beside the suite it belongs
// with, for the reason the note above gives: the sorted list and the END of this
// file are both where every concurrent iotsitewise change puts a declaration of
// its own. Fold it into the list once the churn settles.
#[path = "all/workspace_scoped_job_reads.rs"]
mod workspace_scoped_job_reads;

// `list_workspaces` is declared HERE, beside the note above, rather than in the
// sorted list or at the END of the file: those are the two stretches every
// concurrent iotsitewise change puts a declaration of its own in. Fold it into
// the list once the churn settles.
#[path = "all/list_workspaces.rs"]
mod list_workspaces;

// `component_model_definition_propagation` is declared HERE, beside the note
// above, for the reason it gives: the sorted list and the END of this file are
// both where every concurrent iotsitewise change puts a declaration of its own.
// Fold it into the list once the churn settles.
#[path = "all/component_model_definition_propagation.rs"]
mod component_model_definition_propagation;

// `delete_dataset` is declared HERE, beside the note above, for the reason it
// gives: the sorted list and the END of this file are both where every concurrent
// iotsitewise change puts a declaration of its own. Fold it into the list once the
// churn settles.
#[path = "all/delete_dataset.rs"]
mod delete_dataset;

// `delete_time_series_data_segments` is declared HERE, beside the suite above, for
// the reason its note gives: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold it
// into the list once the churn settles.
#[path = "all/delete_time_series_data_segments.rs"]
mod delete_time_series_data_segments;

// `asset_delete_data_segments` is declared HERE, beside the suite above whose
// rule it carries to `DeleteAsset`, for the reason that suite's note gives: the
// sorted list and the END of this file are both where every concurrent
// iotsitewise change puts a declaration of its own. Fold it into the list once
// the churn settles.
#[path = "all/asset_delete_data_segments.rs"]
mod asset_delete_data_segments;

// `client_token_replay` is declared HERE, at the head of the file, for the reason
// the note above gives: the sorted list and the END of this file are both where
// every concurrent iotsitewise change puts a declaration of its own. Fold it into
// the list once the churn settles.
#[path = "all/client_token_replay.rs"]
mod client_token_replay;

// `client_token_spent_by_another_request` is declared HERE, beside the
// `client_token_replay` suite it belongs with, for the reason that suite's note
// gives: the sorted list and the END of this file are both where every
// concurrent iotsitewise change puts a declaration of its own. Fold it into the
// list once the churn settles.
#[path = "all/client_token_spent_by_another_request.rs"]
mod client_token_spent_by_another_request;

// `client_token_names_the_call` is declared HERE, beside the two token suites it
// belongs with, for the reason their notes give: the sorted list and the END of
// this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/client_token_names_the_call.rs"]
mod client_token_names_the_call;

// `search_client_token_names_the_call` is declared HERE, beside the three token
// suites it belongs with, for the reason their notes give: the sorted list and
// the END of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/search_client_token_names_the_call.rs"]
mod search_client_token_names_the_call;

// `ingest_storage_configuration` is declared HERE, beside the suites above, for
// the reason their notes give: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold
// it into the list once the churn settles.
#[path = "all/ingest_storage_configuration.rs"]
mod ingest_storage_configuration;

// `computation_model_resolve_to_resources` is declared HERE, beside the suite it
// belongs with, rather than in the sorted list or at the END of the file: those
// are the two stretches every concurrent iotsitewise change puts a declaration of
// its own in. Fold it into the list once the churn settles.
#[path = "all/computation_model_resolve_to_resources.rs"]
mod computation_model_resolve_to_resources;

// `computation_model_execution_summary` is declared HERE, beside the note above,
// rather than in the sorted list or at the END of the file: those are the two
// stretches every concurrent iotsitewise change puts a declaration of its own in.
// Fold it into the list once the churn settles.
#[path = "all/computation_model_execution_summary.rs"]
mod computation_model_execution_summary;

// `computation_model_delete` is declared HERE, beside the computation-model
// suite above, for the reason its note gives: the sorted list and the END of this
// file are both stretches every concurrent iotsitewise change puts a declaration
// of its own in. Fold it into the list once the churn settles.
#[path = "all/computation_model_delete.rs"]
mod computation_model_delete;

// `computation_model_client_token_names_the_call` is declared HERE, beside the
// computation-model suites above, for the reason their notes give: the sorted list
// and the END of this file are both stretches every concurrent iotsitewise change
// puts a declaration of its own in. Fold it into the list once the churn settles.
#[path = "all/computation_model_client_token_names_the_call.rs"]
mod computation_model_client_token_names_the_call;

// `workspace_scoped_creates` is declared HERE, beside the suites above, for the
// reason their notes give: the sorted list and the END of this file are both
// where every concurrent iotsitewise change puts a declaration of its own. Fold
// it into the list once the churn settles.
#[path = "all/workspace_scoped_creates.rs"]
mod workspace_scoped_creates;

// `actions` is declared HERE, at the head of the file, rather than in the sorted
// list or appended after it: both of those are where every concurrent
// iotsitewise change puts a declaration of its own, and each of three attempts
// to land this one there collided with a branch that had just appended beside
// it. Fold it into the list once the churn settles.
#[path = "all/actions.rs"]
mod actions;

// `execute_action` is declared HERE, beside the `actions` suite it belongs with,
// for the reason the note above gives and because the END of this file is where
// every concurrent iotsitewise change appends a declaration of its own: two
// appends there collided in a row with this one. Fold it into the list once the
// churn settles.
#[path = "all/execute_action.rs"]
mod execute_action;

// `describe_action_reads` is declared HERE, beside the two action suites it
// belongs with, for the reason the notes above give: the sorted list and the END
// of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/describe_action_reads.rs"]
mod describe_action_reads;

// `composition_relationships` is declared HERE, after the action suites, rather
// than at the head of the file, in the sorted list or at the END: all three are
// stretches a concurrent iotsitewise change puts a declaration of its own in, and
// two attempts to land this one at the head collided with a branch that had just
// inserted beside it. Fold it into the list once the churn settles.
#[path = "all/composition_relationships.rs"]
mod composition_relationships;

// `reserved_tag_keys` is declared here too, at the head of the file, for the
// reason the note above gives: the sorted list and the tail of the file are both
// where every concurrent iotsitewise change puts a declaration of its own, and
// each attempt to land this one in either collided with a branch that had just
// inserted beside it. Fold it into the list once the churn settles.
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;

// `start_query_anchor` is declared HERE, beside the note above, for the reason it
// gives: the sorted list and the END of this file are both where every concurrent
// iotsitewise change puts a declaration of its own. Fold it into the list once the
// churn settles.
#[path = "all/start_query_anchor.rs"]
mod start_query_anchor;

// `property_aggregates` is declared here, ahead of the sorted list, rather than in
// its alphabetical place: every other suite this crate has gained was inserted
// into that list or appended after it, so those are the two spots a concurrent
// change also edits. Fold it into the list once the churn settles.
#[path = "all/property_aggregates.rs"]
mod property_aggregates;

// Same reasoning as `property_aggregates` above, and declared beside it rather than
// inside the list or after it, which are the two spots the comment above names as
// the ones a concurrent change also edits. Fold it into the list once the churn
// settles.
#[path = "all/interpolated_property_values.rs"]
mod interpolated_property_values;

// Same reasoning again, and the reason this one is NOT in the sorted list: its
// alphabetical place is between `asset_property_update` and `associated_assets`,
// the stretch of the list every concurrent asset-side suite lands in, and a
// declaration there conflicted with each mainline tip this fix was rebased onto.
// Fold it into the list once the churn settles.
#[path = "all/asset_relationships.rs"]
mod asset_relationships;

// Same reasoning once more: this suite's alphabetical place is inside the sorted
// list, which is where every concurrent iotsitewise change lands a declaration of
// its own, so it is declared up here with the other suites kept out of that
// stretch. Fold it into the list once the churn settles.
#[path = "all/simultaneous_writes.rs"]
mod simultaneous_writes;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/dataset_lifecycle.rs"]
mod dataset_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/pipeline_executions.rs"]
mod pipeline_executions;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/refusal_message_rule.rs"]
mod refusal_message_rule;

// `workspace_scoped_absence` is declared HERE, beside the refusal-rule suite it
// belongs with, for the reason the notes above give: the sorted list and the END
// of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/workspace_scoped_absence.rs"]
mod workspace_scoped_absence;

// `dataset_read_workspace_resolution` is declared HERE, beside the absence suite
// it belongs with, for the reason the notes above give: the sorted list and the
// END of this file are both where every concurrent iotsitewise change puts a
// declaration of its own. Fold it into the list once the churn settles.
#[path = "all/dataset_read_workspace_resolution.rs"]
mod dataset_read_workspace_resolution;

// Same reasoning: this suite's alphabetical place is inside the sorted list, the
// stretch every concurrent iotsitewise change lands a declaration of its own in,
// so it is declared up here with the other suites kept out of that stretch. Fold
// it into the list once the churn settles.
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/search_lifecycle.rs"]
mod search_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/default_encryption_configuration.rs"]
mod default_encryption_configuration;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/task_lifecycle.rs"]
mod task_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/executions.rs"]
mod executions;

// Declared here beside the `executions` suite it belongs with, for the reason the
// notes above give: the sorted list and the END of this file are both where every
// concurrent iotsitewise change puts a declaration of its own. Fold it into the
// list once the churn settles.
#[path = "all/describe_execution_reads.rs"]
mod describe_execution_reads;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/attribute_default_values.rs"]
mod attribute_default_values;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/capture_data.rs"]
mod capture_data;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/list_gateways.rs"]
mod list_gateways;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/workspace_lifecycle.rs"]
mod workspace_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/account_configuration_effective_state.rs"]
mod account_configuration_effective_state;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/access_policy_lifecycle.rs"]
mod access_policy_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/access_policies.rs"]
mod access_policies;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/pipeline_delete.rs"]
mod pipeline_delete;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/create_pipeline_bounds.rs"]
mod create_pipeline_bounds;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/computation_model_lifecycle.rs"]
mod computation_model_lifecycle;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/asset_model_interface_relationship.rs"]
mod asset_model_interface_relationship;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/workspace_queries.rs"]
mod workspace_queries;

// Same reasoning: this suite's alphabetical place is inside the sorted list, the
// stretch every concurrent iotsitewise change lands a declaration of its own in,
// so it is declared up here with the other suites kept out of that stretch. Fold
// it into the list once the churn settles.
#[path = "all/asset_composite_model.rs"]
mod asset_composite_model;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/hierarchy_label_resolution.rs"]
mod hierarchy_label_resolution;

// Same reasoning: declared outside the sorted list, which is the stretch every
// concurrent iotsitewise change edits. Fold it into the list once the churn
// settles.
#[path = "all/associate_assets_duplicate_sentence.rs"]
mod associate_assets_duplicate_sentence;

#[path = "all/asset_hierarchies.rs"]
mod asset_hierarchies;
#[path = "all/asset_hierarchy_refusal_classes.rs"]
mod asset_hierarchy_refusal_classes;
#[path = "all/asset_lifecycle.rs"]
mod asset_lifecycle;
#[path = "all/asset_composite_model_instances.rs"]
mod asset_composite_model_instances;
#[path = "all/asset_model_composite_model.rs"]
mod asset_model_composite_model;
#[path = "all/asset_model_composite_model_delete.rs"]
mod asset_model_composite_model_delete;
#[path = "all/asset_model_properties.rs"]
mod asset_model_properties;
#[path = "all/asset_model_property_interface_summaries.rs"]
mod asset_model_property_interface_summaries;
// Declared only because the build requires every file under `tests/all/` to be:
// the suite was folded into `update_asset_model.rs` and this module now carries
// no test. See the file's own note.
#[path = "all/asset_model_update.rs"]
mod asset_model_update;
#[path = "all/asset_property_update.rs"]
mod asset_property_update;
#[path = "all/associated_assets.rs"]
mod associated_assets;
#[path = "all/batch_get_latest_values.rs"]
mod batch_get_latest_values;
#[path = "all/batch_property_value_history.rs"]
mod batch_property_value_history;
// In its alphabetical place inside the sorted list, between the two batch suites
// it sorts beside.
#[path = "all/batch_project_assets_client_token.rs"]
mod batch_project_assets_client_token;
#[path = "all/batch_put_timestamp_and_quality.rs"]
mod batch_put_timestamp_and_quality;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and three appends there collided in a row with this
// one.
#[path = "all/bulk_import_jobs.rs"]
mod bulk_import_jobs;
// Declared here, beside the sibling bulk-import suites, for the reason the note
// above gives.
#[path = "all/bulk_import_job_destination_delete.rs"]
mod bulk_import_job_destination_delete;
// Declared here, beside the sibling bulk-import suites, for the reason the note
// above gives.
#[path = "all/bulk_import_job_name_uniqueness.rs"]
mod bulk_import_job_name_uniqueness;
// Declared here, beside the sibling bulk-import suite, for the reason the note
// above gives.
#[path = "all/list_bulk_import_jobs.rs"]
mod list_bulk_import_jobs;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and four appends there collided in a row with this one.
#[path = "all/client_token_idempotency.rs"]
mod client_token_idempotency;
// In its alphabetical place inside the sorted list, rather than at the head of this
// file or appended after the list: both of those are where every concurrent
// iotsitewise change puts a declaration of its own, and two squashes in a row
// collided with this one at the head.
#[path = "all/composite_property_published_id_ingest.rs"]
mod composite_property_published_id_ingest;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there collided with this one.
#[path = "all/create_computation_model_bounds.rs"]
mod create_computation_model_bounds;
// In its alphabetical place inside the sorted list, beside the other create-bounds
// suite, rather than appended after the list: the END of this file is where every
// concurrent iotsitewise change appends a declaration of its own, and two appends
// there collided in a row with this one.
#[path = "all/create_dataset_bounds.rs"]
mod create_dataset_bounds;
// In its alphabetical place inside the sorted list, beside the sibling bounds
// suite, rather than appended after the list: the END of this file is where every
// concurrent iotsitewise change appends a declaration of its own, and an append
// there collided with this one.
#[path = "all/create_workspace_bounds.rs"]
mod create_workspace_bounds;
// In its alphabetical place INSIDE the sorted list rather than appended after it:
// the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and three appends there lost the squash race with this
// one.
#[path = "all/dashboard_delete.rs"]
mod dashboard_delete;
#[path = "all/dataset_export_jobs.rs"]
mod dataset_export_jobs;
#[path = "all/describe_asset_property.rs"]
mod describe_asset_property;
// In its alphabetical place inside the sorted list rather than appended after it:
// the END of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there lost the squash race with one.
#[path = "all/describe_enrichment_job_reads.rs"]
mod describe_enrichment_job_reads;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and four appends there collided in a row with this one.
#[path = "all/describe_logging_options_reads.rs"]
mod describe_logging_options_reads;
#[path = "all/describe_pipeline_reads.rs"]
mod describe_pipeline_reads;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there collided with this one.
#[path = "all/describe_portal_reads.rs"]
mod describe_portal_reads;
#[path = "all/describe_search_reads.rs"]
mod describe_search_reads;
#[path = "all/describe_storage_configuration_reads.rs"]
mod describe_storage_configuration_reads;
#[path = "all/describe_task_reads.rs"]
mod describe_task_reads;
// In its alphabetical place inside the sorted list, rather than appended after it:
// the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there collided with this one.
#[path = "all/enrichment_job_client_token_names_the_call.rs"]
mod enrichment_job_client_token_names_the_call;
#[path = "all/enrichment_jobs.rs"]
mod enrichment_jobs;
#[path = "all/execute_query.rs"]
mod execute_query;
#[path = "all/execute_query_follows_the_store.rs"]
mod execute_query_follows_the_store;
#[path = "all/execute_query_relational_statements.rs"]
mod execute_query_relational_statements;
#[path = "all/gateway_lifecycle.rs"]
mod gateway_lifecycle;
#[path = "all/hierarchies_etags_and_refusals.rs"]
mod hierarchies_etags_and_refusals;
// In its alphabetical place inside the sorted list, rather than appended after
// it: the end of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there collided with this one.
#[path = "all/list_asset_model_composite_models.rs"]
mod list_asset_model_composite_models;
#[path = "all/list_asset_models.rs"]
mod list_asset_models;
#[path = "all/list_dashboards.rs"]
mod list_dashboards;
// In its alphabetical place inside the sorted list, rather than appended after
// the list: the END of this file is where every concurrent iotsitewise change
// appends a declaration of its own, and each append there lost the squash race
// with this one.
#[path = "all/list_datasets.rs"]
mod list_datasets;
// In its alphabetical place inside the sorted list rather than appended after it:
// the END of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and each append there lost the squash race with this
// one.
#[path = "all/list_pipeline_executions.rs"]
mod list_pipeline_executions;
#[path = "all/list_pipelines.rs"]
mod list_pipelines;
// In its alphabetical place inside the sorted list rather than appended after it:
// the END of this file is where every concurrent iotsitewise change appends a
// declaration of its own, and two appends there lost the squash race with this
// one.
#[path = "all/list_searches.rs"]
mod list_searches;
#[path = "all/logging_options.rs"]
mod logging_options;
// In the sorted list, beside the sibling logging suites rather than appended
// after it: the END of this file is where every concurrent iotsitewise change
// appends a declaration of its own.
#[path = "all/logging_options_workspace_scope.rs"]
mod logging_options_workspace_scope;
// In the sorted list, in its alphabetical place beside the other portal suite,
// rather than appended after the list: the END of this file is where every
// concurrent iotsitewise change appends a declaration of its own, and each
// append there collided with this one.
#[path = "all/portal_delete.rs"]
mod portal_delete;
#[path = "all/portal_lifecycle.rs"]
mod portal_lifecycle;
// In the sorted list too, beside the sibling portal suites rather than appended
// after the list: the END of this file is where every concurrent iotsitewise
// change appends a declaration of its own, and three appends there lost the
// squash race with this one.
#[path = "all/portal_update.rs"]
mod portal_update;
#[path = "all/property_value_history.rs"]
mod property_value_history;
// In the sorted list, beside the sibling logging suites' concern rather than
// appended after the list: the END of this file is where every concurrent
// iotsitewise change appends a declaration of its own.
#[path = "all/put_logging_options_workspace_registry.rs"]
mod put_logging_options_workspace_registry;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/single_property_aggregates.rs"]
mod single_property_aggregates;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/storage_configuration.rs"]
mod storage_configuration;
#[path = "all/support.rs"]
mod support;
#[path = "all/tag_key_refusals.rs"]
mod tag_key_refusals;
#[path = "all/time_series.rs"]
mod time_series;
#[path = "all/time_series_disassociation.rs"]
mod time_series_disassociation;
#[path = "all/time_series_ingestion.rs"]
mod time_series_ingestion;
#[path = "all/update_asset.rs"]
mod update_asset;
#[path = "all/update_asset_model.rs"]
mod update_asset_model;
#[path = "all/update_gateway_capability_configuration.rs"]
mod update_gateway_capability_configuration;

// Declared at the very END of this file, after the sorted list, rather than in
// either of the two stretches above: both are where every concurrent iotsitewise
// change inserts a declaration of its own, and each insertion there collided with
// this one. Fold it into the sorted list once the churn settles.
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;

// Same reasoning as `application_lifecycle` above: declared after the sorted
// list, which is the stretch every concurrent iotsitewise change edits. Fold it
// into the list once the churn settles.
#[path = "all/list_interface_relationships.rs"]
mod list_interface_relationships;

// Same reasoning as `application_lifecycle` above: declared at the very END of
// this file rather than in either of the stretches above, both of which are where
// every concurrent iotsitewise change inserts a declaration of its own. Fold it
// into the sorted list once the churn settles.
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;

// Declared after the sorted list for the reason the note above gives: both
// stretches above are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_queries.rs"]
mod list_queries;

// Declared after the suite above, for the same reason it is: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/gateway_capability_configuration.rs"]
mod gateway_capability_configuration;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/asset_model_version_selection.rs"]
mod asset_model_version_selection;

// Declared last, for the same reason as the suites above it. Fold it into the
// sorted list once the churn settles.
#[path = "all/project_lifecycle.rs"]
mod project_lifecycle;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/describe_application_reports.rs"]
mod describe_application_reports;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_asset_properties.rs"]
mod list_asset_properties;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_tasks.rs"]
mod list_tasks;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold them into the sorted list once the churn settles.
#[path = "all/query_cancellation.rs"]
mod query_cancellation;
#[path = "all/gateway_delete.rs"]
mod gateway_delete;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/project_delete.rs"]
mod project_delete;

// Declared at the very end, after every suite above, for the reason the notes
// above give: both stretches higher up are where every concurrent iotsitewise
// change inserts a declaration of its own. Fold it into the sorted list once the
// churn settles.
#[path = "all/computation_model_data_binding_usages.rs"]
mod computation_model_data_binding_usages;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/describe_workspace_reads.rs"]
mod describe_workspace_reads;

// Declared at the very end, after every suite above, for the reason the notes
// higher up give: both stretches above are where every concurrent iotsitewise
// change inserts a declaration of its own. Fold it into the sorted list once the
// churn settles.
#[path = "all/application_delete.rs"]
mod application_delete;

// Declared beside the suite above, for the reason the notes higher up give: the
// sorted list and the end of this file are both where every concurrent
// iotsitewise change puts a declaration of its own. Fold it into the sorted list
// once the churn settles.
#[path = "all/create_project.rs"]
mod create_project;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/project_assets.rs"]
mod project_assets;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/workspace_delete.rs"]
mod workspace_delete;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_applications.rs"]
mod list_applications;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/project_asset_disassociation.rs"]
mod project_asset_disassociation;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_computation_models.rs"]
mod list_computation_models;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/pipeline_execution_cancellation.rs"]
mod pipeline_execution_cancellation;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/dataset_data_segments.rs"]
mod dataset_data_segments;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/asset_model_interface_relationship_delete.rs"]
mod asset_model_interface_relationship_delete;

// Declared at the very end, after every suite above, for the reason the notes
// higher up give: both stretches above are where every concurrent iotsitewise
// change inserts a declaration of its own. Fold it into the sorted list once the
// churn settles.
#[path = "all/list_portals.rs"]
mod list_portals;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/describe_dashboard_reports.rs"]
mod describe_dashboard_reports;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/describe_project_reads.rs"]
mod describe_project_reads;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/get_search_results.rs"]
mod get_search_results;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_project_assets_paging.rs"]
mod list_project_assets_paging;

// Declared at the very end, after every suite above, for the reason the notes
// higher up give: both stretches above are where every concurrent iotsitewise
// change inserts a declaration of its own. Fold it into the sorted list once the
// churn settles.
#[path = "all/list_projects.rs"]
mod list_projects;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold them into the sorted list once the churn settles.
#[path = "all/enrichment_job_cancellation.rs"]
mod enrichment_job_cancellation;
#[path = "all/update_gateway.rs"]
mod update_gateway;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/update_computation_model.rs"]
mod update_computation_model;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold them into the sorted list once the churn settles.
#[path = "all/dataset_data_segment_association.rs"]
mod dataset_data_segment_association;
#[path = "all/list_dataset_data_segments.rs"]
mod list_dataset_data_segments;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/time_series_binding_touches_the_asset.rs"]
mod time_series_binding_touches_the_asset;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretch of declarations ahead
// of it are both where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/update_asset_model_composite_model.rs"]
mod update_asset_model_composite_model;

// Declared at the very end, after every suite above, for the reason the notes
// higher up give: both stretches above are where every concurrent iotsitewise
// change inserts a declaration of its own. Fold it into the sorted list once the
// churn settles.
#[path = "all/list_access_policies.rs"]
mod list_access_policies;

// Declared last, for the same reason the suites above it are: both stretches
// higher up are where every concurrent iotsitewise change inserts a declaration
// of its own. Fold it into the sorted list once the churn settles.
#[path = "all/dataset_data_segment_disassociation.rs"]
mod dataset_data_segment_disassociation;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/shared_page_walk.rs"]
mod shared_page_walk;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_dataset_export_jobs.rs"]
mod list_dataset_export_jobs;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/list_dataset_data_segment_relationships.rs"]
mod list_dataset_data_segment_relationships;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/workspace_time_series.rs"]
mod workspace_time_series;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/pipeline_execution_client_token.rs"]
mod pipeline_execution_client_token;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/session_dataset_data_segment_ingestion.rs"]
mod session_dataset_data_segment_ingestion;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/resolve_to_reference.rs"]
mod resolve_to_reference;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/asset_model_interface_relationship_update.rs"]
mod asset_model_interface_relationship_update;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/dashboard_definition_json_form.rs"]
mod dashboard_definition_json_form;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/dataset_enrichment.rs"]
mod dataset_enrichment;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/aggregate_window_edge.rs"]
mod aggregate_window_edge;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/capture_import.rs"]
mod capture_import;

// Declared at the very END of this file, after every suite above, for the reason
// the notes higher up give: the sorted list and the stretches of declarations
// ahead of it are where every concurrent iotsitewise change inserts a
// declaration of its own. Fold it into the sorted list once the churn settles.
#[path = "all/property_formula_computation.rs"]
mod property_formula_computation;

