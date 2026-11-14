// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `RestoreImageFromRecycleBin`, driven over the wire the service
//! actually speaks — `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. RESTORING A RECYCLED AMI PUTS THE STORED IMAGE BACK: whatever the image is and
//!    whatever retention period it entered the Recycle Bin for, the call answers `true`,
//!    the AMI is one `DescribeImages` reports again — under the same id and with the
//!    members it had — and the bin holds it no more, so a second restore of the same id
//!    has nothing to restore;
//! 2. the id is resolved IN THE BIN, and every id that does not resolve there is refused
//!    with the one code `InvalidAMIID.Malformed` — something that is not an AMI id at all,
//!    a well-formed id nothing was ever stored under, a live AMI nobody deregistered,
//!    another account's image — with nothing moved either way;
//! 3. `DryRun` answers what the request WOULD have done and writes nothing: the AMI stays
//!    in the bin, and a request that would have been refused is refused with that reason
//!    instead;
//! 4. the CLOCK, not a caller, permanently deletes an entry whose retention period has run
//!    out — after which the id resolves to no entry at all and the AMI is gone for good.
//!
//! Each table row is a distinct input along what the rule generalises over — which image
//! (platform, architecture, tags), which retention period at each published bound of a
//! Recycle Bin rule, which shape of unresolvable id, which way a dry run would have gone,
//! and which side of the exit instant the entry sits on — so no row is the specification.
//!
//! Nothing in the EC2 API puts an AMI in the Recycle Bin yet: `DeregisterImage` — "if a
//! deregistered EBS-backed AMI matches a Recycle Bin retention rule, it moves to the
//! Recycle Bin for the specified retention period" — is still a scaffold with its own stub
//! issue, so these tests seed the bin through the state primitive that deregistration will
//! use ([`Ec2State::recycle_image`]), the same way the fast launch suite seeds images the
//! way a register operation would.

use std::time::{Duration, SystemTime};

use ec2::ops::types::Tag;
use ec2::state::{
    ComAmazonawsEc2Amazonec2State as Ec2State, IMAGE_PLATFORM_WINDOWS, ImageSpec,
    PLATFORM_DETAILS_WINDOWS, USAGE_OPERATION_WINDOWS, WINDOWS_ROOT_DEVICE_NAME,
    WINDOWS_ROOT_VOLUME_SIZE, system_time_to_date_time,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, element, elements, error_code, unescaped};

/// The Region every call here is signed for. An AMI carries no Region member, so the Region
/// only selects the ec2Query envelope.
const REGION: &str = "us-west-2";

/// One day, the unit a Recycle Bin retention period is measured in: "the period value for
/// which the retention rule is to retain resources, measured in days" (rbin
/// `RetentionPeriod$RetentionPeriodValue`).
const DAY: Duration = Duration::from_secs(24 * 60 * 60);

/// The shortest retention period a rule may keep an EBS-backed AMI for: "EBS snapshots and
/// EBS-backed AMIs: 1 - 365 days" (rbin `RetentionPeriod$RetentionPeriodValue`).
const MIN_AMI_RETENTION_DAYS: u32 = 1;

/// The longest one, from the same sentence.
const MAX_AMI_RETENTION_DAYS: u32 = 365;

/// Another account, which keeps one private AMI and publishes one public AMI.
const PARTNER_ACCOUNT: &str = "111122223333";

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

/// One `RestoreImageFromRecycleBin` call for `image_id`, with `DryRun` set only when asked
/// for.
async fn restore(
    router: &MultiServiceRouter,
    image_id: &str,
    dry_run: Option<bool>,
) -> (u16, String) {
    let mut params = vec![p("ImageId", image_id)];
    if let Some(flag) = dry_run {
        params.push(p("DryRun", flag.to_string()));
    }
    call(router, REGION, "RestoreImageFromRecycleBin", &params).await
}

/// One `DescribeImages` call narrowed to `image_id` — the implemented reader this suite
/// reads a restored AMI back through.
async fn describe_one(router: &MultiServiceRouter, image_id: &str) -> (u16, String) {
    call(
        router,
        REGION,
        "DescribeImages",
        &[p("ImageId.1", image_id)],
    )
    .await
}

