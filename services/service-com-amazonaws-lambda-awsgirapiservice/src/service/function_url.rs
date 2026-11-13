// SPDX-License-Identifier: CC0-1.0

//! The FUNCTION URL data plane: how a request to the endpoint `CreateFunctionUrlConfig`
//! handed out is served by the function that URL is attached to.
//!
//! A function URL is "a dedicated HTTP(S) endpoint that you can use to invoke your
//! function", published at `https://<url-id>.lambda-url.<region>.on.aws/`. It is NOT a
//! modelled operation — no routing key names it — so it is served through the same two
//! seams the download plane uses: [`claims_parts`] in `Service::can_handle`, so the router
//! does not answer `UnknownService` for an address this service published, and
//! [`FunctionUrlPlane`] in `Service::override_operation`, which serves it.
//!
//! What it serves is one INVOCATION of the function the URL is attached to, through the
//! same in-process engine every other invocation runs on
//! ([`crate::state::run_invocation`]): the request becomes the documented payload-format-2.0
//! event, the handler RUNS, and the response is what the handler's result describes (see
//! [`event`]). Nothing is answered that the function did not produce — a run that raised, a
//! run that timed out, a result that describes no response and a response past the payload
//! ceiling are all the `502` Lambda answers for them, carrying [`BAD_GATEWAY_MESSAGE`] and
//! nothing of the function's own error.
//!
//! ## Which caller is admitted
//!
//! `FunctionUrlConfig$AuthType` decides: "`AWS_IAM` – Restrict access to authenticated
//! users only" and "`NONE` – Bypass IAM authentication to create a public endpoint". The
//! difference is carried by the SCHEMES the plane serving the request declares, not by a
//! check in this module — the emulator verifies every request signature and a service never
//! verifies one itself — so the plane is mounted twice, once per auth type
//! ([`FunctionUrlPlane::anonymous`] and [`FunctionUrlPlane::aws_iam`]), and
//! `override_operation` picks the one the addressed URL's stored `AuthType` names. A URL
//! whose `AuthType` is `AWS_IAM` is therefore served only to a request whose SigV4
//! signature the emulator verified — against this service's own signing name, so a
//! signature scoped to another service, an unsigned request and a request whose body was
//! changed after signing are all refused `403` by the framework before this plane runs —
//! and the caller that signature speaks for is reported in the event's
//! `requestContext.authorizer.iam`.
//!
//! A `NONE` URL bypasses that authentication, and what makes such an endpoint PUBLIC is the
//! function's own resource-based policy: a grant of the function-URL invocation to everyone.
//! That is the one admission decision this plane makes itself ([`admits_the_public`]),
//! because it is a property of the stored POLICY rather than of the request's signature — a
//! policy whose grants all name a principal, and a function with no policy at all, admit no
//! anonymous caller and are refused `403 {"Message":"Forbidden"}`.

//! ## What this plane reads of the configuration, and what it does not
//!
//! It reads the two members that decide WHO is served and WHAT serves them: `AuthType` and
//! the qualifier the URL is attached to, and `FunctionUrlConfig$Cors` — "The cross-origin
//! resource sharing (CORS) settings for your function URL" — which is the ENDPOINT's own
//! setting and so is applied here rather than by the handler ([`cors`]): a CORS PREFLIGHT is
//! answered by this plane itself with the permission the configuration grants and NOTHING
//! invoked, and the answer to every other request carries that permission for an origin the
//! configuration admits. A request whose origin it does not admit is served exactly as one
//! carrying no origin is — the headers are what a browser enforces with, so nothing of the
//! function's answer is withheld for them. `InvokeMode` is read only as the stored value the
//! control plane reports: a `RESPONSE_STREAM` URL is served through the same BUFFERED
//! mapping, because the response-streaming half of a function URL (the handler wrapper that
//! writes a stream, and the metadata prelude that shapes it) is a runtime capability this
//! emulator's engines do not supply yet.
//!
//! A preflight carries an `Origin`, and the emulator's front door refuses a request whose
//! browser-set headers say a browser sent it from another site before any operation of this
//! service runs. That guard stays in front of this plane: a preflight it admits is answered
//! here, and one it refuses never reaches this plane at all — a property of this emulator,
//! which serves a developer's machine, rather than of the service it emulates.

use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{BoxFuture, ErasedOperation, OperationContext, Service};

