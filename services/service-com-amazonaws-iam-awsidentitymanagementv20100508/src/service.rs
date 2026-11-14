// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-iam-awsidentitymanagementv20100508` service: how it claims requests and how it words the
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
//! ## The awsQuery error document
//!
//! IAM speaks `awsQuery`, whose error document is
//! `<ErrorResponse><Error>…</Error><RequestId>…</RequestId></ErrorResponse>`: the
//! `<RequestId>` is a SIBLING of `<Error>`, so no error shape's members can carry it and
//! the framework's error serializers — which write the error shape's own members, inside
//! `<Error>` — do not. It is a property of the RESPONSE, the counterpart of the
//! `ResponseMetadata/RequestId` the success renderer already writes, so it is added where
//! every response of this service passes: the shared
//! [`::service_common::query_error_envelope`], through its operation wrapper for a
//! modelled error (and a refusal raised while an operation was being served) and its
//! definition wrapper for the failures rendered outside the dispatch path.
//!
//! ## Credentials the account has deactivated
//!
//! An access key this account holds authenticates a request only while its stored
//! `Status` is `Active`: `UpdateAccessKey$Status` documents that "Active means that the
//! key can be used for programmatic calls to Amazon Web Services, while Inactive means
//! that the key cannot be used", so deactivating a key must cut the credential off
//! while leaving it listed. That is an AUTHENTICATION outcome — no principal is
//! resolved at all — so it is refused BEFORE any operation runs, and with the code an
//! authentication failure carries ([`UNUSABLE_CREDENTIAL_CODE`]) rather than with an
//! authorization denial or one of the modelled resource errors.
//!
//! The decision reads the records `iam:CreateAccessKey` and `iam:UpdateAccessKey` wrote
//! ([`crate::state::ComAmazonawsIamAwsidentitymanagementv20100508State::access_key_is_usable`]);
//! this file only reaches it ahead of dispatch, through [`Service::override_operation`]
//! — the one seam that sees a whole request before the modelled walk deserializes,
//! validates and dispatches it.
//!
//! ## Wording a request authentication refused
//!
//! The emulator verifies every signature and hands this service ONE structural refusal for
//! every way a credential can fail, carrying a generic code that is an AUTHORIZATION denial
//! — which is not what AWS answers a credential failure with. The rule, and the two fixed
//! sentences, live in [`::service_common::authentication_refusal`]; this file names only the
//! codes this service answers the two failures with ([`AUTHENTICATION_REFUSAL`]) and wires
//! that layer under the definition wrapper above, so the `<RequestId>` is reported on its
//! response too. The unknown-credential code is the one the deactivated-key refusal below
//! already answers with, so a credential this service will not accept and one it cannot
//! resolve at all are stated alike.
//!
//! It is the definition rather than [`Service::override_refusal`] because the seam that is
//! handed WHICH failure it was is the definition's `refusal_response`; the refusal hook sees
//! the structural refusal alone, which is why this service uses it for input values.
//!
//! ## The audit leg
//!
//! Every call this service serves is also a management event of `iam.amazonaws.com`: the
//! routing table its definition answers with is the one whose operations are wrapped in the
//! shared management-event recorder ([`management_events`]), so each served call is
//! recorded in the account's event history where CloudTrail's `LookupEvents` reports it —
//! one code path for every operation rather than a recording written into each handler.

pub mod management_events;
mod refusal_clauses;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::{ProtocolError, Violation, ViolationKind};
use service_common::authentication_refusal::{
    AuthenticationRefusalDefinition, StatedAuthenticationRefusal,
};
// The shared naming of the two documents every OIDC issuer this emulator serves publishes,
// which the route reading below answers with.
use service_common::openid_discovery::WellKnownDocument;
use service_common::query_error_envelope::{
    RequestIdEnvelopeDefinition, RequestIdEnvelopeOperation,
};
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, Refusal, Refused,
    Service,
};
use std::sync::Arc;

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the first URI path segments
/// this service's wire protocol puts its operations under.
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means no usable discriminator and is "no constraint",
/// and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a service that answers
/// `POST /` claims the root path without claiming every path. Matching is EXACT segment
/// equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The URI path space IAM's wire protocol uses: the ROOT path, and nothing else.
///
/// IAM speaks `awsQuery` (`smithy_query(query="service_info")`: protocols `["awsQuery"]`)
/// and no operation in its model carries a `smithy.api#http` binding, so every one of its
/// 180 operations arrives as `POST /` with the operation named by the `Action` form field.
/// The generated `crate::ROUTE_PATH_SEGMENTS` is EMPTY for exactly that case — the
/// generator keeps it empty for an RPC service with no bindings so as not to narrow
/// services that already work — and an empty list means "claims every path", which is
/// what made IAM collide with its signing-name peer. The root claim is therefore stated
/// here, using the framework's own sentinel for it. It is deliberately NOT called
/// `ROUTE_PATH_SEGMENTS`: that name belongs to the generated crate-level constant, which
/// is empty.
const WIRE_PATH_SEGMENTS: &[&str] = &[emulator_server::ROOT_PATH_SEGMENT];

