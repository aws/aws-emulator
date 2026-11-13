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
#[path = "all/absent_required_query_parameter.rs"]
mod absent_required_query_parameter;
#[path = "all/app_authorization_and_ingestion_dependency.rs"]
mod app_authorization_and_ingestion_dependency;
#[path = "all/app_authorization_auth_url.rs"]
mod app_authorization_auth_url;
#[path = "all/app_authorization_connect_validation.rs"]
mod app_authorization_connect_validation;
#[path = "all/app_authorization_deletion.rs"]
mod app_authorization_deletion;
#[path = "all/app_authorization_refusal_error_class.rs"]
mod app_authorization_refusal_error_class;
#[path = "all/app_authorization_tenant_move_precondition.rs"]
mod app_authorization_tenant_move_precondition;
#[path = "all/app_bundle_delete_precondition.rs"]
mod app_bundle_delete_precondition;
#[path = "all/app_bundle_ingestion_lifecycle.rs"]
mod app_bundle_ingestion_lifecycle;
#[path = "all/app_bundle_refusal_wording.rs"]
mod app_bundle_refusal_wording;
#[path = "all/app_bundle_region_quota.rs"]
mod app_bundle_region_quota;
#[path = "all/audit_record_resource_identity.rs"]
mod audit_record_resource_identity;
#[path = "all/bundle_scope_resolved_before_body_rules.rs"]
mod bundle_scope_resolved_before_body_rules;
#[path = "all/chain_read_not_found_attribution.rs"]
mod chain_read_not_found_attribution;
#[path = "all/client_token_region_scope.rs"]
mod client_token_region_scope;
#[path = "all/connected_authorization_rule.rs"]
mod connected_authorization_rule;
#[path = "all/create_app_authorization_contract.rs"]
mod create_app_authorization_contract;
#[path = "all/create_app_authorization_credential_idempotency.rs"]
mod create_app_authorization_credential_idempotency;
#[path = "all/created_resource_tag_set.rs"]
mod created_resource_tag_set;
#[path = "all/idempotent_replay_before_preconditions.rs"]
mod idempotent_replay_before_preconditions;
#[path = "all/identifier_resolution_rule.rs"]
mod identifier_resolution_rule;
#[path = "all/ingestion_authorization_refusal_wording.rs"]
mod ingestion_authorization_refusal_wording;
#[path = "all/ingestion_deletion.rs"]
mod ingestion_deletion;
#[path = "all/ingestion_destination_accepted_status.rs"]
mod ingestion_destination_accepted_status;
#[path = "all/ingestion_destination_lifecycle.rs"]
mod ingestion_destination_lifecycle;
#[path = "all/ingestion_destination_loss.rs"]
mod ingestion_destination_loss;
#[path = "all/ingestion_enablement.rs"]
mod ingestion_enablement;
#[path = "all/nested_resource_arn_region.rs"]
mod nested_resource_arn_region;
#[path = "all/oauth2_auth_type_predicate.rs"]
mod oauth2_auth_type_predicate;
#[path = "all/oauth2_client_id_registration.rs"]
mod oauth2_client_id_registration;
#[path = "all/page_token_authenticity.rs"]
mod page_token_authenticity;
#[path = "all/pagination_token_lifetime.rs"]
mod pagination_token_lifetime;
#[path = "all/repeat_connect_dates_nothing.rs"]
mod repeat_connect_dates_nothing;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/spent_client_token_decision.rs"]
mod spent_client_token_decision;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/supported_application_names.rs"]
mod supported_application_names;
#[path = "all/tagging_refusal_resource_type.rs"]
mod tagging_refusal_resource_type;
#[path = "all/update_app_authorization_contract.rs"]
mod update_app_authorization_contract;
#[path = "all/user_access_task_reporting.rs"]
mod user_access_task_reporting;
#[path = "all/user_access_tasks.rs"]
mod user_access_tasks;
#[path = "all/validation_refusal_shape.rs"]
mod validation_refusal_shape;
