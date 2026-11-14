// SPDX-License-Identifier: CC0-1.0

//! The ONE reading of how a client ADDRESSES an emulated service's DATA plane, for every
//! service that serves one.
//!
//! A data plane is reached at a host the OWNING RESOURCE names, not at the service's
//! endpoint: a load balancer's `DNSName`, a REST API's
//! `{api-id}.execute-api.{region}.amazonaws.com`, a distribution's domain name. The first
//! label of such a host names no service, so the emulator's front door can resolve no
//! target service from the request and answers `404 UnknownService` — for an address the
//! service itself told the caller to use. Each plane therefore claims its own traffic in
//! `Service::can_handle`, and a client on the machine this emulator runs on, which resolves
//! none of those hosts, reaches the emulator's own endpoint and carries in the PATH what it
//! cannot carry in the host.
//!
//! What every one of them needs to read that addressing is the same, and is here so the
//! planes cannot drift on which requests they claim: the reserved path segment that names a
//! plane ([`EMULATOR_PATH_NAMESPACE`]), the first path segment and the rest
//! ([`first_segment`]), the host without its userinfo or port ([`host_only`]), the port a
//! client addressed ([`addressed_port`]), whether a host names a SERVICE at all
//! ([`names_a_service`]) — which is what separates the emulator's own endpoint from
//! another service's — whether a host is one AWS itself publishes an endpoint at
//! ([`addresses_an_aws_endpoint`]), which is what separates an endpoint of AWS's from an
//! ordinary hostname an operator points a client at, and the address of the endpoint this
//! emulator answers on, which is what a DISCOVERY operation publishes for a plane served at
//! the front door ([`emulator_endpoint_authority`]), or the one a service ANNOUNCES to a
//! caller that resolves AWS's own hosts ([`served_endpoint_authority`]).
//!
//! Services still carrying their own copy of these: this module postdates them, and
//! `grep -rln 'EMULATOR_PATH_NAMESPACE' services/*/src` lists them (today the API Gateway,
//! API Gateway V2 and CloudFront planes). Those copies are to be MIGRATED onto this module;
//! they are left in place only because a change does not own those services' issues.
//! Nothing new inlines the rule again — call this module.

/// The reserved path segment that names a DATA plane on the one endpoint this emulator
/// serves its whole surface at, ahead of the segment naming WHICH plane:
/// `/_aws/{plane}/{resource}/{path}`.
///
/// It begins with `_`, which no modelled path segment of any service does, so the prefix
/// collides with no control-plane route however a resource is named — and a path under it
/// that names ANOTHER plane is that plane's, never this one's.
pub const EMULATOR_PATH_NAMESPACE: &str = "_aws";

/// The first path segment and the path that follows it, with the leading `/` kept on the
/// remainder so it stays a path (`/a/b` -> `("a", "/b")`, `/a` -> `("a", "/")`).
///
/// `None` for a path that is not absolute, which is no addressing at all.
#[must_use]
pub fn first_segment(path: &str) -> Option<(&str, &str)> {
    let trimmed = path.strip_prefix('/')?;
    match trimmed.find('/') {
        Some(end) => Some((&trimmed[..end], &trimmed[end..])),
        None => Some((trimmed, "/")),
    }
}

/// A host with any userinfo and `:port` removed, which is the name a resource publishes.
/// An IPv6 literal keeps its brackets, which is where its own colons live.
#[must_use]
pub fn host_only(host: &str) -> &str {
    let without_userinfo = host.rsplit('@').next().unwrap_or(host);
    if let Some(end) = without_userinfo.find(']') {
        return &without_userinfo[..=end];
    }
    without_userinfo
        .split(':')
        .next()
        .unwrap_or(without_userinfo)
}

/// The port a host names, when it names one: the port the client addressed, which is what
/// decides which of a resource's listeners or stages the request reached.
#[must_use]
pub fn addressed_port(host: Option<&str>) -> Option<i32> {
    let authority = host?.rsplit('@').next()?;
    let after_host = match authority.find(']') {
        Some(end) => &authority[end + 1..],
        None => authority,
    };
    after_host.rsplit_once(':')?.1.parse::<i32>().ok()
}

