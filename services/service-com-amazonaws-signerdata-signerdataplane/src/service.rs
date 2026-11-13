// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-signerdata-signerdataplane` service: how it claims requests and how it words the
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
//!   correctly. EDITED HERE: the key and the handler stay the generated table's, but
//!   `GetRevocationStatus` — whose `signatureTimestamp` is bound to the query string —
//!   is served through [`QueryTimestampForms`], so every form the wire names an instant
//!   in binds and a value that names none is refused the way the protocol refuses one.
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
//! # The timestamp in the query string
//!
//! `GetRevocationStatus` binds `signatureTimestamp` to the query string (`@httpQuery`,
//! `@required`, target `Timestamp`, no `@timestampFormat` —
//! `smithy_query(query="operation", operation="GetRevocationStatus")`), and that member
//! decides the operation's answer: a revocation covers only signatures made at or after
//! its effective time. A client renders an instant onto a query string in more forms than
//! the schema the framework builds admits — it reads an `@httpQuery` timestamp as a
//! `Z`-zoned RFC-3339 string only — so the operation is served through the SHARED
//! [`QueryTimestampForms`] wrapper: every form the wire names an instant in (the
//! epoch-seconds number, and RFC 3339 with the numeric zone offset as well as with `Z`)
//! binds that instant, so this operation's own rules — and the modelled constraints of
//! the request's OTHER members — are what decide the answer; and a value that names no
//! instant at all is refused in the framework's envelope, since it never becomes a
//! modelled input of this service.

use service_common::query_timestamps::QueryTimestampForms;
use service_common::quoted_refusals::QuotedRefusals;
use smithy_server_runtime::{
    http, ErasedOperation, OperationRoutingTable, Refusal, Refused, Service,
};
use std::sync::Arc;

use aws_smithy_schema_server::constraints::{Violation, ViolationKind};

/// The first `/`-separated segment of `path`, or `None` for a path with no non-empty
/// first segment (`"/"`, `""`).
fn first_path_segment(path: &str) -> Option<&str> {
    let segment = path.trim_start_matches('/').split('/').next()?;
    (!segment.is_empty()).then_some(segment)
}

/// Whether `parts` addresses a path inside `path_segments` — the model-derived first URI
/// path segments this service declares (`crate::ROUTE_PATH_SEGMENTS`).
///
/// This mirrors the framework's own discriminator,
/// `emulator_server::can_handle_based_on_route_key_and_path`, which the generated
/// `can_handle` entry point uses. The framework helper takes a whole buffered
/// `http::Request`, which the `Service::can_handle` seam below does not receive, so the
/// rule is applied here over `Parts` instead of called. It is kept identical, including
/// both edge cases: an EMPTY list means the model published no usable discriminator and
/// is "no constraint", and the framework's own `ROOT_PATH_SEGMENT` sentinel is how a
/// service that must answer `POST /` claims the root path without claiming every path.
/// Matching is EXACT segment equality, deliberately never a prefix test.
fn claims_modeled_path(parts: &http::request::Parts, path_segments: &[&str]) -> bool {
    if path_segments.is_empty() {
        return true;
    }
    match first_path_segment(parts.uri.path()) {
        Some(segment) => path_segments.contains(&segment),
        None => path_segments.contains(&emulator_server::ROOT_PATH_SEGMENT),
    }
}

