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
#[path = "all/backup_job_listing.rs"]
mod backup_job_listing;
#[path = "all/backup_job_resource_resolution.rs"]
mod backup_job_resource_resolution;
#[path = "all/backup_job_role_outcome.rs"]
mod backup_job_role_outcome;
#[path = "all/backup_plan.rs"]
mod backup_plan;
#[path = "all/backup_plan_deletion.rs"]
mod backup_plan_deletion;
#[path = "all/backup_plan_update.rs"]
mod backup_plan_update;
#[path = "all/backup_selection.rs"]
mod backup_selection;
#[path = "all/copy_job.rs"]
mod copy_job;
#[path = "all/error_bodies.rs"]
mod error_bodies;
#[path = "all/framework.rs"]
mod framework;
#[path = "all/framework_listing.rs"]
mod framework_listing;
#[path = "all/framework_refusals.rs"]
mod framework_refusals;
#[path = "all/framework_update.rs"]
mod framework_update;
#[path = "all/framework_update_idempotency.rs"]
mod framework_update_idempotency;
#[path = "all/job_state_change_events.rs"]
mod job_state_change_events;
#[path = "all/legal_hold.rs"]
mod legal_hold;
#[path = "all/legal_hold_cancel_state_refusal.rs"]
mod legal_hold_cancel_state_refusal;
#[path = "all/legal_hold_coverage.rs"]
mod legal_hold_coverage;
#[path = "all/legal_hold_date_range.rs"]
mod legal_hold_date_range;
#[path = "all/legal_hold_vault_selection.rs"]
mod legal_hold_vault_selection;
#[path = "all/logically_air_gapped_vault.rs"]
mod logically_air_gapped_vault;
#[path = "all/mpa_approval_team_disassociation.rs"]
mod mpa_approval_team_disassociation;
#[path = "all/plan_and_selection_refusals.rs"]
mod plan_and_selection_refusals;
#[path = "all/recovery_point_cross_account_authorization.rs"]
mod recovery_point_cross_account_authorization;
#[path = "all/recovery_point_deletion.rs"]
mod recovery_point_deletion;
#[path = "all/recovery_point_denial_sentence.rs"]
mod recovery_point_denial_sentence;
#[path = "all/recovery_point_read.rs"]
mod recovery_point_read;
#[path = "all/restore_access_vault.rs"]
mod restore_access_vault;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/restore_job.rs"]
mod restore_job;
#[path = "all/restore_job_listing.rs"]
mod restore_job_listing;
#[path = "all/restore_job_state_change_events.rs"]
mod restore_job_state_change_events;
#[path = "all/restore_metadata_validation.rs"]
mod restore_metadata_validation;
#[path = "all/restore_testing_plan.rs"]
mod restore_testing_plan;
#[path = "all/scheduled_runs_preview.rs"]
mod scheduled_runs_preview;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supported_resource_types.rs"]
mod supported_resource_types;
#[path = "all/tiering_configuration.rs"]
mod tiering_configuration;
#[path = "all/vault_and_backup_job.rs"]
mod vault_and_backup_job;
#[path = "all/vault_access_policy.rs"]
mod vault_access_policy;
#[path = "all/vault_lock_refusal_order.rs"]
mod vault_lock_refusal_order;
#[path = "all/vault_read_projection.rs"]
mod vault_read_projection;
#[path = "all/vault_read_visibility.rs"]
mod vault_read_visibility;
#[path = "all/vault_region_namespace.rs"]
mod vault_region_namespace;
#[path = "all/vault_type_and_parameter_refusals.rs"]
mod vault_type_and_parameter_refusals;
#[path = "all/plan_driven_job_event_delivery.rs"]
mod plan_driven_job_event_delivery;
