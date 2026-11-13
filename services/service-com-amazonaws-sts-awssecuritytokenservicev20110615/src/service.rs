// SPDX-License-Identifier: CC0-1.0

//! The `com-amazonaws-sts-awssecuritytokenservicev20110615` service: how it claims requests and how it words the
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
//!   request's resolved route key against this service's signing name, which this
//!   service widens to the unsigned `awsQuery` request its anonymous operations are
//!   sent as (see the method). Edit it for anything the model does not express: a
//!   signing name shared by several services, or routing that keys on the
//!   access-key id.
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

pub mod management_events;
pub mod message_text;
pub mod sensitive_members;

use smithy_server_runtime::{
    http, ErasedOperation, ErasedServiceDefinition, Refusal, Refused, Service,
};
use std::cell::RefCell;
use std::sync::Arc;

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_schema::serde::{SerializableStruct, ShapeDeserializer};
use aws_smithy_schema::{Schema, ShapeId};
use aws_smithy_schema_server::{
    ProtocolError, QueryServerProtocol, QueryShapeDeserializer, ServerProtocol,
};

/// The `awsQuery` field naming the operation a request is for.
///
/// The protocol carries it in the form-encoded body. A call signed under SigV4's
/// query-parameter scheme carries it — and every other field of the call — in the query
/// string instead; see [`QuerySignedCalls`].
const ACTION_FIELD: &str = "Action";

/// This service's XML namespace, as the model declares it
/// (`smithy_query(query="service_info")` -> `xmlNamespace.uri`).
///
/// Stated here because [`Service::override_protocol`] constructs the protocol this
/// service serves with, and the namespace is part of the envelope the generated one is
/// built with: the override has to be the SAME protocol, differing only in where it reads
/// a request from.
const XML_NAMESPACE: &str = "https://sts.amazonaws.com/doc/2011-06-15/";

/// This service's `awsQuery` protocol, reading a call from the query string when that is
/// where SigV4 put it.
///
/// SigV4 has two schemes and this protocol was written for one of them. "Authenticating
/// Requests: Using Authorization Header" signs a `POST` whose call is an
/// `application/x-www-form-urlencoded` BODY, which is what the framework's
/// [`QueryServerProtocol`] reads — the `Action` it routes by and every input member.
/// "Authenticating Requests: Using Query Parameters" — the scheme a PRESIGNED URL uses,
/// and the one `aws eks get-token` and aws-iam-authenticator hand over — signs a request
/// whose call is the QUERY STRING, in the same `name=value&…` encoding, with no body at
/// all. The emulator verifies both signatures, so both are calls this service has to
/// serve; before this, a presigned call named no operation and was answered
/// `InvalidAction`.
///
/// So the query string is read as the call when, and only when, the body does not name an
/// action: a body-carried call keeps being read from its body exactly as before, so no
/// request can be routed or deserialized from a query field in preference to the body it
/// was signed over. Both halves move together — the operation and its input members come
/// from the SAME place — so no call is ever served with the parameters it carried dropped.
/// Everything else (the response envelope, the error envelope, the namespace) is the
/// framework's unchanged.
#[derive(Debug)]
struct QuerySignedCalls {
    inner: QueryServerProtocol,
}

impl QuerySignedCalls {
    fn new() -> Self {
        Self {
            inner: QueryServerProtocol::aws_query().with_namespace(XML_NAMESPACE),
        }
    }
}

/// The query string a request carries its call in, when its BODY carries none.
///
/// `None` for a request whose body names an action (that body is the call) and for one
/// whose query string names none either (nothing to read a call from, which stays the
/// protocol's own `InvalidAction`).
fn call_in_query_string(request: &RtRequest) -> Option<&str> {
    let body_names_an_action = request
        .body()
        .bytes()
        .and_then(|body| ::std::str::from_utf8(body).ok())
        .is_some_and(|body| {
            ::service_common::query_parameter(body, ACTION_FIELD).is_some()
        });
    if body_names_an_action {
        return None;
    }
    let (_, query) = request.uri().split_once('?')?;
    ::service_common::query_parameter(query, ACTION_FIELD).map(|_| query)
}

