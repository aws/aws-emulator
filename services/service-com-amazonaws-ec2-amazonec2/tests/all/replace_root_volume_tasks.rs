// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the root volume replacement slice:
//! `CreateReplaceRootVolumeTask` and `DescribeReplaceRootVolumeTasks`, driven over the
//! wire the service actually speaks — `ec2Query`, through the crate's shared
//! [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `CreateReplaceRootVolumeTask` MINTS A TASK FROM THE REQUEST and
//!    `DescribeReplaceRootVolumeTasks` REPORTS THE STORE: every member the request sets
//!    round-trips through the describe, and the members the service owns (the task id, the
//!    start time, the state, and the snapshot the replacement volume is created from) are
//!    derived rather than taken;
//! 2. the task is handed back in the non-terminal `pending` state and the CLOCK owes it
//!    `in-progress` and then `succeeded` — "the replacement volume is being created",
//!    "the original volume is being detached and the replacement volume is being
//!    attached", "the replacement volume has been successfully attached"
//!    (`ReplaceRootVolumeTask$TaskState`) — with `CompleteTime` reported only once it has
//!    completed, and the describe writing nothing at all;
//! 3. the four restore forms are MUTUALLY EXCLUSIVE: a request may name a snapshot, an
//!    AMI, a volume, or none of the three (its launch state), never two of them;
//! 4. every resource the request names is RESOLVED before a task is stored — the
//!    instance first, by this operation's own validator, which answers
//!    `InvalidInstanceID.Malformed` for any id it cannot resolve, then the restore
//!    source — and a `VolumeInitializationRate` keeps its published range of 100 - 300
//!    MiB/s;
//! 5. one `ClientToken` names one task;
//! 6. the describe narrows by `ReplaceRootVolumeTaskIds` and by every filter it answers,
//!    and pages by `MaxResults`/`NextToken`;
//! 7. `DryRun` reports whether the call WOULD have succeeded and stores nothing.
//!
//! Each table row is a distinct input along what the rule generalises over — which restore
//! form, how many tags, which way an id falls outside the form EC2 issues, which end of the
//! published rate range, which filter, which page size — so no row is the specification.
//! The rate rows are each published bound, one value inside and one past each bound; the
//! state catalog is the model's own `REPLACE_ROOT_VOLUME_TASK_STATE_VALUES`.

use std::time::Duration;

use ec2::ops::types::REPLACE_ROOT_VOLUME_TASK_STATE_VALUES;
use ec2::state::{
    MAX_TAGS_PER_RESOURCE, MAX_VOLUME_INITIALIZATION_RATE, MIN_VOLUME_INITIALIZATION_RATE,
    REPLACE_ROOT_VOLUME_TASK_STATE_PENDING, REPLACE_ROOT_VOLUME_TASK_STATE_SUCCEEDED,
    TRANSITION_DELAY, availability_zones_in_region,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, error_code, filters, indexed};

/// The Region every call here is signed for. A root volume replacement task carries no
/// Region member, so the Region selects the envelope and the zone catalog the volume rows
/// read.
const REGION: &str = "us-west-2";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of a table that expects a refusal: `(case, the parameters, the error code)`.
type RefusalRow = (String, Params, &'static str);

/// Wait until every clock-owed move started before now has certainly fallen due. Derived
/// from the service's own span, not a literal, so the two cannot drift.
async fn past_one_move() {
    tokio::time::sleep(TRANSITION_DELAY + Duration::from_millis(200)).await;
}

/// Wait until both moves a fresh task is owed — `pending` -> `in-progress` -> `succeeded` —
/// have certainly fallen due.
async fn past_both_moves() {
    tokio::time::sleep(TRANSITION_DELAY * 2 + Duration::from_millis(200)).await;
}

fn emulator() -> MultiServiceRouter {
    super::ec2_query::emulator().0
}

async fn api(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, action, params).await
}

async fn create(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "CreateReplaceRootVolumeTask", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    api(router, "DescribeReplaceRootVolumeTasks", params).await
}

/// A `ReplaceRootVolumeTaskId` is `replacevol-` followed by 8 or 17 lowercase hex digits,
/// the short and the long EC2 id form.
fn is_task_id(candidate: &str) -> bool {
    let Some(suffix) = candidate.strip_prefix("replacevol-") else {
        return false;
    };
    matches!(suffix.len(), 8 | 17)
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// An RFC 3339 UTC timestamp, as the service renders `StartTime` and `CompleteTime`:
/// `YYYY-MM-DDThh:mm:ssZ`.
fn is_iso8601_utc(candidate: &str) -> bool {
    candidate.len() >= 20
        && candidate.ends_with('Z')
        && candidate.as_bytes()[4] == b'-'
        && candidate.as_bytes()[7] == b'-'
        && candidate.as_bytes()[10] == b'T'
        && candidate.as_bytes()[13] == b':'
}

/// One reported `replaceRootVolumeTask`, read out of the XML.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Task {
    id: Option<String>,
    instance_id: Option<String>,
    task_state: Option<String>,
    start_time: Option<String>,
    complete_time: Option<String>,
    image_id: Option<String>,
    snapshot_id: Option<String>,
    delete_replaced_root_volume: Option<String>,
    tags: Vec<(String, String)>,
}

impl Task {
    /// The members no clock-owed move can change — what a round-trip assertion compares.
    fn settled(&self) -> Task {
        Task {
            task_state: None,
            complete_time: None,
            ..self.clone()
        }
    }
}

