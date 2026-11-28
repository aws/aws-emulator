// SPDX-License-Identifier: CC0-1.0

//! `application/vnd.amazon.eventstream` wire support for an operation whose OUTPUT
//! is a `@streaming` union, plus the seam that lets an editable `service.rs` route a
//! request to one.
//!
//! # Why this lives here
//!
//! The scaffold's op-stub generator declines an operation with a `@streaming`
//! event-stream member (each service's `src/ops/SKIPPED_STUBS.md` records it), and the
//! generated `ErasedOperation` shim of such an operation declines the request too: its
//! `serialize_as_event_stream<P>` needs a `Sized` protocol, which the object-safe `run`
//! cannot hand it. So every service with a streaming operation needs the SAME pieces,
//! none of which names a service:
//!
//! * [`deserialize_request`] — read a request into the modelled input shape through the
//!   service's own protocol, then check it against the model's constraints.
//! * [`decode_input_stream`] — split a REQUEST body into the event frames a client framed
//!   into it, reading the `:event-type` header the AWS SDKs set on each one, under a frame
//!   budget the calling service chooses.
//! * [`read_request`] — the same read, handing the refusal back STRUCTURALLY (one
//!   [`Violation`] per requirement the request broke) instead of already worded, for a
//!   service that states a requirement in its own words.
//! * [`frame_event`] — serialize one modelled event structure as ONE
//!   `vnd.amazon.eventstream` frame, through the service's own protocol (so the frame
//!   headers and the payload encoding are the protocol's, not this file's).
//! * [`frame_initial_response`] / [`frame_empty_initial_response`] — the frame an
//!   event-stream RESPONSE opens with, before any modelled event frame.
//! * [`event_stream_response`] / [`event_stream_only_response`] / [`error_response`] —
//!   a 200 whose modelled response headers come from the output shape and whose body is
//!   the concatenated frames, and a modelled error in the service's own envelope.
//! * [`EventStreamOperation`] / [`EventStreamRoute`] — the object-safe adapter
//!   `Service::override_operation` hands back.
//!
//! # Services still carrying their own copy
//!
//! Two crates inlined these helpers before there was anywhere shared to put them:
//!
//! * `services/service-com-amazonaws-transcribestreaming-transcribe/src/ops/types/event_stream.rs`
//!   — the full set, near-identical to this module, including its own copy of
//!   [`decode_input_stream`] with that service's frame budget inlined rather than passed;
//!   and
//! * `services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/src/ops/invoke_endpoint_with_bidirectional_stream.rs`
//!   — a partial copy of the framing inside the one operation that needs it.
//!
//! Both are to be MIGRATED onto this module: delete the local copy, `use
//! service_common::event_stream::…`. That migration is NOT in either change that added
//! to this module, because it rewrites the wire path of three Transcribe Streaming
//! operations and one SageMaker Runtime one, whose correctness neither change's issue
//! owns and whose own suites are the only thing that could pass judgement on it; doing it
//! there would put an unreviewed rewrite of two other services inside a fix for a third.
//! Two further operations hold a partial copy of the INPUT decoder for the same reason —
//! `services/service-com-amazonaws-connecthealth-connecthealth/src/ops/start_medical_scribe_listening_session.rs`
//! and `services/service-com-amazonaws-polly-parrot-v1/src/ops/start_speech_synthesis_stream.rs`.
//! Nothing new should inline any of it again — call this module.

use aws_smithy_eventstream::frame::{read_message_from, write_message_to};
use aws_smithy_runtime_api::http::{Request, Response, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use aws_smithy_schema_server::constraints::{envelope, validate};
use aws_smithy_schema_server::{DeserializableStruct, ProtocolError, Violation, ViolationKind};
use aws_smithy_types::body::SdkBody;
use aws_smithy_types::event_stream::{Header, HeaderValue, Message};
use bytes::Buf;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, OperationError, ServerHttpProtocol, Service,
};

