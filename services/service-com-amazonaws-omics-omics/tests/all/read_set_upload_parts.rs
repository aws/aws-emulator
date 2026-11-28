// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the read set upload PART slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`, restJson1
//! serialization, the modelled-constraint walk and the handlers — so what is asserted is
//! what a signed AWS SDK request observes. The HTTP harness is the one
//! `private_workflow_runs` owns and the upload helpers are `multipart_read_set_uploads`';
//! only the part helpers live here.
//!
//! The rules under test:
//!
//! * a part uploaded into a multipart read set upload is stored once inside that upload,
//!   identified by the pair that names it there — its source file and its part number —
//!   and the response reports the digest of the bytes the request carried, so the same
//!   content always answers with the same checksum and different content with a different
//!   one;
//!   * uploading a source and part number the upload already holds REPLACES that part:
//!     its size and checksum become the new content's, the instant it was first uploaded
//!     stands, and the upload holds no second part for that pair;
//!   * the checksum is the SHA-256 digest of the part's bytes rendered as standard,
//!     `=`-padded base64 — the encoding an AWS content checksum travels in — and never the
//!     hexadecimal rendering of the same digest, so one value travels every path that
//!     carries it: the upload response, the listing entry, and the checksum a completion
//!     has to list for that part;
//! * `ListReadSetUploadParts` reports exactly the parts stored for the one upload and
//!   source the request names, in ascending part-number order, each member read from the
//!   stored part, paging over them and narrowing them by creation instant;
//! * `ListReadSets` reports exactly the read sets the one sequence store the request names
//!   still holds, each member read from that read set's own stored record, in ascending
//!   `readSetId` order, paging over them and narrowing them by every member of its filter;
//! * `BatchDeleteReadSet` deletes, one id at a time, exactly the read sets the one
//!   sequence store the request names holds, and reports one error per id it could not
//!   delete — within the batch length the model publishes, outside which the whole call is
//!   refused and nothing is deleted;
//!   * a read set it reports no error for is GONE: no per-id read resolves that id, the
//!     listing no longer carries it, and the state holds no record for it;
//! * a per-id read of a read set id the sequence store the request names does not hold is
//!   the read-set-not-found refusal, whatever the reason the store does not hold it;
//! * a request naming a sequence store or an upload this emulator does not hold — or
//!   carrying no content at all — is refused rather than stored;
//! * a read of a read set's data serves ONE part of ONE of its files, byte for byte as
//!   that part was ingested, and a part or a file the read set does not hold is the
//!   not-found refusal.

use serde_json::{json, Value};
use service_com_amazonaws_omics_omics::state::{
    service_state, CREATION_TYPE_UPLOAD, FIRST_SEQUENCE_STORE_ID,
    MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE, MULTIPART_READ_SET_UPLOAD_PAGE_TOKEN_TAG,
    READ_SET_FILE_INDEX, READ_SET_ID_OFFSET, READ_SET_NOT_FOUND_MESSAGE, READ_SET_PAGE_TOKEN_TAG,
    READ_SET_PART_NOT_FOUND_MESSAGE, READ_SET_STATUS_ACTIVE, READ_SET_STATUS_DELETED,
    READ_SET_STATUS_PROCESSING_UPLOAD, READ_SET_UPLOAD_PART_SIZE, RESOURCE_HAS_DEPENDENTS_MESSAGE,
    SEQUENCE_STORE_NOT_FOUND_MESSAGE,
};
use smithy_server_runtime::MultiServiceRouter;

use crate::multipart_read_set_uploads::{
    create_store, create_upload, created_upload_id, emulator_holding_its_engine, stored_upload,
    REFERENCE_ARN,
};
use crate::private_workflow_runs::{
    assert_absent, assert_arn, assert_members, assert_timestamp, call, error_code, POLL_INTERVAL,
    REGION, SETTLE_DEADLINE,
};

/// Every value the modelled `ReadSetPartSource` enum admits.
/// Source: `smithy_query(query="shape",shape="ReadSetPartSource")` -> SOURCE1 | SOURCE2.
const PART_SOURCES: &[&str] = &["SOURCE1", "SOURCE2"];

/// The published bounds of the modelled `partNumber` range (1..10000).
/// Source: the `partNumber` member of `UploadReadSetPartRequest`.
const PART_NUMBER_RANGE: (i32, i32) = (1, 10_000);

/// The access key id every call below presents, matching the harness's own default.
const AKID: &str = "AKIDOMICSTEST";

/// `PUT /sequencestore/{store}/upload/{upload}/part?partSource=..&partNumber=..` with
/// `payload` as the RAW body.
///
/// That is the one wire form this operation binds: `payload` is an `@httpPayload`
/// streaming blob, and `partSource`/`partNumber` are `@httpQuery`, so the body carries
/// bytes rather than JSON and the harness's JSON `call` cannot express it.
pub(crate) async fn upload_part(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    upload_id: &str,
    part_source: &str,
    part_number: i32,
    payload: &[u8],
) -> (u16, Value) {
    let uri = format!(
        "/sequencestore/{sequence_store_id}/upload/{upload_id}/part\
         ?partSource={part_source}&partNumber={part_number}"
    );
    let request = http::Request::builder()
        .method("PUT")
        .uri(uri)
        .header("host", format!("omics.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{REGION}/omics/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        // The modelled blob declares `@requiresLength`, so the part is sent as a body of
        // a known length and not as a chunked stream.
        .header("content-type", "application/octet-stream")
        .header("content-length", payload.len().to_string())
        .body(bytes::Bytes::copy_from_slice(payload))
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body();
    let mut parsed: Value = if raw.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&raw)
            .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&raw).to_string()))
    };
    // restJson1 carries the error shape in `__type`, and the framework also sets the
    // `x-amzn-errortype` header; fold the header in so `error_code` reads either.
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, parsed)
}

/// `POST /sequencestore/{store}/upload/{upload}/parts{query}` with `body`.
async fn list_parts(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    upload_id: &str,
    query: &str,
    body: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/sequencestore/{sequence_store_id}/upload/{upload_id}/parts{query}"),
        Some(body),
    )
    .await
}

/// The `parts` array of a 200 listing, after asserting the status.
fn parts_of(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from ListReadSetUploadParts, got {body:?}"
    );
    body.get("parts")
        .and_then(Value::as_array)
        .unwrap_or_else(|| {
            panic!("case {case:?}: expected a parts array, got {body:?}")
        })
        .clone()
}

/// The `checksum` of a 200 upload response, after asserting the status and that the value
/// is the base64 SHA-256 form the service reports.
fn uploaded_checksum(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(
        status, 200,
        "case {case:?}: expected 200 from UploadReadSetPart, got {body:?}"
    );
    let checksum = body
        .get("checksum")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no checksum, got {body:?}"))
        .to_string();
    assert_base64_sha256(case, &checksum);
    checksum
}

/// The width of a SHA-256 digest in bytes, and the number of characters standard base64
/// renders it in: 44, the last of them padding.
/// Source: SHA-256 (FIPS 180-4) is a 256-bit digest, and standard base64 (RFC 4648 section
/// 4) codes every 3 bytes as 4 characters, padding the final group.
const SHA256_DIGEST_BYTES: usize = 32;
const BASE64_SHA256_WIDTH: usize = 44;

/// The checksum this service reports for a part carrying `bytes`: the base64 of their
/// SHA-256 digest.
fn expected_part_checksum(bytes: &[u8]) -> String {
    use sha2::Digest;
    ::aws_smithy_types::base64::encode(sha2::Sha256::digest(bytes).as_slice())
}

/// The lowercase hexadecimal rendering of the same digest — used only to prove it is NOT
/// what the service answers with, and is not accepted in its place.
fn hex_part_digest(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}

/// Assert `value` is a SHA-256 digest in standard, padded base64: 44 characters of the
/// base64 alphabet that decode to 32 bytes.
fn assert_base64_sha256(case: &str, value: &str) {
    assert_eq!(
        value.len(),
        BASE64_SHA256_WIDTH,
        "case {case:?}: expected a {BASE64_SHA256_WIDTH}-character base64 checksum, got \
         {value:?} of {} characters",
        value.len()
    );
    assert!(
        value.bytes().all(|byte| byte.is_ascii_alphanumeric()
            || byte == b'+'
            || byte == b'/'
            || byte == b'='),
        "case {case:?}: expected only standard base64 characters, got {value:?}"
    );
    assert!(
        value.ends_with('='),
        "case {case:?}: expected the padding a 32-byte digest's base64 ends in, got {value:?}"
    );
    let decoded = ::aws_smithy_types::base64::decode(value)
        .unwrap_or_else(|error| panic!("case {case:?}: {value:?} is not base64: {error}"));
    assert_eq!(
        decoded.len(),
        SHA256_DIGEST_BYTES,
        "case {case:?}: expected {SHA256_DIGEST_BYTES} decoded digest bytes, got {} from \
         {value:?}",
        decoded.len()
    );
}

/// The request body every upload below is initiated with.
fn upload_request(name: &str) -> Value {
    json!({
        "name": name,
        "sourceFileType": "FASTQ",
        "subjectId": "subject-parts",
        "sampleId": "sample-parts",
    })
}

/// An initiated upload inside a fresh sequence store, with the engine behind them.
async fn store_and_upload(
    name: &str,
) -> (
    MultiServiceRouter,
    std::sync::Arc<engine::InMemoryEngine>,
    String,
    String,
) {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, name).await;
    let (status, body) = create_upload(&router, &store, &upload_request(name)).await;
    let upload = created_upload_id(name, status, &body);
    (router, engine, store, upload)
}

/// A row of the part table: where the part belongs and what it carries.
struct PartCase {
    /// How the row is reported in a failure.
    case: &'static str,
    part_source: &'static str,
    part_number: i32,
    payload: Vec<u8>,
}

/// The table: one row per way a part can differ, so the rule is asserted over the class
/// and not over one request. The rows vary along the source file, the part number
/// (including both published bounds of its range), the size of the content (one byte to
/// kilobytes) and its bytes (text, a byte ramp, and bytes no text encoding admits).
fn part_cases() -> Vec<PartCase> {
    let (first_number, last_number) = PART_NUMBER_RANGE;
    vec![
        PartCase {
            case: "the first part of the first source",
            part_source: "SOURCE1",
            part_number: first_number,
            payload: b"@read1\nACGTACGTACGT\n+\n!!!!!!!!!!!!\n".to_vec(),
        },
        PartCase {
            case: "a later part of the first source",
            part_source: "SOURCE1",
            part_number: 2,
            payload: (0..4096u32).map(|n| (n % 251) as u8).collect(),
        },
        PartCase {
            case: "the highest part number the model admits",
            part_source: "SOURCE1",
            part_number: last_number,
            payload: vec![0x1f, 0x8b, 0x08, 0x00, 0xff, 0xfe],
        },
        PartCase {
            case: "a single byte from the paired source",
            part_source: "SOURCE2",
            part_number: first_number,
            payload: vec![b'N'],
        },
        PartCase {
            case: "the same content as the first row, under the paired source",
            part_source: "SOURCE2",
            part_number: 2,
            payload: b"@read1\nACGTACGTACGT\n+\n!!!!!!!!!!!!\n".to_vec(),
        },
        PartCase {
            case: "a part in the middle of the paired source",
            part_source: "SOURCE2",
            part_number: 573,
            payload: (0..1000u32).map(|n| (n % 7) as u8 + 1).collect(),
        },
    ]
}

#[tokio::test]
async fn an_uploaded_part_is_stored_once_and_reported_from_that_record() {
    let (router, engine, store, upload) = store_and_upload("the part destination").await;
    let cases = part_cases();

    // Upload them all first, so each assertion below also proves the upload keeps the
    // rows apart rather than answering with the most recent one.
    let mut checksums: Vec<String> = Vec::new();
    for part in &cases {
        let (status, body) = upload_part(
            &router,
            &store,
            &upload,
            part.part_source,
            part.part_number,
            &part.payload,
        )
        .await;
        let checksum = uploaded_checksum(part.case, status, &body);
        // The checksum is read off the content: a client that digests the bytes it sent
        // gets exactly this value back.
        assert_eq!(
            checksum,
            expected_part_checksum(&part.payload),
            "case {:?}: expected the digest of the uploaded bytes, got {checksum:?}",
            part.case
        );
        checksums.push(checksum);
    }

    // Same content, same checksum; different content, a different one.
    for (left, left_case) in cases.iter().enumerate().map(|(n, c)| (n, c.case)) {
        for right in (left + 1)..cases.len() {
            let same_content = cases[left].payload == cases[right].payload;
            assert_eq!(
                checksums[left] == checksums[right],
                same_content,
                "case {left_case:?} against {:?}: same content must mean the same \
                 checksum and different content a different one, got {:?} and {:?}",
                cases[right].case,
                checksums[left],
                checksums[right]
            );
        }
    }

    // Read back through the operation that reports an upload's parts: one source at a
    // time, every stored part of it, in ascending part-number order.
    for source in PART_SOURCES {
        let mut expected: Vec<&PartCase> = cases
            .iter()
            .filter(|part| part.part_source == *source)
            .collect();
        expected.sort_by_key(|part| part.part_number);
        let (status, body) = list_parts(
            &router,
            &store,
            &upload,
            "",
            &json!({ "partSource": source }),
        )
        .await;
        let listed = parts_of(source, status, &body);
        assert_eq!(
            listed.len(),
            expected.len(),
            "case {source:?}: expected {} listed parts, got {body:?}",
            expected.len()
        );
        for (part, item) in expected.iter().zip(&listed) {
            let case = part.case;
            assert_eq!(
                item.get("partNumber").and_then(Value::as_i64),
                Some(i64::from(part.part_number)),
                "case {case:?}: expected partNumber {}, got {item:?}",
                part.part_number
            );
            assert_eq!(
                item.get("partSize").and_then(Value::as_i64),
                Some(part.payload.len() as i64),
                "case {case:?}: expected partSize {}, got {item:?}",
                part.payload.len()
            );
            assert_eq!(
                item.get("partSource").and_then(Value::as_str),
                Some(*source),
                "case {case:?}: expected partSource {source:?}, got {item:?}"
            );
            assert_eq!(
                item.get("checksum").and_then(Value::as_str),
                Some(expected_part_checksum(&part.payload).as_str()),
                "case {case:?}: expected the content digest as the listed checksum, \
                 got {item:?}"
            );
            let created = assert_timestamp(case, "creationTime", item);
            let updated = assert_timestamp(case, "lastUpdatedTime", item);
            assert!(
                updated >= created,
                "case {case:?}: expected lastUpdatedTime at or after creationTime, \
                 got {updated} and {created}"
            );
        }
    }

    // And the bytes really are in the service's state: the part a later complete reads is
    // the content the caller sent, not its length alone.
    let stored = stored_upload(&engine, &store, &upload);
    assert_eq!(
        stored.parts.len(),
        cases.len(),
        "expected {} stored parts, got {:?}",
        cases.len(),
        stored.parts.keys().collect::<Vec<_>>()
    );
    for part in &cases {
        let held = stored
            .parts
            .get(&(part.part_source.to_string(), part.part_number))
            .unwrap_or_else(|| {
                panic!(
                    "case {:?}: no part is stored for {} {}",
                    part.case, part.part_source, part.part_number
                )
            });
        assert_eq!(
            held.payload, part.payload,
            "case {:?}: the stored part must carry the bytes the request sent",
            part.case
        );
        assert_eq!(
            held.checksum,
            expected_part_checksum(&part.payload),
            "case {:?}: the stored checksum must be the digest of the stored bytes",
            part.case
        );
    }
}