/// Every image id the account's own unfiltered `DescribeImages` reports.
async fn described_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = call(router, REGION, "DescribeImages", &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImages, got {status} with body {body}"
    );
    elements(&body, "imageId")
}

/// A Linux AMI of the calling account, as a register operation would store one, carrying
/// one tag so a restore can be shown to put the whole image back.
fn linux_spec(name: &str, architecture: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        description: format!("{name} description"),
        architecture: architecture.to_string(),
        tags: vec![Tag {
            key: Some("Name".to_string()),
            value: Some(name.to_string()),
        }],
        ..ImageSpec::default()
    }
}

/// A Windows AMI of the calling account: "this value is set to `windows` for Windows AMIs"
/// (`Image$Platform`).
fn windows_spec(name: &str) -> ImageSpec {
    ImageSpec {
        name: name.to_string(),
        description: format!("{name} description"),
        platform: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
        platform_details: PLATFORM_DETAILS_WINDOWS.to_string(),
        usage_operation: USAGE_OPERATION_WINDOWS.to_string(),
        root_device_name: WINDOWS_ROOT_DEVICE_NAME.to_string(),
        root_volume_size: WINDOWS_ROOT_VOLUME_SIZE,
        tags: vec![Tag {
            key: Some("Platform".to_string()),
            value: Some(IMAGE_PLATFORM_WINDOWS.to_string()),
        }],
        ..ImageSpec::default()
    }
}

/// Store `spec` and answer the `ImageId` it was given.
fn seed_image(engine: &engine::InMemoryEngine, spec: ImageSpec) -> String {
    engine.state::<Ec2State>().lock().insert_image(spec)
}

/// Move `image_id` into the Recycle Bin for `retention`, the way `DeregisterImage` will.
fn recycle(engine: &engine::InMemoryEngine, image_id: &str, retention: Duration) {
    let recycled = engine
        .state::<Ec2State>()
        .lock()
        .recycle_image(image_id, retention);
    assert!(
        recycled,
        "the image {image_id:?} must be in the store to be recycled"
    );
}

/// Does the Recycle Bin hold an entry for `image_id`?
fn bin_holds(engine: &engine::InMemoryEngine, image_id: &str) -> bool {
    engine
        .state::<Ec2State>()
        .lock()
        .images_in_recycle_bin
        .contains_key(image_id)
}

/// Does the image store hold `image_id` — is it an AMI the account can see again?
fn store_holds(engine: &engine::InMemoryEngine, image_id: &str) -> bool {
    engine
        .state::<Ec2State>()
        .lock()
        .images
        .contains_key(image_id)
}

