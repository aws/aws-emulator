// SPDX-License-Identifier: CC0-1.0

//! The members a 200 of this service reports WHATEVER the conversation holds for
//! them, and the dialog a seeded session is read back as.
//!
//! Amazon Lex V2 reports a dialog in full: a member whose value is empty or
//! whose stage has not been reached is reported with that empty value rather
//! than left out, so a client can read it without first testing whether it is
//! there. The rules this suite pins, each over a TABLE rather than one
//! conversation:
//!
//! 1. Every reported ACTIVE intent carries `slots` — "a map of all of the slots
//!    for the intent … if a slot has not been filled, the value is null"
//!    (`Intent$slots`) — and `confirmationState`, which is `None` "if the
//!    confirmation stage has not yet been reached"
//!    (`Intent$confirmationState`, whose enum is Confirmed | Denied | None).
//! 2. A reported session state carries `activeContexts`, whose modelled
//!    `ActiveContextsList` is `len 0..20`: the empty list for a conversation
//!    that has set no context.
//! 3. A turn carries `requestAttributes`, "the attributes sent in the request":
//!    the empty map for a conversation that accumulated none.
//! 4. The alternatives of a RECOGNITION's response carry the locale's fallback
//!    intent, which Amazon Lex V2 INSERTS among them rather than scores
//!    (`Interpretation$nluConfidence`: "determines the threshold where Amazon
//!    Lex V2 will insert the `AMAZON.FallbackIntent` … when returning
//!    alternative intents in a response"), so it carries no confidence, fills
//!    nothing, and is reported once — by the turn that answered the utterance,
//!    not by a later read of the session, which reports what the recognition
//!    itself made of it.
//! 5. A session a caller SEEDED is read back as the dialog completed against the
//!    built locale: the active intent's declared slot map, and the
//!    interpretation of that intent — `GetSessionResponse$interpretations` is
//!    `len 0..5`.
//!
//! Every bot here is the harness's own, authored through the Lex Models V2
//! control plane, so the intent, its slot and the locale's fallback intent are
//! the ones a client built rather than names this file made up.

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2 as lexmodels;

use crate::harness::{Emulator, Reply, INTENT, SLOT};

/// The text `requestContentType` an input stream carries ("Text format —
/// `text/plain; charset=utf-8`").
const TEXT_REQUEST_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// The name of the locale's fallback intent, read off the control plane that
/// authored the locale: the intent "Amazon Lex adds to every locale it creates",
/// based on the built-in `AMAZON.FallbackIntent`.
const FALLBACK: &str = lexmodels::state::FALLBACK_INTENT_NAME;

/// `Lex` itself interpreted the input, as opposed to `Bedrock`
/// (`Interpretation$interpretationSource`).
const LEX: &str = "Lex";

/// The modelled `Interpretations` list is `len 0..5`.
const MAX_INTERPRETATIONS: usize = 5;

/// A sample utterance the harness's bot authors on its intent.
const NAMES_THE_INTENT: &str = "I want to order a pizza";

/// Seed a session with `body` and answer the reply, which must be a 200.
async fn put(emulator: &Emulator, path: &str, case: &str, body: Value) -> Reply {
    let reply = emulator.send("POST", path, Some(body)).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: PutSession must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply
}

/// Read the session back, as `GetSession` reports it.
async fn get(emulator: &Emulator, path: &str, case: &str) -> Value {
    let reply = emulator.send("GET", path, None).await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: GetSession must find the session, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()
}

/// Take one turn through `RecognizeText`.
async fn say(emulator: &Emulator, path: &str, case: &str, text: &str) -> Value {
    let reply = emulator
        .send("POST", &format!("{path}/text"), Some(json!({"text": text})))
        .await;
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: RecognizeText must succeed, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    reply.json()
}