#[tokio::test]
async fn a_part_checksum_is_the_base64_sha256_of_its_bytes_on_every_path() {
    let (router, _engine, store, upload) = store_and_upload("the checksum destination").await;
    // The same table of contents: the encoding rule holds for every size and byte pattern
    // a part can carry, not for one payload.
    let cases = part_cases();
    let mut answered: Vec<(&'static str, i32, String)> = Vec::new();

    for part in &cases {
        let (status, body) = upload_part(
            &router,
            &store,
            &upload,
            part.part_source,
            part.part_number,
            &part.payload,
        )
        .await;
        // `uploaded_checksum` asserts the base64 shape: 44 characters that decode to the
        // 32 bytes of a SHA-256 digest.
        let checksum = uploaded_checksum(part.case, status, &body);
        assert_eq!(
            checksum,
            expected_part_checksum(&part.payload),
            "case {:?}: expected the base64 SHA-256 of the uploaded bytes, got {checksum:?}",
            part.case
        );
        // The same digest rendered the other way is a different string, and the response
        // carries the base64 one.
        let hex = hex_part_digest(&part.payload);
        assert_ne!(
            checksum, hex,
            "case {:?}: expected the base64 rendering of the digest, got the hexadecimal one",
            part.case
        );
        assert_eq!(
            ::aws_smithy_types::base64::decode(&checksum)
                .unwrap_or_else(|error| panic!("case {:?}: {checksum:?} is not base64: {error}", part.case)),
            hex::decode(&hex)
                .unwrap_or_else(|error| panic!("case {:?}: {hex:?} is not hex: {error}", part.case)),
            "case {:?}: both renderings must carry the same digest bytes",
            part.case
        );
        answered.push((part.part_source, part.part_number, checksum));
    }

    // The listing reports, for each part, the very value the upload answered with.
    for source in PART_SOURCES {
        let (status, body) = list_parts(
            &router,
            &store,
            &upload,
            "",
            &json!({ "partSource": source }),
        )
        .await;
        for item in &parts_of(source, status, &body) {
            let number = item
                .get("partNumber")
                .and_then(Value::as_i64)
                .unwrap_or_else(|| panic!("case {source:?}: a listed part carried no partNumber"));
            let expected = answered
                .iter()
                .find(|(part_source, part_number, _)| {
                    part_source == source && i64::from(*part_number) == number
                })
                .map(|(_, _, checksum)| checksum.clone())
                .unwrap_or_else(|| {
                    panic!("case {source:?}: part {number} was never uploaded, got {item:?}")
                });
            let reported = item
                .get("checksum")
                .and_then(Value::as_str)
                .unwrap_or_else(|| panic!("case {source:?}: part {number} listed no checksum"));
            assert_base64_sha256(&format!("{source} part {number}"), reported);
            assert_eq!(
                reported, expected,
                "case {source:?}: part {number} must be listed with the checksum the upload \
                 answered, got {reported:?}"
            );
        }
    }

    // A completion listing the hexadecimal rendering of each digest is naming a checksum
    // no part of this upload carries, so it is refused and forms nothing...
    let hex_listed: Vec<(&str, i32, String)> = cases
        .iter()
        .map(|part| {
            (
                part.part_source,
                part.part_number,
                hex_part_digest(&part.payload),
            )
        })
        .collect();
    let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&hex_listed)).await;
    assert_eq!(
        status, 400,
        "case \"hexadecimal checksums\": expected 400 from the completion, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ValidationException",
        "case \"hexadecimal checksums\": expected ValidationException, got {body:?}"
    );

    // ...while the values the service answered with are exactly what it accepts for them.
    let base64_listed: Vec<(&str, i32, String)> = answered
        .iter()
        .map(|(part_source, part_number, checksum)| {
            (*part_source, *part_number, checksum.clone())
        })
        .collect();
    let (status, body) =
        complete_upload(&router, &store, &upload, &parts_list(&base64_listed)).await;
    let read_set_id = completed_read_set_id("the answered checksums", status, &body);
    assert!(
        !read_set_id.is_empty(),
        "case \"the answered checksums\": expected a readSetId, got {body:?}"
    );
}

#[tokio::test]
async fn re_uploading_a_part_number_replaces_that_part() {
    let (router, engine, store, upload) = store_and_upload("the replacement destination").await;

    // Each row replaces one part number with content that differs from the first upload
    // in length, in bytes, or in neither.
    let rows: Vec<(&str, i32, Vec<u8>, Vec<u8>)> = vec![
        (
            "replaced with longer content",
            1,
            b"the first attempt".to_vec(),
            b"the second attempt, which is longer".to_vec(),
        ),
        (
            "replaced with shorter content",
            2,
            vec![7u8; 512],
            vec![9u8],
        ),
        (
            "replaced with the same content",
            3,
            b"identical bytes".to_vec(),
            b"identical bytes".to_vec(),
        ),
    ];

    for (case, part_number, first, second) in &rows {
        let (status, body) =
            upload_part(&router, &store, &upload, "SOURCE1", *part_number, first).await;
        let first_checksum = uploaded_checksum(case, status, &body);
        assert_eq!(
            first_checksum,
            expected_part_checksum(first),
            "case {case:?}: expected the digest of the first content, got {body:?}"
        );
        let listed = listed_part(&router, &store, &upload, case, *part_number).await;
        let created = assert_timestamp(case, "creationTime", &listed);

        let (status, body) =
            upload_part(&router, &store, &upload, "SOURCE1", *part_number, second).await;
        let second_checksum = uploaded_checksum(case, status, &body);
        assert_eq!(
            second_checksum,
            expected_part_checksum(second),
            "case {case:?}: expected the digest of the replacing content, got {body:?}"
        );

        let listed = listed_part(&router, &store, &upload, case, *part_number).await;
        assert_eq!(
            listed.get("partSize").and_then(Value::as_i64),
            Some(second.len() as i64),
            "case {case:?}: expected the replacing content's size, got {listed:?}"
        );
        assert_eq!(
            listed.get("checksum").and_then(Value::as_str),
            Some(second_checksum.as_str()),
            "case {case:?}: expected the replacing content's checksum, got {listed:?}"
        );
        // A replacement is not a second create: the part number keeps the instant it was
        // first uploaded, and only its last-updated instant can have moved.
        let created_again = assert_timestamp(case, "creationTime", &listed);
        let updated = assert_timestamp(case, "lastUpdatedTime", &listed);
        assert_eq!(
            created_again, created,
            "case {case:?}: expected the creationTime of the first upload to stand, \
             got {created_again} after {created}"
        );
        assert!(
            updated >= created_again,
            "case {case:?}: expected lastUpdatedTime at or after creationTime, got \
             {updated} and {created_again}"
        );
    }

    // One part per number, however often a number was uploaded.
    let stored = stored_upload(&engine, &store, &upload);
    assert_eq!(
        stored.parts.len(),
        rows.len(),
        "expected one stored part per uploaded part number, got {:?}",
        stored.parts.keys().collect::<Vec<_>>()
    );
    for (case, part_number, _, second) in &rows {
        let held = stored
            .parts
            .get(&("SOURCE1".to_string(), *part_number))
            .unwrap_or_else(|| panic!("case {case:?}: no part is stored for {part_number}"));
        assert_eq!(
            &held.payload, second,
            "case {case:?}: the stored part must carry the replacing bytes"
        );
    }
}

/// The one listed item for `part_number` of `SOURCE1`, after asserting there is exactly
/// one.
async fn listed_part(
    router: &MultiServiceRouter,
    store: &str,
    upload: &str,
    case: &str,
    part_number: i32,
) -> Value {
    let (status, body) = list_parts(
        router,
        store,
        upload,
        "",
        &json!({ "partSource": "SOURCE1" }),
    )
    .await;
    let listed = parts_of(case, status, &body);
    let matching: Vec<&Value> = listed
        .iter()
        .filter(|item| item.get("partNumber").and_then(Value::as_i64) == Some(i64::from(part_number)))
        .collect();
    assert_eq!(
        matching.len(),
        1,
        "case {case:?}: expected exactly one listed part numbered {part_number}, got {listed:?}"
    );
    matching[0].clone()
}

/// A row of the part-refusal table: the request, and the refusal it must answer with.
struct RefusalCase {
    /// How the row is reported in a failure.
    case: &'static str,
    sequence_store_id: String,
    upload_id: String,
    part_source: &'static str,
    part_number: i32,
    payload: Vec<u8>,
    status: u16,
    code: &'static str,
    /// The message the refusal must carry verbatim, for the refusals this service words
    /// itself; a modelled-constraint refusal is worded by the centralized walk, so those
    /// rows name none.
    message: Option<&'static str>,
}

/// A row of the listing-refusal table: the ids and query the listing is refused for.
struct ListingRefusalCase {
    case: &'static str,
    sequence_store_id: String,
    upload_id: String,
    query: &'static str,
    status: u16,
    code: &'static str,
    message: Option<&'static str>,
}

