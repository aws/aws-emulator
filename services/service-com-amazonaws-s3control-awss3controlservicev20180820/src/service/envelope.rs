// SPDX-License-Identifier: CC0-1.0

//! This service's error envelope, and the one seam every request it serves runs
//! through.
//!
//! S3 Control is `restXml`, and its refusals travel in the S3-style document
//! `<ErrorResponse><Error><Code/><Message/>[<member/>…]</Error><RequestId/><HostId/></ErrorResponse>`:
//! the `Error` element states WHAT failed, and the `RequestId` and `HostId`
//! BESIDE it name the request it failed for, so a caller can correlate the
//! failure with the service's own record of it. Neither is a member of any error
//! shape the model declares — the model declares no error shapes for most of
//! this service at all — so the protocol serializer, which renders an error from
//! its shape's members, cannot put them there. [`with_error_correlation`] does,
//! for every failure this service answers: one a handler raised, and one the
//! request boundary made before any handler ran, alike. The pair is also
//! reported in the matching `x-amz-request-id` / `x-amz-id-2` response headers,
//! which is where a client that does not parse the body reads it.
//!
//! Where the elements BELONG is the vendored wrapped-restXml renderer, which
//! already writes both on its `noErrorWrapping` path beside those two headers.
//! That tree is read-only from here, and the framework's pre-dispatch renderer
//! for a boundary refusal lives in it too, so the elements are added to the
//! finished document instead. Two sibling services splice an envelope element
//! into their own document the same way —
//! `service-com-amazonaws-cloudfront-cloudfront2020-05-31`'s `service::envelope`
//! and `service-com-amazonaws-cloudformation-cloudformation`'s
//! `service::query_error_envelope` — so hoisting all three belongs with the
//! vendored change that makes them unnecessary rather than with this fix.
//!
//! [`Dispatch`] is also where two things this service decides about a request
//! are decided, both BEFORE the generated routing table gets it:
//!
//! * a request naming an account the caller holds no credentials for is refused
//!   ahead of everything else, because the account names whose control plane the
//!   request is for rather than a resource in it — see [`super::authorization`]
//!   for the rule;
//! * a request addressed to a sub-resource of a Multi-Region Access Point
//!   reaches the operation that sub-resource's route names, with the greedy
//!   label bound to the resource alone — see [`super::routes`] for the rule;
//! * a request that polls an asynchronous request by a token this service could
//!   not have minted is answered the refusal that says the token names no
//!   request, rather than the boundary's shape-validation refusal: a request
//!   token is an opaque identifier the service handed its caller, so it is
//!   RESOLVED against the requests this service holds rather than checked
//!   against a shape, and every token that resolves to nothing is the same
//!   not-found answer whatever its text. A client polling an operation by a
//!   token it did not construct then sees the 404 that means "unknown", not a
//!   400 it reads as its own bug;
//! * a request addressing a Storage Lens configuration by an id the wire cannot
//!   carry is answered as a URI this service could not parse, before an
//!   operation is named and so before anything is read, written or deleted;
//! * a request addressing a BUCKET in neither form those routes document — the
//!   bucket's S3 on Outposts ARN, or its name together with the Outpost header
//!   the REST API requires — is answered the same way, before the request's body
//!   is read at all: what the caller has to change is the resource it addressed,
//!   not the document it sent.
//!
//! Not every failure travels in the wrapped envelope: the Storage Lens
//! CONFIGURATION surface answers the refusals of its OWN resource in the FLAT
//! document `<Error><Code/><Message/>[<member/>…]<RequestId/><HostId/></Error>`
//! — one element, no `<ErrorResponse>` root and no `<Type>`, with the
//! correlation pair INSIDE it — which is what a client reads the code out of
//! there. [`Envelope`] is which of the two a failure travels in, and it is
//! decided from the request PATH rather than from the operation, so a failure
//! answered before an operation is named travels in the same envelope as one a
//! handler raised. A request whose URI this service could not PARSE is the one
//! exception: it addresses no resource at all, so it is stated in the service's
//! own envelope whatever path it carried.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_schema_server::xml_escape;
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, ErasedServiceDefinition, OperationContext, OperationRoutingTable,
    ServerHttpProtocol, Service, ServiceIdentity,
};

use super::{authorization, routes};

/// This service's `@xmlNamespace`, which its error envelope carries exactly as
/// its success documents do (`smithy_query(query="service_info")`:
/// `xmlNamespace.uri = http://awss3control.amazonaws.com/doc/2018-08-20/`).
const XML_NAMESPACE: &str = "http://awss3control.amazonaws.com/doc/2018-08-20/";

/// The envelope element that names the request a failure answers.
const REQUEST_ID_ELEMENT: &str = "RequestId";

/// The envelope element that carries the extended request id.
const HOST_ID_ELEMENT: &str = "HostId";

/// The response header mirroring [`REQUEST_ID_ELEMENT`].
const REQUEST_ID_HEADER: &str = "x-amz-request-id";

/// The response header mirroring [`HOST_ID_ELEMENT`].
const EXTENDED_REQUEST_ID_HEADER: &str = "x-amz-id-2";

/// The envelope root, whose close is where the missing elements are added.
const ERROR_RESPONSE_OPEN: &str = "<ErrorResponse";
const ERROR_RESPONSE_CLOSE: &str = "</ErrorResponse>";

/// `<Error><Type>`: a 4xx is the caller's fault, which restXml words `Sender`.
const SENDER: &str = "Sender";

/// The element a failure is stated in: the one the wrapped envelope holds, and
/// the root of the flat document.
const ERROR_ELEMENT: &str = "Error";

/// The element the wrapped envelope states which side is at fault in
/// ([`SENDER`]), and the flat document does not carry at all.
const TYPE_ELEMENT: &str = "Type";

/// The error code the framework names a request it refused before any handler
/// ran. Not a shape this service's model declares, which is why a response
/// carrying it is a refusal to restate rather than an S3 Control error.
const FRAMEWORK_REFUSAL_CODE: &str = "ValidationException";

/// The status the framework answers such a refusal with.
const REFUSED: u16 = 400;

/// The XML declaration the pre-dispatch renderer opens its envelope with, and
/// the protocol's own serializer never writes — see [`framework_refusal`].
const XML_DECLARATION: &str = "<?xml";

/// The path segment every route of this service is bound under: each route's
/// `http.uri` begins with it (`smithy_query(query="digest")`). A refusal the
/// Storage Lens surface answers names the resource path the request addressed
/// UNDER it, which is what [`addressed_resource`] takes off.
const API_VERSION_PREFIX: &str = "/v20180820/";

/// The resource a Storage Lens CONFIGURATION is addressed as, under
/// [`API_VERSION_PREFIX`]: `GET /v20180820/storagelens/{ConfigId}` and its
/// `/tagging` sub-resource, so the segment after this prefix is the
/// configuration id (`smithy_query(query="digest")`). The prefix ends in a
/// separator, so the Storage Lens GROUP routes under
/// `/v20180820/storagelensgroup` — a different resource, with its own name
/// rules and its own envelope — are not in this space.
const STORAGE_LENS_CONFIGURATION_RESOURCE: &str = "storagelens/";

/// The path space the asynchronous Multi-Region Access Point request
/// operations are bound under: `DescribeMultiRegionAccessPointOperation` is
/// `GET /v20180820/async-requests/mrap/{RequestTokenARN+}`, so a GET under this
/// prefix addresses one request token (`smithy_query(query="operation",
/// operation="DescribeMultiRegionAccessPointOperation")`).
const ASYNC_REQUEST_PATH: &str = "/v20180820/async-requests/mrap/";

/// How a refusal names the member it refused, as the request boundary states it
/// (`Value '<x>' at '<member>' failed to satisfy constraint: …`) — the member
/// path of `DescribeMultiRegionAccessPointOperationRequest$RequestTokenARN`.
const REQUEST_TOKEN_MEMBER: &str = "requestTokenARN";

