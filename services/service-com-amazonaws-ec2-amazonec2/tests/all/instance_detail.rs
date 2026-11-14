// SPDX-License-Identifier: CC0-1.0

//! The FIXED detail every instance reports, and the monitoring state a caller moves.
//!
//! `DescribeInstances` reports a large member set for every instance, not only what the
//! launch named: the machine facts of its AMI, the CPU layout of its instance type, the
//! volumes it holds, where it runs, the groups of its primary interface and the option
//! sets a launch may configure. The rule these suites assert is that EVERY one of those
//! members is read off the state this emulator holds — so each assertion compares the
//! instance report against what `DescribeImages`, `DescribeInstanceTypes`,
//! `DescribeVolumes`, `DescribeNetworkInterfaces` and `DescribeSubnets` report for the
//! same resource, rather than against a value written here.
//!
//! The rows of each table span the class the rule generalises over — the AMI (one of the
//! published catalog, one the account cannot see), the instance type (so a CPU layout
//! varies), the subnet (named, or the zone's default), the groups (named by id, named by
//! name, named not at all) and each option set a launch may ask for — because the rule is
//! about every instance and not about one recorded launch.

use std::time::Duration;

use service_com_amazonaws_ec2_amazonec2::state::{
    BOOT_MODE_UEFI_PREFERRED, HTTP_TOKENS_OPTIONAL, HTTP_TOKENS_REQUIRED,
    IMDS_V2_METADATA_HOP_LIMIT, METADATA_OPTIONS_STATE_APPLIED, MONITORING_STATE_DISABLED,
    MONITORING_STATE_DISABLING, MONITORING_STATE_ENABLED, MONITORING_STATE_PENDING,
    TRANSITION_DELAY,
};
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{self, element, elements, filters, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the right FORM that no image of this account has — the launch this
/// emulator still serves, whose instance therefore has no image facts to read.
const UNKNOWN_AMI: &str = "ami-0123456789abcdef0";

/// An instance id of the right FORM that no instance of this account has.
const UNKNOWN_INSTANCE: &str = "i-0123456789abcdef0";

/// Wait until every clock-owed move started before now has certainly fallen due.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_transition() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// One call that must succeed, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = ec2_query::call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// The id of the newest published Amazon Linux 2023 image of this account's catalog, and
/// the facts `DescribeImages` reports for it.
async fn catalog_image(router: &MultiServiceRouter) -> ImageFacts {
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
    let parsed = document("the published catalog", &body);
    let image = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "imageId").is_some())
        .unwrap_or_else(|| panic!("the catalog must publish an AL2023 image: {body}"));
    ImageFacts {
        image_id: child_text(image, "imageId").unwrap_or_default(),
        root_device_name: child_text(image, "rootDeviceName"),
        root_device_type: child_text(image, "rootDeviceType"),
        virtualization_type: child_text(image, "virtualizationType"),
        hypervisor: child_text(image, "hypervisor"),
        ena_support: child_text(image, "enaSupport"),
        platform_details: child_text(image, "platformDetails"),
        usage_operation: child_text(image, "usageOperation"),
        boot_mode: child_text(image, "bootMode"),
        imds_support: child_text(image, "imdsSupport"),
    }
}

/// What `DescribeImages` reports about one AMI — the facts an instance launched from it
/// has to report as its own.
#[derive(Debug, Clone)]
struct ImageFacts {
    image_id: String,
    root_device_name: Option<String>,
    root_device_type: Option<String>,
    virtualization_type: Option<String>,
    hypervisor: Option<String>,
    ena_support: Option<String>,
    platform_details: Option<String>,
    usage_operation: Option<String>,
    boot_mode: Option<String>,
    imds_support: Option<String>,
}

/// One instance as `DescribeInstances` reports it, read through the document so a nested
/// `state` or `status` is never taken for the instance's own.
#[derive(Debug, Clone, Default)]
struct InstanceReport {
    instance_id: String,
    state: Option<String>,
    root_device_name: Option<String>,
    root_device_type: Option<String>,
    virtualization_type: Option<String>,
    hypervisor: Option<String>,
    ena_support: Option<String>,
    platform_details: Option<String>,
    usage_operation: Option<String>,
    usage_operation_update_time: Option<String>,
    boot_mode: Option<String>,
    current_instance_boot_mode: Option<String>,
    source_dest_check: Option<String>,
    public_dns_name: Option<String>,
    /// `ipAddress`: the public IPv4 address the instance holds, if it holds one.
    public_ip: Option<String>,
    subnet_id: Option<String>,
    vpc_id: Option<String>,
    /// `placement`: the zone, its id, the group name and the tenancy.
    placement: Vec<(String, String)>,
    /// `cpuOptions`: the core count and the threads per core.
    cpu_options: Vec<(String, String)>,
    /// `metadataOptions`, every member of it.
    metadata_options: Vec<(String, String)>,
    monitoring_state: Option<String>,
    enclave_enabled: Option<String>,
    hibernation_configured: Option<String>,
    capacity_reservation_preference: Option<String>,
    auto_recovery: Option<String>,
    reboot_migration: Option<String>,
    bandwidth_weighting: Option<String>,
    private_dns_name_options: Vec<(String, String)>,
    operator_managed: Option<String>,
    /// `blockDeviceMapping`: `(device name, volume id, status)` per entry.
    block_devices: Vec<(String, String, String)>,
    /// `groupSet`: `(group id, group name)` per entry.
    groups: Vec<(String, String)>,
    /// `networkInterfaceSet`: `(interface id, attachment status, device index)` per entry.
    interfaces: Vec<(String, String, String)>,
    /// Whether the `secondaryInterfaceSet` member was reported at all.
    has_secondary_interface_set: bool,
}

