// SPDX-License-Identifier: CC0-1.0

//! What a launched instance reports about its NETWORK: the interfaces attached to it, the
//! subnet and VPC they put it in, and the filters that select on them — driven over the
//! wire the service speaks (`ec2Query`, an `application/x-www-form-urlencoded` POST
//! answered with XML; README.md / OPERATIONS.md "Wire protocol").
//!
//! Two rules are under test, both about `DescribeInstances` reporting the STORE:
//!
//! 1. EVERY INSTANCE IN A VPC HAS A PRIMARY NETWORK INTERFACE, which its launch creates in
//!    the subnet the instance runs in and attaches at device index 0
//!    (`Instance$NetworkInterfaces` / `InstanceNetworkInterfaceAttachment$DeviceIndex`),
//!    carrying the instance's primary private IPv4 address — and it is ONE record, so
//!    `DescribeNetworkInterfaces` reports the same interface, and the instance's own
//!    `SubnetId` / `VpcId` are its subnet and that subnet's VPC.
//! 2. A DOCUMENTED FILTER WHOSE VALUE THE INSTANCE HOLDS RESOLVES, so `vpc-id`,
//!    `subnet-id` and the `network-interface.*` names select the instances whose value
//!    matches and report an EMPTY SET rather than refusing a request.
//!
//! The rows span what those generalise over: whether the launch named a subnet, which VPC
//! the subnet is in (the account's default one and one the suite created), how many
//! instances one launch minted, and whether the launch named a security group. The
//! recorded case — one instance, read for its primary interface — is one row among them.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, filters, indexed};
use super::wire::{child, child_text, document, is_long_id, param, set_items, Param};

/// The Region every call here is signed for.
const REGION: &str = "us-east-1";

/// The image every launch names. `RunInstances` only has to accept the id; which image an
/// instance was launched from has no bearing on its network.
const AMI: &str = "ami-0123456789abcdef0";

/// The CIDR block of the VPC this suite creates, outside the `172.31.0.0/16` a default VPC
/// has, so a reported address says which of the two networks it came from.
const CREATED_VPC_CIDR: &str = "10.60.0.0/16";

/// The CIDR block of the subnet this suite creates inside that VPC.
const CREATED_SUBNET_CIDR: &str = "10.60.1.0/24";

/// A well-formed VPC id no VPC of the account has, for the filter row that must report an
/// empty set rather than refuse.
const UNKNOWN_VPC: &str = "vpc-00000000000000000";

/// One call that must succeed, as its body.
async fn ok(case: &str, router: &MultiServiceRouter, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One launched instance's network, as the assertions below read it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Network {
    instance_id: String,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    private_ip: Option<String>,
    interfaces: Vec<Interface>,
}

/// One entry of an instance's `networkInterfaceSet`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Interface {
    network_interface_id: Option<String>,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    private_ip: Option<String>,
    status: Option<String>,
    mac_address: Option<String>,
    group_ids: Vec<String>,
    attachment_id: Option<String>,
    device_index: Option<String>,
    attachment_status: Option<String>,
    delete_on_termination: Option<String>,
}

