// SPDX-License-Identifier: CC0-1.0

//! The bidirectional half of the Lex V2 runtime: `StartConversation`, driven over
//! the same router the emulator serves with and read back through `GetSession`.
//!
//! Each test states the RULE it checks and then checks it over a TABLE of inputs
//! spanning the class that rule covers: every member of the request event stream's
//! union rather than one, both values of `conversationMode` and the absent header,
//! conversations of one and of several events, and the turn kinds the bot can reach.
//!
//! The union members, the enums and the bounds asserted here are the model's own
//! (`smithy_query(query="operation", operation="StartConversation")`,
//! `smithy_query(query="shape", shape="StartConversationRequestEventStream,StartConversationResponseEventStream,ConversationMode,InputMode")`),
//! quoted beside each constant.

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::Bytes;
use serde_json::{json, Value};

use service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::ops::start_conversation::{
    CONFIGURATION_EVENT_FIRST, MAX_CONVERSATION_EVENTS, MAX_CONVERSATION_STREAM_BYTES,
    WELCOME_MESSAGES_NEED_A_DIALOG_ACTION,
};
use service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::state::bot_model::{
    bot_alias_not_valid_message, BOT_LOCALE_NOT_VALID_MESSAGE,
};

use crate::harness::{Bot, Emulator, Reply, CLOSING_MESSAGE, INTENT, SLOT, SLOT_PROMPT};

/// The media type an event-stream body carries, in both directions.
const EVENT_STREAM: &str = "application/vnd.amazon.eventstream";

/// The header the model binds `conversationMode` to.
const CONVERSATION_MODE_HEADER: &str = "x-amz-lex-conversation-mode";

/// The frame headers an SDK decoder reads.
const MESSAGE_TYPE: &str = ":message-type";
const EVENT_TYPE: &str = ":event-type";
const EXCEPTION_TYPE: &str = ":exception-type";

/// The `:event-type` of the frame an event-stream response opens with, before any
/// member of the `@streaming` union.
const INITIAL_RESPONSE: &str = "initial-response";

/// The members of `StartConversationRequestEventStream`.
const CONFIGURATION_EVENT: &str = "ConfigurationEvent";
const TEXT_INPUT_EVENT: &str = "TextInputEvent";
const DTMF_INPUT_EVENT: &str = "DTMFInputEvent";
const AUDIO_INPUT_EVENT: &str = "AudioInputEvent";
const PLAYBACK_COMPLETION_EVENT: &str = "PlaybackCompletionEvent";
const DISCONNECTION_EVENT: &str = "DisconnectionEvent";

/// The members of `StartConversationResponseEventStream` this service frames.
const INTENT_RESULT_EVENT: &str = "IntentResultEvent";
const TEXT_RESPONSE_EVENT: &str = "TextResponseEvent";
const RESOURCE_NOT_FOUND: &str = "ResourceNotFoundException";
const DEPENDENCY_FAILED: &str = "DependencyFailedException";

/// `ConversationMode`'s whole value set: AUDIO | TEXT.
const AUDIO_MODE: &str = "AUDIO";
const TEXT_MODE: &str = "TEXT";

/// The response content type that asks for text, "if the value is
/// text/plain;charset=utf-8, Amazon Lex V2 returns text in the response"
/// (`ConfigurationEvent$responseContentType`).
const TEXT_CONTENT_TYPE: &str = "text/plain;charset=utf-8";

/// The audio format `StartConversation` documents for its input: "audio input must
/// be in the following format: audio/lpcm sample-rate=8000 sample-size-bits=16
/// channel-count=1; is-big-endian=false".
const AUDIO_INPUT_CONTENT_TYPE: &str =
    "audio/lpcm; sample-rate=8000; sample-size-bits=16; channel-count=1; is-big-endian=false";

/// The error shape every refused request of this model carries at HTTP 400.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The utterance the fixture bot's intent declares as its sample, and an utterance
/// nothing of the locale matches.
const ORDERS_A_PIZZA: &str = "I want to order a pizza";
const MATCHES_NOTHING: &str = "what is the weather like";

/// The name of the intent Amazon Lex V2 reports when nothing of the locale matches,
/// which the control plane adds to every locale it creates.
const FALLBACK_INTENT: &str = "FallbackIntent";

/// One frame, in either direction.
#[derive(Debug, Clone)]
struct Frame {
    message_type: String,
    event_type: String,
    payload: Vec<u8>,
}

impl Frame {
    /// The frame's payload as JSON, which is how a restJson1 event travels.
    fn json(&self, case: &str) -> Value {
        serde_json::from_slice(&self.payload).unwrap_or_else(|error| {
            panic!(
                "case {case:?}: the {} frame carries a JSON payload ({error}), got {:?}",
                self.event_type,
                String::from_utf8_lossy(&self.payload)
            )
        })
    }
}

