// SPDX-License-Identifier: CC0-1.0

//! The request id a response reports, for every emulated service: the `<RequestId>`
//! element an `awsQuery` `<ErrorResponse>` carries beside its `<Error>`, and the response
//! HEADER AWS reports the same id in whatever the protocol.
//!
//! AWS answers an `awsQuery` error with
//!
//! ```xml
//! <ErrorResponse>
//!   <Error><Type>Sender</Type><Code>…</Code><Message>…</Message></Error>
//!   <RequestId>…</RequestId>
//! </ErrorResponse>
//! ```
//!
//! The `<RequestId>` belongs to the ENVELOPE, not to any modelled error shape: no model
//! declares it, and it is what an SDK exposes as the failed call's request id
//! (`ResponseMetadata.RequestId` in botocore), the counterpart of the
//! `ResponseMetadata/RequestId` the framework's SUCCESS renderer already writes. The
//! framework's error renderers write `<Error>` alone
//! (`aws_smithy_schema_server::query_error_body` and the `awsQuery` `serialize_error`), so
//! a service that wants the whole envelope adds the element here — through the two seams
//! below, which cover every layer that can answer an error:
//!
//! * [`RequestIdEnvelopeOperation`], the operation wrapper a service hands back from
//!   `Service::override_operation`, covers a handler-returned modelled error and a
//!   framework refusal raised while a matched operation was being served (an absent
//!   required member, a value outside a declared bound) — with
//!   [`RequestIdHeaderOperation`] and [`RequestIdErrorMemberOperation`] as the same
//!   wrapper under the other two reports, or [`RequestIdReportingOperation`], for a
//!   service that holds its operations already wrapped and composes the report around
//!   them;
//! * [`RequestIdEnvelopeDefinition`], the definition wrapper a service hands back from
//!   `Service::definition`, covers a routing failure (an `Action` naming no modelled
//!   operation), a transport failure detected before any operation was named, and a
//!   pre-dispatch authentication refusal;
//! * a response a service renders ITSELF (an API-version refusal on a shared endpoint,
//!   say) is passed through [`with_request_id`] directly.
//!
//! # The two reports
//!
//! WHERE a response states the id is the one protocol-dependent part, so it is the one
//! thing a service chooses here. The two seams above take that choice as a REPORT — a
//! function from the rendered response to the response as it is sent — and the plumbing
//! around it (resolving the modelled operation, delegating to it, wrapping the three
//! framework-level renderers) is stated once and shared by both:
//!
//! * [`with_request_id`], the `awsQuery` report: the `<RequestId>` element in the error
//!   envelope AND the header, carrying one id so a client correlating on either sees the
//!   same value. Reached through [`RequestIdEnvelopeOperation`] and
//!   [`RequestIdEnvelopeDefinition::new`].
//! * [`with_request_id_header`], the protocol-independent report: the header ALONE, on
//!   every response — success or failure — which is what a service whose protocol has no
//!   envelope element for the id (`restJson1`, `restXml`, `awsJson`, `rpcv2Cbor`) answers
//!   with. Reached through [`RequestIdHeaderOperation`] and
//!   [`RequestIdEnvelopeDefinition::header_only`].
//! * [`with_request_id_error_member`], the JSON-error-body report: the `requestId` MEMBER
//!   beside the `__type` and the `message`, which is what a service whose recorded failures
//!   carry the id in the body itself answers with. Reached through
//!   [`RequestIdErrorMemberOperation`] (a service serving its generated routing table
//!   unchanged), [`RequestIdReportingOperation::error_member`] (one serving its operations
//!   already wrapped) and [`RequestIdEnvelopeDefinition::error_member`].
//!   [`with_request_id_error_member_named`] is that same report for a service whose error
//!   bodies SPELL the member another way, reached through [`RequestIdReportedOperation`]:
//!   the spelling is the service's own fact, read off its recordings, so it is passed in
//!   rather than restated here.
//! * [`with_request_id_header_and_error_member`], the header report and the body-member
//!   report composed under ONE id: the header on every response and the `requestId` member
//!   of a JSON error body, which is what a service whose recorded failures state the id in
//!   the body AND whose responses carry it in the header answers with. Reached through
//!   [`RequestIdReportedOperation`] and
//!   [`RequestIdEnvelopeDefinition::header_and_error_member`].
//!
//! WHERE the id is stated is the protocol's; WHAT it is spelled like is the service's. The
//! three writers above mint [`new_request_id`], the form almost every AWS service reports,
//! and a service whose own ids are spelled another way mints them itself, states them with
//! [`with_stated_request_id_header`], and hands THAT report ([`Report`]) to
//! [`RequestIdEnvelopeDefinition::reporting`] — so the choice of form costs a service its
//! minting function and nothing else.
//!
//! # WHEN the id is decided
//!
//! Every writer above decides it as the answer is rendered, which is where a correlation id
//! belongs: it is a property of the response and nothing else holds one. A service whose
//! recorded wording states the id inside a MESSAGE needs it sooner, while the handler is
//! building those words, so [`answered_under_one_request_id`] decides it before the walk
//! begins, holds it in scope for [`served_request_id`] to read, and reports it on the way out
//! through a [`StatedReport`] ([`named_request_under`] for `awsQuery`,
//! [`with_stated_request_id_header`] for the header alone) — so the message, the envelope and
//! the header of one answer cannot name different ids.
//!
//! Only the request id is this module's business: the HTTP status, the `<Type>`, the
//! `<Code>`, the `<Message>`, the body of a success and every other header stay with the
//! framework renderer that owns them, so each has exactly one home. The only
//! protocol-specific part is the `awsQuery` element: a document that is not an `awsQuery`
//! `<ErrorResponse>` is returned untouched by [`with_request_id`], so a service of any
//! other protocol can hold that wrapper harmlessly — and reaches for
//! [`with_request_id_header`] instead, which reports the id on its every response without
//! reading the body at all.
//!
//! This is the shared home the service-local copies of this rule record as their migration
//! target — `services/service-com-amazonaws-rds-amazonrdsv19`,
//! `services/service-com-amazonaws-docdb-amazonrdsv19`,
//! `services/service-com-amazonaws-cloudformation-cloudformation`,
//! `services/service-com-amazonaws-sns-amazonsimplenotificationservice`,
//! `services/service-com-amazonaws-autoscaling-autoscaling-2011-01-01`,
//! `services/service-com-amazonaws-elasticbeanstalk-awselasticbeanstalkservice` and
//! `services/service-com-amazonaws-redshift-redshiftserviceversion20121201`, each in that
//! service's own `src/service/query_error_envelope.rs`. This module carries the most
//! complete of those behaviours — the `x-amzn-requestid` header and the sibling-scoped
//! idempotence check that the latest of them (autoscaling, redshift) have, which the
//! earlier ones (rds, docdb, cloudformation, sns) do not — so a service migrating onto it
//! gains those two and loses nothing; performing those migrations belongs to those
//! services' issues, so they are left where they are rather than moved by a change that
//! does not own them. Nothing NEW should state the rule again — call this module.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

/// The closing tag of the `awsQuery` error envelope: the point the element is inserted
/// before, and the one marker that tells an error document from a successful one (whose
/// own request id already sits in `<ResponseMetadata>`).
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The closing tag of the element the request id is a SIBLING of: what sits between it and
/// [`ERROR_RESPONSE_CLOSE`] is the only place a `<RequestId>` of this ENVELOPE can be.
const ERROR_ELEMENT_CLOSE: &str = "</Error>";

/// The element this module adds.
const REQUEST_ID_OPEN: &str = "<RequestId>";

