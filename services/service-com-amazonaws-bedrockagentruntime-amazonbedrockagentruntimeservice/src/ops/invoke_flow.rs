// SPDX-License-Identifier: CC0-1.0

//! @op InvokeFlow
//!
//! `POST /flows/{flowIdentifier}/aliases/{flowAliasIdentifier} -> 200`. Serves one flow
//! invocation as a `vnd.amazon.eventstream` body.
//!
//! ## Where a failure travels
//!
//! `InvokeFlow` is modelled `@http(code: 200)` and its `FlowResponseStream` carries the
//! service's modelled exceptions as EVENT MEMBERS beside the data events. So the channel a
//! failure travels in follows when it arises:
//!
//! * BEFORE the invocation begins — the request broke the model's own constraints (a
//!   `flowIdentifier` that is neither a flow ARN nor a flow id), or the caller is not
//!   allowed `bedrock:InvokeFlow` on the alias — it is an HTTP error with the status the
//!   model gives it, because no stream has been opened to carry it;
//! * ONCE the invocation has begun, every modelled exception is delivered INSIDE the stream
//!   as the `FlowResponseStream` member of its name, in a 200
//!   `application/vnd.amazon.eventstream` response. That is the only way a flow reports a
//!   mid-execution failure, and a client that opens the stream and iterates events is what
//!   sees it: a flow the Bedrock Agent plane does not hold is a `resourceNotFoundException`
//!   EVENT, not a transport-level 404.
//!
//! ## What is real here
//!
//! The flow, the alias and the version the alias routes to are the Bedrock Agent build-time
//! plane's resources, and `crate::state::flow_invocation` resolves all three there and
//! walks the definition that version packages. NO LANGUAGE MODEL, LAMBDA FUNCTION OR OTHER
//! EXTERNAL WORKER RUNS IN THIS EMULATOR, so the walk computes the path it can — the
//! request's document enters at the flow's input node and reaches the output nodes that node
//! is connected to — and delivers an in-stream `validationException` for a path through any
//! other node kind rather than inventing a value nothing produced. An input that addresses
//! no input node of the flow leaves that node without the input the run depends on, which is
//! the in-stream `dependencyFailedException` that failure class carries.
//!
//! A served invocation emits one `flowOutputEvent` per output node the value reached, then
//! the `flowCompletionEvent` the modelled `FlowCompletionReason` names for a flow that ran
//! to the end. Each one IS an execution, and the 200 reports which in the modelled
//! `executionId` header: the id the request named, else one minted for that invocation
//! ([`execution_id`]). A request that asks for the trace is served, beside those, the
//! `flowTraceEvent`s of the run that really happened — what the input node emitted and what
//! reached each output node, under the field names the stored flow's own connections carry
//! ([`entry_output_traces`], [`node_input_trace`]); nothing traces a node the walk did not
//! take.
//!
//! This operation is hand-authored rather than generated: its output is a `@streaming`
//! union, which the op-stub generator declines (`src/ops/SKIPPED_STUBS.md`) and whose
//! generated `ErasedOperation` shim would decline the request. The framing and the routing
//! seam live in `service_common::event_stream`, the shapes in
//! `super::types::invoke_flow_types`, and `crate::service` routes a request here through
//! `Service::override_operation`.
//!
//! ## Input
//!
//! * `flowIdentifier` (FlowIdentifier, len ..2048, pattern
//!   `^(arn:aws:bedrock:[a-z0-9-]{1,20}:[0-9]{12}:flow/[0-9a-zA-Z]{10})|([0-9a-zA-Z]{10})$`,
//!   `@httpLabel`) **required**.
//! * `flowAliasIdentifier` (FlowAliasIdentifier, len ..2048, `@httpLabel`) **required**.
//! * `inputs` (FlowInputs, len 1..1) **required** — the one input the flow is entered with.
//! * `enableTrace` (Boolean), `modelPerformanceConfiguration`
//!   (ModelPerformanceConfiguration), `executionId` (FlowExecutionId, len 2..100).
//!
//! ## Output
//!
//! * `responseStream` (FlowResponseStream, `@httpPayload`, `@streaming`) **required** — the
//!   body.
//! * `executionId` (`@httpHeader x-amz-bedrock-flow-execution-id`).
//!
//! ## Errors
//!
//! * `AccessDeniedException` (403), `BadGatewayException` (502), `ConflictException` (409),
//!   `DependencyFailedException` (424), `InternalServerException` (500),
//!   `ResourceNotFoundException` (404), `ServiceQuotaExceededException` (400),
//!   `ThrottlingException` (429), `ValidationException` (400) — each one also a member of
//!   `FlowResponseStream`, which is where it is delivered once the invocation has begun.

