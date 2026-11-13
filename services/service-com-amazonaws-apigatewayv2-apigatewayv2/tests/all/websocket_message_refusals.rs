// SPDX-License-Identifier: CC0-1.0

//! What a WEBSOCKET api's socket does with a client message it does NOT serve, driven over a
//! real connection against the address the api announces.
//!
//! The rule both tables below are slices of:
//!
//! > A WEBSOCKET api carries TEXT messages, in frames up to its frame quota and messages up to
//! > its per-message ceiling. A client message outside that — a data frame whose opcode is not
//! > text, a frame larger than the frame quota, or a message whose ASSEMBLED payload passes
//! > the ceiling — selects no route and reaches no integration: the gateway answers it with an
//! > RFC 6455 CLOSE frame carrying the status code of that refusal (`1003` for data the
//! > endpoint does not accept, `1009` for a message too big to process) and the sentence the
//! > gateway words that refusal with, and the connection then ends. A message inside it is
//! > dispatched and answered as ever.
//!
//! Every api below carries a TWO-WAY `$default` route, so a message that IS served is read
//! back as that route's reply: for a refused message the client reads the CLOSE and then the
//! end of the connection and never that reply, which is what "no integration is invoked"
//! looks like from the wire.
//!
//! The rows span what the rule generalises over: the frame's opcode (text, binary), the
//! payload under it (empty, bytes that are not UTF-8, bytes that ARE a message a route
//! serves), the framing (one frame, fragments), and the size against each published bound (far
//! under it, at it, one byte past it, half again past it).
//!
//! The suites that own the rest of this socket are not restated here: the handshake's own
//! statuses and the dispatch of a served message are `websocket_transport_data_plane.rs`.

use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use smithy_server_runtime::MultiServiceRouter;
use tokio_tungstenite::tungstenite::protocol::frame::coding::{Data, OpCode};
use tokio_tungstenite::tungstenite::protocol::frame::Frame;
use tokio_tungstenite::tungstenite::Message;

use crate::api_lifecycle::emulator;
use crate::websocket_transport_data_plane::{
    create_api, create_integration, create_integration_response, create_route, create_stage,
    handshake_at, Api, ClientSocket,
};

/// The stage every api below is deployed to.
const STAGE: &str = "prod";

/// The per-message ceiling a client's message on a WebSocket api is served under, in bytes:
/// the 128 KiB API Gateway publishes as a WebSocket api's message payload quota, which is also
/// the maximum the push the other way is modelled at (`PostToConnection$Data`,
/// `@length(max: 131072)`, of the `@connections` management API).
const MESSAGE_CEILING_BYTES: usize = 131_072;

/// The largest payload ONE frame of a client's message carries, in bytes: the 32 KiB API
/// Gateway publishes as a WebSocket api's frame quota, so a message at
/// [`MESSAGE_CEILING_BYTES`] arrives as several frames.
const FRAGMENT_BYTES: usize = 32 * 1024;

/// The RFC 6455 §7.4.1 status codes a refused message is closed with: 1003 for "data within a
/// message that it does not understand", 1009 for "a message that is too big for it to
/// process" — each with the sentence API Gateway words that refusal with, which the CLOSE
/// frame carries after the status code.
const CLOSE_UNSUPPORTED_DATA: (u16, &str) = (1003, "Binary is not supported");
const CLOSE_MESSAGE_TOO_BIG: (u16, &str) = (1009, "Message too big");

/// What the `$default` route's MOCK integration renders for a message it serves, so a served
/// message is observable by the frame it is answered with.
const SERVED_REPLY: &str = r#"{"served":"yes"}"#;

/// How long a case waits for the frame its message is answered with before failing: generous
/// for a loaded test machine, and bounded so a case that will never be answered fails instead
/// of hanging the suite.
const FRAME_DEADLINE: Duration = Duration::from_secs(30);

/// What a client reads back after sending one message.
#[derive(Debug, PartialEq, Eq)]
enum Answer {
    /// The payload the serving route's integration response rendered.
    Reply(String),
    /// A CLOSE frame: the status code it carried (`None` for a close carrying none) and the
    /// reason phrase beside it.
    Close(Option<u16>, String),
    /// Nothing at all: the connection ended with no frame written, which is the symptom a
    /// client cannot tell a refusal from.
    Ended,
}

/// The answer a refusal is: the close it states, as a row spells that refusal out.
fn closed((code, reason): (u16, &str)) -> Answer {
    Answer::Close(Some(code), reason.to_string())
}

