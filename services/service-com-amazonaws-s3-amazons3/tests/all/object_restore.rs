// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `RestoreObject`: it initiates the restore of a temporary copy of
//! an ARCHIVED object version, and `HeadObject` / `GetObject` report that restore's state
//! back through `x-amz-restore`.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_versioning`] already own), so what is pinned is the wire behaviour —
//! status, bound headers, error code and message — rather than any handler's internals.
//!
//! The rules under test, with their sources:
//!
//! * `RestoreObject` "restores an archived copy of an object back into Amazon S3", so only
//!   a version whose storage class is an archive one has a copy to restore: every other
//!   storage class is refused with HTTP 403 `InvalidObjectState` and the message S3 words
//!   for that cause, and nothing is recorded. The archive classes are
//!   [`ARCHIVED_STORAGE_CLASSES`]; the refusal table spans every OTHER class a general
//!   purpose bucket can host and a client write may name ([`GENERAL_PURPOSE_STORAGE_CLASSES`]
//!   less [`SERVICE_PLACED_STORAGE_CLASSES`]).
//! * A retrieval `Tier` the object's own class does not OFFER is refused the same way: S3
//!   Glacier Flexible Retrieval offers Expedited, Standard and Bulk, S3 Glacier Deep Archive
//!   offers Standard and Bulk alone (Amazon S3 User Guide, *Archive retrieval options*), so
//!   each row below names a tier its class offers and
//!   [`super::object_write_refusals`] spans the pairs it does not.
//! * A restore the service takes answers the status its own documentation states for what
//!   the call DID: "if the object is not previously restored, then Amazon S3 returns `202
//!   Accepted` in the response. If the object is previously restored, Amazon S3 returns `200
//!   OK` in the response." Either way the retrieval is NOT complete when the call returns —
//!   "you must first initiate a restore request, and then wait until a temporary copy of
//!   the object is available" — so a reader reports `ongoing-request="true"` until the
//!   retrieval falls due and then `ongoing-request="false", expiry-date="…"`, the expiry
//!   being `RestoreRequest.Days` days ("lifetime of the active copy in days") out at
//!   midnight UTC (`HeadObjectOutput$Restore` publishes both renderings).
//! * A restore of a version whose retrieval is still running is refused with HTTP 409
//!   `RestoreAlreadyInProgress`; one over a retrieval that has fallen due is served and
//!   states the lifetime of the copy afresh.
//! * The restore belongs to the addressed object VERSION — "to restore a specific object
//!   version, you can provide a version ID. If you don't provide a version ID, Amazon S3
//!   restores the current version" — so it follows neither another version of the same key
//!   nor another key.
//! * A call whose address this emulator does not hold answers which part is missing
//!   (`NoSuchBucket`, `NoSuchKey`, `NoSuchVersion`, each HTTP 404; a delete marker is a 405)
//!   and writes nothing, and an `x-amz-expected-bucket-owner` that is not the account this
//!   emulator serves is "403 Forbidden (access denied)".

use std::time::{Duration, SystemTime};

use aws_smithy_types::date_time::Format;
use aws_smithy_types::DateTime;
use service_com_amazonaws_s3_amazons3::ops::types::TIER_VALUES;
use service_com_amazonaws_s3_amazons3::service::object_write::{
    GENERAL_PURPOSE_STORAGE_CLASSES, SERVICE_PLACED_STORAGE_CLASSES,
};
use service_com_amazonaws_s3_amazons3::state::{
    ARCHIVED_STORAGE_CLASSES, DEFAULT_STORAGE_CLASS, MALFORMED_XML_MESSAGE,
    RESTORE_ALREADY_IN_PROGRESS_MESSAGE, RESTORE_COMPLETION_DELAY, RESTORE_NOT_ALLOWED_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, head_object, set_versioning};

/// An account id that is NOT the one account this emulator serves, so an
/// `x-amz-expected-bucket-owner` carrying it names an owner no bucket here has.
const NOT_THE_OWNER: &str = "123456789012";

/// Seconds in a day — what a `Days` lifetime is measured in.
const SECONDS_PER_DAY: i64 = 86_400;

/// How long a row waits for a retrieval to fall due: the span the store records plus slack
/// for a loaded machine.
///
/// A blocking sleep rather than `tokio::time::sleep(..).await`: the workspace's tokio
/// carries only the `macros`/`rt` features, so its timer driver is not built.
fn wait_for_the_copy() {
    std::thread::sleep(RESTORE_COMPLETION_DELAY + Duration::from_millis(250));
}

