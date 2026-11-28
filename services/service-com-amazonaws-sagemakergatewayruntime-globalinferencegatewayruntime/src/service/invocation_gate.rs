// SPDX-License-Identifier: CC0-1.0

//! How this service reads an invocation request, and how it words a refusal of one —
//! stated ONCE here and read by both operations, which bind the same input members and
//! are documented with the same rules.
//!
//! # The order a request is decided in
//!
//! 1. the CONTENT TYPE the request declared ([`unsupported_content_type`]): the gateway
//!    reads one OpenAI JSON object, so a body announced as anything else is refused before
//!    it is read at all, and before the gateway the path names is looked up — a request the
//!    gateway cannot read is refused whatever gateway it addressed;
//! 2. the BODY ([`read_body`]): one JSON object, out of which only `model` and `stream` are
//!    read;
//! 3. the GATEWAY and the MODEL, resolved against the registry
//!    (`crate::state::ComAmazonawsSagemakergatewayruntimeGlobalinferencegatewayruntimeState::resolve`).
//!
//! # How a refusal is worded
//!
//! Every error of this service carries its text in `Message`, and the model states what
//! that text opens with: "`Global Inference Gateway: ` -- the gateway rejected or failed
//! the request itself: validation, authorization, throttling, routing (the gateway or
//! model was not found, or no capacity is available) … This includes the 400 for an
//! invalid request". So every sentence this module words carries that prefix
//! ([`GATEWAY_PREFIX`]); the other prefix the model documents, `inference gateway: `,
//! belongs to a status an inference gateway returned, and nothing here speaks for one.

use std::fmt;

/// The prefix the model gives every message the gateway itself states.
pub const GATEWAY_PREFIX: &str = "Global Inference Gateway: ";

/// The `ResourceNotFoundException$ResourceType` of a gateway the registry does not hold —
/// "the kind of resource that was not found: the gateway, or the model"
/// (`smithy_query(query="shape", shape="ResourceNotFoundException")`).
pub const GATEWAY_RESOURCE_TYPE: &str = "gateway";

/// The `ResourceNotFoundException$ResourceType` of a model no target of the gateway serves.
pub const MODEL_RESOURCE_TYPE: &str = "model";

/// The members of the request body this service reads. "The gateway reads only `model` and
/// `stream`, and does not store the body."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadBody {
    /// The top-level `model` member: the model the request is routed to.
    pub model: String,
    /// The top-level `stream` member, absent meaning `false`. The buffered operation
    /// returns one JSON document whatever it says — "the request is always sent with
    /// `"stream": false`" — so it is what the caller asked for, not how the answer is
    /// framed.
    pub stream: bool,
}

/// Why the content type a request declared is not one this gateway reads, or `None` when
/// it is.
///
/// "Pass `application/json` (parameters such as `charset=utf-8` are allowed): the body is
/// one OpenAI JSON object. The Global Inference Gateway rejects other content types with a
/// 415 error." So the declared type is parsed as the media type it is — by the maintained
/// media-type crate, not by string surgery — and only its essence is compared, which is
/// what admits the parameters the documentation allows and rejects everything else.
#[must_use]
pub fn unsupported_content_type(content_type: &str) -> Option<String> {
    let accepted = content_type
        .parse::<::mime::Mime>()
        .is_ok_and(|declared| declared.essence_str() == ::mime::APPLICATION_JSON.essence_str());
    if accepted {
        return None;
    }
    Some(format!(
        "{GATEWAY_PREFIX}the content type {content_type} is not supported. Send the \
         inference request body as {}.",
        ::mime::APPLICATION_JSON.essence_str()
    ))
}

