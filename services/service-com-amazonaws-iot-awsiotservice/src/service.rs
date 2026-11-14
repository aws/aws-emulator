// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iot-awsiotservice` service: how it claims requests and how it words the
//! failures the framework hands back.
//!
//! EDIT THIS FILE. It is emitted once with working defaults and is never
//! regenerated, so a choice made here survives adding another service or
//! re-running the generator.
//!
//! `EmulatedService` is what the router mounts (`EmulatedService::new(engine)`).
//! It wraps the generated [`ServiceDefinition`], which carries the model-derived half
//! of this service — its identity, its wire protocol, and its operation routing
//! table — built from the Smithy model at compile time. You do not edit the
//! entry; you edit the behavioral seams below.
//!
//! Five opt-in seams are yours to change; every one defaults to the framework's
//! own behaviour, so an unedited service still routes and answers correctly:
//!
//! - `can_handle` — which requests this service claims. The default matches the
//!   request's resolved route key against this service's signing name. Edit it
//!   for anything the model does not express: a signing name shared by several
//!   services, or routing that keys on the access-key id.
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. The default returns `None`, so every request resolves through the
//!   generated routing table. Return `Some(op)` to route one yourself.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. See the inline note on that method for how to reword.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.
//!
//! ## The error shape a request-validation refusal names
//!
//! AWS IoT names `InvalidRequestException` in `x-amzn-errortype` QUALIFIED with the
//! service's API doc namespace ([`IOT_DOC_NAMESPACE`]) — the shape name, a `:`, and the
//! namespace — which is the form a restJson1 client resolves an error shape from. Every
//! OTHER error shape of this service is named bare, which is what the framework's
//! serializer already writes.
//!
//! `override_operation` below therefore serves the operations that refuse with that shape
//! — a value outside what the service recognises, and a precondition of the plane that
//! owns the resource — behind [`ValidationRefusalErrorType`], which qualifies the shape
//! name on the way out and changes nothing else about the response.

use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as WireRequest, Response as WireResponse};

/// The header a restJson1 client resolves an error SHAPE from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The error shape AWS IoT refuses a request its validation layer will not accept with.
const INVALID_REQUEST_EXCEPTION: &str = "InvalidRequestException";

/// The namespace AWS IoT's validation layer qualifies an error-shape name with in
/// [`ERROR_TYPE_HEADER`], appended after a `:`.
///
/// It is the service's published API doc namespace, a fixed string: nothing of a request
/// or of this emulator's state goes into it.
const IOT_DOC_NAMESPACE: &str = "http://iot.amazonaws.com/doc/2015-10-02/";

/// One modelled operation whose request-validation refusal is named the way AWS IoT's
/// validation layer names it: the error shape qualified with [`IOT_DOC_NAMESPACE`].
///
/// Nothing else is re-implemented — the request is served by the generated
/// [`ErasedOperation::run`], which owns deserialize -> validate -> handle -> serialize and
/// every modelled error — and nothing but the one header is touched, so a success, a
/// not-found and a server fault answer exactly what they answered before.
pub struct ValidationRefusalErrorType<O> {
    operation: O,
}

impl<O> ValidationRefusalErrorType<O> {
    /// Serve `operation`, already constructed from the shared engine, behind this
    /// wrapper.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O: ErasedOperation> ErasedOperation for ValidationRefusalErrorType<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r WireRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, WireResponse> {
        Box::pin(async move {
            let mut response = self.operation.run(svc, req, ctx).await;
            if response.headers().get(ERROR_TYPE_HEADER) == Some(INVALID_REQUEST_EXCEPTION) {
                response
                    .headers_mut()
                    .insert(ERROR_TYPE_HEADER, qualified_invalid_request());
            }
            response
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `InvalidRequestException` as AWS IoT's validation layer names it in
/// [`ERROR_TYPE_HEADER`]: the shape name, a `:`, and the service's API doc namespace.
fn qualified_invalid_request() -> String {
    format!("{INVALID_REQUEST_EXCEPTION}:{IOT_DOC_NAMESPACE}")
}

/// The `com-amazonaws-iot-awsiotservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The fleet-index reader, served behind [`ValidationRefusalErrorType`]: the
    /// `indexName` this service does not recognise is refused `InvalidRequestException`,
    /// so the refusal names that shape the way this service names it.
    describe_index: ValidationRefusalErrorType<crate::ops::describe_index::DescribeIndex<Eng>>,
    /// The authorizer delete, served behind [`ValidationRefusalErrorType`] for the same
    /// reason: an authorizer the account still has in service is refused
    /// `InvalidRequestException`.
    delete_authorizer:
        ValidationRefusalErrorType<crate::ops::delete_authorizer::DeleteAuthorizer<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            // The SAME shared engine the entry's own operations were constructed from,
            // so the wrapped reader reads the one store.
            describe_index: ValidationRefusalErrorType::new(
                crate::ops::describe_index::DescribeIndex::new(Arc::clone(&engine)),
            ),
            delete_authorizer: ValidationRefusalErrorType::new(
                crate::ops::delete_authorizer::DeleteAuthorizer::new(engine),
            ),
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // Claim a request whose resolved route key is this service's signing
        // name. Edit this for anything the model does not express — a signing
        // name shared by several services, or access-key-id-based routing.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // The operations that refuse with `InvalidRequestException` are served behind
        // `ValidationRefusalErrorType` (see the module docs), which names that shape the
        // way this service names it: qualified with its API doc namespace. Every other
        // request resolves through the generated routing table.
        let key = self.entry.protocol().routing_key(req).ok()?;
        if key == crate::ops::describe_index::DescribeIndex::<Eng>::NAME {
            return Some(&self.describe_index as &dyn ErasedOperation);
        }
        if key == crate::ops::delete_authorizer::DeleteAuthorizer::<Eng>::NAME {
            return Some(&self.delete_authorizer as &dyn ErasedOperation);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording. To state a refusal the way the emulated service states it,
        // replace this body with a `match refusal { .. }` that returns
        // `Some(Refused { code, message })` for the cases you reword and `None`
        // for the rest. Replace BOTH the code and the message: a client branches
        // on the code, so a message without its matching code is worse than
        // leaving both alone.
        let _ = refusal;
        None
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation — neither `override_operation` nor the generated
        // routing table resolved it. Returning `None` renders the protocol-correct
        // default for this service's envelope (query -> 400 InvalidAction text/xml,
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501). Return `Some(resp)` to
        // word that routing failure in your own envelope instead.
        let _ = req;
        None
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this on a transport-level failure it detects up front —
        // before (or instead of) naming an operation — such as a malformed
        // `aws-chunked` body. Returning `None` keeps the entry's protocol-correct
        // rendering. Return `Some(resp)` to reword that failure in your own
        // envelope. The error is borrowed, so the default renderer can still
        // consume it on `None`.
        let _ = error;
        None
    }
}
