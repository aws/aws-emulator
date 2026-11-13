// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-signer-wallabyservice` service: how it claims requests and how it words the
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
//!   before any handler ran. This service rewords the classes AWS Signer states
//!   a rule of its own for — an input member the model binds a LIST to the query
//!   string, for which the request supplied nothing; a signing-profile name
//!   outside the character class its shape admits; a payload outside the byte
//!   window its blob declares — and keeps the framework's wording for every
//!   other refusal. See the inline note on that method.
//! - `override_routing_error` — how this service words a request that reached it
//!   but named no modeled operation. The default returns `None`, so the entry
//!   renders the protocol-correct default (query -> 400 InvalidAction, awsJson ->
//!   404, …). Return `Some(resp)` to answer in your own envelope if desired.
//! - `override_framework_error` — how this service words a transport-level
//!   failure the orchestrator detects up front (e.g. a malformed `aws-chunked`
//!   body). The default returns `None`, keeping the entry's protocol-correct
//!   rendering. Return `Some(resp)` to reword it if desired.
//!
//! `override_operation` is also where this service states the ENVELOPE of an
//! answer that has nothing to report: the operations whose modelled output
//! carries no members are served wrapped in [`NoContentAnswer`], at the one place
//! every response of theirs passes through rather than once per operation. See
//! [`NO_CONTENT_STATUS`] and [`EMPTY_BODY_LENGTH`].
//!
//! It is also where the operations that bind a TIMESTAMP to the query string are served
//! through the shared [`QueryTimestampForms`] wrapper. A client renders an instant onto a
//! query string in more forms than the schema the framework builds from the model admits
//! — it reads an `@httpQuery` timestamp as a `Z`-zoned RFC-3339 string only — so without
//! the wrapper a request carrying the epoch-seconds number, or the numeric zone offset RFC
//! 3339 equally admits, is refused for its timestamp before any of the operation's own
//! rules (or any other member's modelled bound) is applied. With it, every form names its
//! instant, and a value that names NO instant is refused in the framework's own envelope,
//! since such a value never becomes a modelled input of this service.
//!
//! The model-derived half of this service — its identity, wire protocol, routing
//! table, and the default error renderers — lives in the generated `ServiceDefinition`
//! and is reached, not edited, through the one `definition` method below. You
//! do not write it; editing the entry would let this file drift from the model.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema_server::constraints::{
    LengthTrait, PatternTrait, Violation, ViolationKind,
};
use aws_smithy_types::body::SdkBody;
use service_common::query_timestamps::QueryTimestampForms;
use service_common::quoted_refusals::QuotedRefusals;
use smithy_server_runtime::{
    http, BoxFuture, ErasedOperation, Operation, OperationContext, OperationRoutingTable, Refusal,
    Refused, Service,
};
use std::sync::Arc;

/// The status a successful call whose modelled output carries no members answers
/// with: `204 No Content`.
///
/// Recorded behaviour, not a model fact: no operation of this service declares an
/// `@httpResponseCode` and every one of them routes with the protocol-default
/// `200`, yet the recorded AWS answer to each call whose output has nothing to
/// report — the three bound to `smithy.api#Unit` and the two tagging writers,
/// whose response structures declare no members at all — is `204` with no body.
/// It is a fact about the ANSWER (there is no document to carry), so it is stated
/// once here and referenced by each of those operations rather than spelled out
/// per operation: a client that treats `204` as the success signal, or that
/// refuses to parse a bodyless `200` as JSON, reads the same envelope from all of
/// them.
pub const NO_CONTENT_STATUS: u16 = 204;

/// The length an answer with no body states.
///
/// A response that carries no body still has a length, and the recorded AWS
/// answer to a Signer call whose output carries none states it: a client (or an
/// intermediary) that frames a bodyless response by its stated length rather
/// than by the status alone has nothing to read otherwise. The value is a fact
/// about the body — it has no bytes — so it is stated for every such answer
/// rather than per operation.
///
/// It is the LAST word only up to the transport: the HTTP/1.1 writer this
/// emulator is served by removes `content-length` from a `204` (RFC 9110
/// §15.3.5 forbids it there), and that writer is a dependency of the vendored
/// HTTP layer rather than anything this crate decides — so a caller on the
/// in-process front door (the inter-service callback path and the SDK
/// connector, which both serve `dispatch`'s response unchanged) reads it, while
/// one on a socket reads the status and the empty body alone.
const EMPTY_BODY_LENGTH: &str = "0";

/// The response header that states it.
const CONTENT_LENGTH: &str = "content-length";

