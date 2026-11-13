// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the close of the multipart upload loop: what
//! `CompleteMultipartUpload` assembles is the object the key holds afterwards.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. A completion concatenates the parts it names in ascending part-number order into the
//!    object at the upload's key, whatever order those parts arrived in, and the object
//!    reads back — through `GetObject` and `HeadObject` — as exactly those bytes.
//! 2. The object's entity tag is the MULTIPART tag: the MD5 over the concatenated MD5
//!    digests of the parts, hex, with a dash and the number of parts after it — never the
//!    digest of the assembled bytes.
//! 3. A completed upload id names nothing afterwards: a read of its parts, and a second
//!    completion of it, are both 404 `NoSuchUpload`.
//! 4. A completion this emulator cannot carry out as asked is refused rather than written,
//!    and leaves the upload in progress with every part it held: a body naming no part with
//!    400 `MalformedXML`, a parts list that does not ascend with 400 `InvalidPartOrder`, a
//!    part number or entity tag that names no part of the upload with 400 `InvalidPart`, a
//!    part other than the last one below the published minimum part size with 400
//!    `EntityTooSmall`, an `x-amz-mp-object-size` that is not the size of the assembled
//!    parts with 400 `InvalidRequest`, an upload id the store never minted — or one named
//!    under another key — with 404 `NoSuchUpload`, a bucket this emulator does not hold with
//!    404 `NoSuchBucket`, and `x-amz-expected-bucket-owner` naming another account with 403.
//! 5. The conditional members are judged against what the key holds at the moment of the
//!    write: `If-None-Match: *` serves a key holding nothing and is refused with 412
//!    `PreconditionFailed` once the key holds an object, `If-Match` serves exactly the
//!    tag the key holds, and an `If-Match` at a key holding nothing — which has no tag to
//!    match — is refused 404 `NoSuchKey`.

use md5::{Digest, Md5};
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3
/// User Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// The highest part number a part may carry: "Part number that identifies the part. This is
/// a positive integer between 1 and 10,000" (`CompletedPart$PartNumber`, from
/// `smithy_query(query="shape",shape="com.amazonaws.s3#CompletedPart")`).
const MAX_PART_NUMBER: i32 = 10_000;

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, answering the upload id the
/// store minted.
async fn initiate(router: &MultiServiceRouter, name: &str, key: &str) -> String {
    initiate_with(router, name, key, &[]).await
}

/// The same initiation, carrying the bound headers a case names (the checksum members an
/// upload is configured with).
async fn initiate_with(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    headers: &[(&str, &str)],
) -> String {
    let initiated = send(
        router,
        "POST",
        &format!("/{name}/{key}?uploads"),
        headers,
        "",
    )
    .await;
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

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`, answering the entity
/// tag the store recorded for the part.
async fn upload_part(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    part_number: i32,
    body: &[u8],
) -> String {
    let uploaded = send_bytes(
        router,
        "PUT",
        &format!("/{name}/{key}?partNumber={part_number}&uploadId={upload_id}&x-id=UploadPart"),
        &[],
        body,
    )
    .await;
    assert_eq!(
        uploaded.status,
        200,
        "uploading part {part_number} must answer HTTP 200, got {} with body {:?}",
        uploaded.status,
        text(&uploaded)
    );
    uploaded
        .header("etag")
        .expect("a part upload must report an ETag header")
}

/// `POST /<bucket>/<key>?uploadId=<id>` — `CompleteMultipartUpload`, with the parts list as
/// the payload.
async fn complete(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
    payload: &str,
    headers: &[(&str, &str)],
) -> Answer {
    send(
        router,
        "POST",
        &format!("/{name}/{key}?uploadId={upload_id}"),
        headers,
        payload,
    )
    .await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`. The `x-id` literal is
/// the one the model binds that operation under, which is what distinguishes it from a read
/// of the object at the same address.
async fn list_parts(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    upload_id: &str,
) -> Answer {
    send(
        router,
        "GET",
        &format!("/{name}/{key}?uploadId={upload_id}&x-id=ListParts"),
        &[],
        "",
    )
    .await
}

/// `HEAD /<bucket>/<key>` — `HeadObject`.
async fn head_object(router: &MultiServiceRouter, name: &str, key: &str) -> Answer {
    send(router, "HEAD", &format!("/{name}/{key}"), &[], "").await
}

/// `GET /<bucket>/<key>?attributes` — `GetObjectAttributes`, asking for the root-level
/// fields `attributes` names and carrying the paging headers a case gives.
async fn get_object_attributes(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    attributes: &[&str],
    headers: &[(&str, &str)],
) -> Answer {
    let named = attributes.join(", ");
    let mut all: Vec<(&str, &str)> = vec![("x-amz-object-attributes", named.as_str())];
    all.extend_from_slice(headers);
    send(router, "GET", &format!("/{name}/{key}?attributes"), &all, "").await
}

/// Every `<Part>` a read of the object's attributes reports, as `(PartNumber, Size)` pairs
/// in the order the response carries them, beside the checksum element each one names.
fn reported_object_parts(answer: &Answer, checksum_element: &str) -> Vec<(i32, i64, Option<String>)> {
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
                element("Size")
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_default(),
                element(checksum_element),
            )
        })
        .collect()
}

/// How many `<Part>` elements a read of an upload reports.
fn reported_parts(answer: &Answer) -> usize {
    text(answer).matches("<Part>").count()
}

/// The parts-list payload naming each `(part number, entity tag)` pair in the order given.
///
/// The tag is written out exactly as the pair carries it, so a case can name it with the
/// double quotes S3 wraps a tag in or without them — both name the same part.
///
/// `pub(crate)`: the multipart notification suite
/// ([`super::multipart_upload_event_notifications`]) completes its uploads with the same
/// payload rather than keeping a second copy of the parts list.
pub(crate) fn completion_payload(parts: &[(i32, String)]) -> String {
    let mut payload = String::from("<CompleteMultipartUpload>");
    for (part_number, etag) in parts {
        payload.push_str(&format!(
            "<Part><PartNumber>{part_number}</PartNumber><ETag>{etag}</ETag></Part>"
        ));
    }
    payload.push_str("</CompleteMultipartUpload>");
    payload
}

