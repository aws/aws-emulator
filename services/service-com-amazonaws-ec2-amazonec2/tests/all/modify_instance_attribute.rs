// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ModifyInstanceAttribute`, driven over the wire the service
//! actually speaks: `ec2Query` — an `application/x-www-form-urlencoded` POST whose body
//! carries `Action=ModifyInstanceAttribute`, answered with XML (README.md /
//! OPERATIONS.md "Wire protocol").
//!
//! The rule under test is that the operation WRITES THE ONE ATTRIBUTE THE REQUEST NAMES onto
//! the stored instance, so a later read reports the new value and every other attribute is
//! left as it stands. So the tables below span the attributes the operation documents
//! ([`MODIFIABLE_INSTANCE_ATTRIBUTES`]) rather than one of them, each read back through an
//! implemented reader — `DescribeInstanceAttribute` for the attributes it reports, and
//! `DescribeInstances` for the one it reports no value for — and the rows differ along the
//! value, the form of the request (the typed member and the legacy `Attribute`/`Value` pair),
//! the instance state the attribute needs and the record the value lands on (the instance,
//! its primary network interface, the volume it holds).
//!
//! Four suites:
//!
//! 1. the catalog table — one row per attribute, with the value a read must report after;
//! 2. the protection table — termination protection is what a `TerminateInstances` is
//!    refused by, and turning it off again is what lets the terminate through, asked in both
//!    request forms;
//! 3. the group and block-device suites — the two attributes whose value lives on a record
//!    beside the instance, read back off that record;
//! 4. the refusal table — the requests it refuses, and the `DryRun` probe.

use std::collections::BTreeSet;

use ec2::ops::modify_instance_attribute::{
    INSTANCE_ATTRIBUTE_BLOCK_DEVICE_MAPPING, INSTANCE_ATTRIBUTE_DISABLE_API_STOP,
    INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION, INSTANCE_ATTRIBUTE_EBS_OPTIMIZED,
    INSTANCE_ATTRIBUTE_ENA_SUPPORT, INSTANCE_ATTRIBUTE_ENCLAVE_OPTIONS,
    INSTANCE_ATTRIBUTE_GROUP_SET, INSTANCE_ATTRIBUTE_INSTANCE_TYPE,
    INSTANCE_ATTRIBUTE_SHUTDOWN_BEHAVIOR, INSTANCE_ATTRIBUTE_SRIOV_NET_SUPPORT,
    INSTANCE_ATTRIBUTE_USER_DATA, MODIFIABLE_INSTANCE_ATTRIBUTES,
};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DEFAULT_INSTANCE_TYPE,
    IMAGE_SRIOV_NET_SUPPORT_SIMPLE, INSTANCE_ATTRIBUTE_KERNEL, INSTANCE_ATTRIBUTE_RAMDISK,
    INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK, ImageSpec, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, error_code, error_message};
use super::wire::{child, child_text, document, set_items};

/// The Region every call here is signed for.
const REGION: &str = "us-west-2";

/// An AMI id of the form EC2 issues that this store holds no image for — so the instances
/// launched from it carry the facts of the EBS-backed Linux image this crate publishes.
const AMI: &str = "ami-0123456789abcdef0";

/// An instance type the published catalog holds, different from the type a launch that named
/// none runs ([`DEFAULT_INSTANCE_TYPE`]).
const INSTANCE_TYPE: &str = "m5.large";

/// A second published instance type, so the row that varies the value does not reuse the
/// first.
const OTHER_INSTANCE_TYPE: &str = "c5.xlarge";

/// An instance type the published catalog does not hold, which is what makes a modify of
/// `instanceType` answer `InvalidInstanceAttributeValue`.
const UNPUBLISHED_INSTANCE_TYPE: &str = "zz99.nonesuch";

/// A kernel id a request may name; AWS kernel images are `aki-` ids.
const KERNEL: &str = "aki-0123456789abcdef0";

/// A RAM disk id a request may name; AWS RAM disk images are `ari-` ids.
const RAMDISK: &str = "ari-0123456789abcdef0";

/// User data as the base64 the `UserData` blob is carried as — 21 bytes, so the encoding is
/// padding-free and the text a read reports is this one byte for byte.
const USER_DATA: &str = "IyEvYmluL3NoCmVjaG8gaGVsbG8K";

/// The `ShutdownBehavior` member a request asks for: the one that is not the documented
/// default.
const SHUTDOWN_BEHAVIOR_TERMINATE: &str = "terminate";

