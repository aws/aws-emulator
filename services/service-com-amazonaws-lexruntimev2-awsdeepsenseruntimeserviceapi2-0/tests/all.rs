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
#[path = "all/always_reported_members.rs"]
mod always_reported_members;
#[path = "all/bot_addressing.rs"]
mod bot_addressing;
#[path = "all/bot_locale_dialog.rs"]
mod bot_locale_dialog;
#[path = "all/declared_slot_map.rs"]
mod declared_slot_map;
#[path = "all/dialog_turn_reporting.rs"]
mod dialog_turn_reporting;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/recognize_text_nlu.rs"]
mod recognize_text_nlu;
#[path = "all/recognize_utterance.rs"]
mod recognize_utterance;
#[path = "all/session_lifecycle.rs"]
mod session_lifecycle;
#[path = "all/shared_signing_name_claim.rs"]
mod shared_signing_name_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
