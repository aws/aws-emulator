// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for what `HeadObject` REFUSES: a key or a bucket this emulator does
//! not hold, and the request members whose answer a 200 metadata read cannot carry.
//!
//! The read-back rule itself — what a stored object reports — is asserted in
//! `object_write_and_read_back.rs` beside this suite. Every assertion here drives a real
//! HTTP request through the router, so what is pinned is the wire behaviour (status, error
//! code) rather than any handler's internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A key holding no object is `NoSuchKey` and a bucket this emulator does not hold is
//!    `NoSuchBucket`, both with HTTP 404 — the status an SDK waiter and an
//!    `object_exists` check key on. (`NotFound`, the only error `HeadObject` models,
//!    carries no `@httpError`, so returning it would render the framework's client-error
//!    default of 400 instead.)
//! 2. A member whose answer this operation cannot render — a partial read (HTTP 206 and a
//!    `Content-Range`), a conditional read (304 or 412), a version this emulator does not
//!    keep, customer-key encryption nothing performed, a Requester Pays charge nothing
//!    meters — is refused with the scaffold's 501
//!    rather than answered as a plain full-metadata 200.
//! 3. Naming another account as the bucket owner is `AccessDenied` (403), exactly as it is
//!    on a write.

use std::sync::Arc;

use service_com_amazonaws_s3_amazons3::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The router picks a service by the SigV4 credential scope's service field, so every
/// request carries one naming `s3`. Neither the key id nor the signature is a credential:
/// this harness turns request authentication off, exactly as the emulator runs it unless
/// started with authentication on, so nothing here is verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=emulator-test-key/20260101/us-east-1/s3/aws4_request, \
     SignedHeaders=host, Signature=not-verified";

/// One service mounted on its own engine, reached exactly the way the production router
/// reaches it.
fn router() -> MultiServiceRouter {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(engine));
    router
}

/// What a dispatched request answered.
struct Answer {
    status: u16,
    headers: http::HeaderMap,
    body: bytes::Bytes,
}

impl Answer {
    /// A bound response header's value.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    }

    /// The wire error code: the `<Code>` element of S3's restXml error document (the model
    /// sets `noErrorWrapping`, so it is the document's own child), or the
    /// `x-amzn-errortype` header when the protocol carried it there instead.
    fn error_code(&self) -> String {
        let body = String::from_utf8_lossy(&self.body).to_string();
        body.split_once("<Code>")
            .and_then(|(_, rest)| rest.split_once("</Code>"))
            .map(|(code, _)| code.to_string())
            .or_else(|| self.header("x-amzn-errortype"))
            .unwrap_or(body)
    }
}

/// Dispatch one request with the given headers and raw body bytes.
async fn send(
    router: &MultiServiceRouter,
    method: &str,
    uri: &str,
    headers: &[(&str, &str)],
    body: Vec<u8>,
) -> Answer {
    let mut builder = http::Request::builder()
        .method(method)
        .uri(format!("https://s3.us-east-1.amazonaws.com{uri}"))
        .header("host", "s3.us-east-1.amazonaws.com")
        .header("authorization", AUTHORIZATION);
    for (name, value) in headers {
        builder = builder.header(*name, *value);
    }
    let request = builder
        .body(bytes::Bytes::from(body))
        .expect("the test request must build");
    let response = router.dispatch(request).await;
    let (parts, body) = response.into_parts();
    Answer {
        status: parts.status.as_u16(),
        headers: parts.headers,
        body,
    }
}

/// `PUT /<bucket>` — `CreateBucket` in the default Region.
async fn create_bucket(router: &MultiServiceRouter, bucket: &str) -> Answer {
    send(router, "PUT", &format!("/{bucket}"), &[], Vec::new()).await
}

