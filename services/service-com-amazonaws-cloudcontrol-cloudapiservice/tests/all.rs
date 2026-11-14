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
#[path = "all/arn_assigned_identifiers.rs"]
mod arn_assigned_identifiers;
#[path = "all/arn_identified_resources.rs"]
mod arn_identified_resources;
#[path = "all/audit_records.rs"]
mod audit_records;
#[path = "all/cancel_resource_request.rs"]
mod cancel_resource_request;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/native_queue_lifecycle.rs"]
mod native_queue_lifecycle;
#[path = "all/native_resource_resolution.rs"]
mod native_resource_resolution;
#[path = "all/primary_identifier.rs"]
mod primary_identifier;
#[path = "all/refusal_wording.rs"]
mod refusal_wording;
#[path = "all/request_cancellation.rs"]
mod request_cancellation;
#[path = "all/request_listing.rs"]
mod request_listing;
#[path = "all/resource_identity.rs"]
mod resource_identity;
#[path = "all/resource_lifecycle.rs"]
mod resource_lifecycle;
#[path = "all/resource_listing.rs"]
mod resource_listing;
#[path = "all/s3_bucket_provisioning.rs"]
mod s3_bucket_provisioning;
#[path = "all/sns_topic_read_through.rs"]
mod sns_topic_read_through;
#[path = "all/sqs_queue_read_through.rs"]
mod sqs_queue_read_through;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unsupported_actions.rs"]
mod unsupported_actions;
