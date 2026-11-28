// SPDX-License-Identifier: CC0-1.0

//! @op InvokeGlobalInferenceGatewayWithResponseStream
//!
//! Streaming inference: the same gateway, the same registry lookup and the same body read
//! as the buffered sibling, answered as an `application/vnd.amazon.eventstream` body whose
//! frames carry the completion's chunks.
//!
//! HAND-AUTHORED. The scaffold op-stub generator declined this operation (see
//! `src/ops/SKIPPED_STUBS.md`): its output binds a `@streaming` event-stream union
//! (`ResponseStream`), which the generator does not type and whose generated
//! `ErasedOperation::run` answers the framework's "modeled but not served" signal. So this
//! file carries TWO types, and the split is the whole reason it looks unusual:
//!
//! * [`InvokeGlobalInferenceGatewayWithResponseStream`] is the struct the GENERATED
//!   routing table holds. Its `ErasedOperation` impl is emitted by `#[smithy_service]`,
//!   declines, and is never reached — it exists so the model's route (`POST
//!   /global-inference-gateway/{GatewayId}/invocations-response-stream`) is in the
//!   service's route table and so the operation's modelled auth schemes are the ones the
//!   router verifies.
//! * [`ResponseStreamInvocation`] is the real implementation. It implements the shared
//!   `service_common::event_stream::EventStreamOperation` trait and `service.rs` holds one
//!   inside that module's `EventStreamRoute` adapter, returned from
//!   `Service::override_operation` — the documented seam for "serve a request the model's
//!   routing key cannot name correctly". Reading the request, framing an event and
//!   rendering a modelled error are that module's helpers, not local copies.
//!
//! # What the stream carries
//!
//! One `PayloadPart` frame per chunk of the ONE turn
//! [`crate::service::invocation_gate::serve`] computed, in the OpenAI
//! `chat.completion.chunk` envelope: the chunk that names the role, the increments of the
//! text, and the terminal chunk carrying the finish reason and the turn's usage. A client
//! that concatenates the `delta.content` of the chunks reconstructs exactly the text the
//! buffered operation returns in one document, so the two routes deliver the same answer.
//! NO LANGUAGE MODEL RUNS IN THIS EMULATOR and that text says so.
//!
//! The body opens with the `initial-response` frame the shared seam writes for an output
//! whose only member is the `@streaming` payload, which is the frame a client reads the
//! output shape itself out of.
//!
//! ## Input
//!
//! The same members the buffered operation binds: `GatewayId` (@httpLabel, required),
//! `ContentType` (@httpHeader(Content-Type), required), `Accept` (@httpHeader(Accept)),
//! `CustomerIdentityToken` (@httpHeader(X-Amzn-SageMaker-Customer-Identity-Token)) and
//! `Body` (blob, @httpPayload, required, at most 10 MiB).
//!
//! ## Output
//!
//! * `Body` (ResponseStream, `@streaming`, @httpPayload) — the event stream. Its union
//!   members are `PayloadPart` (whose `Bytes` is the `@eventPayload`), `ModelStreamError`
//!   and `InternalStreamFailure` (`smithy_query(query="shape", shape="ResponseStream")`).
//!
//! ## Errors
//!
//! The buffered operation's eight (`AccessDeniedException` 403, `BadGatewayException` 502,
//! `GatewayTimeoutException` 504, `InternalServerException` 500,
//! `PayloadTooLargeException` 413, `ResourceNotFoundException` 404,
//! `ServiceUnavailableException` 503, `ThrottlingException` 429) plus the two of the
//! stream itself: `InternalStreamFailure` (500) and `ModelStreamError` (400).

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use service_common::event_stream::{self, EventStreamOperation};
use smithy_server_runtime::{BoxFuture, OperationContext, Service};

smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime.json");

/// The `ResponseStream` union member one completion chunk travels as, which the frame
/// carries in its `:event-type` header (`smithy_query(query="shape",
/// shape="ResponseStream")`).
const PAYLOAD_PART_EVENT: &str = "PayloadPart";

/// The media type one chunk's bytes are in: `PayloadPart$Bytes` is the event's
/// `@eventPayload`, so the chunk document IS the frame's body, and the body as a whole
/// travels as the event-stream media type.
const CHUNK_CONTENT_TYPE: &str = "application/json";

