// SPDX-License-Identifier: CC0-1.0

//! The event an EBS volume's DELETE is published as, end to end over the wire.
//!
//! THE RULE: a delete Amazon EBS PERFORMS takes the volume out of the account's volumes, and
//! that outcome is published onto the account's default EventBridge bus as `source` `aws.ec2`
//! / `detail-type` `EBS Volume Notification`, naming the volume in the envelope's `resources`
//! and carrying in the event `detail` the call it is the outcome of (`deleteVolume`), the
//! `result` that call reached (`deleted`), the `cause` of a result that failed and the id of
//! the call itself. A call the service REFUSES performs no delete, so it publishes nothing:
//! the event is the outcome of a delete, not of a call.
//!
//! The rows span what the rule generalises over: whether the volume is one the service can
//! delete at all — an unattached volume, the same volume while an instance holds it (refused
//! `VolumeInUse`), and that volume again once it has been detached.
//!
//! The rule's negative half can only be read against a working positive one, so every row
//! watches ONE rule over the whole class of volume notifications: a row that must publish
//! nothing is read on the queue the `createVolume` events of the same volumes arrive on.
//!
//! Nothing here reads this crate's own state: EventBridge and Amazon SQS are mounted BESIDE
//! Amazon EC2 on one engine, exactly as the assembled router mounts them, and every event is
//! read off the queue a real rule on the real default bus delivered it to.

use std::time::Instant;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_event_bus::{
    DELIVERY_BUDGET, POLL_INTERVAL, assert_envelope, drain, ec2_ok, ec2_refused, emulator, p,
    resources, watched_queue,
};
use super::ec2_query::{element, elements, error_code, indexed};

/// The `detail-type` and the `detail.event` / `detail.result` a volume delete is published
/// under, spelled out here rather than imported, so a wrong constant in the implementation
/// cannot make a test pass.
const VOLUME_NOTIFICATION: &str = "EBS Volume Notification";
const DELETE_VOLUME_EVENT: &str = "deleteVolume";
const CREATE_VOLUME_EVENT: &str = "createVolume";
const DELETED: &str = "deleted";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and its root device is the one the image this crate publishes by default carries.
const AMI: &str = "ami-0123456789abcdef0";

/// The device the attached row holds its volume at: a data device, so the instance's own root
/// device is left where the launch put it.
const DATA_DEVICE: &str = "/dev/sdf";

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

/// Look at `instance_id` until it reports `state`, so a call that acts on an instance in a
/// particular state is made then and not before.
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

/// The Availability Zone `instance_id` runs in, read off the instance rather than written
/// here: a volume can only be attached to an instance of its own zone.
async fn instance_zone(router: &MultiServiceRouter, case: &str, instance_id: &str) -> String {
    let body = ec2_ok(
        router,
        case,
        "DescribeInstances",
        &indexed("InstanceId", &[instance_id]),
    )
    .await;
    element(&body, "availabilityZone")
        .unwrap_or_else(|| panic!("case {case:?}: the instance must report its zone: {body}"))
}

/// One volume created in `zone`, answering the volume id it was accepted under and the ARN the
/// account knows it by.
async fn create_volume(router: &MultiServiceRouter, case: &str, zone: &str) -> (String, String) {
    let body = ec2_ok(
        router,
        case,
        "CreateVolume",
        &[
            p("AvailabilityZone", zone),
            p("VolumeType", "gp3"),
            p("Size", "4"),
        ],
    )
    .await;
    let volume_id = element(&body, "volumeId")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports a volumeId, got {body}"));
    let volume_arn = element(&body, "volumeArn")
        .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports a volumeArn, got {body}"));
    (volume_id, volume_arn)
}

/// Look at `volume_id` until it reports the state `state`, so the call that acts on it next is
/// made against a volume in that state.
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

/// Every event drained so far, by the volume ARN it names: the volumes of one table interleave
/// on one queue, so a drain files every event it takes rather than discarding the ones another
/// row is waiting for.
#[derive(Default)]
struct Watcher {
    events: Vec<Value>,
}

impl Watcher {
    /// Take everything waiting on `url` right now and file it.
    async fn drain(&mut self, router: &MultiServiceRouter, case: &str, url: &str) {
        self.events.extend(drain(router, case, url).await);
    }

    /// The events filed for `volume_arn` whose `detail.event` is `event`.
    fn filed(&self, volume_arn: &str, event: &str) -> Vec<&Value> {
        self.events
            .iter()
            .filter(|filed| {
                resources(filed).iter().any(|resource| resource == volume_arn)
                    && filed["detail"]["event"].as_str() == Some(event)
            })
            .collect()
    }

    /// The one `event` notification of `volume_arn`, looked for until the deadline passes
    /// rather than waited out, so the answer is the one the service published however fast the
    /// host runs.
    async fn one(
        &mut self,
        router: &MultiServiceRouter,
        case: &str,
        url: &str,
        volume_arn: &str,
        event: &str,
    ) -> Value {
        let deadline = Instant::now() + DELIVERY_BUDGET;
        loop {
            self.drain(router, case, url).await;
            let found = self.filed(volume_arn, event);
            if !found.is_empty() {
                assert_eq!(
                    found.len(),
                    1,
                    "case {case:?}: exactly one {event} notification is published for \
                     {volume_arn}, got {found:?}"
                );
                return found[0].clone();
            }
            assert!(
                Instant::now() < deadline,
                "case {case:?}: a {event} notification for {volume_arn} must be delivered, \
                 drained {:?} instead",
                self.events
            );
            tokio::time::sleep(POLL_INTERVAL).await;
        }
    }
}

