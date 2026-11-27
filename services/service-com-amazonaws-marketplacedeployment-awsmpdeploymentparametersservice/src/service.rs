// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice` service: how it claims requests and how it words the
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
//! ## The front door
//!
//! This service is not reached directly: the AWS gateway in front of it judges the
//! request envelope BEFORE the service runs, and a request it will not forward is
//! refused with its own HTTP 403 `ForbiddenException` (`"Forbidden"`,
//! [`crate::scaffolded::FrontDoorRefusal::forbidden`]) — a shape the model does not
//! declare, carrying nothing of the request.
//!
//! What the gateway refuses here is an OVERSIZED `@sensitive` payload: a value past the
//! maximum length its modelled shape declares. Such a payload is dropped at the gateway
//! instead of being described back to the caller, so the answer is the opaque 403 rather
//! than the modelled 400 `ValidationException` the walk would otherwise state — which
//! for a `@sensitive` member could only name the member and the bound anyway (see
//! `constraint_clause` below). Every OTHER constraint violation, including one on the
//! same member for being too SHORT or for naming no supported value, stays the modelled
//! `ValidationException`.
//!
//! WHY THAT PREDICATE, and what is NOT claimed by it: what AWS recorded is one fact —
//! a secret past `SecretString`'s modelled maximum is answered 403 `ForbiddenException`
//! while an in-limit one reaches the service. AWS's own gateway rule behind that 403 is
//! not observable from here: a byte threshold on the whole body cannot be derived from
//! the recording either, because the in-limit request that DOES reach the service and
//! the over-limit one that does not differ by a single character, so no body size
//! separates them, and a threshold low enough to separate them would refuse requests the
//! model declares valid (a 15000-character secret, a full 50-entry tag map). So the
//! refusal is keyed to the one thing this emulator can observe and the model can state:
//! the modelled maximum of the payload the request would hand over, on the member whose
//! value may not be echoed. `@sensitive` is a STAND-IN for the unknown gateway rule — it
//! marks the payload a gateway treats as opaque — and the bound is read off the model,
//! never written here, so an oversized NON-sensitive member stays the modelled 400 until
//! a recording says otherwise.
//!
//! Which member that is comes from the MODEL's own trail to it ([`SENSITIVE_MEMBER_PATHS`])
//! and never from the text of a path segment: a segment inside a collection is a value the
//! caller chose, so an entry of a tag map keyed like a member name is still a tag, and an
//! entry of it outside its own modelled bounds is the modelled 400 `ValidationException`
//! with its value echoed like any other tag's.
//!
//! [`EmulatedService::front_door_refuses`] is that check, reached through
//! [`Service::override_operation`] because the refusal PRECEDES the modelled walk:
//! constraint validation runs before a handler, so no handler can answer ahead of it.
//! The request's auth schemes are unaffected — the router reads them off the MODELLED
//! operation the routing key names — so a front-door refusal is still only reached by a
//! request whose signature verified.
//!
//! ## Naming the request a response answers
//!
//! Every response AWS sends names the request it answered, in the response header this
//! protocol reports that id in (`x-amzn-RequestId`) — on a success and on a failure alike.
//! It is a property of the RESPONSE, not of any modelled shape: no output or error shape
//! of this model declares it, so no handler can set it, and the framework's restJson1
//! renderers do not write it.
//!
//! That rule is not stated here: `service_common::query_error_envelope` owns it for every
//! emulated service, and this file only chooses the report its protocol takes (the header
//! alone — restJson1 has no envelope element for the id) and wires it at the layers every
//! response of this service passes through, which are three:
//!
//! * [`RequestIdHeaderOperation`], the wrapper [`Service::override_operation`] hands the
//!   router in place of each modelled operation, covers everything an operation answers —
//!   a success, a modelled error, and a constraint refusal raised while that operation was
//!   being served;
//! * [`RequestIdEnvelopeDefinition::header_only`], the definition wrapper
//!   [`Service::definition`] hands back, covers a response NO operation produced: a
//!   routing failure, a transport failure detected up front, and a pre-dispatch
//!   authentication refusal;
//! * [`FrontDoorAnswer`] renders its own refusal, so it passes that response through the
//!   same [`with_request_id_header`] directly.
//!
//! Nothing else about a response changes: the status, the other headers and every byte of
//! the body stay as the renderer wrote them.
//!
//! ## The body a refused request is answered with
//!
//! The model marks BOTH members of `ValidationException` `@required`, so every refusal
//! answered as that shape names the member it is about as well as stating a sentence. The
//! framework's pre-dispatch renderer cannot: `Service::override_refusal`, its only seam,
//! carries a code and a message and nothing else. So every modelled operation is served
//! through [`ServedOperation`], which states that refusal itself — from the framework's
//! own deserializer and constraint walk, in the SAME envelope `override_refusal` words —
//! in the modelled shape. [`modelled_refusal`] owns it.