/// The content type of an event-stream body, in both directions.
pub const EVENT_STREAM_CONTENT_TYPE: &str = "application/vnd.amazon.eventstream";

/// Read `req` into the modelled input shape `T` and check it against the model.
///
/// Driven through the service's own protocol, so the HTTP bindings the model
/// declares — `@httpLabel`, `@httpHeader`, `@httpQuery`, `@httpPayload` — and the
/// body codec are the protocol's. `Err` carries the refusal sentence, which the
/// caller answers with the modelled error for a bad request; an absent required
/// member is worded exactly as the framework's own boundary words it.
pub fn deserialize_request<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    schema: &Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    read_request(protocol, req, schema).map_err(RequestRefusal::stated)
}

/// Why the shared reader refused a request, before any words were chosen for it.
///
/// The framework describes a boundary refusal STRUCTURALLY and leaves the wording to
/// whoever emulates the service (`smithy_server_runtime::Refusal`), because no model
/// records it. A hand-authored operation reads its own request, so it needs the refusal
/// in the same form the refusal seam gets it in — otherwise a service that states a
/// requirement its own way can only restate the sentence it was handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestRefusal {
    /// The request was read and broke the model's rules: one violation per requirement,
    /// as the framework's own walk reports them, and an absent required member as the
    /// violation it is.
    Invalid(Vec<Violation>),
    /// The request could not be READ at all — a body the protocol's codec could not
    /// parse — which has no violation form, so the reader's own sentence is carried.
    Unreadable(String),
}

impl RequestRefusal {
    /// The sentence this refusal is worded with, every violation stated as the FRAMEWORK
    /// states it.
    ///
    /// A service that words a requirement its own way restates the violations first and
    /// renders the envelope itself, which is what having them structurally is for.
    #[must_use]
    pub fn stated(self) -> String {
        match self {
            // One violation renders, and the reader builds this variant from a non-empty
            // walk only, so the envelope is always there.
            Self::Invalid(violations) => envelope(&violations).unwrap_or_default(),
            Self::Unreadable(message) => message,
        }
    }
}

/// As [`deserialize_request`], with the refusal handed back structurally
/// ([`RequestRefusal`]) rather than already worded.
///
/// Every other part of the read is that function's: the HTTP bindings and the body codec
/// are the service's own protocol's, and the rules checked are the model's.
pub fn read_request<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &Request,
    schema: &Schema,
) -> Result<T, RequestRefusal>
where
    T: DeserializableStruct + SerializableStruct,
{
    let mut deserializer = protocol.deserialize_input(req, schema).map_err(|error| {
        RequestRefusal::Unreadable(format!("the request could not be read: {error}"))
    })?;
    let value = T::deserialize(&mut *deserializer).map_err(refusal_of_serde_error)?;
    drop(deserializer);
    let violations = validate(&value, schema);
    if violations.is_empty() {
        Ok(value)
    } else {
        Err(RequestRefusal::Invalid(violations))
    }
}

/// A deserialization failure as the framework's own boundary describes it: an absent
/// required member is the VIOLATION it is — which is how that boundary reports one, so
/// the two cannot drift — and anything else has no violation form.
fn refusal_of_serde_error(error: SerdeError) -> RequestRefusal {
    match error {
        SerdeError::MissingMember { member_name } => RequestRefusal::Invalid(vec![Violation {
            path: member_name,
            kind: ViolationKind::MissingRequiredMember,
            value: None,
            sensitive: false,
        }]),
        other => RequestRefusal::Unreadable(other.to_string()),
    }
}

/// One frame of a REQUEST's event stream: which member of the `@streaming` input union it
/// carries, and the bytes of its payload.
///
/// The payload of an event whose structure has an `@eventPayload` member is that member's
/// raw value; for any other event it is the event structure encoded in the protocol's body
/// format, which [`read_request`] reads it back out of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedEvent {
    /// The union member's name, as the frame's `:event-type` header spells it.
    pub event_type: String,
    /// The frame's payload bytes, exactly as they arrived.
    pub payload: Vec<u8>,
}