/// Every instance a `DescribeInstances` body reports, with its network.
fn networks(case: &str, body: &str) -> Vec<Network> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "instanceId").is_some())
        .filter(|node| node.tag_name().name() == "item")
        .map(|node| Network {
            instance_id: child_text(node, "instanceId").unwrap_or_default(),
            subnet_id: child_text(node, "subnetId"),
            vpc_id: child_text(node, "vpcId"),
            private_ip: child_text(node, "privateIpAddress"),
            interfaces: set_items(node, "networkInterfaceSet")
                .into_iter()
                .map(|interface| {
                    let attachment = child(interface, "attachment");
                    Interface {
                        network_interface_id: child_text(interface, "networkInterfaceId"),
                        subnet_id: child_text(interface, "subnetId"),
                        vpc_id: child_text(interface, "vpcId"),
                        private_ip: child_text(interface, "privateIpAddress"),
                        status: child_text(interface, "status"),
                        mac_address: child_text(interface, "macAddress"),
                        group_ids: set_items(interface, "groupSet")
                            .into_iter()
                            .filter_map(|group| child_text(group, "groupId"))
                            .collect(),
                        attachment_id: attachment
                            .and_then(|attachment| child_text(attachment, "attachmentId")),
                        device_index: attachment
                            .and_then(|attachment| child_text(attachment, "deviceIndex")),
                        attachment_status: attachment
                            .and_then(|attachment| child_text(attachment, "status")),
                        delete_on_termination: attachment.and_then(|attachment| {
                            child_text(attachment, "deleteOnTermination")
                        }),
                    }
                })
                .collect(),
        })
        .collect()
}

