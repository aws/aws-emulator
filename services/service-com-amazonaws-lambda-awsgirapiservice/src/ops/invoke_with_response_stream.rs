// SPDX-License-Identifier: CC0-1.0

//! @op InvokeWithResponseStream
//!
//! Invokes a function and answers its response as an event stream: `POST
//! /2021-11-15/functions/{FunctionName}/response-streaming-invocations`, whose output
//! binds the `@streaming` union `InvokeWithResponseStreamResponseEvent` (`PayloadChunk`
//! and the terminating `InvokeComplete`) as its `@httpPayload`.
//!
//! This operation is HAND-AUTHORED rather than scaffolded. The op-stub generator declined
//! it ("a `@streaming` event-stream output member, which needs wire framing the generator
//! does not emit", `src/ops/SKIPPED_STUBS.md`), and the `#[smithy_service]` expansion also
//! declines any operation whose output binds a `@streaming` union — the generated
//! `ErasedOperation` shim on [`InvokeWithResponseStream`] therefore answers the
//! framework's "modeled but not served" signal and is never what serves a request. What
//! serves it is [`ResponseStreamEntry`], the hand-written entry
//! `crate::service::EmulatedService::override_operation` returns, which the router
//! consults BEFORE the generated routing table. The table entry still matters: the router
//! reads the MODEL's auth schemes off it, so this operation is SigV4-signed exactly as the
//! model says.
//!
//! ## What this emulator can answer
//!
//! Everything up to the invocation itself is computed from [`crate::state`]: the request is
//! read through the model's own HTTP bindings and checked against the model's constraints,
//! the `FunctionName`/`Qualifier` pair is resolved to a stored function and version — a
//! name that names none is the modelled `ResourceNotFoundException` (404), naming the
//! resource the invocation ADDRESSED (the function at the version it would have run,
//! `$LATEST` when the request named no qualifier), not a platform gap — and a function that
//! is still being provisioned is the modelled
//! `ResourceConflictException` (409), the window `CreateFunction` documents as "During this
//! time, you can't invoke or modify the function".
//!
//! `InvocationType=DryRun` is then the WHOLE operation: "Validate parameter values and
//! verify that the user or role has permission to invoke the function" runs nothing, and
//! the model's own `StatusCode` documentation states what it answers — "For the
//! `RequestResponse` invocation type, this status code is 200. For the `DryRun` invocation
//! type, this status code is 204."
//!
//! `InvocationType=RequestResponse` means "run the deployment package and stream what its
//! handler returned". Whether that invocation happens at all is decided first by Lambda's
//! recursive-loop guardrail: one arriving as part of a request chain that has already come
//! back round into this function is stopped when the function's recursive loop detection
//! configuration is `Terminate`, which is the modelled `RecursiveInvocationException` and
//! one `RecursiveInvocationsDropped` datapoint for the function
//! (`crate::state::recursion`). An invocation nothing stopped is one Lambda admitted to the
//! function, metered as one `Invocations` datapoint of the function and of the resource this
//! call addressed (`crate::state::metrics`). The handler then RUNS, in the interpreter
//! `crate::state::execution` builds for the function's runtime, and what it produced is
//! framed as this operation's event stream (`service_common::event_stream`): the response
//! as a `PayloadChunk`, then the `InvokeComplete` that ends the stream, carrying the error
//! of a handler that raised and the log tail when one was asked for.
//!
//! ## Modeled contract for `InvokeWithResponseStream`
//!
//! From this service's Smithy model
//! (`smithy_query(query="operation", operation="InvokeWithResponseStream",
//! model_path="services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json")`):
//!
//! * **Route:** `POST /2021-11-15/functions/{FunctionName}/response-streaming-invocations
//!   -> 200`
//! * **Input:** `FunctionName` (`NamespacedFunctionName`, `@httpLabel`, **required**),
//!   `InvocationType` (`ResponseStreamingInvocationType`, `@httpHeader
//!   X-Amz-Invocation-Type`, one of `RequestResponse | DryRun`), `LogType`
//!   (`@httpHeader X-Amz-Log-Type`), `ClientContext` (`@httpHeader
//!   X-Amz-Client-Context`), `Qualifier`
//!   (`NumericLatestPublishedOrAliasQualifier`, `@httpQuery Qualifier`), `Payload`
//!   (blob, `@httpPayload`, sensitive), `TenantId` (`@httpHeader X-Amz-Tenant-Id`)
//! * **Output:** `StatusCode` (`@httpResponseCode`), `ExecutedVersion` (`@httpHeader
//!   X-Amz-Executed-Version`), `EventStream` (`@streaming` union, `@httpPayload`),
//!   `ResponseStreamContentType` (`@httpHeader Content-Type`)
//! * **Errors:** the 40 the operation declares, carried on
//!   [`InvokeWithResponseStreamErrorModeled`] below with the status the model gives each.