fn child<'a, 'i>(node: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    node.children()
        .find(|inner| inner.is_element() && inner.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name).map(|inner| inner.text().unwrap_or_default().to_string())
}

fn task_of(node: roxmltree::Node<'_, '_>) -> Task {
    Task {
        id: child_text(node, "replaceRootVolumeTaskId"),
        instance_id: child_text(node, "instanceId"),
        task_state: child_text(node, "taskState"),
        start_time: child_text(node, "startTime"),
        complete_time: child_text(node, "completeTime"),
        image_id: child_text(node, "imageId"),
        snapshot_id: child_text(node, "snapshotId"),
        delete_replaced_root_volume: child_text(node, "deleteReplacedRootVolume"),
        tags: child(node, "tagSet")
            .map(|set| {
                set.children()
                    .filter(roxmltree::Node::is_element)
                    .filter_map(|item| {
                        Some((child_text(item, "key")?, child_text(item, "value")?))
                    })
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The task a create answered with.
fn created_task(body: &str) -> Task {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    let node = child(document.root_element(), "replaceRootVolumeTask").unwrap_or_else(|| {
        panic!("CreateReplaceRootVolumeTask must report the task it started: {body}")
    });
    task_of(node)
}

/// Every task a describe reported, in document order.
fn described_tasks(body: &str) -> Vec<Task> {
    let document = roxmltree::Document::parse(body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    child(document.root_element(), "replaceRootVolumeTaskSet")
        .map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .map(task_of)
                .collect()
        })
        .unwrap_or_default()
}

/// Every task the store holds, by id.
async fn all_task_ids(router: &MultiServiceRouter) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "DescribeReplaceRootVolumeTasks must succeed, got {status}: {body}"
    );
    described_tasks(&body)
        .into_iter()
        .filter_map(|task| task.id)
        .collect()
}

/// One AMI the account can see: its id and the snapshot its ROOT block device mapping
/// names.
#[derive(Debug, Clone)]
struct VisibleImage {
    id: String,
    root_snapshot: Option<String>,
}

/// The AMIs the account can see, read out of `DescribeImages` rather than written here:
/// `ImageId` is resolved against the image store, so a row needs an image that is really
/// in it — and the snapshot a restore from that image uses is a fact of the image, read
/// from the same report.
async fn visible_images(router: &MultiServiceRouter) -> Vec<VisibleImage> {
    let (status, body) = api(router, "DescribeImages", &[]).await;
    assert_eq!(status, 200, "DescribeImages must succeed, got {status}: {body}");
    let document = roxmltree::Document::parse(&body)
        .unwrap_or_else(|error| panic!("the response body must be XML ({error}): {body}"));
    let images = child(document.root_element(), "imagesSet")
        .map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| {
                    let root_device = child_text(item, "rootDeviceName");
                    let root_snapshot = child(item, "blockDeviceMapping")
                        .into_iter()
                        .flat_map(|set| set.children().filter(roxmltree::Node::is_element))
                        .find(|mapping| {
                            root_device.is_none()
                                || child_text(*mapping, "deviceName") == root_device
                        })
                        .and_then(|mapping| child(mapping, "ebs"))
                        .and_then(|ebs| child_text(ebs, "snapshotId"));
                    VisibleImage {
                        id: child_text(item, "imageId").unwrap_or_default(),
                        root_snapshot,
                    }
                })
                .collect::<Vec<VisibleImage>>()
        })
        .unwrap_or_default();
    assert!(
        images.len() >= 2,
        "the account must be able to see at least two AMIs: {body}"
    );
    images
}

/// An AMI id of the long form that no image of the account carries.
const UNKNOWN_AMI: &str = "ami-0123456789abcdef0";

/// Launch one instance from `image_id` and answer its id.
async fn launch_from(router: &MultiServiceRouter, image_id: &str) -> String {
    let (status, body) = api(
        router,
        "RunInstances",
        &[
            p("ImageId", image_id),
            p("MinCount", "1"),
            p("MaxCount", "1"),
        ],
    )
    .await;
    assert_eq!(status, 200, "RunInstances must succeed, got {status}: {body}");
    let instance_id = element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"));
    // "Replaces the EBS-backed root volume for a `running` instance"
    // (`CreateReplaceRootVolumeTask`), and a launch leaves the instance `pending` until the
    // clock carries it on — so every row waits for the state the operation needs rather than
    // for a span of its own.
    wait_until_running(router, &instance_id).await;
    instance_id
}