#[tokio::test]
async fn a_part_this_emulator_cannot_place_is_refused() {
    let (router, engine, store, upload) = store_and_upload("the refusal destination").await;
    // A second store, so an upload id can be presented under a store it does not belong
    // to.
    let other_store = create_store(&router, "the other refusal destination").await;

    // An id no store or upload holds, spelled inside the modelled 10..36 digits.
    let absent = "9999999999";

    let rows: Vec<RefusalCase> = vec![
        RefusalCase {
            case: "a sequence store nothing holds",
            sequence_store_id: absent.to_string(),
            upload_id: upload.clone(),
            part_source: "SOURCE1",
            part_number: 1,
            payload: b"orphan".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(SEQUENCE_STORE_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "an upload nothing holds",
            sequence_store_id: store.clone(),
            upload_id: absent.to_string(),
            part_source: "SOURCE1",
            part_number: 1,
            payload: b"orphan".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "an upload presented under another store",
            sequence_store_id: other_store.clone(),
            upload_id: upload.clone(),
            part_source: "SOURCE1",
            part_number: 1,
            payload: b"orphan".to_vec(),
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a part carrying no content",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            part_source: "SOURCE1",
            part_number: 1,
            payload: Vec::new(),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a part number below the published range",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            part_source: "SOURCE1",
            part_number: PART_NUMBER_RANGE.0 - 1,
            payload: b"out of range".to_vec(),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a part number above the published range",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            part_source: "SOURCE1",
            part_number: PART_NUMBER_RANGE.1 + 1,
            payload: b"out of range".to_vec(),
            status: 400,
            code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a source the enum does not admit",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            part_source: "SOURCE3",
            part_number: 1,
            payload: b"unknown source".to_vec(),
            status: 400,
            code: "ValidationException",
            message: None,
        },
    ];

    for row in &rows {
        let case = row.case;
        let (got, body) = upload_part(
            &router,
            &row.sequence_store_id,
            &row.upload_id,
            row.part_source,
            row.part_number,
            &row.payload,
        )
        .await;
        assert_eq!(
            got, row.status,
            "case {case:?}: expected HTTP {}, got {got} with {body:?}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body:?}",
            row.code
        );
        if let Some(message) = row.message {
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(message),
                "case {case:?}: expected the service's own wording, got {body:?}"
            );
        }
    }

    // Nothing was stored by any refusal, under either store.
    let stored = stored_upload(&engine, &store, &upload);
    assert!(
        stored.parts.is_empty(),
        "expected a refused upload to store nothing, got {:?}",
        stored.parts.keys().collect::<Vec<_>>()
    );

    // The same two ids refuse a LISTING as well, and a token no listing of parts issued
    // is refused rather than read as a resume point.
    let listing_rows: Vec<ListingRefusalCase> = vec![
        ListingRefusalCase {
            case: "listing a sequence store nothing holds",
            sequence_store_id: absent.to_string(),
            upload_id: upload.clone(),
            query: "",
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(SEQUENCE_STORE_NOT_FOUND_MESSAGE),
        },
        ListingRefusalCase {
            case: "listing an upload nothing holds",
            sequence_store_id: store.clone(),
            upload_id: absent.to_string(),
            query: "",
            status: 404,
            code: "ResourceNotFoundException",
            message: Some(MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE),
        },
        ListingRefusalCase {
            case: "a token from another listing",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            query: "?nextToken=rg_31303030303030",
            status: 400,
            code: "ValidationException",
            message: None,
        },
        ListingRefusalCase {
            case: "a token this service never issued",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            query: "?nextToken=not-a-token",
            status: 400,
            code: "ValidationException",
            message: None,
        },
    ];

    for row in &listing_rows {
        let case = row.case;
        let (got, body) = list_parts(
            &router,
            &row.sequence_store_id,
            &row.upload_id,
            row.query,
            &json!({ "partSource": "SOURCE1" }),
        )
        .await;
        assert_eq!(
            got, row.status,
            "case {case:?}: expected HTTP {}, got {got} with {body:?}",
            row.status
        );
        assert_eq!(
            error_code(&body),
            row.code,
            "case {case:?}: expected {}, got {body:?}",
            row.code
        );
        if let Some(message) = row.message {
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(message),
                "case {case:?}: expected the service's own wording, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn listing_an_uploads_parts_pages_and_narrows_over_the_stored_parts() {
    let (router, _engine, store, upload) = store_and_upload("the paging destination").await;

    // Five parts of one source, and two of the other, so a page is asserted to stop at
    // the source it was asked for as well as at its size.
    let numbered: Vec<i32> = vec![1, 2, 3, 4, 5];
    for number in &numbered {
        let (status, body) = upload_part(
            &router,
            &store,
            &upload,
            "SOURCE1",
            *number,
            format!("part {number} of source1").as_bytes(),
        )
        .await;
        uploaded_checksum(&format!("source1 part {number}"), status, &body);
    }
    for number in &[1, 2] {
        let (status, body) = upload_part(
            &router,
            &store,
            &upload,
            "SOURCE2",
            *number,
            format!("part {number} of source2").as_bytes(),
        )
        .await;
        uploaded_checksum(&format!("source2 part {number}"), status, &body);
    }

    // A page size at and below the number of stored parts, so the walk is asserted both
    // with and without a token.
    for page_size in &[1usize, 2, 5, 7] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<i64> = Vec::new();
        let mut token: Option<String> = None;
        // One iteration per part at worst, plus the page that ends the walk.
        for _ in 0..=numbered.len() {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = list_parts(
                &router,
                &store,
                &upload,
                &query,
                &json!({ "partSource": "SOURCE1" }),
            )
            .await;
            let page = parts_of(&case, status, &body);
            assert!(
                page.len() <= *page_size,
                "case {case:?}: a page must hold at most {page_size} parts, got {body:?}"
            );
            seen.extend(
                page.iter()
                    .filter_map(|item| item.get("partNumber").and_then(Value::as_i64)),
            );
            token = body
                .get("nextToken")
                .and_then(Value::as_str)
                .map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {case:?}: the walk must end without a nextToken, got {token:?}"
        );
        let expected: Vec<i64> = numbered.iter().map(|n| i64::from(*n)).collect();
        assert_eq!(
            seen, expected,
            "case {case:?}: expected every part of the source exactly once in ascending \
             order, got {seen:?}"
        );
    }

    // The listing narrows to the source the request names.
    let (status, body) = list_parts(
        &router,
        &store,
        &upload,
        "",
        &json!({ "partSource": "SOURCE2" }),
    )
    .await;
    let listed = parts_of("the paired source", status, &body);
    let numbers: Vec<i64> = listed
        .iter()
        .filter_map(|item| item.get("partNumber").and_then(Value::as_i64))
        .collect();
    assert_eq!(
        numbers,
        vec![1, 2],
        "case \"the paired source\": expected only that source's parts, got {body:?}"
    );

    // And it narrows by creation instant: each bound is inclusive, and a window the
    // parts fall outside of selects none of them.
    let windows: Vec<(&str, Value, usize)> = vec![
        (
            "created after an instant long past",
            json!({ "createdAfter": "2000-01-01T00:00:00Z" }),
            numbered.len(),
        ),
        (
            "created before an instant far ahead",
            json!({ "createdBefore": "2999-01-01T00:00:00Z" }),
            numbered.len(),
        ),
        (
            "inside a window spanning both",
            json!({
                "createdAfter": "2000-01-01T00:00:00Z",
                "createdBefore": "2999-01-01T00:00:00Z",
            }),
            numbered.len(),
        ),
        (
            "created after an instant far ahead",
            json!({ "createdAfter": "2999-01-01T00:00:00Z" }),
            0,
        ),
        (
            "created before an instant long past",
            json!({ "createdBefore": "2000-01-01T00:00:00Z" }),
            0,
        ),
    ];

    for (case, filter, expected) in &windows {
        let (status, body) = list_parts(
            &router,
            &store,
            &upload,
            "",
            &json!({ "partSource": "SOURCE1", "filter": filter }),
        )
        .await;
        let listed = parts_of(case, status, &body);
        assert_eq!(
            listed.len(),
            *expected,
            "case {case:?}: expected {expected} parts, got {body:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// CompleteMultipartReadSetUpload -> GetReadSetMetadata
// ---------------------------------------------------------------------------
//
// The rules the cases below assert:
//
// * completing a multipart read set upload forms ONE read set inside the sequence store
//   the request names, out of the parts the request lists: each file of the read set is
//   the content of that source's parts in ascending part-number order, however the
//   request happened to list them, and every descriptive member is the one the upload
//   carried;
// * the upload is spent by the completion: nothing can be added to, listed in or aborted
//   on it afterwards;
// * a completion naming a part the upload does not hold, a checksum that is not the
//   stored part's, the same part twice, or no part at all is refused and forms nothing;
// * a sequence store that holds a read set is not empty, so deleting it is refused until
//   its read sets are gone.

/// The published bounds of the modelled `ReadSetId` length (10..36).
/// Source: the `readSetId` member of `CompleteMultipartReadSetUploadResponse`.
const READ_SET_ID_WIDTH: (usize, usize) = (10, 36);

/// `POST /sequencestore/{store}/upload/{upload}/complete` with `parts` as the list.
pub(crate) async fn complete_upload(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    upload_id: &str,
    parts: &Value,
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/sequencestore/{sequence_store_id}/upload/{upload_id}/complete"),
        Some(&json!({ "parts": parts })),
    )
    .await
}

/// `GET /sequencestore/{store}/readset/{id}/metadata`.
async fn read_set_metadata(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    read_set_id: &str,
) -> (u16, Value) {
    call(
        router,
        "GET",
        &format!("/sequencestore/{sequence_store_id}/readset/{read_set_id}/metadata"),
        None,
    )
    .await
}

/// `DELETE /sequencestore/{id}`.
async fn delete_sequence_store(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
) -> (u16, Value) {
    call(
        router,
        "DELETE",
        &format!("/sequencestore/{sequence_store_id}"),
        None,
    )
    .await
}

/// The `readSetId` of a 200 completion response, after asserting the status and its shape.
fn completed_read_set_id(case: &str, status: u16, body: &Value) -> String {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
    let id = body
        .get("readSetId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no readSetId, got {body:?}"))
        .to_string();
    let (min, max) = READ_SET_ID_WIDTH;
    assert!(
        id.len() >= min && id.len() <= max && id.bytes().all(|b| b.is_ascii_digit()),
        "case {case:?}: expected a {min}..{max} digit decimal readSetId, got {id:?}"
    );
    let value: u64 = id
        .parse()
        .unwrap_or_else(|_| panic!("case {case:?}: readSetId {id:?} does not parse as a number"));
    let floor = FIRST_SEQUENCE_STORE_ID + READ_SET_ID_OFFSET;
    assert!(
        value >= floor,
        "case {case:?}: expected a readSetId at or above {floor}, got {value}"
    );
    id
}

/// The parts list a completion carries: one entry per uploaded part, with the checksum
/// `UploadReadSetPart` answered with.
pub(crate) fn parts_list(parts: &[(&str, i32, String)]) -> Value {
    Value::Array(
        parts
            .iter()
            .map(|(part_source, part_number, checksum)| {
                json!({
                    "partSource": part_source,
                    "partNumber": part_number,
                    "checksum": checksum,
                })
            })
            .collect(),
    )
}

/// Upload every part of `parts` and answer the completion list entry for each, in the
/// order the parts were uploaded.
pub(crate) async fn upload_all(
    router: &MultiServiceRouter,
    case: &str,
    sequence_store_id: &str,
    upload_id: &str,
    parts: &[(&'static str, i32, Vec<u8>)],
) -> Vec<(&'static str, i32, String)> {
    let mut listed = Vec::new();
    for (part_source, part_number, payload) in parts {
        let (status, body) = upload_part(
            router,
            sequence_store_id,
            upload_id,
            part_source,
            *part_number,
            payload,
        )
        .await;
        let checksum = uploaded_checksum(case, status, &body);
        assert_eq!(
            checksum,
            expected_part_checksum(payload),
            "case {case:?}: part {part_number} of {part_source} must report the digest of \
             its own content"
        );
        listed.push((*part_source, *part_number, checksum));
    }
    listed
}

/// Read the read set until it reports `expected`, and answer that response.
///
/// Polling rather than sleeping a fixed span: the status is owed to the clock, so what is
/// asserted is that it arrives, not when a single read happened to land.
async fn await_read_set_status(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    read_set_id: &str,
    expected: &str,
) -> Value {
    let deadline = std::time::Instant::now() + SETTLE_DEADLINE;
    loop {
        let (status, body) = read_set_metadata(router, sequence_store_id, read_set_id).await;
        assert_eq!(
            status, 200,
            "read set {read_set_id}: expected 200, got {body:?}"
        );
        if body.get("status").and_then(Value::as_str) == Some(expected) {
            return body;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "read set {read_set_id}: expected status {expected} within {SETTLE_DEADLINE:?}, \
             last read {body:?}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// A row of the completion table: what the upload carried, which parts were uploaded into
/// it, and the descriptive members the read set must report.
struct CompleteCase {
    /// How the row is reported in a failure, and the upload's `name`.
    case: &'static str,
    /// The `sourceFileType` the row uploads.
    source_file_type: &'static str,
    /// The members the upload is initiated with around the required ones.
    upload: Value,
    /// The parts uploaded into it: source file, part number, content.
    parts: Vec<(&'static str, i32, Vec<u8>)>,
    /// Every member `GetReadSetMetadata` must report with exactly this value.
    expected: Value,
    /// The modelled `ETagAlgorithm` the read set's data must be reported under: the
    /// format of that data joined to the store's ETag algorithm family.
    e_tag_algorithm: &'static str,
    /// The extension of the object each source file was ingested to.
    extension: &'static str,
    /// Members it must NOT carry: nothing is invented for what the upload left out, and
    /// nothing is reported for what this emulator does not compute.
    absent: Vec<&'static str>,
}

/// The table: one row per way a completed upload can differ, so the rule is asserted over
/// the class and not over one request. The rows vary along the source format, the number
/// of source files (one and a paired pair), the number of parts per source, the part
/// numbers (including the top of the modelled range), the size and bytes of the content,
/// and which optional members the upload carried.
fn complete_cases() -> Vec<CompleteCase> {
    let (_, last_number) = PART_NUMBER_RANGE;
    vec![
        CompleteCase {
            case: "one part from one source",
            source_file_type: "FASTQ",
            upload: json!({}),
            parts: vec![(
                "SOURCE1",
                1,
                b"@read1\nACGTACGTACGT\n+\n!!!!!!!!!!!!\n".to_vec(),
            )],
            expected: json!({
                "fileType": "FASTQ",
                // The records the ingested bytes hold, and the alignment the format
                // implies: one read of twelve bases, unaligned.
                "sequenceInformation": {
                    "alignment": "UNALIGNED",
                    "totalReadCount": 1,
                    "totalBaseCount": 12,
                },
            }),
            absent: vec!["description", "referenceArn", "statusMessage"],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq",
        },
        CompleteCase {
            case: "several parts from one source",
            source_file_type: "FASTQ",
            upload: json!({ "description": "reads for the autumn cohort" }),
            parts: vec![
                ("SOURCE1", 1, vec![b'A'; 64]),
                ("SOURCE1", 2, vec![b'C'; 64]),
                ("SOURCE1", 3, vec![b'G'; 17]),
            ],
            expected: json!({
                "fileType": "FASTQ",
                "description": "reads for the autumn cohort",
                // The bytes are not FASTQ records, so nothing counted them: the
                // alignment the format implies is all this read set reports.
                "sequenceInformation": { "alignment": "UNALIGNED" },
            }),
            absent: vec!["referenceArn"],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq",
        },
        CompleteCase {
            case: "a paired-end upload",
            source_file_type: "FASTQ",
            upload: json!({ "generatedFrom": "1000 Genomes" }),
            parts: vec![
                ("SOURCE1", 1, (0..4096u32).map(|n| (n % 251) as u8).collect()),
                ("SOURCE1", 2, vec![0x1f, 0x8b, 0x08, 0x00, 0xff, 0xfe]),
                ("SOURCE2", 1, vec![b'N']),
            ],
            expected: json!({
                "fileType": "FASTQ",
                "sequenceInformation": {
                    "alignment": "UNALIGNED",
                    "generatedFrom": "1000 Genomes",
                },
            }),
            absent: vec!["description", "referenceArn"],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq",
        },
        CompleteCase {
            case: "an aligned upload with a reference",
            source_file_type: "CRAM",
            upload: json!({
                "referenceArn": REFERENCE_ARN,
                "description": "aligned reads",
            }),
            parts: vec![
                ("SOURCE1", 7, vec![b'T'; 10]),
                ("SOURCE1", 9, vec![b'G'; 3]),
            ],
            expected: json!({
                "fileType": "CRAM",
                "referenceArn": REFERENCE_ARN,
                "description": "aligned reads",
                // CRAM is one of the aligned source formats.
                "sequenceInformation": { "alignment": "ALIGNED" },
            }),
            absent: vec![],
            e_tag_algorithm: "CRAM_MD5up",
            extension: "cram",
        },
        CompleteCase {
            case: "the highest part number the model admits",
            source_file_type: "UBAM",
            upload: json!({}),
            parts: vec![
                ("SOURCE1", 1, vec![0x42; 128]),
                ("SOURCE1", last_number, vec![0x43; 5]),
            ],
            expected: json!({
                "fileType": "UBAM",
                // uBAM is an UNALIGNED format.
                "sequenceInformation": { "alignment": "UNALIGNED" },
            }),
            absent: vec!["description", "referenceArn"],
            // A uBAM read set's data IS a BAM file, which is the format the modelled
            // `ETagAlgorithm` names for it.
            e_tag_algorithm: "BAM_MD5up",
            extension: "bam",
        },
    ]
}

/// The `FileInformation` members a source's parts must be reported as: how many parts
/// formed the file, the part size the upload itself was conducted with, the total content
/// length, and the digest of the bytes the ingest took in.
///
/// `s3Access` is asserted separately ([`assert_read_set_file_s3_uri`]): its bucket is the
/// store's access point alias, which a client reads off the service rather than
/// constructing.
fn expected_file(parts: &[(&'static str, i32, Vec<u8>)], part_source: &str) -> Option<Value> {
    let of_source = parts_of_source(parts, part_source);
    if of_source.is_empty() {
        return None;
    }
    let content = expected_content(parts, part_source);
    Some(json!({
        "totalParts": of_source.len(),
        "partSize": READ_SET_UPLOAD_PART_SIZE,
        "contentLength": content.len(),
        "md5": md5_hex(&content),
    }))
}

/// The parts of `parts` that came from `part_source`, in ascending part-number order.
fn parts_of_source<'a>(
    parts: &'a [(&'static str, i32, Vec<u8>)],
    part_source: &str,
) -> Vec<&'a (&'static str, i32, Vec<u8>)> {
    let mut of_source: Vec<&(&'static str, i32, Vec<u8>)> = parts
        .iter()
        .filter(|(source, _, _)| *source == part_source)
        .collect();
    of_source.sort_by_key(|(_, number, _)| *number);
    of_source
}

/// The MD5 digest of `content`, as a read set file's `md5` and an `MD5up` ETag report it:
/// 32 lowercase hexadecimal characters, computed here from the bytes the case uploaded.
pub(crate) fn md5_hex(content: &[u8]) -> String {
    use md5::Digest;
    hex::encode(md5::Md5::digest(content))
}

/// Every member reported for one file of an ingested read set.
/// Source: `smithy_query(query="shape",shape="FileInformation")` -> totalParts, partSize,
/// contentLength, s3Access; plus the `md5` the service reports for an ingested file.
const READ_SET_FILE_MEMBERS: &[&str] = &[
    "totalParts",
    "partSize",
    "contentLength",
    "s3Access",
    "md5",
];

/// Assert one reported file carries exactly the members above and nothing else, so a
/// member asserted by value below is not joined by one nothing derived.
fn assert_file_members(case: &str, member: &str, file: &Value) {
    let mut reported: Vec<&str> = file
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: expected a {member} object, got {file:?}"))
        .keys()
        .map(String::as_str)
        .collect();
    reported.sort_unstable();
    let mut expected = READ_SET_FILE_MEMBERS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        reported, expected,
        "case {case:?}: expected exactly the file members for {member}, got {file:?}"
    );
}

/// The `ReadSetFiles` member one `ReadSetPartSource` is reported under.
fn file_member(part_source: &str) -> &'static str {
    match part_source {
        "SOURCE1" => "source1",
        _ => "source2",
    }
}

/// Assert the `s3Uri` of one ingested read set file is the object that file lives at:
/// the store's access point alias as the bucket, and then the account, the store, the read
/// set, the read set's name, which source file it is and the format's extension.
///
/// The alias is environment-specific — S3 assigns the unique part of an access point
/// alias — so what is asserted about it is its SHAPE (`<store id>-<discriminator>-s3alias`
/// with a non-empty discriminator); every other segment is the one the request and the
/// state name.
pub(crate) fn assert_read_set_file_s3_uri(
    case: &str,
    s3_uri: &str,
    sequence_store_id: &str,
    read_set_id: &str,
    read_set_name: &str,
    part_source: &str,
    extension: &str,
) {
    let address = s3_uri.strip_prefix("s3://").unwrap_or_else(|| {
        panic!("case {case:?}: expected an s3:// URI, got {s3_uri:?}")
    });
    let (bucket, key) = address.split_once('/').unwrap_or_else(|| {
        panic!("case {case:?}: expected a bucket and a key in {s3_uri:?}")
    });
    let discriminator = bucket
        .strip_prefix(&format!("{sequence_store_id}-"))
        .and_then(|rest| rest.strip_suffix("-s3alias"))
        .unwrap_or_else(|| {
            panic!(
                "case {case:?}: expected the store's access point alias as the bucket, \
                 got {bucket:?} in {s3_uri:?}"
            )
        });
    assert!(
        !discriminator.is_empty() && discriminator.bytes().all(|b| b.is_ascii_alphanumeric()),
        "case {case:?}: expected an alphanumeric alias discriminator, got {discriminator:?}"
    );
    let source = file_member(part_source);
    let expected_key = format!(
        "{}/sequenceStore/{sequence_store_id}/readSet/{read_set_id}/\
         {read_set_name}_{source}.{extension}",
        engine::emulator_account_id()
    );
    assert_eq!(
        key, expected_key,
        "case {case:?}: expected the ingested file's key, got {s3_uri:?}"
    );
}

/// The content a source's file must hold: its parts concatenated in ascending part-number
/// order, whatever order they were uploaded or listed in.
fn expected_content(parts: &[(&'static str, i32, Vec<u8>)], part_source: &str) -> Vec<u8> {
    let mut of_source: Vec<&(&'static str, i32, Vec<u8>)> = parts
        .iter()
        .filter(|(source, _, _)| *source == part_source)
        .collect();
    of_source.sort_by_key(|(_, number, _)| *number);
    of_source
        .iter()
        .flat_map(|(_, _, payload)| payload.clone())
        .collect()
}

#[tokio::test]
async fn completing_an_upload_forms_a_read_set_from_the_parts_it_lists() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the completion destination").await;
    let mut read_set_ids: Vec<String> = Vec::new();

    for row in complete_cases() {
        let case = row.case;
        let mut upload_body = row.upload.as_object().cloned().unwrap_or_default();
        upload_body.insert("name".to_string(), Value::String(case.to_string()));
        upload_body.insert(
            "sourceFileType".to_string(),
            Value::String(row.source_file_type.to_string()),
        );
        upload_body.insert(
            "subjectId".to_string(),
            Value::String(format!("subject for {case}")),
        );
        upload_body.insert(
            "sampleId".to_string(),
            Value::String(format!("sample for {case}")),
        );
        let (status, body) = create_upload(&router, &store, &Value::Object(upload_body)).await;
        let upload = created_upload_id(case, status, &body);

        let mut listed = upload_all(&router, case, &store, &upload, &row.parts).await;
        // Listed in the REVERSE of the order they were uploaded: the read set's files are
        // assembled by part number, not by the order the request happened to name them.
        listed.reverse();
        let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&listed)).await;
        let read_set_id = completed_read_set_id(case, status, &body);
        assert!(
            !read_set_ids.contains(&read_set_id),
            "case {case:?}: readSetId {read_set_id} was already minted for another read set"
        );
        read_set_ids.push(read_set_id.clone());

        // The read set is readable back through its own Get, every member from the one
        // stored record. The status is the transient one the completion leaves behind
        // until the clock moves it, so both values are admitted here and the move itself
        // is awaited below.
        let (status, metadata) = read_set_metadata(&router, &store, &read_set_id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {metadata:?}");
        let reported_status = metadata.get("status").and_then(Value::as_str);
        assert!(
            reported_status == Some(READ_SET_STATUS_PROCESSING_UPLOAD)
                || reported_status == Some(READ_SET_STATUS_ACTIVE),
            "case {case:?}: expected a read set still being processed or already active, \
             got {metadata:?}"
        );
        assert_members(
            case,
            "GetReadSetMetadata",
            &json!({
                "id": read_set_id,
                "sequenceStoreId": store,
                "subjectId": format!("subject for {case}"),
                "sampleId": format!("sample for {case}"),
                "name": case,
                "creationType": CREATION_TYPE_UPLOAD,
            }),
            &metadata,
        );
        // A read set a direct upload formed was produced by no import job, so it carries
        // no creation job id for a caller to resolve.
        assert_absent(case, &["creationJobId"], &metadata);
        assert_members(case, "GetReadSetMetadata", &row.expected, &metadata);
        assert_absent(case, &row.absent, &metadata);
        assert_timestamp(case, "creationTime", &metadata);
        let arn = metadata
            .get("arn")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("case {case:?}: response carried no arn, got {metadata:?}"));
        assert_arn(
            case,
            arn,
            &format!("sequenceStore/{store}/readSet"),
            &read_set_id,
        );

        // The ETag of the read set's data: the algorithm is the format of that data
        // joined to the store's own ETag algorithm family, and each source's digest is
        // the digest of the bytes that source was ingested with.
        let etag = metadata
            .get("etag")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: response carried no etag, got {metadata:?}"));
        assert_members(
            case,
            "GetReadSetMetadata",
            &json!({ "algorithm": row.e_tag_algorithm }),
            &etag,
        );
        for part_source in PART_SOURCES {
            let content = expected_content(&row.parts, part_source);
            let member = file_member(part_source);
            let expected = (!content.is_empty()).then(|| Value::String(md5_hex(&content)));
            assert_eq!(
                etag.get(member),
                expected.as_ref(),
                "case {case:?}: expected the {member} ETag over that source's bytes, \
                 got {etag:?}"
            );
        }

        // One `FileInformation` per source the parts came from, read off those parts and
        // off the bytes the ingest took in.
        let files = metadata
            .get("files")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: response carried no files, got {metadata:?}"));
        for part_source in PART_SOURCES {
            let member = file_member(part_source);
            match expected_file(&row.parts, part_source) {
                Some(expected) => {
                    let file = files.get(member).unwrap_or_else(|| {
                        panic!("case {case:?}: expected a {member} file, got {files:?}")
                    });
                    assert_members(case, member, &expected, file);
                    assert_file_members(case, member, file);
                    let s3_uri = file
                        .get("s3Access")
                        .and_then(|access| access.get("s3Uri"))
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| {
                            panic!("case {case:?}: expected an s3Access.s3Uri, got {file:?}")
                        });
                    assert_read_set_file_s3_uri(
                        case,
                        s3_uri,
                        &store,
                        &read_set_id,
                        case,
                        part_source,
                        row.extension,
                    );
                }
                None => assert!(
                    files.get(member).is_none(),
                    "case {case:?}: expected no {member}, got {files:?}"
                ),
            }
        }
        // No index file is reported: nothing here builds one.
        assert_absent(case, &["index"], &files);

        // And the read set really holds the uploaded bytes, in part-number order.
        {
            let state = service_state(&*engine).expect("the omics service state");
            let registry = state.lock();
            let stored = registry
                .read_set(&store, &read_set_id)
                .unwrap_or_else(|| panic!("case {case:?}: no read set {read_set_id} is stored"));
            for part_source in PART_SOURCES {
                let expected = expected_content(&row.parts, part_source);
                match stored.sources.get(*part_source) {
                    Some(file) => assert_eq!(
                        file.content, expected,
                        "case {case:?}: {part_source} must hold its parts in part-number \
                         order, got {} bytes",
                        file.content.len()
                    ),
                    None => assert!(
                        expected.is_empty(),
                        "case {case:?}: expected a stored {part_source} file of {} bytes",
                        expected.len()
                    ),
                }
            }
            // The upload is spent: the completion consumed it.
            assert!(
                registry
                    .multipart_read_set_upload(&store, &upload)
                    .is_none(),
                "case {case:?}: upload {upload} must not survive its completion"
            );
        }

        // Which is also what the wire says: nothing can be listed on the spent upload.
        let (status, body) = list_parts(
            &router,
            &store,
            &upload,
            "",
            &json!({ "partSource": "SOURCE1" }),
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404 from ListReadSetUploadParts on a completed \
             upload, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the upload-not-found message, got {body:?}"
        );

        // The status the clock owes the read set arrives, and its creation instant does
        // not move with it.
        let settled = await_read_set_status(&router, &store, &read_set_id, READ_SET_STATUS_ACTIVE)
            .await;
        assert_eq!(
            settled.get("creationTime"),
            metadata.get("creationTime"),
            "case {case:?}: the creation instant must not move, got {settled:?}"
        );
    }
}

#[tokio::test]
async fn a_completion_this_emulator_cannot_place_is_refused() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the refusal destination").await;
    let (status, body) = create_upload(&router, &store, &upload_request("a refused completion"))
        .await;
    let upload = created_upload_id("the fixture upload", status, &body);
    let uploaded = upload_all(
        &router,
        "the fixture upload",
        &store,
        &upload,
        &[
            ("SOURCE1", 1, vec![b'A'; 32]),
            ("SOURCE2", 1, vec![b'C'; 16]),
        ],
    )
    .await;
    let good = parts_list(&uploaded);
    let checksum = uploaded[0].2.clone();
    let other_checksum = uploaded[1].2.clone();

    // Each row is a way a completion can fail to name the parts the upload holds, plus
    // the two ids it resolves; every one is refused and forms no read set.
    struct RefusalCase {
        /// How the row is reported in a failure.
        case: &'static str,
        sequence_store_id: String,
        upload_id: String,
        parts: Value,
        expected_status: u16,
        expected_code: &'static str,
        /// The modelled message, where the refusal is one this service words for the
        /// whole resource class.
        message: Option<&'static str>,
    }
    let rows = vec![
        RefusalCase {
            case: "no part at all",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            parts: Value::Array(Vec::new()),
            expected_status: 400,
            expected_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a part number the upload does not hold",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            parts: parts_list(&[("SOURCE1", 2, checksum.clone())]),
            expected_status: 400,
            expected_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a source the part was not uploaded from",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            parts: parts_list(&[("SOURCE2", 2, other_checksum.clone())]),
            expected_status: 400,
            expected_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "a checksum that is not the stored part's",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            parts: parts_list(&[("SOURCE1", 1, other_checksum.clone())]),
            expected_status: 400,
            expected_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "the same part listed twice",
            sequence_store_id: store.clone(),
            upload_id: upload.clone(),
            parts: parts_list(&[
                ("SOURCE1", 1, checksum.clone()),
                ("SOURCE1", 1, checksum.clone()),
            ]),
            expected_status: 400,
            expected_code: "ValidationException",
            message: None,
        },
        RefusalCase {
            case: "an upload id this emulator does not hold",
            sequence_store_id: store.clone(),
            upload_id: "9999999999".to_string(),
            parts: good.clone(),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            message: Some(MULTIPART_READ_SET_UPLOAD_NOT_FOUND_MESSAGE),
        },
        RefusalCase {
            case: "a sequence store this emulator does not hold",
            sequence_store_id: "9999999999".to_string(),
            upload_id: upload.clone(),
            parts: good.clone(),
            expected_status: 404,
            expected_code: "ResourceNotFoundException",
            message: Some(SEQUENCE_STORE_NOT_FOUND_MESSAGE),
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body) = complete_upload(
            &router,
            &row.sequence_store_id,
            &row.upload_id,
            &row.parts,
        )
        .await;
        assert_eq!(
            status, row.expected_status,
            "case {case:?}: expected {}, got {body:?}",
            row.expected_status
        );
        assert_eq!(
            error_code(&body),
            row.expected_code,
            "case {case:?}: expected {}, got {body:?}",
            row.expected_code
        );
        if let Some(message) = row.message {
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(message),
                "case {case:?}: expected the modelled message, got {body:?}"
            );
        }
        // Nothing was formed and nothing was spent: the upload is still there with its
        // parts, and the store holds no read set.
        let stored = stored_upload(&engine, &store, &upload);
        assert_eq!(
            stored.parts.len(),
            uploaded.len(),
            "case {case:?}: the refused completion must leave the upload's parts alone, \
             got {stored:?}"
        );
        let state = service_state(&*engine).expect("the omics service state");
        let registry = state.lock();
        assert_eq!(
            registry.read_sets_for(&store).count(),
            0,
            "case {case:?}: a refused completion must form no read set"
        );
    }

    // A read set no store holds is not found, and so is one addressed through another
    // store: a read set is resolved inside the store the request names.
    let elsewhere = create_store(&router, "another store").await;
    let listed = parts_list(&uploaded);
    let (status, body) = complete_upload(&router, &store, &upload, &listed).await;
    let read_set_id = completed_read_set_id("the accepted completion", status, &body);
    for (case, sequence_store_id, id) in [
        ("a read set id nothing holds", store.as_str(), "9999999999"),
        (
            "a read set addressed through another store",
            elsewhere.as_str(),
            read_set_id.as_str(),
        ),
    ] {
        let (status, body) = read_set_metadata(&router, sequence_store_id, id).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(READ_SET_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the read-set-not-found message, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_sequence_store_holding_a_read_set_cannot_be_deleted() {
    let (router, engine) = emulator_holding_its_engine();
    let empty = create_store(&router, "an empty store").await;
    let filled = create_store(&router, "a store with a read set").await;
    let (status, body) = create_upload(&router, &filled, &upload_request("the held read set"))
        .await;
    let upload = created_upload_id("the held read set", status, &body);
    let uploaded = upload_all(
        &router,
        "the held read set",
        &filled,
        &upload,
        &[("SOURCE1", 1, vec![b'A'; 48])],
    )
    .await;
    let (status, body) = complete_upload(&router, &filled, &upload, &parts_list(&uploaded)).await;
    let read_set_id = completed_read_set_id("the held read set", status, &body);

    // The store is not empty, so the delete is refused and the store stays.
    let (status, body) = delete_sequence_store(&router, &filled).await;
    assert_eq!(
        status, 409,
        "case \"a store holding a read set\": expected 409, got {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "case \"a store holding a read set\": expected ConflictException, got {body:?}"
    );
    assert_eq!(
        body.get("message").and_then(Value::as_str),
        Some(RESOURCE_HAS_DEPENDENTS_MESSAGE),
        "case \"a store holding a read set\": expected the modelled message, got {body:?}"
    );
    let (status, body) = read_set_metadata(&router, &filled, &read_set_id).await;
    assert_eq!(
        status, 200,
        "case \"a store holding a read set\": the read set must survive the refused \
         delete, got {body:?}"
    );

    // A store holding none deletes as before, so what the refusal is about is the read
    // set and not the delete itself.
    let (status, body) = delete_sequence_store(&router, &empty).await;
    assert_eq!(
        status, 200,
        "case \"an empty store\": expected 200, got {body:?}"
    );
    let state = service_state(&*engine).expect("the omics service state");
    let registry = state.lock();
    assert!(
        registry.sequence_store(&filled).is_some(),
        "case \"a store holding a read set\": the store must survive its refused delete"
    );
    assert!(
        registry.sequence_store(&empty).is_none(),
        "case \"an empty store\": the store must be gone"
    );
}

// ---------------------------------------------------------------------------
// ListReadSets
// ---------------------------------------------------------------------------
//
// The rules the cases below assert:
//
// * `ListReadSets` reports exactly the read sets the one sequence store the request names
//   still holds, in ascending `readSetId` order — which is creation order — each member
//   read from that read set's own stored record, so a listed read set reports what its own
//   `GetReadSetMetadata` reports and nothing else;
// * the listing pages over them by `maxResults`/`nextToken`, and a token it did not issue
//   is refused;
// * every member of the filter narrows the listing on its own, a member the filter leaves
//   out narrows nothing, and both instant bounds are inclusive;
// * a read set the store no longer holds leaves the listing;
// * a sequence store id no store holds is the modelled 404.

/// The published bounds of the modelled `maxResults` range (1..100).
/// Source: the `maxResults` member of `ListReadSetsRequest`.
const MAX_RESULTS_RANGE: (i64, i64) = (1, 100);

/// Every member the modelled `ReadSetListItem` carries.
/// Source: `smithy_query(query="shape",shape="ReadSetListItem")`.
const READ_SET_LIST_ITEM_MEMBERS: &[&str] = &[
    "id",
    "arn",
    "sequenceStoreId",
    "subjectId",
    "sampleId",
    "status",
    "name",
    "description",
    "referenceArn",
    "fileType",
    "sequenceInformation",
    "creationTime",
    "statusMessage",
    "creationType",
    "etag",
];

/// The members of `ReadSetListItem` the model makes required, which every listed read set
/// therefore has to carry.
/// Source: the same shape — `id`, `arn`, `sequenceStoreId`, `status`, `fileType` and
/// `creationTime` are `@required`.
const READ_SET_LIST_ITEM_REQUIRED: &[&str] = &[
    "id",
    "arn",
    "sequenceStoreId",
    "status",
    "fileType",
    "creationTime",
];

/// The members `GetReadSetMetadata` reports that the listing's item does NOT model, so a
/// listed read set carries neither.
/// Source: `ReadSetListItem` has no `files` and no `creationJobId` member.
const NOT_IN_READ_SET_LIST_ITEM: &[&str] = &["files", "creationJobId"];

/// The modelled `CreationType` an uploaded read set does NOT carry.
/// Source: `smithy_query(query="shape",shape="CreationType")` -> IMPORT | UPLOAD.
const CREATION_TYPE_IMPORT: &str = "IMPORT";

/// A modelled `ReadSetStatus` no read set this emulator forms ever carries.
/// Source: `smithy_query(query="shape",shape="ReadSetStatus")` -> ARCHIVED | ACTIVATING |
/// ACTIVE | DELETING | DELETED | PROCESSING_UPLOAD | UPLOAD_FAILED.
const READ_SET_STATUS_ARCHIVED: &str = "ARCHIVED";

/// `POST /sequencestore/{store}/readsets{query}`, carrying `filter` when the case names
/// one: `maxResults` and `nextToken` are `@httpQuery`, the filter is the request body.
async fn list_read_sets(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    query: &str,
    filter: Option<&Value>,
) -> (u16, Value) {
    let body = match filter {
        Some(filter) => json!({ "filter": filter }),
        None => json!({}),
    };
    call(
        router,
        "POST",
        &format!("/sequencestore/{sequence_store_id}/readsets{query}"),
        Some(&body),
    )
    .await
}

/// The `readSets` of a 200 listing response, after asserting the status.
fn listed_read_sets(case: &str, status: u16, body: &Value) -> Vec<Value> {
    assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
    body.get("readSets")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: response carried no readSets, got {body:?}"))
        .clone()
}

/// The `id` of each listed read set, in the order the listing reported them.
fn listed_ids(case: &str, items: &[Value]) -> Vec<String> {
    items
        .iter()
        .map(|item| {
            item.get("id")
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: a listed read set carried no id, got {item:?}")
                })
                .to_string()
        })
        .collect()
}

/// A row of the read set table: how the row is reported in a failure, and the members the
/// upload that forms it carries.
struct ListedReadSet {
    case: &'static str,
    upload: Value,
}

/// The read sets every listing case below is asserted over, so each rule is checked over a
/// class of read sets and not over one. The rows differ along the source format, the
/// subject and the sample they came from, where the sequence was generated, whether they
/// name a reference genome and whether they carry a description — so every filter member
/// has rows on both sides of it, sharing values across rows where a filter must select
/// several.
fn listed_read_set_table() -> Vec<ListedReadSet> {
    vec![
        ListedReadSet {
            case: "a single-source FASTQ",
            upload: json!({
                "name": "a single-source FASTQ",
                "sourceFileType": "FASTQ",
                "subjectId": "subject-alpha",
                "sampleId": "sample-one",
            }),
        },
        ListedReadSet {
            case: "a described FASTQ",
            upload: json!({
                "name": "a described FASTQ",
                "sourceFileType": "FASTQ",
                "subjectId": "subject-alpha",
                "sampleId": "sample-two",
                "description": "the second cohort",
            }),
        },
        ListedReadSet {
            case: "a FASTQ with a named origin",
            upload: json!({
                "name": "a FASTQ with a named origin",
                "sourceFileType": "FASTQ",
                "subjectId": "subject-beta",
                "sampleId": "sample-one",
                "generatedFrom": "1000 Genomes",
            }),
        },
        ListedReadSet {
            case: "an aligned CRAM",
            upload: json!({
                "name": "an aligned CRAM",
                "sourceFileType": "CRAM",
                "subjectId": "subject-beta",
                "sampleId": "sample-three",
                "referenceArn": REFERENCE_ARN,
                "description": "aligned reads",
            }),
        },
        ListedReadSet {
            case: "a UBAM",
            upload: json!({
                "name": "a UBAM",
                "sourceFileType": "UBAM",
                "subjectId": "subject-gamma",
                "sampleId": "sample-one",
            }),
        },
    ]
}

/// Form one read set per row of [`listed_read_set_table`] inside `store`, each out of one
/// uploaded part, and answer their ids in the order they were formed.
async fn form_listed_read_sets(router: &MultiServiceRouter, store: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for row in listed_read_set_table() {
        let (status, body) = create_upload(router, store, &row.upload).await;
        let upload = created_upload_id(row.case, status, &body);
        let uploaded = upload_all(
            router,
            row.case,
            store,
            &upload,
            &[("SOURCE1", 1, row.case.as_bytes().to_vec())],
        )
        .await;
        let (status, body) = complete_upload(router, store, &upload, &parts_list(&uploaded)).await;
        ids.push(completed_read_set_id(row.case, status, &body));
    }
    ids
}

/// Wait until every read set of `ids` carries the status the clock owes it, so a case that
/// reads or filters on the status sees a settled store rather than a race.
async fn await_all_active(router: &MultiServiceRouter, store: &str, ids: &[String]) {
    for id in ids {
        await_read_set_status(router, store, id, READ_SET_STATUS_ACTIVE).await;
    }
}

/// The read set ids the rows named by `cases` were formed with, in table order.
fn ids_of(cases: &[&str], ids: &[String]) -> Vec<String> {
    listed_read_set_table()
        .iter()
        .zip(ids)
        .filter(|(row, _)| cases.contains(&row.case))
        .map(|(_, id)| id.clone())
        .collect()
}

#[tokio::test]
async fn list_read_sets_reports_the_read_sets_its_store_holds() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the listed store").await;
    let elsewhere = create_store(&router, "another listed store").await;
    let ids = form_listed_read_sets(&router, &store).await;
    let elsewhere_ids = form_listed_read_sets(&router, &elsewhere).await;
    await_all_active(&router, &store, &ids).await;

    let case = "every read set the store holds";
    let (status, body) = list_read_sets(&router, &store, "", None).await;
    let items = listed_read_sets(case, status, &body);
    assert_eq!(
        listed_ids(case, &items),
        ids,
        "case {case:?}: expected the store's read sets in creation order, got {body:?}"
    );
    // The last page carries no token: everything the store holds fitted in it.
    assert_absent(case, &["nextToken"], &body);
    // Ascending `readSetId`, which is what lets a page token name a resume point.
    let keys: Vec<u64> = ids
        .iter()
        .map(|id| id.parse().expect("a numeric readSetId"))
        .collect();
    assert!(
        keys.windows(2).all(|pair| pair[0] < pair[1]),
        "case {case:?}: expected ascending read set ids, got {keys:?}"
    );

    // Every member of a listed read set is the stored record's, which is exactly what that
    // read set's own `GetReadSetMetadata` reports.
    for (row, (id, item)) in listed_read_set_table()
        .iter()
        .zip(ids.iter().zip(items.iter()))
    {
        let case = row.case;
        let (status, metadata) = read_set_metadata(&router, &store, id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {metadata:?}");
        for member in READ_SET_LIST_ITEM_MEMBERS {
            assert_eq!(
                item.get(member),
                metadata.get(member),
                "case {case:?}: the listed {member} must be the one GetReadSetMetadata \
                 reports, got {item:?} against {metadata:?}"
            );
        }
        for member in READ_SET_LIST_ITEM_REQUIRED {
            assert!(
                item.get(member).is_some(),
                "case {case:?}: a listed read set must carry {member}, got {item:?}"
            );
        }
        assert_absent(case, NOT_IN_READ_SET_LIST_ITEM, item);
        assert_members(
            case,
            "ListReadSets",
            &json!({
                "id": id,
                "sequenceStoreId": store,
                "name": case,
                "status": READ_SET_STATUS_ACTIVE,
                "creationType": CREATION_TYPE_UPLOAD,
            }),
            item,
        );
        assert_timestamp(case, "creationTime", item);
    }

    // A read set belongs to the store that holds it: the other store's read sets are
    // neither in this listing nor mixed into its own.
    for id in &elsewhere_ids {
        assert!(
            !ids.contains(id),
            "case {case:?}: read set {id} was minted for another store"
        );
    }
    let case = "the other store's own listing";
    let (status, body) = list_read_sets(&router, &elsewhere, "", None).await;
    let items = listed_read_sets(case, status, &body);
    assert_eq!(
        listed_ids(case, &items),
        elsewhere_ids,
        "case {case:?}: expected only the other store's read sets, got {body:?}"
    );
}

#[tokio::test]
async fn list_read_sets_narrows_on_every_filter_member() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the filtered store").await;
    let ids = form_listed_read_sets(&router, &store).await;
    await_all_active(&router, &store, &ids).await;

    /// A row of the filter table: the filter a case sends and the rows it must select.
    struct FilterCase {
        case: &'static str,
        filter: Value,
        selects: &'static [&'static str],
    }
    let every_row: &[&str] = &[
        "a single-source FASTQ",
        "a described FASTQ",
        "a FASTQ with a named origin",
        "an aligned CRAM",
        "a UBAM",
    ];
    let rows = vec![
        FilterCase {
            case: "a filter naming no member at all",
            filter: json!({}),
            selects: every_row,
        },
        FilterCase {
            case: "a name one read set carries",
            filter: json!({ "name": "a described FASTQ" }),
            selects: &["a described FASTQ"],
        },
        FilterCase {
            case: "a name nothing carries",
            filter: json!({ "name": "no read set is called this" }),
            selects: &[],
        },
        FilterCase {
            case: "the status every settled read set carries",
            filter: json!({ "status": READ_SET_STATUS_ACTIVE }),
            selects: every_row,
        },
        FilterCase {
            case: "a modelled status none carries",
            filter: json!({ "status": READ_SET_STATUS_ARCHIVED }),
            selects: &[],
        },
        FilterCase {
            case: "a sample three read sets share",
            filter: json!({ "sampleId": "sample-one" }),
            selects: &[
                "a single-source FASTQ",
                "a FASTQ with a named origin",
                "a UBAM",
            ],
        },
        FilterCase {
            case: "a subject two read sets share",
            filter: json!({ "subjectId": "subject-beta" }),
            selects: &["a FASTQ with a named origin", "an aligned CRAM"],
        },
        FilterCase {
            case: "where the sequence was generated",
            filter: json!({ "generatedFrom": "1000 Genomes" }),
            selects: &["a FASTQ with a named origin"],
        },
        FilterCase {
            case: "the creation type an uploaded read set carries",
            filter: json!({ "creationType": CREATION_TYPE_UPLOAD }),
            selects: every_row,
        },
        FilterCase {
            case: "the other modelled creation type",
            filter: json!({ "creationType": CREATION_TYPE_IMPORT }),
            selects: &[],
        },
        FilterCase {
            case: "a reference genome one read set is aligned against",
            filter: json!({ "referenceArn": REFERENCE_ARN }),
            selects: &["an aligned CRAM"],
        },
        FilterCase {
            case: "a reference genome nothing is aligned against",
            filter: json!({
                "referenceArn":
                    "arn:aws:omics:us-west-2:000000000000:referenceStore/1000000000/reference/9",
            }),
            selects: &[],
        },
        FilterCase {
            // The modelled `ReferenceArnFilter` pattern is `^$|^arn:.+$` with length
            // 0..127, so the empty string is a value the member admits; it names no
            // reference genome, so it narrows nothing.
            case: "the empty reference ARN the model admits",
            filter: json!({ "referenceArn": "" }),
            selects: every_row,
        },
        FilterCase {
            case: "two members that agree on one read set",
            filter: json!({ "name": "a UBAM", "sampleId": "sample-one" }),
            selects: &["a UBAM"],
        },
        FilterCase {
            case: "two members that agree on none",
            filter: json!({ "name": "a UBAM", "sampleId": "sample-two" }),
            selects: &[],
        },
        FilterCase {
            case: "a subject and a sample that meet in one read set",
            filter: json!({ "subjectId": "subject-alpha", "sampleId": "sample-one" }),
            selects: &["a single-source FASTQ"],
        },
    ];

    for row in &rows {
        let case = row.case;
        let (status, body) = list_read_sets(&router, &store, "", Some(&row.filter)).await;
        let items = listed_read_sets(case, status, &body);
        assert_eq!(
            listed_ids(case, &items),
            ids_of(row.selects, &ids),
            "case {case:?}: expected the read sets {:?}, got {body:?}",
            row.selects
        );
    }
}

#[tokio::test]
async fn list_read_sets_bounds_the_listing_by_creation_instant() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the time-filtered store").await;
    let ids = form_listed_read_sets(&router, &store).await;
    await_all_active(&router, &store, &ids).await;

    // What each read set reports as its own creation instant: the wire value a filter
    // bound carries, and the instant the expectation is computed from — so the rule is
    // asserted against the store's own instants rather than against an assumed ordering.
    let mut instants: Vec<(String, String, f64)> = Vec::new();
    for id in &ids {
        let (status, metadata) = read_set_metadata(&router, &store, id).await;
        assert_eq!(status, 200, "read set {id}: expected 200, got {metadata:?}");
        let text = metadata
            .get("creationTime")
            .and_then(Value::as_str)
            .unwrap_or_else(|| panic!("read set {id}: no creationTime, got {metadata:?}"))
            .to_string();
        let seconds = assert_timestamp(id, "creationTime", &metadata);
        instants.push((id.clone(), text, seconds));
    }

    for (id, text, seconds) in &instants {
        // Both bounds are INCLUSIVE, so a read set's own instant selects it.
        let case = format!("createdAfter the instant of read set {id}");
        let (status, body) =
            list_read_sets(&router, &store, "", Some(&json!({ "createdAfter": text }))).await;
        let items = listed_read_sets(&case, status, &body);
        let expected: Vec<String> = instants
            .iter()
            .filter(|(_, _, other)| other >= seconds)
            .map(|(other, _, _)| other.clone())
            .collect();
        assert_eq!(
            listed_ids(&case, &items),
            expected,
            "case {case:?}: expected every read set created at or after {text}, got {body:?}"
        );

        let case = format!("createdBefore the instant of read set {id}");
        let (status, body) =
            list_read_sets(&router, &store, "", Some(&json!({ "createdBefore": text }))).await;
        let items = listed_read_sets(&case, status, &body);
        let expected: Vec<String> = instants
            .iter()
            .filter(|(_, _, other)| other <= seconds)
            .map(|(other, _, _)| other.clone())
            .collect();
        assert_eq!(
            listed_ids(&case, &items),
            expected,
            "case {case:?}: expected every read set created at or before {text}, got {body:?}"
        );

        // The two bounds together are a window, and the narrowest one still holds the read
        // set whose instant it was taken from.
        let case = format!("the one-instant window of read set {id}");
        let (status, body) = list_read_sets(
            &router,
            &store,
            "",
            Some(&json!({ "createdAfter": text, "createdBefore": text })),
        )
        .await;
        let items = listed_read_sets(&case, status, &body);
        assert!(
            listed_ids(&case, &items).contains(id),
            "case {case:?}: expected the window to hold read set {id}, got {body:?}"
        );
    }

    // A window that closes before the store held anything selects nothing, and one that
    // opens after it selects nothing either.
    let (earliest, _, _) = instants.first().expect("at least one read set");
    let (status, metadata) = read_set_metadata(&router, &store, earliest).await;
    assert_eq!(status, 200, "expected 200, got {metadata:?}");
    for (case, filter) in [
        (
            "a window closing before the store held anything",
            json!({ "createdBefore": "2001-01-01T00:00:00Z" }),
        ),
        (
            "a window opening long after it",
            json!({ "createdAfter": "2999-01-01T00:00:00Z" }),
        ),
    ] {
        let (status, body) = list_read_sets(&router, &store, "", Some(&filter)).await;
        let items = listed_read_sets(case, status, &body);
        assert!(
            items.is_empty(),
            "case {case:?}: expected no read set, got {body:?}"
        );
    }
}

