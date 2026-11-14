// SPDX-License-Identifier: CC0-1.0

//! @op InvokeEndpointAsync
//!
//! IMPLEMENTED: decides the CALLER's authorization first — the identity-based IAM
//! policies in force on the principal the request signed as, for
//! `sagemaker:InvokeEndpointAsync` on the endpoint it addressed
//! ([`crate::state::authorization`]), refused 403 `AccessDeniedException` before anything
//! else is looked at — then queues one asynchronous inference against an endpoint the
//! SageMaker CONTROL PLANE holds, through the resolver all three operations of this
//! service share
//! ([`crate::state::resolve_endpoint`]), and answers HTTP 202 with WHERE the result will
//! be. The two locations are computed from the endpoint's own
//! `AsyncInferenceConfig$OutputConfig` paths plus the request's
//! `S3OutputPathExtension`/`Filename` and the inference id, so resubmitting one id yields
//! the same strings by construction. A submission naming no endpoint, an endpoint with no
//! deployment behind it, or an endpoint whose configuration carries no
//! `AsyncInferenceConfig` is refused with the modelled `ValidationError` (HTTP 400). WHICH
//! input source a submission carries is decided before any of that, because it is a
//! property of the request alone: a submission carrying neither `Body` nor `InputLocation`,
//! or both, is refused naming that rule and never the endpoint, and an inline `Body` past
//! the maximum the model declares for it is refused there too, since the size of the inline
//! source is part of choosing it. Everything the endpoint itself decides follows the
//! resolution of the endpoint the submission addressed (the order
//! [`crate::service::error_envelope`] states for the whole service). This
//! is the ONLY file to touch for this operation.
//!
//! ## What becomes of a queued submission
//!
//! Accepting the submission is work this service really does, so the 202 is honest; the
//! INFERENCE is not, because a SageMaker model is a customer-provided container image and
//! this emulator runs none. So the queued record leaves `InProgress` for the modelled
//! failure state once its outcome falls due, carrying
//! [`crate::state::NO_INFERENCE_CONTAINER`] as the reason, and nothing is ever written to
//! the reported `OutputLocation` — never a `Completed` claiming an output object that
//! does not exist.
//!
//! ## Shape of an operation
//!
//! Each operation is a struct that owns the shared engine and implements
//! [`smithy_server_runtime::Operation`]. The framework constructs one per service via
//! `InvokeEndpointAsync::new(engine)` and drives it through the centralized request walk:
//! deserialize → validate → `handle` → serialize. You write ONLY the `handle` body.
//!
//! ## Reaching this service's state
//!
//! The engine is held on `self` (constructor-injected), NOT passed per request — so
//! engine access is guaranteed by the time any request runs. Reach the store through
//! [`crate::state::service_state`] and read and write it through the guard it hands back:
//!
//! ```ignore
//!     let state = crate::state::service_state(&*self.engine)
//!         .ok_or(/* a modelled server error */)?;
//!     state.lock().your_field = /* ... */;
//! ```
//!
//! `ctx` ([`smithy_server_runtime::OperationContext`]) carries request metadata only —
//! headers, URI, method, and anything middleware placed in `ctx.parts().extensions`.
//! The engine is NOT reached through `ctx`.
//!
//! Shared types live in `super::types`; the error channel in `crate::scaffolded`.
//!
//! ---
//!
//! # Modeled contract for `InvokeEndpointAsync`
//!
//! What AWS documents for this operation, from this service's Smithy model. HTML is
//! stripped and long entries are clipped at ` …`. For the FULL text of any one item,
//! ask the model rather than reading the raw JSON:
//!
//! ```text
//! smithy_query(query="doc",shape="<Shape>",member="<Member>",model_path="services/service-com-amazonaws-sagemakerruntime-amazonsagemakerruntime/smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json")
//! ```
//!
//! After you deploy a model into production using Amazon SageMaker AI hosting services,
//! your client applications use this API to get inferences from the model hosted at the
//! specified endpoint in an asynchronous manner. Inference requests sent to this API are
//! enqueued for asynchronous processing. The processing of the inference request may or may
//! not complete before you receive a response from this API. The response from this API
//! will not contain the result of the inference request but contain information about where
//! you can locate it. Amazon SageMaker AI strips all POST headers except those supported by
//! the API. Amazon SageMaker AI might add additional headers. You should not rely on the
//! behavior of headers outside those enumerated in the request syntax. Calls to
//! InvokeEndpointAsync are authenticated by using Amazon Web Services Signature Version 4.
//! For information, see Authenticating Requests (Amazon Web Services Signature Version 4)
//! in the Amazon S3 API Reference.
//!
//! ## Input
//!
//! * `Accept` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The desired MIME type of the
//!   inference response from the model container.
//! * `Body` (AsyncBodyBlob, len 0..128000) — Provides inline input data for the inference
//!   request, in the format specified in the ContentType request header. Use this parameter
//!   to send the request payload directly in the API call instead of uploading it to Amazon
//!   S3 and referencing it with InputLocation. The inline payload can be up to 128,000 bytes.
//!   Body and InputLocation are mutually exclusive. Provide exactly one of them. For
//!   information about the format of the request body, see Common Data Formats-Inference.
//! * `ContentType` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The MIME type of the input
//!   data in the request body.
//! * `CustomAttributes` (CustomAttributesHeader, len 0..1024, pattern ^\p{ASCII}*$) —
//!   Provides additional information about a request for an inference submitted to a model
//!   hosted at an Amazon SageMaker AI endpoint. The information is an opaque value that is
//!   forwarded verbatim. You could use this value, for example, to provide an ID that you can
//!   use to track a request or to provide other metadata that a service endpoint was
//!   programmed to process. The value must consist of no more than 1024 visible US-ASCII
//!   characters as specified in Section 3.3.6. Field Value Components of the Hypertext
//!   Transfer Protocol (HTTP/1.1). The code in your model is responsible for setting or
//!   updating any custom attributes in the response. If your code does not set this value in
//!   the response, an …
//! * `EndpointName` (EndpointName, len 0..63, pattern ^[a-zA-Z0-9](-*[a-zA-Z0-9])*$)
//!   **required** — The name of the endpoint that you specified when you created the endpoint
//!   using the CreateEndpoint API.
//! * `Filename` (FilenameHeader, len 1..32, pattern
//!   ^(?!.*\..*\.)[a-zA-Z0-9][a-zA-Z0-9-_\.]*$) — The filename for the inference response
//!   payload stored in Amazon S3. If not specified, Amazon SageMaker AI generates a filename
//!   based on the inference ID.
//! * `InferenceId` (InferenceId, len 1..64, pattern ^\A\S[\p{Print}]*\z$) — The identifier
//!   for the inference request. Amazon SageMaker AI will generate an identifier for you if
//!   none is specified.
//! * `InputLocation` (InputLocationHeader, len 1..1024, pattern ^(https|s3)://([^/]+)/?(.*)$)
//!   — The Amazon S3 URI where the inference request payload is stored.
//! * `InvocationTimeoutSeconds` (InvocationTimeoutSecondsHeader, range 1..3600) — Maximum
//!   amount of time in seconds a request can be processed before it is marked as expired. The
//!   default is 15 minutes, or 900 seconds.
//! * `RequestTTLSeconds` (RequestTTLSecondsHeader, range 60..21600) — Maximum age in seconds
//!   a request can be in the queue before it is marked as expired. The default is 6 hours, or
//!   21,600 seconds.
//! * `S3OutputPathExtension` (S3OutputPathExtensionHeader, len 1..512, pattern
//!   ^(?!s3:|https:)[a-zA-Z0-9!_.*'()/-]+$) — The path extension that is appended to the
//!   Amazon S3 output path where the inference response payload is stored.
//!
//! ## Output
//!
//! * `FailureLocation` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The Amazon S3 URI where
//!   the inference failure response payload is stored.
//! * `InferenceId` (Header, len 0..1024, pattern ^\p{ASCII}*$) — Identifier for an inference
//!   request. This will be the same as the InferenceId specified in the input. Amazon
//!   SageMaker AI will generate an identifier for you if you do not specify one.
//! * `OutputLocation` (Header, len 0..1024, pattern ^\p{ASCII}*$) — The Amazon S3 URI where
//!   the inference response payload is stored.
//!
//! ## Errors
//!
//! * `InternalFailure` (HTTP 500) — An internal failure occurred.
//! * `ServiceUnavailable` (HTTP 503) — The service is unavailable. Try your call again.
//! * `ValidationError` (HTTP 400) — Inspect your request and try again.