/// The path space a Multi-Region Access Point is addressed in BY NAME: every
/// route that names one is `/v20180820/mrap/instances/{Name+}` or a
/// sub-resource of it (`.../policy`, `.../policystatus`), so the segment after
/// this prefix is the access point NAME (`smithy_query(query="digest")`). The
/// prefix ends in a separator, so `GET /v20180820/mrap/instances` — the
/// listing, which names none — is not in this space.
const MULTI_REGION_ACCESS_POINT_PATH: &str = "/v20180820/mrap/instances/";

/// The members a request states the Multi-Region Access Point NAME it is about
/// in, spelled as the request boundary states a member path: the `Name` of the
/// request document the asynchronous routes carry
/// (`CreateMultiRegionAccessPointInput$Name` under `Details`, and the same of
/// the delete and the policy put), and the `Name` label the routes that address
/// one by name carry (`GetMultiRegionAccessPointRequest$Name`, per
/// `smithy_query(query="operation")`). Each is a
/// `MultiRegionAccessPointName`, so each is held to that shape's rule.
const MULTI_REGION_ACCESS_POINT_NAME_MEMBERS: [&str; 2] = ["details.name", "name"];

/// The member a create states the bucket an access point is attached to in
/// (`CreateAccessPointRequest$Bucket`, which is `@required`, per
/// `smithy_query(query="operation", operation="CreateAccessPoint")`), spelled as
/// the request boundary states a member path.
const ACCESS_POINT_BUCKET_MEMBER: &str = "bucket";

/// The members a request's own ADDRESS is read from, as a refusal names them:
/// the configuration id its path carries and the account id its header carries
/// (`PutStorageLensConfigurationRequest$ConfigId` is an `@httpLabel` and
/// `$AccountId` an `@httpHeader`, per `smithy_query(query="operation")`),
/// spelled as the request boundary states a member path.
const ADDRESS_MEMBERS: [&str; 2] = ["configId", "accountId"];

/// The path space a Batch Operations job is addressed in: `CreateJob` and
/// `ListJobs` are bound on [`JOBS_COLLECTION_PATH`] itself and every per-job
/// route is that path with the job id and an optional sub-resource under it
/// (`smithy_query(query="digest")`), so a request addressing a job is one whose
/// path is the collection or lies under [`JOBS_PATH`]. The second ends in a
/// separator, so a sibling segment that merely starts with it is not claimed.
const JOBS_COLLECTION_PATH: &str = "/v20180820/jobs";
const JOBS_PATH: &str = "/v20180820/jobs/";

/// The path space an access point is addressed in: every route that names one
/// is `/v20180820/accesspoint/{Name}` or a sub-resource of it
/// (`.../policy`, `.../policyStatus`, `.../scope`), so the segment after this
/// prefix is the access point NAME
/// (`smithy_query(query="digest")`). The Object Lambda access points under
/// `/v20180820/accesspointforobjectlambda` are a different resource with its
/// own name rules and are NOT in this space — the prefix ends in a separator,
/// so a sibling segment that merely starts with it is not claimed.
const ACCESS_POINT_PATH: &str = "/v20180820/accesspoint/";

/// The path space a BUCKET is addressed in: every bucket route is
/// `/v20180820/bucket/{Bucket}` or a sub-resource of it, so what follows this
/// prefix is the bucket ADDRESS (`smithy_query(query="digest")`). The prefix ends
/// in a separator, so `GET /v20180820/bucket` — the regional-bucket listing,
/// which addresses no bucket — is not in this space.
const BUCKET_PATH: &str = "/v20180820/bucket/";

/// The sub-resources of a bucket this service's routes bind, as the trailing
/// literal segment of their URI templates (`.../{Bucket}/lifecycleconfiguration`,
/// `/policy`, `/replication`, `/tagging`, `/versioning`, per
/// `smithy_query(query="digest")`). What a bucket route's path carries BEFORE one
/// of these is the bucket's address, whether or not the separators inside it are
/// percent-encoded.
const BUCKET_SUB_RESOURCES: [&str; 5] = [
    "lifecycleconfiguration",
    "policy",
    "replication",
    "tagging",
    "versioning",
];

/// The header the REST API carries the Outpost a bucket lives on in. Every
/// bucket route documents it: "All Amazon S3 on Outposts REST API requests for
/// this action require an additional parameter of x-amz-outpost-id to be passed
/// with the request", and `CreateBucket` binds it as its `OutpostId` member
/// (`smithy_query(query="operation",operation="CreateBucket")`).
const OUTPOST_ID_HEADER: &str = "x-amz-outpost-id";

/// The bucket address `path` carries and the sub-resource it addresses under it,
/// or `None` for a path that addresses no bucket.
///
/// The address is read as one span rather than one segment: it carries a
/// URL-encoded ARN, and a client that leaves the ARN's own separators unescaped
/// writes it across several segments, so the trailing sub-resource segment is
/// what the address ends before.
fn bucket_address(path: &str) -> Option<(&str, Option<&str>)> {
    let rest = path.strip_prefix(BUCKET_PATH)?;
    let (address, sub_resource) = match rest.rsplit_once('/') {
        Some((before, last)) if BUCKET_SUB_RESOURCES.contains(&last) => (before, Some(last)),
        _ => (rest, None),
    };
    (!address.is_empty()).then_some((address, sub_resource))
}

/// Whether this request addresses a bucket in neither form the bucket routes
/// document, so there is no bucket for the path to address.
///
/// Those routes address an S3 on Outposts bucket, and their documentation states
/// two addressing forms: the bucket's ARN in the label — "you must specify the
/// ARN of the bucket accessed in the format
/// arn:aws:s3-outposts:::outpost//bucket/ … The value must be URL encoded" — or
/// its NAME in the label together with the [`OUTPOST_ID_HEADER`] the REST API
/// requires ("you must specify the name and the x-amz-outpost-id as well", see
/// `smithy_query(query="doc",shape="GetBucketRequest",member="Bucket")`). A
/// request carrying neither names no Outpost, no account and no Region, so the
/// URI cannot be parsed at all — a different answer from a bucket addressed
/// properly that merely does not exist.
///
/// `CreateBucket` is outside this reading: it is the one bucket route whose
/// label the model documents as "The name of the bucket", with the Outpost it is
/// created on carried in its own `OutpostId` member, so a plain name there is
/// the documented form and is left to that operation.
///
/// The escapes are the wire's rather than part of the address, so the span is
/// percent-decoded the way the codec that deserializes the label would decode it
/// ([`::service_common::percent_decode_path_label`]) before it is read.
fn bucket_address_unparsable(req: &RtRequest, path: &str) -> bool {
    let Some((address, sub_resource)) = bucket_address(path) else {
        return false;
    };
    // `PUT /v20180820/bucket/{Bucket}` is `CreateBucket`, the one route that
    // takes the bucket's name.
    if sub_resource.is_none() && req.method().eq_ignore_ascii_case("PUT") {
        return false;
    }
    if req.headers().get(OUTPOST_ID_HEADER).is_some() {
        return false;
    }
    let decoded = ::service_common::percent_decode_path_label(address);
    !crate::state::addressable_bucket(&decoded)
}

/// The fixed words the framework's own refusal envelope states a value it READ
/// and found to break a rule in (`N validation error(s) detected: …`).
///
/// A codec failure — a body that is not a document this protocol can parse, or
/// a value that is not of the type its member declares — has no structured
/// form, so it is rendered under the same code and status from the codec's own
/// sentence instead. The marker is what tells the two apart; see
/// [`refused_as_unreadable`].
const VALIDATION_ENVELOPE_MARKER: &str = "validation error";

/// The path component of a request URI, without the scheme, the authority or
/// the query.
fn request_path(uri: &str) -> &str {
    let after_authority = match uri.split_once("://") {
        None => uri,
        Some((_, after_scheme)) => match after_scheme.find('/') {
            None => "/",
            Some(path_start) => after_scheme.split_at(path_start).1,
        },
    };
    after_authority
        .split('?')
        .next()
        .unwrap_or(after_authority)
}

