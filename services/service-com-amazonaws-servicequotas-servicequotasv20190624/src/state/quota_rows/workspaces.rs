// SPDX-License-Identifier: CC0-1.0

//! Amazon WorkSpaces' published default-quota catalog.

use super::*;

/// Amazon WorkSpaces' published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `workspaces` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code workspaces` reports,
/// which is the "Amazon WorkSpaces quotas" page of its administration guide), and the
/// `QuotaCode`s that catalog is keyed on.
///
/// WorkSpaces publishes a quota PER HARDWARE BUNDLE class — the WorkSpaces of each
/// graphics family start at zero for a new account, so running one at all needs an
/// increase — beside the image, directory and access-control planes of the account and
/// the request rate of each of its APIs. A rate row is built by [`request_rate`], so it
/// carries the one-second window AWS publishes it over and the `AWS/Usage` `CallCount` of
/// the API it throttles; a bundle row is metered instead by the `AWS/Usage`
/// `ResourceCount` of that bundle's own series, which carries the window those counts are
/// published over ([`graphics_bundle`]).
///
/// The recorded listing states the quota published under `L-40A36AF9`
/// (`Graphics.g6.4xlarge WorkSpaces`), under `L-BD784BE1` (`Graphics.g6.16xlarge
/// WorkSpaces`), under `L-63F14199` (`Graphics.g7.2xlarge WorkSpaces`), under
/// `L-F64A92CF` (`Graphics.g7.4xlarge WorkSpaces`) and under `L-8C70C791` (the General
/// Purpose Value streaming instances of the account's WorkSpaces Pools, 50), every one of
/// them metered by that plane's resource count. The connection-alias association count,
/// the `DescribeAccount` and `CreateTags` rates, the per-directory certificate count and
/// the `StartWorkspaces` rate this catalog used to key on those codes are NOT published
/// here: no source available here states AWS's code for them, so they are left to the
/// default-quota-set gap rather than served under another quota's code. A row whose
/// published `Description` no source available here states carries none, rather than a
/// sentence this emulator words itself.
pub(in crate::state) fn workspaces_quotas() -> Vec<QuotaRow> {
    let rate = |code: &str, api: &str, per_second: f64| {
        request_rate(code, WORKSPACES_USAGE_SERVICE, api, per_second)
    };
    vec![
        quota("WorkSpaces", 1.0)
            .code("L-011B3C20")
            .describe(
                "The maximum number of WorkSpaces in this account in the current Region.",
            ),
        quota("Graphics WorkSpaces", 0.0)
            .code("L-0798A2C9")
            .describe(
                "The maximum number of Graphics WorkSpaces in this account in the current \
                 Region.",
            ),
        quota("GraphicsPro WorkSpaces", 0.0)
            .code("L-07C6D3A5")
            .describe(
                "The maximum number of GraphicsPro WorkSpaces in this account in the \
                 current Region.",
            ),
        quota("Graphics.g4dn WorkSpaces", 0.0)
            .code("L-0B7D5CA2")
            .describe(
                "The maximum number of Graphics.g4dn WorkSpaces in this account in the \
                 current Region.",
            ),
        quota("GraphicsPro.g4dn WorkSpaces", 0.0)
            .code("L-0E312A12")
            .describe(
                "The maximum number of GraphicsPro.g4dn WorkSpaces in this account in the \
                 current Region.",
            ),
        quota("Standby WorkSpaces", 0.0)
            .code("L-158CE743")
            .describe(
                "The maximum number of standby WorkSpaces in this account in the current \
                 Region.",
            ),
        quota("WorkSpaces Pools", 0.0)
            .code("L-18CE281C")
            .describe(
                "The maximum number of WorkSpaces pools in this account in the current \
                 Region.",
            ),
        quota("WorkSpaces Personal bundles", 25.0)
            .code("L-18F1706C")
            .describe(
                "The maximum number of custom WorkSpaces Personal bundles in this account \
                 in the current Region.",
            ),
        quota("Images", 5.0)
            .code("L-254B485B")
            .describe(
                "The maximum number of WorkSpaces images in this account in the current \
                 Region.",
            ),
        quota("Image copy requests in progress", 5.0)
            .code("L-2F7BEDD3")
            .fixed()
            .describe("The maximum number of image copy requests running at once."),
        quota("IP access control groups per Region", 100.0)
            .code("L-337DA39D")
            .describe(
                "The maximum number of IP access control groups in this account in the \
                 current Region.",
            ),
        quota("Rules per IP access control group", 10.0)
            .code("L-34278094")
            .fixed()
            .describe("The maximum number of rules in one IP access control group."),
        quota("IP access control groups per directory", 25.0)
            .code("L-34D55C89")
            .fixed()
            .describe(
                "The maximum number of IP access control groups associated with one \
                 directory.",
            ),
        quota("Connection aliases per account", 20.0)
            .code("L-3AC69A9E")
            .describe(
                "The maximum number of connection aliases in this account in the current \
                 Region.",
            ),
        graphics_bundle("L-40A36AF9", "Graphics.g6.4xlarge", "GraphicsG64xlargeWorkSpace"),
        quota("Registered directories", 50.0)
            .code("L-465DA8AF")
            .describe(
                "The maximum number of directories registered with WorkSpaces in this \
                 account in the current Region.",
            ),
        quota("Tags per resource", 50.0)
            .code("L-5782CB4D")
            .fixed()
            .describe("The maximum number of tags on one WorkSpaces resource."),
        // The Graphics.g7.2xlarge bundle plane, published like the other bundle rows:
        // the series its count is published under follows the one the recorded
        // `Graphics.g6.4xlarge` and `Graphics.g6.16xlarge` rows carry.
        graphics_bundle("L-63F14199", "Graphics.g7.2xlarge", "GraphicsG72xlargeWorkSpace"),
        rate("L-6E205E73", "AssociateIpGroups", 5.0),
        rate("L-70A02F17", "AuthorizeIpRules", 5.0),
        rate("L-843E43DA", "CreateIpGroup", 5.0),
        // The streaming-instance plane of the account's WorkSpaces Pools, metered by that
        // plane's own resource count rather than by a call count.
        metered_plane(
            "L-8C70C791",
            "General Purpose Value streaming instances for WorkSpaces Pools".to_string(),
            50.0,
            "GeneralPurposeValueStreamingInstancesForWorkSpacesPool",
        ),
        rate("L-925915DC", "CreateWorkspaces", 5.0),
        rate("L-9A67B5CB", "DeleteIpGroup", 5.0),
        rate("L-9C1F7EB2", "DeleteTags", 5.0),
        rate("L-BCACAEBC", "DeleteWorkspaceImage", 5.0),
        graphics_bundle(
            "L-BD784BE1",
            "Graphics.g6.16xlarge",
            "GraphicsG616xlargeWorkSpace",
        ),
        rate("L-BE9A8466", "DescribeIpGroups", 10.0),
        rate("L-C266A5F4", "DescribeTags", 10.0),
        rate("L-C76331EA", "DescribeWorkspaceBundles", 10.0),
        rate("L-D34D6523", "DescribeWorkspaceDirectories", 10.0),
        rate("L-D6CFBE96", "DescribeWorkspaces", 10.0),
        rate("L-E2CD2905", "DescribeWorkspacesConnectionStatus", 10.0),
        rate("L-EEB759DE", "ModifyWorkspaceProperties", 5.0),
        rate("L-EF6817EC", "RebootWorkspaces", 5.0),
        rate("L-F61D0B79", "RebuildWorkspaces", 5.0),
        graphics_bundle("L-F64A92CF", "Graphics.g7.4xlarge", "GraphicsG74xlargeWorkSpace"),
        rate("L-F7854FE2", "StopWorkspaces", 5.0),
    ]
}