#[allow(unused_imports)]
use super::types::*;

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use service_common::event_stream::{
    error_response, event_stream_response, frame_event, EventStreamOperation,
};
use smithy_server_runtime::{BoxFuture, OperationContext, ServerHttpProtocol, Service};

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

// The operation's MODELED errors, every one the model declares for it. The scaffold's
// `NotImplemented` cannot live here — `#[operation_error]` accepts only variants the model
// declares — hence the `InvokeWithResponseStreamError` alias below.
#[smithy_macros::operation_error("com.amazonaws.lambda#InvokeWithResponseStream")]
#[derive(Debug)]
pub enum InvokeWithResponseStreamErrorModeled {
    CodeArtifactUserDeletedException(CodeArtifactUserDeletedException),
    CodeArtifactUserFailedException(CodeArtifactUserFailedException),
    CodeArtifactUserPendingException(CodeArtifactUserPendingException),
    EC2AccessDeniedException(EC2AccessDeniedException),
    EC2ThrottledException(EC2ThrottledException),
    EC2UnexpectedException(EC2UnexpectedException),
    EFSIOException(EFSIOException),
    EFSMountConnectivityException(EFSMountConnectivityException),
    EFSMountFailureException(EFSMountFailureException),
    EFSMountTimeoutException(EFSMountTimeoutException),
    ENILimitReachedException(ENILimitReachedException),
    InvalidParameterValueException(InvalidParameterValueException),
    InvalidRequestContentException(InvalidRequestContentException),
    InvalidRuntimeException(InvalidRuntimeException),
    InvalidSecurityGroupIDException(InvalidSecurityGroupIDException),
    InvalidSubnetIDException(InvalidSubnetIDException),
    InvalidZipFileException(InvalidZipFileException),
    KMSAccessDeniedException(KMSAccessDeniedException),
    KMSDisabledException(KMSDisabledException),
    KMSInvalidStateException(KMSInvalidStateException),
    KMSNotFoundException(KMSNotFoundException),
    NoPublishedVersionException(NoPublishedVersionException),
    RecursiveInvocationException(RecursiveInvocationException),
    RequestTooLargeException(RequestTooLargeException),
    ResourceConflictException(ResourceConflictException),
    ResourceNotFoundException(ResourceNotFoundException),
    ResourceNotReadyException(ResourceNotReadyException),
    S3FilesMountConnectivityException(S3FilesMountConnectivityException),
    S3FilesMountFailureException(S3FilesMountFailureException),
    S3FilesMountTimeoutException(S3FilesMountTimeoutException),
    SerializedRequestEntityTooLargeException(SerializedRequestEntityTooLargeException),
    ServiceException(ServiceException),
    ServiceQuotaExceededException(ServiceQuotaExceededException),
    SnapStartException(SnapStartException),
    SnapStartNotReadyException(SnapStartNotReadyException),
    SnapStartRegenerationFailureException(SnapStartRegenerationFailureException),
    SnapStartTimeoutException(SnapStartTimeoutException),
    SubnetIPAddressLimitReachedException(SubnetIPAddressLimitReachedException),
    TooManyRequestsException(TooManyRequestsException),
    UnsupportedMediaTypeException(UnsupportedMediaTypeException),
}

/// This operation's error channel. Wrapping the modeled enum is what lets it refuse a bad
/// request the model has no error for (`Scaffolded::validation`) and state the one path it
/// cannot serve as not implemented rather than as a modelled failure.
pub type InvokeWithResponseStreamError =
    crate::scaffolded::Scaffolded<InvokeWithResponseStreamErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel.
impl From<InvokeWithResponseStreamErrorModeled> for InvokeWithResponseStreamError {
    fn from(modeled: InvokeWithResponseStreamErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `InvokeWithResponseStream` operation.
///
/// Owns the shared engine (the constructor seam). The generated `ErasedOperation` shim
/// on this type declines, because the operation's output binds a `@streaming` union;
/// [`ResponseStreamEntry`] is what the service returns from `override_operation`, and it
/// delegates to [`Self::serve_request`] here.
pub struct InvokeWithResponseStream<Eng> {
    engine: Arc<Eng>,
}

impl<Eng> InvokeWithResponseStream<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `ServiceDefinition::new(engine)`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self { engine }
    }
}

/// The hand-written entry that actually serves the request; see the module docs.
pub struct ResponseStreamEntry<Eng> {
    op: InvokeWithResponseStream<Eng>,
}

impl<Eng> ResponseStreamEntry<Eng> {
    /// Construct the entry from the shared engine — the same seam every operation uses,
    /// so this entry and every other operation read ONE store.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            op: InvokeWithResponseStream::new(engine),
        }
    }
}

