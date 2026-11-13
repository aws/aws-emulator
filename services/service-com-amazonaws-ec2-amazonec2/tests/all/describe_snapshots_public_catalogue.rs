// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the PUBLIC half of the inventory `DescribeSnapshots` reports, and
//! for the members every snapshot of that inventory projects — driven over the wire the
//! service actually speaks (`ec2Query`), through the crate's shared [`super::ec2_query`]
//! harness.
//!
//! The rules under test are:
//!
//! 1. THE ACCOUNT-VISIBLE INVENTORY INCLUDES THE PUBLIC SNAPSHOTS, not only the ones the
//!    account created: every AMI of the catalogue restores its root device from a snapshot,
//!    and the snapshots the public images are published with are reported as snapshots of
//!    that inventory — owned by the publisher, under its owner alias, complete, in the
//!    standard storage tier, and carrying the size of the device the image restores. Each
//!    narrowing a caller can reach them by selects them;
//! 2. EVERY REPORTED SNAPSHOT PROJECTS ITS FULL SIZE AND ITS TRANSFER TYPE: the full size is
//!    the source volume's size in bytes, whatever that size is, and the transfer type is the
//!    standard one for a snapshot no caller asked a completion duration for;
//! 3. A TRUNCATED PAGE CARRIES A `NextToken` AND THE WALK RESUMES AFTER IT, over an inventory
//!    the account did not create;
//! 4. A `SnapshotIds` ENTRY THAT IS NOT IN THE FORM EC2 PRINTS A SNAPSHOT ID IN IS REFUSED AT
//!    THE PARAMETER LAYER — `InvalidParameterValue`, naming the parameter and the form it
//!    expects — which is a different refusal from the id-shape one for a value INSIDE that
//!    form that no snapshot can carry, and from the not-found one for a well-formed id the
//!    inventory holds no snapshot for.
//!
//! Each table row is a distinct input along what the rule generalises over — which narrowing
//! reaches the public snapshots, which volume size the full size is measured from, which page
//! size truncates the walk, which malformed form is refused — so no row is the specification.

use smithy_server_runtime::MultiServiceRouter;

use service_com_amazonaws_ec2_amazonec2::state::{
    BYTES_PER_GIB, IMAGE_OWNER_ALIAS_AMAZON, SNAPSHOT_PROGRESS_COMPLETED,
    SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER, SNAPSHOT_STATE_COMPLETED, SNAPSHOT_STORAGE_TIER_STANDARD,
    SNAPSHOT_TRANSFER_TYPE_STANDARD,
};

use super::ec2_query::{as_timestamp, elements, error_code, error_message, filters, indexed};
use super::volume_lifecycle::{REGION, p, zone_name};

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A snapshot id of the long form this service mints that no snapshot of the inventory
/// carries — a well-formed id naming nothing is a different cause from a value that is not a
/// snapshot id at all, so the refusal table needs one of each.
const UNKNOWN_SNAPSHOT_ID: &str = "snap-0123456789abcdef0";

/// The all-zero placeholder id: seventeen hexadecimal digits, so INSIDE the form EC2 prints a
/// snapshot id in, which is why it is refused as a bad id rather than as a bad parameter.
const NULL_SNAPSHOT_ID: &str = "snap-00000000000000000";

