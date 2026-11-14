// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteVolume`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. A DELETE REMOVES THE ONE VOLUME ITS `VolumeId` NAMES from the account's state, and
//!    nothing else: `DescribeVolumes` refuses that id with `InvalidVolume.NotFound`
//!    afterwards, a second delete of it is refused the same way rather than answering a
//!    second success, every other volume of the account is still reported, and what is
//!    reachable only THROUGH the deleted volume — the `ClientToken` it was minted under —
//!    goes with it;
//! 2. "The volume must be in the available state (not attached to an instance)"
//!    (`DeleteVolume`): a volume something still holds an attachment of is refused with
//!    `VolumeInUse` naming the volume and, as the brace-wrapped set the service renders them
//!    as, every holder that has to let go of it first, the volume stays stored, and the same
//!    delete succeeds once nothing holds it;
//! 3. an id the account holds no volume for is refused BEFORE anything is removed — a value
//!    outside the form EC2 issues ids in is a bad id (`InvalidVolumeID.Malformed`) and only
//!    a well-formed id no volume of the account has is `InvalidVolume.NotFound` — and a
//!    `DryRun` of a request that WOULD have succeeded answers `DryRunOperation` and removes
//!    nothing, while a dry run of a request that would not have succeeded is still refused
//!    for the reason it would have failed.
//!
//! Each table row is a distinct input along what the rules generalise over — the volume's
//! type, its size, whether it carries tags and a client token, the route it reached
//! `available` by, what holds the attachment and which member names it, and which way an id
//! can fail to name a volume — so no row is the specification.

use std::time::SystemTime;

use ec2::ops::types::AttachVolumeOutput;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_VOLUME_ID_MALFORMED,
    INVALID_VOLUME_ID_NOT_FOUND, VOLUME_ATTACHMENT_STATE_ATTACHED, VOLUME_IN_USE,
    VOLUME_STATE_AVAILABLE, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, error_message, indexed};
use super::fpga_image_tasks_and_volume_io::attach_in_store;
use super::volume_lifecycle::{REGION, is_volume_id, p, past_the_transition, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The HTTP status `DryRunOperation` carries: the request was refused because a
/// precondition the CALLER set holds, not because anything in it was wrong.
const DRY_RUN_STATUS: u16 = 412;

/// Well-formed instance ids (the long, 17 hex digit form EC2 mints today) for the
/// attachments rule 2 needs. `AttachVolume` is a scaffold still, so an attachment is
/// written into the store the way that operation will write it.
const INSTANCE_ID: &str = "i-0123456789abcdef0";
const OTHER_INSTANCE_ID: &str = "i-0fedcba987654321f";

/// One `action` call signed for the Region the volume suites work in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Mint one volume from `params` and answer its id.
async fn create_volume(router: &MultiServiceRouter, case: &str, params: &[(String, String)]) -> String {
    let (status, body) = op(router, "CreateVolume", params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
    );
    let volume_id = element(&body, "volumeId").unwrap_or_default();
    assert!(
        is_volume_id(&volume_id),
        "case {case:?}: expected a minted volume id, got {volume_id:?}"
    );
    volume_id
}

/// `DeleteVolume` naming `volume_id`, with whatever the case adds.
fn delete_params(volume_id: &str, extra: Params) -> Params {
    let mut params = vec![p("VolumeId", volume_id)];
    params.extend(extra);
    params
}

/// The ids `DescribeVolumes` reports for the whole account, in the order it reported them.
async fn all_stored_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = op(router, "DescribeVolumes", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeVolumes, got {status} with body {body}"
    );
    elements(&body, "volumeId")
}

/// Assert that `DescribeVolumes` still reports `volume_id`, and answer the state it reports
/// it in.
async fn reported_state(router: &MultiServiceRouter, case: &str, volume_id: &str) -> String {
    let (status, body) = op(router, "DescribeVolumes", &indexed("VolumeId", &[volume_id])).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 reading {volume_id} back, got {status} with body {body}"
    );
    assert_eq!(
        element(&body, "volumeId").as_deref(),
        Some(volume_id),
        "case {case:?}: expected DescribeVolumes to report {volume_id}, got {body}"
    );
    element(&body, "status").unwrap_or_default()
}

