// SPDX-License-Identifier: CC0-1.0

//! SERVER-SIDE ENCRYPTION WITH A CUSTOMER-PROVIDED KEY (SSE-C), end to end over the wire.
//!
//! Every assertion drives a real HTTP request through the router (the harness the
//! bucket-lifecycle and object suites in this crate own), so what is pinned is the wire
//! behaviour a client sees: the status, the bound response headers, the bytes served, and the
//! `<Code>` and `<Message>` of each refusal.
//!
//! The rules under test:
//!
//! 1. A write that supplies its OWN key — the `x-amz-server-side-encryption-customer-`
//!    `algorithm`, `-key` and `-key-MD5` headers — is SERVED, and what the object records of
//!    that key is the algorithm and the key's DIGEST: "Amazon S3 does not store the encryption
//!    key you provide" (Amazon S3 User Guide, *Using server-side encryption with
//!    customer-provided keys*), and the write and every later read report back the algorithm
//!    and that digest and nothing else about the key.
//! 2. The object's bytes come of that key, so a read is decided by the key it presents,
//!    against the digest the object carries: the SAME key is served the object, ANOTHER key is
//!    refused the way any caller not authorized for its plaintext is (403 `AccessDenied`), and
//!    NO key cannot be served at all (400 `InvalidRequest`). A read presenting a key for an
//!    object stored under some other form of encryption names parameters that do not apply to
//!    it.
//! 3. The three members carry ONE key, and each is checked: an algorithm S3 does not serve, a
//!    key that is not one of the length that algorithm takes, a missing or mismatched digest,
//!    and a customer key beside a request for an S3-managed algorithm are each refused — 400,
//!    nothing stored.
//! 4. A MULTIPART upload initiated under a customer key records that key's digest, so every
//!    part and the completion must present the same key ("this must be the same encryption key
//!    specified in the initiate multipart upload request", `UploadPartRequest$SSECustomerKey`),
//!    and the object the completion assembles is read with it.
//! 5. `s3:x-amz-server-side-encryption-customer-algorithm` is a condition key of the request
//!    (Amazon S3 User Guide, *Actions, resources, and condition keys for Amazon S3*), so a
//!    bucket policy written over it decides a write that supplies a customer key — and decides
//!    a multipart INITIATION the same way, because "you must be allowed to perform the
//!    `s3:PutObject` action on an object to initiate a multipart upload"
//!    (`CreateMultipartUpload`'s own documentation).
//! 6. A part COPY carries two keys: the source's, in the `x-amz-copy-source-` spelling of the
//!    three headers ("the encryption key provided in this header must be the same one that was
//!    used when the source object was created",
//!    `UploadPartCopyRequest$CopySourceSSECustomerKey`), and the upload's own — so an SSE-C
//!    source is re-keyed into an SSE-C multipart object, and each half is refused on its own
//!    when the key it needs is missing or is another one.
//!
//! The rows differ along what the rules generalise over: the key material (several distinct
//! 256-bit keys), the FORM of the object key (a plain name, `/` separators), the size of the
//! object, which member of the three does not hold, which operation presents the key (a write,
//! a read, a metadata read, an initiation, a part write, a completion), and which way round
//! the policy's condition is written (deny when the key is supplied, deny when it is not).

use ::base64::Engine as _;
use ::md5::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{put_bucket_policy, router, send, send_bytes, Answer};
use super::multipart_part_copy::upload_part_copy;
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_write_authorization::{explicit_deny_message, policy_document, statement};

/// The one algorithm S3 serves a customer-provided key under: "`AES256` is the only supported
/// value" (`PutObjectRequest$SSECustomerAlgorithm`).
const ALGORITHM: &str = "AES256";

/// The three headers one customer key travels in.
const ALGORITHM_HEADER: &str = "x-amz-server-side-encryption-customer-algorithm";
const KEY_HEADER: &str = "x-amz-server-side-encryption-customer-key";
const KEY_MD5_HEADER: &str = "x-amz-server-side-encryption-customer-key-md5";

/// The codes and sentences the refusals below carry.
const ACCESS_DENIED: &str = "AccessDenied";
const INVALID_REQUEST: &str = "InvalidRequest";
const INVALID_ARGUMENT: &str = "InvalidArgument";
const INVALID_DIGEST: &str = "InvalidDigest";

/// The sentence S3 answers a read of an SSE-C object that presents no key.
const KEY_REQUIRED_MESSAGE: &str =
    "The object was stored using a form of Server Side Encryption. The correct parameters \
     must be provided to retrieve the object.";

