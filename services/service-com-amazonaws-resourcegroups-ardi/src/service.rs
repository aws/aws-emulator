// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-resourcegroups-ardi` service: how it claims requests and how it words the
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

use aws_smithy_schema_server::{Violation, ViolationKind};
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

mod error_message_member;
mod stated_error_bodies;

/// The `com-amazonaws-resourcegroups-ardi` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The same modelled operations the generated table holds, each wrapped so that the
    /// error bodies it answers with are the ones this service answers with
    /// ([`stated_error_bodies`]). Keyed by the routing key the protocol resolves — the key
    /// the generated table uses — and built from the SAME shared engine, so a wrapped
    /// operation reads and writes the one store.
    stated_error_bodies: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            stated_error_bodies: stated_error_bodies::stated_error_bodies(&engine),
            entry: crate::ServiceDefinition::new(engine),
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
        // NOT a re-route: the key is the one the generated table's own protocol resolves,
        // and the operation it names is the one served — wrapped so that every error body
        // reports its text in the member this model declares
        // ([`error_message_member`]). A key the model does not name resolves to `None`
        // here exactly as it does in the generated table, so the router's routing-error
        // path is unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.stated_error_bodies.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // Resource Groups words a request that violates the model's own rules — an
        // absent required member, a value outside a declared length, pattern or
        // range — as `BadRequestException`: "The request includes one or more
        // parameters that violate validation rules." (the model's own documentation
        // of that error). The framework's default code here is
        // `ValidationException`, which this service never returns, so the code is
        // replaced.
        //
        // The MESSAGE is the shared AWS wording of the constraint envelope — the
        // framework's own envelope, member paths and requirements, with the three
        // renderings AWS differs from the framework in: a `@pattern` requirement is
        // quoted as the expression the SERVICE validates against
        // ([`as_this_service_names_it`]) and without the whole-value anchors the model
        // wraps it in, and a clause about a value this service does not quote states no
        // value at all ([`states_no_value`]). Nothing about the envelope, the paths or
        // the other requirements is spelled here, so this service cannot drift from the
        // others stating the same sentence.
        //
        // An authentication refusal keeps the framework's wording: the emulator, not
        // this service, decides how a caller is refused.
        match refusal {
            Refusal::Invalid(violations) => {
                let stated = Refusal::Invalid(
                    violations.iter().map(as_this_service_names_it).collect(),
                );
                Some(Refused {
                    code: BAD_REQUEST_EXCEPTION.to_string(),
                    message:
                        ::service_common::validation_message::aws_validation_refusal_withholding_violations(
                            &stated,
                            states_no_value,
                        )
                        .map(|stated| stated.message)
                        .unwrap_or_else(|| BAD_REQUEST_MESSAGE.to_string()),
                })
            }
            _ => None,
        }
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

/// The MODELLED error this service answers a request that violates its input shape with:
/// `BadRequestException`, documented as "The request includes one or more parameters that
/// violate validation rules." — exactly this refusal class, and the code every operation of
/// this service declares for it.
const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What this service says when the refusal carries no violation to state at all — the
/// modelled shape's own sentence, since there is no member to name.
const BAD_REQUEST_MESSAGE: &str =
    "The request includes one or more parameters that violate validation rules.";

/// Whether the clause for `violation` states no value — true for a `@pattern` requirement,
/// where this service's refusals report the expression the member's value had to match and
/// not the value that missed it, and false for every other constraint class, whose clauses
/// do quote the value the request carried.
///
/// It is asked about the VIOLATION rather than about a member path because the answer is a
/// property of the constraint CLASS rather than of one member: the same member quotes its
/// value for a `@range` or `@length` requirement and withholds it for a `@pattern` one
/// (`service_common::validation_message::aws_validation_refusal_withholding_violations`).
fn states_no_value(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::Pattern(_))
}

/// The ARN segments this service's own validator writes WIDER than its published model
/// does, as (what the model declares, what the service names) pairs.
///
/// A Resource Groups ARN admits the service's non-production stages in its service segment
/// and a region prefix of up to four letters, neither of which the published model spells:
/// the model writes `resource-groups` and `[a-z]{2}` where the service validates
/// `resource-groups(-(test|beta|gamma))?` and `[a-z]{2,4}`. A refusal therefore quotes
/// those segments as the service names them.
///
/// A segment is a property of the EXPRESSION rather than of one shape, one member or one
/// request, so the rule holds for every ARN pattern of this model — `TagSyncTaskArn`,
/// `GroupArn`, `GroupArnV2`, `GroupStringV2`, `ApplicationArn` — and for every operation
/// that takes a member reaching one. The CHECK is unaffected: it is still the model's
/// pattern as written, so only the quoting changes.
const NAMED_BY_THE_SERVICE: [(&str, &str); 2] = [
    (":resource-groups:", ":resource-groups(-(test|beta|gamma))?:"),
    (r"[a-z]{2}(-[a-z]+)+-\d{1}", r"[a-z]{2,4}(-[a-z]+)+-\d{1}"),
];

/// `violation` as this service states it: a `@pattern` requirement carrying the expression
/// the SERVICE validates against ([`NAMED_BY_THE_SERVICE`]), and every other violation
/// untouched.
///
/// Only the expression moves. The member path, the value half, the requirement's wording,
/// the anchor rendering and the envelope all stay with the one place that writes them, so
/// this service cannot word a requirement differently from any other.
fn as_this_service_names_it(violation: &Violation) -> Violation {
    let ViolationKind::Pattern(declared) = &violation.kind else {
        return violation.clone();
    };
    let mut stated = declared.clone();
    for (model_names, service_names) in NAMED_BY_THE_SERVICE {
        stated = stated.replace(model_names, service_names);
    }
    Violation {
        kind: ViolationKind::Pattern(stated),
        ..violation.clone()
    }
}
