// SPDX-License-Identifier: CC0-1.0

//! The event every change of an AMI's state is published as, end to end over the wire, and
//! the AMI `CreateImage` makes of an instance.
//!
//! THE RULE: an AMI created from an instance is assembled asynchronously — the call answers
//! an `ImageId` and the image is `pending` until the snapshots backing it are written — and
//! each state the AMI moves INTO is published onto the account's default EventBridge bus as
//! `source` `aws.ec2` / `detail-type` `EC2 AMI State Change`, naming the image in the
//! envelope's `resources` and carrying in the event `detail` the image's id, the state it
//! moved into, the error message of a state that failed and the id of the call the move
//! belongs to. So the AMI reaching `available` and the AMI being `deregistered` are both
//! observable without polling `DescribeImages`, and a client that bakes an image is told
//! instead of hanging.
//!
//! The AMI itself is computed from the INSTANCE: "if you customized your instance with […]
//! Amazon EBS volumes in addition to the root device volume, the new AMI contains block
//! device mapping information for those volumes", so every Amazon EBS volume the instance
//! holds becomes one device of the AMI restored from a snapshot this call makes, and the
//! image is read back through `DescribeImages` and its snapshots through
//! `DescribeSnapshots`.
//!
//! The rows span what the rule generalises over: the state the AMI moved into (`available`
//! and `deregistered`) and HOW MANY volumes the instance holds (its root volume alone, and a
//! root volume plus a data volume), plus a request the service refuses, which moves no AMI
//! anywhere and so publishes nothing.
//!
//! Nothing here reads this crate's own state: EventBridge and Amazon SQS are mounted BESIDE
//! Amazon EC2 on one engine, exactly as the assembled router mounts them, and every event is
//! read off the queue a real rule on the real default bus delivered it to.

use std::time::Instant;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_event_bus::{
    DELIVERY_BUDGET, POLL_INTERVAL, REGION, assert_envelope, drain, ec2_ok, ec2_refused, emulator,
    p, resources, watched_queue,
};
use super::ec2_query::{element, elements, error_code, indexed};

/// The `detail-type` an AMI's state change is published under, spelled out here rather than
/// imported, so a wrong constant in the implementation cannot make a test pass.
const AMI_STATE_CHANGE: &str = "EC2 AMI State Change";

/// The two states an AMI of this suite moves into.
const AVAILABLE: &str = "available";
const DEREGISTERED: &str = "deregistered";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and its root device is the one the image this crate publishes by default carries.
const AMI: &str = "ami-0123456789abcdef0";

/// The devices the data-volume rows hold their volumes at, one each: a data device, so the
/// instance's own root device is left where the launch put it.
const DATA_DEVICES: [&str; 2] = ["/dev/sdf", "/dev/sdg"];

