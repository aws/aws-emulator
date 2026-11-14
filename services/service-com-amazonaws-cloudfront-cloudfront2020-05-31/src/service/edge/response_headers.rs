// SPDX-License-Identifier: CC0-1.0

//! The RESPONSE HEADERS POLICY the matched cache behaviour attaches: the headers CloudFront
//! puts on the answer it serves a viewer, and the ones it takes off.
//!
//! A cache behaviour names a policy in its `ResponseHeadersPolicyId`, and the policy's
//! configuration is a set of header groups CloudFront applies to the response it sends:
//! "CloudFront adds these headers to HTTP responses that it sends for requests that match a
//! cache behavior that's associated with this response headers policy"
//! (`ResponseHeadersPolicyCustomHeadersConfig`, and the same sentence on the CORS and the
//! security group), and `RemoveHeadersConfig` is "a list of HTTP header names that CloudFront
//! removes from HTTP responses to requests that match the cache behavior that this response
//! headers policy is attached to".
//!
//! So the groups are applied HERE, to whatever the edge serves — the origin's answer, the
//! same answer out of the edge cache, or a page CloudFront generated itself — and never to
//! what is cached: the CORS group's value depends on the VIEWER's `Origin`, so it is computed
//! per request rather than held with the bytes.
//!
//! Each group states whether it wins against the origin:
//!
//! * the CORS group's `OriginOverride` — "whether CloudFront overrides HTTP response headers
//!   received from the origin with the ones specified in this response headers policy";
//! * each security header's and each custom header's own `Override`, which is that same
//!   sentence per header.
//!
//! A header the configuration does not override is left as the origin sent it when the origin
//! sent one, and added when it did not.
//!
//! The CORS group applies to a CORS request — one carrying an `Origin` — and only when
//! `AccessControlAllowOrigins` grants that origin: the list is "a list of origins (domain
//! names) that CloudFront can use as the value for the `Access-Control-Allow-Origin` HTTP
//! response header", so an origin it does not name is answered without the group, which is
//! what a browser reads as a refusal. WHICH of the group's headers the answer carries is the
//! headers' own definitions: the allowed methods, the allowed headers and the max age answer
//! the question a PREFLIGHT asks and appear on a preflight's answer alone, while the exposed
//! headers answer what an ACTUAL request's answer may be read for and appear on that one
//! ([`ViewerCors`]).
//!
//! `ServerTimingHeadersConfig` is the one group this module does not write: its header
//! carries CloudFront's own CDN timing metrics, which are not a value this emulator can
//! state for work it did not do.

use ::aws_smithy_runtime_api::http::Response as RtResponse;

use crate::ops::types::{
    ResponseHeadersPolicyConfig, ResponseHeadersPolicyCorsConfig,
    ResponseHeadersPolicySecurityHeadersConfig,
    RESPONSE_HEADERS_POLICY_ACCESS_CONTROL_ALLOW_METHODS_VALUES_VALUES,
};

/// The `AccessControlAllowOrigins` (and `AccessControlAllowHeaders`) entry that stands for
/// every origin (or header) rather than naming one: the value CloudFront answers verbatim.
pub const ANY: &str = "*";

/// The `AccessControlAllowMethods` entry that stands for every method the member's value set
/// names, rather than naming one — expanded by [`allowed_methods`].
const ALL_METHODS: &str = "ALL";

/// The request header that makes a request a CORS request, and whose value the CORS group is
/// decided against.
const ORIGIN: &str = "origin";

/// The request header a browser asks a PREFLIGHT with: "the `Access-Control-Request-Method`
/// request header is used by browsers when issuing a preflight request, to let the server
/// know which HTTP method will be used when the actual request is made" (MDN). Its presence
/// on an `OPTIONS` request is what makes that request a preflight rather than an ordinary
/// one.
const REQUEST_METHOD: &str = "access-control-request-method";

/// The method a CORS preflight is sent with.
const PREFLIGHT_METHOD: &str = "OPTIONS";