/// The host a request ADDRESSED, which is what a plane compares its resource's published
/// name against: the `Host` header, or the authority of an absolute-form URI for a
/// request that carries its endpoint there (HTTP/2, or an SDK-built request).
///
/// Both are passed in rather than a whole request, because the framework hands
/// `Service::can_handle` the request's PARTS and the other seams a runtime-API request:
/// each has these two things and no common type.
///
/// `None` when the request names no host at all, which addresses nothing.
#[must_use]
pub fn addressed_host(uri: &str, host_header: Option<&str>) -> Option<String> {
    if let Some(host) = host_header.filter(|host| !host.is_empty()) {
        return Some(host.to_string());
    }
    let after_scheme = uri.split_once("://")?.1;
    let authority = after_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or(after_scheme);
    (!authority.is_empty()).then(|| authority.to_string())
}

/// The endpoint host a request REACHED this emulator on, as a URL the service hands
/// back has to name: the host the request addressed when it is shaped like one, and the
/// service's published endpoint for `region` otherwise.
///
/// A URL an operation announces — an upload destination, a published document, a site —
/// has to resolve back to where the caller is already talking, or it names a host this
/// emulator does not serve. Only a value SHAPED like a host is taken ([`is_host_shaped`]),
/// so a caller cannot get an arbitrary string of its own into a URL a service reports.
///
/// Services still carrying their own copy of this reading, to be MIGRATED onto it (they
/// predate this function):
/// `services/service-com-amazonaws-wisdom-wisdomservice/src/state.rs` and
/// `services/service-com-amazonaws-qconnect-wisdomservice/src/state.rs`. Nothing new
/// inlines it again — call this module.
#[must_use]
pub fn reached_endpoint_host(
    uri: &str,
    host_header: Option<&str>,
    endpoint_prefix: &str,
    region: &str,
) -> String {
    addressed_host(uri, host_header)
        .filter(|host| is_host_shaped(host))
        .unwrap_or_else(|| format!("{endpoint_prefix}.{region}.amazonaws.com"))
}

/// The `host:port` a URL addresses one of the EMULATOR'S OWN listeners by — the address
/// a service PUBLISHES for a resource it opened a core listener for
/// (`engine.listeners()`), which is the one address a client on this machine can reach
/// that resource at.
///
/// The core's own reading of that question
/// ([`::emulator_server::ResourceListener::authority`], "the `host:port` a client dials…
/// Use it for a field an AWS API returns without a scheme"), so a published address is the
/// one the core says the listener is dialable at: a WILDCARD bind (`0.0.0.0`, `::`) has no
/// single address to dial and is published as loopback, and an IPv6 bind is bracketed, so
/// the URL parses wherever the operator bound it.
///
/// Services still carrying their own copy of this reading, to be MIGRATED onto this function
/// (they predate it):
/// `services/service-com-amazonaws-pricing-awspricelistservice/src/state.rs` and
/// `services/service-com-amazonaws-lambdamicrovms-lambdamicrovms/src/state/microvm_endpoint.rs`.
/// Nothing new inlines it again — call this module.
#[must_use]
pub fn listener_authority(listener: &::emulator_server::ResourceListener) -> String {
    listener.authority()
}

/// The `host[:port]` of the ONE endpoint this emulator serves its whole surface at — what
/// a DISCOVERY operation publishes for a plane the client then DIALS, when that plane is
/// served at the emulator's own front door rather than on a listener of its own.
///
/// [`::smithy_server_runtime::OperationContext::endpoint`] is "the base URL a later request
/// addresses… Callers send normally signed requests to it"; this is that base without its
/// scheme ([`url_authority`]), because what such an operation publishes is an address a
/// client appends a path to, not a URL.
///
/// It is the data port's own published base, so it does not depend on which host the
/// request that read it carried: an endpoint discovered through an in-process call is
/// published at the same address as one a client discovered itself.
#[must_use]
pub fn emulator_endpoint_authority(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
) -> String {
    url_authority(ctx.endpoint().as_str()).to_string()
}

