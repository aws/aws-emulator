// SPDX-License-Identifier: CC0-1.0

//! The FRONT DOOR an AWS REST service puts in front of its application code, as a
//! wrapper any service can serve its operations behind.
//!
//! # What a front door is
//!
//! Several AWS REST services validate a request's JSON BODY against the operation's
//! modelled input at a layer AHEAD of the code that owns the service's modelled
//! errors, and reject it there: HTTP 400 with the error code
//! [`BAD_REQUEST_EXCEPTION`] — a shape those services' models do not declare — and the
//! fixed sentence [`INVALID_REQUEST_BODY`], which names neither the member nor the
//! value. A constraint on a value bound to the request URI (`@httpLabel`,
//! `@httpQuery`) is NOT a body rejection: the service itself refuses those, as the
//! modelled validation error, naming what failed.
//!
//! # Why this cannot live in `Service::override_refusal`
//!
//! The framework refuses every modelled constraint before a handler runs and words
//! that refusal through [`smithy_server_runtime::Service::override_refusal`], which is
//! handed the violations ALONE — no operation, no request — so it cannot tell a body
//! violation from a URI one. Two operations of one service can even bind the same
//! member name both ways (a create carrying an ARN in the body, a read carrying it in
//! the path), so no rule over violation paths alone can place them.
//!
//! [`FrontDoor`] is where that distinction lives: a service serves each modelled
//! operation wrapped in it (through its `override_operation` seam), and the wrapper
//! reads the HTTP BINDING of each violated member off THAT operation's own input
//! schema.
//!
//! # Services still carrying their own copy
//!
//! This rule predates this module and is inlined in the service crates that needed it
//! before there was anywhere shared to put it —
//! `grep -rln 'fn binds_to_body\|fn body_bound\|fn is_bound_to_request' services/*/src/service.rs`
//! lists them (today: `networkmonitor`, `artifact`, `uxc`, `directoryservicedata`).
//! Those copies are to be MIGRATED onto this module; they are left in place only
//! because this change does not own those services' issues, and each is covered by its
//! own suite. Nothing new should inline the rule again — call this module.
//!
//! # Two wordings
//!
//! Most front doors answer the one fixed sentence above. Some report, in brackets, the
//! CLAUSE their body-schema validator states for each constraint class it found — the
//! regex a value does not match, the length bound a string broke. That is the only thing
//! a service chooses here: [`FrontDoor::worded`] takes a [`BodyWording`] built from the
//! sentences in [`crate::validation_message`], so the sentences themselves still cannot
//! drift per crate.
//!
//! # The other half: a value bound to the URI
//!
//! A constraint broken by a value the request carries OUTSIDE its body is not a body
//! rejection, so a front door leaves it to the layers behind the door by default — the
//! framework's walk, worded through `Service::override_refusal`. A service that words
//! such a refusal PER OPERATION cannot word it there either, and for the same reason the
//! body half cannot: a service-level seam is handed the violations alone, so two
//! operations refusing the same member name are indistinguishable to it — and AWS does
//! word them differently, one operation to the next. [`FrontDoor::wording_uri`] is where
//! that choice lives: the wording is attached to ONE wrapped operation, so the sentence
//! belongs to the operation that raised the refusal.
//!
//! # The stage ahead of the body: the caller
//!
//! A REST front door decides WHO is calling before it reads what the call says, and the
//! services reached through one answer a caller it could not verify with that layer's own
//! refusal rather than with any of their modelled errors: HTTP [`UNAUTHORIZED_STATUS`],
//! the wire code [`UNAUTHORIZED_CODE`] and the one fixed sentence
//! [`UNAUTHORIZED_MESSAGE`], and nothing else in the body
//! ([`unauthorized_response`]). The emulator verifies every signature centrally and a
//! service never checks one itself, so the only seam handed that verdict is the service's
//! own definition, where `refusal_response` renders it: [`UnauthorizedDefinition`] wraps a
//! generated definition there. It is the sibling of
//! [`crate::authentication_refusal::AuthenticationRefusalDefinition`], which restates the
//! same verdict for the services whose answer is a MODELLED shape at the protocol's own
//! status; this one answers a shape no model declares, on its own status, which a wording
//! seam cannot do.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::constraints::path;
use aws_smithy_schema_server::ProtocolError;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, Operation, OperationContext,
    OperationRoutingTable, Refused, ServerHttpProtocol, Service, ServiceIdentity, Violation,
    WireProtocol,
};

