// SPDX-License-Identifier: CC0-1.0

//! What every turn of a conversation REPORTS about itself, beside the intent it
//! recognised: the two members a client's dialog loop reads off
//! `sessionState.intent`, and the id that ties the turns of one conversation
//! together.
//!
//! `recognize_text_nlu.rs` is about WHICH intent an utterance is interpreted as
//! and which slot the bot asks for next; this suite is about the shape of the
//! answer, over the same authored bot (its `pizza_bot` builds it through the Lex
//! Models V2 control plane, so there is one authoring sequence in this crate and
//! not two).
//!
//! The rules under test, each over a TABLE of turns rather than one recorded
//! conversation:
//!
//! 1. Every turn reports `sessionState.intent.slots` — "a map of all of the
//!    slots for the intent … if a slot has not been filled, the value is null"
//!    (`Intent$slots`) — as a map that is THERE, naming every slot the
//!    recognized intent declares with that `null` for each one the dialog has no
//!    value for, and `sessionState.intent.state`, whatever the
//!    turn made of the utterance: a matched intent, an intent still eliciting,
//!    and the fallback intent alike. Both encodings of a turn report them, and
//!    so does `GetSession`.
//! 2. A dialog the bot closes reports the state its own authoring earns:
//!    `Fulfilled` where the intent has a closing response to convey that it is
//!    fulfilled and no fulfilment code hook to hand it to, and
//!    `ReadyForFulfillment` where its author wrote none. The
//!    authored response is what the turn sends.
//! 3. `sessionState.originatingRequestId` — "a unique identifier for a specific
//!    request" — is minted when the session comes into existence, is the same on
//!    every later turn and on every read of that session, and is a DIFFERENT one
//!    for a conversation started on the same session id after `DeleteSession`.
//!    No two conversations share one.
//! 4. `sessionState.sessionAttributes` is reported as a map too — empty for a
//!    conversation nothing has put an attribute on.
//! 5. An utterance nothing of the locale satisfies reports the fallback intent
//!    on top, unscored, with the bot's OWN intents beside it at the scores they
//!    reached — all below the locale's `nluIntentConfidenceThreshold`, nought
//!    included — so a fallback the threshold caused is distinguishable from a
//!    bot that matches nothing at all.

use bytes::Bytes;
use serde_json::{json, Value};

use crate::harness::Emulator;
use crate::recognize_text_nlu::{
    messages, pizza_bot, say, ORDER_PIZZA, PIZZA_SIZE, SIZE_PROMPT, THRESHOLD,
};

/// The closing response authored on the intent of the bot that has one.
const CLOSING: &str = "Your pizza is on the way.";

/// The name of the intent Amazon Lex V2 reports when nothing of the locale
/// matches, which the control plane adds to every locale it creates.
const FALLBACK_INTENT: &str = "FallbackIntent";

/// `sessionState.intent` of one turn, as the response carries it.
fn intent(turn: &Value) -> &Value {
    turn.pointer("/sessionState/intent")
        .unwrap_or_else(|| panic!("a turn must report its intent, got {turn}"))
}

/// The slots map a turn reports, which must be present on every turn.
fn slots(case: &str, turn: &Value) -> serde_json::Map<String, Value> {
    intent(turn)
        .get("slots")
        .and_then(Value::as_object)
        .unwrap_or_else(|| {
            panic!("case {case:?}: intent.slots must be reported as a map, got {turn}")
        })
        .clone()
}

/// The state a turn reports, which must be present on every turn.
fn state(case: &str, turn: &Value) -> String {
    intent(turn)
        .get("state")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: intent.state must be reported, got {turn}"))
        .to_string()
}

/// The `originatingRequestId` of a session state, which must be present on every
/// state the service answers with.
fn origin(case: &str, state: &Value) -> String {
    let origin = state
        .get("originatingRequestId")
        .and_then(Value::as_str)
        .unwrap_or_else(|| {
            panic!("case {case:?}: sessionState.originatingRequestId must be reported, got {state}")
        })
        .to_string();
    assert!(
        !origin.is_empty(),
        "case {case:?}: originatingRequestId is a NonEmptyString, got {origin:?}"
    );
    origin
}

/// Is `id` the form Amazon Lex reports an `originatingRequestId` in: five
/// lower-case hexadecimal groups of 8, 4, 4, 4 and 12 digits?
fn is_request_id_form(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .map(|group| group.len())
            .eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()))
}

