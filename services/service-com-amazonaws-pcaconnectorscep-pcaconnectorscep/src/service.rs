// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-pcaconnectorscep-pcaconnectorscep` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name. EDITED HERE: it
//!   also claims a request addressed to a connector's SCEP data plane (see "The SCEP
//!   data plane" below).
//! - `override_operation` — serve a request the model's routing key cannot name
//!   correctly. EDITED HERE: a request to a published `Connector$Endpoint` is named as an
//!   operation, so this service answers it; every other request resolves through the
//!   generated routing table.
//! - `override_refusal` — how this service words a request the framework refused
//!   before any handler ran. The default returns `None`, keeping the framework's
//!   wording. KEPT at the default here: this service's two wordings for a value its model
//!   does not admit depend on WHERE in the request that value travelled, which this seam
//!   cannot tell (see "Two wordings" below and [`refusal`]).
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
//! ## The SCEP data plane
//!
//! This service publishes two kinds of address. The CONTROL plane is
//! `pca-connector-scep.<region>.amazonaws.com`, whose first label names the service, and
//! that is what the framework's route key resolves. Beside it, every connector is given
//! its own SCEP URL (`Connector$Endpoint`, "the connector's HTTPS public SCEP URL"), which
//! is the address a SCEP-enabled device or an MDM system is configured with. A request to
//! it has to reach THIS service rather than the control-plane resolver, which can name no
//! service from a connector's own label — or from the emulator's own endpoint, where the
//! same plane is published with the connector in the PATH for a client that resolves no
//! `amazonaws.com` name ([`crate::state::announced_connector_endpoint`]).
//!
//! Two seams carry it, and nothing else changes:
//!
//! - [`Service::can_handle`] also claims a request addressed to that plane
//!   ([`scep::claims_parts`]), which is what routes it here at all.
//! - [`Service::override_operation`] serves it, through [`scep::ScepDataPlane`]. A SCEP
//!   device presents no AWS credential, so that plane declares no auth scheme and the
//!   router serves the request however it arrived.
//!
//! What is served on it — the private CA's own certificate for `GetCACert`, and an error
//! for every SCEP operation this emulator does not implement — is documented in [`scep`].
//! Every call this service makes into Amazon Web Services Private CA, on that plane and on
//! the control plane alike, goes through [`private_ca`], and every question about a VPC
//! endpoint a connector names goes to Amazon EC2 through [`vpc_endpoint`]; how it words a
//! request its own model refused is in [`refusal`].
//!
//! ## Two wordings for a value the model does not admit
//!
//! This service answers a value outside a constraint its model declares in TWO wordings, one
//! per layer that read the value: the body-schema validator's bracketed findings for a value
//! the request carried in its JSON body, and the standard validation envelope for one it
//! carried on its request line. [`refusal`] states the rule and the evidence for it.
//!
//! Neither can be chosen in [`Service::override_refusal`], which is handed the violations
//! alone: this model binds `ConnectorArn` to a body, to a path and to a query string in
//! different operations, so two refusals that are indistinguishable there are worded
//! differently on the wire. Every modelled operation is therefore served wrapped in
//! [`FrontDoor`], which reads each violated member's HTTP binding off THAT operation's own
//! input schema and words the two halves through the two functions attached to the door.

pub mod private_ca;
pub mod refusal;
pub mod scep;
pub mod vpc_endpoint;

use service_common::front_door::FrontDoor;
use smithy_server_runtime::{
    http, ErasedOperation, Operation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

/// Serve `operation` behind this service's front door, under its own modelled name — the
/// routing key the protocol resolves and the generated table is keyed by, read off the
/// operation rather than spelled again here.
///
/// Both of this service's wordings are attached to every door: an operation that binds
/// nothing to its body never has a body violation to find, and one that binds nothing to its
/// request line never has a request-line violation, which the wrapper reads off that
/// operation's own input schema.
fn serve<O: Operation + ErasedOperation + 'static>(table: &mut OperationRoutingTable, operation: O) {
    table.add_operation(
        O::NAME,
        Box::new(
            FrontDoor::worded(operation, refusal::body_refusal)
                .wording_uri(refusal::request_line_refusal),
        ) as Box<dyn ErasedOperation>,
    );
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each served behind
/// this service's front door ([`serve`]).
macro_rules! serve_operations {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            serve(
                &mut $table,
                crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
            );
        )+
    };
}

