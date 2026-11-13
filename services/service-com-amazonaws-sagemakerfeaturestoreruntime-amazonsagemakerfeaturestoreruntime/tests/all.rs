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
#[path = "all/batch_write_record_bulk_ingestion.rs"]
mod batch_write_record_bulk_ingestion;
#[path = "all/feature_group_default_ttl.rs"]
mod feature_group_default_ttl;
#[path = "all/feature_group_name_validation.rs"]
mod feature_group_name_validation;
#[path = "all/feature_group_resolution.rs"]
mod feature_group_resolution;
#[path = "all/offline_store_object_visibility.rs"]
mod offline_store_object_visibility;
#[path = "all/offline_store_replication.rs"]
mod offline_store_replication;
#[path = "all/online_store_record_lifecycle.rs"]
mod online_store_record_lifecycle;
#[path = "all/pattern_refusal_rendering.rs"]
mod pattern_refusal_rendering;
#[path = "all/refusal_message_wording.rs"]
mod refusal_message_wording;
#[path = "all/rejection_statements.rs"]
mod rejection_statements;
#[path = "all/sagemaker_signing_name_routing.rs"]
mod sagemaker_signing_name_routing;
#[path = "all/size_bound_statements.rs"]
mod size_bound_statements;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_record_partial_update.rs"]
mod update_record_partial_update;
#[path = "all/update_record_semantics.rs"]
mod update_record_semantics;
