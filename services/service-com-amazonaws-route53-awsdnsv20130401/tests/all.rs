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
#[path = "all/account_limits.rs"]
mod account_limits;
#[path = "all/checker_ip_ranges.rs"]
mod checker_ip_ranges;
#[path = "all/cidr_collections.rs"]
mod cidr_collections;
#[path = "all/cidr_collection_blocks.rs"]
mod cidr_collection_blocks;
// Declared beside the block suite: this one is the BODY shape the two CIDR-collection
// refusals the service reports by their error class alone answer with.
#[path = "all/cidr_collection_refusal_bodies.rs"]
mod cidr_collection_refusal_bodies;
// Declared beside the block suite: this one is the metadata-only read over a
// collection's locations — what it reports, how it pages and what it refuses.
#[path = "all/cidr_location_listing.rs"]
mod cidr_location_listing;
#[path = "all/delegation_set_deletion.rs"]
mod delegation_set_deletion;
#[path = "all/delegation_set_limits.rs"]
mod delegation_set_limits;
#[path = "all/delegation_set_listing.rs"]
mod delegation_set_listing;
#[path = "all/dns_answers.rs"]
mod dns_answers;
#[path = "all/dnssec_status.rs"]
mod dnssec_status;
#[path = "all/enum_value_set_order.rs"]
mod enum_value_set_order;
#[path = "all/error_classification.rs"]
mod error_classification;
#[path = "all/error_envelope.rs"]
mod error_envelope;
#[path = "all/geo_locations.rs"]
mod geo_locations;
#[path = "all/geolocation_routing.rs"]
mod geolocation_routing;
// Declared beside the other health-check suites: this one is the type gate both
// readers of a health check's checker observations apply.
#[path = "all/health_check_calculated_reads.rs"]
mod health_check_calculated_reads;
// Declared beside the other health-check suites: this one is the members of a
// `HealthCheckConfig` the service decides for a request that leaves them out.
#[path = "all/health_check_config_defaults.rs"]
mod health_check_config_defaults;
#[path = "all/health_check_creation.rs"]
mod health_check_creation;
// Declared beside the creation suite: this one is the request-schema enumeration a
// health check's probe cadence is held to, which is refused before any handler runs.
#[path = "all/health_check_request_interval_enumeration.rs"]
mod health_check_request_interval_enumeration;
// Declared beside the other health-check suites: this one is the `ChildHealthChecks`
// length bound, which the creation suite's tables do not reach.
#[path = "all/health_check_child_list_bound.rs"]
mod health_check_child_list_bound;
#[path = "all/health_check_deletion.rs"]
mod health_check_deletion;
#[path = "all/health_check_last_failure_reason.rs"]
mod health_check_last_failure_reason;
#[path = "all/health_check_listing.rs"]
mod health_check_listing;
// Declared beside the other health-check suites: the CloudWatch alarm an
// alarm-reading health check monitors, which is the one part of a reported health
// check that comes from another service's store.
#[path = "all/health_check_monitored_alarm.rs"]
mod health_check_monitored_alarm;
#[path = "all/health_check_not_found_message.rs"]
mod health_check_not_found_message;
#[path = "all/health_check_refusal_messages.rs"]
mod health_check_refusal_messages;
#[path = "all/health_check_status.rs"]
mod health_check_status;
#[path = "all/health_check_update_contention.rs"]
mod health_check_update_contention;
#[path = "all/health_check_updates.rs"]
mod health_check_updates;
#[path = "all/health_checks.rs"]
mod health_checks;
#[path = "all/hosted_zone_config.rs"]
mod hosted_zone_config;
#[path = "all/hosted_zone_count.rs"]
mod hosted_zone_count;
// Declared beside the zone suites: this one is the domain-name format every
// caller-supplied DNS name is held to, across the operations that take one.
#[path = "all/hosted_zone_dns_name_validation.rs"]
mod hosted_zone_dns_name_validation;
#[path = "all/hosted_zone_features.rs"]
mod hosted_zone_features;
// Declared beside the features suite: this one is the `Features` CONTAINER every
// hosted-zone projection carries, which the setting suite's tables do not ask about.
#[path = "all/hosted_zone_features_envelope.rs"]
mod hosted_zone_features_envelope;
#[path = "all/hosted_zone_limits.rs"]
mod hosted_zone_limits;
#[path = "all/hosted_zone_records.rs"]
mod hosted_zone_records;
#[path = "all/hosted_zones_by_name.rs"]
mod hosted_zones_by_name;
#[path = "all/hosted_zones_by_vpc.rs"]
mod hosted_zones_by_vpc;
// Declared beside the DNSSEC suite: this one is the key-signing key a zone is
// signed with — what a create computes from its customer managed key, what it
// persists and what it refuses.
#[path = "all/key_signing_keys.rs"]
mod key_signing_keys;
// Declared beside the listing suites: this one is the page size every
// `maxitems`-paged listing reports, across all of them at once.
#[path = "all/listing_page_size_echo.rs"]
mod listing_page_size_echo;
#[path = "all/reference_catalogs.rs"]
mod reference_catalogs;
// Declared beside the other refusal-wording suites: this one is the three
// refusals whose message is a fixed form the other suites' tables do not cover.
#[path = "all/refusal_message_forms.rs"]
mod refusal_message_forms;
#[path = "all/refusal_message_rules.rs"]
mod refusal_message_rules;
#[path = "all/record_name_canonical_form.rs"]
mod record_name_canonical_form;
#[path = "all/record_set_health_check_references.rs"]
mod record_set_health_check_references;
#[path = "all/record_set_refusal_rules.rs"]
mod record_set_refusal_rules;
#[path = "all/record_set_validation.rs"]
mod record_set_validation;
#[path = "all/private_zone_creation.rs"]
mod private_zone_creation;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/resource_tags_batch.rs"]
mod resource_tags_batch;
#[path = "all/reusable_delegation_sets.rs"]
mod reusable_delegation_sets;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Declared beside the hosted-zone and tagging suites: this one is the two reads
// TOGETHER — the listing a client pages and the tag set it selects a zone by.
#[path = "all/tag_driven_zone_discovery.rs"]
mod tag_driven_zone_discovery;
// Declared beside the tagging suites: this one is the FIELD a refusal of a tag
// operation's resource id names, across the three operations that take one.
#[path = "all/tag_resource_id_refusal.rs"]
mod tag_resource_id_refusal;
#[path = "all/traffic_policies.rs"]
mod traffic_policies;
#[path = "all/traffic_policy_comment_updates.rs"]
mod traffic_policy_comment_updates;
#[path = "all/traffic_policy_deletion.rs"]
mod traffic_policy_deletion;
#[path = "all/traffic_policy_identity.rs"]
mod traffic_policy_identity;
#[path = "all/traffic_policy_instance_count.rs"]
mod traffic_policy_instance_count;
#[path = "all/traffic_policy_instance_deletion.rs"]
mod traffic_policy_instance_deletion;
#[path = "all/traffic_policy_instance_listing.rs"]
mod traffic_policy_instance_listing;
#[path = "all/traffic_policy_instances.rs"]
mod traffic_policy_instances;
#[path = "all/traffic_policy_listing.rs"]
mod traffic_policy_listing;
// Declared beside the traffic-policy suites: this one is the BODY shape the two
// traffic-policy not-found refusals answer with, across every operation that
// binds them.
#[path = "all/traffic_policy_not_found_envelope.rs"]
mod traffic_policy_not_found_envelope;
#[path = "all/traffic_policy_version_listing.rs"]
mod traffic_policy_version_listing;
#[path = "all/traffic_policy_versions.rs"]
mod traffic_policy_versions;
#[path = "all/vpc_association_authorizations.rs"]
mod vpc_association_authorizations;
#[path = "all/vpc_associations.rs"]
mod vpc_associations;
#[path = "all/vpc_authorization_deletion.rs"]
mod vpc_authorization_deletion;
#[path = "all/vpc_disassociation.rs"]
mod vpc_disassociation;
#[path = "all/zone_refusal_messages.rs"]
mod zone_refusal_messages;
