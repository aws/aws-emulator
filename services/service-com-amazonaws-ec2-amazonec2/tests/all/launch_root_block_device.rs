// SPDX-License-Identifier: CC0-1.0

//! The root volume a LAUNCH creates, and the hibernation prerequisite asked of it, driven
//! over the wire the service actually speaks: `ec2Query` — an
//! `application/x-www-form-urlencoded` POST whose body carries `Action=…`, answered with XML
//! (README.md / OPERATIONS.md "Wire protocol").
//!
//! Two rules, one resource:
//!
//! 1. "The block device mapping, which defines the EBS volumes and instance store volumes to
//!    attach to the instance at launch" (`RunInstancesRequest$BlockDeviceMappings`): the
//!    entry a launch carries for the AMI's own root device DEFINES the root volume the launch
//!    creates — its size, type and encryption — and the AMI's mapping answers only what the
//!    launch left out. Encryption is the one member a launch can only ADD ("in no case can
//!    you remove encryption from an encrypted volume", `EbsBlockDevice$Encrypted`), and the
//!    Region's EBS encryption-by-default adds it to every new volume.
//! 2. An instance writes its memory to its root volume when it hibernates, so "the root
//!    volume must be an encrypted Amazon EBS volume" is a prerequisite of hibernation (the
//!    EC2 User Guide's "Hibernate your Amazon EC2 instance"): a launch that asks for
//!    `HibernationOptions.Configured=true` over a root volume that would not be encrypted is
//!    refused `UnsupportedHibernationConfiguration` and creates nothing at all.
//!
//! The tables span those classes rather than one recorded launch: the mapping absent, naming
//! the root device, naming another device, encrypting or not, overriding the size and the
//! type; the encryption coming from the request, from the Region's default and from a launch
//! template; and the assertions read the volume back through `DescribeVolumes` and the
//! instance through `DescribeInstances`, never against a value written here.

use std::time::{Duration, Instant};

use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, filters, indexed};
use super::wire::{child, child_text, document, Param};

/// The Region every call of this suite is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and its root device is the one the image this crate publishes by default carries.
const UNHELD_AMI: &str = "ami-0123456789abcdef0";

/// The refusal a launch that cannot hibernate is answered with.
const UNSUPPORTED_HIBERNATION: &str = "UnsupportedHibernationConfiguration";

/// The whole sentence EC2 words for it: fixed text that interpolates nothing from the
/// request.
const ROOT_MUST_BE_ENCRYPTED: &str = "For hibernation, the root device volume must be encrypted.";

/// How long a poll loop keeps looking for a clock-owed move before it gives up — a generous
/// multiple of the service's own span, so a loaded host is never the reason a case fails.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

fn p(name: &str, value: &str) -> Param {
    (name.to_string(), value.to_string())
}

/// One call that must be served, as its body.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[Param]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// The `BlockDeviceMapping.1` fields that name `device` with the Amazon EBS members
/// `members` carries.
fn mapping(device: &str, members: &[(&str, &str)]) -> Vec<Param> {
    let mut params = vec![p("BlockDeviceMapping.1.DeviceName", device)];
    for (member, value) in members {
        params.push(p(&format!("BlockDeviceMapping.1.Ebs.{member}"), value));
    }
    params
}

/// One `RunInstances` of a single instance from `image_id` with `extra` beside it.
async fn launch(
    router: &MultiServiceRouter,
    image_id: &str,
    extra: &[Param],
) -> (u16, String) {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    params.extend_from_slice(extra);
    call(router, REGION, "RunInstances", &params).await
}

/// The root device name an instance launched from `image_id` reports, read off the launch
/// itself rather than written here: it is the AMI's own `rootDeviceName`.
async fn root_device_of_a_launch(router: &MultiServiceRouter, case: &str, image_id: &str) -> String {
    let (status, body) = launch(router, image_id, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the probe launch must be served, got {status} with body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));
    let described = ok(router, case, "DescribeInstances", &indexed("InstanceId", &[&instance_id]))
        .await;
    element(&described, "rootDeviceName").unwrap_or_else(|| {
        panic!("case {case:?}: the instance must report its root device: {described}")
    })
}

