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
#[path = "all/attribute_mappings.rs"]
mod attribute_mappings;
#[path = "all/crl_delete_and_reference_refusals.rs"]
mod crl_delete_and_reference_refusals;
#[path = "all/crl_import_and_read.rs"]
mod crl_import_and_read;
#[path = "all/crl_listing.rs"]
mod crl_listing;
#[path = "all/crl_update_in_place.rs"]
mod crl_update_in_place;
#[path = "all/iso8601_timestamp_members.rs"]
mod iso8601_timestamp_members;
#[path = "all/native_create_session.rs"]
mod native_create_session;
#[path = "all/not_found_refusal_messages.rs"]
mod not_found_refusal_messages;
#[path = "all/notification_settings.rs"]
mod notification_settings;
#[path = "all/profile_disablement.rs"]
mod profile_disablement;
#[path = "all/profile_update_in_place.rs"]
mod profile_update_in_place;
#[path = "all/resource_tag_quota.rs"]
mod resource_tag_quota;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/trust_anchor_and_profile_lifecycle.rs"]
mod trust_anchor_and_profile_lifecycle;
#[path = "all/trust_anchor_enablement.rs"]
mod trust_anchor_enablement;
#[path = "all/trust_anchor_reenablement.rs"]
mod trust_anchor_reenablement;
#[path = "all/trust_anchor_source_replacement.rs"]
mod trust_anchor_source_replacement;