/// The query component of a request URI (everything after the first `?`), or
/// the empty string when it has none.
fn request_query(uri: &str) -> &str {
    uri.split_once('?').map_or("", |(_, query)| query)
}

/// The body of `resp` as text, when it is in memory and valid UTF-8.
fn body_text(resp: &RtResponse) -> Option<&str> {
    std::str::from_utf8(resp.body().bytes()?).ok()
}

/// The content of the first `<name>` element of `document`.
///
/// Enough for the envelope's own flat elements (`Code`, `Message`) and for its
/// one nesting level (the `Error` element, whose content is the markup of
/// everything the failure states), which is all this module reads: the
/// documents it reads are the ones this service just serialized.
fn element_text<'a>(document: &'a str, name: &str) -> Option<&'a str> {
    let after_open = document.split_once(&format!("<{name}>"))?.1;
    Some(after_open.split_once(&format!("</{name}>"))?.0)
}

/// `document` without the `<name>` element it carries, and unchanged when it
/// carries none.
///
/// What re-rooting a finished failure document drops: the elements the flat
/// form states elsewhere or not at all ([`as_flat_error`]). Removing an element
/// it does not carry is a no-op, which is what makes re-rooting a document
/// already in that form leave it alone.
fn without_element(document: &str, name: &str) -> String {
    match element_text(document, name) {
        Some(text) => document.replace(&format!("<{name}>{text}</{name}>"), ""),
        None => document.to_string(),
    }
}

/// The resource path a request addressed: its path without this service's API
/// version segment, which is how a Storage Lens refusal names the URI it could
/// not parse.
fn addressed_resource(path: &str) -> &str {
    path.strip_prefix(API_VERSION_PREFIX).unwrap_or(path)
}

/// The Storage Lens configuration id `path` addresses, or `None` for a path
/// that addresses no configuration — the Storage Lens listing route, which
/// names no id, and every other resource of this service included.
fn storage_lens_config_id(path: &str) -> Option<&str> {
    let rest = addressed_resource(path).strip_prefix(STORAGE_LENS_CONFIGURATION_RESOURCE)?;
    let config_id = rest.split('/').next().unwrap_or_default();
    (!config_id.is_empty()).then_some(config_id)
}

/// Which document a failure travels in.
///
/// Decided from the request PATH, for every failure answered once the resource
/// the request addresses is resolved: the boundary's, the handler's and this
/// module's own alike. A request whose URI this service could not PARSE has no
/// resolved resource, so its refusal is stated in [`Envelope::Wrapped`] — the
/// service's own envelope — whatever path it carried.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Envelope {
    /// `<ErrorResponse><Error>…</Error><RequestId/><HostId/></ErrorResponse>`,
    /// which the rest of this service answers in.
    Wrapped,
    /// `<Error>…<RequestId/><HostId/></Error>`, which the Storage Lens
    /// configuration surface answers in.
    Flat,
}

impl Envelope {
    /// The envelope a failure answered for `path` travels in.
    fn of(path: &str) -> Self {
        match storage_lens_config_id(path) {
            Some(_) => Self::Flat,
            None => Self::Wrapped,
        }
    }
}

/// `req` with its path replaced by `path`, so the operation a sub-resource
/// route names binds its `@httpLabel` members from the resource's own path (see
/// [`super::routes`]). Everything else about the request — its method, its
/// query and its headers — is carried through unchanged, because the route and
/// the input are read from all of them.
///
/// `None` when the request body is not in memory or the rewritten URI is not
/// one the HTTP layer can hold, which leaves the request to the generated
/// routing table rather than failing it.
fn with_bound_path(req: &RtRequest, path: &str) -> Option<RtRequest> {
    let query = request_query(req.uri());
    let uri = match query.is_empty() {
        true => path.to_string(),
        false => format!("{path}?{query}"),
    };
    let body = req.body().bytes()?.to_vec();
    let mut rebuilt = RtRequest::new(SdkBody::from(body));
    rebuilt.set_uri(uri.as_str()).ok()?;
    rebuilt.set_method(req.method()).ok()?;
    for (name, value) in req.headers().iter() {
        rebuilt
            .headers_mut()
            .append(name.to_string(), value.to_string());
    }
    Some(rebuilt)
}

/// The error envelope this service answers `status` / `code` / `message` with,
/// carrying its correlation pair and the members the refusal names what the
/// request named in.
///
/// The one renderer for a failure this module answers itself; a failure a
/// handler raised or the boundary refused is rendered by the protocol and
/// reaches the same envelope through [`with_error_correlation`] or
/// [`as_flat_error`].
fn error_response(
    envelope: Envelope,
    status: u16,
    code: &str,
    message: &str,
    details: &[(&str, &str)],
) -> RtResponse {
    let (request_id, host_id) = crate::state::new_error_correlation();
    let named: String = details
        .iter()
        .map(|(element, value)| {
            format!(
                "<{element}>{value}</{element}>",
                element = xml_escape(element),
                value = xml_escape(value)
            )
        })
        .collect();
    let code = xml_escape(code);
    let message = xml_escape(message);
    let correlation = format!(
        "<{REQUEST_ID_ELEMENT}>{request_id}</{REQUEST_ID_ELEMENT}>\
         <{HOST_ID_ELEMENT}>{host_id}</{HOST_ID_ELEMENT}>"
    );
    let body = match envelope {
        Envelope::Wrapped => format!(
            "<ErrorResponse xmlns=\"{namespace}\"><Error><Type>{SENDER}</Type><Code>{code}</Code>\
             <Message>{message}</Message>{named}</Error>{correlation}</ErrorResponse>",
            namespace = xml_escape(XML_NAMESPACE),
        ),
        // The flat document is one element, so the pair naming the request
        // sits inside it rather than beside it.
        Envelope::Flat => format!(
            "<Error><Code>{code}</Code><Message>{message}</Message>{named}{correlation}</Error>"
        ),
    };
    let mut resp = RtResponse::new(
        StatusCode::try_from(status)
            .unwrap_or(StatusCode::try_from(REFUSED).expect("400 is a valid status code")),
        SdkBody::from(body),
    );
    resp.headers_mut().insert("content-type", "application/xml");
    with_correlation_headers(resp, request_id, host_id)
}

/// `resp` reporting `request_id` and `host_id` in the two headers whose values
/// the envelope's own elements mirror.
fn with_correlation_headers(
    mut resp: RtResponse,
    request_id: String,
    host_id: String,
) -> RtResponse {
    resp.headers_mut().insert(REQUEST_ID_HEADER, request_id);
    resp.headers_mut()
        .insert(EXTENDED_REQUEST_ID_HEADER, host_id);
    resp
}

/// `resp` with its error envelope naming the request it answers.
///
/// A response that is not an error envelope, or one that already names the
/// request, is handed back untouched — so this is applied to EVERY response
/// without having to know which ones are errors.
pub fn with_error_correlation(mut resp: RtResponse) -> RtResponse {
    // A success is never an error envelope, whatever its document happens to
    // contain: the status is what says this response states a failure.
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let Some(body) = body_text(&resp) else {
        return resp;
    };
    if !body.contains(ERROR_RESPONSE_OPEN) || body.contains(&format!("<{REQUEST_ID_ELEMENT}>")) {
        return resp;
    }
    let Some(close_at) = body.rfind(ERROR_RESPONSE_CLOSE) else {
        return resp;
    };
    let (request_id, host_id) = crate::state::new_error_correlation();
    let mut rewritten = String::with_capacity(body.len() + 128);
    rewritten.push_str(body.split_at(close_at).0);
    rewritten.push_str(&format!(
        "<{REQUEST_ID_ELEMENT}>{request_id}</{REQUEST_ID_ELEMENT}>\
         <{HOST_ID_ELEMENT}>{host_id}</{HOST_ID_ELEMENT}>"
    ));
    rewritten.push_str(body.split_at(close_at).1);
    let length = rewritten.len();
    *resp.body_mut() = SdkBody::from(rewritten);
    // Only a response that already states its length gets the new one: a
    // renderer that left the framing to the transport keeps doing so.
    if resp.headers().get("content-length").is_some() {
        resp.headers_mut()
            .insert("content-length", length.to_string());
    }
    with_correlation_headers(resp, request_id, host_id)
}