/// The entity tag a multipart object of these parts must report, derived the way the Amazon
/// S3 User Guide (*Checking object integrity*) states it: "Amazon S3 calculates the MD5
/// digest of each individual part … concatenates the bytes for the MD5 digests together and
/// then calculates the MD5 digest of these concatenated values", with "a dash with the total
/// number of parts" appended. Computed here from the part bytes alone, in ascending
/// part-number order.
///
/// `pub(crate)`: the multipart notification suite
/// ([`super::multipart_upload_event_notifications`]) judges the entity tag a delivered
/// record reports against the same derivation, rather than keeping a second copy of it.
pub(crate) fn expected_multipart_etag(parts: &[Vec<u8>]) -> String {
    let mut digest = Md5::new();
    for part in parts {
        digest.update(Md5::digest(part));
    }
    format!("\"{}-{}\"", hex::encode(digest.finalize()), parts.len())
}

/// `length` bytes from a fixed seed, so a part is large and byte-varied without anything
/// random: a 32-bit LCG (the "ANSI C" multiplier and increment), taking the high byte of
/// each step.
fn filler(length: usize, seed: u32) -> Vec<u8> {
    let mut state = seed;
    (0..length)
        .map(|_| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            (state >> 16) as u8
        })
        .collect()
}

/// One row of the completion table: the case name, the key the upload is initiated for, and
/// the parts to upload as `(part number, bytes)` in the order they ARRIVE — deliberately not
/// always ascending, because the order the completion assembles in comes from its list.
struct CompletionCase {
    case: &'static str,
    key: &'static str,
    arrivals: Vec<(i32, Vec<u8>)>,
    /// Whether the completion names each part's tag with the double quotes the upload
    /// answered it with, or with them stripped.
    quoted: bool,
}

/// The completion table. The rows differ along what the rule generalises over: how many
/// parts the object is assembled from (one, two, three), the size of those parts (a handful
/// of bytes, exactly the published minimum, far above it), whether the part numbers are
/// consecutive or have gaps, where the published maximum part number sits in the list, the
/// shape of the key (a bare name, a nested prefix), the order the parts arrive in, and
/// whether the tags are quoted on the way back.
fn completion_cases() -> Vec<CompletionCase> {
    vec![
        CompletionCase {
            case: "one part, the last one, which no minimum size applies to",
            key: "single/part.bin",
            arrivals: vec![(1, b"hello multipart world".to_vec())],
            quoted: true,
        },
        CompletionCase {
            case: "two parts arriving last-first, the first exactly the published minimum",
            key: "nested/path/assembled.bin",
            arrivals: vec![
                (2, b"the tail bytes".to_vec()),
                (1, filler(MIN_MULTIPART_PART_BYTES, 7)),
            ],
            quoted: false,
        },
        CompletionCase {
            case: "three parts with gaps between their numbers, the last at the maximum",
            key: "gapped.bin",
            arrivals: vec![
                (7, filler(MIN_MULTIPART_PART_BYTES, 11)),
                (MAX_PART_NUMBER, b"end".to_vec()),
                (1, filler(MIN_MULTIPART_PART_BYTES + 1, 13)),
            ],
            quoted: true,
        },
        CompletionCase {
            case: "one part under the published maximum part number",
            key: "max-number.dat",
            arrivals: vec![(MAX_PART_NUMBER, filler(1_024, 17))],
            quoted: true,
        },
    ]
}