/// How many instances and volumes the account holds — what a refused launch must leave
/// untouched.
async fn held_counts(router: &MultiServiceRouter, case: &str) -> (usize, usize) {
    let instances = ok(router, case, "DescribeInstances", &[]).await;
    let volumes = ok(router, case, "DescribeVolumes", &[]).await;
    (
        elements(&instances, "instanceId").len(),
        elements(&volumes, "volumeId").len(),
    )
}

/// The one volume `DescribeVolumes` reports attached to `instance_id`, as its members, once
/// the launch's clock-owed moves have been made.
async fn root_volume_of(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Vec<(String, String)> {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let body = ok(
            router,
            case,
            "DescribeVolumes",
            &filters(&[("attachment.instance-id", &[instance_id])]),
        )
        .await;
        let parsed = document(case, &body);
        let volume = parsed
            .descendants()
            .find(|node| node.is_element() && child_text(*node, "volumeId").is_some());
        if let Some(volume) = volume {
            return volume
                .children()
                .filter(roxmltree::Node::is_element)
                .map(|member| {
                    (
                        member.tag_name().name().to_string(),
                        member.text().unwrap_or_default().to_string(),
                    )
                })
                .collect();
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the launch must give its instance a root volume, got {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Whether the one instance `instance_id` names reports `hibernationOptions.configured` as
/// `true`, read through the document so the member of ANOTHER option set is never taken for
/// this one.
async fn reports_hibernation(
    router: &MultiServiceRouter,
    case: &str,
    instance_id: &str,
) -> Option<String> {
    let body = ok(
        router,
        case,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    let parsed = document(case, &body);
    let instance = parsed
        .descendants()
        .find(|node| node.is_element() && child_text(*node, "instanceId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: the describe must report the instance: {body}"));
    child(instance, "hibernationOptions").and_then(|options| child_text(options, "configured"))
}

/// The value of `member` among a volume's reported members.
fn member_of(volume: &[(String, String)], member: &str) -> Option<String> {
    volume
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, value)| value.clone())
}

/// RULE 2: a launch that asks for hibernation is admitted on whether its ROOT volume would be
/// encrypted — the mapping it carries for the AMI's root device decides it — and a refused
/// launch creates neither an instance nor a volume.
#[tokio::test]
async fn hibernation_is_admitted_on_the_root_volumes_encryption() {
    let (router, _engine) = super::ec2_query::emulator();
    let root_device = root_device_of_a_launch(&router, "the probe launch", UNHELD_AMI).await;

    // (what the launch says about its root device, the mapping fields, whether it may launch)
    let rows: Vec<(&str, Vec<Param>, bool)> = vec![
        ("no block device mapping at all", Vec::new(), false),
        (
            "the root device, explicitly unencrypted",
            mapping(&root_device, &[("Encrypted", "false")]),
            false,
        ),
        (
            "the root device, sized but saying nothing about encryption",
            mapping(&root_device, &[("VolumeSize", "30")]),
            false,
        ),
        (
            "another device encrypted, leaving the root device alone",
            mapping("/dev/sdb", &[("Encrypted", "true"), ("VolumeSize", "8")]),
            false,
        ),
        (
            "the root device, encrypted",
            mapping(&root_device, &[("Encrypted", "true")]),
            true,
        ),
        (
            "the root device, encrypted and resized onto another volume type",
            mapping(
                &root_device,
                &[
                    ("Encrypted", "true"),
                    ("VolumeSize", "40"),
                    ("VolumeType", "gp3"),
                ],
            ),
            true,
        ),
    ];

    for (case, fields, may_launch) in rows {
        let before = held_counts(&router, case).await;
        let mut params = vec![p("HibernationOptions.Configured", "true")];
        params.extend(fields);
        let (status, body) = launch(&router, UNHELD_AMI, &params).await;

        if !may_launch {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, UNSUPPORTED_HIBERNATION),
                "case {case:?}: expected 400 {UNSUPPORTED_HIBERNATION}, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_message(&body),
                ROOT_MUST_BE_ENCRYPTED,
                "case {case:?}: expected the fixed sentence, got body {body}"
            );
            assert_eq!(
                held_counts(&router, case).await,
                before,
                "case {case:?}: a refused launch must create neither an instance nor a volume"
            );
            continue;
        }

        assert_eq!(
            status, 200,
            "case {case:?}: a hibernation launch over an encrypted root must be served, got \
             {status} with body {body}"
        );
        let instance_id = element(&body, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));
        // The instance reports the hibernation it was launched with…
        assert_eq!(
            reports_hibernation(&router, case, &instance_id).await.as_deref(),
            Some("true"),
            "case {case:?}: the instance must report hibernationOptions.configured=true"
        );
        // … and the volume store really holds an encrypted root volume for it, which is what
        // the admission above was about.
        let volume = root_volume_of(&router, case, &instance_id).await;
        assert_eq!(
            member_of(&volume, "encrypted").as_deref(),
            Some("true"),
            "case {case:?}: the launch's root volume must be encrypted, got {volume:?}"
        );
    }
}

