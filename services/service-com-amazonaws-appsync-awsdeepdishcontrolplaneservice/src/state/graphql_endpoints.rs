// SPDX-License-Identifier: CC0-1.0

//! WHERE a `GraphqlApi`'s two endpoints are PUBLISHED: the NAME each is announced under —
//! which is what `GraphqlApi$dns` ("The DNS records for the API") carries — and the URL a
//! client dials it at, which is what `GraphqlApi$uris` ("The URIs.") carries.
//!
//! The model constrains neither member (both are a `MapOfStringToString`), and both are read
//! by a client that then has to REACH the API, so what they have to satisfy is the addressing
//! AppSync publishes:
//!
//! * a `dns` record is a HOST and nothing else — no scheme, no port and no path, because that
//!   is all a DNS record can hold — and it is the host of the `uris` entry of the SAME
//!   endpoint, so the two members can never disagree about where that endpoint is;
//! * a `uris` entry is the URL over that host: the `GRAPHQL` endpoint an `https://` URL and
//!   the `REALTIME` one a `wss://` URL, both addressing the API's [`GRAPHQL_PATH`];
//! * both entries belong to ONE API and are published over the one host this emulator answers
//!   that API on, so the pair a caller reads is one API's GraphQL and real-time endpoint
//!   rather than two addresses with nothing in common.
//!
//! What this emulator substitutes for AWS's own hostnames is the address the endpoint is
//! SERVED at. AWS publishes `<apiId>.appsync-api.<region>.amazonaws.com` and
//! `<apiId>.appsync-realtime-api.<region>.amazonaws.com`, names a client of AWS resolves; no
//! client on this machine resolves either, and the whole purpose of both members is that a
//! client DIALS them, so each endpoint is published on the host the core bound the socket
//! serving it on ([`crate::service::graphql_api`]) — one of this emulator's own hosts — with
//! that socket's PORT carried by the `uris` entry, the member a client dials, and left out of
//! the `dns` record, which can hold no port.

use std::collections::BTreeMap;

use ::service_common::data_plane_addressing as addressing;

/// The path both of an API's endpoints serve: a client POSTs a GraphQL document to it on the
/// `GRAPHQL` endpoint and completes the WebSocket handshake against it on the `REALTIME` one.
pub const GRAPHQL_PATH: &str = "/graphql";

/// One PUBLISHED endpoint of a GraphQL API: the host it is announced on, and the port of the
/// socket that serves it.
///
/// The two are kept apart because the two published members carry different things: `dns`
/// carries the HOST alone and `uris` the URL over that host and the port. An endpoint whose
/// socket landed on no port carries none, and its URL then names the host's default port.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublishedEndpoint {
    /// The host this endpoint is announced on — the value `dns` reports for it. Held WITHOUT
    /// the brackets an IPv6 literal is written in inside a URL, because a DNS record carries
    /// the address and not a URL's punctuation; [`Self::authority`] puts them back.
    pub host: String,
    /// The port the socket serving this endpoint landed on.
    pub port: Option<u16>,
}

impl PublishedEndpoint {
    /// The authority a client dials this endpoint at: its host, with the port of the socket
    /// that serves it where there is one, and an IPv6 host bracketed as RFC 3986 requires
    /// ([`addressing::host_port_authority`]).
    #[must_use]
    pub fn authority(&self) -> String {
        match self.port {
            Some(port) => addressing::host_port_authority(&self.host, port),
            None => self.host.clone(),
        }
    }

    /// The URL this endpoint is published at: `scheme` over [`Self::authority`] and the API's
    /// [`GRAPHQL_PATH`] — the value `uris` reports for it.
    #[must_use]
    pub fn uri(&self, scheme: &str) -> String {
        format!("{scheme}://{}{GRAPHQL_PATH}", self.authority())
    }
}

