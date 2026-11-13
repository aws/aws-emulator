// SPDX-License-Identifier: CC0-1.0

//! The Lex V2 runtime session lifecycle, end to end over `restJson1`.
//!
//! Every test drives the real router through [`crate::harness::Emulator`]: it
//! builds the service over one shared engine, registers it, and dispatches
//! buffered HTTP requests, so what is asserted is the wire behaviour (status,
//! headers, body) and not a handler called directly.
//!
//! The rules under test, each asserted over a TABLE of inputs rather than one
//! recorded request:
//!
//! 1. A session is addressed by its four `@httpLabel` values, and what
//!    `PutSession`/`RecognizeText` persist is what `GetSession` reports back,
//!    until `DeleteSession` removes it — after which the key is a 404.
//! 2. The structured `x-amz-lex-*` response headers carry their JSON compressed
//!    with gzip and then base64 encoded.
//! 3. An active context lives until the first of its two budgets runs out: its
//!    `timeToLiveInSeconds` or its `turnsToLive`, one turn per `RecognizeText`.
//! 4. A value outside a modelled constraint is refused with HTTP 400
//!    `ValidationException`.

use serde_json::{json, Value};

use crate::harness::{Emulator, INTENT, SLOT};

/// The content type a text session answers with when the caller asks for none.
const TEXT_CONTENT_TYPE: &str = "text/plain;charset=utf-8";

/// One row of the lifecycle table: a session, described by what addresses it and
/// what is put into it.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    bot_id: &'static str,
    bot_alias_id: &'static str,
    locale_id: &'static str,
    session_id: String,
    /// `sessionState.sessionAttributes`.
    attributes: Vec<(&'static str, &'static str)>,
    /// The `PlainText` contents of `messages`, in order.
    messages: Vec<&'static str>,
}

impl Case {
    fn session_path(&self) -> String {
        format!(
            "/bots/{}/botAliases/{}/botLocales/{}/sessions/{}",
            self.bot_id, self.bot_alias_id, self.locale_id, self.session_id
        )
    }

    fn attribute_map(&self) -> Value {
        Value::Object(
            self.attributes
                .iter()
                .map(|(k, v)| ((*k).to_string(), json!(v)))
                .collect(),
        )
    }

    fn message_list(&self) -> Value {
        Value::Array(
            self.messages
                .iter()
                .map(|content| json!({"content": content, "contentType": "PlainText"}))
                .collect(),
        )
    }
}

/// The lifecycle table. The rows differ along what rule 1 generalises over: how
/// many attributes and messages a session carries (including the `0` and `10`
/// ends of `Messages`' `len 0..10`), the form and length of the `sessionId`
/// (including both ends of `len 2..100` and every character class its
/// `^[0-9a-zA-Z._:-]+$` pattern allows), and the bot / alias / locale that
/// address it.
fn lifecycle_cases() -> Vec<Case> {
    vec![
        Case {
            name: "empty session, shortest id",
            bot_id: "ABCDEFGHIJ",
            bot_alias_id: "TSTALIASID",
            locale_id: "en_US",
            session_id: "ab".to_string(),
            attributes: vec![],
            messages: vec![],
        },
        Case {
            name: "one attribute, one message",
            bot_id: "ABCDEFGHIJ",
            bot_alias_id: "TSTALIASID",
            locale_id: "en_US",
            session_id: "sess-001".to_string(),
            attributes: vec![("cart", "2")],
            messages: vec!["Your order has been placed"],
        },
        Case {
            name: "punctuated id, several attributes, ten messages",
            bot_id: "0123456789",
            bot_alias_id: "LIVEALIAS1",
            locale_id: "en_GB",
            session_id: "user_1.turn:2-x".to_string(),
            attributes: vec![("cart", "2"), ("floor", ""), ("name", "Zoë")],
            messages: vec![
                "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten",
            ],
        },
        Case {
            name: "longest id, other locale",
            bot_id: "zzzzzzzzzz",
            bot_alias_id: "A",
            locale_id: "fr_FR",
            session_id: "c".repeat(100),
            attributes: vec![("étape", "2")],
            messages: vec!["Bonjour"],
        },
    ]
}