impl<Eng: Send + Sync + 'static> EventStreamOperation for ResponseStreamEntry<Eng> {
    fn serve<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The author's `override_protocol` wins over the model-derived one, the same
            // resolution the centralized sequence applies.
            let protocol = svc
                .override_protocol()
                .unwrap_or_else(|| svc.definition().protocol());
            self.op.serve_request(protocol, req, ctx).await
        })
    }
}

impl<Eng: Send + Sync + 'static> InvokeWithResponseStream<Eng> {
    /// Serve one request, rendering a failure in this service's own envelope — the same
    /// envelope every other operation's modelled error is rendered in, so a client reads
    /// one error shape from the whole service.
    ///
    /// The request is also RECORDED, which this operation does itself because it does not
    /// run through the centralized sequence that records it for every other one. The
    /// record is value-free: the signing name, the operation, the access-key id the
    /// request presented (the public half of a credential, never a secret) and the status
    /// answered — never a payload, a header value or a credential secret.
    ///
    /// An error this operation answers names the CLASS of its fault in the `Type` member
    /// every error shape of this model declares, exactly as every other modelled operation
    /// of this service does (`crate::service::fault_class`). The class is a property of the
    /// failure rather than of the raise site, so it is named here — the one place this
    /// operation's whole rendered response passes through, because the wrapper that names it
    /// for the rest of the surface serves only operations the generated routing table holds,
    /// which this `@streaming`-output one is not. A success (a dry run, an event stream)
    /// reports no failure and is answered untouched.
    async fn serve_request(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> RtResponse {
        let response = crate::service::fault_class::with_fault_class(
            OPERATION_NAME,
            match self.invoke(protocol, req, ctx).await {
                Ok(response) => response,
                Err(error) => error_response(protocol, &error),
            },
        );
        tracing::info!(
            service = ctx.identity().signing_name(),
            operation = OPERATION_NAME,
            access_key_id = ::engine::extract_akid(&ctx.parts().headers),
            status = response.status().as_u16(),
            "served a response-streaming invocation"
        );
        response
    }

    /// The invocation itself: read the request through the model's bindings, resolve the
    /// function out of the store, and answer what this emulator can compute.
    async fn invoke(
        &self,
        protocol: &dyn ServerHttpProtocol,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> Result<RtResponse, InvokeWithResponseStreamError> {
        // Read through the service's own protocol and check against the model, so the
        // `@httpLabel`, `@httpQuery`, `@httpHeader` and `@httpPayload` bindings and every
        // constraint this request is held to are the model's. A refusal is worded the way
        // the framework's own boundary words one, and answered with the same 400
        // `ValidationException` code it answers with.
        let input: InvokeWithResponseStreamInput = service_common::event_stream::deserialize_request(
            protocol,
            req,
            INVOKE_WITH_RESPONSE_STREAM_INPUT_SCHEMA,
        )
        .map_err(crate::scaffolded::Scaffolded::validation)?;

        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(service_error("Lambda state is not reachable on this engine"));
        };

        // A `Qualifier` outside the pattern its member declares names no version or alias
        // at all, so it is refused as the request violation it is, before anything is
        // looked up (`crate::state::admitted_qualifier`).
        crate::state::admitted_qualifier(input.qualifier.as_deref())?;

        // "By default, Lambda invokes your function synchronously (i.e. the
        // InvocationType is RequestResponse)" — the one default `Invoke` documents for
        // the same member, and the constraint walk above has already held the value to
        // the model's `ResponseStreamingInvocationType` value set.
        let invocation_type = input
            .invocation_type
            .clone()
            .unwrap_or_else(|| INVOCATION_TYPE_REQUEST_RESPONSE.to_string());

        let now = ::std::time::SystemTime::now();
        let region = crate::state::request_region(ctx);
        // Lambda decides an invocation at the IAM layer, BEFORE the function is reached: this
        // entry point needs "permission for the `lambda:InvokeFunction` action" exactly as
        // `Invoke` does, so the same gate answers it (`crate::state::invoke_authorization`).
        // Asked first, with no store guard held, so a refused invocation runs nothing.
        crate::state::authorize_invocation(
            &*self.engine,
            ctx,
            crate::state::INVOKE_FUNCTION_ACTION,
            &region,
            &input.function_name,
            input.qualifier.as_deref(),
        )?;

        // The resource an INVOCATION addresses is a function AT a version: `Qualifier`
        // "specif[ies] a version or alias to invoke a published version of the function",
        // and an invocation that names none runs the unpublished version, which is what
        // the output's `ExecutedVersion` reports as `$LATEST`. So the refusal for a pair
        // that resolves to nothing names that qualified resource, built from the request by
        // the one construction every invoking operation shares
        // (`crate::state::addressed_function_arn`).
        let reference =
            crate::state::parse_function_reference(&input.function_name).ok_or_else(|| {
                invalid_parameter(format!(
                    "1 validation error detected: Value '{}' at 'functionName' failed to \
                     satisfy constraint: Member must satisfy one of the documented name \
                     formats (name, name:qualifier, partial ARN or function ARN)",
                    input.function_name
                ))
            })?;
        let addressed_arn =
            crate::state::addressed_arn_of(&region, &reference, input.qualifier.as_deref());

        // The store guard lives in THIS block and no longer: everything the run needs is
        // copied out of the record here, and the run below happens with no guard held.
        let (mut invocation, function_name, resource, kms_key_arn, stopped, qualifier) = {
            let mut store = state.lock();
        let (record, version) = crate::state::resolve_function(
            &mut store,
            &input.function_name,
            input.qualifier.as_deref(),
            &region,
            now,
        )
        .map_err(|failure| match failure {
            crate::state::ResolveFailure::Malformed(message) => invalid_parameter(message),
            crate::state::ResolveFailure::NotFound(message) => match &addressed_arn {
                Some(arn) => not_found(format!("Function not found: {arn}")),
                // No version was resolved for this name — an ARN of another Region is
                // refused on the Region — so the refusal is the resolution's own.
                None => not_found(message),
            },
        })?;

        // "During this time, you can't invoke or modify the function" (`CreateFunction`),
        // so an invocation that arrives inside the provisioning window is refused rather
        // than served.
        if record.state != crate::state::STATE_ACTIVE {
            return Err(resource_conflict(format!(
                "The operation cannot be performed at this time. The function is currently \
                 in the following state: {}",
                record.state
            )));
        }

        let payload = input.payload.as_deref().unwrap_or_default();
        // "Up to 3,583 bytes of base64-encoded data about the invoking client to pass to
        // the function in the context object": the handler is given the DECODED document,
        // and a value that is not base64 is refused rather than passed on as itself.
        let client_context = match input.client_context.as_deref() {
            None => None,
            Some(encoded) => match ::base64::Engine::decode(
                &::base64::engine::general_purpose::STANDARD,
                encoded,
            )
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            {
                Some(decoded) => Some(decoded),
                None => {
                    return Err(invalid_request_content(
                        "ClientContext is not valid base64-encoded data.".to_string(),
                    ))
                }
            },
        };
        // "For synchronous invocations, the maximum payload size is 6 MB" (`Invoke`): a
        // response-streaming invocation is a synchronous one, and the request half of it
        // is bounded the same way.
        if payload.len() > MAX_SYNCHRONOUS_PAYLOAD_BYTES {
            return Err(request_too_large(format!(
                "Request must be smaller than {MAX_SYNCHRONOUS_PAYLOAD_BYTES} bytes for the \
                 {invocation_type} invocation type"
            )));
        }
        // "The JSON that you want to provide to your Lambda function as input": a body
        // that is not JSON is refused with the modelled 400 carrying the parse diagnostic
        // (`crate::state::invocation_payload`), and an absent or empty one is the
        // documented no-event invocation.
        if let Some(message) = crate::state::unparseable_payload_message(payload) {
            return Err(invalid_request_content(message));
        }

        let function_arn = record.versioned_arn(&version);
        // `ExecutedVersion` is the version the qualifier resolves to for THIS invocation,
        // never the alias name the call addressed; an alias that splits its invocations
        // between two versions resolves per invocation (`crate::state::alias_routing`).
        let executed_version = record
            .routed_version(&version)
            .unwrap_or_else(|| version.clone());
        // The configuration the invocation RESOLVED, which is what the run happens under.
        let resolved_config = record
            .config_of(&executed_version)
            .cloned()
            .unwrap_or_else(|| record.config.clone());

        if invocation_type == INVOCATION_TYPE_DRY_RUN {
            // "DryRun – Validate parameter values and verify that the user or role has
            // permission to invoke the function." Nothing runs for a dry run even on the
            // real service, so the validation above IS the operation, and the model's own
            // `StatusCode` documentation states the status it answers. It invokes nothing,
            // so it is no hop of any request chain either.
            return Ok(dry_run_response(&executed_version));
        }

        // Lambda's recursive-loop guardrail decides whether this invocation happens at
        // all, exactly as it does on `Invoke`: a response-streaming invocation is an
        // invocation of the function, so it is counted against the request chain it belongs
        // to, and one the guardrail stops is the modelled 400 and one
        // `RecursiveInvocationsDropped` datapoint (`crate::state::recursion`). The metric is
        // published with the store lock released, because it goes into CloudWatch's own
        // store.
        let chain = crate::state::request_chain(ctx);
        let stopped = crate::state::guardrail(record, chain.as_deref(), now)
            == crate::state::Guardrail::Stopped;
        let function_name = record.config.name.clone();
        let resource = crate::state::invoked_resource(&function_name, &version);
        // Everything the RUN needs is copied out of the record here, because the run
        // happens with no store guard held.
        let invocation = crate::state::Invocation {
            function_name: function_name.clone(),
            executed_version: executed_version.clone(),
            invoked_function_arn: function_arn.clone(),
            region: region.clone(),
            runtime: resolved_config.runtime.clone(),
            handler: resolved_config.handler.clone(),
            code_revision: resolved_config.revision_id.clone(),
            variables: resolved_config.environment.clone(),
            memory_size: resolved_config.memory_size,
            timeout: resolved_config.timeout,
            architecture: resolved_config
                .architectures
                .first()
                .cloned()
                .unwrap_or_default(),
            zip_bytes: record.code_of(&executed_version).zip_bytes.clone(),
            layer_zips: crate::state::layer_archives(&resolved_config.layers),
            payload: payload.to_vec(),
            client_context,
            tenant_id: input.tenant_id.clone(),
            // The run's own id, minted here with the rest of what the run needs. It is
            // stated as the invocation this answer names once the invocation is ADMITTED
            // below, so a request refused instead keeps a response id of its own (see
            // `crate::service::request_id_header`).
            request_id: crate::state::mint_request_id(),
            // The X-Ray trace context this invocation runs with: the chain the caller
            // named, under the rule the function's `TracingConfig` mode gives
            // (`crate::state::execution::trace_context`).
            trace: crate::state::invocation_trace_context(
                crate::state::supplied_trace_header(ctx).as_deref(),
                &resolved_config.tracing_mode,
                now,
            ),
            // Which kind of environment serves it is the admission's to decide, below:
            // nothing has been admitted while the store guard is still held.
            initialization_type: crate::state::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            // The AWS SDK this runtime supplies the handler reaches the rest of this
            // emulator through the callback the framework handed THIS request, signing
            // every call the handler makes as the session of the function's own execution
            // role (`crate::state::execution_role_session`).
            aws: crate::state::function_sdk_access(
                &*self.engine,
                ctx,
                &region,
                &resolved_config.role,
                &function_name,
            ),
            // Where the run's log is delivered, and what the function's execution role may
            // do there (`crate::state::execution::log_delivery`).
            logs: Some(crate::state::log_target(
                &*self.engine,
                &region,
                &resolved_config,
            )),
            // What this invocation is metered by once it has run: the time the run took and
            // the failure it came to (`crate::state::metrics`).
            metrics: Some(crate::state::InvocationMetrics::new(
                &*self.engine,
                &function_name,
                &resource,
            )),
        };
        let kms_key_arn = resolved_config.kms_key_arn.clone();
            (
                invocation,
                function_name,
                resource,
                kms_key_arn,
                stopped,
                version,
            )
        };
        let executed_version = invocation.executed_version.clone();
        if stopped {
            crate::state::publish_recursive_invocation_dropped(&*self.engine, &function_name);
            return Err(recursive_invocation());
        }
        // Lambda admits the invocation to the function only while the function's own
        // concurrency pool has room for it, and a response-streaming invocation draws on the
        // SAME pool every other invocation of the function does — "concurrency settings
        // apply to the function as a whole" (`PutFunctionConcurrency`). The slot is held
        // until this invocation is answered, and the admission decides which kind of
        // environment serves it (`crate::state::execution_slot`).
        // A request the pool had no room for is answered the modelled 429 and metered as one
        // request the service throttled, exactly as on `Invoke` (`crate::state::metrics`).
        let admitted = match crate::state::admit_invocation(&state, &function_name, &qualifier, now)
        {
            Ok(admitted) => admitted,
            Err(refused) => {
                crate::state::publish_throttle(&*self.engine, &function_name, &resource);
                return Err(throttled(refused));
            }
        };
        invocation.initialization_type = admitted.initialization_type;
        let _slot = admitted.slot;
        // Admitting the invocation raised the function's concurrency, which is published as
        // one reading of that gauge exactly as on `Invoke`
        // (`crate::state::concurrency_metering`).
        crate::state::publish_function_concurrency(
            &*self.engine,
            &function_name,
            admitted.concurrent_executions,
            now,
        );
        // The invocation this call runs is the one its answer names, exactly as on `Invoke`.
        crate::service::request_id_header::serving_invocation(&invocation.request_id);
        // The invocation is admitted to the function at this point, so it is metered as one
        // `Invocations` datapoint of the function and of the resource this call addressed
        // (`crate::state::metrics`) before the run is attempted: the count is of invocation
        // requests that reached the function, and says nothing about what the execution
        // produced.
        crate::state::publish_invocation(&*self.engine, &function_name, &resource);

        // A function whose environment variables are encrypted with a customer managed key
        // is only invoked while that key can decrypt them, exactly as on `Invoke`.
        if let Some(fault) = crate::state::environment_key_fault(
            ctx.callback(),
            &region,
            kms_key_arn.as_deref(),
            &invocation.variables,
        )
        .await
        {
            return Err(environment_key_refusal(fault));
        }

        // "InvocationType=RequestResponse" here means "run the deployment package and
        // stream what its handler returned": the handler is RUN, its response travels as
        // the `PayloadChunk` events, and the stream ends with the `InvokeComplete` event
        // that "is returned when the stream has ended and all the payload chunks have been
        // returned" — carrying the error of a handler that raised, and the log tail when
        // the caller asked for one.
        let invoked = crate::state::run_invocation(invocation)
            .await
            .map_err(unrunnable)?;
        streamed_response(
            protocol,
            &executed_version,
            &invoked,
            input.log_type.as_deref() == Some(LOG_TYPE_TAIL),
        )
    }
}

