// SPDX-License-Identifier: CC0-1.0

//! What a started backup job COMES TO: whether the backup it was started for can be
//! performed at all, which is what decides the job's terminal state.
//!
//! # The rule this module implements
//!
//! `StartBackupJob` is accepted on what it can be judged on synchronously — the vault, the
//! parameters — and the job's TERMINAL state is decided by whether the backup it asks for
//! can be performed. Two conditions say it cannot, and each produces a job that captured
//! nothing: it is stored with NO recovery point at all and the clock settles it `FAILED`,
//! carrying why as its `StatusMessage` beside the `MessageCategory` of that class
//! ([`BackupFailure`]). A backup nothing shows cannot be performed ends `COMPLETED` with the
//! recovery point it wrote.
//!
//! * The ROLE it runs under. The backup is performed under the `IamRoleArn` the request named
//!   "to create the target recovery point", not under the caller's own identity, so a role
//!   whose permissions do not allow the action that creates the recovery point of the named
//!   resource can produce none.
//! * The RESOURCE it was started for. A `ResourceArn` naming a resource the service that owns
//!   it does not hold is a backup with nothing to capture
//!   ([`super::protected_resource_answer`]).
//!
//! The role is asked first: a role that may not perform the backup never reaches the
//! resource, so that denial is what such a job reports.
//!
//! Both questions are answered BEFORE the job is stored, so the stored job already carries
//! the outcome it is owed: there is no instant at which the clock could settle a job whose
//! outcome is still being decided.
//!
//! The role question is put to IAM ITSELF, over the emulator's own in-process callback,
//! through the tree's one reading of what a principal's policies decide
//! ([`::service_common::iam_role::role_withholds_action`]): what settles it is the documents
//! a client's `PutRolePolicy` or `AttachRolePolicy` left on the role, and the sentence the
//! denial is worded with is the tree's one wording of an identity-based denial
//! ([`::service_common::iam_authorization`]), so two services that reach this decision state
//! it the same way.
//!
//! Nothing is decided on an answer this emulator could not get: an emulator with no IAM
//! mounted beside this service, a value naming no role of this account, and a role IAM holds
//! nothing under all leave the job's outcome alone, for the reason the shared reading gives —
//! a backup role is routinely provisioned out of band, so its absence from this store is not
//! evidence about its permissions. A resource type this emulator knows no creating action for
//! is the same kind of silence ([`backup_action_for`]), and so is every silence the resource
//! question meets.

/// The action AWS Backup performs UNDER a job's role to create the recovery point of a
/// protected resource, per Backup resource type ([`super::resource_type_of`]).
///
/// SOURCE: the per-resource-type grants AWS publishes for the backup role (AWS Backup
/// *Managed policies*, `AWSBackupServiceRolePolicyForBackup`) — of each type's grants, the
/// one action that CREATES the recovery point, which is the call a role with no permissions
/// is denied. The snapshot of a cluster the `rds` API serves is created by the same action
/// whichever engine fronts it, so Aurora, Amazon DocumentDB and Amazon Neptune share it.
///
/// A type this table does not name is one this emulator knows no creating action for, so it
/// grounds no decision — see the module docs.
const BACKUP_ROLE_ACTIONS: &[(&str, &str)] = &[
    ("Aurora", "rds:CreateDBClusterSnapshot"),
    ("DocumentDB", "rds:CreateDBClusterSnapshot"),
    ("DynamoDB", "dynamodb:CreateBackup"),
    ("EBS", "ec2:CreateSnapshot"),
    ("EC2", "ec2:CreateImage"),
    ("EFS", "elasticfilesystem:Backup"),
    ("FSx", "fsx:CreateBackup"),
    ("Neptune", "rds:CreateDBClusterSnapshot"),
    ("RDS", "rds:CreateDBSnapshot"),
    ("Redshift", "redshift:CreateClusterSnapshot"),
    ("Timestream", "timestream:StartAwsBackupJob"),
];