/// A WEBSOCKET api deployed with a TWO-WAY `$default` route over a MOCK integration that
/// renders [`SERVED_REPLY`], whatever the message's own body is.
async fn deployed_api(emulator: &MultiServiceRouter, case: &str, name: &str) -> Api {
    let api = create_api(emulator, case, name).await;
    let integration = create_integration(
        emulator,
        case,
        &api.api_id,
        json!({
            "integrationType": "MOCK",
            "requestTemplates": { "$default": "{\"statusCode\": 200}" }
        }),
    )
    .await;
    create_integration_response(
        emulator,
        case,
        &api.api_id,
        &integration,
        json!({
            "integrationResponseKey": "$default",
            "responseTemplates": { "$default": SERVED_REPLY }
        }),
    )
    .await;
    create_route(emulator, case, &api.api_id, "$default", &integration, true).await;
    create_stage(emulator, case, &api.api_id, STAGE, true).await;
    api
}

/// Open one connection of `api`'s own socket.
async fn connect(case: &str, api: &Api) -> ClientSocket {
    handshake_at(case, &api.authority, &format!("/{STAGE}"))
        .await
        .unwrap_or_else(|status| panic!("case {case:?}: the handshake must complete, got {status}"))
}

/// Write `frames` as they stand and read what the api answers.
///
/// A write that fails is not a failure of the case: a connection the api has already refused
/// and closed is one the rest of a message cannot be written to, and what the api answered is
/// read from the frames it wrote before that.
async fn answer(case: &str, socket: &mut ClientSocket, frames: Vec<Message>) -> Answer {
    for frame in frames {
        if socket.send(frame).await.is_err() {
            break;
        }
    }
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(Some(Ok(Message::Text(text)))) => Answer::Reply(text.as_str().to_string()),
        Ok(Some(Ok(Message::Close(frame)))) => match frame {
            Some(frame) => Answer::Close(
                Some(u16::from(frame.code)),
                frame.reason.as_str().to_string(),
            ),
            None => Answer::Close(None, String::new()),
        },
        Ok(Some(Ok(other))) => panic!("case {case:?}: unexpected frame {other:?}"),
        Ok(Some(Err(error))) => panic!("case {case:?}: the socket failed: {error}"),
        Ok(None) => Answer::Ended,
        Err(_elapsed) => panic!(
            "case {case:?}: the api answered nothing within {FRAME_DEADLINE:?}, so neither a \
             refusal nor a reply is observable"
        ),
    }
}

/// After a refusal the connection is over: whatever the transport reports next, it is never
/// another frame of the api — in particular never the reply a served message gets.
async fn ended(case: &str, socket: &mut ClientSocket) {
    match tokio::time::timeout(FRAME_DEADLINE, socket.next()).await {
        Ok(None) | Ok(Some(Err(_))) => {}
        Ok(Some(Ok(frame))) => panic!(
            "case {case:?}: expected the connection ended after the refusal, got {frame:?}"
        ),
        Err(_elapsed) => {
            panic!("case {case:?}: expected the connection ended after the refusal, it is open")
        }
    }
}

/// `count` bytes of text, as a message of that size.
fn text_of(count: usize) -> String {
    "a".repeat(count)
}

/// One text message of `count` bytes, written as fragments of at most [`FRAGMENT_BYTES`]: the
/// first frame carries the text opcode and is not final, every later frame continues it, and
/// the last one ends the message (RFC 6455 §5.4).
fn fragments(count: usize) -> Vec<Message> {
    let payload = text_of(count).into_bytes();
    let chunks: Vec<&[u8]> = payload.chunks(FRAGMENT_BYTES).collect();
    let last = chunks.len().saturating_sub(1);
    chunks
        .iter()
        .enumerate()
        .map(|(at, chunk)| {
            let opcode = match at {
                0 => OpCode::Data(Data::Text),
                _ => OpCode::Data(Data::Continue),
            };
            Message::Frame(Frame::message(
                Bytes::copy_from_slice(chunk),
                opcode,
                at == last,
            ))
        })
        .collect()
}

