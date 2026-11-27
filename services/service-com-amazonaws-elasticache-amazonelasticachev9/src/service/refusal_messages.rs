// SPDX-License-Identifier: CC0-1.0

//! Refusal sentences more than one handler of this service states.
//!
//! Each one is the whole difference a client sees for its class of refusal, and each is
//! raised by several handlers, so it is worded here once rather than in each of them.
//!
//! `InvalidUserGroupStateFault` is "the user group is not in an active state", and several
//! handlers answer it — the change to a user group itself, and the changes and creates that
//! ATTACH one to another resource — so the sentence is stated here once rather than in
//! each of them. Its words are the whole difference a client sees: the message is the only
//! place the service reports WHICH status blocked the call, which is what a caller racing
//! two changes, or waiting to retry, reads.
//!
//! The service states such a gate as two sentences: the group's id as the request named it
//! and the status the group carries now as a describe reports it, then the status that
//! class of call admits. The sibling gate for a DELETE states the same shape over the two
//! statuses a delete admits ([`crate::state::user_group_delete_state_refusal`]). Neither
//! template carries anything from one call's values.

use crate::state::{UserGroupRecord, DEFAULT_USER_ID, STATUS_DELETING, USER_STATUS_ACTIVE};

/// How this service refuses a call that would CHANGE the built-in `default`
/// user: `default user can't be <action>.`
///
/// `action` is what the call would have done to it (`deleted`, `modified`). The
/// built-in user governs the access every RBAC user group gives a client that
/// authenticates as nobody in particular, so the service keeps it as it ships
/// and the two handlers that could change it refuse with this one sentence. The
/// id in it is the built-in user's own ([`DEFAULT_USER_ID`]) rather than the
/// characters the request wrote it with, so every spelling of that one id is
/// answered the same way.
pub fn built_in_user_refusal(action: &str) -> String {
    format!("{DEFAULT_USER_ID} user can't be {action}.")
}

/// Why `group` cannot take part in a change at this instant, or `None` when it can.
///
/// A group still being created, already carrying a change the clock has not settled, or on
/// its way out takes no further change; the refusal reports the group's own id and its
/// current status, both read off the record the store holds.
pub fn user_group_change_state_refusal(group: &UserGroupRecord) -> Option<String> {
    (group.status != USER_STATUS_ACTIVE).then(|| user_group_state_sentence(group))
}

/// Why `group` cannot be ATTACHED to a resource a create is provisioning, or `None` when it
/// can.
///
/// A narrower gate than a change to the group itself: a group still coming up can be
/// attached, because the create provisions its own resources over a span of its own and the
/// two settle side by side, while a group on its way OUT is about to stop holding anybody —
/// which is the state `InvalidUserGroupStateFault` ("the user group is not in an active
/// state") refuses. The sentence is the same one a blocked change states, so a client reads
/// one wording for the status that blocked its call.
pub fn user_group_attachment_state_refusal(group: &UserGroupRecord) -> Option<String> {
    (group.status == STATUS_DELETING).then(|| user_group_state_sentence(group))
}

/// The sentence both gates above state: the group's own id and the status it carries now,
/// then the status this class of call admits.
fn user_group_state_sentence(group: &UserGroupRecord) -> String {
    format!(
        "User group {} has status {}. Only {} user groups can be modified.",
        group.user_group_id, group.status, USER_STATUS_ACTIVE
    )
}