/// Wait until `DescribeInstances` reports `instance_id` as `running`.
///
/// Polls for the STATE rather than sleeping for the span the clock owes it, so a loaded
/// machine cannot read the instance before the move has been applied; the deadline is
/// generous, because what is under test is the operation rather than how fast the clock runs.
async fn wait_until_running(router: &MultiServiceRouter, instance_id: &str) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(60);
    loop {
        let state = super::ec2_query::described_instance_state(router, REGION, instance_id).await;
        if state == "running" {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "the instance {instance_id} must reach the running state, last read {state:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Launch one instance from an AMI this account holds no image for — the launch path does
/// not resolve the image, so nothing in the store says what its root volume came from.
async fn launch(router: &MultiServiceRouter) -> String {
    launch_from(router, UNKNOWN_AMI).await
}

/// The Availability Zone `DescribeInstances` reports the instance is placed in.
async fn instance_zone(router: &MultiServiceRouter, instance_id: &str) -> String {
    let (status, body) = api(
        router,
        "DescribeInstances",
        &[p("InstanceId.1", instance_id)],
    )
    .await;
    assert_eq!(
        status, 200,
        "DescribeInstances must succeed, got {status}: {body}"
    );
    element(&body, "availabilityZone")
        .unwrap_or_else(|| panic!("the instance must report the zone it is placed in: {body}"))
}

/// Create one `available` volume in `zone` and answer its id.
async fn volume_in(router: &MultiServiceRouter, zone: &str) -> String {
    let (status, body) = api(
        router,
        "CreateVolume",
        &[p("AvailabilityZone", zone), p("Size", "8")],
    )
    .await;
    assert_eq!(status, 200, "CreateVolume must succeed, got {status}: {body}");
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("CreateVolume must report the volume it minted: {body}"));
    // A fresh volume is `creating`; the replacement source must be `available`, which the
    // clock owes it one transition later.
    past_one_move().await;
    volume_id
}

/// Rule 1 and rule 2: whatever restore form a request names, the task is stored and
/// reported from the store, in `pending`, with the members the service owns derived — among
/// them `SnapshotId`, the snapshot the replacement volume is created from.
#[tokio::test]
async fn a_task_is_minted_from_the_request_and_reported_from_the_store() {
    /// Which AMI the row's instance is launched from.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum LaunchedFrom {
        /// An AMI of the account, so the store knows the snapshot its root volume came
        /// from.
        AnImageOfTheAccount,
        /// An AMI the account holds no image for, so the store knows nothing about it.
        AnImageTheAccountDoesNotHold,
    }

    /// Which restore source the row's create names.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RestoreFrom {
        /// None of the three members: the original root volume's launch state.
        TheLaunchState,
        /// `ImageId`, naming an AMI of the account other than the launch one.
        AnotherImage,
        /// `VolumeId`, naming an `available` volume of the instance's zone.
        AVolume,
    }

    /// One create form: the parameters it adds and what the stored task must then carry.
    struct Case {
        label: &'static str,
        launched_from: LaunchedFrom,
        restore_from: RestoreFrom,
        extra: Params,
        expected_delete_flag: &'static str,
        expected_tags: Vec<(&'static str, &'static str)>,
    }

    let cases = vec![
        Case {
            label: "restored to the launch state of an AMI the account holds",
            launched_from: LaunchedFrom::AnImageOfTheAccount,
            restore_from: RestoreFrom::TheLaunchState,
            extra: Vec::new(),
            expected_delete_flag: "false",
            expected_tags: Vec::new(),
        },
        Case {
            label: "restored to the launch state of an AMI the account does not hold",
            launched_from: LaunchedFrom::AnImageTheAccountDoesNotHold,
            restore_from: RestoreFrom::TheLaunchState,
            extra: Vec::new(),
            expected_delete_flag: "false",
            expected_tags: Vec::new(),
        },
        Case {
            label: "restored from another AMI, deleting the replaced volume",
            launched_from: LaunchedFrom::AnImageOfTheAccount,
            restore_from: RestoreFrom::AnotherImage,
            extra: vec![p("DeleteReplacedRootVolume", "true")],
            expected_delete_flag: "true",
            expected_tags: Vec::new(),
        },
        Case {
            label: "replaced by a specified volume, keeping the replaced volume",
            launched_from: LaunchedFrom::AnImageOfTheAccount,
            restore_from: RestoreFrom::AVolume,
            extra: vec![p("DeleteReplacedRootVolume", "false")],
            expected_delete_flag: "false",
            expected_tags: Vec::new(),
        },
        Case {
            label: "restored to the launch state at the slowest published rate",
            launched_from: LaunchedFrom::AnImageTheAccountDoesNotHold,
            restore_from: RestoreFrom::TheLaunchState,
            extra: vec![p(
                "VolumeInitializationRate",
                MIN_VOLUME_INITIALIZATION_RATE.to_string(),
            )],
            expected_delete_flag: "false",
            expected_tags: Vec::new(),
        },
        Case {
            label: "tagged with two tags, naming the task's resource type",
            launched_from: LaunchedFrom::AnImageTheAccountDoesNotHold,
            restore_from: RestoreFrom::TheLaunchState,
            extra: vec![
                p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "root-swap"),
                p("TagSpecification.1.Tag.2.Key", "Team"),
                p("TagSpecification.1.Tag.2.Value", "platform"),
            ],
            expected_delete_flag: "false",
            expected_tags: vec![("Name", "root-swap"), ("Team", "platform")],
        },
        Case {
            label: "tagged with one tag, naming the task's resource type",
            launched_from: LaunchedFrom::AnImageTheAccountDoesNotHold,
            restore_from: RestoreFrom::TheLaunchState,
            extra: vec![
                p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "implicit"),
            ],
            expected_delete_flag: "false",
            expected_tags: vec![("Name", "implicit")],
        },
    ];

    for case in cases {
        let router = emulator();
        let images = visible_images(&router).await;
        let launch_image = match case.launched_from {
            LaunchedFrom::AnImageOfTheAccount => images[0].clone(),
            LaunchedFrom::AnImageTheAccountDoesNotHold => VisibleImage {
                id: UNKNOWN_AMI.to_string(),
                root_snapshot: None,
            },
        };
        let instance_id = launch_from(&router, &launch_image.id).await;
        let zone = instance_zone(&router, &instance_id).await;

        let mut params = vec![p("InstanceId", instance_id.as_str())];
        // The AMI the replacement is restored from, and with it the snapshot the task must
        // report: the one the request named, else the instance's own launch AMI; a volume
        // replacement restores from no snapshot at all.
        let (expected_image, expected_snapshot) = match case.restore_from {
            RestoreFrom::TheLaunchState => (None, launch_image.root_snapshot.clone()),
            RestoreFrom::AnotherImage => {
                let other = images[1].clone();
                params.push(p("ImageId", other.id.as_str()));
                (Some(other.id.clone()), other.root_snapshot.clone())
            }
            RestoreFrom::AVolume => {
                let volume_id = volume_in(&router, &zone).await;
                params.push(p("VolumeId", volume_id.as_str()));
                (None, None)
            }
        };
        params.extend(case.extra.clone());

        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {:?}: CreateReplaceRootVolumeTask must succeed, got {status}: {body}",
            case.label
        );
        let created = created_task(&body);
        let task_id = created
            .id
            .clone()
            .unwrap_or_else(|| panic!("case {:?}: the task must carry an id: {body}", case.label));
        assert!(
            is_task_id(&task_id),
            "case {:?}: expected a replacevol- task id, got {task_id:?}",
            case.label
        );
        assert_eq!(
            created.instance_id.as_deref(),
            Some(instance_id.as_str()),
            "case {:?}: the task must name the instance it was created for, got {:?}",
            case.label,
            created.instance_id
        );
        assert_eq!(
            created.task_state.as_deref(),
            Some(REPLACE_ROOT_VOLUME_TASK_STATE_PENDING),
            "case {:?}: a fresh task is pending, got {:?}",
            case.label,
            created.task_state
        );
        assert_eq!(
            created.complete_time, None,
            "case {:?}: a task that has not completed has no completeTime, got {:?}",
            case.label, created.complete_time
        );
        let start_time = created.start_time.clone().unwrap_or_default();
        assert!(
            is_iso8601_utc(&start_time),
            "case {:?}: expected an ISO 8601 UTC startTime, got {start_time:?}",
            case.label
        );
        assert_eq!(
            created.image_id, expected_image,
            "case {:?}: the task must report the AMI the replacement was restored from, \
             got {:?}",
            case.label, created.image_id
        );
        assert_eq!(
            created.snapshot_id, expected_snapshot,
            "case {:?}: the task must report the snapshot the replacement volume was \
             created from, got {:?}",
            case.label, created.snapshot_id
        );
        assert_eq!(
            created.delete_replaced_root_volume.as_deref(),
            Some(case.expected_delete_flag),
            "case {:?}: expected deleteReplacedRootVolume {:?}, got {:?}",
            case.label,
            case.expected_delete_flag,
            created.delete_replaced_root_volume
        );
        let expected_tags: Vec<(String, String)> = case
            .expected_tags
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        assert_eq!(
            created.tags, expected_tags,
            "case {:?}: expected the tags the request asked for, got {:?}",
            case.label, created.tags
        );

        // The store, not the request object, is what the describe answers from.
        let (describe_status, describe_body) =
            describe(&router, &[p("ReplaceRootVolumeTaskId.1", task_id.as_str())]).await;
        assert_eq!(
            describe_status, 200,
            "case {:?}: DescribeReplaceRootVolumeTasks must succeed, got \
             {describe_status}: {describe_body}",
            case.label
        );
        let reported = described_tasks(&describe_body);
        assert_eq!(
            reported.len(),
            1,
            "case {:?}: exactly the one task must be reported, got {reported:?}",
            case.label
        );
        assert_eq!(
            reported[0].settled(),
            created.settled(),
            "case {:?}: every member the request set must round-trip through the describe",
            case.label
        );
        let state = reported[0].task_state.clone().unwrap_or_default();
        assert!(
            REPLACE_ROOT_VOLUME_TASK_STATE_VALUES.contains(&state.as_str()),
            "case {:?}: the reported state must be a published one, got {state:?}",
            case.label
        );
    }
}