/// A client DATA frame that is not text is refused with CLOSE 1003 and reaches no route,
/// whatever it carries; a text message over the same connection's api is served as ever.
#[tokio::test]
async fn a_client_data_frame_that_is_not_text_is_refused_with_close_1003() {
    /// One row: the frames the client writes, and what it must read back.
    struct Row {
        case: &'static str,
        frames: Vec<Message>,
        expected: Answer,
    }

    let selecting = json!({ "action": "$default" }).to_string();
    let rows = vec![
        Row {
            case: "a binary frame carrying a payload",
            frames: vec![Message::Binary(Bytes::from_static(b"\x00\x01\x02binary"))],
            expected: closed(CLOSE_UNSUPPORTED_DATA),
        },
        Row {
            case: "an empty binary frame",
            frames: vec![Message::Binary(Bytes::new())],
            expected: closed(CLOSE_UNSUPPORTED_DATA),
        },
        Row {
            case: "a binary frame carrying bytes that are not UTF-8",
            frames: vec![Message::Binary(Bytes::from_static(&[
                0xff, 0xfe, 0x00, 0x80,
            ]))],
            expected: closed(CLOSE_UNSUPPORTED_DATA),
        },
        Row {
            // The opcode is what decides: these bytes are exactly the message the api's
            // $default route serves when they arrive as text.
            case: "a binary frame whose bytes are the message a route serves",
            frames: vec![Message::Binary(Bytes::from(selecting.clone()))],
            expected: closed(CLOSE_UNSUPPORTED_DATA),
        },
        Row {
            case: "a binary message written as two fragments",
            frames: vec![
                Message::Frame(Frame::message(
                    Bytes::from_static(b"first"),
                    OpCode::Data(Data::Binary),
                    false,
                )),
                Message::Frame(Frame::message(
                    Bytes::from_static(b"second"),
                    OpCode::Data(Data::Continue),
                    true,
                )),
            ],
            expected: closed(CLOSE_UNSUPPORTED_DATA),
        },
        Row {
            // The contrast row: the same payload as a TEXT frame is dispatched and answered.
            case: "a text message carrying the same bytes",
            frames: vec![Message::Text(selecting.clone().into())],
            expected: Answer::Reply(SERVED_REPLY.to_string()),
        },
    ];

    let emulator = emulator();
    let api = deployed_api(&emulator, "non-text client frames", "opcode-api").await;

    for row in rows {
        let case = row.case;
        // A refusal ends the connection, so every row drives one of its own.
        let mut socket = connect(case, &api).await;
        let answered = answer(case, &mut socket, row.frames).await;
        assert_eq!(
            answered, row.expected,
            "case {case:?}: expected {:?}, got {answered:?}",
            row.expected
        );
        if matches!(row.expected, Answer::Close(_, _)) {
            ended(case, &mut socket).await;
        }
    }
}

/// A client message is served up to the published bounds — the frame quota, and the
/// per-message ceiling over the payload the FRAGMENTS assemble to — and refused with CLOSE
/// 1009 past either.
#[tokio::test]
async fn a_client_message_past_a_published_size_bound_is_refused_with_close_1009() {
    /// One row: the frames the client writes, and what it must read back.
    struct Row {
        case: &'static str,
        frames: Vec<Message>,
        expected: Answer,
    }

    let rows = vec![
        Row {
            case: "a short text message, far under both bounds",
            frames: vec![Message::Text(text_of(64).into())],
            expected: Answer::Reply(SERVED_REPLY.to_string()),
        },
        Row {
            case: "one frame carrying exactly the frame quota",
            frames: vec![Message::Text(text_of(FRAGMENT_BYTES).into())],
            expected: Answer::Reply(SERVED_REPLY.to_string()),
        },
        Row {
            case: "fragments assembling to exactly the ceiling",
            frames: fragments(MESSAGE_CEILING_BYTES),
            expected: Answer::Reply(SERVED_REPLY.to_string()),
        },
        Row {
            case: "one frame carrying one byte past the frame quota",
            frames: vec![Message::Text(text_of(FRAGMENT_BYTES + 1).into())],
            expected: closed(CLOSE_MESSAGE_TOO_BIG),
        },
        Row {
            case: "fragments assembling to one byte past the ceiling, each frame itself at the \
                   frame quota",
            frames: fragments(MESSAGE_CEILING_BYTES + 1),
            expected: closed(CLOSE_MESSAGE_TOO_BIG),
        },
        Row {
            case: "fragments assembling to half again the ceiling",
            frames: fragments(MESSAGE_CEILING_BYTES + MESSAGE_CEILING_BYTES / 2),
            expected: closed(CLOSE_MESSAGE_TOO_BIG),
        },
    ];

    let emulator = emulator();
    let api = deployed_api(&emulator, "the published size bounds", "ceiling-api").await;

    for row in rows {
        let case = row.case;
        let mut socket = connect(case, &api).await;
        let answered = answer(case, &mut socket, row.frames).await;
        assert_eq!(
            answered, row.expected,
            "case {case:?}: expected {:?}, got {answered:?}",
            row.expected
        );
        if matches!(row.expected, Answer::Close(_, _)) {
            ended(case, &mut socket).await;
        }
    }
}
