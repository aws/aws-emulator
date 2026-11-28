// SPDX-License-Identifier: CC0-1.0

//! The background task `ReplacePermissionAssociations` starts, and how it moves the
//! resource shares that task names onto another managed permission.
//!
//! `ReplacePermissionAssociations` "updates all resource shares that use a managed
//! permission to a different managed permission", and it does so ASYNCHRONOUSLY: the
//! call answers with "a data structure that you can use to track the asynchronous tasks
//! that RAM performs to complete this operation"
//! (`ReplacePermissionAssociationsResponse$replacePermissionAssociationsWork`), whose
//! `status` is one of `IN_PROGRESS`, `COMPLETED` and `FAILED`
//! (`ReplacePermissionAssociationsWork$status`). So the call does not itself move the
//! shares: it records a task that still has them to move, answers it in the non-terminal
//! status, and the move — the task reaching `COMPLETED` and every share it named being
//! governed by the new permission — is what the clock owes
//! ([`REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS`]). Nothing sleeps for it:
//! the due instant is stored on the record and
//! [`ComAmazonawsRamAmazonresourcesharingState::settle_due_permission_replacements`]
//! materialises it on the first call at or after it.
//!
//! The task is kept beside the permissions so `ListReplacePermissionAssociationsWork`
//! can report what became of it, keyed by the id the call handed back.

use super::{
    decode_cursor, ComAmazonawsRamAmazonresourcesharingState, OwedMove, SharePermissionRecord,
};
use crate::ops::types::ReplacePermissionAssociationsWork;

/// The `ReplacePermissionAssociationsWorkStatus` a task carries while it still has
/// shares to move: "IN_PROGRESS" (`ReplacePermissionAssociationsWork$status`).
pub const REPLACE_WORK_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The `ReplacePermissionAssociationsWorkStatus` a task carries once every share it
/// named has been moved: "COMPLETED" (`ReplacePermissionAssociationsWork$status`).
pub const REPLACE_WORK_STATUS_COMPLETED: &str = "COMPLETED";

/// How long after the call that starts it RAM takes to complete one
/// `ReplacePermissionAssociations` background task.
///
/// The call answers with the task in the non-terminal `IN_PROGRESS` status, so reaching
/// `COMPLETED` is a move the clock owes rather than something a caller asks for. AWS
/// publishes no duration for it, so this span is this emulator's own choice; it is
/// deliberately not zero, so a caller that reads straight back still observes a task in
/// flight and the shares it has not moved yet.
pub const REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS: i64 = 3_000;

/// The most `ReplacePermissionAssociations` tasks this emulator keeps.
///
/// Emulator resource guard, NOT an AWS quota. A task is kept after it completes so a
/// read can still report it, and nothing else reclaims one, so at the cap the oldest
/// COMPLETED task is forgotten. A task still in flight is never dropped: the shares it
/// has to move are on it.
pub const MAX_REPLACE_PERMISSION_ASSOCIATIONS_WORKS: usize = 500;

/// One `ReplacePermissionAssociations` background task.
#[derive(Debug, Clone)]
pub struct ReplaceWorkRecord {
    /// "The unique identifier for the background task associated with one
    /// ReplacePermissionAssociations request" (`ReplacePermissionAssociationsWork$id`),
    /// which is also the key this record is held under.
    pub id: String,
    /// The managed permission this task is replacing.
    pub from_permission_arn: String,
    /// The version of it the request restricted the task to, or `None` when the task
    /// moves every share governed by the permission whatever version it attached.
    pub from_permission_version: Option<i32>,
    /// The managed permission this task is associating with the shares instead.
    pub to_permission_arn: String,
    /// The version of that permission the task associates: its default version, because
    /// the operation "always applies the default version of the target managed
    /// permission" — resolved when the task was created, which is the version the call
    /// reported.
    pub to_permission_version: i32,
    /// Where the task has got to: [`REPLACE_WORK_STATUS_IN_PROGRESS`], then
    /// [`REPLACE_WORK_STATUS_COMPLETED`].
    pub status: String,
    /// When the task was created, in epoch milliseconds.
    pub creation_time: i64,
    /// When its status was last updated, in epoch milliseconds.
    pub last_updated_time: i64,
    /// The shares this task still has to move, resolved when it was created: the live
    /// shares then governed by [`Self::from_permission_arn`] by name, at
    /// [`Self::from_permission_version`] when the request named one. Empty once the task
    /// has run.
    pub pending_shares: Vec<String>,
    /// The move to [`REPLACE_WORK_STATUS_COMPLETED`] the clock owes, or `None` once it
    /// has been made.
    pub owed_move: Option<OwedMove>,
}