/// Rule 1 and rule 2, over the whole table: put a session, read it back, advance
/// it, read it back again, delete it, and find it gone.
#[tokio::test]
async fn a_session_is_created_read_advanced_and_deleted() {
    for case in lifecycle_cases() {
        let emulator = Emulator::new();
        // The bot the session hangs off: every call that needs the bot's runtime
        // model is served only against a built locale, so the labels this row
        // addresses have to name one.
        emulator.built_locale(case.bot_id, case.bot_alias_id, case.locale_id);
        let path = case.session_path();

        // --- PutSession ------------------------------------------------------
        let put = emulator
            .send(
                "POST",
                &path,
                Some(json!({
                    "sessionState": {
                        "dialogAction": {"type": "Delegate"},
                        "sessionAttributes": case.attribute_map(),
                    },
                    "messages": case.message_list(),
                    "requestAttributes": {"channel": "test"},
                })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {:?}: PutSession must succeed, got {} {:?}",
            case.name,
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        assert_eq!(
            put.header("x-amz-lex-session-id").as_deref(),
            Some(case.session_id.as_str()),
            "case {:?}: the session id header must name the stored session",
            case.name
        );
        assert_eq!(
            put.header("content-type").as_deref(),
            Some(TEXT_CONTENT_TYPE),
            "case {:?}: a text session reports the default content type",
            case.name
        );
        assert!(
            put.body.is_empty(),
            "case {:?}: a text session carries no audio stream, got {:?}",
            case.name,
            put.body
        );

        let put_state = put
            .lex_header("x-amz-lex-session-state")
            .unwrap_or_else(|| panic!("case {:?}: x-amz-lex-session-state is required", case.name));
        assert_eq!(
            put_state.get("sessionAttributes").cloned(),
            Some(case.attribute_map()),
            "case {:?}: the session-state header must carry the stored attributes, got {put_state:?}",
            case.name
        );
        assert_eq!(
            put_state.pointer("/dialogAction/type").and_then(Value::as_str),
            Some("Delegate"),
            "case {:?}: the stored dialog action must be the one supplied, got {put_state:?}",
            case.name
        );

        let put_messages = put.lex_header("x-amz-lex-messages");
        assert_eq!(
            put_messages.clone().map(|m| m.as_array().expect("a list").len()),
            (!case.messages.is_empty()).then_some(case.messages.len()),
            "case {:?}: the messages header must carry the stored messages, got {put_messages:?}",
            case.name
        );
        assert_eq!(
            put.lex_header("x-amz-lex-request-attributes"),
            Some(json!({"channel": "test"})),
            "case {:?}: the request-attributes header must carry the stored attributes",
            case.name
        );

        // --- GetSession ------------------------------------------------------
        let got = emulator.send("GET", &path, None).await;
        assert_eq!(
            got.status, 200,
            "case {:?}: GetSession must find the session just created, got {}",
            case.name, got.status
        );
        let body = got.json();
        assert_eq!(
            body.get("sessionId").and_then(Value::as_str),
            Some(case.session_id.as_str()),
            "case {:?}: GetSession reports the session's own id, got {body:?}",
            case.name
        );
        assert_eq!(
            body.pointer("/sessionState/sessionAttributes").cloned(),
            Some(case.attribute_map()),
            "case {:?}: GetSession must report what PutSession persisted, got {body:?}",
            case.name
        );
        assert_eq!(
            body.get("messages").cloned(),
            Some(case.message_list()),
            "case {:?}: GetSession must report the stored messages, got {body:?}",
            case.name
        );
        assert_eq!(
            body.get("interpretations").and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {:?}: nothing has been recognized yet, got {body:?}",
            case.name
        );

        // --- RecognizeText ---------------------------------------------------
        let recognized = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({
                    "text": "I would like a large pizza",
                    "requestAttributes": {"turn": "1"},
                })),
            )
            .await;
        assert_eq!(
            recognized.status, 200,
            "case {:?}: RecognizeText must succeed, got {} {:?}",
            case.name,
            recognized.status,
            String::from_utf8_lossy(&recognized.body)
        );
        let turn = recognized.json();
        assert_eq!(
            turn.pointer("/interpretations/0/intent/name")
                .and_then(Value::as_str),
            Some("FallbackIntent"),
            "case {:?}: with no intent in the session the fallback intent is interpreted, got {turn:?}",
            case.name
        );
        assert_eq!(
            turn.pointer("/interpretations/0/interpretationSource")
                .and_then(Value::as_str),
            Some("Lex"),
            "case {:?}: Lex itself interpreted the input, got {turn:?}",
            case.name
        );
        assert_eq!(
            turn.pointer("/sessionState/sessionAttributes").cloned(),
            Some(case.attribute_map()),
            "case {:?}: the session attributes survive the turn, got {turn:?}",
            case.name
        );
        assert_eq!(
            turn.pointer("/sessionState/dialogAction/type")
                .and_then(Value::as_str),
            Some("Close"),
            "case {:?}: the turn reports the step the bot decided on \u{2014} a supplied \
             `Delegate` asks Amazon Lex V2 to decide, and with nothing elicited the dialog \
             closes \u{2014} got {turn:?}",
            case.name
        );
        assert_eq!(
            turn.get("requestAttributes").cloned(),
            Some(json!({"channel": "test", "turn": "1"})),
            "case {:?}: request attributes merge into the session, got {turn:?}",
            case.name
        );
        assert_eq!(
            turn.get("sessionId").and_then(Value::as_str),
            Some(case.session_id.as_str()),
            "case {:?}: the turn names the session it advanced, got {turn:?}",
            case.name
        );

        // --- GetSession observes the turn ------------------------------------
        let after = emulator.send("GET", &path, None).await;
        assert_eq!(after.status, 200, "case {:?}: the session still exists", case.name);
        let after_body = after.json();
        assert_eq!(
            after_body.get("interpretations").cloned(),
            turn.get("interpretations").cloned(),
            "case {:?}: RecognizeText's interpretation must be persisted, got {after_body:?}",
            case.name
        );
        assert_eq!(
            after_body.pointer("/sessionState/intent/name").and_then(Value::as_str),
            Some("FallbackIntent"),
            "case {:?}: the active intent is persisted, got {after_body:?}",
            case.name
        );

        // --- DeleteSession ---------------------------------------------------
        let deleted = emulator.send("DELETE", &path, None).await;
        assert_eq!(
            deleted.status, 200,
            "case {:?}: DeleteSession must remove the session, got {}",
            case.name, deleted.status
        );
        assert_eq!(
            deleted.json(),
            json!({
                "botId": case.bot_id,
                "botAliasId": case.bot_alias_id,
                "localeId": case.locale_id,
                "sessionId": case.session_id,
            }),
            "case {:?}: DeleteSession echoes the four labels",
            case.name
        );

        // --- and it is gone --------------------------------------------------
        let missing = emulator.send("GET", &path, None).await;
        assert_eq!(
            missing.status, 404,
            "case {:?}: a deleted session must be a 404, got {} {:?}",
            case.name,
            missing.status,
            String::from_utf8_lossy(&missing.body)
        );
        assert_eq!(
            missing.error_type(),
            "ResourceNotFoundException",
            "case {:?}: the 404 names the modelled error",
            case.name
        );
        let delete_again = emulator.send("DELETE", &path, None).await;
        assert_eq!(
            delete_again.status, 404,
            "case {:?}: deleting what is not there removes nothing, got {}",
            case.name, delete_again.status
        );
        assert_eq!(
            delete_again.error_type(),
            "ResourceNotFoundException",
            "case {:?}: the second delete names the modelled error",
            case.name
        );
    }
}