/// The event one request becomes and the response one result becomes — the documented
/// payload format 2.0; see the module's own header.
pub mod event;

/// What the endpoint's own `Cors` settings make of a request, and what it answers a
/// browser's PREFLIGHT; see the module's own header.
mod cors;

use event::{mapped_result, url_event, CallerIdentity, Mapped, UrlRequest};

/// The body of every `502` a function URL answers.
///
/// Lambda does not pass the function's own failure through to the caller — the client is
/// told the gateway got no usable response, and the function's error is in the function's
/// log — so one sentence covers every class of it.
const BAD_GATEWAY_MESSAGE: &str = "Internal Server Error";

/// The status Lambda answers when the function produced no usable response.
const BAD_GATEWAY_STATUS: u16 = 502;

/// The status this plane answers when the INVOCATION never happened at all, because this
/// emulator holds nothing it could run for the function.
///
/// It is not a `502`: a `502` says the function answered unusably, and nothing ran here.
/// The body names the reason, which is the same diagnostic an asynchronous delivery records
/// when a run cannot be started ([`crate::state::async_invocation`]).
const NOT_RUN_STATUS: u16 = 500;

/// The largest response a function URL serves is not stated here: it is the RUN's own
/// bound. "The maximum payload size is 6 MB for synchronous invocations" (the model's own
/// `Invoke` documentation) bounds what a handler may answer at all, so a result past it is
/// stopped where it is produced ([`crate::state::execution`]) and reaches this plane as the
/// function error it became — answered by the same [`bad_gateway`] every other unusable
/// response is. One place states the ceiling.
///
/// The REQUEST half of that quota is this plane's, because a function URL request becomes
/// the event the function is invoked with: "the maximum payload size is 6 MB for synchronous
/// invocations", and a function URL invokes synchronously, so a body larger than that is one
/// no invocation can carry and is refused before the event is built from it.
const MAX_REQUEST_BYTES: usize = 6 * 1024 * 1024;

/// The status and the sentence a request whose body is past [`MAX_REQUEST_BYTES`] is
/// refused with: the payload is larger than the invocation it would become can carry.
const TOO_LARGE_STATUS: u16 = 413;
/// See [`TOO_LARGE_STATUS`].
const TOO_LARGE_MESSAGE: &str = "Request Entity Too Large";

/// What a request whose body this plane cannot read at all is told: the invocation carries
/// the request's bytes, so there is no event to build and nothing runs.
const UNREADABLE_BODY_MESSAGE: &str =
    "the request's body could not be read, so no invocation was made for it";

/// The header a function URL response reports its request id in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The address a request that names none came from: this emulator is reached in-process or
/// over the loopback interface, which is where a request with no forwarded address came
/// from.
const LOOPBACK_SOURCE_IP: &str = "127.0.0.1";

/// The header a proxy records the client's own address in, whose FIRST hop is the client.
const FORWARDED_FOR_HEADER: &str = "x-forwarded-for";

/// The function URL `addressed` names, out of the one shared store.
///
/// The ADDRESS is read first, with no guard taken: a request that addresses no function URL
/// at all — every control-plane call, and every other service's traffic, which this runs for
/// on the way to `can_handle`'s answer — is answered before the store is touched.
pub(super) fn target_of<Eng: Send + Sync + 'static>(
    engine: &Eng,
    addressed: &crate::state::AddressedFunctionUrl<'_>,
) -> Option<crate::state::FunctionUrlTarget> {
    let named = crate::state::function_url::addressed_url(addressed)?;
    let state = crate::state::service_state(engine)?;
    let store = state.lock();
    crate::state::function_url::held_function_url(&store, &named)
}

/// Which callers a mounted plane serves, which is what its declared auth schemes say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Admits {
    /// `AuthType=NONE`: "Bypass IAM authentication to create a public endpoint", so the
    /// request is served however it arrived — while the function's resource-based policy
    /// grants the invocation to everyone ([`admits_the_public`]).
    Everyone,
    /// `AuthType=AWS_IAM`: "Restrict access to authenticated users only", so the request is
    /// served only once the emulator has verified its SigV4 signature.
    SignedCallers,
}

