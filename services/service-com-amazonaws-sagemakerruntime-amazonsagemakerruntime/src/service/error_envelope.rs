// SPDX-License-Identifier: CC0-1.0

//! How this data plane states a failure on the wire: the error SHAPE it names, the
//! envelope a request refused before any handler ran is stated in, and WHEN the request's
//! payload is examined.
//!
//! All three are decided here rather than per operation, because they are the same for
//! every route this service serves.
//!
//! # The shape a failure names
//!
//! A restJson1 client resolves the error shape from the `x-amzn-errortype` response
//! header. This service names it there as the modelled shape followed by the separator
//! and the service stack's own locator for that shape ([`ERROR_TYPE_LOCATOR`]) — the form
//! every recorded failure of this data plane carries. Every SDK reads the shape from the
//! text BEFORE the separator, so the locator changes nothing a client resolves; the body
//! keeps naming the bare shape in `__type`. [`with_error_type_locator`] applies it to a
//! response on its way out, so no renderer and no handler has to know about the header.
//!
//! # The envelope a pre-handler refusal is stated in
//!
//! `ValidationError` (`@httpError 400`, one member `Message`) is the ONLY client-input
//! error this service models, and it is what the real service answers a member outside a
//! modelled constraint with: the AWS validation envelope in that shape's own `Message`
//! member. The framework's pre-dispatch renderer gets the status right but names a
//! `ValidationException` shape this model does not declare and writes the prose into a
//! framework-chosen lowercase `message` key, so the body carries no `Message` at all and
//! an SDK surfaces a constraint refusal with no reason and no resolvable shape.
//!
//! [`InModelledEnvelope`] closes that: each modelled operation is served wrapped in it,
//! and a request the wrapper finds refusable is answered through the modelled
//! `ValidationError` instead. The violations come from the framework's OWN deserializer
//! and constraint walk, so the wrapper cannot refuse a different set of requests than the
//! walk that follows it, and the envelope, the clauses and the withholding of a
//! `@sensitive` member's value are `service-common`'s shared wording. Two choices are this
//! service's: which expression a `@pattern` clause quotes for the member it refuses
//! ([`quoted_pattern`]) and which members are withheld ([`WITHHELD_MEMBERS`], read off this
//! service's own model).
//!
//! # When the payload is examined
//!
//! An invocation is ADDRESSED to an endpoint by the `@httpLabel` in its URI, and the
//! service resolves that endpoint before it looks at the payload it streams to the model
//! container: a request naming an endpoint the account does not host is refused for that
//! whatever that payload is. The framework's boundary walk cannot express that order — it
//! refuses every constraint violation before any handler runs — so a request whose payload
//! ALONE is outside its modelled bound is handed to the handler, and the handler states
//! where in its own order the payload belongs ([`payload_over_maximum`], called from each
//! route). The two routes whose `Body` the model declares `@required` call it once their
//! addressing has resolved; the asynchronous route, whose `Body` is one of two mutually
//! exclusive input sources the request layer chooses between, calls it with that choice,
//! before the endpoint resolves. Every other refusal keeps the boundary's order.

use std::sync::LazyLock;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::serde::{SerdeError, SerializableStruct};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::{self, LengthTrait};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, DeserializableStruct, ErasedOperation, Operation, OperationContext, OperationError,
    Refusal, Refused, ServerHttpProtocol, Service, Violation, ViolationKind,
};

/// This service's Smithy model: the one source for which of its members are `@sensitive`,
/// and so for which values a refusal states none.
const MODEL_JSON: &str =
    include_str!("../../smithy-model/com-amazonaws-sagemakerruntime-amazonsagemakerruntime.json");

/// The members of this model whose value a refusal WITHHOLDS, read off [`MODEL_JSON`]
/// rather than listed here.
///
/// This model marks what a caller sends to the model container: the request payload
/// (`Body`, on all three routes) and the opaque `CustomAttributes` it forwards verbatim.
/// A clause about one of those names the member and the requirement and states no value,
/// because a refusal that echoed it would disclose it into every log the response passes
/// through.
static WITHHELD_MEMBERS: LazyLock<::service_common::sensitive_members::WithheldMembers> =
    LazyLock::new(|| ::service_common::sensitive_members::WithheldMembers::from_model(MODEL_JSON));

