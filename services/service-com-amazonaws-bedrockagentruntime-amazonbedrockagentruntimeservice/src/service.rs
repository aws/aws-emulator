// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice` service: how it claims requests and how it words the
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

use service_common::event_stream::EventStreamRoute;
use smithy_server_runtime::{http, ErasedOperation, Refusal, Refused, Service};
use std::sync::Arc;

use crate::ops::agentic_retrieve_stream::{self, AgenticRetrieveStream};
use crate::ops::invoke_agent::{self, InvokeAgent};
use crate::ops::invoke_flow::{self, InvokeFlow};
use crate::ops::invoke_inline_agent::{self, InvokeInlineAgent};
use crate::ops::optimize_prompt::{self, OptimizePrompt};
use crate::ops::retrieve_and_generate_stream::{self, RetrieveAndGenerateStream};

/// See the module docs of that file: serves the content of an ingested document at the
/// pre-signed URL `GetDocumentContent` hands out for it, which is not an API call at all.
pub(crate) mod ingested_content;

/// The `@pattern` this service PUBLISHES for a member whose on-disk model records a STALE
/// spelling of the same constraint, keyed by the stale text the generated schema carries.
///
/// A refusal states the requirement the SERVICE publishes, and for these members the model
/// shipped in `smithy-model/` and the service disagree: each pair below is one member shape
/// whose recorded refusal quotes an expression the on-disk `@pattern` does not spell, so the
/// published one is what a caller lifting the expression out of the message has to satisfy.
/// Keyed by the MODEL's text rather than by a member path, so a model that is refreshed to
/// the published spelling stops matching here and the override falls away by itself, and so
/// that every member reaching the same shape states one requirement.
///
/// What each pair records, from the recorded refusals of this service:
///
/// * `KnowledgeBaseArn` — the partition alternation names the three ISOLATED partitions
///   beside the commercial, China and GovCloud ones, so a knowledge-base ARN in any
///   partition this service serves satisfies the stated expression;
/// * `KmsKeyArn` — the same partition alternation, on the customer-managed-key ARN both
///   `CreateSession` and the inline-agent invocation accept;
/// * `TaggableResourcesArn` — the published expression admits the two RUNTIME resource
///   kinds this plane's tagging operations address (the session and the prompt-optimisation
///   job) with an identifier of the general ARN resource grammar, where the on-disk
///   spelling names the session alone with a uuid body.
///
/// The CHECK stays the model's: only the expression a refusal QUOTES is decided here.
const PUBLISHED_PATTERNS: &[(&str, &str)] = &[
    (
        "^arn:aws(|-cn|-us-gov):bedrock:[a-zA-Z0-9-]*:[0-9]{12}:knowledge-base/[0-9a-zA-Z]+$",
        "^arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):bedrock:[a-zA-Z0-9-]*:[0-9]{12}\
         :knowledge-base/[0-9a-zA-Z]+$",
    ),
    (
        "^arn:aws(|-cn|-us-gov):kms:[a-zA-Z0-9-]*:[0-9]{12}:key/[a-zA-Z0-9-]{36}$",
        "^arn:aws(|-cn|-us-gov|-iso|-iso-b|-iso-f):kms:[a-zA-Z0-9-]*:[0-9]{12}\
         :key/[a-zA-Z0-9-]{36}$",
    ),
    (
        "(^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}:(session)/[a-f0-9]{8}-[a-f0-9]{4}\
         -[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$)",
        "^arn:aws(-[^:]+)?:bedrock:[a-zA-Z0-9-]+:[0-9]{12}\
         :(session|advanced-optimize-prompt-job)/[a-zA-Z0-9-]+$",
    ),
];

/// The expression a `@pattern` refusal of this service QUOTES: the published constraint
/// ([`PUBLISHED_PATTERNS`]) stated as the expression the whole value must match
/// (`service_common::validation::whole_value_expression`).
fn quoted_pattern(modelled: &str) -> String {
    let published = PUBLISHED_PATTERNS
        .iter()
        .find(|(stale, _)| *stale == modelled)
        .map_or(modelled, |(_, published)| *published);
    service_common::validation::whole_value_expression(published)
}