/// Assert that every read of `volume_id` answers the not-found refusal EC2 makes for a
/// volume the account does not have — which is what a delete leaves behind.
async fn is_gone(router: &MultiServiceRouter, case: &str, volume_id: &str) {
    for (action, params) in [
        ("DescribeVolumes", indexed("VolumeId", &[volume_id])),
        ("DeleteVolume", delete_params(volume_id, Vec::new())),
    ] {
        let (status, body) = op(router, action, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} from {action} of the deleted \
             {volume_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_VOLUME_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_VOLUME_ID_NOT_FOUND} from {action} of the deleted \
             {volume_id}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The volume '{volume_id}' does not exist."),
            "case {case:?}: expected the not-found wording from {action}, got {body}"
        );
    }
}

/// Give `volume_id` the attachment `attachment`, the way `AttachVolume` will. The
/// instance-and-device case is [`attach_in_store`]; this one takes the whole attachment, for
/// the attachments to an Amazon Web Services-managed resource rule 2 also spans.
fn push_attachment(engine: &engine::InMemoryEngine, volume_id: &str, attachment: AttachVolumeOutput) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    guard
        .volumes
        .get_mut(volume_id)
        .expect("the volume just created")
        .view
        .attachments
        .get_or_insert_with(Vec::new)
        .push(attachment);
}

/// An attachment of `volume_id` to an Amazon Web Services-managed resource: the instance
/// members are null ("If the volume is attached to an Amazon Web Services-managed resource,
/// this parameter returns null", `VolumeAttachment$InstanceId`) and the managed resource is
/// named by whichever of the two markers the case sets.
fn managed_attachment(
    volume_id: &str,
    associated_resource: Option<&str>,
    instance_owning_service: Option<&str>,
) -> AttachVolumeOutput {
    AttachVolumeOutput {
        volume_id: Some(volume_id.to_string()),
        instance_id: None,
        device: None,
        state: Some(VOLUME_ATTACHMENT_STATE_ATTACHED.to_string()),
        attach_time: Some(system_time_to_date_time(SystemTime::now())),
        delete_on_termination: Some(false),
        associated_resource: associated_resource.map(str::to_string),
        instance_owning_service: instance_owning_service.map(str::to_string),
        ebs_card_index: Some(0),
    }
}

/// One row of the delete table: the case, the `CreateVolume` members that mint the volume,
/// the `ClientToken` it was minted under where the row has one, and whether the volume
/// reached `available` by having an attachment detached rather than by being created into
/// it.
struct Row {
    case: &'static str,
    create: Params,
    client_token: Option<&'static str>,
    detached_first: bool,
}