/// The reserved name the viewer's `Origin` reaches this data plane under when the viewer
/// addressed a distribution's OWN address rather than this emulator's front door.
///
/// That address answers a browser directly, so it is where the browser-set headers are READ
/// and the request is admitted or refused ([`crate::state::edge_endpoint`]); the leg from
/// there into this data plane is this emulator calling itself, and a leg carrying a browser's
/// `Origin` is a leg the emulator's own front door refuses as a cross-site browser request —
/// for a viewer request that address already admitted. So the value travels under a name of
/// this emulator's own, which no client sets and no origin request carries (it is in the
/// edge's own `HEADERS_NEVER_FORWARDED`), and the CORS group is decided from it exactly as
/// from the viewer's own header ([`ViewerCors::of_request`]).
pub const VIEWER_ORIGIN: &str = "x-emulator-cloudfront-viewer-origin";

/// The response headers the CORS group writes.
const ALLOW_ORIGIN: &str = "access-control-allow-origin";
const ALLOW_HEADERS: &str = "access-control-allow-headers";
const ALLOW_METHODS: &str = "access-control-allow-methods";
const ALLOW_CREDENTIALS: &str = "access-control-allow-credentials";
const EXPOSE_HEADERS: &str = "access-control-expose-headers";
const MAX_AGE: &str = "access-control-max-age";

/// The ONE value `Access-Control-Allow-Credentials` carries: "the only valid value for this
/// header is `true`" (MDN, which the member's own documentation links as the header's
/// meaning), so the header is written for a configuration that asks for credentials and left
/// off for one that does not.
const CREDENTIALS_ALLOWED: &str = "true";

/// The response headers the security group writes.
const XSS_PROTECTION: &str = "x-xss-protection";
const FRAME_OPTIONS: &str = "x-frame-options";
const REFERRER_POLICY: &str = "referrer-policy";
const CONTENT_SECURITY_POLICY: &str = "content-security-policy";
const CONTENT_TYPE_OPTIONS: &str = "x-content-type-options";
const STRICT_TRANSPORT_SECURITY: &str = "strict-transport-security";

/// The ONE value `X-Content-Type-Options` carries: the member "determines whether CloudFront
/// includes the `X-Content-Type-Options` HTTP response header with its value set to
/// `nosniff`".
const NOSNIFF: &str = "nosniff";

/// What the CORS group of a policy is decided from: the viewer's `Origin`, and whether the
/// request is the PREFLIGHT a browser sends ahead of the real one.
///
/// The two carry different halves of the group, as the headers themselves are defined: the
/// methods, the headers and the max age answer the question a PREFLIGHT asks ("used by
/// browsers when issuing a preflight request"), and the exposed headers answer the one an
/// ACTUAL request's answer is about, so neither set appears on the other's response.
#[derive(Debug, Clone, Copy)]
pub struct ViewerCors<'r> {
    /// The `Origin` the viewer presented, or `None` for a request that presented none —
    /// which is no CORS request at all.
    origin: Option<&'r str>,
    /// Whether this is a preflight: an `OPTIONS` request carrying
    /// `Access-Control-Request-Method`.
    preflight: bool,
}

impl<'r> ViewerCors<'r> {
    /// What `req` asks of the CORS group.
    pub fn of_request(req: &'r ::aws_smithy_runtime_api::http::Request) -> Self {
        let header = |name: &str| req.headers().get(name).filter(|value| !value.is_empty());
        Self {
            // The reserved name a distribution's own address forwards the viewer's `Origin`
            // under ([`VIEWER_ORIGIN`]) ahead of the viewer's own header, which is what a
            // request that reached this emulator's front door directly carries.
            origin: header(VIEWER_ORIGIN).or_else(|| header(ORIGIN)),
            preflight: req.method().eq_ignore_ascii_case(PREFLIGHT_METHOD)
                && header(REQUEST_METHOD).is_some(),
        }
    }
}

