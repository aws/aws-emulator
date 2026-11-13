// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `DeleteSnapshot`, driven over the wire the service actually speaks
//! — `ec2Query`, through the crate's shared [`super::ec2_query`] harness — and, for the
//! snapshots the EBS direct APIs mint, over that service's own restJson1 wire on the same
//! engine ([`super::ebs_direct_api_snapshots`]).
//!
//! The rules under test are:
//!
//! 1. A DELETE REMOVES THE ONE SNAPSHOT ITS `SnapshotId` NAMES from the account's state,
//!    whichever of the two APIs that describe the account's snapshots minted it, and
//!    nothing else: `DescribeSnapshots` refuses that id with `InvalidSnapshot.NotFound`
//!    afterwards, a second delete of it is refused the same way rather than answering a
//!    second success, and every other snapshot of the account is still reported.
//! 2. An id the account holds no snapshot for is refused BEFORE anything is removed — a
//!    value outside the form EC2 issues ids in is a bad id (`InvalidSnapshotID.Malformed`)
//!    and only a well-formed id no snapshot of the account has is `InvalidSnapshot.NotFound`
//!    — and a `DryRun` of a request that WOULD have succeeded answers `DryRunOperation` and
//!    removes nothing.
//! 3. "You cannot delete a snapshot of the root device of an EBS volume used by a registered
//!    AMI. You must first deregister the AMI before you can delete the snapshot"
//!    (`DeleteSnapshot`): the delete is refused `InvalidSnapshot.InUse` naming the image,
//!    and the snapshot is still the account's afterwards.
//! 4. The delete is what the REST of the lifecycle then sees: a deleted snapshot's id names
//!    nothing, so `ebs:StartSnapshot` with it as `ParentSnapshotId` is refused the modelled
//!    `ResourceNotFoundException` — the boundary no fabricated id can reach, since a
//!    fabricated one fails `ParentSnapshotId`'s own constraints instead.
//!
//! Each table row is a distinct input along what the rules generalise over — which API
//! minted the snapshot, the volume size it is of, whether it carries a description and tags,
//! how many blocks were written into it, which phase of its lifecycle the delete lands in,
//! and which way an id can fail to name a snapshot — so no row is the specification.

use std::sync::Arc;

use ec2::ops::types::{BlockDeviceMapping, EbsBlockDevice};
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, DRY_RUN_OPERATION, INVALID_SNAPSHOT_ID_MALFORMED,
    INVALID_SNAPSHOT_ID_NOT_FOUND, INVALID_SNAPSHOT_IN_USE,
};
use serde_json::json;
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ebs_direct_api_snapshots::{complete_snapshot, ebs_send, put_block, start_snapshot};
use super::ec2_query::{call, element, elements, error_code, error_message, indexed};
use super::volume_lifecycle::{p, zone_name, REGION};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// One row of the delete table: the case, how the snapshot is minted, and the members the
/// request that minted it carried.
type DeleteRow = (&'static str, Minted, Params);

/// The HTTP status EC2 answers a refusal about the request with.
const REFUSAL_STATUS: u16 = 400;

/// The HTTP status `DryRunOperation` carries: the request was refused because a precondition
/// the CALLER set holds, not because anything in it was wrong.
const DRY_RUN_STATUS: u16 = 412;

/// The HTTP status the EBS direct APIs' modelled `ResourceNotFoundException` carries
/// (`@httpError 404`).
const EBS_NOT_FOUND_STATUS: u16 = 404;

/// Which API minted the snapshot a row deletes: the two sides of the one snapshot resource
/// this account has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Minted {
    /// `ec2:CreateSnapshot`, of a volume of the account.
    ByEc2,
    /// `ebs:StartSnapshot`, sealed by `ebs:CompleteSnapshot` once the row's blocks are
    /// written.
    ByDirectApi { blocks: i32, sealed: bool },
}

/// Both services on ONE engine, which is what makes the two sides of the snapshot resource
/// the same resource.
fn emulator() -> (MultiServiceRouter, Arc<engine::InMemoryEngine>) {
    super::ebs_direct_api_snapshots::emulator()
}

/// One `action` call signed for the Region the snapshot suites work in.
async fn op(router: &MultiServiceRouter, action: &str, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, action, params).await
}

