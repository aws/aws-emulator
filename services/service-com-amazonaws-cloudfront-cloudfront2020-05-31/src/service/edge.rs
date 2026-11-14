// SPDX-License-Identifier: CC0-1.0

//! The distribution DATA plane: how a viewer request addressed to a distribution's own
//! domain name is recognised, decided and answered.
//!
//! A distribution is reached at its `Distribution$DomainName` — "the domain name that
//! CloudFront assigns when you create a distribution, such as
//! `d111111abcdef8.cloudfront.net`" — or at one of the alternate domain names its
//! `Aliases` carry. That host is NOT the control plane's
//! (`cloudfront.amazonaws.com`): its first label is the distribution's own generated
//! label, which names no service, so the emulator's front door can resolve no target
//! service from such a request and answers it `404 UnknownService` — for the hostname
//! this very service told the caller to use. The one thing that is never correct is for
//! the host to be unknown to the service.
//!
//! So this module is the second half of [`crate::service::EmulatedService`]: the claim
//! ([`claims_parts`], read by `Service::can_handle`) and the serving ([`serve`], reached
//! from `Service::override_operation`). The decisions it makes are CloudFront's OWN — the
//! ones a viewer sees before any origin is involved — and every one of them is computed
//! from the distribution's stored configuration:
//!
//! 1. the host (or the path, see below) names a distribution this service HOLDS, whose
//!    configuration is `Enabled`; a disabled distribution serves nothing;
//! 2. the distribution's `Restrictions.GeoRestriction` decides whether the viewer's
//!    COUNTRY is served at all. A viewer a `blacklist` names, or one a `whitelist` leaves
//!    out, is refused BY CLOUDFRONT with `403`, before the cache is read and before any
//!    origin request exists ([`geo_restricts`]);
//! 3. the CACHE BEHAVIOUR is selected by path pattern: `CacheBehaviors$Items` in the
//!    order the configuration lists them, first match wins, and the
//!    `DefaultCacheBehavior` when none matches — "If none of the path patterns match,
//!    CloudFront applies the default cache behavior";
//! 4. that behaviour's `ViewerProtocolPolicy` decides whether the SCHEME the viewer
//!    dialled is served at all: `redirect-to-https` answers a plaintext viewer `301` with
//!    the HTTPS URL of the same request, `https-only` answers it `403`, and `allow-all`
//!    serves it ([`viewer_protocol_plan`]);
//! 5. that behaviour's `AllowedMethods` decide whether the viewer's METHOD is served at
//!    all. A method the behaviour does not allow is refused BY CLOUDFRONT with `403` and
//!    `X-Cache: Error from cloudfront`, and no origin request is made;
//! 6. that behaviour's `TrustedKeyGroups` decide whether the viewer is AUTHORIZED at all:
//!    "when a cache behavior contains trusted key groups, CloudFront requires signed URLs or
//!    signed cookies for all requests that match the cache behavior", so a request carrying
//!    no signature, one whose key is not in those groups, or one whose policy's conditions do
//!    not hold is refused BY CLOUDFRONT with `403` under its own error code, before the cache
//!    is read and with no origin request made ([`signed_access`]);
//! 7. that behaviour's `CachedMethods` and cache policy decide whether the answer may
//!    come from the edge cache: a cacheable method whose TTL has not run out is answered
//!    `X-Cache: Hit from cloudfront` without touching the origin, and everything else
//!    goes to the origin and is answered `X-Cache: Miss from cloudfront`;
//! 8. only then is the ORIGIN the behaviour's `TargetOriginId` names fetched, in process,
//!    through the emulator this service is running within — over the scheme that origin's
//!    own `OriginProtocolPolicy` names ([`origin_uri`]) and as the distribution's own origin
//!    access control says that origin is to be addressed (see below);
//! 8. and whatever the answer turns out to be — the origin's, the cache's, or a page
//!    CloudFront generated itself — the header groups of the behaviour's
//!    `ResponseHeadersPolicyId` are applied to it: the CORS headers for a viewer whose
//!    `Origin` the policy grants, the security and custom headers it configures, and the
//!    headers its `RemoveHeadersConfig` takes off ([`response_headers`]).
//!
//! # What the origin fetch is, and what it is not
//!
//! The fetch is a real HTTP request to the origin's own `DomainName`, made through the
//! IN-PROCESS transport `OperationContext::callback` hands a handler: it is dispatched
//! back into this same emulator and opens no connection, so the only origin it can reach
//! is one the emulator itself hosts. An origin this emulator serves (an S3 bucket
//! endpoint, another emulated endpoint) answers it, and whatever it answers — its status
//! and its bytes — is what the edge serves and caches. An origin nothing in this emulator
//! serves answers nothing, and that is reported as CloudFront reports an origin it could
//! not reach: `502` with `X-Cache: Error from cloudfront`. Nothing is invented for an
//! origin that did not answer, and nothing off this machine is ever addressed.
//!
//! The SCHEME of that request is the origin's own, from its `OriginProtocolPolicy`, and an
//! origin addressed over a scheme it does not serve is the same `502`: every AWS service
//! endpoint this emulator serves answers over TLS alone, so an origin whose policy
//! addresses one of them over plaintext is an origin connection that cannot be opened
//! ([`origin_accepts_scheme`]).
//!
//! One origin shape is worth naming because it is the one the Lambda-function-URL
//! topology uses: a function URL host is served by Lambda's own function-URL data plane
//! (`services/service-com-amazonaws-lambda-awsgirapiservice/src/service/function_url.rs`),
//! so whether such an origin answers at all — and what it answers — is that plane's
//! decision, and whatever it answers is what the edge serves and caches. Nothing is
//! invented for it here: an origin that answers nothing is the `502` above. The enforcement
//! in steps 1-6, which is CloudFront's own and needs no origin, is served in full whatever
//! the origin does.
//!
//! Two things the fetch will not do. A REQUEST LOOP is refused instead of followed: an
//! origin whose host the edge itself serves, and a request that has already been through
//! an edge (its `Via` names CloudFront), are answered with the same `502` rather than
//! handed back to the edge they came from. And the VIEWER'S OWN CREDENTIAL is never
//! relayed: the origin is named by the distribution's stored configuration, so an
//! `Authorization` header (or a session token) a viewer presents is dropped rather than
//! forwarded to a destination that viewer did not choose
//! ([`HEADERS_NEVER_FORWARDED`]) — every other viewer header travels exactly as the
//! behaviour's origin request policy says.
//!
//! An origin fetch is made ON BEHALF OF the distribution that configured it. The
//! `OriginAccessControl` the origin's `OriginAccessControlId` names decides whether that
//! fetch is SIGNED — `SigningBehavior` `always` "signs all origin requests" — and the
//! ORIGIN's own resource policy decides whether it is signed FOR: a grant to the
//! `cloudfront.amazonaws.com` service principal, scoped by `aws:SourceArn` to this
//! distribution. Both live in [`super::origin_access`], which this module calls once while
//! planning the fetch and once while making it; a fetch the origin's document does not grant
//! travels unsigned, so the origin answers it as it answers any caller presenting no
//! credential, and whatever it answers is what the edge serves.
//!
//! # Addressing
//!
//! A client on the machine this emulator runs on cannot reach `d<label>.cloudfront.net`
//! at all: nothing resolves it there, so it points at the emulator's own endpoint and
//! carries in the PATH what it cannot carry in the host. [`resolve`] reads three
//! addressings, and each one is checked against this service's own records, so one
//! request still has one claimant:
//!
//! * HOST-STYLE: the host is a distribution's `DomainName` or one of its `Aliases`, and
//!   the whole path is the viewer path;
//! * PREFIXED PATH-STYLE: `/_aws/cloudfront/{distribution-id}/{viewer-path}`, read on
//!   whatever host it arrives at — the reserved first segment is in no service's modelled
//!   path space;
//! * PATH-STYLE: the first path segment names a distribution this service holds — its id,
//!   or the domain name a viewer would have addressed in the host, the same name carried
//!   where this client can carry it — read only on a host that
//!   names NO SERVICE (the emulator's own endpoint) or names THIS service, so a first
//!   segment that happens to name a distribution cannot take a request away from the service
//!   whose endpoint the host names.
//!
//! A distribution is addressed by the name it PUBLISHES, in the host or in that first
//! segment, and by nothing else: what an origin happens to hold decides what a fetch
//! answers, never which distribution a request is for.
//!
//! The viewer's SCHEME is not carried on the request the router hands a service
//! (origin-form URI plus a `Host` header), so `ViewerProtocolPolicy` — which is about
//! HTTP versus HTTPS from the viewer — is not decided here.

/// The content encodings the edge compresses an answer with, and the bytes they produce.
pub mod compression;
/// WHICH headers, cookies and query strings the behaviour's policies key on and forward.
pub mod forwarding;
/// The `CloudFront-*` request headers the edge generates itself, and the viewer-stated ones
/// it drops.
pub mod generated_headers;
/// The real-time log RECORD one served viewer request produces, and the Kinesis data streams
/// the cache behaviour's real-time log configuration delivers it to.
pub mod realtime_logs;
/// The headers the cache behaviour's RESPONSE HEADERS POLICY puts on the answer, and the ones
/// it takes off.
pub mod response_headers;
/// SIGNED URLs and SIGNED COOKIES: whether a cache behaviour that restricts its content to
/// trusted signers serves one viewer request.
pub mod signed_access;

use aws_smithy_runtime_api::client::http::HttpConnector;
use aws_smithy_runtime_api::http::{Request as RtRequest, Response as RtResponse, StatusCode};
use aws_smithy_types::body::SdkBody;
use smithy_server_runtime::{http, OperationContext};
use std::time::{Duration, Instant, SystemTime};

use super::edge_metrics;
use super::origin_access::{self, OriginAccess};
use compression::Encoding;
use crate::ops::types::{
    AllowedMethods, CacheBehavior, DefaultCacheBehavior, ForwardedValues, Origin,
    ResponseHeadersPolicyConfig, TrustedKeyGroups,
};
use crate::state::{
    managed_cache_policy, matches_path_pattern, same_domain, CachedEdgeResponse,
    ComAmazonawsCloudfrontCloudfront20200531State, StoredDistribution, DEFAULT_ALLOWED_METHODS,
    DEFAULT_COMPRESS, DEFAULT_ORIGIN_READ_TIMEOUT, GEO_RESTRICTION_BLACKLIST,
    GEO_RESTRICTION_WHITELIST,
};

/// The reserved path segment that names a DATA plane on the one endpoint this emulator
/// serves its whole surface at, ahead of the distribution id:
/// `/_aws/cloudfront/{distribution-id}/{viewer-path}`.
///
/// It begins with `_`, which no modelled path segment of any service does, so the prefix
/// cannot collide with a control-plane route however a viewer path is spelled.
const EMULATOR_PATH_NAMESPACE: &str = "_aws";

/// The second segment of the prefixed path-style addressing: the data plane this module
/// serves.
const EDGE_PATH_LABEL: &str = "cloudfront";

/// The response header CloudFront reports a cache outcome in.
const X_CACHE: &str = "x-cache";

/// `X-Cache` for an answer the edge cache held.
const CACHE_HIT: &str = "Hit from cloudfront";

/// `X-Cache` for an answer the origin produced for this request.
const CACHE_MISS: &str = "Miss from cloudfront";

/// `X-Cache` for an answer CloudFront itself produced instead of the origin's.
const CACHE_ERROR: &str = "Error from cloudfront";

/// The response (and origin-request) header carrying the identifier of the request the
/// edge served, from `X-Amz-Cf-Id`: "identifies the request that you're interested in".
const X_AMZ_CF_ID: &str = "x-amz-cf-id";

/// The `Server` of a response CloudFront generated itself, and the name its `Via` carries
/// on an origin request — which is also how a request that has already been through an
/// edge is recognised ([`carries_cloudfront_via`]).
const SERVER: &str = "CloudFront";

/// The name CloudFront's own `Via` carries.
const VIA_CLOUDFRONT: &str = SERVER;

/// The `User-Agent` CloudFront sends an origin request with when the viewer's own is not
/// forwarded to the origin.
const ORIGIN_USER_AGENT: &str = "Amazon CloudFront";

/// The status a viewer request gets when CloudFront refuses it at the edge.
const FORBIDDEN: u16 = 403;

/// The status a viewer request gets when the behaviour's `ViewerProtocolPolicy` is
/// `redirect-to-https` and the viewer dialled the plaintext scheme: "CloudFront returns an
/// HTTP status code of 301 (Moved Permanently) to the viewer along with the HTTPS URL".
const MOVED_PERMANENTLY: u16 = 301;

/// The response header that redirect carries the HTTPS URL in.
const LOCATION: &str = "location";

/// `ViewerProtocolPolicy` `redirect-to-https`: "If a viewer submits an HTTP request,
/// CloudFront returns an HTTP status code of 301 (Moved Permanently) to the viewer along
/// with the HTTPS URL. The viewer then resubmits the request using the new URL."
const REDIRECT_TO_HTTPS: &str = "redirect-to-https";

/// `ViewerProtocolPolicy` `https-only`: "If a viewer sends an HTTP request, CloudFront
/// returns an HTTP status code of 403 (Forbidden)."
const HTTPS_ONLY: &str = "https-only";

/// The scheme a viewer that dialled the plaintext half of an edge address used, which is
/// the only scheme either policy above acts on. The one spelling of it
/// ([`generated_headers`] reads the viewer's scheme off the request).
const PLAINTEXT_SCHEME: &str = generated_headers::PLAINTEXT_SCHEME;

/// The scheme a redirected viewer is sent to.
const SECURE_SCHEME: &str = generated_headers::DEFAULT_SCHEME;

/// `OriginProtocolPolicy` `http-only`: the origin is reached over HTTP alone.
const ORIGIN_HTTP_ONLY: &str = "http-only";

/// `OriginProtocolPolicy` `match-viewer`: the origin is reached over the scheme the VIEWER
/// dialled.
const ORIGIN_MATCH_VIEWER: &str = "match-viewer";

/// The default port of each scheme, which an origin URI leaves out.
const HTTP_DEFAULT_PORT: i32 = 80;
const HTTPS_DEFAULT_PORT: i32 = 443;

/// The status a viewer request gets when CloudFront could not get a response from the
/// origin.
const BAD_GATEWAY: u16 = 502;

/// The detail line CloudFront's generated page carries for a request it refused.
const BAD_REQUEST_DETAIL: &str = "Bad request.";

/// The detail line CloudFront's generated page carries when it could not reach the
/// origin, from CloudFront's own troubleshooting of a 502.
const NO_ORIGIN_DETAIL: &str = "CloudFront wasn't able to connect to the origin.";

/// The detail line CloudFront's generated page carries for a viewer its geographic
/// restriction refuses.
const GEO_RESTRICTED_DETAIL: &str =
    "The Amazon CloudFront distribution is configured to block access from your country.";

/// The detail line CloudFront's generated page carries for a viewer method the matched cache
/// behaviour does not allow — the refusal `AllowedMethods` makes, which names the method as
/// the thing refused rather than the request.
const METHOD_NOT_ALLOWED_DETAIL: &str =
    "This distribution is not configured to allow the HTTP request method that was used for \
     this request. The distribution supports only cachable requests.";