/// One `RunInstances` of a single instance from [`AMI`], answering the instance it minted.
async fn launch(router: &MultiServiceRouter, case: &str) -> String {
    let body = ec2_ok(
        router,
        case,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: the launch must report its instance: {body}"))
}

/// Look at `instance_id` until it reports `state`.
async fn until_instance(router: &MultiServiceRouter, case: &str, instance_id: &str, state: &str) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let body = ec2_ok(
            router,
            case,
            "DescribeInstances",
            &indexed("InstanceId", &[instance_id]),
        )
        .await;
        if elements(&body, "name").iter().any(|name| name == state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {instance_id} must reach {state}, got {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// Look at `volume_id` until it reports the state `state`.
async fn until_volume(router: &MultiServiceRouter, case: &str, volume_id: &str, state: &str) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    loop {
        let body = ec2_ok(
            router,
            case,
            "DescribeVolumes",
            &indexed("VolumeId", &[volume_id]),
        )
        .await;
        if elements(&body, "status").iter().any(|status| status == state) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: {volume_id} must reach {state}, got {body}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A RUNNING instance holding `data_volumes` data volumes beside its root volume, answering
/// the instance id.
async fn instance_with_data_volumes(
    router: &MultiServiceRouter,
    case: &str,
    data_volumes: usize,
) -> String {
    let instance_id = launch(router, case).await;
    until_instance(router, case, &instance_id, "running").await;
    let described = ec2_ok(
        router,
        case,
        "DescribeInstances",
        &indexed("InstanceId", &[&instance_id]),
    )
    .await;
    let zone = element(&described, "availabilityZone")
        .unwrap_or_else(|| panic!("case {case:?}: the instance reports its zone: {described}"));
    for index in 0..data_volumes {
        let created = ec2_ok(
            router,
            case,
            "CreateVolume",
            &[
                p("AvailabilityZone", &zone),
                p("VolumeType", "gp3"),
                p("Size", "4"),
            ],
        )
        .await;
        let volume_id = element(&created, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports a volumeId: {created}"));
        until_volume(router, case, &volume_id, "available").await;
        ec2_ok(
            router,
            case,
            "AttachVolume",
            &[
                p("VolumeId", &volume_id),
                p("InstanceId", &instance_id),
                // One device per extra volume, so each is held at its own.
                p("Device", DATA_DEVICES[index % DATA_DEVICES.len()]),
            ],
        )
        .await;
        until_volume(router, case, &volume_id, "in-use").await;
    }
    instance_id
}

/// Every event drained so far: the AMIs of one table interleave on one queue, so a drain
/// files every event it takes rather than discarding the ones another row is waiting for.
#[derive(Default)]
struct Watcher {
    events: Vec<Value>,
}

impl Watcher {
    /// The events filed for `image_id` reporting the state `state`.
    fn filed(&self, image_id: &str, state: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|event| {
                event["detail"]["ImageId"].as_str() == Some(image_id)
                    && event["detail"]["State"].as_str() == Some(state)
            })
            .collect()
    }

    /// The one `state` change of `image_id`, looked for until the deadline passes rather than
    /// waited out, so the answer is the one the service published however fast the host runs.
    async fn one(
        &mut self,
        router: &MultiServiceRouter,
        case: &str,
        url: &str,
        image_id: &str,
        state: &str,
    ) -> Value {
        let deadline = Instant::now() + DELIVERY_BUDGET;
        loop {
            self.events.extend(drain(router, case, url).await);
            let found = self.filed(image_id, state);
            if !found.is_empty() {
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: exactly one {state} change is published for {image_id}, \
                     got {found:?}"
                );
                return found[0].clone();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: a {state} change of {image_id} must be delivered, drained \
                 {:?} instead",
                self.events
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The members every AMI state change carries, asserted once per row.
fn assert_state_change(case: &str, image_id: &str, state: &str, event: &Value) {
    assert_envelope(case, AMI_STATE_CHANGE, event);
    // The AMI's own ARN, which carries no account segment: an AMI is addressed as
    // `arn:aws:ec2:<region>::image/<image-id>`.
    assert_eq!(
        resources(event),
        vec![format!("arn:aws:ec2:{REGION}::image/{image_id}")],
        "case {case:?}: the event names the one AMI it is about, by its own ARN, got {event}"
    );
    let detail = &event["detail"];
    assert_eq!(
        detail["State"].as_str(),
        Some(state),
        "case {case:?}: expected the state {state}, got {event}"
    );
    assert_eq!(
        detail["ErrorMessage"].as_str(),
        Some(""),
        "case {case:?}: a move that failed at nothing reports no error message, got {event}"
    );
    let request_id = detail["RequestId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the detail carries a RequestId, got {event}"));
    assert!(
        ::service_common::query_error_envelope::is_request_id(request_id),
        "case {case:?}: expected a request id of the 8-4-4-4-12 form, got {request_id:?}"
    );
}

/// THE RULE over the states an AMI created from an instance moves into, and the AMI the
/// create computes from that instance.
#[tokio::test]
async fn an_ami_created_from_an_instance_publishes_available_and_then_deregistered() {
    let router = emulator();
    let mut watcher = Watcher::default();
    let url = watched_queue(
        &router,
        "the AMI state changes",
        "ami-state-changes",
        AMI_STATE_CHANGE,
    )
    .await;

    // The rows: how many volumes the instance holds beside its root volume, which is what the
    // AMI's own mappings and the snapshots behind them are counted against.
    for (case, data_volumes, name) in [
        ("an instance holding its root volume alone", 0usize, "root-only-image"),
        ("an instance holding a data volume too", 1usize, "root-and-data-image"),
    ] {
        let instance_id = instance_with_data_volumes(&router, case, data_volumes).await;
        let created = ec2_ok(
            &router,
            case,
            "CreateImage",
            &[
                p("InstanceId", &instance_id),
                p("Name", name),
                p("Description", "baked by the image suite"),
            ],
        )
        .await;
        let image_id = element(&created, "imageId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateImage reports an imageId: {created}"));

        // The AMI is answered while it is still being assembled, so the `available` move is
        // the clock's: the event for it is what tells a client the image is usable.
        let event = watcher.one(&router, case, &url, &image_id, AVAILABLE).await;
        assert_state_change(case, &image_id, AVAILABLE, &event);

        // Read the AMI back through the describe: the create's answer and a later read report
        // the same image, with one Amazon EBS device per volume the instance held and a
        // snapshot of this account behind each.
        let described = ec2_ok(
            &router,
            case,
            "DescribeImages",
            &indexed("ImageId", &[&image_id]),
        )
        .await;
        assert_eq!(
            element(&described, "imageState").as_deref(),
            Some(AVAILABLE),
            "case {case:?}: the AMI the event announced is available when read back: {described}"
        );
        assert_eq!(
            element(&described, "name").as_deref(),
            Some(name),
            "case {case:?}: the AMI carries the name the create was given: {described}"
        );
        assert_eq!(
            element(&described, "sourceInstanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: the AMI names the instance it was created from: {described}"
        );
        let snapshot_ids = elements(&described, "snapshotId");
        assert_eq!(
            snapshot_ids.len(),
            data_volumes + 1,
            "case {case:?}: one device of the AMI per volume the instance held, got \
             {snapshot_ids:?} in {described}"
        );
        for snapshot_id in &snapshot_ids {
            let snapshot = ec2_ok(
                &router,
                case,
                "DescribeSnapshots",
                &indexed("SnapshotId", &[snapshot_id]),
            )
            .await;
            assert!(
                element(&snapshot, "volumeId").is_some(),
                "case {case:?}: {snapshot_id} is a snapshot of this account, of the volume it \
                 was taken of: {snapshot}"
            );
        }

        // The other end of the lifecycle: a deregistration moves the AMI out of the images the
        // account can see, and that state change is published too.
        ec2_ok(
            &router,
            case,
            "DeregisterImage",
            &[p("ImageId", &image_id)],
        )
        .await;
        let event = watcher
            .one(&router, case, &url, &image_id, DEREGISTERED)
            .await;
        assert_state_change(case, &image_id, DEREGISTERED, &event);
        // "A deregistered AMI can't be used to launch new instances": the image the event
        // announced as deregistered is no image of the account any more, which is what a read
        // of the account's own images reports.
        let owned = ec2_ok(
            &router,
            case,
            "DescribeImages",
            &indexed("Owner", &["self"]),
        )
        .await;
        assert!(
            !elements(&owned, "imageId").contains(&image_id),
            "case {case:?}: {image_id} was deregistered, so it is none of the account's \
             images: {owned}"
        );
        // And the id itself still READS as an AMI id: this service issued it as one, so a
        // question about it is a question about an image the account cannot see rather than
        // about a value that cannot be read.
        let (status, body) = ec2_refused(
            &router,
            case,
            "DescribeImages",
            &indexed("ImageId", &[&image_id]),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidAMIID.NotFound"),
            "case {case:?}: a deregistered AMI's own id names an image that is not there, got \
             {status} with {body}"
        );
    }
}

/// THE RULE over the STATE the source instance is in: "creates an Amazon EBS-backed AMI from
/// an Amazon EBS-backed instance that is either running or stopped" — so a stopped instance
/// is imaged exactly as a running one is, and an instance in any other state is refused
/// `IncorrectInstanceState` and moves no AMI anywhere, so it publishes nothing.
#[tokio::test]
async fn only_a_running_or_stopped_instance_can_be_imaged() {
    let router = emulator();
    let mut watcher = Watcher::default();
    let url = watched_queue(
        &router,
        "the source instance's state",
        "ami-source-states",
        AMI_STATE_CHANGE,
    )
    .await;

    // A STOPPED instance still holds its volumes, so the AMI of it describes them and reaches
    // `available` like any other.
    let stopped = "a stopped instance";
    let instance_id = instance_with_data_volumes(&router, stopped, 0).await;
    ec2_ok(
        &router,
        stopped,
        "StopInstances",
        &indexed("InstanceId", &[&instance_id]),
    )
    .await;
    until_instance(&router, stopped, &instance_id, "stopped").await;
    let created = ec2_ok(
        &router,
        stopped,
        "CreateImage",
        &[p("InstanceId", &instance_id), p("Name", "stopped-source")],
    )
    .await;
    let image_id = element(&created, "imageId")
        .unwrap_or_else(|| panic!("case {stopped:?}: CreateImage reports an imageId: {created}"));
    let event = watcher
        .one(&router, stopped, &url, &image_id, AVAILABLE)
        .await;
    assert_state_change(stopped, &image_id, AVAILABLE, &event);
    let described = ec2_ok(
        &router,
        stopped,
        "DescribeImages",
        &indexed("ImageId", &[&image_id]),
    )
    .await;
    assert_eq!(
        elements(&described, "snapshotId").len(),
        1,
        "case {stopped:?}: a stopped instance keeps its root volume, so the AMI describes it: \
         {described}"
    );

    // An instance in any OTHER state is refused, and the refusal publishes nothing: there was
    // no AMI to move.
    let terminated = "a terminated instance";
    ec2_ok(
        &router,
        terminated,
        "TerminateInstances",
        &indexed("InstanceId", &[&instance_id]),
    )
    .await;
    until_instance(&router, terminated, &instance_id, "terminated").await;
    let (status, body) = ec2_refused(
        &router,
        terminated,
        "CreateImage",
        &[p("InstanceId", &instance_id), p("Name", "terminated-source")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "IncorrectInstanceState"),
        "case {terminated:?}: only a running or stopped instance can be imaged, got {status} \
         with {body}"
    );
    watcher.events.extend(drain(&router, terminated, &url).await);
    let after: Vec<&Value> = watcher
        .events
        .iter()
        .filter(|event| event["detail"]["ImageId"].as_str() != Some(image_id.as_str()))
        .collect();
    assert!(
        after.is_empty(),
        "case {terminated:?}: the refused create made no AMI, so it published nothing, got \
         {after:?}"
    );
}

/// THE RULE's other half: the event is the outcome of a MOVE, so a create the service refuses
/// — one naming a name an AMI of the account already holds — moves no AMI anywhere and
/// publishes nothing.
#[tokio::test]
async fn a_refused_create_publishes_no_state_change() {
    let case = "a name another AMI already holds";
    let router = emulator();
    let mut watcher = Watcher::default();
    let url = watched_queue(&router, case, "ami-refusals", AMI_STATE_CHANGE).await;

    let instance_id = instance_with_data_volumes(&router, case, 0).await;
    let created = ec2_ok(
        &router,
        case,
        "CreateImage",
        &[p("InstanceId", &instance_id), p("Name", "one-and-only")],
    )
    .await;
    let image_id = element(&created, "imageId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateImage reports an imageId: {created}"));
    // The positive control of the SAME wiring, which is what makes the absence below a fact
    // about the refused create rather than about the bus.
    watcher.one(&router, case, &url, &image_id, AVAILABLE).await;

    let (status, body) = ec2_refused(
        &router,
        case,
        "CreateImage",
        &[p("InstanceId", &instance_id), p("Name", "one-and-only")],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "InvalidAMIName.Duplicate"),
        "case {case:?}: a name already in use is refused, got {status} with {body}"
    );

    // Anything the refused call published would be on the queue by now: both calls have been
    // answered and the first one's own event has already been taken off.
    watcher.events.extend(drain(&router, case, &url).await);
    let after: Vec<&Value> = watcher
        .events
        .iter()
        .filter(|event| event["detail"]["ImageId"].as_str() != Some(image_id.as_str()))
        .collect();
    assert!(
        after.is_empty(),
        "case {case:?}: the refused create made no AMI, so it published nothing, got {after:?}"
    );
}
