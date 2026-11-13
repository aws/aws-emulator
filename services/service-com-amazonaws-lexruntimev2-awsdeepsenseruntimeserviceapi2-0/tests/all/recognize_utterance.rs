// SPDX-License-Identifier: CC0-1.0

//! `RecognizeUtterance`: the second encoding of one conversation turn.
//!
//! The operation sends "user input to Amazon Lex V2" exactly as `RecognizeText`
//! does, but carries it differently on the wire — the utterance as the
//! `inputStream` `@httpPayload`, and everything structured on `x-amz-lex-*`
//! headers. The model states the encoding of those headers for every member that
//! uses one: "the following request fields must be compressed with gzip and then
//! base64 encoded before you send them to Amazon Lex V2 — `requestAttributes`,
//! `sessionState`" and "the following response fields are compressed using gzip
//! and then base64 encoded by Amazon Lex V2 — `inputTranscript`,
//! `interpretations`, `messages`, `requestAttributes`, `sessionState`"
//! (`smithy_query(query="operation", operation="RecognizeUtterance")` and the
//! per-member docs it points at).
//!
//! The rules under test, each over a TABLE of inputs rather than one request:
//!
//! 1. A text input stream advances the session EXACTLY as the same input through
//!    `RecognizeText` does: same dialog state, interpretations, messages,
//!    request attributes and session id.
//! 2. Every structured header payload is the member's JSON (or, for
//!    `inputTranscript`, its text) gzip-compressed and then base64 encoded — in
//!    both directions.
//! 3. What the turn persisted is what `GetSession` reports afterwards.
//! 4. Input this emulator cannot process is answered with the modelled error and
//!    changes nothing: speech with `DependencyFailedException` (424, no
//!    transcription or synthesis here), and a malformed request with
//!    `ValidationException` (400).

use bytes::Bytes;
use serde_json::{json, Value};
use service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2 as lexmodels;

use crate::harness::{gzip_base64, Emulator};

/// The intent Amazon Lex V2 inserts among the alternative intents of a
/// recognition's response, named as the control plane names the one it adds to
/// every locale it creates.
const FALLBACK: &str = lexmodels::state::FALLBACK_INTENT_NAME;

/// The text `requestContentType` the model lists ("Text format — `text/plain;
/// charset=utf-8`").
const TEXT_REQUEST_CONTENT_TYPE: &str = "text/plain; charset=utf-8";

/// The content type a text turn answers with when the caller asks for none.
const TEXT_CONTENT_TYPE: &str = "text/plain;charset=utf-8";

/// One row of the agreement table: what addresses the session, what is seeded
/// into it, and what is then said to it.
#[derive(Debug, Clone)]
struct Case {
    name: &'static str,
    bot_id: &'static str,
    bot_alias_id: &'static str,
    locale_id: &'static str,
    session_id: String,
    /// The `PutSession` body that seeds the session, or `None` to let the turn
    /// start the session itself.
    seed: Option<Value>,
    /// The utterance, as the bytes of the input stream.
    text: String,
    /// `sessionState`, carried as a header by `RecognizeUtterance` and in the
    /// body by `RecognizeText`.
    session_state: Option<Value>,
    /// `requestAttributes`, carried the same two ways.
    request_attributes: Option<Value>,
}

impl Case {
    fn session_path(&self) -> String {
        format!(
            "/bots/{}/botAliases/{}/botLocales/{}/sessions/{}",
            self.bot_id, self.bot_alias_id, self.locale_id, self.session_id
        )
    }
}