/// Take the same turn through `RecognizeUtterance`, whose dialog state travels
/// on its own gzipped, base64-encoded header.
async fn utter(emulator: &Emulator, path: &str, case: &str, text: &str) -> Value {
    let reply = emulator
        .send_raw(
            "POST",
            &format!("{path}/utterance"),
            Bytes::from(text.to_string()),
            TEXT_REQUEST_CONTENT_TYPE,
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
    reply
        .lex_header("x-amz-lex-session-state")
        .unwrap_or_else(|| panic!("case {case:?}: the session-state header is reported"))
}

/// The active intent of one reported session state.
fn active_intent(case: &str, carrier: &str, state: &Value) -> Value {
    state
        .get("intent")
        .cloned()
        .unwrap_or_else(|| panic!("case {case:?}: {carrier} must report the active intent: {state}"))
}

/// Rule 1: every carrier of a dialog reports the active intent's slot map and
/// its confirmation state, over the whole `ConfirmationState` enum plus the
/// dialog that recorded none.
#[tokio::test]
async fn every_reported_active_intent_carries_its_slots_and_confirmation_state() {
    // (case, the confirmationState the seed carries, the one every carrier must
    //  report). The rows span the modelled enum —
    //  `smithy_query(query="shape",shape="ConfirmationState")`: Confirmed,
    //  Denied, None — plus the seed that names none at all, which is the dialog
    //  whose confirmation stage has not been reached.
    let rows: [(&str, Option<&str>, &str); 4] = [
        ("a dialog that recorded no confirmation", None, "None"),
        ("a dialog that names the unreached stage itself", Some("None"), "None"),
        ("an intent the user confirmed", Some("Confirmed"), "Confirmed"),
        ("an intent the user denied", Some("Denied"), "Denied"),
    ];

    let (emulator, bot) = Emulator::with_bot().await;

    for (index, (case, seeded, expected)) in rows.iter().enumerate() {
        let mut intent = json!({"name": INTENT, "state": "InProgress"});
        if let Some(seeded) = seeded {
            intent["confirmationState"] = json!(seeded);
        }
        let seed = json!({
            "sessionState": {
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                "intent": intent,
            },
        });

        // Both encodings of a turn are driven, each on a session of its own, so
        // the row is asserted on every carrier of a dialog this service has.
        let by_text = bot.session_path(&format!("confirm-text-{index}"));
        let by_utterance = bot.session_path(&format!("confirm-utterance-{index}"));
        let seeded_header = put(&emulator, &by_text, case, seed.clone())
            .await
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: PutSession reports the session state"));
        put(&emulator, &by_utterance, case, seed).await;

        let read = get(&emulator, &by_text, case).await;
        let turn = say(&emulator, &by_text, case, NAMES_THE_INTENT).await;
        let spoken = utter(&emulator, &by_utterance, case, NAMES_THE_INTENT).await;

        // (carrier, the Intent it reported)
        let carriers: Vec<(&str, Value)> = vec![
            ("the PutSession session-state header", active_intent(case, "PutSession", &seeded_header)),
            (
                "the GetSession session state",
                active_intent(case, "GetSession", &read["sessionState"]),
            ),
            (
                "the GetSession interpretation of the seeded dialog",
                read.pointer("/interpretations/0/intent")
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: the seeded dialog is interpreted: {read}")),
            ),
            (
                "the RecognizeText session state",
                active_intent(case, "RecognizeText", &turn["sessionState"]),
            ),
            (
                "the RecognizeText interpretation of the turn",
                turn.pointer("/interpretations/0/intent")
                    .cloned()
                    .unwrap_or_else(|| panic!("case {case:?}: a turn reports its interpretation: {turn}")),
            ),
            (
                "the RecognizeUtterance session-state header",
                active_intent(case, "RecognizeUtterance", &spoken),
            ),
        ];

        for (carrier, reported) in carriers {
            assert_eq!(
                reported.get("confirmationState").and_then(Value::as_str),
                Some(*expected),
                "case {case:?}: {carrier} must report confirmationState {expected:?}, \
                 got {reported}"
            );
            // The map names every slot the bot declares for the intent, with the
            // documented null for the one no answer has filled.
            assert_eq!(
                reported.get("slots"),
                Some(&json!({SLOT: Value::Null})),
                "case {case:?}: {carrier} must name every declared slot, got {reported}"
            );
        }
    }
}

/// Rule 5: a seeded dialog is read back completed against the built locale — the
/// intent's declared slot map and the interpretation of that intent — over a
/// table of seeded slot maps, and a seed that names no intent at all.
#[tokio::test]
async fn a_seeded_dialog_is_read_back_completed_against_the_locale() {
    // (case, the intent the seed carries, the value the declared slot must then
    //  hold — None for the documented null)
    let filled = json!({"value": {"originalValue": "large", "interpretedValue": "large"}});
    let rows: [(&str, Option<Value>, Option<&str>); 5] = [
        (
            "a seed that carries no slots member",
            Some(json!({"name": INTENT, "state": "InProgress"})),
            None,
        ),
        (
            "a seed that carries the empty slot map",
            Some(json!({"name": INTENT, "state": "InProgress", "slots": {}})),
            None,
        ),
        (
            "a seed that filled the declared slot",
            Some(json!({"name": INTENT, "state": "InProgress", "slots": {SLOT: filled}})),
            Some("large"),
        ),
        (
            "a seed naming a slot the intent does not declare",
            Some(json!({
                "name": INTENT,
                "state": "InProgress",
                "slots": {"Crust": {"value": {"interpretedValue": "thin"}}},
            })),
            None,
        ),
        ("a seed that names no intent", None, None),
    ];

    let (emulator, bot) = Emulator::with_bot().await;

    for (index, (case, intent, value)) in rows.iter().enumerate() {
        let path = bot.session_path(&format!("seeded-{index}"));
        let mut state = json!({"dialogAction": {"type": "Delegate"}});
        if let Some(intent) = intent {
            state["intent"] = intent.clone();
        }
        put(&emulator, &path, case, json!({"sessionState": state})).await;

        let read = get(&emulator, &path, case).await;
        let interpretations = read
            .get("interpretations")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: GetSession reports a list: {read}"));

        let Some(_) = intent else {
            // Nothing is active, so there is nothing to interpret.
            assert!(
                interpretations.is_empty(),
                "case {case:?}: a session on no intent interprets nothing, got {read}"
            );
            assert_eq!(
                read.pointer("/sessionState/intent"),
                None,
                "case {case:?}: no intent was seeded, so none is reported, got {read}"
            );
            continue;
        };

        let reported = active_intent(case, "GetSession", &read["sessionState"]);
        let slots = reported
            .get("slots")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("case {case:?}: the intent names its slots: {reported}"));
        assert_eq!(
            slots.keys().map(String::as_str).collect::<Vec<_>>(),
            vec![SLOT],
            "case {case:?}: the map names every slot the intent declares and nothing else, \
             got {slots:?}"
        );
        assert_eq!(
            slots
                .get(SLOT)
                .and_then(|slot| slot.pointer("/value/interpretedValue"))
                .and_then(Value::as_str),
            *value,
            "case {case:?}: the declared slot holds what the seed filled, got {slots:?}"
        );
        assert_eq!(
            reported.get("confirmationState").and_then(Value::as_str),
            Some("None"),
            "case {case:?}: the seeded dialog reached no confirmation stage, got {reported}"
        );

        // The dialog the session is on is what it is interpreted as: one entry,
        // carrying that very intent. Nothing was said to the bot, so there is no
        // confidence and no interpreting service to report.
        assert_eq!(
            interpretations,
            &vec![json!({"intent": reported})],
            "case {case:?}: the seeded dialog is interpreted as its own intent, got {read}"
        );
        assert!(
            interpretations.len() <= MAX_INTERPRETATIONS,
            "case {case:?}: the modelled list holds at most {MAX_INTERPRETATIONS}, got {read}"
        );
    }
}

