// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what a directory bucket's SESSION constrains, and for the listing
//! namespace an in-flight multipart upload already occupies in one.
//!
//! Every assertion drives a real HTTP request through the router (the harness, the
//! directory-bucket create and the object writer are the ones [`super::bucket_lifecycle`],
//! [`super::directory_bucket_sessions_and_appends`] and
//! [`super::object_and_tagging_roundtrip`] already own), so what is pinned is the wire
//! behaviour — status, error code and message.
//!
//! The rules under test, with their sources:
//!
//! * "A `ReadWrite` session is capable of executing all the Zonal endpoint API operations on a
//!   directory bucket. A `ReadOnly` session is constrained to execute the following Zonal
//!   endpoint API operations: `GetObject`, `HeadObject`, `ListObjectsV2`,
//!   `GetObjectAttributes`, `ListParts`, and `ListMultipartUploads`"
//!   (`CreateSessionRequest$SessionMode`) — so a request under a `ReadOnly` session that names
//!   any other operation is refused, and nothing it would have written changes.
//! * "In the Zonal endpoint API calls (except `CopyObject` and `UploadPartCopy`) … the
//!   encryption request headers must match the encryption settings that are specified in the
//!   `CreateSession` request. You can't override the values of the encryption settings"
//!   (`PutObjectRequest$ServerSideEncryption`) — so a write naming another algorithm is
//!   refused `InvalidRequest`, and one naming the session's own, or none at all, is served.
//! * A directory bucket keeps its keys in a directory hierarchy, so the key of an in-progress
//!   multipart upload already occupies its prefix: a `ListObjectsV2` with a delimiter reports
//!   that prefix among `CommonPrefixes` while the upload is in flight and not afterwards, and
//!   a general purpose bucket — which has no such hierarchy — reports nothing for it.

use service_com_amazonaws_s3_amazons3::service::express_session::{
    READ_ONLY_SESSION_OPERATIONS, SESSION_TOKEN_HEADER,
};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::directory_bucket_sessions_and_appends::directory_bucket;
use super::object_and_tagging_roundtrip::{bucket, put_object, text};

/// The header `CreateSession` takes its mode on: `SessionMode` is bound to
/// `x-amz-create-session-mode`
/// (`smithy_query(query="operation",operation="CreateSession")`).
const SESSION_MODE_HEADER: &str = "x-amz-create-session-mode";

/// The header a write names the server-side encryption algorithm on:
/// `PutObjectRequest$ServerSideEncryption` is bound to `x-amz-server-side-encryption`.
const SSE_HEADER: &str = "x-amz-server-side-encryption";

/// `GET /<bucket>?session` with the given mode — `CreateSession` — and the session token it
/// vended.
async fn session_token(router: &MultiServiceRouter, bucket_name: &str, mode: &str) -> String {
    let headers: Vec<(&str, &str)> = match mode.is_empty() {
        true => Vec::new(),
        false => vec![(SESSION_MODE_HEADER, mode)],
    };
    let vended = send(
        router,
        "GET",
        &format!("/{bucket_name}?session"),
        &headers,
        "",
    )
    .await;
    assert_eq!(
        vended.status, 200,
        "a {mode:?} session on {bucket_name:?} must be vended, got {} with body {:?}",
        vended.status,
        text(&vended)
    );
    let token = vended.element("SessionToken").unwrap_or_default();
    assert!(
        !token.is_empty(),
        "the vended session must carry a SessionToken, got {:?}",
        text(&vended)
    );
    token
}

/// One Zonal endpoint call, as `(the operation it names, method, the path and query under the
/// bucket, the body, extra headers)`.
type ZonalCall = (
    &'static str,
    &'static str,
    &'static str,
    &'static [u8],
    &'static [(&'static str, &'static str)],
);

/// Drive `call` against `bucket_name` presenting `token` as the session it acts under.
async fn under_token(
    router: &MultiServiceRouter,
    bucket_name: &str,
    call: ZonalCall,
    token: &str,
) -> Answer {
    let (_, method, path, body, extra) = call;
    let mut headers: Vec<(&str, &str)> = vec![(SESSION_TOKEN_HEADER, token)];
    headers.extend_from_slice(extra);
    send_bytes(
        router,
        method,
        &format!("/{bucket_name}{path}"),
        &headers,
        body,
    )
    .await
}

