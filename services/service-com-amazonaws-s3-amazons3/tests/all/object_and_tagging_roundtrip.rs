// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an OBJECT's lifecycle: `PutObject` stores it and `GetObject`
//! reads it back.
//!
//! Every assertion drives a real HTTP request through the router (the harness, and the
//! tagging helpers the last suite here uses, are the ones the bucket-lifecycle and
//! bucket-tagging suites in this crate already own), so what is pinned is the wire
//! behaviour — status, bound headers, payload, error code — rather than any handler's
//! internals.
//!
//! The rules under test, with their sources:
//!
//! * `PutObject` stores the request's bytes and metadata under `<Bucket>/<Key>`, replacing
//!   any object already there — "if it receives multiple write requests for the same
//!   object simultaneously, it overwrites all but the last object written", and "You
//!   cannot use PutObject to only update a single piece of metadata for an existing
//!   object. You must put the entire object with updated metadata" — and reports the
//!   object's entity tag: "for objects where the ETag is the MD5 digest of the object, you
//!   can calculate the MD5 while putting an object to Amazon S3 and compare the returned
//!   ETag to the calculated MD5 value" (`PutObjectOutput$ETag`).
//! * `GetObject` answers with exactly those stored bytes and headers, reports
//!   `StorageClass` "for all objects except for S3 Standard storage class objects"
//!   (`GetObjectOutput$StorageClass`), and lets the `response-*` query parameters override
//!   the matching response header for that one read.
//! * A write naming a bucket this emulator does not hold answers the service's
//!   `NoSuchBucket` (HTTP 404) and stores nothing; a read of a key that is not there
//!   answers the modelled `NoSuchKey` (HTTP 404).
//! * `Content-MD5` is "a message integrity check to verify that the request body was not
//!   corrupted in transit", so a digest that does not match the payload received is
//!   refused and nothing is stored.
//! * A member whose semantics this emulator cannot carry out keeps the scaffold's 501
//!   rather than being stripped and answered as a plain success.

use std::sync::Arc;
use std::time::SystemTime;

use md5::{Digest, Md5};
use service_com_amazonaws_s3_amazons3::state::{
    ComAmazonawsS3Amazons3State, Object, ObjectEncryption, ObjectWrite, DEFAULT_STORAGE_CLASS,
    MAX_OBJECT_KEY_BYTES, MAX_OBJECT_METADATA_BYTES, MAX_RETAINED_BYTES_TOTAL,
    MAX_STORED_VERSIONS_TOTAL,
};
use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{create_bucket, router, send_bytes, Answer};
use super::bucket_tagging::{get_bucket_tagging, put_bucket_tagging, tag, tag_set_of};

/// A stored object of `length` zero bytes, for the suites that fill a bucket's budget
/// through the store rather than over the wire.
fn sized_object(length: usize) -> Object {
    let body = vec![0u8; length];
    Object {
        etag: expected_etag(&body),
        last_modified: SystemTime::now(),
        storage_class: DEFAULT_STORAGE_CLASS.to_string(),
        content_type: None,
        cache_control: None,
        content_disposition: None,
        content_encoding: None,
        content_language: None,
        expires: None,
        website_redirect_location: None,
        metadata: std::collections::BTreeMap::new(),
        tags: Vec::new(),
        // A write that names no server-side encryption records none, so a stored object
        // seeded through the store carries none either.
        encryption: ObjectEncryption::default(),
        // The store mints an object's version id; one handed to `put_object` carries none.
        version_id: None,
        // A write places no Object Lock retention; `PutObjectRetention` is what does.
        retention: None,
        // Nor a legal hold; `PutObjectLegalHold` is what applies one.
        legal_hold: None,
        // A write initiates no restore either; `RestoreObject` is what does.
        restore: None,
        // And it attaches no annotation; `PutObjectAnnotation` is what does.
        annotations: std::collections::BTreeMap::new(),
        // Nor an ACL; `PutObjectAcl` is what sets one.
        acl: None,
        // And no checksum: a write that names no checksum algorithm stores none.
        checksum: None,
        // Nor is it assembled from parts: one write stored it whole.
        parts: Vec::new(),
        // And nothing replicated it: the store settles a replication record from the
        // bucket's own configuration.
        replication: None,
        body,
    }
}

/// A stored object whose RETAINED footprint — what the budgets count, payload plus every
/// string kept beside it — is exactly `bytes`.
///
/// The non-payload part of a bare object is a fixed size (the quoted 32-hex entity tag and
/// the storage class), so the body is the remainder; the assertion pins that arithmetic
/// rather than trusting it.
fn object_retaining(bytes: usize) -> Object {
    let overhead = sized_object(0).retained_bytes();
    let object = sized_object(bytes - overhead);
    assert_eq!(
        object.retained_bytes(),
        bytes,
        "case {bytes}: the test's own object must retain exactly the bytes it claims, got {}",
        object.retained_bytes()
    );
    object
}