/// Frame one request event the way an SDK frames it: the union member's name in
/// `:event-type`, and the event structure as the JSON payload.
fn event_frame(event_type: &str, payload: &Value) -> Vec<u8> {
    raw_frame(
        "event",
        EVENT_TYPE,
        event_type,
        serde_json::to_vec(payload).expect("a serializable event"),
    )
}

/// One frame with the `:message-type`, the named header and the payload bytes given
/// — the writer the malformed-stream rows reach for.
fn raw_frame(
    message_type: &str,
    type_header: &'static str,
    event_type: &str,
    payload: Vec<u8>,
) -> Vec<u8> {
    let mut message = Message::new(payload).add_header(Header::new(
        MESSAGE_TYPE,
        HeaderValue::String(message_type.to_string().into()),
    ));
    if !event_type.is_empty() {
        message = message.add_header(Header::new(
            type_header,
            HeaderValue::String(event_type.to_string().into()),
        ));
    }
    let mut framed = Vec::new();
    write_message_to(&message, &mut framed).expect("an in-memory frame writer cannot fail");
    framed
}

/// The configuration event a conversation opens with: text responses, and whatever
/// else the row adds to it.
fn configuration(extra: Value) -> Vec<u8> {
    let mut event = json!({ "responseContentType": TEXT_CONTENT_TYPE });
    if let Some(members) = extra.as_object() {
        for (name, value) in members {
            event[name.as_str()] = value.clone();
        }
    }
    event_frame(CONFIGURATION_EVENT, &event)
}

/// A text input event carrying one utterance.
fn text_input(text: &str) -> Vec<u8> {
    event_frame(TEXT_INPUT_EVENT, &json!({ "text": text }))
}

/// A DTMF input event carrying one character, which `DTMFRegex` bounds to one of
/// `[A-D0-9#*]`.
fn dtmf_input(character: &str) -> Vec<u8> {
    event_frame(DTMF_INPUT_EVENT, &json!({ "inputCharacter": character }))
}

/// An audio input event. `audioChunk` is an ordinary blob member, so the chunk
/// travels base64-encoded inside the JSON payload; `QUFB` is the three bytes `AAA`.
fn audio_input() -> Vec<u8> {
    event_frame(
        AUDIO_INPUT_EVENT,
        &json!({ "audioChunk": "QUFB", "contentType": AUDIO_INPUT_CONTENT_TYPE }),
    )
}

/// Concatenate the frames of one request stream.
fn stream(frames: &[Vec<u8>]) -> Bytes {
    Bytes::from(frames.concat())
}

/// Decode a response body into its frames.
fn decode(case: &str, body: &[u8]) -> Vec<Frame> {
    let mut buffer = Bytes::copy_from_slice(body);
    let mut frames = Vec::new();
    while !buffer.is_empty() {
        let message = read_message_from(&mut buffer).unwrap_or_else(|error| {
            panic!("case {case:?}: the body must decode as event-stream frames: {error}")
        });
        let read = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .map(|header| match header.value() {
                    HeaderValue::String(value) => value.as_str().to_string(),
                    other => format!("{other:?}"),
                })
                .unwrap_or_default()
        };
        let message_type = read(MESSAGE_TYPE);
        let event_type = if message_type == "exception" {
            read(EXCEPTION_TYPE)
        } else {
            read(EVENT_TYPE)
        };
        frames.push(Frame {
            message_type,
            event_type,
            payload: message.payload().to_vec(),
        });
    }
    frames
}

/// The conversation URI of one session of `bot`.
fn conversation_path(bot: &Bot, session_id: &str) -> String {
    format!("{}/conversation", bot.session_path(session_id))
}

/// Dispatch one conversation: the stream as the body, under the event-stream media
/// type, with the `conversationMode` header when the row names one.
async fn converse(
    emulator: &Emulator,
    path: &str,
    mode: Option<&str>,
    body: Bytes,
) -> Reply {
    let headers: Vec<(&str, &str)> = match mode {
        Some(mode) => vec![(CONVERSATION_MODE_HEADER, mode)],
        None => Vec::new(),
    };
    emulator
        .send_raw("POST", path, body, EVENT_STREAM, &headers)
        .await
}

/// The frames of a conversation that was SERVED: a 200 whose body opens with the
/// initial-response frame, which is dropped here so a row's expectations are about
/// the conversation's own events.
fn served(case: &str, reply: &Reply) -> Vec<Frame> {
    assert_eq!(
        reply.status,
        200,
        "case {case:?}: a served conversation answers 200, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.header("content-type").as_deref(),
        Some(EVENT_STREAM),
        "case {case:?}: a served conversation answers an event stream, got {:?}",
        reply.header("content-type")
    );
    let frames = decode(case, &reply.body);
    let first = frames
        .first()
        .unwrap_or_else(|| panic!("case {case:?}: the response stream carries no frame at all"));
    assert_eq!(
        (first.message_type.as_str(), first.event_type.as_str()),
        ("event", INITIAL_RESPONSE),
        "case {case:?}: the stream must open with the initial-response frame, got {first:?}"
    );
    frames[1..].to_vec()
}