/// The `host[:port]` this emulator is SERVED at, read off the base URL
/// [`::smithy_server_runtime::OperationContext::endpoint`] reports ("the base URL a later
/// request addresses… Callers send normally signed requests to it"), or `None` for an
/// emulator that is served on no port at all — which is what its unserved base names
/// ([`::smithy_server_runtime::UNSERVED_URL_BASE`], a host RFC 6761 reserves so that
/// nothing resolves it).
///
/// The address a service announces for a resource whose plane is served at the front door,
/// for a request that addressed one of AWS's own hosts: an in-process call of another
/// service is announced the one address this process answers on, so the caller of that
/// service can reach the resource too. [`reached_emulator_endpoint`] is the reading for the
/// request that addressed the emulator's own endpoint directly, and this one the fallback
/// behind it.
///
/// Only a value SHAPED like a host is taken ([`is_host_shaped`]), so nothing that is not an
/// address reaches a URL a service announces.
///
/// Services still carrying their own copy of this reading, to be MIGRATED onto it (they
/// predate this function):
/// `services/service-com-amazonaws-apigatewayv2-apigatewayv2/src/state.rs` and
/// `services/service-com-amazonaws-pcaconnectorscep-pcaconnectorscep/src/state.rs`. Nothing
/// new inlines it again — call this module.
#[must_use]
pub fn served_endpoint_authority(served_base: &str) -> Option<String> {
    if served_base == ::smithy_server_runtime::UNSERVED_URL_BASE {
        return None;
    }
    let authority = url_authority(served_base).trim_end_matches('/');
    is_host_shaped(authority).then(|| authority.to_string())
}

/// The authority of `url`: what follows its `scheme://`, or the whole value when it names
/// no scheme — which is an authority already.
#[must_use]
pub fn url_authority(url: &str) -> &str {
    url.split_once("://")
        .map_or(url, |(_scheme, authority)| authority)
}

/// A `host:port` authority, with an IPv6 host bracketed as RFC 3986 requires
/// (`[::1]:8443`) and any host already written in brackets left as it is.
#[must_use]
pub fn host_port_authority(host: &str, port: u16) -> String {
    if host.starts_with('[') || host.parse::<std::net::Ipv4Addr>().is_ok() {
        return format!("{host}:{port}");
    }
    if host.parse::<std::net::Ipv6Addr>().is_ok() {
        return format!("[{host}]:{port}");
    }
    format!("{host}:{port}")
}

/// The EMULATOR's OWN endpoint a request reached, or `None` for a request addressed to a
/// host that names some service — which is a caller that resolves AWS's own hostnames.
///
/// The counterpart of [`reached_endpoint_host`] for a URL whose whole point is that the
/// client DIALS it as given (an ACME directory handed to certbot, a data-plane endpoint a
/// stock client is configured with): such a caller resolves nothing but the endpoint it is
/// already talking to, so `Some(endpoint)` is the address to publish — with the resource's
/// identity in the PATH, under [`EMULATOR_PATH_NAMESPACE`] — and `None` is the caller that
/// is told the service's own published host instead.
///
/// Composed from the two readings above, so no service writes a second rule for which
/// host a request addressed or which host names a service.
#[must_use]
pub fn reached_emulator_endpoint(uri: &str, host_header: Option<&str>) -> Option<String> {
    let host = addressed_host(uri, host_header)?;
    (is_host_shaped(&host) && !names_a_service(&host)).then_some(host)
}

/// Whether `host` is a non-empty `host[:port]` written with the characters a hostname, an
/// IP literal and a port carry, and no longer than a DNS name may be.
#[must_use]
pub fn is_host_shaped(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b':' | b'[' | b']')
        })
}

/// Whether `host` names some SERVICE: a service's own endpoint, control plane or data
/// plane. Such a host keeps answering as the service it names, so an addressing that reads
/// the EMULATOR's own endpoint is read only when it names none.
///
/// The front door's OWN host reader decides it for a named host
/// (`emulator_server::service_from_host`), so a plane reads a host exactly as the router
/// does. An ADDRESS names no service whatever that reader makes of it: it is the only
/// endpoint a client on this machine can reach, and a bracketed IPv6 literal carries its
/// own colons, which that reader's `:port` split does not expect.
#[must_use]
pub fn names_a_service(host: &str) -> bool {
    service_named_by_host(host).is_some()
}

/// WHICH service `host` names, by the same reading [`names_a_service`] answers from — for a
/// plane that has to tell its OWN service's endpoint from another service's, where "names a
/// service" alone would reject the host its own control plane answers on.
///
/// `None` for a host that names none: the emulator's own endpoint, an IP literal (a
/// bracketed IPv6 one included, whose colons that reader's `:port` split does not expect),
/// and a value no host header can carry.
#[must_use]
pub fn service_named_by_host(host: &str) -> Option<String> {
    let host = host_only(host);
    if host.starts_with('[') || host.parse::<std::net::IpAddr>().is_ok() {
        return None;
    }
    let value = ::http::HeaderValue::from_str(host).ok()?;
    let mut headers = ::http::HeaderMap::new();
    headers.insert("host", value);
    ::emulator_server::service_from_host(&headers)
}

