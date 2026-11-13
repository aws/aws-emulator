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
#[path = "all/absent_job_refusal.rs"]
mod absent_job_refusal;
#[path = "all/async_job_data_access_role.rs"]
mod async_job_data_access_role;
#[path = "all/data_access_role_refusal_classes.rs"]
mod data_access_role_refusal_classes;
#[path = "all/entities_detection_v2.rs"]
mod entities_detection_v2;
#[path = "all/icd10_cm_inference.rs"]
mod icd10_cm_inference;
#[path = "all/ontology_candidate_ranking.rs"]
mod ontology_candidate_ranking;
#[path = "all/phi_detection.rs"]
mod phi_detection;
#[path = "all/phi_detection_job_describe.rs"]
mod phi_detection_job_describe;
#[path = "all/phi_detection_job_run.rs"]
mod phi_detection_job_run;
#[path = "all/phi_detection_jobs.rs"]
mod phi_detection_jobs;
#[path = "all/refusal_messages.rs"]
mod refusal_messages;
#[path = "all/rx_norm_inference.rs"]
mod rx_norm_inference;
#[path = "all/snomedct_attribute_concepts.rs"]
mod snomedct_attribute_concepts;
#[path = "all/snomedct_inference.rs"]
mod snomedct_inference;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/synchronous_inference_shape.rs"]
mod synchronous_inference_shape;