/// Why a request's event-stream body could not be used.
///
/// The two cases are carried apart rather than as one sentence because a service answers
/// them with different modelled errors: a body that is not a message stream is the
/// caller's mistake, while a body past the frame budget is a limit the service states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputStreamRefusal {
    /// The body is not a usable `vnd.amazon.eventstream` message stream, or one of its
    /// frames is not an event a client may send.
    Malformed(String),
    /// The body carries more frames than the budget the service passed admits.
    TooManyEvents(String),
}

impl InputStreamRefusal {
    /// The sentence this refusal is worded with.
    #[must_use]
    pub fn stated(self) -> String {
        match self {
            Self::Malformed(message) | Self::TooManyEvents(message) => message,
        }
    }
}

/// Split an event-stream request body into its frames, in ARRIVAL ORDER.
///
/// Each frame has to be an `event` frame naming its union member in `:event-type`, which
/// is what an AWS SDK's own encoder writes: `exception` is the direction a SERVER answers
/// in, so a client sending one is sending something no input union declares.
///
/// Bounded at `max_events` frames: the frame count is caller-chosen work, so it is refused
/// before the work is done rather than after. The budget belongs to the calling service —
/// how many events one of its requests may carry is its own limit — so it is a parameter
/// here and the refusal carries it.
pub fn decode_input_stream(
    body: &[u8],
    max_events: usize,
) -> Result<Vec<DecodedEvent>, InputStreamRefusal> {
    let mut buffer = bytes::Bytes::copy_from_slice(body);
    let mut events: Vec<DecodedEvent> = Vec::new();
    while buffer.has_remaining() {
        if events.len() >= max_events {
            return Err(InputStreamRefusal::TooManyEvents(format!(
                "the request stream carries more than the {max_events} events this service \
                 accepts in one request; send fewer, larger events"
            )));
        }
        let before = buffer.remaining();
        let message = read_message_from(&mut buffer).map_err(|error| {
            InputStreamRefusal::Malformed(format!(
                "the request body is not a valid {EVENT_STREAM_CONTENT_TYPE} message \
                 stream: {error}"
            ))
        })?;
        if buffer.remaining() >= before {
            return Err(InputStreamRefusal::Malformed(
                "the request body carries a zero-length event-stream frame".to_string(),
            ));
        }
        let header = |name: &str| {
            message
                .headers()
                .iter()
                .find(|header| header.name().as_str() == name)
                .and_then(|header| match header.value() {
                    HeaderValue::String(value) => Some(value.as_str().to_string()),
                    _ => None,
                })
        };
        let message_type = header(":message-type").unwrap_or_default();
        if message_type != "event" {
            return Err(InputStreamRefusal::Malformed(format!(
                "an event-stream frame with :message-type '{message_type}' may not be sent \
                 on a request stream; only 'event' frames may"
            )));
        }
        let Some(event_type) = header(":event-type") else {
            return Err(InputStreamRefusal::Malformed(
                "an event-stream frame carries no :event-type header naming the event"
                    .to_string(),
            ));
        };
        events.push(DecodedEvent {
            event_type,
            payload: message.payload().to_vec(),
        });
    }
    Ok(events)
}

