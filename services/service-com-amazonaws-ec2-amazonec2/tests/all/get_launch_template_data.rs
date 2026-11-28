// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `GetLaunchTemplateData`, driven over the wire the service actually
//! speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. the operation REPORTS THE LAUNCH PARAMETERS THE SERVICE HOLDS for the instance the
//!    request names — "retrieves the configuration data of the specified instance. You can
//!    use this data to create a launch template" — so every member of the reported
//!    `launchTemplateData` is read off that instance's own state, and the one the launch set
//!    is the one that comes back; a member no state of the instance answers is left out
//!    rather than defaulted;
//! 2. the data cannot DISAGREE with the describes the same state serves: the instance's own
//!    report (`DescribeInstances`), the volumes it holds (`DescribeVolumes`), its interfaces,
//!    its user data (`DescribeInstanceAttribute`) and its credit option
//!    (`DescribeInstanceCreditSpecifications`) are where each member comes from;
//! 3. an instance id OUTSIDE THE FORM EC2 ISSUES is refused by syntax, before any lookup,
//!    with `InvalidInstanceID.Malformed`, and only an id inside that form which names no
//!    instance of the account is `InvalidInstanceID.NotFound`;
//! 4. `DryRun` answers whether the call WOULD have succeeded — `DryRunOperation` for a
//!    request that would have been served, and the refusal itself for one that would have
//!    been refused;
//! 5. the operation is a READ: two calls about one settled instance answer the same thing
//!    and neither moves the instance.
//!
//! Each table row is a distinct input along what the rule generalises over — the instance
//! type and so whether it has a credit option at all, whether the launch named a key pair,
//! user data, tags, detailed monitoring, stop and termination protection, a shutdown
//! behaviour or metadata options, how many interfaces it attached, how many volumes the
//! instance holds, and for the id rule every way an id can fall outside the form — so no row
//! is the specification.

use std::time::{Duration, Instant};

use ec2::state::{ComAmazonawsEc2Amazonec2State as Ec2State, ImageSpec, RESOURCE_TYPE_INSTANCE};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed, tag_specification};
use super::wire::{child, child_text, document, set_items};

/// The Region every call of this suite is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and the instance reports the id it was given.
const UNHELD_AMI: &str = "ami-0123456789abcdef0";

/// How long a poll loop keeps looking before it gives up — generous, so a loaded or slow
/// host is never the reason a case fails.
const POLL_DEADLINE: Duration = Duration::from_secs(120);

/// How long a poll loop waits before looking again.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One stored volume as the mapping assertions compare it: the device it is attached at, its
/// size, its type and the snapshot it was created from.
type VolumeRow = (String, Option<String>, Option<String>, Option<String>);

/// One interface the instance holds, as the interface assertions compare it: its id, its
/// subnet, its primary private address and the device index it is attached at.
type InterfaceRow = (String, Option<String>, Option<String>, Option<String>);

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One call that must be served, as its body.
async fn ok(router: &MultiServiceRouter, case: &str, action: &str, params: &[(String, String)]) -> String {
    let (status, body) = call(router, REGION, action, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from {action}, got {status} with body {body}"
    );
    body
}

/// One emulator over its own engine, with one image of the calling account seeded — so a row
/// can launch from an AMI the store holds as well as from one it does not.
fn emulator() -> (MultiServiceRouter, String) {
    let (router, engine) = super::ec2_query::emulator();
    let image_id = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        guard.insert_image(ImageSpec {
            name: "launch-template-data-image".to_string(),
            ..ImageSpec::default()
        })
    };
    (router, image_id)
}

/// Launch one instance from `image_id` with `extra` beside it, and answer its id.
async fn launch(router: &MultiServiceRouter, case: &str, image_id: &str, extra: &[(String, String)]) -> String {
    let mut params = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    params.extend_from_slice(extra);
    // A launch is GIVEN the key pair it names, so the account has to hold one under that name
    // before the launch can take it.
    for (_, key_name) in extra.iter().filter(|(key, _)| key == "KeyName") {
        ok(router, case, "CreateKeyPair", &[p("KeyName", key_name)]).await;
    }
    let body = ok(router, case, "RunInstances", &params).await;
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances must report the instance it minted: {body}"))
}

/// The `GetLaunchTemplateData` body for one instance.
async fn launch_template_data(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    ok(
        router,
        case,
        "GetLaunchTemplateData",
        &[p("InstanceId", instance_id)],
    )
    .await
}