/// The IAM action a write — and a multipart initiation — is authorized under.
const PUT_OBJECT_ACTION: &str = "s3:PutObject";

/// A 256-bit key, base64 encoded as the `-key` header carries it, derived from `seed` so that
/// every row of this suite presents a DIFFERENT key.
fn customer_key(seed: u8) -> String {
    let material: Vec<u8> = (0..32_u8)
        .map(|index| index.wrapping_mul(37).wrapping_add(seed))
        .collect();
    ::base64::engine::general_purpose::STANDARD.encode(&material)
}

/// The base64 MD5 of the key `key` carries, as the `-key-MD5` header states it: "the 128-bit
/// MD5 digest of the encryption key according to RFC 1321"
/// (`PutObjectRequest$SSECustomerKeyMD5`), computed here from the key's own bytes.
fn key_digest(key: &str) -> String {
    let material = ::base64::engine::general_purpose::STANDARD
        .decode(key)
        .expect("the test key must be base64");
    ::base64::engine::general_purpose::STANDARD.encode(::md5::Md5::digest(&material))
}

/// The three headers a request presenting `key` carries.
fn presenting(key: &str) -> Vec<(&'static str, String)> {
    vec![
        (ALGORITHM_HEADER, ALGORITHM.to_string()),
        (KEY_HEADER, key.to_string()),
        (KEY_MD5_HEADER, key_digest(key)),
    ]
}

/// `headers` as the borrowed pairs the request harness takes.
fn borrowed<'a>(headers: &'a [(&'static str, String)]) -> Vec<(&'static str, &'a str)> {
    headers
        .iter()
        .map(|(name, value)| (*name, value.as_str()))
        .collect()
}

/// `HEAD /<bucket>/<key>` carrying `headers`.
async fn head_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&'static str, &str)],
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{name}/{key}"), headers, b"").await
}

/// What a row of the round-trip table is: the case, the key material it presents, the object
/// key it writes and the bytes it writes there.
struct Stored {
    case: &'static str,
    key_seed: u8,
    object_key: &'static str,
    body: Vec<u8>,
}