/// The operations whose modelled output carries NO members, and whose successful
/// answers therefore carry no document and state [`EMPTY_BODY_LENGTH`].
///
/// Source: the output shapes this model binds
/// (`smithy_query(query="digest", model_path="services/\
/// service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")` plus
/// `smithy_query(query="shape", shape="TagResourceResponse,UntagResourceResponse")`)
/// — `CancelSigningProfile`, `RevokeSignature` and `RevokeSigningProfile` output
/// `smithy.api#Unit`, and `TagResource` / `UntagResource` output structures
/// declaring `{}`. Every other operation of this service answers a modelled
/// structure with members, so its body is a document and its length is the
/// document's.
///
/// The wrapper decides on the ANSWER rather than on the operation, so a response
/// of one of these that does carry a body (a refusal's error document) keeps both
/// the body and the length its renderer stated.
macro_rules! serve_no_content_answers {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(QuotedRefusals::over::<
                    crate::ops::$module::$operation<Eng>,
                >(NoContentAnswer::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                ))) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// Wire one `crate::ops::<module>::<Operation>` per line into `table`, each wrapped in
/// [`QuotedRefusals`] alone — the composition of an operation that neither answers
/// without a document nor binds a timestamp to the query string.
macro_rules! serve_quoted_refusals {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(QuotedRefusals::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                )) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// The operations whose input binds a TIMESTAMP to the query string, and whose values
/// therefore have to be read in every form the wire names an instant in (see the module
/// docs and `service_common::query_timestamps`).
///
/// Source: the input members this model binds (`OPERATIONS.md`, the one place this
/// crate's whole surface is stated) — `GetRevocationStatus`' `signatureTimestamp` and
/// `ListSigningJobs`' `signatureExpiresBefore` / `signatureExpiresAfter` are the only
/// `(timestamp, @httpQuery)` members of this service; every other timestamp it takes is
/// bound to a body, which the protocol's own codec already reads in more than one form.
/// Which NAMES are rewritten for a wrapped operation is read off that operation's own
/// input schema by the wrapper, not written out here.
macro_rules! serve_query_timestamp_forms {
    ($table:expr, $engine:expr, $($module:ident :: $operation:ident),+ $(,)?) => {
        $(
            $table.add_operation(
                <crate::ops::$module::$operation<Eng> as Operation>::NAME,
                ::std::boxed::Box::new(QueryTimestampForms::over::<
                    crate::ops::$module::$operation<Eng>,
                >(QuotedRefusals::new(
                    crate::ops::$module::$operation::new(::std::sync::Arc::clone(&$engine)),
                ))) as ::std::boxed::Box<dyn ErasedOperation>,
            );
        )+
    };
}

/// One modelled operation, served with the envelope an answer that has nothing to
/// report carries.
///
/// Everything else about the operation is the operation's: the request walk, the
/// status, every other header and every refusal come from the wrapped
/// `impl Operation`, and the auth schemes are the wrapped operation's own —
/// wrapping must not change which schemes sign a request.
struct NoContentAnswer<O> {
    operation: O,
}

impl<O> NoContentAnswer<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for NoContentAnswer<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { without_a_document(self.operation.run(svc, req, ctx).await) })
    }

    /// The wrapped operation's modelled auth schemes.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// `resp` with the envelope of an answer that has nothing to report — no body,
/// and the length of that nothing stated — and anything else left exactly as the
/// operation answered it.
///
/// It applies only to a SUCCESS: a refusal of one of these operations carries an
/// error document, which is a body with something in it and keeps the length its
/// own renderer stated. The modelled output of each wrapped operation declares no
/// members, so a success has no member to serialize: the empty JSON object the
/// REST codec writes for a memberless structure is not a document the caller can
/// read anything out of, and the recorded AWS answer carries no bytes at all. A
/// streamed body — whose length is not known here — is left to the transport. The
/// insert is FALLIBLE: a header the HTTP layer cannot represent is dropped rather
/// than turned into a transport failure.
fn without_a_document(mut resp: RtResponse) -> RtResponse {
    if !resp.status().is_success() {
        return resp;
    }
    if resp.body().bytes().is_some_and(|bytes| !bytes.is_empty()) {
        *resp.body_mut() = SdkBody::empty();
    }
    if resp.body().bytes().is_some_and(<[u8]>::is_empty)
        && resp.headers().get(CONTENT_LENGTH).is_none()
    {
        let _ = resp
            .headers_mut()
            .try_insert(CONTENT_LENGTH, EMPTY_BODY_LENGTH);
    }
    resp
}

