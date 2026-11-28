// SPDX-License-Identifier: CC0-1.0

//! The event the COMPLETION of a multi-volume snapshot set is published as, end to end over
//! the wire.
//!
//! THE RULE: `CreateSnapshots` takes one snapshot of every Amazon EBS volume of an instance,
//! and the set is not finished when the call is answered — each snapshot is handed back
//! `pending`. Once they have completed, ONE event for the SET is published onto the account's
//! default EventBridge bus as `source` `aws.ec2` / `detail-type` `EBS Multi-Volume Snapshots
//! Completion Status`, naming every snapshot of the set in the envelope's `resources` and
//! listing each of them in the event `detail` with the volume it was taken of, the state it
//! settled in and the interval it ran over, beside the action the set is the outcome of, the
//! result it reached and the interval the set ran over. So a client that promotes a backup set
//! only once the whole set is safe has one signal to wait for instead of joining the
//! snapshots' own events itself.
//!
//! The rows span what the rule generalises over: HOW MANY volumes the set covers — every
//! volume of the instance, and the same instance with its boot volume excluded — the detail
//! listing exactly the snapshots the call answered with, whichever it is.
//!
//! Nothing here reads this crate's own state: EventBridge and Amazon SQS are mounted BESIDE
//! Amazon EC2 on one engine, exactly as the assembled router mounts them, and every event is
//! read off the queue a real rule on the real default bus delivered it to.

use std::time::Instant;

use serde_json::Value;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_event_bus::{
    DELIVERY_BUDGET, POLL_INTERVAL, assert_envelope, drain, ec2_ok, emulator, p, resources,
    watched_queue,
};
use super::ec2_query::{element, elements, indexed};

/// The `detail-type` and the `detail` members a set's completion is published under, spelled
/// out here rather than imported, so a wrong constant in the implementation cannot make a test
/// pass.
const MULTI_VOLUME_COMPLETION: &str = "EBS Multi-Volume Snapshots Completion Status";
const CREATE_SNAPSHOTS_EVENT: &str = "createSnapshots";
const SUCCEEDED: &str = "succeeded";

/// The state every snapshot of a set that held its source's data settles in.
const COMPLETED: &str = "completed";

/// An AMI id of the right FORM that this account holds no image for: the launch is still
/// served, and its root device is the one the image this crate publishes by default carries.
const AMI: &str = "ami-0123456789abcdef0";

/// The devices the data volumes are held at, one each.
const DATA_DEVICES: [&str; 2] = ["/dev/sdf", "/dev/sdg"];

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

/// A RUNNING instance holding one data volume per entry of [`DATA_DEVICES`] beside its root
/// volume, answering the instance id.
async fn instance_with_data_volumes(router: &MultiServiceRouter, case: &str) -> String {
    let launched = ec2_ok(
        router,
        case,
        "RunInstances",
        &[p("ImageId", AMI), p("MinCount", "1"), p("MaxCount", "1")],
    )
    .await;
    let instance_id = element(&launched, "instanceId")
        .unwrap_or_else(|| panic!("case {case:?}: the launch reports its instance: {launched}"));
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
    for device in DATA_DEVICES {
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
                p("Device", device),
            ],
        )
        .await;
        until_volume(router, case, &volume_id, "in-use").await;
    }
    instance_id
}