#[allow(unused_imports)]
use super::types::*;

use aws_smithy_runtime_api::http::{Request, Response};
use aws_smithy_schema_server::ProtocolError;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, read_request, EventStreamOperation,
    RequestRefusal,
};
use smithy_server_runtime::{BoxFuture, OperationContext, Refusal, ServerHttpProtocol, Service};

use crate::state::flow_invocation as flows;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json");

/// The operation's MODELED errors; `#[operation_error]` accepts only variants the model
/// declares for it. No `Scaffolded` wrapper: this operation is implemented, so the 501
/// channel has nothing to carry.
#[smithy_macros::operation_error("com.amazonaws.bedrockagentruntime#InvokeFlow")]
#[derive(Debug)]
pub enum InvokeFlowError {
    AccessDeniedException(AccessDeniedException),
    BadGatewayException(BadGatewayException),
    ConflictException(ConflictException),
    DependencyFailedException(DependencyFailedException),
    InternalServerException(InternalServerException),
    ResourceNotFoundException(ResourceNotFoundException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    ThrottlingException(ThrottlingException),
    ValidationException(ValidationException),
}

/// This operation's modelled 400 `ValidationException`, for a request refused before the
/// invocation begins.
fn validation(message: impl Into<String>) -> InvokeFlowError {
    InvokeFlowError::ValidationException(ValidationException {
        message: Some(message.into()),
    })
}

/// This operation's modelled 500 `InternalServerException`.
fn internal(message: impl Into<String>) -> InvokeFlowError {
    InvokeFlowError::InternalServerException(InternalServerException {
        message: Some(message.into()),
        reason: None,
    })
}

/// The `FlowResponseStream` members a served invocation travels under, which the frames
/// carry as their `:event-type`
/// (`smithy_query(query="shape",shape="FlowResponseStream")`).
const OUTPUT_EVENT: &str = "flowOutputEvent";

/// See [`OUTPUT_EVENT`]: the member the end of a run travels under.
const COMPLETION_EVENT: &str = "flowCompletionEvent";

/// The `FlowResponseStream` members a modelled exception travels under once the invocation
/// has begun, which the frames carry as their `:exception-type`.
const NOT_FOUND_EVENT: &str = "resourceNotFoundException";

/// See [`NOT_FOUND_EVENT`]: the member an input fault of the invocation travels under.
const VALIDATION_EVENT: &str = "validationException";

/// See [`NOT_FOUND_EVENT`]: the member a failure of something the RUN depended on travels
/// under.
const DEPENDENCY_FAILED_EVENT: &str = "dependencyFailedException";

/// See [`NOT_FOUND_EVENT`]: the member a server fault of the invocation travels under.
const INTERNAL_EVENT: &str = "internalServerException";

/// The `FlowCompletionReason` of a flow that ran to the end
/// (`smithy_query(query="shape",shape="FlowCompletionReason")` -> SUCCESS, INPUT_REQUIRED).
const SUCCESS: &str = "SUCCESS";

/// The operation name, which names the IAM action this call is authorized against
/// (`bedrock:InvokeFlow`). Declared here because this operation has no generated
/// `impl Operation` to carry a `NAME`.
const OPERATION_NAME: &str = "InvokeFlow";

/// The literal first segment of this operation's modelled URI.
const FLOWS_SEGMENT: &str = "flows";

/// The literal third segment of that URI, between the two labels.
const ALIASES_SEGMENT: &str = "aliases";

/// The `InvokeFlow` operation.
///
/// Owns the shared engine (the constructor seam). Held by `crate::service` behind an
/// `EventStreamRoute`, because the generated routing table cannot serve a
/// `@streaming`-output operation.
pub struct InvokeFlow<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeFlow<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// Whether `req` addresses this operation's modelled route: `POST` on
/// `/flows/{flowIdentifier}/aliases/{flowAliasIdentifier}`.
///
/// The seam that routes it ([`crate::service::EmulatedService::override_operation`]) asks
/// this, so the path this operation recognises is stated once, beside the operation. The
/// build-time plane binds the same two literal segments with NO alias label
/// (`POST /flows/{flowIdentifier}/aliases/`, its `CreateFlowAlias`), so the route is told
/// apart by carrying one.
pub fn addresses(req: &Request) -> bool {
    if req.method() != "POST" {
        return false;
    }
    let path = service_common::uri_path(req.uri());
    let mut segments = path.trim_matches('/').split('/');
    let (Some(FLOWS_SEGMENT), Some(flow), Some(ALIASES_SEGMENT), Some(alias), None) = (
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
        segments.next(),
    ) else {
        return false;
    };
    !flow.is_empty() && !alias.is_empty()
}

impl<Eng: Send + Sync + 'static> InvokeFlow<Eng> {
    /// The whole request once it has been read and authorized: run the flow and frame what
    /// it answered, or frame the modelled exception the run earned.
    ///
    /// Infallible by construction once the invocation has begun: every outcome of the run
    /// is a frame in the 200 the caller already holds.
    fn respond(
        &self,
        protocol: &dyn ServerHttpProtocol,
        ctx: &OperationContext<'_>,
        input: &InvokeFlowInput,
        execution_id: &str,
    ) -> Response {
        let mut frames = Vec::new();
        match self.run(ctx, input) {
            Ok(outputs) => {
                // A request that asked for the trace is served the trace of the run that
                // really happened, beside its outputs: what the entry node emitted, and
                // what reached each output node it is connected to.
                let traced = input.enable_trace == Some(true);
                if traced {
                    for trace in entry_output_traces(input, &outputs) {
                        if let Err(error) = frame_trace(protocol, &mut frames, trace) {
                            return self.framing_failure(protocol, error);
                        }
                    }
                }
                for output in outputs {
                    if traced {
                        if let Some(trace) = node_input_trace(&output) {
                            if let Err(error) = frame_trace(protocol, &mut frames, trace) {
                                return self.framing_failure(protocol, error);
                            }
                        }
                    }
                    if let Err(error) = frame(
                        protocol,
                        &mut frames,
                        OUTPUT_EVENT,
                        false,
                        &FlowOutputEvent {
                            node_name: output.node_name,
                            content: FlowOutputContent::Document(output.document),
                        },
                        &FlowOutputEvent::SCHEMA,
                    ) {
                        return self.framing_failure(protocol, error);
                    }
                }
                if let Err(error) = frame(
                    protocol,
                    &mut frames,
                    COMPLETION_EVENT,
                    false,
                    &FlowCompletionEvent {
                        completion_reason: SUCCESS.to_string(),
                    },
                    &FlowCompletionEvent::SCHEMA,
                ) {
                    return self.framing_failure(protocol, error);
                }
            }
            Err(InStream::NotFound(message)) => {
                if let Err(error) = frame(
                    protocol,
                    &mut frames,
                    NOT_FOUND_EVENT,
                    true,
                    &ResourceNotFoundException {
                        message: Some(message),
                    },
                    &SCHEMA_RESOURCE_NOT_FOUND_EXCEPTION,
                ) {
                    return self.framing_failure(protocol, error);
                }
            }
            Err(InStream::DependencyFailed(resource_name)) => {
                if let Err(error) = frame(
                    protocol,
                    &mut frames,
                    DEPENDENCY_FAILED_EVENT,
                    true,
                    &DependencyFailedException {
                        message: Some(flows::dependency_failed_message()),
                        resource_name: Some(resource_name),
                    },
                    &SCHEMA_DEPENDENCY_FAILED_EXCEPTION,
                ) {
                    return self.framing_failure(protocol, error);
                }
            }
            Err(InStream::Invalid(message)) => {
                if let Err(error) = frame(
                    protocol,
                    &mut frames,
                    VALIDATION_EVENT,
                    true,
                    &ValidationException {
                        message: Some(message),
                    },
                    &SCHEMA_VALIDATION_EXCEPTION,
                ) {
                    return self.framing_failure(protocol, error);
                }
            }
            Err(InStream::Internal(message)) => {
                if let Err(error) = frame(
                    protocol,
                    &mut frames,
                    INTERNAL_EVENT,
                    true,
                    &InternalServerException {
                        message: Some(message),
                        reason: None,
                    },
                    &SCHEMA_INTERNAL_SERVER_EXCEPTION,
                ) {
                    return self.framing_failure(protocol, error);
                }
            }
        }
        // Every served invocation IS an execution, and `InvokeFlowResponse` reports which
        // one in its modelled header: the id the request named, else the one this service
        // minted for this invocation. The headers are serialized from the output shape
        // through the protocol, so the header name and the rendering are the model's.
        match event_stream_response(
            protocol,
            OPERATION_NAME,
            &InvokeFlowOutput {
                execution_id: Some(execution_id.to_string()),
            },
            &InvokeFlowOutput::SCHEMA,
            frames,
        ) {
            Ok(response) => response,
            Err(error) => self.framing_failure(
                protocol,
                format!("the response could not be serialized: {error}"),
            ),
        }
    }

