// SPDX-License-Identifier: CC0-1.0

//! The endpoint's own HTTP surface: what this service answers a request it names no
//! operation for, and the media type its answers are announced in.
//!
//! A DynamoDB endpoint is an HTTP endpoint before it is an API: the request reaches a front
//! door that decides, from the request line and the headers ALONE, whether the request is
//! one the endpoint routes to an operation at all. Three decisions live there, and all
//! three happen ahead of any operation of this model:
//!
//! * the METHOD SURFACE of the endpoint ROOT. The API is served on one method at the root
//!   path; the root reached with the method a client probes an endpoint's health with
//!   answers a plain-text report of that endpoint's health, naming the regional endpoint the
//!   request addressed, and the root reached with any other method — or any other path, with
//!   any method — is a route the endpoint binds nothing for ([`unserved`]).
//! * the MEDIA TYPE of the request. The endpoint serves its API under the JSON media types
//!   it publishes and no others; a request announcing any other media type — or none at all
//!   — is not routed to an operation, and is answered as a route the endpoint binds nothing
//!   for, which is a different answer from refusing a request an operation DID receive
//!   ([`serves_media_type`]).
//! * the TARGET. A request whose `x-amz-target` names no operation this endpoint binds —
//!   including one that does not have the `Service.Operation` form at all — is refused by
//!   the front door in ITS own error vocabulary ([`UNKNOWN_OPERATION_TYPE`]), not in this
//!   service's modelled one: no operation was identified, so there is no operation to
//!   answer in. The front door has no notion of an operation that is modelled but
//!   unserved: a target it cannot bind is simply unknown.
//!
//! # What each answer is, and what it is not
//!
//! A route the endpoint binds nothing for is a MISSING ROUTE: HTTP 404 whose whole body is
//! [`smithy_server_runtime::UNRECOGNISED_PROTOCOL_BODY`], the bare element naming the fault
//! — no envelope, no operation, no member and no value, because the front door read none.
//! That is deliberately not the 400 the front door answers a request it DID read and could
//! name no operation in, and it is also not the 400 an operation answers a request it
//! received and found invalid: a client that distinguishes "no such route or media type"
//! from "bad request for this operation" reads the status.
//!
//! An unknown target is read, so it is answered in the front door's own JSON envelope with
//! [`UNKNOWN_OPERATION_STATUS`] and no `message` member at all: the front door identified
//! no operation, so there is no name, path, member or value for a sentence to state, and
//! this error class interpolates none — the whole body is the template. The shape it names
//! belongs to the front door rather than to any DynamoDB model, which is why no model
//! declares it and why no `x-amzn-errortype` names a shape of this service's.
//!
//! # The media type a response announces
//!
//! The endpoint serves one wire protocol under more than one media type, and a response
//! announces the media type the REQUEST named it under: a client that asked in
//! `application/json` is answered in `application/json`, and one that asked in the
//! protocol's own `application/x-amz-json-1.0` is answered in that. The protocol, the
//! routing and every byte of the body are untouched by this — the awsJson codec reads and
//! writes the same JSON either way — so [`announced`] re-announces that ONE header and
//! changes nothing else. A response the front door itself wrote is announced the same way,
//! because which LAYER answered does not change the media type a client asked to be
//! answered in.

use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{
    BoxFuture, ErasedOperation, OperationContext, Service, UNRECOGNISED_PROTOCOL_BODY,
};

/// The response header a media type is announced in.
const CONTENT_TYPE: &str = "content-type";

/// The media type the `awsJson1_0` codec stamps on this service's responses, and one of the
/// two a request may name the API under — the media type that protocol trait implies, from
/// the tree's one statement of it. Every media type of the versioned `awsJson` family begins
/// `application/x-amz-json-`, and this endpoint serves exactly ONE version of it: a request
/// announcing another version announces a protocol this endpoint does not speak.
const PROTOCOL_MEDIA_TYPE: &str = ::service_common::response_media_type::AWS_JSON_1_0;

