// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the `ChecksumType` a multipart upload is SETTLED with: which type
//! `CreateMultipartUpload` settles for the algorithm it names, which (algorithm, type) pairs
//! it refuses, and that the settled type is the one every later reader of the upload reports
//! and the one the completion composes its value under.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour (status, bound headers, payload elements) rather than any handler's
//! internals.
//!
//! The rules under test, each asserted over a SET of inputs rather than one request:
//!
//! 1. An initiation naming a checksum algorithm settles a checksum type for the upload, even
//!    when the request names none: `FULL_OBJECT` for an algorithm whose checksum "is always
//!    a full object checksum" (`ChecksumCRC64NVME`), and `COMPOSITE` — the composition over
//!    the parts' checksums — for every other algorithm. The settled type is what the
//!    initiation reports and what a read of the upload reports.
//! 2. An initiation asking for a type the algorithm does not admit is refused 400
//!    `InvalidRequest`, carrying the sentence that names the TYPE the request asked for and
//!    the ALGORITHM it named, and records no upload: a full-object checksum of a multipart
//!    object is only available under the CRC-based algorithms (the ones whose part checksums
//!    combine into the whole object's), and the one algorithm that is always a full-object
//!    checksum admits no composition.
//! 3. The settled type is the one the completed object carries, with the part-count suffix a
//!    composition carries and none on a full-object checksum — and a read of the object
//!    reports the same pair.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, text};

/// The two values `ChecksumType` publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumType")` reports `[COMPOSITE,
/// FULL_OBJECT]`) — the whole space an initiation may ask for.
const COMPOSITE: &str = "COMPOSITE";
/// See [`COMPOSITE`].
const FULL_OBJECT: &str = "FULL_OBJECT";

/// The code and status S3 answers "a parameter or header in your request isn't valid" with:
/// `InvalidRequest`, HTTP 400 (Amazon S3 *Error responses*, list of error codes).
const INVALID_REQUEST: &str = "InvalidRequest";

/// The sentence S3 refuses an (algorithm, type) pair that cannot exist with: the fixed text
/// "The ", " checksum type cannot be used with the " and " checksum algorithm.", with the type
/// as the request stated it and the algorithm rendered lower case — the spelling the
/// `x-amz-checksum-*` member names carry it in. Written here from the published sentence rather
/// than read off the handler, and in both directions: the type a pair is impossible in is
/// whichever one the request asked for.
fn incompatible_pair_message(checksum_type: &str, algorithm: &str) -> String {
    format!(
        "The {checksum_type} checksum type cannot be used with the {} checksum algorithm.",
        algorithm.to_ascii_lowercase()
    )
}

/// One row of the checksum-type table: a `ChecksumAlgorithm` value, the type an initiation
/// naming it and no type settles, and which of the two types it admits when a request asks
/// for one explicitly.
struct AlgorithmCase {
    algorithm: &'static str,
    settled_without_a_request: &'static str,
    admits_composite: bool,
    admits_full_object: bool,
}

