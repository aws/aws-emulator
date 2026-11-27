// SPDX-License-Identifier: CC0-1.0

//! CloudFront's error envelope: the one place every failure this service answers is
//! finished off before it goes on the wire.
//!
//! CloudFront is `restXml`, and its error document is
//! `<ErrorResponse><Error><Type/><Code/><Message/></Error><RequestId/></ErrorResponse>`:
//! the `Error` element states WHAT failed, and the `RequestId` BESIDE it names the
//! request it failed for, so a caller can correlate the failure with the service's own
//! record of it. `RequestId` is a property of the ENVELOPE, not a member of any error
//! shape the model declares, so the protocol serializer — which renders an error from
//! its shape's members — cannot put it there. [`ErrorEnvelope`] does, for every response
//! this service answers: a modelled error a handler raised, and a refusal the request
//! boundary made before any handler ran, alike.
//!
//! Where the element BELONGS is the vendored wrapped-restXml renderer
//! (`aws_smithy_schema_server::rest_xml`'s `serialize_error_wrapped`, which already
//! writes a `RequestId` on the `noErrorWrapping` path beside the `x-amz-request-id`
//! header mirroring it). That tree is read-only from here, and the framework's
//! pre-dispatch renderer for a boundary refusal lives in it too, so the element is
//! added to the finished document instead — delete both halves of this seam once the
//! vendored renderer carries it. Two sibling services splice the same element into their
//! own (awsQuery) envelope the same way —
//! `service-com-amazonaws-cloudformation-cloudformation`'s `query_error_envelope` and
//! `service-com-amazonaws-sns-amazonsimplenotificationservice`'s `service` — so the three
//! are named here together: the splice is per-service today because the envelope and the
//! renderer that writes it are, and hoisting all three belongs with the vendored change
//! that makes them unnecessary rather than with this fix.
//!
//! EVERY failure this service answers passes through here: a modelled error and a
//! boundary refusal reach [`ErrorEnvelope`] on the dispatch path, and the three the
//! router renders OUTSIDE it — a request that names no modelled operation, a
//! transport-level failure, and a refusal the router makes before any operation runs —
//! reach [`CloudFrontDefinition`], which wraps the generated definition's own renderers.
//!
//! The same seam is where a refusal the boundary states in the FRAMEWORK's vocabulary is
//! restated in CloudFront's own. The framework refuses a request it cannot read into the
//! operation's input with `400 ValidationException` and the codec's own text;
//! `ValidationException` is not a shape this service's model declares at all, so a client
//! branching on the error code falls through to a generic failure and never reaches its
//! input-correction path. Four such refusals have a modelled CloudFront answer, and each
//! is read off the REQUEST and the refusal rather than off the operation:
//!
//! * a `MaxItems` the parameter's integer type cannot take is `InvalidArgument` (400),
//!   stating the constraint on the parameter — the same refusal, in the same words, a
//!   list handler makes for a value that parses but names no page
//!   ([`crate::state::MAX_ITEMS_INVALID_MESSAGE`]) — and only where the refusal shows
//!   that value as the one it refused, so a request refused for something else keeps
//!   the cause its caller has to fix;
//! * a value outside the SET its member admits is no value of that member at all: the value
//!   set is part of the schema this API publishes for the request, so a request naming a
//!   value outside it is one this API cannot read, and the answer is `MalformedXML` (400)
//!   under the sentence the boundary already wrote, which names the member and the value
//!   set it broke. The BINDING the value travelled in does not change that — a query
//!   parameter, a member of the request's document and a member nested inside its payload
//!   are all the same schema — so the condition reads the constraint the refusal reports
//!   and nothing else. Neither does the LAYER that read the set: a set the model documents
//!   in prose but declares nowhere (the event types a function association admits) is
//!   checked by the operation, which states the same sentence, and the same code answers
//!   it. The value set that sentence lists is a property of the SHAPE or of the member's
//!   documentation rather than of the request, so it is listed in the order this service
//!   publishes it ([`STATED_VALUE_ORDERS`]);
//! * a body-bearing request that carries NO body cannot have a payload at all, so the
//!   null member the boundary reports is one nested inside that payload and the member
//!   that is really absent is the payload itself: the answer is `MalformedXML` (400)
//!   naming that payload member. A refusal naming a member nested in nothing is about a
//!   member bound outside the body, and keeps the boundary's own wording;
//! * every OTHER constraint the boundary reports on a member nested inside the payload — a
//!   member that is absent, too short or long, outside its range or outside its pattern — is
//!   a request document this API cannot read just as surely, so it too is `MalformedXML`
//!   (400) under the sentence the boundary wrote. Two parts of that sentence this service
//!   states itself: a PATTERN, which it reports as the pattern it enforces rather than as the
//!   one the model spells ([`stated_pattern`]), and the two numbers a `@range` clause states —
//!   the value submitted and the bound declared — which are rendered in the member's own
//!   numeric type, so a floating-point member's carry a decimal fraction
//!   ([`with_fractional_bounds`]).
//!
//! All of them leave every other refusal exactly as the layer that made it rendered it: a
//! request the boundary read, and an operation's refusal that reports another rule, are
//! never second-guessed here.
//!
//! One restatement is about an OPERATION's own refusal rather than the boundary's: a value
//! outside a constraint the model publishes for the SHAPE that value has to be, where the
//! member it travelled in declares none, is a value this API cannot read exactly as one the
//! boundary caught is — an operation states it in the same whole frame, the count of violations
//! followed by the constraint one of them broke — so it is answered under the same
//! `MalformedXML` (400) and keeps the sentence the operation wrote
//! ([`refused_over_a_published_constraint`]).
//!
//! One refusal is made HERE rather than reworded: a request whose DOCUMENT is larger than
//! this API reads is answered with the transport's `413` and no document at all
//! ([`MAX_REQUEST_BODY_BYTES`]). Nothing parsed such a request far enough to name an
//! operation, so there is no modelled error to state it in, and an SDK has only the status to
//! classify it by. The emulator's shared serve path has a ceiling of its own, larger and for
//! every service alike, which is what bounds what the host buffers; this one is this API's
//! own, and decides the ANSWER.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::{missing_member_message, xml_escape};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    percent_decode, BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext,
    OperationRoutingTable, ServerHttpProtocol, Service, ServiceIdentity,
};

/// This service's `@xmlNamespace`, which its error envelope carries exactly as its
/// success documents do (`smithy_query(query="service_info")`:
/// `xmlNamespace.uri = http://cloudfront.amazonaws.com/doc/2020-05-31/`).
pub const XML_NAMESPACE: &str = "http://cloudfront.amazonaws.com/doc/2020-05-31/";

/// The envelope element the request-correlation id sits beside `Error` in.
const REQUEST_ID_ELEMENT: &str = "RequestId";

/// The envelope root, whose close is where a missing [`REQUEST_ID_ELEMENT`] is added.
const ERROR_RESPONSE_OPEN: &str = "<ErrorResponse";
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// The error code the framework names a request it refused before any handler ran.
/// Not a shape this service's model declares, which is why a response carrying it is
/// a refusal to restate rather than a CloudFront error.
const FRAMEWORK_REFUSAL_CODE: &str = "ValidationException";

/// The status every refusal restated here answers with: the model gives `InvalidArgument`
/// HTTP 400, and an unreadable body is a client error too.
const REFUSED: u16 = 400;

/// `<Error><Type>`: a 4xx is the caller's fault, which restXml words `Sender`.
const SENDER: &str = "Sender";

/// The wire code for an argument the operation models but cannot take
/// (`com.amazonaws.cloudfront#InvalidArgument`, HTTP 400).
const INVALID_ARGUMENT: &str = "InvalidArgument";

/// The wire code CloudFront's front door names a request that matches no action of this API
/// ([`unknown_action`]).
///
/// The model declares no shape for it — it is the FRONT DOOR's refusal rather than any
/// operation's modelled error, raised before an action is named at all — and the recorded
/// responses in this project's corpus carry `<Code>InvalidAction</Code>` under
/// [`UNKNOWN_ACTION`] for a request whose path matches no route of this API.
const INVALID_ACTION: &str = "InvalidAction";

/// The status that refusal answers with: the action was not FOUND, which is a 404 and a client
/// error, never a server fault.
const UNKNOWN_ACTION: u16 = 404;

/// The wire code CloudFront answers a request whose XML body it cannot read — including
/// a required payload that is not there at all.
///
/// The model declares no shape for it (`smithy_query(query="shape",shape="MalformedXML")`
/// finds none), but the service answers it: the recorded responses in this project's
/// corpus carry `<Code>MalformedXML</Code>` for a `CreateDistribution` with an empty
/// body. The alternative the model does declare, `MissingBody`, describes a request with
/// no body AND no content-type; what a client branches on here is the code the service
/// sends, so the code the service sends is what this answers.
const MALFORMED_XML: &str = "MalformedXML";

/// The wire code and status of the refusal the AUTHORIZATION layer answers an unauthorized
/// request with, which is the shape this model declares for it on every operation that
/// declares one (`com.amazonaws.cloudfront#AccessDenied`, HTTP 403).
const ACCESS_DENIED: &str = "AccessDenied";
const FORBIDDEN: u16 = 403;

