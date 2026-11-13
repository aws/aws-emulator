// SPDX-License-Identifier: CC0-1.0

//! What a PARTIAL read of an object answers, and when a read is barred by the storage class
//! the object's bytes are in.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, body bytes) rather than any handler's internals.
//!
//! The rules under test, each over a SET of inputs rather than one request:
//!
//! 1. A read that names a span of the object answers that span: HTTP 206 with
//!    `Content-Range: bytes <first>-<last>/<total>` and a `Content-Length` of the span, where
//!    a `partNumber` names the span of that part of the object (`GetObjectRequest$PartNumber`
//!    — "effectively performs a 'ranged' GET request for the part specified") and a `Range`
//!    names the byte span RFC 9110's `bytes` unit defines: `first-last`, `first-` to the end,
//!    and `-suffix` for the last bytes, with a `last` past the end clamped to it.
//! 2. A span the object cannot satisfy is refused, with the code and the published sentence
//!    for which kind of span it was: `InvalidPartNumber` (416) for a part number the object
//!    has no part for, `InvalidRange` (416) for a `Range` whose first byte is past the end.
//! 3. A `Range` the `bytes` unit does not define is IGNORED and the whole object served
//!    (RFC 9110: an origin server "MUST ignore a Range header field that contains a range
//!    unit it does not understand"), which is also what "Amazon S3 doesn't support
//!    retrieving multiple ranges of data per `GET` request" (`GetObjectRequest$Range`) leaves
//!    for a multi-range header.
//! 4. A read of an object whose bytes are in an ARCHIVED storage class is refused with
//!    `InvalidObjectState` (403) and the sentence S3 words for it, until a restore has made a
//!    temporary copy available — "before you can retrieve the object you must first restore a
//!    copy using RestoreObject. Otherwise, this operation returns an InvalidObjectState
//!    error" (`GetObject`'s documentation of that error). The class the object is stored in
//!    decides it, however the object was written.

use std::time::Duration;

use service_com_amazonaws_s3_amazons3::service::object_write::{
    GENERAL_PURPOSE_STORAGE_CLASSES, SERVICE_PLACED_STORAGE_CLASSES,
};
use service_com_amazonaws_s3_amazons3::state::{ARCHIVED_STORAGE_CLASSES, RESTORE_COMPLETION_DELAY};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The `<Message>` S3 sends with `InvalidPartNumber` — the FIXED text that code's entry in
/// Amazon S3's list of error codes publishes, spelled out here rather than read from the
/// crate under test so a wording regression fails this suite.
const INVALID_PART_NUMBER_MESSAGE: &str = "The requested partnumber is not satisfiable";

/// The `<Message>` S3 sends with `InvalidRange`, from the same published list.
const INVALID_RANGE_MESSAGE: &str = "The requested range is not satisfiable";

/// The `<Message>` S3 sends with `InvalidObjectState` when the storage class the object's
/// bytes are in bars the read.
const INVALID_OBJECT_STATE_READ_MESSAGE: &str =
    "The operation is not valid for the object's storage class";

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The highest part number a part may carry: "This value is a positive integer between 1 and
/// 10,000" (`ObjectPart$PartNumber`).
const MAX_PART_NUMBER: i32 = 10_000;

/// How long a row waits for a retrieval to fall due: the span the store records plus slack
/// for a loaded machine.
///
/// A blocking sleep rather than `tokio::time::sleep(..).await`: the workspace's tokio
/// carries only the `macros`/`rt` features, so its timer driver is not built.
fn wait_for_the_copy() {
    std::thread::sleep(RESTORE_COMPLETION_DELAY + Duration::from_millis(250));
}

