// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda` service: how it claims requests and how it words the
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
//!   before any handler ran. This service states them AWS's way: the framework's
//!   envelope and clauses, with the value dropped from a clause about a `@sensitive`
//!   member and an enumerated type's value set listed in the order the table of those
//!   values is walked rather than in the order the model declares them
//!   (`service_common::validation_message::in_hashed_enum_order`). See the inline note
//!   on that method.
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

use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::{Arc, LazyLock};

/// This service's Smithy model: the one source for which of its members are
/// `@sensitive`, and so for which values a refusal does not echo.
const MODEL_JSON: &str =
    include_str!("../smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json");

/// The members of this model whose value a refusal WITHHOLDS, read off
/// [`MODEL_JSON`] rather than listed here, so the decision cannot drift from the model
/// as the model grows a `@sensitive` member.
///
/// This model marks a great many of them: `FlowDefinition` (so every value inside a
/// flow definition), `Instruction`, `CollaborationInstruction`, `InlineCode`, an
/// action group's `Payload`, a prompt's variants, templates, messages and metadata.
static WITHHELD_MEMBERS: LazyLock<service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

/// The `com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
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
        // This service signs as `bedrock`, the signing name every Bedrock-family service
        // shares while serving its own endpoint (`bedrock-agent` here). Claiming on that
        // name alone makes all six claimants for every Bedrock request, which the router
        // answers `500 AmbiguousService` — a code no AWS model carries — rather than
        // serving one of them.
        //
        // So the claim is this model's own path space (`agents`, `knowledgebases`,
        // `flows`, `prompts`, `retrievalinstructions`, `resourcepolicy`, `tags`), through
        // the one rule `service_common::bedrock` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split). Four
        // of those segments are also published by `bedrock-agent-runtime`, and the rule
        // splits them there: under `agents`, `knowledgebases` and `flows` the two models
        // bind DIFFERENT routes, so each service claims only a request its own model binds
        // a route for; under `tags` they bind the identical three routes, so the ARN in the
        // path decides — each model enumerates the resource types its OWN tagging admits
        // and the two enumerations are disjoint, so a request naming a resource THIS plane
        // mints (an agent, an agent alias, a knowledge base, a flow, a prompt, a retrieval
        // instruction) is this service's and every other one is the data plane's.
        // `None` for the RPC argument: every operation here is bound to a URI.
        service_common::bedrock::claims_request(
            parts,
            self.definition(),
            crate::ROUTE_PATH_SEGMENTS,
            None,
        )
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        let _ = req;
        None
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The framework hands the refusal over structurally — one violation per
        // offending value — and leaves the words to the service. This service states
        // them AWS's way: the framework's envelope, clauses, member paths and
        // requirements, with the VALUE half dropped from the clauses that state none.
        //
        // A clause states no value when the value is one the model marks `@sensitive`
        // ([`WITHHELD_MEMBERS`]): a server does not echo back what the model says not
        // to disclose. Everything else — the envelope, the member path, the
        // `@pattern` expression and every other requirement — stays exactly as the
        // framework worded it.
        //
        // One ORDER is this service's too: an enumerated type's admissible values are
        // listed in the order this front door walks its OWN table of them — a table
        // keyed on each value's text, so the listing is that table's buckets in
        // ascending order (`in_hashed_enum_order`) — and not in the order the model
        // declares them. Each violation goes through that re-ordering BEFORE the
        // envelope words it, so only the values move: the member path, the echoed
        // value and every requirement's wording keep coming from the one place that
        // writes them, and every other violation class comes back untouched. The order
        // is derived from the values themselves, so it belongs to the enumerated TYPE:
        // every member of every operation targeting one enum is listed alike, at
        // whatever depth and in whatever binding the value arrived, and an enum the
        // model gains needs nothing added here.
        let restated = match refusal {
            Refusal::Invalid(violations) => std::borrow::Cow::Owned(Refusal::Invalid(
                violations
                    .iter()
                    .map(service_common::validation_message::in_hashed_enum_order)
                    .collect(),
            )),
            // Not a constraint refusal, so it carries no value set to re-order.
            other => std::borrow::Cow::Borrowed(other),
        };
        service_common::validation_message::aws_validation_refusal_withholding_with(
            &restated,
            // The expression a `@pattern` clause advertises is the model's own, as the
            // framework already quotes it: nothing recorded for this service shows it
            // quoted any other way, so only the value half below is restated.
            |pattern| pattern.to_string(),
            |path| WITHHELD_MEMBERS.withholds_value(path),
        )
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