/// The header a restJson1 client resolves the error shape from.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// What this service appends to the shape name in [`ERROR_TYPE_HEADER`]: the separator,
/// then the service stack's own locator for the shape.
///
/// The locator is not part of the shape's NAME — every SDK resolves the shape from the
/// text before the separator (and after a `#`, for a namespaced name) — but it is what
/// this service's wire carries, and a client that compares the whole header value sees it.
const ERROR_TYPE_LOCATOR: &str = ":http://internal.amazon.com/coral/com.amazonaws.sagemaker.runtime/";

/// The one client-input error this service models: `ValidationError`, which every
/// operation of this service declares (`smithy_query(query="shape",
/// shape="ValidationError")` — `errorKind` client, `httpError` 400, one member `Message`).
pub const VALIDATION_ERROR: &str = "ValidationError";

/// The status that shape's `@httpError` declares.
const VALIDATION_ERROR_STATUS: u16 = 400;

/// The server-fault error this service states a response it could not render with, and the
/// status the model gives it (`InternalFailure`, `@httpError 500`).
const INTERNAL_FAILURE: &str = "InternalFailure";
const INTERNAL_FAILURE_STATUS: u16 = 500;

/// The lowest HTTP status that names an error shape at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// `resp` as this service answers it: a failure names its error shape in
/// [`ERROR_TYPE_HEADER`] with this service's locator appended.
///
/// A success is returned untouched, as is a failure that names no shape (nothing to
/// qualify). IDEMPOTENT: the shape is read back out of whatever the header carries, so a
/// value that already ends in the locator is rewritten to itself. A header value the HTTP
/// layer will not carry is left as it was rather than turned into a transport failure,
/// which on an error path would replace a modelled 4xx with a 500 an SDK retries.
pub fn with_error_type_locator(mut resp: RtResponse) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    let Some(named) = resp.headers().get(ERROR_TYPE_HEADER) else {
        return resp;
    };
    let shape = shape_named(named);
    if shape.is_empty() {
        return resp;
    }
    let qualified = format!("{shape}{ERROR_TYPE_LOCATOR}");
    let _ = resp.headers_mut().try_insert(ERROR_TYPE_HEADER, qualified);
    resp
}

/// The bare error SHAPE a discriminator value names: a value may arrive namespaced
/// (`com.amazonaws.sagemakerruntime#ValidationError`) or with a locator already appended,
/// and neither is part of the shape's name.
fn shape_named(raw: &str) -> &str {
    let raw = raw.trim();
    let after_namespace = raw.rsplit('#').next().unwrap_or(raw);
    after_namespace
        .split(':')
        .next()
        .unwrap_or(after_namespace)
        .trim()
}

/// This service's wording for a refusal the FRAMEWORK still renders — one raised before
/// any operation was named, and the fallback below — so it cannot disagree with the
/// envelope [`InModelledEnvelope`] states: a constraint violation is this service's
/// `ValidationError` either way.
///
/// An authentication refusal keeps the framework's wording and status: it is not an
/// input-constraint failure.
pub fn stated_refusal(refusal: &Refusal) -> Option<Refused> {
    match refusal {
        Refusal::Invalid(violations) => Some(Refused {
            code: VALIDATION_ERROR.to_owned(),
            message: constraint_envelope(violations)?,
        }),
        _ => None,
    }
}

/// The expression the SageMaker Runtime family validates a resource NAME against — an
/// endpoint name, a production variant, a container hostname.
///
/// Anchored at the START only, which is how the family's own models declare it: the HTTP2
/// endpoint runtime binds the same endpoint-name label and the same target-variant header
/// and declares exactly this expression for both
/// (`smithy_query(query="shape", shape="InvokeEndpointWithBidirectionalStreamInput",
/// model_path="services/service-com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2/smithy-model/com-amazonaws-sagemakerruntimehttp2-amazonsagemakerruntimehttp2.json")`,
/// whose OTHER patterned members do carry an end anchor), while this service's model
/// exports the same three members with an end anchor appended. A clause quotes the
/// expression declared for the member it refuses, so for those three it quotes this form;
/// the CHECK itself always used the expression this model declares.
const RESOURCE_NAME_PATTERN: &str = "^[a-zA-Z0-9](-*[a-zA-Z0-9])*";

