// SPDX-License-Identifier: CC0-1.0

//! What EVERY one of this service's data planes reads and answers with: how an endpoint's
//! host and path are read, how an authorizer's identity source is looked up in a request,
//! and the `{"message": …}` envelope an answer is rendered in.
//!
//! API Gateway v2 serves two protocols and a developer portal from one control plane, and
//! each has a data plane of its own — [`super::websocket`] for the RFC 6455 opening
//! handshake of a WEBSOCKET api, [`super::invoke`] for a request to a route of an HTTP api,
//! [`super::portal`] for the website a published portal serves — and the two api planes are
//! addressed at the SAME endpoint shape, the one `Api$ApiEndpoint` documents
//! (`{api-id}.execute-api.{region}.amazonaws.com`, "the stage name … appended to this URI
//! to form a complete path to a deployed API stage"), while the portal's is the domain
//! `EndpointConfigurationResponse$PortalDefaultDomainName` reports. All of them answer in
//! the same envelope. This module owns those shared readings so the planes cannot disagree
//! about which host is one of this service's endpoints, which path space belongs to another
//! service, or what a refusal looks like on the wire.
//!
//! Everything here is a PURE reading of a request plus stored values: nothing in this
//! module touches state, and every function is total — a host, path or expression it
//! cannot read answers `None` rather than panicking on a caller's input.

use std::collections::BTreeMap;