/// Rule 1: whatever the image and whatever its retention period, restoring it answers
/// `true`, `DescribeImages` reports the same AMI again, and the bin is empty of it — so a
/// second restore of that id has nothing left to restore.
#[tokio::test]
async fn a_recycled_image_is_restored_and_reported_again() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the image (Linux x86_64 with a tag, a Windows AMI, an arm64 Linux AMI) paired
    // with the retention period it entered the bin for — the published floor, a period
    // between the bounds, and the published ceiling.
    let cases: Vec<(&str, ImageSpec, u32)> = vec![
        (
            "a tagged x86_64 Linux AMI at the 1-day floor",
            linux_spec("app-linux-x86_64", "x86_64"),
            MIN_AMI_RETENTION_DAYS,
        ),
        (
            "a Windows AMI retained for 30 days",
            windows_spec("app-windows-base"),
            30,
        ),
        (
            "an arm64 Linux AMI at the 365-day ceiling",
            linux_spec("app-linux-arm64", "arm64"),
            MAX_AMI_RETENTION_DAYS,
        ),
    ];

    for (case, spec, retention_days) in cases {
        let expected_name = spec.name.clone();
        let expected_architecture = spec.architecture.clone();
        let expected_tag_keys: Vec<String> = spec
            .tags
            .iter()
            .filter_map(|tag| tag.key.clone())
            .collect();
        let image_id = seed_image(&engine, spec);

        // Before the deregistration the AMI is one the account can see.
        let (status, body) = describe_one(&router, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: the seeded image must be describable, got {status} with body {body}"
        );

        recycle(&engine, &image_id, DAY * retention_days);

        // A recycled AMI is NOT one of the images available to the account.
        let reported = described_ids(&router, case).await;
        assert!(
            !reported.contains(&image_id),
            "case {case:?}: a recycled AMI must not be reported, got {reported:?}"
        );
        let (status, body) = describe_one(&router, &image_id).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a recycled AMI must not resolve as an image, got {status} with \
             body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.NotFound",
            "case {case:?}: expected InvalidAMIID.NotFound for a recycled AMI, got {body}"
        );

        let (status, body) = restore(&router, &image_id, None).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from the restore, got {status} with body {body}"
        );
        let returned = element(&body, "return").unwrap_or_default();
        assert_eq!(
            returned, "true",
            "case {case:?}: expected the restore to report true, got {returned:?} in {body}"
        );

        // Read the AMI back through the implemented describe: the same image, not a
        // rebuilt copy.
        let (status, body) = describe_one(&router, &image_id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a restored AMI must be describable, got {status} with body {body}"
        );
        let reported_id = element(&body, "imageId").unwrap_or_default();
        assert_eq!(
            reported_id, image_id,
            "case {case:?}: expected the restored AMI {image_id:?}, got {reported_id:?}"
        );
        let reported_name = element(&body, "name").unwrap_or_default();
        assert_eq!(
            reported_name, expected_name,
            "case {case:?}: expected the stored name {expected_name:?}, got {reported_name:?}"
        );
        let reported_architecture = element(&body, "architecture").unwrap_or_default();
        assert_eq!(
            reported_architecture, expected_architecture,
            "case {case:?}: expected the stored architecture {expected_architecture:?}, got \
             {reported_architecture:?}"
        );
        let reported_tag_keys = elements(&body, "key");
        assert_eq!(
            reported_tag_keys, expected_tag_keys,
            "case {case:?}: expected the stored tag keys {expected_tag_keys:?}, got \
             {reported_tag_keys:?}"
        );
        assert!(
            !bin_holds(&engine, &image_id),
            "case {case:?}: the Recycle Bin must not hold {image_id:?} after the restore"
        );

        // Nothing is left to restore, so the id resolves to no recycled image at all.
        let (status, body) = restore(&router, &image_id, None).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a second restore must be refused, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.Malformed",
            "case {case:?}: expected InvalidAMIID.Malformed from the second restore, got {body}"
        );
    }
}