#[tokio::test]
async fn a_completion_assembles_its_parts_into_the_object_at_the_key() {
    for row in completion_cases() {
        let CompletionCase {
            case,
            key,
            arrivals,
            quoted,
        } = row;

        // One store per row: the parts and the object they become are both held in this
        // process's memory, and the rows are independent of one another.
        let router = router();
        let name = "multipart-completion-bucket";
        bucket(&router, name).await;
        let upload_id = initiate(&router, name, key).await;

        // The parts, recorded in the order the row names them.
        let mut recorded: Vec<(i32, String, Vec<u8>)> = Vec::new();
        for (part_number, body) in &arrivals {
            let etag = upload_part(&router, name, key, &upload_id, *part_number, body).await;
            recorded.push((*part_number, etag, body.clone()));
        }

        // The list the completion carries, in ascending part-number order.
        recorded.sort_by_key(|(part_number, _, _)| *part_number);
        let named: Vec<(i32, String)> = recorded
            .iter()
            .map(|(part_number, etag, _)| {
                let etag = if quoted {
                    etag.clone()
                } else {
                    etag.trim_matches('"').to_string()
                };
                (*part_number, etag)
            })
            .collect();
        let assembled: Vec<u8> = recorded
            .iter()
            .flat_map(|(_, _, body)| body.clone())
            .collect();
        let bodies: Vec<Vec<u8>> = recorded.iter().map(|(_, _, body)| body.clone()).collect();
        let expected_etag = expected_multipart_etag(&bodies);

        let completed = complete(
            &router,
            name,
            key,
            &upload_id,
            &completion_payload(&named),
            &[],
        )
        .await;
        assert_eq!(
            completed.status,
            200,
            "case {case:?}: expected HTTP 200 from CompleteMultipartUpload, got {} with \
             body {:?}",
            completed.status,
            text(&completed)
        );
        assert_eq!(
            completed.element("Bucket").as_deref(),
            Some(name),
            "case {case:?}: expected the completion to report the bucket it wrote into, \
             got {:?}",
            completed.element("Bucket")
        );
        assert_eq!(
            completed.element("Key").as_deref(),
            Some(key),
            "case {case:?}: expected the completion to report the key it wrote, got {:?}",
            completed.element("Key")
        );
        // "The URI that identifies the newly created object" — on the endpoint serving the
        // request, which for this harness is the base an emulator served on no port reports
        // (`smithy_server_runtime::UNSERVED_URL_BASE`). The rule itself is pinned over a
        // table of endpoint bases by [`super::announced_response_urls`].
        assert_eq!(
            completed.element("Location").as_deref(),
            Some(
                format!(
                    "{}/{name}/{key}",
                    smithy_server_runtime::UNSERVED_URL_BASE
                )
                .as_str()
            ),
            "case {case:?}: expected the URI of the new object on the serving endpoint, \
             got {:?}",
            completed.element("Location")
        );
        assert_eq!(
            completed.element("ETag").as_deref(),
            Some(expected_etag.as_str()),
            "case {case:?}: expected the multipart entity tag over the assembled parts, \
             got {:?}",
            completed.element("ETag")
        );
        assert_ne!(
            completed.element("ETag").as_deref(),
            Some(format!("\"{}\"", hex::encode(Md5::digest(&assembled))).as_str()),
            "case {case:?}: a multipart object's tag is not the digest of its bytes, got {:?}",
            completed.element("ETag")
        );

        // The object the parts became, read back through the readers of a stored object.
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.status,
            200,
            "case {case:?}: expected the assembled object to read back with HTTP 200, got \
             {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.body.len(),
            assembled.len(),
            "case {case:?}: expected the object to hold the {} assembled bytes, got {}",
            assembled.len(),
            read.body.len()
        );
        assert!(
            read.body.as_ref() == assembled.as_slice(),
            "case {case:?}: expected the object to be its parts concatenated in ascending \
             part-number order, and it is not"
        );
        assert_eq!(
            read.header("etag").as_deref(),
            Some(expected_etag.as_str()),
            "case {case:?}: expected the stored object to carry the multipart tag, got {:?}",
            read.header("etag")
        );

        let headed = head_object(&router, name, key).await;
        assert_eq!(
            headed.status,
            200,
            "case {case:?}: expected HeadObject to answer HTTP 200 for the assembled \
             object, got {}",
            headed.status
        );
        assert_eq!(
            headed.header("content-length").as_deref(),
            Some(assembled.len().to_string().as_str()),
            "case {case:?}: expected HeadObject to report the assembled size, got {:?}",
            headed.header("content-length")
        );
        assert_eq!(
            headed.header("etag").as_deref(),
            Some(expected_etag.as_str()),
            "case {case:?}: expected HeadObject to report the multipart tag, got {:?}",
            headed.header("etag")
        );

        // The upload id names nothing once the upload is completed.
        let listed = list_parts(&router, name, key, &upload_id).await;
        assert_eq!(
            listed.status,
            404,
            "case {case:?}: expected a read of the completed upload to be 404, got {} with \
             body {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            listed.error_code(),
            "NoSuchUpload",
            "case {case:?}: expected NoSuchUpload from a read of the completed upload, got \
             {:?}",
            listed.error_code()
        );
        let again = complete(
            &router,
            name,
            key,
            &upload_id,
            &completion_payload(&named),
            &[],
        )
        .await;
        assert_eq!(
            again.status,
            404,
            "case {case:?}: expected a second completion of one upload to be 404, got {} \
             with body {:?}",
            again.status,
            text(&again)
        );
        assert_eq!(
            again.error_code(),
            "NoSuchUpload",
            "case {case:?}: expected NoSuchUpload from the second completion, got {:?}",
            again.error_code()
        );
    }
}

/// One row of the refusal table: the case name, the address the completion is sent to (each
/// `None` meaning the row's own upload), the parts-list payload built from the two part tags
/// the row's upload holds, the headers the request carries, and the status and wire error
/// code it must answer.
struct RefusalCase {
    case: &'static str,
    bucket_override: Option<&'static str>,
    key_override: Option<&'static str>,
    upload_id_override: Option<&'static str>,
    payload: fn(&str, &str) -> String,
    headers: &'static [(&'static str, &'static str)],
    status: u16,
    code: &'static str,
    /// The `<Message>` the refusal must carry, when AWS publishes a FIXED text for that
    /// code; the empty string asks only that a message reached the wire at all.
    message: &'static str,
}