/// Every operation of this service, each paired with the input schema that carries its
/// `@httpQuery` bindings.
///
/// Source: the operations this model binds (`smithy_query(query="digest",
/// model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")`). Only the operation NAME — the key the
/// generated routing table holds it under — is written out here: not a method, not a
/// path, not one parameter name. The parameters come off that operation's own input
/// schema ([`aws_smithy_schema::Schema::members`]) and the route itself stays the
/// router's, so the model is the one source of what a route is and which parameters it
/// binds.
const OPERATION_INPUTS: &[(&str, &::aws_smithy_schema::Schema)] = &[
    (
        "AddProfilePermission",
        crate::ops::types::ADD_PROFILE_PERMISSION_INPUT_SCHEMA,
    ),
    (
        "CancelSigningProfile",
        crate::ops::types::CANCEL_SIGNING_PROFILE_INPUT_SCHEMA,
    ),
    (
        "DescribeSigningJob",
        crate::ops::types::DESCRIBE_SIGNING_JOB_INPUT_SCHEMA,
    ),
    (
        "GetRevocationStatus",
        crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA,
    ),
    (
        "GetSigningPlatform",
        crate::ops::types::GET_SIGNING_PLATFORM_INPUT_SCHEMA,
    ),
    (
        "GetSigningProfile",
        crate::ops::types::GET_SIGNING_PROFILE_INPUT_SCHEMA,
    ),
    (
        "ListProfilePermissions",
        crate::ops::types::LIST_PROFILE_PERMISSIONS_INPUT_SCHEMA,
    ),
    (
        "ListSigningJobs",
        crate::ops::types::LIST_SIGNING_JOBS_INPUT_SCHEMA,
    ),
    (
        "ListSigningPlatforms",
        crate::ops::types::LIST_SIGNING_PLATFORMS_INPUT_SCHEMA,
    ),
    (
        "ListSigningProfiles",
        crate::ops::types::LIST_SIGNING_PROFILES_INPUT_SCHEMA,
    ),
    (
        "ListTagsForResource",
        crate::ops::types::LIST_TAGS_FOR_RESOURCE_INPUT_SCHEMA,
    ),
    (
        "PutSigningProfile",
        crate::ops::types::PUT_SIGNING_PROFILE_INPUT_SCHEMA,
    ),
    (
        "RemoveProfilePermission",
        crate::ops::types::REMOVE_PROFILE_PERMISSION_INPUT_SCHEMA,
    ),
    (
        "RevokeSignature",
        crate::ops::types::REVOKE_SIGNATURE_INPUT_SCHEMA,
    ),
    (
        "RevokeSigningProfile",
        crate::ops::types::REVOKE_SIGNING_PROFILE_INPUT_SCHEMA,
    ),
    ("SignPayload", crate::ops::types::SIGN_PAYLOAD_INPUT_SCHEMA),
    (
        "StartSigningJob",
        crate::ops::types::START_SIGNING_JOB_INPUT_SCHEMA,
    ),
    ("TagResource", crate::ops::types::TAG_RESOURCE_INPUT_SCHEMA),
    (
        "UntagResource",
        crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
    ),
];

/// `req` with every `@httpQuery` parameter `input` binds that `req` does not already
/// carry ADDED to its query string — or `None` when it carries them all, so there is
/// nothing to add, and when its target cannot be read as a URI.
///
/// The added parameters carry no value, because the router's required-binding check asks
/// only whether the parameter's NAME occurs on the query string
/// (`required_binding_score`, `vendor/smithy-schema-server/src/rest_xml.rs`, which reads
/// raw `key[=value]` pairs and compares names): a bare name is exactly "supplied" to it.
/// Everything else about the request — its method, its path, its headers — is carried
/// over unchanged, so the route this answers for is the route the request addressed.
///
/// The result is a PROBE, asked of the router and then dropped: it is never deserialized
/// and never served, so the values the added names do not carry are never read. The
/// request the operation is then run against is the caller's own, which still leaves its
/// required parameter out — and is refused for exactly that.
///
/// Parsed with `http::Uri` rather than by hand, so an absolute-form target, an
/// origin-form one, and the userinfo/port/fragment variants all yield the path and query
/// the router itself reads.
fn with_query_parameters_supplied(
    req: &RtRequest,
    input: &::aws_smithy_schema::Schema,
) -> Option<RtRequest> {
    let uri = req.uri().parse::<http::Uri>().ok()?;
    let path_and_query = uri.path_and_query()?;
    let carried = path_and_query.query().unwrap_or_default();
    // Split the way the router splits it: raw pairs, the name being everything before
    // the first `=`, so the two cannot disagree about what a request supplied.
    let supplied: Vec<&str> = carried
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| pair.split_once('=').map_or(pair, |(name, _)| name))
        .collect();

    let mut query = carried.to_string();
    let mut added = false;
    for member in input.members() {
        let Some(parameter) = member.http_query().map(|binding| binding.value()) else {
            continue;
        };
        if supplied.contains(&parameter) {
            continue;
        }
        if !query.is_empty() {
            query.push('&');
        }
        query.push_str(parameter);
        added = true;
    }
    if !added {
        return None;
    }

    let mut probe = RtRequest::new(::aws_smithy_types::body::SdkBody::empty());
    probe
        .set_uri(format!("{}?{query}", path_and_query.path()).as_str())
        .ok()?;
    probe.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        // `append`, not `insert`: a repeated or multi-valued header keeps every value, so
        // a route discriminated by a header reads the same on the probe as on `req`. The
        // append is FALLIBLE: a header the HTTP layer cannot represent is dropped rather
        // than turned into a panic on a request path.
        let _ = probe
            .headers_mut()
            .try_append(name.to_string(), value.to_string());
    }
    Some(probe)
}

/// The error code every refusal of a request's own values carries on this
/// service's 400: the modelled shape's own name (see
/// [`crate::state::ERROR_CODE_VALIDATION`]), which is also the code the
/// framework's own refusal renderer picks — so rewording a refusal's message
/// leaves the code a client branches on exactly where it was.
const VALIDATION_EXCEPTION_CODE: &str = "ValidationException";