/// The headers that belong to ONE hop of an HTTP connection and are never forwarded to an
/// origin, as RFC 9110 lists them, plus the viewer's own `Authorization` and the headers
/// CloudFront itself puts on an origin request.
///
/// A viewer credential is never relayed to an origin by this emulator: the origin is named
/// by the distribution's stored configuration, so forwarding a caller's credential there
/// would hand it to a destination the caller of THIS request did not choose. Every other
/// viewer header is forwarded exactly as the behaviour's origin request policy says
/// ([`forwarding`]).
///
/// [`X_AMZ_CF_ID`] is here for the other reason: it is CloudFront's OWN identifier for the
/// request it is serving ([`stamp`] mints one per request), so an origin request carries the
/// edge's value and never a viewer's — a header a viewer sent under that name describes a
/// request the edge did not serve. The whole `CloudFront-*` family is dropped for that same
/// reason, through [`generated_headers::is_generated`].
const HEADERS_NEVER_FORWARDED: [&str; 12] = [
    "authorization",
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
    "x-amz-security-token",
    X_AMZ_CF_ID,
    // The reserved name the viewer's `Origin` reaches this data plane under when the request
    // came in on a distribution's own address ([`response_headers::VIEWER_ORIGIN`]): it is
    // this emulator's own plumbing between one of its addresses and this service, so it stops
    // here rather than travelling on to an origin.
    response_headers::VIEWER_ORIGIN,
];

/// The header a viewer asks for a content coding with, and the one an answer states its
/// coding in: `Accept-Encoding` is normalized by the cache policy that enables gzip or
/// Brotli ([`forwarding::accept_encoding`]), and `Content-Encoding` says what decodes the
/// bytes the edge served.
const ACCEPT_ENCODING: &str = "accept-encoding";

/// The response header naming the coding the served bytes are under.
const CONTENT_ENCODING: &str = "content-encoding";

/// The header the viewer's cookies travel in, as one `name=value` list.
const COOKIE: &str = "cookie";

/// The response (and request) header naming the media type of a body.
const CONTENT_TYPE: &str = "content-type";

/// What a viewer request addresses, as the wire carries it: the request's host — absent
/// on a request that carries neither a `Host` header nor an absolute URI — its path, and
/// its query.
///
/// Built from BOTH shapes the router hands this module, the buffered parts the claim
/// reads and the runtime-API request the serving seam reads, so one rule resolves both and
/// the claim cannot resolve a different distribution than the answer does.
#[derive(Debug)]
pub struct Addressed<'a> {
    host: Option<&'a str>,
    path: &'a str,
    query: &'a str,
}

impl<'a> Addressed<'a> {
    /// The addressing of the buffered request these parts came from.
    fn of_parts(parts: &'a http::request::Parts) -> Self {
        Self {
            host: request_host(parts),
            path: parts.uri.path(),
            query: parts.uri.query().unwrap_or_default(),
        }
    }

    /// The addressing of a runtime-API request, which carries its URI as a string.
    fn of_request(req: &'a RtRequest) -> Self {
        Self {
            host: req.headers().get("host").or_else(|| authority_of(req.uri())),
            path: path_of(req.uri()),
            query: query_of(req.uri()),
        }
    }
}

/// What a viewer request addresses, as this service's records resolve it: the
/// distribution, and the viewer path WITHIN its endpoint.
#[derive(Debug)]
struct Resolved<'s, 'r> {
    distribution: &'s StoredDistribution,
    viewer_path: &'r str,
}

/// What `addressed` addresses, or `None` when this data plane serves it not at all.
///
/// The three addressings are the module docs' own list, read in that order.
fn resolve<'s, 'r>(
    store: &'s ComAmazonawsCloudfrontCloudfront20200531State,
    addressed: &Addressed<'r>,
) -> Option<Resolved<'s, 'r>> {
    if let Some(host) = addressed.host {
        if let Some(distribution) = store
            .distributions
            .values()
            .find(|distribution| serves_host(distribution, host))
        {
            return Some(Resolved {
                distribution,
                viewer_path: addressed.path,
            });
        }
    }
    // PREFIXED PATH-STYLE: the path names this data plane before it names the
    // distribution, so it is read on whatever host it arrives at.
    if let Some((id, viewer_path)) = split_edge_prefix(addressed.path) {
        return store
            .distributions
            .get(id)
            .map(|distribution| Resolved {
                distribution,
                viewer_path,
            });
    }
    // PATH-STYLE, on the emulator's own endpoint or this service's: the first segment
    // names a distribution this service holds.
    let host_service = addressed.host.and_then(service_named_by_host);
    if host_service
        .as_deref()
        .is_some_and(|service| service != crate::ROUTE_KEY)
    {
        return None;
    }
    let (first, rest) = split_first_segment(addressed.path)?;
    // The segment is the distribution's id, or the domain name a viewer would have
    // addressed in the host — the same name, carried where this client can carry it, port
    // and all ([`serves_host`] reads both forms).
    store
        .distributions
        .get(first)
        .or_else(|| {
            store
                .distributions
                .values()
                .find(|distribution| serves_host(distribution, first))
        })
        .map(|distribution| Resolved {
            distribution,
            viewer_path: rest,
        })
}

/// Whether `host` is a domain name this distribution is served on: the `DomainName`
/// CloudFront assigned it, or one of the alternate domain names its `Aliases` carry.
///
/// DNS names are case-insensitive, so the comparison is ([`same_domain`]). A multi-tenant
/// distribution reports no domain of its own and so is served on none: its viewers reach
/// it through a connection group's routing endpoint.
///
/// The `DomainName` is compared against the host AS ADDRESSED first, because the name a
/// distribution publishes is the address of the listener its edge is served on
/// (`crate::state::EdgeEndpoint`) and that address names a PORT: two distributions differ in
/// exactly that port, so a comparison that dropped it would resolve every one of them to
/// whichever the store lists first. A published name that carries no port — the name
/// CloudFront's own zone would give it — matches the host without its port, which is what a
/// client that resolves such a name sends.
///
/// An alias is only read when it is a name a VIEWER could be sent to: the alternate domain
/// names are caller-supplied and unverifiable here, so one spelled like another service's
/// endpoint, like `localhost`, or like an IP literal would take over the address every
/// client on this machine dials ([`addressable_alias`]). The domain CloudFront itself
/// assigned is never such a name, and a distribution whose alias is one is still reachable
/// through the reserved data-plane path.
fn serves_host(distribution: &StoredDistribution, host: &str) -> bool {
    if same_domain(&distribution.domain_name, host) {
        return true;
    }
    let host = bare_host(host);
    if same_domain(&distribution.domain_name, host) {
        return true;
    }
    if !addressable_alias(host) {
        return false;
    }
    distribution
        .config
        .aliases
        .as_ref()
        .and_then(|aliases| aliases.items.as_ref())
        .is_some_and(|aliases| aliases.iter().any(|alias| same_domain(alias, host)))
}

/// Whether `host` is a name an alternate domain name may claim: a customer domain, rather
/// than an address this emulator — or AWS itself — is reached at.
///
/// An alternate domain name is a domain the customer owns and points at the distribution,
/// so none of these is one: the loopback names a client on this machine dials (`localhost`
/// and the names under it), any IP literal (bracketed IPv6 included), and a name in one of
/// the zones AWS serves its own endpoints in ([`AWS_ENDPOINT_SUFFIXES`]) — which is where
/// every other emulated service's endpoint host lives, and where CloudFront's own
/// `cloudfront.net` domains live. A stored alias naming one of them would take over an
/// address other callers reach this emulator at, and `Aliases` is caller-supplied and
/// unverifiable here.
fn addressable_alias(host: &str) -> bool {
    let host = bare_host(host);
    if host.is_empty() {
        return false;
    }
    let lowercase = host.to_ascii_lowercase();
    if lowercase == "localhost" || lowercase.ends_with(".localhost") {
        return false;
    }
    // An IP literal: a bracketed IPv6 address, or a name that parses as an address.
    if lowercase.starts_with('[') || lowercase.parse::<::std::net::IpAddr>().is_ok() {
        return false;
    }
    !AWS_ENDPOINT_SUFFIXES
        .iter()
        .any(|suffix| lowercase == suffix.trim_start_matches('.') || lowercase.ends_with(suffix))
}

/// The zones AWS serves its own endpoints in, which an alternate domain name is never in:
/// every emulated service's endpoint host is under one of them, and so is the
/// `cloudfront.net` domain CloudFront assigns a distribution itself.
const AWS_ENDPOINT_SUFFIXES: [&str; 5] = [
    ".amazonaws.com",
    ".amazonaws.com.cn",
    ".api.aws",
    ".on.aws",
    ".cloudfront.net",
];

/// The distribution id and the viewer path of a PREFIXED PATH-STYLE address, or `None`
/// when `path` does not carry that prefix.
///
/// The viewer path keeps the leading `/` the host-style addressing carries, so one path is
/// matched against a `PathPattern` however the request was addressed.
fn split_edge_prefix(path: &str) -> Option<(&str, &str)> {
    let (namespace, after_namespace) = split_first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (label, after_label) = split_first_segment(after_namespace)?;
    if label != EDGE_PATH_LABEL {
        return None;
    }
    split_first_segment(after_label)
}

/// The first non-empty segment of `path` and the rest of the path, which keeps a leading
/// `/` so it reads as a path of its own.
fn split_first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.trim_start_matches('/');
    let (first, rest) = match trimmed.find('/') {
        Some(mark) => (&trimmed[..mark], &trimmed[mark..]),
        None => (trimmed, "/"),
    };
    (!first.is_empty()).then_some((first, rest))
}

/// The service `host` names to the emulator's front door, or `None` when it names none.
///
/// The front door's OWN host reader decides it, rather than a second rule written here:
/// the request the path addressings are read for is exactly the one that reader can place
/// nowhere — a loopback address or `localhost`, the only endpoint a client on this machine
/// can reach.
fn service_named_by_host(host: &str) -> Option<String> {
    let mut headers = http::HeaderMap::new();
    let value = http::HeaderValue::from_str(host).ok()?;
    headers.insert(http::header::HOST, value);
    ::emulator_server::service_from_host(&headers)
}

/// A host without its userinfo or its port, as the front door's own host reader reads it.
///
/// A bracketed IPv6 literal (`[::1]:443`) keeps its brackets and all of its colons: only
/// a port after the closing bracket is removed.
fn bare_host(host: &str) -> &str {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    if let Some(closing) = without_userinfo.find(']') {
        return &without_userinfo[..=closing];
    }
    without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo)
}

/// Whether THIS data plane serves the request these parts came from — the data-plane half
/// of `Service::can_handle`.
///
/// Narrowed to a distribution this service HOLDS, read from the one shared store, so
/// nothing is claimed that this service cannot answer about and exactly one service claims
/// a request.
pub fn claims_parts<Eng: Send + Sync + 'static>(engine: &Eng, parts: &http::request::Parts) -> bool {
    claims(engine, &Addressed::of_parts(parts))
}

/// [`claims_parts`] for the runtime-API request the serving seam is handed.
pub fn claims_request<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    claims(engine, &Addressed::of_request(req))
}

/// Whether `req` is addressed to the EDGE DOMAIN of a distribution this service holds —
/// the host-style addressing alone.
///
/// Which plane a request belongs to is decided by its HOST: a request whose host is a
/// distribution's own domain name is viewer traffic to that distribution, whatever its
/// path spells, and never a call to the control plane that happens to share a path with a
/// modelled route. The two PATH addressings are the other way round, because they arrive
/// on a host that names the control plane or nothing: there the modelled route wins and
/// the path addressings are read only for a path no modelled route claims.
pub fn claims_request_by_host<Eng: Send + Sync + 'static>(engine: &Eng, req: &RtRequest) -> bool {
    let addressed = Addressed::of_request(req);
    let Some(host) = addressed.host else {
        return false;
    };
    crate::state::service_state(engine).is_some_and(|state| {
        state
            .lock()
            .distributions
            .values()
            .any(|distribution| serves_host(distribution, host))
    })
}

/// Whether this data plane serves `addressed`.
fn claims<Eng: Send + Sync + 'static>(engine: &Eng, addressed: &Addressed<'_>) -> bool {
    crate::state::service_state(engine)
        .is_some_and(|state| resolve(&state.lock(), addressed).is_some())
}

/// The cache behaviour selected for one viewer request, flattened out of the two shapes a
/// behaviour is modelled in (`DefaultCacheBehavior` and `CacheBehavior` carry the same
/// members).
#[derive(Debug)]
struct Behavior {
    /// `PathPattern` of the ordered behaviour that matched, or `None` for the default
    /// behaviour, which matches by being last.
    path_pattern: Option<String>,
    /// `RealtimeLogConfigArn` — "the Amazon Resource Name (ARN) of the real-time log
    /// configuration that is attached to this cache behavior", which is what the edge
    /// delivers this request's log record through ([`realtime_logs`]).
    realtime_log_config_arn: Option<String>,
    target_origin_id: String,
    /// `ViewerProtocolPolicy` — "the protocol that viewers can use to access the files in
    /// the origin specified by `TargetOriginId`", which is a required member of both
    /// shapes.
    viewer_protocol_policy: String,
    allowed_methods: Option<AllowedMethods>,
    /// `TrustedKeyGroups` — "a list of key groups that CloudFront can use to validate signed
    /// URLs or signed cookies": a behaviour that enables one serves only a request carrying a
    /// signature one of those groups' public keys verifies ([`signed_access`]).
    trusted_key_groups: Option<TrustedKeyGroups>,
    cache_policy_id: Option<String>,
    origin_request_policy_id: Option<String>,
    /// `Compress` — "whether you want CloudFront to automatically compress certain files
    /// for this cache behavior", and [`DEFAULT_COMPRESS`] for a behaviour that states none.
    compress: bool,
    /// The legacy per-behaviour TTL, for a behaviour that names no cache policy.
    default_ttl: Option<i64>,
    max_ttl: Option<i64>,
    /// `ForwardedValues` — the legacy member that states what a behaviour naming no cache
    /// policy keys on and forwards: "a `CacheBehavior` must include either a
    /// `CachePolicyId` or `ForwardedValues`" ([`forwarding`] reads it).
    forwarded_values: Option<ForwardedValues>,
    /// `ResponseHeadersPolicyId` — the policy whose header groups the edge applies to the
    /// answer it serves this request ([`response_headers`]).
    response_headers_policy_id: Option<String>,
}