/// The smallest stored object, for the suite that fills a bucket's object count.
fn one_byte_object() -> Object {
    sized_object(1)
}

/// `PUT /<bucket>/<key>` — `PutObject`. No `x-id` query literal, which is what botocore
/// puts on the wire.
///
/// `pub(crate)`, like the four helpers below it: the object listing and deletion suite in
/// this same test crate writes and reads its objects through these rather than standing up
/// a second set.
pub(crate) async fn put_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "PUT", &format!("/{bucket}/{key}"), headers, body).await
}

/// `GET /<bucket>/<key>` — `GetObject`.
pub(crate) async fn get_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    query: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket}/{key}{query}"),
        headers,
        b"",
    )
    .await
}

/// A bucket to hold objects, created the way the bucket-lifecycle suite creates one.
pub(crate) async fn bucket(router: &MultiServiceRouter, name: &str) {
    let created = create_bucket(router, name, None, None).await;
    assert_eq!(
        created.status, 200,
        "the bucket {name:?} must be created first, got {} with body {:?}",
        created.status,
        String::from_utf8_lossy(&created.body)
    );
}

/// The body as text, for error documents.
pub(crate) fn text(answer: &Answer) -> String {
    String::from_utf8_lossy(&answer.body).to_string()
}

/// The entity tag an object of these bytes must report: the MD5 digest, lowercase hex,
/// inside the quotes S3 wraps it in.
pub(crate) fn expected_etag(body: &[u8]) -> String {
    format!("\"{}\"", hex::encode(Md5::digest(body)))
}

/// `length` bytes spanning the whole 0..=255 range, from a fixed seed, so the payload is
/// large and byte-varied without anything random: a 32-bit LCG (the "ANSI C" multiplier
/// and increment), taking the high byte of each step.
fn generated_bytes(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect()
}

/// One row of the metadata table: the case name, the headers the write carries, and the
/// headers the read must then report.
type MetadataRow<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [(&'a str, &'a str)]);

/// One row of the overwrite table: the case name, the payload the write carries, and the
/// headers it carries with it.
type OverwriteRow<'a> = (&'a str, &'a [u8], &'a [(&'a str, &'a str)]);

#[tokio::test]
async fn objects_round_trip_through_put_and_get() {
    let router = router();
    bucket(&router, "object-bucket").await;

    // (case, key, payload, the published MD5 of that payload when there is one).
    //
    // The rows differ along what the rule generalises over: the key's shape (a flat name,
    // the `dags/` prefix a provisioning client uploads under, a deep path), and the
    // payload's size (empty, one byte, a megabyte) and type (ASCII text, UTF-8, bytes
    // including NUL and 0xFF). The hex digests are the MD5 test-suite vectors published
    // in RFC 1321, Appendix A.5 — an expectation computed nowhere in this process.
    let million = generated_bytes(1_048_576, 0x5EED_1234);
    let rows: [(&str, &str, &[u8], Option<&str>); 8] = [
        (
            "the empty object",
            "empty",
            b"",
            Some("d41d8cd98f00b204e9800998ecf8427e"),
        ),
        (
            "a single byte",
            "a.txt",
            b"a",
            Some("0cc175b9c0f1b6a831c399e269772661"),
        ),
        (
            "three bytes under the dags/ prefix",
            "dags/abc.py",
            b"abc",
            Some("900150983cd24fb0d6963f7d28e17f72"),
        ),
        (
            "a short phrase",
            "message",
            b"message digest",
            Some("f96b697d7cb7938d525a2f31aaf161d0"),
        ),
        (
            "the lowercase alphabet at a deep path",
            "dags/sub/dir/alphabet.txt",
            b"abcdefghijklmnopqrstuvwxyz",
            Some("c3fcd3d76192e4007dfb496cca67e13b"),
        ),
        (
            "bytes that are not text, including NUL and 0xFF",
            "binary.bin",
            &[0u8, 1, 2, 0xFE, 0xFF, 0x80, 0x00, 0x7F],
            None,
        ),
        (
            "UTF-8 text",
            "unicode.txt",
            "DAG — données ✓".as_bytes(),
            None,
        ),
        ("a megabyte", "big/payload.bin", &million, None),
    ];

    for (case, key, payload, published_md5) in rows {
        let put = put_object(&router, "object-bucket", key, payload, &[]).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
            put.status,
            text(&put)
        );
        let etag = expected_etag(payload);
        if let Some(published) = published_md5 {
            assert_eq!(
                etag,
                format!("\"{published}\""),
                "case {case:?}: the digest this test expects must be the published one, \
                 got {etag:?}"
            );
        }
        assert_eq!(
            put.header("etag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected the MD5 entity tag {etag:?}, got {:?}",
            put.header("etag")
        );

        // Read the object back: the same bytes, the same tag, and a Last-Modified.
        let got = get_object(&router, "object-bucket", key, "", &[]).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetObject on the object just written, \
             got {} with body {:?}",
            got.status,
            text(&got)
        );
        assert_eq!(
            got.body.as_ref(),
            payload,
            "case {case:?}: expected the stored payload back ({} bytes), got {} bytes",
            payload.len(),
            got.body.len()
        );
        assert_eq!(
            got.header("etag").as_deref(),
            Some(etag.as_str()),
            "case {case:?}: expected GetObject to report the same entity tag {etag:?}, \
             got {:?}",
            got.header("etag")
        );
        assert_eq!(
            got.header("content-length"),
            Some(payload.len().to_string()),
            "case {case:?}: expected Content-Length {}, got {:?}",
            payload.len(),
            got.header("content-length")
        );
        assert!(
            got.header("last-modified").is_some(),
            "case {case:?}: a stored object reports when it was written"
        );
        // STANDARD is the storage class a PUT naming none gets, and "Amazon S3 returns
        // this header for all objects except for S3 Standard storage class objects".
        assert_eq!(
            got.header("x-amz-storage-class"),
            None,
            "case {case:?}: a STANDARD object reports no storage class, got {:?}",
            got.header("x-amz-storage-class")
        );
    }
}