/// The header AWS reports the same id in, so a client correlating on the header and one
/// reading the document see one value.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// Mint the id one response reports as its own.
///
/// A correlation id, not a security token, and per-response rather than per-resource: two
/// responses must be distinguishable, which is what a client logs and what an SDK's retry
/// and telemetry layers read. A fresh v4 UUID from the maintained `uuid` crate — the same
/// thing the framework's own renderers put in a successful response's
/// `ResponseMetadata/RequestId` (`vendor/smithy-server-runtime/src/pre_dispatch.rs`), so a
/// client cannot tell the two apart. It needs no state, so a request refused before any
/// store is reached still answers with one.
///
/// It is the form MOST AWS services report, not every one: the id is opaque to a client, so
/// a service free to spell it its own way sometimes does (a fixed-width run of uppercase
/// alphanumeric characters, say). A service whose ids have a form of its own mints them
/// itself, states them through [`with_stated_request_id_header`], and hands that report to
/// the two wrappers below ([`Report`]) rather than restating this plumbing.
pub fn new_request_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Whether `id` has the 8-4-4-4-12 lowercase-hex form MOST AWS services' request ids carry
/// — what [`new_request_id`] mints, and the only shape this module will report as one. A
/// service that mints its own form ([`new_request_id`]) states and asserts that form itself.
///
/// Public so a service's own test can assert the FORM of the id its responses name the
/// request by without restating that form; the module's writers below are its only other
/// readers.
pub fn is_request_id(id: &str) -> bool {
    let mut groups = id.split('-');
    for width in [8usize, 4, 4, 4, 12] {
        let Some(group) = groups.next() else {
            return false;
        };
        let hex = group.len() == width
            && group
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
        if !hex {
            return false;
        }
    }
    groups.next().is_none()
}

/// Where the envelope ends, and the span its OWN children occupy.
///
/// The `<RequestId>` belongs to the ENVELOPE, so only the envelope's own children count: a
/// `<RequestId>` a modelled error shape carries inside its `<Error>`, and the text of a
/// `<Message>` the renderer built from the request, are not the envelope's. So the span is
/// what follows the last `</Error>` — and the whole envelope when there is no closing
/// `</Error>` to anchor on (a self-closing `<Error/>`). One reader for both the idempotence
/// decision and the id, so the two cannot disagree about where that span starts.
///
/// `None` for a document that is not an `awsQuery` error envelope at all.
fn envelope_children(body: &str) -> Option<(usize, &str)> {
    let close = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let siblings = match body[..close].rfind(ERROR_ELEMENT_CLOSE) {
        Some(error_end) => &body[error_end..close],
        None => &body[..close],
    };
    Some((close, siblings))
}

/// The `<RequestId>` the envelope ALREADY states, read from the sibling span.
///
/// `None` for a document that is not an `awsQuery` error envelope, for one that states no
/// id of its own, and for a "stated id" that does not have the FORM of one: the span is
/// located textually and renderer-built text is the one part of an error document a
/// request can influence, so nothing but a well-formed id is ever reported as the
/// response's id.
fn envelope_request_id(body: &str) -> Option<&str> {
    let (_, siblings) = envelope_children(body)?;
    let stated = first_element_text(siblings, "RequestId")?;
    is_request_id(stated).then_some(stated)
}

