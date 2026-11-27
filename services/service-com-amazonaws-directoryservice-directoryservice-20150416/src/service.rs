// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-directoryservice-directoryservice-20150416` service: how it claims requests and how it words the
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
//! One of those seams is taken here, for the reason named in [`request_id_header`]: a
//! refusal of this service states the id of the call it answers in its document, and AWS
//! reports that same id in the response's `x-amzn-requestid` header, which the framework's
//! error renderers do not write. `override_operation` wraps the operation the routing
//! table names, so the header reaches a handler-returned modelled error, the scaffold's
//! 501 and a refusal raised while serving it. Everything else about those paths — status,
//! `__type`, `Message`, the whole success path — stays with the framework.
//!
//! This service's model declares `awsJson1_1` and `rpcv2Cbor` side by side, so a client may
//! call every operation in either and each request is served in the one it arrived in. The
//! wrapper therefore resolves the route with the protocol the framework selected for THAT
//! request rather than with the first the model declares: the two name their operation in
//! different places (a header, the URI), so a key resolved with the other codec names no
//! operation and the request would be served without the wrapper's header or the refusals
//! below.
//!
//! A second seam is taken for the three things this service words differently from the
//! framework when it refuses a request at its boundary: see the `override_refusal` body
//! below, [`stated_enum_values`] (the value set an enum clause lists, and its order),
//! [`published_patterns`] (the expression a clause quotes for a member whose published
//! expression is not the model's own text, per shape) and [`stated_clause_order`] (the order the
//! clauses of one refusal are stated in). A
//! refusal of a constraint a LIST's ELEMENTS break is stated at the list member instead of at
//! the element, which needs the request as well as the violations and so is answered in that
//! same operation wrapper ([`list_element_refusals`], whose nested requirement set is
//! closed per [`stated_element_sets`]) — and so is the one CONSTRAINT the
//! framework's own walk cannot apply, a `@pattern` whose look-around its regex engine does
//! not compile ([`model_pattern_refusals`]).
//!
//! [`authorization`] is not a seam at all: it is the IAM decision the operations whose
//! model declares `AccessDeniedException` take before they touch the store, held beside
//! this file because it reads the IAM and STS stores rather than this service's own.

pub mod authorization;
pub mod list_element_refusals;
pub mod model_pattern_refusals;
pub mod published_patterns;
pub mod published_value_sets;
pub mod request_id_header;
pub mod stated_clause_order;
pub mod stated_element_sets;

use stated_clause_order::{in_stated_clause_order, reorders};

use request_id_header::RequestIdHeaderOperation;
use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service, Violation,
    ViolationKind,
};
use std::sync::Arc;

/// The error code the framework answers a boundary constraint refusal with on this
/// service's protocol, restated here so a reworded message always travels with the code a
/// client branches on (`protocol_error_response_with` in
/// `vendor/smithy-server-runtime/src/pre_dispatch.rs` picks it for `ProtocolError::Invalid`).
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// An enum's admissible wire values as a refusal of this service LISTS them: the value set
/// the SERVICE publishes for the member's target type, in the order the front door lists a
/// set in.
///
/// A validation clause states the value set as the remediation hint a caller is shown
/// (`Member must satisfy enum value set: [<values>]`), and both halves of that list are the
/// service's rather than the model's — which values it holds and how each is spelled, and the
/// order they are listed in. Both are [`published_value_sets`]', which reads them off the
/// values themselves and off this service's own published sets, so they belong to the
/// enumerated TYPE and not to a member, an operation or a depth: one type is listed alike
/// wherever it is carried, and a type the model gains needs a row nowhere.
fn stated_enum_values(allowed: &[String]) -> Vec<String> {
    published_value_sets::published_value_set(allowed)
}