/// Assert a request that could not be READ: the modelled `ValidationException` at HTTP
/// 400, with `expected` as its message when the row names one. This is the channel for a
/// request whose stream never opened; what the events of an opened stream say is answered
/// in band ([`in_band_validation`]).
fn refused(case: &str, reply: &Reply, expected: Option<&str>) {
    assert_eq!(
        reply.status,
        400,
        "case {case:?}: a request that cannot be read answers 400, got {} {:?}",
        reply.status,
        String::from_utf8_lossy(&reply.body)
    );
    assert_eq!(
        reply.error_type(),
        VALIDATION_EXCEPTION,
        "case {case:?}: a refused request names {VALIDATION_EXCEPTION}, got {:?}",
        reply.error_type()
    );
    let message = reply.error_message();
    assert!(
        !message.is_empty(),
        "case {case:?}: a refusal states why, got an empty message"
    );
    if let Some(expected) = expected {
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}

/// Assert a conversation whose stream opened and whose EVENTS were refused: a 200 whose
/// stream ends on the in-band `ValidationException` member, with `expected` as its
/// message when the row names one.
fn in_band_validation(case: &str, reply: &Reply, expected: Option<&str>) {
    let frames = served(case, reply);
    let (member, message) = in_band(case, &frames);
    assert_eq!(
        member, VALIDATION_EXCEPTION,
        "case {case:?}: expected the in-band {VALIDATION_EXCEPTION}, got {member}"
    );
    assert!(
        !message.is_empty(),
        "case {case:?}: an in-band refusal states why, got an empty message"
    );
    if let Some(expected) = expected {
        assert_eq!(
            message, expected,
            "case {case:?}: expected the refusal {expected:?}, got {message:?}"
        );
    }
}

/// The one in-band exception frame a conversation ended on: its member name and the
/// message it carries.
fn in_band(case: &str, frames: &[Frame]) -> (String, String) {
    let frame = frames
        .last()
        .unwrap_or_else(|| panic!("case {case:?}: the stream carries no frame to end on"));
    assert_eq!(
        frame.message_type, "exception",
        "case {case:?}: an in-band failure travels as an exception frame, got {frame:?}"
    );
    let payload = frame.json(case);
    let message = payload
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or_else(|| panic!("case {case:?}: an in-band exception states why, got {payload}"))
        .to_string();
    (frame.event_type.clone(), message)
}

/// The contents of the messages a `TextResponseEvent` carries.
fn message_contents(case: &str, frame: &Frame) -> Vec<String> {
    let payload = frame.json(case);
    payload
        .get("messages")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("case {case:?}: a {TEXT_RESPONSE_EVENT} carries messages: {payload}"))
        .iter()
        .map(|message| {
            message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string()
        })
        .collect()
}

