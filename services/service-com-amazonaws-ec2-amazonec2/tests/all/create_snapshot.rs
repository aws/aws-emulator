// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `CreateSnapshot`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. ONE SNAPSHOT OF THE VOLUME THE REQUEST NAMES, with every member of it read off THAT
//!    volume (its id, its size, its encryption state) or off the request (the description,
//!    the tags) — "snapshots that are taken from encrypted volumes are automatically
//!    encrypted", so the request cannot ask for an encryption state of its own;
//! 2. THE SNAPSHOT IS HANDED BACK `pending` WITH NO PROGRESS, and the CLOCK — not a caller —
//!    moves it to `completed`;
//! 3. IT IS THE SAME SNAPSHOT A LATER READ REPORTS: `DescribeSnapshots` answers it by its own
//!    id and in the listing filtered on the source volume, which is how a backup flow finds
//!    it;
//! 4. A `VolumeId` THIS OPERATION CANNOT RESOLVE IS REFUSED WITH ONE ANSWER, whichever way it
//!    fails to name a volume of the account, and nothing is stored;
//! 5. ANY OTHER REQUEST THAT CANNOT BE SERVED IS REFUSED WITH EC2'S OWN CODE AND NOTHING IS
//!    STORED: a location or an Outpost this emulator has none of, a value outside a modelled
//!    enum's catalog, and a `DryRun` that reports whether the call would have worked.
//!
//! Each table row is a distinct input along what the rule generalises over — the volume's
//! size, its encryption state, whether the request describes or tags the snapshot, which id
//! form an unresolvable volume id takes, and which refusal — so no row is the specification.

use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use ec2::state::{
    SNAPSHOT_PROGRESS_COMPLETED, SNAPSHOT_PROGRESS_PENDING, SNAPSHOT_STATE_COMPLETED,
    SNAPSHOT_STATE_PENDING, SNAPSHOT_STORAGE_TIER_STANDARD,
};

use super::ec2_query::{element, elements, error_code, error_message};
use super::volume_lifecycle::{p, past_the_transition, zone_name, REGION};

/// A volume id of the long form this service mints that no volume of the account carries.
const UNKNOWN_VOLUME_ID: &str = "vol-0123456789abcdef0";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

/// Create one volume of `size` GiB in the Region's first zone, encrypted or not, and answer
/// its id — the volume whose snapshot each row is about.
async fn create_volume(router: &MultiServiceRouter, size: i32, encrypted: bool) -> String {
    let mut params = vec![
        p("AvailabilityZone", zone_name(0)),
        p("Size", size.to_string()),
    ];
    if encrypted {
        params.push(p("Encrypted", "true"));
    }
    let (status, body) = op(router, "CreateVolume", &params).await;
    assert_eq!(
        status, 200,
        "creating a {size} GiB volume: expected HTTP 200, got {status} with body {body}"
    );
    element(&body, "volumeId").expect("CreateVolume reports the volume it minted")
}