#[tokio::test]
async fn list_read_sets_pages_over_its_store_in_creation_order() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the paged store").await;
    let ids = form_listed_read_sets(&router, &store).await;
    await_all_active(&router, &store, &ids).await;

    let (smallest, largest) = MAX_RESULTS_RANGE;
    for page_size in [smallest, 2, 3, ids.len() as i64, largest] {
        let case = format!("maxResults={page_size}");
        let mut seen: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        loop {
            let query = match &token {
                Some(token) => format!("?maxResults={page_size}&nextToken={token}"),
                None => format!("?maxResults={page_size}"),
            };
            let (status, body) = list_read_sets(&router, &store, &query, None).await;
            let items = listed_read_sets(&case, status, &body);
            assert!(
                items.len() as i64 <= page_size,
                "case {case:?}: expected at most {page_size} read sets in a page, got {body:?}"
            );
            seen.extend(listed_ids(&case, &items));
            assert!(
                seen.len() <= ids.len(),
                "case {case:?}: the pages reported more read sets than the store holds, \
                 got {seen:?}"
            );
            match body.get("nextToken").and_then(Value::as_str) {
                Some(next) => token = Some(next.to_string()),
                None => break,
            }
        }
        assert_eq!(
            seen, ids,
            "case {case:?}: paging must report every read set exactly once, in creation order"
        );
    }

    // A page size outside the modelled range is refused by the constraint walk, so the
    // page a request can ask for is bounded on both sides.
    for page_size in [smallest - 1, largest + 1] {
        let case = format!("maxResults={page_size}");
        let (status, body) =
            list_read_sets(&router, &store, &format!("?maxResults={page_size}"), None).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // A `nextToken` this listing did not issue resumes nothing, so it is refused rather
    // than read as a position in the store.
    for (case, token) in [
        ("a token this service never issued", "notatoken".to_string()),
        (
            "another listing's token",
            format!(
                "{MULTIPART_READ_SET_UPLOAD_PAGE_TOKEN_TAG}{}",
                hex::encode(&ids[0])
            ),
        ),
        (
            "a read set token naming no read set",
            READ_SET_PAGE_TOKEN_TAG.to_string(),
        ),
    ] {
        let (status, body) =
            list_read_sets(&router, &store, &format!("?nextToken={token}"), None).await;
        assert_eq!(status, 400, "case {case:?}: expected 400, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ValidationException",
            "case {case:?}: expected ValidationException, got {body:?}"
        );
    }

    // And the token this listing DOES issue resumes it: the page after the first read set
    // is the rest of the store.
    let case = "the token the first page issued";
    let (status, body) = list_read_sets(&router, &store, "?maxResults=1", None).await;
    let items = listed_read_sets(case, status, &body);
    assert_eq!(
        listed_ids(case, &items),
        ids[..1].to_vec(),
        "case {case:?}: expected the first read set, got {body:?}"
    );
    let token = body
        .get("nextToken")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: expected a nextToken, got {body:?}"));
    let (status, body) = list_read_sets(
        &router,
        &store,
        &format!("?nextToken={token}&maxResults={largest}"),
        None,
    )
    .await;
    let items = listed_read_sets(case, status, &body);
    assert_eq!(
        listed_ids(case, &items),
        ids[1..].to_vec(),
        "case {case:?}: expected the rest of the store, got {body:?}"
    );
}

