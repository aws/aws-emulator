// SPDX-License-Identifier: CC0-1.0

//! The preconditions a snapshot or cluster export is accepted against: the IAM role that
//! writes the objects, and the KMS key they are encrypted under.
//!
//! "The provided IAM role must have access to the S3 bucket" (`StartExportTask`), and the
//! role Amazon RDS assumes to write an export is one that trusts the EXPORT principal
//! `export.rds.amazonaws.com` and carries a policy granting it the object write the export
//! performs ("Provide access to an Amazon S3 bucket using an IAM role", Exporting DB
//! snapshot data to Amazon S3). The key is "the ID of the Amazon Web Services KMS key to
//! use to encrypt the data exported to Amazon S3" (`KmsKeyId`), and the export cannot run
//! under a key that does not exist or is not enabled.
//!
//! Each question is answered against the service that OWNS the answer — IAM for the role
//! and its policies, Amazon S3 for the destination bucket's policy, KMS for the key — and
//! each refusal is grounded in what that service ANSWERED: an emulator with no IAM, S3 or
//! KMS beside this one establishes nothing, so it refuses nothing.

use super::export_objects::bucket_policy;

/// The IAM action one export object write performs: it writes the exported file into the
/// destination bucket, which is the `s3:PutObject` the role's policy has to grant ("include
/// the following required actions to allow the transfer of files from Amazon RDS or Amazon
/// Aurora to an S3 bucket: s3:PutObject* ...", `IamRoleArn`).
pub const EXPORT_OBJECT_WRITE_ACTION: &str = "s3:PutObject";

/// What is wrong with the role an export was asked to run under.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportRoleDefect {
    /// IAM answered that the value names no role this account holds, or that the role it
    /// holds does not let the export principal assume it: the modelled
    /// `IamRoleNotFoundFault`, "The IAM role is missing for exporting to an Amazon S3
    /// bucket".
    NotAssumable,
    /// The role is assumable, but nothing grants it the object write the export performs,
    /// or the destination bucket's own policy denies it: the modelled
    /// `IamRoleMissingPermissionsFault`, which both halves of the decision are refused with
    /// in the one wording [`super::export_role_missing_permissions_message`] builds.
    MissingPermissions,
}

/// Judge the export role the way Amazon RDS does before it accepts an export: `None` for a
/// role the export can run under, and the defect that stops it otherwise.
///
/// `bucket` is the destination, whose objects are the resource the write is authorized on
/// and whose own policy is the other half of the decision.
///
/// Nothing is refused on an answer this emulator could not get: a value naming no role of
/// this account, an IAM that is not mounted beside this service, or policies that could not
/// be enumerated each leave the role unjudged, exactly as
/// [`::service_common::iam_role::held_role_is_unassumable`] leaves a role IAM holds nothing
/// under.
pub async fn export_role_defect<Eng: Send + Sync + 'static>(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    engine: &Eng,
    region: &str,
    role_arn: &str,
    bucket: &str,
) -> Option<ExportRoleDefect> {
    if ::service_common::iam_role::role_is_unassumable(
        ctx,
        region,
        role_arn,
        super::EXPORT_SERVICE_PRINCIPAL,
    )
    .await
    {
        return Some(ExportRoleDefect::NotAssumable);
    }
    // The objects the export writes: everything under the destination bucket, which is the
    // resource form the required actions are granted on
    // ("arn:aws:s3:::your-s3-bucket/*", `IamRoleArn`).
    let objects = ::service_com_amazonaws_s3_amazons3::state::object_arn(bucket, "*");
    let principals = export_principals(role_arn);
    let context = export_context(role_arn);
    let request = ::service_common::iam_authorization::Request::new(
        EXPORT_OBJECT_WRITE_ACTION,
        &objects,
        &principals,
    )
    .with_context(&context);
    // The bucket's own policy is read first: "an explicit deny in any policy overrides any
    // allow", so a bucket that denies the role the write refuses the export whatever the
    // role carries.
    if let Some(policy) = bucket_policy(engine, bucket) {
        if ::service_common::iam_authorization::resource_decision(&policy, &request)
            == ::service_common::iam_authorization::Decision::Denied
        {
            return Some(ExportRoleDefect::MissingPermissions);
        }
    }
    // And the role's own identity policies have to GRANT it: a role carrying none of them
    // cannot write the export, which is what the missing-permissions fault reports. A set
    // of documents this emulator could not read establishes nothing.
    let documents = role_identity_documents(ctx, region, role_arn).await?;
    if ::service_common::iam_authorization::identity_decision(&documents, &request)
        == ::service_common::iam_authorization::Decision::Allowed
    {
        return None;
    }
    Some(ExportRoleDefect::MissingPermissions)
}

/// Every spelling of the role a resource-based policy may name: the role's own ARN, and the
/// account spellings a DENY of the account refuses each of its principals under
/// ([`::service_common::iam_authorization::account_principals`]).
fn export_principals(role_arn: &str) -> Vec<String> {
    let mut principals = vec![role_arn.to_string()];
    principals.extend(::service_common::iam_authorization::account_principals(
        role_arn,
    ));
    principals
}

/// The condition keys the calls an export makes carry: the export acts UNDER the role the
/// request named, so `aws:PrincipalArn` is that role's ARN — which is what a destination
/// policy that admits or refuses one principal is written over.
fn export_context(role_arn: &str) -> Vec<(String, String)> {
    vec![(PRINCIPAL_ARN_KEY.to_string(), role_arn.to_string())]
}