#[tokio::test]
async fn an_objects_metadata_and_headers_round_trip() {
    let router = router();
    bucket(&router, "metadata-bucket").await;

    // Each row is written with the headers in the middle column and must read back with
    // the headers in the right one: the members S3 stores WITH the object and returns on
    // every read, plus the storage class, which is reported only when it is not STANDARD.
    let rows: [MetadataRow<'_>; 5] = [
        (
            "a content type",
            &[("content-type", "text/x-python")],
            &[("content-type", "text/x-python")],
        ),
        (
            "caching and disposition",
            &[
                ("cache-control", "max-age=31536000"),
                ("content-disposition", "attachment; filename=\"dag.py\""),
            ],
            &[
                ("cache-control", "max-age=31536000"),
                ("content-disposition", "attachment; filename=\"dag.py\""),
            ],
        ),
        (
            "encoding and language",
            &[("content-encoding", "gzip"), ("content-language", "fr-CA")],
            &[("content-encoding", "gzip"), ("content-language", "fr-CA")],
        ),
        (
            "user-defined metadata",
            &[
                ("x-amz-meta-dag-id", "example_dag"),
                ("x-amz-meta-uploaded-by", "provisioner"),
            ],
            &[
                ("x-amz-meta-dag-id", "example_dag"),
                ("x-amz-meta-uploaded-by", "provisioner"),
            ],
        ),
        (
            "a storage class that is not STANDARD",
            &[("x-amz-storage-class", "STANDARD_IA")],
            &[("x-amz-storage-class", "STANDARD_IA")],
        ),
    ];

    for (index, (case, request_headers, expected_headers)) in rows.iter().enumerate() {
        let key = format!("dags/metadata-{index}.py");
        let put = put_object(&router, "metadata-bucket", &key, b"# dag", request_headers).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
            put.status,
            text(&put)
        );

        let got = get_object(&router, "metadata-bucket", &key, "", &[]).await;
        assert_eq!(
            got.status, 200,
            "case {case:?}: expected HTTP 200 from GetObject, got {} with body {:?}",
            got.status,
            text(&got)
        );
        for (name, value) in *expected_headers {
            assert_eq!(
                got.header(name).as_deref(),
                Some(*value),
                "case {case:?}: expected the stored {name} to read back as {value:?}, \
                 got {:?}",
                got.header(name)
            );
        }
    }

    // The `response-*` query parameters override the matching response header on a read
    // ("Sets the Content-Type header of the response", and so on), leaving the stored
    // object untouched.
    let key = "dags/override.py";
    put_object(
        &router,
        "metadata-bucket",
        key,
        b"# dag",
        &[
            ("content-type", "text/x-python"),
            ("cache-control", "no-cache"),
        ],
    )
    .await;
    let overridden = get_object(
        &router,
        "metadata-bucket",
        key,
        "?response-content-type=application/octet-stream&response-cache-control=max-age%3D60",
        &[],
    )
    .await;
    assert_eq!(
        overridden.header("content-type").as_deref(),
        Some("application/octet-stream"),
        "case \"response-content-type\": the query override must win over the stored \
         value, got {:?}",
        overridden.header("content-type")
    );
    assert_eq!(
        overridden.header("cache-control").as_deref(),
        Some("max-age=60"),
        "case \"response-cache-control\": the query override must win over the stored \
         value, got {:?}",
        overridden.header("cache-control")
    );
    let plain = get_object(&router, "metadata-bucket", key, "", &[]).await;
    assert_eq!(
        plain.header("content-type").as_deref(),
        Some("text/x-python"),
        "case \"override is per-read\": the stored content type must survive an \
         overridden read, got {:?}",
        plain.header("content-type")
    );
}