/// The input schemas of the operations that bind a LIST input member to the
/// query string — the only two this model has, `GetRevocationStatus`'s
/// `certificateHashes` and `UntagResource`'s `tagKeys`
/// (`smithy_query(query="operation",operation="GetRevocationStatus,UntagResource",\
/// model_path="services/service-com-amazonaws-signer-wallabyservice/smithy-model/\
/// com-amazonaws-signer-wallabyservice.json")`).
///
/// The member NAMES are deliberately not written out here: the schemas are, and
/// [`is_query_bound_list_member`] reads the binding and the list-ness off them,
/// so the model stays the one source of which members the rule covers.
const QUERY_BOUND_LIST_INPUTS: &[&::aws_smithy_schema::Schema] = &[
    crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA,
    crate::ops::types::UNTAG_RESOURCE_INPUT_SCHEMA,
];

/// Whether `member_name` names an input member this model binds a LIST to on the
/// query string, read off the generated input schemas in
/// [`QUERY_BOUND_LIST_INPUTS`].
fn is_query_bound_list_member(member_name: &str) -> bool {
    QUERY_BOUND_LIST_INPUTS.iter().any(|input| {
        input.members().iter().any(|member| {
            member.http_query().map(|query| query.value()) == Some(member_name)
                && matches!(member.shape_type(), ::aws_smithy_schema::ShapeType::List)
        })
    })
}

/// The two sentences this service states for an input member a request supplied nothing
/// for, re-exported from the ONE home AWS Signer's wording has: this control plane and the
/// data plane (`services/service-com-amazonaws-signerdata-signerdataplane`) are two views
/// of the same API, so a sentence written out in each could come to differ between them.
/// `missing_list_member_constraint` is shared with the handler
/// ([`crate::ops::get_revocation_status`]), so a member the boundary validator finds empty
/// and one a handler finds empty are stated identically; `absent_member_constraint` states
/// an absent single-value member in the `Invalid <member>: …` voice every refusal of this
/// service's own rules speaks ([`crate::state::invalid_arn_message`] states a value that is
/// not ARN-shaped in it).
pub use service_common::signer_input_refusals::{
    absent_member_constraint, missing_list_member_constraint,
};

/// How this service states a refusal whose every violation is a query-bound LIST
/// member the request supplied nothing for: one clause per member, in the order
/// the violations arrive (the model's member order), each naming the member.
///
/// `None` for every other refusal, which is then offered to
/// [`service_stated_constraint_refusal`] and, failing that, keeps the
/// framework's own wording: a single-value member that is absent really was
/// null.
fn missing_list_member_refusal(violations: &[Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    let mut clauses = Vec::with_capacity(violations.len());
    for violation in violations {
        if violation.kind != ViolationKind::MissingRequiredMember
            || !is_query_bound_list_member(&violation.path)
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

/// How this service states a refusal of ONE required single-value member the request
/// supplied nothing for.
///
/// `None` for anything else — a member whose value was PRESENT and outside a bound (which
/// the clauses above state), a LIST member ([`missing_list_member_refusal`]), and a request
/// that left SEVERAL members out: this service's sentences each state ONE rule, so a
/// refusal carrying more than one violation keeps the framework's enumerating envelope
/// rather than reading as though only one rule applied.
fn absent_member_refusal(violations: &[Violation]) -> Option<Refused> {
    let [violation] = violations else {
        return None;
    };
    if violation.kind != ViolationKind::MissingRequiredMember
        || is_query_bound_list_member(&violation.path)
    {
        return None;
    }
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message: absent_member_constraint(&violation.path),
    })
}

/// How Signer states a signing-profile NAME outside the character class the
/// modelled name shape admits.
///
/// The whole sentence is fixed: it carries nothing from the request — not the
/// rejected name, and not the anchored expression the walk matched against —
/// because what a caller can act on is the RULE a name has to meet, and the
/// value it just sent is already in its own hands.
const PROFILE_NAME_CHARACTER_CLASS: &str =
    "Profile name must contain only alphanumeric characters and underscore.";

/// How Signer states a blob whose size falls outside the window its modelled
/// `@length` declares: what the member is, then the admissible RANGE in bytes,
/// built from `bounds`.
///
/// The subject is the MEMBER's own name rather than a word per member
/// ([`stated_subject`]), and both bounds are stated — the floor as much as the
/// ceiling, which is what tells a caller an empty value is refused too. The size
/// the request actually carried is not stated, because the window is the
/// requirement and the measurement is not. `None` when the model declares only
/// one of the two, since half a range is not a range: such a refusal keeps the
/// framework's wording.
fn blob_size_requirements(path: &str, bounds: LengthTrait) -> Option<String> {
    let (min, max) = (bounds.min?, bounds.max?);
    let subject = stated_subject(path)?;
    Some(format!(
        "{subject} does not meet size requirements: [{min}B - {max}B]"
    ))
}

/// The member at `path` as a sentence names it: its own name with the first
/// character upper-cased, the member path being lowerCamel. `None` for a path
/// that names no single member (a nested or collection path), which has no
/// subject to state.
fn stated_subject(path: &str) -> Option<String> {
    if path.contains('.') {
        return None;
    }
    let mut characters = path.chars();
    let first = characters.next()?;
    Some(first.to_uppercase().chain(characters).collect())
}

/// The `@pattern` a signing-profile name is admitted by, as this model declares
/// it — or `None` when it declares none.
///
/// Read off the generated input schema of the operation that CREATES a profile,
/// whose one `@httpLabel` member IS that name (its route is
/// `/signing-profiles/{profileName}`), so the expression here is the one the
/// validating walk enforces and no member name has to be written out. Every other
/// operation reaches the same name through the same modelled shape, and so
/// declares the same expression — which is why matching on the EXPRESSION covers
/// every one of them without naming an operation either.
///
/// EXACTLY one label is required: a route binding a second one would make "the
/// label" ambiguous, and nothing is reworded rather than the wrong bound being
/// reworded.
fn profile_name_pattern() -> Option<&'static str> {
    let mut labels = crate::ops::types::PUT_SIGNING_PROFILE_INPUT_SCHEMA
        .members()
        .iter()
        .filter(|member| member.http_label().is_some());
    let member = labels.next()?;
    if labels.next().is_some() {
        return None;
    }
    let declared = member.traits()?.get(&PatternTrait::TRAIT_ID)?;
    Some(declared.as_any().downcast_ref::<PatternTrait>()?.regex)
}

