// SPDX-License-Identifier: CC0-1.0

//! The PROBE a parallel-download client makes before it fans out, end to end.
//!
//! Every parallel-download path (the AWS CRT, `s3transfer`, `aws s3 cp`) discovers an
//! object's part boundaries with ONE part-scoped `HeadObject` and then reads the object in
//! pieces — by part number or by byte range — so this suite drives that whole workflow
//! through the router and reassembles the object from the pieces it got back.
//!
//! The rule under test, over a multipart object and a one-shot one: the pieces a client is
//! told about add up to the object. A part-scoped read reports the object's part count
//! (`PartsCount`) and the span of the part it served (`Content-Range`), each span is the
//! bytes of exactly that part, and reading every span in order — once by part number, once
//! by the byte ranges those `Content-Range` headers name — reproduces the object's bytes
//! and its reported size.
//!
//! The spans and counts here are COMPUTED from the parts each case uploads rather than
//! written down, so the suite spans part counts (one, two, three) and part sizes rather
//! than one recorded object.

use smithy_server_runtime::MultiServiceRouter;

use super::bucket_lifecycle::{router, send, Answer};
use super::object_and_tagging_roundtrip::{bucket, get_object, put_object, text};
use super::object_partial_reads::{completed_multipart_object, seeded_payload};

/// The fewest bytes a part other than the last one may carry: "Part size: 5 MiB to 5 GiB.
/// There is no minimum size limit on the last part of your multipart upload" (Amazon S3 User
/// Guide, *Amazon S3 multipart upload limits*).
const MIN_MULTIPART_PART_BYTES: usize = 5 * 1024 * 1024;

/// `length` bytes from `seed` — see [`seeded_payload`].
fn sized_payload(seed: u32, length: usize) -> Vec<u8> {
    seeded_payload(seed, length)
}

/// `HEAD /<bucket>/<key><query>` — `HeadObject`.
async fn head_object(
    router: &MultiServiceRouter,
    name: &str,
    key: &str,
    query: &str,
) -> Answer {
    send(router, "HEAD", &format!("/{name}/{key}{query}"), &[], "").await
}

/// The `bytes <first>-<last>/<total>` a `Content-Range` names, as `(first, last, total)`.
fn parsed_content_range(answer: &Answer) -> (usize, usize, usize) {
    let header = answer
        .header("content-range")
        .unwrap_or_else(|| panic!("a partial read must report a Content-Range, got none"));
    let span = header
        .strip_prefix("bytes ")
        .unwrap_or_else(|| panic!("a Content-Range must name the bytes unit, got {header:?}"));
    let (span, total) = span
        .split_once('/')
        .unwrap_or_else(|| panic!("a Content-Range must name the total, got {header:?}"));
    let (first, last) = span
        .split_once('-')
        .unwrap_or_else(|| panic!("a Content-Range must name a span, got {header:?}"));
    let number = |text: &str| {
        text.parse::<usize>()
            .unwrap_or_else(|_| panic!("a Content-Range must carry whole numbers, got {header:?}"))
    };
    (number(first), number(last), number(total))
}

