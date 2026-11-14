// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ssmquicksetup-quicksetup` service: how it claims requests and how it words the
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
//!   correctly. EDITED here: a request addressing a resource ARN the caller is not entitled
//!   to is refused by the front door (see below) rather than dispatched to the operation its
//!   route names.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. EDITED here: a modelled-constraint refusal is worded the
//!   way AWS words it on a JSON protocol.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. EDITED here: such a request is refused by the
//!   front door described below, rather than with the framework's own
//!   unimplemented-route answer.
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
//! ## A route's target resource: the ARN's entitlement is decided in front, its resolution is not
//!
//! Several routes of this endpoint carry a resource ARN as a path label — the manager's
//! own `/configurationManager/{ManagerArn}`, the tag plane's `/tags/{ResourceArn}`, and
//! `/configurationDefinition/{ManagerArn}/{Id}`, whose label is the ARN of the manager
//! that OWNS the definition the route addresses. An ARN names the ACCOUNT that owns the
//! resource, so one thing about such a request IS decided in front of the handlers
//! ([`target_entitlement`]): whether the ARN is a resource ARN of this service belonging to
//! the account the request is signed for. A request naming anything else addresses a
//! resource that is not the caller's and is refused by the front door with
//! [`UNAUTHORIZED_STATUS`] / [`UNAUTHORIZED_CODE`] / [`UNAUTHORIZED_MESSAGE`], before any
//! handler and before the modelled constraint walk.
//!
//! Whether a resource ANSWERS to an ARN the caller IS entitled to is a RESOURCE outcome,
//! never an authorization one, so nothing stands between such a request and its handler:
//! each operation resolves the ARN it was given against this service's state and answers its
//! own modelled `ResourceNotFoundException` (HTTP 404) when nothing answers to it — the same
//! shape, with the same sentence, as for an ARN a request carries in its BODY
//! (`ListConfigurations`) or for the configuration id `GetConfiguration`'s label carries. A
//! manager that has been torn down, one this endpoint never minted, and one another Region's
//! endpoint minted are all that 404.
//!
//! A consequence of there being nothing ELSE in front of the handlers: a path label the
//! caller IS entitled to that falls outside the `@pattern` the model states for it is refused
//! by the modelled constraint walk, in the `400 ValidationException` envelope every other
//! malformed input earns. The tag plane's `ResourceArn` carries no `@pattern` and no `@length`
//! at all, so any value the route matches and the caller is entitled to reaches the handler
//! and is resolved there.
//!
//! ## The front door's answer to a request that names no route
//!
//! A request this service claims but that resolves to NO modelled route is refused by the
//! front door with HTTP [`UNAUTHORIZED_STATUS`], the wire error code
//! [`UNAUTHORIZED_CODE`] and the one fixed sentence [`UNAUTHORIZED_MESSAGE`]: it names no
//! operation, so nothing of this service's own — no modelled error, and no report about
//! the emulator — is what it earns. Two shapes of request land there, and the refusal is
//! one rule over both:
//!
//! * a method and path that match no route of this endpoint at all;
//! * a method and path that match one, while the request carries no value for a `@required`
//!   query or header member that route binds — the operation could not be invoked without
//!   it, so the route does not match it either. `UntagResource`'s `TagKeys` is the case
//!   this service has: `@required` and bound to the `tagKeys` query parameter, so an SDK
//!   asked to remove an EMPTY set of keys sends `DELETE /tags/{ResourceArn}` with no query
//!   at all, and that request names no route.
//!
//! [`Service::override_routing_error`] is the seam the framework offers for exactly this,
//! and it is what [`EmulatedService`] answers it on, so the refusal is stated once for
//! every route rather than guessed at per operation — and a handler behind the front door
//! may rely on having been reached only by a request that named its route.

pub mod target_entitlement;

use target_entitlement::UnauthorizedTargetOperation;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use aws_smithy_schema::{Schema, ShapeId, ShapeType};
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

/// The wire error code (`__type` and `x-amzn-errortype`) of the front door's refusal of a
/// request that names no modelled route.
///
/// Not a shape this service's model declares, and not an answer any modelled operation
/// has: a request that named no operation is refused ahead of the code that owns the
/// modelled errors, so the refusal travels with its own code and status rather than
/// through an operation's error enum.
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The HTTP status that refusal is served on.
pub const UNAUTHORIZED_STATUS: u16 = 401;