/// This operation's MODELED errors, declared here because the generator emitted no module
/// for the declined operation. The `#[operation_error]` macro accepts only variants the
/// model declares, and it is what gives each one its wire shape name and HTTP status, so
/// the shared renderer answers them exactly as it answers a generated operation's.
#[smithy_macros::operation_error(
    "com.amazonaws.sagemakergatewayruntime#InvokeGlobalInferenceGatewayWithResponseStream"
)]
#[derive(Debug)]
pub enum InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    GatewayTimeoutException(GatewayTimeoutException),
    InternalServerException(InternalServerException),
    InternalStreamFailure(InternalStreamFailure),
    ModelStreamError(ModelStreamError),
    PayloadTooLargeException(PayloadTooLargeException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the shared renderer takes it: the modelled enum plus
/// the unmodelled refusals this service's model documents without declaring a shape for
/// them — the 400 for a body the gateway cannot read and the 415 for a content type it
/// does not read (`crate::scaffolded`).
pub type InvokeGlobalInferenceGatewayWithResponseStreamError =
    crate::scaffolded::Scaffolded<InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled>;

/// Lets `.into()` lift a modelled failure into the error channel.
impl From<InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled>
    for InvokeGlobalInferenceGatewayWithResponseStreamError
{
    fn from(modeled: InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The request this operation reads, declared here because the generator emitted no types
/// module entry for the declined operation. Deserialization and constraint validation then
/// run through the framework's own codec, exactly as a generated operation's do.
#[smithy_macros::smithy(
    shape = "com.amazonaws.sagemakergatewayruntime#InvokeGlobalInferenceGatewayWithResponseStreamInput"
)]
#[derive(Debug, Clone, Default)]
pub struct InvokeGlobalInferenceGatewayWithResponseStreamInput {
    pub gateway_id: String,
    pub accept: Option<String>,
    pub content_type: String,
    pub customer_identity_token: Option<String>,
    pub body: Vec<u8>,
}

/// The `ResponseStream` union's `ModelStreamError` event, declared here because the
/// generator emitted no types-module entry for the declined operation. It is also one of
/// this operation's modelled ERRORS, which is the form it is answered in: an error raised
/// before the stream starts is the response, not a frame in it.
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakergatewayruntime#ModelStreamError")]
#[derive(Debug, Clone, Default)]
pub struct ModelStreamError {
    pub message: String,
    pub error_code: Option<String>,
}

/// The `ResponseStream` union's `InternalStreamFailure` event, declared here for the same
/// reason as [`ModelStreamError`].
#[smithy_macros::smithy(shape = "com.amazonaws.sagemakergatewayruntime#InternalStreamFailure")]
#[derive(Debug, Clone, Default)]
pub struct InternalStreamFailure {
    pub message: String,
}

/// The generated routing-table entry for `InvokeGlobalInferenceGatewayWithResponseStream`.
///
/// Carries the engine so the generated `{Op}::new(engine.clone())` construction compiles;
/// its generated `run` declines (the generator does not serve a `@streaming` output), and
/// `service.rs` routes the request to [`ResponseStreamInvocation`] before the table is
/// consulted.
pub struct InvokeGlobalInferenceGatewayWithResponseStream<Eng> {
    #[allow(dead_code)]
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeGlobalInferenceGatewayWithResponseStream<Eng> {
    /// Construct the operation from the shared engine, as the generated
    /// `ServiceDefinition::new` does for every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The hand-authored `InvokeGlobalInferenceGatewayWithResponseStream` implementation.
pub struct ResponseStreamInvocation<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ResponseStreamInvocation<Eng> {
    /// Construct from the shared engine — the same constructor seam every operation uses.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> ResponseStreamInvocation<Eng> {
    /// Whether `req` is the request this operation serves: the model's method and URI shape
    /// (`POST /global-inference-gateway/{GatewayId}/invocations-response-stream`). Matched
    /// on the whole path, segment by segment — not on a suffix — so a path the model does
    /// not publish is left to the framework's own routing failure. Read by `service.rs`'s
    /// `override_operation`.
    pub fn claims(req: &RtRequest) -> bool {
        if !req.method().eq_ignore_ascii_case("POST") {
            return false;
        }
        // `override_operation` sees the URI as a string that may be absolute, which is the
        // form an AWS SDK sends; `service_common::uri_path` is the ONE reader of that in
        // this workspace, and `nth_path_segment` compares segments as written (an empty
        // one is `Some("")`, so it cannot disappear).
        let path = ::service_common::uri_path(req.uri());
        let segment = |index: usize| ::service_common::nth_path_segment(path, index);
        segment(0) == Some(crate::service::GATEWAY_PATH_SEGMENT)
            && segment(1).is_some_and(|gateway_id| !gateway_id.is_empty())
            && segment(2) == Some("invocations-response-stream")
            && segment(3).is_none()
    }

    /// Resolve and answer. Reading the request into the modelled input shape and checking
    /// it against the model is the shared event-stream path
    /// (`service_common::event_stream::deserialize_request`), so a hand-authored operation
    /// reads a request exactly as a generated one does.
    fn serve(&self, svc: &dyn Service, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let protocol = svc.definition().protocol();

        let input: InvokeGlobalInferenceGatewayWithResponseStreamInput =
            match event_stream::deserialize_request(
                protocol,
                req,
                INVOKE_GLOBAL_INFERENCE_GATEWAY_WITH_RESPONSE_STREAM_INPUT_SCHEMA,
            ) {
                Ok(input) => input,
                // A request the MODEL refuses — an absent required member, a `GatewayId`
                // outside its pattern or length — is the 400 the framework's own boundary
                // answers for the buffered sibling, worded from the same violations.
                Err(message) => {
                    return self.render(svc, crate::scaffolded::Scaffolded::validation(message))
                }
            };

        // The whole decision is the one the buffered route takes, in one place: the
        // content-type gate, the body read, the registry resolution, the completion
        // sequence and the invocation log.
        let turn = match crate::service::invocation_gate::serve(
            &*self.engine,
            ctx,
            &crate::service::invocation_gate::Invocation {
                gateway_id: &input.gateway_id,
                content_type: &input.content_type,
                accept: input.accept.as_deref(),
                identity_token_present: input.customer_identity_token.is_some(),
                body: &input.body,
                kind: crate::state::InvocationKind::Streamed,
            },
        ) {
            Ok(turn) => turn,
            Err(refused) => return self.render(svc, refusal(refused)),
        };

        match self.frames(protocol, &turn) {
            Ok(frames) => event_stream::event_stream_only_response(frames),
            // Unreachable for the documents above — each a few hundred bytes, far inside
            // the framing's own limits — and a handler must not panic, so a body that
            // could not be framed is the modelled stream failure rather than a
            // half-written stream.
            Err(message) => self.render(
                svc,
                InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled::InternalStreamFailure(
                    InternalStreamFailure { message },
                )
                .into(),
            ),
        }
    }

    /// The whole event-stream body: the `initial-response` frame a client reads the output
    /// shape out of, then one `PayloadPart` frame per chunk of the turn.
    fn frames(
        &self,
        protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
        turn: &crate::state::completion::ServedTurn,
    ) -> Result<Vec<u8>, String> {
        let mut frames = event_stream::frame_empty_initial_response(protocol)
            .map_err(|error| format!("the initial response frame could not be encoded: {error}"))?;
        for chunk in turn.stream_chunks() {
            frames.extend(
                event_stream::frame_payload_event(PAYLOAD_PART_EVENT, CHUNK_CONTENT_TYPE, chunk)
                    .map_err(|error| {
                        format!("a completion chunk frame could not be encoded: {error}")
                    })?,
            );
        }
        Ok(frames)
    }

    /// Render anything on this operation's error channel in this service's own envelope,
    /// with the status the model (or, for the two refusals the model declares no shape for,
    /// `crate::scaffolded`) gives it — through the shared event-stream helper, so this
    /// route refuses exactly as the generated route does.
    fn render(
        &self,
        svc: &dyn Service,
        error: InvokeGlobalInferenceGatewayWithResponseStreamError,
    ) -> RtResponse {
        event_stream::error_response(svc.definition().protocol(), &error)
    }
}

/// Lift a refusal the shared reader and resolver produced into this operation's error
/// channel, each on the status the model gives it — the same mapping the buffered route
/// applies, over the same refusals.
fn refusal(
    refused: crate::service::invocation_gate::Refused,
) -> InvokeGlobalInferenceGatewayWithResponseStreamError {
    use crate::service::invocation_gate::{Refused, GATEWAY_RESOURCE_TYPE, MODEL_RESOURCE_TYPE};

    match refused {
        Refused::UnsupportedMediaType(message) => {
            crate::scaffolded::Scaffolded::unsupported_media_type(message)
        }
        Refused::Invalid(message) => crate::scaffolded::Scaffolded::validation(message),
        Refused::NoSuchGateway {
            message,
            gateway_id,
        } => InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message,
                resource_id: Some(gateway_id),
                resource_type: Some(GATEWAY_RESOURCE_TYPE.to_string()),
            },
        )
        .into(),
        Refused::NoSuchModel { message, model } => {
            InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message,
                    resource_id: Some(model),
                    resource_type: Some(MODEL_RESOURCE_TYPE.to_string()),
                },
            )
            .into()
        }
        Refused::NoCapacity(message) => {
            InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException { message },
            )
            .into()
        }
        Refused::NoState(message) => {
            InvokeGlobalInferenceGatewayWithResponseStreamErrorModeled::InternalServerException(
                InternalServerException { message },
            )
            .into()
        }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> EventStreamOperation
    for ResponseStreamInvocation<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        ::std::boxed::Box::pin(async move { ResponseStreamInvocation::serve(self, svc, req, ctx) })
    }
}
