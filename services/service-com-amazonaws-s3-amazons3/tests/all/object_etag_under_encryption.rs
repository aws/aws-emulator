// SPDX-License-Identifier: CC0-1.0

//! WHAT an object's entity tag is, as the encryption it is stored under decides it.
//!
//! RULE: an object's `ETag` is the MD5 digest of its bytes only where Amazon S3 holds those
//! bytes under its own key — "objects created by the PUT Object, POST Object, or Copy
//! operation, or through the Amazon Web Services Management Console, and are encrypted by
//! SSE-S3 or plaintext, have ETags that are an MD5 digest of their object data. Objects
//! created … that are encrypted by SSE-C or SSE-KMS, have ETags that are not an MD5 digest of
//! their object data" (Amazon S3 API Reference, *Common Response Headers*, `ETag`). So an
//! object sealed with a KMS key or with a key the caller supplied carries a tag that is NOT
//! that digest, while remaining an entity tag in every other respect: a client that computes
//! the MD5 of what it uploaded and compares cannot be told the two agree when the stored
//! representation is not the plaintext.
//!
//! The rows differ along what the rule generalises over: WHICH encryption the object is stored
//! under (none at all, SSE-S3, each of the two KMS algorithms, a KMS write naming no key of
//! its own — so the Amazon Web Services managed key for this service — and a key the caller
//! supplied), and WHICH write stored it (`PutObject`, `CopyObject`, and a multipart upload,
//! whose tag carries the `-<parts>` suffix either way). Every row is then read back through
//! both implemented readers, because a tag is settled by the write and reported unchanged
//! afterwards.
//!
//! Two further properties of the tag, asserted over the same rows: it still changes with the
//! bytes (two bodies under one encryption never share a tag), and it is of the key as well as
//! of the bytes (one body sealed under two different keys does not), which is what makes it a
//! tag of the stored representation rather than a constant standing in for one.
//!
//! Every assertion drives a real HTTP request through the router holding S3 beside KMS over
//! one shared engine, so what is pinned is the wire answer a client sees.

use ::base64::Engine as _;
use ::md5::Digest as _;
use service_com_amazonaws_s3_amazons3::state::{multipart_object_etag, object_etag};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{send, send_bytes, Answer};
use super::kms_key_references::{
    copy_object, create_multipart_upload, created_key, described_key_arn, router_with_kms,
};
use super::multipart_upload_completion::completion_payload;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The header `ServerSideEncryption` binds to
/// (`smithy_query(query="operation",operation="PutObject")`).
const ALGORITHM_HEADER: &str = "x-amz-server-side-encryption";

/// The header `SSEKMSKeyId` binds to.
const KEY_ID_HEADER: &str = "x-amz-server-side-encryption-aws-kms-key-id";

/// The three headers a write supplying its OWN key carries: `SSECustomerAlgorithm`,
/// `SSECustomerKey` and `SSECustomerKeyMD5`.
const CUSTOMER_ALGORITHM_HEADER: &str = "x-amz-server-side-encryption-customer-algorithm";
/// See [`CUSTOMER_ALGORITHM_HEADER`].
const CUSTOMER_KEY_HEADER: &str = "x-amz-server-side-encryption-customer-key";
/// See [`CUSTOMER_ALGORITHM_HEADER`].
const CUSTOMER_KEY_MD5_HEADER: &str = "x-amz-server-side-encryption-customer-key-md5";

/// The one `ServerSideEncryption` value whose key is Amazon S3's own
/// (`smithy_query(query="shape",shape="ServerSideEncryption")` lists it beside the two KMS
/// ones), and the one `SSECustomerAlgorithm` value the model declares.
const AES256: &str = "AES256";

/// The two `ServerSideEncryption` values a KMS key belongs to.
const KMS: &str = "aws:kms";
/// See [`KMS`].
const DSSE_KMS: &str = "aws:kms:dsse";

/// The bytes every row writes.
const BODY: &[u8] = b"id,name\n1,alice\n2,bob\n";