use aws_smithy_runtime_api::http::{Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::http;

/// The second label of every `execute-api` endpoint host: `{api-id}.execute-api.…`.
///
/// Re-exported from [`crate::state::EXECUTE_API_LABEL`], which owns the label because the
/// control plane ANNOUNCES it (see `crate::state::announced_execute_api_endpoint`), so what
/// a caller is given and what the data planes read are one constant.
pub use crate::state::EXECUTE_API_LABEL;

/// The path segment that belongs to ApiGatewayManagementApi rather than to a stage.
///
/// `@connections` is bound on the SAME host an api is reached at — that service binds
/// `DELETE|GET|POST /@connections/{ConnectionId}` and its callers address it through the
/// api's invoke URL — so a request naming it is not one of this service's to answer and is
/// left to the service that models it. Claiming it here would make two services claim one
/// request, which the router answers `500 AmbiguousService`.
pub const CONNECTIONS_SEGMENT: &str = "@connections";

/// The media type a data plane answers in. A data plane is plain HTTPS rather than a
/// modelled operation, so it answers in the envelope API Gateway answers in — a JSON body
/// — rather than in this service's restJson1 error envelope.
pub const JSON_CONTENT_TYPE: &str = "application/json";

/// The member a data-plane body carries its reason under: API Gateway's own `execute-api`
/// error bodies are `{"message": …}`.
pub const MESSAGE_MEMBER: &str = "message";

/// The response header a restJson1 client resolves an error shape from, set so a caller
/// can branch on a refusal without parsing the body.
pub const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The status a request this service cannot place at all is answered with — a fault of the
/// emulator or of the api's configuration, never of the caller.
pub const UNPLACEABLE_STATUS: u16 = 500;

/// The sentence API Gateway's `execute-api` endpoint answers for an api id it does not
/// serve and for a stage with no deployment: `403` `{"message":"Forbidden"}`.
pub const FORBIDDEN_MESSAGE: &str = "Forbidden";

/// The status that sentence is answered with.
pub const FORBIDDEN_STATUS: u16 = 403;

/// Whether `status` is a CLIENT error: one of the `4xx` class, whose cause is the call rather
/// than the api serving it.
///
/// One reading for both planes, so what a gateway refuses a caller with, what an HTTP api's
/// stage counts in its `4xx` series and what a WebSocket stage meters as its client-error
/// count cannot disagree. The class is the one HTTP defines: "4xx (Client Error): The request
/// contains bad syntax or cannot be fulfilled" (RFC 9110 §15.5), so every status from 400 up
/// to but not including 500 is one.
pub fn is_client_error_status(status: u16) -> bool {
    (FIRST_CLIENT_ERROR_STATUS..FIRST_SERVER_ERROR_STATUS).contains(&status)
}

/// Whether `status` is a SERVER error: one of the `5xx` class, "the server is aware that it has
/// erred or is incapable of performing the requested method" (RFC 9110 §15.6) — the class
/// beside [`is_client_error_status`]'s, read the same one way.
pub fn is_server_error_status(status: u16) -> bool {
    (FIRST_SERVER_ERROR_STATUS..FIRST_SERVER_ERROR_STATUS + STATUSES_PER_CLASS)
        .contains(&status)
}

/// The lowest status of the client-error class and the lowest of the server-error class, and
/// how many statuses a class holds (RFC 9110 §15.5, §15.6).
const FIRST_CLIENT_ERROR_STATUS: u16 = 400;
const FIRST_SERVER_ERROR_STATUS: u16 = 500;
const STATUSES_PER_CLASS: u16 = 100;

/// The status a request whose authorizer refused it, or was not even invoked, is answered
/// with: "it returns a 401 Unauthorized response without calling the Lambda function"
/// (`Authorizer$IdentitySource`).
pub const UNAUTHORIZED_STATUS: u16 = 401;

/// The sentence that status carries.
pub const UNAUTHORIZED_MESSAGE: &str = "Unauthorized";

/// The status a request a served endpoint has nothing at is answered with, and its
/// sentence — the endpoint serves the host, but nothing of it serves this method and path.
pub const NOT_FOUND_STATUS: u16 = 404;
pub const NOT_FOUND_MESSAGE: &str = "Not Found";

/// The first path segment that marks an endpoint carried IN THE PATH rather than in the
/// host. Re-exported from [`crate::state::LOCAL_ENDPOINT_PREFIX`], which owns the marker
/// because the control plane ANNOUNCES it (see
/// `crate::state::announced_portal_default_domain_name`), so what a caller is given and
/// what the data planes read are one constant.
pub use crate::state::LOCAL_ENDPOINT_PREFIX;

/// The identity-source spellings that name a REQUEST HEADER, from
/// `Authorizer$IdentitySource`: "this value is route.request.header.Auth … for WebSocket
/// APIs. For HTTP APIs, use selection expressions prefixed with $, for example,
/// $request.header.Auth".
pub const HEADER_IDENTITY_SOURCES: &[&str] = &["route.request.header.", "$request.header."];

/// The identity-source spellings that name a QUERY STRING PARAMETER; the same member's
/// `route.request.querystring.Name` and `$request.querystring.Name`.
pub const QUERY_IDENTITY_SOURCES: &[&str] =
    &["route.request.querystring.", "$request.querystring."];

/// The identity-source spellings that name a STAGE VARIABLE — "the identity source can be
/// headers, query string parameters, stage variables, and context parameters"
/// (`Authorizer$IdentitySource`), read from the stage the request addresses.
pub const STAGE_VARIABLE_IDENTITY_SOURCES: &[&str] = &["stagevariables.", "$stagevariables."];

/// The identity-source spellings that name a CONTEXT parameter, the fourth part the same
/// member names. The service computes one for every call, so a request cannot be missing
/// it, and a data plane neither reads nor refuses on one.
pub const CONTEXT_IDENTITY_SOURCES: &[&str] = &["context.", "$context."];

/// `403` `{"message":"Forbidden"}` — the answer an `execute-api` endpoint gives for an api
/// or a stage it does not serve.
pub fn forbidden() -> RtResponse {
    json_response(FORBIDDEN_STATUS, FORBIDDEN_MESSAGE, None)
}

/// Whether this api's DEFAULT `execute-api` endpoint serves anything at all.
///
/// "Specifies whether clients can invoke your API by using the default execute-api endpoint.
/// By default, clients can invoke your API with the default
/// https://{api_id}.execute-api.{region}.amazonaws.com endpoint. To require that clients use
/// a custom domain name to invoke your API, disable the default endpoint"
/// (`Api$DisableExecuteApiEndpoint`): while the member is set, that endpoint serves NOTHING
/// of the api — whatever stage, deployment, route or authorization the request would have
/// resolved — and it serves again as soon as the member is cleared, because the member is a
/// property of the ENDPOINT and not of the deployment a stage froze.
///
/// Both api data planes read the member HERE, so neither can disagree with the other about an
/// endpoint being off: an addressing that resolves an api is still this service's to answer
/// ([`forbidden`]), since the host or path the client used is one this very service announced
/// for the api.
pub fn serves_default_endpoint(api: &crate::state::Api) -> bool {
    !api.disable_execute_api_endpoint
}

/// The reference form a stage variable is interpolated into an integration's configuration
/// under: `${stageVariables.<name>}`.
///
/// "You can use API Gateway stage variables … as part of an HTTP integration URL", written
/// as `http://${stageVariables.<variable_name>}/resource` — so the backend a stage reaches
/// is chosen by the stage's own variables rather than by the integration record, which is
/// the documented way one api serves a different backend per stage. That is the ONE spelling
/// the reference is documented under, so it is the one matched: a reference spelled any other
/// way is not this one and is left as written.
pub const STAGE_VARIABLE_REFERENCE_PREFIX: &str = "stageVariables.";

/// `template` with every `${stageVariables.<name>}` reference replaced by the value the
/// stage carries for that variable, or `None` when the stage carries no value for one of
/// the variables it names, or when the result would not fit the member it is a value of.
///
/// `None` is "this configuration could not be resolved for this stage", which is a fault of
/// the api's configuration rather than of the caller: the backend the reference names does
/// not exist for this stage, or does not fit in the URI the integration holds, so nothing
/// can be reached at it.
///
/// BOUNDED: the resolved value is still a value of the member it came from, so it is held to
/// that member's own documented length ([`crate::state::INTEGRATION_URI_LEN`]) — checked
/// BEFORE each substitution appends, so the string this builds cannot grow past that bound
/// however many references the template carries and however long the stage's values are.
/// Both factors of the expansion are therefore capped by a constant rather than by the
/// request, and nothing a client sends chooses the work or the answer's size.
///
/// Every OTHER `${…}` reference is left EXACTLY as written: this reads only the part of the
/// configuration a stage decides, so a mapping expression of another kind — and a reference
/// spelled otherwise than the documented [`STAGE_VARIABLE_REFERENCE_PREFIX`] — is carried
/// through for whatever reads it rather than being resolved, or refused, here. The variable
/// NAME is read as written too, since that is the key the stage stores it under.
pub fn resolve_stage_variables(
    template: &str,
    stage_variables: &BTreeMap<String, String>,
) -> Option<String> {
    const OPEN: &str = "${";
    // The bound every piece below is appended under: the upper end of the window the
    // member's own shape documents.
    let limit = crate::state::INTEGRATION_URI_LEN.1;
    let mut resolved = String::with_capacity(template.len().min(limit));
    // Append `piece`, or answer `None` when it would take the result past the bound.
    let push = |resolved: &mut String, piece: &str| -> Option<()> {
        (resolved.chars().count() + piece.chars().count() <= limit).then(|| resolved.push_str(piece))
    };
    let mut rest = template;
    while let Some(at) = rest.find(OPEN) {
        push(&mut resolved, &rest[..at])?;
        let after = &rest[at + OPEN.len()..];
        // An unterminated `${` closes no reference at all, so it is text like any other.
        let Some((reference, tail)) = after.split_once('}') else {
            push(&mut resolved, OPEN)?;
            rest = after;
            continue;
        };
        match reference
            .strip_prefix(STAGE_VARIABLE_REFERENCE_PREFIX)
            .filter(|name| !name.is_empty())
        {
            Some(name) => push(&mut resolved, stage_variables.get(name)?)?,
            None => {
                push(&mut resolved, OPEN)?;
                push(&mut resolved, reference)?;
                push(&mut resolved, "}")?;
            }
        }
        rest = tail;
    }
    push(&mut resolved, rest)?;
    Some(resolved)
}

/// One data-plane response: a status, a JSON `{"message": …}` body, and, for a refusal a
/// client branches on, the error code in [`ERROR_TYPE_HEADER`].
pub fn json_response(status: u16, message: &str, error_code: Option<&str>) -> RtResponse {
    let body = ::serde_json::json!({ MESSAGE_MEMBER: message })
        .to_string()
        .into_bytes();
    let length = body.len();
    let mut response = RtResponse::new(
        StatusCode::try_from(status).unwrap_or(
            StatusCode::try_from(UNPLACEABLE_STATUS).expect("500 is a valid status code"),
        ),
        SdkBody::from(body),
    );
    let headers = response.headers_mut();
    let _ = headers.try_insert("content-type", JSON_CONTENT_TYPE);
    let _ = headers.try_insert("content-length", length.to_string());
    if let Some(error_code) = error_code {
        let _ = headers.try_insert(ERROR_TYPE_HEADER, error_code.to_string());
    }
    response
}

/// One header's value as text, or `None` when it is absent or not text.
pub fn header_of<'h>(headers: &'h http::HeaderMap, name: &str) -> Option<&'h str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// The first label of `host` when `host` is an `execute-api` endpoint: the api id.
///
/// The shape is `{api-id}.execute-api.{region}.amazonaws.com` — a first label, then the
/// [`EXECUTE_API_LABEL`] label, then at least a region and a domain. Any `:port` and
/// userinfo is removed first, as the front door's own host reader does. The region and the
/// domain are not matched against a list: an emulator serves whatever region a caller
/// addresses, and a FIPS or a dualstack spelling of the same endpoint is the same endpoint.
pub fn api_id_of_host(host: &str) -> Option<&str> {
    let mut labels = host_name(host).split('.');
    let api_id = labels.next()?;
    let second = labels.next()?;
    // The label is `execute-api` itself, or one of its endpoint VARIANTS, which AWS spells
    // with a suffix after a hyphen (`execute-api-fips`). A label that merely STARTS with
    // those characters (`execute-apifoo`) is a different host and is not claimed.
    let execute_api = second == EXECUTE_API_LABEL
        || second
            .strip_prefix(EXECUTE_API_LABEL)
            .is_some_and(|variant| variant.starts_with('-') && variant.len() > 1);
    if api_id.is_empty() || !execute_api {
        return None;
    }
    // A region label and a domain: `{api-id}.execute-api.{region}.amazonaws.com` has four
    // labels after the id, and the shortest form this accepts still has two, so a bare
    // `execute-api.something` (no api id in front) is not read as one.
    (labels.count() >= 2).then_some(api_id)
}