/// The function URL data plane: one invocation of the function a URL is attached to.
///
/// Mounted once per `AuthType` ([`Admits`]), because which callers a URL admits is carried
/// by the auth schemes the serving operation declares.
pub struct FunctionUrlPlane<Eng> {
    /// The ONE shared engine this service's operations read and write, so a request served
    /// here sees exactly the functions the control plane persisted.
    engine: Arc<Eng>,
    /// Which callers this mounting serves.
    admits: Admits,
}

impl<Eng: Send + Sync + 'static> FunctionUrlPlane<Eng> {
    /// The plane serving the URLs whose `AuthType` is `NONE`.
    pub fn anonymous(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            admits: Admits::Everyone,
        }
    }

    /// The plane serving the URLs whose `AuthType` is `AWS_IAM`.
    pub fn aws_iam(engine: Arc<Eng>) -> Self {
        Self {
            engine,
            admits: Admits::SignedCallers,
        }
    }

    /// Whether THIS plane serves the request these parts came from — the half of
    /// `Service::can_handle` that claims a function URL.
    ///
    /// Narrowed to a function URL this service HOLDS, read from the one shared store, so
    /// nothing is claimed that this service cannot answer about and exactly one service
    /// claims a request.
    pub fn claims(&self, parts: &smithy_server_runtime::http::request::Parts) -> bool {
        target_of(
            &*self.engine,
            &crate::state::AddressedFunctionUrl::of_parts(parts),
        )
        .is_some()
    }

    /// The function URL the runtime-API request `req` addresses, or `None` when it
    /// addresses none this service holds.
    pub fn target(&self, req: &RtRequest) -> Option<crate::state::FunctionUrlTarget> {
        target_of(
            &*self.engine,
            &crate::state::AddressedFunctionUrl::of_request(req),
        )
    }

    /// Whether this mounting is the one that serves `auth_type`.
    pub fn serves_auth_type(&self, auth_type: &str) -> bool {
        match self.admits {
            Admits::SignedCallers => auth_type == AUTH_TYPE_AWS_IAM,
            Admits::Everyone => auth_type != AUTH_TYPE_AWS_IAM,
        }
    }

    /// Serve one request: run the function the URL is attached to with the event the
    /// request becomes, and answer what the run produced — with the endpoint's own CORS
    /// permission on the answer, and ANSWERING a preflight itself rather than running
    /// anything for it (see [`cors`]).
    async fn serve(&self, req: &RtRequest, ctx: &OperationContext<'_>) -> RtResponse {
        let Some(target) = self.target(req) else {
            // The URL was resolved when the request was CLAIMED; a configuration deleted
            // between the two is a URL that no longer serves, and nothing of the function
            // is answered for it.
            return plane_error(
                NOT_FOUND_STATUS,
                "The function URL this request addressed does not exist.",
            );
        };
        // The endpoint's own `Cors` settings, which decide the answer to a preflight whole
        // and decorate the answer to everything else. A URL configured with none states no
        // permission at all, and its preflight is a request like any other.
        if let Some(configured) = target.cors.as_ref() {
            if let Some(asked) = cors::preflight_of(req) {
                return cors::answered_preflight(configured, &asked);
            }
        }
        let permitted = target
            .cors
            .as_ref()
            .map_or_else(Vec::new, |configured| cors::decoration(configured, req));
        let mut response = self.invoked(&target, req, ctx).await;
        cors::decorate(&mut response, &permitted);
        response
    }

    /// The answer the INVOCATION of the function `target`'s URL is attached to produces —
    /// every answer of this plane except the preflight the endpoint answers itself.
    async fn invoked(
        &self,
        target: &crate::state::FunctionUrlTarget,
        req: &RtRequest,
        ctx: &OperationContext<'_>,
    ) -> RtResponse {
        let request_id = crate::state::mint_request_id();
        // The request's own bytes, read ONCE: the event the function is invoked with carries
        // them, so a body this plane cannot read is a request it cannot turn into an
        // invocation at all — reported as the one it could not make rather than invoked with
        // an empty body the caller did not send.
        let Some(body) = req.body().bytes() else {
            ::tracing::warn!(
                request_id = %request_id,
                "a function URL request carries a body this emulator cannot read, so nothing \
                 ran"
            );
            return plane_error(NOT_RUN_STATUS, UNREADABLE_BODY_MESSAGE);
        };
        // The event this request becomes is the invocation's payload, so the quota on that
        // payload bounds the request: a body past it is refused before anything is built
        // from it (see [`MAX_REQUEST_BYTES`]).
        if body.len() > MAX_REQUEST_BYTES {
            return plane_error(TOO_LARGE_STATUS, TOO_LARGE_MESSAGE);
        }
        let incoming = Incoming {
            req,
            body,
            request_id: &request_id,
            received: ::std::time::SystemTime::now(),
        };
        let mut prepared = match self.prepare(target, &incoming, ctx) {
            Ok(prepared) => prepared,
            Err(response) => return *response,
        };
        // A function whose environment variables are encrypted with a customer managed key
        // is only invoked while that key can decrypt them, exactly as on `Invoke`: the
        // invocation begins with that decrypt, and a key that cannot be used means nothing
        // ran.
        if let Some(fault) = crate::state::environment_key_fault(
            ctx.callback(),
            &prepared.invocation.region,
            prepared.kms_key_arn.as_deref(),
            &prepared.invocation.variables,
        )
        .await
        {
            ::tracing::warn!(
                function = %prepared.invocation.function_name,
                detail = ?fault,
                "a function URL request reached a function whose environment variables cannot \
                 be decrypted, so nothing ran"
            );
            return bad_gateway(&request_id);
        }
        // A function URL request is an invocation of the function, so it draws on the SAME
        // concurrency pool every other invocation does — "concurrency settings apply to the
        // function as a whole, including all published versions and the unpublished version"
        // (`PutFunctionConcurrency`). It is admitted the way `Invoke`'s is, holds its slot
        // until the run ends however it ends, and a request the pool has no room for is
        // answered as the throttle it is with nothing run
        // (`crate::state::execution_slot`).
        let _slot = match crate::state::service_state(&*self.engine).map(|state| {
            crate::state::admit_invocation(
                &state,
                &prepared.invocation.function_name,
                &prepared.qualifier,
                ::std::time::SystemTime::now(),
            )
        }) {
            Some(Ok(admitted)) => {
                prepared.invocation.initialization_type = admitted.initialization_type;
                // Admitting the invocation raised the function's concurrency, which is
                // published as one reading of that gauge exactly as on `Invoke`
                // (`crate::state::concurrency_metering`).
                crate::state::publish_function_concurrency(
                    &*self.engine,
                    &prepared.invocation.function_name,
                    admitted.concurrent_executions,
                    ::std::time::SystemTime::now(),
                );
                Some(admitted.slot)
            }
            Some(Err(throttled)) => {
                ::tracing::info!(
                    request_id = %request_id,
                    function = %prepared.invocation.function_name,
                    reason = %throttled.reason,
                    "a function URL request found the function's concurrency pool full, so \
                     nothing ran"
                );
                // Answered the throttle, so metered as one request the service throttled,
                // exactly as on `Invoke` (`crate::state::metrics`).
                crate::state::publish_throttle(
                    &*self.engine,
                    &prepared.invocation.function_name,
                    &prepared.resource,
                );
                return plane_error(THROTTLED_STATUS, &throttled.message);
            }
            None => None,
        };
        // The invocation is admitted to the function at this point, so it is metered as one
        // `Invocations` datapoint of the function and of the resource the URL addressed,
        // before the run is attempted: the count is of invocation requests that reached the
        // function, and says nothing about what the execution produced.
        crate::state::publish_invocation(
            &*self.engine,
            &prepared.invocation.function_name,
            &prepared.resource,
        );
        let invoked = match crate::state::run_invocation(prepared.invocation).await {
            // No run slot of the substrate came free inside this request's whole wait: the
            // capacity that was full is the one a function with no reservation draws on, so
            // the request is answered as the throttle it is rather than as a gateway failure
            // of a run that was never attempted.
            Err(crate::state::Unrunnable::Overloaded(message)) => {
                ::tracing::warn!(
                    request_id = %request_id,
                    "a function URL request found no capacity to run the function, so nothing \
                     ran"
                );
                return plane_error(THROTTLED_STATUS, &message);
            }
            Ok(invoked) => invoked,
            Err(why) => {
                ::tracing::warn!(
                    detail = ?why,
                    "a function URL request could not be run by this emulator"
                );
                return plane_error(NOT_RUN_STATUS, &unrunnable_reason(&why));
            }
        };
        // "An unhandled error" is the function's own failure, which a function URL does not
        // pass through: the caller is told the gateway got no usable response, and the error
        // stays in the run's log.
        if invoked.function_error.is_some() {
            ::tracing::info!(
                request_id = %request_id,
                "a function URL invocation ended in a function error, answered as a bad gateway"
            );
            return bad_gateway(&request_id);
        }
        match mapped_result(&invoked.payload) {
            Mapped::Response {
                status,
                headers,
                body,
            } => served(status, &headers, body, &request_id),
            Mapped::Malformed(why) => {
                ::tracing::info!(
                    request_id = %request_id,
                    detail = %why,
                    "a function URL response describes no response, answered as a bad gateway"
                );
                bad_gateway(&request_id)
            }
        }
    }

    /// Everything the run needs, read out of the store under one guard: the invocation
    /// itself, the resource the metering is charged to, and the key the function's
    /// environment variables are encrypted with.
    ///
    /// The guard lives in this call and no longer — the run below it happens with no guard
    /// held, so one caller's handler cannot stall another caller's request.
    fn prepare(
        &self,
        target: &crate::state::FunctionUrlTarget,
        incoming: &Incoming<'_>,
        ctx: &OperationContext<'_>,
    ) -> Result<Prepared, Box<RtResponse>> {
        let Some(state) = crate::state::service_state(&*self.engine) else {
            return Err(Box::new(plane_error(
                NOT_RUN_STATUS,
                "Lambda state is not reachable on this engine",
            )));
        };
        let now = incoming.received;
        let request_id = incoming.request_id;
        let mut store = state.lock();
        // A URL whose `AuthType` is `NONE` is "a public endpoint", and what makes it public
        // is the function's own resource-based policy: a grant of the function-URL
        // invocation to everyone. A policy that grants it to a NAMED principal — or no
        // policy at all — admits no anonymous caller, which is the one refusal this plane
        // makes itself, because it is a property of the stored POLICY and not of the
        // request's signature (see [`admits_the_public`]).
        if self.admits == Admits::Everyone
            && !admits_the_public(&store, &target.function_name, &target.qualifier)
        {
            ::tracing::info!(
                function = %target.function_name,
                "a function URL whose AuthType is NONE is addressed, and its function's \
                 resource-based policy grants the invocation to nobody public"
            );
            return Err(Box::new(forbidden()));
        }
        // A function URL is attached to the unpublished version or to an ALIAS, and the
        // alias names the version it invokes AT THE TIME OF THE CALL, so the version is
        // resolved here, on every request, by the one resolution every invoking path shares.
        let qualifier = (target.qualifier != crate::state::VERSION_LATEST)
            .then(|| target.qualifier.clone());
        let Ok((record, version)) = crate::state::resolve_function(
            &mut store,
            &target.function_name,
            qualifier.as_deref(),
            // The region the caller ADDRESSED, which the published endpoint host carries;
            // an addressing that carries none resolves under this service's default.
            target
                .addressed_region
                .as_deref()
                .unwrap_or(crate::state::DEFAULT_REGION),
            now,
        ) else {
            return Err(Box::new(plane_error(
                NOT_FOUND_STATUS,
                "The function this URL is attached to does not exist.",
            )));
        };
        // "During this time, you can't invoke or modify the function" (`CreateFunction`): a
        // request that arrives inside the provisioning window reaches no instance of the
        // function, so the URL has no response to serve.
        if record.state != crate::state::STATE_ACTIVE {
            ::tracing::info!(
                function = %target.function_name,
                state = %record.state,
                "a function URL request reached a function that cannot be invoked yet"
            );
            return Err(Box::new(bad_gateway(request_id)));
        }
        // Lambda's recursive-loop guardrail decides whether this invocation happens at all,
        // exactly as it does on `Invoke`: an invocation through a function URL is an
        // invocation of the function.
        let chain = crate::state::request_chain(ctx);
        if crate::state::guardrail(record, chain.as_deref(), now) == crate::state::Guardrail::Stopped
        {
            let function_name = record.config.name.clone();
            drop(store);
            crate::state::publish_recursive_invocation_dropped(&*self.engine, &function_name);
            return Err(Box::new(bad_gateway(request_id)));
        }
        let function_name = record.config.name.clone();
        // The version this invocation resolved to: a URL attached to an alias that splits its
        // invocations between two versions resolves per invocation
        // (`crate::state::alias_routing`), so the configuration and the package the run uses
        // are the ones of the version it was routed to.
        let executed_version = record
            .routed_version(&version)
            .unwrap_or_else(|| version.clone());
        let resolved_config = record
            .config_of(&executed_version)
            .cloned()
            .unwrap_or_else(|| record.config.clone());
        let invocation = crate::state::Invocation {
            function_name: function_name.clone(),
            executed_version: executed_version.clone(),
            invoked_function_arn: record.versioned_arn(&version),
            region: record.region.clone(),
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
            payload: self.event_for(target, incoming, ctx, &record.region),
            // A function URL request carries no `ClientContext` and no `TenantId`: both are
            // members of the `Invoke` API, which this plane is not.
            client_context: None,
            tenant_id: None,
            request_id: request_id.to_string(),
            // The X-Ray trace context this invocation runs with: the chain the client
            // named on its own request, under the rule the function's `TracingConfig` mode
            // gives (`crate::state::execution::trace_context`).
            trace: Some(crate::state::invocation_trace_context(
                crate::state::supplied_trace_header(ctx).as_deref(),
                &resolved_config.tracing_mode,
                ::std::time::SystemTime::now(),
            )),
            // Which kind of environment serves it is the admission's to decide, once this
            // store guard is released (`crate::state::execution_slot`).
            initialization_type: crate::state::execution_slot::INITIALIZATION_TYPE_ON_DEMAND,
            // The AWS SDK this runtime supplies the handler reaches the rest of this
            // emulator through the callback the framework handed THIS request, signing
            // every call the handler makes as the session of the function's own execution
            // role (`crate::state::execution_role_session`).
            aws: crate::state::function_sdk_access(
                &*self.engine,
                ctx,
                &record.region,
                &resolved_config.role,
                &function_name,
            ),
            // Where the run's log is delivered, and what the function's execution role may
            // do there (`crate::state::execution::log_delivery`): an invocation through a
            // function URL is an invocation of the function, so it logs like one.
            logs: Some(crate::state::log_target(
                &*self.engine,
                &record.region,
                &resolved_config,
            )),
            // What this invocation is metered by once it has run, exactly as on `Invoke`:
            // the time the run took and the failure it came to
            // (`crate::state::metrics`).
            metrics: Some(crate::state::InvocationMetrics::new(
                &*self.engine,
                &function_name,
                &crate::state::invoked_resource(&function_name, &version),
            )),
        };
        Ok(Prepared {
            resource: crate::state::invoked_resource(&function_name, &version),
            qualifier: version,
            kms_key_arn: resolved_config.kms_key_arn.clone(),
            invocation,
        })
    }

    /// The payload-format-2.0 event this request becomes.
    fn event_for(
        &self,
        target: &crate::state::FunctionUrlTarget,
        incoming: &Incoming<'_>,
        ctx: &OperationContext<'_>,
        region: &str,
    ) -> Vec<u8> {
        let req = incoming.req;
        let headers: Vec<(String, String)> = req
            .headers()
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        let content_type = req.headers().get("content-type").map(str::to_string);
        let source_ip = req
            .headers()
            .get(FORWARDED_FOR_HEADER)
            .and_then(|forwarded| forwarded.split(',').next())
            .map(str::trim)
            .filter(|hop| !hop.is_empty())
            .unwrap_or(LOOPBACK_SOURCE_IP)
            .to_string();
        let domain_name = function_url_host(region, &target.url_id);
        let request = UrlRequest {
            method: req.method(),
            raw_path: &target.request_path,
            raw_query: raw_query(req.uri()),
            headers: &headers,
            body: incoming.body,
            content_type: content_type.as_deref(),
            url_id: &target.url_id,
            domain_name: &domain_name,
            account_id: ::engine::emulator_account_id(),
            request_id: incoming.request_id,
            source_ip: &source_ip,
            received: incoming.received,
            iam: self.caller(ctx),
        };
        url_event(&request)
    }

    /// The caller a request was signed by, for the mounting that serves `AWS_IAM` URLs.
    ///
    /// The signature itself was verified by the emulator before this plane ran, so what is
    /// reported here is the identity THAT credential speaks for: the access key id the
    /// request presented (the public half, never a secret) and the account's own principal,
    /// which is the one long-term credential this emulator publishes
    /// ([`service_common::caller_principal`]). The `NONE` mounting reports no caller at
    /// all, because an anonymous endpoint authorizes none.
    fn caller(&self, ctx: &OperationContext<'_>) -> Option<CallerIdentity> {
        if self.admits != Admits::SignedCallers {
            return None;
        }
        Some(CallerIdentity {
            access_key: ::engine::extract_akid(&ctx.parts().headers),
            account_id: ::engine::emulator_account_id().to_string(),
            user_arn: ::service_common::caller_principal::ambient_user_arn(),
            user_id: ::service_common::caller_principal::ambient_user_id(),
        })
    }
}