    /// The last resort when a frame itself could not be serialized: the stream cannot
    /// carry the failure, so it is answered as the modelled server error it is.
    fn framing_failure(&self, protocol: &dyn ServerHttpProtocol, error: String) -> Response {
        error_response(protocol, &internal(error))
    }

    /// Run the flow, with every outcome of the run stated as what the stream delivers.
    /// Run the flow, with every outcome of the run stated as what the stream delivers.
    ///
    /// `ctx` is what keys the resolution to the PRINCIPAL the authorization decision was
    /// made about — the account that principal acts in and the Region it signed for — so the
    /// flow this walks is one that caller's own account holds and never one named by the
    /// request's identifiers alone (see `flows::run`).
    fn run(
        &self,
        ctx: &OperationContext<'_>,
        input: &InvokeFlowInput,
    ) -> Result<Vec<flows::FlowOutputValue>, InStream> {
        let entry = input
            .inputs
            .first()
            .ok_or_else(|| InStream::Invalid("inputs carries no input to enter the flow with".to_string()))?;
        let FlowInputContent::Document(document) = &entry.content;
        let outputs = flows::run(
            &*self.engine,
            ctx,
            &input.flow_identifier,
            &input.flow_alias_identifier,
            &entry.node_name,
            document,
        )
        .map_err(|refusal| match refusal {
            flows::FlowRefusal::StoreUnavailable => {
                InStream::Internal("the flow store is unavailable".to_string())
            }
            flows::FlowRefusal::NotFound => InStream::NotFound(flows::not_found_message()),
            flows::FlowRefusal::InputNodeUnavailable(node) => InStream::DependencyFailed(node),
            flows::FlowRefusal::Unservable(message) => InStream::Invalid(message),
        })?;
        Ok(outputs)
    }
}

