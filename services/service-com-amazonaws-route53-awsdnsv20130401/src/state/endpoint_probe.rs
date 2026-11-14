// SPDX-License-Identifier: CC0-1.0

//! The PROBE a health checker makes of an endpoint-type health check's endpoint, and the
//! description it reports what it observed as.
//!
//! `HealthCheckConfig$Type` documents what each endpoint type's probe DOES, and this module
//! does it:
//!
//! * `TCP` — "Route 53 tries to establish a TCP connection";
//! * `HTTP` / `HTTPS` — "tries to establish a TCP connection. If successful, Route 53 submits
//!   an HTTP request and waits for an HTTP status code of 200 or greater and less than 400";
//! * `HTTP_STR_MATCH` / `HTTPS_STR_MATCH` — the same request, and then it "searches the first
//!   5,120 bytes of the response body for the string that you specify in `SearchString`"
//!   ([`MAX_SEARCHED_BODY_BYTES`]). "If the string appears in the response body, Route 53
//!   considers the resource healthy", and "Route 53 considers case when searching for
//!   SearchString in the response body" — so the test is a case-sensitive substring test, and
//!   it is applied to the body of THIS probe, which is what makes the reported status follow a
//!   body that changes under a health check nobody touched.
//!
//! Where the request goes is read off the same configuration ([`probe_target`]): the scheme
//! from the type, the endpoint from `IPAddress` ("Route 53 sends health check requests to the
//! specified IPv4 or IPv6 address") or else from `FullyQualifiedDomainName` ("If you don't
//! specify a value for `IPAddress`, Route 53 sends a DNS request to resolve the domain name
//! that you specify in `FullyQualifiedDomainName`"), the `Host` header from
//! `FullyQualifiedDomainName` — with `:Port` appended when the port is not the one the scheme
//! is reached on, which is the rule that member spells out — and the path from `ResourcePath`.
//!
//! The probe is made over the emulator's own IN-PROCESS transport
//! (`OperationContext::callback`, the handle the health-check writes hand this plane) and
//! carries NO credential, which is what a health checker sends: no socket is opened, no name
//! is resolved and nothing leaves this process, so what a checker can observe is exactly what
//! a service of this emulator serves to everyone. An endpoint nothing here serves is one no
//! checker of this emulator can connect to, and it is reported as the transport failure it is
//! ([`STATUS_CONNECTION_FAILED`]) rather than as a success nothing observed. The one endpoint
//! a checker does not dial is the API that HOLDS the health check ([`addresses_this_service`]):
//! a checker is a client outside the service it reports to, so a probe is never a call back
//! onto this plane, and one round of probing is bounded as a whole in the time it may take
//! ([`super::health_check_status`]).

use ::std::time::Duration;

use ::aws_smithy_types::body::SdkBody;
use ::smithy_server_runtime::EmbeddedEmulator;

use super::{
    health_check_member_given, ENDPOINT_HEALTH_CHECK_TYPES, HEALTH_CHECK_TYPE_TCP,
    STATUS_FAILURE_PREFIX, STATUS_SUCCESS_PREFIX, STATUS_TCP_CONNECTION_ESTABLISHED,
    STRING_MATCH_HEALTH_CHECK_TYPES, TLS_HEALTH_CHECK_TYPES,
};
use crate::ops::types::HealthCheckConfig;

/// How much of the response body a string-match probe searches.
///
/// `HealthCheckType`'s own documentation for `HTTP_STR_MATCH` and `HTTPS_STR_MATCH`: Route 53
/// "searches the first 5,120 bytes of the response body for the string that you specify in
/// SearchString", so a marker further into the body than that is not found.
pub const MAX_SEARCHED_BODY_BYTES: usize = 5_120;

/// What a string-match probe reports when it reached the endpoint and the endpoint's answer
/// did NOT carry `SearchString`.
///
/// `StatusReport$Status` is "a description of the status of the health check endpoint as
/// reported by one of the Amazon Route 53 health checkers", free text the model constrains in
/// no way, and this is the description the service reports for a failure of the string match
/// itself — a different fact about the endpoint from a connection that could not be made or an
/// answer the request types reject, each of which has its own description below. The wording
/// is the one MEASURED against the service for this failure class (the recorded replay this
/// plane was written from, us-west-2): the whole of it is fixed text, with nothing in it taken
/// from the request.
pub const STATUS_STRING_MATCH_FAILURE: &str = "Failure: STRING_MATCH_FAILURE";