/// The `AuthType` whose URLs are served to authenticated callers only.
const AUTH_TYPE_AWS_IAM: &str = "AWS_IAM";

/// The status a request the function's concurrency pool had no room for is answered with:
/// the throttle AWS answers an invocation past a concurrency limit with.
const THROTTLED_STATUS: u16 = 429;

/// The status a request addressing a function URL this store no longer holds is answered
/// with: the endpoint names nothing.
const NOT_FOUND_STATUS: u16 = 404;

/// One request as the plane read it off the wire: the request itself, the bytes of its body
/// (read once), the id this request and its invocation share, and the instant it arrived —
/// so one request has ONE instant and one reading of its body however many steps serve it.
struct Incoming<'r> {
    /// The request the plane was handed.
    req: &'r RtRequest,
    /// Its body's bytes.
    body: &'r [u8],
    /// This request's id.
    request_id: &'r str,
    /// When it arrived.
    received: ::std::time::SystemTime,
}

/// Everything one function URL request needs once the store's guard is released.
struct Prepared {
    /// The invocation to run.
    invocation: crate::state::Invocation,
    /// The resource this invocation is metered against.
    resource: String,
    /// The version label the URL addressed — the alias the URL is attached to, or the
    /// unpublished version — which the invocation is admitted against.
    qualifier: String,
    /// The key the function's environment variables are encrypted with, when it has one.
    kms_key_arn: Option<String>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for FunctionUrlPlane<Eng> {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let response = self.serve(req, ctx).await;
            ::tracing::info!(
                service = ctx.identity().signing_name(),
                access_key_id = ::engine::extract_akid(&ctx.parts().headers),
                status = response.status().as_u16(),
                "served a function URL request"
            );
            response
        })
    }

    /// The schemes this mounting's callers sign with: a URL whose `AuthType` is `AWS_IAM`
    /// is served only to a request the emulator verified a SigV4 signature for, and a URL
    /// whose `AuthType` is `NONE` is "a public endpoint", served however it arrived.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        match self.admits {
            Admits::SignedCallers => Some(&[SIGV4_SCHEME]),
            Admits::Everyone => Some(&[]),
        }
    }
}