/// The envelope an unauthorized request is refused in: `403 AccessDenied` under the sentence
/// IAM states the refusal with ([`super::authorization`]).
///
/// Rendered here rather than returned by a handler because the decision is taken in FRONT of
/// the operation, where no handler has run — and rendered through this module's one error
/// renderer, so it carries the same `<Type>`, namespace and `RequestId` every other failure
/// of this service carries.
pub fn access_denied(message: &str) -> RtResponse {
    error_response(FORBIDDEN, ACCESS_DENIED, message)
}

/// The query parameter every CloudFront list operation binds its page size to
/// (`@httpQuery("MaxItems")` over an `integer`, from the model's list operations).
const MAX_ITEMS_PARAM: &str = "MaxItems";

/// The status a request whose payload is larger than this API reads answers with: the
/// transport's own "payload too large".
const PAYLOAD_TOO_LARGE: u16 = 413;

/// The bytes one invalidation path spends on the value it carries, at most: an invalidation
/// path names an object of the distribution, and an object key carries at most 1,024 bytes
/// (Amazon S3's documented key length, which is what an origin object is named by), plus the
/// leading slash the path begins with.
const LONGEST_INVALIDATION_PATH_BYTES: usize = 1 + 1024;

/// The bytes one `<Path>` element spends on its own tags, beside the value inside them.
const PATH_ELEMENT_BYTES: usize = "<Path></Path>".len();

/// The most bytes a request DOCUMENT this API reads may carry.
///
/// A document past it is refused with the transport's `413` and NOTHING else — no error
/// document, because nothing parsed the request far enough to name an operation or one of its
/// modelled errors, so an SDK has only the HTTP status to classify it by. That is the answer
/// this API gives a request document far over the ceiling, and it is a DIFFERENT condition
/// from a modelled refusal that a document the service DID read describes something it cannot
/// serve (an invalidation batch naming more objects than one request may, which is
/// `BatchTooLarge` carrying its own `Code` under the same status).
///
/// This is a fidelity rule about WHICH answer the API gives, not the bound that protects the
/// host: that one is the emulator's shared serve path, which buffers no more than its own
/// ceiling and answers the same bare `413` past it, for every service alike. This ceiling is
/// SMALLER because it is this API's own, and it is read off the document the request already
/// carries rather than off anything the request states about itself.
///
/// AWS publishes no request-document ceiling for this API, so the number is DERIVED from this
/// service's own published quotas instead of chosen: the largest document any of them admits
/// is the biggest invalidation batch — [`crate::state::MAX_INVALIDATION_PATHS`] paths, each at
/// its longest — and every other document the quotas admit is orders of magnitude under that
/// (a function's code is 10 KiB, a tag set 50 pairs, a tenant 5 domains). So a document whose
/// elements all sit inside the published bounds is always readable, and only one far past them
/// is refused unread.
pub const MAX_REQUEST_BODY_BYTES: usize =
    crate::state::MAX_INVALIDATION_PATHS * (LONGEST_INVALIDATION_PATH_BYTES + PATH_ELEMENT_BYTES);

/// How the request boundary states the constraint an ENUM member carries (`Member must
/// satisfy enum value set: [...]`), the one refusal that says the request named a value the
/// model's enum does not admit.
///
/// It identifies the refusal by the CONSTRAINT it reports rather than by the member or the
/// operation, so it holds for every enum-constrained member of every operation this service
/// serves.
const ENUM_VALUE_SET_MARKER: &str = "Member must satisfy enum value set:";

/// How a refusal states the constraint of a value set the member's own shape does NOT declare
/// (`Member must satisfy value set: [...]`) — the set a member's documentation admits where
/// the shape it targets admits more (a CloudFront function association's event type, whose
/// `EventType` enum carries the origin-facing events a Lambda@Edge association uses).
///
/// The two markers are the two ways one defect is stated, so every value-set rewrite below
/// reads whichever of them the refusal carries.
const VALUE_SET_MARKER: &str = "Member must satisfy value set:";

/// Every marker a refusal states a value-set constraint with, longest first so the enum one
/// is recognised as itself rather than as the bare set it ends with.
const VALUE_SET_MARKERS: [&str; 2] = [ENUM_VALUE_SET_MARKER, VALUE_SET_MARKER];

/// How the request boundary states the constraint a `@pattern` member carries, up to the
/// pattern itself.
const PATTERN_MARKER: &str = "Member must satisfy regular expression pattern: ";

/// The markers a constraint refusal names the VALUE it is about with (`Value '<value>' at
/// ...`), with the apostrophes as they stand raw and as the XML envelope escapes them.
const VIOLATED_VALUE_MARKERS: [(&str, &str); 2] =
    [("Value &apos;", "&apos;"), ("Value '", "'")];

/// The two ways the request boundary states a `@range` bound (`Member must have value
/// {greater,less} than or equal to <bound>`), up to the bound itself, which runs to the end of
/// the clause.
const RANGE_MARKERS: [&str; 2] = [
    "Member must have value greater than or equal to ",
    "Member must have value less than or equal to ",
];

/// The member paths whose leaf targets a FLOATING-POINT shape of this model, each as the tail
/// of the path a refusal about it states.
///
/// A numeric constraint refusal reports the value submitted and the bound the model declares,
/// and both are rendered in the member's OWN numeric type — a floating-point member's with a
/// decimal fraction, an integral member's without one. The layer that writes the refusal
/// renders an f64 that happens to be integral without its fraction, so the paths whose leaf is
/// floating-point are restated ([`with_fractional_bounds`]) and no other refusal is touched.
///
/// The list is the model's: `ResponseHeadersPolicyServerTimingHeadersConfig$SamplingRate` is
/// the one `@range`-constrained floating-point member this service has
/// (`smithy_query(query="shape",shape="ResponseHeadersPolicyServerTimingHeadersConfig")` — a
/// `double` with `@range` min 0.0 max 100.0), reached through whichever operation submits a
/// `ResponseHeadersPolicyConfig`. The model's other floating-point member,
/// `ContinuousDeploymentSingleWeightConfig$Weight` (a `float`), carries no `@range`, so no
/// bound is ever reported for it.
const FRACTIONAL_MEMBER_PATHS: [&str; 1] = [".serverTimingHeadersConfig.samplingRate"];

/// The partition segment an ARN pattern the MODEL carries names, and the one the service
/// reports for it.
///
/// The model writes its ARN patterns for the `aws` partition alone, while the service accepts
/// an ARN of whichever partition the account is in, so the pattern a refusal reports
/// generalises that segment. Nothing else of the pattern is touched.
const MODELLED_ARN_PARTITION: &str = "arn:aws:";
const STATED_ARN_PARTITION: &str = "arn:aws(-[a-z]+)*:";

/// The marker the framework's refusal message states an absent member with
/// (`Value null at '<member path>' failed to satisfy constraint: Member must not be
/// null`), with the apostrophes as the XML envelope escapes them.
const NULL_MEMBER_MARKERS: [(&str, &str); 2] = [
    ("Value null at &apos;", "&apos;"),
    ("Value null at '", "'"),
];

/// The markers a constraint refusal names the MEMBER it is about with (`... at '<member
/// path>' failed to satisfy constraint: ...`), with the apostrophes as they stand raw and as
/// the XML envelope escapes them.
const CONSTRAINED_MEMBER_MARKERS: [(&str, &str); 2] = [
    (" at &apos;", "&apos; failed to satisfy constraint"),
    (" at '", "' failed to satisfy constraint"),
];

/// The separator the request boundary joins the clauses of one refusal with
/// (`N validation errors detected: <clause>; <clause>`).
const CLAUSE_SEPARATOR: &str = "; ";

/// How any layer of this service states that a value is outside a constraint: the frame the
/// request boundary states every one of them in (`... failed to satisfy constraint: <the
/// constraint>`). What it identifies is the DEFECT — a value the API cannot read because it is
/// outside a constraint the API publishes — whichever layer read the constraint, which is why
/// an operation's own refusal in this frame is answered as the boundary's is.
const CONSTRAINT_MARKER: &str = "failed to satisfy constraint:";

/// How the request boundary counts ONE violation, and how the validator that reads a
/// distribution configuration counts it: that validator writes the plural noun whatever the
/// count is, which is the wording recorded for every refusal of that document
/// (`crate::state::function_association_event_refusal` states the same count).
const SINGULAR_COUNT: &str = "1 validation error detected:";
const PLURAL_COUNT: &str = "1 validation errors detected:";

/// How every clause of a refusal message OPENS: it states the value it is about first
/// (`Value '<value>' at '<member>' failed to satisfy constraint: ...`), the one exception being
/// the first clause, which the count prefix opens. It is what tells a clause BOUNDARY from the
/// `; ` an escaped apostrophe inside a clause ends with ([`stated_clauses`]).
const CLAUSE_OPENING: &str = "Value ";

