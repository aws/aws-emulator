// SPDX-License-Identifier: CC0-1.0

//! @op InvokeModelWithResponseStream
//!
//! `POST /model/{modelId}/invoke-with-response-stream -> 200`. Serves the same
//! inference `InvokeModel` serves — the same model resolution, the same body and
//! guardrail validation, the same tokenizer and the same generated turn, all from
//! [`crate::state`] — and returns it as a `vnd.amazon.eventstream` body of
//! `ResponseStream` `chunk` events, with the MIME type of the payload those chunks
//! carry reported in the modelled, required `X-Amzn-Bedrock-Content-Type` response
//! header.
//!
//! Each `chunk` carries ONE event of the invoked model family's own streaming
//! protocol — not the unary `InvokeModel` response body, which belongs to that
//! operation and never to a `ResponseStream` chunk. The sequences are per family
//! (see [`crate::state::invoke_model_stream_chunks`]), and the terminal chunk
//! additionally carries the per-invocation accounting the service itself measures
//! ([`crate::state::INVOCATION_METRICS_FIELD`]).
//!
//! No language model runs in this emulator, so the completion is produced in one
//! step (see [`crate::state::emulated_completion_text`]) and therefore arrives as
//! one content delta; nothing here splits it into fabricated increments, and
//! nothing is echoed back from the request.
//!
//! This operation is hand-authored rather than generated: its output is a
//! `@streaming` union, which the op-stub generator declines
//! (`src/ops/SKIPPED_STUBS.md`) and whose generated `ErasedOperation` shim refuses
//! the request. The framing and the routing seam live in `service_common::event_stream`;
//! this file holds the behaviour.
//!
//! ## Input
//!
//! * `modelId` (InvokeModelIdentifier, len 1..2048, `@httpLabel`) **required**
//! * `body` (blob, len ..25000000, `@httpPayload`)
//! * `contentType` (`@httpHeader("Content-Type")`),
//!   `accept` (`@httpHeader("X-Amzn-Bedrock-Accept")`)
//! * `trace` (`@httpHeader("X-Amzn-Bedrock-Trace")`),
//!   `guardrailIdentifier` / `guardrailVersion`
//!   (`@httpHeader("X-Amzn-Bedrock-GuardrailIdentifier"/"-GuardrailVersion")`)
//! * `performanceConfigLatency` (default `standard`), `serviceTier`,
//!   `requestMetadata` (len ..8500)
//!
//! ## Output
//!
//! * `body` (ResponseStream, `@httpPayload`, `@streaming`) **required** — the
//!   event-stream body, whose `chunk` events carry `bytes`.
//! * `contentType` (`@httpHeader("X-Amzn-Bedrock-Content-Type")`) **required**
//! * `performanceConfigLatency`, `serviceTier` — echoed from the request, as
//!   `InvokeModel` echoes them.
//!
//! ## Errors
//!
//! * `ValidationException` (HTTP 400) — a `modelId` that is not a model
//!   identifier, a body that is not `application/json`, an empty body, or one of
//!   the guardrail combinations the operation documents as errors.
//! * `ResourceNotFoundException` (HTTP 404) — the model identifier names no model.
//! * `AccessDeniedException` (403), `InternalServerException` (500),
//!   `ModelErrorException` (424), `ModelNotReadyException` (429),
//!   `ModelStreamErrorException` (424), `ModelTimeoutException` (408),
//!   `ServiceQuotaExceededException` (400), `ServiceUnavailableException` (503),
//!   `ThrottlingException` (429).