/// RULE: every input event of a conversation that carries input is one TURN, the
/// same turn the two `Recognize*` operations take, reported as an
/// `IntentResultEvent` for the state the conversation reached and a
/// `TextResponseEvent` for whatever the bot has to say at that step — and no
/// `TextResponseEvent` where the bot authors nothing to say.
#[tokio::test]
async fn every_input_event_of_a_conversation_is_one_turn() {
    let (emulator, bot) = Emulator::with_bot().await;

    // (case, conversationMode, input events after the configuration, the turns owed
    //  as (inputMode, dialogAction type, slotToElicit, intent state, messages))
    //
    // The rows span the input members a conversation can carry (text, DTMF and the
    // playback notification that carries none), the number of turns one stream takes
    // (none, one, two), whether the utterance matched an intent of the locale or fell
    // back, and whether the step the dialog reached has an authored message.
    type Turn<'t> = (&'t str, &'t str, Option<&'t str>, &'t str, Vec<&'t str>);
    /// One row: the case, the mode, the input events after the configuration, and
    /// the turns owed.
    type Row<'r> = (&'r str, Option<&'r str>, Vec<Vec<u8>>, Vec<Turn<'r>>);
    let rows: Vec<Row> = vec![
        (
            "a configuration and nothing else takes no turn",
            Some(TEXT_MODE),
            vec![],
            vec![],
        ),
        (
            "one utterance that names an intent of the locale",
            Some(TEXT_MODE),
            vec![text_input(ORDERS_A_PIZZA)],
            vec![("Text", "ElicitSlot", Some(SLOT), "InProgress", vec![SLOT_PROMPT])],
        ),
        (
            "an utterance nothing of the locale matches, which the bot says nothing for",
            Some(TEXT_MODE),
            vec![text_input(MATCHES_NOTHING)],
            vec![("Text", "Close", None, "ReadyForFulfillment", vec![])],
        ),
        (
            "two utterances: the intent, then the answer that fills its slot",
            Some(TEXT_MODE),
            vec![text_input(ORDERS_A_PIZZA), text_input("large")],
            vec![
                ("Text", "ElicitSlot", Some(SLOT), "InProgress", vec![SLOT_PROMPT]),
                ("Text", "Close", None, "Fulfilled", vec![CLOSING_MESSAGE]),
            ],
        ),
        (
            "a playback notification, which carries no input and advances nothing",
            Some(TEXT_MODE),
            vec![event_frame(PLAYBACK_COMPLETION_EVENT, &json!({}))],
            vec![],
        ),
        (
            // "If the conversation mode is AUDIO you can send both audio and DTMF
            // information": the characters of one entry accumulate and the end
            // character is what makes them one turn, so three events are ONE turn.
            "DTMF digits ended by the end character, which answer the elicited slot",
            Some(AUDIO_MODE),
            vec![
                text_input(ORDERS_A_PIZZA),
                dtmf_input("1"),
                dtmf_input("2"),
                dtmf_input("#"),
            ],
            vec![
                ("Text", "ElicitSlot", Some(SLOT), "InProgress", vec![SLOT_PROMPT]),
                ("DTMF", "Close", None, "Fulfilled", vec![CLOSING_MESSAGE]),
            ],
        ),
        (
            // The deletion character "clears the accumulated DTMF digits and
            // immediately ends the input", so nothing is left to interpret.
            "DTMF digits cleared by the deletion character, which interpret nothing",
            None,
            vec![dtmf_input("7"), dtmf_input("*"), dtmf_input("#")],
            vec![],
        ),
    ];

    for (index, (case, mode, events, expected)) in rows.iter().enumerate() {
        let session_id = format!("turns-{index}");
        let path = conversation_path(&bot, &session_id);
        let mut frames = vec![configuration(json!({}))];
        frames.extend(events.iter().cloned());
        let reply = converse(&emulator, &path, *mode, stream(&frames)).await;
        let answered = served(case, &reply);

        // One `IntentResultEvent`, plus a `TextResponseEvent` only where the step
        // the dialog reached has an authored message.
        let owed: usize = expected
            .iter()
            .map(|(_, _, _, _, messages)| 1 + usize::from(!messages.is_empty()))
            .sum();
        assert_eq!(
            answered.len(),
            owed,
            "case {case:?}: expected {owed} response frames, got {:?}",
            answered
                .iter()
                .map(|frame| frame.event_type.clone())
                .collect::<Vec<_>>()
        );

        let mut frame = answered.iter();
        for (input_mode, action, slot, intent_state, messages) in expected {
            let result = frame
                .next()
                .unwrap_or_else(|| panic!("case {case:?}: a turn reports an {INTENT_RESULT_EVENT}"));
            assert_eq!(
                result.event_type, INTENT_RESULT_EVENT,
                "case {case:?}: a turn opens with an {INTENT_RESULT_EVENT}, got {result:?}"
            );
            let reported = result.json(case);
            assert_eq!(
                reported.get("inputMode").and_then(Value::as_str),
                Some(*input_mode),
                "case {case:?}: expected inputMode {input_mode:?}, got {reported}"
            );
            assert_eq!(
                reported.get("sessionId").and_then(Value::as_str),
                Some(session_id.as_str()),
                "case {case:?}: a turn reports the session it advanced, got {reported}"
            );
            assert_eq!(
                reported.pointer("/sessionState/dialogAction/type").and_then(Value::as_str),
                Some(*action),
                "case {case:?}: expected dialogAction.type {action:?}, got {reported}"
            );
            assert_eq!(
                reported
                    .pointer("/sessionState/dialogAction/slotToElicit")
                    .and_then(Value::as_str),
                *slot,
                "case {case:?}: expected slotToElicit {slot:?}, got {reported}"
            );
            assert_eq!(
                reported.pointer("/sessionState/intent/state").and_then(Value::as_str),
                Some(*intent_state),
                "case {case:?}: expected intent.state {intent_state:?}, got {reported}"
            );
            assert!(
                reported
                    .pointer("/interpretations/0/intent/name")
                    .and_then(Value::as_str)
                    .is_some(),
                "case {case:?}: a turn reports what it made of the input, got {reported}"
            );
            if messages.is_empty() {
                continue;
            }
            let text = frame
                .next()
                .unwrap_or_else(|| panic!("case {case:?}: a turn with messages sends them"));
            assert_eq!(
                text.event_type, TEXT_RESPONSE_EVENT,
                "case {case:?}: the bot's messages travel in a {TEXT_RESPONSE_EVENT}, got {text:?}"
            );
            assert_eq!(
                message_contents(case, text),
                *messages,
                "case {case:?}: expected the messages {messages:?}"
            );
        }
    }
}