/// Rule 2: the clock walks a task through its states, `CompleteTime` appears only once it
/// has completed, and the describe writes nothing.
#[tokio::test]
async fn the_clock_moves_a_task_to_its_terminal_state() {
    let router = emulator();
    let instance_id = launch(&router).await;

    let (status, body) = create(&router, &[p("InstanceId", instance_id.as_str())]).await;
    assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
    let created = created_task(&body);
    let task_id = created.id.clone().expect("the task must carry an id");
    assert_eq!(
        created.task_state.as_deref(),
        Some(REPLACE_ROOT_VOLUME_TASK_STATE_PENDING),
        "case \"straight after the create\": the task must still be pending, got {:?}",
        created.task_state
    );

    // Past the first move the task has left `pending` for good: which of the two later
    // states it is in depends only on how far the clock has run.
    past_one_move().await;
    let (_, body) = describe(&router, &[p("ReplaceRootVolumeTaskId.1", task_id.as_str())]).await;
    let midway = described_tasks(&body);
    let midway_state = midway
        .first()
        .and_then(|task| task.task_state.clone())
        .unwrap_or_default();
    assert_ne!(
        midway_state, REPLACE_ROOT_VOLUME_TASK_STATE_PENDING,
        "case \"past the first clock-owed move\": the task must have left pending, got \
         {midway_state:?}"
    );

    past_both_moves().await;
    let (_, body) = describe(&router, &[p("ReplaceRootVolumeTaskId.1", task_id.as_str())]).await;
    let finished = described_tasks(&body);
    let task = finished
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("the task must still be reported: {body}"));
    assert_eq!(
        task.task_state.as_deref(),
        Some(REPLACE_ROOT_VOLUME_TASK_STATE_SUCCEEDED),
        "case \"past both clock-owed moves\": expected succeeded, got {:?}",
        task.task_state
    );
    let complete_time = task.complete_time.clone().unwrap_or_default();
    assert!(
        is_iso8601_utc(&complete_time),
        "case \"a completed task\": expected an ISO 8601 UTC completeTime, got \
         {complete_time:?}"
    );
    assert!(
        complete_time >= task.start_time.clone().unwrap_or_default(),
        "case \"a completed task\": the completion cannot precede the start, got {:?} and \
         {complete_time:?}",
        task.start_time
    );

    // The describe is a READ: asked twice at the same point of the task's life it answers
    // the same thing, and it never moves a settled task.
    let (_, repeat_body) =
        describe(&router, &[p("ReplaceRootVolumeTaskId.1", task_id.as_str())]).await;
    assert_eq!(
        described_tasks(&repeat_body),
        finished,
        "case \"two reads of one task\": a read must not change what a later caller sees"
    );
}