/// The members whose `@pattern` a clause quotes as [`RESOURCE_NAME_PATTERN`]: the endpoint
/// a request addresses, the production variant it targets and the container hostname it
/// targets, on every route that binds them.
///
/// The family declares the endpoint name and the production variant itself, which is the
/// evidence for the form. The container hostname is the family's third binding of the same
/// expression and the same kind of name, and the sibling model binds no member for it, so
/// nothing states its declaration either way: it is carried with the other two, which is
/// what this service answered for it before the choice became member-keyed, and it is the
/// one entry here to re-key the moment a refusal of that member is recorded.
///
/// The choice is keyed on the MEMBER and never on the expression's text, because this
/// model's export writes other members' declarations with the same characters —
/// `SessionIdHeader` is exported as `^[a-zA-Z0-9](-*[a-zA-Z0-9])*$`, and that member's
/// refusals quote it WITH its end anchor — and only the member says which declaration a
/// clause is quoting.
const RESOURCE_NAME_MEMBERS: [&str; 3] =
    ["endpointName", "targetVariant", "targetContainerHostname"];

/// The expression a clause quotes for the member at `path`, which broke the `@pattern`
/// this model exports as `exported`: that expression VERBATIM, anchors included, except at
/// a [`RESOURCE_NAME_MEMBERS`] member, where it is the family's own declaration of it
/// ([`RESOURCE_NAME_PATTERN`]).
fn quoted_pattern(path: &str, exported: &str) -> String {
    let as_the_family_declares_it = exported.strip_suffix('$').unwrap_or(exported);
    if RESOURCE_NAME_MEMBERS.contains(&path) && as_the_family_declares_it == RESOURCE_NAME_PATTERN {
        return as_the_family_declares_it.to_owned();
    }
    exported.to_owned()
}

/// `violation` with its `@pattern` requirement quoted for the member it refuses
/// ([`quoted_pattern`]), and everything else about it — its member path, its value, its
/// `@sensitive` half — exactly as the framework's own walk reported it.
///
/// The requoting happens HERE, before the shared wording states the clause, because the
/// shared entry point is told the expression alone and this service's choice needs the
/// member too.
fn quoted_for_its_member(violation: &Violation) -> Violation {
    let mut quoted = violation.clone();
    if let ViolationKind::Pattern(exported) = &violation.kind {
        quoted.kind = ViolationKind::Pattern(quoted_pattern(&violation.path, exported));
    }
    quoted
}

/// The `Message` of a constraint refusal: the AWS validation envelope
/// (`N validation error(s) detected: `) over one clause per violation, each naming the
/// rejected value, the member path in lowerCamelCase and the constraint it failed.
/// `None` when there is nothing to state.
///
/// The envelope, the clauses and the dropping of a `@sensitive` member's value half are
/// `service-common`'s shared wording for every service on a JSON protocol, so none of them
/// can drift per crate. This service chooses only the two things that are its own: which
/// expression a `@pattern` clause quotes for the member it refuses ([`quoted_pattern`],
/// applied by [`quoted_for_its_member`]) and which members are withheld
/// ([`WITHHELD_MEMBERS`], read off this service's model).
pub fn constraint_envelope(violations: &[Violation]) -> Option<String> {
    let quoted: Vec<Violation> = violations.iter().map(quoted_for_its_member).collect();
    let stated = ::service_common::validation_message::aws_validation_refusal_withholding_with(
        &Refusal::Invalid(quoted),
        // Already quoted for the member above: the shared wording states it as it stands.
        |pattern| pattern.to_owned(),
        |path| WITHHELD_MEMBERS.withholds_value(path),
    )?;
    // The CODE that shared wording travels with is the JSON default, which this model does
    // not declare: every caller here pairs the message with this service's own shape.
    Some(stated.message)
}