/// RULE: a conversation is held against the SESSION the four labels address — it
/// creates one that was not there, the configuration event configures it, and every
/// turn writes to it, so `GetSession` reports the conversation back.
#[tokio::test]
async fn a_conversation_is_held_against_the_session_the_labels_address() {
    let (emulator, bot) = Emulator::with_bot().await;
    let session_id = "held";
    let session = bot.session_path(session_id);

    // Nothing has started this conversation yet, so there is no session to read.
    let before = emulator.send("GET", &session, None).await;
    assert_eq!(
        before.status,
        404,
        "a session no conversation has started is absent, got {} {:?}",
        before.status,
        String::from_utf8_lossy(&before.body)
    );

    let case = "a configured conversation of one turn";
    let reply = converse(
        &emulator,
        &conversation_path(&bot, session_id),
        Some(TEXT_MODE),
        stream(&[
            configuration(json!({
                "welcomeMessages": [{"content": "Welcome to PizzaBot", "contentType": "PlainText"}],
                "sessionState": {"dialogAction": {"type": "ElicitIntent"}},
                "requestAttributes": {"channel": "web"},
            })),
            text_input(ORDERS_A_PIZZA),
        ]),
    )
    .await;
    let frames = served(case, &reply);

    // "A list of messages to send to the user": the welcome messages open the
    // response stream, before any turn.
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![TEXT_RESPONSE_EVENT, INTENT_RESULT_EVENT, TEXT_RESPONSE_EVENT],
        "case {case:?}: the welcome messages come first, then the turn"
    );
    assert_eq!(
        message_contents(case, &frames[0]),
        vec!["Welcome to PizzaBot"],
        "case {case:?}: the welcome messages are the ones the configuration sent"
    );
    let turn = frames[1].json(case);
    assert_eq!(
        turn.pointer("/requestAttributes/channel").and_then(Value::as_str),
        Some("web"),
        "case {case:?}: a turn reports the attributes the conversation accumulated, got {turn}"
    );

    // The session the conversation created and advanced, read back through the
    // operation that reports one.
    let read = emulator.send("GET", &session, None).await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: the conversation created the session it was held on, got {} {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let reported = read.json();
    assert_eq!(
        reported.pointer("/sessionState/dialogAction/type").and_then(Value::as_str),
        turn.pointer("/sessionState/dialogAction/type").and_then(Value::as_str),
        "case {case:?}: GetSession reports the dialog the conversation left, got {reported}"
    );
    assert_eq!(
        reported.pointer("/sessionState/intent/name").and_then(Value::as_str),
        Some(INTENT),
        "case {case:?}: GetSession reports the intent the conversation recognised, got {reported}"
    );
    assert_eq!(
        reported
            .pointer("/messages/0/content")
            .and_then(Value::as_str),
        Some(SLOT_PROMPT),
        "case {case:?}: GetSession reports the messages the turn last sent, got {reported}"
    );
    assert_eq!(
        reported.pointer("/sessionState/sessionAttributes").and_then(Value::as_object),
        Some(&serde_json::Map::new()),
        "case {case:?}: a conversation that set no session attribute reports an empty map, \
         got {reported}"
    );
}

/// RULE: "if the conversation mode is AUDIO you can send both audio and DTMF
/// information. If the mode is TEXT you can only send text" — so an audio or DTMF
/// event on a TEXT conversation is refused, and every mode takes text.
#[tokio::test]
async fn the_conversation_mode_decides_which_input_events_are_admitted() {
    let (emulator, bot) = Emulator::with_bot().await;

    // (case, conversationMode, the input event, whether the mode admits it)
    //
    // The rows are the three input-carrying members of the union against both values
    // of the enum and the absent header, which names no mode at all.
    let rows: Vec<(&str, Option<&str>, Vec<u8>, bool)> = vec![
        ("text on a TEXT conversation", Some(TEXT_MODE), text_input(ORDERS_A_PIZZA), true),
        ("text on an AUDIO conversation", Some(AUDIO_MODE), text_input(ORDERS_A_PIZZA), true),
        ("text with no mode named", None, text_input(ORDERS_A_PIZZA), true),
        ("DTMF on a TEXT conversation", Some(TEXT_MODE), dtmf_input("1"), false),
        ("DTMF on an AUDIO conversation", Some(AUDIO_MODE), dtmf_input("1"), true),
        ("DTMF with no mode named", None, dtmf_input("1"), true),
        ("audio on a TEXT conversation", Some(TEXT_MODE), audio_input(), false),
    ];

    for (index, (case, mode, event, admitted)) in rows.iter().enumerate() {
        let path = conversation_path(&bot, &format!("mode-{index}"));
        let reply = converse(
            &emulator,
            &path,
            *mode,
            stream(&[configuration(json!({})), event.clone()]),
        )
        .await;
        if *admitted {
            assert_eq!(
                reply.status,
                200,
                "case {case:?}: the mode admits the event, got {} {:?}",
                reply.status,
                String::from_utf8_lossy(&reply.body)
            );
        } else {
            // The stream was open by the time the event arrived, so the mode's
            // refusal travels on it.
            in_band_validation(case, &reply, None);
            let (_, message) = in_band(case, &served(case, &reply));
            assert!(
                message.contains(TEXT_MODE),
                "case {case:?}: the refusal names the mode that narrowed the input, got {message:?}"
            );
        }
    }
}