/// The agreement table. The rows differ along everything the rule generalises
/// over: whether the session already exists, whether the turn carries a dialog
/// state or request attributes, the size and character classes of the utterance
/// (including the 1-character and 1024-character ends of the sibling `Text`
/// member's `len 1..1024`), and the labels that address the session.
fn agreement_cases() -> Vec<Case> {
    vec![
        Case {
            name: "a bare first turn, no state and no attributes",
            bot_id: "ABCDEFGHIJ",
            bot_alias_id: "TSTALIASID",
            locale_id: "en_US",
            session_id: "ut".to_string(),
            seed: None,
            text: "hello".to_string(),
            session_state: None,
            request_attributes: None,
        },
        Case {
            name: "a turn that supplies the intent and attributes",
            bot_id: "0123456789",
            bot_alias_id: "LIVEALIAS1",
            locale_id: "en_GB",
            session_id: "user_1.turn:2-x".to_string(),
            seed: None,
            text: "I would like to order flowers".to_string(),
            session_state: Some(json!({
                "intent": {"name": "OrderFlowers", "state": "InProgress"},
                "dialogAction": {"type": "ElicitSlot", "slotToElicit": "FlowerType"},
                "sessionAttributes": {"cart": "2"},
            })),
            request_attributes: Some(json!({"channel": "web"})),
        },
        Case {
            name: "a turn in a session a PutSession already seeded",
            bot_id: "zzzzzzzzzz",
            bot_alias_id: "A",
            locale_id: "fr_FR",
            session_id: "seeded-session".to_string(),
            seed: Some(json!({
                "sessionState": {
                    "dialogAction": {"type": "Delegate"},
                    "sessionAttributes": {"étape": "2"},
                },
                "messages": [
                    {"content": "Quelle fleur ?", "contentType": "PlainText"},
                    {"content": "Dites-moi la couleur", "contentType": "PlainText"},
                ],
                "requestAttributes": {"canal": "téléphone"},
            })),
            text: "des roses — s'il vous plaît".to_string(),
            session_state: None,
            request_attributes: Some(json!({"tour": "2"})),
        },
        Case {
            name: "the shortest utterance",
            bot_id: "ABCDEFGHIJ",
            bot_alias_id: "TSTALIASID",
            locale_id: "en_US",
            session_id: "shortest".to_string(),
            seed: None,
            text: "x".to_string(),
            session_state: None,
            request_attributes: None,
        },
        Case {
            name: "the longest utterance the sibling Text member allows",
            bot_id: "ABCDEFGHIJ",
            bot_alias_id: "TSTALIASID",
            locale_id: "en_US",
            session_id: "longest".to_string(),
            seed: None,
            text: "a".repeat(1_024),
            session_state: None,
            request_attributes: None,
        },
    ]
}

/// Dispatch one `RecognizeUtterance` for `case`, with the structured members on
/// their headers under the documented encoding.
async fn recognize_utterance(emulator: &Emulator, case: &Case) -> crate::harness::Reply {
    let mut headers: Vec<(String, String)> = Vec::new();
    if let Some(state) = &case.session_state {
        headers.push((
            "x-amz-lex-session-state".to_string(),
            gzip_base64(&state.to_string()),
        ));
    }
    if let Some(attributes) = &case.request_attributes {
        headers.push((
            "x-amz-lex-request-attributes".to_string(),
            gzip_base64(&attributes.to_string()),
        ));
    }
    let borrowed: Vec<(&str, &str)> = headers
        .iter()
        .map(|(name, value)| (name.as_str(), value.as_str()))
        .collect();
    emulator
        .send_raw(
            "POST",
            &format!("{}/utterance", case.session_path()),
            Bytes::from(case.text.clone()),
            TEXT_REQUEST_CONTENT_TYPE,
            &borrowed,
        )
        .await
}

/// Dispatch the SAME turn through `RecognizeText`, which carries every member in
/// the JSON body.
async fn recognize_text(emulator: &Emulator, case: &Case) -> crate::harness::Reply {
    let mut body = json!({"text": case.text});
    if let Some(state) = &case.session_state {
        body["sessionState"] = state.clone();
    }
    if let Some(attributes) = &case.request_attributes {
        body["requestAttributes"] = attributes.clone();
    }
    emulator
        .send(
            "POST",
            &format!("{}/text", case.session_path()),
            Some(body),
        )
        .await
}

