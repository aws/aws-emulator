// SPDX-License-Identifier: CC0-1.0

//! The SOCKET a custom domain name is reached on, and which api stage a request made there
//! is served by.
//!
//! `CreateDomainName` mints an address a client DIALS: "the domain name associated with the
//! regional endpoint for this custom domain name" (`DomainNameConfiguration$
//! ApiGatewayDomainName`) is the value a caller points its own DNS record at, and what a
//! client then sends a request to. An `ApiMapping` of that domain name says what answers
//! there — "the API mapping key" and the `{ApiId, Stage}` it binds — so a request to the
//! domain, optionally under a mapping key, is served exactly as the api's own `execute-api`
//! endpoint serves it.
//!
//! A name under API Gateway's own zone resolves to nothing on the machine this emulator runs
//! on, so each domain name is given a socket of the EMULATOR's own
//! ([`crate::state::domain_name_transport`]) and publishes the `host:port` that socket
//! answers on. This module is the handler the core hands each connection accepted there:
//!
//! 1. encryption is negotiated in band, with the core's own TLS server
//!    ([`::service_common::in_band_tls`]), because the address is published for `https`
//!    callers and the handshake's first bytes have to be read by this socket to tell a TLS
//!    client from a plaintext one;
//! 2. the request is read, bounded and handed on by the one HTTP endpoint both core-owned
//!    sockets of this service share ([`super::websocket::endpoint_http`]);
//! 3. before it is handed on, it is RE-ADDRESSED: the api mapping whose base path the
//!    request's path begins with decides the api and stage, and the request is addressed at
//!    that stage's own invoke path on this emulator's front door — which is what makes the
//!    answer the stage's own, with its routes, authorizers, throttles, CORS configuration and
//!    integrations all applied by [`super::invoke`]. A path no mapping of the domain name
//!    answers to reaches no api at all and is refused `403`, the status API Gateway's front
//!    door turns such a request away with.
//!
//! The mapping is read PER REQUEST, so a mapping created or deleted after the connection was
//! opened decides the next request on it; and the domain name is read by the KEY its record is
//! stored under, which never changes, so the socket always serves the record a client dialled
//! the address of.

use std::pin::Pin;
use std::sync::Arc;

use ::emulator_server::{ListenerConn, NativeHandler, TlsUpgrader};
use ::service_common::in_band_tls::negotiated;
use ::smithy_server_runtime::EmbeddedEmulator;

use super::websocket::endpoint_http::{Addressed, Readdress};
use crate::state::ComAmazonawsApigatewayv2Apigatewayv2State as ServiceStore;

/// The host the re-addressed request names this emulator's front door by: the endpoint that
/// names THIS service, so the request resolves to the plane it is meant for. The api itself
/// is named by the reserved data-plane path, not by this host.
const SERVICE_ENDPOINT: &str = "apigateway.amazonaws.com";

/// The status API Gateway's front door turns a request away with when it reaches no api of
/// the account — here, a path no api mapping of the domain name answers to — and the sentence
/// it carries, which is the one every refusal of this service's data planes carries
/// (`super::data_plane::FORBIDDEN_MESSAGE`).
const FORBIDDEN: u16 = 403;
const FORBIDDEN_MESSAGE: &str = super::data_plane::FORBIDDEN_MESSAGE;

/// The status a request whose `Host` is not the name this address was published for is
/// answered with — the status for a request that reached a server unwilling to answer for the
/// authority it named (RFC 9110 §15.5.22) — and the sentence beside it.
const MISDIRECTED_REQUEST: u16 = 421;
const NOT_THIS_DOMAIN_NAME: &str = "Misdirected Request";