/// Rule 2: an `ImageId` the Recycle Bin holds no entry of this account's for is refused
/// `InvalidAMIID.Malformed`, whatever its shape, and nothing is restored.
#[tokio::test]
async fn an_image_id_the_recycle_bin_holds_no_entry_for_is_refused() {
    let (router, engine) = super::ec2_query::emulator();

    // A live AMI of the account, which nobody deregistered.
    let live = seed_image(&engine, linux_spec("app-live-image", "x86_64"));
    // Another account's images: one private, one public. The bin is per-ACCOUNT, so NEITHER
    // is an entry of this account's even while both sit in the bin — not even the public one,
    // which this account can otherwise see through `DescribeImages`.
    let partner_private = seed_image(
        &engine,
        ImageSpec {
            owner_id: PARTNER_ACCOUNT.to_string(),
            ..linux_spec("partner-private-image", "x86_64")
        },
    );
    let partner_public = seed_image(
        &engine,
        ImageSpec {
            owner_id: PARTNER_ACCOUNT.to_string(),
            public: true,
            ..linux_spec("partner-public-image", "x86_64")
        },
    );
    recycle(&engine, &partner_private, DAY);
    recycle(&engine, &partner_public, DAY);
    // One restorable entry of the account's own, so a refusal cannot be "the bin is empty".
    let recycled = seed_image(&engine, linux_spec("app-recycled-image", "x86_64"));
    recycle(&engine, &recycled, DAY);

    // Rows, over every shape an unresolvable id comes in: another resource's id, a non-hex
    // digit, too few digits, too many, an uppercase suffix, free text, the empty value, a
    // well-formed long id nothing is stored under, a well-formed short id, a live AMI
    // nobody deregistered, and the two recycled images another account owns — the private
    // one and the PUBLIC one. Every one is the same refusal.
    let unresolvable = [
        "i-0123456789abcdef0".to_string(),
        "snap-0123456789abcdef0".to_string(),
        "ami-0123456789abcdefg".to_string(),
        "ami-0123456".to_string(),
        "ami-0123456789abcdef01".to_string(),
        "ami-0123456789ABCDEF0".to_string(),
        "not-an-ami".to_string(),
        String::new(),
        "ami-0ffffffffffffffff".to_string(),
        "ami-0abc1234".to_string(),
        live.clone(),
        partner_public.clone(),
        partner_private.clone(),
    ];
    for image_id in &unresolvable {
        let (status, body) = restore(&router, image_id, None).await;
        assert_eq!(
            status, 400,
            "case {image_id:?}: expected HTTP 400, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidAMIID.Malformed",
            "case {image_id:?}: expected InvalidAMIID.Malformed, got {body}"
        );
        // The refusal is built from the request: it names the id the caller gave, in the
        // wording EC2's strict image-id validators answer an id they cannot resolve with.
        let message = unescaped(&element(&body, "Message").unwrap_or_default());
        assert_eq!(
            message,
            format!("The image ID '{image_id}' is malformed"),
            "case {image_id:?}: the refusal must name the id the request gave, got {message:?}"
        );
        for untouched in [&partner_private, &partner_public] {
            assert!(
                !store_holds(&engine, untouched) && bin_holds(&engine, untouched),
                "case {image_id:?}: a refused restore must restore nothing, yet \
                 {untouched:?} left the bin"
            );
        }
    }

    // The account's own entry is untouched by every refusal above, and still restorable.
    assert!(
        bin_holds(&engine, &recycled),
        "a refused restore must leave the bin as it was, yet {recycled:?} is gone from it"
    );
    let (status, body) = restore(&router, &recycled, None).await;
    assert_eq!(
        status, 200,
        "the account's own recycled AMI must still be restorable, got {status} with body {body}"
    );
}

/// Rule 3: `DryRun` says what the request would have done and writes nothing.
#[tokio::test]
async fn a_dry_run_leaves_the_image_in_the_recycle_bin() {
    let (router, engine) = super::ec2_query::emulator();

    let recycled = seed_image(&engine, linux_spec("app-dry-run-image", "x86_64"));
    recycle(&engine, &recycled, DAY * MAX_AMI_RETENTION_DAYS);
    let live = seed_image(&engine, linux_spec("app-dry-run-live", "x86_64"));

    // Rows: the id the request would have restored, one that is not an AMI id, and a
    // well-formed one with no entry behind it — a dry run answers the reason the request
    // would have ended with, not a blanket `DryRunOperation`.
    let cases: [(&str, String, u16, &str); 3] = [
        (
            "an entry inside its retention period",
            recycled.clone(),
            412,
            "DryRunOperation",
        ),
        (
            "an id that is not an AMI id",
            "not-an-ami".to_string(),
            400,
            "InvalidAMIID.Malformed",
        ),
        (
            "a live AMI nobody deregistered",
            live.clone(),
            400,
            "InvalidAMIID.Malformed",
        ),
    ];

    for (case, image_id, expected_status, expected_code) in cases {
        let (status, body) = restore(&router, &image_id, Some(true)).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        assert_eq!(
            error_code(&body),
            expected_code,
            "case {case:?}: expected {expected_code}, got {body}"
        );
    }

    // Nothing moved: the entry is still in the bin and still not an image the account can
    // see, and `DryRun=false` restores it.
    assert!(
        bin_holds(&engine, &recycled),
        "a dry run must leave {recycled:?} in the Recycle Bin"
    );
    assert!(
        !store_holds(&engine, &recycled),
        "a dry run must not put {recycled:?} back among the account's images"
    );
    let (status, body) = restore(&router, &recycled, Some(false)).await;
    assert_eq!(
        status, 200,
        "DryRun=false must restore the AMI, got {status} with body {body}"
    );
    assert!(
        store_holds(&engine, &recycled),
        "the restored AMI {recycled:?} must be in the image store"
    );
}