/// The event-stream answer to a run: one `PayloadChunk` per write the handler made, then
/// the terminating `InvokeComplete`.
///
/// A handler that RAISED still streams what it produced — "the details of any returned
/// error" are the complete event's, which is where a streaming client reads them. For a
/// handler that never began a stream the error document travels as the chunk and the
/// completion names the error; for one that failed after writing, the bytes it wrote travel
/// as the chunks and the completion carries the error document as its details, so the client
/// can tell those bytes are a response cut short. A chunk is written only when there is
/// payload to write.
fn streamed_response(
    protocol: &dyn ServerHttpProtocol,
    executed_version: &str,
    invoked: &crate::state::Invoked,
    tail_requested: bool,
) -> Result<RtResponse, InvokeWithResponseStreamError> {
    let mut frames = Vec::new();
    // One chunk per WRITE a response-streaming handler made, so the boundaries a client
    // reads are the handler's own; a handler that returned a value instead wrote nothing,
    // and its whole response travels as the one chunk it is.
    let written: Vec<&[u8]> = if invoked.chunks.is_empty() {
        vec![invoked.payload.as_slice()]
    } else {
        invoked.chunks.iter().map(Vec::as_slice).collect()
    };
    for chunk in written {
        if chunk.is_empty() {
            continue;
        }
        frames.extend(framed_payload_chunk(chunk).map_err(|error| {
            service_error(&format!("the payload chunk could not be framed: {error}"))
        })?);
    }
    let complete = InvokeWithResponseStreamCompleteEvent {
        error_code: invoked.function_error.clone(),
        error_details: invoked.error_document(),
        log_result: if tail_requested {
            Some(invoked.log_tail())
        } else {
            None
        },
    };
    frames.extend(
        frame_event(
            protocol,
            INVOKE_COMPLETE_EVENT,
            false,
            &complete,
            &InvokeWithResponseStreamCompleteEvent::SCHEMA,
        )
        .map_err(|error| {
            service_error(&format!("the completion event could not be framed: {error}"))
        })?,
    );
    let headers = InvokeWithResponseStreamOutput {
        status_code: REQUEST_RESPONSE_STATUS,
        executed_version: Some(executed_version.to_string()),
        // "The type of data the function returned": the response of a run that returned a
        // JSON document, which is what the runtime serialized it to.
        response_stream_content_type: Some(RESPONSE_CONTENT_TYPE.to_string()),
    };
    event_stream_response(
        protocol,
        OPERATION_NAME,
        &headers,
        &InvokeWithResponseStreamOutput::SCHEMA,
        frames,
    )
    .map_err(|error| service_error(&format!("the event stream could not be rendered: {error}")))
}