/// The other: the plain JSON media type, under which the same endpoint serves the same
/// protocol.
const PLAIN_JSON_MEDIA_TYPE: &str = "application/json";

/// The method this endpoint serves its API on: every `awsJson` call is a POST, which is
/// what the protocol itself requires. Matched exactly, because an HTTP method is a
/// case-sensitive token and this endpoint serves no other spelling of it.
const API_METHOD: &str = "POST";

/// The method the endpoint root reports its health under — the method a health probe or a
/// load balancer reads an endpoint's root with. Matched exactly, for the same reason.
const HEALTH_METHOD: &str = "GET";

/// The path of the endpoint root, which is the one path this endpoint binds anything at:
/// the API is served there and so is the health report, so the two are told apart by the
/// METHOD, and any other path is bound by nothing.
const ROOT_PATH: &str = "/";

/// The fixed part of the health report the root answers: the word, its separator, and the
/// one space the report ends with. The only part that is not fixed is the endpoint the
/// report is about.
const HEALTH_PREFIX: &str = "healthy: ";

/// The space that closes the report, after the endpoint's name.
const HEALTH_SUFFIX: &str = " ";

/// The first label of every endpoint host this service is served on: its `aws.api#service`
/// `endpointPrefix` (`smithy_query(query="service_info")` on this crate's model —
/// `endpointPrefix: "dynamodb"`), which is also the SigV4 signing name a request addresses
/// it by and the prefix `DescribeEndpoints` lays its own addresses out under.
const ENDPOINT_PREFIX: &str = "dynamodb";

/// What follows the region label in the name of every regional endpoint of this partition.
const ENDPOINT_SUFFIX: &str = "amazonaws.com";

/// The media type the health report is written in: it is a plain-text sentence for a
/// human or a probe to read, not a document of this API.
const HEALTH_MEDIA_TYPE: &str = "text/plain";

/// The status a request the endpoint binds no route for is answered with: the request
/// reached an endpoint that serves nothing at that method and media type, which is a
/// missing route rather than a request an operation read and refused.
const UNROUTED_STATUS: u16 = 404;

/// The status the front door refuses a request whose target it cannot bind with: the
/// request was read and names no operation, which is a fault in the request rather than a
/// missing route — and a terminal one, so a client does not retry a target that can never
/// be served.
const UNKNOWN_OPERATION_STATUS: u16 = 400;

/// The shape the front door names for such a request, in the namespace the front door
/// itself declares — no DynamoDB model binds this shape, so it is stated here rather than
/// serialized through an error schema of this service's.
const UNKNOWN_OPERATION_TYPE: &str = "com.amazon.coral.service#UnknownOperationException";

/// Which answer the endpoint's own surface owns for a request, when it owns one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unserved {
    /// The endpoint root, reached with the method its health is read under: the plain-text
    /// health report.
    Health,
    /// A method, path and media type this endpoint binds no route for at all.
    Unroutable,
}

/// The answer the endpoint's own surface owns for `req`, or `None` when the request is one
/// the API serves — in which case the target decides which operation reads it.
///
/// Read from the request line and the headers alone, exactly as a front door decides it:
/// nothing here reads the body, and nothing here reads or changes any stored state.
pub fn unserved(req: &RtRequest) -> Option<Unserved> {
    // The PATH of the request target, whichever form the target takes: a client given an
    // endpoint may send the absolute form (`GET http://host/ HTTP/1.1`), and the router hands
    // the target over as it arrived, so the one reading of this in the tree is what resolves
    // it ([`::service_common::request_path`]).
    let root = ::service_common::request_path(req.uri()) == ROOT_PATH;
    if !root || req.method() != API_METHOD {
        return Some(if root && req.method() == HEALTH_METHOD {
            Unserved::Health
        } else {
            Unserved::Unroutable
        });
    }
    match requested_media_type(req) {
        Some(media_type) if serves_media_type(&media_type) => None,
        _ => Some(Unserved::Unroutable),
    }
}

