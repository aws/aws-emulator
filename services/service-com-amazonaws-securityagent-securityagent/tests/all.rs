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
#[path = "all/agent_space_batch_read.rs"]
mod agent_space_batch_read;
#[path = "all/agent_space_deletion.rs"]
mod agent_space_deletion;
#[path = "all/agent_space_lifecycle.rs"]
mod agent_space_lifecycle;
#[path = "all/agent_space_update.rs"]
mod agent_space_update;
#[path = "all/agent_space_update_echo.rs"]
mod agent_space_update_echo;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/artifact_deletion.rs"]
mod artifact_deletion;
#[path = "all/artifact_metadata_batch_read.rs"]
mod artifact_metadata_batch_read;
#[path = "all/artifact_listing.rs"]
mod artifact_listing;
#[path = "all/artifact_storage.rs"]
mod artifact_storage;
#[path = "all/aws_managed_pack_catalog.rs"]
mod aws_managed_pack_catalog;
#[path = "all/error_message_wording.rs"]
mod error_message_wording;
#[path = "all/minted_identifiers.rs"]
mod minted_identifiers;
#[path = "all/private_connection_lifecycle.rs"]
mod private_connection_lifecycle;
#[path = "all/recorded_wire_form.rs"]
mod recorded_wire_form;
#[path = "all/security_requirement_pack.rs"]
mod security_requirement_pack;
#[path = "all/security_requirement_pack_lifecycle.rs"]
mod security_requirement_pack_lifecycle;
#[path = "all/security_requirement_packs.rs"]
mod security_requirement_packs;
#[path = "all/security_requirements_listing.rs"]
mod security_requirements_listing;
#[path = "all/security_requirements_batch.rs"]
mod security_requirements_batch;
#[path = "all/security_requirements_batch_delete.rs"]
mod security_requirements_batch_delete;
#[path = "all/security_requirements_batch_update.rs"]
mod security_requirements_batch_update;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Not a suite of its own: the wire harness every suite shares, reached as
// `crate::support`. It is declared here because every file under `tests/all/` must be.
#[path = "all/support.rs"]
mod support;
#[path = "all/target_domain_deletion.rs"]
mod target_domain_deletion;
#[path = "all/target_domain_registration.rs"]
mod target_domain_registration;
#[path = "all/target_domain_verification.rs"]
mod target_domain_verification;