/// The step a member path takes from a structure into one of its members, which is what makes
/// a path NESTED: a member reached through another member travelled inside the request's
/// payload, since the members bound outside the body (a header, a query parameter, a label of
/// the URI) are members of the input structure itself and name no parent.
const MEMBER_PATH_STEP: char = '.';

/// A fresh request id for one response, from the one mint this service has for it
/// ([`crate::state::new_request_id`]).
///
/// The VALUE is per-request and is not a requirement — nothing can predict it and no
/// caller may depend on a particular one; its PRESENCE is what the envelope requires,
/// and two responses carry two different ids so a caller can tell them apart. The mint
/// renders the 8-4-4-4-12 lowercase hex an AWS request id takes, the same form this
/// service's other opaque identifiers are rendered in.
fn new_request_id() -> String {
    crate::state::new_request_id()
}

/// The body of `resp` as text, when it is in memory and valid UTF-8.
fn body_text(resp: &RtResponse) -> Option<&str> {
    std::str::from_utf8(resp.body().bytes()?).ok()
}

/// How many bytes the request DOCUMENT carries, or `None` when this layer holds no document
/// to measure.
///
/// The bytes the request already carries are the only evidence read: what a request SAYS
/// about its own length is not evidence of what it sent, and a body this layer was handed
/// nothing of is one the shared serve path's own ceiling is what bounds.
fn request_document_bytes(req: &RtRequest) -> Option<usize> {
    req.body().bytes().map(<[u8]>::len)
}

/// The transport-level refusal of a document past [`MAX_REQUEST_BODY_BYTES`]: the status and
/// nothing else, which is all an API that never read the request has to say about it.
fn payload_too_large() -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(PAYLOAD_TOO_LARGE).expect("413 is a valid status code"),
        SdkBody::empty(),
    )
}

/// The text of the first `<name>` element of `document`.
///
/// Enough for the envelope's own flat elements (`Code`, `Message`), which is all this
/// module reads: the documents it reads are the ones this service just serialized.
fn element_text<'a>(document: &'a str, name: &str) -> Option<&'a str> {
    let after_open = document.split_once(&format!("<{name}>"))?.1;
    Some(after_open.split_once(&format!("</{name}>"))?.0)
}

/// The error envelope this service answers `status` / `code` / `message` with, carrying
/// its request id.
///
/// The one renderer for an error this module raises itself; a modelled error a handler
/// raised is rendered by the protocol and reaches the same envelope through
/// [`with_request_id`].
fn error_response(status: u16, code: &str, message: &str) -> RtResponse {
    envelope_response(status, code, Some(message))
}

/// The error envelope this service answers `status` / `code` with, stating `message` when
/// the refusal has something to say beyond its code.
///
/// A refusal with NO message is not one whose message is empty: the `Message` element is
/// absent altogether, which is how the front door answers a request it could not read as any
/// action of this API — there is no member, no value and no resource for it to name.
fn envelope_response(status: u16, code: &str, message: Option<&str>) -> RtResponse {
    let stated = match message {
        Some(message) => format!("<Message>{}</Message>", xml_escape(message)),
        None => String::new(),
    };
    let body = format!(
        "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"{namespace}\"><Error>\
         <Type>{SENDER}</Type><Code>{code}</Code>{stated}</Error>\
         <{REQUEST_ID_ELEMENT}>{request_id}</{REQUEST_ID_ELEMENT}></ErrorResponse>",
        namespace = xml_escape(XML_NAMESPACE),
        request_id = new_request_id(),
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(REFUSED).expect("400 is a valid status code"),
        ),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/xml");
    resp
}

/// The refusal this service's front door answers a request whose method and path name none of
/// the actions it serves.
///
/// What failed is the ACTION lookup — nothing read the request as an operation of this API, so
/// no operation, member or resource is named — and CloudFront answers it as the unknown action
/// it is: [`UNKNOWN_ACTION`] with [`INVALID_ACTION`] and no `Message`. Deliberately NOT the
/// framework's generic `501 NotImplemented`: that is a server-fault class telling a client
/// this emulator lacks the operation, which is false for a path it merely could not match (an
/// identifier carrying a `/` — an ARN where an id belongs — spills into the next path segment
/// and matches no route, while the operation itself is served), and its sentence would echo
/// the request's own path back, which this API never does.
pub fn unknown_action() -> RtResponse {
    envelope_response(UNKNOWN_ACTION, INVALID_ACTION, None)
}

/// The body `resp` should carry so its error envelope names the request, or `None` when
/// it is not an envelope that is missing one (a success, an empty body, an envelope that
/// already carries the element).
fn request_id_insertion(resp: &RtResponse) -> Option<String> {
    // A success is never an error envelope, whatever its document happens to contain:
    // the status is what says this response states a failure.
    if resp.status().as_u16() < 400 {
        return None;
    }
    let body = body_text(resp)?;
    if !body.contains(ERROR_RESPONSE_OPEN) || body.contains(&format!("<{REQUEST_ID_ELEMENT}>")) {
        return None;
    }
    let close_at = body.rfind(ERROR_RESPONSE_CLOSE)?;
    let mut rewritten = String::with_capacity(body.len() + 64);
    rewritten.push_str(&body[..close_at]);
    rewritten.push_str(&format!(
        "<{REQUEST_ID_ELEMENT}>{}</{REQUEST_ID_ELEMENT}>",
        new_request_id()
    ));
    rewritten.push_str(&body[close_at..]);
    Some(rewritten)
}

/// `resp` with its error envelope naming the request it answers.
///
/// A response that is not a restXml error envelope, or one that already names the
/// request, is handed back untouched — so this is applied to EVERY response without
/// having to know which ones are errors.
pub fn with_request_id(mut resp: RtResponse) -> RtResponse {
    let Some(body) = request_id_insertion(&resp) else {
        return resp;
    };
    let length = body.len();
    *resp.body_mut() = SdkBody::from(body);
    // Only a response that already states its length gets the new one: a renderer that
    // left the framing to the transport keeps doing so.
    if resp.headers().get("content-length").is_some() {
        resp.headers_mut()
            .insert("content-length", length.to_string());
    }
    resp
}

/// The query component of a request URI (everything after the first `?`).
///
/// Shared with [`super::authorization`], which reads the resource a request addresses in a
/// query parameter through the same split rather than a second one.
pub(super) fn request_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The `(key, value)` pairs a query component carries — the one split every lookup in this
/// module reads a query string through.
fn query_pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query.split('&').filter_map(|pair| pair.split_once('='))
}

/// The raw value a query component gives `name`, or `None` when it names it no value.
pub(super) fn query_value<'q>(query: &'q str, name: &str) -> Option<&'q str> {
    query_pairs(query).find_map(|(key, value)| (key == name).then_some(value))
}

/// The `MaxItems` the request asks for, read the way the request boundary reads a query
/// value (`+` is a space, `%XX` is the byte it names) through the one decoder the
/// framework vends, so this cannot drift from the boundary it mirrors.
fn requested_max_items(req: &RtRequest) -> Option<String> {
    query_value(request_query(req.uri()), MAX_ITEMS_PARAM).map(percent_decode)
}

/// Whether `value` is a page size `MaxItems` can take.
///
/// Delegates to [`crate::state::page_size`], the one place this service decides what a
/// page size is, so a refusal made at the request boundary and one made by a list
/// handler cannot disagree about which values are usable.
fn is_usable_max_items(value: &str) -> bool {
    value
        .parse::<i32>()
        .is_ok_and(|requested| crate::state::page_size(Some(requested)).is_some())
}

/// How the request boundary shows a value its codec could not read as the parameter's
/// type: ``invalid i32 `x` ``, the shape its integer coercion failure takes. `MaxItems`
/// is modelled as `integer` (the model's list operations) and carries no `@range`, so a
/// value the parameter cannot take fails at THAT coercion and nowhere else — which is
/// why this, and not the value appearing anywhere in the refusal, is what identifies the
/// refusal as being about `MaxItems`.
///
/// Both the raw marker and the one the envelope's escaping produces are returned: the
/// message is XML-escaped into the document, so a value carrying `&`, `<` or an
/// apostrophe reaches the body in its escaped form.
fn coercion_markers(value: &str) -> [String; 2] {
    [
        format!("invalid i32 `{value}`"),
        format!("invalid i32 `{}`", xml_escape(value)),
    ]
}

/// Whether `refusal` is the boundary refusing the request OVER its `MaxItems`: the value
/// is one the parameter cannot take, AND the refusal is the coercion failure of that
/// value.
///
/// The second half matters because a request can carry an unusable `MaxItems` and be
/// refused for something else entirely (a value the boundary reads happily beside an
/// absent member) — restating that refusal as an argument error would hide the cause the
/// caller has to fix, and a value echoed into some other refusal's text cannot stand in
/// for the coercion that never happened.
fn refused_over_max_items(req: &RtRequest, refusal: &str) -> bool {
    let Some(value) = requested_max_items(req) else {
        return false;
    };
    if is_usable_max_items(&value) {
        return false;
    }
    coercion_markers(&value)
        .iter()
        .any(|marker| refusal.contains(marker))
}