/// One row: the volume the snapshot is taken of, and what the request says about it.
struct SnapshotCase {
    case: &'static str,
    size: i32,
    encrypted: bool,
    description: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// The class the rule covers: any volume of the account, described and tagged however the
/// request chooses.
fn snapshot_cases() -> Vec<SnapshotCase> {
    vec![
        SnapshotCase {
            case: "a one GiB plain volume, no description and no tags",
            size: 1,
            encrypted: false,
            description: None,
            tags: &[],
        },
        SnapshotCase {
            case: "an eight GiB encrypted volume, described and tagged",
            size: 8,
            encrypted: true,
            description: Some("nightly backup"),
            tags: &[("Name", "nightly"), ("owner", "data")],
        },
    ]
}

#[tokio::test]
async fn a_snapshot_reports_the_volume_it_was_taken_of() {
    for row in snapshot_cases() {
        let case = row.case;
        let (router, _engine) = super::ec2_query::emulator();
        let volume_id = create_volume(&router, row.size, row.encrypted).await;

        let mut params = vec![p("VolumeId", &volume_id)];
        if let Some(description) = row.description {
            params.push(p("Description", description));
        }
        for (index, (key, value)) in row.tags.iter().enumerate() {
            let at = index + 1;
            params.push(p("TagSpecification.1.ResourceType", "snapshot"));
            params.push(p(&format!("TagSpecification.1.Tag.{at}.Key"), *key));
            params.push(p(&format!("TagSpecification.1.Tag.{at}.Value"), *value));
        }
        let (status, body) = op(&router, "CreateSnapshot", &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );

        // What the snapshot IS: the volume it was taken of, at that volume's own size and
        // encryption state.
        let snapshot_id = element(&body, "snapshotId")
            .unwrap_or_else(|| panic!("case {case:?}: no snapshotId in {body}"));
        assert!(
            snapshot_id.starts_with("snap-"),
            "case {case:?}: expected the id EC2 mints for a snapshot, got {snapshot_id:?}"
        );
        for (member, expected) in [
            ("volumeId", volume_id.clone()),
            ("volumeSize", row.size.to_string()),
            ("encrypted", row.encrypted.to_string()),
            ("ownerId", engine::emulator_account_id().to_string()),
            ("status", SNAPSHOT_STATE_PENDING.to_string()),
            ("progress", SNAPSHOT_PROGRESS_PENDING.to_string()),
            ("storageTier", SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
        ] {
            let actual = element(&body, member).unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {member} {expected:?}, got {actual:?} in {body}"
            );
        }
        let described = element(&body, "description").unwrap_or_default();
        assert_eq!(
            described,
            row.description.unwrap_or_default(),
            "case {case:?}: expected the description the request supplied, got {described:?}"
        );
        for (key, value) in row.tags {
            assert!(
                elements(&body, "key").contains(&(*key).to_string())
                    && elements(&body, "value").contains(&(*value).to_string()),
                "case {case:?}: expected the tag {key}={value} on the snapshot, got {body}"
            );
        }

        // The CLOCK completes it, and the completed snapshot is what a later read reports —
        // by its own id and in the listing filtered on the source volume.
        past_the_transition().await;
        let (status, listed) = op(
            &router,
            "DescribeSnapshots",
            &[p("SnapshotId.1", &snapshot_id)],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the snapshot to be addressable by its id, got {status} \
             {listed}"
        );
        for (member, expected) in [
            ("volumeId", volume_id.clone()),
            ("volumeSize", row.size.to_string()),
            ("status", SNAPSHOT_STATE_COMPLETED.to_string()),
            ("progress", SNAPSHOT_PROGRESS_COMPLETED.to_string()),
        ] {
            let actual = element(&listed, member).unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the stored snapshot's {member} to be {expected:?}, \
                 got {actual:?} in {listed}"
            );
        }
        let (status, filtered) = op(
            &router,
            "DescribeSnapshots",
            &[
                p("Owner.1", "self"),
                p("Filter.1.Name", "volume-id"),
                p("Filter.1.Value.1", &volume_id),
            ],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the filtered listing to succeed, got {status} {filtered}"
        );
        let found = elements(&filtered, "snapshotId");
        assert_eq!(
            found,
            vec![snapshot_id.clone()],
            "case {case:?}: expected exactly the snapshot of {volume_id} in the filtered \
             listing, got {found:?}"
        );
    }
}