use std::marker::PhantomData;

use aws_smithy_schema::serde::SerializableStruct;
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::validate;
use service_common::query_error_envelope::{
    with_request_id_header, RequestIdEnvelopeDefinition, RequestIdHeaderOperation,
};
use smithy_server_runtime::{
    http, BoxFuture, DeserializableStruct, ErasedOperation, ErasedServiceDefinition, Operation,
    Refusal, Refused, Service, Violation, ViolationKind,
};
use std::sync::Arc;

use crate::ops::types;
use crate::scaffolded::FrontDoorRefusal;

/// The modelled body a refused request is answered with — see the module docs.
mod modelled_refusal;

/// The error code this service answers an input-constraint violation with: the
/// modelled `ValidationException`, which every operation of this service declares
/// with `@httpError 400` (`smithy_query(query="service_info")` -> `common_errors`).
/// It is the framework's own default code for a refusal too, restated here because
/// `override_refusal` replaces the code and the message together.
const VALIDATION_EXCEPTION: &str = "ValidationException";

/// The members this service marks `@sensitive`, each as the WHOLE trail a validation
/// path spells it with, from the operation input down.
///
/// `smithy_query(query="shape",shape="SecretString")`: `SecretString` is the only
/// shape in this model carrying `sensitive: true`, and it is reached as
/// `DeploymentParameterInput$secretString`, which `PutDeploymentParameterRequest`
/// reaches through its `deploymentParameter` member — so a violation on it is
/// reported at `deploymentParameter.secretString`.
///
/// The trail is spelled out, rather than just the member's own name, because only a
/// STRUCTURE member contributes a segment the model named: a segment inside a
/// collection is the caller's own data (a map key, a list position), so a path's last
/// segment alone does not say which modelled member — if any — a violation is about.
/// `TagsMap`'s key shape admits the text of any member name
/// (`smithy_query(query="shape",shape="TagsMap")`: `TagKey`, `length 1..128`,
/// `^[a-zA-Z0-9/_+=.:@-]+$`), and a violation on an entry's VALUE is reported at
/// `tags.<that key>`, so matching the last segment alone would let a caller's choice of
/// tag key decide how this service answers.
const SENSITIVE_MEMBER_PATHS: &[&str] = &["deploymentParameter.secretString"];

/// Whether `path` names a member this service marks `@sensitive`.
fn names_a_sensitive_member(path: &str) -> bool {
    SENSITIVE_MEMBER_PATHS.contains(&path)
}

/// Whether `violation` is the one the front door answers for: a `@sensitive` payload
/// past the MAXIMUM length its modelled shape declares.
///
/// The bound itself is never spelled here — it is read off the violation the model's own
/// validator reported — so this holds for every `@sensitive` length-bounded member of
/// this model, and moves with the model if a bound changes. A violation on the same
/// member for being too SHORT is not this refusal: it is the modelled
/// `ValidationException`, which states the member and the bound without echoing the
/// value. See the module docs for why the predicate is keyed this way and what it does
/// not claim.
fn is_oversized_sensitive_payload(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(_))
        && names_a_sensitive_member(&violation.path)
}