/// Whether `refusal` is a refusal of the request over a value outside the set its member
/// admits — the set the member's shape declares, or the set its documentation does.
///
/// Read off the REFUSAL alone: the value set a member admits is part of the schema this API
/// publishes for the request, so the binding the value travelled in — a query parameter, a
/// member of the request's document, a member nested inside its payload — says nothing about
/// whether the request describes something this API can read. What identifies the refusal is
/// the CONSTRAINT it reports ([`VALUE_SET_MARKERS`]), which holds for every value-set
/// constrained member of every operation this service serves.
fn refused_over_value_set(refusal: &str) -> bool {
    element_text(refusal, "Message")
        .is_some_and(|message| VALUE_SET_MARKERS.iter().any(|marker| message.contains(marker)))
}

/// The member path a constraint refusal is about, as the layer that wrote it rendered it, or
/// `None` when the refusal states no member at all.
fn constrained_member_path(refusal: &str) -> Option<&str> {
    member_path(element_text(refusal, "Message")?)
}

/// Whether `refusal` is about a member NESTED inside the request's payload — a member reached
/// through another member, which is what a member of the request DOCUMENT is
/// ([`MEMBER_PATH_STEP`]).
fn refused_over_a_document_member(refusal: &str) -> bool {
    constrained_member_path(refusal).is_some_and(|path| path.contains(MEMBER_PATH_STEP))
}

/// Whether `refusal` is an OPERATION refusing a value outside a constraint the MODEL publishes
/// for the shape that value has to be, rather than for the member it travelled in — which it
/// states in the frame every constraint violation of this API is stated in, because that is
/// what the defect is.
///
/// The WHOLE frame is what identifies it: the message OPENS with the count of violations
/// ([`SINGULAR_COUNT`], [`PLURAL_COUNT`]) and goes on to state the constraint one of them broke
/// ([`CONSTRAINT_MARKER`]). A refusal that merely CARRIES that text is not one — several of this
/// service's refusals echo a value the caller submitted, and a caller who writes the marker into
/// such a value must not thereby choose the code its refusal is answered under.
fn refused_over_a_published_constraint(refusal: &str) -> bool {
    element_text(refusal, "Message").is_some_and(|message| {
        [SINGULAR_COUNT, PLURAL_COUNT]
            .iter()
            .any(|count| message.starts_with(count))
            && message.contains(CONSTRAINT_MARKER)
    })
}

/// The value sets this service publishes in an order of its own, each listed in that order.
///
/// A refusal reports the SET a member admits rather than the shape that set came from, so a
/// set is recognised by the values it holds: every member targeting that enum is stated
/// alike, in every operation and in whatever binding carried the value. A value set with no
/// row here is listed in the order it ARRIVED in — the order this API's schema declares the
/// enum in, which is the only order there is evidence for.
const STATED_VALUE_ORDERS: &[&[&str]] = &[
    // `FunctionStage`, the stage every function and connection-function operation names a
    // copy of a function by: the service lists the published stage first, which is not the
    // order the schema declares (`smithy_query(query="shape",shape="FunctionStage")` ->
    // DEVELOPMENT, LIVE).
    &["LIVE", "DEVELOPMENT"],
    // `CachePolicyType`, the kind a cache policy listing filters on: the service lists the
    // caller-owned kind first, which is not the order the schema declares
    // (`smithy_query(query="shape",shape="CachePolicyType")` -> managed, custom).
    &["custom", "managed"],
    // `OriginAccessControlSigningBehaviors`, the signing behaviour an origin access control
    // states: the service lists the behaviour that signs and overrides an existing
    // `Authorization` header first, which is not the order the schema declares
    // (`smithy_query(query="shape",shape="OriginAccessControlSigningBehaviors")` -> never,
    // always, no-override, always-amz-auth).
    &["always-amz-auth", "never", "always", "no-override"],
    // `OriginAccessControlOriginTypes`, the kind of origin an origin access control signs
    // requests to, which this API publishes as the signing name of that origin's own service:
    // the service lists the object store first and then the compute and media origins, which is
    // not the order the schema declares
    // (`smithy_query(query="shape",shape="OriginAccessControlOriginTypes")` -> s3, mediastore,
    // mediapackagev2, lambda).
    &["s3", "lambda", "mediastore", "mediapackagev2"],
];

/// `refusal` with the enum's admitted values listed in the order this service publishes that
/// enum's values.
///
/// The value set a refusal reports is a property of the ENUM the member targets, not of the
/// request that broke it, so the order it is listed in is the service's own for that enum
/// ([`STATED_VALUE_ORDERS`]) and nothing the emulator chooses: a set whose published order is
/// the order it arrived in is handed back untouched.
fn with_stated_enum_values(refusal: &str) -> String {
    let refusal = with_narrowed_value_set(refusal);
    let Some(message) = element_text(&refusal, "Message") else {
        return refusal;
    };
    let Some(stated) = stated_enum_values(message) else {
        return refusal.clone();
    };
    refusal.replacen(
        &format!("<Message>{message}</Message>"),
        &format!("<Message>{stated}</Message>"),
        1,
    )
}

/// Whether `path` addresses the EVENT TYPE of a CloudFront function association: the member
/// travels in the `member` of the `items` list of a cache behaviour's `functionAssociations`,
/// whichever behaviour of whichever payload holds it.
fn is_function_association_event_type(path: &str) -> bool {
    path.contains(".functionAssociations.items.") && path.ends_with(".member.eventType")
}

/// `refusal` with the value set stated for a member whose own documentation admits a NARROWER
/// set than the shape it targets, in the sentence the operation that owns that rule states.
///
/// Which values a member admits is a property of the MEMBER, so one member is refused in one
/// sentence whichever layer read the value: the request boundary reads the set the SHAPE
/// declares and refuses a value outside it before any handler runs, while the narrower set the
/// member documents is read by the operation ([`crate::state::function_association_event_refusal`]).
/// Left as it stands for every other member, whose shape's set is the set it admits.
///
/// Today that is the event type of a CloudFront function association: `FunctionAssociation`'s
/// `EventType` targets the enum the Lambda@Edge associations share, whose origin-facing values
/// no CloudFront function runs on.
fn with_narrowed_value_set(refusal: &str) -> String {
    if !constrained_member_path(refusal).is_some_and(is_function_association_event_type) {
        return refusal.to_string();
    }
    let Some(message) = element_text(refusal, "Message") else {
        return refusal.to_string();
    };
    let Some(marker_at) = VALUE_SET_MARKERS
        .iter()
        .find_map(|marker| message.find(marker))
    else {
        return refusal.to_string();
    };
    // One clause, so the set it lists runs to the end of the sentence: the head names the
    // member and the value, which the boundary already stated, and the tail is the set this
    // member admits.
    let stated = format!(
        "{}{}",
        &message[..marker_at],
        crate::state::function_association_event_set_clause()
    )
    .replacen(SINGULAR_COUNT, PLURAL_COUNT, 1);
    refusal.replacen(
        &format!("<Message>{message}</Message>"),
        &format!("<Message>{stated}</Message>"),
        1,
    )
}

/// `message` with the value set it lists after one of [`VALUE_SET_MARKERS`] listed in this
/// service's order for that set, or `None` when it lists no set or lists one this service
/// states in the order it already stands in — the rewrite reads the MESSAGE rather than the
/// document, so nothing else the envelope carries can be touched by it.
fn stated_enum_values(message: &str) -> Option<String> {
    let marker_at = VALUE_SET_MARKERS
        .iter()
        .find_map(|marker| message.find(marker))?;
    let tail = &message[marker_at..];
    let open = tail.find('[')?;
    let close = open + tail[open..].find(']')?;
    let values: Vec<&str> = tail[open + 1..close].split(',').map(str::trim).collect();
    let stated = stated_value_order(&values)?;
    if stated == values {
        return None;
    }
    Some(format!(
        "{}{}{}",
        &message[..marker_at + open + 1],
        stated.join(", "),
        &message[marker_at + close..]
    ))
}

/// The order this service lists `values` in, or `None` when that is the order they already
/// stand in — every value set but the ones [`STATED_VALUE_ORDERS`] pins.
fn stated_value_order(values: &[&str]) -> Option<Vec<&'static str>> {
    STATED_VALUE_ORDERS
        .iter()
        .find(|stated| is_same_value_set(stated, values))
        .map(|stated| stated.to_vec())
}

/// Whether two value sets hold the same values. An enum's values are unique by definition,
/// so equal lengths plus containment is set equality.
fn is_same_value_set(stated: &[&str], values: &[&str]) -> bool {
    stated.len() == values.len() && stated.iter().all(|value| values.contains(value))
}

