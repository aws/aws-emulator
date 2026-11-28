// SPDX-License-Identifier: CC0-1.0

//! How the delete of a managed permission reaches the reads that report it.
//!
//! `DeletePermission` "deletes the specified customer managed permission … The
//! operation deletes all versions associated with the customer managed permission",
//! and it "is performed asynchronously" — its `permissionStatus` "indicates the
//! current status" (`DeletePermissionResponse$permissionStatus`). So a delete is not a
//! record vanishing: the permission and every version of it leave the attachable
//! statuses for `DELETING` ("this permission or version is in the process of being
//! deleted") and reach `DELETED` ("this permission or version is deleted") once the
//! move the clock owes falls due — the two terminal values of the modelled
//! `PermissionStatus` enum.
//!
//! The record SURVIVES in that terminal status, the way a deleted resource share's
//! does, so a caller polling `GetPermission` for the deletion sees it complete. What
//! the delete does take away is everything a live permission is for: it is no longer a
//! permission a resource share can be associated with, no longer one a new version can
//! be written for, and once the delete has completed its name is free for another
//! permission ([`ComAmazonawsRamAmazonresourcesharingState::live_permission`]).

use super::{
    ComAmazonawsRamAmazonresourcesharingState, OwedMove, PermissionRecord,
    PermissionVersionRecord,
};

/// The `PermissionStatus` a permission carries while its delete is in flight: "this
/// permission or version is in the process of being deleted"
/// (`ResourceSharePermissionDetail$status`).
pub const PERMISSION_STATUS_DELETING: &str = "DELETING";

/// The `PermissionStatus` a permission carries once its delete has completed: "this
/// permission or version is deleted" (`ResourceSharePermissionDetail$status`).
pub const PERMISSION_STATUS_DELETED: &str = "DELETED";

/// How long after the call that deletes a managed permission RAM takes to move it from
/// `DELETING` to `DELETED`.
///
/// `DeletePermission` "is performed asynchronously" and answers with the permission in
/// the non-terminal `DELETING` status, so reaching `DELETED` is a move the clock owes
/// rather than something a caller asks for. AWS publishes no duration for it, so this
/// span is this emulator's own choice; it is deliberately not zero, so a caller that
/// reads straight back still observes `DELETING`. Nothing sleeps for it: the due
/// instant is stored and the move is materialised by the first call at or after it
/// ([`ComAmazonawsRamAmazonresourcesharingState::settle_due_permissions`]).
pub const PERMISSION_DELETED_AFTER_MILLIS: i64 = 3_000;

/// Where the delete of one managed permission has got to.
///
/// A permission carries one of these only once `DeletePermission` has been called on
/// it: the delete is a property of the whole permission rather than of one version,
/// because "the operation deletes all versions associated with the customer managed
/// permission".
#[derive(Debug, Clone)]
pub struct PermissionDeletion {
    /// The `PermissionStatus` this permission and every version of it reports from the
    /// delete on: [`PERMISSION_STATUS_DELETING`] while the delete is in flight, then
    /// [`PERMISSION_STATUS_DELETED`].
    pub status: String,
    /// The move to [`PERMISSION_STATUS_DELETED`] the clock owes, or `None` once it has
    /// been made.
    pub owed_move: Option<OwedMove>,
}

impl PermissionRecord {
    /// Whether `DeletePermission` has been called on this permission — it is either
    /// being deleted or deleted, and in neither case is it a permission an operation
    /// that acts on a live one can act on.
    pub fn is_deleting_or_deleted(&self) -> bool {
        self.deletion.is_some()
    }

    /// Whether the delete of this permission has completed, which is when its name
    /// becomes free for another permission.
    pub fn is_deleted(&self) -> bool {
        self.deletion
            .as_ref()
            .is_some_and(|deletion| deletion.status == PERMISSION_STATUS_DELETED)
    }