/// The media type `req` announces its body under: the `content-type` header with its
/// parameters (`; charset=…`) dropped and its case folded, which is how a media type is
/// compared. `None` when the request announces no media type at all, which is not the
/// announcement of one the API is served under either.
fn requested_media_type(req: &RtRequest) -> Option<String> {
    let announced = req.headers().get(CONTENT_TYPE)?;
    let media_type = announced.split(';').next().unwrap_or(announced).trim();
    (!media_type.is_empty()).then(|| media_type.to_ascii_lowercase())
}

/// Whether `media_type` is one of the JSON media types this endpoint serves its API under:
/// the plain one, and the one its own protocol version publishes. Another version of the
/// `awsJson` family is a protocol this endpoint does not speak, so it is no more routable
/// here than a media type outside JSON altogether.
fn serves_media_type(media_type: &str) -> bool {
    media_type == PLAIN_JSON_MEDIA_TYPE || media_type == PROTOCOL_MEDIA_TYPE
}

/// `resp` announcing the media type `req` named this endpoint's API under, where that is
/// not the one the protocol's own renderer stamped.
///
/// Only a response the PROTOCOL announced is re-announced: a response already carrying some
/// other media type — the health report's plain text, the front door's own envelope — was
/// produced by a layer that announced it itself, and is left exactly as it was. The status,
/// every other header and every byte of the body are untouched either way.
pub fn announced(req: &RtRequest, mut resp: RtResponse) -> RtResponse {
    if resp.headers().get(CONTENT_TYPE) != Some(PROTOCOL_MEDIA_TYPE) {
        return resp;
    }
    let asked = match requested_media_type(req) {
        Some(media_type) if serves_media_type(&media_type) => media_type,
        _ => return resp,
    };
    if asked == PROTOCOL_MEDIA_TYPE {
        return resp;
    }
    let _ = resp.headers_mut().try_insert(CONTENT_TYPE, asked);
    resp
}

/// The health report the endpoint root answers: the fixed sentence over the name of the
/// endpoint the report is about, as plain text.
///
/// The endpoint a report names is the one the request ADDRESSED, which is a regional
/// endpoint of this service: its name is the service's endpoint prefix, the region the
/// request names, and the suffix every regional endpoint of the partition carries. The
/// region is resolved exactly the way every other request of this service resolves the
/// region it addresses ([`crate::state::request_region`]) — a resolution that admits only a
/// region-shaped label and otherwise answers the default region, so the report always names
/// a syntactically real endpoint — and two probes of two regional endpoints therefore read
/// back the endpoint each of them is talking to.
///
/// It is the endpoint's NAME in a report ABOUT that endpoint, not an address for a caller
/// to send anything to, which is why this is not
/// [`::service_common::data_plane_addressing::reached_endpoint_host`]: that one answers the
/// other question (a URL a caller must be able to reach back) and so prefers the host the
/// request arrived at, which would make this report name the loopback address a client
/// reaches this emulator at rather than the endpoint it addressed. `DescribeEndpoints` is
/// where this service answers an address a client should send its next request to, and that
/// one does resolve to this emulator.
fn health_response(parts: &::http::request::Parts) -> RtResponse {
    let endpoint = addressed_endpoint(parts);
    let mut resp = RtResponse::new(
        StatusCode::try_from(200).expect("the status is a constant of this module"),
        SdkBody::from(format!("{HEALTH_PREFIX}{endpoint}{HEALTH_SUFFIX}")),
    );
    resp.headers_mut().insert(CONTENT_TYPE, HEALTH_MEDIA_TYPE);
    resp
}