/// RULE 1: a delete removes the ONE volume its `VolumeId` names — the id is unreadable
/// afterwards, a second delete of it is refused rather than answering success, the account's
/// other volumes are untouched, and the `ClientToken` the volume was minted under goes with
/// it. RULE 3's dry-run half rides along: the probe answers `DryRunOperation` and the volume
/// is still there, so the delete below is what removed it.
///
/// The rows differ along the volume's type, its size, whether it carries tags and a client
/// token, and the ROUTE it reached `available` by (created into it, or by a detach the clock
/// has since finished), so no one of them is the rule.
#[tokio::test]
async fn a_delete_removes_the_volume_it_names_and_leaves_the_others_stored() {
    let (router, engine) = emulator();

    let rows = [
        Row {
            case: "a gp3 volume at the bottom of its size range",
            create: vec![p("VolumeType", "gp3"), p("Size", "1")],
            client_token: None,
            detached_first: false,
        },
        Row {
            case: "a standard volume of 8 GiB",
            create: vec![p("VolumeType", "standard"), p("Size", "8")],
            client_token: None,
            detached_first: false,
        },
        Row {
            case: "a tagged io2 volume minted under a client token",
            create: vec![
                p("VolumeType", "io2"),
                p("Size", "500"),
                // "Required for io1 and io2 volumes" (`CreateVolumeRequest$Iops`).
                p("Iops", "100"),
                p("TagSpecification.1.ResourceType", "volume"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "doomed"),
            ],
            client_token: Some("delete-volume-token"),
            detached_first: true,
        },
        Row {
            case: "a gp2 volume whose detach the clock has finished",
            create: vec![p("VolumeType", "gp2"), p("Size", "40")],
            client_token: None,
            detached_first: true,
        },
    ];

    // The volume no row names: it must still be reported after every delete, so a delete
    // that removed more than the id it was given cannot pass.
    let sentinel = create_volume(
        &router,
        "the sentinel volume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", "4")],
    )
    .await;

    let mut targets = Vec::new();
    for row in &rows {
        let mut params = vec![p("AvailabilityZone", zone_name(0))];
        params.extend(row.create.iter().cloned());
        if let Some(token) = row.client_token {
            params.push(p("ClientToken", token));
        }
        let volume_id = create_volume(&router, row.case, &params).await;
        if row.detached_first {
            attach_in_store(&engine, &volume_id, INSTANCE_ID, "/dev/sdf");
            let (status, body) =
                op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
            assert_eq!(
                status, 200,
                "case {}: expected HTTP 200 seeding the detach, got {status} with body {body}",
                row.case
            );
        }
        targets.push(volume_id);
    }

    // One wait for the whole table: every clock-owed move started above — the creations and
    // the detaches — has fallen due by now, so each row's volume is `available`.
    past_the_transition().await;

    for (row, volume_id) in rows.iter().zip(&targets) {
        let case = row.case;
        let state = reported_state(&router, case, volume_id).await;
        assert_eq!(
            state, VOLUME_STATE_AVAILABLE,
            "case {case:?}: expected {volume_id} to be {VOLUME_STATE_AVAILABLE} before the \
             delete, got {state:?}"
        );

        // RULE 3, the dry-run half: "If you have the required permissions, the error
        // response is DryRunOperation" (`DryRun`), and the volume is still readable after it.
        let (status, body) = op(
            &router,
            "DeleteVolume",
            &delete_params(volume_id, vec![p("DryRun", "true")]),
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry-run delete of \
             {volume_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION} from the dry run, got {body}"
        );
        assert_eq!(
            error_message(&body),
            "Request would have succeeded, but DryRun flag is set.",
            "case {case:?}: expected the dry-run wording, got {body}"
        );
        let after_probe = reported_state(&router, case, volume_id).await;
        assert_eq!(
            after_probe, VOLUME_STATE_AVAILABLE,
            "case {case:?}: a dry run performs nothing, so {volume_id} must still be \
             {VOLUME_STATE_AVAILABLE}, got {after_probe:?}"
        );

        let (status, body) = op(&router, "DeleteVolume", &delete_params(volume_id, Vec::new())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {volume_id}, got {status} with body {body}"
        );

        // ROUND TRIP: the id is gone from the store — every read of it, and a second delete,
        // answer the not-found refusal.
        is_gone(&router, case, volume_id).await;

        // And nothing else went with it: the sentinel and every row not yet deleted are
        // still reported.
        let stored = all_stored_ids(&router, case).await;
        assert!(
            stored.contains(&sentinel),
            "case {case:?}: expected the sentinel {sentinel} to survive the delete of \
             {volume_id}, got {stored:?}"
        );
        assert!(
            !stored.contains(volume_id),
            "case {case:?}: expected {volume_id} to be gone from the account's volumes, got \
             {stored:?}"
        );

        // What is reachable only THROUGH the volume goes with it: the token that minted it
        // names no volume any more, so it mints a NEW one rather than reporting the deleted
        // id.
        if let Some(token) = row.client_token {
            let minted = create_volume(
                &router,
                case,
                &[
                    p("AvailabilityZone", zone_name(0)),
                    p("Size", "4"),
                    p("ClientToken", token),
                ],
            )
            .await;
            assert_ne!(
                &minted, volume_id,
                "case {case:?}: the client token {token} must not report the volume the \
                 delete removed"
            );
        }
    }
}