/// What a probe reports when it could not reach the endpoint at all: nothing in this emulator
/// serves the address the configuration names — or the address is this plane's own API, which
/// its checkers do not dial ([`addresses_this_service`]) — so the checker got no answer out of
/// it.
///
/// That is the fact a checker reports when its request is not answered, and the description
/// the service words such a report as. The service distinguishes finer causes of "no answer"
/// (a name that resolves to nothing, a connection refused, a handshake that failed); none of
/// them is derivable here, because the in-process transport reports only that no service
/// claimed the request — so every unanswered probe is reported as the one class this module
/// can tell, and the classes it CAN tell apart (the string match, the answered status) have
/// their own descriptions.
pub const STATUS_CONNECTION_FAILED: &str = "Failure: Connection timed out";

/// The endpoint prefix of the API this plane belongs to: a host whose first label is this is
/// Route 53's own endpoint (`route53.amazonaws.com`), which is the one address a checker of
/// this emulator does not dial.
const OWN_ENDPOINT_PREFIX: &str = "route53";

/// The scheme a probe of a type that negotiates TLS ([`TLS_HEALTH_CHECK_TYPES`]) is sent over,
/// and the one every other endpoint type's is.
const SECURE_SCHEME: &str = "https";
/// See [`SECURE_SCHEME`].
const PLAINTEXT_SCHEME: &str = "http";

/// The port a probe is sent to when the configuration names none: the port the scheme the
/// type's request is made over is served on.
const HTTPS_PORT: i32 = 443;
/// See [`HTTPS_PORT`].
const HTTP_PORT: i32 = 80;

/// The path a probe requests when `ResourcePath` names none — the root of the endpoint, which
/// is what a request carrying no path addresses.
const DEFAULT_RESOURCE_PATH: &str = "/";

/// How long one probe waits for the endpoint's answer before reporting that it did not reach
/// it.
///
/// The emulator's own bound, not an AWS span: it bounds the work one probe round can hold, so
/// a checker clock cannot be parked on an in-process call that never completes.
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The lowest and the first excluded HTTP status of an answer the request types accept:
/// `HealthCheckType` — Route 53 "waits for an HTTP status code of 200 or greater and less than
/// 400".
const LOWEST_HEALTHY_STATUS: u16 = 200;
/// See [`LOWEST_HEALTHY_STATUS`].
const FIRST_UNHEALTHY_STATUS: u16 = 400;

/// What ONE probe of a health check's endpoint observed: the description the checker reports
/// it as, and whether the criterion the check's `Type` sets was met.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// The `StatusReport$Status` every checker of this health check reports for this probe
    /// round.
    pub status: String,
    /// Whether this probe counts as a passing health check, which is what the check's
    /// aggregate assessment (and so its CloudWatch series) is folded from.
    pub healthy: bool,
}

impl ProbeReport {
    /// A probe whose criterion was met.
    fn passed(status: String) -> Self {
        Self {
            status,
            healthy: true,
        }
    }

    /// A probe whose criterion was not met.
    fn failed(status: String) -> Self {
        Self {
            status,
            healthy: false,
        }
    }
}

/// Where a probe of `config` is sent, or `None` for a configuration that names no endpoint to
/// probe (a type that reads an alarm, combines children or follows a routing control).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeTarget {
    /// The absolute request URI: the scheme the type's request is made over, the endpoint the
    /// checker connects to, its port and the path `ResourcePath` names.
    pub uri: String,
    /// The `Host` header the request carries, which is the name the endpoint serves under.
    pub host: String,
}