/// The DNS name of the regional endpoint `parts` addresses.
fn addressed_endpoint(parts: &::http::request::Parts) -> String {
    let region = crate::state::request_region(parts);
    format!("{ENDPOINT_PREFIX}.{region}.{ENDPOINT_SUFFIX}")
}

/// What this endpoint answers a method, path and media type it binds NO route for: the 404
/// whose whole body is the bare element naming the fault.
///
/// Nothing in it comes from the request: no route matched, so there is no operation, path,
/// member or value to report — and no media type is announced, because the body is not a
/// document of this API.
pub fn unroutable_response() -> RtResponse {
    RtResponse::new(
        StatusCode::try_from(UNROUTED_STATUS).expect("the status is a constant of this module"),
        SdkBody::from(UNRECOGNISED_PROTOCOL_BODY),
    )
}

/// What the front door refuses a request whose `x-amz-target` names no operation this
/// endpoint binds with — including a target that does not parse as `Service.Operation`.
///
/// The body is the whole template: the class alone, with no `message` member, because the
/// front door identified no operation and this class interpolates nothing. No
/// `x-amzn-errortype` header either: the awsJson family conveys the class in the body's
/// `__type`, which is where it is written. It is announced in the media type `req` named
/// the API under, exactly as an operation's own answer is: which LAYER answered does not
/// change the media type a client asked to be answered in.
pub fn unknown_operation_response(req: &RtRequest) -> RtResponse {
    let mut resp = RtResponse::new(
        StatusCode::try_from(UNKNOWN_OPERATION_STATUS)
            .expect("the status is a constant of this module"),
        SdkBody::from(unknown_operation_body()),
    );
    resp.headers_mut().insert(CONTENT_TYPE, PROTOCOL_MEDIA_TYPE);
    announced(req, resp)
}

/// That refusal's bytes, written through the JSON writer rather than as a literal, so the
/// one class name above is the only place its text lives.
fn unknown_operation_body() -> String {
    ::serde_json::json!({ "__type": UNKNOWN_OPERATION_TYPE }).to_string()
}

/// The endpoint's own surface, as the one [`ErasedOperation`] this service hands back for a
/// request no operation of it serves (`super::EmulatedService::override_operation`).
///
/// Stateless: every answer it gives is computed from the request line and the headers, and
/// nothing it is handed is read or written — no table, no item, no stored state at all.
#[derive(Debug, Clone, Copy)]
pub struct EndpointSurface;