/// A SECOND payload, so a row can assert that the tag still changes with the bytes.
const OTHER_BODY: &[u8] = b"id,name\n1,alice\n2,bob\n3,carol\n";

/// A 256-bit key a caller supplies, Base64 encoded as `SSECustomerKey` takes it, derived from
/// `seed` so that two rows can present two different keys.
fn customer_key(seed: u8) -> String {
    let material: Vec<u8> = (0..32_u8)
        .map(|index| index.wrapping_mul(29).wrapping_add(seed))
        .collect();
    ::base64::engine::general_purpose::STANDARD.encode(&material)
}

/// The Base64 MD5 of the key `key` carries, as `SSECustomerKeyMD5` states it: "the 128-bit MD5
/// digest of the encryption key according to RFC 1321"
/// (`PutObjectRequest$SSECustomerKeyMD5`), computed from the key's own bytes.
fn key_digest(key: &str) -> String {
    let material = ::base64::engine::general_purpose::STANDARD
        .decode(key)
        .expect("the test key must be base64");
    ::base64::engine::general_purpose::STANDARD.encode(::md5::Md5::digest(&material))
}

/// The three headers a request presenting `key` carries.
fn presenting(key: &str) -> Vec<(&'static str, String)> {
    vec![
        (CUSTOMER_ALGORITHM_HEADER, AES256.to_string()),
        (CUSTOMER_KEY_HEADER, key.to_string()),
        (CUSTOMER_KEY_MD5_HEADER, key_digest(key)),
    ]
}

