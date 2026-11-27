// SPDX-License-Identifier: CC0-1.0

//! Amazon VPC Lattice's published default-quota catalog.

use super::*;

/// Amazon VPC Lattice's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `vpc-lattice` service code (what
/// `aws service-quotas list-aws-default-service-quotas --service-code vpc-lattice`
/// reports, which is the "Quotas for Amazon VPC Lattice" page of its user guide), and the
/// `QuotaCode`s that catalog is keyed on.
///
/// The service's quotas are the planes of its service network — the networks of an
/// account, the services and VPCs associated with one, and the listeners, rules and
/// target groups a service routes through — so each row names the plane it counts. The
/// one row published as a RATE carries the one-second window AWS publishes it over.
///
/// The recorded listing states the quota published under `L-87FCA9B2` (the size of one
/// auth policy, in Kilobytes, NOT adjustable, with the sentence the service words it
/// with), under `L-5FF8F9B9` (the resource configurations of the Region, 2000), under
/// `L-0DCA4434` (the resource gateways of one VPC, 500, adjustable), under `L-6095700C`
/// (the resource configurations of one service network, 500), under `L-CA6A1CC5` (the
/// security groups of one association, 5, NOT adjustable), under `L-75D4A19E` (the SERVICES
/// associated with one service network, 500), under `L-73D0F278` (the domain verifications
/// of the Region, 5) and under `L-9CAD07FB` (the SERVICE NETWORKS of the Region, whose
/// published limit no source available here states, so the row keeps the one this code has
/// carried). The header-match condition count, the per-resource auth-policy count, the
/// per-service listener count, the account's target-group count, the per-listener rule count
/// and the service networks one service is associated with, which this catalog used to key
/// on those codes, are NOT published here: no source available here states AWS's code for
/// them, so they are left to the default-quota-set gap rather than served under another
/// quota's code. A row whose published `Description` no source available here states
/// carries none, rather than a sentence this emulator words itself.
pub(in crate::state) fn vpc_lattice_quotas() -> Vec<QuotaRow> {
    vec![
        quota("Resource gateways per VPC", 500.0)
            .code("L-0DCA4434")
            .describe(
                "The maximum number of resource gateways in a VPC. For additional capacity \
                 and limit increases, contact AWS Support.",
            ),
        quota("Certificate associations per service", 1.0)
            .code("L-3DEC3B9F")
            .fixed()
            .describe("The maximum number of certificates associated with one service."),
        quota("Resource configurations per AWS Region", 2000.0).code("L-5FF8F9B9"),
        quota("Resource Configurations per service network", 500.0).code("L-6095700C"),
        quota("Requests per second per service", 10_000.0)
            .code("L-620C821E")
            .period(1, "SECOND")
            .describe("The maximum number of requests per second that one service serves."),
        quota("Domain Verifications per AWS Region", 5.0).code("L-73D0F278"),
        quota("Service associations per service network", 500.0)
            .code("L-75D4A19E")
            .describe("The maximum number of services associated with one service network."),
        quota("Auth policy size", 10.0)
            .code("L-87FCA9B2")
            .unit("Kilobytes")
            .fixed()
            .describe("The maximum size of a JSON file in an Auth policy."),
        quota("Service networks per VPC", 5.0)
            .code("L-89DEA27F")
            .describe("The maximum number of service networks one VPC is associated with."),
        quota("Services per account", 500.0)
            .code("L-9BC96FEF")
            .describe("The maximum number of services in this account in the current Region."),
        // The SERVICE NETWORKS of the Region. The recorded listing states the plane this
        // code publishes but not its limit, so the row carries the limit this code has
        // carried, as the module header says such a member is kept.
        quota("Service networks per AWS Region", 500.0).code("L-9CAD07FB"),
        quota("Tags per resource", 50.0)
            .code("L-BB11C6B9")
            .fixed()
            .describe("The maximum number of tags on one VPC Lattice resource."),
        quota("Security groups per association", 5.0)
            .code("L-CA6A1CC5")
            .fixed(),
        quota("Target groups per rule", 2.0)
            .code("L-CF78395E")
            .fixed()
            .describe("The maximum number of target groups one rule forwards to."),
        quota("Targets per target group", 1000.0)
            .code("L-D64E952E")
            .describe("The maximum number of targets in one target group."),
        quota("VPC associations per service network", 500.0)
            .code("L-D71303F3")
            .describe("The maximum number of VPCs associated with one service network."),
        quota("Weighted target groups per rule", 10.0)
            .code("L-EF6E2D62")
            .fixed()
            .describe("The maximum number of weighted target groups in one rule's action."),
    ]
}