/// Where a probe of `config` goes, following `HealthCheckConfig`'s own rules for the endpoint,
/// the `Host` header and the path (see the module docs).
pub fn probe_target(config: &HealthCheckConfig) -> Option<ProbeTarget> {
    let kind = config.r#type.as_str();
    if !ENDPOINT_HEALTH_CHECK_TYPES.contains(&kind) {
        return None;
    }
    let secure = TLS_HEALTH_CHECK_TYPES.contains(&kind);
    let scheme = if secure { SECURE_SCHEME } else { PLAINTEXT_SCHEME };
    let scheme_port = if secure { HTTPS_PORT } else { HTTP_PORT };
    let port = config.port.unwrap_or(scheme_port);
    let given = |member: Option<&'_ String>| -> Option<String> {
        member
            .map(|value| value.trim().to_string())
            .filter(|value| health_check_member_given(Some(value)))
            // A fully qualified domain name may be written with the root label's dot
            // (`site.example.com.`), which names the same host; the authority a request
            // carries and the `Host` header it sends write it without.
            .map(|value| value.trim_end_matches('.').to_string())
            .filter(|value| !value.is_empty())
    };
    let address = given(config.ip_address.as_ref());
    let domain = given(config.fully_qualified_domain_name.as_ref());
    // The endpoint the checker connects to is the address when the configuration names one,
    // and otherwise the name it resolves; the `Host` header is the NAME, falling back to the
    // address — "If you don't specify a value for FullyQualifiedDomainName, Route 53
    // substitutes the value of IPAddress in the Host header".
    let endpoint = address.clone().or_else(|| domain.clone())?;
    let named = domain.or(address)?;
    // A literal IPv6 address is bracketed in a URI's authority (RFC 3986 §3.2.2).
    let authority = if endpoint.contains(':') {
        format!("[{endpoint}]")
    } else {
        endpoint
    };
    // "If you specify a value of 80 for Port and HTTP or HTTP_STR_MATCH for Type, Route 53
    // passes the value of FullyQualifiedDomainName to the endpoint in the Host header […] If
    // you specify another value for Port and any value except TCP for Type, Route 53 passes
    // FullyQualifiedDomainName:Port". A `TCP` check sends no request and so no header of its
    // own; the name it connects to is what addresses the endpoint for it.
    let host = if port == scheme_port || kind == HEALTH_CHECK_TYPE_TCP {
        named
    } else {
        format!("{named}:{port}")
    };
    Some(ProbeTarget {
        uri: format!("{scheme}://{authority}:{port}{}", resource_path(config)),
        host,
    })
}

/// The path a probe of `config` requests: `ResourcePath` as it was given — "the path, if any,
/// that you want Amazon Route 53 to request when performing health checks", which "can also
/// include query string parameters" — and the endpoint's root for a configuration that names
/// none. A path that does not open at the root is requested under it, which is where a request
/// can address it at all.
fn resource_path(config: &HealthCheckConfig) -> String {
    let path = config
        .resource_path
        .as_deref()
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .unwrap_or(DEFAULT_RESOURCE_PATH);
    if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    }
}

/// Probe the endpoint `config` names over `transport` and report what the checker observed, or
/// `None` for a configuration with no endpoint to probe.
///
/// Call it with no store guard held: it awaits an answer from another service of this
/// emulator.
pub async fn probe(
    transport: &EmbeddedEmulator,
    config: &HealthCheckConfig,
) -> Option<ProbeReport> {
    let target = probe_target(config)?;
    // The one endpoint a checker of this emulator does not dial: the API that HOLDS the
    // health check. A checker is a client outside the service it reports to, so a probe that
    // came back round onto this plane would be a checker assessing the thing that made it —
    // and the answer it carried would say nothing about any endpoint. Nothing dials it, so
    // nothing answers the checker, which is the report below.
    if addresses_this_service(&target.host) {
        return Some(ProbeReport::failed(STATUS_CONNECTION_FAILED.to_string()));
    }
    Some(match fetch(transport, &target).await {
        // Nothing answered the checker, so there is no connection and no answer to assess —
        // whatever the type would have done with one.
        Fetched::Unreachable => ProbeReport::failed(STATUS_CONNECTION_FAILED.to_string()),
        Fetched::Answered { status, body } => assess(config, status, body.as_deref()),
    })
}

/// Whether `host` addresses THIS service's own API — the plane that holds the health check and
/// makes the probe — read by the front door's own host reader
/// (`service_common::data_plane_addressing::service_named_by_host`), so a probe reads a host
/// exactly as the router routes one.
fn addresses_this_service(host: &str) -> bool {
    ::service_common::data_plane_addressing::service_named_by_host(host)
        .is_some_and(|named| named == OWN_ENDPOINT_PREFIX)
}

