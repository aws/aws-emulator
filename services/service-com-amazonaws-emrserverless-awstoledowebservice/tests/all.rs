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
#[path = "all/access_grant_refusals.rs"]
mod access_grant_refusals;
#[path = "all/application_lifecycle.rs"]
mod application_lifecycle;
#[path = "all/application_projection.rs"]
mod application_projection;
#[path = "all/effective_configuration_projection.rs"]
mod effective_configuration_projection;
#[path = "all/idempotency_and_idle_lifecycle.rs"]
mod idempotency_and_idle_lifecycle;
#[path = "all/idle_timeout_observability.rs"]
mod idle_timeout_observability;
#[path = "all/job_run_cloud_watch_log_publication.rs"]
mod job_run_cloud_watch_log_publication;
#[path = "all/job_run_log_publication.rs"]
mod job_run_log_publication;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/session_endpoint.rs"]
mod session_endpoint;
#[path = "all/session_lifecycle.rs"]
mod session_lifecycle;
#[path = "all/session_listing.rs"]
mod session_listing;
#[path = "all/session_refusal_semantics.rs"]
mod session_refusal_semantics;
#[path = "all/session_termination.rs"]
mod session_termination;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_mutation_status.rs"]
mod tag_mutation_status;
#[path = "all/update_application.rs"]
mod update_application;
#[path = "all/update_application_while_started.rs"]
mod update_application_while_started;
