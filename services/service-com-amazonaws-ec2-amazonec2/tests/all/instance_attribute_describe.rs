// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DescribeInstanceAttribute`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=DescribeInstanceAttribute`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation REPORTS ONE ATTRIBUTE OF THE STORED INSTANCE:
//! the response carries the single member `Attribute` names, read off the state the launch
//! wrote and the AMI, instance type, volume store and interface store it is derived from,
//! beside the instance's own id — and nothing else. So the tables below span the
//! `InstanceAttributeName` catalog rather than one attribute, and each attribute is asked
//! of instances that differ along what it reads: a launch that configured it and one that
//! configured nothing, an AMI this store holds and one it does not, an image that carries a
//! product code and one that carries none.
//!
//! Three suites:
//!
//! 1. the catalog table — every attribute the model publishes, on four instances, with the
//!    value the store holds for it, and the read asserted to change nothing;
//! 2. the agreement table — the value this operation reports for an attribute is the value
//!    `DescribeInstances` reports for the member of the same instance, so an attribute read
//!    cannot drift from an instance read;
//! 3. the refusal table — the instance ids and the attributes it refuses, and the `DryRun`
//!    probe.
//!
//! The images the launches name are seeded through the store's own writer
//! (`ComAmazonawsEc2Amazonec2State::insert_image`, what a register operation calls), because
//! `RegisterImage` and `CreateImage` are still scaffolds with their own stub issues, and the
//! one instance whose source/destination checking is OFF is written the same way:
//! `ModifyInstanceAttribute` is still a scaffold, so no call can turn it off.

use std::collections::BTreeSet;

use ec2::ops::types::{INSTANCE_ATTRIBUTE_NAME_VALUES, ProductCode};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_DISABLE_API_STOP, DEFAULT_INSTANCE_TYPE,
    DEFAULT_SHUTDOWN_BEHAVIOR, DEFAULT_SOURCE_DEST_CHECK, IMAGE_SRIOV_NET_SUPPORT_SIMPLE,
    INSTANCE_ID_PREFIX, INSTANCE_IDS_PARAMETER, ImageSpec, LINUX_ROOT_DEVICE_NAME,
    TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the form EC2 issues that this store holds no image for — what a launch
/// naming an AMI the image catalogue does not carry runs from.
const UNSTORED_AMI: &str = "ami-0123456789abcdef0";

/// The instance type the configured launch names, so its `instanceType` attribute is not
/// the type a launch that named none runs ([`DEFAULT_INSTANCE_TYPE`]).
const CONFIGURED_INSTANCE_TYPE: &str = "m5.large";

/// The kernel the configured launch names (`RunInstancesRequest$KernelId`); AWS kernel
/// images are `aki-` ids.
const KERNEL: &str = "aki-0123456789abcdef0";

/// The RAM disk that launch names (`RunInstancesRequest$RamdiskId`).
const RAMDISK: &str = "ari-0123456789abcdef0";

/// The user data that launch supplies: base64, which is the form `UserData` carries.
const USER_DATA: &str = "IyEvYmluL3NoCmVjaG8gaGVsbG8K";

/// The root device name the configured launch's own AMI declares (`Image$RootDeviceName`),
/// different from the one every default Linux image of this store carries
/// ([`LINUX_ROOT_DEVICE_NAME`]), so the `rootDeviceName` attribute is read off the AMI
/// rather than from one fixed word.
const CONFIGURED_ROOT_DEVICE_NAME: &str = "/dev/sda1";

/// A Marketplace product code an AMI may carry (`Image$ProductCodes`), in the 25-character
/// form AWS issues.
const PRODUCT_CODE: &str = "aw0evgkw8e5c1q413zjy5pjce";

/// Its type: the `ProductCodeValues` catalog's `marketplace` member
/// (`ec2::ops::types::PRODUCT_CODE_VALUES_VALUES`).
const PRODUCT_CODE_TYPE: &str = "marketplace";

