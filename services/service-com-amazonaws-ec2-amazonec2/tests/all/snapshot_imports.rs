// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for `ImportSnapshot`, driven over the wire the service actually speaks —
//! `ec2Query`, through the crate's shared [`super::ec2_query`] harness.
//!
//! The rules under test are:
//!
//! 1. `ImportSnapshot` STARTS A TASK FROM THE REQUEST'S DISK CONTAINER and answers it out of
//!    the store: every member the request sets round-trips through
//!    `DescribeImportSnapshotTasks`, and the members the service owns (the task id, the
//!    state, the progress and the snapshot) are derived rather than taken;
//! 2. the task is `active` with no snapshot yet, and the CLOCK — not a caller — turns the
//!    disk into an EBS snapshot OF THIS ACCOUNT, which `DescribeSnapshots` then reports like
//!    any other, and only once that snapshot is itself `completed` is the task;
//! 3. a disk container outside what the model documents — no container at all, two sources
//!    for one disk, none, a missing format, a format outside the published catalog — is
//!    refused with EC2's own error code and nothing is stored;
//! 4. one `ClientToken` names one import task;
//! 5. a `DryRun` reports whether the call would have worked and writes nothing.
//!
//! Each table row is a distinct input along what those rules generalise over — which disk
//! image format and how it is spelled, which source (an S3 URL or a bucket and key), whether
//! the import is encrypted and under which KMS key, whether a description, an upload size or
//! tags are supplied, and which way a request fails to name one disk — so no row is the
//! specification.

use std::time::Duration;

use ec2::state::{
    IMPORT_SNAPSHOT_DISK_IMAGE_FORMATS, IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN,
    IMPORT_SNAPSHOT_TASK_STATUS_MESSAGE_PENDING, IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE,
    IMPORT_TASK_STATE_ACTIVE, IMPORT_TASK_STATE_COMPLETED, MAX_ACTIVE_IMPORT_PROGRESS,
    MAX_CLIENT_TOKEN_CHARS, MAX_KMS_KEY_ID_CHARS, SNAPSHOT_PROGRESS_COMPLETED,
    SNAPSHOT_STATE_COMPLETED, TRANSITION_DELAY,
};
use service_com_amazonaws_ec2_amazonec2 as ec2;
use smithy_server_runtime::MultiServiceRouter;

use super::ec2_query::{call, elements, error_code};
use super::wire::{child, child_text, document};

/// The Region every call here is signed for. An import snapshot task carries no Region
/// member, so the Region only selects the ec2Query envelope.
const REGION: &str = "eu-west-1";

/// One request's form parameters, as the ec2Query harness takes them.
type Params = Vec<(String, String)>;

/// A `(key, value)` request parameter.
fn p(key: &str, value: impl Into<String>) -> (String, String) {
    (key.to_string(), value.into())
}

fn emulator() -> (MultiServiceRouter, std::sync::Arc<engine::InMemoryEngine>) {
    super::ec2_query::emulator()
}

async fn import(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "ImportSnapshot", params).await
}

async fn describe(router: &MultiServiceRouter, params: &[(String, String)]) -> (u16, String) {
    call(router, REGION, "DescribeImportSnapshotTasks", params).await
}

async fn describe_snapshots(
    router: &MultiServiceRouter,
    params: &[(String, String)],
) -> (u16, String) {
    call(router, REGION, "DescribeSnapshots", params).await
}

/// Wait until the whole window an import takes has certainly passed: the conversion that
/// mints the snapshot, and the snapshot's own move to `completed` that the task waits for.
/// Derived from the service's own span, not a literal, so the two cannot drift.
async fn past_the_whole_import() {
    tokio::time::sleep(TRANSITION_DELAY * 2 + Duration::from_millis(300)).await;
}