/// The member a request's PAYLOAD binds to, when the operation's input declares one
/// (`@httpPayload`).
fn payload_member(input_schema: &Schema) -> Option<&Schema> {
    input_schema
        .members()
        .iter()
        .copied()
        .find(|member| member.http_payload().is_some())
}

/// The violation path the payload member is reported at: its own name in lowerCamel, built
/// by the framework's own path speller so this cannot drift from what the walk reports.
fn payload_path(input_schema: &Schema) -> Option<String> {
    payload_member(input_schema)?
        .member_name()
        .map(constraints::path::member)
}

/// The maximum the MODEL declares for the operation's payload, read off that member's own
/// `@length` trait rather than restated here.
fn payload_maximum(input_schema: &Schema) -> Option<i64> {
    let traits = payload_member(input_schema)?.traits()?;
    traits
        .get(&LengthTrait::TRAIT_ID)?
        .as_any()
        .downcast_ref::<LengthTrait>()?
        .max
}

/// The refusal a request whose payload is past the maximum the model declares for it is
/// owed, or `None` when the payload is inside that maximum (or the operation declares no
/// payload member and no maximum).
///
/// A handler calls this where the payload belongs in ITS order — after the addressing has
/// resolved for the payload the service streams to the model container, and with the
/// input-source choice for a payload that is one of several sources: the clause and the
/// bound are still the framework's and the model's, so a payload refused here reads exactly
/// as one refused at the boundary would.
pub fn payload_over_maximum(input_schema: &Schema, bytes: usize) -> Option<String> {
    let maximum = payload_maximum(input_schema)?;
    if i64::try_from(bytes).is_ok_and(|bytes| bytes <= maximum) {
        return None;
    }
    constraint_envelope(&[Violation {
        path: payload_path(input_schema)?,
        kind: ViolationKind::MaxLength(maximum),
        value: Some(format!("{bytes} bytes")),
        sensitive: false,
    }])
}

/// What reading `req` against a modelled input shape says about it.
enum Read<I> {
    /// Nothing the model declares is broken.
    Valid(I),
    /// The ONLY broken constraint is on the request's payload. This service examines a
    /// payload after the endpoint the request addressed resolves, so the handler answers,
    /// with the input as the codec read it.
    PayloadOnly(I),
    /// One or more members outside what the model declares.
    Refused(Vec<Violation>),
    /// Not a refusal at all: the request could not be read as this input, which is a
    /// serialization failure and keeps the framework's own wording.
    Unread(String),
}

/// Serves one modelled operation, stating a request refused before its handler ran in this
/// service's own modelled envelope, and naming the error shape the way this service names
/// it.
///
/// Nothing else is re-implemented: a request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error.
pub struct InModelledEnvelope<O> {
    operation: O,
}

impl<O> InModelledEnvelope<O> {
    /// Wrap one operation, already constructed from the shared engine.
    pub fn new(operation: O) -> Self {
        Self { operation }
    }
}

