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
#[path = "all/association_vpc_references.rs"]
mod association_vpc_references;
#[path = "all/batch_firewall_rule_deletions.rs"]
mod batch_firewall_rule_deletions;
#[path = "all/batch_firewall_rule_updates.rs"]
mod batch_firewall_rule_updates;
#[path = "all/batch_firewall_rules.rs"]
mod batch_firewall_rules;
#[path = "all/endpoint_subnet_references.rs"]
mod endpoint_subnet_references;
#[path = "all/endpoint_vpc_and_addresses.rs"]
mod endpoint_vpc_and_addresses;
#[path = "all/firewall_association_error_bodies.rs"]
mod firewall_association_error_bodies;
#[path = "all/firewall_association_priority_band.rs"]
mod firewall_association_priority_band;
#[path = "all/firewall_block_option_refusals.rs"]
mod firewall_block_option_refusals;
#[path = "all/firewall_canonical_dns_names.rs"]
mod firewall_canonical_dns_names;
#[path = "all/firewall_config_listing.rs"]
mod firewall_config_listing;
#[path = "all/firewall_configs.rs"]
mod firewall_configs;
#[path = "all/firewall_domain_import_refusals.rs"]
mod firewall_domain_import_refusals;
#[path = "all/firewall_domain_import_steps.rs"]
mod firewall_domain_import_steps;
#[path = "all/firewall_domain_imports.rs"]
mod firewall_domain_imports;
#[path = "all/firewall_domain_list_deletion.rs"]
mod firewall_domain_list_deletion;
#[path = "all/firewall_domain_list_listing.rs"]
mod firewall_domain_list_listing;
#[path = "all/firewall_domain_lists.rs"]
mod firewall_domain_lists;
#[path = "all/firewall_domains.rs"]
mod firewall_domains;
#[path = "all/firewall_lifecycle_status_messages.rs"]
mod firewall_lifecycle_status_messages;
#[path = "all/firewall_resource_in_use.rs"]
mod firewall_resource_in_use;
#[path = "all/firewall_rule_deletion.rs"]
mod firewall_rule_deletion;
#[path = "all/firewall_rule_group_deletion.rs"]
mod firewall_rule_group_deletion;
#[path = "all/firewall_rule_group_listing.rs"]
mod firewall_rule_group_listing;
#[path = "all/firewall_rule_group_policies.rs"]
mod firewall_rule_group_policies;
#[path = "all/firewall_rule_group_policy_refusal_message.rs"]
mod firewall_rule_group_policy_refusal_message;
#[path = "all/firewall_rule_groups.rs"]
mod firewall_rule_groups;
#[path = "all/firewall_rule_lookup_sentences.rs"]
mod firewall_rule_lookup_sentences;
#[path = "all/firewall_rule_priority_uniqueness.rs"]
mod firewall_rule_priority_uniqueness;
#[path = "all/firewall_rule_updates.rs"]
mod firewall_rule_updates;
#[path = "all/firewall_rules_and_associations.rs"]
mod firewall_rules_and_associations;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/list_page_size_echo.rs"]
mod list_page_size_echo;
#[path = "all/managed_firewall_domain_lists.rs"]
mod managed_firewall_domain_lists;
#[path = "all/outpost_binding.rs"]
mod outpost_binding;
#[path = "all/outpost_resolvers.rs"]
mod outpost_resolvers;
#[path = "all/query_log_refusal_trace_clause.rs"]
mod query_log_refusal_trace_clause;
#[path = "all/resolver_config_listing.rs"]
mod resolver_config_listing;
#[path = "all/resolver_config_reads.rs"]
mod resolver_config_reads;
#[path = "all/resolver_configs.rs"]
mod resolver_configs;
#[path = "all/resolver_dnssec_config_listing.rs"]
mod resolver_dnssec_config_listing;
#[path = "all/resolver_dnssec_config_reads.rs"]
mod resolver_dnssec_config_reads;
#[path = "all/resolver_dnssec_configs.rs"]
mod resolver_dnssec_configs;
#[path = "all/resolver_endpoint_ip_address_association.rs"]
mod resolver_endpoint_ip_address_association;
#[path = "all/resolver_endpoints.rs"]
mod resolver_endpoints;
#[path = "all/resolver_query_log_config_association_listing.rs"]
mod resolver_query_log_config_association_listing;
#[path = "all/resolver_query_log_config_associations.rs"]
mod resolver_query_log_config_associations;
#[path = "all/resolver_query_log_config_deletion.rs"]
mod resolver_query_log_config_deletion;
#[path = "all/resolver_query_log_config_disassociation.rs"]
mod resolver_query_log_config_disassociation;
#[path = "all/resolver_query_log_config_listing.rs"]
mod resolver_query_log_config_listing;
#[path = "all/resolver_query_log_config_policies.rs"]
mod resolver_query_log_config_policies;
#[path = "all/resolver_query_log_configs.rs"]
mod resolver_query_log_configs;
#[path = "all/resolver_rule_association_listing.rs"]
mod resolver_rule_association_listing;
#[path = "all/resolver_rule_association_ordering.rs"]
mod resolver_rule_association_ordering;
#[path = "all/resolver_rule_delete_identity.rs"]
mod resolver_rule_delete_identity;
#[path = "all/resolver_rule_deletion.rs"]
mod resolver_rule_deletion;
#[path = "all/resolver_rule_disassociation.rs"]
mod resolver_rule_disassociation;
#[path = "all/resolver_rule_error_bodies.rs"]
mod resolver_rule_error_bodies;
#[path = "all/resolver_rule_listing_bounds.rs"]
mod resolver_rule_listing_bounds;
#[path = "all/resolver_rule_policies.rs"]
mod resolver_rule_policies;
#[path = "all/resolver_rule_tokens_and_refusal_sentences.rs"]
mod resolver_rule_tokens_and_refusal_sentences;
#[path = "all/resolver_rule_updates.rs"]
mod resolver_rule_updates;
#[path = "all/resolver_rules.rs"]
mod resolver_rules;
#[path = "all/resource_not_found_identifier.rs"]
mod resource_not_found_identifier;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/scratch_probe.rs"]
mod scratch_probe;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_acceptance.rs"]
mod tag_acceptance;
#[path = "all/vpc_configuration_inventory.rs"]
mod vpc_configuration_inventory;
#[path = "all/vpc_configuration_preconditions.rs"]
mod vpc_configuration_preconditions;