/// Serialize one modelled event structure as a single `vnd.amazon.eventstream`
/// frame, through the service's own protocol.
///
/// `event_type` is the `@streaming` union's member name, which the frame carries in
/// its `:event-type` header; `is_exception` marks an in-band error member, which
/// travels as `:message-type: exception` instead.
pub fn frame_event(
    protocol: &dyn ServerHttpProtocol,
    event_type: &str,
    is_exception: bool,
    event: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<Vec<u8>, ProtocolError> {
    let framed = protocol.serialize_event_stream_message(event_type, is_exception, event, schema)?;
    Ok(framed
        .into_body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default())
}

/// Frame one event whose payload is ALREADY encoded as a single
/// `vnd.amazon.eventstream` message.
///
/// [`frame_event`] writes an event the model declares as a STRUCTURE, serializing its
/// members into the frame's payload through the service's own protocol. An event whose
/// payload is a BLOB has no member to serialize: the `@eventPayload` of an event shape,
/// and the payload of a model that declares its stream as a streaming blob rather than
/// as a union of event structures, ARE the frame's body, under the media type those
/// bytes are in. That binding is the one a struct serializer cannot express — it would
/// base64 the bytes into a JSON member — so the bytes are framed here instead, with the
/// same three headers a protocol-serialized event frame carries, and read by a client's
/// decoder exactly as one of those is.
pub fn frame_payload_event(
    event_type: &str,
    payload_content_type: &str,
    payload: Vec<u8>,
) -> Result<Vec<u8>, ProtocolError> {
    let message = Message::new(payload)
        .add_header(Header::new(
            ":message-type",
            HeaderValue::String("event".into()),
        ))
        .add_header(Header::new(
            ":event-type",
            HeaderValue::String(event_type.to_string().into()),
        ))
        .add_header(Header::new(
            ":content-type",
            HeaderValue::String(payload_content_type.to_string().into()),
        ));
    let mut framed = Vec::new();
    write_message_to(&message, &mut framed).map_err(|error| {
        ProtocolError::Serde(SerdeError::custom(format!(
            "the {event_type} frame could not be encoded: {error}"
        )))
    })?;
    Ok(framed)
}

/// The `:event-type` of the frame an event-stream response OPENS with.
///
/// A response whose body is an event stream begins with a frame of this type carrying the
/// output's non-stream members, and only then the frames of the `@streaming` union. A
/// client reads that frame FIRST — it is how the output shape itself arrives — and treats a
/// body whose first frame is of any other type as unparseable, so a stream that omits it
/// delivers none of the events behind it.
pub const INITIAL_RESPONSE_EVENT: &str = "initial-response";

/// The leading frame of an event-stream response, carrying the output's non-stream members.
///
/// Framed exactly as a data event is ([`frame_event`]), through the service's own protocol,
/// so the payload encoding is the protocol's.
pub fn frame_initial_response(
    protocol: &dyn ServerHttpProtocol,
    members: &dyn SerializableStruct,
    schema: &Schema,
) -> Result<Vec<u8>, ProtocolError> {
    frame_event(protocol, INITIAL_RESPONSE_EVENT, false, members, schema)
}

/// The leading frame for an output whose ONLY member is the `@streaming` payload: there are
/// no other members to carry, so the frame's payload is the empty document — which is what a
/// client reads the output shape out of.
pub fn frame_empty_initial_response(
    protocol: &dyn ServerHttpProtocol,
) -> Result<Vec<u8>, ProtocolError> {
    frame_initial_response(protocol, &NoMembers, &NO_MEMBERS_SCHEMA)
}

/// An output with no non-stream members, for [`frame_empty_initial_response`]: it writes no
/// member, so the protocol's codec renders it as the empty document.
struct NoMembers;

impl SerializableStruct for NoMembers {
    fn serialize_members(&self, _: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        Ok(())
    }
}

/// The schema [`NoMembers`] is written through: a structure with no members, which is what
/// `smithy.api#Unit` is.
static NO_MEMBERS_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static("smithy.api#Unit", "smithy.api", "Unit"),
    ShapeType::Structure,
    &[],
);