/// The members of this model that carry a tag MAP, each as the whole trail a
/// validation path spells it with, from the operation input down.
///
/// `smithy_query(query="shape",shape="TagsMap,Tags")`: the tag maps of this model are
/// reached as `PutDeploymentParameterRequest$tags` and
/// `StartDeploymentConfigurationJobRequest$tags` (the constrained `TagsMap`,
/// `length max 50`) and `TagResourceRequest$tags` (`Tags`, which the model leaves
/// unconstrained), so all three are reported at the path `tags`.
///
/// Spelled as the whole trail for the reason [`SENSITIVE_MEMBER_PATHS`] gives: only a
/// STRUCTURE member contributes a segment the model named, so a path ending in a
/// caller's own tag KEY (`tags.<that key>`, where a violation on an entry's value is
/// reported) is not this member and keeps the modelled answer.
const TAG_MAP_MEMBER_PATHS: &[&str] = &["tags"];

/// The tagging front end's own refusal for `violations`, or `None` when none of them
/// is one of its.
///
/// AWS's per-resource tag COUNT is a limit of the tagging front end that runs ahead of
/// this service's modelled input validation, not a rule of this service: its recorded
/// answer carries the generic `BadRequestException` identity
/// ([`crate::scaffolded::BAD_REQUEST_CODE`]) and the one fixed sentence
/// [`crate::state::too_many_tags`] states, rather than the modelled
/// `ValidationException` an SDK unmarshals into a typed error. The model restates that
/// count for its constrained copy of the tag map, which is why the violation reaches
/// this service's refusal path at all — so it is recognised here, on the MAP's own
/// length violation, and handed the identity the front end gives it.
///
/// Because that layer runs first, its refusal takes precedence over every other
/// violation of the same request; the service's own modelled validation answers what
/// is left. The sentence is the same one the handlers state for the half of the limit
/// that needs the stored record, so no layer can word this refusal differently.
fn tagging_front_end_refusal(violations: &[Violation]) -> Option<crate::scaffolded::BadRequest> {
    violations
        .iter()
        .any(is_tag_count_violation)
        .then(crate::state::too_many_tags)
}

/// Whether `violation` is a tag map past the number of entries the map a tagging
/// member targets admits — the MAP's own `@length` maximum, not a bound on one entry.
fn is_tag_count_violation(violation: &Violation) -> bool {
    matches!(violation.kind, ViolationKind::MaxLength(_))
        && TAG_MAP_MEMBER_PATHS.contains(&violation.path.as_str())
}

/// One refusal the front door answers with, as a pre-dispatch operation.
///
/// WHY this is a seam and not a line in a handler: the refusal happens BEFORE the
/// modelled walk — the gateway judges the request envelope ahead of the service — and
/// the walk refuses a modelled constraint violation before any handler runs, so the
/// answer is not reachable from inside `handle`. [`Service::override_operation`] is the
/// one hook that sees the whole request ahead of that walk.
struct FrontDoorAnswer(FrontDoorRefusal);

impl ErasedOperation for FrontDoorAnswer {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        _ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // Rendered through this service's own protocol, so the refusal carries the
            // same envelope (and the same `x-amzn-errortype` header) as every other
            // error it answers. The status and the error code come from the refusal
            // itself: the shape is not in the model, so there is no modelled error to
            // read them off. It is the one response this file renders itself, so it names
            // the request it answers here (see the module docs).
            with_request_id_header(
                svc.definition()
                    .protocol()
                    .serialize_error(
                        &self.0,
                        &crate::scaffolded::FRONT_DOOR_SCHEMA,
                        self.0.code,
                        self.0.status,
                    )
                    .unwrap_or_else(|e| svc.definition().framework_error_response(e, req)),
            )
        })
    }
}

