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
#[path = "all/channel_configuration_lookup_order.rs"]
mod channel_configuration_lookup_order;
#[path = "all/channel_configuration_pair_lookup.rs"]
mod channel_configuration_pair_lookup;
#[path = "all/channel_configuration_workspace_precondition.rs"]
mod channel_configuration_workspace_precondition;
#[path = "all/channel_role_account.rs"]
mod channel_role_account;
#[path = "all/channel_role_name_pattern.rs"]
mod channel_role_name_pattern;
#[path = "all/channel_role_pass_role_form.rs"]
mod channel_role_pass_role_form;
#[path = "all/console_authorized_workspace.rs"]
mod console_authorized_workspace;
#[path = "all/delete_slack_workspace_configuration.rs"]
mod delete_slack_workspace_configuration;
#[path = "all/error_message_templates.rs"]
mod error_message_templates;
#[path = "all/handler_validation_refusal_shape.rs"]
mod handler_validation_refusal_shape;
#[path = "all/identity_authorization.rs"]
mod identity_authorization;
#[path = "all/listing_pagination.rs"]
mod listing_pagination;
#[path = "all/notification_flag_default.rs"]
mod notification_flag_default;
#[path = "all/organization_account_role.rs"]
mod organization_account_role;
#[path = "all/organization_membership.rs"]
mod organization_membership;
#[path = "all/resource_preconditions.rs"]
mod resource_preconditions;
#[path = "all/slack_channel_configurations.rs"]
mod slack_channel_configurations;
#[path = "all/slack_workspace_configurations.rs"]
mod slack_workspace_configurations;
#[path = "all/slack_workspace_team_name.rs"]
mod slack_workspace_team_name;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/update_severity_rule_scope.rs"]
mod update_severity_rule_scope;
#[path = "all/workspace_registry_presence.rs"]
mod workspace_registry_presence;
