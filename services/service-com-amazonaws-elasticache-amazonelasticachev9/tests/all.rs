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
#[path = "all/allowed_node_type_modifications.rs"]
mod allowed_node_type_modifications;
#[path = "all/cache_cluster_lifecycle.rs"]
mod cache_cluster_lifecycle;
#[path = "all/cache_engine_versions.rs"]
mod cache_engine_versions;
#[path = "all/cache_parameter_groups.rs"]
mod cache_parameter_groups;
#[path = "all/engine_default_parameters.rs"]
mod engine_default_parameters;
#[path = "all/events.rs"]
mod events;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/modeled_error_surfaces.rs"]
mod modeled_error_surfaces;
#[path = "all/rbac_users.rs"]
mod rbac_users;
#[path = "all/replication_group_lifecycle.rs"]
mod replication_group_lifecycle;
#[path = "all/reserved_cache_nodes.rs"]
mod reserved_cache_nodes;
#[path = "all/reserved_cache_nodes_offerings.rs"]
mod reserved_cache_nodes_offerings;
#[path = "all/serverless_cache_describe.rs"]
mod serverless_cache_describe;
#[path = "all/service_updates.rs"]
mod service_updates;
#[path = "all/serverless_cache_lifecycle.rs"]
mod serverless_cache_lifecycle;
#[path = "all/snapshot_copies.rs"]
mod snapshot_copies;
#[path = "all/snapshots.rs"]
mod snapshots;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_actions.rs"]
mod update_actions;
// Declared after the list above rather than inside it: suites land in this file
// concurrently, and every insertion into the sorted run collides with another
// branch's insertion at the same place. This is the snapshot-rule suite; the two
// drivers it calls through live in the shared `harness` module declared above.
#[path = "all/snapshot_lifecycle.rs"]
mod snapshot_lifecycle;
// Appended for the same reason: these two are the `DescribeUsers` describe-axis
// suite and the page walk that calls through its helpers.
#[path = "all/users.rs"]
mod users;
#[path = "all/describe_users.rs"]
mod describe_users;
// Appended for the same reason: the `DescribeUpdateActions` suite.
#[path = "all/describe_update_actions.rs"]
mod describe_update_actions;
// Appended for the same reason: the awsQuery error-envelope, subnet-resolution
// and identifier-validation suite.
#[path = "all/subnet_resolution_and_error_envelope.rs"]
mod subnet_resolution_and_error_envelope;
// Appended for the same reason: the awsQuery error-envelope suite and the
// `DeleteCacheParameterGroup` suite it shares a fix with.
#[path = "all/query_error_envelope.rs"]
mod query_error_envelope;
#[path = "all/cache_parameter_group_delete.rs"]
mod cache_parameter_group_delete;
// Appended for the same reason: the `DescribeCacheParameters` suite.
#[path = "all/describe_cache_parameters.rs"]
mod describe_cache_parameters;
// Appended for the same reason: the tagging suite (`AddTagsToResource`,
// `ListTagsForResource`, `RemoveTagsFromResource`).
#[path = "all/resource_tags.rs"]
mod resource_tags;
// Appended for the same reason: the `DeleteServerlessCache` suite.
#[path = "all/serverless_cache_delete.rs"]
mod serverless_cache_delete;
// Appended for the same reason: the maintenance-window, cluster-state-gate,
// cache-security-group-closure and cluster-message suite.
#[path = "all/cache_cluster_gates_and_messages.rs"]
mod cache_cluster_gates_and_messages;
// Appended for the same reason: the `TestFailover` suite.
#[path = "all/test_failover.rs"]
mod test_failover;
// Appended for the same reason: the RBAC access-string, `DeleteUser` and error-envelope
// suite.
#[path = "all/rbac_user_access_string_and_deletion.rs"]
mod rbac_user_access_string_and_deletion;
// Appended for the same reason: the `CacheNodeType` catalogue-validation suite.
#[path = "all/cache_node_type_validation.rs"]
mod cache_node_type_validation;
// Appended for the same reason: the `ModifyUserGroup` membership-change suite.
#[path = "all/user_group_membership.rs"]
mod user_group_membership;
// Appended for the same reason: the `DeleteSnapshot` suite.
#[path = "all/snapshot_delete.rs"]
mod snapshot_delete;
// Appended for the same reason: the `ModifyReplicationGroup` settings-change
#[path = "all/modify_replication_group.rs"]
mod modify_replication_group;
// Appended for the same reason: the read-only catalog publication suite (engine
// versions, default parameter groups, offerings, service updates).
#[path = "all/catalog_publication.rs"]
mod catalog_publication;
// Appended for the same reason: the `CreateGlobalReplicationGroup` suite.
#[path = "all/global_replication_group_create.rs"]
mod global_replication_group_create;
// Appended for the same reason: the `ModifyCacheParameterGroup` suite.
#[path = "all/modify_cache_parameter_group.rs"]
mod modify_cache_parameter_group;
// Appended for the same reason: the `ModifyUser` suite.
#[path = "all/modify_user.rs"]
mod modify_user;
// Appended for the same reason: the `ResetCacheParameterGroup` suite, which
// drives the writer through the `ModifyCacheParameterGroup` suite's helpers.
#[path = "all/reset_cache_parameter_group.rs"]
mod reset_cache_parameter_group;
// Appended for the same reason: the `DeleteUser` suite about the user the service itself
// owns.
#[path = "all/delete_user_built_in_user.rs"]
mod delete_user_built_in_user;
// Appended for the same reason: the refusal-wording suite (the identifier form of a
// parameter-group name, the parameters a customer may not change, the tag rules and a
// user-group id already taken).
#[path = "all/refusal_message_fidelity.rs"]
mod refusal_message_fidelity;
// Appended for the same reason: the suite about a request that states how a user
// authenticates twice, which drives both writers through the `ModifyUser` suite's reader.
#[path = "all/user_authentication_mode_conflict.rs"]
mod user_authentication_mode_conflict;
// Appended for the same reason: the access-string refusal-wording suite, which drives
// both writers of a rule list through the `ModifyUser` suite's reader.
#[path = "all/access_string_refusal_wording.rs"]
mod access_string_refusal_wording;
// Appended for the same reason: the suite that continues where the wording suite stops —
// the stored-length bound a rule list is refused at, what the refusal message does NOT
// say, and what the user takes after one.
#[path = "all/access_string_refusal_message.rs"]
mod access_string_refusal_message;
// Appended for the same reason: the suite about the sentence a user group without the
// service's own user is refused with, at both writers of a membership.
#[path = "all/default_user_required_wording.rs"]
mod default_user_required_wording;
// Appended for the same reason: the `ModifyServerlessCache` suite.
#[path = "all/serverless_cache_modify.rs"]
mod serverless_cache_modify;
// Appended for the same reason: the read-and-validate fidelity suite (the engine
// filter of the version catalog, the order the event log is reported in, the key
// an encrypted resource stores, a blank description, the maintenance-window rule
// and the refusal templates).
#[path = "all/read_validate_fidelity.rs"]
mod read_validate_fidelity;
// Appended for the same reason: the `StartMigration` suite.
#[path = "all/start_migration.rs"]
mod start_migration;
// Appended for the same reason: the suite about WHICH rule a refusal states and
// in whose words (the family vocabulary, the parameter-group name grammar, the
// access-control and serverless engine capabilities, the ARN structure, the
// snapshot creator's cluster resolution and the statuses a user-group delete
// admits).
#[path = "all/refusal_rule_statements.rs"]
mod refusal_rule_statements;
// Appended for the same reason: the serverless cache snapshot suite
// (`CreateServerlessCacheSnapshot`, `DescribeServerlessCacheSnapshots` and the
// final snapshot a `DeleteServerlessCache` takes).
#[path = "all/serverless_cache_snapshots.rs"]
mod serverless_cache_snapshots;
// Appended for the same reason: the `DeleteCacheSubnetGroup` suite.
#[path = "all/cache_subnet_group_delete.rs"]
mod cache_subnet_group_delete;
// Appended for the same reason: the suite about the sentence a change that needs an
// ACTIVE user group is refused with while the group carries another status.
#[path = "all/user_group_state_refusal.rs"]
mod user_group_state_refusal;
// Appended for the same reason: the suite about the TEMPLATES three refusals are worded
// with — the two the parameter-group not-found fault has, the one `ModifyReplicationGroup`
// has of its own, and the parameter value a published set does not cover.
#[path = "all/refusal_message_templates.rs"]
mod refusal_message_templates;
// Appended for the same reason: the suite about WHICH of its two refusals an access string
// gets, which drives both writers of a rule list through the `ModifyUser` suite's reader.
#[path = "all/access_string_category_refusal.rs"]
mod access_string_category_refusal;
// Appended for the same reason: the suite about WHICH refusal an operation answers when one
// request is both malformed and names a resource this account does not hold.
#[path = "all/identifier_length_precedence.rs"]
mod identifier_length_precedence;
// Appended for the same reason: the suite about the ONE source every ElastiCache placement
// resolves a subnet against (Amazon EC2, the service that owns subnets) and the sentence a
// placement that names no cache subnet group of this account is refused with.
#[path = "all/subnet_placement_resolves_against_ec2.rs"]
mod subnet_placement_resolves_against_ec2;
// Appended for the same reason: the `ModifyCacheSubnetGroup` suite, the shard-configuration
// suite (`ModifyReplicationGroupShardConfiguration` and the replica count a cluster mode
// enabled create defaults to), the `CompleteMigration` suite and the suite about the
// spelling of the lifecycle a client polls on.
#[path = "all/cache_subnet_group_modify.rs"]
mod cache_subnet_group_modify;
#[path = "all/shard_configuration.rs"]
mod shard_configuration;
#[path = "all/migration_completion.rs"]
mod migration_completion;
#[path = "all/serverless_lifecycle_spelling.rs"]
mod serverless_lifecycle_spelling;
// Appended for the same reason: the suite about the ONE source every ElastiCache operation
// that attaches a resource to VPC security groups resolves those identifiers against
// (Amazon EC2, the service that owns security groups).
#[path = "all/security_groups_resolve_against_ec2.rs"]
mod security_groups_resolve_against_ec2;
// Appended for the same reason: the suite about the engine versions a create or an upgrade
// may name — the ones `DescribeCacheEngineVersions` offers.
#[path = "all/engine_version_must_be_offered.rs"]
mod engine_version_must_be_offered;
// Appended for the same reason: the suite about the ENGINE a cache parameter group a request
// names has to belong to.
#[path = "all/parameter_group_engine_must_match.rs"]
mod parameter_group_engine_must_match;
// Appended for the same reason: the suite about the two sentences an `AuthToken` is refused
// with.
#[path = "all/auth_token_refusal_wording.rs"]
mod auth_token_refusal_wording;
// Appended for the same reason: the `ExportServerlessCacheSnapshot` suite, which reads the
// object the export wrote back through Amazon S3's own operations.
#[path = "all/serverless_cache_snapshot_export.rs"]
mod serverless_cache_snapshot_export;
// Appended for the same reason: the suite about the INVENTORY the read-only
// reference catalogs publish (engine versions and their families, the default
// parameter groups, the offering grid, both kinds of service update) and about
// which of the two parameter lists a page bounds.
#[path = "all/reference_catalog_inventory.rs"]
mod reference_catalog_inventory;
// Appended for the same reason: the cluster event-notification suite, which reads the
// notifications a cluster's events are delivered as back through Amazon SNS, the service
// that owns topics and their deliveries.
#[path = "all/cluster_event_notifications.rs"]
mod cluster_event_notifications;
// Appended for the same reason: the `TestMigration` suite, the Global-datastore SECONDARY
// suite, the suite about the two changes ElastiCache reports as outstanding after it has
// accepted them, and the suite about the two refusals whose whole sentence is fixed text.
#[path = "all/test_migration.rs"]
mod test_migration;
#[path = "all/global_replication_group_secondary.rs"]
mod global_replication_group_secondary;
#[path = "all/deferred_change_reporting.rs"]
mod deferred_change_reporting;
#[path = "all/fixed_refusal_sentences.rs"]
mod fixed_refusal_sentences;
// Appended for the same reason: the DATA-PLANE suite (the address a cache publishes, the
// RESP session a client opens on it, and what that session may do) and the suite about the
// three membership rules — which user name is unique and where, which replication-group
// shapes a create takes, and which member a delete may take.
#[path = "all/cache_data_plane.rs"]
mod cache_data_plane;
#[path = "all/cache_membership_rules.rs"]
mod cache_membership_rules;
// Appended for the same reason: the suite about the network types a cache subnet group and
// its subnets publish, the suite about the engine-version catalog's two description strings,
// and the suite about WHICH refusal a parameter this service cannot take gets.
#[path = "all/cache_subnet_group_network_types.rs"]
mod cache_subnet_group_network_types;
#[path = "all/engine_catalog_descriptions.rs"]
mod engine_catalog_descriptions;
#[path = "all/parameter_refusal_classification.rs"]
mod parameter_refusal_classification;
// Appended for the same reason: the suite about the refusal a contradictory serverless
// `CacheUsageLimits` envelope is owed at both operations that would store one, over the
// network a client stands up through Amazon EC2 first.
#[path = "all/serverless_usage_limit_envelope.rs"]
mod serverless_usage_limit_envelope;
// Appended for the same reason: the suite about the sentence a serverless-cache-snapshot
// lookup that resolves to nothing is refused with, at both operations that resolve one by
// name.
#[path = "all/serverless_cache_snapshot_not_found_sentence.rs"]
mod serverless_cache_snapshot_not_found_sentence;
