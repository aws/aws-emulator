// SPDX-License-Identifier: CC0-1.0

//! AWS Direct Connect's published default-quota catalog.

use super::*;

/// The `AWS/Usage` display name AWS Direct Connect publishes its resource counts under,
/// which is the `Service` dimension of every metered `directconnect` quota.
const DIRECT_CONNECT_USAGE_SERVICE: &str = "Direct Connect";

/// The `UsageMetric` of one metered AWS Direct Connect plane: the `AWS/Usage` resource
/// count of the series the plane's count is published under.
///
/// `resource` is the dimension VALUE CloudWatch publishes that count under — the series
/// name, not the resource type — so one constructor keeps the whole block identical
/// across this service's metered rows.
fn directconnect_resource_count(resource: &str) -> MetricInfo {
    usage_metric(
        DIRECT_CONNECT_USAGE_SERVICE,
        &[
            ("Class", "None"),
            ("Resource", resource),
            ("Type", "Resource"),
        ],
    )
}

/// AWS Direct Connect's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `directconnect` service code (the
/// "AWS Direct Connect quotas" page of its User Guide), and the recorded default-quota
/// listing of that code for which `QuotaCode` names which quota, for the value and
/// adjustability each publishes, for the whole `AWS/Usage` `ResourceCount` block a
/// metered plane carries (its `Service` and `Resource` dimensions and the window those
/// counts are published over), for the resource type and scope of a per-resource plane,
/// and for the `Description` the service publishes under a code the listing states one
/// for. The remaining codes that listing reports name quotas whose published value no
/// source available here states, and a code identifies ONE quota, so those rows are left
/// to the default-quota-set gap rather than published under a guessed binding (see
/// [`crate::state::descriptive_quota_code`]).
pub(in crate::state) fn directconnect_quotas() -> Vec<QuotaRow> {
    vec![
        // The virtual-interface plane of one DEDICATED connection, published the way the
        // resource planes below are: counted per resource, with the resource type that
        // plane applies to named and metered with the service's `AWS/Usage` resource count
        // over the window those counts are published over. The recorded listing states the
        // whole metric block and the resource type; the sentence AWS publishes for the
        // quota is not among the data available here, so the row carries none.
        quota(
            "Public or private virtual interfaces per dedicated connection",
            50.0,
        )
        .code("L-0B7EBFD7")
        .fixed()
        .adjustable_at(LEVEL_ALL, "AWS::DirectConnect::Connections")
        .counted_per_resource()
        .metric(directconnect_resource_count(
            "PublicOrPrivateVIFsPerDedicatedConnection",
        )),
        quota(
            "Private or public or transit virtual interfaces per hosted connection",
            1.0,
        )
        // A plane AWS publishes as a hard limit of the hosted connection.
        .code("L-3745876E")
        .fixed()
        .describe(
            "The maximum number of private, public, or transit virtual interfaces per AWS Direct \
             Connect hosted connection.",
        ),
        // The member plane of a LAG: AWS applies this quota to an individual link
        // aggregation group as well as account-wide, so the row names the resource type
        // its per-resource plane applies to, and publishes the quota as one counted per
        // LAG — the `RESOURCE` scope every record of it reports, including the one that
        // names no resource. It meters the plane with the service's `AWS/Usage` resource
        // count, which is what sets the window the count is published over. Every
        // dimension of that metric is the recorded listing's own, `Resource` included:
        // the recorded row for this code carries the whole `UsageMetric` block, so the
        // series the count is published under is cited rather than named here.
        quota("Members per LAG", 4.0)
            .code("L-5A971579")
            .fixed()
            .adjustable_at(LEVEL_ALL, "AWS::DirectConnect::Lags")
            .counted_per_resource()
            .metric(directconnect_resource_count("MembersPerLag"))
            .describe(
                "The maximum number of dedicated connections or interconnects per link \
                 aggregation group (LAG).",
            ),
        // The prefix plane of one transit gateway, which AWS publishes as a hard limit of
        // that gateway: a quota counted per resource, whose records name the EC2 transit
        // gateway type the plane applies to. The recorded listing states no window or
        // usage metric for it, so the row carries neither.
        quota("Prefixes per transit gateway", 200.0)
            .code("L-A1004C93")
            .fixed()
            .adjustable_at(LEVEL_ALL, "AWS::EC2::TransitGateways")
            .counted_per_resource(),
        // The virtual-private-gateway plane of one Direct Connect gateway, published the
        // same way as the LAG member plane above: a quota counted per resource, whose
        // record names the resource type that plane applies to and which is metered with
        // the service's `AWS/Usage` resource count over the window those counts are
        // published over. AWS publishes it as a hard limit of the gateway.
        quota("Virtual private gateways per Direct Connect gateway", 20.0)
            .code("L-FDE06981")
            .fixed()
            .adjustable_at(LEVEL_ALL, "AWS::DirectConnect::Gateways")
            .counted_per_resource()
            .metric(directconnect_resource_count(
                "VirtualPrivateGatewaysPerDirectConnectGateway",
            ))
            .describe(
                "The maximum number of virtual private gateways per AWS Direct Connect gateway.",
            ),
    ]
}
