// SPDX-License-Identifier: CC0-1.0

//! `UploadPartCopy`: ONE part of an in-progress multipart upload recorded from the bytes of
//! an object this emulator already holds.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle, object and multipart suites in this crate already own), so what is
//! pinned is the wire behaviour — status, bound headers, payload elements, the parts a later
//! `ListParts` reports and the bytes the completed object serves.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. "Uploads a part by copying data from an existing object as data source" — the part the
//!    copy records holds the SOURCE's bytes, is addressable by its part number exactly as an
//!    `UploadPart` body is, and carries the entity tag of those bytes, which is what a later
//!    `CompleteMultipartUpload` names it by and what `ListParts` reports for it. The source
//!    is read the way `CopyObject` reads its own `x-amz-copy-source`: a bucket and key, with
//!    an optional `?versionId=` naming one stored version.
//! 2. "The range of bytes to copy from the source object. The range value must use the form
//!    bytes=first-last, where the first and last are the zero-based byte offsets to copy"
//!    (`UploadPartCopyRequest$CopySourceRange`), and "you can copy a range only if the source
//!    object is greater than 5 MB" — so a ranged copy of a large enough source records
//!    exactly those bytes, and one of a smaller source is the `InvalidRequest` this
//!    operation's own special errors publish ("the specified copy source is not supported as
//!    a byte-range copy source").
//! 3. A copy this emulator cannot carry out as asked records NOTHING: the upload holds
//!    exactly the parts it held before, and the caller is told with the code and status S3
//!    answers — an upload id the store never minted (or one named under another key) with 404
//!    `NoSuchUpload`, a source bucket or key it does not hold with 404 `NoSuchBucket` /
//!    `NoSuchKey`, a part number outside 1..=10,000 with 400 `InvalidArgument`, a copy-source
//!    precondition that does not hold with 412, and `x-amz-expected-bucket-owner` naming
//!    another account with 403.
//! 4. The parts a copy recorded are the parts a completion assembles: the object at the
//!    upload's key afterwards holds the concatenated source bytes, reads back through
//!    `GetObject`, and is reported by a batch delete of that key.

use service_com_amazonaws_s3_amazons3::state::{
    ACCESS_DENIED_CODE, INVALID_ARGUMENT_CODE, INVALID_REQUEST_CODE, MIN_MULTIPART_PART_BYTES,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{
    bucket, expected_etag, get_object, put_object, text,
};
use super::object_partial_reads::seeded_payload;

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(router: &MultiServiceRouter, name: &str, key: &str) -> String {
    let initiated = send(router, "POST", &format!("/{name}/{key}?uploads"), &[], "").await;
    assert_eq!(
        initiated.status,
        200,
        "an initiation must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId")
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>&x-id=UploadPartCopy` carrying
/// `x-amz-copy-source` — `UploadPartCopy`.
///
/// `pub(crate)`: the suite that asserts what a copied part REPORTS
/// ([`super::multipart_part_copy_reported_members`]) drives its copies through this one
/// helper rather than spelling the same request a second way.
pub(crate) async fn upload_part_copy(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(
        router,
        "PUT",
        &format!(
            "/{name}/{key}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPartCopy"
        ),
        &all,
        b"",
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`. The `x-id` literal is
/// the one the model binds that operation under, which is what distinguishes it from a read
/// of the object at the same address.
async fn list_parts(router: &MultiServiceRouter, name: &str, key: &str, upload_id: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}/{key}?uploadId={upload_id}&x-id=ListParts"),
        &[],
        "",
    )
    .await
}