impl<O> ErasedOperation for InModelledEnvelope<O>
where
    O: Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // Read the protocol out before the await: `&dyn ErasedServiceDefinition` is
            // not `Sync` and this future is. The protocol itself is.
            let protocol: &dyn ServerHttpProtocol = svc.definition().protocol();
            let answered = match read_request::<O>(protocol, req) {
                Read::Refused(violations) => match validation_error(protocol, &violations) {
                    Some(refusal) => refusal,
                    // Nothing could be stated, so the framework's own walk refuses the
                    // request in its envelope rather than this answering a 400 with no
                    // reason at all.
                    None => self.operation.run(svc, req, ctx).await,
                },
                Read::PayloadOnly(input) => {
                    answer_of(protocol, &self.operation, input, ctx).await
                }
                // The framework's own sequence serves everything else, so it keeps owning
                // deserialize -> validate -> handle -> serialize and telemetry. It reads
                // the request again, which is the price of leaving that sequence whole:
                // the read above decides only HOW the request is refused.
                Read::Valid(_) | Read::Unread(_) => self.operation.run(svc, req, ctx).await,
            };
            with_error_type_locator(answered)
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// What `O`'s handler answers `input` with, serialized through the service's protocol —
/// the same two renderings the framework's own sequence makes of a handler's `Result`.
///
/// Reached only for a request whose payload alone is outside a modelled bound: the handler
/// resolves the endpoint the request addressed and then examines the payload
/// ([`payload_over_maximum`]), which is the order the real service answers in and the one
/// the framework's boundary walk cannot express.
async fn answer_of<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    operation: &O,
    input: O::Input,
    ctx: &OperationContext<'_>,
) -> RtResponse {
    match operation.handle(input, ctx).await {
        Ok(output) => protocol.serialize_output(
            &output,
            O::OUTPUT_SCHEMA,
            O::NAME,
            O::SUCCESS_STATUS,
        ),
        Err(error) => protocol.serialize_error(
            error.as_serializable(),
            error.variant_schema(),
            error.shape_name(),
            error.http_status(),
        ),
    }
    .unwrap_or_else(|failure| {
        // Serializing a modelled answer cannot fail for a JSON envelope, but an error
        // path must not panic: state the fault rather than drop the request.
        internal_failure(protocol, &failure.to_string())
    })
}

/// A 500 in this service's own envelope, for a response this process could not render.
fn internal_failure(protocol: &dyn ServerHttpProtocol, detail: &str) -> RtResponse {
    let failure = crate::ops::types::InternalFailure {
        message: Some(format!("The response could not be rendered: {detail}")),
    };
    protocol
        .serialize_error(
            &failure,
            &crate::ops::types::SCHEMA_INTERNAL_FAILURE,
            INTERNAL_FAILURE,
            INTERNAL_FAILURE_STATUS,
        )
        .unwrap_or_else(|_| {
            let mut resp = RtResponse::new(
                StatusCode::try_from(INTERNAL_FAILURE_STATUS).expect("500 is a valid status"),
                SdkBody::from(r#"{"__type":"InternalFailure"}"#),
            );
            resp.headers_mut().insert("content-type", "application/json");
            resp
        })
}

/// What reading `req` against `O`'s modelled input finds: the violations that refuse it
/// before its handler runs — a `@required` member absent, or a value outside a declared
/// constraint — and, when only its payload is outside one, the input itself.
///
/// [`Read::Valid`] for a request the model accepts, and for a failure that is NOT a
/// refusal: a body the codec cannot read is a serialization failure, which the framework's
/// own walk renders.
///
/// Both refusal classes come from the framework's own machinery (the protocol's
/// deserializer and `constraints::validate` over the built input), so this cannot find a
/// different set of violations from the walk that follows it.
fn read_request<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Read<O::Input> {
    read_against::<O::Input>(protocol, req, O::INPUT_SCHEMA)
}

/// [`read_request`] for a route that reads its own request — the hand-authored streaming
/// one, whose output binds a `@streaming` union the generated sequence cannot carry.
///
/// `Err` is the refusal's `Message`, which the caller answers with the modelled client
/// error; `Ok` is the input, including one whose PAYLOAD is outside its modelled bound,
/// because that is examined once the endpoint the request addressed resolves
/// ([`payload_over_maximum`]) — the same order the generated routes answer in.
pub fn read_addressed_request<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    schema: &'static Schema,
) -> Result<T, String>
where
    T: DeserializableStruct + SerializableStruct,
{
    match read_against::<T>(protocol, req, schema) {
        Read::Valid(input) | Read::PayloadOnly(input) => Ok(input),
        Read::Refused(violations) => {
            Err(constraint_envelope(&violations).unwrap_or_else(|| UNREADABLE.to_string()))
        }
        Read::Unread(detail) => Err(detail),
    }
}