#[allow(unused_imports)]
use super::types::*;

/// The SageMaker CONTROL PLANE, whose store owns every endpoint this data plane invokes;
/// reached through the one shared engine, so this service keeps no second copy of an
/// endpoint that could drift from it.
use ::service_com_amazonaws_sagemaker_sagemaker as sagemaker;

// `#[operation_error]` below expands to an invocation of the LOCAL declarative
// `smithy!` macro, which this line is what emits into the module. Without it the
// expansion fails with `cannot find macro 'smithy' in this scope` and the
// `InvokeEndpointAsyncError` enum is never produced.
smithy_macros::smithy_model!("smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json");

// The operation's MODELED errors. `#[operation_error]` accepts only variants the
// model declares for this operation, so the scaffold's `NotImplemented` cannot live
// here — hence the `InvokeEndpointAsyncError` alias below. An operation that models no errors
// generates an UNINHABITED enum here, which is correct and expected: there is nothing
// to construct until the model says otherwise.
#[smithy_macros::operation_error("com.amazonaws.sagemakerruntime#InvokeEndpointAsync")]
#[derive(Debug)]
pub enum InvokeEndpointAsyncErrorModeled {
    InternalFailure(InternalFailure),
    ServiceUnavailable(ServiceUnavailable),
    ValidationError(ValidationError),
}