impl ErasedOperation for EndpointSurface {
    fn run<'r>(
        &'r self,
        _svc: &'r dyn Service,
        req: &'r RtRequest,
        ctx: &'r OperationContext<'r>,
    ) -> BoxFuture<'r, RtResponse> {
        Box::pin(async move {
            match unserved(req) {
                // The front door's own answers name the request they answered exactly as
                // every other response of this service does; they carry no body checksum,
                // because neither is a document of this API.
                Some(Unserved::Health) => {
                    super::request_id::with_request_id_header(health_response(ctx.parts()))
                }
                Some(Unserved::Unroutable) => {
                    super::request_id::with_request_id_header(unroutable_response())
                }
                // `override_operation` hands this surface back only for a request
                // [`unserved`] owns, so nothing reaches this arm today. A request that is
                // routable at the HTTP layer and still names no operation is the unknown
                // TARGET above, which is what it would be answered with — reported like
                // every other document of this API.
                None => super::reported(unknown_operation_response(req)),
            }
        })
    }

    fn auth_schemes(&self) -> Option<&'static [&'static str]> {
        // `None` is "defer to the modelled operation the routing key names", so a request
        // that names one is signed exactly as that operation's model declares, and this
        // surface cannot weaken any scheme.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, uri: &str, content_type: Option<&str>) -> RtRequest {
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_method(method).expect("a valid method");
        req.set_uri(uri).expect("a valid uri");
        if let Some(media_type) = content_type {
            req.headers_mut()
                .insert(CONTENT_TYPE, media_type.to_string());
        }
        req
    }

    /// One row of the table below: the case, the method, the path, the media type the
    /// request announces, and the answer the endpoint's own surface owns for it.
    type Row = (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
        Option<Unserved>,
    );

    /// Which requests the endpoint's own surface owns, over the method surface and the
    /// media types a client can announce.
    #[test]
    fn the_surface_owns_every_request_no_operation_serves() {
        // (case, method, uri, content-type, the answer the surface owns)
        let rows: [Row; 19] = [
            (
                "the API, under the protocol's own media type",
                "POST",
                "/",
                Some("application/x-amz-json-1.0"),
                None,
            ),
            (
                "the API, addressed in the absolute form a client given an endpoint sends",
                "POST",
                "http://dynamodb.us-east-1.amazonaws.com/",
                Some("application/x-amz-json-1.0"),
                None,
            ),
            (
                "the API, addressed in the absolute form with no trailing slash",
                "POST",
                "http://dynamodb.us-east-1.amazonaws.com",
                Some("application/x-amz-json-1.0"),
                None,
            ),
            (
                "the API, under the plain JSON media type",
                "POST",
                "/",
                Some("application/json"),
                None,
            ),
            (
                "the API, under a parameterised JSON media type",
                "POST",
                "/",
                Some("application/json; charset=utf-8"),
                None,
            ),
            (
                "the API, under another version of the awsJson family, which this endpoint \
                 does not speak",
                "POST",
                "/",
                Some("application/x-amz-json-1.1"),
                Some(Unserved::Unroutable),
            ),
            (
                "the API, announcing no media type at all",
                "POST",
                "/",
                None,
                Some(Unserved::Unroutable),
            ),
            (
                "plain text, which is not a JSON media type",
                "POST",
                "/",
                Some("text/plain"),
                Some(Unserved::Unroutable),
            ),
            (
                "XML, which is not a JSON media type",
                "POST",
                "/",
                Some("application/xml"),
                Some(Unserved::Unroutable),
            ),
            (
                "an octet stream, which is not a JSON media type",
                "POST",
                "/",
                Some("application/octet-stream"),
                Some(Unserved::Unroutable),
            ),
            (
                "the root, read for its health",
                "GET",
                "/",
                None,
                Some(Unserved::Health),
            ),
            (
                "the root, read for its health in the absolute form",
                "GET",
                "https://dynamodb.eu-west-1.amazonaws.com/",
                None,
                Some(Unserved::Health),
            ),
            (
                "the root, read for its health with a query string",
                "GET",
                "/?probe=1",
                None,
                Some(Unserved::Health),
            ),
            (
                "the root, read for its health under a media type the API does not serve",
                "GET",
                "/",
                Some("text/plain"),
                Some(Unserved::Health),
            ),
            (
                "a path the endpoint binds nothing for",
                "GET",
                "/ping",
                None,
                Some(Unserved::Unroutable),
            ),
            (
                "the API's own method at a path the endpoint binds nothing for",
                "POST",
                "/ping",
                Some("application/x-amz-json-1.0"),
                Some(Unserved::Unroutable),
            ),
            (
                "the API's own method at a path the endpoint binds nothing for, in the \
                 absolute form",
                "POST",
                "http://dynamodb.us-east-1.amazonaws.com/ping",
                Some("application/x-amz-json-1.0"),
                Some(Unserved::Unroutable),
            ),
            (
                "the API's own method spelled in lower case, which is another method",
                "post",
                "/",
                Some("application/x-amz-json-1.0"),
                Some(Unserved::Unroutable),
            ),
            (
                "a method the endpoint does not serve",
                "DELETE",
                "/",
                Some("application/x-amz-json-1.0"),
                Some(Unserved::Unroutable),
            ),
        ];

        for (case, method, uri, content_type, expected) in rows {
            let actual = unserved(&request(method, uri, content_type));
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }
}
