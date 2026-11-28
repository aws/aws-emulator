// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0` service: how it claims requests and how it words the
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
//! - `override_operation` — EDITED HERE: serve a request the model's routing key
//!   cannot name correctly. It no longer returns the default `None` for every
//!   request: the model's sixth operation, `StartConversation`, carries a
//!   `@streaming` event-stream input and output, whose generated `ErasedOperation`
//!   shim declines the request, so a request for it is served by this file's
//!   hand-authored copy of that operation instead.
//! - `override_refusal` — EDITED HERE: how this service words a request the
//!   framework refused before any handler ran. It no longer returns the default
//!   `None` for every refusal — a refusal that blames the bot identifier of a
//!   session address is worded as the Lex V2 runtime words it. See the inline
//!   note on that method.
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

use smithy_server_runtime::{
    http, ErasedOperation, Refusal, Refused, Service, Violation,
};
use service_common::event_stream::EventStreamRoute;
use std::sync::Arc;

/// The dotted path the validating walk gives the bot identifier every operation
/// of this service addresses a session by: the one `@httpLabel` member targeting
/// `BotIdentifier` (`@length(min: 10, max: 10)`, `@pattern("^[0-9a-zA-Z]+$")`),
/// which all six operations of the model bind under this name — `GetSession`,
/// `PutSession`, `DeleteSession`, `RecognizeText` and `RecognizeUtterance`
/// through the framework's own validating walk, and `StartConversation` through
/// the request reader of its hand-authored handler, which asks
/// [`bot_id_refusal`] for the same sentence.
///
/// It is the ONLY label of a session address this rule can be reached through:
/// the bot ALIAS label targets `BotAliasIdentifier`, which declares neither a
/// length nor a pattern, so no violation of it is ever raised.
const BOT_ID_PATH: &str = "botId";

/// What the Amazon Lex V2 runtime answers a request whose bot identifier is not
/// a well-formed bot identifier.
///
/// The sentence is entirely FIXED: it names neither the member nor the value the
/// caller sent, and states no bound — the frontend reports that it could not
/// address a bot, not which rule of the identifier shape the value broke.
const INVALID_BOT_OR_ALIAS_MESSAGE: &str = "INVALID_REQUEST - Invalid bot name or alias";

/// The error every operation of this model declares for a request it refuses at
/// HTTP 400, and the code the framework already names for a refused value — so
/// rewording such a refusal keeps it.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The ONE wording this service gives a refusal that blames the bot identifier of
/// a session address, reached by both paths that can raise one.
///
/// The framework's validating walk reaches it through [`EmulatedService::override_refusal`],
/// which is handed the violations alone; the hand-authored `StartConversation`
/// reads its own request, so it asks for the sentence here. Neither holds a
/// wording of its own, so the two cannot drift.
pub(crate) mod bot_id_refusal {
    use super::{BOT_ID_PATH, INVALID_BOT_OR_ALIAS_MESSAGE};

    /// The sentence owed for `violations` when one of them blames the bot
    /// identifier, `None` when none does — in which case the refusal keeps the
    /// framework's own wording.
    pub(crate) fn sentence_for(violations: &[super::Violation]) -> Option<String> {
        violations
            .iter()
            .any(|violation| violation.path == BOT_ID_PATH)
            .then(|| INVALID_BOT_OR_ALIAS_MESSAGE.to_string())
    }
}

/// The `com-amazonaws-lexruntimev2-awsdeepsenseruntimeserviceapi2-0` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The hand-authored `StartConversation`, the model's one event-stream
    /// operation. The generated routing table holds a copy of it whose generated
    /// `ErasedOperation` shim DECLINES the request (its
    /// `serialize_as_event_stream<P>` needs a `Sized` protocol, which an
    /// object-safe `run` cannot hand it), so this copy is held behind an
    /// [`EventStreamRoute`] and routed by [`EmulatedService::override_operation`]
    /// — the seam the router consults before that table. Both reach the same
    /// sessions, because the sessions live in the shared engine.
    start_conversation: EventStreamRoute<crate::ops::start_conversation::StartConversation<Eng>>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(Arc::clone(&engine)),
            start_conversation: EventStreamRoute(
                crate::ops::start_conversation::StartConversation::new(engine),
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
        // This service signs as `lex` — the signing name the WHOLE Lex family shares
        // (this V2 runtime, the V2 model-building control plane, and the V1 model-building
        // and runtime APIs), while AWS separates them by endpoint host (`runtime-v2-lex`
        // here). The router keys on the SigV4 credential scope first, so all four resolve
        // to `lex`: claiming on that name alone makes every one of them claim every Lex
        // request and the router answers `500 AmbiguousService` — an error code no AWS
        // model declares — instead of serving any of them.
        //
        // The claim is therefore this service's own modelled wire surface, through the one
        // rule `service_common::lex` states for the family (which narrows
        // `service_common::claims_request` with the family's shared-segment split): this
        // model's one first path segment is `bots`, which the two model-building control
        // planes publish too, so the modelled route decides — and this model's session
        // URIs carry the literals `botAliases`/`botLocales`/`sessions`, which no sibling
        // binds (segment matching is case-SENSITIVE, so the V2 control plane's
        // `botaliases` is a different segment).
        service_common::lex::claims_request(parts, self.definition(), crate::ROUTE_PATH_SEGMENTS)
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // NOT a re-route: the key is the one the generated table's own protocol
        // resolves from this request, and every operation it names is served by the
        // generated table — except the one whose generated shim declines an
        // event-stream request, which is served by its hand-authored handler here.
        // That handler owns the whole request-to-response step, so it reads its own
        // request; a key the model does not name resolves to `None` exactly as it
        // does in the table, leaving the router's routing-error path unchanged.
        let key = self.entry.protocol().routing_key(req).ok()?;
        (key == crate::ops::start_conversation::OPERATION)
            .then_some(&self.start_conversation as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // The Lex V2 runtime resolves the bot a request addresses BEFORE it reads
        // anything else, and it states a bot identifier that is not a
        // well-formed one in its own fixed words
        // ([`INVALID_BOT_OR_ALIAS_MESSAGE`]) rather than reporting which rule the
        // value broke: a bot address is either one the frontend can look a bot up
        // by or it is not. So any refusal that blames the bot identifier is
        // worded that way, whichever operation it was reached through, and every
        // other refusal keeps the framework's wording (`None`), which is what
        // this service's remaining bounded labels and body members are stated by.
        //
        // The error CODE is the one the framework already names for a refused
        // value, `ValidationException`, which is the error every operation of this
        // model declares at HTTP 400 — so only the message changes.
        match refusal {
            Refusal::Invalid(violations) => {
                bot_id_refusal::sentence_for(violations).map(|message| Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message,
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