/// Rule 1, over a table of turns against one built bot: the three kinds of turn
/// the runtime answers all report the two members, through `RecognizeText`,
/// through `RecognizeUtterance` and through `GetSession`.
#[tokio::test]
async fn every_turn_reports_the_slots_map_and_the_intent_state() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "TurnShapeBot", None, true, Some(CLOSING)).await;

    // (case, utterance, the intent it is interpreted as, the state it reports,
    //  whether the slot the bot elicits is filled by it)
    //
    // The rows span what the rule generalises over: an utterance that names the
    // slot's value, one that matches the intent without naming it, one that
    // matches nothing of the locale, and the answer to the elicitation the
    // second one leaves open.
    let rows: [(&str, &str, &str, &str, bool); 4] = [
        (
            "an utterance naming the slot's value",
            "I want a large pizza",
            ORDER_PIZZA,
            "Fulfilled",
            true,
        ),
        (
            "an utterance that names no value for the required slot",
            "I want to order a pizza",
            ORDER_PIZZA,
            "InProgress",
            false,
        ),
        (
            "an utterance nothing of the locale matches",
            "what is the weather in reykjavik tomorrow afternoon",
            FALLBACK_INTENT,
            "ReadyForFulfillment",
            false,
        ),
        (
            "a single word that names no intent either",
            "hello",
            FALLBACK_INTENT,
            "ReadyForFulfillment",
            false,
        ),
    ];

    for (index, (case, text, expected_intent, expected_state, fills)) in rows.iter().enumerate() {
        let session_id = format!("shape-{index}");
        let turn = say(&emulator, &bot, &session_id, text).await;
        assert_eq!(
            intent(&turn).get("name").and_then(Value::as_str),
            Some(*expected_intent),
            "case {case:?}: expected the intent {expected_intent:?}, got {turn}"
        );
        assert_eq!(
            state(case, &turn),
            *expected_state,
            "case {case:?}: expected intent.state {expected_state:?}, got {turn}"
        );
        let filled = slots(case, &turn);
        // "A map of all of the slots for the intent … if a slot has not been
        // filled, the value is null" (`Intent$slots`): the map names every slot
        // the recognized intent declares — `PizzaSize` for the bot's own intent,
        // none at all for the fallback intent — and a slot the dialog has no
        // value for is reported as that `null` rather than left out, so a client
        // can tell a slot that is declared and open from one the intent does not
        // have.
        let declares_slot = *expected_intent == ORDER_PIZZA;
        assert_eq!(
            filled.contains_key(PIZZA_SIZE),
            declares_slot,
            "case {case:?}: the intent's own slot must{} be named, got {filled:?}",
            if declares_slot { "" } else { " not" }
        );
        assert_eq!(
            filled.len(),
            usize::from(declares_slot),
            "case {case:?}: the map names the intent's declared slots and nothing \
             else, got {filled:?}"
        );
        if declares_slot {
            let value = filled
                .get(PIZZA_SIZE)
                .unwrap_or_else(|| panic!("case {case:?}: the slot is named"));
            assert_eq!(
                value.is_null(),
                !*fills,
                "case {case:?}: a slot the turn filled carries a value and one it did \
                 not is null, got {value}"
            );
        }

        // The two members are session state: a read of the conversation reports
        // the same ones.
        let read_back = emulator.send("GET", &bot.session(&session_id), None).await;
        assert_eq!(
            read_back.status, 200,
            "case {case:?}: the turn's session must be readable, got {}",
            read_back.status
        );
        let stored = read_back.json();
        assert_eq!(
            stored.pointer("/sessionState/intent"),
            turn.pointer("/sessionState/intent"),
            "case {case:?}: GetSession must report the intent the turn left, got {stored}"
        );

        // And the other encoding of the same turn answers the same intent, in
        // its own gzipped, base64-encoded header.
        let by_utterance = Emulator::new();
        let other = pizza_bot(&by_utterance, "TurnShapeBot", None, true, Some(CLOSING)).await;
        let reply = by_utterance
            .send_raw(
                "POST",
                &format!("{}/utterance", other.session(&session_id)),
                Bytes::from((*text).to_string()),
                "text/plain; charset=utf-8",
                &[],
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: RecognizeUtterance must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let decoded = reply
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: no session-state header"));
        assert_eq!(
            decoded.get("intent"),
            turn.pointer("/sessionState/intent"),
            "case {case:?}: the decoded intent must be the one the text turn reports, got {decoded}"
        );
        // The authored messages of the turn travel as TEXT on their own header,
        // and the payload of a turn that asked for text carries no audio — not
        // even for a turn whose bot had something to say.
        assert_eq!(
            reply
                .lex_header("x-amz-lex-messages")
                .unwrap_or_else(|| json!([])),
            turn.get("messages").cloned().unwrap_or_else(|| json!([])),
            "case {case:?}: the decoded messages must be the ones the text turn reports"
        );
        assert!(
            reply.body.is_empty(),
            "case {case:?}: a text turn carries no audio stream, got {:?}",
            String::from_utf8_lossy(&reply.body)
        );
    }
}