/// The `com-amazonaws-signerdata-signerdataplane` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// The operations that bind a timestamp to the query string, each the generated
    /// table's own operation wrapped in [`QueryTimestampForms`] and built from the SAME
    /// shared engine, so a wrapped operation reads the one store. Keyed by the routing key
    /// the protocol resolves, which is the key the generated table uses.
    query_timestamp_forms: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // The one operation this service models, and it binds a timestamp to the query
        // string (`signatureTimestamp`), so it is the whole of this table.
        let mut query_timestamp_forms = OperationRoutingTable::new();
        query_timestamp_forms.add_operation(
            "GetRevocationStatus",
            Box::new(QueryTimestampForms::over::<
                crate::ops::get_revocation_status::GetRevocationStatus<Eng>,
            >(QuotedRefusals::new(
                crate::ops::get_revocation_status::GetRevocationStatus::new(Arc::clone(&engine)),
            ))) as Box<dyn ErasedOperation>,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            query_timestamp_forms,
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
        // Claim a request whose resolved routing key names THIS service's ENDPOINT,
        // which the model gives as `data-signer` while the SigV4 signing name is
        // `signer`. The router resolves the key from the credential scope first and
        // falls back to the `Host` label, so `data-signer` is the key of a request
        // addressed at this service's own endpoint host
        // (`data-signer.<region>.amazonaws.com`) that carries no credential scope —
        // exactly the traffic this endpoint answers alone.
        //
        // The signing name itself is SHARED with AWS Signer
        // (`services/service-com-amazonaws-signer-wallabyservice`, endpoint `signer`),
        // and it is NOT claimed here: a request signed for `signer` resolves to that
        // key whichever host it was sent to, and the control plane holds the
        // revocation records such a request is answered from
        // (`RevokeSigningProfile` / `RevokeSignature` write them), so it serves the
        // signing name's traffic whole. Claiming the shared name here as well would
        // give every `signer`-signed request two claimants and the router would answer
        // `500 AmbiguousService` for all of it.
        //
        // The claim is also narrowed to this service's own modeled URI path space
        // (`/revocations`, see `claims_modeled_path`), so a request addressed at this
        // endpoint on a path this model does not publish reaches the router's own
        // `404 UnknownService` rather than this service.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) if key == ENDPOINT_PREFIX => {
                claims_modeled_path(parts, crate::ROUTE_PATH_SEGMENTS)
            }
            _ => false,
        }
    }

    /// Serve the request through the operation the generated table's own protocol names,
    /// wrapped in [`QueryTimestampForms`] so the `signatureTimestamp` this service's one
    /// operation binds to the query string is read in every form the wire names an instant
    /// in (see the module docs).
    ///
    /// This is NOT a re-route: the key is the one the generated table's own protocol
    /// resolves, and the operation it names is the one served. A request the protocol can
    /// name no operation for — this service's route declares five `@required` `@httpQuery`
    /// members, so one of them absent matches no route — resolves to `None` here exactly
    /// as it does for an unedited service, leaving [`Self::override_routing_error`] to
    /// state the refusal it earns.
    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        let key = self.entry.protocol().routing_key(req).ok()?;
        self.query_timestamp_forms.get(key)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // Two classes are reworded, both of them a member the request supplied
        // nothing for, because this service's own validator refuses an absent
        // member in a voice of its own rather than in the framework's schema-fact
        // clause ("Value null at '<member>' failed to satisfy constraint: Member
        // must not be null"):
        //
        // - a LIST member, for which that clause asserts something the wire
        //   cannot support — a query-bound list that is absent and a query-bound
        //   EMPTY list are the same request, so "null" names a constraint the
        //   request may not have violated. What is stated instead is the
        //   constraint such a member actually violated, naming it
        //   ([`missing_list_member_constraint`]).
        // - a SINGLE-VALUE member, stated in the `Invalid <member>: …` voice this
        //   service refuses its own rules in ([`absent_member_constraint`]).
        //
        // Every other refusal — a value present but outside a bound, a request
        // that left several members out — keeps the framework's own wording,
        // which is accurate for those.
        match refusal {
            Refusal::Invalid(violations) => missing_member_refusal(violations)
                .or_else(|| absent_member_refusal(violations)),
            _ => None,
        }
    }

    fn override_routing_error(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<aws_smithy_runtime_api::http::Response> {
        // The router calls this when a request reached this service but named no
        // modeled operation. This service models exactly ONE operation, and every one
        // of its five input members is `@required` + `@httpQuery`, so the REST
        // router's required-binding check rejects the only route it has whenever one
        // of those query parameters is absent — and the protocol default would then
        // answer 501 `NotImplemented`, claiming nobody implemented the operation when
        // what actually happened is that the request left a required parameter out.
        //
        // A `GET /revocations` that failed to route is therefore a GetRevocationStatus
        // missing required query parameters, and the service states it the way it
        // states any other refusal of the model's own rules: 400 `ValidationException`
        // naming each absent member. Anything else (another path, another method)
        // keeps the framework's protocol-correct default.
        let missing = missing_required_query_parameters(req)?;
        let violations: Vec<Violation> = missing
            .into_iter()
            .map(|member| Violation {
                path: member.to_string(),
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            })
            .collect();
        // Rendered through the generated entry, so this refusal lands in the same
        // envelope as one the boundary validator raises for a value that was
        // present but out of bounds — and through `refusal_response` with this
        // service's own wording (`override_refusal`), so a required member the
        // ROUTER found absent and one the VALIDATOR found absent are stated
        // identically.
        let chosen = self.override_refusal(&Refusal::Invalid(violations.clone()));
        Some(self.entry.refusal_response(
            aws_smithy_schema_server::ProtocolError::Invalid(violations),
            chosen,
            req,
        ))
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

/// This service's endpoint prefix, as the model gives it (`data-signer`) — distinct
/// from its SigV4 signing name (`signer`), which the generated identity carries.
/// `can_handle` claims a request that names either.
pub const ENDPOINT_PREFIX: &str = "data-signer";

/// The error code every refusal of a request's own values carries: the modelled
/// shape that is this service's 400
/// (`smithy_query(query="operation",operation="GetRevocationStatus")` lists
/// `ValidationException` @httpError 400).
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// Whether the input member `member_name` names is a LIST, read off the generated
/// input schema so the model stays the one source of which members are lists.
fn is_list_member(member_name: &str) -> bool {
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA
        .members()
        .iter()
        .any(|member| {
            member.http_query().map(|query| query.value()) == Some(member_name)
                && matches!(member.shape_type(), aws_smithy_schema::ShapeType::List)
        })
}

/// The two sentences this endpoint states for an input member a request supplied
/// nothing for, re-exported from the ONE home AWS Signer's wording has: this
/// endpoint and the control plane
/// (`services/service-com-amazonaws-signer-wallabyservice`) are two views of the
/// same API, so a sentence written out in each could come to differ between them.
/// Shared with the handler, so a member the boundary validator finds empty and
/// one the handler finds empty are stated identically.
pub use service_common::signer_input_refusals::{
    absent_member_constraint, missing_list_member_constraint,
};

/// How this service states a refusal whose every violation is a required LIST
/// member the request supplied nothing for: one clause per member, in the order
/// the violations arrive (the model's member order), each naming the member and
/// the constraint it actually violated.
///
/// `None` for every other refusal, which is then offered to
/// [`absent_member_refusal`] and, failing that, keeps the framework's own
/// wording: a value present but outside a declared bound is already described
/// accurately, with the offending value echoed.
fn missing_member_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let mut clauses = Vec::with_capacity(violations.len());
    for violation in violations {
        if violation.kind != ViolationKind::MissingRequiredMember || !is_list_member(&violation.path)
        {
            return None;
        }
        clauses.push(missing_list_member_constraint(&violation.path));
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: clauses.join("; "),
    })
}