/// The one completion event naming `snapshot_ids`, looked for until the deadline passes rather
/// than waited out, so the answer is the one the service published however fast the host runs.
///
/// Every event drained on the way is kept, so an event of another set is reported rather than
/// silently dropped.
async fn completion_for(
    router: &MultiServiceRouter,
    case: &str,
    url: &str,
    snapshot_ids: &[String],
) -> (Value, Vec<Value>) {
    let deadline = Instant::now() + DELIVERY_BUDGET;
    let mut drained: Vec<Value> = Vec::new();
    loop {
        drained.extend(drain(router, case, url).await);
        let (mine, others): (Vec<Value>, Vec<Value>) = drained.iter().cloned().partition(|event| {
            let named = resources(event);
            snapshot_ids.iter().all(|snapshot_id| {
                named
                    .iter()
                    .any(|resource| resource.ends_with(&format!("snapshot/{snapshot_id}")))
            })
        });
        if !mine.is_empty() {
            assert_eq!(
                mine.len(),
                1,
                "case {case:?}: one call made one set, so one completion is published, got \
                 {mine:?}"
            );
            return (mine[0].clone(), others);
        }
        assert!(
            Instant::now() < deadline,
            "case {case:?}: a completion naming {snapshot_ids:?} must be delivered, drained \
             {drained:?} instead"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// THE RULE over the sets a `CreateSnapshots` call can produce: one completion event per call,
/// listing every snapshot the call answered with.
#[tokio::test]
async fn every_multi_volume_snapshot_set_publishes_one_completion_listing_its_snapshots() {
    let router = emulator();
    let url = watched_queue(
        &router,
        "the set completions",
        "ebs-multi-volume-completions",
        MULTI_VOLUME_COMPLETION,
    )
    .await;

    // The rows: whether the set covers every volume of the instance or leaves out its boot
    // volume, which is what the number of snapshots in one set is read against.
    for (case, exclude_boot_volume, expected) in [
        (
            "a set of every volume of the instance",
            false,
            DATA_DEVICES.len() + 1,
        ),
        (
            "a set of the data volumes alone",
            true,
            DATA_DEVICES.len(),
        ),
    ] {
        let instance_id = instance_with_data_volumes(&router, case).await;
        let mut params = vec![p("InstanceSpecification.InstanceId", &instance_id)];
        if exclude_boot_volume {
            params.push(p("InstanceSpecification.ExcludeBootVolume", "true"));
        }
        let created = ec2_ok(&router, case, "CreateSnapshots", &params).await;
        let snapshot_ids = elements(&created, "snapshotId");
        assert_eq!(
            snapshot_ids.len(),
            expected,
            "case {case:?}: one snapshot per volume of the set, got {snapshot_ids:?} in \
             {created}"
        );

        let (event, others) = completion_for(&router, case, &url, &snapshot_ids).await;
        assert!(
            others.is_empty(),
            "case {case:?}: only this set's own completion is published, got {others:?}"
        );
        assert_envelope(case, MULTI_VOLUME_COMPLETION, &event);
        assert_eq!(
            resources(&event).len(),
            snapshot_ids.len(),
            "case {case:?}: the event names every snapshot of the set and nothing else, got \
             {event}"
        );

        let detail = &event["detail"];
        assert_eq!(
            detail["event"].as_str(),
            Some(CREATE_SNAPSHOTS_EVENT),
            "case {case:?}: expected the event {CREATE_SNAPSHOTS_EVENT}, got {event}"
        );
        assert_eq!(
            detail["result"].as_str(),
            Some(SUCCEEDED),
            "case {case:?}: every snapshot of the set held its source's data, so the set \
             succeeded, got {event}"
        );
        assert_eq!(
            detail["cause"].as_str(),
            Some(""),
            "case {case:?}: a set that failed at nothing states no cause, got {event}"
        );
        let request_id = detail["request-id"].as_str().unwrap_or_else(|| {
            panic!("case {case:?}: the detail carries a request-id, got {event}")
        });
        assert!(
            ::service_common::query_error_envelope::is_request_id(request_id),
            "case {case:?}: expected a request id of the 8-4-4-4-12 form, got {request_id:?}"
        );

        // The detail LISTS the set: one entry per snapshot the call answered with, each naming
        // the snapshot, the volume it was taken of and the state it settled in.
        let listed = detail["snapshots"].as_array().cloned().unwrap_or_default();
        let mut listed_snapshots: Vec<String> = listed
            .iter()
            .map(|entry| {
                entry["snapshot_id"]
                    .as_str()
                    .unwrap_or_default()
                    .rsplit('/')
                    .next()
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
        listed_snapshots.sort();
        let mut answered = snapshot_ids.clone();
        answered.sort();
        assert_eq!(
            listed_snapshots, answered,
            "case {case:?}: the detail lists exactly the snapshots the call answered with, \
             got {event}"
        );
        for entry in &listed {
            assert_eq!(
                entry["status"].as_str(),
                Some(COMPLETED),
                "case {case:?}: a snapshot of a succeeded set settled completed, got {entry}"
            );
            let source = entry["source"].as_str().unwrap_or_default();
            assert!(
                source.contains("volume/vol-"),
                "case {case:?}: each entry names the volume it was taken of, got {entry}"
            );
            assert!(
                entry["startTime"].as_str().is_some_and(|at| !at.is_empty()),
                "case {case:?}: each entry reports when it was initiated, got {entry}"
            );
            assert!(
                entry["endTime"].as_str().is_some_and(|at| !at.is_empty()),
                "case {case:?}: each entry reports when it completed, got {entry}"
            );
        }

        // The set's own interval, read off the same instants: the completion cannot end before
        // it started.
        let started = detail["startTime"].as_str().unwrap_or_default();
        let ended = detail["endTime"].as_str().unwrap_or_default();
        assert!(
            !started.is_empty() && !ended.is_empty() && started <= ended,
            "case {case:?}: the set reports an interval that does not end before it started, \
             got {event}"
        );

        // Every snapshot the event lists is a snapshot of this account, completed as the event
        // says it is — the event and the API cannot say two different things about one
        // snapshot.
        for snapshot_id in &snapshot_ids {
            let described = ec2_ok(
                &router,
                case,
                "DescribeSnapshots",
                &indexed("SnapshotId", &[snapshot_id]),
            )
            .await;
            assert_eq!(
                element(&described, "status").as_deref(),
                Some(COMPLETED),
                "case {case:?}: {snapshot_id} is completed when read back: {described}"
            );
        }
    }
}