use crate::validation::SubmittedEcho;

/// The HTTP status a REST front door refuses a caller it could not verify with: the
/// CALLER was refused, rather than the request's body (400), its permissions on a resource
/// (403) or the resource it named (404).
pub const UNAUTHORIZED_STATUS: u16 = 401;

/// The wire error code that refusal names, in `x-amzn-errortype` — the header `restJson1`
/// resolves an error shape from. No model declares it: the layer that states the refusal
/// is not the service, so a client finds no modelled shape behind the code, which is what
/// tells it which layer answered.
pub const UNAUTHORIZED_CODE: &str = "UnauthorizedException";

/// The one sentence that refusal states, for every caller it refuses.
///
/// Wholly fixed text: it names no member, no value, no resource and no principal, so
/// nothing about this account — or about which half of the credential was wrong — reaches
/// a caller the layer refused.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The member that sentence travels under: LOWERCASE `message`, which is how the layer
/// ahead of the service writes it — the modelled errors of the services behind one often
/// carry theirs under `Message`, so which member holds the sentence is part of what tells
/// a client that this refusal is not one of theirs.
const UNAUTHORIZED_MESSAGE_MEMBER: &str = "message";

/// The media type a `restJson1` service renders a body in.
const JSON_CONTENT_TYPE: &str = "application/json";

/// The front door's refusal of a caller it could not verify: the status, the wire error
/// code in the header a client resolves an error from, and a body carrying the one fixed
/// sentence under its own lowercase member — and nothing else.
///
/// Built here rather than through the framework's refusal renderer because that renderer
/// takes the status from the protocol (400 for a constraint refusal) and the envelope from
/// the service's model, and this refusal is neither the protocol's status nor a modelled
/// shape.
#[must_use]
pub fn unauthorized_response() -> RtResponse {
    let body = ::serde_json::json!({ UNAUTHORIZED_MESSAGE_MEMBER: UNAUTHORIZED_MESSAGE });
    let status =
        StatusCode::try_from(UNAUTHORIZED_STATUS).expect("the refusal's status is a constant");
    let mut response = RtResponse::new(status, SdkBody::from(body.to_string()));
    let headers = response.headers_mut();
    headers.insert("content-type", JSON_CONTENT_TYPE);
    headers.insert("x-amzn-errortype", UNAUTHORIZED_CODE);
    response
}

/// A service's model-derived half with the AUTHENTICATION layer's refusal stated as the
/// front door ahead of that service states it (see the module docs): HTTP
/// [`UNAUTHORIZED_STATUS`], [`UNAUTHORIZED_CODE`], [`UNAUTHORIZED_MESSAGE`].
///
/// Identity, the routing table, the protocol and every other rendering come from the
/// wrapped definition untouched — nothing model-derived is restated here. Generic over
/// that definition rather than over an engine, so this crate names no service's generated
/// type.
///
/// A refusal the service worded itself through
/// [`smithy_server_runtime::Service::override_refusal`] is passed through, and so is every
/// other refusal class: this wrapper answers the one verdict a wording seam cannot, the
/// one the layer itself reached about the CALLER.
pub struct UnauthorizedDefinition<D> {
    inner: D,
}