/// `PUT /<bucket>/<key>?partNumber=1&uploadId=<id>` — `UploadPart`.
async fn upload_part(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    body: &[u8],
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}?partNumber=1&uploadId={upload_id}&x-id=UploadPart"),
        headers,
        body,
    )
    .await
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`.
async fn complete(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    upload_id: &str,
    parts: &[(i32, String)],
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploadId={upload_id}"),
        headers,
        &completion_payload(parts),
    )
    .await
}

/// `HEAD /<bucket>/<key>` carrying `headers` — a read of an object sealed with the caller's
/// own key has to present that key again (`HeadObjectRequest$SSECustomerKey`).
async fn head_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "HEAD",
        &format!("/{bucket_name}/{key}"),
        headers,
        b"",
    )
    .await
}

/// The entity tag `answer` reports in its `ETag` header, with the operation named when it
/// reports none.
fn reported_etag(case: &str, operation: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status / 100,
        2,
        "case {case:?}: {operation} must be served, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    answer.header("etag").unwrap_or_else(|| {
        panic!(
            "case {case:?}: {operation} must report an ETag, got status {} and body {:?}",
            answer.status,
            text(answer)
        )
    })
}

/// The entity tag `answer` reports in the `<ETag>` of its result document — where a copy and a
/// completion carry the object's tag.
fn document_etag(case: &str, operation: &str, answer: &Answer) -> String {
    assert_eq!(
        answer.status / 100,
        2,
        "case {case:?}: {operation} must be served, got {} with body {:?}",
        answer.status,
        text(answer)
    );
    answer.element("ETag").unwrap_or_else(|| {
        panic!(
            "case {case:?}: {operation} must report the object's ETag, got {:?}",
            text(answer)
        )
    })
}

/// Assert `etag` is shaped like one Amazon S3 reports: the digest in double quotes, 32
/// lowercase hexadecimal digits, with the `-<parts>` suffix exactly when the object came of a
/// multipart upload.
fn assert_etag_shape(case: &str, operation: &str, etag: &str, parts: Option<usize>) {
    let value = etag
        .strip_prefix('"')
        .and_then(|inner| inner.strip_suffix('"'))
        .unwrap_or_else(|| {
            panic!("case {case:?}: {operation} must report a quoted ETag, got {etag:?}")
        });
    let (digest, suffix) = match value.split_once('-') {
        Some((digest, suffix)) => (digest, Some(suffix)),
        None => (value, None),
    };
    let expected = parts.map(|count| count.to_string());
    assert_eq!(
        suffix,
        expected.as_deref(),
        "case {case:?}: {operation} must report the part-count suffix {expected:?}, got {etag:?}"
    );
    assert!(
        digest.len() == 32
            && digest
                .chars()
                .all(|digit| digit.is_ascii_hexdigit() && !digit.is_ascii_uppercase()),
        "case {case:?}: {operation} must report a 32-digit lowercase hexadecimal digest, got \
         {etag:?}"
    );
}

/// Assert the relation between `etag` and the digest of the object's own bytes that `sealed`
/// states: equal for an object Amazon S3 holds under its own key, and different for one sealed
/// with a KMS key or with the caller's.
fn assert_digest_relation(case: &str, operation: &str, etag: &str, plaintext: &str, sealed: bool) {
    if sealed {
        assert_ne!(
            etag, plaintext,
            "case {case:?}: {operation} must NOT report the MD5 digest of the object's data \
             {plaintext:?} for a sealed object, got {etag:?}"
        );
    } else {
        assert_eq!(
            etag, plaintext,
            "case {case:?}: {operation} must report the MD5 digest of the object's data \
             {plaintext:?}, got {etag:?}"
        );
    }
}

/// One row of the table: the case, the headers a write carries, and whether the encryption
/// those headers ask for SEALS the object — the two halves of the published rule.
struct Row {
    case: &'static str,
    headers: Vec<(&'static str, String)>,
    sealed: bool,
}

impl Row {
    /// The headers a WRITE carries, as the request helpers take them.
    fn write_headers(&self) -> Vec<(&'static str, &str)> {
        self.headers
            .iter()
            .map(|(name, value)| (*name, value.as_str()))
            .collect()
    }

    /// The headers a READ of the stored object carries: the caller's own key where the object
    /// was sealed with one, and nothing at all otherwise — the KMS and SSE-S3 members are a
    /// write's, and a read of such an object presents none of them.
    fn read_headers(&self) -> Vec<(&'static str, &str)> {
        self.write_headers()
            .into_iter()
            .filter(|(name, _)| {
                [
                    CUSTOMER_ALGORITHM_HEADER,
                    CUSTOMER_KEY_HEADER,
                    CUSTOMER_KEY_MD5_HEADER,
                ]
                .contains(name)
            })
            .collect()
    }
}

/// The rows the tables below walk, built against the keys KMS holds for this router.
async fn rows(router: &MultiServiceRouter) -> Vec<Row> {
    let (key_id, key_arn) = created_key(router, "the key these writes name").await;
    // The predefined Amazon Web Services alias of this service carries no key until something
    // asks for it; asking here is what gives the row below a key to resolve.
    let _managed = described_key_arn(router, "alias/aws/s3").await;
    vec![
        Row {
            case: "no encryption member at all",
            headers: Vec::new(),
            sealed: false,
        },
        Row {
            case: "SSE-S3, whose key is Amazon S3's own",
            headers: vec![(ALGORITHM_HEADER, AES256.to_string())],
            sealed: false,
        },
        Row {
            case: "SSE-KMS naming a key ARN",
            headers: vec![
                (ALGORITHM_HEADER, KMS.to_string()),
                (KEY_ID_HEADER, key_arn),
            ],
            sealed: true,
        },
        Row {
            case: "dual-layer SSE-KMS naming a bare key id",
            headers: vec![
                (ALGORITHM_HEADER, DSSE_KMS.to_string()),
                (KEY_ID_HEADER, key_id),
            ],
            sealed: true,
        },
        Row {
            case: "SSE-KMS naming no key, so the Amazon Web Services managed key",
            headers: vec![(ALGORITHM_HEADER, KMS.to_string())],
            sealed: true,
        },
        Row {
            case: "a key the caller supplied",
            headers: presenting(&customer_key(7)),
            sealed: true,
        },
    ]
}

#[tokio::test]
async fn a_written_object_reports_the_digest_of_its_bytes_only_where_s3_holds_the_plaintext() {
    let router = router_with_kms();
    let plaintext = object_etag(BODY);

    for (index, row) in rows(&router).await.iter().enumerate() {
        let case = row.case;
        let name = format!("etag-write-{index}");
        bucket(&router, &name).await;
        let headers = row.write_headers();
        let reads = row.read_headers();

        let written = put_object(&router, &name, "put.bin", BODY, &headers).await;
        let etag = reported_etag(case, "PutObject", &written);
        assert_etag_shape(case, "PutObject", &etag, None);
        assert_digest_relation(case, "PutObject", &etag, &plaintext, row.sealed);

        // A tag is settled by the write and reported unchanged by every reader of the object.
        let read = get_object(&router, &name, "put.bin", "", &reads).await;
        assert_eq!(
            reported_etag(case, "GetObject", &read),
            etag,
            "case {case:?}: GetObject must report the tag the write recorded {etag:?}"
        );
        assert_eq!(
            read.body.as_ref(),
            BODY,
            "case {case:?}: the stored bytes must come back unchanged"
        );
        let headed = head_object(&router, &name, "put.bin", &reads).await;
        assert_eq!(
            reported_etag(case, "HeadObject", &headed),
            etag,
            "case {case:?}: HeadObject must report the tag the write recorded {etag:?}"
        );

        // ...and it is a tag of the BYTES as much as of the key: another payload under the
        // same encryption carries another tag.
        let other = put_object(&router, &name, "other.bin", OTHER_BODY, &headers).await;
        let other_etag = reported_etag(case, "PutObject of another payload", &other);
        assert_ne!(
            other_etag, etag,
            "case {case:?}: two payloads under one encryption must not share a tag, got \
             {etag:?} twice"
        );
        assert_digest_relation(
            case,
            "PutObject of another payload",
            &other_etag,
            &object_etag(OTHER_BODY),
            row.sealed,
        );
    }
}

#[tokio::test]
async fn a_copy_reports_the_tag_of_the_encryption_it_stored_the_destination_under() {
    let router = router_with_kms();
    let plaintext = object_etag(BODY);
    // The source is held in the clear, so what decides the destination's tag is the
    // destination's own encryption rather than anything about the source.
    bucket(&router, "etag-copy-source").await;
    let source = put_object(&router, "etag-copy-source", "source.bin", BODY, &[]).await;
    assert_eq!(
        reported_etag("the copy source", "PutObject", &source),
        plaintext,
        "the source is held in the clear, so its tag is the digest of its bytes"
    );

    for (index, row) in rows(&router).await.iter().enumerate() {
        let case = row.case;
        // A copy whose DESTINATION is sealed with a key the caller supplies is one this
        // emulator does not serve yet — it answers 501 for
        // `x-amz-server-side-encryption-customer-algorithm` on `CopyObject` — so the rule is
        // asserted over that encryption through the write and the multipart tables instead.
        if !row.read_headers().is_empty() {
            continue;
        }
        let name = format!("etag-copy-{index}");
        bucket(&router, &name).await;
        let copied = copy_object(
            &router,
            &name,
            "copied.bin",
            "etag-copy-source/source.bin",
            &row.write_headers(),
        )
        .await;
        let etag = document_etag(case, "CopyObject", &copied);
        assert_etag_shape(case, "CopyObject", &etag, None);
        assert_digest_relation(case, "CopyObject", &etag, &plaintext, row.sealed);
        let read = get_object(&router, &name, "copied.bin", "", &row.read_headers()).await;
        assert_eq!(
            reported_etag(case, "GetObject of the copy", &read),
            etag,
            "case {case:?}: a read of the copy must report the tag the copy recorded {etag:?}"
        );
    }
}

#[tokio::test]
async fn a_multipart_object_keeps_its_part_count_suffix_whatever_seals_it() {
    let router = router_with_kms();
    // "Amazon S3 concatenates the bytes for the MD5 digests together and then calculates the
    // MD5 digest of these concatenated values", then "appends a dash with the total number of
    // parts": the tag a one-part upload carries in the clear.
    let plaintext = multipart_object_etag(&[BODY]);

    for (index, row) in rows(&router).await.iter().enumerate() {
        let case = row.case;
        let name = format!("etag-multipart-{index}");
        bucket(&router, &name).await;
        // The INITIATION settles the encryption of the whole upload; a write under the
        // caller's own key presents that key again on every call of the upload.
        let initiated =
            create_multipart_upload(&router, &name, "assembled.bin", &row.write_headers()).await;
        let upload_id = initiated.element("UploadId").unwrap_or_else(|| {
            panic!(
                "case {case:?}: an initiation must report an UploadId, got {:?}",
                text(&initiated)
            )
        });
        let reads = row.read_headers();
        let part =
            upload_part(&router, &name, "assembled.bin", &upload_id, BODY, &reads).await;
        let part_etag = reported_etag(case, "UploadPart", &part);
        let completed = complete(
            &router,
            &name,
            "assembled.bin",
            &upload_id,
            &[(1, part_etag)],
            &reads,
        )
        .await;
        let etag = document_etag(case, "CompleteMultipartUpload", &completed);
        assert_etag_shape(case, "CompleteMultipartUpload", &etag, Some(1));
        assert_digest_relation(
            case,
            "CompleteMultipartUpload",
            &etag,
            &plaintext,
            row.sealed,
        );
        let read = get_object(&router, &name, "assembled.bin", "", &reads).await;
        assert_eq!(
            reported_etag(case, "GetObject of the assembled object", &read),
            etag,
            "case {case:?}: a read of the assembled object must report the tag the completion \
             recorded {etag:?}"
        );
    }
}

#[tokio::test]
async fn one_payload_sealed_under_two_keys_carries_two_tags() {
    // The tag of a sealed object is of the stored representation, which the key is part of: so
    // the same bytes under two different KMS keys, and under two keys the caller supplied,
    // carry four different tags — and none of them is the digest of those bytes.
    let router = router_with_kms();
    let (_, first_arn) = created_key(&router, "the first key the payload is sealed under").await;
    let (_, second_arn) = created_key(&router, "the second key the payload is sealed under").await;
    bucket(&router, "etag-two-keys").await;
    let plaintext = object_etag(BODY);

    let first_customer = presenting(&customer_key(11));
    let second_customer = presenting(&customer_key(23));
    let cases: [(&str, Vec<(&'static str, &str)>); 4] = [
        (
            "the first KMS key",
            vec![(ALGORITHM_HEADER, KMS), (KEY_ID_HEADER, &first_arn)],
        ),
        (
            "the second KMS key",
            vec![(ALGORITHM_HEADER, KMS), (KEY_ID_HEADER, &second_arn)],
        ),
        (
            "the first key the caller supplied",
            first_customer
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect(),
        ),
        (
            "the second key the caller supplied",
            second_customer
                .iter()
                .map(|(name, value)| (*name, value.as_str()))
                .collect(),
        ),
    ];

    let mut seen: Vec<(&str, String)> = Vec::new();
    for (index, (case, headers)) in cases.iter().enumerate() {
        let key = format!("sealed-{index}.bin");
        let written = put_object(&router, "etag-two-keys", &key, BODY, headers).await;
        let etag = reported_etag(case, "PutObject", &written);
        assert_ne!(
            etag, plaintext,
            "case {case:?}: a sealed object must not report the digest of its data \
             {plaintext:?}"
        );
        for (other, other_etag) in &seen {
            assert_ne!(
                &etag, other_etag,
                "case {case:?}: the same bytes sealed under another key ({other}) must carry \
                 another tag, got {etag:?} twice"
            );
        }
        seen.push((case, etag));
    }
}