#[tokio::test]
async fn the_part_discovery_probe_reads_the_whole_object_back_in_pieces() {
    let router = router();

    // One bucket per case, so each object's parts and assembled bytes are kept apart from
    // the next case's.
    let cases: [(&str, &str, Vec<Vec<u8>>, bool); 4] = [
        (
            "a two-part upload",
            "probe-two",
            vec![
                sized_payload(1, MIN_MULTIPART_PART_BYTES),
                sized_payload(2, 1_024),
            ],
            true,
        ),
        (
            "a three-part upload",
            "probe-three",
            vec![
                sized_payload(3, MIN_MULTIPART_PART_BYTES),
                sized_payload(4, MIN_MULTIPART_PART_BYTES),
                sized_payload(5, 7),
            ],
            true,
        ),
        (
            "a single-part upload",
            "probe-one",
            vec![sized_payload(6, 3_333)],
            true,
        ),
        (
            // A one-shot write was not uploaded as a multipart upload, so it reports no part
            // count — but a part-scoped read of it is still the whole object, which is what
            // lets a client probe any key.
            "a one-shot write",
            "probe-whole",
            vec![sized_payload(7, 4_096)],
            false,
        ),
    ];

    for (case, name, parts, multipart) in cases {
        bucket(&router, name).await;
        let key = "large.bin";
        let object: Vec<u8> = parts.concat();
        if multipart {
            completed_multipart_object(&router, name, key, &parts, &[]).await;
        } else {
            let stored = put_object(&router, name, key, &object, &[]).await;
            assert_eq!(
                stored.status,
                200,
                "case {case:?}: the write must succeed, got {} with body {:?}",
                stored.status,
                text(&stored)
            );
        }

        // STEP 1 — the discovery probe: one HEAD at the first part.
        let probe = head_object(&router, name, key, "?partNumber=1").await;
        assert_eq!(
            probe.status,
            206,
            "case {case:?}: a part-scoped HEAD must answer HTTP 206, got {}",
            probe.status
        );
        assert_eq!(
            probe.header("x-amz-mp-parts-count"),
            multipart.then(|| parts.len().to_string()),
            "case {case:?}: the reported part count must be the object's, got {:?}",
            probe.header("x-amz-mp-parts-count")
        );
        let (first, last, total) = parsed_content_range(&probe);
        assert_eq!(
            (first, last, total),
            (0, parts[0].len() - 1, object.len()),
            "case {case:?}: the probe must report the first part's span of the whole object"
        );
        assert_eq!(
            probe.header("content-length"),
            Some(parts[0].len().to_string()),
            "case {case:?}: the probe must report the part's length, got {:?}",
            probe.header("content-length")
        );
        assert_eq!(
            probe.header("accept-ranges"),
            Some(String::from("bytes")),
            "case {case:?}: a probe must be told byte ranges are served, got {:?}",
            probe.header("accept-ranges")
        );

        // STEP 2 — the fan-out, by part number: each part read on its own, reassembled.
        let mut by_part: Vec<u8> = Vec::new();
        let mut spans: Vec<(usize, usize)> = Vec::new();
        let reported_parts = if multipart { parts.len() } else { 1 };
        for part_number in 1..=reported_parts {
            let fetched = get_object(
                &router,
                name,
                key,
                &format!("?partNumber={part_number}"),
                &[],
            )
            .await;
            assert_eq!(
                fetched.status,
                206,
                "case {case:?}: part {part_number} must answer HTTP 206, got {} with body {:?}",
                fetched.status,
                text(&fetched)
            );
            let (first, last, span_total) = parsed_content_range(&fetched);
            assert_eq!(
                span_total,
                object.len(),
                "case {case:?}: part {part_number} must report the whole object's size"
            );
            spans.push((first, last));
            by_part.extend_from_slice(&fetched.body);
        }
        assert_eq!(
            by_part,
            object,
            "case {case:?}: the parts read by number must reassemble the object; got {} bytes \
             against {}",
            by_part.len(),
            object.len()
        );

        // STEP 3 — the same fan-out by BYTE RANGE, over the spans the parts reported: a
        // client that reads ranges instead of parts gets the same bytes.
        let mut by_range: Vec<u8> = Vec::new();
        for (first, last) in &spans {
            let fetched = get_object(
                &router,
                name,
                key,
                "",
                &[("range", &format!("bytes={first}-{last}"))],
            )
            .await;
            assert_eq!(
                fetched.status,
                206,
                "case {case:?}: the range {first}-{last} must answer HTTP 206, got {} with body \
                 {:?}",
                fetched.status,
                text(&fetched)
            );
            assert_eq!(
                parsed_content_range(&fetched),
                (*first, *last, object.len()),
                "case {case:?}: the range {first}-{last} must report its own span"
            );
            by_range.extend_from_slice(&fetched.body);
        }
        assert_eq!(
            by_range,
            object,
            "case {case:?}: the ranges read over the reported spans must reassemble the object; \
             got {} bytes against {}",
            by_range.len(),
            object.len()
        );

        // STEP 4 — the whole-object read a client falls back to reports no part count and no
        // range at all, so nothing about this object claims a partial answer.
        let whole = get_object(&router, name, key, "", &[]).await;
        assert_eq!(
            whole.status,
            200,
            "case {case:?}: a whole-object read must answer HTTP 200, got {}",
            whole.status
        );
        assert_eq!(
            whole.header("content-range"),
            None,
            "case {case:?}: a whole-object read reports no Content-Range, got {:?}",
            whole.header("content-range")
        );
        assert_eq!(
            whole.header("x-amz-mp-parts-count"),
            None,
            "case {case:?}: a whole-object read reports no part count, got {:?}",
            whole.header("x-amz-mp-parts-count")
        );
        assert_eq!(
            whole.body.as_ref(),
            object.as_slice(),
            "case {case:?}: a whole-object read must serve every byte, got {} of {}",
            whole.body.len(),
            object.len()
        );
    }
}