#[tokio::test]
async fn writing_an_object_replaces_the_one_at_that_key() {
    let router = router();
    bucket(&router, "overwrite-bucket").await;

    // "If it receives multiple write requests for the same object simultaneously, it
    // overwrites all but the last object written" — and "You cannot use PutObject to only
    // update a single piece of metadata for an existing object. You must put the entire
    // object with updated metadata", so each write REPLACES the whole object, metadata
    // included. Every row below is written at the SAME key.
    let rows: [OverwriteRow<'_>; 4] = [
        (
            "the first version",
            b"version one",
            &[("content-type", "text/plain"), ("x-amz-meta-v", "1")],
        ),
        (
            "a longer body, no headers at all",
            b"version two is longer",
            &[],
        ),
        ("a shorter body", b"v3", &[("content-type", "text/x-python")]),
        ("the empty body", b"", &[]),
    ];

    for (case, payload, headers) in rows {
        let put = put_object(&router, "overwrite-bucket", "dags/dag.py", payload, headers).await;
        assert_eq!(
            put.status, 200,
            "case {case:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
            put.status,
            text(&put)
        );

        let got = get_object(&router, "overwrite-bucket", "dags/dag.py", "", &[]).await;
        assert_eq!(
            got.body.as_ref(),
            payload,
            "case {case:?}: the object must be exactly the last one written, got {:?}",
            text(&got)
        );
        assert_eq!(
            got.header("etag").as_deref(),
            Some(expected_etag(payload).as_str()),
            "case {case:?}: the entity tag must follow the stored bytes, got {:?}",
            got.header("etag")
        );
        // The metadata of the previous version is gone, not merged: a header is reported
        // only when THIS write carried it.
        let expected_type = headers
            .iter()
            .find(|(header, _)| *header == "content-type")
            .map(|(_, value)| (*value).to_string());
        match &expected_type {
            Some(value) => assert_eq!(
                got.header("content-type").as_deref(),
                Some(value.as_str()),
                "case {case:?}: expected the content type of this write ({value:?}), got {:?}",
                got.header("content-type")
            ),
            // An object stored with no `Content-Type` carries S3's documented default,
            // `binary/octet-stream` — never the type of the object this write replaced,
            // and never a type inferred from the key.
            None => assert_eq!(
                got.header("content-type").as_deref(),
                Some("binary/octet-stream"),
                "case {case:?}: an object written with no content type must report S3's \
                 default rather than the replaced object's, got {:?}",
                got.header("content-type")
            ),
        }
        let expected_meta = headers
            .iter()
            .find(|(header, _)| *header == "x-amz-meta-v")
            .map(|(_, value)| (*value).to_string());
        assert_eq!(
            got.header("x-amz-meta-v"),
            expected_meta,
            "case {case:?}: expected the user metadata of this write ({expected_meta:?}), \
             got {:?}",
            got.header("x-amz-meta-v")
        );
    }
}

