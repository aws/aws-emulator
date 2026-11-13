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
#[path = "all/authored_refusal_messages.rs"]
mod authored_refusal_messages;
#[path = "all/client_token_record_is_the_first_creates.rs"]
mod client_token_record_is_the_first_creates;
#[path = "all/client_token_replay.rs"]
mod client_token_replay;
#[path = "all/create_probe_tag_union_ceiling.rs"]
mod create_probe_tag_union_ceiling;
#[path = "all/delete_guards.rs"]
mod delete_guards;
#[path = "all/delete_probe_and_monitor_state.rs"]
mod delete_probe_and_monitor_state;
#[path = "all/draining_probe_tag_mutations.rs"]
mod draining_probe_tag_mutations;
#[path = "all/idempotent_replay_state.rs"]
mod idempotent_replay_state;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/list_monitors_no_position.rs"]
mod list_monitors_no_position;
#[path = "all/list_monitors_pagination.rs"]
mod list_monitors_pagination;
#[path = "all/monitor_and_probe.rs"]
mod monitor_and_probe;
#[path = "all/monitor_lifecycle_and_front_door.rs"]
mod monitor_lifecycle_and_front_door;
#[path = "all/monitor_modified_at_follows_probe_writes.rs"]
mod monitor_modified_at_follows_probe_writes;
#[path = "all/monitor_report_projection.rs"]
mod monitor_report_projection;
#[path = "all/monitor_state_follows_probe_activation.rs"]
mod monitor_state_follows_probe_activation;
#[path = "all/probe_report_projection.rs"]
mod probe_report_projection;
#[path = "all/probe_rules_precede_count_ceilings.rs"]
mod probe_rules_precede_count_ceilings;
#[path = "all/probe_source_subnet_resolution.rs"]
mod probe_source_subnet_resolution;
#[path = "all/probe_subnet_identity.rs"]
mod probe_subnet_identity;
#[path = "all/probe_tag_writes_move_the_monitor.rs"]
mod probe_tag_writes_move_the_monitor;
#[path = "all/probe_teardown.rs"]
mod probe_teardown;
#[path = "all/probes_per_monitor_limit.rs"]
mod probes_per_monitor_limit;
#[path = "all/probes_per_subnet_limit.rs"]
mod probes_per_subnet_limit;
#[path = "all/region_isolation.rs"]
mod region_isolation;
#[path = "all/reserved_tag_keys.rs"]
mod reserved_tag_keys;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/source_arn_grammar.rs"]
mod source_arn_grammar;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_count_ceiling_refusal.rs"]
mod tag_count_ceiling_refusal;
#[path = "all/tag_mutation_no_content.rs"]
mod tag_mutation_no_content;
#[path = "all/tagging.rs"]
mod tagging;
#[path = "all/update_monitor.rs"]
mod update_monitor;
#[path = "all/update_probe.rs"]
mod update_probe;
#[path = "all/validation_message_wording.rs"]
mod validation_message_wording;