/// `length` bytes from `seed`, so a payload is byte-varied without anything random: a
/// 32-bit LCG (the "ANSI C" multiplier and increment), taking the high byte of each state.
///
/// `pub(crate)`, with [`multipart_object`] below it: the three suites that need a multipart
/// object ([`super::zz_probe`], [`super::presigned_object_reads`] and
/// [`super::object_conditional_writes`]) drive one through these rather than keeping a copy
/// of the loop each.
pub(crate) fn seeded_payload(seed: u32, length: usize) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 24) as u8
        })
        .collect()
}

/// [`seeded_payload`] from this suite's own seed.
fn sized_payload(length: usize) -> Vec<u8> {
    seeded_payload(0x5f37_1337, length)
}

/// Initiate an upload at `key`, upload `parts` in order, and complete it — `initiation` the
/// headers the initiating request carries (where the finished object's storage class and
/// checksum algorithm are settled) and `completion` the headers the completion carries
/// (where its conditional members travel). Answers what the COMPLETION answered, so a case
/// that expects a refused completion reads it from here.
///
/// Every step but the completion is asserted to have succeeded: a case is about the
/// completion, so an initiation or a part upload that failed is a broken fixture rather than
/// a result to judge.
pub(crate) async fn multipart_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    parts: &[Vec<u8>],
    initiation: &[(&str, &str)],
    completion: &[(&str, &str)],
) -> Answer {
    let initiated = send(
        router,
        "POST",
        &format!("/{name}/{key}?uploads"),
        initiation,
        "",
    )
    .await;
    assert_eq!(
        initiated.status,
        200,
        "the initiation of {name}/{key} must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    let mut payload = String::from("<CompleteMultipartUpload>");
    for (index, part) in parts.iter().enumerate() {
        let part_number = index + 1;
        let uploaded = send_bytes(
            router,
            "PUT",
            &format!("/{name}/{key}?partNumber={part_number}&uploadId={upload_id}"),
            &[],
            part,
        )
        .await;
        assert_eq!(
            uploaded.status,
            200,
            "uploading part {part_number} of {name}/{key} must answer HTTP 200, got {} with \
             body {:?}",
            uploaded.status,
            text(&uploaded)
        );
        let etag = uploaded
            .header("etag")
            .expect("a part upload must report an ETag header");
        payload.push_str(&format!(
            "<Part><PartNumber>{part_number}</PartNumber><ETag>{etag}</ETag></Part>"
        ));
    }
    payload.push_str("</CompleteMultipartUpload>");
    send(
        router,
        "POST",
        &format!("/{name}/{key}?uploadId={upload_id}"),
        completion,
        &payload,
    )
    .await
}

/// [`multipart_object`] for a case that expects the completion to succeed.
pub(crate) async fn completed_multipart_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    parts: &[Vec<u8>],
    initiation: &[(&str, &str)],
) {
    let completed = multipart_object(router, name, key, parts, initiation, &[]).await;
    assert_eq!(
        completed.status,
        200,
        "the completion of {name}/{key} must answer HTTP 200, got {} with body {:?}",
        completed.status,
        text(&completed)
    );
}

