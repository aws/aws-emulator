// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `AttachVolume`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. AN ATTACH RECORDS THE ATTACHMENT ON THE VOLUME AND THE CLOCK BRINGS IT UP: the
//!    response is the attachment as the store now holds it — the volume, the instance, the
//!    device the request named, the EBS card index ("the default EBS card index is 0") and
//!    `deleteOnTermination` — in the non-terminal `attaching` state, which the clock, not a
//!    caller, moves to `attached`; the volume holds it from that moment, so it reports
//!    `in-use`, and `DescribeVolumes` reports the same projection;
//! 2. AN ATTACH IS ADMITTED ONLY BY THE REQUEST AND THE STATE BEHIND IT: "attaches an
//!    Amazon EBS volume to a running or stopped instance" in the same Availability Zone
//!    ("the volume and instance must be within the same Availability Zone",
//!    `AttachVolumeRequest$VolumeId`), at a device name the instance does not already hold a
//!    volume at — and each refusal carries EC2's own code for what is wrong, with nothing
//!    written;
//! 3. A MULTI-ATTACH ENABLED VOLUME IS HELD BY SEVERAL INSTANCES, up to the number the
//!    Amazon EBS User Guide publishes ([`MULTI_ATTACH_INSTANCE_LIMIT`]), and never twice by
//!    the same one;
//! 4. AN INSTANCE HOLDS AS MANY EBS VOLUMES AS ITS INSTANCE TYPE SUPPORTS: "if you exceed
//!    the volume attachment limit for an instance type, the attachment request fails with
//!    the AttachmentLimitExceeded error" — the limit being the type's own
//!    `EbsInfo$MaximumEbsAttachments`, which `DescribeInstanceTypes` publishes.
//!
//! Each table row is a distinct input along what the rule generalises over — which device
//! name, which EBS card index, which state the instance or the volume is in, which id form,
//! which instance type and so which limit — so no row is the specification.

use std::sync::Arc;