    /// The `PermissionStatus` a read of `version` of this permission reports: the
    /// status the delete put the whole permission in, then the status the delete of
    /// that one version put it in, then — for a permission `UpdatePermission`
    /// deprecated — [`super::PERMISSION_STATUS_DEPRECATED`], or else the version's own
    /// status.
    ///
    /// A delete outranks a deprecation: a permission being deleted is no longer one a
    /// status of its own describes.
    pub fn version_status(&self, version: &PermissionVersionRecord) -> String {
        match self.deletion.as_ref().or(version.deletion.as_ref()) {
            Some(deletion) => deletion.status.clone(),
            None if self.is_deprecated() => super::PERMISSION_STATUS_DEPRECATED.to_string(),
            None => version.status.clone(),
        }
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// The managed permission `arn` names, for an operation the spec says acts on a
    /// live one: attaching it to a resource share, or writing a new version of it.
    ///
    /// A permission `DeletePermission` has deleted — or is deleting — is gone as far as
    /// those operations are concerned, so this answers `None` for one: the
    /// `UnknownResourceException` case. The mirror of [`Self::live_share`]; a read of
    /// the permission in its own right (`GetPermission`) goes through
    /// [`Self::permission`] instead, because reporting the delete is the point of it.
    pub fn live_permission(&self, arn: &str) -> Option<&PermissionRecord> {
        self.permissions
            .get(arn)
            .filter(|permission| !permission.is_deleting_or_deleted())
    }

    /// Start the delete of the managed permission `arn` names and answer the
    /// `PermissionStatus` it now reports, or `None` when this store holds no permission
    /// under that ARN.
    ///
    /// The permission and every version of it go to [`PERMISSION_STATUS_DELETING`] at
    /// once and the clock owes [`PERMISSION_STATUS_DELETED`]
    /// [`PERMISSION_DELETED_AFTER_MILLIS`] later. A permission already on the delete
    /// path is left exactly as it is: the call that started the delete is the one that
    /// decided when it completes, so a repeat neither restarts the move nor moves the
    /// stamps — it reports the status the permission has reached.
    pub fn delete_permission(&mut self, arn: &str, now: i64) -> Option<String> {
        let permission = self.permissions.get_mut(arn)?;
        if let Some(deletion) = &permission.deletion {
            return Some(deletion.status.clone());
        }
        permission.deletion = Some(PermissionDeletion {
            status: PERMISSION_STATUS_DELETING.to_string(),
            owed_move: Some(OwedMove {
                status: PERMISSION_STATUS_DELETED.to_string(),
                due_at: now + PERMISSION_DELETED_AFTER_MILLIS,
            }),
        });
        permission.last_updated_time = now;
        // Every version is being deleted with the permission, so each one was last
        // updated by this call.
        for version in permission.versions.values_mut() {
            version.last_updated_time = now;
        }
        Some(PERMISSION_STATUS_DELETING.to_string())
    }

    /// Make every managed permission the move its delete owed at or before `now`.
    ///
    /// Like [`Self::settle_due_associations`], this only materialises what was already
    /// due: a permission `DeletePermission` accepted in `DELETING` is `DELETED`
    /// [`PERMISSION_DELETED_AFTER_MILLIS`] after the call that deleted it, whether or
    /// not anybody asks. The delete of ONE version is the same owed move, so it is
    /// settled from here too
    /// ([`ComAmazonawsRamAmazonresourcesharingState::settle_due_permission_versions`]):
    /// every call that settles a permission settles its versions. So is the background
    /// task `ReplacePermissionAssociations` starts
    /// ([`ComAmazonawsRamAmazonresourcesharingState::settle_due_permission_replacements`]),
    /// which is owed on the same terms.
    pub fn settle_due_permissions(&mut self, now: i64) {
        self.settle_due_permission_versions(now);
        self.settle_due_permission_replacements(now);
        for permission in self.permissions.values_mut() {
            let Some(deletion) = &mut permission.deletion else {
                continue;
            };
            let Some(owed) = deletion.owed_move.clone() else {
                continue;
            };
            if owed.due_at > now {
                continue;
            }
            deletion.status = owed.status;
            deletion.owed_move = None;
            // The move happened when it fell due, not when somebody noticed.
            permission.last_updated_time = owed.due_at;
            for version in permission.versions.values_mut() {
                version.last_updated_time = owed.due_at;
            }
        }
    }

    /// The ARNs of the live resource shares the managed permission `permission_arn` is
    /// attached to by name, in share ARN order.
    ///
    /// "You can delete a customer managed permission only if it isn't attached to any
    /// resource share" (`DeletePermission`'s own documentation), so these are the
    /// attachments that stand in the way of a delete. A share
    /// `DeleteResourceShare` has removed holds nothing back: it is itself gone.
    pub fn shares_attaching_permission(&self, permission_arn: &str) -> Vec<String> {
        self.resource_shares
            .values()
            .filter(|share| !share.is_deleted() && share.attaches_permission(permission_arn))
            .map(|share| share.arn.clone())
            .collect()
    }
}

/// The message of the `OperationNotPermittedException` a delete renders when the
/// permission it names is one Amazon Web Services manages.
///
/// `DeletePermission` "deletes the specified CUSTOMER MANAGED permission", and an
/// `AWS_MANAGED` permission is one "Amazon Web Services created and manages" that "you
/// can associate with your resource shares, but you can't modify"
/// (`ResourceSharePermissionDetail$permissionType`) — so there is no delete of one for
/// a caller to ask for, and the message names the permission that was asked for.
pub fn aws_managed_permission_not_deletable_message(arn: &str) -> String {
    format!(
        "Permission {arn} is an Amazon Web Services managed permission, which Amazon Web \
         Services creates and manages. You can't modify or delete it."
    )
}

/// The message of the `OperationNotPermittedException` a delete renders when the
/// permission it names is still attached to a resource share.
///
/// "You can delete a customer managed permission only if it isn't attached to any
/// resource share" (`DeletePermission`'s own documentation), so the message names the
/// permission, one share it is attached to and how many shares hold it.
pub fn permission_attached_to_share_message(arn: &str, shares: &[String]) -> String {
    let attached = shares.first().map(String::as_str).unwrap_or_default();
    format!(
        "Permission {arn} is associated with {} resource share(s), including {attached}. You \
         can delete a customer managed permission only if it isn't associated with any \
         resource share.",
        shares.len()
    )
}