/// `refusal` with every pattern it reports stated as the SERVICE states one.
///
/// What a caller has to satisfy is the pattern the service enforces, not the one the model
/// happens to spell: a `@pattern` is a containment test, so the anchors a model writes around
/// one say nothing about the value, and an ARN pattern the model writes for the `aws`
/// partition alone is satisfied by an ARN of whichever partition the account is in. A refusal
/// that reports the model's spelling tells the caller its value is wrong for a reason that is
/// not the rule \u2014 that no other partition is allowed, and that the value has to match end to
/// end \u2014 so the pattern is restated ([`stated_pattern`]) and nothing else of the refusal is.
///
/// The clauses it walks are the ones [`stated_clauses`] reads, so a pattern that carries the
/// text a clause boundary is written with is restated whole rather than cut at it.
fn with_stated_pattern(refusal: &str) -> String {
    let Some(message) = element_text(refusal, "Message") else {
        return refusal.to_string();
    };
    let stated: Vec<String> = stated_clauses(message)
        .iter()
        .map(|clause| match clause.split_once(PATTERN_MARKER) {
            Some((head, pattern)) => {
                format!("{head}{PATTERN_MARKER}{}", stated_pattern(pattern))
            }
            None => clause.to_string(),
        })
        .collect();
    let stated = stated.join(CLAUSE_SEPARATOR);
    if stated == message {
        return refusal.to_string();
    }
    refusal.replacen(
        &format!("<Message>{message}</Message>"),
        &format!("<Message>{stated}</Message>"),
        1,
    )
}

/// `pattern` as the service states it: unanchored, with the partition segment of an ARN
/// pattern generalised over the partitions an ARN can name.
///
/// The anchor pair is dropped by the ONE rule this workspace owns for it
/// (`service_common::validation::unanchored`), which drops the pair positionally and leaves an
/// escaped `$` alone, so this service cannot disagree with its siblings about what an
/// unanchored expression is. The partition generalisation is this API's own, so it stays here.
fn stated_pattern(pattern: &str) -> String {
    service_common::validation::unanchored(pattern)
        .replace(MODELLED_ARN_PARTITION, STATED_ARN_PARTITION)
}

/// `refusal` with the value and the bound of every clause about a FLOATING-POINT member
/// rendered as that member's own type renders a number: with a decimal fraction.
///
/// A numeric constraint refusal reports two numbers — the value submitted and the bound the
/// model declares — and both belong to the MEMBER, whose type decides how a number of it is
/// written. The layer that wrote the refusal renders an integral floating-point value and an
/// integral bound without their fraction, which states the constraint of an integral member;
/// the members this holds for are the model's own ([`FRACTIONAL_MEMBER_PATHS`]), and every
/// other clause is handed back exactly as it stands.
fn with_fractional_bounds(refusal: &str) -> String {
    let Some(message) = element_text(refusal, "Message") else {
        return refusal.to_string();
    };
    let clauses: Vec<String> = stated_clauses(message)
        .iter()
        .map(|clause| as_fractional(clause))
        .collect();
    let restated = clauses.join(CLAUSE_SEPARATOR);
    if restated == message {
        return refusal.to_string();
    }
    refusal.replacen(
        &format!("<Message>{message}</Message>"),
        &format!("<Message>{restated}</Message>"),
        1,
    )
}

/// The CLAUSES of `message`, each a whole constraint the request broke.
///
/// The layer that writes a refusal joins its clauses with [`CLAUSE_SEPARATOR`], and an
/// XML-escaped apostrophe (`&apos;`) ENDS in the semicolon that separator opens with — so a
/// split on the separator alone cuts one clause into pieces wherever the message names a
/// member or echoes a value. Every clause opens by stating the value it is about
/// ([`CLAUSE_OPENING`]), so a piece that does not open one belongs to the clause before it.
fn stated_clauses(message: &str) -> Vec<String> {
    let mut clauses: Vec<String> = Vec::new();
    for piece in message.split(CLAUSE_SEPARATOR) {
        match clauses.last_mut() {
            Some(last) if !piece.starts_with(CLAUSE_OPENING) => {
                last.push_str(CLAUSE_SEPARATOR);
                last.push_str(piece);
            }
            _ => clauses.push(piece.to_string()),
        }
    }
    clauses
}

/// Whether `clause` is about a member whose shape is floating-point
/// ([`FRACTIONAL_MEMBER_PATHS`]).
fn names_a_fractional_member(clause: &str) -> bool {
    member_path(clause).is_some_and(|path| {
        FRACTIONAL_MEMBER_PATHS
            .iter()
            .any(|member| path.ends_with(member))
    })
}

/// `clause` with the value it echoes and the bound it reports written as decimal fractions —
/// or exactly as it stands when it is about a member of any other type.
fn as_fractional(clause: &str) -> String {
    if !names_a_fractional_member(clause) {
        return clause.to_string();
    }
    let stated = with_fractional_value(clause).unwrap_or_else(|| clause.to_string());
    with_fractional_bound(&stated).unwrap_or(stated)
}

/// `clause` with the VALUE it echoes written as a decimal fraction, or `None` when it echoes
/// none or when that value already carries one.
fn with_fractional_value(clause: &str) -> Option<String> {
    for (open, close) in VIOLATED_VALUE_MARKERS {
        let Some((head, rest)) = clause.split_once(open) else {
            continue;
        };
        let (value, tail) = rest.split_once(close)?;
        return fractional(value).map(|stated| format!("{head}{open}{stated}{close}{tail}"));
    }
    None
}

/// `clause` with the BOUND it reports written as a decimal fraction, or `None` when it reports
/// none or when that bound already carries one. One clause states one constraint, so the bound
/// runs to the end of it.
fn with_fractional_bound(clause: &str) -> Option<String> {
    for marker in RANGE_MARKERS {
        let Some((head, bound)) = clause.split_once(marker) else {
            continue;
        };
        return fractional(bound).map(|stated| format!("{head}{marker}{stated}"));
    }
    None
}

/// `number` with a decimal fraction, or `None` when it is already written with one — or is no
/// plain decimal integer, which is the only form a fraction can be added to.
fn fractional(number: &str) -> Option<String> {
    let digits = number.strip_prefix('-').unwrap_or(number);
    (!digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()))
        .then(|| format!("{number}.0"))
}

/// The member path a constraint CLAUSE is about, or `None` when it states no member.
fn member_path(clause: &str) -> Option<&str> {
    CONSTRAINED_MEMBER_MARKERS.iter().find_map(|(open, close)| {
        let after = clause.split_once(open)?.1;
        Some(after.split_once(close)?.0)
    })
}