impl Behavior {
    /// The ordered behaviour `behavior` describes.
    fn of_ordered(behavior: &CacheBehavior) -> Self {
        Self {
            path_pattern: Some(behavior.path_pattern.clone()),
            realtime_log_config_arn: behavior.realtime_log_config_arn.clone(),
            target_origin_id: behavior.target_origin_id.clone(),
            viewer_protocol_policy: behavior.viewer_protocol_policy.clone(),
            allowed_methods: behavior.allowed_methods.clone(),
            trusted_key_groups: behavior.trusted_key_groups.clone(),
            cache_policy_id: behavior.cache_policy_id.clone(),
            origin_request_policy_id: behavior.origin_request_policy_id.clone(),
            compress: behavior.compress.unwrap_or(DEFAULT_COMPRESS),
            default_ttl: behavior.default_ttl,
            max_ttl: behavior.max_ttl,
            forwarded_values: behavior.forwarded_values.clone(),
            response_headers_policy_id: behavior.response_headers_policy_id.clone(),
        }
    }

    /// The default behaviour `behavior` describes.
    fn of_default(behavior: &DefaultCacheBehavior) -> Self {
        Self {
            path_pattern: None,
            realtime_log_config_arn: behavior.realtime_log_config_arn.clone(),
            target_origin_id: behavior.target_origin_id.clone(),
            viewer_protocol_policy: behavior.viewer_protocol_policy.clone(),
            allowed_methods: behavior.allowed_methods.clone(),
            trusted_key_groups: behavior.trusted_key_groups.clone(),
            cache_policy_id: behavior.cache_policy_id.clone(),
            origin_request_policy_id: behavior.origin_request_policy_id.clone(),
            compress: behavior.compress.unwrap_or(DEFAULT_COMPRESS),
            default_ttl: behavior.default_ttl,
            max_ttl: behavior.max_ttl,
            forwarded_values: behavior.forwarded_values.clone(),
            response_headers_policy_id: behavior.response_headers_policy_id.clone(),
        }
    }

    /// The methods this behaviour serves at all: `AllowedMethods$Items`, or
    /// [`DEFAULT_ALLOWED_METHODS`] for a behaviour that names none.
    fn allows(&self, method: &str) -> bool {
        match &self.allowed_methods {
            Some(allowed) => allowed
                .items
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(method)),
            None => DEFAULT_ALLOWED_METHODS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(method)),
        }
    }

    /// Whether this behaviour caches the response to `method`: `CachedMethods$Items`,
    /// or [`DEFAULT_ALLOWED_METHODS`] when the behaviour names neither list.
    ///
    /// "The HTTP methods for which CloudFront caches responses": a method that is allowed
    /// but not cached is forwarded to the origin every time and never answered from the
    /// cache.
    fn caches(&self, method: &str) -> bool {
        match self
            .allowed_methods
            .as_ref()
            .and_then(|allowed| allowed.cached_methods.as_ref())
        {
            Some(cached) => cached
                .items
                .iter()
                .any(|cached| cached.eq_ignore_ascii_case(method)),
            None => DEFAULT_ALLOWED_METHODS
                .iter()
                .any(|cached| cached.eq_ignore_ascii_case(method)),
        }
    }
}

/// The behaviour that applies to `viewer_path`: the first ordered behaviour whose
/// `PathPattern` matches it, and otherwise the default behaviour.
///
/// "CloudFront compares the path in the viewer request with the path patterns in the cache
/// behaviors ... in the order in which the cache behaviors are listed", and "if none of
/// the path patterns match, CloudFront applies the default cache behavior".
fn select_behavior(distribution: &StoredDistribution, viewer_path: &str) -> Behavior {
    let ordered = distribution
        .config
        .cache_behaviors
        .as_ref()
        .and_then(|behaviors| behaviors.items.as_ref())
        .into_iter()
        .flatten()
        .find(|behavior| matches_path_pattern(&behavior.path_pattern, viewer_path));
    match ordered {
        Some(behavior) => Behavior::of_ordered(behavior),
        None => Behavior::of_default(&distribution.config.default_cache_behavior),
    }
}

/// What the matched behaviour's `ViewerProtocolPolicy` does with a viewer that dialled the
/// PLAINTEXT scheme, or `None` when this viewer's scheme is served as it arrived.
///
/// The member is "the protocol that viewers can use to access the files in the origin
/// specified by `TargetOriginId`", and each value states its own answer: `allow-all` —
/// "viewers can use HTTP or HTTPS" — serves the request as it arrived; `redirect-to-https`
/// answers "an HTTP status code of 301 (Moved Permanently) to the viewer along with the
/// HTTPS URL", which the viewer "then resubmits the request using"; and `https-only`
/// answers "an HTTP status code of 403 (Forbidden)". A viewer that already dialled `https`
/// is served by every one of them, so the decision reads the scheme first.
///
/// Decided from the configuration and the connection alone: it needs no origin, no cache
/// and no method, so it is made before any of them.
fn viewer_protocol_plan(
    behavior: &Behavior,
    addressed: &Addressed<'_>,
    resolved: &Resolved<'_, '_>,
    req: &RtRequest,
) -> Option<Plan> {
    if generated_headers::viewer_scheme(req.uri()) != PLAINTEXT_SCHEME {
        return None;
    }
    match behavior.viewer_protocol_policy.as_str() {
        REDIRECT_TO_HTTPS => Some(Plan::Redirect {
            location: secure_url(
                viewer_host(addressed, resolved),
                resolved.viewer_path,
                addressed.query,
            ),
        }),
        HTTPS_ONLY => Some(Plan::Generated {
            status: FORBIDDEN,
            detail: BAD_REQUEST_DETAIL,
        }),
        // `allow-all`, and a policy value this emulator does not know: the request is
        // served over the scheme it arrived on.
        _ => None,
    }
}

/// The host a redirected viewer is sent back to: the one it ADDRESSED, when that host is a
/// domain name this distribution is served on, and otherwise the `DomainName` the
/// distribution publishes.
///
/// A viewer reaches a distribution at a name the distribution itself published — its
/// `DomainName` or one of its `Aliases` — so a redirect keeps the viewer on the name it
/// chose. A request that arrived through the reserved data-plane path carries this
/// service's own host instead of a name of the distribution's, and the name to send such a
/// viewer to is the one the distribution publishes.
fn viewer_host<'a>(addressed: &Addressed<'a>, resolved: &'a Resolved<'_, '_>) -> &'a str {
    addressed
        .host
        .filter(|host| serves_host(resolved.distribution, host))
        .unwrap_or(&resolved.distribution.domain_name)
}

/// The `https` URL of the same request: the host the viewer addressed, the viewer path and
/// the viewer query, which is the "HTTPS URL" a `redirect-to-https` answer carries.
fn secure_url(host: &str, viewer_path: &str, query: &str) -> String {
    let path = if viewer_path.starts_with('/') {
        viewer_path.to_string()
    } else {
        format!("/{viewer_path}")
    };
    if query.is_empty() {
        format!("{SECURE_SCHEME}://{host}{path}")
    } else {
        format!("{SECURE_SCHEME}://{host}{path}?{query}")
    }
}

/// The country a viewer request comes from, as the edge resolves it
/// ([`generated_headers::viewer_country`]): the ONE value this request's country has, read
/// by the geographic restriction below and by the header the origin receives alike.
fn viewer_country(req: &RtRequest) -> String {
    generated_headers::viewer_country(req)
}

/// Whether this distribution's GEOGRAPHIC RESTRICTION refuses a viewer in `country`,
/// computed from `Restrictions.GeoRestriction` alone.
///
/// `RestrictionType` decides how the `Items` are read. `blacklist`: the locations "specify
/// the countries in which you don't want CloudFront to distribute your content", so a
/// viewer in one of them is refused and every other viewer is served. `whitelist`: they
/// "specify the countries in which you want CloudFront to distribute your content", so a
/// viewer anywhere else is refused. `none`: "No geo restriction is enabled, meaning access
/// to content is not restricted by client geo location", so nobody is refused — which is
/// also what a configuration carrying no `Restrictions` at all means
/// ([`crate::state::GEO_RESTRICTION_NONE`]).
///
/// A `Location` is "a two-letter, uppercase country code", so the country the edge resolved
/// is matched without regard to its case.
fn geo_restricts(distribution: &StoredDistribution, country: &str) -> bool {
    let Some(restrictions) = distribution.config.restrictions.as_ref() else {
        return false;
    };
    let geo = &restrictions.geo_restriction;
    let listed = geo
        .items
        .as_deref()
        .unwrap_or_default()
        .iter()
        .any(|location| location.eq_ignore_ascii_case(country));
    match geo.restriction_type.as_str() {
        GEO_RESTRICTION_BLACKLIST => listed,
        GEO_RESTRICTION_WHITELIST => !listed,
        // `none`, and a restriction type this emulator does not know: nothing is
        // restricted, which is what an absent `Restrictions` member means too.
        _ => false,
    }
}

/// The longest an object is held in the edge cache, whatever the configuration asks for:
/// a year in seconds, the `MaxTTL` every AWS-managed cache policy that caches at all
/// publishes ([`crate::state::MANAGED_CACHE_POLICIES`]). A stored TTL is an `i64` taken
/// from a request, so clamping it here is what keeps an expiry instant computable.
const MAX_CACHE_TTL_SECONDS: u64 = 31_536_000;

/// How long a response the behaviour caches may be served from the edge cache, or `None`
/// when the behaviour caches nothing at all.
///
/// The cache policy the behaviour names decides it — its `DefaultTTL` is "the default
/// amount of time ... that you want objects to stay in the CloudFront cache" — and a
/// policy whose TTLs are all zero (`Managed-CachingDisabled`) caches nothing, so such a
/// behaviour forwards every request. A behaviour that names no cache policy carries the
/// legacy per-behaviour `DefaultTTL` instead.
fn cache_ttl(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    behavior: &Behavior,
) -> Option<Duration> {
    let seconds = match &behavior.cache_policy_id {
        Some(id) => match store.cache_policies.get(id) {
            Some(stored) => stored.config.default_ttl.unwrap_or(stored.config.min_ttl),
            None => match managed_cache_policy(id) {
                Some(managed) => managed.default_ttl,
                // A behaviour naming a cache policy this account does not hold cannot be
                // created, so this is a configuration that lost its policy: nothing is
                // cached under a policy that cannot be read.
                None => 0,
            },
        },
        None => behavior
            .default_ttl
            .or(behavior.max_ttl)
            .unwrap_or_default(),
    };
    let seconds = u64::try_from(seconds).unwrap_or_default();
    // Clamped to the longest TTL CloudFront's own cache policies carry, so a stored TTL
    // is a span of time rather than an instant the clock cannot reach: every published
    // managed policy's `MaxTTL` is this value ([`crate::state::MANAGED_CACHE_POLICIES`]),
    // and a configuration asking for more is held for this long.
    let seconds = seconds.min(MAX_CACHE_TTL_SECONDS);
    (seconds > 0).then(|| Duration::from_secs(seconds))
}