/// `resp` with its failure document re-rooted as the FLAT one
/// ([`Envelope::Flat`]), naming the request it answers.
///
/// The protocol serializer and the request boundary both render a failure in
/// the wrapped envelope — the one the rest of this service answers in, and both
/// renderers live in the read-only vendored tree — so a refusal for a resource
/// whose surface states the flat document is re-rooted here rather than in a
/// second serializer. What the failure SAYS is carried through unchanged: every
/// element the `Error` element holds, less the `Type` the flat form does not
/// carry, and the correlation pair moved inside the one element the document
/// has. A response that is not a failure, or one already in that form, is
/// handed back saying exactly what it said — so this, like
/// [`with_error_correlation`], is applied to EVERY response of its surface
/// without having to know which ones are errors.
pub fn as_flat_error(mut resp: RtResponse) -> RtResponse {
    // A success is never an error envelope, whatever its document happens to
    // contain: the status is what says this response states a failure.
    if resp.status().as_u16() < 400 {
        return resp;
    }
    let Some(body) = body_text(&resp) else {
        return resp;
    };
    let Some(stated) = element_text(body, ERROR_ELEMENT) else {
        return resp;
    };
    // The pair naming the request: the one the document already carries,
    // wherever it carries it, and a new one for a document rendered without it.
    let (request_id, host_id) = match (
        element_text(body, REQUEST_ID_ELEMENT),
        element_text(body, HOST_ID_ELEMENT),
    ) {
        (Some(request_id), Some(host_id)) => (request_id.to_string(), host_id.to_string()),
        _ => crate::state::new_error_correlation(),
    };
    let stated = without_element(stated, TYPE_ELEMENT);
    let stated = without_element(&stated, REQUEST_ID_ELEMENT);
    let stated = without_element(&stated, HOST_ID_ELEMENT);
    let rewritten = format!(
        "<{ERROR_ELEMENT}>{stated}\
         <{REQUEST_ID_ELEMENT}>{request_id}</{REQUEST_ID_ELEMENT}>\
         <{HOST_ID_ELEMENT}>{host_id}</{HOST_ID_ELEMENT}></{ERROR_ELEMENT}>"
    );
    let length = rewritten.len();
    *resp.body_mut() = SdkBody::from(rewritten);
    // Only a response that already states its length gets the new one: a
    // renderer that left the framing to the transport keeps doing so.
    if resp.headers().get("content-length").is_some() {
        resp.headers_mut()
            .insert("content-length", length.to_string());
    }
    with_correlation_headers(resp, request_id, host_id)
}

/// `resp`'s body when `resp` is a refusal the request boundary made before any
/// handler ran, and `None` for anything else — including every refusal this
/// service's own handlers answer.
///
/// A handler's refusal can carry the framework's own error code too
/// ([`FRAMEWORK_REFUSAL_CODE`] is what `Scaffolded::validation` renders), so
/// the code alone does not say which layer refused: the document does. The
/// pre-dispatch renderer writes the XML DECLARATION ahead of its envelope and
/// the protocol's own serializer writes none, which is what tells a refusal
/// made before any handler ran from one a handler answered.
fn framework_refusal(resp: &RtResponse) -> Option<&str> {
    if resp.status().as_u16() != REFUSED {
        return None;
    }
    let body = body_text(resp)?;
    if !body.trim_start().starts_with(XML_DECLARATION) {
        return None;
    }
    (element_text(body, "Code")? == FRAMEWORK_REFUSAL_CODE).then_some(body)
}

/// Whether `refusal` is the boundary refusing the request over `member`.
///
/// Read off the refusal's own sentence, which names the member it refused: a
/// request under the same route refused for something else — the account id it
/// names, say — keeps the cause its caller has to fix. Both spellings of the
/// quotation marks are looked for, because the sentence is XML-escaped into the
/// envelope.
fn refused_over_member(refusal: &str, member: &str) -> bool {
    refusal_states(refusal, &format!("at '{member}'"))
}

/// Whether `refusal` is the boundary refusing the request because `member` was
/// ABSENT from it, rather than because a value it carried broke a rule.
///
/// The boundary states both in one sentence and tells them apart by the value it
/// echoes: an absent member has no value to echo, so the clause names `null`
/// where a value that broke a rule names the value itself
/// (`aws_smithy_schema_server::constraints`). A refusal of a value the member DID
/// carry is about that value, so it keeps the cause its caller has to fix.
fn refused_over_absent_member(refusal: &str, member: &str) -> bool {
    refusal_states(refusal, &format!("Value null at '{member}'"))
}

/// Whether `refusal`'s own sentence states `clause`.
///
/// Both spellings of the quotation marks are looked for, because the sentence is
/// XML-escaped into the envelope.
fn refusal_states(refusal: &str, clause: &str) -> bool {
    let Some(message) = element_text(refusal, "Message") else {
        return false;
    };
    message.contains(clause) || message.contains(&clause.replace('\'', "&apos;"))
}

/// Whether `refusal` is the boundary refusing a value it read out of the
/// request's own ADDRESS — the configuration id its path carries or the account
/// id its header carries — rather than out of the DOCUMENT the request
/// carried.
///
/// What tells the two apart on the Storage Lens configuration surface: a
/// document that surface will not take is one refusal
/// ([`crate::state::MALFORMED_XML_CODE`]), and a request whose address it could
/// not read is not about the document at all, so it keeps the cause its caller
/// has to fix.
fn refused_over_address(refusal: &str) -> bool {
    ADDRESS_MEMBERS
        .iter()
        .any(|member| refused_over_member(refusal, member))
}

/// Whether `refusal` is the boundary refusing the request over the Multi-Region
/// Access Point NAME it carried, on a path that is about one.
///
/// A name a Multi-Region Access Point may not be given is a request this
/// service will not carry out, stated in its own words rather than in the
/// framework's validation vocabulary — see
/// [`crate::state::INVALID_MULTI_REGION_ACCESS_POINT_NAME_MESSAGE`]. The path
/// is read as well as the member, so a request to another resource refused over
/// a member of the same name keeps the cause its caller has to fix.
fn refused_over_multi_region_access_point_name(path: &str, refusal: &str) -> bool {
    (path.starts_with(ASYNC_REQUEST_PATH) || path.starts_with(MULTI_REGION_ACCESS_POINT_PATH))
        && MULTI_REGION_ACCESS_POINT_NAME_MEMBERS
            .iter()
            .any(|member| refused_over_member(refusal, member))
}

/// Whether `refusal` is the boundary refusing the request because it could not
/// READ it, rather than because a value it did read broke a rule the model
/// states.
///
/// The two are rendered alike — 400 under the framework's own code — and only
/// the second has a structured form: an absent required member and a value
/// outside a declared bound are described as a `Refusal` and stated through one
/// envelope, while a codec failure carries the codec's own sentence. The
/// envelope's fixed words ([`VALIDATION_ENVELOPE_MARKER`]) are what tells them
/// apart.
fn refused_as_unreadable(refusal: &str) -> bool {
    element_text(refusal, "Message").is_some_and(|message| {
        !message.contains(VALIDATION_ENVELOPE_MARKER)
    })
}