/// The Smithy auth scheme a SigV4-signed request presents.
const SIGV4_SCHEME: &str = "aws.auth#sigv4";

/// The query a request carries, without its `?`, and empty for a request carrying none.
fn raw_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The host the function URL `url_id` publishes in `region`, which the event reports as
/// `requestContext.domainName` — read off the ONE spelling of a function URL endpoint host
/// ([`crate::state::function_url::function_url_endpoint_host`]), so the host the event names
/// is the endpoint the control plane handed the caller.
fn function_url_host(region: &str, url_id: &str) -> String {
    crate::state::function_url::function_url_endpoint_host(region, url_id)
}

/// One served response: the status, headers and body the handler's result described.
///
/// The headers are appended through the NON-panicking appender: a header the mapping already
/// held to what a response can carry ([`event::mapped_result`]) cannot fail here, and a
/// response this one could not carry is answered as the unusable response it is rather than
/// by a panic on the request's path.
fn served(
    status: u16,
    headers: &[(String, String)],
    body: Vec<u8>,
    request_id: &str,
) -> RtResponse {
    let mut response = RtResponse::new(plane_status(status), SdkBody::from(body));
    for (name, value) in headers {
        // A header the result names more than once — `set-cookie`, one per cookie — is
        // answered once per value, so every cookie the function set reaches the client.
        if response
            .headers_mut()
            .try_append(name.clone(), value.clone())
            .is_err()
        {
            ::tracing::info!(
                request_id = %request_id,
                "a function URL response names a header no response can carry, answered as a \
                 bad gateway"
            );
            return bad_gateway(request_id);
        }
    }
    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id.to_string());
    response
}

