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
#[path = "all/bad_request_error_attribute.rs"]
mod bad_request_error_attribute;
#[path = "all/broker_instance_options.rs"]
mod broker_instance_options;
#[path = "all/broker_lifecycle.rs"]
mod broker_lifecycle;
#[path = "all/broker_read_wire_form.rs"]
mod broker_read_wire_form;
#[path = "all/configuration_deleted_id.rs"]
mod configuration_deleted_id;
#[path = "all/configuration_lifecycle.rs"]
mod configuration_lifecycle;
#[path = "all/configuration_name.rs"]
mod configuration_name;
#[path = "all/configuration_revision_document_form.rs"]
mod configuration_revision_document_form;
#[path = "all/configuration_revision_isolation.rs"]
mod configuration_revision_isolation;
#[path = "all/configuration_wire_form.rs"]
mod configuration_wire_form;
#[path = "all/no_content_framing.rs"]
mod no_content_framing;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/user_deletion.rs"]
mod user_deletion;
