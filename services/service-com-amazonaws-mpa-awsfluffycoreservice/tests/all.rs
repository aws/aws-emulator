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
#[path = "all/absent_approval_team_version_delete.rs"]
mod absent_approval_team_version_delete;
#[path = "all/active_team_deletion_approval.rs"]
mod active_team_deletion_approval;
#[path = "all/approval_sessions.rs"]
mod approval_sessions;
#[path = "all/approval_team_lifecycle.rs"]
mod approval_team_lifecycle;
#[path = "all/approver_identity_source_arn.rs"]
mod approver_identity_source_arn;
#[path = "all/approver_mfa_synchronization.rs"]
mod approver_mfa_synchronization;
#[path = "all/arn_addressed_resource_inventory.rs"]
mod arn_addressed_resource_inventory;
#[path = "all/authorization_gate.rs"]
mod authorization_gate;
#[path = "all/baseline_session_minting.rs"]
mod baseline_session_minting;
#[path = "all/clock_due_instant_arithmetic.rs"]
mod clock_due_instant_arithmetic;
#[path = "all/clock_due_order.rs"]
mod clock_due_order;
#[path = "all/clock_owed_order.rs"]
mod clock_owed_order;
#[path = "all/create_tag_ceiling.rs"]
mod create_tag_ceiling;
#[path = "all/filtered_listing_pagination.rs"]
mod filtered_listing_pagination;
#[path = "all/handshake_live_version_identity.rs"]
mod handshake_live_version_identity;
#[path = "all/idempotent_create_replay.rs"]
mod idempotent_create_replay;
#[path = "all/identity_source_approval_portal.rs"]
mod identity_source_approval_portal;
#[path = "all/identity_source_arn_addressing.rs"]
mod identity_source_arn_addressing;
#[path = "all/identity_source_deletion.rs"]
mod identity_source_deletion;
#[path = "all/identity_source_detail.rs"]
mod identity_source_detail;
#[path = "all/identity_source_id_form.rs"]
mod identity_source_id_form;
#[path = "all/identity_source_in_use_by_pending_update.rs"]
mod identity_source_in_use_by_pending_update;
#[path = "all/identity_source_region.rs"]
mod identity_source_region;
#[path = "all/identity_source_removal_in_use.rs"]
mod identity_source_removal_in_use;
#[path = "all/identity_sources.rs"]
mod identity_sources;
#[path = "all/inactive_team_version_deletion.rs"]
mod inactive_team_version_deletion;
#[path = "all/key_token_pagination.rs"]
mod key_token_pagination;
#[path = "all/management_account_gate_order.rs"]
mod management_account_gate_order;
#[path = "all/pagination_token_provenance.rs"]
mod pagination_token_provenance;
#[path = "all/pagination_token_spelling.rs"]
mod pagination_token_spelling;
#[path = "all/pending_baseline_session_uniqueness.rs"]
mod pending_baseline_session_uniqueness;
#[path = "all/pending_team_update.rs"]
mod pending_team_update;
#[path = "all/policy_attachment_status.rs"]
mod policy_attachment_status;
#[path = "all/policy_catalog.rs"]
mod policy_catalog;
#[path = "all/policy_version_detail.rs"]
mod policy_version_detail;
#[path = "all/policy_versions.rs"]
mod policy_versions;
#[path = "all/published_catalog_and_iso_timestamps.rs"]
mod published_catalog_and_iso_timestamps;
#[path = "all/refusal_message_bound.rs"]
mod refusal_message_bound;
#[path = "all/refused_write_mints_nothing.rs"]
mod refused_write_mints_nothing;
#[path = "all/rejection_wording.rs"]
mod rejection_wording;
#[path = "all/reserved_tag_namespace.rs"]
mod reserved_tag_namespace;
#[path = "all/resource_arn_reads.rs"]
mod resource_arn_reads;
#[path = "all/resource_scoped_listing_refusal_order.rs"]
mod resource_scoped_listing_refusal_order;
#[path = "all/resource_policies.rs"]
mod resource_policies;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/seq_token_pagination.rs"]
mod seq_token_pagination;
#[path = "all/session_arn_nondisclosure.rs"]
mod session_arn_nondisclosure;
#[path = "all/session_cancellation.rs"]
mod session_cancellation;
#[path = "all/session_execution_status.rs"]
mod session_execution_status;
#[path = "all/session_filter_value_bound.rs"]
mod session_filter_value_bound;
#[path = "all/session_listing.rs"]
mod session_listing;
#[path = "all/session_listing_team_description.rs"]
mod session_listing_team_description;
#[path = "all/session_store_ceiling.rs"]
mod session_store_ceiling;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/tag_write_rule_order.rs"]
mod tag_write_rule_order;
#[path = "all/update_while_a_version_is_outstanding.rs"]
mod update_while_a_version_is_outstanding;
#[path = "all/version_delete_refusal_statuses.rs"]
mod version_delete_refusal_statuses;
#[path = "all/write_path_input_validation.rs"]
mod write_path_input_validation;
