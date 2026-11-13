// SPDX-License-Identifier: CC0-1.0

//! What this service's media store RETAINS, over more streams than its ceiling allows.
//!
//! The rule under test:
//!
//! > The media this store holds is bounded however many streams a producer addresses: a log
//! > is kept for at most `MAX_STORED_STREAM_LOGS` streams, and when a further stream is
//! > produced to, the log written longest ago is dropped together with the cursors that
//! > named it. A stream whose log has gone still RESOLVES — the stream is the control
//! > plane's record, which this store neither holds nor removes — so it reads as a stream
//! > nothing has been produced to (HTTP 200, empty payload), never as a missing resource,
//! > and a continuation token minted for it is refused as the bad argument a cursor that
//! > resolves to no position is.
//!
//! The rows span the positions a stream can be in relative to the ceiling: the newest logs
//! (kept whole), the logs written longest ago (dropped), and the boundary between them.
//! Every stream here exists throughout, so what the cases vary is only how recently each was
//! written.

use service_com_amazonaws_kinesisvideomedia_awsacuityinletservice as media;

use crate::control_plane_stream_reads::{error_code, status, Harness};

/// The chunk produced to every stream in this suite.
const MEDIA: &[&[u8]] = &[b"one-chunk-per-stream"];

/// How many streams past the ceiling this suite produces to, so there are dropped logs,
/// kept logs and a boundary between them.
const OVERSHOOT: usize = 4;

/// RULE: the number of retained media logs is bounded, the logs written longest ago are the
/// ones dropped, and a stream whose log has gone still reads — empty.
#[tokio::test]
async fn the_store_retains_a_bounded_number_of_stream_logs() {
    let harness = Harness::new();
    let ceiling = media::state::MAX_STORED_STREAM_LOGS;
    let total = ceiling + OVERSHOOT;

    // One stream per step, each produced to once, in order: the write order is what decides
    // which logs survive.
    let mut names = Vec::with_capacity(total);
    let mut arns = Vec::with_capacity(total);
    let mut tokens = Vec::with_capacity(total);
    for index in 0..total {
        let name = format!("ceiling-stream-{index:03}");
        let arn = harness.create_stream(&name).await;
        harness.produce(Some(&name), None, MEDIA);
        // The cursor this stream's own chunk hands back, so a dropped log's cursor can be
        // presented later.
        let read = harness
            .get_media(
                &serde_json::json!({ "StreamName": name }),
                serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            )
            .await;
        assert_eq!(
            status(&read),
            200,
            "setup {name:?}: its own read must succeed, got {} ({})",
            status(&read),
            error_code(&read)
        );
        tokens.push(
            crate::get_media::split_payload(&name, read.body(), MEDIA)
                .first()
                .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
                .unwrap_or_default(),
        );
        names.push(name);
        arns.push(arn);
    }

    // The store never holds more logs than the ceiling allows, whatever was produced.
    let held = {
        let state = media::state::service_state(&*harness.engine).expect("the media store");
        let store = state.lock();
        store.streams.len()
    };
    assert!(
        held <= ceiling,
        "expected at most {ceiling} retained logs after producing to {total} streams, got \
         {held}"
    );

    // (case, the index of the stream, whether its media must still be there)
    let rows: Vec<(String, usize, bool)> = vec![
        ("the first stream written".to_string(), 0, false),
        (
            "the last stream whose log was dropped".to_string(),
            OVERSHOOT - 1,
            false,
        ),
        (
            "the oldest stream whose log was kept".to_string(),
            OVERSHOOT,
            true,
        ),
        ("the most recent stream".to_string(), total - 1, true),
    ];

    for (case, index, media_kept) in &rows {
        let name = &names[*index];
        let response = harness
            .get_media(
                &serde_json::json!({ "StreamName": name }),
                serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            )
            .await;
        // A stream whose log has gone is still the control plane's record, so it reads.
        assert_eq!(
            status(&response),
            200,
            "case {case:?} ({name}): a registered stream must read whatever this store \
             retains, got {} ({})",
            status(&response),
            error_code(&response)
        );
        if *media_kept {
            assert!(
                !response.body().is_empty(),
                "case {case:?} ({name}): its log must still carry its chunk, got an empty \
                 payload"
            );
        } else {
            assert!(
                response.body().is_empty(),
                "case {case:?} ({name}): its log was dropped, so it must read empty, got {} \
                 byte(s)",
                response.body().len()
            );
            // And the cursor that named the dropped media resolves to no position.
            let resumed = harness
                .get_media(
                    &serde_json::json!({ "StreamName": name }),
                    serde_json::json!({
                        "StartSelectorType": "CONTINUATION_TOKEN",
                        "ContinuationToken": tokens[*index],
                    }),
                )
                .await;
            assert_eq!(
                status(&resumed),
                400,
                "case {case:?} ({name}): a cursor into dropped media must be refused, got {} \
                 ({})",
                status(&resumed),
                error_code(&resumed)
            );
            assert_eq!(
                error_code(&resumed),
                "InvalidArgumentException",
                "case {case:?} ({name}): expected InvalidArgumentException, got {:?}",
                error_code(&resumed)
            );
        }
        // Whichever side of the boundary it is on, the stream is addressable by its ARN too.
        let by_arn = harness
            .get_media(
                &serde_json::json!({ "StreamARN": arns[*index] }),
                serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            )
            .await;
        assert_eq!(
            status(&by_arn),
            200,
            "case {case:?} ({name}): addressed by ARN it must read alike, got {} ({})",
            status(&by_arn),
            error_code(&by_arn)
        );
    }
}
