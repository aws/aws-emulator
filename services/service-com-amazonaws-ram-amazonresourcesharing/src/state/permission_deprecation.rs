// SPDX-License-Identifier: CC0-1.0

//! How the status `UpdatePermission` writes onto a managed permission reaches the reads
//! that report it and the attaches it stops.
//!
//! `UpdatePermission` "updates the status of the specified customer managed permission.
//! Setting the status to DEPRECATED prevents new resource share associations. Setting it
//! to ATTACHABLE restores a previously deprecated permission. Existing resource shares
//! that use the permission continue to function." So the status is state the whole
//! permission carries, not a property of one of its versions: every version of a
//! deprecated permission reports `DEPRECATED` — "the permission owner deprecated this
//! permission. It can't be associated with new resource shares, but existing
//! associations continue to function" (`ResourceSharePermissionDetail$status`) — and
//! going back to `ATTACHABLE` restores "this permission or version can be associated
//! with resource shares".
//!
//! What deprecation takes away is therefore exactly one thing: a NEW attachment of the
//! permission to a resource share. The attachments a share already holds are untouched,
//! and so is every read of them.

use super::{
    ComAmazonawsRamAmazonresourcesharingState, PermissionRecord, PERMISSION_STATUS_ATTACHABLE,
};

/// The `PermissionStatus` a deprecated permission and every version of it reports:
/// "the permission owner deprecated this permission. It can't be associated with new
/// resource shares, but existing associations continue to function"
/// (`ResourceSharePermissionDetail$status`).
pub const PERMISSION_STATUS_DEPRECATED: &str = "DEPRECATED";

/// The `UpdatePermissionStatus` value that deprecates a permission, from
/// `smithy_query(query="shape",shape="UpdatePermissionStatus")`.
pub const UPDATE_PERMISSION_STATUS_DEPRECATED: &str = PERMISSION_STATUS_DEPRECATED;

/// The `UpdatePermissionStatus` value that restores a deprecated permission, which is
/// the `PermissionStatus` an undeprecated permission already carries.
pub const UPDATE_PERMISSION_STATUS_ATTACHABLE: &str = PERMISSION_STATUS_ATTACHABLE;

impl PermissionRecord {
    /// Whether `UpdatePermission` has deprecated this permission, so no new resource
    /// share association may be made with it.
    pub fn is_deprecated(&self) -> bool {
        self.deprecated
    }
}

impl ComAmazonawsRamAmazonresourcesharingState {
    /// Write the `UpdatePermissionStatus` `status` onto the managed permission `arn`
    /// names and answer the status it now reports, or `None` when this store holds no
    /// permission under that ARN.
    ///
    /// A call that asks for the status the permission already carries changes nothing,
    /// so it leaves `lastUpdatedTime` alone: what moved the permission was the call that
    /// changed its status.
    pub fn set_permission_status(
        &mut self,
        arn: &str,
        status: &str,
        now: i64,
    ) -> Option<&'static str> {
        let permission = self.permissions.get_mut(arn)?;
        let deprecated = status == UPDATE_PERMISSION_STATUS_DEPRECATED;
        if permission.deprecated != deprecated {
            permission.deprecated = deprecated;
            permission.last_updated_time = now;
            // The status is the whole permission's, so every version of it is now
            // reported in that status and was last updated by this call.
            for version in permission.versions.values_mut() {
                version.last_updated_time = now;
            }
        }
        Some(permission_status(permission.deprecated))
    }

    /// The `UpdatePermissionStatus` the managed permission `arn` names reports, or
    /// `None` when this store holds no permission under that ARN.
    pub fn permission_update_status(&self, arn: &str) -> Option<&'static str> {
        self.permissions
            .get(arn)
            .map(|permission| permission_status(permission.deprecated))
    }
}

/// The `UpdatePermissionStatus` a permission reports for a stored deprecation flag.
fn permission_status(deprecated: bool) -> &'static str {
    if deprecated {
        UPDATE_PERMISSION_STATUS_DEPRECATED
    } else {
        UPDATE_PERMISSION_STATUS_ATTACHABLE
    }
}

/// The message of the `OperationNotPermittedException` an attach renders when the
/// managed permission the request names is deprecated.
///
/// A deprecated permission "can't be associated with new resource shares, but existing
/// associations continue to function" (`ResourceSharePermissionDetail$status`), so the
/// message names the permission that was asked for and the status that stops the
/// attach.
pub fn deprecated_permission_not_attachable_message(arn: &str) -> String {
    format!(
        "Permission {arn} is DEPRECATED, so it can't be associated with new resource \
         shares. Set its status to {UPDATE_PERMISSION_STATUS_ATTACHABLE} to associate it \
         again."
    )
}

/// The message of the `OperationNotPermittedException` `UpdatePermission` renders when
/// the permission it names is one Amazon Web Services manages.
///
/// The operation "updates the status of the specified CUSTOMER MANAGED permission", and
/// an `AWS_MANAGED` permission is one "Amazon Web Services created and manages" that
/// "you can associate with your resource shares, but you can't modify"
/// (`ResourceSharePermissionDetail$permissionType`), so there is no status of one for a
/// caller to write.
pub fn aws_managed_permission_not_updatable_message(arn: &str) -> String {
    format!(
        "Permission {arn} is an Amazon Web Services managed permission, which Amazon Web \
         Services creates and manages. You can't modify it."
    )
}