impl<D: ErasedServiceDefinition> UnauthorizedDefinition<D> {
    /// Wrap a service's generated definition.
    pub fn new(inner: D) -> Self {
        Self { inner }
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for UnauthorizedDefinition<D> {
    fn identity(&self) -> &ServiceIdentity {
        self.inner.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.inner.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.inner.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        self.inner.routing_error_response(req)
    }

    fn framework_error_response(&self, error: ProtocolError, req: &RtRequest) -> RtResponse {
        self.inner.framework_error_response(error, req)
    }

    fn refusal_response(
        &self,
        error: ProtocolError,
        chosen: Option<Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        match (&chosen, &error) {
            // The authentication layer refused the caller: the endpoint states that
            // refusal itself, whichever way the credential failed — the sentence names no
            // half of it, so there is nothing per-failure to word.
            (None, ProtocolError::Unauthenticated { .. }) => unauthorized_response(),
            _ => self.inner.refusal_response(error, chosen, req),
        }
    }

    fn wire_protocol(&self, req: &RtRequest) -> WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}

/// The error code a front door names for a request body that breaks a modelled
/// constraint. NOT a shape in the serving service's model: the rejection happens ahead
/// of the application code that owns the modelled errors, so it is worded through the
/// framework's refusal channel ([`Refused`]) rather than through an error enum.
pub const BAD_REQUEST_EXCEPTION: &str = "BadRequestException";

/// What a front door says about a body it refused.
///
/// It validates the body against the shape and reports the class of failure, not the
/// member: a caller fixes the request from the constraint the model publishes. (The
/// member-by-member clause the framework writes is what the service's own modelled
/// validation error carries, for the refusals the application layer owns.)
pub const INVALID_REQUEST_BODY: &str = "Invalid request body";

/// How a front door words a body it refused, for a service whose recordings show the
/// body-schema validator's CLAUSES rather than the fixed sentence alone.
///
/// It is handed every violation the request's BODY broke, each with the value the caller
/// submitted ([`SubmittedEcho`]) — the framework's own echo of a long value is elided, so
/// a clause that quotes one needs this — and answers the code and the message together,
/// or `None` to leave the request to the layers behind the door.
pub type BodyWording = fn(&[(Violation, SubmittedEcho)]) -> Option<Refused>;

/// How ONE operation words a refusal of a value its request carries outside the body (an
/// `@httpLabel` path segment, an `@httpQuery` parameter).
///
/// Handed every violation of such a value, each with the value the caller submitted
/// ([`SubmittedEcho`]) — a sentence that states the rejected value needs it, because the
/// framework's own echo elides a long one — and answers the code and the message
/// together, or `None` to leave the request to the layers behind the door, which refuse
/// it in the service's own modelled wording.
///
/// The same signature as [`BodyWording`] and a distinct name, because the two are chosen
/// for different halves of one request: a service attaches a [`BodyWording`] to its whole
/// front door and a [`UriWording`] to the single operation whose sentence it is.
pub type UriWording = fn(&[(Violation, SubmittedEcho)]) -> Option<Refused>;

/// The violations of one request a wording is handed: each one with the value the caller
/// submitted, as both [`BodyWording`] and [`UriWording`] take them.
pub type RefusedWithEchoes = Vec<(Violation, SubmittedEcho)>;

/// One modelled operation, served behind its service's front door.
///
/// Wrap every modelled operation of a service that has one, so the rule is applied by
/// ONE code path rather than per operation: an operation that binds nothing to the body
/// simply never has a body violation to find, which the wrapper reads off its input
/// schema.
///
/// Nothing else is re-implemented. A body this wrapper does not refuse is handed to the
/// generated [`ErasedOperation::run`], which owns deserialize -> validate -> handle ->
/// serialize, telemetry and every modelled error — including the modelled validation
/// error for a URI-bound value outside its constraint, and for the rules a handler
/// enforces that the model states only in prose.
///
/// The violations come from the framework's own machinery (the protocol's deserializer
/// and `constraints::validate` over the built input), so this cannot find a different
/// set from the walk that follows it. The cost of that is DELIBERATE: a request this
/// wrapper does not refuse is read and walked twice, once here and once in the
/// operation's own sequence, because the framework hands no already-built input to an
/// operation. Two parses of the same bytes, bounded by the body limits the framework
/// already enforces, buy the per-operation placement no other seam can make.
pub struct FrontDoor<O> {
    operation: O,
    /// How this service's front door words a refused body, or `None` for the fixed
    /// sentence [`INVALID_REQUEST_BODY`].
    wording: Option<BodyWording>,
    /// How THIS operation words a refused value bound to the request URI, or `None` to
    /// leave such a refusal to the layers behind the door.
    uri_wording: Option<UriWording>,
}

impl<O> FrontDoor<O> {
    /// Wrap one operation, already constructed from the shared engine. Its front door
    /// answers the fixed sentence.
    pub fn new(operation: O) -> Self {
        Self {
            operation,
            wording: None,
            uri_wording: None,
        }
    }

    /// Wrap one operation whose front door states `wording`'s clauses instead of the
    /// fixed sentence.
    pub fn worded(operation: O, wording: BodyWording) -> Self {
        Self {
            operation,
            wording: Some(wording),
            uri_wording: None,
        }
    }

    /// Word a refusal of a value THIS operation carries outside its body with `wording`
    /// ([`UriWording`]), rather than leaving it to the layers behind the door.
    ///
    /// A body this door refuses still wins: the body is what the door is in front of, and
    /// a request that breaks a rule in both halves is refused as the body rejection it
    /// already was.
    #[must_use]
    pub fn wording_uri(mut self, wording: UriWording) -> Self {
        self.uri_wording = Some(wording);
        self
    }
}

impl<O> ErasedOperation for FrontDoor<O>
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
            // The door's own answer is computed in one call that reads, uses and drops
            // the protocol: `&dyn ErasedServiceDefinition` is not `Sync`, so holding it
            // across the await below would make this future non-`Send`.
            let refused = self.refusal(svc, req);
            if let Some(response) = refused {
                return response;
            }
            self.operation.run(svc, req, ctx).await
        })
    }

    /// The wrapped operation's modelled auth schemes: wrapping an operation must not
    /// change which schemes sign it.
    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        self.operation.auth_schemes()
    }
}