/// One `action` call signed for the Region the suite works in.
async fn op(
    router: &MultiServiceRouter,
    action: &str,
    params: &[(String, String)],
) -> (u16, String) {
    super::ec2_query::call(router, REGION, action, params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    op(router, "DescribeSnapshots", params).await
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

/// Every snapshot a successful `DescribeSnapshots` answer reported, read the one way this
/// crate's suites read a reported snapshot.
async fn reported(
    router: &MultiServiceRouter,
    case: &str,
    params: &[(String, String)],
) -> Vec<super::describe_snapshots::ReportedSnapshot> {
    let (status, body) = describe(router, params).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body {body}"
    );
    super::describe_snapshots::described_snapshots(case, &body)
}

/// One EBS device mapping of the published AMI catalogue, as the AMI itself reports it:
/// `(snapshotId, volumeSize, encrypted)`.
///
/// Every EBS-backed image restores each of its devices "from a snapshot"
/// (`EbsBlockDevice$SnapshotId`), of a size and an encryption state the image reports
/// (`$VolumeSize`, `$Encrypted`) — so these are what the inventory has to resolve and report,
/// read off the image rather than written here by hand.
type CatalogueMapping = (String, String, String);

/// The EBS device mappings of the public images Amazon publishes, read over the wire.
///
/// `IncludeDeprecated`, because a deprecated build is left out of a describe by default
/// ("if `true`, all deprecated AMIs are included in the response",
/// `DescribeImagesRequest$IncludeDeprecated`) while the snapshot it restores from is a
/// snapshot of the inventory like any other.
async fn catalogue_mappings(router: &MultiServiceRouter) -> Vec<CatalogueMapping> {
    let mut params = indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]);
    params.push(p("IncludeDeprecated", "true"));
    let (status, body) = op(router, "DescribeImages", &params).await;
    assert_eq!(
        status, 200,
        "the published AMI catalogue: expected HTTP 200, got {status} with body {body}"
    );
    let ids = elements(&body, "snapshotId");
    let sizes = elements(&body, "volumeSize");
    let encrypted = elements(&body, "encrypted");
    assert!(
        !ids.is_empty(),
        "the published AMI catalogue restores its root devices from snapshots, got {body}"
    );
    assert_eq!(
        (ids.len(), ids.len()),
        (sizes.len(), encrypted.len()),
        "every mapping of the catalogue reports its snapshot, its size and its encryption \
         state, got {body}"
    );
    ids.into_iter()
        .zip(sizes)
        .zip(encrypted)
        .map(|((id, size), encrypted)| (id, size, encrypted))
        .collect()
}