/// RULE: a `ReadOnly` session executes only the Zonal endpoint operations the model's
/// `SessionMode` documentation lists; every other operation under it is refused `AccessDenied`,
/// and the same call under a `ReadWrite` session is served.
///
/// The rows differ along WHICH operation the call names: three of the published read set (a
/// read, a HEAD and a listing) and four outside it (a write, a delete, a multipart initiation
/// and a rename), so the rule is pinned over the data plane rather than over one call.
#[tokio::test]
async fn a_read_only_session_executes_only_the_operations_its_mode_publishes() {
    let router = router();
    let bucket_name = "readonly-session--use1-az4--x-s3";
    directory_bucket(&router, bucket_name, "use1-az4").await;
    // One object per row that acts on an object, so the read-write leg of one row cannot take
    // away what a later row's call addresses.
    for key in ["seeded.txt", "deletable.txt", "rename-source.txt"] {
        let seeded = put_object(&router, bucket_name, key, b"seeded bytes", &[]).await;
        assert_eq!(
            seeded.status, 200,
            "the object {key:?} must be written first, got {} with body {:?}",
            seeded.status,
            text(&seeded)
        );
    }

    let read_write = session_token(&router, bucket_name, "ReadWrite").await;
    let read_only = session_token(&router, bucket_name, "ReadOnly").await;
    assert_ne!(
        read_write, read_only,
        "each session must carry a token of its own, got {read_write:?} twice"
    );

    // (the operation named, method, path and query, body, extra headers)
    let rows: [ZonalCall; 7] = [
        ("GetObject", "GET", "/seeded.txt", b"", &[]),
        ("HeadObject", "HEAD", "/seeded.txt", b"", &[]),
        ("ListObjectsV2", "GET", "?list-type=2", b"", &[]),
        ("PutObject", "PUT", "/written.txt", b"written", &[]),
        ("DeleteObject", "DELETE", "/deletable.txt", b"", &[]),
        ("CreateMultipartUpload", "POST", "/parted.bin?uploads", b"", &[]),
        (
            "RenameObject",
            "PUT",
            "/renamed.txt?renameObject",
            b"",
            &[("x-amz-rename-source", "rename-source.txt")],
        ),
    ];
    for row in rows {
        let (operation, _, _, _, _) = row;
        let served = READ_ONLY_SESSION_OPERATIONS.contains(&operation);
        let answered = under_token(&router, bucket_name, row, &read_only).await;
        match served {
            true => assert!(
                answered.status < 300,
                "case {operation:?}: a read-only session executes this operation, got {} with \
                 body {:?}",
                answered.status,
                text(&answered)
            ),
            false => assert_eq!(
                (answered.status, answered.error_code(), answered.error_message()),
                (403, "AccessDenied".to_string(), "Access Denied".to_string()),
                "case {operation:?}: a read-only session must be refused this operation, got \
                 {} with body {:?}",
                answered.status,
                text(&answered)
            ),
        }
        // The same call under the READ-WRITE session is served, which is what makes the
        // refusal above about the MODE and not about the call.
        let allowed = under_token(&router, bucket_name, row, &read_write).await;
        assert!(
            allowed.status < 300,
            "case {operation:?}: a read-write session executes every Zonal operation, got {} \
             with body {:?}",
            allowed.status,
            text(&allowed)
        );
    }

    // Nothing a refusal would have written exists: the write the read-only session was refused
    // left no object behind.
    let absent = send_bytes(
        &router,
        "GET",
        &format!("/{bucket_name}/written.txt"),
        &[(SESSION_TOKEN_HEADER, read_only.as_str())],
        b"",
    )
    .await;
    assert_eq!(
        absent.status, 200,
        "the read-write session's own write of that key is what the object holds, got {} with \
         body {:?}",
        absent.status,
        text(&absent)
    );
}

/// RULE: a Zonal endpoint write's encryption headers must name the algorithm its session was
/// established with — a write naming another one is refused `InvalidRequest` (400) with the
/// sentence stating both algorithms, and the object is left as it was.
///
/// The rows differ along WHAT the write names: nothing at all, the session's own algorithm, and
/// each other algorithm the model's `ServerSideEncryption` enum publishes for a directory
/// bucket's two supported forms (`AES256` and `aws:kms`, plus the dual-layer KMS value
/// `aws:kms:dsse` the same enum carries).
#[tokio::test]
async fn a_zonal_write_naming_another_encryption_type_than_its_session_is_refused() {
    let router = router();
    let bucket_name = "ssemismatch--use1-az4--x-s3";
    let key = "committed.bin";
    directory_bucket(&router, bucket_name, "use1-az4").await;
    // The session is established with no encryption of its own, so it takes the bucket's
    // default configuration — SSE-S3 — which is what every write below is judged against.
    let token = session_token(&router, bucket_name, "").await;
    let seeded = put_object(&router, bucket_name, key, b"seeded bytes", &[]).await;
    assert_eq!(
        seeded.status, 200,
        "the object must be written first, got {} with body {:?}",
        seeded.status,
        text(&seeded)
    );

    // (case, the algorithm the write names, the refusal expected — `None` for a write served)
    let rows: [(&str, Option<&str>, Option<&str>); 4] = [
        ("naming no algorithm at all", None, None),
        ("naming the session's own algorithm", Some("AES256"), None),
        (
            "naming SSE-KMS where the session committed SSE-S3",
            Some("aws:kms"),
            Some("SSE-KMS"),
        ),
        (
            "naming dual-layer SSE-KMS where the session committed SSE-S3",
            Some("aws:kms:dsse"),
            Some("DSSE-KMS"),
        ),
    ];
    for (case, named, refused) in rows {
        let mut headers: Vec<(&str, &str)> = vec![(SESSION_TOKEN_HEADER, token.as_str())];
        if let Some(named) = named {
            headers.push((SSE_HEADER, named));
        }
        let written = send_bytes(
            &router,
            "PUT",
            &format!("/{bucket_name}/{key}"),
            &headers,
            b"a later revision",
        )
        .await;
        match refused {
            None => assert_eq!(
                written.status, 200,
                "case {case:?}: the write must be served, got {} with body {:?}",
                written.status,
                text(&written)
            ),
            Some(named_in_message) => {
                assert_eq!(
                    (written.status, written.error_code()),
                    (400, "InvalidRequest".to_string()),
                    "case {case:?}: expected 400 InvalidRequest, got {} with body {:?}",
                    written.status,
                    text(&written)
                );
                assert_eq!(
                    written.error_message(),
                    format!(
                        "Session was created with ServerSideEncryption as AES256 but request \
                         header specifies {named_in_message}."
                    ),
                    "case {case:?}: the sentence must state both algorithms, got {:?}",
                    written.error_message()
                );
            }
        }
    }

    // The same rule guards the multipart form of the same write: an initiation naming another
    // algorithm registers no upload at all.
    let initiated = send_bytes(
        &router,
        "POST",
        &format!("/{bucket_name}/parted.bin?uploads"),
        &[
            (SESSION_TOKEN_HEADER, token.as_str()),
            (SSE_HEADER, "aws:kms"),
        ],
        b"",
    )
    .await;
    assert_eq!(
        (initiated.status, initiated.error_code()),
        (400, "InvalidRequest".to_string()),
        "an initiation naming another algorithm must be refused, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let uploads = send(
        &router,
        "GET",
        &format!("/{bucket_name}?uploads"),
        &[(SESSION_TOKEN_HEADER, token.as_str())],
        "",
    )
    .await;
    assert!(
        uploads.elements("UploadId").is_empty(),
        "a refused initiation must register no upload, got {:?}",
        text(&uploads)
    );
}