/// The `ShutdownBehavior` member the configured launch asks for — the one that is not the
/// documented default ([`DEFAULT_SHUTDOWN_BEHAVIOR`]).
const SHUTDOWN_BEHAVIOR_TERMINATE: &str = "terminate";

/// One `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `DescribeInstanceAttribute` call for `instance_id` and `attribute`.
async fn describe(
    router: &MultiServiceRouter,
    instance_id: &str,
    attribute: &str,
    extra: &[(String, String)],
) -> (u16, String) {
    let mut params = vec![p("InstanceId", instance_id), p("Attribute", attribute)];
    params.extend_from_slice(extra);
    call(router, REGION, "DescribeInstanceAttribute", &params).await
}

/// Look again until the instance `instance_id` reports the block device mapping its launch
/// gave it, or a generous deadline passes.
///
/// "Any block device mapping entries for the instance" are the volumes the instance HOLDS,
/// and the volumes of a launch are attached as part of that launch completing — so an
/// instance still `pending` reports none and the mapping rows below ask about one whose
/// launch has completed. The condition is looked at AGAIN in a loop rather than waited out,
/// so nothing here depends on how fast the host runs; the deadline is a multiple of the
/// service's own transition span, far enough out that a loaded host is never the reason it
/// fails.
async fn past_the_launch(case: &str, router: &MultiServiceRouter, instance_id: &str) {
    let deadline = std::time::Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let (status, body) = describe(router, instance_id, "blockDeviceMapping", &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the mapping read must succeed, got {status} with body {body}"
        );
        if !set_texts(case, &body, "blockDeviceMapping", "deviceName").is_empty() {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the launch's own volumes must reach the instance, got body {body}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
}

/// Launch one instance with `params` beside `ImageId`, and answer its id.
async fn launch(
    case: &str,
    router: &MultiServiceRouter,
    image_id: &str,
    params: Vec<(String, String)>,
) -> String {
    let mut all = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    all.extend(params);
    let (status, body) = call(router, REGION, "RunInstances", &all).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance: {body}"))
}

/// The instances every table below asks about.
struct Launched {
    /// A launch that configured every attribute a launch can set, from an AMI this store
    /// holds: a Marketplace image with its own root device name.
    configured: String,
    /// A launch that configured nothing, from an AMI this store holds no image for.
    plain: String,
    /// A launch from an image whose `sriovNetSupport` is not set, so the instance has no
    /// enhanced networking value to report.
    without_enhanced_networking: String,
    /// The [`Launched::plain`] instance with its stored source/destination checking turned
    /// OFF, so `sourceDestCheck` is asked of both values rather than only the default.
    unchecked: String,
}

/// Seed the images, launch the four instances and turn one instance's source/destination
/// checking off.
async fn launched(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Launched {
    let (marketplace, without_sriov) = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let marketplace = guard.insert_image(ImageSpec {
            name: "instance-attribute-marketplace".to_string(),
            description: "A paid image with its own root device".to_string(),
            root_device_name: CONFIGURED_ROOT_DEVICE_NAME.to_string(),
            product_codes: vec![ProductCode {
                product_code_id: Some(PRODUCT_CODE.to_string()),
                product_code_type: Some(PRODUCT_CODE_TYPE.to_string()),
            }],
            ..ImageSpec::default()
        });
        let without_sriov = guard.insert_image(ImageSpec {
            name: "instance-attribute-without-enhanced-networking".to_string(),
            ..ImageSpec::default()
        });
        // `Image$SriovNetSupport` carries no value for an image enhanced networking was
        // never enabled on; every image this store publishes carries one, and
        // `ModifyImageAttribute` is still a scaffold, so this is the only way to reach that
        // image.
        if let Some(image) = guard.images.get_mut(&without_sriov) {
            image.sriov_net_support = None;
        }
        (marketplace, without_sriov)
    };

    let configured = launch(
        "the configured launch",
        router,
        &marketplace,
        vec![
            p("InstanceType", CONFIGURED_INSTANCE_TYPE),
            p("KernelId", KERNEL),
            p("RamdiskId", RAMDISK),
            p("UserData", USER_DATA),
            p("EbsOptimized", "true"),
            p("DisableApiTermination", "true"),
            p("DisableApiStop", "true"),
            p(
                "InstanceInitiatedShutdownBehavior",
                SHUTDOWN_BEHAVIOR_TERMINATE,
            ),
            p("EnclaveOptions.Enabled", "true"),
        ],
    )
    .await;
    let plain = launch("the plain launch", router, UNSTORED_AMI, Vec::new()).await;
    let without_enhanced_networking = launch(
        "the launch without enhanced networking",
        router,
        &without_sriov,
        Vec::new(),
    )
    .await;
    let unchecked = launch("the unchecked launch", router, UNSTORED_AMI, Vec::new()).await;
    {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let instance = guard
            .instances
            .get_mut(&unchecked)
            .unwrap_or_else(|| panic!("the store must hold the launched instance {unchecked}"));
        instance.source_dest_check = false;
    }

    Launched {
        configured,
        plain,
        without_enhanced_networking,
        unchecked,
    }
}

/// Every element of the response envelope other than the request id, as (element name, the
/// texts its leaf elements carry in document order).
///
/// Read with the XML parser rather than by substring, because the rule under test is about
/// WHICH members the envelope carries: a leaf element with no text reports an attribute the
/// instance holds no value for, and that is a different answer from the element being
/// absent.
fn reported(case: &str, body: &str) -> Vec<(String, Vec<String>)> {
    document(case, body)
        .root_element()
        .children()
        .filter(|node| node.is_element() && node.tag_name().name() != "requestId")
        .map(|node| {
            let texts = node
                .descendants()
                .filter(|inner| {
                    inner.is_element() && !inner.children().any(|child| child.is_element())
                })
                .map(|leaf| leaf.text().unwrap_or_default().to_string())
                .collect();
            (node.tag_name().name().to_string(), texts)
        })
        .collect()
}

/// The texts the envelope's `name` element carries, or `None` when it carries no such
/// element at all.
fn attribute_texts(case: &str, body: &str, name: &str) -> Option<Vec<String>> {
    reported(case, body)
        .into_iter()
        .find(|(element, _)| element == name)
        .map(|(_, texts)| texts)
}

/// The element names the envelope carries, the instance id apart — which must be exactly
/// the one attribute the request asked about ("You can specify only one attribute at a
/// time").
fn attribute_elements(case: &str, body: &str) -> Vec<String> {
    reported(case, body)
        .into_iter()
        .map(|(element, _)| element)
        .filter(|element| element != "instanceId")
        .collect()
}

/// The `child` text of every item of the envelope's `element` set, in document order.
fn set_texts(case: &str, body: &str, element: &str, name: &str) -> Vec<String> {
    let parsed = document(case, body);
    let Some(set) = child(parsed.root_element(), element) else {
        panic!("case {case:?}: the envelope carries no <{element}>: {body}");
    };
    set.children()
        .filter(roxmltree::Node::is_element)
        .filter_map(|item| child_text(item, name))
        .collect()
}

/// What one attribute of one instance must be reported as.
enum Expected {
    /// `<element><value>…</value></element>` — the empty string for an attribute the
    /// instance holds no value for, which is reported as the element carrying no value.
    Value(String),
    /// A set member, as the `child` text of each of its items.
    Items {
        /// The child element each item is read by.
        child: &'static str,
        /// The texts those children must carry, in document order.
        values: Vec<String>,
    },
    /// `<enclaveOptions><enabled>…</enabled></enclaveOptions>`.
    Enabled(bool),
    /// Nothing but the instance's own id.
    Nothing,
}

/// One row of the catalog table.
struct Row {
    /// What this row is a case of, named in every assertion of it.
    case: String,
    /// The instance the request names.
    instance_id: String,
    /// The `Attribute` it asks for.
    attribute: &'static str,
    /// The element the attribute is reported in — the model's own `xmlName`
    /// (`smithy_query(query="operation",operation="DescribeInstanceAttribute")`).
    element: &'static str,
    /// What must come back for it.
    expected: Expected,
}

/// A row carrying a scalar `<value>`.
fn value(
    case: &str,
    instance_id: &str,
    attribute: &'static str,
    element: &'static str,
    expected: &str,
) -> Row {
    Row {
        case: case.to_string(),
        instance_id: instance_id.to_string(),
        attribute,
        element,
        expected: Expected::Value(expected.to_string()),
    }
}

/// Rule 1: for every attribute of the `InstanceAttributeName` catalog, the response carries
/// THAT member and no other, the instance's own id, and the value the store holds for it.
#[tokio::test]
async fn reports_the_one_named_attribute_out_of_the_stored_instance() {
    let (router, engine) = super::ec2_query::emulator();
    let instances = launched(&router, &engine).await;
    // The block device mapping rows below ask about the volumes each launch gave its
    // instance, which reach it as that launch completes.
    past_the_launch("the configured launch", &router, &instances.configured).await;
    past_the_launch("the plain launch", &router, &instances.plain).await;

    // The groups a launch that named none is given: its VPC's default group, carried by the
    // instance's primary network interface. Read off `DescribeInstances`, so the expectation
    // is the groups the instance report itself carries rather than an id written here.
    let plain_groups = described_group_ids(&router, &instances.plain).await;
    assert!(
        !plain_groups.is_empty(),
        "a launch that named no security group must still report the default group it was \
         given, got {plain_groups:?}"
    );

    // Rows: every value the model publishes for `InstanceAttributeName`, each asked of an
    // instance that carries a value for it AND — where a launch can leave it unset — of one
    // that does not, so no row is the specification.
    let rows: Vec<Row> = vec![
        value(
            "the instance type of a launch that named one",
            &instances.configured,
            "instanceType",
            "instanceType",
            CONFIGURED_INSTANCE_TYPE,
        ),
        value(
            "the instance type of a launch that named none",
            &instances.plain,
            "instanceType",
            "instanceType",
            DEFAULT_INSTANCE_TYPE,
        ),
        value(
            "the kernel of a launch that named one",
            &instances.configured,
            "kernel",
            "kernel",
            KERNEL,
        ),
        value(
            "the kernel of a launch that named none, from an AMI carrying none",
            &instances.plain,
            "kernel",
            "kernel",
            "",
        ),
        value(
            "the RAM disk of a launch that named one",
            &instances.configured,
            "ramdisk",
            "ramdisk",
            RAMDISK,
        ),
        value(
            "the RAM disk of a launch that named none",
            &instances.plain,
            "ramdisk",
            "ramdisk",
            "",
        ),
        value(
            "the user data of a launch that supplied some",
            &instances.configured,
            "userData",
            "userData",
            USER_DATA,
        ),
        value(
            "the user data of a launch that supplied none",
            &instances.plain,
            "userData",
            "userData",
            "",
        ),
        value(
            "termination protection asked for by the launch",
            &instances.configured,
            "disableApiTermination",
            "disableApiTermination",
            "true",
        ),
        value(
            "termination protection nothing asked for",
            &instances.plain,
            "disableApiTermination",
            "disableApiTermination",
            "false",
        ),
        value(
            "stop protection asked for by the launch",
            &instances.configured,
            "disableApiStop",
            "disableApiStop",
            "true",
        ),
        value(
            "stop protection nothing asked for",
            &instances.plain,
            "disableApiStop",
            "disableApiStop",
            &DEFAULT_DISABLE_API_STOP.to_string(),
        ),
        value(
            "the shutdown behaviour the launch chose",
            &instances.configured,
            "instanceInitiatedShutdownBehavior",
            "instanceInitiatedShutdownBehavior",
            SHUTDOWN_BEHAVIOR_TERMINATE,
        ),
        value(
            "the shutdown behaviour of a launch that chose none",
            &instances.plain,
            "instanceInitiatedShutdownBehavior",
            "instanceInitiatedShutdownBehavior",
            DEFAULT_SHUTDOWN_BEHAVIOR,
        ),
        value(
            "source/destination checking at its default",
            &instances.plain,
            "sourceDestCheck",
            "sourceDestCheck",
            &DEFAULT_SOURCE_DEST_CHECK.to_string(),
        ),
        value(
            "source/destination checking turned off",
            &instances.unchecked,
            "sourceDestCheck",
            "sourceDestCheck",
            "false",
        ),
        value(
            "EBS optimization asked for by the launch",
            &instances.configured,
            "ebsOptimized",
            "ebsOptimized",
            "true",
        ),
        value(
            "EBS optimization nothing asked for",
            &instances.plain,
            "ebsOptimized",
            "ebsOptimized",
            "false",
        ),
        value(
            "the root device name of an AMI that declares its own",
            &instances.configured,
            "rootDeviceName",
            "rootDeviceName",
            CONFIGURED_ROOT_DEVICE_NAME,
        ),
        value(
            "the root device name of an instance whose AMI this store does not hold",
            &instances.plain,
            "rootDeviceName",
            "rootDeviceName",
            LINUX_ROOT_DEVICE_NAME,
        ),
        value(
            "the enhanced networking of an instance whose AMI carries it",
            &instances.plain,
            "sriovNetSupport",
            "sriovNetSupport",
            IMAGE_SRIOV_NET_SUPPORT_SIMPLE,
        ),
        value(
            "the enhanced networking of an instance whose AMI carries none",
            &instances.without_enhanced_networking,
            "sriovNetSupport",
            "sriovNetSupport",
            "",
        ),
        Row {
            case: "the Nitro Enclaves option a launch enabled".to_string(),
            instance_id: instances.configured.clone(),
            attribute: "enclaveOptions",
            element: "enclaveOptions",
            expected: Expected::Enabled(true),
        },
        Row {
            case: "the Nitro Enclaves option a launch left alone".to_string(),
            instance_id: instances.plain.clone(),
            attribute: "enclaveOptions",
            element: "enclaveOptions",
            expected: Expected::Enabled(false),
        },
        Row {
            case: "the block device mapping of an AMI with its own root device".to_string(),
            instance_id: instances.configured.clone(),
            attribute: "blockDeviceMapping",
            element: "blockDeviceMapping",
            expected: Expected::Items {
                child: "deviceName",
                values: vec![CONFIGURED_ROOT_DEVICE_NAME.to_string()],
            },
        },
        Row {
            case: "the block device mapping of an instance whose AMI this store does not hold"
                .to_string(),
            instance_id: instances.plain.clone(),
            attribute: "blockDeviceMapping",
            element: "blockDeviceMapping",
            expected: Expected::Items {
                child: "deviceName",
                values: vec![LINUX_ROOT_DEVICE_NAME.to_string()],
            },
        },
        Row {
            case: "the product codes of an instance launched from a paid AMI".to_string(),
            instance_id: instances.configured.clone(),
            attribute: "productCodes",
            element: "productCodes",
            expected: Expected::Items {
                child: "productCode",
                values: vec![PRODUCT_CODE.to_string()],
            },
        },
        Row {
            case: "the product codes of an instance launched from an AMI nobody sells".to_string(),
            instance_id: instances.plain.clone(),
            attribute: "productCodes",
            element: "productCodes",
            expected: Expected::Items {
                child: "productCode",
                values: Vec::new(),
            },
        },
        Row {
            case: "the security groups a launch that named none was given".to_string(),
            instance_id: instances.plain.clone(),
            attribute: "groupSet",
            element: "groupSet",
            expected: Expected::Items {
                child: "groupId",
                values: plain_groups.clone(),
            },
        },
        Row {
            // "Note that the `enaSupport` attribute is not supported"
            // (`DescribeInstanceAttributeRequest$Attribute`): the one catalog value this
            // operation reports no value for.
            case: "the unsupported ENA attribute of a configured launch".to_string(),
            instance_id: instances.configured.clone(),
            attribute: "enaSupport",
            element: "enaSupport",
            expected: Expected::Nothing,
        },
        Row {
            case: "the unsupported ENA attribute of a plain launch".to_string(),
            instance_id: instances.plain.clone(),
            attribute: "enaSupport",
            element: "enaSupport",
            expected: Expected::Nothing,
        },
    ];

    // The table spans the catalog the model publishes, so no value of it is left untested.
    let covered: BTreeSet<&str> = rows.iter().map(|row| row.attribute).collect();
    let published: BTreeSet<&str> = INSTANCE_ATTRIBUTE_NAME_VALUES.iter().copied().collect();
    assert_eq!(
        covered, published,
        "every published InstanceAttributeName must appear in the table, missing {:?}",
        published.difference(&covered).collect::<Vec<_>>()
    );

    for row in rows {
        let case = &row.case;
        let (status, body) = describe(&router, &row.instance_id, row.attribute, &[]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            attribute_texts(case, &body, "instanceId"),
            Some(vec![row.instance_id.clone()]),
            "case {case:?}: expected the instance's own id, got body {body}"
        );
        match &row.expected {
            Expected::Nothing => assert!(
                attribute_elements(case, &body).is_empty(),
                "case {case:?}: expected the instance id alone, got {:?} in {body}",
                attribute_elements(case, &body)
            ),
            _ => assert_eq!(
                attribute_elements(case, &body),
                vec![row.element.to_string()],
                "case {case:?}: expected <{element}> as the only attribute of the envelope, \
                 got body {body}",
                element = row.element
            ),
        }
        match &row.expected {
            Expected::Value(expected) => assert_eq!(
                attribute_texts(case, &body, row.element),
                Some(vec![expected.clone()]),
                "case {case:?}: expected the value {expected:?}, got body {body}"
            ),
            Expected::Items { child, values } => assert_eq!(
                &set_texts(case, &body, row.element, child),
                values,
                "case {case:?}: expected the <{child}> entries {values:?}, got body {body}"
            ),
            Expected::Enabled(expected) => assert_eq!(
                attribute_texts(case, &body, row.element),
                Some(vec![expected.to_string()]),
                "case {case:?}: expected enabled={expected}, got body {body}"
            ),
            Expected::Nothing => {}
        }

        // A read: asking twice answers the same, so reporting an attribute changed nothing a
        // later caller sees.
        let (again_status, again_body) = describe(&router, &row.instance_id, row.attribute, &[]).await;
        assert_eq!(
            again_status, 200,
            "case {case:?}: the second read must succeed too, got {again_status} with body \
             {again_body}"
        );
        assert_eq!(
            reported(case, &again_body),
            reported(case, &body),
            "case {case:?}: two reads of one attribute must report the same, got {again_body}"
        );
    }
}

/// The one instance `DescribeInstances` reports for `instance_id`, as a document to read
/// members off.
async fn described(case: &str, router: &MultiServiceRouter, instance_id: &str) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances for {instance_id} must succeed, got {status} with \
         body {body}"
    );
    body
}