/// Whether `path` addresses the `@connections` space of ApiGatewayManagementApi, in either
/// of the two forms that service's callers use: with the stage prefix its endpoint carries
/// (`/{stage}/@connections/{id}`) and without it.
pub fn addresses_connections(path: &str) -> bool {
    path_segments(path)
        .into_iter()
        .take(2)
        .any(|segment| segment == CONNECTIONS_SEGMENT)
}

/// The first non-empty path segment: the stage name of an invoke path.
pub fn first_segment(path: &str) -> Option<&str> {
    path_segments(path).into_iter().next()
}

/// The non-empty `/`-separated segments of a path.
pub fn path_segments(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|segment| !segment.is_empty())
        .collect()
}

/// The first non-empty segment of `path` and the path that follows it (with its leading
/// `/`), or `None` for a path with no segment at all.
pub fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.find('/') {
        Some(boundary) => (&trimmed[..boundary], &trimmed[boundary..]),
        None => (trimmed, "/"),
    };
    (!first.is_empty()).then_some((first, rest))
}

/// The service `host` names to the emulator's front door, or `None` when it names none.
///
/// The front door's OWN host reader decides it (`emulator_server::service_from_host`, the
/// last tier of the routing key every front door resolves), rather than a second rule
/// written here: the request the path addressings are read for is exactly the one that
/// reader can place nowhere — a loopback address or `localhost`, which is the only endpoint
/// a client on this machine can reach.
pub fn service_named_by_host(host: &str) -> Option<String> {
    let mut headers = http::HeaderMap::new();
    let value = http::HeaderValue::from_str(host).ok()?;
    headers.insert(http::header::HOST, value);
    ::emulator_server::service_from_host(&headers)
}