/// Rule 1's key: all four labels identify the session, so the same `sessionId`
/// under a different bot, alias or locale is a different conversation.
#[tokio::test]
async fn sessions_under_different_labels_are_distinct() {
    let emulator = Emulator::new();
    let base = Case {
        name: "base",
        bot_id: "ABCDEFGHIJ",
        bot_alias_id: "TSTALIASID",
        locale_id: "en_US",
        session_id: "shared-id".to_string(),
        attributes: vec![],
        messages: vec![],
    };
    // Each row changes exactly one of the four labels.
    let variants = vec![
        ("same key", base.clone(), "0"),
        (
            "other bot",
            Case {
                bot_id: "JIHGFEDCBA",
                ..base.clone()
            },
            "1",
        ),
        (
            "other alias",
            Case {
                bot_alias_id: "OTHERALIAS",
                ..base.clone()
            },
            "2",
        ),
        (
            "other locale",
            Case {
                locale_id: "de_DE",
                ..base.clone()
            },
            "3",
        ),
        (
            "other session id",
            Case {
                session_id: "another-id".to_string(),
                ..base.clone()
            },
            "4",
        ),
    ];

    // Every label combination addresses a bot locale of its own: a session is
    // created only for labels that resolve to a built locale, so each row needs
    // the one it names.
    for (_, case, _) in &variants {
        emulator.built_locale(case.bot_id, case.bot_alias_id, case.locale_id);
    }

    for (name, case, marker) in &variants {
        let put = emulator
            .send(
                "POST",
                &case.session_path(),
                Some(json!({
                    "sessionState": {"sessionAttributes": {"marker": marker}},
                })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {name:?}: PutSession must succeed, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
    }

    for (name, case, marker) in &variants {
        let got = emulator.send("GET", &case.session_path(), None).await;
        assert_eq!(got.status, 200, "case {name:?}: the session must exist");
        assert_eq!(
            got.json().pointer("/sessionState/sessionAttributes/marker").and_then(Value::as_str),
            Some(*marker),
            "case {name:?}: each label combination is its own session",
        );
    }

    // Removing one leaves the others alone.
    let (_, first, _) = &variants[0];
    assert_eq!(
        emulator.send("DELETE", &first.session_path(), None).await.status,
        200
    );
    for (name, case, _) in variants.iter().skip(1) {
        assert_eq!(
            emulator.send("GET", &case.session_path(), None).await.status,
            200,
            "case {name:?}: deleting another session must not remove this one",
        );
    }
}

/// `RecognizeText` starts a session when none exists, and the intent a caller
/// puts in the session is the one interpreted — over a table of intent names.
#[tokio::test]
async fn recognize_text_starts_a_session_and_interprets_the_intent_in_it() {
    // Three rows: no intent in the session (the fallback), and two named
    // intents, one of them the fallback's own name.
    let rows: Vec<(&str, Option<&str>, &str)> = vec![
        ("no intent supplied", None, "FallbackIntent"),
        ("named intent", Some("OrderPizza"), "OrderPizza"),
        ("fallback named explicitly", Some("FallbackIntent"), "FallbackIntent"),
    ];

    for (index, (name, supplied, expected)) in rows.iter().enumerate() {
        let emulator = Emulator::new();
        emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
        let path = format!(
            "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/implicit-{index}"
        );
        let session_state = match supplied {
            Some(intent) => json!({"intent": {"name": intent}}),
            None => json!({}),
        };
        let turn = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({"text": "hello there", "sessionState": session_state})),
            )
            .await;
        assert_eq!(
            turn.status, 200,
            "case {name:?}: RecognizeText starts the session, got {} {:?}",
            turn.status,
            String::from_utf8_lossy(&turn.body)
        );
        let body = turn.json();
        assert_eq!(
            body.pointer("/interpretations/0/intent/name").and_then(Value::as_str),
            Some(*expected),
            "case {name:?}: the interpreted intent, got {body:?}",
        );
        assert_eq!(
            body.pointer("/sessionState/dialogAction/type").and_then(Value::as_str),
            Some("Close"),
            "case {name:?}: nothing was elicited, so the dialog closes, got {body:?}",
        );
        assert_eq!(
            body.get("messages").and_then(Value::as_array).map(Vec::len),
            Some(0),
            "case {name:?}: no message has been set for this session, got {body:?}",
        );

        // The implicitly started session is readable, and carries the turn.
        let got = emulator.send("GET", &path, None).await;
        assert_eq!(
            got.status, 200,
            "case {name:?}: the implicitly started session must be readable",
        );
        assert_eq!(
            got.json().pointer("/sessionState/intent/name").and_then(Value::as_str),
            Some(*expected),
            "case {name:?}: the active intent is persisted",
        );
    }
}