/// One row of the mapping table: the case, the Amazon EBS members the launch names for its
/// root device, and the `size`, `volumeType` and `encrypted` the created volume is then due
/// (`None` where the row leaves the member to the AMI's own).
struct MappingRow {
    case: &'static str,
    members: Vec<(&'static str, &'static str)>,
    size: Option<&'static str>,
    volume_type: Option<&'static str>,
    encrypted: &'static str,
}

/// RULE 1: the root volume a launch creates is the AMI's root device with the launch's own
/// entry for that device overlaid — so what the launch named is what `DescribeVolumes`
/// reports, and what it left out is the AMI's own.
#[tokio::test]
async fn the_root_volume_is_created_from_the_launchs_own_mapping() {
    let (router, _engine) = super::ec2_query::emulator();
    let root_device = root_device_of_a_launch(&router, "the probe launch", UNHELD_AMI).await;

    // What the launch names for its root device, and the volume that must then exist.
    let rows: Vec<MappingRow> = vec![
        MappingRow {
            case: "nothing at all",
            members: Vec::new(),
            size: None,
            volume_type: None,
            encrypted: "false",
        },
        MappingRow {
            case: "a size of its own",
            members: vec![("VolumeSize", "50")],
            size: Some("50"),
            volume_type: None,
            encrypted: "false",
        },
        MappingRow {
            case: "a size and a type of its own",
            members: vec![("VolumeSize", "12"), ("VolumeType", "gp2")],
            size: Some("12"),
            volume_type: Some("gp2"),
            encrypted: "false",
        },
        MappingRow {
            case: "encryption of its own",
            members: vec![("Encrypted", "true")],
            size: None,
            volume_type: None,
            encrypted: "true",
        },
        MappingRow {
            case: "every member at once",
            members: vec![
                ("VolumeSize", "64"),
                ("VolumeType", "gp3"),
                ("Encrypted", "true"),
            ],
            size: Some("64"),
            volume_type: Some("gp3"),
            encrypted: "true",
        },
    ];

    for row in rows {
        let MappingRow {
            case,
            members,
            size,
            volume_type,
            encrypted,
        } = row;
        let extra = if members.is_empty() {
            Vec::new()
        } else {
            mapping(&root_device, &members)
        };
        let (status, body) = launch(&router, UNHELD_AMI, &extra).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the launch must be served, got {status} with body {body}"
        );
        let instance_id = element(&body, "instanceId")
            .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"));
        let volume = root_volume_of(&router, case, &instance_id).await;

        assert_eq!(
            member_of(&volume, "encrypted").as_deref(),
            Some(encrypted),
            "case {case:?}: expected encrypted={encrypted}, got {volume:?}"
        );
        if let Some(size) = size {
            assert_eq!(
                member_of(&volume, "size").as_deref(),
                Some(size),
                "case {case:?}: expected size={size}, got {volume:?}"
            );
        }
        if let Some(volume_type) = volume_type {
            assert_eq!(
                member_of(&volume, "volumeType").as_deref(),
                Some(volume_type),
                "case {case:?}: expected volumeType={volume_type}, got {volume:?}"
            );
        }
        // The device the volume is attached at is the AMI's root device whatever the launch
        // said, so an entry never moves the root device.
        let attached = ok(
            &router,
            case,
            "DescribeVolumes",
            &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        assert_eq!(
            element(&attached, "device").as_deref(),
            Some(root_device.as_str()),
            "case {case:?}: the volume must be attached at the AMI's root device, got \
             {attached}"
        );
    }
}