/// Whether `path` names an access point by a name the wire cannot address, and
/// whether it names one an access point may not be given — with the value the
/// refusal's `URI` member carries.
///
/// Read off the PATH and not off a deserialized member: an access point name is
/// one segment of the request path, so the rules it is held to are answered
/// before any operation runs — for every route that addresses an access point
/// by name — and the two rules are answered apart (see
/// [`crate::state::addressable_access_point_name`] and
/// [`crate::state::valid_access_point_name`]). `None` for a path that names no
/// access point at all.
///
/// The two refusals name what the stage that answered them can name. A segment
/// LONGER than the wire can carry as a name
/// ([`crate::state::access_point_name_within_addressable_length`]) is not
/// something this service can read as a resource name at all, so the refusal
/// echoes the request target it was handed, verbatim. A segment of a length it
/// can read names a resource — whether or not the name is one an access point
/// may be given — so the refusal names that resource under the service root
/// ([`addressed_resource`]), which is what a caller reads the offending identity
/// out of.
fn access_point_name_refusal(path: &str) -> Option<(u16, &'static str, &'static str, &str)> {
    let rest = path.strip_prefix(ACCESS_POINT_PATH)?;
    let name = rest.split('/').next().unwrap_or_default();
    if name.is_empty() {
        return None;
    }
    let named = match crate::state::access_point_name_within_addressable_length(name) {
        true => addressed_resource(path),
        false => path,
    };
    if !crate::state::addressable_access_point_name(name) {
        return Some((
            crate::state::INVALID_URI_STATUS,
            crate::state::INVALID_URI_CODE,
            crate::state::INVALID_URI_MESSAGE,
            named,
        ));
    }
    if !crate::state::valid_access_point_name(name) {
        return Some((
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::INVALID_ACCESS_POINT_NAME_MESSAGE,
            named,
        ));
    }
    None
}

/// Whether `req` CREATES an access point: the one route bound as a `PUT` on an
/// access point's own path, with nothing under it — `PUT
/// /v20180820/accesspoint/{Name}`, per `smithy_query(query="operation",
/// operation="CreateAccessPoint")`. A `PUT` on a SUB-RESOURCE of the same path
/// (`…/policy`, `…/scope`) is a different operation with its own members.
fn creates_access_point(req: &RtRequest, path: &str) -> bool {
    req.method().eq_ignore_ascii_case("PUT")
        && path
            .strip_prefix(ACCESS_POINT_PATH)
            .is_some_and(|name| !name.is_empty() && !name.contains('/'))
}

/// This service's own answer to a refusal the boundary stated in the
/// framework's vocabulary, or `None` to keep the framework's rendering.
///
/// Reads the REQUEST and the REFUSAL, not the operation: a GET under the
/// asynchronous-request path space addresses one request token, and a token the
/// boundary would not read is one this service never minted, so it names no
/// request — which is the one refusal that says so (see the module docs). A
/// request whose BODY the codec could not read at all is restated as the one
/// refusal this service answers a request it will not carry out with, whatever
/// the member and whatever the value: the codec's own sentence is neither this
/// service's wording nor a code a client branches on. The Batch Operations job
/// surface answers that SAME refusal for every value the model does not admit,
/// read or unread, for the same reason.
fn restated(req: &RtRequest, resp: &RtResponse) -> Option<RtResponse> {
    let refusal = framework_refusal(resp)?;
    let path = request_path(req.uri());
    if req.method().eq_ignore_ascii_case("GET")
        && path.starts_with(ASYNC_REQUEST_PATH)
        && refused_over_member(refusal, REQUEST_TOKEN_MEMBER)
    {
        return Some(error_response(
            Envelope::of(path),
            crate::state::NO_SUCH_ASYNC_REQUEST_STATUS,
            crate::state::NO_SUCH_ASYNC_REQUEST_CODE,
            crate::state::NO_SUCH_ASYNC_REQUEST_MESSAGE,
            &[],
        ));
    }
    // A request about a Multi-Region Access Point whose NAME the boundary
    // refused: the name a Multi-Region Access Point may be given is a rule of
    // this service's own, so the refusal states it in this service's words and
    // under the code a client branches on, rather than restating the value and
    // the pattern back to the caller that sent them.
    if refused_over_multi_region_access_point_name(path, refusal) {
        return Some(error_response(
            Envelope::of(path),
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::INVALID_MULTI_REGION_ACCESS_POINT_NAME_MESSAGE,
            &[],
        ));
    }
    // The create of an access point that named no bucket at all: the bucket an
    // access point is attached to is a `@required` member, so its ABSENCE is
    // found by the boundary, but the refusal is still this operation's own — it
    // states what the caller has to add, under the code this service's refusals
    // carry, rather than in the framework's validation vocabulary, which is
    // neither this service's wording nor a code a client branches on. A value
    // the member DID carry and that broke one of its rules is a refusal about
    // that value, and is left where it is.
    if creates_access_point(req, path)
        && refused_over_absent_member(refusal, ACCESS_POINT_BUCKET_MEMBER)
    {
        return Some(error_response(
            Envelope::of(path),
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::MISSING_ACCESS_POINT_BUCKET_MESSAGE,
            &[],
        ));
    }
    // A Storage Lens configuration request the boundary refused over the
    // DOCUMENT it carried: that surface answers one refusal for a document it
    // will not take, whether the document could not be read at all or a value
    // it did read breaks a rule its shape states.
    if storage_lens_config_id(path).is_some() && !refused_over_address(refusal) {
        return Some(error_response(
            Envelope::Flat,
            crate::state::MALFORMED_XML_STATUS,
            crate::state::MALFORMED_XML_CODE,
            crate::state::MALFORMED_XML_MESSAGE,
            &[],
        ));
    }
    if refused_as_unreadable(refusal) {
        return Some(error_response(
            Envelope::of(path),
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::REQUEST_INVALID_MESSAGE,
            &[],
        ));
    }
    // A Batch Operations job request the boundary refused over a value the
    // model does not admit — a status outside its enum, a priority outside its
    // range, a reason past its length. The job surface answers ONE refusal for
    // a request it will not carry out, whatever the member and whatever the
    // value ([`crate::state::request_invalid`]): the framework's validation
    // vocabulary is neither a code this service answers nor wording a client
    // can branch on, and the sentence it builds restates the value and the rule
    // back to the caller that sent them.
    if path == JOBS_COLLECTION_PATH || path.starts_with(JOBS_PATH) {
        return Some(error_response(
            Envelope::of(path),
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::REQUEST_INVALID_MESSAGE,
            &[],
        ));
    }
    None
}

/// The generated definition, with the error envelope finished on the three
/// failures the router renders OUTSIDE the dispatch path: a request that names
/// no modelled operation, a transport-level failure, and a refusal made before
/// any operation runs. Each is the generated renderer's own document with the
/// envelope's correlation pair added, so a caller correlating a failure never
/// has to care which layer refused it.
///
/// Identity, the protocol and the routing table come from the generated
/// [`crate::ServiceDefinition`] untouched — nothing model-derived is restated
/// here.
pub struct S3ControlDefinition<Eng: Send + Sync + 'static> {
    entry: crate::ServiceDefinition<Eng>,
}

impl<Eng: Send + Sync + 'static> S3ControlDefinition<Eng> {
    /// Build the definition from the shared engine, which the generated entry
    /// threads into every operation.
    pub fn new(engine: ::std::sync::Arc<Eng>) -> Self {
        Self {
            entry: crate::ServiceDefinition::new(engine),
        }
    }

    /// Whether this request is one [`Dispatch`] serves: either the generated
    /// routing table resolves it, or it addresses a sub-resource whose route
    /// the table cannot name (see [`super::routes`]).
    pub fn routes(&self, req: &RtRequest) -> bool {
        let modelled = self
            .entry
            .protocol()
            .routing_key(req)
            .ok()
            .and_then(|key| self.entry.routing_table().get(key))
            .is_some();
        modelled || routes::resolve(req.method(), request_path(req.uri())).is_some()
    }
}