/// `refusal` answered under `code`, with everything else the boundary rendered — its
/// `Message` above all — left exactly as it stands.
///
/// Used where what this service restates is the CODE alone: the sentence the boundary wrote
/// already states the member and the constraint it broke, which is the rule the refusal is
/// built from, so rewriting it would say less rather than more. Rewriting the element
/// in place is also what keeps the text's escaping as the renderer produced it.
fn recoded(refusal: &str, from: &str, code: &str) -> RtResponse {
    let body = refusal.replacen(
        &format!("<Code>{from}</Code>"),
        &format!("<Code>{code}</Code>"),
        1,
    );
    let mut resp = RtResponse::new(
        StatusCode::try_from(REFUSED).expect("400 is a valid status code"),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/xml");
    resp
}

/// Whether the request carries no document at all: a body that is there and holds
/// nothing but whitespace.
///
/// A body the transport has not buffered reads as UNKNOWN, not as blank: answering
/// "the payload is null" for a document nobody looked at would hide the member a caller
/// actually has to fix.
fn body_is_blank(req: &RtRequest) -> bool {
    req.body()
        .bytes()
        .is_some_and(|bytes| bytes.iter().all(u8::is_ascii_whitespace))
}

/// The member path a refusal message states as null, as the framework rendered it.
fn null_member_path(refusal: &str) -> Option<&str> {
    NULL_MEMBER_MARKERS.iter().find_map(|(open, close)| {
        let after = refusal.split_once(open)?.1;
        Some(after.split_once(close)?.0)
    })
}

/// `resp`'s body when `resp` is a refusal the request boundary made before any handler
/// ran, and `None` for anything else — including every modelled CloudFront error, whose
/// code is a shape this model declares.
fn framework_refusal(resp: &RtResponse) -> Option<&str> {
    if resp.status().as_u16() != REFUSED {
        return None;
    }
    let body = body_text(resp)?;
    (element_text(body, "Code")? == FRAMEWORK_REFUSAL_CODE).then_some(body)
}

/// `resp`'s body and its code when `resp` is a refusal an OPERATION of this service wrote —
/// a 400 under a code this model declares — and `None` for anything else.
fn operation_refusal(resp: &RtResponse) -> Option<(&str, &str)> {
    if resp.status().as_u16() != REFUSED {
        return None;
    }
    let body = body_text(resp)?;
    let code = element_text(body, "Code")?;
    (code != FRAMEWORK_REFUSAL_CODE).then_some((body, code))
}

/// This service's own answer to a refusal the boundary stated in the framework's
/// vocabulary, or `None` to keep the framework's rendering.
///
/// Reads the REQUEST and the REFUSAL, not the operation: every condition is a property of
/// one of them (the value the request gives its page-size parameter, the constraint the
/// refusal reports, the member it is about, and whether the request carries a body at all),
/// so they hold for every operation that binds them rather than for a list of operations
/// named here.
fn restated(req: &RtRequest, resp: &RtResponse) -> Option<RtResponse> {
    // A value outside the set its member admits is no value of that member at all,
    // whichever layer read the set: the boundary refuses one outside a set the SCHEMA
    // carries, and an operation refuses one outside a set the model documents in prose but
    // declares nowhere (a function association's event type). Both report the same defect in
    // the same sentence, so both are answered under `MalformedXML` — the code this API
    // answers a request document it cannot read with.
    if let Some((refusal, code)) = operation_refusal(resp) {
        if refused_over_value_set(refusal) {
            return Some(recoded(&with_stated_enum_values(refusal), code, MALFORMED_XML));
        }
        // A value outside a constraint the MODEL publishes for the shape that value has to be
        // is a value this API cannot read, whichever layer read the constraint: the boundary
        // refuses one against a constraint the schema carries and this service answers
        // `MalformedXML` for it below, so an operation that reads such a constraint itself —
        // a listing whose `Marker` is a pagination token the API publishes a pattern for,
        // which the member it travels in does not declare — reaches the same answer rather
        // than a second one. The sentence states the member, the value and the constraint it
        // broke, so the code alone is restated.
        if refused_over_a_published_constraint(refusal) {
            return Some(recoded(refusal, code, MALFORMED_XML));
        }
        return None;
    }

    let refusal = framework_refusal(resp)?;

    // "An argument is invalid": the page size the request asks for is not one.
    if refused_over_max_items(req, refusal) {
        return Some(error_response(
            REFUSED,
            INVALID_ARGUMENT,
            crate::state::MAX_ITEMS_INVALID_MESSAGE,
        ));
    }

    // A value the model's enum does not admit is no value of that member at all: the value
    // set is part of the schema this API publishes for the request, so a request naming a
    // value outside it is one CloudFront cannot read, which it answers with `MalformedXML`
    // (400) — the same code it answers any unreadable request document with, in whatever
    // binding the value travelled. The member and the values it admits are what the
    // boundary's own sentence states, so the code is restated and the sentence is left as it
    // stands — except for the ORDER the value set is listed in, which belongs to the enum and
    // is this service's own for it.
    if refused_over_value_set(refusal) {
        return Some(recoded(
            &with_stated_enum_values(refusal),
            FRAMEWORK_REFUSAL_CODE,
            MALFORMED_XML,
        ));
    }

    // A request with a body to give carried none, so the whole payload is null. The
    // member the boundary found null is one NESTED inside that payload — which is what an
    // absent payload produces, since a member the payload itself requires cannot be read
    // from a document that is not there — and the member that is actually absent is the
    // payload, the ROOT of that path. A refusal naming a member that is nested in
    // nothing is about a member bound somewhere other than the body (a header, a query
    // parameter), so it keeps the wording the boundary gave it.
    if matches!(req.method(), "POST" | "PUT") && body_is_blank(req) {
        if let Some((payload, _)) = null_member_path(refusal).and_then(|path| path.split_once('.'))
        {
            return Some(error_response(
                REFUSED,
                MALFORMED_XML,
                &missing_member_message(payload),
            ));
        }
    }

    // EVERY other constraint the boundary reports on a member NESTED inside the request's
    // payload — a member that is absent, too long, outside its range or outside its pattern —
    // is a request DOCUMENT this API cannot read, which it answers with `MalformedXML` (400):
    // `ValidationException` is no code this service's model declares, so a caller branching on
    // CloudFront's own vocabulary would fall through to a generic failure and never reach its
    // input-correction path. The member and the constraint it broke are what the boundary's
    // sentence already states, so the code is restated and the sentence is left as it stands —
    // except for a PATTERN, which is restated as the service states one
    // ([`with_stated_pattern`]).
    //
    // A refusal about a member bound OUTSIDE the body (a header, a query parameter, a label of
    // the URI) names a member of the input structure itself rather than one reached through
    // another member, so it is not a defect of the document and keeps the boundary's wording.
    if refused_over_a_document_member(refusal) {
        return Some(recoded(
            &with_stated_pattern(&with_fractional_bounds(refusal)),
            FRAMEWORK_REFUSAL_CODE,
            MALFORMED_XML,
        ));
    }

    None
}

/// The generated definition, with the error envelope finished on the three failures the
/// router renders OUTSIDE the dispatch path: a request that names no modelled operation,
/// a transport-level failure, and a refusal made before any operation runs. The last two are
/// the generated renderer's own document with the envelope's request id added, so a caller
/// correlating a failure never has to care which layer refused it; the FIRST is this
/// service's own front-door refusal ([`unknown_action`]), because the action a request names
/// is what this layer resolves and `501 NotImplemented` would report an operation this
/// emulator serves as one it lacks.
///
/// Identity, the protocol and the routing table come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated here. The
/// one thing between this wrapper and that entry is the AUDIT leg
/// ([`super::management_events`]): the table every request is routed from is the audited
/// one, whose keys are the same modelled operation names, so no request can be served by a
/// path that records nothing.
pub struct CloudFrontDefinition<Eng: Send + Sync + 'static> {
    entry: ::service_common::audited_operation::AuditedDefinition<crate::ServiceDefinition<Eng>>,
}

impl<Eng: Send + Sync + 'static> CloudFrontDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry threads
    /// into every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            entry: ::service_common::audited_operation::AuditedDefinition::new(
                crate::ServiceDefinition::new(::std::sync::Arc::clone(&engine)),
                super::management_events::audited_table(&engine),
            ),
        }
    }

    /// Whether the generated routing table can serve this request — the condition under
    /// which this service claims it for [`ErrorEnvelope`].
    pub fn routes(&self, req: &RtRequest) -> bool {
        self.entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for CloudFrontDefinition<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, _req: &RtRequest) -> RtResponse {
        // NOT the generated renderer's `501 NotImplemented`: a request this API could not
        // match to a route is one whose ACTION the front door could not find
        // ([`unknown_action`]), which is a client error this service names itself. The
        // generated renderer is left unread for this one failure; the other two below still
        // go through it.
        unknown_action()
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_request_id(ErasedServiceDefinition::refusal_response(
            &self.entry,
            error,
            chosen,
            req,
        ))
    }

    fn wire_protocol(&self, req: &RtRequest) -> smithy_server_runtime::WireProtocol<'_> {
        self.entry.wire_protocol(req)
    }
}

/// The modelled operation the request names together with the NAME the model names it by
/// (which is the key the routing table holds it under, and the action IAM publishes it as),
/// read from the model-derived routing table (never from the service's own override, which
/// routes straight back here).
fn modelled_route<'s, 'r>(
    svc: &'s dyn Service,
    req: &'r RtRequest,
) -> Option<(&'r str, &'s dyn ErasedOperation)> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    let operation = svc.definition().routing_table().get(key)?;
    Some((key, operation))
}

/// The seam every request this service serves runs through: it takes IAM's decision about
/// the request, then dispatches the modelled operation unchanged, then finishes the
/// response's error envelope (see the module docs).
///
/// It holds the ONE shared engine, because the decision reads the caller's identity and the
/// policies in force on it out of the account's STS and IAM stores
/// ([`super::authorization`]); the operation it runs is still looked up on the service the
/// router hands it, so there is no second instance of any operation.
pub struct ErrorEnvelope<Eng> {
    engine: ::std::sync::Arc<Eng>,
}

impl<Eng> ErrorEnvelope<Eng> {
    /// Build the seam from the shared engine the generated entry threads into every
    /// operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self { engine }
    }
}

impl<Eng: Send + Sync + 'static> ErasedOperation for ErrorEnvelope<Eng> {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            // A document past what this API reads is refused on its SIZE, in the
            // transport's own vocabulary ([`payload_too_large`]), before an operation is
            // named: nothing read it, so there is no modelled error to state it in.
            if request_document_bytes(req).is_some_and(|carried| carried > MAX_REQUEST_BODY_BYTES)
            {
                return payload_too_large();
            }
            // The generated deserialize -> validate -> handle -> serialize walk, run
            // exactly as the router would have run it. A request that names no modelled
            // operation never reaches here: the service claims only the ones its routing
            // table resolves.
            let Some((operation, op)) = modelled_route(svc, req) else {
                return with_request_id(svc.definition().routing_error_response(req));
            };
            // IAM's decision, taken in front of the operation: the action is the one the
            // request named under this service's prefix, and the resource is the one the
            // REQUEST addresses. A refused request never reaches the operation, so it reads
            // nothing, writes nothing and reports nothing about the resource it named.
            if let Some(refused) =
                super::authorization::refusal(&*self.engine, ctx, operation, req)
            {
                return refused;
            }
            let resp = op.run(svc, req, ctx).await;
            let resp = restated(req, &resp).unwrap_or(resp);
            with_request_id(resp)
        })
    }
}

#[cfg(test)]
mod tests {
    //! The envelope surgery and the three restatement conditions, read off documents and
    //! requests rather than through the router (the crate's integration suite drives the
    //! same rules end to end over HTTP).

    use super::*;

    /// An error envelope as the protocol serializer renders one: no `RequestId`.
    fn serialized_error(code: &str) -> RtResponse {
        let body = format!(
            "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
             <Code>{code}</Code><Message>something</Message></Error></ErrorResponse>"
        );
        RtResponse::new(
            StatusCode::try_from(400u16).expect("400"),
            SdkBody::from(body),
        )
    }

