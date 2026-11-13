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
#[path = "all/address_list_deletion.rs"]
mod address_list_deletion;
#[path = "all/address_list_import_jobs.rs"]
mod address_list_import_jobs;
#[path = "all/address_list_lifecycle.rs"]
mod address_list_lifecycle;
#[path = "all/address_list_members.rs"]
mod address_list_members;
#[path = "all/address_list_resolution.rs"]
mod address_list_resolution;
#[path = "all/archive_lifecycle.rs"]
mod archive_lifecycle;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