/// RULE 1: the public snapshots the AMI catalogue is published with are snapshots of the
/// account-visible inventory, and every narrowing that names them selects them.
#[tokio::test]
async fn the_inventory_carries_the_public_snapshots_of_the_ami_catalogue() {
    let (router, _engine) = emulator();
    let account = engine::emulator_account_id();
    let published = catalogue_mappings(&router).await;
    let one_published = published[0].0.clone();
    let publisher = reported(&router, "one published snapshot", &indexed("SnapshotId", &[&one_published]))
        .await
        .first()
        .map(|snapshot| snapshot.member("ownerId").to_string())
        .unwrap_or_else(|| panic!("the inventory reports the published snapshot {one_published}"));

    // Each row reaches the public snapshots by a DIFFERENT member of the request: the owner
    // list, the owner's own account id, the two owner filters, the restorable-by list that
    // names every account, and one id by name.
    let rows: Vec<(&str, Params)> = vec![
        (
            "OwnerIds naming the publisher's alias",
            indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]),
        ),
        (
            "OwnerIds naming the publisher's account id",
            indexed("Owner", &[publisher.as_str()]),
        ),
        (
            "the owner-alias filter",
            filters(&[("owner-alias", &[IMAGE_OWNER_ALIAS_AMAZON])]),
        ),
        (
            "the owner-id filter",
            filters(&[("owner-id", &[publisher.as_str()])]),
        ),
        (
            "RestorableByUserIds all: a public snapshot is restorable by every account",
            indexed("RestorableBy", &["all"]),
        ),
        (
            "one published snapshot by id",
            indexed("SnapshotId", &[&one_published]),
        ),
    ];

    for (case, params) in rows {
        let snapshots = reported(&router, case, &params).await;
        assert!(
            !snapshots.is_empty(),
            "case {case:?}: the public snapshots of the inventory are reported, got none"
        );
        // The row that narrows by the publisher's ALIAS reports exactly the snapshots the
        // catalogue's images restore from — the set read off `DescribeImages`, so neither
        // side of the one resource is read from this suite's own words.
        if case.contains("alias") {
            let mut expected: Vec<String> = published
                .iter()
                .map(|(snapshot_id, _, _)| snapshot_id.clone())
                .collect();
            expected.sort();
            assert_eq!(
                ids_of(&snapshots),
                expected,
                "case {case:?}: expected every snapshot the catalogue's images restore from"
            );
        }
        // The row that narrows by the publisher's ACCOUNT ID can only report that publisher's
        // snapshots, which is what it is for.
        if case.contains("account id") {
            for snapshot in &snapshots {
                assert_eq!(
                    snapshot.member("ownerId"),
                    publisher,
                    "case {case:?}: expected the snapshots of {publisher}, got {:?}",
                    snapshot.member("ownerId")
                );
            }
        }
        for snapshot in &snapshots {
            for (member, expected) in [
                ("status", SNAPSHOT_STATE_COMPLETED.to_string()),
                ("progress", SNAPSHOT_PROGRESS_COMPLETED.to_string()),
                ("storageTier", SNAPSHOT_STORAGE_TIER_STANDARD.to_string()),
                ("transferType", SNAPSHOT_TRANSFER_TYPE_STANDARD.to_string()),
                (
                    "volumeId",
                    SNAPSHOT_SOURCE_VOLUME_PLACEHOLDER.to_string(),
                ),
            ] {
                assert_eq!(
                    snapshot.member(member),
                    expected,
                    "case {case:?}: expected <{member}> {expected:?}, got {:?}",
                    snapshot.member(member)
                );
            }
            // The owner and its alias are read PER ITEM rather than compared against one
            // publisher's, so a catalogue published by more than one account still reads as
            // the rule says: the snapshot belongs to the account that published the image,
            // under that account's alias, and neither is the calling account's. WHICH alias
            // is the alias-narrowed row's own subject.
            assert!(
                !snapshot.member("ownerAlias").is_empty(),
                "case {case:?}: a published snapshot carries its publisher's alias, got {:?}",
                snapshot.member("ownerAlias")
            );
            assert!(
                !snapshot.member("ownerId").is_empty(),
                "case {case:?}: a published snapshot names the account that owns it, got {:?}",
                snapshot.member("ownerId")
            );
            assert_ne!(
                snapshot.member("ownerId"),
                account,
                "case {case:?}: a published snapshot is owned by its publisher, got {:?}",
                snapshot.member("ownerId")
            );
            assert!(
                !snapshot.member("description").is_empty(),
                "case {case:?}: a published snapshot describes itself, got {:?}",
                snapshot.member("description")
            );
            for member in ["startTime", "completionTime"] {
                assert!(
                    as_timestamp(snapshot.member(member)).is_some(),
                    "case {case:?}: expected an instant in <{member}>, got {:?}",
                    snapshot.member(member)
                );
            }
            let volume_size: i64 = snapshot.member("volumeSize").parse().unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: expected a volume size in GiB, got {:?}",
                    snapshot.member("volumeSize")
                )
            });
            assert_eq!(
                snapshot.member("fullSnapshotSizeInBytes"),
                (volume_size * BYTES_PER_GIB).to_string(),
                "case {case:?}: the full size is the {volume_size} GiB source volume in bytes, \
                 got {:?}",
                snapshot.member("fullSnapshotSizeInBytes")
            );
        }
    }

    // A read changes nothing a later caller sees: the same narrowing answers the same set.
    let first = reported(
        &router,
        "the publisher's snapshots",
        &indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]),
    )
    .await;
    let again = reported(
        &router,
        "the publisher's snapshots, read twice",
        &indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]),
    )
    .await;
    assert_eq!(
        ids_of(&first),
        ids_of(&again),
        "a second read of the public inventory reports the same snapshots"
    );
}

/// The snapshot ids of a reported set, in report order.
fn ids_of(snapshots: &[super::describe_snapshots::ReportedSnapshot]) -> Vec<String> {
    snapshots
        .iter()
        .map(|snapshot| snapshot.member("snapshotId").to_string())
        .collect()
}

