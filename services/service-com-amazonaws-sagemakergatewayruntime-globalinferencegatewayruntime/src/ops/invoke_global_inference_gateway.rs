// SPDX-License-Identifier: CC0-1.0

//! @op InvokeGlobalInferenceGateway
//!
//! Buffered inference: the gateway the path names resolves the model the body names, and
//! answers ONE OpenAI completion document computed from this service's own state.
//!
//! The decisions, in the order this service takes them, all of them shared with the
//! streaming sibling so the two routes cannot disagree:
//!
//! 1. the CONTENT TYPE and the BODY, through the one reader both routes use
//!    ([`crate::service::invocation_gate`]): a content type the gateway does not read is
//!    refused 415, and a body that is not one JSON object — or that repeats the top-level
//!    `model` member with different values — is refused 400. Only `model` and `stream` are
//!    read out of the body, and the body itself is never stored.
//! 2. the GATEWAY and the MODEL, resolved against the gateway registry
//!    (`crate::state::…State::resolve`): an id the registry does not hold is
//!    `ResourceNotFoundException` (404) about the gateway, a model no target serves is the
//!    same error about the model, and a model whose targets have no capacity right now is
//!    `ServiceUnavailableException` (503) — the request is not dispatched.
//! 3. the ANSWER, computed by [`crate::state::completion::ServedTurn`] from the resolved
//!    model, this call's position in the store's completion sequence and the bytes that
//!    arrived. NO LANGUAGE MODEL RUNS IN THIS EMULATOR and the completion says so; what is
//!    computed around it — the completion id, the `created` stamp and the token counts — is
//!    real, so two calls of one gateway are two distinct completions and neither is a
//!    canned blob or the request echoed back.
//!
//! A served invocation is recorded in the log this service keeps
//! (`crate::state::InvocationRecord`); a refusal is not, because nothing was dispatched.
//! A `"stream": true` in the body changes nothing here — "this operation returns one JSON
//! document, so a `"stream": true` in the body is overridden" — and is recorded as what
//! the caller asked for.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `InvokeGlobalInferenceGateway`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime/smithy-model/com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime.json")
//! ```
//!
//! Buffered (non-streaming) inference. Returns the full OpenAI completion body. Errors
//! carry their origin in the `Message` prefix. Every typed error's text is in the `Message`
//! member (JSON key `Message`), and starts with one of these prefixes: * `Global Inference
//! Gateway: ` -- the gateway rejected or failed the request itself: validation,
//! authorization, throttling, routing (the gateway or model was not found, or no capacity
//! is available), a gateway limit or time limit, or failing to reach the inference gateway
//! (`Global Inference Gateway: could not reach the inference gateway: `, or `Global
//! Inference Gateway: timed out waiting for the inference gateway`). This includes the 400
//! for an invalid request and the 413 for a request body that is too large. * `inference
//! gateway: ` -- the inference gateway serving the model returned an error status. The
//! message gives that status code and up to 256 bytes of its error message, for example
//! `inference gateway: 503 `. A BadGatewayException can instead read `inference gateway:
//! response ended unexpectedly`, with no status, when the inference gateway's connection
//! failed while it was sending the response. * `inference gateway (): ` -- the same, when
//! the inference gateway reports which of its components produced the error, for example
//! `inference gateway (model server): 502 `. The component is one of: * `config` -- the
//! inference gateway's configuration and proxy layer. * `scheduler` -- the inference
//! gateway's request scheduler. * `model selector` -- the inference gateway's model
//! selector. * `model server` -- the model server. Every error status the inference gateway
//! returns is a typed error, chosen by that status: * 400 and any other 4xx not listed here
//! -- a 400 validation error. * 401 and 403 -- AccessDeniedException (403). * 404 --
//! ResourceNotFoundException. * 408 -- GatewayTimeoutException (504). * 429 --
//! ThrottlingException, with the `Retry-After` header. * 502, and 500, 503 or any other 5xx
//! not listed here -- BadGatewayException (502). A 500 or 503 from the inference gateway
//! comes after the request was dispatched, so it is not modeled as retryable, and the
//! gateway may already have tried another inference gateway. Standard SDK retry modes may
//! still retry a 5xx status; set max attempts to 1 if a repeated inference is unacceptable.
//! * 504 -- GatewayTimeoutException.
//!
//! ## Input
//!
//! * `Accept` (String) — The MIME type you want for the response. It is passed to the model
//!   server.
//! * `Body` (RequestPayloadBlob, len ..10485760) **required** — The inference request body:
//!   one OpenAI JSON object, at most 10 MiB. Its content is passed to the model server
//!   unchanged, except that the request is always sent with `"stream": false`: this operation
//!   returns one JSON document, so a `"stream": true` in the body is overridden (use
//!   InvokeGlobalInferenceGatewayWithResponseStream to stream). A body that is not one JSON
//!   object, or that repeats the top-level `model` key with different values, is rejected
//!   with a 400 validation error. A body larger than 10 MiB is rejected with
//!   PayloadTooLargeException (413). The gateway reads only `model` and `stream`, and does
//!   not store the body.
//! * `ContentType` (RequestContentType, len 1..1024) **required** — The MIME type of the
//!   request body. Pass `application/json` (parameters such as `charset=utf-8` are allowed):
//!   the body is one OpenAI JSON object. The Global Inference Gateway rejects other content
//!   types with a 415 error.
//! * `CustomerIdentityToken` (CustomerIdentityToken) — Per-caller customer identity token (a
//!   JWT), forwarded unchanged to the inference gateway, which validates it. The gateway does
//!   not use it to authorize the request. Include it in the SigV4 signed headers so it cannot
//!   be altered in transit. Redacted from logs, metrics and traces.
//! * `GatewayId` (GatewayId, len 1..64, pattern ^[a-z0-9]+$) **required** — The ID of the
//!   Global Inference Gateway to invoke. IAM policies refer to the gateway by its ARN,
//!   `arn::sagemaker:::global-inference-gateway/`.
//!
//! ## Output
//!
//! * `Body` (ResponsePayloadBlob, len ..10485760) — The model server's OpenAI completion
//!   JSON, returned unchanged. A response larger than 10 MiB fails with BadGatewayException
//!   (502).
//!
//! ## Errors
//!
//! * `AccessDeniedException` (HTTP 403) — The caller is not authorized to invoke this
//!   gateway. Invoking requires the `sagemaker:InvokeGlobalInferenceGateway` permission, or
//!   `sagemaker:InvokeGlobalInferenceGatewayWithResponseStream` for the streaming operation,
//!   on the gateway ARN. A message starting `inference gateway` means the inference gateway
//!   serving the model refused the request with a 401 or 403, for example for its customer
//!   identity token. Its status code is in the message (the response status is 403), and up
//!   to 256 bytes of its error message are quoted.
//! * `BadGatewayException` (HTTP 502) — 502 -- The gateway could not get a usable response
//!   from the inference gateway. The `Message` prefix says why: * `Global Inference Gateway:
//!   could not reach the inference gateway: ` -- the request could not be sent, or the
//!   connection closed before a response arrived (for example, the connection failed, or the
//!   inference gateway closed it after reading the request). * `Global Inference Gateway:
//!   timed out waiting for the inference gateway` -- on the buffered operation, the inference
//!   gateway stopped sending partway through the response body and the gateway's read timeout
//!   expired. (The same text on a GatewayTimeoutException means no response arrived.) * Any
//!   other `Global Inference Gateway: ` …
//! * `GatewayTimeoutException` (HTTP 504) — 504 -- The inference gateway did not respond in
//!   time. The `Message` prefix says whose time limit expired: * `Global Inference Gateway:
//!   timed out waiting for the inference gateway` -- the gateway's limit on the wait for the
//!   first part of the response. * `inference gateway: ` -- the inference gateway serving the
//!   model returned a 504, or a 408 because it timed out waiting for the request to arrive.
//!   Its status code is in the message (the response status is 504), and up to 256 bytes of
//!   its error message are quoted. The inference gateway names a component in the prefix when
//!   it reports one: `inference gateway (): `, where the component is `config` (the inference
//!   gateway's configuration and proxy …
//! * `InternalServerException` (HTTP 500) — An unexpected internal failure occurred.
//! * `PayloadTooLargeException` (HTTP 413) — The request body exceeds the 10 MiB limit.
//!   Reduce the size of the payload and send the request again.
//! * `ResourceNotFoundException` (HTTP 404) — No gateway with this ID exists in this account
//!   and Region, or no target of the gateway serves the model named in the request body. A
//!   model that is served but has no available target right now returns
//!   ServiceUnavailableException instead. A message starting `inference gateway` means the
//!   inference gateway serving the model returned a 404; up to 256 bytes of its error message
//!   are quoted.
//! * `ServiceUnavailableException` (HTTP 503) — The service is temporarily unable to serve
//!   the request, for example because no capacity is available for the requested model. The
//!   request was not dispatched for inference. A 503 returned by the inference gateway
//!   serving the model after dispatch is reported as BadGatewayException, with the 503 in its
//!   `Message`, not as this error.
//! * `ThrottlingException` (HTTP 429) — The request was throttled before it was dispatched
//!   for inference. Retry the request with backoff, after the `Retry-After` seconds when the
//!   response carries one. A message starting `inference gateway` means the last inference
//!   gateway tried for the model returned a 429. The gateway may have tried others first, and
//!   they may have failed for other reasons. Up to 256 bytes of its error message are quoted.

