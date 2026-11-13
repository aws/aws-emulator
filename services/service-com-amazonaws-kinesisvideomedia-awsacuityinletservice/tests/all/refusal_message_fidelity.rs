// SPDX-License-Identifier: CC0-1.0

//! The WORDING of `GetMedia`'s two state-level refusals, over the whole class of request
//! that earns each one.
//!
//! The rule under test, one level above any single recorded call:
//!
//! > Each of these two refusals of `GetMedia` states ONE fixed sentence, carrying nothing
//! > from the request and nothing from this service's state. An identifier the stream
//! > registry does not hold is HTTP 404 `ResourceNotFoundException` with the one sentence
//! > this family states for a stream that is not there to serve — however the stream came
//! > to stop resolving, and whichever identifier form addressed it. A
//! > `StartSelector$ContinuationToken` this service never issued is HTTP 400
//! > `InvalidArgumentException` with the one sentence the service states for a token it
//! > cannot decode — whatever text the token carries and whichever stream it is presented
//! > against.
//!
//! The rows span what the rule generalises over: for the not-found class, HOW an identifier
//! stops resolving (never created, created then deleted, deleted after being read, deleted
//! having held media) and both identifier forms; for the token class, the token's VALUE,
//! LENGTH and character classes across what the model admits, a token minted for another
//! stream, a token shaped like one of this service's own, and both identifier forms. The
//! calls the issue recorded are one row of each table.
//!
//! Every row also asserts that the identifier it sent does NOT appear in the answer: the
//! requirement is not merely different wording but an error channel that states no value of
//! the request or of the state.

use bytes::Bytes;
use service_com_amazonaws_kinesisvideo_kinesisvideo_20170930 as control_plane;

use crate::control_plane_stream_reads::{error_code, json, status, Harness};
use crate::get_media::split_payload;

/// The chunk every stream in this suite is seeded with, when it is seeded at all.
const MEDIA: &[&[u8]] = &[b"chunk-bytes-for-the-wording-suite"];