/// How this service states a refusal of ONE required single-value member the
/// request supplied nothing for.
///
/// `None` for anything else — a LIST member ([`missing_member_refusal`]), a
/// value that was PRESENT and outside a bound, and a request that left SEVERAL
/// members out: the sentence states ONE rule, so a refusal carrying more than
/// one violation keeps the framework's enumerating envelope rather than reading
/// as though only one rule applied.
fn absent_member_refusal(violations: &[Violation]) -> Option<Refused> {
    let [violation] = violations else {
        return None;
    };
    if violation.kind != ViolationKind::MissingRequiredMember || is_list_member(&violation.path) {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: absent_member_constraint(&violation.path),
    })
}

/// The method of the one route this service's model binds (`GET /revocations`).
const REVOCATIONS_METHOD: &str = "GET";
/// The path of that route, as the single path segment it consists of. Compared the way
/// the router compares it: `route_lookup` trims the path's leading and trailing `/`
/// and splits the rest into segments, so `/revocations`, `/revocations/` and
/// `//revocations//` are all that one segment and anything else is another route.
const REVOCATIONS_PATH_SEGMENT: &str = "revocations";

/// The wire names of `GetRevocationStatus`'s `@httpQuery`-bound input members, read off
/// the generated input schema rather than written out here, so the model stays the one
/// source of the names.
///
/// Every one of them is also `@required`
/// (`smithy_query(query="operation",operation="GetRevocationStatus")` shows all five
/// input members as `required` + `httpQuery`), which is exactly the set the REST
/// router's generated required-binding table holds for this route — so a name here
/// missing from a request is a name the router refused to match the route for.
///
/// `@required` itself cannot be read back at runtime, which is why the filter is on the
/// query binding alone: `aws-smithy-schema` exposes no `required` accessor, and codegen
/// does not carry the trait on a member's generic `TraitMap` either — it CONSUMES
/// `@required` at generation time, to decide whether the Rust field is `Option<T>` and
/// to build `Route::with_required` (`vendor/smithy-macros/src/lib.rs`). The model fact
/// above is therefore enforced by test rather than by the type system: the
/// `a_missing_required_query_parameter_is_a_validation_error` suite asserts a 400 for
/// each of the five members omitted on its own, and a 200 for the complete request.
fn required_query_parameters() -> impl Iterator<Item = &'static str> {
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA
        .members()
        .iter()
        .filter_map(|member| member.http_query().map(|query| query.value()))
}

/// The required query parameters `req` leaves out, when `req` targets this service's
/// one route and leaves any out; `None` when it targets another route, or carries them
/// all (in which case it failed to route for some other reason and the framework's
/// default wording stands).
///
/// A parameter counts as present when its NAME is on the query string, whatever value
/// it carries, and the name is compared as the request spells it. That is precisely the
/// test the router's own required-binding check applies — it reads raw `key[=value]`
/// pairs (`parse_query_pairs`) and asks whether the name occurs
/// (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`), with no
/// percent-decoding of names on the routing path — so the two cannot disagree about
/// which parameters a request supplied. A present-but-invalid value is not this
/// function's business: the route then matches and the boundary validator refuses the
/// value itself.
fn missing_required_query_parameters(
    req: &aws_smithy_runtime_api::http::Request,
) -> Option<Vec<&'static str>> {
    if req.method() != REVOCATIONS_METHOD {
        return None;
    }
    // Parsed with `http::Uri` rather than by hand, so an absolute-form URI, an
    // origin-form one, and the userinfo/port/fragment variants all yield the same path
    // and query the router reads. A URI that does not parse is not this route's.
    let uri: http::Uri = req.uri().parse().ok()?;
    if uri.path().trim_matches('/') != REVOCATIONS_PATH_SEGMENT {
        return None;
    }

    let query = uri.query().unwrap_or_default();
    let supplied: Vec<&str> = query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
        .collect();
    let missing: Vec<&'static str> = required_query_parameters()
        .filter(|required| !supplied.contains(required))
        .collect();
    (!missing.is_empty()).then_some(missing)
}