/// Rule 2: every reported session state carries its active contexts, empty list
/// included, over a table of how many the conversation set.
#[tokio::test]
async fn every_session_state_reports_its_active_contexts() {
    /// One context a caller may set, inside both published ends of
    /// `turnsToLive`'s `range 1..20` and of `timeToLiveInSeconds`.
    fn context(name: &str) -> Value {
        json!({
            "name": name,
            "timeToLive": {"timeToLiveInSeconds": 86400, "turnsToLive": 20},
            "contextAttributes": {"step": "payment"},
        })
    }

    // (case, the activeContexts the seed carries, how many must be reported)
    let rows: [(&str, Option<Value>, usize); 4] = [
        ("a seed carrying no activeContexts member", None, 0),
        ("a seed carrying the empty list", Some(json!([])), 0),
        ("one context", Some(json!([context("checkout")])), 1),
        (
            "two contexts",
            Some(json!([context("checkout"), context("upsell")])),
            2,
        ),
    ];

    let (emulator, bot) = Emulator::with_bot().await;

    for (index, (case, contexts, expected)) in rows.iter().enumerate() {
        let path = bot.session_path(&format!("context-{index}"));
        let mut state = json!({"dialogAction": {"type": "Delegate"}});
        if let Some(contexts) = contexts {
            state["activeContexts"] = contexts.clone();
        }
        let seeded = put(&emulator, &path, case, json!({"sessionState": state}))
            .await
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {case:?}: PutSession reports the session state"));
        let read = get(&emulator, &path, case).await;
        let turn = say(&emulator, &path, case, NAMES_THE_INTENT).await;

        for (carrier, state) in [
            ("the PutSession session-state header", seeded.clone()),
            ("the GetSession session state", read["sessionState"].clone()),
            ("the RecognizeText session state", turn["sessionState"].clone()),
        ] {
            assert_eq!(
                state.get("activeContexts").and_then(Value::as_array).map(Vec::len),
                Some(*expected),
                "case {case:?}: {carrier} must report {expected} active context(s) as a list, \
                 got {state}"
            );
        }
    }
}