/// The one sentence that refusal states, for every request it refuses.
///
/// Wholly fixed text: a request that named no route reports no member, no value and no
/// resource, so there is nothing in it to derive from the request.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The `message` member of the synthetic shape the front door's refusal body is rendered
/// from.
///
/// Lowercase `message`, which is what the layer in front of the service names it —
/// unlike this service's own modelled `ResourceNotFoundException` / `ValidationException`,
/// whose single member is `Message`. `with_xml_name` keeps the member correct on the XML
/// envelopes too, as the synthetic shapes in [`crate::scaffolded`] do.
static UNAUTHORIZED_MESSAGE_MEMBER: Schema = Schema::new_member(
    ShapeId::from_static(
        "smithy.framework#UnroutableRequestRefusal$message",
        "smithy.framework",
        "message",
    ),
    ShapeType::String,
    "message",
    0,
)
.with_xml_name("Message");

/// Schema for that synthetic shape.
///
/// In the `smithy.framework` namespace for the same reason as the shapes in
/// [`crate::scaffolded`]: it is in no service's model, so it must not collide with a
/// modelled one. The schema shapes only the BODY; the wire error code and the status
/// travel beside it.
static UNAUTHORIZED_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "smithy.framework#UnroutableRequestRefusal",
        "smithy.framework",
        "UnroutableRequestRefusal",
    ),
    ShapeType::Structure,
    &[&UNAUTHORIZED_MESSAGE_MEMBER],
);

/// The body of the front door's refusal: the fixed sentence and nothing else.
#[derive(Debug, Clone, Copy)]
struct Unauthorized;

impl SerializableStruct for Unauthorized {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(&UNAUTHORIZED_MESSAGE_MEMBER, UNAUTHORIZED_MESSAGE)?;
        Ok(())
    }
}

/// The front door's refusal, rendered through this service's own protocol so it carries
/// the same envelope (and the same `x-amzn-errortype` header) as every other error this
/// service answers. The status and the error code come from the refusal itself: the shape
/// is not in the model, so there is no modelled error to read them off.
pub(crate) fn unauthorized(svc: &dyn Service, req: &RtRequest) -> RtResponse {
    svc.definition()
        .protocol()
        .serialize_error(
            &Unauthorized,
            &UNAUTHORIZED_SCHEMA,
            UNAUTHORIZED_CODE,
            UNAUTHORIZED_STATUS,
        )
        .unwrap_or_else(|error| svc.definition().framework_error_response(error, req))
}

/// The `com-amazonaws-ssmquicksetup-quicksetup` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The refusal served for a request addressing a resource ARN the caller is not entitled
    /// to. Held in a field, built once, because `override_operation` hands back a reference
    /// to it.
    unauthorized_target: UnauthorizedTargetOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            unauthorized_target: UnauthorizedTargetOperation,
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

    fn override_operation(&self, req: &RtRequest) -> Option<&dyn ErasedOperation> {
        // The one routing decision the model's route table cannot make: a request whose path
        // label carries a resource ARN the caller is not entitled to — another account's, or a
        // value that is no resource ARN of this service at all — is stopped by the front door
        // (see [`target_entitlement`]) instead of being dispatched to the operation its route
        // names. Every other request resolves through the generated routing table, which is
        // this service's only other operation source.
        target_entitlement::is_refused(req).then_some(&self.unauthorized_target as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent required
        // member, or a value outside a declared bound) to let this service reword it.
        //
        // This model is restJson1, where AWS states a constraint refusal as the
        // `400 ValidationException` envelope the framework already renders —
        // `N validation error(s) detected: Value '<value>' at '<member path>' failed to
        // satisfy constraint: <requirement>` — except that it quotes a `@pattern`
        // requirement WITHOUT the `^`/`$` anchor pair a whole-value Smithy pattern wraps
        // the expression in. That rendering is shared by every service on a JSON protocol,
        // so it lives in `service-common` rather than here; the CHECK still uses the
        // model's pattern as written, so only the way the expression is quoted back
        // changes. The value half stays the framework's, which is what keeps a `@sensitive`
        // member's value out of the sentence.
        ::service_common::validation_message::aws_validation_refusal(refusal)
    }

    fn override_routing_error(&self, req: &RtRequest) -> Option<RtResponse> {
        // The router calls this when a request reached this service but named no modelled
        // operation — the generated routing table did not resolve it, because its method
        // and path match no route of this endpoint, or the route they match binds a
        // `@required` query or header value the request does not carry, without which that
        // operation cannot be invoked.
        //
        // Such a request is stopped by the front door (see the module docs) with the
        // status, the wire error code and the one fixed sentence that layer states for
        // everything it refuses. The framework's default would instead report this
        // emulator's own unimplemented-route answer, which is not an outcome this service
        // has — every operation it models is implemented, so a request that names no route
        // is one its front door never admits rather than one nobody has written.
        Some(unauthorized(self, req))
    }

    fn override_framework_error(
        &self,
        error: &aws_smithy_schema_server::ProtocolError,
    ) -> Option<RtResponse> {
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