/// The `com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
///
/// Besides the generated entry it holds `InvokeInlineAgent` and `OptimizePrompt`. Neither
/// is in the entry's routing table in a servable form: the generated `ErasedOperation` shim
/// of an operation whose output is a `@streaming` union declines the request (it cannot
/// drive `serialize_as_event_stream<P>` through an object-safe `run`), so each is held here
/// behind an [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`] —
/// which the router consults FIRST, before the generated table.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    agentic_retrieve_stream: EventStreamRoute<AgenticRetrieveStream<Eng>>,
    invoke_agent: EventStreamRoute<InvokeAgent<Eng>>,
    invoke_flow: EventStreamRoute<InvokeFlow<Eng>>,
    invoke_inline_agent: EventStreamRoute<InvokeInlineAgent<Eng>>,
    optimize_prompt: EventStreamRoute<OptimizePrompt<Eng>>,
    retrieve_and_generate_stream: EventStreamRoute<RetrieveAndGenerateStream<Eng>>,
    /// Serves the pre-signed addresses `GetDocumentContent` hands out for the content of a
    /// document whose bytes travelled in its ingestion; [`Service::override_operation`]
    /// hands it the requests it claims (see [`ingested_content`]).
    ingested_content: ingested_content::IngestedContentRoute<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine.clone()),
            agentic_retrieve_stream: EventStreamRoute(AgenticRetrieveStream::new(engine.clone())),
            invoke_agent: EventStreamRoute(InvokeAgent::new(engine.clone())),
            invoke_flow: EventStreamRoute(InvokeFlow::new(engine.clone())),
            invoke_inline_agent: EventStreamRoute(InvokeInlineAgent::new(engine.clone())),
            optimize_prompt: EventStreamRoute(OptimizePrompt::new(engine.clone())),
            retrieve_and_generate_stream: EventStreamRoute(RetrieveAndGenerateStream::new(
                engine.clone(),
            )),
            ingested_content: ingested_content::IngestedContentRoute::new(engine),
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
        // The content of an ingested document, which is not an API call at all: it is the
        // artifact `GetDocumentContent` hands out a pre-signed URL for, on a path space no
        // Bedrock model publishes (see [`ingested_content`]). Claimed first, because such a
        // request addresses no modelled route for the rule below to recognise.
        if crate::state::document_content::addresses_ingested_content(parts.uri.path()) {
            return true;
        }
        // This service signs as `bedrock`, the signing name every Bedrock-family service
        // shares while serving its own endpoint (`bedrock-agent-runtime` here). Claiming
        // on that name alone makes all six claimants for every Bedrock request, which the
        // router answers `500 AmbiguousService` — a code no AWS model carries — rather
        // than serving one of them.
        //
        // So the claim is this model's own path space (`sessions`, `rerank`,
        // `retrieveAndGenerate`, `agenticRetrieveStream`, `generateQuery`,
        // `optimize-prompt`, …), through the one rule `service_common::bedrock` states
        // for the family (which narrows `service_common::claims_request` with the family's
        // shared-segment split). The `agents`, `knowledgebases`, `flows` and `tags`
        // segments are also published by the `bedrock-agent` build-time plane: under the
        // first three the two models bind DIFFERENT routes, so this service claims only
        // the routes its own model binds (`POST /knowledgebases/{id}/retrieve`, the flow
        // execution paths, the agent memory and invocation paths); under `tags` both bind
        // the identical three routes and the ARN in the path names the plane that MINTS the
        // tagged resource — each model enumerates the resource types its OWN tagging
        // admits and the two enumerations are disjoint — so this service claims a request
        // naming a resource IT mints (a session, an advanced-optimize-prompt job), and an
        // ARN naming no taggable resource of either plane, which neither can resolve, as
        // the segment's declared owner. `None` for the RPC argument: every operation here
        // is bound to a URI.
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
        // `InvokeInlineAgent` and `OptimizePrompt`, which the generated routing table
        // cannot serve: each one's output is a `@streaming` union, so the generated shim
        // declines the request. Each is routed by the one route the operation itself
        // recognises (`<operation>::addresses`), so nothing else is touched — every other
        // request resolves through the generated table.
        if agentic_retrieve_stream::addresses(req) {
            return Some(&self.agentic_retrieve_stream);
        }
        if invoke_agent::addresses(req) {
            return Some(&self.invoke_agent);
        }
        if invoke_flow::addresses(req) {
            return Some(&self.invoke_flow);
        }
        if invoke_inline_agent::addresses(req) {
            return Some(&self.invoke_inline_agent);
        }
        if optimize_prompt::addresses(req) {
            return Some(&self.optimize_prompt);
        }
        if retrieve_and_generate_stream::addresses(req) {
            return Some(&self.retrieve_and_generate_stream);
        }
        // The fetch of a document's content, which the generated table cannot serve either:
        // the model binds no operation to its path space at all.
        if self.ingested_content.claims(req).is_some() {
            return Some(&self.ingested_content as &dyn ErasedOperation);
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
        //
        // This service restates a `@pattern` failure as the expression the WHOLE VALUE has
        // to match: the `^`/`$` a model writes to demand a whole-string match are not part
        // of the expression a value satisfies, and an end the model leaves OPEN is spelled
        // with the unconstrained run it admits, so a caller that compiles or compares the
        // quoted expression gets one its own whole value has to satisfy. Of the renderings
        // the shared home offers this service states
        // `service_common::validation::whole_value_expression`, which decides the two ends
        // independently and keeps a leftover top-level alternation grouped.
        //
        // The expression it is applied to is the constraint this service PUBLISHES
        // (`PUBLISHED_PATTERNS`), which for three member shapes is not the stale spelling the
        // on-disk model carries. Both choices are the ONLY things decided here: which
        // refusals are restated, the modelled `ValidationException` code, the envelope, the
        // member paths and every other requirement are the shared
        // `service_common::validation_message::aws_validation_refusal_with`'s and the
        // framework's, so this service cannot drift from them. Every other violation class
        // keeps the framework's wording, and an authentication refusal keeps it too: it is
        // not an input-constraint failure.
        service_common::validation_message::aws_validation_refusal_with(refusal, |pattern| {
            quoted_pattern(pattern)
        })
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