/// Rule 3: a turn reports the request attributes the conversation accumulated,
/// the empty map included, over a table of what the turns carried.
#[tokio::test]
async fn a_turn_reports_the_request_attributes_it_accumulated() {
    // (case, the attributes each turn of the conversation sends in order, the
    //  map every turn must then report)
    let rows: [(&str, &[Value], Value); 4] = [
        ("a turn that sends none", &[], json!({})),
        (
            "a turn that sends one",
            &[json!({"channel": "web"})],
            json!({"channel": "web"}),
        ),
        (
            "a turn that sends several",
            &[json!({"channel": "web", "locale": "en", "tier": "gold"})],
            json!({"channel": "web", "locale": "en", "tier": "gold"}),
        ),
        (
            "a second turn that adds to the first",
            &[json!({"channel": "web"}), json!({"turn": "2"})],
            json!({"channel": "web", "turn": "2"}),
        ),
    ];

    let (emulator, bot) = Emulator::with_bot().await;

    for (index, (case, turns, expected)) in rows.iter().enumerate() {
        let path = bot.session_path(&format!("attributes-{index}"));
        // A conversation that sends nothing still takes its turn.
        let sent: Vec<Value> = if turns.is_empty() {
            vec![Value::Null]
        } else {
            turns.to_vec()
        };

        let mut last = Value::Null;
        for attributes in sent {
            let mut body = json!({"text": NAMES_THE_INTENT});
            if !attributes.is_null() {
                body["requestAttributes"] = attributes;
            }
            let reply = emulator
                .send("POST", &format!("{path}/text"), Some(body))
                .await;
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: RecognizeText must succeed, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            last = reply.json();
        }

        assert_eq!(
            last.get("requestAttributes"),
            Some(expected),
            "case {case:?}: the turn must report the attributes the session accumulated, \
             got {last}"
        );
    }
}