/// Read `body` as the one JSON object the gateway reads `model` and `stream` out of, or the
/// sentence a body it cannot read is refused with.
///
/// "A body that is not one JSON object, or that repeats the top-level `model` key with
/// different values, is rejected with a 400 validation error." Both are refused here: the
/// members are walked as they ARRIVE, so a repeated `model` is seen rather than silently
/// overwritten by the last occurrence, and trailing content after the object makes the
/// body more than one object.
pub fn read_body(body: &[u8]) -> Result<ReadBody, String> {
    let read: BodyMembers = serde_json::from_slice(body).map_err(|_| not_one_object())?;
    if read.repeated_model {
        return Err(format!(
            "{GATEWAY_PREFIX}the request body repeats the top-level \"model\" member with \
             different values."
        ));
    }
    let model = match read.model {
        None => {
            return Err(format!(
                "{GATEWAY_PREFIX}the request body must name the model to invoke in its \
                 top-level \"model\" member."
            ))
        }
        Some(serde_json::Value::String(model)) => model,
        Some(_) => {
            return Err(format!(
                "{GATEWAY_PREFIX}the top-level \"model\" member of the request body must be a \
                 string."
            ))
        }
    };
    let stream = match read.stream {
        None | Some(serde_json::Value::Null) => false,
        Some(serde_json::Value::Bool(stream)) => stream,
        Some(_) => {
            return Err(format!(
                "{GATEWAY_PREFIX}the top-level \"stream\" member of the request body must be a \
                 boolean."
            ))
        }
    };
    Ok(ReadBody { model, stream })
}

/// The sentence a body that is not one JSON object is refused with.
fn not_one_object() -> String {
    format!("{GATEWAY_PREFIX}the request body must be one JSON object.")
}

/// The sentence a gateway id the registry does not hold is refused with.
#[must_use]
pub fn no_such_gateway(gateway_id: &str) -> String {
    format!("{GATEWAY_PREFIX}no gateway with ID {gateway_id} exists in this account and Region.")
}

/// The sentence a model no target of the gateway serves is refused with.
#[must_use]
pub fn no_such_model(gateway_id: &str, model: &str) -> String {
    format!("{GATEWAY_PREFIX}no target of gateway {gateway_id} serves the model {model}.")
}

/// The sentence a model whose targets have no capacity right now is refused with. "The
/// request was not dispatched for inference."
#[must_use]
pub fn no_capacity(gateway_id: &str, model: &str) -> String {
    format!(
        "{GATEWAY_PREFIX}no capacity is available for the model {model} on gateway \
         {gateway_id}. The request was not dispatched for inference."
    )
}

/// One invocation request, as both operations hand it to [`serve`]: the members the model
/// binds, and which operation is serving it.
#[derive(Debug)]
pub struct Invocation<'i> {
    /// The `{GatewayId}` label of the path.
    pub gateway_id: &'i str,
    /// The `Content-Type` header the request declared.
    pub content_type: &'i str,
    /// The `Accept` header, which the model says is passed to the model server.
    pub accept: Option<&'i str>,
    /// Whether the request carried a customer identity token. The token itself is
    /// `@sensitive` and "Redacted from logs, metrics and traces", so only its PRESENCE
    /// travels past the handler.
    pub identity_token_present: bool,
    /// The `@httpPayload` body.
    pub body: &'i [u8],
    /// Which operation is serving the request.
    pub kind: crate::state::InvocationKind,
}

/// Why an invocation was refused, with the sentence it is refused in.
///
/// Each operation declares its own error enum (the model gives the streaming one two
/// errors the buffered one does not have), so this names the refusal and each route lifts
/// it into its own modelled variant — which is what keeps the two routes refusing the same
/// request in the same words with the same status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refused {
    /// The content type is not one this gateway reads: HTTP 415.
    UnsupportedMediaType(String),
    /// The body is not one JSON object the gateway can route: HTTP 400.
    Invalid(String),
    /// The registry holds no gateway with this id: `ResourceNotFoundException` (404),
    /// carrying the gateway id as the resource that was not found.
    NoSuchGateway { message: String, gateway_id: String },
    /// No target of the gateway serves this model: `ResourceNotFoundException` (404),
    /// carrying the model as the resource that was not found.
    NoSuchModel { message: String, model: String },
    /// The model is served and no target has capacity: `ServiceUnavailableException` (503).
    NoCapacity(String),
    /// This service is mounted on an engine that holds no store for it:
    /// `InternalServerException` (500).
    NoState(String),
}