/// The refusal table. Every row drives a freshly initiated upload holding the SAME two
/// parts, so what differs between rows is only the way the completion is wrong.
fn refusal_cases() -> Vec<RefusalCase> {
    vec![
        RefusalCase {
            case: "a body naming no part at all",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, _| String::from("<CompleteMultipartUpload></CompleteMultipartUpload>"),
            headers: &[],
            status: 400,
            code: "MalformedXML",
            message: "The XML you provided was not well-formed or did not validate against \
                      our published schema",
        },
        RefusalCase {
            case: "the parts named in descending order",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |first, second| {
                completion_payload(&[(2, second.to_string()), (1, first.to_string())])
            },
            headers: &[],
            status: 400,
            code: "InvalidPartOrder",
            message: "The list of parts was not in ascending order. Parts must be ordered by \
                      part number.",
        },
        RefusalCase {
            case: "one part number named twice",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |first, _| {
                completion_payload(&[(1, first.to_string()), (1, first.to_string())])
            },
            headers: &[],
            status: 400,
            code: "InvalidPartOrder",
            message: "The list of parts was not in ascending order. Parts must be ordered by \
                      part number.",
        },
        RefusalCase {
            case: "a part number the upload holds nothing under",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |first, _| completion_payload(&[(3, first.to_string())]),
            headers: &[],
            status: 400,
            code: "InvalidPart",
            message: "One or more of the specified parts could not be found.  The part may \
                      not have been uploaded, or the specified entity tag may not match \
                      the part's entity tag.",
        },
        RefusalCase {
            case: "an entity tag that is not the named part's",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, second| completion_payload(&[(1, second.to_string())]),
            headers: &[],
            status: 400,
            code: "InvalidPart",
            message: "One or more of the specified parts could not be found.  The part may \
                      not have been uploaded, or the specified entity tag may not match \
                      the part's entity tag.",
        },
        RefusalCase {
            case: "a part other than the last one below the published minimum part size",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |first, second| {
                completion_payload(&[(1, first.to_string()), (2, second.to_string())])
            },
            headers: &[],
            status: 400,
            code: "EntityTooSmall",
            message: "Your proposed upload is smaller than the minimum allowed size",
        },
        RefusalCase {
            case: "an expected object size that is not the size of the assembled parts",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, second| completion_payload(&[(2, second.to_string())]),
            headers: &[("x-amz-mp-object-size", "999999")],
            status: 400,
            code: "InvalidRequest",
            message: "",
        },
        RefusalCase {
            case: "an upload id the store never minted",
            bucket_override: None,
            key_override: None,
            upload_id_override: Some("an-upload-id-nothing-minted"),
            payload: |first, _| completion_payload(&[(1, first.to_string())]),
            headers: &[],
            status: 404,
            code: "NoSuchUpload",
            message: "",
        },
        RefusalCase {
            case: "the upload id named under another key",
            bucket_override: None,
            key_override: Some("another/key.bin"),
            upload_id_override: None,
            payload: |first, _| completion_payload(&[(1, first.to_string())]),
            headers: &[],
            status: 404,
            code: "NoSuchUpload",
            message: "",
        },
        RefusalCase {
            case: "a bucket this emulator does not hold",
            bucket_override: Some("multipart-completion-no-such-bucket"),
            key_override: None,
            upload_id_override: None,
            payload: |first, _| completion_payload(&[(1, first.to_string())]),
            headers: &[],
            status: 404,
            code: "NoSuchBucket",
            message: "",
        },
        RefusalCase {
            case: "x-amz-expected-bucket-owner naming another account",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, second| completion_payload(&[(2, second.to_string())]),
            headers: &[("x-amz-expected-bucket-owner", "210987654321")],
            status: 403,
            code: "AccessDenied",
            message: "Access Denied",
        },
        RefusalCase {
            case: "SSE-C members that do not name one usable key",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, second| completion_payload(&[(2, second.to_string())]),
            headers: &[(
                "x-amz-server-side-encryption-customer-algorithm",
                "AES256",
            )],
            status: 400,
            code: "InvalidArgument",
            message: "",
        },
        RefusalCase {
            case: "an If-None-Match that is not the '*' the member expects",
            bucket_override: None,
            key_override: None,
            upload_id_override: None,
            payload: |_, second| completion_payload(&[(2, second.to_string())]),
            headers: &[("if-none-match", "\"not-a-wildcard\"")],
            status: 501,
            code: "NotImplemented",
            message: "",
        },
    ]
}

#[tokio::test]
async fn a_completion_it_cannot_assemble_is_refused_and_leaves_the_upload_in_progress() {
    let router = router();
    let name = "multipart-completion-refusal-bucket";
    bucket(&router, name).await;

    for (index, row) in refusal_cases().into_iter().enumerate() {
        let RefusalCase {
            case,
            bucket_override,
            key_override,
            upload_id_override,
            payload,
            headers,
            status,
            code,
            message,
        } = row;

        // Each row gets its own upload, under its own key, holding two parts of a handful of
        // bytes each — so the one row that names both of them is also the row whose first
        // part is below the published minimum part size.
        let key = format!("refusals/{index}/assembled.bin");
        let upload_id = initiate(&router, name, &key).await;
        let first = upload_part(&router, name, &key, &upload_id, 1, b"the first ten").await;
        let second = upload_part(&router, name, &key, &upload_id, 2, b"the second ten").await;

        let refused = complete(
            &router,
            bucket_override.unwrap_or(name),
            key_override.unwrap_or(&key),
            upload_id_override.unwrap_or(&upload_id),
            &payload(&first, &second),
            headers,
        )
        .await;
        assert_eq!(
            refused.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            refused.status,
            text(&refused)
        );
        assert_eq!(
            refused.error_code(),
            code,
            "case {case:?}: expected the wire code {code:?}, got {:?}",
            refused.error_code()
        );
        if message.is_empty() {
            assert!(
                !refused.error_message().is_empty(),
                "case {case:?}: expected the refusal to carry a Message, got body {:?}",
                text(&refused)
            );
        } else {
            assert_eq!(
                refused.error_message(),
                message,
                "case {case:?}: expected the published message for {code:?}, got {:?}",
                refused.error_message()
            );
        }

        // Nothing was written at the key the row's upload would have become, and the upload
        // is still in progress with both parts it held, so the caller can retry it.
        let read = get_object(&router, name, &key, "", &[]).await;
        assert_eq!(
            read.status, 404,
            "case {case:?}: expected the refused completion to write nothing at the key, \
             got {} with body {:?}",
            read.status,
            text(&read)
        );
        let listed = list_parts(&router, name, &key, &upload_id).await;
        assert_eq!(
            listed.status, 200,
            "case {case:?}: expected the upload to still be in progress, got {} with body \
             {:?}",
            listed.status,
            text(&listed)
        );
        assert_eq!(
            reported_parts(&listed),
            2,
            "case {case:?}: expected the upload to still hold its 2 parts, got {}",
            reported_parts(&listed)
        );
    }
}

/// One row of the checksum table: the `ChecksumAlgorithm` the upload is initiated with, the
/// `ChecksumType` the initiation names (when any), how many BITS that algorithm's digest is
/// wide, and the checksum type the completed object must carry.
struct ChecksumCase {
    algorithm: &'static str,
    requested_type: Option<&'static str>,
    digest_bits: usize,
    expected_type: &'static str,
}

