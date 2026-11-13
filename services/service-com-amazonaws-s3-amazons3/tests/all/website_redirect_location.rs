// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the object member `WebsiteRedirectLocation`, bound to
//! `x-amz-website-redirect-location`.
//!
//! RULE: a write that carries the member is served only when the value is ADDRESSABLE — it
//! begins with `http://`, with `https://` or with `/` — because the value is the `Location` a
//! website endpoint would later answer with ("redirects requests for this object to another
//! object in the same bucket or to an external URL",
//! `PutObjectRequest$WebsiteRedirectLocation`). Every other value is refused HTTP 400
//! `InvalidRedirectLocation` with the fixed sentence that code publishes, and the refusal
//! stores nothing: the key the write named holds no object afterwards. An addressable value is
//! stored and reported back by every later read of the object.
//!
//! The rule is asserted over a TABLE of values spanning the member — both schemes, a path, a
//! path with a query, a scheme-relative value (which begins with the path prefix, so it is
//! admitted), a bare relative key, an empty value and a value whose prefix is only a
//! case-variant — and over EVERY write of this service that carries the member: the
//! single-request write, the copy and the initiation of a multipart upload. Each assertion
//! drives a real HTTP request through the router, so what is pinned is the wire behaviour
//! rather than any handler's internals.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The code S3 refuses an unaddressable redirect location with, and the status it is rendered
/// at: Amazon S3 *Error responses* lists `InvalidRedirectLocation` as a client error, which is
/// an HTTP 400.
const INVALID_REDIRECT_LOCATION: &str = "InvalidRedirectLocation";
/// See [`INVALID_REDIRECT_LOCATION`].
const REFUSAL_STATUS: u16 = 400;
/// The sentence that code carries, which is ENTIRELY fixed text: it states the requirement
/// rather than anything about the value submitted.
const REFUSAL_MESSAGE: &str =
    "The website redirect location must have a prefix of 'http://' or 'https://' or '/'.";

/// The code and sentence a read of a key no write created answers with — what proves a refused
/// write stored nothing.
const NO_SUCH_KEY: &str = "NoSuchKey";
/// See [`NO_SUCH_KEY`].
const NO_SUCH_KEY_MESSAGE: &str = "The specified key does not exist.";

/// The header the member is bound to
/// (`smithy_query(query="operation",operation="PutObject")` reports
/// `WebsiteRedirectLocation` on `x-amz-website-redirect-location`).
const REDIRECT_HEADER: &str = "x-amz-website-redirect-location";

/// One row of the value table: the case, the value the write carries, and whether S3 stores it.
struct ValueRow {
    case: &'static str,
    value: &'static str,
    addressable: bool,
}

impl ValueRow {
    /// A key naming this row, built of characters that travel through a key, a path and an XML
    /// document unchanged — so a listing that holds the key can be searched for it as written.
    fn key(&self, write: &str) -> String {
        let slug: String = self
            .case
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '-'
                }
            })
            .collect();
        format!("{write}/{slug}.html")
    }
}

/// The table. The three admitted prefixes come from the sentence the refusal publishes
/// (`http://`, `https://`, `/`); the rejected rows are values that begin with none of them,
/// including the ones a client most easily writes by mistake — a bare relative key, a
/// scheme-relative URL and another scheme — and the empty value, which has no prefix at all.
///
/// The prefix is matched as the sentence spells it, so a value differing only in the case of
/// its scheme is NOT one of the three: nothing folds case here, and the row pins that.
fn value_rows() -> Vec<ValueRow> {
    vec![
        ValueRow {
            case: "an absolute URL over http",
            value: "http://example.com/new.html",
            addressable: true,
        },
        ValueRow {
            case: "an absolute URL over https",
            value: "https://example.com/moved/new.html",
            addressable: true,
        },
        ValueRow {
            case: "an absolute URL with a port and a query",
            value: "https://example.com:8443/new.html?v=2",
            addressable: true,
        },
        ValueRow {
            case: "a path on the website host itself",
            value: "/new.html",
            addressable: true,
        },
        ValueRow {
            case: "the website host's root",
            value: "/",
            addressable: true,
        },
        ValueRow {
            case: "a deep path with a query",
            value: "/docs/new.html?lang=en",
            addressable: true,
        },
        ValueRow {
            case: "a bare relative key",
            value: "new.html",
            addressable: false,
        },
        ValueRow {
            case: "a relative path below the current one",
            value: "./docs/new.html",
            addressable: false,
        },
        ValueRow {
            // A scheme-relative URL begins with the path prefix, so it is one of the three
            // the sentence admits: the rule is the PREFIX, not a URL grammar.
            case: "a scheme-relative URL",
            value: "//example.com/new.html",
            addressable: true,
        },
        ValueRow {
            case: "another scheme",
            value: "ftp://example.com/new.html",
            addressable: false,
        },
        ValueRow {
            case: "a scheme in another casing",
            value: "HTTP://example.com/new.html",
            addressable: false,
        },
        ValueRow {
            case: "the empty value",
            value: "",
            addressable: false,
        },
    ]
}