/// The error code IAM names a refusal of a request's INPUT with: a value outside a bound,
/// a pattern or an enumerated set, a value outside a member's published form, and a
/// `@required` member the request did not carry.
///
/// It is the code the query protocols name a refused value with
/// (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), and it is stated here for EVERY
/// input refusal because that is what the recordings show: the framework names a request it
/// could not read at all — including one whose absent required member the codec found while
/// reading it — with the other query code (`MalformedInput`), while AWS answers an absent
/// required member with the same validation code a bad value gets. A client branches on the
/// code, so the two layers' findings are not told apart by it.
const VALUE_REFUSAL_CODE: &str = "ValidationError";

/// AWS's own sentence for `violation`, or `None` when the violation belongs to the
/// model-level constraint walk and keeps that walk's clause.
///
/// The two readings of a form IAM takes ITSELF, in one place so every caller below partitions
/// the violations the same way: the members whose WHOLE form is one sentence
/// ([`crate::state::member_format_refusal`]), and a value outside the PATTERN its member's
/// shape publishes ([`crate::state::pattern_refusal`]), which is keyed by the pattern the
/// violation carries, so it answers for every member targeting a shape that publishes it. A
/// violation of the member's LENGTH is not one of them: those sentences state no length, so
/// that bound stays the walk's own clause.
///
/// An ABSENT member is never one of them either, whatever its name: each of these sentences
/// states the form a VALUE has to take, and a member the request did not carry submitted no
/// value to be outside it. Its absence is the walk's own clause
/// ([`refusal_clauses::stated_clause`]).
fn self_taken_sentence(violation: &Violation) -> Option<String> {
    if matches!(violation.kind, ViolationKind::MissingRequiredMember) {
        return None;
    }
    if let Some(sentence) = crate::state::member_format_refusal(&violation.path) {
        return Some(sentence.to_string());
    }
    match &violation.kind {
        ViolationKind::Pattern(pattern) => {
            crate::state::pattern_refusal(&violation.path, pattern)
        }
        _ => None,
    }
}

/// A refusal stated as IAM states it, or `None` when there was nothing to state.
///
/// IAM takes a request's values in TWO layers, and the recorded refusals show both: the
/// forms [`self_taken_sentence`] answers for are taken by IAM itself, which answers one
/// fixed sentence for the form and no envelope at all, while every other constraint is taken
/// by the model-level walk, whose envelope states one clause per constraint the request
/// broke. So a refusal is stated by whichever layer the request reached:
///
/// - a request whose only broken members are the ones IAM takes itself is answered with
///   that member's sentence, and with nothing else — no envelope, no clause and no value;
/// - a request that ALSO broke a member the model-level walk takes is answered by that
///   walk, over its own members' clauses alone, because the self-taken members' constraints
///   are never stated as a clause in any refusal;
/// - a request that broke none of them is answered by that same walk over all of its
///   violations.
///
/// The envelope is the framework's, rendered over the clauses this service states for the
/// walk's violations ([`refusal_clauses::stated_clause`]) through the shared reader of that
/// envelope's own count word and separator
/// ([`::service_common::validation_message::envelope_over`]), so neither is spelled here.
fn stated_refusal(violations: &[Violation]) -> Option<Refused> {
    let (self_taken, modeled): (Vec<Violation>, Vec<Violation>) = violations
        .iter()
        .cloned()
        .partition(|violation| self_taken_sentence(violation).is_some());
    // A self-taken sentence states that member's WHOLE form, so no clause about the SAME
    // member's value is stated beside it: IAM takes the form itself, before the
    // model-level walk, so a value that is both outside the published form AND outside the
    // member's length is answered with the form sentence alone. A clause about ANOTHER
    // member is left where it is — it states a requirement the sentence says nothing
    // about.
    let modeled: Vec<Violation> = modeled
        .into_iter()
        .filter(|violation| {
            !self_taken
                .iter()
                .any(|stated| stated.path == violation.path)
        })
        .collect();
    if modeled.is_empty() {
        // One sentence, so a request that broke two self-taken members is answered with
        // the first of them in the order the walk reported them.
        let stated = self_taken.iter().find_map(self_taken_sentence)?;
        return Some(Refused {
            code: VALUE_REFUSAL_CODE.to_string(),
            message: stated,
        });
    }
    let clauses: Vec<String> = modeled
        .iter()
        // BOUNDED by the same ceiling the framework's own walk collects under, as the
        // shared renderer of this envelope bounds itself
        // (`::service_common::validation::MAX_VIOLATIONS`): the walk never hands over more,
        // so the clause count — and with it the length of the message and the work of
        // rendering it — is a constant of this crate rather than something one request
        // chooses.
        .take(::service_common::validation::MAX_VIOLATIONS)
        .map(refusal_clauses::stated_clause)
        .collect();
    Some(Refused {
        code: VALUE_REFUSAL_CODE.to_string(),
        message: ::service_common::validation_message::envelope_over(&clauses)?,
    })
}