/// Rule 2: the same conversation against two bots that differ ONLY in whether
/// the intent authors a closing response.
#[tokio::test]
async fn a_closed_dialog_reports_the_state_its_authoring_earns() {
    // (case, the authored closing response, the state a closed dialog reports,
    //  the messages the closing turn sends)
    let rows: [(&str, Option<&str>, &str, &[&str]); 2] = [
        (
            "an intent with a closing response",
            Some(CLOSING),
            "Fulfilled",
            &[CLOSING],
        ),
        (
            "an intent whose author wrote none",
            None,
            "ReadyForFulfillment",
            &[],
        ),
    ];

    for (index, (case, closing, expected_state, expected_messages)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        let bot = pizza_bot(&emulator, &format!("ClosingBot{index}"), None, true, *closing).await;
        let session_id = "closing";

        // Turn one leaves the required slot open, so the dialog is not closed yet
        // and the authored closing response is not what it sends.
        let first = say(&emulator, &bot, session_id, "I want to order a pizza").await;
        assert_eq!(
            state(case, &first),
            "InProgress",
            "case {case:?}: a turn still eliciting is in progress, got {first}"
        );
        assert_eq!(
            messages(&first),
            vec![SIZE_PROMPT.to_string()],
            "case {case:?}: the bot asks with the slot's own prompt, got {first}"
        );

        // Turn two fills it, which closes the dialog.
        let second = say(&emulator, &bot, session_id, "large").await;
        assert_eq!(
            second
                .pointer("/sessionState/dialogAction/type")
                .and_then(Value::as_str),
            Some("Close"),
            "case {case:?}: with nothing left to elicit the dialog closes, got {second}"
        );
        assert_eq!(
            state(case, &second),
            *expected_state,
            "case {case:?}: expected intent.state {expected_state:?}, got {second}"
        );
        assert_eq!(
            messages(&second),
            expected_messages
                .iter()
                .map(|text| (*text).to_string())
                .collect::<Vec<_>>(),
            "case {case:?}: expected the authored messages {expected_messages:?}, got {second}"
        );
        // The turn that closed the dialog is still scored against the locale's
        // threshold, so the two sides of the rule differ in nothing else.
        let score = second
            .pointer("/interpretations/0/nluConfidence/score")
            .and_then(Value::as_f64);
        assert!(
            score.is_none_or(|score| (THRESHOLD..=1.0).contains(&score)),
            "case {case:?}: a scored interpretation sits in {THRESHOLD}..=1.00, got {score:?}"
        );
    }
}

