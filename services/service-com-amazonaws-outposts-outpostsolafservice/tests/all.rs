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
#[path = "all/arn_partition.rs"]
mod arn_partition;
#[path = "all/asset_configured_capacity.rs"]
mod asset_configured_capacity;
#[path = "all/asset_instances.rs"]
mod asset_instances;
#[path = "all/asset_scoped_capacity_configuration.rs"]
mod asset_scoped_capacity_configuration;
#[path = "all/availability_zone_identity.rs"]
mod availability_zone_identity;
#[path = "all/blocking_instance_asset_scope.rs"]
mod blocking_instance_asset_scope;
#[path = "all/blocking_instances.rs"]
mod blocking_instances;
#[path = "all/capacity_task_cancellation.rs"]
mod capacity_task_cancellation;
#[path = "all/capacity_task_dry_run.rs"]
mod capacity_task_dry_run;
#[path = "all/capacity_task_lifecycle.rs"]
mod capacity_task_lifecycle;
#[path = "all/capacity_task_listing.rs"]
mod capacity_task_listing;
#[path = "all/capacity_task_order_outpost.rs"]
mod capacity_task_order_outpost;
#[path = "all/capacity_task_order_region.rs"]
mod capacity_task_order_region;
#[path = "all/capacity_task_outcome.rs"]
mod capacity_task_outcome;
#[path = "all/catalog_item_specification.rs"]
mod catalog_item_specification;
#[path = "all/catalog_items.rs"]
mod catalog_items;
#[path = "all/configured_instance_types.rs"]
mod configured_instance_types;
#[path = "all/connection_store_bound.rs"]
mod connection_store_bound;
#[path = "all/created_identity_and_tag_state.rs"]
mod created_identity_and_tag_state;
#[path = "all/delete_conflicts.rs"]
mod delete_conflicts;
#[path = "all/dependent_resource_conflict.rs"]
mod dependent_resource_conflict;
#[path = "all/error_classification.rs"]
mod error_classification;
#[path = "all/error_message_bound.rs"]
mod error_message_bound;
#[path = "all/error_message_rules.rs"]
mod error_message_rules;
#[path = "all/filter_selection.rs"]
mod filter_selection;
#[path = "all/get_catalog_item.rs"]
mod get_catalog_item;
#[path = "all/get_order_reporting.rs"]
mod get_order_reporting;
#[path = "all/get_site_address_reporting.rs"]
mod get_site_address_reporting;
#[path = "all/identifier_arn_identity.rs"]
mod identifier_arn_identity;
#[path = "all/identifier_filter_rule.rs"]
mod identifier_filter_rule;
#[path = "all/instance_type_catalogs.rs"]
mod instance_type_catalogs;
#[path = "all/nested_page_tokens.rs"]
mod nested_page_tokens;
#[path = "all/not_found_wording.rs"]
mod not_found_wording;
#[path = "all/order_cancellation.rs"]
mod order_cancellation;
#[path = "all/order_catalog_item_status.rs"]
mod order_catalog_item_status;
#[path = "all/order_lifecycle.rs"]
mod order_lifecycle;
#[path = "all/order_line_item_assets.rs"]
mod order_line_item_assets;
#[path = "all/order_listing.rs"]
mod order_listing;
#[path = "all/order_quote_references.rs"]
mod order_quote_references;
#[path = "all/orderable_instance_types.rs"]
mod orderable_instance_types;
#[path = "all/outpost_assets.rs"]
mod outpost_assets;
#[path = "all/outpost_decommission.rs"]
mod outpost_decommission;
#[path = "all/outpost_deletion_cascade.rs"]
mod outpost_deletion_cascade;
#[path = "all/outpost_hardware_installation.rs"]
mod outpost_hardware_installation;
#[path = "all/outpost_hardware_type_change.rs"]
mod outpost_hardware_type_change;
#[path = "all/outpost_installed_hardware_facts.rs"]
mod outpost_installed_hardware_facts;
#[path = "all/outpost_order_driven_lifecycle.rs"]
mod outpost_order_driven_lifecycle;
#[path = "all/outpost_server_asset_kind.rs"]
mod outpost_server_asset_kind;
#[path = "all/outpost_server_connection.rs"]
mod outpost_server_connection;
#[path = "all/page_boundary_rule.rs"]
mod page_boundary_rule;
#[path = "all/page_token_canonical_spelling.rs"]
mod page_token_canonical_spelling;
#[path = "all/page_token_refusal_shape.rs"]
mod page_token_refusal_shape;
#[path = "all/precondition_gates.rs"]
mod precondition_gates;
#[path = "all/private_connectivity.rs"]
mod private_connectivity;
#[path = "all/private_connectivity_ownership.rs"]
mod private_connectivity_ownership;
#[path = "all/private_connectivity_roles.rs"]
mod private_connectivity_roles;
#[path = "all/quote_deletion.rs"]
mod quote_deletion;
#[path = "all/quote_lifecycle.rs"]
mod quote_lifecycle;
#[path = "all/quote_listing.rs"]
mod quote_listing;
#[path = "all/quote_options.rs"]
mod quote_options;
#[path = "all/quote_price_range.rs"]
mod quote_price_range;
#[path = "all/quote_store_bound.rs"]
mod quote_store_bound;
#[path = "all/quote_update.rs"]
mod quote_update;
#[path = "all/quote_update_after_order.rs"]
mod quote_update_after_order;
#[path = "all/regional_scope.rs"]
mod regional_scope;
#[path = "all/renewal_contract.rs"]
mod renewal_contract;
#[path = "all/renewal_pricing.rs"]
mod renewal_pricing;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_untagging.rs"]
mod resource_untagging;
#[path = "all/single_valued_filter_rule.rs"]
mod single_valued_filter_rule;
#[path = "all/site_address.rs"]
mod site_address;
#[path = "all/site_address_order_hold.rs"]
mod site_address_order_hold;
#[path = "all/site_and_outpost_lifecycle.rs"]
mod site_and_outpost_lifecycle;
#[path = "all/site_and_outpost_projection.rs"]
mod site_and_outpost_projection;
#[path = "all/site_delete_refusal.rs"]
mod site_delete_refusal;
#[path = "all/site_operating_address_windows.rs"]
mod site_operating_address_windows;
#[path = "all/site_rack_properties_order_hold.rs"]
mod site_rack_properties_order_hold;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/store_ceiling_wording.rs"]
mod store_ceiling_wording;
#[path = "all/supported_instance_types.rs"]
mod supported_instance_types;
#[path = "all/supported_instance_types_order_scope.rs"]
mod supported_instance_types_order_scope;
#[path = "all/tagging_arn_identity.rs"]
mod tagging_arn_identity;
#[path = "all/tagging_not_found_wording.rs"]
mod tagging_not_found_wording;
#[path = "all/tagging_resource_arn.rs"]
mod tagging_resource_arn;
#[path = "all/update_persistence.rs"]
mod update_persistence;