/// What the edge does with one viewer request, decided from the stored configuration
/// alone.
///
/// Owned rather than borrowed: the decision is made under the store's lock and the origin
/// fetch happens after it is released.
#[derive(Debug)]
enum Plan {
    /// CloudFront answers this itself: a status it generated and the detail line its
    /// error page carries.
    Generated { status: u16, detail: &'static str },
    /// CloudFront refuses this itself because the behaviour restricts its content to
    /// trusted signers and this request's signed access did not hold ([`signed_access`]).
    Refused { refusal: signed_access::Refusal },
    /// CloudFront sends this viewer to the HTTPS url of what it asked for, the matched
    /// behaviour's `ViewerProtocolPolicy` being `redirect-to-https`.
    Redirect {
        /// The absolute `https` URL of the same request, which the redirect's `Location`
        /// carries.
        location: String,
    },
    /// The edge cache holds the answer.
    Hit {
        /// The ORIGIN's own answer, as the entry holds it.
        answer: OriginAnswer,
        /// The coding the edge compresses it with, when the behaviour asks for compression
        /// and this viewer accepts one ([`compression`]).
        encode: Option<Encoding>,
    },
    /// The origin must answer it.
    Fetch(Box<OriginFetch>),
}

/// One answer an ORIGIN produced, as the edge holds it and serves it: the status, the bytes,
/// and the two things the origin said about those bytes.
///
/// What the edge serves FROM it — the content coding one viewer negotiated — is computed per
/// request rather than held ([`served_origin_answer`]), so one viewer's negotiation is never
/// served to another.
#[derive(Debug)]
struct OriginAnswer {
    status: u16,
    body: Vec<u8>,
    /// `Content-Type` as the origin stated it, or `None` when it stated none.
    content_type: Option<String>,
    /// `Content-Encoding` as the origin stated it — the coding its own bytes are under — or
    /// `None` for bytes the origin did not encode.
    content_encoding: Option<String>,
}

/// One origin fetch the edge is to make, and what to do with what comes back.
#[derive(Debug)]
struct OriginFetch {
    /// The distribution whose cache a cacheable answer is held in.
    distribution_id: String,
    /// The absolute URI of the origin request.
    uri: String,
    /// What the origin's ORIGIN ACCESS CONTROL makes of this fetch, or `None` for an origin
    /// whose configuration signs no request (see [`origin_access`]).
    access: Option<OriginAccess>,
    /// The headers the origin request carries beside its body's own.
    headers: Vec<(String, String)>,
    /// How long the edge waits for the origin, from `OriginReadTimeout`.
    read_timeout: Duration,
    /// The cache key and TTL a cacheable answer is held under, or `None` when this
    /// request's answer is not cached at all.
    cache: Option<CacheDirective>,
    /// The coding the edge compresses the origin's answer with, when the behaviour asks for
    /// compression and this viewer accepts one ([`compression`]).
    encode: Option<Encoding>,
    /// The viewer path, which an invalidation is matched against.
    viewer_path: String,
}

/// Where a cacheable answer is held, and for how long.
#[derive(Debug)]
struct CacheDirective {
    key: String,
    ttl: Duration,
}

/// Serve one viewer request addressed to a distribution's edge.
///
/// The one serving path: the claim ([`claims_request`]) and this function read the same
/// addressing, so a request this service claimed is a request this function resolves.
pub async fn serve<Eng: Send + Sync + 'static>(
    engine: &Eng,
    req: &RtRequest,
    ctx: &OperationContext<'_>,
) -> RtResponse {
    let now = SystemTime::now();
    // When the edge RECEIVED this request, on the monotonic clock: the instant the origin
    // latency of an answer an origin produces is measured from ([`edge_metrics`]).
    let received = Instant::now();
    let cf_id;
    let served_for;
    let plan;
    let realtime_log;
    {
        let Some(state) = crate::state::service_state(engine) else {
            return generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, "");
        };
        let mut store = state.lock();
        // The edge reads the distribution as every other read does: through the clock,
        // which owes a just-created configuration its deployment.
        store.settle(now);
        cf_id = store.mint_edge_request_id();
        (served_for, plan) = plan_request(&store, req, &cf_id, now);
        // WHETHER this request's log data is delivered at all is the matched behaviour's
        // real-time log configuration to decide, and it is decided HERE, over the sequence of
        // the requests that configuration covers ([`realtime_logs::sampled`]) — the record
        // itself is built from the answer, which is not served yet.
        realtime_log = served_for.as_ref().and_then(|served| {
            realtime_logs::sampled(&mut store, served.realtime_log_config_arn.as_deref())
        });
    }
    let method = req.method().to_ascii_uppercase();
    let request_bytes = req.body().bytes().map_or(0, <[u8]>::len);
    // The WAF web ACL the distribution's configuration puts in front of it decides the
    // request before anything the plan would do: "CloudFront responds to requests either
    // with the requested content or with an HTTP 403 status code (Forbidden)", so a request
    // it does not let through is answered here — before the edge cache is read and with no
    // origin request made — while the answer is still counted into the distribution's own
    // series, the edge having served it. Asked with THIS service's store released, since
    // the decision locks the WAFv2 service's own ([`super::web_acl`]).
    let (edge_refusal, logged) = match served_for.as_ref() {
        Some(served) => super::web_acl::refusal(
            engine,
            &served.web_acl_id,
            req,
            &served.viewer_path,
            &served.query,
        ),
        None => (None, None),
    };
    // What that web ACL LOGS about the request it has just decided, delivered to the
    // destination its own `LoggingConfiguration` names: WAF records the request it
    // inspected, so the record is delivered for a request it refused at the edge as much
    // as for one it let through to the cache.
    if let Some(logged) = logged.as_ref() {
        ::service_com_amazonaws_wafv2_awswaf_20190729::service::logs::deliver(ctx, logged).await;
    }
    let (mut response, served_from, origin_latency) = match edge_refusal {
        Some(refusal) => (
            refused_by_web_acl(refusal, &cf_id),
            edge_metrics::ServedFrom::CloudFront,
            None,
        ),
        None => match plan {
            Plan::Generated { status, detail } => (
                generated(status, detail, &cf_id),
                edge_metrics::ServedFrom::CloudFront,
                None,
            ),
            Plan::Refused { refusal } => (
                refused_signed_access(refusal, &cf_id),
                edge_metrics::ServedFrom::CloudFront,
                None,
            ),
            Plan::Redirect { location } => (
                redirected(&location, &cf_id),
                edge_metrics::ServedFrom::CloudFront,
                None,
            ),
            Plan::Hit { answer, encode } => (
                served_origin_answer(&answer, encode, CACHE_HIT, &cf_id),
                edge_metrics::ServedFrom::EdgeCache,
                None,
            ),
            Plan::Fetch(fetch) => {
                let fetched = fetch_origin(engine, ctx, *fetch, &cf_id, req, received).await;
                // An origin that answered is what the answer came FROM; one that answered
                // nothing leaves CloudFront to produce the answer itself, which is no
                // origin's and has no origin latency.
                let served_from = if fetched.origin_latency.is_some() {
                    edge_metrics::ServedFrom::Origin
                } else {
                    edge_metrics::ServedFrom::CloudFront
                };
                (fetched.response, served_from, fetched.origin_latency)
            }
        },
    };
    // The header groups of the response headers policy the matched behaviour attaches, put on
    // the answer CloudFront is about to serve — whoever produced it, since the policy is the
    // distribution's statement about the RESPONSE rather than about the origin
    // ([`response_headers`]). Applied before the request is counted and logged, so what those
    // report is the answer the viewer actually gets.
    if let Some(policy) = served_for
        .as_ref()
        .and_then(|served| served.response_headers_policy.as_ref())
    {
        response_headers::apply(
            policy,
            response_headers::ViewerCors::of_request(req),
            &mut response,
        );
    }
    // What the edge DID with this request is reported as the distribution's own CloudWatch
    // series, whoever produced the answer: the counters and error rates a client watches a
    // distribution with are the viewer requests it served ([`edge_metrics`]). The series the
    // distribution's monitoring subscription adds to those are published beside them where
    // that subscription enables them, and for no other distribution.
    if let Some(served) = served_for {
        edge_metrics::count_viewer_request(
            engine,
            &served.id,
            now,
            &edge_metrics::ServedRequest {
                method: &method,
                status: response.status().as_u16(),
                response_bytes: response.body().bytes().map_or(0, <[u8]>::len),
                request_bytes,
                served_from,
                origin_latency,
            },
            served.additional_metrics,
        );
        // And what the edge did with it is ALSO what the real-time log record of this request
        // states, for a cache behaviour whose real-time log configuration covers it: one
        // record per request, delivered to each data stream that configuration names
        // ([`realtime_logs`]). Delivered with this service's store released, since the hop
        // locks the stores of the services it asks.
        if let Some(configured) = realtime_log {
            // The span the edge spent on the request, measured once, and the instant it
            // finished responding read from the SAME pair of clock readings the rest of this
            // request is reported by: `now` is when the edge received it, so the two values
            // the record carries cannot describe different requests.
            let taken = received.elapsed();
            let logged = realtime_logs::LoggedRequest {
                req,
                at: now + taken,
                taken,
                origin_latency,
                status: response.status().as_u16(),
                response_bytes: response.body().bytes().map_or(0, <[u8]>::len),
                content_type: response
                    .headers()
                    .get(CONTENT_TYPE),
                distribution_domain: &served.domain_name,
                viewer_path: &served.viewer_path,
                query: &served.query,
                country: &served.country,
                request_id: &cf_id,
                path_pattern: served.path_pattern.as_deref(),
                result_type: realtime_logs::ResultType::of_answer(
                    served_from,
                    response.status().as_u16(),
                ),
            };
            let record = logged.record(&configured.fields);
            realtime_logs::deliver(ctx, &configured, &record, &cf_id).await;
        }
    }
    response
}

/// The URLs one viewer request addresses, which a signed URL's policy `Resource` is matched
/// against ([`signed_access`]).
///
/// "The base URL, including the query string, if any" — built from the names the distribution
/// answers at: its own `DomainName`, "the domain name that CloudFront assigns when you create
/// a distribution", and each of its `Aliases`, which are "the CNAMEs, if any, for this
/// distribution". A policy may be written for any name a viewer reaches the distribution by,
/// and the viewer's own `Host` is not among the candidates because the request has already
/// been resolved to THIS distribution: whichever of its names was dialled, the policy has to
/// name one of them.
///
/// The CloudFront signature parameters are not part of the URL a policy was signed for — they
/// are what carries the signature — so they are dropped from the query string here.
fn resource_urls(
    distribution: &StoredDistribution,
    scheme: &str,
    viewer_path: &str,
    query: &str,
) -> Vec<String> {
    let signed_query = signed_access::resource_query(query);
    let suffix = if signed_query.is_empty() {
        viewer_path.to_string()
    } else {
        format!("{viewer_path}?{signed_query}")
    };
    ::std::iter::once(&distribution.domain_name)
        .chain(
            distribution
                .config
                .aliases
                .as_ref()
                .and_then(|aliases| aliases.items.as_ref())
                .into_iter()
                .flatten(),
        )
        .map(|host| format!("{scheme}://{host}{suffix}"))
        .collect()
}

/// The answer the edge serves for a viewer request whose SIGNED ACCESS did not hold: `403`
/// under the error code and sentence CloudFront states for that failure, carrying no origin
/// bytes and reported as an answer CloudFront produced itself.
fn refused_signed_access(refusal: signed_access::Refusal, cf_id: &str) -> RtResponse {
    let body = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <Error><Code>{}</Code><Message>{}</Message></Error>",
        refusal.code(),
        refusal.message()
    );
    let mut response = served(FORBIDDEN, body.into_bytes(), CACHE_ERROR, cf_id);
    let _ = response
        .headers_mut()
        .try_insert(CONTENT_TYPE, "text/xml".to_string());
    let _ = response
        .headers_mut()
        .try_insert("server", SERVER.to_string());
    response
}

/// The answer the edge serves for a viewer request the distribution's web ACL did not let
/// through: the status, headers and body WAF decided, under the edge's own request id and
/// reported as an answer CloudFront produced itself rather than one an origin did.
fn refused_by_web_acl(refusal: super::web_acl::Refusal, cf_id: &str) -> RtResponse {
    let mut response = served(
        refusal.status,
        refusal.body,
        CACHE_ERROR,
        cf_id,
    );
    if let Some(content_type) = refusal.content_type {
        let _ = response
            .headers_mut()
            .try_insert("content-type", content_type);
    }
    for (name, value) in refusal.headers {
        let _ = response.headers_mut().try_append(name, value);
    }
    response
}

/// The distribution one viewer request resolved to, and everything about it the edge still
/// needs once the store's lock is released: the id its series is counted under, whether its
/// monitoring subscription enables the additional series, the web ACL its configuration puts
/// in front of it, and the viewer path and query a rule inspects.
#[derive(Debug)]
struct ServedFor {
    id: String,
    /// What this distribution's monitoring subscription says about its ADDITIONAL CloudWatch
    /// series ([`edge_metrics::AdditionalMetrics`]).
    additional_metrics: edge_metrics::AdditionalMetrics,
    /// `DistributionConfig$WebACLId` as the stored configuration carries it — the empty
    /// string for a distribution that names none.
    web_acl_id: String,
    viewer_path: String,
    query: String,
    /// `Distribution$DomainName`, which is the name a record reports as the distribution's
    /// own ([`realtime_logs`]).
    domain_name: String,
    /// The country the edge resolved for this viewer, read once and reported wherever a
    /// country is.
    country: String,
    /// The `PathPattern` of the cache behaviour that matched, or `None` for the default
    /// behaviour.
    path_pattern: Option<String>,
    /// `RealtimeLogConfigArn` of the behaviour that matched — the configuration this
    /// request's log record goes through, when the behaviour names one.
    realtime_log_config_arn: Option<String>,
    /// The configuration of the response headers policy the matched behaviour attaches, read
    /// under the store's lock, or `None` when the behaviour attaches none (or names one this
    /// account no longer holds). Its header groups are applied to whatever the edge serves
    /// ([`response_headers`]).
    response_headers_policy: Option<ResponseHeadersPolicyConfig>,
}

/// Decide what the edge does with one viewer request, under the store's lock, and name the
/// distribution the request resolved to — the one whose series it is counted into.
fn plan_request(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    req: &RtRequest,
    cf_id: &str,
    now: SystemTime,
) -> (Option<ServedFor>, Plan) {
    let addressed = Addressed::of_request(req);
    let Some(resolved) = resolve(store, &addressed) else {
        // The distribution was deleted between the claim and now: an endpoint that serves
        // no distribution of this account is refused, and counted into no distribution's
        // series, there being none to count it into.
        return (
            None,
            Plan::Generated {
                status: FORBIDDEN,
                detail: BAD_REQUEST_DETAIL,
            },
        );
    };
    let behavior = select_behavior(resolved.distribution, resolved.viewer_path);
    let served_for = ServedFor {
        id: resolved.distribution.id.clone(),
        additional_metrics: edge_metrics::AdditionalMetrics::of_subscription(
            resolved.distribution.monitoring_subscription.as_ref(),
        ),
        web_acl_id: resolved
            .distribution
            .config
            .web_acl_id
            .clone()
            .unwrap_or_default(),
        viewer_path: resolved.viewer_path.to_string(),
        query: addressed.query.to_string(),
        domain_name: resolved.distribution.domain_name.clone(),
        country: viewer_country(req),
        path_pattern: behavior.path_pattern.clone(),
        realtime_log_config_arn: behavior.realtime_log_config_arn.clone(),
        // Resolved HERE, under the lock the whole plan is decided under, so the policy
        // applied to the answer is the one the behaviour attached when the request was
        // planned — a read of a policy AWS published included
        // (`readable_response_headers_policy`).
        response_headers_policy: behavior
            .response_headers_policy_id
            .as_deref()
            .and_then(|id| store.readable_response_headers_policy(id))
            .map(|policy| policy.config.clone()),
    };
    let plan = plan_resolved(store, &addressed, &resolved, req, cf_id, now, behavior);
    (Some(served_for), plan)
}