/// RULE 1 (continued): a published snapshot reports the device the image restores from it —
/// the same size and encryption state the AMI's own block device mapping carries, for every
/// image of the catalogue.
#[tokio::test]
async fn a_published_snapshot_reports_the_device_its_image_restores() {
    let (router, _engine) = emulator();
    let published = catalogue_mappings(&router).await;

    // Every mapping of the catalogue, not only the first: the rule holds for each image,
    // whatever root volume size and encryption state it was published with, and each row's
    // expectation is READ OFF that image rather than written here.
    for (snapshot_id, size, encrypted) in &published {
        let snapshots = reported(
            &router,
            snapshot_id,
            &indexed("SnapshotId", &[snapshot_id.as_str()]),
        )
        .await;
        assert_eq!(
            ids_of(&snapshots),
            vec![snapshot_id.clone()],
            "case {snapshot_id:?}: the inventory resolves the snapshot the AMI restores from"
        );
        let snapshot = &snapshots[0];
        for (member, expected) in [("volumeSize", size), ("encrypted", encrypted)] {
            assert_eq!(
                snapshot.member(member),
                expected,
                "case {snapshot_id:?}: expected the AMI's own <{member}> {expected:?}, got {:?}",
                snapshot.member(member)
            );
        }
        let volume_size: i64 = snapshot.member("volumeSize").parse().unwrap_or_else(|_| {
            panic!(
                "case {snapshot_id:?}: expected a volume size in GiB, got {:?}",
                snapshot.member("volumeSize")
            )
        });
        assert!(
            volume_size > 0,
            "case {snapshot_id:?}: an AMI restores a device of some size, got {volume_size}"
        );
        assert_eq!(
            snapshot.member("fullSnapshotSizeInBytes"),
            (volume_size * BYTES_PER_GIB).to_string(),
            "case {snapshot_id:?}: the full size is the {volume_size} GiB device in bytes, got \
             {:?}",
            snapshot.member("fullSnapshotSizeInBytes")
        );
    }
}

/// RULE 2: the full size a snapshot reports is the size of the volume it was taken of, in
/// bytes, for every volume size — and its transfer type is the standard one.
#[tokio::test]
async fn a_snapshot_reports_the_full_size_of_the_volume_it_was_taken_of() {
    let (router, _engine) = emulator();

    // One row per source volume size, spanning the sizes a volume can be created at: the
    // smallest, a middling one and a large one.
    for size in [1_i64, 8, 125, 1_024] {
        let case = format!("a {size} GiB source volume");
        let (status, body) = op(
            &router,
            "CreateVolume",
            &[p("AvailabilityZone", zone_name(0)), p("Size", size.to_string())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the source volume: expected HTTP 200, got {status} with \
             body {body}"
        );
        let volume_id = super::ec2_query::element(&body, "volumeId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateVolume reports the volume it minted"));

        let (status, body) = op(
            &router,
            "CreateSnapshot",
            &[p("VolumeId", volume_id.clone())],
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case:?}: creating the snapshot: expected HTTP 200, got {status} with body \
             {body}"
        );
        let snapshot_id = super::ec2_query::element(&body, "snapshotId")
            .unwrap_or_else(|| panic!("case {case:?}: CreateSnapshot reports the snapshot"));

        let snapshots = reported(
            &router,
            &case,
            &indexed("SnapshotId", &[snapshot_id.as_str()]),
        )
        .await;
        let snapshot = &snapshots[0];
        assert_eq!(
            snapshot.member("fullSnapshotSizeInBytes"),
            (size * BYTES_PER_GIB).to_string(),
            "case {case:?}: the full size is the source volume's size in bytes, got {:?}",
            snapshot.member("fullSnapshotSizeInBytes")
        );
        assert_eq!(
            snapshot.member("transferType"),
            SNAPSHOT_TRANSFER_TYPE_STANDARD,
            "case {case:?}: a snapshot no completion duration was asked for is the standard \
             transfer, got {:?}",
            snapshot.member("transferType")
        );
        assert_eq!(
            snapshot.member("volumeId"),
            volume_id,
            "case {case:?}: the snapshot names the volume it was taken of, got {:?}",
            snapshot.member("volumeId")
        );
    }
}