impl<O: Operation> FrontDoor<O> {
    /// This door's answer to `req`, or `None` when it breaks nothing the door refuses —
    /// in which case the operation behind it is served.
    ///
    /// The body comes first, because that is what a front door stands in front of; a
    /// value bound to the URI is answered only by the wording the wrapped operation
    /// carries for it, and otherwise left to the layers behind the door.
    ///
    /// The protocol is read, used and dropped inside this one function, so nothing
    /// borrowed from the service definition is held across the caller's await.
    fn refusal(&self, svc: &dyn Service, req: &RtRequest) -> Option<RtResponse> {
        // The codec the request is SERVED in, resolved the way the framework's own
        // dispatch resolves it: the author's `override_protocol` when the service replaces
        // the model-derived one, else the one the definition selects FOR THIS REQUEST. A
        // door that read the request with a different codec from the operation behind it
        // would find a different violation set — or none at all, for a service whose own
        // codec is what reads its bodies. The selection is per request because a service
        // may declare several protocols and serves each request in the one it names, so the
        // service's first-declared codec is only its own for a service that declares one.
        let protocol = svc
            .override_protocol()
            .unwrap_or_else(|| svc.definition().wire_protocol(req).protocol());
        // A door that quotes no value at all reads the cheaper walk: the fixed sentence
        // names neither the member nor the value, so no echo has to be restored.
        if self.wording.is_none() && self.uri_wording.is_none() {
            let refused = body_violations::<O>(protocol, req);
            return (!refused.is_empty()).then(|| rejection(svc, req, refused));
        }
        let (body, uri) = violations_by_binding::<O>(protocol, req);
        if !body.is_empty() {
            let violations: Vec<Violation> = match self.wording {
                // A service that words its own body rejection is asked first, and leaves
                // the request to the layers behind the door when it has no wording for it.
                Some(wording) => {
                    let chosen = wording(&body)?;
                    let violations = body.into_iter().map(|(violation, _)| violation).collect();
                    return Some(worded_rejection(svc, req, violations, chosen));
                }
                None => body.into_iter().map(|(violation, _)| violation).collect(),
            };
            return Some(rejection(svc, req, violations));
        }
        let chosen = self.uri_wording?(&uri)?;
        let violations = uri.into_iter().map(|(violation, _)| violation).collect();
        Some(worded_rejection(svc, req, violations, chosen))
    }
}