/// Rule 3: a request restores from exactly one source, so any two of the three members are
/// refused together — whichever two.
#[tokio::test]
async fn the_restore_sources_are_mutually_exclusive() {
    let router = emulator();
    let instance_id = launch(&router).await;
    let zone = instance_zone(&router, &instance_id).await;
    let image_id = visible_images(&router).await[0].id.clone();
    let volume_id = volume_in(&router, &zone).await;
    let snapshot_id = "snap-0123456789abcdef0".to_string();

    let snapshot = p("SnapshotId", snapshot_id.as_str());
    let image = p("ImageId", image_id.as_str());
    let volume = p("VolumeId", volume_id.as_str());

    let rows: Vec<RefusalRow> = vec![
        (
            "a snapshot and an AMI".to_string(),
            vec![snapshot.clone(), image.clone()],
            "InvalidParameterCombination",
        ),
        (
            "a snapshot and a volume".to_string(),
            vec![snapshot.clone(), volume.clone()],
            "InvalidParameterCombination",
        ),
        (
            "an AMI and a volume".to_string(),
            vec![image.clone(), volume.clone()],
            "InvalidParameterCombination",
        ),
        (
            "all three sources at once".to_string(),
            vec![snapshot.clone(), image.clone(), volume.clone()],
            "InvalidParameterCombination",
        ),
    ];

    for (label, extra, code) in rows {
        let mut params = vec![p("InstanceId", instance_id.as_str())];
        params.extend(extra);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }
    assert!(
        all_task_ids(&router).await.is_empty(),
        "a refused create must store no task"
    );

    // Naming ONE source is not a combination failure: the volume row is served.
    let (status, body) = create(
        &router,
        &[p("InstanceId", instance_id.as_str()), volume.clone()],
    )
    .await;
    assert_eq!(
        status, 200,
        "one restore source must be accepted, got {status}: {body}"
    );
}

/// Rule 4: `VolumeInitializationRate` keeps its published range — "Valid range: 100 - 300
/// MiB/s" — tested on each bound, inside it and one past each bound.
#[tokio::test]
async fn the_volume_initialization_rate_keeps_its_published_range() {
    let router = emulator();
    let instance_id = launch(&router).await;

    let inside = (MIN_VOLUME_INITIALIZATION_RATE + MAX_VOLUME_INITIALIZATION_RATE) / 2;
    // (what the rate is, the rate, the expected status, the expected error code)
    let rows: Vec<(String, i64, u16, &str)> = vec![
        (
            "the bottom of the published range".to_string(),
            MIN_VOLUME_INITIALIZATION_RATE,
            200,
            "",
        ),
        ("inside the published range".to_string(), inside, 200, ""),
        (
            "the top of the published range".to_string(),
            MAX_VOLUME_INITIALIZATION_RATE,
            200,
            "",
        ),
        (
            "one below the bottom".to_string(),
            MIN_VOLUME_INITIALIZATION_RATE - 1,
            400,
            "InvalidParameterValue",
        ),
        (
            "one above the top".to_string(),
            MAX_VOLUME_INITIALIZATION_RATE + 1,
            400,
            "InvalidParameterValue",
        ),
        ("a rate of zero".to_string(), 0, 400, "InvalidParameterValue"),
        (
            "far above the top".to_string(),
            MAX_VOLUME_INITIALIZATION_RATE * 100,
            400,
            "InvalidParameterValue",
        ),
    ];

    for (label, rate, expected_status, code) in rows {
        let (status, body) = create(
            &router,
            &[
                p("InstanceId", instance_id.as_str()),
                p("VolumeInitializationRate", rate.to_string()),
            ],
        )
        .await;
        assert_eq!(
            status, expected_status,
            "case {label:?} (rate {rate}): expected {expected_status}, got {status}: {body}"
        );
        if expected_status != 200 {
            assert_eq!(
                error_code(&body),
                code,
                "case {label:?} (rate {rate}): expected {code}, got {body}"
            );
        }
    }
}