/// One `PayloadChunk` frame carrying `payload` as the frame's OWN body.
///
/// `InvokeResponseStreamUpdate$Payload` is the event's `@eventPayload` and a blob, so the
/// bytes the handler produced ARE the frame's payload — not a member of a JSON object
/// inside it — and the frame announces them as the opaque bytes they are. That binding is
/// what the framework's event serializer does not express (it renders the whole event
/// structure as JSON, which would base64-encode the blob into a member), so this frame is
/// written here, through the same event-stream encoder.
fn framed_payload_chunk(payload: &[u8]) -> Result<Vec<u8>, String> {
    let message = ::aws_smithy_types::event_stream::Message::new(payload.to_vec())
        .add_header(::aws_smithy_types::event_stream::Header::new(
            ":message-type",
            ::aws_smithy_types::event_stream::HeaderValue::String("event".into()),
        ))
        .add_header(::aws_smithy_types::event_stream::Header::new(
            ":event-type",
            ::aws_smithy_types::event_stream::HeaderValue::String(PAYLOAD_CHUNK_EVENT.into()),
        ))
        .add_header(::aws_smithy_types::event_stream::Header::new(
            ":content-type",
            ::aws_smithy_types::event_stream::HeaderValue::String(
                PAYLOAD_CHUNK_CONTENT_TYPE.into(),
            ),
        ));
    let mut framed = Vec::new();
    ::aws_smithy_eventstream::frame::write_message_to(&message, &mut framed)
        .map_err(|error| error.to_string())?;
    Ok(framed)
}