/// The `@length` bounds this model declares for the input member whose violation
/// path is `path`, where that member is a BLOB — the byte window a payload the
/// service is handed has to fit.
///
/// Resolved by member path across every operation input ([`OPERATION_INPUTS`]),
/// because `override_refusal` is handed the violations and not the operation they
/// came from (`Refusal::Invalid` carries no schema). A path two inputs declare as
/// a blob with DIFFERENT bounds is declined rather than guessed at, so no refusal
/// can state a window the operation being called does not enforce.
fn declared_blob_length(path: &str) -> Option<LengthTrait> {
    let mut resolved: Option<LengthTrait> = None;
    for (_, input) in OPERATION_INPUTS {
        for member in input.members() {
            if !matches!(member.shape_type(), ::aws_smithy_schema::ShapeType::Blob) {
                continue;
            }
            if member.member_name().map(member_path).as_deref() != Some(path) {
                continue;
            }
            let Some(declared) = member
                .traits()
                .and_then(|traits| traits.get(&LengthTrait::TRAIT_ID))
                .and_then(|found| found.as_any().downcast_ref::<LengthTrait>())
            else {
                continue;
            };
            match resolved {
                Some(first) if first != *declared => return None,
                _ => resolved = Some(*declared),
            }
        }
    }
    resolved
}

/// A member's name as a violation's path spells it — the one spelling the
/// validating walk writes, so a lookup here cannot drift from what it reported.
fn member_path(member_name: &str) -> String {
    ::aws_smithy_schema_server::constraints::path::member(member_name)
}

/// How this service words a refusal of ONE value the model bounds, for the two
/// bounds AWS Signer publishes a rule of its own for: the character class a
/// signing-profile name is admitted by, and the byte window a blob has to fit.
///
/// Both are stated from the MODEL — the expression a name is matched against and
/// the bounds a blob declares — and neither echoes the request, which is what the
/// framework's generic clause does instead.
///
/// `None` for every other refusal, and for a refusal carrying more than one
/// violation: Signer's sentences each state ONE rule, so a request that broke
/// several keeps the framework's enumerating envelope rather than being reported
/// as though only one rule applied.
///
/// A size refusal also has to be about the member the window was read off: the
/// bound the walk REPORTED is checked against the one that member declares, so a
/// path resolving to anything else states nothing rather than a rule the refused
/// value was never judged by.
fn service_stated_constraint_refusal(violations: &[Violation]) -> Option<Refused> {
    let [violation] = violations else {
        return None;
    };
    let message = match &violation.kind {
        ViolationKind::Pattern(violated) if profile_name_pattern() == Some(violated.as_str()) => {
            PROFILE_NAME_CHARACTER_CLASS.to_string()
        }
        ViolationKind::MinLength(reported) => {
            let bounds = declared_blob_length(&violation.path)?;
            if bounds.min != Some(*reported) {
                return None;
            }
            blob_size_requirements(&violation.path, bounds)?
        }
        ViolationKind::MaxLength(reported) => {
            let bounds = declared_blob_length(&violation.path)?;
            if bounds.max != Some(*reported) {
                return None;
            }
            blob_size_requirements(&violation.path, bounds)?
        }
        _ => return None,
    };
    Some(Refused {
        code: VALIDATION_EXCEPTION_CODE.to_string(),
        message,
    })
}