/// Every `<Part>` a read of the upload reports, as `(PartNumber, ETag, Size)` in the order
/// the response carries them.
fn reported_parts(answer: &Answer) -> Vec<(i32, String, i64)> {
    let body = text(answer);
    body.split("<Part>")
        .skip(1)
        .filter_map(|chunk| chunk.split("</Part>").next().map(str::to_string))
        .map(|chunk| {
            let element = |name: &str| {
                chunk
                    .split_once(&format!("<{name}>"))
                    .and_then(|(_, rest)| rest.split_once(&format!("</{name}>")))
                    .map(|(value, _)| value.to_string())
            };
            (
                element("PartNumber")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
                // The XML escaping the serializer applies to the quotes an entity tag is
                // reported inside, undone the way the harness's own element reader undoes it.
                super::bucket_tagging::unescaped(&element("ETag").unwrap_or_default()),
                element("Size")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
            )
        })
        .collect()
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`, with the parts list as
/// the payload.
async fn complete(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
) -> Answer {
    let elements: String = parts
        .iter()
        .map(|(number, etag)| {
            format!("<Part><PartNumber>{number}</PartNumber><ETag>{etag}</ETag></Part>")
        })
        .collect();
    send(
        router,
        "POST",
        &format!("/{name}/{key}?uploadId={upload_id}"),
        &[],
        &format!("<CompleteMultipartUpload>{elements}</CompleteMultipartUpload>"),
    )
    .await
}

/// `PUT /<bucket>?versioning` — `PutBucketVersioning`, enabling versioning so a source key
/// holds more than one version for a copy to address by id.
async fn enable_versioning(router: &MultiServiceRouter, name: &str) {
    let enabled = send(
        router,
        "PUT",
        &format!("/{name}?versioning"),
        &[],
        "<VersioningConfiguration><Status>Enabled</Status></VersioningConfiguration>",
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "versioning must be enabled on {name:?}, got {}",
        enabled.status
    );
}

/// How a row names the source bytes the copy is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    /// The whole current object at the source key.
    Whole,
    /// The whole object of the source key's FIRST stored version, named by its id.
    FirstVersion,
    /// The inclusive byte range `(first, last)` of the large source object.
    Range(usize, usize),
}

/// One row of the recording table: the case name, which source bytes the copy names, the part
/// number it records them under, and whether the source key is the large one (the only source
/// a ranged copy is served from).
struct Row {
    case: &'static str,
    source: Source,
    part_number: i32,
    large: bool,
}

#[tokio::test]
async fn a_part_copy_records_the_source_bytes_it_names_as_that_part_of_the_upload() {
    let router = router();
    let origin = "part-copy-origin";
    let target = "part-copy-target";
    bucket(&router, origin).await;
    bucket(&router, target).await;
    enable_versioning(&router, origin).await;

    // Two sources: a small object whose bytes a whole-object copy takes, and one past the
    // published 5 MiB minimum, which is the only kind a RANGED copy may be taken from. The
    // small key is written twice, so its first version is addressable by id while a copy
    // naming no version takes the second.
    let small_key = "sources/small.bin";
    let large_key = "sources/large.bin";
    let first = seeded_payload(0x1234_5678, 64);
    let second = seeded_payload(0x8765_4321, 96);
    let large = seeded_payload(0x0f0f_0f0f, MIN_MULTIPART_PART_BYTES + 1);
    let first_write = put_object(&router, origin, small_key, &first, &[]).await;
    assert_eq!(
        first_write.status, 200,
        "the first fixture write must succeed, got {}",
        first_write.status
    );
    let first_version = first_write
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket reports a version id");
    let second_write = put_object(&router, origin, small_key, &second, &[]).await;
    assert_eq!(
        second_write.status, 200,
        "the second fixture write must succeed, got {}",
        second_write.status
    );
    let second_version = second_write
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket reports a version id");
    let large_write = put_object(&router, origin, large_key, &large, &[]).await;
    assert_eq!(
        large_write.status, 200,
        "the large fixture write must succeed, got {}",
        large_write.status
    );
    let large_version = large_write
        .header("x-amz-version-id")
        .expect("a write into a versioning-enabled bucket reports a version id");

    // The rows differ along what the rule generalises over: whether the source is the key's
    // current version or one named by id, whether the copy takes the whole object or a byte
    // range of it, where in the source that range falls and how long it is, and which part
    // number the bytes are recorded under.
    let rows: Vec<Row> = vec![
        Row {
            case: "the whole current version of a key",
            source: Source::Whole,
            part_number: 1,
            large: false,
        },
        Row {
            case: "a stored version named by id",
            source: Source::FirstVersion,
            part_number: 2,
            large: false,
        },
        Row {
            case: "the whole of a source past the minimum part size",
            source: Source::Whole,
            part_number: 3,
            large: true,
        },
        Row {
            case: "the first ten bytes of that source",
            source: Source::Range(0, 9),
            part_number: 4,
            large: true,
        },
        Row {
            case: "one byte in the middle of it",
            source: Source::Range(1_000, 1_000),
            part_number: 5,
            large: true,
        },
        Row {
            case: "its last byte",
            source: Source::Range(MIN_MULTIPART_PART_BYTES, MIN_MULTIPART_PART_BYTES),
            part_number: 10_000,
            large: true,
        },
    ];

    for row in &rows {
        let case = row.case;
        let key = format!("copies/{}.bin", row.part_number);
        let upload_id = initiate(&router, target, &key).await;
        let source_key = if row.large { large_key } else { small_key };
        let (copy_source, expected_bytes, expected_source_version) = match row.source {
            // A copy that names no version takes the key's CURRENT version, whose own id is
            // what the answer reports: the source bucket here keeps versions.
            Source::Whole => {
                let (bytes, version) = if row.large {
                    (large.clone(), large_version.clone())
                } else {
                    (second.clone(), second_version.clone())
                };
                (format!("{origin}/{source_key}"), bytes, Some(version))
            }
            Source::FirstVersion => (
                format!("{origin}/{source_key}?versionId={first_version}"),
                first.clone(),
                Some(first_version.clone()),
            ),
            Source::Range(from, to) => (
                format!("{origin}/{source_key}"),
                large[from..=to].to_vec(),
                Some(large_version.clone()),
            ),
        };
        let range = match row.source {
            Source::Range(from, to) => format!("bytes={from}-{to}"),
            _ => String::new(),
        };
        let headers: Vec<(&str, &str)> = if range.is_empty() {
            Vec::new()
        } else {
            vec![("x-amz-copy-source-range", range.as_str())]
        };

        let copied = upload_part_copy(
            &router,
            target,
            &key,
            &upload_id,
            row.part_number,
            &copy_source,
            &headers,
        )
        .await;
        assert_eq!(
            copied.status,
            200,
            "case {case:?}: expected HTTP 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        let etag = expected_etag(&expected_bytes);
        assert_eq!(
            copied.element("ETag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected the CopyPartResult to carry the tag of the copied \
             bytes, got {:?}",
            copied.element("ETag")
        );
        assert!(
            copied.element("LastModified").is_some(),
            "case {case:?}: expected the CopyPartResult to carry a LastModified, got body \
             {:?}",
            text(&copied)
        );
        assert_eq!(
            copied.header("x-amz-copy-source-version-id"),
            expected_source_version,
            "case {case:?}: expected the source version the copy was taken from, got {:?}",
            copied.header("x-amz-copy-source-version-id")
        );

        // The part the upload holds afterwards, read back through the reader of the upload:
        // its number, the tag the copy answered, and the size of the bytes it took.
        let listed = list_parts(&router, target, &key, &upload_id).await;
        assert_eq!(
            listed.status,
            200,
            "case {case:?}: the upload must be readable, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            reported_parts(&listed),
            vec![(
                row.part_number,
                etag.clone(),
                i64::try_from(expected_bytes.len()).unwrap_or_default()
            )],
            "case {case:?}: expected exactly the part the copy recorded, got body {:?}",
            text(&listed)
        );
    }
}

/// One row of the refusal table: the case name, the copy source, the part number, the extra
/// headers, and the status and error code the refusal must answer.
struct RefusalRow {
    case: &'static str,
    upload_key: Option<&'static str>,
    copy_source: String,
    part_number: i32,
    headers: Vec<(&'static str, &'static str)>,
    status: u16,
    code: &'static str,
}

#[tokio::test]
async fn a_part_copy_this_emulator_cannot_serve_records_nothing_and_says_why() {
    let router = router();
    let origin = "part-copy-refusal-origin";
    let target = "part-copy-refusal-target";
    bucket(&router, origin).await;
    bucket(&router, target).await;
    let source_key = "sources/small.bin";
    let payload = b"source-bytes";
    let stored = put_object(&router, origin, source_key, payload, &[]).await;
    assert_eq!(
        stored.status, 200,
        "the fixture write must succeed, got {}",
        stored.status
    );
    let large_key = "sources/large.bin";
    let large = seeded_payload(0x0bad_f00d, MIN_MULTIPART_PART_BYTES + 1);
    let stored_large = put_object(&router, origin, large_key, &large, &[]).await;
    assert_eq!(
        stored_large.status, 200,
        "the large fixture write must succeed, got {}",
        stored_large.status
    );

    let key = "copies/refused.bin";
    let upload_id = initiate(&router, target, key).await;
    // An upload of ANOTHER key, so a row can name this upload's id under a key it was not
    // initiated for — which addresses no upload at all.
    let other_key = "copies/other.bin";
    let _other_upload = initiate(&router, target, other_key).await;

    let rows: Vec<RefusalRow> = vec![
        RefusalRow {
            case: "a source bucket this emulator does not hold",
            upload_key: None,
            copy_source: "no-such-origin/sources/small.bin".to_string(),
            part_number: 1,
            headers: Vec::new(),
            status: 404,
            code: "NoSuchBucket",
        },
        RefusalRow {
            case: "a source key the bucket holds nothing at",
            upload_key: None,
            copy_source: format!("{origin}/sources/absent.bin"),
            part_number: 1,
            headers: Vec::new(),
            status: 404,
            code: "NoSuchKey",
        },
        RefusalRow {
            case: "the upload id named under another key",
            upload_key: Some("copies/other.bin"),
            copy_source: format!("{origin}/{source_key}"),
            part_number: 1,
            headers: Vec::new(),
            status: 404,
            code: "NoSuchUpload",
        },
        RefusalRow {
            case: "a part number below the first one",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 0,
            headers: Vec::new(),
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a part number past the last one",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 10_001,
            headers: Vec::new(),
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a copy source naming neither a bucket nor a key",
            upload_key: None,
            copy_source: "/?versionId=".to_string(),
            part_number: 1,
            headers: Vec::new(),
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a copy-source entity tag the source does not carry",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 1,
            headers: vec![(
                "x-amz-copy-source-if-match",
                "\"00000000000000000000000000000000\"",
            )],
            status: 412,
            code: "PreconditionFailed",
        },
        RefusalRow {
            case: "a range of a source too small to copy a range of",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 1,
            headers: vec![("x-amz-copy-source-range", "bytes=0-1")],
            status: 400,
            code: INVALID_REQUEST_CODE,
        },
        RefusalRow {
            case: "a range that is not of the published form",
            upload_key: None,
            copy_source: format!("{origin}/{large_key}"),
            part_number: 1,
            headers: vec![("x-amz-copy-source-range", "bytes=0")],
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a range whose last offset the source does not have",
            upload_key: None,
            copy_source: format!("{origin}/{large_key}"),
            part_number: 1,
            headers: vec![(
                "x-amz-copy-source-range",
                "bytes=0-99999999",
            )],
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a range whose first offset is past its last",
            upload_key: None,
            copy_source: format!("{origin}/{large_key}"),
            part_number: 1,
            headers: vec![("x-amz-copy-source-range", "bytes=9-8")],
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "a copy source naming an object through an access point ARN",
            upload_key: None,
            copy_source: "arn:aws:s3:us-east-1:000000000000:accesspoint/ap/object/source.bin"
                .to_string(),
            part_number: 1,
            headers: Vec::new(),
            status: 501,
            code: "NotImplemented",
        },
        RefusalRow {
            case: "a customer-provided encryption key for the part with no key beside it",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 1,
            headers: vec![(
                "x-amz-server-side-encryption-customer-algorithm",
                "AES256",
            )],
            status: 400,
            code: INVALID_ARGUMENT_CODE,
        },
        RefusalRow {
            case: "an expected bucket owner that is not this account",
            upload_key: None,
            copy_source: format!("{origin}/{source_key}"),
            part_number: 1,
            headers: vec![("x-amz-expected-bucket-owner", "123456789012")],
            status: 403,
            code: ACCESS_DENIED_CODE,
        },
    ];

    for row in &rows {
        let case = row.case;
        let refused = upload_part_copy(
            &router,
            target,
            row.upload_key.unwrap_or(key),
            &upload_id,
            row.part_number,
            &row.copy_source,
            &row.headers,
        )
        .await;
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
            "case {case:?}: expected the {} code, got {:?}",
            row.code,
            refused.error_code()
        );
        // Nothing was recorded: the upload holds no part at all, so a refused copy cannot be
        // told from one that never arrived.
        let listed = list_parts(&router, target, key, &upload_id).await;
        assert_eq!(
            reported_parts(&listed),
            Vec::new(),
            "case {case:?}: a refused copy must record no part, got body {:?}",
            text(&listed)
        );
    }

    // An upload id the store never minted names no upload either, whatever the copy source.
    let unknown = upload_part_copy(
        &router,
        target,
        key,
        "not-an-upload-id",
        1,
        &format!("{origin}/{source_key}"),
        &[],
    )
    .await;
    assert_eq!(
        unknown.status,
        404,
        "an upload id the store never minted must answer 404, got {} with body {:?}",
        unknown.status,
        text(&unknown)
    );
    assert_eq!(
        unknown.error_code(),
        "NoSuchUpload",
        "expected the published code for an upload that does not exist, got {:?}",
        unknown.error_code()
    );
}

#[tokio::test]
async fn the_parts_a_copy_recorded_are_the_object_a_completion_assembles() {
    let router = router();
    let origin = "part-copy-assembly-origin";
    let target = "part-copy-assembly-target";
    bucket(&router, origin).await;
    bucket(&router, target).await;

    // The sources: one past the published minimum part size, because every part but the last
    // one has to carry at least that much, and a small one for the last part.
    let head = seeded_payload(0x5eed_0001, MIN_MULTIPART_PART_BYTES + 1);
    let tail = b"tail-bytes".to_vec();
    for (key, body) in [("sources/head.bin", &head), ("sources/tail.bin", &tail)] {
        let stored = put_object(&router, origin, key, body, &[]).await;
        assert_eq!(
            stored.status, 200,
            "the fixture write of {key:?} must succeed, got {}",
            stored.status
        );
    }

    let key = "assembled/object.bin";
    let upload_id = initiate(&router, target, key).await;
    let mut parts: Vec<(i32, String)> = Vec::new();
    for (number, source) in [(1, "sources/head.bin"), (2, "sources/tail.bin")] {
        let copied = upload_part_copy(
            &router,
            target,
            key,
            &upload_id,
            number,
            &format!("{origin}/{source}"),
            &[],
        )
        .await;
        assert_eq!(
            copied.status,
            200,
            "copying part {number} must answer HTTP 200, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        parts.push((
            number,
            copied
                .element("ETag")
                .expect("a part copy must report the part's ETag"),
        ));
    }

    // Both parts are there before the completion, in ascending part-number order.
    let listed = list_parts(&router, target, key, &upload_id).await;
    let reported = reported_parts(&listed);
    assert_eq!(
        reported.iter().map(|(number, _, _)| *number).collect::<Vec<_>>(),
        vec![1, 2],
        "expected both copied parts to be listed, got body {:?}",
        text(&listed)
    );

    let completed = complete(&router, target, key, &upload_id, &parts).await;
    assert_eq!(
        completed.status,
        200,
        "the completion must answer HTTP 200, got {} with body {:?}",
        completed.status,
        text(&completed)
    );

    // The object the key holds afterwards is the concatenation of the copied parts, in part
    // order, and it reads back through `GetObject`.
    let assembled: Vec<u8> = [head.clone(), tail.clone()].concat();
    let read = get_object(&router, target, key, "", &[]).await;
    assert_eq!(
        read.status,
        200,
        "the assembled object must read back, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.header("content-length"),
        Some(assembled.len().to_string()),
        "expected the summed part sizes, got {:?}",
        read.header("content-length")
    );
    assert_eq!(
        read.body.as_ref(),
        assembled.as_slice(),
        "expected the assembled object to hold the copied bytes in part order"
    );

    // And the key the copy assembled is a key a batch delete reports an outcome for, like any
    // other object the bucket holds.
    let deleted = send(
        &router,
        "POST",
        &format!("/{target}?delete"),
        &[],
        &format!("<Delete><Object><Key>{key}</Key></Object></Delete>"),
    )
    .await;
    assert_eq!(
        deleted.status,
        200,
        "the batch delete must answer HTTP 200, got {} with body {:?}",
        deleted.status,
        text(&deleted)
    );
    assert_eq!(
        deleted.element("Key").as_deref(),
        Some(key),
        "expected the batch delete to report an entry for the assembled key, got body {:?}",
        text(&deleted)
    );
    let gone = get_object(&router, target, key, "", &[]).await;
    assert_eq!(
        gone.status, 404,
        "the assembled object must be gone once deleted, got {}",
        gone.status
    );
}