/// Every modelled constraint `req`'s BODY breaks, for the operation `O`.
///
/// Absence counts: a required BODY member the request omits is found while READING the
/// request rather than while walking the value it built, but it is the same refusal to
/// a caller — the framework's own `refusal_of` folds it into exactly the violation this
/// returns — so a front door rejects it as a body too. Any OTHER codec failure (a body
/// that is not parseable at all, a member of the wrong type) is not a constraint
/// violation, so it is left to the framework's own walk, which refuses it in the
/// service's modelled validation error. That is the conservative reading of the
/// evidence rather than a settled rule: the recordings behind this module cover a body
/// member outside its declared constraints, not a body a codec cannot read at all, so
/// a recording of THAT is what should decide whether a front door words it the same
/// way.
///
/// Empty when the body satisfies the model and when the operation binds nothing to the
/// body.
pub fn body_violations<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> Vec<Violation> {
    // The collecting walk is the shared one (`crate::validation`), so a front door cannot
    // find a different violation set from the framework's own; the echo is left as the
    // framework rendered it, because a front door's answer quotes no value. The narrowing
    // below is this module's own rule.
    crate::validation::violations_in::<O>(protocol, req, crate::validation::Echoes::Rendered)
        .violations
        .into_iter()
        .filter(|violation| binds_to_body(O::INPUT_SCHEMA, &violation.path))
        .collect()
}

/// Every modelled constraint `req` breaks, each violation paired with how a clause can
/// state the value it is about ([`SubmittedEcho`]), split into the ones its BODY broke
/// and the ones a value it carries OUTSIDE the body broke.
///
/// The split is [`binds_to_body`]'s, read off `O`'s own input schema, and the violations
/// are the shared collecting walk's, so a front door cannot find a different set from the
/// framework's own. The echo is restored to the bytes the caller sent, which a clause that
/// QUOTES the value needs: the framework's own echo elides a long one, so a clause built
/// from it would state text the caller never sent.
pub fn violations_by_binding<O: Operation>(
    protocol: &dyn ServerHttpProtocol,
    req: &RtRequest,
) -> (RefusedWithEchoes, RefusedWithEchoes) {
    crate::validation::refusing_violations_with_echoes::<O>(protocol, req)
        .into_iter()
        .partition(|(violation, _)| binds_to_body(O::INPUT_SCHEMA, &violation.path))
}

/// Whether `violation_path` names a member `input_schema` carries in the request BODY.
///
/// Read off the member's own HTTP binding traits, so the model decides: a member with
/// no `@httpLabel` / `@httpQuery` / `@httpQueryParams` / `@httpHeader` /
/// `@httpPrefixHeaders` binding travels in the body. (`@httpPayload` is not one of
/// those: a payload member IS the body.) A nested path (`probe.destination`,
/// `probes.1.member.packetSize`, `tags.someKey.member`) is placed by its FIRST segment,
/// the top-level member the whole subtree hangs off.
pub fn binds_to_body(input_schema: &Schema, violation_path: &str) -> bool {
    // `split` always yields at least one item, so this is the whole path when it has
    // no `.` in it.
    let member_name = violation_path.split('.').next().unwrap_or_default();
    input_schema.members().iter().any(|member| {
        member
            .member_name()
            .is_some_and(|name| path::member(name) == member_name)
            && member.http_label().is_none()
            && member.http_query().is_none()
            && member.http_query_params().is_none()
            && member.http_header().is_none()
            && member.http_prefix_headers().is_none()
    })
}

/// A front door's answer to a body it refused: the service's protocol envelope and the
/// status the framework's refusal channel chooses for a constraint failure (400), with
/// [`BAD_REQUEST_EXCEPTION`] and [`INVALID_REQUEST_BODY`] replacing the code and the
/// message together — a client branches on the code, so one without the other is worse
/// than neither.
pub fn rejection(svc: &dyn Service, req: &RtRequest, refused: Vec<Violation>) -> RtResponse {
    worded_rejection(
        svc,
        req,
        refused,
        Refused {
            code: BAD_REQUEST_EXCEPTION.to_string(),
            message: INVALID_REQUEST_BODY.to_string(),
        },
    )
}

/// As [`rejection`], with the code and message a service's own [`BodyWording`] chose.
///
/// The envelope and the status stay the framework's — a constraint failure is a 400 in
/// every protocol, rendered in the envelope of the protocol `req` is served in — so a
/// service's wording decides the words and nothing else.
pub fn worded_rejection(
    svc: &dyn Service,
    req: &RtRequest,
    refused: Vec<Violation>,
    chosen: Refused,
) -> RtResponse {
    svc.definition()
        .refusal_response(ProtocolError::Invalid(refused), Some(chosen), req)
}

