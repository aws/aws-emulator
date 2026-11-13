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
#[path = "all/automated_discovery_configuration.rs"]
mod automated_discovery_configuration;
#[path = "all/custom_data_identifier_patterns.rs"]
mod custom_data_identifier_patterns;
#[path = "all/custom_data_identifier_response_shape.rs"]
mod custom_data_identifier_response_shape;
#[path = "all/findings_publication_configuration.rs"]
mod findings_publication_configuration;
#[path = "all/macie_session_and_custom_data_identifiers.rs"]
mod macie_session_and_custom_data_identifiers;
#[path = "all/member_relationship_status_filter.rs"]
mod member_relationship_status_filter;
#[path = "all/members.rs"]
mod members;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