/// RULE 2: a volume something still holds an attachment of cannot be deleted — the refusal
/// is `VolumeInUse` naming the volume and its holders as a brace-wrapped set, the volume is
/// still stored, and the same delete succeeds once the attachment is gone.
///
/// The rows differ along WHAT holds the volume and which member names it: an instance, a
/// second instance at another device, and an attachment to an Amazon Web Services-managed
/// resource under each of the two markers that name one.
#[tokio::test]
async fn a_volume_something_still_holds_cannot_be_deleted() {
    let (router, engine) = emulator();

    // (case, how the attachment is seeded, the holder the refusal must name, whether a
    // `DetachVolume` can release it).
    #[allow(clippy::type_complexity)]
    let rows: Vec<(&str, Box<dyn Fn(&engine::InMemoryEngine, &str)>, String, bool)> = vec![
        (
            "attached to an instance",
            Box::new(|engine, volume_id| attach_in_store(engine, volume_id, INSTANCE_ID, "/dev/sdf")),
            INSTANCE_ID.to_string(),
            true,
        ),
        (
            "attached to another instance at another device",
            Box::new(|engine, volume_id| {
                attach_in_store(engine, volume_id, OTHER_INSTANCE_ID, "/dev/xvdb")
            }),
            OTHER_INSTANCE_ID.to_string(),
            true,
        ),
        (
            "attached to a managed resource named by AssociatedResource",
            Box::new(|engine, volume_id| {
                let attachment = managed_attachment(
                    volume_id,
                    Some("arn:aws:fsx:us-west-2:123456789012:file-system/fs-0123456789abcdef0"),
                    None,
                );
                push_attachment(engine, volume_id, attachment);
            }),
            "arn:aws:fsx:us-west-2:123456789012:file-system/fs-0123456789abcdef0".to_string(),
            false,
        ),
        (
            "attached to a managed resource named by InstanceOwningService",
            Box::new(|engine, volume_id| {
                let attachment = managed_attachment(volume_id, None, Some("fsx.amazonaws.com"));
                push_attachment(engine, volume_id, attachment);
            }),
            "fsx.amazonaws.com".to_string(),
            false,
        ),
    ];

    let mut releasable = Vec::new();
    for (case, seed, holder, can_detach) in &rows {
        let volume_id = create_volume(
            &router,
            case,
            &[p("AvailabilityZone", zone_name(0)), p("Size", "8")],
        )
        .await;
        seed(&engine, &volume_id);

        let (status, body) = op(&router, "DeleteVolume", &delete_params(&volume_id, Vec::new())).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} deleting the held {volume_id}, got \
             {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            VOLUME_IN_USE,
            "case {case:?}: expected {VOLUME_IN_USE} deleting the held {volume_id}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Volume {volume_id} is currently attached to {{{holder}}}"),
            "case {case:?}: expected the refusal to name the volume and its holder set, got \
             {body}"
        );

        // Nothing was removed: the volume is still the account's.
        let stored = all_stored_ids(&router, case).await;
        assert!(
            stored.contains(&volume_id),
            "case {case:?}: a refused delete removes nothing, so {volume_id} must still be \
             stored, got {stored:?}"
        );

        // A dry run of a request that would NOT have succeeded is refused for the reason it
        // would have failed, not with `DryRunOperation`.
        let (status, body) = op(
            &router,
            "DeleteVolume",
            &delete_params(&volume_id, vec![p("DryRun", "true")]),
        )
        .await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} from a dry-run delete of the held \
             {volume_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            VOLUME_IN_USE,
            "case {case:?}: expected {VOLUME_IN_USE} from the dry run of a delete that could \
             not have succeeded, got {body}"
        );

        if *can_detach {
            let (status, body) = op(&router, "DetachVolume", &[p("VolumeId", &volume_id)]).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 detaching {volume_id}, got {status} with \
                 body {body}"
            );
            releasable.push((*case, volume_id));
        }
    }

    // The clock finishes the detaches, and only then is the volume deletable.
    past_the_transition().await;
    for (case, volume_id) in &releasable {
        let (status, body) = op(&router, "DeleteVolume", &delete_params(volume_id, Vec::new())).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting the detached {volume_id}, got {status} \
             with body {body}"
        );
        is_gone(&router, case, volume_id).await;
    }
}