impl<Eng: Send + Sync + 'static> ErasedServiceDefinition for S3ControlDefinition<Eng> {
    fn identity(&self) -> &ServiceIdentity {
        self.entry.identity()
    }

    fn protocol(&self) -> &dyn ServerHttpProtocol {
        self.entry.protocol()
    }

    fn routing_table(&self) -> &OperationRoutingTable {
        self.entry.routing_table()
    }

    fn routing_error_response(&self, req: &RtRequest) -> RtResponse {
        with_error_correlation(self.entry.routing_error_response(req))
    }

    fn framework_error_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_correlation(self.entry.framework_error_response(error, req))
    }

    fn refusal_response(
        &self,
        error: aws_smithy_schema_server::ProtocolError,
        chosen: Option<smithy_server_runtime::Refused>,
        req: &RtRequest,
    ) -> RtResponse {
        with_error_correlation(ErasedServiceDefinition::refusal_response(
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

/// The modelled operation the request names, read from the model-derived
/// routing table (never from the service's own override, which routes straight
/// back here).
fn modelled_operation<'s>(
    svc: &'s dyn Service,
    req: &RtRequest,
) -> Option<&'s dyn ErasedOperation> {
    let key = svc.definition().protocol().routing_key(req).ok()?;
    svc.definition().routing_table().get(key)
}

/// The seam every request this service serves runs through: it resolves the
/// operation — including the sub-resource routes the generated table cannot
/// name — runs it unchanged, then finishes the response's error envelope (see
/// the module docs).
///
/// Stateless and engine-free: the operation it runs is looked up on the service
/// the router hands it, so this holds nothing per request and nothing per
/// engine.
#[derive(Debug, Default)]
pub struct Dispatch;

impl ErasedOperation for Dispatch {
    fn run<'r>(
        &'r self,
        svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            let path = request_path(req.uri());
            // The authorization stage, ahead of every other: the account a
            // request names says whose control plane the request is for, so one
            // naming an account this caller holds no credentials for is refused
            // before anything is parsed, read or written — which is what keeps
            // the answer the same whether or not the resource it names exists
            // (see [`super::authorization`]).
            if let Some(account) = authorization::unauthorized_account(req) {
                return error_response(
                    Envelope::of(path),
                    crate::state::ACCESS_DENIED_STATUS,
                    crate::state::ACCESS_DENIED_CODE,
                    crate::state::ACCESS_DENIED_MESSAGE,
                    &[(crate::state::ACCOUNT_ID_ELEMENT, account.as_str())],
                );
            }
            // The URI-parsing stage: a path that names an access point by a
            // name the wire cannot address is not a path this service can
            // parse, and one naming a name an access point may not be given is
            // a request it will not carry out. Both are answered here — before
            // an operation is named, and so before anything is read, created or
            // deleted.
            if let Some((status, code, message, named)) = access_point_name_refusal(path) {
                // What the `URI` member carries is the stage's own: the resource
                // the request addressed for a path whose segment this service
                // could read as a name, and the request target verbatim for one
                // too long to read as a name at all — see
                // [`access_point_name_refusal`]. A path this service could not
                // parse names no resource, so the refusal is stated in the
                // service's own envelope rather than in any resource's.
                return error_response(
                    Envelope::Wrapped,
                    status,
                    code,
                    message,
                    &[("URI", named)],
                );
            }
            // The same stage for the bucket space: those routes address an S3 on
            // Outposts bucket, by its ARN or by its name together with the
            // Outpost header the REST API requires, so a request carrying
            // neither addresses no bucket at all
            // ([`bucket_address_unparsable`]) — the path is refused here, before
            // an operation is named and so before the request's BODY is read,
            // because what the caller has to change is the resource it
            // addressed.
            if bucket_address_unparsable(req, path) {
                return error_response(
                    Envelope::Wrapped,
                    crate::state::INVALID_URI_STATUS,
                    crate::state::INVALID_URI_CODE,
                    crate::state::INVALID_URI_MESSAGE,
                    &[("URI", addressed_resource(path))],
                );
            }
            // The same stage for the Storage Lens configuration space: an id
            // the wire cannot carry
            // ([`crate::state::addressable_storage_lens_config_id`]) is not a
            // path this service can parse, so it is answered here — before an
            // operation is named, and so before any configuration is read,
            // written or deleted — naming the resource path the caller
            // addressed.
            if storage_lens_config_id(path)
                .is_some_and(|id| !crate::state::addressable_storage_lens_config_id(id))
            {
                return error_response(
                    Envelope::Wrapped,
                    crate::state::INVALID_URI_STATUS,
                    crate::state::INVALID_URI_CODE,
                    crate::state::INVALID_URI_MESSAGE,
                    &[("URI", addressed_resource(path))],
                );
            }
            // A sub-resource of a Multi-Region Access Point: the operation that
            // route names, handed the request with the route's trailing literal
            // segments taken off its path, so the greedy label binds the
            // resource alone.
            let sub_resource = routes::resolve(req.method(), request_path(req.uri())).and_then(
                |(operation, bound)| {
                    let op = svc.definition().routing_table().get(operation)?;
                    Some((op, with_bound_path(req, &bound)?))
                },
            );
            // The generated deserialize -> validate -> handle -> serialize
            // walk, run exactly as the router would have run it.
            let resp = match &sub_resource {
                Some((op, rebuilt)) => op.run(svc, rebuilt, ctx).await,
                None => match modelled_operation(svc, req) {
                    Some(op) => op.run(svc, req, ctx).await,
                    None => svc.definition().routing_error_response(req),
                },
            };
            let resp = restated(req, &resp).unwrap_or(resp);
            // Finished in the envelope this request's own resource answers in,
            // whichever layer rendered the document.
            match Envelope::of(path) {
                Envelope::Flat => as_flat_error(resp),
                Envelope::Wrapped => with_error_correlation(resp),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    //! The envelope surgery and the restatement condition, read off documents
    //! and requests rather than through the router (the crate's integration
    //! suite drives the same rules end to end over HTTP).

    use super::*;

    /// An error envelope as the PROTOCOL serializer renders one, for a failure a
    /// handler answered: no correlation pair, and no XML declaration.
    fn serialized_error(status: u16, code: &str, message: &str) -> RtResponse {
        let body = format!(
            "<ErrorResponse xmlns=\"{XML_NAMESPACE}\"><Error><Type>Sender</Type>\
             <Code>{code}</Code><Message>{message}</Message></Error></ErrorResponse>"
        );
        RtResponse::new(
            StatusCode::try_from(status).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    /// An error envelope as the PRE-DISPATCH renderer renders one, for a request
    /// refused before any handler ran: the same envelope behind the XML
    /// declaration that renderer opens with.
    fn boundary_refusal(code: &str, message: &str) -> RtResponse {
        let body = format!(
            "<?xml version=\"1.0\"?><ErrorResponse xmlns=\"{XML_NAMESPACE}\">\
             <Error><Type>Sender</Type><Code>{code}</Code><Message>{message}</Message>\
             </Error></ErrorResponse>"
        );
        RtResponse::new(
            StatusCode::try_from(REFUSED).expect("a valid status"),
            SdkBody::from(body),
        )
    }

    fn request(method: &str, uri: &str) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri(uri).expect("a valid uri");
        req.set_method(method).expect("a valid method");
        req
    }

    #[test]
    fn every_error_envelope_gains_the_correlation_pair_once() {
        let answered = with_error_correlation(serialized_error(
            404,
            "NoSuchMultiRegionAccessPoint",
            "The specified Multi-Region Access Point does not exist.",
        ));
        let body = body_text(&answered).expect("a text body").to_string();
        let request_id = element_text(&body, REQUEST_ID_ELEMENT).map(str::to_string);
        let host_id = element_text(&body, HOST_ID_ELEMENT).map(str::to_string);
        assert!(
            request_id.as_deref().is_some_and(|id| !id.is_empty()),
            "case \"a serialized refusal\": expected a RequestId element, got {body}"
        );
        assert!(
            host_id.as_deref().is_some_and(|id| !id.is_empty()),
            "case \"a serialized refusal\": expected a HostId element, got {body}"
        );
        assert_eq!(
            answered
                .headers()
                .get(REQUEST_ID_HEADER)
                .map(str::to_string),
            request_id,
            "case \"a serialized refusal\": the header must mirror the element, got {body}"
        );
        assert!(
            body.find("</Error>").is_some_and(|error_end| {
                body.find(&format!("<{REQUEST_ID_ELEMENT}>"))
                    .is_some_and(|at| at > error_end)
            }),
            "case \"a serialized refusal\": the pair sits beside Error, not inside it: {body}"
        );

        // Idempotent: a document that already names the request is untouched.
        let again = with_error_correlation(answered);
        assert_eq!(
            body_text(&again).map(str::to_string),
            Some(body.clone()),
            "case \"an envelope that already names the request\": expected no second pair"
        );
    }

    #[test]
    fn a_success_is_never_given_a_correlation_pair() {
        let mut ok = RtResponse::new(
            StatusCode::try_from(200u16).expect("200"),
            SdkBody::from("<GetMultiRegionAccessPointResult/>".to_string()),
        );
        ok.headers_mut().insert("content-type", "application/xml");
        let answered = with_error_correlation(ok);
        assert_eq!(
            body_text(&answered),
            Some("<GetMultiRegionAccessPointResult/>"),
            "case \"a success\": expected the document untouched"
        );
    }

    #[test]
    fn a_token_the_boundary_refused_is_restated_as_the_not_found_refusal() {
        let req = request("GET", "/v20180820/async-requests/mrap/not-an-arn");
        let refusal = boundary_refusal(
            FRAMEWORK_REFUSAL_CODE,
            "1 validation error detected: Value &apos;not-an-arn&apos; at \
             &apos;requestTokenARN&apos; failed to satisfy constraint: Member must satisfy \
             regular expression pattern: ^arn:.+$",
        );
        let restated = restated(&req, &refusal).expect("the refusal is restated");
        let body = body_text(&restated).expect("a text body").to_string();
        assert_eq!(
            (
                restated.status().as_u16(),
                element_text(&body, "Code").map(str::to_string)
            ),
            (
                crate::state::NO_SUCH_ASYNC_REQUEST_STATUS,
                Some(crate::state::NO_SUCH_ASYNC_REQUEST_CODE.to_string())
            ),
            "case \"a token that is not an ARN\": expected the not-found refusal, got {body}"
        );
        assert_eq!(
            element_text(&body, "Message"),
            Some(crate::state::NO_SUCH_ASYNC_REQUEST_MESSAGE),
            "case \"a token that is not an ARN\": expected the fixed message, got {body}"
        );
    }

    #[test]
    fn a_body_the_codec_could_not_read_is_restated_as_the_invalid_request_refusal() {
        // One row per way the codec can fail to READ a request: a value that is
        // not of the type its member declares, and a body that is not a
        // document at all. Neither has a structured form, so neither carries
        // the refusal envelope's own words.
        for (case, message) in [
            ("a boolean member carrying something else", "invalid boolean: &apos;maybe&apos;"),
            ("an integer member carrying something else", "invalid integer: &apos;soon&apos;"),
            ("a body that is not XML", "xml parse error: ill-formed document"),
        ] {
            let req = request("PUT", "/v20180820/accesspoint/reports-ap");
            let refusal = boundary_refusal(FRAMEWORK_REFUSAL_CODE, message);
            let restated = restated(&req, &refusal).expect("the refusal is restated");
            let body = body_text(&restated).expect("a text body").to_string();
            assert_eq!(
                (
                    restated.status().as_u16(),
                    element_text(&body, "Code").map(str::to_string),
                    element_text(&body, "Message").map(str::to_string),
                ),
                (
                    crate::state::REQUEST_INVALID_STATUS,
                    Some(crate::state::REQUEST_INVALID_CODE.to_string()),
                    Some(crate::state::REQUEST_INVALID_MESSAGE.to_string()),
                ),
                "case {case:?}: expected the invalid-request refusal, got {body}"
            );
        }
    }

    #[test]
    fn the_two_rules_an_access_point_name_is_held_to_are_answered_apart() {
        let shortest = *crate::state::ACCESS_POINT_NAME_LEN.start();
        let longest = *crate::state::ACCESS_POINT_NAME_LEN.end();
        let unparseable = Some((
            crate::state::INVALID_URI_STATUS,
            crate::state::INVALID_URI_CODE,
            crate::state::INVALID_URI_MESSAGE,
        ));
        let invalid_name = Some((
            crate::state::REQUEST_INVALID_STATUS,
            crate::state::REQUEST_INVALID_CODE,
            crate::state::INVALID_ACCESS_POINT_NAME_MESSAGE,
        ));
        // Each published bound and one past it, plus one row per rule the name
        // is held to: parsing the path reads the characters a segment carries
        // unencoded, within the length a name may have, and the parsed segment
        // is then held to the name's own rules.
        let cases = [
            ("at the shortest", "a".repeat(shortest), None),
            ("one under the shortest", "a".repeat(shortest - 1), invalid_name),
            ("one character", "a".to_string(), invalid_name),
            ("one inside the longest", "a".repeat(longest - 1), None),
            ("at the longest", "b".repeat(longest), None),
            ("one past the longest", "c".repeat(longest + 1), unparseable),
            ("far past the longest", "d".repeat(longest * 4), unparseable),
            ("digits and hyphens", "ap-7-9".to_string(), None),
            ("an upper-case letter", "Reports-ap".to_string(), unparseable),
            ("an underscore", "reports_ap".to_string(), unparseable),
            ("a dot", "reports.ap".to_string(), invalid_name),
            ("a leading hyphen", "-reports-ap".to_string(), invalid_name),
            ("a trailing hyphen", "reports-ap-".to_string(), invalid_name),
            ("the alias suffix", "reports-s3alias".to_string(), invalid_name),
            ("an escaped character", "reports%20ap".to_string(), unparseable),
            ("a sub-delimiter", "reports+ap".to_string(), unparseable),
            ("a character outside the set", "reports!ap".to_string(), unparseable),
        ];
        for (case, name, refusal) in cases {
            for path in [
                format!("/v20180820/accesspoint/{name}"),
                format!("/v20180820/accesspoint/{name}/policy"),
            ] {
                let answered = access_point_name_refusal(&path);
                let stated = answered.map(|(status, code, message, _)| (status, code, message));
                assert_eq!(
                    stated,
                    refusal,
                    "case {case:?} ({} characters) at {path:?}: expected {refusal:?}",
                    name.chars().count()
                );
                // The `URI` member names what the stage that refused can name:
                // the resource for a path whose segment this service could read
                // as a name, and the request target verbatim for a segment too
                // long to read as a name at all.
                let expected_uri = match stated {
                    None => None,
                    Some(_) if name.chars().count() > longest => Some(path.as_str()),
                    Some(_) => Some(addressed_resource(&path)),
                };
                assert_eq!(
                    answered.map(|(.., named)| named),
                    expected_uri,
                    "case {case:?} at {path:?}: expected the URI member to name \
                     {expected_uri:?}"
                );
            }
            assert_eq!(
                access_point_name_refusal(&format!(
                    "/v20180820/accesspointforobjectlambda/{name}"
                )),
                None,
                "case {case:?}: an Object Lambda access point is not in this path space"
            );
        }
        // The listing route names no access point at all.
        for path in ["/v20180820/accesspoint", "/v20180820/accesspoint/"] {
            assert_eq!(
                access_point_name_refusal(path),
                None,
                "case {path:?}: expected a path that names no access point to be served"
            );
        }
    }

    #[test]
    fn a_refusal_this_rule_does_not_own_keeps_its_own_wording() {        let cases: [(&str, RtRequest, RtResponse); 4] = [
            (
                "a refusal over another member of the same route",
                request("GET", "/v20180820/async-requests/mrap/arn:aws:s3:::request"),
                boundary_refusal(
                    FRAMEWORK_REFUSAL_CODE,
                    "1 validation error detected: Value &apos;nope&apos; at &apos;accountId&apos; \
                     failed to satisfy constraint: Member must satisfy regular expression \
                     pattern: ^\\d{12}$",
                ),
            ),
            (
                "a token refusal on another route",
                request("GET", "/v20180820/mrap/instances/reports-mrap"),
                boundary_refusal(
                    FRAMEWORK_REFUSAL_CODE,
                    "1 validation error detected: Value &apos;x&apos; at \
                     &apos;requestTokenARN&apos; failed to satisfy constraint: Member must \
                     satisfy regular expression pattern: ^arn:.+$",
                ),
            ),
            (
                "a refusal this service worded itself",
                request("GET", "/v20180820/async-requests/mrap/arn:aws:s3:::request"),
                serialized_error(404, "NoSuchAsyncRequest", "The specified request was not found."),
            ),
            // A handler's own refusal can carry the framework's error code; the
            // document says which layer answered, so the handler's wording
            // stands.
            (
                "a handler refusing the request it was given",
                request("GET", "/v20180820/accesspoint?nextToken=not-a-token"),
                serialized_error(
                    400,
                    FRAMEWORK_REFUSAL_CODE,
                    "NextToken is not a continuation token this service issued",
                ),
            ),
        ];
        for (case, req, resp) in cases {
            assert!(
                restated(&req, &resp).is_none(),
                "case {case:?}: expected the refusal to keep its own wording"
            );
        }
    }

    #[test]
    fn the_envelope_a_failure_travels_in_follows_the_resource_its_path_addresses() {
        // Each row is a path, and the envelope a failure answered for the
        // resource it addresses travels in: the Storage Lens CONFIGURATION
        // resource and its tagging sub-resource state the flat document, and
        // every other path space of this service — the Storage Lens listing
        // route and the Storage Lens GROUP resource, whose segment merely
        // starts with the same letters, included — states the wrapped
        // envelope.
        for (case, path, envelope) in [
            ("a configuration", "/v20180820/storagelens/dashboard", Envelope::Flat),
            (
                "a configuration's tag set",
                "/v20180820/storagelens/dashboard/tagging",
                Envelope::Flat,
            ),
            ("the configuration listing", "/v20180820/storagelens", Envelope::Wrapped),
            (
                "the configuration listing with a trailing separator",
                "/v20180820/storagelens/",
                Envelope::Wrapped,
            ),
            (
                "a Storage Lens group",
                "/v20180820/storagelensgroup/dev",
                Envelope::Wrapped,
            ),
            ("an access point", "/v20180820/accesspoint/reports-ap", Envelope::Wrapped),
        ] {
            assert_eq!(
                Envelope::of(path),
                envelope,
                "case {case:?} at {path:?}: expected {envelope:?}"
            );
        }
    }

    #[test]
    fn a_failure_re_rooted_as_the_flat_document_says_exactly_what_it_said() {
        // One row per renderer a failure can arrive from: the protocol's, the
        // pre-dispatch one, this module's own wrapped envelope (correlation
        // pair beside the error element), and a document already in the flat
        // form — which must come out of a second re-rooting unchanged.
        let already_flat = as_flat_error(serialized_error(
            404,
            "NoSuchConfiguration",
            "The specified configuration does not exist.",
        ));
        let flat_body = body_text(&already_flat).expect("a text body").to_string();
        for (case, resp) in [
            (
                "a refusal the protocol serialized",
                serialized_error(404, "NoSuchConfiguration", "not there"),
            ),
            (
                "a refusal the boundary rendered",
                boundary_refusal(FRAMEWORK_REFUSAL_CODE, "1 validation error detected: nope"),
            ),
            (
                "a refusal this module answered",
                error_response(Envelope::Wrapped, 400, "MalformedXML", "no good", &[]),
            ),
            (
                "a refusal naming what the request named",
                error_response(
                    Envelope::Wrapped,
                    400,
                    "InvalidURI",
                    "Couldn't parse the specified URI.",
                    &[("URI", "storagelens/a.b")],
                ),
            ),
            ("a document already in the flat form", already_flat),
        ] {
            let before = body_text(&resp).expect("a text body").to_string();
            let code = element_text(&before, "Code").map(str::to_string);
            let message = element_text(&before, "Message").map(str::to_string);
            let named = element_text(&before, "URI").map(str::to_string);
            let answered = as_flat_error(resp);
            let body = body_text(&answered).expect("a text body").to_string();
            assert!(
                body.starts_with(&format!("<{ERROR_ELEMENT}>"))
                    && body.ends_with(&format!("</{ERROR_ELEMENT}>"))
                    && !body.contains(ERROR_RESPONSE_OPEN)
                    && !body.contains(&format!("<{TYPE_ELEMENT}>")),
                "case {case:?}: expected the flat document, got {body}"
            );
            assert_eq!(
                (
                    element_text(&body, "Code").map(str::to_string),
                    element_text(&body, "Message").map(str::to_string),
                    element_text(&body, "URI").map(str::to_string),
                ),
                (code, message, named),
                "case {case:?}: the re-rooted document must say what it said, got {body}"
            );
            assert_eq!(
                body.matches(&format!("<{REQUEST_ID_ELEMENT}>")).count(),
                1,
                "case {case:?}: expected exactly one correlation pair, got {body}"
            );
            assert_eq!(
                answered
                    .headers()
                    .get(REQUEST_ID_HEADER)
                    .map(str::to_string),
                element_text(&body, REQUEST_ID_ELEMENT).map(str::to_string),
                "case {case:?}: the header must mirror the element, got {body}"
            );
        }
        // Re-rooting a document already in the flat form leaves it alone,
        // correlation pair included.
        assert_eq!(
            body_text(&as_flat_error(as_flat_error(RtResponse::new(
                StatusCode::try_from(404u16).expect("a valid status"),
                SdkBody::from(flat_body.clone()),
            ))))
            .map(str::to_string),
            Some(flat_body),
            "case \"a flat document re-rooted twice\": expected it unchanged"
        );
    }

    #[test]
    fn a_success_is_never_re_rooted() {
        let ok = RtResponse::new(
            StatusCode::try_from(200u16).expect("200"),
            SdkBody::from("<GetStorageLensConfigurationResult/>".to_string()),
        );
        assert_eq!(
            body_text(&as_flat_error(ok)),
            Some("<GetStorageLensConfigurationResult/>"),
            "case \"a success\": expected the document untouched"
        );
    }

    #[test]
    fn a_storage_lens_document_the_boundary_refused_is_restated_as_malformed() {
        // A refusal over the DOCUMENT is restated; one over the request's own
        // address — the id in its path, the account in its header — keeps the
        // cause its caller has to fix.
        for (case, message, restated_as_malformed) in [
            (
                "a body the codec could not read",
                "xml parse error: ill-formed document".to_string(),
                true,
            ),
            (
                "a required member of the document left out",
                "1 validation error detected: Value null at \
                 &apos;storageLensConfiguration&apos; failed to satisfy constraint: Member must \
                 not be null"
                    .to_string(),
                true,
            ),
            (
                "a value in the document outside its declared range",
                "1 validation error detected: Value 11 at \
                 &apos;storageLensConfiguration.accountLevel&apos; failed to satisfy constraint: \
                 Member must be between 1 and 10"
                    .to_string(),
                true,
            ),
            (
                "the configuration id in the path",
                format!(
                    "1 validation error detected: Value &apos;x&apos; at &apos;{}&apos; failed \
                     to satisfy constraint: Member must have length less than or equal to 64",
                    ADDRESS_MEMBERS[0]
                ),
                false,
            ),
            (
                "the account id in the header",
                format!(
                    "1 validation error detected: Value &apos;x&apos; at &apos;{}&apos; failed \
                     to satisfy constraint: Member must satisfy regular expression pattern: \
                     ^\\d{{12}}$",
                    ADDRESS_MEMBERS[1]
                ),
                false,
            ),
        ] {
            for path in [
                "/v20180820/storagelens/dashboard",
                "/v20180820/storagelens/dashboard/tagging",
            ] {
                let req = request("PUT", path);
                let refusal = boundary_refusal(FRAMEWORK_REFUSAL_CODE, &message);
                let answered = restated(&req, &refusal);
                let stated = answered.as_ref().and_then(|resp| {
                    body_text(resp).and_then(|body| element_text(body, "Code").map(str::to_string))
                });
                let expected = restated_as_malformed
                    .then(|| crate::state::MALFORMED_XML_CODE.to_string());
                assert_eq!(
                    stated, expected,
                    "case {case:?} at {path:?}: expected {expected:?}"
                );
            }
        }
    }
}