/// A 200 carrying `frames` as its event-stream body and the operation's modelled
/// response headers.
///
/// The headers are serialized from the output shape through the protocol, so every
/// header name, and the rendering of every value, is the model's. The body's own
/// content type is the event-stream one, which replaces whatever the protocol's
/// non-streaming rendering would have set.
pub fn event_stream_response(
    protocol: &dyn ServerHttpProtocol,
    operation: &str,
    headers: &dyn SerializableStruct,
    headers_schema: &Schema,
    frames: Vec<u8>,
) -> Result<Response, ProtocolError> {
    let rendered = protocol.serialize_output(headers, headers_schema, operation, 200)?;
    let mut response = event_stream_only_response(frames);
    for (name, value) in rendered.headers().iter() {
        if name.eq_ignore_ascii_case("content-type") {
            continue;
        }
        response
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Ok(response)
}

/// A 200 carrying `frames` as its event-stream body, for an output shape whose ONLY
/// member is the `@streaming` payload — so there are no modelled response headers to
/// serialize and [`event_stream_response`] would have nothing to read them from.
pub fn event_stream_only_response(frames: Vec<u8>) -> Response {
    let mut response = Response::new(
        StatusCode::try_from(200u16).expect("200 is a valid status"),
        SdkBody::from(frames),
    );
    response
        .headers_mut()
        .insert("content-type", EVENT_STREAM_CONTENT_TYPE);
    response
}

/// Render a modelled error in the serving service's own envelope, with the status the
/// model gives it.
pub fn error_response(protocol: &dyn ServerHttpProtocol, error: &dyn OperationError) -> Response {
    let status = error.http_status();
    protocol
        .serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            error.shape_name(),
            status,
        )
        .unwrap_or_else(|_| {
            // Unreachable for a JSON envelope, but an error path must not panic.
            let mut response = Response::new(
                StatusCode::try_from(500u16).expect("500 is a valid status"),
                SdkBody::from(r#"{"__type":"InternalServerException"}"#),
            );
            response
                .headers_mut()
                .insert("content-type", "application/json");
            response
        })
}

/// One hand-authored event-stream operation: the whole request-to-response step,
/// because the generated `deserialize -> validate -> handle -> serialize` sequence
/// cannot carry an event-stream body.
pub trait EventStreamOperation: Send + Sync {
    /// Serve the request. `svc` carries this service's wire protocol, which every
    /// helper above is driven through.
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response>;
}

/// The object-safe wrapper `Service::override_operation` returns for an
/// [`EventStreamOperation`].
///
/// It exists because the generated `impl ErasedOperation` for a `@streaming`-output
/// operation declines the request, and a type can carry only one impl of that
/// trait: this wrapper is a distinct type, so the operation's own implementation is
/// reachable without touching generated code.
pub struct EventStreamRoute<T>(pub T);

impl<T: EventStreamOperation + 'static> ErasedOperation for EventStreamRoute<T> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        self.0.serve(svc, req, ctx)
    }

    // No override: `None` defers to the auth schemes the model gives the operation
    // the request's routing key names, which is this same operation.
}

#[cfg(test)]
mod tests {
    //! The one half of this module with no protocol in it: splitting a request body
    //! into the frames a client framed into it, and the two refusals that reading
    //! can earn.

    use super::*;

    /// One `event` frame, as an AWS SDK's own encoder writes it.
    fn frame(event_type: &str, payload: &[u8]) -> Vec<u8> {
        let message = Message::new(payload.to_vec())
            .add_header(Header::new(
                ":message-type",
                HeaderValue::String("event".into()),
            ))
            .add_header(Header::new(
                ":event-type",
                HeaderValue::String(event_type.to_string().into()),
            ));
        let mut framed = Vec::new();
        write_message_to(&message, &mut framed).expect("an in-memory writer cannot fail");
        framed
    }

    /// One frame carrying the `:message-type` given and no `:event-type` header,
    /// which is what a frame a client may not send looks like.
    fn typeless_frame(message_type: &str) -> Vec<u8> {
        let message = Message::new(b"{}".to_vec()).add_header(Header::new(
            ":message-type",
            HeaderValue::String(message_type.to_string().into()),
        ));
        let mut framed = Vec::new();
        write_message_to(&message, &mut framed).expect("an in-memory writer cannot fail");
        framed
    }