/// The IAM global condition key naming the identity whose policies are in force: "the ARN
/// of the principal that made the request", which for a request made under an assumed role
/// is that role's own ARN.
const PRINCIPAL_ARN_KEY: &str = "aws:PrincipalArn";

/// The KMS action of the two the export needs that the key `key_id` does NOT authorize the
/// role `role_arn` to perform, or `None` where the key authorizes both of them.
///
/// "The caller of this operation must be authorized to run the following operations. These
/// can be set in the Amazon Web Services KMS key policy: kms:CreateGrant kms:DescribeKey"
/// (`KmsKeyId`), and in KMS "no AWS principal has any permissions to a KMS key unless the
/// key policy allows it" — so the question is what the KEY's own policy says about the role
/// the export runs under, and a policy that delegates to IAM by granting the account (the
/// default key policy) authorizes it.
///
/// `None` for everything that is not an answer: no KMS beside this service, a key it would
/// not report a policy for, or a document that could not be read. A refusal has to be
/// grounded in what KMS said.
pub async fn export_key_withheld_action(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
    role_arn: &str,
) -> Option<&'static str> {
    let kms = super::managed_master_secret::kms_client(ctx, region)?;
    let policy = kms
        .get_key_policy()
        .key_id(key_id)
        .send()
        .await
        .ok()?
        .policy
        .filter(|document| !document.is_empty())?;
    let principals = export_principals(role_arn);
    let context = export_context(role_arn);
    EXPORT_KEY_ACTIONS.into_iter().find(|action| {
        let request = ::service_common::iam_authorization::Request::new(
            action,
            // The key itself is the resource those actions are taken on; a key policy
            // names its own key as `"*"`, which covers it.
            key_id,
            &principals,
        )
        .with_context(&context);
        ::service_common::iam_authorization::resource_decision(&policy, &request)
            != ::service_common::iam_authorization::Decision::Allowed
    })
}

/// The KMS operations an export has to be authorized to run on the key its data is
/// encrypted under, as `KmsKeyId` lists them.
const EXPORT_KEY_ACTIONS: [&str; 2] = ["kms:CreateGrant", "kms:DescribeKey"];

/// Every identity-based policy document IAM holds for the role `role_arn` names — the
/// documents of the managed policies attached to it, and its inline policies — or `None`
/// when this emulator could not read them at all (there is no IAM to ask, the value names no
/// role of this account, or an answer could not be read), which grounds no refusal.
///
/// An enumeration that succeeds and reports NO policy is a role that carries none, which is
/// an answer rather than a silence: it is reported as the empty set, and a role with no
/// policy grants nothing.
async fn role_identity_documents(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<Vec<String>> {
    let role_name = ::service_common::iam_role::role_name(role_arn)?;
    let iam = ::service_common::iam_client::iam_client(ctx, region);
    let mut documents = Vec::new();

    let attached = iam
        .list_attached_role_policies()
        .role_name(role_name)
        .send()
        .await
        .ok()?;
    for policy in attached.attached_policies() {
        let policy_arn = policy.policy_arn()?;
        let described = iam.get_policy().policy_arn(policy_arn).send().await.ok()?;
        let version_id = described.policy()?.default_version_id()?;
        let version = iam
            .get_policy_version()
            .policy_arn(policy_arn)
            .version_id(version_id)
            .send()
            .await
            .ok()?;
        documents.push(version.policy_version()?.document()?.to_string());
    }

    let inline = iam
        .list_role_policies()
        .role_name(role_name)
        .send()
        .await
        .ok()?;
    for policy_name in inline.policy_names() {
        let policy = iam
            .get_role_policy()
            .role_name(role_name)
            .policy_name(policy_name)
            .send()
            .await
            .ok()?;
        documents.push(policy.policy_document().to_string());
    }

    Some(documents)
}

/// Whether KMS ANSWERED that the export cannot be encrypted under `key_id`: it holds no
/// such key, or the key it holds is not enabled.
///
/// `false` for everything that is not such an answer — no KMS mounted beside this service,
/// a signing scope that names no Region to ask it in, or any other failure to ask — because
/// a refusal has to be grounded in what KMS said. The client is the crate's ONE KMS client
/// ([`super::managed_master_secret::kms_client`]), which reaches this emulator's own KMS over
/// the in-process callback: no socket is opened and nothing leaves the process.
pub async fn kms_key_is_inaccessible(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    key_id: &str,
) -> bool {
    let Some(kms) = super::managed_master_secret::kms_client(ctx, region) else {
        return false;
    };
    let answer = kms.describe_key().key_id(key_id).send().await;
    match answer {
        // KMS resolved the key: the export can use it unless the key itself is not enabled,
        // which is the other half of what the fault reports.
        Ok(described) => described
            .key_metadata()
            .is_some_and(|metadata| !metadata.enabled()),
        // KMS answered that this account holds no such key, or that the identifier is not
        // one it can resolve: the refusal is grounded in its answer.
        Err(error) => error.as_service_error().is_some_and(|service| {
            matches!(
                service,
                ::aws_sdk_kms::operation::describe_key::DescribeKeyError::NotFoundException(_)
                    | ::aws_sdk_kms::operation::describe_key::DescribeKeyError::InvalidArnException(
                        _
                    )
            )
        }),
    }
}