/// Rules 3 and 4: the id a conversation is stamped with, over several
/// conversations, across the turns of each, across a read, and across a delete.
#[tokio::test]
async fn one_conversation_carries_one_originating_request_id() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "OriginBot", None, true, Some(CLOSING)).await;

    // Session ids spanning the modelled `SessionId` (len 2..100, pattern
    // `^[0-9a-zA-Z._:-]+$`): the shortest it allows, an ordinary one, and the
    // longest, built from the characters its pattern admits.
    let sessions: [String; 3] = [
        "ab".to_string(),
        "origin-session".to_string(),
        format!("{}{}", "a.b_c:d-1".repeat(11), "0"),
    ];
    let mut minted: Vec<String> = Vec::new();

    for session_id in &sessions {
        let case = format!("the conversation {session_id:?}");
        let path = bot.session(session_id);

        let first = say(&emulator, &bot, session_id, "I want to order a pizza").await;
        let state = first
            .get("sessionState")
            .unwrap_or_else(|| panic!("case {case:?}: no sessionState in {first}"));
        let first_origin = origin(&case, state);
        assert!(
            is_request_id_form(&first_origin),
            "case {case:?}: the id the service mints is five lower-case hexadecimal groups of \
             8-4-4-4-12 digits, got {first_origin:?}"
        );
        assert_eq!(
            state.get("sessionAttributes"),
            Some(&json!({})),
            "case {case:?}: a conversation with no attributes reports the empty map, got {state}"
        );

        // Every later turn of the SAME conversation reports it...
        let second = say(&emulator, &bot, session_id, "large").await;
        assert_eq!(
            origin(
                &case,
                second
                    .get("sessionState")
                    .unwrap_or_else(|| panic!("case {case:?}: no sessionState in {second}"))
            ),
            first_origin,
            "case {case:?}: the next turn reports the same id, got {second}"
        );
        // ...and so does a read of it.
        let read_back = emulator.send("GET", &path, None).await;
        assert_eq!(read_back.status, 200, "case {case:?}: the session is readable");
        let stored = read_back.json();
        assert_eq!(
            origin(
                &case,
                stored
                    .get("sessionState")
                    .unwrap_or_else(|| panic!("case {case:?}: no sessionState in {stored}"))
            ),
            first_origin,
            "case {case:?}: GetSession reports the same id, got {stored}"
        );

        // A conversation started on the same session id after a delete is
        // another conversation, so it is stamped with another id — and the
        // deleted one is gone.
        let deleted = emulator.send("DELETE", &path, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {case:?}: DeleteSession must remove the session, got {}",
            deleted.status
        );
        let missing = emulator.send("GET", &path, None).await;
        assert_eq!(
            missing.status,
            404,
            "case {case:?}: the deleted session must be gone, got {} {:?}",
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        let restarted = say(&emulator, &bot, session_id, "I want to order a pizza").await;
        let fresh = origin(
            &case,
            restarted
                .get("sessionState")
                .unwrap_or_else(|| panic!("case {case:?}: no sessionState in {restarted}")),
        );
        assert_ne!(
            fresh, first_origin,
            "case {case:?}: a conversation started after the delete is stamped with another id"
        );

        minted.push(first_origin);
        minted.push(fresh);
    }

    // No two conversations share an id: it is what identifies one of them.
    let mut unique = minted.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(
        unique.len(),
        minted.len(),
        "every conversation must be stamped with its own id, got {minted:?}"
    );
}