/// How a key that cannot decrypt a function's environment variables is answered: the
/// modelled 502 for the fault the key service reported.
fn environment_key_refusal(
    fault: crate::state::EnvironmentKeyFault,
) -> InvokeWithResponseStreamError {
    match fault {
        crate::state::EnvironmentKeyFault::Disabled(message) => {
            InvokeWithResponseStreamErrorModeled::KMSDisabledException(KMSDisabledException {
                r#type: None,
                message: Some(message),
            })
        }
        crate::state::EnvironmentKeyFault::InvalidState(message) => {
            InvokeWithResponseStreamErrorModeled::KMSInvalidStateException(
                KMSInvalidStateException {
                    r#type: None,
                    message: Some(message),
                },
            )
        }
        crate::state::EnvironmentKeyFault::NotFound(message) => {
            InvokeWithResponseStreamErrorModeled::KMSNotFoundException(KMSNotFoundException {
                r#type: None,
                message: Some(message),
            })
        }
        crate::state::EnvironmentKeyFault::AccessDenied(message) => {
            InvokeWithResponseStreamErrorModeled::KMSAccessDeniedException(
                KMSAccessDeniedException {
                    r#type: None,
                    message: Some(message),
                },
            )
        }
    }
    .into()
}

/// How an invocation the function's concurrency pool had no room for is answered: the
/// modelled 429 naming the limit that was reached, exactly as `Invoke` answers one
/// (`crate::state::execution_slot`).
fn throttled(refused: crate::state::Throttled) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::TooManyRequestsException(TooManyRequestsException {
        r#type: None,
        message: Some(refused.message),
        reason: Some(refused.reason.to_string()),
        retry_after_seconds: None,
    })
    .into()
}