/// Decide what the edge does with one viewer request it has resolved to a distribution,
/// under the cache behaviour its path selected.
fn plan_resolved(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    addressed: &Addressed<'_>,
    resolved: &Resolved<'_, '_>,
    req: &RtRequest,
    cf_id: &str,
    now: SystemTime,
    behavior: Behavior,
) -> Plan {
    let distribution_id = resolved.distribution.id.clone();
    let viewer_path = resolved.viewer_path.to_string();
    if !resolved.distribution.config.enabled {
        // `DistributionConfig$Enabled`: "Whether the distribution is enabled to accept
        // user requests for content." A disabled distribution accepts none.
        return Plan::Generated {
            status: FORBIDDEN,
            detail: BAD_REQUEST_DETAIL,
        };
    }
    // Enforced HERE, by CloudFront, before the edge cache is read and before any origin
    // request exists: a viewer the distribution's geographic restriction refuses sees
    // CloudFront's own page and nothing of the origin — and nothing of a cached answer
    // either, so a restriction cannot be stepped around by asking for an object another
    // viewer already fetched.
    let country = viewer_country(req);
    if geo_restricts(resolved.distribution, &country) {
        ::tracing::debug!(
            distribution = %distribution_id,
            "this distribution's geographic restriction refuses this viewer's country; the \
             edge refuses the request and makes no origin request"
        );
        return Plan::Generated {
            status: FORBIDDEN,
            detail: GEO_RESTRICTED_DETAIL,
        };
    }
    // The behaviour the viewer's path selected, chosen once by the caller so the record this
    // request's log configuration is read off ([`realtime_logs`]) and the plan served here
    // are the SAME behaviour's.
    // Decided by CloudFront from the matched behaviour's `ViewerProtocolPolicy` alone,
    // before the cache is read and before any origin request exists: the SCHEME the viewer
    // dialled is a property of the connection, not of what it asked for.
    if let Some(plan) = viewer_protocol_plan(&behavior, addressed, resolved, req) {
        return plan;
    }
    let method = req.method().to_ascii_uppercase();
    if !behavior.allows(&method) {
        // Enforced HERE, by CloudFront, before any origin request exists: a method the
        // behaviour does not allow never reaches the origin.
        ::tracing::debug!(
            distribution = %distribution_id,
            path_pattern = ?behavior.path_pattern,
            %method,
            "the cache behavior does not allow this method; the edge refuses it and makes \
             no origin request"
        );
        return Plan::Generated {
            status: FORBIDDEN,
            detail: METHOD_NOT_ALLOWED_DETAIL,
        };
    }
    // Enforced HERE, by CloudFront, before the edge cache is read and before any origin
    // request exists: "when a cache behavior contains trusted key groups, CloudFront
    // requires signed URLs or signed cookies for all requests that match the cache
    // behavior". The groups and the public keys in them are read from the store on THIS
    // request, so a key an update has taken out of the group stops admitting the URLs it
    // signed; and because the decision is made before the cache, a signature that does not
    // hold cannot be answered with an object another viewer already fetched.
    if let Some(refusal) = signed_access::refusal(
        store,
        behavior.trusted_key_groups.as_ref(),
        &signed_access::SignedRequest {
            query: addressed.query,
            cookies: req.headers().get(COOKIE).unwrap_or_default(),
            resource_urls: resource_urls(
                resolved.distribution,
                generated_headers::viewer_scheme(req.uri()),
                &viewer_path,
                addressed.query,
            ),
            address: generated_headers::viewer_address(req),
        },
        now,
    ) {
        ::tracing::debug!(
            distribution = %distribution_id,
            path_pattern = ?behavior.path_pattern,
            code = refusal.code(),
            "this behaviour restricts its content to trusted signers and this request's \
             signed access did not hold; the edge refuses it and makes no origin request"
        );
        return Plan::Refused { refusal };
    }
    let ttl = cache_ttl(store, &behavior);
    // What the behaviour's POLICIES make of this request: which of its parts take part in
    // the cache key, which of them reach the origin, and what the cache policy makes of the
    // viewer's `Accept-Encoding` ([`forwarding`]).
    let keyed = forwarding::cache_key_parameters(store, &behavior);
    let forwarded = forwarding::forwarded_parameters(store, &behavior);
    let accept_encoding =
        forwarding::accept_encoding(&keyed, req.headers().get(ACCEPT_ENCODING));
    // "Whether you want CloudFront to automatically compress certain files for this cache
    // behavior": the coding this viewer gets, or none when the behaviour asks for no
    // compression or the viewer accepts neither coding the cache policy enables.
    let encode = if behavior.compress {
        accept_encoding.preferred()
    } else {
        None
    };
    // The `CloudFront-*` headers the EDGE generates for this request: their values are the
    // edge's own wherever one of those headers is read, the cache key included
    // ([`keyed_values`]).
    let generated = generated_headers::Generated::of_request(req);
    let cache_key = cache_key(
        &method,
        &viewer_path,
        &keyed_values(&keyed, &accept_encoding, addressed.query, req, &generated),
    );
    // "The HTTP methods for which CloudFront caches responses": a cacheable method under
    // a caching cache policy may be answered from the cache; everything else goes to the
    // origin every time.
    let cacheable = behavior.caches(&method).then_some(ttl).flatten();
    if cacheable.is_some() {
        if let Some(entry) = resolved.distribution.cached_edge_response(&cache_key, now) {
            return Plan::Hit {
                answer: OriginAnswer {
                    status: entry.status,
                    body: entry.body.clone(),
                    content_type: entry.content_type.clone(),
                    content_encoding: entry.content_encoding.clone(),
                },
                encode,
            };
        }
    }
    let Some(origin) = resolved
        .distribution
        .config
        .origins
        .items
        .iter()
        .find(|origin| origin.id == behavior.target_origin_id)
    else {
        // A behaviour whose `TargetOriginId` names no origin cannot be created, so this
        // is a configuration that lost its origin: there is nothing to fetch.
        return Plan::Generated {
            status: BAD_GATEWAY,
            detail: NO_ORIGIN_DETAIL,
        };
    };
    let headers = origin_request_headers(store, &behavior, origin, req, cf_id);
    // A REQUEST LOOP: the origin is a host the edge itself serves, or this request has
    // already been through an edge (its `Via` names CloudFront). Either way, fetching it
    // would hand the request back to the edge it came from, so the fetch is not made and
    // the request is answered the way an origin CloudFront cannot get a response from is.
    if serves_host(resolved.distribution, bare_host(&origin.domain_name))
        || carries_cloudfront_via(req)
    {
        ::tracing::warn!(
            distribution = %distribution_id,
            origin = %origin.domain_name,
            "this origin request would return to the edge it came from, so no origin \
             request is made"
        );
        return Plan::Generated {
            status: BAD_GATEWAY,
            detail: NO_ORIGIN_DETAIL,
        };
    }
    // The ORIGIN ACCESS CONTROL this origin carries, read from the same stored
    // configuration: what it says about this fetch is decided after the lock is released
    // (see [`origin_access`]), because the origin's own resource policy is read from the
    // origin's own service.
    let uri = origin_uri(
        origin,
        generated_headers::viewer_scheme(req.uri()),
        &viewer_path,
        &origin_query(&keyed, &forwarded, addressed.query),
    );
    // An origin request the edge cannot OPEN at all is reported as an origin CloudFront
    // could not reach, and no request is sent: the configuration decides the scheme
    // ([`origin_uri`]), and the origin either answers on it or does not exist on it.
    if !origin_accepts_scheme(&uri) {
        ::tracing::warn!(
            distribution = %distribution_id,
            origin = %origin.domain_name,
            "this origin's protocol policy addresses it over a scheme it does not serve, so \
             the origin connection cannot be completed"
        );
        return Plan::Generated {
            status: BAD_GATEWAY,
            detail: NO_ORIGIN_DETAIL,
        };
    }
    let access = origin_access::of_fetch(
        store,
        resolved.distribution,
        origin,
        &method,
        path_of(&uri),
        req.headers().get("authorization").is_some(),
    );
    let fetch = OriginFetch {
        distribution_id,
        uri,
        access,
        headers,
        read_timeout: origin_read_timeout(origin),
        cache: cacheable.map(|ttl| CacheDirective {
            key: cache_key,
            ttl,
        }),
        encode,
        viewer_path,
    };
    Plan::Fetch(Box::new(fetch))
}

/// The query string the ORIGIN is asked for: the viewer's own parameters that either policy
/// names, in the order the viewer sent them.
///
/// "The headers, cookies, and query strings that are included in the cache key are also
/// included in requests that CloudFront sends to the origin", and the origin request policy
/// names what reaches the origin beside those — so the origin sees the UNION of the two, and
/// a parameter neither policy names is withheld from it.
fn origin_query(
    keyed: &forwarding::CacheKeyParameters,
    forwarded: &forwarding::ForwardedParameters,
    query: &str,
) -> String {
    forwarding::selected_query(query, |name| {
        keyed.query_strings.contains(name) || forwarded.query_strings.contains(name)
    })
}

/// The key one request's answer is cached under: the method, the path, and the values of the
/// parts of the request the behaviour's cache policy puts in the cache key
/// ([`keyed_values`]).
///
/// Nothing the cache policy does NOT name takes part in it — "the headers, cookies, and
/// query strings that are included in the cache key" are what separates one object from
/// another — so two requests the policy says are the same object share one entry however
/// they differ elsewhere. Every part the request contributes is rendered under its own LENGTH
/// ([`keyed_values`]), so no value a viewer chooses can reproduce the rendering of a
/// different request: the key reads as the list of parts it is rather than as text.
fn cache_key(method: &str, viewer_path: &str, keyed: &str) -> String {
    format!("{method} {}:{viewer_path}#{keyed}", viewer_path.len())
}

/// The values of the parts of the request the behaviour's cache policy keys on, rendered as
/// the part of the cache key they contribute.
///
/// "The headers, cookies, and query strings that are included in the cache key": two
/// requests that differ in any value the policy names therefore have different cache keys,
/// so one viewer's answer is never served to a request the policy says is a different one.
/// Each kind of part is rendered under a prefix of its own, so a header and a query string
/// of the same name are two values rather than one, and each kind is rendered in the order
/// that kind reaches the origin in: headers and cookies by name, the query string as the
/// viewer sent it — a query string is keyed on whole, so two requests that carry the same
/// parameters in a different order are two objects. The normalized `Accept-Encoding` is
/// keyed on last — "includes the normalized header in the cache key".
///
/// A header the EDGE generates contributes the EDGE'S OWN value ([`generated_headers`]),
/// never the one a viewer stated for it: such a header describes the viewer's CONNECTION, so
/// the value the origin request carries for it is the value that separates one object from
/// another — a policy keying on the forwarded scheme really does give the two schemes two
/// entries, and a viewer that states a `CloudFront-*` header of its own keys on nothing of its
/// own.
///
/// Each part is rendered under its own LENGTH, so a delimiter inside a value a viewer chose
/// cannot make one request's parts render as another's.
fn keyed_values(
    keyed: &forwarding::CacheKeyParameters,
    accept_encoding: &forwarding::AcceptEncoding,
    query: &str,
    req: &RtRequest,
    generated: &generated_headers::Generated,
) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (name, value) in req.headers().iter() {
        let name = name.to_ascii_lowercase();
        // A value the viewer stated for a header the edge generates is not that header's
        // value, so it takes no part in the key; the edge's own is added below.
        if generated_headers::is_generated(&name) {
            continue;
        }
        if keyed.headers.contains_header(&name) {
            parts.push(format!("h:{name}={value}"));
        }
    }
    for (name, value) in generated.headers() {
        if keyed.headers.contains_header(&name) {
            parts.push(format!("h:{name}={value}"));
        }
    }
    for name in keyed.headers.named() {
        // A `CloudFront-*` header is never one the VIEWER sent, so it is not keyed on as a
        // header the viewer did not send: the two the edge generates are keyed on by their
        // generated value above, and another member of the family contributes nothing to the
        // key at all, as it reaches no origin request either.
        if generated_headers::is_generated(name) {
            continue;
        }
        if req.headers().get(name).is_none() {
            parts.push(format!("h:{}", name.to_ascii_lowercase()));
        }
    }
    parts.sort();
    let cookie = req.headers().get(COOKIE).unwrap_or_default();
    parts.extend(
        forwarding::keyed_pairs(
            &forwarding::cookie_pairs(cookie),
            &keyed.cookies,
            forwarding::PairOrder::ByName,
        )
        .into_iter()
        .map(|pair| format!("c:{pair}")),
    );
    parts.extend(
        forwarding::keyed_pairs(
            &forwarding::query_pairs(query),
            &keyed.query_strings,
            forwarding::PairOrder::AsSent,
        )
        .into_iter()
        .map(|pair| format!("q:{pair}")),
    );
    if let Some(value) = accept_encoding.header_value() {
        parts.push(format!("{ACCEPT_ENCODING}={value}"));
    }
    // Each part under its own length, so a `;` (or a `#`, or an `=`) inside a value a viewer
    // chose cannot render as the boundary between two parts.
    parts
        .iter()
        .map(|part| format!("{}:{part}", part.len()))
        .collect::<Vec<_>>()
        .join(";")
}

/// Whether `req` has already been through a CloudFront edge: its `Via` names CloudFront,
/// which is the header an edge stamps on every origin request it makes
/// ([`origin_request_headers`]).
///
/// Read on the INBOUND path, which is what makes the stamp a loop signal rather than
/// decoration: a request that reaches the edge carrying it came from an edge.
fn carries_cloudfront_via(req: &RtRequest) -> bool {
    req.headers()
        .get("via")
        .is_some_and(|via| via.contains(VIA_CLOUDFRONT))
}

/// Whether the origin an origin request is addressed at SERVES the scheme that request is
/// addressed over, so the connection can be opened at all.
///
/// A plaintext origin request reaches only an origin that answers plaintext, and the AWS
/// SERVICE ENDPOINTS this emulator serves answer over TLS alone: every one of them is
/// reached at this emulator's own front door, which is ONE self-signed HTTPS listener and
/// refuses a plaintext request to that port (`vendor/server/src/lib.rs`, whose production
/// contract is a single TLS data listener). So an origin whose `OriginProtocolPolicy`
/// addresses one of them over `http` — an API Gateway stage's `execute-api` endpoint, a
/// bucket's own endpoint, any other service's — names an endpoint that answers nothing on
/// that scheme, which is "CloudFront can't connect to the origin" and the `502` an
/// unreachable origin is reported with. The divergence that follows is named rather than
/// hidden: AWS's own S3 REST endpoint does serve HTTP, so an `http-only` custom origin over
/// a bucket endpoint is served there and is this `502` here, because this emulator serves
/// that endpoint over TLS only.
///
/// Two kinds of host are NOT such an endpoint. A per-resource listener this emulator opens
/// negotiates its encryption in band, so it answers both schemes at one address — a
/// distribution's own edge domain among them ([`crate::state::edge_endpoint`]). And a host
/// of nobody's this emulator serves at all is reached over either scheme and answers on
/// neither, which the fetch itself reports.
fn origin_accepts_scheme(uri: &str) -> bool {
    let Some((scheme, _rest)) = uri.split_once("://") else {
        return true;
    };
    if !scheme.eq_ignore_ascii_case(PLAINTEXT_SCHEME) {
        return true;
    }
    let Some(authority) = authority_of(uri) else {
        return true;
    };
    !names_a_service_endpoint(bare_host(authority))
}

/// Whether `host` is an AWS SERVICE ENDPOINT of this emulator: a host the front door
/// resolves a service from ([`service_named_by_host`]) in one of the zones AWS serves its
/// service endpoints in.
///
/// The edge zone is not one of them: `cloudfront.net` carries the domain names
/// DISTRIBUTIONS publish, which are addresses of this emulator's own per-distribution
/// listeners rather than of the front door, and those serve both schemes.
fn names_a_service_endpoint(host: &str) -> bool {
    if service_named_by_host(host).is_none() {
        return false;
    }
    let lowercase = host.to_ascii_lowercase();
    AWS_ENDPOINT_SUFFIXES
        .iter()
        .filter(|suffix| **suffix != EDGE_ZONE_SUFFIX)
        .any(|suffix| lowercase.ends_with(suffix))
}

/// The zone a distribution's own published domain name is in, which is the one zone of
/// [`AWS_ENDPOINT_SUFFIXES`] that names no service endpoint.
const EDGE_ZONE_SUFFIX: &str = ".cloudfront.net";

/// The absolute URI of the origin request: the origin's own host, its `OriginPath`, the
/// viewer path and the viewer query.
///
/// `Origin$OriginPath` is "an optional path that CloudFront appends to the origin domain
/// name when CloudFront requests content". The scheme and port come from the origin's own
/// configuration: an `S3OriginConfig` origin is reached over HTTPS, and a
/// `CustomOriginConfig` one over the protocol its `OriginProtocolPolicy` names, on the
/// matching port — `http-only` is "CloudFront uses only HTTP to access the origin",
/// `https-only` only HTTPS, and `match-viewer` is CloudFront "communicating with your
/// origin using HTTP or HTTPS, depending on the protocol of the viewer request", which is
/// what `viewer_scheme` carries.
fn origin_uri(origin: &Origin, viewer_scheme: &str, viewer_path: &str, query: &str) -> String {
    let (scheme, port) = match &origin.custom_origin_config {
        Some(custom) => match custom.origin_protocol_policy.as_str() {
            ORIGIN_HTTP_ONLY => (PLAINTEXT_SCHEME, custom.http_port),
            ORIGIN_MATCH_VIEWER if viewer_scheme == PLAINTEXT_SCHEME => {
                (PLAINTEXT_SCHEME, custom.http_port)
            }
            // `https-only`, `match-viewer` for a viewer that dialled HTTPS, and a policy
            // value this emulator does not know: the origin is reached over TLS.
            _ => (SECURE_SCHEME, custom.https_port),
        },
        None => (SECURE_SCHEME, HTTPS_DEFAULT_PORT),
    };
    let authority = match (scheme, port) {
        (PLAINTEXT_SCHEME, HTTP_DEFAULT_PORT) | (SECURE_SCHEME, HTTPS_DEFAULT_PORT) => {
            origin.domain_name.clone()
        }
        _ => format!("{}:{port}", origin.domain_name),
    };
    let origin_path = origin
        .origin_path
        .as_deref()
        .unwrap_or_default()
        .trim_end_matches('/');
    let path = if viewer_path.starts_with('/') {
        viewer_path.to_string()
    } else {
        format!("/{viewer_path}")
    };
    if query.is_empty() {
        format!("{scheme}://{authority}{origin_path}{path}")
    } else {
        format!("{scheme}://{authority}{origin_path}{path}?{query}")
    }
}

