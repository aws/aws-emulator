// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the REQUEST HEADERS an object READ carries: the ones a read may not
//! carry at all, and the one a read of an object's attributes must.
//!
//! Every assertion drives a real HTTP request through the router (the harness is the one
//! [`super::bucket_lifecycle`], [`super::object_and_tagging_roundtrip`] and
//! [`super::object_versioning`] already own), so what is pinned is the wire behaviour —
//! status, error code, the error document's members, the bound headers — rather than any
//! handler's internals.
//!
//! The rules under test, each asserted over a SET of requests rather than one:
//!
//! * The encryption of an object is chosen by the WRITE that stores it: a READ — `GetObject`,
//!   `HeadObject`, `GetObjectAttributes` — binds none of the headers a write names an
//!   S3-managed encryption with (`smithy_query(query="operation",operation="GetObject")`
//!   reports only the SSE-C members on its input, and
//!   `smithy_query(query="operation",operation="PutObject")` the four write ones), and reports
//!   that stored configuration in headers of the same names instead. So a read carrying one of
//!   them is refused for the HEADER — `InvalidArgument` (400) naming it in `<ArgumentName>`
//!   and echoing it in `<ArgumentValue>` — rather than served the object with the header
//!   ignored.
//! * "Specifies the fields at the root level that you want returned in the response"
//!   (`GetObjectAttributesRequest$ObjectAttributes`, a REQUIRED `@httpHeader` member): a read
//!   of an object's attributes that SENDS no such header states no attribute and is refused
//!   `InvalidRequest` (400) with the sentence naming that header, never answered with the
//!   object's BODY, which is a different operation's answer. A header the request did send
//!   names attributes, so one outside the published set — the header sent empty among them —
//!   is that attribute's own refusal.
//! * A read answered about a key whose version is a DELETE MARKER names that marker in its
//!   `x-amz-delete-marker` and `x-amz-version-id` headers, which is how a caller tells a
//!   deleted object from a missing one and learns which version to delete to bring it back.
//! * "Specifies the byte range of the object to get" (`GetObjectRequest$Range`): the SPAN a
//!   read asks for is resolved before the key it presents, so a request that both asks for
//!   bytes outside the object and presents encryption parameters that do not apply to it is
//!   answered with the 416 for the span — recorded behaviour for that pair.

use ::base64::Engine as _;
use ::md5::Digest as _;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_versioning::{delete_object, set_versioning};

/// The bytes every object in this suite is written with.
const BODY: &[u8] = b"a,b\n1,2\n";

/// Every header a WRITE names the S3-managed encryption of the object it stores in, with a
/// value a write would carry — none of which a read binds.
///
/// Source: the `@httpHeader` members of `PutObject`'s input
/// (`smithy_query(query="operation",operation="PutObject")` reports `ServerSideEncryption`,
/// `SSEKMSKeyId`, `SSEKMSEncryptionContext` and `BucketKeyEnabled`).
const WRITE_ONLY_ENCRYPTION_HEADERS: [(&str, &str); 4] = [
    ("x-amz-server-side-encryption", "AES256"),
    (
        "x-amz-server-side-encryption-aws-kms-key-id",
        "arn:aws:kms:us-east-1:123456789012:key/4e2b6f1c-1111-2222-3333-444455556666",
    ),
    ("x-amz-server-side-encryption-context", "eyJrIjoidiJ9"),
    (
        "x-amz-server-side-encryption-bucket-key-enabled",
        "true",
    ),
];

/// `HEAD /<bucket>/<key>` with the headers the case carries.
async fn head_object_with(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(router, "HEAD", &format!("/{name}/{key}"), headers, b"").await
}

/// `GET /<bucket>/<key>?attributes` with the headers the case carries.
async fn get_object_attributes_with(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send_bytes(
        router,
        "GET",
        &format!("/{name}/{key}?attributes"),
        headers,
        b"",
    )
    .await
}

/// Which of the three reads of an object a case drives.
#[derive(Debug, Clone, Copy)]
enum Read {
    /// `GET /<bucket>/<key>` — the object itself.
    Object,
    /// `HEAD /<bucket>/<key>` — its metadata.
    Metadata,
    /// `GET /<bucket>/<key>?attributes` — the attributes document.
    Attributes,
}

/// Every read of an object this service serves, as a case names it.
const READS: [Read; 3] = [Read::Object, Read::Metadata, Read::Attributes];