/// The members every delete notification carries, asserted once per row.
fn assert_delete_event(case: &str, volume_arn: &str, event: &Value) {
    assert_envelope(case, VOLUME_NOTIFICATION, event);
    assert_eq!(
        resources(event),
        vec![volume_arn.to_string()],
        "case {case:?}: the event names the one volume it is about, got {event}"
    );
    let detail = &event["detail"];
    assert_eq!(
        detail["result"].as_str(),
        Some(DELETED),
        "case {case:?}: expected the result {DELETED}, got {event}"
    );
    assert_eq!(
        detail["cause"].as_str(),
        Some(""),
        "case {case:?}: a delete that was performed failed at nothing, so it states no cause, \
         got {event}"
    );
    let request_id = detail["request-id"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: the detail carries a request-id, got {event}"));
    assert!(
        ::service_common::query_error_envelope::is_request_id(request_id),
        "case {case:?}: expected a request id of the 8-4-4-4-12 form, got {request_id:?}"
    );
}

/// THE RULE over the deletes a volume can be asked for: a delete the service performs is
/// published, and one it refuses is not.
#[tokio::test]
async fn a_delete_the_service_performs_publishes_one_notification_of_its_outcome() {
    let case = "the volume deletes";
    let router = emulator();
    let url = watched_queue(&router, case, "ebs-volume-deletes", VOLUME_NOTIFICATION).await;
    let mut watcher = Watcher::default();

    let instance_id = launch(&router, case).await;
    until_instance(&router, case, &instance_id, "running").await;
    let zone = instance_zone(&router, case, &instance_id).await;

    // Row 1: a volume nothing holds. The delete is performed, so its outcome is published.
    let unattached = "an unattached volume";
    let (lone_id, lone_arn) = create_volume(&router, unattached, &zone).await;
    until_volume(&router, unattached, &lone_id, "available").await;
    ec2_ok(
        &router,
        unattached,
        "DeleteVolume",
        &[p("VolumeId", &lone_id)],
    )
    .await;
    let event = watcher
        .one(&router, unattached, &url, &lone_arn, DELETE_VOLUME_EVENT)
        .await;
    assert_delete_event(unattached, &lone_arn, &event);

    // Row 2: the same delete of a volume an instance HOLDS. The call is refused, so no delete
    // was performed and nothing is published for it — read on the queue the volume's own
    // `createVolume` event arrives on, so "no event for a refused delete" cannot be confused
    // with "no events at all".
    let attached = "a volume an instance holds";
    let (held_id, held_arn) = create_volume(&router, attached, &zone).await;
    until_volume(&router, attached, &held_id, "available").await;
    ec2_ok(
        &router,
        attached,
        "AttachVolume",
        &[
            p("VolumeId", &held_id),
            p("InstanceId", &instance_id),
            p("Device", DATA_DEVICE),
        ],
    )
    .await;
    until_volume(&router, attached, &held_id, "in-use").await;
    let (status, body) = ec2_refused(
        &router,
        attached,
        "DeleteVolume",
        &[p("VolumeId", &held_id)],
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "VolumeInUse"),
        "case {attached:?}: a delete of a held volume is refused VolumeInUse, got {status} \
         with {body}"
    );
    // The positive control of the SAME wiring: the volume's create was published, which is
    // what makes the absence below a fact about this delete rather than about the bus.
    watcher
        .one(&router, attached, &url, &held_arn, CREATE_VOLUME_EVENT)
        .await;
    let for_refusal = watcher.filed(&held_arn, DELETE_VOLUME_EVENT);
    assert!(
        for_refusal.is_empty(),
        "case {attached:?}: a refused delete performs none, so it publishes none, got \
         {for_refusal:?}"
    );

    // Row 3: that volume again, once nothing holds it. The delete is performed, so its outcome
    // is published — one event, for the one delete that happened.
    let detached = "a volume an instance has let go of";
    ec2_ok(&router, detached, "DetachVolume", &[p("VolumeId", &held_id)]).await;
    until_volume(&router, detached, &held_id, "available").await;
    ec2_ok(&router, detached, "DeleteVolume", &[p("VolumeId", &held_id)]).await;
    let event = watcher
        .one(&router, detached, &url, &held_arn, DELETE_VOLUME_EVENT)
        .await;
    assert_delete_event(detached, &held_arn, &event);

    // Every volume of this table is gone, so a later read of either id reports neither.
    for volume_id in [&lone_id, &held_id] {
        let (status, body) =
            ec2_refused(&router, case, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidVolume.NotFound"),
            "case {case:?}: {volume_id} was deleted, so no read of it reports a volume, got \
             {status} with {body}"
        );
    }
}