/// The error code AWS answers a signed request whose credential it will not accept
/// with: a key id that was never issued, one that has been deleted, and one whose
/// `Status` the account has moved off `Active`. It is an AUTHENTICATION failure — no
/// principal was resolved — rather than an authorization denial, so it is the code an
/// SDK's credential-refresh path keys on, the same code this emulator's STS front door
/// answers an unresolvable credential with.
const UNUSABLE_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// How AWS words that refusal: a fixed sentence carrying nothing from the request, so a
/// caller presenting a credential the service will not accept learns nothing about the
/// credential or about this account. The wording of a credential failure is one rule for
/// every emulated service, so it is read from the module that owns it.
const UNUSABLE_CREDENTIAL_MESSAGE: &str =
    ::service_common::authentication_refusal::UNRESOLVED_CREDENTIAL_MESSAGE;

/// The two codes this service answers a request the emulator's authentication layer refused
/// with (see [`::service_common::authentication_refusal`], which owns the rule and the
/// sentences): the `awsQuery` unknown-credential code — the same one the deactivated-key
/// refusal above answers with — for an access key id that resolves to no credential, and the
/// SigV4 signature code for one that resolves and whose signature does not verify.
const AUTHENTICATION_REFUSAL: StatedAuthenticationRefusal = StatedAuthenticationRefusal {
    unresolved_credential: UNUSABLE_CREDENTIAL_CODE,
    signature_mismatch: "SignatureDoesNotMatch",
};

/// Answers a request that presented a credential this account will not accept, as a
/// pre-dispatch operation.
///
/// Rendered through the service's own refusal channel — the same path the framework's
/// authentication refusal takes — so the envelope, the status (403) and the `<Code>` a
/// client branches on are the ones every other refusal from this service carries.
struct UnusableCredential;

impl ErasedOperation for UnusableCredential {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        // The refusal is reached from the credential alone: nothing of the request's
        // body or metadata is read, and nothing is written — the request is handed to
        // the renderer only so the refusal closes the envelope of the protocol this
        // request is served in.
        let _ = ctx;
        Box::pin(async move {
            svc.definition().refusal_response(
                ProtocolError::Unauthenticated {
                    code: UNUSABLE_CREDENTIAL_CODE,
                    message: UNUSABLE_CREDENTIAL_MESSAGE,
                },
                None,
                req,
            )
        })
    }
}

/// The access key id `req` presented, or `None` for a request carrying no SigV4
/// `Authorization` header at all.
///
/// Read through the framework's ONE parser of that header
/// (`emulator_server::extract_akid`, which every handler in this crate resolves a
/// nameless caller through). That parser takes an `http::HeaderMap`, which this
/// pre-dispatch seam is not handed — it sees the runtime-API request — so the one
/// header it reads is lifted into a map rather than the parse being written again here.
fn presented_access_key_id(req: &RtRequest) -> Option<String> {
    let authorization = req.headers().get(http::header::AUTHORIZATION.as_str())?;
    let mut headers = http::HeaderMap::new();
    headers.insert(
        http::header::AUTHORIZATION,
        http::HeaderValue::from_str(authorization).ok()?,
    );
    Some(::engine::extract_akid(&headers))
}