/// Mint one volume of `size` GiB and answer its id, for the rows `ec2:CreateSnapshot` takes
/// a snapshot of.
async fn create_volume(router: &MultiServiceRouter, case: &str, size: i32) -> String {
    let (status, body) = op(
        router,
        "CreateVolume",
        &[p("AvailabilityZone", zone_name(0)), p("Size", size.to_string())],
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from CreateVolume, got {status} with body {body}"
    );
    element(&body, "volumeId").unwrap_or_else(|| panic!("case {case:?}: no volumeId in {body}"))
}

/// Mint the snapshot a row deletes, through the API the row names, and answer its id.
async fn mint_snapshot(
    router: &MultiServiceRouter,
    case: &str,
    minted: Minted,
    request: &Params,
) -> String {
    match minted {
        Minted::ByEc2 => {
            // "The ID of the Amazon EBS volume" (`CreateSnapshotRequest$VolumeId`): the
            // snapshot is OF a volume, so one is created for it first.
            let volume_id = create_volume(router, case, 1).await;
            let mut params = vec![p("VolumeId", &volume_id)];
            params.extend(request.iter().cloned());
            let (status, body) = op(router, "CreateSnapshot", &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200 from CreateSnapshot, got {status} with body \
                 {body}"
            );
            element(&body, "snapshotId")
                .unwrap_or_else(|| panic!("case {case:?}: no snapshotId in {body}"))
        }
        Minted::ByDirectApi { blocks, sealed } => {
            let mut body = json!({ "VolumeSize": 1 });
            for (name, value) in request {
                body[name.clone()] = json!(value);
            }
            let snapshot_id = start_snapshot(router, case, body).await;
            for index in 0..blocks {
                put_block(router, case, &snapshot_id, index).await;
            }
            if sealed {
                complete_snapshot(router, case, &snapshot_id, blocks).await;
            }
            snapshot_id
        }
    }
}

/// The ids `DescribeSnapshots` reports for the snapshots this account owns, in report order.
async fn account_snapshot_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = op(router, "DescribeSnapshots", &[p("Owner.1", "self")]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    elements(&body, "snapshotId")
}

/// Assert that every read of `snapshot_id` answers the not-found refusal EC2 makes for a
/// snapshot the account does not have — which is what a delete leaves behind.
async fn is_gone(router: &MultiServiceRouter, case: &str, snapshot_id: &str) {
    for (action, params) in [
        ("DescribeSnapshots", indexed("SnapshotId", &[snapshot_id])),
        ("DeleteSnapshot", vec![p("SnapshotId", snapshot_id)]),
    ] {
        let (status, body) = op(router, action, &params).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} from {action} of the deleted \
             {snapshot_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_SNAPSHOT_ID_NOT_FOUND,
            "case {case:?}: expected {INVALID_SNAPSHOT_ID_NOT_FOUND} from {action} of the \
             deleted {snapshot_id}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The snapshot '{snapshot_id}' does not exist."),
            "case {case:?}: expected the not-found wording from {action}, got {body}"
        );
    }
}

