// SPDX-License-Identifier: CC0-1.0

//! Whether the IAM role a backup would be PERFORMED under is one AWS Backup can establish
//! the backup's permissions from — the question `StartBackupJob` settles before it creates
//! anything.
//!
//! # The rule this module implements
//!
//! A backup is performed under the `IamRoleArn` the request names "to create the target
//! recovery point", not under the caller's own identity, so AWS Backup resolves that value
//! and ASSUMES the role it names BEFORE it mints a job. Two answers say there is no role to
//! perform the backup under, and each is refused with the parameter-value class carrying
//! this condition's own token ([`ERROR_CODE_ROLE_WITHOUT_BACKUP_PERMISSIONS`]) — no job, no
//! recovery point, nothing for a later read to find:
//!
//! * the value names no IAM ROLE OF THIS ACCOUNT at all — another account's role, an IAM
//!   entity that is not a role, an `iam` ARN naming no entity — read through the tree's one
//!   reading of an IAM ARN ([`::service_common::iam_role::role_name`]), the same reading the
//!   role of a backup selection is resolved by ([`super::rejected_selection_role`]);
//! * this account's IAM ANSWERS that it holds no role under the name the value carries
//!   ([`::service_common::iam_role::role_is_absent`]), so there is nothing to assume. This is
//!   the one call of this service for which a role's ABSENCE is an answer about the backup
//!   rather than about this store: the backup is performed by assuming that role, and a role
//!   that is not there cannot be assumed, which is why a client that has just created one
//!   retries this call until the record is visible.
//!
//! This is a different question from the one the job's own OUTCOME rests on
//! ([`super::withheld_backup`]). There the role IS one this account holds: the backup is
//! performed under it, so what IAM answers about that role's PERMISSIONS decides the
//! terminal state the clock owes the job — a role whose policy set allows the action nowhere
//! ends it `FAILED` carrying the denial, which is what AWS answers for a role it could
//! assume and act under. Here there is nothing to act under at all, which is settled
//! synchronously.
//!
//! Nothing is decided on an answer this emulator could not get, exactly as the shared
//! reading defines: an answer this emulator could not read and an emulator with no IAM
//! mounted beside this service leave the call alone. A value that is not an ARN at all is
//! left alone too — it is refused for THAT by the reading every member this service types as
//! an unpatterned ARN takes ([`super::rejected_arn`]), so one value earns one refusal.

use super::InvalidParameterValueException;

/// `Code` for "the role this backup would be performed under does not have the permissions
/// the backup needs" — the token of that condition, constant for it and taken from no part
/// of the request, as every other token this service states is
/// ([`super::ERROR_CODE_ROLE_NOT_ASSUMABLE`], the neighbouring role condition).
pub const ERROR_CODE_ROLE_WITHOUT_BACKUP_PERMISSIONS: &str = "ERROR_3020";

/// The sentence AWS states for that condition — FIXED text: neither the role nor the
/// resource the call named appears in it, the way the neighbouring role conditions of this
/// service word theirs ([`super::MESSAGE_ROLE_NOT_ASSUMABLE`]).
pub const MESSAGE_ROLE_WITHOUT_BACKUP_PERMISSIONS: &str =
    "IAM Role does not have sufficient permissions to execute the backup";

/// The modelled refusal for a backup whose role the permissions cannot be established from —
/// see [`MESSAGE_ROLE_WITHOUT_BACKUP_PERMISSIONS`], with the condition's own token.
#[must_use]
pub fn role_without_backup_permissions() -> InvalidParameterValueException {
    InvalidParameterValueException {
        code: Some(ERROR_CODE_ROLE_WITHOUT_BACKUP_PERMISSIONS.to_string()),
        message: Some(MESSAGE_ROLE_WITHOUT_BACKUP_PERMISSIONS.to_string()),
        ..Default::default()
    }
}

/// The modelled refusal for a `StartBackupJob` whose `IamRoleArn` names no role this
/// account holds for the backup to be performed under, or `None` when nothing shows there
/// is none (the module docs).
///
/// Asked BEFORE anything is stored and with NO store guard held: it makes at most the one
/// in-process hop the existence question needs and opens no socket.
pub async fn rejected_backup_role(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<InvalidParameterValueException> {
    // Not an ARN at all: that refusal is the ARN reading's, not this one's.
    if super::rejected_arn("IamRoleArn", role_arn).is_some() {
        return None;
    }
    if ::service_common::iam_role::role_name(role_arn).is_none() {
        return Some(role_without_backup_permissions());
    }
    ::service_common::iam_role::role_is_absent(ctx, region, role_arn)
        .await
        .then(role_without_backup_permissions)
}