/// The `502` every class of unusable response is answered with: AWS's own sentence, and
/// nothing of the function's failure.
fn bad_gateway(request_id: &str) -> RtResponse {
    let mut response = plane_error(BAD_GATEWAY_STATUS, BAD_GATEWAY_MESSAGE);
    response
        .headers_mut()
        .insert(REQUEST_ID_HEADER, request_id.to_string());
    response
}

/// One refusal of a function URL request, in the `{"message": …}` envelope a function URL
/// answers with: the endpoint serves no modelled operation, so there is no modelled error
/// to render.
fn plane_error(status: u16, message: &str) -> RtResponse {
    let body = ::serde_json::json!({ "message": message }).to_string();
    let mut response = RtResponse::new(plane_status(status), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", event::DEFAULT_CONTENT_TYPE);
    response
}

/// The refusal an endpoint gives a caller it does not admit: `403 {"Message":"Forbidden"}`.
///
/// The member is spelled `Message` here and `message` on the failures above because that is
/// how the two are answered — the authorization refusal of a Lambda front door carries the
/// capitalised member, and nothing of the request or the policy is named in it.
///
/// It is the ONE wording of that refusal, shared with the seam that words the refusal of a
/// request the emulator's authentication layer turned away before this plane could run
/// ([`super::function_url_refusal`]), so an endpoint cannot refuse the same caller two ways.
pub(super) fn forbidden() -> RtResponse {
    let body = ::serde_json::json!({ "Message": FORBIDDEN_MESSAGE }).to_string();
    let mut response = RtResponse::new(plane_status(FORBIDDEN_STATUS), SdkBody::from(body));
    response
        .headers_mut()
        .insert("content-type", event::DEFAULT_CONTENT_TYPE);
    response
}

/// The status and the sentence a caller the endpoint does not admit is refused with.
const FORBIDDEN_STATUS: u16 = 403;
/// See [`FORBIDDEN_STATUS`].
const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// Whether the function's resource-based policy makes the function URL of `qualifier` a
/// PUBLIC endpoint — the condition `AuthType=NONE` serves an anonymous caller under.
///
/// `FunctionUrlAuthType=NONE` means "Bypass IAM authentication to create a public
/// endpoint", and what grants the public that invocation is a statement of the function's
/// resource-based policy allowing everyone: a policy whose grants all name a principal, and
/// a function carrying no policy for that qualifier at all, admit nobody anonymous. The
/// reading of "allows everyone" is this service's ONE definition of it
/// ([`super::public_access::grants_public_access`]), the same one `GetPolicy` reports
/// `PublicAccessAllowed` from, so a caller cannot be told the policy is public and then
/// refused by it.
///
/// A policy is attached to the ARN the permission named, so the policy read is the one of
/// the qualifier the URL is attached to — the function-level policy for a URL on the
/// unpublished version, and the alias's own for a URL on an alias.
fn admits_the_public(
    store: &crate::state::ComAmazonawsLambdaAwsgirapiserviceState,
    function_name: &str,
    qualifier: &str,
) -> bool {
    store
        .functions
        .get(function_name)
        .and_then(|function| function.policies.get(qualifier))
        .is_some_and(|policy| super::public_access::grants_public_access(&policy.to_json()))
}

/// A status this plane answers with. Every call site passes a constant or a status
/// [`mapped_result`] already held to the HTTP range, so the conversion cannot fail.
fn plane_status(status: u16) -> StatusCode {
    StatusCode::try_from(status).unwrap_or_else(|_| {
        StatusCode::try_from(BAD_GATEWAY_STATUS).expect("a constant status is a valid one")
    })
}

/// Why the function a function URL request addressed never ran.
///
/// The sentences are the ONE set this crate has for a run that could not be made
/// ([`crate::state::async_invocation::unrunnable_reason`], which the asynchronous queue
/// records on its own entry and which a durable execution fails with); only the SUBJECT
/// differs, which is this request rather than a queued event.
fn unrunnable_reason(why: &crate::state::Unrunnable) -> String {
    crate::state::async_invocation::unrunnable_reason(why, THIS_REQUEST)
}

/// What the reason sentences above call the work that never ran, here.
const THIS_REQUEST: &str = "this request";