/// Rule 4: the instance is resolved first, then the restore source, and a resource that
/// cannot be resolved means no task is stored.
#[tokio::test]
async fn an_unresolvable_resource_is_refused_before_the_task_is_started() {
    let router = emulator();
    let instance_id = launch(&router).await;
    let zone = instance_zone(&router, &instance_id).await;
    let other_zone = availability_zones_in_region(REGION)
        .into_iter()
        .map(|record| record.zone_name)
        .find(|name| *name != zone)
        .expect("the Region must have more than one zone");
    let elsewhere = volume_in(&router, &other_zone).await;

    let mut rows: Vec<RefusalRow> = Vec::new();
    // Every way an instance id can fail to name an instance of the account: this
    // operation's own validator answers `InvalidInstanceID.Malformed` for all of them,
    // whether or not the value reads like the long id form (see the handler's
    // `require_replaceable_instance` for the recorded evidence).
    for (label, candidate, code) in [
        (
            "the resource prefix of no kind at all",
            "instance-00000000000000001",
            "InvalidInstanceID.Malformed",
        ),
        (
            "one digit short of the long form",
            "i-0123456789abcde",
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
            "the null id",
            "i-00000000000000000",
            "InvalidInstanceID.Malformed",
        ),
        (
            "the long id form, naming no instance of the account",
            "i-00000000000000999",
            "InvalidInstanceID.Malformed",
        ),
        (
            "another long-form id naming no instance of the account",
            "i-0000000000deadbee",
            "InvalidInstanceID.Malformed",
        ),
    ] {
        rows.push((
            format!("an instance id with {label}"),
            vec![p("InstanceId", candidate)],
            code,
        ));
    }
    // The restore source, each kind resolved its own way.
    for (label, params, code) in [
        (
            "an AMI id outside the form EC2 issues",
            vec![p("ImageId", "ami-not-an-id")],
            "InvalidAMIID.Malformed",
        ),
        (
            "a well-formed AMI id the account cannot see",
            vec![p("ImageId", "ami-0fedcba987654321f")],
            "InvalidAMIID.NotFound",
        ),
        (
            "a snapshot id outside the form EC2 issues",
            vec![p("SnapshotId", "snap-nope")],
            "InvalidSnapshotID.Malformed",
        ),
        (
            "a well-formed snapshot id no snapshot exists for",
            vec![p("SnapshotId", "snap-0123456789abcdef0")],
            "InvalidSnapshot.NotFound",
        ),
        (
            "a volume id outside the form EC2 issues",
            vec![p("VolumeId", "vol-nope")],
            "InvalidVolumeID.Malformed",
        ),
        (
            "a well-formed volume id no volume exists for",
            vec![p("VolumeId", "vol-0fedcba987654321f")],
            "InvalidVolume.NotFound",
        ),
        (
            "a volume in another Availability Zone",
            vec![p("VolumeId", elsewhere.as_str())],
            "InvalidParameterValue",
        ),
    ] {
        let mut full = vec![p("InstanceId", instance_id.as_str())];
        full.extend(params);
        rows.push((label.to_string(), full, code));
    }

    for (label, params, code) in rows {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }
    assert!(
        all_task_ids(&router).await.is_empty(),
        "a refused create must store no task"
    );

    // A volume the clock has not yet made `available` is not a replacement either: the
    // member requires the `available` state.
    let (status, body) = api(
        &router,
        "CreateVolume",
        &[p("AvailabilityZone", zone.as_str()), p("Size", "8")],
    )
    .await;
    assert_eq!(status, 200, "CreateVolume must succeed, got {status}: {body}");
    let creating = element(&body, "volumeId").expect("CreateVolume must report the volume");
    let (status, body) = create(
        &router,
        &[
            p("InstanceId", instance_id.as_str()),
            p("VolumeId", creating.as_str()),
        ],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidParameterValue"),
        "case \"a volume that is still being created\": expected 400 \
         InvalidParameterValue, got {status}: {body}"
    );

    // An instance on its way to `terminated` has no root volume left to replace.
    let (status, body) = api(
        &router,
        "TerminateInstances",
        &[p("InstanceId.1", instance_id.as_str())],
    )
    .await;
    assert_eq!(
        status, 200,
        "TerminateInstances must succeed, got {status}: {body}"
    );
    let (status, body) = create(&router, &[p("InstanceId", instance_id.as_str())]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectInstanceState"),
        "case \"an instance on its way to terminated\": expected 400 \
         IncorrectInstanceState, got {status}: {body}"
    );
}