/// Session attributes accumulate across turns: a key a later turn omits keeps
/// the value the session holds, and one it repeats is overwritten.
#[tokio::test]
async fn session_attributes_accumulate_across_turns() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/attrs";
    assert_eq!(
        emulator
            .send(
                "POST",
                path,
                Some(json!({"sessionState": {"sessionAttributes": {"cart": "2", "tier": "gold"}}})),
            )
            .await
            .status,
        200
    );

    // Each row is one turn: what it supplies, and what the session holds after.
    let turns: Vec<(&str, Value, Value)> = vec![
        (
            "omitting every key keeps them all",
            json!({}),
            json!({"cart": "2", "tier": "gold"}),
        ),
        (
            "a repeated key is overwritten",
            json!({"cart": "3"}),
            json!({"cart": "3", "tier": "gold"}),
        ),
        (
            "a new key is added",
            json!({"coupon": "SAVE5"}),
            json!({"cart": "3", "tier": "gold", "coupon": "SAVE5"}),
        ),
        (
            "an empty value is a value",
            json!({"coupon": ""}),
            json!({"cart": "3", "tier": "gold", "coupon": ""}),
        ),
    ];

    for (name, supplied, expected) in turns {
        let turn = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({
                    "text": "and then?",
                    "sessionState": {"sessionAttributes": supplied},
                })),
            )
            .await;
        assert_eq!(turn.status, 200, "case {name:?}: the turn must succeed");
        assert_eq!(
            turn.json().pointer("/sessionState/sessionAttributes").cloned(),
            Some(expected.clone()),
            "case {name:?}: the session attributes after the turn",
        );
        let got = emulator.send("GET", path, None).await;
        assert_eq!(
            got.json().pointer("/sessionState/sessionAttributes").cloned(),
            Some(expected),
            "case {name:?}: and GetSession observes the same ones",
        );
    }
}