/// Drive `read` against `key`, carrying `headers` — plus, for the attributes read, the
/// `x-amz-object-attributes` that read requires, so what the case varies is the encryption
/// header alone.
async fn drive(
    router: &MultiServiceRouter,
    read: Read,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> Answer {
    match read {
        Read::Object => get_object(router, name, key, "", headers).await,
        Read::Metadata => head_object_with(router, name, key, headers).await,
        Read::Attributes => {
            let mut all = headers.to_vec();
            all.push(("x-amz-object-attributes", "ETag"));
            get_object_attributes_with(router, name, key, &all).await
        }
    }
}

#[tokio::test]
async fn a_read_carrying_a_write_only_encryption_header_is_refused_for_that_header() {
    for read in READS {
        let router = router();
        let name = "read-encryption-headers";
        bucket(&router, name).await;
        let key = "data/one.csv";
        let written = put_object(&router, name, key, BODY, &[]).await;
        assert_eq!(
            written.status,
            200,
            "case {read:?}: the setup write must be accepted, got {} with body {:?}",
            written.status,
            text(&written)
        );

        // Every write-only encryption header, one request each: the refusal names the header
        // the request carried and echoes the value as sent.
        for (header, value) in WRITE_ONLY_ENCRYPTION_HEADERS {
            let case = format!("{read:?} carrying {header}");
            let answer = drive(&router, read, name, key, &[(header, value)]).await;
            assert_eq!(
                answer.status,
                400,
                "case {case:?}: expected HTTP 400, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.header("content-type").as_deref(),
                Some("application/xml"),
                "case {case:?}: a refusal is an XML error document, got {:?}",
                answer.header("content-type")
            );
            assert_eq!(
                answer.error_code(),
                "InvalidArgument",
                "case {case:?}: expected InvalidArgument, got {:?}",
                answer.error_code()
            );
            assert_eq!(
                answer.error_message(),
                format!("{header} header is not supported for this operation."),
                "case {case:?}: expected the sentence to name the header, got {:?}",
                answer.error_message()
            );
            assert_eq!(
                answer.element("ArgumentName").as_deref(),
                Some(header),
                "case {case:?}: the document names the argument it rejected, got {:?}",
                answer.element("ArgumentName")
            );
            assert_eq!(
                answer.element("ArgumentValue").as_deref(),
                Some(value),
                "case {case:?}: the document echoes the value as sent, got {:?}",
                answer.element("ArgumentValue")
            );
        }

        // And a read carrying none of the write-only four is served, so what is refused is
        // the header rather than the read.
        let served = drive(&router, read, name, key, &[]).await;
        assert_eq!(
            served.status,
            200,
            "case {read:?}: a read carrying no write-only header is served, got {} with body \
             {:?}",
            served.status,
            text(&served)
        );
    }
}

#[tokio::test]
async fn a_read_resolves_the_span_it_asks_for_before_the_key_it_presents() {
    // "Specifies the byte range of the object to get" (`GetObjectRequest$Range`): a range
    // outside the object asks for bytes the object does not have, which S3 answers with the
    // 416 that names the range and the object's size — and it answers that BEFORE the
    // encryption parameters the request presented, which is the order recorded for a read
    // that earns both refusals at once.
    let router = router();
    let name = "read-span-precedence";
    bucket(&router, name).await;
    let key = "data/one.csv";
    put_object(&router, name, key, BODY, &[]).await;

    // A well-formed customer key, which this object — stored under none — has nothing to do
    // with, so a read presenting it earns the "parameters are not applicable" refusal.
    let material: Vec<u8> = (0..32_u8).map(|index| index.wrapping_mul(37)).collect();
    let presented = ::base64::engine::general_purpose::STANDARD.encode(&material);
    let digest = ::base64::engine::general_purpose::STANDARD.encode(::md5::Md5::digest(&material));
    let customer_key: Vec<(&str, &str)> = vec![
        ("x-amz-server-side-encryption-customer-algorithm", "AES256"),
        ("x-amz-server-side-encryption-customer-key", &presented),
        ("x-amz-server-side-encryption-customer-key-md5", &digest),
    ];

    // (case, the read, the range it asks for, the status and code it earns). The rows differ
    // along whether the span can be served at all and which of the two refusals the request
    // earns, over both readers.
    for read in [Read::Object, Read::Metadata] {
        let unsatisfiable = format!("bytes={}-{}", BODY.len() + 10, BODY.len() + 20);
        let rows: [(&str, &str, u16, &str); 2] = [
            (
                "a range outside the object, beside a key the object has nothing to do with",
                &unsatisfiable,
                416,
                "InvalidRange",
            ),
            (
                "a range the object can serve, beside that same key",
                "bytes=0-1",
                400,
                "InvalidRequest",
            ),
        ];
        for (case, range, status, code) in rows {
            let case = format!("{read:?}: {case}");
            let mut headers = customer_key.clone();
            headers.push(("range", range));
            let answer = drive(&router, read, name, key, &headers).await;
            assert_eq!(
                answer.status,
                status,
                "case {case:?}: expected HTTP {status}, got {} with body {:?}",
                answer.status,
                text(&answer)
            );
            assert_eq!(
                answer.error_code(),
                code,
                "case {case:?}: expected {code}, got {:?}",
                answer.error_code()
            );
        }
    }
}

#[tokio::test]
async fn an_attributes_read_stating_no_field_is_refused_rather_than_answered_with_the_object() {
    let router = router();
    let name = "attributes-header";
    bucket(&router, name).await;
    let key = "data/one.csv";
    let written = put_object(&router, name, key, BODY, &[]).await;
    assert_eq!(
        written.status,
        200,
        "the setup write must be accepted, got {} with body {:?}",
        written.status,
        text(&written)
    );

    // The one way a request states no attribute at all: it sends no such header. A header
    // the request DID send names attributes — even sent empty, which names one that is not a
    // published attribute (below).
    {
        let case = "no header at all";
        let answer = get_object_attributes_with(&router, name, key, &[]).await;
        assert_eq!(
            answer.status,
            400,
            "case {case:?}: expected HTTP 400, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.header("content-type").as_deref(),
            Some("application/xml"),
            "case {case:?}: a refusal is an XML error document, got {:?}",
            answer.header("content-type")
        );
        assert_eq!(
            answer.error_code(),
            "InvalidRequest",
            "case {case:?}: expected InvalidRequest, got {:?}",
            answer.error_code()
        );
        assert_eq!(
            answer.error_message(),
            "The x-amz-object-attributes header specifying the attributes to be retrieved is \
             either missing or empty",
            "case {case:?}: expected the sentence S3 publishes for the header, got {:?}",
            answer.error_message()
        );
        assert!(
            !answer.body.as_ref().starts_with(BODY),
            "case {case:?}: a refusal must not carry the object's bytes, got {:?}",
            text(&answer)
        );
    }

    // A header that DOES name a field is served, and one whose fields are outside the
    // published set is the other refusal — so the rule above is about carrying no field and
    // not about the attributes call at large.
    let served = get_object_attributes_with(
        &router,
        name,
        key,
        &[("x-amz-object-attributes", "ETag")],
    )
    .await;
    assert_eq!(
        served.status,
        200,
        "case \"a header naming one field\": expected HTTP 200, got {} with body {:?}",
        served.status,
        text(&served)
    );
    for (case, header) in [
        ("a field outside the published set", "objectsize"),
        ("a field spelled in another case", "etag"),
        ("a header of separators alone", ","),
        ("a header sent empty", ""),
    ] {
        let unpublished = get_object_attributes_with(
            &router,
            name,
            key,
            &[("x-amz-object-attributes", header)],
        )
        .await;
        assert_eq!(
            (unpublished.status, unpublished.error_code()),
            (400, "InvalidArgument".to_string()),
            "case {case:?}: expected 400 InvalidArgument, got {} {:?}",
            unpublished.status,
            unpublished.error_code()
        );
        assert_eq!(
            unpublished.error_message(),
            "Invalid attribute name specified.",
            "case {case:?}: expected the sentence S3 publishes for an attribute that is not \
             one, got {:?}",
            unpublished.error_message()
        );
    }
}

#[tokio::test]
async fn an_attributes_read_of_a_deleted_key_names_the_delete_marker() {
    let router = router();
    let name = "attributes-delete-marker";
    bucket(&router, name).await;
    set_versioning(&router, name, "Enabled").await;
    let key = "data/one.csv";
    put_object(&router, name, key, BODY, &[]).await;
    let deleted = delete_object(&router, name, key, "").await;
    let marker = deleted
        .header("x-amz-version-id")
        .expect("the delete must report the id of the marker it laid");
    assert_eq!(
        deleted.header("x-amz-delete-marker").as_deref(),
        Some("true"),
        "the delete must report that it laid a marker, got {:?}",
        deleted.header("x-amz-delete-marker")
    );

    // (case, the query the read names the version with, the status it is answered)
    let rows: [(&str, String, u16); 2] = [
        ("the key, whose current version is the marker", String::new(), 404),
        (
            "the marker itself, by version id",
            format!("&versionId={marker}"),
            405,
        ),
    ];
    for (case, query, status) in rows {
        let answer = send_bytes(
            &router,
            "GET",
            &format!("/{name}/{key}?attributes{query}"),
            &[("x-amz-object-attributes", "ETag")],
            b"",
        )
        .await;
        assert_eq!(
            answer.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answer.status,
            text(&answer)
        );
        assert_eq!(
            answer.header("x-amz-delete-marker").as_deref(),
            Some("true"),
            "case {case:?}: the answer must say the version is a delete marker, got {:?}",
            answer.header("x-amz-delete-marker")
        );
        assert_eq!(
            answer.header("x-amz-version-id").as_deref(),
            Some(marker.as_str()),
            "case {case:?}: the answer must name the marker's version, got {:?}",
            answer.header("x-amz-version-id")
        );
    }
}