/// Add `<RequestId>` to an `awsQuery` `<ErrorResponse>` document, as the last child of the
/// envelope (the sibling after `<Error>`).
///
/// `None` means the document needs nothing: it is not an `<ErrorResponse>` envelope (a
/// successful `<…Response>` carries its request id in `ResponseMetadata` already), or the
/// envelope's own children already hold one — so this is idempotent, and a future framework
/// renderer that starts emitting the element itself cannot end up with two.
fn add_request_id(body: &str, request_id: &str) -> Option<String> {
    let (close, siblings) = envelope_children(body)?;
    if siblings.contains(REQUEST_ID_OPEN) {
        return None;
    }
    Some(format!(
        "{}<RequestId>{request_id}</RequestId>{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response with a freshly minted `<RequestId>` in its `<ErrorResponse>` envelope, and
/// the same id in the header AWS reports it in.
///
/// Status, headers and every other byte of the body are left exactly as the renderer wrote
/// them; a response that is not an `awsQuery` error envelope is returned untouched (and
/// mints nothing), so the success path cannot be affected. An envelope that already holds
/// an id keeps its document, and the header reports THAT id — the two can never name
/// different ids — or carries none when what the document holds is not a well-formed id.
pub fn with_request_id(resp: RtResponse) -> RtResponse {
    with_stated_request_id(resp, &new_request_id())
}

/// The response with `request_id` stated as the `<RequestId>` of its `<ErrorResponse>`
/// envelope, and the same id in the header AWS reports it in: the report for an answer whose
/// service ALREADY knows which id it names this request by.
///
/// [`with_request_id`] mints the id, because for almost every call the correlation id belongs
/// to the response alone and nothing else holds one. Where the id is decided BEFORE the answer
/// is rendered — because the answer's own words carry it, as a service that states the id
/// inside a message does — the envelope states THAT id instead of a second one, so a client
/// reading the envelope, the header and the message sees one value
/// ([`answered_under_one_request_id`]).
///
/// Idempotent and lossless exactly as [`with_request_id`] is: status, headers and every other
/// byte of the body are left as the renderer wrote them, a response that is not an `awsQuery`
/// error envelope is returned untouched, and an envelope that already holds an id keeps its
/// document and reports that same id.
pub fn with_stated_request_id(mut resp: RtResponse, request_id: &str) -> RtResponse {
    let (rewritten, request_id) = {
        // An error envelope is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error envelope and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        // Not an `awsQuery` error envelope at all: not this module's business, and no id
        // is stated on it.
        if envelope_children(text).is_none() {
            return resp;
        }
        let minted = request_id.to_string();
        match add_request_id(text, &minted) {
            Some(rewritten) => (Some(rewritten), Some(minted)),
            // The envelope already holds an id: the document stands, and the header
            // reports that same id.
            None => (None, envelope_request_id(text).map(str::to_string)),
        }
    };
    if let Some(rewritten) = rewritten {
        // Only a renderer that advertised a length needs it corrected; the framework's
        // error renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = rewritten.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = SdkBody::from(rewritten);
    }
    if let Some(request_id) = request_id {
        // Dropped rather than fatal if the HTTP layer cannot carry it: the document already
        // states the id, and a header is not worth failing a modelled error for.
        let _ = resp.headers_mut().try_insert(REQUEST_ID_HEADER, request_id);
    }
    resp
}

/// The response with the request it answers named in the header AWS reports that id in,
/// and nothing else changed — the protocol-independent report.
///
/// Every response gets one, a success as much as a failure: the header is a property of
/// the RESPONSE, which is why no model declares it and no handler can set it, and it is
/// what SDK logging and `ServiceError::request_id()` read. The body, the status and every
/// other header are left exactly as the renderer wrote them, so this is the whole report
/// for a protocol with no envelope element for the id (`restJson1`, `restXml`, `awsJson`,
/// `rpcv2Cbor`).
///
/// A response that already names its request keeps the id it has, so this is idempotent
/// and two layers of one service cannot report two ids for one call. A header the HTTP
/// layer will not carry is dropped rather than turned into a transport failure, which on
/// an error path would replace a modelled 4xx with a 500 an SDK retries.
pub fn with_request_id_header(resp: RtResponse) -> RtResponse {
    with_stated_request_id_header(resp, &new_request_id())
}

/// The response with `request_id` named as the request it answers, in that same header:
/// the report for an answer whose service ALREADY knows which request it is.
///
/// [`with_request_id_header`] mints the id, because for almost every call the correlation
/// id belongs to the response alone and nothing else holds one. Where the operation that
/// served the call already has the id AWS reports for it — a call that started work which
/// names its own id everywhere else it is observable — the response states THAT id instead
/// of a second one, so a client correlating the call with the work it started reads one
/// value in both places.
///
/// Idempotent and lossless exactly as [`with_request_id_header`] is: a response that
/// already names its request keeps the id it has, the body, the status and every other
/// header are left as the renderer wrote them, and a header the HTTP layer will not carry
/// is dropped rather than turned into a transport failure.
pub fn with_stated_request_id_header(mut resp: RtResponse, request_id: &str) -> RtResponse {
    if resp.headers().get(REQUEST_ID_HEADER).is_some() {
        return resp;
    }
    let _ = resp
        .headers_mut()
        .try_insert(REQUEST_ID_HEADER, request_id.to_string());
    resp
}

/// The body member a JSON error envelope reports the request id in, for the services whose
/// error bodies carry it beside `message` rather than in a header alone.
const REQUEST_ID_MEMBER: &str = "requestId";

/// The lowest HTTP status that is a failure, and so the lowest one answered by an error
/// body at all.
const FIRST_FAILURE_STATUS: u16 = 400;

/// Add `"requestId"` to a JSON error body, as its last member.
///
/// `None` means the document needs nothing: it is not a JSON OBJECT, or it already carries
/// the member — so this is idempotent, and a renderer that starts writing the member itself
/// cannot end up with two. The insertion is TEXTUAL, at the object's closing brace, so every
/// other byte of the body — the member order a renderer chose, its escaping, its spacing —
/// survives exactly as it was written; the pair that goes in is SERIALIZED rather than
/// formatted, so a member name or an id holding a character JSON escapes cannot produce a
/// body that no longer parses.
fn add_error_request_id(body: &str, request_id: &str, member: &str) -> Option<String> {
    let document: serde_json::Value = serde_json::from_str(body).ok()?;
    let members = document.as_object()?;
    if members.contains_key(member) {
        return None;
    }
    let close = body.rfind('}')?;
    let separator = if members.is_empty() { "" } else { "," };
    let name = serde_json::to_string(member).ok()?;
    let value = serde_json::to_string(request_id).ok()?;
    Some(format!(
        "{}{separator}{name}:{value}{}",
        &body[..close],
        &body[close..]
    ))
}

/// The response with the request it answers named in the `requestId` MEMBER of its JSON
/// error body — the report for a service whose own error envelope carries the id there.
///
/// Several AWS REST services answer a failure as
/// `{"__type":"<shape>","message":"<why>","requestId":"<id>"}`, where the id belongs to the
/// ENVELOPE and not to any modelled error shape: no error shape declares it, so no handler
/// can set it, and the framework's JSON error renderers write the `__type` and the `message`
/// alone. It is the id such a service's SDK surfaces for the failed call, and the same id a
/// successful call reads out of the modelled `requestId` of its own output.
///
/// A SUCCESS is left untouched: its body is the modelled output, which states the id through
/// whatever member the model gives it, and adding one here would write a member the model
/// does not declare. A failure whose body is not a JSON object (an XML envelope, a streamed
/// body) is left untouched too, and one that already names its request keeps the id it has,
/// so two layers of one service cannot report two ids for one call.
pub fn with_request_id_error_member(resp: RtResponse) -> RtResponse {
    with_request_id_error_member_named(resp, REQUEST_ID_MEMBER)
}

/// [`with_request_id_error_member`], with the MEMBER the error body spells the id in chosen
/// by the caller.
///
/// WHICH member a service's error bodies carry the id in is that service's own fact, read off
/// its recordings — several spell it `requestId`, others `RequestId` beside the capitalized
/// members their models declare — and it is the ONLY thing a caller chooses here. Everything
/// else is the body [`with_request_id_error_member`] runs: the same minted id, the same
/// idempotence, the same textual insertion that leaves every other byte as the renderer wrote
/// it, and the same silence about a success or a body that is not a JSON object.
pub fn with_request_id_error_member_named(resp: RtResponse, member: &str) -> RtResponse {
    with_stated_error_member(resp, &new_request_id(), member)
}

/// The response with `request_id` named as the request it answers, in that same body member:
/// the body-member report for an answer whose service ALREADY knows which request it is.
///
/// [`with_request_id_error_member`] mints the id, because for almost every call the
/// correlation id belongs to the response alone and nothing else holds one. A service that
/// reports the id in MORE than one place — the body member of a failure and the header of
/// every response alike ([`with_request_id_header_and_error_member`]) — states the one id it
/// already minted here, so a client correlating on either reads the same value.
///
/// Which responses it touches and what it leaves alone are exactly
/// [`with_request_id_error_member`]'s: a success, a body that is not a JSON object and a body
/// that already names its request are all returned as the renderer wrote them.
pub fn with_stated_request_id_error_member(resp: RtResponse, request_id: &str) -> RtResponse {
    with_stated_error_member(resp, request_id, REQUEST_ID_MEMBER)
}

/// The ONE statement of the body-member report: `request_id` written into `member` of a JSON
/// error body, which is what the three writers above differ only in the id and the member of.
fn with_stated_error_member(mut resp: RtResponse, request_id: &str, member: &str) -> RtResponse {
    if resp.status().as_u16() < FIRST_FAILURE_STATUS {
        return resp;
    }
    let rewritten = {
        // An error body is built in memory by its renderer, so the bytes are readable
        // here; a streamed body is not an error body and is left alone.
        let Some(bytes) = resp.body().bytes() else {
            return resp;
        };
        let Ok(text) = std::str::from_utf8(bytes) else {
            return resp;
        };
        add_error_request_id(text, request_id, member)
    };
    if let Some(rewritten) = rewritten {
        // Only a renderer that advertised a length needs it corrected; the framework's
        // error renderers set `content-type` alone and leave framing to the transport.
        if resp.headers().get("content-length").is_some() {
            let length = rewritten.len().to_string();
            let _ = resp.headers_mut().try_insert("content-length", length);
        }
        *resp.body_mut() = SdkBody::from(rewritten);
    }
    resp
}

/// The response with the request it answers named in BOTH places a JSON service states it
/// in: the header AWS reports it in on every response, and the `requestId` member of a JSON
/// error body — under ONE id.
///
/// The report for a service whose recorded failures carry the id in the body itself AND whose
/// every response carries it in the header: the two writers it composes
/// ([`with_stated_request_id_header`], through [`named_request_as`], and
/// [`with_stated_request_id_error_member`]) are handed the
/// same value, so a client correlating out of the header and one parsing it out of the body
/// read the same id for one call. An id a layer below already stated in the header is the one
/// reported, which keeps this idempotent: two layers of one service cannot report two ids for
/// one call.
///
/// Nothing else changes: a success keeps its modelled body, a failure whose body is not a
/// JSON object keeps it byte for byte, and the status and every other header stay as the
/// renderer wrote them.
pub fn with_request_id_header_and_error_member(resp: RtResponse) -> RtResponse {
    let (resp, stated) = named_request_as(resp, new_request_id);
    with_stated_request_id_error_member(resp, &stated)
}

/// The text of the FIRST `<name>` element of `text`, or `None` when it holds none.
fn first_element_text<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&format!("</{name}>"))? + start;
    Some(&text[start..end])
}

/// The request id a SUCCESSFUL query response states: `awsQuery` reports it in the
/// `<ResponseMetadata>` block that closes the document, and the `ec2Query` dialect in a
/// lower-case `<requestId>` in place of that block (both written by
/// `aws_smithy_schema_server::query`).
///
/// `None` for a document that states none, and for a stated value that does not have the
/// FORM of a request id — the same guard [`envelope_request_id`] applies, for the same
/// reason.
fn success_request_id(body: &str) -> Option<&str> {
    const RESPONSE_METADATA_OPEN: &str = "<ResponseMetadata>";
    let stated = match body.find(RESPONSE_METADATA_OPEN) {
        Some(at) => first_element_text(&body[at..], "RequestId"),
        None => first_element_text(body, "requestId"),
    }?;
    is_request_id(stated).then_some(stated)
}

/// The id a rendered response ALREADY names the request by, as the caller reads it: the
/// header, the `awsQuery` error envelope's own element, or a successful query document's
/// `ResponseMetadata`.
///
/// Read-only — it mints nothing and changes nothing — so a layer that needs to know which
/// request a response names (an audit record of the call, which carries that id so a client
/// holding the response can find the record of that very call) can ask without affecting
/// what the caller receives. `None` means the renderer named no request, which is what
/// [`named_request`] then reports one for.
pub fn reported_request_id(resp: &RtResponse) -> Option<String> {
    if let Some(stated) = resp.headers().get(REQUEST_ID_HEADER) {
        if is_request_id(stated) {
            return Some(stated.to_string());
        }
    }
    // Built in memory by its renderer, so the bytes are readable here; a streamed body
    // states no request id.
    let text = std::str::from_utf8(resp.body().bytes()?).ok()?;
    match envelope_children(text) {
        Some(_) => envelope_request_id(text).map(str::to_string),
        None => success_request_id(text).map(str::to_string),
    }
}

/// One response and the ONE id it names its request by, reported where the renderer named
/// none.
///
/// A response that already names its request ([`reported_request_id`]) is returned
/// untouched under that id — so every success keeps the id its `ResponseMetadata` already
/// carries, and nothing here can make a caller see two ids for one call. One that names
/// none is reported through the two writers above ([`with_request_id`], then
/// [`with_request_id_header`]), which is why this is idempotent and why a layer outside it
/// that reports the id again finds the same one.
pub fn named_request(resp: RtResponse) -> (RtResponse, String) {
    if let Some(stated) = reported_request_id(&resp) {
        return (resp, stated);
    }
    let resp = with_request_id_header(with_request_id(resp));
    // Every path above reports an id; a response whose header the HTTP layer refused is
    // named by the id it would have carried rather than by nothing.
    let stated = reported_request_id(&resp).unwrap_or_else(new_request_id);
    (resp, stated)
}

/// One response and the ONE id it names its request by, where that id is spelled in a FORM
/// of the serving service's own and stated in the header alone.
///
/// [`named_request`] is the same thing for the form almost every AWS service reports: it
/// reads a stated id through [`reported_request_id`], which only reports a value having
/// THAT form, so a service whose ids are spelled another way (a fixed-width run of
/// uppercase alphanumeric characters, say) would be told its own response names no
/// request. This reads the header the id is stated in without holding the value to a form,
/// and mints one with `mint` only where no layer stated any — so a layer that needs to know
/// which request a response names (an audit record of the call, which carries that id so a
/// client holding the response can find the record of that very call) reads the id the
/// CALLER was answered under whichever form the service spells its ids in.
///
/// Idempotent exactly as [`named_request`] is, and the whole report for a protocol with no
/// envelope element for the id: the body, the status and every other header are left as the
/// renderer wrote them.
pub fn named_request_as(resp: RtResponse, mint: fn() -> String) -> (RtResponse, String) {
    let stated = resp
        .headers()
        .get(REQUEST_ID_HEADER)
        .filter(|stated| !stated.is_empty())
        .map(ToString::to_string);
    if let Some(stated) = stated {
        return (resp, stated);
    }
    let minted = mint();
    (with_stated_request_id_header(resp, &minted), minted)
}

/// One response named by the id its service ALREADY decided this request would be answered
/// under, reported where the renderer named none.
///
/// [`named_request`] is the same thing for a call whose id nothing holds before the answer
/// exists: it mints one on the way out. This states the id that was decided first, in both
/// places that report it — the `awsQuery` envelope element and the header — so an answer whose
/// own words carry the id cannot name one id in its message and another in its envelope.
///
/// Idempotent and lossless exactly as [`named_request`] is: a response that already names its
/// request keeps the id it has (so every success keeps the id its `ResponseMetadata` carries),
/// and the body, the status and every other header are left as the renderer wrote them.
pub fn named_request_under(resp: RtResponse, request_id: &str) -> RtResponse {
    if reported_request_id(&resp).is_some() {
        return resp;
    }
    with_stated_request_id_header(with_stated_request_id(resp, request_id), request_id)
}

::tokio::task_local! {
    /// The id the answer to the request this task is serving names that request by, put in
    /// scope by [`answered_under_one_request_id`] before the request is served and read back
    /// through [`served_request_id`].
    ///
    /// Owned, because the scope holds the id across the whole walk that serves the request and
    /// a task-local cannot carry a borrow of it.
    static SERVED_REQUEST_ID: String;
}

/// The id the answer to the request being served will name that request by, for a layer
/// INSIDE the walk — a handler whose own words have to carry it.
///
/// The id a response reports belongs to the response, so it is normally decided as the
/// response is rendered and no handler can see it. A service whose recorded wording states
/// the id inside a MESSAGE needs it before that, which is what
/// [`answered_under_one_request_id`] decides it for; this is how such a handler reads it, and
/// the value is the one the same answer reports in its envelope and its header.
///
/// `None` outside such a scope — a response rendered by a layer no request-serving scope was
/// opened around, where nothing has decided the id yet.
#[must_use]
pub fn served_request_id() -> Option<String> {
    SERVED_REQUEST_ID.try_with(String::clone).ok()
}

/// How one response of a service reports an id DECIDED BEFORE it was rendered:
/// [`named_request_under`] for `awsQuery`, [`with_stated_request_id_header`] for every other
/// protocol.
///
/// The stated-id counterpart of [`Report`], and a function pointer for the same reason: one
/// service's wrapper is handed the report rather than restating it.
pub type StatedReport = fn(RtResponse, &str) -> RtResponse;

/// Serve one request under ONE request id: decide the id, hold it in scope while the request
/// is served ([`served_request_id`]), and `report` it on whatever was answered.
///
/// For almost every call the id belongs to the response alone and is minted as that response
/// is rendered ([`named_request`]). A service whose recorded wording states the id inside a
/// message cannot wait that long: the words are built by the handler, so the id has to be
/// decided before the walk begins and the same value reported on the way out. That is the
/// whole of what this does — the request is served by exactly the layer that would have served
/// it, and only the id is decided earlier.
pub async fn answered_under_one_request_id<F>(answer: F, report: StatedReport) -> RtResponse
where
    F: ::std::future::Future<Output = RtResponse>,
{
    let decided = new_request_id();
    let answered = SERVED_REQUEST_ID.scope(decided.clone(), answer).await;
    report(answered, &decided)
}

/// The modelled operation the service's routing table names for this request, or `None`
/// when the request names none.
fn modeled_operation<'s>(svc: &'s dyn Service, req: &RtRequest) -> Option<&'s dyn ErasedOperation> {
    let definition = svc.definition();
    let key = definition.protocol().routing_key(req).ok()?;
    definition.routing_table().get(key)
}