/// The members of `node`'s `name` child, as `(member, text)` pairs — one structure member
/// of the instance, read whole so a missing member is visible as a missing pair.
fn members(node: roxmltree::Node<'_, '_>, name: &str) -> Vec<(String, String)> {
    child(node, name)
        .map(|held| {
            held.children()
                .filter(roxmltree::Node::is_element)
                .map(|member| {
                    (
                        member.tag_name().name().to_string(),
                        member.text().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The value of `member` among `pairs`.
fn pick(pairs: &[(String, String)], member: &str) -> Option<String> {
    pairs
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, value)| value.clone())
}

/// Every instance one `DescribeInstances` body reports.
fn reported_instances(case: &str, body: &str) -> Vec<InstanceReport> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| {
            node.is_element()
                && node.tag_name().name() == "item"
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .map(|node| InstanceReport {
            instance_id: child_text(node, "instanceId").unwrap_or_default(),
            state: child(node, "instanceState").and_then(|state| child_text(state, "name")),
            root_device_name: child_text(node, "rootDeviceName"),
            root_device_type: child_text(node, "rootDeviceType"),
            virtualization_type: child_text(node, "virtualizationType"),
            hypervisor: child_text(node, "hypervisor"),
            ena_support: child_text(node, "enaSupport"),
            platform_details: child_text(node, "platformDetails"),
            usage_operation: child_text(node, "usageOperation"),
            usage_operation_update_time: child_text(node, "usageOperationUpdateTime"),
            boot_mode: child_text(node, "bootMode"),
            current_instance_boot_mode: child_text(node, "currentInstanceBootMode"),
            source_dest_check: child_text(node, "sourceDestCheck"),
            public_dns_name: child_text(node, "dnsName"),
            public_ip: child_text(node, "ipAddress"),
            subnet_id: child_text(node, "subnetId"),
            vpc_id: child_text(node, "vpcId"),
            placement: members(node, "placement"),
            cpu_options: members(node, "cpuOptions"),
            metadata_options: members(node, "metadataOptions"),
            monitoring_state: child(node, "monitoring").and_then(|held| child_text(held, "state")),
            enclave_enabled: child(node, "enclaveOptions")
                .and_then(|held| child_text(held, "enabled")),
            hibernation_configured: child(node, "hibernationOptions")
                .and_then(|held| child_text(held, "configured")),
            capacity_reservation_preference: child(node, "capacityReservationSpecification")
                .and_then(|held| child_text(held, "capacityReservationPreference")),
            auto_recovery: child(node, "maintenanceOptions")
                .and_then(|held| child_text(held, "autoRecovery")),
            reboot_migration: child(node, "maintenanceOptions")
                .and_then(|held| child_text(held, "rebootMigration")),
            bandwidth_weighting: child(node, "networkPerformanceOptions")
                .and_then(|held| child_text(held, "bandwidthWeighting")),
            private_dns_name_options: members(node, "privateDnsNameOptions"),
            operator_managed: child(node, "operator").and_then(|held| child_text(held, "managed")),
            block_devices: set_items(node, "blockDeviceMapping")
                .into_iter()
                .map(|item| {
                    let ebs = child(item, "ebs");
                    (
                        child_text(item, "deviceName").unwrap_or_default(),
                        ebs.and_then(|ebs| child_text(ebs, "volumeId"))
                            .unwrap_or_default(),
                        ebs.and_then(|ebs| child_text(ebs, "status"))
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            groups: set_items(node, "groupSet")
                .into_iter()
                .map(|item| {
                    (
                        child_text(item, "groupId").unwrap_or_default(),
                        child_text(item, "groupName").unwrap_or_default(),
                    )
                })
                .collect(),
            interfaces: set_items(node, "networkInterfaceSet")
                .into_iter()
                .map(|item| {
                    let attachment = child(item, "attachment");
                    (
                        child_text(item, "networkInterfaceId").unwrap_or_default(),
                        attachment
                            .and_then(|held| child_text(held, "status"))
                            .unwrap_or_default(),
                        attachment
                            .and_then(|held| child_text(held, "deviceIndex"))
                            .unwrap_or_default(),
                    )
                })
                .collect(),
            has_secondary_interface_set: child(node, "secondaryInterfaceSet").is_some(),
        })
        .collect()
}

/// The one instance `DescribeInstances` reports for `instance_id`.
async fn describe_one(router: &MultiServiceRouter, case: &str, instance_id: &str) -> InstanceReport {
    let body = ok(
        router,
        case,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    let mut instances = reported_instances(case, &body);
    assert_eq!(
        instances.len(),
        1,
        "case {case:?}: expected exactly one instance for {instance_id}, got {instances:?}"
    );
    instances.pop().expect("checked just above")
}

/// Launch one instance with `params` beside the image id, and answer its id.
async fn launch_one(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
    params: &[(String, String)],
) -> String {
    let mut fields = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    fields.extend_from_slice(params);
    let body = ok(router, case, "RunInstances", &fields).await;
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report the instance: {body}"))
}

/// One row of the launch table: how the launch was made.
struct LaunchRow {
    case: &'static str,
    /// Launch from the published catalog image rather than from an AMI nothing holds.
    from_catalog: bool,
    instance_type: &'static str,
    extra: Vec<(String, String)>,
}

/// Rule: every instance reports the fixed member set, each member read off the state this
/// emulator holds — the AMI's facts, the instance type's CPU layout, the volume store, the
/// interface store and the subnet.
#[tokio::test]
async fn describe_instances_reports_the_fixed_detail_derived_from_state() {
    let (router, _engine) = ec2_query::emulator();
    let catalog = catalog_image(&router).await;
    // One placement group of the account, so a launch has a real group to be placed in and
    // the instance's `placement.groupName`/`groupId` are read off a stored record.
    const PLACEMENT_GROUP: &str = "instance-detail-group";
    let created_group = ok(
        &router,
        "the placement group",
        "CreatePlacementGroup",
        &[p("GroupName", PLACEMENT_GROUP), p("Strategy", "cluster")],
    )
    .await;
    let placement_group_id = element(&created_group, "groupId").unwrap_or_else(|| {
        panic!("CreatePlacementGroup must report the group it minted: {created_group}")
    });

    let rows = vec![
        LaunchRow {
            case: "the published catalog AMI, default instance type",
            from_catalog: true,
            instance_type: "t3.micro",
            extra: Vec::new(),
        },
        LaunchRow {
            case: "the published catalog AMI, a larger instance type",
            from_catalog: true,
            instance_type: "m5.4xlarge",
            extra: Vec::new(),
        },
        LaunchRow {
            case: "an AMI this account cannot see",
            from_catalog: false,
            instance_type: "t3.small",
            extra: Vec::new(),
        },
        LaunchRow {
            case: "a single-threaded instance type",
            from_catalog: true,
            instance_type: "t4g.nano",
            extra: Vec::new(),
        },
        LaunchRow {
            case: "a launch placed in a placement group",
            from_catalog: true,
            instance_type: "c5.large",
            extra: vec![p("Placement.GroupName", PLACEMENT_GROUP)],
        },
        LaunchRow {
            case: "a launch that asks for the option sets",
            from_catalog: true,
            instance_type: "c5.large",
            extra: vec![
                p("EnclaveOptions.Enabled", "true"),
                p("HibernationOptions.Configured", "true"),
                // Hibernation requires an encrypted root device volume, so the launch that
                // asks for it overrides the AMI's root mapping to encrypt it
                // (`UnsupportedHibernationConfiguration` is the refusal otherwise).
                p(
                    "BlockDeviceMapping.1.DeviceName",
                    catalog.root_device_name.clone().unwrap_or_default().as_str(),
                ),
                p("BlockDeviceMapping.1.Ebs.Encrypted", "true"),
                p("Placement.Tenancy", "dedicated"),
                p("MaintenanceOptions.AutoRecovery", "disabled"),
                p("NetworkPerformanceOptions.BandwidthWeighting", "vpc-1"),
                p(
                    "CapacityReservationSpecification.CapacityReservationPreference",
                    "none",
                ),
                p("PrivateDnsNameOptions.HostnameType", "resource-name"),
                p("PrivateDnsNameOptions.EnableResourceNameDnsARecord", "true"),
            ],
        },
    ];

    for row in &rows {
        let case = row.case;
        let image_id = if row.from_catalog {
            catalog.image_id.clone()
        } else {
            UNKNOWN_AMI.to_string()
        };
        let mut params = vec![p("InstanceType", row.instance_type)];
        params.extend_from_slice(&row.extra);
        let instance_id = launch_one(&router, case, &image_id, &params).await;
        past_the_transition().await;
        let reported = describe_one(&router, case, &instance_id).await;

        // The machine facts of the AMI, compared against what `DescribeImages` reports
        // for that same AMI — never against a value written here.
        if row.from_catalog {
            for (member, expected, actual) in [
                (
                    "rootDeviceName",
                    catalog.root_device_name.clone(),
                    reported.root_device_name.clone(),
                ),
                (
                    "rootDeviceType",
                    catalog.root_device_type.clone(),
                    reported.root_device_type.clone(),
                ),
                (
                    "virtualizationType",
                    catalog.virtualization_type.clone(),
                    reported.virtualization_type.clone(),
                ),
                (
                    "hypervisor",
                    catalog.hypervisor.clone(),
                    reported.hypervisor.clone(),
                ),
                (
                    "enaSupport",
                    catalog.ena_support.clone(),
                    reported.ena_support.clone(),
                ),
                (
                    "platformDetails",
                    catalog.platform_details.clone(),
                    reported.platform_details.clone(),
                ),
                (
                    "usageOperation",
                    catalog.usage_operation.clone(),
                    reported.usage_operation.clone(),
                ),
                (
                    "bootMode",
                    catalog.boot_mode.clone(),
                    reported.boot_mode.clone(),
                ),
            ] {
                assert_eq!(
                    actual, expected,
                    "case {case:?}: the instance's {member} must be the AMI's own, \
                     expected {expected:?}, got {actual:?}"
                );
            }
            // "`uefi-preferred` … the AMI supports both UEFI and Legacy BIOS", so which mode
            // such an instance boots is the INSTANCE TYPE's own question — compared against
            // the boot modes `DescribeInstanceTypes` publishes for that type rather than
            // against a value written here.
            let published = ok(
                &router,
                case,
                "DescribeInstanceTypes",
                &indexed("InstanceType", &[row.instance_type]),
            )
            .await;
            let published = document(case, &published);
            let supported: Vec<String> = published
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == "supportedBootModes")
                .map(|set| {
                    set.children()
                        .filter(roxmltree::Node::is_element)
                        .map(|item| item.text().unwrap_or_default().to_string())
                        .collect()
                })
                .unwrap_or_default();
            let expected_boot_mode = match catalog.boot_mode.as_deref() {
                Some(BOOT_MODE_UEFI_PREFERRED) if !supported.iter().any(|mode| mode == "uefi") => {
                    "legacy-bios"
                }
                _ => "uefi",
            };
            assert_eq!(
                reported.current_instance_boot_mode.as_deref(),
                Some(expected_boot_mode),
                "case {case:?}: expected currentInstanceBootMode {expected_boot_mode:?} for an \
                 AMI of boot mode {:?} on a type supporting {supported:?}, got {:?}",
                catalog.boot_mode,
                reported.current_instance_boot_mode
            );
            assert!(
                reported.usage_operation_update_time.is_some(),
                "case {case:?}: an instance of an AMI with a creation date reports when its \
                 usage operation was last updated, got {reported:?}"
            );
        } else {
            // An AMI the account cannot see publishes nothing to read, and the instance is
            // EBS-backed all the same: it reports the facts of the default Linux image this
            // crate publishes rather than nothing at all.
            assert_eq!(
                reported.root_device_type.as_deref(),
                Some("ebs"),
                "case {case:?}: an EBS-backed instance reports an ebs root device, got \
                 {reported:?}"
            );
            assert!(
                reported.root_device_name.is_some()
                    && reported.virtualization_type.is_some()
                    && reported.hypervisor.is_some()
                    && reported.platform_details.is_some()
                    && reported.usage_operation.is_some(),
                "case {case:?}: the machine facts are reported for every instance, got \
                 {reported:?}"
            );
        }

        // The CPU layout of the instance's own type, compared against what
        // `DescribeInstanceTypes` reports for that type.
        let types = ok(
            &router,
            case,
            "DescribeInstanceTypes",
            &indexed("InstanceType", &[row.instance_type]),
        )
        .await;
        let expected_cores = element(&types, "defaultCores");
        let expected_threads = element(&types, "defaultThreadsPerCore");
        assert_eq!(
            pick(&reported.cpu_options, "coreCount"),
            expected_cores,
            "case {case:?}: cpuOptions.coreCount must be the instance type's own, got \
             {:?}",
            reported.cpu_options
        );
        assert_eq!(
            pick(&reported.cpu_options, "threadsPerCore"),
            expected_threads,
            "case {case:?}: cpuOptions.threadsPerCore must be the instance type's own, got \
             {:?}",
            reported.cpu_options
        );

        // The volumes the instance holds, compared against the volume store: every block
        // device mapping entry names a volume `DescribeVolumes` reports attached to THIS
        // instance at THAT device.
        let volumes = ok(
            &router,
            case,
            "DescribeVolumes",
            &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        let volume_ids = elements(&volumes, "volumeId");
        assert!(
            !reported.block_devices.is_empty(),
            "case {case:?}: an EBS-backed instance reports its root block device, got \
             {reported:?}"
        );
        for (device_name, volume_id, status) in &reported.block_devices {
            assert!(
                volume_ids.contains(volume_id),
                "case {case:?}: the mapping at {device_name} names {volume_id}, which \
                 DescribeVolumes does not report attached to the instance: {volume_ids:?}"
            );
            assert_eq!(
                status, "attached",
                "case {case:?}: the attachment at {device_name} has had its span, expected \
                 attached, got {status:?}"
            );
            assert!(
                !device_name.is_empty(),
                "case {case:?}: every mapping names its device, got {reported:?}"
            );
        }

        // The interfaces the instance holds, compared against the interface store.
        let interfaces = ok(
            &router,
            case,
            "DescribeNetworkInterfaces",
            &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        let interface_ids = elements(&interfaces, "networkInterfaceId");
        assert_eq!(
            reported.interfaces.len(),
            1,
            "case {case:?}: a launch gives the instance its primary interface, got {:?}",
            reported.interfaces
        );
        for (interface_id, status, device_index) in &reported.interfaces {
            assert!(
                interface_ids.contains(interface_id),
                "case {case:?}: the instance reports {interface_id}, which \
                 DescribeNetworkInterfaces does not report attached to it: {interface_ids:?}"
            );
            assert_eq!(
                status, "attached",
                "case {case:?}: the primary interface is attached, got {status:?}"
            );
            assert_eq!(
                device_index, "0",
                "case {case:?}: the primary interface sits at device index 0, got \
                 {device_index:?}"
            );
        }

        // Where the instance runs: its zone, that zone's id, the tenancy the launch asked
        // for and the empty group name of an instance in no placement group.
        let zones = ok(
            &router,
            case,
            "DescribeAvailabilityZones",
            &indexed(
                "ZoneName",
                &[pick(&reported.placement, "availabilityZone")
                    .unwrap_or_default()
                    .as_str()],
            ),
        )
        .await;
        assert_eq!(
            pick(&reported.placement, "availabilityZoneId"),
            element(&zones, "zoneId"),
            "case {case:?}: placement.availabilityZoneId must be the zone's own id, got \
             {:?}",
            reported.placement
        );
        assert_eq!(
            pick(&reported.placement, "groupName"),
            Some(
                row.extra
                    .iter()
                    .find(|(field, _)| field == "Placement.GroupName")
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default()
            ),
            "case {case:?}: placement.groupName is the group the launch named, and empty for \
             an instance in none, got {:?}",
            reported.placement
        );
        assert_eq!(
            pick(&reported.placement, "groupId"),
            row.extra
                .iter()
                .find(|(field, _)| field == "Placement.GroupName")
                .map(|_| placement_group_id.clone()),
            "case {case:?}: placement.groupId is the id of that same stored group, got {:?}",
            reported.placement
        );

        // The groups the instance reports are the groups of its primary interface, both
        // members of each one.
        let interface_groups: Vec<(String, String)> = {
            let parsed = document(case, &interfaces);
            parsed
                .descendants()
                .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
                .map(|node| {
                    set_items(node, "groupSet")
                        .into_iter()
                        .map(|item| {
                            (
                                child_text(item, "groupId").unwrap_or_default(),
                                child_text(item, "groupName").unwrap_or_default(),
                            )
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        assert_eq!(
            reported.groups, interface_groups,
            "case {case:?}: the instance's groupSet must be its primary interface's groups, \
             got {:?} against {interface_groups:?}",
            reported.groups
        );
        assert!(
            reported
                .groups
                .iter()
                .all(|(group_id, group_name)| !group_id.is_empty() && !group_name.is_empty()),
            "case {case:?}: every reported group names BOTH its id and its name, got {:?}",
            reported.groups
        );

        // The option sets: what the launch asked for, or the documented default.
        let asked = |name: &str| -> Option<String> {
            row.extra
                .iter()
                .find(|(field, _)| field == name)
                .map(|(_, value)| value.clone())
        };
        for (member, actual, expected) in [
            (
                "enclaveOptions.enabled",
                reported.enclave_enabled.clone(),
                asked("EnclaveOptions.Enabled").unwrap_or_else(|| "false".to_string()),
            ),
            (
                "hibernationOptions.configured",
                reported.hibernation_configured.clone(),
                asked("HibernationOptions.Configured").unwrap_or_else(|| "false".to_string()),
            ),
            (
                "placement.tenancy",
                pick(&reported.placement, "tenancy"),
                asked("Placement.Tenancy").unwrap_or_else(|| "default".to_string()),
            ),
            (
                "maintenanceOptions.autoRecovery",
                reported.auto_recovery.clone(),
                asked("MaintenanceOptions.AutoRecovery").unwrap_or_else(|| "default".to_string()),
            ),
            (
                "networkPerformanceOptions.bandwidthWeighting",
                reported.bandwidth_weighting.clone(),
                asked("NetworkPerformanceOptions.BandwidthWeighting")
                    .unwrap_or_else(|| "default".to_string()),
            ),
            (
                "capacityReservationSpecification.capacityReservationPreference",
                reported.capacity_reservation_preference.clone(),
                asked("CapacityReservationSpecification.CapacityReservationPreference")
                    .unwrap_or_else(|| "open".to_string()),
            ),
            (
                "privateDnsNameOptions.hostnameType",
                pick(&reported.private_dns_name_options, "hostnameType"),
                asked("PrivateDnsNameOptions.HostnameType").unwrap_or_else(|| "ip-name".to_string()),
            ),
            (
                "privateDnsNameOptions.enableResourceNameDnsARecord",
                pick(
                    &reported.private_dns_name_options,
                    "enableResourceNameDnsARecord",
                ),
                asked("PrivateDnsNameOptions.EnableResourceNameDnsARecord")
                    .unwrap_or_else(|| "false".to_string()),
            ),
        ] {
            assert_eq!(
                actual.as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected {member} {expected:?}, got {actual:?}"
            );
        }
        assert_eq!(
            reported.reboot_migration.as_deref(),
            Some("default"),
            "case {case:?}: an instance nothing disabled reboot migration for reports the \
             default behaviour, got {:?}",
            reported.reboot_migration
        );
        assert_eq!(
            reported.operator_managed.as_deref(),
            Some("false"),
            "case {case:?}: no service provider manages the caller's own instance, got {:?}",
            reported.operator_managed
        );
        assert_eq!(
            reported.source_dest_check.as_deref(),
            Some("true"),
            "case {case:?}: source/destination checking starts enabled, got {:?}",
            reported.source_dest_check
        );
        // "The public DNS name assigned to the instance" (`Instance$PublicDnsName`)
        // follows the public IPv4 address it holds: the Amazon public hostname of that
        // address, and the EMPTY name for an instance holding none. The address itself is
        // the subnet's doing — "indicates whether instances launched in this subnet receive
        // a public IPv4 address" (`Subnet$MapPublicIpOnLaunch`) — so this row asserts the
        // two members agree rather than which of them the launch got.
        let expected_public_dns_name = match reported.public_ip.as_deref() {
            Some(public_ip) if !public_ip.is_empty() => format!(
                "ec2-{}.{REGION}.compute.amazonaws.com",
                public_ip.replace('.', "-")
            ),
            _ => String::new(),
        };
        assert_eq!(
            reported.public_dns_name.as_deref(),
            Some(expected_public_dns_name.as_str()),
            "case {case:?}: the public DNS name must name the public address the instance \
             holds ({:?}), got {:?}",
            reported.public_ip,
            reported.public_dns_name
        );
        assert!(
            reported.has_secondary_interface_set,
            "case {case:?}: the secondary interface set is reported, empty, for an instance \
             that has none: {reported:?}"
        );
        // The subnet the instance runs in, and that subnet's VPC — both read off the
        // subnet store.
        let subnet_id = reported.subnet_id.clone().unwrap_or_else(|| {
            panic!("case {case:?}: an instance in a VPC reports its subnet: {reported:?}")
        });
        let subnets = ok(
            &router,
            case,
            "DescribeSubnets",
            &indexed("SubnetId", &[subnet_id.as_str()]),
        )
        .await;
        assert_eq!(
            reported.vpc_id,
            element(&subnets, "vpcId"),
            "case {case:?}: the instance's VPC must be its subnet's, got {reported:?}"
        );
    }

    // A placement group the account does not hold is refused rather than accepted and
    // dropped: an instance may not report a group nothing holds.
    let (status, body) = ec2_query::call(
        &router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", UNKNOWN_AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("Placement.GroupName", "no-such-placement-group"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a launch naming a placement group the account does not hold is refused, got \
         {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "InvalidPlacementGroup.Unknown",
        "an unknown placement group is refused as unknown: {body}"
    );
}

/// One row of the monitoring table: how many instances the call names, and how.
struct MonitoringRow {
    case: &'static str,
    /// How many instances the batch names.
    instances: usize,
    /// Name every instance twice, so a repeat is the same request and not a second move.
    repeat: bool,
    /// Launch with detailed monitoring already on.
    monitoring_at_launch: bool,
}

/// Rule: detailed monitoring moves through a transient state — `pending` on its way to
/// `enabled`, `disabling` on its way to `disabled` — and `DescribeInstances` reports the
/// state the clock has reached, whatever the batch and whether or not the launch turned
/// monitoring on.
#[tokio::test]
async fn monitor_and_unmonitor_instances_move_monitoring_through_its_transient_state() {
    let (router, _engine) = ec2_query::emulator();

    let rows = vec![
        MonitoringRow {
            case: "one instance",
            instances: 1,
            repeat: false,
            monitoring_at_launch: false,
        },
        MonitoringRow {
            case: "three instances in one call",
            instances: 3,
            repeat: false,
            monitoring_at_launch: false,
        },
        MonitoringRow {
            case: "one instance named twice",
            instances: 1,
            repeat: true,
            monitoring_at_launch: false,
        },
        MonitoringRow {
            case: "an instance launched with monitoring already on",
            instances: 1,
            repeat: false,
            monitoring_at_launch: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let mut instance_ids = Vec::new();
        for index in 0..row.instances {
            let extra = if row.monitoring_at_launch {
                vec![p("Monitoring.Enabled", "true")]
            } else {
                Vec::new()
            };
            let instance_id = launch_one(&router, case, UNKNOWN_AMI, &extra).await;
            assert_eq!(
                describe_one(&router, case, &instance_id)
                    .await
                    .monitoring_state
                    .as_deref(),
                Some(if row.monitoring_at_launch {
                    MONITORING_STATE_ENABLED
                } else {
                    MONITORING_STATE_DISABLED
                }),
                "case {case:?}: instance {index} must report the monitoring its launch asked \
                 for"
            );
            instance_ids.push(instance_id);
        }
        let mut named: Vec<&str> = instance_ids.iter().map(String::as_str).collect();
        if row.repeat {
            named.extend(instance_ids.iter().map(String::as_str));
        }

        // Turning it ON: the answer carries the transient state, and so does the describe
        // that follows it at once.
        let body = ok(
            &router,
            case,
            "MonitorInstances",
            &indexed("InstanceId", &named),
        )
        .await;
        let answered = elements(&body, "state");
        // An instance already monitored with nothing owed is left exactly as it is, so the
        // call reports the state it is in rather than re-entering the transient one.
        let expected_transient = if row.monitoring_at_launch {
            MONITORING_STATE_ENABLED
        } else {
            MONITORING_STATE_PENDING
        };
        assert_eq!(
            answered,
            vec![expected_transient.to_string(); instance_ids.len()],
            "case {case:?}: MonitorInstances answers one {expected_transient} entry \
             per instance it acted on, got {answered:?} in {body}"
        );
        let monitored = elements(&body, "instanceId");
        assert_eq!(
            monitored, instance_ids,
            "case {case:?}: one entry per instance however often the batch names it, got \
             {monitored:?}"
        );
        for instance_id in &instance_ids {
            assert_eq!(
                describe_one(&router, case, instance_id)
                    .await
                    .monitoring_state
                    .as_deref(),
                Some(expected_transient),
                "case {case:?}: {instance_id} reports the transient state at once"
            );
        }

        // And at its due instant the clock has it enabled, for a caller that only reads.
        past_the_transition().await;
        for instance_id in &instance_ids {
            assert_eq!(
                describe_one(&router, case, instance_id)
                    .await
                    .monitoring_state
                    .as_deref(),
                Some(MONITORING_STATE_ENABLED),
                "case {case:?}: {instance_id} reaches {MONITORING_STATE_ENABLED}"
            );
        }
        // The filter reads the same state the report carries.
        let filtered = ok(
            &router,
            case,
            "DescribeInstances",
            &filters(&[("monitoring-state", &[MONITORING_STATE_ENABLED])]),
        )
        .await;
        let selected = elements(&filtered, "instanceId");
        for instance_id in &instance_ids {
            assert!(
                selected.contains(instance_id),
                "case {case:?}: the monitoring-state filter must select {instance_id}, got \
                 {selected:?}"
            );
        }

        // Turning it OFF: the other direction of the same move.
        let body = ok(
            &router,
            case,
            "UnmonitorInstances",
            &indexed("InstanceId", &named),
        )
        .await;
        let answered = elements(&body, "state");
        assert_eq!(
            answered,
            vec![MONITORING_STATE_DISABLING.to_string(); instance_ids.len()],
            "case {case:?}: UnmonitorInstances answers one {MONITORING_STATE_DISABLING} \
             entry per instance, got {answered:?} in {body}"
        );
        past_the_transition().await;
        for instance_id in &instance_ids {
            assert_eq!(
                describe_one(&router, case, instance_id)
                    .await
                    .monitoring_state
                    .as_deref(),
                Some(MONITORING_STATE_DISABLED),
                "case {case:?}: {instance_id} reaches {MONITORING_STATE_DISABLED}"
            );
        }
    }
}

/// One row of the refusal table: the call, the request, and the refusal it must answer.
struct RefusalRow {
    case: &'static str,
    action: &'static str,
    params: Vec<(String, String)>,
    status: u16,
    code: &'static str,
}

/// Rule: a monitoring call acts on every instance it names or on none of them — an id it
/// cannot resolve, an instance in a state with nothing running to monitor, an empty list
/// and a dry run each leave the monitoring state exactly as it was.
#[tokio::test]
async fn monitoring_refuses_what_it_cannot_act_on_and_changes_nothing() {
    let (router, _engine) = ec2_query::emulator();
    let instance_id = launch_one(&router, "the held instance", UNKNOWN_AMI, &[]).await;
    past_the_transition().await;

    let terminated = launch_one(&router, "the terminated instance", UNKNOWN_AMI, &[]).await;
    past_the_transition().await;
    ok(
        &router,
        "terminate",
        "TerminateInstances",
        &[p("InstanceId.1", &terminated)],
    )
    .await;
    past_the_transition().await;

    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "an id of no instance",
            action: "MonitorInstances",
            params: vec![p("InstanceId.1", UNKNOWN_INSTANCE)],
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        RefusalRow {
            // The monitoring pair validate their `InstanceIds` list as a request
            // PARAMETER, so a value outside the instance-id form is refused for the
            // parameter rather than as an unreadable instance id. The wording and the
            // forms that reach it are a table of their own in
            // `tests/all/monitor_instances.rs`.
            case: "an id that is not an instance id at all",
            action: "MonitorInstances",
            params: vec![p("InstanceId.1", "not-an-instance")],
            status: 400,
            code: "InvalidParameterValue",
        },
        RefusalRow {
            // `InstanceIds` is `@required`, so the boundary validator refuses a request
            // that omits the list before the handler runs.
            case: "no instance at all",
            action: "MonitorInstances",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
        RefusalRow {
            case: "a terminated instance",
            action: "MonitorInstances",
            params: vec![p("InstanceId.1", &terminated)],
            status: 400,
            code: "IncorrectInstanceState",
        },
        RefusalRow {
            case: "one resolvable instance beside one that is not",
            action: "MonitorInstances",
            params: vec![
                p("InstanceId.1", &instance_id),
                p("InstanceId.2", UNKNOWN_INSTANCE),
            ],
            status: 400,
            code: "InvalidInstanceID.NotFound",
        },
        RefusalRow {
            case: "a request that would have succeeded, under DryRun",
            action: "MonitorInstances",
            params: vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
        },
        RefusalRow {
            case: "the same, turning monitoring off",
            action: "UnmonitorInstances",
            params: vec![p("InstanceId.1", &instance_id), p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
        },
        RefusalRow {
            case: "no instance at all, turning monitoring off",
            action: "UnmonitorInstances",
            params: Vec::new(),
            status: 400,
            code: "MalformedInput",
        },
    ];

    for row in &rows {
        let RefusalRow {
            case,
            action,
            params,
            status: expected_status,
            code: expected_code,
        } = row;
        let (status, body) = ec2_query::call(&router, REGION, action, params).await;
        assert_eq!(
            status, *expected_status,
            "case {case:?}: expected HTTP {expected_status} from {action}, got {status} \
             with body {body}"
        );
        let code = ec2_query::error_code(&body);
        assert_eq!(
            code, *expected_code,
            "case {case:?}: expected the error code {expected_code}, got {code:?} with \
             body {body}"
        );
        assert_eq!(
            describe_one(&router, case, &instance_id)
                .await
                .monitoring_state
                .as_deref(),
            Some(MONITORING_STATE_DISABLED),
            "case {case:?}: a refused call leaves the monitoring state as it was"
        );
    }
}

/// Rule: a `DescribeInstances` narrowed by a tag reports exactly the instances whose tags
/// match — across pages, each instance reporting the state the clock has reached.
#[tokio::test]
async fn a_tag_filtered_describe_reports_the_matching_instances_across_pages() {
    let (router, _engine) = ec2_query::emulator();

    // Twelve instances, tagged two ways: the rule is about which instances a tag selects, so
    // both the selected and the unselected set carry more than one member — and the selected
    // set is larger than the smallest page this operation pages with, so the paged row below
    // really spans pages.
    let mut selected = Vec::new();
    let mut unselected = Vec::new();
    for index in 0..12 {
        let (key, value) = if index % 2 == 0 {
            ("Stage", "replay")
        } else {
            ("Stage", "other")
        };
        let instance_id = launch_one(
            &router,
            "the tagged launches",
            UNKNOWN_AMI,
            &[
                p("TagSpecification.1.ResourceType", "instance"),
                p("TagSpecification.1.Tag.1.Key", key),
                p("TagSpecification.1.Tag.1.Value", value),
            ],
        )
        .await;
        if value == "replay" {
            selected.push(instance_id);
        } else {
            unselected.push(instance_id);
        }
    }
    past_the_transition().await;

    // Rows: how the request narrows and pages the report. Each must answer the same SET of
    // instances, every one of them running.
    let rows: Vec<(&str, Vec<(String, String)>)> = vec![
        ("the tag, one page", filters(&[("tag:Stage", &["replay"])])),
        (
            "the tag, a wildcard value",
            filters(&[("tag:Stage", &["rep*"])]),
        ),
        (
            "the tag beside the state the instances are in",
            filters(&[
                ("tag:Stage", &["replay"]),
                ("instance-state-name", &["running"]),
            ]),
        ),
    ];

    for (case, narrowing) in &rows {
        // The page size is the smallest `DescribeInstances` pages with, so the walk below
        // spans pages without asking for a page the operation refuses.
        for page_size in [None, Some(5)] {
            let mut params = narrowing.clone();
            if let Some(page_size) = page_size {
                params.push(p("MaxResults", &page_size.to_string()));
            }
            let mut reported: Vec<String> = Vec::new();
            let mut token: Option<String> = None;
            loop {
                let mut paged = params.clone();
                if let Some(token) = token.clone() {
                    paged.push(p("NextToken", &token));
                }
                let body = ok(&router, case, "DescribeInstances", &paged).await;
                for instance in reported_instances(case, &body) {
                    assert_eq!(
                        instance.state.as_deref(),
                        Some("running"),
                        "case {case:?}: a launched instance reaches running, got {instance:?}"
                    );
                    reported.push(instance.instance_id);
                }
                token = element(&body, "nextToken");
                if token.is_none() {
                    break;
                }
            }
            assert_eq!(
                reported, selected,
                "case {case:?} with page size {page_size:?}: the tag selects exactly the \
                 instances carrying it, got {reported:?}"
            );
            for instance_id in &unselected {
                assert!(
                    !reported.contains(instance_id),
                    "case {case:?}: {instance_id} carries another tag value and must not be \
                     reported, got {reported:?}"
                );
            }
        }
    }

    // A tag nothing carries selects nothing — an empty reservation set, not every instance.
    let body = ok(
        &router,
        "a tag nothing carries",
        "DescribeInstances",
        &filters(&[("tag:Stage", &["absent"])]),
    )
    .await;
    assert!(
        reported_instances("a tag nothing carries", &body).is_empty(),
        "a tag nothing carries selects no instance, got {body}"
    );
}

/// One row of the metadata options table.
struct MetadataRow {
    case: &'static str,
    /// Launch from the published IMDSv2-only catalog image rather than from an AMI
    /// nothing holds.
    from_catalog: bool,
    extra: Vec<(String, String)>,
    expected_tokens: &'static str,
    expected_hop_limit: String,
    expected_endpoint: &'static str,
    expected_protocol_ipv6: &'static str,
    expected_tags: &'static str,
}

/// Rule: an instance's metadata options are the ones its launch named, the account's own
/// defaults for what it left out, and the documented default otherwise — and the option
/// set reaches `applied` at the instant the clock owes it, having been `pending` until
/// then.
#[tokio::test]
async fn metadata_options_follow_the_launch_and_settle() {
    let (router, _engine) = ec2_query::emulator();
    let catalog = catalog_image(&router).await;
    assert_eq!(
        catalog.imds_support.as_deref(),
        Some("v2.0"),
        "the published Amazon Linux 2023 catalog image is IMDSv2-only: {catalog:?}"
    );

    let rows = vec![
        MetadataRow {
            // An AMI this store does not hold publishes no `ImdsSupport` of its own, so
            // the instance runs under the requirement the images this crate publishes
            // carry — IMDSv2, which is "`HttpTokens` automatically set to `required` and
            // `HttpPutResponseHopLimit` set to `2`" (`RegisterImageRequest$ImdsSupport`).
            case: "an AMI nothing holds, nothing asked for",
            from_catalog: false,
            extra: Vec::new(),
            expected_tokens: HTTP_TOKENS_REQUIRED,
            expected_hop_limit: IMDS_V2_METADATA_HOP_LIMIT.to_string(),
            expected_endpoint: "enabled",
            expected_protocol_ipv6: "disabled",
            expected_tags: "disabled",
        },
        MetadataRow {
            // "If you set ImdsSupport to v2.0 … instances launched from this AMI will have
            // HttpTokens automatically set to required and HttpPutResponseHopLimit set to
            // 2" (`RegisterImageRequest$ImdsSupport`).
            case: "an IMDSv2-only AMI, nothing asked for",
            from_catalog: true,
            extra: Vec::new(),
            expected_tokens: HTTP_TOKENS_REQUIRED,
            expected_hop_limit: IMDS_V2_METADATA_HOP_LIMIT.to_string(),
            expected_endpoint: "enabled",
            expected_protocol_ipv6: "disabled",
            expected_tags: "disabled",
        },
        MetadataRow {
            case: "the launch asks for every metadata option",
            from_catalog: true,
            extra: vec![
                p("MetadataOptions.HttpTokens", "optional"),
                p("MetadataOptions.HttpPutResponseHopLimit", "64"),
                p("MetadataOptions.HttpEndpoint", "disabled"),
                p("MetadataOptions.HttpProtocolIpv6", "enabled"),
                p("MetadataOptions.InstanceMetadataTags", "enabled"),
            ],
            expected_tokens: HTTP_TOKENS_OPTIONAL,
            expected_hop_limit: "64".to_string(),
            expected_endpoint: "disabled",
            expected_protocol_ipv6: "enabled",
            expected_tags: "enabled",
        },
        MetadataRow {
            case: "the launch asks for the bottom of the hop-limit range",
            from_catalog: false,
            extra: vec![
                p("MetadataOptions.HttpTokens", "required"),
                p("MetadataOptions.HttpPutResponseHopLimit", "1"),
            ],
            expected_tokens: HTTP_TOKENS_REQUIRED,
            expected_hop_limit: "1".to_string(),
            expected_endpoint: "enabled",
            expected_protocol_ipv6: "disabled",
            expected_tags: "disabled",
        },
    ];

    for row in &rows {
        let case = row.case;
        let image_id = if row.from_catalog {
            catalog.image_id.clone()
        } else {
            UNKNOWN_AMI.to_string()
        };
        let instance_id = launch_one(&router, case, &image_id, &row.extra).await;

        // Read at once: the options a launch has just set are still being applied.
        let fresh = describe_one(&router, case, &instance_id).await;
        assert_eq!(
            pick(&fresh.metadata_options, "state").as_deref(),
            Some("pending"),
            "case {case:?}: the options of a launch just served are still pending, got {:?}",
            fresh.metadata_options
        );

        past_the_transition().await;
        let reported = describe_one(&router, case, &instance_id).await;
        for (member, expected) in [
            ("state", METADATA_OPTIONS_STATE_APPLIED.to_string()),
            ("httpTokens", row.expected_tokens.to_string()),
            ("httpPutResponseHopLimit", row.expected_hop_limit.clone()),
            ("httpEndpoint", row.expected_endpoint.to_string()),
            ("httpProtocolIpv6", row.expected_protocol_ipv6.to_string()),
            ("instanceMetadataTags", row.expected_tags.to_string()),
        ] {
            assert_eq!(
                pick(&reported.metadata_options, member).as_deref(),
                Some(expected.as_str()),
                "case {case:?}: expected metadataOptions.{member} {expected:?}, got {:?}",
                reported.metadata_options
            );
        }
    }

    // The ACCOUNT's own default stands in for what a launch leaves out.
    ok(
        &router,
        "the account default",
        "ModifyInstanceMetadataDefaults",
        &[
            p("HttpTokens", "required"),
            p("HttpPutResponseHopLimit", "8"),
            p("InstanceMetadataTags", "enabled"),
        ],
    )
    .await;
    let case = "a launch under the account's defaults";
    let instance_id = launch_one(&router, case, UNKNOWN_AMI, &[]).await;
    past_the_transition().await;
    let reported = describe_one(&router, case, &instance_id).await;
    for (member, expected) in [
        ("httpTokens", "required"),
        ("httpPutResponseHopLimit", "8"),
        ("instanceMetadataTags", "enabled"),
    ] {
        assert_eq!(
            pick(&reported.metadata_options, member).as_deref(),
            Some(expected),
            "case {case:?}: expected metadataOptions.{member} {expected:?}, got {:?}",
            reported.metadata_options
        );
    }

    // A hop limit outside the published range is refused rather than stored.
    let (status, body) = ec2_query::call(
        &router,
        REGION,
        "RunInstances",
        &[
            p("ImageId", UNKNOWN_AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("MetadataOptions.HttpPutResponseHopLimit", "65"),
        ],
    )
    .await;
    assert_eq!(
        status, 400,
        "a hop limit above the published range is refused, got {status} with body {body}"
    );
    assert_eq!(
        ec2_query::error_code(&body),
        "InvalidParameterValue",
        "a hop limit above the published range is an invalid parameter value: {body}"
    );
}
