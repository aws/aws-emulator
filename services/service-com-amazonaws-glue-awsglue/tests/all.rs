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
#[path = "all/asset_attachments.rs"]
mod asset_attachments;
#[path = "all/asset_deletion.rs"]
mod asset_deletion;
#[path = "all/asset_types.rs"]
mod asset_types;
#[path = "all/assets.rs"]
mod assets;
#[path = "all/batch_delete_connection.rs"]
mod batch_delete_connection;
#[path = "all/batch_delete_partition.rs"]
mod batch_delete_partition;
#[path = "all/batch_get_crawlers.rs"]
mod batch_get_crawlers;
#[path = "all/batch_get_custom_entity_types.rs"]
mod batch_get_custom_entity_types;
#[path = "all/batch_get_jobs.rs"]
mod batch_get_jobs;
#[path = "all/batch_get_partition.rs"]
mod batch_get_partition;
#[path = "all/batch_get_triggers.rs"]
mod batch_get_triggers;
#[path = "all/batch_stop_job_run.rs"]
mod batch_stop_job_run;
#[path = "all/batch_table_version_deletion.rs"]
mod batch_table_version_deletion;
#[path = "all/batch_update_partition.rs"]
mod batch_update_partition;
#[path = "all/blueprint_deletion.rs"]
mod blueprint_deletion;
#[path = "all/blueprint_lifecycle_miss.rs"]
mod blueprint_lifecycle_miss;
#[path = "all/blueprint_listing.rs"]
mod blueprint_listing;
#[path = "all/blueprint_run_listing.rs"]
mod blueprint_run_listing;
#[path = "all/blueprint_runs.rs"]
mod blueprint_runs;
#[path = "all/blueprints.rs"]
mod blueprints;
#[path = "all/catalog_encryption.rs"]
mod catalog_encryption;
#[path = "all/catalog_identity.rs"]
mod catalog_identity;
#[path = "all/catalog_import_status.rs"]
mod catalog_import_status;
#[path = "all/catalog_input_refusals.rs"]
mod catalog_input_refusals;
#[path = "all/catalog_mutations.rs"]
mod catalog_mutations;
#[path = "all/catalog_projections.rs"]
mod catalog_projections;
#[path = "all/catalog_resource_deletion.rs"]
mod catalog_resource_deletion;
#[path = "all/catalog_resources.rs"]
mod catalog_resources;
#[path = "all/catalog_structure.rs"]
mod catalog_structure;
#[path = "all/catalogs.rs"]
mod catalogs;
#[path = "all/classifiers.rs"]
mod classifiers;
#[path = "all/column_statistics_computation.rs"]
mod column_statistics_computation;
#[path = "all/column_statistics_persistence.rs"]
mod column_statistics_persistence;
#[path = "all/column_statistics_task_conflicts.rs"]
mod column_statistics_task_conflicts;
#[path = "all/column_statistics_task_run_history.rs"]
mod column_statistics_task_run_history;
#[path = "all/column_statistics_task_run_index.rs"]
mod column_statistics_task_run_index;
#[path = "all/column_statistics_task_runs.rs"]
mod column_statistics_task_runs;
#[path = "all/column_statistics_task_settings.rs"]
mod column_statistics_task_settings;
#[path = "all/column_statistics_task_settings_deletion.rs"]
mod column_statistics_task_settings_deletion;
#[path = "all/connection_deletion.rs"]
mod connection_deletion;
#[path = "all/connection_listing.rs"]
mod connection_listing;
#[path = "all/connection_not_found.rs"]
mod connection_not_found;
#[path = "all/connection_updates.rs"]
mod connection_updates;
#[path = "all/constraint_refusal_class.rs"]
mod constraint_refusal_class;
#[path = "all/crawler_deletion.rs"]
mod crawler_deletion;
#[path = "all/crawler_deletion_scope.rs"]
mod crawler_deletion_scope;
#[path = "all/crawler_event_queue.rs"]
mod crawler_event_queue;
#[path = "all/crawler_listing.rs"]
mod crawler_listing;
#[path = "all/crawler_metrics.rs"]
mod crawler_metrics;
#[path = "all/crawler_role_trust.rs"]
mod crawler_role_trust;
#[path = "all/crawler_runs.rs"]
mod crawler_runs;
#[path = "all/crawler_schedule_suspension.rs"]
mod crawler_schedule_suspension;
#[path = "all/crawler_schedules.rs"]
mod crawler_schedules;
#[path = "all/crawler_updates.rs"]
mod crawler_updates;
#[path = "all/crawlers.rs"]
mod crawlers;
#[path = "all/create_partition.rs"]
mod create_partition;
#[path = "all/custom_entity_type_deletion.rs"]
mod custom_entity_type_deletion;
#[path = "all/custom_entity_type_listing.rs"]
mod custom_entity_type_listing;
#[path = "all/custom_entity_types.rs"]
mod custom_entity_types;
#[path = "all/dashboard_url.rs"]
mod dashboard_url;
#[path = "all/data_catalog.rs"]
mod data_catalog;
#[path = "all/data_catalog_encryption_settings.rs"]
mod data_catalog_encryption_settings;
#[path = "all/data_catalog_export_configuration.rs"]
mod data_catalog_export_configuration;
#[path = "all/data_quality_rule_recommendation_runs.rs"]
mod data_quality_rule_recommendation_runs;
#[path = "all/data_quality_results.rs"]
mod data_quality_results;
#[path = "all/data_quality_ruleset_deletion.rs"]
mod data_quality_ruleset_deletion;
#[path = "all/data_quality_ruleset_evaluation_runs.rs"]
mod data_quality_ruleset_evaluation_runs;
#[path = "all/data_quality_ruleset_identity.rs"]
mod data_quality_ruleset_identity;
#[path = "all/data_quality_ruleset_listing.rs"]
mod data_quality_ruleset_listing;
#[path = "all/data_quality_ruleset_parsing.rs"]
mod data_quality_ruleset_parsing;
#[path = "all/data_quality_ruleset_updates.rs"]
mod data_quality_ruleset_updates;
#[path = "all/data_quality_rulesets.rs"]
mod data_quality_rulesets;
#[path = "all/database_listing.rs"]
mod database_listing;
#[path = "all/delete_partition.rs"]
mod delete_partition;
#[path = "all/derived_resource_identities.rs"]
mod derived_resource_identities;
#[path = "all/duplicate_create_refusals.rs"]
mod duplicate_create_refusals;
#[path = "all/error_classification.rs"]
mod error_classification;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/etl_connections.rs"]
mod etl_connections;
#[path = "all/etl_jobs_and_connections.rs"]
mod etl_jobs_and_connections;
#[path = "all/event_batching_conditions.rs"]
mod event_batching_conditions;
#[path = "all/event_trigger_batch_firing.rs"]
mod event_trigger_batch_firing;
#[path = "all/execution_role_admission.rs"]
mod execution_role_admission;
#[path = "all/federated_catalogs.rs"]
mod federated_catalogs;
#[path = "all/form_type_deletion.rs"]
mod form_type_deletion;
#[path = "all/form_types.rs"]
mod form_types;
#[path = "all/get_mapping.rs"]
mod get_mapping;
#[path = "all/get_plan.rs"]
mod get_plan;
#[path = "all/get_table_entity_not_found.rs"]
mod get_table_entity_not_found;
#[path = "all/iceberg_tables.rs"]
mod iceberg_tables;
#[path = "all/identity_center_configuration.rs"]
mod identity_center_configuration;
#[path = "all/identity_center_instance.rs"]
mod identity_center_instance;
#[path = "all/glossaries.rs"]
mod glossaries;
#[path = "all/glossary_deletion.rs"]
mod glossary_deletion;
#[path = "all/glossary_term_association.rs"]
mod glossary_term_association;
#[path = "all/glossary_term_deletion.rs"]
mod glossary_term_deletion;
#[path = "all/glossary_term_listing.rs"]
mod glossary_term_listing;
#[path = "all/glossary_terms.rs"]
mod glossary_terms;
#[path = "all/glossary_updates.rs"]
mod glossary_updates;
#[path = "all/integration_property_validation.rs"]
mod integration_property_validation;
#[path = "all/integration_resource_properties.rs"]
mod integration_resource_properties;
#[path = "all/integration_resource_property_delete_misses.rs"]
mod integration_resource_property_delete_misses;
#[path = "all/integration_table_properties.rs"]
mod integration_table_properties;
#[path = "all/job_bookmarks.rs"]
mod job_bookmarks;
#[path = "all/job_capacity_refusal.rs"]
mod job_capacity_refusal;
#[path = "all/job_definition_listing.rs"]
mod job_definition_listing;
#[path = "all/job_deletion.rs"]
mod job_deletion;
#[path = "all/job_glue_version_pattern.rs"]
mod job_glue_version_pattern;
#[path = "all/job_listing.rs"]
mod job_listing;
#[path = "all/job_not_found_message.rs"]
mod job_not_found_message;
#[path = "all/job_regions.rs"]
mod job_regions;
#[path = "all/job_run_execution_role.rs"]
mod job_run_execution_role;
#[path = "all/job_run_listing.rs"]
mod job_run_listing;
#[path = "all/job_run_miss_message.rs"]
mod job_run_miss_message;
#[path = "all/job_run_rest.rs"]
mod job_run_rest;
#[path = "all/job_run_retries.rs"]
mod job_run_retries;
#[path = "all/job_run_state_change_events.rs"]
mod job_run_state_change_events;
#[path = "all/job_runs.rs"]
mod job_runs;
#[path = "all/job_script_bounds.rs"]
mod job_script_bounds;
#[path = "all/job_script_execution.rs"]
mod job_script_execution;
#[path = "all/job_script_execution_wait.rs"]
mod job_script_execution_wait;
#[path = "all/job_updates.rs"]
mod job_updates;
#[path = "all/jobs.rs"]
mod jobs;
#[path = "all/lake_formation_authorization.rs"]
mod lake_formation_authorization;
#[path = "all/lake_formation_database_reads.rs"]
mod lake_formation_database_reads;
#[path = "all/metadata_catalog_reads.rs"]
mod metadata_catalog_reads;
#[path = "all/ml_task_run_reads.rs"]
mod ml_task_run_reads;
#[path = "all/ml_task_run_timeout.rs"]
mod ml_task_run_timeout;
#[path = "all/ml_task_runs.rs"]
mod ml_task_runs;
#[path = "all/ml_transform_deletion.rs"]
mod ml_transform_deletion;
#[path = "all/ml_transform_reads.rs"]
mod ml_transform_reads;
#[path = "all/ml_transform_updates.rs"]
mod ml_transform_updates;
#[path = "all/ml_transforms.rs"]
mod ml_transforms;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/mutations_are_applied.rs"]
mod mutations_are_applied;
#[path = "all/owed_status_materialisation.rs"]
mod owed_status_materialisation;
#[path = "all/partition_index_registration.rs"]
mod partition_index_registration;
#[path = "all/partition_indexes.rs"]
mod partition_indexes;
#[path = "all/partition_listing.rs"]
mod partition_listing;
#[path = "all/partitions.rs"]
mod partitions;
#[path = "all/refusal_classification.rs"]
mod refusal_classification;
#[path = "all/registries.rs"]
mod registries;
#[path = "all/registry_listing.rs"]
mod registry_listing;
#[path = "all/reported_members.rs"]
mod reported_members;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/response_members.rs"]
mod response_members;
#[path = "all/response_request_id.rs"]
mod response_request_id;
#[path = "all/schema_registry.rs"]
mod schema_registry;
#[path = "all/schema_version_by_definition.rs"]
mod schema_version_by_definition;
#[path = "all/schema_version_compatibility.rs"]
mod schema_version_compatibility;
#[path = "all/schema_version_deletion.rs"]
mod schema_version_deletion;
#[path = "all/schema_version_registration.rs"]
mod schema_version_registration;
#[path = "all/search_tables.rs"]
mod search_tables;
#[path = "all/security_configurations.rs"]
mod security_configurations;
#[path = "all/service_filled_read_members.rs"]
mod service_filled_read_members;
#[path = "all/session_deletion.rs"]
mod session_deletion;
#[path = "all/session_tags.rs"]
mod session_tags;
#[path = "all/sessions.rs"]
mod sessions;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/statement_listing.rs"]
mod statement_listing;
#[path = "all/statements.rs"]
mod statements;
#[path = "all/store_capacity.rs"]
mod store_capacity;
#[path = "all/table_optimizers.rs"]
mod table_optimizers;
#[path = "all/tag_projection_account_scope.rs"]
mod tag_projection_account_scope;
#[path = "all/table_update_concurrency.rs"]
mod table_update_concurrency;
#[path = "all/table_version_deletion.rs"]
mod table_version_deletion;
#[path = "all/table_version_listing.rs"]
mod table_version_listing;
#[path = "all/table_versions.rs"]
mod table_versions;
#[path = "all/trigger_action_runs.rs"]
mod trigger_action_runs;
#[path = "all/trigger_definition_reads.rs"]
mod trigger_definition_reads;
#[path = "all/trigger_deletion.rs"]
mod trigger_deletion;
#[path = "all/trigger_listing.rs"]
mod trigger_listing;
#[path = "all/trigger_on_demand_lifecycle.rs"]
mod trigger_on_demand_lifecycle;
#[path = "all/trigger_starts.rs"]
mod trigger_starts;
#[path = "all/trigger_stops.rs"]
mod trigger_stops;
#[path = "all/trigger_updates.rs"]
mod trigger_updates;
#[path = "all/triggers.rs"]
mod triggers;
#[path = "all/unfiltered_metadata.rs"]
mod unfiltered_metadata;
#[path = "all/unfiltered_table_metadata.rs"]
mod unfiltered_table_metadata;
#[path = "all/update_partition.rs"]
mod update_partition;
#[path = "all/usage_profiles.rs"]
mod usage_profiles;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/workflow_batch_reads.rs"]
mod workflow_batch_reads;
#[path = "all/workflow_graph.rs"]
mod workflow_graph;
#[path = "all/workflow_listing.rs"]
mod workflow_listing;
#[path = "all/workflow_run_actions.rs"]
mod workflow_run_actions;
#[path = "all/workflow_run_history_absence.rs"]
mod workflow_run_history_absence;
#[path = "all/workflow_run_listing.rs"]
mod workflow_run_listing;
#[path = "all/workflow_run_properties.rs"]
mod workflow_run_properties;
#[path = "all/workflow_runs.rs"]
mod workflow_runs;
#[path = "all/workflow_updates.rs"]
mod workflow_updates;
#[path = "all/workflows.rs"]
mod workflows;
#[path = "all/session_listing.rs"]
mod session_listing;
