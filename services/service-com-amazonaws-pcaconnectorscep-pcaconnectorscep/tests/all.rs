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
#[path = "all/challenge_client_token.rs"]
mod challenge_client_token;
#[path = "all/challenge_deletion.rs"]
mod challenge_deletion;
#[path = "all/challenge_metadata.rs"]
mod challenge_metadata;
#[path = "all/challenge_password_connector_state.rs"]
mod challenge_password_connector_state;
#[path = "all/connector_delete_not_found.rs"]
mod connector_delete_not_found;
#[path = "all/connector_deletion.rs"]
mod connector_deletion;
#[path = "all/connector_endpoint_host_routing.rs"]
mod connector_endpoint_host_routing;
#[path = "all/connector_lifecycle.rs"]
mod connector_lifecycle;
#[path = "all/connector_oidc_parameters.rs"]
mod connector_oidc_parameters;
#[path = "all/connector_private_ca_reachability.rs"]
mod connector_private_ca_reachability;
#[path = "all/connector_projection_agreement.rs"]
mod connector_projection_agreement;
#[path = "all/connector_reference_resolution.rs"]
mod connector_reference_resolution;
#[path = "all/connector_scep_endpoint.rs"]
mod connector_scep_endpoint;
#[path = "all/connector_vpc_endpoint.rs"]
mod connector_vpc_endpoint;
// The shared harness the suites drive the service through: one copy of the router, the
// request builder, the `@httpLabel` encoding and the pending-transition clock move.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/harness_contract.rs"]
mod harness_contract;
#[path = "all/malformed_resource_arn.rs"]
mod malformed_resource_arn;
#[path = "all/minted_identifier_form.rs"]
mod minted_identifier_form;
#[path = "all/missing_resource_status_class.rs"]
mod missing_resource_status_class;
#[path = "all/model_pattern_refusal.rs"]
mod model_pattern_refusal;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/region_segment_rule.rs"]
mod region_segment_rule;
#[path = "all/resource_arn_validation.rs"]
mod resource_arn_validation;
#[path = "all/resource_not_found_message.rs"]
mod resource_not_found_message;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_resource.rs"]
mod tag_resource;
#[path = "all/tag_updated_at.rs"]
mod tag_updated_at;
#[path = "all/tag_validation.rs"]
mod tag_validation;
#[path = "all/tag_write_under_teardown.rs"]
mod tag_write_under_teardown;
#[path = "all/tagging_absent_resource_request_shape.rs"]
mod tagging_absent_resource_request_shape;
#[path = "all/untag_resource.rs"]
mod untag_resource;