/// Apply `policy` to the answer the edge serves one viewer request, which asked `viewer` of
/// the CORS group.
///
/// The removals are applied first and the groups then write their own headers, so a
/// configuration naming the same header in both reports the policy's value rather than
/// nothing — the removal is of what the ORIGIN sent.
pub fn apply(
    policy: &ResponseHeadersPolicyConfig,
    viewer: ViewerCors<'_>,
    response: &mut RtResponse,
) {
    remove(policy, response);
    if let Some(cors) = policy.cors_config.as_ref() {
        apply_cors(cors, viewer, response);
    }
    if let Some(security) = policy.security_headers_config.as_ref() {
        apply_security(security, response);
    }
    for header in policy
        .custom_headers_config
        .as_ref()
        .and_then(|custom| custom.items.as_ref())
        .into_iter()
        .flatten()
    {
        put(
            response,
            &header.header,
            header.value.clone(),
            header.r#override,
        );
    }
}

/// The headers `RemoveHeadersConfig` takes off the response.
fn remove(policy: &ResponseHeadersPolicyConfig, response: &mut RtResponse) {
    for header in policy
        .remove_headers_config
        .as_ref()
        .and_then(|removed| removed.items.as_ref())
        .into_iter()
        .flatten()
    {
        response.headers_mut().remove(header.header.as_str());
    }
}

/// The CORS group, for a request that carried `request_origin`.
///
/// A request carrying no `Origin` is no CORS request and gets none of these headers, and an
/// origin the configuration does not grant ([`grants`]) gets none either.
fn apply_cors(
    cors: &ResponseHeadersPolicyCorsConfig,
    viewer: ViewerCors<'_>,
    response: &mut RtResponse,
) {
    let Some(origin) = viewer.origin.filter(|origin| grants(cors, origin)) else {
        return;
    };
    let overridden = cors.origin_override;
    // "A list of origins (domain names) that CloudFront can use as the value for the
    // `Access-Control-Allow-Origin` HTTP response header": the wildcard entry is the value
    // itself, and a list that NAMES origins answers the one this viewer presented — the
    // granted origin, never the whole list, which the header cannot carry.
    let allowed = if names_any(&cors.access_control_allow_origins.items) {
        ANY.to_string()
    } else {
        origin.to_string()
    };
    put(response, ALLOW_ORIGIN, allowed, overridden);
    // The permission a PREFLIGHT asked about: which methods and which headers the real
    // request may use, and how long the browser may remember the answer.
    if viewer.preflight {
        put(
            response,
            ALLOW_HEADERS,
            joined(&cors.access_control_allow_headers.items),
            overridden,
        );
        put(
            response,
            ALLOW_METHODS,
            allowed_methods(&cors.access_control_allow_methods.items),
            overridden,
        );
        if let Some(max_age) = cors.access_control_max_age_sec {
            put(response, MAX_AGE, max_age.to_string(), overridden);
        }
    } else if let Some(exposed) = cors
        .access_control_expose_headers
        .as_ref()
        .and_then(|exposed| exposed.items.as_ref())
    {
        // And the one an ACTUAL request's answer is about: which of its headers the page
        // that made it may read.
        put(response, EXPOSE_HEADERS, joined(exposed), overridden);
    }
    // Whether the credentials may be sent at all is asked of both.
    if cors.access_control_allow_credentials {
        put(
            response,
            ALLOW_CREDENTIALS,
            CREDENTIALS_ALLOWED.to_string(),
            overridden,
        );
    }
}

/// Whether `cors` grants `origin` the CORS group at all: the wildcard entry grants every
/// origin, and otherwise the list has to NAME this one.
///
/// An `Origin` is compared as the viewer sent it, the scheme and host of a serialized origin
/// being case-insensitive while nothing else about the value is.
fn grants(cors: &ResponseHeadersPolicyCorsConfig, origin: &str) -> bool {
    let items = &cors.access_control_allow_origins.items;
    names_any(items) || items.iter().any(|named| named.eq_ignore_ascii_case(origin))
}

/// The origins `cors` NAMES, which is what the edge admits a browser request from
/// ([`crate::state::edge_endpoint`]): the wildcard entry names none.
pub fn named_origins(policy: &ResponseHeadersPolicyConfig) -> impl Iterator<Item = &str> {
    policy
        .cors_config
        .as_ref()
        .map(|cors| cors.access_control_allow_origins.items.as_slice())
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .filter(|origin| *origin != ANY)
}

