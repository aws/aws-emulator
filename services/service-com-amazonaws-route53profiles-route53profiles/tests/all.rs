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
#[path = "all/error_reporting.rs"]
mod error_reporting;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/name_pattern.rs"]
mod name_pattern;
#[path = "all/profile_lifecycle.rs"]
mod profile_lifecycle;
#[path = "all/profile_status_messages.rs"]
mod profile_status_messages;
#[path = "all/resource_association_lifecycle.rs"]
mod resource_association_lifecycle;
#[path = "all/resource_association_listing.rs"]
mod resource_association_listing;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
