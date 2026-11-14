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
#[path = "all/glue_resource_tags.rs"]
mod glue_resource_tags;

#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;

#[path = "all/tag_read_write_loop.rs"]
mod tag_read_write_loop;

#[path = "all/tagging_api_fidelity.rs"]
mod tagging_api_fidelity;

// Declared after the sorted list on purpose: every owner this API gains adds its suite
// inside that list, so a line inserted there collides with every other suite landing in
// the same window. Appending keeps this change out of the contended region; the suite
// itself is an ordinary `tests/all/<name>.rs` file. Fold it into the sorted list whenever
// the list is next touched for another reason.
#[path = "all/apigateway_tagged_resource_index.rs"]
mod apigateway_tagged_resource_index;

#[path = "all/sfn_tagged_resource_index.rs"]
mod sfn_tagged_resource_index;

#[path = "all/arn_parameter_refusal_wording.rs"]
mod arn_parameter_refusal_wording;

#[path = "all/pagination_termination.rs"]
mod pagination_termination;

#[path = "all/elb_tagged_resource_index.rs"]
mod elb_tagged_resource_index;

#[path = "all/lambda_tagged_resource_index.rs"]
mod lambda_tagged_resource_index;

#[path = "all/map_value_constraint_clauses.rs"]
mod map_value_constraint_clauses;

#[path = "all/sns_tagged_resource_index.rs"]
mod sns_tagged_resource_index;

#[path = "all/arn_list_value_fault_template.rs"]
mod arn_list_value_fault_template;

#[path = "all/resource_type_filter_spelling.rs"]
mod resource_type_filter_spelling;

#[path = "all/health_agent_tagged_resource_index.rs"]
mod health_agent_tagged_resource_index;