/// Open the socket `domain_name` is reached on, answering the socket itself and the
/// `host:port` it landed on — the address that record publishes as its `ApiGatewayDomainName`.
///
/// The core owns the socket, the interface, the port and the connection caps; this hands it
/// the handler and answers the HANDLE, which the caller remembers on this service's own store
/// once the record is written ([`crate::state::domain_name_transport`]) — so a create that is
/// refused after this point leaves nothing listening and displaces no socket another domain
/// name holds. Dropping the handle closes the port. `Err` when this service was mounted on an
/// engine with no listener surface, or when the core could not bind: the caller then falls
/// back to the name API Gateway's own zone would carry, which no client on this machine
/// resolves.
///
/// `callback` is the in-process transport a request accepted there is served over — the handle
/// the operation that mints the domain name is given (`OperationContext::callback`), kept for
/// the socket's whole life because a request arriving on it is served by this emulator's own
/// front door.
pub async fn open_for_domain_name<Eng: Send + Sync + 'static>(
    engine: &Eng,
    callback: EmbeddedEmulator,
    domain_name: &str,
) -> Result<Opened, String> {
    let shared = (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .ok_or_else(|| "this emulator's engine has no listener surface".to_string())?;
    let state = crate::state::service_state(engine)
        .ok_or_else(|| "this emulator's engine does not carry this service's store".to_string())?;
    let listeners = shared.listeners();
    let handler = DomainNameSocket {
        state: state.clone(),
        domain_name: domain_name.to_string(),
        callback,
        // The core's own TLS server, for a client that dialled `https`. A core that cannot
        // prepare it leaves this `None`, and such a client is then refused at the first byte
        // rather than being answered in the clear.
        upgrader: listeners.tls_upgrader().ok(),
    };
    // Opened on a task of its OWN, so a runtime that cannot bind a socket at all — one built
    // with no IO driver, which is what a host that drives this emulator in process may hand a
    // request — leaves this create without an address of its own instead of taking the request
    // down with it.
    let opened = ::tokio::spawn(async move { listeners.tcp_plaintext(handler).await }).await;
    let listener = match opened {
        Ok(Ok(listener)) => Arc::new(listener),
        Ok(Err(error)) => {
            return Err(format!(
                "the custom domain name's socket could not be opened: {error}"
            ))
        }
        Err(error) => {
            return Err(format!(
                "the custom domain name's socket could not be opened on this runtime: {error}"
            ))
        }
    };
    let authority = crate::state::domain_name_transport::authority_of(&listener);
    ::tracing::info!(
        %domain_name,
        %authority,
        "opened the socket a custom domain name is reached on"
    );
    Ok(Opened {
        authority,
        listener,
    })
}

/// The socket one custom domain name is reached on, before the record that owns it is written:
/// the address it landed on, and the handle whose life is that record's.
pub struct Opened {
    /// The `host:port` a client dials, which is what the record publishes.
    pub authority: String,
    /// The listener itself. Dropping it closes the port, which is what a create refused after
    /// the socket was opened does.
    pub listener: Arc<::emulator_server::ResourceListener>,
}

/// The handler the core hands each connection accepted on ONE custom domain name's socket.
///
/// It carries the shared store (so every request reads the records the control plane writes),
/// the key the domain name is stored under, and the in-process transport the request is served
/// over.
struct DomainNameSocket {
    state: ::engine::ServiceState<ServiceStore>,
    domain_name: String,
    callback: EmbeddedEmulator,
    upgrader: Option<TlsUpgrader>,
}

impl NativeHandler for DomainNameSocket {
    fn serve(&self, conn: ListenerConn) -> Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
        // Owned clones before the future: the core's handler contract hands back a `'static`
        // future, so nothing may borrow `&self` across it.
        let state = self.state.clone();
        let domain_name = self.domain_name.clone();
        let callback = self.callback.clone();
        let upgrader = self.upgrader.clone();
        Box::pin(async move {
            let Some(conn) = negotiated(conn, upgrader).await else {
                return;
            };
            let readdress: Readdress = Arc::new(move |parts: &::http::request::Parts| {
                invoke_target(&state, &domain_name, parts)
            });
            super::websocket::endpoint_http::serve_readdressed(
                callback,
                conn,
                Some(readdress),
            )
            .await;
        })
    }
}

