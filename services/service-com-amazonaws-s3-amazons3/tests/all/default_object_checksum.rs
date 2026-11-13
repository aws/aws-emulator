// SPDX-License-Identifier: CC0-1.0

//! The checksum an object carries when its write named NO algorithm.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a client sees: the `x-amz-checksum-*` headers of a write and of a read,
//! the `Checksum` container of a `GetObjectAttributes` answer and the `ChecksumAlgorithm`
//! of a listed object.
//!
//! The rule under test, from the model: a `Checksum*` member "is present if the object was
//! uploaded with the `CRC64NVME` checksum algorithm, or if it was uploaded without a
//! checksum (and Amazon S3 added the default checksum, `CRC64NVME`, to the uploaded
//! object)" (`PutObjectOutput$ChecksumCRC64NVME`). So EVERY stored object carries a
//! whole-object checksum — the algorithm the write named, else `CRC64NVME` — whose value is
//! the digest of the bytes that were stored, and every surface that reports a stored
//! object's checksum reports that one pair: the write itself, a read that enabled the
//! checksum mode ("to retrieve the checksum, this mode must be enabled",
//! `GetObjectRequest$ChecksumMode`) whether it is a `GET` or a `HEAD`, the object's
//! attributes, and the listing entry.
//!
//! The rows differ along what the rule generalises over: the SIZE and content of the bytes
//! stored (empty, one byte, text, byte-varied binary, several kilobytes), and HOW the write
//! names an algorithm (not at all, through `x-amz-sdk-checksum-algorithm`, and through its
//! own `x-amz-checksum-*` digest) — the named rows are what show the default applies only
//! where a write named none.
//!
//! The expected digests are computed here through `aws-smithy-checksums`, the AWS SDK's own
//! implementation of these checksums, rather than read back from the crate under test.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The algorithm S3 adds to an object whose write named none
/// (`PutObjectOutput$ChecksumCRC64NVME`).
const DEFAULT_ALGORITHM: &str = "CRC64NVME";

/// The `ChecksumType` a single whole-object write settles: "for `PutObject` uploads, the
/// checksum type is always `FULL_OBJECT`" (`PutObjectRequest$ChecksumType`).
const FULL_OBJECT: &str = "FULL_OBJECT";

/// Every `x-amz-checksum-*` value header the model binds, so a row can assert that the one
/// algorithm its object carries is the ONLY one reported
/// (`smithy_query(query="operation",operation="GetObject")` binds one per `Checksum*`
/// member).
const EVERY_CHECKSUM_HEADER: [&str; 10] = [
    "x-amz-checksum-crc32",
    "x-amz-checksum-crc32c",
    "x-amz-checksum-crc64nvme",
    "x-amz-checksum-sha1",
    "x-amz-checksum-sha256",
    "x-amz-checksum-sha512",
    "x-amz-checksum-md5",
    "x-amz-checksum-xxhash64",
    "x-amz-checksum-xxhash3",
    "x-amz-checksum-xxhash128",
];

/// The Base64 digest of `data` under the `ChecksumAlgorithm` value `algorithm`, as every
/// `Checksum*` member carries it, computed by the AWS SDK's own checksum crate.
fn digest_of(algorithm: &str, data: &[u8]) -> String {
    let parsed: aws_smithy_checksums::ChecksumAlgorithm = algorithm
        .to_ascii_lowercase()
        .parse()
        .unwrap_or_else(|error| panic!("{algorithm} must be a checksum this crate computes: {error}"));
    let mut checksum = parsed.into_impl();
    checksum.update(data);
    aws_smithy_types::base64::encode(checksum.finalize())
}

/// The response header one algorithm's value travels on: `x-amz-checksum-` followed by the
/// algorithm in lower case (`ChecksumCRC64NVME -> x-amz-checksum-crc64nvme`).
fn header_of(algorithm: &str) -> String {
    format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase())
}

/// How a row's write names its algorithm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Names {
    /// Nothing at all, so the object carries the default.
    Nothing,
    /// `x-amz-sdk-checksum-algorithm`, the member that asks S3 to compute the digest.
    TheAlgorithm,
    /// The write's own `x-amz-checksum-<algorithm>` digest of the same bytes.
    ItsOwnDigest,
}

