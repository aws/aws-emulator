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
#[path = "all/access_type_coip_pool_pairing.rs"]
mod access_type_coip_pool_pairing;
#[path = "all/arn_partition_admission.rs"]
mod arn_partition_admission;
#[path = "all/arn_partitions.rs"]
mod arn_partitions;
#[path = "all/arn_patterns.rs"]
mod arn_patterns;
#[path = "all/control_plane_inventory.rs"]
mod control_plane_inventory;
#[path = "all/control_plane_partition_agreement.rs"]
mod control_plane_partition_agreement;
#[path = "all/endpoint_commitment_conflict.rs"]
mod endpoint_commitment_conflict;
#[path = "all/endpoint_cursor_scope.rs"]
mod endpoint_cursor_scope;
#[path = "all/endpoint_lifecycle.rs"]
mod endpoint_lifecycle;
#[path = "all/endpoint_paging.rs"]
mod endpoint_paging;
#[path = "all/endpoint_teardown.rs"]
mod endpoint_teardown;
#[path = "all/listing_harness.rs"]
mod listing_harness;
#[path = "all/not_found_messages.rs"]
mod not_found_messages;
#[path = "all/one_listing_rule.rs"]
mod one_listing_rule;
#[path = "all/one_ordered_capacity_rule.rs"]
mod one_ordered_capacity_rule;
#[path = "all/one_outpost_resolution_rule.rs"]
mod one_outpost_resolution_rule;
#[path = "all/outpost_cursor_scope.rs"]
mod outpost_cursor_scope;
#[path = "all/outpost_departure.rs"]
mod outpost_departure;
#[path = "all/region_scoping.rs"]
mod region_scoping;
#[path = "all/shadowed_outpost_authority.rs"]
mod shadowed_outpost_authority;
#[path = "all/shared_endpoint_listing.rs"]
mod shared_endpoint_listing;
#[path = "all/shared_test_harness.rs"]
mod shared_test_harness;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/subnet_vpc_region_scope.rs"]
mod subnet_vpc_region_scope;
#[path = "all/unprovisioned_account.rs"]
mod unprovisioned_account;