/// Where a request `parts` addresses goes on the custom domain name `domain_name`: the stage
/// its api mapping names, addressed at this emulator's own front door, with the query string
/// the client sent carried on unchanged.
///
/// Two things are refused here, and nothing else is:
///
/// * a `Host` this socket does not stand for ([`addressed_to_this_domain_name`]). The address
///   is published for ONE domain name, and what a client sends as `Host` is the name it
///   resolved — so the name this socket serves, and the loopback address the socket itself
///   landed on, are the two fields it answers for;
/// * a path no api mapping of the record answers to ([`addressed_mapping`]), which reaches no
///   api of the account.
fn invoke_target(
    state: &::engine::ServiceState<ServiceStore>,
    domain_name: &str,
    parts: &::http::request::Parts,
) -> Addressed {
    if !addressed_to_this_domain_name(domain_name, parts) {
        return Addressed::Refused(MISDIRECTED_REQUEST, NOT_THIS_DOMAIN_NAME);
    }
    let store = state.lock();
    let Some(record) = store.domain_names.get(domain_name) else {
        return Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE);
    };
    addressed_mapping(record, &parts.uri)
}

/// Whether the `Host` of `parts` names what this socket stands for: the domain name it was
/// opened for (a wildcard record standing for one label under its own suffix included, as
/// `DomainName$DomainName` admits one), or the loopback address the socket landed on — which
/// is what a client that was handed the published `host:port` sends.
///
/// This is the socket's OWN reading of the field the core's HTTP listeners run an allowlist of
/// the emulator's own addresses over: a custom domain name exists to be addressed by a name of
/// the CUSTOMER's choosing, which no allowlist of this emulator's addresses can hold, so the
/// name this address was published for is what the field is checked against instead.
///
/// A DNS name is compared case-insensitively (RFC 4343) and without the port, which names the
/// address rather than the name.
fn addressed_to_this_domain_name(domain_name: &str, parts: &::http::request::Parts) -> bool {
    let host = parts
        .headers
        .get(::http::header::HOST)
        .and_then(|value| value.to_str().ok())
        .or_else(|| parts.uri.host())
        .map(::service_common::data_plane_addressing::host_only)
        .unwrap_or_default();
    if host.is_empty() {
        return false;
    }
    if ::emulator_server::is_loopback_host(host) || host.eq_ignore_ascii_case(domain_name) {
        return true;
    }
    // A wildcard domain name stands for ONE label under its own suffix: `*.example.com` is the
    // name `shop.example.com` resolves, and `a.b.example.com` is not under it.
    let Some(suffix) = domain_name.strip_prefix(WILDCARD_LABEL) else {
        return false;
    };
    if suffix.is_empty() {
        return false;
    }
    let under = host
        .strip_suffix(suffix)
        .and_then(|label| label.strip_suffix('.'));
    under.is_some_and(|label| !label.is_empty() && !label.contains('.'))
}

/// The leading label a wildcard custom domain name stands for every single label with.
const WILDCARD_LABEL: &str = "*.";

/// Where a request addressed at `target` goes on the record `record`: the PURE reading the
/// socket's decision above is made of, so what a request is re-addressed at is decided in one
/// place.
///
/// `Refused` for a path no mapping of the record answers to, and for a request whose path does
/// not render as one UNDER the reserved data-plane prefix of the api that mapping names — the
/// one thing a path the CLIENT chose must not be able to leave.
fn addressed_mapping(record: &crate::state::DomainName, target: &::http::Uri) -> Addressed {
    let Some((mapping, under)) = matching_mapping(record, target.path()) else {
        return Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE);
    };
    // The STAGE the mapping names, as the api's own invoke path spells it: a named stage is
    // reached with its name as the first path segment, and the stage that serves the api's
    // root ([`crate::state::DEFAULT_STAGE_NAME`]) with no stage segment at all — its name is
    // a reserved word of this service, never a path segment of a deployment.
    let stage = if mapping.stage == crate::state::DEFAULT_STAGE_NAME {
        String::new()
    } else {
        format!("/{}", mapping.stage)
    };
    let prefix = format!(
        "/{marker}/{label}/{api}",
        marker = crate::state::LOCAL_ENDPOINT_PREFIX,
        label = crate::state::EXECUTE_API_LABEL,
        api = mapping.api_id,
    );
    let query = target
        .query()
        .map(|query| format!("?{query}"))
        .unwrap_or_default();
    let uri = format!("https://{SERVICE_ENDPOINT}{prefix}{stage}/{under}{query}");
    // READ BACK off the rendered target: the path under the base path is the CLIENT's own, so
    // this is the assertion that nothing it carried moved the request to another api, another
    // plane of this process or another service.
    let stays_under = ::http::Uri::try_from(&uri)
        .map(|rendered| rendered.path().starts_with(&prefix))
        .unwrap_or(false);
    if !stays_under {
        return Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE);
    }
    Addressed::At {
        uri,
        host: SERVICE_ENDPOINT.to_string(),
    }
}

