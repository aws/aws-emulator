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
#[path = "all/access_preview.rs"]
mod access_preview;
#[path = "all/access_preview_findings.rs"]
mod access_preview_findings;
#[path = "all/analyzed_resource.rs"]
mod analyzed_resource;
#[path = "all/analyzer_lifecycle.rs"]
mod analyzer_lifecycle;
#[path = "all/archive_rule_update.rs"]
mod archive_rule_update;
#[path = "all/aws_worded_messages.rs"]
mod aws_worded_messages;
#[path = "all/check_no_public_access.rs"]
mod check_no_public_access;
#[path = "all/error_payloads.rs"]
mod error_payloads;
#[path = "all/external_access_findings.rs"]
mod external_access_findings;
#[path = "all/fixed_refusal_texts.rs"]
mod fixed_refusal_texts;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/modeled_rejections.rs"]
mod modeled_rejections;
#[path = "all/modelled_wire_shape.rs"]
mod modelled_wire_shape;
#[path = "all/no_content_responses.rs"]
mod no_content_responses;
#[path = "all/policy_access_check.rs"]
mod policy_access_check;
#[path = "all/policy_check_reason_text.rs"]
mod policy_check_reason_text;
#[path = "all/policy_checks.rs"]
mod policy_checks;
#[path = "all/policy_generation.rs"]
mod policy_generation;
#[path = "all/policy_validation.rs"]
mod policy_validation;
#[path = "all/queue_policy_findings.rs"]
mod queue_policy_findings;
#[path = "all/resource_scan.rs"]
mod resource_scan;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/response_vocabulary.rs"]
mod response_vocabulary;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_path_not_found_text.rs"]
mod tag_path_not_found_text;
#[path = "all/update_analyzer.rs"]
mod update_analyzer;