impl ServerProtocol for QuerySignedCalls {
    type Request = RtRequest;
    type Response = RtResponse;

    fn protocol_id(&self) -> &ShapeId {
        self.inner.protocol_id()
    }

    fn routing_key<'a>(&self, request: &'a RtRequest) -> Result<&'a str, ProtocolError> {
        match call_in_query_string(request) {
            Some(query) => ::service_common::query_parameter(query, ACTION_FIELD).ok_or_else(|| {
                // Unreachable: `call_in_query_string` answers `Some` only for a query
                // string this same reader found an action in.
                ProtocolError::UnknownOperation("request is missing the `Action` field".into())
            }),
            None => self.inner.routing_key(request),
        }
    }

    fn deserialize_input<'a>(
        &self,
        request: &'a RtRequest,
        input_schema: &Schema,
    ) -> Result<Box<dyn ShapeDeserializer + 'a>, ProtocolError> {
        match call_in_query_string(request) {
            // The same reader the framework uses over the body, over the query string the
            // call was signed in: one encoding, so one deserializer, and the member bounds
            // and depth limits it enforces are the framework's own. The `X-Amz-*` signing
            // fields beside the members are not member names, so nothing reads them.
            Some(query) => Ok(Box::new(QueryShapeDeserializer::new(
                query.as_bytes(),
                false,
            ))),
            None => self.inner.deserialize_input(request, input_schema),
        }
    }

    fn serialize_output(
        &self,
        output: &dyn SerializableStruct,
        output_schema: &Schema,
        operation_name: &str,
        success_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner
            .serialize_output(output, output_schema, operation_name, success_status)
    }

    fn serialize_error(
        &self,
        error: &dyn SerializableStruct,
        error_schema: &Schema,
        error_code: &str,
        http_status: u16,
    ) -> Result<RtResponse, ProtocolError> {
        self.inner
            .serialize_error(error, error_schema, error_code, http_status)
    }
}

/// The error code AWS answers a request whose signing credential it cannot resolve
/// with: an access key id that was never issued, or one that has been deleted. It is
/// distinct from an authorization denial — the call failed before any principal was
/// found — and is the code an SDK's credential-refresh path keys on.
const UNRESOLVED_CREDENTIAL_CODE: &str = "InvalidClientTokenId";

/// How AWS words that refusal.
///
/// The wording of a credential failure is one rule for every emulated service, so it is
/// read from the module that owns it ([`::service_common::authentication_refusal`]).
const UNRESOLVED_CREDENTIAL_MESSAGE: &str =
    ::service_common::authentication_refusal::UNRESOLVED_CREDENTIAL_MESSAGE;

/// The error code AWS answers a request whose access key id DOES resolve but whose
/// signature does not verify under that credential's secret: the credential was found,
/// so the caller is known; what failed is the signature.
const SIGNATURE_MISMATCH_CODE: &str = "SignatureDoesNotMatch";

/// How the SigV4 signing layer words that refusal — the fixed sentence it produces for
/// this one condition, which names the half of the credential pair that is wrong. Read
/// from the module that owns the wording, as the sentence above is.
const SIGNATURE_MISMATCH_MESSAGE: &str =
    ::service_common::authentication_refusal::SIGNATURE_MISMATCH_MESSAGE;

/// The media type an `awsQuery` request carries its form-encoded body in, which is the
/// only body this service's protocol reads.
const QUERY_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

