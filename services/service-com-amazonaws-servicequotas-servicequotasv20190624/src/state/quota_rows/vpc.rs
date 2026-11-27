// SPDX-License-Identifier: CC0-1.0

//! Amazon Virtual Private Cloud's published default-quota catalog.

use super::*;

/// Amazon VPC's published default-quota catalog.
///
/// SOURCE: AWS's published Service Quotas data for the `vpc` service code (what `aws
/// service-quotas list-aws-default-service-quotas --service-code vpc` reports, which is
/// the "Amazon VPC quotas" page of the Amazon VPC User Guide), and the `QuotaCode`s that
/// catalog is keyed on.
///
/// Two rows are published as quotas OF A RESOURCE as well as of the account — the rules
/// of one security group and the routes of one route table — so each names the resource
/// type that plane applies to and the level AWS publishes the quota as adjustable at
/// ([`QuotaRow::adjustable_at`]), which for both of them is the ACCOUNT: the value is
/// applied to each resource, but an increase is asked for at the account level. Each also
/// reports the RESOURCE scope on every record of it, because the plane the quota is
/// counted on is the resource whichever record is read
/// ([`QuotaRow::counted_per_resource`]).
/// The planes CloudWatch publishes a count for carry that `AWS/Usage` `ResourceCount`
/// metric, and with it the window those counts are published over; VPC's counts sit
/// under the `EC2` service dimension, because the VPC API is served by EC2.
///
/// The recorded listing states the quota published under `L-5F53652F` (the Elastic IP
/// addresses of one NAT gateway, 2), under `L-42B9C2CA` (the VPC Block Public Access
/// exclusions of the Region, 50, NOT adjustable), under `L-CA6CC422` (the `resource` VPC
/// endpoints of one VPC, 500), under `L-8312C5BB` (the expiry of an unaccepted VPC
/// peering connection request, NOT adjustable, published as `VPC peering connection
/// expiry hours` with the hours in the quota's own text rather than in `Unit`), under
/// `L-A272D574` (the VPCs one security group may be associated with, 100) and under
/// `L-DFA99DE7` (the private IP addresses of one NAT gateway, 8). It also
/// states the sentence the service publishes for the gateway-endpoint plane of a Region,
/// the Network Address Usage of one VPC, the security groups of a Region, the network
/// interfaces of an Availability Zone and the active peering connections of one VPC (whose
/// sentence names the maximum the quota may be raised to). The prefix-list entry count, the
/// prefix-list count, the peered Network Address Usage, the prefix-list reference count and
/// the per-Region traffic-mirror-filter count this catalog used to key on those codes are
/// NOT published here: no source available here states AWS's code for them, so they are
/// left to the default-quota-set gap rather than served under another quota's code. A row
/// whose published `Description` no source available here states carries none, rather than
/// a sentence this emulator words itself.
///
/// THE CODE NO SOURCE BINDS. `L-2C462E13` is a code this catalog publishes whose own quota
/// no source available here names, so it carries the plane one of the bindings above
/// displaced — the Gateway Load Balancer endpoints of one VPC — as the module header says
/// such a row does. That is not a claim that AWS publishes that plane under that code.
pub(in crate::state) fn vpc_quotas() -> Vec<QuotaRow> {
    vec![
        // AWS publishes the IPv6 block plane of a VPC as adjustable, like the IPv4 one,
        // and at the allowance its quota table gives a VPC rather than at a single block:
        // the recorded default listing of this code reports both `Adjustable` and `Value`
        // differently from the fixed single-block row this catalog used to carry.
        quota("IPv6 CIDR blocks per VPC", 5.0)
            .code("L-085A6257")
            .describe("The maximum number of IPv6 CIDR blocks per VPC."),
        // The rule plane of ONE security group: the value is applied to each security
        // group, an increase is asked for at the account level, and CloudWatch publishes
        // the count the quota is measured against.
        quota("Inbound or outbound rules per security group", 60.0)
            .code("L-0EA8095F")
            .adjustable_at(LEVEL_ACCOUNT, "AWS::EC2::SecurityGroup")
            .counted_per_resource()
            .metric(usage_metric(
                VPC_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "RulesPerSecurityGroup"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of inbound or outbound rules per security group. This \
                 quota counts the inbound rules and the outbound rules of a security group \
                 separately.",
            ),
        quota("Outstanding VPC peering connection requests", 25.0)
            .code("L-12E49864")
            .describe(
                "The maximum number of outstanding VPC peering connection requests in this \
                 account in the current Region.",
            ),
        // A code whose own published quota no source available here states, carrying the
        // plane the `L-A272D574` binding displaced (see the header).
        quota("Gateway Load Balancer endpoints per VPC", 50.0)
            .code("L-2C462E13")
            .describe("The maximum number of Gateway Load Balancer endpoints per VPC."),
        quota("Gateway VPC endpoints per Region", 20.0)
            .code("L-1B52E74A")
            .describe(
                "The maximum number of gateway VPC endpoints per Region. The maximum is \
                 255 gateway endpoints per VPC.",
            ),
        quota("Interface VPC endpoints per VPC", 50.0)
            .code("L-29B6F2EB")
            .metric(usage_metric(
                VPC_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "InterfaceVpcEndpointsPerVpc"),
                    ("Type", "Resource"),
                ],
            ))
            .describe("The maximum number of interface VPC endpoints per VPC."),
        quota("Rules per network ACL", 20.0)
            .code("L-2AEEBF1A")
            .describe(
                "The maximum number of inbound or outbound rules per network ACL. This \
                 quota counts the inbound rules and the outbound rules of a network ACL \
                 separately.",
            ),
        quota("Security groups per network interface", 5.0)
            .code("L-2AFB9258")
            .describe("The maximum number of security groups per network interface."),
        quota("Participant accounts per VPC", 100.0)
            .code("L-3248932A")
            .describe("The maximum number of accounts one VPC's subnets are shared with."),
        quota("Subnets that can be shared with an account", 100.0)
            .code("L-3B4E38D2")
            .describe("The maximum number of subnets that can be shared with one account."),
        quota("Subnets per VPC", 200.0)
            .code("L-407747CB")
            .describe("The maximum number of subnets per VPC."),
        quota("VPC Block Public Access exclusions per account per Region", 50.0)
            .code("L-42B9C2CA")
            .fixed(),
        quota("Versions per customer-managed prefix list", 1000.0)
            .code("L-44499CD2")
            .fixed()
            .describe(
                "The maximum number of versions of one customer-managed prefix list.",
            ),
        quota("Egress-only internet gateways per Region", 5.0)
            .code("L-45FE3B85")
            .describe(
                "The maximum number of egress-only (outbound-only) internet gateways per \
                 Region. This quota is directly tied to the maximum number of VPCs per \
                 Region. To increase this quota, increase the number of VPCs per Region.",
            ),
        quota("Route tables per VPC", 200.0)
            .code("L-589F43AA")
            .describe(
                "The maximum number of route tables per VPC. The main route table counts \
                 toward this quota.",
            ),
        quota("Elastic IP address quota per NAT gateway", 2.0).code("L-5F53652F"),
        // The active peering connections of ONE VPC, whose published sentence names the
        // maximum an increase may take the quota to as well as the plane it counts.
        quota("Active VPC peering connections per VPC", 50.0)
            .code("L-7E9ECCDB")
            .describe(
                "The maximum number of active VPC peering connections per VPC. This quota \
                 can be increased up to a maximum of 125.",
            ),
        quota("VPC peering connection request expiry hours", 168.0)
            .code("L-8312C5BB")
            .fixed()
            .describe(
                "The maximum number of hours after which an unaccepted VPC peering \
                 connection request expires. The default value is 168 hours (one week).",
            ),
        quota("IPv4 CIDR blocks per VPC", 5.0)
            .code("L-83CA0A9D")
            .describe("The maximum number of IPv4 CIDR blocks per VPC."),
        // The route plane of ONE route table, published like the security-group rules
        // above: the value is applied to each route table, an increase is asked for at the
        // account level, and CloudWatch publishes the count it is measured against.
        quota("Routes per route table", 50.0)
            .code("L-93826ACB")
            .adjustable_at(LEVEL_ACCOUNT, "AWS::EC2::RouteTable")
            .counted_per_resource()
            .metric(usage_metric(
                VPC_USAGE_SERVICE,
                &[
                    ("Class", "None"),
                    ("Resource", "RoutesPerRouteTable"),
                    ("Type", "Resource"),
                ],
            ))
            .describe(
                "The maximum number of non-propagated routes per route table. This quota \
                 can be increased up to a maximum of 1000; however, network performance \
                 might be impacted.",
            ),
        quota("VPC associations per security group", 100.0).code("L-A272D574"),
        quota("Internet gateways per Region", 5.0)
            .code("L-A4707A72")
            .describe(
                "The maximum number of internet gateways per Region. This quota is directly \
                 tied to the maximum number of VPCs per Region. To increase this quota, \
                 increase the number of VPCs per Region.",
            ),
        quota("Network ACLs per VPC", 200.0)
            .code("L-B4A6D682")
            .describe("The maximum number of network ACLs per VPC."),
        quota("Network Address Usage", 64_000.0)
            .code("L-BB24F6E5")
            .describe("The maximum Network Address Usage for a single VPC."),
        quota("VPC endpoints of type 'resource' per VPC", 500.0).code("L-CA6CC422"),
        quota("Traffic mirror sessions per Region", 10_000.0)
            .code("L-CD17FD4B")
            .describe(
                "The maximum number of traffic mirror sessions in this account in the \
                 current Region.",
            ),
        quota("Traffic mirror targets per Region", 10_000.0)
            .code("L-DC9F7029")
            .describe(
                "The maximum number of traffic mirror targets in this account in the \
                 current Region.",
            ),
        quota("Network interfaces per Region", 5000.0)
            .code("L-DF5E4CA3")
            .describe(
                "The maximum number of network interfaces per Availability Zone in a \
                 Region.",
            ),
        quota("Private IP address quota per NAT gateway", 8.0).code("L-DFA99DE7"),
        quota("VPC security groups per Region", 2500.0)
            .code("L-E79EC296")
            .describe("The maximum number of VPC security groups per Region."),
        // The VPC plane of the Region, which AWS publishes with all three of the optional
        // members a `ServiceQuota` can carry: a description, the CloudWatch usage metric
        // the count is measured by, and the window that metric is published over.
        quota("VPCs per Region", 5.0)
            .code("L-F678F1CE")
            .metric(vpcs_per_region_usage_metric())
            .period(5, "MINUTE")
            .describe(
                "The maximum number of VPCs per Region. This quota is directly tied to the \
                 maximum number of internet gateways per Region.",
            ),
        quota("NAT gateways per Availability Zone", 5.0)
            .code("L-FE5A380F")
            .describe("The maximum number of NAT gateways per Availability Zone."),
    ]
}

/// The name `AWS/Usage` publishes Amazon VPC's resource counts under.
///
/// The VPC resource counts CloudWatch publishes sit under the EC2 service dimension —
/// the VPC API is served by EC2 — so that is the `Service` dimension a `GetMetricData`
/// query names for one of them. It is held once here rather than spelled at each row.
const VPC_USAGE_SERVICE: &str = "EC2";