/// Rule 5: what an utterance NOTHING satisfies reports beside the fallback
/// intent — the bot's own intents, at the scores they reached, which is what
/// tells a fallback the locale's threshold caused from a bot that matches
/// nothing at all; and what a turn that DID match reports beside its own
/// intent — the fallback intent Amazon Lex V2 inserts among the alternatives of
/// a response, unscored.
#[tokio::test]
async fn the_bots_own_intents_are_reported_as_scored_alternatives() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "AlternativeBot", None, true, Some(CLOSING)).await;

    // (case, utterance, the intent of the turn, whether the turn's own
    //  interpretation is scored)
    //
    // The rows span the classes the rule divides utterances into: one that
    // shares nothing with any sample utterance (which scores nought), one that
    // shares a word but not enough of them to reach the locale's 0.40 threshold,
    // and one that is a sample utterance.
    let rows: [(&str, &str, &str, bool); 3] = [
        (
            "an utterance sharing nothing with any sample utterance",
            "what is the weather in reykjavik tomorrow afternoon",
            FALLBACK_INTENT,
            false,
        ),
        (
            "an utterance sharing a word but not reaching the threshold",
            "pizza",
            FALLBACK_INTENT,
            false,
        ),
        (
            "a sample utterance of the bot's own intent",
            "I want a large pizza",
            ORDER_PIZZA,
            true,
        ),
    ];

    for (index, (case, text, expected_intent, scored)) in rows.iter().enumerate() {
        let turn = say(&emulator, &bot, &format!("alt-{index}"), text).await;
        let interpretations = turn
            .get("interpretations")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: no interpretations in {turn}"));
        assert_eq!(
            interpretations
                .first()
                .and_then(|top| top.pointer("/intent/name"))
                .and_then(Value::as_str),
            Some(*expected_intent),
            "case {case:?}: the top interpretation is the intent of the turn, got {turn}"
        );
        let top_score = interpretations
            .first()
            .and_then(|top| top.pointer("/nluConfidence/score"))
            .and_then(Value::as_f64);
        assert_eq!(
            top_score.is_some(),
            *scored,
            "case {case:?}: the fallback intent is not scored and a match is, got {turn}"
        );
        if let Some(score) = top_score {
            assert!(
                (THRESHOLD..=1.0).contains(&score),
                "case {case:?}: a match sits in {THRESHOLD}..=1.00, got {score}"
            );
        }

        let alternatives: Vec<(String, Option<f64>)> = interpretations
            .iter()
            .skip(1)
            .map(|other| {
                (
                    other
                        .pointer("/intent/name")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    other
                        .pointer("/nluConfidence/score")
                        .and_then(Value::as_f64),
                )
            })
            .collect();
        if *scored {
            // The locale declares one intent an utterance can match and the
            // turn matched it, so the one alternative left is the fallback
            // intent Amazon Lex V2 INSERTS among the alternatives — inserted
            // rather than scored, so it carries no confidence.
            assert_eq!(
                alternatives,
                vec![(FALLBACK_INTENT.to_string(), None)],
                "case {case:?}: the inserted fallback intent is the remaining alternative, \
                 got {alternatives:?}"
            );
        } else {
            // The turn is already ON the fallback intent, so it is reported as
            // the turn's own interpretation and the alternatives are the bot's
            // own intents, each at the score it reached.
            assert_eq!(
                alternatives
                    .iter()
                    .map(|(name, _)| name.as_str())
                    .collect::<Vec<_>>(),
                vec![ORDER_PIZZA],
                "case {case:?}: the bot's own intent must be reported beside the fallback, \
                 got {alternatives:?}"
            );
            for (name, score) in &alternatives {
                let score = score.unwrap_or_else(|| {
                    panic!("case {case:?}: a scored alternative carries its confidence: {name}")
                });
                assert!(
                    (0.0..THRESHOLD).contains(&score),
                    "case {case:?}: {name} did not reach the locale's {THRESHOLD} threshold, \
                     so its score must sit in 0.00..{THRESHOLD}, got {score}"
                );
            }
            // Every alternative carries an empty slots map, like every other
            // intent this service serializes.
            for other in interpretations.iter().skip(1) {
                assert_eq!(
                    other.pointer("/intent/slots"),
                    Some(&json!({})),
                    "case {case:?}: an alternative fills nothing, so its map is empty: {other}"
                );
            }
        }
    }
}

/// Rule 3 for the operation that seeds a conversation rather than advancing it:
/// `PutSession` reports the id too, the turn that follows it reports the same
/// one, and a caller that supplies one of its own is answered with that.
#[tokio::test]
async fn put_session_reports_the_id_the_next_turn_carries() {
    let emulator = Emulator::new();
    let bot = pizza_bot(&emulator, "SeedBot", None, true, Some(CLOSING)).await;

    // (case, the originatingRequestId the seed supplies, the id the conversation
    //  must then report)
    let rows: [(&str, Option<&str>); 2] = [
        ("a seed that supplies none", None),
        (
            "a seed that names the request its conversation originated from",
            Some("client-correlation-1"),
        ),
    ];

    for (index, (case, supplied)) in rows.iter().enumerate() {
        let session_id = format!("seeded-{index}");
        let path = bot.session(&session_id);
        let mut seed = json!({"sessionState": {"dialogAction": {"type": "Delegate"}}});
        if let Some(supplied) = supplied {
            seed["sessionState"]["originatingRequestId"] = json!(supplied);
        }
        let put = emulator.send("POST", &path, Some(seed)).await;
        assert_eq!(
            put.status,
            200,
            "case {case:?}: PutSession must succeed, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let seeded = put
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: no session-state header"));
        let seeded_origin = origin(case, &seeded);
        if let Some(supplied) = supplied {
            assert_eq!(
                seeded_origin, *supplied,
                "case {case:?}: the id the caller supplied is the one reported back"
            );
        }

        let turn = say(&emulator, &bot, &session_id, "I want a large pizza").await;
        assert_eq!(
            origin(
                case,
                turn.get("sessionState")
                    .unwrap_or_else(|| panic!("case {case:?}: no sessionState in {turn}"))
            ),
            seeded_origin,
            "case {case:?}: the turn after the seed reports the seed's own id, got {turn}"
        );
    }
}