#[tokio::test]
async fn a_read_set_its_store_no_longer_holds_leaves_the_listing() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the emptied store").await;
    let ids = form_listed_read_sets(&router, &store).await;
    await_all_active(&router, &store, &ids).await;

    // A read set leaves the store through `BatchDeleteReadSet`, which is the call a client
    // makes, so it leaves the way a client makes it leave rather than by writing the
    // stored record from the test.
    let released = ids[1].clone();
    let (status, body) = batch_delete_read_set(&router, &store, &[released.as_str()]).await;
    assert_eq!(
        status, 200,
        "expected 200 from BatchDeleteReadSet, got {body:?}"
    );
    {
        let state = service_state(&*engine).expect("the omics service state");
        let registry = state.lock();
        assert!(
            registry.read_set(&store, &released).is_none(),
            "the deleted read set must be gone from the store's read sets"
        );
    }

    let case = "a read set the store no longer holds";
    let (status, body) = list_read_sets(&router, &store, "", None).await;
    let items = listed_read_sets(case, status, &body);
    let expected: Vec<String> = ids.iter().filter(|id| **id != released).cloned().collect();
    assert_eq!(
        listed_ids(case, &items),
        expected,
        "case {case:?}: expected the released read set to leave the listing, got {body:?}"
    );

    // Including when the filter asks for its status by name: it is no longer among the
    // store's read sets at all.
    let (status, body) = list_read_sets(
        &router,
        &store,
        "",
        Some(&json!({ "status": READ_SET_STATUS_DELETED })),
    )
    .await;
    let items = listed_read_sets(case, status, &body);
    assert!(
        items.is_empty(),
        "case {case:?}: expected no read set, got {body:?}"
    );
}