/// One modelled operation of this service, served with the refusal it owes stated in the
/// model's own `ValidationException` body.
///
/// NOT a re-route and not an operation of its own: the operation served is the one this
/// service's routing table names for the request, reached through the shared
/// [`RequestIdHeaderOperation`] so every response still names the request it answers. What
/// this wrapper adds is the one answer that layer cannot give: a request the modelled walk
/// refuses before any handler runs is refused with BOTH members the modelled shape marks
/// `@required` — the sentence and the member it blames — rather than with the sentence
/// alone, which is all the framework's refusal seam can carry. See [`modelled_refusal`].
struct ServedOperation<Eng> {
    /// The shared wrapper every response of this service passes through.
    request_id: RequestIdHeaderOperation,
    /// The engine type this service was built with, carried as a type alone: the refusal
    /// below reads each request through the modelled input schema of the operation the
    /// routing key names, and those operations are generic over it. No engine value is
    /// held here — the operations hold the one shared engine.
    engine: PhantomData<fn() -> Eng>,
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ServedOperation<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r aws_smithy_runtime_api::http::Request,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> BoxFuture<'r, aws_smithy_runtime_api::http::Response> {
        Box::pin(async move {
            // Scoped so the borrowed `&dyn ErasedServiceDefinition` — which is not `Sync`,
            // while this future is — is dropped before the await below.
            let refusal = {
                let protocol = svc.definition().protocol();
                modelled_refusal::refusal_response::<Eng>(protocol, req)
            };
            match refusal {
                // Rendered here, so it names the request it answers here (see the module
                // docs); everything else about it is the protocol renderer's.
                Some(response) => with_request_id_header(response),
                None => self.request_id.run(svc, req, ctx).await,
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", which is the
        // operation this wrapper serves — so wrapping cannot weaken the schemes the model
        // declares for it.
        None
    }
}

/// Render one constraint violation the way this service states it.
///
/// The envelope and the clause skeleton are AWS's standard constraint report —
/// `Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>` — and two of its details are rules of how the report is BUILT
/// rather than of the model it is built from:
///
/// * a `@pattern` is reported as the expression a value must satisfy, WITHOUT the
///   `^` / `$` anchoring the model spells it with (see [`unanchored`]): Smithy's
///   `@pattern` is an unanchored find, so those characters are the model's way of
///   making the match total, not part of the character class a client may send;
/// * a `@sensitive` member's value is NOT echoed — the clause names the member and
///   the constraint and stops there, so a rejected secret never reaches a client's
///   logs through an error body.
///
/// Everything else — the member path, the bound a `@length` clause states, the
/// membership of an enumerated value set — is read off the violation the framework
/// reports.
///
/// Returns `None` for a violation class this service has no wording for, which lets
/// [`constraint_envelope`] decline the whole refusal and keep the framework's
/// wording rather than invent a sentence for it ([`ViolationKind`] is
/// `#[non_exhaustive]`, so a class can be added upstream).
fn constraint_clause(violation: &Violation) -> Option<String> {
    let requirement = match &violation.kind {
        ViolationKind::MissingRequiredMember => "Member must not be null".to_string(),
        ViolationKind::MinLength(min) => {
            format!("Member must have length greater than or equal to {min}")
        }
        ViolationKind::MaxLength(max) => {
            format!("Member must have length less than or equal to {max}")
        }
        ViolationKind::MinValue(min) => {
            format!("Member must have value greater than or equal to {min}")
        }
        ViolationKind::MaxValue(max) => {
            format!("Member must have value less than or equal to {max}")
        }
        ViolationKind::Pattern(pattern) => format!(
            "Member must satisfy regular expression pattern: {}",
            unanchored(pattern)
        ),
        ViolationKind::EnumValue(allowed) => {
            format!("Member must satisfy enum value set: [{}]", allowed.join(", "))
        }
        ViolationKind::UniqueItems => "Member must have unique values".to_string(),
        _ => return None,
    };
    let subject = match (&violation.kind, &violation.value) {
        // A value the validating walk itself WITHHELD is a withheld value and not an
        // absent one: the walk drops a `@sensitive` member's value as it records the
        // violation, so an empty value half is not evidence that the member was absent.
        _ if violation.sensitive => "Value".to_string(),
        // An absent member is reported as the null it was: a fact of the request's
        // shape, not a rendering of a value the caller sent.
        (ViolationKind::MissingRequiredMember, _) => "Value null".to_string(),
        (_, None) => "Value null".to_string(),
        // A `@sensitive` member this service recognises names itself and nothing of what
        // it held, for a value that reached here with the member still named.
        (_, Some(_)) if names_a_sensitive_member(&violation.path) => "Value".to_string(),
        (_, Some(shown)) => format!("Value '{shown}'"),
    };
    Some(format!(
        "{subject} at '{}' failed to satisfy constraint: {requirement}",
        violation.path
    ))
}

/// A `@pattern` expression without the anchors a model may spell it with. A `$`
/// escaped by a backslash is a literal dollar sign inside the expression, not the
/// end anchor, so it stays.
fn unanchored(pattern: &str) -> &str {
    let body = pattern.strip_prefix('^').unwrap_or(pattern);
    match body.strip_suffix('$') {
        Some(trimmed) => {
            let backslashes = trimmed.len() - trimmed.trim_end_matches('\\').len();
            if backslashes % 2 == 0 {
                trimmed
            } else {
                body
            }
        }
        None => body,
    }
}

/// The `ValidationException` message for a refusal: the AWS envelope
/// (`N validation error(s) detected: `) over one clause per violation.
///
/// `None` declines the whole refusal — see [`constraint_clause`].
fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let clauses: Vec<String> = violations
        .iter()
        .map(constraint_clause)
        .collect::<Option<Vec<_>>>()?;
    match clauses.len() {
        0 => None,
        1 => Some(format!("1 validation error detected: {}", clauses[0])),
        n => Some(format!(
            "{n} validation errors detected: {}",
            clauses.join("; ")
        )),
    }
}

/// The `com-amazonaws-marketplacedeployment-awsmpdeploymentparametersservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, with its three framework-level error renderers wrapped so a
    /// response no operation produced still names the request it answers.
    entry: RequestIdEnvelopeDefinition<crate::ServiceDefinition<Eng>>,
    /// Answers a request the front door will not forward: HTTP 403 `ForbiddenException`.
    forbidden: FrontDoorAnswer,
    /// Serves every modelled operation, so each response names the request it answers and
    /// a request refused before any handler runs is refused in the modelled shape.
    /// Held, not built per request: `override_operation` returns a reference to it.
    served: ServedOperation<Eng>,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        Self {
            entry: RequestIdEnvelopeDefinition::header_only(crate::ServiceDefinition::new(engine)),
            forbidden: FrontDoorAnswer(FrontDoorRefusal::forbidden()),
            served: ServedOperation {
                request_id: RequestIdHeaderOperation,
                engine: PhantomData,
            },
        }
    }

    /// Whether the front door refuses this request outright — see the module docs.
    ///
    /// The input is read through the PROTOCOL's own binding against the operation's own
    /// model-derived input schema, so the value judged is the value dispatch would hand
    /// the handler (URI labels percent-decoded, body members parsed), and the bound
    /// checked is the model's, through the same validator the walk runs.
    /// Deserialization does not enforce constraints, which is what lets the gateway
    /// judge a value the modelled constraint would otherwise have refused.
    ///
    /// One arm per operation whose input carries a member the gateway judges — today
    /// `PutDeploymentParameter`, the only operation of this model carrying a
    /// `@sensitive` member. Every other operation's request has nothing for the front
    /// door to judge, so the modelled walk answers it.
    fn front_door_refuses(&self, req: &aws_smithy_runtime_api::http::Request) -> bool {
        let protocol = self.definition().protocol();
        let Ok(key) = protocol.routing_key(req) else {
            return false;
        };
        if key
            != <crate::ops::put_deployment_parameter::PutDeploymentParameter<Eng> as Operation>::NAME
        {
            return false;
        }
        oversized_sensitive_payload::<types::PutDeploymentParameterInput>(
            protocol,
            req,
            types::PUT_DEPLOYMENT_PARAMETER_INPUT_SCHEMA,
        )
    }
}