/// The longest an origin fetch may wait, whatever the configuration asks for:
/// `OriginReadTimeout`'s own documented maximum ("the minimum timeout is 1 second, the
/// maximum is 120 seconds"), so one viewer request can never hold a worker longer than
/// the member admits.
const MAX_ORIGIN_READ_TIMEOUT_SECONDS: u64 = 120;

/// How long the edge waits for the origin's response.
///
/// `CustomOriginConfig$OriginReadTimeout` / `S3OriginConfig$OriginReadTimeout`, in
/// seconds, and [`DEFAULT_ORIGIN_READ_TIMEOUT`] for an origin that names none, clamped to
/// the range that member documents.
fn origin_read_timeout(origin: &Origin) -> Duration {
    let seconds = origin
        .custom_origin_config
        .as_ref()
        .and_then(|custom| custom.origin_read_timeout)
        .or_else(|| {
            origin
                .s3_origin_config
                .as_ref()
                .and_then(|s3| s3.origin_read_timeout)
        })
        .unwrap_or(DEFAULT_ORIGIN_READ_TIMEOUT);
    let seconds = u64::try_from(seconds).unwrap_or(0);
    Duration::from_secs(seconds.clamp(1, MAX_ORIGIN_READ_TIMEOUT_SECONDS))
}

/// The headers one origin request carries.
///
/// The origin's own `CustomHeaders` ("CloudFront adds these headers to every origin
/// request"), the `Host` of the origin, the `X-Amz-Cf-Id` of the request the origin is
/// being fetched for, a `Via` naming CloudFront, the `User-Agent` CloudFront sends when
/// the viewer's is not forwarded, the viewer headers the behaviour's ORIGIN REQUEST POLICY
/// forwards, the ones its CACHE POLICY keys on — "the headers, cookies, and query strings
/// that are included in the cache key are also included in requests that CloudFront sends
/// to the origin" ([`forwarding`]) — and the headers the EDGE ITSELF generates that the
/// origin request policy names ([`generated_headers`]).
///
/// The cookies travel as the two policies select them, one `name=value` at a time, and the
/// `Accept-Encoding` travels normalized where the cache policy normalizes it. A header that
/// belongs to one hop of a connection, the viewer's own credential, and a value the viewer
/// stated for a header CloudFront generates itself are never forwarded
/// ([`HEADERS_NEVER_FORWARDED`], [`generated_headers::is_generated`]).
fn origin_request_headers(
    store: &ComAmazonawsCloudfrontCloudfront20200531State,
    behavior: &Behavior,
    origin: &Origin,
    req: &RtRequest,
    cf_id: &str,
) -> Vec<(String, String)> {
    let keyed = forwarding::cache_key_parameters(store, behavior);
    let forwarded = forwarding::forwarded_parameters(store, behavior);
    let accept_encoding = forwarding::accept_encoding(&keyed, req.headers().get(ACCEPT_ENCODING));
    let generated = generated_headers::Generated::of_request(req);
    let mut headers: Vec<(String, String)> = Vec::new();
    for (name, value) in req.headers().iter() {
        let name = name.to_ascii_lowercase();
        if HEADERS_NEVER_FORWARDED.contains(&name.as_str()) || name == "host" {
            continue;
        }
        // A `CloudFront-*` header describes the viewer's CONNECTION, so the value the viewer
        // stated for one is dropped here: the edge's own value is added below, where the
        // policy names it.
        if generated_headers::is_generated(&name) {
            continue;
        }
        // The cookies and the negotiated content coding are selected and normalized below,
        // rather than forwarded as the viewer sent them.
        if name == COOKIE || (name == ACCEPT_ENCODING && accept_encoding.normalizes()) {
            continue;
        }
        // The body's own description travels with the body, whatever the policies say
        // about the viewer's other headers.
        let carries_body = name == CONTENT_TYPE || name == "content-length";
        if carries_body
            || keyed.headers.contains_header(&name)
            || forwarded.headers.contains_header(&name)
        {
            headers.push((name, value.to_string()));
        }
    }
    // The cookies the cache key carries, and the ones the origin request policy forwards
    // beside them, in the order the viewer sent them; a cookie neither policy names is
    // withheld from the origin, and a request whose cookies are all withheld carries no
    // `Cookie` header at all.
    if let Some(cookie) = req.headers().get(COOKIE) {
        let selected = forwarding::selected_cookies(cookie, |name| {
            keyed.cookies.contains(name) || forwarded.cookies.contains(name)
        });
        if let Some(selected) = selected {
            headers.push((COOKIE.to_string(), selected));
        }
    }
    // "CloudFront always includes the `Accept-Encoding` header in origin requests when the
    // value of this field is `true`" — as the normalized value, which is also what the cache
    // key carries.
    if let Some(value) = accept_encoding.header_value() {
        headers.push((ACCEPT_ENCODING.to_string(), value));
    }
    headers.push(("host".to_string(), origin.domain_name.clone()));
    headers.push((X_AMZ_CF_ID.to_string(), cf_id.to_string()));
    headers.push((
        "via".to_string(),
        format!("1.1 {cf_id}.cloudfront.net ({VIA_CLOUDFRONT})"),
    ));
    headers.push(("user-agent".to_string(), ORIGIN_USER_AGENT.to_string()));
    // The headers the edge generated for this request, where the origin request policy names
    // them: a behaviour with no origin request policy, or one whose policy does not list a
    // generated header, sends the origin none of them.
    for (name, value) in generated.headers() {
        if forwarded.generated_headers.contains_header(&name) {
            headers.push((name, value));
        }
    }
    for custom in origin
        .custom_headers
        .as_ref()
        .and_then(|custom| custom.items.as_ref())
        .into_iter()
        .flatten()
    {
        headers.push((
            custom.header_name.to_ascii_lowercase(),
            custom.header_value.clone(),
        ));
    }
    headers
}

/// One answer the edge reached for the origin to produce, and what the metrics read it by:
/// the response the viewer gets, and — when an ORIGIN really answered — how long the edge had
/// been working on the request when that answer was ready, which is the span
/// [`edge_metrics::ORIGIN_LATENCY_METRIC`] measures. `origin_latency` is `None` for every path
/// where no origin answered, so an answer CloudFront produced for an origin it could not reach
/// is not reported as an origin's.
struct Fetched {
    response: RtResponse,
    origin_latency: Option<Duration>,
}

impl Fetched {
    /// An answer CloudFront produced itself, no origin having answered.
    fn of_cloudfront(response: RtResponse) -> Self {
        Self {
            response,
            origin_latency: None,
        }
    }
}

/// Fetch the origin, serve what it answered, and cache it when the behaviour caches it.
///
/// The fetch goes through the transport `OperationContext::callback` hands this handler,
/// so an origin this emulator serves answers it in process. An origin that answers
/// nothing at all is reported as CloudFront reports an origin it could not reach.
async fn fetch_origin<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &OperationContext<'_>,
    fetch: OriginFetch,
    cf_id: &str,
    req: &RtRequest,
    received: Instant,
) -> Fetched {
    let body = req.body().bytes().map(<[u8]>::to_vec).unwrap_or_default();
    let mut builder = ::http::Request::builder()
        .method(req.method())
        .uri(&fetch.uri);
    for (name, value) in &fetch.headers {
        builder = builder.header(name, value);
    }
    let built = builder.body(body).map_err(|error| error.to_string());
    let mut request = match built {
        Ok(request) => request,
        Err(error) => {
            ::tracing::warn!(
                uri = %fetch.uri,
                %error,
                "the configured origin does not render an HTTP request, so no origin \
                 request was made"
            );
            return Fetched::of_cloudfront(generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, cf_id));
        }
    };
    // The ORIGIN ACCESS CONTROL's own half of the fetch: a request the origin's resource
    // policy grants this distribution is SIGNED, which is what such an origin serves a
    // request for at all; one it does not grant travels as it would with no control, so the
    // origin answers it itself (see [`origin_access`]).
    //
    // A granted fetch is signed AS CloudFront's own service principal, acting for this
    // distribution: the credential it is signed with is minted for this one request and
    // recorded as that call, so a decision the origin's service makes itself about it (the
    // key policy of a KMS key the object's bytes are encrypted under) is made for that
    // principal rather than for the account. Both the credential and the record are destroyed
    // when `_as_the_service_principal` is dropped, which is every path out of this function.
    let mut _as_the_service_principal = None;
    if let Some(access) = &fetch.access {
        if origin_access::grants_origin_request(ctx, access, fetch.read_timeout).await {
            match origin_access::sign_as_the_service_principal(
                engine,
                &mut request,
                access,
                ctx,
                fetch.read_timeout,
            )
            .await
            {
                Ok(in_flight) => _as_the_service_principal = in_flight,
                Err(error) => ::tracing::warn!(
                    uri = %fetch.uri,
                    %error,
                    "the origin request could not be signed for this origin access control, \
                     so it is made as a request carrying no credential"
                ),
            }
        }
    }
    let (parts, body) = request.into_parts();
    let built = RtRequest::try_from(::http::Request::from_parts(
        parts,
        SdkBody::from(::bytes::Bytes::from(body)),
    ))
    .map_err(|error| error.to_string());
    let request = match built {
        Ok(request) => request,
        Err(error) => {
            ::tracing::warn!(
                uri = %fetch.uri,
                %error,
                "the configured origin does not render an HTTP request, so no origin \
                 request was made"
            );
            return Fetched::of_cloudfront(generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, cf_id));
        }
    };
    let transport = ctx.callback().http_connector();
    let answered = ::tokio::time::timeout(fetch.read_timeout, transport.call(request)).await;
    let response = match answered {
        Err(_elapsed) => {
            ::tracing::warn!(
                uri = %fetch.uri,
                timeout_seconds = fetch.read_timeout.as_secs(),
                "the origin did not answer within its read timeout"
            );
            return Fetched::of_cloudfront(generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, cf_id));
        }
        Ok(Err(error)) => {
            ::tracing::warn!(
                uri = %fetch.uri,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "the configured origin could not be reached"
            );
            return Fetched::of_cloudfront(generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, cf_id));
        }
        Ok(Ok(response)) => response,
    };
    // The origin has answered, so this is the span from the edge receiving the viewer request
    // to the answer being ready: what the origin latency of this request is.
    let origin_latency = received.elapsed();
    let status = response.status().as_u16();
    // What the ORIGIN said about its own bytes: the media type, which decides whether the
    // edge may compress them, and the coding they are already under, which means it must
    // not.
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .map(ToString::to_string);
    let content_encoding = response
        .headers()
        .get(CONTENT_ENCODING)
        .map(ToString::to_string);
    let body = response
        .into_body()
        .bytes()
        .map(<[u8]>::to_vec)
        .unwrap_or_default();
    if is_transport_refusal(&body) {
        ::tracing::warn!(
            uri = %fetch.uri,
            "nothing in this emulator serves the configured origin, so no origin response \
             exists to serve"
        );
        return Fetched::of_cloudfront(generated(BAD_GATEWAY, NO_ORIGIN_DETAIL, cf_id));
    }
    let answer = OriginAnswer {
        status,
        body,
        content_type,
        content_encoding,
    };
    if let Some(directive) = fetch.cache {
        // The status the edge may hold at all; the size bound the entry has to stay within is
        // [`cache`]'s own, stated where the bytes are held.
        if is_cacheable_status(status) {
            cache(
                engine,
                &fetch.distribution_id,
                directive,
                &fetch.viewer_path,
                &answer,
            );
        }
    }
    // The origin answered, so the answer is the origin's — a MISS, since this request is
    // what went to the origin. An origin answer that is itself an error is reported as
    // one CloudFront did not produce from its cache either.
    let x_cache = if is_cacheable_status(status) || status < 400 {
        CACHE_MISS
    } else {
        CACHE_ERROR
    };
    Fetched {
        response: served_origin_answer(&answer, fetch.encode, x_cache, cf_id),
        origin_latency: Some(origin_latency),
    }
}

/// The error types that are the in-process TRANSPORT's own refusal rather than an answer
/// from an origin: this emulator serves nothing at that host, or more than one of its
/// services claims it. Either way the fetch reached no origin, which is the same class as
/// an origin that never answered.
///
/// This is the shape the Lambda-function-URL origin takes here: no service of this
/// emulator claims a function URL host, so the fetch of such an origin reaches nothing.
const TRANSPORT_REFUSALS: [&str; 2] = ["UnknownService", "AmbiguousService"];

/// How much of an answer is read to tell the transport's own refusal from an origin's
/// response. The refusal envelope is two short fields; anything larger is a response from
/// something that served the request, so it is not read at all.
const MAX_REFUSAL_ENVELOPE_BYTES: usize = 4 * 1024;

/// Whether `body` is the in-process transport's OWN refusal of the origin request rather
/// than a response from an origin that served it.
///
/// The refusal is told by the error type its envelope carries ([`TRANSPORT_REFUSALS`]);
/// the bridge the request crosses keeps the body but not the headers, so the envelope is
/// where that type can be read at all. The same reading
/// `services/service-com-amazonaws-eventbridge-awsevents` makes of an api-destination
/// delivery nothing served.
fn is_transport_refusal(body: &[u8]) -> bool {
    if body.len() > MAX_REFUSAL_ENVELOPE_BYTES {
        return false;
    }
    let Ok(envelope) = ::serde_json::from_slice::<::serde_json::Value>(body) else {
        return false;
    };
    let Some(error_type) = envelope.get("__type").and_then(::serde_json::Value::as_str) else {
        return false;
    };
    // The type may carry a shape namespace; the code is its last segment.
    let code = error_type.rsplit('#').next().unwrap_or(error_type);
    TRANSPORT_REFUSALS.contains(&code)
}

/// The largest origin response this edge holds in its cache.
///
/// A cached answer is held in memory beside the distribution, so what one viewer request
/// can add to the process is bounded here as well as by the entry count: a response larger
/// than this is served to this viewer and not held, which a caller sees only as a cache
/// MISS. This emulator's own bound, not CloudFront's.
const MAX_CACHED_BODY_BYTES: usize = 1024 * 1024;

/// Whether a status the origin answered is one the edge may hold in its cache.///
/// "CloudFront caches ... responses with the following status codes: 200, 203, 204, 206,
/// 300, 301, 302, 303, 304, 307, 308, 404, 405, 410, 414, and 501" — the successful,
/// redirect and documented error statuses; anything else is served to this viewer and not
/// held.
fn is_cacheable_status(status: u16) -> bool {
    const CACHEABLE: [u16; 15] = [
        200, 203, 204, 206, 300, 301, 302, 303, 304, 307, 308, 404, 405, 410, 414,
    ];
    CACHEABLE.contains(&status) || status == 501
}