/// `PUT /<bucket>/<key>` carrying `x-amz-copy-source` — `CopyObject`, the second write that
/// carries the member.
async fn copy_object(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    copy_source: &str,
    redirect: &str,
) -> Answer {
    send_bytes(
        router,
        "PUT",
        &format!("/{bucket_name}/{key}"),
        &[
            ("x-amz-copy-source", copy_source),
            (REDIRECT_HEADER, redirect),
        ],
        b"",
    )
    .await
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, the third.
async fn initiate_upload(
    router: &MultiServiceRouter,
    bucket_name: &str,
    key: &str,
    redirect: &str,
) -> Answer {
    send_bytes(
        router,
        "POST",
        &format!("/{bucket_name}/{key}?uploads"),
        &[(REDIRECT_HEADER, redirect)],
        b"",
    )
    .await
}

/// `GET /<bucket>?uploads` — the listing that says whether an initiation recorded an upload.
async fn list_uploads(router: &MultiServiceRouter, bucket_name: &str) -> Answer {
    send_bytes(router, "GET", &format!("/{bucket_name}?uploads"), &[], b"").await
}

/// Assert that `answered` is the refusal an unaddressable value earns.
fn refused(answered: &Answer, case: &str) {
    assert_eq!(
        answered.status, REFUSAL_STATUS,
        "case {case:?}: expected the write to be refused {REFUSAL_STATUS}, got {} with body \
         {:?}",
        answered.status,
        text(answered)
    );
    assert_eq!(
        answered.error_code(),
        INVALID_REDIRECT_LOCATION,
        "case {case:?}: expected the code {INVALID_REDIRECT_LOCATION}, got {:?} in body {:?}",
        answered.error_code(),
        text(answered)
    );
    assert_eq!(
        answered.error_message(),
        REFUSAL_MESSAGE,
        "case {case:?}: expected the fixed sentence, got {:?}",
        answered.error_message()
    );
}

/// Assert that nothing is stored at `key`: the read of it answers the code and sentence S3
/// answers a key that does not exist with.
async fn holds_nothing(router: &MultiServiceRouter, bucket_name: &str, key: &str, case: &str) {
    let read = get_object(router, bucket_name, key, "", &[]).await;
    assert_eq!(
        read.status, 404,
        "case {case:?}: a refused write must leave {key:?} uncreated, got {} with body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.error_code(),
        NO_SUCH_KEY,
        "case {case:?}: expected {NO_SUCH_KEY} for {key:?}, got {:?}",
        read.error_code()
    );
    assert_eq!(
        read.error_message(),
        NO_SUCH_KEY_MESSAGE,
        "case {case:?}: expected the sentence {NO_SUCH_KEY} publishes, got {:?}",
        read.error_message()
    );
}

#[tokio::test]
async fn a_single_request_write_stores_only_an_addressable_redirect_location() {
    let router = router_for_tests();
    let name = "website-redirect-location-bucket";
    bucket(&router, name).await;

    for row in value_rows() {
        let key = row.key("put");
        let written = put_object(
            &router,
            name,
            &key,
            b"body",
            &[(REDIRECT_HEADER, row.value)],
        )
        .await;

        if !row.addressable {
            refused(&written, row.case);
            holds_nothing(&router, name, &key, row.case).await;
            continue;
        }

        assert_eq!(
            written.status, 200,
            "case {:?}: expected the write to answer HTTP 200, got {} with body {:?}",
            row.case,
            written.status,
            text(&written)
        );
        // The stored value is reported back by a read of the object, which is what makes it a
        // property of the object rather than of the one response.
        let read = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {:?}: expected the read to answer HTTP 200, got {} with body {:?}",
            row.case,
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header(REDIRECT_HEADER).as_deref(),
            Some(row.value),
            "case {:?}: expected the read to report the stored redirect {:?}, got {:?}",
            row.case,
            row.value,
            read.header(REDIRECT_HEADER)
        );
    }
}

#[tokio::test]
async fn a_copy_stores_only_an_addressable_redirect_location() {
    let router = router_for_tests();
    let name = "website-redirect-copy-bucket";
    bucket(&router, name).await;
    let source = "source.html";
    let seeded = put_object(&router, name, source, b"source bytes", &[]).await;
    assert_eq!(
        seeded.status, 200,
        "the source object must be written first, got {} with body {:?}",
        seeded.status,
        text(&seeded)
    );
    let copy_source = format!("/{name}/{source}");

    for row in value_rows() {
        let key = row.key("copy");
        let copied = copy_object(&router, name, &key, &copy_source, row.value).await;

        if !row.addressable {
            refused(&copied, row.case);
            holds_nothing(&router, name, &key, row.case).await;
            continue;
        }

        assert_eq!(
            copied.status, 200,
            "case {:?}: expected the copy to answer HTTP 200, got {} with body {:?}",
            row.case,
            copied.status,
            text(&copied)
        );
        let read = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            read.header(REDIRECT_HEADER).as_deref(),
            Some(row.value),
            "case {:?}: expected the copy's object to report the redirect {:?}, got {:?}",
            row.case,
            row.value,
            read.header(REDIRECT_HEADER)
        );
    }
}

#[tokio::test]
async fn an_initiation_records_only_an_addressable_redirect_location() {
    let router = router_for_tests();
    let name = "website-redirect-upload-bucket";
    bucket(&router, name).await;

    for row in value_rows() {
        let key = row.key("mpu");
        let initiated = initiate_upload(&router, name, &key, row.value).await;

        if !row.addressable {
            refused(&initiated, row.case);
            // A refused initiation records no upload either: the key it named holds none
            // afterwards.
            let listed = list_uploads(&router, name).await;
            assert!(
                !text(&listed).contains(&key),
                "case {:?}: expected no upload recorded for {key:?}, got {:?}",
                row.case,
                text(&listed)
            );
            continue;
        }

        assert_eq!(
            initiated.status, 200,
            "case {:?}: expected the initiation to answer HTTP 200, got {} with body {:?}",
            row.case,
            initiated.status,
            text(&initiated)
        );
        let listed = list_uploads(&router, name).await;
        assert!(
            text(&listed).contains(&key),
            "case {:?}: expected the upload for {key:?} to be recorded, got {:?}",
            row.case,
            text(&listed)
        );
    }
}

/// The router every test above drives, built the way this crate's suites build one.
fn router_for_tests() -> MultiServiceRouter {
    super::bucket_lifecycle::router()
}