#[allow(unused_imports)]
use super::types::*;
use aws_smithy_runtime_api::http::{Request, Response};
use service_common::event_stream::{deserialize_request, error_response, event_stream_response};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};
use std::sync::Arc;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockruntime-amazonbedrockfrontendservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the
/// model declares for it.
#[smithy_macros::operation_error("com.amazonaws.bedrockruntime#InvokeModelWithResponseStream")]
#[derive(Debug)]
pub enum InvokeModelWithResponseStreamErrorModeled {
    AccessDeniedException(AccessDeniedException),
    InternalServerException(InternalServerException),
    ModelErrorException(ModelErrorException),
    ModelNotReadyException(ModelNotReadyException),
    ModelStreamErrorException(ModelStreamErrorException),
    ModelTimeoutException(ModelTimeoutException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

fn validation(message: impl Into<String>) -> InvokeModelWithResponseStreamErrorModeled {
    InvokeModelWithResponseStreamErrorModeled::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

fn internal(message: impl Into<String>) -> InvokeModelWithResponseStreamErrorModeled {
    InvokeModelWithResponseStreamErrorModeled::InternalServerException(InternalServerException {
        message: Some(message.into()),
    })
}

/// The `InvokeModelWithResponseStream` operation.
///
/// Owns the shared engine (the constructor seam). The generated routing table holds
/// one of these and `service.rs` holds another wrapped in an `EventStreamRoute`;
/// both reach the same state, because the state lives in the shared engine, not on
/// the operation.
pub struct InvokeModelWithResponseStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> InvokeModelWithResponseStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> InvokeModelWithResponseStream<Eng> {
    /// The whole request: validate it, serve the inference, record it in the
    /// inference log, and frame the model's response payload as the event stream.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &Request,
    ) -> Result<Response, InvokeModelWithResponseStreamErrorModeled> {
        let started = std::time::Instant::now();
        let input: InvokeModelWithResponseStreamInput =
            deserialize_request(protocol, req, &InvokeModelWithResponseStreamInput::SCHEMA)
                .map_err(validation)?;

        let state = crate::state::service_state(&*self.engine)
            .ok_or_else(|| internal("the Bedrock Runtime emulator has no state store"))?;

        let model = {
            let guard = state.lock();
            match guard.resolve_model_for_inference(&input.model_id) {
                Ok(entry) => entry.clone(),
                // An identifier the service cannot interpret is a 400; one
                // naming a model or resource that is not available here is a
                // 404 (see `crate::state::resolve_model_for_inference`).
                Err(crate::state::ModelRejection::Invalid(message)) => {
                    return Err(validation(message))
                }
                Err(crate::state::ModelRejection::NotFound(message)) => {
                    return Err(
                        InvokeModelWithResponseStreamErrorModeled::ResourceNotFoundException(
                            ResourceNotFoundException {
                                message: Some(message),
                            },
                        ),
                    )
                }
            }
        };

        // A model whose modality this emulator computes nothing for — the same
        // refusal `InvokeModel` makes, for the same reason: nothing ran, so the
        // answer is the modelled model-processing error rather than a fabricated
        // embedding, image or video payload.
        let Some(family) = model.family else {
            return Err(
                InvokeModelWithResponseStreamErrorModeled::ModelErrorException(
                    ModelErrorException {
                        message: Some(format!(
                            "No model runs in this Amazon Bedrock Runtime emulator, so it serves \
                             no {} inference for {}.",
                            model.modality.label(),
                            model.model_id
                        )),
                        original_status_code: None,
                        resource_name: Some(model.model_id.clone()),
                    },
                ),
            );
        };

        // Every refusal this operation makes, in the order it makes them — the
        // same request `InvokeModel` takes, so both read the rule from
        // `crate::state`.
        let body = input.body.as_deref().unwrap_or_default();
        crate::state::validate_invoke_request(
            family,
            input.content_type.as_deref(),
            input.accept.as_deref(),
            body,
            input.guardrail_identifier.as_deref(),
            input.guardrail_version.as_deref(),
        )
        .map_err(validation)?;

        // The input-size stage `InvokeModel` runs, for the same reason: a model
        // reads a bounded prompt, so a body whose counted prompt tokens are past
        // the resolved model's context window is refused rather than streamed
        // against a shortened prompt.
        if crate::state::context_window::exceeds_context_window(
            &model,
            crate::state::body_input_tokens(body),
        ) {
            return Err(validation(
                crate::state::context_window::INPUT_TOKENS_EXCEEDED_MESSAGE.to_string(),
            ));
        }

        // The same inference `InvokeModel` serves, so the two surfaces cannot
        // drift; here the turn is delivered as the model family's own streaming
        // events rather than as the unary response body.
        let turn = crate::state::invoke_turn(&model, family, body);
        // The guardrail stage `InvokeModel` runs, for the same reason: the
        // account-enforced guardrail applies to this invocation whether or not
        // the request asked for one.
        let (turn, guardrail_action) = crate::state::enforced_guardrail::guard_invoke_turn(
            &*self.engine,
            &model,
            family,
            body,
            turn,
        );
        let first_byte_latency_ms = crate::state::latency_ms(started.elapsed());

        let logged = crate::state::InvocationLogEntry {
            model_id: model.model_id.clone(),
            kind: crate::state::InvokeKind::SyncStream,
            input_tokens: turn.input_tokens,
            output_tokens: turn.output_tokens,
            at_millis: crate::state::now_millis(),
        };
        state.lock().log_invocation(logged.clone());
        // The duration this call reports in the family's own metrics event,
        // measured over serving the inference.
        let invocation_latency_ms = crate::state::latency_ms(started.elapsed());
        // The token volume and the latency this call reports in the family's own
        // metrics event are also its contribution to the `AWS/Bedrock` series
        // dimensioned on the invoked model, beside the one successful request it
        // is, published with no state guard of this service held.
        crate::state::invocation_metrics::publish_served_invocation(
            &*self.engine,
            &logged,
            invocation_latency_ms,
        );

        // One `chunk` event per streaming event of the model's own protocol, in
        // the order the family emits them.
        let mut chunks = crate::state::invoke_model_stream_chunks(
            family,
            &model.model_id,
            &turn,
            crate::state::InvocationMetrics {
                input_tokens: turn.input_tokens,
                output_tokens: turn.output_tokens,
                invocation_latency_ms,
                first_byte_latency_ms,
            },
        );
        // The guardrail action this invocation took, reported on the terminal
        // chunk beside the service's own accounting — the one place the service
        // adds its own members to a model's stream.
        if let Some(action) = guardrail_action {
            if let Some(last) = chunks.last_mut() {
                crate::state::enforced_guardrail::report_action(last, action);
            }
        }
        let mut frames = Vec::new();
        for chunk in &chunks {
            let payload = serde_json::to_vec(chunk).map_err(|error| {
                internal(format!("could not render the inference response: {error}"))
            })?;
            let frame = frame_padded_event(
                protocol,
                "chunk",
                &PayloadPart {
                    bytes: Some(payload),
                },
                &PayloadPart::SCHEMA,
            )
            .map_err(|error| internal(format!("the chunk event could not be framed: {error}")))?;
            frames.extend_from_slice(&frame);
        }

        event_stream_response(
            protocol,
            "InvokeModelWithResponseStream",
            &InvokeModelWithResponseStreamHeaders {
                content_type: crate::state::JSON_MIME.to_string(),
                performance_config_latency: Some(input.performance_config_latency.clone()),
                service_tier: input.service_tier.clone(),
            },
            &InvokeModelWithResponseStreamHeaders::SCHEMA,
            frames,
        )
        .map_err(|error| internal(format!("the response could not be serialized: {error}")))
    }
}

impl<Eng: Send + Sync + 'static> service_common::event_stream::EventStreamOperation
    for InvokeModelWithResponseStream<Eng>
{
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let protocol = svc.definition().protocol();
            let response = match self.respond(protocol, req) {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            };
            // One access record per request, so a refused stream is diagnosable.
            // No request payload and no credential reaches it.
            tracing::debug!(
                operation = "InvokeModelWithResponseStream",
                service = ctx.identity().signing_name(),
                method = ctx.parts().method.as_str(),
                status = response.status().as_u16(),
                "served an event-stream request"
            );
            response
        })
    }
}
