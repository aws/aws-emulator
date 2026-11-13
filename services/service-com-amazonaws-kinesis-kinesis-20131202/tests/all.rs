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
#[path = "all/account_limits.rs"]
mod account_limits;
#[path = "all/account_settings.rs"]
mod account_settings;
#[path = "all/account_settings_reporting.rs"]
mod account_settings_reporting;
#[path = "all/batch_write_failure_count.rs"]
mod batch_write_failure_count;
#[path = "all/batch_write_validation.rs"]
mod batch_write_validation;
#[path = "all/capacity_mode.rs"]
mod capacity_mode;
#[path = "all/channel_configuration_updates.rs"]
mod channel_configuration_updates;
#[path = "all/channel_dependents.rs"]
mod channel_dependents;
#[path = "all/channel_key_authorization.rs"]
mod channel_key_authorization;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/channel_removal.rs"]
mod channel_removal;
#[path = "all/consumer_deregistration.rs"]
mod consumer_deregistration;
#[path = "all/consumer_listing.rs"]
mod consumer_listing;
#[path = "all/constraint_refusal_wording.rs"]
mod constraint_refusal_wording;
#[path = "all/data_records.rs"]
mod data_records;
#[path = "all/encrypted_data_plane.rs"]
mod encrypted_data_plane;
#[path = "all/encrypted_records.rs"]
mod encrypted_records;
#[path = "all/encryption_key_resolution.rs"]
mod encryption_key_resolution;
#[path = "all/enhanced_monitoring.rs"]
mod enhanced_monitoring;
#[path = "all/handler_refusal_wording.rs"]
mod handler_refusal_wording;
#[path = "all/manual_resharding.rs"]
mod manual_resharding;
#[path = "all/max_record_size.rs"]
mod max_record_size;
#[path = "all/merge_adjacency.rs"]
mod merge_adjacency;
#[path = "all/pagination_token_classification.rs"]
mod pagination_token_classification;
#[path = "all/record_distribution_strategy.rs"]
mod record_distribution_strategy;
#[path = "all/record_flow.rs"]
mod record_flow;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_policy_absence.rs"]
mod resource_policy_absence;
#[path = "all/resource_policy_stream_state.rs"]
mod resource_policy_stream_state;
#[path = "all/resource_tag_listing.rs"]
mod resource_tag_listing;
#[path = "all/resource_tag_removal.rs"]
mod resource_tag_removal;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/retention_period.rs"]
mod retention_period;
#[path = "all/shard_filter_timestamp.rs"]
mod shard_filter_timestamp;
#[path = "all/shard_iterators.rs"]
mod shard_iterators;
#[path = "all/shard_listing.rs"]
mod shard_listing;
#[path = "all/shard_quota.rs"]
mod shard_quota;
#[path = "all/shard_scaling.rs"]
mod shard_scaling;
#[path = "all/shard_write_quota.rs"]
mod shard_write_quota;
#[path = "all/split_shard_lineage.rs"]
mod split_shard_lineage;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/stream_consumers.rs"]
mod stream_consumers;
#[path = "all/stream_encryption.rs"]
mod stream_encryption;
#[path = "all/stream_lifecycle.rs"]
mod stream_lifecycle;
#[path = "all/stream_not_active_refusal.rs"]
mod stream_not_active_refusal;
#[path = "all/stream_removal.rs"]
mod stream_removal;
#[path = "all/stream_tag_removal.rs"]
mod stream_tag_removal;
#[path = "all/stream_tags.rs"]
mod stream_tags;
#[path = "all/success_response_members.rs"]
mod success_response_members;
#[path = "all/warm_throughput.rs"]
mod warm_throughput;