impl ReplaceWorkRecord {
    /// This task as the modelled `ReplacePermissionAssociationsWork` wire shape.
    ///
    /// Both version members are modelled as strings
    /// (`smithy_query(query="shape",shape="ReplacePermissionAssociationsWork")`), so the
    /// numbers are rendered as such, and `statusMessage` "is present only when there
    /// status is FAILED" — which a task this emulator runs never is, because the moves
    /// it makes are the ones the store already resolved.
    pub fn view(&self) -> ReplacePermissionAssociationsWork {
        ReplacePermissionAssociationsWork {
            id: Some(self.id.clone()),
            from_permission_arn: Some(self.from_permission_arn.clone()),
            from_permission_version: self
                .from_permission_version
                .map(|version| version.to_string()),
            to_permission_arn: Some(self.to_permission_arn.clone()),
            to_permission_version: Some(self.to_permission_version.to_string()),
            // The model declares the template parameters the replacement applied; a
            // permission this slice holds declares none, so there is nothing to report.
            permission_template_parameters: None,
            status: Some(self.status.clone()),
            status_message: None,
            creation_time: Some(super::to_date_time(self.creation_time)),
            last_updated_time: Some(super::to_date_time(self.last_updated_time)),
        }
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// The background task `id` names, whatever its status.
    pub fn replace_permission_associations_work(&self, id: &str) -> Option<&ReplaceWorkRecord> {
        self.replace_permission_associations_works.get(id)
    }

    /// Record the background task that moves every live resource share governed by
    /// `from_permission_arn` — at `from_permission_version` when the request named one —
    /// onto `to_permission_arn` at `to_permission_version`, and answer it.
    ///
    /// The shares to move are resolved HERE, from the attachments the store holds at the
    /// call, so a share attached to the old permission afterwards is not swept up by a
    /// task created before it. `None` means this emulator is already holding
    /// [`MAX_REPLACE_PERMISSION_ASSOCIATIONS_WORKS`] tasks that are all still in flight,
    /// so there is no room to record another.
    pub fn start_permission_replacement(
        &mut self,
        from_permission_arn: &str,
        from_permission_version: Option<i32>,
        to_permission_arn: &str,
        to_permission_version: i32,
        now: i64,
    ) -> Option<ReplaceWorkRecord> {
        // The shares governed by the permission the task replaces: the ones holding it
        // by name, which is what an attachment a replacement can move is.
        let pending_shares = match from_permission_version {
            Some(version) => {
                self.shares_attaching_permission_version(from_permission_arn, version)
            }
            None => self.shares_attaching_permission(from_permission_arn),
        };
        if !self.make_room_for_replacement_work() {
            return None;
        }
        let id = self.fresh_replacement_work_id();
        let record = ReplaceWorkRecord {
            id: id.clone(),
            from_permission_arn: from_permission_arn.to_string(),
            from_permission_version,
            to_permission_arn: to_permission_arn.to_string(),
            to_permission_version,
            status: REPLACE_WORK_STATUS_IN_PROGRESS.to_string(),
            creation_time: now,
            last_updated_time: now,
            pending_shares,
            owed_move: Some(OwedMove {
                status: REPLACE_WORK_STATUS_COMPLETED.to_string(),
                due_at: now + REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS,
            }),
        };
        self.replace_permission_associations_works
            .insert(id, record.clone());
        Some(record)
    }

    /// Run every background task the clock owed at or before `now`: each one moves the
    /// shares it still has to move onto the permission it names and reaches
    /// [`REPLACE_WORK_STATUS_COMPLETED`].
    ///
    /// Like [`Self::settle_due_associations`], this only materialises what was already
    /// due: the task runs [`REPLACE_PERMISSION_ASSOCIATIONS_COMPLETED_AFTER_MILLIS`]
    /// after the call that started it whether or not anybody asks. A share the task
    /// named that is gone, or that is no longer governed by the permission the task
    /// replaces, is left alone: what the task moves is the attachment it was created
    /// for, and a later call that changed that attachment is the more recent decision.
    pub fn settle_due_permission_replacements(&mut self, now: i64) {
        let due: Vec<String> = self
            .replace_permission_associations_works
            .values()
            .filter(|work| {
                work.owed_move
                    .as_ref()
                    .is_some_and(|owed| owed.due_at <= now)
            })
            .map(|work| work.id.clone())
            .collect();
        for id in due {
            let Some(work) = self.replace_permission_associations_works.get_mut(&id) else {
                continue;
            };
            let Some(owed) = work.owed_move.take() else {
                continue;
            };
            work.status = owed.status;
            // The task ran when it fell due, not when somebody noticed.
            work.last_updated_time = owed.due_at;
            let from_permission_arn = work.from_permission_arn.clone();
            let from_permission_version = work.from_permission_version;
            let to_permission_arn = work.to_permission_arn.clone();
            let to_permission_version = work.to_permission_version;
            let shares = std::mem::take(&mut work.pending_shares);
            for share_arn in shares {
                let Some(share) = self.live_share_mut(&share_arn) else {
                    continue;
                };
                let moves = share
                    .attached_permissions
                    .get(&from_permission_arn)
                    .is_some_and(|attachment| {
                        from_permission_version
                            .is_none_or(|version| attachment.version == version)
                    });
                if !moves {
                    continue;
                }
                // A task whose two permissions are the same one moves the share onto the
                // version the task resolved rather than detaching anything: that is the
                // "update all of your resource shares to use the current default version
                // of the permission" case (`ReplacePermissionAssociations`' own
                // documentation).
                if from_permission_arn != to_permission_arn {
                    share.detach_permission(&from_permission_arn);
                }
                share.attach_permission(SharePermissionRecord {
                    permission_arn: to_permission_arn.clone(),
                    version: to_permission_version,
                    // The task moved the attachment when it fell due, not when
                    // somebody noticed.
                    last_updated_time: owed.due_at,
                });
            }
        }
    }

    /// Forget completed tasks until there is room for another, and answer whether there
    /// is. `false` means every task held is still in flight
    /// ([`MAX_REPLACE_PERMISSION_ASSOCIATIONS_WORKS`] of them), so none may be dropped.
    fn make_room_for_replacement_work(&mut self) -> bool {
        while self.replace_permission_associations_works.len()
            >= MAX_REPLACE_PERMISSION_ASSOCIATIONS_WORKS
        {
            let oldest = self
                .replace_permission_associations_works
                .values()
                .filter(|work| work.owed_move.is_none())
                .min_by_key(|work| (work.creation_time, work.id.clone()))
                .map(|work| work.id.clone());
            match oldest {
                Some(id) => {
                    self.replace_permission_associations_works.remove(&id);
                }
                None => return false,
            }
        }
        true
    }

    /// A task id no task held is using. Built the way [`Self::fresh_share_arn`] builds a
    /// share's id: a fresh UUID, with a bounded fallback so a pathological clock cannot
    /// spin here.
    fn fresh_replacement_work_id(&mut self) -> String {
        for _ in 0..1024 {
            let candidate = self.fresh_uuid();
            if !self
                .replace_permission_associations_works
                .contains_key(&candidate)
            {
                return candidate;
            }
        }
        (0u64..=self.replace_permission_associations_works.len() as u64)
            .map(|n| format!("00000000-0000-4000-8000-{n:012x}"))
            .find(|id| !self.replace_permission_associations_works.contains_key(id))
            .unwrap_or_else(|| "00000000-0000-4000-8000-ffffffffffff".to_string())
    }
}

/// A `nextToken` naming the background task the next page starts at.
///
/// A read that pages over tasks pages over their ids, so the value a token carries is
/// the id itself, hex-encoded the way every other cursor of this service is — opaque to
/// a caller, not secret.
pub fn encode_replace_work_cursor(id: &str) -> String {
    hex::encode(id)
}

/// The task id a `nextToken` names, or `None` when the token is not one this service
/// issued — the `InvalidNextTokenException` case.
///
/// A token this service issued names a task id, whether or not the store still holds
/// that task, so the test is on the FORM of the value: a non-empty id.
pub fn decode_replace_work_cursor(token: &str) -> Option<String> {
    decode_cursor(token).filter(|id| !id.is_empty())
}

/// The message of the `InvalidParameterException` a replacement renders when the two
/// managed permissions it names apply to different resource types.
///
/// A managed permission governs the resources of exactly one resource type
/// (`ResourceSharePermissionSummary$resourceType`) and "a resource share can have only
/// one permission per resource type" (`AssociateResourceSharePermission$replace`), so a
/// permission can only be replaced on a share by another permission for the same
/// resource type: one for a different type would govern nothing the first one governed.
/// The message names both permissions and both resource types.
pub fn permission_resource_type_mismatch_message(
    from_permission_arn: &str,
    from_resource_type: &str,
    to_permission_arn: &str,
    to_resource_type: &str,
) -> String {
    format!(
        "Permission {to_permission_arn} applies to resource type {to_resource_type}, but \
         permission {from_permission_arn} applies to resource type {from_resource_type}. You can \
         replace a managed permission only with a managed permission for the same resource type."
    )
}
