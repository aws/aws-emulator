// SPDX-License-Identifier: CC0-1.0

//! Single integration-test target for this crate.
//!
//! Put a new suite at `tests/all/<name>.rs` and declare it below, keeping the list
//! sorted.
//! A suite that is not declared here compiles nowhere and its tests never run. A
//! suite left at the top level of `tests/` fails the build.
// `tests/all.rs` is a test-target CRATE ROOT, so a bare `mod` resolves to a SIBLING
// (`tests/state_roundtrip.rs`) — which the layout check refuses. The `#[path]` is what
// points it into `tests/all/`.
#[path = "all/channel_deletion.rs"]
mod channel_deletion;
#[path = "all/channel_error_envelope.rs"]
mod channel_error_envelope;
#[path = "all/channel_lifecycle.rs"]
mod channel_lifecycle;
#[path = "all/channel_listing.rs"]
mod channel_listing;
#[path = "all/channel_updates.rs"]
mod channel_updates;
#[path = "all/dashboard_deletion.rs"]
mod dashboard_deletion;
#[path = "all/dashboard_enumeration.rs"]
mod dashboard_enumeration;
#[path = "all/dashboard_lifecycle.rs"]
mod dashboard_lifecycle;
#[path = "all/dashboard_refresh.rs"]
mod dashboard_refresh;
#[path = "all/dashboard_updates.rs"]
mod dashboard_updates;
#[path = "all/disable_federation.rs"]
mod disable_federation;
#[path = "all/emulator.rs"]
mod emulator;
#[path = "all/enum_refusal_wording.rs"]
mod enum_refusal_wording;
#[path = "all/event_configuration.rs"]
mod event_configuration;
#[path = "all/event_data_store_ingestion.rs"]
mod event_data_store_ingestion;
#[path = "all/event_data_store_ingestion_start.rs"]
mod event_data_store_ingestion_start;
#[path = "all/event_data_store_lifecycle.rs"]
mod event_data_store_lifecycle;
#[path = "all/event_data_store_reads.rs"]
mod event_data_store_reads;
#[path = "all/event_data_store_update.rs"]
mod event_data_store_update;
#[path = "all/event_selectors.rs"]
mod event_selectors;
#[path = "all/federation_lifecycle.rs"]
mod federation_lifecycle;
#[path = "all/generate_query.rs"]
mod generate_query;
#[path = "all/import_list.rs"]
mod import_list;
#[path = "all/import_reads.rs"]
mod import_reads;
#[path = "all/import_start.rs"]
mod import_start;
#[path = "all/import_stop.rs"]
mod import_stop;
#[path = "all/insight_selectors.rs"]
mod insight_selectors;
#[path = "all/insights_data.rs"]
mod insights_data;
#[path = "all/insights_metric_data.rs"]
mod insights_metric_data;
#[path = "all/lake_queries.rs"]
mod lake_queries;
#[path = "all/list_tags_reader.rs"]
mod list_tags_reader;
#[path = "all/management_event_lookup.rs"]
mod management_event_lookup;
#[path = "all/query_cancellation.rs"]
mod query_cancellation;
#[path = "all/query_results.rs"]
mod query_results;
#[path = "all/query_results_target_prefix.rs"]
mod query_results_target_prefix;
#[path = "all/range_refusal_wording.rs"]
mod range_refusal_wording;
#[path = "all/refusal_fidelity.rs"]
mod refusal_fidelity;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/sample_queries.rs"]
mod sample_queries;
#[path = "all/start_query.rs"]
mod start_query;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
// Declared HERE rather than in the `import_*` span of the sorted list above: that span
// is where every import suite landing in the same window inserts a line, so it is the
// contended region, while these neighbours are stable. `harness` holds this suite's
// import-registry fixtures.
#[path = "all/harness.rs"]
mod harness;
#[path = "all/import_failures.rs"]
mod import_failures;
// Declared beside `import_failures` for the same reason: the sorted span above is the
// contended region. `pattern_refusal_wording` pins how this service words a boundary
// refusal for a `@pattern` violation, over every operation that has one.
#[path = "all/pattern_refusal_wording.rs"]
mod pattern_refusal_wording;
#[path = "all/tag_refusal_envelope.rs"]
mod tag_refusal_envelope;
#[path = "all/tagging_arn_spelling.rs"]
mod tagging_arn_spelling;
#[path = "all/target_prefix_dispatch.rs"]
mod target_prefix_dispatch;
#[path = "all/target_prefix_routing.rs"]
mod target_prefix_routing;
#[path = "all/target_routing.rs"]
mod target_routing;
#[path = "all/trail_lifecycle.rs"]
mod trail_lifecycle;
#[path = "all/trail_refusals.rs"]
mod trail_refusals;
#[path = "all/trail_update.rs"]
mod trail_update;
// Declared only because the build's layout check requires every file under `tests/all/`
// to be declared: these three are empty leftovers (see their own doc comments), kept
// because the fixers' tool cannot delete a file. `probe.rs` held what is now the
// better-named `query_results_target_prefix.rs`.
#[path = "all/probe.rs"]
mod probe;
#[path = "all/wire.rs"]
mod wire;
#[path = "all/zz_diagnostic.rs"]
mod zz_diagnostic;
// Declared after the alphabetical block above, not inside it: this file is edited by
// every fix that adds a suite to this crate, and a declaration appended here cannot
// collide with one inserted alphabetically beside a neighbouring suite.
#[path = "all/resource_policy.rs"]
mod resource_policy;
#[path = "all/resource_policy_isolation.rs"]
mod resource_policy_isolation;
#[path = "all/lake_query_listing.rs"]
mod lake_query_listing;
#[path = "all/event_data_store_listing.rs"]
mod event_data_store_listing;
#[path = "all/resource_policy_delete.rs"]
mod resource_policy_delete;
#[path = "all/dashboard_protection_and_policy_readiness.rs"]
mod dashboard_protection_and_policy_readiness;
// `insight_selectors_api_state` reads Insights selectors back over state built only
// through this API (`CreateTrail` / `CreateEventDataStore`), beside the older
// `insight_selectors` suite that seeds the store registry directly.
#[path = "all/insight_selectors_api_state.rs"]
mod insight_selectors_api_state;
// `channel_destination_identifier` pins how a `Destination$Location` is read and how
// each way of failing to read it is refused, over both operations that take a
// `Destinations` list.
#[path = "all/channel_destination_identifier.rs"]
mod channel_destination_identifier;
// `page_size_and_query_id_refusals` pins what a page size and a query id a request
// names are answered with, across the operations each rule reaches.
#[path = "all/page_size_and_query_id_refusals.rs"]
mod page_size_and_query_id_refusals;
// `event_data_store_restore` pins `RestoreEventDataStore`: which status it applies to,
// what the clock owes the store it brings back, and how it refuses an identifier.
#[path = "all/event_data_store_restore.rs"]
mod event_data_store_restore;
// `dashboard_update_status` pins the `DashboardStatus` a write leaves a dashboard in:
// the status an accepted update reports from the moment it returns, and that a read or
// a refusal moves none.
#[path = "all/dashboard_update_status.rs"]
mod dashboard_update_status;
// `dashboard_termination_protection_wording` pins the WORDING of the refusal a delete of
// a termination-protected dashboard carries: one fixed sentence, with no part of the
// request or of the dashboard interpolated into it.
#[path = "all/dashboard_termination_protection_wording.rs"]
mod dashboard_termination_protection_wording;
// `event_history_of_every_served_call` pins that EVERY call this service serves — a read
// as much as a write — is in event history, that an unfiltered window reports what was
// recorded, and that a record names the resource its call addressed.
#[path = "all/event_history_of_every_served_call.rs"]
mod event_history_of_every_served_call;
// `selectors_that_log_nothing` pins the refusal of a basic event selector that would log
// nothing at all, and that the refusal leaves the trail's selectors untouched.
#[path = "all/selectors_that_log_nothing.rs"]
mod selectors_that_log_nothing;
// `request_validation` pins what this service checks about a REQUEST before it acts on
// it: the destination bucket a trail is pointed at, the window an Insights metric read
// may ask about, the fixed sentences a lookup refusal carries, and that an event
// configuration call is judged on its parameters before its target.
#[path = "all/request_validation.rs"]
mod request_validation;
// `digest_signing_public_keys` pins `ListPublicKeys`: which of the Region's digest-signing
// keys a window selects, what a reported key is, and how an inverted window and a page
// marker the service never issued are refused.
#[path = "all/digest_signing_public_keys.rs"]
mod digest_signing_public_keys;
// `referenced_resource_validation` pins the rule over every resource a trail's settings
// REFERENCE — the destination bucket and its policy, the notification topic's policy, the
// encryption key and the CloudWatch Logs destination — and the one every advanced event
// selector has to state.
#[path = "all/referenced_resource_validation.rs"]
mod referenced_resource_validation;
// `selector_value_refusal_wording` pins how a value outside the set its event-selector
// parameter admits is refused, over every such parameter and whichever side settles the set.
#[path = "all/selector_value_refusal_wording.rs"]
mod selector_value_refusal_wording;
// `trail_log_file_delivery` pins what a LOGGING trail delivers into its bucket: the gzipped
// log file each recorded call lands in, under the documented key layout.
#[path = "all/trail_log_file_delivery.rs"]
mod trail_log_file_delivery;
// `trail_log_file_delivery_of_data_events` pins the delivery of a DATA event: a call on the
// CONTENTS of a resource reaches the bucket of every trail whose advanced selector admits
// the event's category and resource type, and reaches event history never.
#[path = "all/trail_log_file_delivery_of_data_events.rs"]
mod trail_log_file_delivery_of_data_events;
// `trail_log_file_delivery_of_other_services` pins the same delivery for the calls of the
// account's OTHER services: whichever service served a management call, its record lands
// under the trail's own key prefix, named by the id the caller was answered under.
#[path = "all/trail_log_file_delivery_of_other_services.rs"]
mod trail_log_file_delivery_of_other_services;
// `trail_not_found_identifier_echo` pins WHICH identifier a `TrailNotFoundException`
// names — the one the request supplied, in the form it supplied it — over every
// operation that addresses a trail by `Name` / `TrailName`.
#[path = "all/trail_not_found_identifier_echo.rs"]
mod trail_not_found_identifier_echo;
// `trail_destination_revalidation` pins WHEN a trail's log-delivery destination is
// resolved: on every write of the trail's delivery, against what the account holds at
// that call's instant, and before anything is persisted.
#[path = "all/trail_destination_revalidation.rs"]
mod trail_destination_revalidation;
// `advanced_event_selector_rules` pins the rules an ADVANCED event selector set is held
// to — the category it has to state, the fields that category admits, the condition every
// field selector carries and the resource types a data-event selector names — and the
// fixed sentence each refusal carries, over every operation that stores such a set.
#[path = "all/advanced_event_selector_rules.rs"]
mod advanced_event_selector_rules;
// `identity_authorization` pins WHO may call this service: every operation is judged
// against the identity-based IAM policies in force on the principal the request's
// credential speaks for, for the action `cloudtrail:<OperationName>` on what the request
// addresses, before the operation acts on it.
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