/// Rule 3: an active context is spent one turn per `RecognizeText` and is gone
/// once its `turnsToLive` runs out. The table spans both published ends of
/// `turnsToLive`'s `range 1..20`.
#[tokio::test]
async fn an_active_context_lives_for_its_turns_to_live() {
    // Every published bound of `turnsToLive`, plus two in between.
    for turns_to_live in [1_i64, 2, 3, 20] {
        let emulator = Emulator::new();
        emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
        let path = format!(
            "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/ctx-{turns_to_live}"
        );
        let put = emulator
            .send(
                "POST",
                &path,
                Some(json!({
                    "sessionState": {
                        "activeContexts": [{
                            "name": "checkout",
                            "timeToLive": {
                                "timeToLiveInSeconds": 86400,
                                "turnsToLive": turns_to_live,
                            },
                            "contextAttributes": {"step": "payment"},
                        }],
                    },
                })),
            )
            .await;
        assert_eq!(
            put.status, 200,
            "case {turns_to_live}: PutSession must accept the context, got {} {:?}",
            put.status,
            String::from_utf8_lossy(&put.body)
        );
        let stored = emulator.send("GET", &path, None).await.json();
        assert_eq!(
            stored
                .pointer("/sessionState/activeContexts/0/timeToLive/turnsToLive")
                .and_then(Value::as_i64),
            Some(turns_to_live),
            "case {turns_to_live}: the context starts on its full turn budget, got {stored:?}",
        );

        // One turn is spent per RecognizeText, so after `k` turns the context
        // has `turnsToLive - k` left and is reported while that is above zero.
        for turn in 1..=turns_to_live {
            let reply = emulator
                .send(
                    "POST",
                    &format!("{path}/text"),
                    Some(json!({"text": "next"})),
                )
                .await;
            assert_eq!(
                reply.status, 200,
                "case {turns_to_live}: turn {turn} must succeed"
            );
            let body = reply.json();
            let remaining = turns_to_live - turn;
            let reported = body
                .pointer("/sessionState/activeContexts/0/timeToLive/turnsToLive")
                .and_then(Value::as_i64);
            assert_eq!(
                reported,
                (remaining > 0).then_some(remaining),
                "case {turns_to_live}: after {turn} turn(s) the context must have \
                 {remaining} turn(s) left, got {body:?}",
            );
            // And the read agrees with the turn that wrote it.
            let got = emulator.send("GET", &path, None).await.json();
            assert_eq!(
                got.pointer("/sessionState/activeContexts/0/timeToLive/turnsToLive")
                    .and_then(Value::as_i64),
                reported,
                "case {turns_to_live}: GetSession must report the same context as turn {turn}",
            );
        }
    }
}

/// Rule 4: a modelled constraint refuses the request with HTTP 400
/// `ValidationException`, and the values just inside the same bounds are served.
#[tokio::test]
async fn values_outside_the_modelled_constraints_are_refused() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let ok_state = json!({"sessionState": {"dialogAction": {"type": "Delegate"}}});

    // (case, botId, sessionId, body, expected status) — each row sits on a
    // published bound of `BotIdentifier` (len 10..10, ^[0-9a-zA-Z]+$) or
    // `SessionId` (len 2..100, ^[0-9a-zA-Z._:-]+$), or on `sessionState`'s
    // @required, with the rows just inside the bound served.
    let rows: Vec<(&str, String, String, Value, u16)> = vec![
        (
            "both labels valid",
            "ABCDEFGHIJ".to_string(),
            "ab".to_string(),
            ok_state.clone(),
            200,
        ),
        (
            "longest session id",
            "ABCDEFGHIJ".to_string(),
            "s".repeat(100),
            ok_state.clone(),
            200,
        ),
        (
            "bot id one short",
            "ABCDEFGHI".to_string(),
            "ok-id".to_string(),
            ok_state.clone(),
            400,
        ),
        (
            "bot id one long",
            "ABCDEFGHIJK".to_string(),
            "ok-id".to_string(),
            ok_state.clone(),
            400,
        ),
        (
            "bot id outside the pattern",
            "ABCDEFGHI-".to_string(),
            "ok-id".to_string(),
            ok_state.clone(),
            400,
        ),
        (
            "session id one short",
            "ABCDEFGHIJ".to_string(),
            "a".to_string(),
            ok_state.clone(),
            400,
        ),
        (
            "session id one long",
            "ABCDEFGHIJ".to_string(),
            "s".repeat(101),
            ok_state.clone(),
            400,
        ),
        (
            "session id outside the pattern",
            "ABCDEFGHIJ".to_string(),
            "no spaces".to_string(),
            ok_state.clone(),
            400,
        ),
        (
            "no session state",
            "ABCDEFGHIJ".to_string(),
            "ok-id".to_string(),
            json!({}),
            400,
        ),
        (
            "context time to live below the range",
            "ABCDEFGHIJ".to_string(),
            "ok-id".to_string(),
            json!({"sessionState": {"activeContexts": [{
                "name": "c",
                "timeToLive": {"timeToLiveInSeconds": 4, "turnsToLive": 1},
                "contextAttributes": {},
            }]}}),
            400,
        ),
        (
            "context turns to live above the range",
            "ABCDEFGHIJ".to_string(),
            "ok-id".to_string(),
            json!({"sessionState": {"activeContexts": [{
                "name": "c",
                "timeToLive": {"timeToLiveInSeconds": 5, "turnsToLive": 21},
                "contextAttributes": {},
            }]}}),
            400,
        ),
        (
            "context on both lowest bounds",
            "ABCDEFGHIJ".to_string(),
            "ok-id".to_string(),
            json!({"sessionState": {"activeContexts": [{
                "name": "c",
                "timeToLive": {"timeToLiveInSeconds": 5, "turnsToLive": 1},
                "contextAttributes": {},
            }]}}),
            200,
        ),
    ];

    for (name, bot_id, session_id, body, expected) in rows {
        let path = format!(
            "/bots/{bot_id}/botAliases/TSTALIASID/botLocales/en_US/sessions/{}",
            session_id.replace(' ', "%20")
        );
        let reply = emulator.send("POST", &path, Some(body)).await;
        assert_eq!(
            reply.status, expected,
            "case {name:?}: expected HTTP {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        if expected == 400 {
            assert_eq!(
                reply.error_type(),
                "ValidationException",
                "case {name:?}: a refusal names ValidationException",
            );
        }
    }

    // The same bounds are enforced on the reading operations.
    for (name, method, suffix) in [
        ("GetSession", "GET", ""),
        ("DeleteSession", "DELETE", ""),
        ("RecognizeText", "POST", "/text"),
    ] {
        let reply = emulator
            .send(
                method,
                &format!("/bots/SHORT/botAliases/A/botLocales/en_US/sessions/x{suffix}"),
                (method == "POST").then(|| json!({"text": "hi"})),
            )
            .await;
        assert_eq!(
            reply.status, 400,
            "case {name:?}: a bad label is refused before the store is touched, got {}",
            reply.status
        );
        assert_eq!(
            reply.error_type(),
            "ValidationException",
            "case {name:?}: a refusal names ValidationException",
        );
    }
}