/// The two endpoints one API publishes, keyed as AppSync keys them: the `GRAPHQL` endpoint on
/// the socket `graphql_socket` landed on and the `REALTIME` one on `realtime_socket`
/// (`host:port`, as the core reports a listener's address).
///
/// Both are endpoints of ONE API, and both are published on the host this emulator answers
/// that API on, so what a caller reads is that API's endpoint pair; what tells the two apart
/// is the socket each is served on, which the `uris` entry carries.
#[must_use]
pub fn api_endpoints(
    graphql_socket: &str,
    realtime_socket: &str,
) -> BTreeMap<String, PublishedEndpoint> {
    BTreeMap::from([
        (
            super::GRAPHQL_ENDPOINT.to_string(),
            served_endpoint(graphql_socket),
        ),
        (
            super::REALTIME_ENDPOINT.to_string(),
            served_endpoint(realtime_socket),
        ),
    ])
}

/// The endpoint served at `socket_authority`: the `host:port` the core reports for a listener,
/// read apart into the host the endpoint is announced on and the port a client dials it on.
#[must_use]
pub fn served_endpoint(socket_authority: &str) -> PublishedEndpoint {
    PublishedEndpoint {
        host: announced_host(socket_authority),
        port: socket_port(socket_authority),
    }
}

/// The `dns` records of a published pair: the HOST of each endpoint, which is what "The DNS
/// records for the API" holds and all a DNS record can hold.
#[must_use]
pub fn dns_of(endpoints: &BTreeMap<String, PublishedEndpoint>) -> BTreeMap<String, String> {
    endpoints
        .iter()
        .map(|(key, endpoint)| (key.clone(), endpoint.host.clone()))
        .collect()
}

/// The `uris` of a published pair: the URL of each endpoint over the host its own `dns` record
/// names, the real-time endpoint's a WebSocket URL and every other one an HTTPS URL.
#[must_use]
pub fn uris_of(
    endpoints: &BTreeMap<String, PublishedEndpoint>,
) -> std::collections::HashMap<String, String> {
    endpoints
        .iter()
        .map(|(key, endpoint)| (key.clone(), endpoint.uri(scheme_of(key))))
        .collect()
}

/// The scheme the endpoint keyed `endpoint` is addressed through: the real-time endpoint is
/// dialled as a WebSocket and every other endpoint of an API over HTTPS.
#[must_use]
pub fn scheme_of(endpoint: &str) -> &'static str {
    match endpoint == super::REALTIME_ENDPOINT {
        true => "wss",
        false => "https",
    }
}

/// The host an endpoint served at `socket_authority` is ANNOUNCED on: that authority's host
/// ([`addressing::host_only`]) with no port, and with the brackets an IPv6 literal carries
/// inside a URL removed, so the value is the address itself.
fn announced_host(socket_authority: &str) -> String {
    addressing::host_only(socket_authority)
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_string()
}

/// The port a core listener's authority names, or `None` for one that names none.
fn socket_port(socket_authority: &str) -> Option<u16> {
    addressing::addressed_port(Some(socket_authority)).and_then(|port| u16::try_from(port).ok())
}

