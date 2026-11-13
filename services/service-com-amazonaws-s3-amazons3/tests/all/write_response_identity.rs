// SPDX-License-Identifier: CC0-1.0

//! What a successful S3 WRITE and a successful BUCKET READ hand back: the identity of what
//! was written or looked up, end to end through the router.
//!
//! Three rules are under test, each over a SET of inputs rather than one recorded request:
//!
//! 1. A success response whose output binds NOTHING to the body carries no body. Every
//!    member of such an output goes out in a response header, so there is no document to
//!    send and S3 sends none — `PutObject` answers "Content-Length: 0" with the stored
//!    object's `ETag` and the `x-amz-version-id` of the version it created in its headers
//!    (Amazon S3 API Reference, *PutObject* sample response). An output that DOES bind a
//!    member to the body still answers with that document.
//! 2. A write reports the stored object's identity: the entity tag of the bytes it stored
//!    ("for objects where the ETag is the MD5 digest of the object, you can calculate the
//!    MD5 while putting an object to Amazon S3 and compare the returned ETag to the
//!    calculated MD5 value", `PutObjectOutput$ETag`), the version id the bucket's
//!    versioning minted for it ("Amazon S3 automatically generates a unique version ID for
//!    the object being stored. Amazon S3 returns this ID in the response",
//!    `PutObjectOutput$VersionId`), and — for a copy — the checksum the destination object
//!    carries under the member its algorithm names, beside the `ChecksumType` it was
//!    computed under (`CopyObjectResult$ChecksumCRC32` and `…$ChecksumType`).
//! 3. A read that reports a bucket reports that bucket's ARN: "the Amazon Resource Name
//!    (ARN) of the S3 bucket" (`HeadBucketOutput$BucketArn`, `Bucket$BucketArn`), which for
//!    a general purpose bucket carries its globally unique name and leaves the Region and
//!    account-id fields empty.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a client sees.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, head_bucket, router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, expected_etag, get_object, put_object, text};
use super::object_versioning::{head_object, set_versioning};

/// The `ChecksumType` a single whole-object write computes its checksum under: there are no
/// parts to compose one from (`CopyObjectResult$ChecksumType`).
const FULL_OBJECT: &str = "FULL_OBJECT";

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
    headers: &[(&str, &str)],
) -> Answer {
    let mut all: Vec<(&str, &str)> = vec![("x-amz-copy-source", copy_source)];
    all.extend_from_slice(headers);
    send_bytes(router, "PUT", &format!("/{bucket_name}/{key}"), &all, b"").await
}

/// `GET /?x-id=ListBuckets` — `ListBuckets`.
async fn list_buckets(router: &MultiServiceRouter) -> Answer {
    send(router, "GET", "/", &[], "").await
}

/// The ARN of a general purpose bucket: its name alone, with the Region and account-id
/// fields of the ARN left empty "because a bucket name is globally unique" — the form the
/// `^arn:[^:]+:(s3|s3express):` pattern of `HeadBucketOutput$BucketArn` admits.
fn general_purpose_bucket_arn(name: &str) -> String {
    format!("arn:aws:s3:::{name}")
}

/// One row of the no-body table: the case, the request to send, and whether the operation's
/// output binds a member to the response BODY (and so answers a document).
struct BodyRow<'a> {
    case: &'a str,
    method: &'a str,
    uri: &'a str,
    headers: &'a [(&'a str, &'a str)],
    body: &'a [u8],
    answers_a_document: bool,
}

