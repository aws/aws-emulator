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
#[path = "all/business_public_key_material.rs"]
mod business_public_key_material;
#[path = "all/console_signup_admission.rs"]
mod console_signup_admission;
#[path = "all/conversion_events.rs"]
mod conversion_events;
#[path = "all/cross_account_pass_role.rs"]
mod cross_account_pass_role;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/meta_resource_lookups.rs"]
mod meta_resource_lookups;
#[path = "all/not_found_message.rs"]
mod not_found_message;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/setup_enabled_listing.rs"]
mod setup_enabled_listing;
#[path = "all/signup_admission_order.rs"]
mod signup_admission_order;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/validation_message_rendering.rs"]
mod validation_message_rendering;
#[path = "all/whatsapp_business_account.rs"]
mod whatsapp_business_account;
