// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-ebs-ebs` service: how it claims requests and how it words the
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

use aws_smithy_schema::Schema;
use smithy_server_runtime::{http, ErasedOperation, Operation, Refusal, Refused, Service};
use std::sync::Arc;

pub mod authorization;
mod required_members;

use required_members::{Binding, RequiredMember, RequiresMembers};

use crate::ops::complete_snapshot::CompleteSnapshot;
use crate::ops::get_snapshot_block::GetSnapshotBlock;
use crate::ops::put_snapshot_block::PutSnapshotBlock;

/// `PutSnapshotBlock`'s required members, in the order it judges them: the `Checksum`
/// that binds the unsigned block payload to the request first — a request without one
/// carries no block this service can accept, whatever else it left out — then the
/// algorithm that computed it, then the length it declares. Wire names from
/// `smithy_query(query="operation",operation="PutSnapshotBlock")`.
static PUT_SNAPSHOT_BLOCK_REQUIRED: &[RequiredMember] = &[
    RequiredMember {
        member: "Checksum",
        binding: Binding::Header("x-amz-Checksum"),
        stated: Some(crate::state::MISSING_CHECKSUM_MESSAGE),
    },
    RequiredMember {
        member: "ChecksumAlgorithm",
        binding: Binding::Header("x-amz-Checksum-Algorithm"),
        stated: None,
    },
    RequiredMember {
        member: "DataLength",
        binding: Binding::Header("x-amz-Data-Length"),
        stated: None,
    },
];

/// `GetSnapshotBlock`'s one required member: the token that names the block to read.
static GET_SNAPSHOT_BLOCK_REQUIRED: &[RequiredMember] = &[RequiredMember {
    member: "BlockToken",
    binding: Binding::Query("blockToken"),
    stated: None,
}];

/// `CompleteSnapshot`'s one required member: the count the seal is checked against.
static COMPLETE_SNAPSHOT_REQUIRED: &[RequiredMember] = &[RequiredMember {
    member: "ChangedBlocksCount",
    binding: Binding::Header("x-amz-ChangedBlocksCount"),
    stated: None,
}];

/// The `com-amazonaws-ebs-ebs` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The three operations whose model requires a header or query member, each behind
    /// the guard that answers an absent one — see [`required_members`] for why they are
    /// held here rather than reached through the generated routing table.
    complete_snapshot: RequiresMembers<CompleteSnapshot<Eng>>,
    put_snapshot_block: RequiresMembers<PutSnapshotBlock<Eng>>,
    get_snapshot_block: RequiresMembers<GetSnapshotBlock<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    ///
    /// The three guarded operations are constructed from the SAME shared engine, so a
    /// request served through one of them reads and writes the one store every other
    /// operation of this service uses.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            complete_snapshot: RequiresMembers::new(
                CompleteSnapshot::new(Arc::clone(&engine)),
                COMPLETE_SNAPSHOT_REQUIRED,
            ),
            put_snapshot_block: RequiresMembers::new(
                PutSnapshotBlock::new(Arc::clone(&engine)),
                PUT_SNAPSHOT_BLOCK_REQUIRED,
            ),
            get_snapshot_block: RequiresMembers::new(
                GetSnapshotBlock::new(engine),
                GET_SNAPSHOT_BLOCK_REQUIRED,
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
        // The three operations whose model requires a header or query member are named
        // HERE, by the request's method and path, and served behind the guard that answers
        // an absent one.
        //
        // The generated routing table cannot name them for such a request: a REST route
        // also carries its operation's `@required` header and query members and declines
        // a request missing one, which is how two operations sharing a method and URI
        // template are told apart. This model has no such pair, so the method and path
        // already name the operation — and the request is that operation's, with an input
        // error, rather than a request for an operation the emulator does not serve.
        // `required_members` states the rule and what the guard answers.
        //
        // No method or URI template is written out: each candidate is matched through the
        // shared reader (`service_common::addresses_modeled_http_route`, which exists for
        // exactly this case) against the route its OWN input schema's `@http` binding
        // gives it, so this seam cannot drift from the model and an operation can only
        // ever serve the route the model binds to it. Which operations belong here is the
        // one thing a schema cannot say — the routing table hands back erased operations —
        // so the three are named, and they are exactly the ones
        // `smithy_query(query="operation",…)` reports a `@required` header or query member
        // for; `StartSnapshot`, `ListSnapshotBlocks` and `ListChangedBlocks` declare none,
        // so nothing can stop the table from naming them.
        //
        // Every other request resolves through the generated table exactly as before.
        let held: [(&'static Schema, &dyn ErasedOperation); 3] = [
            (
                <CompleteSnapshot<Eng> as Operation>::INPUT_SCHEMA,
                &self.complete_snapshot,
            ),
            (
                <PutSnapshotBlock<Eng> as Operation>::INPUT_SCHEMA,
                &self.put_snapshot_block,
            ),
            (
                <GetSnapshotBlock<Eng> as Operation>::INPUT_SCHEMA,
                &self.get_snapshot_block,
            ),
        ];
        held.into_iter()
            .find(|(schema, _)| service_common::addresses_modeled_http_route(req, schema))
            .map(|(_, operation)| operation)
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