/// `POST /<bucket>/<key>?restore` — `RestoreObject`, with whatever extra query parameters
/// and headers the case carries and `payload` as the `RestoreRequest` container.
async fn restore_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
    payload: &str,
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket}/{key}?restore{query}"),
        headers,
        payload,
    )
    .await
}

/// Where a row names the retrieval tier: both `RestoreRequest$Tier` and
/// `GlacierJobParameters$Tier` name it, and a request that names neither takes the
/// documented default.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TierNamedIn {
    /// `<Tier>` directly inside the container.
    Request(&'static str),
    /// `<GlacierJobParameters><Tier>` — "S3 Glacier related parameters pertaining to this
    /// job".
    GlacierJobParameters(&'static str),
    /// Neither, so the default applies.
    Nothing,
}

impl TierNamedIn {
    /// The tier this names, if any — what the row expects the restore to be processed at.
    fn tier(self) -> Option<&'static str> {
        match self {
            Self::Request(tier) | Self::GlacierJobParameters(tier) => Some(tier),
            Self::Nothing => None,
        }
    }
}

/// The `RestoreRequest` container a restore carries: the members the case names, and
/// nothing else.
fn restore_payload(days: Option<i32>, tier: TierNamedIn) -> String {
    let mut body = String::from("<RestoreRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">");
    if let Some(days) = days {
        body.push_str(&format!("<Days>{days}</Days>"));
    }
    match tier {
        TierNamedIn::Request(tier) => body.push_str(&format!("<Tier>{tier}</Tier>")),
        TierNamedIn::GlacierJobParameters(tier) => body.push_str(&format!(
            "<GlacierJobParameters><Tier>{tier}</Tier></GlacierJobParameters>"
        )),
        TierNamedIn::Nothing => {}
    }
    body.push_str("</RestoreRequest>");
    body
}

/// An object written at `key` in `storage_class`, so a restore has a version to act on.
/// Reports the version id the write minted, when the bucket's versioning state made it mint
/// one.
async fn object_in(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    storage_class: &str,
) -> Option<String> {
    let written = put_object(
        router,
        bucket_name,
        key,
        b"archived payload",
        &[("x-amz-storage-class", storage_class)],
    )
    .await;
    assert_eq!(
        written.status, 200,
        "case {storage_class:?}: the object {bucket_name}/{key} must be written first, got {} \
         with body {:?}",
        written.status,
        text(&written)
    );
    written.header("x-amz-version-id")
}

/// The epoch seconds the `expiry-date` of an `x-amz-restore` header names, read back through
/// the HTTP date format the member carries it in — so a row compares INSTANTS rather than
/// two renderings of one.
fn expiry_seconds(header: &str) -> i64 {
    let rendered = header
        .split_once("expiry-date=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map(|(date, _)| date.to_string())
        .unwrap_or_else(|| panic!("the restore header {header:?} must carry an expiry-date"));
    DateTime::from_str(&rendered, Format::HttpDate)
        .unwrap_or_else(|error| {
            panic!("the reported expiry-date {rendered:?} must be an HTTP date, got {error}")
        })
        .secs()
}

/// Now, in epoch seconds: what a row's expected expiry is an offset from.
fn now_seconds() -> i64 {
    DateTime::from(SystemTime::now()).secs()
}

/// One row of the restore table: the case name, the key the object is stored at, the
/// storage class it is archived in, the `Days` the restore asks for, and where the row
/// names the tier.
type RestoreRow = (&'static str, &'static str, &'static str, i32, TierNamedIn);

/// The table of restores every row of which must be initiated, reported as ongoing and then
/// reported as an available copy.
///
/// The rows differ along what the initiate-and-report rule generalises over: which archive
/// storage class the object is in (both values of [`ARCHIVED_STORAGE_CLASSES`]), how long
/// the copy lives (a day, a week, a month, a year), where the retrieval tier is named (the
/// container, the Glacier job parameters, nowhere) and the shape of the key (a flat name, a
/// prefixed one, a deep path, one with dots).
fn restore_rows() -> Vec<RestoreRow> {
    vec![
        (
            "glacier, one day, Expedited named in the container",
            "flat-key",
            "GLACIER",
            1,
            TierNamedIn::Request("Expedited"),
        ),
        (
            "deep archive, a week, Standard named in the container",
            "archive/weekly.bin",
            "DEEP_ARCHIVE",
            7,
            TierNamedIn::Request("Standard"),
        ),
        (
            "glacier, a year, Bulk named in the Glacier job parameters",
            "a/deep/path/ledger.bin",
            "GLACIER",
            365,
            TierNamedIn::GlacierJobParameters("Bulk"),
        ),
        (
            "deep archive, a month, no tier named at all",
            "dotted.name.txt",
            "DEEP_ARCHIVE",
            30,
            TierNamedIn::Nothing,
        ),
    ]
}

#[tokio::test]
async fn restoring_an_archived_object_reports_an_ongoing_retrieval_then_an_available_copy() {
    let router = router();
    bucket(&router, "object-restore").await;

    // The rows span the whole published tier catalog and both archive storage classes, so
    // the table cannot quietly stop covering a value the model declares.
    let mut tiers: Vec<&str> = restore_rows().iter().filter_map(|row| row.4.tier()).collect();
    tiers.sort_unstable();
    tiers.dedup();
    let mut published: Vec<&str> = TIER_VALUES.to_vec();
    published.sort_unstable();
    assert_eq!(
        tiers, published,
        "the table must cover every Tier the model publishes, got {tiers:?}"
    );
    let mut classes: Vec<&str> = restore_rows().iter().map(|row| row.2).collect();
    classes.sort_unstable();
    classes.dedup();
    let mut archived: Vec<&str> = ARCHIVED_STORAGE_CLASSES.to_vec();
    archived.sort_unstable();
    assert_eq!(
        classes, archived,
        "the table must cover every archive storage class, got {classes:?}"
    );

    for (case, key, storage_class, days, tier) in restore_rows() {
        object_in(&router, "object-restore", key, storage_class).await;

        // A version nobody has asked to restore reports no restore at all: the header is
        // sent only once a retrieval is in progress or a copy is there.
        let before = head_object(&router, "object-restore", key, "").await;
        assert_eq!(
            before.header("x-amz-restore"),
            None,
            "case {case:?}: an object nobody restored must report no restore state, got {:?}",
            before.header("x-amz-restore")
        );

        let initiated = restore_object(
            &router,
            "object-restore",
            key,
            "",
            &[],
            &restore_payload(Some(days), tier),
        )
        .await;
        assert_eq!(
            initiated.status, 202,
            "case {case:?}: expected the 202 of a retrieval this call initiated, got {} with \
             body {:?}",
            initiated.status,
            text(&initiated)
        );
        // Requester Pays is not emulated and only a select restore writes its output
        // somewhere, so neither bound response header is reported.
        assert_eq!(
            initiated.header("x-amz-request-charged"),
            None,
            "case {case:?}: no requester was charged, got {:?}",
            initiated.header("x-amz-request-charged")
        );
        assert_eq!(
            initiated.header("x-amz-restore-output-path"),
            None,
            "case {case:?}: a regular restore writes no output path, got {:?}",
            initiated.header("x-amz-restore-output-path")
        );

        // The copy is NOT there when the call returns: a HEAD of the object reports the
        // retrieval as ongoing, and a GET of it is still barred by the storage class the
        // bytes are in — "before you can retrieve the object you must first restore a copy
        // using RestoreObject. Otherwise, this operation returns an InvalidObjectState
        // error" (`GetObject`'s documentation of that error), and a retrieval that has not
        // fallen due has made no copy to retrieve.
        let ongoing = head_object(&router, "object-restore", key, "").await;
        assert_eq!(
            ongoing.header("x-amz-restore").as_deref(),
            Some("ongoing-request=\"true\""),
            "case {case:?}: the retrieval must be reported as ongoing, got {:?}",
            ongoing.header("x-amz-restore")
        );
        let read = get_object(&router, "object-restore", key, "", &[]).await;
        assert_eq!(
            read.status, 403,
            "case {case:?}: a read of the object while the retrieval is ongoing must be \
             refused, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.error_code(),
            "InvalidObjectState",
            "case {case:?}: expected the InvalidObjectState code, got {:?}",
            read.error_code()
        );

        // Once the retrieval falls due the copy is there, and the header says when S3 is
        // scheduled to delete it: `days` days out, at a midnight UTC.
        let due_from = now_seconds();
        wait_for_the_copy();
        let available = head_object(&router, "object-restore", key, "").await;
        let header = available.header("x-amz-restore").unwrap_or_else(|| {
            panic!("case {case:?}: a restored copy must report a restore state")
        });
        assert!(
            header.starts_with("ongoing-request=\"false\""),
            "case {case:?}: expected the retrieval to have fallen due, got {header:?}"
        );
        let expiry = expiry_seconds(&header);
        assert_eq!(
            expiry.rem_euclid(SECONDS_PER_DAY),
            0,
            "case {case:?}: expected the expiry at a midnight UTC, got {expiry} from {header:?}"
        );
        let earliest = due_from + i64::from(days) * SECONDS_PER_DAY;
        assert!(
            expiry >= earliest && expiry < earliest + SECONDS_PER_DAY,
            "case {case:?}: expected an expiry in {earliest}..{} ({days} days out, rounded up \
             to midnight), got {expiry} from {header:?}",
            earliest + SECONDS_PER_DAY
        );

        // Restoring changes nothing else about the version: its bytes and its storage class
        // read back unchanged, because what a restore makes available is a temporary COPY.
        let object_read = get_object(&router, "object-restore", key, "", &[]).await;
        assert_eq!(
            object_read.body.as_ref(),
            b"archived payload",
            "case {case:?}: a restore must not touch the object's bytes, got {:?}",
            text(&object_read)
        );
        assert_eq!(
            available.header("x-amz-storage-class").as_deref(),
            Some(storage_class),
            "case {case:?}: a restore must not move the object out of its storage class, got \
             {:?}",
            available.header("x-amz-storage-class")
        );
    }
}

#[tokio::test]
async fn restoring_an_object_that_is_not_archived_is_refused_for_its_storage_class() {
    let router = router();
    bucket(&router, "restore-not-allowed").await;

    // Every storage class a general purpose bucket can host that a client write may NAME and
    // that is NOT an archive one, plus the class a write that named none is stored in — the
    // whole catalog the rule divides. A class belonging to another kind of bucket, and one
    // only another AWS service places objects in, never reaches a restore at all: the write
    // naming it is itself refused.
    let not_archived: Vec<&str> = GENERAL_PURPOSE_STORAGE_CLASSES
        .iter()
        .copied()
        .filter(|class| {
            !ARCHIVED_STORAGE_CLASSES.contains(class)
                && !SERVICE_PLACED_STORAGE_CLASSES.contains(class)
        })
        .collect();
    assert_eq!(
        not_archived.len(),
        GENERAL_PURPOSE_STORAGE_CLASSES.len()
            - ARCHIVED_STORAGE_CLASSES.len()
            - SERVICE_PLACED_STORAGE_CLASSES.len(),
        "the catalog must divide into archive, service-placed and plain classes, got \
         {not_archived:?}"
    );
    assert!(
        not_archived.contains(&DEFAULT_STORAGE_CLASS),
        "the class a write that names none is stored in must be among them, got {not_archived:?}"
    );

    for (index, storage_class) in not_archived.iter().enumerate() {
        let key = format!("not-archived-{index}");
        object_in(&router, "restore-not-allowed", &key, storage_class).await;

        let refused = restore_object(
            &router,
            "restore-not-allowed",
            &key,
            "",
            &[],
            &restore_payload(Some(1), TierNamedIn::Request("Standard")),
        )
        .await;
        assert_eq!(
            refused.status, 403,
            "case {storage_class:?}: expected HTTP 403 restoring an object that is not \
             archived, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidObjectState",
            "case {storage_class:?}: expected InvalidObjectState, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            RESTORE_NOT_ALLOWED_MESSAGE,
            "case {storage_class:?}: expected the storage-class wording, got {:?}",
            refused.error_message()
        );

        // The refusal wrote nothing: the version still reports no restore state.
        let read = head_object(&router, "restore-not-allowed", &key, "").await;
        assert_eq!(
            read.header("x-amz-restore"),
            None,
            "case {storage_class:?}: a refused restore must record nothing, got {:?}",
            read.header("x-amz-restore")
        );
    }
}

#[tokio::test]
async fn a_restore_while_one_is_in_progress_is_refused_and_a_later_one_restates_the_lifetime() {
    let router = router();
    bucket(&router, "restore-in-progress").await;
    object_in(&router, "restore-in-progress", "twice", "GLACIER").await;

    let first = restore_object(
        &router,
        "restore-in-progress",
        "twice",
        "",
        &[],
        &restore_payload(Some(1), TierNamedIn::Request("Standard")),
    )
    .await;
    assert_eq!(
        first.status, 202,
        "case first: the restore must be initiated with a 202, got {} with body {:?}",
        first.status,
        text(&first)
    );

    // The retrieval is still running, so a second restore of the same version is a
    // conflict rather than a second job.
    let conflicting = restore_object(
        &router,
        "restore-in-progress",
        "twice",
        "",
        &[],
        &restore_payload(Some(1), TierNamedIn::Request("Standard")),
    )
    .await;
    assert_eq!(
        conflicting.status, 409,
        "case while in progress: expected HTTP 409, got {} with body {:?}",
        conflicting.status,
        text(&conflicting)
    );
    assert_eq!(
        conflicting.error_code(),
        "RestoreAlreadyInProgress",
        "case while in progress: expected RestoreAlreadyInProgress, got {:?}",
        conflicting.error_code()
    );
    assert_eq!(
        conflicting.error_message(),
        RESTORE_ALREADY_IN_PROGRESS_MESSAGE,
        "case while in progress: expected the published cause, got {:?}",
        conflicting.error_message()
    );

    wait_for_the_copy();
    let one_day = head_object(&router, "restore-in-progress", "twice", "").await;
    let one_day_header = one_day
        .header("x-amz-restore")
        .unwrap_or_else(|| panic!("case after the copy: a restored copy must report its state"));
    let one_day_expiry = expiry_seconds(&one_day_header);

    // A restore over a retrieval that has fallen due is served, and states the lifetime of
    // the copy afresh: a longer `Days` moves the expiry out.
    let again = restore_object(
        &router,
        "restore-in-progress",
        "twice",
        "",
        &[],
        &restore_payload(Some(10), TierNamedIn::Request("Bulk")),
    )
    .await;
    assert_eq!(
        again.status, 200,
        "case restating the lifetime: a version whose copy is already there answers 200, got \
         {} with body {:?}",
        again.status,
        text(&again)
    );
    let ongoing = head_object(&router, "restore-in-progress", "twice", "").await;
    assert_eq!(
        ongoing.header("x-amz-restore").as_deref(),
        Some("ongoing-request=\"true\""),
        "case restating the lifetime: the fresh retrieval must be ongoing, got {:?}",
        ongoing.header("x-amz-restore")
    );
    wait_for_the_copy();
    let ten_days = head_object(&router, "restore-in-progress", "twice", "").await;
    let ten_days_header = ten_days.header("x-amz-restore").unwrap_or_else(|| {
        panic!("case restating the lifetime: the second copy must report its state")
    });
    let ten_days_expiry = expiry_seconds(&ten_days_header);
    assert!(
        ten_days_expiry >= one_day_expiry + 9 * SECONDS_PER_DAY,
        "case restating the lifetime: expected the expiry at least nine days past {one_day_expiry}, \
         got {ten_days_expiry} from {ten_days_header:?}"
    );
}

#[tokio::test]
async fn a_restore_belongs_to_the_object_version_it_names() {
    let router = router();
    bucket(&router, "restore-versions").await;
    set_versioning(&router, "restore-versions", "Enabled").await;

    let older = object_in(&router, "restore-versions", "versioned", "GLACIER")
        .await
        .expect("a versioned write must mint a version id");
    let newer = object_in(&router, "restore-versions", "versioned", "GLACIER")
        .await
        .expect("a versioned write must mint a version id");
    // Another key in the same bucket, to pin that a restore does not follow the bucket.
    object_in(&router, "restore-versions", "untouched", "GLACIER").await;

    let initiated = restore_object(
        &router,
        "restore-versions",
        "versioned",
        &format!("&versionId={older}"),
        &[],
        &restore_payload(Some(2), TierNamedIn::Request("Standard")),
    )
    .await;
    assert_eq!(
        initiated.status, 202,
        "case the older version: the restore must be initiated with a 202, got {} with body \
         {:?}",
        initiated.status,
        text(&initiated)
    );

    for (case, key, query, expected) in [
        (
            "the version the restore named",
            "versioned",
            format!("?versionId={older}"),
            Some("ongoing-request=\"true\""),
        ),
        (
            "the other version of the same key",
            "versioned",
            format!("?versionId={newer}"),
            None,
        ),
        ("the key's current version", "versioned", String::new(), None),
        ("another key", "untouched", String::new(), None),
    ] {
        let read = head_object(&router, "restore-versions", key, &query).await;
        assert_eq!(
            read.header("x-amz-restore").as_deref(),
            expected,
            "case {case:?}: expected {expected:?}, got {:?}",
            read.header("x-amz-restore")
        );
    }
}

/// One row of the refusal table: the case name, the request that provokes the refusal, and
/// the status, wire code and `<Message>` it must answer with (`None` where the row pins
/// only the code and the status).
struct RefusalRow {
    case: &'static str,
    bucket: &'static str,
    key: &'static str,
    query: String,
    headers: Vec<(&'static str, &'static str)>,
    payload: String,
    status: u16,
    code: &'static str,
    message: Option<&'static str>,
}

#[tokio::test]
async fn a_restore_this_emulator_cannot_serve_answers_what_is_wrong_with_it() {
    let router = router();
    bucket(&router, "restore-refusals").await;
    set_versioning(&router, "restore-refusals", "Enabled").await;
    object_in(&router, "restore-refusals", "archived", "GLACIER").await;
    // A key whose current version is a delete marker, so a call on that marker has no
    // object behind it.
    object_in(&router, "restore-refusals", "deleted", "GLACIER").await;
    let marker = delete_object(&router, "restore-refusals", "deleted", "").await;
    let marker_version = marker
        .header("x-amz-version-id")
        .expect("a delete on a versioned bucket must mint the marker's version id");

    let a_day = restore_payload(Some(1), TierNamedIn::Request("Standard"));
    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "a bucket this emulator does not hold",
            bucket: "restore-absent",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: a_day.clone(),
            status: 404,
            code: "NoSuchBucket",
            message: None,
        },
        RefusalRow {
            case: "a key the bucket does not hold",
            bucket: "restore-refusals",
            key: "absent",
            query: String::new(),
            headers: Vec::new(),
            payload: a_day.clone(),
            status: 404,
            code: "NoSuchKey",
            message: None,
        },
        RefusalRow {
            case: "a versionId the bucket holds no version for",
            bucket: "restore-refusals",
            key: "archived",
            query: String::from("&versionId=not-a-version-this-store-minted"),
            headers: Vec::new(),
            payload: a_day.clone(),
            status: 404,
            code: "NoSuchVersion",
            message: None,
        },
        RefusalRow {
            case: "a version that is a delete marker",
            bucket: "restore-refusals",
            key: "deleted",
            query: format!("&versionId={marker_version}"),
            headers: Vec::new(),
            payload: a_day.clone(),
            status: 405,
            code: "MethodNotAllowed",
            message: None,
        },
        RefusalRow {
            case: "an expected bucket owner that owns nothing here",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: vec![("x-amz-expected-bucket-owner", NOT_THE_OWNER)],
            payload: a_day.clone(),
            status: 403,
            code: "AccessDenied",
            message: Some("Access Denied"),
        },
        RefusalRow {
            case: "no RestoreRequest container at all",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: String::new(),
            status: 400,
            code: "MalformedXML",
            message: Some(MALFORMED_XML_MESSAGE),
        },
        RefusalRow {
            case: "a container that names no lifetime",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: restore_payload(None, TierNamedIn::Request("Standard")),
            status: 400,
            code: "MalformedXML",
            message: Some(MALFORMED_XML_MESSAGE),
        },
        RefusalRow {
            case: "a lifetime of no days",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: restore_payload(Some(0), TierNamedIn::Request("Standard")),
            status: 400,
            code: "InvalidArgument",
            // S3's own sentence for a lifetime below one day: fixed text that states the
            // limit and names neither the value nor the member.
            message: Some("restoration days should be at least 1"),
        },
        RefusalRow {
            case: "a lifetime of negative days",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: restore_payload(Some(-5), TierNamedIn::Request("Standard")),
            status: 400,
            code: "InvalidArgument",
            message: Some("restoration days should be at least 1"),
        },
        RefusalRow {
            // The largest `Days` the member can carry: its expiry falls on no date this
            // emulator can report, and a restore nothing could report is refused rather
            // than recorded.
            case: "a lifetime no expiry date can be reported for",
            bucket: "restore-refusals",
            key: "archived",
            query: String::new(),
            headers: Vec::new(),
            payload: restore_payload(Some(i32::MAX), TierNamedIn::Request("Standard")),
            status: 400,
            code: "InvalidArgument",
            message: Some(
                "Days must be a number of days a temporary copy can live for, got 2147483647",
            ),
        },
    ];

    for row in rows {
        let refused = restore_object(
            &router,
            row.bucket,
            row.key,
            &row.query,
            &row.headers,
            &row.payload,
        )
        .await;
        let case = row.case;
        assert_eq!(
            refused.status, row.status,
            "case {case:?}: expected HTTP {}, got {} with body {:?}",
            row.status,
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            row.code,
            "case {case:?}: expected the code {:?}, got {:?}",
            row.code,
            refused.error_code()
        );
        if let Some(message) = row.message {
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}: expected the message {message:?}, got {:?}",
                refused.error_message()
            );
        }
    }

    // None of those refusals recorded anything: the archived object still reports no
    // restore state.
    let read = head_object(&router, "restore-refusals", "archived", "").await;
    assert_eq!(
        read.header("x-amz-restore"),
        None,
        "a refused restore must record nothing, got {:?}",
        read.header("x-amz-restore")
    );

    // A select restore needs a query engine and a second write this emulator performs
    // nothing of, so it is refused as unwritten rather than answered as a plain restore.
    let select = restore_object(
        &router,
        "restore-refusals",
        "archived",
        "",
        &[],
        "<RestoreRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
         <Type>SELECT</Type></RestoreRequest>",
    )
    .await;
    assert_eq!(
        select.status, 501,
        "case a select restore: expected HTTP 501, got {} with body {:?}",
        select.status,
        text(&select)
    );
    assert_eq!(
        select.error_code(),
        "NotImplemented",
        "case a select restore: expected NotImplemented, got {:?}",
        select.error_code()
    );
}

