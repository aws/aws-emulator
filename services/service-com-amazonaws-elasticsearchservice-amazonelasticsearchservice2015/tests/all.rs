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
#[path = "all/cluster_instance_types.rs"]
mod cluster_instance_types;
#[path = "all/compatible_versions.rs"]
mod compatible_versions;
#[path = "all/describe_domains_batch.rs"]
mod describe_domains_batch;
#[path = "all/domain_data_plane.rs"]
mod domain_data_plane;
#[path = "all/domain_lifecycle.rs"]
mod domain_lifecycle;
#[path = "all/domain_tags.rs"]
mod domain_tags;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/handler_input_constraint_order.rs"]
mod handler_input_constraint_order;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/instance_type_limits_version_refusal.rs"]
mod instance_type_limits_version_refusal;
#[path = "all/missing_resource_not_found_status.rs"]
mod missing_resource_not_found_status;
#[path = "all/outbound_cross_cluster_search_connections.rs"]
mod outbound_cross_cluster_search_connections;
#[path = "all/package_associations.rs"]
mod package_associations;
#[path = "all/package_versions.rs"]
mod package_versions;
#[path = "all/packages.rs"]
mod packages;
#[path = "all/pattern_refusal_quoting.rs"]
mod pattern_refusal_quoting;
#[path = "all/published_package_catalog.rs"]
mod published_package_catalog;
#[path = "all/refusal_value_echo.rs"]
mod refusal_value_echo;
#[path = "all/shared_es_signing_name_claim.rs"]
mod shared_es_signing_name_claim;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/vpc_endpoint_id_expression.rs"]
mod vpc_endpoint_id_expression;