/// RULE 3: a page `MaxResults` truncated carries a `NextToken`, the walk resumes after it,
/// and the last page carries none.
#[tokio::test]
async fn a_truncated_page_carries_a_token_the_walk_resumes_from() {
    let (router, _engine) = emulator();
    let publisher_narrowing = indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON]);
    let whole = ids_of(&reported(&router, "the whole public inventory", &publisher_narrowing).await);
    assert!(
        whole.len() > 2,
        "the public inventory holds more than two snapshots to page over, got {whole:?}"
    );

    // One row per page size, spanning a page of one, a page of several and a page that holds
    // the whole set (which is not truncated, so it carries no token).
    for page_size in [1_usize, 2, whole.len()] {
        let case = format!("a page of {page_size}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        // One more pass than the set could need, so a token that never clears fails here
        // rather than spinning.
        for _ in 0..whole.len() + 2 {
            let mut params = publisher_narrowing.clone();
            params.push(p("MaxResults", page_size.to_string()));
            if let Some(token) = token.as_deref() {
                params.push(p("NextToken", token));
            }
            let (status, body) = describe(&router, &params).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected HTTP 200, got {status} with body {body}"
            );
            let page = ids_of(&super::describe_snapshots::described_snapshots(&case, &body));
            assert!(
                page.len() <= page_size,
                "case {case:?}: a page of at most {page_size}, got {page:?}"
            );
            token = super::ec2_query::element(&body, "nextToken");
            assert_eq!(
                token.is_some(),
                walked.len() + page.len() < whole.len(),
                "case {case:?}: a token follows exactly a truncated page, got {token:?} after \
                 {} of {} snapshots",
                walked.len() + page.len(),
                whole.len()
            );
            walked.extend(page);
            if token.is_none() {
                break;
            }
        }
        assert_eq!(
            walked, whole,
            "case {case:?}: the walk reports the whole public inventory once each, in \
             SnapshotId order"
        );
    }
}

/// RULE 4: a `SnapshotIds` entry outside the form EC2 prints a snapshot id in is refused as a
/// bad PARAMETER, and the two refusals for a value inside that form keep their own codes.
#[tokio::test]
async fn a_snapshot_id_outside_the_printed_form_is_refused_as_a_bad_parameter() {
    let (router, _engine) = emulator();

    /// One row: `(the value, the expected code, the expected message)`.
    type RefusalRow = (&'static str, &'static str, String);

    // The malformed rows differ in HOW the value leaves the form: no prefix at all, the
    // prefix with non-hexadecimal text, the prefix with a width between the two EC2 prints,
    // the prefix alone, and an id of another kind of resource.
    let malformed = |value: &str| -> String {
        format!("Value ({value}) for parameter snapshotId is invalid. Expected: 'snap-...'.")
    };
    let rows: Vec<RefusalRow> = vec![
        ("not-a-snapshot", "InvalidParameterValue", malformed("not-a-snapshot")),
        ("snap-bad-id", "InvalidParameterValue", malformed("snap-bad-id")),
        (
            "snap-0123456789abcdef",
            "InvalidParameterValue",
            malformed("snap-0123456789abcdef"),
        ),
        ("snap-", "InvalidParameterValue", malformed("snap-")),
        (
            "vol-0123456789abcdef0",
            "InvalidParameterValue",
            malformed("vol-0123456789abcdef0"),
        ),
        (
            NULL_SNAPSHOT_ID,
            "InvalidSnapshotID.Malformed",
            format!("Value ({NULL_SNAPSHOT_ID}) for snapshot ID is invalid."),
        ),
        (
            UNKNOWN_SNAPSHOT_ID,
            "InvalidSnapshot.NotFound",
            format!("The snapshot '{UNKNOWN_SNAPSHOT_ID}' does not exist."),
        ),
    ];

    for (value, code, message) in rows {
        let (status, body) = describe(&router, &indexed("SnapshotId", &[value])).await;
        assert_eq!(
            status, 400,
            "case {value:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            code,
            "case {value:?}: expected the code {code:?}, got {:?}",
            error_code(&body)
        );
        assert_eq!(
            error_message(&body),
            message,
            "case {value:?}: expected the message {message:?}, got {:?}",
            error_message(&body)
        );
    }

    // Nothing a refusal touched: the public inventory is still there to read.
    assert!(
        !reported(
            &router,
            "after every refusal",
            &indexed("Owner", &[IMAGE_OWNER_ALIAS_AMAZON])
        )
        .await
        .is_empty(),
        "a refused read leaves the inventory as it was"
    );
}