/// RULE 4: every `VolumeId` this operation cannot resolve to a volume of the account gets the
/// SAME answer — 400 `InvalidVolumeID.Malformed` with the service's parameter-layer sentence
/// naming the id the request sent — whether the value is not a volume id at all or is well
/// formed and names no volume. The recorded unit `ec2.op-contract-create-snapshot` (corpus
/// `simparity-behavioral-aws-v1`, named in this operation's issue) carries that answer for a
/// well-formed id of the long form the account holds no volume for, which is why this
/// operation does not split the two the way `DeleteVolume` and `AttachVolume` do.
///
/// The rows span both id forms EC2 has issued (8 and 17 hexadecimal digits), the all-zero
/// placeholder, values outside that form, and the id of another kind of resource.
#[tokio::test]
async fn a_volume_id_this_operation_cannot_resolve_gets_one_answer() {
    let (router, _engine) = super::ec2_query::emulator();
    // One volume the account DOES hold, so each refusal below is about the id the request
    // named rather than about an empty store.
    let held = create_volume(&router, 1, false).await;

    let rows: [(&str, &str); 8] = [
        ("a well-formed long id naming no volume", UNKNOWN_VOLUME_ID),
        ("a well-formed short id naming no volume", "vol-1a2b3c4d"),
        (
            "the all-zero placeholder no mint can issue",
            "vol-00000000000000000",
        ),
        ("a value that is not a volume id at all", "not-a-volume"),
        ("the prefix with no digits", "vol-"),
        ("a digit count between the two id forms", "vol-0123456789"),
        ("an uppercase id", "VOL-0123456789ABCDEF0"),
        (
            "the id of another kind of resource",
            "snap-0123456789abcdef0",
        ),
    ];
    for (case, volume_id) in rows {
        let (status, body) = op(&router, "CreateSnapshot", &[p("VolumeId", volume_id)]).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidVolumeID.Malformed",
            "case {case:?}: expected InvalidVolumeID.Malformed, got {body}"
        );
        assert_eq!(
            error_message(&body),
            format!("Value ( {volume_id} ) for parameter VOLUME is invalid. "),
            "case {case:?}: expected the refusal's whole sentence, got {:?}",
            error_message(&body)
        );
    }

    // Nothing was stored for any of them, and the volume the account holds is still
    // snapshottable — so the rows above were refused for the ids they named.
    let (status, listed) = op(&router, "DescribeSnapshots", &[p("Owner.1", "self")]).await;
    assert_eq!(
        status, 200,
        "expected DescribeSnapshots to succeed, got {status} {listed}"
    );
    let stored = elements(&listed, "snapshotId");
    assert!(
        stored.is_empty(),
        "a refused request stores no snapshot, got {stored:?}"
    );
    let (status, body) = op(&router, "CreateSnapshot", &[p("VolumeId", &held)]).await;
    assert_eq!(
        status, 200,
        "the volume the account holds is still snapshottable, got {status} with body {body}"
    );
    assert_eq!(
        elements(&body, "snapshotId").len(),
        1,
        "that create mints exactly one snapshot, got {body}"
    );
}

/// One refusal row: the case, the request's parameters, and the HTTP status and EC2 error
/// code it must be answered with.
type RefusalRow = (&'static str, Vec<(String, String)>, u16, &'static str);

#[tokio::test]
async fn a_request_that_cannot_be_served_is_refused_and_stores_nothing() {
    let (router, _engine) = super::ec2_query::emulator();
    let volume_id = create_volume(&router, 1, false).await;
    let rows: Vec<RefusalRow> = vec![
        (
            "a location only a volume in a Local Zone can ask for",
            vec![p("VolumeId", &volume_id), p("Location", "local")],
            400,
            "InvalidParameterValue",
        ),
        (
            "a location outside the modelled catalog, refused by the enum itself",
            vec![p("VolumeId", &volume_id), p("Location", "orbital")],
            400,
            "ValidationError",
        ),
        (
            "an Outpost no volume of this emulator is on",
            vec![
                p("VolumeId", &volume_id),
                p(
                    "OutpostArn",
                    engine::emulator_arn("outposts", REGION, "outpost/op-0123456789abcdef0"),
                ),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a tag specification naming a resource this call does not create",
            vec![
                p("VolumeId", &volume_id),
                p("TagSpecification.1.ResourceType", "volume"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a dry run of a request that would have worked",
            vec![p("VolumeId", &volume_id), p("DryRun", "true")],
            412,
            "DryRunOperation",
        ),
    ];
    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = op(&router, "CreateSnapshot", &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
        assert!(
            !error_message(&body).is_empty(),
            "case {case:?}: a refusal says why, got {body}"
        );
        // Nothing was stored for any of them: the listing of the account's snapshots is
        // still empty.
        let (status, listed) = op(&router, "DescribeSnapshots", &[p("Owner.1", "self")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected DescribeSnapshots to succeed, got {status} {listed}"
        );
        let stored = elements(&listed, "snapshotId");
        assert!(
            stored.is_empty(),
            "case {case:?}: a refused request stores no snapshot, got {stored:?}"
        );
    }
}