/// How one response of a service reports the request it answers: [`with_request_id`] for
/// `awsQuery`, [`with_request_id_header`] for every other protocol.
///
/// A function rather than a type parameter, so one service's operation wrapper and
/// definition wrapper can be handed the SAME report and cannot report differently.
///
/// Public so a service whose report none of the three writers below states — one whose ids
/// have a FORM of their own, minted through [`with_stated_request_id_header`] — can hand
/// its own report to [`RequestIdEnvelopeDefinition::reporting`] and reuse all of this
/// plumbing.
pub type Report = fn(RtResponse) -> RtResponse;

/// Serve the modelled operation the service's own routing table names for `req`, exactly
/// as that table would, and `report` the request id on the response.
///
/// The ONE statement of the operation-wrapper plumbing: the two wrappers below differ only
/// in the report they pass in. No operation of its own is stored and no routing decision is
/// made here — the operation run is the one the service's table names for the request, so
/// the framework still owns the whole deserialize -> validate -> handle -> serialize
/// sequence and every status, code and message it answers with.
async fn serve_reporting<'r>(
    svc: &'r dyn Service,
    req: &'r RtRequest,
    ctx: &'r OperationContext<'r>,
    report: Report,
) -> RtResponse {
    match modeled_operation(svc, req) {
        Some(op) => report(op.run(svc, req, ctx).await),
        // Unreachable through a `Service::override_operation` that claims a request only
        // when the routing table holds its key ([`RequestIdEnvelopeDefinition::routes`]);
        // answered as the framework answers an unroutable request rather than guessed at
        // (and a service holding the definition wrapper reports the id on it there).
        None => svc.definition().routing_error_response(req),
    }
}