#[tokio::test]
async fn list_read_sets_refuses_a_sequence_store_nothing_holds() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the only store").await;

    // However the id is shaped inside the modelled `SequenceStoreId` (len 10..36, decimal):
    // one no store holds is the modelled 404, and so is one wider than any id this
    // emulator mints.
    for (case, sequence_store_id) in [
        ("an id no store holds", "9999999999".to_string()),
        ("an id at the top of the modelled length", "9".repeat(36)),
        (
            "an id one digit past the ids this emulator mints",
            format!("{}0", FIRST_SEQUENCE_STORE_ID),
        ),
    ] {
        let (status, body) = list_read_sets(&router, &sequence_store_id, "", None).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(SEQUENCE_STORE_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the modelled message, got {body:?}"
        );
    }

    // A store that holds no read set yet is an EMPTY listing, not a refusal.
    let case = "a store holding no read set";
    let (status, body) = list_read_sets(&router, &store, "", None).await;
    let items = listed_read_sets(case, status, &body);
    assert!(
        items.is_empty(),
        "case {case:?}: expected an empty listing, got {body:?}"
    );
    assert_absent(case, &["nextToken"], &body);
}

// ---------------------------------------------------------------------------
// The metadata an ingest derives, and the delete that releases a read set
// ---------------------------------------------------------------------------
//
// The rules the cases below assert:
//
// * a read set formed by a direct upload is reported with the metadata the service
//   derived from the bytes it ingested — the ETag of each source's sequence data under
//   the store's own ETag algorithm family, the digest of each ingested file, the read and
//   base counts of the records those files hold, the alignment the source format implies,
//   the S3 location each file was ingested to, and the part size the upload itself was
//   conducted with — and with NO creation job, because no import job produced it;
// * a read set deleted through `BatchDeleteReadSet` stops being held by its store, so the
//   store's own delete succeeds, while a store that still holds a read set is refused.

/// A FASTQ file of one read of four bases.
const ONE_READ_FASTQ: &[u8] = b"@read1\nACGT\n+\nIIII\n";

/// A FASTQ file of three reads of five, seven and two bases.
const THREE_READ_FASTQ: &[u8] =
    b"@r1\nACGTA\n+\nIIIII\n@r2\nTTTTTTT\n+\nIIIIIII\n@r3\nGG\n+\nII\n";

/// A FASTQ file of two reads of ten and two bases: one mate of a pair.
const FORWARD_FASTQ: &[u8] = b"@a\nACGTACGTAC\n+\nIIIIIIIIII\n@b\nTT\n+\nII\n";

/// Its other mate: two reads of four and five bases.
const REVERSE_FASTQ: &[u8] = b"@a/2\nGGGG\n+\nIIII\n@b/2\nCCCCC\n+\nIIIII\n";

/// Every value the modelled `ETagAlgorithmFamily` enum admits.
/// Source: `smithy_query(query="shape",shape="ETagAlgorithmFamily")` -> MD5up | SHA256up
/// | SHA512up.
const E_TAG_ALGORITHM_FAMILIES: &[&str] = &["MD5up", "SHA256up", "SHA512up"];

/// The ETag algorithm family a store created without one ingests under.
/// Source: `CreateSequenceStoreRequest$eTagAlgorithmFamily` ("The default value is
/// MD5up").
const DEFAULT_FAMILY: &str = "MD5up";

/// `content` as a gzip stream, which is how a FASTQ read set is uploaded compressed.
fn gzipped(content: &[u8]) -> Vec<u8> {
    use std::io::Write;
    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(content)
        .expect("the gzip encoder takes the FASTQ");
    encoder.finish().expect("a complete gzip stream")
}

/// The digest `family` names, over `data`, rendered as lowercase hexadecimal: what the
/// `<format>_<family>` ETag of a read set's source file must be.
fn family_digest(family: &str, data: &[u8]) -> String {
    use sha2::Digest;
    match family {
        "MD5up" => md5_hex(data),
        "SHA256up" => hex::encode(sha2::Sha256::digest(data)),
        "SHA512up" => hex::encode(sha2::Sha512::digest(data)),
        other => panic!("unmodelled ETag algorithm family {other:?}"),
    }
}

/// `POST /sequencestore`, naming the ETag algorithm family the store ingests under when
/// the row asks for one.
async fn create_store_ingesting_under(
    router: &MultiServiceRouter,
    name: &str,
    family: Option<&str>,
) -> String {
    let mut request = json!({ "name": name });
    if let Some(family) = family {
        assert!(
            E_TAG_ALGORITHM_FAMILIES.contains(&family),
            "case {name:?}: {family:?} is not a modelled ETagAlgorithmFamily"
        );
        request["eTagAlgorithmFamily"] = json!(family);
    }
    let (status, body) = call(router, "POST", "/sequencestore", Some(&request)).await;
    assert_eq!(
        status, 200,
        "case {name:?}: expected 200 from CreateSequenceStore, got {body:?}"
    );
    body.get("id")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {name:?}: response carried no id, got {body:?}"))
        .to_string()
}

/// A row of the ingest table: what was uploaded, what sequence data those bytes hold, and
/// what the service must therefore report for the read set.
struct IngestCase {
    /// How the row is reported in a failure, and the read set's name.
    case: &'static str,
    /// The ETag algorithm family the store is created with; `None` leaves it at the
    /// service's default.
    family: Option<&'static str>,
    source_file_type: &'static str,
    /// The parts uploaded: source file, part number, content.
    parts: Vec<(&'static str, i32, Vec<u8>)>,
    /// The sequence data each source's bytes hold — the content itself for a plaintext
    /// upload, the uncompressed records for a gzipped one — which is what the ETag is
    /// taken over and what the counts come from.
    sequence_data: Vec<(&'static str, Vec<u8>)>,
    /// The modelled `ETagAlgorithm` the read set must be reported under.
    e_tag_algorithm: &'static str,
    /// The extension of the object each source file was ingested to.
    extension: &'static str,
    /// The reads and bases the ingested records hold together.
    counts: (i64, i64),
}

/// The table: one row per way the bytes a read set is formed from can differ, so the rule
/// is asserted over the class and not over the one recorded upload. The rows differ along
/// whether the ingested file is gzip-compressed, how many records it holds, how many
/// bases those records hold, how many source files the read set has, how many parts
/// formed a file, and which ETag algorithm family the store ingests under.
fn ingest_cases() -> Vec<IngestCase> {
    let split = gzipped(THREE_READ_FASTQ);
    let (head, tail) = split.split_at(split.len() / 2);
    vec![
        IngestCase {
            case: "a gzipped FASTQ of one read",
            family: None,
            source_file_type: "FASTQ",
            parts: vec![("SOURCE1", 1, gzipped(ONE_READ_FASTQ))],
            sequence_data: vec![("SOURCE1", ONE_READ_FASTQ.to_vec())],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq.gz",
            counts: (1, 4),
        },
        IngestCase {
            case: "a plaintext FASTQ of three reads",
            family: Some("MD5up"),
            source_file_type: "FASTQ",
            parts: vec![("SOURCE1", 1, THREE_READ_FASTQ.to_vec())],
            sequence_data: vec![("SOURCE1", THREE_READ_FASTQ.to_vec())],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq",
            counts: (3, 14),
        },
        IngestCase {
            case: "a store that ingests under SHA256up",
            family: Some("SHA256up"),
            source_file_type: "FASTQ",
            parts: vec![("SOURCE1", 1, FORWARD_FASTQ.to_vec())],
            sequence_data: vec![("SOURCE1", FORWARD_FASTQ.to_vec())],
            e_tag_algorithm: "FASTQ_SHA256up",
            extension: "fastq",
            counts: (2, 12),
        },
        IngestCase {
            case: "a store that ingests under SHA512up",
            family: Some("SHA512up"),
            source_file_type: "FASTQ",
            parts: vec![("SOURCE1", 1, gzipped(FORWARD_FASTQ))],
            sequence_data: vec![("SOURCE1", FORWARD_FASTQ.to_vec())],
            e_tag_algorithm: "FASTQ_SHA512up",
            extension: "fastq.gz",
            counts: (2, 12),
        },
        IngestCase {
            case: "a paired-end gzipped FASTQ",
            family: None,
            source_file_type: "FASTQ",
            parts: vec![
                ("SOURCE1", 1, gzipped(FORWARD_FASTQ)),
                ("SOURCE2", 1, gzipped(REVERSE_FASTQ)),
            ],
            sequence_data: vec![
                ("SOURCE1", FORWARD_FASTQ.to_vec()),
                ("SOURCE2", REVERSE_FASTQ.to_vec()),
            ],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq.gz",
            // The reads and bases of both mates together.
            counts: (4, 21),
        },
        IngestCase {
            case: "one gzipped FASTQ split over two parts of different sizes",
            family: None,
            source_file_type: "FASTQ",
            parts: vec![
                ("SOURCE1", 1, head.to_vec()),
                ("SOURCE1", 2, tail.to_vec()),
            ],
            sequence_data: vec![("SOURCE1", THREE_READ_FASTQ.to_vec())],
            e_tag_algorithm: "FASTQ_MD5up",
            extension: "fastq.gz",
            counts: (3, 14),
        },
    ]
}

#[tokio::test]
async fn an_uploaded_read_set_reports_what_the_ingest_derived_from_its_bytes() {
    let (router, _engine) = emulator_holding_its_engine();

    for row in ingest_cases() {
        let case = row.case;
        let family = row.family.unwrap_or(DEFAULT_FAMILY);
        let store = create_store_ingesting_under(&router, case, row.family).await;
        let (status, body) = create_upload(
            &router,
            &store,
            &json!({
                "name": case,
                "sourceFileType": row.source_file_type,
                "subjectId": format!("subject for {case}"),
                "sampleId": format!("sample for {case}"),
            }),
        )
        .await;
        let upload = created_upload_id(case, status, &body);
        let listed = upload_all(&router, case, &store, &upload, &row.parts).await;
        let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&listed)).await;
        let read_set_id = completed_read_set_id(case, status, &body);

        let (status, metadata) = read_set_metadata(&router, &store, &read_set_id).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {metadata:?}");

        // The counts and the alignment the ingest read out of the records, and nothing
        // generated from: the upload named no source.
        let (reads, bases) = row.counts;
        assert_eq!(
            metadata.get("sequenceInformation"),
            Some(&json!({
                "alignment": "UNALIGNED",
                "totalReadCount": reads,
                "totalBaseCount": bases,
            })),
            "case {case:?}: expected the counts of the ingested records, got {metadata:?}"
        );

        // The ETag: the algorithm names the data's format and the store's family, and
        // each source's digest is that family's digest of that source's SEQUENCE DATA —
        // not of the compressed object, which is what `files.<source>.md5` reports.
        let mut expected_e_tag = json!({ "algorithm": row.e_tag_algorithm });
        for (part_source, sequence_data) in &row.sequence_data {
            expected_e_tag[file_member(part_source)] =
                json!(family_digest(family, sequence_data));
        }
        assert_eq!(
            metadata.get("etag"),
            Some(&expected_e_tag),
            "case {case:?}: expected the ETag of the ingested sequence data, got {metadata:?}"
        );

        // And one file per source, each reporting the parts that formed it, the upload's
        // own part size, the length and digest of the bytes ingested, and where the file
        // was ingested to.
        let files = metadata
            .get("files")
            .cloned()
            .unwrap_or_else(|| panic!("case {case:?}: response carried no files, got {metadata:?}"));
        for part_source in PART_SOURCES {
            let member = file_member(part_source);
            let content = expected_content(&row.parts, part_source);
            if content.is_empty() {
                assert!(
                    files.get(member).is_none(),
                    "case {case:?}: expected no {member}, got {files:?}"
                );
                continue;
            }
            let file = files
                .get(member)
                .unwrap_or_else(|| panic!("case {case:?}: expected a {member}, got {files:?}"));
            assert_file_members(case, member, file);
            assert_eq!(
                file.get("totalParts").and_then(Value::as_i64),
                Some(parts_of_source(&row.parts, part_source).len() as i64),
                "case {case:?}: expected the parts that formed {member}, got {file:?}"
            );
            assert_eq!(
                file.get("partSize").and_then(Value::as_i64),
                Some(READ_SET_UPLOAD_PART_SIZE),
                "case {case:?}: expected the upload's own part size for {member}, \
                 got {file:?}"
            );
            assert_eq!(
                file.get("contentLength").and_then(Value::as_i64),
                Some(content.len() as i64),
                "case {case:?}: expected the length of the ingested {member}, got {file:?}"
            );
            assert_eq!(
                file.get("md5").and_then(Value::as_str),
                Some(md5_hex(&content).as_str()),
                "case {case:?}: expected the digest of the ingested {member}, got {file:?}"
            );
            let s3_uri = file
                .get("s3Access")
                .and_then(|access| access.get("s3Uri"))
                .and_then(Value::as_str)
                .unwrap_or_else(|| {
                    panic!("case {case:?}: expected an s3Access.s3Uri for {member}, got {file:?}")
                });
            assert_read_set_file_s3_uri(
                case,
                s3_uri,
                &store,
                &read_set_id,
                case,
                part_source,
                row.extension,
            );
        }

        // No import job produced it, so there is no creation job id to resolve.
        assert_absent(case, &["creationJobId"], &metadata);

        // The listing reads the same record, so it reports the same ETag.
        let (status, body) = list_read_sets(&router, &store, "", None).await;
        let items = listed_read_sets(case, status, &body);
        let item = items
            .iter()
            .find(|item| item.get("id").and_then(Value::as_str) == Some(read_set_id.as_str()))
            .unwrap_or_else(|| panic!("case {case:?}: the read set is not listed, got {body:?}"));
        assert_eq!(
            item.get("etag"),
            Some(&expected_e_tag),
            "case {case:?}: expected the listing to report the same ETag, got {item:?}"
        );
        assert_eq!(
            item.get("sequenceInformation"),
            metadata.get("sequenceInformation"),
            "case {case:?}: expected the listing to report the same sequence information, \
             got {item:?}"
        );
    }
}

/// `POST /sequencestore/{store}/readset/batch/delete` with `ids` as the list.
async fn batch_delete_read_set(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    ids: &[&str],
) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/sequencestore/{sequence_store_id}/readset/batch/delete"),
        Some(&json!({ "ids": ids })),
    )
    .await
}

/// Form one read set in `store` out of a single part, and answer its id.
async fn form_read_set(router: &MultiServiceRouter, store: &str, name: &str) -> String {
    let (status, body) = create_upload(router, store, &upload_request(name)).await;
    let upload = created_upload_id(name, status, &body);
    let listed = upload_all(
        router,
        name,
        store,
        &upload,
        &[("SOURCE1", 1, ONE_READ_FASTQ.to_vec())],
    )
    .await;
    let (status, body) = complete_upload(router, store, &upload, &parts_list(&listed)).await;
    completed_read_set_id(name, status, &body)
}