/// One row of the second-request table: the case, the tier the restore already running was
/// asked at, the tier the second request names, and what that second request must answer.
///
/// `Some(status)` is a request S3 SERVES — the documented tier upgrade — and `None` one it
/// refuses with `RestoreAlreadyInProgress`.
type UpgradeRow = (&'static str, &'static str, &'static str, Option<u16>);

/// A second restore request over a retrieval that is still running is an UPGRADE when it
/// names a strictly faster tier, and the same request again when it does not.
///
/// "You can upgrade the speed of your restore while it is in progress" by issuing another
/// restore request naming a faster tier (Amazon S3 User Guide, *Upgrading the speed of an
/// in-progress restore*), and the speeds are Expedited, then Standard, then Bulk (the same
/// guide's *Archive retrieval options*: 1–5 minutes, 3–5 hours, 5–12 hours). An upgrade is
/// accepted with the `202 Accepted` a retrieval that is not previously restored answers,
/// leaves the copy unavailable and the object in its archive class; every other second
/// request asks for the retrieval already under way, which is what
/// `RestoreAlreadyInProgress` reports.
///
/// The rows differ along the pair of tiers: every ordered pair of the three the GLACIER class
/// offers, so each of the three is tried as the running tier and as the requested one.
fn upgrade_rows() -> Vec<UpgradeRow> {
    vec![
        ("bulk upgraded to standard", "Bulk", "Standard", Some(202)),
        ("bulk upgraded to expedited", "Bulk", "Expedited", Some(202)),
        (
            "standard upgraded to expedited",
            "Standard",
            "Expedited",
            Some(202),
        ),
        ("bulk asked for again", "Bulk", "Bulk", None),
        ("standard asked for again", "Standard", "Standard", None),
        ("expedited asked for again", "Expedited", "Expedited", None),
        ("standard downgraded to bulk", "Standard", "Bulk", None),
        ("expedited downgraded to standard", "Expedited", "Standard", None),
        ("expedited downgraded to bulk", "Expedited", "Bulk", None),
    ]
}

#[tokio::test]
async fn a_second_restore_is_served_only_for_a_faster_tier() {
    let router = router();
    bucket(&router, "restore-upgrades").await;

    for (case, running, asked, served) in upgrade_rows() {
        // One key per row, so each row starts from a version with no retrieval of its own.
        let key = format!("{}-to-{}", running.to_ascii_lowercase(), asked.to_ascii_lowercase());
        object_in(&router, "restore-upgrades", &key, "GLACIER").await;

        let initiated = restore_object(
            &router,
            "restore-upgrades",
            &key,
            "",
            &[],
            &restore_payload(Some(1), TierNamedIn::Request(running)),
        )
        .await;
        assert_eq!(
            initiated.status, 202,
            "case {case:?}: the first restore initiates the retrieval and is accepted with \
             202, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );

        // The second request, while the first retrieval is still running: it names no
        // lifetime, because an upgrade changes how fast the copy arrives rather than how
        // long it lives.
        let second = restore_object(
            &router,
            "restore-upgrades",
            &key,
            "",
            &[],
            &restore_payload(None, TierNamedIn::Request(asked)),
        )
        .await;
        match served {
            Some(status) => assert_eq!(
                second.status, status,
                "case {case:?}: a faster tier upgrades the retrieval and is accepted with \
                 {status}, got {} with body {:?}",
                second.status,
                text(&second)
            ),
            None => {
                assert_eq!(
                    second.status, 409,
                    "case {case:?}: the retrieval already running is reported with 409, got \
                     {} with body {:?}",
                    second.status,
                    text(&second)
                );
                assert_eq!(
                    second.error_code(),
                    "RestoreAlreadyInProgress",
                    "case {case:?}: expected the RestoreAlreadyInProgress code, got {:?}",
                    second.error_code()
                );
                assert_eq!(
                    second.error_message(),
                    RESTORE_ALREADY_IN_PROGRESS_MESSAGE,
                    "case {case:?}: expected the message {RESTORE_ALREADY_IN_PROGRESS_MESSAGE:?}, \
                     got {:?}",
                    second.error_message()
                );
            }
        }

        // Either way the retrieval is still running and the object is still archived: the
        // copy is not there yet, so a reader reports an ongoing request and the class the
        // object is stored in.
        let read = head_object(&router, "restore-upgrades", &key, "").await;
        assert_eq!(
            read.header("x-amz-restore").as_deref(),
            Some("ongoing-request=\"true\""),
            "case {case:?}: the retrieval must still be reported as ongoing, got {:?}",
            read.header("x-amz-restore")
        );
        assert_eq!(
            read.header("x-amz-storage-class").as_deref(),
            Some("GLACIER"),
            "case {case:?}: the object must keep its archive storage class, got {:?}",
            read.header("x-amz-storage-class")
        );
    }
}

#[tokio::test]
async fn an_upgraded_retrieval_is_not_due_later_than_the_one_it_upgrades() {
    let router = router();
    bucket(&router, "restore-upgrade-timing").await;
    object_in(&router, "restore-upgrade-timing", "archived", "GLACIER").await;

    // A faster tier is asked for to get the copy SOONER, so the upgrade cannot push the
    // instant the copy falls due any further out than the retrieval it replaces: the copy is
    // there one span after the FIRST request, not one span after the second.
    let initiated = restore_object(
        &router,
        "restore-upgrade-timing",
        "archived",
        "",
        &[],
        &restore_payload(Some(1), TierNamedIn::Request("Bulk")),
    )
    .await;
    assert_eq!(
        initiated.status, 202,
        "the first restore must be accepted with 202, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let upgraded = restore_object(
        &router,
        "restore-upgrade-timing",
        "archived",
        "",
        &[],
        &restore_payload(None, TierNamedIn::Request("Standard")),
    )
    .await;
    assert_eq!(
        upgraded.status, 202,
        "the upgrade must be accepted with 202, got {} with body {:?}",
        upgraded.status,
        text(&upgraded)
    );

    wait_for_the_copy();
    let read = head_object(&router, "restore-upgrade-timing", "archived", "").await;
    let reported = read
        .header("x-amz-restore")
        .expect("a restored version must report its restore state");
    assert!(
        reported.contains("ongoing-request=\"false\""),
        "one span after the FIRST request the copy must be available, got {reported:?}"
    );
    // And the lifetime is the one the restore it upgrades was stated with: an upgrade changes
    // how fast the copy arrives, not how long it lives.
    let expiry = expiry_seconds(&reported);
    let a_day_out = now_seconds() + SECONDS_PER_DAY;
    assert!(
        expiry >= a_day_out && expiry < a_day_out + SECONDS_PER_DAY,
        "the copy must live for the one day the first request asked for (an expiry in \
         [{a_day_out}, {})), got {expiry}",
        a_day_out + SECONDS_PER_DAY
    );
}
