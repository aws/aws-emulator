// SPDX-License-Identifier: CC0-1.0

//! How an instance, a network interface and a volume ATTACHMENT are projected, and the
//! state the clock owes an attachment a launch makes.
//!
//! The rules under test, each asserted over a table of inputs rather than over one launch:
//!
//! 1. A RESERVATION'S OWN `groupSet` IS "NOT SUPPORTED" (`Reservation$Groups`): it is
//!    reported EMPTY whatever groups the launch named, by `RunInstances` and by
//!    `DescribeInstances` alike, because a VPC instance's groups live on its primary network
//!    interface and are reported as the INSTANCE's own `groupSet`.
//! 2. AN ATTACHMENT IS BEING MADE WHEN THE CALL THAT MAKES IT ANSWERS: every interface a
//!    launch attaches is `attaching` in the launch's own answer and reaches `attached` at
//!    the instant the clock owes it; and the volumes a launch attaches are the instance's
//!    once that launch has completed, so an instance still `pending` reports no block device
//!    mapping entry and reports one per volume afterwards.
//! 3. AN INTERFACE REPORTS ITS PUBLIC HOSTNAME AND ITS REQUESTER: `publicDnsName` is
//!    reported for every interface — the hostname of the public address it holds, and the
//!    EMPTY name for one holding none — and `requesterId` names the principal that created
//!    it, which for an interface the caller created is this account's own principal.
//! 4. A REPORTED VOLUME ATTACHMENT CARRIES THE AVAILABILITY ZONE OF ITS VOLUME, by name and
//!    by id — "the volume must be in the same Availability Zone as the instance"
//!    (`AttachVolume`), so the attachment's zone IS the volume's.
//! 5. AN INSTANCE REPORTS THE BOOT MODE OF ITS AMI AND THE MODE ITS INSTANCE TYPE BOOTS: an
//!    AMI that "supports both UEFI and Legacy BIOS" boots UEFI on a type that supports UEFI
//!    and the legacy BIOS on one that does not, which is the type's own
//!    `SupportedBootModes`; and the IPv4 metadata endpoint is reported beside the IPv6 one.
//!
//! Every expectation is read back out of the emulator's own state — the groups the instance
//! reports, the zone the volume reports, the boot modes `DescribeInstanceTypes` publishes —
//! so no row of a table is itself the specification.

use std::time::{Duration, Instant};

use ec2::state::{
    BOOT_MODE_UEFI_PREFERRED, TRANSITION_DELAY, VOLUME_ATTACHMENT_STATE_ATTACHED,
    VOLUME_STATE_AVAILABLE,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, filters, indexed};
use super::wire::{child, child_text, document, set_items};

/// The Region every call of this suite is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and the instance reports the facts of the image this crate publishes by default.
const UNHELD_AMI: &str = "ami-0123456789abcdef0";