/// `refusal` stated the way this service states a boundary constraint refusal, or `None`
/// to keep the framework's own wording.
///
/// The only things restated are the three renderings this service's own refusals differ from
/// the framework's in — the value set an enum clause lists ([`stated_enum_values`]), the
/// expression a `@pattern` clause quotes ([`published_patterns`]) and the order
/// the clauses of one refusal are stated in ([`stated_clause_order`]); the envelope, the
/// clause, the member path, the echoed value and every other requirement are the framework's,
/// built by handing it the violations back ([`::aws_smithy_schema_server::envelope`]), so a
/// reworded refusal cannot drift from one the framework words. A refusal with neither
/// difference to state is left alone entirely.
fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    let Refusal::Invalid(violations) = refusal else {
        return None;
    };
    let lists_an_enum = violations
        .iter()
        .any(|violation| matches!(violation.kind, ViolationKind::EnumValue(_)));
    let restates_a_pattern = violations.iter().any(|violation| match &violation.kind {
        ViolationKind::Pattern(declared) => published_patterns::restates(declared),
        _ => false,
    });
    if !lists_an_enum && !restates_a_pattern && !reorders(violations) {
        return None;
    }
    let restated: Vec<Violation> = in_stated_clause_order(violations)
        .into_iter()
        .map(stated_violation)
        .collect();
    Some(Refused {
        code: VALIDATION_EXCEPTION.to_string(),
        message: ::aws_smithy_schema_server::envelope(&restated)?,
    })
}

/// `violation` as this service STATES it: an enum's admissible values LISTED as the value set
/// this service publishes for the member's target type ([`stated_enum_values`]), with
/// everything else — its member path, its
/// echoed value, a `@pattern` expression, its sensitivity and every other constraint class —
/// exactly as the framework found it.
///
/// A `@pattern` is quoted as the expression this service PUBLISHES for the member's shape
/// ([`published_patterns`]): the model's text, anchors and all, for almost every shape of
/// this model — the `^`/`$` a Smithy `@pattern` wraps a whole-value expression in are part
/// of the expression a recorded Directory Service refusal quotes back, per-member clause and
/// nested element set alike — and, for the few shapes whose recorded refusal publishes the
/// grammar alone, that text without its closing anchor or without either of them. The same
/// holds for a clause built
/// where the framework's own walk could not apply the expression at all
/// ([`model_pattern_refusals`]) and for the one a handler words itself
/// (`crate::ops::create_alias`), so one rendering serves every member whatever found it.
///
/// THE one place that restating is made, so the three paths it is applied on (the boundary
/// refusal this file words, the list-element clause [`list_element_refusals`] words, and the
/// skipped-`@pattern` clause [`model_pattern_refusals`] words) cannot disagree about it.
fn stated_violation(violation: &Violation) -> Violation {
    let kind = match &violation.kind {
        ViolationKind::EnumValue(allowed) => ViolationKind::EnumValue(stated_enum_values(allowed)),
        ViolationKind::Pattern(declared) => {
            ViolationKind::Pattern(published_patterns::published_pattern(declared))
        }
        other => other.clone(),
    };
    Violation {
        path: violation.path.clone(),
        kind,
        value: violation.value.clone(),
        sensitive: violation.sensitive,
    }
}

/// The `com-amazonaws-directoryservice-directoryservice-20150416` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The wrapper every routable request is served through, so the answer carries the
    /// request id its document states; see [`request_id_header`].
    request_id_header: RequestIdHeaderOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
            request_id_header: RequestIdHeaderOperation,
        }
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn ErasedServiceDefinition {
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
        // Every request the generated routing table can serve is served through the
        // header wrapper, which delegates to that same table entry and then reports the
        // answer's own request id in the header AWS carries it in (see
        // [`request_id_header`]). The routing stays the model's: the key is resolved with
        // the protocol this request is served in — this service declares two — and a
        // request whose key names no modelled operation is declined here, so the framework
        // answers it as the unroutable request it is.
        let routes = self
            .entry
            .wire_protocol(req)
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some();
        routes.then_some(&self.request_id_header as _)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this service
        // reword it.
        //
        // THREE renderings are reworded, inside the framework's own envelope, clause, member
        // path and echoed value: a value outside an enumerated type's value set has the
        // admissible values LISTED as the value set this service publishes for that type,
        // in the order its front door lists a set in (see `stated_enum_values`), a clause
        // about a value outside its
        // member's `@pattern` quotes the expression this service PUBLISHES for that shape
        // (see `published_patterns`), and a refusal about several members
        // states its clauses in the order this service states them in (see
        // `stated_clause_order`). The code travels with it, unchanged from the one the
        // framework picks for this refusal, so a client still branches on what it branched on
        // before.
        //
        // Everything else — an absent member, a length, a numeric bound, and an
        // authentication refusal, which is not an input-constraint failure
        // at all — keeps the framework's wording.
        stated_refusal(refusal)
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
