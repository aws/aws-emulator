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
#[path = "all/bot_aliases.rs"]
mod bot_aliases;
#[path = "all/bot_lifecycle.rs"]
mod bot_lifecycle;
#[path = "all/bot_locale.rs"]
mod bot_locale;
#[path = "all/bot_versions.rs"]
mod bot_versions;
#[path = "all/exports.rs"]
mod exports;
#[path = "all/intents_and_builds.rs"]
mod intents_and_builds;
#[path = "all/locale_build_outcome.rs"]
mod locale_build_outcome;
#[path = "all/locale_build_slot_priorities.rs"]
mod locale_build_slot_priorities;
#[path = "all/locale_deletion.rs"]
mod locale_deletion;
#[path = "all/name_already_exists.rs"]
mod name_already_exists;
#[path = "all/optional_collections.rs"]
mod optional_collections;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/response_media_type.rs"]
mod response_media_type;
#[path = "all/service_claim.rs"]
mod service_claim;
#[path = "all/slot_constraints.rs"]
mod slot_constraints;
#[path = "all/slot_prompt_defaults.rs"]
mod slot_prompt_defaults;
#[path = "all/slot_type.rs"]
mod slot_type;
#[path = "all/slot_type_listing.rs"]
mod slot_type_listing;
#[path = "all/slots.rs"]
mod slots;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