/// How far past [`TRANSITION_DELAY`] a poll loop keeps looking before it gives up — a
/// multiple of the service's own span, so a loaded or slow host is never the reason a case
/// fails.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the launch-attachment table: the case, how many interfaces the launch gives
/// its instance, and the parameters it carries beside the AMI.
type AttachmentRow = (&'static str, usize, Params);

fn p(name: &str, value: &str) -> (String, String) {
    (name.to_string(), value.to_string())
}

/// One call that must be served, as its body.
async fn ok(
    router: &MultiServiceRouter,
    case: &str,
    action: &str,
    params: &[(String, String)],
) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// Launch one instance with `extra` beside the AMI and answer the launch's whole body.
async fn launch(
    router: &MultiServiceRouter,
    case: &str,
    image_id: &str,
    extra: &[(String, String)],
) -> String {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    params.extend_from_slice(extra);
    ok(router, case, "RunInstances", &params).await
}

/// `DescribeInstances` narrowed to one instance, as its body.
async fn described(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    ok(
        router,
        case,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await
}

/// Every `<item>` of the `name` set of the first element of `body` that has one.
fn items_of(case: &str, body: &str, name: &str) -> Vec<Vec<(String, String)>> {
    let parsed = document(case, body);
    let Some(set) = parsed
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
    else {
        return Vec::new();
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .map(|item| {
            item.children()
                .filter(roxmltree::Node::is_element)
                .map(|member| {
                    (
                        member.tag_name().name().to_string(),
                        member.text().unwrap_or_default().to_string(),
                    )
                })
                .collect()
        })
        .collect()
}

/// Whether `body` carries a `name` element at all — the difference between a member
/// reported EMPTY and a member left out.
fn carries(case: &str, body: &str, name: &str) -> bool {
    document(case, body)
        .descendants()
        .any(|node| node.is_element() && node.tag_name().name() == name)
}

/// The one instance `<item>` of a launch or describe body, as the node it is.
fn instance_members(case: &str, body: &str) -> Vec<(String, String)> {
    items_of(case, body, "instancesSet")
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"))
}

/// The group ids the INSTANCE of a launch or describe body reports as its own — read off
/// the instance item rather than off the first `groupSet` of the document, because the
/// reservation has one too.
fn instance_group_ids(case: &str, body: &str) -> Vec<String> {
    let parsed = document(case, body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    set_items(instance, "groupSet")
        .into_iter()
        .filter_map(|group| child_text(group, "groupId"))
        .collect()
}

/// The value of `member` among `members`, `None` when the member was not reported.
fn pick(members: &[(String, String)], member: &str) -> Option<String> {
    members
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, value)| value.clone())
}

/// The id of the instance a launch body reports.
fn launched_instance_id(case: &str, body: &str) -> String {
    pick(&instance_members(case, body), "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: the launch reports no instance id in {body}"))
}

/// The `(attachment status, interface status)` of every interface the instance report in
/// `body` carries, in document order.
fn reported_attachments(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    parsed
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "networkInterfaceSet")
        .flat_map(|set| set.children().filter(roxmltree::Node::is_element))
        .map(|interface| {
            let attachment = child(interface, "attachment")
                .and_then(|attachment| child_text(attachment, "status"))
                .unwrap_or_default();
            (
                attachment,
                child_text(interface, "status").unwrap_or_default(),
            )
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Rule 1: a reservation's own groupSet is empty
// ---------------------------------------------------------------------------

/// Rule 1: whatever groups a launch names, the RESERVATION reports an empty `groupSet` and
/// the INSTANCE reports the groups its primary interface holds.
#[tokio::test]
async fn a_reservation_reports_an_empty_group_set_whatever_groups_its_instances_hold() {
    let (router, _engine) = super::ec2_query::emulator();

    // Two groups of the account's default VPC, so a launch can name none, one or both and
    // the rule is asserted over a SET of group counts rather than over one launch.
    let mut group_ids = Vec::new();
    for name in ["eni-projection-one", "eni-projection-two"] {
        let created = ok(
            &router,
            name,
            "CreateSecurityGroup",
            &[p("GroupName", name), p("GroupDescription", name)],
        )
        .await;
        group_ids.push(
            element(&created, "groupId")
                .unwrap_or_else(|| panic!("CreateSecurityGroup must mint a group: {created}")),
        );
    }

    let rows: Vec<(&str, Vec<String>)> = vec![
        ("a launch that names no group", Vec::new()),
        ("a launch that names one group", vec![group_ids[0].clone()]),
        (
            "a launch that names two groups",
            vec![group_ids[0].clone(), group_ids[1].clone()],
        ),
    ];

    for (case, named) in &rows {
        let mut extra = Vec::new();
        for (index, group_id) in named.iter().enumerate() {
            extra.push(p(&format!("SecurityGroupId.{}", index + 1), group_id));
        }
        let launched = launch(&router, case, UNHELD_AMI, &extra).await;

        // The reservation's own groupSet: present, and holding nothing.
        assert!(
            carries(case, &launched, "groupSet"),
            "case {case:?}: the reservation's groupSet is reported, got {launched}"
        );
        let reservation_groups = items_of(case, &launched, "groupSet");
        assert!(
            reservation_groups.is_empty(),
            "case {case:?}: the reservation's groupSet is empty, got {reservation_groups:?}"
        );

        // The INSTANCE's groups: the groups the launch named, or the default group it was
        // given when it named none — one entry per group, each by id and by name.
        let instance_id = launched_instance_id(case, &launched);
        let described = described(&router, case, &instance_id).await;
        for (read, from) in [
            (instance_group_ids(case, &launched), "the launch"),
            (instance_group_ids(case, &described), "the describe"),
        ] {
            assert_eq!(
                read.len(),
                named.len().max(1),
                "case {case:?}: expected {} group(s) on the instance {from} reports, got \
                 {read:?}",
                named.len().max(1)
            );
            for group_id in named {
                assert!(
                    read.contains(group_id),
                    "case {case:?}: expected the named group {group_id} among the groups {from} \
                     reports, got {read:?}"
                );
            }
        }
        // And the describe reports the reservation's own groupSet exactly as the launch did.
        assert!(
            carries(case, &described, "groupSet")
                && items_of(case, &described, "groupSet").is_empty(),
            "case {case:?}: the describe reports an empty reservation groupSet too, got \
             {described}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2: an attachment a launch makes is being made
// ---------------------------------------------------------------------------

/// Rule 2: every interface a launch attaches is `attaching` in the launch's own answer and
/// `attached` once the clock has carried it, and the launch's volumes reach the instance as
/// the launch completes.
#[tokio::test]
async fn a_launch_attachment_is_attaching_until_the_clock_completes_it() {
    let (router, _engine) = super::ec2_query::emulator();

    // The account's default subnet in the first zone, which a launch that names interfaces
    // has to place them in — read off `DescribeSubnets` rather than written here.
    let subnets = ok(
        &router,
        "the default subnet",
        "DescribeSubnets",
        &filters(&[("default-for-az", &["true"])]),
    )
    .await;
    let subnet_id = element(&subnets, "subnetId")
        .unwrap_or_else(|| panic!("the account must hold a default subnet: {subnets}"));

    let rows: Vec<AttachmentRow> = vec![
        ("a launch that names no interface", 1, Vec::new()),
        (
            "a launch that names a client token",
            1,
            vec![p("ClientToken", "eni-projection-token")],
        ),
        (
            "a launch that names two interfaces",
            2,
            vec![
                p("NetworkInterface.1.DeviceIndex", "0"),
                p("NetworkInterface.1.SubnetId", &subnet_id),
                p("NetworkInterface.2.DeviceIndex", "1"),
                p("NetworkInterface.2.SubnetId", &subnet_id),
            ],
        ),
    ];

    for (case, interfaces, extra) in &rows {
        let launched = launch(&router, case, UNHELD_AMI, extra).await;
        let members = instance_members(case, &launched);

        // The launch's own answer: every attachment is being made, and the interface that
        // holds it is in use from that moment.
        let attachments = reported_attachments(case, &launched);
        assert_eq!(
            attachments.len(),
            *interfaces,
            "case {case:?}: expected {interfaces} interface(s) in the launch's answer, got \
             {attachments:?}"
        );
        for (attachment_status, interface_status) in &attachments {
            assert_eq!(
                attachment_status, "attaching",
                "case {case:?}: an attachment the launch just made is being made, got \
                 {attachments:?}"
            );
            assert_eq!(
                interface_status, "in-use",
                "case {case:?}: an interface something holds is in use, got {attachments:?}"
            );
        }

        // The volumes of the launch are not the instance's until the launch completes.
        assert!(
            pick(&members, "blockDeviceMapping").is_some(),
            "case {case:?}: the block device mapping is reported, got {members:?}"
        );
        assert!(
            items_of(case, &launched, "blockDeviceMapping").is_empty(),
            "case {case:?}: a launch still running holds no volume yet, got {:?}",
            items_of(case, &launched, "blockDeviceMapping")
        );

        // The idempotency token: the one the launch carried, and the EMPTY value for a
        // launch that carried none — reported either way.
        let expected_token = extra
            .iter()
            .find(|(name, _)| name == "ClientToken")
            .map_or(String::new(), |(_, value)| value.clone());
        assert_eq!(
            pick(&members, "clientToken"),
            Some(expected_token.clone()),
            "case {case:?}: expected clientToken {expected_token:?}, got {members:?}"
        );

        // And the clock, not a caller, finishes every attachment and hands the instance its
        // volumes: the report is looked at AGAIN until it has, so nothing here depends on
        // how fast the host runs.
        let instance_id = launched_instance_id(case, &launched);
        let deadline = Instant::now() + POLL_DEADLINE;
        let settled = loop {
            let body = described(&router, case, &instance_id).await;
            let attachments = reported_attachments(case, &body);
            let mappings = items_of(case, &body, "blockDeviceMapping");
            if attachments
                .iter()
                .all(|(attachment, _)| attachment == "attached")
                && !mappings.is_empty()
            {
                break body;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: every attachment the launch made must reach attached and the \
                 instance must hold its volumes, got {attachments:?} and {mappings:?}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        };
        assert_eq!(
            reported_attachments(case, &settled).len(),
            *interfaces,
            "case {case:?}: the instance keeps the interfaces its launch gave it, got \
             {settled}"
        );
        // Every mapping entry names a volume `DescribeVolumes` reports attached to THIS
        // instance, so the mapping and the volume store cannot disagree.
        let volumes = ok(
            &router,
            case,
            "DescribeVolumes",
            &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        for mapping in items_of(case, &settled, "blockDeviceMapping") {
            let device = pick(&mapping, "deviceName").unwrap_or_default();
            assert!(
                !device.is_empty(),
                "case {case:?}: every mapping entry names its device, got {mapping:?}"
            );
            assert!(
                volumes.contains(&device),
                "case {case:?}: the volume store must report a volume at {device}, got \
                 {volumes}"
            );
        }
        // `DescribeNetworkInterfaces` reports the same attachments as the instance does.
        let reported = ok(
            &router,
            case,
            "DescribeNetworkInterfaces",
            &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        let interface_items = items_of(case, &reported, "networkInterfaceSet");
        assert_eq!(
            interface_items.len(),
            *interfaces,
            "case {case:?}: the interface store holds the launch's interfaces, got \
             {interface_items:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 3: an interface's public hostname and its requester
// ---------------------------------------------------------------------------

/// Rule 3: every interface reports a `publicDnsName` — empty when it holds no public
/// address — and the `requesterId` of the principal that created it.
#[tokio::test]
async fn an_interface_reports_its_public_hostname_and_the_principal_that_created_it() {
    let (router, _engine) = super::ec2_query::emulator();

    let subnets = ok(
        &router,
        "the default subnet",
        "DescribeSubnets",
        &filters(&[("default-for-az", &["true"])]),
    )
    .await;
    let subnet_id = element(&subnets, "subnetId")
        .unwrap_or_else(|| panic!("the account must hold a default subnet: {subnets}"));

    // Two interfaces that differ along what the rule generalises over: one the caller
    // created and nothing holds, and the one a LAUNCH created and its instance holds.
    let created = ok(
        &router,
        "a created interface",
        "CreateNetworkInterface",
        &[p("SubnetId", &subnet_id)],
    )
    .await;
    let created_id = element(&created, "networkInterfaceId")
        .unwrap_or_else(|| panic!("CreateNetworkInterface must mint an interface: {created}"));
    let launched = launch(&router, "a launched interface", UNHELD_AMI, &[]).await;
    let launched_id = super::ec2_query::primary_network_interface(
        &router,
        REGION,
        "a launched interface",
        &launched_instance_id("a launched interface", &launched),
    )
    .await;

    // The principal this emulator's credential speaks for, which is who created both
    // interfaces — read from the one definition of it rather than written here.
    let caller = ::service_common::caller_principal::ambient_user_id();

    for (case, network_interface_id) in [
        ("a created interface", created_id),
        ("a launched interface", launched_id),
    ] {
        let body = ok(
            &router,
            case,
            "DescribeNetworkInterfaces",
            &indexed("NetworkInterfaceId", &[&network_interface_id]),
        )
        .await;
        let parsed = document(case, &body);
        let interface = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no interface in {body}"));

        assert_eq!(
            child_text(interface, "requesterId").as_deref(),
            Some(caller.as_str()),
            "case {case:?}: the interface names the principal that created it, got {body}"
        );
        assert_eq!(
            child_text(interface, "requesterManaged").as_deref(),
            Some("false"),
            "case {case:?}: no service manages an interface the caller created, got {body}"
        );

        // The public hostname: reported either way, and the name of the public address the
        // interface holds exactly when it holds one.
        let public_dns_name = child_text(interface, "publicDnsName");
        assert!(
            public_dns_name.is_some(),
            "case {case:?}: the public hostname is reported for every interface, got {body}"
        );
        let association = child(interface, "association")
            .and_then(|association| child_text(association, "publicDnsName"));
        match association {
            Some(name) => assert_eq!(
                public_dns_name.as_deref(),
                Some(name.as_str()),
                "case {case:?}: the interface's public hostname names the address it holds, \
                 got {body}"
            ),
            None => assert_eq!(
                public_dns_name.as_deref(),
                Some(""),
                "case {case:?}: an interface holding no public address reports the empty \
                 hostname, got {body}"
            ),
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 4: a volume attachment carries the zone of its volume
// ---------------------------------------------------------------------------

/// Rule 4: a reported volume attachment carries the Availability Zone of the volume it
/// belongs to, by name and by id — asserted in two different zones, and for a volume
/// nothing holds.
#[tokio::test]
async fn a_reported_volume_attachment_carries_the_zone_of_its_volume() {
    let (router, _engine) = super::ec2_query::emulator();
    let zones = ec2::state::availability_zones_in_region(REGION);

    for (case, index, device) in [
        ("an attachment in the Region's first zone", 0, "/dev/sdf"),
        ("an attachment in another zone", 1, "/dev/sdg"),
    ] {
        let zone = zones[index].zone_name.clone();
        let launched = launch(
            &router,
            case,
            UNHELD_AMI,
            &[p("Placement.AvailabilityZone", &zone)],
        )
        .await;
        let instance_id = launched_instance_id(case, &launched);
        let created = ok(
            &router,
            case,
            "CreateVolume",
            &[p("AvailabilityZone", &zone), p("Size", "8")],
        )
        .await;
        let volume_id = element(&created, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateVolume must mint a volume: {created}"));

        // A fresh volume holds no attachment at all, so nothing names a zone for it.
        let fresh = ok(&router, case, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
        assert!(
            items_of(case, &fresh, "attachmentSet").is_empty(),
            "case {case:?}: a volume nothing holds reports no attachment, got {fresh}"
        );

        // The volume is `creating` and the instance `pending` until the clock moves them;
        // both are looked at again until they are ready for the attach.
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let body = ok(&router, case, "DescribeVolumes", &indexed("VolumeId", &[&volume_id])).await;
            if element(&body, "status").as_deref() == Some(VOLUME_STATE_AVAILABLE) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the volume must become available, got {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }

        ok(
            &router,
            case,
            "AttachVolume",
            &[
                p("VolumeId", &volume_id),
                p("InstanceId", &instance_id),
                p("Device", device),
            ],
        )
        .await;

        let reported = ok(
            &router,
            case,
            "DescribeVolumes",
            &indexed("VolumeId", &[&volume_id]),
        )
        .await;
        let parsed = document(case, &reported);
        let volume = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "volumeId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no volume in {reported}"));
        let volume_zone = child_text(volume, "availabilityZone").unwrap_or_default();
        let volume_zone_id = child_text(volume, "availabilityZoneId").unwrap_or_default();
        assert_eq!(
            volume_zone, zone,
            "case {case:?}: the volume is in the zone its create named, got {reported}"
        );
        let attachments = set_items(volume, "attachmentSet");
        assert_eq!(
            attachments.len(),
            1,
            "case {case:?}: the volume holds the one attachment just made, got {reported}"
        );
        for attachment in attachments {
            assert_eq!(
                child_text(attachment, "availabilityZone").as_deref(),
                Some(volume_zone.as_str()),
                "case {case:?}: the attachment is in the volume's own zone, got {reported}"
            );
            assert_eq!(
                child_text(attachment, "availabilityZoneId").as_deref(),
                Some(volume_zone_id.as_str()),
                "case {case:?}: the attachment names the id of that zone, got {reported}"
            );
            assert!(
                !volume_zone_id.is_empty(),
                "case {case:?}: the zone id is a value, got {reported}"
            );
        }

        // Once the clock has finished the attach, the zone is still the volume's: the
        // elements are a function of the record, not of the state it is in.
        let deadline = Instant::now() + POLL_DEADLINE;
        loop {
            let body = ok(
                &router,
                case,
                "DescribeVolumes",
                &indexed("VolumeId", &[&volume_id]),
            )
            .await;
            let attached = items_of(case, &body, "attachmentSet")
                .into_iter()
                .all(|attachment| {
                    pick(&attachment, "status").as_deref() == Some(VOLUME_ATTACHMENT_STATE_ATTACHED)
                        && pick(&attachment, "availabilityZone").as_deref() == Some(zone.as_str())
                });
            if attached {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: the attachment must reach attached in the volume's zone, got \
                 {body}"
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

// ---------------------------------------------------------------------------
// Rule 5: the AMI's boot mode, the mode the type boots, and the IPv4 endpoint
// ---------------------------------------------------------------------------

/// Rule 5: an instance reports the boot mode of its AMI and the mode its own instance type
/// boots, and reports the IPv4 metadata endpoint beside the IPv6 one.
///
/// The rows span the classes of instance type the boot-mode rule turns on — a pre-Nitro x86
/// type, two Nitro x86 types and a Graviton one — and the expectation for each is read off
/// the `supportedBootModes` `DescribeInstanceTypes` publishes for that type rather than
/// written here.
#[tokio::test]
async fn an_instance_reports_the_boot_mode_of_its_ami_and_the_mode_its_type_boots() {
    let (router, _engine) = super::ec2_query::emulator();

    for case in ["t2.micro", "t3.micro", "m5.large", "t4g.nano"] {
        let published = ok(
            &router,
            case,
            "DescribeInstanceTypes",
            &indexed("InstanceType", &[case]),
        )
        .await;
        let parsed = document(case, &published);
        let supported: Vec<String> = parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "supportedBootModes")
            .map(|set| {
                set.children()
                    .filter(roxmltree::Node::is_element)
                    .map(|item| item.text().unwrap_or_default().to_string())
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !supported.is_empty(),
            "case {case:?}: the type must publish the boot modes it supports, got {published}"
        );

        let launched = launch(&router, case, UNHELD_AMI, &[p("InstanceType", case)]).await;
        let members = instance_members(case, &launched);

        // The AMI's own boot mode: an image this store does not hold is the EBS-backed
        // Linux image this crate publishes, whose boot mode supports both modes.
        assert_eq!(
            pick(&members, "bootMode").as_deref(),
            Some(BOOT_MODE_UEFI_PREFERRED),
            "case {case:?}: expected the published image's boot mode, got {members:?}"
        );
        // And the mode the instance BOOTS: UEFI exactly where its type supports UEFI.
        let expected = if supported.iter().any(|mode| mode == "uefi") {
            "uefi"
        } else {
            "legacy-bios"
        };
        assert_eq!(
            pick(&members, "currentInstanceBootMode").as_deref(),
            Some(expected),
            "case {case:?}: a type supporting {supported:?} boots {expected:?}, got \
             {members:?}"
        );

        // The metadata option set: the IPv4 endpoint beside the IPv6 one, for every
        // instance.
        let instance_id = launched_instance_id(case, &launched);
        let described = described(&router, case, &instance_id).await;
        let parsed = document(case, &described);
        let options = parsed
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "metadataOptions")
            .unwrap_or_else(|| panic!("case {case:?}: no metadata options in {described}"));
        assert_eq!(
            child_text(options, "httpProtocolIpv4").as_deref(),
            Some("enabled"),
            "case {case:?}: the IPv4 metadata endpoint is reported enabled, got {described}"
        );
        assert!(
            child_text(options, "httpProtocolIpv6").is_some(),
            "case {case:?}: the IPv6 endpoint is reported beside it, got {described}"
        );
    }

    // The span every clock-owed move of this crate takes is the service's own, so the poll
    // loops above are bounded by a multiple of it rather than by a figure of their own.
    assert!(
        POLL_DEADLINE > TRANSITION_DELAY * 4,
        "the poll deadline must be far past the service's own transition span, got \
         {POLL_DEADLINE:?} against {TRANSITION_DELAY:?}"
    );
}