/// Serves a modelled operation through the framework exactly as the routing table would,
/// then adds the envelope's `<RequestId>` to the response — the `awsQuery` report.
///
/// It stores no operation of its own: the one it delegates to is the one the service's
/// routing table names for the request, resolved from the `svc` the framework passes into
/// [`ErasedOperation::run`]. So the framework still owns the whole
/// deserialize → validate → handle → serialize sequence (and every status, code and
/// message it answers with), and this wrapper owns one element of the envelope.
pub struct RequestIdEnvelopeOperation;

impl ErasedOperation for RequestIdEnvelopeOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(serve_reporting(svc, req, ctx, with_request_id))
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names" — which is
        // the operation this wrapper delegates to, so its model-declared schemes decide
        // and this wrapper cannot weaken them.
        None
    }
}

/// Serves a modelled operation exactly as the routing table would, then names the request
/// in the response's header — the protocol-independent report, for a service whose protocol
/// carries the id in the header alone.
///
/// The same wrapper serves every operation of such a service, and it is the one place each
/// of their responses passes through, so no operation can answer — with a success, a
/// modelled error, or a constraint refusal raised while it was being served — without
/// naming the request it answered.
pub struct RequestIdHeaderOperation;

impl ErasedOperation for RequestIdHeaderOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(serve_reporting(svc, req, ctx, with_request_id_header))
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // See [`RequestIdEnvelopeOperation::auth_schemes`]: the modelled operation this
        // wrapper delegates to is what decides, so wrapping cannot weaken its schemes.
        None
    }
}

/// Serves a modelled operation exactly as the routing table would, then names the request
/// in the `requestId` member of the response's JSON error body — the body-member report, for
/// a service whose own error envelope carries the id there.
///
/// The counterpart of [`RequestIdHeaderOperation`] for that report: the same wrapper serves
/// every operation of such a service and stores none of its own, which is the whole operation
/// half of the wiring for a service that serves its generated routing table unchanged. A
/// service that serves its operations wrapped composes
/// [`RequestIdReportingOperation::error_member`] around each of them instead.
pub struct RequestIdErrorMemberOperation;

impl ErasedOperation for RequestIdErrorMemberOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(serve_reporting(svc, req, ctx, with_request_id_error_member))
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // See [`RequestIdEnvelopeOperation::auth_schemes`]: the modelled operation this
        // wrapper delegates to is what decides, so wrapping cannot weaken its schemes.
        None
    }
}

/// Serves the modelled operation the service's own routing table names for a request exactly
/// as that table would, then reports the request id on whatever it answered — with the report
/// the SERVICE chose.
///
/// The three wrappers above each hold ONE of the reports this module states. A service whose
/// report is its own — because the FORM of the id, the MEMBER its error bodies spell it in, or
/// the set of places it states it, is that service's fact — holds this one instead and hands it
/// the same [`Report`] it hands
/// [`RequestIdEnvelopeDefinition::reporting`], so the response an operation produced and the
/// response no operation produced name their request the same way. It stores no operation of
/// its own: the one it delegates to is the one the service's routing table names for the
/// request, so the framework still owns the whole deserialize -> validate -> handle ->
/// serialize sequence and every status, code and message it answers with.
pub struct RequestIdReportedOperation {
    report: Report,
}

impl RequestIdReportedOperation {
    /// Serve every modelled operation of the service, reporting the request id with `report`.
    #[must_use]
    pub fn new(report: Report) -> Self {
        Self { report }
    }
}

impl ErasedOperation for RequestIdReportedOperation {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(serve_reporting(svc, req, ctx, self.report))
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // See [`RequestIdEnvelopeOperation::auth_schemes`]: the modelled operation this
        // wrapper delegates to is what decides, so wrapping cannot weaken its schemes.
        None
    }
}

/// Serves an operation a service already WRAPPED — behind its front door, say — and reports
/// the request id on whatever that wrapper answered.
///
/// The two wrappers above resolve the modelled operation out of the service's own generated
/// routing table, which is the whole wiring for a service that serves that table unchanged.
/// A service that serves its operations wrapped holds the wrapped operation itself, so it
/// composes this around that one instead: the report is applied to the response the wrapped
/// operation produced, whatever layer inside it answered — a success, a modelled error, a
/// refusal its front door worded, or a constraint refusal raised while it was being served.
pub struct RequestIdReportingOperation<O> {
    operation: O,
    report: Report,
}

impl<O: ErasedOperation> RequestIdReportingOperation<O> {
    /// Serve `operation` and report the id in the `requestId` member of its JSON error body
    /// ([`with_request_id_error_member`]).
    pub fn error_member(operation: O) -> Self {
        Self {
            operation,
            report: with_request_id_error_member,
        }
    }

    /// Serve `operation` and report the id in the response header alone
    /// ([`with_request_id_header`]).
    pub fn header(operation: O) -> Self {
        Self {
            operation,
            report: with_request_id_header,
        }
    }

    /// Serve `operation` and report the id with `report` — the constructor for a service
    /// whose report neither of the two above states, because the id it names a request by
    /// is its own (see [`Report`]).
    ///
    /// The operation counterpart of [`RequestIdEnvelopeDefinition::reporting`], for a
    /// service that serves some of its operations WRAPPED: handed the same `report` as that
    /// definition, so the response an operation produced and the response no operation
    /// produced name their request the same way.
    pub fn reported(operation: O, report: Report) -> Self {
        Self { operation, report }
    }

    /// The operation this report wraps, for a service that asks it a question of its own
    /// before handing it the request (whether its front door claims that request, say).
    ///
    /// Wrapping an operation to report the request id must not take away what the service
    /// could ask the operation before, which is why the wrapped operation is reachable
    /// rather than swallowed; it is handed out as a BORROW, so nothing outside can serve a
    /// request through it unreported.
    pub fn inner(&self) -> &O {
        &self.operation
    }
}

impl<O: ErasedOperation> ErasedOperation for RequestIdReportingOperation<O> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move { (self.report)(self.operation.run(svc, req, ctx).await) })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // The wrapped operation's own schemes: wrapping must not change which schemes
        // sign it.
        self.operation.auth_schemes()
    }
}

/// A service's model-derived half with the framework-level error renderers wrapped, so a
/// routing failure, a transport failure and a pre-dispatch refusal answer in the same
/// envelope every other error of that service does.
///
/// Identity, the routing table and the protocol come from the wrapped definition
/// untouched — nothing model-derived is restated here. Generic over that definition rather
/// than over an engine, so this crate names no service's generated type.
///
/// The report is chosen by the constructor: [`new`](Self::new) for an `awsQuery` service
/// (the envelope element and the header) and [`header_only`](Self::header_only) for every
/// other protocol (the header alone, on every response).
pub struct RequestIdEnvelopeDefinition<D> {
    inner: D,
    report: Report,
}

impl<D: ErasedServiceDefinition> RequestIdEnvelopeDefinition<D> {
    /// Wrap a service's generated definition, reporting the id the way `awsQuery` does:
    /// the `<RequestId>` element in the error envelope, and the header beside it.
    pub fn new(inner: D) -> Self {
        Self {
            inner,
            report: with_request_id,
        }
    }

    /// Wrap a service's generated definition, reporting the id in the header alone — the
    /// whole report for a protocol with no envelope element for it.
    pub fn header_only(inner: D) -> Self {
        Self {
            inner,
            report: with_request_id_header,
        }
    }

    /// Wrap a service's generated definition, reporting the id in the `requestId` member of
    /// the JSON error body ([`with_request_id_error_member`]) — the report for a service
    /// whose own error envelope carries it there.
    pub fn error_member(inner: D) -> Self {
        Self {
            inner,
            report: with_request_id_error_member,
        }
    }

    /// Wrap a service's generated definition, reporting the id in the header of every
    /// response AND in the `requestId` member of a JSON error body, under one id
    /// ([`with_request_id_header_and_error_member`]) — the report for a service that states it
    /// in both places.
    pub fn header_and_error_member(inner: D) -> Self {
        Self {
            inner,
            report: with_request_id_header_and_error_member,
        }
    }