/// Where the disk image one row imports comes from: the two sources
/// `SnapshotDiskContainer` names — "the URL to the Amazon S3-based disk image being
/// imported" and "the Amazon S3 bucket for the disk image".
#[derive(Debug, Clone, Copy)]
enum Source {
    Url(&'static str),
    Bucket(&'static str, &'static str),
}

/// One row of the success table: a request, and what the task it starts must report.
struct ImportCase {
    case: &'static str,
    /// The format as the REQUEST spells it.
    format: &'static str,
    /// The format as the task must report it: the catalog's own spelling.
    reported_format: &'static str,
    source: Source,
    description: Option<&'static str>,
    upload_size: Option<f64>,
    encrypted: bool,
    kms_key_id: Option<&'static str>,
    tags: &'static [(&'static str, &'static str)],
}

/// A KMS key ARN of the form `ImportSnapshotRequest$KmsKeyId` documents ("ARN using key
/// ID"), used by the rows that import into an encrypted snapshot.
const KMS_KEY_ARN: &str =
    "arn:aws:kms:eu-west-1:012345678910:key/abcd1234-a123-456a-a12b-a123b4cd56ef";

/// The class rule 1 covers: one bare disk image, in any format the model publishes for a
/// snapshot disk container, named by either source, described, sized and tagged however the
/// request chooses, plain or encrypted.
///
/// The three formats are exactly the published catalog
/// (`SnapshotDiskContainer$Format`: "Valid values: VHD | VMDK | RAW"), one row each, and one
/// of them is spelled in a case the catalog does not use so the reported spelling is asserted
/// to be the service's rather than the request's.
fn import_cases() -> Vec<ImportCase> {
    vec![
        ImportCase {
            case: "a VMDK at an S3 URL, described, sized and tagged",
            format: "VMDK",
            reported_format: "VMDK",
            source: Source::Url("s3://disk-images/appliance.vmdk"),
            description: Some("nightly appliance disk"),
            upload_size: Some(8.0),
            encrypted: false,
            kms_key_id: None,
            tags: &[("Project", "vmimport"), ("Owner", "TeamA")],
        },
        ImportCase {
            case: "a RAW image in an S3 bucket, encrypted under a named KMS key",
            format: "RAW",
            reported_format: "RAW",
            source: Source::Bucket("disk-images", "golden/volume.raw"),
            description: Some("golden volume"),
            upload_size: Some(512.5),
            encrypted: true,
            kms_key_id: Some(KMS_KEY_ARN),
            tags: &[("Owner", "TeamB")],
        },
        ImportCase {
            case: "a lowercase vhd at an https URL, nothing else named",
            format: "vhd",
            reported_format: "VHD",
            source: Source::Url("https://example.com/disks/legacy.vhd"),
            description: None,
            upload_size: None,
            encrypted: false,
            kms_key_id: None,
            tags: &[],
        },
    ]
}

/// The form fields of one row's request.
fn import_params(row: &ImportCase) -> Params {
    let mut params = vec![p("DiskContainer.Format", row.format)];
    match row.source {
        Source::Url(url) => params.push(p("DiskContainer.Url", url)),
        Source::Bucket(bucket, key) => {
            params.push(p("DiskContainer.UserBucket.S3Bucket", bucket));
            params.push(p("DiskContainer.UserBucket.S3Key", key));
        }
    }
    if let Some(description) = row.description {
        params.push(p("Description", description));
    }
    if let Some(upload_size) = row.upload_size {
        params.push(p("ClientData.UploadSize", upload_size.to_string()));
    }
    if row.encrypted {
        params.push(p("Encrypted", "true"));
    }
    if let Some(kms_key_id) = row.kms_key_id {
        params.push(p("KmsKeyId", kms_key_id));
    }
    if !row.tags.is_empty() {
        params.push(p(
            "TagSpecification.1.ResourceType",
            IMPORT_SNAPSHOT_TASK_TAGGABLE_RESOURCE,
        ));
        for (index, (key, value)) in row.tags.iter().enumerate() {
            let at = index + 1;
            params.push(p(&format!("TagSpecification.1.Tag.{at}.Key"), *key));
            params.push(p(&format!("TagSpecification.1.Tag.{at}.Value"), *value));
        }
    }
    params
}

/// The sets nested inside an import snapshot task: a scalar member of the task itself is
/// never read from inside one of these.
const NESTED_SETS: [&str; 1] = ["tagSet"];

/// One import snapshot task as the assertions read it, from either operation's body: the
/// task's own members, the members of its `SnapshotTaskDetail`, and its tags.
#[derive(Debug, Clone, PartialEq)]
struct TaskRow {
    import_task_id: String,
    description: Option<String>,
    detail_description: Option<String>,
    status: Option<String>,
    status_message: Option<String>,
    progress: Option<String>,
    format: Option<String>,
    disk_image_size: Option<String>,
    encrypted: Option<String>,
    kms_key_id: Option<String>,
    snapshot_id: Option<String>,
    url: Option<String>,
    s3_bucket: Option<String>,
    s3_key: Option<String>,
    tags: Vec<(String, String)>,
}

/// The FIRST import snapshot task a response body carries — `ImportSnapshot`'s own response
/// is one such task, and a describe narrowed to one id carries exactly one.
fn task_row(case: &str, body: &str) -> TaskRow {
    let parsed = document(case, body);
    let task = parsed
        .descendants()
        .find(|node| node.is_element() && child(*node, "importTaskId").is_some())
        .unwrap_or_else(|| panic!("case {case:?}: no import task (no <importTaskId>) in {body}"));
    let detail = child(task, "snapshotTaskDetail")
        .unwrap_or_else(|| panic!("case {case:?}: no <snapshotTaskDetail> in {body}"));
    let bucket = child(detail, "userBucket");
    let tags = child(task, "tagSet")
        .map(|set| {
            set.children()
                .filter(roxmltree::Node::is_element)
                .map(|item| {
                    (
                        child_text(item, "key").unwrap_or_default(),
                        child_text(item, "value").unwrap_or_default(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    TaskRow {
        import_task_id: child_text(task, "importTaskId").unwrap_or_default(),
        description: task
            .children()
            .filter(|node| node.is_element() && !NESTED_SETS.contains(&node.tag_name().name()))
            .find(|node| node.tag_name().name() == "description")
            .map(|node| node.text().unwrap_or_default().to_string()),
        detail_description: child_text(detail, "description"),
        status: child_text(detail, "status"),
        status_message: child_text(detail, "statusMessage"),
        progress: child_text(detail, "progress"),
        format: child_text(detail, "format"),
        disk_image_size: child_text(detail, "diskImageSize"),
        encrypted: child_text(detail, "encrypted"),
        kms_key_id: child_text(detail, "kmsKeyId"),
        snapshot_id: child_text(detail, "snapshotId"),
        url: child_text(detail, "url"),
        s3_bucket: bucket.and_then(|bucket| child_text(bucket, "s3Bucket")),
        s3_key: bucket.and_then(|bucket| child_text(bucket, "s3Key")),
        tags,
    }
}

/// The task `import_task_id` as `DescribeImportSnapshotTasks` reports it.
async fn described_task(router: &MultiServiceRouter, case: &str, import_task_id: &str) -> TaskRow {
    let (status, body) = describe(router, &[p("ImportTaskId.1", import_task_id)]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImportSnapshotTasks, got {status} \
         with body {body}"
    );
    task_row(case, &body)
}

/// Every import task id the store holds, as the unnarrowed describe reports them.
async fn stored_task_ids(router: &MultiServiceRouter, case: &str) -> Vec<String> {
    let (status, body) = describe(router, &[]).await;
    assert_eq!(
        status, 200,
        "case {case:?}: expected HTTP 200 from DescribeImportSnapshotTasks, got {status} \
         with body {body}"
    );
    elements(&body, "importTaskId")
}

// ---------------------------------------------------------------------------
// RULE 1: the task is started from the request and answered out of the store
// ---------------------------------------------------------------------------

/// Every member the request sets is reported on the task the response carries and on the
/// task a later describe reports, on the element the model names for it; the task id is the
/// service's to mint, the state is `active`, no snapshot exists yet, and the progress is a
/// percentage of an import that has not finished.
#[tokio::test]
async fn an_import_starts_an_active_task_the_store_reports_back() {
    for row in import_cases() {
        let case = row.case;
        let (router, _engine) = emulator();

        let (status, body) = import(&router, &import_params(&row)).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = task_row(case, &body);

        let import_task_id = reported.import_task_id.clone();
        assert!(
            import_task_id.starts_with(IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN)
                && import_task_id.len() > IMPORT_SNAPSHOT_TASK_ID_PREFIX_WITH_HYPHEN.len(),
            "case {case:?}: expected a service-minted import snapshot task id, got \
             {import_task_id:?}"
        );

        let (expected_url, expected_bucket, expected_key) = match row.source {
            Source::Url(url) => (Some(url.to_string()), None, None),
            Source::Bucket(bucket, key) => {
                (None, Some(bucket.to_string()), Some(key.to_string()))
            }
        };
        let expected = TaskRow {
            import_task_id: import_task_id.clone(),
            description: row.description.map(str::to_string),
            // One request field, reported on the task and on its detail alike.
            detail_description: row.description.map(str::to_string),
            status: Some(IMPORT_TASK_STATE_ACTIVE.to_string()),
            status_message: Some(IMPORT_SNAPSHOT_TASK_STATUS_MESSAGE_PENDING.to_string()),
            // Derived from the import's window, asserted on its own below.
            progress: reported.progress.clone(),
            format: Some(row.reported_format.to_string()),
            disk_image_size: reported.disk_image_size.clone(),
            encrypted: Some(row.encrypted.to_string()),
            kms_key_id: row.kms_key_id.map(str::to_string),
            // "The snapshot ID of the disk being imported": there is none until the clock
            // has read the disk.
            snapshot_id: None,
            url: expected_url,
            s3_bucket: expected_bucket,
            s3_key: expected_key,
            tags: row
                .tags
                .iter()
                .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                .collect(),
        };
        assert_eq!(
            reported, expected,
            "case {case:?}: expected the task the request describes, got {reported:?}"
        );

        let size = reported
            .disk_image_size
            .as_deref()
            .map(|size| {
                size.parse::<f64>().unwrap_or_else(|_| {
                    panic!("case {case:?}: diskImageSize must be a number, got {size:?}")
                })
            });
        assert_eq!(
            size, row.upload_size,
            "case {case:?}: expected the uploaded disk's size on the task, got {size:?}"
        );

        let progress = reported
            .progress
            .as_deref()
            .unwrap_or_else(|| panic!("case {case:?}: an active import reports a percentage"))
            .parse::<u64>()
            .unwrap_or_else(|_| {
                panic!(
                    "case {case:?}: progress must be a percentage, got {:?}",
                    reported.progress
                )
            });
        assert!(
            progress <= MAX_ACTIVE_IMPORT_PROGRESS,
            "case {case:?}: an import that has not finished reports at most \
             {MAX_ACTIVE_IMPORT_PROGRESS}, got {progress}"
        );

        // The SAME task a later read reports, which is what makes the response the store's
        // and not the request object's. Only the members a WRITER set are compared here: the
        // state, the status message, the progress and the snapshot are the clock's, and the
        // conversion window may have fallen due between the two calls, so what the clock owes
        // a task over time is asserted by the conversion table below instead.
        let described = described_task(&router, case, &import_task_id).await;
        assert_eq!(
            described.import_task_id, reported.import_task_id,
            "case {case:?}: the describe must report the task the import started, got \
             {described:?}"
        );
        for (member, expected, actual) in [
            ("description", &expected.description, &described.description),
            (
                "snapshotTaskDetail/description",
                &expected.detail_description,
                &described.detail_description,
            ),
            ("format", &expected.format, &described.format),
            ("encrypted", &expected.encrypted, &described.encrypted),
            ("kmsKeyId", &expected.kms_key_id, &described.kms_key_id),
            ("url", &expected.url, &described.url),
            ("s3Bucket", &expected.s3_bucket, &described.s3_bucket),
            ("s3Key", &expected.s3_key, &described.s3_key),
        ] {
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the describe to report {member} {expected:?}, got \
                 {actual:?}"
            );
        }
        assert_eq!(
            described.tags, expected.tags,
            "case {case:?}: expected the describe to report the task's tags, got {described:?}"
        );

        // One call, one task.
        let stored = stored_task_ids(&router, case).await;
        assert_eq!(
            stored,
            vec![import_task_id.clone()],
            "case {case:?}: expected exactly the task this import started, got {stored:?}"
        );
    }
}

/// The three formats a snapshot disk container may carry are exactly the catalog the model
/// publishes for the member, so the table above covers each of them once and nothing else.
#[test]
fn the_success_table_covers_every_published_disk_image_format() {
    let mut covered: Vec<&str> = import_cases()
        .iter()
        .map(|row| row.reported_format)
        .collect();
    covered.sort_unstable();
    let mut published: Vec<&str> = IMPORT_SNAPSHOT_DISK_IMAGE_FORMATS.to_vec();
    published.sort_unstable();
    assert_eq!(
        covered, published,
        "expected one row per published disk image format, got {covered:?} against \
         {published:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 2: the clock makes the snapshot, and the task waits for it
// ---------------------------------------------------------------------------

/// One row: an import, and the snapshot the clock must make from it.
struct ConversionCase {
    case: &'static str,
    params: Params,
    /// "The size of the uploaded disk image, in GiB" the request named, which is the size of
    /// the volume the snapshot reports.
    volume_size: Option<&'static str>,
    encrypted: bool,
    description: Option<&'static str>,
}

fn conversion_cases() -> Vec<ConversionCase> {
    vec![
        ConversionCase {
            case: "a sized, described VMDK",
            params: vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("ClientData.UploadSize", "8"),
                p("Description", "nightly appliance disk"),
            ],
            volume_size: Some("8"),
            encrypted: false,
            description: Some("nightly appliance disk"),
        },
        ConversionCase {
            case: "an encrypted RAW image of a fractional size, undescribed",
            params: vec![
                p("DiskContainer.Format", "RAW"),
                p("DiskContainer.UserBucket.S3Bucket", "disk-images"),
                p("DiskContainer.UserBucket.S3Key", "golden/volume.raw"),
                p("ClientData.UploadSize", "16.5"),
                p("Encrypted", "true"),
                p("KmsKeyId", KMS_KEY_ARN),
            ],
            // A disk of 16.5 GiB does not fit in a 16 GiB volume, so the volume the snapshot
            // reports is the next whole GiB up.
            volume_size: Some("17"),
            encrypted: true,
            description: None,
        },
        ConversionCase {
            case: "a VHD whose upload size the request never named",
            params: vec![
                p("DiskContainer.Format", "VHD"),
                p("DiskContainer.Url", "s3://disk-images/legacy.vhd"),
            ],
            volume_size: None,
            encrypted: false,
            description: None,
        },
    ]
}

/// A task is handed back `active` with no snapshot, and the CLOCK turns the disk into an EBS
/// snapshot of this account: the task then reports that snapshot's id, `DescribeSnapshots`
/// reports the snapshot itself, and the task is `completed` only once the snapshot it made is.
#[tokio::test]
async fn the_clock_turns_the_disk_into_a_snapshot_of_the_account_and_completes_the_task() {
    for row in conversion_cases() {
        let case = row.case;
        let (router, _engine) = emulator();

        let (status, body) = import(&router, &row.params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let started = task_row(case, &body);
        assert_eq!(
            started.status.as_deref(),
            Some(IMPORT_TASK_STATE_ACTIVE),
            "case {case:?}: an import is handed back running, got {started:?}"
        );
        assert_eq!(
            started.snapshot_id, None,
            "case {case:?}: no snapshot exists before the disk has been read, got {started:?}"
        );

        // The account holds no snapshot yet either: the task reporting none and the snapshot
        // store holding none are the same fact.
        let (status, listed) = describe_snapshots(&router, &[p("Owner.1", "self")]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200 from DescribeSnapshots, got {status} with body \
             {listed}"
        );
        assert!(
            elements(&listed, "snapshotId").is_empty(),
            "case {case:?}: expected no snapshot while the import is still converting, got \
             {listed}"
        );

        past_the_whole_import().await;

        let settled = described_task(&router, case, &started.import_task_id).await;
        assert_eq!(
            settled.status.as_deref(),
            Some(IMPORT_TASK_STATE_COMPLETED),
            "case {case:?}: once the whole import window has passed the task is \
             {IMPORT_TASK_STATE_COMPLETED}, got {settled:?}"
        );
        assert_eq!(
            settled.progress, None,
            "case {case:?}: a finished import has no percentage left to make, got {settled:?}"
        );
        assert_eq!(
            settled.status_message, None,
            "case {case:?}: a finished import has no stage left to report, got {settled:?}"
        );
        let snapshot_id = settled.snapshot_id.clone().unwrap_or_else(|| {
            panic!("case {case:?}: a completed import reports the snapshot it made, got {settled:?}")
        });
        assert!(
            snapshot_id.starts_with("snap-"),
            "case {case:?}: expected the id EC2 mints for a snapshot, got {snapshot_id:?}"
        );

        // The snapshot is a snapshot of the ACCOUNT, reported by the store that owns one.
        let (status, listed) = describe_snapshots(&router, &[p("SnapshotId.1", &snapshot_id)]).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the imported snapshot to be addressable by its id, got \
             {status} with body {listed}"
        );
        let parsed = document(case, &listed);
        let snapshot = parsed
            .descendants()
            .find(|node| node.is_element() && child(*node, "snapshotId").is_some())
            .unwrap_or_else(|| panic!("case {case:?}: no snapshot in {listed}"));
        for (member, expected) in [
            ("snapshotId", Some(snapshot_id.clone())),
            ("status", Some(SNAPSHOT_STATE_COMPLETED.to_string())),
            ("progress", Some(SNAPSHOT_PROGRESS_COMPLETED.to_string())),
            (
                "ownerId",
                Some(engine::emulator_account_id().to_string()),
            ),
            ("encrypted", Some(row.encrypted.to_string())),
            ("volumeSize", row.volume_size.map(str::to_string)),
            ("description", row.description.map(str::to_string)),
        ] {
            let actual = child_text(snapshot, member);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the imported snapshot's {member} to be {expected:?}, \
                 got {actual:?} in {listed}"
            );
        }

        // Reading again changes nothing: both moves were owed by the clock, not caused by
        // the read.
        let again = described_task(&router, case, &started.import_task_id).await;
        assert_eq!(
            again, settled,
            "case {case:?}: describing a finished import must not change it, got {again:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// RULE 3: a request that does not name one importable disk is refused
// ---------------------------------------------------------------------------

/// One refusal row: the parameters under test and the EC2 error code the refusal carries.
type RefusalRow = (&'static str, Params, &'static str);

/// A request that fails to name exactly one disk image this emulator can import, or that
/// carries a value outside what the model publishes, is refused with EC2's own code.
///
/// The rows depart from an importable request in one way each: no container at all, two
/// sources for one disk, neither, no format, a format outside the published catalog (the two
/// container formats a WHOLE-MACHINE import takes but a bare-disk import does not, and a
/// format of no VM image at all), a KMS key without encryption, and a key identifier and an
/// idempotency token past the width each is held to.
fn refusal_rows() -> Vec<RefusalRow> {
    let long = |width: usize| "k".repeat(width + 1);
    vec![
        (
            "no disk container at all",
            vec![],
            "MissingParameter",
        ),
        (
            "a container naming no disk image",
            vec![p("DiskContainer.Format", "VMDK")],
            "MissingParameter",
        ),
        (
            "a container naming two sources for one disk",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("DiskContainer.UserBucket.S3Bucket", "disk-images"),
                p("DiskContainer.UserBucket.S3Key", "appliance.vmdk"),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a disk image with no format",
            vec![p("DiskContainer.Url", "s3://disk-images/appliance.vmdk")],
            "MissingParameter",
        ),
        (
            "OVA, a container format only a whole-machine import takes",
            vec![
                p("DiskContainer.Format", "OVA"),
                p("DiskContainer.Url", "s3://disk-images/appliance.ova"),
            ],
            "InvalidParameterValue",
        ),
        (
            "VHDX, the other format only a whole-machine import takes",
            vec![
                p("DiskContainer.Format", "VHDX"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vhdx"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a format of no VM disk image at all",
            vec![
                p("DiskContainer.Format", "qcow2"),
                p("DiskContainer.Url", "s3://disk-images/appliance.qcow2"),
            ],
            "InvalidParameterValue",
        ),
        (
            "a KMS key without encryption",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("KmsKeyId", KMS_KEY_ARN),
            ],
            "InvalidParameterCombination",
        ),
        (
            "a KMS key identifier past the width one may be",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("Encrypted", "true"),
                p("KmsKeyId", long(MAX_KMS_KEY_ID_CHARS)),
            ],
            "InvalidParameterValue",
        ),
        (
            "an idempotency token past the width one may be",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("ClientToken", long(MAX_CLIENT_TOKEN_CHARS)),
            ],
            "InvalidParameterValue",
        ),
        (
            "tags for a resource this call does not create",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("TagSpecification.1.ResourceType", "snapshot"),
                p("TagSpecification.1.Tag.1.Key", "Name"),
                p("TagSpecification.1.Tag.1.Value", "nightly"),
            ],
            "InvalidParameterValue",
        ),
    ]
}

#[tokio::test]
async fn a_request_that_names_no_importable_disk_is_refused_and_nothing_is_stored() {
    let (router, _engine) = emulator();

    for (case, params, expected_code) in refusal_rows() {
        let (status, body) = import(&router, &params).await;
        assert_eq!(
            status, 400,
            "case {case:?}: expected HTTP 400, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} in {body}"
        );
    }

    let stored = stored_task_ids(&router, "after every refusal").await;
    assert!(
        stored.is_empty(),
        "expected a refused import to store nothing, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 4: one token names one task
// ---------------------------------------------------------------------------

/// "Token to enable idempotency for VM import requests": a repeat of the same request under
/// one token reports the task that token started and starts no second import, while another
/// token starts its own.
#[tokio::test]
async fn one_client_token_names_one_import_snapshot_task() {
    let (router, _engine) = emulator();
    let base = vec![
        p("DiskContainer.Format", "VMDK"),
        p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
        p("Description", "nightly appliance disk"),
    ];
    let with_token = |token: &str| {
        let mut params = base.clone();
        params.push(p("ClientToken", token));
        params
    };

    let rows: Vec<(&str, Params)> = vec![
        ("the first call under a token", with_token("token-one")),
        ("the same request again", with_token("token-one")),
        (
            "the same request with a different description",
            {
                let mut params = with_token("token-one");
                params.retain(|(key, _)| key != "Description");
                params.push(p("Description", "a different description"));
                params
            },
        ),
    ];

    let mut first: Option<String> = None;
    for (case, params) in rows {
        let (status, body) = import(&router, &params).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {body}"
        );
        let reported = task_row(case, &body);
        match &first {
            None => first = Some(reported.import_task_id.clone()),
            Some(expected) => assert_eq!(
                &reported.import_task_id, expected,
                "case {case:?}: one token names one task, got {reported:?}"
            ),
        }
    }
    let replayed = first.expect("the first call reports a task");

    let (status, body) = import(&router, &with_token("token-two")).await;
    assert_eq!(
        status, 200,
        "another token: expected HTTP 200, got {status} with body {body}"
    );
    let second = task_row("another token", &body).import_task_id;
    assert_ne!(
        second, replayed,
        "another token starts its own import, got {second}"
    );

    let mut stored = stored_task_ids(&router, "two tokens").await;
    stored.sort();
    let mut expected = vec![replayed, second];
    expected.sort();
    assert_eq!(
        stored, expected,
        "expected one task per token, got {stored:?}"
    );
}

// ---------------------------------------------------------------------------
// RULE 5: a dry run reports what would have happened
// ---------------------------------------------------------------------------

/// `DryRun` asks whether the request WOULD have worked: one that would have succeeded is
/// answered `DryRunOperation` and stores nothing, and one that would have been refused is
/// answered with that refusal instead.
#[tokio::test]
async fn a_dry_run_import_reports_what_would_have_happened_and_writes_nothing() {
    let (router, _engine) = emulator();

    let rows: Vec<(&str, Params, u16, &str)> = vec![
        (
            "a request that would have succeeded",
            vec![
                p("DiskContainer.Format", "VMDK"),
                p("DiskContainer.Url", "s3://disk-images/appliance.vmdk"),
                p("DryRun", "true"),
            ],
            412,
            "DryRunOperation",
        ),
        (
            "a request that would have been refused for its format",
            vec![
                p("DiskContainer.Format", "qcow2"),
                p("DiskContainer.Url", "s3://disk-images/appliance.qcow2"),
                p("DryRun", "true"),
            ],
            400,
            "InvalidParameterValue",
        ),
        (
            "a request that would have been refused for naming no disk",
            vec![p("DryRun", "true")],
            400,
            "MissingParameter",
        ),
    ];

    for (case, params, expected_status, expected_code) in rows {
        let (status, body) = import(&router, &params).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {status} with body {body}"
        );
        let code = error_code(&body);
        assert_eq!(
            code, expected_code,
            "case {case:?}: expected {expected_code}, got {code} in {body}"
        );
    }

    let stored = stored_task_ids(&router, "after the dry runs").await;
    assert!(
        stored.is_empty(),
        "expected a dry run to store nothing, got {stored:?}"
    );
    let (status, listed) = describe_snapshots(&router, &[p("Owner.1", "self")]).await;
    assert_eq!(
        status, 200,
        "expected HTTP 200 from DescribeSnapshots, got {status} with body {listed}"
    );
    assert!(
        elements(&listed, "snapshotId").is_empty(),
        "expected a dry run to make no snapshot, got {listed}"
    );
}