/// One row of a table that drives a request: the fields the call carries beside the
/// instance id, as the call helpers want them.
type Fields = Vec<(String, String)>;

/// One row of the refusal table: what the case is, the instance the request names, the
/// fields beside it, and the status and code it must be refused with. Named so the table's
/// own type is a name rather than a nest of tuples.
type Refusal = (&'static str, String, Fields, u16, &'static str);

/// One `(key, value)` request parameter.
fn p(key: &str, value: &str) -> (String, String) {
    (key.to_string(), value.to_string())
}

/// One `ModifyInstanceAttribute` call for `instance_id` with `params` beside its id.
async fn modify(
    router: &MultiServiceRouter,
    instance_id: &str,
    params: &[(String, String)],
) -> (u16, String) {
    let mut all = vec![p("InstanceId", instance_id)];
    all.extend_from_slice(params);
    call(router, REGION, "ModifyInstanceAttribute", &all).await
}

/// Launch one instance from `image_id` with `params` beside it, and answer its id.
async fn launch(
    case: &str,
    router: &MultiServiceRouter,
    image_id: &str,
    params: &[(String, String)],
) -> String {
    let mut all = vec![
        p("ImageId", image_id),
        p("MinCount", "1"),
        p("MaxCount", "1"),
    ];
    all.extend_from_slice(params);
    let (status, body) = call(router, REGION, "RunInstances", &all).await;
    assert_eq!(
        status, 200,
        "case {case:?}: RunInstances must succeed, got {status} with body {body}"
    );
    super::ec2_query::element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: RunInstances reported no instance: {body}"))
}

/// Look again until `instance_id` reports the state `wanted`, or a generous deadline passes.
///
/// The condition is looked at AGAIN in a loop rather than waited out, so nothing here depends
/// on how fast the host runs; the deadline is a multiple of the service's own transition span,
/// far enough out that a loaded host is never the reason it fails.
async fn poll_until_state(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    wanted: &str,
) {
    let deadline = std::time::Instant::now() + TRANSITION_DELAY * 40;
    loop {
        let state = super::ec2_query::described_instance_state(router, REGION, instance_id).await;
        if state == wanted {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the instance {instance_id} must reach {wanted:?}, last read \
             {state:?}"
        );
        tokio::time::sleep(TRANSITION_DELAY / 10).await;
    }
}

/// Stop `instance_id` and wait until it reports `stopped`.
async fn stop(case: &str, router: &MultiServiceRouter, instance_id: &str) {
    let (status, body) = call(
        router,
        REGION,
        "StopInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: StopInstances must succeed, got {status} with body {body}"
    );
    poll_until_state(case, router, instance_id, "stopped").await;
}

/// One `DescribeInstanceAttribute` body for `instance_id` and `attribute`.
async fn describe_attribute(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    attribute: &str,
) -> String {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstanceAttribute",
        &[p("InstanceId", instance_id), p("Attribute", attribute)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: the {attribute} read must succeed, got {status} with body {body}"
    );
    body
}

/// The `<attribute><value>` text `DescribeInstanceAttribute` reports for one attribute — the
/// empty string for an attribute the instance holds no value for.
async fn reported_value(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    attribute: &str,
) -> String {
    let body = describe_attribute(case, router, instance_id, attribute).await;
    let parsed = document(case, &body);
    let element = child(parsed.root_element(), attribute).unwrap_or_else(|| {
        panic!("case {case:?}: the envelope carries no <{attribute}>: {body}")
    });
    child_text(element, "value").unwrap_or_default()
}

/// The `child` text of every item of the `attribute` set `DescribeInstanceAttribute` reports.
async fn reported_items(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    attribute: &str,
    name: &str,
) -> Vec<String> {
    let body = describe_attribute(case, router, instance_id, attribute).await;
    let parsed = document(case, &body);
    assert!(
        child(parsed.root_element(), attribute).is_some(),
        "case {case:?}: the envelope carries no <{attribute}>: {body}"
    );
    set_items(parsed.root_element(), attribute)
        .into_iter()
        .filter_map(|item| child_text(item, name))
        .collect()
}