/// The DNS zones AWS publishes its OWN service endpoints in: the `dnsSuffix` its
/// partitions declare (`amazonaws.com`, and `amazonaws.com.cn` for China) and the
/// suffixes it serves dual-stack and short-form endpoints under (`api.aws`, `on.aws`).
///
/// A host under one of them is an endpoint AWS itself publishes, so its first label names
/// the SERVICE that answers there — which is the only thing about a host that names a
/// service. [`names_a_service`] asks the front door's own host reader, and that reader
/// takes the first label of ANY host: it answers `Some` for an ordinary hostname an
/// operator points a client at (a compose service name, a reverse proxy, a corporate DNS
/// name) as well, and such a key names nothing. A service that has to tell "somebody
/// else's endpoint" from "a host naming no service at all" reads this.
///
/// A domain a SERVICE assigns a resource (a distribution's `cloudfront.net` name, say) is
/// not here: this is the list of endpoint zones, and a service that also excludes its own
/// assigned-domain zone adds it where that question is asked.
///
/// Services still carrying their own copy of this list, to be MIGRATED onto this one (they
/// predate it): the two readers in
/// `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/src/service/`. Nothing
/// new writes the list again — call this module.
pub const AWS_ENDPOINT_DNS_SUFFIXES: [&str; 4] = [
    ".amazonaws.com",
    ".amazonaws.com.cn",
    ".api.aws",
    ".on.aws",
];

/// Whether `host` is an endpoint AWS itself publishes: a host under one of
/// [`AWS_ENDPOINT_DNS_SUFFIXES`].
///
/// Any userinfo and `:port` are removed first ([`host_only`]) and the comparison ignores
/// case, since a `Host` header carries whatever case the client wrote.
#[must_use]
pub fn addresses_an_aws_endpoint(host: &str) -> bool {
    let host = host_only(host).to_ascii_lowercase();
    AWS_ENDPOINT_DNS_SUFFIXES
        .iter()
        .any(|suffix| host.ends_with(suffix))
}