/// A modelled exception the run earned, which the stream delivers as the
/// `FlowResponseStream` member of its name.
#[derive(Debug)]
enum InStream {
    /// `resourceNotFoundException`.
    NotFound(String),
    /// `dependencyFailedException`, carrying the name of the dependency that failed.
    DependencyFailed(String),
    /// `validationException`.
    Invalid(String),
    /// `internalServerException`.
    Internal(String),
}

/// The `FlowResponseStream` member one trace travels under, which the frames carry as
/// their `:event-type`.
const TRACE_EVENT: &str = "flowTraceEvent";

/// The traces of what the flow's ENTRY node emitted: one per output name the walk really
/// carried the invocation's document out on, with the document that left.
///
/// The field name is the connection's own `sourceOutput` where the connection names one,
/// else the `nodeOutputName` the request supplied the document as — both are names the
/// stored flow or the request carries, never invented. One trace per distinct output name,
/// because a value fanned out to two nodes left the entry node once on each name it left
/// by.
fn entry_output_traces(
    input: &InvokeFlowInput,
    outputs: &[flows::FlowOutputValue],
) -> Vec<FlowTrace> {
    let Some(entry) = input.inputs.first() else {
        return Vec::new();
    };
    let mut named: Vec<(String, &::aws_smithy_types::Document)> = Vec::new();
    for output in outputs {
        let FlowInputContent::Document(document) = &entry.content;
        let name = output
            .carried_by
            .as_ref()
            .map(|carried| carried.source_output.clone())
            .or_else(|| entry.node_output_name.clone());
        let Some(name) = name else { continue };
        if named.iter().any(|(seen, _)| *seen == name) {
            continue;
        }
        named.push((name, document));
    }
    named
        .into_iter()
        .map(|(name, document)| {
            FlowTrace::nodeOutputTrace(FlowTraceNodeOutputEvent {
                node_name: entry.node_name.clone(),
                timestamp: crate::state::now(),
                fields: vec![FlowTraceNodeOutputField {
                    node_output_name: name,
                    content: FlowTraceNodeOutputContent::Document(document.clone()),
                }],
            })
        })
        .collect()
}