/// The `Message` member of an error answer, as the restJson1 error envelope carries it
/// (`ResourceNotFoundException$Message` / `InvalidArgumentException$Message`,
/// `smithy_query(query="shape", shape="InvalidArgumentException")`).
fn message(response: &http::Response<Bytes>) -> String {
    let body = json(response);
    body.get("Message")
        .or_else(|| body.get("message"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The sentence Kinesis Video Streams refuses a `ContinuationToken` it cannot DECODE with,
/// written out here rather than read off the handler's own constant: a test that re-asserted
/// that constant would follow any drift of it instead of pinning the wording.
///
/// The recorded refusal that shows it presented a token carrying an interior `-`, which is
/// the form the rows below mark `recorded_form`. Every row asserts the CLASS invariant — one
/// fixed sentence, the same for every token and every stream, carrying nothing from either —
/// and the rows of the recorded form additionally assert that the sentence is this one, which
/// together fix the wording for the whole class without claiming a second sentence no
/// recording shows.
const UNDECODABLE_TOKEN_MESSAGE: &str = "Illegal embedded sign character";

/// The modelled `ContinuationToken` bounds the token rows are built against:
/// `StartSelector$ContinuationToken` is `@length(min: 1, max: 128)` and
/// `@pattern("^[a-zA-Z0-9_\.\-]+$")`
/// (`smithy_query(query="shape", shape="StartSelector", model_path="services/service-com-amazonaws-kinesisvideomedia-awsacuityinletservice/smithy-model/com-amazonaws-kinesisvideomedia-awsacuityinletservice.json")`).
/// A token outside either constraint is refused by the boundary validator instead, which is
/// a different refusal and not this one.
const TOKEN_MIN_LENGTH: usize = 1;
const TOKEN_MAX_LENGTH: usize = 128;

/// RULE: an identifier the stream registry does not hold is refused 404
/// `ResourceNotFoundException` with the one fixed sentence, whatever made it stop
/// resolving and whichever identifier form carried it.
#[tokio::test]
async fn every_unresolvable_stream_is_refused_with_the_one_fixed_sentence() {
    let harness = Harness::new();

    // A live stream, so every refusal row below is about the registry and not about this
    // suite's addressing.
    let live_arn = harness.create_stream("wording-live").await;
    harness.produce(Some("wording-live"), None, MEDIA);

    // The streams that STOP resolving, each for a different reason: one deleted having held
    // media and been read successfully (the sequence the issue recorded), and one deleted
    // having held none at all. A delete needs the stream ACTIVE, which the control plane's
    // own clock owes it.
    let mut deleted_arns = Vec::new();
    for (name, produced) in [("wording-deleted-read", 1usize), ("wording-deleted-empty", 0)] {
        let arn = harness.create_stream(name).await;
        harness.produce(Some(name), None, &MEDIA[..produced]);
        if produced > 0 {
            let read = harness
                .get_media(
                    &serde_json::json!({ "StreamName": name }),
                    serde_json::json!({ "StartSelectorType": "EARLIEST" }),
                )
                .await;
            assert_eq!(
                status(&read),
                200,
                "setup {name:?}: the stream must read back before it is deleted, got {} ({})",
                status(&read),
                error_code(&read)
            );
        }
        harness.wait_until_active(name).await;
        let deleted = harness
            .call("/deleteStream", &serde_json::json!({ "StreamARN": arn }))
            .await;
        assert_eq!(
            status(&deleted),
            200,
            "setup {name:?}: DeleteStream must succeed, got {} ({})",
            status(&deleted),
            error_code(&deleted)
        );
        harness.expire_deletion(name);
        deleted_arns.push(arn);
    }

    let never_held_arn = live_arn.replace("wording-live", "wording-never-created");
    // (case, the addressing members, the identifier text the answer must not carry)
    let rows: Vec<(&str, serde_json::Value, String)> = vec![
        (
            "a name nothing ever created",
            serde_json::json!({ "StreamName": "wording-never-created" }),
            "wording-never-created".to_string(),
        ),
        (
            "a well-formed ARN the registry never held",
            serde_json::json!({ "StreamARN": never_held_arn.clone() }),
            never_held_arn,
        ),
        (
            "the name of a stream deleted after it was read",
            serde_json::json!({ "StreamName": "wording-deleted-read" }),
            "wording-deleted-read".to_string(),
        ),
        (
            "the ARN of a stream deleted after it was read",
            serde_json::json!({ "StreamARN": deleted_arns[0].clone() }),
            deleted_arns[0].clone(),
        ),
        (
            "the name of a deleted stream that held no media",
            serde_json::json!({ "StreamName": "wording-deleted-empty" }),
            "wording-deleted-empty".to_string(),
        ),
        (
            "the ARN of a deleted stream that held no media",
            serde_json::json!({ "StreamARN": deleted_arns[1].clone() }),
            deleted_arns[1].clone(),
        ),
    ];

    for (case, addressing, identifier) in &rows {
        let response = harness
            .get_media(
                addressing,
                serde_json::json!({ "StartSelectorType": "EARLIEST" }),
            )
            .await;
        assert_eq!(
            status(&response),
            404,
            "case {case:?}: expected HTTP 404, got {} ({})",
            status(&response),
            error_code(&response)
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {:?}",
            error_code(&response)
        );
        let answered = message(&response);
        assert_eq!(
            answered,
            control_plane::state::STREAM_NOT_FOUND_MESSAGE,
            "case {case:?}: expected the one fixed sentence {:?}, got {answered:?}",
            control_plane::state::STREAM_NOT_FOUND_MESSAGE
        );
        assert!(
            !answered.contains(identifier.as_str()),
            "case {case:?}: the refusal must carry no identifier from the request, but \
             {answered:?} contains {identifier:?}"
        );
    }

    // The positive control: the live stream still reads, so the rows above are refusals of
    // the registry's answer and not of every call this suite makes.
    let live = harness
        .get_media(
            &serde_json::json!({ "StreamName": "wording-live" }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    assert_eq!(
        status(&live),
        200,
        "case \"the live stream\": expected HTTP 200, got {} ({})",
        status(&live),
        error_code(&live)
    );
}

/// RULE: a `StartSelector$ContinuationToken` this service never issued is refused 400
/// `InvalidArgumentException` with the one fixed sentence, whatever the token's text and
/// whichever stream it is presented against.
#[tokio::test]
async fn every_unissued_continuation_token_is_refused_with_the_one_fixed_sentence() {
    let harness = Harness::new();

    let arn = harness.create_stream("token-stream").await;
    harness.produce(Some("token-stream"), None, MEDIA);
    let _other_arn = harness.create_stream("token-other-stream").await;
    harness.produce(Some("token-other-stream"), None, MEDIA);

    // A token this service really did mint, for the OTHER stream: a cursor names a position
    // in one stream's log, so presenting it against this stream is a token this stream was
    // never issued.
    let read_other = harness
        .get_media(
            &serde_json::json!({ "StreamName": "token-other-stream" }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    assert_eq!(
        status(&read_other),
        200,
        "setup \"the other stream\": expected HTTP 200, got {} ({})",
        status(&read_other),
        error_code(&read_other)
    );
    let other_token = split_payload("the other stream", read_other.body(), MEDIA)
        .first()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .unwrap_or_default();
    assert!(
        !other_token.is_empty(),
        "setup \"the other stream\": its chunk must carry a continuation token"
    );

    // (case, the token text, whether it carries the interior sign character the recorded
    // token did) — spanning the token VALUES the model admits: the shortest and the longest,
    // every character class of the pattern, a token minted for another stream, and one shaped
    // like a cursor of this service's own.
    let rows: Vec<(&str, String, bool)> = vec![
        (
            "a token of arbitrary text",
            "bogus-token-000".to_string(),
            true,
        ),
        (
            "a token at the modelled minimum length",
            "x".repeat(TOKEN_MIN_LENGTH),
            false,
        ),
        (
            "a token at the modelled maximum length",
            "z".repeat(TOKEN_MAX_LENGTH),
            false,
        ),
        (
            "a token using every character class the pattern admits",
            "Aa0_.-".to_string(),
            false,
        ),
        (
            "a token minted for another stream",
            other_token.clone(),
            other_token.trim_matches('-').contains('-'),
        ),
        (
            "a token shaped like one of this service's own cursors",
            "kvm-999-999".to_string(),
            true,
        ),
    ];

    // Both identifier forms address the one stream the token is presented against.
    let addressings: Vec<(&str, serde_json::Value, String)> = vec![
        (
            "by name",
            serde_json::json!({ "StreamName": "token-stream" }),
            "token-stream".to_string(),
        ),
        (
            "by ARN",
            serde_json::json!({ "StreamARN": arn.clone() }),
            arn.clone(),
        ),
    ];

    // Every row's sentence, so the class invariant — one message, independent of the token
    // and of the stream — is asserted over the whole table rather than row by row.
    let mut answered_sentences: Vec<(String, String)> = Vec::new();

    for (token_case, token, recorded_form) in &rows {
        for (addressing_case, addressing, identifier) in &addressings {
            let case = format!("{token_case} {addressing_case}");
            let response = harness
                .get_media(
                    addressing,
                    serde_json::json!({
                        "StartSelectorType": "CONTINUATION_TOKEN",
                        "ContinuationToken": token,
                    }),
                )
                .await;
            assert_eq!(
                status(&response),
                400,
                "case {case:?}: expected HTTP 400, got {} ({})",
                status(&response),
                error_code(&response)
            );
            assert_eq!(
                error_code(&response),
                "InvalidArgumentException",
                "case {case:?}: expected InvalidArgumentException, got {:?}",
                error_code(&response)
            );
            let answered = message(&response);
            if *recorded_form {
                assert_eq!(
                    answered, UNDECODABLE_TOKEN_MESSAGE,
                    "case {case:?}: expected the recorded sentence \
                     {UNDECODABLE_TOKEN_MESSAGE:?}, got {answered:?}"
                );
            }
            assert!(
                !answered.contains(token.as_str()) && !answered.contains(identifier.as_str()),
                "case {case:?}: the refusal must carry neither the token nor the stream, but \
                 {answered:?} carries one of {token:?} / {identifier:?}"
            );
            answered_sentences.push((case, answered));
        }
    }

    // The class invariant: the whole message is fixed text, so it does not vary with the
    // token or with the stream — every row above answered the very same sentence, which with
    // the recorded-form rows above fixes that sentence for the whole class.
    let (first_case, first_sentence) = answered_sentences
        .first()
        .cloned()
        .expect("the table has rows");
    for (case, answered) in &answered_sentences {
        assert_eq!(
            *answered, first_sentence,
            "case {case:?}: the refusal is one fixed sentence, so it must read exactly as \
             case {first_case:?}'s ({first_sentence:?}), got {answered:?}"
        );
    }

    // The positive control: a token THIS stream was issued still resumes it, so the rows
    // above refuse an unissued token rather than every token.
    let read = harness
        .get_media(
            &serde_json::json!({ "StreamName": "token-stream" }),
            serde_json::json!({ "StartSelectorType": "EARLIEST" }),
        )
        .await;
    let own_token = split_payload("this stream", read.body(), MEDIA)
        .first()
        .map(|chunk| chunk.tag("AWS_KINESISVIDEO_CONTINUATION_TOKEN"))
        .unwrap_or_default();
    assert_ne!(
        own_token, other_token,
        "setup \"this stream\": its cursor must differ from the other stream's"
    );
    let resumed = harness
        .get_media(
            &serde_json::json!({ "StreamName": "token-stream" }),
            serde_json::json!({
                "StartSelectorType": "CONTINUATION_TOKEN",
                "ContinuationToken": own_token,
            }),
        )
        .await;
    assert_eq!(
        status(&resumed),
        200,
        "case \"a token this stream was issued\": expected HTTP 200, got {} ({})",
        status(&resumed),
        error_code(&resumed)
    );
    assert!(
        resumed.body().is_empty(),
        "case \"a token this stream was issued\": it resumes after the only chunk, so the \
         payload must be empty, got {} byte(s)",
        resumed.body().len()
    );
}