/// One session state with the member that identifies the CONVERSATION left
/// aside: `originatingRequestId` is minted per session, so two turns taken
/// against two emulators report two different ones and everything else is what
/// the two encodings must agree on.
fn correlation_aside(state: &Value) -> Value {
    let mut state = state.clone();
    if let Some(object) = state.as_object_mut() {
        object.remove("originatingRequestId");
    }
    state
}

/// Rules 1, 2 and 3: the two encodings of a turn agree, member by member, and
/// what either one persisted is what `GetSession` then reports.
#[tokio::test]
async fn an_utterance_advances_the_session_exactly_as_the_same_text_does() {
    for case in agreement_cases() {
        // Two emulators, one per encoding, so each turn is the FIRST turn of its
        // own session and the two are compared against the same starting point.
        let by_utterance = Emulator::new();
        let by_text = Emulator::new();
        for emulator in [&by_utterance, &by_text] {
            // A turn is interpreted against a built locale, so the labels this
            // row addresses have to name one.
            emulator.built_locale(case.bot_id, case.bot_alias_id, case.locale_id);
            if let Some(seed) = &case.seed {
                let put = emulator
                    .send("POST", &case.session_path(), Some(seed.clone()))
                    .await;
                assert_eq!(
                    put.status, 200,
                    "case {:?}: seeding the session must succeed, got {} {:?}",
                    case.name,
                    put.status,
                    String::from_utf8_lossy(&put.body)
                );
            }
        }

        let utterance = recognize_utterance(&by_utterance, &case).await;
        assert_eq!(
            utterance.status,
            200,
            "case {:?}: RecognizeUtterance must succeed, got {} {:?}",
            case.name,
            utterance.status,
            String::from_utf8_lossy(&utterance.body)
        );
        let text = recognize_text(&by_text, &case).await;
        assert_eq!(
            text.status,
            200,
            "case {:?}: RecognizeText must succeed, got {} {:?}",
            case.name,
            text.status,
            String::from_utf8_lossy(&text.body)
        );
        let expected = text.json();

        // Rule 1 + rule 2: each structured header decodes to exactly what the
        // JSON-bodied sibling reports for the same member.
        let actual_state = utterance.lex_header("x-amz-lex-session-state");
        assert_eq!(
            actual_state.as_ref().map(correlation_aside),
            expected.get("sessionState").map(correlation_aside),
            "case {:?}: the session state of the turn must agree, got {actual_state:?}",
            case.name
        );
        // The one member the two cannot share: each turn opened a session of its
        // own on its own emulator, and `originatingRequestId` identifies one
        // conversation, so both report one and the two differ.
        for (encoding, state) in [
            ("RecognizeUtterance", actual_state.clone()),
            ("RecognizeText", expected.get("sessionState").cloned()),
        ] {
            let minted = state
                .as_ref()
                .and_then(|state| state.pointer("/originatingRequestId").cloned())
                .and_then(|value| value.as_str().map(str::to_string));
            assert!(
                minted.as_deref().is_some_and(|id| !id.is_empty()),
                "case {:?}: {encoding} must report the session's own originatingRequestId, \
                 got {state:?}",
                case.name
            );
        }
        let actual_interpretations = utterance
            .lex_header("x-amz-lex-interpretations")
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            Some(&actual_interpretations),
            expected.get("interpretations"),
            "case {:?}: the interpretations must agree, got {actual_interpretations:?}",
            case.name
        );
        let actual_messages = utterance
            .lex_header("x-amz-lex-messages")
            .unwrap_or_else(|| json!([]));
        assert_eq!(
            Some(&actual_messages),
            expected.get("messages"),
            "case {:?}: the messages must agree, got {actual_messages:?}",
            case.name
        );
        let actual_attributes = utterance.lex_header("x-amz-lex-request-attributes");
        // The attributes the two encodings report must agree. An empty
        // collection has no header form here — this operation's collection
        // headers are optional `NonEmptyString`s — so the empty map its
        // JSON-bodied sibling reports is an absent header, and the header is
        // reported exactly when the map is not empty.
        let expected_attributes = expected
            .get("requestAttributes")
            .cloned()
            .unwrap_or_else(|| json!({}));
        assert_eq!(
            actual_attributes.clone().unwrap_or_else(|| json!({})),
            expected_attributes,
            "case {:?}: the request attributes must agree, got {actual_attributes:?}",
            case.name
        );
        assert_eq!(
            actual_attributes.is_none(),
            expected_attributes == json!({}),
            "case {:?}: the header is reported exactly when the map is not empty, \
             got {actual_attributes:?} against {expected_attributes}",
            case.name
        );
        assert_eq!(
            utterance.header("x-amz-lex-session-id"),
            expected
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string),
            "case {:?}: the turn names the session it advanced",
            case.name
        );

        // Rule 2 for the transcript, and the two members only this encoding has.
        assert_eq!(
            utterance.lex_header_text("x-amz-lex-input-transcript"),
            Some(case.text.clone()),
            "case {:?}: the transcript is the text that was processed",
            case.name
        );
        assert_eq!(
            utterance.header("x-amz-lex-input-mode").as_deref(),
            Some("Text"),
            "case {:?}: the input mode of a text stream is Text",
            case.name
        );
        assert_eq!(
            utterance.header("content-type").as_deref(),
            Some(TEXT_CONTENT_TYPE),
            "case {:?}: a text turn reports the default content type",
            case.name
        );
        // "The prompt or statement to send to the user": a turn that asked for
        // text is answered with no audio at all — the messages travel as text on
        // their own header, which the assertions above compare member by member,
        // and nothing synthesized speech for this turn. A recorded text turn
        // whose messages are NOT empty carries no audio bytes either.
        assert!(
            utterance.body.is_empty(),
            "case {:?}: a text turn carries no audio stream, got {:?}",
            case.name,
            String::from_utf8_lossy(&utterance.body)
        );

        // Rule 3: both encodings leave the same session behind.
        let read_back = by_utterance.send("GET", &case.session_path(), None).await;
        assert_eq!(
            read_back.status, 200,
            "case {:?}: the turn's session must be readable, got {}",
            case.name, read_back.status
        );
        let stored = read_back.json();
        assert_eq!(
            stored.get("sessionState").cloned(),
            actual_state,
            "case {:?}: GetSession must report what the turn persisted, got {stored:?}",
            case.name
        );
        // The SESSION holds what the recognition made of the utterance; the
        // response adds the fallback intent Amazon Lex V2 inserts among its
        // alternative intents, which is part of the answer and not of the
        // session.
        let reported: Vec<Value> = actual_interpretations
            .as_array()
            .cloned()
            .unwrap_or_default();
        let persisted: Vec<Value> = stored
            .get("interpretations")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(
            reported.get(..persisted.len()).map(<[Value]>::to_vec),
            Some(persisted.clone()),
            "case {:?}: the turn's interpretation must be persisted, got {persisted:?} \
             against {reported:?}",
            case.name
        );
        let added: Vec<Value> = reported.into_iter().skip(persisted.len()).collect();
        // The one entry a response adds is the inserted fallback intent — unless
        // the turn is already ON the fallback, which reports it as its own
        // interpretation and has nothing to insert beside it.
        let on_fallback = actual_state
            .as_ref()
            .and_then(|state| state.pointer("/intent/name"))
            .and_then(Value::as_str)
            == Some(FALLBACK);
        let expected_added: Vec<Value> = if on_fallback {
            Vec::new()
        } else {
            vec![json!({
                "intent": {"name": FALLBACK, "slots": {}},
                "interpretationSource": "Lex",
            })]
        };
        assert_eq!(
            added, expected_added,
            "case {:?}: the only entry the response adds is the inserted fallback intent, \
             got {added:?}",
            case.name
        );
        let after_text = by_text.send("GET", &case.session_path(), None).await;
        assert_eq!(
            stored.get("sessionState").map(correlation_aside),
            after_text
                .json()
                .get("sessionState")
                .map(correlation_aside),
            "case {:?}: both encodings leave the same session behind",
            case.name
        );
    }
}