/// One row of the unsatisfiable-span table: the case name, the query the read carries, the
/// headers it carries, and the wire code and sentence it must be refused with.
type Unsatisfiable = (
    &'static str,
    String,
    Vec<(&'static str, String)>,
    &'static str,
    &'static str,
);

/// What a row expects a span-naming read to answer.
struct Served {
    /// The case name, for the assertion messages.
    case: &'static str,
    /// The `Range` header, when the row names one.
    range: Option<&'static str>,
    /// The `partNumber` query parameter, when the row names one.
    part_number: Option<i32>,
    /// The span of the object the answer must carry, as `(first, last)` inclusive, or `None`
    /// for a row whose read is of the WHOLE object (an ignored header).
    span: Option<(usize, usize)>,
}

#[tokio::test]
async fn a_read_that_names_a_span_answers_that_span() {
    let router = router();
    bucket(&router, "span-bucket").await;

    // A multipart object, so a part number names a span that is not the whole object, and a
    // one-shot one, so the same rule is pinned where the object has no parts.
    let parts = vec![sized_payload(MIN_MULTIPART_PART_BYTES), sized_payload(777)];
    let object: Vec<u8> = parts.concat();
    completed_multipart_object(&router, "span-bucket", "assembled.bin", &parts, &[]).await;
    let whole = sized_payload(1_000);
    let stored = put_object(&router, "span-bucket", "written.bin", &whole, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "the one-shot write must succeed, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    let total = object.len();
    let first_part = parts[0].len();
    // The rows differ along what the rule generalises over: which FORM of span the read
    // names (a part number, a closed byte range, an open-ended one, a suffix one, one whose
    // end is past the object), where in the object the span falls (the first byte, the
    // middle, the last byte, one byte), and whether the header names a span at all.
    let rows: Vec<Served> = vec![
        Served {
            case: "the first part",
            range: None,
            part_number: Some(1),
            span: Some((0, first_part - 1)),
        },
        Served {
            case: "the last part",
            range: None,
            part_number: Some(2),
            span: Some((first_part, total - 1)),
        },
        Served {
            // Both at once: the part number IS the range it asks for ("effectively performs
            // a 'ranged' GET request for the part specified",
            // `GetObjectRequest$PartNumber`), so it decides the span and the `Range` beside
            // it cannot name a second one the answer also is.
            case: "a part number beside a byte range",
            range: Some("bytes=0-9"),
            part_number: Some(2),
            span: Some((first_part, total - 1)),
        },
        Served {
            case: "a closed range inside the first part",
            range: Some("bytes=0-9"),
            part_number: None,
            span: Some((0, 9)),
        },
        Served {
            case: "a closed range of one byte in the middle",
            range: Some("bytes=1024-1024"),
            part_number: None,
            span: Some((1_024, 1_024)),
        },
        Served {
            case: "an open-ended range",
            range: Some("bytes=5242870-"),
            part_number: None,
            span: Some((5_242_870, total - 1)),
        },
        Served {
            case: "a suffix range",
            range: Some("bytes=-100"),
            part_number: None,
            span: Some((total - 100, total - 1)),
        },
        Served {
            case: "a suffix range longer than the object",
            range: Some("bytes=-99999999"),
            part_number: None,
            span: Some((0, total - 1)),
        },
        Served {
            case: "a closed range whose end is past the object",
            range: Some("bytes=5242870-99999999"),
            part_number: None,
            span: Some((5_242_870, total - 1)),
        },
        Served {
            case: "the whole object as a range",
            range: Some("bytes=0-"),
            part_number: None,
            span: Some((0, total - 1)),
        },
        // Headers the `bytes` unit does not define, which are IGNORED: the whole object is
        // served with 200 and no `Content-Range` at all.
        Served {
            case: "a range unit this service does not understand",
            range: Some("items=0-9"),
            part_number: None,
            span: None,
        },
        Served {
            case: "two ranges at once",
            range: Some("bytes=0-9,20-29"),
            part_number: None,
            span: None,
        },
        Served {
            case: "a range that is not a span at all",
            range: Some("bytes=not-a-range"),
            part_number: None,
            span: None,
        },
        Served {
            case: "a range whose last byte is before its first",
            range: Some("bytes=20-10"),
            part_number: None,
            span: None,
        },
    ];

    for row in &rows {
        let query = row
            .part_number
            .map(|number| format!("?partNumber={number}"))
            .unwrap_or_default();
        let headers: Vec<(&str, &str)> = row
            .range
            .map(|range| vec![("range", range)])
            .unwrap_or_default();
        let answer = get_object(&router, "span-bucket", "assembled.bin", &query, &headers).await;
        let case = row.case;
        match row.span {
            Some((first, last)) => {
                assert_eq!(
                    answer.status, 206,
                    "case {case:?}: a span-naming read must answer the partial-content \
                     status, got {} with body {:?}",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.header("content-range"),
                    Some(format!("bytes {first}-{last}/{total}")),
                    "case {case:?}: expected the span of the whole object, got {:?}",
                    answer.header("content-range")
                );
                assert_eq!(
                    answer.header("content-length"),
                    Some((last - first + 1).to_string()),
                    "case {case:?}: expected the span's own length, got {:?}",
                    answer.header("content-length")
                );
                assert_eq!(
                    answer.body.as_ref(),
                    &object[first..=last],
                    "case {case:?}: expected the span's bytes, got {} of them",
                    answer.body.len()
                );
            }
            None => {
                assert_eq!(
                    answer.status, 200,
                    "case {case:?}: an ignored range must serve the whole object, got {} with \
                     body {:?}",
                    answer.status,
                    text(&answer)
                );
                assert_eq!(
                    answer.header("content-range"),
                    None,
                    "case {case:?}: a whole-object read reports no Content-Range, got {:?}",
                    answer.header("content-range")
                );
                assert_eq!(
                    answer.body.len(),
                    total,
                    "case {case:?}: expected every byte of the object, got {}",
                    answer.body.len()
                );
            }
        }
    }

    // The part count is reported only to a part-scoped read of an object that WAS uploaded
    // as a multipart upload ("this value is only returned if you specify partNumber in your
    // request and the object was uploaded as a multipart upload").
    for (case, key, query, expected) in [
        (
            "a part-scoped read of a multipart object",
            "assembled.bin",
            "?partNumber=1",
            Some("2"),
        ),
        (
            "a ranged read of a multipart object",
            "assembled.bin",
            "",
            None,
        ),
        (
            "a part-scoped read of a one-shot write",
            "written.bin",
            "?partNumber=1",
            None,
        ),
    ] {
        let answer = get_object(&router, "span-bucket", key, query, &[]).await;
        assert_eq!(
            answer.header("x-amz-mp-parts-count").as_deref(),
            expected,
            "case {case:?}: expected the part count {expected:?}, got {:?}",
            answer.header("x-amz-mp-parts-count")
        );
    }

    // A one-shot write has one part — itself — so part 1 is the whole object.
    let only_part = get_object(&router, "span-bucket", "written.bin", "?partNumber=1", &[]).await;
    assert_eq!(
        only_part.status, 206,
        "case \"part 1 of a one-shot write\": expected HTTP 206, got {} with body {:?}",
        only_part.status,
        text(&only_part)
    );
    assert_eq!(
        only_part.body.as_ref(),
        whole.as_slice(),
        "case \"part 1 of a one-shot write\": expected the whole object, got {} bytes",
        only_part.body.len()
    );
}

#[tokio::test]
async fn a_span_the_object_cannot_satisfy_is_refused_with_its_published_message() {
    let router = router();
    bucket(&router, "unsatisfiable-bucket").await;

    let parts = vec![sized_payload(MIN_MULTIPART_PART_BYTES), sized_payload(64)];
    let total: usize = parts.iter().map(Vec::len).sum();
    completed_multipart_object(&router, "unsatisfiable-bucket", "assembled.bin", &parts, &[])
        .await;

    // The rows differ along which kind of span could not be served, and how far outside the
    // object it fell: the part after the last one, the highest part number a part may
    // carry, the first byte past the end, far past the end, and a suffix of no bytes at all.
    let rows: [Unsatisfiable; 6] = [
        (
            "the part after the last one",
            String::from("?partNumber=3"),
            Vec::new(),
            "InvalidPartNumber",
            INVALID_PART_NUMBER_MESSAGE,
        ),
        (
            "the highest part number a part may carry",
            format!("?partNumber={MAX_PART_NUMBER}"),
            Vec::new(),
            "InvalidPartNumber",
            INVALID_PART_NUMBER_MESSAGE,
        ),
        (
            "the first byte past the end of the object",
            String::new(),
            vec![("range", format!("bytes={total}-{total}"))],
            "InvalidRange",
            INVALID_RANGE_MESSAGE,
        ),
        (
            "an open-ended range starting past the end",
            String::new(),
            vec![("range", format!("bytes={}-", total + 1_000))],
            "InvalidRange",
            INVALID_RANGE_MESSAGE,
        ),
        (
            "a suffix of no bytes",
            String::new(),
            vec![("range", String::from("bytes=-0"))],
            "InvalidRange",
            INVALID_RANGE_MESSAGE,
        ),
        (
            "a closed range starting past the end",
            String::new(),
            vec![("range", format!("bytes={}-{}", total, total + 10))],
            "InvalidRange",
            INVALID_RANGE_MESSAGE,
        ),
    ];

    for (case, query, headers, code, message) in rows {
        let borrowed: Vec<(&str, &str)> = headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect();
        let refused = get_object(
            &router,
            "unsatisfiable-bucket",
            "assembled.bin",
            &query,
            &borrowed,
        )
        .await;
        assert_eq!(
            refused.status, 416,
            "case {case:?}: expected HTTP 416, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the {code} code, got {:?}",
            refused.error_code()
        );
        assert_eq!(
            refused.error_message(),
            message,
            "case {case:?}: expected the published sentence, got {:?}",
            refused.error_message()
        );
    }

    // The same refusals from a HEAD of the same object, which "has the same options as a GET
    // operation on an object".
    let refused_head = send(
        &router,
        "HEAD",
        "/unsatisfiable-bucket/assembled.bin?partNumber=3",
        &[],
        "",
    )
    .await;
    assert_eq!(
        refused_head.status, 416,
        "case \"a part-scoped HEAD past the last part\": expected HTTP 416, got {}",
        refused_head.status
    );
}

#[tokio::test]
async fn a_read_of_an_archived_object_is_refused_until_a_copy_is_restored() {
    let router = router();
    bucket(&router, "archived-bucket").await;

    // Every storage class a general purpose bucket can host and a client write may NAME,
    // written both ways an object comes to be in one: a one-shot write naming
    // `x-amz-storage-class`, and a multipart upload whose INITIATION named it (which is where
    // the finished object's class is settled). The two archived classes are refused and every
    // other class is served, so what decides the refusal is the class rather than the write.
    // A class only another AWS service places objects in is left out: no write may name one,
    // so no read of this suite's kind can reach it.
    assert!(
        GENERAL_PURPOSE_STORAGE_CLASSES.len() > ARCHIVED_STORAGE_CLASSES.len(),
        "a general purpose bucket must host more storage classes than the archived ones, got \
         {GENERAL_PURPOSE_STORAGE_CLASSES:?}"
    );
    let written_classes: Vec<&str> = GENERAL_PURPOSE_STORAGE_CLASSES
        .iter()
        .copied()
        .filter(|class| !SERVICE_PLACED_STORAGE_CLASSES.contains(class))
        .collect();
    for storage_class in &written_classes {
        let archived = ARCHIVED_STORAGE_CLASSES.contains(storage_class);
        let key = format!("{}.bin", storage_class.to_ascii_lowercase());
        let payload = sized_payload(2_048);
        let written = put_object(
            &router,
            "archived-bucket",
            &key,
            &payload,
            &[("x-amz-storage-class", storage_class)],
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {storage_class:?}: the write must succeed, got {} with body {:?}",
            written.status,
            text(&written)
        );

        let read = get_object(&router, "archived-bucket", &key, "", &[]).await;
        if archived {
            assert_eq!(
                read.status, 403,
                "case {storage_class:?}: a read of an archived object must be refused, got {} \
                 with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.error_code(),
                "InvalidObjectState",
                "case {storage_class:?}: expected the InvalidObjectState code, got {:?}",
                read.error_code()
            );
            assert_eq!(
                read.error_message(),
                INVALID_OBJECT_STATE_READ_MESSAGE,
                "case {storage_class:?}: expected the sentence S3 words for this state, got \
                 {:?}",
                read.error_message()
            );
            // A HEAD of the same object is NOT refused: it reports the object's metadata and
            // its restore state, which is how a client learns it must restore first.
            let head = send(
                &router,
                "HEAD",
                &format!("/archived-bucket/{key}"),
                &[],
                "",
            )
            .await;
            assert_eq!(
                head.status, 200,
                "case {storage_class:?}: a HEAD of an archived object must still answer 200, \
                 got {}",
                head.status
            );
            assert_eq!(
                head.header("x-amz-storage-class").as_deref(),
                Some(*storage_class),
                "case {storage_class:?}: the HEAD must report the class the object is in, got \
                 {:?}",
                head.header("x-amz-storage-class")
            );
        } else {
            assert_eq!(
                read.status, 200,
                "case {storage_class:?}: a read of an object that is not archived must serve \
                 it, got {} with body {:?}",
                read.status,
                text(&read)
            );
            assert_eq!(
                read.body.as_ref(),
                payload.as_slice(),
                "case {storage_class:?}: expected the stored bytes, got {} of them",
                read.body.len()
            );
        }
    }

    // The same refusal for an object whose class was settled by the multipart INITIATION,
    // and the same service once a restore has made a copy available: the bar is the state of
    // the bytes, not the shape of the write.
    for storage_class in ARCHIVED_STORAGE_CLASSES {
        let key = format!("mpu-{}.bin", storage_class.to_ascii_lowercase());
        let parts = vec![sized_payload(MIN_MULTIPART_PART_BYTES), sized_payload(32)];
        let assembled: Vec<u8> = parts.concat();
        // The published bucket naming rules admit no underscore, so the class names the
        // bucket with hyphens in place of them.
        let bucket_name = format!(
            "archived-mpu-{}",
            storage_class.to_ascii_lowercase().replace('_', "-")
        );
        bucket(&router, &bucket_name).await;
        completed_multipart_object(
            &router,
            &bucket_name,
            &key,
            &parts,
            &[("x-amz-storage-class", storage_class)],
        )
        .await;

        let refused = get_object(&router, &bucket_name, &key, "", &[]).await;
        assert_eq!(
            refused.status, 403,
            "case {storage_class:?}: a read of a completed upload in an archived class must \
             be refused, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            "InvalidObjectState",
            "case {storage_class:?}: expected the InvalidObjectState code, got {:?}",
            refused.error_code()
        );

        // A part-scoped read of the same object is barred by the same state, so the
        // discovery probe cannot read bytes a restore has not made available.
        let part = get_object(&router, &bucket_name, &key, "?partNumber=1", &[]).await;
        assert_eq!(
            part.status, 403,
            "case {storage_class:?}: a part-scoped read must be refused too, got {}",
            part.status
        );

        // Once a restore has run and its temporary copy has fallen due, the same read is
        // served — and serves the bytes the upload assembled.
        let restored = send(
            &router,
            "POST",
            &format!("/{bucket_name}/{key}?restore"),
            &[],
            "<RestoreRequest xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">\
             <Days>1</Days></RestoreRequest>",
        )
        .await;
        assert!(
            restored.status == 202 || restored.status == 200,
            "case {storage_class:?}: the restore must be accepted, got {} with body {:?}",
            restored.status,
            text(&restored)
        );
        wait_for_the_copy();
        let served = get_object(&router, &bucket_name, &key, "", &[]).await;
        assert_eq!(
            served.status, 200,
            "case {storage_class:?}: a read of a restored copy must be served, got {} with \
             body {:?}",
            served.status,
            text(&served)
        );
        assert_eq!(
            served.body.as_ref(),
            assembled.as_slice(),
            "case {storage_class:?}: expected the assembled bytes, got {} of them",
            served.body.len()
        );
    }
}