#[tokio::test]
async fn object_writes_and_reads_are_refused_outside_what_is_stored() {
    let router = router();
    bucket(&router, "refusal-bucket").await;
    put_object(&router, "refusal-bucket", "dags/held.py", b"# held", &[]).await;

    // A write into a bucket this emulator does not hold is refused with the 404 the
    // SERVICE models for `NoSuchBucket`, never recorded. The names below are not the one
    // bucket created above: a different name, a prefix of it, an extension of it, and one
    // differing only in case.
    for name in [
        "absent-bucket",
        "refusal",
        "refusal-bucket-2",
        "Refusal-Bucket",
    ] {
        let put = put_object(&router, name, "dags/held.py", b"# dag", &[]).await;
        assert_eq!(
            put.status, 404,
            "case {name:?}: expected PutObject into a bucket this emulator does not hold \
             to be refused with HTTP 404, got {} with body {:?}",
            put.status,
            text(&put)
        );
        assert_eq!(
            put.error_code(),
            "NoSuchBucket",
            "case {name:?}: expected the NoSuchBucket code, got {:?}",
            put.error_code()
        );

        let got = get_object(&router, name, "dags/held.py", "", &[]).await;
        assert_eq!(
            got.status, 404,
            "case {name:?}: expected GetObject on a bucket this emulator does not hold to \
             answer HTTP 404, got {} with body {:?}",
            got.status,
            text(&got)
        );
        assert_eq!(
            got.error_code(),
            "NoSuchBucket",
            "case {name:?}: expected the NoSuchBucket code, got {:?}",
            got.error_code()
        );
    }

    // Keys that are NOT the one object stored above, including one differing only in case
    // (object keys are case-sensitive).
    for key in [
        "dags/absent.py",
        "dags",
        "dags/held",
        "dags/held.py.bak",
        "DAGS/HELD.PY",
    ] {
        let got = get_object(&router, "refusal-bucket", key, "", &[]).await;
        assert_eq!(
            got.status, 404,
            "case {key:?}: expected the modelled NoSuchKey status for a key this bucket \
             does not hold, got {} with body {:?}",
            got.status,
            text(&got)
        );
        assert_eq!(
            got.error_code(),
            "NoSuchKey",
            "case {key:?}: expected the NoSuchKey code, got {:?}",
            got.error_code()
        );
    }

    // `Content-MD5` is "a message integrity check to verify that the request body was not
    // corrupted in transit", so a value that does not describe the payload received is
    // refused and the key is not created. Which refusal depends on WHAT is wrong with the
    // value: one that cannot be a 128-bit digest at all is `InvalidDigest`, and a
    // well-formed digest of other bytes is `BadDigest` — both HTTP 400, neither a shape in
    // this model.
    let correct = ::aws_smithy_types::base64::encode(Md5::digest(b"# dag"));
    let matching = put_object(
        &router,
        "refusal-bucket",
        "dags/checked.py",
        b"# dag",
        &[("content-md5", correct.as_str())],
    )
    .await;
    assert_eq!(
        matching.status, 200,
        "case \"matching Content-MD5\": expected the write to be accepted, got {} with \
         body {:?}",
        matching.status,
        text(&matching)
    );
    for (case, digest, code) in [
        (
            "a digest of different bytes",
            ::aws_smithy_types::base64::encode(Md5::digest(b"# other")),
            "BadDigest",
        ),
        (
            "a digest of the empty body",
            ::aws_smithy_types::base64::encode(Md5::digest(b"")),
            "BadDigest",
        ),
        (
            "not a digest at all",
            "not-base64-at-all".to_string(),
            "InvalidDigest",
        ),
    ] {
        let refused = put_object(
            &router,
            "refusal-bucket",
            "dags/unchecked.py",
            b"# dag",
            &[("content-md5", digest.as_str())],
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected a mismatched Content-MD5 to be refused with HTTP \
             400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the {code} code, got {:?}",
            refused.error_code()
        );
        let absent = get_object(&router, "refusal-bucket", "dags/unchecked.py", "", &[]).await;
        assert_eq!(
            absent.status, 404,
            "case {case:?}: a refused write must store nothing, got GetObject {}",
            absent.status
        );
    }

    // Every member whose semantics this emulator once could not carry out is now served and
    // recorded: the server-side-encryption pair is recorded with the object and reported back
    // (the `object_encryption` suite covers it), `x-amz-tagging`'s tag set is parsed and
    // stored (`object_tagging_on_put.rs`), the conditional members are judged against
    // what the key holds (`object_conditional_writes.rs`), the Object Lock mode,
    // retain-until date, legal hold and EVENT HOLD are stored ON the version the write
    // records (`object_lock_on_write.rs`), and a customer-provided key is recorded as the
    // algorithm and that key's digest (`object_customer_key_encryption.rs`) — so this write
    // has no member left to be refused the scaffold's 501 for.

    // `versionId` is NOT in this table: it names one stored version of the key, which
    // this emulator does keep (see the `object_versioning` suite), so a read naming one
    // is answered rather than refused. Neither is a PARTIAL read: a `Range` and a
    // `partNumber` each name a span of the object, which is served as the 206 the
    // `zz_probe` suite reads back in pieces. Nor are the two conditional headers that
    // answer 304: they are served (see [`super::object_conditional_reads`]).
    for (case, headers, query) in [
        ("a ranged read", &[("range", "bytes=0-3")][..], ""),
        ("a part-scoped read", &[][..], "?partNumber=1"),
    ] {
        let partial = get_object(&router, "refusal-bucket", "dags/held.py", query, headers).await;
        assert_eq!(
            partial.status, 206,
            "case {case:?}: expected the partial-content status, got {} with body {:?}",
            partial.status,
            text(&partial)
        );
    }

    // And the object that IS held still reads back, so every row above failed on what it
    // named rather than on the operation being unreachable.
    let held = get_object(&router, "refusal-bucket", "dags/held.py", "", &[]).await;
    assert_eq!(
        held.status, 200,
        "the held object must still read back, got {} with body {:?}",
        held.status,
        text(&held)
    );
    assert_eq!(
        held.body.as_ref(),
        b"# held",
        "the held object's bytes must be untouched by the refusals, got {:?}",
        text(&held)
    );
}

