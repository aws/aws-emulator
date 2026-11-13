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
#[path = "all/celebrity_recognition.rs"]
mod celebrity_recognition;
#[path = "all/collection_face_lifecycle.rs"]
mod collection_face_lifecycle;
#[path = "all/collection_users.rs"]
mod collection_users;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/face_collections.rs"]
mod face_collections;
#[path = "all/face_comparison.rs"]
mod face_comparison;
#[path = "all/face_detection.rs"]
mod face_detection;
#[path = "all/face_search_by_image.rs"]
mod face_search_by_image;
#[path = "all/image_content_refusal.rs"]
mod image_content_refusal;
#[path = "all/label_detection.rs"]
mod label_detection;
#[path = "all/photographic_enrolment.rs"]
mod photographic_enrolment;
#[path = "all/project_policies.rs"]
mod project_policies;
#[path = "all/projects.rs"]
mod projects;
#[path = "all/protective_equipment.rs"]
mod protective_equipment;
#[path = "all/resource_tag_values.rs"]
mod resource_tag_values;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/video_face_detection.rs"]
mod video_face_detection;