/// Rule 4: the request's tag specifications are read by the crate's one reader of them, so
/// a specification naming another resource type or none at all is refused, and so are a
/// duplicate key, a reserved key and one tag too many.
#[tokio::test]
async fn the_tags_of_a_task_are_the_tags_of_a_task() {
    let router = emulator();
    let instance_id = launch(&router).await;

    for resource_type in ["instance", "volume", "vpc", "launch-template"] {
        let (status, body) = create(
            &router,
            &[
                p("InstanceId", instance_id.as_str()),
                p("TagSpecification.1.ResourceType", resource_type),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nope"),
            ],
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidParameterValue"),
            "case {resource_type:?}: a specification for another resource type must be \
             refused, got {status}: {body}"
        );
    }

    // (what is wrong with the specification, its parameters, the code EC2 answers)
    let rows: Vec<RefusalRow> = vec![
        (
            "a specification that names no resource type".to_string(),
            vec![
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nameless"),
            ],
            "MissingParameter",
        ),
        (
            "one tag key given twice".to_string(),
            vec![
                p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "first"),
                p("TagSpecification.1.Tag.2.Key", "Name"),
                p("TagSpecification.1.Tag.2.Value", "second"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a key in the reserved aws: namespace".to_string(),
            vec![
                p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
                p("TagSpecification.1.Tag.1.Key", "aws:created-by"),
                p("TagSpecification.1.Tag.1.Value", "me"),
            ],
            "InvalidParameterValue",
        ),
    ];
    for (label, extra, code) in rows {
        let mut params = vec![p("InstanceId", instance_id.as_str())];
        params.extend(extra);
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }

    let mut too_many = vec![
        p("InstanceId", instance_id.as_str()),
        p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
    ];
    for nth in 0..=MAX_TAGS_PER_RESOURCE {
        too_many.push(p(
            &format!("TagSpecification.1.Tag.{}.Key", nth + 1),
            format!("k{nth}"),
        ));
        too_many.push(p(&format!("TagSpecification.1.Tag.{}.Value", nth + 1), "v"));
    }
    let (status, body) = create(&router, &too_many).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "TagLimitExceeded"),
        "case \"one tag past the documented maximum\": expected 400 TagLimitExceeded, got \
         {status}: {body}"
    );
    assert!(
        all_task_ids(&router).await.is_empty(),
        "a refused create must store no task"
    );
}

/// Rule 5: one `ClientToken` names one task, however many times the request is replayed.
#[tokio::test]
async fn one_client_token_names_one_task() {
    let router = emulator();
    let instance_id = launch(&router).await;

    let params = vec![
        p("InstanceId", instance_id.as_str()),
        p("ClientToken", "root-swap-once"),
    ];
    let (status, body) = create(&router, &params).await;
    assert_eq!(status, 200, "the first create must succeed, got {status}: {body}");
    let first = created_task(&body).id.expect("the task must carry an id");

    for attempt in 1..=2 {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            status, 200,
            "replay {attempt}: the replay must succeed, got {status}: {body}"
        );
        assert_eq!(
            created_task(&body).id.as_deref(),
            Some(first.as_str()),
            "replay {attempt}: one token names one task, got {body}"
        );
    }
    assert_eq!(
        all_task_ids(&router).await,
        vec![first.clone()],
        "a replay must start no second task"
    );

    // A different token is a different request, so it starts its own task.
    let (status, body) = create(
        &router,
        &[
            p("InstanceId", instance_id.as_str()),
            p("ClientToken", "root-swap-again"),
        ],
    )
    .await;
    assert_eq!(status, 200, "the second create must succeed, got {status}: {body}");
    let second = created_task(&body).id.expect("the task must carry an id");
    assert_ne!(
        second, first,
        "a different client token must name a different task"
    );
}