/// `DescribeInstances` narrowed to `instance_id`, as that instance's network.
async fn described(case: &str, router: &MultiServiceRouter, instance_id: &str) -> Network {
    let body = ok(
        case,
        router,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    let mut reported = networks(case, &body);
    assert_eq!(
        reported.len(),
        1,
        "case {case:?}: expected exactly one instance for {instance_id}, got {reported:?}"
    );
    reported.pop().expect("checked just above")
}

/// The instance ids a launch minted.
async fn launch(
    case: &str,
    router: &MultiServiceRouter,
    count: u32,
    extra: &[Param],
) -> Vec<String> {
    let mut params = vec![
        param("ImageId", AMI),
        param("MinCount", &count.to_string()),
        param("MaxCount", &count.to_string()),
    ];
    params.extend_from_slice(extra);
    let body = ok(case, router, "RunInstances", &params).await;
    let ids = elements(&body, "instanceId");
    assert_eq!(
        ids.len(),
        count as usize,
        "case {case:?}: expected {count} instance(s) from RunInstances, got {ids:?}"
    );
    ids
}

/// The account's default VPC id and its default subnet ids, in zone order.
async fn default_network(router: &MultiServiceRouter) -> (String, Vec<String>) {
    let vpcs = ok(
        "the default network",
        router,
        "DescribeVpcs",
        &filters(&[("is-default", &["true"])]),
    )
    .await;
    let vpc_id = element(&vpcs, "vpcId").unwrap_or_default();
    let subnets = ok(
        "the default network",
        router,
        "DescribeSubnets",
        &filters(&[("vpc-id", &[vpc_id.as_str()])]),
    )
    .await;
    (vpc_id.clone(), elements(&subnets, "subnetId"))
}

/// Whether `address` falls inside the `/16` or `/24` block `cidr_block` names — enough for
/// the two blocks this suite uses, and read as text so the assertion names both.
fn inside_block(cidr_block: &str, address: &str) -> bool {
    let (base, prefix_length) = cidr_block
        .split_once('/')
        .unwrap_or_else(|| panic!("{cidr_block} must be a CIDR block"));
    let labels = match prefix_length {
        "16" => 2,
        "24" => 3,
        other => panic!("this suite only compares /16 and /24 blocks, not /{other}"),
    };
    let head = |value: &str| {
        value
            .split('.')
            .take(labels)
            .collect::<Vec<&str>>()
            .join(".")
    };
    head(base) == head(address)
}

/// One launch row: what it names about its network, and where that puts the instance.
struct LaunchRow {
    case: &'static str,
    count: u32,
    extra: Vec<Param>,
    /// The subnet the instance must report.
    subnet_id: String,
    /// The block its private address must fall inside.
    cidr_block: &'static str,
    /// The security group the launch named, if it named one.
    group_id: Option<String>,
}

/// Rule 1: a launch gives its instance a primary network interface in the subnet it runs
/// in, the instance reports that subnet and its VPC, and the interface is the same record
/// `DescribeNetworkInterfaces` reports.
#[tokio::test]
async fn every_launched_instance_reports_its_primary_network_interface() {
    let (router, _engine) = super::ec2_query::emulator();
    let (default_vpc, default_subnets) = default_network(&router).await;
    let default_subnet = default_subnets
        .first()
        .cloned()
        .expect("a fresh account has a default subnet per zone");

    let created_vpc = {
        let body = ok(
            "the created VPC",
            &router,
            "CreateVpc",
            &[param("CidrBlock", CREATED_VPC_CIDR)],
        )
        .await;
        element(&body, "vpcId").unwrap_or_default()
    };
    let created_subnet = {
        let body = ok(
            "the created subnet",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &created_vpc),
                param("CidrBlock", CREATED_SUBNET_CIDR),
            ],
        )
        .await;
        element(&body, "subnetId").unwrap_or_default()
    };
    let created_group = {
        let body = ok(
            "the created group",
            &router,
            "CreateSecurityGroup",
            &[
                param("VpcId", &created_vpc),
                param("GroupName", "network-suite"),
                param("GroupDescription", "a group this suite launches with"),
            ],
        )
        .await;
        element(&body, "groupId").unwrap_or_default()
    };

    // Rows: what the launch named about its network, and where that puts the instance —
    // the default VPC when it named nothing, the named subnet's VPC when it did, one
    // instance or several from one launch, and a launch that named a security group.
    let rows = vec![
        LaunchRow {
            case: "a launch that named no subnet",
            count: 1,
            extra: Vec::new(),
            subnet_id: default_subnet.clone(),
            cidr_block: "172.31.0.0/16",
            group_id: None,
        },
        LaunchRow {
            case: "a launch that named a subnet of a VPC of its own",
            count: 1,
            extra: vec![param("SubnetId", &created_subnet)],
            subnet_id: created_subnet.clone(),
            cidr_block: CREATED_SUBNET_CIDR,
            group_id: None,
        },
        LaunchRow {
            case: "a launch of three instances into one subnet",
            count: 3,
            extra: vec![param("SubnetId", &created_subnet)],
            subnet_id: created_subnet.clone(),
            cidr_block: CREATED_SUBNET_CIDR,
            group_id: None,
        },
        LaunchRow {
            case: "a launch that named a security group",
            count: 1,
            extra: vec![
                param("SubnetId", &created_subnet),
                param("SecurityGroupId.1", &created_group),
            ],
            subnet_id: created_subnet.clone(),
            cidr_block: CREATED_SUBNET_CIDR,
            group_id: Some(created_group.clone()),
        },
    ];

    let mut interface_ids: Vec<String> = Vec::new();
    for row in rows {
        let LaunchRow {
            case,
            count,
            extra,
            subnet_id: expected_subnet,
            cidr_block: expected_block,
            group_id: expected_group,
        } = row;
        let expected_vpc = if expected_subnet == default_subnet {
            default_vpc.as_str()
        } else {
            created_vpc.as_str()
        };
        for instance_id in launch(case, &router, count, &extra).await {
            let network = described(case, &router, &instance_id).await;
            assert_eq!(
                network.subnet_id.as_deref(),
                Some(expected_subnet.as_str()),
                "case {case:?}: expected the instance in subnet {expected_subnet}, \
                 got {network:?}"
            );
            assert_eq!(
                network.vpc_id.as_deref(),
                Some(expected_vpc),
                "case {case:?}: expected the instance in VPC {expected_vpc}, got {network:?}"
            );
            let address = network.private_ip.clone().unwrap_or_default();
            assert!(
                inside_block(expected_block, &address),
                "case {case:?}: expected a private address inside {expected_block}, \
                 got {address:?}"
            );
            assert_eq!(
                network.interfaces.len(),
                1,
                "case {case:?}: a launch gives its instance one interface, got {network:?}"
            );
            let interface = &network.interfaces[0];
            assert_eq!(
                interface.device_index.as_deref(),
                Some("0"),
                "case {case:?}: the primary interface is attached at device index 0, \
                 got {interface:?}"
            );
            assert_eq!(
                interface.private_ip, network.private_ip,
                "case {case:?}: the primary interface holds the instance's own private \
                 address, got {interface:?}"
            );
            assert_eq!(
                interface.subnet_id, network.subnet_id,
                "case {case:?}: the interface is in the instance's subnet, got {interface:?}"
            );
            assert_eq!(
                interface.vpc_id, network.vpc_id,
                "case {case:?}: the interface is in the instance's VPC, got {interface:?}"
            );
            assert_eq!(
                interface.status.as_deref(),
                Some("in-use"),
                "case {case:?}: an attached interface is in use, got {interface:?}"
            );
            assert_eq!(
                interface.attachment_status.as_deref(),
                Some("attaching"),
                "case {case:?}: the launch's own attachment is still being made when the \
                 launch answers, got {interface:?}"
            );
            assert_eq!(
                interface.delete_on_termination.as_deref(),
                Some("true"),
                "case {case:?}: the primary interface goes with its instance, \
                 got {interface:?}"
            );
            let network_interface_id = interface.network_interface_id.clone().unwrap_or_default();
            assert!(
                is_long_id("eni", &network_interface_id),
                "case {case:?}: expected a long network interface id, \
                 got {network_interface_id:?}"
            );
            assert!(
                interface
                    .attachment_id
                    .as_deref()
                    .is_some_and(|id| id.starts_with("eni-attach-")),
                "case {case:?}: expected an eni-attach- attachment id, got {interface:?}"
            );
            assert!(
                interface
                    .mac_address
                    .as_deref()
                    .is_some_and(|mac| mac.matches(':').count() == 5),
                "case {case:?}: expected a MAC address on the interface, got {interface:?}"
            );
            if let Some(group_id) = expected_group.as_deref() {
                assert!(
                    interface.group_ids.iter().any(|held| held == group_id),
                    "case {case:?}: expected the launch's group {group_id} on the \
                     interface, got {interface:?}"
                );
            } else {
                assert!(
                    !interface.group_ids.is_empty(),
                    "case {case:?}: a launch that named no group gets its VPC's default \
                     group, got {interface:?}"
                );
            }
            assert!(
                !interface_ids.contains(&network_interface_id),
                "case {case:?}: {network_interface_id} was already reported for another \
                 instance"
            );
            interface_ids.push(network_interface_id.clone());

            // The same record through the interface's own describe: one interface, not two
            // copies of one. Read through the document, so the interface's own `<status>`
            // is never confused with its attachment's.
            let described_interface = ok(
                case,
                &router,
                "DescribeNetworkInterfaces",
                &indexed("NetworkInterfaceId", &[network_interface_id.as_str()]),
            )
            .await;
            let parsed = document(case, &described_interface);
            let item = parsed
                .descendants()
                .find(|node| {
                    node.is_element() && child(*node, "networkInterfaceId").is_some()
                })
                .unwrap_or_else(|| {
                    panic!("case {case:?}: no interface in {described_interface}")
                });
            for (name, expected) in [
                ("subnetId", interface.subnet_id.clone()),
                ("vpcId", interface.vpc_id.clone()),
                ("privateIpAddress", interface.private_ip.clone()),
                ("status", interface.status.clone()),
                ("macAddress", interface.mac_address.clone()),
            ] {
                assert_eq!(
                    child_text(item, name),
                    expected,
                    "case {case:?}: DescribeNetworkInterfaces must report the same \
                     {name} as the instance did, got {described_interface}"
                );
            }
            assert_eq!(
                element(&described_interface, "instanceId").as_deref(),
                Some(instance_id.as_str()),
                "case {case:?}: the interface's attachment must name the instance that \
                 holds it, got {described_interface}"
            );
        }
    }

    // A read changes nothing a later caller sees.
    let first = ok("the second read", &router, "DescribeInstances", &[]).await;
    let second = ok("the second read", &router, "DescribeInstances", &[]).await;
    assert_eq!(
        networks("the second read", &first),
        networks("the second read", &second),
        "a read must not change what a later caller sees"
    );
}