/// The checksum table: every value `ChecksumAlgorithm` publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumAlgorithm")` reports
/// `[CRC32, CRC32C, SHA1, SHA256, CRC64NVME, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]`),
/// with the digest width each `Checksum*` member's own documentation publishes for it ("the
/// Base64 encoded, 32-bit CRC32 checksum of the object", "160-bit SHA1", "256-bit SHA256",
/// and so on, from
/// `smithy_query(query="shape",shape="com.amazonaws.s3#CompleteMultipartUploadOutput")`).
///
/// The rows differ along the algorithm, its digest width, and the checksum type: a
/// composition over the parts' checksums by default, a full-object checksum for the one
/// algorithm whose checksum "is always a full object checksum" (`ChecksumCRC64NVME`), and a
/// full-object checksum for an initiation that asks for one explicitly under a CRC-based
/// algorithm — the only algorithms a multipart object may carry one under (see
/// `tests/all/multipart_checksum_type.rs`, which tables every pair).
fn checksum_cases() -> Vec<ChecksumCase> {
    vec![
        ChecksumCase {
            algorithm: "CRC32",
            requested_type: None,
            digest_bits: 32,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "CRC32C",
            requested_type: None,
            digest_bits: 32,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "CRC64NVME",
            requested_type: None,
            digest_bits: 64,
            expected_type: "FULL_OBJECT",
        },
        ChecksumCase {
            algorithm: "SHA1",
            requested_type: None,
            digest_bits: 160,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "SHA256",
            requested_type: None,
            digest_bits: 256,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "SHA512",
            requested_type: None,
            digest_bits: 512,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "MD5",
            requested_type: None,
            digest_bits: 128,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "CRC32",
            requested_type: Some("FULL_OBJECT"),
            digest_bits: 32,
            expected_type: "FULL_OBJECT",
        },
        ChecksumCase {
            algorithm: "XXHASH64",
            requested_type: None,
            digest_bits: 64,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "XXHASH3",
            requested_type: None,
            digest_bits: 64,
            expected_type: "COMPOSITE",
        },
        ChecksumCase {
            algorithm: "XXHASH128",
            requested_type: None,
            digest_bits: 128,
            expected_type: "COMPOSITE",
        },
    ]
}

/// The response element that carries `algorithm`'s checksum, and the bound header the same
/// member travels in — `Checksum<ALGORITHM>` and `x-amz-checksum-<algorithm>`, which is the
/// `httpHeader` binding the model gives every one of those members
/// (`smithy_query(query="operation",operation="CompleteMultipartUpload")`).
fn checksum_member(algorithm: &str) -> (String, String) {
    (
        format!("Checksum{algorithm}"),
        format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase()),
    )
}

#[tokio::test]
async fn a_completed_object_carries_the_checksum_its_upload_was_initiated_with() {
    for row in checksum_cases() {
        let ChecksumCase {
            algorithm,
            requested_type,
            digest_bits,
            expected_type,
        } = row;
        let case = format!("{algorithm} as {expected_type}");

        let router = router();
        let name = "multipart-completion-checksum-bucket";
        bucket(&router, name).await;
        let key = "checksummed/assembled.bin";

        // Two parts, so a COMPOSITE value really is a composition: the first is exactly the
        // published minimum part size, and the last — exempt from it — is a handful of
        // bytes.
        let mut headers: Vec<(&str, &str)> = vec![("x-amz-checksum-algorithm", algorithm)];
        if let Some(requested) = requested_type {
            headers.push(("x-amz-checksum-type", requested));
        }
        let upload_id = initiate_with(&router, name, key, &headers).await;
        let parts = [filler(MIN_MULTIPART_PART_BYTES, 23), b"the tail".to_vec()];
        let mut named: Vec<(i32, String)> = Vec::new();
        for (index, body) in parts.iter().enumerate() {
            let number = i32::try_from(index).expect("a part index must be a part number") + 1;
            let etag = upload_part(&router, name, key, &upload_id, number, body).await;
            named.push((number, etag));
        }
        let assembled: Vec<u8> = parts.concat();

        let completed = complete(
            &router,
            name,
            key,
            &upload_id,
            &completion_payload(&named),
            &[],
        )
        .await;
        assert_eq!(
            completed.status,
            200,
            "case {case:?}: expected HTTP 200 from the completion, got {} with body {:?}",
            completed.status,
            text(&completed)
        );

        let (element, header) = checksum_member(algorithm);
        let reported = completed.element(&element).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the completion to report {element}, got body {:?}",
                text(&completed)
            )
        });
        assert_eq!(
            completed.element("ChecksumType").as_deref(),
            Some(expected_type),
            "case {case:?}: expected ChecksumType {expected_type:?}, got {:?}",
            completed.element("ChecksumType")
        );

        // An object carries the checksum of ONE algorithm — the one its upload named — so no
        // other member is reported beside it.
        for other in checksum_cases()
            .iter()
            .map(|row| checksum_member(row.algorithm).0)
            .filter(|other| *other != element)
        {
            assert_eq!(
                completed.element(&other),
                None,
                "case {case:?}: expected no {other} beside {element}, got {:?}",
                completed.element(&other)
            );
        }

        // The value is the algorithm's digest, Base64 encoded — with the number of parts
        // after a dash when it is a composition of their checksums, and nothing after it
        // when it is the checksum of the whole object.
        let (encoded, suffix) = match reported.split_once('-') {
            Some((encoded, parts_in_value)) => (encoded, Some(parts_in_value)),
            None => (reported.as_str(), None),
        };
        let expected_suffix = (expected_type == "COMPOSITE").then(|| parts.len().to_string());
        assert_eq!(
            suffix.map(str::to_string),
            expected_suffix,
            "case {case:?}: expected the part-count suffix {expected_suffix:?} on \
             {reported:?}, got {suffix:?}"
        );
        let digest = ::aws_smithy_types::base64::decode(encoded).unwrap_or_else(|error| {
            panic!("case {case:?}: {reported:?} must be Base64: {error}")
        });
        assert_eq!(
            digest.len() * 8,
            digest_bits,
            "case {case:?}: expected a {digest_bits}-bit digest, got {} bits from \
             {reported:?}",
            digest.len() * 8
        );

        // The two value checks the test can derive itself. An MD5 COMPOSITE checksum is the
        // digest the multipart entity tag renders in hex, so the two must agree; an MD5
        // FULL_OBJECT checksum is the digest of the assembled object's own bytes.
        if algorithm == "MD5" {
            let expected = if expected_type == "COMPOSITE" {
                let mut digests = Vec::new();
                for part in &parts {
                    digests.extend_from_slice(&Md5::digest(part));
                }
                Md5::digest(&digests).to_vec()
            } else {
                Md5::digest(&assembled).to_vec()
            };
            assert_eq!(
                hex::encode(&digest),
                hex::encode(&expected),
                "case {case:?}: expected the MD5 digest {:?}, got {:?}",
                hex::encode(&expected),
                hex::encode(&digest)
            );
        }
        // A SHA256 COMPOSITE checksum is the SHA-256 of the parts' SHA-256 digests,
        // computed here through the `sha2` crate rather than through anything the handler
        // uses.
        if algorithm == "SHA256" {
            use sha2::{Digest as _, Sha256};
            let mut digests = Vec::new();
            for part in &parts {
                digests.extend_from_slice(&Sha256::digest(part));
            }
            let expected = Sha256::digest(&digests).to_vec();
            assert_eq!(
                hex::encode(&digest),
                hex::encode(&expected),
                "case {case:?}: expected the SHA-256 digest {:?}, got {:?}",
                hex::encode(&expected),
                hex::encode(&digest)
            );
        }

        // Every later read of the object reports the same checksum — but only to a read that
        // asked for it: "to retrieve the checksum, this mode must be enabled"
        // (`GetObjectRequest$ChecksumMode`).
        let enabled = [("x-amz-checksum-mode", "ENABLED")];
        let headed = send(&router, "HEAD", &format!("/{name}/{key}"), &enabled, "").await;
        let read = get_object(&router, name, key, "", &enabled).await;
        for (reader, answer) in [("HeadObject", headed), ("GetObject", read)] {
            assert_eq!(
                answer.status, 200,
                "case {case:?}: expected {reader} to answer HTTP 200, got {}",
                answer.status
            );
            assert_eq!(
                answer.header(&header).as_deref(),
                Some(reported.as_str()),
                "case {case:?}: expected {reader} to report {header} as {reported:?}, got \
                 {:?}",
                answer.header(&header)
            );
            assert_eq!(
                answer.header("x-amz-checksum-type").as_deref(),
                Some(expected_type),
                "case {case:?}: expected {reader} to report the checksum type \
                 {expected_type:?}, got {:?}",
                answer.header("x-amz-checksum-type")
            );
        }
        let unasked = head_object(&router, name, key).await;
        assert_eq!(
            unasked.header(&header),
            None,
            "case {case:?}: a read that did not enable the checksum mode reports no \
             checksum, got {:?}",
            unasked.header(&header)
        );
        assert_eq!(
            unasked.header("x-amz-checksum-type"),
            None,
            "case {case:?}: a read that did not enable the checksum mode reports no \
             checksum type, got {:?}",
            unasked.header("x-amz-checksum-type")
        );
    }
}