/// Rule 4: the locale's fallback intent is one of the alternatives every turn
/// reports — inserted unscored, filling nothing — and is reported once, over a
/// table of utterances spanning what a turn can land on.
#[tokio::test]
async fn the_locales_fallback_intent_is_an_inserted_alternative_of_every_turn() {
    // (case, the utterances of the conversation in order, the intent the last
    //  turn is on)
    let rows: [(&str, &[&str], &str); 4] = [
        (
            "an utterance that names the bot's own intent",
            &[NAMES_THE_INTENT],
            INTENT,
        ),
        (
            "an answer to the slot the previous turn elicited",
            &[NAMES_THE_INTENT, "large"],
            INTENT,
        ),
        (
            "an utterance nothing of the locale matches",
            &["what is the weather in reykjavik tomorrow"],
            FALLBACK,
        ),
        (
            "a single word the locale has nothing for",
            &["nonsense"],
            FALLBACK,
        ),
    ];

    let (emulator, bot) = Emulator::with_bot().await;

    for (index, (case, utterances, on)) in rows.iter().enumerate() {
        let path = bot.session_path(&format!("fallback-{index}"));
        let mut turn = Value::Null;
        for text in *utterances {
            turn = say(&emulator, &path, case, text).await;
        }
        let interpretations = turn
            .get("interpretations")
            .and_then(Value::as_array)
            .unwrap_or_else(|| panic!("case {case:?}: a turn reports its interpretations: {turn}"))
            .clone();
        assert!(
            interpretations.len() <= MAX_INTERPRETATIONS,
            "case {case:?}: the modelled list holds at most {MAX_INTERPRETATIONS}, got {turn}"
        );
        assert_eq!(
            interpretations
                .first()
                .and_then(|top| top.pointer("/intent/name"))
                .and_then(Value::as_str),
            Some(*on),
            "case {case:?}: the top interpretation is the intent of the turn, got {turn}"
        );

        let naming_fallback: Vec<&Value> = interpretations
            .iter()
            .filter(|entry| {
                entry.pointer("/intent/name").and_then(Value::as_str) == Some(FALLBACK)
            })
            .collect();
        assert_eq!(
            naming_fallback.len(),
            1,
            "case {case:?}: the fallback intent is reported exactly once, got {turn}"
        );

        if *on == FALLBACK {
            // The turn is already ON the fallback intent, so it is the turn's
            // own interpretation — carrying the dialog members an active intent
            // carries — and nothing is inserted beside it.
            assert_eq!(
                naming_fallback[0].pointer("/intent/confirmationState"),
                Some(&json!("None")),
                "case {case:?}: the intent of the turn carries its confirmation state, got {turn}"
            );
        } else {
            // The inserted entry: the locale's fallback intent, unscored, with
            // nothing filled for it, reported after every intent that earned a
            // score.
            assert_eq!(
                interpretations.last(),
                Some(&json!({
                    "intent": {"name": FALLBACK, "slots": {}},
                    "interpretationSource": LEX,
                })),
                "case {case:?}: the fallback intent is inserted among the alternatives, \
                 got {turn}"
            );
        }

        // The insertion belongs to the RESPONSE to the utterance: what the
        // session holds is what the recognition itself made of it, so a read
        // reports those and not the inserted entry.
        let read = get(&emulator, &path, case).await;
        let recognized: Vec<Value> = interpretations
            .iter()
            .filter(|entry| {
                *on == FALLBACK
                    || entry.pointer("/intent/name").and_then(Value::as_str) != Some(FALLBACK)
            })
            .cloned()
            .collect();
        assert_eq!(
            read.get("interpretations").and_then(Value::as_array),
            Some(&recognized),
            "case {case:?}: GetSession reports what the recognition made of the utterance, \
             got {read}"
        );
    }
}