/// RULES 1 and 2: an object written under a customer-provided key records that key's digest,
/// is served to a read presenting the same key, and is refused to every other read.
#[tokio::test]
async fn an_object_stored_under_a_customer_key_is_served_only_to_that_key() {
    let router = router();
    bucket(&router, "sse-c-bucket").await;

    // The rows differ along the key material, the FORM of the object key and the size of the
    // object — none of which the rule depends on.
    let rows = [
        Stored {
            case: "a plain key name",
            key_seed: 1,
            object_key: "secret.bin",
            body: b"the bytes a customer key encrypted".to_vec(),
        },
        Stored {
            case: "a key with / separators",
            key_seed: 2,
            object_key: "deep/dir/secret.bin",
            body: vec![0x5A; 4_096],
        },
        Stored {
            case: "an empty object",
            key_seed: 3,
            object_key: "empty.bin",
            body: Vec::new(),
        },
    ];

    for Stored {
        case,
        key_seed,
        object_key,
        body,
    } in &rows
    {
        let key = customer_key(*key_seed);
        let digest = key_digest(&key);
        let headers = presenting(&key);
        let written = put_object(
            &router,
            "sse-c-bucket",
            object_key,
            body,
            &borrowed(&headers),
        )
        .await;
        assert_eq!(
            written.status, 200,
            "case {case:?}: a write supplying its own key must be served, got {} with body \
             {:?}",
            written.status,
            text(&written)
        );
        // What the write reports of the key: the algorithm and the DIGEST, and no S3-managed
        // algorithm at all — which is what tells a client which form the object is under.
        assert_eq!(
            written.header(ALGORITHM_HEADER).as_deref(),
            Some(ALGORITHM),
            "case {case:?}: the write must confirm the algorithm, got {:?}",
            written.header(ALGORITHM_HEADER)
        );
        assert_eq!(
            written.header(KEY_MD5_HEADER).as_deref(),
            Some(digest.as_str()),
            "case {case:?}: the write must confirm the digest of the key it was given, got \
             {:?}",
            written.header(KEY_MD5_HEADER)
        );
        assert_eq!(
            written.header("x-amz-server-side-encryption"),
            None,
            "case {case:?}: an SSE-C object reports no S3-managed algorithm, got {:?}",
            written.header("x-amz-server-side-encryption")
        );

        // A read presenting the SAME key is served the object, and reports the same pair.
        let served = get_object(
            &router,
            "sse-c-bucket",
            object_key,
            "",
            &borrowed(&headers),
        )
        .await;
        assert_eq!(
            served.status, 200,
            "case {case:?}: a read presenting the key must be served, got {} with body {:?}",
            served.status,
            text(&served)
        );
        assert_eq!(
            served.body.as_ref(),
            body.as_slice(),
            "case {case:?}: the read must serve the stored bytes, got {} bytes",
            served.body.len()
        );
        assert_eq!(
            served.header(KEY_MD5_HEADER).as_deref(),
            Some(digest.as_str()),
            "case {case:?}: the read must confirm the digest of the key, got {:?}",
            served.header(KEY_MD5_HEADER)
        );
        // And so is a read of its METADATA alone.
        let described = head_object(&router, "sse-c-bucket", object_key, &borrowed(&headers)).await;
        assert_eq!(
            described.status, 200,
            "case {case:?}: a HEAD presenting the key must be served, got {}",
            described.status
        );
        assert_eq!(
            described.header("content-length").as_deref(),
            Some(body.len().to_string().as_str()),
            "case {case:?}: the HEAD must report the object's size, got {:?}",
            described.header("content-length")
        );

        // A read presenting NO key cannot be served the object at all.
        let keyless = get_object(&router, "sse-c-bucket", object_key, "", &[]).await;
        assert_eq!(
            keyless.status, 400,
            "case {case:?}: a read presenting no key must be refused, got {} with body {:?}",
            keyless.status,
            text(&keyless)
        );
        assert_eq!(
            keyless.error_code(),
            INVALID_REQUEST,
            "case {case:?}: expected the {INVALID_REQUEST} code, got {:?}",
            keyless.error_code()
        );
        assert_eq!(
            keyless.error_message(),
            KEY_REQUIRED_MESSAGE,
            "case {case:?}: expected AWS's sentence for a read missing its key, got {:?}",
            keyless.error_message()
        );

        // And a read presenting ANOTHER well-formed key is refused the way a caller not
        // authorized for the object's plaintext is.
        let other = presenting(&customer_key(key_seed.wrapping_add(100)));
        let wrong = get_object(
            &router,
            "sse-c-bucket",
            object_key,
            "",
            &borrowed(&other),
        )
        .await;
        assert_eq!(
            wrong.status, 403,
            "case {case:?}: a read presenting another key must be refused 403, got {} with \
             body {:?}",
            wrong.status,
            text(&wrong)
        );
        assert_eq!(
            wrong.error_code(),
            ACCESS_DENIED,
            "case {case:?}: expected the {ACCESS_DENIED} code, got {:?}",
            wrong.error_code()
        );
        let wrong_head =
            head_object(&router, "sse-c-bucket", object_key, &borrowed(&other)).await;
        assert_eq!(
            wrong_head.status, 403,
            "case {case:?}: a HEAD presenting another key must be refused 403, got {}",
            wrong_head.status
        );
    }

    // The other side of rule 2: an object stored WITHOUT a customer key is not read with one
    // — the parameters do not apply to it — and is served to a read presenting none.
    let plain = put_object(&router, "sse-c-bucket", "plain.bin", b"no key at all", &[]).await;
    assert_eq!(
        plain.status, 200,
        "the unencrypted write must be served, got {} with body {:?}",
        plain.status,
        text(&plain)
    );
    let keyed = presenting(&customer_key(7));
    let not_applicable = get_object(
        &router,
        "sse-c-bucket",
        "plain.bin",
        "",
        &borrowed(&keyed),
    )
    .await;
    assert_eq!(
        not_applicable.status, 400,
        "case \"a key for an object stored without one\": expected HTTP 400, got {} with body \
         {:?}",
        not_applicable.status,
        text(&not_applicable)
    );
    assert_eq!(
        not_applicable.error_code(),
        INVALID_REQUEST,
        "case \"a key for an object stored without one\": expected the {INVALID_REQUEST} code, \
         got {:?}",
        not_applicable.error_code()
    );
}

/// One row of the member table: the case, the headers the write carries, and the code it must
/// be refused with.
type MemberRow<'a> = (&'a str, Vec<(&'static str, String)>, &'a str);

/// One presentation of the policy table: what the request presents, the headers it carries,
/// and whether the policy refuses it.
type Presentation<'a> = (&'a str, Vec<(&'static str, &'a str)>, bool);