#[tokio::test]
async fn a_read_of_the_object_s_attributes_reports_the_parts_it_was_assembled_from() {
    let router = router();
    let name = "multipart-completion-attributes-bucket";
    bucket(&router, name).await;
    let key = "attributes/assembled.bin";

    // A three-part upload under a checksum algorithm, so every part has a checksum of its
    // own to report: the first two are the published minimum, and the last is exempt.
    let upload_id = initiate_with(
        &router,
        name,
        key,
        &[("x-amz-checksum-algorithm", "SHA256")],
    )
    .await;
    let parts = [
        filler(MIN_MULTIPART_PART_BYTES, 31),
        filler(MIN_MULTIPART_PART_BYTES, 37),
        b"the tail of it".to_vec(),
    ];
    let numbers = [1, 4, 9];
    let mut named: Vec<(i32, String)> = Vec::new();
    for (number, body) in numbers.iter().zip(parts.iter()) {
        let etag = upload_part(&router, name, key, &upload_id, *number, body).await;
        named.push((*number, etag));
    }
    let completed = complete(
        &router,
        name,
        key,
        &upload_id,
        &completion_payload(&named),
        &[],
    )
    .await;
    assert_eq!(
        completed.status,
        200,
        "the completion must answer HTTP 200, got {} with body {:?}",
        completed.status,
        text(&completed)
    );
    let object_checksum = completed
        .element("ChecksumSHA256")
        .expect("the completion must report the object's SHA256 checksum");

    // Every root-level field asked for, over the object a multipart upload assembled.
    let attributes = ["ETag", "Checksum", "ObjectParts", "ObjectSize"];
    let read = get_object_attributes(&router, name, key, &attributes, &[]).await;
    assert_eq!(
        read.status,
        200,
        "case \"every attribute of an assembled object\": expected HTTP 200, got {} with \
         body {:?}",
        read.status,
        text(&read)
    );
    assert_eq!(
        read.element("ObjectSize")
            .and_then(|size| size.parse::<usize>().ok()),
        Some(parts.concat().len()),
        "case \"every attribute of an assembled object\": expected the assembled size, got \
         {:?}",
        read.element("ObjectSize")
    );
    assert_eq!(
        read.element("PartsCount")
            .and_then(|count| count.parse::<usize>().ok()),
        Some(parts.len()),
        "case \"every attribute of an assembled object\": expected {} parts, got {:?}",
        parts.len(),
        read.element("PartsCount")
    );
    assert_eq!(
        read.element("ChecksumSHA256").as_deref(),
        Some(
            object_checksum
                .rsplit_once('-')
                .map(|(digest, _)| digest)
                .unwrap_or(object_checksum.as_str())
        ),
        "case \"every attribute of an assembled object\": this reader states the part count \
         in its own `TotalPartsCount`, so the checksum container carries the composition's \
         digest alone, got {:?}",
        read.element("ChecksumSHA256")
    );
    assert_eq!(
        read.element("IsTruncated").as_deref(),
        Some("false"),
        "case \"every attribute of an assembled object\": a page carrying every part is not \
         truncated, got {:?}",
        read.element("IsTruncated")
    );
    let reported = reported_object_parts(&read, "ChecksumSHA256");
    let expected: Vec<(i32, i64)> = numbers
        .iter()
        .zip(parts.iter())
        .map(|(number, body)| {
            (
                *number,
                i64::try_from(body.len()).expect("a part size must fit a long"),
            )
        })
        .collect();
    assert_eq!(
        reported
            .iter()
            .map(|(number, size, _)| (*number, *size))
            .collect::<Vec<(i32, i64)>>(),
        expected,
        "case \"every attribute of an assembled object\": expected the parts {expected:?} \
         in ascending part-number order, got {reported:?}"
    );
    for (number, _, checksum) in &reported {
        assert!(
            checksum.is_some(),
            "case \"part {number}\": a part of an upload initiated with a checksum \
             algorithm reports its own checksum, got {checksum:?}"
        );
    }

    // One part per page: the page is the parts above the marker, and a page with parts left
    // over reports the last one it carried as the next marker.
    let first = get_object_attributes(
        &router,
        name,
        key,
        &["ObjectParts"],
        &[("x-amz-max-parts", "1")],
    )
    .await;
    assert_eq!(
        first.element("IsTruncated").as_deref(),
        Some("true"),
        "case \"a page of one part\": expected a truncated page, got {:?}",
        first.element("IsTruncated")
    );
    assert_eq!(
        first.element("NextPartNumberMarker").as_deref(),
        Some("1"),
        "case \"a page of one part\": expected the first part as the next marker, got {:?}",
        first.element("NextPartNumberMarker")
    );
    assert_eq!(
        reported_object_parts(&first, "ChecksumSHA256")
            .iter()
            .map(|(number, _, _)| *number)
            .collect::<Vec<i32>>(),
        vec![1],
        "case \"a page of one part\": expected part 1 alone, got {:?}",
        reported_object_parts(&first, "ChecksumSHA256")
    );
    let next = get_object_attributes(
        &router,
        name,
        key,
        &["ObjectParts"],
        &[("x-amz-part-number-marker", "1")],
    )
    .await;
    assert_eq!(
        reported_object_parts(&next, "ChecksumSHA256")
            .iter()
            .map(|(number, _, _)| *number)
            .collect::<Vec<i32>>(),
        vec![4, 9],
        "case \"the parts above the marker\": expected the parts after 1, got {:?}",
        reported_object_parts(&next, "ChecksumSHA256")
    );

    // An object one write stored whole was assembled from no parts, so there is no
    // collection to report for it. Its CHECKSUM is the default S3 adds to an upload that
    // names no algorithm (`PutObjectOutput$ChecksumCRC64NVME`), over the whole object.
    let whole = "attributes/written-whole.bin";
    let put = put_object(&router, name, whole, b"stored by one write", &[]).await;
    assert_eq!(
        put.status,
        200,
        "the one-shot write must succeed, got {} with body {:?}",
        put.status,
        text(&put)
    );
    let plain = get_object_attributes(&router, name, whole, &attributes, &[]).await;
    assert_eq!(
        plain.status, 200,
        "case \"an object stored by one write\": expected HTTP 200, got {}",
        plain.status
    );
    for absent in ["PartsCount", "ChecksumSHA256"] {
        assert_eq!(
            plain.element(absent),
            None,
            "case \"an object stored by one write\": expected no {absent}, got {:?}",
            plain.element(absent)
        );
    }
    assert_eq!(
        plain.element("ChecksumType").as_deref(),
        Some("FULL_OBJECT"),
        "case \"an object stored by one write\": its checksum is over the whole object, got \
         {:?}",
        plain.element("ChecksumType")
    );
    assert_eq!(
        plain.element("ChecksumCRC64NVME"),
        service_com_amazonaws_s3_amazons3::state::full_object_checksum(
            "CRC64NVME",
            b"stored by one write"
        ),
        "case \"an object stored by one write\": the default checksum of the stored bytes \
         must be reported, got {:?}",
        plain.element("ChecksumCRC64NVME")
    );
    assert!(
        plain.element("ObjectSize").is_some(),
        "case \"an object stored by one write\": the fields it does have are still \
         reported, got body {:?}",
        text(&plain)
    );

    // A multipart object whose upload named NO checksum algorithm: "if an additional
    // checksum … isn't applied to the object specified in the request, the response doesn't
    // return the Part element" (`GetObjectAttributesParts$Parts`), so how many parts it was
    // assembled from is reported and none of the parts themselves.
    let unchecksummed = "attributes/no-checksum.bin";
    let plain_upload = initiate(&router, name, unchecksummed).await;
    let plain_parts = [filler(MIN_MULTIPART_PART_BYTES, 41), b"tail".to_vec()];
    let mut plain_named: Vec<(i32, String)> = Vec::new();
    for (index, body) in plain_parts.iter().enumerate() {
        let number = i32::try_from(index).expect("a part index must be a part number") + 1;
        let etag = upload_part(&router, name, unchecksummed, &plain_upload, number, body).await;
        plain_named.push((number, etag));
    }
    let plain_completed = complete(
        &router,
        name,
        unchecksummed,
        &plain_upload,
        &completion_payload(&plain_named),
        &[],
    )
    .await;
    assert_eq!(
        plain_completed.status,
        200,
        "the completion of the unchecksummed upload must answer HTTP 200, got {} with body \
         {:?}",
        plain_completed.status,
        text(&plain_completed)
    );
    let plain_attributes =
        get_object_attributes(&router, name, unchecksummed, &attributes, &[]).await;
    assert_eq!(
        plain_attributes
            .element("PartsCount")
            .and_then(|count| count.parse::<usize>().ok()),
        Some(plain_parts.len()),
        "case \"a multipart object with no additional checksum\": expected {} parts, got \
         {:?}",
        plain_parts.len(),
        plain_attributes.element("PartsCount")
    );
    assert_eq!(
        reported_object_parts(&plain_attributes, "ChecksumSHA256"),
        Vec::new(),
        "case \"a multipart object with no additional checksum\": expected no Part element, \
         got {:?}",
        reported_object_parts(&plain_attributes, "ChecksumSHA256")
    );
}