/// Whether a request target carries a DOT SEGMENT — `.` or `..`, in the clear or
/// percent-encoded, read through the one decoding of a request path
/// ([`::smithy_server_runtime::percent_decode`]) so an encoded separator cannot hide one.
///
/// The guard a plane served on a RESERVED PATH needs: such a target resolves, for anything
/// that resolves it, to a path that need not stay under the prefix naming the resource whose
/// plane must answer — and that prefix is the only thing that says whose configuration the
/// request is served from. A socket that forwards a client's path under such a prefix
/// therefore refuses the request itself rather than forwarding it.
///
/// Services still carrying their own copy of this reading, to be MIGRATED onto this function
/// (they predate it): the ELBv2 and API Gateway custom-domain sockets, and the CloudFront
/// edge endpoint. Nothing new inlines it again — call this module.
#[must_use]
pub fn carries_a_dot_segment(request_path: &str) -> bool {
    ::smithy_server_runtime::percent_decode(request_path)
        .split('/')
        .any(|segment| segment == "." || segment == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_target_carries_a_dot_segment_however_it_is_spelled() {
        // The rows span what the reading generalises over: ordinary paths, a dot and a
        // double-dot segment in the clear, their percent-encoded spellings in either case,
        // and a segment that merely contains a dot.
        let cases: &[(&str, bool)] = &[
            ("/", false),
            ("/a/b", false),
            ("/index.html", false),
            ("/a/./b", true),
            ("/../b", true),
            ("/a/%2e%2e/b", true),
            ("/a/%2E/b", true),
        ];
        for (path, expected) in cases {
            let read = carries_a_dot_segment(path);
            assert_eq!(
                read, *expected,
                "case {path:?}: expected {expected}, got {read}"
            );
        }
    }

    #[test]
    fn a_path_splits_into_its_first_segment_and_the_rest() {
        let cases: &[(&str, Option<(&str, &str)>)] = &[
            ("/a/b", Some(("a", "/b"))),
            ("/a", Some(("a", "/"))),
            ("/", Some(("", "/"))),
            ("/_aws/execute-api/x", Some(("_aws", "/execute-api/x"))),
            ("a/b", None),
        ];
        for (path, expected) in cases {
            let split = first_segment(path);
            assert_eq!(
                split, *expected,
                "case {path:?}: expected {expected:?}, got {split:?}"
            );
        }
    }

    #[test]
    fn a_request_reaches_an_endpoint_host_or_the_published_one() {
        // The rows span what the reading generalises over: the host a caller addressed,
        // a loopback endpoint with a port, a host-shaped value on the URI, a value no
        // host is shaped like, and a request naming no host at all.
        let published = "amplify.us-east-1.amazonaws.com";
        let cases: &[(&str, Option<&str>, &str)] = &[
            ("/apps", Some("amplify.us-east-1.amazonaws.com"), published),
            ("/apps", Some("127.0.0.1:9297"), "127.0.0.1:9297"),
            ("https://localhost:9297/apps", None, "localhost:9297"),
            ("/apps", Some("host with spaces"), published),
            ("/apps", None, published),
        ];
        for (uri, header, expected) in cases {
            let answered = reached_endpoint_host(uri, *header, "amplify", "us-east-1");
            assert_eq!(
                answered, *expected,
                "case ({uri:?}, {header:?}): expected {expected:?}, got {answered:?}"
            );
        }
    }

    #[test]
    fn a_request_reaches_the_emulator_endpoint_only_when_the_host_names_no_service() {
        // The rows span what the reading generalises over: every shape of address the
        // emulator's own endpoint takes (an IPv4 literal with and without a port, a
        // loopback name, a bracketed IPv6 literal), a host that names a service's own
        // endpoint, a host-shaped value on an absolute URI, a value no host is shaped
        // like, and a request naming no host at all.
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("/apps", Some("127.0.0.1:9297"), Some("127.0.0.1:9297")),
            ("/apps", Some("127.0.0.1"), Some("127.0.0.1")),
            ("/apps", Some("localhost:9297"), Some("localhost:9297")),
            ("/apps", Some("[::1]:9297"), Some("[::1]:9297")),
            ("/apps", Some("s3.us-west-2.amazonaws.com"), None),
            ("https://localhost:9297/apps", None, Some("localhost:9297")),
            ("/apps", Some("host with spaces"), None),
            ("/apps", None, None),
        ];
        for (uri, header, expected) in cases {
            let answered = reached_emulator_endpoint(uri, *header);
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case ({uri:?}, {header:?}): expected {expected:?}, got {answered:?}"
            );
        }
    }

    #[test]
    fn an_endpoint_base_reads_as_the_authority_a_client_dials() {
        // The rows span the bases `OperationContext::endpoint` reports: the unserved
        // base, a loopback bind with a port, a bracketed IPv6 bind, a name, and a value
        // that is an authority already.
        let cases: &[(&str, &str)] = &[
            ("https://emulator.invalid", "emulator.invalid"),
            ("https://127.0.0.1:39355", "127.0.0.1:39355"),
            ("https://[::1]:9297", "[::1]:9297"),
            ("http://localhost:9297", "localhost:9297"),
            ("127.0.0.1:9297", "127.0.0.1:9297"),
        ];
        for (base, expected) in cases {
            let authority = url_authority(base);
            assert_eq!(
                authority, *expected,
                "case {base:?}: expected {expected:?}, got {authority:?}"
            );
        }
    }

    #[test]
    fn a_served_base_reads_as_the_authority_only_when_the_emulator_is_served() {
        // The rows span the bases `OperationContext::endpoint` reports: the unserved base
        // (served on no port at all), a loopback bind with a port, a bracketed IPv6 bind, a
        // name, a base carrying a trailing slash, and a value no host is shaped like.
        let cases: &[(&str, Option<&str>)] = &[
            (::smithy_server_runtime::UNSERVED_URL_BASE, None),
            ("https://127.0.0.1:39355", Some("127.0.0.1:39355")),
            ("https://[::1]:9297", Some("[::1]:9297")),
            ("http://localhost:9297", Some("localhost:9297")),
            ("https://127.0.0.1:9297/", Some("127.0.0.1:9297")),
            ("https://host with spaces", None),
        ];
        for (base, expected) in cases {
            let authority = served_endpoint_authority(base);
            assert_eq!(
                authority.as_deref(),
                *expected,
                "case {base:?}: expected {expected:?}, got {authority:?}"
            );
        }
    }

    #[test]
    fn a_listeners_address_is_written_as_a_url_parses_it() {        // The rows span the bind addresses a core listener reports: an IPv4 literal, an
        // IPv6 literal (bracketed here), one already bracketed, and a name.
        let cases: &[(&str, u16, &str)] = &[
            ("127.0.0.1", 9297, "127.0.0.1:9297"),
            ("::1", 9297, "[::1]:9297"),
            ("[::1]", 8443, "[::1]:8443"),
            ("localhost", 443, "localhost:443"),
        ];
        for (host, port, expected) in cases {
            let authority = host_port_authority(host, *port);
            assert_eq!(
                authority, *expected,
                "case ({host:?}, {port}): expected {expected:?}, got {authority:?}"
            );
        }
    }

    #[test]
    fn a_request_addresses_the_host_it_names() {
        // The rows span where a host can travel: the header, an absolute-form URI with
        // and without a path or query, an empty header falling back to the URI, and a
        // request naming no host at all.
        let cases: &[(&str, Option<&str>, Option<&str>)] = &[
            ("/apps/d1", Some("main.d1.amplifyapp.com"), Some("main.d1.amplifyapp.com")),
            ("https://main.d1.amplifyapp.com/index.html", None, Some("main.d1.amplifyapp.com")),
            ("https://127.0.0.1:9297", None, Some("127.0.0.1:9297")),
            ("https://host.example/p?q=a://b", None, Some("host.example")),
            ("https://host.example/p", Some(""), Some("host.example")),
            ("/apps/d1", None, None),
        ];
        for (uri, header, expected) in cases {
            let answered = addressed_host(uri, *header);
            assert_eq!(
                answered.as_deref(),
                *expected,
                "case ({uri:?}, {header:?}): expected {expected:?}, got {answered:?}"
            );
        }
    }

    #[test]
    fn a_host_is_an_aws_endpoint_only_inside_a_zone_aws_publishes_endpoints_in() {
        // The rows span every suffix the list carries, each with and without a port or a
        // deeper label, against the hosts that are NOT AWS's: an ordinary operator
        // hostname, a single-label name, a loopback address, a bracketed IPv6 literal, a
        // host that merely CONTAINS a suffix without ending in it, and a host whose name
        // is the suffix itself with no service label in front of it.
        let cases: &[(&str, bool)] = &[
            ("iam.amazonaws.com", true),
            ("s3.us-west-2.amazonaws.com", true),
            ("S3.US-WEST-2.AMAZONAWS.COM", true),
            ("portal.sso.us-east-1.amazonaws.com:443", true),
            ("dynamodb.cn-north-1.amazonaws.com.cn", true),
            ("lambda.us-east-1.api.aws", true),
            ("console.us-east-1.on.aws", true),
            ("user@s3.us-west-2.amazonaws.com:8443", true),
            ("emulator.corp.local", false),
            ("host.docker.internal:9297", false),
            ("aws:9297", false),
            ("localhost:9297", false),
            ("127.0.0.1:9297", false),
            ("[::1]:9297", false),
            ("amazonaws.com.evil.example", false),
            ("amazonaws.com", false),
            ("", false),
        ];
        for (host, expected) in cases {
            let answered = addresses_an_aws_endpoint(host);
            assert_eq!(
                answered, *expected,
                "case {host:?}: expected addresses_an_aws_endpoint {expected}, got {answered}"
            );
        }
    }

    #[test]
    fn a_host_reads_as_a_name_a_port_and_whether_it_names_a_service() {
        // The rows span what the reading generalises over: a published resource host, a
        // service endpoint, every shape of loopback address, and userinfo.
        let cases: &[(&str, &str, Option<i32>, bool)] = &[
            (
                "my-alb-123.us-west-2.elb.amazonaws.com",
                "my-alb-123.us-west-2.elb.amazonaws.com",
                None,
                true,
            ),
            (
                "my-alb-123.us-west-2.elb.amazonaws.com:8080",
                "my-alb-123.us-west-2.elb.amazonaws.com",
                Some(8080),
                true,
            ),
            ("127.0.0.1:9297", "127.0.0.1", Some(9297), false),
            ("localhost:9297", "localhost", Some(9297), false),
            ("[::1]:9297", "[::1]", Some(9297), false),
            ("[::1]", "[::1]", None, false),
            ("user@s3.us-west-2.amazonaws.com", "s3.us-west-2.amazonaws.com", None, true),
        ];
        for (host, name, port, service) in cases {
            assert_eq!(
                host_only(host),
                *name,
                "case {host:?}: expected the name {name:?}, got {:?}",
                host_only(host)
            );
            assert_eq!(
                addressed_port(Some(host)),
                *port,
                "case {host:?}: expected the port {port:?}, got {:?}",
                addressed_port(Some(host))
            );
            assert_eq!(
                names_a_service(host),
                *service,
                "case {host:?}: expected names_a_service {service}, got {}",
                names_a_service(host)
            );
        }
    }
}