/// RULE: a request that cannot be READ — its address labels or its conversation mode
/// outside what the model declares, or a payload that is not a readable sequence of
/// event frames (not a message stream, past a budget, or carrying no event where the
/// member is `@required`) — is answered with the modelled `ValidationException` at HTTP
/// 400, and no stream is opened for it.
#[tokio::test]
async fn a_request_that_cannot_be_read_is_refused_with_a_validation_exception() {
    let (emulator, bot) = Emulator::with_bot().await;
    let valid = bot.bot_id.clone();

    /// One row: the case, the conversation path, the mode, the body, and the sentence
    /// owed where this service words the refusal itself.
    type Row<'r> = (&'r str, String, Option<&'r str>, Bytes, Option<&'r str>);
    let rows: Vec<Row> = vec![
        (
            "a stream with no event at all, which is a request with no payload",
            conversation_path(&bot, "empty"),
            Some(TEXT_MODE),
            Bytes::new(),
            None,
        ),
        (
            "a body that is not an event-stream message stream",
            conversation_path(&bot, "garbage"),
            Some(TEXT_MODE),
            Bytes::from_static(b"not an event stream at all"),
            None,
        ),
        (
            "a frame truncated mid-message",
            conversation_path(&bot, "truncated"),
            Some(TEXT_MODE),
            {
                let whole = configuration(json!({}));
                Bytes::from(whole[..whole.len() - 1].to_vec())
            },
            None,
        ),
        (
            "a frame a client may not send at all",
            conversation_path(&bot, "exception-frame"),
            Some(TEXT_MODE),
            stream(&[raw_frame(
                "exception",
                EXCEPTION_TYPE,
                VALIDATION_EXCEPTION,
                b"{}".to_vec(),
            )]),
            None,
        ),
        (
            "more frames than one request may carry",
            conversation_path(&bot, "too-many"),
            Some(TEXT_MODE),
            {
                let mut frames = vec![configuration(json!({}))];
                frames.extend((0..MAX_CONVERSATION_EVENTS).map(|_| text_input(ORDERS_A_PIZZA)));
                stream(&frames)
            },
            None,
        ),
        (
            "more bytes of frames than one request may carry",
            conversation_path(&bot, "too-large"),
            Some(TEXT_MODE),
            {
                // One frame whose payload alone is past the byte budget: the budget is
                // on the body, so one oversized frame is refused like many.
                let oversized = "a".repeat(MAX_CONVERSATION_STREAM_BYTES + 1);
                stream(&[configuration(json!({})), text_input(&oversized)])
            },
            None,
        ),
        (
            // `BotIdentifier` is `@length(10, 10)` with `@pattern(^[0-9a-zA-Z]+$)`,
            // and this service states a bot address it cannot look a bot up by in
            // its own fixed words.
            "a botId outside the shape of a bot identifier",
            format!(
                "/bots/short/botAliases/{}/botLocales/{}/sessions/bad-bot/conversation",
                bot.alias_id,
                bot.locale()
            ),
            Some(TEXT_MODE),
            stream(&[configuration(json!({}))]),
            Some("INVALID_REQUEST - Invalid bot name or alias"),
        ),
        (
            // `SessionId` is `@length(2, 100)` with `@pattern(^[0-9a-zA-Z._:-]+$)`.
            "a sessionId outside the shape of a session identifier",
            format!(
                "/bots/{valid}/botAliases/{}/botLocales/{}/sessions/x/conversation",
                bot.alias_id,
                bot.locale()
            ),
            Some(TEXT_MODE),
            stream(&[configuration(json!({}))]),
            None,
        ),
        (
            "a conversationMode outside the modelled enum",
            conversation_path(&bot, "bad-mode"),
            Some("VIDEO"),
            stream(&[configuration(json!({}))]),
            None,
        ),
    ];

    for (case, path, mode, body, expected) in rows {
        let reply = converse(&emulator, &path, mode, body).await;
        refused(case, &reply, expected);
    }
}