/// What the criterion `config`'s `Type` sets makes of an answer: `status` came back from the
/// endpoint, so the connection was established, and `body` is the bytes it carried where the
/// checker could read them (`None` for an answer whose body it could not).
///
/// Separated from the fetch so the rule each type is assessed by is decided by the
/// configuration and the answer alone.
fn assess(config: &HealthCheckConfig, status: u16, body: Option<&[u8]>) -> ProbeReport {
    let kind = config.r#type.as_str();
    // "TCP: Route 53 tries to establish a TCP connection" — and nothing beyond it, so an
    // answer of any status is a connection that was established.
    if kind == HEALTH_CHECK_TYPE_TCP {
        return ProbeReport::passed(STATUS_TCP_CONNECTION_ESTABLISHED.to_string());
    }
    // The request types "wait for an HTTP status code of 200 or greater and less than 400",
    // which the string-match types' own request is subject to as well: a status outside that
    // range is the failure, and the body is not searched.
    if !(LOWEST_HEALTHY_STATUS..FIRST_UNHEALTHY_STATUS).contains(&status) {
        return ProbeReport::failed(http_status_description(STATUS_FAILURE_PREFIX, status));
    }
    // Only the string-match types read the body at all, so an answer the checker could not
    // read a body out of still passes an `HTTP`, `HTTPS` or `TCP` check on its status.
    if STRING_MATCH_HEALTH_CHECK_TYPES.contains(&kind) {
        // The string is searched for in the BYTES the endpoint answered with, case included,
        // so a body that is not text is searched exactly as one that is.
        let needle = config.search_string.as_deref().unwrap_or_default();
        let found = body.is_some_and(|body| {
            holds_search_string(
                &body[..body.len().min(MAX_SEARCHED_BODY_BYTES)],
                needle.as_bytes(),
            )
        });
        if !found {
            return ProbeReport::failed(STATUS_STRING_MATCH_FAILURE.to_string());
        }
    }
    ProbeReport::passed(http_status_description(STATUS_SUCCESS_PREFIX, status))
}

/// Whether `body` carries `needle`, as the case-sensitive substring test
/// `HealthCheckConfig$SearchString` documents ("Route 53 considers case when searching for
/// SearchString in the response body").
///
/// A check with no string to search for has nothing that can appear in a body, so nothing
/// matches — the configuration a create or an update would have to have persisted for that to
/// be asked ([`super::health_check_config_problem`] refuses a string-match type that names no
/// `SearchString`).
fn holds_search_string(body: &[u8], needle: &[u8]) -> bool {
    if needle.is_empty() {
        return false;
    }
    body.windows(needle.len()).any(|window| window == needle)
}

/// The description a checker reports an ANSWER as: the prefix the observation is reported
/// under and the status the endpoint answered with, named as the status line names it
/// (`Success: HTTP Status Code 200, OK`).
fn http_status_description(prefix: &str, status: u16) -> String {
    match ::http::StatusCode::from_u16(status)
        .ok()
        .and_then(|code| code.canonical_reason())
    {
        Some(reason) => format!("{prefix}HTTP Status Code {status}, {reason}"),
        // A status no reason phrase is registered for is still the status the endpoint
        // answered with.
        None => format!("{prefix}HTTP Status Code {status}"),
    }
}

/// What one fetch of the endpoint came back with.
enum Fetched {
    /// The endpoint answered: the status, and the bytes it carried where the checker could
    /// read them — `None` for an answer whose body was not there to read, which is a body the
    /// string-match types searched nothing in and the other types never look at.
    Answered {
        status: u16,
        body: Option<Vec<u8>>,
    },
    /// Nothing in this emulator serves that endpoint, or the transport could not carry the
    /// request: the checker reached nothing.
    Unreachable,
}