#[tokio::test]
async fn object_writes_are_bounded_only_by_the_published_key_and_metadata_limits() {
    // This suite needs the store itself, to stand a bucket past every capacity figure this
    // emulator ever kept without dispatching thousands of requests. The engine is the SAME
    // one the router hands the service, so what is seeded here is what the handler sees.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    bucket(&router, "key-bound-bucket").await;
    bucket(&router, "count-bound-bucket").await;
    bucket(&router, "byte-bound-bucket").await;

    // A published limit, so it is tested at the bound and one past it: "The name for a key
    // is a sequence of Unicode characters whose UTF-8 encoding is at most 1,024 bytes
    // long" (Amazon S3 User Guide, "Creating object key names").
    for (case, key_bytes, expected_status) in [
        ("one byte", 1, 200),
        ("just inside the published key limit", MAX_OBJECT_KEY_BYTES - 1, 200),
        ("exactly at the published key limit", MAX_OBJECT_KEY_BYTES, 200),
        ("one byte past the published key limit", MAX_OBJECT_KEY_BYTES + 1, 400),
    ] {
        let key = "k".repeat(key_bytes);
        let answer = put_object(&router, "key-bound-bucket", &key, b"x", &[]).await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for a {key_bytes}-byte key, \
             got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let read_back = get_object(&router, "key-bound-bucket", &key, "", &[]).await;
        let expected_read = if expected_status == 200 { 200 } else { 404 };
        assert_eq!(
            read_back.status, expected_read,
            "case {case:?}: a refused write must store nothing and an accepted one must \
             read back, got GetObject {}",
            read_back.status
        );
    }

    // What the store's memory guard counts is what an object RETAINS, not its payload
    // alone: the strings kept beside it and every metadata key and value are part of the
    // footprint, so none of them escapes the one budget that bounds this process.
    let plain = sized_object(10);
    let mut annotated = sized_object(10);
    annotated
        .metadata
        .insert("dag-id".to_string(), "example".to_string());
    annotated.content_type = Some("text/x-python".to_string());
    assert_eq!(
        annotated.retained_bytes(),
        plain.retained_bytes() + "dag-id".len() + "example".len() + "text/x-python".len(),
        "an object's retained footprint must include its metadata and headers, got {} \
         against a bare {}",
        annotated.retained_bytes(),
        plain.retained_bytes()
    );
    assert_eq!(
        annotated.metadata_bytes(),
        "dag-id".len() + "example".len(),
        "the metadata size must be its keys and values, got {}",
        annotated.metadata_bytes()
    );

    // The published metadata ceiling, so it is tested at the bound and one past it:
    // "Within the PUT request header, the user-defined metadata is limited to 2 KB in
    // size" (Amazon S3 User Guide, "Working with object metadata"). The key `k` is one
    // byte of that budget, so the value carries the rest.
    for (case, metadata_bytes, expected_status) in [
        ("well inside the metadata limit", 16, 200),
        (
            "exactly at the published metadata limit",
            MAX_OBJECT_METADATA_BYTES,
            200,
        ),
        (
            "one byte past the published metadata limit",
            MAX_OBJECT_METADATA_BYTES + 1,
            400,
        ),
    ] {
        let value = "v".repeat(metadata_bytes - 1);
        let key = format!("dags/metadata-{metadata_bytes}.py");
        let answer = put_object(
            &router,
            "key-bound-bucket",
            &key,
            b"x",
            &[("x-amz-meta-k", value.as_str())],
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status} for {metadata_bytes} bytes of \
             metadata, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let read_back = get_object(&router, "key-bound-bucket", &key, "", &[]).await;
        let expected_read = if expected_status == 200 { 200 } else { 404 };
        assert_eq!(
            read_back.status, expected_read,
            "case {case:?}: a refused write must store nothing and an accepted one must \
             read back, got GetObject {}",
            read_back.status
        );
    }

    // No ceiling of the BUCKET's own bounds how many objects it holds — "you can store any
    // number of objects in a bucket" — so a bucket is seeded past every count this emulator
    // ever kept per bucket and still takes a write over the wire. Seeded through the store
    // because the point is the COUNT, not the dispatch.
    const KEYS_PAST_EVERY_FORMER_CEILING: usize = 1_200;
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        for index in 0..KEYS_PAST_EVERY_FORMER_CEILING {
            let outcome = store.put_object(
                "count-bound-bucket",
                &format!("filler-{index}"),
                one_byte_object(),
            );
            assert_eq!(
                outcome,
                Some(ObjectWrite::Created),
                "case {index}: a bucket takes any number of objects, so object {index} \
                 must be recorded, got {outcome:?}"
            );
        }
        assert_eq!(
            store.put_object("count-bound-bucket", "filler-0", one_byte_object()),
            Some(ObjectWrite::Replaced),
            "a key the bucket already holds is a replacement"
        );
        // And a name that is not a bucket is refused rather than created.
        assert_eq!(
            store.put_object("not-a-bucket", "k", one_byte_object()),
            None,
            "a write naming a bucket this emulator does not hold must record nothing"
        );
    }
    for (case, key) in [
        ("a fresh key past every former ceiling", "over-the-ceiling"),
        ("a deep fresh key", "deep/path/one-more.bin"),
    ] {
        let answer = put_object(&router, "count-bound-bucket", key, b"x", &[]).await;
        assert_eq!(
            answer.status, 200,
            "case {case:?}: a bucket holding {KEYS_PAST_EVERY_FORMER_CEILING} objects must \
             still serve a write, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        let read_back = get_object(&router, "count-bound-bucket", key, "", &[]).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the write must read back, got GetObject {}",
            read_back.status
        );
    }

    // The other dimension, and the same rule: S3 bounds what one bucket HOLDS not at all,
    // so objects summing well past every per-bucket budget this emulator ever kept are all
    // recorded in one bucket, whatever each of them retains.
    let chunk = 24 * 1024 * 1024;
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        for index in 0..4 {
            let outcome = store.put_object(
                "byte-bound-bucket",
                &format!("chunk-{index}"),
                object_retaining(chunk),
            );
            assert_eq!(
                outcome,
                Some(ObjectWrite::Created),
                "case {index}: a bucket holding {} bytes must still record a {chunk}-byte \
                 object, got {outcome:?}",
                index * chunk
            );
        }
        // Replacing an object releases everything it retained, so what the bucket holds
        // does not grow with a same-size replacement.
        assert_eq!(
            store.put_object("byte-bound-bucket", "chunk-0", object_retaining(chunk)),
            Some(ObjectWrite::Replaced),
            "a replacement that retains the same bytes must be recorded as a replacement"
        );
        // An object with NO payload still retains its strings, and is recorded like any
        // other: nothing about the footprint is a ceiling.
        let mut empty_with_metadata = sized_object(0);
        empty_with_metadata
            .metadata
            .insert("k".to_string(), "v".repeat(64));
        let outcome = store.put_object(
            "byte-bound-bucket",
            "empty-with-metadata",
            empty_with_metadata,
        );
        assert_eq!(
            outcome,
            Some(ObjectWrite::Created),
            "case \"empty body with metadata\": a payload-free object must be recorded, got \
             {outcome:?}"
        );
    }
    let past_bytes = put_object(&router, "byte-bound-bucket", "over-the-budget", b"x", &[]).await;
    assert_eq!(
        past_bytes.status, 200,
        "case \"a bucket holding {} bytes\": the write must be served, got {} with body {:?}",
        4 * chunk,
        past_bytes.status,
        text(&past_bytes)
    );
    let stored = get_object(&router, "byte-bound-bucket", "over-the-budget", "", &[]).await;
    assert_eq!(
        stored.status, 200,
        "case \"a bucket holding {} bytes\": the write must read back, got GetObject {}",
        4 * chunk,
        stored.status
    );
}