/// The one instance `DescribeInstances` reports for `instance_id`, as a `(member, text)`
/// reader: the member read is the one the attribute reads cannot answer.
async fn described_member(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    name: &str,
) -> Option<String> {
    let (status, body) = call(
        router,
        REGION,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: DescribeInstances must succeed, got {status} with body {body}"
    );
    let parsed = document(case, &body);
    let instance = parsed
        .descendants()
        .find(|node| {
            node.is_element()
                && child_text(*node, "instanceId").as_deref() == Some(instance_id)
                && child(*node, "instanceState").is_some()
        })
        .unwrap_or_else(|| panic!("case {case:?}: no instance {instance_id} in {body}"));
    child_text(instance, name)
}

/// The `deleteOnTermination` the instance's own block device mapping reports for `device`:
/// the attachment's member, which a mapping entry carries inside its `<ebs>`.
async fn reported_delete_on_termination(
    case: &str,
    router: &MultiServiceRouter,
    instance_id: &str,
    device: &str,
) -> Vec<String> {
    let body = describe_attribute(
        case,
        router,
        instance_id,
        INSTANCE_ATTRIBUTE_BLOCK_DEVICE_MAPPING,
    )
    .await;
    let parsed = document(case, &body);
    set_items(
        parsed.root_element(),
        INSTANCE_ATTRIBUTE_BLOCK_DEVICE_MAPPING,
    )
    .into_iter()
    .filter(|item| child_text(*item, "deviceName").as_deref() == Some(device))
    .filter_map(|item| {
        child(item, "ebs").and_then(|ebs| child_text(ebs, "deleteOnTermination"))
    })
    .collect()
}

/// Which of the instances the catalog table launches a row modifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Which {
    /// A `stopped` instance launched from an AMI this store does not hold.
    Stopped,
    /// A `stopped` instance launched from a stored AMI that carries no `sriovNetSupport` of
    /// its own, so a modify of that attribute is told apart from the value an instance
    /// inherits.
    StoppedWithoutEnhancedNetworking,
    /// A `running` instance launched from an AMI this store does not hold.
    Running,
}

/// Where the new value has to show up.
#[derive(Debug)]
enum Read {
    /// `DescribeInstanceAttribute`'s `<attribute><value>` text.
    Value(String),
    /// `DescribeInstanceAttribute`'s `<enclaveOptions><enabled>` text.
    Enabled(bool),
    /// A member of the instance `DescribeInstances` reports, for the one attribute
    /// `DescribeInstanceAttribute` reports no value for.
    InstanceMember {
        /// The member element, as the model's own `xmlName` spells it.
        name: &'static str,
        /// The text it has to carry.
        value: String,
    },
}

/// One row of the catalog table.
struct Row {
    /// What this row is a case of, named in every assertion of it.
    case: &'static str,
    /// The instance it modifies.
    instance: Which,
    /// The attribute it names, which is also the element the read looks at.
    attribute: &'static str,
    /// The request fields beside the instance id.
    params: Vec<(String, String)>,
    /// What a read must report afterwards.
    expected: Read,
}

/// The instances the catalog table drives.
struct Launched {
    stopped: String,
    stopped_without_enhanced_networking: String,
    running: String,
}

impl Launched {
    /// The id of the instance a row names.
    fn id(&self, which: Which) -> &str {
        match which {
            Which::Stopped => &self.stopped,
            Which::StoppedWithoutEnhancedNetworking => {
                &self.stopped_without_enhanced_networking
            }
            Which::Running => &self.running,
        }
    }
}

/// Launch the three instances the catalog table modifies and park each in the state its rows
/// need: both `stopped` instances are stopped in one pass, so the suite waits for the
/// service's own span once rather than once per row.
async fn launched(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) -> Launched {
    // `Image$SriovNetSupport` carries no value for an image enhanced networking was never
    // enabled on, and every image this store publishes carries one, so the image is seeded
    // through the store's own writer the way the describe suite seeds its images.
    let without_sriov = {
        let store = engine.state::<Ec2State>();
        let mut guard = store.lock();
        let image_id = guard.insert_image(ImageSpec {
            name: "modify-instance-attribute-without-enhanced-networking".to_string(),
            ..ImageSpec::default()
        });
        if let Some(image) = guard.images.get_mut(&image_id) {
            image.sriov_net_support = None;
        }
        image_id
    };

    let stopped = launch("the stopped launch", router, AMI, &[]).await;
    let stopped_without_enhanced_networking = launch(
        "the launch without enhanced networking",
        router,
        &without_sriov,
        &[],
    )
    .await;
    let running = launch("the running launch", router, AMI, &[]).await;

    poll_until_state("the running launch", router, &running, "running").await;
    stop("the stopped launch", router, &stopped).await;
    stop(
        "the launch without enhanced networking",
        router,
        &stopped_without_enhanced_networking,
    )
    .await;

    Launched {
        stopped,
        stopped_without_enhanced_networking,
        running,
    }
}

