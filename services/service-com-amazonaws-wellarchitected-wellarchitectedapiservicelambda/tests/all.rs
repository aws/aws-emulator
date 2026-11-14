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
#[path = "all/agent_context_create_limits.rs"]
mod agent_context_create_limits;
#[path = "all/agent_contexts.rs"]
mod agent_contexts;
#[path = "all/agent_goal_listing.rs"]
mod agent_goal_listing;
#[path = "all/agent_goals.rs"]
mod agent_goals;
// Out of alphabetical order ON PURPOSE: declared next to the goal suites it extends
// rather than ahead of them, because the sorted position for this name falls in the one
// gap of this list that every new agent suite lands in, and an insertion there is what
// repeatedly refused to merge.
#[path = "all/agent_goal_create_limits.rs"]
mod agent_goal_create_limits;
// Out of alphabetical order for the reason the note above gives: declared after the
// create-limits suite rather than in the gap every new agent suite sorts into.
#[path = "all/agent_goal_updates.rs"]
mod agent_goal_updates;
#[path = "all/agent_profile_lifecycle.rs"]
mod agent_profile_lifecycle;
#[path = "all/agent_profile_listing.rs"]
mod agent_profile_listing;
#[path = "all/agent_recommendation_generations.rs"]
mod agent_recommendation_generations;
#[path = "all/answer_updates.rs"]
mod answer_updates;
#[path = "all/applied_lens_identity.rs"]
mod applied_lens_identity;
#[path = "all/check_details.rs"]
mod check_details;
#[path = "all/custom_lens_answer_writes.rs"]
mod custom_lens_answer_writes;
#[path = "all/custom_lens_import.rs"]
mod custom_lens_import;
#[path = "all/custom_lens_review_answers.rs"]
mod custom_lens_review_answers;
#[path = "all/custom_lens_review_rollup.rs"]
mod custom_lens_review_rollup;
#[path = "all/custom_lens_versions.rs"]
mod custom_lens_versions;
#[path = "all/delete_idempotency.rs"]
mod delete_idempotency;
#[path = "all/get_agent_context.rs"]
mod get_agent_context;
#[path = "all/get_agent_goal.rs"]
mod get_agent_goal;
#[path = "all/get_agent_profile.rs"]
mod get_agent_profile;
#[path = "all/get_agent_recommendation.rs"]
mod get_agent_recommendation;
#[path = "all/get_answer_reads.rs"]
mod get_answer_reads;
#[path = "all/get_lens.rs"]
mod get_lens;
#[path = "all/import_lens_token_parameters.rs"]
mod import_lens_token_parameters;
#[path = "all/lens_listing.rs"]
mod lens_listing;
#[path = "all/lens_resolution.rs"]
mod lens_resolution;
#[path = "all/lens_review_answers.rs"]
mod lens_review_answers;
#[path = "all/lens_review_improvements.rs"]
mod lens_review_improvements;
#[path = "all/lens_review_listing.rs"]
mod lens_review_listing;
#[path = "all/lens_review_notes.rs"]
mod lens_review_notes;
#[path = "all/lens_review_rollup.rs"]
mod lens_review_rollup;
#[path = "all/lens_share_deletion.rs"]
mod lens_share_deletion;
#[path = "all/lens_share_invitation_answers.rs"]
mod lens_share_invitation_answers;
#[path = "all/lens_shares.rs"]
mod lens_shares;
#[path = "all/lens_version_difference.rs"]
mod lens_version_difference;
#[path = "all/lookup_error_envelopes.rs"]
mod lookup_error_envelopes;
#[path = "all/milestone_listing.rs"]
mod milestone_listing;
#[path = "all/milestone_name_uniqueness.rs"]
mod milestone_name_uniqueness;
#[path = "all/no_op_updates.rs"]
mod no_op_updates;
#[path = "all/official_lens_catalog.rs"]
mod official_lens_catalog;
#[path = "all/official_lens_catalog_metadata.rs"]
mod official_lens_catalog_metadata;
#[path = "all/profile_deletion.rs"]
mod profile_deletion;
#[path = "all/profile_lifecycle.rs"]
mod profile_lifecycle;
#[path = "all/profile_listing.rs"]
mod profile_listing;
#[path = "all/profile_reads.rs"]
mod profile_reads;
#[path = "all/profile_shares.rs"]
mod profile_shares;
#[path = "all/question_choice_catalog.rs"]
mod question_choice_catalog;
#[path = "all/resource_tagging.rs"]
mod resource_tagging;
#[path = "all/resource_tags.rs"]
mod resource_tags;
#[path = "all/review_template_answer_reads.rs"]
mod review_template_answer_reads;
#[path = "all/review_template_answer_updates.rs"]
mod review_template_answer_updates;
#[path = "all/review_template_answers.rs"]
mod review_template_answers;
// Declared in its sorted position, beside the review-template suite it extends, rather
// than at the end of this list: the tail is where every new suite appends, and an
// insertion there is what repeatedly refused to merge.
#[path = "all/review_template_imported_lens_answers.rs"]
mod review_template_imported_lens_answers;
#[path = "all/review_template_deletion.rs"]
mod review_template_deletion;
#[path = "all/review_template_lens_review.rs"]
mod review_template_lens_review;
#[path = "all/review_template_lens_review_reads.rs"]
mod review_template_lens_review_reads;
#[path = "all/review_template_lens_review_upgrade.rs"]
mod review_template_lens_review_upgrade;
#[path = "all/review_template_listing.rs"]
mod review_template_listing;
#[path = "all/review_template_lifecycle.rs"]
mod review_template_lifecycle;
#[path = "all/share_invitation_answers.rs"]
mod share_invitation_answers;
#[path = "all/share_invitations.rs"]
mod share_invitations;
#[path = "all/state_roundtrip.rs"]
mod state_roundtrip;
#[path = "all/token_ledgers.rs"]
mod token_ledgers;
#[path = "all/untag_resource.rs"]
mod untag_resource;
#[path = "all/validation_envelopes.rs"]
mod validation_envelopes;
#[path = "all/workload_computed_members.rs"]
mod workload_computed_members;
#[path = "all/workload_harness.rs"]
mod workload_harness;
#[path = "all/workload_lens_association.rs"]
mod workload_lens_association;
#[path = "all/workload_lens_disassociation.rs"]
mod workload_lens_disassociation;
#[path = "all/workload_lifecycle.rs"]
mod workload_lifecycle;
#[path = "all/workload_profile_association.rs"]
mod workload_profile_association;
#[path = "all/workload_profile_disassociation.rs"]
mod workload_profile_disassociation;
#[path = "all/workload_profile_upgrade.rs"]
mod workload_profile_upgrade;
#[path = "all/workload_region_invariant.rs"]
mod workload_region_invariant;
#[path = "all/workload_share_permissions.rs"]
mod workload_share_permissions;
#[path = "all/workload_shares.rs"]
mod workload_shares;
// Out of alphabetical order ON PURPOSE, for the reason the `agent_goal_create_limits`
// declaration above gives: the sorted position for an agent-profile suite falls in the one
// repeatedly refused to merge. Declared last instead, where nothing else is inserting.
#[path = "all/agent_profile_updates.rs"]
mod agent_profile_updates;
// Out of alphabetical order for the same reason as the declaration above: the sorted
// position for an agent suite is the one gap of this list every new agent suite lands in.
#[path = "all/agent_recommendations.rs"]
mod agent_recommendations;
// Declared last rather than among the `profile_*` suites, for the reason the notes above
// give: an insertion into a crowded run of this list is what repeatedly refused to merge.
#[path = "all/profile_share_deletion.rs"]
mod profile_share_deletion;
// Declared last for the same reason as the declarations above.
#[path = "all/agent_recommendation_paging.rs"]
mod agent_recommendation_paging;
// Declared last for the same reason as the declarations above: an insertion into this
// list's sorted middle is what repeatedly refused to merge.
#[path = "all/global_settings.rs"]
mod global_settings;
#[path = "all/agent_context_updates.rs"]
mod agent_context_updates;
// Declared last for the same reason as the declarations above: modules are appended here
// so a new suite does not collide with another insertion.
#[path = "all/template_shares.rs"]
mod template_shares;
// Declared last for the same reason as the declarations above: the sorted position for a
// profile suite falls in a crowded run of this list.
#[path = "all/profile_revisions.rs"]
mod profile_revisions;
// workload suite falls in a crowded run of this list.
#[path = "all/workload_lens_review_upgrade.rs"]
mod workload_lens_review_upgrade;
// Declared last for the same reason as the declaration above.
#[path = "all/workload_integration_sync.rs"]
mod workload_integration_sync;
#[path = "all/profile_notifications.rs"]
mod profile_notifications;
// Declared last for the same reason as the declaration above: modules are appended here so
// a new suite does not collide with another insertion.
#[path = "all/review_template_write_instants.rs"]
mod review_template_write_instants;
#[path = "all/lens_notifications.rs"]
mod lens_notifications;
#[path = "all/template_share_deletion.rs"]
mod template_share_deletion;
#[path = "all/shared_resource_deletion.rs"]
mod shared_resource_deletion;
// review-template suite falls in a crowded run of this list.
#[path = "all/review_template_imported_lens_review.rs"]
mod review_template_imported_lens_review;
// Declared last for the same reason as the declarations above: the sorted position for an
// agent suite is the one gap of this list every new agent suite lands in.
#[path = "all/agent_context_deletion.rs"]
mod agent_context_deletion;
#[path = "all/create_agent_profile.rs"]
mod create_agent_profile;
// Declared last for the same reason as the declaration above: the sorted position for a
#[path = "all/review_template_imported_lens_answer_writes.rs"]
mod review_template_imported_lens_answer_writes;
#[path = "all/workload_create_token_parameters.rs"]
mod workload_create_token_parameters;
#[path = "all/workload_from_review_template_content.rs"]
mod workload_from_review_template_content;
#[path = "all/agent_recommendation_feedback.rs"]
mod agent_recommendation_feedback;
#[path = "all/workload_share_deletion.rs"]
mod workload_share_deletion;
#[path = "all/lens_review_pinned_version_content.rs"]
mod lens_review_pinned_version_content;
#[path = "all/profile_share_quota.rs"]
mod profile_share_quota;
// Declared last for the same reason as the declarations above: modules are appended here so
#[path = "all/workload_industry_type.rs"]
mod workload_industry_type;
// Appended at the end of this list rather than inserted among the other agent suites, for
// the reason the notes above give: an insertion into this list's sorted middle is what
#[path = "all/agent_recommendation_items.rs"]
mod agent_recommendation_items;
// Appended at the end of this list for the reason the notes above give: the sorted position
// for a review-template suite falls in a crowded run of this list.
#[path = "all/review_template_updates.rs"]
mod review_template_updates;
// lens suite falls in a crowded run of this list.
#[path = "all/lens_share_listing.rs"]
mod lens_share_listing;
// check suite falls beside the per-account suite it extends, in a crowded run of this list.
#[path = "all/check_summaries.rs"]
mod check_summaries;
// custom-lens suite falls in a crowded run of this list.
#[path = "all/custom_lens_deletion.rs"]
mod custom_lens_deletion;
// Appended at the end of this list rather than inserted among the `profile_*` suites, for
#[path = "all/profile_template.rs"]
mod profile_template;
// Appended at the end of this list for the reason the notes above give: an insertion into
// this list's sorted middle is what repeatedly refused to merge.
#[path = "all/shared_page_size.rs"]
mod shared_page_size;
#[path = "all/workload_paging_stability.rs"]
mod workload_paging_stability;
// the reason the notes above give: the sorted position for an agent suite is the one gap of
// this list every new agent suite lands in.
#[path = "all/agent_profile_tag_writes.rs"]
mod agent_profile_tag_writes;
// Appended at the end of this list for the reason the notes above give: the sorted
// position for a review-template share suite falls in a crowded run of this list.
#[path = "all/template_share_invitation_answers.rs"]
mod template_share_invitation_answers;
// for a lens suite falls in a crowded run of this list.
#[path = "all/lens_token_alias_spelling.rs"]
mod lens_token_alias_spelling;
// for a workload suite falls in a crowded run of this list.
#[path = "all/workload_pillar_order.rs"]
mod workload_pillar_order;
// Appended at the end of this list for the reason the notes above give: modules are appended
// here so a new suite does not collide with another insertion.
#[path = "all/workload_path_id_resolution.rs"]
mod workload_path_id_resolution;
#[path = "all/answer_choice_set_changes.rs"]
mod answer_choice_set_changes;
#[path = "all/lens_share_token_lens_identity.rs"]
mod lens_share_token_lens_identity;
#[path = "all/tag_family_envelopes.rs"]
mod tag_family_envelopes;
// for an agent suite is the one gap of this list every new agent suite lands in.
#[path = "all/agent_recommendation_status_idempotence.rs"]
mod agent_recommendation_status_idempotence;
#[path = "all/review_template_name_uniqueness.rs"]
mod review_template_name_uniqueness;
// Appended at the end of this list for the same reason as the declaration above.
#[path = "all/review_template_not_found_wording.rs"]
mod review_template_not_found_wording;
// Appended at the end of this list for the same reason as the declarations above: the
// sorted position for a workload suite falls in a crowded run of this list.
#[path = "all/workload_ftr_support.rs"]
mod workload_ftr_support;
// Appended at the end of this list for the same reason as the declaration above: modules
// are appended here so a new suite does not collide with another insertion.
#[path = "all/workload_lens_list_projections.rs"]
mod workload_lens_list_projections;
#[path = "all/unique_name_token_identity.rs"]
mod unique_name_token_identity;
#[path = "all/custom_lens_publish_chain.rs"]
mod custom_lens_publish_chain;
// Appended at the end of this list for the same reason as the declaration above: the sorted
// position for a workload suite falls in a crowded run of this list.
#[path = "all/workload_pillar_identity.rs"]
mod workload_pillar_identity;
#[path = "all/agent_no_op_updates.rs"]
mod agent_no_op_updates;
// Appended at the end of this list for the same reason as the declarations above: modules
#[path = "all/defaulted_member_token_identity.rs"]
mod defaulted_member_token_identity;
// position for a lens suite falls in a crowded run of this list.
#[path = "all/bare_lens_name_resolution.rs"]
mod bare_lens_name_resolution;
#[path = "all/next_token_spelling.rs"]
mod next_token_spelling;
#[path = "all/regional_resource_scope.rs"]
mod regional_resource_scope;
// position for a check suite falls beside the two check suites it spans.
#[path = "all/check_best_practice_agreement.rs"]
mod check_best_practice_agreement;
#[path = "all/lens_content_validation.rs"]
mod lens_content_validation;
#[path = "all/lens_catalog_identity.rs"]
mod lens_catalog_identity;
#[path = "all/lens_review_report.rs"]
mod lens_review_report;
#[path = "all/cleared_answer_risk.rs"]
mod cleared_answer_risk;
#[path = "all/submitted_name_conflict_echo.rs"]
mod submitted_name_conflict_echo;
#[path = "all/enum_refusal_wording.rs"]
mod enum_refusal_wording;
// Appended at the end for the reason the notes above give: the sorted position for a lens
// suite falls in a crowded run of this list.
#[path = "all/pillar_filter_refusal.rs"]
mod pillar_filter_refusal;
// Appended at the end for the reason the notes above give: an insertion into this list's
// sorted middle is what repeatedly refused to merge.
#[path = "all/schema_finding_wording.rs"]
mod schema_finding_wording;
// Appended at the end for the reason the notes above give: an insertion into this list's
// sorted middle is what repeatedly refused to merge.
#[path = "all/error_type_header_form.rs"]
mod error_type_header_form;
// Appended at the end for the reason the notes above give: an insertion into this list's
// sorted middle is what repeatedly refused to merge.
#[path = "all/consolidated_report.rs"]
mod consolidated_report;
