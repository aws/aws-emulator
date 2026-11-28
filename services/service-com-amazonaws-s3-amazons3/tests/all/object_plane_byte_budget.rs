// SPDX-License-Identifier: CC0-1.0

//! What ONE bucket may hold: the traffic AWS accepts, not a slice of it.
//!
//! Every assertion drives a real HTTP request through the router, so what is pinned is the
//! wire behaviour a client doing ordinary object traffic sees.
//!
//! The rule under test: the emulator's capacity guard is not part of the modelled API
//! surface, and nothing about it is judged per bucket. S3 constrains the size of ONE payload
//! — the 5 MiB minimum for a part other than the last and the 5 GiB maximum for a part or a
//! `PutObject` body (Amazon S3 User Guide, *Amazon S3 multipart upload limits*) — and bounds
//! what a bucket HOLDS not at all, while `UploadPart` models no error whatsoever
//! (`smithy_query(query="operation",operation="UploadPart")` reports `errors: []`). So the
//! one budget this emulator keeps to bound its own memory is sized ABOVE the traffic AWS
//! accepts: a write whose size S3 accepts is RECORDED however many MiB the bucket already
//! holds, and a multipart upload of many MiB completes and reads back byte-exact.
//!
//! The rows differ along what the rule generalises over: how the bytes arrive (a one-shot
//! `PutObject`, a multipart upload split at the part size the high-level clients choose),
//! how many MiB the bucket already holds when they do, and the sizes themselves — each
//! object here is larger than a bucket's whole allowance used to be, and the bucket ends up
//! holding several times it.

use service_com_amazonaws_s3_amazons3::state::MAX_RETAINED_BYTES_TOTAL;
use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, send_bytes, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::seeded_payload;

/// The part size the AWS CLI and the SDKs' transfer managers split an upload into by
/// default: 8 MiB (Amazon S3 User Guide, *Amazon S3 multipart upload limits* — "part size:
/// 5 MiB to 5 GiB"; the default the high-level clients choose is 8 MiB).
const DEFAULT_PART_BYTES: usize = 8 * 1024 * 1024;

/// How large one uploaded file is in these rows: 12 MiB, which the default part size splits
/// into an 8 MiB part and a 4 MiB one.
const FILE_BYTES: usize = 12 * 1024 * 1024;

/// Upload `parts` at `key` as a multipart upload, asserting every step is SERVED — an
/// `UploadPart` that is refused is exactly the defect this suite is about, so each one is
/// asserted on its own.
async fn upload_in_parts(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    parts: &[Vec<u8>],
) -> Answer {
    let initiated = send(router, "POST", &format!("/{name}/{key}?uploads"), &[], "").await;
    assert_eq!(
        initiated.status,
        200,
        "the initiation of {key:?} must answer HTTP 200, got {} with body {:?}",
        initiated.status,
        text(&initiated)
    );
    let upload_id = initiated
        .element("UploadId")
        .expect("an initiation must report an UploadId");

    let mut payload = String::from("<CompleteMultipartUpload>");
    for (index, part) in parts.iter().enumerate() {
        let part_number = index + 1;
        let uploaded = send_bytes(
            router,
            "PUT",
            &format!("/{name}/{key}?partNumber={part_number}&uploadId={upload_id}"),
            &[],
            part,
        )
        .await;
        assert_eq!(
            uploaded.status,
            200,
            "uploading part {part_number} ({} bytes) of {key:?} must be recorded, got {} \
             with body {:?}",
            part.len(),
            uploaded.status,
            text(&uploaded)
        );
        let etag = uploaded
            .header("etag")
            .expect("a part upload must report an ETag header");
        payload.push_str(&format!(
            "<Part><PartNumber>{part_number}</PartNumber><ETag>{etag}</ETag></Part>"
        ));
    }
    payload.push_str("</CompleteMultipartUpload>");
    send(
        router,
        "POST",
        &format!("/{name}/{key}?uploadId={upload_id}"),
        &[],
        &payload,
    )
    .await
}

#[tokio::test]
async fn one_bucket_takes_more_bytes_than_a_single_upload_carries() {
    let router = router();
    let name = "unbounded-bucket";
    bucket(&router, name).await;

    // Three 12 MiB files into ONE bucket: two through multipart uploads split at the default
    // part size (so the bucket already holds 12 MiB when the second upload's parts arrive,
    // and 24 MiB when the third write does) and one through a single `PutObject`.
    for (index, key) in ["uploads/first.bin", "uploads/second.bin"]
        .iter()
        .enumerate()
    {
        let seed = 0x1000_0000u32 + index as u32;
        let parts = vec![
            seeded_payload(seed, DEFAULT_PART_BYTES),
            seeded_payload(seed ^ 0x5555_5555, FILE_BYTES - DEFAULT_PART_BYTES),
        ];
        let completed = upload_in_parts(&router, name, key, &parts).await;
        assert_eq!(
            completed.status,
            200,
            "case {key:?}: the completion must answer HTTP 200, got {} with body {:?}",
            completed.status,
            text(&completed)
        );

        // Read back byte-exact: the object is the parts concatenated, in order.
        let whole: Vec<u8> = parts.concat();
        let read = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            read.status, 200,
            "case {key:?}: the assembled object must be readable, got {} with body {:?}",
            read.status,
            text(&read)
        );
        assert_eq!(
            read.header("content-length"),
            Some(whole.len().to_string()),
            "case {key:?}: expected {} bytes, got {:?}",
            whole.len(),
            read.header("content-length")
        );
        assert!(
            read.body.as_ref() == whole.as_slice(),
            "case {key:?}: expected the bytes that were uploaded, got {} of them",
            read.body.len()
        );
    }

    // And a one-shot write of the same size into the same bucket, which now holds 24 MiB.
    let third = seeded_payload(0x3000_0000, FILE_BYTES);
    let stored = put_object(&router, name, "uploads/third.bin", &third, &[]).await;
    assert_eq!(
        stored.status,
        200,
        "case \"a one-shot write into a bucket already holding 24 MiB\": expected the write \
         to be recorded, got {} with body {:?}",
        stored.status,
        text(&stored)
    );
    let read = get_object(&router, name, "uploads/third.bin", "", &[]).await;
    assert_eq!(
        read.header("content-length"),
        Some(third.len().to_string()),
        "case \"a one-shot write into a bucket already holding 24 MiB\": expected {} bytes, \
         got {:?}",
        third.len(),
        read.header("content-length")
    );
    assert!(
        read.body.as_ref() == third.as_slice(),
        "case \"a one-shot write into a bucket already holding 24 MiB\": expected the bytes \
         that were written, got {} of them",
        read.body.len()
    );
}

#[tokio::test]
async fn the_budgets_are_above_the_traffic_aws_accepts() {
    // The structural half of the same rule: the ONE budget that bounds this process's
    // memory is sized above an ordinary session's multipart traffic — many files of this
    // size, in one bucket or spread over several — so no write AWS accepts meets it on the
    // way, and no budget of a BUCKET's own exists for one to meet instead.
    let store_allowance = MAX_RETAINED_BYTES_TOTAL;
    assert!(
        store_allowance >= 32 * FILE_BYTES,
        "case \"the whole store's allowance\": it must take many files of {FILE_BYTES} \
         bytes, got {store_allowance}"
    );
}