/// `host` with any userinfo and `:port` removed, the way the front door's own host reader
/// strips them: what is left is the name a stored endpoint host is compared against.
pub fn host_name(host: &str) -> &str {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo)
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
pub fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, with any scheme, authority and query removed.
pub fn path_of(uri: &str) -> &str {
    // The query is dropped FIRST: a query VALUE may carry `://`, which a scheme scan over
    // the whole URI would mistake for the authority.
    let without_query = uri.split('?').next().unwrap_or(uri);
    match without_query.split_once("://") {
        Some((_scheme, authority_and_path)) => match authority_and_path.find('/') {
            Some(path_start) => &authority_and_path[path_start..],
            None => "/",
        },
        None => without_query,
    }
}

/// The raw query string of a request URI, or `None` when it carries none. The query is
/// where an identity source of the `querystring` form is read from.
pub fn query_of(uri: &str) -> Option<&str> {
    uri.split_once('?').map(|(_before, query)| query)
}

/// The query parameters of a request URI, decoded ONCE per request so that reading N
/// identity sources costs N lookups rather than N scans of the query string.
///
/// The query is decoded as `application/x-www-form-urlencoded` — the form a URI's query
/// carries — through the framework's own decoder for that grammar
/// (`aws_smithy_schema_server::query::parse_form_urlencoded`, which turns `+` into a space
/// and `%XX` into its byte, and reads a pair with no `=` as the empty value), so this file
/// states no grammar of its own and cannot disagree with the rest of the tree about one.
/// A name the query repeats keeps its FIRST value, the one a reader of the query meets
/// first.
pub fn query_parameters(uri: &str) -> BTreeMap<String, String> {
    let mut parameters = BTreeMap::new();
    let Some(query) = query_of(uri) else {
        return parameters;
    };
    for (name, value) in ::aws_smithy_schema_server::query::parse_form_urlencoded(query.as_bytes())
    {
        parameters.entry(name).or_insert(value);
    }
    parameters
}