// --- the catalog table ------------------------------------------------------

/// Rule: a modify writes exactly the attribute the request named onto the instance, and a
/// read reports the new value.
///
/// One row per attribute the operation documents, differing along the value, the form of the
/// request and the record the value lands on. The two attributes whose value lives BESIDE the
/// instance — its security groups and the delete-on-termination of the volumes it holds — have
/// suites of their own below, and the coverage assertion names them.
#[tokio::test]
async fn a_modify_writes_the_named_attribute_and_a_read_reports_it() {
    let (router, engine) = super::ec2_query::emulator();
    let instances = launched(&router, &engine).await;

    let rows: Vec<Row> = vec![
        Row {
            case: "the instance type of a stopped instance",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_INSTANCE_TYPE,
            params: vec![p("InstanceType.Value", INSTANCE_TYPE)],
            expected: Read::Value(INSTANCE_TYPE.to_string()),
        },
        Row {
            case: "the instance type again, to another published type",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_INSTANCE_TYPE,
            params: vec![p("InstanceType.Value", OTHER_INSTANCE_TYPE)],
            expected: Read::Value(OTHER_INSTANCE_TYPE.to_string()),
        },
        Row {
            case: "the kernel, named through the legacy attribute and value pair",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_KERNEL,
            params: vec![
                p("Attribute", INSTANCE_ATTRIBUTE_KERNEL),
                p("Value", KERNEL),
            ],
            expected: Read::Value(KERNEL.to_string()),
        },
        Row {
            case: "the RAM disk, named through its own member",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_RAMDISK,
            params: vec![p("Ramdisk.Value", RAMDISK)],
            expected: Read::Value(RAMDISK.to_string()),
        },
        Row {
            case: "the user data, carried as the blob its member is",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_USER_DATA,
            params: vec![p("UserData.Value", USER_DATA)],
            expected: Read::Value(USER_DATA.to_string()),
        },
        Row {
            case: "EBS optimization turned on",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_EBS_OPTIMIZED,
            params: vec![p("EbsOptimized.Value", "true")],
            expected: Read::Value("true".to_string()),
        },
        Row {
            case: "Nitro Enclaves turned on",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_ENCLAVE_OPTIONS,
            params: vec![p("EnclaveOptions.Enabled", "true")],
            expected: Read::Enabled(true),
        },
        Row {
            // "Note that the enaSupport attribute is not supported"
            // (`DescribeInstanceAttributeRequest$Attribute`), so the instance report is the
            // read AWS leaves this one to.
            case: "enhanced networking with ENA turned off",
            instance: Which::Stopped,
            attribute: INSTANCE_ATTRIBUTE_ENA_SUPPORT,
            params: vec![p("EnaSupport.Value", "false")],
            expected: Read::InstanceMember {
                name: "enaSupport",
                value: "false".to_string(),
            },
        },
        Row {
            case: "enhanced networking on an instance whose AMI carries none",
            instance: Which::StoppedWithoutEnhancedNetworking,
            attribute: INSTANCE_ATTRIBUTE_SRIOV_NET_SUPPORT,
            params: vec![p("SriovNetSupport.Value", IMAGE_SRIOV_NET_SUPPORT_SIMPLE)],
            expected: Read::Value(IMAGE_SRIOV_NET_SUPPORT_SIMPLE.to_string()),
        },
        Row {
            case: "termination protection turned on while the instance runs",
            instance: Which::Running,
            attribute: INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION,
            params: vec![p("DisableApiTermination.Value", "true")],
            expected: Read::Value("true".to_string()),
        },
        Row {
            case: "termination protection turned off again",
            instance: Which::Running,
            attribute: INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION,
            params: vec![p("DisableApiTermination.Value", "false")],
            expected: Read::Value("false".to_string()),
        },
        Row {
            case: "stop protection turned on while the instance runs",
            instance: Which::Running,
            attribute: INSTANCE_ATTRIBUTE_DISABLE_API_STOP,
            params: vec![p("DisableApiStop.Value", "true")],
            expected: Read::Value("true".to_string()),
        },
        Row {
            case: "the instance-initiated shutdown behaviour",
            instance: Which::Running,
            attribute: INSTANCE_ATTRIBUTE_SHUTDOWN_BEHAVIOR,
            params: vec![p(
                "InstanceInitiatedShutdownBehavior.Value",
                SHUTDOWN_BEHAVIOR_TERMINATE,
            )],
            expected: Read::Value(SHUTDOWN_BEHAVIOR_TERMINATE.to_string()),
        },
        Row {
            case: "source/destination checking turned off while the instance runs",
            instance: Which::Running,
            attribute: INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
            params: vec![p("SourceDestCheck.Value", "false")],
            expected: Read::Value("false".to_string()),
        },
    ];

    // The table spans the attributes the operation documents: the two whose value lives on a
    // record beside the instance have suites of their own.
    let covered: BTreeSet<&str> = rows
        .iter()
        .map(|row| row.attribute)
        .chain([
            INSTANCE_ATTRIBUTE_GROUP_SET,
            INSTANCE_ATTRIBUTE_BLOCK_DEVICE_MAPPING,
        ])
        .collect();
    let documented: BTreeSet<&str> = MODIFIABLE_INSTANCE_ATTRIBUTES.iter().copied().collect();
    assert_eq!(
        covered, documented,
        "every modifiable attribute must be covered, missing {:?}",
        documented.difference(&covered).collect::<Vec<_>>()
    );

    for row in &rows {
        let instance_id = instances.id(row.instance);
        let (status, body) = modify(&router, instance_id, &row.params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the modify must succeed, got {status} with body {body}",
            case = row.case
        );
        match &row.expected {
            Read::Value(expected) => {
                let reported = reported_value(row.case, &router, instance_id, row.attribute).await;
                assert_eq!(
                    &reported, expected,
                    "case {case:?}: {attribute} must read back as {expected:?}, got \
                     {reported:?}",
                    case = row.case,
                    attribute = row.attribute
                );
            }
            Read::Enabled(expected) => {
                let body = describe_attribute(row.case, &router, instance_id, row.attribute).await;
                let parsed = document(row.case, &body);
                let element = child(parsed.root_element(), row.attribute).unwrap_or_else(|| {
                    panic!(
                        "case {case:?}: the envelope carries no <{attribute}>: {body}",
                        case = row.case,
                        attribute = row.attribute
                    )
                });
                let reported = child_text(element, "enabled");
                assert_eq!(
                    reported.as_deref(),
                    Some(expected.to_string().as_str()),
                    "case {case:?}: {attribute} must read back as enabled={expected}, got \
                     {reported:?}",
                    case = row.case,
                    attribute = row.attribute
                );
            }
            Read::InstanceMember { name, value } => {
                let reported = described_member(row.case, &router, instance_id, name).await;
                assert_eq!(
                    reported.as_deref(),
                    Some(value.as_str()),
                    "case {case:?}: the instance's {name} must read back as {value:?}, got \
                     {reported:?}",
                    case = row.case
                );
            }
        }
    }

    // Every OTHER attribute is left as it stands: no row modifies the running instance's
    // type or the stopped instance's source/destination checking, so both still report the
    // value their launch left.
    let running_type = reported_value(
        "the untouched instance type",
        &router,
        &instances.running,
        INSTANCE_ATTRIBUTE_INSTANCE_TYPE,
    )
    .await;
    assert_eq!(
        running_type, DEFAULT_INSTANCE_TYPE,
        "a modify of other attributes must leave the instance type alone, got {running_type:?}"
    );
    let stopped_check = reported_value(
        "the untouched source/destination check",
        &router,
        &instances.stopped,
        INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK,
    )
    .await;
    assert_eq!(
        stopped_check, "true",
        "a modify of other attributes must leave source/destination checking alone, got \
         {stopped_check:?}"
    );
}