    /// An enum refusal's value set is listed in the order this service publishes that enum's
    /// values, and a set this service has no order of its own for keeps the order it arrived
    /// in. Nothing else the sentence says changes either way.
    #[test]
    fn an_enum_refusals_value_set_is_listed_in_the_services_order() {
        // (case, the set as the boundary listed it, the set as this service lists it). The
        // value sets come from `smithy_query(query="shape",shape="…")`: FunctionStage
        // (DEVELOPMENT, LIVE), CachePolicyType (managed, custom), IpAddressType (ipv4,
        // ipv6, dualstack) and OriginAccessControlSigningBehaviors (never, always,
        // no-override, always-amz-auth).
        let rows: [(&str, &str, &str); 7] = [
            (
                "the stage set as the schema declares it",
                "[DEVELOPMENT, LIVE]",
                "[LIVE, DEVELOPMENT]",
            ),
            (
                "the stage set already in the order the service lists it",
                "[LIVE, DEVELOPMENT]",
                "[LIVE, DEVELOPMENT]",
            ),
            (
                "the policy-kind set as the schema declares it",
                "[managed, custom]",
                "[custom, managed]",
            ),
            (
                "a three-value set with no order of its own",
                "[ipv4, ipv6, dualstack]",
                "[ipv4, ipv6, dualstack]",
            ),
            (
                "a one-value set holding a stage",
                "[DEVELOPMENT]",
                "[DEVELOPMENT]",
            ),
            (
                "the signing-behaviour set as the schema declares it",
                "[never, always, no-override, always-amz-auth]",
                "[always-amz-auth, never, always, no-override]",
            ),
            (
                "the signing-behaviour set already in the order the service lists it",
                "[always-amz-auth, never, always, no-override]",
                "[always-amz-auth, never, always, no-override]",
            ),
        ];

        for (case, listed, expected) in rows {
            let refusal = format!(
                "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
                 <Code>{FRAMEWORK_REFUSAL_CODE}</Code><Message>1 validation error detected: \
                 Value &apos;bogus&apos; at &apos;stage&apos; failed to satisfy constraint: \
                 {ENUM_VALUE_SET_MARKER} {listed}</Message></Error></ErrorResponse>"
            );
            let stated = with_stated_enum_values(&refusal);
            let actual = element_text(&stated, "Message").unwrap_or_default();
            let wanted = format!(
                "1 validation error detected: Value &apos;bogus&apos; at &apos;stage&apos; \
                 failed to satisfy constraint: {ENUM_VALUE_SET_MARKER} {expected}"
            );
            assert_eq!(
                actual, wanted,
                "case {case:?}: expected the sentence {wanted:?}, got {actual:?}"
            );
        }

        // A refusal that lists no value set is handed back exactly as it stands.
        let other = "<ErrorResponse><Error><Code>ValidationException</Code>\
                     <Message>Value null at &apos;x.y&apos; failed to satisfy constraint: \
                     Member must not be null</Message></Error></ErrorResponse>";
        let actual = with_stated_enum_values(other);
        assert_eq!(
            actual, other,
            "case \"another constraint\": expected the refusal left alone, got {actual:?}"
        );
    }

    /// Which refusals the published-constraint restatement claims: the WHOLE frame — the count
    /// of violations, then the constraint one of them broke — and not a message that merely
    /// carries that text inside a value the caller submitted.
    #[test]
    fn only_a_whole_constraint_frame_is_answered_as_an_unreadable_value() {
        // (case, the message an operation wrote, whether its code is restated)
        let rows: [(&str, String, bool); 4] = [
            (
                "an operation stating a constraint the model publishes",
                format!(
                    "{SINGULAR_COUNT} Value &apos;a!b&apos; at &apos;nextToken&apos; failed to \
                     satisfy constraint: Member must satisfy regular expression pattern: [a-z]*"
                ),
                true,
            ),
            (
                "the same frame with the plural count a document walk writes",
                format!(
                    "{PLURAL_COUNT} Value &apos;x&apos; at &apos;a.b&apos; failed to satisfy \
                     constraint: Member must satisfy value set: [one, two]"
                ),
                true,
            ),
            (
                "a refusal echoing the marker inside a submitted value",
                "Marker failed to satisfy constraint: is not a marker this listing handed out"
                    .to_string(),
                false,
            ),
            (
                "a refusal stating a rule of its own",
                "The parameter Name/ARN must be provided.".to_string(),
                false,
            ),
        ];

        for (case, message, restated) in rows {
            let refusal = format!(
                "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
                 <Code>{INVALID_ARGUMENT}</Code><Message>{message}</Message></Error>\
                 </ErrorResponse>"
            );
            assert_eq!(
                refused_over_a_published_constraint(&refusal),
                restated,
                "case {case:?}: expected the refusal to be read as a published-constraint \
                 violation: {restated}"
            );
        }
    }

    /// Which members [`FRACTIONAL_MEMBER_PATHS`] has to cover is the MODEL's answer, not this
    /// catalogue's: a member whose `@range` is read against a floating-point shape. Reading it
    /// back off the model is what fails this suite when the model gains another one, so the
    /// catalogue cannot fall silently out of step with the types it is derived from.
    #[test]
    fn every_range_constrained_floating_point_member_of_the_model_is_restated() {
        const MODEL: &str =
            include_str!("../../smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json");
        const RANGE_TRAIT: &str = "smithy.api#range";

        let model: serde_json::Value =
            serde_json::from_str(MODEL).expect("this service's model is readable JSON");
        let shapes = model["shapes"]
            .as_object()
            .expect("a Smithy model declares its shapes");

        // The floating-point shapes of the model, by shape id.
        let floating: Vec<&str> = shapes
            .iter()
            .filter(|(_, shape)| matches!(shape["type"].as_str(), Some("double" | "float")))
            .map(|(id, _)| id.as_str())
            .collect();

        // Every member targeting one of them whose range is declared — on the member itself or
        // on the shape it targets — named as a refusal names a member: in lowerCamel.
        let mut constrained: Vec<String> = Vec::new();
        for shape in shapes.values() {
            let Some(members) = shape["members"].as_object() else {
                continue;
            };
            for (member, declaration) in members {
                let Some(target) = declaration["target"].as_str() else {
                    continue;
                };
                if !floating.contains(&target) {
                    continue;
                }
                if declaration["traits"][RANGE_TRAIT].is_object()
                    || shapes[target]["traits"][RANGE_TRAIT].is_object()
                {
                    let mut characters = member.chars();
                    let stated = match characters.next() {
                        Some(first) => {
                            first.to_lowercase().collect::<String>() + characters.as_str()
                        }
                        None => continue,
                    };
                    constrained.push(stated);
                }
            }
        }
        assert!(
            !constrained.is_empty(),
            "the model must declare at least one range-constrained floating-point member for \
             this rule to be about anything"
        );

        for member in &constrained {
            assert!(
                FRACTIONAL_MEMBER_PATHS
                    .iter()
                    .any(|path| path.ends_with(&format!(".{member}"))),
                "case {member:?}: the model declares a range-constrained floating-point member \
                 no entry of {FRACTIONAL_MEMBER_PATHS:?} covers, so its refusal would state an \
                 integral value and bound"
            );
        }
    }