/// This operation's error channel, as the generated `impl Operation` names it
/// (`crate::ops::invoke_endpoint_async::InvokeEndpointAsyncError`). Wrapping the modeled enum is what
/// lets a scaffolded operation answer 501 for one whose model declares no errors at all.
pub type InvokeEndpointAsyncError = crate::scaffolded::Scaffolded<InvokeEndpointAsyncErrorModeled>;

/// Lets `?` and `.into()` lift a modeled failure into the error channel, so an
/// implemented handler writes `Err(SomeModeledError.into())` rather than naming the
/// wrapper variant. See `crate::scaffolded` for how to drop the wrapper entirely once
/// this operation is implemented.
impl From<InvokeEndpointAsyncErrorModeled> for InvokeEndpointAsyncError {
    fn from(modeled: InvokeEndpointAsyncErrorModeled) -> Self {
        crate::scaffolded::Scaffolded::Modeled(modeled)
    }
}

/// The `InvokeEndpointAsync` operation.
///
/// Owns the shared engine (the constructor seam) and implements
/// [`smithy_server_runtime::Operation`]. Generic over the engine type `Eng`, so this
/// crate names no concrete engine type: a migrated service that needs state picks a
/// concrete engine where it mounts the service, not here.
pub struct InvokeEndpointAsync<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> InvokeEndpointAsync<Eng> {
    /// Construct the operation from the shared engine. INFALLIBLE — the engine is a
    /// construction-time dependency, so engine access is guaranteed by the time any
    /// request runs. Called by the generated `EmulatedService::new(engine)` constructor.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: ::std::marker::Send + ::std::marker::Sync + 'static> smithy_server_runtime::Operation
    for InvokeEndpointAsync<Eng>
{
    type Input = InvokeEndpointAsyncInput;
    type Output = InvokeEndpointAsyncOutput;
    type Error = InvokeEndpointAsyncError;

    const NAME: &'static str = "InvokeEndpointAsync";
    const INPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_ENDPOINT_ASYNC_INPUT_SCHEMA;
    const OUTPUT_SCHEMA: &'static ::aws_smithy_schema::Schema = super::types::INVOKE_ENDPOINT_ASYNC_OUTPUT_SCHEMA;
    const SUCCESS_STATUS: u16 = 202u16;

    async fn handle(
        &self,
        input: Self::Input,
        ctx: &smithy_server_runtime::OperationContext<'_>,
    ) -> Result<Self::Output, Self::Error> {
        // The same front-door decision every route of this data plane takes, and the first
        // thing it does: a principal whose identity-based policies do not allow
        // `sagemaker:InvokeEndpointAsync` on the endpoint the request addressed is refused
        // 403 `AccessDeniedException` before the request's input sources are looked at and
        // before the endpoint resolves, so no submission is queued and nothing is logged
        // (`crate::state::authorization`).
        if let Some(denial) = crate::state::authorization::refusal(
            &*self.engine,
            ctx,
            Self::NAME,
            &input.endpoint_name,
        ) {
            return Err(crate::scaffolded::Scaffolded::access_denied(denial));
        }

        let Some(log) = crate::state::service_state(&*self.engine) else {
            return Err(internal_failure(crate::state::NO_STATE));
        };

        let now = ::std::time::SystemTime::now();
        // Everything the clock already owes this service's own store is materialised
        // before this request reads or extends it: a queued submission whose outcome was
        // due has it whether or not anybody submitted another one.
        log.lock().settle_async_invocations(now);
        // "Endpoints are scoped to an individual account" (`InvokeEndpoint`'s model doc),
        // and this emulator serves the one account every emulated resource belongs to.
        let account = ::engine::EMULATOR_ACCOUNT_ID;

        // This submission, as the data-plane log replays it once its outcome is known.
        // The `@sensitive` members (the inline body, `CustomAttributes`) are deliberately
        // absent: only the body's SIZE is recorded. `effective_inference_id` is the id the
        // RESPONSE reported — the caller's, or the one this service generated — so the log
        // and the queued submission can be read against each other.
        let record = |outcome: crate::state::InvocationOutcome,
                      effective_inference_id: Option<String>| {
            let invocation = crate::state::EndpointInvocation {
                endpoint_name: input.endpoint_name.clone(),
                // This operation's input binds no `TargetVariant`.
                target_variant: None,
                request_bytes: input.body.as_deref().map_or(0, <[u8]>::len),
                content_type: input.content_type.clone(),
                accept: input.accept.clone(),
                inference_id: effective_inference_id,
                // Nor a `TargetModel`.
                target_model: None,
                session_id: None,
                outcome,
                at: now,
            };
            tracing::info!(
                operation = Self::NAME,
                endpoint = %invocation.endpoint_name,
                request_bytes = invocation.request_bytes,
                inference_id = invocation.inference_id.as_deref().unwrap_or_default(),
                outcome = outcome.label(),
                access_key_id = %::emulator_server::extract_akid(&ctx.parts().headers),
                "sagemaker runtime asynchronous invocation"
            );
            log.lock().record_invocation(invocation);
        };
        // Record the submission, then refuse it: a refused submission is still one the
        // data plane served, the way the real service counts it. A refusal reports no id
        // of its own, so the log keeps the caller's, if it sent one.
        let refuse = |outcome: crate::state::InvocationOutcome, message: String| {
            record(outcome, input.inference_id.clone());
            validation(message)
        };

        // "Body and InputLocation are mutually exclusive. Provide exactly one of them."
        // (`smithy_query(query="doc", shape="InvokeEndpointAsyncInput", member="Body")`).
        // An empty `@httpPayload` blob is indistinguishable on the wire from an absent
        // one, so an empty inline body counts as "not provided".
        let inline_body = input.body.as_deref().filter(|body| !body.is_empty());

        // Which input source the submission carries is a property of the REQUEST alone, so
        // it is decided before the endpoint the request addresses is resolved: a submission
        // that names no input source at all, or two, is refused for that whether or not the
        // account hosts the endpoint, and the refusal names the rule rather than the
        // endpoint. The two halves of the disjunction are separate refusals because they
        // are separate mistakes — nothing to send, and nothing to choose between.
        match (input.input_location.as_deref(), inline_body) {
            (Some(_), None) | (None, Some(_)) => {}
            (Some(_), Some(_)) => {
                return Err(refuse(
                    crate::state::InvocationOutcome::InputAmbiguous,
                    "Both Body and InputLocation were provided. Provide exactly one.".to_string(),
                ))
            }
            (None, None) => {
                return Err(refuse(
                    crate::state::InvocationOutcome::NoInput,
                    "One of Body or InputLocation is required".to_string(),
                ))
            }
        }

        // The inline source's SIZE is part of choosing it: "The inline payload can be up to
        // 128,000 bytes" is stated on the member itself
        // (`smithy_query(query="doc", shape="InvokeEndpointAsyncInput", member="Body")`),
        // the bound `AsyncBodyBlob` declares, so an inline payload past it is refused here,
        // with the request's other input-source refusals and before the endpoint resolves.
        // The clause and the bound are the framework's and the model's
        // (`crate::service::error_envelope::payload_over_maximum`), so this reads exactly as
        // a payload refused at the boundary would.
        if let Some(message) = crate::service::error_envelope::payload_over_maximum(
            Self::INPUT_SCHEMA,
            input.body.as_deref().map_or(0, <[u8]>::len),
        ) {
            return Err(refuse(
                crate::state::InvocationOutcome::PayloadOverMaximum,
                message,
            ));
        }

        // The endpoint is the control plane's resource, so it is resolved there and
        // nowhere else — the same resolver `InvokeEndpoint` uses, so the two operations
        // cannot disagree about which endpoints exist or serve traffic.
        let facts = match crate::state::resolve_endpoint(&*self.engine, &input.endpoint_name, now) {
            crate::state::EndpointLookup::NoControlPlane => {
                return Err(internal_failure(crate::state::NO_CONTROL_PLANE_STATE))
            }
            crate::state::EndpointLookup::NotFound => {
                // The one refusal the three routes state identically, discriminator and
                // all (`crate::scaffolded::Scaffolded::no_such_endpoint`). A refusal
                // reports no id of its own, so the log keeps the caller's, if it sent one.
                record(
                    crate::state::InvocationOutcome::EndpointNotFound,
                    input.inference_id.clone(),
                );
                return Err(crate::scaffolded::Scaffolded::no_such_endpoint(
                    &input.endpoint_name,
                ));
            }
            crate::state::EndpointLookup::Found(facts) => facts,
        };
        if facts.settled {
            // The transition is a state change of the control plane's resource, so it
            // leaves the same record there as the call that created the endpoint.
            sagemaker::state::record_state_change(Self::NAME, &facts.arn, ctx);
        }
        // The same serving rule `InvokeEndpoint` applies, stated once in
        // `crate::state::NON_SERVING_ENDPOINT_STATUSES`. This operation models no
        // not-ready error (only `ValidationError`, `ServiceUnavailable` and
        // `InternalFailure`), so an endpoint with no deployment behind it is a 400.
        if crate::state::NON_SERVING_ENDPOINT_STATUSES.contains(&facts.status.as_str()) {
            return Err(refuse(
                crate::state::InvocationOutcome::EndpointNotInService,
                format!(
                    "Endpoint {} of account {account} is not in service: its status is {}.",
                    input.endpoint_name, facts.status
                ),
            ));
        }

        // A submission can only be queued where the control plane configured somewhere to
        // put its result: the response's `OutputLocation` is computed from the endpoint's
        // own `AsyncInferenceConfig$OutputConfig$S3OutputPath`, so an endpoint without one
        // has nothing to answer with.
        let s3_output_path = match facts
            .async_output
            .as_ref()
            .and_then(|paths| paths.s3_output_path.clone())
        {
            Some(path) => path,
            None => {
                return Err(refuse(
                    crate::state::InvocationOutcome::AsyncInferenceNotConfigured,
                    format!(
                        "Endpoint {} of account {account} is not configured for asynchronous \
                         inference: its endpoint configuration carries no AsyncInferenceConfig \
                         OutputConfig S3OutputPath.",
                        input.endpoint_name
                    ),
                ))
            }
        };

        // A `Filename` the member's own modelled pattern does not admit is refused BEFORE
        // anything is written, so a refused submission consumes no identifier sequence —
        // the framework's regex-based constraint walk cannot enforce that pattern's
        // negative lookahead, so the value reaches here (see [`filename_is_modelled`]).
        if let Some(filename) = input.filename.as_deref() {
            if !filename_is_modelled(filename) {
                return Err(refuse(
                    crate::state::InvocationOutcome::OutputLocationInvalid,
                    filename_refusal(filename),
                ));
            }
        }

        // "Amazon SageMaker AI will generate an identifier for you if none is specified."
        // A generated one is minted by the workspace's shared mixer over this submission's
        // endpoint, instant and the store's monotonic submission sequence — taken and
        // advanced in one locked call — so two submissions never share an id.
        let inference_id = match input.inference_id.clone() {
            Some(id) => id,
            None => {
                let sequence = log.lock().next_submission_sequence();
                let since_epoch = now
                    .duration_since(::std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                ::service_common::stable_hex_id(&[
                    "sagemaker-async",
                    &input.endpoint_name,
                    &since_epoch.to_string(),
                    &sequence.to_string(),
                ])
            }
        };

        // The two locations are a pure function of the endpoint's configured paths, the
        // request's extension/filename and the inference id, so resubmitting one id yields
        // the SAME strings by construction rather than by answering from the record — and
        // every member the resubmission carries (TTL, timeout, extension, filename) takes
        // effect instead of being accepted and ignored. The model declares no uniqueness
        // rule for `InferenceId`, so nothing here refuses a resubmission.
        let output_location = output_location(
            &s3_output_path,
            input.s3_output_path_extension.as_deref(),
            input.filename.as_deref(),
            &inference_id,
        );
        // `FailureLocation` is reported only when the endpoint configures a failure path:
        // the member is optional, and inventing a location nothing is configured to write
        // to would send the caller looking for an object that cannot appear.
        let failure_location = facts
            .async_output
            .as_ref()
            .and_then(|paths| paths.s3_failure_path.as_deref())
            .map(|path| failure_location(path, &inference_id));

        // The record keeps WHERE the result will be, not a result: no container ran the
        // inference, so its outcome is the modelled failure state once it falls due (see
        // `crate::state::AsyncInvocation::settle`), and nothing is written to the reported
        // output location.
        log.lock()
            .queue_async_invocation(crate::state::AsyncInvocation {
                inference_id: inference_id.clone(),
                endpoint_name: input.endpoint_name.clone(),
                input_location: input.input_location.clone(),
                inline_input_bytes: inline_body.map(<[u8]>::len),
                content_type: input.content_type.clone(),
                accept: input.accept.clone(),
                output_location: output_location.clone(),
                failure_location: failure_location.clone(),
                request_ttl_seconds: input.request_ttl_seconds,
                invocation_timeout_seconds: input.invocation_timeout_seconds,
                status: crate::state::AsyncInvocationStatus::InProgress,
                failure_reason: None,
                submitted_at: now,
                // `checked_add`, so no stored duration can panic inside a handler.
                outcome_due_at: now
                    .checked_add(crate::state::ASYNC_OUTCOME_DUE_AFTER)
                    .unwrap_or(now),
            });
        record(
            crate::state::InvocationOutcome::Queued,
            Some(inference_id.clone()),
        );

        Ok(InvokeEndpointAsyncOutput {
            inference_id: Some(inference_id),
            output_location: Some(output_location),
            failure_location,
        })
    }
}

/// Where the response payload of one async invocation is stored, or the reason the
/// request's `Filename` cannot be used — naming the member and the constraint it broke,
/// since that is what the caller can correct.
///
/// The endpoint's configured `S3OutputPath` is the base; `S3OutputPathExtension` is "the
/// path extension that is appended to the Amazon S3 output path", so it becomes an extra
/// path segment; and the object is named by `Filename` when the caller supplies one, else
/// by the inference id ("If not specified, Amazon SageMaker AI generates a filename based
/// on the inference ID").
///
/// The extension's segments are spliced in as the member wrote them — `.` and `..`
/// included, since `S3OutputPathExtension`'s pattern
/// (`^(?!s3:|https:)[a-zA-Z0-9!_.*'()/-]+$`) admits them, an S3 key is the string it was
/// given and nothing here ever resolves the reported key. The one normalisation is
/// DELIBERATE and structural: a leading or trailing `/` and an empty segment are dropped,
/// so the extension joins the configured path with exactly one separator and the reported
/// key carries no `//`. A `Filename` the member's own modelled pattern does not admit is
/// refused by the caller BEFORE this is reached (see [`filename_is_modelled`] and
/// [`filename_refusal`]), so every value that gets here can be spliced.
fn output_location(
    s3_output_path: &str,
    extension: Option<&str>,
    filename: Option<&str>,
    inference_id: &str,
) -> String {
    let mut location = s3_output_path.trim_end_matches('/').to_string();
    if let Some(extension) = extension {
        let extension = extension.trim_matches('/');
        let segments: Vec<&str> = extension
            .split('/')
            .filter(|segment| !segment.is_empty())
            .collect();
        if !segments.is_empty() {
            location.push('/');
            location.push_str(&segments.join("/"));
        }
    }
    location.push('/');
    match filename {
        Some(filename) => location.push_str(filename),
        None => {
            location.push_str(inference_id);
            location.push_str(".out");
        }
    }
    location
}

/// The refusal sentence for a `Filename` [`filename_is_modelled`] rejects: it names the
/// member and the constraint it broke, which is what the caller can correct, in the
/// wording the framework's own constraint walk uses for a pattern. The expression is
/// quoted from [`FILENAME_PATTERN`] — the ONE place this crate holds it — exactly as the
/// model declares it, anchors included, which is how every `@pattern` clause of this
/// service quotes a member's own declaration
/// ([`crate::service::error_envelope::constraint_envelope`]).
fn filename_refusal(filename: &str) -> String {
    format!(
        "Filename {filename:?} failed to satisfy constraint: Member must satisfy regular \
         expression pattern: {FILENAME_PATTERN}"
    )
}

/// `Filename`'s modelled pattern, as `smithy_query(query="operation",
/// operation="InvokeEndpointAsync")` reports it — the single copy both the predicate that
/// enforces it ([`filename_is_modelled`]) and the refusal that quotes it
/// ([`filename_refusal`]) are written from, so a model change moves one string.
const FILENAME_PATTERN: &str = r"^(?!.*\..*\.)[a-zA-Z0-9][a-zA-Z0-9-_\.]*$";

/// Whether `filename` is a value `Filename`'s modelled pattern admits.
///
/// The pattern is [`FILENAME_PATTERN`], which is three rules: the first character is ASCII
/// alphanumeric, every character is ASCII alphanumeric or one of `-`, `_`, `.`, and the
/// negative lookahead admits at most ONE `.` in the whole value. Transcribed here because
/// the framework's constraint walk does not refuse a value that breaks the lookahead or
/// the first-character class, and the member's pattern is the rule for what may be spliced
/// into a reported S3 key. The member's `len 1..32` half is NOT re-checked here: the
/// framework's walk does enforce a length range, so this covers only what it cannot.
fn filename_is_modelled(filename: &str) -> bool {
    let mut characters = filename.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && characters
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        && filename.chars().filter(|character| *character == '.').count() <= 1
}

/// Where the failure payload of one async invocation is stored: the endpoint's configured
/// `S3FailurePath` plus the inference id's error object.
fn failure_location(s3_failure_path: &str, inference_id: &str) -> String {
    format!(
        "{}/{inference_id}-error.out",
        s3_failure_path.trim_end_matches('/')
    )
}

/// This operation's modelled client-input error (HTTP 400).
fn validation(message: String) -> InvokeEndpointAsyncError {
    InvokeEndpointAsyncErrorModeled::ValidationError(ValidationError {
        message: Some(message),
    })
    .into()
}

/// This operation's modelled 500 `InternalFailure`, for a fault of this process rather
/// than of the request.
fn internal_failure(message: impl Into<String>) -> InvokeEndpointAsyncError {
    InvokeEndpointAsyncErrorModeled::InternalFailure(InternalFailure {
        message: Some(message.into()),
    })
    .into()
}