/// RULE 3: an id the account holds no volume for is refused before anything is removed, and
/// WHICH refusal says so is a fact of the id's form — a value outside the form EC2 issues
/// ids in is a bad id, and only a well-formed id no volume of the account has is not found.
///
/// The rows differ along the way the id fails to name a volume: a suffix of neither
/// published length, a non-hexadecimal digit, an upper-case digit, another resource's
/// prefix, the prefix alone, no prefix at all, and the two well-formed lengths EC2 issues.
#[tokio::test]
async fn an_id_that_names_no_volume_is_refused_and_removes_nothing() {
    let (router, _engine) = emulator();

    // The one volume the account has: it must still be stored after every refused delete.
    let stored_volume = create_volume(
        &router,
        "the stored volume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", "8")],
    )
    .await;

    // (case, the id the request names, the code, the message).
    let rows: Vec<(&str, &str, &str)> = vec![
        ("a suffix of neither published length", "vol-012345", INVALID_VOLUME_ID_MALFORMED),
        (
            "a suffix one digit too long",
            "vol-04fd27e9e8a1b2c3de",
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "a non-hexadecimal digit",
            "vol-04fd27e9e8a1b2c3z",
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "an upper-case digit",
            "vol-04FD27E9E8A1B2C3D",
            INVALID_VOLUME_ID_MALFORMED,
        ),
        (
            "another resource's prefix",
            "snap-04fd27e9e8a1b2c3d",
            INVALID_VOLUME_ID_MALFORMED,
        ),
        ("the prefix alone", "vol-", INVALID_VOLUME_ID_MALFORMED),
        ("no prefix at all", "not-a-volume", INVALID_VOLUME_ID_MALFORMED),
        (
            "a well-formed long id nothing was created for",
            "vol-04fd27e9e8a1b2c3d",
            INVALID_VOLUME_ID_NOT_FOUND,
        ),
        (
            "a well-formed short id nothing was created for",
            "vol-04fd27e9",
            INVALID_VOLUME_ID_NOT_FOUND,
        ),
    ];

    for (case, volume_id, expected_code) in rows {
        let expected_message = if expected_code == INVALID_VOLUME_ID_MALFORMED {
            // The sentence AWS words an unreadable VOLUME id with — the kind has one of its
            // own rather than the quoted template other id kinds take.
            format!("The volume ID '{volume_id}' is malformed")
        } else {
            format!("The volume '{volume_id}' does not exist.")
        };

        // The same answer whether or not the request is a dry run: the id is resolved before
        // the permission probe, so a request that could not have succeeded never reports
        // that it would have.
        for extra in [Vec::new(), vec![p("DryRun", "true")]] {
            let probe = if extra.is_empty() { "a delete" } else { "a dry-run delete" };
            let (status, body) =
                op(&router, "DeleteVolume", &delete_params(volume_id, extra)).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} from {probe} of {volume_id:?}, \
                 got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code} from {probe} of {volume_id:?}, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the refusal wording for {volume_id:?}, got {body}"
            );
        }

        let stored = all_stored_ids(&router, case).await;
        assert_eq!(
            stored,
            vec![stored_volume.clone()],
            "case {case:?}: a refused delete removes nothing, so the account's volumes must \
             be unchanged, got {stored:?}"
        );
    }
}