/// Rule 2, on the REQUEST side: the dialog state and the request attributes a
/// turn carries are read out of their gzipped, base64-encoded headers, so what
/// they say reaches the session.
#[tokio::test]
async fn the_structured_request_headers_are_read_out_of_the_documented_encoding() {
    // (case, the sessionState header's JSON, the intent the session must hold)
    let rows: [(&str, Value, &str); 4] = [
        (
            "an intent the header names",
            json!({"intent": {"name": "OrderFlowers", "state": "InProgress"}}),
            "OrderFlowers",
        ),
        (
            "another intent, with a dialog action beside it",
            json!({
                "intent": {"name": "CheckBalance", "state": "ReadyForFulfillment"},
                "dialogAction": {"type": "Delegate"},
            }),
            "CheckBalance",
        ),
        (
            "a state that names no intent at all, so the fallback is interpreted",
            json!({"dialogAction": {"type": "Delegate"}}),
            "FallbackIntent",
        ),
        (
            "an intent carrying slots",
            json!({
                "intent": {
                    "name": "BookTrip",
                    "slots": {"City": {"value": {"interpretedValue": "Paris"}}},
                },
            }),
            "BookTrip",
        ),
    ];

    for (case, state, expected) in rows {
        let emulator = Emulator::new();
        emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
        let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/hdr";
        let reply = emulator
            .send_raw(
                "POST",
                &format!("{path}/utterance"),
                Bytes::from_static(b"an utterance"),
                TEXT_REQUEST_CONTENT_TYPE,
                &[(
                    "x-amz-lex-session-state",
                    gzip_base64(&state.to_string()).as_str(),
                )],
            )
            .await;
        assert_eq!(
            reply.status,
            200,
            "case {case:?}: the turn must succeed, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let actual = reply
            .lex_header("x-amz-lex-session-state")
            .and_then(|state| {
                state
                    .pointer("/intent/name")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        assert_eq!(
            actual.as_deref(),
            Some(expected),
            "case {case:?}: expected the session to hold intent {expected:?}, got {actual:?}"
        );
        // The same state must be what a later read reports: the header was
        // stored, not merely echoed.
        let stored = emulator.send("GET", path, None).await.json();
        assert_eq!(
            stored.pointer("/sessionState/intent/name").and_then(Value::as_str),
            Some(expected),
            "case {case:?}: GetSession must report the intent the header set, got {stored:?}"
        );
    }
}

/// One row of the refusal table: the case's name, the `requestContentType` it
/// declares, the `Response-Content-Type` it asks for, the bytes of its input
/// stream, the extra `x-amz-lex-*` headers it carries, and the HTTP status and
/// modelled error shape it must be answered with. Factored into a `type` so the
/// table's shape has a name of its own.
type Refusal = (
    &'static str,
    &'static str,
    Option<&'static str>,
    &'static [u8],
    Vec<(String, String)>,
    u16,
    &'static str,
);

/// An input stream one character past the most an utterance may carry: the
/// sibling `Text` member's `len 1..1024` is the bound a turn applies whichever
/// encoding carries it.
const OVER_LONG_UTTERANCE: &[u8; 1025] = &[b'a'; 1025];

/// Rule 4: a request this emulator cannot process is answered with the modelled
/// error — and leaves no session behind, so a following `GetSession` is still
/// the modelled 404.
#[tokio::test]
async fn input_that_cannot_be_processed_is_refused_and_changes_nothing() {
    // (case, requestContentType, Response-Content-Type, body, extra headers,
    //  expected status, expected error shape)
    //
    // The rows span the classes `requestContentType` divides input into (every
    // audio prefix the model lists, the text one, and a value outside all of
    // them), the speech values of `responseContentType`, an input stream that is
    // not the text it claims to be, one a character past the longest utterance a
    // turn interprets, and each structured header carrying something that is not
    // the documented encoding.
    let rows: [Refusal; 12] = [
        (
            "an input stream one character past the longest utterance",
            TEXT_REQUEST_CONTENT_TYPE,
            None,
            OVER_LONG_UTTERANCE,
            vec![],
            400,
            "ValidationException",
        ),
        (
            "PCM audio input",
            "audio/l16; rate=16000; channels=1",
            None,
            b"\x00\x01\x02\x03",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "the x-l16 spelling of PCM",
            "audio/x-l16; sample-rate=16000; channel-count=1",
            None,
            b"\x00\x01",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "lpcm audio input",
            "audio/lpcm; sample-rate=8000; sample-size-bits=16; channel-count=1; \
             is-big-endian=false",
            None,
            b"\x00\x01",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "Opus audio input",
            "audio/x-cbr-opus-with-preamble;preamble-size=0;bit-rate=256000;\
             frame-size-milliseconds=4",
            None,
            b"\x00\x01",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "a text turn asking for a spoken answer",
            TEXT_REQUEST_CONTENT_TYPE,
            Some("audio/mpeg"),
            b"hello",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "a text turn asking for pcm speech",
            TEXT_REQUEST_CONTENT_TYPE,
            Some("audio/pcm"),
            b"hello",
            vec![],
            424,
            "DependencyFailedException",
        ),
        (
            "a content type no prefix of the model matches",
            "application/json",
            None,
            b"hello",
            vec![],
            400,
            "ValidationException",
        ),
        (
            "an empty input stream",
            TEXT_REQUEST_CONTENT_TYPE,
            None,
            b"",
            vec![],
            400,
            "ValidationException",
        ),
        (
            "an input stream that is not the UTF-8 text it claims to be",
            TEXT_REQUEST_CONTENT_TYPE,
            None,
            &[0xff, 0xfe, 0x00],
            vec![],
            400,
            "ValidationException",
        ),
        (
            "a session-state header that is not the documented encoding",
            TEXT_REQUEST_CONTENT_TYPE,
            None,
            b"hello",
            vec![(
                "x-amz-lex-session-state".to_string(),
                "{\"intent\":{\"name\":\"X\"}}".to_string(),
            )],
            400,
            "ValidationException",
        ),
        (
            "a request-attributes header that decodes to something else",
            TEXT_REQUEST_CONTENT_TYPE,
            None,
            b"hello",
            vec![(
                "x-amz-lex-request-attributes".to_string(),
                gzip_base64("[\"not\",\"a\",\"map\"]"),
            )],
            400,
            "ValidationException",
        ),
    ];

    for (case, request_content_type, response_content_type, body, extra, status, shape) in rows {
        let emulator = Emulator::new();
        // The locale IS built, so what each row is refused for is the input it
        // carries and not the bot it addresses.
        emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
        let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/refused";
        let mut headers: Vec<(&str, &str)> = extra
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str()))
            .collect();
        if let Some(response_content_type) = response_content_type {
            headers.push(("Response-Content-Type", response_content_type));
        }
        let reply = emulator
            .send_raw(
                "POST",
                &format!("{path}/utterance"),
                Bytes::copy_from_slice(body),
                request_content_type,
                &headers,
            )
            .await;
        assert_eq!(
            reply.status,
            status,
            "case {case:?}: expected HTTP {status}, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        let actual_shape = reply.error_type();
        assert!(
            actual_shape.ends_with(shape),
            "case {case:?}: expected the modelled {shape}, got {actual_shape:?}"
        );

        let after = emulator.send("GET", path, None).await;
        assert_eq!(
            after.status, 404,
            "case {case:?}: a refused turn must start no session, got {} {:?}",
            after.status,
            String::from_utf8_lossy(&after.body)
        );
    }
}

/// Rule 4 for the one refusal that is about the attributes themselves: the
/// reserved namespace, checked on the header-borne map exactly as it is on the
/// body-borne one.
#[tokio::test]
async fn a_request_attribute_in_the_reserved_namespace_is_refused() {
    // The namespace is reserved whatever the case of its letters: "don't create
    // any request attributes with the prefix x-amz-lex:"
    // (`RecognizeUtteranceRequest$requestAttributes`).
    for (case, key) in [
        ("the documented spelling", "x-amz-lex:channel"),
        ("upper case", "X-Amz-Lex:Channel"),
    ] {
        let emulator = Emulator::new();
        emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
        let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/reserved";
        let attributes = json!({key: "web"});
        let reply = emulator
            .send_raw(
                "POST",
                &format!("{path}/utterance"),
                Bytes::from_static(b"hello"),
                TEXT_REQUEST_CONTENT_TYPE,
                &[(
                    "x-amz-lex-request-attributes",
                    gzip_base64(&attributes.to_string()).as_str(),
                )],
            )
            .await;
        assert_eq!(
            reply.status,
            400,
            "case {case:?}: a reserved key is the caller's to fix, got {} {:?}",
            reply.status,
            String::from_utf8_lossy(&reply.body)
        );
        assert!(
            reply.error_type().ends_with("ValidationException"),
            "case {case:?}: expected ValidationException, got {:?}",
            reply.error_type()
        );
    }
}

/// Rule 2, as bytes: the headers are not plain text, and what they carry only
/// comes back out through base64-then-gunzip.
#[tokio::test]
async fn the_response_headers_are_gzipped_before_they_are_base64_encoded() {
    let emulator = Emulator::new();
    emulator.built_locale("ABCDEFGHIJ", "TSTALIASID", "en_US");
    let path = "/bots/ABCDEFGHIJ/botAliases/TSTALIASID/botLocales/en_US/sessions/encoding";
    let put = emulator
        .send(
            "POST",
            path,
            Some(json!({
                "sessionState": {"dialogAction": {"type": "Delegate"}},
                "messages": [{"content": "What size?", "contentType": "PlainText"}],
            })),
        )
        .await;
    assert_eq!(put.status, 200, "seeding the session must succeed");

    let reply = emulator
        .send_raw(
            "POST",
            &format!("{path}/utterance"),
            Bytes::from_static(b"a large one"),
            TEXT_REQUEST_CONTENT_TYPE,
            &[],
        )
        .await;
    assert_eq!(reply.status, 200, "the turn must succeed");

    // `PutSession` answers with the messages it was given, and the turn answers
    // with the ones IT sends — none, here, since the locale this session is
    // addressed to has no message authored for the step the dialog reached. Both
    // responses carry every header they do have under the documented encoding.
    for (what, answer, headers) in [
        (
            "PutSession",
            &put,
            &["x-amz-lex-session-state", "x-amz-lex-messages"][..],
        ),
        (
            "the turn",
            &reply,
            &[
                "x-amz-lex-session-state",
                "x-amz-lex-interpretations",
                "x-amz-lex-input-transcript",
            ][..],
        ),
    ] {
        for header in headers {
            let raw = answer
                .header(header)
                .unwrap_or_else(|| panic!("{what}: {header} must be present"));
            assert!(
                serde_json::from_str::<Value>(&raw).is_err(),
                "{what}: {header} must not be plain JSON: {raw}"
            );
            let decoded = aws_smithy_types::base64::decode(&raw)
                .unwrap_or_else(|e| panic!("{what}: {header} must be base64 ({e}): {raw}"));
            // The gzip magic number: the base64 layer wraps a gzip MEMBER, which
            // is what a client is told to inflate.
            assert_eq!(
                decoded.first().copied().zip(decoded.get(1).copied()),
                Some((0x1f, 0x8b)),
                "{what}: {header} must carry a gzip stream, got {decoded:?}"
            );
        }
    }
    assert_eq!(
        put.lex_header("x-amz-lex-messages"),
        Some(json!([{"content": "What size?", "contentType": "PlainText"}])),
        "the decoded messages header is the messages PutSession was given"
    );
    assert_eq!(
        reply.header("x-amz-lex-messages"),
        None,
        "a turn reports the messages it sends, and this one sent none, so the header is \
         absent rather than carrying what was last sent before it"
    );
    assert_eq!(
        reply.lex_header_text("x-amz-lex-input-transcript"),
        Some("a large one".to_string()),
        "the decoded transcript header is the text that was processed"
    );
}