/// `RecognizeText`'s `text` sits on `len 1..1024`: both ends are served and a
/// value past either is refused.
#[tokio::test]
async fn the_text_of_a_turn_is_bounded_by_the_model() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/text-bounds";

    let rows: Vec<(&str, Value, u16)> = vec![
        ("shortest text", json!("a"), 200),
        ("longest text", json!("x".repeat(1024)), 200),
        ("empty text", json!(""), 400),
        ("one past the longest", json!("x".repeat(1025)), 400),
    ];

    for (name, text, expected) in rows {
        let reply = emulator
            .send("POST", &format!("{path}/text"), Some(json!({"text": text})))
            .await;
        assert_eq!(
            reply.status, expected,
            "case {name:?}: expected HTTP {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
    }

    // A turn with no text at all is a missing required member, not a 500.
    let reply = emulator
        .send("POST", &format!("{path}/text"), Some(json!({})))
        .await;
    assert_eq!(reply.status, 400, "a turn without text is refused");
    assert_eq!(reply.error_type(), "ValidationException");
}

/// A `responseContentType` asking for speech is answered with the modelled
/// dependency failure: this emulator has no speech synthesis, and an invented
/// audio stream would be worse than saying so. Every text form is served.
#[tokio::test]
async fn an_audio_response_content_type_is_refused() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/content-type";

    // The accepted values come from `RecognizeUtterance$responseContentType`:
    // "audio/mpeg audio/ogg audio/pcm" and "text/plain;charset=utf-8".
    let rows: Vec<(&str, Option<&str>, u16)> = vec![
        ("no content type asked for", None, 200),
        ("text", Some("text/plain;charset=utf-8"), 200),
        ("text, spelled with a space", Some("text/plain; charset=utf-8"), 200),
        ("mpeg audio", Some("audio/mpeg"), 424),
        ("ogg audio", Some("audio/ogg"), 424),
        ("pcm audio", Some("audio/pcm"), 424),
        ("audio, upper case", Some("AUDIO/PCM"), 424),
    ];

    for (name, content_type, expected) in rows {
        let body = json!({"sessionState": {"dialogAction": {"type": "Delegate"}}});
        // `responseContentType` is bound with @httpHeader(ResponseContentType),
        // so it travels as a header and not as a body member.
        let headers: Vec<(&str, &str)> = content_type
            .map(|value| vec![("ResponseContentType", value)])
            .unwrap_or_default();
        let reply = emulator
            .send_with_headers("POST", path, Some(body), &headers)
            .await;
        assert_eq!(
            reply.status, expected,
            "case {name:?}: expected HTTP {expected}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        match expected {
            200 => assert_eq!(
                reply.header("content-type").as_deref(),
                Some(content_type.unwrap_or(TEXT_CONTENT_TYPE)),
                "case {name:?}: the response reports the content type asked for",
            ),
            _ => assert_eq!(
                reply.error_type(),
                "DependencyFailedException",
                "case {name:?}: speech synthesis is the missing dependency",
            ),
        }
    }
}