/// The `com-amazonaws-signer-wallabyservice` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
    /// EVERY operation of this service, each the generated table's own operation wrapped
    /// in the layers this file adds in front of it, and built from the SAME shared engine
    /// so a wrapped operation reads and writes the one store. Keyed by the routing key the
    /// protocol resolves — the key the generated table uses.
    ///
    /// Three compositions, one per class of operation, all of them in this ONE table so
    /// `override_operation` has one place to look and no operation can fall through the
    /// gaps between several:
    ///
    /// - every operation is wrapped in [`QuotedRefusals`], so a refusal's clause quotes
    ///   the value the request carried rather than the framework's prefix of it;
    /// - the operations whose modelled output carries no members are wrapped in
    ///   [`NoContentAnswer`] as well, so their successful answer carries no document and
    ///   states its length;
    /// - the operations that bind a timestamp to the query string are wrapped in
    ///   [`QueryTimestampForms`] as well, OUTSIDE the restoring layer, so what reads the
    ///   request's values is handed every timestamp in the form the framework's walk
    ///   reads.
    served: OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        // Every operation this model binds, each in the composition its class calls for
        // (see [`Self::served`]). The three lists together are the whole surface
        // `OPERATION_INPUTS` names, so an operation added to the model and left out of
        // them would be served by the generated table alone.
        let mut served = OperationRoutingTable::new();
        serve_no_content_answers!(
            served,
            engine,
            cancel_signing_profile::CancelSigningProfile,
            revoke_signature::RevokeSignature,
            revoke_signing_profile::RevokeSigningProfile,
            tag_resource::TagResource,
            untag_resource::UntagResource,
        );
        serve_query_timestamp_forms!(
            served,
            engine,
            get_revocation_status::GetRevocationStatus,
            list_signing_jobs::ListSigningJobs,
        );
        serve_quoted_refusals!(
            served,
            engine,
            add_profile_permission::AddProfilePermission,
            describe_signing_job::DescribeSigningJob,
            get_signing_platform::GetSigningPlatform,
            get_signing_profile::GetSigningProfile,
            list_profile_permissions::ListProfilePermissions,
            list_signing_platforms::ListSigningPlatforms,
            list_signing_profiles::ListSigningProfiles,
            list_tags_for_resource::ListTagsForResource,
            put_signing_profile::PutSigningProfile,
            remove_profile_permission::RemoveProfilePermission,
            sign_payload::SignPayload,
            start_signing_job::StartSigningJob,
        );
        Self {
            entry: crate::ServiceDefinition::new(engine),
            served,
        }
    }

    /// The operation a request that named NO modelled operation nevertheless ADDRESSED,
    /// when the only thing between it and that operation is a `@required` query
    /// parameter it left out — or `None` when it addressed no route of this service.
    ///
    /// The REST router matches a route only for a request carrying every `@required`
    /// header/query binding the operation declares, because that is how REST services
    /// tell apart operations overloaded on ONE method + URI template. That
    /// discrimination picks BETWEEN operations; it cannot un-name the operation a route
    /// has. So a request that leaves such a parameter out resolves to NO operation, and
    /// the protocol default for that is 501 `NotImplemented` — which says nobody
    /// implemented the operation, when what actually happened is that the request left a
    /// required parameter out.
    ///
    /// Naming the operation here hands such a request to the operation it addressed, so
    /// the centralized walk's own boundary validator refuses it with the modelled 400
    /// naming each absent member.
    ///
    /// Which route a request addressed is answered by the ROUTER rather than by matching
    /// paths here: each candidate is asked of `routing_key` with that candidate's own
    /// query parameters supplied ([`with_query_parameters_supplied`]), and a candidate
    /// counts only when the router then resolves the candidate itself — so the method,
    /// the path labels and the route precedence are all the router's own, and a request
    /// addressing nothing this service routes keeps the framework's answer.
    fn operation_addressed_without_a_required_parameter(
        &self,
        req: &RtRequest,
    ) -> Option<&'static str> {
        OPERATION_INPUTS.iter().find_map(|(operation, input)| {
            let probe = with_query_parameters_supplied(req, input)?;
            let resolved = self.entry.protocol().routing_key(&probe).ok()?;
            (resolved == *operation).then_some(*operation)
        })
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
        // AWS Signer and the Signer data plane are two emulated services behind ONE
        // SigV4 signing name (`signer`), which the model cannot express. The router
        // resolves a request's routing key from the credential scope FIRST and falls
        // back to the `Host` label, so the key names an ENDPOINT: `signer` for this
        // service (whose endpoint prefix IS the shared signing name, and which every
        // signed request to either endpoint therefore resolves to) and `data-signer`
        // for the peer's own endpoint, which only a request addressed at that host
        // without a credential scope resolves to.
        //
        // So this service claims the signing name's traffic WHOLE, its own
        // `GetRevocationStatus` (`GET /revocations`) included: it holds the records
        // that answer it — `RevokeSigningProfile` and `RevokeSignature` write them —
        // and a request signed for `signer` is one this service serves whichever of
        // the two endpoint hosts it was sent to. The peer
        // (`services/service-com-amazonaws-signerdata-signerdataplane`) claims the key
        // of its OWN endpoint and no longer the shared signing name, so the two claims
        // are disjoint by key and no request has the two claimants the router answers
        // `500 AmbiguousService` for.
        match smithy_server_runtime::resolve_route_key(parts) {
            Some(key) => key == self.entry.identity().signing_name(),
            None => false,
        }
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        match self.entry.protocol().routing_key(req) {
            // NOT a re-route: the key is the one the generated table's own protocol
            // resolves, and the operation it names is the one served — through the
            // wrappers this file puts in front of it (see [`Self::served`]). A key no
            // wrapped operation holds resolves to `None` here, so a request this model
            // does not name is served exactly as it is for an unedited service.
            Ok(key) => self.served.get(key),
            // The request named no operation. One class of such request addresses an
            // operation this service serves: see
            // [`EmulatedService::operation_addressed_without_a_required_parameter`].
            Err(_) => {
                let operation = self.operation_addressed_without_a_required_parameter(req)?;
                // Served through the same wrappers: the request this names an operation
                // for leaves a `@required` member out, so the walk refuses it before the
                // handler — and that refusal is one whose clause the restoring layer owns,
                // just as it owns every other refusal of this service.
                self.served
                    .get(operation)
                    .or_else(|| self.entry.routing_table().get(operation))
            }
        }
    }

    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it.
        //
        // ONE class is reworded here, and only for a member the model binds a
        // LIST to the query string: a request that supplied nothing for it. The
        // framework states that as "Value null at '<member>' failed to satisfy
        // constraint: Member must not be null", which asserts the value was NULL
        // — a claim the wire cannot support for a query-bound list, because a
        // list that is ABSENT and one that is EMPTY are the same request (an SDK
        // serializing an empty list emits no parameter at all). This service
        // states instead that such a member must carry something, naming it (see
        // [`missing_list_member_constraint`]).
        //
        // Two further classes are reworded, and for the same reason: AWS Signer
        // publishes a rule of its own for them, so the generic clause — which
        // quotes the caller's value and the raw model expression and states the
        // requirement as a schema fact — is not how this service's refusal reads.
        // A signing-profile name outside the class its shape admits is answered
        // with that class, and a payload outside the byte window its blob
        // declares with that window (see
        // [`service_stated_constraint_refusal`]).
        //
        // A single-value member the request supplied nothing for is reworded too,
        // for the same reason: this service's own validator refuses it in the
        // `Invalid <member>: …` voice rather than as a constraint on a `null`
        // value (see [`absent_member_constraint`]).
        //
        // Every other refusal keeps the framework's own wording — a value outside
        // a bound this service publishes no sentence for, a request that broke
        // several rules at once — which is accurate for those.
        match refusal {
            Refusal::Invalid(violations) => missing_list_member_refusal(violations)
                .or_else(|| absent_member_refusal(violations))
                .or_else(|| service_stated_constraint_refusal(violations)),
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

#[cfg(test)]
mod tests {
    use super::{
        is_query_bound_list_member, missing_list_member_constraint, missing_list_member_refusal,
        with_query_parameters_supplied, RtRequest, Violation, ViolationKind,
        VALIDATION_EXCEPTION_CODE,
    };

    /// One request, by method and target, with no body — enough for a route lookup.
    fn request(method: &str, target: &str) -> RtRequest {
        let mut req = RtRequest::new(::aws_smithy_types::body::SdkBody::empty());
        req.set_uri(target).expect("the target is well formed");
        req.set_method(method).expect("the method is well formed");
        req
    }

    /// The probe supplies exactly the query parameters the request left out, as bare
    /// names, keeps the ones it carried, and is `None` when nothing is left to supply.
    ///
    /// The rows span what the rule generalises over: a route binding ONE query parameter
    /// and a route binding five, the parameter absent, present, present with no value,
    /// and present beside a parameter the model does not bind.
    #[test]
    fn a_probe_supplies_only_the_parameters_the_request_left_out() {
        let removal = crate::ops::types::REMOVE_PROFILE_PERMISSION_INPUT_SCHEMA;
        let revocation = crate::ops::types::GET_REVOCATION_STATUS_INPUT_SCHEMA;
        let cases: &[(&str, &str, &str, &::aws_smithy_schema::Schema, Option<&str>)] = &[
            (
                "the one required parameter absent",
                "DELETE",
                "/signing-profiles/P/permissions/s1",
                removal,
                Some("/signing-profiles/P/permissions/s1?revisionId"),
            ),
            (
                "the one required parameter present",
                "DELETE",
                "/signing-profiles/P/permissions/s1?revisionId=abcdefghij",
                removal,
                None,
            ),
            (
                "the one required parameter present carrying no value",
                "DELETE",
                "/signing-profiles/P/permissions/s1?revisionId=",
                removal,
                None,
            ),
            (
                "another parameter present, the required one absent",
                "DELETE",
                "/signing-profiles/P/permissions/s1?nextToken=x",
                removal,
                Some("/signing-profiles/P/permissions/s1?nextToken=x&revisionId"),
            ),
            (
                "a route binding five parameters, none of them supplied",
                "GET",
                "/revocations",
                revocation,
                Some(
                    "/revocations?signatureTimestamp&platformId&profileVersionArn&jobArn\
                     &certificateHashes",
                ),
            ),
            (
                "a route binding five parameters, one of them supplied",
                "GET",
                "/revocations?platformId=AWSLambda-SHA384-ECDSA",
                revocation,
                Some(
                    "/revocations?platformId=AWSLambda-SHA384-ECDSA&signatureTimestamp\
                     &profileVersionArn&jobArn&certificateHashes",
                ),
            ),
        ];

        for (case, method, target, input, expected) in cases {
            let probe = with_query_parameters_supplied(&request(method, target), input);
            let actual = probe.as_ref().map(|probe| probe.uri().to_string());
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
            if let Some(probe) = probe {
                assert_eq!(
                    probe.method(),
                    *method,
                    "case {case:?}: expected the method carried over, got {:?}",
                    probe.method()
                );
            }
        }
    }

    /// One violation of `kind` against `path`, carrying `value`.
    fn violation(path: &str, kind: ViolationKind, value: Option<&str>) -> Violation {
        Violation {
            path: path.to_string(),
            kind,
            value: value.map(str::to_string),
            sensitive: false,
        }
    }

    /// Which members the rule covers is read off the model: BOTH query-bound list
    /// members are covered, and no single-value member is — whatever its own
    /// shape or operation.
    #[test]
    fn only_query_bound_list_members_are_covered() {
        // Covered: the two members the model binds a list to on the query string.
        // Not covered: a query-bound string, a query-bound timestamp, a path
        // label, and a member no operation has.
        let cases: &[(&str, bool)] = &[
            ("certificateHashes", true),
            ("tagKeys", true),
            ("platformId", false),
            ("signatureTimestamp", false),
            ("profileVersionArn", false),
            ("resourceArn", false),
            ("tags", false),
            ("notAMember", false),
        ];
        for (member, covered) in cases {
            assert_eq!(
                is_query_bound_list_member(member),
                *covered,
                "case {member:?}: expected covered={covered}, got {}",
                is_query_bound_list_member(member)
            );
        }
    }

    /// A refusal whose every violation is a query-bound list member the request
    /// supplied nothing for is stated this service's way, naming each member.
    #[test]
    fn a_supplied_nothing_list_member_is_stated_by_name() {
        for member in ["certificateHashes", "tagKeys"] {
            let refused = missing_list_member_refusal(&[violation(
                member,
                ViolationKind::MissingRequiredMember,
                None,
            )])
            .unwrap_or_else(|| panic!("case {member:?}: expected a reworded refusal, got None"));
            assert_eq!(
                refused.message,
                missing_list_member_constraint(member),
                "case {member:?}: expected the member named, got {:?}",
                refused.message
            );
            assert_eq!(
                refused.code, VALIDATION_EXCEPTION_CODE,
                "case {member:?}: expected the code left where it was, got {:?}",
                refused.code
            );
        }
    }

    /// Two such members are stated in one message, in the order the violations
    /// arrive.
    #[test]
    fn two_supplied_nothing_list_members_are_both_stated() {
        let refused = missing_list_member_refusal(&[
            violation("certificateHashes", ViolationKind::MissingRequiredMember, None),
            violation("tagKeys", ViolationKind::MissingRequiredMember, None),
        ])
        .expect("case \"two list members\": expected a reworded refusal, got None");
        assert_eq!(
            refused.message, "certificateHashes must not be null or empty; tagKeys must not be null or empty",
            "case \"two list members\": expected both members named in order, got {:?}",
            refused.message
        );
    }

    /// Every other refusal keeps the framework's own wording: a single-value
    /// member that really was absent, a value present but outside a declared
    /// bound, a mixture of the two, and the empty violation list.
    #[test]
    fn every_other_refusal_keeps_the_framework_wording() {
        let cases: &[(&str, Vec<Violation>)] = &[
            (
                "an absent single-value member",
                vec![violation(
                    "platformId",
                    ViolationKind::MissingRequiredMember,
                    None,
                )],
            ),
            (
                "a list member present but too short",
                vec![violation("tagKeys", ViolationKind::MinLength(1), Some("[]"))],
            ),
            (
                "a value outside a declared bound",
                vec![violation(
                    "jobArn",
                    ViolationKind::MinLength(20),
                    Some("arn:aws:signer"),
                )],
            ),
            (
                "an absent list member beside an absent single-value member",
                vec![
                    violation("certificateHashes", ViolationKind::MissingRequiredMember, None),
                    violation("jobArn", ViolationKind::MissingRequiredMember, None),
                ],
            ),
            ("no violations at all", Vec::new()),
        ];
        for (case, violations) in cases {
            let refused = missing_list_member_refusal(violations);
            assert!(
                refused.is_none(),
                "case {case:?}: expected the framework's wording kept, got {refused:?}"
            );
        }
    }
}
