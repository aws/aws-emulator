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
#[path = "all/already_exists_conflicts.rs"]
mod already_exists_conflicts;
#[path = "all/db_cluster_lifecycle.rs"]
mod db_cluster_lifecycle;
#[path = "all/db_cluster_marketplace_subscription.rs"]
mod db_cluster_marketplace_subscription;
#[path = "all/db_instance_lifecycle.rs"]
mod db_instance_lifecycle;
#[path = "all/db_instance_reboot.rs"]
mod db_instance_reboot;
#[path = "all/db_parameter_group.rs"]
mod db_parameter_group;
#[path = "all/db_parameter_group_catalog.rs"]
mod db_parameter_group_catalog;
#[path = "all/db_parameter_group_listing.rs"]
mod db_parameter_group_listing;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/parameters_union_roundtrip.rs"]
mod parameters_union_roundtrip;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
