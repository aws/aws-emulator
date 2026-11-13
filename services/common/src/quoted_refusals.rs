// SPDX-License-Identifier: CC0-1.0

//! A refusal whose clause QUOTES the value the request carried, as a wrapper any service
//! can serve its operations behind.
//!
//! # The rule
//!
//! An AWS constraint clause states the value that broke the constraint as the request
//! carried it — `Value '<the submitted value>' at '<member>' failed to satisfy
//! constraint: …` — however long that value was. The framework's own constraint walk
//! does not: it renders the value into the clause through a ceiling of its own (a
//! byte-bounded prefix plus `... [<n> bytes]`,
//! `vendor/smithy-schema-server/src/constraints.rs`), which is wire text AWS never
//! emits. A caller comparing the echo against what it sent then cannot find its own
//! value in it, and a caller that fixes the parameter from the clause is shown a value
//! it never submitted.
//!
//! # Why this cannot live in `Service::override_refusal`
//!
//! That seam is handed the violations ALONE — by then the value the framework elided is
//! gone, and the request it came from is out of reach. Restoring it takes the request, so
//! it has to happen where the request still is: around the operation. [`QuotedRefusals`]
//! is that wrapper. It re-reads the request through the framework's OWN machinery
//! ([`crate::validation::refusing_violations`] — the protocol's deserializer and
//! `constraints::validate` over the built input), so it cannot find a different set of
//! violations from the walk that follows it, and every bound on what one clause may quote
//! is that module's.
//!
//! The wording itself is NOT this wrapper's: the refusal is rendered through the service's
//! own [`Service::override_refusal`] and its generated envelope, exactly as the walk would
//! have rendered it. Only the clause's value half changes, and only where the framework
//! elided it.
//!
//! # Services still carrying their own copy
//!
//! The restoring rule predates this wrapper and is inlined in the service crates that
//! needed it before there was anywhere shared to put the wrapper itself
//! (`grep -rln 'quoting_the_whole_submitted_value' services/*/src` lists them). Those
//! copies are to be MIGRATED onto this module; nothing new should inline it again.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::ProtocolError;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, Operation, OperationContext, Refusal, Service, Violation,
};

/// One modelled operation, served with every constraint clause of its refusals quoting the
/// value the request carried.
///
/// Nothing else is re-implemented: a request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error.
///
/// The cost is DELIBERATE, and the same one [`crate::front_door`] states: a request that
/// IS refused is read twice, once here and once by the walk, because the framework hands
/// no already-built input to an operation. Both reads are the framework's own, over bytes
/// its body limits already bound. The second read is what a framework that handed the
/// built input to an operation would remove — until it does, the collecting walk is the
/// only place a clause's value half can be restored from.
pub struct QuotedRefusals<O> {
    operation: O,
    /// Collects the violations of one request against the input schema of the operation
    /// BEHIND this wrapper — `O`'s own, or (for an operation already wrapped in another
    /// layer, which is no longer an [`Operation`]) the one named to [`Self::over`].
    collect: Collect,
}

/// How a wrapper collects the violations that refuse one request: the framework's own
/// walk, bound to one operation's input schema.
type Collect = fn(&dyn smithy_server_runtime::ServerHttpProtocol, &RtRequest) -> Vec<Violation>;

impl<O: Operation> QuotedRefusals<O> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            collect: crate::validation::refusing_violations::<O>,
        }
    }
}

impl<O> QuotedRefusals<O> {
    /// Wrap an operation that is already wrapped in another layer — so it is an
    /// [`ErasedOperation`] but no longer an [`Operation`] — naming the operation `S` whose
    /// input schema its violations are collected against.
    pub fn over<S: Operation>(operation: O) -> Self {
        Self {
            operation,
            collect: crate::validation::refusing_violations::<S>,
        }
    }

    /// How this service refuses `req`, with each clause quoting the value the request
    /// carried — or `None` when `req` breaks no modelled constraint, which leaves it to
    /// the wrapped operation.
    fn refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        let definition = svc.definition();
        let violations = (self.collect)(definition.protocol(), req);
        if violations.is_empty() {
            return None;
        }
        // The service's own wording, asked exactly as the framework's walk asks it, so a
        // refusal this wrapper renders and one the walk renders cannot differ in anything
        // but the clause's value half.
        let chosen = svc.override_refusal(&Refusal::Invalid(violations.clone()));
        Some(definition.refusal_response(ProtocolError::Invalid(violations), chosen, req))
    }
}

impl<O> ErasedOperation for QuotedRefusals<O>
where
    O: ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The definition is read, used and dropped inside this one call:
            // `&dyn ErasedServiceDefinition` is not `Sync`, so holding it across the await
            // below would make this future non-`Send`.
            if let Some(response) = self.refusal(svc, req) {
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}
