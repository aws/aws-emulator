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
#[path = "all/a_removed_snapshot_resolves_nowhere.rs"]
mod a_removed_snapshot_resolves_nowhere;
#[path = "all/absent_required_members.rs"]
mod absent_required_members;
#[path = "all/changed_blocks_between_snapshots.rs"]
mod changed_blocks_between_snapshots;
#[path = "all/declared_block_inputs.rs"]
mod declared_block_inputs;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/minted_query_tokens.rs"]
mod minted_query_tokens;
#[path = "all/payload_checksum_integrity.rs"]
mod payload_checksum_integrity;
#[path = "all/read_path_requires_a_completed_snapshot.rs"]
mod read_path_requires_a_completed_snapshot;
#[path = "all/refusal_order.rs"]
mod refusal_order;
#[path = "all/response_bodies_and_refusals.rs"]
mod response_bodies_and_refusals;
#[path = "all/sealed_status_and_fixed_refusal_text.rs"]
mod sealed_status_and_fixed_refusal_text;
#[path = "all/snapshot_block_loop.rs"]
mod snapshot_block_loop;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