#[tokio::test]
async fn a_read_set_deleted_in_a_batch_releases_its_store() {
    let (router, engine) = emulator_holding_its_engine();

    // Each row is a way a batch can name read sets: one id, several at once, an id whose
    // read set is already deleted (the delete is `@idempotent`), and an id the store
    // holds no read set for, which is the one failure a batch reports.
    struct DeleteCase {
        case: &'static str,
        /// How many read sets the store is given.
        held: usize,
        /// Which of those read sets the batch names, by index, and how many ids it
        /// repeats.
        named: Vec<usize>,
        /// Whether the batch also names an id no read set carries.
        unheld_id: Option<&'static str>,
    }

    for row in [
        DeleteCase {
            case: "one read set",
            held: 1,
            named: vec![0],
            unheld_id: None,
        },
        DeleteCase {
            case: "every read set the store holds",
            held: 3,
            named: vec![0, 1, 2],
            unheld_id: None,
        },
        DeleteCase {
            case: "an id named twice, which deletes it once",
            held: 1,
            named: vec![0, 0],
            unheld_id: None,
        },
        DeleteCase {
            case: "an id no read set carries, beside one that exists",
            held: 1,
            named: vec![0],
            unheld_id: Some("9999999999"),
        },
    ] {
        let case = row.case;
        let store = create_store(&router, case).await;
        let mut ids = Vec::new();
        for index in 0..row.held {
            ids.push(form_read_set(&router, &store, &format!("{case} {index}")).await);
        }

        let mut named: Vec<&str> = row.named.iter().map(|index| ids[*index].as_str()).collect();
        if let Some(unheld) = row.unheld_id {
            named.push(unheld);
        }
        let (status, body) = batch_delete_read_set(&router, &store, &named).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        match row.unheld_id {
            // Exactly one error, for the one id the store holds no read set for, carrying
            // the identity and the sentence this service words for a read set nothing
            // holds.
            Some(unheld) => assert_eq!(
                body.get("errors"),
                Some(&json!([{
                    "id": unheld,
                    "code": "ResourceNotFoundException",
                    "message": READ_SET_NOT_FOUND_MESSAGE,
                }])),
                "case {case:?}: expected the one unheld id to be reported, got {body:?}"
            ),
            // Every id was deleted, so there is no failure to report at all.
            None => assert_absent(case, &["errors"], &body),
        }

        // Each read set the batch named is GONE: its id resolves to nothing, so the
        // per-id read a client makes is the read-set-not-found refusal.
        for index in &row.named {
            let id = &ids[*index];
            let (status, metadata) = read_set_metadata(&router, &store, id).await;
            assert_eq!(status, 404, "case {case:?}: expected 404, got {metadata:?}");
            assert_eq!(
                error_code(&metadata),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {metadata:?}"
            );
            assert_eq!(
                metadata.get("message").and_then(Value::as_str),
                Some(READ_SET_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the read-set-not-found message, got {metadata:?}"
            );
        }

        // And the store no longer holds them: a store whose every read set is deleted is
        // empty, so its own delete succeeds with an empty body; a store that still holds
        // one is refused.
        let still_held: Vec<usize> = (0..row.held).filter(|i| !row.named.contains(i)).collect();
        let (status, body) = delete_sequence_store(&router, &store).await;
        if still_held.is_empty() {
            assert_eq!(
                status, 200,
                "case {case:?}: expected the emptied store to delete, got {body:?}"
            );
            assert_eq!(
                body,
                json!({}),
                "case {case:?}: expected an empty body, got {body:?}"
            );
            let state = service_state(&*engine).expect("the omics service state");
            let registry = state.lock();
            assert!(
                registry.sequence_store(&store).is_none(),
                "case {case:?}: the store must be gone"
            );
            continue;
        }
        assert_eq!(
            status, 409,
            "case {case:?}: a store still holding a read set must be refused, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(RESOURCE_HAS_DEPENDENTS_MESSAGE),
            "case {case:?}: expected the modelled message, got {body:?}"
        );

        // Deleting the rest empties it, and then it deletes.
        let rest: Vec<&str> = still_held.iter().map(|index| ids[*index].as_str()).collect();
        let (status, body) = batch_delete_read_set(&router, &store, &rest).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        let (status, body) = delete_sequence_store(&router, &store).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the emptied store to delete, got {body:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_delete_this_emulator_cannot_place_is_refused() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the batch delete destination").await;
    let read_set_id = form_read_set(&router, &store, "the read set another store cannot reach")
        .await;
    let elsewhere = create_store(&router, "another store").await;

    // A store this emulator does not hold is the modelled 404 for the whole call: no id
    // in it names anything. So is a read set addressed through another store — it is not
    // a read set that store holds — which the batch reports as that id's own failure.
    for (case, sequence_store_id, expected_status) in [
        ("a sequence store id no store holds", "9999999999", 404),
        ("a read set addressed through another store", elsewhere.as_str(), 200),
    ] {
        let (status, body) = batch_delete_read_set(&router, sequence_store_id, &[&read_set_id])
            .await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status}, got {body:?}"
        );
        if expected_status == 404 {
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException, got {body:?}"
            );
            assert_eq!(
                body.get("message").and_then(Value::as_str),
                Some(SEQUENCE_STORE_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the modelled message, got {body:?}"
            );
            continue;
        }
        assert_eq!(
            body.get("errors"),
            Some(&json!([{
                "id": read_set_id,
                "code": "ResourceNotFoundException",
                "message": READ_SET_NOT_FOUND_MESSAGE,
            }])),
            "case {case:?}: expected the id to be reported as not deleted, got {body:?}"
        );
    }

    // And the read set its own store holds survived every refusal.
    let (status, metadata) = read_set_metadata(&router, &store, &read_set_id).await;
    assert_eq!(
        status, 200,
        "the read set must survive a batch addressed to another store, got {metadata:?}"
    );
    assert_eq!(
        metadata.get("id").and_then(Value::as_str),
        Some(read_set_id.as_str()),
        "expected the surviving read set to be reported, got {metadata:?}"
    );
}

/// The published bounds of the modelled `ReadSetIdList` length (1..100) — the same limit
/// the operation documents for one call.
/// Source: `smithy_query(query="shape",shape="ReadSetIdList")` -> length min 1, max 100.
const READ_SET_ID_LIST_LENGTH: (usize, usize) = (1, 100);

/// The `n`th of a run of decimal ids of the modelled `ReadSetId` width (10..36) that no
/// read set carries, so a batch can name as many unheld ids as its bound admits.
fn unheld_read_set_id(n: usize) -> String {
    format!("{}", 9_000_000_000_u64 + n as u64)
}

/// One entry of the modelled error list, as a batch words the one failure a read set
/// delete reports.
fn expected_batch_error(id: &str) -> Value {
    json!({
        "id": id,
        "code": "ResourceNotFoundException",
        "message": READ_SET_NOT_FOUND_MESSAGE,
    })
}

#[tokio::test]
async fn a_batch_delete_is_bounded_by_the_modelled_id_list_length() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the bounded batch's store").await;
    // A read set the store holds that no batch below names: a batch refused at the
    // boundary must delete nothing at all, and an accepted one must touch only the ids it
    // names.
    let bystander = form_read_set(&router, &store, "the read set no batch names").await;

    let (min, max) = READ_SET_ID_LIST_LENGTH;
    // Each published bound of the list, and one id past each of them.
    for count in [min - 1, min, max, max + 1] {
        let case = format!("{count} ids");
        let ids: Vec<String> = (0..count).map(unheld_read_set_id).collect();
        assert!(
            !ids.contains(&bystander),
            "case {case:?}: the unheld ids must not name the store's own read set \
             {bystander}"
        );
        let named: Vec<&str> = ids.iter().map(String::as_str).collect();
        let (status, body) = batch_delete_read_set(&router, &store, &named).await;

        if count < min || count > max {
            // Outside the modelled length, so the request never reaches the store.
            assert_eq!(
                status, 400,
                "case {case:?}: a list outside {min}..{max} must be refused, got {body:?}"
            );
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        } else {
            assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
            // Every id in it names nothing, so every id is reported once, in the order
            // the request listed them.
            let expected = Value::Array(
                ids.iter()
                    .map(|id| expected_batch_error(id))
                    .collect::<Vec<Value>>(),
            );
            assert_eq!(
                body.get("errors"),
                Some(&expected),
                "case {case:?}: expected one error per named id, got {body:?}"
            );
        }

        // Whatever the batch answered, the read set it never named is still there.
        let (status, metadata) = read_set_metadata(&router, &store, &bystander).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a read set no batch named must survive, got {metadata:?}"
        );
        assert_eq!(
            metadata.get("id").and_then(Value::as_str),
            Some(bystander.as_str()),
            "case {case:?}: expected the bystander to be reported, got {metadata:?}"
        );
    }
}

#[tokio::test]
async fn a_batch_delete_reports_each_id_it_could_not_delete_in_request_order() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the interleaved batch's store").await;
    let first = form_read_set(&router, &store, "the first read set the batch names").await;
    let second = form_read_set(&router, &store, "the second read set the batch names").await;
    let unheld: Vec<String> = (0..3).map(unheld_read_set_id).collect();
    for id in [&first, &second] {
        assert!(
            !unheld.contains(id),
            "the unheld ids must name no read set the store holds, got {id}"
        );
    }

    // The ids interleave what the store holds with what it does not, so each failure is
    // attributed to its own id rather than to the batch, and the ids that do name a read
    // set are deleted beside them.
    let named: Vec<&str> = vec![
        unheld[0].as_str(),
        first.as_str(),
        unheld[1].as_str(),
        second.as_str(),
        unheld[2].as_str(),
    ];
    let first_pass_errors = Value::Array(
        unheld
            .iter()
            .map(|id| expected_batch_error(id))
            .collect::<Vec<Value>>(),
    );
    // A second pass names the same ids, and by then the store holds a read set for none
    // of them: the two the first pass deleted are gone, so every id is reported, in the
    // order the request listed them.
    let second_pass_errors = Value::Array(
        named
            .iter()
            .map(|id| expected_batch_error(id))
            .collect::<Vec<Value>>(),
    );

    for (pass, expected) in [
        ("the first pass", &first_pass_errors),
        ("the second pass", &second_pass_errors),
    ] {
        let (status, body) = batch_delete_read_set(&router, &store, &named).await;
        assert_eq!(status, 200, "case {pass:?}: expected 200, got {body:?}");
        assert_eq!(
            body.get("errors"),
            Some(expected),
            "case {pass:?}: expected the ids naming no read set to be reported, got {body:?}"
        );

        // And each id the store did hold is gone: its per-id read is not found.
        for id in [&first, &second] {
            let (status, metadata) = read_set_metadata(&router, &store, id).await;
            assert_eq!(
                status, 404,
                "case {pass:?}: expected 404 for read set {id}, got {metadata:?}"
            );
            assert_eq!(
                error_code(&metadata),
                "ResourceNotFoundException",
                "case {pass:?}: expected ResourceNotFoundException for {id}, got {metadata:?}"
            );
            assert_eq!(
                metadata.get("message").and_then(Value::as_str),
                Some(READ_SET_NOT_FOUND_MESSAGE),
                "case {pass:?}: expected the read-set-not-found message, got {metadata:?}"
            );
        }
    }
}

/// A row of the gone-after-delete table: what the store holds, which of those read sets
/// the batch names, whether it also names an id no read set carries, and whether the read
/// sets had settled to their terminal status before the delete.
struct GoneCase {
    case: &'static str,
    held: usize,
    named: Vec<usize>,
    also_named: Option<&'static str>,
    settle_first: bool,
}

#[tokio::test]
async fn a_read_set_a_batch_delete_reports_no_error_for_is_gone_from_every_read() {
    let (router, engine) = emulator_holding_its_engine();

    // One row per way a delete can name read sets, so the rule is asserted over the class
    // and not over one request: how many read sets the store holds, how many of them the
    // batch names (one, several, every one of them, the same one twice), whether the batch
    // also names an id no read set carries — the partial-failure shape, where the only
    // error reported is that id's — and whether the named read sets had settled to their
    // terminal status first.
    for row in [
        GoneCase {
            case: "the store's only read set, deleted before it settles",
            held: 1,
            named: vec![0],
            also_named: None,
            settle_first: false,
        },
        GoneCase {
            case: "one settled read set of three",
            held: 3,
            named: vec![1],
            also_named: None,
            settle_first: true,
        },
        GoneCase {
            case: "two read sets of three, beside an id no read set carries",
            held: 3,
            named: vec![0, 2],
            also_named: Some("9999999999"),
            settle_first: true,
        },
        GoneCase {
            case: "every read set the store holds",
            held: 2,
            named: vec![0, 1],
            also_named: None,
            settle_first: false,
        },
        GoneCase {
            case: "the same read set named twice in one call, which deletes it once",
            held: 2,
            named: vec![1, 1],
            also_named: None,
            settle_first: true,
        },
    ] {
        let case = row.case;
        let store = create_store(&router, case).await;
        let mut ids = Vec::new();
        for index in 0..row.held {
            ids.push(form_read_set(&router, &store, &format!("{case} {index}")).await);
        }
        if row.settle_first {
            for id in &ids {
                await_read_set_status(&router, &store, id, READ_SET_STATUS_ACTIVE).await;
            }
        }

        let mut named: Vec<&str> = row.named.iter().map(|index| ids[*index].as_str()).collect();
        if let Some(also) = row.also_named {
            named.push(also);
        }
        let (status, body) = batch_delete_read_set(&router, &store, &named).await;
        assert_eq!(status, 200, "case {case:?}: expected 200, got {body:?}");
        // Only the id no read set carries is reported, so each id the batch did delete was
        // deleted outright rather than reported as a failure.
        match row.also_named {
            Some(also) => assert_eq!(
                body.get("errors"),
                Some(&json!([expected_batch_error(also)])),
                "case {case:?}: expected only the unheld id to be reported, got {body:?}"
            ),
            None => assert_absent(case, &["errors"], &body),
        }

        // Every id the batch deleted resolves to nothing afterwards: the per-id read is
        // the read-set-not-found refusal, and no record for it is left in the state.
        let state = service_state(&*engine).expect("the omics service state");
        for index in &row.named {
            let id = &ids[*index];
            let (status, metadata) = read_set_metadata(&router, &store, id).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for read set {id}, got {metadata:?}"
            );
            assert_eq!(
                error_code(&metadata),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for {id}, got {metadata:?}"
            );
            assert_eq!(
                metadata.get("message").and_then(Value::as_str),
                Some(READ_SET_NOT_FOUND_MESSAGE),
                "case {case:?}: expected the read-set-not-found message, got {metadata:?}"
            );
            let registry = state.lock();
            assert!(
                registry.read_set(&store, id).is_none(),
                "case {case:?}: the state must hold no read set {id} any more"
            );
        }

        // And the store is exactly the read sets the batch did not name — every one of
        // them still readable through its own id.
        let surviving: Vec<String> = (0..row.held)
            .filter(|index| !row.named.contains(index))
            .map(|index| ids[index].clone())
            .collect();
        let (status, body) = list_read_sets(&router, &store, "", None).await;
        let items = listed_read_sets(case, status, &body);
        assert_eq!(
            listed_ids(case, &items),
            surviving,
            "case {case:?}: expected only the read sets no id named, got {body:?}"
        );
        for id in &surviving {
            let (status, metadata) = read_set_metadata(&router, &store, id).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected 200 for the surviving read set {id}, got {metadata:?}"
            );
            assert_eq!(
                metadata.get("id").and_then(Value::as_str),
                Some(id.as_str()),
                "case {case:?}: expected read set {id} to be reported, got {metadata:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_set_id_the_named_store_does_not_hold_is_not_found() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the store the reads name").await;
    let elsewhere = create_store(&router, "another store").await;
    let held = form_read_set(&router, &store, "the read set this store holds").await;
    let deleted = form_read_set(&router, &store, "the read set a delete takes away").await;
    let (status, body) = batch_delete_read_set(&router, &store, &[deleted.as_str()]).await;
    assert_eq!(
        status, 200,
        "expected 200 from BatchDeleteReadSet, got {body:?}"
    );

    let (min, max) = READ_SET_ID_WIDTH;
    // Every way the store the request names can hold no read set for a syntactically valid
    // `ReadSetId`: an id nothing was ever minted for (at each published width of the id),
    // one just past the ids this emulator mints, one whose read set another store holds,
    // and one whose read set a delete took away.
    for (case, sequence_store_id, id) in [
        (
            "an id no read set carries, at the bottom of the modelled width",
            store.as_str(),
            "9".repeat(min),
        ),
        (
            "an id no read set carries, at the top of the modelled width",
            store.as_str(),
            "9".repeat(max),
        ),
        (
            "an id one digit past the ids this emulator mints",
            store.as_str(),
            format!("{}0", FIRST_SEQUENCE_STORE_ID + READ_SET_ID_OFFSET),
        ),
        (
            "a read set addressed through another store",
            elsewhere.as_str(),
            held.clone(),
        ),
        (
            "a read set a batch delete took away",
            store.as_str(),
            deleted.clone(),
        ),
    ] {
        let (status, body) = read_set_metadata(&router, sequence_store_id, &id).await;
        assert_eq!(status, 404, "case {case:?}: expected 404, got {body:?}");
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(READ_SET_NOT_FOUND_MESSAGE),
            "case {case:?}: expected the read-set-not-found message, got {body:?}"
        );
        // Nothing of the request is echoed into that sentence: it is fixed text for the
        // whole class, so neither id appears in it.
        let message = body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(
            !message.contains(&id) && !message.contains(sequence_store_id),
            "case {case:?}: the message must carry no id from the request, got {message:?}"
        );
    }

    // The read set its own store holds is untouched by every refusal above.
    let (status, metadata) = read_set_metadata(&router, &store, &held).await;
    assert_eq!(status, 200, "expected 200 for the held read set, got {metadata:?}");
    assert_eq!(
        metadata.get("id").and_then(Value::as_str),
        Some(held.as_str()),
        "expected the held read set to be reported, got {metadata:?}"
    );
}

// ---------------------------------------------------------------------------
// Reading a read set's data back
// ---------------------------------------------------------------------------
//
// The rules the cases below assert:
//
// * a read of a read set's data serves ONE part of ONE of its files: the bytes that part
//   was ingested with, unchanged — so a part read back is byte for byte the part that was
//   uploaded, a compressed part is served still compressed, and the parts of a file in
//   order are the file the metadata describes;
//   * a file's parts are counted from one in the order they formed it, as its `totalParts`
//     reports, whatever part numbers the upload itself used;
//   * the file read is the modelled `ReadSetFile` the request names, and the read set's
//     first source file when the request names none;
// * a part the file does not hold — a number past the parts that formed it — and a file
//   the read set does not hold are the not-found refusal: that part is simply not there;
// * a read set whose files the service is still forming cannot have its data served, and
//   the same read answers the stored part once the read set is active.