/// Hold one origin answer in the distribution's edge cache.
///
/// The ORIGIN'S OWN bytes are held, under its own description of them: what the edge serves
/// from the entry is computed for each viewer request ([`served_origin_answer`]), so an
/// object cached for one viewer is not held under another viewer's content coding.
///
/// The size bound is enforced HERE, where the bytes are held, as well as where the plan asks
/// for them to be: an answer larger than [`MAX_CACHED_BODY_BYTES`] is served to this viewer
/// and not held, so what one viewer request can add to this process stays bounded whoever
/// calls this.
fn cache<Eng: Send + Sync + 'static>(
    engine: &Eng,
    distribution_id: &str,
    directive: CacheDirective,
    viewer_path: &str,
    answer: &OriginAnswer,
) {
    if answer.body.len() > MAX_CACHED_BODY_BYTES {
        ::tracing::debug!(
            distribution = %distribution_id,
            bytes = answer.body.len(),
            "this origin answer is larger than the edge cache holds, so it is served to this \
             viewer and not held"
        );
        return;
    }
    let Some(state) = crate::state::service_state(engine) else {
        return;
    };
    let now = SystemTime::now();
    // The expiry is computed BEFORE the store is locked and without arithmetic that can
    // fail: a TTL the clock cannot reach holds the entry for the longest span the clamp
    // admits rather than making the instant uncomputable.
    let Some(expires_at) = now.checked_add(directive.ttl) else {
        ::tracing::warn!(
            distribution = %distribution_id,
            ttl_seconds = directive.ttl.as_secs(),
            "this cache behavior's TTL runs past the clock, so the answer is served to \
             this viewer and not held"
        );
        return;
    };
    let mut store = state.lock();
    let Some(distribution) = store.distributions.get_mut(distribution_id) else {
        return;
    };
    distribution.store_edge_response(
        directive.key,
        CachedEdgeResponse {
            status: answer.status,
            body: answer.body.clone(),
            content_type: answer.content_type.clone(),
            content_encoding: answer.content_encoding.clone(),
            path: viewer_path.to_string(),
            stored_at: now,
            expires_at,
        },
    );
}

/// One answer the edge serves on an ORIGIN's behalf, whether the origin produced it for this
/// request or the edge cache held it: the origin's bytes under the origin's own description
/// of them, compressed with `encode` where the behaviour asks for compression and these
/// bytes are compressible ([`compression`]).
///
/// An answer the origin already encoded keeps the origin's bytes and the origin's coding: it
/// is not encoded twice, and nothing is relabelled. An answer larger than the size
/// CloudFront's compression documents ([`compression::MAX_COMPRESSED_BYTES`]) is served as
/// the origin sent it, which also bounds the work one viewer request asks of this process.
fn served_origin_answer(
    answer: &OriginAnswer,
    encode: Option<Encoding>,
    x_cache: &str,
    cf_id: &str,
) -> RtResponse {
    let content_type = answer.content_type.as_deref();
    let mut body = answer.body.clone();
    let mut encoding = answer.content_encoding.clone();
    if encoding.is_none() {
        let compressible = compression::compressible(content_type)
            && body.len() <= compression::MAX_COMPRESSED_BYTES;
        if let Some(coding) = encode.filter(|_| compressible) {
            if let Some(compressed) = coding.compress(&body) {
                body = compressed;
                encoding = Some(coding.token().to_string());
            }
        }
    }
    let mut response = served(answer.status, body, x_cache, cf_id);
    if let Some(content_type) = content_type {
        let _ = response
            .headers_mut()
            .try_insert(CONTENT_TYPE, content_type.to_string());
    }
    if let Some(encoding) = encoding {
        let _ = response.headers_mut().try_insert(CONTENT_ENCODING, encoding);
    }
    response
}

/// One answer the edge serves on behalf of an origin, or out of its cache.
fn served(status: u16, body: Vec<u8>, x_cache: &str, cf_id: &str) -> RtResponse {
    let mut response = RtResponse::new(status_code(status), SdkBody::from(body));
    stamp(&mut response, x_cache, cf_id);
    response
}

/// One answer CloudFront generated itself, as its own error page.
///
/// The page is the one CloudFront serves for a request it did not pass to an origin: the
/// status, the sentence it heads the page with, the detail line for that failure, and the
/// identifier of the request it answered, which is what a report to AWS Support quotes.
fn generated(status: u16, detail: &str, cf_id: &str) -> RtResponse {
    let page = format!(
        "<!DOCTYPE HTML PUBLIC \"-//W3C//DTD HTML 4.01 Transitional//EN\" \
         \"http://www.w3.org/TR/html4/loose.dtd\">\n\
         <HTML><HEAD><META HTTP-EQUIV=\"Content-Type\" CONTENT=\"text/html; \
         charset=iso-8859-1\">\n\
         <TITLE>ERROR: The request could not be satisfied</TITLE>\n\
         </HEAD><BODY>\n\
         <H1>{status} ERROR</H1>\n\
         <H2>The request could not be satisfied.</H2>\n\
         <HR noshade size=\"1px\">\n\
         {detail}\n\
         <BR clear=\"all\">\n\
         <HR noshade size=\"1px\">\n\
         <PRE>Generated by cloudfront (CloudFront)\nRequest ID: {cf_id}\n</PRE>\n\
         <ADDRESS></ADDRESS>\n\
         </BODY></HTML>"
    );
    let mut response = RtResponse::new(status_code(status), SdkBody::from(page.into_bytes()));
    let _ = response
        .headers_mut()
        .try_insert("content-type", "text/html".to_string());
    let _ = response
        .headers_mut()
        .try_insert("server", SERVER.to_string());
    stamp(&mut response, CACHE_ERROR, cf_id);
    response
}

/// The redirect CloudFront answers a viewer whose behaviour is `redirect-to-https`: the
/// status, the HTTPS URL to resubmit the request to, and the page CloudFront serves beside
/// it.
fn redirected(location: &str, cf_id: &str) -> RtResponse {
    let page = "<html>\n<head><title>301 Moved Permanently</title></head>\n<body>\n\
                <center><h1>301 Moved Permanently</h1></center>\n\
                <hr><center>CloudFront</center>\n</body>\n</html>\n";
    let mut response = RtResponse::new(
        status_code(MOVED_PERMANENTLY),
        SdkBody::from(page.as_bytes().to_vec()),
    );
    let _ = response
        .headers_mut()
        .try_insert(LOCATION, location.to_string());
    let _ = response
        .headers_mut()
        .try_insert(CONTENT_TYPE, "text/html".to_string());
    let _ = response
        .headers_mut()
        .try_insert("server", SERVER.to_string());
    stamp(&mut response, CACHE_ERROR, cf_id);
    response
}

/// Put the headers every edge answer carries on `response`: its cache outcome and the
/// identifier of the request it answers.
fn stamp(response: &mut RtResponse, x_cache: &str, cf_id: &str) {
    let _ = response
        .headers_mut()
        .try_insert(X_CACHE, x_cache.to_string());
    if !cf_id.is_empty() {
        let _ = response
            .headers_mut()
            .try_insert(X_AMZ_CF_ID, cf_id.to_string());
    }
}

/// `status` as a status code, falling back to the one CloudFront answers when it got no
/// origin response — the only way a status this module computed is not a status code is
/// if an origin answered one the wire cannot carry.
fn status_code(status: u16) -> StatusCode {
    StatusCode::try_from(status).unwrap_or_else(|_| {
        StatusCode::try_from(BAD_GATEWAY).expect("502 is a valid HTTP status code")
    })
}

/// The `Host` header of a buffered request, or the URI authority when the request is
/// HTTP/2 (which carries `:authority` on the URI instead of a `Host` header).
fn request_host(parts: &http::request::Parts) -> Option<&str> {
    match parts.headers.get("host").and_then(|host| host.to_str().ok()) {
        Some(host) => Some(host),
        None => parts.uri.authority().map(http::uri::Authority::as_str),
    }
}

/// The authority of an absolute request URI, or `None` when the URI is origin-form.
fn authority_of(uri: &str) -> Option<&str> {
    let (_scheme, rest) = uri.split_once("://")?;
    Some(match rest.find('/') {
        Some(path_start) => &rest[..path_start],
        None => rest,
    })
}

/// The path component of a request URI, whether it is origin-form or absolute.
fn path_of(uri: &str) -> &str {
    let after_authority = match uri.split_once("://") {
        Some((_scheme, rest)) => match rest.find('/') {
            Some(path_start) => &rest[path_start..],
            None => "/",
        },
        None => uri,
    };
    match after_authority.find('?') {
        Some(mark) => &after_authority[..mark],
        None => after_authority,
    }
}

/// The query component of a request URI: everything after the first `?`.
fn query_of(uri: &str) -> &str {
    match uri.find('?') {
        Some(mark) => &uri[mark + 1..],
        None => "",
    }
}

#[cfg(test)]
mod tests {
    //! The pure rules this module owns that a wire test cannot vary cheaply: how a URI is
    //! split into an addressing, and how an origin request URI is built. The served
    //! behaviour is pinned end to end in `tests/all/edge_data_plane.rs`.

    use super::*;
    use crate::ops::types::CustomOriginConfig;

    #[test]
    fn a_uri_is_split_into_its_path_and_its_query() {
        // Rows differ along the form of the URI (origin-form, absolute), whether it
        // carries a query, and whether it carries a path at all.
        let cases: [(&str, &str, &str); 6] = [
            ("/", "/", ""),
            ("/opt/thing", "/opt/thing", ""),
            ("/opt/thing?a=1&b=2", "/opt/thing", "a=1&b=2"),
            ("https://d1.cloudfront.net/opt", "/opt", ""),
            ("https://d1.cloudfront.net/opt?a=1", "/opt", "a=1"),
            ("https://d1.cloudfront.net", "/", ""),
        ];
        for (uri, path, query) in cases {
            let (actual_path, actual_query) = (path_of(uri), query_of(uri));
            assert_eq!(
                (actual_path, actual_query),
                (path, query),
                "case {uri:?}: expected the path {path:?} and the query {query:?}, got \
                 {actual_path:?} and {actual_query:?}"
            );
        }
    }