/// RULE 1 and 2 together, through the other two ways a launch's root device can come to be
/// encrypted: the Region's EBS encryption-by-default, and a launch template's own mapping.
#[tokio::test]
async fn encryption_by_default_and_a_launch_template_admit_the_same_hibernation() {
    let (router, _engine) = super::ec2_query::emulator();
    let root_device = root_device_of_a_launch(&router, "the probe launch", UNHELD_AMI).await;
    let hibernate = vec![p("HibernationOptions.Configured", "true")];

    // Before the Region's default is on, the same launch cannot hibernate …
    let (status, body) = launch(&router, UNHELD_AMI, &hibernate).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, UNSUPPORTED_HIBERNATION),
        "with encryption by default off, a hibernation launch over the AMI's own root device \
         must be refused, got {status} with body {body}"
    );

    // … and with it on, every new volume is encrypted, so the same launch is served and its
    // root volume reports the encryption the Region's setting gave it.
    let enabled = ok(
        &router,
        "the Region's EBS encryption default",
        "EnableEbsEncryptionByDefault",
        &[],
    )
    .await;
    assert_eq!(
        element(&enabled, "ebsEncryptionByDefault").as_deref(),
        Some("true"),
        "the Region must report EBS encryption by default on, got {enabled}"
    );
    let (status, body) = launch(&router, UNHELD_AMI, &hibernate).await;
    assert_eq!(
        status, 200,
        "with encryption by default on, the same launch must be served, got {status} with \
         body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("the launch must report its instance: {body}"));
    let volume = root_volume_of(&router, "the Region's EBS encryption default", &instance_id).await;
    assert_eq!(
        member_of(&volume, "encrypted").as_deref(),
        Some("true"),
        "the Region's encryption default must encrypt the launch's root volume, got {volume:?}"
    );

    // A launch template expresses the same facts, so a template-driven launch and the
    // spelled-out one are served alike. The default is turned back off first, so the
    // template's own mapping is what encrypts this one.
    let disabled = ok(
        &router,
        "the Region's EBS encryption default",
        "DisableEbsEncryptionByDefault",
        &[],
    )
    .await;
    assert_eq!(
        element(&disabled, "ebsEncryptionByDefault").as_deref(),
        Some("false"),
        "the Region must report EBS encryption by default off again, got {disabled}"
    );
    let created = ok(
        &router,
        "the launch template",
        "CreateLaunchTemplate",
        &[
            p("LaunchTemplateName", "hibernation-root-encrypted"),
            p("LaunchTemplateData.ImageId", UNHELD_AMI),
            p(
                "LaunchTemplateData.BlockDeviceMapping.1.DeviceName",
                &root_device,
            ),
            p(
                "LaunchTemplateData.BlockDeviceMapping.1.Ebs.Encrypted",
                "true",
            ),
        ],
    )
    .await;
    let template_id = element(&created, "launchTemplateId")
        .unwrap_or_else(|| panic!("CreateLaunchTemplate must report the template: {created}"));
    let (status, body) = call(
        &router,
        REGION,
        "RunInstances",
        &[
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("LaunchTemplate.LaunchTemplateId", &template_id),
            p("HibernationOptions.Configured", "true"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "a launch whose template encrypts the root device must be served, got {status} with \
         body {body}"
    );
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("the template launch must report its instance: {body}"));
    let volume = root_volume_of(&router, "the launch template", &instance_id).await;
    assert_eq!(
        member_of(&volume, "encrypted").as_deref(),
        Some("true"),
        "the template's mapping must encrypt the launch's root volume, got {volume:?}"
    );
    // And the instance reports the hibernation the REQUEST asked for over that template.
    assert_eq!(
        reports_hibernation(&router, "the launch template", &instance_id)
            .await
            .as_deref(),
        Some("true"),
        "the template-driven instance must report hibernationOptions.configured=true"
    );
}