/// The api mapping of `record` that answers `path`, and the path left UNDER its base path.
///
/// "After the custom domain name is created, you can map an API to it" and the mapping key is
/// the base path a request carries after the host, so the LONGEST base path that is a prefix
/// of the request's path — on a segment boundary, since a base path is made of path segments
/// and is not a string prefix — is the mapping that serves it, and the mapping carrying NO key
/// (`ApiMapping::base_path`, the empty path) answers every path no other mapping claims.
///
/// A path carrying a DOT SEGMENT (`.` or `..`) answers to no mapping: such a segment is a
/// relative reference, and a remainder that could climb back out of the base path would select
/// an api the mapping does not name. The reading is the tree's one
/// (`service_common::data_plane_addressing::carries_a_dot_segment`), which resolves the
/// escapes before it splits the path, so a separator hidden in an escape is seen too.
fn matching_mapping<'r>(
    record: &'r crate::state::DomainName,
    path: &'r str,
) -> Option<(&'r crate::state::ApiMapping, &'r str)> {
    if ::service_common::data_plane_addressing::carries_a_dot_segment(path) {
        return None;
    }
    let requested = path.trim_start_matches('/');
    let mut best: Option<(&crate::state::ApiMapping, &str)> = None;
    for mapping in record.api_mappings.values() {
        let base = mapping.base_path().trim_matches('/');
        if base.is_empty() {
            continue;
        }
        let Some(under) = requested.strip_prefix(base) else {
            continue;
        };
        let remainder = match under.strip_prefix('/') {
            Some(remainder) => remainder,
            // The path IS the base path, with nothing under it.
            None if under.is_empty() => under,
            // A longer segment that merely starts with the base path is a different path.
            None => continue,
        };
        if best.is_none_or(|(held, _)| held.base_path().trim_matches('/').len() < base.len()) {
            best = Some((mapping, remainder));
        }
    }
    best.or_else(|| {
        record
            .api_mappings
            .values()
            .find(|mapping| mapping.base_path().trim_matches('/').is_empty())
            .map(|mapping| (mapping, requested))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One mapping of a domain name, as the store holds one.
    fn mapping(key: Option<&str>, api_id: &str, stage: &str) -> crate::state::ApiMapping {
        crate::state::ApiMapping {
            api_mapping_id: format!("map-{}", key.unwrap_or("root")),
            api_id: api_id.to_string(),
            api_mapping_key: key.map(str::to_string),
            stage: stage.to_string(),
        }
    }

    /// A domain name holding `mappings`.
    fn domain(mappings: Vec<crate::state::ApiMapping>) -> crate::state::DomainName {
        crate::state::DomainName {
            domain_name_arn: "arn:aws:apigateway:us-east-1::/domainnames/api.example.com"
                .to_string(),
            api_gateway_domain_name: "127.0.0.1:0".to_string(),
            region: "us-east-1".to_string(),
            domain_name_id: "abcdefghij".to_string(),
            domain_name: "api.example.com".to_string(),
            configurations: Vec::new(),
            mutual_tls_authentication: None,
            routing_mode: crate::state::DEFAULT_ROUTING_MODE.to_string(),
            tags: Default::default(),
            migration_due_at: None,
            api_mappings: mappings
                .into_iter()
                .map(|mapping| (mapping.api_mapping_id.clone(), mapping))
                .collect(),
            api_mapping_id_seed: 0,
            routing_rules: Default::default(),
            routing_rule_id_seed: 0,
        }
    }

    #[test]
    fn the_longest_matching_base_path_serves_the_request() {
        let record = domain(vec![
            mapping(None, "rootapi0000", "prod"),
            mapping(Some("shop"), "shopapi0000", "live"),
            mapping(Some("shop/eu"), "euapi000000", "$default"),
        ]);
        // The rows span what the rule generalises over: the root mapping, a one-segment key,
        // a two-segment key that is longer than another that also matches, the key itself
        // with nothing under it, a segment that merely starts with a key, a query string, and
        // a path with a dot segment in it — plainly and hidden in an escape.
        let cases: [(&str, &str, Option<&str>); 9] = [
            ("the root mapping", "/items", Some("rootapi0000/prod/items")),
            ("a one-segment key", "/shop/items", Some("shopapi0000/live/items")),
            (
                "the longer of two matching keys",
                "/shop/eu/items",
                Some("euapi000000/items"),
            ),
            ("the key with nothing under it", "/shop", Some("shopapi0000/live/")),
            (
                "a segment that merely starts with a key",
                "/shopping/items",
                Some("rootapi0000/prod/shopping/items"),
            ),
            ("the domain root itself", "/", Some("rootapi0000/prod/")),
            (
                "a query string under a key",
                "/shop/items?colour=red",
                Some("shopapi0000/live/items?colour=red"),
            ),
            ("a path carrying a dot segment", "/shop/../items", None),
            (
                // The shared reading resolves the escapes before it splits the path, so a
                // separator hidden in one is seen too.
                "a dot segment hidden in an escape",
                "/shop/%2e%2e%2fitems",
                None,
            ),
        ];
        for (case, target, expected) in cases {
            let target = ::http::Uri::try_from(target).expect("the row's target is a URI");
            let addressed = addressed_mapping(&record, &target);
            let expected = match expected {
                Some(under) => Addressed::At {
                    uri: format!(
                        "https://{SERVICE_ENDPOINT}/{}/{}/{under}",
                        crate::state::LOCAL_ENDPOINT_PREFIX,
                        crate::state::EXECUTE_API_LABEL,
                    ),
                    host: SERVICE_ENDPOINT.to_string(),
                },
                None => Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE),
            };
            assert_eq!(
                addressed, expected,
                "case {case:?}: expected {expected:?}, got {addressed:?}"
            );
        }
    }

    #[test]
    fn a_domain_name_with_no_mapping_serves_nothing() {
        let record = domain(Vec::new());
        for (case, path) in [("the domain root", "/"), ("a path under it", "/items")] {
            let target = ::http::Uri::try_from(path).expect("the row's target is a URI");
            let addressed = addressed_mapping(&record, &target);
            assert_eq!(
                addressed,
                Addressed::Refused(FORBIDDEN, FORBIDDEN_MESSAGE),
                "case {case:?}: a domain name with no mapping reaches no api, got {addressed:?}"
            );
        }
    }

    #[test]
    fn only_the_name_this_address_was_published_for_is_answered_for() {
        // The rows span the fields a client sends: the domain name itself, its spelling in
        // another case and with a port, the loopback address the socket landed on, a name of
        // another domain name, and the two forms a wildcard record stands for.
        let cases: [(&str, &str, &str, bool); 9] = [
            ("the domain name itself", "api.example.com", "api.example.com", true),
            ("another case of it", "api.example.com", "API.Example.COM", true),
            ("it with a port", "api.example.com", "api.example.com:8443", true),
            ("the address the socket landed on", "api.example.com", "127.0.0.1:34567", true),
            ("another domain name", "api.example.com", "other.example.com", false),
            ("a name that merely ends with it", "api.example.com", "evilapi.example.com", false),
            ("one label under a wildcard record", "*.example.com", "shop.example.com", true),
            ("two labels under it", "*.example.com", "a.shop.example.com", false),
            ("the wildcard's own suffix", "*.example.com", "example.com", false),
        ];
        for (case, domain_name, host, expected) in cases {
            let parts = ::http::Request::builder()
                .uri("/items")
                .header(::http::header::HOST, host)
                .body(())
                .expect("the row's request builds")
                .into_parts()
                .0;
            let actual = addressed_to_this_domain_name(domain_name, &parts);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected {domain_name:?} to answer for {host:?}: {expected}, \
                 got {actual}"
            );
        }
        // And a request naming no host at all names no domain name either.
        let parts = ::http::Request::builder()
            .uri("/items")
            .body(())
            .expect("the request builds")
            .into_parts()
            .0;
        assert!(
            !addressed_to_this_domain_name("api.example.com", &parts),
            "case {:?}: a request naming no host names no domain name",
            "no host at all"
        );
    }
}