/// Rule 6: the describe narrows by ids and by every filter it answers, and pages.
#[tokio::test]
async fn the_describe_narrows_by_id_and_filter_and_pages() {
    let router = emulator();
    let first_instance = launch(&router).await;
    let second_instance = launch(&router).await;

    // Three tasks on the first instance and two on the second, tagged so the tag filters
    // have something to select on.
    let mut on_first = Vec::new();
    let mut on_second = Vec::new();
    for (index, instance_id) in [
        &first_instance,
        &first_instance,
        &first_instance,
        &second_instance,
        &second_instance,
    ]
    .into_iter()
    .enumerate()
    {
        let (status, body) = create(
            &router,
            &[
                p("InstanceId", instance_id.as_str()),
                p("ClientToken", format!("task-{index}")),
                p("TagSpecification.1.ResourceType", "replace-root-volume-task"),
                p("TagSpecification.1.Tag.1.Key", "Tier"),
                p(
                    "TagSpecification.1.Tag.1.Value",
                    if index % 2 == 0 { "web" } else { "batch" },
                ),
            ],
        )
        .await;
        assert_eq!(status, 200, "the create must succeed, got {status}: {body}");
        let task_id = created_task(&body).id.expect("the task must carry an id");
        if instance_id == &first_instance {
            on_first.push(task_id);
        } else {
            on_second.push(task_id);
        }
    }
    let mut every_id: Vec<String> = on_first.iter().chain(on_second.iter()).cloned().collect();
    every_id.sort();

    // (what the filter selects on, the filter parameters, the ids it must select)
    let rows: Vec<(String, Params, Vec<String>)> = vec![
        (
            "the first instance".to_string(),
            filters(&[("instance-id", &[first_instance.as_str()])]),
            on_first.clone(),
        ),
        (
            "the second instance".to_string(),
            filters(&[("instance-id", &[second_instance.as_str()])]),
            on_second.clone(),
        ),
        (
            "either instance, two values in one filter".to_string(),
            filters(&[(
                "instance-id",
                &[first_instance.as_str(), second_instance.as_str()],
            )]),
            every_id.clone(),
        ),
        (
            "an instance nothing was created for".to_string(),
            filters(&[("instance-id", &["i-00000000000000999"])]),
            Vec::new(),
        ),
        (
            "a tag value".to_string(),
            filters(&[("tag:Tier", &["web"])]),
            vec![
                on_first[0].clone(),
                on_first[2].clone(),
                on_second[1].clone(),
            ],
        ),
        (
            "the other tag value".to_string(),
            filters(&[("tag:Tier", &["batch"])]),
            vec![on_first[1].clone(), on_second[0].clone()],
        ),
        (
            "a tag key every task carries".to_string(),
            filters(&[("tag-key", &["Tier"])]),
            every_id.clone(),
        ),
        (
            "a tag key no task carries".to_string(),
            filters(&[("tag-key", &["Absent"])]),
            Vec::new(),
        ),
        (
            "an instance and a tag value together".to_string(),
            filters(&[
                ("instance-id", &[first_instance.as_str()]),
                ("tag:Tier", &["batch"]),
            ]),
            vec![on_first[1].clone()],
        ),
    ];

    for (label, params, expected) in rows {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {label:?}: the describe must succeed, got {status}: {body}"
        );
        let mut reported: Vec<String> = described_tasks(&body)
            .into_iter()
            .filter_map(|task| task.id)
            .collect();
        reported.sort();
        let mut wanted = expected.clone();
        wanted.sort();
        assert_eq!(
            reported, wanted,
            "case {label:?}: expected the tasks {wanted:?}, got {reported:?}"
        );
    }

    // Narrowing by id reports exactly those tasks.
    let named: Vec<&str> = on_second.iter().map(String::as_str).collect();
    let (status, body) = describe(&router, &indexed("ReplaceRootVolumeTaskId", &named)).await;
    assert_eq!(status, 200, "the describe must succeed, got {status}: {body}");
    let mut reported: Vec<String> = described_tasks(&body)
        .into_iter()
        .filter_map(|task| task.id)
        .collect();
    reported.sort();
    let mut wanted = on_second.clone();
    wanted.sort();
    assert_eq!(
        reported, wanted,
        "case \"narrowed by id\": expected {wanted:?}, got {reported:?}"
    );

    // Paging: every page size walks the same ids in id order.
    for page_size in [1usize, 2, 5] {
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let mut params = vec![p("MaxResults", page_size.to_string())];
            if let Some(next) = token.clone() {
                params.push(p("NextToken", next));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case \"page size {page_size}\": the describe must succeed, got \
                 {status}: {body}"
            );
            let page: Vec<String> = described_tasks(&body)
                .into_iter()
                .filter_map(|task| task.id)
                .collect();
            assert!(
                page.len() <= page_size,
                "case \"page size {page_size}\": a page must hold at most {page_size} \
                 tasks, got {page:?}"
            );
            seen.extend(page);
            token = element(&body, "nextToken");
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            seen, every_id,
            "case \"page size {page_size}\": paging must report every task once, in id order"
        );
    }

    // (what is wrong with the request, the parameters, the code EC2 answers)
    let refusals: Vec<RefusalRow> = vec![
        (
            "a filter name the operation does not answer".to_string(),
            filters(&[("task-state", &["pending"])]),
            "InvalidParameterValue",
        ),
        (
            "a tag filter with no key".to_string(),
            filters(&[("tag:", &["web"])]),
            "InvalidParameterValue",
        ),
        (
            "a task id outside the form EC2 issues".to_string(),
            vec![p("ReplaceRootVolumeTaskId.1", "replacevol-nope")],
            "InvalidReplaceRootVolumeTaskId.Malformed",
        ),
        (
            "a well-formed task id nothing was created for".to_string(),
            vec![p("ReplaceRootVolumeTaskId.1", "replacevol-0123456789abcdef0")],
            "InvalidReplaceRootVolumeTaskId.NotFound",
        ),
        (
            "a pagination token this service did not mint".to_string(),
            vec![p("NextToken", "not-a-token")],
            "InvalidPaginationToken",
        ),
    ];
    for (label, params, code) in refusals {
        let (status, body) = describe(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, code),
            "case {label:?}: expected 400 {code}, got {status}: {body}"
        );
    }
}

/// Rule 7: `DryRun` says whether the call would have succeeded, and stores nothing.
#[tokio::test]
async fn dry_run_reports_whether_the_call_would_have_succeeded() {
    let router = emulator();
    let instance_id = launch(&router).await;

    // (what the dry run names, the parameters, the status and code EC2 answers)
    let rows: Vec<(String, Params, u16, &str)> = vec![
        (
            "an instance the account holds".to_string(),
            vec![p("InstanceId", instance_id.as_str()), p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
        (
            "an id inside the long form that names nothing".to_string(),
            vec![p("InstanceId", "i-00000000000000999"), p("DryRun", "true")],
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "an id outside the form EC2 issues".to_string(),
            vec![p("InstanceId", "i-1234567890abcdef0"), p("DryRun", "true")],
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            "a rate past the published range".to_string(),
            vec![
                p("InstanceId", instance_id.as_str()),
                p(
                    "VolumeInitializationRate",
                    (MAX_VOLUME_INITIALIZATION_RATE + 1).to_string(),
                ),
                p("DryRun", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
    ];

    for (label, params, expected_status, code) in rows {
        let (status, body) = create(&router, &params).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (expected_status, code),
            "case {label:?}: expected {expected_status} {code}, got {status}: {body}"
        );
    }
    assert!(
        all_task_ids(&router).await.is_empty(),
        "a dry run must start no task"
    );

    // The describe answers the flag the same way, after it has validated the request.
    let (status, body) = describe(&router, &[p("DryRun", "true")]).await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (412, "DryRunOperation"),
        "case \"a dry run of the describe\": expected 412 DryRunOperation, got {status}: {body}"
    );

    // And the request the dry run stood for is served.
    let (status, body) = create(&router, &[p("InstanceId", instance_id.as_str())]).await;
    assert_eq!(
        status, 200,
        "the request the dry run stood for must be served, got {status}: {body}"
    );
}