/// The text of the reported `launchTemplateData` member at `path` — dotted, so
/// `monitoring.enabled` reads the `<enabled>` inside `<monitoring>`. `None` when any step of
/// the path is absent, `Some("")` for an element present and empty: the two cases an
/// assertion about an optional member has to tell apart.
fn at(case: &str, body: &str, path: &str) -> Option<String> {
    let parsed = document(case, body);
    let mut node = child(parsed.root_element(), "launchTemplateData")
        .unwrap_or_else(|| panic!("case {case:?}: no <launchTemplateData> in {body}"));
    let mut steps = path.split('.').peekable();
    while let Some(step) = steps.next() {
        if steps.peek().is_none() {
            return child_text(node, step);
        }
        node = child(node, step)?;
    }
    None
}

/// Whether the reported data carries a `name` element at all, at any depth — the difference
/// between a member reported EMPTY and a member left out.
fn carries(case: &str, body: &str, name: &str) -> bool {
    let parsed = document(case, body);
    let data = child(parsed.root_element(), "launchTemplateData")
        .unwrap_or_else(|| panic!("case {case:?}: no <launchTemplateData> in {body}"));
    data.descendants()
        .any(|node| node.is_element() && node.tag_name().name() == name)
}

/// Every `<item>` of the reported data's `name` set, as `(member, text)` pairs per item.
fn items(case: &str, body: &str, name: &str) -> Vec<Vec<(String, String)>> {
    let parsed = document(case, body);
    let data = child(parsed.root_element(), "launchTemplateData")
        .unwrap_or_else(|| panic!("case {case:?}: no <launchTemplateData> in {body}"));
    set_items(data, name)
        .into_iter()
        .map(|item| {
            item.descendants()
                .filter(|node| node.is_element() && node != &item)
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

/// The value of `member` among `members`, `None` when the member was not reported.
fn pick(members: &[(String, String)], member: &str) -> Option<String> {
    members
        .iter()
        .find(|(name, _)| name == member)
        .map(|(_, value)| value.clone())
}

/// The one instance element of a `DescribeInstances` body, as `(member, text)` pairs — the
/// scalar members only, so a member of a nested set is never read as the instance's own.
fn described_members(case: &str, body: &str) -> Vec<(String, String)> {
    let parsed = document(case, body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    instance
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|member| {
            (
                member.tag_name().name().to_string(),
                member.text().unwrap_or_default().to_string(),
            )
        })
        .collect()
}

/// The text at `path` under the one instance of a `DescribeInstances` body, read the same way
/// [`at`] reads the launch data, so the two reports are compared member for member.
fn described_at(case: &str, body: &str, path: &str) -> Option<String> {
    let parsed = document(case, body);
    let mut node = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    let mut steps = path.split('.').peekable();
    while let Some(step) = steps.next() {
        if steps.peek().is_none() {
            return child_text(node, step);
        }
        node = child(node, step)?;
    }
    None
}

/// `DescribeInstances` narrowed to one instance.
async fn described(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    ok(
        router,
        case,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await
}

/// The `instanceState/name` of one instance.
async fn state_of(router: &MultiServiceRouter, instance_id: &str) -> String {
    super::ec2_query::described_instance_state(router, REGION, instance_id).await
}

/// Look again until `instance_id` is `running` and holds at least one volume: the clock, not
/// a caller, completes a launch, so the test waits for the CONDITION rather than for a span.
async fn settled(router: &MultiServiceRouter, case: &str, instance_id: &str) {
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let body = described(router, case, instance_id).await;
        let running = described_at(case, &body, "instanceState.name").as_deref() == Some("running");
        let volumes = ok(
            router,
            case,
            "DescribeVolumes",
            &filters(&[("attachment.instance-id", &[instance_id])]),
        )
        .await;
        if running && element(&volumes, "volumeId").is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the launch must complete and hand the instance its volumes, got \
             {body} and {volumes}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// One launch form whose reported launch data is read back.
struct Case {
    label: &'static str,
    /// Whether the launch names the AMI the store holds (`false` names one it does not).
    held_image: bool,
    /// The parameters the launch carries beside the AMI.
    params: Params,
    /// The members the data must carry, as `(dotted path, value)`.
    expected: Vec<(&'static str, &'static str)>,
    /// The elements the data must NOT carry at all.
    absent: Vec<&'static str>,
}

/// Rule 1: whatever the launch form, the reported data is that instance's own configuration —
/// the settings the launch made come back, and a setting the instance has no answer for is
/// left out.
#[tokio::test]
async fn reported_data_is_the_instances_own_configuration() {
    let cases = vec![
        Case {
            label: "a burstable launch that names nothing else",
            held_image: true,
            params: vec![p("InstanceType", "t3.micro")],
            expected: vec![
                ("instanceType", "t3.micro"),
                ("monitoring.enabled", "false"),
                ("ebsOptimized", "false"),
                ("disableApiTermination", "false"),
                ("disableApiStop", "false"),
                ("instanceInitiatedShutdownBehavior", "stop"),
                // A burstable performance instance has a credit option; one whose launch
                // named none runs the account default for its family, and "T3 … instances
                // launch as unlimited by default".
                ("creditSpecification.cpuCredits", "unlimited"),
                ("enclaveOptions.enabled", "false"),
                ("hibernationOptions.configured", "false"),
            ],
            absent: vec![
                "userData",
                "keyName",
                "kernelId",
                "ramDiskId",
                "tagSpecificationSet",
                "securityGroupIdSet",
                "securityGroupSet",
                "instanceMarketOptions",
                "instanceRequirements",
                "licenseSet",
                "operator",
                "elasticGpuSpecificationSet",
                "elasticInferenceAcceleratorSet",
                // The addresses themselves are reported, so no count asks a launch to pick
                // that many new ones.
                "ipv6AddressCount",
                "secondaryPrivateIpAddressCount",
                "ipv4PrefixCount",
                "ipv6PrefixCount",
            ],
        },
        Case {
            label: "a configured launch on a type with no credit option",
            held_image: true,
            params: vec![
                p("InstanceType", "m5.large"),
                p("KeyName", "launch-template-data-key"),
                p("UserData", "ZWNob0BsYXVuY2g="),
                p("EbsOptimized", "true"),
                p("DisableApiTermination", "true"),
                p("DisableApiStop", "true"),
                p("InstanceInitiatedShutdownBehavior", "terminate"),
                p("Monitoring.Enabled", "true"),
                p("KernelId", "aki-0123456789abcdef0"),
                p("RamdiskId", "ari-0123456789abcdef0"),
                p("MetadataOptions.HttpTokens", "required"),
                p("MetadataOptions.HttpPutResponseHopLimit", "3"),
                p("MetadataOptions.InstanceMetadataTags", "enabled"),
            ],
            expected: vec![
                ("instanceType", "m5.large"),
                ("keyName", "launch-template-data-key"),
                ("userData", "ZWNob0BsYXVuY2g="),
                ("ebsOptimized", "true"),
                ("disableApiTermination", "true"),
                ("disableApiStop", "true"),
                ("instanceInitiatedShutdownBehavior", "terminate"),
                ("monitoring.enabled", "true"),
                ("kernelId", "aki-0123456789abcdef0"),
                ("ramDiskId", "ari-0123456789abcdef0"),
                ("metadataOptions.httpTokens", "required"),
                ("metadataOptions.httpPutResponseHopLimit", "3"),
                ("metadataOptions.instanceMetadataTags", "enabled"),
            ],
            // "If you specify an instance ID that is not a burstable performance instance,
            // Amazon EC2 returns the standard credit option" — the instance has no credit
            // option of its own, and a launch template naming one for such a type could not
            // be used.
            absent: vec!["creditSpecification", "instanceMarketOptions"],
        },
        Case {
            label: "a burstable launch of the family that launches standard",
            held_image: true,
            params: vec![p("InstanceType", "t2.micro")],
            expected: vec![
                ("instanceType", "t2.micro"),
                // "T2 instances launch as standard by default".
                ("creditSpecification.cpuCredits", "standard"),
            ],
            absent: vec!["userData"],
        },
        Case {
            label: "a burstable launch that names the credit option itself, tagged",
            held_image: true,
            params: {
                let mut params = vec![
                    p("InstanceType", "t2.small"),
                    p("CreditSpecification.CpuCredits", "unlimited"),
                ];
                params.extend(tag_specification(
                    RESOURCE_TYPE_INSTANCE,
                    &[("Name", "launch-data"), ("tier", "web")],
                ));
                params
            },
            expected: vec![
                ("instanceType", "t2.small"),
                // The launch's own choice stands over the family's default.
                ("creditSpecification.cpuCredits", "unlimited"),
            ],
            absent: vec!["userData"],
        },
        Case {
            label: "a burstable launch of another family",
            held_image: true,
            params: vec![p("InstanceType", "t3a.small")],
            expected: vec![
                ("instanceType", "t3a.small"),
                ("creditSpecification.cpuCredits", "unlimited"),
            ],
            absent: vec!["userData"],
        },
        Case {
            label: "a launch from an AMI this account does not hold",
            held_image: false,
            params: vec![p("InstanceType", "t3.micro")],
            expected: vec![("instanceType", "t3.micro")],
            absent: vec!["userData", "keyName"],
        },
    ];

    for case in cases {
        let (router, held) = emulator();
        let image_id = if case.held_image {
            held.clone()
        } else {
            UNHELD_AMI.to_string()
        };
        let instance_id = launch(&router, case.label, &image_id, &case.params).await;
        let body = launch_template_data(&router, case.label, &instance_id).await;

        // The AMI the instance was launched from is reported as the instance holds it.
        assert_eq!(
            at(case.label, &body, "imageId"),
            Some(image_id.clone()),
            "case {:?}: the data names the AMI the instance was launched from, got {body}",
            case.label
        );
        for (path, value) in &case.expected {
            assert_eq!(
                at(case.label, &body, path).as_deref(),
                Some(*value),
                "case {:?}: expected {path} = {value:?}, got {:?} in {body}",
                case.label,
                at(case.label, &body, path)
            );
        }
        for name in &case.absent {
            assert!(
                !carries(case.label, &body, name),
                "case {:?}: nothing of this instance answers <{name}>, got {body}",
                case.label
            );
        }

        // The tags the launch assigned come back as the instance's own tag specification.
        let launched_tags: Vec<(String, String)> = case
            .params
            .iter()
            .filter(|(key, _)| key.ends_with(".Key"))
            .map(|(key, tag_key)| {
                let value = case
                    .params
                    .iter()
                    .find(|(other, _)| *other == key.replace(".Key", ".Value"))
                    .map(|(_, value)| value.clone())
                    .unwrap_or_default();
                (tag_key.clone(), value)
            })
            .collect();
        let specifications = items(case.label, &body, "tagSpecificationSet");
        if launched_tags.is_empty() {
            assert!(
                specifications.is_empty(),
                "case {:?}: an untagged instance names no resource to tag, got {specifications:?}",
                case.label
            );
        } else {
            assert_eq!(
                specifications.len(),
                1,
                "case {:?}: the instance's own tags are one specification, got {specifications:?}",
                case.label
            );
            let reported = &specifications[0];
            assert_eq!(
                pick(reported, "resourceType").as_deref(),
                Some(RESOURCE_TYPE_INSTANCE),
                "case {:?}: the tags are the ones applied to the instance, got {reported:?}",
                case.label
            );
            for (key, value) in &launched_tags {
                assert!(
                    reported.iter().any(|(name, text)| name == "key" && text == key)
                        && reported
                            .iter()
                            .any(|(name, text)| name == "value" && text == value),
                    "case {:?}: the tag {key}={value} the launch assigned must be reported, got \
                     {reported:?}",
                    case.label
                );
            }
        }

        // Rule 2: the data and the instance's own report are one state read twice.
        let instance = described(&router, case.label, &instance_id).await;
        for path in [
            "imageId",
            "instanceType",
            "keyName",
            "kernelId",
            "ramdiskId",
            "ebsOptimized",
        ] {
            // The launch data spells the RAM disk `ramDiskId` and the instance `ramdiskId`,
            // which is the only name the two reports do not share.
            let data_path = if path == "ramdiskId" { "ramDiskId" } else { path };
            assert_eq!(
                at(case.label, &body, data_path),
                described_at(case.label, &instance, path),
                "case {:?}: {data_path} must be the instance's own {path}, got {body} and \
                 {instance}",
                case.label
            );
        }
        for (data_path, instance_path) in [
            ("placement.availabilityZone", "placement.availabilityZone"),
            ("placement.tenancy", "placement.tenancy"),
            ("placement.groupName", "placement.groupName"),
            ("cpuOptions.coreCount", "cpuOptions.coreCount"),
            ("cpuOptions.threadsPerCore", "cpuOptions.threadsPerCore"),
            ("metadataOptions.httpTokens", "metadataOptions.httpTokens"),
            ("metadataOptions.httpEndpoint", "metadataOptions.httpEndpoint"),
            (
                "metadataOptions.httpPutResponseHopLimit",
                "metadataOptions.httpPutResponseHopLimit",
            ),
            (
                "metadataOptions.instanceMetadataTags",
                "metadataOptions.instanceMetadataTags",
            ),
            ("enclaveOptions.enabled", "enclaveOptions.enabled"),
            ("hibernationOptions.configured", "hibernationOptions.configured"),
            ("maintenanceOptions.autoRecovery", "maintenanceOptions.autoRecovery"),
            (
                "privateDnsNameOptions.hostnameType",
                "privateDnsNameOptions.hostnameType",
            ),
            (
                "networkPerformanceOptions.bandwidthWeighting",
                "networkPerformanceOptions.bandwidthWeighting",
            ),
            (
                "capacityReservationSpecification.capacityReservationPreference",
                "capacityReservationSpecification.capacityReservationPreference",
            ),
        ] {
            assert_eq!(
                at(case.label, &body, data_path),
                described_at(case.label, &instance, instance_path),
                "case {:?}: {data_path} must be what the instance reports as {instance_path}, \
                 got {body} and {instance}",
                case.label
            );
        }

        // The user data the launch supplied is the one `DescribeInstanceAttribute` reports.
        let attribute = ok(
            &router,
            case.label,
            "DescribeInstanceAttribute",
            &[p("InstanceId", instance_id.as_str()), p("Attribute", "userData")],
        )
        .await;
        assert_eq!(
            at(case.label, &body, "userData"),
            element(&attribute, "value"),
            "case {:?}: the user data must be the instance's own attribute, got {body} and \
             {attribute}",
            case.label
        );

        // The credit option is the one `DescribeInstanceCreditSpecifications` reports, for
        // every instance that has one of its own.
        if at(case.label, &body, "creditSpecification.cpuCredits").is_some() {
            let credits = ok(
                &router,
                case.label,
                "DescribeInstanceCreditSpecifications",
                &indexed("InstanceId", &[instance_id.as_str()]),
            )
            .await;
            assert_eq!(
                at(case.label, &body, "creditSpecification.cpuCredits"),
                element(&credits, "cpuCredits"),
                "case {:?}: the credit option must be the instance's own, got {body} and \
                 {credits}",
                case.label
            );
        }

        // And the instance the data was read from is still where it was: the read moved
        // nothing a caller can see.
        let members = described_members(case.label, &instance);
        assert_eq!(
            pick(&members, "instanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {:?}: the instance must still be the one the data was read from, got \
             {instance}",
            case.label
        );
    }
}

/// Rule 1 and 2, for the volumes: the block device mappings name the volumes the instance
/// HOLDS, with the settings those volumes really have — and an instance that holds none
/// reports no mapping at all.
#[tokio::test]
async fn block_device_mappings_name_the_volumes_the_instance_holds() {
    let case = "an instance that holds two volumes";
    let (router, image_id) = emulator();
    let instance_id = launch(&router, case, &image_id, &[p("InstanceType", "t3.micro")]).await;

    // A launch still running holds no volume yet, so there is no mapping to report.
    let launching = launch_template_data(&router, case, &instance_id).await;
    if state_of(&router, &instance_id).await == "pending" {
        assert!(
            !carries(case, &launching, "blockDeviceMappingSet"),
            "case {case:?}: a launch still running holds no volume to report, got {launching}"
        );
    }

    settled(&router, case, &instance_id).await;

    // A second volume, differing from the root volume along size, type and the snapshot it
    // was created from — which is what the mapping has to report off the volume itself.
    let zone = described_at(case, &described(&router, case, &instance_id).await, "placement.availabilityZone")
        .unwrap_or_else(|| panic!("case {case:?}: the instance must report its zone"));
    let created = ok(
        &router,
        case,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone.as_str()),
            p("Size", "20"),
            p("VolumeType", "gp3"),
        ],
    )
    .await;
    let volume_id =
        element(&created, "volumeId").unwrap_or_else(|| panic!("case {case:?}: no volume in {created}"));
    // The volume is `creating` until the clock moves it, so the attach is looked at again
    // until it is served rather than waited for by the wall clock.
    let deadline = Instant::now() + POLL_DEADLINE;
    loop {
        let (status, body) = call(
            &router,
            REGION,
            "AttachVolume",
            &[
                p("VolumeId", volume_id.as_str()),
                p("InstanceId", instance_id.as_str()),
                p("Device", "/dev/sdf"),
            ],
        )
        .await;
        if status == 200 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: the volume must become attachable, got {status}: {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }

    // Every device the volume store reports for this instance, with the settings of the
    // volume attached there — the expectation is read out of the emulator's own state.
    let volumes = ok(
        &router,
        case,
        "DescribeVolumes",
        &filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
    )
    .await;
    let parsed = document(case, &volumes);
    let mut expected: Vec<VolumeRow> = parsed
        .descendants()
        .filter(|node| node.is_element() && child(*node, "volumeId").is_some() && child(*node, "size").is_some())
        .map(|volume| {
            let device = set_items(volume, "attachmentSet")
                .into_iter()
                .find_map(|attachment| child_text(attachment, "device"))
                .unwrap_or_default();
            (
                device,
                child_text(volume, "size"),
                child_text(volume, "volumeType"),
                child_text(volume, "snapshotId"),
            )
        })
        .collect();
    expected.sort();
    assert!(
        expected.len() >= 2,
        "case {case:?}: the instance must hold its root volume and the attached one, got \
         {volumes}"
    );

    let body = launch_template_data(&router, case, &instance_id).await;
    let mut reported: Vec<VolumeRow> =
        items(case, &body, "blockDeviceMappingSet")
            .into_iter()
            .map(|mapping| {
                (
                    pick(&mapping, "deviceName").unwrap_or_default(),
                    pick(&mapping, "volumeSize"),
                    pick(&mapping, "volumeType"),
                    pick(&mapping, "snapshotId"),
                )
            })
            .collect();
    reported.sort();
    assert_eq!(
        reported, expected,
        "case {case:?}: every mapping must name a volume the instance holds, with that \
         volume's own size, type and snapshot, got {body} and {volumes}"
    );

    // The attached volume's own row, so the assertion above cannot pass on the root volume
    // alone: the mapping at the device the attach named reports the size the create asked
    // for, which differs from the root volume's.
    let attached = reported
        .iter()
        .find(|(device, _, _, _)| device == "/dev/sdf")
        .unwrap_or_else(|| panic!("case {case:?}: no mapping at the attached device in {body}"));
    assert_eq!(
        attached.1.as_deref(),
        Some("20"),
        "case {case:?}: the mapping reports the size of the volume attached there, got \
         {attached:?}"
    );
    assert_eq!(
        attached.2.as_deref(),
        Some("gp3"),
        "case {case:?}: the mapping reports the type of the volume attached there, got \
         {attached:?}"
    );
}

/// Rule 1 and 2, for the interfaces: the reported interfaces are the ones the instance holds,
/// carrying the groups and addresses they carry — and the security groups are NOT reported
/// beside them, because "if you specify a network interface, you must specify any security
/// groups as part of the network interface instead of using this parameter"
/// (`RunInstancesRequest$SecurityGroupIds`).
#[tokio::test]
async fn network_interfaces_carry_the_groups_and_addresses_the_instance_holds() {
    let (router, image_id) = emulator();

    // The account's default subnet in the first zone, read off `DescribeSubnets`.
    let subnets = ok(
        &router,
        "the default subnet",
        "DescribeSubnets",
        &filters(&[("default-for-az", &["true"])]),
    )
    .await;
    let subnet_id = element(&subnets, "subnetId")
        .unwrap_or_else(|| panic!("the account must hold a default subnet: {subnets}"));
    let group = ok(
        &router,
        "a security group",
        "CreateSecurityGroup",
        &[
            p("GroupName", "launch-template-data-group"),
            p("GroupDescription", "the group the launch names"),
        ],
    )
    .await;
    let group_id =
        element(&group, "groupId").unwrap_or_else(|| panic!("CreateSecurityGroup must mint a group: {group}"));

    // Two launches differing in how many interfaces the instance holds.
    let rows: Vec<(&str, Params, usize)> = vec![
        (
            "a launch that names a subnet and a group",
            vec![
                p("InstanceType", "t3.micro"),
                p("SubnetId", subnet_id.as_str()),
                p("SecurityGroupId.1", group_id.as_str()),
            ],
            1,
        ),
        (
            "a launch that names two interfaces",
            vec![
                p("InstanceType", "t3.micro"),
                p("NetworkInterface.1.DeviceIndex", "0"),
                p("NetworkInterface.1.SubnetId", subnet_id.as_str()),
                p("NetworkInterface.2.DeviceIndex", "1"),
                p("NetworkInterface.2.SubnetId", subnet_id.as_str()),
            ],
            2,
        ),
    ];

    for (case, params, interfaces) in rows {
        let instance_id = launch(&router, case, &image_id, &params).await;
        let body = launch_template_data(&router, case, &instance_id).await;
        let reported = items(case, &body, "networkInterfaceSet");
        assert_eq!(
            reported.len(),
            interfaces,
            "case {case:?}: the data reports the interfaces the instance holds, got {body}"
        );

        // Every interface the instance reports, by id, with the subnet and the primary
        // address it carries — read out of `DescribeInstances` rather than written here.
        let instance = described(&router, case, &instance_id).await;
        let parsed = document(case, &instance);
        let held: Vec<InterfaceRow> = parsed
            .descendants()
            .filter(|node| node.is_element() && child(*node, "networkInterfaceId").is_some())
            .map(|interface| {
                (
                    child_text(interface, "networkInterfaceId").unwrap_or_default(),
                    child_text(interface, "subnetId"),
                    child_text(interface, "privateIpAddress"),
                    child(interface, "attachment")
                        .and_then(|attachment| child_text(attachment, "deviceIndex")),
                )
            })
            .collect();

        for item in &reported {
            let interface_id = pick(item, "networkInterfaceId").unwrap_or_default();
            let matching = held
                .iter()
                .find(|(id, _, _, _)| *id == interface_id)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: {interface_id} is not an interface the instance holds: {instance}")
                });
            assert_eq!(
                pick(item, "subnetId"),
                matching.1,
                "case {case:?}: the interface's subnet is the one it is in, got {item:?}"
            );
            assert_eq!(
                pick(item, "privateIpAddress"),
                matching.2,
                "case {case:?}: the interface's address is the one it carries, got {item:?}"
            );
            assert_eq!(
                pick(item, "deviceIndex"),
                matching.3,
                "case {case:?}: the interface's device index is the one it is attached at, got \
                 {item:?}"
            );
            assert_eq!(
                pick(item, "deleteOnTermination").as_deref(),
                Some("true"),
                "case {case:?}: an interface a launch created is deleted with its instance, \
                 got {item:?}"
            );
            // The public IPv4 setting is a fact of the interface a launch creates, so it is
            // reported for that one alone.
            match pick(item, "deviceIndex").as_deref() {
                Some("0") => assert!(
                    pick(item, "associatePublicIpAddress").is_some(),
                    "case {case:?}: the launch's own interface reports whether a public IPv4 \
                     address is mapped to it, got {item:?}"
                ),
                _ => assert!(
                    pick(item, "associatePublicIpAddress").is_none(),
                    "case {case:?}: only the interface at device index 0 carries that \
                     setting, got {item:?}"
                ),
            }
        }

        // The group the launch named is on the interface, and nowhere else.
        if interfaces == 1 {
            let groups: Vec<String> = reported[0]
                .iter()
                .filter(|(name, _)| name == "groupId")
                .map(|(_, value)| value.clone())
                .collect();
            assert!(
                groups.contains(&group_id),
                "case {case:?}: the group the launch named is carried by the interface, got \
                 {groups:?} in {body}"
            );
        }
        assert!(
            !carries(case, &body, "securityGroupIdSet"),
            "case {case:?}: the groups live on the interface, not beside it, got {body}"
        );
        assert!(
            !carries(case, &body, "securityGroupSet"),
            "case {case:?}: the groups live on the interface, not beside it, got {body}"
        );
    }
}

/// Rule 3: an id outside the form EC2 issues is malformed, before any lookup; an id inside it
/// that names nothing is not found.
#[tokio::test]
async fn an_unresolvable_instance_id_is_refused_before_any_data_is_read() {
    let (router, image_id) = emulator();
    // One live instance, so the store is not empty when the unknown ids are looked up.
    let known = launch(&router, "the known instance", &image_id, &[]).await;

    // (what is wrong with the id, the id, the code EC2 answers)
    let cases: [(&str, &str, &str); 12] = [
        (
            "the resource prefix of no kind at all",
            "instance-00000000000000001",
            "InvalidInstanceID.Malformed",
        ),
        (
            "no resource prefix",
            "00000000000000001",
            "InvalidInstanceID.Malformed",
        ),
        (
            "one digit short of the long form",
            "i-0123456789abcde",
            "InvalidInstanceID.Malformed",
        ),
        (
            "one digit past the long form",
            "i-0123456789abcdef01",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the 8-digit short form, nothing launched for it",
            "i-0abc1234",
            "InvalidInstanceID.NotFound",
        ),
        (
            "a digit that is not hexadecimal",
            "i-0z23456789abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "upper-case hexadecimal digits",
            "i-0ABCDEF0123456789",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a leading digit above zero",
            "i-1234567890abcdef0",
            "InvalidInstanceID.Malformed",
        ),
        (
            "every digit above zero",
            "i-fffffffffffffffff",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the null id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "a well-formed id nothing was launched for",
            "i-00000000000000999",
            "InvalidInstanceID.NotFound",
        ),
        (
            "another well-formed id nothing was launched for",
            "i-0f0f0f0f0f0f0f0f0",
            "InvalidInstanceID.NotFound",
        ),
    ];

    for (label, instance_id, code) in cases {
        let (status, body) = call(
            &router,
            REGION,
            "GetLaunchTemplateData",
            &[p("InstanceId", instance_id)],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?} ({instance_id}): expected 400 {code}, got {status}: {body}"
        );
    }

    // The id that does resolve still resolves: the refusals above are about the ids, not
    // about the operation.
    let body = launch_template_data(&router, "the known instance", &known).await;
    assert!(
        at("the known instance", &body, "instanceType").is_some(),
        "the launched instance must still be readable, got {body}"
    );
}

/// Rule 4: `DryRun` says whether the call would have succeeded — and a request that would
/// have been refused is refused with that reason instead.
#[tokio::test]
async fn dry_run_reports_whether_the_call_would_have_succeeded() {
    let (router, image_id) = emulator();
    let instance_id = launch(&router, "the dry run instance", &image_id, &[]).await;

    // (what the dry run names, the id, the status and code EC2 answers)
    let cases: [(&str, String, u16, &str); 3] = [
        (
            "an instance the account holds",
            instance_id.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "an id inside the form that names nothing",
            "i-00000000000000999".to_string(),
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "an id outside the form EC2 issues",
            "i-1234567890abcdef0".to_string(),
            400,
            "InvalidInstanceID.Malformed",
        ),
    ];

    for (label, id, expected_status, code) in cases {
        let (status, body) = call(
            &router,
            REGION,
            "GetLaunchTemplateData",
            &[p("InstanceId", id.as_str()), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, code),
            "case {label:?} ({id}): expected {expected_status} {code}, got {status}: {body}"
        );
    }

    // And the same call without the flag is served.
    let body = launch_template_data(&router, "the dry run instance", &instance_id).await;
    assert!(
        at("the dry run instance", &body, "imageId").is_some(),
        "the request the dry run stood for must be served, got {body}"
    );
}

/// Rule 5: the operation is a read — two calls about one settled instance answer the same
/// thing and neither moves it.
#[tokio::test]
async fn reading_the_data_changes_nothing_a_later_caller_sees() {
    let case = "a settled instance";
    let (router, image_id) = emulator();
    let instance_id = launch(
        &router,
        case,
        &image_id,
        &[p("InstanceType", "t3.micro"), p("Monitoring.Enabled", "true")],
    )
    .await;
    settled(&router, case, &instance_id).await;

    let before = state_of(&router, &instance_id).await;
    let first = launch_template_data(&router, case, &instance_id).await;
    let second = launch_template_data(&router, case, &instance_id).await;
    let after = state_of(&router, &instance_id).await;

    let data_of = |body: &str| {
        let parsed = document(case, body);
        let data = child(parsed.root_element(), "launchTemplateData")
            .unwrap_or_else(|| panic!("case {case:?}: no <launchTemplateData> in {body}"));
        data.descendants()
            .filter(roxmltree::Node::is_element)
            .map(|node| {
                (
                    node.tag_name().name().to_string(),
                    node.text().unwrap_or_default().to_string(),
                )
            })
            .collect::<Vec<(String, String)>>()
    };
    assert_eq!(
        data_of(&first),
        data_of(&second),
        "case {case:?}: two reads of one instance must answer the same thing, got {first} and \
         {second}"
    );
    assert_eq!(
        after, before,
        "case {case:?}: reading the launch data must not move the instance"
    );
    assert_eq!(
        after, "running",
        "case {case:?}: the instance was meant to stay running, got {after:?}"
    );
}