/// The session store is bounded: an attribute map a request would grow past the
/// 12 KB Amazon Lex counts the attributes of a conversation against is refused
/// with `ValidationException`, and a refused turn changes nothing.
#[tokio::test]
async fn an_over_large_attribute_map_is_refused() {
    const MAX_ATTRIBUTE_BYTES: usize = 12 * 1024;
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/attr-bounds";

    // Rows on both sides of the cap, counting the key's bytes as well as the
    // value's.
    let rows: Vec<(&str, usize, u16)> = vec![
        ("well under the cap", 1024, 200),
        ("one byte under the cap", MAX_ATTRIBUTE_BYTES - 1, 200),
        ("exactly the cap", MAX_ATTRIBUTE_BYTES, 200),
        ("one byte past the cap", MAX_ATTRIBUTE_BYTES + 1, 400),
        ("far past the cap", MAX_ATTRIBUTE_BYTES * 4, 400),
    ];

    for (name, total_bytes, expected) in rows {
        let value = "v".repeat(total_bytes - "cart".len());
        let reply = emulator
            .send(
                "POST",
                path,
                Some(json!({"sessionState": {"sessionAttributes": {"cart": value}}})),
            )
            .await;
        assert_eq!(
            reply.status, expected,
            "case {name:?}: {total_bytes} bytes of attributes must answer HTTP {expected}, got {}",
            reply.status
        );
        if expected == 400 {
            assert_eq!(
                reply.error_type(),
                "ValidationException",
                "case {name:?}: an over-large map is the caller's to fix",
            );
        }
    }

    // The same bound holds over the accumulation across turns, and a refused
    // turn leaves the conversation exactly as it was.
    let half = "h".repeat(MAX_ATTRIBUTE_BYTES / 2);
    assert_eq!(
        emulator
            .send(
                "POST",
                path,
                Some(json!({"sessionState": {"sessionAttributes": {"a": half}}})),
            )
            .await
            .status,
        200,
        "case \"half the cap\": PutSession must succeed"
    );
    let refused = emulator
        .send(
            "POST",
            &format!("{path}/text"),
            Some(json!({
                "text": "more",
                "sessionState": {"sessionAttributes": {"b": "b".repeat(MAX_ATTRIBUTE_BYTES)}},
            })),
        )
        .await;
    assert_eq!(
        refused.status, 400,
        "case \"a turn that would pass the cap\": expected HTTP 400, got {}",
        refused.status
    );
    assert_eq!(refused.error_type(), "ValidationException");
    let after = emulator.send("GET", path, None).await.json();
    assert_eq!(
        after
            .pointer("/sessionState/sessionAttributes")
            .and_then(Value::as_object)
            .map(|map| map.keys().cloned().collect::<Vec<_>>()),
        Some(vec!["a".to_string()]),
        "case \"a turn that would pass the cap\": the refused turn must change nothing, got {after:?}",
    );
    let accepted = emulator
        .send(
            "POST",
            &format!("{path}/text"),
            Some(json!({"text": "more", "sessionState": {"sessionAttributes": {"b": "1"}}})),
        )
        .await;
    assert_eq!(
        accepted.status, 200,
        "case \"a turn that fits\": expected HTTP 200, got {}",
        accepted.status
    );
}

/// A refused turn leaves NO session behind: `RecognizeText` implicitly starts a
/// session, so a first turn that is refused must not leave an empty one that a
/// following `GetSession` would answer 200 for — rule 1 says a key with no live
/// session is a 404.
#[tokio::test]
async fn a_refused_first_turn_starts_no_session() {
    const MAX_ATTRIBUTE_BYTES: usize = 12 * 1024;
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");

    // One row per way a first turn can be refused after the session key is
    // known: an over-large attribute map, and a key in the reserved namespace.
    let rows: Vec<(&str, &str, Value)> = vec![
        (
            "an over-large session attribute map",
            "refused-too-large",
            json!({
                "text": "hi",
                "sessionState": {
                    "sessionAttributes": {"cart": "v".repeat(MAX_ATTRIBUTE_BYTES)},
                },
            }),
        ),
        (
            "a reserved request-attribute key",
            "refused-reserved",
            json!({"text": "hi", "requestAttributes": {"x-amz-lex:channel": "web"}}),
        ),
    ];

    for (name, session_id, body) in rows {
        let path =
            format!("/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/{session_id}");
        let refused = emulator
            .send("POST", &format!("{path}/text"), Some(body))
            .await;
        assert_eq!(
            refused.status, 400,
            "case {name:?}: expected HTTP 400, got {} {:?}",
            refused.status,
            String::from_utf8_lossy(&refused.body)
        );
        assert_eq!(
            refused.error_type(),
            "ValidationException",
            "case {name:?}: a refused turn is the caller's to fix",
        );

        let read = emulator.send("GET", &path, None).await;
        assert_eq!(
            read.status, 404,
            "case {name:?}: the refused turn must have started no session, got HTTP {} {:?}",
            read.status,
            String::from_utf8_lossy(&read.body)
        );
        assert_eq!(
            read.error_type(),
            "ResourceNotFoundException",
            "case {name:?}: an absent session is the modelled 404",
        );

        // And a well-formed turn for the same key still starts one.
        let accepted = emulator
            .send(
                "POST",
                &format!("{path}/text"),
                Some(json!({"text": "hi"})),
            )
            .await;
        assert_eq!(
            accepted.status, 200,
            "case {name:?}: a turn that is not refused starts the session, got HTTP {}",
            accepted.status
        );
        assert_eq!(
            emulator.send("GET", &path, None).await.status,
            200,
            "case {name:?}: the started session is readable",
        );
    }
}