/// The name `AWS/Usage` publishes Amazon WorkSpaces' API call counts under.
///
/// It is the `Service` dimension value a `GetMetricData` query has to name for one of
/// these counts, so it is held once here rather than spelled at each row.
const WORKSPACES_USAGE_SERVICE: &str = "WorkSpaces";

/// One Amazon WorkSpaces GRAPHICS BUNDLE quota: how many WorkSpaces of that hardware
/// bundle the account may run in the Region.
///
/// SOURCE: the recorded default-quota listing of `workspaces`, which publishes such a row
/// under the name `<Bundle> WorkSpaces`, at the zero a new account starts at, and metered
/// like every other plane of the account ([`metered_plane`]). Those are the quota's
/// published members, so one constructor fills them and two rows of the class cannot drift
/// apart.
fn graphics_bundle(code: &str, bundle: &str, usage_resource: &str) -> QuotaRow {
    metered_plane(code, format!("{bundle} WorkSpaces"), 0.0, usage_resource)
}

/// One Amazon WorkSpaces plane CloudWatch publishes a RESOURCE COUNT for: how many of
/// that plane's resources the account may run in the Region.
///
/// SOURCE: the recorded default-quota listing of `workspaces`, which publishes such a row
/// as adjustable and metered by the `AWS/Usage` `ResourceCount` of the plane's own
/// `Resource` series, over the window those counts are published over. It is the class the
/// bundle planes and the pool streaming-instance planes share, so one constructor fills it
/// and two rows of the class cannot drift apart.
fn metered_plane(
    code: &str,
    quota_name: String,
    value: f64,
    usage_resource: &str,
) -> QuotaRow {
    quota(quota_name, value)
        .code(code)
        .metric(usage_metric(
            WORKSPACES_USAGE_SERVICE,
            &[
                ("Class", "None"),
                ("Resource", usage_resource),
                ("Type", "Resource"),
            ],
        ))
}
