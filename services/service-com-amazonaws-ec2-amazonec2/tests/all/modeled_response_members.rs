// SPDX-License-Identifier: CC0-1.0

//! The MODELLED MEMBERS a successful response carries, over the operations whose answers
//! are built from several stores at once.
//!
//! THE RULE: a response reports every member of its modelled output shape whose value this
//! emulator knows — a member whose value is EMPTY as the empty element, a LIST member as a
//! list with one entry per value it holds, and a member AWS publishes per Region out of the
//! catalog that holds it — rather than leaving the member out. A client that reads a member
//! to decide something (is this volume snapshot-derived, which physical zone is it in, which
//! groups did this launch use, which tags does this address carry) can only decide it if the
//! member is on the wire.
//!
//! Every call goes over the wire the way a client makes it: an
//! `application/x-www-form-urlencoded` ec2Query POST through the same `MultiServiceRouter`
//! the emulator serves with. Bodies are read through the XML DOCUMENT, never by substring,
//! because the same element name occurs at several depths (an instance's `groupSet` and its
//! reservation's, a zone's `geographySet` entries and the zone list's own `item`s).
//!
//! Each table's rows differ along what the rule generalises over — which Region the display
//! data comes from, which zone a volume is created in and how the request names it, how many
//! groups a launch names and how many instances it starts, how many versions a launch
//! template holds, and how many tags a resource carries — because the rule is about every
//! response of these shapes and not about one recorded call.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::TRANSITION_DELAY;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, element, elements, filters, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region most calls here are signed for: the Region the recorded traces used, and one
/// whose display data AWS publishes.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `LaunchTemplateData.<member>` parameter.
fn data(member: &str, value: &str) -> (String, String) {
    p(&format!("LaunchTemplateData.{member}"), value)
}

fn emulator() -> MultiServiceRouter {
    ec2_query::emulator().0
}

/// One call signed for `REGION` that must succeed, as its body.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// The id of a published Amazon Linux 2023 AMI of this account's catalog — an image whose
/// machine facts an instance of it reports, so the member table below asks about a launch
/// with facts to read rather than one from an AMI nobody published.
async fn catalog_image(router: &MultiServiceRouter) -> String {
    let body = ok(
        router,
        "the published catalog",
        "DescribeImages",
        &filters(&[(
            "public-ssm-parameter-name",
            &["/aws/service/ami-amazon-linux-latest/al2023-ami-kernel-6.1-x86_64"],
        )]),
    )
    .await;
    element(&body, "imageId")
        .unwrap_or_else(|| panic!("the catalog must publish an AL2023 image: {body}"))
}

/// One row of the display-data table: the Region, and the zone-group long name, the
/// geography and the sub-geography its zones must report — each `None` where AWS publishes
/// none.
type DisplayCase<'a> = (&'a str, Option<&'a str>, Option<&'a str>, Option<&'a str>);

/// One row of the launch table: the case, how many instances it starts, whether it names
/// the caller's own subnet, which of the created groups it names, and whether its subnet
/// maps a public IPv4 address on launch.
type LaunchCase<'a> = (&'a str, &'a str, Option<&'a str>, &'a [usize], bool);

// ---------------------------------------------------------------------------
// The Region display data a zone reports
// ---------------------------------------------------------------------------