/// Send one request to `target` over the in-process transport and read the answer.
///
/// The request carries NO credential, which is what a health checker sends: "Route 53
/// submits an HTTP request" to an endpoint of the caller's, holding nothing of the caller's
/// own and nothing of the service's. So a probe observes exactly what the endpoint serves to
/// everyone — a surface this emulator serves unsigned answers it, and one that requires a
/// signature answers its own refusal, which is reported as the answer it is.
async fn fetch(transport: &EmbeddedEmulator, target: &ProbeTarget) -> Fetched {
    use ::aws_smithy_runtime_api::client::http::HttpConnector as _;

    let built = ::http::Request::get(target.uri.as_str())
        .header(::http::header::HOST, target.host.as_str())
        .body(SdkBody::empty());
    let Ok(request) = built else {
        // A URI or a host the transport cannot even be handed is an endpoint nothing was
        // asked about, which is the same fact as one nothing serves.
        return Fetched::Unreachable;
    };
    let Ok(request) = ::aws_smithy_runtime_api::http::Request::try_from(request) else {
        return Fetched::Unreachable;
    };
    let answered = ::tokio::time::timeout(PROBE_TIMEOUT, transport.http_connector().call(request));
    let Ok(Ok(response)) = answered.await else {
        ::tracing::debug!(
            uri = target.uri.as_str(),
            "a health checker's probe did not reach the endpoint the configuration names"
        );
        return Fetched::Unreachable;
    };
    let status = response.status().as_u16();
    // Only the prefix a probe can read is taken out of the answer: the string-match types
    // search the first [`MAX_SEARCHED_BODY_BYTES`] of the body and the other types read none
    // of it, so an endpoint of this emulator serving a large object cannot make a probe round
    // copy it.
    let body = response
        .into_body()
        .bytes()
        .map(|bytes| bytes[..bytes.len().min(MAX_SEARCHED_BODY_BYTES)].to_vec());
    // The front door's own refusal of a call that reached no service is not an endpoint's
    // answer: nothing here serves that address.
    if body
        .as_deref()
        .is_some_and(::service_common::in_process_refusal::is_transport_refusal)
    {
        return Fetched::Unreachable;
    }
    Fetched::Answered { status, body }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A configuration of `kind` pointed at `domain`, with the search string `needle` where
    /// the type takes one.
    fn config(kind: &str, needle: &str) -> HealthCheckConfig {
        HealthCheckConfig {
            r#type: kind.to_string(),
            fully_qualified_domain_name: Some("site.example.com".to_string()),
            search_string: (!needle.is_empty()).then(|| needle.to_string()),
            ..HealthCheckConfig::default()
        }
    }

    #[test]
    fn a_string_match_probe_searches_the_answered_body_for_the_configured_string() {
        // The rows differ along what the rule generalises over: whether the string is in the
        // body, where it sits, its case, the case of the body, a string that is a prefix of a
        // longer word, a body that carries the string past the searched prefix and one that
        // carries it just inside it, and a body that is not text at all.
        let far = format!("{}MARKER", "p".repeat(MAX_SEARCHED_BODY_BYTES));
        let near = format!(
            "{}MARKER",
            "p".repeat(MAX_SEARCHED_BODY_BYTES - "MARKER".len())
        );
        let rows: Vec<(&str, &str, &[u8], bool)> = vec![
            ("the string is the whole body", "MARKER", b"MARKER", true),
            (
                "the string sits inside the body",
                "MARKER",
                b"<html>MARKER</html>",
                true,
            ),
            (
                "the body no longer carries the string",
                "MARKER",
                b"<html>maintenance</html>",
                false,
            ),
            (
                "the body carries the string in another case",
                "MARKER",
                b"<html>marker</html>",
                false,
            ),
            (
                "the configured string is in another case",
                "marker",
                b"<html>MARKER</html>",
                false,
            ),
            (
                "the string is a prefix of a longer word",
                "MARK",
                b"<html>MARKER</html>",
                true,
            ),
            ("an empty body", "MARKER", b"", false),
            (
                "the string sits just inside the searched prefix",
                "MARKER",
                near.as_bytes(),
                true,
            ),
            (
                "the string sits past the searched prefix",
                "MARKER",
                far.as_bytes(),
                false,
            ),
            (
                "a body that is not text",
                "MARKER",
                &[0x00, 0xff, b'M', b'A', b'R', b'K', b'E', b'R', 0x00],
                true,
            ),
            (
                "a check with no string to search for",
                "",
                b"<html>MARKER</html>",
                false,
            ),
        ];
        for (case, needle, body, healthy) in rows {
            for kind in STRING_MATCH_HEALTH_CHECK_TYPES {
                let report = assess(&config(kind, needle), 200, Some(body));
                assert_eq!(
                    report.healthy, healthy,
                    "case {case:?} ({kind}): expected healthy {healthy}, got {report:?}"
                );
                let expected_prefix = if healthy {
                    STATUS_SUCCESS_PREFIX
                } else {
                    STATUS_FAILURE_PREFIX
                };
                assert!(
                    report.status.starts_with(expected_prefix),
                    "case {case:?} ({kind}): expected a status opening with \
                     {expected_prefix:?}, got {:?}",
                    report.status
                );
                if !healthy {
                    assert_eq!(
                        report.status, STATUS_STRING_MATCH_FAILURE,
                        "case {case:?} ({kind}): a body that does not carry the string is a \
                         string-match failure, got {:?}",
                        report.status
                    );
                }
            }
        }
    }

    #[test]
    fn the_request_types_accept_the_statuses_their_type_documents() {
        // `HealthCheckType`: "waits for an HTTP status code of 200 or greater and less than
        // 400". The rows are both published bounds, one past each of them, and the statuses in
        // between a client is most likely to see.
        let rows: Vec<(&str, u16, bool)> = vec![
            ("one below the accepted range", 199, false),
            ("the lowest accepted status", 200, true),
            ("a redirect", 301, true),
            ("the highest accepted status", 399, true),
            ("the first rejected status", 400, false),
            ("an endpoint that is not serving", 503, false),
        ];
        for (case, status, healthy) in rows {
            for kind in ["HTTP", "HTTPS"] {
                let report = assess(&config(kind, ""), status, Some(b"<html>page</html>"));
                assert_eq!(
                    report.healthy, healthy,
                    "case {case:?} ({kind}): expected healthy {healthy} for status {status}, \
                     got {report:?}"
                );
                assert!(
                    report.status.contains(&format!("HTTP Status Code {status}")),
                    "case {case:?} ({kind}): the description names the status the endpoint \
                     answered with, got {:?}",
                    report.status
                );
            }
            // A string-match check's request is subject to the same statuses, and its body is
            // searched only for an answer inside the accepted range.
            let report = assess(&config("HTTP_STR_MATCH", "page"), status, Some(b"page"));
            assert_eq!(
                report.healthy, healthy,
                "case {case:?} (HTTP_STR_MATCH): expected healthy {healthy} for status \
                 {status}, got {report:?}"
            );
        }
        // The description of the lowest accepted status is the one this crate names as the
        // answer of an endpoint that is serving.
        let serving = assess(&config("HTTP", ""), LOWEST_HEALTHY_STATUS, Some(b"page"));
        assert_eq!(
            serving.status,
            crate::state::STATUS_HTTP_OK,
            "the description of a {LOWEST_HEALTHY_STATUS} answer is the one the store names, \
             got {serving:?}"
        );
    }

    #[test]
    fn a_tcp_probe_reports_the_connection_it_established() {
        // A TCP check "tries to establish a TCP connection" and nothing more, so the status
        // the endpoint answered with and the body it carried are immaterial to it.
        for (case, status, body) in [
            ("an endpoint serving a page", 200u16, b"MARKER".as_slice()),
            ("an endpoint answering an error", 503, b"".as_slice()),
        ] {
            let report = assess(&config(HEALTH_CHECK_TYPE_TCP, ""), status, Some(body));
            assert!(
                report.healthy && report.status == STATUS_TCP_CONNECTION_ESTABLISHED,
                "case {case:?}: expected the established-connection report, got {report:?}"
            );
        }
    }

    #[test]
    fn only_the_string_match_types_need_a_body_they_could_read() {
        // An answer whose body the checker could not read is still an answer: the types whose
        // criterion is the status alone pass on it, and the types that search the body find
        // nothing in a body they do not have.
        // (case, type, healthy)
        let rows: &[(&str, &str, bool)] = &[
            ("a connection check", HEALTH_CHECK_TYPE_TCP, true),
            ("a request check", "HTTP", true),
            ("a TLS request check", "HTTPS", true),
            ("a string-match check", "HTTP_STR_MATCH", false),
            ("a TLS string-match check", "HTTPS_STR_MATCH", false),
        ];
        for (case, kind, healthy) in rows {
            let report = assess(&config(kind, "MARKER"), LOWEST_HEALTHY_STATUS, None);
            assert_eq!(
                report.healthy, *healthy,
                "case {case:?}: expected healthy {healthy} for an answer with no readable \
                 body, got {report:?}"
            );
        }
    }

    /// One row of the addressing table: a configuration, and where a probe of it goes.
    struct AddressRow {
        case: &'static str,
        kind: &'static str,
        address: &'static str,
        domain: &'static str,
        port: Option<i32>,
        path: &'static str,
        uri: &'static str,
        host: &'static str,
    }

    #[test]
    fn the_probe_is_addressed_as_the_configuration_describes_it() {
        let rows: Vec<AddressRow> = vec![
            AddressRow {
                case: "a name on the port its scheme is served on",
                kind: "HTTP_STR_MATCH",
                address: "",
                domain: "site.example.com",
                port: Some(80),
                path: "/",
                uri: "http://site.example.com:80/",
                host: "site.example.com",
            },
            AddressRow {
                case: "a TLS type on its own port",
                kind: "HTTPS_STR_MATCH",
                address: "",
                domain: "site.example.com",
                port: Some(443),
                path: "/index.html",
                uri: "https://site.example.com:443/index.html",
                host: "site.example.com",
            },
            AddressRow {
                case: "a port the scheme is not served on, which the Host header carries",
                kind: "HTTP",
                address: "",
                domain: "site.example.com",
                port: Some(8080),
                path: "/health",
                uri: "http://site.example.com:8080/health",
                host: "site.example.com:8080",
            },
            AddressRow {
                case: "an address with a name to pass in the Host header",
                kind: "HTTP",
                address: "192.0.2.10",
                domain: "site.example.com",
                port: Some(80),
                path: "",
                uri: "http://192.0.2.10:80/",
                host: "site.example.com",
            },
            AddressRow {
                case: "an address with no name at all",
                kind: "TCP",
                address: "192.0.2.10",
                domain: "",
                port: Some(443),
                path: "",
                uri: "http://192.0.2.10:443/",
                host: "192.0.2.10",
            },
            AddressRow {
                case: "a TCP check of a name on a port of its own, which sends no Host of its own",
                kind: "TCP",
                address: "",
                domain: "db.example.com",
                port: Some(5432),
                path: "",
                uri: "http://db.example.com:5432/",
                host: "db.example.com",
            },
            AddressRow {
                case: "an IPv6 address, bracketed in the authority",
                kind: "HTTP",
                address: "2001:db8::1",
                domain: "",
                port: None,
                path: "/",
                uri: "http://[2001:db8::1]:80/",
                host: "2001:db8::1",
            },
            AddressRow {
                case: "a path carrying a query string, which the member admits",
                kind: "HTTP",
                address: "",
                domain: "site.example.com",
                port: None,
                path: "/welcome.html?language=jp",
                uri: "http://site.example.com:80/welcome.html?language=jp",
                host: "site.example.com",
            },
            AddressRow {
                case: "a path that does not open at the root",
                kind: "HTTP",
                address: "",
                domain: "site.example.com",
                port: None,
                path: "health",
                uri: "http://site.example.com:80/health",
                host: "site.example.com",
            },
            AddressRow {
                case: "a name written with the root label's dot",
                kind: "HTTP_STR_MATCH",
                address: "",
                domain: "site.example.com.",
                port: None,
                path: "/",
                uri: "http://site.example.com:80/",
                host: "site.example.com",
            },
        ];
        for row in rows {
            let case = row.case;
            let config = HealthCheckConfig {
                r#type: row.kind.to_string(),
                ip_address: (!row.address.is_empty()).then(|| row.address.to_string()),
                fully_qualified_domain_name: (!row.domain.is_empty())
                    .then(|| row.domain.to_string()),
                port: row.port,
                resource_path: (!row.path.is_empty()).then(|| row.path.to_string()),
                ..HealthCheckConfig::default()
            };
            let target = probe_target(&config)
                .unwrap_or_else(|| panic!("case {case:?}: an endpoint check has a target"));
            assert_eq!(
                (target.uri.as_str(), target.host.as_str()),
                (row.uri, row.host),
                "case {case:?}: expected the probe to be addressed to {:?} with Host {:?}, got \
                 {target:?}",
                row.uri,
                row.host
            );
        }
    }

    #[test]
    fn a_check_that_probes_no_endpoint_has_nothing_to_probe() {
        // The types that read a CloudWatch alarm, combine child health checks or follow a
        // routing control have no endpoint, so there is no probe to make of them.
        for kind in ["CALCULATED", "CLOUDWATCH_METRIC", "RECOVERY_CONTROL"] {
            let target = probe_target(&config(kind, ""));
            assert!(
                target.is_none(),
                "case {kind:?}: a check that probes no endpoint has no probe target, got \
                 {target:?}"
            );
        }
    }
}