// --- termination protection -------------------------------------------------

/// Rule: the `disableApiTermination` a modify writes is what a `TerminateInstances` of that
/// instance is then refused by, and writing it off again is what lets the terminate through.
///
/// The two rows differ in the FORM of the request that writes it: the typed member and the
/// legacy `Attribute`/`Value` pair.
#[tokio::test]
async fn termination_protection_a_modify_wrote_refuses_a_terminate() {
    let (router, _engine) = super::ec2_query::emulator();
    // (case, the fields that turn protection on, the fields that turn it off)
    let rows: [(&str, Fields, Fields); 2] = [
        (
            "protection written through the typed member",
            vec![p("DisableApiTermination.Value", "true")],
            vec![p("DisableApiTermination.Value", "false")],
        ),
        (
            "protection written through the legacy attribute and value pair",
            vec![
                p("Attribute", INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION),
                p("Value", "true"),
            ],
            vec![
                p("Attribute", INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION),
                p("Value", "false"),
            ],
        ),
    ];

    for (case, protect, unprotect) in &rows {
        let instance_id = launch(case, &router, AMI, &[]).await;
        poll_until_state(case, &router, &instance_id, "running").await;

        let (status, body) = modify(&router, &instance_id, protect).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the modify must succeed, got {status} with body {body}"
        );

        let (status, body) = call(
            &router,
            REGION,
            "TerminateInstances",
            &[p("InstanceId.1", &instance_id)],
        )
        .await;
        assert_eq!(
            status, 400,
            "case {case:?}: a protected instance must not be terminated, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "OperationNotPermitted",
            "case {case:?}: the refusal must be OperationNotPermitted, got body {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message,
            format!(
                "The instance '{instance_id}' may not be terminated. Modify its \
                 '{INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION}' instance attribute and try \
                 again."
            ),
            "case {case:?}: the refusal must name the instance and the attribute to modify, \
             got {message:?}"
        );
        let state =
            super::ec2_query::described_instance_state(&router, REGION, &instance_id).await;
        assert_eq!(
            state, "running",
            "case {case:?}: the refused terminate must leave the instance running, got \
             {state:?}"
        );

        let (status, body) = modify(&router, &instance_id, unprotect).await;
        assert_eq!(
            status, 200,
            "case {case:?}: turning protection off must succeed, got {status} with body {body}"
        );
        let (status, body) = call(
            &router,
            REGION,
            "TerminateInstances",
            &[p("InstanceId.1", &instance_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: an unprotected instance must terminate, got {status} with body \
             {body}"
        );
    }
}

// --- the records beside the instance ----------------------------------------

/// Rule: the security groups a modify writes replace the instance's set, and every reader of
/// that set — the attribute read, the instance report and the interface that carries them —
/// reports the new one.
///
/// The rows differ along the NUMBER of groups the request names, one group being the smallest
/// set the member admits.
#[tokio::test]
async fn a_modify_replaces_the_security_groups_of_the_instance() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the security groups of a running instance";
    let instance_id = launch(case, &router, AMI, &[]).await;
    poll_until_state(case, &router, &instance_id, "running").await;
    let vpc_id = described_member(case, &router, &instance_id, "vpcId")
        .await
        .unwrap_or_else(|| panic!("case {case:?}: the launched instance must report a VPC"));

    let mut created: Vec<String> = Vec::new();
    for index in 0..2 {
        let (status, body) = call(
            &router,
            REGION,
            "CreateSecurityGroup",
            &[
                p("GroupName", &format!("modify-instance-attribute-{index}")),
                p("GroupDescription", "A group a modify moves the instance into"),
                p("VpcId", &vpc_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: CreateSecurityGroup must succeed, got {status} with body {body}"
        );
        created.push(
            super::ec2_query::element(&body, "groupId")
                .unwrap_or_else(|| panic!("case {case:?}: no group id in {body}")),
        );
    }

    // (case, the groups the request names)
    let rows: [(&str, Vec<String>); 2] = [
        ("one group, the smallest set the member admits", vec![created[0].clone()]),
        ("two groups, replacing the set the first row left", created.clone()),
    ];
    for (row, groups) in &rows {
        let params: Vec<(String, String)> = groups
            .iter()
            .enumerate()
            .map(|(index, group_id)| p(&format!("GroupId.{}", index + 1), group_id))
            .collect();
        let (status, body) = modify(&router, &instance_id, &params).await;
        assert_eq!(
            status, 200,
            "case {row:?}: the modify must succeed, got {status} with body {body}"
        );

        let mut reported = reported_items(
            row,
            &router,
            &instance_id,
            INSTANCE_ATTRIBUTE_GROUP_SET,
            "groupId",
        )
        .await;
        reported.sort();
        let mut expected = groups.clone();
        expected.sort();
        assert_eq!(
            reported, expected,
            "case {row:?}: the attribute read must report the new set, got {reported:?}"
        );

        // The groups live on the interface the instance's launch attached, so the interface
        // report is where a write that only touched the instance record would show up as a
        // disagreement.
        let interface_id =
            super::ec2_query::primary_network_interface(&router, REGION, row, &instance_id).await;
        let (status, body) = call(
            &router,
            REGION,
            "DescribeNetworkInterfaces",
            &[p("NetworkInterfaceId.1", &interface_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: DescribeNetworkInterfaces must succeed, got {status} with body \
             {body}"
        );
        let parsed = document(row, &body);
        let interface = parsed
            .descendants()
            .find(|node| {
                node.is_element()
                    && child_text(*node, "networkInterfaceId").as_deref() == Some(&interface_id)
            })
            .unwrap_or_else(|| panic!("case {row:?}: no interface {interface_id} in {body}"));
        let mut on_interface: Vec<String> = set_items(interface, "groupSet")
            .into_iter()
            .filter_map(|item| child_text(item, "groupId"))
            .collect();
        on_interface.sort();
        assert_eq!(
            on_interface, expected,
            "case {row:?}: the interface must carry the new set, got {on_interface:?}"
        );
    }
}

/// Rule: the `DeleteOnTermination` a modify writes for a device lands on the attachment the
/// volume store holds, so the attribute read and `DescribeVolumes` report the same value.
///
/// The rows differ along the value, including the entry that states none — "if no value is
/// specified for DeleteOnTermination, the default is true".
#[tokio::test]
async fn a_modify_writes_delete_on_termination_onto_the_attachment() {
    let (router, _engine) = super::ec2_query::emulator();
    let case = "the block device mapping of a launched instance";
    let instance_id = launch(case, &router, AMI, &[]).await;
    poll_until_state(case, &router, &instance_id, "running").await;

    // The volumes of a launch reach the instance as that launch completes, so the device is
    // read off the instance rather than written down here.
    let devices = reported_items(
        case,
        &router,
        &instance_id,
        INSTANCE_ATTRIBUTE_BLOCK_DEVICE_MAPPING,
        "deviceName",
    )
    .await;
    let device = devices
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the launch must give the instance a volume"))
        .clone();

    // (case, the fields the entry carries beside the device, the value a read must report)
    let rows: [(&str, Fields, &str); 3] = [
        (
            "delete on termination turned off",
            vec![p("BlockDeviceMapping.1.Ebs.DeleteOnTermination", "false")],
            "false",
        ),
        (
            "delete on termination turned on again",
            vec![p("BlockDeviceMapping.1.Ebs.DeleteOnTermination", "true")],
            "true",
        ),
        (
            "an entry that states no value at all, which defaults to true",
            Vec::new(),
            "true",
        ),
    ];
    for (row, fields, expected) in &rows {
        // Every row first parks the attachment at the other value, so a row that changed
        // nothing could not pass.
        let park = if *expected == "true" { "false" } else { "true" };
        let (status, body) = modify(
            &router,
            &instance_id,
            &[
                p("BlockDeviceMapping.1.DeviceName", &device),
                p("BlockDeviceMapping.1.Ebs.DeleteOnTermination", park),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: parking the attachment must succeed, got {status} with body {body}"
        );

        let mut params = vec![p("BlockDeviceMapping.1.DeviceName", &device)];
        params.extend_from_slice(fields);
        let (status, body) = modify(&router, &instance_id, &params).await;
        assert_eq!(
            status, 200,
            "case {row:?}: the modify must succeed, got {status} with body {body}"
        );

        let reported = reported_delete_on_termination(row, &router, &instance_id, &device).await;
        assert_eq!(
            reported,
            vec![(*expected).to_string()],
            "case {row:?}: the attribute read must report deleteOnTermination={expected}, got \
             {reported:?}"
        );

        // The attachment lives on the VOLUME, so the volume report is where a write that
        // only touched the instance record would show up as a disagreement.
        let (status, body) = call(
            &router,
            REGION,
            "DescribeVolumes",
            &super::ec2_query::filters(&[("attachment.instance-id", &[instance_id.as_str()])]),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {row:?}: DescribeVolumes must succeed, got {status} with body {body}"
        );
        let parsed = document(row, &body);
        let on_volume: Vec<String> = parsed
            .descendants()
            .filter(|node| {
                node.is_element() && child_text(*node, "device").as_deref() == Some(device.as_str())
            })
            .filter_map(|attachment| child_text(attachment, "deleteOnTermination"))
            .collect();
        assert_eq!(
            on_volume,
            vec![(*expected).to_string()],
            "case {row:?}: the volume's attachment must carry deleteOnTermination={expected}, \
             got {on_volume:?}"
        );
    }
}

// --- the refusal table ------------------------------------------------------

/// Rule: a request this operation cannot act on is refused with the code of the class it
/// falls in, and nothing is written.
///
/// The rows span the classes: no attribute, two attributes, an attribute outside the
/// modifiable set, a value beside an attribute that takes none, an unreadable and an absent
/// instance id, an instance state that does not admit the modification, a value outside its
/// own catalog, a group this account does not hold, and the `DryRun` probe.
#[tokio::test]
async fn the_requests_it_refuses() {
    let (router, _engine) = super::ec2_query::emulator();
    let running = launch("the running launch", &router, AMI, &[]).await;
    poll_until_state("the running launch", &router, &running, "running").await;
    let stopped = launch("the stopped launch", &router, AMI, &[]).await;
    stop("the stopped launch", &router, &stopped).await;

    // (case, the instance the request names, the fields beside it, status, code)
    let rows: Vec<Refusal> = vec![
        (
            "a request that names no attribute at all",
            running.clone(),
            Vec::new(),
            400,
            "MissingParameter",
        ),
        (
            "a request that names two attributes",
            running.clone(),
            vec![
                p("SourceDestCheck.Value", "false"),
                p("DisableApiStop.Value", "true"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an attribute of an instance that is a fact of its AMI",
            running.clone(),
            vec![p("Attribute", "rootDeviceName"), p("Value", "/dev/sda1")],
            400,
            "InvalidParameterValue",
        ),
        (
            "an attribute no value may be given for",
            running.clone(),
            vec![
                p("Attribute", INSTANCE_ATTRIBUTE_SOURCE_DEST_CHECK),
                p("Value", "false"),
            ],
            400,
            "InvalidParameterCombination",
        ),
        (
            "an attribute named with nothing to set it to",
            running.clone(),
            vec![p("Attribute", INSTANCE_ATTRIBUTE_DISABLE_API_TERMINATION)],
            400,
            "MissingParameter",
        ),
        (
            "a value that is no instance id at all",
            "not-an-instance".to_string(),
            vec![p("DisableApiStop.Value", "true")],
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "an instance id this account holds no instance for",
            "i-1a2b3c4d".to_string(),
            vec![p("DisableApiStop.Value", "true")],
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "an attribute that needs a stopped instance, asked of a running one",
            running.clone(),
            vec![p("InstanceType.Value", INSTANCE_TYPE)],
            400,
            "IncorrectInstanceState",
        ),
        (
            "an instance type the catalog does not publish",
            stopped.clone(),
            vec![p("InstanceType.Value", UNPUBLISHED_INSTANCE_TYPE)],
            400,
            "InvalidInstanceAttributeValue",
        ),
        (
            "a shutdown behaviour outside its own catalog",
            running.clone(),
            vec![p("InstanceInitiatedShutdownBehavior.Value", "hibernate")],
            400,
            "InvalidParameterValue",
        ),
        (
            "enhanced networking asked for a word that is not the one it takes",
            stopped.clone(),
            vec![p("SriovNetSupport.Value", "advanced")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a security group this account does not hold",
            running.clone(),
            vec![p("GroupId.1", "sg-0123456789abcdef0")],
            400,
            "InvalidGroup.NotFound",
        ),
        (
            "a device the instance holds no volume at",
            running.clone(),
            vec![
                p("BlockDeviceMapping.1.DeviceName", "/dev/nosuchdevice"),
                p("BlockDeviceMapping.1.Ebs.DeleteOnTermination", "false"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run of a request that would have succeeded",
            running.clone(),
            vec![p("DisableApiStop.Value", "true"), p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];

    for (case, instance_id, params, status, code) in &rows {
        let (answered, body) = modify(&router, instance_id, params).await;
        assert_eq!(
            answered, *status,
            "case {case:?}: expected HTTP {status}, got {answered} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            *code,
            "case {case:?}: expected the code {code}, got body {body}"
        );
    }

    // The dry run above would have turned stop protection on, so the probe writing nothing is
    // what the instance still reporting the launch's value says.
    let protection = reported_value(
        "the dry run wrote nothing",
        &router,
        &running,
        INSTANCE_ATTRIBUTE_DISABLE_API_STOP,
    )
    .await;
    assert_eq!(
        protection, "false",
        "a dry run must write nothing, got stop protection {protection:?}"
    );
}