thread_local! {
    /// The access key id the request this service is serving presented, for the length of
    /// that one request.
    ///
    /// Not state: a request-scoped note, needed because the two seams that decide an
    /// authentication refusal see different halves of it. [`Service::can_handle`] is
    /// handed the request and records the key id here; [`Service::override_refusal`] is
    /// handed only the STRUCTURAL refusal — `Refusal::Unauthenticated`, which says that
    /// authentication refused the caller and nothing about WHICH credential it refused —
    /// and has to choose between the two refusals AWS words differently for it. The
    /// framework calls both within one synchronous stretch of the one dispatch, on the
    /// thread that dispatch is running on, so the value a refusal reads is the one the
    /// claim of that same request recorded.
    static PRESENTED_ACCESS_KEY_ID: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Record the access key id `parts` presented as the one this thread is serving.
fn remember_presented_access_key_id(parts: &http::request::Parts) {
    let access_key_id = ::service_common::request_access_key_id(parts);
    PRESENTED_ACCESS_KEY_ID.with(|presented| {
        *presented.borrow_mut() = Some(access_key_id);
    });
}

/// The access key id recorded by [`remember_presented_access_key_id`] for the request
/// this thread is serving, TAKEN: the note is request-scoped, so reading it is what ends
/// it. A later refusal on this thread that was not preceded by this service's own claim
/// then reads no credential at all rather than the previous request's.
fn presented_access_key_id() -> Option<String> {
    PRESENTED_ACCESS_KEY_ID.with(|presented| presented.borrow_mut().take())
}

/// Whether `parts` addresses this service's own wire form while naming NO service at all.
///
/// Every `awsQuery` operation is a `POST` of an `application/x-www-form-urlencoded` body
/// to the ROOT path, with the operation in the body's `Action` field — the protocol
/// publishes no `smithy.api#http` binding and no `X-Amz-Target`, so that is the whole
/// signature such a request has.
///
/// It is the signature a request has to be claimed by when nothing else in it names a
/// service: this service's `AssumeRoleWithWebIdentity` and `AssumeRoleWithSAML` are
/// modelled anonymous (`auth: []`, `optionalAuth`), so a conforming client sends them
/// UNSIGNED — no SigV4 credential scope to resolve a routing key from — and a client
/// pointed at this emulator's own endpoint (a loopback address, which names no service)
/// leaves the host tier nothing to resolve either. Claiming by the signing name alone
/// therefore serves every signed call and leaves the anonymous ones unclaimed, which the
/// router answers `UnknownService` in a JSON envelope an `awsQuery` client cannot even
/// parse.
///
/// Nothing weaker is claimed: a request that resolves to SOME service's routing key is
/// that service's however it is addressed, and a request on any other path or method, or
/// carrying any other body type, reaches nobody exactly as before.
fn addresses_unattributed_query_request(parts: &http::request::Parts) -> bool {
    if ::emulator_server::resolve_route_key(parts).is_some() {
        // SOMETHING in the request names a service — a SigV4 credential scope, an
        // `X-Amz-Target`, or an endpoint host — and it is not this service's name (the
        // claim's first conjunct asked that already). That request is its own service's,
        // however it is addressed.
        return false;
    }
    let form_encoded = parts
        .headers
        .get(http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| {
            value
                .split(';')
                .next()
                .unwrap_or(value)
                .trim()
                .eq_ignore_ascii_case(QUERY_CONTENT_TYPE)
        });
    parts.method == http::Method::POST
        && parts.uri.path().trim_matches('/').is_empty()
        && form_encoded
}

/// One modelled operation, served so that a refused request is stated the way AWS states
/// it: with the value as SUBMITTED between the quotes.
///
/// The framework hands [`Service::override_refusal`] the violations ALONE, and the walk
/// that found them stores an ELIDED rendering of each value — a bounded prefix followed by
/// a byte count — because the clause interpolates whatever is stored and a caller controls
/// the length. An AWS constraint message quotes the value the caller sent, which is the one
/// place the server reports the offending input back, so the value has to be restored from
/// the request; and the request is reachable only HERE, per operation.
///
/// Nothing else is re-implemented. A request this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error. The violations come from the framework's
/// own machinery (`service_common::validation::refusing_violations`), so this cannot refuse
/// a request the framework's own walk would accept, nor accept one it would refuse, and the
/// WORDING is the same [`stated_violations`] that [`Service::override_refusal`] uses — so
/// the two refusal paths cannot drift. The cost is that a request this wrapper does not
/// refuse is read twice, once here and once in the operation's own sequence, because the
/// framework hands no already-built input to an operation.
struct StatedPerOperation<O> {
    operation: O,
}

impl<O> StatedPerOperation<O> {
    /// Wrap one operation, already constructed from the shared engine.
    fn new(operation: O) -> Self {
        Self { operation }
    }
}

/// How this service words a constraint refusal: the violations the framework's own walk
/// finds over `req`, each value echo restored to the bytes the caller sent, under the error
/// code STS answers a constraint violation with.
///
/// `None` when the request breaks no constraint, and for a failure that is NOT a refusal (a
/// body the codec cannot read at all, a member of the wrong type), which the framework's own
/// walk answers in its own envelope.
fn stated_refusal<O: smithy_server_runtime::Operation>(
    protocol: &dyn smithy_server_runtime::ServerHttpProtocol,
    req: &RtRequest,
) -> Option<(Vec<aws_smithy_schema_server::Violation>, Refused)> {
    let violations = ::service_common::validation::refusing_violations::<O>(protocol, req);
    let stated = stated_violations(&violations)?;
    Some((violations, stated))
}

/// The ONE renderer both of this service's refusal paths go through —
/// [`StatedPerOperation`] for a request a wrapped operation refuses, and
/// [`Service::override_refusal`] for one the centralized walk refuses — so a caller cannot
/// be told the same broken constraint two ways.
///
/// A clause about a member this service does not disclose the value of states no value at
/// all; which members those are is read from this service's model by
/// [`sensitive_members`], and the clause, the envelope and every requirement stay the
/// shared renderer's and the framework's.
fn stated_violations(violations: &[aws_smithy_schema_server::Violation]) -> Option<Refused> {
    if violations.is_empty() {
        return None;
    }
    ::service_common::validation_message::aws_validation_refusal_withholding(
        &Refusal::Invalid(violations.to_vec()),
        sensitive_members::withholds_value,
    )
    .map(|stated| Refused {
        code: crate::scaffolded::VALIDATION_ERROR_CODE.to_string(),
        message: stated.message,
    })
}

impl<O> ErasedOperation for StatedPerOperation<O>
where
    O: smithy_server_runtime::Operation + ErasedOperation,
{
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r smithy_server_runtime::OperationContext<'r>,
    ) -> smithy_server_runtime::BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // The protocol is read, used and dropped inside this statement: neither it nor
            // `&dyn ErasedServiceDefinition` is `Sync`, so holding one across the await
            // below would make this future non-`Send`. It is the protocol this service
            // SERVES with, so the re-read sees the call where the request carries it (see
            // [`QuerySignedCalls`]).
            let refused = match svc.override_protocol() {
                Some(protocol) => stated_refusal::<O>(protocol, req),
                None => stated_refusal::<O>(svc.definition().protocol(), req),
            };
            if let Some((violations, stated)) = refused {
                // Stated through the same seam as everything else, so this path does not
                // depend on which definition the service hands back; the restatement is
                // idempotent, so passing an already-stated document through costs nothing.
                return message_text::with_literal_message_text(
                    svc.definition().refusal_response(
                        ProtocolError::Invalid(violations),
                        Some(stated),
                        req,
                    ),
                );
            }
            // Every failure an operation answers — a modelled error, and a refusal the
            // centralized walk raised while it was being served — states its message with
            // the escaping XML asks of a text node; see [`message_text`]; and names the
            // request it answers in the envelope's `<RequestId>`, which the framework's
            // error renderers do not write (see
            // [`::service_common::query_error_envelope`]). A successful response carries
            // its own id in `ResponseMetadata` already and is returned untouched.
            ::service_common::query_error_envelope::with_request_id(
                message_text::with_literal_message_text(self.operation.run(svc, req, ctx).await),
            )
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not change
    /// which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

/// The `com-amazonaws-sts-awssecuritytokenservicev20110615` emulated service: the generated [`ServiceDefinition`] plus
/// the behavioral seams this file owns.
///
/// Generic over the engine type `Eng` so this file names no concrete engine; the
/// router builds one shared engine and hands it to `new`.
pub struct EmulatedService<Eng: Send + Sync + 'static> {
    /// The generated entry, wrapped so the three failures the router renders OUTSIDE the
    /// dispatch path state their message as a text node too (see [`message_text`]) and name
    /// the request they answer in the `awsQuery` error envelope's `<RequestId>`, as every
    /// other error of this service does (see
    /// [`::service_common::query_error_envelope`]). Identity, the protocol and the routing
    /// table are the generated entry's untouched.
    entry: ::service_common::query_error_envelope::RequestIdEnvelopeDefinition<
        message_text::StatedMessageDefinition<Eng>,
    >,
    /// The SAME shared engine the generated entry threaded into every operation, kept
    /// here too because the refusal wording below has to ask whether the account holds
    /// the credential a refused request presented, and the records that answer that are
    /// the ones the handlers read ([`Self::account_holds_credential`]). Never a store of
    /// its own: it is the `Arc` the router handed to `new`.
    engine: Arc<Eng>,
    /// Held, not built per request: [`Service::override_protocol`] returns a reference to
    /// it. One instance for every request, as the framework's own protocols are
    /// ("Server protocols are stateless").
    protocol: QuerySignedCalls,
    /// Every modelled operation of this service, each served AUDITED — the call is
    /// recorded in the account's event history (see [`management_events`]) — and inside
    /// that, wrapped in [`StatedPerOperation`]; keyed by the routing key the protocol
    /// resolves. It is the table [`Service::override_operation`] serves a request from, so
    /// a refused request is stated with the value it was refused for whichever operation
    /// it named, and no served call goes unrecorded.
    stated: smithy_server_runtime::OperationRoutingTable,
}