/// `length` bytes spanning the whole 0..=255 range from a fixed seed — a 32-bit LCG (the
/// "ANSI C" multiplier and increment), taking the high byte of each step — so a row's
/// payload is byte-varied without anything random.
fn seeded(seed: u32, length: usize) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 24) as u8
        })
        .collect()
}

/// `HEAD /<bucket>/<key>` — `HeadObject`, with the headers the case carries.
async fn head_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(router, "HEAD", &format!("/{bucket}/{key}"), headers, "").await
}

/// `GET /<bucket>/<key>?attributes` — `GetObjectAttributes`, asking for the `Checksum`
/// container.
async fn object_checksum_attribute(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{bucket}/{key}?attributes"),
        &[("x-amz-object-attributes", "Checksum")],
        b"",
    )
    .await
}

/// `GET /<bucket>?list-type=2&prefix=<key>` — `ListObjectsV2` narrowed to one key.
async fn list_one(router: &MultiServiceRouter, bucket: &str, key: &str) -> Answer {
    send(
        router,
        "GET",
        &format!("/{bucket}?list-type=2&prefix={key}"),
        &[],
        "",
    )
    .await
}

/// The text between `<element>` and `</element>` in `body`, or `None` when it carries no
/// such element.
fn element(body: &str, element: &str) -> Option<String> {
    body.split_once(&format!("<{element}>"))
        .and_then(|(_, tail)| tail.split_once(&format!("</{element}>")))
        .map(|(value, _)| value.to_string())
}

/// Assert that `answer` reports `algorithm`'s `expected` value and NO other algorithm's.
fn reports_only(answer: &Answer, case: &str, surface: &str, algorithm: &str, expected: &str) {
    let carried = header_of(algorithm);
    assert_eq!(
        answer.header(&carried).as_deref(),
        Some(expected),
        "case {case:?}/{surface}: expected {carried} = {expected:?}, got {:?}",
        answer.header(&carried)
    );
    assert_eq!(
        answer.header("x-amz-checksum-type").as_deref(),
        Some(FULL_OBJECT),
        "case {case:?}/{surface}: a one-shot write's checksum is over the whole object, got \
         {:?}",
        answer.header("x-amz-checksum-type")
    );
    for other in EVERY_CHECKSUM_HEADER {
        if other == carried {
            continue;
        }
        assert_eq!(
            answer.header(other),
            None,
            "case {case:?}/{surface}: an object carries the checksum of ONE algorithm, so \
             {other} must be absent, got {:?}",
            answer.header(other)
        );
    }
}