/// The text of one member of the instance `DescribeInstances` reports, or `None` when the
/// report leaves it out.
fn described_member(case: &str, body: &str, name: &str) -> Option<String> {
    let parsed = document(case, body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    child_text(instance, name)
}

/// The security group ids `DescribeInstances` reports for one instance.
async fn described_group_ids(router: &MultiServiceRouter, instance_id: &str) -> Vec<String> {
    let case = format!("the groups of {instance_id}");
    let body = described(&case, router, instance_id).await;
    let parsed = document(&case, &body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child(*node, "instanceId").is_some()
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance in {body}"));
    child(instance, "groupSet")
        .map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .filter_map(|item| child_text(item, "groupId"))
                .collect()
        })
        .unwrap_or_default()
}

/// Rule 2: the value reported for an attribute is the value `DescribeInstances` reports for
/// the member of the same instance, so an attribute read cannot drift from an instance read.
#[tokio::test]
async fn agrees_with_the_instance_describe_about_every_shared_member() {
    let (router, engine) = super::ec2_query::emulator();
    let instances = launched(&router, &engine).await;

    // (attribute, the `Instance` member of the same fact). Both names are the model's own
    // `xmlName`s: `InstanceAttribute`'s on the left, `Instance`'s on the right
    // (`smithy_query(query="shape",shape="Instance")`).
    let shared: [(&str, &str); 6] = [
        ("instanceType", "instanceType"),
        ("kernel", "kernelId"),
        ("ramdisk", "ramdiskId"),
        ("sourceDestCheck", "sourceDestCheck"),
        ("ebsOptimized", "ebsOptimized"),
        ("rootDeviceName", "rootDeviceName"),
    ];

    for instance_id in [
        &instances.configured,
        &instances.plain,
        &instances.unchecked,
        &instances.without_enhanced_networking,
    ] {
        let body = described("the instance report", &router, instance_id).await;
        for (attribute, member) in shared {
            let case = format!("{attribute} of {instance_id}");
            // An instance that holds no value for a member has that member left OUT of the
            // instance report, where the attribute report carries the element with no value
            // — so an absent member and an empty attribute are the same answer.
            let from_instance = described_member(&case, &body, member).unwrap_or_default();
            let (status, attribute_body) = describe(&router, instance_id, attribute, &[]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {attribute_body}"
            );
            let from_attribute = attribute_texts(&case, &attribute_body, attribute)
                .unwrap_or_default()
                .join("");
            assert_eq!(
                from_attribute, from_instance,
                "case {case:?}: the attribute reported {from_attribute:?} where the instance \
                 report says {from_instance:?}"
            );
        }
    }
}

