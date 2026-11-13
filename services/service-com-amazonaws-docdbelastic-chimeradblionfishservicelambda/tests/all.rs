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
#[path = "all/cluster_delete_state_gate.rs"]
mod cluster_delete_state_gate;
#[path = "all/cluster_lifecycle.rs"]
mod cluster_lifecycle;
#[path = "all/cluster_snapshot_copy.rs"]
mod cluster_snapshot_copy;
#[path = "all/cluster_snapshot_listing.rs"]
mod cluster_snapshot_listing;
#[path = "all/cluster_stop.rs"]
mod cluster_stop;
#[path = "all/concurrent_cluster_modification.rs"]
mod concurrent_cluster_modification;
#[path = "all/create_cluster_service_rules.rs"]
mod create_cluster_service_rules;
#[path = "all/duplicate_name_conflict_wording.rs"]
mod duplicate_name_conflict_wording;
#[path = "all/error_resource_identity.rs"]
mod error_resource_identity;
#[path = "all/maintenance_and_restore.rs"]
mod maintenance_and_restore;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/pending_maintenance_reads.rs"]
mod pending_maintenance_reads;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_tag_writes.rs"]
mod resource_tag_writes;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/snapshot_listing_is_repeatable.rs"]
mod snapshot_listing_is_repeatable;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tagging_not_found_sentence.rs"]
mod tagging_not_found_sentence;