impl<Eng: Send + Sync + 'static> EmulatedService<Eng> {
    /// Build this service from the shared engine. The engine is threaded into the
    /// generated [`ServiceDefinition`], which constructs each operation from it; the
    /// operation's behaviour lives in that operation's own `impl Operation`.
    pub fn new(engine: Arc<Eng>) -> Self {
        let stated = management_events::audited_table(&engine);
        Self {
            entry: ::service_common::query_error_envelope::RequestIdEnvelopeDefinition::new(
                message_text::StatedMessageDefinition::new(Arc::clone(&engine)),
            ),
            engine,
            protocol: QuerySignedCalls::new(),
            stated,
        }
    }

    /// Whether `access_key_id` names a credential this emulator ISSUED, and so one an
    /// authentication refusal has RESOLVED: the key pair the process publishes at start,
    /// an IAM user's access key, or a session this service minted.
    ///
    /// That is the whole set of access key ids that resolve to a principal here, each read
    /// from the records that own it rather than from a copy — so a key deleted by
    /// `iam:DeleteAccessKey`, or a session that expired, stops resolving at the moment its
    /// own record goes.
    fn account_holds_credential(&self, access_key_id: &str) -> bool {
        if ::emulator_server::request_authentication::SigV4Authenticator::minted_credentials()
            .is_some_and(|published| published.access_key_id == access_key_id)
        {
            return true;
        }
        if crate::state::iam_state(&*self.engine)
            .is_some_and(|iam| iam.lock().access_keys.contains_key(access_key_id))
        {
            return true;
        }
        crate::state::service_state(&*self.engine)
            .is_some_and(|sts| sts.lock().session(access_key_id).is_some())
    }

    /// How this service words a request authentication refused, which is TWO refusals AWS
    /// words differently:
    ///
    /// * a key id that resolves to no credential at all — never issued, or deleted — is
    ///   `InvalidClientTokenId`, the authentication failure an SDK's credential-refresh
    ///   path keys on; and
    /// * a key id the emulator DOES hold is `SignatureDoesNotMatch`: the credential was
    ///   found and the signature computed under its secret did not match the one
    ///   presented, which points the operator at the secret half of the pair and not at
    ///   the key id.
    ///
    /// Which of the two it is follows from the credential the request presented
    /// ([`Self::account_holds_credential`]), because the framework's structural refusal
    /// does not carry it.
    fn unauthenticated_refusal(&self) -> Refused {
        let resolves = presented_access_key_id()
            .is_some_and(|access_key_id| self.account_holds_credential(&access_key_id));
        if resolves {
            return Refused {
                code: SIGNATURE_MISMATCH_CODE.to_string(),
                message: SIGNATURE_MISMATCH_MESSAGE.to_string(),
            };
        }
        Refused {
            code: UNRESOLVED_CREDENTIAL_CODE.to_string(),
            message: UNRESOLVED_CREDENTIAL_MESSAGE.to_string(),
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
        // Claim a request whose resolved route key is this service's signing name, and —
        // when NOTHING in the request names a service — one that addresses this
        // service's own `awsQuery` wire form, which is what makes the operations the
        // model marks anonymous reachable from a client holding no credential (see
        // [`addresses_unattributed_query_request`]).
        let claimed = ::service_common::routes_to(parts, self.entry.identity().signing_name())
            || addresses_unattributed_query_request(parts);
        if claimed {
            // The credential this request presented, kept for the length of the request
            // so an authentication refusal can be worded for the credential it refused;
            // see [`PRESENTED_ACCESS_KEY_ID`].
            remember_presented_access_key_id(parts);
        }
        claimed
    }

    fn override_operation(
        &self,
        req: &aws_smithy_runtime_api::http::Request,
    ) -> Option<&dyn ErasedOperation> {
        // Every modelled operation is served AUDITED and WRAPPED. This is NOT a re-route:
        // the key is the one this service's own protocol resolves — which reads the call
        // from the query string when that is where SigV4 put it and from the form-encoded
        // BODY otherwise (see [`QuerySignedCalls`]) — and the operation it names is the one
        // served, recorded as a management event of this service and wrapped in
        // [`StatedPerOperation`] so a refused request quotes the value it refused. A key
        // the model does not name resolves to `None` here exactly as it does in the
        // generated table, which leaves the request on the router's own routing-error path
        // and this service's protocol-correct answer.
        let action = self.protocol.routing_key(req).ok()?;
        self.stated.get(action)
    }

    fn override_protocol(&self) -> Option<&dyn smithy_server_runtime::ServerHttpProtocol> {
        // The model's own `awsQuery`, plus SigV4's second scheme; see
        // [`QuerySignedCalls`].
        Some(&self.protocol)
    }


    fn override_refusal(&self, refusal: &Refusal) -> Option<Refused> {
        // The centralized walk calls this on a framework-level refusal (an absent
        // required member, or a value outside a declared bound) to let this
        // service reword it. Returning `None` keeps the framework's default
        // wording.
        match refusal {
            // AWS answers a request authentication refused with ONE of two codes, and
            // which one it is depends on the credential the request presented, not on
            // the structure of the refusal — see [`Self::unauthenticated_refusal`]. The
            // framework collapses every pre-dispatch authentication refusal into one
            // structural case, so this service states the two cases the way AWS states
            // them.
            Refusal::Unauthenticated => Some(self.unauthenticated_refusal()),
            // A refusal of the request's VALUES is stated the way AWS states one: the
            // framework's own envelope and member paths, with the code STS answers a
            // constraint violation with, a `@pattern` quoted back WITHOUT the anchors a
            // whole-value pattern is written with — the tree's one rendering of that rule
            // — and no value at all for a member this service does not disclose. The CHECK
            // still uses the model's pattern as written; only the quoting changes.
            Refusal::Invalid(violations) => stated_violations(violations),
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
    //! The two rules this file owns, each over the signals a request can carry: WHICH
    //! requests this service claims, and HOW it words a request authentication refused.
    //! The served behaviour is pinned over the wire by `tests/all/web_identity_federation.rs`;
    //! these tables are the rules themselves, row by row.

    use super::{
        addresses_unattributed_query_request, remember_presented_access_key_id, EmulatedService,
        QUERY_CONTENT_TYPE, SIGNATURE_MISMATCH_CODE, SIGNATURE_MISMATCH_MESSAGE,
        UNRESOLVED_CREDENTIAL_CODE, UNRESOLVED_CREDENTIAL_MESSAGE,
    };
    use crate::state::{Identity, Session, SessionKind};
    use std::sync::Arc;
    use std::time::{Duration, SystemTime};

    /// This service's SigV4 signing name, as the generated identity states it
    /// (`smithy_query(query="service_info")` -> `sigv4Name` `sts`).
    const SIGNING_NAME: &str = "sts";

    /// A credential scope naming `signing_name`, the primary routing signal.
    fn authorization(access_key_id: &str, signing_name: &str) -> String {
        format!(
            "AWS4-HMAC-SHA256 Credential={access_key_id}/20260101/us-east-1/{signing_name}/aws4_request, \
             SignedHeaders=host, Signature=abc"
        )
    }

    /// The parts of a request addressed as the row describes.
    fn parts(
        method: &str,
        uri: &str,
        scope: Option<(&str, &str)>,
        target: Option<&str>,
        host: Option<&str>,
        content_type: Option<&str>,
    ) -> super::http::request::Parts {
        let mut builder = super::http::Request::builder().method(method).uri(uri);
        if let Some((access_key_id, signing_name)) = scope {
            builder = builder.header("authorization", authorization(access_key_id, signing_name));
        }
        if let Some(target) = target {
            builder = builder.header("x-amz-target", target);
        }
        if let Some(host) = host {
            builder = builder.header("host", host);
        }
        if let Some(content_type) = content_type {
            builder = builder.header("content-type", content_type);
        }
        builder
            .body(())
            .expect("a well-formed test request")
            .into_parts()
            .0
    }

    /// A request naming no service is claimed by the `awsQuery` wire form, and only by it.
    #[test]
    fn an_unattributed_request_is_claimed_only_on_this_protocol_s_wire_form() {
        // One row: how the request is addressed, and whether the wire form claims it.
        let rows: Vec<(&str, super::http::request::Parts, bool)> = vec![
            (
                "unsigned form POST to the root path",
                parts("POST", "/", None, None, Some("127.0.0.1:9297"), Some(QUERY_CONTENT_TYPE)),
                true,
            ),
            (
                "unsigned form POST with a charset parameter",
                parts(
                    "POST",
                    "/",
                    None,
                    None,
                    Some("localhost:9297"),
                    Some("application/x-www-form-urlencoded; charset=utf-8"),
                ),
                true,
            ),
            (
                "unsigned form POST with no host at all",
                parts("POST", "/", None, None, None, Some(QUERY_CONTENT_TYPE)),
                true,
            ),
            (
                // The credential scope names a service, so that service's claim decides.
                "signed for another service",
                parts("POST", "/", Some(("AKIDEXAMPLE", "iam")), None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "signed for this service",
                parts("POST", "/", Some(("AKIDEXAMPLE", SIGNING_NAME)), None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "targeted at an awsJson service",
                parts(
                    "POST",
                    "/",
                    None,
                    Some("AWSCognitoIdentityService.GetId"),
                    None,
                    Some(QUERY_CONTENT_TYPE),
                ),
                false,
            ),
            (
                "addressed by a service's endpoint host",
                parts("POST", "/", None, None, Some("sts.us-east-1.amazonaws.com"), Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned form POST to a path no awsQuery operation uses",
                parts("POST", "/2011-06-15/assume-role", None, None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned GET of the root path",
                parts("GET", "/", None, None, None, Some(QUERY_CONTENT_TYPE)),
                false,
            ),
            (
                "unsigned POST declaring a JSON body",
                parts("POST", "/", None, None, None, Some("application/json")),
                false,
            ),
            (
                "unsigned POST declaring no body type",
                parts("POST", "/", None, None, None, None),
                false,
            ),
        ];

        for (case, parts, expected) in rows {
            let actual = addresses_unattributed_query_request(&parts);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the wire-form claim == {expected}, got {actual}"
            );
        }
    }

    /// The service, over an engine holding the credentials the rows present.
    fn service() -> EmulatedService<::engine::InMemoryEngine> {
        let engine = Arc::new(::engine::InMemoryEngine::new());
        let account = ::engine::emulator_account_id().to_string();
        engine
            .state::<::service_com_amazonaws_iam_awsidentitymanagementv20100508::ComAmazonawsIamAwsidentitymanagementv20100508State>()
            .lock()
            .access_keys
            .insert(
                "AKIAEMULATORUSERKEY1".to_string(),
                ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::AccessKeyRecord {
                    user_key: ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::name_key("alice"),
                    status: "Active".to_string(),
                    create_date: ::aws_smithy_types::DateTime::from_secs(0),
                },
            );
        let now = SystemTime::now();
        engine
            .state::<crate::ComAmazonawsStsAwssecuritytokenservicev20110615State>()
            .lock()
            .sessions
            .insert(
                "ASIAEMULATORSESSION1".to_string(),
                Session {
                    access_key_id: "ASIAEMULATORSESSION1".to_string(),
                    created_at: now,
                    expires_at: now + Duration::from_secs(3600),
                    duration_seconds: 3600,
                    identity: Identity {
                        user_id: "AROATESTROLEID000001:probe".to_string(),
                        account: account.clone(),
                        arn: format!("arn:aws:sts::{account}:assumed-role/demo/probe"),
                    },
                    kind: SessionKind::AssumedRole,
                    source_identity: None,
                    tags: Vec::new(),
                    transitive_tag_keys: Vec::new(),
                    packed_policy_size: None,
                    policies: crate::state::SessionPolicies::default(),
                    external_id: None,
                    role_arn: Some(format!("arn:aws:iam::{account}:role/demo")),
                    federating_user_arn: None,
                    mfa_authenticated: false,
                    federated_claims: Vec::new(),
                    revoked: false,
                },
            );
        EmulatedService::new(engine)
    }

    /// An authentication refusal is worded for the CREDENTIAL the request presented: a key
    /// id the account holds is a signature mismatch, and one it does not hold is a
    /// credential that resolves to nothing.
    #[test]
    fn an_authentication_refusal_names_the_failure_the_presented_credential_had() {
        let service = service();
        // One row: the access key id the refused request presented, and the code and
        // message AWS words for that credential's failure.
        let rows: Vec<(&str, Option<&str>, &str, &str)> = vec![
            (
                "an IAM user's own access key, which the account holds",
                Some("AKIAEMULATORUSERKEY1"),
                SIGNATURE_MISMATCH_CODE,
                SIGNATURE_MISMATCH_MESSAGE,
            ),
            (
                "a session this service minted",
                Some("ASIAEMULATORSESSION1"),
                SIGNATURE_MISMATCH_CODE,
                SIGNATURE_MISMATCH_MESSAGE,
            ),
            (
                "a syntactically valid key id that was never issued",
                Some("AKIAIOSFODNN7EXAMPLE"),
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
            (
                "a key id one character off a held one",
                Some("AKIAEMULATORUSERKEY2"),
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
            (
                // No credential scope at all: the framework reads the key id as the
                // sentinel `default`, which names no credential.
                "no credential at all",
                None,
                UNRESOLVED_CREDENTIAL_CODE,
                UNRESOLVED_CREDENTIAL_MESSAGE,
            ),
        ];

        for (case, access_key_id, expected_code, expected_message) in rows {
            let scope = access_key_id.map(|key| (key, SIGNING_NAME));
            let parts = parts("POST", "/", scope, None, None, Some(QUERY_CONTENT_TYPE));
            remember_presented_access_key_id(&parts);
            let refused = service.unauthenticated_refusal();
            assert_eq!(
                (refused.code.as_str(), refused.message.as_str()),
                (expected_code, expected_message),
                "case {case:?}: expected {expected_code} with AWS's own wording, got {refused:?}"
            );
            // The note the wording is taken from is REQUEST-scoped: a refusal that this
            // service's own claim did not precede reads no credential at all, rather than
            // the one the previous request presented.
            let unattributed = service.unauthenticated_refusal();
            assert_eq!(
                (unattributed.code.as_str(), unattributed.message.as_str()),
                (UNRESOLVED_CREDENTIAL_CODE, UNRESOLVED_CREDENTIAL_MESSAGE),
                "case {case:?}: expected a refusal with no credential noted to resolve \
                 nothing, got {unattributed:?}"
            );
        }
    }
}