/// The port `socket_authority` names, as a test row states one.
#[cfg(test)]
fn port_of(socket_authority: &str) -> String {
    socket_port(socket_authority).map_or_else(String::new, |port| port.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two keys AppSync publishes a GraphQL API's endpoints under, with the scheme each is
    /// addressed through. Source: `GraphqlApi$dns` ("The DNS records for the API") and
    /// `GraphqlApi$uris` ("The URIs."), as `crate::state::GRAPHQL_ENDPOINT` /
    /// `crate::state::REALTIME_ENDPOINT` name them.
    const KEYS: [(&str, &str); 2] = [
        (crate::state::GRAPHQL_ENDPOINT, "https"),
        (crate::state::REALTIME_ENDPOINT, "wss"),
    ];

    /// RULE: a `dns` record is the bare HOST the endpoint is served on — no scheme, no port
    /// and no path — and the `uris` entry of the same endpoint is the URL over that host and
    /// the port of the socket serving it.
    ///
    /// The rows span the forms a core listener's authority comes in: a loopback literal, the
    /// loopback name, a routable address, a bracketed IPv6 literal, and an authority naming no
    /// port at all.
    #[test]
    fn a_dns_record_is_the_bare_host_of_its_own_uri() {
        // (case, the authority the socket landed on, the host announced, the authority dialled)
        let rows: &[(&str, &str, &str, &str)] = &[
            ("a loopback literal", "127.0.0.1:43923", "127.0.0.1", "127.0.0.1:43923"),
            ("the loopback name", "localhost:8443", "localhost", "localhost:8443"),
            ("a routable address", "198.51.100.7:9297", "198.51.100.7", "198.51.100.7:9297"),
            ("a bracketed IPv6 literal", "[::1]:39643", "::1", "[::1]:39643"),
            ("an authority naming no port", "127.0.0.1", "127.0.0.1", "127.0.0.1"),
        ];
        for (case, socket, host, authority) in rows {
            let endpoints = api_endpoints(socket, socket);
            let dns = dns_of(&endpoints);
            let uris = uris_of(&endpoints);
            for (key, scheme) in KEYS {
                let published = dns.get(key).cloned().unwrap_or_default();
                assert_eq!(
                    published, *host,
                    "case {case:?}: expected dns.{key} {host:?}, got {published:?}"
                );
                assert!(
                    !published.contains('/') && !published.ends_with(&format!(":{}", port_of(socket))),
                    "case {case:?}: dns.{key} must carry no port and no path, got {published:?}"
                );
                let uri = uris.get(key).cloned().unwrap_or_default();
                let want = format!("{scheme}://{authority}{GRAPHQL_PATH}");
                assert_eq!(
                    uri, want,
                    "case {case:?}: expected uris.{key} {want:?}, got {uri:?}"
                );
            }
        }
    }

    /// RULE: the pair is ONE API's two endpoints — both published on the host this emulator
    /// answers that API on — so the two `dns` records name that one host whichever sockets the
    /// core assigned the endpoints.
    #[test]
    fn both_endpoints_of_one_api_are_published_on_one_host() {
        // (case, the GRAPHQL socket, the REALTIME socket)
        let rows: &[(&str, &str, &str)] = &[
            ("two loopback sockets", "127.0.0.1:43923", "127.0.0.1:39643"),
            ("two sockets on a routable address", "198.51.100.7:1", "198.51.100.7:65535"),
            ("two IPv6 sockets", "[::1]:43923", "[::1]:39643"),
        ];
        for (case, graphql, realtime) in rows {
            let endpoints = api_endpoints(graphql, realtime);
            let dns = dns_of(&endpoints);
            let graphql_host = dns
                .get(crate::state::GRAPHQL_ENDPOINT)
                .cloned()
                .unwrap_or_default();
            let realtime_host = dns
                .get(crate::state::REALTIME_ENDPOINT)
                .cloned()
                .unwrap_or_default();
            assert_eq!(
                graphql_host, realtime_host,
                "case {case:?}: both endpoints of one API are announced on one host, got \
                 {graphql_host:?} and {realtime_host:?}"
            );
            let uris = uris_of(&endpoints);
            for (key, scheme) in KEYS {
                let uri = uris.get(key).cloned().unwrap_or_default();
                let dialled = addressing::url_authority(&uri)
                    .strip_suffix(GRAPHQL_PATH)
                    .unwrap_or_default()
                    .to_string();
                let host = announced_host(&dialled);
                assert!(
                    uri.starts_with(&format!("{scheme}://")) && host == graphql_host,
                    "case {case:?}: uris.{key} must be the {scheme} URL over {graphql_host:?} \
                     and the API's path, got {uri:?}"
                );
            }
        }
    }
}