/// The query parameters of a request URI with a name the query REPEATS keeping every one of
/// its values, joined into the one comma-separated value a field list is (RFC 9110 §5.3).
///
/// That is the form a proxy event's `queryStringParameters` states them in — "the query
/// string parameters, with a repeated parameter's values comma-separated" — so the event
/// reports a repeated parameter as the list it was sent as rather than as whichever value a
/// reader met first. The same decoder as [`query_parameters`] reads the grammar, so the two
/// readings cannot disagree about what a pair says.
pub fn joined_query_parameters(uri: &str) -> BTreeMap<String, String> {
    let mut parameters: BTreeMap<String, String> = BTreeMap::new();
    let Some(query) = query_of(uri) else {
        return parameters;
    };
    for (name, value) in ::aws_smithy_schema_server::query::parse_form_urlencoded(query.as_bytes())
    {
        parameters
            .entry(name)
            .and_modify(|held| {
                held.push(',');
                held.push_str(&value);
            })
            .or_insert(value);
    }
    parameters
}

/// The parameter name `source` carries after whichever of `prefixes` it begins with, or
/// `None` when it begins with none of them or names an empty parameter.
///
/// The prefix is compared case-insensitively, because the same member documents one of
/// these parts under two spellings (`stagevariables.` and `$stageVariables.`), while the
/// parameter name itself is carried through as written — a query-string parameter name is
/// case-sensitive, and a header name is matched case-insensitively by the header map.
pub fn parameter_name<'s>(source: &'s str, prefixes: &[&str]) -> Option<&'s str> {
    prefixes
        .iter()
        .filter_map(|prefix| {
            let head = source.get(..prefix.len())?;
            head.eq_ignore_ascii_case(prefix)
                .then(|| &source[prefix.len()..])
        })
        .find(|name| !name.is_empty())
}

/// Whether the request presents a non-empty value for the request part `identity_source`
/// names.
///
/// `Authorizer$IdentitySource` calls each entry "a set of one or more mapping expressions
/// of the specified request parameters", whose parts are "headers, query string
/// parameters, stage variables, and context parameters", and requires each to be "present
/// in the request, not null, and non-empty" — so a value that is only whitespace counts as
/// absent, exactly as a missing one does.
///
/// A CONTEXT parameter answers `true`: the service computes one for every call, so no
/// request can be missing it. (A context key an AUTHORIZER produces — `context.authorizer.`
/// — exists only once one has run, so it cannot be an identity source of the authorizer
/// that produces it; nothing here reads one, and an authorizer configured over one is a
/// configuration this step does not refuse.) Every OTHER spelling — one no part of this
/// request can be read from, and one naming an empty parameter — is NOT satisfied, so an
/// expression a data plane cannot evaluate never passes the precondition it is a
/// precondition of.
pub fn presents_identity_source(
    identity_source: &str,
    header: impl Fn(&str) -> Option<String>,
    query_parameters: &BTreeMap<String, String>,
    stage_variables: &BTreeMap<String, String>,
) -> bool {
    let source = identity_source.trim();
    if let Some(name) = parameter_name(source, HEADER_IDENTITY_SOURCES) {
        return header(name).is_some_and(|value| !value.trim().is_empty());
    }
    if let Some(name) = parameter_name(source, QUERY_IDENTITY_SOURCES) {
        return query_parameters
            .get(name)
            .is_some_and(|value| !value.trim().is_empty());
    }
    if let Some(name) = parameter_name(source, STAGE_VARIABLE_IDENTITY_SOURCES) {
        return stage_variables
            .get(name)
            .is_some_and(|value| !value.trim().is_empty());
    }
    parameter_name(source, CONTEXT_IDENTITY_SOURCES).is_some()
}