#[tokio::test]
async fn every_stored_object_carries_a_whole_object_checksum_on_every_surface() {
    let router = router();
    let name = "default-checksum-bucket";
    bucket(&router, name).await;

    let varied = seeded(0x5eed_1234, 4_096);
    let long = "a line of stored text\n".repeat(256).into_bytes();
    // (case, body, the algorithm the object must carry, how the write names it).
    let rows: Vec<(&str, Vec<u8>, &str, Names)> = vec![
        (
            "an empty object written with no algorithm",
            Vec::new(),
            DEFAULT_ALGORITHM,
            Names::Nothing,
        ),
        (
            "a one-byte object written with no algorithm",
            b"x".to_vec(),
            DEFAULT_ALGORITHM,
            Names::Nothing,
        ),
        (
            "a text object written with no algorithm",
            b"a,b\n1,2\n".to_vec(),
            DEFAULT_ALGORITHM,
            Names::Nothing,
        ),
        (
            "byte-varied binary written with no algorithm",
            varied,
            DEFAULT_ALGORITHM,
            Names::Nothing,
        ),
        (
            "several kilobytes of text written with no algorithm",
            long,
            DEFAULT_ALGORITHM,
            Names::Nothing,
        ),
        (
            "an object whose write asked for a CRC",
            b"the bytes a named algorithm covers".to_vec(),
            "CRC32",
            Names::TheAlgorithm,
        ),
        (
            "an object whose write asked for the default by name",
            b"the bytes the default algorithm covers".to_vec(),
            DEFAULT_ALGORITHM,
            Names::TheAlgorithm,
        ),
        (
            "an object whose write carried its own digest",
            b"the bytes a client digested itself".to_vec(),
            "SHA256",
            Names::ItsOwnDigest,
        ),
    ];

    for (index, (case, body, algorithm, names)) in rows.iter().enumerate() {
        let key = format!("stored/object-{index}.bin");
        let expected = digest_of(algorithm, body);
        let own_header = header_of(algorithm);
        let headers: Vec<(&str, &str)> = match names {
            Names::Nothing => Vec::new(),
            Names::TheAlgorithm => vec![("x-amz-sdk-checksum-algorithm", algorithm)],
            Names::ItsOwnDigest => vec![(own_header.as_str(), expected.as_str())],
        };

        // 1. The write reports what it stored.
        let stored = put_object(&router, name, &key, body, &headers).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        reports_only(&stored, case, "PutObject", algorithm, &expected);

        // 2. A read that enables the checksum mode reports the same pair, whether it is a
        //    GET or a HEAD of the same object.
        let enabled = [("x-amz-checksum-mode", "ENABLED")];
        let read = get_object(&router, name, &key, "", &enabled).await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: the read must be served, got {} with body {:?}",
            read.status,
            text(&read)
        );
        reports_only(&read, case, "GetObject", algorithm, &expected);
        assert_eq!(
            read.body.as_ref(),
            body.as_slice(),
            "case {case:?}: the read must carry the stored bytes, got {} of them",
            read.body.len()
        );
        let headed = head_object(&router, name, &key, &enabled).await;
        reports_only(&headed, case, "HeadObject", algorithm, &expected);

        // 3. And a read that does NOT enable the mode reports none of it: "to retrieve the
        //    checksum, this mode must be enabled".
        let unasked = get_object(&router, name, &key, "", &[]).await;
        for header in EVERY_CHECKSUM_HEADER {
            assert_eq!(
                unasked.header(header),
                None,
                "case {case:?}: a read that enabled no checksum mode reports none, got \
                 {header} = {:?}",
                unasked.header(header)
            );
        }

        // 4. The object's ATTRIBUTES report the same value and type, off the same record.
        let attribute = object_checksum_attribute(&router, name, &key).await;
        let document = text(&attribute);
        let member = format!("Checksum{algorithm}");
        assert_eq!(
            element(&document, &member).as_deref(),
            Some(expected.as_str()),
            "case {case:?}/GetObjectAttributes: expected <{member}> {expected:?}, got body \
             {document:?}"
        );
        assert_eq!(
            element(&document, "ChecksumType").as_deref(),
            Some(FULL_OBJECT),
            "case {case:?}/GetObjectAttributes: expected <ChecksumType> {FULL_OBJECT:?}, got \
             body {document:?}"
        );

        // 5. And the LISTING names the algorithm whose member carries that value, read off
        //    the same record.
        let listed = list_one(&router, name, &key).await;
        let listing = text(&listed);
        assert_eq!(
            element(&listing, "ChecksumAlgorithm").as_deref(),
            Some(*algorithm),
            "case {case:?}/ListObjectsV2: expected <ChecksumAlgorithm> {algorithm:?}, got \
             body {listing:?}"
        );
        assert_eq!(
            element(&listing, "ChecksumType").as_deref(),
            Some(FULL_OBJECT),
            "case {case:?}/ListObjectsV2: expected <ChecksumType> {FULL_OBJECT:?}, got body \
             {listing:?}"
        );
    }
}

#[tokio::test]
async fn a_second_write_of_one_key_carries_the_checksum_of_its_own_bytes() {
    let router = router();
    let name = "default-checksum-replacement-bucket";
    bucket(&router, name).await;
    let key = "replaced.bin";

    // A write REPLACES the object, so the checksum a read reports after it is the digest of
    // the bytes THAT write stored — under the default both times, since neither names an
    // algorithm.
    for (case, body) in [
        ("the first revision", b"first revision bytes".as_slice()),
        ("a longer second revision", b"second revision, rather longer".as_slice()),
        ("an empty third revision", b"".as_slice()),
    ] {
        let stored = put_object(&router, name, key, body, &[]).await;
        assert_eq!(
            stored.status, 200,
            "case {case:?}: the write must succeed, got {} with body {:?}",
            stored.status,
            text(&stored)
        );
        let expected = digest_of(DEFAULT_ALGORITHM, body);
        reports_only(&stored, case, "PutObject", DEFAULT_ALGORITHM, &expected);
        let read = get_object(
            &router,
            name,
            key,
            "",
            &[("x-amz-checksum-mode", "ENABLED")],
        )
        .await;
        reports_only(&read, case, "GetObject", DEFAULT_ALGORITHM, &expected);
    }
}
