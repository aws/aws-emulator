// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-cleanroomsml-awsstarkcontrolservice` service: how it claims requests and how it words the
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
//!   generated routing table. THIS SERVICE takes it for authorization, which Clean Rooms ML
//!   settles AHEAD of request validation: a call that names the IAM role this service acts
//!   as, and carries none, is answered `AccessDeniedException` (403) there (see
//!   [`role_arn_authorization`]), and so is a call that references a resource this endpoint
//!   resolves to nothing (see [`resource_authorization`]).
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. THIS SERVICE takes it: a member ABSENT from the request is
//!   stated with no value token at all; see [`validation`].
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
//! A sixth seam, `override_protocol`, is the one this service DOES take: every timestamp
//! member of this model declares `@timestampFormat("date-time")`, so each instant it
//! reports is an RFC 3339 date-time string rather than the protocol's default
//! epoch-seconds number.

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service,
};
use std::sync::Arc;

/// The role-ARN authorization this service takes ahead of request validation: a call that
/// names no role is an authorization failure, not a validation one.
mod role_arn_authorization;

/// The referenced-resource authorization this service takes ahead of request validation: a
/// call naming a resource this endpoint resolves to nothing is an authorization failure, not a
/// validation one.
mod resource_authorization;

/// How this service words a request the framework refused before any handler ran.
mod validation;

/// The `com-amazonaws-cleanroomsml-awsstarkcontrolservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The codec every modelled answer of this service is serialized by, built ONCE
    /// here because [`Service::override_protocol`] hands back a borrow of it.
    protocol: service_common::date_time_json::DateTimeRestJson,
    /// The 403 answer for a call that names no IAM role to act as. Held here so
    /// [`Service::override_operation`] can hand out a reference to it; it carries no state,
    /// because the denial is built from the request alone.
    deny_absent_role_arn: role_arn_authorization::DenyAbsentRoleArn,
    /// The 403 answer for a call that references a resource this endpoint holds none of. Held
    /// here for the same reason, and it carries the shared engine, because that denial is
    /// decided from the request AND from this service's own store.
    deny_unauthorized_reference: resource_authorization::DenyUnauthorizedReference<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            protocol: service_common::date_time_json::DateTimeRestJson::new(),
            deny_absent_role_arn: role_arn_authorization::DenyAbsentRoleArn,
            deny_unauthorized_reference:
                resource_authorization::DenyUnauthorizedReference::new(engine),
        }
    }

    /// Does `req` call an operation that takes an IAM role to act as, without naming one?
    ///
    /// The read itself is [`role_arn_authorization::absent_role_arn`], the one the refusal is
    /// built from too, so the gate and the message it answers with cannot disagree about
    /// which request is refused.
    fn names_no_role_to_act_as(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        role_arn_authorization::absent_role_arn(self.entry.wire_protocol(req).protocol(), req)
            .is_some()
    }

    /// Does `req` reference a Clean Rooms ML resource this endpoint resolves to nothing?
    ///
    /// The read is the gate's own, the one the refusal is built from too, so the gate and the
    /// message it answers with cannot disagree about which request is refused.
    fn references_an_unauthorized_resource(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> bool {
        self.deny_unauthorized_reference
            .applies(self.entry.wire_protocol(req).protocol(), req)
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
        // Authorization comes before request validation: a call that names the IAM role this
        // service acts as, and carries none, has no principal whose policies could allow it,
        // and a call that references a resource this endpoint holds none of has no
        // resource-based policy that could allow it — so both are refused
        // `AccessDeniedException` (403) here rather than falling to the modelled
        // required-member or constraint rejection (400) the framework's pre-handler walk would
        // answer with. See [`role_arn_authorization`] and [`resource_authorization`]. Every
        // other request resolves through the generated routing table.
        if self.names_no_role_to_act_as(req) {
            return Some(&self.deny_absent_role_arn as _);
        }
        if self.references_an_unauthorized_resource(req) {
            return Some(&self.deny_unauthorized_reference as _);
        }
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a member ABSENT from the request with no value token at all —
        // there was no value to state — and every other clause with the value the request
        // carried. The rendering, and the envelope around it, are the shared ones in
        // `service_common::validation_message` — see [`validation`]. Code and message are
        // replaced together, and the code stays the modelled `ValidationException` (HTTP 400)
        // a client branches on.
        validation::refused(refusal)
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

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // Every timestamp member of this model declares
        // `@timestampFormat("date-time")` — a training dataset's `createTime` and
        // `updateTime`, an audience model's training window, every job's and every
        // channel's two instants — so each one this service reports is an RFC 3339
        // date-time string rather than the protocol's default epoch-seconds number,
        // which is the only form a client generated from this model can read. The
        // codec that applies it is otherwise the model's own `restJson1`.
        Some(&self.protocol)
    }
}