/// The trace of what reached ONE node of the flow: the input of that node the connection
/// carried the value to, and the document that arrived.
///
/// `None` for a hop whose connection names no input — a trace field's `nodeInputName` is
/// `@required`, and nothing here invents a name the stored flow does not carry.
fn node_input_trace(output: &flows::FlowOutputValue) -> Option<FlowTrace> {
    let carried = output.carried_by.as_ref()?;
    Some(FlowTrace::nodeInputTrace(FlowTraceNodeInputEvent {
        node_name: output.node_name.clone(),
        timestamp: crate::state::now(),
        fields: vec![FlowTraceNodeInputField {
            node_input_name: carried.target_input.clone(),
            content: FlowTraceNodeInputContent::Document(output.document.clone()),
        }],
    }))
}

/// Append one `flowTraceEvent` to the stream.
fn frame_trace(
    protocol: &dyn ServerHttpProtocol,
    frames: &mut Vec<u8>,
    trace: FlowTrace,
) -> Result<(), String> {
    frame(
        protocol,
        frames,
        TRACE_EVENT,
        false,
        &FlowTraceEvent { trace },
        &FlowTraceEvent::SCHEMA,
    )
}

/// The execution one invocation runs as, as its `executionId` reports it.///
/// "The unique identifier for the current flow execution. If you don't provide a value,
/// Amazon Bedrock creates the identifier for you"
/// (`smithy_query(query="doc",shape="InvokeFlowRequest",member="executionId")`), so a
/// request that names an execution runs as that one and a request that names none runs as a
/// fresh execution of this service's own minting — a distinct one per invocation, which is
/// what lets a caller tell two invocations of the same alias apart.
///
/// The minted form is a version-4 UUID (`crate::state::new_uuid`), whose 36 characters of
/// lowercase hexadecimal and hyphens sit inside the modelled `FlowExecutionId`'s
/// `@length(2, 100)` and satisfy its `@pattern ^[0-9a-zA-Z._:-]+$`
/// (`smithy_query(query="operation",operation="InvokeFlow")`).
fn execution_id(input: &InvokeFlowInput) -> String {
    input
        .execution_id
        .clone()
        .unwrap_or_else(crate::state::new_uuid)
}

/// Append one `FlowResponseStream` member to the stream. `is_exception` marks a modelled
/// exception, which travels as `:message-type: exception` rather than as a data event.
fn frame(
    protocol: &dyn ServerHttpProtocol,
    frames: &mut Vec<u8>,
    event_type: &str,
    is_exception: bool,
    event: &dyn ::aws_smithy_schema::serde::SerializableStruct,
    schema: &::aws_smithy_schema::Schema,
) -> Result<(), String> {
    let frame = frame_event(protocol, event_type, is_exception, event, schema)
        .map_err(|error| format!("the {event_type} event could not be framed: {error}"))?;
    frames.extend_from_slice(&frame);
    Ok(())
}

/// Serving this operation is hand-authored because the generated `ErasedOperation` shim of
/// a `@streaming`-output operation declines the request.
///
/// The request half is the model's own: the framework's deserializer and constraint walk
/// over THIS operation's input, refused in this service's own wording
/// (`Service::override_refusal`), so a malformed URI label is refused exactly as one from a
/// non-streaming operation of the same service — and that refusal is an HTTP error, because
/// it is decided before any stream is opened.
impl<Eng: Send + Sync + 'static> EventStreamOperation for InvokeFlow<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r Request,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, Response> {
        Box::pin(async move {
            let definition = svc.definition();
            let protocol = definition.protocol();
            let input =
                match read_request::<InvokeFlowInput>(protocol, req, &InvokeFlowInput::SCHEMA) {
                    Ok(input) => input,
                    Err(RequestRefusal::Invalid(violations)) => {
                        let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
                        return definition.refusal_response(
                            ProtocolError::Invalid(violations),
                            chosen,
                            req,
                        );
                    }
                    Err(RequestRefusal::Unreadable(message)) => {
                        return error_response(protocol, &validation(message))
                    }
                };
            // Authorization is decided against the flow alias the call invokes — the
            // resource an identity-based policy for `bedrock:InvokeFlow` names — before any
            // store is read and before any stream is opened, so it is an HTTP refusal and an
            // unauthorized call is answered the same way whether or not the flow exists.
            let resource = flows::alias_resource_arn(
                ctx,
                &input.flow_identifier,
                &input.flow_alias_identifier,
            );
            if let Err(denied) =
                crate::state::authorize(&*self.engine, ctx, OPERATION_NAME, &resource)
            {
                return error_response(protocol, &InvokeFlowError::AccessDeniedException(denied));
            }
            // `ctx` travels on into the run: the resolution of the flow and its alias is
            // keyed to the same principal this decision was made about (see `flows::run`).
            self.respond(protocol, ctx, &input, &execution_id(&input))
        })
    }
}