#[tokio::test]
async fn the_object_store_bounds_this_process_rather_than_a_bucket_or_an_account() {
    // What bounds the store is this PROCESS's memory and nothing the API announces: AWS
    // publishes no quota on what one bucket or one account holds, so there is ONE pair of
    // whole-store figures and they are sized far above the traffic the API admits. Both
    // halves of that are asserted here — the figures, and that writes spread over several
    // buckets, summing past every capacity figure this emulator ever kept, are all
    // recorded. Seeded through the store, which is the SAME one the router hands the
    // service.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));

    // An ordinary session of uploads leaves hundreds of MiB and thousands of versions in
    // the store, so the guard has to be ABOVE that to be a memory bound rather than a
    // quota a client meets.
    let byte_budget = MAX_RETAINED_BYTES_TOTAL;
    let version_budget = MAX_STORED_VERSIONS_TOTAL;
    assert!(
        byte_budget >= 512 * 1024 * 1024,
        "case \"the store's byte budget\": it must take hundreds of MiB of uploads, got \
         {byte_budget}"
    );
    assert!(
        version_budget >= 100_000,
        "case \"the store's version budget\": it must take tens of thousands of writes, \
         got {version_budget}"
    );

    // Six buckets, each holding a 24 MiB object: 144 MiB across the store, which is more
    // than this emulator ever kept in total, let alone per bucket.
    let chunk = 24 * 1024 * 1024;
    let buckets_to_fill = 6;
    for index in 0..=buckets_to_fill {
        bucket(&router, &format!("total-bound-{index}")).await;
    }

    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        for index in 0..buckets_to_fill {
            let outcome = store.put_object(
                &format!("total-bound-{index}"),
                "payload.bin",
                object_retaining(chunk),
            );
            assert_eq!(
                outcome,
                Some(ObjectWrite::Created),
                "case {index}: a store already holding {} bytes must record a {chunk}-byte \
                 object, got {outcome:?}",
                index * chunk
            );
        }
        // With 144 MiB across the store, one more byte is still recorded — in the empty
        // bucket and in a bucket that already holds its own object.
        for name in [
            format!("total-bound-{buckets_to_fill}"),
            "total-bound-0".to_string(),
        ] {
            let outcome = store.put_object(&name, "one-byte-more", sized_object(1));
            assert_eq!(
                outcome,
                Some(ObjectWrite::Created),
                "case {name:?}: a write into a store holding {} bytes must be recorded, got \
                 {outcome:?}",
                buckets_to_fill * chunk
            );
        }
        // Replacing an object releases everything it retained, so what the store holds
        // does not grow with a same-size replacement.
        assert_eq!(
            store.put_object("total-bound-0", "payload.bin", object_retaining(chunk)),
            Some(ObjectWrite::Replaced),
            "a replacement that retains the same bytes must be recorded as a replacement"
        );
    }

    // And over the wire: a fresh key in the empty bucket is served and reads back.
    let empty_bucket = format!("total-bound-{buckets_to_fill}");
    let stored = put_object(&router, &empty_bucket, "dags/dag.py", b"# dag", &[]).await;
    assert_eq!(
        stored.status, 200,
        "case \"a store holding {} bytes\": the write must be served, got {} with body {:?}",
        buckets_to_fill * chunk,
        stored.status,
        text(&stored)
    );
    let read_back = get_object(&router, &empty_bucket, "dags/dag.py", "", &[]).await;
    assert_eq!(
        read_back.body.as_ref(),
        b"# dag",
        "case \"a store holding {} bytes\": the write must read back byte-exact, got {:?}",
        buckets_to_fill * chunk,
        text(&read_back)
    );

    // The other dimension, over many buckets: thousands of one-byte objects spread over
    // several buckets are all recorded, because no count anywhere — per key, per bucket or
    // per account — is a ceiling a client can meet.
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    let buckets_needed = 4;
    let objects_each = 1_200;
    for index in 0..=buckets_needed {
        bucket(&router, &format!("count-total-{index}")).await;
    }
    {
        let state = engine.state::<ComAmazonawsS3Amazons3State>();
        let mut store = state.lock();
        for bucket_index in 0..buckets_needed {
            for object_index in 0..objects_each {
                let outcome = store.put_object(
                    &format!("count-total-{bucket_index}"),
                    &format!("filler-{object_index}"),
                    one_byte_object(),
                );
                assert_eq!(
                    outcome,
                    Some(ObjectWrite::Created),
                    "case ({bucket_index}, {object_index}): every one of \
                     {objects_each} objects in each of {buckets_needed} buckets must be \
                     recorded, got {outcome:?}"
                );
            }
        }
        let outcome = store.put_object(
            &format!("count-total-{buckets_needed}"),
            "one-more",
            one_byte_object(),
        );
        assert_eq!(
            outcome,
            Some(ObjectWrite::Created),
            "a fresh key with {} objects already in the store must be recorded, got \
             {outcome:?}",
            buckets_needed * objects_each
        );
        assert_eq!(
            store.put_object("count-total-0", "filler-0", one_byte_object()),
            Some(ObjectWrite::Replaced),
            "a key a bucket already holds is a replacement"
        );
    }
    let past_total = put_object(
        &router,
        &format!("count-total-{buckets_needed}"),
        "dags/dag.py",
        b"x",
        &[],
    )
    .await;
    assert_eq!(
        past_total.status, 200,
        "case \"a store holding {} objects\": the write must be served, got {} with body \
         {:?}",
        buckets_needed * objects_each,
        past_total.status,
        text(&past_total)
    );
}

