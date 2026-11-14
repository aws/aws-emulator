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
#[path = "all/blueprint_catalog.rs"]
mod blueprint_catalog;
#[path = "all/blueprint_retrieval.rs"]
mod blueprint_retrieval;
#[path = "all/error_messages.rs"]
mod error_messages;
#[path = "all/pattern_messages.rs"]
mod pattern_messages;
#[path = "all/pipeline_lifecycle.rs"]
mod pipeline_lifecycle;
#[path = "all/pipeline_not_found_wording.rs"]
mod pipeline_not_found_wording;
#[path = "all/pipeline_refusal_wording.rs"]
mod pipeline_refusal_wording;
#[path = "all/pipeline_status_settling.rs"]
mod pipeline_status_settling;
#[path = "all/pipeline_update.rs"]
mod pipeline_update;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