/// RULE 3: the three members carry one key, and a request whose members do not name one is
/// refused — nothing stored.
#[tokio::test]
async fn members_that_do_not_name_one_usable_key_are_refused_and_store_nothing() {
    let router = router();
    bucket(&router, "sse-c-refusals").await;
    let key = customer_key(11);
    let digest = key_digest(&key);
    let other_digest = key_digest(&customer_key(12));
    // A base64 key of 128 bits — well formed, but not the length `AES256` takes.
    let short = ::base64::engine::general_purpose::STANDARD.encode([0x11_u8; 16]);

    // (case, the headers the write carries, the code it must be refused with)
    let rows: Vec<MemberRow<'_>> = vec![
        (
            "an algorithm with no key beside it",
            vec![(ALGORITHM_HEADER, ALGORITHM.to_string())],
            INVALID_ARGUMENT,
        ),
        (
            "a key with no algorithm beside it",
            vec![
                (KEY_HEADER, key.clone()),
                (KEY_MD5_HEADER, digest.clone()),
            ],
            INVALID_ARGUMENT,
        ),
        (
            "an algorithm S3 does not serve",
            vec![
                (ALGORITHM_HEADER, "AES128".to_string()),
                (KEY_HEADER, key.clone()),
                (KEY_MD5_HEADER, digest.clone()),
            ],
            INVALID_ARGUMENT,
        ),
        (
            "a key of the wrong length",
            vec![
                (ALGORITHM_HEADER, ALGORITHM.to_string()),
                (KEY_HEADER, short.clone()),
                (KEY_MD5_HEADER, key_digest(&short)),
            ],
            INVALID_ARGUMENT,
        ),
        (
            "a key that is not base64 at all",
            vec![
                (ALGORITHM_HEADER, ALGORITHM.to_string()),
                (KEY_HEADER, "not base64".to_string()),
                (KEY_MD5_HEADER, digest.clone()),
            ],
            INVALID_ARGUMENT,
        ),
        (
            "a key with no digest of it",
            vec![
                (ALGORITHM_HEADER, ALGORITHM.to_string()),
                (KEY_HEADER, key.clone()),
            ],
            INVALID_ARGUMENT,
        ),
        (
            "another key's digest",
            vec![
                (ALGORITHM_HEADER, ALGORITHM.to_string()),
                (KEY_HEADER, key.clone()),
                (KEY_MD5_HEADER, other_digest.clone()),
            ],
            INVALID_DIGEST,
        ),
        (
            "a customer key beside a request for an S3-managed algorithm",
            vec![
                (ALGORITHM_HEADER, ALGORITHM.to_string()),
                (KEY_HEADER, key.clone()),
                (KEY_MD5_HEADER, digest.clone()),
                ("x-amz-server-side-encryption", "AES256".to_string()),
            ],
            INVALID_ARGUMENT,
        ),
    ];

    for (index, (case, headers, code)) in rows.iter().enumerate() {
        let object_key = format!("refused/row-{index}.bin");
        let refused = put_object(
            &router,
            "sse-c-refusals",
            &object_key,
            b"bytes that must not be stored",
            &borrowed(headers),
        )
        .await;
        assert_eq!(
            refused.status, 400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            *code,
            "case {case:?}: expected the {code} code, got {:?} with body {:?}",
            refused.error_code(),
            text(&refused)
        );
        // Nothing was stored: the key the refused write addressed holds nothing, whichever
        // key a later read presents.
        let absent = get_object(
            &router,
            "sse-c-refusals",
            &object_key,
            "",
            &borrowed(&presenting(&key)),
        )
        .await;
        assert_eq!(
            absent.error_code(),
            "NoSuchKey",
            "case {case:?}: a refused write must store nothing, got {:?} with status {}",
            absent.error_code(),
            absent.status
        );
    }
}