    /// Wrap a service's generated definition, reporting the id with `report` — the
    /// constructor for a service whose report none of the three above states, because the
    /// FORM of the id it names a request by is its own (see [`Report`]).
    ///
    /// The same `report` the service hands its operation wrapper, so the response an
    /// operation produced and the response no operation produced name their request the
    /// same way.
    pub fn reporting(inner: D, report: Report) -> Self {
        Self { inner, report }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which a service claims it for [`RequestIdEnvelopeOperation`],
    /// [`RequestIdHeaderOperation`], [`RequestIdErrorMemberOperation`] or
    /// [`RequestIdReportedOperation`], so a request
    /// naming no modelled operation (an `Action` the model does not declare, a method and
    /// path the model does not bind) stays the framework's (or the service's own) routing
    /// failure.
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.inner
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.inner.routing_table().get(key))
            .is_some()
    }
}

impl<D: ErasedServiceDefinition> ErasedServiceDefinition for RequestIdEnvelopeDefinition<D> {
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
        (self.report)(self.inner.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        (self.report)(self.inner.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        (self.report)(self.inner.refusal_response(error, chosen, req))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.inner.wire_protocol(req)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The element lands as the last child of the envelope — after `</Error>`, inside
    /// `</ErrorResponse>` — whether or not the root carries a service namespace.
    #[test]
    fn request_id_is_the_last_child_of_the_envelope() {
        for case in [
            "<?xml version=\"1.0\"?><ErrorResponse><Error><Type>Sender</Type>\
             <Code>TargetGroupNotFound</Code><Message>no</Message></Error></ErrorResponse>",
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"ns\"><Error><Type>Sender</Type>\
             <Code>TargetGroupNotFound</Code><Message>no</Message></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected <RequestId> as the envelope's last child, got \
                 {rewritten:?}"
            );
        }
    }

    /// The envelope's own children decide whether an id is there already: an `<Error>`
    /// whose `<Message>` reproduces the element's name — the renderer builds that message
    /// from the request's own action and version — still gets the envelope's id.
    #[test]
    fn text_inside_the_error_does_not_suppress_the_envelope_id() {
        for case in [
            "<ErrorResponse><Error><Type>Sender</Type><Code>InvalidAction</Code>\
             <Message>Could not find operation <RequestId>x</RequestId> for version 1\
             </Message></Error></ErrorResponse>",
            "<ErrorResponse><Error><Code>TargetGroupNotFound</Code>\
             <RequestId>a modelled member</RequestId></Error></ErrorResponse>",
        ] {
            let rewritten =
                add_request_id(case, "11112222-3333-4444-5555-666677778888").expect("rewritten");
            assert!(
                rewritten.ends_with(
                    "</Error><RequestId>11112222-3333-4444-5555-666677778888</RequestId>\
                     </ErrorResponse>"
                ),
                "case {case:?}: expected the envelope's own <RequestId> to be added, got \
                 {rewritten:?}"
            );
        }
    }

    /// Nothing to add: a document that already carries a request id, and one that is not
    /// an error envelope at all, are both left alone.
    #[test]
    fn nothing_is_added_twice_and_nothing_else_is_touched() {
        for case in [
            "<ErrorResponse><Error/><RequestId>already</RequestId></ErrorResponse>",
            "<DescribeTargetGroupsResponse><DescribeTargetGroupsResult/><ResponseMetadata>\
             <RequestId>a</RequestId></ResponseMetadata></DescribeTargetGroupsResponse>",
            "<DescribeTargetGroupsResponse><DescribeTargetGroupsResult/>\
             </DescribeTargetGroupsResponse>",
        ] {
            assert!(
                add_request_id(case, "fresh").is_none(),
                "case {case:?}: expected no rewrite"
            );
        }
    }

    /// The id the envelope already states is read from the SIBLING position, so the header
    /// written beside such a document reports that id rather than a second one — and text
    /// inside `<Error>` is not mistaken for it.
    #[test]
    fn the_envelopes_own_request_id_is_read_from_the_sibling_position() {
        for (case, body, expected) in [
            (
                "an envelope that states its own id",
                "<ErrorResponse><Error><Code>c</Code></Error>\
                 <RequestId>11112222-3333-4444-5555-666677778888</RequestId></ErrorResponse>",
                Some("11112222-3333-4444-5555-666677778888"),
            ),
            (
                "a sibling value that is not a request id",
                "<ErrorResponse><Error><Code>c</Code></Error><RequestId>not an id</RequestId>\
                 </ErrorResponse>",
                None,
            ),
            (
                "an id inside the error, which is not the envelope's",
                "<ErrorResponse><Error><Code>c</Code><RequestId>a member</RequestId></Error>\
                 </ErrorResponse>",
                None,
            ),
            (
                "a successful document, which is not this module's business",
                "<DescribeTargetGroupsResponse><ResponseMetadata><RequestId>s</RequestId>\
                 </ResponseMetadata></DescribeTargetGroupsResponse>",
                None,
            ),
        ] {
            assert_eq!(
                envelope_request_id(body),
                expected,
                "case {case:?}: expected {expected:?} as the envelope's own id"
            );
        }
    }

    /// The minted id has the shape of an AWS request id, and two of them differ — the
    /// invariant a correlation id carries.
    #[test]
    fn minted_ids_are_well_formed_and_distinct() {
        let ids: Vec<String> = (0..8).map(|_| new_request_id()).collect();
        for id in &ids {
            assert!(
                is_request_id(id),
                "case {id:?}: expected the 8-4-4-4-12 lowercase-hex form of a request id"
            );
        }
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            ids.len(),
            "expected 8 distinct request ids, got {ids:?}"
        );
    }

    /// What the form test accepts and refuses: only the 8-4-4-4-12 lowercase-hex shape,
    /// which is what keeps anything else out of the response's request id.
    #[test]
    fn only_the_request_id_form_is_accepted() {
        for (case, id, expected) in [
            ("the minted form", "11112222-3333-4444-5555-666677778888", true),
            ("every hex digit", "abcdef01-2345-6789-abcd-ef0123456789", true),
            ("uppercase hex", "AAAA2222-3333-4444-5555-666677778888", false),
            ("a non-hex letter", "1111222g-3333-4444-5555-666677778888", false),
            ("a group too short", "1111222-3333-4444-5555-666677778888", false),
            ("a group too long", "111122223-3333-4444-5555-66667777888", false),
            ("a sixth group", "11112222-3333-4444-5555-666677778888-9999", false),
            ("no groups at all", "111122223333444455556666777788889999", false),
            ("prose", "not an id", false),
            ("nothing", "", false),
        ] {
            assert_eq!(
                is_request_id(id),
                expected,
                "case {case:?}: expected is_request_id({id:?}) == {expected}"
            );
        }
    }