#[tokio::test]
async fn a_response_that_binds_nothing_to_its_body_carries_none() {
    let router = router();
    bucket(&router, "no-body-bucket").await;
    put_object(&router, "no-body-bucket", "copied.txt", b"source", &[]).await;
    put_object(&router, "no-body-bucket", "deleted.txt", b"gone", &[]).await;

    // The rows span the methods and the output shapes the rule generalises over: a write
    // whose result is all headers (PutObject, CopyObject's own VersionId aside), a create,
    // a delete, a HEAD, and — the other side of the rule — the three operations that DO
    // bind a member to the body and so must keep answering their document.
    let rows: &[BodyRow<'_>] = &[
        BodyRow {
            case: "PutObject — ETag and VersionId are headers, so there is no document",
            method: "PUT",
            uri: "/no-body-bucket/written.txt",
            headers: &[("content-type", "text/plain")],
            body: b"written",
            answers_a_document: false,
        },
        BodyRow {
            case: "CreateBucket — Location is a header",
            method: "PUT",
            uri: "/another-no-body-bucket",
            headers: &[],
            body: b"",
            answers_a_document: false,
        },
        BodyRow {
            case: "HeadBucket — every member is a header",
            method: "HEAD",
            uri: "/no-body-bucket",
            headers: &[],
            body: b"",
            answers_a_document: false,
        },
        BodyRow {
            case: "HeadObject — every member is a header",
            method: "HEAD",
            uri: "/no-body-bucket/copied.txt",
            headers: &[],
            body: b"",
            answers_a_document: false,
        },
        BodyRow {
            case: "PutObjectTagging — only VersionId comes back, as a header",
            method: "PUT",
            uri: "/no-body-bucket/copied.txt?tagging",
            headers: &[("content-type", "application/xml")],
            body: b"<Tagging><TagSet><Tag><Key>stage</Key><Value>probe</Value></Tag>\
                   </TagSet></Tagging>",
            answers_a_document: false,
        },
        BodyRow {
            case: "DeleteObject — DeleteMarker and VersionId are headers",
            method: "DELETE",
            uri: "/no-body-bucket/deleted.txt",
            headers: &[],
            body: b"",
            answers_a_document: false,
        },
        BodyRow {
            case: "CopyObject — the CopyObjectResult container IS the body",
            method: "PUT",
            uri: "/no-body-bucket/copy.txt",
            headers: &[("x-amz-copy-source", "/no-body-bucket/copied.txt")],
            body: b"",
            answers_a_document: true,
        },
        BodyRow {
            case: "ListBuckets — the listing IS the body",
            method: "GET",
            uri: "/",
            headers: &[],
            body: b"",
            answers_a_document: true,
        },
        BodyRow {
            case: "GetObjectTagging — the tag set IS the body",
            method: "GET",
            uri: "/no-body-bucket/copied.txt?tagging",
            headers: &[],
            body: b"",
            answers_a_document: true,
        },
    ];

    for row in rows {
        let answer = send_bytes(&router, row.method, row.uri, row.headers, row.body).await;
        let case = row.case;
        assert!(
            (200..300).contains(&answer.status),
            "case {case:?}: the request must succeed, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        if row.answers_a_document {
            assert!(
                !answer.body.is_empty(),
                "case {case:?}: an output that binds a member to the body must answer that \
                 document, got an empty body"
            );
            continue;
        }
        assert!(
            answer.body.is_empty(),
            "case {case:?}: an output that binds nothing to the body must answer no body, \
             got {:?}",
            text(&answer)
        );
    }
}

/// One row of the write-identity table: the case, the key written, and the bytes.
type WriteRow<'a> = (&'a str, &'a str, &'a [u8]);

/// The rows differ along what the ETag rule generalises over: the SIZE of the body (empty,
/// one byte, a few bytes, 8 KiB) and what it holds (text, digits, bytes that are not UTF-8).
const WRITE_ROWS: &[WriteRow<'static>] = &[
    ("the empty body", "empty.bin", b""),
    ("a single byte", "one.bin", b"a"),
    ("a short text body", "short.txt", b"the quick brown fox"),
    (
        "eighty digits",
        "digits.txt",
        b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
    ),
    (
        "bytes that are not text at all",
        "binary.bin",
        b"\x00\x01\x02\xff\xfe\xfd\x00",
    ),
];

/// 8 KiB of one byte, for the largest row of [`WRITE_ROWS`] — built rather than written out.
fn repeated_bytes() -> Vec<u8> {
    vec![0x7a; 8 * 1024]
}

