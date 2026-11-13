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
#[path = "all/approval_rule_content_required.rs"]
mod approval_rule_content_required;
#[path = "all/approval_rule_template_content_canonicalization.rs"]
mod approval_rule_template_content_canonicalization;
#[path = "all/approval_rule_template_content_update.rs"]
mod approval_rule_template_content_update;
#[path = "all/approval_rule_template_deletion.rs"]
mod approval_rule_template_deletion;
#[path = "all/approval_rule_template_description.rs"]
mod approval_rule_template_description;
#[path = "all/approval_rule_template_disassociation.rs"]
mod approval_rule_template_disassociation;
#[path = "all/approval_rule_template_rename.rs"]
mod approval_rule_template_rename;
#[path = "all/approval_rule_templates.rs"]
mod approval_rule_templates;
#[path = "all/approval_rules_and_commits.rs"]
mod approval_rules_and_commits;
#[path = "all/aws_error_message_text.rs"]
mod aws_error_message_text;
#[path = "all/batch_associate_approval_rule_template.rs"]
mod batch_associate_approval_rule_template;
#[path = "all/batch_disassociate_approval_rule_template.rs"]
mod batch_disassociate_approval_rule_template;
#[path = "all/batch_get_repositories.rs"]
mod batch_get_repositories;
#[path = "all/batch_merge_conflict_descriptions.rs"]
mod batch_merge_conflict_descriptions;
#[path = "all/blob_content.rs"]
mod blob_content;
#[path = "all/blob_differences.rs"]
mod blob_differences;
#[path = "all/branch_create.rs"]
mod branch_create;
#[path = "all/branch_creation.rs"]
mod branch_creation;
#[path = "all/branch_delete.rs"]
mod branch_delete;
#[path = "all/branch_listing.rs"]
mod branch_listing;
#[path = "all/branch_listing_writes.rs"]
mod branch_listing_writes;
#[path = "all/caller_credential_identity.rs"]
mod caller_credential_identity;
#[path = "all/caller_identity_agrees_with_sts.rs"]
mod caller_identity_agrees_with_sts;
#[path = "all/caller_identity_matches_sts.rs"]
mod caller_identity_matches_sts;
#[path = "all/client_error_messages.rs"]
mod client_error_messages;
#[path = "all/client_request_token_validation.rs"]
mod client_request_token_validation;
#[path = "all/comment_content_deletion.rs"]
mod comment_content_deletion;
#[path = "all/comment_deleted_refusal_templates.rs"]
mod comment_deleted_refusal_templates;
#[path = "all/comment_deleted_refusals.rs"]
mod comment_deleted_refusals;
#[path = "all/comment_id_form.rs"]
mod comment_id_form;
#[path = "all/comment_lifecycle.rs"]
mod comment_lifecycle;
#[path = "all/comment_reaction_surfacing.rs"]
mod comment_reaction_surfacing;
#[path = "all/comment_reactions_read.rs"]
mod comment_reactions_read;
#[path = "all/comment_replies.rs"]
mod comment_replies;
#[path = "all/comment_thread_traversal.rs"]
mod comment_thread_traversal;
#[path = "all/commit_date_identity.rs"]
mod commit_date_identity;
#[path = "all/commit_file_entry_bound.rs"]
mod commit_file_entry_bound;
#[path = "all/commit_folder_and_diff_members.rs"]
mod commit_folder_and_diff_members;
#[path = "all/commit_identity_from_caller.rs"]
mod commit_identity_from_caller;
#[path = "all/commit_reads.rs"]
mod commit_reads;
#[path = "all/compared_commit_comments.rs"]
mod compared_commit_comments;
#[path = "all/default_branch.rs"]
mod default_branch;
#[path = "all/describe_merge_conflicts.rs"]
mod describe_merge_conflicts;
#[path = "all/differences.rs"]
mod differences;
#[path = "all/file_commit_history.rs"]
mod file_commit_history;
#[path = "all/file_deletion.rs"]
mod file_deletion;
#[path = "all/file_does_not_exist_message.rs"]
mod file_does_not_exist_message;
#[path = "all/folder_collection_members.rs"]
mod folder_collection_members;
#[path = "all/folder_listing.rs"]
mod folder_listing;
#[path = "all/git_object_and_commit_specifier_messages.rs"]
mod git_object_and_commit_specifier_messages;
#[path = "all/git_object_ids.rs"]
mod git_object_ids;
#[path = "all/git_push_authorization.rs"]
mod git_push_authorization;
#[path = "all/git_transport.rs"]
mod git_transport;
#[path = "all/git_transport_body_bound.rs"]
mod git_transport_body_bound;
#[path = "all/harness.rs"]
mod harness;
#[path = "all/manual_merge_required_message.rs"]
mod manual_merge_required_message;
#[path = "all/merge_branches_fast_forward.rs"]
mod merge_branches_fast_forward;
#[path = "all/merge_branches_squash.rs"]
mod merge_branches_squash;
#[path = "all/merge_branches_three_way.rs"]
mod merge_branches_three_way;
#[path = "all/merge_commit_read.rs"]
mod merge_commit_read;
#[path = "all/merge_conflicts.rs"]
mod merge_conflicts;
#[path = "all/merge_options.rs"]
mod merge_options;
#[path = "all/merge_pull_request_fast_forward.rs"]
mod merge_pull_request_fast_forward;
#[path = "all/merge_pull_request_repeat.rs"]
mod merge_pull_request_repeat;
#[path = "all/merge_pull_request_squash.rs"]
mod merge_pull_request_squash;
#[path = "all/merge_pull_request_three_way.rs"]
mod merge_pull_request_three_way;
#[path = "all/merge_strategy_and_resolution_rules.rs"]
mod merge_strategy_and_resolution_rules;
#[path = "all/object_store_bounds.rs"]
mod object_store_bounds;
#[path = "all/path_depth_bound.rs"]
mod path_depth_bound;
#[path = "all/pull_request_approval_rule_evaluation.rs"]
mod pull_request_approval_rule_evaluation;
#[path = "all/pull_request_approval_rule_override.rs"]
mod pull_request_approval_rule_override;
#[path = "all/pull_request_approval_rules.rs"]
mod pull_request_approval_rules;
#[path = "all/pull_request_approval_state.rs"]
mod pull_request_approval_state;
#[path = "all/pull_request_approval_states_read.rs"]
mod pull_request_approval_states_read;
#[path = "all/pull_request_closed_refusal_messages.rs"]
mod pull_request_closed_refusal_messages;
#[path = "all/pull_request_comment_reads.rs"]
mod pull_request_comment_reads;
#[path = "all/pull_request_comments.rs"]
mod pull_request_comments;
#[path = "all/pull_request_creation.rs"]
mod pull_request_creation;
#[path = "all/pull_request_does_not_exist_message.rs"]
mod pull_request_does_not_exist_message;
#[path = "all/pull_request_events.rs"]
mod pull_request_events;
#[path = "all/pull_request_identifier_refusals.rs"]
mod pull_request_identifier_refusals;
#[path = "all/pull_request_listing.rs"]
mod pull_request_listing;
#[path = "all/pull_request_lookup_precedence.rs"]
mod pull_request_lookup_precedence;
#[path = "all/pull_request_override_state_read.rs"]
mod pull_request_override_state_read;
#[path = "all/pull_request_status_update.rs"]
mod pull_request_status_update;
#[path = "all/pull_request_text_update.rs"]
mod pull_request_text_update;
#[path = "all/pull_request_title_update.rs"]
mod pull_request_title_update;
#[path = "all/pull_request_token_required.rs"]
mod pull_request_token_required;
#[path = "all/refusal_message_shape.rs"]
mod refusal_message_shape;
#[path = "all/repositories_for_approval_rule_template.rs"]
mod repositories_for_approval_rule_template;
#[path = "all/repository_description.rs"]
mod repository_description;
#[path = "all/repository_encryption_key.rs"]
mod repository_encryption_key;
#[path = "all/repository_encryption_key_projection.rs"]
mod repository_encryption_key_projection;
#[path = "all/repository_lifecycle.rs"]
mod repository_lifecycle;
#[path = "all/repository_name_and_system_tag_message_text.rs"]
mod repository_name_and_system_tag_message_text;
#[path = "all/repository_rename.rs"]
mod repository_rename;
#[path = "all/repository_triggers.rs"]
mod repository_triggers;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/rule_content_sha256_precondition_message.rs"]
mod rule_content_sha256_precondition_message;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/unreferenced_merge_commits.rs"]
mod unreferenced_merge_commits;
#[path = "all/wire.rs"]
mod wire;