/// `GET /sequencestore/{store}/readset/{id}?partNumber=..[&file=..]`, with the RAW response
/// body beside the response document.
///
/// That is the one wire form this operation binds: `payload` is an `@httpPayload` streaming
/// blob, so a success carries bytes rather than JSON and the harness's JSON `call` cannot
/// express it. A refusal still carries the modelled error document, which is parsed beside
/// those bytes so one helper serves both.
pub(crate) async fn get_read_set(
    router: &MultiServiceRouter,
    sequence_store_id: &str,
    read_set_id: &str,
    file: Option<&str>,
    part_number: i32,
) -> (u16, Vec<u8>, Value) {
    let file_query = file.map(|file| format!("&file={file}")).unwrap_or_default();
    let uri = format!(
        "/sequencestore/{sequence_store_id}/readset/{read_set_id}\
         ?partNumber={part_number}{file_query}"
    );
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", format!("omics.{REGION}.amazonaws.com"))
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential={AKID}/20260101/{REGION}/omics/aws4_request, \
                 SignedHeaders=host, Signature=0000"
            ),
        )
        .body(bytes::Bytes::new())
        .expect("a well-formed test request");

    let response = router.dispatch(request).await;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get("x-amzn-errortype")
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let raw = response.into_body().to_vec();
    // A payload of bytes is not JSON, so a success parses as nothing: only a refusal has a
    // document, and the `x-amzn-errortype` header is folded into it the way `call` does.
    let mut parsed: Value = serde_json::from_slice(&raw).unwrap_or(Value::Null);
    if let (Some(error_type), Value::Object(map)) = (error_type, &mut parsed) {
        map.entry("__type")
            .or_insert_with(|| Value::String(error_type));
    }
    (status, raw, parsed)
}

/// `content` read back out of the gzip stream `gzipped` wrote it into.
fn gunzipped(content: &[u8]) -> Vec<u8> {
    use std::io::Read;
    let mut decoded = Vec::new();
    flate2::read::GzDecoder::new(content)
        .read_to_end(&mut decoded)
        .expect("the gzip decoder takes the served payload");
    decoded
}

/// A row of the data-read table: the parts one read set is formed from.
struct DataReadCase {
    case: &'static str,
    /// The parts uploaded into it: source file, part number, content.
    parts: Vec<(&'static str, i32, Vec<u8>)>,
}

/// The table: one row per way the parts a read set is formed from can differ, so the rule
/// is asserted over the class and not over one request. The rows differ along how many
/// parts formed a file, the sizes of those parts, whether the uploaded bytes are
/// compressed, which part NUMBERS the upload used (consecutive, and sparse up to the top of
/// the modelled range), and how many source files the read set holds.
fn data_read_cases() -> Vec<DataReadCase> {
    let (_, last_number) = PART_NUMBER_RANGE;
    let stream = gzipped(THREE_READ_FASTQ);
    let third = stream.len() / 3;
    vec![
        DataReadCase {
            case: "one plaintext part",
            parts: vec![("SOURCE1", 1, ONE_READ_FASTQ.to_vec())],
        },
        DataReadCase {
            case: "one gzipped part",
            parts: vec![("SOURCE1", 1, gzipped(ONE_READ_FASTQ))],
        },
        DataReadCase {
            case: "three parts of different sizes",
            parts: vec![
                ("SOURCE1", 1, stream[..third].to_vec()),
                ("SOURCE1", 2, stream[third..third * 2 + 1].to_vec()),
                ("SOURCE1", 3, stream[third * 2 + 1..].to_vec()),
            ],
        },
        DataReadCase {
            case: "parts the upload numbered sparsely",
            parts: vec![
                ("SOURCE1", 1, FORWARD_FASTQ.to_vec()),
                ("SOURCE1", last_number, REVERSE_FASTQ.to_vec()),
            ],
        },
        DataReadCase {
            case: "a paired read set, two parts of one source and one of the other",
            parts: vec![
                ("SOURCE1", 1, FORWARD_FASTQ.to_vec()),
                ("SOURCE1", 2, ONE_READ_FASTQ.to_vec()),
                ("SOURCE2", 1, REVERSE_FASTQ.to_vec()),
            ],
        },
    ]
}

#[tokio::test]
async fn a_read_of_a_read_set_serves_the_part_it_was_ingested_with() {
    let (router, _engine) = emulator_holding_its_engine();

    for row in data_read_cases() {
        let case = row.case;
        let store = create_store(&router, case).await;
        let (status, body) = create_upload(&router, &store, &upload_request(case)).await;
        let upload = created_upload_id(case, status, &body);
        let listed = upload_all(&router, case, &store, &upload, &row.parts).await;
        let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&listed)).await;
        let read_set = completed_read_set_id(case, status, &body);
        let metadata =
            await_read_set_status(&router, &store, &read_set, READ_SET_STATUS_ACTIVE).await;

        // The parts of each source file, in the ascending part-number order they formed
        // that file in — which is the order the file's own parts are counted in, from one.
        let mut by_source: std::collections::BTreeMap<&str, Vec<Vec<u8>>> =
            std::collections::BTreeMap::new();
        let mut uploaded: Vec<(&str, i32, Vec<u8>)> = row
            .parts
            .iter()
            .map(|(source, number, payload)| (*source, *number, payload.clone()))
            .collect();
        uploaded.sort_by_key(|(source, number, _)| (*source, *number));
        for (source, _, payload) in uploaded {
            by_source.entry(source).or_default().push(payload);
        }

        for (&source, parts) in &by_source {
            let mut file = Vec::new();
            for (index, expected) in parts.iter().enumerate() {
                let part_number = index as i32 + 1;
                let (status, payload, body) =
                    get_read_set(&router, &store, &read_set, Some(source), part_number).await;
                assert_eq!(
                    status, 200,
                    "case {case:?}: expected 200 for part {part_number} of {source}, \
                     got {status} {body:?}"
                );
                assert_eq!(
                    payload, *expected,
                    "case {case:?}: part {part_number} of {source} must be served byte for \
                     byte as uploaded, got {payload:?}"
                );
                file.extend_from_slice(&payload);
            }

            // The parts served for the file ARE the file the metadata describes: as many
            // parts as it reports, holding as many bytes as it reports.
            let reported = metadata
                .get("files")
                .and_then(|files| files.get(source.to_lowercase()))
                .cloned()
                .unwrap_or_else(|| {
                    panic!("case {case:?}: metadata reports no {source} file, got {metadata:?}")
                });
            assert_eq!(
                reported.get("totalParts").and_then(Value::as_i64),
                Some(parts.len() as i64),
                "case {case:?}: expected {source} to report the parts that formed it, \
                 got {reported:?}"
            );
            assert_eq!(
                reported.get("contentLength").and_then(Value::as_i64),
                Some(file.len() as i64),
                "case {case:?}: expected {source}'s length to be the bytes its parts served, \
                 got {reported:?}"
            );

            // One past the parts that formed the file names no part of it.
            let past = parts.len() as i32 + 1;
            let (status, payload, body) =
                get_read_set(&router, &store, &read_set, Some(source), past).await;
            assert_eq!(
                status, 404,
                "case {case:?}: expected 404 for part {past} of {source}, got {status} \
                 {body:?} with {} bytes",
                payload.len()
            );
            assert_eq!(
                error_code(&body),
                "ResourceNotFoundException",
                "case {case:?}: expected ResourceNotFoundException for part {past} of \
                 {source}, got {body:?}"
            );
        }

        // A read naming no file serves the read set's first source file.
        let first = by_source
            .get("SOURCE1")
            .unwrap_or_else(|| panic!("case {case:?}: every row uploads a SOURCE1 file"));
        for index in 0..first.len() {
            let part_number = index as i32 + 1;
            let (status, defaulted, body) =
                get_read_set(&router, &store, &read_set, None, part_number).await;
            let (named_status, named, _) =
                get_read_set(&router, &store, &read_set, Some("SOURCE1"), part_number).await;
            assert_eq!(
                (status, &defaulted),
                (named_status, &named),
                "case {case:?}: a read naming no file must answer part {part_number} of \
                 SOURCE1, got {status} {body:?} with {} bytes",
                defaulted.len()
            );
        }
    }
}

#[tokio::test]
async fn a_compressed_part_is_served_still_compressed() {
    // The service answers a read set in the format it was uploaded in, so what comes back
    // is the gzip stream the caller sent and never the data inside it.
    let (router, _engine) = emulator_holding_its_engine();

    for (case, plain) in [
        ("one read", ONE_READ_FASTQ),
        ("three reads", THREE_READ_FASTQ),
        ("one mate of a pair", FORWARD_FASTQ),
    ] {
        let compressed = gzipped(plain);
        let store = create_store(&router, case).await;
        let (status, body) = create_upload(&router, &store, &upload_request(case)).await;
        let upload = created_upload_id(case, status, &body);
        let listed = upload_all(
            &router,
            case,
            &store,
            &upload,
            &[("SOURCE1", 1, compressed.clone())],
        )
        .await;
        let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&listed)).await;
        let read_set = completed_read_set_id(case, status, &body);
        await_read_set_status(&router, &store, &read_set, READ_SET_STATUS_ACTIVE).await;

        let (status, payload, body) = get_read_set(&router, &store, &read_set, None, 1).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected 200, got {status} {body:?}"
        );
        assert_eq!(
            payload, compressed,
            "case {case:?}: expected the uploaded gzip stream, got {payload:?}"
        );
        assert_ne!(
            payload.as_slice(),
            plain,
            "case {case:?}: the service must not decompress what it was given"
        );
        // And the served bytes really are that stream: they decode back to the records.
        assert_eq!(
            gunzipped(&payload),
            plain,
            "case {case:?}: the served payload must decompress to the uploaded records"
        );
    }
}

#[tokio::test]
async fn a_read_of_a_part_or_a_file_a_read_set_does_not_hold_is_not_found() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the store holding the read set").await;
    let elsewhere = create_store(&router, "another store").await;
    let case = "a read set of two parts from one source";
    let (status, body) = create_upload(&router, &store, &upload_request(case)).await;
    let upload = created_upload_id(case, status, &body);
    let listed = upload_all(
        &router,
        case,
        &store,
        &upload,
        &[
            ("SOURCE1", 1, FORWARD_FASTQ.to_vec()),
            ("SOURCE1", 2, REVERSE_FASTQ.to_vec()),
        ],
    )
    .await;
    let (status, body) = complete_upload(&router, &store, &upload, &parts_list(&listed)).await;
    let read_set = completed_read_set_id(case, status, &body);
    await_read_set_status(&router, &store, &read_set, READ_SET_STATUS_ACTIVE).await;

    let (_, last_number) = PART_NUMBER_RANGE;
    // Every way a read can name data this read set does not hold: a part number just past
    // the parts that formed the file and one at the top of the modelled range, the source
    // file the read set has no part from, the index file this service builds none of, a
    // read set id the store holds nothing for, the read set addressed through another
    // store, and a sequence store this emulator does not hold.
    for (case, sequence_store_id, read_set_id, file, part_number, message) in [
        (
            "one part past the parts that formed the file",
            store.as_str(),
            read_set.as_str(),
            None,
            3,
            READ_SET_PART_NOT_FOUND_MESSAGE,
        ),
        (
            "a part number at the top of the modelled range",
            store.as_str(),
            read_set.as_str(),
            None,
            last_number,
            READ_SET_PART_NOT_FOUND_MESSAGE,
        ),
        (
            "the source file this read set holds no part from",
            store.as_str(),
            read_set.as_str(),
            Some("SOURCE2"),
            1,
            READ_SET_PART_NOT_FOUND_MESSAGE,
        ),
        (
            "the index file this service builds none of",
            store.as_str(),
            read_set.as_str(),
            Some(READ_SET_FILE_INDEX),
            1,
            READ_SET_PART_NOT_FOUND_MESSAGE,
        ),
        (
            "a read set id the store holds nothing for",
            store.as_str(),
            "9999999999",
            None,
            1,
            READ_SET_NOT_FOUND_MESSAGE,
        ),
        (
            "the read set addressed through another store",
            elsewhere.as_str(),
            read_set.as_str(),
            None,
            1,
            READ_SET_NOT_FOUND_MESSAGE,
        ),
        (
            "a sequence store this emulator does not hold",
            "9999999999",
            read_set.as_str(),
            None,
            1,
            SEQUENCE_STORE_NOT_FOUND_MESSAGE,
        ),
    ] {
        let (status, payload, body) = get_read_set(
            &router,
            sequence_store_id,
            read_set_id,
            file,
            part_number,
        )
        .await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {body:?} with {} bytes",
            payload.len()
        );
        assert_eq!(
            error_code(&body),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {body:?}"
        );
        assert_eq!(
            body.get("message").and_then(Value::as_str),
            Some(message),
            "case {case:?}: expected the message this service words that refusal with, \
             got {body:?}"
        );
    }

    // The parts the read set does hold are untouched by every refusal above.
    for (part_number, expected) in [(1, FORWARD_FASTQ), (2, REVERSE_FASTQ)] {
        let (status, payload, body) =
            get_read_set(&router, &store, &read_set, None, part_number).await;
        assert_eq!(
            status, 200,
            "expected 200 for part {part_number}, got {status} {body:?}"
        );
        assert_eq!(
            payload.as_slice(),
            expected,
            "expected part {part_number} to be served as uploaded, got {payload:?}"
        );
    }
}

#[tokio::test]
async fn a_part_number_outside_the_modelled_range_is_refused() {
    let (router, _engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the store holding the read set").await;
    let read_set = form_read_set(&router, &store, "the read set a read names").await;
    await_read_set_status(&router, &store, &read_set, READ_SET_STATUS_ACTIVE).await;

    let (first, last) = PART_NUMBER_RANGE;
    // Each published bound of the modelled range and one past it: inside the range the
    // answer is about what the read set holds — its only part, and a part it does not have
    // — while outside it the request itself is refused.
    for (case, part_number, expected_status) in [
        ("one below the bottom of the range", first - 1, 400),
        ("the bottom of the range", first, 200),
        ("the top of the range", last, 404),
        ("one past the top of the range", last + 1, 400),
    ] {
        let (status, payload, body) =
            get_read_set(&router, &store, &read_set, None, part_number).await;
        assert_eq!(
            status, expected_status,
            "case {case:?}: expected {expected_status} for partNumber {part_number}, got \
             {status} {body:?} with {} bytes",
            payload.len()
        );
        if expected_status == 400 {
            assert_eq!(
                error_code(&body),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {body:?}"
            );
        }
    }
}

#[tokio::test]
async fn a_read_set_still_being_processed_cannot_have_its_data_read() {
    let (router, engine) = emulator_holding_its_engine();
    let store = create_store(&router, "the store holding the read set").await;
    let read_set = form_read_set(&router, &store, "the read set a read names").await;
    await_read_set_status(&router, &store, &read_set, READ_SET_STATUS_ACTIVE).await;
    let state = service_state(&*engine).expect("the omics service state");

    // Held in the transient status a completion leaves behind, with no transition owed, so
    // what is asserted is the refusal that status carries rather than when the clock
    // happened to move it.
    {
        let mut registry = state.lock();
        let held = registry
            .read_sets
            .values_mut()
            .find(|held| held.id == read_set)
            .expect("the read set the completion formed");
        held.status = READ_SET_STATUS_PROCESSING_UPLOAD.to_string();
        held.status_due_at_ms = None;
    }

    let (status, payload, body) = get_read_set(&router, &store, &read_set, None, 1).await;
    assert_eq!(
        status, 409,
        "expected 409 while the read set is {READ_SET_STATUS_PROCESSING_UPLOAD}, got \
         {status} {body:?}"
    );
    assert_eq!(
        error_code(&body),
        "ConflictException",
        "expected ConflictException, got {body:?}"
    );
    assert_ne!(
        payload.as_slice(),
        ONE_READ_FASTQ,
        "a refusal must carry no part of the read set's data"
    );

    // The same read answers the stored part once the read set is active again.
    {
        let mut registry = state.lock();
        let held = registry
            .read_sets
            .values_mut()
            .find(|held| held.id == read_set)
            .expect("the read set the completion formed");
        held.status = READ_SET_STATUS_ACTIVE.to_string();
    }
    let (status, payload, body) = get_read_set(&router, &store, &read_set, None, 1).await;
    assert_eq!(status, 200, "expected 200, got {status} {body:?}");
    assert_eq!(
        payload.as_slice(),
        ONE_READ_FASTQ,
        "expected the stored part, got {payload:?}"
    );
}