/// No single request on this path can unwind the serving process, and an arithmetic failure
/// loses no AMI: a retention period with no representable exit instant moves nothing, and
/// every representable extreme still round-trips.
///
/// The arithmetic the path performs is the enter instant plus the retention period
/// ([`Ec2State::recycle_image`]) and the comparison of that instant against the clock
/// ([`Ec2State::purge_expired_recycled_images`]); neither panics — an unrepresentable sum
/// is refused before the image leaves the image store, so the AMI stays visible.
#[tokio::test]
async fn an_unrepresentable_retention_period_moves_nothing_rather_than_crashing() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: the retention period the entry is given — the largest `Duration` there is (no
    // representable exit instant), a century, the published 365-day ceiling and the 1-day
    // floor — paired with whether the move into the bin happens at all.
    let cases: [(&str, Duration, bool); 4] = [
        ("the largest Duration there is", Duration::MAX, false),
        (
            "a century, far past the published ceiling",
            DAY * 365 * 100,
            true,
        ),
        (
            "the published 365-day ceiling",
            DAY * MAX_AMI_RETENTION_DAYS,
            true,
        ),
        ("the published 1-day floor", DAY * MIN_AMI_RETENTION_DAYS, true),
    ];

    for (case, retention, expect_recycled) in cases {
        let image_id = seed_image(
            &engine,
            linux_spec(&format!("app-extreme-{}", retention.as_secs()), "x86_64"),
        );
        let moved = engine
            .state::<Ec2State>()
            .lock()
            .recycle_image(&image_id, retention);
        assert_eq!(
            moved, expect_recycled,
            "case {case:?}: expected the move into the bin to be {expect_recycled}, got {moved}"
        );

        let (status, body) = restore(&router, &image_id, None).await;
        if expect_recycled {
            assert_eq!(
                status, 200,
                "case {case:?}: a representable retention period must leave the AMI \
                 restorable, got {status} with body {body}"
            );
            assert!(
                store_holds(&engine, &image_id),
                "case {case:?}: the restored AMI {image_id:?} must be in the image store"
            );
            assert!(
                !bin_holds(&engine, &image_id),
                "case {case:?}: the Recycle Bin must not hold {image_id:?} after the restore"
            );
        } else {
            // Nothing moved, so the AMI is still a live image of the account — and a live
            // AMI is no entry the bin can restore.
            assert!(
                store_holds(&engine, &image_id) && !bin_holds(&engine, &image_id),
                "case {case:?}: an unrepresentable retention period must leave {image_id:?} \
                 among the account's images"
            );
            assert_eq!(
                status, 400,
                "case {case:?}: a live AMI is no entry to restore, got {status} with body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidAMIID.Malformed",
                "case {case:?}: expected InvalidAMIID.Malformed, got {body}"
            );
        }
    }

    // The process is still serving: an ordinary restore after every extreme above still
    // answers from the store.
    let after = seed_image(&engine, linux_spec("app-extreme-after", "x86_64"));
    recycle(&engine, &after, DAY);
    let (status, body) = restore(&router, &after, None).await;
    assert_eq!(
        status, 200,
        "the service must keep serving after every extreme above, got {status} with body {body}"
    );
}