#[allow(unused_imports)]
use super::types::*;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `InvokeGlobalInferenceGatewayError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakergatewayruntime-globalinferencegatewayruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `InvokeGlobalInferenceGatewayError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemakergatewayruntime#InvokeGlobalInferenceGateway")]
#[derive(Debug)]
pub enum InvokeGlobalInferenceGatewayErrorModeled {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    GatewayTimeoutException(GatewayTimeoutException),
    InternalServerException(InternalServerException),
    PayloadTooLargeException(PayloadTooLargeException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceUnavailableException(ServiceUnavailableException),
    ThrottlingException(ThrottlingException),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::invoke_global_inference_gateway::InvokeGlobalInferenceGatewayError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type InvokeGlobalInferenceGatewayError = crate::scaffolded::Scaffolded<InvokeGlobalInferenceGatewayErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<InvokeGlobalInferenceGatewayErrorModeled> for InvokeGlobalInferenceGatewayError {
    fn from(modeled: InvokeGlobalInferenceGatewayErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `InvokeGlobalInferenceGateway` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct InvokeGlobalInferenceGateway<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeGlobalInferenceGateway<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for InvokeGlobalInferenceGateway<Eng>
{
    type Input = InvokeGlobalInferenceGatewayInput;
    type Output = InvokeGlobalInferenceGatewayOutput;
    type Error = InvokeGlobalInferenceGatewayError;

    const NAME: &'static str = "InvokeGlobalInferenceGateway";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_GLOBAL_INFERENCE_GATEWAY_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_GLOBAL_INFERENCE_GATEWAY_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 200u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The whole decision — the content-type gate, the body read, the registry
        // resolution, the completion sequence and the invocation log — is the one both
        // routes of this service take, so it is taken in one place.
        let turn = crate::service::invocation_gate::serve(
            &*self.engine,
            ctx,
            &crate::service::invocation_gate::Invocation {
                gateway_id: &input.gateway_id,
                content_type: &input.content_type,
                accept: input.accept.as_deref(),
                identity_token_present: input.customer_identity_token.is_some(),
                body: &input.body,
                kind: crate::state::InvocationKind::Buffered,
            },
        )
        .map_err(refusal)?;
        // "The model server's OpenAI completion JSON": one document, whatever the body's
        // `stream` asked for.
        Ok(InvokeGlobalInferenceGatewayOutput {
            body: Some(turn.response_body()),
        })
    }
}

/// Lift a refusal the shared reader and resolver produced into this operation's error
/// channel, each on the status the model gives it.
fn refusal(refused: crate::service::invocation_gate::Refused) -> InvokeGlobalInferenceGatewayError {
    use crate::service::invocation_gate::{Refused, GATEWAY_RESOURCE_TYPE, MODEL_RESOURCE_TYPE};

    match refused {
        // The model declares no 415 error shape while documenting the 415 answer, so the
        // refusal travels on the unmodelled channel (`crate::scaffolded`) with that status.
        Refused::UnsupportedMediaType(message) => {
            crate::scaffolded::Scaffolded::unsupported_media_type(message)
        }
        // The model declares no client-input error either: "a 400 validation error" is the
        // documented answer for a body the gateway cannot read.
        Refused::Invalid(message) => crate::scaffolded::Scaffolded::validation(message),
        Refused::NoSuchGateway {
            message,
            gateway_id,
        } => InvokeGlobalInferenceGatewayErrorModeled::ResourceNotFoundException(
            ResourceNotFoundException {
                message,
                resource_id: Some(gateway_id),
                resource_type: Some(GATEWAY_RESOURCE_TYPE.to_string()),
            },
        )
        .into(),
        Refused::NoSuchModel { message, model } => {
            InvokeGlobalInferenceGatewayErrorModeled::ResourceNotFoundException(
                ResourceNotFoundException {
                    message,
                    resource_id: Some(model),
                    resource_type: Some(MODEL_RESOURCE_TYPE.to_string()),
                },
            )
            .into()
        }
        Refused::NoCapacity(message) => {
            InvokeGlobalInferenceGatewayErrorModeled::ServiceUnavailableException(
                ServiceUnavailableException { message },
            )
            .into()
        }
        Refused::NoState(message) => {
            InvokeGlobalInferenceGatewayErrorModeled::InternalServerException(
                InternalServerException { message },
            )
            .into()
        }
    }
}