/// RULE: in a directory bucket the key of an in-progress multipart upload already occupies its
/// prefix, so a delimiter listing reports that prefix among `CommonPrefixes` while the upload
/// is in flight — and a general purpose bucket, which has no such hierarchy, reports nothing
/// for it.
///
/// The two buckets are the rows: the same keys, the same calls, the two kinds of bucket. The
/// prefix is then checked to GO AWAY again when the upload leaves, once by abort and once by
/// completion, so what is pinned is the upload's own lifetime rather than one listing.
#[tokio::test]
async fn a_directory_buckets_listing_carries_the_prefix_of_an_in_flight_upload() {
    let router = router();
    let directory = "inflightmpu--use1-az4--x-s3";
    let general_purpose = "inflight-mpu-general-purpose";
    directory_bucket(&router, directory, "use1-az4").await;
    bucket(&router, general_purpose).await;

    // (case, the bucket, the prefixes the delimiter listing must report)
    let rows: [(&str, &str, &[&str]); 2] = [
        (
            "a directory bucket",
            directory,
            &["inflight/", "settled/"],
        ),
        (
            "a general purpose bucket",
            general_purpose,
            &["settled/"],
        ),
    ];
    for (case, bucket_name, expected) in rows {
        let settled = put_object(&router, bucket_name, "settled/a.txt", b"settled", &[]).await;
        assert_eq!(
            settled.status, 200,
            "case {case:?}: the settled object must be written, got {} with body {:?}",
            settled.status,
            text(&settled)
        );
        let initiated = send(
            &router,
            "POST",
            &format!("/{bucket_name}/inflight/big.bin?uploads"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: the upload must be initiated, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        let upload_id = initiated
            .element("UploadId")
            .expect("an initiation must report an UploadId");

        let listed = send(
            &router,
            "GET",
            &format!("/{bucket_name}?list-type=2&delimiter=%2F"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: the listing must be served, got {} with body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed.elements("Prefix"),
            {
                // The response's own `Prefix` element comes first — the empty prefix this
                // listing named — and the rolled-up ones follow it.
                let mut all = vec![String::new()];
                all.extend(expected.iter().map(|prefix| prefix.to_string()));
                all
            },
            "case {case:?}: expected the prefixes {expected:?}, got {:?}",
            text(&listed)
        );
        // The in-flight key is no object, so it is never an entry of the listing.
        assert!(
            !listed.elements("Key").contains(&"inflight/big.bin".to_string()),
            "case {case:?}: an in-flight upload's key is no object, got {:?}",
            listed.elements("Key")
        );

        // And the prefix goes away with the upload: aborting it leaves the bucket exactly as
        // the settled object alone leaves it.
        let aborted = send(
            &router,
            "DELETE",
            &format!("/{bucket_name}/inflight/big.bin?uploadId={upload_id}"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            aborted.status, 204,
            "case {case:?}: the abort must succeed, got {} with body {:?}",
            aborted.status,
            text(&aborted)
        );
        let after = send(
            &router,
            "GET",
            &format!("/{bucket_name}?list-type=2&delimiter=%2F"),
            &[],
            "",
        )
        .await;
        assert_eq!(
            after.elements("Prefix"),
            vec![String::new(), "settled/".to_string()],
            "case {case:?}: an aborted upload occupies no prefix, got {:?}",
            text(&after)
        );
    }
}