    /// The two numbers a `@range` clause states are written as the member's own type writes a
    /// number: a floating-point member's with a decimal fraction, every other member's exactly
    /// as the layer that wrote the refusal rendered it.
    #[test]
    fn a_range_clause_about_a_double_states_both_numbers_as_doubles() {
        // (case, the member path the clause is about, the value, the requirement, the bound,
        // the sentence this service states)
        let rows: [(&str, &str, &str, &str, &str, &str); 4] = [
            (
                "an integral value past a double member's maximum",
                "responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate",
                "150",
                "less",
                "100",
                "1 validation error detected: Value &apos;150.0&apos; at \
                 &apos;responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate&apos; \
                 failed to satisfy constraint: Member must have value less than or equal to 100.0",
            ),
            (
                "an integral value below a double member's minimum",
                "responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate",
                "-2",
                "greater",
                "0",
                "1 validation error detected: Value &apos;-2.0&apos; at \
                 &apos;responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate&apos; \
                 failed to satisfy constraint: Member must have value greater than or equal to \
                 0.0",
            ),
            (
                "a value already written with a fraction",
                "responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate",
                "100.5",
                "less",
                "100.0",
                "1 validation error detected: Value &apos;100.5&apos; at \
                 &apos;responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate&apos; \
                 failed to satisfy constraint: Member must have value less than or equal to 100.0",
            ),
            (
                "a range clause about a member of an integral type",
                "distributionConfig.cacheBehaviors.items.1.member.minTTL",
                "7",
                "less",
                "3",
                "1 validation error detected: Value &apos;7&apos; at \
                 &apos;distributionConfig.cacheBehaviors.items.1.member.minTTL&apos; failed to \
                 satisfy constraint: Member must have value less than or equal to 3",
            ),
        ];

        for (case, path, value, requirement, bound, expected) in rows {
            let clause = format!(
                "1 validation error detected: Value &apos;{value}&apos; at &apos;{path}&apos; \
                 failed to satisfy constraint: Member must have value {requirement} than or \
                 equal to {bound}"
            );
            assert_eq!(
                member_path(&clause),
                Some(path),
                "case {case:?}: expected the clause to name the member {path:?}"
            );
            let refusal = format!(
                "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
                 <Code>{FRAMEWORK_REFUSAL_CODE}</Code><Message>{clause}</Message>\
                 </Error></ErrorResponse>"
            );
            let stated = with_fractional_bounds(&refusal);
            let actual = element_text(&stated, "Message").unwrap_or_default();
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the sentence {expected:?}, got {actual:?}"
            );
        }
    }

    /// A message that states SEVERAL constraints is read clause by clause — the escaped
    /// apostrophes inside a clause end in the semicolon the clause separator opens with, so a
    /// clause boundary is where the next clause opens — and only the clauses about a
    /// floating-point member are restated.
    #[test]
    fn a_message_of_several_clauses_restates_only_the_double_members_clauses() {
        let message = "1 validation errors detected: Value &apos;200&apos; at \
                       &apos;responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate\
                       &apos; failed to satisfy constraint: Member must have value less than or \
                       equal to 100; Value &apos;k&apos; at &apos;tags.items.1.member.key&apos; \
                       failed to satisfy constraint: Member must have length greater than or \
                       equal to 2";
        let refusal = format!(
            "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
             <Code>{FRAMEWORK_REFUSAL_CODE}</Code><Message>{message}</Message>\
             </Error></ErrorResponse>"
        );
        let expected = "1 validation errors detected: Value &apos;200.0&apos; at \
                        &apos;responseHeadersPolicyConfig.serverTimingHeadersConfig.samplingRate\
                        &apos; failed to satisfy constraint: Member must have value less than or \
                        equal to 100.0; Value &apos;k&apos; at &apos;tags.items.1.member.key\
                        &apos; failed to satisfy constraint: Member must have length greater \
                        than or equal to 2";
        let stated = with_fractional_bounds(&refusal);
        let actual = element_text(&stated, "Message").unwrap_or_default();
        assert_eq!(
            actual, expected,
            "case \"two clauses\": expected the sentence {expected:?}, got {actual:?}"
        );
    }

    #[test]
    fn request_id_is_added_beside_error_and_only_once() {
        let first = with_request_id(serialized_error("NoSuchDistribution"));
        let body = body_text(&first).expect("an in-memory body").to_string();
        let id = element_text(&body, REQUEST_ID_ELEMENT).expect("a request id");
        assert!(
            body.contains(&format!("</Error><RequestId>{id}</RequestId></ErrorResponse>")),
            "the id must sit beside Error inside the envelope, got {body}"
        );

        // Idempotent: an envelope that already names the request keeps the id it has.
        let again = with_request_id(first);
        assert_eq!(
            element_text(body_text(&again).expect("a body"), REQUEST_ID_ELEMENT),
            Some(id),
            "a second pass must not add a second id"
        );
    }

    #[test]
    fn each_response_carries_its_own_request_id() {
        let one = with_request_id(serialized_error("NoSuchDistribution"));
        let two = with_request_id(serialized_error("NoSuchDistribution"));
        let left = body_text(&one).and_then(|b| element_text(b, REQUEST_ID_ELEMENT).map(str::to_string));
        let right = body_text(&two).and_then(|b| element_text(b, REQUEST_ID_ELEMENT).map(str::to_string));
        assert!(
            left.is_some() && left != right,
            "two responses must carry two ids, got {left:?} and {right:?}"
        );
    }

    #[test]
    fn a_success_document_is_untouched() {
        let body = "<DistributionList><Quantity>0</Quantity></DistributionList>";
        let resp = RtResponse::new(
            StatusCode::try_from(200u16).expect("200"),
            SdkBody::from(body),
        );
        let answered = with_request_id(resp);
        assert_eq!(
            body_text(&answered),
            Some(body),
            "a response that is no error envelope must be handed back as it is"
        );
    }

    #[test]
    fn max_items_is_usable_only_as_a_non_negative_integer() {
        // (query, the decoded value, whether it is a page size) — the class is "what the
        // integer page size can take", read as the boundary reads a query value. A zero
        // bounds the page at no items, which is a page this service serves.
        let cases = [
            ("MaxItems=not-a-number", Some("not-a-number"), false),
            ("MaxItems=", Some(""), false),
            ("MaxItems=0", Some("0"), true),
            ("MaxItems=-1", Some("-1"), false),
            ("MaxItems=3.5", Some("3.5"), false),
            ("MaxItems=99999999999", Some("99999999999"), false),
            ("MaxItems=%20", Some(" "), false),
            ("MaxItems=1", Some("1"), true),
            // `%31` is the digit `1`: the boundary decodes it, so this does too.
            ("MaxItems=%31", Some("1"), true),
            ("MaxItems=100&Marker=E1", Some("100"), true),
            ("Marker=E1", None, false),
            ("", None, false),
        ];
        for (query, decoded, usable) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri(format!("/2020-05-31/distribution?{query}"))
                .expect("a valid uri");
            let actual = requested_max_items(&req);
            assert_eq!(
                actual.as_deref(),
                decoded,
                "case {query:?}: expected the value {decoded:?}, got {actual:?}"
            );
            if let Some(value) = actual {
                let got = is_usable_max_items(&value);
                assert_eq!(
                    got, usable,
                    "case {query:?}: expected usable={usable}, got {got}"
                );
            }
        }
    }

    #[test]
    fn only_a_refusal_over_max_items_is_restated_as_an_argument_error() {
        // The boundary refuses an unreadable value by showing it; a request carrying a
        // value the boundary READS happily beside a different refusal keeps that refusal.
        let coercion = "<ErrorResponse><Error><Code>ValidationException</Code>\
             <Message>invalid i32 `not-a-number`: invalid digit found in string</Message>\
             </Error></ErrorResponse>"
            .to_string();
        let absent_member = format!(
            "<ErrorResponse><Error><Code>ValidationException</Code><Message>{}</Message>\
             </Error></ErrorResponse>",
            xml_escape(&missing_member_message("distributionConfig.callerReference"))
        );
        let cases = [
            ("MaxItems=not-a-number", coercion.as_str(), true),
            ("MaxItems=0", absent_member.as_str(), false),
            ("MaxItems=10", coercion.as_str(), false),
            // A value echoed into ANOTHER refusal's text is not a coercion of it: the
            // member path a null-member refusal names cannot make the page size the
            // cause.
            (
                "MaxItems=distributionConfig.callerReference",
                absent_member.as_str(),
                false,
            ),
        ];
        for (query, refusal, expected) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri(format!("/2020-05-31/distribution?{query}"))
                .expect("a valid uri");
            let actual = refused_over_max_items(&req, refusal);
            assert_eq!(
                actual, expected,
                "case {query:?}: expected restated={expected}, got {actual}"
            );
        }
    }

    #[test]
    fn a_null_member_path_is_read_from_the_framework_refusal() {
        let refusal = format!(
            "<ErrorResponse><Error><Code>ValidationException</Code><Message>{}</Message>\
             </Error></ErrorResponse>",
            xml_escape(&missing_member_message("invalidationBatch.callerReference"))
        );
        assert_eq!(
            null_member_path(&refusal),
            Some("invalidationBatch.callerReference"),
            "the member path must be read back out of the rendered refusal: {refusal}"
        );
    }

    #[test]
    fn only_a_member_nested_in_a_payload_is_restated_as_malformed_xml() {
        // (the null member the boundary reports, the code the service answers) — a member
        // nested inside the body's payload is the absent payload; one nested in nothing
        // is bound somewhere other than the body, so the boundary's refusal stands.
        let cases = [
            ("distributionConfig.callerReference", MALFORMED_XML),
            ("invalidationBatch.paths.quantity", MALFORMED_XML),
            ("alias", FRAMEWORK_REFUSAL_CODE),
            ("ifMatch", FRAMEWORK_REFUSAL_CODE),
        ];
        for (member, expected) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri("/2020-05-31/distribution").expect("a valid uri");
            req.set_method("POST").expect("a valid method");
            let refused = {
                let mut resp = RtResponse::new(
                    StatusCode::try_from(REFUSED).expect("400"),
                    SdkBody::from(format!(
                        "<ErrorResponse><Error><Code>{FRAMEWORK_REFUSAL_CODE}</Code>\
                         <Message>{}</Message></Error></ErrorResponse>",
                        xml_escape(&missing_member_message(member))
                    )),
                );
                resp.headers_mut().insert("content-type", "application/xml");
                resp
            };
            let answered = restated(&req, &refused).unwrap_or(refused);
            let body = body_text(&answered).expect("an in-memory body").to_string();
            let code = element_text(&body, "Code");
            assert_eq!(
                code,
                Some(expected),
                "case {member:?}: expected code {expected}, got {code:?} in {body}"
            );
        }
    }
}