/// Whether the request carries a `@sensitive` payload past its modelled maximum length,
/// read through `protocol` against `schema`.
///
/// Generic over the input shape so a second operation carrying a `@sensitive` member
/// needs one call here rather than a second copy of the judgement. A request the protocol
/// cannot bind at all is NOT judged: the framework's own refusal answers it, and the
/// gateway's answer to a malformed envelope is not this one.
///
/// COST, knowingly paid: this binds and validates the request once here and dispatch
/// binds and validates it again, so a request carrying a large payload is parsed twice.
/// The framework offers no seam for handing the already-bound input on to dispatch (the
/// pre-dispatch hook answers with an operation, not with an input), and the alternative —
/// answering from inside the handler — cannot work at all, because the modelled walk
/// refuses the request before any handler runs. The work is bounded by the same payload
/// the request already carries.
fn oversized_sensitive_payload<I>(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
    req: &aws_smithy_runtime_api::http::Request,
    schema: &'static Schema,
) -> bool
where
    I: DeserializableStruct + SerializableStruct,
{
    let Ok(mut deserializer) = protocol.deserialize_input(req, schema) else {
        return false;
    };
    let Ok(input) = I::deserialize(&mut *deserializer) else {
        return false;
    };
    validate(&input, schema)
        .iter()
        .any(is_oversized_sensitive_payload)
}

