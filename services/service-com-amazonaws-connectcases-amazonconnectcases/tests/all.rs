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
#[path = "all/case_audit_history.rs"]
mod case_audit_history;
#[path = "all/case_event_configuration.rs"]
mod case_event_configuration;
#[path = "all/case_lifecycle.rs"]
mod case_lifecycle;
#[path = "all/case_not_found.rs"]
mod case_not_found;
#[path = "all/case_search.rs"]
mod case_search;
#[path = "all/domain_deletion.rs"]
mod domain_deletion;
#[path = "all/domain_listing.rs"]
mod domain_listing;
#[path = "all/field_deletion.rs"]
mod field_deletion;
#[path = "all/field_listing.rs"]
mod field_listing;
#[path = "all/field_options.rs"]
mod field_options;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/layout_deletion.rs"]
mod layout_deletion;
#[path = "all/layout_lifecycle.rs"]
mod layout_lifecycle;
#[path = "all/not_found_refusal_wording.rs"]
mod not_found_refusal_wording;
#[path = "all/related_item_search.rs"]
mod related_item_search;
#[path = "all/related_items.rs"]
mod related_items;
#[path = "all/response_and_refusal_fidelity.rs"]
mod response_and_refusal_fidelity;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/system_fields.rs"]
mod system_fields;
#[path = "all/template_deletion.rs"]
mod template_deletion;
#[path = "all/template_listing.rs"]
mod template_listing;
#[path = "all/template_update.rs"]
mod template_update;