/// `requestAttributes` may not name the namespace Amazon Lex V2 reserves: "the
/// namespace x-amz-lex: is reserved for special attributes. Don't create any
/// request attributes with the prefix x-amz-lex:". Keys outside it are served.
#[tokio::test]
async fn a_request_attribute_in_the_reserved_namespace_is_refused() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/reserved";

    // Rows around the reserved prefix: in it (any casing), and names that only
    // look like it.
    let rows: Vec<(&str, &str, u16)> = vec![
        ("an ordinary key", "channel", 200),
        ("the prefix itself", "x-amz-lex:channel", 400),
        ("the prefix, upper case", "X-Amz-Lex:Channel", 400),
        ("the prefix with nothing after it", "x-amz-lex:", 400),
        ("the prefix without its colon", "x-amz-lex-channel", 200),
        ("the prefix not at the start", "my-x-amz-lex:channel", 200),
    ];

    for (name, key, expected) in rows {
        // Both operations that take requestAttributes enforce it.
        for (operation, suffix, extra) in [
            ("PutSession", "", json!({"sessionState": {}})),
            ("RecognizeText", "/text", json!({"text": "hi"})),
        ] {
            let mut body = extra;
            body["requestAttributes"] = json!({key: "v"});
            let reply = emulator
                .send("POST", &format!("{path}{suffix}"), Some(body))
                .await;
            assert_eq!(
                reply.status, expected,
                "case {name:?} on {operation}: key {key:?} must answer HTTP {expected}, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
            if expected == 400 {
                assert_eq!(
                    reply.error_type(),
                    "ValidationException",
                    "case {name:?} on {operation}: a reserved key is the caller's to fix",
                );
            }
        }
    }
}

/// The `x-amz-lex-*` headers are encoded as the model documents, not plain: the
/// exact bytes of one known payload gunzip back to the JSON of the stored value.
#[tokio::test]
async fn the_lex_headers_are_gzipped_and_base64_encoded() {
    let (emulator, bot) = Emulator::with_bot().await;
    let path = bot.session_path("encoding");
    let put = emulator
        .send(
            "POST",
            &path,
            Some(json!({
                "sessionState": {
                    "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
                    "intent": {"name": INTENT, "state": "InProgress"},
                    "sessionAttributes": {"cart": "2"},
                },
                "messages": [{"content": "What size?", "contentType": "PlainText"}],
                "requestAttributes": {"channel": "web"},
            })),
        )
        .await;
    assert_eq!(put.status, 200, "PutSession must succeed");

    for header in [
        "x-amz-lex-session-state",
        "x-amz-lex-messages",
        "x-amz-lex-request-attributes",
    ] {
        let raw = put
            .header(header)
            .unwrap_or_else(|| panic!("{header} must be present"));
        assert!(
            serde_json::from_str::<Value>(&raw).is_err(),
            "{header} must not be plain JSON: {raw}"
        );
    }

    // The dialog state a caller reads back carries the session's own
    // `originatingRequestId` beside what the request set: the service mints one
    // when the session comes into existence and reports it on every state it
    // answers with, so it is asserted as a present, non-empty value and the rest
    // of the state is compared against what was stored.
    let decoded = put
        .lex_header("x-amz-lex-session-state")
        .expect("x-amz-lex-session-state is required");
    let originating_request_id = decoded
        .get("originatingRequestId")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !originating_request_id.is_empty(),
        "the session state must carry the originatingRequestId the service minted, got {decoded:?}"
    );
    assert_eq!(
        decoded,
        json!({
            "dialogAction": {"type": "ElicitSlot", "slotToElicit": SLOT},
            // The seeded dialog is completed against the bot's own locale: the
            // active intent names every slot that bot declares for it, with the
            // documented null for the one nothing has filled, and carries the
            // confirmation state of a dialog that has not reached its
            // confirmation stage.
            "intent": {
                "name": INTENT,
                "state": "InProgress",
                "slots": {SLOT: Value::Null},
                "confirmationState": "None",
            },
            "sessionAttributes": {"cart": "2"},
            // Reported on every state, empty for a conversation that has set no
            // context.
            "activeContexts": [],
            "originatingRequestId": originating_request_id,
        }),
        "the decoded session-state header is the stored state's JSON"
    );
    assert_eq!(
        put.lex_header("x-amz-lex-messages"),
        Some(json!([{"content": "What size?", "contentType": "PlainText"}])),
        "the decoded messages header is the stored messages' JSON"
    );
    assert_eq!(
        put.lex_header("x-amz-lex-request-attributes"),
        Some(json!({"channel": "web"})),
        "the decoded request-attributes header is the stored attributes' JSON"
    );
}