/// The table: EVERY value `ChecksumAlgorithm` publishes
/// (`smithy_query(query="shape",shape="com.amazonaws.s3#ChecksumAlgorithm")` reports
/// `[CRC32, CRC32C, SHA1, SHA256, CRC64NVME, SHA512, MD5, XXHASH64, XXHASH3, XXHASH128]`),
/// including the values the issue cited nowhere, so the rows span the member rather than one
/// recorded value.
///
/// The expectations are the published rule, not a second copy of the handler's code:
///
/// * `CRC64NVME`'s checksum "is always a full object checksum"
///   (`CompleteMultipartUploadOutput$ChecksumCRC64NVME`), so it settles `FULL_OBJECT` and
///   admits no composition;
/// * a full-object checksum of a multipart object is settled from its parts, so it is only
///   available under the CRC-based algorithms the Amazon S3 User Guide (*Checking object
///   integrity*) publishes full object checksums for — `CRC32`, `CRC32C` and `CRC64NVME`;
/// * every other algorithm settles the `COMPOSITE` checksum that "determines how part-level
///   checksums are combined to create an object-level checksum for multipart objects"
///   (`CreateMultipartUploadRequest$ChecksumType`), and admits nothing else.
fn algorithm_cases() -> Vec<AlgorithmCase> {
    vec![
        AlgorithmCase {
            algorithm: "CRC32",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: true,
        },
        AlgorithmCase {
            algorithm: "CRC32C",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: true,
        },
        AlgorithmCase {
            algorithm: "CRC64NVME",
            settled_without_a_request: FULL_OBJECT,
            admits_composite: false,
            admits_full_object: true,
        },
        AlgorithmCase {
            algorithm: "SHA1",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "SHA256",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "SHA512",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "MD5",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "XXHASH64",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "XXHASH3",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
        AlgorithmCase {
            algorithm: "XXHASH128",
            settled_without_a_request: COMPOSITE,
            admits_composite: true,
            admits_full_object: false,
        },
    ]
}

/// `POST /<bucket>/<key>?uploads` — `CreateMultipartUpload`, carrying the checksum headers a
/// row names.
async fn initiate(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    algorithm: &str,
    requested_type: Option<&str>,
) -> Answer {
    let mut headers: Vec<(&str, &str)> = vec![("x-amz-checksum-algorithm", algorithm)];
    if let Some(requested) = requested_type {
        headers.push(("x-amz-checksum-type", requested));
    }
    send(router, "POST", &format!("/{name}/{key}?uploads"), &headers, "").await
}

/// `GET /<bucket>/<key>?uploadId=<id>&x-id=ListParts` — `ListParts`, the read-back of the
/// configuration an initiation settled.
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

/// `GET /<bucket>?uploads` — `ListMultipartUploads`, which is what shows whether a refused
/// initiation recorded an upload after all.
async fn list_uploads(router: &MultiServiceRouter, name: &str) -> Answer {
    send(router, "GET", &format!("/{name}?uploads"), &[], "").await
}

/// `PUT /<bucket>/<key>?partNumber=<n>&uploadId=<id>` — `UploadPart`, answering the entity
/// tag the store recorded.
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

#[tokio::test]
async fn an_initiation_settles_the_checksum_type_its_algorithm_admits() {
    let router = router();
    let name = "multipart-checksum-type-bucket";
    bucket(&router, name).await;

    for row in algorithm_cases() {
        // The three things a request may say about the type: nothing at all, and each of the
        // two values the enum publishes.
        for requested in [None, Some(COMPOSITE), Some(FULL_OBJECT)] {
            let admitted = match requested {
                None => Some(row.settled_without_a_request),
                Some(COMPOSITE) if row.admits_composite => Some(COMPOSITE),
                Some(FULL_OBJECT) if row.admits_full_object => Some(FULL_OBJECT),
                _ => None,
            };
            let case = format!("{} asked for {requested:?}", row.algorithm);
            let key = format!(
                "settled/{}-{}.bin",
                row.algorithm.to_ascii_lowercase(),
                requested.unwrap_or("none").to_ascii_lowercase()
            );

            let initiated = initiate(&router, name, &key, row.algorithm, requested).await;

            let Some(settled) = admitted else {
                assert_eq!(
                    initiated.status, 400,
                    "case {case:?}: expected the initiation to be refused 400, got {} with \
                     body {:?}",
                    initiated.status,
                    text(&initiated)
                );
                assert_eq!(
                    initiated.error_code(),
                    INVALID_REQUEST,
                    "case {case:?}: expected the code {INVALID_REQUEST}, got {:?} in body {:?}",
                    initiated.error_code(),
                    text(&initiated)
                );
                assert_eq!(
                    initiated.error_message(),
                    incompatible_pair_message(requested.unwrap_or_default(), row.algorithm),
                    "case {case:?}: expected the sentence that names the pair, got {:?}",
                    initiated.error_message()
                );
                // A refused initiation records nothing: the key it named holds no upload at
                // all afterwards.
                let listed = list_uploads(&router, name).await;
                assert!(
                    !text(&listed).contains(&key),
                    "case {case:?}: expected no upload recorded for {key:?}, got {:?}",
                    text(&listed)
                );
                continue;
            };

            assert_eq!(
                initiated.status, 200,
                "case {case:?}: expected the initiation to answer HTTP 200, got {} with body \
                 {:?}",
                initiated.status,
                text(&initiated)
            );
            assert_eq!(
                initiated.header("x-amz-checksum-type").as_deref(),
                Some(settled),
                "case {case:?}: expected the initiation to report the settled type \
                 {settled:?}, got {:?}",
                initiated.header("x-amz-checksum-type")
            );
            assert_eq!(
                initiated.header("x-amz-checksum-algorithm").as_deref(),
                Some(row.algorithm),
                "case {case:?}: expected the initiation to report the algorithm it was given, \
                 got {:?}",
                initiated.header("x-amz-checksum-algorithm")
            );

            // The SAME pair reads back off the recorded upload, which is what makes the
            // settled type a property of the upload rather than of the one response.
            let upload_id = initiated
                .element("UploadId")
                .unwrap_or_else(|| panic!("case {case:?}: an initiation must report an UploadId"));
            let listed = list_parts(&router, name, &key, &upload_id).await;
            assert_eq!(
                listed.status, 200,
                "case {case:?}: expected the read of the upload to answer HTTP 200, got {} with \
                 body {:?}",
                listed.status,
                text(&listed)
            );
            assert_eq!(
                listed.element("ChecksumType").as_deref(),
                Some(settled),
                "case {case:?}: expected the read of the upload to report ChecksumType \
                 {settled:?}, got {:?}",
                listed.element("ChecksumType")
            );
            assert_eq!(
                listed.element("ChecksumAlgorithm").as_deref(),
                Some(row.algorithm),
                "case {case:?}: expected the read of the upload to report ChecksumAlgorithm \
                 {:?}, got {:?}",
                row.algorithm,
                listed.element("ChecksumAlgorithm")
            );
        }
    }
}

#[tokio::test]
async fn an_initiation_naming_a_checksum_type_outside_the_published_values_is_refused() {
    let router = router();
    let name = "multipart-checksum-type-bogus-bucket";
    bucket(&router, name).await;

    // Values outside the two the enum publishes: a lowercase spelling of one of them, a
    // near-miss, and a value of another member entirely. None of them is a type this
    // service settles, so none may be accepted as "the request named nothing".
    for bogus in ["composite", "FULL-OBJECT", "WHOLE_OBJECT", "SHA256"] {
        let case = format!("x-amz-checksum-type: {bogus}");
        let key = format!("bogus/{}.bin", bogus.to_ascii_lowercase());
        let initiated = initiate(&router, name, &key, "CRC32", Some(bogus)).await;
        assert_eq!(
            initiated.status, 400,
            "case {case:?}: expected the initiation to be refused 400, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        let listed = list_uploads(&router, name).await;
        assert!(
            !text(&listed).contains(&key),
            "case {case:?}: expected no upload recorded for {key:?}, got {:?}",
            text(&listed)
        );
    }
}

#[tokio::test]
async fn an_initiation_naming_no_algorithm_settles_no_checksum_type() {
    let router = router();
    let name = "multipart-checksum-type-unasked-bucket";
    bucket(&router, name).await;
    let key = "settled/no-algorithm.bin";

    // No algorithm means no checksum is computed for the object at all, so there is no
    // computation for a type to describe — and a type named beside no algorithm settles
    // nothing either.
    for requested in [None, Some(COMPOSITE), Some(FULL_OBJECT)] {
        let case = format!("no algorithm, asked for {requested:?}");
        let mut headers: Vec<(&str, &str)> = Vec::new();
        if let Some(requested) = requested {
            headers.push(("x-amz-checksum-type", requested));
        }
        let initiated = send(
            &router,
            "POST",
            &format!("/{name}/{key}?uploads"),
            &headers,
            "",
        )
        .await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: expected the initiation to answer HTTP 200, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        assert_eq!(
            initiated.header("x-amz-checksum-type"),
            None,
            "case {case:?}: expected no checksum type reported, got {:?}",
            initiated.header("x-amz-checksum-type")
        );
    }
}

#[tokio::test]
async fn the_settled_type_is_the_one_the_completed_object_carries() {
    // (case, algorithm, requested type, the type the object must carry).
    //
    // The rows differ along the algorithm and along how the type was settled: taken from the
    // algorithm, asked for explicitly, and taken from the one algorithm that is always a
    // full-object checksum.
    let rows: [(&str, &str, Option<&str>, &str); 4] = [
        ("a composition by default", "SHA256", None, COMPOSITE),
        ("a composition asked for", "CRC32", Some(COMPOSITE), COMPOSITE),
        (
            "a full-object checksum asked for",
            "CRC32C",
            Some(FULL_OBJECT),
            FULL_OBJECT,
        ),
        (
            "the algorithm that is always a full-object checksum",
            "CRC64NVME",
            None,
            FULL_OBJECT,
        ),
    ];

    for (case, algorithm, requested, expected_type) in rows {
        let router = router();
        let name = "multipart-checksum-type-completion-bucket";
        bucket(&router, name).await;
        let key = format!("completed/{}.bin", algorithm.to_ascii_lowercase());

        let initiated = initiate(&router, name, &key, algorithm, requested).await;
        assert_eq!(
            initiated.status, 200,
            "case {case:?}: expected the initiation to answer HTTP 200, got {} with body {:?}",
            initiated.status,
            text(&initiated)
        );
        let upload_id = initiated
            .element("UploadId")
            .unwrap_or_else(|| panic!("case {case:?}: an initiation must report an UploadId"));

        // One part, which the published minimum part size does not apply to ("There is no
        // minimum size limit on the last part of your multipart upload", Amazon S3 User
        // Guide, *Amazon S3 multipart upload limits*).
        let body = b"the whole of this object arrived as one part".to_vec();
        let etag = upload_part(&router, name, &key, &upload_id, 1, &body).await;
        let payload = format!(
            "<CompleteMultipartUpload><Part><PartNumber>1</PartNumber><ETag>{etag}</ETag>\
             </Part></CompleteMultipartUpload>"
        );
        let completed = send(
            &router,
            "POST",
            &format!("/{name}/{key}?uploadId={upload_id}"),
            &[],
            &payload,
        )
        .await;
        assert_eq!(
            completed.status, 200,
            "case {case:?}: expected the completion to answer HTTP 200, got {} with body {:?}",
            completed.status,
            text(&completed)
        );
        assert_eq!(
            completed.element("ChecksumType").as_deref(),
            Some(expected_type),
            "case {case:?}: expected the completion to report ChecksumType {expected_type:?}, \
             got {:?}",
            completed.element("ChecksumType")
        );

        // The VALUE follows the type: a composition carries the number of parts after a
        // dash, a full-object checksum carries no suffix at all.
        let element = format!("Checksum{algorithm}");
        let value = completed.element(&element).unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the completion to report {element}, got body {:?}",
                text(&completed)
            )
        });
        let suffix = value.split_once('-').map(|(_, parts)| parts.to_string());
        let expected_suffix = (expected_type == COMPOSITE).then(|| String::from("1"));
        assert_eq!(
            suffix, expected_suffix,
            "case {case:?}: expected the part-count suffix {expected_suffix:?} on {value:?}, \
             got {suffix:?}"
        );

        // Every later read of the object reports the same pair — to a read that asked for it.
        let read = send(
            &router,
            "HEAD",
            &format!("/{name}/{key}"),
            &[("x-amz-checksum-mode", "ENABLED")],
            "",
        )
        .await;
        assert_eq!(
            read.status, 200,
            "case {case:?}: expected the read-back to answer HTTP 200, got {}",
            read.status
        );
        assert_eq!(
            read.header("x-amz-checksum-type").as_deref(),
            Some(expected_type),
            "case {case:?}: expected the read-back to report {expected_type:?}, got {:?}",
            read.header("x-amz-checksum-type")
        );
        assert_eq!(
            read.header(&format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase()))
                .as_deref(),
            Some(value.as_str()),
            "case {case:?}: expected the read-back to report the checksum {value:?}, got {:?}",
            read.header(&format!("x-amz-checksum-{}", algorithm.to_ascii_lowercase()))
        );
    }
}