    /// RULE: every frame of the body is read, in arrival order, as the member its
    /// `:event-type` names and the payload bytes it carries — and a body that is not
    /// a stream of `event` frames, or that carries more than the budget admits, is
    /// refused rather than partly read.
    #[test]
    fn a_request_stream_is_read_frame_by_frame_under_the_budget_it_was_given() {
        let one = frame("First", b"{\"a\":1}");
        let two = frame("Second", b"{}");

        // (case, body, budget, the frames owed, else the refusal owed)
        type Row<'r> = (&'r str, Vec<u8>, usize, Option<Vec<(&'r str, &'r [u8])>>);
        let rows: Vec<Row> = vec![
            ("an empty body carries no event", Vec::new(), 4, Some(Vec::new())),
            (
                "one frame",
                one.clone(),
                4,
                Some(vec![("First", b"{\"a\":1}".as_slice())]),
            ),
            (
                "two frames, in arrival order",
                [one.clone(), two.clone()].concat(),
                4,
                Some(vec![("First", b"{\"a\":1}".as_slice()), ("Second", b"{}".as_slice())]),
            ),
            (
                "as many frames as the budget admits",
                [one.clone(), two.clone()].concat(),
                2,
                Some(vec![("First", b"{\"a\":1}".as_slice()), ("Second", b"{}".as_slice())]),
            ),
            (
                "one frame more than the budget admits",
                [one.clone(), two.clone()].concat(),
                1,
                None,
            ),
            ("a budget of none at all", one.clone(), 0, None),
            ("a body that is not a message stream", b"not a frame".to_vec(), 4, None),
            (
                "a frame truncated mid-message",
                one.iter().copied().take(one.len() - 1).collect(),
                4,
                None,
            ),
            ("an exception frame, which only a server sends", typeless_frame("exception"), 4, None),
            ("a frame naming no event at all", typeless_frame("event"), 4, None),
        ];

        for (case, body, budget, expected) in rows {
            let actual = decode_input_stream(&body, budget);
            match (expected, actual) {
                (Some(expected), Ok(events)) => {
                    let read: Vec<(&str, &[u8])> = events
                        .iter()
                        .map(|event| (event.event_type.as_str(), event.payload.as_slice()))
                        .collect();
                    assert_eq!(read, expected, "case {case:?}: expected {expected:?}");
                }
                (Some(expected), Err(refusal)) => panic!(
                    "case {case:?}: expected the frames {expected:?}, got the refusal {refusal:?}"
                ),
                (None, Ok(events)) => {
                    panic!("case {case:?}: expected a refusal, got {events:?}")
                }
                (None, Err(refusal)) => assert!(
                    !refusal.clone().stated().is_empty(),
                    "case {case:?}: a refusal states why, got {refusal:?}"
                ),
            }
        }
    }

    /// RULE: a body past the budget is refused as a count the service limits, and
    /// one that cannot be read at all as a malformed stream — two refusals, because a
    /// service answers them with two different modelled errors.
    #[test]
    fn the_two_refusals_of_a_request_stream_are_told_apart() {
        let body = [frame("First", b"{}"), frame("Second", b"{}")].concat();
        let too_many = decode_input_stream(&body, 1).expect_err("the budget admits one frame");
        assert!(
            matches!(too_many, InputStreamRefusal::TooManyEvents(_)),
            "a body past the budget is refused as a count, got {too_many:?}"
        );
        assert!(
            too_many.clone().stated().contains('1'),
            "the refusal carries the budget it was given, got {too_many:?}"
        );

        let malformed = decode_input_stream(b"not a frame at all", 1)
            .expect_err("a body that is not a message stream cannot be read");
        assert!(
            matches!(malformed, InputStreamRefusal::Malformed(_)),
            "an unreadable body is refused as malformed, got {malformed:?}"
        );
    }
}