/// Whether a list carries the entry that stands for every value rather than naming one.
fn names_any(items: &[String]) -> bool {
    items.iter().any(|item| item == ANY)
}

/// A header value listing `items`, as a header carrying a list of tokens does.
fn joined(items: &[String]) -> String {
    items.join(", ")
}

/// `Access-Control-Allow-Methods` for `items`: the methods as listed, with the entry that
/// stands for every method expanded into the ones the member's own value set names.
///
/// The expansion is read off that value set
/// (`RESPONSE_HEADERS_POLICY_ACCESS_CONTROL_ALLOW_METHODS_VALUES_VALUES`, the model's own
/// list for `ResponseHeadersPolicyAccessControlAllowMethodsValues`), so it cannot drift from
/// the methods the member accepts.
fn allowed_methods(items: &[String]) -> String {
    if items.iter().any(|method| method == ALL_METHODS) {
        return RESPONSE_HEADERS_POLICY_ACCESS_CONTROL_ALLOW_METHODS_VALUES_VALUES
            .iter()
            .filter(|method| **method != ALL_METHODS)
            .copied()
            .collect::<Vec<_>>()
            .join(", ");
    }
    joined(items)
}

/// The security group: one header per member the configuration sets, each under its own
/// `Override`.
fn apply_security(
    security: &ResponseHeadersPolicySecurityHeadersConfig,
    response: &mut RtResponse,
) {
    if let Some(xss) = security.xss_protection.as_ref() {
        // "1" enables the browser's filter and "0" turns it off; `ModeBlock` and `ReportUri`
        // are the directives that follow it.
        let mut value = String::from(if xss.protection { "1" } else { "0" });
        if xss.mode_block.unwrap_or(false) {
            value.push_str("; mode=block");
        }
        if let Some(report) = xss.report_uri.as_deref().filter(|uri| !uri.is_empty()) {
            value.push_str("; report=");
            value.push_str(report);
        }
        put(response, XSS_PROTECTION, value, xss.r#override);
    }
    if let Some(frame) = security.frame_options.as_ref() {
        put(
            response,
            FRAME_OPTIONS,
            frame.frame_option.clone(),
            frame.r#override,
        );
    }
    if let Some(referrer) = security.referrer_policy.as_ref() {
        put(
            response,
            REFERRER_POLICY,
            referrer.referrer_policy.clone(),
            referrer.r#override,
        );
    }
    if let Some(csp) = security.content_security_policy.as_ref() {
        put(
            response,
            CONTENT_SECURITY_POLICY,
            csp.content_security_policy.clone(),
            csp.r#override,
        );
    }
    if let Some(content_type) = security.content_type_options.as_ref() {
        put(
            response,
            CONTENT_TYPE_OPTIONS,
            NOSNIFF.to_string(),
            content_type.r#override,
        );
    }
    if let Some(hsts) = security.strict_transport_security.as_ref() {
        // "max-age=<seconds>", then the two directives the member carries, in the order the
        // header declares them.
        let mut value = format!("max-age={}", hsts.access_control_max_age_sec);
        if hsts.include_subdomains.unwrap_or(false) {
            value.push_str("; includeSubDomains");
        }
        if hsts.preload.unwrap_or(false) {
            value.push_str("; preload");
        }
        put(
            response,
            STRICT_TRANSPORT_SECURITY,
            value,
            hsts.r#override,
        );
    }
}

/// Write one header of the policy's, unless the response already carries that header and the
/// configuration does not override it — "whether CloudFront overrides HTTP response headers
/// received from the origin with the ones specified in this response headers policy".
///
/// A name or a value no header can carry is left off rather than taking the answer down: the
/// configuration is a caller's, and a viewer gets the answer either way.
fn put(response: &mut RtResponse, name: &str, value: String, overridden: bool) {
    let name = name.to_ascii_lowercase();
    if !overridden && response.headers().get(&name).is_some() {
        return;
    }
    let _ = response.headers_mut().try_insert(name, value);
}