/// The VALUE the request carries for the request part `identity_source` names, or `None`
/// when it carries none, carries only whitespace, or the expression names a part no value
/// can be read out of.
///
/// Only the two REQUEST-BOUND forms are read, which is what the member documents for the
/// source a token is extracted from: "For JWT, a single entry that specifies where to
/// extract the JSON Web Token (JWT) from inbound requests. Currently only header-based and
/// query parameter-based selections are supported, for example $request.header.
/// Authorization" (`Authorizer$IdentitySource`). An expression naming anything else names
/// no token, which is the same to the caller as presenting none.
pub fn identity_source_value(
    identity_source: &str,
    header: impl Fn(&str) -> Option<String>,
    query_parameters: &BTreeMap<String, String>,
) -> Option<String> {
    let source = identity_source.trim();
    let value = if let Some(name) = parameter_name(source, HEADER_IDENTITY_SOURCES) {
        header(name)
    } else {
        parameter_name(source, QUERY_IDENTITY_SOURCES)
            .and_then(|name| query_parameters.get(name).cloned())
    }?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
mod tests {
    //! The pure readings this module owns that a wire test cannot vary cheaply: which host
    //! is an `execute-api` endpoint, how a URI is split, and which request satisfies an
    //! identity source. The served behaviour of each data plane is pinned end to end in
    //! `tests/all/websocket_handshake_data_plane.rs` and
    //! `tests/all/http_api_jwt_data_plane.rs`.

    use super::*;

    #[test]
    fn execute_api_hosts_are_read_and_others_are_not() {
        let cases: &[(&str, Option<&str>)] = &[
            (
                "abc1230000.execute-api.us-east-1.amazonaws.com",
                Some("abc1230000"),
            ),
            (
                "0123456789.execute-api.eu-west-3.amazonaws.com",
                Some("0123456789"),
            ),
            ("a.execute-api.ap-southeast-2.amazonaws.com:443", Some("a")),
            // An endpoint variant: the same endpoint under a FIPS spelling.
            (
                "abc1230000.execute-api-fips.us-gov-west-1.amazonaws.com",
                Some("abc1230000"),
            ),
            (
                "abc1230000.execute-api.cn-north-1.amazonaws.com.cn",
                Some("abc1230000"),
            ),
            // Not an `execute-api` endpoint at all.
            ("abc1230000.execute-apifoo.us-east-1.amazonaws.com", None),
            ("apigateway.us-east-1.amazonaws.com", None),
            ("execute-api.us-east-1.amazonaws.com", None),
            ("127.0.0.1:9297", None),
            ("localhost", None),
            ("sockets.example.com", None),
            ("abc1230000.execute-api", None),
        ];
        for (host, expected) in cases {
            assert_eq!(
                api_id_of_host(host),
                *expected,
                "case {host:?}: expected the api id {expected:?}"
            );
        }
    }

    #[test]
    fn the_connections_path_space_is_another_services() {
        for path in ["/@connections/abc", "/prod/@connections/abc", "/@connections"] {
            assert!(
                addresses_connections(path),
                "case {path:?}: the @connections space belongs to ApiGatewayManagementApi"
            );
        }
        for path in ["/prod", "/prod/connections/abc", "/@connectionsx/abc"] {
            assert!(
                !addresses_connections(path),
                "case {path:?}: expected a stage path, not the @connections space"
            );
        }
    }

    #[test]
    fn a_uri_is_split_into_its_authority_and_path() {
        assert_eq!(authority_of("https://host:9297/prod"), Some("host:9297"));
        assert_eq!(authority_of("/prod"), None);
        assert_eq!(path_of("https://host:9297/prod"), "/prod");
        assert_eq!(path_of("https://host:9297/prod?a=b://c"), "/prod");
        assert_eq!(path_of("https://host:9297"), "/");
        assert_eq!(path_of("/prod/sub"), "/prod/sub");
        assert_eq!(first_segment("/prod/sub"), Some("prod"));
        assert_eq!(first_segment("/"), None);
        assert_eq!(query_of("https://host/prod?token=abc"), Some("token=abc"));
        assert_eq!(query_of("/prod?a=1&b=2"), Some("a=1&b=2"));
        assert_eq!(query_of("/prod"), None);
    }

    #[test]
    fn a_query_string_is_decoded_once_into_its_parameters() {
        let parameters =
            query_parameters("https://host/prod?token=abc&Name=&flag&my%20token=space&padded=+++");
        assert_eq!(parameters.get("token").map(String::as_str), Some("abc"));
        // Present with no value, and present with no `=` at all: both empty.
        assert_eq!(parameters.get("Name").map(String::as_str), Some(""));
        assert_eq!(parameters.get("flag").map(String::as_str), Some(""));
        // A query parameter name is case-sensitive, unlike a header's.
        assert_eq!(parameters.get("name"), None);
        assert_eq!(parameters.get("absent"), None);
        // Decoded as the client wrote it: a percent-encoded name, and a `+`-spelled space.
        assert_eq!(parameters.get("my token").map(String::as_str), Some("space"));
        assert_eq!(parameters.get("padded").map(String::as_str), Some("   "));
        // A repeated name keeps the value a reader meets first.
        assert_eq!(
            query_parameters("/prod?token=first&token=second")
                .get("token")
                .map(String::as_str),
            Some("first")
        );
        assert!(query_parameters("/prod").is_empty());
    }

    /// Each identity-source form, against a request that presents the part it names and a
    /// request that does not. The rule is `Authorizer$IdentitySource`'s: present in the
    /// request, not null and non-empty.
    #[test]
    fn an_identity_source_is_satisfied_only_by_a_non_empty_value() {
        let headers: BTreeMap<&str, &str> = BTreeMap::from([("auth", "t0ken"), ("blank", "   ")]);
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| (*value).to_string())
        };
        let query = query_parameters("/prod?token=t0ken&empty=&my%20token=decoded&padded=+");
        let stage_variables = BTreeMap::from([
            ("tenant".to_string(), "acme".to_string()),
            ("blank".to_string(), " ".to_string()),
        ]);

        let cases: &[(&str, &str, bool)] = &[
            // The WebSocket spelling the member documents, and the HTTP one.
            (
                "a header the request carries",
                "route.request.header.Auth",
                true,
            ),
            ("the same header, HTTP spelling", "$request.header.auth", true),
            (
                "a header the request does not carry",
                "route.request.header.Missing",
                false,
            ),
            (
                "a header carrying only whitespace",
                "route.request.header.blank",
                false,
            ),
            (
                "a query parameter the request carries",
                "route.request.querystring.token",
                true,
            ),
            (
                "the same parameter, HTTP spelling",
                "$request.querystring.token",
                true,
            ),
            (
                "a query parameter the request does not carry",
                "route.request.querystring.missing",
                false,
            ),
            (
                "a query parameter with an empty value",
                "route.request.querystring.empty",
                false,
            ),
            (
                "a query parameter whose name the request percent-encoded",
                "route.request.querystring.my token",
                true,
            ),
            (
                "a query parameter whose value decodes to whitespace",
                "route.request.querystring.padded",
                false,
            ),
            ("a stage variable the stage holds", "stagevariables.tenant", true),
            ("the same variable, HTTP spelling", "$stageVariables.tenant", true),
            (
                "a stage variable the stage does not hold",
                "stagevariables.missing",
                false,
            ),
            (
                "a stage variable holding only whitespace",
                "stagevariables.blank",
                false,
            ),
            // Computed by the service for every call, so no request can be missing it.
            ("a context parameter", "context.identity.sourceIp", true),
            ("a context parameter, HTTP spelling", "$context.requestId", true),
            // Nothing in the request can be read for these, so neither is satisfied.
            (
                "a form this data plane does not read",
                "route.request.body.action",
                false,
            ),
            ("a prefix naming no parameter", "route.request.header.", false),
            ("an empty expression", "", false),
            // Surrounding whitespace in the stored expression is not part of the name.
            (
                "the expression with surrounding spaces",
                "  route.request.header.Auth  ",
                true,
            ),
        ];

        for (case, identity_source, expected) in cases {
            assert_eq!(
                presents_identity_source(identity_source, header, &query, &stage_variables),
                *expected,
                "case {case:?}: expected {identity_source:?} to be presented: {expected}"
            );
        }
    }

    /// The VALUE read out of an identity source: the two request-bound forms, trimmed, and
    /// `None` for every part no value can be read from.
    #[test]
    fn an_identity_source_value_is_read_from_the_request_only() {
        let headers: BTreeMap<&str, &str> =
            BTreeMap::from([("authorization", " Bearer t0ken "), ("blank", "   ")]);
        let header = |name: &str| {
            headers
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| (*value).to_string())
        };
        let query = query_parameters("/prod?access_token=qt0ken&empty=");

        let cases: &[(&str, &str, Option<&str>)] = &[
            (
                "the header the JWT doc names",
                "$request.header.Authorization",
                Some("Bearer t0ken"),
            ),
            (
                "the same header, WebSocket spelling and other case",
                "route.request.header.authorization",
                Some("Bearer t0ken"),
            ),
            (
                "a query parameter",
                "$request.querystring.access_token",
                Some("qt0ken"),
            ),
            ("a header the request does not carry", "$request.header.X", None),
            ("a header carrying only whitespace", "$request.header.blank", None),
            (
                "a query parameter with an empty value",
                "$request.querystring.empty",
                None,
            ),
            ("a stage variable, which names no token", "$stageVariables.t", None),
            ("a context parameter, which names no token", "$context.requestId", None),
            ("an expression of no form at all", "Authorization", None),
            ("an empty expression", "", None),
        ];

        for (case, identity_source, expected) in cases {
            let actual = identity_source_value(identity_source, header, &query);
            assert_eq!(
                actual.as_deref(),
                *expected,
                "case {case:?}: expected {identity_source:?} to read {expected:?}, got {actual:?}"
            );
        }
    }

    /// A stage variable reference in an integration's configuration resolves against the
    /// STAGE the request addresses: the documented `${stageVariables.<name>}` form, in every
    /// position a URL carries one, with every other kind of reference carried through as
    /// written and a reference the stage holds no value for resolving to nothing at all.
    #[test]
    fn a_stage_variable_reference_resolves_against_the_stage_that_serves_the_request() {
        let stage_variables: BTreeMap<String, String> = [
            ("backendUrl", "abc1230000.execute-api.eu-west-1.amazonaws.com/prod"),
            ("host", "backend.example"),
            ("path", "items"),
            ("empty", ""),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();

        // (case, the stored configuration, what it resolves to for this stage)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "no reference at all",
                "https://backend.example/items",
                Some("https://backend.example/items"),
            ),
            (
                "the whole host and path, the documented HTTP-proxy use case",
                "https://${stageVariables.backendUrl}",
                Some("https://abc1230000.execute-api.eu-west-1.amazonaws.com/prod"),
            ),
            (
                "a reference followed by more path",
                "https://${stageVariables.host}/items/{id}",
                Some("https://backend.example/items/{id}"),
            ),
            (
                "two references in one URL",
                "https://${stageVariables.host}/${stageVariables.path}",
                Some("https://backend.example/items"),
            ),
            (
                "the lower-case spelling of the prefix, which is not the documented one",
                "https://${stagevariables.host}/items",
                Some("https://${stagevariables.host}/items"),
            ),
            (
                "a variable whose value is empty",
                "https://${stageVariables.empty}/items",
                Some("https:///items"),
            ),
            (
                "a reference the stage holds no variable for",
                "https://${stageVariables.absent}/items",
                None,
            ),
            (
                "a reference of another kind, carried through as written",
                "https://backend.example/${request.path.id}",
                Some("https://backend.example/${request.path.id}"),
            ),
            (
                "an unterminated reference, which is text",
                "https://backend.example/${stageVariables.host",
                Some("https://backend.example/${stageVariables.host"),
            ),
            (
                "the prefix naming no variable",
                "https://${stageVariables.}/items",
                Some("https://${stageVariables.}/items"),
            ),
            ("the empty configuration", "", Some("")),
        ];

        for (case, template, expected) in rows {
            let resolved = resolve_stage_variables(template, &stage_variables);
            assert_eq!(
                resolved.as_deref(),
                *expected,
                "case {case:?}: expected {template:?} to resolve to {expected:?}, got \
                 {resolved:?}"
            );
        }

        // The BOUND, over the two ways a result can grow: a long value, and many
        // references to it. Both are capped by the member's own documented length, so a
        // template that would resolve past it resolves to nothing at all — whatever the
        // stage's values are and however many references it carries.
        let limit = crate::state::INTEGRATION_URI_LEN.1;
        let long: BTreeMap<String, String> = [("big".to_string(), "x".repeat(limit))]
            .into_iter()
            .collect();
        let rows: Vec<(&str, String, bool)> = vec![
            (
                "one reference whose value fills the bound exactly",
                "${stageVariables.big}".to_string(),
                true,
            ),
            (
                "the same value with one character of text beside it",
                "/${stageVariables.big}".to_string(),
                false,
            ),
            (
                "two references to a value that fits once",
                "${stageVariables.big}${stageVariables.big}".to_string(),
                false,
            ),
            (
                "a hundred references to it",
                "${stageVariables.big}".repeat(100),
                false,
            ),
        ];
        for (case, template, resolves) in rows {
            let resolved = resolve_stage_variables(&template, &long);
            assert_eq!(
                resolved.is_some(),
                resolves,
                "case {case:?}: expected resolves={resolves}, got a result of {:?} characters",
                resolved.map(|value| value.chars().count())
            );
        }
    }
}