/// Serve one invocation: read it, resolve it against the gateway registry, and compute the
/// turn the answer carries — or refuse it.
///
/// This is the WHOLE decision both operations take; they differ only in how they frame the
/// turn ([`crate::state::completion::ServedTurn::response_body`] against
/// [`crate::state::completion::ServedTurn::stream_chunks`]) and in the modelled variant
/// each refusal is lifted into.
///
/// A SERVED invocation takes the next position in the store's completion sequence and is
/// recorded in the invocation log; a refused one takes neither, because the model says a
/// refusal of this kind means "the request was not dispatched for inference", so nothing
/// was served to log.
pub fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    call: &Invocation<'_>,
) -> Result<crate::state::completion::ServedTurn, Refused> {
    if let Some(message) = unsupported_content_type(call.content_type) {
        return Err(Refused::UnsupportedMediaType(message));
    }
    let read = read_body(call.body).map_err(Refused::Invalid)?;
    let Some(state) = crate::state::service_state(engine) else {
        return Err(Refused::NoState(crate::state::NO_STATE.to_string()));
    };
    let now = ::std::time::SystemTime::now();
    let seq = {
        let mut store = state.lock();
        match store.resolve(call.gateway_id, &read.model) {
            crate::state::Resolution::NoSuchGateway => {
                return Err(Refused::NoSuchGateway {
                    message: no_such_gateway(call.gateway_id),
                    gateway_id: call.gateway_id.to_string(),
                })
            }
            crate::state::Resolution::NoSuchModel => {
                return Err(Refused::NoSuchModel {
                    message: no_such_model(call.gateway_id, &read.model),
                    model: read.model.clone(),
                })
            }
            crate::state::Resolution::NoCapacity => {
                return Err(Refused::NoCapacity(no_capacity(
                    call.gateway_id,
                    &read.model,
                )))
            }
            crate::state::Resolution::Served => {
                let seq = store.next_completion_seq();
                store.record_invocation(crate::state::InvocationRecord {
                    seq,
                    gateway_id: call.gateway_id.to_string(),
                    model: read.model.clone(),
                    request_bytes: call.body.len(),
                    content_type: call.content_type.to_string(),
                    accept: call.accept.map(str::to_string),
                    identity_token_present: call.identity_token_present,
                    streaming: call.kind.streaming(),
                    stream_requested: read.stream,
                    at: now,
                });
                seq
            }
        }
    };
    ::tracing::info!(
        operation = call.kind.operation(),
        gateway = call.gateway_id,
        model = %read.model,
        request_bytes = call.body.len(),
        streaming = call.kind.streaming(),
        seq,
        access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
        "global inference gateway invocation"
    );
    Ok(crate::state::completion::ServedTurn::new(
        call.gateway_id,
        &read.model,
        seq,
        call.body,
        now,
    ))
}

/// The two members [`read_body`] reads, plus whether `model` arrived more than once with
/// different values.
///
/// Read through a map visitor rather than into a `serde_json::Map`, because a map keeps
/// ONE value per key: the repeated-`model` rule is about what the body CARRIES, which is
/// only visible while the members are still arriving.
#[derive(Debug, Default)]
struct BodyMembers {
    model: Option<serde_json::Value>,
    stream: Option<serde_json::Value>,
    repeated_model: bool,
}

impl<'de> serde::Deserialize<'de> for BodyMembers {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Reads the top-level members of the body, in arrival order.
        struct Members;

        impl<'v> serde::de::Visitor<'v> for Members {
            type Value = BodyMembers;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("one JSON object")
            }

            fn visit_map<A: serde::de::MapAccess<'v>>(
                self,
                mut members: A,
            ) -> Result<BodyMembers, A::Error> {
                let mut read = BodyMembers::default();
                while let Some(member) = members.next_key::<String>()? {
                    let value: serde_json::Value = members.next_value()?;
                    match member.as_str() {
                        "model" => {
                            if read.model.as_ref().is_some_and(|held| *held != value) {
                                read.repeated_model = true;
                            }
                            read.model = Some(value);
                        }
                        "stream" => read.stream = Some(value),
                        // Every other member of an OpenAI request is the model server's
                        // business; the gateway reads none of it.
                        _ => {}
                    }
                }
                Ok(read)
            }
        }

        deserializer.deserialize_map(Members)
    }
}