/// The `com-amazonaws-pcaconnectorscep-pcaconnectorscep` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// Serves the SCEP data plane a connector publishes — the wire its `Endpoint`
    /// advertises. Held on the service because [`Service::override_operation`] hands the
    /// router a BORROW of one of its own operations, and it owns the shared engine so every
    /// answer is computed from the same store the control-plane handlers write.
    scep: scep::ScepDataPlane<Eng>,
    /// The same modelled operations the generated table holds, each served behind this
    /// service's front door so a value its model does not admit is worded by the layer that
    /// read it. Keyed by the routing key the protocol resolves — the key the generated table
    /// uses — and built from the SAME shared engine, so a wrapped operation reads and writes
    /// the one store. Held on the service because [`Service::override_operation`] hands the
    /// router a BORROW of one of its own operations.
    front_door: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let mut front_door = OperationRoutingTable::new();
        // EVERY modelled operation, so the rule is applied by one code path rather than per
        // operation.
        serve_operations!(
            front_door,
            engine,
            create_challenge::CreateChallenge,
            create_connector::CreateConnector,
            delete_challenge::DeleteChallenge,
            delete_connector::DeleteConnector,
            get_challenge_metadata::GetChallengeMetadata,
            get_challenge_password::GetChallengePassword,
            get_connector::GetConnector,
            list_challenge_metadata::ListChallengeMetadata,
            list_connectors::ListConnectors,
            list_tags_for_resource::ListTagsForResource,
            tag_resource::TagResource,
            untag_resource::UntagResource,
        );
        Self {
            scep: scep::ScepDataPlane::new(Arc::clone(&engine)),
            front_door,
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
        // A request whose resolved route key is this service's signing name is the control
        // plane, exactly as the framework's default claims it.
        if smithy_server_runtime::resolve_route_key(parts)
            .is_some_and(|key| key == self.entry.identity().signing_name())
        {
            return true;
        }
        // And a request addressed to a connector's SCEP data plane — the URL this service
        // PUBLISHES in every connector's `Endpoint`, whose host is a connector's own rather
        // than this service's, or the reserved path that plane is published at on the
        // emulator's OWN endpoint. Neither can be named by a route key. See the module docs:
        // without this the router answers `404 UnknownService` for a URL this service handed
        // out.
        scep::claims_parts(parts)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // A request to a published connector `Endpoint` names no modelled operation — SCEP
        // is the device's own protocol — so it is named here instead, and served by the
        // plane that holds this service's state. Everything else resolves through the
        // generated routing table.
        if self.scep.claims(req) {
            return Some(&self.scep);
        }
        // NOT a re-route: the key is the one the generated table's own protocol resolves, and
        // the operation it names is the one served — behind this service's front door, so a
        // value its model does not admit is worded by the layer that read it. A key the model
        // does not name resolves to `None` here exactly as it does in the generated table, so
        // the router's routing-error path is unchanged, and the door declares the wrapped
        // operation's own auth schemes, so wrapping cannot change which schemes sign it.
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.front_door.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Nothing is reworded HERE, because nothing this service words differently can be
        // placed from the violations alone: this model binds one member name to a body, to a
        // path and to a query string in different operations, and the two wordings follow the
        // BINDING (see [`refusal`]). Both are attached to the front door each operation is
        // served behind, which is the only layer that knows which operation was addressed.
        // What still reaches this seam is a refusal that door does not report — a body no
        // codec can read, a refusal mixing a `@pattern` with another constraint class — and
        // the framework's own wording, which states the constraint the value broke, is the
        // right answer for it.
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