/// The core of both: read `req` into `T` against `schema`, then walk `schema`'s
/// constraints over what was read.
fn read_against<T>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
    schema: &'static Schema,
) -> Read<T>
where
    T: DeserializableStruct + SerializableStruct,
{
    let mut deserializer = match protocol.deserialize_input(req, schema) {
        Ok(deserializer) => deserializer,
        Err(error) => return Read::Unread(format!("{UNREADABLE}: {error}")),
    };
    let input = match <T as DeserializableStruct>::deserialize(&mut *deserializer) {
        Ok(input) => input,
        // Absence is found while READING the request and a bad value while walking what
        // was read; to a caller they are one refusal, so they are one here.
        Err(SerdeError::MissingMember { member_name }) => {
            return Read::Refused(vec![Violation {
                path: member_name,
                kind: ViolationKind::MissingRequiredMember,
                value: None,
                sensitive: false,
            }])
        }
        Err(error) => return Read::Unread(format!("{UNREADABLE}: {error}")),
    };
    drop(deserializer);
    let violations = constraints::validate(&input, schema);
    if violations.is_empty() {
        return Read::Valid(input);
    }
    let payload = payload_path(schema);
    if payload.is_some_and(|payload| violations.iter().all(|violation| violation.path == payload)) {
        return Read::PayloadOnly(input);
    }
    Read::Refused(violations)
}

/// How a request this service could not read at all is stated — the wording the shared
/// event-stream reader uses, so the hand-authored route answers as it did.
const UNREADABLE: &str = "the request could not be read";

/// This service's answer to `violations`: HTTP 400 `ValidationError` with the prose in the
/// `Message` member the model declares.
///
/// `None` when there is nothing to refuse — and also when the error cannot be serialized,
/// which leaves the framework's own walk to refuse the request in its envelope rather than
/// answering a 400 with no reason at all.
fn validation_error(
    protocol: &dyn ServerHttpProtocol,
    violations: &[Violation],
) -> Option<RtResponse> {
    let refused = crate::ops::types::ValidationError {
        message: Some(constraint_envelope(violations)?),
    };
    protocol
        .serialize_error(
            &refused,
            &crate::ops::types::SCHEMA_VALIDATION_ERROR,
            VALIDATION_ERROR,
            VALIDATION_ERROR_STATUS,
        )
        .ok()
}

#[cfg(test)]
mod tests {
    //! The header rule over the forms a response can name its shape in. That every
    //! refusal of every route carries it, and that a pre-handler refusal carries the
    //! modelled `Message`, is asserted over the wire in `tests/all/refusal_envelope.rs`.

    use super::*;
    use aws_smithy_runtime_api::http::StatusCode;
    use aws_smithy_types::body::SdkBody;