/// One row of the conditional table: the case name, whether the key already holds an object
/// when the completion is made, the conditional header the request carries (its value
/// resolved from the tag that object holds for the `If-Match` rows), and the status and code
/// the completion must answer.
struct ConditionCase {
    case: &'static str,
    key_holds_an_object: bool,
    header: &'static str,
    /// The header's value, or `None` to send the tag the key's object really holds.
    value: Option<&'static str>,
    status: u16,
    code: &'static str,
}

/// The conditional table: both members, each on a key that holds an object and on one that
/// does not, and `If-Match` against a tag the key does not hold.
fn condition_cases() -> Vec<ConditionCase> {
    vec![
        ConditionCase {
            case: "If-None-Match '*' onto a key holding nothing",
            key_holds_an_object: false,
            header: "if-none-match",
            value: Some("*"),
            status: 200,
            code: "",
        },
        ConditionCase {
            case: "If-None-Match '*' onto a key that appeared while the parts were uploaded",
            key_holds_an_object: true,
            header: "if-none-match",
            value: Some("*"),
            status: 412,
            code: "PreconditionFailed",
        },
        ConditionCase {
            case: "If-Match the tag the key really holds",
            key_holds_an_object: true,
            header: "if-match",
            value: None,
            status: 200,
            code: "",
        },
        ConditionCase {
            case: "If-Match a tag the key does not hold",
            key_holds_an_object: true,
            header: "if-match",
            value: Some("\"00000000000000000000000000000000\""),
            status: 412,
            code: "PreconditionFailed",
        },
        ConditionCase {
            case: "If-Match onto a key holding nothing, which has no tag to match",
            key_holds_an_object: false,
            header: "if-match",
            value: Some("\"00000000000000000000000000000000\""),
            status: 404,
            code: "NoSuchKey",
        },
    ]
}