/// RULE: what the EVENTS of a readable stream say is answered ON that stream: the
/// response is open from the first event onwards, so a configuration out of place, a
/// member the input union does not declare and a payload the model refuses all travel
/// as the in-band `ValidationException` member and end the conversation.
#[tokio::test]
async fn an_event_the_model_refuses_travels_on_the_stream_it_arrived_on() {
    let (emulator, bot) = Emulator::with_bot().await;

    /// One row: the case, the body, and the sentence owed where this service words the
    /// refusal itself.
    type Row<'r> = (&'r str, Bytes, Option<&'r str>);
    let rows: Vec<Row> = vec![
        (
            "a stream that opens with an input event",
            stream(&[text_input(ORDERS_A_PIZZA)]),
            Some(CONFIGURATION_EVENT_FIRST),
        ),
        (
            "a stream that opens with a disconnection",
            stream(&[event_frame(DISCONNECTION_EVENT, &json!({}))]),
            Some(CONFIGURATION_EVENT_FIRST),
        ),
        (
            "a configuration event sent again, mid-conversation",
            stream(&[
                configuration(json!({})),
                text_input(ORDERS_A_PIZZA),
                configuration(json!({})),
            ]),
            Some(CONFIGURATION_EVENT_FIRST),
        ),
        (
            "a frame naming no member of the request union",
            stream(&[configuration(json!({})), event_frame("SomethingElse", &json!({}))]),
            None,
        ),
        (
            "a configuration event without the content type it requires",
            stream(&[event_frame(CONFIGURATION_EVENT, &json!({}))]),
            None,
        ),
        (
            "an utterance outside the length the model bounds it by",
            stream(&[configuration(json!({})), text_input("")]),
            None,
        ),
        (
            "a DTMF character outside the pattern the model declares",
            stream(&[configuration(json!({})), dtmf_input("E")]),
            None,
        ),
        (
            "welcome messages with no dialog action to send them in",
            stream(&[configuration(json!({
                "welcomeMessages": [{"content": "Hello", "contentType": "PlainText"}],
            }))]),
            Some(WELCOME_MESSAGES_NEED_A_DIALOG_ACTION),
        ),
        (
            "a seeded dialog naming an intent the locale does not declare",
            stream(&[configuration(json!({
                "sessionState": {"intent": {"name": "NoSuchIntent"}},
            }))]),
            Some("The intent is invalid."),
        ),
    ];

    for (index, (case, body, expected)) in rows.into_iter().enumerate() {
        let path = conversation_path(&bot, &format!("event-{index}"));
        let reply = converse(&emulator, &path, Some(AUDIO_MODE), body).await;
        in_band_validation(case, &reply, expected);
    }
}

/// RULE: an event refused mid-stream ends the conversation without discarding it — the
/// turns already taken keep their frames, and the session still holds what they left,
/// which is why that refusal travels in band rather than as a status that replaces the
/// whole response.
#[tokio::test]
async fn a_refusal_mid_stream_keeps_the_turns_already_taken() {
    let (emulator, bot) = Emulator::with_bot().await;
    let session_id = "refused-midway";
    let case = "a turn, then an event the conversation mode does not admit";

    let reply = converse(
        &emulator,
        &conversation_path(&bot, session_id),
        Some(TEXT_MODE),
        stream(&[
            configuration(json!({})),
            text_input(ORDERS_A_PIZZA),
            dtmf_input("1"),
            text_input("large"),
        ]),
    )
    .await;
    let frames = served(case, &reply);
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![INTENT_RESULT_EVENT, TEXT_RESPONSE_EVENT, VALIDATION_EXCEPTION],
        "case {case:?}: the turn taken before the refusal keeps its frames"
    );
    in_band_validation(case, &reply, None);

    // The session the first turn advanced is still there, and the utterance behind the
    // refused event took no turn: the slot it would have filled is still unfilled.
    let read = emulator
        .send("GET", &bot.session_path(session_id), None)
        .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: the session the conversation advanced survives the refusal, got {} {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let reported = read.json();
    assert_eq!(
        reported.pointer("/sessionState/dialogAction/slotToElicit").and_then(Value::as_str),
        Some(SLOT),
        "case {case:?}: the session holds the dialog the first turn reached, got {reported}"
    );
    assert!(
        reported
            .pointer(&format!("/sessionState/intent/slots/{SLOT}/value"))
            .is_none(),
        "case {case:?}: the event behind the refusal took no turn, got {reported}"
    );
}