#[tokio::test]
async fn objects_and_tag_sets_are_held_per_bucket() {
    let router = router();

    // Two buckets, the same object key and the same tag key in each: a write to one must
    // not be visible through the other — the provisioning sequence a client runs (create
    // the bucket, tag it, upload the DAG) over two buckets at once.
    let sides = [
        ("bucket-left", &b"left bytes"[..], "left"),
        ("bucket-right", &b"right bytes"[..], "right"),
    ];
    for (name, payload, side) in sides {
        bucket(&router, name).await;
        let put = put_object(&router, name, "dags/dag.py", payload, &[]).await;
        assert_eq!(
            put.status, 200,
            "case {name:?}: expected HTTP 200 from PutObject, got {} with body {:?}",
            put.status,
            text(&put)
        );
        let tagged = put_bucket_tagging(&router, name, &[tag("side", side)]).await;
        assert_eq!(
            tagged.status, 204,
            "case {name:?}: expected HTTP 204 from PutBucketTagging, got {} with body {:?}",
            tagged.status,
            text(&tagged)
        );
    }

    for (name, payload, side) in sides {
        let got = get_object(&router, name, "dags/dag.py", "", &[]).await;
        assert_eq!(
            got.body.as_ref(),
            payload,
            "case {name:?}: each bucket holds its own object at that key, got {:?}",
            text(&got)
        );
        let tags = tag_set_of(&get_bucket_tagging(&router, name).await);
        assert_eq!(
            tags,
            vec![tag("side", side)],
            "case {name:?}: each bucket holds its own tag set, got {tags:?}"
        );
    }
}