/// Rule 4: the clock permanently deletes an entry once its retention period has run out, so
/// only an entry still inside its period can be restored.
#[tokio::test]
async fn an_entry_whose_retention_period_has_run_out_is_permanently_deleted() {
    let (router, engine) = super::ec2_query::emulator();

    // Rows: how far the entry's exit instant sits from now — a second and a whole day past
    // it (the retention period has run out) against a second and the published ceiling
    // ahead of it (still inside the period).
    let cases: [(&str, i64, bool); 4] = [
        ("one second past the exit instant", -1, false),
        ("a day past the exit instant", -(DAY.as_secs() as i64), false),
        ("one second before the exit instant", 1, true),
        (
            "the 365-day ceiling ahead of it",
            (DAY.as_secs() as i64) * i64::from(MAX_AMI_RETENTION_DAYS),
            true,
        ),
    ];

    for (case, offset_seconds, expect_restorable) in cases {
        let image_id = seed_image(
            &engine,
            linux_spec(&format!("app-retention-{offset_seconds}"), "x86_64"),
        );
        recycle(&engine, &image_id, DAY);
        // Move the entry's exit instant to `offset_seconds` from now: the retention period
        // is the rule's, and what this row varies is which side of it the clock is on.
        {
            let store = engine.state::<Ec2State>();
            let mut guard = store.lock();
            let record = guard
                .images_in_recycle_bin
                .get_mut(&image_id)
                .unwrap_or_else(|| panic!("case {case:?}: the entry must be in the bin"));
            let offset = Duration::from_secs(offset_seconds.unsigned_abs());
            record.recycle_bin_exit_time = if offset_seconds < 0 {
                SystemTime::now() - offset
            } else {
                SystemTime::now() + offset
            };
        }

        let (status, body) = restore(&router, &image_id, None).await;
        if expect_restorable {
            assert_eq!(
                status, 200,
                "case {case:?}: an entry inside its retention period must be restorable, got \
                 {status} with body {body}"
            );
            assert!(
                store_holds(&engine, &image_id),
                "case {case:?}: the restored AMI {image_id:?} must be in the image store"
            );
        } else {
            assert_eq!(
                status, 400,
                "case {case:?}: an expired entry must not be restorable, got {status} with \
                 body {body}"
            );
            assert_eq!(
                error_code(&body),
                "InvalidAMIID.Malformed",
                "case {case:?}: expected InvalidAMIID.Malformed for an expired entry, got {body}"
            );
            assert!(
                !store_holds(&engine, &image_id),
                "case {case:?}: an expired entry is permanently deleted, yet {image_id:?} is \
                 in the image store"
            );
        }
        assert!(
            !bin_holds(&engine, &image_id),
            "case {case:?}: the Recycle Bin must not hold {image_id:?} any more"
        );
    }
}

/// The projection the bin's reader answers with is read off the STORED image — its id, name
/// and description — beside the two instants the entry itself carries: the enter instant
/// lies inside the window the deregistration happened in, and the exit instant is that plus
/// the retention period the entry was given.
#[test]
fn a_recycled_entry_projects_the_stored_image() {
    let (_router, engine) = super::ec2_query::emulator();
    let image_id = seed_image(&engine, linux_spec("app-projected-image", "x86_64"));
    let retention = DAY * MIN_AMI_RETENTION_DAYS;
    // The window the entry's enter instant has to fall inside, measured independently of
    // the record so a projection reading the wrong member cannot satisfy it.
    let before = SystemTime::now();
    recycle(&engine, &image_id, retention);
    let after = SystemTime::now();

    let store = engine.state::<Ec2State>();
    let guard = store.lock();
    let record = guard
        .images_in_recycle_bin
        .get(&image_id)
        .unwrap_or_else(|| panic!("the entry for {image_id:?} must be in the Recycle Bin"));
    let view = record.view();
    assert_eq!(
        view.image_id.as_deref(),
        Some(image_id.as_str()),
        "expected the stored id {image_id:?}, got {:?}",
        view.image_id
    );
    assert_eq!(
        view.name.as_deref(),
        Some("app-projected-image"),
        "expected the stored name, got {:?}",
        view.name
    );
    assert_eq!(
        view.description.as_deref(),
        Some("app-projected-image description"),
        "expected the stored description, got {:?}",
        view.description
    );
    let entered = view
        .recycle_bin_enter_time
        .expect("the entry reports when it entered the bin");
    let exited = view
        .recycle_bin_exit_time
        .expect("the entry reports when it is to be permanently deleted");
    assert!(
        entered >= system_time_to_date_time(before) && entered <= system_time_to_date_time(after),
        "expected the enter instant inside the window the deregistration fell in \
         ({:?}..={:?}), got {entered:?}",
        system_time_to_date_time(before),
        system_time_to_date_time(after)
    );
    assert!(
        exited >= system_time_to_date_time(before + retention)
            && exited <= system_time_to_date_time(after + retention),
        "expected the exit instant to be the enter instant plus {retention:?} \
         ({:?}..={:?}), got {exited:?}",
        system_time_to_date_time(before + retention),
        system_time_to_date_time(after + retention)
    );
    assert_eq!(
        record.recycle_bin_exit_time,
        record.recycle_bin_enter_time + retention,
        "expected the stored exit instant to be the enter instant plus {retention:?}, got {:?}",
        record.recycle_bin_exit_time
    );
}