#[tokio::test]
async fn a_conditional_completion_is_judged_against_what_the_key_holds() {
    for row in condition_cases() {
        let ConditionCase {
            case,
            key_holds_an_object,
            header,
            value,
            status,
            code,
        } = row;

        let router = router();
        let name = "multipart-completion-conditional-bucket";
        bucket(&router, name).await;
        let key = "conditional/assembled.bin";

        // What the key holds when the completion is made: an object another write put there
        // while the parts were being uploaded, or nothing at all.
        let standing = b"the object that was already there".to_vec();
        let mut held_etag = String::new();
        if key_holds_an_object {
            let put = put_object(&router, name, key, &standing, &[]).await;
            assert_eq!(
                put.status,
                200,
                "case {case:?}: the standing object must be written first, got {} with body \
                 {:?}",
                put.status,
                text(&put)
            );
            held_etag = put
                .header("etag")
                .expect("a write must report an ETag header");
        }

        let upload_id = initiate(&router, name, key).await;
        let part = b"the assembled object".to_vec();
        let etag = upload_part(&router, name, key, &upload_id, 1, &part).await;
        let header_value = value.map(str::to_string).unwrap_or(held_etag);

        let answered = complete(
            &router,
            name,
            key,
            &upload_id,
            &completion_payload(&[(1, etag)]),
            &[(header, header_value.as_str())],
        )
        .await;
        assert_eq!(
            answered.status, status,
            "case {case:?}: expected HTTP {status}, got {} with body {:?}",
            answered.status,
            text(&answered)
        );

        let read = get_object(&router, name, key, "", &[]).await;
        if code.is_empty() {
            // The condition held, so the key holds the assembled object now.
            assert_eq!(
                answered.element("ETag").as_deref(),
                Some(expected_multipart_etag(std::slice::from_ref(&part)).as_str()),
                "case {case:?}: expected the multipart tag of the assembled part, got {:?}",
                answered.element("ETag")
            );
            assert!(
                read.body.as_ref() == part.as_slice(),
                "case {case:?}: expected the key to hold the assembled object afterwards, \
                 and it does not"
            );
        } else {
            assert_eq!(
                answered.error_code(),
                code,
                "case {case:?}: expected the wire code {code:?}, got {:?}",
                answered.error_code()
            );
            // Each refusal carries the fixed sentence of its own class: the precondition
            // that did not hold, or the key that is not there.
            let expected_message = match code {
                "NoSuchKey" => "The specified key does not exist.",
                _ => "At least one of the pre-conditions you specified did not hold",
            };
            assert_eq!(
                answered.error_message(),
                expected_message,
                "case {case:?}: expected the published message for {code:?}, got {:?}",
                answered.error_message()
            );
            // A refused completion writes nothing: the key holds exactly what it held
            // before, and the upload is still in progress.
            if key_holds_an_object {
                assert!(
                    read.body.as_ref() == standing.as_slice(),
                    "case {case:?}: expected the standing object to be left exactly as it \
                     was, and it is not"
                );
            } else {
                assert_eq!(
                    read.status, 404,
                    "case {case:?}: expected the key to still hold nothing, got {}",
                    read.status
                );
            }
            let listed = list_parts(&router, name, key, &upload_id).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected the upload to still be in progress, got {} with \
                 body {:?}",
                listed.status,
                text(&listed)
            );
        }
    }
}