/// How an invocation that ran NOTHING is answered: one modelled failure per reason, so a
/// client is never told a function ran when none did.
fn unrunnable(why: crate::state::Unrunnable) -> InvokeWithResponseStreamError {
    match why {
        crate::state::Unrunnable::NoEngine { runtime } => {
            InvokeWithResponseStreamErrorModeled::InvalidRuntimeException(InvalidRuntimeException {
                r#type: None,
                message: Some(format!(
                    "The runtime {runtime} is not supported by this emulator, which runs the \
                     Python and node runtimes."
                )),
            })
            .into()
        }
        crate::state::Unrunnable::NoHandler => invalid_parameter(
            "The function configuration names no Handler to run.".to_string(),
        ),
        crate::state::Unrunnable::PackageNotHeld => {
            InvokeWithResponseStreamErrorModeled::InvalidZipFileException(InvalidZipFileException {
                r#type: None,
                message: Some(
                    "The deployment package of this function is not held by this emulator, so \
                     there is nothing to run."
                        .to_string(),
                ),
            })
            .into()
        }
        crate::state::Unrunnable::PackageUnreadable(why) => {
            InvokeWithResponseStreamErrorModeled::InvalidZipFileException(InvalidZipFileException {
                r#type: None,
                message: Some(format!("The deployment package could not be read: {why}")),
            })
            .into()
        }
        // The capacity that was full is the host's pool of run slots, which stands in for the
        // account's concurrent executions, so the throttle names that limit.
        crate::state::Unrunnable::Overloaded(message) => {
            InvokeWithResponseStreamErrorModeled::TooManyRequestsException(
                TooManyRequestsException {
                    r#type: None,
                    message: Some(message),
                    reason: Some(crate::state::ACCOUNT_CONCURRENCY_REASON.to_string()),
                    retry_after_seconds: None,
                },
            )
            .into()
        }
        crate::state::Unrunnable::Internal(message) => service_error(&message),
    }
}

/// Whether `req` is the request this operation serves: `POST
/// /2021-11-15/functions/{FunctionName}/response-streaming-invocations`.
///
/// This is the route match `crate::service::EmulatedService::override_operation` applies,
/// kept beside the handler so the two cannot disagree about which requests this claims.
/// The METHOD is part of it, so another method addressed to the same path falls through to
/// the generated routing table rather than being claimed here.
#[must_use]
pub fn addresses_response_streaming_invocations(req: &RtRequest) -> bool {
    // An HTTP method is a case-sensitive token, so the real service would not route a
    // lowercase `post` here either.
    if req.method() != "POST" {
        return false;
    }
    let mut segments = service_common::request_path(req.uri()).split('/');
    // A path always starts with `/`, so the first split segment is empty.
    if segments.next() != Some("") {
        return false;
    }
    if segments.next() != Some(API_DATE) || segments.next() != Some("functions") {
        return false;
    }
    // The `{FunctionName}` label, which the handler reads through the model's own binding.
    let Some(label) = segments.next() else {
        return false;
    };
    !label.is_empty()
        && segments.next() == Some(RESPONSE_STREAMING_INVOCATIONS)
        && segments.next().is_none()
}