/// The action a backup of a resource of `resource_type` performs under the job's role, or
/// `None` for a type [`BACKUP_ROLE_ACTIONS`] names none for — including a resource whose ARN
/// resolved to no Backup resource type at all.
#[must_use]
pub fn backup_action_for(resource_type: Option<&str>) -> Option<&'static str> {
    let resource_type = resource_type?;
    BACKUP_ROLE_ACTIONS
        .iter()
        .find(|(named, _)| *named == resource_type)
        .map(|(_, action)| *action)
}

/// The `MessageCategory` a job that failed for the PERMISSIONS of its role reports.
///
/// SOURCE: the example strings the model's own documentation for the member gives
/// (`smithy_query(query="doc", shape="DescribeBackupJobOutput", member="MessageCategory",
/// model_path="services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")`),
/// of which this is the one naming a denial.
pub const MESSAGE_CATEGORY_ACCESS_DENIED: &str = "AccessDenied";

/// The `MessageCategory` a job that failed because the resource it was started for does not
/// exist reports, as AWS answers for that class.
pub const MESSAGE_CATEGORY_RESOURCE_NOT_FOUND: &str = "RESOURCE_NOT_FOUND";

/// Why a started backup job can create no recovery point: the `StatusMessage` its terminal
/// `FAILED` state carries, and the `MessageCategory` of the class it failed in.
///
/// The two travel together because neither stands alone: a job can never report a failure
/// with no reason, nor a category it did not fail in.
#[derive(Debug, Clone)]
pub struct BackupFailure {
    pub status_message: String,
    pub message_category: String,
}

/// Why the backup of `resource_arn` under `role_arn` can create no recovery point, or `None`
/// when nothing shows it cannot — the two conditions the module docs state, in the order
/// they are asked.
///
/// Asked BEFORE the job is stored, with NO store guard held: it makes at most the two
/// in-process calls the two questions need and opens no socket.
pub async fn withheld_backup(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    resource_arn: &str,
) -> Option<BackupFailure> {
    let resource_type = super::resource_type_of(resource_arn);
    if let Some(status_message) =
        withheld_backup_denial(ctx, region, role_arn, resource_arn, resource_type.as_deref()).await
    {
        return Some(BackupFailure {
            status_message,
            message_category: MESSAGE_CATEGORY_ACCESS_DENIED.to_string(),
        });
    }
    match super::protected_resource_answer(
        ctx.callback(),
        region,
        resource_type.as_deref(),
        resource_arn,
    )
    .await
    {
        super::ProtectedResource::Absent => Some(BackupFailure {
            status_message: resource_not_found_status_message(resource_arn),
            message_category: MESSAGE_CATEGORY_RESOURCE_NOT_FOUND.to_string(),
        }),
        // Both the resource being there and nothing having been learned about it leave the
        // job's outcome alone (the module docs).
        super::ProtectedResource::Held | super::ProtectedResource::Unknown => None,
    }
}

/// The `StatusMessage` a job that was started for a resource no service holds carries: the
/// fixed sentence AWS words that class with, around the `ResourceArn` the request named.
#[must_use]
pub fn resource_not_found_status_message(resource_arn: &str) -> String {
    format!("Requested resource not found: ResourceArn: {resource_arn} not found")
}

/// Why the backup of `resource_arn` under `role_arn` may not be performed at all — the
/// denial the job's terminal `FAILED` state carries — or `None` when nothing shows it may
/// not be.
///
/// The action asked about is selected by the Backup resource type of the protected resource,
/// derived from its ARN the way every other report of that type is
/// ([`super::resource_type_of`]), so one resource is judged on one action however many
/// callers ask.
async fn withheld_backup_denial(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    resource_arn: &str,
    resource_type: Option<&str>,
) -> Option<String> {
    let action = backup_action_for(resource_type)?;
    if !::service_common::iam_role::role_withholds_action(
        ctx, region, role_arn, action, resource_arn,
    )
    .await
    {
        return None;
    }
    Some(
        ::service_common::iam_authorization::not_authorized_by_identity_policy(
            role_arn,
            action,
            resource_arn,
            &::service_common::iam_authorization::no_identity_policy_allows(action),
        ),
    )
}