#[tokio::test]
async fn a_write_reports_the_entity_tag_it_stored_and_the_version_id_it_minted() {
    let router = router();
    // The same table written into a bucket in each versioning state the rule distinguishes:
    // only `Enabled` mints a version id for a write.
    let buckets: [(&str, Option<&str>, bool); 3] = [
        ("a bucket no versioning was ever set on", None, false),
        ("a versioning-ENABLED bucket", Some("Enabled"), true),
        ("a versioning-SUSPENDED bucket", Some("Suspended"), false),
    ];

    for (bucket_case, versioning, mints_a_version) in buckets {
        let name = match versioning {
            None => "etag-plain",
            Some("Enabled") => "etag-enabled",
            _ => "etag-suspended",
        };
        bucket(&router, name).await;
        if let Some(status) = versioning {
            set_versioning(&router, name, status).await;
        }

        let large = repeated_bytes();
        let rows = WRITE_ROWS
            .iter()
            .copied()
            .chain([("8 KiB of one byte", "repeated.bin", large.as_slice())]);
        for (row_case, key, body) in rows {
            let case = format!("{bucket_case} / {row_case}");
            let written = put_object(&router, name, key, body, &[]).await;
            assert_eq!(
                written.status, 200,
                "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
                written.status,
                text(&written)
            );
            assert_eq!(
                written.header("etag").as_deref(),
                Some(expected_etag(body).as_str()),
                "case {case:?}: expected the quoted MD5 of the stored bytes, got {:?}",
                written.header("etag")
            );
            let version = written.header("x-amz-version-id");
            assert_eq!(
                version.is_some(),
                mints_a_version,
                "case {case:?}: a version id is reported exactly when the bucket's \
                 versioning minted one, got {version:?}"
            );

            // The identity the write reported addresses the object it created: a read of
            // the key reports the same tag, and a read of the reported version id resolves
            // to it.
            let read = head_object(&router, name, key, "").await;
            assert_eq!(
                read.header("etag"),
                written.header("etag"),
                "case {case:?}: the read must report the ETag the write did, got {:?}",
                read.header("etag")
            );
            if let Some(version_id) = version {
                let by_version =
                    head_object(&router, name, key, &format!("?versionId={version_id}")).await;
                assert_eq!(
                    by_version.status, 200,
                    "case {case:?}: the reported version id {version_id:?} must address the \
                     version just written, got {}",
                    by_version.status
                );
                assert_eq!(
                    by_version.header("etag").as_deref(),
                    Some(expected_etag(body).as_str()),
                    "case {case:?}: the version the write reported must hold its bytes, got \
                     {:?}",
                    by_version.header("etag")
                );
            }
        }
    }
}

/// One row of the copy-checksum table: the case, the `ChecksumAlgorithm` value, and the XML
/// element of `CopyObjectResult` that reports it.
///
/// The algorithms and their member names are the model's own
/// (`smithy_query(query="shape",shape="ChecksumAlgorithm")` lists `CRC32`, `CRC32C`, `SHA1`,
/// `SHA256`, `CRC64NVME`, `SHA512`, `MD5`, `XXHASH64`, `XXHASH3`, `XXHASH128`, and
/// `…shape="CopyObjectResult"` the matching `Checksum*` members), so the table spans every
/// value the enum admits rather than the one the recorded copy used.
const CHECKSUM_ROWS: &[(&str, &str, &str)] = &[
    ("the CRC32 the enum lists first", "CRC32", "ChecksumCRC32"),
    ("a second CRC", "CRC32C", "ChecksumCRC32C"),
    ("the 64-bit CRC", "CRC64NVME", "ChecksumCRC64NVME"),
    ("a 160-bit digest", "SHA1", "ChecksumSHA1"),
    ("a 256-bit digest", "SHA256", "ChecksumSHA256"),
    ("a 512-bit digest", "SHA512", "ChecksumSHA512"),
    ("the MD5 digest", "MD5", "ChecksumMD5"),
    ("a non-cryptographic hash", "XXHASH64", "ChecksumXXHASH64"),
    ("its 64-bit successor", "XXHASH3", "ChecksumXXHASH3"),
    ("its 128-bit successor", "XXHASH128", "ChecksumXXHASH128"),
];