    /// One response, built from a status and the shape its renderer named.
    fn response(status: u16, named: Option<&str>) -> RtResponse {
        let mut resp = RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from("{}"),
        );
        if let Some(named) = named {
            resp.headers_mut().insert(ERROR_TYPE_HEADER, named.to_string());
        }
        resp
    }

    /// Every shape a renderer can name gets its own name qualified, so the header is not
    /// keyed to one recorded refusal.
    #[test]
    fn the_header_qualifies_whatever_shape_the_renderer_named() {
        for shape in [
            "ValidationError",
            "ModelError",
            "ModelNotReadyException",
            "InternalDependencyException",
            "InternalFailure",
            "ServiceUnavailable",
        ] {
            let answered = with_error_type_locator(response(400, Some(shape)));
            let expected = format!("{shape}{ERROR_TYPE_LOCATOR}");
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(expected.as_str()),
                "case {shape:?}: expected the qualified shape, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// The shape is read out of every form a renderer can name it in, and a value that
    /// already carries the locator is not doubled.
    #[test]
    fn the_shape_is_read_out_of_every_form_it_is_named_in() {
        let qualified = format!("ValidationError{ERROR_TYPE_LOCATOR}");
        for (case, named) in [
            ("bare", "ValidationError"),
            (
                "namespaced",
                "com.amazonaws.sagemakerruntime#ValidationError",
            ),
            ("already qualified", qualified.as_str()),
        ] {
            let answered = with_error_type_locator(response(400, Some(named)));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                Some(qualified.as_str()),
                "case {case:?}: expected one shape and one locator, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// Nothing to qualify: a success, and a failure whose renderer named no shape, are
    /// both answered exactly as they were.
    #[test]
    fn a_response_that_names_no_shape_is_left_as_it_was() {
        for (case, status, named) in [
            ("a success", 200, Some("ValidationError")),
            ("a 202 submission", 202, None),
            ("no shape named", 400, None),
            ("an empty shape", 400, Some("")),
        ] {
            let answered = with_error_type_locator(response(status, named));
            assert_eq!(
                answered.headers().get(ERROR_TYPE_HEADER),
                named,
                "case {case:?}: expected the header untouched, got {:?}",
                answered.headers().get(ERROR_TYPE_HEADER)
            );
        }
    }

    /// A constraint refusal is stated in the AWS envelope, quoting the expression declared
    /// for the MEMBER it refuses: the family's own declaration at a resource-name member,
    /// and this model's export verbatim everywhere else — including at a member this model
    /// exports with the very same characters.
    #[test]
    fn a_pattern_clause_quotes_the_expression_declared_for_its_member() {
        // Each row: the member the violation sits at, the expression this model exports for
        // it, and the one a clause quotes (`smithy_query(query="operation",
        // operation="InvokeEndpoint,InvokeEndpointAsync,InvokeEndpointWithResponseStream")`).
        for (path, exported, quoted) in [
            // `EndpointName`, `TargetVariantHeader` and `TargetContainerHostnameHeader`:
            // the family declares the resource-name expression anchored at the start only.
            (
                "endpointName",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*",
            ),
            (
                "targetVariant",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*",
            ),
            (
                "targetContainerHostname",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*",
            ),
            // `SessionIdHeader`, on the streamed route: the same characters as the
            // resource-name export, declared WITH its end anchor, so quoted with it.
            (
                "sessionId",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
                "^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
            ),
            // `SessionIdOrNewSessionConstantHeader`: an alternation, quoted as declared.
            (
                "sessionId",
                "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
                "^(NEW_SESSION)$|^[a-zA-Z0-9](-*[a-zA-Z0-9])*$",
            ),
            // `InferenceComponentHeader`: a resource name in prose, a different expression
            // in the model, so this model's export is what a clause quotes.
            (
                "inferenceComponentName",
                r"^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$",
                r"^[a-zA-Z0-9]([\-a-zA-Z0-9]*[a-zA-Z0-9])?$",
            ),
            // `InputLocationHeader`.
            (
                "inputLocation",
                "^(https|s3)://([^/]+)/?(.*)$",
                "^(https|s3)://([^/]+)/?(.*)$",
            ),
            // `Header` (`ContentType`, `Accept`).
            ("accept", r"^\p{ASCII}*$", r"^\p{ASCII}*$"),
        ] {
            let stated = constraint_envelope(&[Violation {
                path: path.to_string(),
                kind: ViolationKind::Pattern(exported.to_string()),
                value: Some("submitted".to_string()),
                sensitive: false,
            }])
            .expect("a constraint refusal is stated");
            assert_eq!(
                stated,
                format!(
                    "1 validation error detected: Value 'submitted' at '{path}' failed to \
                     satisfy constraint: Member must satisfy regular expression pattern: {quoted}"
                ),
                "case {path} / {exported:?}: got {stated:?}"
            );
        }
    }

    /// The code travels with that message, so the two refusal paths of this service name
    /// one shape.
    #[test]
    fn the_stated_refusal_names_the_modelled_shape() {
        let stated = stated_refusal(&Refusal::Invalid(vec![Violation {
            path: "endpointName".to_string(),
            kind: ViolationKind::MaxLength(63),
            value: Some("x".repeat(64)),
            sensitive: false,
        }]))
        .expect("a constraint refusal is stated");
        assert_eq!(
            stated.code, VALIDATION_ERROR,
            "case \"a length clause\": expected the modelled shape, got {:?}",
            stated.code
        );
        assert!(
            stated.message.starts_with("1 validation error detected: "),
            "case \"a length clause\": expected the AWS envelope, got {:?}",
            stated.message
        );
        assert!(
            stated_refusal(&Refusal::Unauthenticated).is_none(),
            "case \"unauthenticated\": an authentication refusal is not reworded"
        );
    }
}