    /// One response, as the header-only report hands it on.
    fn reported(status: u16, body: &'static str) -> RtResponse {
        with_request_id_header(RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(status)
                .expect("the test status is valid"),
            SdkBody::from(body),
        ))
    }

    /// The protocol-independent report names the request on EVERY response — a success as
    /// much as a failure, with a body or without one — and changes nothing else about it.
    #[test]
    fn the_header_report_names_the_request_on_every_response() {
        for (case, status, body) in [
            ("a restJson1 success", 200u16, "{\"tags\":{}}"),
            ("an empty-bodied success", 204, ""),
            ("a restJson1 error document", 404, "{\"message\":\"no\"}"),
            ("a body that is no document at all", 501, "not a document"),
        ] {
            let reported = reported(status, body);
            let stated = reported
                .headers()
                .get(REQUEST_ID_HEADER)
                .expect("the report names the request");
            assert!(
                is_request_id(stated),
                "case {case:?}: expected the form of a request id, got {stated:?}"
            );
            assert_eq!(
                reported.status().as_u16(),
                status,
                "case {case:?}: the status must not change"
            );
            assert_eq!(
                reported.body().bytes(),
                Some(body.as_bytes()),
                "case {case:?}: the body must not change"
            );
        }
    }

    /// The report is per CALL and idempotent: two responses never name the same request,
    /// and a response that already names one keeps the id it has.
    #[test]
    fn the_header_report_is_fresh_per_call_and_idempotent() {
        let first = reported(200, "{}");
        let second = reported(200, "{}");
        let (first, second) = (
            first.headers().get(REQUEST_ID_HEADER).map(str::to_string),
            second.headers().get(REQUEST_ID_HEADER).map(str::to_string),
        );
        assert_ne!(
            first, second,
            "expected a fresh id per call, got {first:?} twice"
        );

        let mut already = RtResponse::new(
            aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
            SdkBody::from("{}"),
        );
        let stated = "11112222-3333-4444-5555-666677778888";
        let _ = already.headers_mut().try_insert(REQUEST_ID_HEADER, stated);
        let kept = with_request_id_header(already);
        assert_eq!(
            kept.headers().get(REQUEST_ID_HEADER),
            Some(stated),
            "a response that already names its request keeps that id"
        );
    }

    /// The STATED report names the id its caller holds, on every response, and keeps the id
    /// a response already names — so a service whose call already knows which request it
    /// answered reports that one id and no second one.
    ///
    /// The rows span the responses it is applied to: a success, an empty-bodied success and
    /// an error document.
    #[test]
    fn the_stated_report_names_the_id_it_was_given() {
        let stated = "11112222-3333-4444-5555-666677778888";
        for (case, status, body) in [
            ("a success", 200u16, "{\"ran\":true}"),
            ("an empty-bodied success", 204, ""),
            ("an error document", 429, "{\"message\":\"Rate Exceeded.\"}"),
        ] {
            let response = RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status)
                    .expect("the test status is valid"),
                SdkBody::from(body),
            );
            let named = with_stated_request_id_header(response, stated);
            assert_eq!(
                named.headers().get(REQUEST_ID_HEADER),
                Some(stated),
                "case {case:?}: expected the response to name the id it was given"
            );
            assert_eq!(
                named.body().bytes(),
                Some(body.as_bytes()),
                "case {case:?}: the body must not change"
            );
            // And the header-only report leaves that id alone rather than minting a second.
            let kept = with_request_id_header(named);
            assert_eq!(
                kept.headers().get(REQUEST_ID_HEADER),
                Some(stated),
                "case {case:?}: a response that already names its request keeps that id"
            );
        }
    }

    /// One response names ONE request: the id a rendered document already states is the id
    /// read back off it, and a document that states none is reported one — so a layer
    /// reading the id and the caller can never see two values for one call.
    ///
    /// The rows span the forms a response states it in: an `awsQuery` success's
    /// `ResponseMetadata`, the `ec2Query` dialect's lower-case element, an error envelope
    /// that already carries one, an error envelope that carries none (reported here), and a
    /// document of a protocol with no element for it at all.
    #[test]
    fn a_response_names_one_request_whichever_layer_reports_it() {
        let stated = "11112222-3333-4444-5555-666677778888";
        for (case, body, expected) in [
            (
                "an awsQuery success",
                format!(
                    "<CreateTargetGroupResponse><CreateTargetGroupResult/><ResponseMetadata>\
                     <RequestId>{stated}</RequestId></ResponseMetadata>\
                     </CreateTargetGroupResponse>"
                ),
                Some(stated),
            ),
            (
                "an ec2Query success",
                format!("<DescribeTagsResponse><requestId>{stated}</requestId></DescribeTagsResponse>"),
                Some(stated),
            ),
            (
                "an error envelope that states its own id",
                format!(
                    "<ErrorResponse><Error><Code>TargetGroupNotFound</Code></Error>\
                     <RequestId>{stated}</RequestId></ErrorResponse>"
                ),
                Some(stated),
            ),
            (
                "an error envelope that states none",
                "<ErrorResponse><Error><Code>TargetGroupNotFound</Code></Error></ErrorResponse>"
                    .to_string(),
                None,
            ),
            ("a document of another protocol", "{\"tags\":{}}".to_string(), None),
        ] {
            let built = || {
                RtResponse::new(
                    aws_smithy_runtime_api::http::StatusCode::try_from(200u16).expect("200"),
                    SdkBody::from(body.clone()),
                )
            };
            assert_eq!(
                reported_request_id(&built()).as_deref(),
                expected,
                "case {case:?}: expected the stated id {expected:?}"
            );
            let (named, id) = named_request(built());
            assert!(
                is_request_id(&id),
                "case {case:?}: expected a well-formed id, got {id:?}"
            );
            if let Some(expected) = expected {
                assert_eq!(
                    id, expected,
                    "case {case:?}: a response that states an id keeps it, got {id:?}"
                );
            }
            assert_eq!(
                reported_request_id(&named).as_deref(),
                Some(id.as_str()),
                "case {case:?}: the reported response must name the id {id:?} back"
            );
            // Idempotent: reporting again finds the same id and changes nothing.
            let (again, second) = named_request(named);
            assert_eq!(
                second, id,
                "case {case:?}: expected the same id on a second report, got {second:?}"
            );
            let _ = again;
        }
    }

    /// RULE: a FAILURE whose body is a JSON object names the request it answers in that
    /// object's `requestId` member, and nothing else about the response changes.
    ///
    /// The rows span what the report generalises over: which shape the body names, whether
    /// the object carries members at all, a body that is not a JSON object, a body that
    /// already names its request, and a success.
    #[test]
    fn a_json_failure_names_the_request_in_its_body() {
        // (case, status, the rendered body, whether the member is added)
        let rows = [
            (
                "a refusal",
                400u16,
                r#"{"__type":"InvalidParameterValueException","message":"no"}"#,
                true,
            ),
            ("a modelled error", 404, r#"{"message":"gone"}"#, true),
            ("an empty object", 400, "{}", true),
            ("a server fault", 500, r#"{"__type":"ServiceException"}"#, true),
            ("an XML envelope", 400, "<Error><Code>x</Code></Error>", false),
            (
                "a body that already names it",
                400,
                r#"{"message":"no","requestId":"11112222-3333-4444-5555-666677778888"}"#,
                false,
            ),
            ("a success", 200, r#"{"requestId":null}"#, false),
        ];

        for (case, status, body, adds) in rows {
            let answered = with_request_id_error_member(RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(body),
            ));
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {case:?}: the status must not move"
            );
            let answered_body = std::str::from_utf8(answered.body().bytes().expect("bytes"))
                .expect("utf-8")
                .to_string();
            if !adds {
                assert_eq!(
                    answered_body, body,
                    "case {case:?}: expected the body to survive byte for byte, got \
                     {answered_body:?}"
                );
                continue;
            }
            let document: serde_json::Value =
                serde_json::from_str(&answered_body).expect("a JSON object");
            let reported = document
                .get(REQUEST_ID_MEMBER)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            assert!(
                is_request_id(reported),
                "case {case:?}: expected a well-formed id in {REQUEST_ID_MEMBER}, got \
                 {answered_body:?}"
            );
            // Every member the renderer wrote is still there, and reporting again adds no
            // second id.
            let original: serde_json::Value = serde_json::from_str(body).expect("a JSON object");
            for (member, value) in original.as_object().expect("an object") {
                assert_eq!(
                    document.get(member),
                    Some(value),
                    "case {case:?}: expected the member {member:?} to survive"
                );
            }
            let again = with_request_id_error_member(RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(answered_body.clone()),
            ));
            assert_eq!(
                std::str::from_utf8(again.body().bytes().expect("bytes")).expect("utf-8"),
                answered_body,
                "case {case:?}: expected a second report to change nothing"
            );
        }
    }

    /// RULE: the MEMBER the id is named in is the caller's, and everything else about the
    /// report is unchanged — the id's form, the members the renderer wrote, and the
    /// idempotence that keeps a second report from adding a second id.
    ///
    /// The rows span what the choice generalises over: the default spelling, a capitalized
    /// one (what a service whose modelled members are capitalized carries), a member whose
    /// text needs JSON escaping, and a body that already names the chosen member while
    /// carrying the default one.
    #[test]
    fn the_error_member_report_names_the_request_in_the_member_it_was_given() {
        // (case, the member to name, the rendered body, whether the member is added)
        let rows = [
            ("the default spelling", REQUEST_ID_MEMBER, r#"{"message":"no"}"#, true),
            ("a capitalized member", "RequestId", r#"{"Code":"NotFound","Message":"no"}"#, true),
            ("a member needing escaping", r#"Req"Id"#, r#"{"message":"no"}"#, true),
            ("an empty object", "RequestId", "{}", true),
            (
                "a body already naming the chosen member",
                "RequestId",
                r#"{"message":"no","RequestId":"11112222-3333-4444-5555-666677778888"}"#,
                false,
            ),
            (
                "a body naming only the DEFAULT member",
                "RequestId",
                r#"{"message":"no","requestId":"11112222-3333-4444-5555-666677778888"}"#,
                true,
            ),
            ("a success", "RequestId", r#"{"message":"fine"}"#, false),
        ];

        for (case, member, body, adds) in rows {
            let status = if case == "a success" { 200u16 } else { 400 };
            let answered = with_request_id_error_member_named(
                RtResponse::new(
                    aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                    SdkBody::from(body),
                ),
                member,
            );
            let answered_body = std::str::from_utf8(answered.body().bytes().expect("bytes"))
                .expect("utf-8")
                .to_string();
            if !adds {
                assert_eq!(
                    answered_body, body,
                    "case {case:?}: expected the body to survive byte for byte, got \
                     {answered_body:?}"
                );
                continue;
            }
            let document: serde_json::Value = serde_json::from_str(&answered_body)
                .unwrap_or_else(|e| panic!("case {case:?}: expected JSON, got {answered_body:?} ({e})"));
            let reported = document
                .get(member)
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            assert!(
                is_request_id(reported),
                "case {case:?}: expected a well-formed id in {member:?}, got {answered_body:?}"
            );
            let original: serde_json::Value = serde_json::from_str(body).expect("a JSON object");
            for (name, value) in original.as_object().expect("an object") {
                assert_eq!(
                    document.get(name),
                    Some(value),
                    "case {case:?}: expected the member {name:?} to survive"
                );
            }
            // Idempotent under the SAME member: a second report adds no second id.
            let again = with_request_id_error_member_named(
                RtResponse::new(
                    aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                    SdkBody::from(answered_body.clone()),
                ),
                member,
            );
            assert_eq!(
                std::str::from_utf8(again.body().bytes().expect("bytes")).expect("utf-8"),
                answered_body,
                "case {case:?}: expected a second report to change nothing"
            );
        }
    }

    /// RULE: the composed report states ONE id in both places a JSON service reports it — the
    /// header of every response and the `requestId` member of a JSON error body.
    ///
    /// The rows span what it generalises over: a success (the header alone — its body is the
    /// modelled output), failures of three classes, and a body that is not a JSON object.
    #[test]
    fn the_composed_report_states_one_id_in_the_header_and_the_body() {
        // (case, status, the rendered body, whether the body gains the member)
        let rows = [
            ("a success", 200u16, r#"{"arn":"a"}"#, false),
            ("a refusal", 400, r#"{"message":"no"}"#, true),
            (
                "a modelled error",
                404,
                r#"{"__type":"ResourceNotFoundException","message":"gone"}"#,
                true,
            ),
            ("a server fault", 500, r#"{"__type":"ServiceException"}"#, true),
            ("an XML envelope", 400, "<Error><Code>x</Code></Error>", false),
        ];

        for (case, status, body, adds) in rows {
            let answered = with_request_id_header_and_error_member(RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(body),
            ));
            assert_eq!(
                answered.status().as_u16(),
                status,
                "case {case:?}: the status must not move"
            );
            let stated = answered
                .headers()
                .get(REQUEST_ID_HEADER)
                .unwrap_or_default()
                .to_string();
            assert!(
                is_request_id(&stated),
                "case {case:?}: expected the header to name the request, got {stated:?}"
            );
            let answered_body = std::str::from_utf8(answered.body().bytes().expect("bytes"))
                .expect("utf-8")
                .to_string();
            if !adds {
                assert_eq!(
                    answered_body, body,
                    "case {case:?}: expected the body to survive byte for byte, got \
                     {answered_body:?}"
                );
                continue;
            }
            let document: serde_json::Value =
                serde_json::from_str(&answered_body).expect("a JSON object");
            assert_eq!(
                document
                    .get(REQUEST_ID_MEMBER)
                    .and_then(serde_json::Value::as_str),
                Some(stated.as_str()),
                "case {case:?}: expected the body to name the SAME id the header does, got \
                 {answered_body:?}"
            );
            // Idempotent: a second report over the rewritten body adds no second id.
            let again = with_request_id_header_and_error_member(RtResponse::new(
                aws_smithy_runtime_api::http::StatusCode::try_from(status).expect("a status"),
                SdkBody::from(answered_body.clone()),
            ));
            assert_eq!(
                std::str::from_utf8(again.body().bytes().expect("bytes")).expect("utf-8"),
                answered_body,
                "case {case:?}: expected a second report to change nothing"
            );
        }
    }

    /// RULE: a request served under one decided id is answered under THAT id — the id a layer
    /// inside the walk reads is the one the answer reports, wherever its protocol states it.
    ///
    /// The rows span what the rule generalises over: the `awsQuery` error envelope (the element
    /// and the header), a binary-protocol error body (the header alone), a response a layer
    /// already named its request in (which keeps the id it has), and a success document that
    /// states its own id.
    #[tokio::test]
    async fn a_request_is_answered_under_the_id_decided_for_it() {
        // (case, status, the rendered body, whether the answer names the decided id)
        let rows = [
            (
                "an awsQuery error envelope",
                400u16,
                "<ErrorResponse><Error><Code>ValidationError</Code><Message>no</Message>\
                 </Error></ErrorResponse>"
                    .to_string(),
                true,
            ),
            (
                "a JSON error body",
                400,
                r#"{"__type":"ValidationException","message":"no"}"#.to_string(),
                true,
            ),
            (
                "a success document naming its own request",
                200,
                "<PutMetricAlarmResponse><ResponseMetadata><RequestId>\
                 11112222-3333-4444-5555-666677778888</RequestId></ResponseMetadata>\
                 </PutMetricAlarmResponse>"
                    .to_string(),
                false,
            ),
        ];

        for (case, status, body, names_decided) in rows {
            let answered = answered_under_one_request_id(
                async {
                    let decided = served_request_id().unwrap_or_default();
                    assert!(
                        is_request_id(&decided),
                        "case {case:?}: a layer inside the walk must read the decided id, got \
                         {decided:?}"
                    );
                    RtResponse::new(
                        aws_smithy_runtime_api::http::StatusCode::try_from(status)
                            .expect("a status"),
                        SdkBody::from(format!("{body}<!--{decided}-->")),
                    )
                },
                named_request_under,
            )
            .await;
            let reported = reported_request_id(&answered).unwrap_or_default();
            let rendered = std::str::from_utf8(answered.body().bytes().expect("bytes"))
                .expect("utf-8")
                .to_string();
            assert!(
                is_request_id(&reported),
                "case {case:?}: expected the answer to name the request, got {reported:?}"
            );
            assert_eq!(
                rendered.contains(&format!("<!--{reported}-->")),
                names_decided,
                "case {case:?}: expected the id the inner layer read to be the one reported: \
                 {names_decided}, got {rendered:?} reported as {reported:?}"
            );
        }
    }

    /// Outside a served request — a response rendered by a layer no scope was opened around —
    /// there is no decided id to read, rather than a panic or an empty one.
    #[tokio::test]
    async fn nothing_is_decided_outside_a_served_request() {
        assert_eq!(
            served_request_id(),
            None,
            "expected no decided id outside a served request"
        );
    }
}
