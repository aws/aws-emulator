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
#[path = "all/azure_blob_credentials.rs"]
mod azure_blob_credentials;
#[path = "all/azure_blob_location.rs"]
mod azure_blob_location;
#[path = "all/bounded_stores.rs"]
mod bounded_stores;
#[path = "all/bucket_access_role_assume.rs"]
mod bucket_access_role_assume;
#[path = "all/efs_location.rs"]
mod efs_location;
#[path = "all/location_bucket_preflight.rs"]
mod location_bucket_preflight;
#[path = "all/location_deletion.rs"]
mod location_deletion;
#[path = "all/location_listing.rs"]
mod location_listing;
#[path = "all/object_storage_location.rs"]
mod object_storage_location;
#[path = "all/s3_transfer_execution.rs"]
mod s3_transfer_execution;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/task_refusal_classes.rs"]
mod task_refusal_classes;
#[path = "all/transfer_task_lifecycle.rs"]
mod transfer_task_lifecycle;