/// `DescribeAvailabilityZones` reports the display data AWS publishes for the zone's
/// Region: the zone group's long name, the geography as a LIST, and the sub-geography as a
/// list — and reports none of the three for a Region AWS publishes no display name for.
///
/// The rows differ in what the Region's published display name names: a state of a country
/// (`US West (Oregon)`), a city (`Europe (Frankfurt)`), the country itself (`Europe
/// (Ireland)`, which therefore has no place below it), a part of a country with no
/// published place (`AWS GovCloud (US-West)`) and an isolated Region, for which AWS
/// publishes no display data at all. The values are the display data AWS publishes for each
/// Region; `US West (Oregon) 1` is the example in the operation's own `group-long-name`
/// filter doc
/// (`smithy_query(query="doc",shape="DescribeAvailabilityZonesRequest",member="Filters")`),
/// and `United States of America` / `Oregon` are the examples on
/// `AvailabilityZoneGeography$Name` and `AvailabilityZoneSubGeography$Name`.
#[tokio::test]
async fn availability_zones_report_the_display_data_of_their_region() {
    let router = emulator();
    let cases: [DisplayCase; 6] = [
        (
            "us-west-2",
            Some("US West (Oregon) 1"),
            Some("United States of America"),
            Some("Oregon"),
        ),
        (
            "us-east-2",
            Some("US East (Ohio) 1"),
            Some("United States of America"),
            Some("Ohio"),
        ),
        (
            "eu-central-1",
            Some("Europe (Frankfurt) 1"),
            Some("Germany"),
            Some("Frankfurt"),
        ),
        ("eu-west-1", Some("Europe (Ireland) 1"), Some("Ireland"), None),
        (
            "us-gov-west-1",
            Some("AWS GovCloud (US-West) 1"),
            Some("United States of America"),
            None,
        ),
        ("us-iso-east-1", None, None, None),
    ];

    for (region, group_long_name, geography, sub_geography) in cases {
        let (status, body) =
            ec2_query::call(&router, region, "DescribeAvailabilityZones", &[]).await;
        assert_eq!(
            status, 200,
            "case {region:?}: expected HTTP 200, got {status} with body {body}"
        );
        let parsed = document(region, &body);
        let zones: Vec<roxmltree::Node<'_, '_>> = parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "availabilityZoneInfo")
            .map(|list| {
                list.children()
                    .filter(|node| node.is_element() && node.tag_name().name() == "item")
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !zones.is_empty(),
            "case {region:?}: the Region must report its zones, got {body}"
        );
        for zone in zones {
            let name = child_text(zone, "zoneName").unwrap_or_default();
            assert_eq!(
                child_text(zone, "groupLongName").as_deref(),
                group_long_name,
                "case {region:?}: expected the zone group long name {group_long_name:?} on \
                 {name}, got {body}"
            );
            // Each geography is a LIST member on the wire whose entries carry a `name`.
            for (member, expected) in [
                ("geographySet", geography),
                ("subGeographySet", sub_geography),
            ] {
                let reported: Vec<String> = set_items(zone, member)
                    .into_iter()
                    .filter_map(|entry| child_text(entry, "name"))
                    .collect();
                let wanted: Vec<String> =
                    expected.map(ToString::to_string).into_iter().collect();
                assert_eq!(
                    reported, wanted,
                    "case {region:?}: expected <{member}> {wanted:?} on {name}, got {body}"
                );
                assert_eq!(
                    child(zone, member).is_some(),
                    expected.is_some(),
                    "case {region:?}: <{member}> is reported exactly when the Region \
                     publishes one, got {body}"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// A volume's snapshot, zone id and operator
// ---------------------------------------------------------------------------

/// A created volume reports `snapshotId` as the EMPTY element it is for a volume created
/// from no snapshot, the zone id of the zone it was created in, and — read back through
/// `DescribeVolumes` — the same `snapshotId` beside the `operator` every unmanaged resource
/// carries.
///
/// The rows differ in which zone of the Region the volume is created in, in the size it is
/// created with, and in whether the request names its zone by NAME (`AvailabilityZone`) or
/// by ID (`AvailabilityZoneId`) — the zone id reported is in every row the one
/// `DescribeAvailabilityZones` gives for that zone, read back from the service rather than
/// written here, so the two operations cannot report one zone two ways.
#[tokio::test]
async fn a_volume_reports_its_snapshot_its_zone_id_and_its_operator() {
    let router = emulator();

    // The Region's zones as the service reports them: `(zone name, zone id)`.
    let body = ok(&router, "the Region's zones", "DescribeAvailabilityZones", &[]).await;
    let parsed = document("the Region's zones", &body);
    let zones: Vec<(String, String)> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "zoneId").is_some())
        .filter_map(|zone| Some((child_text(zone, "zoneName")?, child_text(zone, "zoneId")?)))
        .collect();
    assert!(
        zones.len() >= 3,
        "the Region must report at least three zones, got {zones:?}"
    );

    // (case, the zone index of the Region, the size, whether the request names the zone id)
    let cases: [(&str, usize, &str, bool); 4] = [
        ("the first zone, by name", 0, "8", false),
        ("the second zone, by name", 1, "1", false),
        ("the third zone, by name", 2, "1024", false),
        ("the second zone, by id", 1, "16", true),
    ];

    for (case, index, size, by_zone_id) in cases {
        let (zone_name, zone_id) = zones[index].clone();
        let params = vec![
            if by_zone_id {
                p("AvailabilityZoneId", &zone_id)
            } else {
                p("AvailabilityZone", &zone_name)
            },
            p("Size", size),
        ];
        let created = ok(&router, case, "CreateVolume", &params).await;
        let volume_id = element(&created, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: the create must report its volume: {created}"));
        let parsed = document(case, &created);
        let volume = parsed.root_element();
        assert_eq!(
            child_text(volume, "snapshotId").as_deref(),
            Some(""),
            "case {case:?}: a volume created from no snapshot carries the empty \
             <snapshotId>, got {created}"
        );
        assert_eq!(
            child_text(volume, "availabilityZone").as_deref(),
            Some(zone_name.as_str()),
            "case {case:?}: expected the zone {zone_name}, got {created}"
        );
        assert_eq!(
            child_text(volume, "availabilityZoneId").as_deref(),
            Some(zone_id.as_str()),
            "case {case:?}: expected the zone id DescribeAvailabilityZones gives for \
             {zone_name} ({zone_id}), got {created}"
        );

        // The same volume, read back: the snapshot member again, and the operator.
        let described = ok(
            &router,
            case,
            "DescribeVolumes",
            &indexed("VolumeId", &[volume_id.as_str()]),
        )
        .await;
        let parsed = document(case, &described);
        let item = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "volumeId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: the describe must report the volume: {described}"));
        assert_eq!(
            child_text(item, "snapshotId").as_deref(),
            Some(""),
            "case {case:?}: the describe carries the empty <snapshotId> too, got {described}"
        );
        assert_eq!(
            child_text(item, "availabilityZoneId").as_deref(),
            Some(zone_id.as_str()),
            "case {case:?}: the describe reports the same zone id, got {described}"
        );
        let operator = child(item, "operator")
            .unwrap_or_else(|| panic!("case {case:?}: the volume must carry an <operator>: {described}"));
        assert_eq!(
            (
                child_text(operator, "managed").as_deref(),
                child_text(operator, "hiddenByDefault").as_deref(),
            ),
            (Some("false"), Some("false")),
            "case {case:?}: no service provider manages the caller's own volume, got \
             {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// A launch's reservation and its instances
// ---------------------------------------------------------------------------

/// Every member of an instance that is a fact of the instance rather than of what the
/// launch asked for, so a response that omits one cannot be told from a response about an
/// instance that lacks it.
///
/// The list is the member set the `Instance` shape declares
/// (`smithy_query(query="shape",shape="Instance")`) that this emulator holds a value for,
/// spelled as the wire spells each one.
const INSTANCE_MEMBERS: [&str; 32] = [
    "blockDeviceMapping",
    "bootMode",
    "capacityReservationSpecification",
    "cpuOptions",
    "currentInstanceBootMode",
    "dnsName",
    "enaSupport",
    "enclaveOptions",
    "groupSet",
    "hibernationOptions",
    "hypervisor",
    "instanceId",
    "instanceState",
    "instanceType",
    "maintenanceOptions",
    "metadataOptions",
    "monitoring",
    "networkInterfaceSet",
    "networkPerformanceOptions",
    "operator",
    "placement",
    "platformDetails",
    "privateDnsNameOptions",
    "productCodes",
    "rootDeviceName",
    "rootDeviceType",
    "secondaryInterfaceSet",
    "sourceDestCheck",
    "subnetId",
    "usageOperation",
    "usageOperationUpdateTime",
    "virtualizationType",
];

/// One reservation as a response reports it: the groups that apply to all of its instances,
/// and its instances.
#[derive(Debug, Clone)]
struct Reservation {
    groups: Vec<(String, String)>,
    instances: Vec<InstanceItem>,
}

/// The members of one reported instance this suite reads.
#[derive(Debug, Clone)]
struct InstanceItem {
    instance_id: String,
    members: Vec<String>,
    groups: Vec<(String, String)>,
    state: Option<String>,
    state_reason: Option<(String, String)>,
    public_ip: Option<String>,
    public_dns_name: Option<String>,
    private_dns_name: Option<String>,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    placement: Vec<(String, String)>,
    interfaces: usize,
}

/// The `(groupId, groupName)` pairs of `node`'s own `groupSet` — its DIRECT child, so an
/// instance's groups are never read as its reservation's and the reverse.
fn group_pairs(node: roxmltree::Node<'_, '_>) -> Vec<(String, String)> {
    set_items(node, "groupSet")
        .into_iter()
        .map(|item| {
            (
                child_text(item, "groupId").unwrap_or_default(),
                child_text(item, "groupName").unwrap_or_default(),
            )
        })
        .collect()
}

/// Every reservation a `RunInstances` or `DescribeInstances` body reports.
fn reservations(case: &str, body: &str) -> Vec<Reservation> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "reservationId").is_some())
        .map(|node| Reservation {
            groups: group_pairs(node),
            instances: set_items(node, "instancesSet")
                .into_iter()
                .map(|item| InstanceItem {
                    instance_id: child_text(item, "instanceId").unwrap_or_default(),
                    members: item
                        .children()
                        .filter(roxmltree::Node::is_element)
                        .map(|member| member.tag_name().name().to_string())
                        .collect(),
                    groups: group_pairs(item),
                    state: child(item, "instanceState")
                        .and_then(|state| child_text(state, "name")),
                    state_reason: child(item, "stateReason").map(|reason| {
                        (
                            child_text(reason, "code").unwrap_or_default(),
                            child_text(reason, "message").unwrap_or_default(),
                        )
                    }),
                    public_ip: child_text(item, "ipAddress"),
                    public_dns_name: child_text(item, "dnsName"),
                    private_dns_name: child_text(item, "privateDnsName"),
                    subnet_id: child_text(item, "subnetId"),
                    vpc_id: child_text(item, "vpcId"),
                    placement: child(item, "placement")
                        .map(|placement| {
                            placement
                                .children()
                                .filter(roxmltree::Node::is_element)
                                .map(|member| {
                                    (
                                        member.tag_name().name().to_string(),
                                        member.text().unwrap_or_default().to_string(),
                                    )
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    interfaces: set_items(item, "networkInterfaceSet").len(),
                })
                .collect(),
        })
        .collect()
}

/// The one reservation a body must report.
fn one_reservation(case: &str, body: &str) -> Reservation {
    let mut reported = reservations(case, body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one reservation, got {reported:?} from {body}"
    );
    reported.remove(0)
}

/// A launch reports the groups that apply to all the instances of the launch request as a
/// LIST, every modelled member of each instance it minted, and the same of both through
/// `DescribeInstances`.
///
/// The rows differ in how many groups the launch names (none, so the VPC's default group
/// applies; one; two), in how many instances it starts (so the reservation's groups are the
/// ones common to more than one instance), and in whether it is placed in a subnet that maps
/// public IPv4 addresses on launch — the default subnet of the zone — or in one of the
/// caller's own, which does not.
#[tokio::test]
async fn a_launch_reports_its_reservation_groups_and_every_instance_member() {
    let router = emulator();
    let image_id = catalog_image(&router).await;

    // A VPC of the caller's own, with a subnet that maps no public address on launch and
    // two groups to name.
    let vpc = ok(&router, "the caller's VPC", "CreateVpc", &[p("CidrBlock", "10.40.0.0/16")]).await;
    let vpc_id = element(&vpc, "vpcId").unwrap_or_else(|| panic!("CreateVpc must report its VPC: {vpc}"));
    let subnet = ok(
        &router,
        "the caller's subnet",
        "CreateSubnet",
        &[p("VpcId", &vpc_id), p("CidrBlock", "10.40.1.0/24")],
    )
    .await;
    let subnet_id = element(&subnet, "subnetId")
        .unwrap_or_else(|| panic!("CreateSubnet must report its subnet: {subnet}"));
    let mut group_ids: Vec<String> = Vec::new();
    for (index, name) in ["members-left", "members-right"].into_iter().enumerate() {
        let body = ok(
            &router,
            name,
            "CreateSecurityGroup",
            &[
                p("GroupName", name),
                p("GroupDescription", "a group this suite launches with"),
                p("VpcId", &vpc_id),
            ],
        )
        .await;
        group_ids.push(
            element(&body, "groupId")
                .unwrap_or_else(|| panic!("group {index}: the create must report its group: {body}")),
        );
    }

    // (case, instance count, the subnet named, the groups named, a public address expected)
    let cases: [LaunchCase; 4] = [
        ("one instance in the zone's default subnet", "1", None, &[], true),
        ("two instances in the zone's default subnet", "2", None, &[], true),
        (
            "one instance in the caller's subnet, one group named",
            "1",
            Some("own"),
            &[0],
            false,
        ),
        (
            "two instances in the caller's subnet, two groups named",
            "2",
            Some("own"),
            &[0, 1],
            false,
        ),
    ];

    for (case, count, subnet, named, expect_public_address) in cases {
        let mut params = vec![
            p("ImageId", &image_id),
            p("MinCount", count),
            p("MaxCount", count),
        ];
        if subnet.is_some() {
            params.push(p("SubnetId", &subnet_id));
        }
        for (index, group) in named.iter().enumerate() {
            params.push(p(
                &format!("SecurityGroupId.{}", index + 1),
                &group_ids[*group],
            ));
        }
        let launched = ok(&router, case, "RunInstances", &params).await;
        let reservation = one_reservation(case, &launched);
        assert_eq!(
            reservation.instances.len(),
            count.parse::<usize>().unwrap_or_default(),
            "case {case:?}: expected {count} instance(s), got {reservation:?}"
        );

        // "Not supported" (`Reservation$Groups`): the reservation's own groupSet is the
        // retired EC2-Classic member, so it is reported EMPTY whatever groups the launch
        // named — a VPC instance's groups live on its primary network interface and are
        // reported there, as the instance's own groupSet.
        assert!(
            reservation.groups.is_empty(),
            "case {case:?}: the reservation's groupSet is empty, got {:?}",
            reservation.groups
        );
        let instance_groups = reservation.instances[0].groups.clone();
        assert_eq!(
            instance_groups.len(),
            named.len().max(1),
            "case {case:?}: expected {} group(s) in the instance's groupSet, got {instance_groups:?}",
            named.len().max(1)
        );
        for group in named {
            assert!(
                instance_groups
                    .iter()
                    .any(|(group_id, _)| group_id == &group_ids[*group]),
                "case {case:?}: expected the named group {} among {instance_groups:?}",
                group_ids[*group]
            );
        }
        for (group_id, group_name) in &instance_groups {
            assert!(
                !group_id.is_empty() && !group_name.is_empty(),
                "case {case:?}: each group is reported by id AND name, got {instance_groups:?}"
            );
        }

        for instance in &reservation.instances {
            // Every member of the shape this emulator holds a value for is on the wire.
            for member in INSTANCE_MEMBERS {
                assert!(
                    instance.members.iter().any(|name| name == member),
                    "case {case:?}: the launch's instance must carry <{member}>, got {:?} \
                     in {launched}",
                    instance.members
                );
            }
            // The reason for the most recent transition of an instance still being
            // launched is that launch itself.
            assert_eq!(
                instance.state.as_deref(),
                Some("pending"),
                "case {case:?}: a just-launched instance is pending, got {instance:?}"
            );
            assert_eq!(
                instance.state_reason,
                Some(("pending".to_string(), "pending".to_string())),
                "case {case:?}: expected the pending state reason, got {instance:?}"
            );
            // Where it runs, read off the subnet the launch placed it in.
            assert!(
                instance.subnet_id.as_deref().is_some_and(|id| !id.is_empty()),
                "case {case:?}: an instance in a VPC reports its subnet, got {instance:?}"
            );
            assert!(
                instance.vpc_id.as_deref().is_some_and(|id| !id.is_empty()),
                "case {case:?}: an instance in a VPC reports its VPC, got {instance:?}"
            );
            if let Some("own") = subnet {
                assert_eq!(
                    instance.subnet_id.as_deref(),
                    Some(subnet_id.as_str()),
                    "case {case:?}: expected the subnet the launch named, got {instance:?}"
                );
                assert_eq!(
                    instance.vpc_id.as_deref(),
                    Some(vpc_id.as_str()),
                    "case {case:?}: expected that subnet's VPC, got {instance:?}"
                );
            }
            assert_eq!(
                instance.interfaces, 1,
                "case {case:?}: the launch's primary interface is reported, got {instance:?}"
            );
            for member in ["availabilityZone", "availabilityZoneId", "groupName", "tenancy"] {
                assert!(
                    instance.placement.iter().any(|(name, _)| name == member),
                    "case {case:?}: the placement must carry {member}, got {instance:?}"
                );
            }
            // The private hostname names the private IPv4 address the LAUNCH assigned the
            // instance, so the launch's own answer already carries it.
            assert_eq!(
                instance.private_dns_name.as_deref().map(str::is_empty),
                Some(false),
                "case {case:?}: the launch reports the private hostname of the address it \
                 assigned, got {instance:?}"
            );
            // A subnet that maps public IPv4 addresses on launch gives its instances one,
            // and the public DNS name names exactly that address.
            assert_eq!(
                instance.public_ip.as_deref().is_some_and(|ip| !ip.is_empty()),
                expect_public_address,
                "case {case:?}: expected a public address: {expect_public_address}, got \
                 {instance:?}"
            );
            let expected_dns_name = match instance.public_ip.as_deref() {
                Some(public_ip) if !public_ip.is_empty() => format!(
                    "ec2-{}.{REGION}.compute.amazonaws.com",
                    public_ip.replace('.', "-")
                ),
                _ => String::new(),
            };
            assert_eq!(
                instance.public_dns_name.as_deref(),
                Some(expected_dns_name.as_str()),
                "case {case:?}: the public DNS name must name the address the instance \
                 holds, got {instance:?}"
            );
        }

        // The same reservation, read back: the groups and the member set again, now with
        // the private DNS name the instance has once it is running.
        past_the_transition().await;
        let ids: Vec<String> = reservation
            .instances
            .iter()
            .map(|instance| instance.instance_id.clone())
            .collect();
        let described = ok(
            &router,
            case,
            "DescribeInstances",
            &indexed(
                "InstanceId",
                &ids.iter().map(String::as_str).collect::<Vec<&str>>(),
            ),
        )
        .await;
        let read_back = one_reservation(case, &described);
        assert_eq!(
            read_back.groups, reservation.groups,
            "case {case:?}: the describe reports the same reservation groups, got \
             {read_back:?}"
        );
        for instance in &read_back.instances {
            for member in INSTANCE_MEMBERS {
                assert!(
                    instance.members.iter().any(|name| name == member),
                    "case {case:?}: the described instance must carry <{member}>, got {:?} \
                     in {described}",
                    instance.members
                );
            }
            assert_eq!(
                instance.state.as_deref(),
                Some("running"),
                "case {case:?}: the instance must be running once its span has passed, got \
                 {instance:?}"
            );
            assert!(
                instance
                    .private_dns_name
                    .as_deref()
                    .is_some_and(|name| name.ends_with(".compute.internal")),
                "case {case:?}: a running instance reports its private DNS name, got \
                 {instance:?}"
            );
            assert_eq!(
                instance.groups,
                reservation.instances[0].groups,
                "case {case:?}: the instance keeps the groups its launch gave it, got \
                 {instance:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// A launch template's version counters and operator
// ---------------------------------------------------------------------------

/// One row of the launch-interface table: the case, the `NetworkInterfaces` entries the
/// launch names as `(member, value)` pairs, and whether eth0 must receive the public IPv4
/// address its subnet maps on launch.
type InterfaceCase<'a> = (&'a str, &'a [&'a [(&'a str, &'a str)]], bool);

/// The subnet's own answer is taken only where such an address CAN go: a launch that names
/// one interface in a mapping subnet receives the address, and the same launch naming a
/// second interface receives none — the restrictions
/// `InstanceNetworkInterfaceSpecification$AssociatePublicIpAddress` states ("can only be
/// assigned to a network interface for eth0", "you cannot specify more than one network
/// interface in the request") bind the subnet's implicit answer as well as an explicit
/// request for one.
///
/// The rows differ in how many interfaces the launch names and at which device indexes,
/// all in the SAME mapping subnet — the default subnet of the zone, resolved from a launch
/// rather than written here.
#[tokio::test]
async fn a_mapping_subnet_gives_an_address_only_where_one_can_go() {
    let router = emulator();
    let image_id = catalog_image(&router).await;
    let launch = |params: Vec<(String, String)>| {
        let mut fields = vec![
            p("ImageId", &image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ];
        fields.extend(params);
        fields
    };

    // The default subnet of the launch's zone, read off a launch that names none.
    let body = ok(&router, "the zone's default subnet", "RunInstances", &launch(Vec::new())).await;
    let default_subnet = one_reservation("the zone's default subnet", &body).instances[0]
        .subnet_id
        .clone()
        .unwrap_or_else(|| panic!("a launch must report the subnet it was placed in: {body}"));

    // (case, the interfaces the launch names, whether an address is expected on eth0)
    let cases: [InterfaceCase; 3] = [
        ("no interface named at all", &[], true),
        (
            "one eth0 in the mapping subnet",
            &[&[("DeviceIndex", "0"), ("SubnetId", "")]],
            true,
        ),
        (
            "two interfaces in the mapping subnet",
            &[
                &[("DeviceIndex", "0"), ("SubnetId", "")],
                &[("DeviceIndex", "1"), ("SubnetId", "")],
            ],
            false,
        ),
    ];

    for (case, entries, expect_public_address) in cases {
        let mut params: Vec<(String, String)> = Vec::new();
        for (index, members) in entries.iter().enumerate() {
            for (name, value) in members.iter() {
                let value = if *name == "SubnetId" {
                    default_subnet.as_str()
                } else {
                    value
                };
                params.push(p(
                    &format!("NetworkInterface.{}.{name}", index + 1),
                    value,
                ));
            }
        }
        let body = ok(&router, case, "RunInstances", &launch(params)).await;
        let reservation = one_reservation(case, &body);
        let instance = &reservation.instances[0];
        assert_eq!(
            instance.interfaces,
            entries.len().max(1),
            "case {case:?}: expected {} interface(s), got {instance:?}",
            entries.len().max(1)
        );
        assert_eq!(
            instance.public_ip.as_deref().is_some_and(|ip| !ip.is_empty()),
            expect_public_address,
            "case {case:?}: expected a public address: {expect_public_address}, got \
             {instance:?}"
        );
    }
}
/// The `(defaultVersionNumber, latestVersionNumber, operator managed)` a
/// `DeleteLaunchTemplate` reports for the template it took.
fn deleted_template(case: &str, body: &str) -> (Option<String>, Option<String>, Option<String>) {
    let parsed = document(case, body);
    let template = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "launchTemplateId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: the delete must report the template: {body}"));
    (
        child_text(template, "defaultVersionNumber"),
        child_text(template, "latestVersionNumber"),
        child(template, "operator").and_then(|operator| child_text(operator, "managed")),
    )
}

/// A launch template reports the version numbers its versions have made it carry, and the
/// operator every template carries — on the create, on every reader, and on the delete that
/// takes it.
///
/// The rows differ in how many versions the template holds when it is deleted and in
/// whether its default version was moved: one version (both counters at the first version),
/// three versions (the latest follows the versions created, the default stays), and three
/// versions with the default moved to the last one.
#[tokio::test]
async fn a_launch_template_reports_its_version_counters_and_its_operator() {
    let router = emulator();

    // (case, versions created after the first, the version made default, expected counters)
    let cases: [(&str, usize, Option<&str>, &str, &str); 3] = [
        ("one version", 0, None, "1", "1"),
        ("three versions", 2, None, "1", "3"),
        ("three versions, the last made default", 2, Some("3"), "3", "3"),
    ];

    for (case, extra_versions, default_version, expected_default, expected_latest) in cases {
        let name = format!("members-{}", case.replace([' ', ','], "-"));
        let created = ok(
            &router,
            case,
            "CreateLaunchTemplate",
            &[p("LaunchTemplateName", &name), data("InstanceType", "t3.micro")],
        )
        .await;
        let launch_template_id = element(&created, "launchTemplateId")
            .unwrap_or_else(|| panic!("case {case:?}: the create must report its id: {created}"));
        let parsed = document(case, &created);
        let template = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "launchTemplateId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: the create must report the template: {created}"));
        assert_eq!(
            child(template, "operator").and_then(|operator| child_text(operator, "managed")),
            Some("false".to_string()),
            "case {case:?}: no service provider manages the caller's own template, got \
             {created}"
        );

        for round in 0..extra_versions {
            let body = ok(
                &router,
                case,
                "CreateLaunchTemplateVersion",
                &[
                    p("LaunchTemplateId", &launch_template_id),
                    data("InstanceType", "t3.small"),
                ],
            )
            .await;
            assert_eq!(
                element(&body, "versionNumber"),
                Some((round + 2).to_string()),
                "case {case:?}: versions are numbered in the order they are created, got \
                 {body}"
            );
        }
        if let Some(version) = default_version {
            let body = ok(
                &router,
                case,
                "ModifyLaunchTemplate",
                &[
                    p("LaunchTemplateId", &launch_template_id),
                    p("SetDefaultVersion", version),
                ],
            )
            .await;
            assert_eq!(
                element(&body, "defaultVersionNumber"),
                Some(version.to_string()),
                "case {case:?}: the modify reports the default version it was asked for, \
                 got {body}"
            );
        }

        // The reader and the delete report the SAME counters: the version numbers the
        // versions created made the template carry.
        let described = ok(
            &router,
            case,
            "DescribeLaunchTemplates",
            &indexed("LaunchTemplateId", &[launch_template_id.as_str()]),
        )
        .await;
        assert_eq!(
            (
                element(&described, "defaultVersionNumber"),
                element(&described, "latestVersionNumber"),
            ),
            (
                Some(expected_default.to_string()),
                Some(expected_latest.to_string())
            ),
            "case {case:?}: expected the counters ({expected_default}, {expected_latest}), \
             got {described}"
        );
        let deleted = ok(
            &router,
            case,
            "DeleteLaunchTemplate",
            &[p("LaunchTemplateId", &launch_template_id)],
        )
        .await;
        assert_eq!(
            deleted_template(case, &deleted),
            (
                Some(expected_default.to_string()),
                Some(expected_latest.to_string()),
                Some("false".to_string()),
            ),
            "case {case:?}: expected the delete to report the counters \
             ({expected_default}, {expected_latest}) and the operator, got {deleted}"
        );
    }
}

// ---------------------------------------------------------------------------
// Every tag a resource carries
// ---------------------------------------------------------------------------

/// `DescribeAddresses` reports every tag the address carries as its own `<item>` of the
/// `tagSet` list, not only the first.
///
/// The rows differ in how many tags the address carries (none, one, two — the recorded
/// case — and three) and in the tags' values (a plain value and an empty one), because the
/// rule is about a list of any length.
#[tokio::test]
async fn an_address_reports_every_tag_it_carries() {
    let router = emulator();

    let cases: [(&str, &[(&str, &str)]); 4] = [
        ("no tag", &[]),
        ("one tag", &[("Name", "solo")]),
        ("two tags", &[("Name", "pair"), ("Tier", "web")]),
        (
            "three tags, one with an empty value",
            &[("Name", "trio"), ("Tier", "db"), ("Blank", "")],
        ),
    ];

    for (case, tags) in cases {
        let allocated = ok(&router, case, "AllocateAddress", &[p("Domain", "vpc")]).await;
        let allocation_id = element(&allocated, "allocationId")
            .unwrap_or_else(|| panic!("case {case:?}: the allocate must report its id: {allocated}"));
        if !tags.is_empty() {
            let mut params = vec![p("ResourceId.1", &allocation_id)];
            for (index, (key, value)) in tags.iter().enumerate() {
                params.push(p(&format!("Tag.{}.Key", index + 1), key));
                params.push(p(&format!("Tag.{}.Value", index + 1), value));
            }
            let _ = ok(&router, case, "CreateTags", &params).await;
        }

        let described = ok(
            &router,
            case,
            "DescribeAddresses",
            &indexed("AllocationId", &[allocation_id.as_str()]),
        )
        .await;
        let parsed = document(case, &described);
        let address = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "allocationId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: the describe must report the address: {described}"));
        let mut reported: Vec<(String, String)> = set_items(address, "tagSet")
            .into_iter()
            .map(|item| {
                (
                    child_text(item, "key").unwrap_or_default(),
                    child_text(item, "value").unwrap_or_default(),
                )
            })
            .collect();
        reported.sort();
        let mut wanted: Vec<(String, String)> = tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected every tag as its own item of the list, got {described}"
        );
        // The list is reported whether or not it holds anything, so a client that reads it
        // finds the empty list rather than nothing at all.
        assert!(
            child(address, "tagSet").is_some(),
            "case {case:?}: the tagSet member is reported even when empty, got {described}"
        );
        // …and the tag keys are reachable the other way round, through the filter that
        // reads the same stored tags.
        if let Some((key, value)) = tags.first() {
            let filtered = ok(
                &router,
                case,
                "DescribeAddresses",
                &filters(&[(&format!("tag:{key}"), &[value])]),
            )
            .await;
            assert!(
                elements(&filtered, "allocationId").contains(&allocation_id),
                "case {case:?}: the tag filter must select the address it was written on, \
                 got {filtered}"
            );
        }
    }
}
