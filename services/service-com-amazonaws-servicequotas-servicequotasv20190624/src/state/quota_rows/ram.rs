// SPDX-License-Identifier: CC0-1.0

//! AWS Resource Access Manager's published default-quota catalog.

use super::*;

/// AWS Resource Access Manager's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `ram` service code, and the
/// recorded default-quota listing of that code for WHICH `QuotaCode` names which
/// quota and for the sentence published under each: this service publishes its planes
/// in the "Number of …" form the rows below carry, each under a code in AWS's own
/// namespace. The rest of the service's published catalog names no quota this catalog
/// can cite under the remaining codes that listing reports, and a code identifies ONE
/// quota, so those rows are left to the default-quota-set gap rather than published
/// under a binding this catalog would be guessing at (see
/// [`super::descriptive_quota_code`]).
///
/// The service publishes two planes of ONE RESOURCE SHARE beside the account-wide
/// planes — the resources a share may hold and the principals it may be shared with —
/// and publishes both as NOT adjustable, which is why they carry the per-share
/// allowance rather than the account's.
pub(in crate::state) fn ram_quotas() -> Vec<QuotaRow> {
    vec![
        // The two planes of ONE resource share, which AWS publishes as fixed: their
        // sentences name what may be included in, or specified in, a share.
        quota("Number of resource associations per resource share", 5000.0)
            .code("L-1F7F8A25")
            .fixed()
            .describe(
                "The maximum number of resources that you can include in a resource share.",
            ),
        quota("Number of pending invitations", 250.0)
            .code("L-238C96EE")
            .describe("The maximum number of pending invitations."),
        quota("Number of principal associations per resource share", 5000.0)
            .code("L-275DAC00")
            .fixed()
            .describe(
                "The maximum number of principals that you can specify in a resource share.",
            ),
        quota("Number of custom permissions per resource type", 10.0)
            .code("L-2870BE9D")
            .describe(
                "The maximum number of custom permissions that you can create per resource \
                 type.",
            ),
        quota("Number of resource associations", 25_000.0)
            .code("L-4A6CEE66")
            .describe("The maximum number of resources associations."),
        quota("Number of resource shares", 25_000.0)
            .code("L-595828F9")
            .describe("The maximum number of resource shares."),
        quota("Number of principal associations", 25_000.0)
            .code("L-8491BF81")
            .describe("The maximum number of principal associations."),
        // The account's custom-permission plane, whose published sentence,
        // adjustability and VALUE are all the recorded listing's: the account may hold
        // far more custom permissions in total than the per-resource-type plane above
        // allows for any one type, which is why the two planes' allowances differ by
        // more than a factor of a hundred.
        quota("Number of custom permissions", 1500.0)
            .code("L-9EBA15DD")
            .describe("The maximum number of custom permissions."),
    ]
}