/// Rule 2: the documented filters whose value an instance holds select on it, and a value
/// nothing matches is an empty set rather than a refusal.
#[tokio::test]
async fn the_vpc_and_interface_filters_select_the_instances_that_match() {
    let (router, _engine) = super::ec2_query::emulator();
    let (default_vpc, _) = default_network(&router).await;

    let created_vpc = {
        let body = ok(
            "the created VPC",
            &router,
            "CreateVpc",
            &[param("CidrBlock", CREATED_VPC_CIDR)],
        )
        .await;
        element(&body, "vpcId").unwrap_or_default()
    };
    let created_subnet = {
        let body = ok(
            "the created subnet",
            &router,
            "CreateSubnet",
            &[
                param("VpcId", &created_vpc),
                param("CidrBlock", CREATED_SUBNET_CIDR),
            ],
        )
        .await;
        element(&body, "subnetId").unwrap_or_default()
    };

    let in_default = launch("the default VPC's instances", &router, 2, &[]).await;
    let in_created = launch(
        "the created VPC's instances",
        &router,
        1,
        &[param("SubnetId", &created_subnet)],
    )
    .await;
    let every = {
        let mut every = in_default.clone();
        every.extend(in_created.clone());
        every.sort();
        every
    };

    // Rows: one filter name per thing an instance's network holds, each with a value that
    // selects a known group of instances — including a well-formed value nothing matches.
    let rows: Vec<(&str, String, Vec<String>)> = vec![
        ("vpc-id", default_vpc.clone(), in_default.clone()),
        ("vpc-id", created_vpc.clone(), in_created.clone()),
        ("vpc-id", UNKNOWN_VPC.to_string(), Vec::new()),
        ("subnet-id", created_subnet.clone(), in_created.clone()),
        (
            "network-interface.vpc-id",
            created_vpc.clone(),
            in_created.clone(),
        ),
        (
            "network-interface.subnet-id",
            created_subnet.clone(),
            in_created.clone(),
        ),
        (
            "network-interface.attachment.device-index",
            "0".to_string(),
            every.clone(),
        ),
        (
            "network-interface.attachment.device-index",
            "1".to_string(),
            Vec::new(),
        ),
        (
            "network-interface.attachment.delete-on-termination",
            "true".to_string(),
            every.clone(),
        ),
        (
            "network-interface.status",
            "in-use".to_string(),
            every.clone(),
        ),
        (
            "network-interface.status",
            "available".to_string(),
            Vec::new(),
        ),
    ];

    for (name, value, expected) in rows {
        let case = format!("{name}={value}");
        let body = ok(
            &case,
            &router,
            "DescribeInstances",
            &filters(&[(name, &[value.as_str()])]),
        )
        .await;
        let mut reported: Vec<String> = networks(&case, &body)
            .into_iter()
            .map(|network| network.instance_id)
            .collect();
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {case:?}: expected the instances {wanted:?}, got {reported:?}"
        );
    }

    // A filter value nothing matches is an empty set; a filter NAME the operation does not
    // document is still refused, so an unknown name never reads as "nothing matched".
    let (status, body) = call(
        &router,
        REGION,
        "DescribeInstances",
        &filters(&[("vpc-id-of-something", &[created_vpc.as_str()])]),
    )
    .await;
    assert_eq!(
        status, 400,
        "an undocumented filter name must be refused, got {status} with body {body}"
    );
    assert_eq!(
        super::ec2_query::error_code(&body),
        ec2::state::INVALID_PARAMETER_VALUE,
        "an undocumented filter name must be refused with InvalidParameterValue, got {body}"
    );
}
