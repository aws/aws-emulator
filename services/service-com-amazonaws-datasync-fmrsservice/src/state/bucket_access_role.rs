// SPDX-License-Identifier: CC0-1.0

//! Whether this service can ASSUME a bucket access role a request names.
//!
//! Every `BucketAccessRoleArn` this API takes is "the ARN of the IAM role that DataSync
//! uses to access your S3 bucket" (`S3Config$BucketAccessRoleArn`, and the same member on
//! `S3ManifestConfig` and `ReportDestinationS3`), so the service takes that role ON before
//! it reads or writes anything under the bucket. Taking it on is `sts:AssumeRole` against
//! the role's trust policy, and a role this service principal cannot assume is one no
//! transfer, manifest read or task report could ever use — so the request that names it is
//! refused synchronously, with the modelled client error and nothing persisted, rather than
//! a resource being minted whose work could never run.
//!
//! The question itself is the tree's shared reading of a caller-supplied role
//! ([`::service_common::iam_role::role_is_unassumable_through`]): which role of this account
//! the ARN names, the in-process `GetRole`, and the trust-policy reading
//! ([`::service_common::iam_trust_policy`]). This module states only WHICH principal has to
//! be admitted, how this service words the refusal, and which members of a request carry
//! such a role. A refusal is therefore always grounded in what IAM ANSWERED — this account's
//! IAM holds no role under the name the ARN carries, or the trust policy of the role it does
//! hold does not admit this principal with the assume. Anything that is not such an answer
//! refuses nothing: a value naming no role of this account (another account's IAM is not
//! this emulator's to ask), an answer this emulator could not read, an emulator with no IAM
//! mounted at all.
//!
//! What the role's own policies then allow under the bucket is the OTHER half of the access
//! test ([`super::transfer::bucket_listing_withheld`]); this half is asked first, because a
//! role this service cannot take on at all is one no bucket policy can say anything useful
//! about.

use ::smithy_server_runtime::EmbeddedEmulator;

use crate::ops::types::{InvalidRequestException, ManifestConfig, TaskReportConfig};

/// The service principal a bucket access role has to trust: DataSync is what assumes it,
/// in the global spelling a trust policy names a service by.
pub const SERVICE_PRINCIPAL: &str = "datasync.amazonaws.com";

/// The class discriminator DataSync reports for a request naming a role it could not
/// assume.
///
/// Every client failure of every operation of this service is the one modelled client
/// error, so the refusal CLASS is carried by that error's two code members — and a caller
/// branching on "my trust policy is wrong" reads it there.
pub const STS_ASSUME_ROLE_CODE: &str = "StsAssumeRoleInvalidRequest";

/// How a refusal of an unassumable role is worded: fixed text around the ONE variable
/// slot, the role ARN exactly as the request supplied it.
pub fn unassumable_message(role_arn: &str) -> String {
    format!("Invalid IAM role, {role_arn}. DataSync service was unable to perform sts:AssumeRole.")
}

/// The modelled client error for the FIRST role among `role_arns` that IAM answered this
/// service cannot assume, or `None` when every one of them is assumable as far as IAM
/// said.
///
/// Callers hand the roles in the order the request names them, and ask BEFORE they take
/// the state lock or write anything: a refused request stores nothing.
pub async fn unassumable_role_refusal(
    callback: &EmbeddedEmulator,
    region: &str,
    role_arns: &[&str],
) -> Option<InvalidRequestException> {
    for role_arn in role_arns {
        if ::service_common::iam_role::role_is_unassumable_through(
            callback,
            region,
            role_arn,
            SERVICE_PRINCIPAL,
        )
        .await
        {
            let code = Some(STS_ASSUME_ROLE_CODE.to_string());
            return Some(InvalidRequestException {
                message: Some(unassumable_message(role_arn)),
                error_code: code.clone(),
                datasync_error_code: code,
            });
        }
    }
    None
}

/// The bucket access roles a task-shaped request names, in the order its members appear:
/// the role that reads the manifest the transfer is driven by, then the role that uploads
/// the task report.
///
/// `CreateTask` and `StartTaskExecution` both carry these two optional configurations, so
/// both resolve the roles they name the same way.
pub fn task_configuration_roles<'a>(
    manifest_config: Option<&'a ManifestConfig>,
    task_report_config: Option<&'a TaskReportConfig>,
) -> Vec<&'a str> {
    let manifest = manifest_config
        .and_then(|config| config.source.as_ref())
        .map(|source| source.s3.bucket_access_role_arn.as_str());
    let report = task_report_config
        .and_then(|config| config.destination.as_ref())
        .and_then(|destination| destination.s3.as_ref())
        .map(|s3| s3.bucket_access_role_arn.as_str());
    manifest.into_iter().chain(report).collect()
}