/// The issuer segment and the document `uri` addresses, and `None` for every other path.
///
/// The shared reading of such a path ([`::service_common::openid_discovery::well_known_route`]),
/// asked with this service's own answer for whether a segment has the FORM it mints an
/// issuer in — so a path of the same shape addressed to another mounted service is not
/// claimed here. WHICH account holds that issuer is decided by the store when the document
/// is served, so a request addressed to an issuer form this emulator mints is answered by
/// this service rather than falling through to the router's "no service claimed this"
/// refusal.
fn issuer_document_route(uri: &str) -> Option<(&str, WellKnownDocument)> {
    ::service_common::openid_discovery::well_known_route(uri, &|segment| {
        crate::state::is_issuer_identifier_segment(segment)
    })
}

/// The `com-amazonaws-iam-awsidentitymanagementv20100508` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, behind the `awsQuery` error-envelope wrapper: every error this
    /// service answers carries the `<RequestId>` that envelope has beside its `<Error>`
    /// (see `service_common::query_error_envelope`), whichever layer rendered it — a
    /// routing failure, a transport failure and the pre-dispatch refusal below included.
    /// Identity, the protocol and the routing table are the generated entry's untouched.
    entry: RequestIdEnvelopeDefinition<
        AuthenticationRefusalDefinition<
            management_events::AuditedDefinition<crate::ServiceDefinition<Eng>>,
        >,
    >,
    /// The SAME shared engine the generated entry threaded into every operation, kept
    /// here too because the pre-dispatch credential check below has to read the access
    /// keys this service's own operations wrote. Never a store of its own: it is the
    /// `Arc` the router handed to `new`.
    engine: Arc<Eng>,
    /// Answers a request presenting a credential this account will not accept.
    unusable_credential: UnusableCredential,
    /// The envelope wrapper every modelled operation of this service is served through, so
    /// a modelled error and a refusal the centralized walk raised while an operation was
    /// being served name the request they answer too. Held in a field, not built per
    /// request: `override_operation` hands back a reference borrowed from `&self`.
    envelope: RequestIdEnvelopeOperation,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::new(AuthenticationRefusalDefinition::new(
                management_events::AuditedDefinition::new(
                    crate::ServiceDefinition::new(Arc::clone(&engine)),
                    // Every operation of this service, each wrapped in the shared
                    // management-event recorder and built from this same shared engine (see
                    // [`management_events`]), so each call this service serves is recorded as
                    // a management event of `iam.amazonaws.com`.
                    management_events::audited_table(&engine),
                ),
                AUTHENTICATION_REFUSAL,
            )),
            engine,
            unusable_credential: UnusableCredential,
            envelope: RequestIdEnvelopeOperation,
        }
    }

    /// Whether `req` presented an access key this account holds and will not accept —
    /// one whose stored `Status` is not `Active` (see the module docs).
    ///
    /// A request that names no modelled operation is left alone: there is nothing to
    /// serve it with, and the routing failure is the answer it gets. A store this
    /// process cannot reach holds no key status either, so such a request is admitted
    /// and the handler reports the unreachable store as the modelled server error.
    ///
    /// A READ: it takes the store's lock to look one key up and writes nothing, so a
    /// refused request changes nothing a later caller sees.
    fn presents_an_unusable_credential(&self, req: &RtRequest) -> bool {
        if self.definition().protocol().routing_key(req).is_err() {
            return false;
        }
        let Some(access_key_id) = presented_access_key_id(req) else {
            return false;
        };
        crate::state::service_state(&*self.engine)
            .is_some_and(|state| !state.lock().access_key_is_usable(&access_key_id))
    }
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry.
        //
        // It is reached through the `awsQuery` envelope wrapper, which leaves identity,
        // the protocol and the routing table exactly as the entry carries them and adds
        // the `<RequestId>` element to the framework-level error envelopes it renders — a
        // routing failure, a transport failure and a pre-dispatch refusal — so each
        // carries what a modelled error served through [`RequestIdEnvelopeOperation`]
        // carries.
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `iam` — a signing name it SHARES with IAM Toolbox
        // (`services/service-com-amazonaws-iamtoolbox-authrequestservice`, sigv4Name
        // `iam`, endpointPrefix `iam-toolbox`). The router's routing key comes from the
        // SigV4 credential scope first, so both services resolve to `iam` and matching on
        // the signing name ALONE makes both claim every request — which the router
        // answers `500 AmbiguousService` for, in a JSON body an awsQuery client cannot
        // even parse.
        //
        // So the claim is narrowed the way the framework narrows the other signing-name
        // collisions: the signing name must match AND the request must address this
        // service's own URI path space, which for an awsQuery service is the root path
        // (see `WIRE_PATH_SEGMENTS`). IAM Toolbox's `/authorization-details/…` is then
        // claimed by it alone.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == self.entry.identity().signing_name() => {
                claims_modeled_path(parts, WIRE_PATH_SEGMENTS)
            }
            // The two documents an account's outbound web identity ISSUER publishes are
            // fetched by whatever relying party holds a token vended against that issuer,
            // or bootstraps from its discovery document: anonymously, with no credential
            // scope and no `Action` to name this service by, and at the endpoint the issuer
            // itself announces. A GET of exactly those paths is therefore claimed here, so
            // each document resolves where this account announced it instead of reaching no
            // service at all.
            _ => {
                parts.method == http::Method::GET
                    && issuer_document_route(&parts.uri.to_string()).is_some()
            }
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Return `Some(op)` to serve a request the model's routing key cannot name
        // correctly; `None` (the default) lets the generated routing table resolve
        // it. `req` is the whole request, so a decision that depends on the body —
        // the `Action` form field on a query protocol, say — is reachable here.
        //
        // A credential the account has deactivated is refused HERE, before the
        // operation it named runs (see the module docs): the modelled walk
        // deserializes, validates and dispatches before any handler could answer, so
        // this pre-dispatch seam is the only place an authentication outcome is
        // reachable from.
        //
        // The request's auth schemes are unaffected: the router reads them off the
        // MODELLED operation the routing key names, so this changes nothing about which
        // requests must be signed.
        if self.presents_an_unusable_credential(req) {
            return Some(&self.unusable_credential);
        }
        // Every other request this model serves goes through the `awsQuery` envelope
        // wrapper, which resolves the operation from the SAME routing table the router
        // would have — this service's AUDITED table (see [`management_events`]), so the
        // call is recorded inside the typed call it delegates the whole
        // deserialize → validate → handle → serialize walk to — and adds the
        // `<RequestId>` the envelope carries beside `<Error>`. A request the routing table
        // does not answer is declined here, so it stays the router's own routing failure
        // (itself stated through the wrapped definition, which names the request too).
        self.entry
            .routes(req)
            .then_some(&self.envelope as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound, a pattern or an
        // enumerated set) to let this service state it. Returning `None` keeps the
        // framework's default wording; which refusals this service states differently,
        // and how, is [`stated_refusal`] — the member whose whole form AWS answers one
        // fixed sentence for, the code every input refusal of this service carries
        // ([`VALUE_REFUSAL_CODE`]), and the clauses [`refusal_clauses`] states for the
        // model-level walk's own violations.
        //
        // An authentication refusal is declined here: WHICH credential failure it was is
        // not in the structural refusal this seam is handed, so it is worded by the
        // definition layer instead (see the module docs).
        match refusal {
            Refusal::Invalid(violations) => stated_refusal(violations),
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
        //
        // An account's outbound web identity ISSUER publishes two documents this service's
        // model does not describe: its JSON Web Key Set, which every verifier of a token
        // vended against the issuer fetches to check that token's signature under the key
        // its `kid` names, and its OpenID Connect discovery document, which a relying party
        // reads the issuer and the location of that key set from. Both carry only PUBLIC
        // values and are served unauthenticated, which is what this seam gives: the router
        // consults it before it authenticates anything, so an anonymous GET reaches each
        // document the way it does at a real issuer.
        //
        // The documents are served whatever the feature FLAG stands at, because a disable
        // "does not affect tokens that were issued before the feature was disabled": a
        // verifier of such a token still has to find the key that signed it. An account
        // that never enabled the feature holds no issuer at all, so a request addressed to
        // one is answered as the issuer that does not exist rather than left to the
        // router's "no service claimed this request" refusal.
        if req.method() != "GET" {
            return None;
        }
        let (segment, document) = issuer_document_route(req.uri())?;
        let state = crate::state::service_state(&*self.engine)?;
        let rendered = {
            // A pure READ: no key is generated here and nothing is written.
            let store = state.lock();
            let held = store
                .outbound_web_identity_federation
                .as_ref()
                .filter(|held| held.issuer_identifier.ends_with(segment));
            let Some(held) = held else {
                let refusal = ::serde_json::json!({
                    "message": crate::state::outbound_federation_disabled_message(),
                });
                return ::service_common::openid_discovery::json_response(
                    404,
                    &refusal.to_string(),
                );
            };
            match document {
                WellKnownDocument::KeySet => held.jwks(),
                WellKnownDocument::OpenIdConfiguration => held.openid_configuration(),
            }
        };
        ::service_common::openid_discovery::json_response(200, &rendered.to_string())
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