    #[test]
    fn the_data_plane_path_prefix_names_the_distribution_before_the_viewer_path() {
        // Rows differ along what follows the prefix (an id with a viewer path, an id
        // alone), along the spelling of the prefix itself, and along paths that carry no
        // prefix at all.
        let cases: [(&str, Option<(&str, &str)>); 6] = [
            ("/_aws/cloudfront/E123/opt/x", Some(("E123", "/opt/x"))),
            ("/_aws/cloudfront/E123", Some(("E123", "/"))),
            ("/_aws/cloudfront/E123/", Some(("E123", "/"))),
            ("/_aws/cloudfront", None),
            ("/_aws/execute-api/abc/stage", None),
            ("/2020-05-31/distribution", None),
        ];
        for (path, expected) in cases {
            let actual = split_edge_prefix(path);
            assert_eq!(
                actual, expected,
                "case {path:?}: expected {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn an_origin_request_uri_carries_the_origins_scheme_port_and_path() {
        // Rows differ along the origin protocol policy, the ports, the origin path and
        // the viewer query — everything the rule generalises over.
        let custom = |policy: &str, http_port: i32, https_port: i32| CustomOriginConfig {
            http_port,
            https_port,
            origin_protocol_policy: policy.to_string(),
            ..CustomOriginConfig::default()
        };
        let origin = |config: Option<CustomOriginConfig>, origin_path: Option<&str>| Origin {
            id: "primary".to_string(),
            domain_name: "origin.example.com".to_string(),
            origin_path: origin_path.map(str::to_string),
            custom_origin_config: config,
            ..Origin::default()
        };
        // (case, the origin, the scheme the viewer dialled, the viewer path, the query, the
        // origin URI)
        let cases: [(&str, Origin, &str, &str, &str, &str); 8] = [
            (
                "an https-only origin on the default port",
                origin(Some(custom("https-only", 80, 443)), None),
                "https",
                "/index.html",
                "",
                "https://origin.example.com/index.html",
            ),
            (
                "an http-only origin on the default port",
                origin(Some(custom("http-only", 80, 443)), None),
                "https",
                "/index.html",
                "",
                "http://origin.example.com/index.html",
            ),
            (
                "a match-viewer origin for a viewer that dialled plaintext",
                origin(Some(custom("match-viewer", 80, 443)), None),
                "http",
                "/index.html",
                "",
                "http://origin.example.com/index.html",
            ),
            (
                "a match-viewer origin for a viewer that dialled TLS",
                origin(Some(custom("match-viewer", 80, 443)), None),
                "https",
                "/index.html",
                "",
                "https://origin.example.com/index.html",
            ),
            (
                "a match-viewer origin on an http port of its own",
                origin(Some(custom("match-viewer", 8080, 443)), None),
                "http",
                "/index.html",
                "",
                "http://origin.example.com:8080/index.html",
            ),
            (
                "an https origin on a port of its own",
                origin(Some(custom("https-only", 80, 8443)), None),
                "https",
                "/index.html",
                "",
                "https://origin.example.com:8443/index.html",
            ),
            (
                "an origin path the viewer path is appended to",
                origin(Some(custom("https-only", 80, 443)), Some("/prefix")),
                "https",
                "/index.html",
                "",
                "https://origin.example.com/prefix/index.html",
            ),
            (
                "an S3 origin, over https, carrying the viewer query",
                origin(None, None),
                "http",
                "/key",
                "a=1",
                "https://origin.example.com/key?a=1",
            ),
        ];
        for (case, origin, viewer_scheme, viewer_path, query, expected) in cases {
            let actual = origin_uri(&origin, viewer_scheme, viewer_path, query);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the origin URI {expected:?}, got {actual:?}"
            );
        }
    }

    #[test]
    fn an_origin_addressed_over_a_scheme_it_does_not_serve_cannot_be_connected_to() {
        // Rows differ along the scheme the origin request is addressed over and along the
        // KIND of host it names: an AWS service endpoint this emulator serves (which is
        // served over TLS alone), a bucket's own virtual-hosted endpoint, a per-resource
        // listener of this emulator (which negotiates its encryption in band), and a host
        // nothing here serves.
        let cases: [(&str, &str, bool); 9] = [
            (
                "an api stage over TLS",
                "https://abcdef1234.execute-api.us-east-1.amazonaws.com/live/echo",
                true,
            ),
            (
                "an api stage over plaintext",
                "http://abcdef1234.execute-api.us-east-1.amazonaws.com/live/echo",
                false,
            ),
            (
                "a bucket endpoint over TLS",
                "https://shelf.s3.us-east-1.amazonaws.com/key",
                true,
            ),
            (
                "a bucket endpoint over plaintext",
                "http://shelf.s3.us-east-1.amazonaws.com/key",
                false,
            ),
            (
                "a per-resource listener over plaintext",
                "http://127.0.0.1:38211/index.html",
                true,
            ),
            (
                "a per-resource listener over TLS",
                "https://127.0.0.1:38211/index.html",
                true,
            ),
            (
                "a host nothing here serves, over plaintext",
                "http://origin.example.com/index.html",
                true,
            ),
            (
                "a distribution's own edge domain, over plaintext",
                "http://d111111abcdef8.cloudfront.net/index.html",
                true,
            ),
            ("an origin-form URI", "/index.html", true),
        ];
        for (case, uri, expected) in cases {
            let actual = origin_accepts_scheme(uri);
            assert_eq!(
                actual, expected,
                "case {case:?} ({uri:?}): expected origin_accepts_scheme == {expected}, got \
                 {actual}"
            );
        }
    }

    #[test]
    fn an_origin_request_carries_no_viewer_credential_whatever_the_policy_forwards() {
        // The rows differ along the origin request policy's header behaviour — none (the
        // default), `allViewer` and `allViewerExceptHostHeader`, the AWS-managed policies
        // whose ids `MANAGED_ORIGIN_REQUEST_POLICIES` publishes — and along the viewer
        // header: a credential, a hop-by-hop header, and an ordinary one.
        const ALL_VIEWER: &str = "216adef6-5c7f-47e4-b989-5492eafa07d3";
        const ALL_VIEWER_EXCEPT_HOST: &str = "b689b0a8-53d0-40ab-baf2-68738e2966ac";
        let store = ComAmazonawsCloudfrontCloudfront20200531State::default();
        let origin = Origin {
            id: "primary".to_string(),
            domain_name: "origin.example.com".to_string(),
            ..Origin::default()
        };
        // (case, the policy the behaviour names, the viewer header, forwarded)
        let cases: [(&str, Option<&str>, &str, bool); 9] = [
            ("no policy, a credential", None, "authorization", false),
            ("no policy, an ordinary header", None, "accept-language", false),
            (
                "allViewer, a credential",
                Some(ALL_VIEWER),
                "authorization",
                false,
            ),
            (
                "allViewer, a session token",
                Some(ALL_VIEWER),
                "x-amz-security-token",
                false,
            ),
            (
                "allViewer, a hop-by-hop header",
                Some(ALL_VIEWER),
                "connection",
                false,
            ),
            (
                "allViewer, an ordinary header",
                Some(ALL_VIEWER),
                "accept-language",
                true,
            ),
            (
                "allViewerExceptHostHeader, a credential",
                Some(ALL_VIEWER_EXCEPT_HOST),
                "authorization",
                false,
            ),
            (
                "allViewerExceptHostHeader, an ordinary header",
                Some(ALL_VIEWER_EXCEPT_HOST),
                "accept-language",
                true,
            ),
            (
                "a policy this account does not hold",
                Some("00000000-0000-0000-0000-000000000000"),
                "accept-language",
                false,
            ),
        ];
        for (case, policy, header, forwarded) in cases {
            let behavior = Behavior {
                path_pattern: None,
                realtime_log_config_arn: None,
                target_origin_id: "primary".to_string(),
                viewer_protocol_policy: "allow-all".to_string(),
                allowed_methods: None,
                trusted_key_groups: None,
                cache_policy_id: None,
                origin_request_policy_id: policy.map(str::to_string),
                compress: false,
                default_ttl: None,
                max_ttl: None,
                forwarded_values: None,
                response_headers_policy_id: None,
            };
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri("/index.html").expect("a valid URI");
            req.headers_mut().insert(header.to_string(), "v".to_string());
            let headers = origin_request_headers(&store, &behavior, &origin, &req, "cfid");
            let carried = headers.iter().any(|(name, _)| name == header);
            assert_eq!(
                carried, forwarded,
                "case {case:?}: expected the origin request to carry {header:?} == \
                 {forwarded}, got {carried} (headers {headers:?})"
            );
            // Whatever the policy says, the origin request always names the ORIGIN's host
            // and the request it is being made for.
            assert!(
                headers
                    .iter()
                    .any(|(name, value)| name == "host" && value == &origin.domain_name)
                    && headers.iter().any(|(name, _)| name == X_AMZ_CF_ID),
                "case {case:?}: expected the origin's host and an X-Amz-Cf-Id, got \
                 {headers:?}"
            );
        }
    }

    #[test]
    fn an_alias_may_not_claim_an_address_this_emulator_is_reached_at() {
        // Rows differ along the shape of the stored alternate domain name: a customer
        // domain, the loopback names and IP literals a client on this machine dials, a
        // host that names a service endpoint, and the empty name.
        let cases: [(&str, &str, bool); 11] = [
            ("a customer domain", "cdn.example.com", true),
            ("a wildcard-free subdomain", "assets.cdn.example.com", true),
            ("the loopback name", "localhost", false),
            ("the loopback name with a port", "localhost:9297", false),
            ("a name under the loopback name", "edge.localhost", false),
            ("an IPv4 literal", "127.0.0.1", false),
            ("an IPv4 literal with a port", "127.0.0.1:9297", false),
            ("a bracketed IPv6 literal", "[::1]:9297", false),
            ("another service's endpoint", "s3.us-east-1.amazonaws.com", false),
            ("an edge domain CloudFront assigns", "d111.cloudfront.net", false),
            ("the empty name", "", false),
        ];
        for (case, alias, expected) in cases {
            let actual = addressable_alias(alias);
            assert_eq!(
                actual, expected,
                "case {case:?} ({alias:?}): expected addressable_alias == {expected}, got \
                 {actual}"
            );
        }
    }

    /// The policies one behaviour names, held in a store so the shaping below reads them
    /// exactly as a served request does: a cache policy that keys on one cookie and
    /// normalizes the content coding, and an origin request policy that forwards one other
    /// cookie, one viewer header and one header the EDGE generates.
    fn store_with_policies() -> ComAmazonawsCloudfrontCloudfront20200531State {
        use crate::ops::types::{
            CachePolicyConfig, CachePolicyCookiesConfig, CachePolicyHeadersConfig,
            CachePolicyQueryStringsConfig, CookieNames, Headers,
            OriginRequestPolicyConfig, OriginRequestPolicyCookiesConfig,
            OriginRequestPolicyHeadersConfig, OriginRequestPolicyQueryStringsConfig,
            ParametersInCacheKeyAndForwardedToOrigin, QueryStringNames,
        };
        let name_list = |names: &[&str]| {
            Some((
                i32::try_from(names.len()).unwrap_or_default(),
                Some(names.iter().map(|name| (*name).to_string()).collect()),
            ))
        };
        let mut store = ComAmazonawsCloudfrontCloudfront20200531State::default();
        let cookies = name_list(&["sim_key"]).expect("the list is not empty");
        store.cache_policies.insert(
            CACHE_POLICY.to_string(),
            crate::state::StoredCachePolicy {
                id: CACHE_POLICY.to_string(),
                config: CachePolicyConfig {
                    name: "keys-on-one-cookie".to_string(),
                    min_ttl: 0,
                    default_ttl: Some(60),
                    max_ttl: Some(60),
                    comment: None,
                    parameters_in_cache_key_and_forwarded_to_origin: Some(
                        ParametersInCacheKeyAndForwardedToOrigin {
                            enable_accept_encoding_gzip: true,
                            enable_accept_encoding_brotli: Some(true),
                            headers_config: CachePolicyHeadersConfig {
                                header_behavior: "none".to_string(),
                                headers: None,
                            },
                            cookies_config: CachePolicyCookiesConfig {
                                cookie_behavior: "whitelist".to_string(),
                                cookies: Some(CookieNames {
                                    quantity: cookies.0,
                                    items: cookies.1,
                                }),
                            },
                            query_strings_config: CachePolicyQueryStringsConfig {
                                query_string_behavior: "none".to_string(),
                                query_strings: None,
                            },
                        },
                    ),
                },
                last_modified_time: SystemTime::UNIX_EPOCH,
                etag: "E1".to_string(),
            },
        );
        let headers = name_list(&["x-fwd", "CloudFront-Viewer-Country"])
            .expect("the list is not empty");
        let forwarded_cookies = name_list(&["sim_fwd"]).expect("the list is not empty");
        let forwarded_queries = name_list(&["keep"]).expect("the list is not empty");
        store.origin_request_policies.insert(
            ORIGIN_REQUEST_POLICY.to_string(),
            crate::state::StoredOriginRequestPolicy {
                id: ORIGIN_REQUEST_POLICY.to_string(),
                config: OriginRequestPolicyConfig {
                    name: "forwards-what-the-key-excludes".to_string(),
                    comment: None,
                    headers_config: OriginRequestPolicyHeadersConfig {
                        header_behavior: "whitelist".to_string(),
                        headers: Some(Headers {
                            quantity: headers.0,
                            items: headers.1,
                        }),
                    },
                    cookies_config: OriginRequestPolicyCookiesConfig {
                        cookie_behavior: "whitelist".to_string(),
                        cookies: Some(CookieNames {
                            quantity: forwarded_cookies.0,
                            items: forwarded_cookies.1,
                        }),
                    },
                    query_strings_config: OriginRequestPolicyQueryStringsConfig {
                        query_string_behavior: "whitelist".to_string(),
                        query_strings: Some(QueryStringNames {
                            quantity: forwarded_queries.0,
                            items: forwarded_queries.1,
                        }),
                    },
                },
                last_modified_time: SystemTime::UNIX_EPOCH,
                etag: "E2".to_string(),
            },
        );
        store
    }

    /// The id the cache policy of [`store_with_policies`] is held under.
    const CACHE_POLICY: &str = "11111111-1111-1111-1111-111111111111";

    /// The id the origin request policy of [`store_with_policies`] is held under.
    const ORIGIN_REQUEST_POLICY: &str = "22222222-2222-2222-2222-222222222222";

    /// RULE: the origin request carries the UNION of what the cache key names and what the
    /// origin request policy forwards, and nothing else — and a header CloudFront generates
    /// itself carries the EDGE's value, never a value the viewer stated for it.
    ///
    /// The rows differ along what the viewer sent and what is asked about: a cookie the cache
    /// key names, a cookie only the origin request policy names, a cookie neither names, a
    /// header the policy names, a header it does not, a header the edge generates (both for a
    /// viewer that stated nothing and for one that stated a value of its own), and the
    /// content coding the cache policy normalizes.
    #[test]
    fn the_origin_request_carries_what_the_policies_name_and_the_edge_generated_values() {
        let store = store_with_policies();
        let origin = Origin {
            id: "primary".to_string(),
            domain_name: "origin.example.com".to_string(),
            ..Origin::default()
        };
        let behavior = behavior_under_test();
        // What one viewer sent: two cookies the policies name and one neither does, a header
        // the policy names and one it does not, a value spoofed for a header the edge
        // generates, and a content coding it accepts.
        let mut req = RtRequest::new(SdkBody::empty());
        req.set_uri("/obj?keep=1&drop=2").expect("a valid URI");
        for (name, value) in [
            (COOKIE, "sim_fwd=c1; sim_unl=leak; sim_key=k1"),
            ("x-fwd", "forwarded"),
            ("x-unlisted", "withheld"),
            (generated_headers::VIEWER_COUNTRY, "ZZ"),
            (ACCEPT_ENCODING, "gzip, deflate, br"),
        ] {
            req.headers_mut().insert(name.to_string(), value.to_string());
        }
        let headers = origin_request_headers(&store, &behavior, &origin, &req, "cfid");
        let carried = |name: &str| {
            headers
                .iter()
                .find(|(carried, _)| carried == name)
                .map(|(_, value)| value.clone())
        };

        // (case, the header the origin request carries, the value it must carry)
        let cases: [(&str, &str, Option<&str>); 7] = [
            (
                "the cookies the two policies name, in name order, and no other",
                COOKIE,
                Some("sim_fwd=c1; sim_key=k1"),
            ),
            (
                "a viewer header the origin request policy names",
                "x-fwd",
                Some("forwarded"),
            ),
            (
                "a viewer header neither policy names",
                "x-unlisted",
                None,
            ),
            (
                "the country the EDGE resolved, not the one the viewer stated",
                generated_headers::VIEWER_COUNTRY,
                Some("US"),
            ),
            (
                "a generated header the policy does not name",
                generated_headers::FORWARDED_PROTO,
                None,
            ),
            (
                "the normalized content coding, which takes precedence for brotli",
                ACCEPT_ENCODING,
                Some("br"),
            ),
            ("the origin's own host", "host", Some("origin.example.com")),
        ];
        for (case, header, expected) in cases {
            let actual = carried(header);
            assert_eq!(
                actual.as_deref(),
                expected,
                "case {case:?}: expected the origin request to carry {header:?} == \
                 {expected:?}, got {actual:?} (headers {headers:?})"
            );
        }
    }

    #[test]
    fn the_origin_is_asked_for_the_query_strings_the_policies_name() {
        let store = store_with_policies();
        let keyed = forwarding::cache_key_parameters(&store, &behavior_under_test());
        let forwarded = forwarding::forwarded_parameters(&store, &behavior_under_test());
        // Rows differ along the viewer's query: a named parameter, an unnamed one, both, the
        // order they arrived in, and no query at all.
        let cases: [(&str, &str, &str); 5] = [
            ("a named parameter alone", "keep=1", "keep=1"),
            ("an unnamed parameter is withheld", "drop=2", ""),
            (
                "the named one travels and the unnamed one does not",
                "drop=2&keep=1",
                "keep=1",
            ),
            (
                "the viewer's order is kept",
                "keep=1&drop=2&keep=3",
                "keep=1&keep=3",
            ),
            ("no query at all", "", ""),
        ];
        for (case, query, expected) in cases {
            let actual = origin_query(&keyed, &forwarded, query);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected the origin to be asked for {expected:?}, got \
                 {actual:?}"
            );
        }
    }

    /// The behaviour the two shaping tests above drive: it names both policies of
    /// [`store_with_policies`] and asks for automatic compression.
    fn behavior_under_test() -> Behavior {
        Behavior {
            path_pattern: None,
            realtime_log_config_arn: None,
            target_origin_id: "primary".to_string(),
            viewer_protocol_policy: "allow-all".to_string(),
            allowed_methods: None,
            trusted_key_groups: None,
            cache_policy_id: Some(CACHE_POLICY.to_string()),
            origin_request_policy_id: Some(ORIGIN_REQUEST_POLICY.to_string()),
            compress: true,
            default_ttl: None,
            max_ttl: None,
            forwarded_values: None,
            response_headers_policy_id: None,
        }
    }

    #[test]
    fn a_request_that_has_already_been_through_an_edge_is_recognised() {
        // Rows differ along the `Via` the request carries: one an edge of this emulator
        // stamped, one another proxy did, a chain ending in one, and none at all.
        let cases: [(&str, Option<&str>, bool); 4] = [
            ("no Via at all", None, false),
            (
                "a Via an edge stamped",
                Some("1.1 cfid.cloudfront.net (CloudFront)"),
                true,
            ),
            ("another proxy's Via", Some("1.1 proxy.internal"), false),
            (
                "a chain whose last hop is an edge",
                Some("1.1 proxy.internal, 1.1 cfid.cloudfront.net (CloudFront)"),
                true,
            ),
        ];
        for (case, via, expected) in cases {
            let mut req = RtRequest::new(SdkBody::empty());
            req.set_uri("/index.html").expect("a valid URI");
            if let Some(via) = via {
                req.headers_mut().insert("via".to_string(), via.to_string());
            }
            let actual = carries_cloudfront_via(&req);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected carries_cloudfront_via == {expected}, got {actual}"
            );
        }
    }
}