/// One refusal row: the request, and the status, code and message it must be refused with.
struct Refusal {
    /// What this row is a case of.
    case: &'static str,
    /// The `InstanceId` the request carries.
    instance_id: String,
    /// The `Attribute` it asks for.
    attribute: &'static str,
    /// Any further request members — the `DryRun` flag, for the probe rows.
    extra: Vec<(String, String)>,
    /// The HTTP status of the refusal.
    status: u16,
    /// Its `<Code>`.
    code: &'static str,
    /// Its `<Message>`, built from the request the row sends.
    message: String,
}

/// Rule 3: what it refuses — an instance id it cannot resolve, and the `DryRun` probe.
#[tokio::test]
async fn refuses_the_ids_it_cannot_resolve_and_answers_the_dry_run_probe() {
    let (router, engine) = super::ec2_query::emulator();
    let instances = launched(&router, &engine).await;

    // The FORMS a value that is no instance id takes — an id of another kind,
    // non-hexadecimal digits, too few digits, too many, uppercase digits and free text — are
    // all refused at the PARAMETER layer, naming the parameter and the form it expected,
    // there being nothing to look up; a well-formed id the account holds no instance for is
    // the not-found half.
    let mut rows: Vec<Refusal> = vec![
        Refusal {
            case: "a well-formed id the account holds no instance for",
            instance_id: "i-0000000000000beef".to_string(),
            attribute: "instanceType",
            extra: Vec::new(),
            status: 400,
            code: "InvalidInstanceID.NotFound",
            message: "The instance ID 'i-0000000000000beef' does not exist".to_string(),
        },
        Refusal {
            case: "a dry run of a request that would have succeeded",
            instance_id: instances.plain.clone(),
            attribute: "userData",
            extra: vec![p("DryRun", "true")],
            status: 412,
            code: "DryRunOperation",
            message: "Request would have succeeded, but DryRun flag is set.".to_string(),
        },
        Refusal {
            case: "a dry run of a request that would have been refused",
            instance_id: "i-0000000000000beef".to_string(),
            attribute: "instanceType",
            extra: vec![p("DryRun", "true")],
            status: 400,
            code: "InvalidInstanceID.NotFound",
            message: "The instance ID 'i-0000000000000beef' does not exist".to_string(),
        },
    ];
    for malformed in [
        "vol-0123456789abcdef0",
        "i-zzzz",
        "i-0123456",
        "i-0123456789ABCDEF0",
        "not-an-instance",
    ] {
        rows.push(Refusal {
            case: "a value that is not an instance id at all",
            instance_id: malformed.to_string(),
            attribute: "instanceType",
            extra: Vec::new(),
            status: 400,
            code: "InvalidParameterValue",
            // The sentence EC2's parameter layer refuses an id it cannot read with, built
            // from the parameter that layer names and the prefix the form expects rather
            // than written out here.
            message: format!(
                "Value ({malformed}) for parameter {INSTANCE_IDS_PARAMETER} is invalid. \
                 Expected: '{INSTANCE_ID_PREFIX}-...'."
            ),
        });
    }

    for row in rows {
        let case = row.case;
        let (status, body) = describe(&router, &row.instance_id, row.attribute, &row.extra).await;
        assert_eq!(
            status, row.status,
            "case {case:?} ({id}): expected HTTP {expected}, got {status} with body {body}",
            id = row.instance_id,
            expected = row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?} ({id}): expected the code {code:?}, got body {body}",
            id = row.instance_id,
            code = row.code
        );
        assert_eq!(
            error_message(&body),
            row.message,
            "case {case:?} ({id}): expected the message {message:?}, got body {body}",
            id = row.instance_id,
            message = row.message
        );
    }

    // Nothing above wrote: the launched instance still reports the user data its launch
    // supplied, and the dry run left it alone.
    let (status, body) = describe(&router, &instances.configured, "userData", &[]).await;
    assert_eq!(
        status, 200,
        "the stored instance must still report its user data, got {status} with body {body}"
    );
    assert_eq!(
        attribute_texts("after the refusals", &body, "userData"),
        Some(vec![USER_DATA.to_string()]),
        "the refusals must leave the stored instance alone, got body {body}"
    );
}

/// Rule 3b: an `Attribute` outside the modelled `InstanceAttributeName` catalog never
/// reaches the handler — the request walk refuses it first, with the validator's own code
/// and a message naming the member and the value the request sent.
#[tokio::test]
async fn an_attribute_outside_the_modelled_catalog_is_refused() {
    let (router, engine) = super::ec2_query::emulator();
    let instances = launched(&router, &engine).await;

    // Rows: a misspelling of a catalog value (the enum is case-sensitive), a name the model
    // declares for no attribute of an instance, and the empty value.
    for (case, attribute) in [
        ("a catalog value in the wrong case", "instancetype"),
        ("an attribute of another resource", "launchPermission"),
        ("no attribute at all", ""),
    ] {
        let (status, body) = describe(&router, &instances.plain, attribute, &[]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "ValidationError",
            "case {case:?}: expected the request walk's own code, got body {body}"
        );
        let message = error_message(&body);
        assert!(
            message.contains(&format!("'{attribute}'")) && message.contains("'attribute'"),
            "case {case:?}: expected a message naming the member and the value \
             {attribute:?}, got {message:?}"
        );
    }
}