impl<Eng: Send + Sync + 'static> Service for EmulatedService<Eng> {
    fn definition(&self) -> &dyn smithy_server_runtime::ErasedServiceDefinition {
        // The model-derived half of this service — identity, protocol, routing
        // table, and the default error renderers — lives in the generated entry.
        // The router reads it through this one accessor. Do not edit the entry: what
        // is handed back is the shared `RequestIdEnvelopeDefinition`, which reads
        // identity, protocol and the routing table straight off that entry and adds the
        // request-id header to the three error renderers (see the module docs).
        &self.entry
    }

    fn can_handle(&self, parts: &http::request::Parts) -> bool {
        // This service signs as `aws-marketplace` — a signing name it SHARES with
        // every other AWS Marketplace-family service (Catalog, Reporting, Discovery,
        // Agreement, Entitlement, Metering), each serving its own endpoint
        // (`deployment-marketplace` here). The router's routing key comes from the
        // SigV4 credential scope first, so all of them resolve to `aws-marketplace`
        // and matching on the signing name ALONE makes every one of them claim every
        // request — which the router answers `500 AmbiguousService`.
        //
        // So the claim is narrowed to this service's own modelled wire surface, by the
        // one rule `service-common` states for every service under a shared signing
        // name. This model is `restJson1`: its surface is the URI path space its
        // operations bind (`/catalogs/…`, `/tags/…`), disjoint from every sibling's,
        // which is what `claims_request` matches against the generated
        // `ROUTE_PATH_SEGMENTS` when a service declares no RPC protocol.
        service_common::claims_request(
            parts,
            self.entry.identity().signing_name(),
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
        //
        // This service is reached through a gateway that answers before it (see the
        // module docs): a request carrying a `@sensitive` payload past its modelled
        // maximum is refused there and never reaches the modelled walk at all. That
        // answer is pre-dispatch, which is what this hook is.
        if self.front_door_refuses(req) {
            return Some(&self.forbidden as &dyn ErasedOperation);
        }
        // NOT a re-route: every other request is served by the operation this service's
        // own routing table names for it, through the shared `RequestIdHeaderOperation` —
        // the one place every operation's response passes, so no operation can answer
        // without naming the request it answered. A request the table does not answer
        // declines here exactly as it would there, leaving the router's routing-error path
        // unchanged.
        self.entry
            .routes(req)
            .then_some(&self.served as &dyn ErasedOperation)
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // This service states a constraint violation in AWS's standard validation
        // report, which `constraint_envelope` builds: a modelled `@pattern` quoted
        // WITHOUT its `^` / `$` anchors, and a `@sensitive` member's value not
        // echoed at all. The code stays the modelled `ValidationException` (HTTP
        // 400), which is what the framework would have answered — only the wording
        // changes.
        //
        // `ServedOperation` answers a refusal it can state in the modelled shape's whole
        // BODY, so what still reaches here is what that path declines: a violation naming
        // no member, and the fallback when the modelled error cannot be serialized.
        // Worded from the same envelope, so the two cannot disagree.
        //
        // An authentication refusal keeps the framework's wording: it is not an
        // input-constraint failure.
        match refusal {
            Refusal::Invalid(violations) => {
                // The tagging front end's count bound runs ahead of this service's
                // modelled validation, so its own identity answers first — the same
                // refusal `ServedOperation` renders in full, worded from the one place
                // that states it, so the two paths cannot disagree.
                if let Some(refused) = tagging_front_end_refusal(violations) {
                    return Some(Refused {
                        code: crate::scaffolded::BAD_REQUEST_CODE.to_string(),
                        message: refused.message,
                    });
                }
                Some(Refused {
                    code: VALIDATION_EXCEPTION.to_string(),
                    message: constraint_envelope(violations)?,
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
        // awsJson -> 404, rpcv2Cbor -> 404, rest -> 501) through the shared
        // `RequestIdEnvelopeDefinition`, which names the request that failure answers.
        // Return `Some(resp)` to word that routing failure in your own envelope instead.
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