/// The routing table a service serves its operations behind its front door through:
/// every operation wrapped in [`FrontDoor`], keyed by the routing key the protocol
/// resolves.
///
/// A service builds one in its `EmulatedService::new` and returns it from
/// `override_operation`, which is the whole wiring: [`serve`](Self::serve) takes one
/// already-constructed operation, wraps it and keys it by the operation's own
/// [`Operation::NAME`] — the model-derived shape name the generated table is keyed by, so
/// a wrapped operation cannot be filed under a key the protocol never resolves. Every
/// service that had this wiring wrote the same wrapping loop by hand; this is that loop,
/// once.
pub struct FrontDoorTable {
    table: smithy_server_runtime::OperationRoutingTable,
    wording: Option<BodyWording>,
}

impl FrontDoorTable {
    /// An empty table whose doors answer the fixed sentence [`INVALID_REQUEST_BODY`].
    #[must_use]
    pub fn new() -> Self {
        Self {
            table: smithy_server_runtime::OperationRoutingTable::new(),
            wording: None,
        }
    }

    /// An empty table whose doors state `wording`'s clauses instead.
    #[must_use]
    pub fn worded(wording: BodyWording) -> Self {
        Self {
            table: smithy_server_runtime::OperationRoutingTable::new(),
            wording: Some(wording),
        }
    }

    /// Serve `operation` behind this table's front door, under its modelled name.
    pub fn serve<O: Operation + ErasedOperation + 'static>(&mut self, operation: O) -> &mut Self {
        let door = match self.wording {
            Some(wording) => FrontDoor::worded(operation, wording),
            None => FrontDoor::new(operation),
        };
        self.table.add_operation(O::NAME, Box::new(door));
        self
    }

    /// The table, for the service to hold and answer `override_operation` from.
    #[must_use]
    pub fn finish(self) -> smithy_server_runtime::OperationRoutingTable {
        self.table
    }
}

impl Default for FrontDoorTable {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! Where a member travels, read off one input schema that binds members every
    //! way a REST protocol can.

    use super::*;
    use aws_smithy_schema::{ShapeId, ShapeType};

    static LABEL: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$resourceArn", "test#Input", "resourceArn"),
        ShapeType::String,
        "resourceArn",
        0,
    )
    .with_http_label();
    static QUERY: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$tagKeys", "test#Input", "tagKeys"),
        ShapeType::List,
        "tagKeys",
        1,
    )
    .with_http_query("tagKeys");
    static HEADER: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$clientToken", "test#Input", "clientToken"),
        ShapeType::String,
        "clientToken",
        2,
    )
    .with_http_header("x-client-token");
    static BODY_SCALAR: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$spendingLimit", "test#Input", "spendingLimit"),
        ShapeType::String,
        "spendingLimit",
        3,
    );
    static BODY_STRUCT: Schema = Schema::new_member(
        ShapeId::from_static("test#Input$timePeriod", "test#Input", "timePeriod"),
        ShapeType::Structure,
        "timePeriod",
        4,
    );
    static MEMBERS: &[&Schema] = &[&LABEL, &QUERY, &HEADER, &BODY_SCALAR, &BODY_STRUCT];
    static INPUT: Schema = Schema::new_struct(
        ShapeId::from_static("test#Input", "test", "Input"),
        ShapeType::Structure,
        MEMBERS,
    );

    /// A violation path is placed by the HTTP binding of the top-level member it hangs
    /// off — for every location a REST protocol binds, at every depth.
    #[test]
    fn a_violation_is_placed_by_its_top_level_members_binding() {
        // (case, violation path, carried in the body)
        let rows: [(&str, &str, bool); 8] = [
            ("a URI label", "resourceArn", false),
            ("a query parameter", "tagKeys", false),
            ("an element of a query list", "tagKeys.1.member", false),
            ("a header", "clientToken", false),
            ("a body member", "spendingLimit", true),
            ("a nested body member", "timePeriod.startAt", true),
            ("a body member's map entry", "timePeriod.someKey.member", true),
            ("a member this shape does not declare", "nobody", false),
        ];

        for (case, violation_path, expected) in rows {
            let actual = binds_to_body(&INPUT, violation_path);
            assert_eq!(
                actual, expected,
                "case {case:?} ({violation_path}): expected binds_to_body == {expected}, \
                 got {actual}"
            );
        }
    }
}