/// The API-date segment of this operation's route.
const API_DATE: &str = "2021-11-15";

/// This operation's name, as the model spells it and as the audit record names it.
const OPERATION_NAME: &str = "InvokeWithResponseStream";

/// The last segment of this operation's route.
const RESPONSE_STREAMING_INVOCATIONS: &str = "response-streaming-invocations";

/// `ResponseStreamingInvocationType` values, from the model's enum
/// (`RequestResponse | DryRun`).
const INVOCATION_TYPE_REQUEST_RESPONSE: &str = "RequestResponse";
/// See [`INVOCATION_TYPE_REQUEST_RESPONSE`].
const INVOCATION_TYPE_DRY_RUN: &str = "DryRun";

/// The `StatusCode` a `DryRun` invocation answers with, which the model's own `StatusCode`
/// documentation states: "For the DryRun invocation type, this status code is 204."
const DRY_RUN_STATUS: u16 = 204;

/// The `StatusCode` a `RequestResponse` invocation answers with: "For the
/// `RequestResponse` invocation type, this status code is 200."
const REQUEST_RESPONSE_STATUS: i32 = 200;

/// The `LogType` value that asks for the log tail (`None | Tail`).
const LOG_TYPE_TAIL: &str = "Tail";

/// The `@streaming` union member one chunk of the response travels as (source:
/// `smithy_query(query="shape", shape="InvokeWithResponseStreamResponseEvent")`).
const PAYLOAD_CHUNK_EVENT: &str = "PayloadChunk";

/// The `@streaming` union member that terminates the stream; see [`PAYLOAD_CHUNK_EVENT`].
const INVOKE_COMPLETE_EVENT: &str = "InvokeComplete";

/// `ResponseStreamContentType` for a run whose handler returned a value: the runtime
/// serialized that value to JSON, so that is what the stream carries.
const RESPONSE_CONTENT_TYPE: &str = "application/json";

/// The media type ONE `PayloadChunk` frame announces for its own body: the bytes the
/// handler produced, carried as the event's `@eventPayload` blob.
const PAYLOAD_CHUNK_CONTENT_TYPE: &str = "application/octet-stream";

/// "For synchronous invocations, the maximum payload size is 6 MB" (the model's own
/// `Invoke` documentation).
const MAX_SYNCHRONOUS_PAYLOAD_BYTES: usize = 6 * 1024 * 1024;

/// The answer to a `DryRun`: the documented status, no event stream, and the version the
/// invocation resolved to on the output's own `@httpHeader` binding
/// (`ExecutedVersion`/`X-Amz-Executed-Version`).
///
/// The body is EMPTY rather than an event stream with no events in it: nothing ran, so
/// there is no chunk sequence to frame, and the documented 204 carries no body at all.
fn dry_run_response(version: &str) -> RtResponse {
    let mut response = RtResponse::new(
        StatusCode::try_from(DRY_RUN_STATUS).expect("the documented dry-run status is valid"),
        SdkBody::empty(),
    );
    response
        .headers_mut()
        .insert("x-amz-executed-version", version.to_string());
    response
}

/// The modelled 400 for a `FunctionName`/`Qualifier` pair that names no single resource.
fn invalid_parameter(message: String) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::InvalidParameterValueException(
        InvalidParameterValueException {
            r#type: None,
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 404 for a name and qualifier that name no function or no such version.
fn not_found(message: String) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::ResourceNotFoundException(ResourceNotFoundException {
        r#type: None,
        message: Some(message),
    })
    .into()
}

/// The modelled 409 for an invocation of a function that is not ready to take one.
fn resource_conflict(message: String) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::ResourceConflictException(ResourceConflictException {
        r#type: None,
        message: Some(message),
    })
    .into()
}

/// The modelled 413 for a payload past the documented synchronous limit.
fn request_too_large(message: String) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::RequestTooLargeException(RequestTooLargeException {
        r#type: None,
        message: Some(message),
    })
    .into()
}

/// The modelled 400 for a request body that is not the JSON event the function takes.
fn invalid_request_content(message: String) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::InvalidRequestContentException(
        InvalidRequestContentException {
            r#type: None,
            message: Some(message),
        },
    )
    .into()
}

/// The modelled 400 for an invocation Lambda's recursive-loop guardrail stopped.
fn recursive_invocation() -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::RecursiveInvocationException(
        RecursiveInvocationException {
            r#type: None,
            message: Some(crate::state::RECURSIVE_INVOCATION_MESSAGE.to_string()),
        },
    )
    .into()
}

/// The modelled 500 for a failure of the store itself.
fn service_error(message: &str) -> InvokeWithResponseStreamError {
    InvokeWithResponseStreamErrorModeled::ServiceException(ServiceException {
        r#type: None,
        message: Some(message.to_string()),
    })
    .into()
}
