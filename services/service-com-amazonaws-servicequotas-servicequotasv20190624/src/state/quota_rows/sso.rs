// SPDX-License-Identifier: CC0-1.0

//! AWS IAM Identity Center's published default-quota catalog, under the `sso`
//! `ServiceCode`.

use super::*;

/// AWS IAM Identity Center's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `sso` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code sso` reports, which is
/// the "Quotas for IAM Identity Center" page of its user guide), and the `QuotaCode`s
/// that catalog is keyed on.
///
/// The service's quotas are the planes of ONE instance of IAM Identity Center — the
/// permission sets it holds, the policies one permission set may carry, and the
/// identities and applications the instance serves — so every row names the plane it
/// counts rather than the account.
///
/// None of them is a GLOBAL quota: an IAM Identity Center instance belongs to the Region
/// it was enabled in, so the planes of that instance are counted per Region, which is what
/// `GlobalQuota` reports for every row here.
///
/// The recorded listing states the quota published under `L-2FDFC908` (the groups that may
/// be assigned to a permission set per account or to an application, 100, with the sentence
/// the service words it with), under `L-D519A297` (the Regions IAM Identity Center may be
/// enabled in, 6, with its own sentence), under `L-89954265` (the permission sets one AWS
/// account may have provisioned, 500, adjustable, in `Unit` `None`), under `L-C4FCA052`
/// (the file size of the service provider's SAML 2.0 certificates, 2 Kilobytes, NOT
/// adjustable, with the sentence it states for it), under `L-0299121C` (the AWS accounts
/// or applications an instance may have configured, 7000), under `L-B44C7A29` (the
/// permission sets of an instance, 3500) and under `L-7E1D4E33` (the groups the instance's
/// identity store supports). It also publishes `L-3C8D41A0` as ADJUSTABLE. The
/// managed-policy count, the trusted-token-issuer count and the permission-set inline-policy
/// size this catalog used to key on those codes are NOT published here: no source available
/// here states AWS's code for them, so they are left to the default-quota-set gap rather
/// than served under another quota's code. A row whose published `Description` no source
/// available here states carries none, rather than a sentence this emulator words itself.
pub(in crate::state) fn sso_quotas() -> Vec<QuotaRow> {
    vec![
        quota(
            "Total number of AWS accounts or applications that can be configured",
            7000.0,
        )
        .code("L-0299121C"),
        quota(
            "Total number of groups that can be assigned to a permission set per AWS \
             account, or to an application",
            100.0,
        )
        .code("L-2FDFC908")
        .fixed()
        .describe(
            "Total number of groups that can be assigned to a permission set per AWS \
             account, or to an application.",
        ),
        quota("Customer managed policies per permission set", 20.0)
            .code("L-3C8D41A0")
            .describe(
                "The maximum number of customer managed policies that you can attach to a \
                 permission set.",
            ),
        quota("Number of groups supported in IAM Identity Center", 100_000.0)
            .code("L-7E1D4E33"),
        quota("Number of permission sets allowed per AWS account", 500.0)
            .code("L-89954265"),
        quota("Number of permission sets allowed in IAM Identity Center", 3500.0)
            .code("L-B44C7A29"),
        // The SAML 2.0 metadata the service provider publishes: a file SIZE in Kilobytes,
        // which the service publishes as NOT adjustable, with the sentence the recorded
        // listing states for it.
        quota(
            "File size of service provider SAML 2.0 certificates (in PEM format)",
            2.0,
        )
        .code("L-C4FCA052")
        .unit("Kilobytes")
        .fixed()
        .describe(
            "The maximum file size (in KB) of service provider SAML 2.0 certificates (in \
             PEM format).",
        ),
        quota("Total number of regions for IAM Identity Center", 6.0)
            .code("L-D519A297")
            .describe(
                "The total number of regions that can be enabled for IAM Identity Center, \
                 including the primary region.",
            ),
    ]
}