/// `PUT /<bucket>/<key>` — `PutObject`.
async fn put_object(router: &MultiServiceRouter, bucket: &str, key: &str, body: &[u8]) -> Answer {
    send(
        router,
        "PUT",
        &format!("/{bucket}/{key}"),
        &[],
        body.to_vec(),
    )
    .await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`, with the given bound headers.
async fn head_object(
    router: &MultiServiceRouter,
    bucket: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "HEAD",
        &format!("/{bucket}/{key}"),
        headers,
        Vec::new(),
    )
    .await
}

#[tokio::test]
async fn reading_an_object_this_emulator_does_not_hold_is_not_found() {
    let router = router();
    create_bucket(&router, "sparse-bucket").await;
    assert_eq!(
        put_object(&router, "sparse-bucket", "a/held.txt", b"body")
            .await
            .status,
        200,
        "the one object the rows below are near-misses of must be written first"
    );

    // Keys that are NOT the one object written: a different key, a prefix of it, an
    // extension of it, and one differing only in case (object keys are case-sensitive).
    for key in ["absent.txt", "a/held", "a/held.txt.bak", "A/Held.txt"] {
        let read = head_object(&router, "sparse-bucket", key, &[]).await;
        assert_eq!(
            read.status, 404,
            "case {key:?}: expected HTTP 404 for a key holding no object, got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            "NoSuchKey",
            "case {key:?}: expected NoSuchKey, got {:?}",
            read.error_code()
        );
    }

    // Buckets that are NOT the one created: the same four shapes of near-miss.
    for bucket in [
        "absent-bucket",
        "sparse",
        "sparse-bucket-2",
        "Sparse-Bucket",
    ] {
        let read = head_object(&router, bucket, "a/held.txt", &[]).await;
        assert_eq!(
            read.status, 404,
            "case {bucket:?}: expected HTTP 404 for a bucket this emulator does not hold, \
             got {}",
            read.status
        );
        assert_eq!(
            read.error_code(),
            "NoSuchBucket",
            "case {bucket:?}: expected NoSuchBucket, got {:?}",
            read.error_code()
        );
    }

    // And the object that IS held still reads back, so the rows above failed on the
    // lookup rather than on the operation being unreachable.
    let held = head_object(&router, "sparse-bucket", "a/held.txt", &[]).await;
    assert_eq!(
        held.status, 200,
        "the written key must still read back, got {}",
        held.status
    );
}

#[tokio::test]
async fn a_read_asking_for_a_member_this_operation_cannot_answer_is_refused() {
    let router = router();
    let bucket = "refusal-bucket";
    create_bucket(&router, bucket).await;
    put_object(&router, bucket, "held.txt", b"abc").await;

    // Each bound header below names a feature no object here has: a 200 full-metadata read
    // could not report it. The two conditional headers that answer 304 are NOT here — they
    // are served (see [`super::object_conditional_reads`]) — and neither are the SSE-C
    // members, which carry the key an object stored under a customer-provided one is read
    // with (see [`super::object_customer_key_encryption`]).
    let rows: [(&str, &str, &str); 1] =
        [("a requester-pays read", "x-amz-request-payer", "requester")];
    for (case, header, value) in rows {
        let answer = head_object(&router, bucket, "held.txt", &[(header, value)]).await;
        assert_eq!(
            answer.status, 501,
            "case {case:?}: expected HTTP 501 for a member this operation cannot answer, \
             got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.error_code(),
            "NotImplemented",
            "case {case:?}: expected NotImplemented, got {:?}",
            answer.error_code()
        );
    }

    // The members this read DOES answer: a byte range is served as the partial read it asks
    // for ("a HEAD request has the same options as a GET operation on an object"), and a
    // precondition is judged against the object — served when it holds, refused with the
    // documented 412 when it does not.
    let ranged = head_object(&router, bucket, "held.txt", &[("range", "bytes=0-1")]).await;
    assert_eq!(
        ranged.status, 206,
        "case \"a byte range\": expected HTTP 206, got {} with body {:?}",
        ranged.status,
        String::from_utf8_lossy(&ranged.body)
    );
    assert_eq!(
        ranged.header("content-range"),
        Some(String::from("bytes 0-1/3")),
        "case \"a byte range\": expected the span of the object, got {:?}",
        ranged.header("content-range")
    );
    let held_etag = head_object(&router, bucket, "held.txt", &[])
        .await
        .header("etag")
        .expect("the stored object must report an ETag");
    let matched = head_object(&router, bucket, "held.txt", &[("if-match", &held_etag)]).await;
    assert_eq!(
        matched.status, 200,
        "case \"a conditional match that holds\": expected HTTP 200, got {} with body {:?}",
        matched.status,
        String::from_utf8_lossy(&matched.body)
    );
    for (case, header, value) in [
        ("a conditional match that does not hold", "if-match", "\"not-this-object\""),
        (
            "an unmodified-since condition that does not hold",
            "if-unmodified-since",
            "2020-10-21T07:28:00Z",
        ),
    ] {
        let refused = head_object(&router, bucket, "held.txt", &[(header, value)]).await;
        assert_eq!(
            refused.status, 412,
            "case {case:?}: expected HTTP 412, got {} with body {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
    }

    // `x-amz-checksum-mode` IS served: "to retrieve the checksum, this parameter must be
    // enabled" (`HeadObjectRequest$ChecksumMode`) asks for the checksum the object was
    // STORED with. This object was written under no checksum algorithm of its own, so what
    // it carries is the default S3 adds to such an upload — "if it was uploaded without a
    // checksum (and Amazon S3 added the default checksum, CRC64NVME, to the uploaded
    // object)" (`PutObjectOutput$ChecksumCRC64NVME`) — over the whole object.
    let enabled = head_object(
        &router,
        bucket,
        "held.txt",
        &[("x-amz-checksum-mode", "ENABLED")],
    )
    .await;
    assert_eq!(
        enabled.status, 200,
        "case \"a checksum read of an object stored without one\": expected HTTP 200, got {} \
         with body {:?}",
        enabled.status,
        String::from_utf8_lossy(&enabled.body)
    );
    assert_eq!(
        enabled.header("x-amz-checksum-crc64nvme"),
        service_com_amazonaws_s3_amazons3::state::full_object_checksum("CRC64NVME", b"abc"),
        "case \"a checksum read of an object stored without one\": the default checksum of \
         the stored bytes must be reported, got {:?}",
        enabled.header("x-amz-checksum-crc64nvme")
    );
    assert_eq!(
        enabled.header("x-amz-checksum-type").as_deref(),
        Some("FULL_OBJECT"),
        "case \"a checksum read of an object stored without one\": a one-shot write's \
         checksum is over the whole object, got {:?}",
        enabled.header("x-amz-checksum-type")
    );
    for member in [
        "x-amz-checksum-crc32",
        "x-amz-checksum-crc32c",
        "x-amz-checksum-sha1",
        "x-amz-checksum-sha256",
    ] {
        assert_eq!(
            enabled.header(member),
            None,
            "case {member:?}: an object carries the checksum of ONE algorithm, got {:?}",
            enabled.header(member)
        );
    }

    // `partNumber` is a query parameter rather than a header, and is served the same way a
    // `Range` is: this object was stored by one write, so part 1 IS the whole object and a
    // part past it is not satisfiable.
    for (case, query, expected_status) in [
        ("the only part of a one-shot write", "partNumber=1", 206),
        ("a part number past the object's parts", "partNumber=2", 416),
    ] {
        let answer = send(
            &router,
            "HEAD",
            &format!("/{bucket}/held.txt?{query}"),
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
    }

    // `versionId` IS served: it names one stored version of the key. The bucket here was
    // never versioning-enabled, so its object is the `null` version and no other version
    // id names anything — which is `NoSuchVersion` (404), not the 501 of an unserved
    // member and not the `NoSuchKey` of an absent key.
    for (case, query, expected_status, expected_code) in [
        ("the null version of an unversioned object", "versionId=null", 200, ""),
        (
            "a WELL-FORMED version id the bucket holds no version for",
            "versionId=0123456789abcdef0123456789abcdef",
            404,
            "NoSuchVersion",
        ),
        (
            "a version id that is not shaped like one",
            "versionId=abc",
            400,
            "InvalidArgument",
        ),
    ] {
        let answer = send(
            &router,
            "HEAD",
            &format!("/{bucket}/held.txt?{query}"),
            &[],
            Vec::new(),
        )
        .await;
        assert_eq!(
            answer.status, expected_status,
            "case {case:?}: expected HTTP {expected_status}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if !expected_code.is_empty() {
            assert_eq!(
                answer.error_code(),
                expected_code,
                "case {case:?}: expected {expected_code}, got {:?}",
                answer.error_code()
            );
        }
    }

    // Naming another account as the bucket owner is a 403, as it is on a write.
    for (case, owner, expected) in [
        (
            "the account this emulator serves",
            engine::emulator_account_id(),
            200,
        ),
        ("another twelve-digit account", "111122223333", 403),
        ("an account id that is not one", "not-an-account", 403),
    ] {
        let answer = head_object(
            &router,
            bucket,
            "held.txt",
            &[("x-amz-expected-bucket-owner", owner)],
        )
        .await;
        assert_eq!(
            answer.status, expected,
            "case {case:?}: expected HTTP {expected}, got {} with body {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        if expected == 403 {
            assert_eq!(
                answer.error_code(),
                "AccessDenied",
                "case {case:?}: expected AccessDenied, got {:?}",
                answer.error_code()
            );
        }
    }

    // None of the refusals changed what the bucket holds.
    let held = head_object(&router, bucket, "held.txt", &[]).await;
    assert_eq!(
        held.status, 200,
        "a refused read must leave the stored object readable, got {}",
        held.status
    );
}