#[tokio::test]
async fn a_copy_reports_the_checksum_and_type_the_destination_object_carries() {
    let router = router();
    let name = "copy-checksum-bucket";
    bucket(&router, name).await;
    let payload = b"the bytes the copy carries over".as_slice();
    put_object(&router, name, "source.txt", payload, &[]).await;

    for (case, algorithm, member) in CHECKSUM_ROWS {
        // A copy that NAMES an algorithm has its checksum computed under that one.
        let destination = format!("copy-{}.txt", algorithm.to_ascii_lowercase());
        let copied = copy_object(
            &router,
            name,
            &destination,
            &format!("/{name}/source.txt"),
            &[("x-amz-checksum-algorithm", algorithm)],
        )
        .await;
        assert_eq!(
            copied.status, 200,
            "case {case:?}: expected HTTP 200 from CopyObject, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        let reported = copied.element(member).unwrap_or_default();
        assert!(
            !reported.is_empty(),
            "case {case:?}: the copy must report its checksum under <{member}>, got body {:?}",
            text(&copied)
        );
        assert_eq!(
            copied.element("ChecksumType").as_deref(),
            Some(FULL_OBJECT),
            "case {case:?}: a one-shot copy's checksum is over the whole object, got {:?}",
            copied.element("ChecksumType")
        );
        // The value describes the DESTINATION object: a read of it with the checksum mode
        // enabled reports the same checksum under the same algorithm.
        let read = get_object(
            &router,
            name,
            &destination,
            "",
            &[("x-amz-checksum-mode", "ENABLED")],
        )
        .await;
        let header = format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase());
        assert_eq!(
            read.header(&header).as_deref(),
            Some(reported.as_str()),
            "case {case:?}: the copy's checksum must be the one every reader of the stored \
             object reports, got {:?}",
            read.header(&header)
        );
        assert_eq!(
            read.header("x-amz-checksum-type").as_deref(),
            Some(FULL_OBJECT),
            "case {case:?}: the stored checksum's type must read back, got {:?}",
            read.header("x-amz-checksum-type")
        );
    }

    // A copy that names NO algorithm carries the source object's own whole-object
    // checksum — the bytes are the same bytes — and reports it with its type.
    let with_checksum = put_object(
        &router,
        name,
        "checksummed.txt",
        payload,
        &[("x-amz-sdk-checksum-algorithm", "CRC32")],
    )
    .await;
    let stored = with_checksum.header("x-amz-checksum-crc32");
    assert!(
        stored.is_some(),
        "the source write must store a checksum for the copy to carry over, got {:?}",
        text(&with_checksum)
    );
    let inherited = copy_object(
        &router,
        name,
        "inherited.txt",
        &format!("/{name}/checksummed.txt"),
        &[],
    )
    .await;
    assert_eq!(
        inherited.element("ChecksumCRC32"),
        stored,
        "a copy naming no algorithm carries the source's whole-object checksum, got body \
         {:?}",
        text(&inherited)
    );
    assert_eq!(
        inherited.element("ChecksumType").as_deref(),
        Some(FULL_OBJECT),
        "the inherited checksum's type travels with it, got {:?}",
        inherited.element("ChecksumType")
    );

    // And a copy of a source whose write named no algorithm carries the DEFAULT one S3
    // added to that source ("Amazon S3 added the default checksum, CRC64NVME, to the
    // uploaded object", `PutObjectOutput$ChecksumCRC64NVME`), over the destination's bytes —
    // never a second algorithm's value beside it.
    let bare = copy_object(
        &router,
        name,
        "bare.txt",
        &format!("/{name}/source.txt"),
        &[],
    )
    .await;
    assert_eq!(
        bare.element("ChecksumCRC64NVME"),
        service_com_amazonaws_s3_amazons3::state::full_object_checksum("CRC64NVME", payload),
        "a copy of an object carrying the default checksum carries it too, got body {:?}",
        text(&bare)
    );
    assert_eq!(
        (bare.element("ChecksumCRC32"), bare.element("ChecksumType")),
        (None, Some(FULL_OBJECT.to_string())),
        "the inherited default checksum is over the whole object, and no other algorithm's \
         value is reported, got body {:?}",
        text(&bare)
    );
}

#[tokio::test]
async fn a_bucket_read_reports_the_arn_of_the_bucket_it_read() {
    let router = router();
    // The rows differ along the shape of the bucket NAME the ARN carries: the shortest name
    // the rules admit, a hyphenated one, one with digits and dots, and the longest.
    let names: [(&str, String); 4] = [
        ("the shortest name S3 admits", "abc".to_string()),
        ("a hyphenated name", "head-bucket-arn-probe".to_string()),
        ("digits and dots", "bucket.7.logs".to_string()),
        ("the longest name S3 admits", "b".repeat(63)),
    ];

    for (case, name) in &names {
        let created = create_bucket(&router, name, None, None).await;
        assert_eq!(
            created.status, 200,
            "case {case:?}: the bucket {name:?} must be created, got {} with body {:?}",
            created.status,
            text(&created)
        );

        let head = head_bucket(&router, name).await;
        assert_eq!(
            head.status, 200,
            "case {case:?}: HeadBucket must answer 200, got {}",
            head.status
        );
        assert_eq!(
            head.header("x-amz-bucket-arn").as_deref(),
            Some(general_purpose_bucket_arn(name).as_str()),
            "case {case:?}: HeadBucket must report the bucket's ARN, got {:?}",
            head.header("x-amz-bucket-arn")
        );
    }

    // The listing names every bucket exactly as a read of that one bucket does.
    let listed = list_buckets(&router).await;
    let document = text(&listed);
    for (case, name) in &names {
        let arn = general_purpose_bucket_arn(name);
        assert!(
            document.contains(&format!("<BucketArn>{arn}</BucketArn>")),
            "case {case:?}: the listing entry for {name:?} must carry <BucketArn>{arn}</BucketArn>, \
             got {document:?}"
        );
    }
}