/// RULE 1 and RULE 2's dry-run half: a delete removes the ONE snapshot its id names,
/// whichever API minted it — the id is unreadable afterwards, a second delete of it is
/// refused rather than answering success, and the account's other snapshots are untouched.
///
/// The rows differ along which API minted the snapshot, whether it carries a description and
/// tags, how many blocks were written into it, and whether its own API had sealed it yet, so
/// no one of them is the rule.
#[tokio::test]
async fn a_delete_removes_the_snapshot_it_names_and_leaves_the_others_stored() {
    let (router, _engine) = emulator();

    let rows: Vec<DeleteRow> = vec![
        (
            "a snapshot EC2 took of a volume",
            Minted::ByEc2,
            Vec::new(),
        ),
        (
            "a described and tagged snapshot EC2 took of a volume",
            Minted::ByEc2,
            vec![
                p("Description", "deleted by the delete suite"),
                p("TagSpecification.1.ResourceType", "snapshot"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "doomed"),
            ],
        ),
        (
            "a sealed direct-API snapshot with one block written",
            Minted::ByDirectApi {
                blocks: 1,
                sealed: true,
            },
            Vec::new(),
        ),
        (
            "a sealed, described direct-API snapshot with two blocks written",
            Minted::ByDirectApi {
                blocks: 2,
                sealed: true,
            },
            vec![("Description".to_string(), "the direct-API one".to_string())],
        ),
        (
            "a direct-API snapshot nothing has sealed yet",
            Minted::ByDirectApi {
                blocks: 0,
                sealed: false,
            },
            Vec::new(),
        ),
    ];

    // The snapshot no row names: it must still be reported after every delete, so a delete
    // that removed more than the id it was given cannot pass.
    let sentinel = mint_snapshot(&router, "the sentinel snapshot", Minted::ByEc2, &Vec::new()).await;
    let direct_api_sentinel = mint_snapshot(
        &router,
        "the direct-API sentinel snapshot",
        Minted::ByDirectApi {
            blocks: 1,
            sealed: true,
        },
        &Vec::new(),
    )
    .await;

    for (case, minted, request) in &rows {
        let snapshot_id = mint_snapshot(&router, case, *minted, request).await;

        // It is the account's snapshot before the delete, whichever side holds it.
        let stored = account_snapshot_ids(&router, case).await;
        assert!(
            stored.contains(&snapshot_id),
            "case {case:?}: expected {snapshot_id} to be reported before the delete, got \
             {stored:?}"
        );

        // RULE 2, the dry-run half: "If you have the required permissions, the error
        // response is DryRunOperation" (`DryRun`), and the snapshot is still readable after
        // it.
        let (status, body) = op(
            &router,
            "DeleteSnapshot",
            &[p("SnapshotId", &snapshot_id), p("DryRun", "true")],
        )
        .await;
        assert_eq!(
            status, DRY_RUN_STATUS,
            "case {case:?}: expected HTTP {DRY_RUN_STATUS} from a dry-run delete of \
             {snapshot_id}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            DRY_RUN_OPERATION,
            "case {case:?}: expected {DRY_RUN_OPERATION} from the dry run, got {body}"
        );
        let after_probe = account_snapshot_ids(&router, case).await;
        assert!(
            after_probe.contains(&snapshot_id),
            "case {case:?}: a dry run performs nothing, so {snapshot_id} must still be \
             reported, got {after_probe:?}"
        );

        let (status, body) = op(&router, "DeleteSnapshot", &[p("SnapshotId", &snapshot_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {snapshot_id}, got {status} with body \
             {body}"
        );

        // ROUND TRIP: the id is gone from the account's state — every read of it, and a
        // second delete, answer the not-found refusal.
        is_gone(&router, case, &snapshot_id).await;

        // And nothing else went with it: both sentinels are still reported.
        let stored = account_snapshot_ids(&router, case).await;
        for survivor in [&sentinel, &direct_api_sentinel] {
            assert!(
                stored.contains(survivor),
                "case {case:?}: expected the sentinel {survivor} to survive the delete of \
                 {snapshot_id}, got {stored:?}"
            );
        }
        assert!(
            !stored.contains(&snapshot_id),
            "case {case:?}: expected {snapshot_id} to be gone from the account's snapshots, \
             got {stored:?}"
        );
    }
}

/// RULE 2: an id the account holds no snapshot for is refused before anything is removed,
/// and WHICH refusal says so is a fact of the id's form.
///
/// The rows differ along the way the id fails to name a snapshot: a suffix of neither
/// published length, a non-hexadecimal digit, an upper-case digit, another resource's prefix,
/// the prefix alone, no prefix at all, the all-zero id, and the two well-formed lengths EC2
/// issues.
#[tokio::test]
async fn an_id_that_names_no_snapshot_is_refused_and_removes_nothing() {
    let (router, _engine) = emulator();

    // The one snapshot the account has: it must still be stored after every refused delete.
    let stored_snapshot =
        mint_snapshot(&router, "the stored snapshot", Minted::ByEc2, &Vec::new()).await;

    // (case, the id the request names, the code the refusal carries).
    let rows: Vec<(&str, &str, &str)> = vec![
        (
            "a suffix of neither published length",
            "snap-012345",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "a suffix one digit too long",
            "snap-04fd27e9e8a1b2c3de",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "a non-hexadecimal digit",
            "snap-04fd27e9e8a1b2c3z",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "an upper-case digit",
            "snap-04FD27E9E8A1B2C3D",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "another resource's prefix",
            "vol-04fd27e9e8a1b2c3d",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        ("the prefix alone", "snap-", INVALID_SNAPSHOT_ID_MALFORMED),
        (
            "no prefix at all",
            "not-a-snapshot",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "the all-zero id of a published length",
            "snap-00000000000000000",
            INVALID_SNAPSHOT_ID_MALFORMED,
        ),
        (
            "a well-formed long id nothing was created for",
            "snap-04fd27e9e8a1b2c3d",
            INVALID_SNAPSHOT_ID_NOT_FOUND,
        ),
        (
            "a well-formed short id nothing was created for",
            "snap-04fd27e9",
            INVALID_SNAPSHOT_ID_NOT_FOUND,
        ),
    ];

    for (case, snapshot_id, expected_code) in rows {
        let expected_message = if expected_code == INVALID_SNAPSHOT_ID_MALFORMED {
            format!("Value ({snapshot_id}) for snapshot ID is invalid.")
        } else {
            format!("The snapshot '{snapshot_id}' does not exist.")
        };

        // The same answer whether or not the request is a dry run: the id is resolved before
        // the permission probe, so a request that could not have succeeded never reports that
        // it would have.
        for extra in [Vec::new(), vec![p("DryRun", "true")]] {
            let probe = if extra.is_empty() {
                "a delete"
            } else {
                "a dry-run delete"
            };
            let mut params = vec![p("SnapshotId", snapshot_id)];
            params.extend(extra);
            let (status, body) = op(&router, "DeleteSnapshot", &params).await;
            assert_eq!(
                status, REFUSAL_STATUS,
                "case {case:?}: expected HTTP {REFUSAL_STATUS} from {probe} of \
                 {snapshot_id:?}, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                expected_code,
                "case {case:?}: expected {expected_code} from {probe} of {snapshot_id:?}, got \
                 {body}"
            );
            assert_eq!(
                error_message(&body),
                expected_message,
                "case {case:?}: expected the refusal wording for {snapshot_id:?}, got {body}"
            );
        }

        let stored = account_snapshot_ids(&router, case).await;
        assert_eq!(
            stored,
            vec![stored_snapshot.clone()],
            "case {case:?}: a refused delete removes nothing, so the account's snapshots must \
             be unchanged, got {stored:?}"
        );
    }
}

/// RULE 3: a snapshot a registered AMI restores a device from cannot be deleted — the
/// refusal is `InvalidSnapshot.InUse` naming the snapshot and the image to deregister first,
/// the snapshot is still stored, and the same delete succeeds once no image names it.
///
/// `RegisterImage` and `CreateImage` are still scaffolds, so the mapping that makes the image
/// depend on the snapshot is written into the store the way those operations will write it.
/// The rows differ along which device of the image the snapshot backs (the root device and a
/// second data device) and along which API minted the snapshot.
#[tokio::test]
async fn a_snapshot_a_registered_image_uses_cannot_be_deleted() {
    let (router, engine) = emulator();

    // (case, how the snapshot is minted, the device name the image maps it at).
    let rows: Vec<(&str, Minted, &str)> = vec![
        ("the root device of the image", Minted::ByEc2, "/dev/xvda"),
        (
            "a data device of the image",
            Minted::ByEc2,
            "/dev/sdb",
        ),
        (
            "a direct-API snapshot the image restores a device from",
            Minted::ByDirectApi {
                blocks: 1,
                sealed: true,
            },
            "/dev/xvdf",
        ),
    ];

    for (case, minted, device_name) in rows {
        let snapshot_id = mint_snapshot(&router, case, minted, &Vec::new()).await;
        let image_id = map_snapshot_onto_an_image(&engine, case, device_name, &snapshot_id);

        let (status, body) = op(&router, "DeleteSnapshot", &[p("SnapshotId", &snapshot_id)]).await;
        assert_eq!(
            status, REFUSAL_STATUS,
            "case {case:?}: expected HTTP {REFUSAL_STATUS} deleting the in-use {snapshot_id}, \
             got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            INVALID_SNAPSHOT_IN_USE,
            "case {case:?}: expected {INVALID_SNAPSHOT_IN_USE} deleting the in-use \
             {snapshot_id}, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("The snapshot {snapshot_id} is currently in use by {image_id}"),
            "case {case:?}: expected the refusal to name the snapshot and the image, got {body}"
        );

        // Nothing was removed: the snapshot is still the account's.
        let stored = account_snapshot_ids(&router, case).await;
        assert!(
            stored.contains(&snapshot_id),
            "case {case:?}: a refused delete removes nothing, so {snapshot_id} must still be \
             reported, got {stored:?}"
        );

        // Once the image no longer names it — what deregistering the AMI leaves behind — the
        // same delete succeeds.
        unmap_snapshot_from_images(&engine, &snapshot_id);
        let (status, body) = op(&router, "DeleteSnapshot", &[p("SnapshotId", &snapshot_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting {snapshot_id} once no image uses it, \
             got {status} with body {body}"
        );
        is_gone(&router, case, &snapshot_id).await;
    }
}

/// Give one registered AMI of the account a block device mapping at `device_name` that
/// restores from `snapshot_id`, and answer that image's id.
fn map_snapshot_onto_an_image(
    engine: &engine::InMemoryEngine,
    case: &str,
    device_name: &str,
    snapshot_id: &str,
) -> String {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    let image_id = guard
        .images
        .keys()
        .next()
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: the account holds no registered image"));
    let image = guard
        .images
        .get_mut(&image_id)
        .expect("the image just resolved");
    image
        .block_device_mappings
        .get_or_insert_with(Vec::new)
        .push(BlockDeviceMapping {
            device_name: Some(device_name.to_string()),
            ebs: Some(EbsBlockDevice {
                snapshot_id: Some(snapshot_id.to_string()),
                ..EbsBlockDevice::default()
            }),
            ..BlockDeviceMapping::default()
        });
    image_id
}

/// Take `snapshot_id` out of every image's block device mappings, which is the state
/// deregistering the AMI that used it leaves behind.
fn unmap_snapshot_from_images(engine: &engine::InMemoryEngine, snapshot_id: &str) {
    let store = engine.state::<Ec2State>();
    let mut guard = store.lock();
    for image in guard.images.values_mut() {
        if let Some(mappings) = image.block_device_mappings.as_mut() {
            mappings.retain(|mapping| {
                mapping
                    .ebs
                    .as_ref()
                    .is_none_or(|ebs| ebs.snapshot_id.as_deref() != Some(snapshot_id))
            });
        }
    }
}

/// RULE 4: the delete is what the rest of the lifecycle then sees — `ebs:StartSnapshot` with
/// the deleted id as `ParentSnapshotId` is refused the modelled `ResourceNotFoundException`,
/// while the same call against a snapshot that is still there succeeds.
///
/// The rows differ along how many blocks the deleted parent held and whether anything was
/// written into it at all, so no row is the rule.
#[tokio::test]
async fn a_deleted_snapshot_can_no_longer_be_a_parent() {
    let (router, _engine) = emulator();

    for (case, blocks) in [
        ("a parent with no block written", 0),
        ("a parent with one block written", 1),
        ("a parent with two blocks written", 2),
    ] {
        let parent = mint_snapshot(
            &router,
            case,
            Minted::ByDirectApi {
                blocks,
                sealed: true,
            },
            &Vec::new(),
        )
        .await;

        // While it is there, it is a parent a new snapshot can be taken from.
        let child = start_snapshot(
            &router,
            case,
            json!({ "VolumeSize": 1, "ParentSnapshotId": parent.clone() }),
        )
        .await;
        assert_ne!(
            child, parent,
            "case {case:?}: StartSnapshot must mint a new id for the child"
        );

        let (status, body) = op(&router, "DeleteSnapshot", &[p("SnapshotId", &parent)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 deleting the parent {parent}, got {status} with \
             body {body}"
        );

        // And once it is gone, the SAME well-formed id names no snapshot: the modelled
        // not-found refusal, not a validation failure about the id's form.
        let (status, answer) = ebs_send(
            &router,
            "POST",
            "/snapshots",
            &[("content-type", "application/json".to_string())],
            serde_json::to_vec(&json!({ "VolumeSize": 1, "ParentSnapshotId": parent.clone() }))
                .expect("a serializable StartSnapshot body"),
        )
        .await;
        assert_eq!(
            status, EBS_NOT_FOUND_STATUS,
            "case {case:?}: expected HTTP {EBS_NOT_FOUND_STATUS} from StartSnapshot naming \
             the deleted {parent}, got {status} with {answer}"
        );
        assert_eq!(
            answer.get("Reason").and_then(serde_json::Value::as_str),
            Some("SNAPSHOT_NOT_FOUND"),
            "case {case:?}: expected the snapshot not-found reason, got {answer}"
        );
        assert_eq!(
            answer.get("Message").and_then(serde_json::Value::as_str),
            Some(format!("The snapshot '{parent}' does not exist.").as_str()),
            "case {case:?}: expected the not-found wording for {parent}, got {answer}"
        );

        // The child the parent was taken from is still the account's: deleting a parent does
        // not delete what was taken from it.
        let stored = account_snapshot_ids(&router, case).await;
        assert!(
            stored.contains(&child),
            "case {case:?}: expected the child {child} to survive the delete of {parent}, got \
             {stored:?}"
        );
    }
}