/// RULE 4: a multipart upload initiated under a customer key is carried out with that key and
/// with no other, and the object it assembles is read with it.
#[tokio::test]
async fn a_multipart_upload_under_a_customer_key_is_carried_out_with_that_key() {
    let router = router();
    bucket(&router, "sse-c-multipart").await;
    let key = customer_key(21);
    let digest = key_digest(&key);
    let headers = presenting(&key);
    let other = presenting(&customer_key(22));
    let body = vec![0xA5_u8; 2_048];

    let initiated = send_bytes(
        &router,
        "POST",
        "/sse-c-multipart/assembled.bin?uploads",
        &borrowed(&headers),
        b"",
    )
    .await;
    assert_eq!(
        initiated.status, 200,
        "an initiation supplying its own key must be served, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    assert_eq!(
        initiated.header(KEY_MD5_HEADER).as_deref(),
        Some(digest.as_str()),
        "the initiation must confirm the digest of the key it was given, got {:?}",
        initiated.header(KEY_MD5_HEADER)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");
    let part_address =
        format!("/sse-c-multipart/assembled.bin?partNumber=1&uploadId={upload_id}");

    // A part presenting ANOTHER key is refused for parameters that are not the upload's own,
    // and a part presenting none cannot be served at all.
    let wrong_part = send_bytes(&router, "PUT", &part_address, &borrowed(&other), &body).await;
    assert_eq!(
        wrong_part.status, 400,
        "case \"a part under another key\": expected HTTP 400, got {} with body {:?}",
        wrong_part.status,
        text(&wrong_part)
    );
    assert_eq!(
        wrong_part.error_code(),
        INVALID_REQUEST,
        "case \"a part under another key\": expected the {INVALID_REQUEST} code, got {:?}",
        wrong_part.error_code()
    );
    let keyless_part = send_bytes(&router, "PUT", &part_address, &[], &body).await;
    assert_eq!(
        keyless_part.status, 400,
        "case \"a part presenting no key\": expected HTTP 400, got {} with body {:?}",
        keyless_part.status,
        text(&keyless_part)
    );

    // The part presenting the upload's own key is served, and reports the same pair.
    let part = send_bytes(&router, "PUT", &part_address, &borrowed(&headers), &body).await;
    assert_eq!(
        part.status, 200,
        "case \"a part under the upload's key\": expected HTTP 200, got {} with body {:?}",
        part.status,
        text(&part)
    );
    assert_eq!(
        part.header(KEY_MD5_HEADER).as_deref(),
        Some(digest.as_str()),
        "case \"a part under the upload's key\": the part write must confirm the digest, got \
         {:?}",
        part.header(KEY_MD5_HEADER)
    );
    let etag = part
        .header("etag")
        .expect("a part upload must report an ETag header");
    let completion = format!(
        "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag>\
         </Part></CompleteMultipartUpload>"
    );

    // A completion under another key is refused the same way, and the upload is still there.
    let wrong_completion = send(
        &router,
        "POST",
        &format!("/sse-c-multipart/assembled.bin?uploadId={upload_id}"),
        &borrowed(&other),
        &completion,
    )
    .await;
    assert_eq!(
        wrong_completion.error_code(),
        ACCESS_DENIED,
        "case \"a completion under another key\": expected the {ACCESS_DENIED} code, got {:?} \
         with status {}",
        wrong_completion.error_code(),
        wrong_completion.status
    );

    let completed = send(
        &router,
        "POST",
        &format!("/sse-c-multipart/assembled.bin?uploadId={upload_id}"),
        &borrowed(&headers),
        &completion,
    )
    .await;
    assert_eq!(
        completed.status, 200,
        "case \"a completion under the upload's key\": expected HTTP 200, got {} with body \
         {:?}",
        completed.status,
        text(&completed)
    );

    // And the object it assembled is read with that key, and with no other.
    let served = get_object(
        &router,
        "sse-c-multipart",
        "assembled.bin",
        "",
        &borrowed(&headers),
    )
    .await;
    assert_eq!(
        served.body.as_ref(),
        body.as_slice(),
        "case \"the assembled object\": expected the uploaded bytes, got {} bytes with status \
         {}",
        served.body.len(),
        served.status
    );
    assert_eq!(
        served.header(KEY_MD5_HEADER).as_deref(),
        Some(digest.as_str()),
        "case \"the assembled object\": the read must confirm the digest of the key, got {:?}",
        served.header(KEY_MD5_HEADER)
    );
    let keyless = get_object(&router, "sse-c-multipart", "assembled.bin", "", &[]).await;
    assert_eq!(
        keyless.error_code(),
        INVALID_REQUEST,
        "case \"the assembled object, read with no key\": expected the {INVALID_REQUEST} code, \
         got {:?} with status {}",
        keyless.error_code(),
        keyless.status
    );
}

/// One row of the policy table: the bucket it configures (its own, so one row's policy cannot
/// decide another row's write), the condition clause the `Deny` is written over, and whether a
/// write that SUPPLIES a customer key, and one that does not, must be refused.
struct PolicyRow {
    case: &'static str,
    bucket: &'static str,
    condition: &'static str,
    keyed_refused: bool,
    plain_refused: bool,
}

/// RULE 5: a bucket policy written over the customer-key condition decides a write that
/// supplies one — and decides a multipart initiation the same way.
#[tokio::test]
async fn a_policy_over_the_customer_key_condition_decides_the_write_and_the_initiation() {
    let router = router();
    let key = customer_key(31);
    let headers = presenting(&key);

    // The rows differ along which way round the clause is written: deny when the key is
    // supplied, deny when it is NOT (which is how a bucket that requires customer keys is
    // written), and deny when the algorithm named is the one S3 serves.
    let rows = [
        PolicyRow {
            case: "a Deny when a customer key is supplied",
            bucket: "sse-c-denied",
            condition:
                "{\"Null\":{\"s3:x-amz-server-side-encryption-customer-algorithm\":\"false\"}}",
            keyed_refused: true,
            plain_refused: false,
        },
        PolicyRow {
            case: "a Deny unless a customer key is supplied",
            bucket: "sse-c-required",
            condition:
                "{\"Null\":{\"s3:x-amz-server-side-encryption-customer-algorithm\":\"true\"}}",
            keyed_refused: false,
            plain_refused: true,
        },
        PolicyRow {
            case: "a Deny on the algorithm the key names",
            bucket: "sse-c-algorithm-denied",
            condition: "{\"StringEquals\":{\"s3:x-amz-server-side-encryption-customer-\
                        algorithm\":\"AES256\"}}",
            keyed_refused: true,
            plain_refused: false,
        },
    ];

    for row in &rows {
        bucket(&router, row.bucket).await;
        let resource = format!("arn:aws:s3:::{}/*", row.bucket);
        let document = policy_document(&statement(
            "Deny",
            PUT_OBJECT_ACTION,
            &resource,
            Some(row.condition),
        ));
        let configured = put_bucket_policy(&router, row.bucket, &document).await;
        assert_eq!(
            configured.status, 204,
            "case {:?}: the policy must be accepted first, got {} with body {:?}",
            row.case,
            configured.status,
            text(&configured)
        );

        // (what the request presents, the key it writes, whether the policy refuses it)
        let presentations: [Presentation<'_>; 2] = [
            ("supplying a customer key", borrowed(&headers), row.keyed_refused),
            ("supplying none", Vec::new(), row.plain_refused),
        ];
        for (presentation, request_headers, refused) in presentations {
            let case = format!("{} {presentation}", row.case);
            let object_key = format!("written/{}.bin", presentation.replace(' ', "-"));
            let written = put_object(
                &router,
                row.bucket,
                &object_key,
                b"policy-governed-payload",
                &request_headers,
            )
            .await;
            // And the MULTIPART form of the same write, which is authorized under the same
            // action against the same policy.
            let initiated = send_bytes(
                &router,
                "POST",
                &format!("/{}/{object_key}?uploads", row.bucket),
                &request_headers,
                b"",
            )
            .await;
            if refused {
                for (operation, answer) in [("PutObject", &written), ("CreateMultipartUpload", &initiated)]
                {
                    assert_eq!(
                        answer.status, 403,
                        "case {case:?} ({operation}): expected HTTP 403, got {} with body {:?}",
                        answer.status,
                        text(answer)
                    );
                    assert_eq!(
                        answer.error_code(),
                        ACCESS_DENIED,
                        "case {case:?} ({operation}): expected the {ACCESS_DENIED} code, got \
                         {:?}",
                        answer.error_code()
                    );
                    assert_eq!(
                        answer.error_message(),
                        explicit_deny_message(PUT_OBJECT_ACTION, &format!("arn:aws:s3:::{}/{object_key}", row.bucket)),
                        "case {case:?} ({operation}): expected IAM's explicit-deny sentence, \
                         got {:?}",
                        answer.error_message()
                    );
                }
                // A refused write stores nothing.
                let absent = get_object(
                    &router,
                    row.bucket,
                    &object_key,
                    "",
                    &borrowed(&presenting(&key)),
                )
                .await;
                assert_ne!(
                    absent.status, 200,
                    "case {case:?}: a refused write must store nothing, got {} bytes",
                    absent.body.len()
                );
            } else {
                assert_eq!(
                    written.status, 200,
                    "case {case:?} (PutObject): the policy admits this write, got {} with body \
                     {:?}",
                    written.status,
                    text(&written)
                );
                assert_eq!(
                    initiated.status, 200,
                    "case {case:?} (CreateMultipartUpload): the policy admits this initiation, \
                     got {} with body {:?}",
                    initiated.status,
                    text(&initiated)
                );
            }
        }
    }
}

/// The three COPY-SOURCE headers a part copy presents the SOURCE object's own key in: the
/// `x-amz-copy-source-` spelling of the same three
/// (`smithy_query(query="operation",operation="UploadPartCopy")` binds
/// `CopySourceSSECustomerAlgorithm`, `CopySourceSSECustomerKey` and
/// `CopySourceSSECustomerKeyMD5` to them).
const COPY_SOURCE_ALGORITHM_HEADER: &str =
    "x-amz-copy-source-server-side-encryption-customer-algorithm";
const COPY_SOURCE_KEY_HEADER: &str = "x-amz-copy-source-server-side-encryption-customer-key";
const COPY_SOURCE_KEY_MD5_HEADER: &str =
    "x-amz-copy-source-server-side-encryption-customer-key-MD5";

/// The three copy-source headers a part copy reading `key` carries.
fn presenting_source(key: &str) -> Vec<(&'static str, String)> {
    vec![
        (COPY_SOURCE_ALGORITHM_HEADER, ALGORITHM.to_string()),
        (COPY_SOURCE_KEY_HEADER, key.to_string()),
        (COPY_SOURCE_KEY_MD5_HEADER, key_digest(key)),
    ]
}

/// The sentence S3 answers a PART of an SSE-C upload that presents no key: the initiation
/// asked for encryption, so every part of it has to carry the same key.
const INITIATE_ENCRYPTED_MESSAGE: &str =
    "The multipart upload initiate requested encryption. Subsequent part requests must \
     include the appropriate encryption parameters.";

/// The sentence S3 answers a PART that presents a DIFFERENT key from the upload's own.
const PARAMETERS_MISMATCH_MESSAGE: &str =
    "The provided encryption parameters did not match the ones used originally.";

/// One row of the part-copy table: the case, the key the copy presents for the SOURCE and the
/// key it presents for the PART (`None` for a row that presents neither), and the refusal
/// expected as `(status, code, message)` — `None` for a copy that is served.
type CopyRow = (
    &'static str,
    Option<u8>,
    Option<u8>,
    Option<(u16, &'static str, &'static str)>,
);

/// RULE: a part copy whose source is stored under a customer-provided key presents that key in
/// the `x-amz-copy-source-` headers and the UPLOAD's own key in its own, so an SSE-C source is
/// re-keyed into an SSE-C multipart object; each half is refused on its own when the key it
/// needs is missing or is another one.
///
/// The rows differ along WHICH half of the copy carries which key — both, neither, each one
/// alone, and each one wrong — so the rule is pinned over the two keys rather than over one
/// request.
#[tokio::test]
async fn a_part_copy_re_keys_an_ssec_source_into_an_ssec_upload() {
    let router = router();
    bucket(&router, "sse-c-part-copy").await;
    let source_seed = 41_u8;
    let upload_seed = 42_u8;
    let source_key = customer_key(source_seed);
    let upload_key = customer_key(upload_seed);
    assert_ne!(
        source_key, upload_key,
        "the source and the upload must be stored under different keys"
    );
    let body = vec![0x5A_u8; 1_024];

    // The SOURCE object, stored under its own customer key.
    let stored = send_bytes(
        &router,
        "PUT",
        "/sse-c-part-copy/source.bin",
        &borrowed(&presenting(&source_key)),
        &body,
    )
    .await;
    assert_eq!(
        stored.status, 200,
        "the source object must be stored under its key, got {} with body {:?}",
        stored.status,
        text(&stored)
    );

    // The UPLOAD, initiated under a DIFFERENT customer key: re-keying is what this copy does.
    let initiated = send_bytes(
        &router,
        "POST",
        "/sse-c-part-copy/rekeyed.bin?uploads",
        &borrowed(&presenting(&upload_key)),
        b"",
    )
    .await;
    assert_eq!(
        initiated.status, 200,
        "the upload must be initiated under its key, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    let rows: [CopyRow; 5] = [
        (
            "no key for either half",
            None,
            None,
            Some((400, INVALID_REQUEST, INITIATE_ENCRYPTED_MESSAGE)),
        ),
        (
            "the upload's key and no source key",
            None,
            Some(upload_seed),
            Some((400, INVALID_REQUEST, KEY_REQUIRED_MESSAGE)),
        ),
        (
            "the source's key and no upload key",
            Some(source_seed),
            None,
            Some((400, INVALID_REQUEST, INITIATE_ENCRYPTED_MESSAGE)),
        ),
        (
            "another key for the upload",
            Some(source_seed),
            Some(upload_seed.wrapping_add(7)),
            Some((400, INVALID_REQUEST, PARAMETERS_MISMATCH_MESSAGE)),
        ),
        (
            "another key for the source",
            Some(source_seed.wrapping_add(7)),
            Some(upload_seed),
            Some((403, ACCESS_DENIED, "Access Denied")),
        ),
    ];
    for (case, source, upload, refused) in rows {
        let mut headers: Vec<(&'static str, String)> = Vec::new();
        if let Some(seed) = source {
            headers.extend(presenting_source(&customer_key(seed)));
        }
        if let Some(seed) = upload {
            headers.extend(presenting(&customer_key(seed)));
        }
        let copied = upload_part_copy(
            &router,
            "sse-c-part-copy",
            "rekeyed.bin",
            &upload_id,
            1,
            "sse-c-part-copy/source.bin",
            &borrowed(&headers),
        )
        .await;
        let (status, code, message) =
            refused.expect("every row of this table names a refusal");
        assert_eq!(
            (copied.status, copied.error_code(), copied.error_message()),
            (status, code.to_string(), message.to_string()),
            "case {case:?}: expected {status} {code:?}, got {} with body {:?}",
            copied.status,
            text(&copied)
        );
        // A refused copy recorded nothing: the upload still holds no part.
        let parts = send(
            &router,
            "GET",
            &format!("/sse-c-part-copy/rekeyed.bin?uploadId={upload_id}&x-id=ListParts"),
            &borrowed(&presenting(&upload_key)),
            "",
        )
        .await;
        assert!(
            parts.elements("PartNumber").is_empty(),
            "case {case:?}: a refused copy must record no part, got {:?}",
            text(&parts)
        );
    }

    // Both keys together: the copy is served, reports the UPLOAD's digest (the part is stored
    // under the upload's key, not the source's), and the assembled object reads back with the
    // upload's key alone.
    let mut both = presenting_source(&source_key);
    both.extend(presenting(&upload_key));
    let copied = upload_part_copy(
        &router,
        "sse-c-part-copy",
        "rekeyed.bin",
        &upload_id,
        1,
        "sse-c-part-copy/source.bin",
        &borrowed(&both),
    )
    .await;
    assert_eq!(
        copied.status, 200,
        "a copy presenting both keys must be served, got {} with body {:?}",
        copied.status,
        text(&copied)
    );
    assert_eq!(
        copied.header(KEY_MD5_HEADER).as_deref(),
        Some(key_digest(&upload_key).as_str()),
        "the copy must confirm the UPLOAD's key digest, got {:?}",
        copied.header(KEY_MD5_HEADER)
    );
    let etag = copied
        .element("ETag")
        .expect("a part copy must report the part's ETag");
    let completed = send(
        &router,
        "POST",
        &format!("/sse-c-part-copy/rekeyed.bin?uploadId={upload_id}"),
        &borrowed(&presenting(&upload_key)),
        &format!(
            "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag>\
             </Part></CompleteMultipartUpload>"
        ),
    )
    .await;
    assert_eq!(
        completed.status, 200,
        "the completion under the upload's key must be served, got {} with body {:?}",
        completed.status,
        text(&completed)
    );
    let served = get_object(
        &router,
        "sse-c-part-copy",
        "rekeyed.bin",
        "",
        &borrowed(&presenting(&upload_key)),
    )
    .await;
    assert_eq!(
        served.body.as_ref(),
        body.as_slice(),
        "the re-keyed object must hold the source's bytes, got {} bytes with status {}",
        served.body.len(),
        served.status
    );
    let with_source_key = get_object(
        &router,
        "sse-c-part-copy",
        "rekeyed.bin",
        "",
        &borrowed(&presenting(&source_key)),
    )
    .await;
    assert_eq!(
        (with_source_key.status, with_source_key.error_code()),
        (403, ACCESS_DENIED.to_string()),
        "the re-keyed object must not open with the SOURCE's key, got {} with body {:?}",
        with_source_key.status,
        text(&with_source_key)
    );
}