/// RULE: a failure of the CONVERSATION once the request has been accepted — an
/// address that resolves to no built bot locale, or input this emulator cannot
/// process without a dependency it does not have — travels as the in-band exception
/// member the response union declares for it, on the stream the 200 opened, and ends
/// the conversation.
#[tokio::test]
async fn what_the_conversation_cannot_serve_travels_as_an_in_band_exception() {
    let (emulator, bot) = Emulator::with_bot().await;
    // A locale of the same bot that was created but never built, so the alias
    // resolves and the locale has no runtime model.
    emulator.create_locale(&bot.bot_id, "de_DE").await;

    let unknown_alias = "NOSUCHALIAS";
    // (case, the conversation path, the stream, the member owed, the message owed)
    let rows: Vec<(&str, String, Bytes, &str, String)> = vec![
        (
            "an alias that is not an alias of that bot",
            format!(
                "/bots/{}/botAliases/{unknown_alias}/botLocales/{}/sessions/absent-alias/conversation",
                bot.bot_id,
                bot.locale()
            ),
            stream(&[configuration(json!({}))]),
            RESOURCE_NOT_FOUND,
            bot_alias_not_valid_message(&bot.bot_id, unknown_alias),
        ),
        (
            "a locale of that bot with no built model",
            format!(
                "/bots/{}/botAliases/{}/botLocales/de_DE/sessions/unbuilt/conversation",
                bot.bot_id, bot.alias_id
            ),
            stream(&[configuration(json!({}))]),
            RESOURCE_NOT_FOUND,
            BOT_LOCALE_NOT_VALID_MESSAGE.to_string(),
        ),
        (
            "a response content type that asks for synthesized speech",
            conversation_path(&bot, "spoken"),
            stream(&[configuration(json!({"responseContentType": "audio/mpeg"}))]),
            DEPENDENCY_FAILED,
            String::new(),
        ),
        (
            "audio input, which no recogniser here can transcribe",
            conversation_path(&bot, "spoken-input"),
            stream(&[configuration(json!({})), audio_input(), text_input(ORDERS_A_PIZZA)]),
            DEPENDENCY_FAILED,
            String::new(),
        ),
    ];

    for (case, path, body, member, message) in rows {
        let reply = converse(&emulator, &path, Some(AUDIO_MODE), body).await;
        let frames = served(case, &reply);
        let (reported_member, reported_message) = in_band(case, &frames);
        assert_eq!(
            reported_member, member,
            "case {case:?}: expected the in-band {member}, got {reported_member}"
        );
        if !message.is_empty() {
            assert_eq!(
                reported_message, message,
                "case {case:?}: expected the sentence {message:?}, got {reported_message:?}"
            );
        }
        // The exception ENDS the stream: nothing of the conversation follows it, so
        // the input event behind it took no turn.
        assert!(
            frames
                .iter()
                .all(|frame| frame.event_type != INTENT_RESULT_EVENT),
            "case {case:?}: an in-band exception ends the conversation, got {:?}",
            frames
                .iter()
                .map(|frame| frame.event_type.clone())
                .collect::<Vec<_>>()
        );
    }
}

/// RULE: "a notification from the client that it is disconnecting from Amazon Lex
/// V2" ends the conversation, so nothing behind that event is read — while the
/// session it was held on survives it, as every session of this service does until
/// it goes idle or is deleted.
#[tokio::test]
async fn a_disconnection_event_ends_the_conversation() {
    let (emulator, bot) = Emulator::with_bot().await;
    let session_id = "disconnected";
    let case = "an utterance, a disconnection, and an utterance behind it";

    let reply = converse(
        &emulator,
        &conversation_path(&bot, session_id),
        Some(TEXT_MODE),
        stream(&[
            configuration(json!({})),
            text_input(ORDERS_A_PIZZA),
            event_frame(DISCONNECTION_EVENT, &json!({})),
            text_input("large"),
        ]),
    )
    .await;
    let frames = served(case, &reply);
    assert_eq!(
        frames
            .iter()
            .map(|frame| frame.event_type.as_str())
            .collect::<Vec<_>>(),
        vec![INTENT_RESULT_EVENT, TEXT_RESPONSE_EVENT],
        "case {case:?}: only the turn before the disconnection is served"
    );

    let read = emulator
        .send("GET", &bot.session_path(session_id), None)
        .await;
    assert_eq!(
        read.status,
        200,
        "case {case:?}: the session survives the disconnection, got {} {:?}",
        read.status,
        String::from_utf8_lossy(&read.body)
    );
    let reported = read.json();
    assert_eq!(
        reported.pointer("/sessionState/intent/name").and_then(Value::as_str),
        Some(INTENT),
        "case {case:?}: the session holds the turn taken before the disconnection, got {reported}"
    );
    // The utterance behind the disconnection was not read, so the slot it would have
    // filled is still the documented null.
    assert!(
        reported
            .pointer(&format!("/sessionState/intent/slots/{SLOT}/value"))
            .is_none(),
        "case {case:?}: the event behind the disconnection took no turn, got {reported}"
    );
    assert_ne!(
        reported.pointer("/sessionState/intent/name").and_then(Value::as_str),
        Some(FALLBACK_INTENT),
        "case {case:?}: the conversation recognised the bot's own intent, got {reported}"
    );
}