use ec2::state::{
    ATTACHMENT_DELETE_ON_TERMINATION, ComAmazonawsEc2Amazonec2State as Ec2State,
    DEFAULT_INSTANCE_TYPE, MAX_DEVICE_NAME_CHARS, MULTI_ATTACH_INSTANCE_LIMIT,
    VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_ATTACHMENT_STATE_ATTACHING, VOLUME_STATE_AVAILABLE,
    VOLUME_STATE_CREATING, VOLUME_STATE_IN_USE, ebs_attachment_limit,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::detach_volume::hold_in_creating;
use super::ec2_query::{as_timestamp, element, elements, error_code, error_message, indexed};
use super::volume_lifecycle::{REGION, p, past_the_transition, required_iops, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The AMI every launch here names. The launch path stores the id it was given, so an id of
/// the long form is all a launch needs — and an instance launched from an AMI the account
/// cannot see still gets the EBS root volume every instance has, which is one attachment
/// before this suite attaches anything.
const AMI: &str = "ami-0123456789abcdef0";

/// A well-formed volume id the account holds no volume for.
const UNKNOWN_VOLUME: &str = "vol-0fedcba987654321f";

/// A well-formed instance id the account holds no instance for.
const UNKNOWN_INSTANCE: &str = "i-0fedcba987654321f";

/// One `action` call signed for the Region the suite works in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Launch one instance of `instance_type` and answer its id, without waiting for the clock:
/// a launch answers `pending`, which is a state of its own for rule 2.
async fn launch(router: &MultiServiceRouter, instance_type: &str) -> String {
    let (status, body) = op(
        router,
        "RunInstances",
        &[
            p("ImageId", AMI),
            p("MinCount", "1"),
            p("MaxCount", "1"),
            p("InstanceType", instance_type),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "RunInstances must succeed, got {status} with body {body}"
    );
    element(&body, "instanceId")
        .unwrap_or_else(|| panic!("RunInstances must report the instance it minted: {body}"))
}

/// One `running` instance of the default type: launched, then past the span the clock owes
/// its `pending` -> `running` move.
async fn running_instance(router: &MultiServiceRouter) -> String {
    let instance_id = launch(router, DEFAULT_INSTANCE_TYPE).await;
    past_the_transition().await;
    instance_id
}

/// Create one 8 GiB volume in `zone`, with whatever extra parameters the case needs, and
/// answer its id.
async fn volume_in(router: &MultiServiceRouter, zone: &str, extra: Params) -> String {
    let mut params = vec![p("AvailabilityZone", zone), p("Size", "8")];
    params.extend(extra);
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "CreateVolume must succeed, got {status} with body {body}"
    );
    element(&body, "volumeId").expect("CreateVolume reports the volume it minted")
}

/// One `available` volume in the Region's first zone — the zone a launch that names none
/// places its instance in. The volume is `creating` until the clock moves it, so a caller
/// that attaches it waits for that span once for every volume it created.
async fn volume(router: &MultiServiceRouter) -> String {
    volume_in(router, &zone_name(0), Vec::new()).await
}

/// `count` volumes in the Region's first zone.
async fn volumes(router: &MultiServiceRouter, count: usize) -> Vec<String> {
    let mut created = Vec::with_capacity(count);
    for _ in 0..count {
        created.push(volume(router).await);
    }
    created
}

/// The `AttachVolume` parameters for one attach.
fn attach_params(volume_id: &str, instance_id: &str, device: &str) -> Params {
    vec![
        p("VolumeId", volume_id),
        p("InstanceId", instance_id),
        p("Device", device),
    ]
}

/// Attach `volume_id` to `instance_id` at `device`, asserting the call was served.
async fn attach(
    router: &MultiServiceRouter,
    case: &str,
    volume_id: &str,
    instance_id: &str,
    device: &str,
) -> String {
    let (status, body) = op(
        router,
        "AttachVolume",
        &attach_params(volume_id, instance_id, device),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from AttachVolume, got {status} with body {body}"
    );
    body
}

/// The `DescribeVolumes` body for one volume.
async fn described(router: &MultiServiceRouter, volume_id: &str) -> String {
    let (status, body) = op(router, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await;
    assert_eq!(
        status, 200,
        "DescribeVolumes must succeed, got {status} with body {body}"
    );
    body
}

/// The `(device, state)` of every attachment the STORE holds for `volume_id`, as stored
/// rather than as derived on read — what a materialised move leaves behind.
fn stored_attachments(engine: &engine::InMemoryEngine, volume_id: &str) -> Vec<(String, String)> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .volumes
        .get(volume_id)
        .expect("the volume just created")
        .view
        .attachments
        .iter()
        .flatten()
        .map(|attachment| {
            (
                attachment.device.clone().unwrap_or_default(),
                attachment.state.clone().unwrap_or_default(),
            )
        })
        .collect()
}

/// The attachments of `volume_id` the store still owes an `attached` move, by the
/// `(instance, device)` pair that names each one.
fn owed_attachments(
    engine: &engine::InMemoryEngine,
    volume_id: &str,
) -> Vec<(String, String)> {
    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    guard
        .volumes
        .get(volume_id)
        .expect("the volume just created")
        .attaching_at
        .keys()
        .cloned()
        .collect()
}

/// RULE 1: the attachment the response reports is the one the store now holds, in the
/// `attaching` state the clock then moves to `attached`.
///
/// The rows span the device name forms the member documents ("for example, /dev/sdh or
/// xvdh"), a name at the length bound this service keeps for a stored caller-chosen string,
/// and whether the request names an EBS card index at all — one instance holding several
/// volumes at once, which is the ordinary case.
#[tokio::test]
async fn an_attach_records_the_attachment_the_request_names_and_the_clock_brings_it_up() {
    let (router, _engine) = emulator();
    let instance_id = launch(&router, DEFAULT_INSTANCE_TYPE).await;

    // `(case, device, the EbsCardIndex parameter, the index the attachment must report)`.
    let rows: Vec<(&str, String, Option<&str>, i32)> = vec![
        (
            "the device form the member documents first",
            "/dev/sdh".to_string(),
            None,
            0,
        ),
        (
            "the second form the member documents",
            "xvdh".to_string(),
            None,
            0,
        ),
        (
            "a device with the EBS card index the default names",
            "/dev/xvdf".to_string(),
            Some("0"),
            0,
        ),
        (
            "a device on a second EBS card",
            "/dev/nvme1n1".to_string(),
            Some("1"),
            1,
        ),
        (
            "a device name at the length bound",
            format!("/dev/{}", "d".repeat(MAX_DEVICE_NAME_CHARS - 5)),
            None,
            0,
        ),
    ];

    let mut attached: Vec<(String, String)> = Vec::new();
    // One wait for the whole table: the launch's `pending` -> `running` move and every
    // volume's `creating` -> `available` move are owed from the calls that started them, so
    // they fall due together rather than once per row.
    let created = volumes(&router, rows.len()).await;
    past_the_transition().await;
    for ((case, device, card_index, expected_index), volume_id) in rows.iter().zip(created) {
        let mut params = attach_params(&volume_id, &instance_id, device);
        if let Some(index) = card_index {
            params.push(p("EbsCardIndex", *index));
        }
        let (status, body) = op(&router, "AttachVolume", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        assert_eq!(
            element(&body, "volumeId").as_deref(),
            Some(volume_id.as_str()),
            "case {case:?}: expected the volume {volume_id}, got {body}"
        );
        assert_eq!(
            element(&body, "instanceId").as_deref(),
            Some(instance_id.as_str()),
            "case {case:?}: expected the instance {instance_id}, got {body}"
        );
        assert_eq!(
            element(&body, "device").as_deref(),
            Some(device.as_str()),
            "case {case:?}: expected the device {device:?}, got {body}"
        );
        assert_eq!(
            element(&body, "status").as_deref(),
            Some(VOLUME_ATTACHMENT_STATE_ATTACHING),
            "case {case:?}: expected {VOLUME_ATTACHMENT_STATE_ATTACHING}, got {body}"
        );
        assert_eq!(
            element(&body, "ebsCardIndex").as_deref(),
            Some(expected_index.to_string().as_str()),
            "case {case:?}: expected the EBS card index {expected_index}, got {body}"
        );
        assert_eq!(
            element(&body, "deleteOnTermination").as_deref(),
            Some(if ATTACHMENT_DELETE_ON_TERMINATION {
                "true"
            } else {
                "false"
            }),
            "case {case:?}: expected deleteOnTermination \
             {ATTACHMENT_DELETE_ON_TERMINATION}, got {body}"
        );
        let attach_time = element(&body, "attachTime").unwrap_or_default();
        assert!(
            as_timestamp(&attach_time).is_some(),
            "case {case:?}: expected a timestamp for attachTime, got {attach_time:?}"
        );

        // The volume holds the attachment from this moment, so it is in use — and the
        // describe reports the same projection of the one record.
        let body = described(&router, &volume_id).await;
        let states = elements(&body, "status");
        assert!(
            states.contains(&VOLUME_STATE_IN_USE.to_string())
                && states.contains(&VOLUME_ATTACHMENT_STATE_ATTACHING.to_string()),
            "case {case:?}: expected the volume {VOLUME_STATE_IN_USE} with its attachment \
             {VOLUME_ATTACHMENT_STATE_ATTACHING}, got {states:?} in {body}"
        );
        assert_eq!(
            elements(&body, "device"),
            vec![device.clone()],
            "case {case:?}: expected the attachment at {device:?}, got {body}"
        );
        attached.push((device.clone(), volume_id));
    }

    // The clock, not a caller, finishes the move: every attachment reads `attached` once
    // its span has passed, and the volume is still in use.
    past_the_transition().await;
    for (device, volume_id) in &attached {
        let body = described(&router, volume_id).await;
        let states = elements(&body, "status");
        assert!(
            states.contains(&VOLUME_STATE_IN_USE.to_string())
                && states.contains(&VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
            "case {device:?}: expected the volume {VOLUME_STATE_IN_USE} with its attachment \
             {VOLUME_ATTACHMENT_STATE_ATTACHED}, got {states:?} in {body}"
        );
        assert!(
            !states.contains(&VOLUME_ATTACHMENT_STATE_ATTACHING.to_string()),
            "case {device:?}: no attachment is left {VOLUME_ATTACHMENT_STATE_ATTACHING}, \
             got {states:?} in {body}"
        );
    }
}

/// RULE 2: every refusal, each with the code EC2 answers for what the request got wrong —
/// and nothing is attached by a refused call.
///
/// The rows span the two ids and both ways each can fail, the device name (blank and one
/// past the length bound), the EBS card index, the zone, the state of the volume, the state
/// of the instance, a volume an instance already holds and a device one already holds.
#[tokio::test]
async fn an_attach_is_refused_unless_the_request_and_the_state_behind_it_admit_it() {
    let (router, engine) = emulator();
    let instance_id = launch(&router, DEFAULT_INSTANCE_TYPE).await;
    // A second instance, so the "held by another instance" row names a real holder.
    let other_instance = launch(&router, DEFAULT_INSTANCE_TYPE).await;
    // A third, to be terminated below: an instance on its way out.
    let terminated = launch(&router, DEFAULT_INSTANCE_TYPE).await;

    let free = volume(&router).await;
    // In another zone of the same Region, so only the zone differs from the row above.
    let elsewhere = volume_in(&router, &zone_name(1), Vec::new()).await;
    // Held inside its `creating` -> `available` span, so the row reads the state it is
    // really in however long the table around it takes.
    let creating = volume(&router).await;
    hold_in_creating(&engine, &creating);
    // One this instance holds, and one the other instance holds.
    let mine = volume(&router).await;
    let theirs = volume(&router).await;

    // One wait for every move the calls above started: the instances are running and every
    // volume but the held one is available.
    past_the_transition().await;

    attach(
        &router,
        "seeding the volume this instance holds",
        &mine,
        &instance_id,
        "/dev/sdm",
    )
    .await;
    attach(
        &router,
        "seeding the volume the other instance holds",
        &theirs,
        &other_instance,
        "/dev/sdt",
    )
    .await;
    // An instance still coming up: launched last and not waited for.
    let pending = launch(&router, DEFAULT_INSTANCE_TYPE).await;
    let (status, body) = op(
        &router,
        "TerminateInstances",
        &indexed("InstanceId", &[&terminated]),
    )
    .await;
    assert_eq!(
        status, 200,
        "TerminateInstances must succeed, got {status} with body {body}"
    );

    // `(case, the parameters under test, the HTTP status, the error code)`.
    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a volume id that is not one",
            attach_params("vol-nothexdigits", &instance_id, "/dev/sdf"),
            400,
            "InvalidVolumeID.Malformed",
        ),
        (
            "an instance id that is not one",
            attach_params(&free, "i-nothexdigits", "/dev/sdf"),
            400,
            "InvalidInstanceID.Malformed",
        ),
        (
            // The DECODE half of the id's form question carries this operation's own code:
            // a value in the printed long form whose identifier the mint never handed out
            // is one the service will not take, where a value outside the printed form is
            // the id kind's own `InvalidVolumeID.Malformed` above.
            "a volume id whose long-form identifier the mint never handed out",
            attach_params(UNKNOWN_VOLUME, &instance_id, "/dev/sdf"),
            400,
            "InvalidParameterValue",
        ),
        (
            "a well-formed instance id the account holds no instance for",
            attach_params(&free, UNKNOWN_INSTANCE, "/dev/sdf"),
            400,
            "InvalidInstanceID.NotFound",
        ),
        (
            "no device name at all",
            attach_params(&free, &instance_id, ""),
            400,
            "InvalidParameterValue",
        ),
        (
            "a device name one past the length bound",
            attach_params(
                &free,
                &instance_id,
                &format!("/dev/{}", "d".repeat(MAX_DEVICE_NAME_CHARS - 4)),
            ),
            400,
            "InvalidParameterValue",
        ),
        (
            "an EBS card index below the first card",
            {
                let mut params = attach_params(&free, &instance_id, "/dev/sdf");
                params.push(p("EbsCardIndex", "-1"));
                params
            },
            400,
            "InvalidParameterValue",
        ),
        (
            "a volume in another Availability Zone",
            attach_params(&elsewhere, &instance_id, "/dev/sdf"),
            400,
            "InvalidVolume.ZoneMismatch",
        ),
        (
            "a volume the clock has not finished creating",
            attach_params(&creating, &instance_id, "/dev/sdf"),
            400,
            "IncorrectState",
        ),
        (
            "a volume this instance already holds",
            attach_params(&mine, &instance_id, "/dev/sdn"),
            400,
            "VolumeInUse",
        ),
        (
            "a volume another instance holds",
            attach_params(&theirs, &instance_id, "/dev/sdf"),
            400,
            "VolumeInUse",
        ),
        (
            "a device the instance already holds a volume at",
            attach_params(&free, &instance_id, "/dev/sdm"),
            400,
            "InvalidParameterValue",
        ),
        (
            "an instance that is still coming up",
            attach_params(&free, &pending, "/dev/sdf"),
            400,
            "IncorrectInstanceState",
        ),
        (
            "an instance that has been terminated",
            attach_params(&free, &terminated, "/dev/sdf"),
            400,
            "IncorrectInstanceState",
        ),
        (
            "a dry run of a request that would have been served",
            {
                let mut params = attach_params(&free, &instance_id, "/dev/sdf");
                params.push(p("DryRun", "true"));
                params
            },
            412,
            "DryRunOperation",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = op(&router, "AttachVolume", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        // Nothing was attached: the volume every row names is as it was before the call.
        let describe_body = described(&router, &free).await;
        let states = elements(&describe_body, "status");
        assert_eq!(
            states,
            vec![VOLUME_STATE_AVAILABLE.to_string()],
            "case {case:?}: the volume no refused attach touched is still \
             {VOLUME_STATE_AVAILABLE}, got {states:?} in {describe_body}"
        );
    }

    // The volume still being created is reported in that state, which is what the refusal
    // for it said — the message names the kind of resource, the id and the state.
    let (status, body) = op(
        &router,
        "AttachVolume",
        &attach_params(&creating, &instance_id, "/dev/sdf"),
    )
    .await;
    assert_eq!(
        (status, error_message(&body)),
        (
            400,
            format!("Volume '{creating}' is in the '{VOLUME_STATE_CREATING}' state.")
        ),
        "a volume still being created is refused for that state, got {status} with body {body}"
    );
}

/// RULE 2 does not refuse a STOPPED instance: "attaches an Amazon EBS volume to a running or
/// stopped instance" — both of those states are served, which is what tells this rule from
/// one about the instance being up.
#[tokio::test]
async fn a_stopped_instance_takes_an_attach() {
    let (router, _engine) = emulator();
    let instance_id = running_instance(&router).await;
    let volume_id = volume(&router).await;
    let (status, body) = op(
        &router,
        "StopInstances",
        &indexed("InstanceId", &[&instance_id]),
    )
    .await;
    assert_eq!(
        status, 200,
        "StopInstances must succeed, got {status} with body {body}"
    );
    // One wait for both moves: the instance reaches `stopped` and the volume `available`.
    past_the_transition().await;

    let body = attach(&router, "a stopped instance", &volume_id, &instance_id, "/dev/sdf").await;
    assert_eq!(
        element(&body, "status").as_deref(),
        Some(VOLUME_ATTACHMENT_STATE_ATTACHING),
        "a stopped instance takes the attach, got {body}"
    );
}

/// RULE 3: a Multi-Attach enabled volume is held by several instances at once — never twice
/// by the same one — and the clock-owed move of each attachment is materialised on the
/// record rather than only derived on read.
#[tokio::test]
async fn a_multi_attach_volume_is_held_by_several_instances_but_once_by_each() {
    let (router, engine) = emulator();
    let first = launch(&router, DEFAULT_INSTANCE_TYPE).await;
    let second = launch(&router, DEFAULT_INSTANCE_TYPE).await;

    // "This parameter is supported with io1 and io2 volumes only"
    // (`CreateVolumeRequest$MultiAttachEnabled`), and io2 requires its IOPS.
    let mut extra = vec![p("VolumeType", "io2"), p("MultiAttachEnabled", "true")];
    extra.extend(required_iops("io2"));
    let volume_id = volume_in(&router, &zone_name(0), extra).await;
    // One wait for the launches and the create alike.
    past_the_transition().await;

    attach(&router, "the first instance", &volume_id, &first, "/dev/sdf").await;
    assert_eq!(
        owed_attachments(&engine, &volume_id),
        vec![(first.clone(), "/dev/sdf".to_string())],
        "the first attach owes exactly its own move, got {:?}",
        owed_attachments(&engine, &volume_id)
    );
    past_the_transition().await;

    attach(&router, "the second instance", &volume_id, &second, "/dev/sdg").await;
    // The second attach is a writer, so it materialised the first attachment's finished
    // move: the record itself now reads `attached` there, and only the new attachment is
    // owed a move.
    assert_eq!(
        stored_attachments(&engine, &volume_id),
        vec![
            (
                "/dev/sdf".to_string(),
                VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()
            ),
            (
                "/dev/sdg".to_string(),
                VOLUME_ATTACHMENT_STATE_ATTACHING.to_string()
            ),
        ],
        "expected the finished attach stored as {VOLUME_ATTACHMENT_STATE_ATTACHED} beside \
         the new one, got {:?}",
        stored_attachments(&engine, &volume_id)
    );
    assert_eq!(
        owed_attachments(&engine, &volume_id),
        vec![(second.clone(), "/dev/sdg".to_string())],
        "only the new attachment is owed a move, got {:?}",
        owed_attachments(&engine, &volume_id)
    );

    // One instance holds one volume once, however many devices the request names.
    let (status, body) = op(
        &router,
        "AttachVolume",
        &attach_params(&volume_id, &first, "/dev/sdh"),
    )
    .await;
    assert_eq!(
        (status, error_code(&body).as_str()),
        (400, "VolumeInUse"),
        "attaching the same volume to the same instance twice: expected VolumeInUse, got \
         {status} with body {body}"
    );

    // Both instances are reported as holding it, and the volume is in use.
    let describe_body = described(&router, &volume_id).await;
    let holders = elements(&describe_body, "instanceId");
    assert!(
        holders.contains(&first) && holders.contains(&second),
        "expected both {first} and {second} to hold the volume, got {holders:?} in \
         {describe_body}"
    );
    assert!(
        elements(&describe_body, "status").contains(&VOLUME_STATE_IN_USE.to_string()),
        "the volume is {VOLUME_STATE_IN_USE}, got {describe_body}"
    );
}

/// RULE 3's bound: a Multi-Attach enabled volume is attached to as many instances as the
/// Amazon EBS User Guide publishes, and the one past that is refused with
/// `AttachmentLimitExceeded`.
#[tokio::test]
async fn a_multi_attach_volume_is_refused_past_its_published_instance_count() {
    let (router, _engine) = emulator();
    let mut extra = vec![p("VolumeType", "io1"), p("MultiAttachEnabled", "true")];
    extra.extend(required_iops("io1"));
    let volume_id = volume_in(&router, &zone_name(0), extra).await;

    let mut instances = Vec::new();
    for _ in 0..=MULTI_ATTACH_INSTANCE_LIMIT {
        instances.push(launch(&router, DEFAULT_INSTANCE_TYPE).await);
    }
    past_the_transition().await;

    for (index, instance_id) in instances.iter().enumerate() {
        let device = format!("/dev/sdm{index}");
        let (status, body) = op(
            &router,
            "AttachVolume",
            &attach_params(&volume_id, instance_id, &device),
        )
        .await;
        if index < MULTI_ATTACH_INSTANCE_LIMIT {
            assert_eq!(
                status, 200,
                "attachment {} of {MULTI_ATTACH_INSTANCE_LIMIT}: expected HTTP 200, got \
                 {status} with body {body}",
                index + 1
            );
        } else {
            assert_eq!(
                (status, error_code(&body).as_str()),
                (400, "AttachmentLimitExceeded"),
                "the attachment past {MULTI_ATTACH_INSTANCE_LIMIT}: expected \
                 AttachmentLimitExceeded, got {status} with body {body}"
            );
        }
    }
}

/// RULE 4: an instance holds as many EBS volumes as its instance type supports, and the
/// attach past that limit is refused with `AttachmentLimitExceeded`.
///
/// The rows are two instance types with DIFFERENT published limits, and the limit each row
/// is held to is the service's own [`ebs_attachment_limit`] — whose source is the type's
/// `EbsInfo$MaximumEbsAttachments` — rather than a number written out here. The instance's
/// EBS root volume is one of those attachments, so it is counted too.
#[tokio::test]
async fn an_instance_holds_as_many_volumes_as_its_instance_type_supports() {
    for instance_type in ["m1.small", "t3.micro"] {
        let (router, engine) = emulator();
        let limit = usize::try_from(
            ebs_attachment_limit(instance_type)
                .unwrap_or_else(|| panic!("case {instance_type:?}: the type must publish a limit")),
        )
        .expect("a positive published limit");
        let instance_id = launch(&router, instance_type).await;
        // Enough volumes to fill the type's limit and one past it, created before the one
        // wait so every `creating` -> `available` move falls due together.
        let created = volumes(&router, limit).await;
        past_the_transition().await;

        // The attachments the instance already has: the EBS root volume its launch created.
        let held = {
            let store = engine.state::<Ec2State>();
            let guard = store.lock();
            guard
                .instance_attachments_at(&instance_id, std::time::Instant::now())
                .len()
        };
        assert!(
            held >= 1,
            "case {instance_type:?}: a launched instance holds its root volume, got {held}"
        );

        for (offset, volume_id) in created.iter().enumerate() {
            // How many attachments the instance holds when this attach is made.
            let holding = held + offset;
            let device = format!("/dev/sdv{offset}");
            let (status, body) = op(
                &router,
                "AttachVolume",
                &attach_params(volume_id, &instance_id, &device),
            )
            .await;
            if holding < limit {
                assert_eq!(
                    status, 200,
                    "case {instance_type:?}: attachment {} of {limit}: expected HTTP 200, \
                     got {status} with body {body}",
                    holding + 1
                );
            } else {
                assert_eq!(
                    (status, error_code(&body).as_str()),
                    (400, "AttachmentLimitExceeded"),
                    "case {instance_type:?}: the attachment past {limit}: expected \
                     AttachmentLimitExceeded, got {status} with body {body}"
                );
            }
        }
    }
}
